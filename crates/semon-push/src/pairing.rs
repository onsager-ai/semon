//! Unwired `public_tls` pairing core. Callers must validate normal TLS chain trust,
//! hostname and redirects before entering this protocol. TLS exporters are supplied
//! by the transport, never taken from a request field. Origin MAC always goes first.
//! A mistyped hostname hands that host the code: endpoint/exporter binding and the
//! PBKDF2 cost bound what it can do, but PAKE is the real fix, on a later track.
//!
//! Injected Unix seconds are clamped to the latest persisted/observed time. A
//! backwards clock cannot reopen a window. Session codes and pending state exist
//! only in memory. The gate and registry share Store's exclusive lock. Share Core
//! behind a mutex: reserve before releasing it to do expensive verification.
//! Cross-machine persistence is not atomic. Origin consent must be durable before
//! activation; this library accepts verified signature evidence, not signatures.
use crate::{
    credentials::{Bearer, Kind, Secret},
    enrollment::{self, Binding, CertificatePin, Enrollment, Error, KindScope, Registry, Store},
};
use ring::{hkdf, hmac, pbkdf2};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Error>;
pub const GATE_FILE: &str = "pairing-gate.json";
pub const GATE_CAP: u64 = 4096;
pub const FORMAT: u32 = 1;
pub const PBKDF2_ITERATIONS: u32 = 600_000;
pub const WINDOW: u64 = 600;
pub const LIFETIME: u64 = 1200;
const HOUR: u64 = 3600;
const PAUSE: u64 = 1800;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const WEIGHTS: [u8; 9] = [1, 3, 5, 7, 9, 11, 13, 15, 17];

fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
    let mut bytes = Zeroizing::new([0; N]);
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(bytes.as_mut())
        .map_err(|_| Error::Random)?;
    Ok(bytes)
}
fn check(data: &[u8]) -> u8 {
    data.iter()
        .zip(WEIGHTS)
        .fold(0u16, |sum, (v, w)| sum + u16::from(*v) * u16::from(w)) as u8
        & 31
}
/// Pure local input validation. Only one optional middle hyphen is accepted.
/// Aliases are normalized before checking; U and non-ASCII input are refused.
pub fn parse_code(input: &str) -> Result<Secret> {
    let input = input.trim();
    if input.len() != 10 && !(input.len() == 11 && input.as_bytes()[5] == b'-') {
        return Err(Error::Invalid);
    }
    let mut values = Zeroizing::new(Vec::with_capacity(10));
    for (i, byte) in input.bytes().enumerate() {
        if input.len() == 11 && i == 5 {
            continue;
        }
        let byte = match byte.to_ascii_uppercase() {
            b'O' => b'0',
            b'I' | b'L' => b'1',
            b => b,
        };
        values.push(
            ALPHABET
                .iter()
                .position(|b| *b == byte)
                .ok_or(Error::Invalid)? as u8,
        );
    }
    if check(&values[..9]) != values[9] {
        return Err(Error::Invalid);
    }
    Ok(Secret::new(
        values
            .iter()
            .map(|v| ALPHABET[*v as usize] as char)
            .collect(),
    ))
}
fn generate_code() -> Result<Secret> {
    // Masking uniform independent bytes yields unbiased five-bit symbols.
    let mut values = random::<9>()?;
    for v in values.iter_mut() {
        *v &= 31;
    }
    let mut code: String = values
        .iter()
        .map(|v| ALPHABET[*v as usize] as char)
        .collect();
    code.push(ALPHABET[check(values.as_ref()) as usize] as char);
    Ok(Secret::new(code))
}
/// Explicit local-only display operation; the returned wrapper still redacts.
pub fn display_code(code: &Secret) -> Secret {
    Secret::new(format!("{}-{}", &code.expose()[..5], &code.expose()[5..]))
}

// Public protocol containers deliberately redact their entire Debug output.
macro_rules! redact { ($($name:ty),+ $(,)?) => { $(impl std::fmt::Debug for $name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(concat!(stringify!($name), "(<redacted>)")) }
})+ }; }
#[derive(Clone, PartialEq, Eq)]
pub struct Transcript {
    pub version: u32,
    pub session_id: String,
    /// Canonical advertised URL: https://host:port (including explicit default port).
    pub endpoint: String,
    pub exporter: Vec<u8>,
    pub origin_key: Vec<u8>,
    pub scope: [u8; 32],
    pub origin_nonce: [u8; 32],
    pub receiver_nonce: [u8; 32],
    pub origin_role: String,
    pub receiver_role: String,
    pub profile: String,
}
fn fields(parts: &[&[u8]]) -> Vec<u8> {
    let mut bytes = vec![];
    for part in parts {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    bytes
}
/// Normalize scheme, host and port. Paths, userinfo, queries and fragments refuse.
pub fn normalize_endpoint(input: &str) -> Result<String> {
    let url = reqwest::Url::parse(input).map_err(|_| Error::Invalid)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Invalid);
    }
    let host = url.host_str().ok_or(Error::Invalid)?;
    Ok(format!(
        "https://{host}:{}",
        url.port_or_known_default().ok_or(Error::Invalid)?
    ))
}
impl Transcript {
    fn validate(&self) -> Result<()> {
        if self.version != FORMAT
            || self.profile != "public_tls"
            || self.origin_role != "origin"
            || self.receiver_role != "receiver"
            || self.endpoint != normalize_endpoint(&self.endpoint)?
            || self.exporter.is_empty()
            || self.exporter.len() > 256
            || self.origin_key.is_empty()
            || self.origin_key.len() > 4096
            || self.session_id.len() != 64
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn prefix(&self) -> Vec<u8> {
        fields(&[
            &self.version.to_be_bytes(),
            self.session_id.as_bytes(),
            &self.origin_nonce,
            &self.receiver_nonce,
            self.origin_role.as_bytes(),
            self.receiver_role.as_bytes(),
        ])
    }
    fn bytes(&self) -> Vec<u8> {
        fields(&[
            &self.prefix(),
            self.endpoint.as_bytes(),
            &self.exporter,
            &self.origin_key,
            &self.scope,
            self.profile.as_bytes(),
        ])
    }
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.bytes()).into()
    }
}
pub struct Keys {
    transcript_digest: [u8; 32],
    origin: Zeroizing<[u8; 32]>,
    receiver: Zeroizing<[u8; 32]>,
    application: Zeroizing<[u8; 32]>,
}
fn expand(prk: &hkdf::Prk, label: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let mut output = Zeroizing::new([0; 32]);
    prk.expand(&[label], hkdf::HKDF_SHA256)
        .map_err(|_| Error::Invalid)?
        .fill(output.as_mut())
        .map_err(|_| Error::Invalid)?;
    Ok(output)
}
pub fn derive_keys(code: &Secret, transcript: &Transcript) -> Result<Keys> {
    transcript.validate()?;
    let code = parse_code(code.expose())?;
    let mut stretched = Zeroizing::new([0; 32]);
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PBKDF2_ITERATIONS).ok_or(Error::Invalid)?,
        &transcript.prefix(),
        code.expose().as_bytes(),
        stretched.as_mut(),
    );
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"semon-pairing-v1").extract(stretched.as_ref());
    Ok(Keys {
        transcript_digest: transcript.digest(),
        origin: expand(&prk, b"origin-confirmation")?,
        receiver: expand(&prk, b"receiver-confirmation")?,
        application: expand(&prk, b"application")?,
    })
}
pub struct Mac([u8; 32]);
fn mac(key: &[u8], transcript: &Transcript) -> Mac {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), &transcript.bytes());
    let mut bytes = [0; 32];
    bytes.copy_from_slice(tag.as_ref());
    Mac(bytes)
}
fn verify(key: &[u8], transcript: &Transcript, proof: &Mac) -> Result<()> {
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        &transcript.bytes(),
        &proof.0,
    )
    .map_err(|_| Error::Invalid)
}
impl Mac {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}
impl Keys {
    pub fn origin_confirmation(&self, transcript: &Transcript) -> Mac {
        mac(self.origin.as_ref(), transcript)
    }
    /// Origin must call this before presenting B4 or using recovery material.
    pub fn confirm_receiver(self, transcript: &Transcript, proof: &Mac) -> Result<ConfirmedKey> {
        if self.transcript_digest != transcript.digest() {
            return Err(Error::Invalid);
        }
        verify(self.receiver.as_ref(), transcript, proof)?;
        Ok(ConfirmedKey {
            application: self.application,
            transcript: transcript.clone(),
        })
    }
}
pub struct ConfirmedKey {
    application: Zeroizing<[u8; 32]>,
    transcript: Transcript,
}
impl ConfirmedKey {
    /// Recovery material is exposed only within the provisioning implementation.
    pub(crate) fn application_key(&self) -> &[u8; 32] {
        &self.application
    }
    #[cfg(test)]
    pub(crate) fn recovery_key_for_test(&self) -> &[u8; 32] {
        &self.application
    }

    /// Receiver recovery only. Caller must verify a fresh connection-bound origin
    /// signature; deliver() independently matches all persisted receipt hashes.
    pub(crate) fn recovered(application: Zeroizing<[u8; 32]>, transcript: Transcript) -> Self {
        Self {
            application,
            transcript,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Pending,
    Confirmed,
    Approved,
    Active,
    Rejected,
    Cancelled,
    Expired,
    Revoked,
}
struct Session {
    binding: String,
    created: u64,
    confirmed: Option<u64>,
    state: State,
    code: Option<Secret>,
    nonce: [u8; 32],
    attempts: u8,
    transcript: Option<Transcript>,
    application: Option<Zeroizing<[u8; 32]>>,
}
pub struct Created {
    pub session_id: String,
    pub receiver_nonce: [u8; 32],
    pub code: Secret,
}
/// Reserved under the Core mutex before any KDF work, and never refunded. A
/// disconnected/dropped ticket still spends its attempt. No Clone or serializer.
pub struct Attempt {
    session_id: String,
    number: u8,
    transcript: Transcript,
    code: Secret,
}
pub struct VerifiedAttempt {
    session_id: String,
    number: u8,
    transcript: Transcript,
    keys: Keys,
}
impl Attempt {
    pub fn verify(self, origin_mac: &Mac) -> Result<VerifiedAttempt> {
        let keys = derive_keys(&self.code, &self.transcript)?;
        verify(keys.origin.as_ref(), &self.transcript, origin_mac)?;
        Ok(VerifiedAttempt {
            session_id: self.session_id,
            number: self.number,
            transcript: self.transcript,
            keys,
        })
    }
}
/// Evidence constructed ONLY after the caller verifies the origin's signature on
/// the exact session/transcript and B4 summary digest using this public key.
pub struct VerifiedApproval {
    pub session_id: String,
    pub origin_key: Vec<u8>,
    pub transcript_digest: [u8; 32],
    pub summary_digest: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    format: u32,
    latest: u64,
    creations: Vec<u64>,
    burns: Vec<u64>,
    pause_until: u64,
}
impl Gate {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let gate: Self = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        if gate.format != FORMAT {
            return Err(Error::UnsupportedVersion);
        }
        if bytes.len() as u64 > GATE_CAP
            || gate.creations.len() > 6
            || gate.burns.len() > 12
            || gate
                .creations
                .iter()
                .chain(&gate.burns)
                .any(|t| *t > gate.latest)
            || gate
                .creations
                .windows(2)
                .chain(gate.burns.windows(2))
                .any(|w| w[0] > w[1])
            || gate.pause_until > gate.latest.saturating_add(PAUSE)
            || (gate.burns.len() >= 3
                && gate.pause_until
                    < gate
                        .burns
                        .last()
                        .copied()
                        .unwrap_or(0)
                        .saturating_add(PAUSE))
        {
            return Err(Error::Invalid);
        }
        Ok(gate)
    }
    fn observe(&mut self, now: u64) -> u64 {
        self.latest = self.latest.max(now);
        self.creations
            .retain(|t| self.latest.saturating_sub(*t) < HOUR);
        self.burns.retain(|t| self.latest.saturating_sub(*t) < HOUR);
        self.latest
    }
}
pub struct Core {
    store: Store,
    endpoint: String,
    receiver_id: String,
    gate: Gate,
    sessions: BTreeMap<String, Session>,
    gate_unavailable: bool,
}
impl Core {
    /// Fresh registry fields populate origin_read::EnrollmentScope; no extra
    /// pairing-side identity state is required by the later broker adapter.
    pub fn registry(&self) -> Result<Registry> {
        self.store.snapshot()
    }
    pub fn open(store: Store, endpoint: &str, receiver_id: String) -> Result<Self> {
        let store = store.into_pairing_owner()?;
        let gate = store
            .read_pairing_file()
            .and_then(|bytes| Gate::parse(&bytes))
            .map_err(|_| Error::GateUnavailable)?;
        Ok(Self {
            store,
            endpoint: normalize_endpoint(endpoint)?,
            receiver_id,
            gate,
            sessions: BTreeMap::new(),
            gate_unavailable: false,
        })
    }
    fn observe(&mut self, now: u64) -> Result<u64> {
        if self.gate_unavailable {
            return Err(Error::GateUnavailable);
        }
        // Never discard unsaved gate changes after any persistence failure.
        let loaded = self
            .store
            .read_pairing_file()
            .and_then(|bytes| Gate::parse(&bytes));
        let mut gate = match loaded {
            Ok(gate) => gate,
            Err(_) => {
                self.gate_unavailable = true;
                return Err(Error::GateUnavailable);
            }
        };
        gate.latest = gate.latest.max(self.gate.latest);
        let now = gate.observe(now);
        self.gate = gate;
        self.save_gate()?;
        for session in self.sessions.values_mut() {
            if matches!(
                session.state,
                State::Pending | State::Confirmed | State::Approved
            ) && (now.saturating_sub(session.created) >= LIFETIME
                || (session.state == State::Pending
                    && now.saturating_sub(session.created) >= WINDOW)
                || (session.state == State::Confirmed
                    && now.saturating_sub(session.confirmed.unwrap_or(session.created)) >= WINDOW))
            {
                session.state = State::Expired;
                session.code = None;
                session.application = None;
            }
        }
        Ok(now)
    }
    fn save_gate(&mut self) -> Result<()> {
        if self.gate_unavailable {
            return Err(Error::GateUnavailable);
        }
        let result = serde_json::to_vec(&self.gate)
            .map_err(|_| Error::Invalid)
            .and_then(|bytes| self.store.write_pairing_file(&bytes));
        if result.is_err() {
            self.gate_unavailable = true;
            return Err(Error::GateUnavailable);
        }
        Ok(())
    }
    pub fn create_session(&mut self, connection_binding: String, now: u64) -> Result<Created> {
        let now = self.observe(now)?;
        if connection_binding.is_empty()
            || connection_binding.len() > 256
            || now < self.gate.pause_until
            || self.gate.creations.len() >= 6
            || self
                .sessions
                .values()
                .filter(|s| matches!(s.state, State::Pending | State::Confirmed | State::Approved))
                .count()
                >= 4
            || self.sessions.values().any(|s| {
                s.binding == connection_binding
                    && matches!(s.state, State::Pending | State::Confirmed | State::Approved)
            })
        {
            return Err(Error::InUse);
        }
        // Terminal pending outcomes are non-authoritative and bounded by one
        // absolute lifetime; committed consumption remains in registry receipts.
        self.sessions
            .retain(|_, s| now.saturating_sub(s.created) < LIFETIME);
        let session_id = enrollment::random_id()?;
        let code = generate_code()?;
        let nonce = *random::<32>()?;
        self.gate.creations.push(now);
        self.save_gate()?;
        self.sessions.insert(
            session_id.clone(),
            Session {
                binding: connection_binding,
                created: now,
                confirmed: None,
                state: State::Pending,
                code: Some(Secret::new(code.expose().into())),
                nonce,
                attempts: 0,
                transcript: None,
                application: None,
            },
        );
        Ok(Created {
            session_id,
            receiver_nonce: nonce,
            code,
        })
    }
    pub fn state(&mut self, id: &str, now: u64) -> Result<State> {
        self.observe(now)?;
        self.sessions.get(id).map(|s| s.state).ok_or(Error::Invalid)
    }
    pub fn reserve(
        &mut self,
        connection_binding: &str,
        transcript: Transcript,
        now: u64,
    ) -> Result<Attempt> {
        let now = self.observe(now)?;
        let session = self
            .sessions
            .get_mut(&transcript.session_id)
            .ok_or(Error::Invalid)?;
        // A request cannot charge another connection's session. Malformed input
        // on a known connection still spends an attempt; unknown ids spend none.
        if session.binding != connection_binding || session.state != State::Pending {
            return Err(Error::Terminal);
        }
        if session.attempts == 3 {
            session.state = State::Rejected;
            session.code = None;
            self.gate.burns.push(now);
            if self.gate.burns.len() >= 3 {
                self.gate.pause_until = now.saturating_add(PAUSE);
            }
            self.save_gate()?;
            return Err(Error::Terminal);
        }
        session.attempts += 1;
        transcript.validate()?;
        if transcript.endpoint != self.endpoint || transcript.receiver_nonce != session.nonce {
            return Err(Error::Invalid);
        }
        Ok(Attempt {
            session_id: transcript.session_id.clone(),
            number: session.attempts,
            transcript,
            code: Secret::new(
                session
                    .code
                    .as_ref()
                    .ok_or(Error::Terminal)?
                    .expose()
                    .into(),
            ),
        })
    }
    /// No receiver MAC exists until Attempt::verify has verified the origin MAC.
    pub fn confirm(&mut self, verified: VerifiedAttempt, now: u64) -> Result<Mac> {
        let now = self.observe(now)?;
        let session = self
            .sessions
            .get_mut(&verified.session_id)
            .ok_or(Error::Invalid)?;
        if session.state != State::Pending || verified.number > session.attempts {
            return Err(Error::Terminal);
        }
        let response = mac(verified.keys.receiver.as_ref(), &verified.transcript);
        session.state = State::Confirmed;
        session.confirmed = Some(now);
        session.code = None;
        session.transcript = Some(verified.transcript);
        session.application = Some(verified.keys.application);
        Ok(response)
    }
    pub fn approve(&mut self, evidence: VerifiedApproval, now: u64) -> Result<()> {
        self.observe(now)?;
        let session = self
            .sessions
            .get_mut(&evidence.session_id)
            .ok_or(Error::Invalid)?;
        let t = session.transcript.as_ref().ok_or(Error::Invalid)?;
        if session.state != State::Confirmed
            || t.origin_key != evidence.origin_key
            || t.digest() != evidence.transcript_digest
            || t.scope != evidence.summary_digest
        {
            return Err(Error::Terminal);
        }
        session.state = State::Approved;
        Ok(())
    }
    pub fn cancel(&mut self, id: &str, now: u64) -> Result<()> {
        self.observe(now)?;
        let session = self.sessions.get_mut(id).ok_or(Error::Invalid)?;
        if !matches!(
            session.state,
            State::Pending | State::Confirmed | State::Approved
        ) {
            return Err(Error::Terminal);
        }
        session.state = State::Cancelled;
        session.code = None;
        session.application = None;
        Ok(())
    }
}

/// Immutable receipt lives in the SAME atomic registry document as enrollment
/// and hashes. No pending session, code, application key or raw bearer is stored.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub(crate) session_id: String,
    pub(crate) enrollment_id: String,
    origin_key_hash: [u8; 32],
    transcript_digest: [u8; 32],
    application_hash: [u8; 32],
    created: u64,
    valid_until: u64,
    pub(crate) deliveries: u8,
}
pub(crate) fn validate_receipts(registry: &Registry) -> Result<()> {
    let mut ids = BTreeSet::new();
    for r in &registry.pairing_receipts {
        if r.session_id.len() != 64
            || !ids.insert(&r.session_id)
            || r.deliveries > 2
            || r.created >= r.valid_until
            || !registry
                .enrollments
                .iter()
                .any(|e| e.enrollment_id == r.enrollment_id)
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationInput {
    pub receiver_id: String,
    pub machine: String,
    pub binding: Binding,
    pub kinds: Vec<(Kind, bool)>,
    pub client_certificate_pins: Vec<CertificatePin>,
    pub valid_until: u64,
}
impl ActivationInput {
    /// Bind the exact enrollment inputs in the intended-scope/B4 commitment.
    pub fn commitment(&self) -> [u8; 32] {
        Sha256::digest(serde_json::to_vec(self).expect("serializable enrollment inputs")).into()
    }
}
pub struct Delivery {
    pub enrollment_id: String,
    pub credentials: Vec<(Kind, Secret)>,
}
fn kind_label(kind: Kind) -> &'static [u8] {
    match kind {
        Kind::AnalyticsPublish => b"analytics_publish",
        Kind::BodyChannel => b"body_channel",
        Kind::MirrorPush => b"mirror_push",
    }
}
fn bearer_secret(application: &[u8], id: &str, kind: Kind) -> Secret {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, application),
        &fields(&[b"semon-pairing-bearer-v1", id.as_bytes(), kind_label(kind)]),
    );
    Secret::new(hex::encode(tag.as_ref()))
}
impl Core {
    /// Commit once, then use deliver() over the confirmed channel. A crash before
    /// commit leaves no enrollment. A crash after commit is recovered with the
    /// origin-held ConfirmedKey and signature-verified evidence. Restart destroys
    /// pending sessions but not consumed receipts. A delivery reservation is
    /// durable BEFORE returning secrets: one initial delivery and one recovery,
    /// never a second enrollment. After two interrupted deliveries, revoke.
    pub fn activate(&mut self, id: &str, input: ActivationInput, now: u64) -> Result<Delivery> {
        self.commit_activation(id, input, now, true)
    }
    fn commit_activation(
        &mut self,
        id: &str,
        input: ActivationInput,
        now: u64,
        deliver: bool,
    ) -> Result<Delivery> {
        let now = self.observe(now)?;
        let session = self.sessions.get_mut(id).ok_or(Error::Invalid)?;
        if session.state != State::Approved {
            return Err(Error::Terminal);
        }
        let t = session.transcript.as_ref().ok_or(Error::Invalid)?;
        let application = session.application.as_ref().ok_or(Error::Invalid)?;
        if input.commitment() != t.scope {
            return Err(Error::Invalid);
        }
        let mut snapshot = self.store.snapshot()?; // fresh under lock; no stale cached authority
        if snapshot.pairing_receipts.iter().any(|r| r.session_id == id) {
            session.state = State::Active;
            return Err(Error::Terminal);
        }
        let enrollment_id = enrollment::random_id()?;
        let kinds: Vec<_> = input
            .kinds
            .into_iter()
            .map(|(kind, required)| {
                let mut k = KindScope::new(kind, 1);
                k.client_cert_required = required;
                k
            })
            .collect();
        let enrollment = Enrollment {
            enrollment_id: enrollment_id.clone(),
            receiver_id: self.receiver_id.clone(),
            machine: input.machine,
            binding: input.binding,
            client_certificate_pins: input.client_certificate_pins,
            credential_kinds: kinds,
            revoked_at: None,
        };
        let mut credentials = vec![];
        for scope in &enrollment.credential_kinds {
            let secret = bearer_secret(application.as_ref(), &enrollment_id, scope.kind);
            snapshot.credentials.push(Bearer {
                credential_id: enrollment::random_id()?,
                enrollment_id: enrollment_id.clone(),
                kind: scope.kind,
                sha256: secret.hash(),
                generation: 1,
                valid_from: now,
                valid_until: input.valid_until,
                revoked_at: None,
            });
            if deliver {
                credentials.push((scope.kind, secret));
            }
        }
        let added_bearers: Vec<_> = snapshot
            .credentials
            .iter()
            .filter(|b| b.enrollment_id == enrollment_id)
            .cloned()
            .collect();
        snapshot.enrollments.push(enrollment.clone());
        let receipt = Receipt {
            session_id: id.into(),
            enrollment_id: enrollment_id.clone(),
            origin_key_hash: Sha256::digest(&t.origin_key).into(),
            transcript_digest: t.digest(),
            application_hash: Sha256::digest(application.as_ref()).into(),
            created: now,
            valid_until: input.valid_until,
            deliveries: u8::from(deliver),
        };
        self.store
            .pairing_transaction(snapshot.revision(), |fresh| {
                if fresh.pairing_receipts.iter().any(|r| r.session_id == id) {
                    return Err(Error::Terminal);
                }
                fresh.enrollments.push(enrollment);
                fresh.credentials.extend(added_bearers);
                fresh.pairing_receipts.push(receipt);
                Ok(())
            })?;
        session.state = State::Active;
        session.application = None;
        Ok(Delivery {
            enrollment_id,
            credentials,
        })
    }
    /// Caller has verified a fresh private-key signature binding this request to
    /// the receipt and confirmed transcript. Recheck current revocation before
    /// each delivery. Recovery never reactivates or resets revoked authority.
    pub fn deliver(
        &mut self,
        key: &ConfirmedKey,
        evidence: VerifiedApproval,
        now: u64,
    ) -> Result<Delivery> {
        let now = self.observe(now)?;
        let revision = self.store.snapshot()?.revision();
        self.store.pairing_transaction(revision, |snapshot| {
            let receipt = snapshot
                .pairing_receipts
                .iter_mut()
                .find(|r| r.session_id == evidence.session_id)
                .ok_or(Error::Invalid)?;
            if evidence.session_id != key.transcript.session_id
                || evidence.origin_key != key.transcript.origin_key
                || evidence.transcript_digest != key.transcript.digest()
                || evidence.summary_digest != key.transcript.scope
                || receipt.origin_key_hash != <[u8; 32]>::from(Sha256::digest(&evidence.origin_key))
                || receipt.transcript_digest != evidence.transcript_digest
                || receipt.application_hash
                    != <[u8; 32]>::from(Sha256::digest(key.application.as_ref()))
                || receipt.deliveries >= 2
                || now >= receipt.valid_until
            {
                return Err(Error::Terminal);
            }
            let enrollment = snapshot
                .enrollments
                .iter()
                .find(|e| e.enrollment_id == receipt.enrollment_id)
                .ok_or(Error::Invalid)?;
            if enrollment.revoked_at.is_some()
                || enrollment
                    .credential_kinds
                    .iter()
                    .any(|k| k.revoked_at.is_some() || k.generations.current != 1)
                || snapshot
                    .credentials
                    .iter()
                    .filter(|b| b.enrollment_id == receipt.enrollment_id)
                    .any(|b| b.revoked_at.is_some() || now >= b.valid_until)
            {
                return Err(Error::Terminal);
            }
            let delivery = Delivery {
                enrollment_id: receipt.enrollment_id.clone(),
                credentials: enrollment
                    .credential_kinds
                    .iter()
                    .map(|k| {
                        (
                            k.kind,
                            bearer_secret(key.application.as_ref(), &receipt.enrollment_id, k.kind),
                        )
                    })
                    .collect(),
            };
            receipt.deliveries += 1;
            Ok(delivery)
        })
    }
}
redact!(
    Transcript,
    Keys,
    Mac,
    ConfirmedKey,
    Session,
    Created,
    Attempt,
    VerifiedAttempt,
    VerifiedApproval,
    Gate,
    Core,
    Receipt,
    ActivationInput,
    Delivery
);
#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;

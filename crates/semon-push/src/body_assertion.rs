//! Pure Ed25519 identity assertions; no transport, enrollment or key custody.
//! Callers pin issuer keys, authenticate subjects, generate unpredictable nonces
//! and bindings, and keep one replay cache for all concurrent grants at an origin.

use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use std::{collections::BTreeMap, fmt};
use zeroize::Zeroizing;

const DOMAIN: &[u8] = b"semon/body-assertion/v1\0";
pub const VERSION: u8 = 1;
pub const MAX_LIFETIME: u64 = 60;
pub const MAX_SKEW: u64 = 5;

/// Internal audit classification. Contains no assertion values or key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Version,
    Field,
    Lifetime,
    Future,
    Expired,
    Deadline,
    Audience,
    Binding,
    UnknownKey,
    Signature,
    Replay,
    Capacity,
    Clock,
    Key,
}
/// Every failure has the same external kind and redacted formatting. An adapter
/// may inspect `reason` for private audit classification, never a remote response.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Refused(Reason);
impl Refused {
    /// Allows an external key handle to report a signing failure without details.
    pub fn signing_failed() -> Self {
        Self(Reason::Key)
    }
    pub fn reason(self) -> Reason {
        self.0
    }
}
impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("identity assertion refused")
    }
}
impl fmt::Debug for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for Refused {}
type Result<T> = std::result::Result<T, Refused>;

#[derive(Clone, PartialEq, Eq)]
pub struct Audience {
    pub origin_incarnation: String,
    pub namespace: String,
}
/// Values are validated at sign and verify. Identifiers use only ASCII letters,
/// digits, `-_.:@`; no whitespace, paths or arbitrary free text. All identifiers
/// are 1..=128 bytes, except subject principal (1..=256).
#[derive(Clone)]
pub struct Claims {
    pub issuer_key_id: String,
    pub issued_at: u64,
    pub expiry: u64,
    pub audience: Audience,
    pub claim_epoch: u64,
    pub nonce: [u8; 32],
    pub subject_principal: String,
    pub machine: String,
    pub namespace: String,
    pub acl_grant_version: String,
    pub receiver_policy_version: String,
    pub request_digest: [u8; 32],
    pub deadline: u64,
    pub channel_binding: [u8; 32],
}
/// In-memory library value, deliberately not a wire format. No Debug prints
/// identifiers, signatures or subject data.
#[derive(Clone)]
pub struct Assertion {
    pub version: u8,
    pub claims: Claims,
    pub signature: [u8; 64],
}

/// Implementations may delegate to an independently managed signing key. The
/// handle must sign these exact bytes with Ed25519, without prehashing.
pub trait SigningKey {
    fn sign(&self, message: &[u8]) -> Result<[u8; 64]>;
}
/// Minimal memory-only seed holder. Our seed is zeroized on drop; ring's
/// temporary expanded key has no public zeroization guarantee. No key-file API.
pub struct MemoryKey(Zeroizing<[u8; 32]>);
impl MemoryKey {
    /// Takes ownership of seed storage; callers must clear any other seed copies.
    pub fn from_seed(seed: Zeroizing<[u8; 32]>) -> Self {
        Self(seed)
    }
    fn key(&self) -> Result<Ed25519KeyPair> {
        Ed25519KeyPair::from_seed_unchecked(self.0.as_ref()).map_err(|_| Refused(Reason::Key))
    }
    pub fn public_key(&self) -> Result<[u8; 32]> {
        self.key()?
            .public_key()
            .as_ref()
            .try_into()
            .map_err(|_| Refused(Reason::Key))
    }
}
impl SigningKey for MemoryKey {
    fn sign(&self, message: &[u8]) -> Result<[u8; 64]> {
        self.key()?
            .sign(message)
            .as_ref()
            .try_into()
            .map_err(|_| Refused(Reason::Key))
    }
}

/// Epoch-second clock supplied by the caller; no I/O or wall-clock access here.
/// Skew only tolerates a slightly future issued-at, never extends expiry/deadline.
pub fn sign(claims: Claims, key: &dyn SigningKey, now: impl FnOnce() -> u64) -> Result<Assertion> {
    validate(&claims, now(), 0)?;
    let signature = key.sign(&encoding(VERSION, &claims))?;
    Ok(Assertion {
        version: VERSION,
        claims,
        signature,
    })
}

/// Verification context is local policy, not supplied by an untrusted assertion.
pub struct Expected<'a> {
    pub audience: &'a Audience,
    pub channel_binding: &'a [u8; 32],
    /// Maximum absolute request deadline allowed by current origin policy.
    pub deadline: u64,
    pub skew: u64,
}

/// Bounded in-memory, non-sliding nonce cache. Caller serializes verification and
/// scopes this cache to an origin incarnation. Restart must rotate the channel
/// binding. Accepted nonces stay until expiry; saturation refuses new grants.
pub struct ReplayCache {
    cap: usize,
    nonces: BTreeMap<[u8; 32], u64>,
    last_now: Option<u64>,
}
impl ReplayCache {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            nonces: BTreeMap::new(),
            last_now: None,
        }
    }
    fn admit(&mut self, nonce: [u8; 32], expiry: u64, now: u64) -> Result<()> {
        if self.last_now.is_some_and(|last| now < last) {
            return Err(Refused(Reason::Clock));
        }
        self.last_now = Some(now);
        self.nonces.retain(|_, expiry| *expiry > now);
        if self.nonces.contains_key(&nonce) {
            return Err(Refused(Reason::Replay));
        }
        if self.nonces.len() >= self.cap {
            return Err(Refused(Reason::Capacity));
        }
        self.nonces.insert(nonce, expiry);
        Ok(())
    }
}

/// Verifies and atomically consumes a nonce before returning claims. Key lookup
/// supplies a pinned public key; assertions never supply their own trust root.
/// Successful verification is identity evidence, not an origin policy grant.
pub fn verify<'a>(
    assertion: &'a Assertion,
    lookup: impl FnOnce(&str) -> Option<[u8; 32]>,
    expected: &Expected<'_>,
    replay: &mut ReplayCache,
    now: impl FnOnce() -> u64,
) -> Result<&'a Claims> {
    if assertion.version != VERSION {
        return Err(Refused(Reason::Version));
    }
    let now = now();
    let claims = &assertion.claims;
    validate(claims, now, expected.skew)?;
    if &claims.audience != expected.audience {
        return Err(Refused(Reason::Audience));
    }
    if &claims.channel_binding != expected.channel_binding {
        return Err(Refused(Reason::Binding));
    }
    if claims.deadline > expected.deadline {
        return Err(Refused(Reason::Deadline));
    }
    let key = lookup(&claims.issuer_key_id).ok_or(Refused(Reason::UnknownKey))?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&encoding(assertion.version, claims), &assertion.signature)
        .map_err(|_| Refused(Reason::Signature))?;
    replay.admit(claims.nonce, claims.expiry, now)?;
    Ok(claims)
}

fn validate(claims: &Claims, now: u64, skew: u64) -> Result<()> {
    for (field, cap) in [
        (claims.issuer_key_id.as_str(), 128),
        (claims.audience.origin_incarnation.as_str(), 128),
        (claims.audience.namespace.as_str(), 128),
        (claims.subject_principal.as_str(), 256),
        (claims.machine.as_str(), 128),
        (claims.namespace.as_str(), 128),
        (claims.acl_grant_version.as_str(), 128),
        (claims.receiver_policy_version.as_str(), 128),
    ] {
        if field.is_empty()
            || field.len() > cap
            || !field
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:@".contains(&b))
        {
            return Err(Refused(Reason::Field));
        }
    }
    if claims.expiry <= claims.issued_at || claims.expiry - claims.issued_at > MAX_LIFETIME {
        return Err(Refused(Reason::Lifetime));
    }
    if skew > MAX_SKEW || claims.issued_at > now.saturating_add(skew) {
        return Err(Refused(Reason::Future));
    }
    if now >= claims.expiry {
        return Err(Refused(Reason::Expired));
    }
    if claims.deadline <= claims.issued_at
        || claims.deadline > claims.expiry
        || now >= claims.deadline
    {
        return Err(Refused(Reason::Deadline));
    }
    Ok(())
}

/// Exact canonical signing bytes followed by the Ed25519 signature.
pub fn encode(assertion: &Assertion) -> Vec<u8> {
    let mut bytes = encoding(assertion.version, &assertion.claims);
    bytes.extend_from_slice(&assertion.signature);
    bytes
}
/// Decode only the canonical bounded representation. Signature and live claim
/// validation still require verify(); decoding never establishes authority.
pub fn decode(bytes: &[u8]) -> Result<Assertion> {
    fn take<'a>(input: &mut &'a [u8]) -> Result<&'a [u8]> {
        let n = input.get(..4).ok_or(Refused(Reason::Field))?;
        let n = u32::from_be_bytes(n.try_into().map_err(|_| Refused(Reason::Field))?) as usize;
        let field = input
            .get(4..4usize.checked_add(n).ok_or(Refused(Reason::Field))?)
            .ok_or(Refused(Reason::Field))?;
        *input = &input[4 + n..];
        Ok(field)
    }
    fn string(input: &mut &[u8]) -> Result<String> {
        String::from_utf8(take(input)?.to_vec()).map_err(|_| Refused(Reason::Field))
    }
    fn integer(input: &mut &[u8]) -> Result<u64> {
        Ok(u64::from_be_bytes(
            take(input)?
                .try_into()
                .map_err(|_| Refused(Reason::Field))?,
        ))
    }
    if bytes.len() > 4096 || !bytes.starts_with(DOMAIN) {
        return Err(Refused(Reason::Field));
    }
    let mut input = &bytes[DOMAIN.len()..];
    let version_field = take(&mut input)?;
    let [version] = version_field else {
        return Err(Refused(Reason::Field));
    };
    let version = *version;
    let claims = Claims {
        issuer_key_id: string(&mut input)?,
        issued_at: integer(&mut input)?,
        expiry: integer(&mut input)?,
        audience: Audience {
            origin_incarnation: string(&mut input)?,
            namespace: string(&mut input)?,
        },
        claim_epoch: integer(&mut input)?,
        nonce: take(&mut input)?
            .try_into()
            .map_err(|_| Refused(Reason::Field))?,
        subject_principal: string(&mut input)?,
        machine: string(&mut input)?,
        namespace: string(&mut input)?,
        acl_grant_version: string(&mut input)?,
        receiver_policy_version: string(&mut input)?,
        request_digest: take(&mut input)?
            .try_into()
            .map_err(|_| Refused(Reason::Field))?,
        deadline: integer(&mut input)?,
        channel_binding: take(&mut input)?
            .try_into()
            .map_err(|_| Refused(Reason::Field))?,
    };
    let signature = input.try_into().map_err(|_| Refused(Reason::Field))?;
    let assertion = Assertion {
        version,
        claims,
        signature,
    };
    if version != VERSION || encode(&assertion) != bytes {
        return Err(Refused(Reason::Field));
    }
    Ok(assertion)
}

// Canonical v1: literal domain, then every field (including version and integers)
// in this fixed order with a big-endian u32 byte length. Integers are big-endian
// fixed width; strings are ASCII. No JSON, concatenation ambiguity or native endian.
fn encoding(version: u8, c: &Claims) -> Vec<u8> {
    let mut out = DOMAIN.to_vec();
    for field in [
        &[version][..],
        c.issuer_key_id.as_bytes(),
        &c.issued_at.to_be_bytes(),
        &c.expiry.to_be_bytes(),
        c.audience.origin_incarnation.as_bytes(),
        c.audience.namespace.as_bytes(),
        &c.claim_epoch.to_be_bytes(),
        &c.nonce,
        c.subject_principal.as_bytes(),
        c.machine.as_bytes(),
        c.namespace.as_bytes(),
        c.acl_grant_version.as_bytes(),
        c.receiver_policy_version.as_bytes(),
        &c.request_digest,
        &c.deadline.to_be_bytes(),
        &c.channel_binding,
    ] {
        out.extend_from_slice(&(field.len() as u32).to_be_bytes());
        out.extend_from_slice(field);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> MemoryKey {
        MemoryKey::from_seed(Zeroizing::new([7; 32]))
    }
    fn claims() -> Claims {
        Claims {
            issuer_key_id: "key-1".into(),
            issued_at: 100,
            expiry: 160,
            audience: Audience {
                origin_incarnation: "origin-1".into(),
                namespace: "work".into(),
            },
            claim_epoch: 3,
            nonce: [1; 32],
            subject_principal: "user:alice".into(),
            machine: "laptop".into(),
            namespace: "work".into(),
            acl_grant_version: "acl-1".into(),
            receiver_policy_version: "policy-1".into(),
            request_digest: [2; 32],
            deadline: 159,
            channel_binding: [3; 32],
        }
    }
    fn check<'a>(a: &'a Assertion, cache: &mut ReplayCache, now: u64) -> Result<&'a Claims> {
        let c = claims();
        verify(
            a,
            |_| Some(key().public_key().unwrap()),
            &Expected {
                audience: &c.audience,
                channel_binding: &c.channel_binding,
                deadline: 160,
                skew: 2,
            },
            cache,
            || now,
        )
    }
    fn signed(c: Claims) -> Assertion {
        sign(c, &key(), || 100).unwrap()
    }
    fn reason(result: Result<&Claims>) -> Reason {
        result.err().unwrap().reason()
    }
    #[test]
    fn transport_encoding_roundtrip_and_refusals() {
        let a = signed(claims());
        let encoded = encode(&a);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(encode(&decoded), encoded);
        assert!(check(&decoded, &mut ReplayCache::new(1), 100).is_ok());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        assert!(decode(&encoded[..encoded.len() - 1]).is_err());
        assert!(decode(&vec![0; 4097]).is_err());
    }
    #[test]
    fn roundtrip_and_replay_capacity_no_sliding() {
        let a = signed(claims());
        let mut cache = ReplayCache::new(1);
        assert!(check(&a, &mut cache, 100).is_ok());
        assert_eq!(reason(check(&a, &mut cache, 101)), Reason::Replay);
        let mut c = claims();
        c.nonce = [4; 32];
        let b = signed(c);
        assert_eq!(reason(check(&b, &mut cache, 102)), Reason::Capacity);
        assert_eq!(cache.nonces[&[1; 32]], 160);
        let mut c = claims();
        c.issued_at = 160;
        c.expiry = 200;
        c.deadline = 190;
        let next = sign(c, &key(), || 160).unwrap();
        let expected = Expected {
            audience: &next.claims.audience,
            channel_binding: &next.claims.channel_binding,
            deadline: 200,
            skew: 0,
        };
        assert!(
            verify(
                &next,
                |_| Some(key().public_key().unwrap()),
                &expected,
                &mut cache,
                || 160
            )
            .is_ok()
        );
        assert_eq!(reason(check(&b, &mut cache, 103)), Reason::Clock);
    }
    #[test]
    fn every_signed_field_tamper_refuses() {
        let a = signed(claims());
        let mutations: [fn(&mut Assertion); 17] = [
            |a| a.version += 1,
            |a| a.claims.issuer_key_id.push('x'),
            |a| a.claims.issued_at -= 1,
            |a| a.claims.expiry -= 1,
            |a| a.claims.audience.origin_incarnation.push('x'),
            |a| a.claims.audience.namespace.push('x'),
            |a| a.claims.claim_epoch += 1,
            |a| a.claims.nonce[0] ^= 1,
            |a| a.claims.subject_principal.push('x'),
            |a| a.claims.machine.push('x'),
            |a| a.claims.namespace.push('x'),
            |a| a.claims.acl_grant_version.push('x'),
            |a| a.claims.receiver_policy_version.push('x'),
            |a| a.claims.request_digest[0] ^= 1,
            |a| a.claims.deadline -= 1,
            |a| a.claims.channel_binding[0] ^= 1,
            |a| a.signature[0] ^= 1,
        ];
        for mutate in mutations {
            let mut b = a.clone();
            mutate(&mut b);
            assert!(check(&b, &mut ReplayCache::new(1), 100).is_err());
        }
    }
    #[test]
    fn time_lifetime_and_field_caps_at_both_boundaries() {
        let a = signed(claims());
        assert_eq!(
            reason(check(&a, &mut ReplayCache::new(1), 160)),
            Reason::Expired
        );
        assert_eq!(
            reason(check(&a, &mut ReplayCache::new(1), 97)),
            Reason::Future
        );
        assert!(check(&a, &mut ReplayCache::new(1), 98).is_ok());
        assert_eq!(
            reason(check(&a, &mut ReplayCache::new(1), 159)),
            Reason::Deadline
        );
        for (c, expected) in [
            (
                {
                    let mut c = claims();
                    c.expiry = 161;
                    c
                },
                Reason::Lifetime,
            ),
            (
                {
                    let mut c = claims();
                    c.expiry = 100;
                    c
                },
                Reason::Lifetime,
            ),
            (
                {
                    let mut c = claims();
                    c.issued_at = 101;
                    c
                },
                Reason::Future,
            ),
            (
                {
                    let mut c = claims();
                    c.deadline = 161;
                    c
                },
                Reason::Deadline,
            ),
            (
                {
                    let mut c = claims();
                    c.machine = "x".repeat(129);
                    c
                },
                Reason::Field,
            ),
            (
                {
                    let mut c = claims();
                    c.subject_principal = "x".repeat(257);
                    c
                },
                Reason::Field,
            ),
            (
                {
                    let mut c = claims();
                    c.namespace = "../private/path".into();
                    c
                },
                Reason::Field,
            ),
            (
                {
                    let mut c = claims();
                    c.issuer_key_id.clear();
                    c
                },
                Reason::Field,
            ),
        ] {
            assert_eq!(
                sign(c.clone(), &key(), || 100).err().unwrap().reason(),
                expected
            );
            let invalid = Assertion {
                signature: key().sign(&encoding(VERSION, &c)).unwrap(),
                version: VERSION,
                claims: c,
            };
            // Verification allows two seconds of future issued-at skew.
            let now = if expected == Reason::Future { 97 } else { 100 };
            assert_eq!(
                reason(check(&invalid, &mut ReplayCache::new(1), now)),
                expected
            );
        }
    }
    #[test]
    fn key_audience_binding_and_local_deadline_refusals() {
        let a = signed(claims());
        let c = claims();
        let mut cache = ReplayCache::new(8);
        let mut e = Expected {
            audience: &c.audience,
            channel_binding: &c.channel_binding,
            deadline: 160,
            skew: 0,
        };
        assert_eq!(
            reason(verify(&a, |_| None, &e, &mut cache, || 100)),
            Reason::UnknownKey
        );
        let other = MemoryKey::from_seed(Zeroizing::new([8; 32]));
        assert_eq!(
            reason(verify(
                &a,
                |_| Some(other.public_key().unwrap()),
                &e,
                &mut cache,
                || 100
            )),
            Reason::Signature
        );
        let audience = Audience {
            origin_incarnation: "other".into(),
            namespace: "work".into(),
        };
        e.audience = &audience;
        assert_eq!(
            reason(verify(
                &a,
                |_| Some(key().public_key().unwrap()),
                &e,
                &mut cache,
                || 100
            )),
            Reason::Audience
        );
        e.audience = &c.audience;
        e.channel_binding = &[9; 32];
        assert_eq!(
            reason(verify(
                &a,
                |_| Some(key().public_key().unwrap()),
                &e,
                &mut cache,
                || 100
            )),
            Reason::Binding
        );
        e.channel_binding = &c.channel_binding;
        e.deadline = 150;
        assert_eq!(
            reason(verify(
                &a,
                |_| Some(key().public_key().unwrap()),
                &e,
                &mut cache,
                || 100
            )),
            Reason::Deadline
        );
        assert!(cache.nonces.is_empty());
    }
    #[test]
    fn canonical_domain_determinism_and_boundaries() {
        let mut a = claims();
        a.machine = "ab".into();
        a.namespace = "c".into();
        let mut b = a.clone();
        b.machine = "a".into();
        b.namespace = "bc".into();
        assert_eq!(encoding(VERSION, &a), encoding(VERSION, &a.clone()));
        assert_ne!(encoding(VERSION, &a), encoding(VERSION, &b));
        assert!(encoding(VERSION, &a).starts_with(DOMAIN));
        assert_eq!(
            &encoding(VERSION, &a)[DOMAIN.len()..DOMAIN.len() + 5],
            &[0, 0, 0, 1, 1]
        );
        let mut wrong_domain = encoding(VERSION, &a);
        wrong_domain[0] ^= 1;
        let assertion = Assertion {
            signature: key().sign(&wrong_domain).unwrap(),
            version: VERSION,
            claims: a,
        };
        assert_eq!(
            reason(check(&assertion, &mut ReplayCache::new(1), 100)),
            Reason::Signature
        );
        assert_eq!(signed(claims()).signature, signed(claims()).signature);
        assert_eq!(
            format!("{:?}", Refused(Reason::Signature)),
            format!("{}", Refused(Reason::UnknownKey))
        );
    }
}

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

use chacha20poly1305::aead::{OsRng, rand_core::RngCore};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::identity::fingerprint;

pub const REPLAY_WINDOW_MS: u64 = 5 * 60 * 1_000;
// This holds more than three five-minute windows at the measured M1 rate of
// about 260 requests/s. Fixed-size replay keys plus timestamps use about 28 MiB
// before hash-table and allocator overhead.
const MAX_REPLAY_NONCES: usize = 262_144;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedHeaders {
    pub machine: String,
    pub timestamp: u64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("machine {0} is not enrolled")]
    Unenrolled(String),
    #[error("request timestamp is outside the five-minute replay window")]
    Stale,
    #[error("request nonce must be 16 bytes of hex")]
    Nonce,
    #[error("request signature must be 64 bytes of hex")]
    SignatureEncoding,
    #[error("request signature is invalid")]
    Signature,
    #[error("request nonce was already used within the replay window")]
    Replay,
    #[error("request replay cache is full; retry after an in-window entry expires")]
    ReplayCacheFull,
    #[error("signed machine does not match the machine in the request body")]
    MachineMismatch,
}

#[derive(Clone)]
pub struct RequestSigner {
    signing: SigningKey,
    machine: String,
}

impl RequestSigner {
    pub fn new(signing: SigningKey) -> Self {
        let machine = fingerprint(&signing.verifying_key());
        Self { signing, machine }
    }

    pub fn machine(&self) -> &str {
        &self.machine
    }

    pub fn sign(&self, method: &str, path: &str, body: &[u8]) -> Result<SignedHeaders, AuthError> {
        self.sign_at(method, path, body, now_ms()?, random_nonce())
    }

    pub fn sign_at(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        timestamp: u64,
        nonce: [u8; 16],
    ) -> Result<SignedHeaders, AuthError> {
        let nonce = hex::encode(nonce);
        let message = signing_message(method, path, body, timestamp, &nonce);
        Ok(SignedHeaders {
            machine: self.machine.clone(),
            timestamp,
            nonce,
            signature: hex::encode(self.signing.sign(&message).to_bytes()),
        })
    }
}

pub struct RequestVerifier {
    machines: BTreeMap<String, VerifyingKey>,
    seen: HashSet<ReplayKey>,
    order: VecDeque<(u64, ReplayKey)>,
    max_replay_nonces: usize,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct ReplayKey {
    machine: [u8; 32],
    nonce: [u8; 16],
}

impl RequestVerifier {
    pub fn new(machines: impl IntoIterator<Item = (String, VerifyingKey)>) -> Self {
        Self {
            machines: machines.into_iter().collect(),
            seen: HashSet::new(),
            order: VecDeque::new(),
            max_replay_nonces: MAX_REPLAY_NONCES,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.machines.is_empty()
    }

    /// Rejects requests that cannot possibly authenticate without reading the body.
    pub fn precheck(&mut self, headers: &SignedHeaders, now_ms: u64) -> Result<(), AuthError> {
        let key = self
            .machines
            .get(&headers.machine)
            .ok_or_else(|| AuthError::Unenrolled(headers.machine.clone()))?;
        if now_ms.abs_diff(headers.timestamp) > REPLAY_WINDOW_MS {
            return Err(AuthError::Stale);
        }
        let nonce = decode_nonce(&headers.nonce)?;
        decode_signature(&headers.signature)?;
        let replay_key = ReplayKey {
            machine: Sha256::digest(key.as_bytes()).into(),
            nonce,
        };
        self.expire(now_ms);
        if self.seen.contains(&replay_key) {
            return Err(AuthError::Replay);
        }
        if self.seen.len() >= self.max_replay_nonces {
            return Err(AuthError::ReplayCacheFull);
        }
        Ok(())
    }

    pub fn verify(
        &mut self,
        method: &str,
        path: &str,
        body: &[u8],
        headers: &SignedHeaders,
        body_machine: &str,
        now_ms: u64,
    ) -> Result<(), AuthError> {
        self.precheck(headers, now_ms)?;
        let key = self
            .machines
            .get(&headers.machine)
            .expect("precheck found the enrolled machine");
        let signature = decode_signature(&headers.signature)?;
        key.verify_strict(
            &signing_message(method, path, body, headers.timestamp, &headers.nonce),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| AuthError::Signature)?;
        let replay_machine = Sha256::digest(key.as_bytes()).into();
        let nonce = decode_nonce(&headers.nonce)?;
        if headers.machine != body_machine {
            return Err(AuthError::MachineMismatch);
        }
        let replay_key = ReplayKey {
            machine: replay_machine,
            nonce,
        };
        self.seen.insert(replay_key);
        self.order.push_back((now_ms, replay_key));
        Ok(())
    }

    fn expire(&mut self, now_ms: u64) {
        while self
            .order
            .front()
            .is_some_and(|(seen_at, _)| now_ms.saturating_sub(*seen_at) > REPLAY_WINDOW_MS)
        {
            self.remove_oldest();
        }
    }

    fn remove_oldest(&mut self) {
        if let Some((_, replay_key)) = self.order.pop_front() {
            self.seen.remove(&replay_key);
        }
    }

    #[cfg(test)]
    fn with_replay_capacity(
        machines: impl IntoIterator<Item = (String, VerifyingKey)>,
        max_replay_nonces: usize,
    ) -> Self {
        let mut verifier = Self::new(machines);
        verifier.max_replay_nonces = max_replay_nonces;
        verifier
    }
}

fn decode_nonce(value: &str) -> Result<[u8; 16], AuthError> {
    decode_fixed(value).map_err(|_| AuthError::Nonce)
}

fn decode_signature(value: &str) -> Result<[u8; 64], AuthError> {
    decode_fixed(value).map_err(|_| AuthError::SignatureEncoding)
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N], hex::FromHexError> {
    let mut bytes = [0_u8; N];
    hex::decode_to_slice(value, &mut bytes)?;
    Ok(bytes)
}

pub fn signing_message(
    method: &str,
    path: &str,
    body: &[u8],
    timestamp: u64,
    nonce: &str,
) -> Vec<u8> {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        method,
        path,
        hex::encode(Sha256::digest(body)),
        timestamp,
        nonce
    )
    .into_bytes()
}

fn random_nonce() -> [u8; 16] {
    let mut nonce = [0_u8; 16];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

fn now_ms() -> Result<u64, AuthError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AuthError::Clock)?
        .as_millis()
        .min(u64::MAX as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_cache_fails_closed_until_entries_expire() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let signer = RequestSigner::new(signing.clone());
        let machine = signer.machine().to_owned();
        let body = format!(r#"{{"machine":"{machine}"}}"#);
        let now = 1_000_000;
        let mut verifier =
            RequestVerifier::with_replay_capacity([(machine.clone(), signing.verifying_key())], 2);

        let first = signer
            .sign_at("POST", "/v1/test", body.as_bytes(), now, [1; 16])
            .unwrap();
        let second = signer
            .sign_at("POST", "/v1/test", body.as_bytes(), now, [2; 16])
            .unwrap();
        for headers in [&first, &second] {
            verifier
                .verify("POST", "/v1/test", body.as_bytes(), headers, &machine, now)
                .unwrap();
        }

        let refused = signer
            .sign_at("POST", "/v1/test", body.as_bytes(), now, [3; 16])
            .unwrap();
        assert!(matches!(
            verifier.verify("POST", "/v1/test", body.as_bytes(), &refused, &machine, now,),
            Err(AuthError::ReplayCacheFull)
        ));
        assert!(matches!(
            verifier.verify("POST", "/v1/test", body.as_bytes(), &first, &machine, now,),
            Err(AuthError::Replay)
        ));

        let later = now + REPLAY_WINDOW_MS + 1;
        assert!(matches!(
            verifier.verify("POST", "/v1/test", body.as_bytes(), &first, &machine, later,),
            Err(AuthError::Stale)
        ));
        let fresh = signer
            .sign_at("POST", "/v1/test", body.as_bytes(), later, [3; 16])
            .unwrap();
        verifier
            .verify("POST", "/v1/test", body.as_bytes(), &fresh, &machine, later)
            .unwrap();
        let reused_after_expiry = signer
            .sign_at("POST", "/v1/test", body.as_bytes(), later, [1; 16])
            .unwrap();
        verifier
            .verify(
                "POST",
                "/v1/test",
                body.as_bytes(),
                &reused_after_expiry,
                &machine,
                later,
            )
            .unwrap();
        assert!(matches!(
            verifier.verify(
                "POST",
                "/v1/test",
                body.as_bytes(),
                &reused_after_expiry,
                &machine,
                later,
            ),
            Err(AuthError::Replay)
        ));
    }
}

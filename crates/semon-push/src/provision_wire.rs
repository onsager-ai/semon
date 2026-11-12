//! Shared public-TLS provisioning messages. Codes and TLS exporters have no wire
//! fields. Transport supplies the exporter from the live TLS 1.3 connection.
use crate::pairing::{self, Mac, Transcript};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_MESSAGE_BYTES: usize = 8192;
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-semon-asi-pairing-v1";
pub const EXPORTER_LENGTH: usize = 32;
pub const READ_SECONDS: u64 = 10;
pub const MAX_CONNECTIONS: usize = 8;
/// Initial validity ceiling; renewal PR4 may revise this reviewed parameter.
pub const INITIAL_CREDENTIAL_VALIDITY_SECONDS: u64 = 7_776_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Open,
    Confirm,
    Approve,
    Cancel,
    Recover,
}
impl Route {
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "/v1/provision/open" => Some(Self::Open),
            "/v1/provision/confirm" => Some(Self::Confirm),
            "/v1/provision/approve" => Some(Self::Approve),
            "/v1/provision/cancel" => Some(Self::Cancel),
            "/v1/provision/recover" => Some(Self::Recover),
            _ => None,
        }
    }
}

/// All parse and protocol failures use one external refusal class. Do not put
/// serde errors or pairing errors (including session existence) on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Open {
    pub version: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opened {
    pub version: u32,
    pub session_id: String,
    pub receiver_nonce: String,
    pub endpoint: String,
    pub receiver_id: String,
    pub issuer_key_id: String,
    pub issuer_public_key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirm {
    pub version: u32,
    pub session_id: String,
    pub origin_key: String,
    pub origin_nonce: String,
    pub scope_commitment: String,
    pub origin_mac: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmed {
    pub version: u32,
    pub receiver_mac: String,
}
/// The signed transcript digest already commits the exact B4 summary digest.
/// The receiver must validate the signature before constructing VerifiedApproval.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approve {
    pub version: u32,
    pub session_id: String,
    pub proof: String,
    pub input: pairing::ActivationInput,
}
/// Original canonical transcript, including its original connection exporter.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTranscript {
    pub session_id: String,
    pub endpoint: String,
    pub exporter: String,
    pub origin_key: String,
    pub scope_commitment: String,
    pub origin_nonce: String,
    pub receiver_nonce: String,
}
impl RecoveryTranscript {
    pub fn rebuild(&self) -> Result<Transcript, Refused> {
        session_id(&self.session_id)?;
        if pairing::normalize_endpoint(&self.endpoint).map_err(|_| Refused)? != self.endpoint {
            return Err(Refused);
        }
        Ok(Transcript {
            version: VERSION,
            session_id: self.session_id.clone(),
            endpoint: self.endpoint.clone(),
            exporter: decode_fixed::<32>(&self.exporter)?.to_vec(),
            origin_key: decode_fixed::<65>(&self.origin_key)?.to_vec(),
            scope: decode_fixed(&self.scope_commitment)?,
            origin_nonce: decode_fixed(&self.origin_nonce)?,
            receiver_nonce: decode_fixed(&self.receiver_nonce)?,
            origin_role: "origin".into(),
            receiver_role: "receiver".into(),
            profile: "public_tls".into(),
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recover {
    pub version: u32,
    pub transcript: RecoveryTranscript,
    pub application_key: String,
    pub proof: String,
}
impl Drop for Recover {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.application_key);
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Activated {
    pub version: u32,
    pub enrollment_id: String,
    pub credentials: Vec<(crate::credentials::Kind, String)>,
}
impl Drop for Activated {
    fn drop(&mut self) {
        for (_, secret) in &mut self.credentials {
            zeroize::Zeroize::zeroize(secret);
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub version: u32,
    pub session_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancelled {
    pub version: u32,
}

pub enum Request {
    Open(Open),
    Confirm(Confirm),
    Approve(Approve),
    Cancel(Cancel),
    Recover(Recover),
}
impl Request {
    pub fn parse(route: Route, bytes: &[u8]) -> Result<Self, Refused> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(Refused);
        }
        let request = match route {
            Route::Open => Self::Open(serde_json::from_slice(bytes).map_err(|_| Refused)?),
            Route::Confirm => Self::Confirm(serde_json::from_slice(bytes).map_err(|_| Refused)?),
            Route::Approve => Self::Approve(serde_json::from_slice(bytes).map_err(|_| Refused)?),
            Route::Cancel => Self::Cancel(serde_json::from_slice(bytes).map_err(|_| Refused)?),
            Route::Recover => Self::Recover(serde_json::from_slice(bytes).map_err(|_| Refused)?),
        };
        match &request {
            Self::Open(m) if m.version == VERSION => {}
            Self::Recover(m) if m.version == VERSION => {
                m.transcript.rebuild()?;
                decode_fixed::<32>(&m.application_key)?;
                decode_fixed::<64>(&m.proof)?;
            }
            Self::Confirm(m) if m.version == VERSION => {
                session_id(&m.session_id)?;
                let key = decode_fixed::<65>(&m.origin_key)?;
                if key[0] != 4 {
                    return Err(Refused);
                }
                decode_fixed::<32>(&m.origin_nonce)?;
                decode_fixed::<32>(&m.scope_commitment)?;
                decode_fixed::<32>(&m.origin_mac)?;
            }
            Self::Approve(m) if m.version == VERSION => {
                session_id(&m.session_id)?;
                decode_fixed::<64>(&m.proof)?;
            }
            Self::Cancel(m) if m.version == VERSION => session_id(&m.session_id)?,
            _ => return Err(Refused),
        }
        Ok(request)
    }
}

pub fn encode_message(message: &impl Serialize) -> Result<Vec<u8>, Refused> {
    let bytes = serde_json::to_vec(message).map_err(|_| Refused)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(Refused);
    }
    Ok(bytes)
}
fn session_id(id: &str) -> Result<(), Refused> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Refused);
    }
    Ok(())
}
impl Confirm {
    pub fn transcript(
        &self,
        endpoint: String,
        receiver_nonce: [u8; 32],
        exporter: [u8; EXPORTER_LENGTH],
    ) -> Result<Transcript, Refused> {
        Ok(Transcript {
            version: self.version,
            session_id: self.session_id.clone(),
            endpoint,
            exporter: exporter.to_vec(),
            origin_key: decode_fixed::<65>(&self.origin_key)?.to_vec(),
            scope: decode_fixed(&self.scope_commitment)?,
            origin_nonce: decode_fixed(&self.origin_nonce)?,
            receiver_nonce,
            origin_role: "origin".into(),
            receiver_role: "receiver".into(),
            profile: "public_tls".into(),
        })
    }
    pub fn mac(&self) -> Result<Mac, Refused> {
        Ok(Mac::from_bytes(decode_fixed(&self.origin_mac)?))
    }
}
/// P-256 fixed-width signature over a domain-separated commitment to pairing's
/// existing canonical transcript and session. No alternate transcript encoding.
pub fn possession_message(transcript: &Transcript) -> Vec<u8> {
    let mut message = b"semon-asi-pairing-pop-v1\0".to_vec();
    message.extend_from_slice(&transcript.digest());
    message.extend_from_slice(transcript.session_id.as_bytes());
    message
}
pub fn verify_possession(
    transcript: &Transcript,
    proof: &str,
) -> Result<pairing::VerifiedApproval, Refused> {
    let signature = decode_fixed::<64>(proof)?;
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::ECDSA_P256_SHA256_FIXED,
        &transcript.origin_key,
    )
    .verify(&possession_message(transcript), &signature)
    .map_err(|_| Refused)?;
    Ok(pairing::VerifiedApproval {
        session_id: transcript.session_id.clone(),
        origin_key: transcript.origin_key.clone(),
        transcript_digest: transcript.digest(),
        summary_digest: transcript.scope,
    })
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for shift in [18, 12, 6, 0].into_iter().take(chunk.len() + 1) {
            out.push(BASE64[((value >> shift) & 63) as usize] as char);
        }
    }
    out
}
pub fn decode_fixed<const N: usize>(text: &str) -> Result<[u8; N], Refused> {
    if text.len() != (N * 8).div_ceil(6) {
        return Err(Refused);
    }
    let mut output = [0; N];
    let mut accumulator = 0u32;
    let mut bits = 0;
    let mut offset = 0;
    for byte in text.bytes() {
        let value = BASE64.iter().position(|v| *v == byte).ok_or(Refused)? as u32;
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output[offset] = (accumulator >> bits) as u8;
            offset += 1;
        }
    }
    if accumulator & ((1 << bits) - 1) != 0 {
        return Err(Refused);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_binary_lengths_and_canonical_encoding() {
        for value in [0, 1, 127, 255] {
            let bytes = [value; 32];
            assert_eq!(decode_fixed::<32>(&encode(&bytes)), Ok(bytes));
        }
        assert_eq!(encode(b"f"), "Zg");
        assert_eq!(encode(b"fo"), "Zm8");
        assert_eq!(encode(b"foo"), "Zm9v");
        for bad in ["Zh", "Zg=", "Z+", "Z/", "Zg\n", "Z"] {
            assert_eq!(decode_fixed::<1>(bad), Err(Refused));
        }
    }
    #[test]
    fn request_grammar_roundtrips_and_refuses_unsupported_input() {
        let mut key = [1; 65];
        key[0] = 4;
        let confirm = Confirm {
            version: VERSION,
            session_id: "a".repeat(64),
            origin_key: encode(&key),
            origin_nonce: encode(&[2; 32]),
            scope_commitment: encode(&[3; 32]),
            origin_mac: encode(&[4; 32]),
        };
        let bytes = encode_message(&confirm).unwrap();
        assert!(matches!(
            Request::parse(Route::Confirm, &bytes),
            Ok(Request::Confirm(_))
        ));
        for (route, bytes) in [
            (Route::Open, br#"{"version":1}"#.to_vec()),
            (
                Route::Approve,
                encode_message(&Approve {
                    version: 1,
                    session_id: "a".repeat(64),
                    proof: encode(&[0; 64]),
                    input: pairing::ActivationInput {
                        machine: "laptop".into(),
                        binding: crate::enrollment::Binding::Inapplicable,
                        kinds: vec![(crate::credentials::Kind::MirrorPush, false)],
                        client_certificate_pins: vec![],
                        valid_until: 1000,
                    },
                })
                .unwrap(),
            ),
            (
                Route::Cancel,
                encode_message(&Cancel {
                    version: 1,
                    session_id: "a".repeat(64),
                })
                .unwrap(),
            ),
        ] {
            assert!(Request::parse(route, &bytes).is_ok());
        }
        for bad in [
            br#"{"version":2}"#.as_slice(),
            br#"{"version":1,"code":"secret"}"#,
            br#"{"version":1,"version":1}"#,
            br#"{"version":1} {}"#,
            b"{}",
        ] {
            assert!(Request::parse(Route::Open, bad).is_err());
        }
        assert!(Request::parse(Route::Open, &vec![b' '; MAX_MESSAGE_BYTES + 1]).is_err());
        assert!(Request::parse(Route::Confirm, br#"{"version":1}"#).is_err());
        assert!(Route::from_path("/v1/provision/open?code=x").is_none());
        assert!(Route::from_path("/v1/body/open").is_none());
    }
}

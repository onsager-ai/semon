//! Strict receiver renewal wire grammar and domain-separated cryptography.
use crate::{
    credentials::{Kind, Secret},
    provision_wire::{self, Refused},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const WINDOW: u64 = 2_592_000;
pub const LIFETIME: u64 = 600;
pub const OVERLAP: u64 = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Begin,
    Finish,
}
impl Route {
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "/v1/renew/begin" => Some(Self::Begin),
            "/v1/renew/finish" => Some(Self::Finish),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub kind: Kind,
    pub generation: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub kind: Kind,
    pub generation: u64,
    pub bearer: String,
}
impl Drop for Credential {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.bearer);
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Begin {
    pub version: u32,
    pub enrollment_id: String,
    pub credentials: Vec<Credential>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Begun {
    pub version: u32,
    pub enrollment_id: String,
    pub renewal_id: String,
    pub receiver_nonce: String,
    pub expires_at: u64,
    pub generations: Vec<Generation>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finish {
    pub version: u32,
    pub enrollment_id: String,
    pub renewal_id: String,
    pub credentials: Vec<Credential>,
    pub application_key: String,
    pub proof: String,
}
impl Drop for Finish {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.application_key);
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveredCredential {
    pub kind: Kind,
    pub credential_id: String,
    pub generation: u64,
    pub bearer: String,
    pub valid_from: u64,
    pub valid_until: u64,
}
impl Drop for DeliveredCredential {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.bearer);
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivered {
    pub version: u32,
    pub enrollment_id: String,
    pub renewal_id: String,
    pub credentials: Vec<DeliveredCredential>,
    pub client_certificate: Option<String>,
}
pub enum Request {
    Begin(Begin),
    Finish(Finish),
}
impl Request {
    pub fn parse(route: Route, bytes: &[u8]) -> Result<Self, Refused> {
        if bytes.len() > provision_wire::MAX_MESSAGE_BYTES {
            return Err(Refused);
        }
        let request = match route {
            Route::Begin => Self::Begin(serde_json::from_slice(bytes).map_err(|_| Refused)?),
            Route::Finish => Self::Finish(serde_json::from_slice(bytes).map_err(|_| Refused)?),
        };
        let (version, id, credentials) = match &request {
            Self::Begin(m) => (m.version, &m.enrollment_id, &m.credentials),
            Self::Finish(m) => {
                identifier(&m.renewal_id)?;
                provision_wire::decode_fixed::<32>(&m.application_key)?;
                provision_wire::decode_fixed::<64>(&m.proof)?;
                (m.version, &m.enrollment_id, &m.credentials)
            }
        };
        if version != 1 {
            return Err(Refused);
        }
        identifier(id)?;
        identities(credentials)?;
        Ok(request)
    }
}
pub(crate) fn identifier(id: &str) -> Result<(), Refused> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Refused);
    }
    Ok(())
}
pub(crate) fn identities(credentials: &[Credential]) -> Result<Vec<Generation>, Refused> {
    if credentials.is_empty() || credentials.len() > 3 {
        return Err(Refused);
    }
    let mut identities = Vec::new();
    for c in credentials {
        if c.generation == 0 || c.bearer.is_empty() || c.bearer.len() > 512 {
            return Err(Refused);
        }
        identities.push(Generation {
            kind: c.kind,
            generation: c.generation,
        });
    }
    identities.sort_by_key(|g| g.kind.wire_name());
    if identities.windows(2).any(|w| w[0].kind == w[1].kind) {
        return Err(Refused);
    }
    Ok(identities)
}
fn fields(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u64).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}
/// Two compact, sorted JSON arrays, each with an eight-byte length prefix.
pub fn scope_digest(authentication: &[Generation], output: &[Generation]) -> [u8; 32] {
    let canonical = |list: &[Generation]| {
        let mut list = list.to_vec();
        list.sort_by_key(|g| g.kind.wire_name());
        serde_json::to_vec(&list).expect("generation list serializes")
    };
    Sha256::digest(fields(&[&canonical(authentication), &canonical(output)])).into()
}
pub fn possession_message(
    receiver_id: &str,
    endpoint: &str,
    begun: &Begun,
    authentication: &[Generation],
    exporter: &[u8; 32],
    application: &[u8; 32],
) -> Result<Vec<u8>, Refused> {
    let nonce = provision_wire::decode_fixed::<32>(&begun.receiver_nonce)?;
    let scope = scope_digest(authentication, &begun.generations);
    let application_hash: [u8; 32] = Sha256::digest(application).into();
    let mut message = b"semon-asi-renewal-pop-v1\0".to_vec();
    message.extend(fields(&[
        b"1",
        receiver_id.as_bytes(),
        endpoint.as_bytes(),
        begun.enrollment_id.as_bytes(),
        begun.renewal_id.as_bytes(),
        &nonce,
        exporter,
        &scope,
        &application_hash,
    ]));
    Ok(message)
}
pub fn bearer(
    application: &[u8; 32],
    enrollment: &str,
    renewal: &str,
    target: &Generation,
) -> Secret {
    let generation = target.generation.to_string();
    let message = fields(&[
        b"semon-renewal-bearer-v1",
        enrollment.as_bytes(),
        renewal.as_bytes(),
        target.kind.wire_name().as_bytes(),
        generation.as_bytes(),
    ]);
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, application),
        &message,
    );
    Secret::new(provision_wire::encode(tag.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_begin_grammar() {
        let valid = format!(
            r#"{{"version":1,"enrollment_id":"{}","credentials":[{{"kind":"mirror_push","generation":1,"bearer":"secret"}}]}}"#,
            "a".repeat(64)
        );
        assert!(Request::parse(Route::Begin, valid.as_bytes()).is_ok());
        for bad in [
            valid.replace("\"version\":1", "\"version\":2"),
            valid.replace("\"version\":1", "\"version\":1,\"version\":1"),
            valid.replace("\"bearer\":", "\"extra\":0,\"bearer\":"),
            valid.replace("\"generation\":1", "\"generation\":0"),
            valid.replace(&"a".repeat(64), &"A".repeat(64)),
        ] {
            assert!(Request::parse(Route::Begin, bad.as_bytes()).is_err());
        }
        assert!(Request::parse(Route::Begin, &vec![b' '; 8193]).is_err());
    }
    #[test]
    fn bearer_domains_separate_kind_generation_and_attempt() {
        let target = Generation {
            kind: Kind::MirrorPush,
            generation: 2,
        };
        let secret = bearer(&[1; 32], "enrollment", "renewal", &target);
        assert_eq!(
            secret.hash(),
            bearer(&[1; 32], "enrollment", "renewal", &target).hash()
        );
        assert_ne!(
            secret.hash(),
            bearer(&[1; 32], "enrollment", "other", &target).hash()
        );
        assert_ne!(
            secret.hash(),
            bearer(
                &[1; 32],
                "enrollment",
                "renewal",
                &Generation {
                    generation: 3,
                    ..target.clone()
                }
            )
            .hash()
        );
        assert_ne!(
            secret.hash(),
            bearer(
                &[1; 32],
                "enrollment",
                "renewal",
                &Generation {
                    kind: Kind::BodyChannel,
                    ..target
                }
            )
            .hash()
        );
    }
}

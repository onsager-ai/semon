//! Immutable receiver objects. No transport, publisher claims or head activation.
use crate::{egress::Projection, governance, schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[cfg(target_os = "linux")]
mod storage;
#[cfg(target_os = "linux")]
pub use storage::{ObjectStore, StagedObject, ValidatedObject};

pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_ROWS: u64 = 20_000;
const PROFILE: &str = "receiver_plain_v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    Invalid,
    Limit,
    Storage,
    Worker,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid receiver object",
            Self::Limit => "receiver object resource limit",
            Self::Storage => "receiver object storage unavailable",
            Self::Worker => "receiver object validation worker failed",
        })
    }
}
impl std::error::Error for Failure {}
type Result<T> = std::result::Result<T, Failure>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub sha256: String,
    pub bytes: u64,
}
fn identity(bytes: &[u8]) -> Identity {
    Identity {
        sha256: hex::encode(Sha256::digest(bytes)),
        bytes: bytes.len() as u64,
    }
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Scope comes from the trusted host's authorization decision, never the object.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub tenant: String,
    pub destination: String,
    pub publisher: String,
    pub namespace: String,
}
impl Scope {
    fn check(&self) -> Result<()> {
        if self.destination != "receiver"
            || [&self.tenant, &self.publisher, &self.namespace]
                .iter()
                .any(|s| {
                    s.is_empty()
                        || s.len() > 128
                        || !s
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                })
        {
            return Err(Failure::Invalid);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    schema: u32,
    profile: String,
    namespace: String,
    encoding: String,
    payload_bytes: u64,
    decoded_bytes: u64,
    sessions: u64,
    rows: BTreeMap<String, u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    manifest: Manifest,
    payload: Payload,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    profile: String,
    sessions: Vec<Session>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    rows: Vec<Row>,
    policy_versions: Vec<PolicyVersion>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    table: String,
    fields: Map<String, Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyVersion {
    source_handle: String,
    policy_version: String,
}

/// Completed fresh bytes and the digest of the entire envelope, including manifest.
pub struct PublicationObject {
    bytes: Vec<u8>,
    identity: Identity,
}
impl PublicationObject {
    pub fn from_projection(projection: &Projection) -> Result<Self> {
        let payload: Payload = serde_json::from_slice(
            &projection
                .serialize_receiver()
                .map_err(|_| Failure::Invalid)?,
        )
        .map_err(|_| Failure::Invalid)?;
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|_| Failure::Invalid)?
            .len() as u64;
        let manifest = Manifest {
            format: 1,
            schema: schema::ASI_SCHEMA_VERSION,
            profile: PROFILE.into(),
            namespace: projection.namespace.clone(),
            encoding: "json_identity".into(),
            payload_bytes,
            decoded_bytes: payload_bytes,
            sessions: payload.sessions.len() as u64,
            rows: counts(&payload),
        };
        let bytes =
            serde_json::to_vec(&Envelope { manifest, payload }).map_err(|_| Failure::Invalid)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Failure::Limit);
        }
        Ok(Self {
            identity: identity(&bytes),
            bytes,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
}
fn counts(payload: &Payload) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for session in &payload.sessions {
        for row in &session.rows {
            *counts.entry(row.table.clone()).or_default() += 1;
        }
    }
    counts
}

// Called only in the disposable worker. Canonical reserialization also rejects
// duplicate keys, alternate encodings and unrecognized metadata at every level.
pub(crate) fn validate(bytes: &[u8], namespace: &str) -> Result<()> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Failure::Limit);
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| Failure::Invalid)?;
    if serde_json::to_vec(&envelope).map_err(|_| Failure::Invalid)? != bytes {
        return Err(Failure::Invalid);
    }
    let m = &envelope.manifest;
    let payload = &envelope.payload;
    let actual = serde_json::to_vec(payload)
        .map_err(|_| Failure::Invalid)?
        .len() as u64;
    if m.format != 1
        || m.schema != schema::ASI_SCHEMA_VERSION
        || m.profile != PROFILE
        || payload.profile != PROFILE
        || m.namespace != namespace
        || m.encoding != "json_identity"
        || m.payload_bytes != actual
        || m.decoded_bytes != actual
        || m.sessions != payload.sessions.len() as u64
        || m.rows != counts(payload)
    {
        return Err(Failure::Invalid);
    }
    if m.rows.values().sum::<u64>() > MAX_ROWS || m.sessions > MAX_ROWS {
        return Err(Failure::Limit);
    }
    let prefix = format!("{}/receiver/", hex::encode(namespace.as_bytes()));
    for session in &payload.sessions {
        if session.policy_versions.is_empty() || session.policy_versions.len() > 1024 {
            return Err(Failure::Invalid);
        }
        for policy in &session.policy_versions {
            let source_prefix = format!("{}/source/", hex::encode(namespace.as_bytes()));
            if !policy
                .source_handle
                .strip_prefix(&source_prefix)
                .is_some_and(hash)
                || !policy
                    .policy_version
                    .strip_prefix("sha256:")
                    .is_some_and(hash)
            {
                return Err(Failure::Invalid);
            }
        }
        if !session.rows.iter().any(|row| row.table == "fact_session") {
            return Err(Failure::Invalid);
        }
        for row in &session.rows {
            let table = schema::table(&row.table).ok_or(Failure::Invalid)?;
            if matches!(table.name, "fact_label" | "dim_price") {
                return Err(Failure::Invalid);
            }
            for column in table.columns {
                if !column.nullable && !row.fields.contains_key(column.name) {
                    return Err(Failure::Invalid);
                }
            }
            for (name, value) in &row.fields {
                let column = table.column(name).ok_or(Failure::Invalid)?;
                if column.class == schema::Class::Body {
                    return Err(Failure::Invalid);
                }
                governance::validate_column_value(column, value, governance::Destination::Receiver)
                    .map_err(|_| Failure::Invalid)?;
                if matches!(
                    name.as_str(),
                    "session_uid"
                        | "parent_session_uid"
                        | "lane_root_uid"
                        | "step_uid"
                        | "turn_uid"
                        | "compaction_uid"
                ) && !value.is_null()
                {
                    let text = value.as_str().ok_or(Failure::Invalid)?;
                    if !text.starts_with(&prefix) || !text.rsplit('/').next().is_some_and(hash) {
                        return Err(Failure::Invalid);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Internal worker only: bounded namespace line, then exact object bytes. No logs.
pub fn worker_stdio() -> i32 {
    use std::io::{Read, Write};
    let result = (|| {
        #[cfg(target_os = "linux")]
        storage::worker_limits()?;
        #[cfg(not(target_os = "linux"))]
        return Err(Failure::Worker);
        #[allow(unreachable_code)]
        {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(MAX_BYTES + 131)
                .read_to_end(&mut bytes)
                .map_err(|_| Failure::Worker)?;
            let split = bytes
                .iter()
                .position(|b| *b == b'\n')
                .filter(|n| *n <= 128)
                .ok_or(Failure::Invalid)?;
            let namespace = std::str::from_utf8(&bytes[..split]).map_err(|_| Failure::Invalid)?;
            validate(&bytes[split + 1..], namespace)
        }
    })();
    let success = result.is_ok();
    let _ = std::io::stdout().write_all(if success { b"1" } else { b"0" });
    if success { 0 } else { 1 }
}

#[cfg(test)]
mod tests;

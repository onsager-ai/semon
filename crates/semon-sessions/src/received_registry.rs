//! Shared machine metadata in the receiver's single enrollment registry.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io, path::Path};

pub const RECEIVER_FORMAT: u32 = 7;
pub const REGISTRY_CAP: u64 = 4 * 1024 * 1024;
pub const REENROLL: &str = "received machine has no current registry record; keep retained logs unchanged and re-enroll the machine";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivedMachine {
    pub id: String,
    /// Immutable name of the existing machines/<name>/ directory.
    pub directory: String,
    pub owner_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineAudit {
    pub time: u64,
    pub action: String,
    pub ids: Vec<String>,
    pub before: ReceivedMachine,
    pub after: ReceivedMachine,
}

pub fn validate_machines(machines: &[ReceivedMachine]) -> bool {
    let mut ids = BTreeSet::new();
    machines.iter().all(|m| {
        crate::machine_id::valid(&m.id)
            && m.directory == m.id
            && ids.insert(&m.id)
            && m.owner_name.as_ref().is_none_or(|n| valid_owner_name(n))
    })
}

pub fn valid_owner_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 253 && !name.chars().any(char::is_control)
}

/// Read the atomically published registry. No second metadata file is authoritative.
pub fn read_machines(dir: &Path) -> io::Result<Vec<ReceivedMachine>> {
    let bytes = crate::read_regular_at_most(&dir.join("enrollment/registry.json"), REGISTRY_CAP)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("receiver registry unavailable ({error}); {REENROLL}"),
            )
        })?;
    machines_from_bytes(&bytes)
}

pub fn machines_from_bytes(bytes: &[u8]) -> io::Result<Vec<ReceivedMachine>> {
    if bytes.len() as u64 > REGISTRY_CAP {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "receiver registry exceeds its size limit",
        ));
    }
    let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    if value["format"].as_u64() != Some(u64::from(RECEIVER_FORMAT)) {
        return Err(invalid(
            "unsupported receiver registry format; keep retained logs unchanged and re-enroll in a new receiver directory",
        ));
    }
    let machines: Vec<ReceivedMachine> =
        serde_json::from_value(value["machines"].clone()).map_err(io::Error::other)?;
    if !validate_machines(&machines) {
        return Err(invalid("invalid receiver machine records"));
    }
    Ok(machines)
}

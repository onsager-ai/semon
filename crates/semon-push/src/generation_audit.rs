//! Append-only ended-generation evidence under a dedicated audit append lock.
use crate::{
    body_cache::CacheDirectory,
    credentials::{Bearer, Kind},
    enrollment::{Error, Pin},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
};

pub const FILE: &str = "generations.jsonl";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndedReason {
    Expired,
    Retired,
    Revoked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndedGeneration {
    pub version: u32,
    pub record_type: String,
    pub event_id: String,
    pub observed_at: u64,
    pub ended_reason: EndedReason,
    pub credential_id: String,
    pub enrollment_id: String,
    pub kind: Kind,
    pub generation: u64,
    pub valid_from: u64,
    pub valid_until: u64,
    pub revoked_at: Option<u64>,
    pub pin: Option<Pin>,
}
impl EndedGeneration {
    pub(crate) fn new(b: &Bearer, pin: Option<Pin>, reason: EndedReason, now: u64) -> Self {
        let mut hash = Sha256::new();
        let generation = b.generation.to_string();
        for field in [
            b"semon-generation-ended-v1".as_slice(),
            b.enrollment_id.as_bytes(),
            b.kind.wire_name().as_bytes(),
            generation.as_bytes(),
            b.credential_id.as_bytes(),
        ] {
            hash.update((field.len() as u64).to_be_bytes());
            hash.update(field);
        }
        Self {
            version: 1,
            record_type: "credential_generation_ended".into(),
            event_id: hex::encode(hash.finalize()),
            observed_at: now,
            ended_reason: reason,
            credential_id: b.credential_id.clone(),
            enrollment_id: b.enrollment_id.clone(),
            kind: b.kind,
            generation: b.generation,
            valid_from: b.valid_from,
            valid_until: b.valid_until,
            revoked_at: b.revoked_at,
            pin,
        }
    }
}
/// Append and fsync before the caller may remove registry evidence. Duplicate IDs
/// are intentionally permitted: they describe the same ended generation.
pub(crate) fn append(receiver: &Path, records: &[EndedGeneration]) -> Result<(), Error> {
    let directory = CacheDirectory::open(&receiver.join("receiver-audit"))
        .map_err(|_| Error::AuditUnavailable)?;
    let lock = directory
        .open_file(Path::new("generations.lock"), libc::O_RDWR | libc::O_CREAT)
        .map_err(|_| Error::AuditUnavailable)?;
    lock.try_lock().map_err(|_| Error::AuditUnavailable)?;
    let mut file = directory
        .open_file(
            Path::new(FILE),
            libc::O_RDWR | libc::O_CREAT | libc::O_APPEND,
        )
        .map_err(|_| Error::AuditUnavailable)?;
    let mut reader = BufReader::new(&mut file);
    loop {
        let mut line = Vec::new();
        let size = Read::by_ref(&mut reader)
            .take(8193)
            .read_until(b'\n', &mut line)
            .map_err(|_| Error::AuditUnavailable)?;
        if size == 0 {
            break;
        }
        if size > 8192 || line.last() != Some(&b'\n') {
            return Err(Error::AuditUnavailable);
        }
        let record: EndedGeneration =
            serde_json::from_slice(&line).map_err(|_| Error::AuditUnavailable)?;
        if record.version != 1 || record.record_type != "credential_generation_ended" {
            return Err(Error::AuditUnavailable);
        }
    }
    let mut bytes = Vec::new();
    for record in records {
        serde_json::to_writer(&mut bytes, record).map_err(|_| Error::AuditUnavailable)?;
        bytes.push(b'\n');
    }
    if file
        .metadata()
        .map_err(|_| Error::AuditUnavailable)?
        .len()
        .saturating_add(bytes.len() as u64)
        > crate::receiver_audit::MAX_BYTES
    {
        return Err(Error::AuditUnavailable);
    }
    file.write_all(&bytes)
        .map_err(|_| Error::AuditUnavailable)?;
    file.sync_all().map_err(|_| Error::AuditUnavailable)?;
    directory.sync().map_err(|_| Error::AuditUnavailable)?;
    Ok(())
}

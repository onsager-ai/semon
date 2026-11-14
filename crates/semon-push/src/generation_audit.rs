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
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Mutex,
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
    append_records(receiver, FILE, records, |line| {
        let record: EndedGeneration =
            serde_json::from_slice(line).map_err(|_| Error::AuditUnavailable)?;
        let bearer = Bearer {
            credential_id: record.credential_id.clone(),
            enrollment_id: record.enrollment_id.clone(),
            kind: record.kind,
            sha256: [0; 32],
            generation: record.generation,
            valid_from: record.valid_from,
            valid_until: record.valid_until,
            revoked_at: record.revoked_at,
        };
        if record.version != 1
            || record.record_type != "credential_generation_ended"
            || record.event_id
                != EndedGeneration::new(
                    &bearer,
                    record.pin.clone(),
                    record.ended_reason,
                    record.observed_at,
                )
                .event_id
            || record.valid_from >= record.valid_until
            || record.generation == 0
        {
            return Err(Error::AuditUnavailable);
        }
        Ok(())
    })
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OwnerOutcome {
    Requested,
    Committed,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerRecord {
    pub version: u32,
    pub observed_at: u64,
    pub owner: String,
    pub enrollment_id: String,
    pub kind: Option<Kind>,
    pub action: String,
    pub outcome: OwnerOutcome,
}
pub(crate) fn owner(receiver: &Path, record: &OwnerRecord) -> Result<(), Error> {
    append_records(
        receiver,
        "owners.jsonl",
        std::slice::from_ref(record),
        |line| {
            let record: OwnerRecord =
                serde_json::from_slice(line).map_err(|_| Error::AuditUnavailable)?;
            if record.version != 1 {
                return Err(Error::AuditUnavailable);
            }
            Ok(())
        },
    )
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenewalRecord {
    pub version: u32,
    pub observed_at: u64,
    pub enrollment_id: String,
    pub renewal_id: String,
    pub generations: Vec<crate::renewal_wire::Generation>,
    pub action: String,
}
pub(crate) fn renewal(receiver: &Path, record: &RenewalRecord) -> Result<(), Error> {
    append_records(
        receiver,
        "renewals.jsonl",
        std::slice::from_ref(record),
        |line| {
            let record: RenewalRecord =
                serde_json::from_slice(line).map_err(|_| Error::AuditUnavailable)?;
            if record.version != 1 {
                return Err(Error::AuditUnavailable);
            }
            Ok(())
        },
    )
}
/// Refusals never append to the authority journal or take its lock. Eleven
/// saturating counters and one last nonsecret scope per class have a fixed budget.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefusalRecord {
    pub class: crate::renewal::Failure,
    pub observed_at: u64,
    pub enrollment_id: String,
    pub renewal_id: String,
    pub generations: Vec<crate::renewal_wire::Generation>,
    pub outcome: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RefusalCount {
    count: u64,
    last: RefusalRecord,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Refusals {
    version: u32,
    classes: Vec<RefusalCount>,
}
pub(crate) const REFUSAL_FILE: &str = "renewal-refusals.json";
pub(crate) const REFUSAL_CAP: u64 = 8192;
pub(crate) fn refusal(receiver: &Path, record: RefusalRecord) -> Result<(), Error> {
    let directory = CacheDirectory::open(&receiver.join("receiver-audit"))
        .map_err(|_| Error::AuditUnavailable)?;
    let lock = directory
        .open_file(
            Path::new("renewal-refusals.lock"),
            libc::O_RDWR | libc::O_CREAT,
        )
        .map_err(|_| Error::AuditUnavailable)?;
    lock.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => Error::InUse,
        std::fs::TryLockError::Error(_) => Error::AuditUnavailable,
    })?;
    let file = directory
        .open_optional_file(Path::new(REFUSAL_FILE), libc::O_RDONLY)
        .map_err(|_| Error::AuditUnavailable)?;
    // Absence initializes only in memory. The final path first appears as a
    // complete fsynced summary after atomic rename. Existing corruption fails
    // visibly at the caller and is never silently reset.
    let mut summary = match file {
        Some(file) => {
            if file.metadata().map_err(|_| Error::AuditUnavailable)?.len() > REFUSAL_CAP {
                return Err(Error::AuditUnavailable);
            }
            let summary: Refusals = serde_json::from_reader(file.take(REFUSAL_CAP + 1))
                .map_err(|_| Error::AuditUnavailable)?;
            if summary.version != 1 || summary.classes.len() > 11 {
                return Err(Error::AuditUnavailable);
            }
            summary
        }
        None => Refusals {
            version: 1,
            classes: Vec::new(),
        },
    };
    if let Some(entry) = summary
        .classes
        .iter_mut()
        .find(|entry| entry.last.class == record.class)
    {
        entry.count = entry.count.saturating_add(1);
        entry.last = record;
    } else {
        if summary.classes.len() >= 11 {
            return Err(Error::AuditUnavailable);
        }
        summary.classes.push(RefusalCount {
            count: 1,
            last: record,
        });
    }
    let bytes = serde_json::to_vec(&summary).map_err(|_| Error::AuditUnavailable)?;
    if bytes.len() as u64 > REFUSAL_CAP {
        return Err(Error::AuditUnavailable);
    }
    let stage = Path::new("renewal-refusals.stage");
    let mut file = directory
        .open_file(stage, libc::O_RDWR | libc::O_CREAT)
        .map_err(|_| Error::AuditUnavailable)?;
    file.set_len(0).map_err(|_| Error::AuditUnavailable)?;
    file.write_all(&bytes)
        .map_err(|_| Error::AuditUnavailable)?;
    #[cfg(test)]
    refusal_test_io()?;
    file.sync_all().map_err(|_| Error::AuditUnavailable)?;
    directory
        .rename(stage, Path::new(REFUSAL_FILE))
        .map_err(|_| Error::AuditUnavailable)?;
    directory.sync().map_err(|_| Error::AuditUnavailable)
}
#[cfg(test)]
pub(crate) enum RefusalIo {
    Interrupt,
    Block {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    },
}
#[cfg(test)]
thread_local! {
    static REFUSAL_IO: std::cell::RefCell<Option<RefusalIo>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn set_refusal_io(hook: RefusalIo) {
    REFUSAL_IO.with(|slot| *slot.borrow_mut() = Some(hook));
}
#[cfg(test)]
fn refusal_test_io() -> Result<(), Error> {
    match REFUSAL_IO.with(|slot| slot.borrow_mut().take()) {
        Some(RefusalIo::Interrupt) => Err(Error::AuditUnavailable),
        Some(RefusalIo::Block { entered, release }) => {
            entered.send(()).map_err(|_| Error::AuditUnavailable)?;
            release
                .recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|_| Error::AuditUnavailable)
        }
        None => Ok(()),
    }
}
// Cache only fully validated snapshots, never authorization. Metadata changes
// (including another process's append/replacement) require full validation again.
// Fixed slots avoid growing a process-wide map with receiver paths.
#[derive(Clone, Copy, PartialEq, Eq)]
struct JournalStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl JournalStamp {
    fn read(file: &std::fs::File) -> Result<Self, Error> {
        let m = file.metadata().map_err(|_| Error::AuditUnavailable)?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        })
    }
}
static VALIDATED: Mutex<[Option<JournalStamp>; 3]> = Mutex::new([None; 3]);
#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum AppendFault {
    Partial,
    Sync,
}
#[cfg(test)]
thread_local! {
    static APPEND_FAULT: std::cell::Cell<Option<AppendFault>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(crate) fn fail_next_append(fault: AppendFault) {
    APPEND_FAULT.set(Some(fault));
}
fn append_records<T: Serialize>(
    receiver: &Path,
    name: &str,
    records: &[T],
    validate: impl Fn(&[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let directory = CacheDirectory::open(&receiver.join("receiver-audit"))
        .map_err(|_| Error::AuditUnavailable)?;
    let lock = directory
        .open_file(Path::new("generations.lock"), libc::O_RDWR | libc::O_CREAT)
        .map_err(|_| Error::AuditUnavailable)?;
    lock.try_lock().map_err(|_| Error::AuditUnavailable)?;
    let mut file = directory
        .open_file(
            Path::new(name),
            libc::O_RDWR | libc::O_CREAT | libc::O_APPEND,
        )
        .map_err(|_| Error::AuditUnavailable)?;
    let slot = match name {
        FILE => 0,
        "owners.jsonl" => 1,
        _ => 2,
    };
    let stamp = JournalStamp::read(&file)?;
    if stamp.size > crate::receiver_audit::MAX_BYTES {
        return Err(Error::AuditUnavailable);
    }
    let mut validated = VALIDATED.lock().map_err(|_| Error::AuditUnavailable)?;
    if validated[slot] != Some(stamp) {
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
            validate(&line)?;
        }
        validated[slot] = Some(stamp);
    }
    let mut bytes = Vec::new();
    for record in records {
        let start = bytes.len();
        serde_json::to_writer(&mut bytes, record).map_err(|_| Error::AuditUnavailable)?;
        bytes.push(b'\n');
        if bytes.len() - start > 8192 {
            return Err(Error::AuditUnavailable);
        }
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
    #[cfg(test)]
    let fault = APPEND_FAULT.take();
    #[cfg(test)]
    if matches!(fault, Some(AppendFault::Partial)) {
        file.write_all(&bytes[..bytes.len() / 2])
            .map_err(|_| Error::AuditUnavailable)?;
        return Err(Error::AuditUnavailable);
    }
    file.write_all(&bytes)
        .map_err(|_| Error::AuditUnavailable)?;
    #[cfg(test)]
    if matches!(fault, Some(AppendFault::Sync)) {
        return Err(Error::AuditUnavailable);
    }
    file.sync_all().map_err(|_| Error::AuditUnavailable)?;
    directory.sync().map_err(|_| Error::AuditUnavailable)?;
    validated[slot] = Some(JournalStamp::read(&file)?);
    Ok(())
}

//! Sensitive receiver read attempts. Successful appends are fsynced before return.
//! Any error must refuse release; this module neither authorizes nor releases bodies.

use crate::body_cache::{CacheDirectory, CacheError, Clock};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::DirBuilderExt,
    path::Path,
};

pub const AUDIT_FILE: &str = "attempts.jsonl";
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RECEIPTS: u32 = 16;
const RECORD_CAP: u64 = 8192;
// Reserve enough for the largest completion and every allowed receipt at begin.
const FOLLOWUP_RESERVE: u64 = (MAX_RECEIPTS as u64 + 1) * RECORD_CAP;
type Result<T> = std::result::Result<T, AuditError>;

/// Exactly 32 opaque bytes rendered as lowercase hex. No free-text constructor.
/// The identity adapter supplies a stable digest/opaque ID for the authenticated
/// principal; it must never pass a credential. Version IDs are content digests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Identifier([u8; 32]);
impl Identifier {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}
impl TryFrom<String> for Identifier {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("expected an opaque 32-byte lowercase hex identifier");
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(value, &mut bytes).map_err(|_| "invalid identifier")?;
        Ok(Self(bytes))
    }
}
impl From<Identifier> for String {
    fn from(value: Identifier) -> Self {
        hex::encode(value.0)
    }
}

/// Opaque local attempt identity, valid only while its originating writer is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AttemptId(Identifier);

/// Half-open exported page/field range, never an origin-native path or selector.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermittedRange {
    start: u64,
    end: u64,
}
impl PermittedRange {
    pub fn new(start: u64, end: u64) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }
    fn len(self) -> u64 {
        self.end - self.start
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    TranscriptPage,
    FieldExpansion,
    StreamRange,
    SearchSnippet,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Granted,
    Denied,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    Viewer,
    Cli,
    Mcp,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed,
    Denied,
}

/// Closed metadata only. There are no body, query, command, path or error fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub request_id: Identifier,
    pub authenticated_principal: Identifier,
    pub source_handle: Identifier,
    pub generation: Identifier,
    pub permitted_range: PermittedRange,
    pub purpose: Purpose,
    pub receiver_policy_version: Identifier,
    pub origin_policy_version: Option<Identifier>,
    pub origin_redactor_version: Option<Identifier>,
    pub decision: Decision,
    pub destination: Destination,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Attempted {
        format: u32,
        attempt_id: AttemptId,
        record: Box<AttemptRecord>,
        bytes: u64,
        time: u64,
    },
    Receipt {
        attempt_id: AttemptId,
        range: PermittedRange,
        bytes: u64,
        time: u64,
    },
    Completion {
        attempt_id: AttemptId,
        outcome: Outcome,
        bytes: u64,
        time: u64,
    },
}

#[derive(Debug)]
pub enum AuditError {
    Custody,
    Full,
    Invalid,
    WriteFailed,
    InUse,
}
impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Custody => "receiver audit custody unavailable",
            Self::Full => "receiver audit capacity exhausted; operator action required",
            Self::Invalid => "invalid receiver audit operation or journal",
            Self::WriteFailed => "receiver audit write failed; reads refused",
            Self::InUse => "receiver audit writer already active",
        })
    }
}
impl std::error::Error for AuditError {}
impl From<CacheError> for AuditError {
    fn from(_: CacheError) -> Self {
        Self::Custody
    }
}

struct Active {
    record: AttemptRecord,
    receipts: u32,
    next: u64,
    delivered: u64,
    reserve: u64,
}

/// One exclusive writer per directory; methods require mutable access. Share via
/// a Mutex for concurrent callers. No automatic rotation, truncation or deletion.
pub struct AuditStore<C: Clock> {
    file: File,
    _directory: CacheDirectory,
    _lock: File,
    clock: C,
    cap: u64,
    used: u64,
    reserved: u64,
    active: BTreeMap<AttemptId, Active>,
    failed: bool,
}
impl<C: Clock> AuditStore<C> {
    /// Dedicated directory, separate from ACLs/cache; the integration owns paths.
    /// `cap` may narrow the 64 MiB ceiling. Existing records are never discarded.
    pub fn open(dir: &Path, clock: C, cap: u64) -> Result<Self> {
        if !dir.exists() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|_| AuditError::Custody)?;
        }
        let directory = CacheDirectory::open(dir)?;
        if directory
            .names()?
            .iter()
            .any(|name| name != Path::new(AUDIT_FILE) && name != Path::new("audit.lock"))
        {
            return Err(AuditError::Custody);
        }
        let lock = directory.open_file(Path::new("audit.lock"), libc::O_RDWR | libc::O_CREAT)?;
        lock.try_lock().map_err(|_| AuditError::InUse)?;
        let mut file = directory.open_file(
            Path::new(AUDIT_FILE),
            libc::O_RDWR | libc::O_CREAT | libc::O_APPEND,
        )?;
        let used = file.metadata().map_err(|_| AuditError::Custody)?.len();
        let cap = cap.min(MAX_BYTES);
        if used > cap {
            return Err(AuditError::Full);
        }
        // A torn/corrupt append refuses reopening; never erase or repair history.
        let mut reader = BufReader::new(&mut file);
        loop {
            let mut line = Vec::new();
            let size = Read::by_ref(&mut reader)
                .take(RECORD_CAP + 1)
                .read_until(b'\n', &mut line)
                .map_err(|_| AuditError::Custody)?;
            if size == 0 {
                break;
            }
            if size as u64 > RECORD_CAP || line.last() != Some(&b'\n') {
                return Err(AuditError::Invalid);
            }
            let event: Event = serde_json::from_slice(&line).map_err(|_| AuditError::Invalid)?;
            match event {
                Event::Attempted {
                    format,
                    record,
                    bytes,
                    ..
                } if format != 1
                    || bytes != 0
                    || record.permitted_range.start >= record.permitted_range.end =>
                {
                    return Err(AuditError::Invalid);
                }
                Event::Receipt { range, bytes, .. }
                    if range.start >= range.end || bytes > range.len() =>
                {
                    return Err(AuditError::Invalid);
                }
                _ => {}
            }
        }
        file.seek(SeekFrom::End(0))
            .map_err(|_| AuditError::Custody)?;
        file.sync_all().map_err(|_| AuditError::WriteFailed)?;
        directory.sync()?;
        Ok(Self {
            file,
            _directory: directory,
            _lock: lock,
            clock,
            cap,
            used,
            reserved: 0,
            active: BTreeMap::new(),
            failed: false,
        })
    }

    /// Persist attempted/zero bytes before returning. This is not authorization.
    /// Callers must refuse reads on any error and separately obtain origin audit.
    pub fn begin_attempt(&mut self, record: AttemptRecord) -> Result<AttemptId> {
        if self.failed {
            return Err(AuditError::WriteFailed);
        }
        if record.permitted_range.start >= record.permitted_range.end {
            return Err(AuditError::Invalid);
        }
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| AuditError::WriteFailed)?;
        let id = AttemptId(Identifier(random));
        if self.active.contains_key(&id) {
            return Err(AuditError::Invalid);
        }
        let event = Event::Attempted {
            format: 1,
            attempt_id: id,
            record: Box::new(record.clone()),
            bytes: 0,
            time: self.clock.now(),
        };
        self.append(&event, FOLLOWUP_RESERVE)?;
        self.reserved += FOLLOWUP_RESERVE;
        self.active.insert(
            id,
            Active {
                next: record.permitted_range.start,
                record,
                receipts: 0,
                delivered: 0,
                reserve: FOLLOWUP_RESERVE,
            },
        );
        Ok(id)
    }

    /// Append an actual delivered range. At most 16 monotonically ordered,
    /// non-overlapping receipts; start a freshly authorized attempt after that.
    pub fn receipt(&mut self, id: AttemptId, range: PermittedRange, bytes: u64) -> Result<()> {
        let active = self.active.get(&id).ok_or(AuditError::Invalid)?;
        if active.record.decision != Decision::Granted
            || active.record.purpose != Purpose::StreamRange
            || active.receipts >= MAX_RECEIPTS
            || range.start < active.next
            || range.start >= range.end
            || range.end > active.record.permitted_range.end
            || bytes > range.len()
        {
            return Err(AuditError::Invalid);
        }
        self.append_reserved(
            &Event::Receipt {
                attempt_id: id,
                range,
                bytes,
                time: self.clock.now(),
            },
            id,
        )?;
        let active = self.active.get_mut(&id).ok_or(AuditError::Invalid)?;
        active.receipts += 1;
        active.next = range.end;
        active.delivered += bytes;
        Ok(())
    }

    /// Completion follows actual release/cancellation. A dropped writer leaves an
    /// attempted record without completion; reopening never turns that into success.
    pub fn complete(&mut self, id: AttemptId, outcome: Outcome, bytes: u64) -> Result<()> {
        let active = self.active.get(&id).ok_or(AuditError::Invalid)?;
        if bytes > active.record.permitted_range.len()
            || bytes < active.delivered
            || (active.record.decision == Decision::Denied
                && (bytes != 0 || outcome != Outcome::Denied))
            || (active.record.decision == Decision::Granted && outcome == Outcome::Denied)
        {
            return Err(AuditError::Invalid);
        }
        self.append_reserved(
            &Event::Completion {
                attempt_id: id,
                outcome,
                bytes,
                time: self.clock.now(),
            },
            id,
        )?;
        let active = self.active.remove(&id).ok_or(AuditError::Invalid)?;
        self.reserved -= active.reserve;
        Ok(())
    }

    fn append_reserved(&mut self, event: &Event, id: AttemptId) -> Result<()> {
        self.reserved -= RECORD_CAP;
        let result = self.append(event, 0);
        if result.is_err() {
            self.reserved += RECORD_CAP;
            return result;
        }
        self.active.get_mut(&id).ok_or(AuditError::Invalid)?.reserve -= RECORD_CAP;
        Ok(())
    }

    fn append(&mut self, event: &Event, extra_reserve: u64) -> Result<()> {
        if self.failed {
            return Err(AuditError::WriteFailed);
        }
        let mut bytes = serde_json::to_vec(event).map_err(|_| AuditError::Invalid)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > RECORD_CAP {
            return Err(AuditError::Invalid);
        }
        if self.used + self.reserved + extra_reserve + bytes.len() as u64 > self.cap {
            return Err(AuditError::Full);
        }
        if self
            .file
            .write_all(&bytes)
            .and_then(|()| self.file.sync_all())
            .is_err()
        {
            self.failed = true;
            return Err(AuditError::WriteFailed);
        }
        self.used += bytes.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "receiver-audit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn events(&self) -> Vec<serde_json::Value> {
            fs::read_to_string(self.0.join(AUDIT_FILE))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct ClockAt(u64);
    impl Clock for ClockAt {
        fn now(&self) -> u64 {
            self.0
        }
    }
    fn record() -> AttemptRecord {
        let id = Identifier::from_bytes([1; 32]);
        AttemptRecord {
            request_id: id,
            authenticated_principal: id,
            source_handle: id,
            generation: id,
            permitted_range: PermittedRange::new(0, 100).unwrap(),
            purpose: Purpose::StreamRange,
            receiver_policy_version: id,
            origin_policy_version: None,
            origin_redactor_version: None,
            decision: Decision::Granted,
            destination: Destination::Viewer,
        }
    }
    #[test]
    fn persisted_before_return_crash_and_completion() {
        let f = Fixture::new();
        let mut store = AuditStore::open(&f.0, ClockAt(42), MAX_BYTES).unwrap();
        let id = store.begin_attempt(record()).unwrap();
        assert_eq!(f.events()[0]["event"], "attempted");
        assert_eq!(f.events()[0]["bytes"], 0);
        assert_eq!(f.events()[0]["time"], 42);
        store.complete(id, Outcome::Cancelled, 7).unwrap();
        assert_eq!(f.events()[1]["bytes"], 7);
        assert_eq!(f.events()[1]["outcome"], "cancelled");
        store.begin_attempt(record()).unwrap();
        drop(store);
        let _reopened = AuditStore::open(&f.0, ClockAt(43), MAX_BYTES).unwrap();
        assert_eq!(f.events().len(), 3);
        assert_eq!(f.events()[2]["event"], "attempted");
    }
    #[test]
    fn cap_refuses_without_dropping_and_reserves_completion() {
        let f = Fixture::new();
        let cap = FOLLOWUP_RESERVE + RECORD_CAP;
        let mut store = AuditStore::open(&f.0, ClockAt(1), cap).unwrap();
        let id = store.begin_attempt(record()).unwrap();
        let before = fs::read(f.0.join(AUDIT_FILE)).unwrap();
        assert!(matches!(
            store.begin_attempt(record()),
            Err(AuditError::Full)
        ));
        assert_eq!(fs::read(f.0.join(AUDIT_FILE)).unwrap(), before);
        store.complete(id, Outcome::Completed, 3).unwrap();
        assert_eq!(f.events().len(), 2);
    }
    #[test]
    fn receipts_are_bounded_and_validate_ranges() {
        let f = Fixture::new();
        let mut store = AuditStore::open(&f.0, ClockAt(1), MAX_BYTES).unwrap();
        let id = store.begin_attempt(record()).unwrap();
        for start in 0..u64::from(MAX_RECEIPTS) {
            store
                .receipt(id, PermittedRange::new(start, start + 1).unwrap(), 1)
                .unwrap();
        }
        assert!(
            store
                .receipt(id, PermittedRange::new(16, 17).unwrap(), 1)
                .is_err()
        );
        assert!(store.complete(id, Outcome::Completed, 15).is_err());
        store.complete(id, Outcome::Completed, 16).unwrap();
        assert_eq!(f.events().len(), MAX_RECEIPTS as usize + 2);
        assert!(store.complete(id, Outcome::Completed, 16).is_err());
        let mut denied = record();
        denied.decision = Decision::Denied;
        let id = store.begin_attempt(denied).unwrap();
        assert!(
            store
                .receipt(id, PermittedRange::new(0, 1).unwrap(), 1)
                .is_err()
        );
        assert!(store.complete(id, Outcome::Completed, 1).is_err());
        store.complete(id, Outcome::Denied, 0).unwrap();
    }
    #[test]
    fn no_free_text_fields() {
        for text in [
            "cat /secret",
            "/native/path",
            "query text",
            "credential",
            "snippet",
            &"a".repeat(65),
        ] {
            assert!(Identifier::try_from(text.to_owned()).is_err());
        }
        let base = serde_json::to_value(record()).unwrap();
        for field in [
            "body",
            "query",
            "command",
            "native_path",
            "credential",
            "snippet",
        ] {
            let mut bad = base.clone();
            bad[field] = "secret".into();
            assert!(serde_json::from_value::<AttemptRecord>(bad).is_err());
        }
        let mut bad = base.clone();
        bad["purpose"] = "arbitrary text".into();
        assert!(serde_json::from_value::<AttemptRecord>(bad).is_err());
        let mut bad = base;
        bad["source_handle"] = "/native/path".into();
        assert!(serde_json::from_value::<AttemptRecord>(bad).is_err());
    }
    #[test]
    fn custody_and_single_writer() {
        let f = Fixture::new();
        let store = AuditStore::open(&f.0, ClockAt(1), MAX_BYTES).unwrap();
        assert!(matches!(
            AuditStore::open(&f.0, ClockAt(1), MAX_BYTES),
            Err(AuditError::InUse)
        ));
        assert_eq!(
            fs::metadata(f.0.join(AUDIT_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(store);
        fs::set_permissions(f.0.join(AUDIT_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(AuditStore::open(&f.0, ClockAt(1), MAX_BYTES).is_err());
        fs::set_permissions(f.0.join(AUDIT_FILE), fs::Permissions::from_mode(0o600)).unwrap();
        let target = Fixture::new();
        fs::rename(f.0.join(AUDIT_FILE), target.0.join("saved")).unwrap();
        symlink(target.0.join("saved"), f.0.join(AUDIT_FILE)).unwrap();
        assert!(AuditStore::open(&f.0, ClockAt(1), MAX_BYTES).is_err());
        fs::remove_file(f.0.join(AUDIT_FILE)).unwrap();
        fs::set_permissions(&f.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(AuditStore::open(&f.0, ClockAt(1), MAX_BYTES).is_err());
        fs::set_permissions(&f.0, fs::Permissions::from_mode(0o700)).unwrap();
        let alias = Fixture::new();
        symlink(&f.0, alias.0.join("link")).unwrap();
        assert!(AuditStore::open(&alias.0.join("link"), ClockAt(1), MAX_BYTES).is_err());
    }
    #[test]
    fn concurrent_callers_append_complete_records() {
        let f = Fixture::new();
        let store = Arc::new(Mutex::new(
            AuditStore::open(&f.0, ClockAt(1), MAX_BYTES).unwrap(),
        ));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let store = Arc::clone(&store);
                scope.spawn(move || {
                    let mut store = store.lock().unwrap();
                    let id = store.begin_attempt(record()).unwrap();
                    store.complete(id, Outcome::Completed, 1).unwrap();
                });
            }
        });
        assert_eq!(f.events().len(), 16);
    }
    #[test]
    fn torn_journal_and_write_failure_refuse() {
        let f = Fixture::new();
        let mut store = AuditStore::open(&f.0, ClockAt(1), MAX_BYTES).unwrap();
        // A read-only descriptor deterministically simulates an append failure.
        store.file = File::open(f.0.join(AUDIT_FILE)).unwrap();
        assert!(matches!(
            store.begin_attempt(record()),
            Err(AuditError::WriteFailed)
        ));
        assert!(matches!(
            store.begin_attempt(record()),
            Err(AuditError::WriteFailed)
        ));
        drop(store);
        fs::write(f.0.join(AUDIT_FILE), b"{\"event\":").unwrap();
        assert!(matches!(
            AuditStore::open(&f.0, ClockAt(1), MAX_BYTES),
            Err(AuditError::Invalid)
        ));
    }
}

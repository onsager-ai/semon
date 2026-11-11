//! Origin authorization history. Persist before rendering or egress, including
//! authorization-only cache reads. Any error refuses release. No repair or pruning.

use crate::{
    body_cache::{CacheDirectory, Clock},
    receiver_audit::{
        AuditError, Decision, Destination, Identifier, Outcome, PermittedRange, Purpose,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::DirBuilderExt,
    path::Path,
};

pub const AUDIT_FILE: &str = "authorizations.jsonl";
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RECEIPTS: u32 = 16;
const RECORD_CAP: u64 = 8192;
const RESERVE: u64 = (MAX_RECEIPTS as u64 + 1) * RECORD_CAP;
type Result<T> = std::result::Result<T, AuditError>;

/// Opaque cross-journal correlation, never a bearer capability. Only a live store
/// may append receipts/completion for the ID it returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AuthorizationId(Identifier);
impl AuthorizationId {
    pub fn identifier(self) -> Identifier {
        self.0
    }
}

/// Closed metadata; scope and export binding have no enrollment schema here.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationRecord {
    pub request_id: Identifier,
    pub authenticated_principal: Identifier,
    pub source_handle: Identifier,
    pub generation: Identifier,
    pub permitted_range: PermittedRange,
    pub purpose: Purpose,
    pub receiver_policy_version: Identifier,
    pub origin_policy_version: Identifier,
    pub origin_redactor_version: Identifier,
    pub enrollment_scope: Identifier,
    pub export_view_binding: Identifier,
    pub decision: Decision,
    pub destination: Destination,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Authorized {
        format: u32,
        authorization_id: AuthorizationId,
        record: Box<AuthorizationRecord>,
        bytes: u64,
        time: u64,
    },
    Receipt {
        authorization_id: AuthorizationId,
        range: PermittedRange,
        bytes: u64,
        time: u64,
    },
    Completion {
        authorization_id: AuthorizationId,
        outcome: Outcome,
        bytes: u64,
        time: u64,
    },
}
struct Active {
    record: AuthorizationRecord,
    receipts: u32,
    next: u64,
    delivered: u64,
    reserve: u64,
}

/// Explicitly configured owned 0700 directory outside native roots and caches.
/// Path separation is the integration's responsibility. Files are owned 0600,
/// descriptor-relative and exclusively locked. Share a writer through a mutex.
pub struct AuditStore<C: Clock> {
    file: File,
    _directory: CacheDirectory,
    _lock: File,
    clock: C,
    cap: u64,
    used: u64,
    reserved: u64,
    active: BTreeMap<AuthorizationId, Active>,
    failed: bool,
}
impl<C: Clock> AuditStore<C> {
    pub fn open(dir: &Path, clock: C, cap: u64) -> Result<Self> {
        if !dir.exists() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|_| AuditError::Custody)?;
            // Persist the new directory entry as well as its members below.
            File::open(dir.parent().ok_or(AuditError::Custody)?)
                .and_then(|f| f.sync_all())
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
        // Validate current history and ordering; interrupted authorizations remain
        // historical only and are never resumed or converted into success.
        let mut history = BTreeMap::<AuthorizationId, Active>::new();
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
                Event::Authorized {
                    format,
                    authorization_id,
                    record,
                    bytes,
                    ..
                } => {
                    if format != 1
                        || bytes != 0
                        || record.permitted_range.start >= record.permitted_range.end
                        || history.contains_key(&authorization_id)
                    {
                        return Err(AuditError::Invalid);
                    }
                    history.insert(
                        authorization_id,
                        Active {
                            next: record.permitted_range.start,
                            record: *record,
                            receipts: 0,
                            delivered: 0,
                            reserve: 0,
                        },
                    );
                }
                Event::Receipt {
                    authorization_id,
                    range,
                    bytes,
                    ..
                } => {
                    let active = history
                        .get_mut(&authorization_id)
                        .ok_or(AuditError::Invalid)?;
                    validate_receipt(active, range, bytes)?;
                    active.receipts += 1;
                    active.next = range.end;
                    active.delivered += bytes;
                }
                Event::Completion {
                    authorization_id,
                    outcome,
                    bytes,
                    ..
                } => {
                    validate_completion(
                        history.get(&authorization_id).ok_or(AuditError::Invalid)?,
                        outcome,
                        bytes,
                    )?;
                    history.remove(&authorization_id);
                }
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
    /// Fsync decision before any render/egress. A denied record permits only a
    /// zero-byte denied completion. A returned ID alone does not grant access.
    pub fn begin_authorization(&mut self, record: AuthorizationRecord) -> Result<AuthorizationId> {
        if record.permitted_range.start >= record.permitted_range.end {
            return Err(AuditError::Invalid);
        }
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| AuditError::WriteFailed)?;
        let id = AuthorizationId(Identifier::from_bytes(random));
        if self.active.contains_key(&id) {
            return Err(AuditError::Invalid);
        }
        self.append(
            &Event::Authorized {
                format: 1,
                authorization_id: id,
                record: Box::new(record.clone()),
                bytes: 0,
                time: self.clock.now(),
            },
            RESERVE,
        )?;
        self.reserved += RESERVE;
        self.active.insert(
            id,
            Active {
                next: record.permitted_range.start,
                record,
                receipts: 0,
                delivered: 0,
                reserve: RESERVE,
            },
        );
        Ok(id)
    }
    pub fn receipt(
        &mut self,
        id: AuthorizationId,
        range: PermittedRange,
        bytes: u64,
    ) -> Result<()> {
        validate_receipt(
            self.active.get(&id).ok_or(AuditError::Invalid)?,
            range,
            bytes,
        )?;
        self.append_reserved(
            &Event::Receipt {
                authorization_id: id,
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
    pub fn complete(&mut self, id: AuthorizationId, outcome: Outcome, bytes: u64) -> Result<()> {
        validate_completion(
            self.active.get(&id).ok_or(AuditError::Invalid)?,
            outcome,
            bytes,
        )?;
        self.append_reserved(
            &Event::Completion {
                authorization_id: id,
                outcome,
                bytes,
                time: self.clock.now(),
            },
            id,
        )?;
        self.reserved -= self.active.remove(&id).ok_or(AuditError::Invalid)?.reserve;
        Ok(())
    }
    fn append_reserved(&mut self, event: &Event, id: AuthorizationId) -> Result<()> {
        self.reserved -= RECORD_CAP;
        if let Err(error) = self.append(event, 0) {
            self.reserved += RECORD_CAP;
            return Err(error);
        }
        self.active.get_mut(&id).ok_or(AuditError::Invalid)?.reserve -= RECORD_CAP;
        Ok(())
    }
    fn append(&mut self, event: &Event, reserve: u64) -> Result<()> {
        if self.failed {
            return Err(AuditError::WriteFailed);
        }
        let mut bytes = serde_json::to_vec(event).map_err(|_| AuditError::Invalid)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > RECORD_CAP {
            return Err(AuditError::Invalid);
        }
        if self.used + self.reserved + reserve + bytes.len() as u64 > self.cap {
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
fn validate_receipt(active: &Active, range: PermittedRange, bytes: u64) -> Result<()> {
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
    Ok(())
}
fn validate_completion(active: &Active, outcome: Outcome, bytes: u64) -> Result<()> {
    if bytes > active.record.permitted_range.len()
        || bytes < active.delivered
        || (active.record.decision == Decision::Denied
            && (bytes != 0 || outcome != Outcome::Denied))
        || (active.record.decision == Decision::Granted && outcome == Outcome::Denied)
    {
        return Err(AuditError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "origin-audit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct At;
    impl Clock for At {
        fn now(&self) -> u64 {
            42
        }
    }
    fn record() -> AuthorizationRecord {
        let id = Identifier::from_bytes([1; 32]);
        AuthorizationRecord {
            request_id: id,
            authenticated_principal: id,
            source_handle: id,
            generation: id,
            permitted_range: PermittedRange::new(0, 100).unwrap(),
            purpose: Purpose::StreamRange,
            receiver_policy_version: id,
            origin_policy_version: id,
            origin_redactor_version: id,
            enrollment_scope: id,
            export_view_binding: id,
            decision: Decision::Granted,
            destination: Destination::Cli,
        }
    }
    #[test]
    fn durable_bounded_receipts_and_completion_reservation() {
        let f = Fixture::new();
        let mut store = AuditStore::open(&f.0, At, RESERVE + RECORD_CAP).unwrap();
        let id = store.begin_authorization(record()).unwrap();
        let text = fs::read_to_string(f.0.join(AUDIT_FILE)).unwrap();
        assert!(text.contains("\"time\":42"));
        assert!(matches!(
            store.begin_authorization(record()),
            Err(AuditError::Full)
        ));
        for start in 0..16 {
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
        drop(store);
        let _store = AuditStore::open(&f.0, At, MAX_BYTES).unwrap();
        assert_eq!(
            fs::read_to_string(f.0.join(AUDIT_FILE))
                .unwrap()
                .lines()
                .count(),
            18
        );
    }
    #[test]
    fn custody_crash_torn_and_write_refusal() {
        let f = Fixture::new();
        let mut store = AuditStore::open(&f.0, At, MAX_BYTES).unwrap();
        assert!(matches!(
            AuditStore::open(&f.0, At, MAX_BYTES),
            Err(AuditError::InUse)
        ));
        assert_eq!(
            fs::metadata(&f.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(f.0.join(AUDIT_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        store.begin_authorization(record()).unwrap();
        drop(store);
        let mut store = AuditStore::open(&f.0, At, MAX_BYTES).unwrap();
        assert!(store.active.is_empty());
        store.file = File::open(f.0.join(AUDIT_FILE)).unwrap();
        assert!(matches!(
            store.begin_authorization(record()),
            Err(AuditError::WriteFailed)
        ));
        assert!(matches!(
            store.begin_authorization(record()),
            Err(AuditError::WriteFailed)
        ));
        drop(store);
        fs::write(f.0.join(AUDIT_FILE), b"{").unwrap();
        assert!(matches!(
            AuditStore::open(&f.0, At, MAX_BYTES),
            Err(AuditError::Invalid)
        ));
        fs::remove_file(f.0.join(AUDIT_FILE)).unwrap();
        symlink("audit.lock", f.0.join(AUDIT_FILE)).unwrap();
        assert!(AuditStore::open(&f.0, At, MAX_BYTES).is_err());
    }
    #[test]
    fn denied_and_invalid_metadata_refuse_delivery() {
        let f = Fixture::new();
        let mut store = AuditStore::open(&f.0, At, MAX_BYTES).unwrap();
        let mut denied = record();
        denied.decision = Decision::Denied;
        let id = store.begin_authorization(denied).unwrap();
        assert!(
            store
                .receipt(id, PermittedRange::new(0, 1).unwrap(), 1)
                .is_err()
        );
        assert!(store.complete(id, Outcome::Completed, 0).is_err());
        store.complete(id, Outcome::Denied, 0).unwrap();
        let mut json = serde_json::to_value(record()).unwrap();
        json["native_path"] = "/private".into();
        assert!(serde_json::from_value::<AuthorizationRecord>(json).is_err());
    }
}

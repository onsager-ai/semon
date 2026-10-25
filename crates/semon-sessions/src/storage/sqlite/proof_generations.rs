//! Owned immutable selected metadata and durable reference/lease union.
//! No Raw deletion, repair, provider creation or Options database fallback.
#![allow(dead_code)] // Private qualification cohort; public factory remains closed.
use super::failed;
use crate::events::store::configured::NativeOwnership;
use crate::model::summary::CatalogRow;
use crate::storage::proof_lifetime::{
    ProofGenerationId, ProofReaderLease, ReaderPage, ReaderSnapshot,
};
use crate::storage::{NativeReaderBinding, StorageError};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const TTL_MAX: u32 = 60_000;
const LEASE_CAP: i64 = 64;
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS storage_proof_generations(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,publication TEXT NOT NULL,proof TEXT NOT NULL,admission INTEGER NOT NULL,stream TEXT NOT NULL,metadata TEXT NOT NULL,slots TEXT NOT NULL,prefix TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_presentations(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_presentation_roots(namespace TEXT NOT NULL,publication TEXT NOT NULL,base TEXT NOT NULL,manifest TEXT NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(namespace,digest)) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS storage_proof_presentation_immutable BEFORE UPDATE ON storage_proof_presentations BEGIN SELECT RAISE(ABORT,'immutable presentation'); END;
CREATE TRIGGER IF NOT EXISTS storage_proof_presentation_root_immutable BEFORE UPDATE ON storage_proof_presentation_roots BEGIN SELECT RAISE(ABORT,'immutable presentation root'); END;
CREATE TABLE IF NOT EXISTS storage_proof_sources(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,path TEXT NOT NULL,ledger TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key,path)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_proof_source_path ON storage_proof_sources(namespace,path,ledger,id);
CREATE TABLE IF NOT EXISTS storage_proof_refs(namespace TEXT NOT NULL,session_key TEXT NOT NULL,kind TEXT NOT NULL CHECK(kind IN('current','history')),id TEXT NOT NULL,PRIMARY KEY(namespace,session_key,kind,id)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_leases(namespace TEXT NOT NULL,token TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,admission INTEGER NOT NULL,stream TEXT NOT NULL,owner_epoch INTEGER NOT NULL,serial INTEGER NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(namespace,token)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_reader_owners(namespace TEXT PRIMARY KEY,epoch INTEGER NOT NULL CHECK(epoch>0),next_lease INTEGER NOT NULL CHECK(next_lease>=0)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_proof_lease_generation ON storage_proof_leases(namespace,id,session_key,expires);
CREATE TRIGGER IF NOT EXISTS storage_proof_generation_immutable BEFORE UPDATE ON storage_proof_generations BEGIN SELECT RAISE(ABORT,'immutable proof generation'); END;
CREATE TRIGGER IF NOT EXISTS storage_proof_source_immutable BEFORE UPDATE ON storage_proof_sources BEGIN SELECT RAISE(ABORT,'immutable proof source'); END;
";

pub(super) fn initialize(db: &Connection) -> Result<(), StorageError> {
    db.execute_batch(SCHEMA).map_err(failed)?;
    // Compare the complete authored current physical contract, including key
    // order, STRICT/WITHOUT ROWID, indexes and immutable triggers. Never adopt
    // a partial shape.
    // Each authored definition occupies one line. Trigger bodies contain an
    // internal semicolon; keep the complete body when comparing sqlite_schema.
    for statement in SCHEMA
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let statement = statement
            .strip_suffix(';')
            .ok_or(StorageError::Unavailable)?;
        let Some((kind, rest)) = ["TABLE", "INDEX", "TRIGGER"].into_iter().find_map(|kind| {
            statement
                .strip_prefix(&format!("CREATE {kind} IF NOT EXISTS "))
                .map(|rest| (kind, rest))
        }) else {
            continue;
        };
        let name = if kind == "TABLE" {
            rest.split('(').next()
        } else {
            rest.split_whitespace().next()
        }
        .ok_or(StorageError::Unavailable)?;
        let actual: Option<String> = db
            .query_row(
                "SELECT sql FROM main.sqlite_schema WHERE type=?1 AND name=?2",
                params![kind.to_ascii_lowercase(), name],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)?;
        let expected = format!("CREATE {kind} {rest}");
        if actual.as_deref() != Some(expected.as_str()) {
            return Err(StorageError::NamespaceUpgradePending);
        }
        if kind == "TABLE" {
            super::catalog_namespace::guard(db, name)?;
        }
    }
    Ok(())
}

pub(super) struct PreparedPresentations<'a> {
    pub(super) prepared: &'a crate::model::PreparedSources,
    pub(super) rows: &'a [(String, String)],
}

pub(super) fn publish(
    tx: &Transaction<'_>,
    namespace: &str,
    admission: u64,
    stream: &str,
    publication: &str,
    proof: &str,
    presentations: PreparedPresentations<'_>,
) -> Result<(), StorageError> {
    let prepared = presentations.prepared;
    let presentations = presentations.rows;
    let id = ProofGenerationId::derive(namespace, admission, stream, proof);
    let sql_admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    for row in &prepared.rows {
        let metadata = serde_json::to_string(row).map_err(|_| StorageError::Unavailable)?;
        let projection = prepared.transcripts.iter().find(|p| p.key == row.key);
        let slots = serde_json::to_string(&projection.map(|p| &p.slots))
            .map_err(|_| StorageError::Unavailable)?;
        let prefix = crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::Unavailable)?;
        tx.execute(
            "INSERT INTO storage_proof_generations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                namespace,
                id.0,
                row.key,
                publication,
                proof,
                sql_admission,
                stream,
                metadata,
                slots,
                prefix
            ],
        )
        .map_err(failed)?;
        let presentation = presentations.iter().find(|(key, _)| key == &row.key).ok_or(StorageError::Unavailable)?;
        tx.execute("INSERT INTO storage_proof_presentations VALUES(?1,?2,?3,?4)", params![namespace,id.0,row.key,presentation.1]).map_err(failed)?;
        for source in &row.sources {
            let digest =
                crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(source));
            tx.execute(
                "INSERT INTO storage_proof_sources VALUES(?1,?2,?3,?4,?5)",
                params![
                    namespace,
                    id.0,
                    row.key,
                    source.path.to_string_lossy(),
                    digest
                ],
            )
            .map_err(failed)?;
        }
        tx.execute("DELETE FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current'", params![namespace, row.key]).map_err(failed)?;
        tx.execute(
            "INSERT INTO storage_proof_refs VALUES(?1,?2,'current',?3)",
            params![namespace, row.key, id.0],
        )
        .map_err(failed)?;
        let history: Option<String> = tx.query_row("SELECT metadata FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2", params![namespace, row.key], |r| r.get(0)).optional().map_err(failed)?;
        let mut retained = std::collections::BTreeSet::new();
        if let Some(history) = history {
            if history.len() > 1_048_576 {
                return Err(StorageError::HistoryRetentionBounds);
            }
            let history: CatalogRow =
                serde_json::from_str(&history).map_err(|_| StorageError::Unavailable)?;
            if history.sources.len() > 64 {
                return Err(StorageError::HistoryRetentionBounds);
            }
            for source in &history.sources {
                let digest =
                    crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(source));
                let ids = tx.prepare("SELECT id FROM storage_proof_sources WHERE namespace=?1 AND session_key=?2 AND path=?3 AND ledger=?4 LIMIT 65").map_err(failed)?.query_map(params![namespace, row.key, source.path.to_string_lossy(), digest], |r| r.get::<_, String>(0)).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
                retained.extend(ids);
                if retained.len() > 64 {
                    return Err(StorageError::HistoryRetentionBounds);
                }
            }
        }
        tx.execute("DELETE FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='history'", params![namespace, row.key]).map_err(failed)?;
        for retained in retained {
            tx.execute(
                "INSERT INTO storage_proof_refs VALUES(?1,?2,'history',?3)",
                params![namespace, row.key, retained],
            )
            .map_err(failed)?;
        }
    }
    Ok(())
}

/// Add durable references before the existing proof-copy/collection algorithm.
/// An absent S2 family is an unbound current profile, never a reader fallback.
pub(crate) fn referenced(db: &Connection, path: &str) -> rusqlite::Result<Vec<String>> {
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='storage_proof_sources')",
        [],
        |r| r.get(0),
    )?;
    if !present {
        return Ok(Vec::new());
    }
    let mut statement = db.prepare("SELECT DISTINCT s.ledger FROM storage_proof_sources s WHERE s.namespace=semon_namespace() AND s.path=?1 AND (EXISTS(SELECT 1 FROM storage_proof_refs r WHERE r.namespace=s.namespace AND r.id=s.id AND r.session_key=s.session_key) OR EXISTS(SELECT 1 FROM storage_proof_leases l JOIN storage_proof_generations g ON g.namespace=l.namespace AND g.id=l.id AND g.session_key=l.session_key JOIN storage_namespaces n ON n.namespace=l.namespace WHERE l.namespace=s.namespace AND l.id=s.id AND l.session_key=s.session_key AND l.admission=n.admission_generation AND l.owner_epoch=(SELECT epoch FROM storage_proof_reader_owners o WHERE o.namespace=l.namespace) AND l.stream=g.stream AND l.stream=(SELECT value FROM main.meta WHERE namespace='' AND key='catalog_changes_epoch') AND l.expires>?2)) LIMIT 257")?;
    let values = statement
        .query_map(params![path, crate::model::now_ms()], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if values.len() > 256 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(values)
}

/// Delete at most `budget` metadata generations. This never touches Raw or
/// interprets lease expiry as completion of a remote write.
pub(crate) fn collect(
    tx: &Transaction<'_>,
    namespace: &str,
    budget: usize,
) -> rusqlite::Result<usize> {
    let candidates = tx.prepare("SELECT id,session_key FROM storage_proof_generations g WHERE namespace=?1 AND NOT EXISTS(SELECT 1 FROM storage_proof_refs r WHERE r.namespace=g.namespace AND r.id=g.id AND r.session_key=g.session_key) AND NOT EXISTS(SELECT 1 FROM storage_proof_leases l JOIN storage_namespaces n ON n.namespace=l.namespace WHERE l.namespace=g.namespace AND l.id=g.id AND l.session_key=g.session_key AND l.admission=n.admission_generation AND l.owner_epoch=(SELECT epoch FROM storage_proof_reader_owners o WHERE o.namespace=l.namespace) AND l.stream=g.stream AND l.stream=(SELECT value FROM main.meta WHERE namespace='' AND key='catalog_changes_epoch') AND l.expires>?2) ORDER BY id,session_key LIMIT ?3")?.query_map(params![namespace,crate::model::now_ms(),budget.min(64) as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, key) in &candidates {
        tx.execute("DELETE FROM storage_proof_presentations WHERE namespace=?1 AND id=?2 AND session_key=?3", params![namespace,id,key])?;

        tx.execute(
            "DELETE FROM storage_proof_sources WHERE namespace=?1 AND id=?2 AND session_key=?3",
            params![namespace, id, key],
        )?;
        tx.execute(
            "DELETE FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3",
            params![namespace, id, key],
        )?;
    }
    Ok(candidates.len())
}

pub(crate) struct Lease {
    owner: NativeOwnership,
    owner_epoch: i64,
    serial: i64,
    token: String,
    snapshot: ReaderSnapshot,
    row: CatalogRow,
    released: bool,
}
fn ttl(ttl: u32) -> Result<i64, StorageError> {
    if ttl == 0 || ttl > TTL_MAX {
        return Err(StorageError::InvalidArguments);
    }
    crate::model::now_ms()
        .checked_add(i64::from(ttl))
        .ok_or(StorageError::InvalidArguments)
}
/// One host runtime owns reader leases independently of writer batch epochs.
/// Clone this handle for concurrent requests; open explicitly on takeover.
#[derive(Clone)]
pub(crate) struct ReaderRuntime {
    owner: NativeOwnership,
    epoch: i64,
}
impl NativeReaderBinding {
    pub(crate) fn reader_runtime(&self) -> Result<ReaderRuntime, StorageError> {
        let Self::Semantic(owner) = self else {
            return Err(StorageError::NamespaceUpgradePending);
        };
        let mut db = super::source_journal::SourceJournal::owned(owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        owner.validate_ready(&tx)?;
        let ns = owner.config().namespace.as_str();
        if tx.execute("INSERT INTO storage_proof_reader_owners VALUES(?1,1,0) ON CONFLICT(namespace) DO UPDATE SET epoch=epoch+1,next_lease=0 WHERE epoch<9223372036854775807", [ns]).map_err(failed)? != 1 {
            return Err(StorageError::Conflict);
        }
        let epoch = tx
            .query_row(
                "SELECT epoch FROM storage_proof_reader_owners WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(ReaderRuntime {
            owner: owner.clone(),
            epoch,
        })
    }
}
impl ReaderRuntime {
    fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        self.owner.validate_ready(db)?;
        let epoch: i64 = db
            .query_row(
                "SELECT epoch FROM storage_proof_reader_owners WHERE namespace=?1",
                [self.owner.config().namespace.as_str()],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if epoch != self.epoch {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
    pub(crate) fn current_generation(&self, key: &str) -> Result<ProofGenerationId, StorageError> {
        let db = super::source_journal::SourceJournal::owned(self.owner.clone()).connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate(&db)?;
        let id = db.query_row("SELECT id FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current'", params![self.owner.config().namespace.as_str(), key], |r| r.get(0)).map_err(failed)?;
        Ok(ProofGenerationId(id))
    }
    /// Retry unknown ACK with the SAME token AND expected generation. Expiry
    /// never selects a new generation or reports a remote operation completed.
    pub(crate) fn selected_generation(
        &self,
        key: &str,
        expected: &ProofGenerationId,
        token: &str,
        ttl_ms: u32,
    ) -> Result<Lease, StorageError> {
        let owner = &self.owner;
        let sql_admission =
            i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidArguments);
        }
        let expires = ttl(ttl_ms)?;
        let mut db = super::source_journal::SourceJournal::owned(owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        let ns = owner.config().namespace.as_str();
        let existing: Option<(String, String, i64)> = tx.query_row("SELECT id,session_key,serial FROM storage_proof_leases WHERE namespace=?1 AND token=?2", params![ns, token], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(failed)?;
        let (id, serial) = if let Some((id, session, serial)) = existing {
            if session != key || id != expected.0 {
                return Err(StorageError::Conflict);
            }
            (id, serial)
        } else {
            // Expired/fenced rows are no longer admitted readers. Their removal
            // cannot rebind a retry: expected generation is an explicit input.
            tx.execute("DELETE FROM storage_proof_leases WHERE namespace=?1 AND token IN (SELECT token FROM storage_proof_leases WHERE namespace=?1 AND (expires<=?2 OR owner_epoch<>?3) LIMIT 64)", params![ns, crate::model::now_ms(), self.epoch]).map_err(failed)?;
            let count: i64 = tx
                .query_row(
                    "SELECT count(*) FROM storage_proof_leases WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            if count >= LEASE_CAP {
                return Err(StorageError::Busy);
            }
            if tx.execute("UPDATE storage_proof_reader_owners SET next_lease=next_lease+1 WHERE namespace=?1 AND epoch=?2 AND next_lease<9223372036854775807", params![ns, self.epoch]).map_err(failed)? != 1 {
                return Err(StorageError::Conflict);
            }
            let serial: i64 = tx
                .query_row(
                    "SELECT next_lease FROM storage_proof_reader_owners WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            tx.execute(
                "INSERT INTO storage_proof_leases VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    ns,
                    token,
                    expected.0,
                    key,
                    sql_admission,
                    owner.config().expected_stream_epoch,
                    self.epoch,
                    serial,
                    expires
                ],
            )
            .map_err(failed)?;
            (expected.0.clone(), serial)
        };
        let (publication, proof, metadata, prefix, admission, stream): (String, String, String, String, i64, String) = tx.query_row("SELECT publication,proof,metadata,prefix,admission,stream FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3", params![ns, id, key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).map_err(failed)?;
        if ProofGenerationId::derive(ns, owner.admission(), &stream, &proof)
            != ProofGenerationId(id.clone())
        {
            return Err(StorageError::ScopeChanged);
        }
        let admission = u64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
        if admission != owner.admission()
            || Some(&stream) != owner.config().expected_stream_epoch.as_ref()
        {
            return Err(StorageError::ScopeChanged);
        }
        let row: CatalogRow =
            serde_json::from_str(&metadata).map_err(|_| StorageError::Unavailable)?;
        if crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::Unavailable)?
            != prefix
        {
            return Err(StorageError::ScopeChanged);
        }
        let lease = Lease {
            owner: owner.clone(),
            owner_epoch: self.epoch,
            serial,
            token: token.into(),
            snapshot: ReaderSnapshot {
                namespace: owner.config().namespace.clone(),
                admission,
                stream_epoch: stream,
                publication_generation: publication,
                proof_generation: ProofGenerationId(id),
                consumed_prefix: prefix,
            },
            row,
            released: false,
        };
        lease.validate(&tx)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(lease)
    }
}
impl Lease {
    fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        self.owner.validate_ready(db)?;
        let proof: Option<String> = db.query_row("SELECT g.proof FROM storage_proof_generations g JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3", params![self.snapshot.namespace.as_str(), self.snapshot.proof_generation.0, self.row.key], |r| r.get(0)).optional().map_err(failed)?;
        let Some(proof) = proof else {
            return Err(StorageError::ScopeChanged);
        };
        if ProofGenerationId::derive(
            self.snapshot.namespace.as_str(),
            self.snapshot.admission,
            &self.snapshot.stream_epoch,
            &proof,
        ) != self.snapshot.proof_generation
        {
            return Err(StorageError::ScopeChanged);
        }
        validate_presentation(db, &self.snapshot, &self.row.key, &proof)?;
        let metadata = serde_json::to_string(&self.row).map_err(|_| StorageError::Unavailable)?;
        let sql_admission =
            i64::try_from(self.snapshot.admission).map_err(|_| StorageError::ScopeChanged)?;
        let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM storage_proof_leases l JOIN storage_proof_generations g ON g.namespace=l.namespace AND g.id=l.id AND g.session_key=l.session_key WHERE l.namespace=?1 AND l.token=?2 AND l.id=?3 AND l.session_key=?4 AND l.admission=?5 AND l.stream=?6 AND l.owner_epoch=?8 AND l.owner_epoch=(SELECT epoch FROM storage_proof_reader_owners o WHERE o.namespace=l.namespace) AND l.expires>?7 AND g.admission=l.admission AND g.stream=l.stream AND g.metadata=?9 AND g.prefix=?10 AND g.publication=?11 AND l.serial=?12)", params![self.snapshot.namespace.as_str(), self.token, self.snapshot.proof_generation.0, self.row.key, sql_admission, self.snapshot.stream_epoch, crate::model::now_ms(), self.owner_epoch, metadata, self.snapshot.consumed_prefix, self.snapshot.publication_generation, self.serial], |r| r.get(0)).map_err(failed)?;
        if self.released || !valid {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
}

struct ControlledReader<'a> {
    reader: &'a dyn crate::SessionSourceReader,
    control: &'a crate::SelectedReadControl,
}
impl crate::SessionSourceReader for ControlledReader<'_> {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> std::io::Result<crate::SessionSourceRange> {
        self.control.check()?;
        let result = self.reader.read_range(source, expected, offset, max)?;
        self.control.check()?;
        Ok(result)
    }
}

impl ProofReaderLease for Lease {
    fn snapshot(&self) -> &ReaderSnapshot {
        &self.snapshot
    }
    fn row(&self) -> &CatalogRow {
        &self.row
    }
    fn renew(&mut self, ttl_ms: u32) -> Result<(), StorageError> {
        let expires = ttl(ttl_ms)?;
        let mut db =
            super::source_journal::SourceJournal::owned(self.owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        tx.execute(
            "UPDATE storage_proof_leases SET expires=?3 WHERE namespace=?1 AND token=?2",
            params![self.snapshot.namespace.as_str(), self.token, expires],
        )
        .map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)
    }
    fn release(&mut self) -> Result<(), StorageError> {
        let mut db =
            super::source_journal::SourceJournal::owned(self.owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.owner.validate_ready(&tx)?;
        let epoch: i64 = tx
            .query_row(
                "SELECT epoch FROM storage_proof_reader_owners WHERE namespace=?1",
                [self.snapshot.namespace.as_str()],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if epoch != self.owner_epoch {
            return Err(StorageError::ScopeChanged);
        }
        tx.execute("DELETE FROM storage_proof_leases WHERE namespace=?1 AND token=?2 AND id=?3 AND owner_epoch=?4 AND serial=?5", params![self.snapshot.namespace.as_str(), self.token, self.snapshot.proof_generation.0, self.owner_epoch, self.serial]).map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        self.released = true;
        Ok(())
    }
    fn read_record_with_control(
        &self,
        options: &crate::Options,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
        path: &std::path::Path,
        offset: u64,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<Option<serde_json::Value>> {
        control.check()?;
        let binding = NativeReaderBinding::Semantic(self.owner.clone());
        let db = binding.connection().map_err(std::io::Error::other)?;
        db.execute_batch("BEGIN DEFERRED")
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        let sources = crate::query_source::Sources::capture_bound_snapshot(
            options,
            &self.row.sources,
            db,
            &self.owner,
        )?;
        let records = sources.reader_with(provider);
        let source = records
            .source_ref(path)
            .ok_or_else(|| std::io::Error::other("selected source unavailable"))?;
        let controlled = ControlledReader {
            reader: &records,
            control,
        };
        let mut budget = 1_048_576;
        let record = crate::source_reader::read_record_bounded(
            &controlled,
            source,
            offset,
            1_048_576,
            &mut budget,
        )?;
        records.finish()?;
        // A fresh authority/lease check fences revocation during byte I/O.
        let db = binding.connection().map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        control.check()?;
        Ok(Some(record))
    }
    fn transcript_records(
        &self,
        options: &crate::Options,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
        start: usize,
        limit: usize,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<ReaderPage> {
        control.check()?;
        if limit == 0 || limit > 8 {
            return Err(std::io::Error::other("private selected page budget"));
        }
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(std::io::Error::other)?;
        db.execute_batch("BEGIN DEFERRED")
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        let text: String = db.query_row("SELECT slots FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3", params![self.snapshot.namespace.as_str(), self.snapshot.proof_generation.0, self.row.key], |r| r.get(0)).map_err(std::io::Error::other)?;
        if text.len() > 1_048_576 {
            return Err(std::io::Error::other("private selected recipe budget"));
        }
        let slots: Option<Vec<crate::slot_projection::SavedSlot>> =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        let slots = slots.ok_or_else(|| std::io::Error::other("selected recipes unavailable"))?;
        if start > slots.len() {
            return Err(std::io::Error::other("selected page range"));
        }
        let sources = crate::query_source::Sources::capture_bound_snapshot(
            options,
            &self.row.sources,
            db,
            &self.owner,
        )?;
        let native = sources.reader_with(provider);
        let controlled = ControlledReader {
            reader: &native,
            control,
        };
        let mut records = Vec::new();
        let mut bytes = 0usize;
        for slot in slots.into_iter().skip(start).take(limit) {
            control.check()?;
            let value = if let Some(file) = &slot.file {
                let source = native
                    .source_ref(&file.path)
                    .ok_or_else(|| std::io::Error::other("selected source unavailable"))?;
                let mut budget = 1_048_576;
                Some(crate::source_reader::read_record_bounded(
                    &controlled,
                    source,
                    slot.offset,
                    1_048_576,
                    &mut budget,
                )?)
            } else {
                None
            };
            bytes = bytes.saturating_add(
                serde_json::to_vec(&value)
                    .map_err(std::io::Error::other)?
                    .len(),
            );
            if bytes > 2 * 1_048_576 {
                return Err(std::io::Error::other("private selected body budget"));
            }
            records.push((slot, value));
        }
        native.finish()?;
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        control.check()?;
        Ok(ReaderPage {
            snapshot: self.snapshot.clone(),
            records,
        })
    }
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn joined_digest(base: &str, manifest: &str) -> Result<String, StorageError> {
    Ok(digest(&serde_json::to_vec(&("selected-presentation-v1", base, manifest)).map_err(|_| StorageError::Unavailable)?))
}

/// The caller has completed every producer write in the same publication TX.
/// Capture typed metadata, never Raw bodies, before committing the proof root.
pub(super) fn capture_presentations(
    tx: &Transaction<'_>, ns: &str, generation: &str,
    prepared: &crate::model::PreparedSources, observed_at: i64,
) -> Result<Vec<(String, String)>, StorageError> {
    use crate::storage::selected::{CapturedContext, PresentationWitness};
    let lookup = super::selected::Context::owned(tx);
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for row in &prepared.rows {
        let witness = if let Some(projection) = prepared.transcripts.iter().find(|p| p.key == row.key) {
            let texts = projection.slots.iter().map(serde_json::to_string).collect::<Result<Vec<_>,_>>().map_err(|_| StorageError::Unavailable)?;
            let header_generation = crate::slot_projection::generation(&texts, &row.sources).map_err(|_| StorageError::Unavailable)?;
            let header: (String,u32,i64) = tx.query_row("SELECT generation,version,total FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2",params![ns,row.key], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
            if header.0 != header_generation || header.1 != crate::slot_projection::VERSION || usize::try_from(header.2).ok() != Some(texts.len()) { return Err(StorageError::ScopeChanged); }
            let has_history = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2)",params![ns,row.key],|r|r.get(0)).map_err(failed)?;
            let catalog = [
                ("catalog_version".into(),crate::model::summary::CATALOG_VERSION.to_string()),
                ("catalog_generation".into(),generation.to_owned()),
                ("slot_projection_catalog_generation".into(),generation.to_owned()),
                ("catalog_completeness".into(),"partial".into()),
                ("catalog_observed_at".into(),observed_at.to_string()),
            ].into_iter().collect();
            let observations = row.query.as_ref().and_then(|query| query.get("selected_presentation_observations")).ok_or(StorageError::Unavailable)?;
            let mut observations: crate::storage::selected::PresentationObservations = serde_json::from_value(observations.clone()).map_err(|_| StorageError::Unavailable)?;
            let context = CapturedContext::capture(&lookup,row,&projection.slots,&mut |relative| observations.capture_relative_status(relative)).map_err(|_| StorageError::Unavailable)?;
            Some(PresentationWitness { header_generation,version:header.1,total:texts.len(),has_history,catalog,context,observations })
        } else { None };
        let text = serde_json::to_string(&witness).map_err(|_| StorageError::Unavailable)?;
        bytes = bytes.saturating_add(text.len());
        if bytes > 8 * 1048576 { return Err(StorageError::HistoryRetentionBounds); }
        result.push((row.key.clone(),text));
    }
    Ok(result)
}

pub(super) fn bind_presentations(tx: &Transaction<'_>, ns: &str, publication: &str, base: &str, rows: &[(String,String)], prepared: &crate::model::PreparedSources) -> Result<String, StorageError> {
    let mut entries = Vec::new();
    for (key,text) in rows {
        let row = prepared.rows.iter().find(|row| &row.key == key).ok_or(StorageError::Unavailable)?;
        let metadata = serde_json::to_string(row).map_err(|_| StorageError::Unavailable)?;
        let projection = prepared.transcripts.iter().find(|projection| &projection.key == key);
        let slots = serde_json::to_string(&projection.map(|projection| &projection.slots)).map_err(|_| StorageError::Unavailable)?;
        let bytes = serde_json::to_vec(&(&metadata,&slots,text)).map_err(|_| StorageError::Unavailable)?;
        entries.push((key,digest(&bytes)));
    }
    let manifest = serde_json::to_string(&entries).map_err(|_| StorageError::Unavailable)?;
    if manifest.len() > 1048576 { return Err(StorageError::HistoryRetentionBounds); }
    let joined = joined_digest(base,&manifest)?;
    tx.execute("INSERT INTO storage_proof_presentation_roots VALUES(?1,?2,?3,?4,?5)",params![ns,publication,base,manifest,joined]).map_err(failed)?;
    Ok(joined)
}
pub(super) fn replayed_presentation(db: &Connection, ns: &str, publication: &str, base: &str, expected: &str) -> Result<bool, StorageError> {
    let row: Option<(String,String,String)> = db.query_row("SELECT base,manifest,digest FROM storage_proof_presentation_roots WHERE namespace=?1 AND publication=?2 AND digest=?3",params![ns,publication,expected],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(failed)?;
    let Some((saved,manifest,joined)) = row else { return Ok(false); };
    Ok(saved == base && manifest.len() <= 1048576 && joined == expected && joined_digest(&saved,&manifest)? == joined)
}
fn validate_presentation(db: &Connection, snapshot: &ReaderSnapshot, key: &str, proof: &str) -> Result<String, StorageError> {
    let (base,manifest,joined): (String,String,String) = db.query_row("SELECT base,manifest,digest FROM storage_proof_presentation_roots WHERE namespace=?1 AND publication=?2 AND digest=?3",params![snapshot.namespace.as_str(),snapshot.publication_generation,proof],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    if manifest.len() > 1048576 || joined != proof || joined_digest(&base,&manifest)? != joined { return Err(StorageError::ScopeChanged); }
    let rows: Vec<(String,String)> = serde_json::from_str(&manifest).map_err(|_| StorageError::Unavailable)?;
    let (text,metadata,slots): (String,String,String) = db.query_row("SELECT CASE WHEN octet_length(p.metadata)<=8388608 THEN p.metadata END,CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END FROM storage_proof_presentations p JOIN storage_proof_generations g ON g.namespace=p.namespace AND g.id=p.id AND g.session_key=p.session_key WHERE p.namespace=?1 AND p.id=?2 AND p.session_key=?3",params![snapshot.namespace.as_str(),snapshot.proof_generation.0,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    let bytes = serde_json::to_vec(&(&metadata,&slots,&text)).map_err(|_| StorageError::Unavailable)?;
    if rows.iter().filter(|(k,_)|k == key).count() != 1 || !rows.iter().any(|(k,d)|k == key && *d == digest(&bytes)) { return Err(StorageError::ScopeChanged); }
    Ok(text)
}

impl crate::storage::selected::SelectedSnapshotStore for Lease {
    fn open_selected(&self, options: &crate::Options, host: &crate::storage::selected::HostSourceBinding, control: &crate::SelectedReadControl) -> std::io::Result<crate::storage::selected::SelectedRead> {
        use std::io;
        control.check()?;
        host.validate(&self.snapshot)?;
        let db = NativeReaderBinding::Semantic(self.owner.clone()).connection().map_err(io::Error::other)?;
        db.execute_batch("BEGIN DEFERRED").map_err(io::Error::other)?;
        self.validate(&db).map_err(io::Error::other)?;
        let text: String = db.query_row("SELECT metadata FROM storage_proof_presentations WHERE namespace=?1 AND id=?2 AND session_key=?3",params![self.snapshot.namespace.as_str(),self.snapshot.proof_generation.0,self.row.key],|r|r.get(0)).map_err(io::Error::other)?;
        let witness: Option<crate::storage::selected::PresentationWitness> = serde_json::from_str(&text).map_err(io::Error::other)?;
        let witness = witness.ok_or_else(|| io::Error::other("selected presentation unavailable"))?;
        let text: String = db.query_row("SELECT CASE WHEN octet_length(slots)<=1048576 THEN slots END FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3",params![self.snapshot.namespace.as_str(),self.snapshot.proof_generation.0,self.row.key],|r|r.get(0)).map_err(io::Error::other)?;
        let slots: Option<Vec<crate::slot_projection::SavedSlot>> = serde_json::from_str(&text).map_err(io::Error::other)?;
        let slots = slots.ok_or_else(||io::Error::other("selected recipes unavailable"))?;
        let texts = slots.iter().map(serde_json::to_string).collect::<Result<Vec<_>,_>>().map_err(io::Error::other)?;
        if texts.iter().any(|text|text.len()>65536) || slots.len() != witness.total || witness.version != crate::slot_projection::VERSION || crate::slot_projection::generation(&texts,&self.row.sources).map_err(io::Error::other)? != witness.header_generation || witness.catalog.get("catalog_generation") != Some(&self.snapshot.publication_generation) {
            return Err(io::Error::other("selected witness changed"));
        }
        let sources = crate::query_source::Sources::capture_bound_snapshot(options,&self.row.sources,db,&self.owner)?;
        control.check()?;
        Ok(crate::storage::selected::SelectedRead { snapshot:self.snapshot.clone(),row:self.row.clone(),witness,slots,sources })
    }
    fn finish_selected(&self, control: &crate::SelectedReadControl) -> std::io::Result<()> {
        control.check()?;
        let db = NativeReaderBinding::Semantic(self.owner.clone()).connection().map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        control.check()
    }
}

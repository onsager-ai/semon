//! Owned immutable selected metadata and durable reference/lease union.
//! No Raw deletion, repair, provider creation or Options database fallback.
#![allow(dead_code)] // Private qualification cohort; public factory remains closed.
use super::failed;
use crate::events::store::configured::NativeOwnership;
use crate::model::summary::CatalogRow;
use crate::storage::proof_codec::{self, LEASE_CAP, PresentationManifest, TTL_MAX};
use crate::storage::proof_lifetime::{
    ProofGenerationId, ProofReaderLease, ReaderPage, ReaderSnapshot,
};
use crate::storage::{NativeReaderBinding, StorageError};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS storage_proof_generations(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,publication TEXT NOT NULL,proof TEXT NOT NULL,admission INTEGER NOT NULL,stream TEXT NOT NULL,metadata TEXT NOT NULL,slots TEXT NOT NULL,prefix TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_presentations(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_presentation_roots(namespace TEXT NOT NULL,publication TEXT NOT NULL,base TEXT NOT NULL,manifest TEXT NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(namespace,digest)) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS storage_proof_presentation_immutable BEFORE UPDATE ON storage_proof_presentations BEGIN SELECT RAISE(ABORT,'immutable presentation'); END;
CREATE TRIGGER IF NOT EXISTS storage_proof_presentation_root_immutable BEFORE UPDATE ON storage_proof_presentation_roots BEGIN SELECT RAISE(ABORT,'immutable presentation root'); END;
CREATE TABLE IF NOT EXISTS storage_proof_sources(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,path TEXT NOT NULL,ledger TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key,path)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_proof_source_path ON storage_proof_sources(namespace,path,ledger,id);
CREATE TABLE IF NOT EXISTS storage_proof_refs(namespace TEXT NOT NULL,session_key TEXT NOT NULL,kind TEXT NOT NULL CHECK(kind IN('current','history')),id TEXT NOT NULL,PRIMARY KEY(namespace,session_key,kind,id)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_catalog_native_observation_grants(namespace TEXT NOT NULL,session_key TEXT NOT NULL,owner_kind TEXT NOT NULL CHECK(owner_kind IN('catalog','history')),metadata_digest TEXT NOT NULL CHECK(length(metadata_digest)=64 AND metadata_digest NOT GLOB '*[^0-9a-f]*'),admission INTEGER NOT NULL CHECK(admission>0),stream TEXT NOT NULL CHECK(length(stream)=64 AND lower(stream) NOT GLOB '*[^0-9a-f]*'),PRIMARY KEY(namespace,session_key,owner_kind)) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS storage_catalog_native_observation_grants_insert BEFORE INSERT ON storage_catalog_native_observation_grants WHEN semon_semantic_publisher()<>1 BEGIN SELECT RAISE(ABORT,'native observation publisher refused'); END;
CREATE TRIGGER IF NOT EXISTS storage_catalog_native_observation_grants_update BEFORE UPDATE ON storage_catalog_native_observation_grants WHEN semon_semantic_publisher()<>1 BEGIN SELECT RAISE(ABORT,'native observation publisher refused'); END;
CREATE TRIGGER IF NOT EXISTS storage_catalog_native_observation_grants_delete BEFORE DELETE ON storage_catalog_native_observation_grants WHEN semon_semantic_publisher()<>1 BEGIN SELECT RAISE(ABORT,'native observation publisher refused'); END;
CREATE TABLE IF NOT EXISTS storage_proof_leases(namespace TEXT NOT NULL,token TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,admission INTEGER NOT NULL,stream TEXT NOT NULL,owner_epoch INTEGER NOT NULL,serial INTEGER NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(namespace,token)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_reader_owners(namespace TEXT PRIMARY KEY,epoch INTEGER NOT NULL CHECK(epoch>0),next_lease INTEGER NOT NULL CHECK(next_lease>=0)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_proof_lease_generation ON storage_proof_leases(namespace,id,session_key,expires);
CREATE TRIGGER IF NOT EXISTS storage_proof_generation_immutable BEFORE UPDATE ON storage_proof_generations BEGIN SELECT RAISE(ABORT,'immutable proof generation'); END;
CREATE TRIGGER IF NOT EXISTS storage_proof_source_immutable BEFORE UPDATE ON storage_proof_sources BEGIN SELECT RAISE(ABORT,'immutable proof source'); END;
";

pub(super) fn initialize(db: &Connection) -> Result<(), StorageError> {
    db.execute_batch(SCHEMA).map_err(failed)?;
    for statement in SCHEMA.lines().map(str::trim) {
        if let Some(rest) = statement.strip_prefix("CREATE TABLE IF NOT EXISTS ") {
            let name = rest.split('(').next().ok_or(StorageError::Unavailable)?;
            super::catalog_namespace::guard(db, name)?;
        }
    }
    validate(db)
}

pub(super) fn validate(db: &Connection) -> Result<(), StorageError> {
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
            super::catalog_namespace::validate_guard(db, name)?;
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
        let record = proof_codec::GenerationRecord {
            namespace: namespace.to_owned(),
            id: id.clone(),
            session_key: row.key.clone(),
            publication: publication.to_owned(),
            proof: proof.to_owned(),
            admission,
            stream: stream.to_owned(),
            metadata,
            slots,
            prefix,
        };
        // These values came directly from the prepared model in this owner
        // transaction. Decode only persisted input, not this freshly encoded row.
        tx.execute(
            "INSERT INTO storage_proof_generations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                namespace,
                record.id.0,
                record.session_key,
                record.publication,
                record.proof,
                sql_admission,
                record.stream,
                record.metadata,
                record.slots,
                record.prefix
            ],
        )
        .map_err(failed)?;
        let presentation = presentations
            .iter()
            .find(|(key, _)| key == &row.key)
            .ok_or(StorageError::Unavailable)?;
        tx.execute(
            "INSERT INTO storage_proof_presentations VALUES(?1,?2,?3,?4)",
            params![namespace, id.0, row.key, presentation.1],
        )
        .map_err(failed)?;
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
    // Outgoing owners are absent from prepared.rows. Their original custody
    // has completed above publication's CAS, so move their existing selected
    // authority to history before collection can observe an unreferenced gap.
    for row in &prepared.relationships.retired_owners {
        tx.execute("INSERT INTO storage_proof_refs(namespace,session_key,kind,id) SELECT r.namespace,r.session_key,'history',r.id FROM storage_proof_refs r WHERE r.namespace=?1 AND r.session_key=?2 AND r.kind='current' AND EXISTS(SELECT 1 FROM session_history_catalog h WHERE h.namespace=r.namespace AND h.session_key=r.session_key) ON CONFLICT DO NOTHING", params![namespace,row.key]).map_err(failed)?;
        tx.execute("DELETE FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current'", params![namespace,row.key]).map_err(failed)?;
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
        let (publication, proof, metadata, slots, prefix, admission, stream): (String, String, String, String, String, i64, String) = tx.query_row("SELECT g.publication,p.proof_digest,g.metadata,g.slots,g.prefix,g.admission,g.stream FROM storage_proof_generations g JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3", params![ns, id, key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(failed)?;
        let admission = u64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
        validate_publication_owner(&tx, owner, &publication, admission)?;
        if Some(&stream) != owner.config().expected_stream_epoch.as_ref() {
            return Err(StorageError::ScopeChanged);
        }
        let (record, row) = proof_codec::decode_generation(
            proof_codec::GenerationRecord {
                namespace: ns.into(),
                id: ProofGenerationId(id.clone()),
                session_key: key.into(),
                publication: publication.clone(),
                proof: proof.clone(),
                admission,
                stream: stream.clone(),
                metadata,
                slots,
                prefix: prefix.clone(),
            },
            proof_codec::ExpectedGeneration {
                namespace: ns,
                id: expected,
                admission,
                stream: owner
                    .config()
                    .expected_stream_epoch
                    .as_deref()
                    .ok_or(StorageError::ScopeChanged)?,
                proof: &proof,
                publication: &publication,
                prefix: &prefix,
            },
        )?;
        let prefix = record.prefix;
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
        validate_publication_owner(
            db,
            &self.owner,
            &self.snapshot.publication_generation,
            self.snapshot.admission,
        )?;
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
            i64::try_from(self.owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
        let producer_admission =
            i64::try_from(self.snapshot.admission).map_err(|_| StorageError::ScopeChanged)?;
        let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM storage_proof_leases l JOIN storage_proof_generations g ON g.namespace=l.namespace AND g.id=l.id AND g.session_key=l.session_key WHERE l.namespace=?1 AND l.token=?2 AND l.id=?3 AND l.session_key=?4 AND l.admission=?5 AND l.stream=?6 AND l.owner_epoch=?8 AND l.owner_epoch=(SELECT epoch FROM storage_proof_reader_owners o WHERE o.namespace=l.namespace) AND l.expires>?7 AND g.admission=?13 AND g.stream=l.stream AND g.metadata=?9 AND g.prefix=?10 AND g.publication=?11 AND l.serial=?12)", params![self.snapshot.namespace.as_str(), self.token, self.snapshot.proof_generation.0, self.row.key, sql_admission, self.snapshot.stream_epoch, crate::model::now_ms(), self.owner_epoch, metadata, self.snapshot.consumed_prefix, self.snapshot.publication_generation, self.serial, producer_admission], |r| r.get(0)).map_err(failed)?;
        if self.released || !valid {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
}

pub(crate) fn imported_catalog(
    db: &Connection,
    binding: &crate::storage::QueryRuntimeBinding,
) -> Result<bool, StorageError> {
    let admission = i64::try_from(binding.admission).map_err(|_| StorageError::ScopeChanged)?;
    let max_bytes = i64::try_from(crate::storage::transfer::MAX_PAGE_BYTES)
        .map_err(|_| StorageError::ScopeChanged)?;
    db.query_row("SELECT EXISTS(SELECT 1 FROM main.meta p WHERE p.namespace=?1 AND p.key='native_transfer_v1:catalog-admission' AND CASE WHEN octet_length(p.value)<=?4 THEN json_extract(p.value,'$[0]')='catalog-import-v1' AND json_extract(p.value,'$[1].namespace')=?1 AND json_extract(p.value,'$[1].stream_epoch')=?2 AND json_extract(p.value,'$[3]')=?3 AND EXISTS(SELECT 1 FROM main.meta a WHERE a.namespace=p.namespace AND a.key='native_transfer_v1:catalog-owner:'||json_extract(p.value,'$[2]') AND a.value=?5) ELSE 0 END)", params![binding.namespace.as_str(), binding.stream_epoch, admission, max_bytes, format!("{}:{}",binding.admission,binding.stream_epoch)], |r| r.get(0)).map_err(failed)
}

/// Query observations use original producer coordinates only after the same
/// destination admission grant that qualifies a retained selected lease.
pub(crate) fn observation_binding(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
) -> Result<crate::storage::QueryRuntimeBinding, StorageError> {
    let ns = current.namespace.as_str();
    let grant = format!("{}:{}", current.admission, current.stream_epoch);
    let mut metadata = selected.clone();
    metadata.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let current_metadata =
        serde_json::to_string(&metadata).map_err(|_| StorageError::Unavailable)?;
    metadata.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
    let retained_metadata =
        if selected.lifecycle == crate::model::summary::CatalogLifecycle::Current {
            current_metadata.clone()
        } else {
            serde_json::to_string(&metadata).map_err(|_| StorageError::Unavailable)?
        };
    let admission = i64::try_from(current.admission).map_err(|_| StorageError::ScopeChanged)?;
    for (retained, original) in [(false, &current_metadata), (true, &retained_metadata)] {
        if selected.lifecycle == crate::model::summary::CatalogLifecycle::Current && retained {
            continue;
        }
        let native: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind IN('catalog','history') AND metadata_digest=?3 AND admission=?4 AND stream=?5)",
                params![
                    ns,
                    selected.key,
                    proof_codec::digest(original.as_bytes()),
                    admission,
                    current.stream_epoch
                ],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if native {
            return Ok(current.clone());
        }
    }
    let current_digest = proof_codec::digest(current_metadata.as_bytes());
    let retained_digest = proof_codec::digest(retained_metadata.as_bytes());
    // Stage grants hash the exact metadata and bind its admission/stream. The
    // joined immutable presentation root is checked below against g.metadata.
    let mut statement = db
        .prepare(
            "SELECT DISTINCT g.id,g.publication,g.proof,g.admission,g.stream,
                CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,
                CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,
                g.prefix,r.base,
                CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END,
                CASE WHEN octet_length(v.metadata)<=1048576 THEN v.metadata END,
                (EXISTS(
                    SELECT 1 FROM main.meta m JOIN main.meta a
                      ON a.namespace=m.namespace AND a.key='native_transfer_v1:catalog-owner:'||m.value
                    WHERE m.namespace=f.namespace
                      AND m.key='native_transfer_v1:publication:'||g.publication
                      AND a.value=?3
                ) OR (
                    g.admission=?8 AND g.stream=?9 AND EXISTS(
                        SELECT 1 FROM main.meta m
                        WHERE m.namespace=f.namespace
                          AND m.key='catalog_native_observation:'||(CASE WHEN g.metadata=?4 THEN ?6 ELSE ?7 END)
                          AND m.value=?3
                    ))
                )
             FROM storage_proof_refs f
             JOIN storage_proof_generations g
               ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key
             JOIN storage_semantic_publications p
               ON p.namespace=g.namespace AND p.generation=g.publication
              AND p.proof_digest=g.proof AND p.admission=g.admission
             JOIN storage_proof_presentation_roots r
               ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest
             JOIN storage_proof_presentations v
               ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key
             WHERE f.namespace=?1 AND f.session_key=?2 AND g.metadata IN (?4,?5)
             ORDER BY g.id LIMIT 1",
        )
        .map_err(failed)?;
    let values = statement
        .query_map(
            params![
                ns,
                selected.key,
                grant,
                current_metadata,
                retained_metadata,
                current_digest,
                retained_digest,
                admission,
                current.stream_epoch
            ],
            |r| {
                let admission = u64::try_from(r.get::<_, i64>(3)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                Ok((
                    proof_codec::GenerationRecord {
                        namespace: ns.to_owned(),
                        id: ProofGenerationId(r.get(0)?),
                        session_key: selected.key.clone(),
                        publication: r.get(1)?,
                        proof: r.get(2)?,
                        admission,
                        stream: r.get(4)?,
                        metadata: r.get(5)?,
                        slots: r.get(6)?,
                        prefix: r.get(7)?,
                    },
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, bool>(11)?,
                ))
            },
        )
        .map_err(failed)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(failed)?;
    let (record, base, manifest, presentation, granted) = values
        .into_iter()
        .next()
        .ok_or(StorageError::ScopeChanged)?;
    if !granted {
        return Err(StorageError::ScopeChanged);
    }
    crate::selected_read::charge(
        64,
        record.metadata.len() + record.slots.len() + manifest.len() + presentation.len(),
    )
    .map_err(|_| StorageError::Busy)?;
    proof_codec::verify_presentation_digest(&base, &manifest, &record.proof)?;
    proof_codec::verify_presentation_entry(
        &selected.key,
        &record.metadata,
        &record.slots,
        &presentation,
        &manifest,
    )?;
    proof_codec::observation_binding(record, current, selected)
}

/// Semantic Stage retains exact-row meta grants beside its immutable
/// publication receipts. Direct Catalog grants use owner slots below.
pub(crate) fn native_observation_grant(
    db: &Connection,
    namespace: &str,
    admission: u64,
    stream: &str,
    metadata: &str,
) -> Result<(), StorageError> {
    db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value", params![namespace,format!("catalog_native_observation:{}",proof_codec::digest(metadata.as_bytes())),format!("{admission}:{stream}")]).map_err(failed)?;
    Ok(())
}

/// The direct Catalog writer owns one producer grant for the latest
/// `session_catalog` metadata of each session. Semantic Stage keeps its
/// immutable receipt grants in `meta` through `native_observation_grant`.
pub(crate) fn native_catalog_observation_grant(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    admission: u64,
    stream: &str,
    metadata: &str,
) -> Result<(), StorageError> {
    let mut row: CatalogRow =
        serde_json::from_str(metadata).map_err(|_| StorageError::Unavailable)?;
    if row.key != session_key {
        return Err(StorageError::Unavailable);
    }
    // Catalog lifecycle is a display change. The publisher grant hashes the
    // row in its producer form so either retained display tag selects it.
    row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let metadata = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    db.execute(
        "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream) VALUES(?1,?2,'catalog',?3,?4,?5) ON CONFLICT(namespace,session_key,owner_kind) DO UPDATE SET metadata_digest=excluded.metadata_digest,admission=excluded.admission,stream=excluded.stream",
        params![
            namespace,
            session_key,
            proof_codec::digest(metadata.as_bytes()),
            admission,
            stream
        ],
    )
    .map_err(failed)?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum CatalogObservationOwner {
    Catalog,
    History,
}

/// Publish one producer-repaired Query observation without changing its
/// source admission or rewriting any immutable Stage proof. The old row must
/// still be authorized by its exact current owner grant or retained proof.
pub(crate) fn publish_repaired_query_observation(
    db: &Transaction<'_>,
    current: &crate::storage::QueryRuntimeBinding,
    owner: CatalogObservationOwner,
    selected: &CatalogRow,
    query: &serde_json::Value,
) -> Result<(), StorageError> {
    let source_generation = crate::retention::source_generation(&selected.sources)
        .map_err(|_| StorageError::Unavailable)?;
    if selected.key.is_empty()
        || !query.is_object()
        || query["version"] != 1
        || query["source_generation"].as_str() != Some(source_generation.as_str())
    {
        return Err(StorageError::ScopeChanged);
    }
    let (table, owner_kind) = match owner {
        CatalogObservationOwner::Catalog => ("session_catalog", "catalog"),
        CatalogObservationOwner::History => ("session_history_catalog", "history"),
    };
    let namespace = current.namespace.as_str();
    let stored: Option<Option<String>> = db
        .query_row(
            &format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.{table} WHERE namespace=?1 AND session_key=?2"),
            params![namespace, selected.key],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let stored = match stored {
        Some(Some(stored)) => stored,
        Some(None) => return Err(StorageError::HistoryRetentionBounds),
        None => return Err(StorageError::ScopeChanged),
    };
    let stored_row: CatalogRow =
        serde_json::from_str(&stored).map_err(|_| StorageError::Unavailable)?;
    if serde_json::to_value(&stored_row).map_err(|_| StorageError::Unavailable)?
        != serde_json::to_value(selected).map_err(|_| StorageError::Unavailable)?
    {
        return Err(StorageError::ScopeChanged);
    }

    // Validate the exact preimage through the ordinary reader authority path.
    // This also permits immutable Stage-only rows without trusting saved Query
    // runtime data as authority.
    let proven = observation_binding(db, current, selected)?;
    if proven.namespace.as_str() != current.namespace.as_str()
        || proven.stream_epoch != current.stream_epoch
    {
        return Err(StorageError::ScopeChanged);
    }

    let mut before = selected.clone();
    before.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let before_metadata = serde_json::to_string(&before).map_err(|_| StorageError::Unavailable)?;
    let before_digest = proof_codec::digest(before_metadata.as_bytes());
    let admission = i64::try_from(current.admission).map_err(|_| StorageError::ScopeChanged)?;
    let owner_slot: Option<(String, i64, String)> = db
        .query_row(
            "SELECT metadata_digest,admission,stream FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3",
            params![namespace, selected.key, owner_kind],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(failed)?;
    if owner_slot
        .as_ref()
        .is_some_and(|(digest, slot_admission, stream)| {
            digest != &before_digest
                || *slot_admission != admission
                || stream != &current.stream_epoch
        })
    {
        return Err(StorageError::ScopeChanged);
    }

    if selected.query.as_ref() == Some(query) {
        return Ok(());
    }
    let mut repaired = before;
    repaired.query = Some(query.clone());
    let repaired_metadata =
        serde_json::to_string(&repaired).map_err(|_| StorageError::Unavailable)?;
    if repaired_metadata.len() > 1_048_576 {
        return Err(StorageError::HistoryRetentionBounds);
    }
    let repaired_digest = proof_codec::digest(repaired_metadata.as_bytes());

    let changed = db
        .execute(
            &format!("UPDATE main.{table} SET metadata=json_set(metadata,'$.query',json(?3)) WHERE namespace=?1 AND session_key=?2 AND metadata=?4"),
            params![namespace, selected.key, query.to_string(), stored],
        )
        .map_err(failed)?;
    if changed != 1 {
        return Err(StorageError::ScopeChanged);
    }
    if owner_slot.is_some() {
        let changed = db
            .execute(
                "UPDATE storage_catalog_native_observation_grants SET metadata_digest=?6 WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3 AND metadata_digest=?4 AND admission=?5 AND stream=?7",
                params![
                    namespace,
                    selected.key,
                    owner_kind,
                    before_digest,
                    admission,
                    repaired_digest,
                    current.stream_epoch
                ],
            )
            .map_err(failed)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
    } else {
        db.execute(
            "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                namespace,
                selected.key,
                owner_kind,
                repaired_digest,
                admission,
                current.stream_epoch
            ],
        )
        .map_err(failed)?;
    }
    Ok(())
}

/// Reconcile both owner slots from the post-publication Catalog rows, keeping
/// only exact producer evidence already attached to those rows. A changed or
/// copied row cannot create local authority.
pub(crate) fn reconcile_native_observation_owners(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    admission: u64,
    stream: &str,
) -> Result<(), StorageError> {
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    let catalog: Option<Option<String>> = db
        .query_row(
            "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
            params![namespace, session_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let history: Option<Option<String>> = db
        .query_row(
            "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
            params![namespace, session_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let normalize = |text: String| -> Result<String, StorageError> {
        let mut row: CatalogRow =
            serde_json::from_str(&text).map_err(|_| StorageError::Unavailable)?;
        if row.key != session_key {
            return Err(StorageError::Unavailable);
        }
        row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
        let metadata = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
        Ok(proof_codec::digest(metadata.as_bytes()))
    };
    let catalog_digest = match catalog {
        None => None,
        Some(None) => return Err(StorageError::HistoryRetentionBounds),
        Some(Some(text)) => Some(normalize(text)?),
    };
    let history_digest = match history {
        None => None,
        Some(None) => return Err(StorageError::HistoryRetentionBounds),
        Some(Some(text)) => Some(normalize(text)?),
    };
    let mut statement = db
        .prepare(
            "SELECT owner_kind,metadata_digest,admission,stream FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 LIMIT 3",
        )
        .map_err(failed)?;
    let grants = statement
        .query_map(params![namespace, session_key], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(failed)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(failed)?;
    if grants.len() > 2 {
        return Err(StorageError::Unavailable);
    }
    let owned = |kind: &str, digest: Option<&String>| {
        digest.is_some_and(|digest| {
            grants
                .iter()
                .any(|(owner, actual, actual_admission, actual_stream)| {
                    owner == kind
                        && actual == digest
                        && *actual_admission == admission
                        && actual_stream == stream
                })
        })
    };
    if !owned("catalog", catalog_digest.as_ref()) {
        db.execute(
            "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
            params![namespace, session_key],
        )
        .map_err(failed)?;
    }
    let history_owned = owned("history", history_digest.as_ref());
    let history_from_catalog = owned("catalog", history_digest.as_ref());
    if !history_owned && history_from_catalog {
        let digest = history_digest.as_ref().ok_or(StorageError::Unavailable)?;
        db.execute(
            "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream) VALUES(?1,?2,'history',?3,?4,?5) ON CONFLICT(namespace,session_key,owner_kind) DO UPDATE SET metadata_digest=excluded.metadata_digest,admission=excluded.admission,stream=excluded.stream",
            params![namespace, session_key, digest, admission, stream],
        )
        .map_err(failed)?;
    } else if !history_owned {
        db.execute(
            "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='history'",
            params![namespace, session_key],
        )
        .map_err(failed)?;
    }
    Ok(())
}

fn validate_publication_owner(
    db: &Connection,
    owner: &crate::events::store::configured::NativeOwnership,
    publication: &str,
    producer_admission: u64,
) -> Result<(), StorageError> {
    if producer_admission == owner.admission() {
        return Ok(());
    }
    let stream = owner
        .config()
        .expected_stream_epoch
        .as_deref()
        .ok_or(StorageError::ScopeChanged)?;
    let binding = format!("{}:{stream}", owner.admission());
    let admitted: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM main.meta p JOIN main.meta a ON a.namespace=p.namespace AND a.key='native_transfer_v1:catalog-owner:'||p.value WHERE p.namespace=?1 AND p.key=?2 AND a.value=?3)", params![owner.config().namespace.as_str(), format!("native_transfer_v1:publication:{publication}"), binding], |r| r.get(0)).map_err(failed)?;
    if admitted {
        Ok(())
    } else {
        Err(StorageError::ScopeChanged)
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

fn joined_digest(base: &str, manifest: &str) -> Result<String, StorageError> {
    proof_codec::presentation_digest(base, manifest)
}

/// The caller has completed every producer write in the same publication TX.
/// Capture typed metadata, never Raw bodies, before committing the proof root.
pub(super) fn capture_presentations(
    tx: &Transaction<'_>,
    ns: &str,
    generation: &str,
    prepared: &crate::model::PreparedSources,
    observed_at: i64,
) -> Result<Vec<(String, String)>, StorageError> {
    use crate::storage::selected::{CapturedContext, PresentationWitness};
    let lookup = super::selected::Context::owned(tx);
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for row in &prepared.rows {
        let witness = if let Some(projection) =
            prepared.transcripts.iter().find(|p| p.key == row.key)
        {
            let texts = projection
                .slots
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| StorageError::Unavailable)?;
            let header_generation = crate::slot_projection::generation(&texts, &row.sources)
                .map_err(|_| StorageError::Unavailable)?;
            let header: (String,u32,i64) = tx.query_row("SELECT generation,version,total FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2",params![ns,row.key], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
            if header.0 != header_generation
                || header.1 != crate::slot_projection::VERSION
                || usize::try_from(header.2).ok() != Some(texts.len())
            {
                return Err(StorageError::ScopeChanged);
            }
            let has_history = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2)",params![ns,row.key],|r|r.get(0)).map_err(failed)?;
            let catalog = [
                (
                    "catalog_version".into(),
                    crate::model::summary::CATALOG_VERSION.to_string(),
                ),
                ("catalog_generation".into(), generation.to_owned()),
                (
                    "slot_projection_catalog_generation".into(),
                    generation.to_owned(),
                ),
                ("catalog_completeness".into(), "partial".into()),
                ("catalog_observed_at".into(), observed_at.to_string()),
            ]
            .into_iter()
            .collect();
            let mut observations = prepared
                .presentation_observations()
                .cloned()
                .ok_or(StorageError::Unavailable)?;
            let context =
                CapturedContext::capture(&lookup, row, &projection.slots, &mut |relative| {
                    observations.capture_relative_status(relative, || {
                        super::catalog::runtime_source_cwds(tx, &relative.sources)
                    })
                })
                .map_err(|_| StorageError::Unavailable)?;
            Some(PresentationWitness {
                header_generation,
                version: header.1,
                total: texts.len(),
                has_history,
                catalog,
                context,
                relative_statuses: std::mem::take(&mut observations.relative_statuses),
                observations,
            })
        } else {
            None
        };
        let text = serde_json::to_string(&witness).map_err(|_| StorageError::Unavailable)?;
        bytes = bytes.saturating_add(text.len());
        if bytes > 8 * 1048576 {
            return Err(StorageError::HistoryRetentionBounds);
        }
        result.push((row.key.clone(), text));
    }
    Ok(result)
}

pub(super) fn bind_presentations(
    tx: &Transaction<'_>,
    ns: &str,
    publication: &str,
    base: &str,
    rows: &[(String, String)],
    prepared: &crate::model::PreparedSources,
) -> Result<String, StorageError> {
    let mut entries = Vec::new();
    for (key, text) in rows {
        let row = prepared
            .rows
            .iter()
            .find(|row| &row.key == key)
            .ok_or(StorageError::Unavailable)?;
        let metadata = serde_json::to_string(row).map_err(|_| StorageError::Unavailable)?;
        let projection = prepared
            .transcripts
            .iter()
            .find(|projection| &projection.key == key);
        let slots = serde_json::to_string(&projection.map(|projection| &projection.slots))
            .map_err(|_| StorageError::Unavailable)?;
        entries.push((
            key.clone(),
            proof_codec::presentation_entry(&metadata, &slots, text)?,
        ));
    }
    let manifest = serde_json::to_string(&PresentationManifest {
        entries,
        observations: prepared.presentation_observations().cloned(),
    })
    .map_err(|_| StorageError::Unavailable)?;
    if manifest.len() > 1048576 {
        return Err(StorageError::HistoryRetentionBounds);
    }
    let joined = joined_digest(base, &manifest)?;
    tx.execute(
        "INSERT INTO storage_proof_presentation_roots VALUES(?1,?2,?3,?4,?5)",
        params![ns, publication, base, manifest, joined],
    )
    .map_err(failed)?;
    Ok(joined)
}
pub(super) fn replayed_presentation(
    db: &Connection,
    ns: &str,
    publication: &str,
    base: &str,
    expected: &str,
) -> Result<bool, StorageError> {
    let row: Option<(String,String,String)> = db.query_row("SELECT base,manifest,digest FROM storage_proof_presentation_roots WHERE namespace=?1 AND publication=?2 AND digest=?3",params![ns,publication,expected],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(failed)?;
    let Some((saved, manifest, joined)) = row else {
        return Ok(false);
    };
    Ok(saved == base
        && manifest.len() <= 1048576
        && joined == expected
        && joined_digest(&saved, &manifest)? == joined)
}
fn validate_presentation(
    db: &Connection,
    snapshot: &ReaderSnapshot,
    key: &str,
    proof: &str,
) -> Result<String, StorageError> {
    let (base,manifest,joined): (String,String,String) = db.query_row("SELECT base,manifest,digest FROM storage_proof_presentation_roots WHERE namespace=?1 AND publication=?2 AND digest=?3",params![snapshot.namespace.as_str(),snapshot.publication_generation,proof],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    if manifest.len() > 1048576 || joined != proof {
        return Err(StorageError::ScopeChanged);
    }
    proof_codec::verify_presentation_digest(&base, &manifest, &joined)?;
    let (text,metadata,slots): (String,String,String) = db.query_row("SELECT CASE WHEN octet_length(p.metadata)<=8388608 THEN p.metadata END,CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END FROM storage_proof_presentations p JOIN storage_proof_generations g ON g.namespace=p.namespace AND g.id=p.id AND g.session_key=p.session_key WHERE p.namespace=?1 AND p.id=?2 AND p.session_key=?3",params![snapshot.namespace.as_str(),snapshot.proof_generation.0,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    proof_codec::verify_presentation_entry(key, &metadata, &slots, &text, &manifest)?;
    Ok(text)
}

impl crate::storage::selected::SelectedSnapshotStore for Lease {
    fn open_selected(
        &self,
        options: &crate::Options,
        host: &crate::storage::selected::HostSourceBinding,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<crate::storage::selected::SelectedRead> {
        use std::io;
        control.check()?;
        host.validate(&self.snapshot)?;
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(io::Error::other)?;
        db.execute_batch("BEGIN DEFERRED")
            .map_err(io::Error::other)?;
        self.validate(&db).map_err(io::Error::other)?;
        // validate() authenticated both blobs in this same read transaction.
        let (text,manifest): (String,String) = db.query_row("SELECT p.metadata,r.manifest FROM storage_proof_presentations p JOIN storage_proof_generations g ON g.namespace=p.namespace AND g.id=p.id AND g.session_key=p.session_key JOIN storage_proof_presentation_roots r ON r.namespace=g.namespace AND r.publication=g.publication AND r.digest=g.proof WHERE p.namespace=?1 AND p.id=?2 AND p.session_key=?3",params![self.snapshot.namespace.as_str(),self.snapshot.proof_generation.0,self.row.key],|r|Ok((r.get(0)?,r.get(1)?))).map_err(io::Error::other)?;
        let witness: Option<crate::storage::selected::PresentationWitness> =
            serde_json::from_str(&text).map_err(io::Error::other)?;
        let mut witness =
            witness.ok_or_else(|| io::Error::other("selected presentation unavailable"))?;
        let manifest: PresentationManifest =
            serde_json::from_str(&manifest).map_err(io::Error::other)?;
        witness.observations = manifest
            .observations
            .ok_or_else(|| io::Error::other("selected observation witness unavailable"))?;
        witness.observations.relative_statuses = std::mem::take(&mut witness.relative_statuses);
        let text: String = db.query_row("SELECT CASE WHEN octet_length(slots)<=1048576 THEN slots END FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3",params![self.snapshot.namespace.as_str(),self.snapshot.proof_generation.0,self.row.key],|r|r.get(0)).map_err(io::Error::other)?;
        let slots: Option<Vec<crate::slot_projection::SavedSlot>> =
            serde_json::from_str(&text).map_err(io::Error::other)?;
        let slots = slots.ok_or_else(|| io::Error::other("selected recipes unavailable"))?;
        let texts = slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::Error::other)?;
        if texts.iter().any(|text| text.len() > 65536)
            || slots.len() != witness.total
            || witness.version != crate::slot_projection::VERSION
            || crate::slot_projection::generation(&texts, &self.row.sources)
                .map_err(io::Error::other)?
                != witness.header_generation
            || witness.catalog.get("catalog_generation")
                != Some(&self.snapshot.publication_generation)
        {
            return Err(io::Error::other("selected witness changed"));
        }
        let sources = crate::query_source::Sources::capture_bound_snapshot(
            options,
            &self.row.sources,
            db,
            &self.owner,
        )?;
        control.check()?;
        Ok(crate::storage::selected::SelectedRead {
            snapshot: self.snapshot.clone(),
            row: self.row.clone(),
            witness,
            slots,
            sources,
        })
    }
    fn finish_selected(&self, control: &crate::SelectedReadControl) -> std::io::Result<()> {
        control.check()?;
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        control.check()
    }
}

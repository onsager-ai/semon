//! Configured native transaction with a composite core Event family. The global/multi-source and
//! Catalog publisher gates remain closed. Parser/identity logic stays in events.
use super::*;
use crate::storage::sqlite::source_journal::SourceJournal;
use crate::storage::{StorageError, StoreConfig, native_events::EventReceipt};
use rusqlite::functions::FunctionFlags;

const CORE_TABLES: &[&str] = &["files", "events", "signals"];

// Authored separately from the transitional Options schema. There are no
// namespace defaults, views, SQL rewriting, or implicit namespace row adoption.
pub(super) const CORE_SCHEMA: &str = "
CREATE TABLE files (
 namespace TEXT NOT NULL, file_id INTEGER NOT NULL, path TEXT NOT NULL,
 dev INTEGER NOT NULL, ino INTEGER NOT NULL, size INTEGER NOT NULL,
 mtime_ns BLOB NOT NULL, ctime_ns BLOB, resume_at INTEGER NOT NULL,
 head_sha256 BLOB NOT NULL, tail_sha256 BLOB NOT NULL,
 entrypoint TEXT, title TEXT, agent_name TEXT, last_model TEXT,
 failed INTEGER NOT NULL, first_ms INTEGER, last_ms INTEGER, cwd TEXT, branch TEXT,
 pending TEXT NOT NULL, tool_ids TEXT NOT NULL, yields TEXT NOT NULL,
 busy TEXT NOT NULL, links TEXT NOT NULL, codex_tokens TEXT NOT NULL,
 codex_tokens_by_model TEXT NOT NULL, rate_limits TEXT, codex_native_usage TEXT,
 codex_pricing_fingerprint TEXT, copilot TEXT,
 PRIMARY KEY(namespace,file_id), UNIQUE(namespace,path)
) STRICT, WITHOUT ROWID;
CREATE INDEX files_cwd ON files(namespace,cwd,path);
CREATE TABLE events (
 namespace TEXT NOT NULL, file_id INTEGER NOT NULL, extra INTEGER NOT NULL,
 seq INTEGER NOT NULL, k TEXT NOT NULL, o INTEGER NOT NULL, b INTEGER NOT NULL,
 t INTEGER, id TEXT, n TEXT, code_mode INTEGER NOT NULL,
 code_mode_open INTEGER NOT NULL, parent INTEGER, script INTEGER NOT NULL,
 y TEXT, poll INTEGER, reply INTEGER NOT NULL, reply_o INTEGER, reply_b INTEGER,
 reply_t INTEGER, reply_e INTEGER, reply_f INTEGER, reply_m TEXT, item INTEGER,
 peer INTEGER NOT NULL, peer_sock TEXT, peer_pid INTEGER, peer_start TEXT, u TEXT,
 PRIMARY KEY(namespace,file_id,extra,seq),
 FOREIGN KEY(namespace,file_id) REFERENCES files(namespace,file_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE signals (
 namespace TEXT NOT NULL, file_id INTEGER NOT NULL, seq INTEGER NOT NULL,
 k TEXT NOT NULL, o INTEGER NOT NULL, t INTEGER, at INTEGER NOT NULL, n TEXT, v INTEGER,
 PRIMARY KEY(namespace,file_id,seq),
 FOREIGN KEY(namespace,file_id) REFERENCES files(namespace,file_id)
) STRICT, WITHOUT ROWID;
";

pub(super) const PUT_FILE: &str = "INSERT INTO files
 (namespace,file_id,path,dev,ino,size,mtime_ns,resume_at,head_sha256,tail_sha256,
 entrypoint,title,agent_name,last_model,failed,first_ms,last_ms,cwd,branch,pending,
 tool_ids,yields,busy,links,codex_tokens,codex_tokens_by_model,rate_limits,
 codex_native_usage,copilot,ctime_ns)
 VALUES(semon_namespace(),coalesce(
 (SELECT file_id FROM files WHERE namespace=semon_namespace() AND path=?1),
 (SELECT coalesce(max(file_id),0)+1 FROM files WHERE namespace=semon_namespace())),
 ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,
 ?21,?22,?23,?24,?25,?26,?27,?28)
 ON CONFLICT(namespace,path) DO UPDATE SET dev=excluded.dev,ino=excluded.ino,
 size=excluded.size,mtime_ns=excluded.mtime_ns,resume_at=excluded.resume_at,
 head_sha256=excluded.head_sha256,tail_sha256=excluded.tail_sha256,
 entrypoint=excluded.entrypoint,title=excluded.title,agent_name=excluded.agent_name,
 last_model=excluded.last_model,failed=excluded.failed,first_ms=excluded.first_ms,
 last_ms=excluded.last_ms,cwd=excluded.cwd,branch=excluded.branch,pending=excluded.pending,
 tool_ids=excluded.tool_ids,yields=excluded.yields,busy=excluded.busy,links=excluded.links,
 codex_tokens=excluded.codex_tokens,codex_tokens_by_model=excluded.codex_tokens_by_model,
 rate_limits=excluded.rate_limits,codex_native_usage=excluded.codex_native_usage,
 copilot=excluded.copilot,ctime_ns=excluded.ctime_ns RETURNING file_id";

pub(super) const PUT_EVENT: &str = "INSERT OR REPLACE INTO events
 (namespace,file_id,extra,seq,k,o,b,t,id,n,code_mode,code_mode_open,parent,script,y,poll,
 reply,reply_o,reply_b,reply_t,reply_e,reply_f,reply_m,item,peer,peer_sock,peer_pid,peer_start,u)
 VALUES(semon_namespace(),?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,
 ?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28)";

pub(super) const PUT_SIGNAL: &str = "INSERT OR REPLACE INTO signals
 (namespace,file_id,seq,k,o,t,at,n,v) VALUES(semon_namespace(),?1,?2,?3,?4,?5,?6,?7,?8)";

const PRICING_FILE_SCHEMA: &str = "
CREATE INDEX files_codex_pricing_fingerprint ON files(namespace,file_id,codex_pricing_fingerprint)
 WHERE codex_native_usage IS NOT NULL;
CREATE TRIGGER pricing_pending_file_insert AFTER INSERT ON files
 WHEN NEW.codex_native_usage IS NOT NULL BEGIN
 UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END
 WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
 INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
 SELECT 'source',NEW.path FROM files f,meta m WHERE f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.key='pricing_recompute_target' AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER pricing_pending_file_update AFTER UPDATE OF codex_native_usage ON files BEGIN
 UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END
 WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
 INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
 SELECT 'source',NEW.path FROM files f,meta m WHERE NEW.codex_native_usage IS NOT NULL AND f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.key='pricing_recompute_target' AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER pricing_pending_native_usage_ready AFTER UPDATE OF usage_ready ON native_usage_sources WHEN NEW.usage_ready=1 BEGIN
 INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
 SELECT 'source',NEW.source_path FROM files f,meta m WHERE f.namespace=semon_namespace() AND f.path=NEW.source_path AND m.key='pricing_recompute_target' AND f.codex_native_usage IS NOT NULL AND f.codex_pricing_fingerprint IS NOT m.value;
END;
";

// Same core family with composite meta. Pricing's target is a store-wide
// interpretation setting, unlike per-source progress/completion cursors.
const PRICING_FILE_SCHEMA_META: &str = "
CREATE INDEX files_codex_pricing_fingerprint ON files(namespace,file_id,codex_pricing_fingerprint)
 WHERE codex_native_usage IS NOT NULL;
CREATE TRIGGER pricing_pending_file_insert AFTER INSERT ON files
 WHEN NEW.codex_native_usage IS NOT NULL BEGIN
 UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END
 WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
 INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
 SELECT 'source',NEW.path FROM files f,meta m WHERE f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.namespace='' AND m.key='pricing_recompute_target' AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER pricing_pending_file_update AFTER UPDATE OF codex_native_usage ON files BEGIN
 UPDATE files SET codex_pricing_fingerprint=CASE WHEN json_valid(NEW.codex_native_usage) THEN json_extract(NEW.codex_native_usage,'$.cost.pricing_fingerprint') END
 WHERE namespace=NEW.namespace AND file_id=NEW.file_id;
 INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
 SELECT 'source',NEW.path FROM files f,meta m WHERE NEW.codex_native_usage IS NOT NULL AND f.namespace=NEW.namespace AND f.file_id=NEW.file_id AND m.namespace='' AND m.key='pricing_recompute_target' AND f.codex_pricing_fingerprint IS NOT m.value;
END;
CREATE TRIGGER pricing_pending_native_usage_ready AFTER UPDATE OF usage_ready ON native_usage_sources WHEN NEW.usage_ready=1 BEGIN
 INSERT OR IGNORE INTO pricing_recompute_pending(owner_kind,owner_key)
 SELECT 'source',NEW.source_path FROM files f,meta m WHERE f.namespace=semon_namespace() AND f.path=NEW.source_path AND m.namespace='' AND m.key='pricing_recompute_target' AND f.codex_native_usage IS NOT NULL AND f.codex_pricing_fingerprint IS NOT m.value;
END;
";

pub(super) fn pricing_file_schema(meta_composite: bool) -> &'static str {
    if meta_composite {
        PRICING_FILE_SCHEMA_META
    } else {
        PRICING_FILE_SCHEMA
    }
}

#[derive(Default)]
struct WriterShapeMeasurements {
    core_calls: usize,
    meta_calls: usize,
    core_us: u128,
    meta_us: u128,
}

thread_local! {
    static WRITER_SHAPE_MEASUREMENTS: std::cell::RefCell<Option<WriterShapeMeasurements>> = const { std::cell::RefCell::new(None) };
}

// Observation only. In particular, this guard caches no shape result: every
// existing selector still executes its complete qualification query.
pub(super) struct WriterShapeProbe(Option<WriterShapeMeasurements>);

impl WriterShapeProbe {
    pub(super) fn new(enabled: bool) -> Self {
        Self(
            WRITER_SHAPE_MEASUREMENTS
                .with(|cell| cell.replace(enabled.then(WriterShapeMeasurements::default))),
        )
    }

    fn start(label: &'static str) -> Option<std::time::Instant> {
        WRITER_SHAPE_MEASUREMENTS.with(|cell| {
            let mut cell = cell.borrow_mut();
            let measurements = cell.as_mut()?;
            match label {
                "core_shape_calls" => measurements.core_calls += 1,
                "meta_shape_calls" => measurements.meta_calls += 1,
                _ => unreachable!("fixed writer shape label"),
            }
            Some(std::time::Instant::now())
        })
    }

    fn finish(label: &'static str, started: Option<std::time::Instant>) {
        WRITER_SHAPE_MEASUREMENTS.with(|cell| {
            if let Some(measurements) = cell.borrow_mut().as_mut() {
                let us = started.map_or(0, |started| started.elapsed().as_micros());
                match label {
                    "core_shape_query" => measurements.core_us += us,
                    "meta_shape_query" => measurements.meta_us += us,
                    _ => unreachable!("fixed writer shape label"),
                }
            }
        });
    }
}

impl Drop for WriterShapeProbe {
    fn drop(&mut self) {
        if let Some(measurements) =
            WRITER_SHAPE_MEASUREMENTS.with(|cell| cell.replace(self.0.take()))
        {
            // Keep this diagnostic separate from cleanup's test recorder.
            eprintln!(
                "semon sessions native: writer_shape core_shape_calls={} core_shape_query_us={} meta_shape_calls={} meta_shape_query_us={}",
                measurements.core_calls,
                measurements.core_us,
                measurements.meta_calls,
                measurements.meta_us
            );
        }
    }
}

/// Select an authored statement for the physical family shape, never rewrite
/// SQL or retry errors against another schema. Singleton stores still support
/// read-only admission/assignment; configured writes require validate_core.
pub(super) fn sql<'a>(
    db: &Connection,
    legacy: &'a str,
    scoped: &'a str,
) -> rusqlite::Result<&'a str> {
    let started = WriterShapeProbe::start("core_shape_calls");
    let composite = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_table_info('files') WHERE name='namespace' AND pk=1)")?.query_row([], |r| r.get::<_, bool>(0));
    WriterShapeProbe::finish("core_shape_query", started);
    let composite = composite?;
    Ok(if composite { scoped } else { legacy })
}

pub(super) fn predicate(db: &Connection) -> rusqlite::Result<&'static str> {
    sql(db, "", "namespace=semon_namespace() AND ")
}

/// Meta is qualified separately from the core rows. Until all store-global
/// owners are ported, a composite files table must not imply composite meta.
/// Select authored statements only; never retry or rewrite SQL on failure.
pub(crate) fn metadata_sql<'a>(
    db: &Connection,
    legacy: &'a str,
    scoped: &'a str,
) -> rusqlite::Result<&'a str> {
    let started = WriterShapeProbe::start("meta_shape_calls");
    crate::native_timing::discovery_count(|counts| {
        counts.schema_probes += 1;
        counts.statements += 1;
    });
    let shape = db.prepare_cached(
        "SELECT count(*),coalesce(max(CASE WHEN name='namespace' AND type='TEXT' AND \"notnull\"=1 THEN pk END),0),coalesce(max(CASE WHEN name='key' AND type='TEXT' THEN pk END),0),coalesce(sum(name='value' AND type='TEXT' AND \"notnull\"=1 AND pk=0),0) FROM pragma_table_info('meta','main')"
    )?.query_row([], |r| Ok((r.get::<_, i64>(0)?,r.get::<_, i64>(1)?,r.get::<_, i64>(2)?,r.get::<_, i64>(3)?)));
    WriterShapeProbe::finish("meta_shape_query", started);
    let (columns, namespace, key, values) = shape?;
    let composite = match (columns, namespace, key, values) {
        (2, 0, 1, 1) => false,
        (3, 1, 2, 1) => true,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(if composite { scoped } else { legacy })
}

// Empty is reserved for store metadata: LogicalSourceNamespace refuses it.
// New globals must be classified deliberately before composite meta is exposed.
const STORE_METADATA_NAMESPACE: &str = "";
const SOURCE_METADATA_PREFIXES: &[&str] = &[
    "archive_event_index:",
    "native_catalog_relocation:",
    "native_claude_sidecar:",
    "native_codex_header:",
    "native_launch_calls:",
    "native_parent_marker:",
    "native_proof_tree:",
    "native_published_chunks:",
    "native_published_chunks_manifest:",
    "native_source_chunks:",
    "native_tree_failure:",
    "native_tree_summary:",
    "slot_upgrade_selected:",
    "history_transfer:",
    "history_original_header:",
    "overview_needed:",
];
const STORE_METADATA_KEYS: &[&str] = &[
    "cache_version",
    "catalog_version",
    "search_version",
    "catalog_changes_epoch",
    "native_tree_repair_clock",
    "pricing_recompute_target",
];
// These describe completed/ongoing work for one logical source, rather than
// the shared SQL schema or physical-store incarnation.
const SOURCE_METADATA_KEYS: &[&str] = &[
    "catalog_semantic_recipe",
    "catalog_generation",
    "catalog_source_revision",
    "catalog_backfill_state",
    "catalog_semantic_gc",
    "catalog_push_driven",
    "catalog_completeness",
    "semantic_listing_generation",
    "semantic_epoch",
    "pricing_recompute_v2",
    "pricing_recompute_completed",
    "overview_generation",
    "catalog_changes_floor_current",
    "catalog_changes_floor_retained_history",
    "catalog_observed_at",
    "catalog_recovery_after",
    "catalog_recovery_reason",
    "catalog_publication_epoch",
    "slot_projection_catalog_generation",
    "search_backfill_complete",
    "search_generation_current",
    "search_generation_retained_history",
    "search_incomplete_current",
    "search_incomplete_retained_history",
    "claude_json_stamp",
];

fn metadata_namespace<'a>(namespace: &'a str, key: &str) -> Result<&'a str, StorageError> {
    if STORE_METADATA_KEYS.contains(&key) {
        Ok(STORE_METADATA_NAMESPACE)
    } else if !namespace.is_empty()
        && (SOURCE_METADATA_KEYS.contains(&key)
            || SOURCE_METADATA_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix) && key.len() > prefix.len()))
    {
        Ok(namespace)
    } else {
        Err(StorageError::InvalidArguments)
    }
}

pub(crate) fn bind_metadata_scope(db: &Connection, namespace: &str) -> rusqlite::Result<()> {
    crate::storage::LogicalSourceNamespace::new(namespace)
        .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))?;
    let namespace = namespace.to_owned();
    db.create_scalar_function(
        "semon_meta_scope",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
        move |context| {
            let key = context.get::<String>(0)?;
            metadata_namespace(&namespace, &key)
                .map(str::to_owned)
                .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
        },
    )
}

/// Create a new physical store only. Existing files, even empty ones, refuse.
/// Enrollment and bounded Catalog assignment remain separate explicit steps.
pub(crate) fn create(config: &StoreConfig) -> Result<(), StorageError> {
    create_with_layout(config, PhysicalLayout::CONFIGURED)
}

fn create_with_layout(config: &StoreConfig, layout: PhysicalLayout) -> Result<(), StorageError> {
    let ddl = authored_storage_ddl(layout).map_err(|_| StorageError::InvalidArguments)?;
    let epoch = config
        .expected_stream_epoch
        .as_deref()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(StorageError::InvalidArguments)?;
    let crate::storage::StoreBackend::Sqlite { database } = &config.backend;
    if database.as_os_str().is_empty() || database == Path::new(":memory:") {
        return Err(StorageError::InvalidArguments);
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(database)
        .map_err(|_| StorageError::Unavailable)?;
    let mut db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(failed)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(failed)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    for chunk in ddl {
        tx.execute_batch(chunk).map_err(failed)?;
    }
    tx.execute(
        metadata_sql(
            &tx,
            "INSERT INTO meta(key,value) VALUES('cache_version',?1)",
            "INSERT INTO meta(namespace,key,value) VALUES('','cache_version',?1)",
        )
        .map_err(failed)?,
        [CACHE_VERSION.to_string()],
    )
    .map_err(failed)?;
    tx.execute(
        metadata_sql(
            &tx,
            "UPDATE meta SET value=?1 WHERE key='catalog_changes_epoch'",
            "UPDATE meta SET value=?1 WHERE namespace='' AND key='catalog_changes_epoch'",
        )
        .map_err(failed)?,
        [epoch],
    )
    .map_err(failed)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(failed)?;
    tx.commit().map_err(|_| StorageError::CommitUncertain)?;
    let mode: String = db
        .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
        .map_err(failed)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StorageError::Unavailable);
    }
    file.sync_all().map_err(|_| StorageError::CommitUncertain)?;
    Ok(())
}

// The connected transaction still owns one admitted namespace. Only CORE_TABLES
// have composite row keys; the other 22 families remain singleton and fenced.
// A public second namespace must still be refused before any operation.
const TABLES: &[&str] = &[
    "files",
    "events",
    "signals",
    "usage_identity",
    "usage",
    "codex_usage",
    "meta",
    "catalog_relocation_candidates",
    "catalog_native_dependencies",
    "session_source_mutations",
    "native_usage_sources",
    "native_usage_current",
    "native_usage_facts",
    "native_usage_identities",
    "native_usage_dimensions",
    "native_tree_repairs",
    "native_tree_generations",
    "native_tree_usage_work",
    "native_tree_codex_work",
    "native_tree_headers",
    "native_tree_models",
    "native_tree_labels",
    "native_tree_tools",
    "native_tree_usage_models",
    "native_tree_copied_identities",
];

type CommitFile<'a> = (
    &'a str,
    Option<&'a Ledger>,
    Option<&'a super::super::DirtyRows>,
    &'a Ledger,
    &'a FileIndex,
);

type CompletionRow = (String, String, Option<i64>, Option<i64>, String);

#[cfg(test)]
thread_local! {
    static FAIL_EFFECT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn effect(name: &'static str) -> rusqlite::Result<()> {
    FAIL_EFFECT.with(|failure| {
        if failure.get() == Some(name) {
            failure.set(None);
            Err(rusqlite::Error::InvalidQuery)
        } else {
            Ok(())
        }
    })
}

fn failed(error: rusqlite::Error) -> StorageError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => StorageError::Busy,
        _ => StorageError::Unavailable,
    }
}

pub(super) fn connection(config: &StoreConfig) -> Result<Connection, StorageError> {
    let db = SourceJournal(config.clone()).connection()?;
    db.pragma_update(None, "foreign_keys", true)
        .map_err(failed)?;
    db.create_scalar_function(
        "semon_event_writer",
        0,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(failed)?;
    copied_usage_sql(&db, "", "").map_err(failed)?;
    semantic_stage::support_sql(&db, "", "").map_err(failed)?;
    Ok(db)
}

pub(crate) fn validate_semantic_schema(config: &StoreConfig) -> Result<(), StorageError> {
    let db = connection(config)?;
    SourceJournal(config.clone()).validate(&db)
}

pub(crate) fn semantic_tables() -> &'static [&'static str] {
    semantic_stage::STAGE_TABLES
}
pub(crate) fn semantic_recipe() -> &'static str {
    semantic_stage::RECIPE
}
pub(crate) fn primitive_native_key(native: &str) -> &str {
    semantic_stage::primitive_native_key(native)
}
pub(crate) fn validate_staging_owner(
    db: &Connection,
    config: &StoreConfig,
) -> Result<(), StorageError> {
    validate(db, config)
}

/// Private primitive writer authority. Database flags cannot manufacture it;
/// public configuration always uses the sole-Source admission path.
#[derive(Clone, Debug)]
pub(crate) struct NativeOwnership {
    config: StoreConfig,
    admission: i64,
}

impl NativeOwnership {
    pub(crate) fn config(&self) -> &StoreConfig {
        &self.config
    }
    pub(crate) fn admission(&self) -> u64 {
        self.admission as u64
    }

    fn validate_physical(db: &Connection) -> Result<(), StorageError> {
        validate_core(db)?;
        if metadata_sql(db, "singleton", "composite").map_err(failed)? != "composite"
            || semantic_stage::support_sql(db, "singleton", "composite").map_err(failed)?
                != "composite"
            || copied_usage_sql(db, "singleton", "composite").map_err(failed)? != "composite"
            || native_usage::sql(db, "singleton", "composite").map_err(failed)? != "composite"
            || tree_state::sql(db, "singleton", "composite").map_err(failed)? != "composite"
        {
            return Err(StorageError::NamespaceUpgradePending);
        }
        Ok(())
    }

    pub(crate) fn validate_ready(&self, db: &Connection) -> Result<(), StorageError> {
        self.validate(db)?;
        let bound: bool=db.query_row("SELECT count(*)=9 AND sum(name='namespace' AND pk=1 AND [notnull]=1)=1 AND sum(name='operation_id' AND pk=2 AND [notnull]=1)=1 AND sum(name='request_digest' AND type='TEXT' AND [notnull]=1)=1 FROM pragma_table_info('storage_event_operations','main')",[],|r|r.get(0)).map_err(failed)?;
        if !bound {
            return Err(StorageError::NamespaceUpgradePending);
        }
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM storage_event_tables WHERE namespace=?1 AND qualification=0",
                [self.config.namespace.as_str()],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if count != (TABLES.len() + 1) as i64 {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for table in TABLES.iter().copied().chain(["pricing_recompute_pending"]) {
            let owned: bool=db.query_row("SELECT EXISTS(SELECT 1 FROM storage_event_tables WHERE namespace=?1 AND table_name=?2 AND qualification=0)",params![self.config.namespace.as_str(),table],|r|r.get(0)).map_err(failed)?;
            if !owned {
                return Err(StorageError::ScopeChanged);
            }
        }
        for table in TABLES.iter().copied().chain([
            "storage_event_tables",
            "storage_event_operations",
            "session_catalog_invalidations",
            "catalog_backfill_sources",
            "pricing_recompute_pending",
        ]) {
            let guards: i64 = db.query_row("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND tbl_name=?1 AND name IN('storage_owned_guard_'||?1||'_INSERT','storage_owned_guard_'||?1||'_UPDATE','storage_owned_guard_'||?1||'_DELETE')", [table], |r| r.get(0)).map_err(failed)?;
            if guards != 3 {
                return Err(StorageError::NamespaceUpgradePending);
            }
        }
        Ok(())
    }

    pub(crate) fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        Self::validate_physical(db)?;
        let admission = crate::storage::sqlite::namespace::require_semantic_admitted(
            db,
            &self.config.namespace,
        )?;
        if admission.generation != self.admission as u64 {
            return Err(StorageError::ScopeChanged);
        }
        let epoch: String = db
            .query_row(
                "SELECT value FROM main.meta WHERE namespace='' AND key='catalog_changes_epoch'",
                [],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if self.config.expected_stream_epoch.as_deref() != Some(epoch.as_str()) {
            return Err(StorageError::ScopeChanged);
        }
        if !current_contract(db).map_err(failed)? {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for table in TABLES
            .iter()
            .copied()
            .chain(semantic_stage::STAGE_TABLES.iter().copied())
            .chain([
                "session_catalog_invalidations",
                "catalog_backfill_sources",
                "pricing_recompute_pending",
            ])
        {
            let scoped: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [table], |r| r.get(0)).map_err(failed)?;
            if !scoped {
                return Err(StorageError::NamespaceUpgradePending);
            }
        }
        Ok(())
    }

    /// Admit only the already-authored, fully composite native profile. No row
    /// conversion, singleton adoption, or public qualification is performed.
    pub(crate) fn admit(config: &StoreConfig) -> Result<Self, StorageError> {
        let mut db = connection(config)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        // One supported current Semon contract, before any enrollment DDL/DML.
        if !current_contract(&tx).map_err(failed)? {
            return Err(StorageError::NamespaceUpgradePending);
        }
        Self::validate_physical(&tx)?;
        for table in TABLES
            .iter()
            .copied()
            .chain(semantic_stage::STAGE_TABLES.iter().copied())
            .chain([
                "session_catalog_invalidations",
                "catalog_backfill_sources",
                "pricing_recompute_pending",
            ])
        {
            let scoped: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [table], |r| r.get(0)).map_err(failed)?;
            if !scoped {
                return Err(StorageError::NamespaceUpgradePending);
            }
        }
        let bound: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_namespaces')",
                [],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if !bound {
            // Fresh profile only: existing native data requires migration.
            let source_meta: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace<>'')",
                    [],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            if source_meta {
                return Err(StorageError::NamespaceUpgradePending);
            }
            for table in TABLES
                .iter()
                .copied()
                .chain(semantic_stage::STAGE_TABLES.iter().copied())
                .chain([
                    "session_catalog_invalidations",
                    "catalog_backfill_sources",
                    "pricing_recompute_pending",
                ])
                .filter(|table| *table != "meta")
            {
                let present: bool = tx
                    .query_row(
                        &format!("SELECT EXISTS(SELECT 1 FROM \"{table}\")"),
                        [],
                        |r| r.get(0),
                    )
                    .map_err(failed)?;
                if present {
                    return Err(StorageError::NamespaceUpgradePending);
                }
            }
            tx.execute_batch(crate::storage::sqlite::namespace::REGISTRY_SCHEMA)
                .map_err(failed)?;
            let tables=tx.prepare("SELECT name FROM main.sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'storage_%' ORDER BY name LIMIT 129").map_err(failed)?.query_map([],|r|r.get::<_,String>(0)).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
            if tables.len() > 128 {
                return Err(StorageError::NamespaceUpgradePending);
            }
            for (index, table) in tables.iter().enumerate() {
                if TABLES.contains(&table.as_str())
                    || semantic_stage::STAGE_TABLES.contains(&table.as_str())
                    || [
                        "session_catalog_invalidations",
                        "catalog_backfill_sources",
                        "pricing_recompute_pending",
                    ]
                    .contains(&table.as_str())
                {
                    continue;
                }
                // Unported Catalog, history, descriptions, trace and pricing
                // consumers are fenced even on this private fresh profile.
                for operation in ["INSERT", "UPDATE", "DELETE"] {
                    let condition = if [
                        "session_catalog",
                        "session_catalog_sources",
                        "session_catalog_changes",
                        "session_history_catalog",
                        "session_history_projections",
                        "session_history_slots",
                        "session_history_slot_parts",
                        "session_slot_projections",
                        "session_slots",
                        "session_search_coverage",
                        "session_search_documents",
                        "session_search_memberships",
                        "session_search_counts",
                    ]
                    .contains(&table.as_str())
                    {
                        let scope = match operation {
                            "UPDATE" => {
                                "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()"
                            }
                            "DELETE" => "OLD.namespace IS NOT semon_namespace()",
                            _ => "NEW.namespace IS NOT semon_namespace()",
                        };
                        format!("WHEN semon_semantic_publisher()<>1 OR {scope}")
                    } else {
                        String::new()
                    };
                    tx.execute_batch(&format!("CREATE TRIGGER storage_private_unported_{index}_{operation} BEFORE {operation} ON \"{}\" {condition} BEGIN SELECT RAISE(ABORT,'private native family unported'); END;",table.replace('"',"\"\""))).map_err(failed)?;
                }
            }
        }
        tx.execute("INSERT INTO storage_namespaces(namespace,admission_generation,admitted_at,format_version,writer_qualification) SELECT ?1,coalesce(max(admission_generation),0)+1,?2,1,0 FROM storage_namespaces WHERE true ON CONFLICT(namespace) DO NOTHING", params![config.namespace.as_str(),crate::model::now_ms()]).map_err(failed)?;
        let admission = tx
            .query_row(
                "SELECT admission_generation FROM storage_namespaces WHERE namespace=?1",
                [config.namespace.as_str()],
                |r| r.get(0),
            )
            .map_err(failed)?;
        let owner = Self {
            config: config.clone(),
            admission,
        };
        owner.validate(&tx)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        prepare_owned(&owner)?;
        Ok(owner)
    }
}

#[allow(dead_code)]
pub(crate) fn create_semantic_store(config: &StoreConfig) -> Result<(), StorageError> {
    create_with_layout(
        config,
        PhysicalLayout {
            core: true,
            metadata: true,
            catalog: true,
            trace: false,
            copied_usage: true,
            native_usage: true,
            tree: true,
            pricing_queue: true,
            semantic_support: true,
            semantic_stage: true,
        },
    )
}

fn prepare_owned(owner: &NativeOwnership) -> Result<(), StorageError> {
    let journal = SourceJournal::owned(owner.clone());
    journal.prepare()?;
    let mut db = connection(owner.config())?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    owner.validate(&tx)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS storage_event_tables(namespace TEXT NOT NULL,table_name TEXT NOT NULL,qualification INTEGER NOT NULL CHECK(qualification IN(0,1)),PRIMARY KEY(namespace,table_name)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_event_operations(namespace TEXT NOT NULL,operation_id TEXT NOT NULL,source_path TEXT NOT NULL,request_digest TEXT NOT NULL CHECK(length(request_digest)=64),base TEXT NOT NULL,observed_revision INTEGER,target TEXT,consumed_bytes INTEGER,state TEXT NOT NULL CHECK(state IN('intent','complete')),PRIMARY KEY(namespace,operation_id)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_event_reconcile ON storage_event_operations(namespace,source_path,base,observed_revision,state);
CREATE INDEX IF NOT EXISTS storage_ns_source_pending ON session_catalog_invalidations(namespace,revision,source_path);").map_err(failed)?;
    for table in TABLES.iter().copied().chain([
        "storage_event_tables",
        "storage_event_operations",
        "session_catalog_invalidations",
        "catalog_backfill_sources",
        "pricing_recompute_pending",
    ]) {
        for (operation, predicate) in [
            ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
            (
                "UPDATE",
                "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
            ),
            ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
        ] {
            let metadata = if table == "meta" {
                let rows: &[&str] = match operation {
                    "UPDATE" => &["OLD", "NEW"],
                    "DELETE" => &["OLD"],
                    _ => &["NEW"],
                };
                rows.iter().map(|row|format!(" OR NOT ({row}.key GLOB 'native_*' OR {row}.key GLOB 'archive_event_index:*' OR (semon_semantic_publisher()=1 AND ({row}.key IN('catalog_generation','catalog_version','catalog_publication_epoch','catalog_observed_at','catalog_completeness','semantic_listing_generation','slot_projection_catalog_generation') OR {row}.key GLOB 'history_transfer:*' OR {row}.key GLOB 'history_original_header:*' OR {row}.key GLOB 'overview_needed:*')))")).collect::<String>()
            } else {
                String::new()
            };
            tx.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS storage_owned_guard_{table}_{operation} BEFORE {operation} ON {table} WHEN semon_event_writer()<>1 OR {predicate}{metadata} BEGIN SELECT RAISE(ABORT,'private native ownership refused'); END;")).map_err(failed)?;
        }
    }
    for table in TABLES.iter().copied().chain(["pricing_recompute_pending"]) {
        tx.execute(
            "INSERT OR IGNORE INTO storage_event_tables VALUES(?1,?2,0)",
            params![owner.config.namespace.as_str(), table],
        )
        .map_err(failed)?;
    }
    tx.commit().map_err(|_| StorageError::CommitUncertain)
}

pub(super) fn validate(db: &Connection, config: &StoreConfig) -> Result<(), StorageError> {
    SourceJournal(config.clone()).validate(db)?;
    validate_core(db)?;
    semantic_stage::support_sql(db, "", "").map_err(failed)?;
    validate_event_metadata(db)?;
    let ready: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_event_operations' AND type='table')", [], |r| r.get(0)).map_err(failed)?;
    if !ready {
        return Err(StorageError::NamespaceUpgradePending);
    }
    // Earlier pre-release singleton stores lack request binding. Never adopt
    // their unbound operation IDs as completions of the current contract.
    let bound: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('storage_event_operations') WHERE name='request_digest' AND type='TEXT' AND \"notnull\"=1)", [], |r| r.get(0)).map_err(failed)?;
    if !bound {
        return Err(StorageError::NamespaceUpgradePending);
    }
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM storage_event_tables WHERE namespace=?1 AND qualification=CASE WHEN table_name IN('files','events','signals') THEN 1 ELSE 0 END",
            [config.namespace.as_str()],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if count != TABLES.len() as i64 {
        return Err(StorageError::NamespaceUpgradePending);
    }
    for table in TABLES {
        let guards: i64 = db.query_row("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND tbl_name=?1 AND name IN ('storage_event_guard_'||?1||'_INSERT','storage_event_guard_'||?1||'_UPDATE','storage_event_guard_'||?1||'_DELETE')", [table], |r| r.get(0)).map_err(failed)?;
        if guards != 3 {
            return Err(StorageError::NamespaceUpgradePending);
        }
    }
    Ok(())
}

fn validate_core(db: &Connection) -> Result<(), StorageError> {
    for table in CORE_TABLES {
        let ready: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='namespace' AND type='TEXT' AND \"notnull\"=1 AND pk=1)", [table], |r| r.get(0)).map_err(failed)?;
        if !ready {
            return Err(StorageError::NamespaceUpgradePending);
        }
        let pk: String = db.query_row("SELECT group_concat(name,',') FROM (SELECT name FROM pragma_table_info(?1) WHERE pk>0 ORDER BY pk)", [table], |r| r.get(0)).map_err(failed)?;
        let expected = match *table {
            "files" => "namespace,file_id",
            "events" => "namespace,file_id,extra,seq",
            _ => "namespace,file_id,seq",
        };
        let bad_indexes: i64 = db.query_row("SELECT count(*) FROM pragma_index_list(?1) i WHERE NOT EXISTS(SELECT 1 FROM pragma_index_info(i.name) WHERE seqno=0 AND name='namespace')", [table], |r| r.get(0)).map_err(failed)?;
        let foreign_keys: i64 = db.query_row("SELECT count(*) FROM pragma_foreign_key_list(?1) WHERE seq=0 AND \"from\"='namespace' AND \"to\"='namespace' AND \"table\"='files'", [table], |r| r.get(0)).map_err(failed)?;
        if pk != expected || bad_indexes != 0 || foreign_keys != i64::from(*table != "files") {
            return Err(StorageError::NamespaceUpgradePending);
        }
    }
    Ok(())
}

fn validate_event_metadata(db: &Connection) -> Result<(), StorageError> {
    // Event preparation qualifies only the core. Metadata remains the authored
    // singleton family until its complete writer/consumer boundary is qualified.
    let composite_meta: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('meta','main') WHERE name='namespace')",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if composite_meta {
        Err(StorageError::NamespaceUpgradePending)
    } else {
        Ok(())
    }
}

/// Constant-size preparation of a fresh composite core. Existing singleton
/// stores refuse before writes; supported-current bounded migration is pending.
pub(crate) fn prepare(config: &StoreConfig) -> Result<(), StorageError> {
    let mut db = connection(config)?;
    SourceJournal(config.clone()).validate(&db)?;
    validate_core(&db)?;
    let composite_support =
        semantic_stage::support_sql(&db, "", "namespace").map_err(failed)? == "namespace";
    validate_event_metadata(&db)?;
    SourceJournal(config.clone()).prepare()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    SourceJournal(config.clone()).validate(&tx)?;
    validate_event_metadata(&tx)?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_event_operations')",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if exists {
        return validate(&tx, config);
    }
    tx.execute_batch("CREATE TABLE storage_event_tables(namespace TEXT NOT NULL,table_name TEXT NOT NULL,qualification INTEGER NOT NULL CHECK(qualification IN(0,1)),PRIMARY KEY(namespace,table_name)) STRICT, WITHOUT ROWID;
CREATE TABLE storage_event_operations(namespace TEXT NOT NULL,operation_id TEXT NOT NULL,source_path TEXT NOT NULL,request_digest TEXT NOT NULL CHECK(length(request_digest)=64),base TEXT NOT NULL,observed_revision INTEGER,target TEXT,consumed_bytes INTEGER,state TEXT NOT NULL CHECK(state IN('intent','complete')),PRIMARY KEY(namespace,operation_id)) STRICT, WITHOUT ROWID;
CREATE INDEX storage_event_reconcile ON storage_event_operations(namespace,source_path,base,observed_revision,state);").map_err(failed)?;
    for table in ["storage_event_tables", "storage_event_operations"] {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            let predicate = match operation {
                "UPDATE" => {
                    "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()"
                }
                "DELETE" => "OLD.namespace IS NOT semon_namespace()",
                _ => "NEW.namespace IS NOT semon_namespace()",
            };
            tx.execute_batch(&format!("CREATE TRIGGER storage_event_scope_{table}_{operation} BEFORE {operation} ON {table} WHEN semon_event_writer()<>1 OR {predicate} BEGIN SELECT RAISE(ABORT,'event operation namespace refused'); END;")).map_err(failed)?;
        }
    }
    for table in TABLES {
        let fences: Vec<String> = tx.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1 AND name GLOB 'storage_namespace_fence_*' ORDER BY name LIMIT 4").map_err(failed)?
            .query_map([table], |r| r.get(0)).map_err(failed)?.collect::<rusqlite::Result<_>>().map_err(failed)?;
        if fences.len() != 3 {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for name in fences {
            tx.execute_batch(&format!("DROP TRIGGER \"{}\";", name.replace('"', "\"\"")))
                .map_err(failed)?;
        }
        tx.execute(
            "INSERT INTO storage_event_tables VALUES(?1,?2,?3)",
            params![
                config.namespace.as_str(),
                table,
                i64::from(CORE_TABLES.contains(table))
            ],
        )
        .map_err(failed)?;
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            let record = if operation == "DELETE" { "OLD" } else { "NEW" };
            let metadata = if *table == "meta" {
                // Native helpers may change source/proof metadata only. Catalog
                // epochs, publication markers and custody records stay fenced.
                // The enclosing guard binds this singleton family to its sole
                // enrolled Source via storage_event_tables and rejects every
                // unbound/wrong-Source writer. Do not manufacture a namespace
                // column without the composite metadata primary key.
                format!(
                    " OR NOT ({record}.key GLOB 'native_*' OR {record}.key GLOB 'archive_event_index:*')"
                )
            } else if CORE_TABLES.contains(table)
                || (composite_support
                    && [
                        "catalog_relocation_candidates",
                        "catalog_native_dependencies",
                        "session_source_mutations",
                    ]
                    .contains(table))
            {
                match operation {
                    "UPDATE" => " OR OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()".to_owned(),
                    "DELETE" => " OR OLD.namespace IS NOT semon_namespace()".to_owned(),
                    _ => " OR NEW.namespace IS NOT semon_namespace()".to_owned(),
                }
            } else {
                String::new()
            };
            tx.execute_batch(&format!("CREATE TRIGGER storage_event_guard_{table}_{operation} BEFORE {operation} ON \"{table}\" WHEN semon_event_writer()<>1 OR (SELECT count(*) FROM storage_namespaces)<>1 OR NOT EXISTS(SELECT 1 FROM storage_event_tables WHERE namespace=semon_namespace() AND table_name='{table}' AND qualification IN(0,1)){metadata} BEGIN SELECT RAISE(ABORT,'configured event writer refused'); END;")).map_err(failed)?;
        }
    }
    tx.commit().map_err(|_| StorageError::CommitUncertain)
}

#[derive(Clone)]
struct Intent {
    id: String,
    base: String,
    revision: Option<i64>,
}

pub(crate) struct Writer {
    config: StoreConfig,
    owner: Option<NativeOwnership>,
    intents: BTreeMap<String, Intent>,
}

impl Writer {
    pub(crate) fn new(config: &StoreConfig) -> Self {
        Self {
            config: config.clone(),
            owner: None,
            intents: BTreeMap::new(),
        }
    }

    pub(crate) fn owned(owner: NativeOwnership) -> Self {
        Self {
            config: owner.config.clone(),
            owner: Some(owner),
            intents: BTreeMap::new(),
        }
    }

    fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        if let Some(owner) = &self.owner {
            owner.validate_ready(db)
        } else {
            validate(db, &self.config)
        }
    }

    fn request_digest(&self, path: &str, harness: &str) -> Result<String, StorageError> {
        if path.is_empty()
            || path.len() > 4096
            || !matches!(harness, "claude" | "codex" | "copilot")
        {
            return Err(StorageError::InvalidArguments);
        }
        let mut digest = Sha256::new();
        digest.update(b"semon-native-event-request-v1");
        digest.update(CACHE_VERSION.to_be_bytes());
        for input in [self.config.namespace.as_str(), path, harness] {
            digest.update((input.len() as u64).to_be_bytes());
            digest.update(input.as_bytes());
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    fn snapshot(&self) -> Result<Connection, StorageError> {
        let db = connection(&self.config)?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate(&db)?;
        Ok(db)
    }

    pub(crate) fn ledger(&self, path: &str) -> Result<Option<Ledger>, StorageError> {
        let db = self.snapshot()?;
        ledger_row(&db, path)
            .map(|row| row.map(|(_, ledger)| ledger))
            .map_err(failed)
    }

    pub(crate) fn load(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StorageError> {
        // read_one starts a transaction, so use the snapshot's already pinned
        // connection directly with the same native row decoder.
        let db = self.snapshot()?;
        let mut readers = Readers::new(&db).map_err(failed)?;
        let mut statement = db
            .prepare(&format!(
                "SELECT {FILE_COLUMNS} FROM files WHERE namespace=semon_namespace() AND path=?1"
            ))
            .map_err(failed)?;
        let mut rows = statement.query([path]).map_err(failed)?;
        let mut found = rows
            .next()
            .map_err(failed)?
            .map(|row| read_file(&mut readers, row).map(|(_, ledger, index)| (ledger, index)))
            .transpose()
            .map_err(failed)?;
        if let Some((_, index)) = &mut found {
            self.bind_tree(index.tree_summary.as_mut());
        }
        Ok(found)
    }

    pub(crate) fn tree(
        &self,
        path: &str,
        stat: &Stat,
    ) -> Result<Option<(Ledger, crate::native_tree::Metadata)>, StorageError> {
        let db = self.snapshot()?;
        let Some((_, ledger)) = ledger_row(&db, path).map_err(failed)? else {
            return Ok(None);
        };
        if ledger.stat != *stat {
            return Ok(None);
        }
        let mut statement = db.prepare(NATIVE_METADATA_VALUE).map_err(failed)?;
        let Some(mut summary) =
            read_tree_summary(&db, &mut statement, path, &ledger).map_err(failed)?
        else {
            return Ok(None);
        };
        self.bind_tree(Some(&mut summary));
        if !chunk_proofs::header_ready(&db, path, &ledger).map_err(failed)? {
            return Ok(None);
        }
        let Some(marker) = read_parent_marker(&mut statement, path, &ledger).map_err(failed)?
        else {
            return Ok(None);
        };
        let Some(calls) = launch_calls::read(&db, path, &ledger).map_err(failed)? else {
            return Ok(None);
        };
        Ok(Some((
            ledger,
            crate::native_tree::Metadata {
                summary,
                chunks: None,
                marker,
                calls: std::sync::Arc::new(calls),
            },
        )))
    }

    fn bind_tree(&self, summary: Option<&mut crate::native_tree::Summary>) {
        if let Some(snapshot) = summary.and_then(|summary| summary.stored.as_mut()) {
            snapshot.bind_configured(self.config.clone());
        }
    }

    pub(crate) fn seen(
        &self,
        path: &str,
        ledger: &Ledger,
        identity: &str,
    ) -> Result<native_usage::Seen, StorageError> {
        native_usage::seen(&self.snapshot()?, path, ledger, identity).map_err(failed)
    }

    pub(crate) fn begin(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        operation_id: &str,
        harness: &str,
    ) -> Result<(), StorageError> {
        let request_digest = self.request_digest(path, harness)?;
        if path.is_empty()
            || path.len() > 4096
            || operation_id.len() != 64
            || !operation_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(StorageError::InvalidArguments);
        }
        let mut db = connection(&self.config)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        if ledger_row(&tx, path)
            .map_err(failed)?
            .as_ref()
            .map(|(_, l)| l)
            != expected
        {
            return Err(StorageError::Conflict);
        }
        let revision = crate::storage::sqlite::source_journal::claims(
            &tx,
            Some(self.config.namespace.as_str()),
            &[path.to_owned()],
        )
        .map_err(failed)?[0]
            .1;
        let base = expected.map(header_ledger).unwrap_or_default();
        let prior: Option<(String, String, Option<i64>, String)> = tx.query_row("SELECT source_path,base,observed_revision,request_digest FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2", params![self.config.namespace.as_str(),operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(failed)?;
        if let Some((old_path, old_base, old_revision, old_request)) = prior {
            if old_path != path
                || old_base != base
                || old_revision != revision
                || old_request != request_digest
            {
                return Err(StorageError::Conflict);
            }
        } else {
            tx.execute("INSERT INTO storage_event_operations(namespace,operation_id,source_path,request_digest,base,observed_revision,state) VALUES(?1,?2,?3,?4,?5,?6,'intent')", params![self.config.namespace.as_str(),operation_id,path,request_digest,base,revision]).map_err(failed)?;
        }
        let id = operation_id.to_owned();
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        self.intents
            .insert(path.to_owned(), Intent { id, base, revision });
        Ok(())
    }

    pub(crate) fn commit(&mut self, files: &[CommitFile<'_>]) -> Result<Outcome, StorageError> {
        if files.is_empty()
            || files.len() > 64
            || files
                .iter()
                .map(|f| f.0)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != files.len()
        {
            return Err(StorageError::InvalidArguments);
        }
        // Establish explicit admission before any Raw access, then release the
        // read snapshot. The writer transaction validates again below.
        drop(self.snapshot()?);
        // Raw checks precede the DB reservation. The parser qualifies complete
        // records and proofs on its open handle; no Raw I/O runs in the DB tx.
        for (path, _, _, ledger, _) in files {
            if Stat::of(Path::new(path)).ok().as_ref() != Some(&ledger.stat) {
                return Err(StorageError::Conflict);
            }
        }
        let mut db = connection(&self.config)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        let mut work = Vec::new();
        for (path, expected, _, ledger, _) in files {
            let intent = self
                .intents
                .get(*path)
                .ok_or(StorageError::InvalidArguments)?;
            if intent.base != expected.map(header_ledger).unwrap_or_default() {
                return Err(StorageError::Conflict);
            }
            let (state, target): (String, Option<String>) = tx.query_row("SELECT state,target FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2 AND source_path=?3 AND base=?4 AND observed_revision IS ?5", params![self.config.namespace.as_str(),intent.id,path,intent.base,intent.revision], |r| Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
            if state == "complete" {
                let target_ledger = header_ledger(ledger);
                if target.as_deref() != Some(target_ledger.as_str()) {
                    return Err(StorageError::Conflict);
                }
                work.push(false);
                continue;
            }
            if ledger_row(&tx, path)
                .map_err(failed)?
                .as_ref()
                .map(|(_, l)| l)
                != *expected
            {
                return Err(StorageError::Conflict);
            }
            let revision = crate::storage::sqlite::source_journal::claims(
                &tx,
                Some(self.config.namespace.as_str()),
                &[(*path).to_owned()],
            )
            .map_err(failed)?[0]
                .1;
            if revision != intent.revision {
                return Err(StorageError::Conflict);
            }
            work.push(true);
        }
        for ((path, expected, changes, ledger, index), write) in files.iter().zip(work) {
            if !write {
                continue;
            }
            if let Some((id, _)) = ledger_row(&tx, path).map_err(failed)? {
                let preserve_tree = *expected == Some(*ledger)
                    && tree_reusable(&tx, path, ledger, index).map_err(failed)?;
                if changes.is_none() {
                    clear_preserving_tree(&tx, id, preserve_tree).map_err(failed)?;
                } else if *expected == Some(*ledger) && !preserve_tree {
                    // A repaired summary may arrive as an empty append delta.
                    // Retire the bad header so write() cannot reuse its generation.
                    tx.execute(tree_state::sql(&tx, "DELETE FROM native_tree_headers WHERE file_id=?1", "DELETE FROM native_tree_headers WHERE namespace=semon_namespace() AND file_id=?1").map_err(failed)?, [id])
                        .map_err(failed)?;
                }
            }
            write_index_changes_measured(
                &tx,
                path,
                ledger,
                index,
                *changes,
                *expected,
                &mut crate::native_timing::Measurements::default(),
                Some(self.config.namespace.as_str()),
            )
            .map_err(failed)?;
            let intent = &self.intents[*path];
            tx.execute("UPDATE storage_event_operations SET target=?3,consumed_bytes=?4,state='complete' WHERE namespace=?1 AND operation_id=?2 AND state='intent'", params![self.config.namespace.as_str(),intent.id,header_ledger(ledger),int(ledger.offset)]).map_err(failed)?;
        }
        #[cfg(test)]
        effect("completion").map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        #[cfg(test)]
        if effect("commit_reply").is_err() {
            return Err(StorageError::CommitUncertain);
        }
        Ok(Outcome::Written)
    }

    pub(crate) fn reconcile(
        &self,
        operation_id: &str,
        path: &str,
        harness: &str,
    ) -> Result<Option<EventReceipt>, StorageError> {
        let request_digest = self.request_digest(path, harness)?;
        if operation_id.len() != 64 || !operation_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidArguments);
        }
        let db = self.snapshot()?;
        let row: Option<CompletionRow> = db.query_row("SELECT source_path,state,consumed_bytes,observed_revision,request_digest FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2", params![self.config.namespace.as_str(),operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(failed)?;
        let Some((source, state, consumed, revision, stored_request)) = row else {
            return Ok(None);
        };
        if source != path || stored_request != request_digest {
            return Err(StorageError::Conflict);
        }
        if state != "complete" {
            return Ok(None);
        }
        Ok(Some(EventReceipt {
            operation_id: operation_id.to_owned(),
            consumed_bytes: uint(consumed.ok_or(StorageError::Unavailable)?),
            observed_revision: revision,
            replayed: true,
        }))
    }

    pub(crate) fn receipt(&self, path: &str) -> Result<EventReceipt, StorageError> {
        let intent = self.intents.get(path).ok_or(StorageError::Unavailable)?;
        let db = self.snapshot()?;
        let consumed: i64 = db.query_row("SELECT consumed_bytes FROM storage_event_operations WHERE namespace=?1 AND operation_id=?2 AND state='complete'", params![self.config.namespace.as_str(),intent.id], |r| r.get(0)).map_err(failed)?;
        Ok(EventReceipt {
            operation_id: intent.id.clone(),
            consumed_bytes: uint(consumed),
            observed_revision: intent.revision,
            replayed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventCache;
    use crate::storage::{LogicalSourceNamespace, StoreBackend};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn private_composite_metadata_recipe_epochs_and_pricing_have_explicit_owners() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(metadata_schema(true)).unwrap();
        db.execute_batch("CREATE TABLE storage_source_journal(namespace TEXT PRIMARY KEY,revision INTEGER NOT NULL); INSERT INTO storage_source_journal VALUES('a',11),('b',22);
            CREATE TABLE files(namespace TEXT NOT NULL,file_id INTEGER NOT NULL,path TEXT NOT NULL,codex_native_usage TEXT,codex_pricing_fingerprint TEXT,PRIMARY KEY(namespace,file_id));
            CREATE TABLE native_usage_sources(source_path TEXT,usage_ready INTEGER);
            CREATE TABLE pricing_recompute_pending(owner_kind TEXT,owner_key TEXT,PRIMARY KEY(owner_kind,owner_key));").unwrap();
        db.execute_batch(PRICING_FILE_SCHEMA_META).unwrap();
        let bind = |db: &Connection, namespace: &str| {
            bind_metadata_scope(db, namespace).unwrap();
            let namespace = namespace.to_owned();
            db.create_scalar_function(
                "semon_namespace",
                0,
                FunctionFlags::SQLITE_UTF8,
                move |_| Ok(namespace.clone()),
            )
            .unwrap();
        };
        bind(&db, "a");
        semantic_stage::record_recipe(&db).unwrap();
        db.execute(
            native_metadata_put(&db).unwrap(),
            params!["catalog_generation", "generation-a"],
        )
        .unwrap();
        db.execute(metadata_increment(&db).unwrap(), ["semantic_epoch"])
            .unwrap();
        db.execute(
            metadata_increment(&db).unwrap(),
            ["native_tree_repair_clock"],
        )
        .unwrap();
        assert!(semantic_stage::recipe_current(&db).unwrap());
        assert_eq!(semantic_stage::revision(&db).unwrap(), 11);
        bind(&db, "b");
        assert!(
            !semantic_stage::recipe_current(&db).unwrap(),
            "A cannot qualify B's stale stage"
        );
        assert_eq!(semantic_stage::generation(&db).unwrap(), None);
        assert_eq!(semantic_stage::revision(&db).unwrap(), 22);
        db.execute(
            native_metadata_put(&db).unwrap(),
            params!["catalog_semantic_recipe", "stale-recipe"],
        )
        .unwrap();
        assert!(!semantic_stage::recipe_current(&db).unwrap());
        semantic_stage::record_recipe(&db).unwrap();
        db.execute(
            native_metadata_put(&db).unwrap(),
            params!["catalog_generation", "generation-b"],
        )
        .unwrap();
        db.execute(metadata_increment(&db).unwrap(), ["semantic_epoch"])
            .unwrap();
        db.execute(
            metadata_increment(&db).unwrap(),
            ["native_tree_repair_clock"],
        )
        .unwrap();
        {
            let tx = db.transaction().unwrap();
            tx.execute(metadata_increment(&tx).unwrap(), ["semantic_epoch"])
                .unwrap();
            tx.execute(
                native_metadata_put(&tx).unwrap(),
                params!["catalog_semantic_recipe", "rolled-back"],
            )
            .unwrap();
        }
        assert!(semantic_stage::recipe_current(&db).unwrap());
        let epochs: Vec<(String, String)> = db
            .prepare(
                "SELECT namespace,value FROM meta WHERE key='semantic_epoch' ORDER BY namespace",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            epochs,
            vec![("a".into(), "1".into()), ("b".into(), "1".into())]
        );
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE namespace='' AND key='native_tree_repair_clock'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "2"
        );
        bind(&db, "a");
        assert_eq!(
            semantic_stage::generation(&db).unwrap().as_deref(),
            Some("generation-a")
        );
        db.execute(
            native_metadata_put(&db).unwrap(),
            params!["pricing_recompute_target", "current"],
        )
        .unwrap();
        // Poison a non-owner row directly in this private fixture. Authored
        // triggers must ignore it and consult only the reserved store scope.
        db.execute(
            "INSERT INTO meta VALUES('a','pricing_recompute_target','wrong')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO files VALUES('a',1,'same',?1,NULL)",
            [r#"{"cost":{"pricing_fingerprint":"current"}}"#],
        )
        .unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM pricing_recompute_pending", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        db.execute(
            native_metadata_put(&db).unwrap(),
            params!["pricing_recompute_target", "next"],
        )
        .unwrap();
        db.execute("UPDATE files SET codex_native_usage=codex_native_usage", [])
            .unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM pricing_recompute_pending", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        for key in SOURCE_METADATA_KEYS {
            assert_eq!(metadata_namespace("a", key), Ok("a"));
            assert_eq!(metadata_namespace("b", key), Ok("b"));
        }
        assert_eq!(
            metadata_namespace("a", "unclassified-owner"),
            Err(StorageError::InvalidArguments)
        );
    }

    #[test]
    fn private_composite_meta_proofs_collide_without_crossing_namespaces() {
        // Only the metadata/proof family is under test. Catalog ownership,
        // public enrollment and the complete native writer remain unqualified.
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(metadata_schema(true)).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(crate::history_projection::SCHEMA).unwrap();
        db.execute_batch(semantic_stage::metadata_indexes(true))
            .unwrap();
        let bind = |db: &Connection, namespace: &str| {
            bind_metadata_scope(db, namespace).unwrap();
            let namespace = namespace.to_owned();
            db.create_scalar_function(
                "semon_namespace",
                0,
                FunctionFlags::SQLITE_UTF8,
                move |_| Ok(namespace.clone()),
            )
            .unwrap();
        };
        let state = |db: &Connection, namespace: &str| -> Vec<(String, String)> {
            db.prepare("SELECT key,value FROM meta WHERE namespace=?1 ORDER BY key")
                .unwrap()
                .query_map([namespace], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        for index in [
            "sqlite_autoindex_meta_1",
            "semantic_codex_header_native",
            "semantic_codex_header_parent",
        ] {
            let first: String = db
                .query_row(
                    "SELECT name FROM pragma_index_xinfo(?1) WHERE seqno=0",
                    [index],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(first, "namespace", "{index}");
        }
        assert!(crate::storage::LogicalSourceNamespace::new(STORE_METADATA_NAMESPACE).is_err());
        assert_eq!(
            metadata_namespace("a", "native_unclassified:original"),
            Err(StorageError::InvalidArguments)
        );
        for prefix in SOURCE_METADATA_PREFIXES {
            assert_eq!(metadata_namespace("a", &format!("{prefix}same")), Ok("a"));
            assert_eq!(metadata_namespace("b", &format!("{prefix}same")), Ok("b"));
        }
        for key in STORE_METADATA_KEYS {
            assert_eq!(metadata_namespace("a", key), Ok(STORE_METADATA_NAMESPACE));
            assert_eq!(metadata_namespace("b", key), Ok(STORE_METADATA_NAMESPACE));
        }
        let path = "same/relative.jsonl";
        let offset = crate::source_chunks::SIZE as u64 * 1024 + 19;
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: offset,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset,
            prefix: [5; 32],
            tail: [6; 32],
        };
        for (namespace, marker) in [("a", 11), ("b", 22)] {
            bind(&db, namespace);
            let chunks = crate::source_chunks::Chunks {
                offset,
                hashes: vec![[marker; 32]; 1025],
                resume: None,
            };
            let mut calls = crate::native_handoff::Index::default();
            let value = serde_json::json!({"message":{"content":[{"type":"tool_use","id":namespace,"name":"Bash","input":{"command":"codex exec"}}]}});
            let bytes = format!("{value}\n");
            calls.observe(bytes.as_bytes(), 0, true, &value).unwrap();
            let tx = db.transaction().unwrap();
            chunk_proofs::write(&tx, path, &ledger, Some(&chunks)).unwrap();
            launch_calls::write(&tx, path, &ledger, Some(&calls)).unwrap();
            tx.execute(
                native_metadata_put(&tx).unwrap(),
                params![format!("native_tree_failure:{path}"), namespace],
            )
            .unwrap();
            // The reserved store scope is bound by the classified key, never
            // supplied by DEFAULT or adopted from the first source.
            tx.execute(
                native_metadata_put(&tx).unwrap(),
                params!["pricing_recompute_target", "one-store-pricing-table"],
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(
                chunk_proofs::read(&db, path, &ledger)
                    .unwrap()
                    .unwrap()
                    .as_ref(),
                &chunks
            );
            assert_eq!(launch_calls::read(&db, path, &ledger).unwrap(), Some(calls));
            assert!(chunk_proofs::header_ready(&db, path, &ledger).unwrap());
        }
        let b = state(&db, "b");
        // An intentionally shared reference fixture tests only the scoped
        // retention copy/GC statements, not Catalog publication qualification.
        let published = serde_json::json!({
            "key":"owner","name":"Synthetic owner","harness":"codex","kind":"Codex",
            "native_ids":["owner"],"model":"unknown","tokens":[0,0,0],
            "cost":{"pricing_fingerprint":"","usd":null,"unpriced_models":[],
                "split_unknown_messages":0,"by_model":{},"by_day":{}},
            "sources":[{"path":path,"native_id":"owner","dev":ledger.stat.dev,
                "ino":ledger.stat.ino,"size":ledger.stat.size,"modified_ns":ledger.stat.modified_ns,
                "changed_ns":ledger.stat.changed_ns,"offset":ledger.offset,
                "prefix_sha256":ledger.prefix,"tail_sha256":ledger.tail}]
        })
        .to_string();
        db.execute("INSERT INTO session_catalog(session_key,lifecycle,last_ms,harness,metadata) VALUES('owner','current',0,'codex',?1)", [published]).unwrap();
        db.execute(
            "INSERT INTO session_catalog_sources VALUES('owner','current','codex','owner',?1)",
            [path],
        )
        .unwrap();
        bind(&db, "a");
        {
            let tx = db.transaction().unwrap();
            chunk_proofs::clear(&tx, path).unwrap();
            let retained: i64 = tx.query_row("SELECT count(*) FROM meta WHERE namespace=?1 AND key GLOB 'native_published_chunks:*'", ["a"], |r| r.get(0)).unwrap();
            assert!(
                retained >= 3,
                "header and both proof pages must be retained"
            );
            assert_eq!(state(&tx, "b"), b);
            // Rollback must restore the live proofs and remove retention copies.
        }
        assert_eq!(state(&db, "b"), b);
        db.execute("DELETE FROM session_catalog_sources", [])
            .unwrap();
        db.execute("DELETE FROM session_catalog", []).unwrap();
        let global = state(&db, STORE_METADATA_NAMESPACE);
        assert_eq!(global.len(), 1);
        bind(&db, "a");
        let a = state(&db, "a");
        assert!(
            db.execute(
                native_metadata_put(&db).unwrap(),
                params!["native_unclassified:original", "refuse"]
            )
            .is_err()
        );
        assert_eq!(state(&db, "a"), a);
        {
            let tx = db.transaction().unwrap();
            chunk_proofs::clear(&tx, path).unwrap();
            launch_calls::clear(&tx, path).unwrap();
            tx.execute(
                native_metadata_delete(&tx).unwrap(),
                [format!("native_tree_failure:{path}")],
            )
            .unwrap();
            assert!(state(&tx, "a").is_empty());
            assert_eq!(state(&tx, "b"), b);
            assert_eq!(state(&tx, STORE_METADATA_NAMESPACE), global);
            // Dropping restores source A's headers, pages and tree witnesses.
        }
        assert_eq!(state(&db, "a"), a);
        let tx = db.transaction().unwrap();
        chunk_proofs::clear(&tx, path).unwrap();
        launch_calls::clear(&tx, path).unwrap();
        tx.execute(
            native_metadata_delete(&tx).unwrap(),
            [format!("native_tree_failure:{path}")],
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(chunk_proofs::read(&db, path, &ledger).unwrap().is_none());
        assert!(launch_calls::read(&db, path, &ledger).unwrap().is_none());
        assert_eq!(state(&db, "b"), b);
        assert_eq!(state(&db, STORE_METADATA_NAMESPACE), global);
        bind(&db, "b");
        assert_eq!(
            chunk_proofs::read(&db, path, &ledger)
                .unwrap()
                .unwrap()
                .hashes,
            vec![[22; 32]; 1025]
        );
        assert!(launch_calls::read(&db, path, &ledger).unwrap().is_some());
    }

    #[test]
    fn fresh_assembled_core_indexes_are_scoped_before_factory() {
        let f = Fixture::new();
        let db = f.db();
        // Inspect the complete create_native_store DDL, before native_events
        // can refuse it. The private core fixture omits semantic-stage DDL.
        let objects: Vec<(String, String, String)> = db
            .prepare("SELECT type,name,coalesce(sql,'') FROM sqlite_schema WHERE type IN('index','trigger') AND tbl_name IN('files','events','signals') ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut semantic_indexes = 0;
        for (kind, name, definition) in objects {
            if kind == "index" {
                let first: Option<String> = db
                    .query_row(
                        "SELECT name FROM pragma_index_xinfo(?1) WHERE seqno=0 AND key=1",
                        [&name],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(first.as_deref(), Some("namespace"), "{name}: {definition}");
                semantic_indexes += usize::from(name.starts_with("semantic_"));
            } else if name.starts_with("pricing_pending_file_") {
                assert!(definition.contains("namespace=NEW.namespace"), "{name}");
            } else {
                assert!(
                    name.starts_with("storage_namespace_fence_"),
                    "unexpected core trigger {name}"
                );
            }
        }
        assert_eq!(semantic_indexes, 5);
        validate_core(&db).unwrap();
    }

    #[test]
    fn composite_core_schema_and_qualification_are_physical() {
        let f = Fixture::new();
        drop(f.config.native_events().unwrap());
        let db = f.db();
        for table in CORE_TABLES
            .iter()
            .copied()
            .chain(["storage_event_tables", "storage_event_operations"])
        {
            let columns: Vec<(String, i64, Option<String>, i64)> = db
                .prepare("SELECT name,\"notnull\",dflt_value,pk FROM pragma_table_info(?1)")
                .unwrap()
                .query_map([table], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert!(
                columns
                    .iter()
                    .any(|(name, not_null, default, pk)| name == "namespace"
                        && *not_null == 1
                        && default.is_none()
                        && *pk == 1),
                "{table}"
            );
            let indexes: Vec<String> = db
                .prepare("SELECT name FROM pragma_index_list(?1)")
                .unwrap()
                .query_map([table], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert!(!indexes.is_empty(), "{table}");
            for index in indexes {
                let first: String = db
                    .query_row(
                        "SELECT name FROM pragma_index_info(?1) WHERE seqno=0",
                        [&index],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(first, "namespace", "{table}/{index}");
            }
            let fks: Vec<(i64, String, String)> = db
                .prepare("SELECT seq,\"from\",\"to\" FROM pragma_foreign_key_list(?1)")
                .unwrap()
                .query_map([table], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            for (_, from, to) in fks.iter().filter(|(seq, _, _)| *seq == 0) {
                assert_eq!(
                    (from.as_str(), to.as_str()),
                    ("namespace", "namespace"),
                    "{table}"
                );
            }
            if table == "events" || table == "signals" {
                assert_eq!(fks.len(), 2);
            }
        }
        for table in TABLES {
            let qualification: i64 = db.query_row("SELECT qualification FROM storage_event_tables WHERE namespace=?1 AND table_name=?2", params![f.config.namespace.as_str(),table], |r| r.get(0)).unwrap();
            assert_eq!(
                qualification,
                i64::from(CORE_TABLES.contains(table)),
                "{table}"
            );
            if !CORE_TABLES.contains(table) {
                let namespace_pk: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='namespace' AND pk=1)", [table], |r| r.get(0)).unwrap();
                assert!(
                    !namespace_pk,
                    "unported family claimed composite keys: {table}"
                );
            }
        }
    }

    #[test]
    fn private_composite_core_collisions_reads_deletes_and_rollback() {
        // Deliberately bypass public enrollment in this private family fixture.
        // The rest of the native writer is still unqualified for two sources.
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(metadata_schema(false)).unwrap();
        db.execute_batch(CORE_SCHEMA).unwrap();
        db.execute_batch(copied_usage_schema(false)).unwrap();
        db.execute_batch(tree_state::SCHEMA).unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=ON; CREATE TABLE storage_namespaces(namespace TEXT PRIMARY KEY);",
        )
        .unwrap();
        let bind = |db: &Connection, namespace: &str| {
            let namespace = namespace.to_owned();
            db.create_scalar_function(
                "semon_namespace",
                0,
                FunctionFlags::SQLITE_UTF8,
                move |_| Ok(namespace.clone()),
            )
            .unwrap();
        };
        let put = |db: &Connection, path: &str, size: i64| -> i64 {
            db.query_row(
                PUT_FILE,
                params![
                    path,
                    1,
                    2,
                    size,
                    0_u128.to_be_bytes(),
                    size,
                    [0_u8; 32],
                    [0_u8; 32],
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    false,
                    Option::<i64>::None,
                    Option::<i64>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    "{}",
                    "{}",
                    "{}",
                    "{}",
                    "{}",
                    "{}",
                    "{}",
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<Vec<u8>>::None
                ],
                |r| r.get(0),
            )
            .unwrap()
        };
        let state = |db: &Connection, namespace: &str| -> Vec<Vec<rusqlite::types::Value>> {
            let mut all = Vec::new();
            for table in CORE_TABLES {
                let mut statement = db
                    .prepare(&format!(
                        "SELECT * FROM {table} WHERE namespace=?1 ORDER BY 1,2,3"
                    ))
                    .unwrap();
                let columns = statement.column_count();
                let rows = statement
                    .query_map([namespace], |r| (0..columns).map(|i| r.get(i)).collect())
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                all.extend(rows);
            }
            all
        };
        for (namespace, size) in [("a", 10), ("b", 20)] {
            bind(&db, namespace);
            assert_eq!(
                put(&db, "same/relative.jsonl", size),
                1,
                "IDs must collide across namespaces"
            );
            let mut statement = db.prepare(PUT_EVENT).unwrap();
            let event = Event {
                n: Some(namespace.to_owned()),
                ..Event::default()
            };
            put_event(&mut statement, 1, false, 0, &event).unwrap();
            db.execute(
                PUT_SIGNAL,
                params![
                    1,
                    0,
                    "fixture",
                    0,
                    Option::<i64>::None,
                    0,
                    namespace,
                    Option::<i64>::None
                ],
            )
            .unwrap();
            let mut readers = Readers::new(&db).unwrap();
            assert_eq!(
                readers
                    .events
                    .query_row([1], |r| r.get::<_, String>(7))
                    .unwrap(),
                namespace
            );
            assert_eq!(
                readers
                    .signals
                    .query_row([1], |r| r.get::<_, String>(5))
                    .unwrap(),
                namespace
            );
            assert_eq!(
                ledger_row(&db, "same/relative.jsonl")
                    .unwrap()
                    .unwrap()
                    .1
                    .offset,
                size as u64
            );
        }
        assert_eq!(put(&db, "only-b.jsonl", 21), 2);
        let b = state(&db, "b");
        bind(&db, "a");
        assert!(
            put_event(
                &mut db.prepare(PUT_EVENT).unwrap(),
                2,
                false,
                0,
                &Event::default()
            )
            .is_err(),
            "a child cannot use b's parent"
        );
        assert_eq!(put(&db, "same/relative.jsonl", 30), 1);
        put_event(
            &mut db.prepare(PUT_EVENT).unwrap(),
            1,
            false,
            1,
            &Event::default(),
        )
        .unwrap();
        assert_eq!(
            ledger_row(&db, "same/relative.jsonl")
                .unwrap()
                .unwrap()
                .1
                .offset,
            30
        );
        assert_eq!(state(&db, "b"), b);
        {
            let tx = db.transaction().unwrap();
            clear(&tx, 1).unwrap();
            tx.execute(
                "DELETE FROM files WHERE namespace=semon_namespace() AND file_id=?1",
                [1],
            )
            .unwrap();
            assert!(ledger_row(&tx, "same/relative.jsonl").unwrap().is_none());
            assert_eq!(state(&tx, "b"), b);
            // Dropping the transaction restores all three families.
        }
        assert!(ledger_row(&db, "same/relative.jsonl").unwrap().is_some());
        let tx = db.transaction().unwrap();
        clear(&tx, 1).unwrap();
        tx.execute(
            "DELETE FROM files WHERE namespace=semon_namespace() AND file_id=?1",
            [1],
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(state(&db, "b"), b);
        bind(&db, "b");
        assert_eq!(
            ledger_row(&db, "same/relative.jsonl")
                .unwrap()
                .unwrap()
                .1
                .offset,
            20
        );
    }

    #[test]
    fn existing_singleton_store_refuses_without_native_changes() {
        let f = Fixture::new();
        // The create-new contract refuses even an already initialized file.
        assert_eq!(
            f.config.create_native_store(),
            Err(StorageError::Unavailable)
        );
        let legacy = f.root.join("legacy.json");
        drop(EventCache::open_scoped(&legacy).unwrap());
        let mut config = f.config.clone();
        config.backend = StoreBackend::Sqlite {
            database: EventCache::path(&legacy),
        };
        let StoreBackend::Sqlite { database } = &config.backend;
        let db = Connection::open(database).unwrap();
        config.expected_stream_epoch = Some(
            db.query_row(
                "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                [],
                |r| r.get(0),
            )
            .unwrap(),
        );
        db.execute("INSERT INTO meta VALUES('original-custody','keep')", [])
            .unwrap();
        config.admit_namespace().unwrap();
        for _ in 0..128 {
            if config.assign_catalog_namespace(64).unwrap().complete {
                break;
            }
        }
        assert!(matches!(
            config.native_events(),
            Err(StorageError::NamespaceUpgradePending)
        ));
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='original-custody'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "keep"
        );
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_event_operations')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('meta') WHERE name='namespace')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
    }

    #[test]
    fn prepared_old_connection_cannot_write_after_event_guard_installation() {
        let f = Fixture::new();
        let old = f.db();
        // Prepare against the admission fences before the configured writer
        // installs its new guards. SQLite must requalify this warm statement.
        let mut old_write = old
            .prepare("UPDATE files SET title='old-writer' WHERE path=?1")
            .unwrap();
        let mut old_metadata = old
            .prepare("UPDATE meta SET value='old-writer' WHERE key GLOB 'native_tree_summary:*'")
            .unwrap();
        let mut writer = f.config.native_events().unwrap();
        writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        let before = f.state();
        let summaries: i64 = f
            .db()
            .query_row(
                "SELECT count(*) FROM meta WHERE key GLOB 'native_tree_summary:*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(summaries > 0, "the metadata fence must protect real rows");
        assert!(old_write.execute([f.raw.to_str().unwrap()]).is_err());
        assert!(old_metadata.execute([]).is_err());
        let mut foreign = f.config.clone();
        foreign.namespace = LogicalSourceNamespace::new("foreign").unwrap();
        let refused = connection(&foreign)
            .unwrap()
            .execute(
                "UPDATE meta SET value='foreign-writer' WHERE key GLOB 'native_tree_summary:*'",
                [],
            )
            .unwrap_err();
        assert_eq!(
            refused.sqlite_error_code(),
            Some(ErrorCode::ConstraintViolation)
        );
        assert_eq!(f.state(), before);
        assert_eq!(
            crate::storage::sqlite::namespace::WRITER_QUALIFICATION_VERSION,
            0
        );
    }

    #[test]
    fn configured_repair_proof_snapshot_uses_admission_and_refuses_unbound_fallback() {
        let f = Fixture::new();
        let binding = crate::storage::NativeReaderBinding::Admitted(f.config.clone());
        assert!(binding.proof_snapshot().is_ok());
        let transitional =
            crate::storage::NativeReaderBinding::TransitionalUnboundLocal(f.config.clone());
        assert!(matches!(
            transitional.proof_snapshot(),
            Err(StorageError::NamespaceUpgradePending)
        ));
        let mut stale = f.config.clone();
        stale.expected_stream_epoch = Some("0".repeat(64));
        assert!(matches!(
            crate::storage::NativeReaderBinding::Admitted(stale).proof_snapshot(),
            Err(StorageError::ScopeChanged)
        ));
        let mut foreign = f.config.clone();
        foreign.namespace = LogicalSourceNamespace::new("foreign").unwrap();
        assert!(matches!(
            crate::storage::NativeReaderBinding::Admitted(foreign).proof_snapshot(),
            Err(StorageError::NamespaceMismatch)
        ));
        let mut absent = f.config.clone();
        let database = f.root.join("absent.sqlite3");
        absent.backend = StoreBackend::Sqlite {
            database: database.clone(),
        };
        assert!(
            crate::storage::NativeReaderBinding::Admitted(absent)
                .proof_snapshot()
                .is_err()
        );
        assert!(!database.exists());
    }

    struct Fixture {
        root: PathBuf,
        config: StoreConfig,
        raw: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_layout(PhysicalLayout::CONFIGURED)
        }
        fn with_layout(layout: PhysicalLayout) -> Self {
            Self::with_setup(layout, |_, _| {})
        }
        fn with_setup(layout: PhysicalLayout, setup: impl FnOnce(&Connection, &Path)) -> Self {
            Self::with_named_setup("configured-events", layout, setup)
        }
        fn with_named_setup(
            namespace: &str,
            layout: PhysicalLayout,
            setup: impl FnOnce(&Connection, &Path),
        ) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-configured-event-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache = root.join("index.json");
            let database = EventCache::path(&cache);
            let config = StoreConfig {
                backend: StoreBackend::Sqlite { database },
                namespace: LogicalSourceNamespace::new(namespace).unwrap(),
                expected_stream_epoch: Some("a".repeat(64)),
            };
            if layout.semantic_support {
                create_with_layout(&config, layout).unwrap();
            } else {
                config.create_native_store().unwrap();
            }
            let StoreBackend::Sqlite { database } = &config.backend;
            let db = Connection::open(database).unwrap();
            let raw = root.join("native.jsonl");
            setup(&db, &raw);
            db.execute(
                "INSERT INTO meta(key,value) VALUES('original-custody','preserve-verbatim')",
                [],
            )
            .unwrap();
            drop(db);
            config.admit_namespace().unwrap();
            for _ in 0..128 {
                if config.assign_catalog_namespace(64).unwrap().complete {
                    break;
                }
            }
            fs::write(&raw, concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"native-session\",\"cwd\":\"/sanitized\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"sanitized fixture\"}]}}\n"
            )).unwrap();
            Self { root, config, raw }
        }
        fn db(&self) -> Connection {
            let StoreBackend::Sqlite { database } = &self.config.backend;
            Connection::open(database).unwrap()
        }
        fn state(&self) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
            let db = self.db();
            TABLES
                .iter()
                .copied()
                .chain(["storage_source_journal", "session_catalog_invalidations"])
                .chain(semantic_stage::STAGE_TABLES.iter().copied())
                .chain(["storage_semantic_profiles"])
                .filter_map(|table| {
                    let present: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)", [table], |r| r.get(0)).unwrap();
                    // Early production refusal may precede journal preparation.
                    // Absence is represented by a missing map key, distinct
                    // from a present empty table, so creation is still a write.
                    if !present {
                        return None;
                    }
                    let mut statement = db
                        .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
                        .unwrap();
                    let columns = statement.column_count();
                    let rows = statement
                        .query_map([], |r| (0..columns).map(|i| r.get(i)).collect())
                        .unwrap()
                        .collect::<rusqlite::Result<_>>()
                        .unwrap();
                    Some((table.to_owned(), rows))
                })
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn configured_semantic_support_ingest_isolates_witnesses_restart_and_rollback() {
        let f = Fixture::with_setup(
            PhysicalLayout {
                semantic_support: true,
                ..PhysicalLayout::CONFIGURED
            },
            |db, raw| {
                let path = raw.to_string_lossy();
                db.execute("INSERT INTO catalog_native_dependencies VALUES('foreign',?1,'semantic-native:codex','native-session')", [path.as_ref()]).unwrap();
                db.execute(
                    "INSERT INTO session_source_mutations VALUES('foreign',?1,'foreign evidence')",
                    [path.as_ref()],
                )
                .unwrap();
                db.execute("INSERT INTO session_source_mutations VALUES('configured-events',?1,'retire this evidence')", [path.as_ref()]).unwrap();
                db.execute("INSERT INTO catalog_relocation_candidates VALUES('foreign','/foreign-old',?1,'native-session')", [path.as_ref()]).unwrap();
                db.execute("INSERT INTO meta VALUES('native_catalog_relocation:/foreign-old','foreign proof')", []).unwrap();
            },
        );
        let foreign = |db: &Connection| -> Vec<(String, String)> {
            db.prepare("SELECT harness,native_id FROM catalog_native_dependencies WHERE namespace='foreign' UNION ALL SELECT 'mutation',evidence FROM session_source_mutations WHERE namespace='foreign' UNION ALL SELECT 'relocation',old_path FROM catalog_relocation_candidates WHERE namespace='foreign' UNION ALL SELECT key,value FROM meta WHERE key='native_catalog_relocation:/foreign-old' ORDER BY 1,2").unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
        };
        let before = foreign(&f.db());
        assert_eq!(before.len(), 4);
        let mut writer = f.config.native_events().unwrap();
        writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        let db = f.db();
        assert_eq!(foreign(&db), before);
        assert_eq!(db.query_row("SELECT count(*) FROM catalog_native_dependencies WHERE namespace='configured-events' AND harness='semantic-native:codex' AND native_id='native-session'", [], |r| r.get::<_,i64>(0)).unwrap(), 1);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM session_source_mutations WHERE namespace='configured-events'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        drop(db);
        use std::io::Write;
        fs::OpenOptions::new().append(true).open(&f.raw).unwrap().write_all(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"append\"}]}}\n").unwrap();
        let committed = f.state();
        FAIL_EFFECT.with(|failure| failure.set(Some("mutation_retirement")));
        assert_eq!(
            writer.ingest(&"b".repeat(64), &f.raw, "codex"),
            Err(StorageError::Unavailable)
        );
        assert_eq!(FAIL_EFFECT.with(|failure| failure.get()), None);
        assert_eq!(f.state(), committed);
        assert_eq!(foreign(&f.db()), before);
        drop(writer);
        let mut reopened = f.config.native_events().unwrap();
        assert!(
            !reopened
                .ingest(&"b".repeat(64), &f.raw, "codex")
                .unwrap()
                .replayed
        );
        assert_eq!(foreign(&f.db()), before);
        let committed = f.state();
        assert!(
            reopened
                .ingest(&"b".repeat(64), &f.raw, "codex")
                .unwrap()
                .replayed
        );
        assert_eq!(f.state(), committed);
        let wrong = connection(&f.config).unwrap();
        assert!(
            wrong
                .execute(
                    "DELETE FROM catalog_native_dependencies WHERE namespace='foreign'",
                    []
                )
                .is_err()
        );
        assert!(wrong.execute("UPDATE session_source_mutations SET evidence='wrong' WHERE namespace='foreign'", []).is_err());
        assert_eq!(foreign(&f.db()), before);
        assert_eq!(
            crate::storage::sqlite::namespace::WRITER_QUALIFICATION_VERSION,
            0
        );
    }

    #[test]
    fn configured_semantic_support_mixed_family_refuses_before_ingest() {
        let f = Fixture::with_setup(
            PhysicalLayout {
                semantic_support: true,
                ..PhysicalLayout::CONFIGURED
            },
            |db, _| {
                db.execute_batch("DROP TABLE catalog_native_dependencies; CREATE TABLE catalog_native_dependencies(source_path TEXT NOT NULL,harness TEXT NOT NULL,native_id TEXT NOT NULL,PRIMARY KEY(source_path,harness,native_id)) STRICT, WITHOUT ROWID;").unwrap();
            },
        );
        let before = f.state();
        assert!(!before.contains_key("storage_source_journal"));
        assert!(matches!(
            f.config.native_events(),
            Err(StorageError::Unavailable)
        ));
        assert_eq!(f.state(), before);
        assert!(!f.state().contains_key("storage_source_journal"));
        assert!(!f.db().query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_event_operations')", [], |r| r.get::<_,bool>(0)).unwrap());
    }

    #[test]
    fn configured_semantic_stage_authored_keys_preserve_collisions_and_refuse_unported_consumers() {
        let mut db = Connection::open_in_memory().unwrap();
        bind_metadata_scope(&db, "a").unwrap();
        db.create_scalar_function("semon_namespace", 0, FunctionFlags::SQLITE_UTF8, |_| {
            Ok("a".to_owned())
        })
        .unwrap();
        db.pragma_update(None, "foreign_keys", true).unwrap();
        for ddl in authored_storage_ddl(PhysicalLayout {
            core: true,
            metadata: true,
            catalog: true,
            native_usage: true,
            pricing_queue: true,
            semantic_support: true,
            semantic_stage: true,
            ..PhysicalLayout::default()
        })
        .unwrap()
        {
            db.execute_batch(ddl).unwrap();
        }
        for table in [
            "catalog_semantic_stage",
            "catalog_semantic_frontier",
            "catalog_semantic_sources",
            "catalog_semantic_inputs",
            "catalog_semantic_keys",
            "catalog_semantic_endpoints",
            "catalog_semantic_native",
            "catalog_semantic_owner_sources",
            "catalog_semantic_paths",
            "catalog_semantic_names",
            "catalog_semantic_events",
            "catalog_semantic_relations",
        ] {
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND type='TEXT' AND [notnull]=1 AND pk=1)", [table], |r| r.get::<_,bool>(0)).unwrap(), "{table}");
            assert_eq!(db.query_row("SELECT count(*) FROM pragma_index_list(?1,'main') i WHERE NOT EXISTS(SELECT 1 FROM pragma_index_info(i.name,'main') WHERE seqno=0 AND name='namespace')", [table], |r| r.get::<_,i64>(0)).unwrap(), 0, "{table}");
        }
        for namespace in ["a", "b"] {
            db.execute("INSERT INTO catalog_semantic_stage(namespace,singleton,epoch,phase,runtime,revision) VALUES(?1,1,7,'indexing','same-runtime',1)", [namespace]).unwrap();
            db.execute("INSERT INTO catalog_semantic_sources(namespace,source_path,claim) VALUES(?1,'/same/native.jsonl',1)", [namespace]).unwrap();
            db.execute("INSERT INTO catalog_semantic_events(namespace,source_path,event_key,owner,kind,metadata,native_seq) VALUES(?1,'/same/native.jsonl','same-event','same-session','message','{}',1)", [namespace]).unwrap();
        }
        let before: i64 = db
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap();
        assert!(semantic_stage::active(&db).is_err());
        assert_eq!(
            db.query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before
        );
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "DELETE FROM catalog_semantic_events WHERE namespace='a'",
            [],
        )
        .unwrap();
        assert_eq!(
            tx.query_row(
                "SELECT count(*) FROM catalog_semantic_events WHERE namespace='b'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        tx.rollback().unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM catalog_semantic_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        assert!(matches!(
            authored_storage_ddl(PhysicalLayout {
                semantic_stage: true,
                ..PhysicalLayout::CONFIGURED
            }),
            Err(DdlProfileError::UnsupportedFamilyCombination)
        ));
    }

    #[test]
    fn configured_semantic_runtime_capability_is_crate_private_and_public_factory_stays_closed() {
        let f = semantic_fixture("runtime-a");
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::validate(&f.config).unwrap();
        let mut runtime =
            crate::storage::sqlite::configured_index::open_semantic(capability).unwrap();
        let paths = [f.raw.to_string_lossy().into_owned()];
        assert_eq!(
            runtime
                .begin_semantic_stage(&paths, "runtime", true)
                .unwrap(),
            Outcome::Written
        );
        let before = f.state();
        let mut public = crate::storage::sqlite::open_configured_index(&f.config).unwrap();
        assert!(
            public
                .begin_semantic_stage(&paths, "runtime", true)
                .is_err()
        );
        assert_eq!(f.state(), before);
        let singleton = Fixture::new();
        drop(singleton.config.native_events().unwrap());
        let before = singleton.state();
        assert!(
            crate::storage::sqlite::semantic::SemanticStaging::validate(&singleton.config).is_err()
        );
        assert_eq!(
            singleton.state(),
            before,
            "capability validation must not convert schema"
        );
        assert!(!singleton.state().contains_key("storage_semantic_profiles"));
        assert_eq!(
            crate::storage::sqlite::namespace::WRITER_QUALIFICATION_VERSION,
            0
        );
    }

    #[test]
    fn configured_semantic_batch_reduces_events_native_keys_frontier_scoped() {
        use crate::model::relation_stage::Phase;
        let f = semantic_fixture("reducer-a");
        let mut store = runtime_semantic(&f.config);
        let paths = [f.raw.to_string_lossy().into_owned()];
        let claims = store.catalog_claims(&paths).unwrap();
        let generation = store.session_catalog_generation().unwrap();
        store.begin_semantic_stage(&paths, "runtime", true).unwrap();
        let indexing = store.semantic_batch(64).unwrap().unwrap();
        assert_eq!(indexing.phase, Phase::Indexing);
        let prepared = semantic_prepared(&mut *store, &f, &indexing);
        assert_eq!(
            store.record_semantic_batch(&indexing, &prepared).unwrap(),
            Outcome::Written
        );
        let identity = store.semantic_batch(64).unwrap().unwrap();
        assert_eq!(identity.phase, Phase::Identity);
        assert_eq!(identity.base_generation, generation);
        let mut prepared = semantic_prepared(&mut *store, &f, &identity);
        add_primitive_endpoint(&mut prepared);
        assert_eq!(
            store.record_semantic_batch(&identity, &prepared).unwrap(),
            Outcome::Written
        );
        let relations = store.semantic_batch(64).unwrap().unwrap();
        assert_eq!(relations.phase, Phase::Relations);
        assert_eq!(
            relations.context.endpoints["native-session"].root,
            "native-session"
        );
        let db = f.db();
        assert_eq!(
            db.query_row(
                "SELECT events_done FROM catalog_semantic_sources WHERE namespace='reducer-a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        // Shared event reduction deliberately creates no Claude relationship
        // events for primitive Codex. Original native events remain authoritative.
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM catalog_semantic_events WHERE namespace='reducer-a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(db.query_row("SELECT session_key FROM catalog_semantic_keys WHERE namespace='reducer-a' AND harness='codex' AND native_id='native-session'",[],|r|r.get::<_,String>(0)).unwrap(),"native-session");
        assert_eq!(db.query_row("SELECT count(*) FROM catalog_semantic_frontier WHERE namespace='reducer-a' AND done=0",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(db.query_row("SELECT parent_resolved+cost_done FROM catalog_semantic_endpoints WHERE namespace='reducer-a'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM catalog_semantic_keys WHERE namespace<>'reducer-a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        let prepared = semantic_prepared(&mut *store, &f, &relations);
        assert_eq!(
            store.record_semantic_batch(&relations, &prepared).unwrap(),
            Outcome::Written
        );
        assert_eq!(store.catalog_claims(&paths).unwrap(), claims);
        assert_eq!(store.session_catalog_generation().unwrap(), generation);
        let publication = store.semantic_batch(64).unwrap().unwrap();
        assert_eq!(publication.phase, Phase::Publication);
        let before = f.state();
        let mut public = crate::storage::sqlite::open_configured_index(&f.config).unwrap();
        assert!(public.begin_semantic_stage(&paths, "public", true).is_err());
        assert_eq!(f.state(), before);
        assert_eq!(
            crate::storage::sqlite::namespace::WRITER_QUALIFICATION_VERSION,
            0
        );
    }

    #[test]
    fn configured_semantic_reset_and_reduction_rollback_zero_partial_writes() {
        let f = semantic_fixture("rollback-a");
        let paths = [f.raw.to_string_lossy().into_owned()];
        let mut store = runtime_semantic(&f.config);
        store.begin_semantic_stage(&paths, "runtime", true).unwrap();
        let batch = store.semantic_batch(64).unwrap().unwrap();
        let prepared = semantic_prepared(&mut *store, &f, &batch);
        store.record_semantic_batch(&batch, &prepared).unwrap();
        let before = f.state();
        FAIL_EFFECT.with(|fault| fault.set(Some("semantic_reduction")));
        assert!(store.semantic_batch(64).is_err());
        assert_eq!(f.state(), before);
        let identity = store.semantic_batch(64).unwrap().unwrap();
        let mut other = runtime_semantic(&f.config);
        let before = f.state();
        FAIL_EFFECT.with(|fault| fault.set(Some("semantic_reset")));
        assert!(
            other
                .begin_semantic_stage(&paths, "runtime-2", true)
                .is_err()
        );
        assert_eq!(f.state(), before);
        assert_eq!(
            other
                .begin_semantic_stage(&paths, "runtime-2", true)
                .unwrap(),
            Outcome::Written
        );
        let before = f.state();
        assert!(
            store.semantic_batch(64).is_err(),
            "stale epoch must refuse before reduction writes"
        );
        assert_eq!(
            store.begin_semantic_stage(&paths, "runtime", true).unwrap(),
            Outcome::Conflict
        );
        let mut stale = semantic_prepared(&mut *other, &f, &identity);
        stale.base_generation = Some("stale-generation".into());
        assert_eq!(
            other.record_semantic_batch(&identity, &stale).unwrap(),
            Outcome::Conflict
        );
        assert_eq!(f.state(), before);
        let restarted = other.semantic_batch(64).unwrap().unwrap();
        assert_eq!(restarted.paths, paths);
        assert!(restarted.epoch > identity.epoch);
        assert_eq!(other.catalog_claims(&paths).unwrap(), prepared.claims);
    }

    fn runtime_semantic(config: &StoreConfig) -> Box<dyn IndexStore> {
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::validate(config).unwrap();
        crate::storage::sqlite::configured_index::open_semantic(capability).unwrap()
    }

    fn owned_semantic(config: &StoreConfig) -> Box<dyn IndexStore> {
        let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
        crate::storage::sqlite::configured_index::open_semantic(capability).unwrap()
    }

    fn shared_state(f: &Fixture) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
        let db = f.db();
        let tables=db.prepare("SELECT name FROM main.sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 129").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert!(tables.len() <= 128);
        tables
            .into_iter()
            .map(|table| {
                let quoted = table.replace('"', "\"\"");
                let columns = db
                    .prepare(&format!("SELECT * FROM \"{quoted}\""))
                    .unwrap()
                    .column_count();
                let order = (1..=columns)
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let mut statement = db
                    .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY {order}"))
                    .unwrap();
                let rows = statement
                    .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<Vec<rusqlite::types::Value>>>>()
                    .unwrap();
                (table, rows)
            })
            .collect()
    }

    fn same_db_sources() -> (Fixture, StoreConfig, PathBuf) {
        use crate::storage::native_events::NativeEventStore;
        let mut a = Fixture::new();
        a.config.namespace = LogicalSourceNamespace::new("source-a").unwrap();
        a.config.backend = StoreBackend::Sqlite {
            database: a.root.join("composite.sqlite3"),
        };
        create_semantic_store(&a.config).unwrap();
        let mut b = a.config.clone();
        b.namespace = LogicalSourceNamespace::new("source-b").unwrap();
        let b_raw = a.root.join("native-b.jsonl");
        fs::write(&b_raw, concat!("{\"type\":\"session_meta\",\"payload\":{\"id\":\"native-b\",\"cwd\":\"/sanitized/b\"}}\n", "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"source B fixture\"}]}}\n")).unwrap();
        for (config, path) in [(&a.config, &a.raw), (&b, &b_raw)] {
            let capability =
                crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
            let mut cache = EventCache::open_semantic(capability).unwrap();
            // Reuse the operation ID deliberately: completion is source-owned.
            let receipt = cache.ingest(&"a".repeat(64), path, "codex").unwrap();
            assert!(!receipt.replayed);
        }
        (a, b, b_raw)
    }

    fn namespace_state(
        f: &Fixture,
        namespace: &str,
    ) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
        let db = f.db();
        shared_state(f)
            .into_iter()
            .map(|(table, rows)| {
                let columns = db
                    .prepare(&format!(
                        "PRAGMA main.table_info(\"{}\")",
                        table.replace('"', "\"\"")
                    ))
                    .unwrap()
                    .query_map([], |r| r.get::<_, String>(1))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                let index = columns.iter().position(|name| name == "namespace");
                let rows = rows
                    .into_iter()
                    .filter(|row| {
                        index.is_some_and(|index| {
                            row.get(index) == Some(&rusqlite::types::Value::Text(namespace.into()))
                        })
                    })
                    .collect();
                (table, rows)
            })
            .collect()
    }

    fn publication_ready(
        store: &mut dyn IndexStore,
        path: &Path,
        runtime: &str,
    ) -> (
        crate::model::relation_stage::Batch,
        crate::model::PreparedSources,
    ) {
        use crate::model::relation_stage::Phase;
        let paths = vec![path.to_string_lossy().into_owned()];
        assert_eq!(
            store.begin_semantic_stage(&paths, runtime, true).unwrap(),
            Outcome::Written
        );
        for _ in 0..128 {
            let batch = store.semantic_batch(64).unwrap().unwrap();
            if batch.paths.is_empty() {
                continue;
            }
            let mut prepared = semantic_prepared_path(store, path, &batch);
            match batch.phase {
                Phase::Indexing => {}
                Phase::Identity => add_primitive_endpoint(&mut prepared),
                Phase::Relations => {}
                Phase::Publication => {
                    prepared.rows = batch
                        .context
                        .endpoints
                        .values()
                        .map(|e| e.row.clone())
                        .collect();
                    assert_eq!(prepared.rows.len(), 1);
                    return (batch, prepared);
                }
            }
            assert_eq!(
                store.record_semantic_batch(&batch, &prepared).unwrap(),
                Outcome::Written
            );
        }
        panic!("bounded semantic stage did not reach publication")
    }

    fn model_ready(
        fixture: &Fixture,
        cache: &mut EventCache,
        options: &crate::Options,
        input: &crate::Input,
        runtime: &str,
    ) -> (
        crate::model::relation_stage::Batch,
        crate::model::SemanticModelBatch,
    ) {
        let paths = vec![input.full_path(options).to_string_lossy().into_owned()];
        assert_eq!(
            cache
                .begin_semantic_stage_with_journal(&paths, runtime, true)
                .unwrap(),
            Outcome::Written
        );
        for _ in 0..128 {
            let batch = cache.semantic_batch(64).unwrap().unwrap();
            if batch.paths.is_empty() {
                continue;
            }
            if batch.phase == crate::model::relation_stage::Phase::Publication {
                let before_prepare = shared_state(fixture);
                let prepared = crate::model::prepare_semantic_batch(
                    options,
                    cache,
                    std::slice::from_ref(input),
                    &batch,
                    1_790_000_000_000,
                )
                .unwrap();
                assert_eq!(
                    shared_state(fixture),
                    before_prepare,
                    "model preparation cannot create ingestion intents or mutate source data"
                );
                let before = shared_state(fixture);
                FAIL_EFFECT.with(|point| point.set(Some("semantic_model_record")));
                assert!(
                    cache
                        .record_semantic_batch(&batch, prepared.prepared())
                        .is_err()
                );
                FAIL_EFFECT.with(|point| point.set(None));
                assert_eq!(shared_state(fixture), before);
            }
            let model = cache
                .prepare_semantic_model_batch(
                    options,
                    std::slice::from_ref(input),
                    &batch,
                    1_790_000_000_000,
                )
                .unwrap();
            if batch.phase == crate::model::relation_stage::Phase::Publication {
                return (batch, model);
            }
        }
        panic!("real model did not reach publication")
    }

    fn model_sources() -> (Fixture, StoreConfig, [crate::Options; 2], [crate::Input; 2]) {
        let mut a = Fixture::new();
        a.config.namespace = LogicalSourceNamespace::new("source-a").unwrap();
        a.config.backend = StoreBackend::Sqlite {
            database: a.root.join("model.sqlite3"),
        };
        create_semantic_store(&a.config).unwrap();
        let mut b = a.config.clone();
        b.namespace = LogicalSourceNamespace::new("source-b").unwrap();
        let inputs = [
            crate::Input {
                root: crate::InputRoot::Codex,
                path: "sessions/rollout.jsonl".into(),
            },
            crate::Input {
                root: crate::InputRoot::Claude,
                path: "projects/sanitized/model-1.jsonl".into(),
            },
        ];
        let options = std::array::from_fn(|i| {
            let home = a.root.join(format!("source-{i}"));
            fs::create_dir_all(home.join("codex/sessions")).unwrap();
            fs::create_dir_all(home.join("claude/projects/sanitized")).unwrap();
            let facts = home.join("facts.json");
            crate::facts::write_facts(
                &facts,
                &crate::Facts {
                    version: 2,
                    hostname: format!("machine-{i}"),
                    ..Default::default()
                },
            )
            .unwrap();
            let options = crate::Options {
                claude_home: home.join("claude"),
                claude_json: home.join("claude.json"),
                codex_home: home.join("codex"),
                copilot_home: home.join("copilot"),
                proc_root: home.join("proc"),
                cache: home.join("unused-index.json"),
                all: true,
                facts: Some(facts),
                ..Default::default()
            };
            fs::write(inputs[i].full_path(&options), format!("{{\"timestamp\":\"2026-10-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"model-{i}\",\"cwd\":\"/sanitized\"}}}}\n{{\"timestamp\":\"2026-10-01T00:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"sanitized model fixture\"}}]}}}}\n")).unwrap();
            if i == 1 {
                fs::write(inputs[i].full_path(&options), concat!(
                    "{\"type\":\"user\",\"sessionId\":\"model-1\",\"uuid\":\"first\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"cwd\":\"/sanitized\",\"message\":{\"role\":\"user\",\"content\":\"sanitized Claude model fixture\"}}\n",
                    "{\"type\":\"assistant\",\"sessionId\":\"model-1\",\"uuid\":\"answer\",\"parentUuid\":\"first\",\"timestamp\":\"2026-10-01T00:01:00Z\",\"message\":{\"role\":\"assistant\",\"model\":\"sanitized-model\",\"content\":[{\"type\":\"text\",\"text\":\"recorded answer\"}]}}\n"
                )).unwrap();
            }
            options
        });
        (a, b, options, inputs)
    }

    fn assert_stored_model(
        fixture: &Fixture,
        config: &StoreConfig,
        options: &crate::Options,
        prepared: &crate::model::PreparedSources,
        query_row: &crate::model::summary::CatalogRow,
    ) {
        use crate::storage::query::QueryCatalogStore;
        let db = fixture.db();
        for row in &prepared.rows {
            let stored: String = db
                .query_row(
                    "SELECT metadata FROM session_catalog WHERE namespace=?1 AND session_key=?2",
                    params![config.namespace.as_str(), row.key],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(stored, serde_json::to_string(row).unwrap());
        }
        for projection in &prepared.transcripts {
            let slots: Vec<String> = db.prepare("SELECT metadata FROM session_slots WHERE namespace=?1 AND session_key=?2 ORDER BY slot").unwrap().query_map(params![config.namespace.as_str(),projection.key],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            assert!(!slots.is_empty(), "persisted transcript must be useful");
            assert_eq!(
                slots,
                projection
                    .slots
                    .iter()
                    .map(|slot| serde_json::to_string(slot).unwrap())
                    .collect::<Vec<_>>()
            );
            assert!(
                projection.slots.iter().any(|slot| slot.file.is_some()),
                "persisted recipe binds actual native evidence"
            );
            let version: u32 = db.query_row("SELECT version FROM session_slot_projections WHERE namespace=?1 AND session_key=?2",params![config.namespace.as_str(),projection.key],|r|r.get(0)).unwrap();
            assert_eq!(version, crate::slot_projection::VERSION);
        }
        let port = crate::storage::sqlite::query::SqliteQueryCatalog {
            options: options.clone(),
            binding: crate::storage::sqlite::semantic::SemanticStaging::admit(config)
                .unwrap()
                .reader_binding()
                .unwrap(),
        };
        let mut remaining = 8 * 1024 * 1024;
        let part = port
            .read_cohort(&config.namespace, 1_790_000_000_000, &mut remaining)
            .unwrap();
        let expected = query_row.query.as_ref().unwrap()["rows"][&query_row.key].clone();
        assert_eq!(
            part.rows[&query_row.key], expected,
            "ordinary Query port reads exact persisted facts"
        );
        assert_eq!(part.rows.len(), 1, "foreign Source excluded");
        port.validate(&config.namespace, &part).unwrap();
        let sources = std::sync::Arc::new(crate::SessionSources::with_machines(vec![(
            config.namespace.as_str().into(),
            options.clone(),
        )]));
        let mut query = crate::Query::from_sources(sources);
        let reply = query
            .call_bound_catalog_at(
                "list_sessions",
                &serde_json::json!({}),
                1_790_000_000_000,
                &port,
            )
            .unwrap();
        assert_eq!(reply["total"], 1);
        assert_eq!(
            reply["sessions"][0], expected["summary"],
            "existing Query filters/rendering consume persisted facts"
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM storage_namespaces WHERE writer_qualification<>0",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM storage_event_tables WHERE qualification<>0",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn configured_semantic_real_model_two_source_query_metadata_and_transcript() {
        use crate::storage::native_events::NativeEventStore;
        let (a, b, options, inputs) = model_sources();
        let mut caches: Vec<_> = [&a.config, &b]
            .into_iter()
            .map(|config| {
                EventCache::open_semantic(
                    crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
                )
                .unwrap()
            })
            .collect();
        for i in 0..2 {
            caches[i]
                .ingest(
                    &"a".repeat(64),
                    &inputs[i].full_path(&options[i]),
                    inputs[i].root.as_str(),
                )
                .unwrap();
        }
        for i in 0..2 {
            let foreign = if i == 0 { "source-b" } else { "source-a" };
            let before = namespace_state(&a, foreign);
            let (batch, model) =
                model_ready(&a, &mut caches[i], &options[i], &inputs[i], "real-model");
            assert!(model.prepared().rows[0].query.is_some());
            assert!(!model.prepared().transcripts[0].slots.is_empty());
            // Same production derivation, using the existing unbound producer
            // as the algorithm parity oracle for both Codex and Claude. The
            // frozen publication-port oracle remains unchanged in CI as well.
            let mut original = EventCache::open_scoped(&options[i].cache).unwrap();
            let original = crate::model::prepare_sources(
                &options[i],
                &mut original,
                std::slice::from_ref(&inputs[i]),
                1_790_000_000_000,
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(&model.prepared().rows).unwrap(),
                serde_json::to_value(&original.rows).unwrap()
            );
            assert_eq!(
                model.prepared().staged_projection_witness().unwrap(),
                original.staged_projection_witness().unwrap()
            );
            assert_eq!(
                caches[i]
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Written
            );
            let published = shared_state(&a);
            assert_eq!(
                caches[i]
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Written
            );
            assert_eq!(shared_state(&a), published);
            assert_eq!(namespace_state(&a, foreign), before);
            let config = if i == 0 { &a.config } else { &b };
            assert_stored_model(
                &a,
                config,
                &options[i],
                model.prepared(),
                &model.prepared().rows[0],
            );
            let port = crate::storage::sqlite::query::SqliteQueryCatalog {
                options: options[i].clone(),
                binding: crate::storage::sqlite::semantic::SemanticStaging::admit(config)
                    .unwrap()
                    .reader_binding()
                    .unwrap(),
            };
            let sources = std::sync::Arc::new(crate::SessionSources::with_machines(vec![(
                config.namespace.as_str().into(),
                options[i].clone(),
            )]));
            let mut query = crate::Query::from_sources(sources);
            let reply = query
                .call_bound_catalog_at(
                    "list_sessions",
                    &serde_json::json!({}),
                    1_790_000_000_000,
                    &port,
                )
                .unwrap();
            assert_eq!(reply["total"], 1);
            assert_eq!(reply["sessions"][0]["id"], format!("model-{i}"));
            assert_eq!(
                caches[i].finish_semantic_stage(batch.epoch).unwrap(),
                Outcome::Written
            );
        }
    }

    #[test]
    fn configured_semantic_real_model_claude_replacement_rollback_stale_and_history() {
        use crate::storage::native_events::NativeEventStore;
        use std::io::Write;
        let (a, b, options, inputs) = model_sources();
        let mut caches: Vec<_> = [&a.config, &b]
            .into_iter()
            .map(|config| {
                EventCache::open_semantic(
                    crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
                )
                .unwrap()
            })
            .collect();
        let mut original = None;
        for i in 0..2 {
            caches[i]
                .ingest(
                    &"a".repeat(64),
                    &inputs[i].full_path(&options[i]),
                    inputs[i].root.as_str(),
                )
                .unwrap();
            let (batch, model) = model_ready(
                &a,
                &mut caches[i],
                &options[i],
                &inputs[i],
                "original-model",
            );
            assert_eq!(
                caches[i]
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Written
            );
            if i == 1 {
                original = Some(model.prepared().rows[0].clone());
            }
            caches[i].finish_semantic_stage(batch.epoch).unwrap();
        }
        let foreign = namespace_state(&a, "source-a");
        let path = inputs[1].full_path(&options[1]);
        let mut raw = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(raw, "{{\"type\":\"user\",\"sessionId\":\"model-1\",\"uuid\":\"replacement\",\"parentUuid\":\"answer\",\"timestamp\":\"2026-10-01T00:02:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"continued Claude replacement\"}}}}").unwrap();
        drop(raw);
        caches[1].ingest(&"b".repeat(64), &path, "claude").unwrap();
        let mut stale = EventCache::open_semantic(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&b).unwrap(),
        )
        .unwrap();
        let (old_batch, old_model) =
            model_ready(&a, &mut stale, &options[1], &inputs[1], "old-model");
        let old_witness: (String, String, String) = a.db().query_row("SELECT metadata,projections,claims FROM storage_semantic_model_batches WHERE namespace='source-b'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        let (batch, model) = model_ready(&a, &mut caches[1], &options[1], &inputs[1], "old-model");
        assert!(batch.epoch > old_batch.epoch);
        let new_witness: (String, String, String) = a.db().query_row("SELECT metadata,projections,claims FROM storage_semantic_model_batches WHERE namespace='source-b'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(
            new_witness, old_witness,
            "same-runtime takeover preserves exact model progress"
        );
        let before = shared_state(&a);
        assert_eq!(
            stale
                .publish_semantic_catalog(&old_batch, old_model.prepared())
                .unwrap()
                .0,
            Outcome::Conflict
        );
        assert_eq!(
            shared_state(&a),
            before,
            "old epoch refuses before generation advances"
        );
        for fault in [
            "semantic_publish_history",
            "semantic_publish_slots",
            "semantic_publish_rows",
            "semantic_publish_search",
            "semantic_publish_proof",
            "semantic_publish_ack",
            "semantic_publish_head",
        ] {
            let before = shared_state(&a);
            FAIL_EFFECT.with(|point| point.set(Some(fault)));
            assert!(
                caches[1]
                    .publish_semantic_catalog(&batch, model.prepared())
                    .is_err(),
                "{fault}"
            );
            FAIL_EFFECT.with(|point| point.set(None));
            assert_eq!(shared_state(&a), before, "{fault}");
        }
        assert_eq!(
            caches[1]
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap()
                .0,
            Outcome::Written
        );
        let mut expected = original.unwrap();
        expected.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        let retained: String = a.db().query_row("SELECT metadata FROM session_history_catalog WHERE namespace='source-b' AND session_key=?1",[&expected.key],|r|r.get(0)).unwrap();
        assert_eq!(retained, serde_json::to_string(&expected).unwrap());
        // Query's existing retained-history preference remains exact after
        // replacement; Current metadata and recipes above are checked separately.
        assert_stored_model(&a, &b, &options[1], model.prepared(), &expected);
        assert_eq!(namespace_state(&a, "source-a"), foreign);
        let before = shared_state(&a);
        let digest = publication_snapshot_digest(&a);
        assert_eq!(
            stale
                .publish_semantic_catalog(&old_batch, old_model.prepared())
                .unwrap()
                .0,
            Outcome::Conflict
        );
        assert_eq!(shared_state(&a), before);
        assert_eq!(publication_snapshot_digest(&a), digest);
    }

    #[test]
    fn configured_semantic_t6_listing_readiness_does_not_qualify_other_readers() {
        use crate::storage::{native_events::NativeEventStore, query::QueryCatalogStore};
        let (a, b, options, inputs) = model_sources();
        let _other = owned_semantic(&b);
        let mut cache = EventCache::open_semantic(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config).unwrap(),
        )
        .unwrap();
        cache
            .ingest(&"a".repeat(64), &inputs[0].full_path(&options[0]), "codex")
            .unwrap();
        let (batch, model) = model_ready(&a, &mut cache, &options[0], &inputs[0], "readiness");
        cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        let binding = crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config)
            .unwrap()
            .reader_binding()
            .unwrap();
        let port = crate::storage::sqlite::query::SqliteQueryCatalog {
            options: options[0].clone(),
            binding: binding.clone(),
        };
        let before = shared_state(&a);
        let completeness: String = a
            .db()
            .query_row(
                "SELECT value FROM meta WHERE namespace='source-a' AND key='catalog_completeness'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(completeness, "partial");
        let mut left = 1024 * 1024;
        assert!(
            port.read_cohort(&a.config.namespace, 1_790_000_000_000, &mut left)
                .is_ok()
        );
        assert!(
            port.read_cohort(&b.namespace, 1_790_000_000_000, &mut left)
                .is_err()
        );
        assert!(
            port.has_search_slots(&a.config.namespace, "model-0", None)
                .is_err()
        );
        assert!(matches!(
            binding.proof_snapshot(),
            Err(StorageError::NamespaceUpgradePending)
        ));
        let navigation = crate::storage::navigation_store(binding.clone());
        assert_eq!(
            navigation.catalog_generation(),
            Err(StorageError::NamespaceUpgradePending)
        );
        assert_eq!(
            navigation.bookmarked_target_keys("model-0"),
            Err(StorageError::NamespaceUpgradePending)
        );
        assert!(matches!(
            navigation.native_resolution(
                "codex",
                "model-0",
                crate::CatalogReadScope::Current,
                None
            ),
            Err(StorageError::NamespaceUpgradePending)
        ));
        let sources = std::sync::Arc::new(crate::SessionSources::with_machines(vec![(
            a.config.namespace.as_str().into(),
            options[0].clone(),
        )]));
        let mut query = crate::Query::from_sources(sources);
        for tool in ["stalls", "find", "get_session", "read_transcript"] {
            assert!(
                query
                    .call_bound_catalog_at(tool, &serde_json::json!({}), 1_790_000_000_000, &port)
                    .is_err(),
                "{tool}"
            );
        }
        assert!(
            a.config
                .query_metadata(&options[0], 1_790_000_000_000, 1024 * 1024)
                .is_err()
        );
        assert!(a.config.catalog_generation().is_err());
        let private = owned_semantic(&a.config);
        assert!(matches!(
            private.session_slots(&model.prepared().rows[0], None, 64),
            Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending
            ))
        ));
        assert!(matches!(
            crate::storage::sqlite::open_configured_index(&a.config),
            Err(StorageError::MultiSourceUnqualified)
        ));
        assert_eq!(shared_state(&a), before);
    }

    #[test]
    fn configured_semantic_model_refuses_noncurrent_schema_and_changed_raw_zero_writes() {
        use crate::storage::native_events::NativeEventStore;
        let (a, _, options, inputs) = model_sources();
        let mut cache = EventCache::open_semantic(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config).unwrap(),
        )
        .unwrap();
        let path = inputs[0].full_path(&options[0]);
        cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
        cache
            .begin_semantic_stage_with_journal(
                &[path.to_string_lossy().into_owned()],
                "read-only",
                true,
            )
            .unwrap();
        let batch = cache.semantic_batch(64).unwrap().unwrap();
        fs::write(&path, "changed Raw\n").unwrap();
        let before = shared_state(&a);
        assert!(
            crate::model::prepare_semantic_batch(
                &options[0],
                &mut cache,
                std::slice::from_ref(&inputs[0]),
                &batch,
                1_790_000_000_000
            )
            .is_err()
        );
        assert_eq!(shared_state(&a), before);
        // Only the current Semon contract is admitted. No upgrade/fallback.
        a.db()
            .pragma_update(None, "user_version", SCHEMA_VERSION - 1)
            .unwrap();
        assert!(matches!(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config),
            Err(StorageError::NamespaceUpgradePending)
        ));
        assert_eq!(shared_state(&a), before);
    }

    #[test]
    fn configured_semantic_two_source_publication_ack_finish_and_head() {
        let (a, b, b_raw) = same_db_sources();
        let mut stores = [owned_semantic(&a.config), owned_semantic(&b)];
        for (i, path) in [&a.raw, &b_raw].into_iter().enumerate() {
            let (batch, prepared) = publication_ready(&mut *stores[i], path, "publish");
            let native = prepared.rows[0].native_ids[0].clone();
            let selected = stores[i]
                .semantic_select(
                    batch.epoch,
                    &crate::model::relation_stage::Selector::NativeKey {
                        native: native.clone(),
                        harness: "codex".into(),
                    },
                )
                .unwrap();
            assert_eq!(selected.key, Some(native));
            let owners = vec![prepared.rows[0].key.clone()];
            assert_eq!(
                stores[i]
                    .semantic_replay_page(batch.epoch, &owners, "")
                    .unwrap()
                    .endpoints
                    .len(),
                1
            );
            let (outcome, generation) = stores[i]
                .publish_semantic_catalog(&batch, &prepared)
                .unwrap();
            assert_eq!(outcome, Outcome::Written);
            assert!(generation.is_some());
            assert_eq!(stores[i].session_catalog_generation().unwrap(), generation);
            assert!(
                stores[i]
                    .catalog_claims(&batch.paths)
                    .unwrap()
                    .iter()
                    .all(|(_, claim)| claim.is_none())
            );
            assert_eq!(
                stores[i].finish_semantic_stage(batch.epoch).unwrap(),
                Outcome::Written
            );
            assert_eq!(stores[i].session_catalog_generation().unwrap(), generation);
        }
        let db = a.db();
        for (ns, key) in [("source-a", "native-session"), ("source-b", "native-b")] {
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM session_catalog WHERE namespace=?1 AND session_key=?2",
                    params![ns, key],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM storage_semantic_publications WHERE namespace=?1",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert!(
                db.query_row(
                    "SELECT count(*) FROM session_catalog_changes WHERE namespace=?1",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap()
                    > 0
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM catalog_semantic_stage WHERE namespace=?1",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM session_catalog WHERE namespace=?1 AND session_key<>?2",
                    params![ns, key],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
    }

    fn publication_snapshot_digest(f: &Fixture) -> String {
        use sha2::{Digest, Sha256};
        // Ordered tables, rows and typed values give a deterministic witness.
        format!(
            "{:x}",
            Sha256::digest(format!("{:?}", shared_state(f)).as_bytes())
        )
    }

    fn assert_old_epoch_zero_writes(
        fixture: &Fixture,
        old: &mut dyn IndexStore,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) {
        let before = shared_state(fixture);
        assert!(old.semantic_batch(64).is_err());
        assert_eq!(shared_state(fixture), before);
        assert_eq!(
            old.record_semantic_batch(batch, prepared).unwrap(),
            Outcome::Conflict
        );
        assert_eq!(shared_state(fixture), before);
        assert_eq!(
            old.publish_semantic_catalog(batch, prepared).unwrap().0,
            Outcome::Conflict
        );
        assert_eq!(shared_state(fixture), before);
        assert_eq!(
            old.finish_semantic_stage(batch.epoch).unwrap(),
            Outcome::Conflict
        );
        assert_eq!(
            old.begin_semantic_stage(&batch.paths, "stale-reset", true)
                .unwrap(),
            Outcome::Conflict
        );
        assert_eq!(shared_state(fixture), before);
    }

    #[test]
    fn configured_semantic_t1_same_runtime_takeover_retains_progress_and_fences_epoch() {
        let (a, _, _) = same_db_sources();
        let mut old = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *old, &a.raw, "same");
        let mut observer = owned_semantic(&a.config);
        let mut successor = owned_semantic(&a.config);
        assert_eq!(
            successor
                .begin_semantic_stage(&batch.paths, "same", true)
                .unwrap(),
            Outcome::Written
        );
        let next = successor.semantic_batch(64).unwrap().unwrap();
        assert!(next.epoch > batch.epoch);
        assert_eq!(next.phase, batch.phase);
        assert_eq!(next.paths, batch.paths);
        assert_eq!(
            serde_json::to_value(&next.context.endpoints).unwrap(),
            serde_json::to_value(&batch.context.endpoints).unwrap()
        );
        assert_eq!(
            successor.catalog_claims(&next.paths).unwrap(),
            prepared.claims
        );
        assert_old_epoch_zero_writes(&a, &mut *old, &batch, &prepared);
        assert_old_epoch_zero_writes(&a, &mut *observer, &batch, &prepared);
        assert_eq!(
            successor
                .publish_semantic_catalog(&next, &prepared)
                .unwrap()
                .0,
            Outcome::Written
        );
    }

    #[test]
    fn configured_semantic_t2_different_runtime_takeover_scopes_reset() {
        let (a, _, _) = same_db_sources();
        let mut old = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *old, &a.raw, "old");
        let foreign = namespace_state(&a, "source-b");
        let mut successor = owned_semantic(&a.config);
        assert_eq!(
            successor
                .begin_semantic_stage(&batch.paths, "new", true)
                .unwrap(),
            Outcome::Written
        );
        let next = successor.semantic_batch(64).unwrap().unwrap();
        assert!(next.epoch > batch.epoch);
        assert_eq!(next.phase, crate::model::relation_stage::Phase::Indexing);
        assert_eq!(namespace_state(&a, "source-b"), foreign);
        assert_old_epoch_zero_writes(&a, &mut *old, &batch, &prepared);
    }

    #[test]
    fn configured_semantic_t3_active_old_epoch_cannot_take_back_successor() {
        let (a, _, _) = same_db_sources();
        let mut old = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *old, &a.raw, "same");
        let mut successor = owned_semantic(&a.config);
        successor
            .begin_semantic_stage(&batch.paths, "same", true)
            .unwrap();
        assert_old_epoch_zero_writes(&a, &mut *old, &batch, &prepared);
        let current = successor.semantic_batch(64).unwrap().unwrap();
        let before = shared_state(&a);
        assert_eq!(
            successor
                .begin_semantic_stage(&batch.paths, "same", true)
                .unwrap(),
            Outcome::Written
        );
        assert_eq!(shared_state(&a), before, "same holder begin is idempotent");
        assert_eq!(
            successor.semantic_batch(64).unwrap().unwrap().epoch,
            current.epoch
        );
    }

    #[test]
    fn configured_semantic_t4_quiescent_post_finish_takeover_fences_new_handle() {
        let (a, _, _) = same_db_sources();
        let mut quiescent = owned_semantic(&a.config);
        let (first, prepared) = publication_ready(&mut *quiescent, &a.raw, "same");
        quiescent
            .publish_semantic_catalog(&first, &prepared)
            .unwrap();
        quiescent.finish_semantic_stage(first.epoch).unwrap();
        let mut old = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *old, &a.raw, "same");
        assert_eq!(
            quiescent
                .begin_semantic_stage(&batch.paths, "same", true)
                .unwrap(),
            Outcome::Written
        );
        let next = quiescent.semantic_batch(64).unwrap().unwrap();
        assert!(next.epoch > batch.epoch);
        assert_eq!(next.phase, batch.phase);
        assert_old_epoch_zero_writes(&a, &mut *old, &batch, &prepared);
        assert_eq!(
            quiescent
                .publish_semantic_catalog(&next, &prepared)
                .unwrap()
                .0,
            Outcome::Written
        );
    }

    #[test]
    fn configured_semantic_t5_same_runtime_takeover_rolls_back_whole_database() {
        let (a, _, _) = same_db_sources();
        let mut old = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *old, &a.raw, "same");
        let mut successor = owned_semantic(&a.config);
        let before = shared_state(&a);
        FAIL_EFFECT.with(|point| point.set(Some("semantic_reset")));
        assert!(
            successor
                .begin_semantic_stage(&batch.paths, "same", true)
                .is_err()
        );
        FAIL_EFFECT.with(|point| point.set(None));
        assert_eq!(shared_state(&a), before);
        assert_eq!(old.semantic_batch(64).unwrap().unwrap().epoch, batch.epoch);
        assert_eq!(
            old.publish_semantic_catalog(&batch, &prepared).unwrap().0,
            Outcome::Written
        );
    }

    #[test]
    fn configured_semantic_competing_publisher_and_begin_zero_writes() {
        let (a, _, _) = same_db_sources();
        let mut stale = owned_semantic(&a.config);
        let (old, old_prepared) = publication_ready(&mut *stale, &a.raw, "old");
        let mut winner = owned_semantic(&a.config);
        let (current, prepared) = publication_ready(&mut *winner, &a.raw, "winner");
        assert!(current.epoch > old.epoch);
        assert_eq!(
            winner
                .publish_semantic_catalog(&current, &prepared)
                .unwrap()
                .0,
            Outcome::Written
        );
        let before = shared_state(&a);
        let digest = publication_snapshot_digest(&a);
        assert_eq!(
            stale
                .publish_semantic_catalog(&old, &old_prepared)
                .unwrap()
                .0,
            Outcome::Conflict
        );
        assert_eq!(shared_state(&a), before);
        assert_eq!(
            stale
                .begin_semantic_stage(&old.paths, "stale-begin", true)
                .unwrap(),
            Outcome::Conflict
        );
        assert_eq!(shared_state(&a), before);
        assert_eq!(publication_snapshot_digest(&a), digest);
    }

    #[test]
    fn configured_semantic_publication_faults_rollback_ack_proof_and_hints() {
        let (a, _, _) = same_db_sources();
        let mut store = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *store, &a.raw, "fault");
        for fault in [
            "semantic_publish_rows",
            "semantic_publish_search",
            "semantic_publish_proof",
            "semantic_publish_ack",
            "semantic_publish_head",
        ] {
            let before = shared_state(&a);
            FAIL_EFFECT.with(|point| point.set(Some(fault)));
            assert!(
                store.publish_semantic_catalog(&batch, &prepared).is_err(),
                "{fault}"
            );
            FAIL_EFFECT.with(|point| point.set(None));
            assert_eq!(shared_state(&a), before, "{fault}");
        }
        assert_eq!(
            store.publish_semantic_catalog(&batch, &prepared).unwrap().0,
            Outcome::Written
        );
    }

    #[test]
    fn configured_semantic_publication_replay_is_idempotent() {
        let (a, _, _) = same_db_sources();
        let mut store = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *store, &a.raw, "replay");
        let first = store.publish_semantic_catalog(&batch, &prepared).unwrap();
        let before = shared_state(&a);
        assert_eq!(
            store.publish_semantic_catalog(&batch, &prepared).unwrap(),
            first
        );
        assert_eq!(shared_state(&a), before);
        assert_eq!(
            store.finish_semantic_stage(batch.epoch).unwrap(),
            Outcome::Written
        );
        let (next, prepared) = publication_ready(&mut *store, &a.raw, "replay-again");
        assert_eq!(
            store.publish_semantic_catalog(&next, &prepared).unwrap(),
            first
        );
        assert_eq!(store.session_catalog_generation().unwrap(), first.1);
    }

    #[test]
    fn configured_semantic_replacement_retains_history_and_fences_stale_publisher() {
        use crate::storage::native_events::NativeEventStore;
        use std::io::Write;
        let (a, b, b_raw) = same_db_sources();
        let mut store = owned_semantic(&a.config);
        let mut other = owned_semantic(&b);
        let (batch, prepared) = publication_ready(&mut *store, &a.raw, "original");
        let original = prepared.rows[0].clone();
        assert_eq!(
            store.publish_semantic_catalog(&batch, &prepared).unwrap().0,
            Outcome::Written
        );
        store.finish_semantic_stage(batch.epoch).unwrap();
        let (batch, prepared) = publication_ready(&mut *other, &b_raw, "other");
        assert_eq!(
            other.publish_semantic_catalog(&batch, &prepared).unwrap().0,
            Outcome::Written
        );
        other.finish_semantic_stage(batch.epoch).unwrap();
        let b_before = namespace_state(&a, "source-b");
        let hints_before: i64 = a
            .db()
            .query_row(
                "SELECT count(*) FROM session_catalog_changes WHERE namespace='source-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut raw = fs::OpenOptions::new().append(true).open(&a.raw).unwrap();
        writeln!(raw, "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"continued replacement\"}}]}}}}").unwrap();
        drop(raw);
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config).unwrap();
        let mut cache = EventCache::open_semantic(capability).unwrap();
        cache.ingest(&"b".repeat(64), &a.raw, "codex").unwrap();
        drop(cache);
        let mut stale = owned_semantic(&a.config);
        let (old_batch, old_prepared) = publication_ready(&mut *stale, &a.raw, "stale-replacement");
        let (batch, prepared) = publication_ready(&mut *store, &a.raw, "replacement");
        assert_ne!(prepared.rows[0].sources, original.sources);
        assert!(
            prepared
                .claims
                .iter()
                .any(|(_, revision)| revision.is_some())
        );
        for fault in [
            "semantic_publish_history",
            "semantic_publish_slots",
            "semantic_publish_rows",
            "semantic_publish_search",
            "semantic_publish_proof",
            "semantic_publish_ack",
            "semantic_publish_head",
        ] {
            let before = shared_state(&a);
            FAIL_EFFECT.with(|point| point.set(Some(fault)));
            assert!(
                store.publish_semantic_catalog(&batch, &prepared).is_err(),
                "{fault}"
            );
            FAIL_EFFECT.with(|point| point.set(None));
            assert_eq!(shared_state(&a), before, "{fault}");
        }
        assert_eq!(
            store.publish_semantic_catalog(&batch, &prepared).unwrap().0,
            Outcome::Written
        );
        let db = a.db();
        let retained: String = db.query_row("SELECT metadata FROM session_history_catalog WHERE namespace='source-a' AND session_key=?1",[&original.key],|r|r.get(0)).unwrap();
        let mut expected = original.clone();
        expected.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        assert_eq!(retained, serde_json::to_string(&expected).unwrap());
        assert!(
            store
                .catalog_claims(&batch.paths)
                .unwrap()
                .iter()
                .all(|(_, revision)| revision.is_none())
        );
        assert!(
            db.query_row(
                "SELECT count(*) FROM session_catalog_changes WHERE namespace='source-a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
                > hints_before
        );
        assert_eq!(namespace_state(&a, "source-b"), b_before);
        let before = shared_state(&a);
        let digest = publication_snapshot_digest(&a);
        assert_eq!(
            stale
                .publish_semantic_catalog(&old_batch, &old_prepared)
                .unwrap()
                .0,
            Outcome::Conflict
        );
        assert_eq!(shared_state(&a), before);
        assert_eq!(publication_snapshot_digest(&a), digest);
    }

    #[test]
    fn configured_semantic_scoped_search_effects_replace_and_rollback() {
        let (a, b, b_raw) = same_db_sources();
        let mut other = owned_semantic(&b);
        let (batch, prepared) = publication_ready(&mut *other, &b_raw, "search-other");
        other.publish_semantic_catalog(&batch, &prepared).unwrap();
        other.finish_semantic_stage(batch.epoch).unwrap();
        let b_before = namespace_state(&a, "source-b");
        let mut store = owned_semantic(&a.config);
        let (batch, prepared) = publication_ready(&mut *store, &a.raw, "search-current");
        let key = &prepared.rows[0].key;
        for table in [
            "session_slots",
            "session_slot_projections",
            "session_history_catalog",
            "session_history_projections",
            "session_history_slots",
            "session_history_slot_parts",
            "session_search_coverage",
            "session_search_documents",
            "session_search_memberships",
            "session_search_counts",
        ] {
            assert!(a.db().query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND pk=1 AND [notnull]=1)",[table],|r|r.get::<_,bool>(0)).unwrap(), "{table}");
        }
        let before = shared_state(&a);
        FAIL_EFFECT.with(|point| point.set(Some("semantic_publish_search")));
        assert!(store.publish_semantic_catalog(&batch, &prepared).is_err());
        FAIL_EFFECT.with(|point| point.set(None));
        assert_eq!(shared_state(&a), before);
        store.publish_semantic_catalog(&batch, &prepared).unwrap();
        assert_eq!(namespace_state(&a, "source-b"), b_before);
        let db = a.db();
        for scope in ["current", "retained_history"] {
            let fields:String=db.query_row("SELECT fields FROM session_search_documents WHERE namespace='source-a' AND read_scope=?1 AND session_key=?2",params![scope,key],|r|r.get(0)).unwrap();
            let fields: Vec<String> = serde_json::from_str(&fields).unwrap();
            assert!(fields.contains(&key.to_lowercase()));
            assert_eq!(db.query_row("SELECT count(*) FROM session_search_counts c WHERE namespace='source-a' AND read_scope=?1 AND members<>(SELECT count(*) FROM session_search_memberships p WHERE p.namespace=c.namespace AND p.gram=c.gram AND CASE WHEN ?1='current' THEN p.current_last_ms ELSE p.history_last_ms END IS NOT NULL)",[scope],|r|r.get::<_,i64>(0)).unwrap(),0);
        }
        assert!(
            db.query_row(
                "SELECT count(*) FROM session_search_memberships WHERE namespace='source-a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
                > 0
        );
        let published = shared_state(&a);
        store.publish_semantic_catalog(&batch, &prepared).unwrap();
        assert_eq!(shared_state(&a), published);
        store.finish_semantic_stage(batch.epoch).unwrap();
        let (next, prepared) = publication_ready(&mut *store, &a.raw, "search-repeat");
        store.publish_semantic_catalog(&next, &prepared).unwrap();
        assert_eq!(namespace_state(&a, "source-b"), b_before);
        assert_eq!(db.query_row("SELECT count(*) FROM session_search_documents WHERE namespace='source-a' AND session_key=?1",[key],|r|r.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn configured_semantic_publisher_keeps_unbound_head_refusal() {
        let (a, _, _) = same_db_sources();
        let mut db = a.db();
        let before = shared_state(&a);
        let tx = db.transaction().unwrap();
        assert!(matches!(
            crate::storage::sqlite::publication::transitional_head(&tx),
            Err(StorageError::NamespaceUpgradePending)
        ));
        drop(tx);
        assert_eq!(shared_state(&a), before);
    }

    #[test]
    fn configured_two_sources_same_db_ingest_and_stage_isolated() {
        let (a, b, b_raw) = same_db_sources();
        let mut stores = [owned_semantic(&a.config), owned_semantic(&b)];
        for (i, path) in [&a.raw, &b_raw].into_iter().enumerate() {
            let store = &mut stores[i];
            let paths = vec![path.to_string_lossy().into_owned()];
            let claims = store.catalog_claims(&paths).unwrap();
            assert!(claims[0].1.is_some());
            assert_eq!(store.session_catalog_generation().unwrap(), None);
            assert_eq!(
                store.begin_semantic_stage(&paths, "runtime", true).unwrap(),
                Outcome::Written
            );
            let batch = store.semantic_batch(64).unwrap().unwrap();
            assert_eq!(batch.paths, paths);
            assert_eq!(batch.base_generation, None);
            // The shared test preparation reads the real namespace-bound ledger.
            let prepared = semantic_prepared_path(&mut **store, path, &batch);
            assert_eq!(
                store.record_semantic_batch(&batch, &prepared).unwrap(),
                Outcome::Written
            );
            let identity = store.semantic_batch(64).unwrap().unwrap();
            assert_eq!(
                identity.phase,
                crate::model::relation_stage::Phase::Identity
            );
            let mut prepared = semantic_prepared_path(&mut **store, path, &identity);
            add_primitive_endpoint(&mut prepared);
            assert_eq!(
                store.record_semantic_batch(&identity, &prepared).unwrap(),
                Outcome::Written
            );
            assert_eq!(store.catalog_claims(&paths).unwrap(), claims);
        }
        let db = a.db();
        for (ns, path, native) in [
            ("source-a", &a.raw, "native-session"),
            ("source-b", &b_raw, "native-b"),
        ] {
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM files WHERE namespace=?1 AND path=?2",
                    params![ns, path.to_str().unwrap()],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert!(
                db.query_row(
                    "SELECT count(*) FROM events WHERE namespace=?1",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap()
                    > 0
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM native_usage_sources WHERE namespace=?1 AND source_id=1",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert!(
                db.query_row(
                    "SELECT count(*) FROM meta WHERE namespace=?1 AND key GLOB 'native_*'",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap()
                    > 0
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM storage_event_tables WHERE namespace=?1",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                (TABLES.len() + 1) as i64
            );
            assert_eq!(db.query_row("SELECT count(*) FROM storage_event_operations WHERE namespace=?1 AND state='complete'",[ns],|r|r.get::<_,i64>(0)).unwrap(),1);
            assert_eq!(db.query_row("SELECT count(*) FROM catalog_semantic_inputs WHERE namespace=?1 AND native_id=?2",params![ns,native],|r|r.get::<_,i64>(0)).unwrap(),1);
            assert_eq!(db.query_row("SELECT count(*) FROM catalog_semantic_sources WHERE namespace=?1 AND source_path<>?2",params![ns,path.to_str().unwrap()],|r|r.get::<_,i64>(0)).unwrap(),0);
            assert!(
                db.query_row(
                    "SELECT count(*) FROM catalog_semantic_frontier WHERE namespace=?1",
                    [ns],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap()
                    > 0
            );
        }
    }

    #[test]
    fn configured_two_sources_same_db_rollback_and_reopen_isolated() {
        use crate::storage::native_events::NativeEventStore;
        let (a, b, b_raw) = same_db_sources();
        let mut store = owned_semantic(&a.config);
        let mut other = owned_semantic(&b);
        let paths = vec![a.raw.to_string_lossy().into_owned()];
        store.begin_semantic_stage(&paths, "runtime", true).unwrap();
        other
            .begin_semantic_stage(&[b_raw.to_string_lossy().into_owned()], "runtime", true)
            .unwrap();
        let batch = store.semantic_batch(64).unwrap().unwrap();
        let prepared = semantic_prepared(&mut *store, &a, &batch);
        let before = shared_state(&a);
        FAIL_EFFECT.with(|fault| fault.set(Some("semantic_record")));
        assert!(store.record_semantic_batch(&batch, &prepared).is_err());
        assert_eq!(
            shared_state(&a),
            before,
            "A rollback must preserve the entire shared database, including B"
        );
        drop(store);
        let mut reopened = owned_semantic(&a.config);
        assert_eq!(
            reopened.semantic_batch(64).unwrap().unwrap().epoch,
            batch.epoch
        );
        assert_eq!(
            reopened.record_semantic_batch(&batch, &prepared).unwrap(),
            Outcome::Written
        );
        let before = shared_state(&a);
        FAIL_EFFECT.with(|fault| fault.set(Some("semantic_reduction")));
        assert!(reopened.semantic_batch(64).is_err());
        assert_eq!(shared_state(&a), before);
        drop(other);
        let mut other = owned_semantic(&b);
        assert_eq!(
            other.semantic_batch(64).unwrap().unwrap().paths,
            vec![b_raw.to_string_lossy().into_owned()]
        );
        assert_eq!(
            reopened.semantic_batch(64).unwrap().unwrap().phase,
            crate::model::relation_stage::Phase::Identity
        );
        for (config, path) in [(&a.config, &a.raw), (&b, &b_raw)] {
            let capability =
                crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
            let mut cache = EventCache::open_semantic(capability).unwrap();
            let before = shared_state(&a);
            assert!(
                cache
                    .ingest(&"a".repeat(64), path, "codex")
                    .unwrap()
                    .replayed
            );
            assert_eq!(
                shared_state(&a),
                before,
                "completion replay must not mutate either Source"
            );
        }
    }

    #[test]
    fn configured_two_sources_cross_namespace_capability_refused_zero_writes() {
        let (a, b, b_raw) = same_db_sources();
        let mut store = owned_semantic(&a.config);
        let mut other = owned_semantic(&b);
        store
            .begin_semantic_stage(
                &[a.raw.to_string_lossy().into_owned()],
                "same-runtime",
                true,
            )
            .unwrap();
        other
            .begin_semantic_stage(
                &[b_raw.to_string_lossy().into_owned()],
                "same-runtime",
                true,
            )
            .unwrap();
        let batch = store.semantic_batch(64).unwrap().unwrap();
        let prepared = semantic_prepared(&mut *store, &a, &batch);
        let before = shared_state(&a);
        assert_eq!(
            other.record_semantic_batch(&batch, &prepared).unwrap(),
            Outcome::Conflict
        );
        assert_eq!(shared_state(&a), before);
        a.db().execute("UPDATE storage_namespaces SET admission_generation=admission_generation+1 WHERE namespace='source-b'",[]).unwrap();
        let before = shared_state(&a);
        assert!(other.semantic_batch(64).is_err());
        assert_eq!(shared_state(&a), before);
    }

    #[test]
    fn configured_public_factory_still_refuses_second_source() {
        let (a, b, _) = same_db_sources();
        let before = shared_state(&a);
        assert!(a.config.native_events().is_err());
        assert!(b.native_events().is_err());
        assert!(b.admit_namespace().is_err());
        assert!(crate::storage::sqlite::configured_index::open(&b).is_err());
        assert!(crate::storage::sqlite::open_private_semantic(&b).is_err());
        assert_eq!(shared_state(&a), before);
        assert_eq!(
            crate::storage::sqlite::namespace::WRITER_QUALIFICATION_VERSION,
            0
        );
    }

    fn add_primitive_endpoint(prepared: &mut crate::model::PreparedSources) {
        let source = prepared.indexed_sources[0].1.clone();
        let key = source.native_id.clone();
        let row = crate::model::summary::CatalogRow {
            overview: None,
            trace: None,
            query: None,
            lifecycle: Default::default(),
            key: key.clone(),
            name: key.clone(),
            harness: "codex".into(),
            kind: "codex-run".into(),
            presentation_kind: None,
            native_ids: vec![key.clone()],
            parent: None,
            parent_source: None,
            repo: None,
            branch: None,
            model: String::new(),
            effort: None,
            start: None,
            last: None,
            tokens: [0.0; 3],
            cost: Default::default(),
            sources: vec![source],
        };
        let endpoint = crate::model::relation_stage::Endpoint {
            row,
            paths: Vec::new(),
            nickname: None,
            names: Vec::new(),
            alive: false,
            busy: false,
            waiting: false,
            lane: false,
            role: false,
            state: "recorded".into(),
            turn_lifecycle: None,
            parent: None,
            native_parents: Vec::new(),
            root: key.clone(),
            root_alive: false,
            tokens: Default::default(),
            facts_kind: "codex-run".into(),
            pid: None,
            observed_alive: None,
            recorded_status: None,
            run: None,
            turns_truncated: false,
        };
        prepared.relationships.endpoints.insert(key, endpoint);
    }

    #[test]
    fn configured_semantic_ingest_begin_batch_record_uses_real_claim_and_generation() {
        let f = semantic_fixture("source-a");
        let mut store = crate::storage::sqlite::open_private_semantic(&f.config).unwrap();
        let paths = vec![f.raw.to_string_lossy().into_owned()];
        let claims = store.catalog_claims(&paths).unwrap();
        assert!(claims[0].1.is_some());
        let base = store.session_catalog_generation().unwrap();
        assert_eq!(
            store
                .begin_semantic_stage(&paths, "runtime-a", true)
                .unwrap(),
            Outcome::Written
        );
        let batch = store.semantic_batch(64).unwrap().unwrap();
        assert_eq!(batch.paths, paths);
        assert_eq!(batch.base_generation, base);
        let prepared = semantic_prepared(&mut *store, &f, &batch);
        assert_eq!(prepared.claims, claims);
        assert_eq!(
            store.record_semantic_batch(&batch, &prepared).unwrap(),
            Outcome::Written
        );
        assert_eq!(
            store.catalog_claims(&paths).unwrap(),
            claims,
            "index staging must not ACK publication claims"
        );
        let db = f.db();
        let staged: (String, i64, Option<i64>, Option<String>) = db
            .query_row(
                "SELECT namespace,index_done,claim,ledger FROM catalog_semantic_sources",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(staged.0, "source-a");
        assert_eq!(staged.1, 1);
        assert_eq!(staged.2, claims[0].1);
        assert!(staged.3.is_some());
        assert_eq!(db.query_row("SELECT count(*) FROM catalog_semantic_inputs WHERE namespace='source-a' AND native_id='native-session'", [], |r|r.get::<_,i64>(0)).unwrap(), 1);
        assert!(
            db.query_row(
                "SELECT count(*) FROM catalog_semantic_frontier WHERE namespace='source-a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
                > 0
        );
        assert!(
            store.semantic_batch(64).is_err(),
            "unported frontier reduction must not certify identity"
        );
    }

    #[test]
    fn configured_semantic_namespace_factories_keep_rollback_and_restart_isolated() {
        let a = semantic_fixture("source-a");
        let b = semantic_fixture("source-b");
        let mut a_store = crate::storage::sqlite::open_private_semantic(&a.config).unwrap();
        let mut b_store = crate::storage::sqlite::open_private_semantic(&b.config).unwrap();
        for (store, f) in [(&mut a_store, &a), (&mut b_store, &b)] {
            assert_eq!(
                store
                    .begin_semantic_stage(&[f.raw.to_string_lossy().into_owned()], "runtime", true)
                    .unwrap(),
                Outcome::Written
            );
        }
        let batch = a_store.semantic_batch(64).unwrap().unwrap();
        let prepared = semantic_prepared(&mut *a_store, &a, &batch);
        let before_a = a.state();
        let before_b = b.state();
        FAIL_EFFECT.with(|failure| failure.set(Some("semantic_record")));
        assert!(a_store.record_semantic_batch(&batch, &prepared).is_err());
        assert_eq!(a.state(), before_a);
        assert_eq!(b.state(), before_b);
        drop(a_store);
        let mut reopened = crate::storage::sqlite::open_private_semantic(&a.config).unwrap();
        assert_eq!(
            reopened.semantic_batch(64).unwrap().unwrap().epoch,
            batch.epoch
        );
        assert_eq!(
            reopened.record_semantic_batch(&batch, &prepared).unwrap(),
            Outcome::Written
        );
        assert_eq!(b.state(), before_b);
        let mut wrong = a.config.clone();
        wrong.namespace = b.config.namespace.clone();
        let before = a.state();
        assert!(crate::storage::sqlite::open_private_semantic(&wrong).is_err());
        assert_eq!(a.state(), before);
    }

    #[test]
    fn configured_semantic_stale_record_and_admission_refuse_without_writes() {
        let f = semantic_fixture("source-a");
        let mut store = crate::storage::sqlite::open_private_semantic(&f.config).unwrap();
        let paths = [f.raw.to_string_lossy().into_owned()];
        store.begin_semantic_stage(&paths, "runtime", true).unwrap();
        let batch = store.semantic_batch(64).unwrap().unwrap();
        let mut prepared = semantic_prepared(&mut *store, &f, &batch);
        prepared.base_generation = Some("stale".into());
        let before = f.state();
        assert_eq!(
            store.record_semantic_batch(&batch, &prepared).unwrap(),
            Outcome::Conflict
        );
        assert_eq!(f.state(), before);
        let mut stale = batch.clone();
        stale.base_generation = Some("stale".into());
        assert_eq!(
            store.record_semantic_batch(&stale, &prepared).unwrap(),
            Outcome::Conflict
        );
        assert_eq!(f.state(), before);
        f.db()
            .execute(
                "UPDATE storage_namespaces SET admission_generation=admission_generation+1",
                [],
            )
            .unwrap();
        let before = f.state();
        assert!(store.begin_semantic_stage(&paths, "runtime", true).is_err());
        assert_eq!(f.state(), before);
    }

    fn semantic_fixture(namespace: &str) -> Fixture {
        let f = Fixture::with_named_setup(namespace, PhysicalLayout::CONFIGURED, |_, _| {});
        f.config
            .native_events()
            .unwrap()
            .ingest(&"a".repeat(64), &f.raw, "codex")
            .unwrap();
        let db = f.db();
        // Schema-only private fixture preparation; native rows came from the
        // real configured parser. No staged/source/claim payload is fabricated.
        for table in semantic_stage::STAGE_TABLES {
            assert_eq!(
                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            db.execute_batch(&format!("DROP TABLE {table};")).unwrap();
        }
        for index in [
            "semantic_catalog_parent_key",
            "semantic_catalog_native_paths",
            "semantic_copilot_pending_id",
            "semantic_pending_filename",
        ] {
            db.execute_batch(&format!("DROP INDEX {index};")).unwrap();
        }
        db.execute_batch(semantic_stage::schema(true)).unwrap();
        drop(db);
        f
    }

    fn semantic_prepared(
        store: &mut dyn IndexStore,
        f: &Fixture,
        batch: &crate::model::relation_stage::Batch,
    ) -> crate::model::PreparedSources {
        let prepared = semantic_prepared_path(store, &f.raw, batch);
        assert_eq!(prepared.indexed_sources[0].1.native_id, "native-session");
        prepared
    }

    fn semantic_prepared_path(
        store: &mut dyn IndexStore,
        raw: &Path,
        batch: &crate::model::relation_stage::Batch,
    ) -> crate::model::PreparedSources {
        let (ledger, index) = store.load_file(raw.to_str().unwrap()).unwrap().unwrap();
        let header = index.codex_header.unwrap();
        let id = header.native_id(raw).unwrap();
        let mut source = ledger.source(raw, &id);
        source.native_role = Some(serde_json::json!({"Codex":header.metadata}));
        crate::model::PreparedSources {
            base_generation: batch.base_generation.clone(),
            claims: store.catalog_claims(&batch.paths).unwrap(),
            rows: Vec::new(),
            transcripts: Vec::new(),
            pids: Default::default(),
            lineage_dependencies: Vec::new(),
            message_dependencies: Some(Vec::new()),
            relationships: Default::default(),
            indexed_sources: vec![("codex".into(), source)],
            indexed_native_parents: Vec::new(),
            withdrawals: Vec::new(),
        }
    }

    #[test]
    fn configured_semantic_stage_validates_every_authored_shape_and_cookie_rollback() {
        for composite in [false, true] {
            let mut db = Connection::open_in_memory().unwrap();
            let layout = if composite {
                PhysicalLayout {
                    core: true,
                    metadata: true,
                    catalog: true,
                    native_usage: true,
                    pricing_queue: true,
                    semantic_support: true,
                    semantic_stage: true,
                    ..PhysicalLayout::default()
                }
            } else {
                PhysicalLayout::CONFIGURED
            };
            for ddl in authored_storage_ddl(layout).unwrap() {
                db.execute_batch(ddl).unwrap();
            }
            assert_eq!(semantic_stage::stage_layout(&db).unwrap(), composite);
            for table in semantic_stage::STAGE_TABLES {
                let tx = db.transaction().unwrap();
                tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN unexpected TEXT;"))
                    .unwrap();
                let before = tx.total_changes();
                assert!(semantic_stage::stage_layout(&tx).is_err(), "{table}");
                assert_eq!(tx.total_changes(), before);
                tx.rollback().unwrap();
                assert_eq!(semantic_stage::stage_layout(&db).unwrap(), composite);
            }
            let tx = db.transaction().unwrap();
            tx.execute_batch("DROP INDEX catalog_semantic_message;")
                .unwrap();
            assert!(semantic_stage::stage_layout(&tx).is_err());
            tx.rollback().unwrap();
            assert_eq!(semantic_stage::stage_layout(&db).unwrap(), composite);
        }
    }

    #[test]
    fn configured_semantic_factory_refuses_malformed_stage_before_journal_and_keeps_fences() {
        let f = Fixture::new();
        f.db()
            .execute_batch("ALTER TABLE catalog_semantic_events ADD COLUMN unexpected TEXT;")
            .unwrap();
        let before = f.state();
        assert!(f.config.native_events().is_err());
        assert!(crate::storage::sqlite::open_configured_index(&f.config).is_err());
        assert_eq!(f.state(), before);
        assert!(!f.state().contains_key("storage_source_journal"));
        let db = f.db();
        assert!(
            db.execute(
                "INSERT INTO catalog_semantic_sources(source_path) VALUES('unbound')",
                []
            )
            .is_err()
        );
    }

    #[test]
    fn configured_semantic_input_shapes_and_support_guards_are_required() {
        for table in [
            "session_catalog_invalidations",
            "catalog_backfill_sources",
            "catalog_relocation_candidates",
            "catalog_native_dependencies",
            "session_source_mutations",
        ] {
            let f = Fixture::new();
            f.db()
                .execute_batch(&format!("ALTER TABLE {table} ADD COLUMN unexpected TEXT;"))
                .unwrap();
            let before = f.state();
            assert!(f.config.native_events().is_err(), "{table}");
            assert_eq!(f.state(), before);
            assert!(!f.state().contains_key("storage_source_journal"));
        }
        for table in [
            "catalog_relocation_candidates",
            "catalog_native_dependencies",
            "session_source_mutations",
        ] {
            let f = Fixture::new();
            drop(f.config.native_events().unwrap());
            f.db()
                .execute_batch(&format!("DROP TRIGGER storage_event_guard_{table}_UPDATE;"))
                .unwrap();
            let before = f.state();
            assert!(f.config.native_events().is_err(), "{table}");
            assert_eq!(f.state(), before);
        }
    }

    #[test]
    fn configured_semantic_support_cache_rejects_transient_and_corrupt_layouts() {
        let mut db = Connection::open_in_memory().unwrap();
        for ddl in authored_storage_ddl(PhysicalLayout::CONFIGURED).unwrap() {
            db.execute_batch(ddl).unwrap();
        }
        assert_eq!(
            semantic_stage::support_sql(&db, "single", "scoped").unwrap(),
            "single"
        );
        let tx = db.transaction().unwrap();
        tx.execute_batch("DROP TABLE session_source_mutations; CREATE TABLE session_source_mutations(namespace TEXT NOT NULL,source_path TEXT NOT NULL,evidence TEXT NOT NULL,PRIMARY KEY(namespace,source_path)) STRICT, WITHOUT ROWID;").unwrap();
        assert!(semantic_stage::support_sql(&tx, "single", "scoped").is_err());
        tx.rollback().unwrap();
        assert_eq!(
            semantic_stage::support_sql(&db, "single", "scoped").unwrap(),
            "single"
        );
        let tx = db.transaction().unwrap();
        tx.execute_batch("DROP INDEX catalog_native_dependency; CREATE INDEX catalog_native_dependency ON catalog_native_dependencies(native_id,source_path);").unwrap();
        assert!(semantic_stage::support_sql(&tx, "single", "scoped").is_err());
        tx.rollback().unwrap();
        assert_eq!(
            semantic_stage::support_sql(&db, "single", "scoped").unwrap(),
            "single"
        );
    }

    #[test]
    fn configured_ingest_restart_uncertain_reply_and_operation_identity() {
        let f = Fixture::new();
        let mut writer = f.config.native_events().unwrap();
        FAIL_EFFECT.with(|failure| failure.set(Some("commit_reply")));
        assert_eq!(
            writer.ingest(&"a".repeat(64), &f.raw, "codex"),
            Err(StorageError::CommitUncertain)
        );
        let state = f.state();
        assert_eq!(state["files"].len(), 1);
        assert!(!state["events"].is_empty());
        assert!(!state["meta"].is_empty());
        assert!(!state["catalog_native_dependencies"].is_empty());
        drop(writer);
        // Reconciliation does no Raw I/O and applies no effects a second time.
        fs::remove_file(&f.raw).unwrap();
        let mut restarted = f.config.native_events().unwrap();
        let receipt = restarted.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(receipt.consumed_bytes > 0);
        assert!(receipt.replayed);
        assert_eq!(
            restarted.ingest(&"a".repeat(64), &f.raw, "claude"),
            Err(StorageError::Conflict)
        );
        assert_eq!(state, f.state());
        assert_eq!(
            restarted.ingest(&"a".repeat(64), &f.root.join("other"), "codex"),
            Err(StorageError::Conflict)
        );
        assert!(
            !current(&f.db()).unwrap(),
            "Catalog/unbound writer fence opened"
        );
        let qualified: i64 = f
            .db()
            .query_row(
                "SELECT writer_qualification FROM storage_namespaces",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(qualified, 0);
        let mut foreign = f.config.clone();
        foreign.namespace = LogicalSourceNamespace::new("foreign").unwrap();
        assert!(matches!(
            foreign.native_events(),
            Err(StorageError::NamespaceMismatch)
        ));
        let mut stale = f.config.clone();
        stale.expected_stream_epoch = Some("0".repeat(64));
        assert!(matches!(
            stale.native_events(),
            Err(StorageError::ScopeChanged)
        ));
        assert!(
            f.db()
                .execute("UPDATE files SET title='unbound'", [])
                .is_err()
        );
        assert!(
            connection(&f.config)
                .unwrap()
                .execute(
                    "UPDATE meta SET value='forged' WHERE key='catalog_changes_epoch'",
                    []
                )
                .is_err()
        );
        let custody: String = f
            .db()
            .query_row(
                "SELECT value FROM meta WHERE key='original-custody'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(custody, "preserve-verbatim");
        // Already-open configured writers also reject a replaced epoch.
        f.db().execute_batch("DROP TRIGGER storage_event_guard_meta_UPDATE; UPDATE meta SET value='0000000000000000000000000000000000000000000000000000000000000000' WHERE key='catalog_changes_epoch';").unwrap();
        assert_eq!(
            restarted.ingest(&"a".repeat(64), &f.raw, "codex"),
            Err(StorageError::ScopeChanged)
        );
    }

    #[test]
    fn configured_replay_after_append_returns_original_receipt_and_fresh_id_advances() {
        let f = Fixture::new();
        let mut writer = f.config.native_events().unwrap();
        let first = writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(!first.replayed);
        let before = f.state();
        use std::io::Write;
        fs::OpenOptions::new().append(true).open(&f.raw).unwrap().write_all(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"new work\"}]}}\n").unwrap();
        drop(writer);
        let mut writer = f.config.native_events().unwrap();
        let replay = writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.consumed_bytes, first.consumed_bytes);
        assert_eq!(replay.observed_revision, first.observed_revision);
        assert_eq!(before, f.state());
        assert_eq!(
            writer.ingest(&"a".repeat(64), &f.raw, "claude"),
            Err(StorageError::Conflict)
        );
        let fresh = writer.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
        assert!(!fresh.replayed);
        assert!(fresh.consumed_bytes > replay.consumed_bytes);
        assert!(f.state()["events"].len() > before["events"].len());
        assert_eq!(
            writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap(),
            replay
        );
    }

    #[test]
    fn fresh_operation_on_unchanged_core_has_its_own_durable_completion() {
        let f = Fixture::new();
        let mut writer = f.config.native_events().unwrap();
        let first = writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        let before = f.state();
        let audit = f.db();
        audit.execute_batch("CREATE TABLE tree_write_audit(writes INTEGER); INSERT INTO tree_write_audit VALUES(0);").unwrap();
        for table in [
            "native_tree_headers",
            "native_tree_generations",
            "native_tree_labels",
        ] {
            for operation in ["INSERT", "UPDATE", "DELETE"] {
                audit.execute_batch(&format!("CREATE TRIGGER audit_{table}_{operation} AFTER {operation} ON {table} BEGIN UPDATE tree_write_audit SET writes=writes+1; END;")).unwrap();
            }
        }
        let second = writer.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
        assert_eq!(second.operation_id, "b".repeat(64));
        assert!(!second.replayed);
        assert_eq!(second.consumed_bytes, first.consumed_bytes);
        let revision: Option<i64> = f.db().query_row("SELECT revision FROM session_catalog_invalidations WHERE namespace=?1 AND source_path=?2",params![f.config.namespace.as_str(),f.raw.to_str().unwrap()],|r| r.get(0)).optional().unwrap();
        assert_eq!(second.observed_revision, revision);
        assert_eq!(
            f.state(),
            before,
            "unchanged core must not advance source effects"
        );
        assert_eq!(
            audit
                .query_row("SELECT writes FROM tree_write_audit", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "a verified unchanged Tree must not be rewritten"
        );
        drop(writer);
        fs::remove_file(&f.raw).unwrap();
        let mut restarted = f.config.native_events().unwrap();
        let replay = restarted.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.operation_id, second.operation_id);
        assert_eq!(replay.consumed_bytes, second.consumed_bytes);
    }

    #[test]
    fn native_usage_repair_readiness_rejects_stale_interpretation() {
        let f = Fixture::new();
        let authored: String = f
            .db()
            .query_row("SELECT sql FROM sqlite_schema WHERE name='meta'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut writer = f.config.native_events().unwrap();
        writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        let path = f.raw.to_str().unwrap();
        let mut db = connection(&f.config).unwrap();
        let ledger = ledger_row(&db, path).unwrap().unwrap().1;
        assert!(tree_state::ready(&db, path, &ledger).unwrap());
        // The JSON/version and physical generation still look ready, but the
        // consumer must refuse a summary built under another interpretation.
        db.execute(
            "UPDATE files SET codex_pricing_fingerprint='changed-interpretation'",
            [],
        )
        .unwrap();
        assert!(!tree_state::ready(&db, path, &ledger).unwrap());
        let tx = db.transaction().unwrap();
        let summary: String = tx
            .query_row(
                native_metadata_value(&tx).unwrap(),
                [tree_summary_key(path)],
                |r| r.get(0),
            )
            .unwrap();
        let record: TreeSummaryRecord = serde_json::from_str(&summary).unwrap();
        write_tree_summary(&tx, path, &ledger, Some(&record.summary), true).unwrap();
        assert!(tree_state::ready(&tx, path, &ledger).unwrap());
        tx.commit().unwrap();
        assert!(tree_state::ready(&db, path, &ledger).unwrap());
        drop(db);
        drop(writer);
        let mut restarted = f.config.native_events().unwrap();
        assert!(
            restarted
                .ingest(&"a".repeat(64), &f.raw, "codex")
                .unwrap()
                .replayed
        );
        assert_eq!(
            f.db()
                .query_row("SELECT sql FROM sqlite_schema WHERE name='meta'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            authored,
            "ingest and reopen must preserve the authored metadata shape"
        );
    }

    #[test]
    fn unchanged_fresh_operation_repairs_missing_or_damaged_tree_witness() {
        for damage in [
            "DELETE FROM native_tree_headers",
            "UPDATE native_tree_headers SET ledger='stale'",
            "UPDATE native_tree_labels SET value='damaged' WHERE label='cwd'",
            "UPDATE files SET codex_pricing_fingerprint='changed-interpretation'",
        ] {
            let f = Fixture::new();
            let mut writer = f.config.native_events().unwrap();
            let first = writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
            let before = f.state();
            let generation: i64 = f
                .db()
                .query_row("SELECT projection_id FROM native_tree_headers", [], |r| {
                    r.get(0)
                })
                .unwrap();
            // Fixture-owned corruption, never a recovery API or real store edit.
            connection(&f.config).unwrap().execute(damage, []).unwrap();
            drop(writer);
            let mut writer = f.config.native_events().unwrap();
            let repaired = writer.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
            assert_eq!(repaired.consumed_bytes, first.consumed_bytes);
            let db = connection(&f.config).unwrap();
            let (path, ledger): (String, String) = db
                .query_row(
                    "SELECT source_path,ledger FROM native_tree_headers WHERE projection_id>?1",
                    [generation],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            let actual = ledger_row(&db, &path).unwrap().unwrap().1;
            assert_eq!(ledger, header_ledger(&actual));
            assert!(tree_state::witness(&db, &path, &actual).unwrap().is_some());
            let cwd: String = db.query_row("SELECT value FROM native_tree_labels WHERE projection_id=(SELECT projection_id FROM native_tree_headers WHERE source_path=?1) AND label='cwd'", [&path], |r| r.get(0)).unwrap();
            assert_eq!(cwd, "/sanitized");
            assert_eq!(f.state()["files"], before["files"]);
            assert_eq!(f.state()["events"], before["events"]);
            let repaired_state = f.state();
            writer.ingest(&"c".repeat(64), &f.raw, "codex").unwrap();
            assert_eq!(
                f.state(),
                repaired_state,
                "repaired witness must be reusable"
            );
        }
    }

    #[test]
    fn configured_same_handle_recovers_busy_and_other_operation_refusals() {
        use crate::storage::native_events::NativeEventStore;
        let f = Fixture::new();
        drop(f.config.native_events().unwrap());
        let mut cache = EventCache::open_configured(&f.config).unwrap();
        let lock = f.db();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            cache.ingest(&"a".repeat(64), &f.raw, "codex"),
            Err(StorageError::Busy)
        );
        assert!(cache.event_operation.is_none());
        lock.execute_batch("ROLLBACK").unwrap();
        let receipt = cache.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(!receipt.replayed);
        assert!(cache.event_operation.is_none());
        let error = cache
            .retain(&std::collections::BTreeSet::new(), &mut false)
            .unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<StorageError>()),
            Some(&StorageError::NamespaceUpgradePending)
        );
        let replay = cache.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.consumed_bytes, receipt.consumed_bytes);
        let stamp = ReportedFileStamp {
            dev: 0,
            ino: 0,
            size: 0,
            modified_ns: 0,
        };
        let error = cache.save_runs(&[], &stamp).unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<StorageError>()),
            Some(&StorageError::NamespaceUpgradePending)
        );
        assert!(
            cache
                .ingest(&"a".repeat(64), &f.raw, "codex")
                .unwrap()
                .replayed
        );
    }

    #[test]
    fn configured_each_effect_rolls_back_rows_proofs_revision_and_completion() {
        let f = Fixture::new();
        let mut writer = f.config.native_events().unwrap();
        writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        use std::io::Write;
        fs::OpenOptions::new().append(true).open(&f.raw).unwrap().write_all(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"sanitized reply\"}]}}\n").unwrap();
        let before = f.state();
        for (i, phase) in [
            "relocation",
            "proof_header",
            "proof_sidecar",
            "proof_chunks",
            "proof_parent",
            "proof_launch",
            "file_upsert",
            "proof_tree",
            "events",
            "signals",
            "usage",
            "codex_usage",
            "native_usage",
            "dependencies",
            "semantic_dependencies",
            "archive_metadata",
            "invalidation",
            "mutation_retirement",
            "completion",
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("{:064x}", i + 1);
            FAIL_EFFECT.with(|failure| failure.set(Some(phase)));
            assert_eq!(
                writer.ingest(&id, &f.raw, "codex"),
                Err(StorageError::Unavailable),
                "{phase}"
            );
            assert_eq!(
                FAIL_EFFECT.with(|failure| failure.get()),
                None,
                "effect was not reached: {phase}"
            );
            assert_eq!(before, f.state(), "{phase} escaped rollback");
            let state: String = f
                .db()
                .query_row(
                    "SELECT state FROM storage_event_operations WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(state, "intent", "completion escaped rollback");
        }
        writer.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
        assert_ne!(before, f.state());
    }

    #[test]
    fn configured_ledger_and_observation_cas_are_distinct_and_busy_is_typed() {
        let f = Fixture::new();
        drop(f.config.native_events().unwrap());
        let mut writer = Writer::new(&f.config);
        let path = f.raw.to_string_lossy().into_owned();
        writer.begin(&path, None, &"a".repeat(64), "codex").unwrap();
        let mut observations = f.config.source_observations().unwrap();
        observations
            .discover(std::slice::from_ref(&path), true)
            .unwrap();
        let ledger = Ledger {
            stat: Stat::of(&f.raw).unwrap(),
            offset: 0,
            prefix: Sha256::digest(b"").into(),
            tail: Sha256::digest(b"").into(),
        };
        assert_eq!(
            writer
                .commit(&[(&path, None, None, &ledger, &FileIndex::default())])
                .unwrap_err(),
            StorageError::Conflict
        );
        assert!(writer.ledger(&path).unwrap().is_none());
        let lock = f.db();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            writer.begin(&path, None, &"b".repeat(64), "codex"),
            Err(StorageError::Busy)
        );
        lock.execute_batch("ROLLBACK").unwrap();
        let mut stale = ledger.clone();
        stale.offset = 1;
        assert_eq!(
            writer.begin(&path, Some(&stale), &"c".repeat(64), "codex"),
            Err(StorageError::Conflict)
        );
    }

    #[test]
    fn configured_initial_batch_is_one_transaction_and_replay_is_idempotent() {
        let f = Fixture::new();
        drop(f.config.native_events().unwrap());
        let mut index = crate::storage::sqlite::open_configured_index(&f.config).unwrap();
        let files = ["first", "second"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let raw = f.root.join(name);
                fs::write(&raw, b"").unwrap();
                let path = raw.to_string_lossy().into_owned();
                let ledger = Ledger {
                    stat: Stat::of(&raw).unwrap(),
                    offset: 0,
                    prefix: Sha256::digest(b"").into(),
                    tail: Sha256::digest(b"").into(),
                };
                index
                    .prepare_file(&path, None, &format!("{:064x}", i + 1), "codex")
                    .unwrap();
                (path, ledger, std::sync::Arc::new(FileIndex::default()))
            })
            .collect::<Vec<_>>();
        let before = f.state();
        f.db().execute_batch("CREATE TRIGGER fail_second_event BEFORE INSERT ON files WHEN NEW.path LIKE '%/second' BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;").unwrap();
        assert!(matches!(
            index.commit_initial_files(&files),
            Err(StoreError::Configured(StorageError::Unavailable))
        ));
        assert_eq!(before, f.state());
        f.db()
            .execute_batch("DROP TRIGGER fail_second_event")
            .unwrap();
        assert_eq!(
            index.commit_initial_files(&files).unwrap(),
            Outcome::Written
        );
        let committed = f.state();
        assert_eq!(
            index.commit_initial_files(&files).unwrap(),
            Outcome::Written
        );
        assert_eq!(committed, f.state());
    }
}

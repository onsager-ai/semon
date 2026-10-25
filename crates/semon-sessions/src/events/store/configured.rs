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
        crate::storage::sqlite::trace::validate(db)?;
        crate::storage::sqlite::semantic::validate_owners(db)?;
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
                // Unported consumers remain fenced. The complete scoped trace
                // family is writable only by the fenced semantic publisher.
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
                        "session_trace_headers",
                        "session_trace_turns",
                        "session_trace_edges",
                        "session_trace_bookmarks",
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
            trace: true,
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
        for row in &prepared.rows {
            let trace = row.trace.as_ref().expect("real model trace");
            let witness: serde_json::Value =
                serde_json::from_str(&trace.publication_witness(row)).unwrap();
            for (table, id, expected) in [
                ("session_trace_turns", "turn_id", &witness[1]),
                ("session_trace_edges", "edge_id", &witness[2]),
            ] {
                let actual: Vec<String> = db.prepare(&format!("SELECT metadata FROM {table} WHERE namespace=?1 AND session_key=?2 ORDER BY {id}")).unwrap().query_map(params![config.namespace.as_str(),row.key],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
                let mut expected: Vec<_> = expected.as_array().unwrap().iter().collect();
                expected.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
                assert_eq!(
                    actual,
                    expected
                        .iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                );
            }
            let (generation, source, metadata): (String, String, String) = db.query_row("SELECT generation,source_generation,metadata FROM session_trace_headers WHERE namespace=?1 AND session_key=?2",params![config.namespace.as_str(),row.key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            use sha2::{Digest, Sha256};
            assert_eq!(
                generation,
                format!(
                    "{:x}",
                    Sha256::digest(trace.publication_witness(row).as_bytes())
                )
            );
            assert_eq!(
                source,
                crate::retention::source_generation(&row.sources).unwrap()
            );
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&metadata).unwrap(),
                witness[0]
            );
        }
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
            let mut original = crate::model::prepare_sources(
                &options[i],
                &mut original,
                std::slice::from_ref(&inputs[i]),
                1_790_000_000_000,
            )
            .unwrap();
            let config = if i == 0 { &a.config } else { &b };
            let runtime = crate::native_evidence::query_runtime_generation(&options[i]).unwrap();
            let admission: i64 = a
                .db()
                .query_row(
                    "SELECT admission_generation FROM storage_namespaces WHERE namespace=?1",
                    [config.namespace.as_str()],
                    |r| r.get(0),
                )
                .unwrap();
            let admission = u64::try_from(admission).unwrap();
            use sha2::{Digest, Sha256};
            let expected = format!(
                "source-runtime-v1:{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(
                        "semon-source-runtime-v1",
                        config.namespace.as_str(),
                        admission,
                        config.expected_stream_epoch.as_deref().unwrap(),
                        &runtime,
                    ))
                    .unwrap()
                )
            );
            assert_eq!(
                model.prepared().rows[0].query.as_ref().unwrap()["runtime"],
                expected
            );
            // The only intentional difference from the unbound oracle is its
            // runtime observation contract. All semantic rows and recipes below
            // still compare byte-for-byte, including source-generation fences.
            for row in &mut original.rows {
                row.query.as_mut().unwrap()["runtime"] = serde_json::json!(expected);
            }
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
    fn configured_semantic_trace_and_runtime_equal_native_ids_are_source_owned() {
        use crate::storage::{native_events::NativeEventStore, query::QueryCatalogStore};
        let (a, b, options, mut inputs) = model_sources();
        inputs[1] = inputs[0].clone();
        fs::copy(
            inputs[0].full_path(&options[0]),
            inputs[1].full_path(&options[1]),
        )
        .unwrap();
        fs::copy(
            options[0].facts.as_ref().unwrap(),
            options[1].facts.as_ref().unwrap(),
        )
        .unwrap();
        let runtime = crate::native_evidence::query_runtime_generation(&options[0]).unwrap();
        assert_eq!(
            runtime,
            crate::native_evidence::query_runtime_generation(&options[1]).unwrap()
        );
        let mut witnesses = Vec::new();
        for (i, config) in [&a.config, &b].into_iter().enumerate() {
            let mut cache = EventCache::open_semantic(
                crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
            )
            .unwrap();
            cache
                .ingest(&"a".repeat(64), &inputs[i].full_path(&options[i]), "codex")
                .unwrap();
            let foreign = if i == 0 { "source-b" } else { "source-a" };
            let before = namespace_state(&a, foreign);
            let (batch, model) =
                model_ready(&a, &mut cache, &options[i], &inputs[i], "same-native-id");
            let row = &model.prepared().rows[0];
            assert_eq!(row.key, "model-0");
            witnesses.push(row.query.as_ref().unwrap()["runtime"].clone());
            cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            assert_stored_model(&a, config, &options[i], model.prepared(), row);
            assert_eq!(namespace_state(&a, foreign), before);
            cache.finish_semantic_stage(batch.epoch).unwrap();
            drop(cache);
            let reopened = EventCache::open_semantic(
                crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
            )
            .unwrap();
            let binding = reopened.query_runtime_binding().unwrap().unwrap();
            assert_eq!(binding.witness(&runtime).unwrap(), witnesses[i]);
            let next = crate::storage::QueryRuntimeBinding {
                admission: binding.admission + 1,
                ..binding.clone()
            };
            assert_ne!(next.witness(&runtime).unwrap(), witnesses[i]);
            assert_ne!(binding.witness("changed-runtime").unwrap(), witnesses[i]);
        }
        assert_ne!(
            witnesses[0], witnesses[1],
            "same runtime/native IDs/relative paths cannot share a Source witness"
        );
        let binding = crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config)
            .unwrap()
            .reader_binding()
            .unwrap();
        let port = crate::storage::sqlite::query::SqliteQueryCatalog {
            options: options[0].clone(),
            binding,
        };
        let mut left = 8 * 1024 * 1024;
        let part = port
            .read_cohort(&a.config.namespace, 1_790_000_000_000, &mut left)
            .unwrap();
        a.db().execute("UPDATE storage_namespaces SET admission_generation=admission_generation+2 WHERE namespace='source-a'", []).unwrap();
        assert!(
            port.validate(&a.config.namespace, &part).is_err(),
            "prior admission is fenced after restart/re-admission"
        );
    }

    #[test]
    fn configured_semantic_unsupported_trace_profile_refuses_without_adoption() {
        let (a, _, _, _) = model_sources();
        // This is a current-format but unsupported physical profile containing
        // nonempty unattributable data, not an old Semon upgrade fixture.
        let db = a.db();
        db.execute_batch("ALTER TABLE session_trace_edges RENAME TO preserved_trace_edges; CREATE TABLE session_trace_edges(session_key TEXT NOT NULL,edge_id TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(session_key,edge_id)) STRICT, WITHOUT ROWID; INSERT INTO session_trace_edges VALUES('retained-owner','retained-edge','{}');").unwrap();
        let before = shared_state(&a);
        assert!(matches!(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config),
            Err(StorageError::NamespaceUpgradePending)
        ));
        assert_eq!(
            shared_state(&a),
            before,
            "unsupported facts and all existing source data remain untouched"
        );
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
            "semantic_publish_trace",
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
        for tool in [
            "stalls",
            "find",
            "get_session",
            "read_transcript",
            "unknown_selector",
        ] {
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

    // Worker43 domain gates. Kept separate from S2's appended test reservation.
    mod domain43 {
        use super::*;
        use crate::model::relation_stage::Phase;
        use crate::storage::native_events::NativeEventStore;

        fn ready(
            cache: &mut EventCache,
            f: &Fixture,
            options: &crate::Options,
            inputs: &[crate::Input],
        ) -> (
            crate::model::relation_stage::Batch,
            crate::model::SemanticModelBatch,
        ) {
            let paths = inputs
                .iter()
                .map(|input| input.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, &runtime, true)
                    .unwrap(),
                Outcome::Written
            );
            let mut fault_hits = BTreeMap::<&str, usize>::new();
            for _ in 0..128 {
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if batch.paths.is_empty() {
                    continue;
                }
                let selected = inputs
                    .iter()
                    .filter(|input| {
                        batch
                            .paths
                            .contains(&input.full_path(options).to_string_lossy().into_owned())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(
                    selected.len(),
                    batch.paths.len(),
                    "all discovered Raw inputs must be supplied"
                );
                let model = crate::model::prepare_semantic_batch(
                    options,
                    cache,
                    &selected,
                    &batch,
                    1_790_000_000_000,
                )
                .unwrap();
                let point = match batch.phase {
                    Phase::Indexing => "semantic_record_owners",
                    Phase::Identity => "semantic_record",
                    Phase::Relations => "semantic_record_relations",
                    Phase::Publication => "semantic_model_record",
                };
                let before = shared_state(f);
                FAIL_EFFECT.with(|p| p.set(Some(point)));
                let failed = cache.record_semantic_batch(&batch, model.prepared());
                assert_eq!(
                    FAIL_EFFECT.with(|p| p.get()),
                    None,
                    "fault not reached: {point}, phase {:?}, result {failed:?}",
                    batch.phase
                );
                assert!(failed.is_err(), "{point} must fail after being hit");
                *fault_hits.entry(point).or_default() += 1;
                FAIL_EFFECT.with(|p| p.set(None));
                assert_eq!(
                    shared_state(f),
                    before,
                    "new owner effects roll back with their stage"
                );
                assert_eq!(
                    cache
                        .record_semantic_batch(&batch, model.prepared())
                        .unwrap(),
                    Outcome::Written
                );
                if batch.phase == Phase::Publication {
                    eprintln!("domain44_fault_hits={fault_hits:?}");
                    return (batch, model);
                }
            }
            panic!("domain producer did not reach publication");
        }

        fn publish(
            cache: &mut EventCache,
            f: &Fixture,
            batch: &crate::model::relation_stage::Batch,
            model: &crate::model::SemanticModelBatch,
        ) {
            for point in [
                "semantic_publish_history",
                "semantic_publish_slots",
                "semantic_publish_rows",
                "semantic_publish_owners",
                "semantic_publish_retirement",
                "semantic_publish_trace",
                "semantic_publish_search",
                "semantic_publish_proof",
                "semantic_publish_ack",
                "semantic_publish_head",
            ] {
                let before = shared_state(f);
                FAIL_EFFECT.with(|p| p.set(Some(point)));
                assert!(
                    cache
                        .publish_semantic_catalog(batch, model.prepared())
                        .is_err(),
                    "{point}"
                );
                assert_eq!(
                    FAIL_EFFECT.with(|p| p.get()),
                    None,
                    "publication fault not reached: {point}"
                );
                FAIL_EFFECT.with(|p| p.set(None));
                assert_eq!(
                    shared_state(f),
                    before,
                    "{point} must roll back every family"
                );
            }
            assert_eq!(
                cache
                    .publish_semantic_catalog(batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Written
            );
        }

        fn oracle(
            cache: &mut EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
            prepared: &crate::model::PreparedSources,
        ) {
            // Keep the reference producer independent of the admitted
            // configured store, but give it the ordinary scoped cache that
            // the native producer requires for its source observations.
            let mut unbound = EventCache::open_scoped(&options.cache).unwrap();
            let mut expected =
                crate::model::prepare_sources(options, &mut unbound, inputs, 1_790_000_000_000)
                    .unwrap();
            let binding = cache.query_runtime_binding().unwrap().unwrap();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            use sha2::{Digest, Sha256};
            let witness = format!(
                "source-runtime-v1:{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(
                        "semon-source-runtime-v1",
                        binding.namespace.as_str(),
                        binding.admission,
                        binding.stream_epoch.as_str(),
                        runtime
                    ))
                    .unwrap()
                )
            );
            for row in &mut expected.rows {
                assert_eq!(
                    prepared
                        .rows
                        .iter()
                        .find(|r| r.key == row.key)
                        .unwrap()
                        .query
                        .as_ref()
                        .unwrap()["runtime"],
                    witness
                );
                row.query.as_mut().unwrap()["runtime"] = serde_json::json!(witness);
            }
            assert_eq!(
                serde_json::to_value(&prepared.rows).unwrap(),
                serde_json::to_value(&expected.rows).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&prepared.relationships.relations).unwrap(),
                serde_json::to_value(&expected.relationships.relations).unwrap(),
                "every relation field, including turn witnesses and placements"
            );
            assert_eq!(
                prepared.staged_projection_witness().unwrap(),
                expected.staged_projection_witness().unwrap()
            );
        }

        #[test]
        fn configured_semantic_domain_parent_relations_and_pid_collisions_are_source_owned() {
            let (f, b, options, _) = model_sources();
            let options = &options[0]; // Deliberate identical absolute Raw paths across Sources.
            let inputs = [
                crate::Input {
                    root: crate::InputRoot::Codex,
                    path: "sessions/parent.jsonl".into(),
                },
                crate::Input {
                    root: crate::InputRoot::Codex,
                    path: "sessions/child.jsonl".into(),
                },
            ];
            for (i, input) in inputs.iter().enumerate() {
                let payload = if i == 0 {
                    serde_json::json!({"id":"parent","cwd":"/sanitized"})
                } else {
                    serde_json::json!({"id":"child","cwd":"/sanitized","parent_thread_id":"parent","thread_source":"subagent","agent_path":"root/child","agent_nickname":"child"})
                };
                fs::write(input.full_path(options),format!("{}\n{}\n",serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":payload}),serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:01:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized domain work"}]}}))).unwrap();
            }
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "domain".into(),
                    codex_locks: std::collections::BTreeMap::from([
                        ("parent".into(), 77),
                        ("child".into(), 88),
                    ]),
                    ..Default::default()
                },
            )
            .unwrap();
            for (config, foreign) in [(&f.config, "source-b"), (&b, "source-a")] {
                let mut cache = EventCache::open_semantic(
                    crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
                )
                .unwrap();
                for (i, input) in inputs.iter().enumerate() {
                    cache
                        .ingest(
                            &format!("{:064x}", i + 900),
                            &input.full_path(options),
                            "codex",
                        )
                        .unwrap();
                }
                let before = namespace_state(&f, foreign);
                let (batch, model) = ready(&mut cache, &f, options, &inputs);
                assert!(
                    !model.prepared().relationships.relations.is_empty(),
                    "actual parent-derived relation"
                );
                oracle(&mut cache, options, &inputs, model.prepared());
                let mut next = EventCache::open_semantic(
                    crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
                )
                .unwrap();
                let (next_batch, next_model) = ready(&mut next, &f, options, &inputs);
                let before_stale = shared_state(&f);
                assert_eq!(
                    cache
                        .publish_semantic_catalog(&batch, model.prepared())
                        .unwrap()
                        .0,
                    Outcome::Conflict
                );
                assert_eq!(
                    shared_state(&f),
                    before_stale,
                    "foreign epoch holder makes zero writes across the new families"
                );
                cache = next;
                let (batch, model) = (next_batch, next_model);
                publish(&mut cache, &f, &batch, &model);
                let db = f.db();
                let parent:(String,String,String) = db.query_row("SELECT parent_native,parent_key,root_key FROM catalog_semantic_endpoints WHERE namespace=?1 AND session_key='child'",[config.namespace.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                assert_eq!(parent, ("parent".into(), "parent".into(), "parent".into()));
                for relation in model.prepared().relationships.relations.values() {
                    let stored:String = db.query_row("SELECT metadata FROM catalog_semantic_relations WHERE namespace=?1 AND relation_id=?2",params![config.namespace.as_str(),relation.id],|r|r.get(0)).unwrap();
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
                        serde_json::to_value(relation).unwrap()
                    );
                }
                let pid:String = db.query_row("SELECT native_id FROM catalog_native_dependencies WHERE namespace=?1 AND source_path=?2 AND harness='runtime:pid'",params![config.namespace.as_str(),inputs[0].full_path(options).to_string_lossy()],|r|r.get(0)).unwrap();
                assert_eq!(pid, "77");
                assert_eq!(namespace_state(&f, foreign), before);
                cache.finish_semantic_stage(batch.epoch).unwrap();
            }
        }

        #[test]
        fn configured_semantic_domain_claude_pid_and_continuation_are_source_owned() {
            let (f, b, options, _) = model_sources();
            let options = &options[1];
            let inputs = [
                crate::Input {
                    root: crate::InputRoot::Claude,
                    path: "projects/sanitized/old.jsonl".into(),
                },
                crate::Input {
                    root: crate::InputRoot::Claude,
                    path: "projects/sanitized/new.jsonl".into(),
                },
            ];
            fs::create_dir_all(options.claude_home.join("sessions")).unwrap();
            fs::write(
                options.claude_home.join("sessions/123.json"),
                r#"{"sessionId":"new","status":"idle"}"#,
            )
            .unwrap();
            for (i, input) in inputs.iter().enumerate() {
                let id = if i == 0 { "old" } else { "new" };
                let mut raw = format!(
                    "{}\n",
                    serde_json::json!({"type":"user","sessionId":id,"uuid":format!("user-{id}"),"timestamp":"2026-10-01T00:00:00Z","cwd":"/sanitized","message":{"role":"user","content":"native continuation"}})
                );
                if i == 0 {
                    raw.push_str(&format!("{}\n",serde_json::json!({"type":"continued-in","sessionId":"old","continuedInSessionId":"new","timestamp":"2026-10-01T00:01:00Z"})));
                }
                fs::write(input.full_path(options), raw).unwrap();
            }
            for (config, foreign) in [(&f.config, "source-b"), (&b, "source-a")] {
                let mut cache = EventCache::open_semantic(
                    crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
                )
                .unwrap();
                for (i, input) in inputs.iter().enumerate() {
                    cache
                        .ingest(
                            &format!("{:064x}", i + 1000),
                            &input.full_path(options),
                            "claude",
                        )
                        .unwrap();
                }
                let before = namespace_state(&f, foreign);
                let (batch, model) = ready(&mut cache, &f, options, &inputs);
                assert!(model.prepared().pids.contains(&123));
                assert!(!model.prepared().lineage_dependencies.is_empty());
                oracle(&mut cache, options, &inputs, model.prepared());
                publish(&mut cache, &f, &batch, &model);
                let db = f.db();
                let links:i64=db.query_row("SELECT count(*) FROM catalog_native_dependencies WHERE namespace=?1 AND harness='lineage:claude' AND native_id IN ('old','new')",[config.namespace.as_str()],|r|r.get(0)).unwrap();
                assert!(links > 0);
                let pid:i64=db.query_row("SELECT count(*) FROM catalog_native_dependencies WHERE namespace=?1 AND harness='runtime:pid' AND native_id='123'",[config.namespace.as_str()],|r|r.get(0)).unwrap();
                assert!(pid > 0);
                assert_eq!(namespace_state(&f, foreign), before);
                cache.finish_semantic_stage(batch.epoch).unwrap();
            }
        }

        #[test]
        fn configured_semantic_domain_unsupported_owner_profile_refuses_before_enrollment() {
            let (f, _, _, _) = model_sources();
            f.db().execute_batch("ALTER TABLE catalog_native_dependencies RENAME TO kept_dependencies; CREATE TABLE catalog_native_dependencies(namespace TEXT NOT NULL,source_path TEXT NOT NULL,harness TEXT NOT NULL,native_id INTEGER NOT NULL,PRIMARY KEY(namespace,source_path,harness,native_id)) STRICT, WITHOUT ROWID; INSERT INTO catalog_native_dependencies VALUES('unattributable','/kept','runtime:pid',123);").unwrap();
            let before = shared_state(&f);
            assert!(crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).is_err());
            assert_eq!(
                shared_state(&f),
                before,
                "nonempty unsupported facts are never adopted, rewritten or reset"
            );
        }

        #[test]
        fn configured_semantic_domain_withdrawal_retires_only_owned_current_and_keeps_recipes() {
            let (f, b, options, inputs) = model_sources();
            let options = &options[0];
            let input = &inputs[0];
            for config in [&f.config, &b] {
                let mut cache = EventCache::open_semantic(
                    crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
                )
                .unwrap();
                cache
                    .ingest(&"b".repeat(64), &input.full_path(options), "codex")
                    .unwrap();
                let (batch, model) = ready(&mut cache, &f, options, std::slice::from_ref(input));
                publish(&mut cache, &f, &batch, &model);
                cache.finish_semantic_stage(batch.epoch).unwrap();
            }
            let foreign = namespace_state(&f, "source-b");
            let slots:Vec<String>=f.db().prepare("SELECT metadata FROM session_slots WHERE namespace='source-a' AND session_key='model-0' ORDER BY slot").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            assert!(!slots.is_empty());
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "domain".into(),
                    codex_rollouts: Some(Default::default()),
                    ..Default::default()
                },
            )
            .unwrap();
            let mut cache = EventCache::open_semantic(
                crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap(),
            )
            .unwrap();
            let (batch, model) = ready(&mut cache, &f, options, std::slice::from_ref(input));
            assert!(model.prepared().rows.is_empty());
            assert_eq!(model.prepared().withdrawals.len(), 1);
            let before_rebind = shared_state(&f);
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    // Replacing the facts record changes its physical
                    // authority identity. Hostname is descriptive only.
                    hostname: "domain".into(),
                    codex_rollouts: Some(Default::default()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                cache
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict,
                "replaced facts authority refuses publication without writes"
            );
            assert_eq!(
                shared_state(&f),
                before_rebind,
                "changed withdrawal authority makes zero publication writes"
            );
            drop(cache);
            let mut cache = EventCache::open_semantic(
                crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap(),
            )
            .unwrap();
            let (batch, model) = ready(&mut cache, &f, options, std::slice::from_ref(input));
            publish(&mut cache, &f, &batch, &model);
            let db = f.db();
            assert_eq!(db.query_row("SELECT lifecycle FROM session_catalog WHERE namespace='source-a' AND session_key='model-0'",[],|r|r.get::<_,String>(0)).unwrap(),"retained");
            let retained:String=db.query_row("SELECT metadata FROM session_catalog WHERE namespace='source-a' AND session_key='model-0'",[],|r|r.get(0)).unwrap();
            let retained: crate::model::summary::CatalogRow =
                serde_json::from_str(&retained).unwrap();
            assert_eq!(
                retained.lifecycle,
                crate::model::summary::CatalogLifecycle::Retained
            );
            assert!(!retained.sources.is_empty());
            assert_eq!(db.query_row("SELECT count(*) FROM session_catalog_sources WHERE namespace='source-a' AND session_key='model-0' AND lifecycle='current'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            assert_eq!(db.query_row("SELECT count(*) FROM session_catalog_sources WHERE namespace='source-a' AND session_key='model-0' AND lifecycle='retained'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
            assert_eq!(db.query_row("SELECT count(*) FROM session_trace_headers WHERE namespace='source-a' AND session_key='model-0'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
            let after:Vec<String>=db.prepare("SELECT metadata FROM session_slots WHERE namespace='source-a' AND session_key='model-0' ORDER BY slot").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            assert_eq!(after, slots);
            assert_eq!(db.query_row("SELECT count(*) FROM session_search_documents WHERE namespace='source-a' AND read_scope='current'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            assert!(db.query_row("SELECT count(*) FROM session_search_documents WHERE namespace='source-a' AND read_scope='retained_history'",[],|r|r.get::<_,i64>(0)).unwrap()>0);
            assert_eq!(namespace_state(&f, "source-b"), foreign);
        }
    }

    // Worker44 stored-corruption refusals use real Raw-derived producer output.
    mod domain44 {
        use super::*;
        use crate::model::relation_stage::{Batch, Phase};
        use crate::storage::native_events::NativeEventStore;

        fn fixture() -> (Fixture, EventCache, crate::Options, Vec<crate::Input>) {
            let (f, foreign_config, options, _) = model_sources();
            let options = options[0].clone();
            let inputs = ["parent", "child"]
                .into_iter()
                .map(|name| crate::Input {
                    root: crate::InputRoot::Codex,
                    path: format!("sessions/{name}.jsonl"),
                })
                .collect::<Vec<_>>();
            for (i, input) in inputs.iter().enumerate() {
                let payload = if i == 0 {
                    serde_json::json!({"id":"parent","cwd":"/sanitized"})
                } else {
                    serde_json::json!({"id":"child","cwd":"/sanitized","parent_thread_id":"parent","thread_source":"subagent"})
                };
                fs::write(input.full_path(&options), format!("{}\n{}\n",
                    serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":payload}),
                    serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:01:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"domain44 stored integrity"}]}})
                )).unwrap();
            }
            // Populate Source-b with the same paths/native identities before
            // every negative gate. shared_state must preserve these real rows.
            let mut foreign = EventCache::open_semantic(
                crate::storage::sqlite::semantic::SemanticStaging::admit(&foreign_config).unwrap(),
            )
            .unwrap();
            for (i, input) in inputs.iter().enumerate() {
                foreign
                    .ingest(
                        &format!("{:064x}", i + 4400),
                        &input.full_path(&options),
                        "codex",
                    )
                    .unwrap();
            }
            drop(foreign);
            let mut cache = EventCache::open_semantic(
                crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap(),
            )
            .unwrap();
            for (i, input) in inputs.iter().enumerate() {
                cache
                    .ingest(
                        &format!("{:064x}", i + 4400),
                        &input.full_path(&options),
                        "codex",
                    )
                    .unwrap();
            }
            let paths = inputs
                .iter()
                .map(|input| input.full_path(&options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, &runtime, true)
                    .unwrap(),
                Outcome::Written
            );
            (f, cache, options, inputs)
        }

        fn phase(
            cache: &mut EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
            phase: Phase,
        ) -> (Batch, crate::model::SemanticModelBatch) {
            for _ in 0..128 {
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if batch.paths.is_empty() {
                    continue;
                }
                let selected = inputs
                    .iter()
                    .filter(|input| {
                        batch
                            .paths
                            .contains(&input.full_path(options).to_string_lossy().into_owned())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let model = crate::model::prepare_semantic_batch(
                    options,
                    cache,
                    &selected,
                    &batch,
                    1_790_000_000_000,
                )
                .unwrap();
                if batch.phase == phase {
                    return (batch, model);
                }
                assert_eq!(
                    cache
                        .record_semantic_batch(&batch, model.prepared())
                        .unwrap(),
                    Outcome::Written
                );
            }
            panic!("producer phase not reached");
        }

        // Deliberate stored-corruption injection; production admission and
        // transaction guards remain installed, including Source predicates.
        fn corrupt_db(f: &Fixture) -> Connection {
            let db = f.db();
            db.create_scalar_function(
                "semon_namespace",
                0,
                FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
                |_| Ok("source-a".to_owned()),
            )
            .unwrap();
            for name in [
                "semon_semantic_writer",
                "semon_semantic_publisher",
                "semon_event_writer",
            ] {
                db.create_scalar_function(
                    name,
                    0,
                    FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
                    |_| Ok(1_i64),
                )
                .unwrap();
            }
            db
        }

        #[test]
        fn configured_semantic_domain44_input_order_preserves_exact_claim_witness() {
            let (f, mut cache, options, inputs) = fixture();
            let batch = cache.semantic_batch(64).unwrap().unwrap();
            assert_eq!(batch.phase, Phase::Indexing);
            let before = shared_state(&f);
            let model = crate::model::prepare_semantic_batch(
                &options,
                &mut cache,
                &inputs,
                &batch,
                1_790_000_000_000,
            )
            .unwrap();
            let reversed = inputs.iter().rev().cloned().collect::<Vec<_>>();
            let other = crate::model::prepare_semantic_batch(
                &options,
                &mut cache,
                &reversed,
                &batch,
                1_790_000_000_000,
            )
            .unwrap();
            assert_eq!(model.prepared().claims, other.prepared().claims);
            assert_eq!(
                model
                    .prepared()
                    .claims
                    .iter()
                    .map(|(path, _)| path.clone())
                    .collect::<Vec<_>>(),
                batch.paths
            );
            let duplicate = vec![inputs[0].clone(), inputs[1].clone(), inputs[0].clone()];
            assert!(
                crate::model::prepare_semantic_batch(
                    &options,
                    &mut cache,
                    &duplicate,
                    &batch,
                    1_790_000_000_000
                )
                .is_err()
            );
            assert_eq!(
                shared_state(&f),
                before,
                "canonical preparation and duplicate refusal make zero DB writes"
            );
            assert_eq!(
                cache
                    .record_semantic_batch(&batch, model.prepared())
                    .unwrap(),
                Outcome::Written
            );
        }

        #[test]
        fn configured_semantic_domain44_oversized_relation_prior_refuses_and_rolls_back() {
            let (f, mut cache, options, inputs) = fixture();
            let (batch, model) = phase(&mut cache, &options, &inputs, Phase::Relations);
            let relation = model
                .prepared()
                .relationships
                .relations
                .values()
                .next()
                .expect("real parent relation");
            let text = format!(
                "{}{}",
                " ".repeat(1048577),
                serde_json::to_string(relation).unwrap()
            );
            let db = corrupt_db(&f);
            db.execute("INSERT INTO catalog_semantic_relations(namespace,relation_id,from_key,to_key,metadata) VALUES('source-a',?1,?2,?3,?4)", params![relation.id,relation.from,relation.to,text]).unwrap();
            let before = shared_state(&f);
            assert!(
                cache
                    .record_semantic_batch(&batch, model.prepared())
                    .is_err()
            );
            assert_eq!(
                shared_state(&f),
                before,
                "D1 rolls back endpoint effects before oversized prior refusal"
            );
            let kept: String = db.query_row("SELECT metadata FROM catalog_semantic_relations WHERE namespace='source-a' AND relation_id=?1", [&relation.id], |r| r.get(0)).unwrap();
            assert_eq!(kept, text);
        }

        #[test]
        fn configured_semantic_domain44_endpoint_identity_mismatch_refuses_and_rolls_back() {
            for corruption in ["physical harness", "metadata harness", "metadata key"] {
                let (f, mut cache, options, inputs) = fixture();
                let (batch, model) = phase(&mut cache, &options, &inputs, Phase::Identity);
                let endpoint = model
                    .prepared()
                    .relationships
                    .endpoints
                    .values()
                    .next()
                    .unwrap();
                let mut stored = endpoint.clone();
                if corruption == "metadata harness" {
                    stored.row.harness = "claude".into();
                }
                if corruption == "metadata key" {
                    stored.row.key = "wrong-owner".into();
                }
                let harness = if corruption == "physical harness" {
                    "claude"
                } else {
                    endpoint.row.harness.as_str()
                };
                let text = serde_json::to_string(&stored).unwrap();
                let db = corrupt_db(&f);
                db.execute("INSERT INTO catalog_semantic_endpoints(namespace,session_key,harness,metadata) VALUES('source-a',?1,?2,?3)",params![endpoint.row.key,harness,text]).unwrap();
                let before = shared_state(&f);
                assert!(
                    cache
                        .record_semantic_batch(&batch, model.prepared())
                        .is_err(),
                    "{corruption}"
                );
                assert_eq!(
                    shared_state(&f),
                    before,
                    "D2 keeps every stored family on {corruption}"
                );
                let kept: (String,String) = db.query_row("SELECT harness,metadata FROM catalog_semantic_endpoints WHERE namespace='source-a' AND session_key=?1", [&endpoint.row.key], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
                assert_eq!(kept, (harness.to_owned(), text));
            }
        }

        #[test]
        fn configured_semantic_domain44_missing_root_refuses_and_rolls_back() {
            let (f, mut cache, options, inputs) = fixture();
            let (batch, model) = phase(&mut cache, &options, &inputs, Phase::Publication);
            let db = corrupt_db(&f);
            assert_eq!(db.execute("UPDATE catalog_semantic_endpoints SET root_key='missing-root' WHERE namespace='source-a' AND session_key='child'", []).unwrap(), 1);
            let before = shared_state(&f);
            assert!(
                cache
                    .record_semantic_batch(&batch, model.prepared())
                    .is_err()
            );
            assert_eq!(
                shared_state(&f),
                before,
                "D5 missing root cannot manufacture inactive authority"
            );
            assert_eq!(db.query_row("SELECT root_key FROM catalog_semantic_endpoints WHERE namespace='source-a' AND session_key='child'", [], |r|r.get::<_,String>(0)).unwrap(), "missing-root");
        }

        #[test]
        fn configured_semantic_domain44_missing_replay_owner_refuses_without_writes() {
            for missing in ["parent", "child"] {
                let (f, mut cache, options, inputs) = fixture();
                let (batch, model) = phase(&mut cache, &options, &inputs, Phase::Publication);
                assert_eq!(
                    cache
                        .record_semantic_batch(&batch, model.prepared())
                        .unwrap(),
                    Outcome::Written
                );
                let db = corrupt_db(&f);
                assert_eq!(db.execute("DELETE FROM catalog_semantic_endpoints WHERE namespace='source-a' AND session_key=?1", [missing]).unwrap(), 1);
                let before = shared_state(&f);
                // Missing parent exercises the requested-owner case. Missing
                // child exercises a relation target with a valid staged parent.
                assert!(
                    cache
                        .semantic_replay_page(batch.epoch, &["parent".into()], "")
                        .is_err()
                );
                assert_eq!(
                    shared_state(&f),
                    before,
                    "D6 completed {missing} identity is missing, not unpublished"
                );
                assert_eq!(db.query_row("SELECT count(*) FROM catalog_semantic_endpoints WHERE namespace='source-a' AND session_key=?1", [missing], |r|r.get::<_,i64>(0)).unwrap(), 0);
            }
        }
    }

    #[test]
    fn configured_semantic_s2_real_l1_pin_survives_append_publication_gc_and_reopen() {
        use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};
        use std::io::Write;
        let (a, b, options, inputs) = model_sources();
        for (i, config) in [&a.config, &b].into_iter().enumerate() {
            let capability =
                crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
            let binding = capability.reader_binding().unwrap();
            let mut cache = EventCache::open_semantic(capability).unwrap();
            let path = inputs[i].full_path(&options[i]);
            cache
                .ingest(&"a".repeat(64), &path, inputs[i].root.as_str())
                .unwrap();
            let (batch, model) = model_ready(&a, &mut cache, &options[i], &inputs[i], "s2-real");
            let (outcome, publication) = cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            assert_eq!(outcome, Outcome::Written);
            let key = model.prepared().rows[0].key.clone();
            let runtime = binding.reader_runtime().unwrap();
            let id = runtime.current_generation(&key).unwrap();
            let token = "b".repeat(64);
            let mut lease = runtime
                .selected_generation(&key, &id, &token, 60_000)
                .unwrap();
            assert_eq!(
                lease.snapshot().publication_generation,
                publication.unwrap()
            );
            let first = lease.read_record(&options[i], None, &path, 0).unwrap();
            assert!(first.is_some());
            let first_page = lease
                .transcript_records(
                    &options[i],
                    None,
                    0,
                    8,
                    &crate::SelectedReadControl::default(),
                )
                .unwrap();
            assert_eq!(first_page.snapshot.proof_generation, id);
            cache.finish_semantic_stage(batch.epoch).unwrap();
            let extra = if i == 0 {
                "{\"timestamp\":\"2026-10-01T00:02:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"S2 appended\"}]}}\n"
            } else {
                "{\"type\":\"user\",\"sessionId\":\"model-1\",\"uuid\":\"s2-next\",\"parentUuid\":\"answer\",\"timestamp\":\"2026-10-01T00:02:00Z\",\"message\":{\"role\":\"user\",\"content\":\"S2 appended\"}}\n"
            };
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(extra.as_bytes())
                .unwrap();
            cache
                .ingest(&"c".repeat(64), &path, inputs[i].root.as_str())
                .unwrap();
            let (next, next_model) =
                model_ready(&a, &mut cache, &options[i], &inputs[i], "s2-real");
            assert_eq!(
                cache
                    .publish_semantic_catalog(&next, next_model.prepared())
                    .unwrap()
                    .0,
                Outcome::Written
            );
            let second_id = runtime.current_generation(&key).unwrap();
            assert_ne!(second_id, id);
            let mut second = runtime
                .selected_generation(&key, &second_id, &"9".repeat(64), 60_000)
                .unwrap();
            let second_page = lease
                .transcript_records(
                    &options[i],
                    None,
                    0,
                    8,
                    &crate::SelectedReadControl::default(),
                )
                .unwrap();
            assert_eq!(second_page.snapshot.proof_generation, id);
            assert_eq!(
                serde_json::to_value(&second_page.records).unwrap(),
                serde_json::to_value(&first_page.records).unwrap()
            );
            cache.finish_semantic_stage(next.epoch).unwrap();
            let third_extra = extra
                .replace("S2 appended", "S2 third")
                .replace("s2-next", "s2-third");
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(third_extra.as_bytes())
                .unwrap();
            cache
                .ingest(&"8".repeat(64), &path, inputs[i].root.as_str())
                .unwrap();
            let (third, third_model) =
                model_ready(&a, &mut cache, &options[i], &inputs[i], "s2-real");
            assert_eq!(
                cache
                    .publish_semantic_catalog(&third, third_model.prepared())
                    .unwrap()
                    .0,
                Outcome::Written
            );
            assert_eq!(
                lease.read_record(&options[i], None, &path, 0).unwrap(),
                first
            );
            // No long-lived connection exists in Lease: reopen resolves the
            // durable token and exact old generation, independently of WAL.
            let reopened = runtime
                .selected_generation(&key, &id, &token, 60_000)
                .unwrap();
            assert_eq!(reopened.snapshot().proof_generation, id);
            assert_eq!(
                reopened.read_record(&options[i], None, &path, 0).unwrap(),
                first
            );
            let owner = NativeOwnership::admit(config).unwrap();
            let mut db = SourceJournal::owned(owner).connection().unwrap();
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert_eq!(
                crate::storage::sqlite::proof_generations::collect(
                    &tx,
                    config.namespace.as_str(),
                    1
                )
                .unwrap(),
                0
            );
            tx.commit().unwrap();
            lease.renew(60_000).unwrap();
            lease.release().unwrap();
            assert!(reopened.read_record(&options[i], None, &path, 0).is_err());
            // L1 is retained History. L2 is kept solely by its active reader.
            assert!(
                runtime
                    .selected_generation(&key, &id, &token, 60_000)
                    .is_ok()
            );
            if i == 0 {
                second.release().unwrap();
            } else {
                db.execute(
                    "UPDATE storage_proof_leases SET expires=0 WHERE namespace=?1 AND token=?2",
                    params![config.namespace.as_str(), "9".repeat(64)],
                )
                .unwrap();
                assert!(second.renew(60_000).is_err());
            }
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert_eq!(
                crate::storage::sqlite::proof_generations::collect(
                    &tx,
                    config.namespace.as_str(),
                    1
                )
                .unwrap(),
                1
            );
            tx.commit().unwrap();
            assert!(
                runtime
                    .selected_generation(&key, &second_id, &"9".repeat(64), 60_000)
                    .is_err()
            );
            // The next actual Index mutation must collect L2's retained chunk
            // namespace, while original History L1 remains readable.
            let old_prefix = format!(
                "native_published_chunks:{}:{}:",
                crate::source_carrier::source_hash(path.to_str().unwrap()),
                crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(
                    &second.row().sources[0]
                ))
            );
            let fourth = extra
                .replace("S2 appended", "S2 fourth")
                .replace("s2-next", "s2-fourth");
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(fourth.as_bytes())
                .unwrap();
            cache
                .ingest(&"7".repeat(64), &path, inputs[i].root.as_str())
                .unwrap();
            let remaining: i64 = db
                .query_row(
                    "SELECT count(*) FROM main.meta WHERE namespace=?1 AND key>=?2 AND key<?3",
                    params![
                        config.namespace.as_str(),
                        old_prefix,
                        format!("{old_prefix}~")
                    ],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(remaining, 0);
            let history = runtime
                .selected_generation(&key, &id, &token, 60_000)
                .unwrap();
            assert_eq!(
                history.read_record(&options[i], None, &path, 0).unwrap(),
                first
            );
            assert!(path.exists(), "metadata GC cannot delete Raw");
            assert!(
                !EventCache::path(&options[i].cache).exists(),
                "owned selected reads cannot open Options.cache"
            );
        }
    }

    #[test]
    fn configured_semantic_s2_real_raw_tamper_missing_roots_and_runtime_fence_refuse() {
        use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};
        let (a, _, options, inputs) = model_sources();
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config).unwrap();
        let binding = capability.reader_binding().unwrap();
        let mut cache = EventCache::open_semantic(capability).unwrap();
        let path = inputs[0].full_path(&options[0]);
        cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
        let (batch, model) = model_ready(&a, &mut cache, &options[0], &inputs[0], "s2-refuse");
        cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        let key = &model.prepared().rows[0].key;
        let runtime = binding.reader_runtime().unwrap();
        let id = runtime.current_generation(key).unwrap();
        let token = "d".repeat(64);
        let mut lease = runtime
            .selected_generation(key, &id, &token, 60_000)
            .unwrap();
        assert!(
            runtime
                .selected_generation(key, &id, &"e".repeat(64), 60_001)
                .is_err()
        );
        struct Denied;
        impl crate::SessionSourceReader for Denied {
            fn read_range(
                &self,
                _: &crate::SessionSourceRef,
                _: Option<&str>,
                _: u64,
                _: usize,
            ) -> std::io::Result<crate::SessionSourceRange> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "fixture revoked",
                ))
            }
        }
        assert!(
            lease
                .read_record(
                    &options[0],
                    Some((&Denied, crate::SessionSourceReadMode::ProviderOnly)),
                    &path,
                    0
                )
                .is_err()
        );
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let control = crate::SelectedReadControl::new(
            std::time::Instant::now() + std::time::Duration::from_secs(20),
            cancelled,
        );
        assert!(
            lease
                .read_record_with_control(&options[0], None, &path, 0, &control)
                .is_err()
        );
        let original = fs::read(&path).unwrap();
        let mut tampered = original.clone();
        tampered[20] ^= 1;
        fs::write(&path, tampered).unwrap();
        assert!(lease.read_record(&options[0], None, &path, 0).is_err());
        fs::write(&path, &original[..original.len() / 2]).unwrap();
        assert!(lease.read_record(&options[0], None, &path, 0).is_err());
        fs::write(&path, &original).unwrap();
        let replacement = path.with_extension("replacement");
        fs::write(&replacement, &original).unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(lease.read_record(&options[0], None, &path, 0).is_err());
        fs::remove_file(&path).unwrap();
        assert!(lease.read_record(&options[0], None, &path, 0).is_err());
        let mut rebound = options[0].clone();
        rebound.codex_home = a.root.join("foreign-root");
        assert!(lease.read_record(&rebound, None, &path, 0).is_err());
        let successor = binding.reader_runtime().unwrap();
        assert!(runtime.current_generation(key).is_err());
        assert!(lease.renew(60_000).is_err());
        assert!(
            successor
                .selected_generation(key, &id, &token, 60_000)
                .is_err()
        );
    }

    #[test]
    fn configured_semantic_s2_generation_ref_failure_rolls_back_and_replays_durably() {
        use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};
        let (a, _, options, inputs) = model_sources();
        let mut cache = EventCache::open_semantic(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config).unwrap(),
        )
        .unwrap();
        let path = inputs[0].full_path(&options[0]);
        cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
        let (batch, model) = model_ready(&a, &mut cache, &options[0], &inputs[0], "s2-fault");
        let before = shared_state(&a);
        FAIL_EFFECT.with(|point| point.set(Some("semantic_publish_generation_refs")));
        assert!(
            cache
                .publish_semantic_catalog(&batch, model.prepared())
                .is_err()
        );
        FAIL_EFFECT.with(|point| point.set(None));
        assert_eq!(shared_state(&a), before);
        let (outcome, head) = cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        assert_eq!(outcome, Outcome::Written);
        let committed = shared_state(&a);
        // Simulate a caller that lost the ACK: durable publication identity
        // reconciles exactly rather than depending on any lease expiry.
        assert_eq!(
            cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap(),
            (Outcome::Written, head.clone())
        );
        assert_eq!(shared_state(&a), committed);
        drop(cache);
        let binding = crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config)
            .unwrap()
            .reader_binding()
            .unwrap();
        let runtime = binding.reader_runtime().unwrap();
        let key = &model.prepared().rows[0].key;
        let id = runtime.current_generation(key).unwrap();
        let lease = runtime
            .selected_generation(key, &id, &"f".repeat(64), 60_000)
            .unwrap();
        assert_eq!(Some(lease.snapshot().publication_generation.clone()), head);
        assert!(
            lease
                .read_record(&options[0], None, &path, 0)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn configured_semantic_s2_proof_witness_and_physical_contract_refuse_tampering() {
        use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};
        let (a, b, options, inputs) = model_sources();
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::admit(&a.config).unwrap();
        let binding = capability.reader_binding().unwrap();
        let mut cache = EventCache::open_semantic(capability).unwrap();
        let path = inputs[0].full_path(&options[0]);
        cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
        let (batch, model) = model_ready(&a, &mut cache, &options[0], &inputs[0], "s2-proof");
        cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        let key = &model.prepared().rows[0].key;
        let runtime = binding.reader_runtime().unwrap();
        let id = runtime.current_generation(key).unwrap();
        let lease = runtime
            .selected_generation(key, &id, &"a".repeat(64), 60_000)
            .unwrap();
        let db = SourceJournal::owned(NativeOwnership::admit(&a.config).unwrap())
            .connection()
            .unwrap();
        db.execute_batch("DROP TRIGGER storage_semantic_proof_immutable_update; DROP TRIGGER storage_semantic_proof_immutable_delete;")
            .unwrap();
        db.execute(
            "UPDATE storage_semantic_publications SET proof_digest=?2 WHERE namespace=?1",
            params![a.config.namespace.as_str(), "0".repeat(64)],
        )
        .unwrap();
        assert!(lease.read_record(&options[0], None, &path, 0).is_err());
        db.execute(
            "DELETE FROM storage_semantic_publications WHERE namespace=?1",
            [a.config.namespace.as_str()],
        )
        .unwrap();
        assert!(
            runtime
                .selected_generation(key, &id, &"b".repeat(64), 60_000)
                .is_err()
        );

        let initialized = crate::storage::sqlite::semantic::SemanticStaging::admit(&b).unwrap();
        let _ = EventCache::open_semantic(initialized).unwrap();
        let db = SourceJournal::owned(NativeOwnership::admit(&b).unwrap())
            .connection()
            .unwrap();
        db.execute_batch("DROP TRIGGER storage_proof_generation_immutable; CREATE TRIGGER storage_proof_generation_immutable BEFORE UPDATE ON storage_proof_generations BEGIN SELECT 1; END;")
            .unwrap();
        let refused = match crate::storage::sqlite::semantic::SemanticStaging::admit(&b) {
            Err(_) => true,
            Ok(capability) => EventCache::open_semantic(capability).is_err(),
        };
        assert!(refused);
        db.execute_batch("DROP TRIGGER storage_proof_generation_immutable; CREATE TRIGGER storage_proof_generation_immutable BEFORE UPDATE ON storage_proof_generations BEGIN SELECT RAISE(ABORT,'immutable proof generation'); END; DROP INDEX storage_proof_lease_generation; CREATE INDEX storage_proof_lease_generation ON storage_proof_leases(namespace,id);")
            .unwrap();
        let refused = match crate::storage::sqlite::semantic::SemanticStaging::admit(&b) {
            Err(_) => true,
            Ok(capability) => EventCache::open_semantic(capability).is_err(),
        };
        assert!(refused);
    }

    #[test]
    fn configured_semantic_s2_equal_native_ids_are_namespace_owned_and_leases_bounded() {
        use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};
        let (a, b, options, mut inputs) = model_sources();
        inputs[1] = inputs[0].clone();
        fs::copy(
            inputs[0].full_path(&options[0]),
            inputs[1].full_path(&options[1]),
        )
        .unwrap();
        let mut runtimes = Vec::new();
        let mut ids = Vec::new();
        let mut keys = Vec::new();
        for (i, config) in [&a.config, &b].into_iter().enumerate() {
            let capability =
                crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
            let binding = capability.reader_binding().unwrap();
            let mut cache = EventCache::open_semantic(capability).unwrap();
            cache
                .ingest(&"a".repeat(64), &inputs[i].full_path(&options[i]), "codex")
                .unwrap();
            let (batch, model) =
                model_ready(&a, &mut cache, &options[i], &inputs[i], "s2-isolation");
            cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            let runtime = binding.reader_runtime().unwrap();
            let key = model.prepared().rows[0].key.clone();
            ids.push(runtime.current_generation(&key).unwrap());
            keys.push(key);
            runtimes.push(runtime);
        }
        assert_eq!(keys[0], keys[1]);
        assert_ne!(ids[0], ids[1]);
        let foreign = namespace_state(&a, "source-a");
        assert!(
            runtimes[1]
                .selected_generation(&keys[1], &ids[0], &"b".repeat(64), 60_000)
                .is_err()
        );
        assert_eq!(namespace_state(&a, "source-a"), foreign);
        let mut leases = Vec::new();
        for i in 0..64 {
            leases.push(
                runtimes[0]
                    .selected_generation(&keys[0], &ids[0], &format!("{i:064x}"), 60_000)
                    .unwrap(),
            );
        }
        assert!(matches!(
            runtimes[0].selected_generation(&keys[0], &ids[0], &"f".repeat(64), 60_000),
            Err(StorageError::Busy)
        ));
        // The same token in another namespace neither conflicts nor grants a
        // foreign source. Capacity remains source-local as well.
        let other = runtimes[1]
            .selected_generation(&keys[1], &ids[1], &format!("{:064x}", 0), 60_000)
            .unwrap();
        assert!(
            other
                .read_record(&options[0], None, &inputs[1].full_path(&options[1]), 0)
                .is_err()
        );
        assert!(
            other
                .read_record(&options[1], None, &inputs[1].full_path(&options[1]), 0)
                .unwrap()
                .is_some()
        );
        leases[0].release().unwrap();
        assert!(
            runtimes[0]
                .selected_generation(&keys[0], &ids[0], &"f".repeat(64), 60_000)
                .is_ok()
        );
    }
    mod domain52 {
        use super::*;
        use crate::model::relation_stage::{Batch, Phase};
        use crate::storage::native_events::NativeEventStore;

        fn write(options: &crate::Options, input: &crate::Input, id: &str, parent: Option<&str>) {
            let mut payload = serde_json::json!({"id":id,"cwd":"/sanitized"});
            if let Some(parent) = parent {
                payload["parent_thread_id"] = serde_json::json!(parent);
                payload["thread_source"] = serde_json::json!("subagent");
            }
            fs::write(input.full_path(options), format!("{}\n{}\n",
                serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":payload}),
                serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:01:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"domain52 lifecycle"}]}})
            )).unwrap();
        }
        fn open(config: &StoreConfig) -> EventCache {
            EventCache::open_semantic(
                crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
            )
            .unwrap()
        }
        fn ready(
            cache: &mut EventCache,
            f: &Fixture,
            options: &crate::Options,
            inputs: &[crate::Input],
        ) -> (Batch, crate::model::SemanticModelBatch) {
            let paths = inputs
                .iter()
                .map(|i| i.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, &runtime, true)
                    .unwrap(),
                Outcome::Written
            );
            let before = shared_state(f);
            FAIL_EFFECT.with(|p| p.set(Some("semantic_lifecycle_enroll")));
            assert!(cache.semantic_batch(64).is_err());
            assert_eq!(
                FAIL_EFFECT.with(|p| p.get()),
                None,
                "enrollment effect must be hit"
            );
            assert_eq!(
                shared_state(f),
                before,
                "closure enrollment rolls back every family"
            );
            for _ in 0..128 {
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if batch.paths.is_empty() {
                    continue;
                }
                let selected = inputs
                    .iter()
                    .filter(|i| {
                        batch
                            .paths
                            .contains(&i.full_path(options).to_string_lossy().into_owned())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(
                    selected.len(),
                    batch.paths.len(),
                    "no discovered owner may be omitted"
                );
                let model = cache
                    .prepare_semantic_model_batch(options, &selected, &batch, 1_790_000_000_000)
                    .unwrap();
                if batch.phase == Phase::Publication {
                    return (batch, model);
                }
            }
            panic!("lifecycle producer failed to reach publication")
        }
        fn publish(
            cache: &mut EventCache,
            f: &Fixture,
            batch: &Batch,
            model: &crate::model::SemanticModelBatch,
        ) {
            for point in [
                "semantic_publish_owners",
                "semantic_publish_retirement",
                "semantic_publish_search",
                "semantic_publish_generation_refs",
                "semantic_publish_ack",
            ] {
                let before = shared_state(f);
                FAIL_EFFECT.with(|p| p.set(Some(point)));
                let result = cache.publish_semantic_catalog(batch, model.prepared());
                assert!(result.is_err(), "{point}");
                let reason = result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                let refusal = if reason.starts_with("semantic_")
                    && reason.len() < 128
                    && reason.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                {
                    reason.as_str()
                } else {
                    "injected_or_other"
                };
                assert_eq!(
                    FAIL_EFFECT.with(|p| p.get()),
                    None,
                    "unconsumed fault {point}; refusal={refusal}; max_slots={}",
                    model
                        .prepared()
                        .transcripts
                        .iter()
                        .map(|p| p.slots.len())
                        .max()
                        .unwrap_or(0)
                );
                assert_eq!(shared_state(f), before, "whole-DB rollback: {point}");
            }
            let first = cache
                .publish_semantic_catalog(batch, model.prepared())
                .unwrap();
            assert_eq!(first.0, Outcome::Written);
            let before = shared_state(f);
            assert_eq!(
                cache
                    .publish_semantic_catalog(batch, model.prepared())
                    .unwrap(),
                first
            );
            assert_eq!(shared_state(f), before, "same ACK is byte-identical");
        }
        fn oracle(
            cache: &EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
            prepared: &crate::model::PreparedSources,
        ) {
            let mut reference = EventCache::open_scoped(&options.cache).unwrap();
            let mut expected =
                crate::model::prepare_sources(options, &mut reference, inputs, 1_790_000_000_000)
                    .unwrap();
            let binding = cache.query_runtime_binding().unwrap().unwrap();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            use sha2::{Digest, Sha256};
            let runtime = format!(
                "source-runtime-v1:{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(
                        "semon-source-runtime-v1",
                        binding.namespace.as_str(),
                        binding.admission,
                        binding.stream_epoch.as_str(),
                        runtime
                    ))
                    .unwrap()
                )
            );
            for row in &mut expected.rows {
                row.query.as_mut().unwrap()["runtime"] = serde_json::json!(runtime);
            }
            assert_eq!(
                serde_json::to_value(&prepared.rows).unwrap(),
                serde_json::to_value(&expected.rows).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&prepared.relationships.relations).unwrap(),
                serde_json::to_value(&expected.relationships.relations).unwrap(),
                "every relation field"
            );
            assert_eq!(
                serde_json::to_value(
                    prepared
                        .transcripts
                        .iter()
                        .map(|p| (&p.key, &p.slots, p.repair))
                        .collect::<Vec<_>>()
                )
                .unwrap(),
                serde_json::to_value(
                    expected
                        .transcripts
                        .iter()
                        .map(|p| (&p.key, &p.slots, p.repair))
                        .collect::<Vec<_>>()
                )
                .unwrap()
            );
            assert_eq!(prepared.pids, expected.pids);
            assert_eq!(prepared.lineage_dependencies, expected.lineage_dependencies);
            // Ordinary production has no configured epoch, prior Current
            // snapshot or lifecycle authority. Those witnesses are checked
            // independently below, not normalized into invented reference data.
        }
        fn rows(f: &Fixture, table: &str, key: &str) -> Vec<Vec<rusqlite::types::Value>> {
            let db = f.db();
            let mut q = db.prepare(&format!("SELECT * FROM {table} WHERE namespace='source-a' AND session_key=?1 ORDER BY 1,2,3")).unwrap();
            let n = q.column_count();
            q.query_map([key], |r| (0..n).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        }

        #[test]
        fn configured_semantic_domain52_identity_pid_and_parent_transition_preserves_custody() {
            let (f, foreign_config, options, _) = model_sources();
            let options = &options[0];
            let inputs = [
                crate::Input {
                    root: crate::InputRoot::Codex,
                    path: "sessions/parent.jsonl".into(),
                },
                crate::Input {
                    root: crate::InputRoot::Codex,
                    path: "sessions/child.jsonl".into(),
                },
            ];
            write(options, &inputs[0], "parent", None);
            write(options, &inputs[1], "child", Some("parent"));
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "domain52".into(),
                    codex_locks: BTreeMap::from([("parent".into(), 77), ("child".into(), 88)]),
                    ..Default::default()
                },
            )
            .unwrap();
            for config in [&f.config, &foreign_config] {
                let mut cache = open(config);
                for (i, input) in inputs.iter().enumerate() {
                    cache
                        .ingest(
                            &format!("{:064x}", 5200 + i),
                            &input.full_path(options),
                            "codex",
                        )
                        .unwrap();
                }
                let (batch, model) = ready(&mut cache, &f, options, &inputs);
                publish(&mut cache, &f, &batch, &model);
                cache.finish_semantic_stage(batch.epoch).unwrap();
            }
            use crate::storage::proof_lifetime::ProofReaderLease;
            let capability =
                crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
            let reader = capability
                .reader_binding()
                .unwrap()
                .reader_runtime()
                .unwrap();
            let old_id = reader.current_generation("parent").unwrap();
            let lease = reader
                .selected_generation("parent", &old_id, &"6".repeat(64), 60_000)
                .unwrap();
            assert!(
                lease
                    .read_record(options, None, &inputs[0].full_path(options), 0)
                    .unwrap()
                    .is_some()
            );
            let foreign = namespace_state(&f, "source-b");
            let retained = [
                "session_history_catalog",
                "session_history_projections",
                "session_history_slots",
                "session_slot_projections",
                "session_slots",
                "session_trace_headers",
                "session_trace_edges",
            ]
            .map(|table| (table, rows(&f, table, "parent")));
            let old: String = f.db().query_row("SELECT metadata FROM session_catalog WHERE namespace='source-a' AND session_key='parent'",[],|r|r.get(0)).unwrap();
            write(options, &inputs[0], "successor", None);
            write(options, &inputs[1], "child", Some("successor"));
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "domain52".into(),
                    codex_locks: BTreeMap::from([("successor".into(), 99), ("child".into(), 88)]),
                    ..Default::default()
                },
            )
            .unwrap();
            let mut cache = open(&f.config);
            for (i, input) in inputs.iter().enumerate() {
                cache
                    .ingest(
                        &format!("{:064x}", 5300 + i),
                        &input.full_path(options),
                        "codex",
                    )
                    .unwrap();
            }
            let (batch, model) = ready(&mut cache, &f, options, &inputs);
            oracle(&cache, options, &inputs, model.prepared());
            assert_eq!(model.prepared().relationships.retired_owners.len(), 1);
            assert_eq!(
                serde_json::to_string(&model.prepared().relationships.retired_owners[0]).unwrap(),
                old
            );
            assert!(
                model
                    .prepared()
                    .relationships
                    .relations
                    .values()
                    .any(|r| r.from == "successor" && r.to.as_deref() == Some("child"))
            );
            publish(&mut cache, &f, &batch, &model);
            let db = f.db();
            assert_eq!(db.query_row("SELECT lifecycle FROM session_catalog WHERE namespace='source-a' AND session_key='parent'",[],|r|r.get::<_,String>(0)).unwrap(),"retained");
            assert_eq!(db.query_row("SELECT parent_key FROM session_catalog WHERE namespace='source-a' AND session_key='child'",[],|r|r.get::<_,String>(0)).unwrap(),"successor");
            for (table, before) in retained {
                assert_eq!(rows(&f, table, "parent"), before, "custody {table}");
            }
            assert_eq!(db.query_row("SELECT count(*) FROM session_search_documents WHERE namespace='source-a' AND session_key='parent' AND read_scope='current'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            assert!(db.query_row("SELECT count(*) FROM session_search_documents WHERE namespace='source-a' AND session_key='parent' AND read_scope='retained_history'",[],|r|r.get::<_,i64>(0)).unwrap()>0);
            assert_eq!(db.query_row("SELECT native_id FROM catalog_native_dependencies WHERE namespace='source-a' AND source_path=?1 AND harness='runtime:pid'",[inputs[0].full_path(options).to_string_lossy()],|r|r.get::<_,String>(0)).unwrap(),"99");
            assert_eq!(namespace_state(&f, "source-b"), foreign);
            assert!(
                lease
                    .read_record(options, None, &inputs[0].full_path(options), 0)
                    .is_err(),
                "replaced Raw cannot serve the old identity"
            );
            let reopened = reader
                .selected_generation("parent", &old_id, &"6".repeat(64), 60_000)
                .unwrap();
            assert!(
                reopened
                    .read_record(options, None, &inputs[0].full_path(options), 0)
                    .is_err(),
                "reopen preserves old proof and refuses changed bytes"
            );
            let before = shared_state(&f);
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "domain52".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                cache
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict
            );
            assert_eq!(
                shared_state(&f),
                before,
                "stale ACK authority has zero effects"
            );
        }

        #[test]
        fn configured_semantic_domain52_path_move_and_l1_lease_keep_original_prefix() {
            use crate::storage::proof_lifetime::ProofReaderLease;
            let (f, _, options, inputs) = model_sources();
            let options = &options[0];
            let old = &inputs[0];
            let moved = crate::Input {
                root: crate::InputRoot::Codex,
                path: "sessions/moved.jsonl".into(),
            };
            let capability =
                crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
            let binding = capability.reader_binding().unwrap();
            let mut cache = EventCache::open_semantic(capability).unwrap();
            cache
                .ingest(&"1".repeat(64), &old.full_path(options), "codex")
                .unwrap();
            let (batch, model) = ready(&mut cache, &f, options, std::slice::from_ref(old));
            publish(&mut cache, &f, &batch, &model);
            let runtime = binding.reader_runtime().unwrap();
            let id = runtime.current_generation("model-0").unwrap();
            let mut lease = runtime
                .selected_generation("model-0", &id, &"2".repeat(64), 60_000)
                .unwrap();
            let prefix = lease
                .read_record(options, None, &old.full_path(options), 0)
                .unwrap();
            assert!(prefix.is_some());
            cache.finish_semantic_stage(batch.epoch).unwrap();
            // Keep the independently verifiable old Raw carrier. If it were
            // absent, the old lease must refuse before returning any bytes.
            fs::copy(old.full_path(options), moved.full_path(options)).unwrap();
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "machine-0".into(),
                    codex_rollouts: Some(std::collections::BTreeSet::from([moved.path.clone()])),
                    ..Default::default()
                },
            )
            .unwrap();
            cache
                .ingest(&"3".repeat(64), &moved.full_path(options), "codex")
                .unwrap();
            let cohort = [old.clone(), moved.clone()];
            let (next, model) = ready(&mut cache, &f, options, &cohort);
            assert_eq!(model.prepared().rows[0].sources.len(), 1);
            assert_eq!(
                model.prepared().rows[0].sources[0].path,
                moved.full_path(options)
            );
            publish(&mut cache, &f, &next, &model);
            assert_eq!(f.db().query_row("SELECT lifecycle FROM session_catalog_sources WHERE namespace='source-a' AND session_key='model-0' AND source_path=?1",[old.full_path(options).to_string_lossy()],|r|r.get::<_,String>(0)).unwrap(),"retained");
            let owner = NativeOwnership::admit(&f.config).unwrap();
            let mut db = SourceJournal::owned(owner).connection().unwrap();
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            crate::storage::sqlite::proof_generations::collect(&tx, "source-a", 64).unwrap();
            tx.commit().unwrap();
            let before = lease.read_record(options, None, &old.full_path(options), 0);
            match before {
                Ok(bytes) => assert_eq!(bytes, prefix),
                Err(error) => assert!(!error.to_string().is_empty(), "precise pre-byte refusal"),
            }
            let reopened = runtime
                .selected_generation("model-0", &id, &"2".repeat(64), 60_000)
                .unwrap();
            match reopened.read_record(options, None, &old.full_path(options), 0) {
                Ok(bytes) => assert_eq!(bytes, prefix),
                Err(error) => assert!(!error.to_string().is_empty(), "precise pre-byte refusal"),
            }
            lease.release().unwrap();
        }
        #[test]
        fn configured_semantic_domain52_foreign_epoch_and_changed_authority_zero_effects() {
            let (f, _, options, inputs) = model_sources();
            let options = &options[0];
            let input = &inputs[0];
            let mut original = open(&f.config);
            original
                .ingest(&"4".repeat(64), &input.full_path(options), "codex")
                .unwrap();
            let (batch, model) = ready(&mut original, &f, options, std::slice::from_ref(input));
            publish(&mut original, &f, &batch, &model);
            original.finish_semantic_stage(batch.epoch).unwrap();
            write(options, input, "replacement", None);
            original
                .ingest(&"5".repeat(64), &input.full_path(options), "codex")
                .unwrap();
            let (old_batch, old_model) =
                ready(&mut original, &f, options, std::slice::from_ref(input));
            let mut successor = open(&f.config);
            let (batch, model) = ready(&mut successor, &f, options, std::slice::from_ref(input));
            let before = shared_state(&f);
            assert_eq!(
                original
                    .publish_semantic_catalog(&old_batch, old_model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict
            );
            assert_eq!(
                shared_state(&f),
                before,
                "old holder cannot publish lifecycle changes"
            );
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "machine-0".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                successor
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict
            );
            assert_eq!(
                shared_state(&f),
                before,
                "same content with replaced Facts identity is stale"
            );
        }

        #[test]
        fn configured_semantic_domain52_high_degree_raw_component_refuses_without_partial_publication()
         {
            fn published_state(f: &Fixture) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
                namespace_state(f, "source-a")
                    .into_iter()
                    .filter(|(table, _)| {
                        (table.starts_with("session_") && table != "session_catalog_invalidations")
                            || table.starts_with("storage_proof_")
                            || table == "storage_semantic_publications"
                    })
                    .collect()
            }

            let (f, foreign_config, options, foreign_inputs) = model_sources();
            // Give the foreign namespace real published state to preserve.
            {
                let mut foreign = open(&foreign_config);
                let input = &foreign_inputs[1];
                foreign
                    .ingest(&"7".repeat(64), &input.full_path(&options[1]), "claude")
                    .unwrap();
                let (batch, model) =
                    ready(&mut foreign, &f, &options[1], std::slice::from_ref(input));
                publish(&mut foreign, &f, &batch, &model);
                assert_eq!(
                    foreign.finish_semantic_stage(batch.epoch).unwrap(),
                    Outcome::Written
                );
            }
            let options = &options[0];
            let mut cache = open(&f.config);
            let inputs = (0..65)
                .map(|i| crate::Input {
                    root: crate::InputRoot::Codex,
                    path: format!("sessions/budget-{i:02}.jsonl"),
                })
                .collect::<Vec<_>>();
            // First publish a complete, legally admitted 64-owner family.
            // Do not create the 65th Raw child until this publication finishes.
            for (i, input) in inputs[..64].iter().enumerate() {
                let id = format!("budget-{i:02}");
                write(
                    options,
                    input,
                    &id,
                    if i == 0 { None } else { Some("budget-00") },
                );
                cache
                    .ingest(
                        &format!("{:064x}", 5400 + i),
                        &input.full_path(options),
                        "codex",
                    )
                    .unwrap();
            }
            let (batch, model) = ready(&mut cache, &f, options, &inputs[..64]);
            assert_eq!(model.prepared().rows.len(), 64);
            publish(&mut cache, &f, &batch, &model);
            assert_eq!(
                cache.finish_semantic_stage(batch.epoch).unwrap(),
                Outcome::Written
            );
            assert_eq!(f.db().query_row("SELECT count(*) FROM session_catalog WHERE namespace='source-a' AND lifecycle='current'",[],|r|r.get::<_,i64>(0)).unwrap(),64);
            assert_eq!(f.db().query_row("SELECT count(*) FROM session_catalog WHERE namespace='source-a' AND parent_key='budget-00' AND lifecycle='current'",[],|r|r.get::<_,i64>(0)).unwrap(),63);
            assert_eq!(f.db().query_row("SELECT count(*) FROM storage_proof_refs WHERE namespace='source-a' AND kind='current'",[],|r|r.get::<_,i64>(0)).unwrap(),64);
            let prior = published_state(&f);
            let foreign = namespace_state(&f, "source-b");

            write(options, &inputs[64], "budget-64", Some("budget-00"));
            cache
                .ingest(
                    &format!("{:064x}", 5464),
                    &inputs[64].full_path(options),
                    "codex",
                )
                .unwrap();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            // Two legal seeds enroll the 64 prior Current owners plus the new
            // child. This is the typed closure source bound, not admission.
            let paths = [&inputs[0], &inputs[64]]
                .iter()
                .map(|input| input.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, &runtime, true)
                    .unwrap(),
                Outcome::Written
            );
            assert_eq!(
                published_state(&f),
                prior,
                "staging preserves prior publication"
            );
            assert_eq!(
                f.db()
                    .query_row(
                        "SELECT count(*) FROM catalog_semantic_sources WHERE namespace='source-a'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                2
            );
            assert_eq!(f.db().query_row("SELECT count(*) FROM catalog_semantic_sources WHERE namespace='source-a' AND index_done=1",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            let before = shared_state(&f);
            let error = cache.semantic_batch(64).unwrap_err();
            assert!(
                error.to_string().contains("budget"),
                "explicit bounded refusal: {error}"
            );
            assert_eq!(error.to_string(), "semantic_affected_owner_source_budget");
            assert_eq!(
                shared_state(&f),
                before,
                "refused closure transaction has zero effects"
            );
            assert_eq!(
                published_state(&f),
                prior,
                "complete prior Current, History, slots and proofs survive"
            );
            assert_eq!(namespace_state(&f, "source-b"), foreign);
            assert_eq!(f.db().query_row("SELECT count(*) FROM session_catalog WHERE namespace='source-a' AND session_key='budget-64'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        }

        #[test]
        fn configured_semantic_domain61_late_child_restarts_complete_native_cohort() {
            let (f, foreign_config, options, foreign_inputs) = model_sources();
            {
                let mut foreign = open(&foreign_config);
                let input = &foreign_inputs[1];
                foreign
                    .ingest(&"7".repeat(64), &input.full_path(&options[1]), "claude")
                    .unwrap();
                let (batch, model) =
                    ready(&mut foreign, &f, &options[1], std::slice::from_ref(input));
                publish(&mut foreign, &f, &batch, &model);
                foreign.finish_semantic_stage(batch.epoch).unwrap();
            }
            let options = &options[0];
            let inputs = (0..4)
                .map(|i| crate::Input {
                    root: crate::InputRoot::Codex,
                    path: format!("sessions/late-{i}.jsonl"),
                })
                .collect::<Vec<_>>();
            let mut cache = open(&f.config);
            for (i, input) in inputs[..3].iter().enumerate() {
                write(
                    options,
                    input,
                    &format!("late-{i}"),
                    if i == 0 { None } else { Some("late-0") },
                );
                cache
                    .ingest(
                        &format!("{:064x}", 6100 + i),
                        &input.full_path(options),
                        "codex",
                    )
                    .unwrap();
            }
            let (batch, model) = ready(&mut cache, &f, options, &inputs[..3]);
            publish(&mut cache, &f, &batch, &model);
            cache.finish_semantic_stage(batch.epoch).unwrap();
            let foreign = namespace_state(&f, "source-b");
            // Hold a fully prepared publication, stronger than merely passing
            // indexing: re-entry must fence this exact old in-memory payload.
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            cache
                .begin_semantic_stage_with_journal(
                    &[inputs[0].full_path(options).to_string_lossy().into_owned()],
                    &runtime,
                    true,
                )
                .unwrap();
            let mut held = None;
            for _ in 0..128 {
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if batch.paths.is_empty() {
                    continue;
                }
                let selected = inputs[..3]
                    .iter()
                    .filter(|input| {
                        batch
                            .paths
                            .contains(&input.full_path(options).to_string_lossy().into_owned())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(selected.len(), batch.paths.len());
                let model = cache
                    .prepare_semantic_model_batch(options, &selected, &batch, 1_790_000_000_000)
                    .unwrap();
                if batch.phase == Phase::Publication {
                    held = Some((batch, model));
                    break;
                }
            }
            let (held, held_model) = held.expect("parent seed discovers its whole prior family");
            assert_eq!(held_model.prepared().rows.len(), 3);
            let prior_current = f
                .db()
                .prepare(
                    "SELECT session_key,metadata FROM session_catalog WHERE namespace='source-a' ORDER BY session_key",
                )
                .unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()
                .unwrap();
            let prior_history = f
                .db()
                .prepare(
                    "SELECT session_key,metadata FROM session_history_catalog WHERE namespace='source-a' ORDER BY session_key",
                )
                .unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()
                .unwrap();
            assert!(
                prior_history.is_empty(),
                "the first same-harness cohort has no retained custody"
            );
            let prior = namespace_state(&f, "source-a")
                .into_iter()
                .filter(|(table, _)| {
                    (table.starts_with("session_") && table != "session_catalog_invalidations")
                        || table.starts_with("storage_proof_")
                        || table == "storage_semantic_publications"
                })
                .collect::<BTreeMap<_, _>>();
            write(options, &inputs[3], "late-3", Some("late-0"));
            cache
                .ingest(
                    &format!("{:064x}", 6103),
                    &inputs[3].full_path(options),
                    "codex",
                )
                .unwrap();
            let before = shared_state(&f);
            FAIL_EFFECT.with(|p| p.set(Some("semantic_late_incident_reset")));
            assert!(cache.semantic_batch(64).is_err());
            assert_eq!(FAIL_EFFECT.with(|p| p.get()), None);
            assert_eq!(shared_state(&f), before, "late reset is atomic");
            let restarted = cache.semantic_batch(64).unwrap().unwrap();
            assert_eq!(restarted.phase, Phase::Indexing);
            assert!(restarted.epoch > held.epoch);
            assert!(restarted.paths.is_empty());
            assert_eq!(f.db().query_row("SELECT count(*) FROM storage_semantic_model_batches WHERE namespace='source-a'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            let before = shared_state(&f);
            assert_eq!(
                cache
                    .publish_semantic_catalog(&held, held_model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict
            );
            assert_eq!(shared_state(&f), before, "held model has zero effects");
            let mut indexed_new_child = false;
            let mut final_model = None;
            for _ in 0..128 {
                let published = namespace_state(&f, "source-a")
                    .into_iter()
                    .filter(|(table, _)| prior.contains_key(table))
                    .collect::<BTreeMap<_, _>>();
                assert_eq!(
                    published, prior,
                    "no intermediate publication loses a prior owner"
                );
                assert_eq!(namespace_state(&f, "source-b"), foreign);
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if batch.paths.is_empty() {
                    continue;
                }
                let selected = inputs
                    .iter()
                    .filter(|input| {
                        batch
                            .paths
                            .contains(&input.full_path(options).to_string_lossy().into_owned())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(selected.len(), batch.paths.len());
                if batch.phase == Phase::Indexing && selected.contains(&inputs[3]) {
                    indexed_new_child = true;
                }
                let model = cache
                    .prepare_semantic_model_batch(options, &selected, &batch, 1_790_000_000_000)
                    .unwrap();
                if batch.phase == Phase::Publication {
                    final_model = Some((batch, model));
                    break;
                }
            }
            assert!(
                indexed_new_child,
                "real header frontier must discover the late child"
            );
            let (batch, model) = final_model.expect("bounded native restart reaches publication");
            assert_eq!(model.prepared().rows.len(), 4);
            oracle(&cache, options, &inputs, model.prepared());
            // The admitted writer preserves a differing Current observation
            // as an original before composing History. With the explicitly
            // empty prior History above, changed owners therefore retain
            // their exact prior metadata, including Query facts and sources,
            // with the explicit Current-to-Retained custody transition.
            let mut expected_history = prior_history;
            for current in &model.prepared().rows {
                let Some(old) = prior_current.get(&current.key) else {
                    continue;
                };
                if old != &serde_json::to_string(current).unwrap() {
                    let mut retained =
                        serde_json::from_str::<crate::model::summary::CatalogRow>(old).unwrap();
                    retained.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
                    expected_history.insert(
                        current.key.clone(),
                        serde_json::to_string(&retained).unwrap(),
                    );
                }
            }
            publish(&mut cache, &f, &batch, &model);
            let db = f.db();
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM session_catalog WHERE namespace='source-a'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap(),
                4
            );
            let history = db
                .prepare(
                    "SELECT session_key,metadata FROM session_history_catalog WHERE namespace='source-a' ORDER BY session_key",
                )
                .unwrap()
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()
                .unwrap();
            assert_eq!(history, expected_history, "exact retained custody metadata");
            assert_eq!(
                history.len(),
                1,
                "one changed parent observation retains original custody"
            );
            let (history_key, history_metadata) = history.iter().next().unwrap();
            let history_row =
                serde_json::from_str::<crate::model::summary::CatalogRow>(history_metadata)
                    .unwrap();
            let mut old = serde_json::from_str::<crate::model::summary::CatalogRow>(
                &prior_current[history_key],
            )
            .unwrap();
            old.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
            assert_eq!(
                serde_json::to_value(&history_row).unwrap(),
                serde_json::to_value(&old).unwrap(),
                "original Query facts, source provenance and consumed prefixes are immutable"
            );
            assert!(
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_history_projections WHERE namespace='source-a' AND session_key=?1 AND total>=0)",
                    [history_key],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap(),
                "retained custody has a history recipe header"
            );
            assert!(
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_history_slots WHERE namespace='source-a' AND session_key=?1)",
                    [history_key],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap(),
                "retained custody preserves an inspectable slot prefix"
            );
            assert_eq!(db.query_row("SELECT count(*) FROM session_catalog WHERE namespace='source-a' AND parent_key='late-0' AND lifecycle='current'",[],|r|r.get::<_,i64>(0)).unwrap(),3);
            assert_eq!(db.query_row("SELECT count(*) FROM storage_proof_refs WHERE namespace='source-a' AND kind='current'",[],|r|r.get::<_,i64>(0)).unwrap(),4);
            assert_eq!(namespace_state(&f, "source-b"), foreign);
        }

        #[test]
        fn configured_semantic_domain61_progressive_index_overflow_is_capacity_not_conflict() {
            // Use the existing admitted primitive staging profile: unlike the
            // Current-owner profile, its discovery is not stopped earlier by
            // the distinct affected-owner source-union guard.
            let (_home, _, options, _) = model_sources();
            let options = &options[0];
            let f = semantic_fixture("source-a");
            let capability =
                crate::storage::sqlite::semantic::SemanticStaging::validate(&f.config).unwrap();
            let mut cache = EventCache::open_semantic(capability).unwrap();
            let inputs = (0..65)
                .map(|i| crate::Input {
                    root: crate::InputRoot::Codex,
                    path: format!("sessions/overflow-{i:02}.jsonl"),
                })
                .collect::<Vec<_>>();
            for (i, input) in inputs.iter().enumerate() {
                // Real duplicate rollouts of one native identity exercise the
                // header-id discovery index without an unqualified owner or
                // a fabricated dependency/endpoint row.
                write(options, input, "overflow", None);
                cache
                    .ingest(
                        &format!("{:064x}", 6200 + i),
                        &input.full_path(options),
                        "codex",
                    )
                    .unwrap();
            }
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            cache
                .begin_semantic_stage_with_journal(
                    &[inputs[0].full_path(options).to_string_lossy().into_owned()],
                    &runtime,
                    true,
                )
                .unwrap();
            let prior = namespace_state(&f, "source-a")
                .into_iter()
                .filter(|(table, _)| {
                    table.starts_with("session_")
                        || table.starts_with("storage_proof_")
                        || table == "storage_semantic_publications"
                })
                .collect::<BTreeMap<_, _>>();
            let mut reached = false;
            for _ in 0..128 {
                let before = shared_state(&f);
                match cache.semantic_batch(64) {
                    Err(error) => {
                        assert_eq!(error.to_string(), "semantic_component_source_budget");
                        assert_eq!(
                            shared_state(&f),
                            before,
                            "capacity refusal has zero effects"
                        );
                        assert_eq!(f.db().query_row("SELECT count(*) FROM catalog_semantic_sources WHERE namespace='source-a' AND index_done=1",[],|r|r.get::<_,i64>(0)).unwrap(),65);
                        reached = true;
                        break;
                    }
                    Ok(Some(batch)) => {
                        assert_eq!(batch.phase, Phase::Indexing);
                        if batch.paths.is_empty() {
                            continue;
                        }
                        let selected = inputs
                            .iter()
                            .filter(|input| {
                                batch.paths.contains(
                                    &input.full_path(options).to_string_lossy().into_owned(),
                                )
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        assert_eq!(selected.len(), batch.paths.len());
                        cache
                            .prepare_semantic_model_batch(
                                options,
                                &selected,
                                &batch,
                                1_790_000_000_000,
                            )
                            .unwrap();
                    }
                    Ok(None) => panic!("real progressive stage disappeared"),
                }
            }
            assert!(
                reached,
                "65 real indexed sources must reach typed capacity guard"
            );
            let after = namespace_state(&f, "source-a")
                .into_iter()
                .filter(|(table, _)| prior.contains_key(table))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(
                after, prior,
                "exact prior publication survives progressive refusal"
            );
        }

        #[test]
        fn configured_semantic_domain61_real_ledger_change_remains_conflict() {
            let (f, _, options, inputs) = model_sources();
            let options = &options[0];
            let input = &inputs[0];
            let mut cache = open(&f.config);
            cache
                .ingest(
                    &format!("{:064x}", 6300),
                    &input.full_path(options),
                    "codex",
                )
                .unwrap();
            let (_batch, _model) = ready(&mut cache, &f, options, std::slice::from_ref(input));
            write(options, input, "changed-ledger", None);
            cache
                .ingest(
                    &format!("{:064x}", 6301),
                    &input.full_path(options),
                    "codex",
                )
                .unwrap();
            let before = shared_state(&f);
            assert_eq!(
                cache.semantic_batch(64).unwrap_err().to_string(),
                StorageError::Conflict.to_string()
            );
            assert_eq!(
                shared_state(&f),
                before,
                "a genuine ledger/claim conflict has zero effects"
            );
        }

        fn selector_inputs(options: &crate::Options, duplicate: bool) -> Vec<crate::Input> {
            (0..3).map(|i| {
                let input = crate::Input { root: crate::InputRoot::Codex, path: format!("sessions/selector-{i}.jsonl") };
                let mut meta = serde_json::json!({"id":format!("selector-{i}"),"cwd":"/sanitized"});
                if i != 0 {
                    meta["parent_thread_id"] = serde_json::json!("selector-0");
                    meta["thread_source"] = serde_json::json!("subagent");
                    meta["agent_path"] = serde_json::json!(if duplicate { "root/shared".into() } else { format!("root/child-{i}") });
                    meta["agent_nickname"] = serde_json::json!(if duplicate { "shared".into() } else { format!("child-{i}") });
                }
                let mut raw = format!("{}\n{}\n",
                    serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":meta}),
                    serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:01:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"selector work"}]}}));
                if i == 0 && !duplicate {
                    raw.push_str(&format!("{}\n", serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:02:00Z","payload":{"type":"function_call","name":"collaboration.send_message","call_id":"selector-relay","arguments":"{\"target\":\"child-1\",\"message\":\"native relay\"}"}})));
                }
                fs::write(input.full_path(options), raw).unwrap();
                input
            }).collect()
        }

        fn selector_ready(
            cache: &mut EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
        ) -> (Batch, crate::model::SemanticModelBatch) {
            for (i, input) in inputs.iter().enumerate() {
                if cache.explicit_store {
                    cache
                        .ingest(
                            &format!("{:064x}", 6800 + i),
                            &input.full_path(options),
                            "codex",
                        )
                        .unwrap();
                } else {
                    crate::events::scan_file(&input.full_path(options), "codex", cache, &mut false)
                        .unwrap();
                }
            }
            let paths = inputs
                .iter()
                .map(|i| i.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, &runtime, true)
                    .unwrap(),
                Outcome::Written
            );
            for _ in 0..128 {
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if batch.paths.is_empty() {
                    continue;
                }
                let selected = inputs
                    .iter()
                    .filter(|i| {
                        batch
                            .paths
                            .contains(&i.full_path(options).to_string_lossy().into_owned())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(selected.len(), batch.paths.len());
                let model = crate::model::prepare_semantic_batch(
                    options,
                    cache,
                    &selected,
                    &batch,
                    1_790_000_000_000,
                )
                .unwrap();
                // Ordinary publication consumes the prepared batch directly;
                // its semantic_record contract accepts only earlier phases.
                if cache.explicit_store || batch.phase != Phase::Publication {
                    assert_eq!(
                        cache
                            .record_semantic_batch(&batch, model.prepared())
                            .unwrap(),
                        Outcome::Written
                    );
                }
                if batch.phase == Phase::Publication {
                    return (batch, model);
                }
            }
            panic!("selector producer failed to reach publication")
        }

        fn selector_value(selection: crate::model::relation_stage::Selection) -> serde_json::Value {
            let mut endpoint = selection.endpoint.map(|e| serde_json::to_value(e).unwrap());
            if let Some(query) = endpoint
                .as_mut()
                .and_then(|e| e.get_mut("row"))
                .and_then(|r| r.get_mut("query"))
                .and_then(serde_json::Value::as_object_mut)
            {
                // Source-bound and ordinary runtime addresses differ; native
                // identity, provenance, locations and every other field agree.
                query.remove("runtime");
            }
            serde_json::json!({"count":selection.count,"key":selection.key,"endpoint":endpoint,"location":selection.location})
        }

        #[test]
        fn configured_semantic_domain67_codex_sibling_relay_wait_real_producer_oracle() {
            use crate::model::relation_stage::Selector;
            for duplicate in [false, true] {
                let (f, _, options, _) = model_sources();
                let options = &options[0];
                let inputs = selector_inputs(options, duplicate);
                let mut admitted = open(&f.config);
                let (batch, model) = selector_ready(&mut admitted, options, &inputs);
                let mut ordinary = EventCache::open_scoped(&options.cache).unwrap();
                let (reference, _) = selector_ready(&mut ordinary, options, &inputs);
                let task = if duplicate { "shared" } else { "child-1" };
                let target = if duplicate {
                    "root/shared"
                } else {
                    "root/child-1"
                };
                let selectors = [
                    Selector::CodexSibling {
                        parent: "selector-0".into(),
                        task: task.into(),
                    },
                    Selector::CodexRelay {
                        root: "selector-0".into(),
                        target: task.into(),
                    },
                    Selector::CodexRelay {
                        root: "selector-0".into(),
                        target: target.into(),
                    },
                    Selector::CodexWait {
                        root: "selector-0".into(),
                        target: target.into(),
                        exclude: "selector-0".into(),
                    },
                    Selector::CodexWait {
                        root: "selector-0".into(),
                        target: target.into(),
                        exclude: "selector-1".into(),
                    },
                ];
                for (i, selector) in selectors.iter().enumerate() {
                    let actual = admitted.semantic_select(batch.epoch, selector).unwrap();
                    let expected = ordinary.semantic_select(reference.epoch, selector).unwrap();
                    let count = if i == 4 {
                        usize::from(duplicate)
                    } else if duplicate {
                        2
                    } else {
                        1
                    };
                    assert_eq!(actual.count, count, "{selector:?}");
                    assert_eq!(
                        selector_value(actual),
                        selector_value(expected),
                        "{selector:?}"
                    );
                }
                oracle(&admitted, options, &inputs, model.prepared());
                publish(&mut admitted, &f, &batch, &model);
            }
        }

        #[test]
        fn configured_semantic_domain67_foreign_native_ids_and_paths_are_isolated() {
            use crate::model::relation_stage::Selector;
            let (f, b, options, _) = model_sources();
            let options = &options[0];
            let inputs = selector_inputs(options, false);
            let mut a_cache = open(&f.config);
            let mut b_cache = open(&b);
            let (a_batch, a_model) = selector_ready(&mut a_cache, options, &inputs);
            let (b_batch, b_model) = selector_ready(&mut b_cache, options, &inputs);
            b_cache
                .publish_semantic_catalog(&b_batch, b_model.prepared())
                .unwrap();
            let foreign = namespace_state(&f, "source-b");
            let before = shared_state(&f);
            for selector in [
                Selector::CodexSibling {
                    parent: "selector-0".into(),
                    task: "child-1".into(),
                },
                Selector::CodexRelay {
                    root: "selector-0".into(),
                    target: "root/child-1".into(),
                },
                Selector::CodexWait {
                    root: "selector-0".into(),
                    target: "root/child-1".into(),
                    exclude: "selector-0".into(),
                },
            ] {
                assert_eq!(
                    a_cache
                        .semantic_select(a_batch.epoch, &selector)
                        .unwrap()
                        .count,
                    1
                );
            }
            assert_eq!(
                shared_state(&f),
                before,
                "selector reads have zero whole-DB effects"
            );
            a_cache
                .publish_semantic_catalog(&a_batch, a_model.prepared())
                .unwrap();
            assert_eq!(namespace_state(&f, "source-b"), foreign);
        }

        #[test]
        fn configured_semantic_domain67_stale_epoch_source_collision_has_zero_effects() {
            use crate::model::relation_stage::Selector;
            let (f, _, options, _) = model_sources();
            let options = &options[0];
            let inputs = selector_inputs(options, false);
            let mut old = open(&f.config);
            let (batch, model) = selector_ready(&mut old, options, &inputs);
            crate::facts::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: 2,
                    hostname: "selector-new-authority".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let mut replacement = open(&f.config);
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            let paths = inputs
                .iter()
                .map(|i| i.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            assert_eq!(
                replacement
                    .begin_semantic_stage_with_journal(&paths, &runtime, false)
                    .unwrap(),
                Outcome::Written
            );
            let before = shared_state(&f);
            assert!(
                old.semantic_select(
                    batch.epoch,
                    &Selector::CodexRelay {
                        root: "selector-0".into(),
                        target: "child-1".into()
                    }
                )
                .is_err()
            );
            assert_eq!(shared_state(&f), before);
            assert_eq!(
                old.publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict
            );
            assert_eq!(shared_state(&f), before);
        }

        #[test]
        fn configured_semantic_domain67_claude_event_producer_projects_real_call() {
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let inputs = claude_inputs(options, 1, 1);
            let mut cache = open(&f.config);
            let (batch, model) = claude_ready(&mut cache, options, &inputs);
            assert_eq!(
                cache
                    .semantic_select(
                        batch.epoch,
                        &crate::model::relation_stage::Selector::ClaudeCall {
                            id: "claude-call-0".into()
                        }
                    )
                    .unwrap()
                    .count,
                1
            );
            oracle(&cache, options, &inputs, model.prepared());
            publish(&mut cache, &f, &batch, &model);
        }
        #[test]
        fn configured_semantic_domain72_claude_unsupported_name_refuses() {
            use crate::model::relation_stage::Selector;
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let inputs = claude_inputs(options, 1, 1);
            let mut cache = open(&f.config);
            let (batch, model) = claude_ready(&mut cache, options, &inputs);
            let before = shared_state(&f);
            assert_eq!(
                cache
                    .semantic_select(
                        batch.epoch,
                        &Selector::ClaudeName {
                            name: "worker".into()
                        }
                    )
                    .unwrap_err()
                    .to_string(),
                "semantic_claude_name_members_pending"
            );
            assert_eq!(shared_state(&f), before);
            publish(&mut cache, &f, &batch, &model);
            cache.finish_semantic_stage(batch.epoch).unwrap();
        }

        #[test]
        fn configured_semantic_domain72_receipt_frontier_pages_to_source_capacity_refusal() {
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let inputs = claude_inputs(options, 65, 0);
            // The ordinary budget counts frontier observations, including empty
            // tasks and already-enrolled paths; it does not promise 64 new rows.
            let mut ordinary = EventCache::open_scoped(&options.cache).unwrap();
            claude_ingest(&mut ordinary, options, &inputs);
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            ordinary
                .begin_semantic_stage_with_journal(
                    &[inputs[0].full_path(options).to_string_lossy().into_owned()],
                    &runtime,
                    true,
                )
                .unwrap();
            let ordinary_db = Connection::open(options.cache.with_extension("sqlite3")).unwrap();
            let mut ordinary_paged = false;
            for _ in 0..256 {
                let batch = ordinary.semantic_batch(64).unwrap().unwrap();
                let pending: bool = ordinary_db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE kind='receipt' AND after_key<>'' AND done=0)", [], |r| r.get(0)).unwrap();
                let enrolled: i64 = ordinary_db
                    .query_row("SELECT count(*) FROM catalog_semantic_sources", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                if pending && enrolled > 1 && enrolled <= 64 {
                    ordinary_paged = true;
                    break;
                }
                if !batch.paths.is_empty() {
                    assert_eq!(batch.phase, Phase::Indexing);
                    let selected = inputs
                        .iter()
                        .filter(|i| {
                            batch
                                .paths
                                .contains(&i.full_path(options).to_string_lossy().into_owned())
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    ordinary
                        .prepare_semantic_model_batch(options, &selected, &batch, 1_790_000_000_000)
                        .unwrap();
                }
            }
            assert!(
                ordinary_paged,
                "ordinary producer must demonstrate the same persisted receipt cursor"
            );
            let mut cache = open(&f.config);
            claude_ingest(&mut cache, options, &inputs);
            cache
                .begin_semantic_stage_with_journal(
                    &[inputs[0].full_path(options).to_string_lossy().into_owned()],
                    &runtime,
                    true,
                )
                .unwrap();
            let mut reached = false;
            let mut paged = false;
            for _ in 0..256 {
                let before = shared_state(&f);
                match cache.semantic_batch(64) {
                    Err(error) => {
                        assert_eq!(error.to_string(), "semantic_component_source_budget");
                        assert_eq!(
                            shared_state(&f),
                            before,
                            "capacity refusal rolls back all namespaces"
                        );
                        reached = true;
                        break;
                    }
                    Ok(Some(batch)) => {
                        let enrolled:i64=f.db().query_row("SELECT count(*) FROM catalog_semantic_sources WHERE namespace='source-a'",[],|r|r.get(0)).unwrap();
                        let pending_receipt: bool = f.db().query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE namespace='source-a' AND kind='receipt' AND after_key<>'' AND done=0)", [], |r| r.get(0)).unwrap();
                        paged |= enrolled > 1 && enrolled <= 64 && pending_receipt;
                        if !batch.paths.is_empty() {
                            assert_eq!(batch.phase, Phase::Indexing);
                            let selected = inputs
                                .iter()
                                .filter(|i| {
                                    batch.paths.contains(
                                        &i.full_path(options).to_string_lossy().into_owned(),
                                    )
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            cache
                                .prepare_semantic_model_batch(
                                    options,
                                    &selected,
                                    &batch,
                                    1_790_000_000_000,
                                )
                                .unwrap();
                        }
                    }
                    Ok(None) => panic!("receipt frontier disappeared"),
                }
            }
            assert!(
                paged,
                "receipt discovery must persist a bounded unfinished keyset page"
            );
            assert!(
                reached,
                "65th physical receipt source must hit the real source bound"
            );
            assert_eq!(
                f.db()
                    .query_row(
                        "SELECT count(*) FROM session_catalog WHERE namespace='source-a'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
        }
        fn claude_inputs(
            options: &crate::Options,
            copies: usize,
            events: usize,
        ) -> Vec<crate::Input> {
            (0..copies).map(|i| {
                let input = crate::Input { root: crate::InputRoot::Claude, path: format!("projects/sanitized/claude72-{i}.jsonl").into() };
                let mut raw = format!("{}\n",serde_json::json!({"type":"user","sessionId":format!("claude72-{i}"),"uuid":format!("first-{i}"),"timestamp":"2026-10-01T00:00:00Z","cwd":"/sanitized","message":{"role":"user","content":"native work"}}));
                for n in 0..events {
                    raw.push_str(&format!("{}\n",serde_json::json!({"type":"assistant","sessionId":format!("claude72-{i}"),"uuid":format!("call-row-{n}"),"timestamp":"2026-10-01T00:01:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":format!("claude-call-{n}"),"name":"Agent","input":{"prompt":"sanitized work"}}]}})));
                }
                raw.push_str(&format!("{}\n",serde_json::json!({"type":"user","sessionId":format!("claude72-{i}"),"uuid":"receipt-shared","timestamp":"2026-10-01T00:02:00Z","origin":{"kind":"peer","msg_id":"claude-message","name":"worker"},"message":{"role":"user","content":"recorded receipt"}})));
                fs::write(input.full_path(options),raw).unwrap();
                input
            }).collect()
        }

        fn claude_ingest(
            cache: &mut EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
        ) {
            for (i, input) in inputs.iter().enumerate() {
                if cache.explicit_store {
                    cache
                        .ingest(
                            &format!("{:064x}", 7200 + i),
                            &input.full_path(options),
                            "claude",
                        )
                        .unwrap();
                } else {
                    crate::events::scan_file(
                        &input.full_path(options),
                        "claude",
                        cache,
                        &mut false,
                    )
                    .unwrap();
                }
            }
        }

        fn ordinary_claude_ready(
            cache: &mut EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
        ) -> Batch {
            assert!(!cache.explicit_store);
            claude_ingest(cache, options, inputs);
            let paths = inputs
                .iter()
                .map(|i| i.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, &runtime, true)
                    .unwrap(),
                Outcome::Written
            );
            for _ in 0..256 {
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if batch.paths.is_empty() {
                    continue;
                }
                let mut selected = batch
                    .paths
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                if batch.phase == Phase::Relations {
                    selected.extend(batch.context.parent_paths(&batch.paths));
                    selected.extend(batch.context.companions.iter().cloned());
                }
                assert!(selected.len() <= 64);
                let selected = selected
                    .iter()
                    .map(|path| {
                        crate::native_evidence::catalog_input(Path::new(path), options).unwrap()
                    })
                    .collect::<Vec<_>>();
                let prepared = match batch.phase {
                    Phase::Indexing => crate::model::prepare_semantic_index_sources(
                        options,
                        cache,
                        &selected,
                        1_790_000_000_000,
                    ),
                    Phase::Identity => crate::model::prepare_semantic_identity_sources(
                        options,
                        cache,
                        &selected,
                        &batch.context,
                        1_790_000_000_000,
                    ),
                    Phase::Relations | Phase::Publication => crate::model::prepare_staged_sources(
                        options,
                        cache,
                        &selected,
                        &batch.context,
                        batch.phase == Phase::Relations,
                        1_790_000_000_000,
                    ),
                }
                .unwrap();
                if batch.phase == Phase::Publication {
                    return batch;
                }
                assert_eq!(
                    cache.record_semantic_batch(&batch, &prepared).unwrap(),
                    Outcome::Written
                );
            }
            panic!("ordinary Claude producer did not finish");
        }

        fn claude_ready(
            cache: &mut EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
        ) -> (Batch, crate::model::SemanticModelBatch) {
            claude_ready_from(cache, options, inputs, inputs)
        }
        fn claude_ready_from(
            cache: &mut EventCache,
            options: &crate::Options,
            inputs: &[crate::Input],
            seeds: &[crate::Input],
        ) -> (Batch, crate::model::SemanticModelBatch) {
            claude_ingest(cache, options, inputs);
            let paths = seeds
                .iter()
                .map(|i| i.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, &runtime, true)
                    .unwrap(),
                Outcome::Written
            );
            for _ in 0..256 {
                let batch = cache.semantic_batch(64).unwrap().unwrap();
                if cache.explicit_store && matches!(batch.phase, Phase::Indexing | Phase::Identity)
                {
                    let error = cache
                        .semantic_select(
                            batch.epoch,
                            &crate::model::relation_stage::Selector::ClaudeCall {
                                id: "claude-call-0".into(),
                            },
                        )
                        .unwrap_err();
                    assert_eq!(
                        error.to_string(),
                        "semantic_claude_event_projection_pending"
                    );
                }
                if batch.paths.is_empty() {
                    continue;
                }
                let selected = inputs
                    .iter()
                    .filter(|i| {
                        batch
                            .paths
                            .contains(&i.full_path(options).to_string_lossy().into_owned())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(selected.len(), batch.paths.len());
                let model = crate::model::prepare_semantic_batch(
                    options,
                    cache,
                    &selected,
                    &batch,
                    1_790_000_000_000,
                )
                .unwrap();
                if cache.explicit_store || batch.phase != Phase::Publication {
                    assert_eq!(
                        cache
                            .record_semantic_batch(&batch, model.prepared())
                            .unwrap(),
                        Outcome::Written,
                        "phase={:?}, admitted={}, rejection={:?}",
                        batch.phase,
                        cache.explicit_store,
                        cache.semantic_rejection()
                    );
                }
                if batch.phase == Phase::Publication {
                    return (batch, model);
                }
            }
            panic!("real Claude producer did not finish");
        }

        fn projection_rows(db: &Connection, ns: Option<&str>) -> Vec<String> {
            let predicate = if ns.is_some() {
                "WHERE namespace=?1"
            } else {
                ""
            };
            let sql = format!(
                "SELECT json_array(source_path,event_key,owner,kind,msg_id,tool_id,name,metadata,native_seq,copy_key,copied,qualified) FROM catalog_semantic_events {predicate} ORDER BY source_path,event_key"
            );
            let mut q = db.prepare(&sql).unwrap();
            if let Some(ns) = ns {
                q.query_map([ns], |r| r.get(0))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            } else {
                q.query_map([], |r| r.get(0))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            }
        }

        #[test]
        fn configured_semantic_domain72_claude_receipt_call_real_producer_oracle() {
            use crate::model::relation_stage::Selector;
            for copies in [1, 2] {
                let (f, _, options, _) = model_sources();
                let options = &options[1];
                let inputs = claude_inputs(options, copies, 1);
                let mut admitted = open(&f.config);
                let (batch, model) = claude_ready(&mut admitted, options, &inputs);
                let mut ordinary = EventCache::open_scoped(&options.cache).unwrap();
                let reference = ordinary_claude_ready(&mut ordinary, options, &inputs);
                let ordinary_db =
                    Connection::open(options.cache.with_extension("sqlite3")).unwrap();
                let actual = projection_rows(&f.db(), Some("source-a"));
                assert_eq!(
                    actual.len(),
                    copies * 2,
                    "real calls and receipts were projected"
                );
                assert_eq!(
                    actual,
                    projection_rows(&ordinary_db, None),
                    "all event metadata and physical owner fields"
                );
                for selector in [
                    Selector::ClaudeCall {
                        id: "claude-call-0".into(),
                    },
                    Selector::ClaudeReceipt {
                        id: "claude-message".into(),
                    },
                ] {
                    let actual = admitted.semantic_select(batch.epoch, &selector).unwrap();
                    let expected = ordinary
                        .semantic_select(reference.epoch, &selector)
                        .unwrap();
                    assert_eq!(
                        actual.count,
                        if matches!(selector, Selector::ClaudeCall { .. }) {
                            copies
                        } else {
                            1
                        }
                    );
                    assert_eq!(selector_value(actual), selector_value(expected));
                }
                oracle(&admitted, options, &inputs, model.prepared());
                publish(&mut admitted, &f, &batch, &model);
            }
        }

        #[test]
        fn configured_semantic_domain72_claude_foreign_namespace_projection_isolated() {
            let (f, b, options, _) = model_sources();
            let options = &options[1];
            let inputs = claude_inputs(options, 1, 1);
            let mut foreign = open(&b);
            let (foreign_batch, foreign_model) = claude_ready(&mut foreign, options, &inputs);
            assert_eq!(
                foreign
                    .publish_semantic_catalog(&foreign_batch, foreign_model.prepared())
                    .unwrap()
                    .0,
                Outcome::Written
            );
            let before = namespace_state(&f, "source-b");
            let mut own = open(&f.config);
            let (batch, model) = claude_ready(&mut own, options, &inputs);
            let mut altered = crate::model::prepare_staged_sources(
                options,
                &mut own,
                &inputs,
                &batch.context,
                false,
                1_790_000_000_000,
            )
            .unwrap();
            let stub = altered
                .transcripts
                .iter()
                .position(|projection| !altered.rows.iter().any(|row| row.key == projection.key))
                .expect("real unmatched native call produces a virtual transcript");
            let real_key = altered.transcripts[stub].key.clone();
            let unchanged = shared_state(&f);
            altered.transcripts[stub].key = "unrecorded-virtual-owner".into();
            assert_eq!(
                own.publish_semantic_catalog(&batch, &altered)
                    .unwrap_err()
                    .to_string(),
                "semantic_publication_projection_owner"
            );
            assert_eq!(
                shared_state(&f),
                unchanged,
                "arbitrary virtual projection has zero effects"
            );
            altered.transcripts[stub].key = real_key.clone();
            let relation = altered
                .relationships
                .relations
                .values_mut()
                .find(|relation| relation.stubs.iter().any(|stub| stub.key == real_key))
                .unwrap();
            relation
                .stubs
                .iter_mut()
                .find(|stub| stub.key == real_key)
                .unwrap()
                .name
                .push_str("-unrecorded");
            assert_eq!(
                own.publish_semantic_catalog(&batch, &altered).unwrap().0,
                Outcome::Conflict
            );
            assert_eq!(
                shared_state(&f),
                unchanged,
                "typed anchored stub must match the recorded producer witness"
            );
            publish(&mut own, &f, &batch, &model);
            for table in [
                "session_catalog",
                "session_catalog_sources",
                "session_slot_projections",
                "session_slots",
                "catalog_semantic_owner_sources",
            ] {
                assert_eq!(
                    f.db().query_row(
                        &format!("SELECT count(*) FROM {table} WHERE namespace='source-a' AND session_key=?1"),
                        [&real_key],
                        |row| row.get::<_, i64>(0),
                    ).unwrap(),
                    0,
                    "virtual stubs have no persisted owner or Raw membership in {table}"
                );
            }
            assert_eq!(
                namespace_state(&f, "source-b"),
                before,
                "identical paths, native IDs and message IDs stay foreign"
            );
        }

        #[test]
        fn configured_semantic_domain72_claude_projection_epoch_reopen_has_zero_effects() {
            use crate::model::relation_stage::Selector;
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let inputs = claude_inputs(options, 1, 130);
            let mut old = open(&f.config);
            claude_ingest(&mut old, options, &inputs);
            let paths = inputs
                .iter()
                .map(|i| i.full_path(options).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            old.begin_semantic_stage_with_journal(&paths, &runtime, true)
                .unwrap();
            let mut reached = false;
            for _ in 0..64 {
                let batch = old.semantic_batch(64).unwrap().unwrap();
                let count:i64=f.db().query_row("SELECT count(*) FROM catalog_semantic_events WHERE namespace='source-a' AND qualified=0",[],|r|r.get(0)).unwrap();
                if count > 0 {
                    assert!(count <= 64, "one bounded projection page before reopening");
                    crate::facts::write_facts(
                        options.facts.as_ref().unwrap(),
                        &crate::Facts {
                            version: 2,
                            hostname: "reopened-event-source".into(),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    let mut next = open(&f.config);
                    let runtime =
                        crate::native_evidence::query_runtime_generation(options).unwrap();
                    next.begin_semantic_stage_with_journal(&paths, &runtime, true)
                        .unwrap();
                    let before = shared_state(&f);
                    assert!(
                        old.semantic_select(
                            batch.epoch,
                            &Selector::ClaudeCall {
                                id: "claude-call-0".into()
                            }
                        )
                        .is_err()
                    );
                    assert_eq!(shared_state(&f), before);
                    assert!(old.semantic_batch(64).is_err());
                    assert_eq!(
                        shared_state(&f),
                        before,
                        "stale projection holder cannot change any namespace"
                    );
                    reached = true;
                    break;
                }
                if !batch.paths.is_empty() {
                    old.prepare_semantic_model_batch(options, &inputs, &batch, 1_790_000_000_000)
                        .unwrap();
                }
            }
            assert!(reached);
        }

        #[test]
        fn configured_semantic_domain72_claude_copied_events_qualify_after_reduce_in_pages() {
            use crate::model::relation_stage::Selector;
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let inputs = claude_inputs(options, 2, 65);
            let old = inputs[0].full_path(options);
            let mut raw = fs::read_to_string(&old).unwrap();
            raw.push_str(&format!("{}\n",serde_json::json!({"type":"continued-in","sessionId":"claude72-0","continuedInSessionId":"claude72-1","timestamp":"2026-10-01T00:03:00Z"})));
            fs::write(old, raw).unwrap();
            let mut admitted = open(&f.config);
            let (batch, model) = claude_ready(&mut admitted, options, &inputs);
            let mut ordinary = EventCache::open_scoped(&options.cache).unwrap();
            let reference = ordinary_claude_ready(&mut ordinary, options, &inputs);
            let db = f.db();
            let counts:(i64,i64,i64)=db.query_row("SELECT count(*),sum(qualified),sum(copied) FROM catalog_semantic_events WHERE namespace='source-a'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            assert_eq!(
                counts,
                (132, 132, 66),
                "continued source copies are suppressed only after physical ownership resolves"
            );
            let ordinary_db = Connection::open(options.cache.with_extension("sqlite3")).unwrap();
            assert_eq!(
                projection_rows(&db, Some("source-a")),
                projection_rows(&ordinary_db, None)
            );
            let selector = Selector::ClaudeCall {
                id: "claude-call-0".into(),
            };
            let actual = admitted.semantic_select(batch.epoch, &selector).unwrap();
            assert_eq!(actual.count, 1, "copy cannot create call ambiguity");
            assert_eq!(
                selector_value(actual),
                selector_value(
                    ordinary
                        .semantic_select(reference.epoch, &selector)
                        .unwrap()
                )
            );
            oracle(&admitted, options, &inputs, model.prepared());
            publish(&mut admitted, &f, &batch, &model);
        }

        fn peer_inputs(options: &crate::Options, copies: usize, peer: &str) -> Vec<crate::Input> {
            let inputs = claude_inputs(options, copies, 1);
            for (i, input) in inputs.iter().enumerate() {
                let path = input.full_path(options);
                let raw = fs::read_to_string(&path)
                    .unwrap()
                    .lines()
                    .map(|line| {
                        let mut record: serde_json::Value = serde_json::from_str(line).unwrap();
                        if record.get("origin").is_some() {
                            record["origin"]["msg_id"] =
                                serde_json::json!(format!("peer81-message-{i}"));
                            if i == 0 {
                                record["origin"]["from"] = serde_json::json!(peer);
                            }
                        }
                        format!("{record}\n")
                    })
                    .collect::<String>();
                fs::write(path, raw).unwrap();
            }
            inputs
        }
        #[test]
        fn configured_semantic_domain81_peer_filename_discovery_and_absent_negative_oracle() {
            for known in [false, true] {
                let (f, _, options, _) = model_sources();
                let options = &options[1];
                let peer = if known { "claude72-1" } else { "absent81" };
                let inputs = peer_inputs(options, if known { 2 } else { 1 }, peer);
                let mut admitted = open(&f.config);
                let (batch, model) =
                    claude_ready_from(&mut admitted, options, &inputs, &inputs[..1]);
                let db = f.db();
                assert!(!db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE namespace='source-a' AND done=0)",[],|r|r.get::<_,bool>(0)).unwrap(), "no negative or identity may precede completed discovery");
                let indexed: i64 = db.query_row("SELECT count(*) FROM catalog_semantic_sources WHERE namespace='source-a' AND index_done=1",[],|r|r.get(0)).unwrap();
                assert_eq!(indexed, inputs.len() as i64);
                if known {
                    assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE namespace='source-a' AND kind='claudeid' AND value='claude72-1.jsonl' AND done=1)",[],|r|r.get::<_,bool>(0)).unwrap(), "target must be discovered by the native filename frontier");
                }
                let mut ordinary = EventCache::open_scoped(&options.cache).unwrap();
                ordinary_claude_ready(&mut ordinary, options, &inputs);
                let reference = Connection::open(options.cache.with_extension("sqlite3")).unwrap();
                assert_eq!(
                    projection_rows(&db, Some("source-a")),
                    projection_rows(&reference, None)
                );
                oracle(&admitted, options, &inputs, model.prepared());
                publish(&mut admitted, &f, &batch, &model);
            }
        }
        #[test]
        fn configured_semantic_domain81_native_peer_stale_epoch_zero_effects() {
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let inputs = peer_inputs(options, 2, "claude72-1");
            let mut old = open(&f.config);
            let (batch, model) = claude_ready_from(&mut old, options, &inputs, &inputs[..1]);
            let mut successor = open(&f.config);
            successor
                .begin_semantic_stage_with_journal(
                    &[inputs[0].full_path(options).to_string_lossy().into_owned()],
                    "peer81-successor",
                    true,
                )
                .unwrap();
            let before = shared_state(&f);
            assert!(old.semantic_batch(64).is_err());
            assert_eq!(shared_state(&f), before);
            assert_eq!(
                old.publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict
            );
            assert_eq!(shared_state(&f), before);
        }

        #[test]
        fn configured_semantic_domain81_peer_foreign_collision_current_and_dependency_custody() {
            let (f, foreign_config, options, _) = model_sources();
            let options = &options[1];
            let inputs = peer_inputs(options, 2, "claude72-1");
            // Identical native IDs, messages and physical paths in both namespaces.
            let mut foreign = open(&foreign_config);
            let (foreign_batch, foreign_model) =
                claude_ready_from(&mut foreign, options, &inputs, &inputs[..1]);
            let frontier = |namespace: &str| {
                f.db().prepare("SELECT kind,value,harness,after_key,after_edge,done FROM catalog_semantic_frontier WHERE namespace=?1 ORDER BY kind,value,harness").unwrap().query_map([namespace], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,bool>(5)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
            };
            let control_frontier = frontier("source-b");
            publish(&mut foreign, &f, &foreign_batch, &foreign_model);
            foreign.finish_semantic_stage(foreign_batch.epoch).unwrap();
            let before = namespace_state(&f, "source-b");
            let mut admitted = open(&f.config);
            let (batch, model) = claude_ready_from(&mut admitted, options, &inputs, &inputs[..1]);
            assert_eq!(
                frontier("source-a"),
                control_frontier,
                "populated foreign Current/dependencies cannot alter the complete local frontier compared with the same Raw control"
            );
            oracle(&admitted, options, &inputs, model.prepared());
            publish(&mut admitted, &f, &batch, &model);
            admitted.finish_semantic_stage(batch.epoch).unwrap();
            assert_eq!(namespace_state(&f, "source-b"), before);
            // Repeat through actual published Current/dependency authority.
            let (next, next_model) =
                claude_ready_from(&mut admitted, options, &inputs, &inputs[..1]);
            assert!(f.db().query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE namespace='source-a' AND kind IN ('native','dependency','nativeall') AND done=1)",[],|r|r.get::<_,bool>(0)).unwrap());
            oracle(&admitted, options, &inputs, next_model.prepared());
            publish(&mut admitted, &f, &next, &next_model);
            assert_eq!(
                namespace_state(&f, "source-b"),
                before,
                "all foreign table rows remain byte-identical"
            );
        }
        #[test]
        fn configured_semantic_domain81_filename_peer_pages_before_source_budget_refusal() {
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let mut inputs = peer_inputs(options, 66, "peer81-target");
            for (i, input) in inputs.iter_mut().enumerate().skip(1) {
                let old = input.full_path(options);
                input.path = format!("projects/peer81-{i}/peer81-target.jsonl").into();
                let path = input.full_path(options);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::rename(old, path).unwrap();
            }
            let mut admitted = open(&f.config);
            claude_ingest(&mut admitted, options, &inputs);
            let seed = inputs[0].full_path(options).to_string_lossy().into_owned();
            let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
            admitted
                .begin_semantic_stage_with_journal(&[seed.clone()], &runtime, true)
                .unwrap();
            let mut paged = false;
            let mut refused = false;
            for _ in 0..128 {
                let before = shared_state(&f);
                match admitted.semantic_batch(64) {
                    Err(error) => {
                        assert_eq!(error.to_string(), "semantic_component_source_budget");
                        assert_eq!(
                            shared_state(&f),
                            before,
                            "whole-TX rollback at source capacity"
                        );
                        refused = true;
                        break;
                    }
                    Ok(Some(batch)) => {
                        let cursor = f.db().query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE namespace='source-a' AND kind='claudeid' AND after_key<>'' AND done=0)",[],|r|r.get::<_,bool>(0)).unwrap();
                        if cursor {
                            assert_eq!(
                                batch.paths.len(),
                                64,
                                "the real filename frontier returns one full bounded page"
                            );
                            assert_eq!(f.db().query_row("SELECT count(*) FROM catalog_semantic_sources WHERE namespace='source-a'", [], |r| r.get::<_,i64>(0)).unwrap(), 65, "seed plus exactly 64 discovered candidates precede the component refusal");
                        }
                        paged |= cursor;
                        if !batch.paths.is_empty() {
                            let selected = inputs
                                .iter()
                                .filter(|i| {
                                    batch.paths.contains(
                                        &i.full_path(options).to_string_lossy().into_owned(),
                                    )
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            admitted
                                .prepare_semantic_model_batch(
                                    options,
                                    &selected,
                                    &batch,
                                    1_790_000_000_000,
                                )
                                .unwrap_or_else(|error| {
                                    panic!("paging model phase {:?}: {error}", batch.phase)
                                });
                        }
                    }
                    Ok(None) => panic!("native frontier stage disappeared"),
                }
            }
            assert!(
                paged && refused,
                "native filename candidates persist a bounded cursor before refusal"
            );
            assert_eq!(
                f.db()
                    .query_row(
                        "SELECT count(*) FROM session_catalog WHERE namespace='source-a'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
            let mut ordinary = EventCache::open_scoped(&options.cache).unwrap();
            claude_ingest(&mut ordinary, options, &inputs);
            ordinary
                .begin_semantic_stage_with_journal(&[seed], &runtime, true)
                .unwrap();
            let db = Connection::open(options.cache.with_extension("sqlite3")).unwrap();
            let mut ordinary_paged = false;
            for _ in 0..128 {
                let batch = ordinary.semantic_batch(64).unwrap().unwrap();
                if db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE kind='claudeid' AND after_key<>'' AND done=0)",[],|r|r.get::<_,bool>(0)).unwrap() { ordinary_paged=true; break; }
                if !batch.paths.is_empty() {
                    let selected = inputs
                        .iter()
                        .filter(|i| {
                            batch
                                .paths
                                .contains(&i.full_path(options).to_string_lossy().into_owned())
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    ordinary
                        .prepare_semantic_model_batch(options, &selected, &batch, 1_790_000_000_000)
                        .unwrap();
                }
            }
            assert!(
                ordinary_paged,
                "ordinary producer must exhibit the same unfinished filename cursor"
            );
        }

        #[test]
        fn configured_semantic_domain84_native_frontier_vm_work_is_indexed() {
            let (f, foreign_config, options, native_inputs) = model_sources();
            let inputs = peer_inputs(&options[1], 2, "claude72-1");
            let mut admitted = open(&f.config);
            let _foreign = open(&foreign_config);
            claude_ingest(&mut admitted, &options[1], &inputs);
            admitted
                .ingest(
                    &format!("{:064x}", 8499),
                    &native_inputs[0].full_path(&options[0]),
                    "codex",
                )
                .unwrap();
            let runtime = crate::native_evidence::query_runtime_generation(&options[1]).unwrap();
            admitted
                .begin_semantic_stage_with_journal(
                    &[inputs[0]
                        .full_path(&options[1])
                        .to_string_lossy()
                        .into_owned()],
                    &runtime,
                    true,
                )
                .unwrap();
            let db = f.db();
            for name in ["semon_event_writer", "semon_semantic_writer"] {
                db.create_scalar_function(
                    name,
                    0,
                    FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
                    |_| Ok(1_i64),
                )
                .unwrap();
            }
            let bind = |namespace: &str| {
                bind_metadata_scope(&db, namespace).unwrap();
                let namespace = namespace.to_owned();
                db.create_scalar_function(
                    "semon_namespace",
                    0,
                    FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
                    move |_| Ok(namespace.clone()),
                )
                .unwrap();
            };
            bind("source-a");
            // Stress-only unrelated physical history, never endpoints, model
            // witnesses or positive authority. Positive files/header above
            // were produced by real Raw ingest through native parsers.
            let columns = db
                .prepare("SELECT name FROM pragma_table_info('files') ORDER BY cid")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let values = columns
                .iter()
                .map(|name| match name.as_str() {
                    "namespace" => "?1".to_owned(),
                    "file_id" => "?2".to_owned(),
                    "path" => "?3".to_owned(),
                    _ => format!("\"{name}\""),
                })
                .collect::<Vec<_>>()
                .join(",");
            let insert = format!(
                "INSERT INTO files SELECT {values} FROM files WHERE namespace='source-a' ORDER BY file_id LIMIT 1"
            );
            let mut baseline: Option<Vec<(String, usize)>> = None;
            let mut previous = 0;
            for count in [0, 256, 2048] {
                db.execute_batch("BEGIN IMMEDIATE").unwrap();
                for namespace in ["source-a", "source-b"] {
                    bind(namespace);
                    for i in previous..count {
                        let path = if i % 2 == 0 {
                            format!("/unrelated/projects/noise-{i}/noise-{i}.jsonl")
                        } else {
                            format!("/unrelated/session-state/noise-{i}/events.jsonl")
                        };
                        db.execute(&insert, params![namespace, 10000 + i, path])
                            .unwrap();
                        db.execute("INSERT INTO session_catalog_invalidations(namespace,source_path,revision,reason,observed_at) VALUES(?1,?2,1,'changed',0)",params![namespace,format!("/unrelated/invalidations/noise-{i}.jsonl")]).unwrap();
                        db.execute("INSERT INTO meta(namespace,key,value) VALUES(?1,?2,?3)",params![namespace,format!("native_codex_header:/unrelated/noise-{i}"),format!(r#"{{"header":{{"id":"noise-{i}","metadata":{{"parent_thread":"noise-parent-{i}"}}}}}}"#)]).unwrap();
                    }
                }
                bind("source-a");
                db.execute_batch("COMMIT").unwrap();
                let measured = ["absent84", "claude72-1", "model-0"]
                    .into_iter()
                    .flat_map(|id| {
                        crate::storage::sqlite::semantic::native_frontier_vm_probe(
                            &db, "source-a", id,
                        )
                    })
                    .collect::<Vec<_>>();
                eprintln!("native_frontier_vm unrelated_same_and_foreign={count}: {measured:?}");
                if let Some(first) = &baseline {
                    for ((kind, steps), (_, initial)) in measured.iter().zip(first.iter()) {
                        assert!(
                            *steps <= *initial + 64,
                            "{kind}: unrelated rows increased VM work from {initial} to {steps}"
                        );
                    }
                } else {
                    baseline = Some(measured.clone());
                }
                assert!(
                    measured
                        .iter()
                        .filter(|(kind, _)| !kind.starts_with("setup:"))
                        .all(|(_, steps)| *steps > 0 && *steps < 4096)
                );
                previous = count;
            }
        }

        #[test]
        fn configured_semantic_domain81_peer_changed_raw_claim_refuses_publication_zero_effects() {
            let (f, _, options, _) = model_sources();
            let options = &options[1];
            let inputs = peer_inputs(options, 2, "claude72-1");
            let mut cache = open(&f.config);
            let (batch, model) = claude_ready_from(&mut cache, options, &inputs, &inputs[..1]);
            let target = inputs[1].full_path(options);
            let mut raw = fs::read_to_string(&target).unwrap();
            raw.push_str(&format!("{}\n",serde_json::json!({"type":"user","sessionId":"claude72-1","uuid":"peer81-claim-change","timestamp":"2026-10-01T00:04:00Z","message":{"role":"user","content":"changed real Raw"}})));
            fs::write(&target, raw).unwrap();
            cache
                .ingest(&format!("{:064x}", 8199), &target, "claude")
                .unwrap();
            let before = shared_state(&f);
            assert_eq!(
                cache
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                Outcome::Conflict
            );
            assert_eq!(
                shared_state(&f),
                before,
                "changed discovered claim cannot publish Current, History, proofs or ACK"
            );
        }
    }
}

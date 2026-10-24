//! Configured native transaction with a composite core Event family. The global/multi-source and
//! Catalog publisher gates remain closed. Parser/identity logic stays in events.
use super::*;
use crate::storage::sqlite::source_journal::SourceJournal;
use crate::storage::{StorageError, StoreConfig, native_events::EventReceipt};
use rusqlite::functions::FunctionFlags;

const CORE_TABLES: &[&str] = &["files", "events", "signals"];

// Authored separately from the transitional Options schema. There are no
// namespace defaults, views, SQL rewriting, or implicit namespace row adoption.
const CORE_SCHEMA: &str = "
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

/// Select an authored statement for the physical family shape, never rewrite
/// SQL or retry errors against another schema. Singleton stores still support
/// read-only admission/assignment; configured writes require validate_core.
pub(super) fn sql<'a>(
    db: &Connection,
    legacy: &'a str,
    scoped: &'a str,
) -> rusqlite::Result<&'a str> {
    let composite: bool = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_table_info('files') WHERE name='namespace' AND pk=1)")?.query_row([], |r| r.get(0))?;
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
    let composite: bool = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_table_info('meta') WHERE name='namespace' AND pk=1)")?.query_row([], |r| r.get(0))?;
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
];
const STORE_METADATA_KEYS: &[&str] = &[
    "cache_version",
    "catalog_changes_epoch",
    "catalog_semantic_recipe",
    "native_tree_repair_clock",
];

fn metadata_namespace<'a>(namespace: &'a str, key: &str) -> Result<&'a str, StorageError> {
    if STORE_METADATA_KEYS.contains(&key) {
        Ok(STORE_METADATA_NAMESPACE)
    } else if !namespace.is_empty()
        && SOURCE_METADATA_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix) && key.len() > prefix.len())
    {
        Ok(namespace)
    } else {
        Err(StorageError::InvalidArguments)
    }
}

fn bind_metadata_scope(db: &Connection, namespace: &str) -> rusqlite::Result<()> {
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
    for ddl in storage_ddl() {
        let ddl = if ddl == LEGACY_EVENT_SCHEMA {
            CORE_SCHEMA
        } else if ddl == reprice::FILE_SCHEMA {
            PRICING_FILE_SCHEMA
        } else if ddl == semantic_stage::SEMANTIC_CORE_INDEXES {
            semantic_stage::SEMANTIC_CORE_INDEXES_NS
        } else {
            ddl
        };
        tx.execute_batch(ddl).map_err(failed)?;
    }
    tx.execute(
        "INSERT INTO meta(key,value) VALUES('cache_version',?1)",
        [CACHE_VERSION.to_string()],
    )
    .map_err(failed)?;
    tx.execute(
        "UPDATE meta SET value=?1 WHERE key='catalog_changes_epoch'",
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
pub(super) fn effect(name: &'static str) -> rusqlite::Result<()> {
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
    bind_metadata_scope(&db, config.namespace.as_str()).map_err(failed)?;
    db.pragma_update(None, "foreign_keys", true)
        .map_err(failed)?;
    db.create_scalar_function(
        "semon_event_writer",
        0,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(failed)?;
    Ok(db)
}

pub(super) fn validate(db: &Connection, config: &StoreConfig) -> Result<(), StorageError> {
    SourceJournal(config.clone()).validate(db)?;
    validate_core(db)?;
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

/// Constant-size preparation of a fresh composite core. Existing singleton
/// stores refuse before writes; supported-current bounded migration is pending.
pub(crate) fn prepare(config: &StoreConfig) -> Result<(), StorageError> {
    let mut db = connection(config)?;
    SourceJournal(config.clone()).validate(&db)?;
    validate_core(&db)?;
    SourceJournal(config.clone()).prepare()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    SourceJournal(config.clone()).validate(&tx)?;
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
    let ns = config.namespace.as_str().replace('\'', "''");
    tx.execute_batch(&format!(
        "ALTER TABLE meta ADD COLUMN namespace TEXT NOT NULL DEFAULT '{ns}';"
    ))
    .map_err(failed)?;
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
                format!(
                    " OR {record}.namespace IS NOT semon_namespace() OR NOT ({record}.key GLOB 'native_*' OR {record}.key GLOB 'archive_event_index:*')"
                )
            } else if CORE_TABLES.contains(table) {
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
    intents: BTreeMap<String, Intent>,
}

impl Writer {
    pub(crate) fn new(config: &StoreConfig) -> Self {
        Self {
            config: config.clone(),
            intents: BTreeMap::new(),
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
        validate(&db, &self.config)?;
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
        validate(&tx, &self.config)?;
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
        validate(&tx, &self.config)?;
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
            if changes.is_none()
                && let Some((id, _)) = ledger_row(&tx, path).map_err(failed)?
            {
                clear(&tx, id).map_err(failed)?;
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
    fn private_composite_meta_proofs_collide_without_crossing_namespaces() {
        // Only the metadata/proof family is under test. Catalog ownership,
        // public enrollment and the complete native writer remain unqualified.
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(metadata_schema(true)).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(crate::history_projection::SCHEMA).unwrap();
        db.execute_batch(semantic_stage::metadata_indexes(true)).unwrap();
        let bind = |db: &Connection, namespace: &str| {
            bind_metadata_scope(db, namespace).unwrap();
            let namespace = namespace.to_owned();
            db.create_scalar_function("semon_namespace", 0, FunctionFlags::SQLITE_UTF8, move |_| Ok(namespace.clone())).unwrap();
        };
        let state = |db: &Connection, namespace: &str| -> Vec<(String, String)> {
            db.prepare("SELECT key,value FROM meta WHERE namespace=?1 ORDER BY key")
                .unwrap()
                .query_map([namespace], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        for index in ["sqlite_autoindex_meta_1", "semantic_codex_header_native", "semantic_codex_header_parent"] {
            let first: String = db.query_row("SELECT name FROM pragma_index_xinfo(?1) WHERE seqno=0", [index], |r| r.get(0)).unwrap();
            assert_eq!(first, "namespace", "{index}");
        }
        assert!(crate::storage::LogicalSourceNamespace::new(STORE_METADATA_NAMESPACE).is_err());
        assert_eq!(metadata_namespace("a", "native_unclassified:original"), Err(StorageError::InvalidArguments));
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
            stat: Stat { dev: 1, ino: 2, size: offset, modified_ns: 3, changed_ns: Some(4) },
            offset, prefix: [5; 32], tail: [6; 32],
        };
        for (namespace, marker) in [("a", 11), ("b", 22)] {
            bind(&db, namespace);
            let chunks = crate::source_chunks::Chunks { offset, hashes: vec![[marker; 32]; 1025], resume: None };
            let mut calls = crate::native_handoff::Index::default();
            let value = serde_json::json!({"message":{"content":[{"type":"tool_use","id":namespace,"name":"Bash","input":{"command":"codex exec"}}]}});
            let bytes = format!("{value}\n");
            calls.observe(bytes.as_bytes(), 0, true, &value).unwrap();
            let tx = db.transaction().unwrap();
            chunk_proofs::write(&tx, path, &ledger, Some(&chunks)).unwrap();
            launch_calls::write(&tx, path, &ledger, Some(&calls)).unwrap();
            tx.execute(native_metadata_put(&tx).unwrap(), params![format!("native_tree_failure:{path}"), namespace]).unwrap();
            // The reserved store scope is bound by the classified key, never
            // supplied by DEFAULT or adopted from the first source.
            tx.execute(native_metadata_put(&tx).unwrap(), params!["catalog_semantic_recipe", "one-store-recipe"]).unwrap();
            tx.commit().unwrap();
            assert_eq!(chunk_proofs::read(&db, path, &ledger).unwrap().unwrap().as_ref(), &chunks);
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
        }).to_string();
        db.execute("INSERT INTO session_catalog(session_key,lifecycle,last_ms,harness,metadata) VALUES('owner','current',0,'codex',?1)", [published]).unwrap();
        db.execute("INSERT INTO session_catalog_sources VALUES('owner','current','codex','owner',?1)", [path]).unwrap();
        bind(&db, "a");
        {
            let tx = db.transaction().unwrap();
            chunk_proofs::clear(&tx, path).unwrap();
            let retained: i64 = tx.query_row("SELECT count(*) FROM meta WHERE namespace=?1 AND key GLOB 'native_published_chunks:*'", ["a"], |r| r.get(0)).unwrap();
            assert!(retained >= 3, "header and both proof pages must be retained");
            assert_eq!(state(&tx, "b"), b);
            // Rollback must restore the live proofs and remove retention copies.
        }
        assert_eq!(state(&db, "b"), b);
        db.execute("DELETE FROM session_catalog_sources", []).unwrap();
        db.execute("DELETE FROM session_catalog", []).unwrap();
        let global = state(&db, STORE_METADATA_NAMESPACE);
        assert_eq!(global.len(), 1);
        bind(&db, "a");
        let a = state(&db, "a");
        assert!(db.execute(native_metadata_put(&db).unwrap(), params!["native_unclassified:original", "refuse"]).is_err());
        assert_eq!(state(&db, "a"), a);
        {
            let tx = db.transaction().unwrap();
            chunk_proofs::clear(&tx, path).unwrap();
            launch_calls::clear(&tx, path).unwrap();
            tx.execute(native_metadata_delete(&tx).unwrap(), [format!("native_tree_failure:{path}")]).unwrap();
            assert!(state(&tx, "a").is_empty());
            assert_eq!(state(&tx, "b"), b);
            assert_eq!(state(&tx, STORE_METADATA_NAMESPACE), global);
            // Dropping restores source A's headers, pages and tree witnesses.
        }
        assert_eq!(state(&db, "a"), a);
        let tx = db.transaction().unwrap();
        chunk_proofs::clear(&tx, path).unwrap();
        launch_calls::clear(&tx, path).unwrap();
        tx.execute(native_metadata_delete(&tx).unwrap(), [format!("native_tree_failure:{path}")]).unwrap();
        tx.commit().unwrap();
        assert!(chunk_proofs::read(&db, path, &ledger).unwrap().is_none());
        assert!(launch_calls::read(&db, path, &ledger).unwrap().is_none());
        assert_eq!(state(&db, "b"), b);
        assert_eq!(state(&db, STORE_METADATA_NAMESPACE), global);
        bind(&db, "b");
        assert_eq!(chunk_proofs::read(&db, path, &ledger).unwrap().unwrap().hashes, vec![[22; 32]; 1025]);
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
                    .query_row("SELECT name FROM pragma_index_xinfo(?1) WHERE seqno=0 AND key=1", [&name], |r| r.get(0))
                    .unwrap();
                assert_eq!(first.as_deref(), Some("namespace"), "{name}: {definition}");
                semantic_indexes += usize::from(name.starts_with("semantic_"));
            } else if name.starts_with("pricing_pending_file_") {
                assert!(definition.contains("namespace=NEW.namespace"), "{name}");
            } else {
                assert!(name.starts_with("storage_namespace_fence_"), "unexpected core trigger {name}");
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
        let mut writer = f.config.native_events().unwrap();
        writer.ingest(&"a".repeat(64), &f.raw, "codex").unwrap();
        let before = f.state();
        assert!(old_write.execute([f.raw.to_str().unwrap()]).is_err());
        assert_eq!(f.state(), before);
        assert_eq!(
            crate::storage::sqlite::namespace::WRITER_QUALIFICATION_VERSION,
            0
        );
    }

    struct Fixture {
        root: PathBuf,
        config: StoreConfig,
        raw: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
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
                namespace: LogicalSourceNamespace::new("configured-events").unwrap(),
                expected_stream_epoch: Some("a".repeat(64)),
            };
            config.create_native_store().unwrap();
            let StoreBackend::Sqlite { database } = &config.backend;
            let db = Connection::open(database).unwrap();
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
            let raw = root.join("native.jsonl");
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
                .map(|table| {
                    let mut statement = db
                        .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
                        .unwrap();
                    let columns = statement.column_count();
                    let rows = statement
                        .query_map([], |r| (0..columns).map(|i| r.get(i)).collect())
                        .unwrap()
                        .collect::<rusqlite::Result<_>>()
                        .unwrap();
                    (table.to_owned(), rows)
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
        drop(writer);
        fs::remove_file(&f.raw).unwrap();
        let mut restarted = f.config.native_events().unwrap();
        let replay = restarted.ingest(&"b".repeat(64), &f.raw, "codex").unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.operation_id, second.operation_id);
        assert_eq!(replay.consumed_bytes, second.consumed_bytes);
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

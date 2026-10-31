//! The native event index persisted in SQLite: `sessions-index.sqlite3`.
//!
//! It holds the metadata represented by [`FileIndex`], versioned stable session
//! descriptions and a focused-read catalog: kinds, offsets, ids, counts, flags
//! and native metadata, never message or tool text (risk:secret). Every
//! file's change commits in one `BEGIN IMMEDIATE` transaction together with
//! its ledger row, so a file's resume offset never runs ahead of its rows,
//! and a writer re-reads the ledger inside that transaction, so two
//! processes never apply the same lines twice.
//!
//! Rows are written and read by destructuring each struct in full, never
//! with `..`: a field added to [`FileIndex`], [`Event`] or the rest fails to
//! compile here until it is persisted. The rule, shown on a stand-in type:
//!
//! ```compile_fail,E0027
//! struct Row { kept: u8, added: u8 }
//! let row = Row { kept: 1, added: 2 };
//! // Adding `added` to `Row` breaks this line until it is written too.
//! let Row { kept } = row;
//! ```

pub(crate) mod chunk_proofs;
pub(crate) mod configured;
mod launch_calls;
pub(crate) mod native_commit;
pub(crate) mod native_decode;
pub(crate) mod native_usage;
mod relocation;
pub(crate) mod reprice;
pub(crate) mod semantic_stage;
pub(crate) mod tree_state;

use std::{
    collections::BTreeMap,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Row, Statement, Transaction,
    TransactionBehavior, params, types::Type,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::native_aux::{ClaudeSidecarRecord, CodexHeaderRecord, ParentMarkerRecord};
#[cfg(test)]
use super::{BillingUsage, Peer, Reply, Yield};
use super::{
    CACHE_VERSION, CodexUsageEvent, Event, FileIndex, IndexStore, Ledger, Loaded, MessageUsage,
    ModelTokens, Outcome, ReportedFileStamp, SESSION_DESCRIPTION_BATCH_LIMIT,
    SessionDescriptionRequest, Signal, Stat, StoreError,
};
use crate::storage::StorageError;
use crate::storage::sqlite::source_journal;
use crate::{Tokens, facts::ReportedRunSnapshot};

#[cfg(test)]
std::thread_local! {
    static BEFORE_FRESH_WAL: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_STORAGE_INIT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static BEFORE_SOURCE_INVALIDATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
std::thread_local! {
    static CATALOG_PUBLISH_DIAGNOSTICS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    static CATALOG_PUBLISH_DIAGNOSTIC_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

const CATALOG_PUBLISH_LOG_BURST: u8 = 8;
const CATALOG_PUBLISH_LOGS_PER_ATTEMPT: usize = 8;
const CATALOG_PUBLISH_LOG_INTERVAL: Duration = Duration::from_secs(10);
const CATALOG_PUBLISH_LOG_REFILL: Duration = Duration::from_millis(1250);

#[derive(Default)]
struct CatalogPublishLogLimiter {
    last: BTreeMap<(String, String, String), (Instant, u64)>,
    next_order: u64,
    tokens: u8,
    last_refill: Option<Instant>,
}

impl CatalogPublishLogLimiter {
    fn refill(&mut self, now: Instant) {
        let Some(last_refill) = self.last_refill else {
            self.tokens = CATALOG_PUBLISH_LOG_BURST;
            self.last_refill = Some(now);
            return;
        };
        let intervals = now.saturating_duration_since(last_refill).as_nanos()
            / CATALOG_PUBLISH_LOG_REFILL.as_nanos();
        if intervals > 0 {
            self.tokens = self
                .tokens
                .saturating_add(intervals.min(u128::from(CATALOG_PUBLISH_LOG_BURST)) as u8)
                .min(CATALOG_PUBLISH_LOG_BURST);
            self.last_refill = Some(now);
        }
    }

    fn allow(&mut self, source: &str, reason: &str, stage: &str, now: Instant) -> bool {
        self.refill(now);
        let key = (source.to_owned(), reason.to_owned(), stage.to_owned());
        if self.last.get(&key).is_some_and(|(at, _)| {
            now.saturating_duration_since(*at) < CATALOG_PUBLISH_LOG_INTERVAL
        }) || self.tokens == 0
        {
            return false;
        }
        if self.last.len() >= 256 && !self.last.contains_key(&key) {
            let oldest = self
                .last
                .iter()
                .min_by_key(|(_, (_, order))| *order)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.last.remove(&oldest);
            }
        }
        self.tokens -= 1;
        self.next_order = self.next_order.wrapping_add(1);
        self.last.insert(key, (now, self.next_order));
        true
    }
}

fn emit_catalog_publish_diagnostic(line: String) {
    eprintln!("{line}");
    #[cfg(test)]
    CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow_mut().push(line));
}

fn catalog_publish_conflict_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some(
            "publication_generation"
            | "prepared_generation"
            | "catalog_generation"
            | "completion_generation"
            | "stage_epoch",
        ) => "collection_generation",
        Some(
            "source_identity" | "source_ledger_missing" | "completion_ledger" | "withdrawal_proof",
        ) => "source_ledger",
        Some("publication_claim" | "prepared_claim" | "witness_claim" | "completion_claim") => {
            "claim"
        }
        Some(
            "closed_scope"
            | "source_membership"
            | "unclaimed_source"
            | "source_missing_without_removal"
            | "owner_budget"
            | "owner_scope"
            | "stage_owned",
        ) => "closure",
        _ => "other",
    }
}

fn log_catalog_publish_attempt(
    connection: &Connection,
    scope: &Path,
    rows: &[crate::model::summary::CatalogRow],
    result: &Result<Outcome, StoreError>,
    rejection: Option<&str>,
) {
    static LAST: OnceLock<Mutex<CatalogPublishLogLimiter>> = OnceLock::new();
    let interests = crate::source_carrier::diagnostic_publish_interests(scope);
    let mut allow = |source: &str, reason: &str, stage: &str, now: Instant| {
        LAST.get_or_init(|| Mutex::new(CatalogPublishLogLimiter::default()))
            .lock()
            .is_ok_and(|mut limiter| limiter.allow(source, reason, stage, now))
    };
    let _ = emit_catalog_publish_attempt(
        connection,
        &interests,
        rows,
        result,
        rejection,
        &mut allow,
        Instant::now(),
    );
}

fn emit_catalog_publish_attempt(
    connection: &Connection,
    interests: &[crate::source_carrier::CatalogPublishInterest],
    rows: &[crate::model::summary::CatalogRow],
    result: &Result<Outcome, StoreError>,
    rejection: Option<&str>,
    allow: &mut impl FnMut(&str, &str, &str, Instant) -> bool,
    now: Instant,
) -> usize {
    let (outcome, reason) = match result {
        Ok(Outcome::Written) => ("published", "none"),
        Ok(Outcome::Conflict) => ("conflict", catalog_publish_conflict_reason(rejection)),
        _ => ("other", "other"),
    };
    let mut emitted = 0;
    for interest in interests {
        if emitted >= CATALOG_PUBLISH_LOGS_PER_ATTEMPT {
            break;
        }
        let stage = "catalog_publish";
        if !allow(&interest.source_hash, reason, stage, now) {
            continue;
        }
        // Both the query and candidate traversal are gated. No batch path
        // hashing or database work is done for a suppressed interest.
        #[cfg(test)]
        CATALOG_PUBLISH_DIAGNOSTIC_WORK.with(|work| work.set(work.get() + 1));
        let index = ledger_row(connection, &interest.path)
            .ok()
            .flatten()
            .map(|(_, ledger)| ledger);
        let index_hash = index.as_ref().map(crate::source_carrier::ledger_digest);
        let candidate = rows
            .iter()
            .flat_map(|row| &row.sources)
            .find(|source| source.path.as_path() == Path::new(&interest.path))
            .map(crate::source_carrier::ledger);
        let candidate = candidate.map_or_else(
            || "candidate=absent".to_owned(),
            |ledger| {
                format!(
                    "candidate_catalog_ledger_digest={} candidate_offset={}",
                    &crate::source_carrier::ledger_digest(&ledger)[..12],
                    ledger.offset
                )
            },
        );
        emit_catalog_publish_diagnostic(format!(
            "semon native catalog: outcome={outcome} reason={reason} stage={stage} source_hash={} ledger_digest_domain=ledger-v1 selected_catalog_ledger_digest={} selected_catalog_offset={} {candidate} index_ledger_digest={} index_offset={:?} sample=independent_statements",
            &interest.source_hash[..12],
            &interest.catalog_ledger_digest[..12],
            interest.catalog_offset,
            index_hash.as_deref().map_or("missing", |hash| &hash[..12]),
            index.as_ref().map(|ledger| ledger.offset)
        ));
        emitted += 1;
    }
    emitted
}

/// `PRAGMA user_version`: the shape of the tables. The parser's version is
/// [`CACHE_VERSION`], kept in `meta`.
// Contract fence: namespace-owned state includes the NOCASE scheduling indexes.
// Unsupported stores refuse without reset; current stores do not run storage DDL.
pub(crate) const SCHEMA_VERSION: i64 = 34;

/// How long a write waits for another process's transaction. Tests wait
/// less, so the busy paths they drive stay quick.
const BUSY_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(5)
};

/// What the WAL is cut back to after a checkpoint: a rebuild grows it to
/// about the store's size, and it would otherwise stay that large.
const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

macro_rules! source_observation_receipts_ddl {
    () => {
        "
CREATE TABLE IF NOT EXISTS source_observation_receipts (
    namespace TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('applied','conflict')),
    recovery_done INTEGER CHECK(recovery_done IN (0,1)),
    PRIMARY KEY(namespace,operation_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS source_observation_intents (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(namespace,operation_id)
) STRICT;
CREATE INDEX IF NOT EXISTS source_observation_intents_pending ON source_observation_intents(namespace,sequence);
"
    };
}
pub(crate) const SOURCE_OBSERVATION_RECEIPTS_SCHEMA: &str = source_observation_receipts_ddl!();

const COMPOSITE_CATALOG_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_catalog (
    namespace TEXT NOT NULL,
    session_key TEXT NOT NULL,
    lifecycle TEXT NOT NULL DEFAULT 'current' CHECK(lifecycle IN ('current','retained')),
    last_ms INTEGER NOT NULL,
    harness TEXT NOT NULL,
    repo TEXT,
    parent_key TEXT,
    metadata TEXT NOT NULL,
    pricing_fingerprint TEXT,
    PRIMARY KEY(namespace,session_key)
) STRICT;
CREATE INDEX IF NOT EXISTS session_catalog_order ON session_catalog(namespace,last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness ON session_catalog(namespace,harness, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_repo ON session_catalog(namespace,repo, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness_repo ON session_catalog(namespace,harness, repo, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_parent ON session_catalog(namespace,parent_key, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_current_order ON session_catalog(namespace,last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness ON session_catalog(namespace,harness, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_repo ON session_catalog(namespace,repo, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness_repo ON session_catalog(namespace,harness, repo, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_parent ON session_catalog(namespace,parent_key, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_order_model ON session_catalog(namespace,json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness_model ON session_catalog(namespace,harness, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_repo_model ON session_catalog(namespace,repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness_repo_model ON session_catalog(namespace,harness, repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_current_order_model ON session_catalog(namespace,json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness_model ON session_catalog(namespace,harness, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_repo_model ON session_catalog(namespace,repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness_repo_model ON session_catalog(namespace,harness, repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_query_last ON session_catalog(namespace,json_extract(metadata,'$.query.last'),session_key);
CREATE INDEX IF NOT EXISTS session_catalog_query_current_state ON session_catalog(namespace,json_extract(metadata,'$.query.state'),session_key) WHERE lifecycle='current';
CREATE TABLE IF NOT EXISTS session_catalog_sources (
    namespace TEXT NOT NULL,
    session_key TEXT NOT NULL,
    lifecycle TEXT NOT NULL DEFAULT 'current' CHECK(lifecycle IN ('current','retained')),
    harness TEXT NOT NULL,
    native_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    PRIMARY KEY(namespace, session_key, source_path),
    FOREIGN KEY(namespace,session_key) REFERENCES session_catalog(namespace,session_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_catalog_native ON session_catalog_sources(namespace,harness, native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_native_aliases ON session_catalog_sources(namespace,native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_source_path ON session_catalog_sources(namespace,source_path, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_current_native ON session_catalog_sources(namespace,lifecycle, harness, native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_current_source_path ON session_catalog_sources(namespace,lifecycle, source_path, session_key);
";

pub(crate) const FILE_COLUMNS: &str = "file_id, path, dev, ino, size, mtime_ns, resume_at, head_sha256, \
     tail_sha256, entrypoint, title, agent_name, last_model, failed, first_ms, last_ms, cwd, \
     branch, pending, tool_ids, yields, busy, links, codex_tokens, codex_tokens_by_model, \
     rate_limits, codex_native_usage, copilot, ctime_ns";

pub(crate) const EVENT_COLUMNS: &str = "extra, seq, k, o, b, t, id, n, code_mode, code_mode_open, parent, \
     script, y, poll, reply, reply_o, reply_b, reply_t, reply_e, reply_f, reply_m, item, peer, \
     peer_sock, peer_pid, peer_start, u";

const COMPOSITE_COPIED_USAGE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS usage_identity (
    namespace TEXT NOT NULL,
    file_id INTEGER NOT NULL,
    message_id TEXT NOT NULL,
    record_uuid TEXT NOT NULL,
    PRIMARY KEY(namespace, file_id, message_id, record_uuid),
    FOREIGN KEY(namespace,file_id) REFERENCES files(namespace,file_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS usage_identity_peers ON usage_identity(namespace, message_id, record_uuid, file_id);
CREATE TABLE IF NOT EXISTS usage (
    namespace TEXT NOT NULL,
    file_id INTEGER NOT NULL,
    message_id TEXT NOT NULL,
    model TEXT,
    input INTEGER NOT NULL,
    cached_input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    reasoning_output INTEGER NOT NULL,
    total INTEGER NOT NULL,
    model_input INTEGER NOT NULL,
    model_output INTEGER NOT NULL,
    model_cache_write INTEGER NOT NULL,
    model_cache_read INTEGER NOT NULL,
    billing_input INTEGER NOT NULL,
    billing_output INTEGER NOT NULL,
    billing_cache_read INTEGER NOT NULL,
    billing_cache_write_5m INTEGER NOT NULL,
    billing_cache_write_1h INTEGER NOT NULL,
    billing_web_search_requests INTEGER NOT NULL,
    billing_speed TEXT,
    billing_service_tier TEXT,
    billing_prompt_size INTEGER NOT NULL,
    billing_timestamp INTEGER,
    billing_split_unknown INTEGER NOT NULL,
    record_ids TEXT NOT NULL,
    PRIMARY KEY (namespace, file_id, message_id),
    FOREIGN KEY(namespace,file_id) REFERENCES files(namespace,file_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS codex_usage (
    namespace TEXT NOT NULL,
    file_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    model TEXT NOT NULL,
    input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    t INTEGER,
    PRIMARY KEY (namespace, file_id, seq),
    FOREIGN KEY(namespace,file_id) REFERENCES files(namespace,file_id)
) STRICT, WITHOUT ROWID;

";

const CATALOG_SEARCH_SCHEMA: &str = "
CREATE INDEX IF NOT EXISTS catalog_search_native ON session_catalog_sources(namespace,native_id COLLATE NOCASE,source_path) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS catalog_search_pending_filename ON session_catalog_invalidations(namespace,substr(source_path,length(rtrim(source_path,replace(source_path,'/','')))+1) COLLATE NOCASE,source_path);
CREATE INDEX IF NOT EXISTS catalog_search_pending_copilot ON session_catalog_invalidations(namespace,substr(substr(source_path,1,length(source_path)-13),length(rtrim(substr(source_path,1,length(source_path)-13),replace(substr(source_path,1,length(source_path)-13),'/','')))+1) COLLATE NOCASE,source_path) WHERE source_path GLOB '*/session-state/*/events.jsonl';
CREATE INDEX IF NOT EXISTS catalog_search_header ON meta(namespace,json_extract(value,'$.header.id') COLLATE NOCASE,key) WHERE key GLOB 'native_codex_header:*' AND json_valid(value);
";

fn copied_usage_schema() -> &'static str {
    COMPOSITE_COPIED_USAGE_SCHEMA
}

fn validate_copied_usage(c: &Connection) -> rusqlite::Result<()> {
    let version: i64 = c.pragma_query_value(None, "schema_version", |r| r.get(0))?;
    let cached: bool = c.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_function_list WHERE name='semon_copied_usage_layout' AND narg=1)")?.query_row([], |r| r.get(0))?;
    if cached {
        let layout: Option<bool> = c
            .prepare_cached("SELECT semon_copied_usage_layout(?1)")?
            .query_row([version], |r| r.get(0))?;
        if layout == Some(true) {
            return Ok(());
        }
    }
    // These tables are one atomic physical family, independent of files and
    // native usage. Reject missing tables and partial namespace keys before DML.
    for (table, keys) in [
        ("usage_identity", "file_id,message_id,record_uuid"),
        ("usage", "file_id,message_id"),
        ("codex_usage", "file_id,seq"),
    ] {
        let actual: String = c.prepare_cached("SELECT coalesce(group_concat(name,','),'') FROM (SELECT name FROM pragma_table_info(?1) WHERE pk>0 ORDER BY pk)")?.query_row([table], |r| r.get(0))?;
        if actual != format!("namespace,{keys}") {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let namespace_valid: bool = c.prepare_cached("SELECT count(*)=1 AND min(pk=1 AND [notnull]=1 AND type='TEXT') FROM pragma_table_info(?1) WHERE name='namespace'")?.query_row([table], |row| row.get(0))?;
        let physical: bool = c
            .prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)",
            )?
            .query_row([table], |r| r.get(0))?;
        if !namespace_valid || !physical {
            return Err(rusqlite::Error::InvalidQuery);
        }
        // Composite payload rows must reference the same physical core key.
        // Refuse a missing, partial or foreign parent constraint, not merely
        // a table with the expected primary-key spelling.
        let foreign_keys = c
            .prepare_cached(
                "SELECT [table],[from],[to] FROM pragma_foreign_key_list(?1) ORDER BY id,seq",
            )?
            .query_map([table], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let expected = vec![
            (
                "files".to_owned(),
                "namespace".to_owned(),
                "namespace".to_owned(),
            ),
            (
                "files".to_owned(),
                "file_id".to_owned(),
                "file_id".to_owned(),
            ),
        ];
        if foreign_keys != expected {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let peer_index: String = c.prepare_cached("SELECT coalesce(group_concat(name,','),'') FROM (SELECT name FROM pragma_index_info('usage_identity_peers') ORDER BY seqno)")?
        .query_row([], |r| r.get(0))?;
    let peer_table: bool = c.prepare_cached("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='index' AND name='usage_identity_peers' AND tbl_name='usage_identity') AND EXISTS(SELECT 1 FROM pragma_index_list('usage_identity') WHERE name='usage_identity_peers' AND [unique]=0 AND partial=0 AND origin='c')")?
        .query_row([], |r| r.get(0))?;
    let expected_index = "namespace,message_id,record_uuid,file_id";
    if !peer_table || peer_index != expected_index {
        return Err(rusqlite::Error::InvalidQuery);
    }
    // Do not cache an uncommitted schema cookie: rollback can reuse it for
    // a different DDL transaction. Existing committed decisions remain usable.
    if !c.is_autocommit() {
        return Ok(());
    }
    c.create_scalar_function(
        "semon_copied_usage_layout",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        move |context| {
            let observed: i64 = context.get(0)?;
            Ok((observed == version).then_some(true))
        },
    )?;
    Ok(())
}

// The current schema is qualified before constructing the transaction writer.
// Retain its schema cookie so DDL inside this transaction invalidates the writer.
struct WriterLayout<'tx, 'connection> {
    transaction: &'tx Transaction<'connection>,
    schema_version: i64,
}

impl<'tx, 'connection> WriterLayout<'tx, 'connection> {
    fn new(transaction: &'tx Transaction<'connection>) -> rusqlite::Result<Self> {
        configured::validate_statement_layout(transaction)?;
        validate_copied_usage(transaction)?;
        semantic_stage::validate_support(transaction)?;
        let schema_version =
            transaction.pragma_query_value(None, "schema_version", |row| row.get(0))?;
        Ok(Self {
            transaction,
            schema_version,
        })
    }

    #[cfg(test)]
    fn copied_usage_only(transaction: &'tx Transaction<'connection>) -> rusqlite::Result<Self> {
        Self::new(transaction)
    }

    fn ensure_current(&self) -> rusqlite::Result<()> {
        let observed: i64 = self
            .transaction
            .pragma_query_value(None, "schema_version", |row| row.get(0))?;
        if observed == self.schema_version {
            Ok(())
        } else {
            Err(rusqlite::Error::InvalidQuery)
        }
    }
}

pub(crate) const SIGNAL_COLUMNS: &str = "seq, k, o, t, at, n, v";

pub(crate) const USAGE_COLUMNS: &str = "message_id, model, input, cached_input, output, reasoning_output, \
     total, model_input, model_output, model_cache_write, model_cache_read, billing_input, \
     billing_output, billing_cache_read, billing_cache_write_5m, billing_cache_write_1h, \
     billing_web_search_requests, billing_speed, billing_service_tier, billing_prompt_size, \
     billing_timestamp, billing_split_unknown, record_ids";

const COMPOSITE_PUT_USAGE: &str = "INSERT OR REPLACE INTO usage (namespace, file_id, message_id, model, input, \
     cached_input, output, reasoning_output, total, model_input, model_output, \
     model_cache_write, model_cache_read, billing_input, billing_output, billing_cache_read, \
     billing_cache_write_5m, billing_cache_write_1h, billing_web_search_requests, \
     billing_speed, billing_service_tier, billing_prompt_size, billing_timestamp, \
     billing_split_unknown, record_ids) \
     VALUES (semon_namespace(), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
     ?19, ?20, ?21, ?22, ?23, ?24)";

pub(crate) const CODEX_USAGE_COLUMNS: &str =
    "seq, model, input, output, cache_write, cache_read, t";

const COMPOSITE_PUT_CODEX_USAGE: &str = "INSERT OR REPLACE INTO codex_usage (namespace, file_id, seq, model, input, \
     output, cache_write, cache_read, t) VALUES (semon_namespace(), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

pub(crate) const RUN_COLUMNS: &str = "session_id, start, cost, duration, api_duration, tool_duration, \
     lines_added, lines_removed, model_usage, capture_at";

pub(crate) const PUT_RUN_SCOPED: &str = "INSERT OR REPLACE INTO reported_runs (namespace,session_id,start,cost,duration,api_duration,tool_duration,lines_added,lines_removed,model_usage,capture_at) VALUES(semon_namespace(),?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)";

/// The SQLite [`IndexStore`]: one connection, used by one
/// [`super::EventCache`].
struct SqliteStore {
    owner: configured::NativeOwnership,
    connection: Connection,
    path: PathBuf,
    selected_runs: Option<std::collections::BTreeSet<String>>,
    semantic_rejection: Option<&'static str>,
    #[cfg(test)]
    traversal_observation: Option<crate::native_timing::CatalogTraversalProbe>,
}

/// The admitted namespace Source journal is the only recovery/discovery port;
/// the store's physical database never supplies an unscoped journal.
pub(super) type Admitted = (Box<dyn IndexStore>, Loaded, source_journal::SourceJournal);

/// Opens (creating when missing) the store at `path` and reads all of it.
/// Existing unsupported or damaged state is refused without replacement.
/// Admission errors are returned to the caller; storage never falls back.
pub(super) fn open(path: &Path, source: &crate::storage::HostSource) -> Result<Admitted, Unopened> {
    open_admitted(path, source).map_err(|error| Unopened::Other(error.to_string()))
}

fn open_admitted(
    path: &Path,
    source: &crate::storage::HostSource,
) -> Result<Admitted, crate::storage::StorageError> {
    let config = source.config(path)?;
    let path = config.backend.sqlite_database()?.as_path();
    if !path
        .try_exists()
        .map_err(|_| crate::storage::StorageError::Unavailable)?
    {
        configured::create(&config)?;
    }
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&config)?;
    #[cfg(test)]
    if let Some(hook) = BEFORE_STORAGE_INIT.with(|slot| slot.borrow_mut().take()) {
        hook();
    }
    make_existing_private(path, &crate::state_dir())
        .map_err(|_| crate::storage::StorageError::Unavailable)?;
    let journal = capability.journal();
    let (store, stamp) =
        crate::storage::sqlite::configured_index::open_semantic_with_stamp(capability)?;
    Ok((
        store,
        Loaded {
            files: Vec::new(),
            reported_runs: Vec::new(),
            stamp,
        },
        journal,
    ))
}

/// Producer open: initialize the durable store without restoring unrelated indexes.
/// Store failures are returned; a background worker must retry rather than silently
/// switch to an unbounded in-memory rebuild.
pub(super) fn open_scoped(
    path: &Path,
    source: &crate::storage::HostSource,
) -> Result<Admitted, String> {
    open_admitted(path, source).map_err(|error| error.to_string())
}

/// Test mutations use the EventCache's admitted writer connection, including
/// its namespace and private event ownership qualification.
#[cfg(test)]
pub(crate) fn qualified_event_writer_connection_for_test(
    path: &Path,
    source: &crate::storage::HostSource,
) -> Result<Connection, StorageError> {
    configured::connection(&source.config(path)?)
}

/// Restart/read interest can resume durable work without opening Event payloads.
/// An absent store alone does not ask a qualified-read observer to cold-index.
pub(crate) fn catalog_work_pending(options: &crate::Options) -> io::Result<bool> {
    #[cfg(feature = "postgres")]
    if let Some(config) = options.source.selected_store()
        && matches!(
            config.backend,
            crate::storage::StoreBackend::Postgres { .. }
        )
    {
        return crate::storage::postgres::host::catalog_work_pending(config)
            .map_err(io::Error::other);
    }
    let path = super::EventCache::path(&options.cache);
    if !path.exists() {
        return Ok(false);
    }
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = db.unchecked_transaction().map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    let pending: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=semon_namespace()) OR EXISTS(SELECT 1 FROM main.meta WHERE namespace=semon_namespace() AND ((key='catalog_backfill_state' AND value IN ('discovering','draining')) OR (key='catalog_semantic_gc' AND value='1'))) OR EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=semon_namespace())",
        [], |row| row.get(0),
    ).map_err(io::Error::other)?;
    Ok(pending)
}

/// Qualify a former Catalog source as a verified relocation of a current one.
/// The history publisher uses the witness inside the same publication CAS.
pub(crate) fn relocated_catalog_source(
    connection: &Connection,
    old: &crate::model::summary::CatalogSource,
    current: &[crate::model::summary::CatalogSource],
) -> rusqlite::Result<bool> {
    relocation::replaces(connection, old, current)
}

/// Record a completed bounded metadata observation without replacing rows or
/// their publication generation. Pending work or another publication prevents
/// acknowledging this observation.
pub(super) fn note_catalog_audit_tx(
    tx: &Connection,
    expected: Option<&str>,
    now: i64,
) -> rusqlite::Result<bool> {
    let ready: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE namespace=semon_meta_scope('catalog_generation') AND key='catalog_generation' AND value=?1) AND NOT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE namespace=semon_namespace()) AND NOT EXISTS(SELECT 1 FROM catalog_semantic_stage WHERE namespace=semon_namespace())", [expected], |r| r.get(0))?;
    if ready {
        tx.execute(
            "INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope('catalog_observed_at'),'catalog_observed_at',?1)",
            [now.to_string()],
        )
        ?;
    }
    Ok(ready)
}

/// Bounded metadata audit for writers without receiver custody proofs. No
/// Event rows, transcript bytes or semantic session models are loaded here.
pub(crate) fn audit_catalog_sources(
    options: &crate::Options,
    discovered: &[crate::Input],
    after: Option<&str>,
    _runtime: &str,
) -> io::Result<Option<String>> {
    let config = options
        .source
        .config(&options.cache)
        .map_err(io::Error::other)?;
    let owner = configured::NativeOwnership::restore(&config).map_err(io::Error::other)?;
    let runtime_binding = crate::storage::NativeReaderBinding::Semantic(owner.clone())
        .runtime_binding()
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other(crate::storage::StorageError::Unavailable))?;
    let observations = crate::native_evidence::RuntimeObservations::new(options);
    let journal = crate::storage::sqlite::source_journal::SourceJournal::owned(owner);
    let db = journal.connection().map_err(io::Error::other)?;
    let machine = crate::facts::MachineFacts::of(options);
    // Observe Raw metadata in a read snapshot. Acquire the write lock only
    // after all physical observations, when consuming the exact ledgers below.
    let tx =
        Transaction::new_unchecked(&db, TransactionBehavior::Deferred).map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    let mut changed = std::collections::BTreeSet::new();
    let mut absent_paths = std::collections::BTreeSet::new();
    let mut removed = Vec::new();
    let mut source_locks = Vec::new();
    for input in discovered {
        if input.root == crate::InputRoot::Codex
            && !machine.codex_rollout_is_current(options, &input.full_path(options))
        {
            continue;
        }
        if input.path.ends_with(".meta.json") {
            let log = input
                .full_path(options)
                .with_extension("")
                .with_extension("jsonl");
            let key = log.to_string_lossy().into_owned();
            if let Some((_, ledger)) = ledger_row(&tx, &key).map_err(io::Error::other)? {
                let sidecar = read_claude_sidecar(
                    &mut tx
                        .prepare(native_metadata_value(&tx).map_err(io::Error::other)?)
                        .map_err(io::Error::other)?,
                    &key,
                    &ledger,
                )
                .map_err(io::Error::other)?;
                if sidecar.is_none_or(|sidecar| !sidecar.current(&log)) {
                    changed.insert(key);
                }
            }
            continue;
        }
        if !input.path.ends_with(".jsonl") {
            continue;
        }
        let path = input.full_path(options);
        let key = path.to_string_lossy().into_owned();
        let observed = super::Stat::of(&path);
        if observed
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        {
            absent_paths.insert(key.clone());
        }
        let recorded = ledger_row(&tx, &key).map_err(io::Error::other)?;
        if match recorded.as_ref() {
            Some((_, ledger)) => observed.ok().as_ref() != Some(&ledger.stat),
            None => true,
        } {
            changed.insert(key.clone());
        }
        // Discovery enumerates logs; a sidecar can change without the log's
        // stat changing. Check its persisted descriptor in the same bounded page.
        if input.root == crate::InputRoot::Claude
            && crate::native_sidecar::agent_path(&path)
            && let Some((_, ledger)) = recorded
        {
            let sidecar = read_claude_sidecar(
                &mut tx
                    .prepare(native_metadata_value(&tx).map_err(io::Error::other)?)
                    .map_err(io::Error::other)?,
                &key,
                &ledger,
            )
            .map_err(io::Error::other)?;
            if sidecar.is_none_or(|sidecar| !sidecar.current(&path)) {
                changed.insert(key);
            }
        }
    }
    let mut last = None;
    if let Some(after) = after {
        let paths = {
            let mut statement = tx.prepare("SELECT s.source_path, CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM session_catalog_sources s JOIN session_catalog c ON c.namespace=s.namespace AND c.session_key=s.session_key WHERE s.namespace=semon_namespace() AND s.lifecycle='current' AND s.source_path>?1 GROUP BY s.source_path ORDER BY s.source_path LIMIT 64").map_err(io::Error::other)?;
            statement
                .query_map([after], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(io::Error::other)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(io::Error::other)?
        };
        for (path, metadata) in paths {
            let row: crate::model::summary::CatalogRow =
                serde_json::from_str(&metadata).map_err(io::Error::other)?;
            let expected = crate::storage::sqlite::catalog::runtime_source_cwds(&tx, &row.sources)
                .and_then(|cwds| observations.scope(&row, &cwds))
                .and_then(|scoped| runtime_binding.witness(&scoped))
                .ok();
            let saved_runtime = row
                .query
                .as_ref()
                .and_then(|query| query["runtime"].as_str());
            let recorded = ledger_row(&tx, &path).map_err(io::Error::other)?;
            source_locks.push(crate::source_change::SourceLock::acquire(
                options, &path, false,
            )?);
            let absent = fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
            if absent {
                absent_paths.insert(path.clone());
            }
            if absent && let Some((_, ledger)) = &recorded {
                removed.push((path.clone(), Some(ledger.clone())));
            }
            if (row.harness == "codex"
                && !machine.codex_rollout_is_current(options, Path::new(&path)))
                || saved_runtime != expected.as_deref()
                || match recorded.as_ref() {
                    Some((_, ledger)) => {
                        super::Stat::of(Path::new(&path)).ok().as_ref() != Some(&ledger.stat)
                    }
                    None => true,
                }
            {
                changed.insert(path.clone());
            }
            last = Some(path);
        }
    }
    // These are real producer removals, qualified above under Source locks;
    // the existing writer preserves retained indexes and compares each ledger.
    remove_files_tx(&tx, &removed, config.namespace.as_str()).map_err(io::Error::other)?;
    for path in changed {
        if !removed.iter().any(|(removed, _)| removed == &path) {
            // Discovery can have queued this path before its ordinary removal
            // committed. Do not replace that durable negative evidence with a
            // generic change after the ledger is gone. A present/reappeared
            // file still follows the ordinary changed-source path.
            if absent_paths.contains(&path) {
                let removed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1 AND reason='removed')", [&path], |row| row.get(0)).map_err(io::Error::other)?;
                if removed {
                    continue;
                }
            }
            invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
        }
    }
    tx.commit().map_err(io::Error::other)?;
    Ok(last)
}

#[derive(Debug)]
struct CatalogDependencyBudgetExceeded;
impl fmt::Display for CatalogDependencyBudgetExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native dependent budget exceeded")
    }
}
impl std::error::Error for CatalogDependencyBudgetExceeded {}

/// Explicit component overflow is distinct from unavailable or invalid native
/// evidence. Callers can enter resumable publication without masking errors.
pub(crate) fn catalog_dependents_bounded(
    options: &crate::Options,
    selectors: &[(String, String)],
) -> io::Result<Option<Vec<crate::Input>>> {
    match catalog_dependents(options, selectors) {
        Ok(inputs) => Ok(Some(inputs)),
        Err(error)
            if error
                .get_ref()
                .is_some_and(|inner| inner.is::<CatalogDependencyBudgetExceeded>()) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn catalog_dependents(
    options: &crate::Options,
    selectors: &[(String, String)],
) -> io::Result<Vec<crate::Input>> {
    let binding =
        crate::storage::local_navigation_binding(options, "").map_err(io::Error::other)?;
    let db = binding.connection().map_err(io::Error::other)?;
    let tx = db.unchecked_transaction().map_err(io::Error::other)?;
    binding
        .validate(&tx, &binding.config().namespace)
        .map_err(io::Error::other)?;
    let mut found = std::collections::BTreeSet::new();
    let mut statement=tx.prepare("SELECT d.source_path FROM main.catalog_native_dependencies d WHERE d.namespace=semon_namespace() AND harness=?1 AND native_id=?2 AND EXISTS(SELECT 1 FROM main.session_catalog_sources s WHERE s.namespace=d.namespace AND s.source_path=d.source_path AND s.lifecycle='current') LIMIT 65").map_err(io::Error::other)?;
    let expanded = selectors.iter().flat_map(|(harness, id)| {
        [
            (harness.clone(), id.clone()),
            (format!("lineage:{harness}"), id.clone()),
        ]
    });
    for (harness, id) in expanded {
        if harness == "message:claude" {
            // These indexed coordinates are discovery hints. Selected shared
            // native preparation still qualifies source ownership and copies.
            for sql in [
                "SELECT DISTINCT f.path FROM main.events e JOIN main.files f ON f.namespace=e.namespace AND f.file_id=e.file_id WHERE e.namespace=semon_namespace() AND e.extra=0 AND e.k='xsm' AND e.id=?1 AND EXISTS(SELECT 1 FROM main.session_catalog_sources s WHERE s.namespace=f.namespace AND s.source_path=f.path AND s.harness='claude' AND s.lifecycle='current') LIMIT 65",
                "SELECT DISTINCT f.path FROM main.events e JOIN main.files f ON f.namespace=e.namespace AND f.file_id=e.file_id WHERE e.namespace=semon_namespace() AND e.extra=0 AND e.k='tool' AND e.n='SendMessage' AND e.reply=1 AND e.reply_m=?1 AND (e.reply_f & ?2)=0 AND EXISTS(SELECT 1 FROM main.session_catalog_sources s WHERE s.namespace=f.namespace AND s.source_path=f.path AND s.harness='claude' AND s.lifecycle='current') LIMIT 65",
            ] {
                let mut messages = tx.prepare_cached(sql).map_err(io::Error::other)?;
                messages
                    .raw_bind_parameter(1, &id)
                    .map_err(io::Error::other)?;
                if messages.parameter_count() == 2 {
                    messages
                        .raw_bind_parameter(2, crate::events::PIN)
                        .map_err(io::Error::other)?;
                }
                let mut rows = messages.raw_query();
                while let Some(row) = rows.next().map_err(io::Error::other)? {
                    found.insert(row.get::<_, String>(0).map_err(io::Error::other)?);
                    if found.len() > 64 {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            CatalogDependencyBudgetExceeded,
                        ));
                    }
                }
            }
            continue;
        }
        found.extend(
            statement
                .query_map(params![harness, id], |row| row.get::<_, String>(0))
                .map_err(io::Error::other)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(io::Error::other)?,
        );
        if found.len() > 64 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                CatalogDependencyBudgetExceeded,
            ));
        }
    }
    found
        .into_iter()
        .map(|path| {
            crate::native_evidence::catalog_input(Path::new(&path), options)
                .ok_or_else(|| io::Error::other("native dependent outside allowlist"))
        })
        .collect()
}

pub(crate) fn verify_relocation_prefix(
    options: &crate::Options,
    path: &Path,
    stat: &Stat,
    length: u64,
    expected: &str,
) -> io::Result<bool> {
    #[cfg(feature = "postgres")]
    if options.source.selected_store().is_some_and(|config| {
        matches!(
            config.backend,
            crate::storage::StoreBackend::Postgres { .. }
        )
    }) {
        let store = source_write_store(options)?;
        if let Some(ledger) = store
            .ledger(&path.to_string_lossy())
            .map_err(|error| io::Error::other(error.to_string()))?
            && ledger.stat == *stat
            && ledger.offset == length
        {
            return Ok(format!(
                "{:x}",
                sha2::digest::generic_array::GenericArray::from(ledger.prefix)
            ) == expected);
        }
        let prefix = crate::source_change::bytes_hash(path, length, || Ok(()))?;
        return Ok(format!(
            "{:x}",
            sha2::digest::generic_array::GenericArray::from(prefix)
        ) == expected);
    }
    let binding =
        crate::storage::local_navigation_binding(options, "").map_err(io::Error::other)?;
    let db = binding.connection().map_err(io::Error::other)?;
    let tx = db.unchecked_transaction().map_err(io::Error::other)?;
    binding
        .validate(&tx, &binding.config().namespace)
        .map_err(io::Error::other)?;
    if let Some((_, ledger)) = ledger_row(&tx, &path.to_string_lossy()).map_err(io::Error::other)?
        && ledger.stat == *stat
        && ledger.offset == length
    {
        return Ok(format!(
            "{:x}",
            sha2::digest::generic_array::GenericArray::from(ledger.prefix)
        ) == expected);
    }
    drop(tx);
    let prefix = crate::source_change::bytes_hash(path, length, || Ok(()))?;
    Ok(format!(
        "{:x}",
        sha2::digest::generic_array::GenericArray::from(prefix)
    ) == expected)
}

pub(crate) fn complete_relocation(
    options: &crate::Options,
    input: &crate::Input,
    evidence: &crate::source_change::RelocationEvidence,
) -> io::Result<()> {
    #[cfg(feature = "postgres")]
    if let Some(config) = options.source.selected_store()
        && matches!(
            config.backend,
            crate::storage::StoreBackend::Postgres { .. }
        )
    {
        return crate::storage::postgres::host::complete_relocation(
            config, options, input, evidence,
        )
        .map_err(io::Error::other);
    }
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    let path = input.full_path(options).to_string_lossy().into_owned();
    let _ = archived_index(&tx, &path).map_err(io::Error::other)?;
    let _ = archived_index(&tx, &evidence.from_path).map_err(io::Error::other)?;
    tx.execute(
        "INSERT OR REPLACE INTO main.session_source_mutations(namespace,source_path,evidence) VALUES(semon_namespace(),?1,?2)",
        params![
            path,
            serde_json::to_string(evidence).map_err(io::Error::other)?
        ],
    )
    .map_err(io::Error::other)?;
    tx.execute(
        "INSERT OR IGNORE INTO main.catalog_relocation_candidates(namespace,old_path,new_path,native_id) VALUES(semon_namespace(),?1,?2,?3)",
        params![evidence.from_path, path, evidence.native_id],
    )
    .map_err(io::Error::other)?;
    invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    invalidate_bound_catalog_source(&tx, &evidence.from_path, "changed")
        .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

/// Selected upgrade demand shares the producer's durable invalidation queue.
/// Existing revisions stay intact, including an in-flight producer's claim.
pub(crate) fn prioritize_projection_sources(
    options: &crate::Options,
    sources: &[crate::model::summary::CatalogSource],
) -> io::Result<()> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    for source in sources {
        let path = source.path.to_string_lossy();
        let _ = archived_index(&tx, &path).map_err(io::Error::other)?;
    }
    let mut paths = sources
        .iter()
        .map(|source| source.path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let encoded = serde_json::to_string(&paths).map_err(io::Error::other)?;
    crate::storage::source_observations::selected_projection_paths(encoded.as_bytes())
        .map_err(io::Error::other)?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
        params![
            crate::storage::source_observations::SELECTED_PROJECTION_KEY,
            encoded
        ],
    )
    .map_err(io::Error::other)?;
    for source in sources {
        let path = source.path.to_string_lossy();
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1)",
                [path.as_ref()],
                |row| row.get(0),
            )
            .map_err(io::Error::other)?;
        if !pending {
            invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
        }
    }
    tx.commit().map_err(io::Error::other)
}

/// Discover prior current paths in bounded durable keyset pages during repair.
/// Filesystem discovery cannot enumerate disappeared or deselected sources.
/// The admitted SourceJournal qualifies `tx` through its current physical
/// contract and owner/namespace/epoch validation before this page.
pub(crate) fn recovery_page(tx: &Transaction<'_>) -> io::Result<bool> {
    let after: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_recovery_after') AND key='catalog_recovery_after'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    crate::native_timing::discovery_count(|counts| counts.statements += 1);
    let Some(after) = after else {
        return Ok(true);
    };
    // Probe the bounded keyset before the driver allocates path or custody
    // payloads. This page owns one aggregate recovery budget on either engine.
    let page_bytes: i64 = tx.query_row(
        "SELECT coalesce(sum(octet_length(source_path)),0) FROM (SELECT DISTINCT source_path FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND lifecycle='current' AND source_path>?1 ORDER BY source_path LIMIT 64)",
        [&after], |row| row.get(0),
    ).map_err(io::Error::other)?;
    let mut page_bytes = usize::try_from(page_bytes).map_err(io::Error::other)?;
    if page_bytes > crate::storage::source_observations::recovery::BYTES {
        return Err(io::Error::other(StorageError::Busy));
    }
    let mut selected=tx.prepare("SELECT DISTINCT source_path FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND lifecycle='current' AND source_path>?1 ORDER BY source_path LIMIT 64").map_err(io::Error::other)?;
    let paths = selected
        .query_map([&after], |row| row.get::<_, String>(0))
        .map_err(io::Error::other)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?;
    if crate::native_timing::enabled() {
        let scans = selected.get_status(rusqlite::StatementStatus::FullscanStep);
        crate::native_timing::discovery_count(|counts| {
            counts.source_fullscan_steps += scans.max(0) as usize
        });
    }
    drop(selected);
    let page = crate::storage::source_observations::recovery::Page::plan(&after, paths)
        .map_err(io::Error::other)?;
    let paths = &page.paths;
    crate::native_timing::discovery_count(|counts| {
        counts.statements += 1;
        counts.rows += paths.len();
    });
    for path in paths {
        let bytes: Option<i64> = tx
            .query_row(
                "SELECT octet_length(value) FROM meta WHERE namespace=semon_namespace() AND key=?1",
                [archive_index_key(path)],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        page_bytes = page_bytes
            .checked_add(usize::try_from(bytes.unwrap_or(0)).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other(StorageError::Busy))?;
        if page_bytes > crate::storage::source_observations::recovery::BYTES {
            return Err(io::Error::other(StorageError::Busy));
        }
        let _ = archived_index(tx, path).map_err(io::Error::other)?;
    }
    for path in paths {
        if tx
            .execute(
                "INSERT OR IGNORE INTO main.catalog_backfill_sources(namespace,source_path) VALUES(semon_namespace(),?1)",
                [path],
            )
            .map_err(io::Error::other)?
            > 0
        {
            invalidate_bound_catalog_source(tx, path, "changed").map_err(io::Error::other)?;
        }
    }
    if let Some(last) = &page.next {
        tx.execute(
            "UPDATE main.meta SET value=?1 WHERE namespace=semon_meta_scope('catalog_recovery_after') AND key='catalog_recovery_after'",
            [last],
        )
        .map_err(io::Error::other)?;
    } else {
        tx.execute("DELETE FROM main.meta WHERE namespace=semon_meta_scope('catalog_recovery_after') AND key='catalog_recovery_after'", [])
            .map_err(io::Error::other)?;
    }
    Ok(page.done)
}

pub(crate) fn begin_catalog_recovery(options: &crate::Options, reason: &str) -> io::Result<()> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let mut db = journal.connection().map_err(io::Error::other)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    tx.execute_batch("DELETE FROM main.catalog_backfill_sources WHERE namespace=semon_namespace(); UPDATE main.storage_source_journal SET backfill_state='discovering' WHERE namespace=semon_namespace(); INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope('catalog_recovery_after'),'catalog_recovery_after','');").map_err(io::Error::other)?;
    tx.execute(
        "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope('catalog_recovery_reason'),'catalog_recovery_reason',?1)",
        [reason],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

pub(crate) fn push_driven(options: &crate::Options) -> io::Result<bool> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = db.unchecked_transaction().map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    tx.query_row("SELECT COALESCE((SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_push_driven'),'0')='1'", [], |row|row.get(0)).map_err(io::Error::other)
}

/// Requeue only sources that changed during an untrusted local observation.
/// This grants no receiver append evidence or Push-driven inventory authority.
pub(crate) fn requeue_concurrent_sources(
    options: &crate::Options,
    prepared: &crate::model::PreparedSources,
) -> io::Result<()> {
    let paths = prepared
        .rows
        .iter()
        .flat_map(|row| &row.sources)
        .filter(|source| {
            super::Stat::of(&source.path).ok().is_none_or(|stat| {
                (
                    stat.dev,
                    stat.ino,
                    stat.size,
                    stat.modified_ns,
                    stat.changed_ns,
                ) != (
                    source.dev,
                    source.ino,
                    source.size,
                    source.modified_ns,
                    source.changed_ns,
                )
            })
        })
        .map(|source| source.path.to_string_lossy().into_owned())
        .collect::<std::collections::BTreeSet<_>>();
    if paths.is_empty() {
        return Ok(());
    }
    let journal = host_journal(options).map_err(io::Error::other)?;
    let mut db = journal.connection().map_err(io::Error::other)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    for path in &paths {
        let _ = archived_index(&tx, path).map_err(io::Error::other)?;
    }
    for path in paths {
        invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)
}

pub(crate) fn enqueue_sources(
    options: &crate::Options,
    inputs: &[crate::inputs::Input],
) -> io::Result<()> {
    #[cfg(feature = "postgres")]
    if let Some(config) = options.source.selected_store()
        && matches!(
            config.backend,
            crate::storage::StoreBackend::Postgres { .. }
        )
    {
        return crate::storage::postgres::host::enqueue_sources(config, options, inputs)
            .map_err(io::Error::other);
    }
    let (_, _, journal) = open_admitted(&super::EventCache::path(&options.cache), &options.source)
        .map_err(io::Error::other)?;
    let mut db = journal.connection().map_err(io::Error::other)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    for input in inputs {
        if !crate::inputs::is_input_path(input.root.as_str(), &input.path)
            || !input.path.ends_with(".jsonl")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source intent outside native log allowlist",
            ));
        }
        let path = input.full_path(options);
        invalidate_catalog_source(
            &tx,
            path.to_str()
                .ok_or_else(|| io::Error::other("nonportable source path"))?,
            "changed",
        )
        .map_err(io::Error::other)?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope('catalog_push_driven'),'catalog_push_driven','1')",
        [],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

fn enqueue_native_tx(tx: &Transaction<'_>, harness: &str, id: &str) -> rusqlite::Result<()> {
    // Facts can change before a new owner has its first published Catalog row.
    // Keep those staged observations under the same exact-source revision fence.
    let mut statement=tx.prepare("SELECT source_path FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND lifecycle='current' AND harness=?1 AND native_id=?2 UNION SELECT i.source_path FROM main.catalog_semantic_inputs i JOIN main.catalog_semantic_sources s USING(namespace,source_path) WHERE i.namespace=semon_namespace() AND i.harness=?1 AND i.native_id=?2 AND s.index_done=1 AND EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=semon_namespace()) UNION SELECT own.source_path FROM main.catalog_semantic_native n JOIN main.catalog_semantic_owner_sources own USING(namespace,session_key) JOIN main.catalog_semantic_sources s USING(namespace,source_path) WHERE n.namespace=semon_namespace() AND n.harness=?1 AND n.native_id=?2 AND s.index_done=1 AND EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=semon_namespace()) UNION SELECT d.source_path FROM main.catalog_native_dependencies d JOIN main.catalog_semantic_sources s USING(namespace,source_path) WHERE d.namespace=semon_namespace() AND (d.harness=?1 OR d.harness='runtime:native:'||?1) AND d.native_id=?2 AND s.index_done=1 AND EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=semon_namespace())")?;
    let paths = statement
        .query_map(params![harness, id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for path in paths {
        invalidate_bound_catalog_source(tx, &path, "changed")?;
    }
    Ok(())
}
fn host_journal(
    options: &crate::Options,
) -> Result<crate::storage::sqlite::source_journal::SourceJournal, crate::storage::StorageError> {
    let config = options.source.config(&options.cache)?;
    Ok(
        crate::storage::sqlite::source_journal::SourceJournal::owned(
            configured::NativeOwnership::restore(&config)?,
        ),
    )
}

fn invalidate_bound_catalog_source(
    tx: &Transaction<'_>,
    path: &str,
    reason: &str,
) -> rusqlite::Result<()> {
    let namespace: String = tx.query_row("SELECT semon_namespace()", [], |r| r.get(0))?;
    crate::storage::sqlite::source_journal::invalidate(tx, &namespace, path, reason)
}

pub(crate) fn enqueue_native(options: &crate::Options, harness: &str, id: &str) -> io::Result<()> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    enqueue_native_tx(&tx, harness, id).map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}
pub(crate) fn enqueue_pid(options: &crate::Options, pid: u32) -> io::Result<()> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    enqueue_dependency_tx(&tx, "runtime:pid", &pid.to_string()).map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}
fn enqueue_dependency_tx(tx: &Transaction<'_>, harness: &str, id: &str) -> rusqlite::Result<()> {
    let paths = {
        let mut statement=tx.prepare("SELECT d.source_path FROM main.catalog_native_dependencies d WHERE d.namespace=semon_namespace() AND harness=?1 AND native_id=?2 AND (EXISTS(SELECT 1 FROM main.session_catalog_sources s WHERE s.namespace=d.namespace AND s.lifecycle='current' AND s.source_path=d.source_path) OR EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=d.namespace AND s.source_path=d.source_path AND s.index_done=1 AND EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=semon_namespace())))")?;
        statement
            .query_map(params![harness, id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for path in paths {
        invalidate_bound_catalog_source(tx, &path, "changed")?;
    }
    Ok(())
}

pub(crate) fn enqueue_facts(
    options: &crate::Options,
    before: &crate::Facts,
    after: &crate::Facts,
    push_driven: bool,
) -> io::Result<()> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    let changes = crate::facts::changes(before, after);
    if changes.global {
        // These fields are actual dependencies of every recorded observation.
        // Queue their source paths; bounded workers still never build a cohort.
        let paths = {
            let mut statement=tx.prepare("SELECT source_path FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND lifecycle='current' UNION SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=semon_namespace() AND index_done=1 AND EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=semon_namespace())").map_err(io::Error::other)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(io::Error::other)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(io::Error::other)?
        };
        for path in paths {
            invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
        }
    }
    for (harness, id) in &changes.native {
        enqueue_native_tx(&tx, harness, id).map_err(io::Error::other)?;
    }
    for cwd in &changes.repos {
        if before.repos.get(cwd) != after.repos.get(cwd) {
            let paths = {
                let mut statement = tx
                    .prepare(
                        "SELECT path FROM main.files WHERE namespace=semon_namespace() AND cwd=?1",
                    )
                    .map_err(io::Error::other)?;
                statement
                    .query_map([cwd], |row| row.get::<_, String>(0))
                    .map_err(io::Error::other)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(io::Error::other)?
            };
            for path in paths {
                invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
            }
        }
    }
    for pid in &changes.pids {
        enqueue_dependency_tx(&tx, "runtime:pid", &pid.to_string()).map_err(io::Error::other)?;
    }
    for pid in crate::model::pid_files(
        options,
        &crate::facts::MachineFacts::Recorded(Box::new(after.clone())),
    ) {
        if before.proc_starts.get(&pid.pid) != after.proc_starts.get(&pid.pid)
            || before.runs.get(&pid.pid) != after.runs.get(&pid.pid)
            || before.process_ancestors.get(&pid.pid) != after.process_ancestors.get(&pid.pid)
        {
            enqueue_native_tx(&tx, "claude", &pid.session).map_err(io::Error::other)?;
        }
    }
    let selected = |facts: &crate::Facts| {
        facts
            .codex_rollouts
            .iter()
            .flatten()
            .chain(facts.codex_provisional_rollouts.iter().flatten())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    };
    let mut previous = selected(before);
    let current = selected(after);
    if before.codex_rollouts.is_none() && after.codex_rollouts.is_some() {
        let mut statement=tx.prepare("SELECT DISTINCT source_path FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND harness='codex' AND lifecycle='current'").map_err(io::Error::other)?;
        for path in statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(io::Error::other)?
        {
            if let Some(relative) = path
                .map_err(io::Error::other)?
                .strip_prefix(options.codex_home.to_string_lossy().as_ref())
                .map(|path| path.trim_start_matches('/').to_owned())
            {
                previous.insert(relative);
            }
        }
    }
    for relative in current.difference(&previous) {
        if !crate::is_input_path("codex", relative) {
            continue;
        }
        let new_path = options.codex_home.join(relative);
        let Some(id) = crate::events::crate_id_for_relocation(&new_path) else {
            continue;
        };
        let mut statement=tx.prepare("SELECT DISTINCT source_path FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND lifecycle='current' AND harness='codex' AND native_id=?1 LIMIT 2").map_err(io::Error::other)?;
        let old_paths = statement
            .query_map([&id], |row| row.get::<_, String>(0))
            .map_err(io::Error::other)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::Error::other)?;
        for old in old_paths {
            let Some(old_relative) = Path::new(&old)
                .strip_prefix(&options.codex_home)
                .ok()
                .and_then(|path| path.to_str())
            else {
                continue;
            };
            if previous.contains(old_relative)
                && !current.contains(old_relative)
                && relative.starts_with("archived_sessions/")
                    != old_relative.starts_with("archived_sessions/")
            {
                tx.execute(
                    "INSERT OR IGNORE INTO main.catalog_relocation_candidates VALUES(semon_namespace(),?1,?2,?3)",
                    params![old, new_path.to_string_lossy(), id],
                )
                .map_err(io::Error::other)?;
            }
        }
    }
    for path in previous.symmetric_difference(&current) {
        if crate::is_input_path("codex", path) {
            invalidate_bound_catalog_source(
                &tx,
                options.codex_home.join(path).to_string_lossy().as_ref(),
                "changed",
            )
            .map_err(io::Error::other)?;
        }
    }
    if push_driven {
        tx.execute(
            "INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope('catalog_push_driven'),'catalog_push_driven','1')",
            [],
        )
        .map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)
}

/// Persist a cold parser continuation only after its ledger commit. A crash in
/// the gap leaves the source revision pending and forces prefix verification;
/// the committed record offset and Event rows remain atomic and replayable.
pub(crate) fn checkpoint_source_scan(
    stem: &Path,
    source: &crate::storage::HostSource,
    path: &str,
    ledger: &Ledger,
    physical: &Stat,
) -> io::Result<()> {
    let config = source.config(stem).map_err(io::Error::other)?;
    let owner = configured::NativeOwnership::restore(&config).map_err(io::Error::other)?;
    let journal = crate::storage::sqlite::source_journal::SourceJournal::owned(owner);
    let mut db = journal.connection().map_err(io::Error::other)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    if ledger_row(&tx, path)
        .map_err(io::Error::other)?
        .map(|(_, ledger)| ledger)
        .as_ref()
        != Some(ledger)
        || Stat::of(Path::new(path)).ok().as_ref() != Some(physical)
    {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "cold checkpoint generation changed",
        ));
    }
    let generation = format!(
        "{:x}",
        Sha256::digest(json(physical).map_err(io::Error::other)?.as_bytes())
    )[..32]
        .to_owned();
    let evidence = crate::source_change::AppendEvidence {
        generation,
        before: ledger.stat,
        after: *physical,
    };
    tx.execute(
        "INSERT OR REPLACE INTO main.session_source_mutations(namespace,source_path,evidence) VALUES(semon_namespace(),?1,?2)",
        params![path, json(&evidence).map_err(io::Error::other)?],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

fn runtime_input_evidence(
    connection: &Connection,
    path: &str,
) -> rusqlite::Result<Option<crate::source_change::RuntimeInputEvidence>> {
    let text: Option<Option<String>> = connection.query_row("SELECT CASE WHEN octet_length(evidence)<=2048 THEN evidence ELSE NULL END FROM session_source_mutations WHERE namespace=semon_namespace() AND source_path=?1", [path], |row| row.get(0)).optional()?;
    Ok(text
        .flatten()
        .and_then(|text| {
            serde_json::from_str::<crate::source_change::RuntimeInputEvidence>(&text).ok()
        })
        .filter(|evidence| {
            let hash = &evidence.unaffected;
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        }))
}

/// Replay a PID input's dependency invalidation under its facts fence.
/// Unrelated process inputs and semantic facts must still match the witness.
pub(crate) fn resume_runtime_input(options: &crate::Options, path: &str) -> io::Result<()> {
    if options.facts.is_none() {
        return Ok(());
    }
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    resume_runtime_input_tx(&tx, options, path).map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

fn resume_runtime_input_tx(
    tx: &Transaction<'_>,
    options: &crate::Options,
    path: &str,
) -> rusqlite::Result<()> {
    let Some(evidence) = runtime_input_evidence(tx, path)? else {
        return Ok(());
    };
    if let Some(id) = resumed_runtime_native(options, path, &evidence)
        .map_err(|_| rusqlite::Error::InvalidQuery)?
    {
        enqueue_native_tx(tx, "claude", &id)?;
    }
    Ok(())
}

/// The source authority/fingerprint recipe performs native observation only.
/// Physical adapters own mutation evidence, dependency writes and transaction.
pub(crate) fn resumed_runtime_native(
    options: &crate::Options,
    path: &str,
    evidence: &crate::source_change::RuntimeInputEvidence,
) -> io::Result<Option<String>> {
    let Some(facts) = options
        .facts
        .as_ref()
        .and_then(|file| crate::read_facts(file).ok())
    else {
        return Ok(None);
    };
    let unaffected = crate::native_evidence::recorded_runtime_fingerprint_excluding(
        options,
        &facts,
        Some(Path::new(path)),
    )?;
    if unaffected != evidence.unaffected {
        return Ok(None);
    }
    Ok(crate::read_regular_at_most(Path::new(path), 65536)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["sessionId"].as_str().map(str::to_owned)))
}

pub(crate) fn source_write_store(options: &crate::Options) -> io::Result<Box<dyn IndexStore>> {
    #[cfg(feature = "postgres")]
    if let Some(config) = options.source.selected_store()
        && let crate::storage::StoreBackend::Postgres { pool, control } = &config.backend
    {
        crate::storage::postgres::require_configured(config, pool, control)
            .map_err(io::Error::other)?;
        return crate::storage::postgres::index_store::open(
            pool.clone(),
            config.namespace.clone(),
            control.clone(),
        )
        .map(|(store, _)| store)
        .map_err(io::Error::other);
    }
    let (store, _, _) = open_admitted(&super::EventCache::path(&options.cache), &options.source)
        .map_err(io::Error::other)?;
    Ok(store)
}

pub(crate) fn begin_source_write(
    options: &crate::Options,
    input: &crate::Input,
) -> io::Result<(i64, Option<crate::source_change::AppendEvidence>)> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let path = input.full_path(options).to_string_lossy().into_owned();
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    let previous = read_append_evidence(&tx, &path).map_err(io::Error::other)?;
    #[cfg(test)]
    if let Some(hook) = BEFORE_SOURCE_INVALIDATION.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    if input.path.ends_with(".meta.json") {
        let log = input
            .full_path(options)
            .with_extension("")
            .with_extension("jsonl");
        invalidate_bound_catalog_source(&tx, log.to_string_lossy().as_ref(), "changed")
            .map_err(io::Error::other)?;
    }
    let revision = tx
        .query_row(
            "SELECT revision FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1",
            [&path],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    tx.execute(
        "DELETE FROM main.session_source_mutations WHERE namespace=semon_namespace() AND source_path=?1",
        [&path],
    )
    .map_err(io::Error::other)?;
    if input.root == crate::InputRoot::Claude
        && input.path.starts_with("sessions/")
        && let Some(facts) = options
            .facts
            .as_ref()
            .and_then(|file| crate::read_facts(file).ok())
    {
        let evidence = crate::source_change::RuntimeInputEvidence {
            unaffected: crate::native_evidence::recorded_runtime_fingerprint_excluding(
                options,
                &facts,
                Some(Path::new(&path)),
            )?,
        };
        tx.execute(
            "INSERT OR REPLACE INTO main.session_source_mutations(namespace,source_path,evidence) VALUES(semon_namespace(),?1,?2)",
            params![path, json(&evidence).map_err(io::Error::other)?],
        )
        .map_err(io::Error::other)?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(semon_meta_scope('catalog_push_driven'),'catalog_push_driven','1')",
        [],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)?;
    Ok((revision, previous))
}

pub(crate) fn complete_source_write(
    options: &crate::Options,
    input: &crate::Input,
    revision: i64,
    evidence: Option<&crate::source_change::AppendEvidence>,
) -> io::Result<()> {
    let journal = host_journal(options).map_err(io::Error::other)?;
    let db = journal.connection().map_err(io::Error::other)?;
    let path = input.full_path(options).to_string_lossy().into_owned();
    let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    journal.validate(&tx).map_err(io::Error::other)?;
    let actual: Option<i64> = tx
        .query_row(
            "SELECT revision FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1",
            [&path],
            |row| row.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    resume_runtime_input_tx(&tx, options, &path).map_err(io::Error::other)?;
    // Publication or another writer may have advanced the intent. Enqueue a
    // fresh conservative revision, never promote that race into trusted evidence.
    if actual == Some(revision)
        && let Some(evidence) = evidence
    {
        tx.execute(
            "INSERT OR REPLACE INTO main.session_source_mutations(namespace,source_path,evidence) VALUES(semon_namespace(),?1,?2)",
            params![path, json(evidence).map_err(io::Error::other)?],
        )
        .map_err(io::Error::other)?;
    } else {
        tx.execute(
            "DELETE FROM main.session_source_mutations WHERE namespace=semon_namespace() AND source_path=?1",
            [&path],
        )
        .map_err(io::Error::other)?;
    }
    // Whole JSON inputs keep their pre-Raw revision until dependency replay.
    // Their shared source/facts fence prevents resolution during the write.
    // Keeping the revision also processes the input before its newly dirtied
    // native logs, avoiding an unnecessary second publication of those logs.
    if !input.path.ends_with(".json") || actual != Some(revision) {
        invalidate_bound_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)
}

fn read_append_evidence(
    connection: &Connection,
    path: &str,
) -> rusqlite::Result<Option<crate::source_change::AppendEvidence>> {
    let text: Option<Option<String>> = connection.query_row("SELECT CASE WHEN octet_length(evidence)<=2048 THEN evidence ELSE NULL END FROM session_source_mutations WHERE namespace=semon_namespace() AND source_path=?1",[path],|row|row.get(0)).optional()?;
    Ok(text
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok()))
}

/// Load only the selected native sources in the caller's existing read snapshot.
/// This never opens a writable store, walks roots or prunes
/// source membership. Catalog identities and event ledgers must agree exactly.
pub(super) fn read_selected(
    connection: &Connection,
    sources: &[crate::model::summary::CatalogSource],
    native_ids: &std::collections::BTreeSet<String>,
) -> rusqlite::Result<Loaded> {
    if !current(connection)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut readers = Readers::new(connection)?;
    let mut statement =
        connection.prepare(&format!("SELECT {FILE_COLUMNS} FROM files WHERE path=?1"))?;
    let mut loaded = Loaded::default();
    let mut seen = std::collections::BTreeMap::new();
    for source in sources {
        if let Some(previous) = seen.insert(&source.path, source) {
            if previous != source {
                return Err(rusqlite::Error::InvalidQuery);
            }
            continue;
        }
        let path = source.path.to_str().ok_or(rusqlite::Error::InvalidQuery)?;
        crate::selected_read::check().map_err(crate::selected_read::sql_error)?;
        let (_, ledger) = ledger_row(connection, path)?.ok_or(rusqlite::Error::InvalidQuery)?;
        if (
            ledger.stat.dev,
            ledger.stat.ino,
            ledger.offset,
            ledger.prefix,
            ledger.tail,
        ) != (
            source.dev,
            source.ino,
            source.offset,
            source.prefix_sha256,
            source.tail_sha256,
        ) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let mut rows = statement.query([path])?;
        let row = rows.next()?.ok_or(rusqlite::Error::InvalidQuery)?;
        let (path, ledger, index) = read_file(&mut readers, row)?;
        if (
            ledger.stat.dev,
            ledger.stat.ino,
            ledger.offset,
            ledger.prefix,
            ledger.tail,
        ) != (
            source.dev,
            source.ino,
            source.offset,
            source.prefix_sha256,
            source.tail_sha256,
        ) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        loaded.files.push((path, ledger, index));
    }
    let mut runs = connection.prepare(&reported_select_sql(true))?;
    for id in native_ids {
        let mut rows = runs.query([id])?;
        while let Some(row) = rows.next()? {
            crate::selected_read::row(row, 512)?;
            loaded.reported_runs.push(read_run(row)?);
        }
    }
    Ok(loaded)
}

/// Read only the generation-bound role proof needed for current ownership.
/// The caller holds the same read transaction as the selected Catalog rows.
pub(super) fn selected_roles_current(
    connection: &Connection,
    sources: &[crate::model::summary::CatalogSource],
) -> rusqlite::Result<bool> {
    if !current(connection)? {
        return Ok(false);
    }
    selected_role_values(connection, sources)
}

pub(crate) fn admitted_roles_current(
    connection: &Connection,
    sources: &[crate::model::summary::CatalogSource],
    owner: &crate::storage::sqlite::semantic::AdmittedSource,
) -> rusqlite::Result<bool> {
    owner
        .validate(connection)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    selected_role_values(connection, sources)
}

fn selected_role_values(
    connection: &Connection,
    sources: &[crate::model::summary::CatalogSource],
) -> rusqlite::Result<bool> {
    let mut metadata = connection.prepare(native_metadata_value(connection)?)?;
    for source in sources {
        let path = source.path.to_str().ok_or(rusqlite::Error::InvalidQuery)?;
        let Some((_, ledger)) = ledger_row(connection, path)? else {
            return Ok(false);
        };
        let mut expected = ledger.source(&source.path, &source.native_id);
        expected.native_role = source.native_role.clone();
        if expected != *source {
            return Ok(false);
        }
        let sidecar = if crate::native_sidecar::agent_path(&source.path) {
            let Some(sidecar) = read_claude_sidecar(&mut metadata, path, &ledger)? else {
                return Ok(false);
            };
            if !sidecar.current(&source.path) {
                return Ok(false);
            }
            Some(sidecar)
        } else {
            None
        };
        if !crate::model::current_query_role(source, sidecar.as_ref()) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Focused reads never prepare or convert another Semon storage contract.
/// The binding registers its namespace and validates the stream epoch and
/// sole current admission inside the opening read snapshot.
pub(crate) fn focused_connection(
    binding: &crate::storage::NativeReaderBinding,
) -> io::Result<Connection> {
    let db = binding.connection().map_err(io::Error::other)?;
    let read = db.unchecked_transaction().map_err(io::Error::other)?;
    if !binding
        .validate(&read, &binding.config().namespace)
        .map_err(io::Error::other)?
    {
        return Err(io::Error::other(
            crate::storage::StorageError::NamespaceUpgradePending,
        ));
    }
    require_current(&read)?;
    read.commit().map_err(io::Error::other)?;
    Ok(db)
}

/// Home/Query readers open the local store only through its restored reader
/// binding, which registers `semon_namespace()`; each reader then qualifies
/// the sole current admission with `require_current` inside its own snapshot.
pub(crate) fn local_reader(options: &crate::Options) -> io::Result<Connection> {
    crate::storage::local_navigation_binding(options, "")
        .and_then(|binding| binding.connection())
        .map_err(io::Error::other)
}

/// Admitted reader binding for the fixture namespace of an existing store.
#[cfg(test)]
pub(crate) fn fixture_reader_binding(
    path: &Path,
) -> io::Result<crate::storage::NativeReaderBinding> {
    let epoch: String = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|db| {
            db.query_row(
                "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
                [],
                |row| row.get(0),
            )
        })
        .map_err(io::Error::other)?;
    Ok(crate::storage::NativeReaderBinding::Admitted(
        crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Sqlite {
                database: path.to_owned(),
            },
            namespace: fixture_namespace(),
            expected_stream_epoch: Some(epoch),
        },
    ))
}

/// The single logical source bound by SqliteStore fixtures.
#[cfg(test)]
pub(crate) fn fixture_namespace() -> crate::storage::LogicalSourceNamespace {
    crate::storage::LogicalSourceNamespace::new("fixture-source").unwrap()
}

#[cfg(test)]
type Hook = std::cell::RefCell<Option<Box<dyn FnOnce()>>>;

#[cfg(test)]
thread_local! {
    static AFTER_TREE_LEDGER: Hook = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn hook(slot: &'static std::thread::LocalKey<Hook>) {
    if let Some(hook) = slot.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

/// `O_NONBLOCK`, which `std` doesn't name: a FIFO at the path opens at
/// once instead of waiting for a writer, and is then skipped.
#[cfg(unix)]
const O_NONBLOCK: i32 = if cfg!(any(target_os = "macos", target_os = "ios")) {
    0x0004
} else if cfg!(target_os = "linux") {
    0o4000
} else {
    0
};

/// Opens a source for reading only if it is a regular file:
/// never through a symlink, and never waiting on a FIFO. Anything else is
/// left where it is and reported once; `None` too when it is gone or can't
/// be opened.
pub(crate) fn open_regular(path: &Path) -> Option<fs::File> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        warn_source_read(path, &"it isn't a regular file");
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(O_NOFOLLOW | O_NONBLOCK);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            warn_source_read(path, &error);
            return None;
        }
    };
    match file.metadata() {
        Ok(metadata) if metadata.is_file() => Some(file),
        Ok(_) => {
            warn_source_read(path, &"it isn't a regular file");
            None
        }
        Err(error) => {
            warn_source_read(path, &error);
            None
        }
    }
}

/// Reports a source read failure once per process.
fn warn_source_read(path: &Path, error: &dyn fmt::Display) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("semon: {} cannot be read ({error})", path.display());
    }
}

fn publication_storage_failure(error: crate::storage::StorageError) -> StoreError {
    match error {
        crate::storage::StorageError::Busy => StoreError::Busy(error.to_string()),
        _ => StoreError::Failed(error.to_string()),
    }
}

/// A rusqlite error, as the cache handles it.
pub(crate) fn failure(error: rusqlite::Error) -> StoreError {
    if is_busy(&error) {
        StoreError::Busy(error.to_string())
    } else if is_data_error(&error) {
        StoreError::Data(error.to_string())
    } else {
        StoreError::Failed(error.to_string())
    }
}

pub(crate) fn read_session_descriptions(
    db: &Connection,
    requests: &[SessionDescriptionRequest],
) -> rusqlite::Result<Vec<Option<String>>> {
    let mut statement = db.prepare(
        "SELECT description FROM main.session_descriptions WHERE namespace=semon_namespace() AND session_key=?1 AND version=?2 AND inputs=?3",
    )?;
    requests
        .iter()
        .map(|request| {
            statement
                .query_row(
                    params![
                        request.key.as_str(),
                        request.version,
                        request.inputs.as_str()
                    ],
                    |row| row.get(0),
                )
                .optional()
        })
        .collect()
}

pub(crate) fn write_session_descriptions(
    tx: &Transaction<'_>,
    descriptions: &[(String, u32, String, String)],
) -> rusqlite::Result<()> {
    let mut put = tx.prepare("INSERT OR REPLACE INTO main.session_descriptions(namespace,session_key,version,inputs,description) VALUES(semon_namespace(),?1,?2,?3,?4)")?;
    for (key, version, inputs, description) in descriptions {
        put.execute(params![key, version, inputs, description])?;
    }
    Ok(())
}

impl IndexStore for SqliteStore {
    fn facts_authority(&self, path: &Path) -> io::Result<crate::facts::source_authority::Sources> {
        crate::facts::source_authority::Sources::sqlite(path)
    }
    fn session_descriptions(
        &self,
        requests: &[SessionDescriptionRequest],
    ) -> Result<Vec<Option<String>>, StoreError> {
        if requests.len() > SESSION_DESCRIPTION_BATCH_LIMIT {
            return Err(StoreError::Configured(StorageError::InvalidArguments));
        }
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let transaction = self.connection.unchecked_transaction().map_err(failure)?;
        if !current(&transaction).map_err(failure)? {
            return Ok(vec![None; requests.len()]);
        }
        read_session_descriptions(&transaction, requests).map_err(failure)
    }

    fn save_session_descriptions(
        &mut self,
        descriptions: &[(String, u32, String, String)],
    ) -> Result<Outcome, StoreError> {
        // Optional reuse must not add another writer-lock wait to startup or
        // refresh. A contended publication is safely retried by a later build.
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(failure)?;
        let result = (|| {
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failure)?;
            if !current(&transaction).map_err(failure)? {
                return Ok(Outcome::Stale);
            }
            {
                write_session_descriptions(&transaction, descriptions).map_err(failure)?;
            }
            transaction.commit().map_err(failure)?;
            Ok(Outcome::Written)
        })();
        self.connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(failure)?;
        result
    }

    fn session_catalog_generation(&self) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_generation') AND key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(failure)
    }

    fn note_catalog_audit(&mut self, expected: Option<&str>, now: i64) -> Result<bool, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        require_current_sql(&tx).map_err(failure)?;
        let ready = note_catalog_audit_tx(&tx, expected, now).map_err(failure)?;
        tx.commit().map_err(failure)?;
        Ok(ready)
    }

    fn publish_session_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.publish_catalog_with_generation(publication, expected_generation, None, false, None)
    }

    fn session_slots(
        &self,
        row: &crate::model::summary::CatalogRow,
        expected_generation: Option<&str>,
        max: usize,
    ) -> Result<Option<Vec<crate::slot_projection::SavedSlot>>, StoreError> {
        let Some(expected) = expected_generation else {
            return Ok(None);
        };
        // Optional metadata reuse must not add another writer-lock wait.
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(failure)?;
        let result = (|| {
            let tx = self.connection.unchecked_transaction().map_err(failure)?;
            if !current(&tx).map_err(failure)? {
                return Ok(None);
            }
            read_session_slots(&tx, row, expected, max)
        })();
        self.connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(failure)?;
        result
    }

    fn begin_semantic_stage(
        &mut self,
        paths: &[String],
        runtime: &str,
        journaled_runtime: bool,
    ) -> Result<Outcome, StoreError> {
        self.semantic_begin_with_journal(paths, runtime, journaled_runtime)
    }
    fn semantic_batch(
        &mut self,
        limit: usize,
    ) -> Result<Option<crate::model::relation_stage::Batch>, StoreError> {
        self.semantic_next(limit)
    }
    fn record_semantic_batch(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<Outcome, StoreError> {
        self.semantic_record(batch, prepared)
    }
    fn semantic_rejection(&self) -> Option<&'static str> {
        self.semantic_rejection
    }
    fn finish_semantic_stage(&mut self, epoch: i64) -> Result<Outcome, StoreError> {
        self.semantic_finish(epoch)
    }
    fn semantic_select(
        &mut self,
        epoch: i64,
        selector: &crate::model::relation_stage::Selector,
    ) -> Result<crate::model::relation_stage::Selection, StoreError> {
        self.semantic_selection(epoch, selector)
    }
    fn semantic_replay_page(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> Result<crate::model::relation_stage::Context, StoreError> {
        self.semantic_replay(epoch, owners, after)
    }
    fn publish_semantic_catalog(
        &mut self,
        batch: &crate::model::relation_stage::Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.semantic_rejection = None;
        if batch.phase != crate::model::relation_stage::Phase::Publication {
            self.semantic_rejection = Some("publication_phase");
            return Ok((Outcome::Conflict, None));
        }
        if batch.base_generation != prepared.base_generation {
            self.semantic_rejection = Some("prepared_generation");
            return Ok((Outcome::Conflict, None));
        }
        self.publish_catalog_with_generation(
            &crate::slot_projection::Publication {
                rows: &prepared.rows,
                transcripts: Some(&prepared.transcripts),
                observations: prepared.presentation_observations(),
            },
            batch.base_generation.as_deref(),
            Some(&prepared.claims),
            false,
            Some(batch),
        )
    }

    fn publish_partial_catalog(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.publish_catalog_with_generation(
            publication,
            expected_generation,
            Some(claims),
            false,
            None,
        )
    }

    fn publish_closed_catalog_fenced(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: &[(String, Option<i64>)],
    ) -> Result<Option<(Outcome, Option<String>)>, StoreError> {
        if !closed_catalog(&self.connection, publication, claims).map_err(failure)? {
            return Ok(None);
        }
        self.publish_catalog_with_generation(
            publication,
            expected_generation,
            Some(claims),
            true,
            None,
        )
        .map(Some)
    }

    fn catalog_component_paths(
        &self,
        paths: &[String],
        keys: &[String],
        native_parents: &[(String, String)],
    ) -> Result<Option<Vec<String>>, StoreError> {
        let tx = self.connection.unchecked_transaction().map_err(failure)?;
        read_catalog_component_paths(&tx, paths, keys, native_parents)
    }

    fn resume_runtime_input(
        &mut self,
        options: &crate::Options,
        path: &str,
    ) -> Result<(), StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        resume_runtime_input_tx(&tx, options, path).map_err(failure)?;
        tx.commit().map_err(failure)
    }
    fn resolve_catalog_input(
        &mut self,
        path: &str,
        revision: i64,
        native: Option<(&str, &str)>,
        log: Option<&str>,
    ) -> Result<Outcome, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        let outcome = resolve_catalog_input_tx(&tx, path, revision, native, log)?;
        if outcome != Outcome::Written {
            return Ok(outcome);
        }
        tx.commit().map_err(failure)?;
        Ok(Outcome::Written)
    }

    fn relocate_codex(
        &mut self,
        path: &str,
        stat: &Stat,
        id: &str,
        file: &mut crate::sealed::LogFile,
        measurements: &mut crate::native_timing::Measurements,
    ) -> Result<Option<String>, StoreError> {
        let writer =
            configured::Writer::owned(self.owner.clone()).map_err(StoreError::Configured)?;
        relocation::reuse(self, path, stat, id, file, measurements, &writer)
    }

    fn append_evidence(
        &self,
        path: &str,
    ) -> Result<Option<crate::source_change::AppendEvidence>, StoreError> {
        read_append_evidence(&self.connection, path).map_err(failure)
    }

    fn qualified_catalog(&self) -> Result<bool, StoreError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(failure)?;
        let ready = qualify_catalog_tx(&tx, self.owner.config().namespace.as_str())?;
        tx.commit().map_err(failure)?;
        Ok(ready)
    }

    fn ledger(&self, path: &str) -> Result<Option<Ledger>, StoreError> {
        ledger_row(&self.connection, path)
            .map(|found| found.map(|(_, ledger)| ledger))
            .map_err(failure)
    }

    fn ledger_batch(&self, paths: &[String]) -> Result<Vec<Option<Ledger>>, StoreError> {
        if paths.len() > 64 {
            return Err(StoreError::Configured(StorageError::InvalidArguments));
        }
        let transaction = self.connection.unchecked_transaction().map_err(failure)?;
        self.owner
            .validate_ready(&transaction)
            .map_err(StoreError::Configured)?;
        let ledgers = paths
            .iter()
            .map(|path| {
                ledger_row(&transaction, path)
                    .map(|found| found.map(|(_, ledger)| ledger))
                    .map_err(failure)
            })
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().map_err(failure)?;
        Ok(ledgers)
    }

    fn load_file(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StoreError> {
        self.read_one(path).map_err(failure)
    }

    fn native_usage_seen(
        &self,
        path: &str,
        ledger: &Ledger,
        identity: &str,
    ) -> Result<native_usage::Seen, StoreError> {
        native_usage::seen(&self.connection, path, ledger, identity).map_err(failure)
    }

    fn advance_tree_repair(
        &mut self,
        options: &crate::Options,
    ) -> io::Result<(bool, Option<String>)> {
        let (pending, changed) = native_usage::advance(self, options, None)?;
        let cleanup = tree_state::cleanup_work(&mut self.connection).map_err(io::Error::other)?;
        Ok((pending || cleanup, changed))
    }

    fn tree_repair_pending(&self) -> Result<bool, StoreError> {
        Ok(
            native_usage::repair_pending(&self.connection).map_err(failure)?
                || tree_state::work_pending(&self.connection).map_err(failure)?,
        )
    }

    fn queue_tree_repair(&mut self, path: &str, ledger: &Ledger, _harness: &str) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !current(&tx).map_err(io::Error::other)? {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        queue_tree_repair_tx(&tx, path, ledger)?;
        tx.commit().map_err(io::Error::other)
    }

    fn load_tree(
        &self,
        path: &str,
        expected: &Stat,
    ) -> Result<Option<(Ledger, crate::native_tree::Metadata)>, StoreError> {
        self.read_tree(path, expected).map_err(failure)
    }

    fn commit_initial_files(
        &mut self,
        files: &[(String, Ledger, std::sync::Arc<FileIndex>)],
    ) -> Result<Outcome, StoreError> {
        let timing = crate::native_timing::enabled();
        let mut measurements = crate::native_timing::Measurements::new(timing);
        let mut row_measurements = crate::native_timing::Measurements::new(timing);
        let mut write_activity = crate::native_timing::WriteActivity::new(
            timing,
            &self.path,
            "index_initial",
            &self.connection,
        );
        let started = measurements.start();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        measurements.record("transaction", started, 1);
        let started = measurements.start();
        if !current(&transaction).map_err(failure)? {
            return Ok(Outcome::Stale);
        }
        if !super::native_commit::initial_batch(files)
            .map_err(|message| StoreError::Data(message.into()))?
        {
            return Ok(Outcome::Conflict);
        }
        for (path, _, _) in files {
            if ledger_row(&transaction, path).map_err(failure)?.is_some() {
                return Ok(Outcome::Conflict);
            }
        }
        measurements.record("fences", started, files.len());
        write_activity.stage("fences", &transaction);
        let started = measurements.start();
        let layout = WriterLayout::new(&transaction).map_err(failure)?;
        let mut usage_deletes = UsageDeletes::default();
        for (path, ledger, index) in files {
            write_index_changes_with_usage_deletes(
                &transaction,
                path,
                ledger,
                index,
                None,
                None,
                &mut row_measurements,
                self.owner.config().namespace.as_str(),
                &layout,
                &mut usage_deletes,
            )
            .map_err(failure)?;
        }
        drop(usage_deletes);
        measurements.record("write_index_changes", started, files.len());
        write_activity.stage("rows", &transaction);
        let started = measurements.start();
        transaction.commit().map_err(failure)?;
        measurements.record("commit", started, 1);
        write_activity.stage("commit", &self.connection);
        if timing {
            measurements.emit(
                &crate::native_timing::id(&self.path),
                "index_initial_publication",
            );
            row_measurements.emit(&crate::native_timing::id(&self.path), "index_rows");
        }
        Ok(Outcome::Written)
    }

    fn commit_file(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        changes: Option<&super::DirtyRows>,
        ledger: &Ledger,
        _physical_stat: &Stat,
        index: &FileIndex,
    ) -> Result<Outcome, StoreError> {
        self.write_one_dirty(path, expected, changes, ledger, index)
            .map_err(failure)
    }

    fn select_reported_runs(
        &mut self,
        ids: &std::collections::BTreeSet<String>,
    ) -> Result<Vec<ReportedRunSnapshot>, StoreError> {
        let transaction = self.connection.unchecked_transaction().map_err(failure)?;
        if !current(&transaction).map_err(failure)? {
            return Err(StoreError::Failed(
                "session index version changed".to_owned(),
            ));
        }
        let runs = read_runs_selected(&transaction, ids).map_err(failure)?;
        transaction.commit().map_err(failure)?;
        self.selected_runs = Some(ids.clone());
        Ok(runs)
    }

    fn file_ledgers(&self) -> Result<Vec<(String, Option<Ledger>)>, StoreError> {
        let transaction = self.connection.unchecked_transaction().map_err(failure)?;
        read_file_ledgers(&transaction)
    }

    fn remove_files(&mut self, files: &[(String, Option<Ledger>)]) -> Result<Outcome, StoreError> {
        self.remove(files).map_err(failure)
    }

    fn save_runs(
        &mut self,
        runs: &[ReportedRunSnapshot],
        stamp: &ReportedFileStamp,
    ) -> Result<Option<Vec<ReportedRunSnapshot>>, StoreError> {
        self.write_runs(runs, stamp).map_err(failure)
    }
}

/// Source observation work survives a process restart. The journal contains
/// paths and observation facts only, never source bodies or runtime liveness.
/// Its revision prevents a future bounded worker from acknowledging newer
/// observations than the revision that worker claimed.
fn invalidate_catalog_source(
    transaction: &Transaction<'_>,
    path: &str,
    reason: &str,
) -> rusqlite::Result<()> {
    crate::storage::sqlite::source_journal::invalidate(
        transaction,
        &transaction.query_row("SELECT semon_namespace()", [], |row| {
            row.get::<_, String>(0)
        })?,
        path,
        reason,
    )
}

fn queue_tree_repair_tx(tx: &Transaction<'_>, path: &str, ledger: &Ledger) -> io::Result<()> {
    if ledger_row(tx, path)
        .map_err(io::Error::other)?
        .map(|(_, l)| l)
        .as_ref()
        != Some(ledger)
    {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    let failed: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?1",
            [format!("native_tree_failure:{path}")],
            |r| r.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    if failed.as_deref() == Some(&header_ledger(ledger))
        && Stat::of(Path::new(path)).ok().as_ref() == Some(&ledger.stat)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native tree compact metadata exceeds budget",
        ));
    }
    tx.execute("INSERT INTO native_tree_repairs(namespace,source_path,receipt) VALUES(semon_namespace(),?1,?2) ON CONFLICT(namespace,source_path) DO UPDATE SET state=CASE WHEN native_tree_repairs.state='done' THEN 'ready' ELSE native_tree_repairs.state END",params![path,header_ledger(ledger)]).map_err(io::Error::other)?;
    Ok(())
}

fn resolve_catalog_input_tx(
    tx: &Transaction<'_>,
    path: &str,
    revision: i64,
    native: Option<(&str, &str)>,
    log: Option<&str>,
) -> Result<Outcome, StoreError> {
    if semantic_stage::active(tx).map_err(failure)? {
        return Ok(Outcome::Conflict);
    }
    let actual: Option<i64> = tx.query_row(
        "SELECT revision FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1",
        [path], |r| r.get(0),
    ).optional().map_err(failure)?;
    if actual != Some(revision) {
        return Ok(Outcome::Conflict);
    }
    if let Some((harness, id)) = native {
        enqueue_native_tx(tx, harness, id).map_err(failure)?;
    }
    if let Some(log) = log {
        invalidate_bound_catalog_source(tx, log, "changed").map_err(failure)?;
    }
    // A dependency may have invalidated this same input. Never acknowledge
    // that newer claim with the revision captured before the dependency write.
    let removed = tx.execute(
        "DELETE FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1 AND revision=?2",
        params![path, revision],
    ).map_err(failure)?;
    if removed == 1 {
        tx.execute(
            "DELETE FROM main.session_source_mutations WHERE namespace=semon_namespace() AND source_path=?1",
            [path],
        ).map_err(failure)?;
    }
    Ok(Outcome::Written)
}

pub(super) enum Unopened {
    Other(String),
}

impl fmt::Display for Unopened {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

/// A closed, unchanged identity component can advance without revisiting other
/// event indexes. Repeat this metadata check inside the publication transaction.
fn current_catalog_row(
    connection: &Connection,
    key: &str,
) -> rusqlite::Result<Option<crate::model::summary::CatalogRow>> {
    let previous: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",
        [key], |row| row.get(0),
    ).optional()?;
    Ok(previous
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok()))
}

fn stable_native_role(previous: Option<&Value>, current: Option<&Value>) -> bool {
    if previous == current {
        return true;
    }
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    let mut previous = previous.clone();
    // Claude's independently observed sidecar owns these content-free fields.
    // Parent identity and the complete old/new relationship closure stay fenced.
    if let (Some(old), Some(new)) = (
        previous.get_mut("Agent").and_then(Value::as_object_mut),
        current.get("Agent").and_then(Value::as_object),
    ) {
        for field in ["tool_use_id", "label", "model", "cwd", "branch"] {
            let Some(value) = new.get(field) else {
                return false;
            };
            old.insert(field.to_owned(), value.clone());
        }
    }
    // A Codex parent rewrite is qualified by both old/current closure and its
    // newly resolved native parent. Other role changes require reconciliation.
    if let (Some(old), Some(new)) = (
        previous.get_mut("Codex").and_then(Value::as_object_mut),
        current.get("Codex").and_then(Value::as_object),
    ) && let Some(parent) = new.get("parent_thread")
    {
        old.insert("parent_thread".to_owned(), parent.clone());
    }
    previous == *current
}

fn stable_native_ids(
    connection: &Connection,
    previous: &crate::model::summary::CatalogRow,
    current: &crate::model::summary::CatalogRow,
) -> rusqlite::Result<bool> {
    for id in previous
        .native_ids
        .iter()
        .filter(|id| !current.native_ids.contains(id))
    {
        let retired: Vec<_> = previous
            .sources
            .iter()
            .filter(|source| &source.native_id == id)
            .collect();
        if retired.is_empty() {
            return Ok(false);
        }
        for source in retired {
            let path = source.path.to_string_lossy();
            if ledger_row(connection, &path)?.is_some() && !connection.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1 AND reason='removed')", [path.as_ref()], |row| row.get::<_,bool>(0))? { return Ok(false); }
        }
    }
    Ok(true)
}

fn closed_catalog(
    connection: &Connection,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
) -> rusqlite::Result<bool> {
    let complete: bool = connection.query_row(
        "SELECT COALESCE((SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_completeness') AND key='catalog_completeness'),'complete')='complete' AND COALESCE((SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_version') AND key='catalog_version'),'')=?1",
        [crate::model::summary::CATALOG_VERSION.to_string()], |row| row.get(0),
    )?;
    if !complete || claims.is_empty() || claims.len() > 64 || publication.rows.len() > 64 {
        return Ok(false);
    }
    let claimed: std::collections::BTreeSet<_> =
        claims.iter().map(|(path, _)| path.as_str()).collect();
    for (path, _) in claims {
        let mut owners = connection.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE source_path=?1 AND lifecycle='current' LIMIT 65")?;
        for owner in owners.query_map([path], |row| row.get::<_, String>(0))? {
            let Some(previous) = current_catalog_row(connection, &owner?)? else {
                return Ok(false);
            };
            if previous
                .sources
                .iter()
                .any(|source| !claimed.contains(source.path.to_string_lossy().as_ref()))
            {
                return Ok(false);
            }
        }
    }
    let keys = publication
        .rows
        .iter()
        .map(|row| row.key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if keys.len() != publication.rows.len() {
        return Ok(false);
    }
    let encoded_keys = serde_json::to_string(&keys).map_err(|_| rusqlite::Error::InvalidQuery)?;
    for row in publication.rows {
        if row
            .parent
            .as_deref()
            .is_some_and(|parent| !keys.contains(parent))
            || row.sources.is_empty()
            || row.native_ids.is_empty()
            || !row
                .trace
                .as_ref()
                .is_some_and(|trace| trace.closed_over(&keys))
        {
            return Ok(false);
        }
        if let Some(previous) = current_catalog_row(connection, &row.key)? {
            if previous.harness != row.harness
                || previous.kind != row.kind
                || !stable_native_ids(connection, &previous, row)?
                || previous.sources.iter().any(|previous| {
                    row.sources
                        .iter()
                        .filter(|source| source.native_id == previous.native_id)
                        .any(|source| {
                            !stable_native_role(
                                previous.native_role.as_ref(),
                                source.native_role.as_ref(),
                            )
                        })
                })
            {
                return Ok(false);
            }
        } else {
            // A genuinely new identity can join a complete Catalog. Missing or
            // corrupt metadata for an existing row must never masquerade as one.
            let exists: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1)",
                [&row.key],
                |row| row.get(0),
            )?;
            if exists {
                let retained: Option<Option<String>>=connection.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1 AND lifecycle='retained'",[&row.key],|row|row.get(0)).optional()?;
                let Some(previous) = retained.flatten().and_then(|metadata| {
                    serde_json::from_str::<crate::model::summary::CatalogRow>(&metadata).ok()
                }) else {
                    return Ok(false);
                };
                if previous.harness != row.harness || previous.native_ids != row.native_ids {
                    return Ok(false);
                }
            }
        }
        let related: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE lifecycle='current' AND parent_key=?1 AND session_key NOT IN (SELECT value FROM json_each(?2))) OR EXISTS(SELECT 1 FROM session_trace_edges WHERE session_key=?1 AND CASE WHEN json_valid(metadata) THEN ((json_extract(metadata,'$.from') IS NULL OR json_extract(metadata,'$.from')='you' OR json_extract(metadata,'$.from') IN (SELECT value FROM json_each(?2))) AND (json_extract(metadata,'$.to') IS NULL OR json_extract(metadata,'$.to')='you' OR json_extract(metadata,'$.to') IN (SELECT value FROM json_each(?2)))) ELSE 0 END IS NOT 1) OR EXISTS(SELECT 1 FROM session_trace_edges WHERE (CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?1 OR CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?1) AND session_key NOT IN (SELECT value FROM json_each(?2))) OR EXISTS(SELECT 1 FROM session_trace_bookmarks WHERE session_key=?1)",
            params![row.key, encoded_keys], |row| row.get(0),
        )?;
        if related {
            return Ok(false);
        }
        for source in &row.sources {
            let external: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE lifecycle='current' AND source_path=?1 AND session_key NOT IN (SELECT value FROM json_each(?2)))",
                params![source.path.to_string_lossy(), encoded_keys], |row| row.get(0),
            )?;
            if external {
                return Ok(false);
            }
        }
        for id in &row.native_ids {
            let external: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 AND session_key NOT IN (SELECT value FROM json_each(?3)))", params![row.harness,id,encoded_keys], |row| row.get(0))?;
            if external {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(Clone, Copy)]
enum CatalogWriter {
    Auto,
    #[cfg(test)]
    RowWise,
    #[cfg(test)]
    Traversal {
        owners: usize,
        measured: bool,
    },
}

/// This is optimization eligibility, not a new publication acceptance limit.
/// Payloads are already serialized by the original publisher and stay borrowed.
fn catalog_batch_bytes(
    rows: &[crate::model::summary::CatalogRow],
    metadata: &[String],
    writer: CatalogWriter,
) -> Option<usize> {
    match writer {
        CatalogWriter::Auto => {}
        #[cfg(test)]
        CatalogWriter::Traversal { .. } => {}
        #[cfg(test)]
        CatalogWriter::RowWise => return None,
    }
    if !(2..=64).contains(&rows.len()) || metadata.len() != rows.len() {
        return None;
    }
    let mut keys = std::collections::BTreeSet::new();
    let mut bytes = 0usize;
    for (row, metadata) in rows.iter().zip(metadata) {
        if !keys.insert(row.key.as_str()) {
            return None;
        }
        for text in [
            row.key.as_str(),
            row.harness.as_str(),
            row.repo.as_deref().unwrap_or(""),
            row.parent.as_deref().unwrap_or(""),
            metadata.as_str(),
        ] {
            bytes = bytes.checked_add(text.len())?;
        }
        if bytes > 1_048_576 {
            return None;
        }
    }
    Some(bytes)
}

impl SqliteStore {
    fn publish_catalog_with_generation(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: Option<&[(String, Option<i64>)]>,
        preserve_complete: bool,
        semantic: Option<&crate::model::relation_stage::Batch>,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        self.publish_catalog_with_writer(
            publication,
            expected_generation,
            claims,
            preserve_complete,
            semantic,
            CatalogWriter::Auto,
        )
    }

    fn publish_catalog_with_writer(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
        claims: Option<&[(String, Option<i64>)]>,
        preserve_complete: bool,
        semantic: Option<&crate::model::relation_stage::Batch>,
        writer: CatalogWriter,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        let ownership = self.owner.clone();
        let owner = &ownership;
        #[cfg(any(test, feature = "test-hooks"))]
        if crate::test_hooks::catalog_paused(&self.path) {
            return Ok((Outcome::Conflict, None));
        }
        self.semantic_rejection = None;
        let rows = publication.rows;
        #[cfg(test)]
        {
            self.traversal_observation = None;
        }
        #[cfg(test)]
        let mut search_observation: Option<crate::native_timing::SearchTraversalProbe> = None;
        let producer =
            crate::native_timing::enabled().then(|| crate::native_timing::id(&self.path));
        let mut commit_boundary =
            crate::native_timing::CatalogCommitBoundary::new(producer.is_some(), &self.path);
        let mut write_activity = crate::native_timing::WriteActivity::new(
            producer.is_some(),
            &self.path,
            "catalog",
            &self.connection,
        );
        let mut stage_started = std::time::Instant::now();
        let mut stage = |name: &'static str, connection: &Connection| {
            if let Some(producer) = &producer {
                let now = std::time::Instant::now();
                eprintln!(
                    "semon sessions native: producer={producer} publication rows={} stage={name} ms={}",
                    rows.len(),
                    now.duration_since(stage_started).as_millis()
                );
                write_activity.stage(name, connection);
                stage_started = std::time::Instant::now();
            }
        };
        if claims.is_some_and(|claims| claims.len() > 64) || (claims.is_some() && rows.len() > 64) {
            return Err(StoreError::Data(
                "scoped catalog source budget exceeded".into(),
            ));
        }
        use sha2::{Digest, Sha256};
        let version = crate::model::summary::CATALOG_VERSION.to_string();
        let metadata: Vec<_> = rows
            .iter()
            .map(json)
            .collect::<Result<_, _>>()
            .map_err(failure)?;
        let batch_bytes = catalog_batch_bytes(rows, &metadata, writer);
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(failure)?;
        let mut published = None;
        let result = (|| {
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failure)?;
            owner
                .validate_ready(&transaction)
                .map_err(StoreError::Configured)?;
            crate::storage::NativeReaderBinding::Semantic(owner.clone())
                .validate(&transaction, &owner.config().namespace)
                .map_err(StoreError::Configured)?;
            transaction
                .create_scalar_function(
                    "semon_semantic_publisher",
                    0,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8
                        | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
                    |_| Ok(1_i64),
                )
                .map_err(failure)?;
            let admitted = crate::storage::sqlite::semantic::AdmittedSource::native(owner.clone());
            if !current_contract(&transaction).map_err(failure)? {
                return Ok(Outcome::Stale);
            }
            use crate::storage::catalog::CatalogPublicationHead;
            let publication_head =
                crate::storage::sqlite::publication::admitted_head(&transaction, &admitted)
                    .map_err(publication_storage_failure)?;
            let previous = publication_head
                .base_generation()
                .map_err(publication_storage_failure)?;
            if previous.as_deref() != expected_generation {
                self.semantic_rejection = Some("publication_generation");
                return Ok(Outcome::Conflict);
            }
            if preserve_complete
                && !closed_catalog(&transaction, publication, claims.unwrap_or_default())
                    .map_err(failure)?
            {
                self.semantic_rejection = Some("closed_scope");
                return Ok(Outcome::Conflict);
            }
            if let Some(batch) = semantic {
                if let Some(reason) =
                    semantic_stage::batch_rejection(&transaction, batch).map_err(failure)?
                {
                    self.semantic_rejection = Some(reason);
                    return Ok(Outcome::Conflict);
                }
            } else if semantic_stage::active(&transaction).map_err(failure)? {
                self.semantic_rejection = Some("stage_owned");
                return Ok(Outcome::Conflict);
            }
            // Do not let a slow builder overwrite a newer complete catalog.
            // Verify source membership and consumed-prefix identities inside
            // the publication transaction, after any competing ledger writes.
            let mut sources = BTreeMap::new();
            for source in rows.iter().flat_map(|row| &row.sources) {
                sources.insert(source.path.to_string_lossy().into_owned(), source);
            }
            let committed: i64 = if claims.is_none() {
                current_file_count(&transaction, &sources).map_err(failure)?
            } else {
                i64::try_from(sources.len()).unwrap_or(-1)
            };
            if committed != i64::try_from(sources.len()).unwrap_or(-1) {
                self.semantic_rejection = Some("source_membership");
                return Ok(Outcome::Conflict);
            }
            let mut affected = std::collections::BTreeSet::new();
            if let Some(claims) = claims {
                let claimed: std::collections::BTreeSet<_> =
                    claims.iter().map(|(path, _)| path.as_str()).collect();
                if sources.keys().any(|path| !claimed.contains(path.as_str())) {
                    self.semantic_rejection = Some("unclaimed_source");
                    return Ok(Outcome::Conflict);
                }
                for path in &claimed {
                    if !sources.contains_key(*path)
                        && ledger_row(&transaction, path).map_err(failure)?.is_some()
                    {
                        let removed: bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1 AND reason='removed')",[*path],|row|row.get(0)).map_err(failure)?;
                        if !removed {
                            self.semantic_rejection = Some("source_missing_without_removal");
                            return Ok(Outcome::Conflict);
                        }
                    }
                }
                for (path, revision) in claims {
                    let actual: Option<i64> = transaction.query_row("SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",[path],|row|row.get(0)).optional().map_err(failure)?;
                    if actual != *revision {
                        self.semantic_rejection = Some("publication_claim");
                        return Ok(Outcome::Conflict);
                    }
                    let mut statement = transaction.prepare("SELECT session_key FROM session_catalog_sources WHERE lifecycle='current' AND source_path=?1 LIMIT 65").map_err(failure)?;
                    for owner in statement
                        .query_map([path], |row| row.get::<_, String>(0))
                        .map_err(failure)?
                    {
                        affected.insert(owner.map_err(failure)?);
                    }
                }
                for row in rows {
                    for id in &row.native_ids {
                        let mut statement = transaction.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 LIMIT 65").map_err(failure)?;
                        for owner in statement
                            .query_map(params![row.harness, id], |row| row.get::<_, String>(0))
                            .map_err(failure)?
                        {
                            affected.insert(owner.map_err(failure)?);
                        }
                    }
                }
                if affected.len() > 64 {
                    self.semantic_rejection = Some("owner_budget");
                    return Ok(Outcome::Conflict);
                }
                for key in &affected {
                    let mut statement = transaction.prepare("SELECT source_path FROM session_catalog_sources WHERE session_key=?1 AND lifecycle='current' LIMIT 65").map_err(failure)?;
                    for path in statement
                        .query_map([key], |row| row.get::<_, String>(0))
                        .map_err(failure)?
                    {
                        if !claimed.contains(path.map_err(failure)?.as_str()) {
                            self.semantic_rejection = Some("owner_scope");
                            return Ok(Outcome::Conflict);
                        }
                    }
                }
            }
            for (path, source) in &sources {
                let Some((_, ledger)) = ledger_row(&transaction, path).map_err(failure)? else {
                    self.semantic_rejection = Some("source_ledger_missing");
                    return Ok(Outcome::Conflict);
                };
                if !crate::storage::catalog::source_matches_ledger(source, &ledger) {
                    self.semantic_rejection = Some("source_identity");
                    return Ok(Outcome::Conflict);
                }
            }
            stage("fences", &transaction);
            // The same transaction owns replacement and withdrawal custody.
            // Two missing outgoing owners suffice: the shared algorithm
            // advances at most one and defers before changing membership.
            transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_members (session_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM catalog_members;").map_err(failure)?;
            for row in rows {
                transaction
                    .execute("INSERT INTO catalog_members VALUES (?1)", [&row.key])
                    .map_err(failure)?;
            }
            let mut withdrawn = Vec::new();
            if claims.is_none() {
                let mut statement = transaction.prepare("SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog c WHERE c.namespace=semon_namespace() AND c.lifecycle='current' AND c.session_key NOT IN (SELECT session_key FROM catalog_members) AND NOT EXISTS (SELECT 1 FROM main.session_history_catalog h WHERE h.namespace=c.namespace AND h.session_key=c.session_key) ORDER BY c.session_key LIMIT 2").map_err(failure)?;
                for text in statement
                    .query_map([], |record| record.get::<_, String>(0))
                    .map_err(failure)?
                {
                    withdrawn.push(
                        serde_json::from_str::<crate::model::summary::CatalogRow>(
                            &text.map_err(failure)?,
                        )
                        .map_err(|_| failure(rusqlite::Error::InvalidQuery))?,
                    );
                }
            } else {
                for key in &affected {
                    let text: Option<String> = transaction.query_row("SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog c WHERE c.namespace=semon_namespace() AND c.session_key=?1 AND c.lifecycle='current' AND c.session_key NOT IN (SELECT session_key FROM catalog_members) AND NOT EXISTS (SELECT 1 FROM main.session_history_catalog h WHERE h.namespace=c.namespace AND h.session_key=c.session_key)", [key], |record| record.get(0)).optional().map_err(failure)?;
                    if let Some(text) = text {
                        withdrawn.push(
                            serde_json::from_str::<crate::model::summary::CatalogRow>(&text)
                                .map_err(|_| failure(rusqlite::Error::InvalidQuery))?,
                        );
                        if withdrawn.len() == 2 {
                            break;
                        }
                    }
                }
            }
            let history =
                crate::storage::sqlite::history::OriginalHistory::admitted(&transaction, &admitted)
                    .map_err(StoreError::Configured)?;
            let preserved =
                crate::storage::catalog::preserve_rows(&history, publication.rows, &withdrawn);
            if !preserved.map_err(failure)? {
                transaction.commit().map_err(failure)?;
                return Ok(Outcome::Deferred);
            }
            crate::storage::catalog::publish_history_rows(
                &history,
                publication.rows,
                publication.transcripts,
            )
            .map_err(failure)?;
            stage("history", &transaction);
            let mut metadata_measurements =
                crate::native_timing::Measurements::new(producer.is_some());
            let mut overview_measurements =
                crate::native_timing::Measurements::new(producer.is_some());
            let setup_started = metadata_measurements.start();
            let previous_version: Option<String> = transaction
                .query_row(
                    "SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_version') AND key='catalog_version'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failure)?;
            metadata_measurements.record("setup", setup_started, 1);

            let mut metadata_sample = None;
            let mut cohort_changed = None;
            {
                let statements_started = metadata_measurements.start();
                // A temporary membership table bounds deletion SQL parameters and
                // never escapes the transaction or becomes part of the read model.
                {
                    let current_effects =
                        crate::storage::sqlite::publication::CurrentWriter::admitted(
                            &transaction,
                            &admitted,
                        )
                        .map_err(StoreError::Configured)?;
                    metadata_measurements.record("statements", statements_started, 1);
                    if batch_bytes.is_some() && producer.is_some() {
                        metadata_sample = Some(crate::native_timing::WriteActivity::new(
                            true,
                            &self.path,
                            "catalog_metadata_cohort",
                            &transaction,
                        ));
                    }
                    let trace_store = if rows.iter().any(|row| row.trace.is_some()) {
                        let store =
                            crate::storage::sqlite::trace::TraceWriter::owned(&transaction, owner)
                                .map_err(|error| {
                                    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                                })
                                .map_err(failure)?;
                        Some(store)
                    } else {
                        None
                    };
                    let bulk_changed = if batch_bytes.is_some() {
                        // UNIQUE keys were certified before reordering. These
                        // helpers read their own History/Overview, never another
                        // row's newly written Current/source membership.
                        for row in rows {
                            let started = metadata_measurements.start();
                            crate::overview::publish_current_owned(
                                &transaction,
                                row,
                                &mut overview_measurements,
                                trace_store.as_ref(),
                            )
                            .map_err(failure)?;
                            metadata_measurements.record("overview", started, 1);
                            let started = metadata_measurements.start();
                            metadata_measurements.record("catalog_members", started, 1);
                        }
                        if let Some(sample) = metadata_sample.as_mut() {
                            sample.stage("overview_and_members", &transaction);
                        }
                        let started = metadata_measurements.start();
                        let changed = crate::storage::sqlite::publication::put_catalog_batch(
                            &transaction,
                            rows,
                            &metadata,
                            owner.config().namespace.as_str(),
                        )
                        .map_err(failure)?;
                        metadata_measurements.record("catalog_rows", started, rows.len());
                        if let Some(sample) = metadata_sample.as_mut() {
                            sample.stage("catalog_rows", &transaction);
                        }
                        cohort_changed = Some(changed.iter().filter(|changed| **changed).count());
                        Some(changed)
                    } else {
                        None
                    };
                    for (position, (row, metadata)) in rows.iter().zip(&metadata).enumerate() {
                        // The first selected row may be unchanged. These
                        // intervals are nested in catalog_metadata, not
                        // additive publication totals.
                        let mut sample =
                            if bulk_changed.is_none() && position == 0 && producer.is_some() {
                                Some(metadata_sample.insert(
                                    crate::native_timing::WriteActivity::new(
                                        true,
                                        &self.path,
                                        "catalog_metadata_sample",
                                        &transaction,
                                    ),
                                ))
                            } else {
                                None
                            };
                        let changed = if let Some(changed) = &bulk_changed {
                            changed[position]
                        } else {
                            let started = metadata_measurements.start();
                            crate::overview::publish_current_owned(
                                &transaction,
                                row,
                                &mut overview_measurements,
                                trace_store.as_ref(),
                            )
                            .map_err(failure)?;
                            metadata_measurements.record("overview", started, 1);
                            if let Some(sample) = sample.as_mut() {
                                sample.stage("overview", &transaction);
                            }
                            let started = metadata_measurements.start();
                            if let Some(sample) = sample.as_mut() {
                                sample.stage("member_insert", &transaction);
                            }
                            let changed = usize::from(
                                crate::storage::catalog::CurrentPublicationStore::current(
                                    &current_effects,
                                    row,
                                    metadata,
                                )
                                .map_err(failure)?,
                            );
                            metadata_measurements.record("catalog_rows", started, 1);
                            if let Some(sample) = sample.as_mut() {
                                sample.stage(
                                    if changed > 0 {
                                        "catalog_rows_changed"
                                    } else {
                                        "catalog_rows_unchanged"
                                    },
                                    &transaction,
                                );
                            }
                            changed > 0
                        };
                        if changed || previous_version.as_deref() != Some(&version) {
                            crate::storage::catalog::publish_current_sources_measured(
                                &current_effects,
                                row,
                                &mut metadata_measurements,
                            )
                            .map_err(failure)?;
                        } else if let Some(sample) = sample.as_mut() {
                            sample.stage("source_retirement", &transaction);
                        }
                        // Native dependency and source DML are interleaved.
                        // All this owner's sources share one final boundary.
                        if let Some(sample) = sample.as_mut() {
                            sample.stage("sources_and_dependencies", &transaction);
                        }
                    }
                    if bulk_changed.is_some()
                        && let Some(sample) = metadata_sample.as_mut()
                    {
                        // Retirements and each source's native dependencies
                        // remain interleaved in original caller order.
                        sample.stage("sources_and_dependencies", &transaction);
                    }
                }
                let started = metadata_measurements.start();
                if claims.is_none() {
                    transaction.execute("UPDATE main.session_catalog SET lifecycle='retained', metadata=CASE WHEN json_valid(metadata) THEN json_set(metadata,'$.lifecycle','retained') ELSE metadata END WHERE namespace=semon_namespace() AND lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)", []).map_err(failure)?;
                    transaction.execute("UPDATE main.session_catalog_sources SET lifecycle='retained' WHERE namespace=semon_namespace() AND lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)", []).map_err(failure)?;
                } else {
                    for key in &affected {
                        transaction.execute("UPDATE main.session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE namespace=semon_namespace() AND session_key=?1 AND session_key NOT IN (SELECT session_key FROM catalog_members)",[key]).map_err(failure)?;
                        transaction.execute("UPDATE main.session_catalog_sources SET lifecycle='retained' WHERE namespace=semon_namespace() AND session_key=?1 AND session_key NOT IN (SELECT session_key FROM catalog_members)",[key]).map_err(failure)?;
                    }
                }
                metadata_measurements.record("lifecycle_retirement", started, 1);
            }
            let started = metadata_measurements.start();
            if claims.is_none() {
                transaction.execute("UPDATE main.session_history_catalog SET metadata=json_set(metadata,'$.lifecycle','retained') WHERE namespace=semon_namespace() AND session_key NOT IN (SELECT session_key FROM catalog_members)",[]).map_err(failure)?;
            } else {
                for key in &affected {
                    transaction.execute("UPDATE main.session_history_catalog SET metadata=json_set(metadata,'$.lifecycle','retained') WHERE namespace=semon_namespace() AND session_key=?1 AND session_key NOT IN (SELECT session_key FROM catalog_members)",[key]).map_err(failure)?;
                }
            }
            metadata_measurements.record("history_retirement", started, 1);
            if let Some(producer) = &producer {
                metadata_measurements.emit(producer, "catalog_metadata");
                overview_measurements.emit(producer, "overview_publish");
            }
            stage("catalog_metadata", &transaction);
            {
                let source = &admitted;
                // Physical effects are namespace-owned; metadata tokenization,
                // retained recipes and posting counts use the existing producer.
                let search =
                    crate::storage::sqlite::search::SearchWriter::admitted(&transaction, source)
                        .map_err(StoreError::Configured)?;
                // Empty Current is a real publication too. No row batch will
                // initialize its two namespace-owned coverage records.
                crate::storage::catalog::SearchPublicationStore::begin(&search, &[])
                    .map_err(failure)?;
                #[allow(unused_mut)]
                let mut refresh = |encoded: &[String]| -> Result<(), StoreError> {
                    let refreshed = encoded
                        .iter()
                        .map(|value| {
                            serde_json::from_str::<crate::model::summary::CatalogRow>(value)
                                .map_err(|_| failure(rusqlite::Error::InvalidQuery))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let refs = refreshed.iter().collect::<Vec<_>>();
                    #[cfg(test)]
                    if let CatalogWriter::Traversal { owners, measured } = writer {
                        return crate::catalog_search::publish_current_rows_effects(
                            &transaction,
                            &search,
                            &refs,
                            owners,
                            measured.then_some(&mut search_observation),
                        )
                        .map_err(failure);
                    }
                    crate::catalog_search::publish_current_rows_with_store(
                        &transaction,
                        &refs,
                        &search,
                    )
                    .map_err(failure)
                };
                if claims.is_none() {
                    transaction.execute_batch("DELETE FROM main.session_search_memberships WHERE namespace=semon_namespace(); DELETE FROM main.session_search_counts WHERE namespace=semon_namespace(); DELETE FROM main.session_search_documents WHERE namespace=semon_namespace(); UPDATE main.session_search_coverage SET incomplete=0 WHERE namespace=semon_namespace();").map_err(failure)?;
                    let mut after = String::new();
                    loop {
                        let mut encoded = Vec::new();
                        let mut bytes = 0usize;
                        let mut statement = transaction.prepare("SELECT session_key,octet_length(metadata),CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key>?1 ORDER BY session_key LIMIT 64").map_err(failure)?;
                        let mut values = statement.query([&after]).map_err(failure)?;
                        while let Some(value) = values.next().map_err(failure)? {
                            let size = usize::try_from(value.get::<_, i64>(1).map_err(failure)?)
                                .map_err(|_| failure(rusqlite::Error::InvalidQuery))?;
                            if size > 1048576 {
                                return Err(failure(rusqlite::Error::InvalidQuery));
                            }
                            if bytes + size > 1048576 {
                                break;
                            }
                            after = value.get(0).map_err(failure)?;
                            encoded.push(value.get::<_, String>(2).map_err(failure)?);
                            bytes += size;
                        }
                        drop(values);
                        drop(statement);
                        if encoded.is_empty() {
                            break;
                        }
                        refresh(&encoded)?;
                    }
                } else {
                    let mut keys = affected.clone();
                    keys.extend(rows.iter().map(|row| row.key.clone()));
                    let mut encoded = Vec::new();
                    let mut bytes = 0usize;
                    for key in keys {
                        let value = transaction.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1", [&key], |r|r.get::<_, String>(0)).optional().map_err(failure)?;
                        if let Some(value) = value {
                            if bytes + value.len() > 1048576 || encoded.len() == 64 {
                                refresh(&encoded)?;
                                encoded.clear();
                                bytes = 0;
                            }
                            bytes += value.len();
                            encoded.push(value);
                        }
                    }
                    refresh(&encoded)?;
                }
                transaction.execute("INSERT INTO main.meta(namespace,key,value) VALUES(semon_meta_scope('search_backfill_complete'),'search_backfill_complete','1') ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value", []).map_err(failure)?;
            }
            stage("search", &transaction);
            let generation = {
                let mut return_component_generation = None;
                let mut digest = Sha256::new();
                digest.update(version.as_bytes());
                if claims.is_some() {
                    transaction.execute("INSERT INTO main.meta(namespace,key,value) VALUES (semon_meta_scope('catalog_publication_epoch'),'catalog_publication_epoch','1') ON CONFLICT(namespace,key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)",[]).map_err(failure)?;
                    let epoch: String = transaction
                        .query_row(
                            "SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_publication_epoch') AND key='catalog_publication_epoch'",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(failure)?;
                    return_component_generation =
                        Some(crate::storage::catalog::component_generation(
                            &version,
                            previous.as_deref(),
                            &epoch,
                            &metadata,
                        ));
                } else {
                    for table in ["session_catalog", "session_history_catalog"] {
                        digest.update(table.as_bytes());
                        let mut statement = transaction
                            .prepare(&format!(
                                "SELECT metadata FROM {table} ORDER BY session_key"
                            ))
                            .map_err(failure)?;
                        let mut values = statement.query([]).map_err(failure)?;
                        while let Some(row) = values.next().map_err(failure)? {
                            let metadata: String = row.get(0).map_err(failure)?;
                            digest.update((metadata.len() as u64).to_le_bytes());
                            digest.update(metadata.as_bytes());
                        }
                    }
                }
                return_component_generation.unwrap_or_else(|| format!("{:x}", digest.finalize()))
            };
            if previous.as_deref() != Some(&generation)
                || previous_version.as_deref() != Some(&version)
            {
                publication_head
                    .publish_generation(&generation, &version)
                    .map_err(publication_storage_failure)?;
            }
            // Closed component publication is coherent for its claimed closure.
            // Its owner advances the cohort observation clock only after all
            // pending revisions and any bounded metadata audit have completed.
            if !preserve_complete || claims.is_none() {
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES (semon_meta_scope('catalog_observed_at'),'catalog_observed_at',?1)",
                        [crate::model::now_ms().to_string()],
                    )
                    .map_err(failure)?;
            }
            stage("generation", &transaction);
            let slot_effects =
                crate::storage::sqlite::publication::SlotWriter::admitted(&transaction, &admitted)
                    .map_err(StoreError::Configured)?;
            if let Some(projections) = publication.transcripts {
                crate::storage::catalog::publish_slot_rows(&slot_effects, rows, projections)?;
                if claims.is_none() {
                    transaction.execute("DELETE FROM main.session_slots WHERE namespace=semon_namespace() AND session_key NOT IN (SELECT session_key FROM session_catalog)", []).map_err(failure)?;
                    transaction.execute("DELETE FROM main.session_slot_projections WHERE namespace=semon_namespace() AND session_key NOT IN (SELECT session_key FROM session_catalog)", []).map_err(failure)?;
                }
                transaction.execute("INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES (semon_meta_scope('slot_projection_catalog_generation'),'slot_projection_catalog_generation',?1)", [&generation]).map_err(failure)?;
            } else {
                transaction
                    .execute(
                        "DELETE FROM main.meta WHERE namespace=semon_meta_scope('slot_projection_catalog_generation') AND key='slot_projection_catalog_generation'",
                        [],
                    )
                    .map_err(failure)?;
            }
            stage("slots", &transaction);
            commit_boundary.capture(&transaction);
            for scope in ["current", "retained_history"] {
                let incomplete: i64 = transaction
                    .query_row(
                        "SELECT incomplete FROM session_search_coverage WHERE read_scope=?1",
                        [scope],
                        |row| row.get(0),
                    )
                    .map_err(failure)?;
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
                        params![format!("search_generation_{scope}"), generation],
                    )
                    .map_err(failure)?;
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
                        params![format!("search_incomplete_{scope}"), incomplete.to_string()],
                    )
                    .map_err(failure)?;
            }
            transaction
                .execute(
                    "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope('search_version'),'search_version',?1)",
                    [crate::catalog_search::VERSION],
                )
                .map_err(failure)?;
            // A complete publication validated every committed ledger and
            // source membership while holding the write transaction. Only now
            // can outstanding source observations be acknowledged atomically.
            if semantic.is_none() {
                if let Some(claims) = claims {
                    for (path, revision) in claims {
                        if let Some(revision) = revision {
                            transaction.execute("DELETE FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1 AND revision=?2",params![path,revision]).map_err(failure)?;
                        }
                    }
                } else {
                    transaction
                        .execute("DELETE FROM main.session_catalog_invalidations WHERE namespace=semon_namespace()", [])
                        .map_err(failure)?;
                }
                if let Some(claims) = claims {
                    for (path, _) in claims {
                        relocation::acknowledge(&transaction, path).map_err(failure)?;
                    }
                }
            }
            if let Some(batch) = semantic {
                semantic_stage::published(&transaction, batch, rows, &generation)
                    .map_err(failure)?;
            }
            transaction
                .execute(
                    "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES (semon_meta_scope('catalog_completeness'),'catalog_completeness',?1)",
                    [if claims.is_some() && !preserve_complete {
                        "partial"
                    } else {
                        "complete"
                    }],
                )
                .map_err(failure)?;
            let stream =
                owner
                    .config()
                    .expected_stream_epoch
                    .as_deref()
                    .ok_or(StoreError::Configured(
                        crate::storage::StorageError::ScopeChanged,
                    ))?;
            let mut grant_owners = std::collections::BTreeSet::new();
            grant_owners.extend(rows.iter().map(|row| row.key.clone()));
            grant_owners.extend(affected.iter().cloned());
            grant_owners.extend(withdrawn.iter().map(|row| row.key.clone()));
            for key in grant_owners {
                crate::storage::sqlite::proof_generations::reconcile_native_observation_owners(
                    &transaction,
                    owner.config().namespace.as_str(),
                    &key,
                    owner.admission(),
                    stream,
                )
                .map_err(StoreError::Configured)?;
            }
            for (row, output) in rows.iter().zip(&metadata) {
                crate::storage::sqlite::proof_generations::native_catalog_observation_grant(
                    &transaction,
                    owner.config().namespace.as_str(),
                    &row.key,
                    owner.admission(),
                    stream,
                    output,
                    if semantic.is_none() {
                        publication.observations
                    } else {
                        None
                    },
                )
                .map_err(StoreError::Configured)?;
            }
            transaction
                .execute(
                    "INSERT INTO main.meta(namespace,key,value) VALUES(?1,'catalog_native_publication',?2) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",
                    params![
                        owner.config().namespace.as_str(),
                        format!("{generation}:{}:{stream}", owner.admission())
                    ],
                )
                .map_err(failure)?;
            #[cfg(test)]
            let commit_before = search_observation
                .as_ref()
                .map(|_| crate::native_timing::ProbeSample::read(&transaction));
            // In simultaneous test-only modes, the old before-probe belongs
            // to tail and the old after-probe to postcommit. Neither sample is
            // inside the new actual-COMMIT clock or counter window. Hub's
            // production publisher has no traversal probes.
            commit_boundary.capture(&transaction);
            transaction.commit().map_err(failure)?;
            crate::pricing::runtime::notify_reprice();
            commit_boundary.capture(&self.connection);
            #[cfg(test)]
            if let Some(before) = commit_before {
                let after = crate::native_timing::ProbeSample::read(&self.connection);
                self.traversal_observation = Some(crate::native_timing::CatalogTraversalProbe {
                    search: search_observation.take().unwrap(),
                    commit: before.interval(&after),
                    changed_rows: cohort_changed
                        .expect("measured 64-owner cohort uses the real batched writer"),
                });
            }
            stage("commit", &self.connection);
            if let Some(sample) = metadata_sample {
                sample.committed();
            }
            if let (Some(producer), Some(bytes), Some(changed)) =
                (&producer, batch_bytes, cohort_changed)
            {
                eprintln!(
                    "semon sessions native: producer={producer} publication=committed operation=catalog_metadata_cohort rows={} changed={changed} bound_text_bytes={bytes}",
                    rows.len()
                );
            }
            published = Some(generation);
            Ok(Outcome::Written)
        })();
        self.connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(failure)?;
        if matches!(&result, Ok(Outcome::Written)) {
            commit_boundary.capture(&self.connection);
            commit_boundary.committed();
        }
        log_catalog_publish_attempt(
            &self.connection,
            &self.path,
            rows,
            &result,
            self.semantic_rejection,
        );
        result.map(|outcome| (outcome, published))
    }

    /// Every file's index, and the reported runs, in one read transaction.
    /// A file whose rows can't be read is left out: it is read from its log
    /// again when next seen, and its rows replaced.
    #[cfg(test)]
    fn load(&self) -> rusqlite::Result<Loaded> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut files = Vec::new();
        {
            let mut readers = Readers::new(&transaction)?;
            let mut statement = transaction.prepare(&format!(
                "SELECT {FILE_COLUMNS} FROM files WHERE namespace=semon_namespace() ORDER BY file_id"
            ))?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                match read_file(&mut readers, row) {
                    Ok(file) => files.push(file),
                    Err(error) if is_data_error(&error) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let reported_runs = read_runs(&transaction)?;
        let stamp = transaction
            .query_row(
                "SELECT value FROM meta WHERE namespace=semon_meta_scope('claude_json_stamp') AND key='claude_json_stamp'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|text| serde_json::from_str(&text).ok());
        transaction.commit()?;
        Ok(Loaded {
            files,
            reported_runs,
            stamp,
        })
    }

    /// Tree metadata and its ledger, without reading event/signal rows.
    fn read_tree(
        &self,
        path: &str,
        expected: &Stat,
    ) -> rusqlite::Result<Option<(Ledger, crate::native_tree::Metadata)>> {
        // The ledger and every evidence record belong to one SQLite snapshot.
        let transaction = self.connection.unchecked_transaction()?;
        if !current(&transaction)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let Some((_, ledger)) = ledger_row(&transaction, path)? else {
            return Ok(None);
        };
        if ledger.stat != *expected {
            return Ok(None);
        }
        #[cfg(test)]
        hook(&AFTER_TREE_LEDGER);
        let mut statement = transaction.prepare(native_metadata_value(&transaction)?)?;
        let Some(summary) = read_tree_summary(&transaction, &mut statement, path, &ledger)? else {
            return Ok(None);
        };
        if !chunk_proofs::header_ready(&transaction, path, &ledger)? {
            return Ok(None);
        }
        let Some(marker) = read_parent_marker(&mut statement, path, &ledger)? else {
            return Ok(None);
        };
        let Some(calls) = launch_calls::read(&transaction, path, &ledger)? else {
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

    /// `path`'s committed ledger and full index, read in one transaction.
    fn read_one(&self, path: &str) -> rusqlite::Result<Option<(Ledger, FileIndex)>> {
        // Dropped in reverse order, so the read transaction ends last.
        let transaction = self.connection.unchecked_transaction()?;
        let mut readers = Readers::new(&transaction)?;
        let mut statement =
            transaction.prepare(&format!("SELECT {FILE_COLUMNS} FROM files WHERE path = ?1"))?;
        let mut rows = statement.query([path])?;
        let found = match rows.next()? {
            Some(row) => Some(read_file(&mut readers, row)?),
            None => None,
        };
        Ok(found.map(|(_, ledger, index)| (ledger, index)))
    }

    /// Commits `path`'s new ledger and index in one `BEGIN IMMEDIATE`
    /// transaction, if its ledger row is still `expected`.
    ///
    /// With `base` (the index `expected` recorded, which `index` grew from),
    /// only the rows that differ from it are written: the appended ones, and
    /// earlier ones a line resolved in place (a tool call's result). Without
    /// it the file's rows are replaced.
    #[cfg(test)]
    fn write_one(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        base: Option<&FileIndex>,
        ledger: &Ledger,
        index: &FileIndex,
    ) -> rusqlite::Result<Outcome> {
        let changes = base.map(|base| super::DirtyRows::between(base, index));
        self.write_one_dirty(path, expected, changes.as_ref(), ledger, index)
    }

    fn write_one_dirty(
        &mut self,
        path: &str,
        expected: Option<&Ledger>,
        changes: Option<&super::DirtyRows>,
        ledger: &Ledger,
        index: &FileIndex,
    ) -> rusqlite::Result<Outcome> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !current(&transaction)? {
            return Ok(Outcome::Stale);
        }
        require_current_sql(&transaction)?;
        let layout = WriterLayout::new(&transaction)?;
        let found = ledger_row(&transaction, path)?;
        if found.as_ref().map(|(_, ledger)| ledger) != expected {
            return Ok(Outcome::Conflict);
        }
        if changes.is_none()
            && let Some((file_id, _)) = found
        {
            clear(&transaction, file_id)?;
        }
        write_index_changes(
            &transaction,
            path,
            ledger,
            index,
            changes,
            expected,
            &layout,
        )?;
        transaction.commit()?;
        crate::pricing::runtime::notify_reprice();
        Ok(Outcome::Written)
    }

    /// Drops files' rows: with a ledger, only if the row is still at it (or
    /// doesn't decode); without one, only if it still doesn't decode.
    fn remove(&mut self, files: &[(String, Option<Ledger>)]) -> rusqlite::Result<Outcome> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !current(&transaction)? {
            return Ok(Outcome::Stale);
        }
        require_current_sql(&transaction)?;
        remove_files_tx(&transaction, files, self.owner.config().namespace.as_str())?;
        transaction.commit()?;
        Ok(Outcome::Written)
    }

    /// Saves reported runs read from `~/.claude.json` and the stamp they
    /// were read at, and returns every reported run the store holds, which
    /// other processes may have added to. `None` when the store is stale.
    fn write_runs(
        &mut self,
        runs: &[ReportedRunSnapshot],
        stamp: &ReportedFileStamp,
    ) -> rusqlite::Result<Option<Vec<ReportedRunSnapshot>>> {
        let rows = super::reported_rows::plan(runs)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !current(&transaction)? {
            return Ok(None);
        }
        validate_prior_reported(&transaction, &rows)?;
        {
            let mut put = transaction.prepare(PUT_RUN_SCOPED)?;
            for row in rows {
                put_run(&mut put, &row.run)?;
            }
        }
        transaction.execute(
            "INSERT OR REPLACE INTO meta (namespace,key,value) VALUES (semon_meta_scope('claude_json_stamp'),'claude_json_stamp', ?1)",
            [json(stamp)?],
        )?;
        let all = match &self.selected_runs {
            Some(ids) => read_runs_selected(&transaction, ids)?,
            None => read_runs(&transaction)?,
        };
        transaction.commit()?;
        Ok(Some(all))
    }
}

/// Another process holds the write lock past [`BUSY_TIMEOUT`].
fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// A row that doesn't decode: rewritten from the log, not a reason to stop
/// using the store.
fn is_data_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
    )
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// `O_NOFOLLOW`, which `std` doesn't name: the file is never created or
/// opened through a symlink. Its value differs by platform (0 where unknown,
/// and the check before the open still refuses one).
#[cfg(unix)]
const O_NOFOLLOW: i32 = if cfg!(any(target_os = "macos", target_os = "ios")) {
    0x0100
} else if cfg!(target_os = "linux") {
    if cfg!(any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    )) {
        0o100_000
    } else {
        0o400_000
    }
} else {
    0
};

/// The one existing directory opening the store may tighten to `0700`:
/// Semon's state directory (`semon_state`) when the store is directly in
/// it, or a `.semon` directory the store is directly in. Nothing else and
/// nothing above either, so a store placed in a user's own tree (an
/// absolute `--cache` in a checkout named `semon`) never changes its modes.
fn tightened(path: &Path, semon_state: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    (parent == semon_state || parent.file_name().is_some_and(|name| name == ".semon"))
        .then(|| parent.to_owned())
}

/// Tightens the existing store and its SQLite siblings without creating any
/// of them. Selected opens call this only after read-only qualification.
fn make_existing_private(path: &Path, semon_state: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("its path is a symlink"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    options.custom_flags(O_NOFOLLOW);
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for sibling_path in [
            path.to_owned(),
            sibling(path, "-wal"),
            sibling(path, "-shm"),
        ] {
            if let Ok(metadata) = fs::symlink_metadata(&sibling_path)
                && metadata.is_file()
                && metadata.permissions().mode() & 0o077 != 0
            {
                fs::set_permissions(&sibling_path, fs::Permissions::from_mode(0o600))?;
            }
        }
        let owner = file.metadata()?.uid();
        if let Some(state) = tightened(path, semon_state)
            && let Ok(metadata) = fs::symlink_metadata(&state)
            && metadata.is_dir()
            && metadata.uid() == owner
            && metadata.permissions().mode() & 0o077 != 0
        {
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = semon_state;
        drop(file);
    }
    Ok(())
}

/// CI-only paired measurements reuse the producer's connection-local policy.
#[cfg(test)]
pub(crate) fn catalog_probe_connection(path: &Path) -> rusqlite::Result<Connection> {
    let store = current_test_store(path, &fixture_namespace());
    Ok(crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(store.owner.config()).unwrap(),
    )
    .unwrap()
    .into_test_connection())
}

/// Bounded selected reads qualify the storage contract in their own snapshot.
/// Archive marker consumers additionally validate the exact selected marker;
/// unrelated archive history is never traversed by a focused request.
/// A bound store is read only through its admitted namespace: the reader
/// connection carries the binding as `semon_namespace()`, which must be the
/// sole current-format admission; an unregistered connection fails closed.
pub(crate) fn require_current(connection: &Connection) -> io::Result<()> {
    if crate::storage::sqlite::namespace::is_bound(connection).map_err(io::Error::other)? {
        let namespace: String = connection
            .query_row("SELECT semon_namespace()", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        let namespace =
            crate::storage::LogicalSourceNamespace::new(namespace).map_err(io::Error::other)?;
        crate::storage::sqlite::namespace::require_admitted(connection, &namespace)
            .map_err(io::Error::other)?;
        return Ok(());
    }
    // The current runtime has no unbound reader.
    Err(io::Error::other(
        crate::storage::StorageError::NamespaceUpgradePending,
    ))
}

/// A writer qualifies the fixed-size current contract in its own transaction
/// before changing durable rows or proof metadata. Writers that touch archive
/// custody validate only the selected marker at that call site.
pub(crate) fn require_current_sql(connection: &Connection) -> rusqlite::Result<()> {
    if !current_contract(connection)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    configured::NativeOwnership::validate_physical(connection)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    require_current(connection).map_err(|_| rusqlite::Error::InvalidQuery)
}

/// The single current physical contract. Every source-owned family uses a
/// namespace-leading key; store-global metadata uses the reserved empty scope.
fn authored_storage_ddl() -> [&'static str; 22] {
    let [usage, repair] = native_usage::schema();
    [
        crate::storage::sqlite::publication::SLOT_SCHEMA,
        COMPOSITE_CATALOG_SCHEMA,
        semantic_stage::support_schema(),
        metadata_schema(),
        CATALOG_SEARCH_SCHEMA,
        configured::CORE_SCHEMA,
        copied_usage_schema(),
        usage,
        repair,
        tree_state::schema(),
        semantic_stage::schema(),
        semantic_stage::SEMANTIC_CORE_INDEXES_NS,
        semantic_stage::metadata_indexes(),
        crate::overview::SCHEMA,
        crate::storage::sqlite::trace::SCHEMA,
        crate::storage::sqlite::history::SCHEMA,
        crate::storage::sqlite::search::SCHEMA,
        crate::storage::sqlite::changes::SCHEMA,
        SOURCE_OBSERVATION_RECEIPTS_SCHEMA,
        reprice::queue_schema(),
        reprice::catalog_schema(),
        reprice::file_schema(),
    ]
}

/// Qualify the complete authored current profile without changing the store.
/// Reference databases contain only schema and seed metadata, never user data.
/// Comparing definitions also checks columns, keys, constraints and trigger
/// bodies; an object with the right name alone cannot qualify a corrupt family.
pub(crate) fn current_objects_present(connection: &Connection) -> rusqlite::Result<bool> {
    static INVENTORY: OnceLock<Mutex<Option<std::sync::Arc<String>>>> = OnceLock::new();
    let mut inventory = INVENTORY
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if inventory.is_none() {
        let reference = Connection::open_in_memory()?;
        tree_state::register(&reference)?;
        for chunk in authored_storage_ddl() {
            reference.execute_batch(chunk)?;
        }
        let objects = reference.prepare("SELECT type,name,sql FROM main.sqlite_schema WHERE sql IS NOT NULL AND type IN ('table','index','trigger') ORDER BY type,name")?
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?
            .map(|row| row.map(|(kind, name, sql)| format!("('{}','{}','{}')", kind.replace('\'', "''"), name.replace('\'', "''"), sql.replace('\'', "''"))))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        *inventory = Some(std::sync::Arc::new(format!(
            "SELECT count(*)={} FROM main.sqlite_schema WHERE (type,name,sql) IN (VALUES {})",
            objects.len(),
            objects.join(",")
        )));
    }
    let sql = inventory
        .as_ref()
        .cloned()
        .ok_or(rusqlite::Error::InvalidQuery)?;
    drop(inventory);
    connection
        .prepare_cached(sql.as_str())?
        .query_row([], |row| row.get(0))
}

fn parser_version(connection: &Connection) -> rusqlite::Result<Option<u32>> {
    Ok(connection
        .prepare_cached("SELECT value FROM meta WHERE namespace='' AND key = 'cache_version'")?
        .query_row([], |row| row.get::<_, String>(0))
        .optional()?
        .and_then(|value| value.parse().ok()))
}

/// The store still has this binary's schema and parser: no other version
/// replaced it since it was opened.
pub(crate) fn current(connection: &Connection) -> rusqlite::Result<bool> {
    crate::native_timing::discovery_count(|counts| counts.statements += 1);
    Ok(current_contract(connection)? && require_current(connection).is_ok())
}

pub(crate) fn current_contract(connection: &Connection) -> rusqlite::Result<bool> {
    crate::native_timing::discovery_count(|counts| {
        counts.statements += 3;
        counts.schema_probes += 1;
    });
    let schema: i64 = connection
        .prepare_cached("PRAGMA user_version")?
        .query_row([], |row| row.get(0))?;
    let application: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    Ok(application == 0
        && schema == SCHEMA_VERSION
        && parser_version(connection)? == Some(CACHE_VERSION))
}

/// A versioned retention marker in the existing metadata region. Event rows
/// remain in their original tables; no duplicate index or body store is created.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivedIndex {
    version: u32,
    parser_version: u32,
    source: crate::SessionSourceRef,
    stat: Stat,
    projection_version: u32,
    projection_generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_header_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_chunks_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_marker_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launch_calls_sha256: Option<String>,
}

impl ArchivedIndex {
    fn matches(&self, ledger: &Ledger) -> bool {
        self.version == 6
            && self.source_chunks_sha256.as_ref().is_some_and(|hash| {
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            && (if self.source.root == "codex"
                || (self.source.root == "claude"
                    && crate::native_sidecar::agent_path(Path::new(&self.source.path)))
            {
                self.native_header_sha256.as_ref().is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            } else {
                self.native_header_sha256.is_none()
            })
            && (if matches!(self.source.root.as_str(), "claude" | "codex") {
                self.parent_marker_sha256.as_ref().is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            } else {
                self.parent_marker_sha256.is_none()
            })
            && self.launch_calls_sha256.as_ref().is_some_and(|hash| {
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            && self.parser_version == CACHE_VERSION
            && crate::is_input_path(&self.source.root, &self.source.path)
            && !self.source.native_id.is_empty()
            && self.source.native_id.len() <= 256
            && self.stat == ledger.stat
            && self.source.offset == ledger.offset
            && self.source.prefix_sha256 == ledger.prefix
            && self.source.tail_sha256 == ledger.tail
    }
}

fn archive_index_key(path: &str) -> String {
    format!("archive_event_index:{path}")
}

/// Complete publication still fences all current ledgers. Only exact archived
/// generations absent from this observation may remain outside its membership;
/// an unmarked or concurrently advanced source still causes a conflict.
fn current_file_count(
    connection: &Connection,
    sources: &BTreeMap<String, &crate::model::summary::CatalogSource>,
) -> rusqlite::Result<i64> {
    let mut count: i64 = connection.query_row(
        "SELECT count(*) FROM files WHERE namespace=semon_namespace()",
        [],
        |row| row.get(0),
    )?;
    let mut statement = connection.prepare(
        "SELECT f.file_id,f.dev,f.ino,f.size,f.mtime_ns,f.resume_at,f.head_sha256,f.tail_sha256,f.ctime_ns,f.path,\
         CASE WHEN octet_length(m.value)<=16384 THEN m.value ELSE NULL END \
         FROM meta m JOIN files f ON f.path=substr(m.key,length(?1)+1) WHERE f.namespace=semon_namespace() AND m.namespace=semon_namespace() AND m.key GLOB ?2",
    )?;
    let mut rows = statement.query(["archive_event_index:", "archive_event_index:*"])?;
    while let Some(row) = rows.next()? {
        let path: String = row.get(9)?;
        let record = archived_index(connection, &path)?.ok_or(rusqlite::Error::InvalidQuery)?;
        if sources.contains_key(&path) {
            continue;
        }
        let (_, ledger) = read_ledger(row)?;
        if record.matches(&ledger) {
            count -= 1;
        }
    }
    Ok(count)
}

fn archived_index(connection: &Connection, path: &str) -> rusqlite::Result<Option<ArchivedIndex>> {
    let encoded: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN octet_length(value)<=16384 THEN value ELSE NULL END FROM meta WHERE namespace=semon_namespace() AND key=?1",
        [archive_index_key(path)], |row| row.get(0),
    ).optional()?;
    encoded
        .map(|value| decode_archive_marker(&value.ok_or(rusqlite::Error::InvalidQuery)?))
        .transpose()
}

/// A retained index is historical custody, rather than a live Source ledger.
/// Share the exact retention predicate used by the ordinary removal writer.
pub(crate) fn archive_ledger_retained(
    connection: &Connection,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<bool> {
    Ok(archived_index(connection, path)?.is_some_and(|record| record.matches(ledger)))
}

fn decode_archive_marker(value: &str) -> rusqlite::Result<ArchivedIndex> {
    let record: ArchivedIndex =
        serde_json::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery)?;
    if record.version != 6 || record.parser_version != CACHE_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(record)
}

#[cfg(feature = "postgres")]
pub(crate) fn validate_archive_marker(value: &str) -> Result<(), StorageError> {
    decode_archive_marker(value)
        .map(|_| ())
        .map_err(|_| StorageError::ScopeChanged)
}

fn proof_ledger(proof: &crate::SessionSourceProof) -> Ledger {
    Ledger {
        stat: Stat {
            dev: proof.dev,
            ino: proof.ino,
            size: proof.size,
            modified_ns: proof.modified_ns,
            changed_ns: proof.changed_ns,
        },
        offset: proof.source.offset,
        prefix: proof.source.prefix_sha256,
        tail: proof.source.tail_sha256,
    }
}

/// Called under the caller's immediate transaction and source-bound projection
/// check. Decode the entire selected index before agreeing to keep it; a ledger
/// alone cannot stand in for missing or malformed event metadata.
pub(crate) fn retain_archive_index(
    connection: &Connection,
    path: &str,
    proof: &crate::SessionSourceProof,
    projection: &crate::SourceProjectionReady,
) -> rusqlite::Result<bool> {
    if !current(connection)? {
        return Ok(false);
    }
    // Refuse unsupported retained custody metadata before replacing its marker.
    let _ = archived_index(connection, path)?;
    let expected = proof_ledger(proof);
    let native_header_sha256;
    let source_chunks_sha256;
    let parent_marker_sha256;
    let launch_calls_sha256;
    {
        let mut readers = Readers::new(connection)?;
        let mut statement =
            connection.prepare(&format!("SELECT {FILE_COLUMNS} FROM files WHERE path=?1"))?;
        let mut rows = statement.query([path])?;
        let Some(row) = rows.next()? else {
            return Ok(false);
        };
        let (_, ledger, index) = read_file(&mut readers, row)?;
        if ledger != expected {
            return Ok(false);
        }
        if index.source_chunks.is_none() {
            return Ok(false);
        }
        let Some(digest) = chunk_proofs::digest(connection, path)? else {
            return Ok(false);
        };
        source_chunks_sha256 = Some(digest);
        if index.handoff_index.is_none() {
            return Ok(false);
        }
        let Some(digest) = launch_calls::digest(connection, path)? else {
            return Ok(false);
        };
        launch_calls_sha256 = Some(digest);
        parent_marker_sha256 = if matches!(proof.source.root.as_str(), "claude" | "codex") {
            if index.marker_index.is_none() {
                return Ok(false);
            }
            let Some(digest) = parent_marker_digest(connection, path)? else {
                return Ok(false);
            };
            Some(digest)
        } else {
            None
        };
        native_header_sha256 = if proof.source.root == "codex" {
            if index
                .codex_header
                .as_ref()
                .and_then(|header| header.native_id(Path::new(path)))
                .as_deref()
                != Some(proof.source.native_id.as_str())
            {
                return Ok(false);
            }
            let Some(digest) = codex_header_digest(connection, path)? else {
                return Ok(false);
            };
            Some(digest)
        } else if proof.source.root == "claude"
            && crate::native_sidecar::agent_path(Path::new(path))
        {
            let Some(sidecar) = index.claude_sidecar.as_ref() else {
                return Ok(false);
            };
            if !sidecar.verified(Path::new(path)) {
                return Ok(false);
            }
            let Some(digest) = claude_sidecar_digest(connection, path)? else {
                return Ok(false);
            };
            Some(digest)
        } else {
            None
        };
    }
    let record = ArchivedIndex {
        version: 6,
        parser_version: CACHE_VERSION,
        source: proof.source.clone(),
        stat: expected.stat,
        projection_version: projection.projection_version,
        projection_generation: projection.projection_generation.clone(),
        native_header_sha256,
        source_chunks_sha256,
        parent_marker_sha256,
        launch_calls_sha256,
    };
    let encoded = serde_json::to_string(&record)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute(
        native_metadata_put(connection)?,
        params![archive_index_key(path), encoded],
    )?;
    Ok(true)
}

pub(crate) fn archive_index_retained(
    connection: &Connection,
    path: &str,
    proof: &crate::SessionSourceProof,
    projection: &crate::SourceProjectionReady,
) -> rusqlite::Result<bool> {
    if !current(connection)? {
        return Ok(false);
    }
    let Some((_, ledger)) = ledger_row(connection, path)? else {
        return Ok(false);
    };
    let Some(record) = archived_index(connection, path)? else {
        return Ok(false);
    };
    let header_matches = if proof.source.root == "codex" {
        record.version == 6
            && record.native_header_sha256.is_some()
            && record.native_header_sha256 == codex_header_digest(connection, path)?
    } else if proof.source.root == "claude" && crate::native_sidecar::agent_path(Path::new(path)) {
        let mut statement = connection.prepare(native_metadata_value(connection)?)?;
        let sidecar = read_claude_sidecar(&mut statement, path, &ledger)?;
        record.version == 6
            && record.native_header_sha256.is_some()
            && record.native_header_sha256 == claude_sidecar_digest(connection, path)?
            && sidecar.is_some_and(|sidecar| sidecar.verified(Path::new(path)))
    } else {
        record.version == 6
    };
    Ok(header_matches
        && record.launch_calls_sha256.is_some()
        && record.launch_calls_sha256 == launch_calls::digest(connection, path)?
        && launch_calls::read(connection, path, &ledger)?.is_some()
        && (if matches!(proof.source.root.as_str(), "claude" | "codex") {
            let mut statement = connection.prepare(native_metadata_value(connection)?)?;
            record.parent_marker_sha256.is_some()
                && record.parent_marker_sha256 == parent_marker_digest(connection, path)?
                && read_parent_marker(&mut statement, path, &ledger)?.is_some()
        } else {
            record.parent_marker_sha256.is_none()
        })
        && record.source_chunks_sha256.is_some()
        && record.source_chunks_sha256 == chunk_proofs::digest(connection, path)?
        && chunk_proofs::read(connection, path, &ledger)?.is_some()
        && ledger == proof_ledger(proof)
        && record.matches(&ledger)
        && record.source == proof.source
        && record.projection_version == projection.projection_version
        && record.projection_generation == projection.projection_generation)
}

// ---- Values --------------------------------------------------------------------------------

/// SQLite integers are signed: a `u64` is stored by its bits, so every value
/// round-trips exactly.
fn int(value: u64) -> i64 {
    value as i64
}

fn uint(value: i64) -> u64 {
    value as u64
}

fn seq_int(value: usize) -> i64 {
    value as i64
}

fn seq_of(value: i64) -> usize {
    value as usize
}

fn json<T: Serialize + ?Sized>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn from_json<T: DeserializeOwned>(column: usize, text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })
}

/// An enum's serde name (`Kind`, `SignalKind`), so a new variant needs no
/// table change and an unknown name fails to decode.
fn tag<T: Serialize>(value: &T) -> rusqlite::Result<String> {
    match serde_json::to_value(value) {
        Ok(Value::String(text)) => Ok(text),
        Ok(_) => Err(rusqlite::Error::ToSqlConversionFailure(
            "not a unit variant".into(),
        )),
        Err(error) => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
    }
}

const NATIVE_METADATA_MAX: usize = 1024 * 1024;

pub(crate) fn metadata_schema() -> &'static str {
    "CREATE TABLE IF NOT EXISTS meta(namespace TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(namespace,key)) STRICT, WITHOUT ROWID;"
}

const NATIVE_METADATA_VALUE: &str = "SELECT CASE WHEN octet_length(value)<=1048576 THEN value ELSE NULL END FROM meta WHERE namespace=semon_meta_scope(?1) AND key=?1";

fn native_metadata_value(_: &Connection) -> rusqlite::Result<&'static str> {
    Ok(NATIVE_METADATA_VALUE)
}

fn native_metadata_put(_: &Connection) -> rusqlite::Result<&'static str> {
    Ok(
        "INSERT INTO main.meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",
    )
}

fn metadata_increment(_: &Connection) -> rusqlite::Result<&'static str> {
    Ok(
        "INSERT INTO main.meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,'1') ON CONFLICT(namespace,key) DO UPDATE SET value=CAST(CAST(meta.value AS INTEGER)+1 AS TEXT)",
    )
}

fn native_metadata_delete(_: &Connection) -> rusqlite::Result<&'static str> {
    Ok("DELETE FROM main.meta WHERE namespace=semon_meta_scope(?1) AND key=?1")
}

use super::tree_rows::TreeSummaryRecord;

fn tree_summary_key(path: &str) -> String {
    format!("native_tree_summary:{path}")
}

fn tree_interpretation(connection: &Connection, path: &str) -> rusqlite::Result<String> {
    let mut statement = connection.prepare(native_metadata_value(connection)?)?;
    let mut values = Vec::<Option<String>>::new();
    for key in ["pricing_recompute_target", "catalog_semantic_recipe"] {
        let value: Option<Option<String>> = statement
            .query_row([key], |r| {
                crate::selected_read::row(r, 64)?;
                r.get(0)
            })
            .optional()?;
        if value == Some(None) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        values.push(value.flatten());
    }
    let pricing: Option<String> = connection.query_row(
        "SELECT codex_pricing_fingerprint FROM files WHERE namespace=semon_namespace() AND path=?1",
        [path],
        |r| {
            crate::selected_read::row(r, 64)?;
            r.get(0)
        },
    )?;
    values.push(pricing);
    super::tree_rows::interpretation(
        &values
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    )
    .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn tree_reusable(
    connection: &Connection,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare(native_metadata_value(connection)?)?;
    let encoded: Option<Option<String>> = statement
        .query_row([tree_summary_key(path)], |r| {
            crate::selected_read::row(r, 512)?;
            r.get(0)
        })
        .optional()?;
    let Some(encoded) = encoded.flatten() else {
        return Ok(false);
    };
    let projection = tree_state::witness(connection, path, ledger)?;
    let interpretation = tree_interpretation(connection, path)?;
    let Some(record) = TreeSummaryRecord::decode_current(
        encoded.as_bytes(),
        &header_ledger(ledger),
        projection.as_deref(),
        &interpretation,
    ) else {
        return Ok(false);
    };
    let Some(summary) = index.tree_summary.as_ref() else {
        return Ok(false);
    };
    if json(&record.summary)? != json(&summary.compact(index.native_usage_observations.is_some()))?
    {
        return Ok(false);
    }
    // Interpretation dependencies are independent of the Raw ledger. They are
    // checked again under the completion reservation, before preserving a Tree.
    if crate::native_sidecar::agent_path(Path::new(path))
        && read_claude_sidecar(&mut statement, path, ledger)?.as_ref()
            != index.claude_sidecar.as_ref()
    {
        return Ok(false);
    }
    Ok(json(&read_codex_header(&mut statement, path, ledger)?)? == json(&index.codex_header)?)
}

fn read_tree_summary(
    connection: &Connection,
    statement: &mut Statement<'_>,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<crate::native_tree::Summary>> {
    let failed: Option<Option<String>> = statement
        .query_row([format!("native_tree_failure:{path}")], |row| {
            crate::selected_read::row(row, 64)?;
            row.get(0)
        })
        .optional()?;
    if failed.is_some() {
        return Ok(None);
    }
    let encoded: Option<Option<String>> = statement
        .query_row([tree_summary_key(path)], |row| {
            crate::selected_read::row(row, 512)?;
            row.get(0)
        })
        .optional()?;
    let Some(text) = encoded.flatten() else {
        return Ok(None);
    };
    let projection = tree_state::witness(connection, path, ledger)?;
    let interpretation = tree_interpretation(connection, path)?;
    let Some(record) = TreeSummaryRecord::decode_current(
        text.as_bytes(),
        &header_ledger(ledger),
        projection.as_deref(),
        &interpretation,
    ) else {
        return Ok(None);
    };
    let mut summary = record.summary;
    if !tree_state::restore(connection, path, ledger, &mut summary)? {
        return Ok(None);
    }
    Ok(Some(summary))
}

fn write_tree_summary(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    summary: Option<&crate::native_tree::Summary>,
    normalized_usage: bool,
) -> rusqlite::Result<()> {
    if let Some(summary) = summary {
        let current = TreeSummaryRecord::new(
            header_ledger(ledger),
            tree_state::witness(transaction, path, ledger)
                .map_err(|error| tree_state::diagnose("witness", error))?,
            tree_interpretation(transaction, path)
                .map_err(|error| tree_state::diagnose("interpretation", error))?,
            summary,
            normalized_usage,
        );
        if current.projection_witness.is_some() {
            let mut statement = transaction.prepare(native_metadata_value(transaction)?)?;
            let encoded: Option<Option<String>> = statement
                .query_row([tree_summary_key(path)], |r| {
                    crate::selected_read::row(r, 512)?;
                    r.get(0)
                })
                .optional()
                .map_err(|error| tree_state::diagnose("summary_metadata_read", error))?;
            let failed: Option<Option<String>> = statement
                .query_row([format!("native_tree_failure:{path}")], |r| {
                    crate::selected_read::row(r, 64)?;
                    r.get(0)
                })
                .optional()
                .map_err(|error| tree_state::diagnose("summary_metadata_read", error))?;
            if failed.is_none() && encoded.flatten().as_deref() == Some(json(&current)?.as_str()) {
                return Ok(());
            }
        }
    }
    let encoded = summary
        .map(|summary| {
            tree_state::write(transaction, path, ledger, summary)
                .map_err(|error| tree_state::diagnose("write", error))?;
            json(&TreeSummaryRecord::new(
                header_ledger(ledger),
                tree_state::witness(transaction, path, ledger)
                    .map_err(|error| tree_state::diagnose("witness", error))?,
                tree_interpretation(transaction, path)
                    .map_err(|error| tree_state::diagnose("interpretation", error))?,
                summary,
                normalized_usage,
            ))
        })
        .transpose()?;
    if let Some(encoded) = encoded {
        if encoded.len() > NATIVE_METADATA_MAX {
            // Retain the source and current rows, but stop retrying a scalar
            // that cannot fit the compact projection budget.
            transaction.execute(
                native_metadata_put(transaction)?,
                params![format!("native_tree_failure:{path}"), header_ledger(ledger)],
            )?;
            transaction.execute(
                "UPDATE native_tree_repairs SET state='blocked_metadata' WHERE namespace=semon_namespace() AND source_path=?1",
                [path],
            )?;
            return Ok(());
        }
        transaction
            .prepare_cached(native_metadata_put(transaction)?)?
            .execute(params![tree_summary_key(path), encoded])?;
        transaction.execute(
            native_metadata_delete(transaction)?,
            [format!("native_tree_failure:{path}")],
        )?;
    }
    Ok(())
}

fn parent_marker_key(path: &str) -> String {
    format!("native_parent_marker:{path}")
}

fn read_parent_marker(
    statement: &mut Statement<'_>,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<crate::native_marker::Index>> {
    let encoded: Option<Option<String>> = statement
        .query_row([parent_marker_key(path)], |row| {
            crate::selected_read::row(row, 512)?;
            row.get(0)
        })
        .optional()?;
    Ok(encoded.flatten().and_then(|text| {
        let record: ParentMarkerRecord = serde_json::from_str(&text).ok()?;
        (record.version == 1
            && record.parser_version == CACHE_VERSION
            && record.ledger == header_ledger(ledger)
            && record.index.valid(ledger))
        .then_some(record.index)
    }))
}

fn parent_marker_digest(connection: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let text: Option<Option<String>> = connection
        .query_row(
            native_metadata_value(connection)?,
            [parent_marker_key(path)],
            |row| row.get(0),
        )
        .optional()?;
    Ok(text
        .flatten()
        .map(|text| format!("{:x}", Sha256::digest(text.as_bytes()))))
}

fn write_parent_marker(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: Option<&crate::native_marker::Index>,
) -> rusqlite::Result<()> {
    if let Some(encoded) = super::native_aux::parent_marker(ledger, index)
        .map_err(|_| rusqlite::Error::InvalidQuery)?
    {
        transaction
            .prepare_cached(native_metadata_put(transaction)?)?
            .execute(params![parent_marker_key(path), encoded])?;
    } else {
        transaction
            .prepare_cached(native_metadata_delete(transaction)?)?
            .execute([parent_marker_key(path)])?;
    }
    Ok(())
}

fn codex_header_key(path: &str) -> String {
    format!("native_codex_header:{path}")
}

pub(crate) fn header_ledger(ledger: &Ledger) -> String {
    let mut digest = Sha256::new();
    for value in [
        ledger.stat.dev,
        ledger.stat.ino,
        ledger.stat.size,
        ledger.offset,
    ] {
        digest.update(value.to_be_bytes());
    }
    digest.update(ledger.stat.modified_ns.to_be_bytes());
    digest.update([u8::from(ledger.stat.changed_ns.is_some())]);
    if let Some(changed) = ledger.stat.changed_ns {
        digest.update(changed.to_be_bytes());
    }
    digest.update(ledger.prefix);
    digest.update(ledger.tail);
    format!("{:x}", digest.finalize())
}

fn read_codex_header(
    statement: &mut Statement<'_>,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<crate::native_header::CodexHeader>> {
    let encoded: Option<Option<String>> = statement
        .query_row([codex_header_key(path)], |row| {
            crate::selected_read::row(row, 512)?;
            row.get(0)
        })
        .optional()?;
    Ok(encoded.flatten().and_then(|text| {
        let record: CodexHeaderRecord = serde_json::from_str(&text).ok()?;
        (record.version == 1
            && record.parser_version == CACHE_VERSION
            && record.ledger == header_ledger(ledger))
        .then_some(record.header)
    }))
}

fn codex_header_digest(connection: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let encoded: Option<Option<String>> = connection
        .query_row(
            native_metadata_value(connection)?,
            [codex_header_key(path)],
            |row| row.get(0),
        )
        .optional()?;
    Ok(encoded
        .flatten()
        .map(|value| format!("{:x}", Sha256::digest(value.as_bytes()))))
}

fn write_codex_header(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    header: Option<&crate::native_header::CodexHeader>,
) -> rusqlite::Result<()> {
    let encoded = super::native_aux::codex_header(ledger, header)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if let Some(encoded) = encoded.filter(|text| text.len() <= NATIVE_METADATA_MAX) {
        transaction
            .prepare_cached(native_metadata_put(transaction)?)?
            .execute(params![codex_header_key(path), encoded])?;
    } else {
        // Current readers keep their complete in-memory header. An oversized
        // projection cannot authorize eviction; do not truncate native fields.
        transaction
            .prepare_cached(native_metadata_delete(transaction)?)?
            .execute([codex_header_key(path)])?;
    }
    Ok(())
}

fn claude_sidecar_key(path: &str) -> String {
    format!("native_claude_sidecar:{path}")
}

fn read_claude_sidecar(
    statement: &mut Statement<'_>,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<crate::native_sidecar::ClaudeSidecar>> {
    let encoded: Option<Option<String>> = statement
        .query_row([claude_sidecar_key(path)], |row| {
            crate::selected_read::row(row, 512)?;
            row.get(0)
        })
        .optional()?;
    Ok(encoded.flatten().and_then(|text| {
        let record: ClaudeSidecarRecord = serde_json::from_str(&text).ok()?;
        (record.version == 1
            && record.parser_version == CACHE_VERSION
            && record.ledger == header_ledger(ledger)
            && record.sidecar.valid())
        .then_some(record.sidecar)
    }))
}

fn claude_sidecar_digest(connection: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let encoded: Option<Option<String>> = connection
        .query_row(
            native_metadata_value(connection)?,
            [claude_sidecar_key(path)],
            |row| row.get(0),
        )
        .optional()?;
    Ok(encoded
        .flatten()
        .map(|value| format!("{:x}", Sha256::digest(value.as_bytes()))))
}

fn write_claude_sidecar(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    sidecar: Option<&crate::native_sidecar::ClaudeSidecar>,
) -> rusqlite::Result<()> {
    if sidecar.is_none() && !crate::native_sidecar::agent_path(Path::new(path)) {
        return Ok(());
    }
    let encoded = super::native_aux::claude_sidecar(ledger, sidecar)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if let Some(encoded) = encoded.filter(|text| text.len() <= NATIVE_METADATA_MAX) {
        transaction
            .prepare_cached(native_metadata_put(transaction)?)?
            .execute(params![claude_sidecar_key(path), encoded])?;
    } else {
        transaction
            .prepare_cached(native_metadata_delete(transaction)?)?
            .execute([claude_sidecar_key(path)])?;
    }
    Ok(())
}

/// Rows of one sequence must number 0, 1, 2, …; anything else was written
/// by something else, and the file is read from its log again.
fn gap(what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        Type::Integer,
        format!("{what} out of sequence").into(),
    )
}

// ---- Reading -------------------------------------------------------------------------------

pub(crate) fn ledger_row(
    connection: &Connection,
    path: &str,
) -> rusqlite::Result<Option<(i64, Ledger)>> {
    connection
        .prepare_cached(
            "SELECT file_id, dev, ino, size, mtime_ns, resume_at, head_sha256, tail_sha256, ctime_ns FROM files WHERE namespace=semon_namespace() AND path=?1",
        )?
        .query_row(
            [path],
            read_ledger,
        )
        .optional()
}

fn read_ledger(row: &Row<'_>) -> rusqlite::Result<(i64, Ledger)> {
    crate::events::native_decode::ledger(&native_decode::values(row, &[])?)
        .map_err(|_| native_decode::malformed(0, Type::Text))
}

/// The per-file child queries, prepared once per read.
struct Readers<'c> {
    connection: &'c Connection,
    events: Statement<'c>,
    signals: Statement<'c>,
    usage: Statement<'c>,
    codex_usage: Statement<'c>,
    codex_header: Statement<'c>,
    claude_sidecar: Statement<'c>,
    parent_marker: Statement<'c>,
    tree_summary: Statement<'c>,
}

impl<'c> Readers<'c> {
    fn new(connection: &'c Connection) -> rusqlite::Result<Self> {
        Ok(Self {
            connection,
            events: connection.prepare(&format!(
                "SELECT {EVENT_COLUMNS} FROM events WHERE {}file_id = ?1 ORDER BY extra, seq",
                "namespace=semon_namespace() AND "
            ))?,
            signals: connection.prepare(&format!(
                "SELECT {SIGNAL_COLUMNS} FROM signals WHERE {}file_id = ?1 ORDER BY seq",
                "namespace=semon_namespace() AND "
            ))?,
            usage: connection.prepare(&format!(
                "SELECT {USAGE_COLUMNS} FROM usage WHERE {}file_id = ?1",
                "namespace=semon_namespace() AND "
            ))?,
            codex_usage: connection.prepare(&format!(
                "SELECT {CODEX_USAGE_COLUMNS} FROM codex_usage WHERE {}file_id = ?1 ORDER BY seq",
                "namespace=semon_namespace() AND "
            ))?,
            codex_header: connection.prepare(native_metadata_value(connection)?)?,
            claude_sidecar: connection.prepare(native_metadata_value(connection)?)?,
            parent_marker: connection.prepare(native_metadata_value(connection)?)?,
            tree_summary: connection.prepare(native_metadata_value(connection)?)?,
        })
    }
}

/// One `files` row (selected as [`FILE_COLUMNS`]) and its child rows.
fn read_file(
    readers: &mut Readers<'_>,
    row: &Row<'_>,
) -> rusqlite::Result<(String, Ledger, FileIndex)> {
    crate::selected_read::row(row, std::mem::size_of::<FileIndex>())?;
    #[cfg(test)]
    super::FILE_READS.with(|reads| reads.set(reads.get() + 1));
    let file_id: i64 = row.get(0)?;
    let path: String = row.get(1)?;
    let ledger = Ledger {
        stat: Stat {
            dev: uint(row.get(2)?),
            ino: uint(row.get(3)?),
            size: uint(row.get(4)?),
            modified_ns: u128::from_be_bytes(row.get(5)?),
            changed_ns: row.get::<_, Option<[u8; 16]>>(28)?.map(i128::from_be_bytes),
        },
        offset: uint(row.get(6)?),
        prefix: row.get(7)?,
        tail: row.get(8)?,
    };

    let mut events = Vec::new();
    let mut extras = Vec::new();
    let mut rows = readers.events.query([file_id])?;
    while let Some(event_row) = rows.next()? {
        crate::selected_read::row(event_row, std::mem::size_of::<Event>() + 32)?;
        crate::selected_read::checkpoint("selected_event")
            .map_err(crate::selected_read::sql_error)?;
        let (extra, seq, event) = read_event(event_row)?;
        let list = if extra { &mut extras } else { &mut events };
        if seq != list.len() {
            return Err(gap("events"));
        }
        list.push(event);
    }

    let mut signals = Vec::new();
    let mut rows = readers.signals.query([file_id])?;
    while let Some(signal_row) = rows.next()? {
        crate::selected_read::row(signal_row, std::mem::size_of::<Signal>() + 32)?;
        if seq_of(signal_row.get(0)?) != signals.len() {
            return Err(gap("signals"));
        }
        signals.push(
            crate::events::native_decode::signal(&native_decode::values(signal_row, &[])?)
                .map_err(|_| native_decode::malformed(0, Type::Text))?
                .1,
        );
    }

    let mut usage_by_id = BTreeMap::new();
    let mut rows = readers.usage.query([file_id])?;
    while let Some(usage_row) = rows.next()? {
        crate::selected_read::row(usage_row, std::mem::size_of::<MessageUsage>() + 64)?;
        let (id, usage) = read_usage(usage_row)?;
        usage_by_id.insert(id, usage);
    }

    let mut codex_usage_events = Vec::new();
    let mut rows = readers.codex_usage.query([file_id])?;
    while let Some(codex_row) = rows.next()? {
        crate::selected_read::row(codex_row, std::mem::size_of::<CodexUsageEvent>() + 32)?;
        if seq_of(codex_row.get(0)?) != codex_usage_events.len() {
            return Err(gap("codex usage"));
        }
        codex_usage_events.push(
            crate::events::native_decode::codex_usage(&native_decode::values(codex_row, &[])?)
                .map_err(|_| native_decode::malformed(0, Type::Text))?
                .1,
        );
    }

    let children = crate::events::native_decode::Children {
        native_usage_observations: native_usage::normalized(readers.connection, &path, &ledger)?
            .then(Vec::new),
        source_chunks: chunk_proofs::read(readers.connection, &path, &ledger)?,
        marker_index: read_parent_marker(&mut readers.parent_marker, &path, &ledger)?,
        tree_summary: read_tree_summary(
            readers.connection,
            &mut readers.tree_summary,
            &path,
            &ledger,
        )?,
        handoff_index: launch_calls::read(readers.connection, &path, &ledger)?,
        codex_header: read_codex_header(&mut readers.codex_header, &path, &ledger)?,
        claude_sidecar: if crate::native_sidecar::agent_path(Path::new(&path)) {
            read_claude_sidecar(&mut readers.claude_sidecar, &path, &ledger)?
        } else {
            None
        },
        events,
        extras,
        signals,
        usage_by_id,
        codex_usage_events,
        codex_native_usage: native_usage::restore(
            readers.connection,
            &path,
            &ledger,
            row.get(26)?,
        )?,
    };
    let index = crate::events::native_decode::file(&native_decode::values(row, &[13])?, children)
        .map_err(|_| native_decode::malformed(0, Type::Text))?;
    Ok((path, ledger, index))
}

/// One `events` row, selected as [`EVENT_COLUMNS`].
fn read_event(row: &Row<'_>) -> rusqlite::Result<(bool, usize, Event)> {
    crate::events::native_decode::event(&native_decode::values(row, &[0, 8, 9, 14, 18, 22])?)
        .map_err(|_| native_decode::malformed(0, Type::Text))
}

/// One `usage` row, selected as [`USAGE_COLUMNS`].
fn read_usage(row: &Row<'_>) -> rusqlite::Result<(String, MessageUsage)> {
    crate::events::native_decode::usage(&native_decode::values(row, &[21])?)
        .map_err(|_| native_decode::malformed(0, Type::Text))
}

fn read_runs(connection: &Connection) -> rusqlite::Result<Vec<ReportedRunSnapshot>> {
    let mut statement = connection.prepare(&reported_select_sql(false))?;
    let mut rows = statement.query([])?;
    let mut runs = Vec::new();
    while let Some(row) = rows.next()? {
        runs.push(read_run(row)?);
    }
    Ok(runs)
}

fn read_runs_selected(
    connection: &Connection,
    ids: &std::collections::BTreeSet<String>,
) -> rusqlite::Result<Vec<ReportedRunSnapshot>> {
    let mut statement = connection.prepare(&reported_select_sql(true))?;
    let mut runs = Vec::new();
    for id in ids {
        let mut rows = statement.query([id])?;
        while let Some(row) = rows.next()? {
            runs.push(read_run(row)?);
        }
    }
    Ok(runs)
}

fn reported_select_sql(selected: bool) -> String {
    if selected {
        format!(
            "SELECT {RUN_COLUMNS} FROM reported_runs WHERE namespace=semon_namespace() AND session_id=?1 ORDER BY start"
        )
    } else {
        format!(
            "SELECT {RUN_COLUMNS} FROM reported_runs WHERE namespace=semon_namespace() ORDER BY session_id,start"
        )
    }
}

fn validate_prior_reported(
    connection: &Connection,
    rows: &[super::reported_rows::ReportedRow],
) -> rusqlite::Result<()> {
    let scope = "namespace=semon_namespace() AND ";
    let mut statement = connection.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM reported_runs WHERE {scope}session_id=?1 AND start=?2"
    ))?;
    for row in rows {
        let mut prior =
            statement.query(params![row.run.last_session_id, row.run.last_start_time])?;
        if let Some(prior) = prior.next()? {
            read_run(prior)?;
        }
    }
    let stamp: Option<String> = connection.query_row("SELECT value FROM meta WHERE namespace=semon_meta_scope('claude_json_stamp') AND key='claude_json_stamp'",[],|row|row.get(0)).optional()?;
    if let Some(stamp) = stamp {
        super::reported_rows::decode_stamp(stamp.as_bytes())
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    } else {
        let exists = connection.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM reported_runs WHERE {scope}1=1 LIMIT 1)"),
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if exists {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

pub(crate) fn read_run(row: &Row<'_>) -> rusqlite::Result<ReportedRunSnapshot> {
    let model_usage: String = row.get(8)?;
    let run = ReportedRunSnapshot {
        last_session_id: row.get(0)?,
        last_start_time: row.get(1)?,
        last_cost: row.get(2)?,
        last_duration: row.get::<_, Option<i64>>(3)?.map(uint),
        last_api_duration: row.get::<_, Option<i64>>(4)?.map(uint),
        last_tool_duration: row.get::<_, Option<i64>>(5)?.map(uint),
        last_lines_added: row.get::<_, Option<i64>>(6)?.map(uint),
        last_lines_removed: row.get::<_, Option<i64>>(7)?.map(uint),
        last_model_usage: from_json(8, &model_usage)?,
        capture_at: row.get(9)?,
    };
    super::reported_rows::ReportedRow::new(&run)
        .and_then(|row| {
            if row.model_usage != model_usage {
                return Err(<serde_json::Error as serde::de::Error>::custom(
                    "noncanonical reported model usage",
                ));
            }
            Ok(row.run)
        })
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

// ---- Writing -------------------------------------------------------------------------------

/// Deletes a file's child rows, keeping its `files` row and id.
fn clear(transaction: &Transaction<'_>, file_id: i64) -> rusqlite::Result<()> {
    clear_preserving_tree(transaction, file_id, false)
}

fn clear_preserving_tree(
    transaction: &Transaction<'_>,
    file_id: i64,
    preserve_tree: bool,
) -> rusqlite::Result<()> {
    validate_copied_usage(transaction)?;
    for table in ["events", "signals"] {
        transaction.execute(
            &format!(
                "DELETE FROM {table} WHERE {}file_id=?1",
                "namespace=semon_namespace() AND "
            ),
            [file_id],
        )?;
    }
    for table in [
        "usage_identity",
        "usage",
        "codex_usage",
        "native_tree_copied_identities",
    ] {
        transaction.execute(
            &format!(
                "DELETE FROM {table} WHERE {}file_id = ?1",
                "namespace=semon_namespace() AND "
            ),
            [file_id],
        )?;
    }
    if !preserve_tree {
        transaction.execute(
            "DELETE FROM native_tree_headers WHERE namespace=semon_namespace() AND file_id=?1",
            [file_id],
        )?;
    }
    Ok(())
}

fn write_index_changes(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    changes: Option<&super::DirtyRows>,
    expected: Option<&Ledger>,
    layout: &WriterLayout<'_, '_>,
) -> rusqlite::Result<()> {
    write_index_changes_with_usage_deletes(
        transaction,
        path,
        ledger,
        index,
        changes,
        expected,
        &mut crate::native_timing::Measurements::default(),
        &transaction.query_row("SELECT semon_namespace()", [], |row| {
            row.get::<_, String>(0)
        })?,
        layout,
        &mut UsageDeletes::default(),
    )
}

#[allow(clippy::too_many_arguments)]
fn write_index_changes_measured(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    changes: Option<&super::DirtyRows>,
    expected: Option<&Ledger>,
    measurements: &mut crate::native_timing::Measurements,
    namespace: &str,
) -> rusqlite::Result<()> {
    let layout = WriterLayout::new(transaction)?;
    write_index_changes_with_usage_deletes(
        transaction,
        path,
        ledger,
        index,
        changes,
        expected,
        measurements,
        namespace,
        &layout,
        &mut UsageDeletes::default(),
    )
}

#[allow(clippy::too_many_arguments)]
fn write_index_changes_with_usage_deletes<'tx>(
    transaction: &'tx Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    changes: Option<&super::DirtyRows>,
    expected: Option<&Ledger>,
    measurements: &mut crate::native_timing::Measurements,
    namespace: &str,
    layout: &WriterLayout<'tx, '_>,
    usage_deletes: &mut UsageDeletes<'tx>,
) -> rusqlite::Result<()> {
    let _shape_probe = configured::WriterShapeProbe::new(measurements.is_enabled());
    layout.ensure_current()?;
    #[cfg(test)]
    let written_before: i64 = transaction.query_row("SELECT total_changes()", [], |r| r.get(0))?;
    // The source ledger fences raw bytes; the sidecar is an independent
    // interpretation dependency. Re-encoding derived proofs changes neither.
    let semantic_change = expected != Some(ledger)
        || (crate::native_sidecar::agent_path(Path::new(path)) && {
            let mut statement = transaction.prepare(native_metadata_value(transaction)?)?;
            read_claude_sidecar(&mut statement, path, ledger)?.as_ref()
                != index.claude_sidecar.as_ref()
        });
    crate::events::native_commit::write(
        &mut native_commit::Writer {
            transaction,
            layout,
            measurements,
            namespace,
            usage_deletes,
            relationship_started: None,
            relationship_rows_before: None,
            dirty_started: None,
        },
        crate::events::native_commit::Input {
            path,
            ledger,
            index,
            changes,
            expected,
            semantic_change,
        },
    )?;
    #[cfg(test)]
    {
        let written_after: i64 =
            transaction.query_row("SELECT total_changes()", [], |r| r.get(0))?;
        super::INDEX_WRITTEN_ROWS
            .with(|n| n.set(n.get() + (written_after - written_before) as u64));
    }
    Ok(())
}

// Actual SQLite row changes (including triggers), distinct from leaf input
// units. Reads are disabled together with timing and are inclusive per leaf.
fn record_writer_rows(
    transaction: &Transaction<'_>,
    measurements: &mut crate::native_timing::Measurements,
    label: &'static str,
    before: Option<u64>,
) {
    if let Some(before) = before {
        let rows = usize::try_from(transaction.total_changes().saturating_sub(before))
            .unwrap_or(usize::MAX);
        measurements.count(label, rows);
    }
}

/// Upserts `path`'s `files` row, then writes its child rows where `index`
/// differs from `old`.
#[cfg(test)]
fn write_file(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    changes: &super::DirtyRows,
    index: &FileIndex,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let layout = WriterLayout::new(transaction)?;
    write_file_with_usage_deletes(
        path,
        ledger,
        changes,
        index,
        measurements,
        &layout,
        &mut UsageDeletes::default(),
    )?;
    write_native_tree(transaction, path, ledger, index, measurements)?;
    for stage in crate::events::native_commit::AUXILIARY_ORDER {
        write_native_aux(transaction, path, ledger, index, measurements, stage)?;
    }
    Ok(())
}

fn write_native_aux(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    measurements: &mut crate::native_timing::Measurements,
    stage: crate::events::native_commit::Auxiliary,
) -> rusqlite::Result<()> {
    use crate::events::native_commit::Auxiliary;
    let FileIndex {
        codex_header,
        claude_sidecar,
        source_chunks,
        marker_index,
        handoff_index,
        ..
    } = index;
    match stage {
        Auxiliary::Header => {
            let started = measurements.start();
            write_codex_header(transaction, path, ledger, codex_header.as_ref())?;
            measurements.record("proof_header", started, 1);
            #[cfg(test)]
            configured::effect("proof_header")?;
        }
        Auxiliary::Sidecar => {
            let started = measurements.start();
            write_claude_sidecar(transaction, path, ledger, claude_sidecar.as_ref())?;
            measurements.record("proof_sidecar", started, 1);
            #[cfg(test)]
            configured::effect("proof_sidecar")?;
        }
        Auxiliary::Chunks => {
            let started = measurements.start();
            let rows_before = started.map(|_| transaction.total_changes());
            if started.is_some() {
                chunk_proofs::write_measured(
                    transaction,
                    path,
                    ledger,
                    source_chunks.as_deref(),
                    measurements,
                )?;
            } else {
                chunk_proofs::write(transaction, path, ledger, source_chunks.as_deref())?;
            }
            measurements.record("proof_chunks", started, 1);
            record_writer_rows(
                transaction,
                measurements,
                "proof_chunks_written_rows",
                rows_before,
            );
            #[cfg(test)]
            configured::effect("proof_chunks")?;
        }
        Auxiliary::Parent => {
            let started = measurements.start();
            write_parent_marker(transaction, path, ledger, marker_index.as_ref())?;
            measurements.record("proof_parent", started, 1);
            #[cfg(test)]
            configured::effect("proof_parent")?;
        }
        Auxiliary::Launch => {
            let started = measurements.start();
            if started.is_some() {
                launch_calls::write_measured(
                    transaction,
                    path,
                    ledger,
                    handoff_index.as_ref(),
                    measurements,
                )?;
            } else {
                launch_calls::write(transaction, path, ledger, handoff_index.as_ref())?;
            }
            measurements.record("proof_launch", started, 1);
            #[cfg(test)]
            configured::effect("proof_launch")?;
        }
    }
    Ok(())
}

fn write_native_tree(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let tree_summary = &index.tree_summary;
    let started = measurements.start();
    let rows_before = started.map(|_| transaction.total_changes());
    write_tree_summary(
        transaction,
        path,
        ledger,
        tree_summary.as_ref(),
        index.native_usage_observations.is_some(),
    )?;
    measurements.record("proof_tree", started, 1);
    record_writer_rows(
        transaction,
        measurements,
        "proof_tree_written_rows",
        rows_before,
    );
    #[cfg(test)]
    configured::effect("proof_tree")?;
    Ok(())
}

fn write_file_with_usage_deletes<'tx>(
    path: &str,
    ledger: &Ledger,
    changes: &super::DirtyRows,
    index: &FileIndex,
    measurements: &mut crate::native_timing::Measurements,
    layout: &WriterLayout<'tx, '_>,
    usage_deletes: &mut UsageDeletes<'tx>,
) -> rusqlite::Result<()> {
    let transaction = layout.transaction;
    let row_plan = super::native_rows::NativeRowPlan::new(index, changes);
    let file_row = super::file_rows::FileRow::new(ledger, index);
    let FileIndex {
        events,
        extras,
        signals,
        usage_by_id,
        codex_usage_events,
        ..
    } = index;
    let started = measurements.start();
    let rate_limits = file_row.rate_limits.as_ref().map(json).transpose()?;
    measurements.record("rate_limit_serialization", started, 1);
    let file_id: i64 = {
        let started = measurements.start();
        let mut statement = transaction.prepare_cached(configured::PUT_FILE)?;
        measurements.record("file_prepare", started, 1);
        let started = measurements.start();
        let parameters = params![
            path,
            int(file_row.dev),
            int(file_row.ino),
            int(file_row.size),
            file_row.modified_ns.to_be_bytes(),
            int(file_row.offset),
            file_row.prefix,
            file_row.tail,
            file_row.entrypoint,
            file_row.title,
            file_row.agent_name,
            file_row.last_model,
            file_row.failed,
            file_row.first,
            file_row.last,
            file_row.cwd,
            file_row.branch,
            json(file_row.pending)?,
            json(file_row.tool_ids)?,
            json(file_row.yields)?,
            json(file_row.busy)?,
            json(file_row.links)?,
            json(file_row.codex_tokens)?,
            json(file_row.codex_tokens_by_model)?,
            rate_limits,
            native_usage::encoded(file_row.index)?,
            file_row.copilot.as_ref().map(json).transpose()?,
            file_row.changed_ns.map(i128::to_be_bytes),
        ];
        measurements.record("file_parameters", started, 1);
        let started = measurements.start();
        let file_id = statement.query_row(parameters, |row| row.get(0))?;
        measurements.record("file_upsert", started, 1);
        #[cfg(test)]
        configured::effect("file_upsert")?;
        file_id
    };

    let started = measurements.start();
    let mut put = transaction.prepare_cached(configured::PUT_EVENT)?;
    let mut cut = transaction
        .prepare_cached("DELETE FROM events WHERE namespace=semon_namespace() AND file_id=?1 AND extra=?2 AND seq>=?3")?;
    for (extra, rows) in [(false, &row_plan.events), (true, &row_plan.extras)] {
        rows.apply(
            |seq, event| put_event(&mut put, file_id, extra, seq, event),
            |from| {
                cut.execute(params![file_id, extra, seq_int(from)])
                    .map(drop)
            },
        )?;
    }
    measurements.record("events", started, 1);
    #[cfg(test)]
    configured::effect("events")?;
    measurements.count("event_records", events.len().saturating_add(extras.len()));

    let started = measurements.start();
    let mut put = transaction.prepare_cached(configured::PUT_SIGNAL)?;
    row_plan.signals.apply(
        |seq, signal| {
            let Signal { k, o, t, at, n, v } = signal;
            put.execute(params![
                file_id,
                seq_int(seq),
                tag(k)?,
                int(*o),
                t,
                at,
                n,
                v.map(int)
            ])
            .map(drop)
        },
        |from| {
            transaction
                .execute(
                    "DELETE FROM signals WHERE namespace=semon_namespace() AND file_id=?1 AND seq>=?2",
                    params![file_id, seq_int(from)],
                )
                .map(drop)
        },
    )?;
    measurements.record("signals", started, 1);
    #[cfg(test)]
    configured::effect("signals")?;
    measurements.count("signal_records", signals.len());

    let started = measurements.start();
    let rows_before = started.map(|_| transaction.total_changes());
    write_copied_usage_rows_with_deletes(
        transaction,
        file_id,
        changes,
        usage_by_id,
        measurements,
        layout,
        usage_deletes,
    )?;
    measurements.record("usage", started, changes.usage.len());
    record_writer_rows(transaction, measurements, "usage_written_rows", rows_before);
    #[cfg(test)]
    configured::effect("usage")?;

    let started = measurements.start();
    let rows_before = started.map(|_| transaction.total_changes());
    let mut put = transaction.prepare_cached(COMPOSITE_PUT_CODEX_USAGE)?;
    row_plan.codex_usage.apply(
        |seq, usage| {
            let CodexUsageEvent {
                model,
                tokens:
                    ModelTokens {
                        input,
                        output,
                        cache_write,
                        cache_read,
                    },
                timestamp,
            } = usage;
            put.execute(params![
                file_id,
                seq_int(seq),
                model,
                int(*input),
                int(*output),
                int(*cache_write),
                int(*cache_read),
                timestamp
            ])
            .map(drop)
        },
        |from| {
            transaction
                .execute(
                    "DELETE FROM codex_usage WHERE namespace=semon_namespace() AND file_id=?1 AND seq>=?2",
                    params![file_id, seq_int(from)],
                )
                .map(drop)
        },
    )?;
    measurements.record("codex_usage", started, 1);
    record_writer_rows(
        transaction,
        measurements,
        "codex_usage_written_rows",
        rows_before,
    );
    #[cfg(test)]
    configured::effect("codex_usage")?;
    measurements.count("codex_usage_records", codex_usage_events.len());
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct UsageDeleteWork {
    identity_prepares: usize,
    identity_executions: usize,
    usage_prepares: usize,
    usage_executions: usize,
}

#[derive(Default)]
struct UsageDeletes<'tx> {
    identity: Option<(&'static str, Statement<'tx>)>,
    usage: Option<(&'static str, Statement<'tx>)>,
}

// Statements belong to the caller's transaction. Keep every
// original family qualification, including on reuse; no layout result or
// statement escapes to a later transaction. Lazy preparation preserves empty
// and insert-only behavior, and DML/trigger order is identical to rowwise DML.
#[cfg(test)]
fn write_copied_usage_rows(
    transaction: &Transaction<'_>,
    file_id: i64,
    changes: &super::DirtyRows,
    usage_by_id: &BTreeMap<String, MessageUsage>,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<UsageDeleteWork> {
    let layout = WriterLayout::copied_usage_only(transaction)?;
    write_copied_usage_rows_with_deletes(
        transaction,
        file_id,
        changes,
        usage_by_id,
        measurements,
        &layout,
        &mut UsageDeletes::default(),
    )
}

fn write_copied_usage_rows_with_deletes<'tx>(
    transaction: &'tx Transaction<'_>,
    file_id: i64,
    changes: &super::DirtyRows,
    usage_by_id: &BTreeMap<String, MessageUsage>,
    measurements: &mut crate::native_timing::Measurements,
    layout: &WriterLayout<'tx, '_>,
    deletes: &mut UsageDeletes<'tx>,
) -> rusqlite::Result<UsageDeleteWork> {
    layout.ensure_current()?;
    let rows = super::native_rows::UsageChanges::new(changes, usage_by_id);
    let mut work = UsageDeleteWork::default();
    let mut put = transaction.prepare_cached(COMPOSITE_PUT_USAGE)?;
    let mut put_identity = transaction.prepare_cached(
        "INSERT INTO usage_identity(namespace,file_id,message_id,record_uuid) VALUES (semon_namespace(),?1,?2,?3)",
    )?;
    for (id, usage) in rows.iter() {
        let started = measurements.start();
        let selected = "DELETE FROM usage_identity WHERE namespace=semon_namespace() AND file_id=?1 AND message_id=?2";
        measurements.record("usage_identity_delete_qualification", started, 1);
        if deletes
            .identity
            .as_ref()
            .is_some_and(|(sql, _)| *sql != selected)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if deletes.identity.is_none() {
            let started = measurements.start();
            deletes.identity = Some((selected, transaction.prepare(selected)?));
            work.identity_prepares += 1;
            measurements.record("usage_identity_delete_prepare", started, 1);
        }
        let (_, statement) = deletes
            .identity
            .as_mut()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let started = measurements.start();
        statement.execute(params![file_id, id])?;
        work.identity_executions += 1;
        measurements.record("usage_identity_delete_execute", started, 1);
        if let Some(usage) = usage {
            put_usage(&mut put, file_id, id, usage)?;
            for uuid in &usage.record_ids {
                put_identity.execute(params![file_id, id, uuid])?;
            }
        } else {
            let started = measurements.start();
            let selected = "DELETE FROM usage WHERE namespace=semon_namespace() AND file_id=?1 AND message_id=?2";
            measurements.record("usage_delete_qualification", started, 1);
            if deletes
                .usage
                .as_ref()
                .is_some_and(|(sql, _)| *sql != selected)
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            if deletes.usage.is_none() {
                let started = measurements.start();
                deletes.usage = Some((selected, transaction.prepare(selected)?));
                work.usage_prepares += 1;
                measurements.record("usage_delete_prepare", started, 1);
            }
            let (_, statement) = deletes
                .usage
                .as_mut()
                .ok_or(rusqlite::Error::InvalidQuery)?;
            let started = measurements.start();
            statement.execute(params![file_id, id])?;
            work.usage_executions += 1;
            measurements.record("usage_delete_execute", started, 1);
        }
    }
    // Reference prepares are the exact authored rowwise implementation, not
    // SQLite's internal automatic reprepare count or a second execution.
    measurements.count(
        "usage_delete_reference_prepares",
        work.identity_executions + work.usage_executions,
    );
    measurements.count(
        "usage_delete_authored_prepares",
        work.identity_prepares + work.usage_prepares,
    );
    Ok(work)
}

fn put_event(
    statement: &mut Statement<'_>,
    file_id: i64,
    extra: bool,
    seq: usize,
    event: &Event,
) -> rusqlite::Result<()> {
    let row = super::native_rows::EventRow::encode(event)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    statement.execute(params![
        file_id,
        extra,
        seq_int(seq),
        row.k,
        row.o,
        row.b,
        row.t,
        row.id,
        row.n,
        row.code_mode,
        row.code_mode_open,
        row.parent,
        row.script,
        row.y,
        row.poll,
        row.reply,
        row.reply_o,
        row.reply_b,
        row.reply_t,
        row.reply_e,
        row.reply_f,
        row.reply_m,
        row.item,
        row.has_peer,
        row.peer_sock,
        row.peer_pid,
        row.peer_start,
        row.u,
    ])?;
    Ok(())
}

fn put_usage(
    statement: &mut Statement<'_>,
    file_id: i64,
    message_id: &str,
    usage: &MessageUsage,
) -> rusqlite::Result<()> {
    let row = super::native_rows::UsageRow::encode(usage)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    statement.execute(params![
        file_id,
        message_id,
        row.model,
        row.input,
        row.cached_input,
        row.output,
        row.reasoning_output,
        row.total,
        row.model_input,
        row.model_output,
        row.model_cache_write,
        row.model_cache_read,
        row.billing_input,
        row.billing_output,
        row.billing_cache_read,
        row.cache_write_5m,
        row.cache_write_1h,
        row.web_search_requests,
        row.speed,
        row.service_tier,
        row.prompt_size,
        row.timestamp,
        row.split_unknown,
        row.record_ids,
    ])?;
    Ok(())
}

pub(crate) fn put_run(
    statement: &mut Statement<'_>,
    run: &ReportedRunSnapshot,
) -> rusqlite::Result<()> {
    let row = super::reported_rows::ReportedRow::new(run)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let ReportedRunSnapshot {
        last_session_id,
        last_start_time,
        last_cost,
        last_duration,
        last_api_duration,
        last_tool_duration,
        last_lines_added,
        last_lines_removed,
        last_model_usage: _,
        capture_at,
    } = run;
    statement.execute(params![
        last_session_id,
        last_start_time,
        last_cost,
        last_duration.map(int),
        last_api_duration.map(int),
        last_tool_duration.map(int),
        last_lines_added.map(int),
        last_lines_removed.map(int),
        row.model_usage,
        capture_at,
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::events::{
        ASYNC, Kind, Links, PIN, RateLimitWindow, RateLimits, SENDS, STDIN, SignalKind, UNKNOWN,
    };
    use crate::storage::source_observations::{
        SourceIntent, SourceMutationReceipt, SourceObservationStore, SourceOperation,
    };

    // Exercise the admitted SourceJournal for this store's database and the
    // fixture namespace; the unbound connection is never an observation port.
    trait TestSourceObservations {
        fn discover_recovery_sources(&mut self) -> Result<bool, StoreError>;
        fn discover_catalog_sources(
            &mut self,
            paths: &[String],
            complete: bool,
        ) -> Result<(), StoreError>;
    }

    fn source_journal(path: &Path) -> Result<Box<dyn SourceObservationStore>, StoreError> {
        let epoch: String = Connection::open(path)
            .and_then(|db| {
                db.query_row(
                    "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
                    [],
                    |row| row.get(0),
                )
            })
            .map_err(|error| StoreError::Failed(error.to_string()))?;
        let config = crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Sqlite {
                database: path.to_owned(),
            },
            namespace: fixture_namespace(),
            expected_stream_epoch: Some(epoch),
        };
        config.admit_namespace().map_err(StoreError::Configured)?;
        crate::storage::sqlite::source_journal::open(&config).map_err(StoreError::Configured)
    }

    impl TestSourceObservations for SqliteStore {
        fn discover_recovery_sources(&mut self) -> Result<bool, StoreError> {
            let _probe = crate::native_timing::DiscoveryProbe::new(&self.path, "recovery");
            let mut journal = source_journal(&self.path)?;
            if journal
                .recovery_is_current()
                .map_err(StoreError::Configured)?
            {
                return Ok(true);
            }
            let cursor = journal.recovery_cursor().map_err(StoreError::Configured)?;
            let operation = SourceOperation::capture(
                fixture_namespace().as_str(),
                "recover",
                &serde_json::to_vec(&cursor).unwrap(),
            );
            journal
                .persist_intent(&SourceIntent::Recover {
                    operation: operation.clone(),
                    cursor: cursor.clone(),
                })
                .map_err(StoreError::Configured)?;
            journal
                .recover(&operation, cursor.as_deref())
                .map(|(_, done)| done)
                .map_err(StoreError::Configured)
        }

        fn discover_catalog_sources(
            &mut self,
            paths: &[String],
            complete: bool,
        ) -> Result<(), StoreError> {
            let _probe = crate::native_timing::DiscoveryProbe::new(&self.path, "publication");
            let mut journal = source_journal(&self.path)?;
            if journal
                .discovery_is_current(paths, complete)
                .map_err(StoreError::Configured)?
            {
                return Ok(());
            }
            let payload = serde_json::to_vec(&(paths, complete)).unwrap();
            let operation =
                SourceOperation::capture(fixture_namespace().as_str(), "discover", &payload);
            match journal
                .discover(&operation, paths, complete)
                .map_err(StoreError::Configured)?
            {
                SourceMutationReceipt::Applied | SourceMutationReceipt::AlreadyApplied => Ok(()),
                SourceMutationReceipt::Conflict => {
                    Err(StoreError::Configured(StorageError::Conflict))
                }
            }
        }
    }

    include!("store/history_custody_tests.rs");

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-store-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn initial_index_batch_rolls_back_evidence_and_revisions_and_rechecks_each_generation() {
        let root = scratch("initial-batch");
        let (mut store, _) = opened(&root.join("index.sqlite3"));
        let files = ["first", "second"]
            .into_iter()
            .map(|name| {
                let path = root.join(name);
                fs::write(&path, b"").unwrap();
                let ledger = Ledger {
                    stat: Stat::of(&path).unwrap(),
                    offset: 0,
                    prefix: sha2::Sha256::digest(b"").into(),
                    tail: sha2::Sha256::digest(b"").into(),
                };
                (
                    path.to_string_lossy().into_owned(),
                    ledger,
                    std::sync::Arc::new(FileIndex::default()),
                )
            })
            .collect::<Vec<_>>();
        store.connection.execute_batch("CREATE TEMP TRIGGER interrupt_initial BEFORE INSERT ON files WHEN NEW.path LIKE '%/second' BEGIN SELECT RAISE(ABORT,'interrupted batch'); END;").unwrap();
        assert!(store.commit_initial_files(&files).is_err());
        for table in [
            "files",
            "session_catalog_invalidations",
            "catalog_native_dependencies",
        ] {
            assert_eq!(
                store
                    .connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0,
                "{table} escaped rollback"
            );
        }
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM meta WHERE key='catalog_source_revision'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        store
            .connection
            .execute_batch("DROP TRIGGER interrupt_initial")
            .unwrap();
        fs::write(&files[1].0, b"changed after parsing").unwrap();
        assert_eq!(
            store.commit_initial_files(&files).unwrap(),
            Outcome::Conflict
        );
        assert!(
            store.ledger(&files[0].0).unwrap().is_none(),
            "one source committed before the other generation was fenced"
        );
        fs::write(&files[1].0, b"").unwrap();
        let mut current = files.clone();
        current[1].1.stat = Stat::of(Path::new(&current[1].0)).unwrap();
        assert_eq!(
            store.commit_initial_files(&current).unwrap(),
            Outcome::Written
        );
        assert_eq!(
            store.commit_initial_files(&current).unwrap(),
            Outcome::Conflict,
            "replay overwrote already committed generations"
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM session_catalog_invalidations",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copied_usage_layout_is_warmed_by_actual_producer_and_selected_factories() {
        let root = scratch("copied-layout-factories");
        let path = root.join("index.sqlite3");
        for selected in [false, true] {
            let mut store = current_test_store(&path, &fixture_namespace());
            if selected {
                let binding = fixture_reader_binding(&path).unwrap();
                drop(focused_connection(&binding).unwrap());
            }
            let cookie: i64 = store
                .connection
                .pragma_query_value(None, "schema_version", |r| r.get(0))
                .unwrap();
            assert_eq!(
                store
                    .connection
                    .query_row("SELECT semon_copied_usage_layout(?1)", [cookie], |r| r
                        .get::<_, Option<
                        bool,
                    >>(
                        0
                    ))
                    .unwrap(),
                Some(true)
            );
            let tx = store.connection.transaction().unwrap();
            tx.execute_batch("DROP INDEX usage_identity_peers;")
                .unwrap();
            assert!(validate_copied_usage(&tx).is_err());
            tx.rollback().unwrap();
            assert_eq!(
                store
                    .connection
                    .query_row("SELECT semon_copied_usage_layout(?1)", [cookie], |r| r
                        .get::<_, Option<
                        bool,
                    >>(
                        0
                    ))
                    .unwrap(),
                Some(true)
            );
            validate_copied_usage(&store.connection).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_statements_recheck_external_schema_and_parser_changes() {
        for schema in [false, true] {
            let root = scratch("cached-authority");
            let path = root.join("index.sqlite3");
            let (mut store, _) = opened(&path);
            let original = ledger(1);
            assert_eq!(
                store
                    .write_one("a.jsonl", None, None, &original, &full())
                    .unwrap(),
                Outcome::Written
            );
            // Warm the actual connection's version/ledger statements before a
            // separate owner changes authority. SQL reuse must not reuse results.
            assert!(current(&store.connection).unwrap());
            assert_eq!(store.ledger("a.jsonl").unwrap(), Some(original.clone()));
            let external = Connection::open(&path).unwrap();
            if schema {
                external
                    .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                    .unwrap();
            } else {
                external
                    .execute(
                        "UPDATE meta SET value=?1 WHERE key='cache_version'",
                        [(CACHE_VERSION + 1).to_string()],
                    )
                    .unwrap();
            }
            assert_eq!(
                store
                    .write_one("a.jsonl", Some(&original), None, &ledger(2), &full())
                    .unwrap(),
                Outcome::Stale
            );
            assert_eq!(
                store
                    .remove(&[("a.jsonl".into(), Some(original.clone()))])
                    .unwrap(),
                Outcome::Stale
            );
            assert_eq!(store.ledger("a.jsonl").unwrap(), Some(original));
            drop(external);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn selected_role_proofs_fence_ledger_sidecar_and_parser_without_event_loads() {
        let root = scratch("selected-role-proof");
        let (mut store, _) = opened(&root.join("index.sqlite3"));
        let path = root.join("subagents/agent-proof.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        let sidecar_path = path.with_extension("meta.json");
        fs::write(
            &sidecar_path,
            r#"{"description":"original","toolUseId":"launch"}"#,
        )
        .unwrap();
        let sidecar = crate::native_sidecar::ClaudeSidecar::observe(&path, None)
            .unwrap()
            .unwrap();
        let original = ledger(1);
        let path_text = path.to_str().unwrap();
        store
            .write_one(
                path_text,
                None,
                None,
                &original,
                &FileIndex {
                    claude_sidecar: Some(sidecar),
                    ..FileIndex::default()
                },
            )
            .unwrap();
        let mut source = original.source(&path, "proof");
        source.native_role = Some(serde_json::json!({"Agent":{
            "parent":"parent", "tool_use_id":"launch", "label":"original",
            "model":null, "cwd":null, "branch":null
        }}));
        let reads = crate::events::FILE_READS.with(std::cell::Cell::get);
        let qualified = |source: &crate::model::summary::CatalogSource| {
            selected_roles_current(&store.connection, std::slice::from_ref(source)).unwrap()
        };
        assert!(qualified(&source));
        for field in 0..8 {
            let mut changed = source.clone();
            match field {
                0 => changed.dev ^= 1,
                1 => changed.ino ^= 1,
                2 => changed.size += 1,
                3 => changed.modified_ns ^= 1,
                4 => changed.changed_ns = None,
                5 => changed.offset += 1,
                6 => changed.prefix_sha256[0] ^= 1,
                _ => changed.tail_sha256[0] ^= 1,
            }
            assert!(
                !qualified(&changed),
                "ledger field {field} must remain bound"
            );
        }
        let mut changed = source.clone();
        changed.native_role.as_mut().unwrap()["Agent"]["label"] = serde_json::json!("different");
        assert!(!qualified(&changed));
        changed.native_role = None;
        assert!(!qualified(&changed));
        fs::write(&sidecar_path, r#"{"description":"changed independently"}"#).unwrap();
        assert!(!qualified(&source));
        fs::remove_file(&sidecar_path).unwrap();
        assert!(
            !qualified(&source),
            "present sidecar cannot become absent silently"
        );
        let mut top = original.source(&path.with_file_name("top.jsonl"), "top");
        store
            .write_one(
                top.path.to_str().unwrap(),
                None,
                None,
                &original,
                &FileIndex::default(),
            )
            .unwrap();
        top.native_role = Some(serde_json::json!({"Top":{"slug":"project"}}));
        assert!(selected_roles_current(&store.connection, &[top.clone()]).unwrap());
        store
            .connection
            .execute(
                "UPDATE meta SET value=?1 WHERE key='cache_version'",
                [(CACHE_VERSION + 1).to_string()],
            )
            .unwrap();
        assert!(!selected_roles_current(&store.connection, &[top]).unwrap());
        assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), reads);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scoped_store_restores_only_named_reported_runs_and_preserves_overwritten_history() {
        let root = scratch("scoped-runs");
        let path = root.join("index.sqlite3");
        let source = crate::Options::default().source;
        let (mut store, _, _) = open_scoped(&path, &source).unwrap();
        let config = source.config(&path).unwrap();
        let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap();
        let mut old = run();
        old.last_session_id = "selected".into();
        let mut newer = old.clone();
        newer.last_start_time += 1;
        newer.last_cost = Some(2.5);
        store
            .save_runs(&[old.clone(), newer.clone()], &stamp())
            .unwrap();
        {
            let mut db = configured::connection(&config).unwrap();
            let transaction = db.transaction().unwrap();
            capability.journal().validate(&transaction).unwrap();
            let mut statement = transaction.prepare(PUT_RUN_SCOPED).unwrap();
            for n in 0..20000 {
                let mut unrelated = run();
                unrelated.last_session_id = format!("unrelated-{n}");
                put_run(&mut statement, &unrelated).unwrap();
            }
            drop(statement);
            transaction.commit().unwrap();
        }
        drop(store);
        let (mut store, loaded, _) = open_scoped(&path, &source).unwrap();
        assert!(loaded.files.is_empty());
        assert!(loaded.reported_runs.is_empty());
        assert_eq!(loaded.stamp, Some(stamp()));
        let selected = std::collections::BTreeSet::from(["selected".to_owned()]);
        let restored = store.select_reported_runs(&selected).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0], old);
        assert_eq!(restored[1], newer);
        let mut last = newer.clone();
        last.last_start_time += 1;
        let saved = store.save_runs(&[last], &stamp()).unwrap().unwrap();
        assert_eq!(saved.len(), 3);
        assert!(saved.iter().all(|run| run.last_session_id == "selected"));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn opened(path: &Path) -> (SqliteStore, Loaded) {
        let store = current_test_store(path, &fixture_namespace());
        let loaded = store.load().unwrap();
        (store, loaded)
    }

    #[test]
    fn cold_discovery_committed_witness_skips_repeated_work_and_recovers_changes_and_rollback_aba()
    {
        let root = scratch("cold-discovery-witness");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        let _recording = crate::native_timing::catalog_commit_tests::recording(true);
        crate::native_timing::take_discovery_records();
        let inputs = vec!["/new-source.jsonl".to_owned()];
        assert!(store.discover_recovery_sources().unwrap());
        store.discover_catalog_sources(&inputs, false).unwrap();
        crate::native_timing::take_discovery_records();
        let changes = store.connection.total_changes();
        assert!(store.discover_recovery_sources().unwrap());
        store.discover_catalog_sources(&inputs, false).unwrap();
        let records = crate::native_timing::take_discovery_records();
        assert_eq!(records.len(), 2);
        for (_, counts) in records {
            assert_eq!(counts.skipped, 1);
            assert_eq!(
                counts.statements, 7,
                "only the transaction and committed witness probes may execute"
            );
            assert_eq!(counts.rows, 0);
            assert_eq!(counts.factory_opens, 0);
            assert_eq!(counts.committed_changes, 0);
        }
        assert_eq!(store.connection.total_changes(), changes);

        // A new durable recovery intent committed by a peer must invalidate the
        // previous complete pass before the next discovery publication.
        let peer = Connection::open(&path).unwrap();
        peer.execute_batch("BEGIN IMMEDIATE; INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES('recover','claude','recover','/recovered.jsonl'); INSERT INTO meta(key,value) VALUES('catalog_recovery_after',''); COMMIT;").unwrap();
        assert!(!store.discover_recovery_sources().unwrap());
        let records = crate::native_timing::take_discovery_records();
        assert_eq!(records.last().unwrap().1.skipped, 0);
        assert_eq!(records.last().unwrap().1.rows, 1);
        assert_eq!(store.connection.query_row("SELECT count(*) FROM catalog_backfill_sources WHERE source_path='/recovered.jsonl'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(store.connection.query_row("SELECT count(*) FROM session_catalog_invalidations WHERE source_path='/recovered.jsonl' AND reason='changed'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert!(store.discover_recovery_sources().unwrap());
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT count(*) FROM meta WHERE key='catalog_recovery_after'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );

        // A recovery marker introduced by discovery itself must not be hidden
        // when a completed witness is advanced across known owner writes.
        store.connection.execute_batch("CREATE TEMP TRIGGER request_recovery_during_discovery AFTER INSERT ON catalog_backfill_sources WHEN NEW.source_path='/trigger-intent.jsonl' BEGIN INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_recovery_after',''); END;").unwrap();
        assert!(store.discover_recovery_sources().unwrap());
        store
            .discover_catalog_sources(&["/trigger-intent.jsonl".into()], false)
            .unwrap();
        crate::native_timing::take_discovery_records();
        assert!(!store.discover_recovery_sources().unwrap());
        assert_eq!(
            crate::native_timing::take_discovery_records()
                .last()
                .unwrap()
                .1
                .skipped,
            0
        );
        assert!(store.discover_recovery_sources().unwrap());
        store
            .connection
            .execute_batch("DROP TRIGGER request_recovery_during_discovery;")
            .unwrap();

        // The schema cookie returns to A, but the rollback clock still changes.
        let schema: i64 = store
            .connection
            .pragma_query_value(None, "schema_version", |r| r.get(0))
            .unwrap();
        store
            .connection
            .execute_batch(
                "BEGIN IMMEDIATE; CREATE TABLE transient_discovery(value TEXT); ROLLBACK;",
            )
            .unwrap();
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "schema_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            schema
        );
        crate::native_timing::take_discovery_records();
        assert!(store.discover_recovery_sources().unwrap());
        assert_eq!(
            crate::native_timing::take_discovery_records()
                .last()
                .unwrap()
                .1
                .skipped,
            0
        );

        // A failed publication rolls back both membership and claims. It cannot
        // arm a successful no-op witness or advance the recovery witness.
        let next = vec!["/fault-source.jsonl".to_owned()];
        store.connection.execute_batch("CREATE TEMP TRIGGER fail_cold_discovery BEFORE INSERT ON session_catalog_invalidations WHEN NEW.source_path='/fault-source.jsonl' BEGIN SELECT RAISE(ABORT,'cold discovery fault'); END;").unwrap();
        assert!(store.discover_catalog_sources(&next, false).is_err());
        assert_eq!(store.connection.query_row("SELECT count(*) FROM catalog_backfill_sources WHERE source_path='/fault-source.jsonl'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        store
            .connection
            .execute_batch("DROP TRIGGER fail_cold_discovery;")
            .unwrap();
        crate::native_timing::take_discovery_records();
        store.discover_catalog_sources(&next, false).unwrap();
        let records = crate::native_timing::take_discovery_records();
        assert_eq!(records.last().unwrap().1.skipped, 0);
        assert!(records.last().unwrap().1.committed_changes > 0);
        assert_eq!(store.connection.query_row("SELECT count(*) FROM session_catalog_invalidations WHERE source_path='/fault-source.jsonl'",[],|r|r.get::<_,i64>(0)).unwrap(),1);

        // Local committed DML and peer commits also invalidate publication.
        store
            .connection
            .execute(
                "INSERT INTO meta(key,value) VALUES('discovery-local-change','1')",
                [],
            )
            .unwrap();
        crate::native_timing::take_discovery_records();
        store.discover_catalog_sources(&next, false).unwrap();
        assert_eq!(
            crate::native_timing::take_discovery_records()
                .last()
                .unwrap()
                .1
                .skipped,
            0
        );
        peer.execute(
            "INSERT INTO meta(key,value) VALUES('discovery-peer-change','1')",
            [],
        )
        .unwrap();
        store.discover_catalog_sources(&next, false).unwrap();
        assert_eq!(
            crate::native_timing::take_discovery_records()
                .last()
                .unwrap()
                .1
                .skipped,
            0
        );

        // A trigger cannot smuggle changed contract metadata into a newly
        // armed witness. The whole discovery transaction must roll back.
        store.connection.execute_batch("CREATE TEMP TRIGGER corrupt_discovery_contract AFTER INSERT ON catalog_backfill_sources WHEN NEW.source_path='/corrupt-contract.jsonl' BEGIN UPDATE meta SET value='unsupported' WHERE key='cache_version'; END;").unwrap();
        assert!(store.discover_recovery_sources().unwrap());
        assert!(
            store
                .discover_catalog_sources(&["/corrupt-contract.jsonl".into()], false)
                .is_err()
        );
        assert_eq!(
            parser_version(&store.connection).unwrap(),
            Some(CACHE_VERSION)
        );
        assert_eq!(store.connection.query_row("SELECT count(*) FROM catalog_backfill_sources WHERE source_path='/corrupt-contract.jsonl'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(store.connection.query_row("SELECT count(*) FROM session_catalog_invalidations WHERE source_path='/corrupt-contract.jsonl'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        store
            .connection
            .execute_batch("DROP TRIGGER corrupt_discovery_contract;")
            .unwrap();

        // Transient DDL never arms a witness, including a cookie ABA after its
        // rollback; an unsupported committed incarnation cannot use the cache.
        store
            .connection
            .execute_batch("BEGIN IMMEDIATE; ALTER TABLE meta ADD COLUMN transient TEXT;")
            .unwrap();
        assert!(store.discover_catalog_sources(&next, false).is_err());
        store.connection.execute_batch("ROLLBACK;").unwrap();
        crate::native_timing::take_discovery_records();
        store.discover_catalog_sources(&next, false).unwrap();
        assert_eq!(
            crate::native_timing::take_discovery_records()
                .last()
                .unwrap()
                .1
                .skipped,
            0
        );
        peer.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        let changes = store.connection.total_changes();
        assert!(store.discover_recovery_sources().is_err());
        assert!(store.discover_catalog_sources(&next, false).is_err());
        assert_eq!(store.connection.total_changes(), changes);
        drop(peer);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn event_cache_uses_owner_committed_discovery_witness() {
        let root = scratch("producer-owner-witness");
        let path = root.join("index.sqlite3");
        let (store, _) = opened(&path);
        let options = crate::Options {
            claude_home: root.join("claude"),
            cache: root.join("index.json"),
            ..crate::Options::default()
        };
        let inputs = vec![crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/owner.jsonl".into(),
        }];
        let mut cache = crate::events::EventCache {
            store: Some(Box::new(store)),
            observation_path: Some(path),
            ..crate::events::EventCache::default()
        };
        let _recording = crate::native_timing::catalog_commit_tests::recording(true);
        crate::native_timing::take_discovery_records();
        assert!(cache.discover_recovery_sources().unwrap());
        cache
            .discover_catalog_sources(&options, &inputs, false)
            .unwrap();
        crate::native_timing::take_discovery_records();
        assert!(cache.discover_recovery_sources().unwrap());
        cache
            .discover_catalog_sources(&options, &inputs, false)
            .unwrap();
        let records = crate::native_timing::take_discovery_records();
        assert_eq!(records.len(), 2);
        for (_, counts) in records {
            assert_eq!(counts.skipped, 1);
            assert_eq!(counts.statements, 7);
            assert_eq!(counts.rows, 0);
            assert_eq!(counts.factory_opens, 0);
            assert_eq!(counts.committed_changes, 0);
        }
        drop(cache);
        fs::remove_dir_all(root).unwrap();
    }

    const CATALOG_FIXTURE_NOW: i64 = 1_790_294_400_000;

    #[test]
    fn cold_discovery_cached_witness_preserves_contract_and_namespace_refusals() {
        for sql in [
            "ALTER TABLE meta ADD COLUMN unexpected TEXT;",
            "DROP TABLE meta;",
            "CREATE TABLE storage_namespaces(namespace TEXT PRIMARY KEY);",
            "UPDATE meta SET value='unsupported' WHERE key='cache_version';",
        ] {
            let root = scratch("cold-discovery-refusal");
            let (mut store, _) = opened(&root.join("index.sqlite3"));
            let inputs = vec!["/accepted.jsonl".into()];
            assert!(store.discover_recovery_sources().unwrap());
            store.discover_catalog_sources(&inputs, false).unwrap();
            store.connection.execute_batch(sql).unwrap();
            let before = store.connection.total_changes();
            assert!(store.discover_recovery_sources().is_err(), "{sql}");
            assert!(
                store
                    .discover_catalog_sources(&["/refused.jsonl".into()], false)
                    .is_err(),
                "{sql}"
            );
            assert_eq!(
                store.connection.total_changes(),
                before,
                "refusal must precede writes: {sql}"
            );
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn catalog_native_fixture(
        root: &Path,
        count: usize,
        seed_catalog: bool,
    ) -> (
        crate::Options,
        Vec<crate::model::summary::CatalogRow>,
        Vec<crate::slot_projection::Projection>,
    ) {
        let options = crate::Options {
            source: crate::Options::default().source,
            claude_home: root.join("claude"),
            claude_json: root.join("claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("seed.json"),
            all: true,
            since: Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
        };
        fs::create_dir_all(options.proc_root.join("sys/kernel")).unwrap();
        fs::write(options.proc_root.join("sys/kernel/hostname"), "fixture\n").unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        for position in 0..count {
            let (path, records) = if position % 2 == 0 {
                let id = format!("native-cl-{position:02}");
                (
                    options
                        .claude_home
                        .join(format!("projects/-work-proj/{id}.jsonl")),
                    vec![
                        serde_json::json!({"type":"user","uuid":format!("user-{id}"),"sessionId":id,"timestamp":"2026-09-24T00:00:00Z","message":{"role":"user","content":"Native publication fixture"}}),
                        serde_json::json!({"type":"assistant","uuid":format!("assistant-{id}"),"sessionId":id,"timestamp":"2026-09-24T00:01:00Z","message":{"id":format!("message-{id}"),"role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"Native answer"}],"usage":{"input_tokens":10,"output_tokens":5}}}),
                    ],
                )
            } else {
                let id = format!("native-cx-{position:02}");
                let mut payload = serde_json::json!({"id":id,"cwd":"/work/proj","originator":"codex_exec","thread_source":"user"});
                if position > 1 {
                    payload["parent_thread_id"] = serde_json::json!("native-cx-01");
                    payload["thread_source"] = serde_json::json!("subagent");
                    payload["agent_path"] =
                        serde_json::json!(format!("native-cx-01/child-{position}"));
                }
                (
                    options
                        .codex_home
                        .join(format!("sessions/2026/09/24/rollout-{id}.jsonl")),
                    vec![
                        serde_json::json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":payload}),
                        serde_json::json!({"type":"response_item","timestamp":"2026-09-24T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Native Codex fixture"}]}}),
                        serde_json::json!({"type":"response_item","timestamp":"2026-09-24T00:01:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Native Codex answer"}]}}),
                    ],
                )
            };
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                records
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n",
            )
            .unwrap();
        }
        if seed_catalog {
            let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
            let complete = (0..128).any(|_| {
                evidence
                    .refresh_catalog(
                        &mut crate::viewer::lock(&evidence.work),
                        CATALOG_FIXTURE_NOW,
                    )
                    .unwrap()
            });
            evidence.retire();
            assert!(complete, "real native fixture publication must qualify");
        }
        let (rows, projections) = catalog_fixture_projection(&options);
        assert_eq!(rows.len(), count);
        (options, rows, projections)
    }

    #[test]
    fn cold_catalog_search_survives_deferred_original_custody_publication() {
        use std::io::Write;
        let root = scratch("catalog-search-deferred-original");
        // Build the large original before its first Catalog publication. An
        // append after publication would already retain the initial revision.
        let (options, _, _) = catalog_native_fixture(&root, 2, false);
        let key = "native-cx-01";
        let relative = "sessions/2026/09/24/rollout-native-cx-01.jsonl";
        let original = options.codex_home.join(relative);
        let mut file = fs::OpenOptions::new().append(true).open(&original).unwrap();
        for index in 2..130 {
            writeln!(file, "{}", serde_json::json!({"type":"response_item","timestamp":"2026-09-24T00:01:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":format!("Original {index}")}]}})).unwrap();
        }
        drop(file);
        let initial = crate::native_evidence::NativeEvidence::new(options.clone());
        assert!((0..128).any(|_| {
            initial
                .refresh_catalog(&mut crate::viewer::lock(&initial.work), CATALOG_FIXTURE_NOW)
                .unwrap()
        }));
        drop(initial);
        let proof = crate::session_source_proof(&options, "codex", relative)
            .unwrap()
            .unwrap();
        let ready = crate::source_projection_ready(&options, "codex", relative)
            .unwrap()
            .unwrap();
        crate::retain_source_event_index(&options, &proof, &ready).unwrap();
        fs::remove_file(original).unwrap();
        let replacement = options
            .claude_home
            .join(format!("projects/-work-proj/{key}.jsonl"));
        fs::write(&replacement, serde_json::json!({"type":"user","uuid":"replacement","sessionId":key,"timestamp":"2026-09-24T00:02:00Z","message":{"role":"user","content":"Replacement"}}).to_string()+"\n").unwrap();
        let config = options.source.config(&options.cache).unwrap();
        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let original_recipes = catalog_recipes(&db, "session_slots", key);
        assert_eq!(original_recipes.len(), 130);
        assert!(catalog_recipes(&db, "session_history_slots", key).is_empty());
        db.execute_batch("UPDATE meta SET value='partial' WHERE namespace=semon_namespace() AND key='catalog_completeness'; UPDATE storage_source_journal SET backfill_state='draining' WHERE namespace=semon_namespace();").unwrap();
        let tx = db.unchecked_transaction().unwrap();
        let ordinary = options
            .claude_home
            .join("projects/-work-proj/native-cl-00.jsonl");
        invalidate_catalog_source(&tx, ordinary.to_str().unwrap(), "changed").unwrap();
        invalidate_catalog_source(&tx, replacement.to_str().unwrap(), "changed").unwrap();
        tx.commit().unwrap();
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        evidence.note_catalog_search(Some((key, Some("claude"))));
        let mut work = crate::viewer::lock(&evidence.work);
        assert!(matches!(
            evidence
                .prepare_catalog(&mut work, CATALOG_FIXTURE_NOW)
                .unwrap(),
            crate::native_evidence::CatalogPreparation::Pending
        ));
        assert_eq!(
            catalog_recipes(&db, "session_history_slots", key).len(),
            64,
            "the first qualified replacement defers after the custody page"
        );
        assert_eq!(
            catalog_recipes(&db, "session_slots", key),
            original_recipes,
            "Deferred custody preserves every Current recipe"
        );
        assert_eq!(
            db.query_row(
                "SELECT harness FROM session_catalog WHERE session_key=?1",
                [key],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "codex"
        );
        evidence
            .prepare_catalog(&mut work, CATALOG_FIXTURE_NOW)
            .unwrap();
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [ordinary.to_string_lossy().as_ref()],
                |row| row.get::<_, bool>(0)
            )
            .unwrap(),
            "ordinary publication progresses between Deferred hinted attempts"
        );
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [replacement.to_string_lossy().as_ref()],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        let published = (0..16).any(|_| {
            evidence
                .prepare_catalog(&mut work, CATALOG_FIXTURE_NOW)
                .unwrap();
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [replacement.to_string_lossy().as_ref()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
        });
        assert!(
            published,
            "the original search survives each Deferred without reposting"
        );
        assert_eq!(
            catalog_recipes(&db, "session_history_slots", key),
            original_recipes,
            "completed custody retains the exact original recipes"
        );
        let history: crate::model::summary::CatalogRow = serde_json::from_str(
            &crate::history_projection::catalog(&db, key)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(history.harness, "codex");
        assert!(crate::history_projection::qualified(&db, &history).unwrap());
        drop(work);
        drop(evidence);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_uses_indexed_pending_matches_before_branch_limits() {
        let root = scratch("catalog-search-pending-indexes");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store.connection = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(store.owner.config()).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        for index in 0..130 {
            let source = format!("/codex/native/{index:04}.jsonl");
            store.connection.execute("INSERT INTO session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES(semon_namespace(),?1,0,'codex','{}')", [format!("session-{index}")]).unwrap();
            store.connection.execute("INSERT INTO session_catalog_sources(namespace,session_key,harness,native_id,source_path) VALUES(semon_namespace(),?1,'codex','Identity',?2)", params![format!("session-{index}"), source]).unwrap();
            store
                .connection
                .execute(
                    "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
                    params![
                        format!("native_codex_header:/codex/header/{index:04}.jsonl"),
                        r#"{"header":{"id":"Identity"}}"#
                    ],
                )
                .unwrap();
        }
        let pending = [
            "/codex/native/0129.jsonl",
            "/codex/header/0129.jsonl",
            "/claude/IDENTITY.jsonl",
            "/copilot/session-state/IDENTITY/events.jsonl",
        ];
        let tx = store.connection.unchecked_transaction().unwrap();
        for source in pending {
            invalidate_catalog_source(&tx, source, "changed").unwrap();
        }
        tx.commit().unwrap();
        let ranges = [
            ("/claude/".into(), "/claude0".into()),
            ("/codex/".into(), "/codex0".into()),
            ("/copilot/".into(), "/copilot0".into()),
        ];
        let lookup = super::super::CatalogSearchLookup {
            query: "identity",
            ranges: &ranges,
        };
        let paths = source_journal::catalog_search_paths(
            &store.connection,
            store.owner.config().namespace.as_str(),
            &lookup,
            64,
        )
        .unwrap();
        assert_eq!(
            paths.into_iter().collect::<BTreeSet<_>>(),
            pending.into_iter().map(str::to_owned).collect()
        );
        let plan: Vec<String> = store
            .connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                source_journal::catalog_search_sql()
            ))
            .unwrap()
            .query_map(
                params![
                    "identity",
                    "identity.jsonl",
                    64,
                    ranges[0].0,
                    ranges[0].1,
                    ranges[1].0,
                    ranges[1].1,
                    ranges[2].0,
                    ranges[2].1,
                    store.owner.config().namespace.as_str()
                ],
                |row| row.get(3),
            )
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for index in [
            "catalog_search_native",
            "catalog_search_pending_filename",
            "catalog_search_pending_copilot",
            "catalog_search_header",
        ] {
            assert!(
                plan.iter()
                    .any(|step| step.contains("SEARCH ") && step.contains(index)),
                "{index}: {plan:?}"
            );
        }
        assert!(
            !plan.iter().any(|step| step.starts_with("SCAN s ")
                || step.starts_with("SCAN m ")
                || step.starts_with("SCAN i ")
                || step.starts_with("SCAN session_catalog_invalidations")),
            "{plan:?}"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_projection_upgrade_precedes_draining_and_preserves_revision() {
        let root = scratch("selected-upgrade-priority");
        let (options, rows, _) = catalog_native_fixture(&root, 2, true);
        let selected = &rows[1];
        let path = selected.sources[0].path.to_string_lossy().into_owned();
        let search_source = &rows[0].sources[0];
        let search_path = search_source.path.to_string_lossy().into_owned();
        let config = options.source.config(&options.cache).unwrap();
        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let mut journal = config.source_observations().unwrap();
        let paths: Vec<String> = Vec::new();
        let operation = crate::storage::source_observations::SourceOperation::capture(
            config.namespace.as_str(),
            "discover",
            &serde_json::to_vec(&(&paths, true)).unwrap(),
        );
        journal.discover(&operation, &paths, true).unwrap();
        assert_eq!(
            journal.backfill_state().unwrap().as_deref(),
            Some("draining")
        );
        db.execute_batch("INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope('catalog_completeness'),'catalog_completeness','partial');").unwrap();
        db.execute(
            "UPDATE main.session_slot_projections SET version=5 WHERE namespace=semon_namespace() AND session_key=?1",
            [&selected.key],
        )
        .unwrap();
        let tx = db.unchecked_transaction().unwrap();
        invalidate_catalog_source(&tx, "/unrelated/first.jsonl", "changed").unwrap();
        invalidate_catalog_source(&tx, &search_path, "changed").unwrap();
        invalidate_catalog_source(&tx, &path, "changed").unwrap();
        tx.commit().unwrap();
        let revision: i64 = db
            .query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [&path],
                |row| row.get(0),
            )
            .unwrap();
        assert!(crate::request_session_projection_upgrade(&options, &selected.key).unwrap());
        assert_eq!(
            journal.pending_catalog_sources(64, None).unwrap().paths,
            vec![path.clone()]
        );
        let preferred = super::super::CatalogPendingSearch {
            paths: std::slice::from_ref(&search_path),
            retry: None,
            yield_to_fifo: true,
            excluded: std::slice::from_ref(&search_path),
        };
        assert_eq!(
            journal
                .pending_catalog_sources(64, Some(&preferred))
                .unwrap()
                .tier,
            super::super::CatalogPendingTier::SelectedUpgrade,
            "search hints must defer to explicit projection upgrades"
        );
        assert_eq!(
            db.query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [&path],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            revision
        );
        drop(journal);
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        evidence.note_catalog_search(Some((&search_source.native_id, None)));
        let upgraded = (0..128).any(|_| {
            evidence
                .refresh_catalog(
                    &mut crate::viewer::lock(&evidence.work),
                    CATALOG_FIXTURE_NOW,
                )
                .unwrap();
            db.query_row(
                "SELECT version FROM session_slot_projections WHERE session_key=?1",
                [&selected.key],
                |row| row.get::<_, u32>(0),
            )
            .unwrap()
                == crate::slot_projection::VERSION
        });
        assert!(
            upgraded,
            "ordinary producer must complete the selected upgrade"
        );
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&search_path],
                |row| row.get::<_, bool>(0)
            )
            .unwrap(),
            "competing search publication must not precede the selected upgrade"
        );
        let searched = (0..128).any(|_| {
            evidence
                .refresh_catalog(
                    &mut crate::viewer::lock(&evidence.work),
                    CATALOG_FIXTURE_NOW,
                )
                .unwrap();
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&search_path],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
        });
        assert!(
            searched,
            "the deferred search must publish without another request"
        );
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path='/unrelated/first.jsonl')", [], |row| row.get::<_, bool>(0)).unwrap(), "deferred search must still precede ordinary FIFO");
        evidence.retire();
        eprintln!("slot_upgrade stage=producer selected_first=true version=6");
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_public_selected_upgrade_ignores_unrelated_archive_and_refuses_selected_marker_without_writes()
     {
        let root = scratch("selected-open-public-entry");
        let (options, rows, _) = catalog_native_fixture(&root, 2, true);
        let selected = &rows[1];
        let path = selected.sources[0].path.to_string_lossy().into_owned();
        let cache_path = super::super::EventCache::path(&options.cache);
        let config = options.source.config(&options.cache).unwrap();
        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        db.execute(
            "UPDATE session_slot_projections SET version=5 WHERE namespace=semon_namespace() AND session_key=?1",
            [&selected.key],
        )
        .unwrap();
        db.execute(
            "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
            params![
                archive_index_key("/synthetic/unrelated.jsonl"),
                r#"{"version":1}"#
            ],
        )
        .unwrap();
        db.execute(
            "DELETE FROM session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1",
            [&path],
        )
        .unwrap();
        drop(db);
        assert!(crate::request_session_projection_upgrade(&options, &selected.key).unwrap());
        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let selected_meta = crate::storage::source_observations::SELECTED_PROJECTION_KEY;
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key=?1",
                [selected_meta],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            serde_json::to_string(&vec![path.clone()]).unwrap()
        );
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&path],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
        );

        db.execute(
            "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
            params![archive_index_key(&path), r#"{"version":5}"#],
        )
        .unwrap();
        let snapshot = |connection: &Connection| {
            let mut meta_statement = connection
                .prepare("SELECT key,value FROM meta ORDER BY key")
                .unwrap();
            let meta = meta_statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let mut invalidation_statement = connection
                .prepare(
                    "SELECT source_path,revision,reason FROM session_catalog_invalidations ORDER BY source_path",
                )
                .unwrap();
            let invalidations = invalidation_statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let mut claims_statement = connection
                .prepare(
                    "SELECT source_path,claim FROM catalog_semantic_sources WHERE claim IS NOT NULL ORDER BY source_path",
                )
                .unwrap();
            let claims = claims_statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (meta, invalidations, claims)
        };
        let before = snapshot(&db);
        let mut wal_path = cache_path.as_os_str().to_owned();
        wal_path.push("-wal");
        let wal_path = PathBuf::from(wal_path);
        let read_wal = || match fs::read(&wal_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("read SQLite WAL: {error}"),
        };
        let database_before = fs::read(&cache_path).unwrap();
        let wal_before = read_wal();
        // Keep this connection alive so closing the last writer cannot
        // checkpoint the fixture between the baseline and the refusal.
        assert!(crate::request_session_projection_upgrade(&options, &selected.key).is_err());
        let wal_after = read_wal();
        assert!(
            wal_after == wal_before,
            "selected refusal changed WAL bytes"
        );
        assert!(
            fs::read(&cache_path).unwrap() == database_before,
            "selected refusal changed database bytes"
        );
        assert_eq!(snapshot(&db), before);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    fn catalog_fixture_projection(
        options: &crate::Options,
    ) -> (
        Vec<crate::model::summary::CatalogRow>,
        Vec<crate::slot_projection::Projection>,
    ) {
        let inputs = crate::inputs::inputs(options)
            .unwrap()
            .into_iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .collect::<Vec<_>>();
        let mut cache =
            crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let mut rows = Vec::new();
        let mut projections = Vec::new();
        for inputs in inputs.chunks(64) {
            let prepared =
                crate::model::prepare_sources(options, &mut cache, inputs, CATALOG_FIXTURE_NOW)
                    .unwrap();
            rows.extend(prepared.rows);
            projections.extend(prepared.transcripts);
        }
        // Exercise caller order rather than the sorted probe's key order.
        rows.reverse();
        projections.reverse();
        assert!(
            projections
                .iter()
                .all(|projection| !projection.slots.is_empty())
        );
        assert!(
            rows.iter().all(|row| row.overview.is_some()
                && row.trace.is_some()
                && !row.sources.is_empty())
        );
        drop(cache);
        (rows, projections)
    }

    fn catalog_pair(options: &crate::Options) -> (SqliteStore, SqliteStore) {
        let seed = crate::events::EventCache::path(&options.cache);
        let left = seed.with_file_name("batch.sqlite3");
        let right = seed.with_file_name("row_wise.sqlite3");
        // All fixture connections are closed, so the committed WAL has been
        // checkpointed. Copy the exact stream epoch and source evidence too.
        fs::copy(&seed, &left).unwrap();
        fs::copy(&seed, &right).unwrap();
        let open = |path: PathBuf| {
            let config = options.source.config(&path.with_extension("json")).unwrap();
            let owner = configured::NativeOwnership::restore(&config).unwrap();
            let connection = crate::storage::sqlite::semantic::Stage::open(
                crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
            )
            .unwrap()
            .into_test_connection();
            SqliteStore {
                owner,
                connection,
                path,
                selected_runs: None,
                semantic_rejection: None,
                traversal_observation: None,
            }
        };
        (open(left), open(right))
    }

    type CatalogState = BTreeMap<String, Vec<Vec<rusqlite::types::Value>>>;

    fn catalog_state(connection: &Connection) -> CatalogState {
        let mut names = connection
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .unwrap();
        let names = names
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        names
            .into_iter()
            .map(|name| {
                let mut statement = connection
                    .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
                    .unwrap();
                let columns = statement.column_count();
                let mut rows = statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|column| row.get::<_, rusqlite::types::Value>(column))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                rows.sort_by_key(|row| format!("{row:?}"));
                (name, rows)
            })
            .collect()
    }

    fn catalog_comparable_state(connection: &Connection) -> CatalogState {
        let mut state = catalog_state(connection);
        // Only the existing observation clock is nondeterministic between
        // separate successful commits. Rollback tests use the FULL snapshot.
        // Epochs, all digests, source generations and recipe bytes stay exact.
        state
            .get_mut("meta")
            .unwrap()
            .retain(|row| row[1] != rusqlite::types::Value::Text("catalog_observed_at".into()));
        state
    }

    fn catalog_write(
        store: &mut SqliteStore,
        rows: &[crate::model::summary::CatalogRow],
        projections: &[crate::slot_projection::Projection],
        scoped: bool,
        writer: CatalogWriter,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        let generation: Option<String> = store
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let claims = rows
            .iter()
            .flat_map(|row| &row.sources)
            .map(|source| source.path.to_string_lossy().into_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|path| {
                let revision = store
                    .connection
                    .query_row(
                        "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                        [&path],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .unwrap();
                (path, revision)
            })
            .collect::<Vec<_>>();
        store.publish_catalog_with_writer(
            &crate::slot_projection::Publication {
                rows,
                transcripts: Some(projections),
                observations: None,
            },
            generation.as_deref(),
            scoped.then_some(claims.as_slice()),
            false,
            None,
            writer,
        )
    }

    fn catalog_write_pair(
        batch: &mut SqliteStore,
        row_wise: &mut SqliteStore,
        rows: &[crate::model::summary::CatalogRow],
        projections: &[crate::slot_projection::Projection],
        scoped: bool,
    ) {
        // Original custody copying is a bounded durable continuation. A
        // Deferred tick does not publish rows or stamp an observation clock.
        let publish = |store: &mut SqliteStore, writer| {
            for tick in 0..128 {
                let timing = matches!(writer, CatalogWriter::RowWise);
                let recording = crate::native_timing::catalog_commit_tests::recording(timing);
                let before = crate::model::now_ms();
                let outcome = catalog_write(store, rows, projections, scoped, writer).unwrap();
                if outcome.0 == Outcome::Written {
                    crate::native_timing::catalog_commit_tests::assert_written(timing);
                    return (
                        outcome,
                        catalog_commit_success_state(
                            &store.connection,
                            before,
                            crate::model::now_ms(),
                        ),
                    );
                }
                assert_eq!(outcome.0, Outcome::Deferred);
                crate::native_timing::catalog_commit_tests::assert_uncommitted();
                drop(recording);
                assert!(tick < 127, "original custody continuation did not complete");
            }
            unreachable!()
        };
        let (left, left_state) = publish(batch, CatalogWriter::Auto);
        let (right, right_state) = publish(row_wise, CatalogWriter::RowWise);
        assert_eq!(left, right, "exact committed collection generation");
        assert_eq!(left.0, Outcome::Written);
        assert_eq!(
            left_state, right_state,
            "full MAIN/TEMP rows and schemas; only the validated observation clock is excluded"
        );
        assert_eq!(
            catalog_comparable_state(&batch.connection),
            catalog_comparable_state(&row_wise.connection)
        );
    }

    #[test]
    fn direct_catalog_grants_are_bounded_and_wal_snapshots_keep_their_generation() {
        let root = scratch("direct-catalog-grant-wal");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 1, false);
        let config = options.source.config(&options.cache).unwrap();
        let mut store = current_test_store_with_epoch(
            &crate::events::EventCache::path(&options.cache),
            &config.namespace,
            config.expected_stream_epoch.as_deref().unwrap(),
        );
        let first =
            catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto).unwrap();
        assert_eq!(first.0, Outcome::Written);
        let first_generation = first.1.unwrap();
        let original = rows[0].clone();
        let namespace = store.owner.config().namespace.as_str().to_owned();
        let admission = store.owner.admission();
        let stream = store
            .owner
            .config()
            .expected_stream_epoch
            .as_deref()
            .unwrap()
            .to_owned();
        let binding = crate::storage::QueryRuntimeBinding {
            namespace: store.owner.config().namespace.clone(),
            admission,
            stream_epoch: stream.clone(),
        };
        let snapshot = Connection::open(&store.path).unwrap();
        snapshot.execute_batch("BEGIN DEFERRED").unwrap();
        let first_marker: String = snapshot
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                [&namespace],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &snapshot, &binding, &original
            )
            .is_ok()
        );

        let mut generation = first_generation;
        for version in 1..=3 {
            rows[0].name = format!("direct grant replacement {version}");
            let published =
                catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto).unwrap();
            assert_eq!(published.0, Outcome::Written);
            generation = published.1.unwrap();
            let grants: i64 = store
                .connection
                .query_row(
                    "SELECT count(*) FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2",
                    params![namespace, rows[0].key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(grants, 1, "one current owner slot per session");
            assert!(
                crate::storage::sqlite::proof_generations::observation_binding(
                    &store.connection,
                    &binding,
                    &rows[0]
                )
                .is_ok()
            );
            assert!(
                crate::storage::sqlite::proof_generations::observation_binding(
                    &store.connection,
                    &binding,
                    &original
                )
                .is_err(),
                "a replaced version has no current or History owner"
            );
            let old_snapshot_marker: String = snapshot
                .query_row(
                    "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                    [&namespace],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(old_snapshot_marker, first_marker);
            assert!(
                crate::storage::sqlite::proof_generations::observation_binding(
                    &snapshot, &binding, &original
                )
                .is_ok(),
                "the pinned WAL snapshot retains its old owner grant"
            );
        }
        let marker_count: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                [&namespace],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker_count, 1);
        let generation_value: String = store
            .connection
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                [&namespace],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            generation_value,
            format!("{generation}:{admission}:{stream}")
        );
        let old_marker_count: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM main.meta WHERE namespace=?1 AND key LIKE 'catalog_native_publication:%'",
                [&namespace],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            old_marker_count, 0,
            "direct writes use only the stable marker"
        );
        drop(snapshot);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_catalog_reprice_preserves_exact_observation_authority() {
        let root = scratch("direct-catalog-reprice-authority");
        let (options, rows, projections) = catalog_native_fixture(&root, 1, false);
        let config = options.source.config(&options.cache).unwrap();
        let mut store = current_test_store_with_epoch(
            &crate::events::EventCache::path(&options.cache),
            &config.namespace,
            config.expected_stream_epoch.as_deref().unwrap(),
        );
        assert_eq!(
            catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto)
                .unwrap()
                .0,
            Outcome::Written
        );

        let original = rows[0].clone();
        let binding = crate::storage::QueryRuntimeBinding {
            namespace: store.owner.config().namespace.clone(),
            admission: store.owner.admission(),
            stream_epoch: store
                .owner
                .config()
                .expected_stream_epoch
                .as_deref()
                .unwrap()
                .to_owned(),
        };
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &store.connection,
                &binding,
                &original,
            )
            .is_ok(),
            "the direct Catalog publication grants its exact Current row"
        );

        let mut retained = original.clone();
        retained.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        store.connection.execute(
            "INSERT INTO main.session_history_catalog(namespace,session_key,metadata) VALUES(?1,?2,?3)",
            params![binding.namespace.as_str(), original.key, serde_json::to_string(&retained).unwrap()],
        ).unwrap();
        crate::storage::sqlite::proof_generations::reconcile_native_observation_owners(
            &store.connection,
            binding.namespace.as_str(),
            &original.key,
            binding.admission,
            &binding.stream_epoch,
        )
        .unwrap();
        let snapshot = Connection::open(&store.path).unwrap();
        snapshot.execute_batch("BEGIN DEFERRED").unwrap();
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &snapshot, &binding, &original,
            )
            .is_ok()
        );

        let mut fixture: Value =
            serde_json::from_slice(include_bytes!("../pricing/fixtures/models-dev.json")).unwrap();
        fixture["anthropic"]["models"]["claude-sonnet-4-5"]["cost"]["input"] = Value::from(300.0);
        let table = std::sync::Arc::new(
            crate::pricing::runtime::adapt(&serde_json::to_vec(&fixture).unwrap(), 1).unwrap(),
        );
        crate::events::store::reprice::drain_for_store_test(&store.owner, table.clone()).unwrap();

        let encoded: String = store
            .connection
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                rusqlite::params![binding.namespace.as_str(), original.key],
                |row| row.get(0),
            )
            .unwrap();
        let repriced: crate::model::summary::CatalogRow = serde_json::from_str(&encoded).unwrap();
        assert_eq!(repriced.cost.pricing_fingerprint, table.fingerprint);
        assert_ne!(
            repriced.cost.usd, original.cost.usd,
            "the actual pricing producer must change the retained cost"
        );
        assert_ne!(encoded, serde_json::to_string(&original).unwrap());
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &store.connection,
                &binding,
                &repriced,
            )
            .is_ok(),
            "repricing must preserve authority for the exact Current metadata it publishes"
        );

        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &store.connection,
                &binding,
                &original,
            )
            .is_err(),
            "the old Current preimage loses both mutable owner slots"
        );
        let encoded_history: String = store.connection.query_row(
            "SELECT metadata FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
            params![binding.namespace.as_str(), original.key],
            |row| row.get(0),
        ).unwrap();
        let repriced_history: crate::model::summary::CatalogRow =
            serde_json::from_str(&encoded_history).unwrap();
        assert_eq!(repriced_history.cost.pricing_fingerprint, table.fingerprint);
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &store.connection,
                &binding,
                &repriced_history,
            )
            .is_ok(),
            "same-source History gets its own exact updated grant"
        );
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &snapshot, &binding, &original,
            )
            .is_ok(),
            "a stale WAL snapshot retains the coherent old row/grant pair"
        );
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &snapshot, &binding, &repriced,
            )
            .is_err()
        );
        drop(snapshot);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_catalog_reprice_refuses_missing_or_tampered_authority() {
        for corrupt in [false, true] {
            let root = scratch("direct-catalog-reprice-refusal");
            let (options, rows, projections) = catalog_native_fixture(&root, 3, false);
            let config = options.source.config(&options.cache).unwrap();
            let mut store = current_test_store_with_epoch(
                &crate::events::EventCache::path(&options.cache),
                &config.namespace,
                config.expected_stream_epoch.as_deref().unwrap(),
            );
            assert_eq!(
                catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto)
                    .unwrap()
                    .0,
                Outcome::Written
            );
            let namespace = config.namespace.as_str();
            let before: String = store.connection.query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![namespace, rows[0].key], |r| r.get(0),
            ).unwrap();
            let publisher =
                crate::storage::sqlite::semantic::pricing_connection(&store.owner).unwrap();
            if corrupt {
                publisher.execute(
                    "UPDATE storage_catalog_native_observation_grants SET metadata_digest=?3 WHERE namespace=?1 AND session_key=?2",
                    params![namespace, rows[0].key, "0".repeat(64)],
                ).unwrap();
            } else {
                publisher.execute(
                    "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2",
                    params![namespace, rows[0].key],
                ).unwrap();
            }
            let mut fixture: Value =
                serde_json::from_slice(include_bytes!("../pricing/fixtures/models-dev.json"))
                    .unwrap();
            fixture["anthropic"]["models"]["claude-sonnet-4-5"]["cost"]["input"] =
                Value::from(300.0);
            let table = std::sync::Arc::new(
                crate::pricing::runtime::adapt(&serde_json::to_vec(&fixture).unwrap(), 1).unwrap(),
            );
            crate::events::store::reprice::drain_for_store_test(&store.owner, table.clone())
                .unwrap();
            let after: String = store.connection.query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![namespace, rows[0].key], |r| r.get(0),
            ).unwrap();
            assert_eq!(
                after, before,
                "unproved rows retain their honest original metadata"
            );
            let unrelated = rows
                .iter()
                .find(|row| row.key != rows[0].key && row.harness == "claude")
                .unwrap();
            let unrelated_fingerprint: String = publisher.query_row(
                "SELECT pricing_fingerprint FROM session_catalog WHERE namespace=?1 AND session_key=?2",
                params![namespace, unrelated.key], |r| r.get(0),
            ).unwrap();
            assert_eq!(
                unrelated_fingerprint, table.fingerprint,
                "missing or invalid evidence does not block a qualified unrelated owner"
            );
            let digest: Option<String> = publisher.query_row(
                "SELECT metadata_digest FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2",
                params![namespace, rows[0].key], |r| r.get(0),
            ).optional().unwrap();
            assert_eq!(
                digest,
                corrupt.then(|| "0".repeat(64)),
                "repricing cannot mint or repair authority"
            );
            drop(publisher);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn direct_catalog_reprice_preparation_sql_failure_rolls_back_progress() {
        use crate::storage::sqlite::proof_generations::FAIL_OBSERVATION_READ;
        for (owner_kind, denied_table) in [
            ("catalog", "session_catalog"),
            ("catalog", "storage_catalog_native_observation_grants"),
            ("history", "session_history_catalog"),
            ("history", "storage_catalog_native_observation_grants"),
        ] {
            let root = scratch("direct-catalog-reprice-preparation-failure");
            let (options, rows, projections) = catalog_native_fixture(&root, 1, false);
            let config = options.source.config(&options.cache).unwrap();
            let mut store = current_test_store_with_epoch(
                &crate::events::EventCache::path(&options.cache),
                &config.namespace,
                config.expected_stream_epoch.as_deref().unwrap(),
            );
            assert_eq!(
                catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto)
                    .unwrap()
                    .0,
                Outcome::Written
            );
            let mut retained = rows[0].clone();
            retained.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
            store.connection.execute(
                "INSERT INTO main.session_history_catalog(namespace,session_key,metadata) VALUES(?1,?2,?3)",
                params![config.namespace.as_str(), retained.key, serde_json::to_string(&retained).unwrap()],
            ).unwrap();
            crate::storage::sqlite::proof_generations::reconcile_native_observation_owners(
                &store.connection,
                config.namespace.as_str(),
                &retained.key,
                store.owner.admission(),
                config.expected_stream_epoch.as_deref().unwrap(),
            )
            .unwrap();
            let publisher =
                crate::storage::sqlite::semantic::pricing_connection(&store.owner).unwrap();
            publisher.execute(
                "INSERT OR IGNORE INTO pricing_recompute_pending(namespace,owner_kind,owner_key) VALUES(?1,'session',?2)",
                params![config.namespace.as_str(), retained.key],
            ).unwrap();
            let before = catalog_state(&store.connection);
            let mut fixture: Value =
                serde_json::from_slice(include_bytes!("../pricing/fixtures/models-dev.json"))
                    .unwrap();
            fixture["anthropic"]["models"]["claude-sonnet-4-5"]["cost"]["input"] =
                Value::from(300.0);
            let table = std::sync::Arc::new(
                crate::pricing::runtime::adapt(&serde_json::to_vec(&fixture).unwrap(), 1).unwrap(),
            );
            FAIL_OBSERVATION_READ.with(|fault| fault.set(Some((owner_kind, denied_table))));
            let error =
                crate::events::store::reprice::drain_for_store_test(&store.owner, table.clone())
                    .unwrap_err();
            assert_eq!(
                error.sqlite_error().map(|error| error.extended_code & 0xff),
                Some(rusqlite::ffi::SQLITE_AUTH)
            );
            let failed = catalog_state(&store.connection);
            for unchanged in [
                "session_catalog",
                "session_history_catalog",
                "storage_catalog_native_observation_grants",
                "pricing_recompute_pending",
            ] {
                assert_eq!(
                    failed[unchanged], before[unchanged],
                    "{owner_kind} preparation failure preserves {unchanged}"
                );
            }
            let error =
                crate::events::store::reprice::drain_for_store_test(&store.owner, table.clone())
                    .unwrap_err();
            assert_eq!(
                error.sqlite_error().map(|error| error.extended_code & 0xff),
                Some(rusqlite::ffi::SQLITE_AUTH)
            );
            assert_eq!(
                catalog_state(&store.connection),
                failed,
                "retry preserves the pricing cursor, pending owner and all observations"
            );
            FAIL_OBSERVATION_READ.with(|fault| fault.set(None));
            crate::events::store::reprice::drain_for_store_test(&store.owner, table.clone())
                .unwrap();
            let fingerprint: String = publisher.query_row(
                "SELECT pricing_fingerprint FROM session_catalog WHERE namespace=?1 AND session_key=?2",
                params![config.namespace.as_str(), retained.key], |row| row.get(0),
            ).unwrap();
            assert_eq!(
                fingerprint, table.fingerprint,
                "the failed owner remains retryable"
            );
            drop(publisher);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn direct_catalog_reprice_grant_failure_rolls_back_progress() {
        let root = scratch("direct-catalog-reprice-grant-failure");
        let (options, rows, projections) = catalog_native_fixture(&root, 1, false);
        let config = options.source.config(&options.cache).unwrap();
        let mut store = current_test_store_with_epoch(
            &crate::events::EventCache::path(&options.cache),
            &config.namespace,
            config.expected_stream_epoch.as_deref().unwrap(),
        );
        assert_eq!(
            catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto)
                .unwrap()
                .0,
            Outcome::Written
        );
        store.connection.execute_batch("CREATE TRIGGER test_reprice_grant_failure BEFORE UPDATE ON storage_catalog_native_observation_grants BEGIN SELECT RAISE(ABORT,'injected reprice grant failure'); END;").unwrap();
        let mut fixture: Value =
            serde_json::from_slice(include_bytes!("../pricing/fixtures/models-dev.json")).unwrap();
        fixture["anthropic"]["models"]["claude-sonnet-4-5"]["cost"]["input"] = Value::from(300.0);
        let table = std::sync::Arc::new(
            crate::pricing::runtime::adapt(&serde_json::to_vec(&fixture).unwrap(), 1).unwrap(),
        );
        let prior = catalog_comparable_state(&store.connection);
        assert!(
            crate::events::store::reprice::drain_for_store_test(&store.owner, table.clone())
                .is_err()
        );
        // Earlier bounded source/phase ticks may commit. Repeating the failed
        // Catalog tick must preserve all rows, grants and durable progress.
        let failed = catalog_comparable_state(&store.connection);
        assert!(
            crate::events::store::reprice::drain_for_store_test(&store.owner, table.clone())
                .is_err()
        );
        assert_eq!(catalog_comparable_state(&store.connection), failed);
        assert_ne!(failed, prior);
        store
            .connection
            .execute_batch("DROP TRIGGER test_reprice_grant_failure")
            .unwrap();
        crate::events::store::reprice::drain_for_store_test(&store.owner, table).unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_catalog_history_grants_preserve_only_existing_producer_metadata() {
        let root = scratch("direct-catalog-history-grant");
        let (options, rows, projections) = catalog_native_fixture(&root, 1, false);
        let config = options.source.config(&options.cache).unwrap();
        let mut store = current_test_store_with_epoch(
            &crate::events::EventCache::path(&options.cache),
            &config.namespace,
            config.expected_stream_epoch.as_deref().unwrap(),
        );
        assert_eq!(
            catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto)
                .unwrap()
                .0,
            Outcome::Written
        );
        let namespace = store.owner.config().namespace.as_str().to_owned();
        let admission = store.owner.admission();
        let stream = store
            .owner
            .config()
            .expected_stream_epoch
            .as_deref()
            .unwrap()
            .to_owned();
        let ordinary =
            crate::storage::sqlite::source_journal::SourceJournal::owned(store.owner.clone())
                .connection()
                .unwrap();
        assert!(ordinary
            .execute(
                "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream) VALUES(?1,?2,'history',?3,?4,?5)",
                params![namespace, rows[0].key, "a".repeat(64), i64::try_from(admission).unwrap(), stream],
            )
            .is_err());
        drop(ordinary);
        let mut retained = rows[0].clone();
        retained.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        store
            .connection
            .execute(
                "INSERT INTO main.session_history_catalog(namespace,session_key,metadata) VALUES(?1,?2,?3)",
                params![namespace, retained.key, serde_json::to_string(&retained).unwrap()],
            )
            .unwrap();
        crate::storage::sqlite::proof_generations::reconcile_native_observation_owners(
            &store.connection,
            &namespace,
            &retained.key,
            admission,
            &stream,
        )
        .unwrap();
        let binding = crate::storage::QueryRuntimeBinding {
            namespace: store.owner.config().namespace.clone(),
            admission,
            stream_epoch: stream.clone(),
        };
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &store.connection,
                &binding,
                &retained
            )
            .is_ok(),
            "the latest History row inherits an exact prior producer grant"
        );

        let mut copied = retained.clone();
        copied.name.push_str(" copied without producer evidence");
        store
            .connection
            .execute(
                "UPDATE main.session_history_catalog SET metadata=?3 WHERE namespace=?1 AND session_key=?2",
                params![namespace, copied.key, serde_json::to_string(&copied).unwrap()],
            )
            .unwrap();
        crate::storage::sqlite::proof_generations::reconcile_native_observation_owners(
            &store.connection,
            &namespace,
            &copied.key,
            admission,
            &stream,
        )
        .unwrap();
        let copied_grants: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='history'",
                params![namespace, copied.key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(copied_grants, 0);
        assert!(
            crate::storage::sqlite::proof_generations::observation_binding(
                &store.connection,
                &binding,
                &copied
            )
            .is_err(),
            "copied History metadata cannot mint local authority"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_catalog_grant_replacement_rolls_back_with_the_catalog_marker() {
        let root = scratch("direct-catalog-grant-rollback");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 1, false);
        let config = options.source.config(&options.cache).unwrap();
        let mut store = current_test_store_with_epoch(
            &crate::events::EventCache::path(&options.cache),
            &config.namespace,
            config.expected_stream_epoch.as_deref().unwrap(),
        );
        let first =
            catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto).unwrap();
        assert_eq!(first.0, Outcome::Written);
        let first_generation = first.1.unwrap();
        let namespace = store.owner.config().namespace.as_str().to_owned();
        let key = rows[0].key.clone();
        let before_row: String = store
            .connection
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![namespace, key],
                |row| row.get(0),
            )
            .unwrap();
        let before_grant: (String, i64, String) = store
            .connection
            .query_row(
                "SELECT metadata_digest,admission,stream FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                params![namespace, key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let before_marker: String = store
            .connection
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                [&namespace],
                |row| row.get(0),
            )
            .unwrap();
        store
            .connection
            .execute_batch("CREATE TRIGGER test_reject_catalog_native_marker BEFORE UPDATE ON main.meta WHEN OLD.key='catalog_native_publication' BEGIN SELECT RAISE(ABORT,'injected catalog marker failure'); END;")
            .unwrap();
        rows[0].name.push_str(" failed replacement");
        assert!(
            catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto).is_err()
        );
        let after_row: String = store
            .connection
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![namespace, key],
                |row| row.get(0),
            )
            .unwrap();
        let after_grant: (String, i64, String) = store
            .connection
            .query_row(
                "SELECT metadata_digest,admission,stream FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                params![namespace, key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let after_marker: String = store
            .connection
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                [&namespace],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after_row, before_row);
        assert_eq!(after_grant, before_grant);
        assert_eq!(after_marker, before_marker);
        assert_eq!(
            after_marker,
            format!(
                "{first_generation}:{}:{}",
                store.owner.admission(),
                before_grant.2
            )
        );
        store
            .connection
            .execute_batch("DROP TRIGGER test_reject_catalog_native_marker;")
            .unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_publish_diagnostic_reports_index_leading_the_captured_catalog_ledger() {
        use std::io::Write;

        let root = scratch("catalog-index-ledger-diagnostic");
        let (options, rows, projections) = catalog_native_fixture(&root, 1, false);
        let path = rows[0].sources[0].path.clone();
        let catalog_offset = rows[0].sources[0].offset;
        let selected_ledger = crate::source_carrier::ledger(&rows[0].sources[0]);
        crate::source_carrier::note_diagnostic_interest(
            &crate::events::EventCache::path(&options.cache),
            &path.to_string_lossy(),
            &selected_ledger,
        );
        let pause = crate::test_hooks::pause_catalog(&options.cache);
        let appended = serde_json::json!({
            "type":"assistant",
            "uuid":"assistant-diagnostic-advance",
            "sessionId":"native-cl-00",
            "timestamp":"2026-09-24T00:02:00Z",
            "message":{"id":"message-diagnostic-advance","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"synthetic index advance"}]}
        });
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(format!("{}\n", appended).as_bytes())
            .unwrap();
        crate::test_hooks::advance_index_only(&options, &path, "claude").unwrap();
        drop(pause);

        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow_mut().clear());
        let mut store = opened(&crate::events::EventCache::path(&options.cache)).0;
        let outcome =
            catalog_write(&mut store, &rows, &projections, true, CatalogWriter::Auto).unwrap();
        assert_eq!(outcome.0, Outcome::Conflict);
        let index_offset = ledger_row(&store.connection, path.to_string_lossy().as_ref())
            .unwrap()
            .unwrap()
            .1
            .offset;
        assert!(
            index_offset > catalog_offset,
            "the Index advanced beyond L1"
        );
        let logs = CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow().clone());
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("outcome=conflict reason=source_ledger"));
        assert!(logs[0].contains(&format!("selected_catalog_offset={catalog_offset}")));
        assert!(logs[0].contains(&format!("candidate_offset={catalog_offset}")));
        assert!(logs[0].contains(&format!("index_offset=Some({index_offset})")));
        assert!(!logs[0].contains(path.to_string_lossy().as_ref()));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_publish_limiter_evicts_oldest_insertion_not_smallest_hash() {
        let mut limiter = CatalogPublishLogLimiter::default();
        let now = Instant::now();
        for index in 0..256 {
            let source = format!("source-{index:03}");
            assert!(limiter.allow(
                &source,
                "source_ledger",
                "catalog_publish",
                now + Duration::from_millis(index as u64 * 1250)
            ));
        }
        let source = (
            "source-new".to_owned(),
            "source_ledger".to_owned(),
            "catalog_publish".to_owned(),
        );
        assert!(limiter.allow(
            &source.0,
            &source.1,
            &source.2,
            now + Duration::from_millis(256 * 1250)
        ));
        assert!(!limiter.last.contains_key(&(
            "source-000".to_owned(),
            "source_ledger".to_owned(),
            "catalog_publish".to_owned()
        )));
        assert!(limiter.last.contains_key(&(
            "source-255".to_owned(),
            "source_ledger".to_owned(),
            "catalog_publish".to_owned()
        )));
        assert!(limiter.last.contains_key(&source));
    }

    #[test]
    fn catalog_publish_limiter_keys_by_reason_and_stage_and_bounds_global_rate() {
        let now = Instant::now();
        let mut limiter = CatalogPublishLogLimiter::default();
        assert!(limiter.allow("source-a", "source_ledger", "catalog_publish", now));
        assert!(!limiter.allow(
            "source-a",
            "source_ledger",
            "catalog_publish",
            now + Duration::from_secs(9)
        ));
        assert!(limiter.allow(
            "source-a",
            "claim",
            "catalog_publish",
            now + Duration::from_secs(9)
        ));
        assert!(limiter.allow(
            "source-a",
            "source_ledger",
            "catalog_page",
            now + Duration::from_secs(9)
        ));

        let mut burst = CatalogPublishLogLimiter::default();
        for index in 0..CATALOG_PUBLISH_LOG_BURST {
            assert!(burst.allow(
                &format!("source-{index}"),
                "source_ledger",
                "catalog_publish",
                now
            ));
        }
        assert!(!burst.allow("source-overflow", "source_ledger", "catalog_publish", now));
        assert!(burst.allow(
            "source-refilled",
            "source_ledger",
            "catalog_publish",
            now + CATALOG_PUBLISH_LOG_REFILL
        ));
    }

    #[test]
    fn catalog_publish_diagnostic_caps_lines_per_attempt_without_logging_paths() {
        let now = Instant::now();
        let interests = (0..16)
            .map(|index| crate::source_carrier::CatalogPublishInterest {
                source_hash: format!("{index:064x}"),
                scope: "synthetic-cache".to_owned(),
                path: format!("/private/session-{index}.jsonl"),
                catalog_ledger_digest: format!("{index:064x}"),
                catalog_offset: index,
            })
            .collect::<Vec<_>>();
        let connection = Connection::open_in_memory().unwrap();
        let result = Ok(Outcome::Conflict);
        let mut limiter = CatalogPublishLogLimiter::default();
        let mut allow = |source: &str, reason: &str, stage: &str, at: Instant| {
            limiter.allow(source, reason, stage, at)
        };
        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow_mut().clear());
        let emitted = emit_catalog_publish_attempt(
            &connection,
            &interests,
            &[],
            &result,
            Some("source_identity"),
            &mut allow,
            now,
        );
        let logs = CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow().clone());
        assert_eq!(emitted, CATALOG_PUBLISH_LOGS_PER_ATTEMPT);
        assert_eq!(logs.len(), CATALOG_PUBLISH_LOGS_PER_ATTEMPT);
        assert!(
            logs.iter()
                .all(|line| line.contains("reason=source_ledger"))
        );
        assert!(logs.iter().all(|line| !line.contains("/private/session-")));
        assert!(
            logs.iter()
                .all(|line| line.contains("index_ledger_digest=missing"))
        );
        assert!(logs.iter().all(|line| line.contains("candidate=absent")));
    }

    #[test]
    fn catalog_publish_diagnostic_skips_all_post_allow_work_when_suppressed() {
        let connection = Connection::open_in_memory().unwrap();
        let interests = [crate::source_carrier::CatalogPublishInterest {
            source_hash: "a".repeat(64),
            scope: "synthetic-cache".to_owned(),
            path: "/private/gated-source.jsonl".to_owned(),
            catalog_ledger_digest: "b".repeat(64),
            catalog_offset: 42,
        }];
        let now = Instant::now();
        let result = Ok(Outcome::Written);
        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow_mut().clear());
        CATALOG_PUBLISH_DIAGNOSTIC_WORK.with(|work| work.set(0));
        let emitted = emit_catalog_publish_attempt(
            &connection,
            &interests,
            &[],
            &result,
            None,
            &mut |_, _, _, _| false,
            now,
        );
        assert_eq!(emitted, 0);
        CATALOG_PUBLISH_DIAGNOSTIC_WORK.with(|work| assert_eq!(work.get(), 0));
        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| assert!(lines.borrow().is_empty()));
        let emitted = emit_catalog_publish_attempt(
            &connection,
            &interests,
            &[],
            &result,
            None,
            &mut |_, _, _, _| true,
            now,
        );
        assert_eq!(emitted, interests.len());
        CATALOG_PUBLISH_DIAGNOSTIC_WORK.with(|work| assert_eq!(work.get(), emitted));
        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| assert_eq!(lines.borrow().len(), emitted));
        // An ordinary successful attempt without interests performs no work.
        CATALOG_PUBLISH_DIAGNOSTIC_WORK.with(|work| work.set(0));
        assert_eq!(
            emit_catalog_publish_attempt(
                &connection,
                &[],
                &[],
                &result,
                None,
                &mut |_, _, _, _| true,
                now,
            ),
            0
        );
        CATALOG_PUBLISH_DIAGNOSTIC_WORK.with(|work| assert_eq!(work.get(), 0));
    }

    #[test]
    fn header_and_publish_diagnostics_use_the_same_domain_tagged_ledger_digest() {
        let root = scratch("catalog-diagnostic-ledger-digest");
        let (options, rows, projections) = catalog_native_fixture(&root, 1, false);
        let source = &rows[0].sources[0];
        let ledger = crate::source_carrier::ledger(source);
        let digest = crate::source_carrier::ledger_digest(&ledger);
        crate::source_carrier::note_diagnostic_interest(
            &crate::events::EventCache::path(&options.cache),
            &source.path.to_string_lossy(),
            &ledger,
        );
        let diagnostic = chunk_proofs::HeaderOpenDiagnostic {
            proof_ledger_digest: Some(crate::source_carrier::ledger_digest_value(&header_ledger(
                &ledger,
            ))),
            index_ledger_digest: None,
            proof_offset: Some(ledger.offset),
            index_offset: None,
            parser_version: Some(CACHE_VERSION),
        };
        chunk_proofs::HEADER_DIAGNOSTICS.with(|lines| lines.borrow_mut().clear());
        chunk_proofs::log_header_rejection(
            "synthetic-diagnostic-source",
            crate::source_carrier::FailureStage::HeaderLedger,
            &ledger,
            &diagnostic,
            None,
            None,
        );
        let header_lines = chunk_proofs::HEADER_DIAGNOSTICS.with(|lines| lines.borrow().clone());
        assert_eq!(header_lines.len(), 1);
        assert!(header_lines[0].contains("ledger_digest_domain=ledger-v1"));
        assert!(header_lines[0].contains(&format!("catalog_ledger_digest={}", &digest[..12])));
        assert!(header_lines[0].contains(&format!("proof_ledger_digest={}", &digest[..12])));

        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow_mut().clear());
        let mut store = opened(&crate::events::EventCache::path(&options.cache)).0;
        let _ = catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto).unwrap();
        let publish_lines = CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow().clone());
        let publish_line = publish_lines.first().expect("publish diagnostic");
        assert!(publish_line.contains("ledger_digest_domain=ledger-v1"));
        assert!(
            publish_line.contains(&format!("selected_catalog_ledger_digest={}", &digest[..12]))
        );
        assert!(publish_line.contains("selected_catalog_offset="));
        assert!(publish_line.contains(&format!(
            "candidate_catalog_ledger_digest={}",
            &digest[..12]
        )));
        assert!(publish_line.contains(&format!("candidate_offset={}", ledger.offset)));
        assert!(publish_line.contains("index_ledger_digest="));
        assert!(publish_line.contains("reason=none"));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_publish_diagnostic_reports_missing_index_without_exposing_path() {
        let root = scratch("catalog-diagnostic-missing-index");
        let (options, rows, _) = catalog_native_fixture(&root, 1, false);
        let source = &rows[0].sources[0];
        let ledger = crate::source_carrier::ledger(source);
        crate::source_carrier::note_diagnostic_interest(
            &crate::events::EventCache::path(&options.cache),
            &source.path.to_string_lossy(),
            &ledger,
        );
        // A separate database intentionally lacks the Index ledger table.
        let connection = Connection::open_in_memory().unwrap();
        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow_mut().clear());
        log_catalog_publish_attempt(
            &connection,
            &crate::events::EventCache::path(&options.cache),
            &rows,
            &Ok(Outcome::Conflict),
            Some("source_identity"),
        );
        let logs = CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow().clone());
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("index_ledger_digest=missing"));
        assert!(logs[0].contains("index_offset=None"));
        assert!(!logs[0].contains(source.path.to_string_lossy().as_ref()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_publish_diagnostic_prioritizes_interested_source_past_path_limit() {
        let root = scratch("catalog-diagnostic-interest-priority");
        let (options, rows, projections) = catalog_native_fixture(&root, 65, false);
        let mut paths = rows
            .iter()
            .flat_map(|row| {
                row.sources
                    .iter()
                    .map(|source| source.path.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        assert!(paths.len() > 64);
        let target = paths.last().unwrap();
        let target_hash = crate::source_carrier::source_hash(target);
        let selected = rows
            .iter()
            .flat_map(|row| &row.sources)
            .find(|source| source.path.to_string_lossy() == target.as_str())
            .unwrap();
        let ledger = crate::source_carrier::ledger(selected);
        crate::source_carrier::note_diagnostic_interest(
            &crate::events::EventCache::path(&options.cache),
            target,
            &ledger,
        );

        CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow_mut().clear());
        let mut store = opened(&crate::events::EventCache::path(&options.cache)).0;
        let _ = catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto).unwrap();
        let logs = CATALOG_PUBLISH_DIAGNOSTICS.with(|lines| lines.borrow().clone());
        assert!(
            logs.iter()
                .any(|line| { line.contains(&format!("source_hash={}", &target_hash[..12])) })
        );
        assert!(logs.len() <= CATALOG_PUBLISH_LOGS_PER_ATTEMPT);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn catalog_commit_success_state(
        connection: &Connection,
        before_ms: i64,
        after_ms: i64,
    ) -> CatalogState {
        let mut state = crate::native_timing::cleanup_tests::snapshot(connection);
        let meta = state.get_mut("main.meta").unwrap();
        let clock = meta
            .iter()
            .filter(|row| row[1] == rusqlite::types::Value::Text("catalog_observed_at".into()))
            .collect::<Vec<_>>();
        assert_eq!(clock.len(), 1);
        let rusqlite::types::Value::Text(value) = &clock[0][2] else {
            panic!("observation clock must be decimal text")
        };
        assert!(!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()));
        let millis = value.parse::<i64>().unwrap();
        assert!(
            (before_ms..=after_ms).contains(&millis),
            "actual publication clock outside test-side bounds"
        );
        // Separate successful commits have independent wall clocks. Every
        // other row and schema remains exact; rollback never uses this filter.
        meta.retain(|row| row[1] != rusqlite::types::Value::Text("catalog_observed_at".into()));
        state
    }

    #[test]
    fn catalog_commit_boundaries_match_real_publisher_with_diagnostics_on_and_off() {
        let root = scratch("catalog-commit-boundary-parity");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 4, false);
        let (mut disabled, mut enabled) = catalog_pair(&options);
        for scoped in [false, true] {
            rows[0].name.push_str(" replacement");
            let mut outcomes = Vec::new();
            let mut states = Vec::new();
            for (store, timing) in [(&mut disabled, false), (&mut enabled, true)] {
                store.connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS boundary_temp(value TEXT); INSERT INTO boundary_temp VALUES('preserved');").unwrap();
                let recording = crate::native_timing::catalog_commit_tests::recording(timing);
                let before = crate::model::now_ms();
                outcomes.push(
                    catalog_write(store, &rows, &projections, scoped, CatalogWriter::Auto).unwrap(),
                );
                states.push(catalog_commit_success_state(
                    &store.connection,
                    before,
                    crate::model::now_ms(),
                ));
                crate::native_timing::catalog_commit_tests::assert_written(timing);
                drop(recording);
            }
            assert_eq!(outcomes[0], outcomes[1]);
            assert_eq!(outcomes[0].0, Outcome::Written);
            assert_eq!(states[0], states[1]);
        }
        drop(disabled);
        drop(enabled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_commit_boundaries_match_real_semantic_publication() {
        use crate::model::relation_stage::Phase;
        let root = scratch("catalog-commit-boundary-semantic");
        let (options, rows, _) = catalog_native_fixture(&root, 4, false);
        let (mut disabled, mut enabled) = catalog_pair(&options);
        let paths = rows
            .iter()
            .flat_map(|row| &row.sources)
            .map(|source| source.path.to_string_lossy().into_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
        let mut outcomes = Vec::new();
        let mut states = Vec::new();
        for (store, timing) in [(&mut disabled, false), (&mut enabled, true)] {
            let scoped_options = crate::Options {
                cache: store.path.with_extension("json"),
                ..options.clone()
            };
            let mut cache = crate::events::EventCache::open_scoped(
                &scoped_options.cache,
                &scoped_options.source,
            )
            .unwrap();
            let stage_before = crate::model::now_ms();
            assert_eq!(
                store.semantic_begin(&paths, &runtime).unwrap(),
                Outcome::Written
            );
            let mut written = Vec::new();
            let mut publication_states = Vec::new();
            let mut finished = false;
            for _ in 0..128 {
                let batch = store
                    .semantic_next(64)
                    .unwrap()
                    .expect("active real semantic fixture");
                if batch.paths.is_empty() {
                    if batch.phase == Phase::Publication {
                        assert_eq!(
                            store.semantic_finish(batch.epoch).unwrap(),
                            Outcome::Written
                        );
                        finished = true;
                        break;
                    }
                    continue;
                }
                let mut selected = batch.paths.iter().cloned().collect::<BTreeSet<_>>();
                if batch.phase == Phase::Relations {
                    selected.extend(batch.context.parent_paths(&batch.paths));
                    selected.extend(batch.context.companions.iter().cloned());
                }
                let inputs = selected
                    .iter()
                    .map(|path| {
                        crate::native_evidence::catalog_input(Path::new(path), &scoped_options)
                            .unwrap()
                    })
                    .collect::<Vec<_>>();
                let prepared = match batch.phase {
                    Phase::Indexing => crate::model::prepare_semantic_index_sources(
                        &scoped_options,
                        &mut cache,
                        &inputs,
                        CATALOG_FIXTURE_NOW,
                    ),
                    Phase::Identity => crate::model::prepare_semantic_identity_sources(
                        &scoped_options,
                        &mut cache,
                        &inputs,
                        &batch.context,
                        CATALOG_FIXTURE_NOW,
                    ),
                    Phase::Relations | Phase::Publication => crate::model::prepare_staged_sources(
                        &scoped_options,
                        &mut cache,
                        &inputs,
                        &batch.context,
                        batch.phase == Phase::Relations,
                        CATALOG_FIXTURE_NOW,
                    ),
                }
                .unwrap();
                if batch.phase == Phase::Publication {
                    let recording = crate::native_timing::catalog_commit_tests::recording(timing);
                    let before = crate::model::now_ms();
                    let outcome = store.publish_semantic_catalog(&batch, &prepared).unwrap();
                    assert_eq!(outcome.0, Outcome::Written);
                    written.push(outcome);
                    publication_states.push(catalog_commit_success_state(
                        &store.connection,
                        before,
                        crate::model::now_ms(),
                    ));
                    crate::native_timing::catalog_commit_tests::assert_written(timing);
                    drop(recording);
                    cache.publish_session_descriptions();
                } else {
                    assert_eq!(
                        store.semantic_record(&batch, &prepared).unwrap(),
                        Outcome::Written
                    );
                }
                cache.end_scan();
                cache.release_published_indexes();
            }
            assert!(
                finished && !written.is_empty(),
                "real stage must publish and complete"
            );
            assert_eq!(
                store
                    .connection
                    .query_row("SELECT COUNT(*) FROM catalog_semantic_stage", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            publication_states.push(catalog_commit_success_state(
                &store.connection,
                stage_before,
                crate::model::now_ms(),
            ));
            outcomes.push(written);
            states.push(publication_states);
        }
        assert_eq!(
            outcomes[0], outcomes[1],
            "exact semantic outcomes and generations"
        );
        assert_eq!(
            states[0], states[1],
            "full MAIN/TEMP semantic publication parity"
        );
        drop(disabled);
        drop(enabled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_batch_complete_publisher_matches_row_wise_rows_feed_sources_slots_and_repairs() {
        let root = scratch("catalog-publisher-parity");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 64, false);
        rows[4].repo = Some("fixture-repo".into());
        let (mut batch, mut row_wise) = catalog_pair(&options);
        let metadata = rows
            .iter()
            .map(json)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(catalog_batch_bytes(&rows, &metadata, CatalogWriter::Auto).is_some());
        catalog_write_pair(&mut batch, &mut row_wise, &rows, &projections, false);
        assert_eq!(
            batch
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM session_catalog_invalidations",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "full producer ACK"
        );
        let new_key = rows[0].key.clone();
        let parent_only = rows[1].key.clone();
        let retained = rows[2].key.clone();
        for store in [&batch, &row_wise] {
            store
                .connection
                .execute(
                    "DELETE FROM session_catalog WHERE session_key=?1",
                    [&new_key],
                )
                .unwrap();
            // Metadata unchanged, but the indexed parent must still repair.
            store
                .connection
                .execute(
                    "UPDATE session_catalog SET parent_key='wrong-parent' WHERE session_key=?1",
                    [&parent_only],
                )
                .unwrap();
            store.connection.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key=?1", [&retained]).unwrap();
            store
                .connection
                .execute(
                    "DELETE FROM session_search_documents WHERE session_key=?1",
                    [&rows[3].key],
                )
                .unwrap();
        }
        rows[4].repo = None;
        rows[4].parent = None;
        rows[5].name = "Updated native description".into();
        // Shared source paths must preserve last-writer dependency order.
        let shared_source = rows[7].sources[0].clone();
        rows[6].sources.push(shared_source);
        rows[6].query = Some(serde_json::json!({"rows":{"runtime":{"summary":{"pid":101}}}}));
        rows[7].query = Some(serde_json::json!({"rows":{"runtime":{"summary":{"pid":202}}}}));
        let shared = rows[7].sources[0].path.clone();
        let feed_before: i64 = batch
            .connection
            .query_row(
                "SELECT COALESCE(MAX(revision),0) FROM session_catalog_changes",
                [],
                |row| row.get(0),
            )
            .unwrap();
        catalog_write_pair(&mut batch, &mut row_wise, &rows, &projections, false);
        let feed = batch.connection.prepare("SELECT session_key,read_scope FROM session_catalog_changes WHERE revision>?1 ORDER BY revision").unwrap().query_map([feed_before], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        let expected = [0, 2, 4, 5, 6, 7]
            .into_iter()
            .flat_map(|position| {
                ["current", "retained_history"]
                    .map(|scope| (rows[position].key.clone(), scope.to_owned()))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            feed, expected,
            "caller order and parent-only no-feed semantics"
        );
        let pids = batch.connection.prepare("SELECT native_id FROM catalog_native_dependencies WHERE source_path=?1 AND harness='runtime:pid'").unwrap().query_map([shared.to_string_lossy()], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(
            pids,
            ["202"],
            "source dependency writes retain caller order"
        );
        {
            let transaction = batch.connection.transaction().unwrap();
            assert!(crate::catalog_search::row_present(&transaction, &rows[3].key).unwrap());
        }
        for store in [&batch, &row_wise] {
            store
                .connection
                .execute(
                    "UPDATE meta SET value=?1 WHERE key='catalog_version'",
                    [(crate::model::summary::CATALOG_VERSION - 1).to_string()],
                )
                .unwrap();
            store
                .connection
                .execute(
                    "UPDATE session_catalog_sources SET lifecycle='retained' WHERE session_key=?1",
                    [&rows[8].key],
                )
                .unwrap();
        }
        catalog_write_pair(&mut batch, &mut row_wise, &rows, &projections, false);
        assert_eq!(batch.connection.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE session_key=?1 AND lifecycle='current'", [&rows[8].key], |row| row.get::<_, i64>(0)).unwrap(), rows[8].sources.len() as i64, "unchanged metadata still repairs old-version sources");
        // A two-row scoped no-op keeps unrelated rows and exact source claims.
        let metadata = rows[10..12]
            .iter()
            .map(json)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(catalog_batch_bytes(&rows[10..12], &metadata, CatalogWriter::Auto).is_some());
        catalog_write_pair(&mut batch, &mut row_wise, &rows[10..12], &projections, true);
        drop(batch);
        drop(row_wise);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_batch_fallbacks_preserve_row_wise_acceptance_and_duplicate_rollback() {
        let root = scratch("catalog-publisher-fallback");
        let (options, rows, projections) = catalog_native_fixture(&root, 65, false);
        let (mut batch, mut row_wise) = catalog_pair(&options);
        let before = catalog_state(&batch.connection);
        for (store, writer) in [
            (&mut batch, CatalogWriter::Auto),
            (&mut row_wise, CatalogWriter::RowWise),
        ] {
            assert_eq!(
                catalog_write(store, &[], &[], false, writer).unwrap(),
                (Outcome::Conflict, None)
            );
            assert_eq!(
                catalog_state(&store.connection),
                before,
                "empty publication must not skip current source membership"
            );
        }
        for (rows, scoped) in [(&rows[..], false), (&rows[..1], true)] {
            let metadata = rows
                .iter()
                .map(json)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(catalog_batch_bytes(rows, &metadata, CatalogWriter::Auto).is_none());
            catalog_write_pair(&mut batch, &mut row_wise, rows, &projections, scoped);
        }
        let mut oversized = rows[..2].to_vec();
        oversized[1].name = "x".repeat(1_048_576);
        let metadata = oversized
            .iter()
            .map(json)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(catalog_batch_bytes(&oversized, &metadata, CatalogWriter::Auto).is_none());
        catalog_write_pair(&mut batch, &mut row_wise, &oversized, &projections, true);
        let duplicate = vec![rows[0].clone(), rows[0].clone()];
        for (store, writer) in [
            (&mut batch, CatalogWriter::Auto),
            (&mut row_wise, CatalogWriter::RowWise),
        ] {
            let before = catalog_state(&store.connection);
            let error = catalog_write(store, &duplicate, &projections, true, writer).unwrap_err();
            assert!(
                matches!(error, StoreError::Failed(_)),
                "duplicate member constraint remains an error: {error}"
            );
            assert_eq!(
                catalog_state(&store.connection),
                before,
                "duplicate rolls back the entire original loop"
            );
        }
        drop(batch);
        drop(row_wise);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_batch_parent_transaction_rolls_back_middle_dml_sources_slots_and_ack_tail() {
        let root = scratch("catalog-publisher-rollback");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 4, false);
        let (mut batch, mut row_wise) = catalog_pair(&options);
        catalog_write_pair(&mut batch, &mut row_wise, &rows, &projections, false);
        let mut stale_sources = rows.clone();
        stale_sources[1].sources[0].prefix_sha256[0] ^= 1;
        for (store, writer) in [
            (&mut batch, CatalogWriter::Auto),
            (&mut row_wise, CatalogWriter::RowWise),
        ] {
            let before = catalog_state(&store.connection);
            let raw_before = crate::native_timing::cleanup_tests::snapshot(&store.connection);
            let recording = crate::native_timing::catalog_commit_tests::recording(true);
            assert_eq!(
                catalog_write(store, &stale_sources, &projections, false, writer).unwrap(),
                (Outcome::Conflict, None)
            );
            let publication = crate::slot_projection::Publication {
                rows: &rows,
                transcripts: Some(&projections),
                observations: None,
            };
            assert_eq!(
                store
                    .publish_catalog_with_writer(
                        &publication,
                        Some("stale-generation"),
                        None,
                        false,
                        None,
                        writer
                    )
                    .unwrap(),
                (Outcome::Conflict, None)
            );
            assert_eq!(
                catalog_state(&store.connection),
                before,
                "source and generation CAS must precede reordered helpers"
            );
            assert_eq!(
                crate::native_timing::cleanup_tests::snapshot(&store.connection),
                raw_before
            );
            crate::native_timing::catalog_commit_tests::assert_uncommitted();
            drop(recording);
        }
        for row in &mut rows {
            row.name.push_str(" replacement");
            let mut overview = serde_json::to_value(row.overview.as_ref().unwrap()).unwrap();
            overview["home"]["name"] = serde_json::json!(row.name);
            row.overview = Some(serde_json::from_value(overview).unwrap());
            row.trace.as_mut().unwrap().relationships_incomplete();
        }
        for store in [&mut batch, &mut row_wise] {
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            for source in rows.iter().flat_map(|row| &row.sources) {
                invalidate_catalog_source(&tx, &source.path.to_string_lossy(), "changed").unwrap();
            }
            tx.execute(
                "UPDATE session_catalog_invalidations SET observed_at=?1",
                [CATALOG_FIXTURE_NOW],
            )
            .unwrap();
            tx.commit().unwrap();
            store
                .connection
                .execute_batch(
                    "DELETE FROM session_slots; UPDATE session_slot_projections SET version=0;",
                )
                .unwrap();
        }
        assert_eq!(
            catalog_comparable_state(&batch.connection),
            catalog_comparable_state(&row_wise.connection)
        );
        let fail_key = &rows[2].key;
        let failures = [
            format!("CREATE TEMP TRIGGER publisher_failure BEFORE UPDATE ON session_catalog WHEN NEW.session_key={} BEGIN SELECT RAISE(ABORT,'middle catalog'); END;", quoted(fail_key)),
            format!("CREATE TEMP TRIGGER publisher_failure BEFORE INSERT ON session_catalog_sources WHEN NEW.session_key={} BEGIN SELECT RAISE(ABORT,'source tail'); END;", quoted(fail_key)),
            format!("CREATE TEMP TRIGGER publisher_failure BEFORE INSERT ON session_slots WHEN NEW.session_key={} BEGIN SELECT RAISE(ABORT,'slot tail'); END;", quoted(fail_key)),
            "CREATE TEMP TRIGGER publisher_failure BEFORE DELETE ON session_catalog_invalidations BEGIN SELECT RAISE(ABORT,'ack tail'); END;".into(),
        ];
        for failure in failures {
            for (store, writer) in [
                (&mut batch, CatalogWriter::Auto),
                (&mut row_wise, CatalogWriter::RowWise),
            ] {
                let before = catalog_state(&store.connection);
                store.connection.execute_batch(&failure).unwrap();
                let raw_before = crate::native_timing::cleanup_tests::snapshot(&store.connection);
                let recording = crate::native_timing::catalog_commit_tests::recording(true);
                assert!(
                    catalog_write(store, &rows, &projections, false, writer).is_err(),
                    "injected complete publisher failure must propagate"
                );
                assert_eq!(
                    catalog_state(&store.connection),
                    before,
                    "overview/trace/feed/source/slot/generation/claims escaped rollback"
                );
                assert_eq!(
                    crate::native_timing::cleanup_tests::snapshot(&store.connection),
                    raw_before
                );
                crate::native_timing::catalog_commit_tests::assert_uncommitted();
                drop(recording);
                store
                    .connection
                    .execute_batch("DROP TRIGGER publisher_failure")
                    .unwrap();
            }
        }
        catalog_write_pair(&mut batch, &mut row_wise, &rows, &projections, false);
        assert_eq!(
            batch
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM session_catalog_invalidations",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(batch);
        drop(row_wise);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_commit_boundary_stale_claims_and_real_commit_failure_emit_no_aggregate() {
        let root = scratch("catalog-commit-boundary-failures");
        let (options, rows, projections) = catalog_native_fixture(&root, 4, false);
        let (mut disabled, mut enabled) = catalog_pair(&options);
        for (store, timing) in [(&mut disabled, false), (&mut enabled, true)] {
            let publication = crate::slot_projection::Publication {
                rows: &rows,
                transcripts: Some(&projections),
                observations: None,
            };
            let claims = rows
                .iter()
                .flat_map(|row| &row.sources)
                .map(|source| (source.path.to_string_lossy().into_owned(), Some(-1)))
                .collect::<Vec<_>>();
            let raw = crate::native_timing::cleanup_tests::snapshot(&store.connection);
            let recording = crate::native_timing::catalog_commit_tests::recording(timing);
            assert_eq!(
                store
                    .publish_catalog_with_writer(
                        &publication,
                        None,
                        Some(&claims),
                        false,
                        None,
                        CatalogWriter::Auto
                    )
                    .unwrap(),
                (Outcome::Conflict, None)
            );
            assert_eq!(
                crate::native_timing::cleanup_tests::snapshot(&store.connection),
                raw
            );
            crate::native_timing::catalog_commit_tests::assert_uncommitted();
            assert_eq!(crate::native_timing::catalog_commit_tests::counts(), (0, 0));
            drop(recording);

            store
                .connection
                .pragma_update(None, "user_version", SCHEMA_VERSION - 1)
                .unwrap();
            let raw = crate::native_timing::cleanup_tests::snapshot(&store.connection);
            let recording = crate::native_timing::catalog_commit_tests::recording(timing);
            assert_eq!(
                catalog_write(store, &rows, &projections, false, CatalogWriter::Auto).unwrap(),
                (Outcome::Stale, None)
            );
            assert_eq!(
                crate::native_timing::cleanup_tests::snapshot(&store.connection),
                raw
            );
            crate::native_timing::catalog_commit_tests::assert_uncommitted();
            assert_eq!(crate::native_timing::catalog_commit_tests::counts(), (0, 0));
            drop(recording);
            store
                .connection
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .unwrap();

            // Enable FK checks before BEGIN; the late trigger's invalid insert
            // is deferred, so all publisher DML reaches the pre-COMMIT capture.
            // Default rusqlite Transaction drop rolls back a failed COMMIT.
            store.connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE boundary_parent(id INTEGER PRIMARY KEY); CREATE TABLE boundary_child(parent INTEGER REFERENCES boundary_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TEMP TABLE boundary_temp(value TEXT); INSERT INTO boundary_temp VALUES('preserved'); CREATE TEMP TRIGGER boundary_late_dml AFTER INSERT ON main.meta WHEN NEW.key='catalog_completeness' BEGIN INSERT INTO boundary_child VALUES(1); INSERT INTO boundary_temp VALUES('late-dml'); END;").unwrap();
            assert_eq!(
                store
                    .connection
                    .pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            let raw = crate::native_timing::cleanup_tests::snapshot(&store.connection);
            let recording = crate::native_timing::catalog_commit_tests::recording(timing);
            let error =
                catalog_write(store, &rows, &projections, false, CatalogWriter::Auto).unwrap_err();
            assert!(
                error.to_string().contains("FOREIGN KEY constraint failed"),
                "expected genuine deferred COMMIT FK error: {error}"
            );
            assert_eq!(
                crate::native_timing::catalog_commit_tests::counts(),
                if timing { (2, 7) } else { (0, 0) },
                "DML completed, but successful-COMMIT capture was not reached"
            );
            assert!(store.connection.is_autocommit());
            assert_eq!(
                crate::native_timing::cleanup_tests::snapshot(&store.connection),
                raw,
                "all raw MAIN/TEMP rows and schemas restored without clock exclusions"
            );
            crate::native_timing::catalog_commit_tests::assert_uncommitted();
            drop(recording);
        }
        drop(disabled);
        drop(enabled);
        fs::remove_dir_all(root).unwrap();
    }

    fn quoted(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    #[test]
    fn catalog_batch_retains_real_original_harness_archive_custody_and_unsupported_header() {
        for unsupported in [false, true] {
            let root = scratch("catalog-batch-original-custody");
            let (options, old_rows, _) = catalog_native_fixture(&root, 4, true);
            let key = "native-cx-01";
            let old = old_rows.iter().find(|row| row.key == key).unwrap();
            let relative = "sessions/2026/09/24/rollout-native-cx-01.jsonl";
            let proof = crate::session_source_proof(&options, "codex", relative)
                .unwrap()
                .unwrap();
            let ready = crate::source_projection_ready(&options, "codex", relative)
                .unwrap()
                .unwrap();
            crate::retain_source_event_index(&options, &proof, &ready).unwrap();
            let config = options.source.config(&options.cache).unwrap();
            let original = crate::storage::sqlite::semantic::Stage::open(
                crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
            )
            .unwrap()
            .into_test_connection();
            let original_recipes = catalog_recipes(&original, "session_slots", key);
            assert!(!original_recipes.is_empty());
            if unsupported {
                original.execute("UPDATE main.session_slot_projections SET version=?1,source_generation=NULL WHERE namespace=semon_namespace() AND session_key=?2", params![crate::slot_projection::VERSION-1,key]).unwrap();
            }
            let header: (String, u32, Option<String>, i64) = original.query_row("SELECT generation,version,source_generation,total FROM session_slot_projections WHERE session_key=?1", [key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
            let archive: String = original
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [archive_index_key(old.sources[0].path.to_str().unwrap())],
                    |row| row.get(0),
                )
                .unwrap();
            drop(original);
            fs::remove_file(&old.sources[0].path).unwrap();
            let path = options
                .claude_home
                .join(format!("projects/-work-proj/{key}.jsonl"));
            fs::write(&path, serde_json::json!({"type":"user","uuid":"replacement-native","sessionId":key,"timestamp":"2026-09-24T00:02:00Z","message":{"role":"user","content":"Replacement native Claude owner"}}).to_string()+"\n").unwrap();
            let (rows, projections) = catalog_fixture_projection(&options);
            assert_eq!(
                rows.iter().find(|row| row.key == key).unwrap().harness,
                "claude",
                "shared native parser must establish the replacement identity"
            );
            let (mut batch, mut row_wise) = catalog_pair(&options);
            catalog_write_pair(&mut batch, &mut row_wise, &rows, &projections, false);
            let metadata = crate::history_projection::catalog(&batch.connection, key)
                .unwrap()
                .unwrap();
            let history: crate::model::summary::CatalogRow =
                serde_json::from_str(&metadata).unwrap();
            assert_eq!(history.harness, "codex");
            assert_eq!(history.sources, old.sources);
            assert_eq!(
                catalog_recipes(&batch.connection, "session_history_slots", key),
                original_recipes,
                "original recipe bytes and slot order survive Current replacement"
            );
            let recorded: String = batch
                .connection
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [format!("history_original_header:{key}")],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                recorded,
                json(&Some(header)).unwrap(),
                "original nullable/versioned header remains exact"
            );
            assert_eq!(
                crate::history_projection::qualified(&batch.connection, &history).unwrap(),
                !unsupported,
                "unsupported custody must not gain completion authority"
            );
            assert_eq!(
                batch
                    .connection
                    .query_row(
                        "SELECT value FROM meta WHERE key=?1",
                        [archive_index_key(old.sources[0].path.to_str().unwrap())],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                archive
            );
            let retained: (String,String) = batch.connection.query_row("SELECT lifecycle,harness FROM session_catalog_sources WHERE session_key=?1 AND source_path=?2", params![key, old.sources[0].path.to_string_lossy()], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
            assert_eq!(retained, ("retained".into(), "codex".into()));
            assert_eq!(
                batch
                    .connection
                    .query_row(
                        "SELECT harness FROM session_catalog WHERE session_key=?1",
                        [key],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                "claude"
            );
            drop(batch);
            drop(row_wise);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn catalog_original_transfer_progresses_across_restart_and_unrelated_publication() {
        use std::io::Write;
        for count in [63usize, 64, 65, 257] {
            let root = scratch("catalog-paged-original");
            let (options, _, _) = catalog_native_fixture(&root, 2, true);
            let key = "native-cx-01";
            let relative = "sessions/2026/09/24/rollout-native-cx-01.jsonl";
            let original_path = options.codex_home.join(relative);
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&original_path)
                .unwrap();
            // The fixture starts with exactly two native transcript entries.
            for n in 2..count {
                writeln!(file,"{}",serde_json::json!({"type":"response_item","timestamp":"2026-09-24T00:01:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":format!("Original native record {n}")}]}})).unwrap();
            }
            drop(file);
            let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
            assert!((0..128).any(|_| {
                evidence
                    .refresh_catalog(
                        &mut crate::viewer::lock(&evidence.work),
                        CATALOG_FIXTURE_NOW,
                    )
                    .unwrap()
            }));
            evidence.retire();
            drop(evidence);
            let proof = crate::session_source_proof(&options, "codex", relative)
                .unwrap()
                .unwrap();
            let ready = crate::source_projection_ready(&options, "codex", relative)
                .unwrap()
                .unwrap();
            crate::retain_source_event_index(&options, &proof, &ready).unwrap();
            let original =
                Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
            let expected = catalog_recipes(&original, "session_slots", key);
            assert_eq!(expected.len(), count);
            let marker: String = original
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [archive_index_key(original_path.to_str().unwrap())],
                    |r| r.get(0),
                )
                .unwrap();
            drop(original);
            fs::remove_file(&original_path).unwrap();
            let replacement = options
                .claude_home
                .join(format!("projects/-work-proj/{key}.jsonl"));
            fs::write(&replacement,serde_json::json!({"type":"user","uuid":"replacement","sessionId":key,"timestamp":"2026-09-24T00:02:00Z","message":{"role":"user","content":"Replacement Current"}}).to_string()+"\n").unwrap();
            let (rows, projections) = catalog_fixture_projection(&options);
            let (mut store, row_wise) = catalog_pair(&options);
            drop(row_wise);
            let mut steps = 0;
            loop {
                let before = catalog_recipes(&store.connection, "session_history_slots", key).len();
                let outcome =
                    catalog_write(&mut store, &rows, &projections, false, CatalogWriter::Auto)
                        .unwrap()
                        .0;
                steps += 1;
                let copied = catalog_recipes(&store.connection, "session_history_slots", key).len();
                assert!(copied - before <= 64);
                if outcome == Outcome::Written {
                    break;
                }
                assert_eq!(outcome, Outcome::Deferred);
                assert!(
                    crate::history_projection::catalog(&store.connection, key)
                        .unwrap()
                        .is_none()
                );
                assert_eq!(
                    store
                        .connection
                        .query_row(
                            "SELECT harness FROM session_catalog WHERE session_key=?1",
                            [key],
                            |r| r.get::<_, String>(0)
                        )
                        .unwrap(),
                    "codex"
                );
                // Stale publication generations cannot advance the transfer.
                let snapshot = crate::native_timing::cleanup_tests::snapshot(&store.connection);
                let outcome = store
                    .publish_catalog_with_writer(
                        &crate::slot_projection::Publication {
                            rows: &rows,
                            transcripts: Some(&projections),
                            observations: None,
                        },
                        Some("stale-publication"),
                        None,
                        false,
                        None,
                        CatalogWriter::Auto,
                    )
                    .unwrap()
                    .0;
                assert_eq!(outcome, Outcome::Conflict);
                assert_eq!(
                    crate::native_timing::cleanup_tests::snapshot(&store.connection),
                    snapshot
                );
                let mut stale_sources = rows.clone();
                stale_sources
                    .iter_mut()
                    .find(|row| row.key == key)
                    .unwrap()
                    .sources[0]
                    .offset += 1;
                assert_eq!(
                    catalog_write(
                        &mut store,
                        &stale_sources,
                        &projections,
                        false,
                        CatalogWriter::Auto
                    )
                    .unwrap()
                    .0,
                    Outcome::Conflict
                );
                assert_eq!(
                    crate::native_timing::cleanup_tests::snapshot(&store.connection),
                    snapshot,
                    "stale replacement source advanced custody"
                );
                let other = rows.iter().find(|row| row.key == "native-cl-00").unwrap();
                let other_projection = projections
                    .iter()
                    .find(|projection| projection.key == other.key)
                    .unwrap();
                assert_eq!(
                    catalog_write(
                        &mut store,
                        std::slice::from_ref(other),
                        std::slice::from_ref(other_projection),
                        true,
                        CatalogWriter::Auto
                    )
                    .unwrap()
                    .0,
                    Outcome::Written,
                    "unrelated publication was blocked by selected custody"
                );
                let path = store.path.clone();
                drop(store);
                store = current_test_store(&path, &fixture_namespace());
                assert!(steps <= count.div_ceil(64));
            }
            assert_eq!(steps, count.div_ceil(64));
            assert_eq!(
                catalog_recipes(&store.connection, "session_history_slots", key),
                expected
            );
            let history: crate::model::summary::CatalogRow = serde_json::from_str(
                &crate::history_projection::catalog(&store.connection, key)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(history.harness, "codex");
            assert!(crate::history_projection::qualified(&store.connection, &history).unwrap());
            assert_eq!(
                store
                    .connection
                    .query_row(
                        "SELECT value FROM meta WHERE key=?1",
                        [archive_index_key(original_path.to_str().unwrap())],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                marker
            );
            assert_eq!(
                store
                    .connection
                    .query_row(
                        "SELECT harness FROM session_catalog WHERE session_key=?1",
                        [key],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                "claude"
            );
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn catalog_recipes(connection: &Connection, table: &str, key: &str) -> Vec<(i64, String)> {
        connection
            .prepare(&format!(
                "SELECT slot,metadata FROM {table} WHERE session_key=?1 ORDER BY slot"
            ))
            .unwrap()
            .query_map([key], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn catalog_batch_matches_row_wise_partial_native_lineage_history_and_archive_proofs() {
        let root = scratch("catalog-batch-partial-lineage");
        let (options, _, _) = catalog_native_fixture(&root, 4, false);
        let relative = "projects/-work-proj/native-cl-02.jsonl";
        let path = options.claude_home.join(relative);
        use std::io::Write;
        writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}", serde_json::json!({"type":"user","uuid":"native-clear-marker","timestamp":"2026-09-24T00:02:00Z","sessionId":"native-cl-02","session_id":"native-cl-00","origin":{"kind":"human"},"message":{"role":"user","content":"Native clear joins original lineage"}})).unwrap();
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        let complete = (0..128).any(|_| {
            evidence
                .refresh_catalog(
                    &mut crate::viewer::lock(&evidence.work),
                    CATALOG_FIXTURE_NOW,
                )
                .unwrap()
        });
        evidence.retire();
        drop(evidence);
        assert!(complete);
        let (old_rows, _) = catalog_fixture_projection(&options);
        let old = old_rows
            .iter()
            .find(|row| row.key == "native-cl-00")
            .unwrap();
        assert_eq!(
            old.sources.len(),
            2,
            "real native clear marker must establish complete owner closure"
        );
        let proof = crate::session_source_proof(&options, "claude", relative)
            .unwrap()
            .unwrap();
        let ready = crate::source_projection_ready(&options, "claude", relative)
            .unwrap()
            .unwrap();
        crate::retain_source_event_index(&options, &proof, &ready).unwrap();
        let seed = Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
        let original_recipes = catalog_recipes(&seed, "session_slots", &old.key)
            .into_iter()
            .map(|(_, text)| text)
            .filter(|text| {
                serde_json::from_str::<crate::slot_projection::SavedSlot>(text)
                    .unwrap()
                    .source_paths()
                    .contains(&&path)
            })
            .collect::<BTreeSet<_>>();
        let archive: String = seed
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
                [archive_index_key(path.to_str().unwrap())],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!original_recipes.is_empty());
        drop(seed);
        fs::remove_file(&path).unwrap();
        let (rows, projections) = catalog_fixture_projection(&options);
        assert_eq!(
            rows.iter()
                .find(|row| row.key == old.key)
                .unwrap()
                .sources
                .len(),
            1
        );
        let (mut batch, mut row_wise) = catalog_pair(&options);
        catalog_write_pair(&mut batch, &mut row_wise, &rows, &projections, false);
        let text = crate::history_projection::catalog(&batch.connection, &old.key)
            .unwrap()
            .unwrap();
        let history: crate::model::summary::CatalogRow = serde_json::from_str(&text).unwrap();
        assert_eq!(history.sources, old.sources);
        assert!(crate::history_projection::qualified(&batch.connection, &history).unwrap());
        let recipes = catalog_recipes(&batch.connection, "session_history_slots", &old.key)
            .into_iter()
            .map(|(_, text)| text)
            .collect::<BTreeSet<_>>();
        assert!(
            original_recipes.is_subset(&recipes),
            "missing native source recipe bytes remain in composite History"
        );
        assert_eq!(
            batch
                .connection
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [archive_index_key(path.to_str().unwrap())],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            archive
        );
        drop(batch);
        drop(row_wise);
        fs::remove_dir_all(root).unwrap();
    }

    fn traversal_stores(options: &crate::Options, count: usize) -> Vec<SqliteStore> {
        let seed = crate::events::EventCache::path(&options.cache);
        (0..count)
            .map(|arm| {
                let path = seed.with_file_name(format!("traversal-{arm}.sqlite3"));
                // Every EventCache/NativeEvidence owner has already been dropped.
                fs::copy(&seed, &path).unwrap();
                let config = options.source.config(&path).unwrap();
                let owner = configured::NativeOwnership::restore(&config).unwrap();
                let connection = configured::connection(&config).unwrap();
                crate::storage::NativeReaderBinding::Semantic(owner.clone())
                    .validate(&connection, &config.namespace)
                    .unwrap();
                SqliteStore {
                    owner,
                    connection,
                    path,
                    selected_runs: Some(Default::default()),
                    semantic_rejection: None,
                    traversal_observation: None,
                }
            })
            .collect()
    }

    fn traversal_header_fixture(
        root: &Path,
    ) -> (
        crate::Options,
        Vec<crate::model::summary::CatalogRow>,
        Vec<crate::slot_projection::Projection>,
    ) {
        let options = crate::Options {
            source: crate::Options::default().source,
            claude_home: root.join("absent-claude"),
            claude_json: root.join("absent-claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("absent-copilot"),
            proc_root: root.join("proc"),
            cache: root.join("seed.json"),
            all: true,
            since: Duration::from_secs(86400),
            session: None,
            facts: Some(root.join("facts.json")),
            scan_window: false,
        };
        fs::create_dir_all(&options.proc_root).unwrap();
        let mut paths = BTreeSet::new();
        for position in 0..2049 {
            let (relative, id) = if position == 2048 {
                (
                    "sessions/2026/10/09/selected.jsonl".to_owned(),
                    "selected".to_owned(),
                )
            } else {
                (
                    format!("sessions/history/{position:05}.jsonl"),
                    format!("unrelated-{position:05}"),
                )
            };
            let path = options.codex_home.join(&relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // Ordinary sanitized native headers, independent of private host code.
            fs::write(
                path,
                serde_json::json!({"type":"session_meta","payload":{"id":id}}).to_string() + "\n",
            )
            .unwrap();
            paths.insert(relative);
        }
        crate::write_facts(
            options.facts.as_ref().unwrap(),
            &crate::Facts {
                version: crate::FACTS_VERSION,
                hostname: "synthetic-traversal".into(),
                codex_rollouts: Some(paths),
                mirror_observation_id: Some("00000000000000000000000000000001".into()),
                mirror_sequence: Some(1),
                ..crate::Facts::default()
            },
            &options.source,
        )
        .unwrap();
        let inputs = crate::inputs::inputs(&options)
            .unwrap()
            .into_iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .collect::<Vec<_>>();
        assert_eq!(inputs.len(), 2049);
        let mut cache =
            crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let mut rows = Vec::new();
        let mut projections = Vec::new();
        for sources in inputs.chunks(64) {
            let prepared =
                crate::model::prepare_sources(&options, &mut cache, sources, CATALOG_FIXTURE_NOW)
                    .unwrap();
            assert!(prepared.claims.len() <= 64);
            rows.extend(prepared.rows);
            projections.extend(prepared.transcripts);
        }
        drop(cache); // close the WAL owner before main-file cloning
        rows.sort_by(|left, right| left.key.cmp(&right.key));
        projections.sort_by(|left, right| left.key.cmp(&right.key));
        assert_eq!(rows.len(), 2049);
        assert_eq!(projections.len(), rows.len());
        assert_eq!(rows.iter().filter(|row| row.key == "selected").count(), 1);
        assert!(
            rows.iter()
                .zip(&projections)
                .all(|(row, projection)| row.key == projection.key && row.sources.len() == 1)
        );
        // Header-only logs legitimately have no transcript slots. The existing
        // rich native fixture's nonempty-slot assertion is unchanged.
        assert!(
            projections
                .iter()
                .all(|projection| projection.slots.is_empty())
        );
        for cohort in rows.chunks(64) {
            crate::catalog_search::assert_posting_multiset(cohort).unwrap();
        }
        (options, rows, projections)
    }

    fn traversal_temp_state(connection: &Connection) -> CatalogState {
        let names = connection
            .prepare("SELECT name FROM sqlite_temp_schema WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        names
            .into_iter()
            .map(|name| {
                let mut statement = connection
                    .prepare(&format!(
                        "SELECT * FROM temp.\"{}\"",
                        name.replace('"', "\"\"")
                    ))
                    .unwrap();
                let width = statement.column_count();
                let mut rows = statement
                    .query_map([], |row| {
                        (0..width)
                            .map(|column| row.get::<_, rusqlite::types::Value>(column))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                rows.sort_by_key(|row| format!("{row:?}"));
                (name, rows)
            })
            .collect()
    }

    fn traversal_reader_oracle(connection: &Connection, queries: &[&str]) -> Value {
        use crate::CatalogReadScope;
        let mut evidence = Vec::new();
        for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
            let scope_key = crate::catalog_search::scope_key(scope);
            let sql = if scope == CatalogReadScope::Current {
                "SELECT metadata FROM session_catalog WHERE lifecycle='current'"
            } else {
                "SELECT COALESCE(h.metadata,c.metadata) FROM session_catalog c LEFT JOIN session_history_catalog h USING(session_key) WHERE h.metadata IS NOT NULL OR c.lifecycle='current'"
            };
            let rows = connection
                .prepare(sql)
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|row| {
                    serde_json::from_str::<crate::model::summary::CatalogRow>(&row.unwrap())
                        .unwrap()
                })
                .collect::<Vec<_>>();
            let mut grams_by_key = BTreeMap::new();
            let mut expected_counts = BTreeMap::<String, i64>::new();
            let mut complete_rows = Vec::new();
            let mut incomplete = 0;
            for row in rows {
                let fields = [
                    Some(row.name.as_str()),
                    Some(row.key.as_str()),
                    row.repo.as_deref(),
                    row.branch.as_deref(),
                    Some(row.model.as_str()),
                    Some(row.harness.as_str()),
                ]
                .into_iter()
                .flatten()
                .map(str::to_lowercase)
                .collect::<Vec<_>>();
                let mut grams = BTreeSet::new();
                for field in &fields {
                    let chars = field.chars().collect::<Vec<_>>();
                    for width in 1..=3 {
                        for slice in chars.windows(width) {
                            grams.insert(slice.iter().collect::<String>());
                        }
                    }
                }
                if fields.iter().map(String::len).sum::<usize>() > 8192 || grams.len() > 1024 {
                    incomplete += 1;
                    continue;
                }
                for gram in &grams {
                    *expected_counts.entry(gram.clone()).or_default() += 1;
                }
                grams_by_key.insert(row.key.clone(), grams);
                complete_rows.push((row, fields));
            }
            complete_rows.sort_by(|(a, _), (b, _)| {
                b.last
                    .unwrap_or(0)
                    .cmp(&a.last.unwrap_or(0))
                    .then_with(|| a.key.cmp(&b.key))
            });
            let actual_counts = connection.prepare("SELECT gram,members FROM session_search_counts WHERE read_scope=?1 ORDER BY gram").unwrap()
                .query_map([scope_key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))).unwrap().collect::<rusqlite::Result<BTreeMap<_, _>>>().unwrap();
            assert_eq!(
                actual_counts, expected_counts,
                "independent fields/grams count oracle"
            );
            let column = if scope == CatalogReadScope::Current {
                "current_last_ms"
            } else {
                "history_last_ms"
            };
            let physical = connection.prepare(&format!("SELECT session_key,gram,{column} FROM session_search_memberships WHERE {column} IS NOT NULL AND ?1 IN ('current','retained_history')")).unwrap()
                .query_map([scope_key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            let expected_positions = complete_rows
                .iter()
                .flat_map(|(row, _)| {
                    grams_by_key[&row.key]
                        .iter()
                        .map(move |gram| (row.key.clone(), gram.clone(), row.last.unwrap_or(0)))
                })
                .collect::<BTreeSet<_>>();
            assert_eq!(
                physical.len(),
                expected_positions.len(),
                "no duplicate logical positions"
            );
            assert_eq!(
                physical.into_iter().collect::<BTreeSet<_>>(),
                expected_positions
            );
            assert_eq!(
                connection
                    .query_row(
                        "SELECT incomplete FROM session_search_coverage WHERE read_scope=?1",
                        [scope_key],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                incomplete
            );
            for query in queries {
                let normalized = crate::catalog_search::normalize_query(query).unwrap();
                let expected = complete_rows
                    .iter()
                    .filter(|(_, fields)| fields.iter().any(|field| field.contains(&normalized)))
                    .map(|(row, _)| row.key.clone())
                    .collect::<Vec<_>>();
                let mut found = Vec::new();
                let mut pages = Vec::new();
                let mut after: Option<crate::catalog_search::Position> = None;
                for _ in 0..4096 {
                    let selection = crate::catalog_search::read_admitted(
                        connection,
                        crate::catalog_search::Query {
                            scope,
                            query: &normalized,
                            harness: None,
                            repo: None,
                            model: None,
                            after: after.clone(),
                            limit: 17,
                            index_complete: incomplete == 0,
                        },
                    )
                    .unwrap();
                    assert_eq!(selection.report.index_complete, incomplete == 0);
                    assert!(selection.report.candidates <= crate::catalog_search::MAX_CANDIDATES);
                    assert!(selection.report.candidates >= selection.rows.len());
                    found.extend(selection.rows.iter().map(|row| row.key.clone()));
                    let next = selection
                        .next
                        .as_ref()
                        .map(|position| (position.last, position.key.clone()));
                    if let (Some(before), Some(next)) = (&after, &selection.next) {
                        assert!(
                            next.last < before.last
                                || (next.last == before.last && next.key > before.key),
                            "checked-candidate cursor progresses"
                        );
                    }
                    pages.push(serde_json::json!({"keys":selection.rows.iter().map(|row| &row.key).collect::<Vec<_>>(),"next":next,"candidates":selection.report.candidates,"partial":selection.report.partial}));
                    after = selection.next;
                    if after.is_none() {
                        break;
                    }
                }
                assert!(after.is_none(), "bounded fixture search exhausted");
                assert_eq!(
                    found, expected,
                    "independent substring/time/key oracle for {scope_key}/{query}"
                );
                evidence.push(serde_json::json!({"scope":scope_key,"query":query,"pages":pages}));
            }
        }
        Value::Array(evidence)
    }

    fn traversal_semantic_and_rollback_oracles() {
        let root = scratch("traversal-semantics");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 64, false);
        crate::catalog_search::assert_posting_multiset(&rows).unwrap();
        let mut stores = traversal_stores(&options, 4);
        let writers = [
            CatalogWriter::Auto,
            CatalogWriter::Traversal {
                owners: 64,
                measured: false,
            },
            CatalogWriter::Traversal {
                owners: 16,
                measured: false,
            },
            CatalogWriter::Traversal {
                owners: 8,
                measured: false,
            },
        ];
        let mut outcomes = Vec::new();
        for (store, writer) in stores.iter_mut().zip(writers) {
            outcomes.push(catalog_write(store, &rows, &projections, true, writer).unwrap());
        }
        assert!(outcomes.windows(2).all(|pair| pair[0] == pair[1]));
        // Keep a history-only owner in every arm while replacing Current grams.
        let retained = rows[63].key.clone();
        for store in &mut stores {
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            tx.execute("INSERT INTO session_history_catalog SELECT session_key,json_set(metadata,'$.lifecycle','retained') FROM session_catalog WHERE session_key=?1", [&retained]).unwrap();
            tx.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key=?1", [&retained]).unwrap();
            crate::catalog_search::retire_key(&tx, &retained).unwrap();
            tx.commit().unwrap();
        }
        for row in &mut rows[..63] {
            row.name = "shared original grams".into();
            row.last = Some(700);
        }
        rows[1].last = None;
        rows[2].repo = None;
        rows[3].name = "x".repeat(8193); // explicit incomplete document, never truncated
        crate::catalog_search::assert_posting_multiset(&rows[..63]).unwrap();
        for (store, writer) in stores.iter_mut().zip(writers) {
            catalog_write(store, &rows[..63], &projections, true, writer).unwrap();
        }
        rows[0].name = "shared replacement selective".into();
        rows[1].name = "shared replacement".into();
        rows[..63].reverse(); // grouping must not depend on caller permutation
        crate::catalog_search::assert_posting_multiset(&rows[..63]).unwrap();
        for (store, writer) in stores.iter_mut().zip(writers) {
            catalog_write(store, &rows[..63], &projections, true, writer).unwrap();
        }
        let state = catalog_comparable_state(&stores[0].connection);
        let reader = traversal_reader_oracle(
            &stores[0].connection,
            &["shared", "selective", "original", "absent-xyz", &retained],
        );
        for store in &stores[1..] {
            assert_eq!(catalog_comparable_state(&store.connection), state);
            assert_eq!(
                traversal_reader_oracle(
                    &store.connection,
                    &["shared", "selective", "original", "absent-xyz", &retained]
                ),
                reader
            );
        }
        for store in &stores {
            assert!(store.connection.query_row("SELECT COUNT(*) FROM session_search_memberships WHERE session_key=?1 AND current_last_ms IS NULL AND history_last_ms IS NOT NULL", [&retained], |row| row.get::<_, i64>(0)).unwrap() > 0, "retained-history-only positions survive Current replacement");
        }
        drop(stores);

        crate::catalog_search::assert_posting_multiset(&rows).unwrap();

        // Cold roster: every arm begins with zero selected memberships. The
        // trigger predicate proves three earlier INSERTs succeeded in the same
        // transaction, irrespective of physical traversal order.
        let mut rollback = traversal_stores(&options, 3);
        for (store, owners) in rollback.iter_mut().zip([64, 16, 8]) {
            assert_eq!(
                store
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM session_search_memberships",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
            for (message, trigger) in [
                (
                    "traversal-after-three-valid",
                    "CREATE TEMP TRIGGER traversal_failure BEFORE INSERT ON session_search_memberships WHEN (SELECT COUNT(*) FROM session_search_memberships)=3 BEGIN SELECT RAISE(ABORT,'traversal-after-three-valid'); END;",
                ),
                (
                    "traversal-ack-after-search",
                    "CREATE TEMP TRIGGER traversal_failure BEFORE DELETE ON session_catalog_invalidations WHEN (SELECT COUNT(*) FROM session_search_memberships)>3 BEGIN SELECT RAISE(ABORT,'traversal-ack-after-search'); END;",
                ),
            ] {
                let tx = store
                    .connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .unwrap();
                for source in rows.iter().flat_map(|row| &row.sources) {
                    invalidate_catalog_source(&tx, &source.path.to_string_lossy(), "changed")
                        .unwrap();
                }
                tx.commit().unwrap();
                let before = catalog_state(&store.connection);
                let temp_before = traversal_temp_state(&store.connection);
                store.connection.execute_batch(trigger).unwrap();
                let error = catalog_write(
                    store,
                    &rows,
                    &projections,
                    true,
                    CatalogWriter::Traversal {
                        owners,
                        measured: true,
                    },
                )
                .unwrap_err();
                assert!(
                    error.to_string().contains(message),
                    "actual injected failure: {error}"
                );
                assert!(
                    store.traversal_observation.is_none(),
                    "rollback cannot emit a committed sample"
                );
                assert_eq!(
                    catalog_state(&store.connection),
                    before,
                    "FULL durable rollback including observation clock"
                );
                assert_eq!(
                    traversal_temp_state(&store.connection),
                    temp_before,
                    "TEMP roster/removal creation and contents roll back"
                );
                store
                    .connection
                    .execute_batch("DROP TRIGGER traversal_failure")
                    .unwrap();
            }
        }
        drop(rollback);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_search_traversal_conditional_growing_prefix_probe() {
        use std::io::Write;
        let root = scratch("traversal-growing-prefix");
        let (options, rows, projections) = traversal_header_fixture(&root);
        let mut stores = traversal_stores(&options, 3);
        let arms = [64, 16, 8];
        let mut observations = (0..3).map(|_| Vec::with_capacity(32)).collect::<Vec<_>>();
        for cohort in 0..32 {
            let range = cohort * 64..(cohort + 1) * 64;
            let mut paired = Vec::new();
            for offset in 0..3 {
                let arm = (cohort + offset) % 3;
                let store = &mut stores[arm];
                let outcome = catalog_write(
                    store,
                    &rows[range.clone()],
                    &projections[range.clone()],
                    true,
                    CatalogWriter::Traversal {
                        owners: arms[arm],
                        measured: true,
                    },
                )
                .unwrap();
                assert_eq!(outcome.0, Outcome::Written);
                paired.push(outcome);
                let observation = store.traversal_observation.take().unwrap();
                assert_eq!(observation.changed_rows, 64);
                assert_eq!(observation.search.owners, 64);
                assert_eq!(
                    observation.search.logical_positions,
                    observation.search.physical_rows * 2
                );
                observations[arm].push(observation);
            }
            assert!(
                paired.windows(2).all(|pair| pair[0] == pair[1]),
                "exact cohort outcome/generation parity"
            );
            let cardinalities = observations
                .iter()
                .map(|arm| arm.last().unwrap().search.physical_rows)
                .collect::<Vec<_>>();
            assert!(cardinalities[0] > 0);
            assert!(cardinalities.windows(2).all(|pair| pair[0] == pair[1]));
            // No snapshot/query oracle/cache reset between growing cohorts.
        }
        let final_outcomes = stores
            .iter_mut()
            .zip(arms)
            .map(|(store, owners)| {
                catalog_write(
                    store,
                    &rows[2048..],
                    &projections[2048..],
                    true,
                    CatalogWriter::Traversal {
                        owners,
                        measured: false,
                    },
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(final_outcomes.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(
            final_outcomes
                .iter()
                .all(|outcome| outcome.0 == Outcome::Written && outcome.1.is_some())
        );
        assert!(
            stores
                .iter()
                .all(|store| store.traversal_observation.is_none())
        );
        let state = catalog_comparable_state(&stores[0].connection);
        let published_keys = stores[0].connection.prepare("SELECT session_key FROM session_catalog WHERE lifecycle='current' ORDER BY session_key").unwrap()
            .query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(
            published_keys,
            rows.iter().map(|row| row.key.clone()).collect::<Vec<_>>(),
            "every independent fixture owner including the final singleton is published"
        );
        let reader = traversal_reader_oracle(
            &stores[0].connection,
            &["unrelated", "unrelated-012", "selected", "absent-xyz"],
        );
        for store in &stores[1..] {
            assert_eq!(
                catalog_comparable_state(&store.connection),
                state,
                "all durable tables/ordered feed/claims/authority/recipes match"
            );
            assert_eq!(
                traversal_reader_oracle(
                    &store.connection,
                    &["unrelated", "unrelated-012", "selected", "absent-xyz"]
                ),
                reader
            );
        }
        drop(stores);
        fs::remove_dir_all(root).unwrap();
        traversal_semantic_and_rollback_oracles();
        // Five fixed windows × three arms, after every span and integrity oracle.
        // Raw kernel counters can include existing SEMON_BUILD_TIMING stderr;
        // neither those counters nor WAL extents establish device writes.
        let timing_diagnostics = crate::native_timing::enabled();
        for (window, cohorts) in [
            ("prefix_0", 0..1),
            ("prefix_256", 4..5),
            ("prefix_1280", 20..21),
            ("prefix_1728", 27..28),
            ("all_32", 0..32),
        ] {
            for (arm, owners) in arms.into_iter().enumerate() {
                let mut total = observations[arm][cohorts.start].clone();
                for sample in &observations[arm][cohorts.start + 1..cohorts.end] {
                    total.search.search.add(&sample.search.search);
                    total.search.postings.add(&sample.search.postings);
                    total.search.counts.add(&sample.search.counts);
                    total.commit.add(&sample.commit);
                    total.search.preparation_us += sample.search.preparation_us;
                    total.search.sampling_us += sample.search.sampling_us;
                    total.search.owners += sample.search.owners;
                    total.search.physical_rows += sample.search.physical_rows;
                    total.search.logical_positions += sample.search.logical_positions;
                    total.changed_rows += sample.changed_rows;
                }
                let mut search_plus_commit = total.search.search.clone();
                search_plus_commit.add(&total.commit);
                let mut posting_and_counts = total.search.postings.clone();
                posting_and_counts.add(&total.search.counts);
                let remaining_search = total.search.search.excluding(&posting_and_counts);
                let unique_sampling_us = total.search.sampling_us + total.commit.sampling_us;
                let record = serde_json::json!({"publication":"committed","driver":"conditional_fully_prepared_index","window":window,"traversal_owners":owners,"cohorts":cohorts.len(),"timing_diagnostics":timing_diagnostics,"io_scope":"calling_thread","nested_intervals_not_additive":true,"sample":total,"remaining_search":remaining_search,"search_plus_actual_commit":search_plus_commit,"unique_sampling_us":unique_sampling_us});
                writeln!(
                    std::io::stderr().lock(),
                    "catalog-search-traversal-probe {record}"
                )
                .unwrap();
            }
        }
    }

    fn ledger(n: u8) -> Ledger {
        Ledger {
            stat: Stat {
                dev: u64::MAX,
                ino: 2,
                size: 3 + u64::from(n),
                modified_ns: u128::MAX - u128::from(n),
                changed_ns: Some(i128::MIN + i128::from(n)),
            },
            offset: 3,
            prefix: [n; 32],
            tail: [n.wrapping_add(1); 32],
        }
    }

    #[test]
    fn tree_metadata_and_evidence_share_one_snapshot_during_concurrent_commit() {
        let root = scratch("tree-snapshot");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        // Direct store reads return lazy checkpoints; bind them to the same
        // admitted Source used by the writer fixture before exercising them.
        let source_config = crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Sqlite {
                database: path.clone(),
            },
            namespace: fixture_namespace(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        let bind_checkpoint = |metadata: &mut crate::native_tree::Metadata| {
            metadata
                .summary
                .stored
                .as_mut()
                .unwrap()
                .bind_configured(source_config.clone());
        };
        let index = |model: &str| FileIndex {
            tree_summary: Some(crate::native_tree::Summary {
                models: [model.to_owned()].into_iter().collect(),
                tools: [("tree-tool".to_owned(), "Bash".to_owned())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            }),
            source_chunks: Some(std::sync::Arc::new(crate::source_chunks::Chunks {
                offset: 3,
                hashes: vec![[1; 32]],
                resume: None,
            })),
            marker_index: Some(crate::native_marker::Index::default()),
            handoff_index: Some(crate::native_handoff::Index::default()),
            ..Default::default()
        };
        let old = ledger(1);
        let new = ledger(2);
        assert_eq!(
            store
                .write_one("a.jsonl", None, None, &old, &index("old"))
                .unwrap(),
            Outcome::Written
        );
        let (_, mut initial) = store.read_tree("a.jsonl", &old.stat).unwrap().unwrap();
        bind_checkpoint(&mut initial);
        assert_eq!(
            initial.summary.tool_name("tree-tool").unwrap().as_deref(),
            Some("Bash")
        );
        assert_eq!(initial.summary.tool_name("absent-tool").unwrap(), None);
        initial.summary.load_shared_usage().unwrap();
        assert!(initial.summary.usage_records.is_empty());
        let (mut writer, _) = opened(&path);
        let old_copy = old.clone();
        let new_copy = new.clone();
        AFTER_TREE_LEDGER.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert_eq!(
                    writer
                        .write_one("a.jsonl", Some(&old_copy), None, &new_copy, &index("new"))
                        .unwrap(),
                    Outcome::Written
                );
            }))
        });
        let reads = super::super::FILE_READS.with(std::cell::Cell::get);
        let (observed, mut metadata) = store.read_tree("a.jsonl", &old.stat).unwrap().unwrap();
        bind_checkpoint(&mut metadata);
        assert_eq!(observed, old);
        assert_eq!(
            metadata.summary.models,
            ["old".to_owned()].into_iter().collect()
        );
        assert_eq!(
            metadata.summary.load_shared_usage().unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "the checkpoint reader retains its ledger refusal after a concurrent commit"
        );
        assert!(store.read_tree("a.jsonl", &old.stat).unwrap().is_none());
        let (observed, mut metadata) = store.read_tree("a.jsonl", &new.stat).unwrap().unwrap();
        bind_checkpoint(&mut metadata);
        assert_eq!(observed, new);
        assert_eq!(
            metadata.summary.models,
            ["new".to_owned()].into_iter().collect()
        );
        assert_eq!(super::super::FILE_READS.with(std::cell::Cell::get), reads);
        fs::remove_dir_all(root).unwrap();
    }

    /// A file index with every field set, and every value at an edge where
    /// one exists. Written without `..`: a field added to `FileIndex` or
    /// `Event` fails to compile here until it is set, and the round trip
    /// then proves it is persisted.
    fn full() -> FileIndex {
        FileIndex {
            native_path: None,
            dirty_rows: None,
            native_usage_observations: None,
            source_chunks: None,
            marker_index: Some(crate::native_marker::Index::default()),
            tree_summary: None,
            handoff_index: Some(crate::native_handoff::Index::default()),
            codex_header: Some(crate::native_header::CodexHeader::default()),
            claude_sidecar: None,
            events: vec![
                Event {
                    k: Kind::Tool,
                    o: u64::MAX,
                    b: u32::MAX,
                    t: Some(i64::MIN),
                    id: Some("call-1".to_owned()),
                    n: Some("Bash".to_owned()),
                    code_mode: true,
                    code_mode_open: true,
                    native_direct_call: true,
                    parent: Some(7),
                    script: STDIN | SENDS,
                    poll_target: Some("26789".to_owned()),
                    result_source: true,
                    y: Some(Box::new(Yield {
                        polls: vec![1, 2, u64::MAX],
                        cut: true,
                        done: Some(Reply {
                            o: 11,
                            b: 1,
                            t: Some(12),
                            e: true,
                            f: UNKNOWN,
                            m: Some("done".to_owned()),
                        }),
                    })),
                    poll: Some(0),
                    r: Some(Reply {
                        o: u64::MAX,
                        b: 2,
                        t: Some(14),
                        e: true,
                        f: ASYNC | PIN,
                        m: Some("msg-2".to_owned()),
                    }),
                    item: Some(u64::MAX),
                    peer: Some(Peer {
                        sock: Some("sock".to_owned()),
                        pid: Some(u64::MAX),
                        start: Some("777".to_owned()),
                    }),
                    u: Some("uuid-1".to_owned()),
                },
                // A peer with nothing known is still a peer.
                Event {
                    k: Kind::Xsm,
                    o: 20,
                    peer: Some(Peer::default()),
                    ..Event::default()
                },
                // A reply with only its line.
                Event {
                    k: Kind::Tool,
                    o: 21,
                    r: Some(Reply::default()),
                    ..Event::default()
                },
            ],
            extras: vec![Event {
                k: Kind::Think,
                o: 30,
                t: Some(31),
                ..Event::default()
            }],
            signals: vec![
                Signal {
                    k: SignalKind::Compact,
                    o: 40,
                    t: Some(41),
                    at: 1,
                    n: Some("auto".to_owned()),
                    v: Some(u64::MAX),
                },
                Signal {
                    k: SignalKind::Interrupt,
                    o: 42,
                    t: None,
                    at: u32::MAX,
                    n: None,
                    v: None,
                },
            ],
            pending: BTreeMap::from([("call-1".to_owned(), 0)]),
            tool_ids: BTreeMap::from([("call-1".to_owned(), vec![0, 2])]),
            yields: BTreeMap::from([("9".to_owned(), 0)]),
            busy: vec![(i64::MIN, -1), (1, i64::MAX)],
            links: Links {
                session_ids: BTreeSet::from(["session".to_owned()]),
                continued_in: BTreeSet::from(["continued".to_owned()]),
                bridges: BTreeSet::from(["bridge".to_owned()]),
            },
            entrypoint: Some("cli".to_owned()),
            title: Some("title".to_owned()),
            agent_name: Some("agent".to_owned()),
            last_model: Some("model".to_owned()),
            failed: true,
            first: Some(-1),
            last: Some(i64::MAX),
            cwd: Some("/work".to_owned()),
            branch: Some("main".to_owned()),
            usage_by_id: BTreeMap::from([
                (
                    "msg-1".to_owned(),
                    MessageUsage {
                        record_ids: std::collections::BTreeSet::from(["record-uuid".into()]),
                        model: Some("claude-test".to_owned()),
                        tokens: Tokens {
                            input: 1,
                            cached_input: 2,
                            output: 3,
                            reasoning_output: 4,
                            total: u64::MAX,
                        },
                        model_tokens: ModelTokens {
                            input: 5,
                            output: 6,
                            cache_write: 7,
                            cache_read: 8,
                        },
                        billing: BillingUsage {
                            input: 9,
                            output: 10,
                            cache_read: 11,
                            cache_write_5m: 12,
                            cache_write_1h: 13,
                            web_search_requests: 14,
                            speed: Some("fast".to_owned()),
                            service_tier: Some("priority".to_owned()),
                            prompt_size: 15,
                            timestamp: Some(16),
                            split_unknown: true,
                        },
                    },
                ),
                ("msg-2".to_owned(), MessageUsage::default()),
            ]),
            codex_native_usage: None,
            copilot: None,
            codex_tokens: Tokens {
                input: 17,
                cached_input: 18,
                output: 19,
                reasoning_output: 20,
                total: 21,
            },
            codex_tokens_by_model: BTreeMap::from([(
                "gpt-test".to_owned(),
                ModelTokens {
                    input: 22,
                    output: 23,
                    cache_write: 24,
                    cache_read: 25,
                },
            )]),
            codex_usage_events: vec![CodexUsageEvent {
                model: "gpt-test".to_owned(),
                tokens: ModelTokens {
                    input: 26,
                    output: 27,
                    cache_write: 28,
                    cache_read: 29,
                },
                timestamp: Some(30),
            }],
            rate_limits: Some(RateLimits {
                recorded_at: 31,
                windows: vec![RateLimitWindow {
                    minutes: 300,
                    used_percent: 12.5,
                    resets_at: 32,
                }],
            }),
        }
    }

    fn run() -> ReportedRunSnapshot {
        ReportedRunSnapshot {
            last_session_id: "run".to_owned(),
            last_start_time: 1000,
            last_cost: Some(1.25),
            last_duration: Some(u64::MAX),
            last_api_duration: Some(2),
            last_tool_duration: None,
            last_lines_added: Some(4),
            last_lines_removed: Some(5),
            last_model_usage: BTreeMap::from([(
                "claude-test".to_owned(),
                crate::facts::ReportedModelUsage {
                    input_tokens: 10,
                    cost_usd: Some(0.5),
                    ..crate::facts::ReportedModelUsage::default()
                },
            )]),
            capture_at: 2000,
        }
    }

    fn stamp() -> ReportedFileStamp {
        ReportedFileStamp {
            dev: 1,
            ino: 2,
            size: 3,
            modified_ns: 1_790_000_000_123_456_789,
        }
    }

    #[test]
    fn only_semons_own_directories_are_tightened() {
        let state = Path::new("/home/u/.local/state/semon");
        let store = |path: &str| Path::new(path).to_owned();
        assert_eq!(
            tightened(&state.join("sessions-index.sqlite3"), state),
            Some(state.to_owned())
        );
        // Received machines sit under it: nothing above them.
        assert_eq!(
            tightened(
                &state.join("received/abc/machine/sessions-index.sqlite3"),
                state
            ),
            None
        );
        assert_eq!(
            tightened(&store("/work/machine/.semon/sessions-index.sqlite3"), state),
            Some(store("/work/machine/.semon"))
        );
        // A checkout named `semon`, directly or further up.
        assert_eq!(
            tightened(&store("/home/u/projects/semon/idx.sqlite3"), state),
            None
        );
        assert_eq!(
            tightened(&store("/home/u/projects/semon/tmp/idx.sqlite3"), state),
            None
        );
        assert_eq!(tightened(&store("/tmp/idx.sqlite3"), state), None);
    }

    #[test]
    fn catalog_invalidation_is_atomic_durable_and_conflict_checked() {
        let root = scratch("catalog-invalidations");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        let invalidation = |store: &SqliteStore| {
            store.connection.query_row(
                "SELECT revision,reason FROM session_catalog_invalidations WHERE source_path='a.jsonl'",
                [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            ).optional().unwrap()
        };
        store
            .write_one("a.jsonl", None, None, &ledger(1), &full())
            .unwrap();
        assert_eq!(invalidation(&store), Some((1, "changed".into())));
        drop(store);
        let (mut store, _) = opened(&path);
        assert_eq!(invalidation(&store), Some((1, "changed".into())));
        assert_eq!(
            store
                .write_one("a.jsonl", None, None, &ledger(2), &full())
                .unwrap(),
            Outcome::Conflict
        );
        assert_eq!(invalidation(&store), Some((1, "changed".into())));
        store.connection.execute_batch("CREATE TEMP TRIGGER fail_observation BEFORE UPDATE ON session_catalog_invalidations BEGIN SELECT RAISE(ABORT,'interrupted'); END;").unwrap();
        assert!(
            store
                .write_one("a.jsonl", Some(&ledger(1)), None, &ledger(2), &full())
                .is_err()
        );
        assert_eq!(store.read_one("a.jsonl").unwrap().unwrap().0, ledger(1));
        assert_eq!(invalidation(&store), Some((1, "changed".into())));
        store
            .connection
            .execute_batch("DROP TRIGGER fail_observation")
            .unwrap();
        store
            .write_one(
                "a.jsonl",
                Some(&ledger(1)),
                Some(&full()),
                &ledger(2),
                &full(),
            )
            .unwrap();
        assert_eq!(invalidation(&store), Some((2, "changed".into())));
        store
            .remove(&[("a.jsonl".into(), Some(ledger(1)))])
            .unwrap();
        assert_eq!(invalidation(&store), Some((2, "changed".into())));
        store
            .remove(&[("a.jsonl".into(), Some(ledger(2)))])
            .unwrap();
        assert_eq!(invalidation(&store), Some((3, "removed".into())));
        assert!(store.read_one("a.jsonl").unwrap().is_none());
        drop(store);
        let (mut store, _) = opened(&path);
        assert_eq!(invalidation(&store), Some((3, "removed".into())));
        // Acknowledging work never reuses a revision for later observations.
        store
            .connection
            .execute("DELETE FROM session_catalog_invalidations", [])
            .unwrap();
        store
            .write_one("a.jsonl", None, None, &ledger(3), &full())
            .unwrap();
        assert_eq!(invalidation(&store), Some((4, "changed".into())));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_selected_repair_ignores_unrelated_archive_markers_and_refuses_selected_damage() {
        let root = scratch("selected-archive-marker");
        let (mut store, _) = opened(&root.join("index.sqlite3"));
        store.connection = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(store.owner.config()).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let unrelated_key = archive_index_key("unrelated.jsonl");
        let selected_key = archive_index_key("selected.jsonl");
        store
            .write_one("selected.jsonl", None, None, &ledger(1), &full())
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
                params![unrelated_key, r#"{"version":1}"#],
            )
            .unwrap();

        // Focused qualification and a selected removal do not traverse an
        // unrelated archive marker. The selected operation remains usable.
        require_current(&store.connection).unwrap();
        assert_eq!(
            store
                .remove(&[("selected.jsonl".into(), Some(ledger(1)))])
                .unwrap(),
            Outcome::Written
        );
        assert!(store.read_one("selected.jsonl").unwrap().is_none());
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [&unrelated_key],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            r#"{"version":1}"#
        );

        // An invalid marker for the selected key still refuses the repair
        // before it changes the indexed source, its events or its claim.
        store
            .write_one("selected.jsonl", None, None, &ledger(2), &full())
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
                params![selected_key, r#"{"version":1}"#],
            )
            .unwrap();
        let before = store
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM files), (SELECT COUNT(*) FROM events), \
                 (SELECT COUNT(*) FROM session_catalog_invalidations WHERE source_path='selected.jsonl'), \
                 (SELECT value FROM meta WHERE key=?1)",
                [&selected_key],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert!(
            store
                .remove(&[("selected.jsonl".into(), Some(ledger(2)))])
                .is_err()
        );
        let after = store
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM files), (SELECT COUNT(*) FROM events), \
                 (SELECT COUNT(*) FROM session_catalog_invalidations WHERE source_path='selected.jsonl'), \
                 (SELECT value FROM meta WHERE key=?1)",
                [&selected_key],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(after, before);
        assert_eq!(
            store.read_one("selected.jsonl").unwrap().unwrap().0,
            ledger(2)
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn usage_delete_fixture() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.pragma_update(None, "foreign_keys", true).unwrap();
        db.create_scalar_function(
            "semon_namespace",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok("selected".to_owned()),
        )
        .unwrap();
        db.execute_batch(
            "CREATE TABLE files(namespace TEXT NOT NULL,file_id INTEGER NOT NULL,PRIMARY KEY(namespace,file_id)) STRICT, WITHOUT ROWID; INSERT INTO files VALUES('selected',1),('selected',2),('foreign',1);",
        )
        .unwrap();
        db.execute_batch(copied_usage_schema()).unwrap();
        let seed = ["a", "b", "c", "z"]
            .into_iter()
            .map(|id| {
                (
                    id.to_owned(),
                    MessageUsage {
                        record_ids: BTreeSet::from([format!("old-{id}")]),
                        ..MessageUsage::default()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let changes = super::super::DirtyRows {
            usage: seed.keys().cloned().collect(),
            ..Default::default()
        };
        db.create_scalar_function(
            "semon_namespace",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok("foreign".to_owned()),
        )
        .unwrap();
        let tx = db.unchecked_transaction().unwrap();
        write_copied_usage_rows(&tx, 1, &changes, &seed, &mut Default::default()).unwrap();
        tx.commit().unwrap();
        db.create_scalar_function(
            "semon_namespace",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok("selected".to_owned()),
        )
        .unwrap();
        let tx = db.unchecked_transaction().unwrap();
        write_copied_usage_rows(&tx, 1, &changes, &seed, &mut Default::default()).unwrap();
        write_copied_usage_rows(&tx, 2, &changes, &seed, &mut Default::default()).unwrap();
        tx.commit().unwrap();
        db.execute_batch("CREATE TABLE audit(seq INTEGER PRIMARY KEY,event TEXT,message TEXT); CREATE TEMP TRIGGER identity_delete AFTER DELETE ON usage_identity BEGIN INSERT INTO audit(event,message) VALUES('identity-delete',OLD.message_id); END; CREATE TEMP TRIGGER usage_delete AFTER DELETE ON usage BEGIN INSERT INTO audit(event,message) VALUES('usage-delete',OLD.message_id); END; CREATE TEMP TRIGGER usage_update AFTER UPDATE ON usage BEGIN INSERT INTO audit(event,message) VALUES('usage-update',NEW.message_id); END; CREATE TEMP TRIGGER identity_insert AFTER INSERT ON usage_identity BEGIN INSERT INTO audit(event,message) VALUES('identity-insert',NEW.message_id); END;").unwrap();
        db
    }

    fn usage_delete_snapshot(db: &Connection) -> Vec<Vec<Vec<String>>> {
        ["usage", "usage_identity", "codex_usage", "audit"]
            .into_iter()
            .map(|table| {
                let mut statement = db
                    .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
                    .unwrap();
                let columns = statement.column_count();
                statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|column| Ok(format!("{:?}", row.get_ref(column)?)))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn usage_delete_reuse_matches_rowwise_order_and_preserves_foreign_rows() {
        let db = usage_delete_fixture();
        let reference = usage_delete_fixture();
        let changes = super::super::DirtyRows {
            usage: ["a", "b", "c", "z"].map(str::to_owned).into(),
            ..Default::default()
        };
        let desired = BTreeMap::from([(
            "b".to_owned(),
            MessageUsage {
                record_ids: BTreeSet::from(["new-b".to_owned(), "other-b".to_owned()]),
                ..MessageUsage::default()
            },
        )]);
        let before = usage_delete_snapshot(&db);
        let tx = db.unchecked_transaction().unwrap();
        let changed_before = tx.total_changes();
        assert_eq!(
            write_copied_usage_rows(
                &tx,
                1,
                &Default::default(),
                &desired,
                &mut Default::default(),
            )
            .unwrap(),
            UsageDeleteWork::default()
        );
        assert_eq!(tx.total_changes(), changed_before);
        assert_eq!(usage_delete_snapshot(&tx), before);
        let work =
            write_copied_usage_rows(&tx, 1, &changes, &desired, &mut Default::default()).unwrap();
        assert_eq!(
            work,
            UsageDeleteWork {
                identity_prepares: 1,
                identity_executions: 4,
                usage_prepares: 1,
                usage_executions: 3,
            }
        );
        tx.commit().unwrap();
        // Frozen rowwise reference: each deletion gets its own prepare;
        // compare complete values and the actual trigger event order.
        let tx = reference.unchecked_transaction().unwrap();
        let mut put = tx.prepare(COMPOSITE_PUT_USAGE).unwrap();
        let mut identity = tx.prepare("INSERT INTO usage_identity(namespace,file_id,message_id,record_uuid) VALUES (semon_namespace(),?1,?2,?3)").unwrap();
        for id in &changes.usage {
            tx.execute("DELETE FROM usage_identity WHERE namespace=semon_namespace() AND file_id=?1 AND message_id=?2", params![1, id]).unwrap();
            if let Some(usage) = desired.get(id) {
                put_usage(&mut put, 1, id, usage).unwrap();
                for uuid in &usage.record_ids {
                    identity.execute(params![1, id, uuid]).unwrap();
                }
            } else {
                tx.execute("DELETE FROM usage WHERE namespace=semon_namespace() AND file_id=?1 AND message_id=?2", params![1, id]).unwrap();
            }
        }
        drop((put, identity));
        tx.commit().unwrap();
        assert_eq!(
            usage_delete_snapshot(&db),
            usage_delete_snapshot(&reference)
        );
        let foreign: i64 = db
            .query_row(
                "SELECT count(*) FROM usage WHERE namespace='foreign'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(foreign, 4);
    }

    #[test]
    fn usage_delete_statements_are_transaction_owned_and_recheck_on_reuse() {
        let db = usage_delete_fixture();
        let changes = super::super::DirtyRows {
            usage: ["a", "b", "c", "z"].map(str::to_owned).into(),
            ..Default::default()
        };
        let before = usage_delete_snapshot(&db);
        let tx = db.unchecked_transaction().unwrap();
        let layout = WriterLayout::copied_usage_only(&tx).unwrap();
        let mut deletes = UsageDeletes::default();
        tx.execute_batch("CREATE TEMP TRIGGER fail_last BEFORE DELETE ON usage WHEN OLD.message_id='z' BEGIN SELECT RAISE(ABORT,'last usage delete'); END;").unwrap();
        for _ in 0..2 {
            tx.execute_batch("SAVEPOINT usage_attempt").unwrap();
            let error = write_copied_usage_rows_with_deletes(
                &tx,
                1,
                &changes,
                &BTreeMap::new(),
                &mut Default::default(),
                &layout,
                &mut deletes,
            )
            .unwrap_err();
            assert!(error.to_string().contains("last usage delete"));
            tx.execute_batch("ROLLBACK TO usage_attempt; RELEASE usage_attempt")
                .unwrap();
            assert_eq!(usage_delete_snapshot(&tx), before);
        }
        tx.execute_batch("DROP TRIGGER fail_last; SAVEPOINT malformed; DROP INDEX usage_identity_peers; CREATE INDEX usage_identity_peers ON usage_identity(file_id);").unwrap();
        let changed_before = tx.total_changes();
        assert!(
            write_copied_usage_rows_with_deletes(
                &tx,
                1,
                &changes,
                &BTreeMap::new(),
                &mut Default::default(),
                &layout,
                &mut deletes,
            )
            .is_err()
        );
        assert_eq!(tx.total_changes(), changed_before);
        tx.execute_batch("ROLLBACK TO malformed; RELEASE malformed")
            .unwrap();
        // Reuse after ABORT and DDL rollback retains no failed bindings.
        let work = write_copied_usage_rows_with_deletes(
            &tx,
            1,
            &changes,
            &BTreeMap::new(),
            &mut Default::default(),
            &layout,
            &mut deletes,
        )
        .unwrap();
        assert_eq!(work.identity_prepares + work.usage_prepares, 0);
        drop(deletes);
        let mut deletes = UsageDeletes::default();
        // Sixty-four calls model a maximum cold transaction: two authored
        // prepares total, versus 64 * (4 + 4) rowwise prepares.
        for call in 0..64 {
            let work = write_copied_usage_rows_with_deletes(
                &tx,
                1 + call % 2,
                &changes,
                &BTreeMap::new(),
                &mut Default::default(),
                &layout,
                &mut deletes,
            )
            .unwrap();
            assert_eq!(
                work,
                UsageDeleteWork {
                    identity_prepares: usize::from(call == 0),
                    identity_executions: 4,
                    usage_prepares: usize::from(call == 0),
                    usage_executions: 4,
                }
            );
        }
        drop(deletes);
        tx.rollback().unwrap();
        assert_eq!(usage_delete_snapshot(&db), before);
        // A new transaction constructs new statements, even after rollback.
        let tx = db.unchecked_transaction().unwrap();
        let work =
            write_copied_usage_rows(&tx, 1, &changes, &BTreeMap::new(), &mut Default::default())
                .unwrap();
        assert_eq!(work.identity_prepares + work.usage_prepares, 2);
        tx.rollback().unwrap();
    }

    #[test]
    fn usage_delete_reuse_rolls_back_and_requalifies_ddl_and_aba() {
        let db = usage_delete_fixture();
        let changes = super::super::DirtyRows {
            usage: ["a", "b", "c", "z"].map(str::to_owned).into(),
            ..Default::default()
        };
        let before = usage_delete_snapshot(&db);
        db.execute_batch("CREATE TEMP TRIGGER fail_last BEFORE DELETE ON usage WHEN OLD.message_id='z' BEGIN SELECT RAISE(ABORT,'last usage delete'); END;").unwrap();
        for _ in 0..2 {
            let tx = db.unchecked_transaction().unwrap();
            let error = write_copied_usage_rows(
                &tx,
                1,
                &changes,
                &BTreeMap::new(),
                &mut Default::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("last usage delete"));
            tx.rollback().unwrap();
            assert_eq!(usage_delete_snapshot(&db), before);
        }
        db.execute_batch("DROP TRIGGER fail_last").unwrap();
        // Same-version malformed current family must refuse before DML.
        // Rollback restores its cookie; a later DDL can reuse that cookie.
        for commit in [false, true] {
            let tx = db.unchecked_transaction().unwrap();
            tx.execute_batch("DROP INDEX usage_identity_peers; CREATE INDEX usage_identity_peers ON usage_identity(file_id);").unwrap();
            let changed_before = tx.total_changes();
            assert!(
                write_copied_usage_rows(
                    &tx,
                    1,
                    &changes,
                    &BTreeMap::new(),
                    &mut Default::default(),
                )
                .is_err()
            );
            assert_eq!(tx.total_changes(), changed_before);
            if commit {
                tx.commit().unwrap();
            } else {
                tx.rollback().unwrap();
            }
            assert_eq!(usage_delete_snapshot(&db), before);
        }
        let tx = db.unchecked_transaction().unwrap();
        let changed_before = tx.total_changes();
        assert!(
            write_copied_usage_rows(&tx, 1, &changes, &BTreeMap::new(), &mut Default::default(),)
                .is_err()
        );
        assert_eq!(tx.total_changes(), changed_before);
        tx.rollback().unwrap();
        db.execute_batch("DROP INDEX usage_identity_peers; CREATE INDEX usage_identity_peers ON usage_identity(namespace,message_id,record_uuid,file_id);")
        .unwrap();
        let tx = db.unchecked_transaction().unwrap();
        assert_eq!(
            write_copied_usage_rows(&tx, 1, &changes, &BTreeMap::new(), &mut Default::default(),)
                .unwrap()
                .identity_prepares,
            1
        );
        tx.commit().unwrap();
    }

    #[test]
    fn usage_identity_peers_require_message_and_record_and_update_atomically() {
        let root = scratch("usage-identities");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        let index = |message: &str, uuid: &str| {
            let mut index = full();
            index.usage_by_id = BTreeMap::from([(
                message.into(),
                MessageUsage {
                    record_ids: BTreeSet::from([uuid.into()]),
                    ..MessageUsage::default()
                },
            )]);
            index
        };
        let peers = |store: &SqliteStore| {
            store
                .connection
                .prepare(
                    "SELECT files.path FROM usage_identity
                JOIN files USING(file_id) WHERE message_id='api' AND record_uuid='record'
                ORDER BY files.path",
                )
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        };
        for (name, message, uuid) in [
            ("a", "api", "record"),
            ("b", "api", "record"),
            ("c", "api", "bridge"),
            ("other-record", "api", "different"),
            ("other-api", "different", "record"),
        ] {
            let mut source = index(message, uuid);
            if name == "b" {
                source
                    .usage_by_id
                    .get_mut("api")
                    .unwrap()
                    .record_ids
                    .insert("bridge".into());
            }
            store
                .write_one(name, None, None, &ledger(1), &source)
                .unwrap();
        }
        assert_eq!(peers(&store), ["a", "b"]);
        let closure_sql = "WITH RECURSIVE related(file_id) AS (
            SELECT file_id FROM files WHERE path='a'
            UNION SELECT peer.file_id FROM related
            JOIN usage_identity mine ON mine.file_id=related.file_id
            JOIN usage_identity peer ON peer.message_id=mine.message_id AND peer.record_uuid=mine.record_uuid
            LIMIT ?1) SELECT path FROM related JOIN files USING(file_id) ORDER BY path";
        let closure = |limit| {
            store
                .connection
                .prepare(closure_sql)
                .unwrap()
                .query_map([limit], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        };
        assert_eq!(closure(4), ["a", "b", "c"]);
        // A worker must treat the extra row as overflow, never complete context.
        assert_eq!(closure(2).len(), 2);
        let plan: String = store.connection.query_row(
            "EXPLAIN QUERY PLAN SELECT file_id FROM usage_identity WHERE message_id=?1 AND record_uuid=?2",
            ["api", "record"], |row| row.get(3),
        ).unwrap();
        assert!(
            plan.contains("COVERING INDEX usage_identity_peers"),
            "{plan}"
        );
        store.connection.execute_batch("CREATE TEMP TRIGGER fail_identity BEFORE INSERT ON usage_identity BEGIN SELECT RAISE(ABORT,'interrupted'); END;").unwrap();
        assert!(
            store
                .write_one(
                    "b",
                    Some(&ledger(1)),
                    None,
                    &ledger(2),
                    &index("api", "replacement")
                )
                .is_err()
        );
        assert_eq!(peers(&store), ["a", "b"]);
        assert_eq!(store.read_one("b").unwrap().unwrap().0, ledger(1));
        store
            .connection
            .execute_batch("DROP TRIGGER fail_identity")
            .unwrap();
        store
            .write_one(
                "b",
                Some(&ledger(1)),
                Some(&index("api", "record")),
                &ledger(2),
                &index("api", "replacement"),
            )
            .unwrap();
        assert_eq!(peers(&store), ["a"]);
        store.remove(&[("a".into(), Some(ledger(1)))]).unwrap();
        assert!(peers(&store).is_empty());
        drop(store);
        let (store, _) = opened(&path);
        assert!(peers(&store).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsupported_contracts_refuse_every_open_without_mutating_originals() {
        let snapshot = |db: &Connection| {
            let version: i64 = db
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            let schema = db
                .prepare("SELECT name,sql FROM sqlite_schema ORDER BY name")
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (version, schema, catalog_state(db))
        };
        for (name, change, reason) in [
            (
                "nonempty-zero",
                "PRAGMA user_version=0",
                "nonempty schema zero",
            ),
            ("newer", "PRAGMA user_version=2147483647", "newer schema"),
            ("older", "PRAGMA user_version=32", "older schema"),
            (
                "application",
                "PRAGMA application_id=1234",
                "application id",
            ),
            (
                "missing-index",
                "DROP INDEX session_catalog_current_order",
                "missing current schema object",
            ),
            (
                "missing-search-index",
                "DROP INDEX catalog_search_header",
                "missing current schema object",
            ),
            (
                "missing-journal",
                "DROP TABLE source_observation_receipts",
                "missing current schema object",
            ),
            (
                "foreign",
                "ALTER TABLE events RENAME TO foreign_events; ALTER TABLE files RENAME TO foreign_files",
                "malformed schema",
            ),
            (
                "mixed",
                "CREATE TABLE unrelated(secret TEXT); PRAGMA user_version=32",
                "older schema",
            ),
            (
                "older-with-invalid-profile-admission",
                "PRAGMA user_version=32; UPDATE main.storage_semantic_profiles SET admission=2147483647 WHERE namespace=semon_namespace()",
                "older schema",
            ),
            (
                "older-with-future-recipe",
                "PRAGMA user_version=32; UPDATE main.storage_semantic_profiles SET recipe='recorded-origin-turn-complete-receipts-v99' WHERE namespace=semon_namespace()",
                "older schema",
            ),
            (
                "missing",
                "DELETE FROM main.storage_semantic_profiles WHERE namespace=semon_namespace()",
                "missing metadata",
            ),
            (
                "missing-table",
                "ALTER TABLE meta RENAME TO original_meta",
                "missing metadata",
            ),
            (
                "missing-reported-runs",
                "DROP TABLE reported_runs",
                "malformed schema",
            ),
            (
                "missing-reported-column",
                "ALTER TABLE reported_runs DROP COLUMN capture_at",
                "malformed persistent state",
            ),
            (
                "missing-file-column",
                "ALTER TABLE files DROP COLUMN pending",
                "malformed persistent state",
            ),
            (
                "missing-event-column",
                "DROP INDEX semantic_index_send; ALTER TABLE events DROP COLUMN reply_m",
                "malformed persistent state",
            ),
            (
                "missing-child-table",
                "DROP TABLE signals",
                "malformed schema",
            ),
            (
                "malformed",
                "UPDATE main.storage_semantic_profiles SET recipe='invalid' WHERE namespace=semon_namespace()",
                "malformed metadata",
            ),
            (
                "invalid-profile-admission",
                "UPDATE main.storage_semantic_profiles SET admission=2147483647 WHERE namespace=semon_namespace()",
                "profile admission mismatch",
            ),
            (
                "publication",
                "INSERT INTO meta VALUES('fixture-source','catalog_version','4294967295')",
                "publication version",
            ),
            (
                "search",
                "INSERT INTO meta VALUES('fixture-source','search_version','4294967295')",
                "publication version",
            ),
            (
                "recipe-other-family",
                "UPDATE main.storage_semantic_profiles SET recipe='recorded-origin-turn-complete-receipts-v2' WHERE namespace=semon_namespace()",
                "publication version",
            ),
            (
                "recipe-older-current-family",
                "UPDATE main.storage_semantic_profiles SET recipe='recorded-origin-turn-native-name-hints-v3' WHERE namespace=semon_namespace()",
                "publication version",
            ),
            (
                "archive",
                "INSERT INTO meta VALUES('fixture-source','archive_event_index:original','{\"version\":7}')",
                "archived marker version",
            ),
        ] {
            let root = scratch(name);
            let v1 = root.join("index.json");
            let path = v1.with_extension("sqlite3");
            let source =
                crate::storage::HostSource::new(fixture_namespace(), "a".repeat(64)).unwrap();
            let native = root.join("claude/projects/fixture/original.jsonl");
            fs::create_dir_all(native.parent().unwrap()).unwrap();
            fs::write(&native, "{\"type\":\"user\",\"uuid\":\"original\",\"sessionId\":\"original\",\"message\":{\"role\":\"user\",\"content\":\"original native input\"}}\n").unwrap();
            let mut cache = super::super::EventCache::open_scoped(&v1, &source).unwrap();
            super::super::scan_file(&native, "claude", &mut cache, &mut false).unwrap();
            drop(cache);
            let (mut store, _) = opened(&path);
            // Fixture writes and corruption target the physical current
            // schema through the admitted publisher, before any writes.
            store.connection = crate::storage::sqlite::semantic::Stage::open(
                crate::storage::sqlite::semantic::SemanticStaging::admit(store.owner.config())
                    .unwrap(),
            )
            .unwrap()
            .into_test_connection();
            store.write_runs(&[run()], &stamp()).unwrap();
            // Custody is deliberately opaque to version classification.
            store.connection.execute("INSERT INTO session_history_catalog(namespace,session_key,metadata) VALUES('fixture-source','original','{\"custody\":\"retain exact bytes\"}')", []).unwrap();
            store
                .connection
                .execute(
                    "INSERT INTO meta VALUES('fixture-source','catalog_recovery_reason','original proof')",
                    [],
                )
                .unwrap();
            if name == "archive" {
                // Bind the unsupported marker to the real consumed input, so
                // Facts discovery reaches its normal selected-source reader.
                store.connection.execute("INSERT INTO main.meta(namespace,key,value) VALUES('fixture-source',?1,'{\"version\":7}')", [archive_index_key(native.to_str().unwrap())]).unwrap();
            } else {
                store
                    .connection
                    .execute_batch(change)
                    .unwrap_or_else(|error| panic!("{name} fixture corruption: {error}"));
            }
            let original_state = snapshot(&store.connection);
            // Finish fixture writes before snapshotting the original file.
            let checkpoint: (i64, i64, i64) = store
                .connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap();
            assert_eq!(checkpoint, (0, 0, 0), "{name} fixture checkpoint");
            drop(store);
            let original = fs::read(&path).unwrap();
            let unchanged = || {
                assert_eq!(fs::read(&path).unwrap(), original, "{name} database bytes");
                // A read-only WAL connection can create coordination sidecars.
                // No WAL bytes means no frames or commits were written.
                match fs::read(sibling(&path, "-wal")) {
                    Ok(bytes) => assert!(bytes.is_empty(), "{name} wrote WAL bytes"),
                    Err(error) => assert_eq!(error.kind(), io::ErrorKind::NotFound),
                }
                assert!(!sibling(&path, ".corrupt").exists());
            };
            unchanged();
            let source =
                crate::storage::HostSource::new(fixture_namespace(), "a".repeat(64)).unwrap();
            if name != "archive" {
                assert!(
                    open_admitted(&path, &source).is_err(),
                    "{name} must be explicitly refused"
                );
                unchanged();
            }
            if name != "archive" {
                assert!(
                    fixture_reader_binding(&path)
                        .and_then(|binding| focused_connection(&binding))
                        .is_err(),
                    "focused {name}"
                );
            } else {
                let db = fixture_reader_binding(&path)
                    .and_then(|binding| focused_connection(&binding))
                    .unwrap();
                assert!(archived_index(&db, native.to_str().unwrap()).is_err());
            }
            unchanged();
            match super::super::EventCache::open(&v1, &source) {
                Err(_) if name != "archive" => {}
                Ok(_) if name == "archive" => {}
                _ => panic!("unexpected admission for {name}"),
            }
            unchanged();
            let options = crate::Options {
                cache: v1.clone(),
                source: source.clone(),
                claude_home: root.join("claude"),
                claude_json: root.join(".claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                ..crate::Options::default()
            };
            let retained_raw = root.join("retained-original.jsonl");
            let original_native = fs::read(&native).unwrap();
            if name == "archive" {
                // Current Facts need not read unused historical custody. Move
                // Raw outside discovery (preserving its bytes) so the ordinary
                // disappearance writer must consume this exact archive marker.
                fs::rename(&native, &retained_raw).unwrap();
            }
            let error = crate::local_facts(&options).unwrap_err();
            if name == "archive" {
                assert_eq!(
                    error
                        .get_ref()
                        .and_then(|error| error.downcast_ref::<StorageError>()),
                    Some(&StorageError::Unavailable),
                    "{name} {reason}: {error}"
                );
                assert_eq!(fs::read(&retained_raw).unwrap(), original_native);
            } else {
                let expected = match open_admitted(&path, &source) {
                    Err(error) => error,
                    Ok(_) => panic!("{name} unexpectedly admitted"),
                };
                assert_eq!(error.to_string(), expected.to_string(), "{name}");
            }
            unchanged();
            let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            assert_eq!(snapshot(&db), original_state, "{name} schema and rows");
            if name != "missing-reported-runs" {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM reported_runs", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    1
                );
            }
            assert_eq!(
                db.query_row("SELECT metadata FROM session_history_catalog", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
                "{\"custody\":\"retain exact bytes\"}"
            );
            drop(db);
            unchanged();
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn unsupported_store_preserves_live_wal_and_retained_facts() {
        let root = scratch("unsupported-live-wal");
        let path = root.join("index.sqlite3");
        let raw = root.join("native.jsonl");
        fs::write(&raw, b"authoritative native bytes\n").unwrap();
        let source = crate::storage::HostSource::new(fixture_namespace(), "a".repeat(64)).unwrap();
        let mut cache =
            super::super::EventCache::open_scoped(&path.with_extension("json"), &source).unwrap();
        super::super::scan_file(&raw, "claude", &mut cache, &mut false).unwrap();
        drop(cache);
        let (mut store, _) = opened(&path);
        store.connection = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(store.owner.config()).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        store.write_runs(&[run()], &stamp()).unwrap();
        store
            .connection
            .execute(
                "INSERT INTO session_history_catalog(namespace,session_key,metadata) VALUES('fixture-source','retained','{\"custody\":\"retained custody\"}')",
                [],
            )
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO meta VALUES('fixture-source','catalog_recovery_reason','independent authority')",
                [],
            )
            .unwrap();
        store
            .connection
            .pragma_update(None, "user_version", SCHEMA_VERSION - 1)
            .unwrap();
        let paths = [
            path.clone(),
            sibling(&path, "-wal"),
            sibling(&path, "-shm"),
            raw,
        ];
        let state = catalog_state(&store.connection);
        let before = paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let source = crate::storage::HostSource::new(fixture_namespace(), "a".repeat(64)).unwrap();
        for _ in 0..2 {
            assert!(open_admitted(&path, &source).is_err());
            for (path, bytes) in paths.iter().zip(&before) {
                let actual = fs::read(path).unwrap();
                assert_eq!(Sha256::digest(&actual), Sha256::digest(bytes));
                assert_eq!(&actual, bytes);
            }
        }
        assert!(open_admitted(&path, &source).is_err());
        assert!(
            fixture_reader_binding(&path)
                .and_then(|binding| focused_connection(&binding))
                .is_err()
        );
        for (path, bytes) in paths.iter().zip(&before) {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
        assert_eq!(catalog_state(&store.connection), state);
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM reported_runs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT metadata FROM session_history_catalog", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "{\"custody\":\"retained custody\"}"
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT value FROM meta WHERE key='catalog_recovery_reason'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "independent authority"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_reopen_keeps_wal_reader_and_concurrent_publication() {
        let root = scratch("current-wal-reopen");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store
            .write_one("original", None, None, &ledger(1), &full())
            .unwrap();
        let reader = fixture_reader_binding(&path)
            .and_then(|binding| focused_connection(&binding))
            .unwrap();
        reader.execute_batch("BEGIN").unwrap();
        assert_eq!(
            reader
                .query_row("SELECT count(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let other_path = path.clone();
        BEFORE_STORAGE_INIT.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                std::thread::spawn(move || {
                    let (mut other, _) = opened(&other_path);
                    other
                        .write_one("concurrent", None, None, &ledger(2), &full())
                        .unwrap();
                })
                .join()
                .unwrap();
            }));
        });
        let (current, loaded) = opened(&path);
        assert_eq!(loaded.files.len(), 2);
        assert_eq!(
            reader
                .query_row("SELECT count(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        reader.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            reader
                .query_row("SELECT count(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            current
                .connection
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        drop(reader);
        drop(current);
        drop(store);
        let (_, loaded) = opened(&path);
        assert_eq!(loaded.files.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_initialization_holds_its_lock_through_the_first_wal_switch() {
        let root = scratch("first-wal-lock");
        let path = root.join("index.sqlite3");
        let other_path = path.clone();
        BEFORE_FRESH_WAL.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                let other = Connection::open(&other_path).unwrap();
                other.busy_timeout(Duration::ZERO).unwrap();
                let error = other
                    .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                    .unwrap_err();
                assert_eq!(error.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
            }));
        });
        let (store, _) = opened(&path);
        let mode: String = store
            .connection
            .query_row("PRAGMA locking_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "normal");
        assert!(current(&store.connection).unwrap());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_open_rechecks_a_concurrent_version_change_before_writes() {
        let root = scratch("concurrent-open-version");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store.connection = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(store.owner.config()).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        store.write_runs(&[run()], &stamp()).unwrap();
        drop(store);
        let original = fs::read(&path).unwrap();
        let captured = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let saved = captured.clone();
        let wal = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let saved_wal = wal.clone();
        let writer = std::rc::Rc::new(std::cell::RefCell::new(None));
        let live_writer = writer.clone();
        let changed = path.clone();
        BEFORE_STORAGE_INIT.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                let db = Connection::open(&changed).unwrap();
                db.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                    .unwrap();
                *saved.borrow_mut() = fs::read(&changed).unwrap();
                *saved_wal.borrow_mut() = fs::read(sibling(&changed, "-wal")).unwrap();
                // Keep the unsupported version committed only in the live WAL.
                *live_writer.borrow_mut() = Some(db);
            }));
        });
        let source = crate::storage::HostSource::new(fixture_namespace(), "a".repeat(64)).unwrap();
        assert!(
            open_admitted(&path, &source).is_err(),
            "the concurrently committed WAL version must be refused"
        );
        assert_eq!(*captured.borrow(), original);
        assert_eq!(fs::read(&path).unwrap(), *captured.borrow());
        assert!(!wal.borrow().is_empty());
        assert_eq!(fs::read(sibling(&path, "-wal")).unwrap(), *wal.borrow());
        assert!(!sibling(&path, ".corrupt").exists());
        drop(writer.borrow_mut().take());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_current_storage_initializes_all_columns_and_restarts_without_reseeding() {
        let root = scratch("current-cold-start");
        let path = root.join("index.sqlite3");
        assert!(!path.exists());
        let (mut store, loaded) = opened(&path);
        store.connection = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(store.owner.config()).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        assert!(loaded.files.is_empty() && loaded.reported_runs.is_empty());
        for (table, column) in [
            ("files", "codex_pricing_fingerprint"),
            ("session_catalog", "pricing_fingerprint"),
        ] {
            assert!(
                store
                    .connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
                        params![table, column],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
            );
        }
        let default: String = store.connection.query_row("SELECT dflt_value FROM pragma_table_info('native_usage_sources') WHERE name='usage_ready'", [], |r| r.get(0)).unwrap();
        assert_eq!(default, "0");
        let retired: i64 = store.connection.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('native_usage_migrations','native_tree_copy_queue','session_search_postings')", [], |r| r.get(0)).unwrap();
        assert_eq!(retired, 0);
        store
            .write_one("current", None, None, &ledger(1), &full())
            .unwrap();
        store.write_runs(&[run()], &stamp()).unwrap();
        drop(store);
        let (store, loaded) = opened(&path);
        require_current(&store.connection).unwrap();
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.reported_runs, [run()]);
        assert_eq!(loaded.stamp, Some(stamp()));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_index_with_every_field_set_round_trips() {
        let root = scratch("round-trip");
        let path = root.join("index.sqlite3");
        let (mut store, loaded) = opened(&path);
        assert!(loaded.files.is_empty());
        let index = full();
        assert_eq!(
            store
                .write_one("a.jsonl", None, None, &ledger(1), &index)
                .unwrap(),
            Outcome::Written
        );
        let (read_ledger, read) = store.read_one("a.jsonl").unwrap().unwrap();
        assert_eq!(read_ledger, ledger(1));
        assert_eq!(format!("{read:?}"), format!("{index:?}"));
        assert!(store.read_one("b.jsonl").unwrap().is_none());

        // A new connection loads the same.
        drop(store);
        let (_, loaded) = opened(&path);
        assert_eq!(
            format!("{:?}", loaded.files),
            format!("{:?}", [("a.jsonl".to_owned(), ledger(1), index)])
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_grown_index_writes_what_changed_and_a_moved_ledger_writes_nothing() {
        let root = scratch("grown");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        let base = full();
        store
            .write_one("a.jsonl", None, None, &ledger(1), &base)
            .unwrap();

        let mut grown = base.clone();
        // Changed in place, appended, shrunk and dropped rows.
        grown.events[0].r = None;
        grown.events[0].code_mode_open = false;
        grown.events.push(Event {
            k: Kind::A,
            o: 900,
            ..Event::default()
        });
        grown.extras.clear();
        grown.signals.truncate(1);
        grown.busy = vec![(1, 2)];
        grown.usage_by_id.remove("msg-2");
        grown
            .usage_by_id
            .insert("msg-3".to_owned(), MessageUsage::default());
        grown.codex_usage_events.push(CodexUsageEvent {
            model: "gpt-test".to_owned(),
            tokens: ModelTokens::default(),
            timestamp: None,
        });
        grown.rate_limits = None;
        assert_eq!(
            store
                .write_one("a.jsonl", Some(&ledger(1)), Some(&base), &ledger(2), &grown)
                .unwrap(),
            Outcome::Written
        );
        let (read_ledger, read) = store.read_one("a.jsonl").unwrap().unwrap();
        assert_eq!(read_ledger, ledger(2));
        assert_eq!(format!("{read:?}"), format!("{grown:?}"));

        // Based on a ledger another writer moved on: refused, untouched.
        assert_eq!(
            store
                .write_one("a.jsonl", Some(&ledger(1)), Some(&base), &ledger(3), &base)
                .unwrap(),
            Outcome::Conflict
        );
        assert_eq!(
            store
                .write_one("new.jsonl", Some(&ledger(1)), None, &ledger(3), &base)
                .unwrap(),
            Outcome::Conflict,
            "a file with no row isn't at any ledger"
        );
        let (read_ledger, read) = store.read_one("a.jsonl").unwrap().unwrap();
        assert_eq!(read_ledger, ledger(2));
        assert_eq!(format!("{read:?}"), format!("{grown:?}"));

        // Without a base, the rows are replaced.
        assert_eq!(
            store
                .write_one("a.jsonl", Some(&ledger(2)), None, &ledger(3), &base)
                .unwrap(),
            Outcome::Written
        );
        let (_, read) = store.read_one("a.jsonl").unwrap().unwrap();
        assert_eq!(format!("{read:?}"), format!("{base:?}"));

        // A removal based on a ledger the row has moved past keeps it.
        store
            .remove(&[("a.jsonl".to_owned(), Some(ledger(2)))])
            .unwrap();
        assert!(store.read_one("a.jsonl").unwrap().is_some());
        store
            .remove(&[("a.jsonl".to_owned(), Some(ledger(3)))])
            .unwrap();
        assert!(store.read_one("a.jsonl").unwrap().is_none());
        for table in ["events", "signals", "usage", "codex_usage", "files"] {
            let rows: i64 = store
                .connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 0, "{table}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_row_that_does_not_decode_leaves_its_file_out() {
        let root = scratch("undecodable");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store
            .write_one("a.jsonl", None, None, &ledger(1), &full())
            .unwrap();
        store
            .write_one("b.jsonl", None, None, &ledger(2), &full())
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE events SET k = 'no-such-kind' WHERE file_id = \
                 (SELECT file_id FROM files WHERE path = 'a.jsonl')",
                [],
            )
            .unwrap();
        assert!(matches!(
            IndexStore::load_file(&store, "a.jsonl"),
            Err(StoreError::Data(_))
        ));
        drop(store);
        let (_, loaded) = opened(&path);
        let paths: Vec<&str> = loaded
            .files
            .iter()
            .map(|(path, _, _)| path.as_str())
            .collect();
        assert_eq!(paths, ["b.jsonl"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_file_row_codec_matches_persisted_dimensions_through_append_and_truncation() {
        use crate::events::file_rows::FileRow;

        let root = scratch("file-row-codec");
        let db = root.join("index.sqlite3");
        let (mut store, _) = opened(&db);
        let mut cold = full();
        cold.entrypoint = Some("entry\0中".into());
        cold.title = None;
        cold.agent_name = Some("agent é e\u{301}".into());
        cold.last_model = None;
        let first = ledger(1);
        assert_eq!(
            store
                .write_one("file.jsonl", None, None, &first, &cold)
                .unwrap(),
            Outcome::Written
        );

        let mut appended = cold.clone();
        appended.events.push(Event {
            k: Kind::A,
            o: 900,
            ..Event::default()
        });
        appended.title = Some("append \0 😀".into());
        let second = ledger(2);
        assert_eq!(
            store
                .write_one("file.jsonl", Some(&first), Some(&cold), &second, &appended)
                .unwrap(),
            Outcome::Written
        );

        let mut truncated = appended.clone();
        truncated.events.truncate(1);
        truncated.title = None;
        let third = ledger(3);
        assert_eq!(
            store
                .write_one(
                    "file.jsonl",
                    Some(&second),
                    Some(&appended),
                    &third,
                    &truncated,
                )
                .unwrap(),
            Outcome::Written
        );

        let row = FileRow::new(&third, &truncated);
        let persisted: (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
        ) = store
            .connection
            .query_row(
                "SELECT entrypoint,title,agent_name,last_model,failed FROM files WHERE path=?1",
                ["file.jsonl"],
                |record| {
                    Ok((
                        record.get(0)?,
                        record.get(1)?,
                        record.get(2)?,
                        record.get(3)?,
                        record.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(persisted.0.as_ref(), row.entrypoint.as_ref());
        assert_eq!(persisted.1.as_ref(), row.title.as_ref());
        assert_eq!(persisted.2.as_ref(), row.agent_name.as_ref());
        assert_eq!(persisted.3.as_ref(), row.last_model.as_ref());
        assert_eq!(persisted.4 != 0, row.failed);
        let (read_ledger, read) = store.read_one("file.jsonl").unwrap().unwrap();
        assert_eq!(read_ledger, third);
        assert_eq!(format!("{read:?}"), format!("{truncated:?}"));

        drop(store);
        let (store, _) = opened(&db);
        let (read_ledger, read) = store.read_one("file.jsonl").unwrap().unwrap();
        assert_eq!(read_ledger, third);
        assert_eq!(format!("{read:?}"), format!("{truncated:?}"));
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_tree_rows_canonical_witness_matches_persisted_dimensions() {
        use crate::events::tree_rows::{TreeRows, witness};
        let root = scratch("tree-rows-witness");
        let db = root.join("index.sqlite3");
        let (mut store, _) = opened(&db);
        let mut expected_witnesses = Vec::new();
        let adversarial = format!(
            "quote\" slash\\ / NUL\0 namespace\0suffix {} é e\u{301} 中 😀 \u{2028}\u{2029}",
            (0..=31).map(char::from).collect::<String>()
        );
        let summaries = [
            crate::native_tree::Summary::default(),
            crate::native_tree::Summary {
                models: ["old".into(), "new".into()].into_iter().collect(),
                tools: [("tree-tool".into(), "Bash".into())].into_iter().collect(),
                ..Default::default()
            },
            crate::native_tree::Summary {
                first: Some(adversarial.clone()),
                last: Some(String::new()),
                cwd: Some("/cwd\0tail".into()),
                branch: Some("branch\n\t".into()),
                models: [adversarial.clone(), String::new(), "é".into()]
                    .into_iter()
                    .collect(),
                tools: [
                    (adversarial.clone(), adversarial.clone()),
                    ("closed".into(), "retained name".into()),
                ]
                .into_iter()
                .collect(),
                closed_tools: ["closed".into(), "closed without name".into()]
                    .into_iter()
                    .collect(),
                usage_models: [("message\0id".into(), adversarial.clone())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        ];
        for (number, summary) in summaries.into_iter().enumerate() {
            let path = format!("tree-{number}.jsonl");
            let ledger = ledger(1);
            let index = FileIndex {
                tree_summary: Some(summary.clone()),
                source_chunks: Some(std::sync::Arc::new(crate::source_chunks::Chunks {
                    offset: 3,
                    hashes: vec![[1; 32]],
                    resume: None,
                })),
                marker_index: Some(crate::native_marker::Index::default()),
                handoff_index: Some(crate::native_handoff::Index::default()),
                ..Default::default()
            };
            assert_eq!(
                store.write_one(&path, None, None, &ledger, &index).unwrap(),
                Outcome::Written
            );
            let rows = TreeRows::new(&summary).unwrap();
            let mut labels: std::collections::BTreeMap<_, _> = rows.labels().collect();
            labels.insert("codex_turn", &rows.codex_turn);
            let expected = witness::digest(
                labels,
                rows.models.iter().map(String::as_str),
                rows.tools
                    .iter()
                    .map(|(id, name)| {
                        (
                            id.as_str(),
                            Some(name.as_str()),
                            rows.closed_tools.contains(id),
                        )
                    })
                    .chain(
                        rows.closed_tools
                            .iter()
                            .filter(|id| !rows.tools.contains_key(*id))
                            .map(|id| (id.as_str(), None, true)),
                    ),
            );
            assert_eq!(
                tree_state::witness(&store.connection, &path, &ledger).unwrap(),
                Some(expected.clone())
            );
            expected_witnesses.push((path.clone(), ledger.clone(), expected));
            let (_, restored) = store.read_tree(&path, &ledger.stat).unwrap().unwrap();
            assert_eq!(restored.summary.first, summary.first);
            assert_eq!(restored.summary.last, summary.last);
            assert_eq!(restored.summary.cwd, summary.cwd);
            assert_eq!(restored.summary.branch, summary.branch);
            assert_eq!(restored.summary.models, summary.models);
            assert_eq!(
                restored.summary.tools,
                summary
                    .tools
                    .into_iter()
                    .filter(|(id, _)| !summary.closed_tools.contains(id))
                    .collect()
            );
        }
        drop(store);
        let (reopened, _) = opened(&db);
        for (path, ledger, expected) in expected_witnesses {
            assert_eq!(
                tree_state::witness(&reopened.connection, &path, &ledger).unwrap(),
                Some(expected)
            );
        }
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
        tree_rows_json_array_encoding_matches_sqlite_edge_values();
        tree_metadata_and_evidence_share_one_snapshot_during_concurrent_commit();
    }

    #[test]
    fn recovery_scalar119_sqlite_current_record_refusal_preserves_bytes() {
        const SCOPED_VALUE: &str =
            "SELECT value FROM meta WHERE namespace=semon_meta_scope(?1) AND key=?1";
        const SCOPED_UPDATE: &str =
            "UPDATE meta SET value=?2 WHERE namespace=semon_meta_scope(?1) AND key=?1";
        let root = scratch("strict-tree-scalar");
        let path = root.join("one.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fixture\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"id\":\"m\",\"role\":\"assistant\",\"model\":\"claude\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n",
                "not-json\n"
            ),
        )
        .unwrap();
        let cache_path = root.join("index.json");
        let mut cache =
            crate::events::EventCache::open(&cache_path, &crate::Options::default().source)
                .unwrap();
        let index = crate::events::scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        cache.flush_cold_writes().unwrap();
        drop(cache);
        // The real current factory admitted this store; write through its
        // bound source-journal writer connection.
        let config = crate::Options::default()
            .source
            .config(&cache_path)
            .unwrap();
        let connection = crate::storage::sqlite::source_journal::SourceJournal(config)
            .connection()
            .unwrap();
        let path = path.to_str().unwrap();
        let (_, ledger) = ledger_row(&connection, path).unwrap().unwrap();
        let key = tree_summary_key(path);
        let original: String = connection
            .query_row(SCOPED_VALUE, [&key], |r| r.get(0))
            .unwrap();
        let base: serde_json::Value = serde_json::from_str(&original).unwrap();
        let read = || {
            let mut statement = connection
                .prepare(native_metadata_value(&connection).unwrap())
                .unwrap();
            read_tree_summary(&connection, &mut statement, path, &ledger).unwrap()
        };
        let actual = read().unwrap();
        let expected = index.tree_summary.as_ref().unwrap();
        assert!(expected.line_count > 0);
        assert!(expected.malformed_lines > 0);
        assert_eq!(
            json(&actual.compact(false)).unwrap(),
            json(&expected.compact(false)).unwrap()
        );
        let mut invalid = vec!["{".to_owned(), " ".repeat(1048577)];
        for field in [
            "version",
            "parser_version",
            "ledger",
            "projection_witness",
            "interpretation_witness",
        ] {
            let mut changed = base.clone();
            changed[field] = serde_json::Value::Null;
            invalid.push(changed.to_string());
        }
        let mut unsupported = base;
        unsupported["version"] = serde_json::json!(2);
        invalid.push(unsupported.to_string());
        for encoded in invalid {
            connection
                .execute(SCOPED_UPDATE, params![key, encoded])
                .unwrap();
            assert!(read().is_none());
            let after: String = connection
                .query_row(SCOPED_VALUE, [&key], |r| r.get(0))
                .unwrap();
            assert_eq!(
                after, encoded,
                "refusal must leave unsupported bytes untouched"
            );
        }
        connection
            .execute(SCOPED_UPDATE, params![key, original])
            .unwrap();
        let failure = format!("native_tree_failure:{path}");
        connection
            .execute(
                "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,'failed')",
                [&failure],
            )
            .unwrap();
        assert!(read().is_none());
        connection
            .execute(
                "DELETE FROM meta WHERE namespace=semon_meta_scope(?1) AND key=?1",
                [&failure],
            )
            .unwrap();
        assert_eq!(read().unwrap().line_count, expected.line_count);
        drop(connection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tree_rows_json_array_encoding_matches_sqlite_edge_values() {
        let connection = Connection::open_in_memory().unwrap();
        let strings = [
            String::new(),
            "\"\\/\0é中😀\u{2028}\u{2029}".into(),
            (0..=31).map(char::from).collect::<String>(),
        ];
        for value in strings {
            let actual: String = connection
                .query_row(
                    "SELECT json_array(?1,NULL,?2,?3)",
                    params![value, i64::MIN, i64::MAX],
                    |row| row.get(0),
                )
                .unwrap();
            let expected =
                serde_json::to_string(&serde_json::json!([value, null, i64::MIN, i64::MAX]))
                    .unwrap();
            assert_eq!(actual.as_bytes(), expected.as_bytes());
        }
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    #[test]
    fn interrupted_summary_batch_publishes_no_partial_generation() {
        let path = std::env::temp_dir().join(format!(
            "semon-summary-rollback-{}.sqlite3",
            std::process::id()
        ));
        let admitted = current_test_store(&path, &fixture_namespace());
        let config = admitted.owner.config().clone();
        drop(admitted);
        let mut store = crate::storage::sqlite::configured_index::open_semantic(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap();
        let db = configured::connection(&config).unwrap();
        let rows = vec![("first".into(), 1, "inputs".into(), "old".into())];
        store.save_session_descriptions(&rows).unwrap();
        let reads = vec![
            SessionDescriptionRequest {
                key: "first".into(),
                version: 1,
                inputs: "inputs".into(),
            },
            SessionDescriptionRequest {
                key: "second".into(),
                version: 1,
                inputs: "inputs".into(),
            },
            SessionDescriptionRequest {
                key: "first".into(),
                version: 2,
                inputs: "inputs".into(),
            },
            SessionDescriptionRequest {
                key: "first".into(),
                version: 1,
                inputs: "other".into(),
            },
        ];
        assert_eq!(
            store.session_descriptions(&reads).unwrap(),
            vec![Some("old".into()), None, None, None]
        );
        let oversized = vec![reads[0].clone(); SESSION_DESCRIPTION_BATCH_LIMIT + 1];
        assert!(matches!(
            store.session_descriptions(&oversized),
            Err(StoreError::Configured(StorageError::InvalidArguments))
        ));
        db.execute_batch("CREATE TRIGGER interrupt_summary BEFORE INSERT ON main.session_descriptions WHEN NEW.namespace=semon_namespace() AND NEW.session_key = 'second' BEGIN SELECT RAISE(ABORT, 'interrupted'); END;").unwrap();
        assert_eq!(
            store.save_session_descriptions(&rows).unwrap(),
            Outcome::Written
        );
        let batch = vec![
            ("first".into(), 1, "inputs".into(), "new".into()),
            ("second".into(), 1, "inputs".into(), "new".into()),
        ];
        assert!(store.save_session_descriptions(&batch).is_err());
        assert_eq!(
            store.session_descriptions(&reads).unwrap(),
            vec![Some("old".into()), None, None, None]
        );
        drop(store);
        db.execute_batch("DROP TRIGGER interrupt_summary").unwrap();
        drop(db);
        let store = crate::storage::sqlite::configured_index::open_semantic(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap();
        assert_eq!(
            store.session_descriptions(&reads).unwrap(),
            vec![Some("old".into()), None, None, None]
        );
        drop(store);
        fs::remove_file(path).unwrap();
    }
}

// Frozen pre-extraction slot publisher, used only by the full-Catalog CI oracle.

pub(crate) fn read_session_slots(
    connection: &Connection,
    row: &crate::model::summary::CatalogRow,
    expected: &str,
    max: usize,
) -> Result<Option<Vec<crate::slot_projection::SavedSlot>>, StoreError> {
    let coherent: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE namespace=semon_meta_scope('catalog_generation') AND key='catalog_generation' AND value=?1) AND EXISTS(SELECT 1 FROM meta WHERE namespace=semon_meta_scope('slot_projection_catalog_generation') AND key='slot_projection_catalog_generation' AND value=?1)",
                [expected], |record| record.get(0),
            ).map_err(failure)?;
    if !coherent {
        return Ok(None);
    }
    let header: Option<(String, u32, i64, Option<String>)> = connection.query_row(
                "SELECT generation,version,total,source_generation FROM session_slot_projections WHERE namespace=semon_namespace() AND session_key=?1",
                [&row.key], |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?, record.get(3)?)),
            ).optional().map_err(failure)?;
    let Some((generation, version, total, sources)) = header else {
        return Ok(None);
    };
    let Ok(total) = usize::try_from(total) else {
        return Ok(None);
    };
    let source_generation = crate::retention::source_generation(&row.sources)
        .map_err(|error| StoreError::Data(error.to_string()))?;
    if version != crate::slot_projection::VERSION
        || total > max
        || sources.as_deref() != Some(&source_generation)
    {
        return Ok(None);
    }
    let mut statement = connection.prepare(
                "SELECT slot,CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_slots WHERE namespace=semon_namespace() AND session_key=?1 ORDER BY slot",
            ).map_err(failure)?;
    let rows = statement
        .query_map([&row.key], |record| {
            Ok((record.get::<_, i64>(0)?, record.get::<_, String>(1)?))
        })
        .map_err(failure)?;
    let mut metadata = Vec::with_capacity(total);
    for record in rows {
        let (slot, text) = record.map_err(failure)?;
        if usize::try_from(slot).ok() != Some(metadata.len()) || metadata.len() >= total {
            return Ok(None);
        }
        metadata.push(text);
    }
    if metadata.len() != total
        || crate::slot_projection::generation(&metadata, &row.sources)
            .map_err(|error| StoreError::Data(error.to_string()))?
            != generation
    {
        return Ok(None);
    }
    metadata
        .into_iter()
        .map(|text| {
            serde_json::from_str(&text).map_err(|error| StoreError::Data(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

pub(crate) fn read_file_ledgers(
    connection: &Connection,
) -> Result<Vec<(String, Option<Ledger>)>, StoreError> {
    let mut statement=connection.prepare("SELECT file_id,dev,ino,size,mtime_ns,resume_at,head_sha256,tail_sha256,ctime_ns,path FROM files WHERE namespace=semon_namespace() ORDER BY file_id").map_err(failure)?;
    let mut rows = statement.query([]).map_err(failure)?;
    let mut ledgers = Vec::new();
    while let Some(row) = rows.next().map_err(failure)? {
        let path = row.get(9).map_err(failure)?;
        let ledger = match read_ledger(row) {
            Ok((_, ledger)) => Some(ledger),
            Err(error) if is_data_error(&error) => None,
            Err(error) => return Err(failure(error)),
        };
        ledgers.push((path, ledger));
    }
    Ok(ledgers)
}

pub(crate) fn remove_files_tx(
    transaction: &Transaction<'_>,
    files: &[(String, Option<Ledger>)],
    namespace: &str,
) -> rusqlite::Result<()> {
    validate_copied_usage(transaction)?;
    for (path, expected) in files {
        match (ledger_row(transaction, path), expected) {
            (Ok(Some((_, found))), Some(expected)) if found == *expected => {
                if archive_ledger_retained(transaction, path, &found)? {
                    // Explicit archive retention does not make this source
                    // current. Complete catalog observation still retires
                    // its membership; only the existing event rows survive.
                    // Re-observing this exact retained generation after its
                    // withdrawal must not advance the stage's own claim.
                    // A fresh announced revision or changed reason remains
                    // visible; neither the claim nor its proof is replaced.
                    let already_removed:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1 AND reason='removed')",[path],|row|row.get(0))?;
                    if !already_removed {
                        crate::storage::sqlite::source_journal::invalidate(
                            transaction,
                            namespace,
                            path,
                            "removed",
                        )?;
                    }
                    continue;
                }
            }
            // Moved on by another process, rewritten, or already gone.
            (Ok(_), _) => continue,
            (Err(error), _) if is_data_error(&error) => {}
            (Err(error), _) => return Err(error),
        }
        // By path, never decoding the row: an undecodable one goes too.
        for table in [
            "events",
            "signals",
            "usage_identity",
            "usage",
            "codex_usage",
            "native_tree_copied_identities",
        ] {
            transaction.execute(
                &format!(
                    "DELETE FROM {table} WHERE {}file_id IN \
                         (SELECT file_id FROM files WHERE {}path = ?1)",
                    "namespace=semon_namespace() AND ", "namespace=semon_namespace() AND "
                ),
                [path],
            )?;
        }
        transaction.execute(
            "DELETE FROM native_tree_headers WHERE namespace=semon_namespace() AND source_path=?1",
            [path],
        )?;
        transaction.execute(
            "DELETE FROM files WHERE namespace=semon_namespace() AND path=?1",
            [path],
        )?;
        transaction.execute("DELETE FROM catalog_native_dependencies WHERE namespace=semon_namespace() AND source_path=?1 AND harness IN ('semantic-native:codex','semantic-parent:codex')", [path])?;
        transaction.execute(
            native_metadata_delete(transaction)?,
            [archive_index_key(path)],
        )?;
        transaction.execute(
            native_metadata_delete(transaction)?,
            [codex_header_key(path)],
        )?;
        transaction.execute(
            native_metadata_delete(transaction)?,
            [claude_sidecar_key(path)],
        )?;
        chunk_proofs::clear(transaction, path)?;
        launch_calls::clear(transaction, path)?;
        transaction.execute(
            native_metadata_delete(transaction)?,
            [parent_marker_key(path)],
        )?;
        transaction.execute(
            native_metadata_delete(transaction)?,
            [tree_summary_key(path)],
        )?;
        crate::storage::sqlite::source_journal::invalidate(
            transaction,
            namespace,
            path,
            "removed",
        )?;
    }
    Ok(())
}

pub(crate) fn read_catalog_component_paths(
    connection: &Connection,
    paths: &[String],
    keys: &[String],
    native_parents: &[(String, String)],
) -> Result<Option<Vec<String>>, StoreError> {
    if paths.is_empty() || paths.len() > 64 || keys.len() > 64 || native_parents.len() > 64 {
        return Ok(None);
    }
    let mut pending: std::collections::BTreeSet<_> = keys.iter().cloned().collect();
    for (harness, id) in native_parents {
        let mut statement = connection.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 LIMIT 65").map_err(failure)?;
        let owners = statement
            .query_map(params![harness, id], |row| row.get::<_, String>(0))
            .map_err(failure)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failure)?;
        if owners.len() > 64 {
            return Ok(None);
        }
        pending.extend(owners);
    }
    for path in paths {
        let mut statement = connection.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE source_path=?1 AND lifecycle='current' LIMIT 65").map_err(failure)?;
        let owners = statement
            .query_map([path], |row| row.get::<_, String>(0))
            .map_err(failure)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failure)?;
        if owners.len() > 64 {
            return Ok(None);
        }
        pending.extend(owners);
    }
    let mut visited = std::collections::BTreeSet::new();
    let mut sources = std::collections::BTreeSet::new();
    for path in paths {
        let mut statement=connection.prepare("SELECT new_path FROM catalog_relocation_candidates WHERE namespace=semon_namespace() AND old_path=?1 UNION SELECT old_path FROM catalog_relocation_candidates WHERE namespace=semon_namespace() AND new_path=?1 LIMIT 65").map_err(failure)?;
        sources.extend(
            statement
                .query_map([path], |row| row.get::<_, String>(0))
                .map_err(failure)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?,
        );
        if sources.len() > 64 {
            return Ok(None);
        }
    }
    while let Some(key) = pending.pop_first() {
        if !visited.insert(key.clone()) {
            continue;
        }
        if visited.len() > 64 {
            return Ok(None);
        }
        let Some(row) = current_catalog_row(connection, &key).map_err(failure)? else {
            continue;
        };
        if row.native_ids.len() > 64 || row.sources.len() > 64 {
            return Ok(None);
        }
        sources.extend(
            row.sources
                .iter()
                .map(|source| source.path.to_string_lossy().into_owned()),
        );
        if sources.len() > 64 {
            return Ok(None);
        }
        let mut related = connection.prepare("SELECT endpoint FROM (SELECT parent_key AS endpoint FROM session_catalog WHERE session_key=?1 AND lifecycle='current' UNION SELECT session_key FROM session_catalog WHERE parent_key=?1 AND lifecycle='current' UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM session_trace_edges WHERE session_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM session_trace_edges WHERE session_key=?1 UNION SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?1 UNION SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?1) WHERE endpoint IS NOT NULL AND endpoint!='you' ORDER BY endpoint LIMIT 65").map_err(failure)?;
        let endpoints = related
            .query_map([&key], |row| row.get::<_, String>(0))
            .map_err(failure)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failure)?;
        if endpoints.len() > 64 {
            return Ok(None);
        }
        pending.extend(
            endpoints
                .into_iter()
                .filter(|endpoint| !visited.contains(endpoint)),
        );
        for id in &row.native_ids {
            let mut aliases = connection.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 LIMIT 65").map_err(failure)?;
            let aliases = aliases
                .query_map(params![row.harness, id], |row| row.get::<_, String>(0))
                .map_err(failure)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?;
            if aliases.len() > 64 {
                return Ok(None);
            }
            pending.extend(
                aliases
                    .into_iter()
                    .filter(|endpoint| !visited.contains(endpoint)),
            );
        }
        if visited.len() + pending.len() > 64 {
            return Ok(None);
        }
    }
    Ok(Some(sources.into_iter().collect()))
}

pub(crate) fn qualify_catalog_tx(
    tx: &Transaction<'_>,
    namespace: &str,
) -> Result<bool, StoreError> {
    let repairs = {
        let mut statement=tx.prepare("SELECT source_path FROM session_catalog_sources WHERE namespace=semon_namespace() AND lifecycle='current' AND session_key IN(SELECT session_key FROM session_slot_projections WHERE namespace=semon_namespace() AND version=?1 AND source_generation IS NULL LIMIT 64) LIMIT 64").map_err(failure)?;
        statement
            .query_map([crate::slot_projection::VERSION], |row| {
                row.get::<_, String>(0)
            })
            .map_err(failure)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failure)?
    };
    for path in repairs {
        if !semantic_stage::owns_header_repair(tx, &path).map_err(failure)? {
            crate::storage::sqlite::source_journal::invalidate(tx, namespace, &path, "changed")
                .map_err(failure)?;
        }
    }

    tx.query_row("SELECT (SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_version') AND key='catalog_version')=?1 AND (SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_completeness') AND key='catalog_completeness')='complete' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_stage WHERE namespace=semon_namespace()) AND (SELECT value FROM meta WHERE namespace=semon_meta_scope('catalog_generation') AND key='catalog_generation')=(SELECT value FROM meta WHERE namespace=semon_meta_scope('slot_projection_catalog_generation') AND key='slot_projection_catalog_generation')", [crate::model::summary::CATALOG_VERSION.to_string()], |row| row.get::<_,Option<bool>>(0)).map(|value| value.unwrap_or(false)).map_err(failure)
}

#[cfg(test)]
pub(crate) fn current_search_test_connection(
    path: &Path,
    namespace: &crate::storage::LogicalSourceNamespace,
) -> Connection {
    current_search_test_connection_with_epoch(path, namespace, &"a".repeat(64))
}

#[cfg(test)]
pub(crate) fn current_search_test_connection_with_epoch(
    path: &Path,
    namespace: &crate::storage::LogicalSourceNamespace,
    stream_epoch: &str,
) -> Connection {
    let mut store = current_test_store_with_epoch(path, namespace, stream_epoch);
    store
        .publish_catalog_with_generation(
            &crate::slot_projection::Publication {
                rows: &[],
                transcripts: None,
                observations: None,
            },
            None,
            None,
            false,
            None,
        )
        .unwrap();
    store.connection
}

#[cfg(test)]
fn current_test_store(
    path: &Path,
    namespace: &crate::storage::LogicalSourceNamespace,
) -> SqliteStore {
    current_test_store_with_epoch(path, namespace, &"a".repeat(64))
}

#[cfg(test)]
fn current_test_store_with_epoch(
    path: &Path,
    namespace: &crate::storage::LogicalSourceNamespace,
    stream_epoch: &str,
) -> SqliteStore {
    let config = crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Sqlite {
            database: path.to_owned(),
        },
        namespace: namespace.clone(),
        expected_stream_epoch: Some(stream_epoch.to_owned()),
    };
    let source =
        crate::storage::HostSource::new(namespace.clone(), stream_epoch.to_owned()).unwrap();
    drop(super::EventCache::open_scoped(&path.with_extension("json"), &source).unwrap());
    let owner = configured::NativeOwnership::restore(&config).unwrap();
    let connection = configured::connection(&config).unwrap();
    crate::storage::NativeReaderBinding::Semantic(owner.clone())
        .validate(&connection, namespace)
        .unwrap();
    SqliteStore {
        owner,
        connection,
        path: path.to_owned(),
        selected_runs: None,
        semantic_rejection: None,
        traversal_observation: None,
    }
}

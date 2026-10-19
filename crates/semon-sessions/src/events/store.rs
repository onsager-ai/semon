//! The event index persisted in SQLite: `sessions-index.sqlite3`, beside the
//! V1 metadata cache (step a1 of `docs/design/derived-store.md`).
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
mod launch_calls;
pub(crate) mod native_usage;
mod relocation;
mod semantic_stage;
pub(crate) mod tree_state;

use std::{
    collections::BTreeMap,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
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

use super::{
    BillingUsage, CACHE_VERSION, CodexUsageEvent, Event, FileIndex, IndexStore, Ledger, Loaded,
    MessageUsage, ModelTokens, Outcome, Peer, Reply, ReportedFileStamp, Signal, Stat, StoreError,
    Yield,
};
use crate::{Tokens, facts::ReportedRunSnapshot};

#[cfg(test)]
std::thread_local! {
    pub(crate) static BEFORE_SOURCE_INVALIDATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// `PRAGMA user_version`: the shape of the tables. The parser's version is
/// [`CACHE_VERSION`], kept in `meta`.
pub(crate) const SCHEMA_VERSION: i64 = 30;

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

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_slot_projections (
    session_key TEXT PRIMARY KEY,
    generation TEXT NOT NULL,
    source_generation TEXT,
    version INTEGER NOT NULL,
    total INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS session_slot_projection_version ON session_slot_projections(version,session_key);
CREATE TABLE IF NOT EXISTS session_slots (
    session_key TEXT NOT NULL,
    slot INTEGER NOT NULL,
    metadata TEXT NOT NULL,
    PRIMARY KEY(session_key,slot)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS session_slots_turn ON session_slots(session_key,CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.turn') END,slot);
CREATE TABLE IF NOT EXISTS session_catalog (
    session_key TEXT PRIMARY KEY,
    lifecycle TEXT NOT NULL DEFAULT 'current' CHECK(lifecycle IN ('current','retained')),
    last_ms INTEGER NOT NULL,
    harness TEXT NOT NULL,
    repo TEXT,
    parent_key TEXT,
    metadata TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS session_catalog_order ON session_catalog(last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness ON session_catalog(harness, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_repo ON session_catalog(repo, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness_repo ON session_catalog(harness, repo, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_parent ON session_catalog(parent_key, last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_current_order ON session_catalog(last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness ON session_catalog(harness, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_repo ON session_catalog(repo, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness_repo ON session_catalog(harness, repo, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_parent ON session_catalog(parent_key, last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_order_model ON session_catalog(json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness_model ON session_catalog(harness, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_repo_model ON session_catalog(repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_harness_repo_model ON session_catalog(harness, repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC);
CREATE INDEX IF NOT EXISTS session_catalog_current_order_model ON session_catalog(json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness_model ON session_catalog(harness, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_repo_model ON session_catalog(repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_current_harness_repo_model ON session_catalog(harness, repo, json_extract(metadata,'$.model'), last_ms DESC, session_key ASC) WHERE lifecycle='current';
CREATE INDEX IF NOT EXISTS session_catalog_query_last ON session_catalog(json_extract(metadata,'$.query.last'),session_key);
CREATE INDEX IF NOT EXISTS session_catalog_query_current_state ON session_catalog(json_extract(metadata,'$.query.state'),session_key) WHERE lifecycle='current';
CREATE TABLE IF NOT EXISTS session_catalog_sources (
    session_key TEXT NOT NULL,
    lifecycle TEXT NOT NULL DEFAULT 'current' CHECK(lifecycle IN ('current','retained')),
    harness TEXT NOT NULL,
    native_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    PRIMARY KEY(session_key, source_path)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_catalog_native ON session_catalog_sources(harness, native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_native_aliases ON session_catalog_sources(native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_source_path ON session_catalog_sources(source_path, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_current_native ON session_catalog_sources(lifecycle, harness, native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_current_source_path ON session_catalog_sources(lifecycle, source_path, session_key);
CREATE TABLE IF NOT EXISTS session_catalog_invalidations (
    source_path TEXT PRIMARY KEY,
    revision INTEGER NOT NULL,
    reason TEXT NOT NULL CHECK(reason IN ('changed', 'removed')),
    observed_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS catalog_relocation_candidates (
    old_path TEXT NOT NULL, new_path TEXT NOT NULL, native_id TEXT NOT NULL,
    PRIMARY KEY(old_path,new_path)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS catalog_relocation_new ON catalog_relocation_candidates(new_path,old_path);
CREATE TABLE IF NOT EXISTS catalog_native_dependencies (
    source_path TEXT NOT NULL, harness TEXT NOT NULL, native_id TEXT NOT NULL,
    PRIMARY KEY(source_path,harness,native_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS catalog_native_dependency ON catalog_native_dependencies(harness,native_id,source_path);
CREATE TABLE IF NOT EXISTS catalog_backfill_sources (source_path TEXT PRIMARY KEY) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS session_source_mutations (
    source_path TEXT PRIMARY KEY,
    evidence TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_catalog_invalidations_revision ON session_catalog_invalidations(revision,source_path);

CREATE TABLE IF NOT EXISTS session_descriptions (
    session_key TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    inputs TEXT NOT NULL,
    description TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS files (
    file_id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    dev INTEGER NOT NULL,
    ino INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns BLOB NOT NULL,
    ctime_ns BLOB,
    resume_at INTEGER NOT NULL,
    head_sha256 BLOB NOT NULL,
    tail_sha256 BLOB NOT NULL,
    entrypoint TEXT,
    title TEXT,
    agent_name TEXT,
    last_model TEXT,
    failed INTEGER NOT NULL,
    first_ms INTEGER,
    last_ms INTEGER,
    cwd TEXT,
    branch TEXT,
    pending TEXT NOT NULL,
    tool_ids TEXT NOT NULL,
    yields TEXT NOT NULL,
    busy TEXT NOT NULL,
    links TEXT NOT NULL,
    codex_tokens TEXT NOT NULL,
    codex_tokens_by_model TEXT NOT NULL,
    rate_limits TEXT,
    codex_native_usage TEXT,
    copilot TEXT
) STRICT;
CREATE TABLE IF NOT EXISTS events (
    file_id INTEGER NOT NULL,
    extra INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    k TEXT NOT NULL,
    o INTEGER NOT NULL,
    b INTEGER NOT NULL,
    t INTEGER,
    id TEXT,
    n TEXT,
    code_mode INTEGER NOT NULL,
    code_mode_open INTEGER NOT NULL,
    parent INTEGER,
    script INTEGER NOT NULL,
    y TEXT,
    poll INTEGER,
    reply INTEGER NOT NULL,
    reply_o INTEGER,
    reply_b INTEGER,
    reply_t INTEGER,
    reply_e INTEGER,
    reply_f INTEGER,
    reply_m TEXT,
    item INTEGER,
    peer INTEGER NOT NULL,
    peer_sock TEXT,
    peer_pid INTEGER,
    peer_start TEXT,
    u TEXT,
    PRIMARY KEY (file_id, extra, seq)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS signals (
    file_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    k TEXT NOT NULL,
    o INTEGER NOT NULL,
    t INTEGER,
    at INTEGER NOT NULL,
    n TEXT,
    v INTEGER,
    PRIMARY KEY (file_id, seq)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS usage_identity (
    file_id INTEGER NOT NULL,
    message_id TEXT NOT NULL,
    record_uuid TEXT NOT NULL,
    PRIMARY KEY(file_id, message_id, record_uuid)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS usage_identity_peers ON usage_identity(message_id, record_uuid, file_id);
CREATE TABLE IF NOT EXISTS usage (
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
    PRIMARY KEY (file_id, message_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS codex_usage (
    file_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    model TEXT NOT NULL,
    input INTEGER NOT NULL,
    output INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    t INTEGER,
    PRIMARY KEY (file_id, seq)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS files_cwd ON files(cwd,path);
CREATE TABLE IF NOT EXISTS reported_runs (
    session_id TEXT NOT NULL,
    start INTEGER NOT NULL,
    cost REAL,
    duration INTEGER,
    api_duration INTEGER,
    tool_duration INTEGER,
    lines_added INTEGER,
    lines_removed INTEGER,
    model_usage TEXT NOT NULL,
    capture_at INTEGER NOT NULL,
    PRIMARY KEY (session_id, start)
) STRICT, WITHOUT ROWID;
";

/// The tables derived from the logs: emptied when the parser's version
/// changes. `reported_runs` isn't among them: its source keeps only the
/// last run, so it can't be rebuilt.
const DERIVED: &str = "
DELETE FROM session_catalog_invalidations;
DELETE FROM session_source_mutations;
DELETE FROM catalog_backfill_sources;
DELETE FROM catalog_relocation_candidates;
DELETE FROM catalog_native_dependencies;
DELETE FROM catalog_semantic_stage;
DELETE FROM catalog_semantic_frontier;
DELETE FROM catalog_semantic_sources;
DELETE FROM catalog_semantic_inputs;
DELETE FROM catalog_semantic_keys;
DELETE FROM catalog_semantic_endpoints;
DELETE FROM catalog_semantic_native;
DELETE FROM catalog_semantic_owner_sources;
DELETE FROM catalog_semantic_paths;
DELETE FROM catalog_semantic_relations;
DELETE FROM catalog_semantic_events;
DELETE FROM catalog_semantic_names;
DELETE FROM meta WHERE key IN ('catalog_semantic_gc','catalog_backfill_state','catalog_recovery_reason','catalog_recovery_after','catalog_legacy_runtime_anchor','catalog_legacy_runtime_semantics');
UPDATE session_catalog SET lifecycle='retained',metadata=CASE WHEN json_valid(metadata) THEN json_set(metadata,'$.lifecycle','retained') ELSE metadata END;
UPDATE session_catalog_sources SET lifecycle='retained';
DELETE FROM meta WHERE key IN ('catalog_version', 'catalog_generation', 'catalog_observed_at', 'catalog_completeness', 'catalog_publication_epoch');
DELETE FROM session_descriptions;
DELETE FROM events;
DELETE FROM signals;
DELETE FROM usage_identity;
DELETE FROM usage;
DELETE FROM codex_usage;
DELETE FROM native_tree_headers;
DELETE FROM native_tree_generations;
DELETE FROM native_tree_usage_work;
DELETE FROM native_tree_codex_work;
DELETE FROM native_tree_models;
DELETE FROM native_tree_labels;
DELETE FROM native_tree_tools;
DELETE FROM native_tree_usage_models;
DELETE FROM native_tree_copied_identities;
DELETE FROM native_usage_current;
DELETE FROM native_usage_migrations;
DELETE FROM meta WHERE key IN ('native_usage_discovery_done','native_usage_discovery_after','native_usage_migration_clock','native_tree_copy_clock');
DELETE FROM files;
INSERT INTO meta(key,value) VALUES('native_tree_legacy_max_file','0') ON CONFLICT(key) DO UPDATE SET value='0';
INSERT INTO meta(key,value) VALUES('native_usage_discovery_done','1') ON CONFLICT(key) DO UPDATE SET value='1';
DELETE FROM meta WHERE key GLOB 'archive_event_index:*';
DELETE FROM meta WHERE key GLOB 'native_codex_header:*';
DELETE FROM meta WHERE key GLOB 'native_claude_sidecar:*';
DELETE FROM meta WHERE key GLOB 'native_source_chunks:*';
DELETE FROM meta WHERE key GLOB 'native_proof_tree:*';
DELETE FROM meta WHERE key GLOB 'native_proof_repair:*';
DELETE FROM meta WHERE key GLOB 'native_parent_marker:*';
DELETE FROM meta WHERE key GLOB 'native_tree_summary:*';
DELETE FROM meta WHERE key GLOB 'native_tree_failure:*';
DELETE FROM meta WHERE key GLOB 'native_launch_calls:*';
";

const FILE_COLUMNS: &str = "file_id, path, dev, ino, size, mtime_ns, resume_at, head_sha256, \
     tail_sha256, entrypoint, title, agent_name, last_model, failed, first_ms, last_ms, cwd, \
     branch, pending, tool_ids, yields, busy, links, codex_tokens, codex_tokens_by_model, \
     rate_limits, codex_native_usage, copilot, ctime_ns";

const PUT_FILE: &str = "INSERT INTO files (path, dev, ino, size, mtime_ns, resume_at, \
     head_sha256, tail_sha256, entrypoint, title, agent_name, last_model, failed, first_ms, \
     last_ms, cwd, branch, pending, tool_ids, yields, busy, links, codex_tokens, \
     codex_tokens_by_model, rate_limits, codex_native_usage, copilot, ctime_ns) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
     ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28) \
     ON CONFLICT (path) DO UPDATE SET dev = excluded.dev, ino = excluded.ino, \
     size = excluded.size, mtime_ns = excluded.mtime_ns, resume_at = excluded.resume_at, \
     head_sha256 = excluded.head_sha256, tail_sha256 = excluded.tail_sha256, \
     entrypoint = excluded.entrypoint, title = excluded.title, \
     agent_name = excluded.agent_name, last_model = excluded.last_model, \
     failed = excluded.failed, first_ms = excluded.first_ms, last_ms = excluded.last_ms, \
     cwd = excluded.cwd, branch = excluded.branch, pending = excluded.pending, \
     tool_ids = excluded.tool_ids, yields = excluded.yields, busy = excluded.busy, \
     links = excluded.links, codex_tokens = excluded.codex_tokens, \
     codex_tokens_by_model = excluded.codex_tokens_by_model, \
     rate_limits = excluded.rate_limits, codex_native_usage = excluded.codex_native_usage, copilot = excluded.copilot, ctime_ns = excluded.ctime_ns \
     RETURNING file_id";

const EVENT_COLUMNS: &str = "extra, seq, k, o, b, t, id, n, code_mode, code_mode_open, parent, \
     script, y, poll, reply, reply_o, reply_b, reply_t, reply_e, reply_f, reply_m, item, peer, \
     peer_sock, peer_pid, peer_start, u";

const PUT_EVENT: &str = "INSERT OR REPLACE INTO events (file_id, extra, seq, k, o, b, t, id, n, \
     code_mode, code_mode_open, parent, script, y, poll, reply, reply_o, reply_b, reply_t, \
     reply_e, reply_f, reply_m, item, peer, peer_sock, peer_pid, peer_start, u) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
     ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)";

const SIGNAL_COLUMNS: &str = "seq, k, o, t, at, n, v";

const PUT_SIGNAL: &str = "INSERT OR REPLACE INTO signals (file_id, seq, k, o, t, at, n, v) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const USAGE_COLUMNS: &str = "message_id, model, input, cached_input, output, reasoning_output, \
     total, model_input, model_output, model_cache_write, model_cache_read, billing_input, \
     billing_output, billing_cache_read, billing_cache_write_5m, billing_cache_write_1h, \
     billing_web_search_requests, billing_speed, billing_service_tier, billing_prompt_size, \
     billing_timestamp, billing_split_unknown, record_ids";

const PUT_USAGE: &str = "INSERT OR REPLACE INTO usage (file_id, message_id, model, input, \
     cached_input, output, reasoning_output, total, model_input, model_output, \
     model_cache_write, model_cache_read, billing_input, billing_output, billing_cache_read, \
     billing_cache_write_5m, billing_cache_write_1h, billing_web_search_requests, \
     billing_speed, billing_service_tier, billing_prompt_size, billing_timestamp, \
     billing_split_unknown, record_ids) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
     ?19, ?20, ?21, ?22, ?23, ?24)";

const CODEX_USAGE_COLUMNS: &str = "seq, model, input, output, cache_write, cache_read, t";

const PUT_CODEX_USAGE: &str = "INSERT OR REPLACE INTO codex_usage (file_id, seq, model, input, \
     output, cache_write, cache_read, t) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const RUN_COLUMNS: &str = "session_id, start, cost, duration, api_duration, tool_duration, \
     lines_added, lines_removed, model_usage, capture_at";

const PUT_RUN: &str = "INSERT OR REPLACE INTO reported_runs (session_id, start, cost, duration, \
     api_duration, tool_duration, lines_added, lines_removed, model_usage, capture_at) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

/// The SQLite [`IndexStore`]: one connection, used by one
/// [`super::EventCache`].
struct SqliteStore {
    connection: Connection,
    path: PathBuf,
    selected_runs: Option<std::collections::BTreeSet<String>>,
    semantic_rejection: Option<&'static str>,
    #[cfg(test)]
    traversal_observation: Option<crate::native_timing::CatalogTraversalProbe>,
}

/// Opens (creating when missing) the store at `path` and reads all of it,
/// first importing what the retired JSON event cache at `legacy` holds that
/// the logs can't give back ([`import_legacy`]).
/// A file that isn't a database, or a damaged one, is renamed aside to
/// `<path>.corrupt` and a new store is made in its place. Any other failure
/// (a read-only directory, a lock held past the timeout, a store a newer
/// Semon wrote) is returned for the caller to run in memory.
pub(super) fn open(path: &Path, legacy: &Path) -> Result<(Box<dyn IndexStore>, Loaded), String> {
    let (store, loaded) = match SqliteStore::attempt(path, Some(legacy)) {
        Err(Unopened::Corrupt(error)) => {
            set_aside(path).map_err(|aside| format!("{error}; it can't be set aside: {aside}"))?;
            eprintln!(
                "semon: the session index {} was damaged ({error}); it was set aside and is being rebuilt",
                path.display()
            );
            SqliteStore::attempt(path, Some(legacy)).map_err(|error| error.to_string())?
        }
        opened => opened.map_err(|error| error.to_string())?,
    };
    Ok((Box::new(store), loaded))
}

/// Producer open: initialize the durable store without restoring unrelated indexes.
/// Store failures are returned; a background worker must retry rather than silently
/// switch to an unbounded in-memory rebuild.
pub(super) fn open_scoped(
    path: &Path,
    legacy: &Path,
) -> Result<(Box<dyn IndexStore>, Loaded), String> {
    let (store, loaded) =
        SqliteStore::attempt_scoped(path, Some(legacy), true).map_err(|error| error.to_string())?;
    Ok((Box::new(store), loaded))
}

/// Restart/read interest can resume durable work without opening Event payloads.
/// An absent store alone does not ask a qualified-read observer to cold-index.
pub(crate) fn catalog_work_pending(options: &crate::Options) -> io::Result<bool> {
    let path = super::EventCache::path(&options.cache);
    if !path.exists() {
        return Ok(false);
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(io::Error::other)?;
    connection
        .busy_timeout(Duration::from_millis(100))
        .map_err(io::Error::other)?;
    let stages: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='catalog_semantic_stage')", [], |row| row.get(0)).map_err(io::Error::other)?;
    let pending: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations) OR EXISTS(SELECT 1 FROM meta WHERE key='catalog_backfill_state' AND value IN ('discovering','draining')) OR EXISTS(SELECT 1 FROM meta WHERE key='catalog_semantic_gc' AND value='1')",[],|row|row.get(0)).map_err(io::Error::other)?;
    Ok(pending || (stages && semantic_stage::active(&connection).map_err(io::Error::other)?))
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
pub(crate) fn note_catalog_audit(
    options: &crate::Options,
    expected: Option<&str>,
    now: i64,
) -> io::Result<bool> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let ready: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='catalog_generation' AND value=?1) AND NOT EXISTS(SELECT 1 FROM session_catalog_invalidations) AND NOT EXISTS(SELECT 1 FROM catalog_semantic_stage)", [expected], |r| r.get(0)).map_err(io::Error::other)?;
    if ready {
        tx.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_observed_at',?1)",
            [now.to_string()],
        )
        .map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)?;
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
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let observations = crate::native_evidence::RuntimeObservations::new(options);
    let machine = crate::facts::MachineFacts::of(options);
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let mut changed = std::collections::BTreeSet::new();
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
                        .prepare(NATIVE_METADATA_VALUE)
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
        let recorded = ledger_row(&tx, &key).map_err(io::Error::other)?;
        if match recorded.as_ref() {
            Some((_, ledger)) => super::Stat::of(&path).ok().as_ref() != Some(&ledger.stat),
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
                    .prepare(NATIVE_METADATA_VALUE)
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
            let mut statement = tx.prepare("SELECT s.source_path, CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM session_catalog_sources s JOIN session_catalog c USING(session_key) WHERE s.lifecycle='current' AND s.source_path>?1 GROUP BY s.source_path ORDER BY s.source_path LIMIT 64").map_err(io::Error::other)?;
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
            let expected = observations.expected(&row);
            let saved_runtime = row
                .query
                .as_ref()
                .and_then(|query| query["runtime"].as_str());
            let recorded = ledger_row(&tx, &path).map_err(io::Error::other)?;
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
    for path in changed {
        invalidate_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
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
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let mut found = std::collections::BTreeSet::new();
    let mut statement=store.connection.prepare("SELECT d.source_path FROM catalog_native_dependencies d WHERE harness=?1 AND native_id=?2 AND EXISTS(SELECT 1 FROM session_catalog_sources s WHERE s.source_path=d.source_path AND s.lifecycle='current') LIMIT 65").map_err(io::Error::other)?;
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
                "SELECT DISTINCT f.path FROM events e JOIN files f USING(file_id) WHERE e.extra=0 AND e.k='xsm' AND e.id=?1 AND EXISTS(SELECT 1 FROM session_catalog_sources s WHERE s.source_path=f.path AND s.harness='claude' AND s.lifecycle='current') LIMIT 65",
                "SELECT DISTINCT f.path FROM events e JOIN files f USING(file_id) WHERE e.extra=0 AND e.k='tool' AND e.n='SendMessage' AND e.reply=1 AND e.reply_m=?1 AND (e.reply_f & ?2)=0 AND EXISTS(SELECT 1 FROM session_catalog_sources s WHERE s.source_path=f.path AND s.harness='claude' AND s.lifecycle='current') LIMIT 65",
            ] {
                let mut messages = store
                    .connection
                    .prepare_cached(sql)
                    .map_err(io::Error::other)?;
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
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    if let Some((_, ledger)) =
        ledger_row(&store.connection, &path.to_string_lossy()).map_err(io::Error::other)?
        && ledger.stat == *stat
        && ledger.offset == length
    {
        return Ok(format!(
            "{:x}",
            sha2::digest::generic_array::GenericArray::from(ledger.prefix)
        ) == expected);
    }
    let mut file = crate::sealed::LogFile::open(path)?;
    let (prefix, _) = super::consumed_hashes(&mut file, length)?;
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
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let path = input.full_path(options).to_string_lossy().into_owned();
    tx.execute(
        "INSERT OR REPLACE INTO session_source_mutations VALUES(?1,?2)",
        params![
            path,
            serde_json::to_string(evidence).map_err(io::Error::other)?
        ],
    )
    .map_err(io::Error::other)?;
    tx.execute(
        "INSERT OR IGNORE INTO catalog_relocation_candidates VALUES(?1,?2,?3)",
        params![evidence.from_path, path, evidence.native_id],
    )
    .map_err(io::Error::other)?;
    invalidate_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    invalidate_catalog_source(&tx, &evidence.from_path, "changed").map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

/// Discover prior current paths in bounded durable keyset pages during repair.
/// Filesystem discovery cannot enumerate disappeared or deselected sources.
pub(crate) fn discover_recovery_sources(options: &crate::Options) -> io::Result<bool> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let after: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_recovery_after'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    let Some(after) = after else {
        return Ok(true);
    };
    let paths=tx.prepare("SELECT DISTINCT source_path FROM session_catalog_sources WHERE lifecycle='current' AND source_path>?1 ORDER BY source_path LIMIT 64").map_err(io::Error::other)?.query_map([after],|row|row.get::<_,String>(0)).map_err(io::Error::other)?.collect::<Result<Vec<_>,_>>().map_err(io::Error::other)?;
    for path in &paths {
        if tx
            .execute(
                "INSERT OR IGNORE INTO catalog_backfill_sources VALUES(?1)",
                [path],
            )
            .map_err(io::Error::other)?
            > 0
        {
            invalidate_catalog_source(&tx, path, "changed").map_err(io::Error::other)?;
        }
    }
    if let Some(last) = paths.last() {
        tx.execute(
            "UPDATE meta SET value=?1 WHERE key='catalog_recovery_after'",
            [last],
        )
        .map_err(io::Error::other)?;
    } else {
        tx.execute("DELETE FROM meta WHERE key='catalog_recovery_after'", [])
            .map_err(io::Error::other)?;
    }
    let complete = paths.is_empty();
    tx.commit().map_err(io::Error::other)?;
    Ok(complete)
}

pub(crate) fn begin_catalog_recovery(options: &crate::Options, reason: &str) -> io::Result<()> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    tx.execute_batch("DELETE FROM catalog_backfill_sources; INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_backfill_state','discovering'),('catalog_recovery_after','');").map_err(io::Error::other)?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_recovery_reason',?1)",
        [reason],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

/// Fence an existing publication's bounded upgrade before an old subset stage
/// can publish the current version. Published history remains available while
/// the ordinary discovery and drain owners rebuild its derived metadata.
pub(crate) fn recover_unsupported_catalog(options: &crate::Options) -> io::Result<bool> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let unsupported: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key='catalog_generation') AND COALESCE((SELECT value FROM meta WHERE key='catalog_version'),'')!=?1 AND NOT EXISTS(SELECT 1 FROM meta WHERE key='catalog_backfill_state' AND value IN ('discovering','draining'))",
        [crate::model::summary::CATALOG_VERSION.to_string()], |row| row.get(0),
    ).map_err(io::Error::other)?;
    if unsupported {
        tx.execute_batch("DELETE FROM catalog_backfill_sources; INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_backfill_state','discovering'),('catalog_recovery_after',''),('catalog_recovery_reason','unsupported-publication-version');").map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)?;
    Ok(unsupported)
}

pub(crate) fn push_driven(options: &crate::Options) -> io::Result<bool> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    store
        .connection
        .query_row(
            "SELECT COALESCE((SELECT value FROM meta WHERE key='catalog_push_driven'),'0')='1'",
            [],
            |row| row.get(0),
        )
        .map_err(io::Error::other)
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
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    for path in paths {
        invalidate_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)
}

pub(crate) fn enqueue_sources(
    options: &crate::Options,
    inputs: &[crate::inputs::Input],
) -> io::Result<()> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
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
        "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_push_driven','1')",
        [],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

fn enqueue_native_tx(tx: &Transaction<'_>, harness: &str, id: &str) -> rusqlite::Result<()> {
    // Facts can change before a new owner has its first published Catalog row.
    // Keep those staged observations under the same exact-source revision fence.
    let mut statement=tx.prepare("SELECT source_path FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 UNION SELECT i.source_path FROM catalog_semantic_inputs i JOIN catalog_semantic_sources s USING(source_path) WHERE i.harness=?1 AND i.native_id=?2 AND s.index_done=1 AND EXISTS(SELECT 1 FROM catalog_semantic_stage) UNION SELECT own.source_path FROM catalog_semantic_native n JOIN catalog_semantic_owner_sources own USING(session_key) JOIN catalog_semantic_sources s USING(source_path) WHERE n.harness=?1 AND n.native_id=?2 AND s.index_done=1 AND EXISTS(SELECT 1 FROM catalog_semantic_stage) UNION SELECT d.source_path FROM catalog_native_dependencies d JOIN catalog_semantic_sources s USING(source_path) WHERE d.harness=?1 AND d.native_id=?2 AND s.index_done=1 AND EXISTS(SELECT 1 FROM catalog_semantic_stage)")?;
    let paths = statement
        .query_map(params![harness, id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for path in paths {
        invalidate_catalog_source(tx, &path, "changed")?;
    }
    Ok(())
}
pub(crate) fn enqueue_native(options: &crate::Options, harness: &str, id: &str) -> io::Result<()> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    enqueue_native_tx(&tx, harness, id).map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}
pub(crate) fn enqueue_pid(options: &crate::Options, pid: u32) -> io::Result<()> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    enqueue_dependency_tx(&tx, "runtime:pid", &pid.to_string()).map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}
fn enqueue_dependency_tx(tx: &Transaction<'_>, harness: &str, id: &str) -> rusqlite::Result<()> {
    let paths = {
        let mut statement=tx.prepare("SELECT d.source_path FROM catalog_native_dependencies d WHERE harness=?1 AND native_id=?2 AND (EXISTS(SELECT 1 FROM session_catalog_sources s WHERE s.lifecycle='current' AND s.source_path=d.source_path) OR EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=d.source_path AND s.index_done=1 AND EXISTS(SELECT 1 FROM catalog_semantic_stage)))")?;
        statement
            .query_map(params![harness, id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for path in paths {
        invalidate_catalog_source(tx, &path, "changed")?;
    }
    Ok(())
}

/// Keep a previously qualified legacy runtime fence usable across a journaled
/// facts write. Only fingerprints are persisted; no process payload is copied.
/// Row readers additionally reject pending source/dependency revisions.
pub(crate) fn preserve_legacy_runtime(
    options: &crate::Options,
    before: &crate::Facts,
    after: &crate::Facts,
) -> io::Result<()> {
    let previous = crate::native_evidence::recorded_runtime_fingerprint(options, before)?;
    let wanted = crate::native_evidence::recorded_runtime_fingerprint(options, after)?;
    let legacy = crate::native_evidence::legacy_runtime_generation(options)?;
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_legacy_runtime_semantics'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    if existing.as_deref() != Some(previous.as_str()) {
        tx.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_legacy_runtime_anchor',?1)",
            [legacy],
        )
        .map_err(io::Error::other)?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_legacy_runtime_semantics',?1)",
        [wanted],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

pub(crate) fn enqueue_facts(
    options: &crate::Options,
    before: &crate::Facts,
    after: &crate::Facts,
    push_driven: bool,
) -> io::Result<()> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let global = before.hostname != after.hostname
        || before.home != after.home
        || before.offline_since != after.offline_since;
    if global {
        // These fields are actual dependencies of every recorded observation.
        // Queue their source paths; bounded workers still never build a cohort.
        let paths = {
            let mut statement=tx.prepare("SELECT source_path FROM session_catalog_sources WHERE lifecycle='current' UNION SELECT source_path FROM catalog_semantic_sources WHERE index_done=1 AND EXISTS(SELECT 1 FROM catalog_semantic_stage)").map_err(io::Error::other)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(io::Error::other)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(io::Error::other)?
        };
        for path in paths {
            invalidate_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
        }
    }
    for id in before.codex_locks.keys().chain(after.codex_locks.keys()) {
        if before.codex_locks.get(id) != after.codex_locks.get(id) {
            enqueue_native_tx(&tx, "codex", id).map_err(io::Error::other)?;
        }
    }
    let names = |facts: &crate::Facts| {
        facts
            .native_names
            .iter()
            .map(|name| {
                (
                    (name.harness.clone(), name.native_id.clone()),
                    (
                        name.name.clone(),
                        name.source.clone(),
                        name.updated_at.clone(),
                        name.freshness.clone(),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let previous_names = names(before);
    let current_names = names(after);
    for (harness, id) in previous_names.keys().chain(current_names.keys()) {
        if previous_names.get(&(harness.clone(), id.clone()))
            != current_names.get(&(harness.clone(), id.clone()))
        {
            enqueue_native_tx(&tx, harness, id).map_err(io::Error::other)?;
        }
    }
    for run in before.reported_runs.iter().chain(&after.reported_runs) {
        if before.reported_runs.contains(run) != after.reported_runs.contains(run) {
            enqueue_native_tx(&tx, "claude", &run.last_session_id).map_err(io::Error::other)?;
        }
    }
    for cwd in before.repos.keys().chain(after.repos.keys()) {
        if before.repos.get(cwd) != after.repos.get(cwd) {
            let paths = {
                let mut statement = tx
                    .prepare("SELECT path FROM files WHERE cwd=?1")
                    .map_err(io::Error::other)?;
                statement
                    .query_map([cwd], |row| row.get::<_, String>(0))
                    .map_err(io::Error::other)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(io::Error::other)?
            };
            for path in paths {
                invalidate_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
            }
        }
    }
    for pid in before
        .proc_starts
        .keys()
        .chain(after.proc_starts.keys())
        .chain(before.runs.keys())
        .chain(after.runs.keys())
        .chain(before.process_ancestors.keys())
        .chain(after.process_ancestors.keys())
    {
        if before.proc_starts.get(pid) != after.proc_starts.get(pid)
            || before.runs.get(pid) != after.runs.get(pid)
            || before.process_ancestors.get(pid) != after.process_ancestors.get(pid)
        {
            enqueue_dependency_tx(&tx, "runtime:pid", &pid.to_string())
                .map_err(io::Error::other)?;
            for (id, owner) in before.codex_locks.iter().chain(&after.codex_locks) {
                if owner == pid {
                    enqueue_native_tx(&tx, "codex", id).map_err(io::Error::other)?;
                }
            }
        }
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
        let mut statement=tx.prepare("SELECT DISTINCT source_path FROM session_catalog_sources WHERE harness='codex' AND lifecycle='current'").map_err(io::Error::other)?;
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
        let mut statement=tx.prepare("SELECT DISTINCT source_path FROM session_catalog_sources WHERE lifecycle='current' AND harness='codex' AND native_id=?1 LIMIT 2").map_err(io::Error::other)?;
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
                    "INSERT OR IGNORE INTO catalog_relocation_candidates VALUES(?1,?2,?3)",
                    params![old, new_path.to_string_lossy(), id],
                )
                .map_err(io::Error::other)?;
            }
        }
    }
    for path in previous.symmetric_difference(&current) {
        if crate::is_input_path("codex", path) {
            invalidate_catalog_source(
                &tx,
                options.codex_home.join(path).to_string_lossy().as_ref(),
                "changed",
            )
            .map_err(io::Error::other)?;
        }
    }
    if push_driven {
        tx.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_push_driven','1')",
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
    path: &str,
    ledger: &Ledger,
    physical: &Stat,
) -> io::Result<()> {
    let (store, _) = SqliteStore::attempt_scoped(&super::EventCache::path(stem), None, true)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
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
        "INSERT OR REPLACE INTO session_source_mutations VALUES(?1,?2)",
        params![path, json(&evidence).map_err(io::Error::other)?],
    )
    .map_err(io::Error::other)?;
    tx.commit().map_err(io::Error::other)
}

fn runtime_input_evidence(
    connection: &Connection,
    path: &str,
) -> rusqlite::Result<Option<crate::source_change::RuntimeInputEvidence>> {
    let text: Option<Option<String>> = connection.query_row("SELECT CASE WHEN octet_length(evidence)<=2048 THEN evidence ELSE NULL END FROM session_source_mutations WHERE source_path=?1", [path], |row| row.get(0)).optional()?;
    Ok(text
        .flatten()
        .and_then(|text| {
            serde_json::from_str::<crate::source_change::RuntimeInputEvidence>(&text).ok()
        })
        .filter(|evidence| {
            [
                &evidence.runtime_before,
                &evidence.legacy_before,
                &evidence.unaffected,
            ]
            .into_iter()
            .all(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        }))
}

/// Resolve a pre-Raw runtime witness under the input's facts fence. This also
/// runs on replay; a crash cannot lose the pre-mutation legacy qualification.
pub(crate) fn resume_runtime_input(options: &crate::Options, path: &str) -> io::Result<()> {
    if options.facts.is_none() {
        return Ok(());
    }
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
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
    let Some(facts) = options
        .facts
        .as_ref()
        .and_then(|file| crate::read_facts(file).ok())
    else {
        return Ok(());
    };
    let unaffected = crate::native_evidence::recorded_runtime_fingerprint_excluding(
        options,
        &facts,
        Some(Path::new(path)),
    )
    .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if unaffected != evidence.unaffected {
        return Ok(());
    }
    // The new PID identity is dirtied before advancing the compatibility fence.
    if let Some(id) = crate::read_regular_at_most(Path::new(path), 65536)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["sessionId"].as_str().map(str::to_owned))
    {
        enqueue_native_tx(tx, "claude", &id)?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_legacy_runtime_anchor',?1)",
        [&evidence.legacy_before],
    )?;
    let wanted = crate::native_evidence::recorded_runtime_fingerprint(options, &facts)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_legacy_runtime_semantics',?1)",
        [wanted],
    )?;
    Ok(())
}

pub(crate) fn begin_source_write(
    options: &crate::Options,
    input: &crate::Input,
) -> io::Result<(i64, Option<crate::source_change::AppendEvidence>)> {
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let path = input.full_path(options).to_string_lossy().into_owned();
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let previous = read_append_evidence(&tx, &path).map_err(io::Error::other)?;
    #[cfg(test)]
    if let Some(hook) = BEFORE_SOURCE_INVALIDATION.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    invalidate_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    if input.path.ends_with(".meta.json") {
        let log = input
            .full_path(options)
            .with_extension("")
            .with_extension("jsonl");
        invalidate_catalog_source(&tx, log.to_string_lossy().as_ref(), "changed")
            .map_err(io::Error::other)?;
    }
    let revision = tx
        .query_row(
            "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
            [&path],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    tx.execute(
        "DELETE FROM session_source_mutations WHERE source_path=?1",
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
        let runtime_before = crate::native_evidence::recorded_runtime_fingerprint(options, &facts)?;
        let preserved: Option<(String,String)> = tx.query_row("SELECT (SELECT value FROM meta WHERE key='catalog_legacy_runtime_anchor'),(SELECT value FROM meta WHERE key='catalog_legacy_runtime_semantics')",[],|row|Ok((row.get(0)?,row.get(1)?))).ok();
        let legacy_before = match preserved {
            Some((anchor, semantics)) if semantics == runtime_before => anchor,
            _ => crate::native_evidence::legacy_runtime_generation(options)?,
        };
        let evidence = crate::source_change::RuntimeInputEvidence {
            runtime_before,
            legacy_before,
            unaffected: crate::native_evidence::recorded_runtime_fingerprint_excluding(
                options,
                &facts,
                Some(Path::new(&path)),
            )?,
        };
        tx.execute(
            "INSERT OR REPLACE INTO session_source_mutations VALUES(?1,?2)",
            params![path, json(&evidence).map_err(io::Error::other)?],
        )
        .map_err(io::Error::other)?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_push_driven','1')",
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
    let (store, _) =
        SqliteStore::attempt_scoped(&super::EventCache::path(&options.cache), None, true)
            .map_err(|error| io::Error::other(error.to_string()))?;
    let path = input.full_path(options).to_string_lossy().into_owned();
    let tx = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    let actual: Option<i64> = tx
        .query_row(
            "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
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
            "INSERT OR REPLACE INTO session_source_mutations VALUES(?1,?2)",
            params![path, json(evidence).map_err(io::Error::other)?],
        )
        .map_err(io::Error::other)?;
    } else {
        tx.execute(
            "DELETE FROM session_source_mutations WHERE source_path=?1",
            [&path],
        )
        .map_err(io::Error::other)?;
    }
    // Whole JSON inputs keep their pre-Raw revision until dependency replay.
    // Their shared source/facts fence prevents resolution during the write.
    // Keeping the revision also processes the input before its newly dirtied
    // native logs, avoiding an unnecessary second publication of those logs.
    if !input.path.ends_with(".json") || actual != Some(revision) {
        invalidate_catalog_source(&tx, &path, "changed").map_err(io::Error::other)?;
    }
    tx.commit().map_err(io::Error::other)
}

fn read_append_evidence(
    connection: &Connection,
    path: &str,
) -> rusqlite::Result<Option<crate::source_change::AppendEvidence>> {
    let text: Option<Option<String>> = connection.query_row("SELECT CASE WHEN octet_length(evidence)<=2048 THEN evidence ELSE NULL END FROM session_source_mutations WHERE source_path=?1",[path],|row|row.get(0)).optional()?;
    Ok(text
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok()))
}

/// Load only the selected native sources in the caller's existing read snapshot.
/// This never opens a writable store, imports legacy data, walks roots or prunes
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
    let mut runs = connection.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM reported_runs WHERE session_id=?1 ORDER BY start"
    ))?;
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
    let mut metadata = connection.prepare(NATIVE_METADATA_VALUE)?;
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

/// Additive storage preparation for already-published focused recipes. This is
/// the existing schema owner with lazy loading and no legacy-file import. A
/// parser change is left to its producer instead of resetting evidence in a read.
pub(crate) fn focused_connection(path: &Path) -> io::Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io::Error::other)?;
    let (schema, parser) = versions(&db).map_err(io::Error::other)?;
    if schema == SCHEMA_VERSION && parser == Some(CACHE_VERSION) {
        return Ok(db);
    }
    if schema > SCHEMA_VERSION || parser != Some(CACHE_VERSION) {
        return Err(io::Error::other(
            "Native focused schema requires its producer",
        ));
    }
    drop(db);
    let _owner = SqliteStore::attempt_scoped(path, None, true)
        .map_err(|_| io::Error::other("Native focused schema preparation unavailable"))?;
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)
}

/// The `meta` key recording when a retired JSON event cache that couldn't
/// be read was set aside (epoch ms). A readable one needs no marker: it is
/// imported whenever it is found, and removed.
const SET_ASIDE: &str = "legacy_json_set_aside";

/// What the retired JSON event cache (`sessions-index.events.json`) holds
/// that the logs can't give back: the reported runs `~/.claude.json` has
/// since overwritten, and the stamp they were last read at. Only these
/// fields are read; the rest of the file (its version and every file's
/// index) is skipped as it streams past, and those files are read again
/// from their logs.
#[derive(Deserialize)]
struct Legacy {
    #[serde(default)]
    reported_runs: BTreeMap<String, BTreeMap<i64, ReportedRunSnapshot>>,
    #[serde(default)]
    claude_json_stamp: Option<ReportedFileStamp>,
}

/// A test hook: runs once, at a set point of the import.
#[cfg(test)]
type Hook = std::cell::RefCell<Option<Box<dyn FnOnce()>>>;

#[cfg(test)]
thread_local! {
    /// The JSON caches this thread imported (or set aside), in order.
    pub(super) static IMPORTS: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Runs once, after the JSON cache is parsed and before its import
    /// takes the write lock.
    pub(super) static BEFORE_IMPORT: Hook = const { std::cell::RefCell::new(None) };
    /// Runs once, after the JSON cache is claimed and before the claimed
    /// file is checked and removed.
    pub(super) static AFTER_CLAIM: Hook = const { std::cell::RefCell::new(None) };
    static AFTER_TREE_LEDGER: Hook = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn hook(slot: &'static std::thread::LocalKey<Hook>) {
    if let Some(hook) = slot.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

/// Which file a handle is: dev, inode, size and modified time. A rename
/// keeps all four; an older semon's save (a new file renamed over the path)
/// changes the inode.
type Identity = (u64, u64, u64, Option<std::time::SystemTime>);

fn identity(file: &fs::File) -> io::Result<Identity> {
    let metadata = file.metadata()?;
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    Ok((dev, ino, metadata.len(), metadata.modified().ok()))
}

/// A JSON cache read from an open handle: which file it was, and what it
/// held. An error when it couldn't be read to its end (an I/O error): it is
/// then left for the next open.
fn read_legacy(file: &fs::File) -> io::Result<(Identity, Result<Legacy, serde_json::Error>)> {
    let read_as = identity(file)?;
    match serde_json::from_reader::<_, Legacy>(io::BufReader::new(file)) {
        Err(error) if error.is_io() => Err(io::Error::other(error)),
        parsed => Ok((read_as, parsed)),
    }
}

/// Imports the retired JSON event cache whenever it is found, then removes
/// it.
///
/// Its reported runs join the store's with `INSERT OR IGNORE`, so a run the
/// store holds (captured since, or imported before) keeps its snapshot, and
/// importing the same file twice, or from two processes at once, changes
/// nothing. Its stamp is taken only if the store has none. It is imported
/// again whenever it is present: an older semon still running after the
/// upgrade rewrites it with runs no process here has seen, and
/// `~/.claude.json` has since overwritten them.
///
/// The file is parsed from its open handle before the write lock is taken,
/// reading only those two fields as it streams past, and its rows commit
/// with `synchronous = FULL`, so they are on disk before the file goes.
///
/// Removal claims the file first, renaming it to `<name>.importing.<pid>.<n>.<16 hex digits>`,
/// so no save an older semon makes to the path is ever removed or set aside
/// unread: the claimed file is removed (or set aside) only if it is the one
/// that was parsed, and read and imported first if it isn't. A claim left by
/// a crash is imported and removed at the next open, as is any other
/// process's claim: every step is idempotent.
///
/// A file that isn't that JSON, whatever its version, is renamed aside
/// (`.corrupt`, then `.corrupt.1`, …; never over an existing file) and
/// recorded in `meta`. A file that can't be opened or read to its end is
/// left for the next open.
fn import_legacy(connection: &mut Connection, legacy: &Path) -> rusqlite::Result<()> {
    for claimed in claims(legacy) {
        finish(connection, legacy, &claimed, None)?;
    }
    let Some(file) = open_regular(legacy) else {
        return Ok(());
    };
    let (read_as, parsed) = match read_legacy(&file) {
        Ok(read) => read,
        Err(error) => {
            warn_import(legacy, &error);
            return Ok(());
        }
    };
    // The handle stays open until the claim is checked, so its inode can't
    // be reused by another file meanwhile.
    #[cfg(test)]
    hook(&BEFORE_IMPORT);
    if let Ok(found) = &parsed {
        import_rows(connection, legacy, Some(found))?;
    }
    let claimed = sibling(legacy, &claim_suffix());
    match fs::rename(legacy, &claimed) {
        Ok(()) => {}
        // Claimed or removed by another process meanwhile: its import is
        // the same as this one.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            warn_import(legacy, &error);
            return Ok(());
        }
    }
    #[cfg(test)]
    hook(&AFTER_CLAIM);
    let finished = finish(connection, legacy, &claimed, Some((read_as, parsed)));
    drop(file);
    finished
}

/// A claim's suffix, unique to this call: the pid, a counter, and 64
/// random bits, so no two claims share a name (a rename would replace the
/// other), whichever processes make them.
fn claim_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    static CLAIMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let claim = CLAIMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut random = std::collections::hash_map::RandomState::new().build_hasher();
    random.write_u64(claim);
    random.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    format!(
        ".importing.{}.{claim}.{:016x}",
        std::process::id(),
        random.finish()
    )
}

/// Whether `suffix` (after `<name>`) is a claim's: `.importing.<pid>.<n>.
/// <16 hex digits>`, exactly as [`claim_suffix`] makes it.
fn is_claim(suffix: &str) -> bool {
    let Some(rest) = suffix.strip_prefix(".importing.") else {
        return false;
    };
    fn digits(part: &str) -> bool {
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
    }
    let parts: Vec<&str> = rest.split('.').collect();
    matches!(
        parts.as_slice(),
        [pid, n, random]
            if digits(pid)
                && digits(n)
                && random.len() == 16
                && random.bytes().all(|byte| byte.is_ascii_hexdigit())
    )
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

/// Opens a JSON cache or claim for reading only if it is a regular file:
/// never through a symlink, and never waiting on a FIFO. Anything else is
/// left where it is and reported once; `None` too when it is gone or can't
/// be opened.
pub(crate) fn open_regular(path: &Path) -> Option<fs::File> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        warn_import(path, &"it isn't a regular file");
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
            warn_import(path, &error);
            return None;
        }
    };
    match file.metadata() {
        Ok(metadata) if metadata.is_file() => Some(file),
        Ok(_) => {
            warn_import(path, &"it isn't a regular file");
            None
        }
        Err(error) => {
            warn_import(path, &error);
            None
        }
    }
}

/// The claims (`<name>.importing.…`) a crash or another process left.
fn claims(legacy: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (legacy.parent(), legacy.file_name()) else {
        return Vec::new();
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let name = name.to_string_lossy();
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .strip_prefix(&*name)
                        .is_some_and(is_claim)
                })
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// Ends the import of a claimed file: removes it when it is the file whose
/// rows were imported (or sets it aside, when that file was unreadable), and
/// otherwise reads it and does the same with its own contents.
fn finish(
    connection: &mut Connection,
    legacy: &Path,
    claimed: &Path,
    read: Option<(Identity, Result<Legacy, serde_json::Error>)>,
) -> rusqlite::Result<()> {
    // Gone (finished by another process), or not a regular file.
    let Some(file) = open_regular(claimed) else {
        return Ok(());
    };
    let parsed = match (identity(&file), read) {
        (Ok(now), Some((read_as, parsed))) if now == read_as => parsed,
        // Not the file that was parsed (or no file was): read this one.
        _ => match read_legacy(&file) {
            Ok((_, parsed)) => {
                if let Ok(found) = &parsed {
                    import_rows(connection, legacy, Some(found))?;
                }
                parsed
            }
            Err(error) => {
                warn_import(legacy, &error);
                return Ok(());
            }
        },
    };
    drop(file);
    match parsed {
        Ok(Legacy { reported_runs, .. }) => {
            remove_legacy(claimed);
            let runs: usize = reported_runs.values().map(BTreeMap::len).sum();
            eprintln!(
                "semon: imported {runs} reported runs from {} into the session index, and removed it",
                legacy.display()
            );
        }
        // Recorded only once it is set aside, so a claim that can't be is
        // tried again rather than recorded as done.
        Err(error) => match set_aside_legacy(legacy, claimed) {
            Ok(Some(aside)) => {
                import_rows(connection, legacy, None)?;
                eprintln!(
                    "semon: {} couldn't be read ({error}); it was set aside as {} and not imported",
                    legacy.display(),
                    aside.display()
                );
            }
            // Set aside by another process meanwhile.
            Ok(None) => {}
            Err(rename) => warn_import(legacy, &rename),
        },
    }
    Ok(())
}

/// Moves an unreadable claimed file to the first free name of
/// `<legacy>.corrupt`, `<legacy>.corrupt.1`, …, never over an existing
/// file: hard-linked there (a link fails rather than replaces), then
/// unlinked from its claim. Where the filesystem has no hard links, it is
/// left intact and an error is returned; a check then rename could overwrite
/// another process's archive. `None` when the claim is
/// already gone: another process set it aside.
fn set_aside_legacy(legacy: &Path, claimed: &Path) -> io::Result<Option<PathBuf>> {
    for n in 0..1000 {
        let aside = if n == 0 {
            sibling(legacy, ".corrupt")
        } else {
            sibling(legacy, &format!(".corrupt.{n}"))
        };
        {
            match fs::hard_link(claimed, &aside) {
                Ok(()) => {
                    match fs::remove_file(claimed) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => {
                            return Err(error);
                        }
                        _ => {}
                    }
                    return Ok(Some(aside));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        if let (Ok(source), Ok(target)) =
                            (fs::symlink_metadata(claimed), fs::symlink_metadata(&aside))
                            && source.dev() == target.dev()
                            && source.ino() == target.ino()
                        {
                            match fs::remove_file(claimed) {
                                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                                    return Err(error);
                                }
                                _ => return Ok(Some(aside)),
                            }
                        }
                    }
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                // No hard links here (EPERM, unsupported): preserve the claim.
                Err(_) => {}
            }
        }
        // Refuse to rename on filesystems without hard links: rename can
        // replace another process's archive after a check for existence.
        // Keep the original claim intact rather than risk losing either file.
        return Err(io::Error::other(
            "cannot archive the claim safely: this filesystem has no hard links",
        ));
    }
    Err(io::Error::other("no free .corrupt name"))
}

/// Writes a parsed JSON cache's runs and stamp in one `BEGIN IMMEDIATE`
/// transaction with `synchronous = FULL`, or, for one that couldn't be read
/// (`None`), the record that it is set aside.
fn import_rows(
    connection: &mut Connection,
    legacy: &Path,
    parsed: Option<&Legacy>,
) -> rusqlite::Result<()> {
    connection.pragma_update(None, "synchronous", "FULL")?;
    let written = write_import(connection, parsed);
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    written?;
    #[cfg(test)]
    IMPORTS.with(|imports| imports.borrow_mut().push(legacy.to_owned()));
    #[cfg(not(test))]
    let _ = legacy;
    Ok(())
}

fn write_import(connection: &mut Connection, parsed: Option<&Legacy>) -> rusqlite::Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match parsed {
        Some(Legacy {
            reported_runs,
            claude_json_stamp,
        }) => {
            let mut put = transaction.prepare(&PUT_RUN.replacen("OR REPLACE", "OR IGNORE", 1))?;
            for run in reported_runs.values().flat_map(BTreeMap::values) {
                put_run(&mut put, run)?;
            }
            drop(put);
            if let Some(stamp) = claude_json_stamp {
                transaction.execute(
                    "INSERT OR IGNORE INTO meta (key, value) VALUES ('claude_json_stamp', ?1)",
                    [json(stamp)?],
                )?;
            }
        }
        None => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis())
                .to_string();
            transaction.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                [SET_ASIDE, now.as_str()],
            )?;
        }
    }
    transaction.commit()
}

/// Removes the imported JSON cache; failing that, it is imported again at
/// the next open.
fn remove_legacy(legacy: &Path) {
    match fs::remove_file(legacy) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => warn_import(legacy, &error),
        _ => {}
    }
}

/// Says once per process that the JSON cache's import is waiting.
fn warn_import(legacy: &Path, error: &dyn fmt::Display) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "semon: {} isn't imported yet ({error}); it is tried again at the next start",
            legacy.display()
        );
    }
}

/// A rusqlite error, as the cache handles it.
fn failure(error: rusqlite::Error) -> StoreError {
    if is_busy(&error) {
        StoreError::Busy(error.to_string())
    } else if is_data_error(&error) {
        StoreError::Data(error.to_string())
    } else {
        StoreError::Failed(error.to_string())
    }
}

impl IndexStore for SqliteStore {
    fn describe(&self) -> String {
        self.path.display().to_string()
    }

    fn session_description(
        &self,
        key: &str,
        version: u32,
        inputs: &str,
    ) -> Result<Option<String>, StoreError> {
        if !current(&self.connection).map_err(failure)? {
            return Ok(None);
        }
        self.connection.query_row(
            "SELECT description FROM session_descriptions WHERE session_key = ?1 AND version = ?2 AND inputs = ?3",
            params![key, version, inputs], |row| row.get(0),
        ).optional().map_err(failure)
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
                let mut put = transaction.prepare("INSERT OR REPLACE INTO session_descriptions (session_key, version, inputs, description) VALUES (?1, ?2, ?3, ?4)").map_err(failure)?;
                for (key, version, inputs, description) in descriptions {
                    put.execute(params![key, version, inputs, description])
                        .map_err(failure)?;
                }
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
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(failure)
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
            let coherent: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key='catalog_generation' AND value=?1) AND EXISTS(SELECT 1 FROM meta WHERE key='slot_projection_catalog_generation' AND value=?1)",
                [expected], |record| record.get(0),
            ).map_err(failure)?;
            if !coherent {
                return Ok(None);
            }
            let header: Option<(String, u32, i64, Option<String>)> = tx.query_row(
                "SELECT generation,version,total,source_generation FROM session_slot_projections WHERE session_key=?1",
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
            let mut statement = tx.prepare(
                "SELECT slot,CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_slots WHERE session_key=?1 ORDER BY slot",
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
        if paths.is_empty() || paths.len() > 64 || keys.len() > 64 || native_parents.len() > 64 {
            return Ok(None);
        }
        let tx = self.connection.unchecked_transaction().map_err(failure)?;
        let mut pending: std::collections::BTreeSet<_> = keys.iter().cloned().collect();
        for (harness, id) in native_parents {
            let mut statement = tx.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 LIMIT 65").map_err(failure)?;
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
            let mut statement = tx.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE source_path=?1 AND lifecycle='current' LIMIT 65").map_err(failure)?;
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
            let mut statement=tx.prepare("SELECT new_path FROM catalog_relocation_candidates WHERE old_path=?1 UNION SELECT old_path FROM catalog_relocation_candidates WHERE new_path=?1 LIMIT 65").map_err(failure)?;
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
            let Some(row) = current_catalog_row(&tx, &key).map_err(failure)? else {
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
            let mut related = tx.prepare("SELECT endpoint FROM (SELECT parent_key AS endpoint FROM session_catalog WHERE session_key=?1 AND lifecycle='current' UNION SELECT session_key FROM session_catalog WHERE parent_key=?1 AND lifecycle='current' UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM session_trace_edges WHERE session_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM session_trace_edges WHERE session_key=?1 UNION SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?1 UNION SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?1) WHERE endpoint IS NOT NULL AND endpoint!='you' ORDER BY endpoint LIMIT 65").map_err(failure)?;
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
                let mut aliases = tx.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 LIMIT 65").map_err(failure)?;
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
        tx.commit().map_err(failure)?;
        Ok(Some(sources.into_iter().collect()))
    }

    fn backfill_state(&self) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_backfill_state'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(failure)
    }
    fn discover_catalog_sources(
        &mut self,
        paths: &[String],
        complete: bool,
    ) -> Result<(), StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        for path in paths {
            if tx
                .execute(
                    "INSERT OR IGNORE INTO catalog_backfill_sources VALUES(?1)",
                    [path],
                )
                .map_err(failure)?
                > 0
            {
                invalidate_catalog_source(&tx, path, "changed").map_err(failure)?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_backfill_state',?1)",
            [if complete { "draining" } else { "discovering" }],
        )
        .map_err(failure)?;
        tx.commit().map_err(failure)
    }
    fn finish_catalog_backfill(&mut self, expected: Option<&str>) -> Result<Outcome, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        let generation: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(failure)?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations) OR EXISTS(SELECT 1 FROM catalog_semantic_stage)",
                [],
                |row| row.get(0),
            )
            .map_err(failure)?;
        if generation.as_deref() != expected || pending {
            return Ok(Outcome::Conflict);
        }
        tx.execute_batch("INSERT OR REPLACE INTO meta(key,value) VALUES('catalog_backfill_state','complete'),('catalog_completeness','complete'),('search_backfill_complete','1'); DELETE FROM meta WHERE key='catalog_recovery_reason';").map_err(failure)?;
        tx.commit().map_err(failure)?;
        Ok(Outcome::Written)
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
        if semantic_stage::active(&tx).map_err(failure)? {
            return Ok(Outcome::Conflict);
        }
        let actual: Option<i64> = tx
            .query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [path],
                |row| row.get(0),
            )
            .optional()
            .map_err(failure)?;
        if actual != Some(revision) {
            return Ok(Outcome::Conflict);
        }
        if let Some((harness, id)) = native {
            enqueue_native_tx(&tx, harness, id).map_err(failure)?;
        }
        if let Some(log) = log {
            invalidate_catalog_source(&tx, log, "changed").map_err(failure)?;
        }
        tx.execute(
            "DELETE FROM session_catalog_invalidations WHERE source_path=?1 AND revision=?2",
            params![path, revision],
        )
        .map_err(failure)?;
        tx.execute(
            "DELETE FROM session_source_mutations WHERE source_path=?1",
            [path],
        )
        .map_err(failure)?;
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
        relocation::reuse(self, path, stat, id, file, measurements)
    }

    fn append_evidence(
        &self,
        path: &str,
    ) -> Result<Option<crate::source_change::AppendEvidence>, StoreError> {
        read_append_evidence(&self.connection, path).map_err(failure)
    }

    fn pending_catalog_sources(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let mut statement = self.connection.prepare("SELECT source_path FROM session_catalog_invalidations ORDER BY revision,source_path LIMIT ?1").map_err(failure)?;
        statement
            .query_map([limit as i64], |row| row.get(0))
            .map_err(failure)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failure)
    }

    fn qualified_catalog(&self) -> Result<bool, StoreError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(failure)?;
        let repairs = {
            let mut statement=tx.prepare("SELECT source_path FROM session_catalog_sources WHERE lifecycle='current' AND session_key IN(SELECT session_key FROM session_slot_projections WHERE version<>?1 LIMIT 64) LIMIT 64").map_err(failure)?;
            statement
                .query_map([crate::slot_projection::VERSION], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(failure)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?
        };
        for path in repairs {
            if !semantic_stage::owns_header_repair(&tx, &path).map_err(failure)? {
                invalidate_catalog_source(&tx, &path, "changed").map_err(failure)?;
            }
        }
        tx.commit().map_err(failure)?;

        self.connection.query_row("SELECT (SELECT value FROM meta WHERE key='catalog_version')=?1 AND (SELECT value FROM meta WHERE key='catalog_completeness')='complete' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_stage) AND (SELECT value FROM meta WHERE key='catalog_generation')=(SELECT value FROM meta WHERE key='slot_projection_catalog_generation')", [crate::model::summary::CATALOG_VERSION.to_string()], |row| row.get::<_,Option<bool>>(0)).map(|value| value.unwrap_or(false)).map_err(failure)
    }

    fn catalog_claims(&self, paths: &[String]) -> Result<Vec<(String, Option<i64>)>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT revision FROM session_catalog_invalidations WHERE source_path=?1")
            .map_err(failure)?;
        paths
            .iter()
            .map(|path| {
                Ok((
                    path.clone(),
                    statement
                        .query_row([path], |row| row.get(0))
                        .optional()
                        .map_err(failure)?,
                ))
            })
            .collect()
    }

    fn ledger(&self, path: &str) -> Result<Option<Ledger>, StoreError> {
        ledger_row(&self.connection, path)
            .map(|found| found.map(|(_, ledger)| ledger))
            .map_err(failure)
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

    fn advance_native_usage_migration(
        &mut self,
        options: &crate::Options,
    ) -> io::Result<(bool, Option<String>)> {
        let (pending, changed) = native_usage::advance(self, options)?;
        let copies = tree_state::advance_copies(&mut self.connection).map_err(io::Error::other)?;
        let cleanup = tree_state::cleanup_work(&mut self.connection).map_err(io::Error::other)?;
        Ok((pending || copies || cleanup, changed))
    }

    fn native_usage_migration_pending(&self) -> Result<bool, StoreError> {
        let usage = native_usage::migration_pending(&self.connection).map_err(failure)?;
        let copies = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_tree_headers WHERE copies_ready=0)",
                [],
                |r| r.get::<_, bool>(0),
            )
            .map_err(failure)?;
        Ok(usage || copies || tree_state::work_pending(&self.connection).map_err(failure)?)
    }

    fn queue_tree_repair(&mut self, path: &str, ledger: &Ledger, harness: &str) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !current(&tx).map_err(io::Error::other)?
            || ledger_row(&tx, path)
                .map_err(io::Error::other)?
                .map(|(_, l)| l)
                .as_ref()
                != Some(ledger)
        {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let failed: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
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
        let usage_needed = harness == "codex"
            && !native_usage::normalized(&tx, path, ledger).map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_usage_migrations(source_path,receipt,usage_needed) VALUES(?1,?2,?3) ON CONFLICT(source_path) DO UPDATE SET state=CASE WHEN native_usage_migrations.state='done' THEN 'ready' ELSE native_usage_migrations.state END",params![path,header_ledger(ledger),usage_needed]).map_err(io::Error::other)?;
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
        if files.len() > 64 {
            return Err(StoreError::Data(
                "initial index batch exceeds source budget".into(),
            ));
        }
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
        for (path, ledger, _) in files {
            if ledger.offset != ledger.stat.size
                || ledger_row(&transaction, path).map_err(failure)?.is_some()
                || Stat::of(Path::new(path)).ok().as_ref() != Some(&ledger.stat)
            {
                return Ok(Outcome::Conflict);
            }
        }
        measurements.record("fences", started, files.len());
        write_activity.stage("fences", &transaction);
        let started = measurements.start();
        for (path, ledger, index) in files {
            write_index_changes_measured(
                &transaction,
                path,
                ledger,
                index,
                None,
                None,
                &mut row_measurements,
            )
            .map_err(failure)?;
        }
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
        let mut statement=transaction.prepare("SELECT file_id,dev,ino,size,mtime_ns,resume_at,head_sha256,tail_sha256,ctime_ns,path FROM files ORDER BY file_id").map_err(failure)?;
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
    // Keep the counter after acknowledgement. Resetting per-path revisions
    // would let an old claim mistake a later observation for its own (ABA).
    transaction
        .prepare_cached(
            "INSERT INTO meta(key,value) VALUES ('catalog_source_revision','1')
         ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(meta.value AS INTEGER)+1 AS TEXT)",
        )?
        .execute([])?;
    transaction.prepare_cached(
        "INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at)
         VALUES (?1,CAST((SELECT value FROM meta WHERE key='catalog_source_revision') AS INTEGER),?2,?3)
         ON CONFLICT(source_path) DO UPDATE SET revision=excluded.revision,
         reason=excluded.reason, observed_at=excluded.observed_at",
    )?.execute(
        params![path, reason, crate::model::now_ms()],
    )?;
    Ok(())
}

enum Unopened {
    /// Not a database, or a damaged one: set aside and rebuilt.
    Corrupt(rusqlite::Error),
    Other(String),
}

impl fmt::Display for Unopened {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt(error) => write!(formatter, "{error}"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

enum Init {
    Ready,
    NoWal,
    Newer,
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
        "SELECT COALESCE((SELECT value FROM meta WHERE key='catalog_completeness'),'complete')='complete' AND COALESCE((SELECT value FROM meta WHERE key='catalog_version'),'')=?1",
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

const CATALOG_INSERT: &str = "INSERT INTO session_catalog (session_key, lifecycle, last_ms, harness, repo, parent_key, metadata) VALUES ";
const CATALOG_CONFLICT: &str = " ON CONFLICT(session_key) DO UPDATE SET lifecycle='current', last_ms=excluded.last_ms, harness=excluded.harness, repo=excluded.repo, parent_key=excluded.parent_key, metadata=excluded.metadata WHERE session_catalog.lifecycle != 'current' OR session_catalog.metadata != excluded.metadata OR session_catalog.parent_key IS NOT excluded.parent_key";

#[derive(Clone, Copy)]
enum CatalogWriter {
    Auto,
    #[cfg(test)]
    Legacy,
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
        CatalogWriter::Legacy => return None,
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

fn put_catalog_batch(
    transaction: &Transaction<'_>,
    rows: &[crate::model::summary::CatalogRow],
    metadata: &[String],
) -> rusqlite::Result<Vec<bool>> {
    let values = (0..rows.len())
        .map(|position| {
            let first = position * 6 + 1;
            format!(
                "(?{first},'current',?{},?{},?{},?{},?{})",
                first + 1,
                first + 2,
                first + 3,
                first + 4,
                first + 5
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("{CATALOG_INSERT}{values}{CATALOG_CONFLICT} RETURNING session_key");
    let mut statement = transaction.prepare(&sql)?;
    for (position, (row, metadata)) in rows.iter().zip(metadata).enumerate() {
        let first = position * 6 + 1;
        statement.raw_bind_parameter(first, &row.key)?;
        statement.raw_bind_parameter(first + 1, row.last.unwrap_or(0))?;
        statement.raw_bind_parameter(first + 2, &row.harness)?;
        statement.raw_bind_parameter(first + 3, &row.repo)?;
        statement.raw_bind_parameter(first + 4, &row.parent)?;
        statement.raw_bind_parameter(first + 5, metadata)?;
    }
    let mut changed = vec![false; rows.len()];
    {
        let mut returned = statement.raw_query();
        while let Some(row) = returned.next()? {
            let rusqlite::types::ValueRef::Text(key) = row.get_ref(0)? else {
                return Err(rusqlite::Error::InvalidQuery);
            };
            let Some(position) = rows.iter().position(|row| row.key.as_bytes() == key) else {
                return Err(rusqlite::Error::InvalidQuery);
            };
            if std::mem::replace(&mut changed[position], true) {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
    }
    // Finalize RETURNING before source/search work or its after-DML sample.
    statement.finalize()?;
    Ok(changed)
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
        self.semantic_rejection = None;
        let rows = publication.rows;
        #[cfg(test)]
        {
            self.traversal_observation = None;
        }
        #[cfg(test)]
        let mut search_observation = None;
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
            if !current(&transaction).map_err(failure)? {
                return Ok(Outcome::Stale);
            }
            let previous: Option<String> = transaction
                .query_row(
                    "SELECT value FROM meta WHERE key = 'catalog_generation'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failure)?;
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
                if (
                    ledger.stat.dev,
                    ledger.stat.ino,
                    ledger.offset,
                    ledger.prefix,
                    ledger.tail,
                    ledger.stat.changed_ns,
                ) != (
                    source.dev,
                    source.ino,
                    source.offset,
                    source.prefix_sha256,
                    source.tail_sha256,
                    source.changed_ns,
                ) {
                    self.semantic_rejection = Some("source_identity");
                    return Ok(Outcome::Conflict);
                }
            }
            stage("fences", &transaction);
            if !crate::history_projection::preserve_replacements(&transaction, publication)
                .map_err(failure)?
            {
                transaction.commit().map_err(failure)?;
                return Ok(Outcome::Deferred);
            }
            crate::history_projection::publish(&transaction, publication).map_err(failure)?;
            stage("history", &transaction);
            let mut metadata_measurements =
                crate::native_timing::Measurements::new(producer.is_some());
            let mut overview_measurements =
                crate::native_timing::Measurements::new(producer.is_some());
            let setup_started = metadata_measurements.start();
            let search_version: Option<String> = transaction
                .query_row(
                    "SELECT value FROM meta WHERE key='search_version'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failure)?;
            let initialize_search =
                search_version.as_deref() != Some(crate::catalog_search::VERSION);
            let search_backfill: Option<String> = transaction
                .query_row(
                    "SELECT value FROM meta WHERE key='search_backfill_complete'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failure)?;
            let rebuild_search =
                claims.is_none() && (initialize_search || search_backfill.as_deref() != Some("1"));
            if initialize_search {
                crate::catalog_search::clear(&transaction).map_err(failure)?;
                transaction.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('search_backfill_complete','0')",[]).map_err(failure)?;
            }
            let previous_version: Option<String> = transaction
                .query_row(
                    "SELECT value FROM meta WHERE key = 'catalog_version'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failure)?;
            metadata_measurements.record("setup", setup_started, 1);
            let mut search_rows = Vec::new();
            let mut metadata_sample = None;
            let mut cohort_changed = None;
            {
                let statements_started = metadata_measurements.start();
                // A temporary membership table bounds deletion SQL parameters and
                // never escapes the transaction or becomes part of the read model.
                transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_members (session_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM catalog_members;").map_err(failure)?;
                {
                    let mut member = transaction
                        .prepare("INSERT INTO catalog_members VALUES (?1)")
                        .map_err(failure)?;
                    let single_sql = format!(
                        "{CATALOG_INSERT}(?1, 'current', ?2, ?3, ?4, ?5, ?6){CATALOG_CONFLICT}"
                    );
                    let mut put = if batch_bytes.is_none() {
                        Some(transaction.prepare(&single_sql).map_err(failure)?)
                    } else {
                        None
                    };
                    let mut remove_sources = transaction
                        .prepare("UPDATE session_catalog_sources SET lifecycle='retained' WHERE session_key=?1")
                        .map_err(failure)?;
                    let mut put_source = transaction.prepare("INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES (?1,?2,?3,?4) ON CONFLICT(session_key,source_path) DO UPDATE SET lifecycle='current',harness=excluded.harness,native_id=excluded.native_id").map_err(failure)?;
                    metadata_measurements.record("statements", statements_started, 1);
                    let bulk_changed = if batch_bytes.is_some() {
                        if producer.is_some() {
                            metadata_sample = Some(crate::native_timing::WriteActivity::new(
                                true,
                                &self.path,
                                "catalog_metadata_cohort",
                                &transaction,
                            ));
                        }
                        // UNIQUE keys were certified before reordering. These
                        // helpers read their own History/Overview, never another
                        // row's newly written Current/source membership.
                        for row in rows {
                            let started = metadata_measurements.start();
                            if producer.is_some() {
                                crate::overview::publish_current_measured(
                                    &transaction,
                                    row,
                                    &mut overview_measurements,
                                )
                                .map_err(failure)?;
                            } else {
                                crate::overview::publish_current(&transaction, row)
                                    .map_err(failure)?;
                            }
                            metadata_measurements.record("overview", started, 1);
                            let started = metadata_measurements.start();
                            member.execute([&row.key]).map_err(failure)?;
                            metadata_measurements.record("catalog_members", started, 1);
                        }
                        if let Some(sample) = metadata_sample.as_mut() {
                            sample.stage("overview_and_members", &transaction);
                        }
                        let started = metadata_measurements.start();
                        let changed =
                            put_catalog_batch(&transaction, rows, &metadata).map_err(failure)?;
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
                            if producer.is_some() {
                                crate::overview::publish_current_measured(
                                    &transaction,
                                    row,
                                    &mut overview_measurements,
                                )
                                .map_err(failure)?;
                            } else {
                                crate::overview::publish_current(&transaction, row)
                                    .map_err(failure)?;
                            }
                            metadata_measurements.record("overview", started, 1);
                            if let Some(sample) = sample.as_mut() {
                                sample.stage("overview", &transaction);
                            }
                            let started = metadata_measurements.start();
                            member.execute([&row.key]).map_err(failure)?;
                            if let Some(sample) = sample.as_mut() {
                                sample.stage("member_insert", &transaction);
                            }
                            let changed = put
                                .as_mut()
                                .expect("legacy writer statement")
                                .execute(params![
                                    row.key,
                                    row.last.unwrap_or(0),
                                    row.harness,
                                    row.repo,
                                    row.parent,
                                    metadata
                                ])
                                .map_err(failure)?;
                            if !rebuild_search
                                && (changed > 0
                                    || initialize_search
                                    || !crate::catalog_search::row_present(&transaction, &row.key)
                                        .map_err(failure)?)
                            {
                                search_rows.push(row);
                            }
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
                        if bulk_changed.is_some()
                            && !rebuild_search
                            && (changed
                                || initialize_search
                                || !crate::catalog_search::row_present(&transaction, &row.key)
                                    .map_err(failure)?)
                        {
                            search_rows.push(row);
                        }
                        if changed || previous_version.as_deref() != Some(&version) {
                            let started = metadata_measurements.start();
                            remove_sources.execute([&row.key]).map_err(failure)?;
                            metadata_measurements.record("source_retirement", started, 1);
                            if let Some(sample) = sample.as_mut() {
                                sample.stage("source_retirement", &transaction);
                            }
                            for source in &row.sources {
                                let started = metadata_measurements.start();
                                let path = source.path.to_string_lossy();
                                transaction.execute("DELETE FROM catalog_native_dependencies WHERE source_path=?1 AND harness NOT LIKE 'lineage:%' AND harness NOT LIKE 'bridge:%'",[path.as_ref()]).map_err(failure)?;
                                let parent = source.native_role.as_ref().and_then(|role| {
                                    role.pointer("/Agent/parent")
                                        .and_then(Value::as_str)
                                        .or_else(|| {
                                            (role
                                                .pointer("/Codex/guardian_review")
                                                .and_then(Value::as_bool)
                                                != Some(true))
                                            .then(|| {
                                                role.pointer("/Codex/parent_thread")
                                                    .and_then(Value::as_str)
                                            })
                                            .flatten()
                                        })
                                });
                                if let Some(observations) = row
                                    .query
                                    .as_ref()
                                    .and_then(|query| query["rows"].as_object())
                                {
                                    for observation in observations.values() {
                                        if let Some(pid) = observation["summary"]["pid"].as_u64() {
                                            transaction.execute("INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'runtime:pid',?2)",params![path.as_ref(),pid.to_string()]).map_err(failure)?;
                                        }
                                    }
                                }
                                if let Some(parent) = parent {
                                    transaction.execute("INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,?2,?3)",params![path.as_ref(),row.harness,parent]).map_err(failure)?;
                                }
                                metadata_measurements.record("native_dependencies", started, 1);
                                let started = metadata_measurements.start();
                                put_source
                                    .execute(params![
                                        row.key,
                                        row.harness,
                                        source.native_id,
                                        source.path.to_string_lossy()
                                    ])
                                    .map_err(failure)?;
                                metadata_measurements.record("catalog_sources", started, 1);
                            }
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
                    transaction.execute("UPDATE session_catalog SET lifecycle='retained', metadata=CASE WHEN json_valid(metadata) THEN json_set(metadata,'$.lifecycle','retained') ELSE metadata END WHERE lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)", []).map_err(failure)?;
                    transaction.execute("UPDATE session_catalog_sources SET lifecycle='retained' WHERE lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)", []).map_err(failure)?;
                } else {
                    for key in &affected {
                        transaction.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key=?1 AND session_key NOT IN (SELECT session_key FROM catalog_members)",[key]).map_err(failure)?;
                        transaction.execute("UPDATE session_catalog_sources SET lifecycle='retained' WHERE session_key=?1 AND session_key NOT IN (SELECT session_key FROM catalog_members)",[key]).map_err(failure)?;
                    }
                }
                metadata_measurements.record("lifecycle_retirement", started, 1);
            }
            let started = metadata_measurements.start();
            if claims.is_none() {
                transaction.execute("UPDATE session_history_catalog SET metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key NOT IN (SELECT session_key FROM catalog_members)",[]).map_err(failure)?;
            } else {
                for key in &affected {
                    transaction.execute("UPDATE session_history_catalog SET metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key=?1 AND session_key NOT IN (SELECT session_key FROM catalog_members)",[key]).map_err(failure)?;
                }
            }
            metadata_measurements.record("history_retirement", started, 1);
            if let Some(producer) = &producer {
                metadata_measurements.emit(producer, "catalog_metadata");
                overview_measurements.emit(producer, "overview_publish");
            }
            stage("catalog_metadata", &transaction);
            #[cfg(test)]
            if let CatalogWriter::Traversal { owners, measured } = writer {
                crate::catalog_search::publish_current_rows_probe(
                    &transaction,
                    &search_rows,
                    owners,
                    measured.then_some(&mut search_observation),
                )
                .map_err(failure)?;
            } else {
                crate::catalog_search::publish_current_rows(&transaction, &search_rows)
                    .map_err(failure)?;
            }
            #[cfg(not(test))]
            crate::catalog_search::publish_current_rows(&transaction, &search_rows)
                .map_err(failure)?;
            stage("search_rows", &transaction);
            if rebuild_search {
                crate::catalog_search::begin_backfill(&transaction).map_err(failure)?;
                let mut statement = transaction
                    .prepare("SELECT metadata FROM session_catalog ORDER BY session_key")
                    .map_err(failure)?;
                let mut encoded = statement.query([]).map_err(failure)?;
                while let Some(value) = encoded.next().map_err(failure)? {
                    let encoded: String = value.get(0).map_err(failure)?;
                    let row: crate::model::summary::CatalogRow = serde_json::from_str(&encoded)
                        .map_err(|_| failure(rusqlite::Error::InvalidQuery))?;
                    crate::catalog_search::backfill_catalog_row(&transaction, &row)
                        .map_err(failure)?;
                }
            }
            if rebuild_search {
                crate::catalog_search::finish_backfill(&transaction).map_err(failure)?;
                transaction.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('search_backfill_complete','1')",[]).map_err(failure)?;
            }
            if claims.is_none() {
                crate::catalog_search::retire_missing(&transaction).map_err(failure)?;
            } else {
                for key in &affected {
                    crate::catalog_search::retire_key(&transaction, key).map_err(failure)?;
                }
            }
            stage("search", &transaction);
            let generation = {
                let mut digest = Sha256::new();
                digest.update(version.as_bytes());
                if claims.is_some() {
                    transaction.execute("INSERT INTO meta(key,value) VALUES ('catalog_publication_epoch','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)",[]).map_err(failure)?;
                    let epoch: String = transaction
                        .query_row(
                            "SELECT value FROM meta WHERE key='catalog_publication_epoch'",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(failure)?;
                    digest.update(previous.as_deref().unwrap_or("").as_bytes());
                    digest.update(epoch.as_bytes());
                    for value in &metadata {
                        digest.update((value.len() as u64).to_le_bytes());
                        digest.update(value.as_bytes());
                    }
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
                format!("{:x}", digest.finalize())
            };
            if previous.as_deref() != Some(&generation)
                || previous_version.as_deref() != Some(&version)
            {
                transaction.execute("INSERT OR REPLACE INTO meta(key,value) VALUES ('catalog_generation',?1),('catalog_version',?2)",params![generation,version]).map_err(failure)?;
            }
            // Closed component publication is coherent for its claimed closure.
            // Its owner advances the cohort observation clock only after all
            // pending revisions and any bounded metadata audit have completed.
            if !preserve_complete || claims.is_none() {
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO meta(key,value) VALUES ('catalog_observed_at',?1)",
                        [crate::model::now_ms().to_string()],
                    )
                    .map_err(failure)?;
            }
            stage("generation", &transaction);
            if let Some(projections) = publication.transcripts {
                let rows_by_key: BTreeMap<_, _> =
                    rows.iter().map(|row| (row.key.as_str(), row)).collect();
                for projection in projections {
                    let Some(row) = rows_by_key.get(projection.key.as_str()) else {
                        continue;
                    };
                    let metadata: Vec<String> = projection
                        .slots
                        .iter()
                        .map(json)
                        .collect::<Result<_, _>>()
                        .map_err(failure)?;
                    let source_generation = crate::retention::source_generation(&row.sources)
                        .map_err(|_| {
                            StoreError::Data("source projection metadata cannot serialize".into())
                        })?;
                    let slot_generation =
                        crate::slot_projection::generation(&metadata, &row.sources)
                            .map_err(|error| StoreError::Data(error.to_string()))?;
                    let changed = transaction.execute(
                        "INSERT INTO session_slot_projections(session_key,generation,version,total,source_generation) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,version=excluded.version,total=excluded.total,source_generation=excluded.source_generation WHERE session_slot_projections.generation != excluded.generation OR session_slot_projections.version != excluded.version OR session_slot_projections.source_generation IS NOT excluded.source_generation",
                        params![projection.key,slot_generation,crate::slot_projection::VERSION,i64::try_from(metadata.len()).map_err(|_| StoreError::Data("slot count exceeds SQL range".into()))?,source_generation]).map_err(failure)?;
                    if changed > 0 || projection.repair {
                        transaction
                            .execute(
                                "DELETE FROM session_slots WHERE session_key=?1",
                                [&projection.key],
                            )
                            .map_err(failure)?;
                        let mut put = transaction.prepare("INSERT INTO session_slots(session_key,slot,metadata) VALUES (?1,?2,?3)").map_err(failure)?;
                        for (slot, metadata) in metadata.iter().enumerate() {
                            put.execute(params![
                                projection.key,
                                i64::try_from(slot).map_err(|_| StoreError::Data(
                                    "slot index exceeds SQL range".into()
                                ))?,
                                metadata
                            ])
                            .map_err(failure)?;
                        }
                    }
                }
                if claims.is_none() {
                    transaction.execute("DELETE FROM session_slots WHERE session_key NOT IN (SELECT session_key FROM session_catalog)", []).map_err(failure)?;
                    transaction.execute("DELETE FROM session_slot_projections WHERE session_key NOT IN (SELECT session_key FROM session_catalog)", []).map_err(failure)?;
                }
                transaction.execute("INSERT OR REPLACE INTO meta(key,value) VALUES ('slot_projection_catalog_generation',?1)", [&generation]).map_err(failure)?;
            } else {
                transaction
                    .execute(
                        "DELETE FROM meta WHERE key='slot_projection_catalog_generation'",
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
                        "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
                        params![format!("search_generation_{scope}"), generation],
                    )
                    .map_err(failure)?;
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
                        params![format!("search_incomplete_{scope}"), incomplete.to_string()],
                    )
                    .map_err(failure)?;
            }
            transaction
                .execute(
                    "INSERT OR REPLACE INTO meta(key,value) VALUES('search_version',?1)",
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
                            transaction.execute("DELETE FROM session_catalog_invalidations WHERE source_path=?1 AND revision=?2",params![path,revision]).map_err(failure)?;
                        }
                    }
                } else {
                    transaction
                        .execute("DELETE FROM session_catalog_invalidations", [])
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
                    "INSERT OR REPLACE INTO meta(key,value) VALUES ('catalog_completeness',?1)",
                    [if claims.is_some() && !preserve_complete {
                        "partial"
                    } else {
                        "complete"
                    }],
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
        result.map(|outcome| (outcome, published))
    }

    fn attempt(path: &Path, legacy: Option<&Path>) -> Result<(Self, Loaded), Unopened> {
        Self::attempt_scoped(path, legacy, false)
    }

    fn attempt_scoped(
        path: &Path,
        legacy: Option<&Path>,
        scoped: bool,
    ) -> Result<(Self, Loaded), Unopened> {
        let classify = |error: rusqlite::Error| {
            if is_corrupt(&error) {
                Unopened::Corrupt(error)
            } else {
                Unopened::Other(error.to_string())
            }
        };
        create_private(path, &crate::state_dir())
            .map_err(|error| Unopened::Other(error.to_string()))?;
        // Never through a symlink: the path is checked above, and SQLite
        // refuses one too.
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(classify)?;
        match init(&mut connection).map_err(classify)? {
            Init::Ready => {}
            Init::NoWal => {
                return Err(Unopened::Other(
                    "its filesystem doesn't support WAL".to_owned(),
                ));
            }
            Init::Newer => {
                return Err(Unopened::Other(
                    "a newer version of semon wrote it".to_owned(),
                ));
            }
        }
        // The import never keeps the store from opening: it is tried again
        // at the next open.
        if let Some(legacy) = legacy
            && let Err(error) = import_legacy(&mut connection, legacy)
        {
            warn_import(legacy, &error);
        }
        let store = Self {
            connection,
            path: path.to_owned(),
            selected_runs: scoped.then(std::collections::BTreeSet::new),
            semantic_rejection: None,
            #[cfg(test)]
            traversal_observation: None,
        };
        let loaded = if scoped {
            Loaded {
                files: Vec::new(),
                reported_runs: Vec::new(),
                stamp: store.read_stamp().map_err(classify)?,
            }
        } else {
            store.load().map_err(classify)?
        };
        Ok((store, loaded))
    }

    fn read_stamp(&self) -> rusqlite::Result<Option<ReportedFileStamp>> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key='claude_json_stamp'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|text| serde_json::from_str(&text).ok()))
    }

    /// Every file's index, and the reported runs, in one read transaction.
    /// A file whose rows can't be read is left out: it is read from its log
    /// again when next seen, and its rows replaced.
    fn load(&self) -> rusqlite::Result<Loaded> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut files = Vec::new();
        {
            let mut readers = Readers::new(&transaction)?;
            let mut statement = transaction.prepare(&format!(
                "SELECT {FILE_COLUMNS} FROM files ORDER BY file_id"
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
                "SELECT value FROM meta WHERE key = 'claude_json_stamp'",
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
        let mut statement = transaction.prepare(NATIVE_METADATA_VALUE)?;
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
        let found = ledger_row(&transaction, path)?;
        if found.as_ref().map(|(_, ledger)| ledger) != expected {
            return Ok(Outcome::Conflict);
        }
        if changes.is_none()
            && let Some((file_id, _)) = found
        {
            clear(&transaction, file_id)?;
        }
        write_index_changes(&transaction, path, ledger, index, changes, expected)?;
        transaction.commit()?;
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
        for (path, expected) in files {
            match (ledger_row(&transaction, path), expected) {
                (Ok(Some((_, found))), Some(expected)) if found == *expected => {
                    if archived_index(&transaction, path)?
                        .is_some_and(|record| record.matches(&found))
                    {
                        // Explicit archive retention does not make this source
                        // current. Complete catalog observation still retires
                        // its membership; only the existing event rows survive.
                        // Re-observing this exact retained generation after its
                        // withdrawal must not advance the stage's own claim.
                        // A fresh announced revision or changed reason remains
                        // visible; neither the claim nor its proof is replaced.
                        let already_removed:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1 AND reason='removed')",[path],|row|row.get(0))?;
                        if !already_removed {
                            invalidate_catalog_source(&transaction, path, "removed")?;
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
                        "DELETE FROM {table} WHERE file_id IN \
                         (SELECT file_id FROM files WHERE path = ?1)"
                    ),
                    [path],
                )?;
            }
            transaction.execute(
                "DELETE FROM native_tree_headers WHERE source_path=?1",
                [path],
            )?;
            transaction.execute("DELETE FROM files WHERE path = ?1", [path])?;
            transaction.execute("DELETE FROM catalog_native_dependencies WHERE source_path=?1 AND harness IN ('semantic-native:codex','semantic-parent:codex')", [path])?;
            transaction.execute("DELETE FROM meta WHERE key=?1", [archive_index_key(path)])?;
            transaction.execute("DELETE FROM meta WHERE key=?1", [codex_header_key(path)])?;
            transaction.execute("DELETE FROM meta WHERE key=?1", [claude_sidecar_key(path)])?;
            chunk_proofs::clear(&transaction, path)?;
            launch_calls::clear(&transaction, path)?;
            transaction.execute("DELETE FROM meta WHERE key=?1", [parent_marker_key(path)])?;
            transaction.execute("DELETE FROM meta WHERE key=?1", [tree_summary_key(path)])?;
            invalidate_catalog_source(&transaction, path, "removed")?;
        }
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !current(&transaction)? {
            return Ok(None);
        }
        {
            let mut put = transaction.prepare(PUT_RUN)?;
            for run in runs {
                put_run(&mut put, run)?;
            }
        }
        transaction.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('claude_json_stamp', ?1)",
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

/// The file isn't a database, or is damaged.
fn is_corrupt(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
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

/// Creates the store file owner-only (`0600`) when missing, so it is never
/// readable by others, not even briefly; SQLite creates `-wal` and `-shm`
/// with the main file's mode. A store left looser by anything else is
/// tightened. Missing directories are created `0700`; an existing one is
/// tightened only as [`tightened`] allows, and only when this user owns it.
/// A symlink at the store's path is refused.
fn create_private(path: &Path, semon_state: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(parent)?;
    }
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("its path is a symlink"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(O_NOFOLLOW);
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for file in [
            path.to_owned(),
            sibling(path, "-wal"),
            sibling(path, "-shm"),
        ] {
            if let Ok(metadata) = fs::symlink_metadata(&file)
                && metadata.is_file()
                && metadata.permissions().mode() & 0o077 != 0
            {
                fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
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

/// Renames a damaged store to `<path>.corrupt` and drops its `-wal` and
/// `-shm`, which belong to it and must not meet the new file.
fn set_aside(path: &Path) -> io::Result<()> {
    // The WAL first: were it left, even for a moment, beside a new file at
    // the path, SQLite could replay it into that file.
    for suffix in ["-wal", "-shm"] {
        match fs::remove_file(sibling(path, suffix)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    fs::rename(path, sibling(path, ".corrupt"))
}

/// CI-only paired measurements reuse the producer's connection-local policy.
#[cfg(test)]
pub(crate) fn catalog_probe_connection(path: &Path) -> rusqlite::Result<Connection> {
    let mut connection = Connection::open(path)?;
    if matches!(init(&mut connection)?, Init::Ready) {
        Ok(connection)
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

/// Readies a connection. The versions are read without a lock, so a store
/// that is current opens even while another process writes; the write lock
/// is taken only to create or migrate it.
fn init(connection: &mut Connection) -> rusqlite::Result<Init> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    // Reuse the small set of per-source ledger/index statements. Cached SQL
    // clears bound values on return; this retains neither source bodies nor
    // authority and never skips the transaction's generation checks.
    connection.set_prepared_statement_cache_capacity(32);
    let mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Ok(Init::NoWal);
    }
    let _: i64 = connection.query_row(
        &format!("PRAGMA journal_size_limit = {JOURNAL_SIZE_LIMIT}"),
        [],
        |row| row.get(0),
    )?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    // One read transaction, so the two versions are read together.
    let read = connection.transaction()?;
    let found = versions(&read)?;
    read.commit()?;
    match found {
        (schema, _) if schema > SCHEMA_VERSION => return Ok(Init::Newer),
        (_, Some(parser)) if parser > CACHE_VERSION => return Ok(Init::Newer),
        (SCHEMA_VERSION, Some(CACHE_VERSION)) => return Ok(Init::Ready),
        _ => {}
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Read again under the lock: another process may have migrated it.
    let (schema, parser) = versions(&transaction)?;
    if schema > SCHEMA_VERSION || parser.is_some_and(|parser| parser > CACHE_VERSION) {
        return Ok(Init::Newer);
    }
    if schema < SCHEMA_VERSION {
        if schema != 0
            && !transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('files') WHERE name='ctime_ns')",
                [],
                |row| row.get::<_, bool>(0),
            )?
        {
            transaction.execute_batch("ALTER TABLE files ADD COLUMN ctime_ns BLOB")?;
        }
        if schema == 6 {
            transaction.execute_batch("ALTER TABLE session_catalog ADD COLUMN parent_key TEXT")?;
        }
        if (6..11).contains(&schema) && !transaction.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_catalog') WHERE name='lifecycle')", [], |row| row.get::<_,bool>(0))? {
            transaction.execute_batch("ALTER TABLE session_catalog ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'current' CHECK(lifecycle IN ('current','retained')); ")?;
        }
        if (7..11).contains(&schema) && !transaction.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_catalog_sources') WHERE name='lifecycle')", [], |row| row.get::<_,bool>(0))? {
            transaction.execute_batch("ALTER TABLE session_catalog_sources ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'current' CHECK(lifecycle IN ('current','retained')); ")?;
        }
        if schema==10 && !transaction.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_slot_projections') WHERE name='source_generation')",[],|row|row.get::<_,bool>(0))? {
            transaction.execute_batch("ALTER TABLE session_slot_projections ADD COLUMN source_generation TEXT")?;
        }
        // Earlier schemas used lifecycle-prefixed indexes containing retained rows.
        // Replace only the Current list trees; history/source lookup indexes
        // still support both lifecycle states and keep their existing authority.
        if (11..17).contains(&schema) {
            transaction.execute_batch(
                "DROP INDEX IF EXISTS session_catalog_current_order;
                DROP INDEX IF EXISTS session_catalog_current_harness;
                DROP INDEX IF EXISTS session_catalog_current_repo;
                DROP INDEX IF EXISTS session_catalog_current_harness_repo;
                DROP INDEX IF EXISTS session_catalog_current_parent;",
            )?;
        }
        transaction.execute_batch(SCHEMA)?;
        transaction.execute_batch(native_usage::SCHEMA)?;
        if !transaction.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('native_usage_sources') WHERE name='usage_ready')",[],|r|r.get::<_,bool>(0))? {
            transaction.execute_batch("ALTER TABLE native_usage_sources ADD COLUMN usage_ready INTEGER NOT NULL DEFAULT 1")?;
        }
        transaction.execute_batch(tree_state::SCHEMA)?;
        // New usage identities are maintained by the installed copy triggers.
        // Only files already present at the additive upgrade need bootstrap.
        transaction.execute("INSERT INTO meta(key,value) SELECT 'native_tree_legacy_max_file',CAST(COALESCE(MAX(file_id),0) AS TEXT) FROM files WHERE true ON CONFLICT(key) DO NOTHING",[])?;
        transaction.execute("INSERT INTO meta(key,value) SELECT 'native_usage_discovery_done','1' WHERE NOT EXISTS(SELECT 1 FROM files) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[])?;
        transaction.execute_batch(semantic_stage::SCHEMA)?;
        transaction.execute_batch(crate::overview::SCHEMA)?;
        transaction.execute_batch(crate::causal_trace::SCHEMA)?;
        transaction.execute_batch(crate::history_projection::SCHEMA)?;
        transaction.execute_batch(crate::catalog_search::SCHEMA)?;
        // Format 2 also reads the existing format-1 postings. Promote only its
        // metadata in this additive transaction, before the publication path
        // could interpret a supported upgrade as a missing search projection.
        transaction.execute(
            "UPDATE meta SET value=?1 WHERE key='search_version' AND value='1'",
            [crate::catalog_search::VERSION],
        )?;
        transaction.execute_batch(crate::catalog_changes::SCHEMA)?;
        if schema < 18 {
            // One-time metadata bootstrap; requests only read this durable
            // stream. Scope indexes permit useful history without consuming
            // every current-list record first.
            transaction.execute_batch("INSERT INTO session_catalog_changes(session_key,read_scope) SELECT session_key,'current' FROM session_catalog ORDER BY last_ms DESC,session_key ASC; INSERT INTO session_catalog_changes(session_key,read_scope) SELECT session_key,'retained_history' FROM (SELECT session_key FROM session_catalog UNION SELECT session_key FROM session_history_catalog) ORDER BY session_key;")?;
        } else if schema == 18 {
            // Schema 18 omitted Current fallback keys from the initial History
            // stream. Append recovery hints without resetting any saved cursor.
            transaction.execute_batch("INSERT INTO session_catalog_changes(session_key,read_scope) SELECT session_key,'retained_history' FROM session_catalog ORDER BY session_key;")?;
        }
        if schema == 1 {
            transaction.execute_batch("ALTER TABLE files ADD COLUMN codex_native_usage TEXT")?;
        }
        if schema == 1 || schema == 2 {
            transaction.execute_batch(
                "ALTER TABLE usage ADD COLUMN record_ids TEXT NOT NULL DEFAULT '[]'",
            )?;
        }
        if (1..=3).contains(&schema) {
            transaction.execute_batch("ALTER TABLE files ADD COLUMN copilot TEXT")?;
        }
        if (4..=8).contains(&schema) {
            // Existing record identities are metadata, not source bodies.
            // Corrupt usage JSON remains unreadable and is rebuilt from source
            // by the existing loader; it must not block additive migration.
            transaction.execute_batch("INSERT INTO usage_identity(file_id,message_id,record_uuid)
                SELECT usage.file_id,usage.message_id,identity.value FROM usage,
                json_each(CASE WHEN json_valid(usage.record_ids) THEN
                    CASE WHEN json_type(usage.record_ids)='array' THEN usage.record_ids ELSE '[]' END
                    ELSE '[]' END) AS identity
                WHERE identity.type='text'
                ON CONFLICT DO NOTHING;")?;
        }
        if schema == 10 {
            let coherent:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM meta a JOIN meta b ON a.value=b.value WHERE a.key='catalog_generation' AND b.key='slot_projection_catalog_generation')",[],|row|row.get(0))?;
            if coherent {
                let mut statement=transaction.prepare("SELECT session_catalog.metadata FROM session_catalog JOIN session_slot_projections USING(session_key) WHERE session_slot_projections.version=?1 AND session_slot_projections.source_generation IS NULL")?;
                let metadata = statement
                    .query_map([crate::slot_projection::VERSION], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                for metadata in metadata {
                    if let Ok(row) =
                        serde_json::from_str::<crate::model::summary::CatalogRow>(&metadata)
                        && let Ok(source_generation) =
                            crate::retention::source_generation(&row.sources)
                    {
                        transaction.execute("UPDATE session_slot_projections SET source_generation=?1 WHERE session_key=?2",params![source_generation,row.key])?;
                    }
                }
            }
        }
        if schema < 21 {
            transaction.execute_batch("INSERT OR IGNORE INTO meta(key,value) SELECT 'overview_needed:'||c.session_key,'1' FROM session_catalog c LEFT JOIN session_overview o USING(session_key) LEFT JOIN session_trace_headers t USING(session_key) WHERE o.session_key IS NULL OR t.session_key IS NULL;")?;
        }
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    let wiped = parser_version(&transaction)? != Some(CACHE_VERSION);
    if wiped {
        transaction.execute_batch(DERIVED)?;
        transaction.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('cache_version', ?1)",
            [CACHE_VERSION.to_string()],
        )?;
    }
    transaction.commit()?;
    if wiped {
        // Not fatal: the automatic checkpoint and the size limit follow.
        checkpoint(connection).ok();
    }
    Ok(Init::Ready)
}

/// The schema and parser versions; no parser before the schema exists.
fn versions(connection: &Connection) -> rusqlite::Result<(i64, Option<u32>)> {
    let schema: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let parser = if schema >= 1 {
        parser_version(connection)?
    } else {
        None
    };
    Ok((schema, parser))
}

/// Moves the WAL into the database and truncates it, after a write as
/// large as the store. Busy readers only defer it: the next automatic
/// checkpoint and [`JOURNAL_SIZE_LIMIT`] cut it back later.
fn checkpoint(connection: &Connection) -> rusqlite::Result<()> {
    connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
}

fn parser_version(connection: &Connection) -> rusqlite::Result<Option<u32>> {
    Ok(connection
        .prepare_cached("SELECT value FROM meta WHERE key = 'cache_version'")?
        .query_row([], |row| row.get::<_, String>(0))
        .optional()?
        .and_then(|value| value.parse().ok()))
}

/// The store still has this binary's schema and parser: no other version
/// rebuilt it since it was opened.
fn current(connection: &Connection) -> rusqlite::Result<bool> {
    let schema: i64 = connection
        .prepare_cached("PRAGMA user_version")?
        .query_row([], |row| row.get(0))?;
    Ok(schema == SCHEMA_VERSION && parser_version(connection)? == Some(CACHE_VERSION))
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
        ((self.version == 1 && self.native_header_sha256.is_none())
            || (self.version == 2
                && self.source.root == "codex"
                && self.native_header_sha256.as_ref().is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                }))
            || (self.version == 3
                && self.source.root == "claude"
                && crate::native_sidecar::agent_path(Path::new(&self.source.path))
                && self.native_header_sha256.as_ref().is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                }))
            || (matches!(self.version, 4..=6)
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
                })))
            && (matches!(self.version, 4..=6) || self.source_chunks_sha256.is_none())
            && (if matches!(self.version, 5 | 6)
                && matches!(self.source.root.as_str(), "claude" | "codex")
            {
                self.parent_marker_sha256.as_ref().is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            } else {
                self.parent_marker_sha256.is_none()
            })
            && (if self.version == 6 {
                self.launch_calls_sha256.as_ref().is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            } else {
                self.launch_calls_sha256.is_none()
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
    let mut count: i64 =
        connection.query_row("SELECT count(*) FROM files", [], |row| row.get(0))?;
    let mut statement = connection.prepare(
        "SELECT f.file_id,f.dev,f.ino,f.size,f.mtime_ns,f.resume_at,f.head_sha256,f.tail_sha256,f.ctime_ns,f.path,\
         CASE WHEN octet_length(m.value)<=16384 THEN m.value ELSE NULL END \
         FROM meta m JOIN files f ON f.path=substr(m.key,length(?1)+1) WHERE m.key GLOB ?2",
    )?;
    let mut rows = statement.query(["archive_event_index:", "archive_event_index:*"])?;
    while let Some(row) = rows.next()? {
        let path: String = row.get(9)?;
        if sources.contains_key(&path) {
            continue;
        }
        let encoded: Option<String> = row.get(10)?;
        let record =
            encoded.and_then(|encoded| serde_json::from_str::<ArchivedIndex>(&encoded).ok());
        if let Some(record) = record {
            let (_, ledger) = read_ledger(row)?;
            if record.matches(&ledger) {
                count -= 1;
            }
        }
    }
    Ok(count)
}

fn archived_index(connection: &Connection, path: &str) -> rusqlite::Result<Option<ArchivedIndex>> {
    let encoded: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN octet_length(value)<=16384 THEN value ELSE NULL END FROM meta WHERE key=?1",
        [archive_index_key(path)], |row| row.get(0),
    ).optional()?;
    Ok(encoded
        .flatten()
        .and_then(|value| serde_json::from_str(&value).ok()))
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
    connection.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![archive_index_key(path), encoded])?;
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
        let mut statement = connection.prepare(NATIVE_METADATA_VALUE)?;
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
            let mut statement = connection.prepare(NATIVE_METADATA_VALUE)?;
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

fn untag<T: DeserializeOwned>(column: usize, text: String) -> rusqlite::Result<T> {
    serde_json::from_value(Value::String(text)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })
}

const NATIVE_METADATA_MAX: usize = 1024 * 1024;
const NATIVE_METADATA_VALUE: &str =
    "SELECT CASE WHEN octet_length(value)<=1048576 THEN value ELSE NULL END FROM meta WHERE key=?1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeSummaryRecord {
    version: u32,
    parser_version: u32,
    ledger: String,
    summary: crate::native_tree::Summary,
}

fn tree_summary_key(path: &str) -> String {
    format!("native_tree_summary:{path}")
}

fn read_tree_summary(
    connection: &Connection,
    statement: &mut Statement<'_>,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<crate::native_tree::Summary>> {
    let encoded: Option<Option<String>> = statement
        .query_row([tree_summary_key(path)], |row| {
            crate::selected_read::row(row, 512)?;
            row.get(0)
        })
        .optional()?;
    let Some(text) = encoded.flatten() else {
        return Ok(None);
    };
    let Some(record) = serde_json::from_str::<TreeSummaryRecord>(&text).ok() else {
        return Ok(None);
    };
    if !(1..=3).contains(&record.version)
        || record.parser_version != CACHE_VERSION
        || record.ledger != header_ledger(ledger)
    {
        return Ok(None);
    }
    let mut summary = record.summary;
    if record.version == 3 && !tree_state::restore(connection, path, ledger, &mut summary)? {
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
    let encoded = summary
        .map(|summary| {
            tree_state::write(transaction, path, ledger, summary)?;
            json(&TreeSummaryRecord {
                version: 3,
                parser_version: CACHE_VERSION,
                ledger: header_ledger(ledger),
                summary: summary.compact(normalized_usage),
            })
        })
        .transpose()?;
    if let Some(encoded) = encoded {
        if encoded.len() > NATIVE_METADATA_MAX {
            // A pathological scalar is explicitly unavailable for this
            // receipt. Retain old compatibility data; never delete and retry
            // the same oversized representation on every unchanged read.
            transaction.execute("INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![format!("native_tree_failure:{path}"),header_ledger(ledger)])?;
            if normalized_usage {
                transaction.execute("UPDATE native_usage_migrations SET state='blocked_metadata' WHERE source_path=?1",[path])?;
            } else {
                // An oversized compatibility accumulator is precisely the
                // migration's input, not a reason to block that worker.
                transaction.execute("INSERT INTO native_usage_migrations(source_path,receipt,usage_needed) VALUES(?1,?2,1) ON CONFLICT(source_path) DO UPDATE SET state=CASE WHEN native_usage_migrations.state='done' THEN 'ready' ELSE native_usage_migrations.state END",params![path,header_ledger(ledger)])?;
            }
            return Ok(());
        }
        transaction.prepare_cached("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value")?.execute(params![tree_summary_key(path), encoded])?;
        transaction.execute(
            "DELETE FROM meta WHERE key=?1",
            [format!("native_tree_failure:{path}")],
        )?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexHeaderRecord {
    version: u32,
    parser_version: u32,
    ledger: String,
    header: crate::native_header::CodexHeader,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentMarkerRecord {
    version: u32,
    parser_version: u32,
    ledger: String,
    index: crate::native_marker::Index,
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
        .query_row(NATIVE_METADATA_VALUE, [parent_marker_key(path)], |row| {
            row.get(0)
        })
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
    if let Some(index) = index.filter(|index| index.valid(ledger)) {
        let encoded = json(&ParentMarkerRecord {
            version: 1,
            parser_version: CACHE_VERSION,
            ledger: header_ledger(ledger),
            index: index.clone(),
        })?;
        transaction.prepare_cached("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value")?.execute(params![parent_marker_key(path), encoded])?;
    } else {
        transaction
            .prepare_cached("DELETE FROM meta WHERE key=?1")?
            .execute([parent_marker_key(path)])?;
    }
    Ok(())
}

fn codex_header_key(path: &str) -> String {
    format!("native_codex_header:{path}")
}

fn header_ledger(ledger: &Ledger) -> String {
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
        .query_row(NATIVE_METADATA_VALUE, [codex_header_key(path)], |row| {
            row.get(0)
        })
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
    let encoded = header
        .map(|header| {
            json(&CodexHeaderRecord {
                version: 1,
                parser_version: CACHE_VERSION,
                ledger: header_ledger(ledger),
                header: header.clone(),
            })
        })
        .transpose()?;
    if let Some(encoded) = encoded.filter(|text| text.len() <= NATIVE_METADATA_MAX) {
        transaction.prepare_cached("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value")?.execute(params![codex_header_key(path), encoded])?;
    } else {
        // Current readers keep their complete in-memory header. An oversized
        // projection cannot authorize eviction; do not truncate native fields.
        transaction
            .prepare_cached("DELETE FROM meta WHERE key=?1")?
            .execute([codex_header_key(path)])?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeSidecarRecord {
    version: u32,
    parser_version: u32,
    ledger: String,
    sidecar: crate::native_sidecar::ClaudeSidecar,
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
        .query_row(NATIVE_METADATA_VALUE, [claude_sidecar_key(path)], |row| {
            row.get(0)
        })
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
    let encoded = sidecar
        .map(|sidecar| {
            json(&ClaudeSidecarRecord {
                version: 1,
                parser_version: CACHE_VERSION,
                ledger: header_ledger(ledger),
                sidecar: sidecar.clone(),
            })
        })
        .transpose()?;
    if let Some(encoded) = encoded.filter(|text| text.len() <= NATIVE_METADATA_MAX) {
        transaction.prepare_cached("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value")?.execute(params![claude_sidecar_key(path), encoded])?;
    } else {
        transaction
            .prepare_cached("DELETE FROM meta WHERE key=?1")?
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

fn ledger_row(connection: &Connection, path: &str) -> rusqlite::Result<Option<(i64, Ledger)>> {
    connection
        .prepare_cached(
            "SELECT file_id, dev, ino, size, mtime_ns, resume_at, head_sha256, tail_sha256, ctime_ns \
             FROM files WHERE path = ?1",
        )?
        .query_row(
            [path],
            read_ledger,
        )
        .optional()
}

fn read_ledger(row: &Row<'_>) -> rusqlite::Result<(i64, Ledger)> {
    Ok((
        row.get(0)?,
        Ledger {
            stat: Stat {
                dev: uint(row.get(1)?),
                ino: uint(row.get(2)?),
                size: uint(row.get(3)?),
                modified_ns: u128::from_be_bytes(row.get(4)?),
                changed_ns: row.get::<_, Option<[u8; 16]>>(8)?.map(i128::from_be_bytes),
            },
            offset: uint(row.get(5)?),
            prefix: row.get(6)?,
            tail: row.get(7)?,
        },
    ))
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
                "SELECT {EVENT_COLUMNS} FROM events WHERE file_id = ?1 ORDER BY extra, seq"
            ))?,
            signals: connection.prepare(&format!(
                "SELECT {SIGNAL_COLUMNS} FROM signals WHERE file_id = ?1 ORDER BY seq"
            ))?,
            usage: connection.prepare(&format!(
                "SELECT {USAGE_COLUMNS} FROM usage WHERE file_id = ?1"
            ))?,
            codex_usage: connection.prepare(&format!(
                "SELECT {CODEX_USAGE_COLUMNS} FROM codex_usage WHERE file_id = ?1 ORDER BY seq"
            ))?,
            codex_header: connection.prepare(NATIVE_METADATA_VALUE)?,
            claude_sidecar: connection.prepare(NATIVE_METADATA_VALUE)?,
            parent_marker: connection.prepare(NATIVE_METADATA_VALUE)?,
            tree_summary: connection.prepare(NATIVE_METADATA_VALUE)?,
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
        signals.push(Signal {
            k: untag(1, signal_row.get(1)?)?,
            o: uint(signal_row.get(2)?),
            t: signal_row.get(3)?,
            at: signal_row.get(4)?,
            n: signal_row.get(5)?,
            v: signal_row.get::<_, Option<i64>>(6)?.map(uint),
        });
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
        codex_usage_events.push(CodexUsageEvent {
            model: codex_row.get(1)?,
            tokens: ModelTokens {
                input: uint(codex_row.get(2)?),
                output: uint(codex_row.get(3)?),
                cache_write: uint(codex_row.get(4)?),
                cache_read: uint(codex_row.get(5)?),
            },
            timestamp: codex_row.get(6)?,
        });
    }

    let index = FileIndex {
        dirty_rows: None,
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
        pending: from_json(18, &row.get::<_, String>(18)?)?,
        tool_ids: from_json(19, &row.get::<_, String>(19)?)?,
        yields: from_json(20, &row.get::<_, String>(20)?)?,
        busy: from_json(21, &row.get::<_, String>(21)?)?,
        links: from_json(22, &row.get::<_, String>(22)?)?,
        entrypoint: row.get(9)?,
        title: row.get(10)?,
        agent_name: row.get(11)?,
        last_model: row.get(12)?,
        failed: row.get(13)?,
        first: row.get(14)?,
        last: row.get(15)?,
        cwd: row.get(16)?,
        branch: row.get(17)?,
        usage_by_id,
        codex_tokens: from_json(23, &row.get::<_, String>(23)?)?,
        codex_tokens_by_model: from_json(24, &row.get::<_, String>(24)?)?,
        codex_usage_events,
        copilot: row
            .get::<_, Option<String>>(27)?
            .map(|text| from_json(27, &text))
            .transpose()?,
        codex_native_usage: native_usage::restore(
            readers.connection,
            &path,
            &ledger,
            row.get(26)?,
        )?,
        rate_limits: row
            .get::<_, Option<String>>(25)?
            .map(|text| from_json(25, &text))
            .transpose()?,
    };
    Ok((path, ledger, index))
}

/// One `events` row, selected as [`EVENT_COLUMNS`].
fn read_event(row: &Row<'_>) -> rusqlite::Result<(bool, usize, Event)> {
    let y = row
        .get::<_, Option<String>>(12)?
        .map(|text| from_json::<Yield>(12, &text).map(Box::new))
        .transpose()?;
    let r = if row.get::<_, bool>(14)? {
        Some(Reply {
            o: uint(row.get(15)?),
            b: row.get(16)?,
            t: row.get(17)?,
            e: row.get(18)?,
            f: row.get(19)?,
            m: row.get(20)?,
        })
    } else {
        None
    };
    let peer = if row.get::<_, bool>(22)? {
        Some(Peer {
            sock: row.get(23)?,
            pid: row.get::<_, Option<i64>>(24)?.map(uint),
            start: row.get(25)?,
        })
    } else {
        None
    };
    let event = Event {
        k: untag(2, row.get(2)?)?,
        o: uint(row.get(3)?),
        b: row.get(4)?,
        t: row.get(5)?,
        id: row.get(6)?,
        n: row.get(7)?,
        code_mode: row.get(8)?,
        code_mode_open: row.get(9)?,
        parent: row.get::<_, Option<i64>>(10)?.map(seq_of),
        script: row.get(11)?,
        y,
        poll: row.get::<_, Option<i64>>(13)?.map(seq_of),
        r,
        item: row.get::<_, Option<i64>>(21)?.map(uint),
        peer,
        u: row.get(26)?,
    };
    Ok((row.get(0)?, seq_of(row.get(1)?), event))
}

/// One `usage` row, selected as [`USAGE_COLUMNS`].
fn read_usage(row: &Row<'_>) -> rusqlite::Result<(String, MessageUsage)> {
    let usage = MessageUsage {
        record_ids: from_json(22, &row.get::<_, String>(22)?)?,
        model: row.get(1)?,
        tokens: Tokens {
            input: uint(row.get(2)?),
            cached_input: uint(row.get(3)?),
            output: uint(row.get(4)?),
            reasoning_output: uint(row.get(5)?),
            total: uint(row.get(6)?),
        },
        model_tokens: ModelTokens {
            input: uint(row.get(7)?),
            output: uint(row.get(8)?),
            cache_write: uint(row.get(9)?),
            cache_read: uint(row.get(10)?),
        },
        billing: BillingUsage {
            input: uint(row.get(11)?),
            output: uint(row.get(12)?),
            cache_read: uint(row.get(13)?),
            cache_write_5m: uint(row.get(14)?),
            cache_write_1h: uint(row.get(15)?),
            web_search_requests: uint(row.get(16)?),
            speed: row.get(17)?,
            service_tier: row.get(18)?,
            prompt_size: uint(row.get(19)?),
            timestamp: row.get(20)?,
            split_unknown: row.get(21)?,
        },
    };
    Ok((row.get(0)?, usage))
}

fn read_runs(connection: &Connection) -> rusqlite::Result<Vec<ReportedRunSnapshot>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM reported_runs ORDER BY session_id, start"
    ))?;
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
    let mut statement = connection.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM reported_runs WHERE session_id=?1 ORDER BY start"
    ))?;
    let mut runs = Vec::new();
    for id in ids {
        let mut rows = statement.query([id])?;
        while let Some(row) = rows.next()? {
            runs.push(read_run(row)?);
        }
    }
    Ok(runs)
}

fn read_run(row: &Row<'_>) -> rusqlite::Result<ReportedRunSnapshot> {
    Ok(ReportedRunSnapshot {
        last_session_id: row.get(0)?,
        last_start_time: row.get(1)?,
        last_cost: row.get(2)?,
        last_duration: row.get::<_, Option<i64>>(3)?.map(uint),
        last_api_duration: row.get::<_, Option<i64>>(4)?.map(uint),
        last_tool_duration: row.get::<_, Option<i64>>(5)?.map(uint),
        last_lines_added: row.get::<_, Option<i64>>(6)?.map(uint),
        last_lines_removed: row.get::<_, Option<i64>>(7)?.map(uint),
        last_model_usage: from_json(8, &row.get::<_, String>(8)?)?,
        capture_at: row.get(9)?,
    })
}

// ---- Writing -------------------------------------------------------------------------------

/// Deletes a file's child rows, keeping its `files` row and id.
fn clear(transaction: &Transaction<'_>, file_id: i64) -> rusqlite::Result<()> {
    for table in [
        "events",
        "signals",
        "usage_identity",
        "usage",
        "codex_usage",
        "native_tree_copied_identities",
    ] {
        transaction.execute(
            &format!("DELETE FROM {table} WHERE file_id = ?1"),
            [file_id],
        )?;
    }
    transaction.execute(
        "DELETE FROM native_tree_headers WHERE file_id=?1",
        [file_id],
    )?;
    Ok(())
}

/// Writes touched rows and the appended tail, deleting a truncated tail
/// only for synthetic deltas (the append parser never removes rows).
fn changed<T>(
    before: usize,
    touched: &std::collections::BTreeSet<usize>,
    new: &[T],
    mut put: impl FnMut(usize, &T) -> rusqlite::Result<()>,
    cut: impl FnOnce(usize) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    let mut rows = touched.clone();
    rows.extend(before..new.len());
    for seq in rows {
        if let Some(row) = new.get(seq) {
            put(seq, row)?;
        }
    }
    if new.len() < before {
        cut(new.len())?;
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
) -> rusqlite::Result<()> {
    write_index_changes_measured(
        transaction,
        path,
        ledger,
        index,
        changes,
        expected,
        &mut crate::native_timing::Measurements::default(),
    )
}

fn write_index_changes_measured(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    changes: Option<&super::DirtyRows>,
    expected: Option<&Ledger>,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    #[cfg(test)]
    let written_before: i64 = transaction.query_row("SELECT total_changes()", [], |r| r.get(0))?;
    let started = measurements.start();
    // The source ledger fences raw bytes; the sidecar is an independent
    // interpretation dependency. Re-encoding derived proofs changes neither.
    let semantic_change = expected != Some(ledger)
        || (crate::native_sidecar::agent_path(Path::new(path)) && {
            let mut statement = transaction.prepare(NATIVE_METADATA_VALUE)?;
            read_claude_sidecar(&mut statement, path, ledger)?.as_ref()
                != index.claude_sidecar.as_ref()
        });
    relocation::advance(transaction, path, expected, ledger, changes.is_some())?;
    native_usage::begin_source(transaction, path, expected, ledger, changes.is_some())?;
    measurements.record("relocation", started, 1);
    let started = measurements.start();
    let full = super::DirtyRows {
        usage: if changes.is_none() {
            index.usage_by_id.keys().cloned().collect()
        } else {
            Default::default()
        },
        ..super::DirtyRows::default()
    };
    measurements.record("dirty_rows", started, 1);
    write_file(
        transaction,
        path,
        ledger,
        changes.unwrap_or(&full),
        index,
        measurements,
    )?;
    native_usage::write(transaction, path, ledger, index)?;
    let started = measurements.start();
    transaction.execute("DELETE FROM catalog_native_dependencies WHERE source_path=?1 AND (harness LIKE 'lineage:%' OR harness LIKE 'bridge:%')", [path])?;
    for id in index
        .links
        .session_ids
        .iter()
        .chain(&index.links.continued_in)
    {
        transaction.execute(
            "INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'lineage:claude',?2)",
            params![path, id],
        )?;
    }
    for id in &index.links.bridges {
        transaction.execute(
            "INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'bridge:claude',?2)",
            params![path, id],
        )?;
    }
    measurements.record("dependencies", started, 1);

    let started = measurements.start();
    transaction.execute("DELETE FROM catalog_native_dependencies WHERE source_path=?1 AND harness IN ('semantic-native:codex','semantic-parent:codex')", [path])?;
    if let Some(header) = &index.codex_header {
        if let Some(id) = header.native_id(Path::new(path)) {
            transaction.execute("INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'semantic-native:codex',?2)", params![path,id])?;
        }
        let metadata =
            serde_json::to_value(&header.metadata).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if metadata.get("guardian_review").and_then(Value::as_bool) != Some(true)
            && let Some(parent) = metadata.get("parent_thread").and_then(Value::as_str)
        {
            transaction.execute("INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'semantic-parent:codex',?2)", params![path,parent])?;
        }
    }
    measurements.record("semantic_dependencies", started, 1);
    let started = measurements.start();
    transaction.execute("DELETE FROM meta WHERE key=?1", [archive_index_key(path)])?;
    measurements.record("archive_metadata", started, 1);
    let started = measurements.start();
    if semantic_change {
        invalidate_catalog_source(transaction, path, "changed")?;
        measurements.record("invalidation", started, 1);
    } else {
        measurements.record("representation_repair", started, 1);
    }
    let started = measurements.start();
    transaction.execute(
        "DELETE FROM session_source_mutations WHERE source_path=?1",
        [path],
    )?;
    measurements.record("mutation_retirement", started, 1);
    #[cfg(test)]
    {
        let written_after: i64 =
            transaction.query_row("SELECT total_changes()", [], |r| r.get(0))?;
        super::INDEX_WRITTEN_ROWS
            .with(|n| n.set(n.get() + (written_after - written_before) as u64));
    }
    Ok(())
}

/// Upserts `path`'s `files` row, then writes its child rows where `index`
/// differs from `old`.
fn write_file(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    changes: &super::DirtyRows,
    index: &FileIndex,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let FileIndex {
        dirty_rows: _,
        native_usage_observations: _,
        tree_summary,
        source_chunks,
        marker_index,
        handoff_index,
        codex_header,
        claude_sidecar,
        events,
        extras,
        signals,
        pending,
        tool_ids,
        yields,
        busy,
        links,
        entrypoint,
        title,
        agent_name,
        last_model,
        failed,
        first,
        last,
        cwd,
        branch,
        usage_by_id,
        codex_tokens,
        codex_tokens_by_model,
        codex_usage_events,
        codex_native_usage: _,
        copilot,
        rate_limits,
    } = index;
    let Ledger {
        stat:
            Stat {
                dev,
                ino,
                size,
                modified_ns,
                changed_ns,
            },
        offset,
        prefix,
        tail,
    } = ledger;
    let started = measurements.start();
    write_codex_header(transaction, path, ledger, codex_header.as_ref())?;
    measurements.record("proof_header", started, 1);
    let started = measurements.start();
    write_claude_sidecar(transaction, path, ledger, claude_sidecar.as_ref())?;
    measurements.record("proof_sidecar", started, 1);
    let started = measurements.start();
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
    let started = measurements.start();
    write_parent_marker(transaction, path, ledger, marker_index.as_ref())?;
    measurements.record("proof_parent", started, 1);
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
    let started = measurements.start();
    let rate_limits = rate_limits.as_ref().map(json).transpose()?;
    measurements.record("rate_limit_serialization", started, 1);
    let file_id: i64 = {
        let started = measurements.start();
        let mut statement = transaction.prepare_cached(PUT_FILE)?;
        measurements.record("file_prepare", started, 1);
        let started = measurements.start();
        let parameters = params![
            path,
            int(*dev),
            int(*ino),
            int(*size),
            modified_ns.to_be_bytes(),
            int(*offset),
            prefix,
            tail,
            entrypoint,
            title,
            agent_name,
            last_model,
            failed,
            first,
            last,
            cwd,
            branch,
            json(pending)?,
            json(tool_ids)?,
            json(yields)?,
            json(busy)?,
            json(links)?,
            json(codex_tokens)?,
            json(codex_tokens_by_model)?,
            rate_limits,
            native_usage::encoded(index)?,
            copilot.as_ref().map(json).transpose()?,
            changed_ns.map(i128::to_be_bytes),
        ];
        measurements.record("file_parameters", started, 1);
        let started = measurements.start();
        let file_id = statement.query_row(parameters, |row| row.get(0))?;
        measurements.record("file_upsert", started, 1);
        file_id
    };

    let started = measurements.start();
    write_tree_summary(
        transaction,
        path,
        ledger,
        tree_summary.as_ref(),
        index.native_usage_observations.is_some(),
    )?;
    measurements.record("proof_tree", started, 1);
    let started = measurements.start();
    let mut put = transaction.prepare_cached(PUT_EVENT)?;
    let mut cut = transaction
        .prepare_cached("DELETE FROM events WHERE file_id = ?1 AND extra = ?2 AND seq >= ?3")?;
    for (extra, before, touched, new_rows) in [
        (false, changes.before_events, &changes.events, events),
        (true, changes.before_extras, &changes.extras, extras),
    ] {
        changed(
            before,
            touched,
            new_rows,
            |seq, event| put_event(&mut put, file_id, extra, seq, event),
            |from| {
                cut.execute(params![file_id, extra, seq_int(from)])
                    .map(drop)
            },
        )?;
    }
    measurements.record("events", started, 1);
    measurements.count("event_records", events.len().saturating_add(extras.len()));

    let started = measurements.start();
    let mut put = transaction.prepare_cached(PUT_SIGNAL)?;
    changed(
        changes.before_signals,
        &changes.signals,
        signals,
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
                    "DELETE FROM signals WHERE file_id = ?1 AND seq >= ?2",
                    params![file_id, seq_int(from)],
                )
                .map(drop)
        },
    )?;
    measurements.record("signals", started, 1);
    measurements.count("signal_records", signals.len());

    let started = measurements.start();
    let mut put = transaction.prepare_cached(PUT_USAGE)?;
    let mut put_identity = transaction.prepare_cached(
        "INSERT INTO usage_identity(file_id,message_id,record_uuid) VALUES (?1,?2,?3)",
    )?;
    for id in &changes.usage {
        transaction.execute(
            "DELETE FROM usage_identity WHERE file_id=?1 AND message_id=?2",
            params![file_id, id],
        )?;
        if let Some(usage) = usage_by_id.get(id) {
            put_usage(&mut put, file_id, id, usage)?;
            for uuid in &usage.record_ids {
                put_identity.execute(params![file_id, id, uuid])?;
            }
        } else {
            transaction.execute(
                "DELETE FROM usage WHERE file_id = ?1 AND message_id = ?2",
                params![file_id, id],
            )?;
        }
    }
    measurements.record("usage", started, changes.usage.len());

    let started = measurements.start();
    let mut put = transaction.prepare_cached(PUT_CODEX_USAGE)?;
    changed(
        changes.before_codex_usage,
        &changes.codex_usage,
        codex_usage_events,
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
                    "DELETE FROM codex_usage WHERE file_id = ?1 AND seq >= ?2",
                    params![file_id, seq_int(from)],
                )
                .map(drop)
        },
    )?;
    measurements.record("codex_usage", started, 1);
    measurements.count("codex_usage_records", codex_usage_events.len());
    Ok(())
}

fn put_event(
    statement: &mut Statement<'_>,
    file_id: i64,
    extra: bool,
    seq: usize,
    event: &Event,
) -> rusqlite::Result<()> {
    let Event {
        k,
        o,
        b,
        t,
        id,
        n,
        code_mode,
        code_mode_open,
        parent,
        script,
        y,
        poll,
        r,
        item,
        peer,
        u,
    } = event;
    let y = y.as_deref().map(json).transpose()?;
    let (reply, reply_o, reply_b, reply_t, reply_e, reply_f, reply_m) = match r {
        Some(Reply {
            o: reply_o,
            b: reply_b,
            t: reply_t,
            e: reply_e,
            f: reply_f,
            m: reply_m,
        }) => (
            true,
            Some(int(*reply_o)),
            Some(*reply_b),
            *reply_t,
            Some(*reply_e),
            Some(*reply_f),
            reply_m.as_deref(),
        ),
        None => (false, None, None, None, None, None, None),
    };
    let (has_peer, peer_sock, peer_pid, peer_start) = match peer {
        Some(Peer { sock, pid, start }) => (true, sock.as_deref(), pid.map(int), start.as_deref()),
        None => (false, None, None, None),
    };
    statement.execute(params![
        file_id,
        extra,
        seq_int(seq),
        tag(k)?,
        int(*o),
        b,
        t,
        id,
        n,
        code_mode,
        code_mode_open,
        parent.map(seq_int),
        script,
        y,
        poll.map(seq_int),
        reply,
        reply_o,
        reply_b,
        reply_t,
        reply_e,
        reply_f,
        reply_m,
        item.map(int),
        has_peer,
        peer_sock,
        peer_pid,
        peer_start,
        u,
    ])?;
    Ok(())
}

fn put_usage(
    statement: &mut Statement<'_>,
    file_id: i64,
    message_id: &str,
    usage: &MessageUsage,
) -> rusqlite::Result<()> {
    let MessageUsage {
        record_ids,
        model,
        tokens:
            Tokens {
                input,
                cached_input,
                output,
                reasoning_output,
                total,
            },
        model_tokens:
            ModelTokens {
                input: model_input,
                output: model_output,
                cache_write: model_cache_write,
                cache_read: model_cache_read,
            },
        billing:
            BillingUsage {
                input: billing_input,
                output: billing_output,
                cache_read: billing_cache_read,
                cache_write_5m,
                cache_write_1h,
                web_search_requests,
                speed,
                service_tier,
                prompt_size,
                timestamp,
                split_unknown,
            },
    } = usage;
    statement.execute(params![
        file_id,
        message_id,
        model,
        int(*input),
        int(*cached_input),
        int(*output),
        int(*reasoning_output),
        int(*total),
        int(*model_input),
        int(*model_output),
        int(*model_cache_write),
        int(*model_cache_read),
        int(*billing_input),
        int(*billing_output),
        int(*billing_cache_read),
        int(*cache_write_5m),
        int(*cache_write_1h),
        int(*web_search_requests),
        speed,
        service_tier,
        int(*prompt_size),
        timestamp,
        split_unknown,
        json(record_ids)?,
    ])?;
    Ok(())
}

fn put_run(statement: &mut Statement<'_>, run: &ReportedRunSnapshot) -> rusqlite::Result<()> {
    let ReportedRunSnapshot {
        last_session_id,
        last_start_time,
        last_cost,
        last_duration,
        last_api_duration,
        last_tool_duration,
        last_lines_added,
        last_lines_removed,
        last_model_usage,
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
        json(last_model_usage)?,
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
        let legacy = root.join("index.events.json");
        let (mut store, _) = SqliteStore::attempt(&path, None)
            .map_err(|error| error.to_string())
            .unwrap();
        let mut old = run();
        old.last_session_id = "selected".into();
        let mut newer = old.clone();
        newer.last_start_time += 1;
        newer.last_cost = Some(2.5);
        store
            .write_runs(&[old.clone(), newer.clone()], &stamp())
            .unwrap();
        {
            let transaction = store.connection.transaction().unwrap();
            let mut statement = transaction.prepare(PUT_RUN).unwrap();
            for n in 0..20000 {
                let mut unrelated = run();
                unrelated.last_session_id = format!("unrelated-{n}");
                put_run(&mut statement, &unrelated).unwrap();
            }
            drop(statement);
            transaction.commit().unwrap();
        }
        drop(store);
        let (mut store, loaded) = open_scoped(&path, &legacy).unwrap();
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

    #[test]
    fn finishing_the_same_corrupt_claim_keeps_one_archive() {
        let root = scratch("same-corrupt-claim");
        let legacy = root.join("index.events.json");
        let claim = root.join("claim");
        let aside = sibling(&legacy, ".corrupt");
        fs::write(&claim, b"damaged cache").unwrap();
        fs::hard_link(&claim, &aside).unwrap();
        assert_eq!(
            set_aside_legacy(&legacy, &claim).unwrap(),
            Some(aside.clone())
        );
        assert!(!claim.exists());
        assert!(!sibling(&legacy, ".corrupt.1").exists());
        assert_eq!(fs::read(&aside).unwrap(), b"damaged cache");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn different_corrupt_claims_never_replace_each_other() {
        let root = scratch("different-corrupt-claims");
        let legacy = root.join("index.events.json");
        let claims: Vec<_> = (0..8)
            .map(|index| {
                let claim = root.join(format!("claim-{index}"));
                fs::write(&claim, index.to_string()).unwrap();
                claim
            })
            .collect();
        let workers: Vec<_> = claims
            .into_iter()
            .map(|claim| {
                let legacy = legacy.clone();
                std::thread::spawn(move || set_aside_legacy(&legacy, &claim).unwrap().unwrap())
            })
            .collect();
        let contents: BTreeSet<_> = workers
            .into_iter()
            .map(|worker| fs::read_to_string(worker.join().unwrap()).unwrap())
            .collect();
        assert_eq!(contents, (0..8).map(|index| index.to_string()).collect());
        fs::remove_dir_all(root).unwrap();
    }

    fn opened(path: &Path) -> (SqliteStore, Loaded) {
        match SqliteStore::attempt(path, None) {
            Ok(opened) => opened,
            Err(error) => panic!("the store opens: {error}"),
        }
    }

    const CATALOG_FIXTURE_NOW: i64 = 1_790_294_400_000;

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
        let mut cache = crate::events::EventCache::open_scoped(&options.cache).unwrap();
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
        let right = seed.with_file_name("legacy.sqlite3");
        // All fixture connections are closed, so the committed WAL has been
        // checkpointed. Copy the exact stream epoch and source evidence too.
        fs::copy(&seed, &left).unwrap();
        fs::copy(&seed, &right).unwrap();
        (opened(&left).0, opened(&right).0)
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
            .retain(|row| row[0] != rusqlite::types::Value::Text("catalog_observed_at".into()));
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
        legacy: &mut SqliteStore,
        rows: &[crate::model::summary::CatalogRow],
        projections: &[crate::slot_projection::Projection],
        scoped: bool,
    ) {
        let before = crate::model::now_ms();
        let recording = crate::native_timing::catalog_commit_tests::recording(false);
        let left = catalog_write(batch, rows, projections, scoped, CatalogWriter::Auto).unwrap();
        let left_state =
            catalog_commit_success_state(&batch.connection, before, crate::model::now_ms());
        crate::native_timing::catalog_commit_tests::assert_written(false);
        drop(recording);
        let before = crate::model::now_ms();
        let recording = crate::native_timing::catalog_commit_tests::recording(true);
        let right =
            catalog_write(legacy, rows, projections, scoped, CatalogWriter::Legacy).unwrap();
        let right_state =
            catalog_commit_success_state(&legacy.connection, before, crate::model::now_ms());
        crate::native_timing::catalog_commit_tests::assert_written(true);
        drop(recording);
        assert_eq!(left, right, "exact committed collection generation");
        assert_eq!(left.0, Outcome::Written);
        assert_eq!(
            left_state, right_state,
            "full MAIN/TEMP rows and schemas; only the validated observation clock is excluded"
        );
        assert_eq!(
            catalog_comparable_state(&batch.connection),
            catalog_comparable_state(&legacy.connection)
        );
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
            .filter(|row| row[0] == rusqlite::types::Value::Text("catalog_observed_at".into()))
            .collect::<Vec<_>>();
        assert_eq!(clock.len(), 1);
        let rusqlite::types::Value::Text(value) = &clock[0][1] else {
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
        meta.retain(|row| row[0] != rusqlite::types::Value::Text("catalog_observed_at".into()));
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
            let mut cache = crate::events::EventCache::open_scoped(&scoped_options.cache).unwrap();
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
    fn old_catalog_component_cannot_preserve_complete_before_bounded_upgrade() {
        let root = scratch("catalog-component-version");
        let (options, rows, projections) = catalog_native_fixture(&root, 2, true);
        let (mut store, other) = catalog_pair(&options);
        let publication = crate::slot_projection::Publication {
            rows: &rows[..1],
            transcripts: Some(&projections),
        };
        let claims = rows[0]
            .sources
            .iter()
            .map(|source| (source.path.to_string_lossy().into_owned(), None))
            .collect::<Vec<_>>();
        let before = store.session_catalog_generation().unwrap();
        store
            .connection
            .execute(
                "UPDATE meta SET value=?1 WHERE key='catalog_version'",
                [(crate::model::summary::CATALOG_VERSION - 1).to_string()],
            )
            .unwrap();
        assert!(
            store
                .publish_closed_catalog_fenced(&publication, before.as_deref(), &claims)
                .unwrap()
                .is_none()
        );
        assert_eq!(store.session_catalog_generation().unwrap(), before);
        assert!(!store.qualified_catalog().unwrap());
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM session_catalog", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(store);
        drop(other);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_batch_complete_publisher_matches_legacy_rows_feed_sources_slots_and_repairs() {
        let root = scratch("catalog-publisher-parity");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 64, false);
        rows[4].repo = Some("fixture-repo".into());
        let (mut batch, mut legacy) = catalog_pair(&options);
        let metadata = rows
            .iter()
            .map(json)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(catalog_batch_bytes(&rows, &metadata, CatalogWriter::Auto).is_some());
        catalog_write_pair(&mut batch, &mut legacy, &rows, &projections, false);
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
        for store in [&batch, &legacy] {
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
        catalog_write_pair(&mut batch, &mut legacy, &rows, &projections, false);
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
        for store in [&batch, &legacy] {
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
        catalog_write_pair(&mut batch, &mut legacy, &rows, &projections, false);
        assert_eq!(batch.connection.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE session_key=?1 AND lifecycle='current'", [&rows[8].key], |row| row.get::<_, i64>(0)).unwrap(), rows[8].sources.len() as i64, "unchanged metadata still repairs old-version sources");
        // A two-row scoped no-op keeps unrelated rows and exact source claims.
        let metadata = rows[10..12]
            .iter()
            .map(json)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(catalog_batch_bytes(&rows[10..12], &metadata, CatalogWriter::Auto).is_some());
        catalog_write_pair(&mut batch, &mut legacy, &rows[10..12], &projections, true);
        drop(batch);
        drop(legacy);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_batch_fallbacks_preserve_legacy_acceptance_and_duplicate_rollback() {
        let root = scratch("catalog-publisher-fallback");
        let (options, rows, projections) = catalog_native_fixture(&root, 65, false);
        let (mut batch, mut legacy) = catalog_pair(&options);
        let before = catalog_state(&batch.connection);
        for (store, writer) in [
            (&mut batch, CatalogWriter::Auto),
            (&mut legacy, CatalogWriter::Legacy),
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
            catalog_write_pair(&mut batch, &mut legacy, rows, &projections, scoped);
        }
        let mut oversized = rows[..2].to_vec();
        oversized[1].name = "x".repeat(1_048_576);
        let metadata = oversized
            .iter()
            .map(json)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(catalog_batch_bytes(&oversized, &metadata, CatalogWriter::Auto).is_none());
        catalog_write_pair(&mut batch, &mut legacy, &oversized, &projections, true);
        let duplicate = vec![rows[0].clone(), rows[0].clone()];
        for (store, writer) in [
            (&mut batch, CatalogWriter::Auto),
            (&mut legacy, CatalogWriter::Legacy),
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
        drop(legacy);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_batch_parent_transaction_rolls_back_middle_dml_sources_slots_and_ack_tail() {
        let root = scratch("catalog-publisher-rollback");
        let (options, mut rows, projections) = catalog_native_fixture(&root, 4, false);
        let (mut batch, mut legacy) = catalog_pair(&options);
        catalog_write_pair(&mut batch, &mut legacy, &rows, &projections, false);
        let mut stale_sources = rows.clone();
        stale_sources[1].sources[0].prefix_sha256[0] ^= 1;
        for (store, writer) in [
            (&mut batch, CatalogWriter::Auto),
            (&mut legacy, CatalogWriter::Legacy),
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
        for store in [&mut batch, &mut legacy] {
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
            catalog_comparable_state(&legacy.connection)
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
                (&mut legacy, CatalogWriter::Legacy),
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
        catalog_write_pair(&mut batch, &mut legacy, &rows, &projections, false);
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
        drop(legacy);
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
            let original =
                Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
            let original_recipes = catalog_recipes(&original, "session_slots", key);
            assert!(!original_recipes.is_empty());
            if unsupported {
                original.execute("UPDATE session_slot_projections SET version=?1,source_generation=NULL WHERE session_key=?2", params![crate::slot_projection::VERSION-1,key]).unwrap();
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
            let (mut batch, mut legacy) = catalog_pair(&options);
            catalog_write_pair(&mut batch, &mut legacy, &rows, &projections, false);
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
            drop(legacy);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn schema_twenty_seven_preserves_original_rows_and_adds_focused_identity_index() {
        let root = scratch("focused-proof-migration");
        let (options, _, _) = catalog_native_fixture(&root, 2, true);
        let path = crate::events::EventCache::path(&options.cache);
        let (store, _) = opened(&path);
        store
            .connection
            .execute_batch("DROP INDEX session_trace_edge_identity; PRAGMA user_version=27;")
            .unwrap();
        let before = crate::native_timing::cleanup_tests::snapshot(&store.connection);
        drop(store);
        let (store, _) = opened(&path);
        assert_eq!(
            versions(&store.connection).unwrap(),
            (SCHEMA_VERSION, Some(CACHE_VERSION))
        );
        let mut after = crate::native_timing::cleanup_tests::snapshot(&store.connection);
        let schema = after.get_mut("main.sqlite_schema").unwrap();
        let added = schema
            .iter()
            .filter(|row| {
                row[1] == rusqlite::types::Value::Text("session_trace_edge_identity".into())
            })
            .count();
        assert_eq!(added, 1);
        schema.retain(|row| {
            row[1] != rusqlite::types::Value::Text("session_trace_edge_identity".into())
        });
        assert_eq!(
            after, before,
            "migration must preserve every existing MAIN/TEMP row and schema object"
        );
        assert!(store.connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='session_trace_edge_identity')",[],|r|r.get::<_,bool>(0)).unwrap());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_twenty_six_adds_custody_parts_without_rebinding_existing_rows() {
        let root = scratch("custody-parts-migration");
        let (options, _, _) = catalog_native_fixture(&root, 2, true);
        let relative = "sessions/2026/09/24/rollout-native-cx-01.jsonl";
        let proof = crate::session_source_proof(&options, "codex", relative)
            .unwrap()
            .unwrap();
        let ready = crate::source_projection_ready(&options, "codex", relative)
            .unwrap()
            .unwrap();
        crate::retain_source_event_index(&options, &proof, &ready).unwrap();
        let path = crate::events::EventCache::path(&options.cache);
        let (store, _) = opened(&path);
        // Recreate the preceding schema's absence of this additive table.
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM session_history_slot_parts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .connection
            .execute_batch("DROP TABLE session_history_slot_parts; PRAGMA user_version=26;")
            .unwrap();
        let before = crate::native_timing::cleanup_tests::snapshot(&store.connection);
        drop(store);
        let (store, _) = opened(&path);
        assert_eq!(
            versions(&store.connection).unwrap(),
            (SCHEMA_VERSION, Some(CACHE_VERSION))
        );
        let mut after = crate::native_timing::cleanup_tests::snapshot(&store.connection);
        assert_eq!(
            after.remove("main.session_history_slot_parts"),
            Some(Vec::new())
        );
        after.get_mut("main.sqlite_schema").unwrap().retain(|row| {
            row[2] != rusqlite::types::Value::Text("session_history_slot_parts".into())
        });
        assert_eq!(
            after, before,
            "migration must preserve every existing MAIN/TEMP row and schema object"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
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
            let (mut store, legacy) = catalog_pair(&options);
            drop(legacy);
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
                store = match SqliteStore::attempt_scoped(&path, None, true) {
                    Ok((store, _)) => store,
                    Err(error) => panic!("reopen transfer store: {error}"),
                };
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
    fn catalog_batch_matches_legacy_partial_native_lineage_history_and_archive_proofs() {
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
        let (mut batch, mut legacy) = catalog_pair(&options);
        catalog_write_pair(&mut batch, &mut legacy, &rows, &projections, false);
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
        drop(legacy);
        fs::remove_dir_all(root).unwrap();
    }

    fn traversal_stores(options: &crate::Options, count: usize) -> Vec<SqliteStore> {
        let seed = crate::events::EventCache::path(&options.cache);
        (0..count)
            .map(|arm| {
                let path = seed.with_file_name(format!("traversal-{arm}.sqlite3"));
                // Every EventCache/NativeEvidence owner has already been dropped.
                fs::copy(&seed, &path).unwrap();
                match SqliteStore::attempt_scoped(&path, None, true) {
                    Ok((store, loaded)) => {
                        assert!(loaded.files.is_empty());
                        assert!(loaded.reported_runs.is_empty());
                        store
                    }
                    Err(error) => panic!("real scoped producer opens: {error}"),
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
        )
        .unwrap();
        let inputs = crate::inputs::inputs(&options)
            .unwrap()
            .into_iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .collect::<Vec<_>>();
        assert_eq!(inputs.len(), 2049);
        let mut cache = crate::events::EventCache::open_scoped(&options.cache).unwrap();
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
            let physical = connection.prepare(&format!("SELECT session_key,gram,{column} FROM session_search_memberships WHERE {column} IS NOT NULL UNION ALL SELECT session_key,gram,last_ms FROM session_search_postings WHERE read_scope=?1")).unwrap()
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
                    let selection = crate::catalog_search::read(
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
        // Seed the same real-schema legacy representation and history-only owner
        // before all arms. Replacements must remove old grams and convert only
        // reached legacy owners without changing the retained owner.
        let retained = rows[63].key.clone();
        for store in &mut stores {
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            tx.execute("INSERT INTO session_history_catalog SELECT session_key,json_set(metadata,'$.lifecycle','retained') FROM session_catalog WHERE session_key=?1", [&retained]).unwrap();
            tx.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key=?1", [&retained]).unwrap();
            crate::catalog_search::retire_key(&tx, &retained).unwrap();
            for (scope, column) in [
                ("current", "current_last_ms"),
                ("retained_history", "history_last_ms"),
            ] {
                tx.execute(&format!("INSERT INTO session_search_postings SELECT ?1,session_key,gram,{column} FROM session_search_memberships WHERE session_key=?2 AND {column} IS NOT NULL"), params![scope,rows[0].key]).unwrap();
            }
            tx.execute(
                "DELETE FROM session_search_memberships WHERE session_key=?1",
                [&rows[0].key],
            )
            .unwrap();
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
            assert_eq!(
                store
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM session_search_postings WHERE session_key=?1",
                        [&rows[62].key],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0,
                "selected legacy owner converted"
            );
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
        let index = |model: &str| FileIndex {
            tree_summary: Some(crate::native_tree::Summary {
                models: [model.to_owned()].into_iter().collect(),
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
        let (observed, metadata) = store.read_tree("a.jsonl", &old.stat).unwrap().unwrap();
        assert_eq!(observed, old);
        assert_eq!(
            metadata.summary.models,
            ["old".to_owned()].into_iter().collect()
        );
        assert!(store.read_tree("a.jsonl", &old.stat).unwrap().is_none());
        let (observed, metadata) = store.read_tree("a.jsonl", &new.stat).unwrap().unwrap();
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
                    parent: Some(7),
                    script: STDIN | SENDS,
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
    fn schema_eight_backfills_exact_usage_identity_without_reading_source_bodies() {
        let root = scratch("usage-identity-migration");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store.write_runs(&[run()], &stamp()).unwrap();
        store
            .write_one("a", None, None, &ledger(1), &full())
            .unwrap();
        store
            .write_one("damaged", None, None, &ledger(1), &full())
            .unwrap();
        store.connection.execute("UPDATE usage SET record_ids='not json' WHERE file_id=(SELECT file_id FROM files WHERE path='damaged')", []).unwrap();
        store
            .connection
            .execute_batch("DROP TABLE usage_identity; PRAGMA user_version=8;")
            .unwrap();
        drop(store);
        let (store, loaded) = opened(&path);
        assert_eq!(loaded.files.len(), 1);
        let expected: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM usage_identity", [], |row| row.get(0))
            .unwrap();
        assert_eq!(expected, 1);
        let identity: (String, String) = store
            .connection
            .query_row(
                "SELECT message_id,record_uuid FROM usage_identity",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(identity.1, "record-uuid");
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM reported_runs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
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
    fn catalog_outbox_migration_includes_current_history_fallback_without_reseeding() {
        for schema in [17, 18] {
            let root = scratch(&format!("outbox-migration-{schema}"));
            let path = root.join("index.sqlite3");
            let (store, _) = opened(&path);
            store.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES('current-only',0,'claude','{}')", []).unwrap();
            let epoch: String = store
                .connection
                .query_row(
                    "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if schema == 17 {
                store.connection.execute_batch("DROP TRIGGER catalog_changes_insert; DROP TRIGGER catalog_changes_update; DROP TRIGGER catalog_changes_delete; DROP TRIGGER catalog_history_changes_insert; DROP TRIGGER catalog_history_changes_update; DROP TRIGGER catalog_history_changes_delete; DROP TABLE session_catalog_changes; DELETE FROM meta WHERE key='catalog_changes_epoch';").unwrap();
            } else {
                // The old bootstrap stream did not seed Current fallback rows
                // for History, but its Current consumer may have a saved head.
                store
                    .connection
                    .execute(
                        "DELETE FROM session_catalog_changes WHERE read_scope='retained_history'",
                        [],
                    )
                    .unwrap();
            }
            store
                .connection
                .execute(
                    "DELETE FROM meta WHERE key LIKE 'catalog_changes_floor_%'",
                    [],
                )
                .unwrap();
            store
                .connection
                .pragma_update(None, "user_version", schema)
                .unwrap();
            drop(store);
            let (store, _) = opened(&path);
            let hints: Vec<(String, String)> = store
                .connection
                .prepare(
                    "SELECT session_key,read_scope FROM session_catalog_changes ORDER BY revision",
                )
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(
                hints,
                [
                    ("current-only".into(), "current".into()),
                    ("current-only".into(), "retained_history".into())
                ]
            );
            if schema == 18 {
                let migrated: String = store
                    .connection
                    .query_row(
                        "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    migrated, epoch,
                    "saved consumer epoch must survive migration"
                );
            }
            drop(store);
            let (store, _) = opened(&path);
            let count: i64 = store
                .connection
                .query_row("SELECT count(*) FROM session_catalog_changes", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(count, 2, "restart cannot repeat migration hints");
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn schema_thirteen_preserves_sources_without_inventing_change_time() {
        let root = scratch("change-time-migration");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        // This historical schema did not persist portable native metadata.
        let mut index = full();
        index.codex_header = None;
        index.marker_index = None;
        index.handoff_index = None;
        store
            .write_one("original.jsonl", None, None, &ledger(1), &index)
            .unwrap();
        store
            .connection
            .execute_batch("ALTER TABLE files DROP COLUMN ctime_ns; PRAGMA user_version = 13;")
            .unwrap();
        drop(store);
        let (store, _) = opened(&path);
        let (migrated, original) = store.read_one("original.jsonl").unwrap().unwrap();
        let mut expected = ledger(1);
        expected.stat.changed_ns = None;
        assert_eq!(migrated, expected);
        assert_eq!(format!("{original:?}"), format!("{index:?}"));
        assert_eq!(
            versions(&store.connection).unwrap(),
            (SCHEMA_VERSION, Some(CACHE_VERSION))
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_one_migrates_native_usage_without_losing_reported_runs() {
        let root = scratch("native-usage-migration");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store.write_runs(&[run()], &stamp()).unwrap();
        store
            .connection
            .execute_batch(
                "ALTER TABLE usage DROP COLUMN record_ids;
             ALTER TABLE files DROP COLUMN copilot;
             ALTER TABLE files DROP COLUMN codex_native_usage;
             PRAGMA user_version = 1;
             UPDATE meta SET value = '19' WHERE key = 'cache_version';",
            )
            .unwrap();
        drop(store);
        let (store, loaded) = opened(&path);
        assert_eq!(
            format!("{:?}", loaded.reported_runs),
            format!("{:?}", [run()])
        );
        assert_eq!(loaded.stamp, Some(stamp()));
        assert!(loaded.files.is_empty());
        assert_eq!(
            versions(&store.connection).unwrap(),
            (SCHEMA_VERSION, Some(CACHE_VERSION))
        );
        store
            .connection
            .prepare("SELECT codex_native_usage FROM files")
            .unwrap();
    }

    #[test]
    fn schema_two_rebuilds_assistant_uuids_and_preserves_reported_runs() {
        let root = scratch("copied-usage-migration");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store.write_runs(&[run()], &stamp()).unwrap();
        store
            .write_one("a.jsonl", None, None, &ledger(1), &full())
            .unwrap();
        store.connection.execute_batch("ALTER TABLE files DROP COLUMN copilot; ALTER TABLE usage DROP COLUMN record_ids; PRAGMA user_version = 2; UPDATE meta SET value = '20' WHERE key = 'cache_version';").unwrap();
        drop(store);
        let (store, loaded) = opened(&path);
        assert!(loaded.files.is_empty());
        assert_eq!(
            format!("{:?}", loaded.reported_runs),
            format!("{:?}", [run()])
        );
        assert_eq!(loaded.stamp, Some(stamp()));
        assert_eq!(
            versions(&store.connection).unwrap(),
            (SCHEMA_VERSION, Some(CACHE_VERSION))
        );
        store
            .connection
            .prepare("SELECT record_ids FROM usage")
            .unwrap();
    }

    #[test]
    fn schema_three_adds_copilot_and_preserves_reported_runs() {
        let root = scratch("copilot-migration");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store.write_runs(&[run()], &stamp()).unwrap();
        store
            .write_one("a.jsonl", None, None, &ledger(1), &full())
            .unwrap();
        store.connection.execute_batch("ALTER TABLE files DROP COLUMN copilot; PRAGMA user_version = 3; UPDATE meta SET value = '21' WHERE key = 'cache_version';").unwrap();
        drop(store);
        let (store, loaded) = opened(&path);
        assert!(loaded.files.is_empty());
        assert_eq!(
            format!("{:?}", loaded.reported_runs),
            format!("{:?}", [run()])
        );
        assert_eq!(loaded.stamp, Some(stamp()));
        assert_eq!(
            versions(&store.connection).unwrap(),
            (SCHEMA_VERSION, Some(CACHE_VERSION))
        );
        store
            .connection
            .prepare("SELECT copilot FROM files")
            .unwrap();
        store
            .connection
            .prepare("SELECT record_ids FROM usage")
            .unwrap();
    }

    #[test]
    fn a_parser_change_rereads_every_file_but_keeps_reported_runs() {
        let root = scratch("versions");
        let path = root.join("index.sqlite3");
        let (mut store, _) = opened(&path);
        store
            .write_one("a.jsonl", None, None, &ledger(1), &full())
            .unwrap();
        let all = store.write_runs(&[run()], &stamp()).unwrap().unwrap();
        assert_eq!(format!("{all:?}"), format!("{:?}", [run()]));

        // Another version rebuilt it for an older parser: this connection
        // notices and writes nothing.
        store
            .connection
            .execute(
                "UPDATE meta SET value = '13' WHERE key = 'cache_version'",
                [],
            )
            .unwrap();
        assert_eq!(
            store
                .write_one("a.jsonl", Some(&ledger(1)), None, &ledger(2), &full())
                .unwrap(),
            Outcome::Stale
        );
        assert_eq!(store.remove(&[]).unwrap(), Outcome::Stale);
        assert!(store.write_runs(&[run()], &stamp()).unwrap().is_none());
        drop(store);

        // Opened by this parser: every file is read again, and the reported
        // runs, which the logs can't give back, stay.
        let (store, loaded) = opened(&path);
        assert!(loaded.files.is_empty());
        assert_eq!(
            format!("{:?}", loaded.reported_runs),
            format!("{:?}", [run()])
        );
        assert_eq!(loaded.stamp, Some(stamp()));

        // A newer parser's store isn't touched.
        let newer = (CACHE_VERSION + 1).to_string();
        store
            .connection
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'cache_version'",
                [&newer],
            )
            .unwrap();
        drop(store);
        assert!(matches!(
            SqliteStore::attempt(&path, None),
            Err(Unopened::Other(_))
        ));
        let connection = Connection::open(&path).unwrap();
        let kept: String = connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'cache_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kept, newer);

        // So is a newer schema's.
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(connection);
        assert!(matches!(
            SqliteStore::attempt(&path, None),
            Err(Unopened::Other(_))
        ));
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
        let (mut store, _) =
            SqliteStore::attempt(&path, None).unwrap_or_else(|error| panic!("{error}"));
        let rows = vec![("first".into(), 1, "inputs".into(), "old".into())];
        store.save_session_descriptions(&rows).unwrap();
        store.connection.execute_batch("CREATE TEMP TRIGGER interrupt_summary BEFORE INSERT ON session_descriptions WHEN NEW.session_key = 'second' BEGIN SELECT RAISE(ABORT, 'interrupted'); END;").unwrap();
        let batch = vec![
            ("first".into(), 1, "inputs".into(), "new".into()),
            ("second".into(), 1, "inputs".into(), "new".into()),
        ];
        assert!(store.save_session_descriptions(&batch).is_err());
        assert_eq!(
            store
                .session_description("first", 1, "inputs")
                .unwrap()
                .as_deref(),
            Some("old")
        );
        assert_eq!(
            store.session_description("second", 1, "inputs").unwrap(),
            None
        );
        drop(store);
        let (store, _) =
            SqliteStore::attempt(&path, None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            store
                .session_description("first", 1, "inputs")
                .unwrap()
                .as_deref(),
            Some("old")
        );
        assert_eq!(
            store.session_description("second", 1, "inputs").unwrap(),
            None
        );
        drop(store);
        fs::remove_file(path).unwrap();
    }
}

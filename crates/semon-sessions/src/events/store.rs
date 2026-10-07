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

use super::{
    BillingUsage, CACHE_VERSION, CodexUsageEvent, Event, FileIndex, IndexStore, Ledger, Loaded,
    MessageUsage, ModelTokens, Outcome, Peer, Reply, ReportedFileStamp, Signal, Stat, StoreError,
    Yield,
};
use crate::{Tokens, facts::ReportedRunSnapshot};

/// `PRAGMA user_version`: the shape of the tables. The parser's version is
/// [`CACHE_VERSION`], kept in `meta`.
const SCHEMA_VERSION: i64 = 12;

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
CREATE TABLE IF NOT EXISTS session_slots (
    session_key TEXT NOT NULL,
    slot INTEGER NOT NULL,
    metadata TEXT NOT NULL,
    PRIMARY KEY(session_key,slot)
) STRICT, WITHOUT ROWID;

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
CREATE TABLE IF NOT EXISTS session_catalog_sources (
    session_key TEXT NOT NULL,
    lifecycle TEXT NOT NULL DEFAULT 'current' CHECK(lifecycle IN ('current','retained')),
    harness TEXT NOT NULL,
    native_id TEXT NOT NULL,
    source_path TEXT NOT NULL,
    PRIMARY KEY(session_key, source_path)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_catalog_native ON session_catalog_sources(harness, native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_source_path ON session_catalog_sources(source_path, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_current_native ON session_catalog_sources(lifecycle, harness, native_id, session_key);
CREATE INDEX IF NOT EXISTS session_catalog_current_source_path ON session_catalog_sources(lifecycle, source_path, session_key);
CREATE TABLE IF NOT EXISTS session_catalog_invalidations (
    source_path TEXT PRIMARY KEY,
    revision INTEGER NOT NULL,
    reason TEXT NOT NULL CHECK(reason IN ('changed', 'removed')),
    observed_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
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
UPDATE session_catalog SET lifecycle='retained',metadata=CASE WHEN json_valid(metadata) THEN json_set(metadata,'$.lifecycle','retained') ELSE metadata END;
UPDATE session_catalog_sources SET lifecycle='retained';
DELETE FROM meta WHERE key IN ('catalog_version', 'catalog_generation', 'catalog_observed_at');
DELETE FROM session_descriptions;
DELETE FROM events;
DELETE FROM signals;
DELETE FROM usage_identity;
DELETE FROM usage;
DELETE FROM codex_usage;
DELETE FROM files;
";

const FILE_COLUMNS: &str = "file_id, path, dev, ino, size, mtime_ns, resume_at, head_sha256, \
     tail_sha256, entrypoint, title, agent_name, last_model, failed, first_ms, last_ms, cwd, \
     branch, pending, tool_ids, yields, busy, links, codex_tokens, codex_tokens_by_model, \
     rate_limits, codex_native_usage, copilot";

const PUT_FILE: &str = "INSERT INTO files (path, dev, ino, size, mtime_ns, resume_at, \
     head_sha256, tail_sha256, entrypoint, title, agent_name, last_model, failed, first_ms, \
     last_ms, cwd, branch, pending, tool_ids, yields, busy, links, codex_tokens, \
     codex_tokens_by_model, rate_limits, codex_native_usage, copilot) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
     ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27) \
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
     rate_limits = excluded.rate_limits, codex_native_usage = excluded.codex_native_usage, copilot = excluded.copilot \
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

/// Load only the selected native sources in the caller's existing read snapshot.
/// This never opens a writable store, imports legacy data, walks roots or prunes
/// source membership. Catalog identities and event ledgers must agree exactly.
#[cfg(test)]
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
            loaded.reported_runs.push(read_run(row)?);
        }
    }
    Ok(loaded)
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

    fn publish_session_catalog(
        &mut self,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> Result<Outcome, StoreError> {
        let rows = publication.rows;
        use sha2::{Digest, Sha256};
        let version = crate::model::summary::CATALOG_VERSION.to_string();
        let metadata: Vec<_> = rows
            .iter()
            .map(json)
            .collect::<Result<_, _>>()
            .map_err(failure)?;
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
            let previous: Option<String> = transaction
                .query_row(
                    "SELECT value FROM meta WHERE key = 'catalog_generation'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failure)?;
            if previous.as_deref() != expected_generation {
                return Ok(Outcome::Conflict);
            }
            // Do not let a slow builder overwrite a newer complete catalog.
            // Verify source membership and consumed-prefix identities inside
            // the publication transaction, after any competing ledger writes.
            let mut sources = BTreeMap::new();
            for source in rows.iter().flat_map(|row| &row.sources) {
                sources.insert(source.path.to_string_lossy().into_owned(), source);
            }
            let committed: i64 = transaction
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
                .map_err(failure)?;
            if committed != i64::try_from(sources.len()).unwrap_or(-1) {
                return Ok(Outcome::Conflict);
            }
            for (path, source) in sources {
                let Some((_, ledger)) = ledger_row(&transaction, &path).map_err(failure)? else {
                    return Ok(Outcome::Conflict);
                };
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
                    return Ok(Outcome::Conflict);
                }
            }
            let previous_version: Option<String> = transaction
                .query_row(
                    "SELECT value FROM meta WHERE key = 'catalog_version'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failure)?;
            {
                // A temporary membership table bounds deletion SQL parameters and
                // never escapes the transaction or becomes part of the read model.
                transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_members (session_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM catalog_members;").map_err(failure)?;
                {
                    let mut member = transaction
                        .prepare("INSERT INTO catalog_members VALUES (?1)")
                        .map_err(failure)?;
                    let mut put = transaction.prepare("INSERT INTO session_catalog (session_key, lifecycle, last_ms, harness, repo, parent_key, metadata) VALUES (?1, 'current', ?2, ?3, ?4, ?5, ?6) ON CONFLICT(session_key) DO UPDATE SET lifecycle='current', last_ms=excluded.last_ms, harness=excluded.harness, repo=excluded.repo, parent_key=excluded.parent_key, metadata=excluded.metadata WHERE session_catalog.lifecycle != 'current' OR session_catalog.metadata != excluded.metadata OR session_catalog.parent_key IS NOT excluded.parent_key").map_err(failure)?;
                    let mut remove_sources = transaction
                        .prepare("DELETE FROM session_catalog_sources WHERE session_key=?1")
                        .map_err(failure)?;
                    let mut put_source = transaction.prepare("INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES (?1,?2,?3,?4)").map_err(failure)?;
                    for (row, metadata) in rows.iter().zip(&metadata) {
                        member.execute([&row.key]).map_err(failure)?;
                        let changed = put
                            .execute(params![
                                row.key,
                                row.last.unwrap_or(0),
                                row.harness,
                                row.repo,
                                row.parent,
                                metadata
                            ])
                            .map_err(failure)?;
                        if changed > 0 || previous_version.as_deref() != Some(&version) {
                            remove_sources.execute([&row.key]).map_err(failure)?;
                            for source in &row.sources {
                                put_source
                                    .execute(params![
                                        row.key,
                                        row.harness,
                                        source.native_id,
                                        source.path.to_string_lossy()
                                    ])
                                    .map_err(failure)?;
                            }
                        }
                    }
                }
                transaction.execute("UPDATE session_catalog SET lifecycle='retained', metadata=CASE WHEN json_valid(metadata) THEN json_set(metadata,'$.lifecycle','retained') ELSE metadata END WHERE lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)", []).map_err(failure)?;
                transaction.execute("UPDATE session_catalog_sources SET lifecycle='retained' WHERE lifecycle='current' AND session_key NOT IN (SELECT session_key FROM catalog_members)", []).map_err(failure)?;
            }
            let generation = {
                let mut digest = Sha256::new();
                digest.update(version.as_bytes());
                let mut statement = transaction
                    .prepare("SELECT metadata FROM session_catalog ORDER BY session_key")
                    .map_err(failure)?;
                let mut values = statement.query([]).map_err(failure)?;
                while let Some(row) = values.next().map_err(failure)? {
                    let metadata: String = row.get(0).map_err(failure)?;
                    digest.update((metadata.len() as u64).to_le_bytes());
                    digest.update(metadata.as_bytes());
                }
                format!("{:x}", digest.finalize())
            };
            if previous.as_deref() != Some(&generation)
                || previous_version.as_deref() != Some(&version)
            {
                transaction.execute("INSERT OR REPLACE INTO meta(key,value) VALUES ('catalog_generation',?1),('catalog_version',?2)",params![generation,version]).map_err(failure)?;
            }
            transaction
                .execute(
                    "INSERT OR REPLACE INTO meta(key,value) VALUES ('catalog_observed_at',?1)",
                    [crate::model::now_ms().to_string()],
                )
                .map_err(failure)?;
            if let Some(projections) = publication.transcripts {
                let membership: std::collections::BTreeSet<_> =
                    rows.iter().map(|row| row.key.as_str()).collect();
                for projection in projections
                    .iter()
                    .filter(|projection| membership.contains(projection.key.as_str()))
                {
                    let metadata: Vec<String> = projection
                        .slots
                        .iter()
                        .map(json)
                        .collect::<Result<_, _>>()
                        .map_err(failure)?;
                    let source_generation = crate::retention::source_generation(
                        &rows
                            .iter()
                            .find(|row| row.key == projection.key)
                            .expect("catalog membership checked")
                            .sources,
                    )
                    .map_err(|_| {
                        StoreError::Data("source projection metadata cannot serialize".into())
                    })?;
                    let slot_generation = format!(
                        "{:x}",
                        Sha256::digest(
                            format!(
                                "{}|{}|{}",
                                crate::slot_projection::VERSION,
                                json(&metadata).map_err(failure)?,
                                json(
                                    &rows
                                        .iter()
                                        .find(|row| row.key == projection.key)
                                        .expect("catalog membership checked")
                                        .sources
                                )
                                .map_err(failure)?
                            )
                            .as_bytes()
                        )
                    );
                    let changed = transaction.execute(
                        "INSERT INTO session_slot_projections(session_key,generation,version,total,source_generation) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,version=excluded.version,total=excluded.total,source_generation=excluded.source_generation WHERE session_slot_projections.generation != excluded.generation OR session_slot_projections.version != excluded.version OR session_slot_projections.source_generation IS NOT excluded.source_generation",
                        params![projection.key,slot_generation,crate::slot_projection::VERSION,i64::try_from(metadata.len()).map_err(|_| StoreError::Data("slot count exceeds SQL range".into()))?,source_generation]).map_err(failure)?;
                    if changed > 0 {
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
                transaction.execute("DELETE FROM session_slots WHERE session_key NOT IN (SELECT session_key FROM session_catalog)", []).map_err(failure)?;
                transaction.execute("DELETE FROM session_slot_projections WHERE session_key NOT IN (SELECT session_key FROM session_catalog)", []).map_err(failure)?;
                transaction.execute("INSERT OR REPLACE INTO meta(key,value) VALUES ('slot_projection_catalog_generation',?1)", [&generation]).map_err(failure)?;
            } else {
                transaction
                    .execute(
                        "DELETE FROM meta WHERE key='slot_projection_catalog_generation'",
                        [],
                    )
                    .map_err(failure)?;
            }
            // A complete publication validated every committed ledger and
            // source membership while holding the write transaction. Only now
            // can outstanding source observations be acknowledged atomically.
            transaction
                .execute("DELETE FROM session_catalog_invalidations", [])
                .map_err(failure)?;
            transaction.commit().map_err(failure)?;
            Ok(Outcome::Written)
        })();
        self.connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(failure)?;
        result
    }

    fn ledger(&self, path: &str) -> Result<Option<Ledger>, StoreError> {
        ledger_row(&self.connection, path)
            .map(|found| found.map(|(_, ledger)| ledger))
            .map_err(failure)
    }

    fn load_file(&self, path: &str) -> Result<Option<(Ledger, FileIndex)>, StoreError> {
        self.read_one(path).map_err(failure)
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
    transaction.execute(
        "INSERT INTO meta(key,value) VALUES ('catalog_source_revision','1')
         ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(meta.value AS INTEGER)+1 AS TEXT)",
        [],
    )?;
    transaction.execute(
        "INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at)
         VALUES (?1,CAST((SELECT value FROM meta WHERE key='catalog_source_revision') AS INTEGER),?2,?3)
         ON CONFLICT(source_path) DO UPDATE SET revision=excluded.revision,
         reason=excluded.reason, observed_at=excluded.observed_at",
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

impl SqliteStore {
    fn attempt(path: &Path, legacy: Option<&Path>) -> Result<(Self, Loaded), Unopened> {
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
        };
        let loaded = store.load().map_err(classify)?;
        Ok((store, loaded))
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

    /// `path`'s committed ledger and index, read in one transaction.
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
        let full = super::DirtyRows {
            usage: index.usage_by_id.keys().cloned().collect(),
            ..super::DirtyRows::default()
        };
        write_file(&transaction, path, ledger, changes.unwrap_or(&full), index)?;
        invalidate_catalog_source(&transaction, path, "changed")?;
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
                (Ok(Some((_, found))), Some(expected)) if found == *expected => {}
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
            ] {
                transaction.execute(
                    &format!(
                        "DELETE FROM {table} WHERE file_id IN \
                         (SELECT file_id FROM files WHERE path = ?1)"
                    ),
                    [path],
                )?;
            }
            transaction.execute("DELETE FROM files WHERE path = ?1", [path])?;
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
        let all = read_runs(&transaction)?;
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

/// Readies a connection. The versions are read without a lock, so a store
/// that is current opens even while another process writes; the write lock
/// is taken only to create or migrate it.
fn init(connection: &mut Connection) -> rusqlite::Result<Init> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
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
        // Schema 11 used lifecycle-prefixed indexes containing retained rows.
        // Replace only the Current list trees; history/source lookup indexes
        // still support both lifecycle states and keep their existing authority.
        if schema == 11 {
            transaction.execute_batch(
                "DROP INDEX IF EXISTS session_catalog_current_order;
                DROP INDEX IF EXISTS session_catalog_current_harness;
                DROP INDEX IF EXISTS session_catalog_current_repo;
                DROP INDEX IF EXISTS session_catalog_current_harness_repo;
                DROP INDEX IF EXISTS session_catalog_current_parent;",
            )?;
        }
        transaction.execute_batch(SCHEMA)?;
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
        .query_row(
            "SELECT value FROM meta WHERE key = 'cache_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse().ok()))
}

/// The store still has this binary's schema and parser: no other version
/// rebuilt it since it was opened.
fn current(connection: &Connection) -> rusqlite::Result<bool> {
    let schema: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(schema == SCHEMA_VERSION && parser_version(connection)? == Some(CACHE_VERSION))
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
        .query_row(
            "SELECT file_id, dev, ino, size, mtime_ns, resume_at, head_sha256, tail_sha256 \
             FROM files WHERE path = ?1",
            [path],
            |row| {
                Ok((
                    row.get(0)?,
                    Ledger {
                        stat: Stat {
                            dev: uint(row.get(1)?),
                            ino: uint(row.get(2)?),
                            size: uint(row.get(3)?),
                            modified_ns: u128::from_be_bytes(row.get(4)?),
                        },
                        offset: uint(row.get(5)?),
                        prefix: row.get(6)?,
                        tail: row.get(7)?,
                    },
                ))
            },
        )
        .optional()
}

/// The per-file child queries, prepared once per read.
struct Readers<'c> {
    events: Statement<'c>,
    signals: Statement<'c>,
    usage: Statement<'c>,
    codex_usage: Statement<'c>,
}

impl<'c> Readers<'c> {
    fn new(connection: &'c Connection) -> rusqlite::Result<Self> {
        Ok(Self {
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
        })
    }
}

/// One `files` row (selected as [`FILE_COLUMNS`]) and its child rows.
fn read_file(
    readers: &mut Readers<'_>,
    row: &Row<'_>,
) -> rusqlite::Result<(String, Ledger, FileIndex)> {
    let file_id: i64 = row.get(0)?;
    let path: String = row.get(1)?;
    let ledger = Ledger {
        stat: Stat {
            dev: uint(row.get(2)?),
            ino: uint(row.get(3)?),
            size: uint(row.get(4)?),
            modified_ns: u128::from_be_bytes(row.get(5)?),
        },
        offset: uint(row.get(6)?),
        prefix: row.get(7)?,
        tail: row.get(8)?,
    };

    let mut events = Vec::new();
    let mut extras = Vec::new();
    let mut rows = readers.events.query([file_id])?;
    while let Some(event_row) = rows.next()? {
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
        let (id, usage) = read_usage(usage_row)?;
        usage_by_id.insert(id, usage);
    }

    let mut codex_usage_events = Vec::new();
    let mut rows = readers.codex_usage.query([file_id])?;
    while let Some(codex_row) = rows.next()? {
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
        codex_native_usage: row
            .get::<_, Option<String>>(26)?
            .map(|text| from_json(26, &text))
            .transpose()?,
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
    ] {
        transaction.execute(
            &format!("DELETE FROM {table} WHERE file_id = ?1"),
            [file_id],
        )?;
    }
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

/// Upserts `path`'s `files` row, then writes its child rows where `index`
/// differs from `old`.
fn write_file(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    changes: &super::DirtyRows,
    index: &FileIndex,
) -> rusqlite::Result<()> {
    let FileIndex {
        dirty_rows: _,
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
        codex_native_usage,
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
            },
        offset,
        prefix,
        tail,
    } = ledger;
    let rate_limits = rate_limits.as_ref().map(json).transpose()?;
    let file_id: i64 = transaction.query_row(
        PUT_FILE,
        params![
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
            codex_native_usage.as_ref().map(json).transpose()?,
            copilot.as_ref().map(json).transpose()?,
        ],
        |row| row.get(0),
    )?;

    let mut put = transaction.prepare(PUT_EVENT)?;
    let mut cut = transaction
        .prepare("DELETE FROM events WHERE file_id = ?1 AND extra = ?2 AND seq >= ?3")?;
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

    let mut put = transaction.prepare(PUT_SIGNAL)?;
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

    let mut put = transaction.prepare(PUT_USAGE)?;
    let mut put_identity = transaction
        .prepare("INSERT INTO usage_identity(file_id,message_id,record_uuid) VALUES (?1,?2,?3)")?;
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

    let mut put = transaction.prepare(PUT_CODEX_USAGE)?;
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

    fn ledger(n: u8) -> Ledger {
        Ledger {
            stat: Stat {
                dev: u64::MAX,
                ino: 2,
                size: 3 + u64::from(n),
                modified_ns: u128::MAX - u128::from(n),
            },
            offset: 3,
            prefix: [n; 32],
            tail: [n.wrapping_add(1); 32],
        }
    }

    /// A file index with every field set, and every value at an edge where
    /// one exists. Written without `..`: a field added to `FileIndex` or
    /// `Event` fails to compile here until it is set, and the round trip
    /// then proves it is persisted.
    fn full() -> FileIndex {
        FileIndex {
            dirty_rows: None,
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

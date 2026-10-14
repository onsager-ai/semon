//! Content-free, resumable native producer storage. This module orders and
//! fences facts; the native model alone decides entries, turns and slots.
use super::*;
use crate::model::{
    native_stage::SourceTime,
    summary::{CatalogRow, CatalogSource},
};

pub(super) mod candidate_delta;
mod native_repair;
pub(crate) mod physical_profile;
pub(crate) mod source_roots;
pub(crate) type CandidateChange = candidate_delta::CandidateChange;
pub(crate) type CandidateInterest = candidate_delta::CandidateInterest;

pub(crate) const ROWS: usize = 64;
pub(crate) const BYTES: usize = 1024 * 1024;
const STATE_BYTES: usize = 64 * 1024;
// Rebuildable private Query metadata is versioned independently from the
// public native Event Index schema28 and the native parser/custody contract.
// Install both Original repair metadata and the cross-channel child index.
pub(super) const SCHEMA_VERSION: i64 = 12;
const SCHEMA_KEY: &str = "native_query_schema";

pub(super) fn schema_version(db: &Connection) -> rusqlite::Result<Option<i64>> {
    let value: Option<String> = db
        .query_row("SELECT value FROM meta WHERE key=?1", [SCHEMA_KEY], |r| {
            r.get(0)
        })
        .optional()?;
    value
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| invalid("invalid native private schema version"))
        })
        .transpose()
}
fn private_current(db: &Connection) -> rusqlite::Result<bool> {
    Ok(current(db)? && schema_version(db)? == Some(SCHEMA_VERSION))
}

pub(super) fn initialize(db: &Connection) -> rusqlite::Result<()> {
    if schema_version(db)?.is_some_and(|version| version > SCHEMA_VERSION) {
        return Err(invalid("native private schema is newer"));
    }
    // Current and History tables have already been initialized by their native
    // owners. Never install these private-meta indexes in standalone Trace.
    db.execute_batch("DROP TRIGGER IF EXISTS native_catalog_owner_update;")?;
    db.execute_batch(SCHEMA)?;
    db.execute_batch(super::native_rank::SCHEMA)?;
    db.execute_batch(super::native_rank::CANDIDATE_TREE_SCHEMA)?;
    db.execute_batch(super::native_rank::ORTHOGONAL_SCHEMA)?;
    db.execute_batch(native_repair::SCHEMA)?;
    db.execute_batch(native_repair::COMPARE_SCHEMA)?;
    db.execute_batch(native_repair::TEXT_VIEW_SCHEMA)?;
    db.execute_batch(native_repair::OBJECT_SCHEMA)?;
    db.execute_batch(physical_profile::SCHEMA)?;
    db.execute_batch(candidate_delta::SCHEMA)?;
    db.execute(
        "INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![SCHEMA_KEY, SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}
fn source_fact_generation(ledger: &Ledger) -> rusqlite::Result<String> {
    Ok(format!("interval-v2:{}", json(ledger)?))
}
fn source_order_generation(ledger: &Ledger) -> rusqlite::Result<String> {
    Ok(format!("order-v2:{}", json(ledger)?))
}
fn source_operation_generation(ledger: &Ledger) -> rusqlite::Result<String> {
    Ok(format!("operation-v2:{}", json(ledger)?))
}
#[cfg(test)]
thread_local! {
    pub(super) static FULL_LINK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static BUSY_FACT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DELTA_LINK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ORDER_FACT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OPERATION_FACT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OPERATION_FACT_CUT_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_source_busy (
    file_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    PRIMARY KEY(file_id,seq)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_source_busy_intervals (
    file_id INTEGER NOT NULL,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    PRIMARY KEY(file_id,start)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_source_links (
    file_id INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('session','continued','bridge')),
    native_id TEXT NOT NULL,
    PRIMARY KEY(file_id,kind,native_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_source_facts (
    file_id INTEGER PRIMARY KEY,
    generation TEXT NOT NULL,
    events INTEGER NOT NULL,
    extras INTEGER NOT NULL,
    signals INTEGER NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS native_source_event_times (
    file_id INTEGER NOT NULL, seq INTEGER NOT NULL, t INTEGER NOT NULL,
    PRIMARY KEY(file_id,seq)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_source_copy_events (
    file_id INTEGER NOT NULL, seq INTEGER NOT NULL, identity TEXT NOT NULL,
    PRIMARY KEY(file_id,seq)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_source_copy_identity ON native_source_copy_events(file_id,identity,seq);
CREATE TABLE IF NOT EXISTS native_source_order_facts (
    file_id INTEGER PRIMARY KEY, generation TEXT NOT NULL,
    first_seq INTEGER, first_ms INTEGER,
    CHECK((first_seq IS NULL)=(first_ms IS NULL))
) STRICT;
CREATE TABLE IF NOT EXISTS native_source_event_positions (
    file_id INTEGER NOT NULL, seq INTEGER NOT NULL, o INTEGER NOT NULL,
    PRIMARY KEY(file_id,o,seq), UNIQUE(file_id,seq)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_source_child_operations (
    file_id INTEGER NOT NULL,
    parent INTEGER NOT NULL,
    extra INTEGER NOT NULL CHECK(extra IN (0,1)),
    seq INTEGER NOT NULL,
    offset_key BLOB NOT NULL CHECK(length(offset_key)=8),
    block INTEGER NOT NULL,
    time INTEGER,
    PRIMARY KEY(file_id,parent,extra,seq),
    UNIQUE(file_id,extra,seq)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_source_child_operation_parent
    ON native_source_child_operations(file_id,parent,offset_key DESC,block DESC,extra DESC,seq ASC);
CREATE TABLE IF NOT EXISTS native_source_child_operation_facts (
    file_id INTEGER PRIMARY KEY,
    generation TEXT NOT NULL
) STRICT;
CREATE TRIGGER IF NOT EXISTS native_source_child_operation_cleanup AFTER DELETE ON files BEGIN
 DELETE FROM native_source_child_operations WHERE file_id=OLD.file_id;
 DELETE FROM native_source_child_operation_facts WHERE file_id=OLD.file_id;
END;
CREATE TABLE IF NOT EXISTS native_catalog_owner_versions (
 session_key TEXT NOT NULL, scope TEXT NOT NULL, generation INTEGER NOT NULL,
 PRIMARY KEY(session_key,scope)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_catalog_owner_insert AFTER INSERT ON session_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(NEW.session_key,'current',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_owner_update AFTER UPDATE ON session_catalog
BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'current',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
 INSERT INTO native_catalog_owner_versions SELECT NEW.session_key,'current',1 WHERE NEW.session_key IS NOT OLD.session_key ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_owner_delete AFTER DELETE ON session_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'current',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_history_insert AFTER INSERT ON session_history_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(NEW.session_key,'history',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_history_update AFTER UPDATE ON session_history_catalog WHEN NEW.metadata IS NOT OLD.metadata OR NEW.session_key IS NOT OLD.session_key BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'history',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
 INSERT INTO native_catalog_owner_versions SELECT NEW.session_key,'history',1 WHERE NEW.session_key IS NOT OLD.session_key ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_history_delete AFTER DELETE ON session_history_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'history',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TABLE IF NOT EXISTS native_query_jobs (
 job INTEGER PRIMARY KEY, view_key TEXT NOT NULL UNIQUE, version INTEGER NOT NULL,
 catalog_generation TEXT, runtime TEXT NOT NULL, phase TEXT NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL, state TEXT NOT NULL,
 sources INTEGER NOT NULL DEFAULT 0, owners INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX IF NOT EXISTS native_query_pending ON native_query_jobs(job) WHERE phase<>'ready';
CREATE TABLE IF NOT EXISTS native_query_ready (
 view_key TEXT PRIMARY KEY, job INTEGER NOT NULL, metadata TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_query_sources (
 job INTEGER NOT NULL, path TEXT NOT NULL, metadata TEXT NOT NULL,
 present INTEGER NOT NULL, dev INTEGER, ino INTEGER, size INTEGER,
 mtime_ns BLOB, ctime_ns BLOB, resume_at INTEGER, prefix BLOB, tail BLOB,
 claim INTEGER, archive TEXT, mutation TEXT,
 PRIMARY KEY(job,path)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_query_owners (
 job INTEGER NOT NULL, identity TEXT NOT NULL, session_key TEXT NOT NULL,
 catalog TEXT, history TEXT, private_header TEXT, trace_header TEXT,
 slot_header TEXT, history_header TEXT,
 PRIMARY KEY(job,identity)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_query_rows (
 job INTEGER NOT NULL, kind TEXT NOT NULL,
 k0 INTEGER NOT NULL, k1 INTEGER NOT NULL, k2 INTEGER NOT NULL,
 k3 INTEGER NOT NULL, k4 INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 identity TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(job,kind,k0,k1,k2,k3,k4,ordinal,identity)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_query_row_identity ON native_query_rows(job,kind,identity,k0,k1,k2,k3,k4,ordinal);
";

#[derive(Clone, Debug)]
pub(crate) struct SourceWitness {
    /// Immutable Original catalog source proof identity.
    pub(crate) source: CatalogSource,
    /// Separately captured Current ledger; it can be absent or newer than `source`.
    pub(crate) ledger: Option<Ledger>,
    /// Current catalog invalidation observation at capture time.
    pub(crate) claim: Option<i64>,
    /// Current archive marker observation at capture time.
    pub(crate) archive: Option<String>,
    /// Current source-mutation evidence at capture time.
    pub(crate) mutation: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EventAddress {
    pub(crate) extra: bool,
    pub(crate) seq: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceHeader {
    pub(crate) witness: SourceWitness,
    pub(crate) entrypoint: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) last_model: Option<String>,
    pub(crate) failed: bool,
    pub(crate) first: Option<i64>,
    pub(crate) last: Option<i64>,
    pub(crate) cwd: Option<String>,
    pub(crate) branch: Option<String>,
    /// Missing primitives are repair interest, never permission to decode a
    /// whole header or fabricate facts for an older publication.
    pub(crate) primitives_ready: bool,
    pub(crate) counts: Option<(usize, usize, usize)>,
    /// `None` requires bounded primitive repair. `Some` with an absent time
    /// qualifies a wholly untimed source; it does not invent i64::MIN evidence.
    pub(crate) time_seed: Option<TimeSeed>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TimeSeed {
    pub(crate) first_seq: Option<usize>,
    pub(crate) first_ms: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnerWitness {
    pub(crate) identity: String,
    pub(crate) key: String,
    catalog: Option<String>,
    history: Option<String>,
    private_header: Option<String>,
    trace_header: Option<String>,
    slot_header: Option<String>,
    history_header: Option<String>,
}

pub(crate) struct CleanupAttempt {
    pub(crate) outcome: Outcome,
    pub(crate) work: super::native_rank::Work,
    pub(crate) connection_vm_steps: u64,
}

/// Work attempted before an error, including statements later rolled back.
/// Nested owners return their metadata counters only on success, so a failed
/// attempt's known work is a lower bound, never an exact or committed receipt.
#[derive(Debug)]
pub(crate) struct CleanupReceipt {
    pub(crate) known_work: super::native_rank::Work,
    pub(crate) connection_vm_steps: u64,
    pub(crate) work_complete: bool,
}

impl CleanupReceipt {
    /// Also available through the compatibility `cleanup` error path.
    pub(crate) fn from_error(error: &io::Error) -> Option<&Self> {
        error
            .get_ref()?
            .downcast_ref::<CleanupFailure>()
            .map(|failure| &failure.receipt)
    }
}

#[derive(Debug)]
struct CleanupFailure {
    source: io::Error,
    receipt: CleanupReceipt,
}

impl std::fmt::Display for CleanupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, formatter)
    }
}

impl std::error::Error for CleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JobSpec {
    pub(crate) view_key: String,
    pub(crate) version: u32,
    pub(crate) catalog_generation: Option<String>,
    pub(crate) runtime: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Job {
    pub(crate) id: i64,
    pub(crate) spec: JobSpec,
    pub(crate) phase: String,
    pub(crate) revision: i64,
    pub(crate) cursor: String,
    pub(crate) state: Value,
}

/// The five native ordering columns remain typed SQL integers. ordinal keeps
/// stable insertion order when several placements share the same native key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct RowKey {
    pub(crate) order: [i64; 5],
    pub(crate) ordinal: i64,
    pub(crate) identity: String,
}

#[derive(Clone, Debug)]
pub(crate) struct StageRow {
    pub(crate) kind: RowKind,
    pub(crate) key: RowKey,
    pub(crate) facts: Value,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RowKind {
    Entry,
    Annotation,
    Group,
    Member,
    Sent,
    Started,
    Operation,
    Copied,
    Relation,
    SourceFact,
    Slot,
    Observation,
    Counter,
    Metadata,
}
impl RowKind {
    fn name(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Annotation => "annotation",
            Self::Group => "group",
            Self::Member => "member",
            Self::Sent => "sent",
            Self::Started => "started",
            Self::Operation => "operation",
            Self::Copied => "copied",
            Self::Relation => "relation",
            Self::SourceFact => "source",
            Self::Slot => "slot",
            Self::Observation => "observation",
            Self::Counter => "counter",
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Work {
    pub(crate) decoded_rows: usize,
    pub(crate) metadata_bytes: usize,
    pub(crate) vm_steps: i32,
}
#[derive(Clone, Debug)]
pub(crate) struct Page<T, K> {
    pub(crate) rows: Vec<T>,
    pub(crate) after: Option<K>,
    pub(crate) complete: bool,
    pub(crate) work: Work,
}

pub(crate) struct NativeStore {
    pub(super) connection: Connection,
    pub(super) options: crate::Options,
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(io::Error::new(
        io::ErrorKind::InvalidData,
        message,
    )))
}
fn guard_columns(columns: &str, length: &str) -> String {
    columns
        .split(',')
        .map(str::trim)
        .map(|column| format!("CASE WHEN ({length})<=1048576 THEN {column} END"))
        .collect::<Vec<_>>()
        .join(",")
}
fn bounded_json<T: Serialize>(value: &T, maximum: usize) -> rusqlite::Result<String> {
    let text = json(value)?;
    if text.len() > maximum {
        return Err(invalid("native producer metadata budget exceeded"));
    }
    Ok(text)
}
fn bounded_value(db: &Connection, key: &str, maximum: usize) -> rusqlite::Result<Option<String>> {
    let row: Option<(i64, Option<String>)> = db.query_row("SELECT octet_length(value),CASE WHEN octet_length(value)<=?2 THEN value END FROM meta WHERE key=?1", params![key, maximum as i64], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    match row {
        Some((_, None)) => Err(invalid("native producer witness exceeds metadata budget")),
        Some((_, text)) => Ok(text),
        None => Ok(None),
    }
}
fn header(db: &Connection, table: &str, key: &str) -> rusqlite::Result<Option<String>> {
    let columns = match table {
        "session_trace_headers" => "generation,source_generation",
        "session_slot_projections" => "generation,version,total,source_generation",
        "session_history_projections" => "generation,version,total,source_generation,incomplete",
        _ => return Err(invalid("unknown native producer header")),
    };
    let length = columns
        .split(',')
        .map(|column| format!("COALESCE(octet_length({column}),0)"))
        .collect::<Vec<_>>()
        .join("+");
    let sql = format!(
        "SELECT CASE WHEN ({length})<=65536 THEN json_array({columns}) END FROM {table} WHERE session_key=?1"
    );
    let row: Option<Option<String>> = db.query_row(&sql, [key], |r| r.get(0)).optional()?;
    match row {
        Some(None) => Err(invalid("native header witness exceeds metadata budget")),
        Some(text) => Ok(text),
        None => Ok(None),
    }
}
fn source_matches(source: &CatalogSource, ledger: &Ledger) -> bool {
    source.dev == ledger.stat.dev
        && source.ino == ledger.stat.ino
        && source.size == ledger.stat.size
        && source.modified_ns == ledger.stat.modified_ns
        && source.changed_ns == ledger.stat.changed_ns
        && source.offset == ledger.offset
        && source.prefix_sha256 == ledger.prefix
        && source.tail_sha256 == ledger.tail
}
fn capture_source_observation(
    db: &Connection,
    source: &CatalogSource,
) -> rusqlite::Result<SourceWitness> {
    if source.path.to_str().is_none() {
        return Err(invalid("native source path is not portable"));
    }
    let ledger = ledger_row(db, &source.path.to_string_lossy())?.map(|(_, ledger)| ledger);
    let path = source.path.to_string_lossy();
    let mutation: Option<Option<String>> = db.query_row("SELECT CASE WHEN octet_length(evidence)<=65536 THEN evidence END FROM session_source_mutations WHERE source_path=?1", [&*path], |r| r.get(0)).optional()?;
    if matches!(mutation, Some(None)) {
        return Err(invalid(
            "native source mutation witness exceeds metadata budget",
        ));
    }
    Ok(SourceWitness {
        source: source.clone(),
        ledger,
        claim: db
            .query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [&*path],
                |r| r.get(0),
            )
            .optional()?,
        archive: bounded_value(db, &archive_index_key(&path), 16384)?,
        mutation: mutation.flatten(),
    })
}
fn capture_source(db: &Connection, source: &CatalogSource) -> rusqlite::Result<SourceWitness> {
    let witness = capture_source_observation(db, source)?;
    if witness
        .ledger
        .as_ref()
        .is_some_and(|ledger| !source_matches(source, ledger))
    {
        return Err(invalid("native source generation changed"));
    }
    Ok(witness)
}
fn check_source(db: &Connection, expected: &SourceWitness) -> rusqlite::Result<bool> {
    if !private_current(db)? {
        return Ok(false);
    }
    let actual = capture_source_observation(db, &expected.source)?;
    Ok(actual.ledger == expected.ledger
        && actual.claim == expected.claim
        && actual.archive == expected.archive
        && actual.mutation == expected.mutation)
}
fn current_file_if_present(
    db: &Connection,
    witness: &SourceWitness,
) -> rusqlite::Result<Option<i64>> {
    if !check_source(db, witness)? {
        return Err(invalid("native source witness changed"));
    }
    let Some((file, ledger)) = ledger_row(db, witness.source.path.to_string_lossy().as_ref())?
    else {
        return Ok(None);
    };
    if !source_matches(&witness.source, &ledger) {
        return Err(invalid(
            "native Current source does not match original identity",
        ));
    }
    Ok(Some(file))
}
fn current_file(db: &Connection, witness: &SourceWitness) -> rusqlite::Result<i64> {
    current_file_if_present(db, witness)?.ok_or_else(|| invalid("native Current source is absent"))
}
fn sequence_sql(value: usize) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|_| invalid("native sequence exceeds SQLite integer range"))
}
fn offset_sql_bits(value: u64) -> rusqlite::Result<i64> {
    const SIGN_BIT: u64 = 1 << 63;
    if value < SIGN_BIT {
        return i64::try_from(value).map_err(|_| invalid("native offset exceeds SQLite range"));
    }
    let low_bits = i64::try_from(value - SIGN_BIT)
        .map_err(|_| invalid("native offset exceeds SQLite integer width"))?;
    i64::MIN
        .checked_add(low_bits)
        .ok_or_else(|| invalid("native offset exceeds SQLite integer width"))
}
fn add_work(total: &mut Work, part: Work) -> io::Result<()> {
    total.decoded_rows = total
        .decoded_rows
        .checked_add(part.decoded_rows)
        .ok_or_else(|| io::Error::other("native work row count overflow"))?;
    total.metadata_bytes = total
        .metadata_bytes
        .checked_add(part.metadata_bytes)
        .ok_or_else(|| io::Error::other("native work byte count overflow"))?;
    total.vm_steps = total
        .vm_steps
        .checked_add(part.vm_steps)
        .ok_or_else(|| io::Error::other("native work VM step count overflow"))?;
    Ok(())
}
fn order_ready(db: &Connection, file: i64, ledger: &Ledger) -> io::Result<Work> {
    let expected = source_order_generation(ledger).map_err(io::Error::other)?;
    let mut query = db
        .prepare("SELECT generation FROM native_source_order_facts WHERE file_id=?1")
        .map_err(io::Error::other)?;
    let generation: Option<String> = query
        .query_row([file], |row| row.get(0))
        .optional()
        .map_err(io::Error::other)?;
    let work = Work {
        decoded_rows: if generation.is_some() { 1 } else { 0 },
        metadata_bytes: generation.as_ref().map_or(0, String::len),
        vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        ..Work::default()
    };
    if generation.as_deref() != Some(expected.as_str()) {
        return Err(io::Error::other(
            "native time primitives require bounded repair",
        ));
    }
    Ok(work)
}
fn child_operations_ready(db: &Connection, file: i64, ledger: &Ledger) -> io::Result<Work> {
    let expected = source_operation_generation(ledger).map_err(io::Error::other)?;
    let mut query = db
        .prepare("SELECT generation FROM native_source_child_operation_facts WHERE file_id=?1")
        .map_err(io::Error::other)?;
    let generation: Option<String> = query
        .query_row([file], |row| row.get(0))
        .optional()
        .map_err(io::Error::other)?;
    let work = Work {
        decoded_rows: if generation.is_some() { 1 } else { 0 },
        metadata_bytes: generation.as_ref().map_or(0, String::len),
        vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        ..Work::default()
    };
    if generation.as_deref() != Some(expected.as_str()) {
        return Err(io::Error::other(
            "native child operation primitives require bounded repair",
        ));
    }
    Ok(work)
}
fn source_time_at_index(
    db: &Connection,
    file: i64,
    regular_seq: usize,
) -> io::Result<(SourceTime, Work)> {
    let seq = sequence_sql(regular_seq).map_err(io::Error::other)?;
    let mut query = db
        .prepare("SELECT t FROM native_source_event_times WHERE file_id=?1 AND seq<=?2 ORDER BY seq DESC LIMIT 1")
        .map_err(io::Error::other)?;
    let time: Option<i64> = query
        .query_row(params![file, seq], |row| row.get(0))
        .optional()
        .map_err(io::Error::other)?;
    let work = Work {
        decoded_rows: if time.is_some() { 1 } else { 0 },
        metadata_bytes: if time.is_some() {
            std::mem::size_of::<i64>()
        } else {
            0
        },
        vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
    };
    Ok((
        time.map_or(SourceTime::BeforeFirst, SourceTime::Observed),
        work,
    ))
}
fn sequence_before_offset_index(
    db: &Connection,
    file: i64,
    offset: u64,
) -> io::Result<(Option<usize>, Work)> {
    let encoded = offset_sql_bits(offset).map_err(io::Error::other)?;
    let mut work = Work::default();
    let sequence = if offset < (1 << 63) {
        let mut query = db
            .prepare("SELECT seq FROM native_source_event_positions WHERE file_id=?1 AND o>=0 AND o<=?2 ORDER BY o DESC,seq DESC LIMIT 1")
            .map_err(io::Error::other)?;
        let seq: Option<i64> = query
            .query_row(params![file, encoded], |row| row.get(0))
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        if seq.is_some() {
            work.decoded_rows = 1;
            work.metadata_bytes = std::mem::size_of::<i64>();
        }
        seq
    } else {
        let mut high = db
            .prepare("SELECT seq FROM native_source_event_positions WHERE file_id=?1 AND o<0 AND o<=?2 ORDER BY o DESC,seq DESC LIMIT 1")
            .map_err(io::Error::other)?;
        let high_seq: Option<i64> = high
            .query_row(params![file, encoded], |row| row.get(0))
            .optional()
            .map_err(io::Error::other)?;
        add_work(
            &mut work,
            Work {
                decoded_rows: if high_seq.is_some() { 1 } else { 0 },
                metadata_bytes: if high_seq.is_some() {
                    std::mem::size_of::<i64>()
                } else {
                    0
                },
                vm_steps: high.get_status(rusqlite::StatementStatus::VmStep),
            },
        )?;
        if high_seq.is_some() {
            high_seq
        } else {
            let mut low = db
                .prepare("SELECT seq FROM native_source_event_positions WHERE file_id=?1 AND o>=0 ORDER BY o DESC,seq DESC LIMIT 1")
                .map_err(io::Error::other)?;
            let low_seq: Option<i64> = low
                .query_row([file], |row| row.get(0))
                .optional()
                .map_err(io::Error::other)?;
            add_work(
                &mut work,
                Work {
                    decoded_rows: if low_seq.is_some() { 1 } else { 0 },
                    metadata_bytes: if low_seq.is_some() {
                        std::mem::size_of::<i64>()
                    } else {
                        0
                    },
                    vm_steps: low.get_status(rusqlite::StatementStatus::VmStep),
                },
            )?;
            low_seq
        }
    };
    let sequence = sequence
        .map(|seq| usize::try_from(seq).map_err(|_| io::Error::other("native sequence is invalid")))
        .transpose()?;
    Ok((sequence, work))
}
fn source_time_before_offset_index(
    db: &Connection,
    file: i64,
    offset: u64,
) -> io::Result<(SourceTime, Work)> {
    let (seq, mut work) = sequence_before_offset_index(db, file, offset)?;
    let (time, time_work) = source_time_at_index(db, file, seq.unwrap_or(0))?;
    add_work(&mut work, time_work)?;
    Ok((time, work))
}
pub(super) fn pending(db: &Connection) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_query_jobs WHERE phase<>'ready')",
        [],
        |r| r.get(0),
    )
}

impl NativeStore {
    pub(super) fn runtime_current(&self, spec: &JobSpec) -> io::Result<bool> {
        Ok(crate::native_evidence::query_runtime_generation(&self.options)? == spec.runtime)
    }
    pub(crate) fn begin(&mut self, spec: &JobSpec) -> io::Result<Job> {
        bounded_json(spec, STATE_BYTES).map_err(io::Error::other)?;
        if !self.runtime_current(spec)? {
            return Err(io::Error::other("native producer runtime changed"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !spec_current(&tx, spec).map_err(io::Error::other)? {
            return Err(io::Error::other("native producer publication changed"));
        }
        if let Some(mut old) = read_job(&tx, &spec.view_key).map_err(io::Error::other)? {
            if old.spec != *spec || !witnesses_current(&tx, old.id).map_err(io::Error::other)? {
                tx.execute(
                    "UPDATE native_query_jobs SET phase='cleanup',revision=revision+1 WHERE job=?1",
                    [old.id],
                )
                .map_err(io::Error::other)?;
                tx.execute("DELETE FROM native_query_ready WHERE job=?1", [old.id])
                    .map_err(io::Error::other)?;
                old.phase = "cleanup".into();
                old.revision += 1;
                tx.commit().map_err(io::Error::other)?;
                return Ok(old);
            }
            return Ok(old);
        }
        tx.execute("INSERT INTO meta(key,value) VALUES('native_query_epoch','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)", []).map_err(io::Error::other)?;
        let id: i64 = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key='native_query_epoch'",
                [],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_query_jobs(job,view_key,version,catalog_generation,runtime,phase,revision,cursor,state) VALUES(?1,?2,?3,?4,?5,'capture',0,'','{}')", params![id,spec.view_key,spec.version,spec.catalog_generation,spec.runtime]).map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok(Job {
            id,
            spec: spec.clone(),
            phase: "capture".into(),
            revision: 0,
            cursor: String::new(),
            state: serde_json::json!({}),
        })
    }
    /// Witness admission is paged and private. The caller supplies the native
    /// closure; no partial admission can be qualified as a reader view.
    pub(crate) fn admit(
        &mut self,
        job: &Job,
        sources: &[SourceWitness],
        owners: &[OwnerWitness],
        cursor: &str,
    ) -> io::Result<Outcome> {
        if sources.len() + owners.len() > ROWS {
            return Err(io::Error::other("native witness row budget exceeded"));
        }
        let mut bytes = cursor.len();
        let source_text = sources
            .iter()
            .map(|s| bounded_json(&s.source, BYTES))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(io::Error::other)?;
        for (source, text) in sources.iter().zip(&source_text) {
            bytes += source_witness_bytes(source, text);
        }
        for owner in owners {
            bytes += owner_bytes(owner);
        }
        if bytes > BYTES || cursor.len() > STATE_BYTES {
            return Err(io::Error::other("native witness metadata budget exceeded"));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(Outcome::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if job.phase != "capture" || !job_current(&tx, job).map_err(io::Error::other)? {
            return Ok(Outcome::Conflict);
        }
        for (source, text) in sources.iter().zip(source_text) {
            if !check_source(&tx, source).map_err(io::Error::other)? {
                return Ok(Outcome::Conflict);
            }
            let ledger = source.ledger.as_ref();
            tx.execute("INSERT INTO native_query_sources VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(job,path) DO NOTHING", params![job.id,source.source.path.to_string_lossy(),text,ledger.is_some(),ledger.map(|l|int(l.stat.dev)),ledger.map(|l|int(l.stat.ino)),ledger.map(|l|int(l.stat.size)),ledger.map(|l|l.stat.modified_ns.to_be_bytes()),ledger.and_then(|l|l.stat.changed_ns).map(i128::to_be_bytes),ledger.map(|l|int(l.offset)),ledger.map(|l|l.prefix),ledger.map(|l|l.tail),source.claim,source.archive,source.mutation]).map_err(io::Error::other)?;
        }
        for owner in owners {
            if !owner_current(&tx, owner).map_err(io::Error::other)? {
                return Ok(Outcome::Conflict);
            }
            tx.execute("INSERT INTO native_query_owners VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(job,identity) DO NOTHING", params![job.id,owner.identity,owner.key,owner.catalog,owner.history,owner.private_header,owner.trace_header,owner.slot_header,owner.history_header]).map_err(io::Error::other)?;
        }
        let sources_count: i64 = tx
            .query_row(
                "SELECT count(*) FROM native_query_sources WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        let owners_count: i64 = tx
            .query_row(
                "SELECT count(*) FROM native_query_owners WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        if sources_count > 64 || owners_count > 64 {
            return Err(io::Error::other("native producer closure budget exceeded"));
        }
        tx.execute("UPDATE native_query_jobs SET sources=?2,owners=?3,cursor=?4,revision=revision+1 WHERE job=?1", params![job.id,sources_count,owners_count,cursor]).map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
    pub(crate) fn reload(&self, view_key: &str) -> io::Result<Option<Job>> {
        read_job(&self.connection, view_key).map_err(io::Error::other)
    }

    /// Private unfinished work, including cleanup, in a bounded indexed page.
    /// Stale-runtime jobs are returned so the scheduler can restart them; this
    /// accessor grants no right to consume their staged outputs.
    pub(crate) fn pending_jobs(&self, after: Option<i64>) -> io::Result<Page<Job, i64>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !current(&tx).map_err(io::Error::other)? {
            return Err(io::Error::other("native store version changed"));
        }
        let length = "192+octet_length(view_key)+COALESCE(octet_length(catalog_generation),0)+octet_length(runtime)+octet_length(phase)+octet_length(cursor)+octet_length(state)";
        let columns = guard_columns(
            "job,version,catalog_generation,runtime,phase,revision,cursor,state,view_key",
            length,
        );
        let mut query = tx.prepare(&format!("SELECT {columns},{length} FROM native_query_jobs WHERE phase<>'ready' AND job>?1 ORDER BY job LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = query
            .query([after.unwrap_or(-1)])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(9).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other("native job exceeds metadata budget"));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let state: String = row.get(7).map_err(io::Error::other)?;
            let job = Job {
                id: row.get(0).map_err(io::Error::other)?,
                spec: JobSpec {
                    view_key: row.get(8).map_err(io::Error::other)?,
                    version: row.get(1).map_err(io::Error::other)?,
                    catalog_generation: row.get(2).map_err(io::Error::other)?,
                    runtime: row.get(3).map_err(io::Error::other)?,
                },
                phase: row.get(4).map_err(io::Error::other)?,
                revision: row.get(5).map_err(io::Error::other)?,
                cursor: row.get(6).map_err(io::Error::other)?,
                state: serde_json::from_str(&state).map_err(io::Error::other)?,
            };
            end = Some(job.id);
            output.push(job);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        drop(query);
        let remains: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_query_jobs WHERE phase<>'ready' AND job>?1)",
                [end.unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    /// Return the originally admitted Catalog/custody/ledger witness. Source
    /// pages are ordered by path; native source ordinals belong to producer
    /// SourceFact rows and are never reconstructed from this lexical order.
    pub(crate) fn admitted_sources_page(
        &self,
        job: &Job,
        after: Option<&str>,
    ) -> io::Result<Page<SourceWitness, String>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native producer runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)? {
            return Err(io::Error::other("native stage witness changed"));
        }
        let length = "64+octet_length(path)+octet_length(metadata)+COALESCE(octet_length(archive),0)+COALESCE(octet_length(mutation),0)";
        let columns = guard_columns(
            "0,dev,ino,size,mtime_ns,resume_at,prefix,tail,ctime_ns,metadata,present,claim,archive,mutation",
            length,
        );
        let comparison = if after.is_some() { ">" } else { ">=" };
        let mut query=tx.prepare(&format!("SELECT {columns},{length} FROM native_query_sources WHERE job=?1 AND path{comparison}?2 ORDER BY path LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = query
            .query(params![job.id, after.unwrap_or("")])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.map(str::to_owned);
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(14).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native source witness exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let text: String = row.get(9).map_err(io::Error::other)?;
            let source: CatalogSource = serde_json::from_str(&text).map_err(io::Error::other)?;
            let ledger = if row.get::<_, bool>(10).map_err(io::Error::other)? {
                Some(read_ledger(row).map_err(io::Error::other)?.1)
            } else {
                None
            };
            end = Some(
                source
                    .path
                    .to_str()
                    .ok_or_else(|| io::Error::other("native source path is not portable"))?
                    .to_owned(),
            );
            output.push(SourceWitness {
                source,
                ledger,
                claim: row.get(11).map_err(io::Error::other)?,
                archive: row.get(12).map_err(io::Error::other)?,
                mutation: row.get(13).map_err(io::Error::other)?,
            });
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        drop(query);
        let comparison = if end.is_some() { ">" } else { ">=" };
        let remains:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM native_query_sources WHERE job=?1 AND path{comparison}?2)"),params![job.id,end.as_deref().unwrap_or("")],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn admitted_owner(
        &self,
        job: &Job,
        identity: &str,
    ) -> io::Result<Option<OwnerWitness>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native producer runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)? {
            return Err(io::Error::other("native stage witness changed"));
        }
        let length = "octet_length(identity)+octet_length(session_key)+COALESCE(octet_length(catalog),0)+COALESCE(octet_length(history),0)+COALESCE(octet_length(private_header),0)+COALESCE(octet_length(trace_header),0)+COALESCE(octet_length(slot_header),0)+COALESCE(octet_length(history_header),0)";
        let columns = guard_columns(
            "identity,session_key,catalog,history,private_header,trace_header,slot_header,history_header",
            length,
        );
        let mut query = tx
            .prepare(&format!(
                "SELECT {columns},{length} FROM native_query_owners WHERE job=?1 AND identity=?2"
            ))
            .map_err(io::Error::other)?;
        let mut rows = query
            .query(params![job.id, identity])
            .map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        if row.get::<_, i64>(8).map_err(io::Error::other)? > BYTES as i64 {
            return Err(io::Error::other(
                "native owner witness exceeds metadata budget",
            ));
        }
        Ok(Some(OwnerWitness {
            identity: row.get(0).map_err(io::Error::other)?,
            key: row.get(1).map_err(io::Error::other)?,
            catalog: row.get(2).map_err(io::Error::other)?,
            history: row.get(3).map_err(io::Error::other)?,
            private_header: row.get(4).map_err(io::Error::other)?,
            trace_header: row.get(5).map_err(io::Error::other)?,
            slot_header: row.get(6).map_err(io::Error::other)?,
            history_header: row.get(7).map_err(io::Error::other)?,
        }))
    }
    /// All staged output and the native reducer cursor commit together. No
    /// implicit row deletion, unbounded materialization or final body load.
    pub(crate) fn checkpoint(
        &mut self,
        job: &Job,
        phase: &str,
        cursor: &str,
        state: &Value,
        rows: &[StageRow],
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Outcome> {
        if rows.len() > ROWS
            || !matches!(
                phase,
                "entries" | "groups" | "slots" | "metadata" | "finish"
            )
        {
            return Err(io::Error::other("invalid native producer checkpoint"));
        }
        let state = bounded_json(state, STATE_BYTES).map_err(io::Error::other)?;
        let texts = rows
            .iter()
            .map(|r| bounded_json(&r.facts, BYTES))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(io::Error::other)?;
        let bytes = state.len()
            + cursor.len()
            + rows
                .iter()
                .zip(&texts)
                .map(|(r, t)| t.len() + r.key.identity.len() + 64)
                .sum::<usize>();
        if bytes > BYTES || cursor.len() > STATE_BYTES {
            return Err(io::Error::other(
                "native checkpoint metadata budget exceeded",
            ));
        }
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(Outcome::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if matches!(job.phase.as_str(), "cleanup" | "ready")
            || !job_current(&tx, job).map_err(io::Error::other)?
        {
            return Ok(Outcome::Conflict);
        }
        let counts: (i64, i64) = tx
            .query_row(
                "SELECT sources,owners FROM native_query_jobs WHERE job=?1",
                [job.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(io::Error::other)?;
        if counts.0 == 0 || counts.1 == 0 {
            return Err(io::Error::other("native producer witnesses are incomplete"));
        }
        let mut put=tx.prepare_cached("INSERT INTO native_query_rows VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(job,kind,k0,k1,k2,k3,k4,ordinal,identity) DO UPDATE SET metadata=excluded.metadata") .map_err(io::Error::other)?;
        for (row, text) in rows.iter().zip(texts) {
            let k = &row.key;
            put.execute(params![
                job.id,
                row.kind.name(),
                k.order[0],
                k.order[1],
                k.order[2],
                k.order[3],
                k.order[4],
                k.ordinal,
                k.identity,
                text
            ])
            .map_err(io::Error::other)?;
        }
        drop(put);
        tx.execute("UPDATE native_query_jobs SET phase=?2,cursor=?3,state=?4,revision=revision+1 WHERE job=?1",params![job.id,phase,cursor,state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
    /// The native caller verifies completed closure/ranks under this same
    /// transaction. Ready is one pointer to immutable, already-staged rows.
    pub(crate) fn qualify(
        &mut self,
        job: &Job,
        metadata: &Value,
        completed: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Outcome> {
        let metadata = bounded_json(metadata, STATE_BYTES).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(Outcome::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if job.phase != "finish"
            || !job_current(&tx, job).map_err(io::Error::other)?
            || !completed(&tx, job.id).map_err(io::Error::other)?
        {
            return Ok(Outcome::Conflict);
        }
        tx.execute("INSERT INTO native_query_ready VALUES(?1,?2,?3) ON CONFLICT(view_key) DO UPDATE SET job=excluded.job,metadata=excluded.metadata",params![job.spec.view_key,job.id,metadata]).map_err(io::Error::other)?;
        tx.execute(
            "UPDATE native_query_jobs SET phase='ready',revision=revision+1 WHERE job=?1",
            [job.id],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
    pub(crate) fn ready(
        &self,
        view_key: &str,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    ) -> io::Result<Option<(Job, Value)>> {
        let Some(job) = read_job(&self.connection, view_key).map_err(io::Error::other)? else {
            return Ok(None);
        };
        if job.phase != "ready" || !self.runtime_current(&job.spec)? {
            return Ok(None);
        };
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, &job).map_err(io::Error::other)? {
            return Ok(None);
        };
        if !closure(&tx, job.id).map_err(io::Error::other)? {
            return Ok(None);
        }
        let text: Option<String> = tx
            .query_row(
                "SELECT metadata FROM native_query_ready WHERE view_key=?1 AND job=?2",
                params![view_key, job.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        text.map(|text| {
            serde_json::from_str(&text)
                .map(|value| (job, value))
                .map_err(io::Error::other)
        })
        .transpose()
    }
    pub(crate) fn cleanup(&mut self, job: &Job) -> io::Result<Outcome> {
        self.cleanup_with_work(job).map(|attempt| attempt.outcome)
    }

    /// Actual complete-attempt receipt, including the native controller's
    /// scalar admission, empty probes, bookkeeping and COMMIT VM instructions.
    /// Errors preserve their original kind, message and source, with attempted
    /// work accessible through `CleanupReceipt::from_error`. Their VM interval
    /// includes the failed statement and transaction rollback; metadata work
    /// is explicitly partial when an inner owner cannot return its counters.
    pub(crate) fn cleanup_with_work(&mut self, job: &Job) -> io::Result<CleanupAttempt> {
        let meter = super::native_rank::cleanup_vm_meter(&self.connection);
        let mut work = super::native_rank::Work::default();
        let result: io::Result<Outcome> = (|| {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(io::Error::other)?;
            // Cleanup needs only a CAS and the final deletion size. Avoid
            // decoding the former job's cursor/state just to retire it.
            work.sql_reads += 1;
            let header: Option<(bool, i64)> = tx.query_row(
                "SELECT job=?2 AND phase='cleanup' AND revision=?3,64+octet_length(view_key)+COALESCE(octet_length(catalog_generation),0)+octet_length(runtime)+octet_length(phase)+octet_length(cursor)+octet_length(state) FROM native_query_jobs WHERE view_key=?1",
                params![job.spec.view_key, job.id, job.revision],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional().map_err(io::Error::other)?;
            let Some((true, header_bytes)) = header else {
                return Ok(Outcome::Conflict);
            };
            let header_bytes = usize::try_from(header_bytes).map_err(io::Error::other)?;
            let mut bytes_left = BYTES;
            // Reserve the final remains probe and revision/delete write before
            // invoking any nested owner. Every nested allowance is residual.
            let step = super::native_rank::cleanup_with_allowance(
                &tx,
                job.id,
                ROWS,
                ROWS - work.rows() - 2,
                &mut bytes_left,
            )
            .map_err(io::Error::other)?;
            let rank_complete = step.complete;
            work.add(step.work);
            if rank_complete {
                let allowance = ROWS - 2;
                let rows_complete =
                    cleanup_rows(&tx, job.id, allowance, &mut bytes_left, &mut work)
                        .map_err(io::Error::other)?;
                if rows_complete
                    && cleanup_witnesses(
                        &tx,
                        job.id,
                        "sources",
                        allowance,
                        &mut bytes_left,
                        &mut work,
                    )
                    .map_err(io::Error::other)?
                {
                    cleanup_witnesses(&tx, job.id, "owners", allowance, &mut bytes_left, &mut work)
                        .map_err(io::Error::other)?;
                }
            }
            work.sql_reads += 1;
            let remains: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_query_rows WHERE job=?1) OR EXISTS(SELECT 1 FROM native_query_sources WHERE job=?1) OR EXISTS(SELECT 1 FROM native_query_owners WHERE job=?1)",
                [job.id], |r| r.get(0),
            ).map_err(io::Error::other)?;
            let finished = rank_complete && !remains && header_bytes <= bytes_left;
            work.sql_writes += 1;
            if finished {
                tx.execute("DELETE FROM native_query_jobs WHERE job=?1", [job.id])
                    .map_err(io::Error::other)?;
                work.metadata_bytes += header_bytes;
            } else {
                tx.execute(
                    "UPDATE native_query_jobs SET revision=revision+1 WHERE job=?1",
                    [job.id],
                )
                .map_err(io::Error::other)?;
            }
            tx.commit().map_err(io::Error::other)?;
            Ok(if finished {
                Outcome::Written
            } else {
                Outcome::Deferred
            })
        })();
        self.connection.progress_handler(0, None::<fn() -> bool>);
        let connection_vm_steps = meter.load(std::sync::atomic::Ordering::Relaxed);
        match result {
            Ok(outcome) => Ok(CleanupAttempt {
                outcome,
                work,
                connection_vm_steps,
            }),
            Err(source) => Err(io::Error::new(
                source.kind(),
                CleanupFailure {
                    source,
                    receipt: CleanupReceipt {
                        known_work: work,
                        connection_vm_steps,
                        work_complete: false,
                    },
                },
            )),
        }
    }
    /// The native resolver can discover a new dependency without changing any
    /// captured row. Invalidating its private work preserves original custody.
    pub(crate) fn restart(&mut self, job: &Job) -> io::Result<Outcome> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if read_job(&tx, &job.spec.view_key)
            .map_err(io::Error::other)?
            .is_none_or(|actual| actual.id != job.id || actual.revision != job.revision)
        {
            return Ok(Outcome::Conflict);
        }
        tx.execute(
            "UPDATE native_query_jobs SET phase='cleanup',revision=revision+1 WHERE job=?1",
            [job.id],
        )
        .map_err(io::Error::other)?;
        tx.execute("DELETE FROM native_query_ready WHERE job=?1", [job.id])
            .map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
}

fn spec_current(db: &Connection, spec: &JobSpec) -> rusqlite::Result<bool> {
    if !private_current(db)? {
        return Ok(false);
    }
    let Some(expected) = &spec.catalog_generation else {
        return Ok(true);
    };
    Ok(db
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .as_ref()
        == Some(expected))
}
struct CleanupRow {
    kind: Option<String>,
    order: [i64; 5],
    ordinal: i64,
    identity: Option<String>,
    bytes: i64,
}

fn cleanup_rows(
    tx: &Transaction<'_>,
    job: i64,
    allowance: usize,
    bytes_left: &mut usize,
    work: &mut super::native_rank::Work,
) -> rusqlite::Result<bool> {
    loop {
        if work.rows() + 2 > allowance {
            return Ok(false);
        }
        work.sql_reads += 1;
        let row: Option<CleanupRow> = tx.query_row(
            "SELECT CASE WHEN octet_length(metadata)+octet_length(identity)+octet_length(kind)+64<=?2 THEN kind END,k0,k1,k2,k3,k4,ordinal,CASE WHEN octet_length(metadata)+octet_length(identity)+octet_length(kind)+64<=?2 THEN identity END,octet_length(metadata)+octet_length(identity)+octet_length(kind)+64 FROM native_query_rows WHERE job=?1 ORDER BY kind,k0,k1,k2,k3,k4,ordinal,identity LIMIT 1",
            params![job, i64::try_from(*bytes_left).map_err(|_| invalid("cleanup byte allowance invalid"))?],
            |r| Ok(CleanupRow { kind: r.get(0)?, order: [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?], ordinal: r.get(6)?, identity: r.get(7)?, bytes: r.get(8)? }),
        ).optional()?;
        let Some(CleanupRow {
            kind,
            order,
            ordinal,
            identity,
            bytes,
        }) = row
        else {
            return Ok(true);
        };
        let bytes = usize::try_from(bytes).map_err(|_| invalid("invalid native cleanup size"))?;
        if bytes > *bytes_left {
            return Ok(false);
        }
        let kind = kind.ok_or_else(|| invalid("native cleanup kind absent"))?;
        let identity = identity.ok_or_else(|| invalid("native cleanup identity absent"))?;
        *bytes_left -= bytes;
        work.metadata_bytes += bytes;
        work.sql_writes += 1;
        if tx.execute("DELETE FROM native_query_rows WHERE job=?1 AND kind=?2 AND (k0,k1,k2,k3,k4,ordinal,identity)=(?3,?4,?5,?6,?7,?8,?9)",params![job,kind,order[0],order[1],order[2],order[3],order[4],ordinal,identity])? != 1 {
            return Err(invalid("native cleanup row disappeared"));
        }
    }
}
fn cleanup_witnesses(
    tx: &Transaction<'_>,
    job: i64,
    table: &str,
    allowance: usize,
    bytes_left: &mut usize,
    work: &mut super::native_rank::Work,
) -> rusqlite::Result<bool> {
    // Sources have seven INTEGER columns (56 bytes); reserve 64 for them.
    // Owners have one INTEGER job column (8 bytes). BLOB/TEXT payloads are
    // charged separately, using the same SQL expression for admission and debit.
    let (table, key, length) = match table {
        "sources" => (
            "native_query_sources",
            "path",
            "octet_length(path)+octet_length(metadata)+COALESCE(octet_length(mtime_ns),0)+COALESCE(octet_length(ctime_ns),0)+COALESCE(octet_length(prefix),0)+COALESCE(octet_length(tail),0)+COALESCE(octet_length(archive),0)+COALESCE(octet_length(mutation),0)+64",
        ),
        "owners" => (
            "native_query_owners",
            "identity",
            "octet_length(identity)+octet_length(session_key)+COALESCE(octet_length(catalog),0)+COALESCE(octet_length(history),0)+COALESCE(octet_length(private_header),0)+COALESCE(octet_length(trace_header),0)+COALESCE(octet_length(slot_header),0)+COALESCE(octet_length(history_header),0)+8",
        ),
        _ => return Err(invalid("unknown native cleanup table")),
    };
    loop {
        if work.rows() + 2 > allowance {
            return Ok(false);
        }
        work.sql_reads += 1;
        let row: Option<(Option<String>, i64)> = tx.query_row(
            &format!("SELECT CASE WHEN {length}<=?2 THEN {key} END,{length} FROM {table} WHERE job=?1 ORDER BY {key} LIMIT 1"),
            params![job, i64::try_from(*bytes_left).map_err(|_| invalid("cleanup byte allowance invalid"))?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        let Some((key_value, bytes)) = row else {
            return Ok(true);
        };
        let bytes = usize::try_from(bytes).map_err(|_| invalid("invalid native cleanup size"))?;
        if bytes > *bytes_left {
            return Ok(false);
        }
        let key_value = key_value.ok_or_else(|| invalid("native cleanup witness key absent"))?;
        *bytes_left -= bytes;
        work.metadata_bytes += bytes;
        work.sql_writes += 1;
        if tx.execute(
            &format!("DELETE FROM {table} WHERE job=?1 AND {key}=?2"),
            params![job, key_value],
        )? != 1
        {
            return Err(invalid("native cleanup witness disappeared"));
        }
    }
}

fn read_job(db: &Connection, key: &str) -> rusqlite::Result<Option<Job>> {
    db.query_row("SELECT job,version,catalog_generation,runtime,phase,revision,cursor,state FROM native_query_jobs WHERE view_key=?1",[key],|r|{
        let state:String=r.get(7)?;
        Ok(Job{id:r.get(0)?,spec:JobSpec{view_key:key.into(),version:r.get(1)?,catalog_generation:r.get(2)?,runtime:r.get(3)?},phase:r.get(4)?,revision:r.get(5)?,cursor:r.get(6)?,state:from_json(7,&state)?})
    }).optional()
}
pub(super) fn job_current(db: &Connection, job: &Job) -> rusqlite::Result<bool> {
    Ok(spec_current(db, &job.spec)?
        && read_job(db, &job.spec.view_key)?.is_some_and(|a| {
            a.id == job.id
                && a.revision == job.revision
                && a.phase == job.phase
                && a.spec == job.spec
        })
        && witnesses_current(db, job.id)?)
}
// Match cleanup_witnesses' SQL debit so every admitted witness can retire
// under the same byte ceiling, including optional Current ledger BLOBs.
fn source_witness_bytes(source: &SourceWitness, metadata: &str) -> usize {
    metadata.len()
        + source.source.path.to_string_lossy().len()
        + source.archive.as_ref().map_or(0, String::len)
        + source.mutation.as_ref().map_or(0, String::len)
        + source.ledger.as_ref().map_or(0, |ledger| {
            ledger.stat.modified_ns.to_be_bytes().len()
                + ledger
                    .stat
                    .changed_ns
                    .map_or(0, |time| time.to_be_bytes().len())
                + ledger.prefix.len()
                + ledger.tail.len()
        })
        + 64
}
fn owner_bytes(owner: &OwnerWitness) -> usize {
    8 + owner.identity.len()
        + owner.key.len()
        + [
            &owner.catalog,
            &owner.history,
            &owner.private_header,
            &owner.trace_header,
            &owner.slot_header,
            &owner.history_header,
        ]
        .into_iter()
        .map(|v| v.as_ref().map_or(0, String::len))
        .sum::<usize>()
}
fn owner_current(db: &Connection, owner: &OwnerWitness) -> rusqlite::Result<bool> {
    Ok(owner_version(db, "current", &owner.key)? == owner.catalog
        && owner_version(db, "history", &owner.key)? == owner.history
        && bounded_value(
            db,
            &crate::query_projection::prefix("header", &owner.identity),
            STATE_BYTES,
        )? == owner.private_header
        && header(db, "session_trace_headers", &owner.key)? == owner.trace_header
        && header(db, "session_slot_projections", &owner.key)? == owner.slot_header
        && header(db, "session_history_projections", &owner.key)? == owner.history_header)
}
fn owner_version(db: &Connection, scope: &str, key: &str) -> rusqlite::Result<Option<String>> {
    let table = match scope {
        "current" => "session_catalog",
        "history" => "session_history_catalog",
        _ => return Err(invalid("unknown native owner scope")),
    };
    db.query_row(&format!("SELECT json_array(COALESCE((SELECT generation FROM native_catalog_owner_versions WHERE session_key=?1 AND scope=?2),0),EXISTS(SELECT 1 FROM {table} WHERE session_key=?1))"),params![key,scope],|r|r.get(0)).map(Some)
}
fn witnesses_current(db: &Connection, job: i64) -> rusqlite::Result<bool> {
    let source_changed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM native_query_sources s LEFT JOIN files f ON f.path=s.path LEFT JOIN session_catalog_invalidations i ON i.source_path=s.path LEFT JOIN meta a ON a.key='archive_event_index:'||s.path LEFT JOIN session_source_mutations m ON m.source_path=s.path WHERE s.job=?1 AND ((f.file_id IS NOT NULL)<>s.present OR f.dev IS NOT s.dev OR f.ino IS NOT s.ino OR f.size IS NOT s.size OR f.mtime_ns IS NOT s.mtime_ns OR f.ctime_ns IS NOT s.ctime_ns OR f.resume_at IS NOT s.resume_at OR f.head_sha256 IS NOT s.prefix OR f.tail_sha256 IS NOT s.tail OR i.revision IS NOT s.claim OR a.value IS NOT s.archive OR m.evidence IS NOT s.mutation))",[job],|r|r.get(0))?;
    if source_changed {
        return Ok(false);
    };
    db.query_row("SELECT NOT EXISTS(SELECT 1 FROM native_query_owners w LEFT JOIN native_catalog_owner_versions cv ON cv.session_key=w.session_key AND cv.scope='current' LEFT JOIN native_catalog_owner_versions hv ON hv.session_key=w.session_key AND hv.scope='history' LEFT JOIN session_catalog c ON c.session_key=w.session_key LEFT JOIN session_history_catalog h ON h.session_key=w.session_key LEFT JOIN meta p ON p.key='native_query_header:'||octet_length(w.identity)||':'||w.identity||':' LEFT JOIN session_trace_headers t ON t.session_key=w.session_key LEFT JOIN session_slot_projections s ON s.session_key=w.session_key LEFT JOIN session_history_projections r ON r.session_key=w.session_key WHERE w.job=?1 AND (json_array(COALESCE(cv.generation,0),c.session_key IS NOT NULL) IS NOT w.catalog OR json_array(COALESCE(hv.generation,0),h.session_key IS NOT NULL) IS NOT w.history OR p.value IS NOT w.private_header OR COALESCE(octet_length(t.generation),0)+COALESCE(octet_length(t.source_generation),0)>65536 OR CASE WHEN t.session_key IS NOT NULL AND COALESCE(octet_length(t.generation),0)+COALESCE(octet_length(t.source_generation),0)<=65536 THEN json_array(t.generation,t.source_generation) END IS NOT w.trace_header OR COALESCE(octet_length(s.generation),0)+COALESCE(octet_length(s.source_generation),0)+COALESCE(octet_length(s.version),0)+COALESCE(octet_length(s.total),0)>65536 OR CASE WHEN s.session_key IS NOT NULL AND COALESCE(octet_length(s.generation),0)+COALESCE(octet_length(s.source_generation),0)+COALESCE(octet_length(s.version),0)+COALESCE(octet_length(s.total),0)<=65536 THEN json_array(s.generation,s.version,s.total,s.source_generation) END IS NOT w.slot_header OR COALESCE(octet_length(r.generation),0)+COALESCE(octet_length(r.source_generation),0)+COALESCE(octet_length(r.version),0)+COALESCE(octet_length(r.total),0)+COALESCE(octet_length(r.incomplete),0)>65536 OR CASE WHEN r.session_key IS NOT NULL AND COALESCE(octet_length(r.generation),0)+COALESCE(octet_length(r.source_generation),0)+COALESCE(octet_length(r.version),0)+COALESCE(octet_length(r.total),0)+COALESCE(octet_length(r.incomplete),0)<=65536 THEN json_array(r.generation,r.version,r.total,r.source_generation,r.incomplete) END IS NOT w.history_header))",[job],|r|r.get(0))
}

impl NativeStore {
    fn selected_file(&self, witness: &SourceWitness) -> io::Result<i64> {
        current_file(&self.connection, witness).map_err(io::Error::other)
    }
    pub(crate) fn signals_page(
        &self,
        witness: &SourceWitness,
        after: Option<usize>,
    ) -> io::Result<Page<(usize, Signal), usize>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let mut q=tx.prepare(&format!("SELECT {SIGNAL_COLUMNS},64+COALESCE(octet_length(k),0)+COALESCE(octet_length(n),0) FROM signals WHERE file_id=?1 AND seq>?2 ORDER BY seq LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, after.map(seq_int).unwrap_or(-1)])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(7).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other("native signal exceeds metadata budget"));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let seq = seq_of(row.get(0).map_err(io::Error::other)?);
            output.push((
                seq,
                Signal {
                    k: untag(1, row.get(1).map_err(io::Error::other)?).map_err(io::Error::other)?,
                    o: uint(row.get(2).map_err(io::Error::other)?),
                    t: row.get(3).map_err(io::Error::other)?,
                    at: row.get(4).map_err(io::Error::other)?,
                    n: row.get(5).map_err(io::Error::other)?,
                    v: row
                        .get::<_, Option<i64>>(6)
                        .map_err(io::Error::other)?
                        .map(uint),
                },
            ));
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
            end = Some(seq);
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let remains: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM signals WHERE file_id=?1 AND seq>?2)",
                params![file, end.map(seq_int).unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    /// Count/billing facts share read_usage. Record identities page separately;
    /// its growing JSON record_ids array is never decoded by this accessor.
    pub(crate) fn usage_page(
        &self,
        witness: &SourceWitness,
        after: Option<&str>,
    ) -> io::Result<Page<(String, MessageUsage), String>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let columns = USAGE_COLUMNS.replace("record_ids", "'[]'");
        let comparison = if after.is_some() { ">" } else { ">=" };
        let length = "192+octet_length(message_id)+COALESCE(octet_length(model),0)+COALESCE(octet_length(billing_speed),0)+COALESCE(octet_length(billing_service_tier),0)";
        let columns = guard_columns(&columns, length);
        let mut q=tx.prepare(&format!("SELECT {columns},{length} FROM usage WHERE file_id=?1 AND message_id{comparison}?2 ORDER BY message_id LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, after.unwrap_or("")])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.map(str::to_owned);
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(23).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other("native usage exceeds metadata budget"));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let (id, usage) = read_usage(row).map_err(io::Error::other)?;
            end = Some(id.clone());
            output.push((id, usage));
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let comparison = if end.is_some() { ">" } else { ">=" };
        let remains:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM usage WHERE file_id=?1 AND message_id{comparison}?2)"),params![file,end.as_deref().unwrap_or("")],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn identities_page(
        &self,
        witness: &SourceWitness,
        after: Option<(&str, &str)>,
    ) -> io::Result<Page<(String, String), (String, String)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let (message, uuid) = after.unwrap_or(("", ""));
        let mut q=tx.prepare("SELECT message_id,record_uuid,octet_length(message_id)+octet_length(record_uuid) FROM usage_identity WHERE file_id=?1 AND (message_id,record_uuid)>(?2,?3) ORDER BY message_id,record_uuid LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, message, uuid])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.map(|(a, b)| (a.into(), b.into()));
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(2).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other("native identity exceeds metadata budget"));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let pair: (String, String) = (
                row.get(0).map_err(io::Error::other)?,
                row.get(1).map_err(io::Error::other)?,
            );
            end = Some(pair.clone());
            output.push(pair);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let (message, uuid) = end
            .as_ref()
            .map_or(("", ""), |(a, b)| (a.as_str(), b.as_str()));
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_identity WHERE file_id=?1 AND (message_id,record_uuid)>(?2,?3))",params![file,message,uuid],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn busy_page(
        &self,
        witness: &SourceWitness,
        after: Option<i64>,
    ) -> io::Result<Page<(i64, i64), i64>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let ready: Option<String> = tx
            .query_row(
                "SELECT generation FROM native_source_facts WHERE file_id=?1",
                [file],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        if ready.as_deref()
            != witness
                .ledger
                .as_ref()
                .map(source_fact_generation)
                .transpose()
                .map_err(io::Error::other)?
                .as_deref()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "native busy primitives require bounded repair",
            ));
        }
        let comparison = if after.is_some() { ">" } else { ">=" };
        let mut query=tx.prepare(&format!("SELECT start,end FROM native_source_busy_intervals WHERE file_id=?1 AND start{comparison}?2 ORDER BY start LIMIT 64")).map_err(io::Error::other)?;
        let output = query
            .query_map(params![file, after.unwrap_or(i64::MIN)], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(io::Error::other)?
            .collect::<rusqlite::Result<Vec<(i64, i64)>>>()
            .map_err(io::Error::other)?;
        let end = output.last().map(|interval| interval.0).or(after);
        let comparison = if end.is_some() { ">" } else { ">=" };
        let remains:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM native_source_busy_intervals WHERE file_id=?1 AND start{comparison}?2)"),params![file,end.unwrap_or(i64::MIN)],|r|r.get(0)).map_err(io::Error::other)?;
        let work = Work {
            decoded_rows: output.len(),
            metadata_bytes: output.len() * 16,
            vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        };
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn stage_page(
        &self,
        job: &Job,
        kind: RowKind,
        after: Option<&RowKey>,
    ) -> io::Result<Page<StageRow, RowKey>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native producer runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)? {
            return Err(io::Error::other("native stage witness changed"));
        }
        let default = RowKey {
            order: [i64::MIN; 5],
            ordinal: i64::MIN,
            identity: String::new(),
        };
        let a = after.unwrap_or(&default);
        let mut q=tx.prepare("SELECT k0,k1,k2,k3,k4,ordinal,identity,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,octet_length(metadata)+octet_length(identity)+64 FROM native_query_rows WHERE job=?1 AND kind=?2 AND (k0,k1,k2,k3,k4,ordinal,identity)>(?3,?4,?5,?6,?7,?8,?9) ORDER BY k0,k1,k2,k3,k4,ordinal,identity LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![
                job.id,
                kind.name(),
                a.order[0],
                a.order[1],
                a.order[2],
                a.order[3],
                a.order[4],
                a.ordinal,
                a.identity
            ])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.cloned();
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(8).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native staged fact exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let text: String = row.get(7).map_err(io::Error::other)?;
            let key = RowKey {
                order: [
                    row.get(0).map_err(io::Error::other)?,
                    row.get(1).map_err(io::Error::other)?,
                    row.get(2).map_err(io::Error::other)?,
                    row.get(3).map_err(io::Error::other)?,
                    row.get(4).map_err(io::Error::other)?,
                ],
                ordinal: row.get(5).map_err(io::Error::other)?,
                identity: row.get(6).map_err(io::Error::other)?,
            };
            end = Some(key.clone());
            output.push(StageRow {
                kind,
                key,
                facts: serde_json::from_str(&text).map_err(io::Error::other)?,
            });
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let a = end.as_ref().unwrap_or(&default);
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_query_rows WHERE job=?1 AND kind=?2 AND (k0,k1,k2,k3,k4,ordinal,identity)>(?3,?4,?5,?6,?7,?8,?9))",params![job.id,kind.name(),a.order[0],a.order[1],a.order[2],a.order[3],a.order[4],a.ordinal,a.identity],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn stage_get(
        &self,
        job: &Job,
        kind: RowKind,
        identity: &str,
    ) -> io::Result<Option<StageRow>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native producer runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)? {
            return Err(io::Error::other("native stage witness changed"));
        }
        let mut q=tx.prepare("SELECT k0,k1,k2,k3,k4,ordinal,identity,CASE WHEN octet_length(metadata)+octet_length(identity)+64<=1048576 THEN metadata END FROM native_query_rows WHERE job=?1 AND kind=?2 AND identity=?3 ORDER BY k0,k1,k2,k3,k4,ordinal LIMIT 2").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![job.id, kind.name(), identity])
            .map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        let text: String = row.get(7).map_err(io::Error::other)?;
        let key = RowKey {
            order: [
                row.get(0).map_err(io::Error::other)?,
                row.get(1).map_err(io::Error::other)?,
                row.get(2).map_err(io::Error::other)?,
                row.get(3).map_err(io::Error::other)?,
                row.get(4).map_err(io::Error::other)?,
            ],
            ordinal: row.get(5).map_err(io::Error::other)?,
            identity: row.get(6).map_err(io::Error::other)?,
        };
        if rows.next().map_err(io::Error::other)?.is_some() {
            return Err(io::Error::other("native staged identity is ambiguous"));
        }
        Ok(Some(StageRow {
            kind,
            key,
            facts: serde_json::from_str(&text).map_err(io::Error::other)?,
        }))
    }
    pub(crate) fn recheck(&self, job: &Job) -> io::Result<bool> {
        if !self.runtime_current(&job.spec)? {
            return Ok(false);
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        job_current(&tx, job).map_err(io::Error::other)
    }
    /// Fence a composition of separately snapshotted native range reads. The
    /// callback also covers newly joining relationship owners, then another
    /// owned-witness check catches mutations during that callback. No read
    /// snapshot spans the callback. Runtime filesystem work remains separate
    /// from the returned actual SQLite VM count.
    pub(crate) fn recheck_closure(
        &self,
        job: &Job,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, u64)> {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        let steps = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&steps);
        self.connection.progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        );
        let result = (|| {
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            let current = job.phase == "ready"
                && self.recheck(job)?
                && closure(&self.connection, job.id).map_err(io::Error::other)?
                && self.recheck(job)?;
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            if !current {
                return Ok(Outcome::Conflict);
            }
            Ok(Outcome::Written)
        })();
        self.connection.progress_handler(0, None::<fn() -> bool>);
        result.map(|outcome| (outcome, steps.load(Ordering::Relaxed)))
    }
    pub(crate) fn links_page(
        &self,
        witness: &SourceWitness,
        after: Option<(&str, &str)>,
    ) -> io::Result<Page<(String, String), (String, String)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let ready: Option<String> = tx
            .query_row(
                "SELECT generation FROM native_source_facts WHERE file_id=?1",
                [file],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        if ready.as_deref()
            != witness
                .ledger
                .as_ref()
                .map(source_fact_generation)
                .transpose()
                .map_err(io::Error::other)?
                .as_deref()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "native lineage primitives require bounded repair",
            ));
        }
        let (kind, id) = after.unwrap_or(("", ""));
        let mut q=tx.prepare("SELECT kind,CASE WHEN octet_length(native_id)<=1048576 THEN native_id END,octet_length(kind)+octet_length(native_id) FROM native_source_links WHERE file_id=?1 AND (kind,native_id)>(?2,?3) ORDER BY kind,native_id LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q.query(params![file, kind, id]).map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.map(|(a, b)| (a.into(), b.into()));
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(2).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native lineage fact exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let pair: (String, String) = (
                row.get(0).map_err(io::Error::other)?,
                row.get(1).map_err(io::Error::other)?,
            );
            end = Some(pair.clone());
            output.push(pair);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let (kind, id) = end
            .as_ref()
            .map_or(("", ""), |(a, b)| (a.as_str(), b.as_str()));
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_source_links WHERE file_id=?1 AND (kind,native_id)>(?2,?3))",params![file,kind,id],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn codex_usage_page(
        &self,
        witness: &SourceWitness,
        after: Option<usize>,
    ) -> io::Result<Page<(usize, CodexUsageEvent), usize>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let mut q=tx.prepare("SELECT seq,CASE WHEN octet_length(model)<=1048576 THEN model END,input,output,cache_write,cache_read,t,64+octet_length(model) FROM codex_usage WHERE file_id=?1 AND seq>?2 ORDER BY seq LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, after.map(seq_int).unwrap_or(-1)])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(7).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native usage event exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let seq = seq_of(row.get(0).map_err(io::Error::other)?);
            if seq != end.map_or(0, |seq| seq + 1) {
                return Err(io::Error::other(
                    "native usage event sequence is incomplete",
                ));
            }
            output.push((
                seq,
                CodexUsageEvent {
                    model: row.get(1).map_err(io::Error::other)?,
                    tokens: ModelTokens {
                        input: uint(row.get(2).map_err(io::Error::other)?),
                        output: uint(row.get(3).map_err(io::Error::other)?),
                        cache_write: uint(row.get(4).map_err(io::Error::other)?),
                        cache_read: uint(row.get(5).map_err(io::Error::other)?),
                    },
                    timestamp: row.get(6).map_err(io::Error::other)?,
                },
            ));
            end = Some(seq);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let remains: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM codex_usage WHERE file_id=?1 AND seq>?2)",
                params![file, end.map(seq_int).unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
}
impl NativeStore {
    /// Foreground metadata reads share the native private contract without
    /// creating a store, migrating it or admitting producer work.
    pub(crate) fn open_read(options: &crate::Options) -> io::Result<Self> {
        let connection = Connection::open_with_flags(
            super::super::EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(io::Error::other)?;
        connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(io::Error::other)?;
        if !private_current(&connection).map_err(io::Error::other)? {
            return Err(io::Error::other(
                "native private producer schema requires repair",
            ));
        }
        Ok(Self {
            connection,
            options: options.clone(),
        })
    }

    /// Scoped initialization never calls read_file or restores a selected Index.
    pub(crate) fn open(options: &crate::Options) -> io::Result<Self> {
        let (store, _) = SqliteStore::attempt_scoped(
            &super::super::EventCache::path(&options.cache),
            None,
            true,
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Self {
            connection: store.connection,
            options: options.clone(),
        })
    }
    pub(crate) fn source_witness(&self, source: &CatalogSource) -> io::Result<SourceWitness> {
        self.admit_configured_source(source)?;
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !private_current(&tx).map_err(io::Error::other)? {
            return Err(io::Error::other("native store version changed"));
        }
        capture_source(&tx, source).map_err(io::Error::other)
    }
    pub(crate) fn source_witness_current(&self, witness: &SourceWitness) -> io::Result<bool> {
        check_source(&self.connection, witness).map_err(io::Error::other)
    }
    /// Captures an immutable original source identity and the separate Current
    /// ledger/proof observations. Current row readers still require that the
    /// observed ledger match this original identity.
    pub(crate) fn original_source_witness(
        &self,
        source: &CatalogSource,
    ) -> io::Result<SourceWitness> {
        self.admit_configured_source(source)?;
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !private_current(&tx).map_err(io::Error::other)? {
            return Err(io::Error::other("native store version changed"));
        }
        capture_source_observation(&tx, source).map_err(io::Error::other)
    }
    fn admit_configured_source(&self, source: &CatalogSource) -> io::Result<()> {
        if crate::native_evidence::catalog_input(&source.path, &self.options).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "native source is outside configured roots",
            ));
        }
        Ok(())
    }
    pub(crate) fn owner_witness(&self, row: &CatalogRow) -> io::Result<OwnerWitness> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !private_current(&tx).map_err(io::Error::other)? {
            return Err(io::Error::other("native store version changed"));
        }
        let identity = crate::query_projection::identity(row);
        // Native Catalog rows are canonically encoded by their publisher. One
        // supplied row is admitted once; subsequent fences use scalar epochs.
        let encoded = bounded_json(row, BYTES).map_err(io::Error::other)?;
        let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1 AND metadata=?2) OR EXISTS(SELECT 1 FROM session_history_catalog WHERE session_key=?1 AND metadata=?2)",params![row.key,encoded],|r|r.get(0)).map_err(io::Error::other)?;
        if !matches {
            return Err(io::Error::other("native owner witness changed"));
        }
        Ok(OwnerWitness {
            identity: identity.clone(),
            key: row.key.clone(),
            catalog: owner_version(&tx, "current", &row.key).map_err(io::Error::other)?,
            history: owner_version(&tx, "history", &row.key).map_err(io::Error::other)?,
            private_header: bounded_value(
                &tx,
                &crate::query_projection::prefix("header", &identity),
                STATE_BYTES,
            )
            .map_err(io::Error::other)?,
            trace_header: header(&tx, "session_trace_headers", &row.key)
                .map_err(io::Error::other)?,
            slot_header: header(&tx, "session_slot_projections", &row.key)
                .map_err(io::Error::other)?,
            history_header: header(&tx, "session_history_projections", &row.key)
                .map_err(io::Error::other)?,
        })
    }
    pub(crate) fn source_header(
        &self,
        witness: &SourceWitness,
    ) -> io::Result<Option<SourceHeader>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let Some(file) = current_file_if_present(&tx, witness).map_err(io::Error::other)? else {
            return Ok(None);
        };
        let Some(ledger) = &witness.ledger else {
            return Err(io::Error::other("native Current source is absent"));
        };
        let length = "COALESCE(octet_length(entrypoint),0)+COALESCE(octet_length(title),0)+COALESCE(octet_length(agent_name),0)+COALESCE(octet_length(last_model),0)+COALESCE(octet_length(cwd),0)+COALESCE(octet_length(branch),0)";
        let columns = guard_columns(
            "entrypoint,title,agent_name,last_model,failed,first_ms,last_ms,cwd,branch",
            length,
        );
        let mut q = tx.prepare(&format!("SELECT {columns},{length},(SELECT generation FROM native_source_facts WHERE file_id=files.file_id),(SELECT events FROM native_source_facts WHERE file_id=files.file_id),(SELECT extras FROM native_source_facts WHERE file_id=files.file_id),(SELECT signals FROM native_source_facts WHERE file_id=files.file_id),(SELECT generation FROM native_source_order_facts WHERE file_id=files.file_id),(SELECT first_seq FROM native_source_order_facts WHERE file_id=files.file_id),(SELECT first_ms FROM native_source_order_facts WHERE file_id=files.file_id) FROM files WHERE file_id=?1")).map_err(io::Error::other)?;
        let mut rows = q.query([file]).map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        if row.get::<_, i64>(9).map_err(io::Error::other)? > BYTES as i64 {
            return Err(io::Error::other(
                "native scalar header exceeds metadata budget",
            ));
        }
        let ready: Option<String> = row.get(10).map_err(io::Error::other)?;
        let primitives_ready =
            ready.as_deref() == Some(&source_fact_generation(ledger).map_err(io::Error::other)?);
        let counts = if primitives_ready {
            Some((
                seq_of(row.get(11).map_err(io::Error::other)?),
                seq_of(row.get(12).map_err(io::Error::other)?),
                seq_of(row.get(13).map_err(io::Error::other)?),
            ))
        } else {
            None
        };
        let order_generation: Option<String> = row.get(14).map_err(io::Error::other)?;
        let time_seed = if order_generation.as_deref()
            == Some(&source_order_generation(ledger).map_err(io::Error::other)?)
        {
            Some(TimeSeed {
                first_seq: row
                    .get::<_, Option<i64>>(15)
                    .map_err(io::Error::other)?
                    .map(seq_of),
                first_ms: row.get(16).map_err(io::Error::other)?,
            })
        } else {
            None
        };
        Ok(Some(SourceHeader {
            witness: witness.clone(),
            entrypoint: row.get(0).map_err(io::Error::other)?,
            title: row.get(1).map_err(io::Error::other)?,
            agent_name: row.get(2).map_err(io::Error::other)?,
            last_model: row.get(3).map_err(io::Error::other)?,
            failed: row.get(4).map_err(io::Error::other)?,
            first: row.get(5).map_err(io::Error::other)?,
            last: row.get(6).map_err(io::Error::other)?,
            cwd: row.get(7).map_err(io::Error::other)?,
            branch: row.get(8).map_err(io::Error::other)?,
            primitives_ready,
            counts,
            time_seed,
        }))
    }

    /// Scalar native ordering evidence. The native lineage comparator needs
    /// continuation emptiness, never the complete growing link set.
    pub(crate) fn source_continued_empty(&self, witness: &SourceWitness) -> io::Result<bool> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = current_file(&tx, witness).map_err(io::Error::other)?;
        let ledger = witness
            .ledger
            .as_ref()
            .ok_or_else(|| io::Error::other("native source is absent"))?;
        let generation = source_fact_generation(ledger).map_err(io::Error::other)?;
        let current:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_source_facts WHERE file_id=?1 AND generation=?2)",params![file,generation],|row|row.get(0)).map_err(io::Error::other)?;
        if !current {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "native lineage facts need bounded repair",
            ));
        }
        tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM native_source_links WHERE file_id=?1 AND kind='continued')",[file],|row|row.get(0)).map_err(io::Error::other)
    }

    /// Exact native record identity, including UUID precedence, is normalized
    /// by the existing writer. No per-candidate source-event scan occurs here.
    pub(crate) fn source_has_copy_identity(
        &self,
        witness: &SourceWitness,
        identity: &str,
    ) -> io::Result<(bool, Work)> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let generation: Option<String> = tx
            .query_row(
                "SELECT generation FROM native_source_order_facts WHERE file_id=?1",
                [file],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        let expected = witness
            .ledger
            .as_ref()
            .map(source_order_generation)
            .transpose()
            .map_err(io::Error::other)?;
        if generation.is_none() || generation != expected {
            return Err(io::Error::other(
                "native copy primitives require bounded repair",
            ));
        }
        let mut q = tx.prepare("SELECT EXISTS(SELECT 1 FROM native_source_copy_events WHERE file_id=?1 AND identity=?2)").map_err(io::Error::other)?;
        let present = q
            .query_row(params![file, identity], |r| r.get(0))
            .map_err(io::Error::other)?;
        let work = Work {
            decoded_rows: 0,
            metadata_bytes: 0,
            vm_steps: q.get_status(rusqlite::StatementStatus::VmStep),
        };
        Ok((present, work))
    }

    /// Returns the latest native regular timestamp at or before `regular_seq`.
    /// An untimed prefix stays symbolic even when a later row has a timestamp.
    pub(crate) fn source_time_at(
        &self,
        witness: &SourceWitness,
        regular_seq: usize,
    ) -> io::Result<(SourceTime, Work)> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = current_file(&tx, witness).map_err(io::Error::other)?;
        let ledger = witness
            .ledger
            .as_ref()
            .ok_or_else(|| io::Error::other("native Current source is absent"))?;
        let mut work = order_ready(&tx, file, ledger)?;
        let (time, read) = source_time_at_index(&tx, file, regular_seq)?;
        add_work(&mut work, read)?;
        Ok((time, work))
    }

    /// Resolves the regular event at the native offset predecessor, with seq0
    /// as the parser's partition-point fallback, then applies the shared time
    /// rule without decoding an Event or walking prior history.
    pub(crate) fn source_time_before_offset(
        &self,
        witness: &SourceWitness,
        offset: u64,
    ) -> io::Result<(SourceTime, Work)> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = current_file(&tx, witness).map_err(io::Error::other)?;
        let ledger = witness
            .ledger
            .as_ref()
            .ok_or_else(|| io::Error::other("native Current source is absent"))?;
        let mut work = order_ready(&tx, file, ledger)?;
        let (time, read) = source_time_before_offset_index(&tx, file, offset)?;
        add_work(&mut work, read)?;
        Ok((time, work))
    }

    /// Returns the last indexed Operation or Tool child for one regular parent.
    /// Equal placements preserve extras before regular events and the first
    /// sequence within each channel, matching the complete model iterator.
    pub(crate) fn last_operation(
        &self,
        witness: &SourceWitness,
        parent_seq: usize,
    ) -> io::Result<(Option<(EventAddress, SourceTime, (u64, u32))>, Work)> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = current_file(&tx, witness).map_err(io::Error::other)?;
        let ledger = witness
            .ledger
            .as_ref()
            .ok_or_else(|| io::Error::other("native Current source is absent"))?;
        let parent = sequence_sql(parent_seq).map_err(io::Error::other)?;
        let mut work = child_operations_ready(&tx, file, ledger)?;
        let mut query = tx
            .prepare("SELECT extra,seq,offset_key,block,time FROM native_source_child_operations WHERE file_id=?1 AND parent=?2 ORDER BY offset_key DESC,block DESC,extra DESC,seq ASC LIMIT 1")
            .map_err(io::Error::other)?;
        let found: Option<(bool, i64, Vec<u8>, i64, Option<i64>)> = query
            .query_row(params![file, parent], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .optional()
            .map_err(io::Error::other)?;
        add_work(
            &mut work,
            Work {
                decoded_rows: if found.is_some() { 1 } else { 0 },
                metadata_bytes: found.as_ref().map_or(0, |(_, _, _, _, time)| {
                    32 + if time.is_some() {
                        std::mem::size_of::<i64>()
                    } else {
                        0
                    }
                }),
                vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
            },
        )?;
        let Some((extra, seq, offset_key, block, explicit_time)) = found else {
            return Ok((None, work));
        };
        let seq = usize::try_from(seq)
            .map_err(|_| io::Error::other("native operation seq is invalid"))?;
        let offset_key: [u8; 8] = offset_key
            .try_into()
            .map_err(|_| io::Error::other("native operation offset is invalid"))?;
        let offset = u64::from_be_bytes(offset_key);
        let block = u32::try_from(block)
            .map_err(|_| io::Error::other("native operation block is invalid"))?;
        let time = if let Some(time) = explicit_time {
            SourceTime::Observed(time)
        } else {
            add_work(&mut work, order_ready(&tx, file, ledger)?)?;
            let (time, read) = source_time_before_offset_index(&tx, file, offset)?;
            add_work(&mut work, read)?;
            time
        };
        Ok((
            Some((EventAddress { extra, seq }, time, (offset, block))),
            work,
        ))
    }

    pub(crate) fn events_page(
        &self,
        witness: &SourceWitness,
        extra: bool,
        after: Option<usize>,
    ) -> io::Result<Page<(usize, Event), usize>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = current_file(&tx, witness).map_err(io::Error::other)?;
        let length = "192+COALESCE(octet_length(k),0)+COALESCE(octet_length(id),0)+COALESCE(octet_length(n),0)+COALESCE(octet_length(y),0)+COALESCE(octet_length(reply_m),0)+COALESCE(octet_length(peer_sock),0)+COALESCE(octet_length(peer_start),0)+COALESCE(octet_length(u),0)";
        let columns = guard_columns(EVENT_COLUMNS, length);
        let mut q = tx.prepare(&format!("SELECT {columns},{length} FROM events WHERE file_id=?1 AND extra=?2 AND seq>?3 ORDER BY seq LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, extra, after.map(seq_int).unwrap_or(-1)])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut work = Work::default();
        let mut end = after;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let length = usize::try_from(row.get::<_, i64>(27).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if length > BYTES {
                return Err(io::Error::other("native event exceeds metadata budget"));
            }
            if work.metadata_bytes + length > BYTES {
                break;
            }
            let (_, seq, event) = read_event(row).map_err(io::Error::other)?;
            if seq != end.map_or(0, |seq| seq + 1) {
                return Err(io::Error::other("native event sequence is incomplete"));
            }
            if let Some(yielded) = &event.y
                && yielded.polls.len() > super::super::POLLS_MAX
            {
                return Err(io::Error::other(
                    "native yielded poll evidence exceeds parser contract",
                ));
            }
            work.metadata_bytes += length;
            work.decoded_rows += 1;
            end = Some(seq);
            output.push((seq, event));
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let remaining: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE file_id=?1 AND extra=?2 AND seq>?3)",
                params![file, extra, end.map(seq_int).unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remaining,
            work,
        })
    }
    /// Native extras inherit the last regular event at or before their byte
    /// offset, with seq0 as the native partition_point fallback. Both probes
    /// use native indexes and never walk the preceding event history.
    pub(crate) fn event_before_offset(
        &self,
        witness: &SourceWitness,
        offset: u64,
    ) -> io::Result<Option<(usize, Event)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = current_file(&tx, witness).map_err(io::Error::other)?;
        let ledger = witness
            .ledger
            .as_ref()
            .ok_or_else(|| io::Error::other("native Current source is absent"))?;
        let _ready = order_ready(&tx, file, ledger)?;
        let (seq, _) = sequence_before_offset_index(&tx, file, offset)?;
        let seq = sequence_sql(seq.unwrap_or(0)).map_err(io::Error::other)?;
        let length = "192+COALESCE(octet_length(k),0)+COALESCE(octet_length(id),0)+COALESCE(octet_length(n),0)+COALESCE(octet_length(y),0)+COALESCE(octet_length(reply_m),0)+COALESCE(octet_length(peer_sock),0)+COALESCE(octet_length(peer_start),0)+COALESCE(octet_length(u),0)";
        let columns = guard_columns(EVENT_COLUMNS, length);
        let mut query = tx
            .prepare(&format!(
                "SELECT {columns},{length} FROM events WHERE file_id=?1 AND extra=0 AND seq=?2"
            ))
            .map_err(io::Error::other)?;
        let mut rows = query.query(params![file, seq]).map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        if row.get::<_, i64>(27).map_err(io::Error::other)? > BYTES as i64 {
            return Err(io::Error::other(
                "native point event exceeds metadata budget",
            ));
        }
        let (_, seq, event) = read_event(row).map_err(io::Error::other)?;
        Ok(Some((seq, event)))
    }
    pub(crate) fn event_at(
        &self,
        witness: &SourceWitness,
        extra: bool,
        seq: usize,
    ) -> io::Result<Option<Event>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let Some(file) = current_file_if_present(&tx, witness).map_err(io::Error::other)? else {
            return Ok(None);
        };
        let seq = sequence_sql(seq).map_err(io::Error::other)?;
        let length = "192+COALESCE(octet_length(k),0)+COALESCE(octet_length(id),0)+COALESCE(octet_length(n),0)+COALESCE(octet_length(y),0)+COALESCE(octet_length(reply_m),0)+COALESCE(octet_length(peer_sock),0)+COALESCE(octet_length(peer_start),0)+COALESCE(octet_length(u),0)";
        let columns = guard_columns(EVENT_COLUMNS, length);
        let mut q = tx
            .prepare(&format!(
                "SELECT {columns},{length} FROM events WHERE file_id=?1 AND extra=?2 AND seq=?3"
            ))
            .map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, extra, seq])
            .map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        if row.get::<_, i64>(27).map_err(io::Error::other)? > BYTES as i64 {
            return Err(io::Error::other(
                "native point event exceeds metadata budget",
            ));
        }
        Ok(Some(read_event(row).map_err(io::Error::other)?.2))
    }
}

/// Writer-owned primitive rows. Append applies the native clustering delta by
/// stable interval start; directed links are the exact parser delta. Older stores are
/// deliberately not marked ready after only a partial append.
pub(super) fn write_source_facts(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    changes: &super::super::DirtyRows,
    complete: bool,
    expected: Option<&Ledger>,
) -> rusqlite::Result<()> {
    if !private_current(tx)? {
        return Err(invalid("native private producer schema changed"));
    }
    let file: i64 = tx.query_row("SELECT file_id FROM files WHERE path=?1", [path], |r| {
        r.get(0)
    })?;
    let was_ready: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_source_facts WHERE file_id=?1 AND generation LIKE 'interval-v2:%')",[file],|r|r.get(0),
    )?;
    if complete {
        tx.execute(
            "DELETE FROM native_source_busy_intervals WHERE file_id=?1",
            [file],
        )?;
    }
    for (start, end) in &changes.busy {
        #[cfg(test)]
        BUSY_FACT_VISITS.with(|n| n.set(n.get() + 1));
        if let Some(end) = end {
            tx.execute("INSERT INTO native_source_busy_intervals VALUES(?1,?2,?3) ON CONFLICT(file_id,start) DO UPDATE SET end=excluded.end",params![file,start,end])?;
        } else {
            tx.execute(
                "DELETE FROM native_source_busy_intervals WHERE file_id=?1 AND start=?2",
                params![file, start],
            )?;
        }
    }
    for (kind, id) in &changes.links {
        #[cfg(test)]
        DELTA_LINK_VISITS.with(|n| n.set(n.get() + 1));
        let present = match kind.as_str() {
            "session" => index.links.session_ids.contains(id),
            "continued" => index.links.continued_in.contains(id),
            "bridge" => index.links.bridges.contains(id),
            _ => return Err(invalid("unsupported native link kind")),
        };
        if present {
            tx.execute(
                "INSERT OR IGNORE INTO native_source_links VALUES(?1,?2,?3)",
                params![file, kind, id],
            )?;
        } else {
            tx.execute(
                "DELETE FROM native_source_links WHERE file_id=?1 AND kind=?2 AND native_id=?3",
                params![file, kind, id],
            )?;
        }
    }
    if complete || was_ready {
        tx.execute(
            "INSERT OR REPLACE INTO native_source_facts VALUES(?1,?2,?3,?4,?5)",
            params![
                file,
                source_fact_generation(ledger)?,
                seq_int(index.events.len()),
                seq_int(index.extras.len()),
                seq_int(index.signals.len())
            ],
        )?;
    }
    write_order_facts(tx, file, ledger, index, changes, complete, expected)?;
    write_operation_facts(tx, file, ledger, index, changes, complete, expected)?;
    Ok(())
}

/// Full initial publication normalizes retained native facts once. Normal
/// append visits only parser-owned touched rows and the appended suffix.
/// Old missing primitives stay unqualified until bounded original repair.
fn write_order_facts(
    tx: &Transaction<'_>,
    file: i64,
    ledger: &Ledger,
    index: &FileIndex,
    changes: &super::super::DirtyRows,
    complete: bool,
    expected: Option<&Ledger>,
) -> rusqlite::Result<()> {
    let old: Option<String> = tx
        .query_row(
            "SELECT generation FROM native_source_order_facts WHERE file_id=?1",
            [file],
            |r| r.get(0),
        )
        .optional()?;
    let qualified =
        complete || old.is_some() && old == expected.map(source_order_generation).transpose()?;
    if complete {
        tx.execute(
            "DELETE FROM native_source_event_times WHERE file_id=?1",
            [file],
        )?;
        tx.execute(
            "DELETE FROM native_source_copy_events WHERE file_id=?1",
            [file],
        )?;
        tx.execute(
            "DELETE FROM native_source_event_positions WHERE file_id=?1",
            [file],
        )?;
    }
    changed(
        changes.before_events,
        &changes.events,
        &index.events,
        |seq, event| {
            #[cfg(test)]
            ORDER_FACT_VISITS.with(|n| n.set(n.get() + 1));
            tx.execute("INSERT INTO native_source_event_positions VALUES(?1,?2,?3) ON CONFLICT(file_id,seq) DO UPDATE SET o=excluded.o",params![file,seq_int(seq),int(event.o)])?;
            if let Some(time) = event.t {
                tx.execute("INSERT INTO native_source_event_times VALUES(?1,?2,?3) ON CONFLICT(file_id,seq) DO UPDATE SET t=excluded.t",params![file,seq_int(seq),time])?;
            } else {
                tx.execute(
                    "DELETE FROM native_source_event_times WHERE file_id=?1 AND seq=?2",
                    params![file, seq_int(seq)],
                )?;
            }
            if let Some(identity) = crate::model::native_copy_identity(event) {
                tx.execute("INSERT INTO native_source_copy_events VALUES(?1,?2,?3) ON CONFLICT(file_id,seq) DO UPDATE SET identity=excluded.identity",params![file,seq_int(seq),identity])?;
            } else {
                tx.execute(
                    "DELETE FROM native_source_copy_events WHERE file_id=?1 AND seq=?2",
                    params![file, seq_int(seq)],
                )?;
            }
            Ok(())
        },
        |cut| {
            tx.execute(
                "DELETE FROM native_source_event_times WHERE file_id=?1 AND seq>=?2",
                params![file, seq_int(cut)],
            )?;
            tx.execute(
                "DELETE FROM native_source_copy_events WHERE file_id=?1 AND seq>=?2",
                params![file, seq_int(cut)],
            )?;
            tx.execute(
                "DELETE FROM native_source_event_positions WHERE file_id=?1 AND seq>=?2",
                params![file, seq_int(cut)],
            )?;
            Ok(())
        },
    )?;
    if qualified {
        let first: Option<(i64, i64)> = tx
            .query_row(
                "SELECT seq,t FROM native_source_event_times WHERE file_id=?1 ORDER BY seq LIMIT 1",
                [file],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        tx.execute("INSERT INTO native_source_order_facts VALUES(?1,?2,?3,?4) ON CONFLICT(file_id) DO UPDATE SET generation=excluded.generation,first_seq=excluded.first_seq,first_ms=excluded.first_ms",params![file,source_order_generation(ledger)?,first.map(|p|p.0),first.map(|p|p.1)])?;
    }
    Ok(())
}

/// Child rows have their own generation footer. A migrated store with no
/// normalized rows stays pending until a complete publication repairs it;
/// qualified appends and cuts then touch only DirtyRows' two-channel delta.
fn write_operation_facts(
    tx: &Transaction<'_>,
    file: i64,
    ledger: &Ledger,
    index: &FileIndex,
    changes: &super::super::DirtyRows,
    complete: bool,
    expected: Option<&Ledger>,
) -> rusqlite::Result<()> {
    let old: Option<String> = tx
        .query_row(
            "SELECT generation FROM native_source_child_operation_facts WHERE file_id=?1",
            [file],
            |row| row.get(0),
        )
        .optional()?;
    let qualified = complete
        || old.is_some() && old == expected.map(source_operation_generation).transpose()?;
    if complete {
        tx.execute(
            "DELETE FROM native_source_child_operations WHERE file_id=?1",
            [file],
        )?;
    }
    for (extra, before, touched, events) in [
        (true, changes.before_extras, &changes.extras, &index.extras),
        (false, changes.before_events, &changes.events, &index.events),
    ] {
        changed(
            before,
            touched,
            events,
            |seq, event| {
                #[cfg(test)]
                OPERATION_FACT_VISITS.with(|count| count.set(count.get() + 1));
                let seq = sequence_sql(seq)?;
                tx.execute(
                    "DELETE FROM native_source_child_operations WHERE file_id=?1 AND extra=?2 AND seq=?3",
                    params![file, extra, seq],
                )?;
                if matches!(
                    event.k,
                    crate::events::Kind::Operation | crate::events::Kind::Tool
                ) && let Some(parent) = event.parent
                {
                    let parent = sequence_sql(parent)?;
                    tx.execute(
                        "INSERT INTO native_source_child_operations(file_id,parent,extra,seq,offset_key,block,time) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![file,parent,extra,seq,event.o.to_be_bytes().as_slice(),i64::from(event.b),event.t],
                    )?;
                }
                Ok(())
            },
            |cut| {
                let _removed = tx.execute(
                    "DELETE FROM native_source_child_operations WHERE file_id=?1 AND extra=?2 AND seq>=?3",
                    params![file, extra, sequence_sql(cut)?],
                )?;
                #[cfg(test)]
                OPERATION_FACT_CUT_ROWS.with(|count| count.set(count.get() + _removed));
                Ok(())
            },
        )?;
    }
    if qualified {
        tx.execute(
            "INSERT INTO native_source_child_operation_facts VALUES(?1,?2) ON CONFLICT(file_id) DO UPDATE SET generation=excluded.generation",
            params![file, source_operation_generation(ledger)?],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Kind, SignalKind};
    use std::cell::Cell;

    struct Fixture {
        root: PathBuf,
        options: crate::Options,
        index: FileIndex,
        row: CatalogRow,
        store: NativeStore,
    }
    impl Fixture {
        fn new(earlier: usize) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-native-query-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let options = crate::Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                cache: root.join("cache"),
                ..crate::Options::default()
            };
            let path = options.claude_home.join("projects/p/chosen.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = b"fixture\n".repeat(earlier + 1);
            fs::write(&path, &bytes).unwrap();
            let ledger = Ledger {
                stat: Stat::of(&path).unwrap(),
                offset: bytes.len() as u64,
                prefix: Sha256::digest(&bytes).into(),
                tail: Sha256::digest(&bytes[bytes.len().saturating_sub(4096)..]).into(),
            };
            let mut index = FileIndex {
                events: (0..=earlier)
                    .map(|seq| Event {
                        k: Kind::A,
                        o: (seq * 8) as u64,
                        t: Some(seq as i64),
                        id: Some(format!("native-{seq}")),
                        ..Event::default()
                    })
                    .collect(),
                ..FileIndex::default()
            };
            index.extras.push(Event {
                k: Kind::Think,
                o: 0,
                ..Event::default()
            });
            index.signals.push(Signal {
                k: SignalKind::Effort,
                o: 0,
                t: Some(0),
                at: 0,
                n: Some("high".into()),
                v: None,
            });
            index.busy = vec![(0, 1), (700_000, 700_001)];
            index.links.continued_in.insert("next".into());
            let source = ledger.source(&path, "chosen");
            let row:CatalogRow=serde_json::from_value(serde_json::json!({"key":"chosen","name":"chosen","harness":"claude","kind":"session","native_ids":["chosen"],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":"test","effort":null,"start":0,"last":earlier,"tokens":[0.0,0.0,0.0],"cost":crate::pricing::Cost::default(),"sources":[source]})).unwrap();
            let (mut writer, _) = SqliteStore::attempt_scoped(
                &super::super::super::EventCache::path(&options.cache),
                None,
                true,
            )
            .unwrap_or_else(|error| panic!("native fixture store: {error}"));
            assert_eq!(
                writer
                    .write_one(path.to_str().unwrap(), None, None, &ledger, &index)
                    .unwrap(),
                Outcome::Written
            );
            writer.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",params![row.key,row.last.unwrap(),row.harness,json(&row).unwrap()]).unwrap();
            writer
                .connection
                .execute(
                    "INSERT INTO meta(key,value) VALUES('catalog_generation','catalog-one')",
                    [],
                )
                .unwrap();
            let store = NativeStore {
                connection: writer.connection,
                options: options.clone(),
            };
            Self {
                root,
                options,
                index,
                row,
                store,
            }
        }
        fn new_codex(records: &[Value]) -> (Self, crate::inputs::Input) {
            let mut fixture = Self::new(0);
            fixture.options.all = true;
            fixture.options.proc_root = fixture.root.join("proc");
            let path = fixture
                .options
                .codex_home
                .join("sessions/2026/10/15/rollout-code-mode-fixture.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut bytes = Vec::new();
            for record in records {
                bytes.extend(serde_json::to_vec(record).unwrap());
                bytes.push(b'\n');
            }
            let ledger = write_generation(&path, &bytes);
            let mut parsed_cache = crate::events::EventCache::default();
            let mut dirty = false;
            let index = crate::events::scan_file(&path, "codex", &mut parsed_cache, &mut dirty)
                .unwrap()
                .as_ref()
                .clone();
            assert!(dirty);
            let source = ledger.source(&path, "codex-code-mode-fixture");
            let row: CatalogRow = serde_json::from_value(serde_json::json!({
                "key":"codex-code-mode-fixture",
                "name":"Codex code mode fixture",
                "harness":"codex",
                "kind":"session",
                "native_ids":["codex-code-mode-fixture"],
                "parent":null,
                "parent_source":null,
                "repo":null,
                "branch":null,
                "model":"test",
                "effort":null,
                "start":0,
                "last":0,
                "tokens":[0.0,0.0,0.0],
                "cost":crate::pricing::Cost::default(),
                "sources":[source]
            }))
            .unwrap();
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path.to_str().unwrap(), &ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            fixture
                .store
                .connection
                .execute(
                    "INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",
                    params![row.key, row.last.unwrap(), row.harness, json(&row).unwrap()],
                )
                .unwrap();
            let relative = path
                .strip_prefix(&fixture.options.codex_home)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            fixture.index = index;
            fixture.row = row;
            (
                fixture,
                crate::inputs::Input {
                    root: crate::inputs::InputRoot::Codex,
                    path: relative,
                },
            )
        }
        fn source(&self) -> SourceWitness {
            self.store.source_witness(&self.row.sources[0]).unwrap()
        }
        fn spec(&self) -> JobSpec {
            JobSpec {
                view_key: "chosen:physical-membership".into(),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&self.options).unwrap(),
            }
        }
        fn admitted(&mut self) -> Job {
            let spec = self.spec();
            let job = self.store.begin(&spec).unwrap();
            let source = self.source();
            let owner = self.store.owner_witness(&self.row).unwrap();
            assert_eq!(
                self.store
                    .admit(&job, &[source], &[owner], "closure-complete")
                    .unwrap(),
                Outcome::Written
            );
            self.store.reload(&spec.view_key).unwrap().unwrap()
        }
        fn write_full_index(&self, index: &FileIndex) {
            let witness = self.source();
            let ledger = witness.ledger.unwrap();
            let tx = self.store.connection.unchecked_transaction().unwrap();
            write_index_changes(
                &tx,
                witness.source.path.to_str().unwrap(),
                &ledger,
                index,
                None,
                None,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        fn write_delta_index(&self, previous: &FileIndex, index: &FileIndex) {
            let witness = self.source();
            let ledger = witness.ledger.unwrap();
            let changes = super::super::super::DirtyRows::between(previous, index);
            let tx = self.store.connection.unchecked_transaction().unwrap();
            write_index_changes(
                &tx,
                witness.source.path.to_str().unwrap(),
                &ledger,
                index,
                Some(&changes),
                Some(&ledger),
            )
            .unwrap();
            tx.commit().unwrap();
        }
    }
    fn write_generation(path: &Path, bytes: &[u8]) -> Ledger {
        fs::write(path, bytes).unwrap();
        Ledger {
            stat: Stat::of(path).unwrap(),
            offset: u64::try_from(bytes.len()).unwrap(),
            prefix: Sha256::digest(bytes).into(),
            tail: Sha256::digest(&bytes[bytes.len().saturating_sub(4096)..]).into(),
        }
    }
    fn install_current_ledger(connection: &Connection, path: &Path, ledger: &Ledger) {
        connection
            .execute(
                "UPDATE files SET dev=?1,ino=?2,size=?3,mtime_ns=?4,ctime_ns=?5,resume_at=?6,head_sha256=?7,tail_sha256=?8 WHERE path=?9",
                params![
                    int(ledger.stat.dev),
                    int(ledger.stat.ino),
                    int(ledger.stat.size),
                    ledger.stat.modified_ns.to_be_bytes(),
                    ledger.stat.changed_ns.map(i128::to_be_bytes),
                    int(ledger.offset),
                    ledger.prefix,
                    ledger.tail,
                    path.to_str().unwrap()
                ],
            )
            .unwrap();
    }
    fn complete_time_at(events: &[Event], seq: usize) -> SourceTime {
        events
            .iter()
            .take(seq.saturating_add(1))
            .rev()
            .find_map(|event| event.t.map(SourceTime::Observed))
            .unwrap_or(SourceTime::BeforeFirst)
    }
    fn complete_time_before_offset(events: &[Event], offset: u64) -> SourceTime {
        let seq = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.o <= offset)
            .max_by_key(|(seq, event)| (event.o, *seq))
            .map(|(seq, _)| seq)
            .unwrap_or(0);
        complete_time_at(events, seq)
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn stage_row(rank: i64, bytes: usize) -> StageRow {
        StageRow {
            kind: RowKind::Slot,
            key: RowKey {
                order: [rank, 0, 0, 0, 0],
                ordinal: 0,
                identity: format!("slot:{rank}"),
            },
            facts: serde_json::json!({"native":"x".repeat(bytes)}),
        }
    }

    #[test]
    fn native_query_pages_preserve_exact_facts_with_zero_full_index_loads_and_fixed_tail_work() {
        let mut steps = Vec::new();
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(earlier);
            let source = fixture.source();
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let page = fixture
                .store
                .events_page(&source, false, earlier.checked_sub(1))
                .unwrap();
            assert_eq!(
                page.rows,
                vec![(earlier, fixture.index.events[earlier].clone())]
            );
            assert!(page.complete);
            assert_eq!(page.work.decoded_rows, 1);
            assert!(page.work.metadata_bytes < 1024);
            steps.push(page.work.vm_steps);
            assert_eq!(
                fixture.store.event_at(&source, false, earlier).unwrap(),
                Some(fixture.index.events[earlier].clone())
            );
            assert_eq!(
                fixture
                    .store
                    .event_before_offset(&source, (earlier * 8) as u64)
                    .unwrap(),
                Some((earlier, fixture.index.events[earlier].clone()))
            );
            assert_eq!(
                fixture.store.event_before_offset(&source, 0).unwrap(),
                Some((0, fixture.index.events[0].clone()))
            );
            assert_eq!(
                fixture.store.events_page(&source, true, None).unwrap().rows,
                vec![(0, fixture.index.extras[0].clone())]
            );
            assert_eq!(
                fixture.store.signals_page(&source, None).unwrap().rows,
                vec![(0, fixture.index.signals[0].clone())]
            );
            assert_eq!(
                fixture.store.busy_page(&source, None).unwrap().rows,
                vec![(0, 1), (700_000, 700_001)]
            );
            assert_eq!(
                fixture.store.links_page(&source, None).unwrap().rows,
                vec![("continued".into(), "next".into())]
            );
            assert!(
                fixture
                    .store
                    .source_header(&source)
                    .unwrap()
                    .unwrap()
                    .primitives_ready
            );
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
        }
        assert!(
            steps.iter().max().unwrap() - steps.iter().min().unwrap() <= 8,
            "tail SQL work grew: {steps:?}"
        );
    }

    #[test]
    fn native_query_source_time_primitives_match_indexed_predecessor_contract() {
        let fixture = Fixture::new(3);
        let mut index = fixture.index.clone();
        for (seq, (offset, time)) in [(4, None), (8, Some(100)), (8, None), (16, Some(-20))]
            .into_iter()
            .enumerate()
        {
            index.events[seq].o = offset;
            index.events[seq].t = time;
        }
        let high_offset = (1u64 << 63) + 5;
        index.events.push(Event {
            o: high_offset,
            t: Some(-30),
            ..Event::default()
        });
        fixture.write_full_index(&index);
        let witness = fixture.source();
        let original_witness = fixture
            .store
            .original_source_witness(&fixture.row.sources[0])
            .unwrap();
        assert!(
            fixture
                .store
                .source_header(&original_witness)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            fixture.store.event_at(&original_witness, false, 0).unwrap(),
            Some(index.events[0].clone())
        );
        assert_eq!(
            fixture
                .store
                .source_time_at(&original_witness, 0)
                .unwrap()
                .0,
            SourceTime::BeforeFirst
        );

        for seq in 0..=index.events.len() {
            let (actual, work) = fixture.store.source_time_at(&witness, seq).unwrap();
            assert_eq!(actual, complete_time_at(&index.events, seq));
            assert!(work.decoded_rows <= 2);
            assert!(
                work.metadata_bytes
                    <= source_order_generation(witness.ledger.as_ref().unwrap())
                        .unwrap()
                        .len()
                        + std::mem::size_of::<i64>()
            );
        }
        assert_eq!(
            fixture.store.source_time_at(&witness, 0).unwrap().0,
            SourceTime::BeforeFirst,
            "a later first timestamp must not seed an earlier sequence"
        );

        for offset in [0, 4, 8, 15, 16, (1u64 << 63) + 4, high_offset, u64::MAX] {
            let (actual, work) = fixture
                .store
                .source_time_before_offset(&witness, offset)
                .unwrap();
            assert_eq!(actual, complete_time_before_offset(&index.events, offset));
            assert!(work.decoded_rows <= 3);
            assert!(
                work.metadata_bytes
                    <= source_order_generation(witness.ledger.as_ref().unwrap())
                        .unwrap()
                        .len()
                        + 2 * std::mem::size_of::<i64>()
            );
        }
        assert_eq!(
            fixture
                .store
                .event_before_offset(&witness, high_offset)
                .unwrap(),
            Some((index.events.len() - 1, index.events.last().unwrap().clone()))
        );
        assert_eq!(
            fixture
                .store
                .source_time_before_offset(&witness, 8)
                .unwrap()
                .0,
            SourceTime::Observed(100),
            "equal regular positions select the highest native sequence"
        );
        if usize::BITS > 63 {
            assert!(fixture.store.source_time_at(&witness, usize::MAX).is_err());
            assert!(fixture.store.event_at(&witness, false, usize::MAX).is_err());
            assert!(fixture.store.last_operation(&witness, usize::MAX).is_err());
        }

        fixture
            .store
            .connection
            .execute(
                "UPDATE native_source_order_facts SET generation='order-v1:stale'",
                [],
            )
            .unwrap();
        assert!(fixture.store.source_time_at(&witness, 0).is_err());
        assert!(
            fixture
                .store
                .source_time_before_offset(&witness, 0)
                .is_err()
        );
    }

    #[test]
    fn native_query_source_time_lookup_work_stays_fixed_across_history_growth() {
        let mut at_steps = Vec::new();
        let mut offset_steps = Vec::new();
        for events_after_prefix in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(events_after_prefix);
            let mut index = fixture.index.clone();
            for (seq, event) in index.events.iter_mut().enumerate() {
                event.o = u64::try_from((seq + 1) * 4).unwrap();
                event.t = (seq != 0).then(|| i64::try_from(seq).unwrap());
            }
            fixture.write_full_index(&index);
            let witness = fixture.source();
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let (at, at_work) = fixture.store.source_time_at(&witness, 0).unwrap();
            let (before, offset_work) = fixture
                .store
                .source_time_before_offset(&witness, 0)
                .unwrap();
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
            assert_eq!(at, SourceTime::BeforeFirst);
            assert_eq!(before, SourceTime::BeforeFirst);
            let ready_bytes = source_order_generation(witness.ledger.as_ref().unwrap())
                .unwrap()
                .len();
            assert_eq!(
                (at_work.decoded_rows, at_work.metadata_bytes),
                (1, ready_bytes)
            );
            assert_eq!(
                (offset_work.decoded_rows, offset_work.metadata_bytes),
                (1, ready_bytes)
            );
            let time_rows: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_source_event_times",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let position_rows: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_source_event_positions",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(time_rows, i64::try_from(events_after_prefix).unwrap());
            assert_eq!(
                position_rows,
                i64::try_from(events_after_prefix + 1).unwrap()
            );
            at_steps.push(at_work.vm_steps);
            offset_steps.push(offset_work.vm_steps);
        }
        assert!(at_steps.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(offset_steps.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn native_query_original_witness_admits_archive_but_refuses_current_reads() {
        let mut fixture = Fixture::new(0);
        let original = fixture.row.sources[0].clone();
        let path = original.path.clone();
        let current = write_generation(&path, b"newer current generation\n");
        install_current_ledger(&fixture.store.connection, &path, &current);
        let archive_key = archive_index_key(path.to_str().unwrap());
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO meta(key,value) VALUES(?1,'archived-original-v1')",
                [&archive_key],
            )
            .unwrap();

        assert!(fixture.store.source_witness(&original).is_err());
        let witness = fixture.store.original_source_witness(&original).unwrap();
        assert_eq!(witness.source, original);
        assert_eq!(witness.ledger.as_ref(), Some(&current));
        assert_eq!(witness.archive.as_deref(), Some("archived-original-v1"));
        assert!(fixture.store.source_header(&witness).is_err());
        assert!(fixture.store.events_page(&witness, false, None).is_err());
        assert!(fixture.store.event_at(&witness, false, 0).is_err());
        assert!(fixture.store.event_before_offset(&witness, 0).is_err());
        assert!(
            fixture
                .store
                .source_has_copy_identity(&witness, "native-0")
                .is_err()
        );
        assert!(fixture.store.source_time_at(&witness, 0).is_err());
        assert!(
            fixture
                .store
                .source_time_before_offset(&witness, 0)
                .is_err()
        );
        assert!(fixture.store.last_operation(&witness, 0).is_err());
        assert!(fixture.store.signals_page(&witness, None).is_err());
        assert!(fixture.store.usage_page(&witness, None).is_err());
        assert!(fixture.store.identities_page(&witness, None).is_err());
        assert!(fixture.store.links_page(&witness, None).is_err());
        assert!(fixture.store.busy_page(&witness, None).is_err());

        let mut outside = original.clone();
        outside.path = fixture.root.join("outside.jsonl");
        assert_eq!(
            fixture
                .store
                .original_source_witness(&outside)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );

        let spec = fixture.spec();
        let created = fixture.store.begin(&spec).unwrap();
        let owner = fixture.store.owner_witness(&fixture.row).unwrap();
        assert_eq!(
            fixture
                .store
                .admit(&created, &[witness], &[owner], "original-complete")
                .unwrap(),
            Outcome::Written
        );
        let job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert!(fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .admitted_sources_page(&job, None)
                .unwrap()
                .rows[0]
                .source,
            original
        );

        let changed = write_generation(&path, b"changed current generation after admission\n");
        install_current_ledger(&fixture.store.connection, &path, &changed);
        assert!(!fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "slots",
                    "changed-original",
                    &serde_json::json!({}),
                    &[],
                    &|| false,
                )
                .unwrap(),
            Outcome::Conflict
        );
    }

    #[test]
    fn native_query_last_child_preserves_complete_model_channel_and_sequence_ties() {
        use crate::model::{
            ToolState,
            native_producer::{Candidate, EventInput},
            native_stage::EntryKind,
        };

        let fixture = Fixture::new(0);
        let mut index = fixture.index.clone();
        index.events[0] = Event {
            k: Kind::Tool,
            o: 0,
            t: Some(7),
            code_mode: true,
            code_mode_open: true,
            ..Event::default()
        };
        index.events.push(Event {
            k: Kind::Tool,
            parent: Some(0),
            o: 6,
            b: 1,
            t: Some(-100),
            ..Event::default()
        });
        index.extras = vec![
            Event {
                k: Kind::Operation,
                parent: Some(0),
                o: 4,
                t: Some(50),
                ..Event::default()
            },
            Event {
                k: Kind::Operation,
                parent: Some(0),
                o: 6,
                t: Some(-5),
                ..Event::default()
            },
            Event {
                k: Kind::Operation,
                parent: Some(0),
                o: 6,
                b: 1,
                ..Event::default()
            },
            Event {
                k: Kind::Tool,
                parent: Some(0),
                o: 6,
                b: 1,
                t: Some(-101),
                ..Event::default()
            },
        ];
        fixture.write_full_index(&index);
        let witness = fixture.source();
        let (actual, work) = fixture.store.last_operation(&witness, 0).unwrap();
        let expected = (
            EventAddress {
                extra: true,
                seq: 2,
            },
            SourceTime::Observed(-100),
            (6, 1),
        );
        assert_eq!(actual, Some(expected));
        assert_eq!(work.decoded_rows, 5);
        assert_eq!(
            work.metadata_bytes,
            source_operation_generation(witness.ledger.as_ref().unwrap())
                .unwrap()
                .len()
                + 40
                + source_order_generation(witness.ledger.as_ref().unwrap())
                    .unwrap()
                    .len()
                + 2 * std::mem::size_of::<i64>()
        );
        assert_eq!(fixture.store.last_operation(&witness, 9).unwrap().0, None);

        let failed = Event {
            k: Kind::Tool,
            o: 1,
            t: Some(90),
            r: Some(crate::events::Reply {
                o: 40,
                b: 2,
                t: Some(-12),
                e: true,
                ..crate::events::Reply::default()
            }),
            ..Event::default()
        };
        let candidate = Candidate::regular(
            EventInput {
                seq: 0,
                event: &failed,
                kind: EntryKind::Tool(ToolState::Err),
                inherited: SourceTime::Observed(90),
                operation: Some((expected.1, expected.2)),
                identity: [0; 32],
                ordinal: 0,
            },
            |_| SourceTime::BeforeFirst,
        );
        assert_eq!(candidate.time, SourceTime::Observed(-12));
        assert_eq!(candidate.entry.pos, (40, 2));
        assert_eq!(candidate.phase, 1);
        assert!(candidate.has_operations);

        let pending = Event { r: None, ..failed };
        let pending_candidate = Candidate::regular(
            EventInput {
                seq: 0,
                event: &pending,
                kind: EntryKind::Tool(ToolState::Pending),
                inherited: SourceTime::BeforeFirst,
                operation: Some((expected.1, expected.2)),
                identity: [0; 32],
                ordinal: 0,
            },
            |_| SourceTime::BeforeFirst,
        );
        assert_eq!(pending_candidate.time, SourceTime::Observed(-100));
        assert_eq!(pending_candidate.entry.pos, (6, 1));
        assert_eq!(pending_candidate.phase, 2);
    }

    #[test]
    fn native_query_codex_children_match_independent_complete_builder_slots() {
        let record = |kind: &str, timestamp: Option<&str>, payload: Value| {
            let mut value = serde_json::json!({"type":kind,"payload":payload});
            if let Some(timestamp) = timestamp {
                value["timestamp"] = serde_json::json!(timestamp);
            }
            value
        };
        let records = vec![
            record(
                "session_meta",
                Some("2026-10-15T00:00:00Z"),
                serde_json::json!({"id":"codex-code-mode-fixture","cwd":"/tmp/fixture","originator":"codex_exec"}),
            ),
            record(
                "response_item",
                Some("2026-10-15T00:00:10Z"),
                serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"fixture request"}]}),
            ),
            record(
                "response_item",
                Some("2026-10-15T00:00:30Z"),
                serde_json::json!({"type":"custom_tool_call","name":"exec","call_id":"pending-wrapper","input":"const result = await tools.lookup({});"}),
            ),
            record(
                "event_msg",
                None,
                serde_json::json!({"type":"item_completed","item":{"type":"CommandExecution","id":"pending-item"}}),
            ),
            record(
                "response_item",
                Some("2026-10-15T00:00:32Z"),
                serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"pending sentinel"}]}),
            ),
            record(
                "event_msg",
                Some("2026-10-15T00:00:29Z"),
                serde_json::json!({"type":"item_completed","item":{"type":"McpToolCall","id":"pending-mcp","namespace":"fixture","tool":"lookup","success":true,"result":{"content":[]}}}),
            ),
            record(
                "event_msg",
                Some("2026-10-15T00:00:33Z"),
                serde_json::json!({"type":"task_complete"}),
            ),
            record(
                "response_item",
                Some("2026-10-15T00:00:34Z"),
                serde_json::json!({"type":"custom_tool_call","name":"exec","call_id":"failed-wrapper","input":"const result = await tools.lookup({});"}),
            ),
            record(
                "event_msg",
                Some("2026-10-15T00:00:35Z"),
                serde_json::json!({"type":"item_completed","item":{"type":"McpToolCall","id":"failed-mcp","namespace":"fixture","tool":"lookup","success":true,"result":{"content":[]}}}),
            ),
            record(
                "event_msg",
                Some("2026-10-15T00:00:36Z"),
                serde_json::json!({"type":"item_completed","item":{"type":"CommandExecution","id":"failed-item","exit_code":0}}),
            ),
            record(
                "response_item",
                Some("2026-10-15T00:00:37Z"),
                serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"failed sentinel"}]}),
            ),
            record(
                "response_item",
                Some("2026-10-15T00:00:38Z"),
                serde_json::json!({"type":"custom_tool_call_output","call_id":"failed-wrapper","output":[{"text":"Script error: fixture failure"}]}),
            ),
        ];
        let (mut fixture, input) = Fixture::new_codex(&records);
        let mut index = fixture.index.clone();
        let pending_parent = index
            .events
            .iter()
            .position(|event| event.id.as_deref() == Some("pending-wrapper"))
            .unwrap();
        let pending_child = index
            .events
            .iter()
            .position(|event| event.id.as_deref() == Some("pending-mcp"))
            .unwrap();
        let pending_operation = index
            .extras
            .iter()
            .position(|event| event.k == Kind::Operation && event.parent == Some(pending_parent))
            .unwrap();
        let pending_position = (index.events[pending_child].o, index.events[pending_child].b);
        // The native parser produced each channel independently. Alias their
        // indexed placement to exercise the complete builder's exact tie rule.
        index.extras[pending_operation].o = pending_position.0;
        index.extras[pending_operation].b = pending_position.1;
        fixture.index = index.clone();
        fixture.write_full_index(&index);

        let witness = fixture.source();
        assert_eq!(
            fixture
                .store
                .last_operation(&witness, pending_parent)
                .unwrap()
                .0,
            Some((
                EventAddress {
                    extra: true,
                    seq: pending_operation,
                },
                SourceTime::Observed(crate::events::parse_ms("2026-10-15T00:00:29Z").unwrap()),
                pending_position,
            )),
            "equal placement keeps the extras Operation before the regular MCP Tool; its untimed child inherits the backward regular timestamp"
        );

        let failed_parent = index
            .events
            .iter()
            .position(|event| event.id.as_deref() == Some("failed-wrapper"))
            .unwrap();
        let failed_child = index
            .events
            .iter()
            .position(|event| event.id.as_deref() == Some("failed-mcp"))
            .unwrap();
        let failed_operation = index
            .extras
            .iter()
            .position(|event| event.k == Kind::Operation && event.parent == Some(failed_parent))
            .unwrap();
        let failed_position = (
            index.extras[failed_operation].o,
            index.extras[failed_operation].b,
        );
        assert_eq!(
            fixture
                .store
                .last_operation(&witness, failed_parent)
                .unwrap()
                .0,
            Some((
                EventAddress {
                    extra: true,
                    seq: failed_operation,
                },
                SourceTime::Observed(crate::events::parse_ms("2026-10-15T00:00:36Z").unwrap()),
                failed_position,
            ))
        );
        let failed_reply = index.events[failed_parent].r.as_ref().unwrap();
        assert_eq!(
            failed_reply.t,
            Some(crate::events::parse_ms("2026-10-15T00:00:38Z").unwrap())
        );
        assert!(failed_reply.o > index.events[failed_child].o);

        let native_ids = fixture.row.native_ids.iter().cloned().collect();
        let mut selected = crate::events::EventCache::from_selected(
            &fixture.store.connection,
            &fixture.row.sources,
            &native_ids,
        )
        .unwrap();
        let built = crate::model::build_sources(
            &fixture.options,
            &mut selected,
            &mut false,
            &mut crate::model::Texts::default(),
            crate::events::parse_ms("2026-10-15T00:01:00Z").unwrap(),
            Some(std::slice::from_ref(&input)),
        )
        .unwrap();
        let transcript = &built.tx[&fixture.row.key];
        let pending_wrapper_offset = index.events[pending_parent].o;
        let pending_child_offset = index.events[pending_child].o;
        let pending_sentinel_offset = index
            .events
            .iter()
            .find(|event| {
                event.k == Kind::A
                    && event.t == Some(crate::events::parse_ms("2026-10-15T00:00:32Z").unwrap())
            })
            .unwrap()
            .o;
        let failed_wrapper_offset = index.events[failed_parent].o;
        let failed_child_offset = index.events[failed_child].o;
        let failed_sentinel_offset = index
            .events
            .iter()
            .find(|event| {
                event.k == Kind::A
                    && event.t == Some(crate::events::parse_ms("2026-10-15T00:00:37Z").unwrap())
            })
            .unwrap()
            .o;
        let slot_position = |offset, matches: fn(&crate::model::SlotKind) -> bool| {
            transcript
                .slots
                .iter()
                .position(|slot| slot.offset == offset && matches(&slot.kind))
                .unwrap()
        };
        let exec = |kind: &crate::model::SlotKind| matches!(kind, crate::model::SlotKind::Tool { name, .. } if name == "exec");
        let lookup = |kind: &crate::model::SlotKind| matches!(kind, crate::model::SlotKind::Tool { name, .. } if name == "fixture.lookup");
        let assistant = |kind: &crate::model::SlotKind| matches!(kind, crate::model::SlotKind::A);
        let pending_wrapper_slot = slot_position(pending_wrapper_offset, exec);
        let pending_child_slot = slot_position(pending_child_offset, lookup);
        let pending_sentinel_slot = slot_position(pending_sentinel_offset, assistant);
        assert!(
            pending_sentinel_slot < pending_child_slot && pending_child_slot < pending_wrapper_slot
        );
        assert!(matches!(
            &transcript.slots[pending_wrapper_slot].kind,
            crate::model::SlotKind::Tool {
                shown: crate::model::Shown::Unfinished | crate::model::Shown::Live,
                ..
            }
        ));

        let failed_wrapper_slot = slot_position(failed_wrapper_offset, exec);
        let failed_child_slot = slot_position(failed_child_offset, lookup);
        let failed_sentinel_slot = slot_position(failed_sentinel_offset, assistant);
        assert!(
            failed_child_slot < failed_sentinel_slot && failed_sentinel_slot < failed_wrapper_slot
        );
        assert!(matches!(
            &transcript.slots[failed_wrapper_slot].kind,
            crate::model::SlotKind::Tool {
                shown: crate::model::Shown::Err,
                reply: Some(reply),
                ..
            } if reply.o == failed_reply.o && reply.t == failed_reply.t
        ));
    }

    #[test]
    fn native_query_last_child_lookup_and_two_channel_deltas_stay_fixed_at_growth() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let mut lookup_steps = Vec::new();
        let mut append_steps = Vec::new();
        for children in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(8192);
            let mut index = fixture.index.clone();
            index.events[0].k = Kind::Tool;
            index.events[0].code_mode = true;
            index.events[0].code_mode_open = true;
            for seq in 1..=children {
                index.events[seq] = Event {
                    k: Kind::Tool,
                    parent: Some(0),
                    o: u64::try_from(seq * 8).unwrap(),
                    t: Some(-i64::try_from(seq).unwrap()),
                    id: Some(format!("regular-{seq}")),
                    ..Event::default()
                };
            }
            index.extras = (0..children)
                .map(|seq| Event {
                    k: if seq % 2 == 0 {
                        Kind::Operation
                    } else {
                        Kind::Tool
                    },
                    parent: Some(0),
                    o: u64::try_from(seq + 1).unwrap(),
                    t: Some(-i64::try_from(seq).unwrap()),
                    ..Event::default()
                })
                .collect();
            OPERATION_FACT_VISITS.with(|count| count.set(0));
            fixture.write_full_index(&index);
            assert_eq!(
                OPERATION_FACT_VISITS.with(Cell::get),
                index.events.len() + index.extras.len()
            );
            let witness = fixture.source();
            let file = fixture
                .store
                .connection
                .query_row(
                    "SELECT file_id FROM files WHERE path=?1",
                    [witness.source.path.to_str().unwrap()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap();
            let rows: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_source_child_operations WHERE file_id=?1",
                    [file],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, i64::try_from(children * 2).unwrap());
            if children == 0 {
                assert!(
                    fixture
                        .store
                        .last_operation(&witness, 0)
                        .unwrap()
                        .0
                        .is_none()
                );
            }

            let previous = index.clone();
            index.events.push(Event {
                k: Kind::Tool,
                parent: Some(0),
                o: u64::try_from(100_000 + children).unwrap(),
                t: Some(-100_000),
                ..Event::default()
            });
            index.extras.push(Event {
                k: Kind::Tool,
                parent: Some(0),
                o: u64::try_from(200_000 + children).unwrap(),
                t: Some(-100_000),
                ..Event::default()
            });
            let changes = super::super::super::DirtyRows::between(&previous, &index);
            OPERATION_FACT_VISITS.with(|count| count.set(0));
            OPERATION_FACT_CUT_ROWS.with(|count| count.set(0));
            let measured = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&measured);
            fixture.store.connection.progress_handler(
                1,
                Some(move || {
                    counted.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            );
            let ledger = witness.ledger.as_ref().unwrap();
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_source_facts(
                &tx,
                witness.source.path.to_str().unwrap(),
                ledger,
                &index,
                &changes,
                false,
                Some(ledger),
            )
            .unwrap();
            tx.commit().unwrap();
            fixture
                .store
                .connection
                .progress_handler(0, None::<fn() -> bool>);
            append_steps.push(measured.load(Ordering::Relaxed));
            assert_eq!(OPERATION_FACT_VISITS.with(Cell::get), 2);
            assert_eq!(OPERATION_FACT_CUT_ROWS.with(Cell::get), 0);

            // A scalar child lookup does not decode or retain a complete FileIndex.
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let (last, work) = fixture.store.last_operation(&witness, 0).unwrap();
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
            assert_eq!(
                last,
                Some((
                    EventAddress {
                        extra: true,
                        seq: children,
                    },
                    SourceTime::Observed(-100_000),
                    (u64::try_from(200_000 + children).unwrap(), 0)
                ))
            );
            assert_eq!(work.decoded_rows, 2);
            assert_eq!(
                work.metadata_bytes,
                source_operation_generation(ledger).unwrap().len() + 40
            );
            lookup_steps.push(work.vm_steps);

            let before_cut = index.clone();
            index.events.pop();
            index.extras.pop();
            let cut = super::super::super::DirtyRows::between(&before_cut, &index);
            OPERATION_FACT_VISITS.with(|count| count.set(0));
            OPERATION_FACT_CUT_ROWS.with(|count| count.set(0));
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_source_facts(
                &tx,
                witness.source.path.to_str().unwrap(),
                ledger,
                &index,
                &cut,
                false,
                Some(ledger),
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(OPERATION_FACT_VISITS.with(Cell::get), 0);
            assert_eq!(OPERATION_FACT_CUT_ROWS.with(Cell::get), 2);
            let rows: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_source_child_operations WHERE file_id=?1",
                    [file],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, i64::try_from(children * 2).unwrap());

            if children > 0 {
                let before_replace = index.clone();
                index.extras[children - 1] = Event {
                    k: Kind::Operation,
                    parent: Some(1),
                    o: u64::try_from(300_000 + children).unwrap(),
                    t: Some(42),
                    ..Event::default()
                };
                index.events[children] = Event {
                    k: Kind::Tool,
                    parent: Some(1),
                    o: u64::try_from(400_000 + children).unwrap(),
                    t: Some(42),
                    ..Event::default()
                };
                let replacement = super::super::super::DirtyRows::between(&before_replace, &index);
                assert_eq!(
                    replacement.extras.iter().copied().collect::<Vec<_>>(),
                    vec![children - 1]
                );
                assert_eq!(
                    replacement.events.iter().copied().collect::<Vec<_>>(),
                    vec![children]
                );
                OPERATION_FACT_VISITS.with(|count| count.set(0));
                fixture.write_delta_index(&before_replace, &index);
                assert_eq!(OPERATION_FACT_VISITS.with(Cell::get), 2);
                let parent_zero = if children > 1 {
                    Some((
                        EventAddress {
                            extra: false,
                            seq: children - 1,
                        },
                        SourceTime::Observed(-i64::try_from(children - 1).unwrap()),
                        (u64::try_from((children - 1) * 8).unwrap(), 0),
                    ))
                } else {
                    None
                };
                assert_eq!(
                    fixture.store.last_operation(&witness, 0).unwrap().0,
                    parent_zero
                );
                assert_eq!(
                    fixture.store.last_operation(&witness, 1).unwrap().0,
                    Some((
                        EventAddress {
                            extra: false,
                            seq: children,
                        },
                        SourceTime::Observed(42),
                        (u64::try_from(400_000 + children).unwrap(), 0)
                    ))
                );
            }
        }
        assert!(lookup_steps.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(append_steps.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn native_query_old_child_footer_stays_pending_until_complete_repair() {
        let mut fixture = Fixture::new(0);
        let mut index = fixture.index.clone();
        index.extras = vec![Event {
            k: Kind::Operation,
            parent: Some(0),
            o: 4,
            t: Some(9),
            ..Event::default()
        }];
        fixture.write_full_index(&index);
        let witness = fixture.source();
        assert!(
            fixture
                .store
                .last_operation(&witness, 0)
                .unwrap()
                .0
                .is_some()
        );

        fixture
            .store
            .connection
            .execute_batch(
                "UPDATE native_source_child_operation_facts SET generation=replace(generation,'operation-v2:','operation-v1:'); UPDATE meta SET value='6' WHERE key='native_query_schema';",
            )
            .unwrap();
        assert!(matches!(
            init(&mut fixture.store.connection).unwrap(),
            Init::Ready
        ));
        assert!(fixture.store.last_operation(&witness, 0).is_err());

        let previous = index.clone();
        index.extras.push(Event {
            k: Kind::Operation,
            parent: Some(0),
            o: 6,
            t: Some(10),
            ..Event::default()
        });
        fixture.write_delta_index(&previous, &index);
        assert!(fixture.store.last_operation(&witness, 0).is_err());

        fixture.write_full_index(&index);
        assert_eq!(
            fixture.store.last_operation(&witness, 0).unwrap().0,
            Some((
                EventAddress {
                    extra: true,
                    seq: 1,
                },
                SourceTime::Observed(10),
                (6, 0),
            ))
        );
        fixture
            .store
            .connection
            .execute(
                "DELETE FROM files WHERE path=?1",
                [witness.source.path.to_str().unwrap()],
            )
            .unwrap();
        let remaining: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT (SELECT count(*) FROM native_source_child_operations)+(SELECT count(*) FROM native_source_child_operation_facts)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn native_query_checkpoint_cancellation_reopen_and_atomic_ready_pointer() {
        let mut fixture = Fixture::new(128);
        let job = fixture.admitted();
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == 2
        };
        let row = stage_row(0, 0);
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "slots",
                    "one",
                    &serde_json::json!({"rank":1}),
                    std::slice::from_ref(&row),
                    &cancel
                )
                .unwrap(),
            Outcome::Deferred
        );
        let same = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        assert_eq!(same.revision, job.revision);
        assert!(
            fixture
                .store
                .stage_get(&same, RowKind::Slot, "slot:0")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &same,
                    "slots",
                    "one",
                    &serde_json::json!({"rank":1}),
                    &[row],
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let resumed = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        assert_eq!(resumed.cursor, "one");
        assert_eq!(resumed.state["rank"], 1);
        assert!(
            fixture
                .store
                .ready(&job.spec.view_key, &|_, _| Ok(true))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fixture
                .store
                .checkpoint(&resumed, "finish", "complete", &resumed.state, &[], &|| {
                    false
                })
                .unwrap(),
            Outcome::Written
        );
        let finished = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let complete = |db: &Connection, job: i64| {
            db.query_row(
                "SELECT count(*)=1 FROM native_query_rows WHERE job=?1 AND kind='slot'",
                [job],
                |r| r.get(0),
            )
        };
        assert_eq!(
            fixture
                .store
                .qualify(
                    &finished,
                    &serde_json::json!({"total":1}),
                    &complete,
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        let (ready, metadata) = fixture
            .store
            .ready(&job.spec.view_key, &complete)
            .unwrap()
            .unwrap();
        assert_eq!(metadata["total"], 1);
        assert_eq!(
            fixture
                .store
                .stage_page(&ready, RowKind::Slot, None)
                .unwrap()
                .rows
                .len(),
            1
        );
    }
    #[test]
    fn native_query_owner_scoped_work_allows_unrelated_publication_and_refuses_related_aba() {
        let mut fixture = Fixture::new(0);
        let job = fixture.admitted();
        fixture
            .store
            .connection
            .execute(
                "UPDATE meta SET value='unrelated-publication' WHERE key='catalog_generation'",
                [],
            )
            .unwrap();
        assert!(fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "entries",
                    "advanced",
                    &serde_json::json!({}),
                    &[],
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        let job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        fixture
            .store
            .connection
            .execute(
                "DELETE FROM session_catalog WHERE session_key=?1",
                [&fixture.row.key],
            )
            .unwrap();
        fixture.store.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",params![fixture.row.key,fixture.row.last.unwrap(),fixture.row.harness,json(&fixture.row).unwrap()]).unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .checkpoint(&job, "slots", "stale", &serde_json::json!({}), &[], &|| {
                    false
                })
                .unwrap(),
            Outcome::Conflict
        );
    }
    #[test]
    fn native_query_scalar_trace_witness_ignores_busy_history_bytes_and_checks_generation() {
        let mut fixture = Fixture::new(0);
        let busy = serde_json::json!({"busy":vec![(1,2);8192]}).to_string();
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO session_trace_headers VALUES(?1,'trace-one','source-one',?2)",
                params![fixture.row.key, busy],
            )
            .unwrap();
        let owner = fixture.store.owner_witness(&fixture.row).unwrap();
        assert!(owner_bytes(&owner) < 4096);
        let job = fixture.admitted();
        assert!(fixture.store.recheck(&job).unwrap());
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_trace_headers SET generation='trace-two' WHERE session_key=?1",
                [&fixture.row.key],
            )
            .unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
    }
    #[test]
    fn native_query_cleanup_respects_combined_row_and_byte_budget_then_restarts() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        for page in 0..3 {
            let rows = (page * 30..(page + 1) * 30)
                .map(|rank| stage_row(rank, 20_000))
                .collect::<Vec<_>>();
            assert_eq!(
                fixture
                    .store
                    .checkpoint(
                        &job,
                        "slots",
                        "page",
                        &serde_json::json!({}),
                        &rows,
                        &|| false
                    )
                    .unwrap(),
                Outcome::Written
            );
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        }
        assert_eq!(fixture.store.restart(&job).unwrap(), Outcome::Written);
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let before: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_query_rows WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fixture.store.cleanup(&job).unwrap(), Outcome::Deferred);
        let after: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_query_rows WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!((1..=52).contains(&(before - after)));
        let mut attempts = 1;
        while let Some(current) = fixture.store.reload(&job.spec.view_key).unwrap() {
            assert!(attempts < 8);
            fixture.store.cleanup(&current).unwrap();
            attempts += 1;
        }
        let next = fixture.store.begin(&fixture.spec()).unwrap();
        assert!(next.id > job.id);
        assert_eq!(next.phase, "capture");
    }
    #[test]
    fn native_query_cleanup_receipts_bound_actual_work_across_history_and_reopen() {
        let mut first_vm = Vec::new();
        for earlier in [0, 128, 1024, 8192] {
            let mut fixture = Fixture::new(earlier);
            let mut job = fixture.admitted();
            let rows = (0..64)
                .map(|rank| stage_row(rank, 1000))
                .collect::<Vec<_>>();
            assert_eq!(
                fixture
                    .store
                    .checkpoint(
                        &job,
                        "slots",
                        "page",
                        &serde_json::json!({}),
                        &rows,
                        &|| false
                    )
                    .unwrap(),
                Outcome::Written
            );
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            assert_eq!(fixture.store.restart(&job).unwrap(), Outcome::Written);
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let (attempt, calls, bytes) =
                crate::test_allocations::measure(|| fixture.store.cleanup_with_work(&job).unwrap());
            assert_eq!(attempt.outcome, Outcome::Deferred);
            assert!(attempt.work.sql_reads > 0 && attempt.work.sql_writes > 1);
            assert!(attempt.work.rows() <= ROWS && attempt.work.metadata_bytes <= BYTES);
            assert!(
                attempt.connection_vm_steps >= attempt.work.vm_steps as u64
                    && attempt.connection_vm_steps <= 500_000
            );
            assert!(
                calls > 0 && bytes <= 2 * BYTES && crate::test_allocations::peak_bytes() <= BYTES
            );
            first_vm.push(attempt.connection_vm_steps);
            let stale = fixture.store.cleanup_with_work(&job).unwrap();
            assert_eq!(stale.outcome, Outcome::Conflict);
            assert_eq!(stale.work.sql_reads, 1);
            assert_eq!(stale.work.sql_writes, 0);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            let mut finished = false;
            for _ in 0..16 {
                let Some(current) = fixture.store.reload(&job.spec.view_key).unwrap() else {
                    finished = true;
                    break;
                };
                let attempt = fixture.store.cleanup_with_work(&current).unwrap();
                assert!(matches!(
                    attempt.outcome,
                    Outcome::Deferred | Outcome::Written
                ));
                assert!(attempt.work.rows() <= ROWS && attempt.work.metadata_bytes <= BYTES);
                assert!(
                    attempt.connection_vm_steps >= attempt.work.vm_steps as u64
                        && attempt.connection_vm_steps <= 500_000
                );
            }
            assert!(finished, "cleanup did not resume: prefix {earlier}");
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
        }
        assert!(
            first_vm.windows(2).all(|pair| pair[1] <= pair[0] + 2048),
            "fixed cleanup grew with native history: {first_vm:?}"
        );
    }

    #[test]
    fn native_query_cleanup_guards_large_kind_before_rust_allocation() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        let kind = "k".repeat(2 * BYTES);
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO native_query_rows VALUES(?1,?2,0,0,0,0,0,0,'small','{}')",
                params![job.id, kind],
            )
            .unwrap();
        assert_eq!(fixture.store.restart(&job).unwrap(), Outcome::Written);
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let (attempt, calls, allocated) =
            crate::test_allocations::measure(|| fixture.store.cleanup_with_work(&job).unwrap());
        assert_eq!(attempt.outcome, Outcome::Deferred);
        assert_eq!(attempt.work.metadata_bytes, 0, "oversize row was admitted");
        assert_eq!(attempt.work.sql_writes, 1, "oversize row was deleted");
        assert!(attempt.work.rows() <= ROWS && attempt.connection_vm_steps > 0);
        assert!(
            calls > 0 && allocated < 128 * 1024,
            "kind allocated {allocated} Rust bytes"
        );
        assert!(crate::test_allocations::peak_bytes() < 128 * 1024);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT octet_length(kind) FROM native_query_rows WHERE job=?1",
                    [job.id],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap(),
            (2 * BYTES) as i64
        );
    }

    #[test]
    fn native_query_cleanup_full_byte_row_retires_before_header_without_pending_forever() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        let metadata =
            serde_json::to_string(&"x".repeat(BYTES - "max".len() - "slot".len() - 64 - 2))
                .unwrap();
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO native_query_rows VALUES(?1,'slot',0,0,0,0,0,0,'max',?2)",
                params![job.id, metadata],
            )
            .unwrap();
        assert_eq!(
            fixture.store.connection.query_row(
                "SELECT octet_length(metadata)+octet_length(identity)+octet_length(kind)+64 FROM native_query_rows WHERE job=?1",
                [job.id], |r| r.get::<_, i64>(0),
            ).unwrap(), BYTES as i64,
        );
        assert_eq!(fixture.store.restart(&job).unwrap(), Outcome::Written);
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let attempt = fixture.store.cleanup_with_work(&job).unwrap();
        assert_eq!(attempt.outcome, Outcome::Deferred);
        assert_eq!(attempt.work.metadata_bytes, BYTES);
        assert!(attempt.work.rows() <= ROWS);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_query_rows WHERE job=?1",
                    [job.id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let current = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let attempt = fixture.store.cleanup_with_work(&current).unwrap();
        assert_eq!(attempt.outcome, Outcome::Written);
        assert!(attempt.work.rows() <= ROWS && attempt.work.metadata_bytes <= BYTES);
    }

    // Independent payload sum: every variable column in the source schema,
    // seven 8-byte INTEGER payloads (job/present/dev/ino/size/resume_at/claim),
    // and the cleanup owner's additional conservative 8-byte scalar reserve.
    fn cleanup_source_payload_bytes(db: &Connection, job: i64) -> usize {
        db.query_row(
            "SELECT COALESCE(sum(7*8+8+length(CAST(path AS BLOB))+length(CAST(metadata AS BLOB))+COALESCE(length(mtime_ns),0)+COALESCE(length(ctime_ns),0)+COALESCE(length(prefix),0)+COALESCE(length(tail),0)+COALESCE(length(CAST(archive AS BLOB)),0)+COALESCE(length(CAST(mutation AS BLOB)),0)),0) FROM native_query_sources WHERE job=?1",
            [job], |row| row.get::<_, i64>(0),
        ).unwrap().try_into().unwrap()
    }

    #[test]
    fn native_query_cleanup_source_blobs_obey_actual_payload_boundary_and_resume() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        fixture.store.connection.execute(
            "UPDATE native_query_sources SET path='a',metadata='',mtime_ns=zeroblob(16),ctime_ns=zeroblob(16),prefix=zeroblob(32),tail=zeroblob(32),archive='archive',mutation='mutation' WHERE job=?1",
            [job.id],
        ).unwrap();
        assert_eq!(fixture.store.connection.query_row(
            "SELECT length(mtime_ns)+length(ctime_ns)+length(prefix)+length(tail) FROM native_query_sources WHERE job=?1",
            [job.id], |row| row.get::<_, i64>(0),
        ).unwrap(), 96);
        let empty_payload = cleanup_source_payload_bytes(&fixture.store.connection, job.id);
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_query_sources SET metadata=?2 WHERE job=?1",
                params![job.id, "x".repeat(BYTES - 16 - empty_payload)],
            )
            .unwrap();
        assert_eq!(
            cleanup_source_payload_bytes(&fixture.store.connection, job.id),
            BYTES - 16
        );
        fixture.store.connection.execute(
            "INSERT INTO native_query_sources SELECT job,'z','{}',present,dev,ino,size,mtime_ns,ctime_ns,resume_at,prefix,tail,claim,archive,mutation FROM native_query_sources WHERE job=?1",
            [job.id],
        ).unwrap();
        let before = cleanup_source_payload_bytes(&fixture.store.connection, job.id);
        fixture.store.restart(&job).unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let attempt = fixture.store.cleanup_with_work(&job).unwrap();
        assert_eq!(attempt.outcome, Outcome::Deferred);
        let remaining = cleanup_source_payload_bytes(&fixture.store.connection, job.id);
        assert!(
            remaining > 16,
            "the next witness must exceed the residual allowance"
        );
        assert_eq!(attempt.work.metadata_bytes, before - remaining);
        assert_eq!(attempt.work.metadata_bytes, BYTES - 16);
        assert!(attempt.work.rows() <= ROWS && attempt.work.metadata_bytes <= BYTES);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT path FROM native_query_sources WHERE job=?1",
                    [job.id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "z"
        );
        let current = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let attempt = fixture.store.cleanup_with_work(&current).unwrap();
        assert_eq!(attempt.outcome, Outcome::Written);
        assert!(attempt.work.rows() <= ROWS && attempt.work.metadata_bytes <= BYTES);
        assert!(fixture.store.reload(&job.spec.view_key).unwrap().is_none());
    }

    #[test]
    fn native_query_cleanup_guards_oversize_source_blob_before_key_decode() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_query_sources SET prefix=zeroblob(?2) WHERE job=?1",
                params![job.id, (2 * BYTES) as i64],
            )
            .unwrap();
        fixture.store.restart(&job).unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let (attempt, _, allocated) =
            crate::test_allocations::measure(|| fixture.store.cleanup_with_work(&job).unwrap());
        assert_eq!(attempt.outcome, Outcome::Deferred);
        assert_eq!(attempt.work.metadata_bytes, 0);
        assert_eq!(attempt.work.sql_writes, 1, "only the revision may change");
        assert!(attempt.work.rows() <= ROWS && attempt.connection_vm_steps > 0);
        assert!(allocated < 128 * 1024 && crate::test_allocations::peak_bytes() < 128 * 1024);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT length(prefix) FROM native_query_sources WHERE job=?1",
                    [job.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            (2 * BYTES) as i64
        );
    }

    #[test]
    fn native_query_maximum_admitted_source_retires_with_present_optional_and_absent_ledger() {
        for state in ["populated", "without-ctime", "absent"] {
            let mut fixture = Fixture::new(0);
            let path = fixture.row.sources[0].path.to_string_lossy().into_owned();
            match state {
                "without-ctime" => {
                    fixture
                        .store
                        .connection
                        .execute("UPDATE files SET ctime_ns=NULL WHERE path=?1", [&path])
                        .unwrap();
                    fixture.row.sources[0].changed_ns = None;
                }
                "absent" => {
                    fixture
                        .store
                        .connection
                        .execute("DELETE FROM files WHERE path=?1", [&path])
                        .unwrap();
                }
                _ => {}
            }
            let mut source = fixture.source();
            source.source.native_id.clear();
            let fixed = source_witness_bytes(&source, &json(&source.source).unwrap());
            source.source.native_id = "x".repeat(BYTES - fixed);
            let mut job = fixture.store.begin(&fixture.spec()).unwrap();
            source.source.native_id.push('x');
            let error = fixture
                .store
                .admit(&job, std::slice::from_ref(&source), &[], "")
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("native witness metadata budget exceeded")
            );
            source.source.native_id.pop();
            assert_eq!(
                fixture.store.admit(&job, &[source], &[], "").unwrap(),
                Outcome::Written
            );
            assert_eq!(
                cleanup_source_payload_bytes(&fixture.store.connection, job.id),
                BYTES,
                "{state}"
            );
            let blob_bytes: i64 = fixture.store.connection.query_row(
                "SELECT COALESCE(length(mtime_ns),0)+COALESCE(length(ctime_ns),0)+COALESCE(length(prefix),0)+COALESCE(length(tail),0) FROM native_query_sources WHERE job=?1",
                [job.id], |row| row.get(0),
            ).unwrap();
            assert_eq!(
                blob_bytes,
                match state {
                    "absent" => 0,
                    "without-ctime" => 80,
                    _ => 96,
                }
            );
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            fixture.store.restart(&job).unwrap();
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            let attempt = fixture.store.cleanup_with_work(&job).unwrap();
            assert_eq!(attempt.outcome, Outcome::Deferred, "{state}");
            assert_eq!(attempt.work.metadata_bytes, BYTES, "{state}");
            assert!(attempt.work.rows() <= ROWS);
            let current = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            let attempt = fixture.store.cleanup_with_work(&current).unwrap();
            assert_eq!(attempt.outcome, Outcome::Written, "{state}");
            assert!(attempt.work.rows() <= ROWS && attempt.work.metadata_bytes <= BYTES);
        }
    }

    #[test]
    fn native_query_cleanup_owner_receipt_includes_job_scalar() {
        let fixture = Fixture::new(0);
        let job = fixture.store.connection.query_row(
            "INSERT INTO native_query_jobs(view_key,version,runtime,phase,revision,cursor,state) VALUES('owner-payload',1,'runtime','cleanup',0,'','{}') RETURNING job",
            [], |row| row.get::<_, i64>(0),
        ).unwrap();
        fixture.store.connection.execute(
            "INSERT INTO native_query_owners VALUES(?1,'identity','key','catalog','history','private','trace','slot','original')",
            [job],
        ).unwrap();
        let expected: usize = fixture.store.connection.query_row(
            "SELECT 8+length(CAST(identity AS BLOB))+length(CAST(session_key AS BLOB))+length(CAST(catalog AS BLOB))+length(CAST(history AS BLOB))+length(CAST(private_header AS BLOB))+length(CAST(trace_header AS BLOB))+length(CAST(slot_header AS BLOB))+length(CAST(history_header AS BLOB)) FROM native_query_owners WHERE job=?1",
            [job], |row| row.get::<_, i64>(0),
        ).unwrap().try_into().unwrap();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        let mut bytes_left = expected - 1;
        let mut work = super::super::native_rank::Work::default();
        assert!(!cleanup_witnesses(&tx, job, "owners", ROWS, &mut bytes_left, &mut work).unwrap());
        assert_eq!(work.metadata_bytes, 0);
        bytes_left = expected;
        assert!(cleanup_witnesses(&tx, job, "owners", ROWS, &mut bytes_left, &mut work).unwrap());
        assert_eq!(bytes_left, 0);
        assert_eq!(work.metadata_bytes, expected);
    }

    fn failed_cleanup_receipt(error: &io::Error) -> &CleanupReceipt {
        let receipt = CleanupReceipt::from_error(error).expect("cleanup failure lost its receipt");
        assert!(
            !receipt.work_complete,
            "failed nested work cannot be claimed exact"
        );
        assert!(receipt.known_work.rows() <= ROWS && receipt.known_work.metadata_bytes <= BYTES);
        assert!(receipt.connection_vm_steps > 0);
        receipt
    }

    #[test]
    fn native_query_cleanup_sql_abort_preserves_attempted_receipt_and_rollback() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        fixture
            .store
            .checkpoint(
                &job,
                "slots",
                "two",
                &serde_json::json!({}),
                &[stage_row(0, 100), stage_row(1, 100)],
                &|| false,
            )
            .unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        fixture.store.restart(&job).unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        fixture.store.connection.execute_batch(
            "CREATE TEMP TRIGGER cleanup_test_abort BEFORE DELETE ON native_query_rows WHEN OLD.identity='slot:1' BEGIN SELECT RAISE(ABORT,'cleanup test abort'); END;",
        ).unwrap();
        // Independently measure the failed statement and rollback VM programs,
        // rather than treating deleted rows as complete connection work.
        fixture
            .store
            .connection
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "DELETE FROM native_query_rows WHERE job=?1 AND identity='slot:0'",
                [job.id],
            )
            .unwrap();
        let (original, failed_vm) = {
            let mut statement = fixture
                .store
                .connection
                .prepare("DELETE FROM native_query_rows WHERE job=?1 AND identity='slot:1'")
                .unwrap();
            let original = statement
                .execute([job.id])
                .map_err(io::Error::other)
                .unwrap_err();
            (
                original,
                statement.get_status(rusqlite::StatementStatus::VmStep) as u64,
            )
        };
        let rollback_vm = {
            let mut statement = fixture.store.connection.prepare("ROLLBACK").unwrap();
            statement.execute([]).unwrap();
            statement.get_status(rusqlite::StatementStatus::VmStep) as u64
        };
        assert!(failed_vm > 0 && rollback_vm > 0);
        let changes = fixture.store.connection.total_changes();
        let error = match fixture.store.cleanup_with_work(&job) {
            Err(error) => error,
            Ok(_) => panic!("ABORT was swallowed"),
        };
        assert_eq!(error.kind(), original.kind());
        assert_eq!(error.to_string(), original.to_string());
        let source = std::error::Error::source(error.get_ref().unwrap()).unwrap();
        assert_eq!(source.to_string(), original.to_string());
        let receipt = failed_cleanup_receipt(&error);
        assert!(receipt.connection_vm_steps >= failed_vm + rollback_vm);
        assert_eq!(
            receipt.known_work.sql_writes, 2,
            "count the failed delete as attempted work"
        );
        assert!(receipt.known_work.metadata_bytes > 200);
        assert_eq!(
            fixture.store.connection.total_changes(),
            changes + 1,
            "the first delete was attempted"
        );
        assert!(fixture.store.connection.is_autocommit());
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_query_rows WHERE job=?1",
                    [job.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2,
            "the first delete must be rolled back"
        );
        assert_eq!(
            fixture
                .store
                .reload(&job.spec.view_key)
                .unwrap()
                .unwrap()
                .revision,
            job.revision
        );
        // The compatibility wrapper must retain the payload as well.
        let error = fixture.store.cleanup(&job).unwrap_err();
        failed_cleanup_receipt(&error);
        fixture
            .store
            .connection
            .execute_batch("DROP TRIGGER cleanup_test_abort")
            .unwrap();
        let attempt = fixture.store.cleanup_with_work(&job).unwrap();
        assert_eq!(attempt.outcome, Outcome::Written);
        assert!(attempt.work.rows() <= ROWS && attempt.work.metadata_bytes <= BYTES);
    }

    #[test]
    fn native_query_cleanup_corrupt_nested_metadata_reports_partial_work_and_keeps_import() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        let metadata = r#"{"Terminal":{"facts":"{}"}}"#;
        let identity = format!("rank/v1/node/64/{:x}", Sha256::digest(metadata.as_bytes()));
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO native_rank_nodes VALUES(?1,64,?2,NULL,NULL,2)",
                params![identity, r#"{"Terminal":{"facts":"[]"}}"#],
            )
            .unwrap();
        for owner in [job.id, job.id + 1] {
            fixture
                .store
                .connection
                .execute(
                    "INSERT INTO native_rank_bindings VALUES(?1,?2)",
                    params![owner, identity],
                )
                .unwrap();
        }
        let memo = format!("rank/v1/memo/{}", "0".repeat(64));
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO native_rank_memos VALUES(?1,?2,'{}')",
                params![job.id, memo],
            )
            .unwrap();
        fixture.store.restart(&job).unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let changes = fixture.store.connection.total_changes();
        let error = match fixture.store.cleanup_with_work(&job) {
            Err(error) => error,
            Ok(_) => panic!("corrupt metadata was swallowed"),
        };
        assert!(
            error
                .to_string()
                .contains("native rank cleanup binding node corrupt")
        );
        let receipt = failed_cleanup_receipt(&error);
        assert!(
            receipt.known_work.sql_reads > 0,
            "the controller admission must remain known"
        );
        assert_eq!(
            receipt.known_work.sql_writes, 0,
            "nested counters were not returned"
        );
        assert_eq!(
            fixture.store.connection.total_changes(),
            changes + 1,
            "nested memo delete was attempted"
        );
        assert!(fixture.store.connection.is_autocommit());
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT refs FROM native_rank_nodes WHERE identity=?1",
                    [&identity],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_rank_bindings WHERE identity=?1",
                    [&identity],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_rank_memos WHERE job=?1",
                    [job.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "nested memo delete must roll back"
        );
        assert_eq!(
            fixture
                .store
                .reload(&job.spec.view_key)
                .unwrap()
                .unwrap()
                .revision,
            job.revision
        );
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_rank_nodes SET metadata=?2 WHERE identity=?1",
                params![identity, metadata],
            )
            .unwrap();
        assert_eq!(
            fixture.store.cleanup_with_work(&job).unwrap().outcome,
            Outcome::Written
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT refs FROM native_rank_nodes WHERE identity=?1",
                    [&identity],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "the imported root must survive predecessor cleanup"
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_rank_bindings WHERE job=?1 AND identity=?2",
                    params![job.id + 1, identity],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn native_query_cleanup_begin_failure_preserves_exact_connection_vm_and_error() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        fixture.store.restart(&job).unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        fixture
            .store
            .connection
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        let direct_meter = super::super::native_rank::cleanup_vm_meter(&fixture.store.connection);
        let original = {
            let mut statement = fixture.store.connection.prepare("BEGIN IMMEDIATE").unwrap();
            let original = statement.execute([]).map_err(io::Error::other).unwrap_err();
            assert!(statement.get_status(rusqlite::StatementStatus::VmStep) > 0);
            original
        };
        let expected_vm = direct_meter.load(std::sync::atomic::Ordering::Relaxed);
        fixture
            .store
            .connection
            .progress_handler(0, None::<fn() -> bool>);
        let error = match fixture.store.cleanup_with_work(&job) {
            Err(error) => error,
            Ok(_) => panic!("nested BEGIN was swallowed"),
        };
        assert_eq!(error.kind(), original.kind());
        assert_eq!(error.to_string(), original.to_string());
        let receipt = failed_cleanup_receipt(&error);
        assert_eq!(receipt.connection_vm_steps, expected_vm);
        assert_eq!(
            receipt.known_work.rows(),
            0,
            "BEGIN failed before metadata admission"
        );
        assert!(
            !fixture.store.connection.is_autocommit(),
            "the preexisting transaction is owned by the caller"
        );
        fixture.store.connection.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            fixture.store.cleanup_with_work(&job).unwrap().outcome,
            Outcome::Written
        );
    }

    #[test]
    fn native_query_primitive_append_changes_tail_only_and_old_missing_primitives_remain_pending() {
        let fixture = Fixture::new(0);
        let path = fixture.row.sources[0].path.to_str().unwrap();
        let witness = fixture.source();
        let ledger = witness.ledger.unwrap();
        let mut index = fixture.index.clone();
        index.busy.last_mut().unwrap().1 += 10;
        index.busy.push((1_400_000, 1_400_001));
        index.links.session_ids.insert("origin".into());
        let changes = super::super::super::DirtyRows::between(&fixture.index, &index);
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_source_facts(&tx, path, &ledger, &index, &changes, false, Some(&ledger)).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            fixture
                .store
                .busy_page(&fixture.source(), None)
                .unwrap()
                .rows,
            vec![(0, 1), (700_000, 700_011), (1_400_000, 1_400_001)]
        );
        fixture
            .store
            .connection
            .execute("DELETE FROM native_source_facts", [])
            .unwrap();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_source_facts(&tx, path, &ledger, &index, &changes, false, Some(&ledger)).unwrap();
        tx.commit().unwrap();
        assert!(
            !fixture
                .store
                .source_header(&fixture.source())
                .unwrap()
                .unwrap()
                .primitives_ready
        );
        assert!(fixture.store.busy_page(&fixture.source(), None).is_err());
    }
    #[test]
    fn native_query_incremental_link_work_is_delta_only_across_history_growth() {
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(0);
            let source = fixture.source();
            let ledger = source.ledger.unwrap();
            let path = source.source.path.to_str().unwrap();
            let mut index = fixture.index.clone();
            index
                .links
                .session_ids
                .extend((0..earlier).map(|n| format!("historical-{n}")));
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path, &ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            let mut changes = super::super::super::DirtyRows::begin(&index);
            index.links.session_ids.insert("new-link".into());
            changes.links.insert(("session".into(), "new-link".into()));
            FULL_LINK_VISITS.with(|n| n.set(0));
            DELTA_LINK_VISITS.with(|n| n.set(0));
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path, &ledger, &index, Some(&changes), Some(&ledger)).unwrap();
            tx.commit().unwrap();
            assert_eq!(FULL_LINK_VISITS.with(Cell::get), 0);
            assert_eq!(DELTA_LINK_VISITS.with(Cell::get), 1);
        }
    }
    #[test]
    fn native_query_symbolic_first_time_and_copy_primitives_have_fixed_append_work() {
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(0);
            let witness = fixture.source();
            let ledger = witness.ledger.as_ref().unwrap();
            let path = witness.source.path.to_str().unwrap();
            let mut index = fixture.index.clone();
            index.events = (0..=earlier)
                .map(|seq| Event {
                    k: Kind::A,
                    o: seq as u64,
                    u: Some(format!("uuid-{seq}")),
                    ..Event::default()
                })
                .collect();
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path, ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            assert_eq!(
                fixture
                    .store
                    .source_header(&fixture.source())
                    .unwrap()
                    .unwrap()
                    .time_seed,
                Some(TimeSeed {
                    first_seq: None,
                    first_ms: None
                })
            );
            let mut work = Vec::new();
            for (time, identity) in [(200, "first-timed"), (10, "later-earlier-time")] {
                let mut changes = super::super::super::DirtyRows::begin(&index);
                index.events.push(Event {
                    k: Kind::Tool,
                    o: index.events.len() as u64,
                    t: Some(time),
                    id: Some(identity.into()),
                    ..Event::default()
                });
                // DirtyRows::begin owns the previous tail. No old candidate is
                // touched when the first native timestamp resolves its seed.
                changes.events.insert(index.events.len() - 1);
                ORDER_FACT_VISITS.with(|n| n.set(0));
                let tx = fixture.store.connection.unchecked_transaction().unwrap();
                write_index_changes(&tx, path, ledger, &index, Some(&changes), Some(ledger))
                    .unwrap();
                tx.commit().unwrap();
                assert_eq!(ORDER_FACT_VISITS.with(Cell::get), 1);
                let source = fixture.source();
                let seed = fixture
                    .store
                    .source_header(&source)
                    .unwrap()
                    .unwrap()
                    .time_seed
                    .unwrap();
                assert_eq!(seed.first_seq, Some(earlier + 1));
                assert_eq!(seed.first_ms, index.events.iter().find_map(|e| e.t));
                assert_eq!(seed.first_ms, Some(200));
                let (present, measured) = fixture
                    .store
                    .source_has_copy_identity(&source, identity)
                    .unwrap();
                assert!(present);
                assert_eq!(measured.decoded_rows, 0);
                work.push(measured.vm_steps);
            }
            assert_eq!(work[0], work[1]);
            assert!(
                fixture
                    .store
                    .source_has_copy_identity(&fixture.source(), "uuid-0")
                    .unwrap()
                    .0
            );
        }
    }

    #[test]
    fn native_query_copy_identity_uses_native_uuid_precedence_and_old_footer_stays_pending() {
        let fixture = Fixture::new(0);
        let witness = fixture.source();
        let ledger = witness.ledger.as_ref().unwrap();
        let path = witness.source.path.to_str().unwrap();
        let mut index = fixture.index.clone();
        index.events = vec![
            Event {
                k: Kind::A,
                id: Some("plain-assistant-id".into()),
                ..Event::default()
            },
            Event {
                k: Kind::Tool,
                id: Some("shadowed-tool-id".into()),
                u: Some("native-uuid".into()),
                ..Event::default()
            },
            Event {
                k: Kind::Xsm,
                id: Some("native-message-id".into()),
                ..Event::default()
            },
        ];
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_index_changes(&tx, path, ledger, &index, None, None).unwrap();
        tx.commit().unwrap();
        for identity in [
            "plain-assistant-id",
            "shadowed-tool-id",
            "native-uuid",
            "native-message-id",
        ] {
            let oracle = index
                .events
                .iter()
                .filter_map(crate::model::native_copy_identity)
                .any(|candidate| candidate == identity);
            assert_eq!(
                fixture
                    .store
                    .source_has_copy_identity(&fixture.source(), identity)
                    .unwrap()
                    .0,
                oracle
            );
        }
        fixture
            .store
            .connection
            .execute("DELETE FROM native_source_order_facts", [])
            .unwrap();
        let changes = super::super::super::DirtyRows::begin(&index);
        index.events.push(Event {
            k: Kind::A,
            t: Some(700),
            ..Event::default()
        });
        ORDER_FACT_VISITS.with(|n| n.set(0));
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_index_changes(&tx, path, ledger, &index, Some(&changes), Some(ledger)).unwrap();
        tx.commit().unwrap();
        assert_eq!(ORDER_FACT_VISITS.with(Cell::get), 1);
        assert!(
            fixture
                .store
                .source_header(&fixture.source())
                .unwrap()
                .unwrap()
                .time_seed
                .is_none()
        );
        assert!(
            fixture
                .store
                .source_has_copy_identity(&fixture.source(), "native-uuid")
                .is_err()
        );
    }
    #[test]
    fn native_rank_transactions_cancel_reopen_and_refuse_changed_related_owners() {
        use super::super::native_rank::ApplyInput;
        let mut fixture = Fixture::new(0);
        let mut related = fixture.row.clone();
        related.key = "related".into();
        related.name = "related".into();
        fixture.store.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",params![related.key,related.last.unwrap(),related.harness,json(&related).unwrap()]).unwrap();
        let spec = fixture.spec();
        let job = fixture.store.begin(&spec).unwrap();
        let source = fixture.source();
        let owners = [
            fixture.store.owner_witness(&fixture.row).unwrap(),
            fixture.store.owner_witness(&related).unwrap(),
        ];
        fixture
            .store
            .admit(&job, &[source], &owners, "complete-closure")
            .unwrap();
        let mut job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        let empty = serde_json::to_string(&crate::model::range_stage::Facts::default()).unwrap();
        let one = serde_json::to_string(&crate::model::range_stage::Facts {
            count: 1,
            ..Default::default()
        })
        .unwrap();
        let checks = Cell::new(0);
        let cancel_after_write = || {
            checks.set(checks.get() + 1);
            checks.get() == 2
        };
        assert_eq!(
            fixture
                .store
                .rank_guard(
                    &job,
                    &one,
                    &empty,
                    &[(0, true)],
                    &|_, _| Ok(true),
                    &cancel_after_write
                )
                .unwrap()
                .0,
            Outcome::Deferred
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_rank_nodes", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let (_, root) = fixture
            .store
            .rank_guard(&job, &one, &empty, &[(0, true)], &|_, _| Ok(true), &|| {
                false
            })
            .unwrap();
        let root = root.unwrap().0;
        job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        let (_, identity) = fixture
            .store
            .rank_guard(&job, &empty, &empty, &[], &|_, _| Ok(true), &|| false)
            .unwrap();
        let identity = identity.unwrap().0;
        job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert_eq!(
            fixture
                .store
                .rank_begin_apply(
                    &job,
                    ApplyInput {
                        cursor: "append",
                        version: "native-range-v1",
                        identity: &identity,
                        left: &root,
                        right: &root
                    },
                    &|_, _| Ok(true),
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert!(
            fixture
                .store
                .rank_advance_apply(
                    &job,
                    "append",
                    &|_, _| Err(io::Error::other("injected native combine failure")),
                    &|_, _| Ok(true),
                    &|| false
                )
                .is_err()
        );
        assert_eq!(
            fixture
                .store
                .reload(&spec.view_key)
                .unwrap()
                .unwrap()
                .revision,
            job.revision
        );
        for table in ["native_rank_nodes", "native_rank_memos"] {
            let before: i64 = fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_rank_nodes", [], |r| r.get(0))
                .unwrap();
            fixture.store.connection.execute_batch(&format!("CREATE TEMP TRIGGER rank_injected_write BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'injected rank write'); END")).unwrap();
            assert!(
                fixture
                    .store
                    .rank_advance_apply(
                        &job,
                        "append",
                        &|left, right| {
                            let mut facts: crate::model::range_stage::Facts =
                                serde_json::from_str(left).map_err(io::Error::other)?;
                            facts
                                .append(serde_json::from_str(right).map_err(io::Error::other)?)
                                .map_err(io::Error::other)?;
                            serde_json::to_string(&facts).map_err(io::Error::other)
                        },
                        &|_, _| Ok(true),
                        &|| false
                    )
                    .is_err()
            );
            fixture
                .store
                .connection
                .execute_batch("DROP TRIGGER rank_injected_write")
                .unwrap();
            assert_eq!(
                fixture
                    .store
                    .reload(&spec.view_key)
                    .unwrap()
                    .unwrap()
                    .revision,
                job.revision
            );
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM native_rank_nodes", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                before
            );
        }
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        let combine = |left: &str, right: &str| {
            let mut facts: crate::model::range_stage::Facts =
                serde_json::from_str(left).map_err(io::Error::other)?;
            facts
                .append(serde_json::from_str(right).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            serde_json::to_string(&facts).map_err(io::Error::other)
        };
        let complete = loop {
            let step = fixture
                .store
                .rank_advance_apply(&job, "append", &combine, &|_, _| Ok(true), &|| false)
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            assert!(step.work.sql_reads + step.work.sql_writes <= ROWS);
            job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
            if let Some(root) = step.complete {
                break root;
            }
        };
        assert!(
            fixture
                .store
                .rank_evaluate(&job, &complete, 1, &|_, _| Ok(true))
                .unwrap()
                .is_none(),
            "staged nodes have no reader authority"
        );
        fixture
            .store
            .checkpoint(
                &job,
                "finish",
                "complete",
                &serde_json::json!({}),
                &[],
                &|| false,
            )
            .unwrap();
        job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        fixture
            .store
            .qualify(
                &job,
                &serde_json::to_value(&complete).unwrap(),
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        let (facts, _) = fixture
            .store
            .rank_evaluate(&job, &complete, 1, &|_, _| Ok(true))
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::model::range_stage::Facts>(&facts)
                .unwrap()
                .count,
            2
        );
        assert!(
            fixture
                .store
                .rank_evaluate(&job, &complete, 1, &|_, _| Ok(false))
                .unwrap()
                .is_none()
        );
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_catalog SET metadata=metadata||' ' WHERE session_key='related'",
                [],
            )
            .unwrap();
        assert!(
            fixture
                .store
                .rank_evaluate(&job, &complete, 1, &|_, _| Ok(true))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn native_rank_qualified_point_read_accounts_for_all_witness_vm_work_at_fixed_page() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let mut vm = Vec::new();
        for earlier in [0, 128, 1024, 8192] {
            let mut fixture = Fixture::new(earlier);
            let mut job = fixture.admitted();
            let empty =
                serde_json::to_string(&crate::model::range_stage::Facts::default()).unwrap();
            let one = serde_json::to_string(&crate::model::range_stage::Facts {
                count: 1,
                ..Default::default()
            })
            .unwrap();
            let (_, root) = fixture
                .store
                .rank_guard(
                    &job,
                    &one,
                    &empty,
                    &[(0, true), (1, false)],
                    &|_, _| Ok(true),
                    &|| false,
                )
                .unwrap();
            let root = root.unwrap().0;
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            fixture
                .store
                .checkpoint(
                    &job,
                    "finish",
                    "complete",
                    &serde_json::json!({}),
                    &[],
                    &|| false,
                )
                .unwrap();
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            fixture
                .store
                .qualify(
                    &job,
                    &serde_json::to_value(&root).unwrap(),
                    &|_, _| Ok(true),
                    &|| false,
                )
                .unwrap();
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let builds = crate::model::COMPAT_BUILDS.with(Cell::get);
            let opcodes = Arc::new(AtomicUsize::new(0));
            let observed = opcodes.clone();
            fixture.store.connection.progress_handler(
                1,
                Some(move || {
                    observed.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            );
            let (facts, work) = fixture
                .store
                .rank_evaluate(&job, &root, 1, &|_, _| Ok(true))
                .unwrap()
                .unwrap();
            vm.push(opcodes.load(Ordering::Relaxed));
            assert_eq!(
                serde_json::from_str::<crate::model::range_stage::Facts>(&facts)
                    .unwrap()
                    .count,
                1
            );
            assert_eq!(work.sql_reads, 3);
            assert!(work.metadata_bytes < 4096);
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
            assert_eq!(crate::model::COMPAT_BUILDS.with(Cell::get), builds);
        }
        assert!(
            vm.windows(2).all(|pair| pair[0] == pair[1]),
            "full qualified point-read SQLite VM work {vm:?}"
        );
    }

    #[test]
    fn native_rank_full_source_guard_admission_resumes_in_bounded_steps() {
        use super::super::native_rank::GuardInput;
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        let empty = serde_json::to_string(&crate::model::range_stage::Facts::default()).unwrap();
        let one = serde_json::to_string(&crate::model::range_stage::Facts {
            count: 1,
            ..Default::default()
        })
        .unwrap();
        let guard = (0..64).map(|source| (source, true)).collect::<Vec<_>>();
        fixture
            .store
            .rank_begin_guard(
                &job,
                GuardInput {
                    cursor: "full-guard",
                    facts: &one,
                    empty: &empty,
                    decisions: &guard,
                },
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        let mut attempts = 0;
        let root = loop {
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            let step = fixture
                .store
                .rank_advance_guard(&job, "full-guard", &|_, _| Ok(true), &|| false)
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            assert!(step.work.sql_reads + step.work.sql_writes <= ROWS);
            assert!(step.work.metadata_bytes <= BYTES);
            attempts += 1;
            assert!(attempts < 10);
            if let Some(root) = step.complete {
                break root;
            }
        };
        assert!(attempts > 1);
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        fixture
            .store
            .checkpoint(
                &job,
                "finish",
                "complete",
                &serde_json::json!({}),
                &[],
                &|| false,
            )
            .unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        fixture
            .store
            .qualify(
                &job,
                &serde_json::to_value(&root).unwrap(),
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let (facts, work) = fixture
            .store
            .rank_evaluate(&job, &root, u64::MAX, &|_, _| Ok(true))
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::model::range_stage::Facts>(&facts)
                .unwrap()
                .count,
            1
        );
        assert_eq!(work.sql_reads, 65);
        for source in 0..64 {
            let (facts, _) = fixture
                .store
                .rank_evaluate(&job, &root, u64::MAX ^ (1u64 << source), &|_, _| Ok(true))
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<crate::model::range_stage::Facts>(&facts)
                    .unwrap()
                    .count,
                0
            );
        }
    }
    #[test]
    fn native_rank_apply_cursor_retains_predecessor_inputs_before_first_resume() {
        use super::super::native_rank::ApplyInput;
        let mut fixture = Fixture::new(0);
        let mut previous = fixture.admitted();
        let empty = serde_json::to_string(&crate::model::range_stage::Facts::default()).unwrap();
        let one = serde_json::to_string(&crate::model::range_stage::Facts {
            count: 1,
            ..Default::default()
        })
        .unwrap();
        let (_, root) = fixture
            .store
            .rank_guard(
                &previous,
                &one,
                &empty,
                &[(0, true)],
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        let root = root.unwrap().0;
        previous = fixture
            .store
            .reload(&previous.spec.view_key)
            .unwrap()
            .unwrap();
        let (_, identity) = fixture
            .store
            .rank_guard(&previous, &empty, &empty, &[], &|_, _| Ok(true), &|| false)
            .unwrap();
        let identity = identity.unwrap().0;
        previous = fixture
            .store
            .reload(&previous.spec.view_key)
            .unwrap()
            .unwrap();
        let spec = JobSpec {
            view_key: "next-publication".into(),
            ..fixture.spec()
        };
        let next = fixture.store.begin(&spec).unwrap();
        let source = fixture.source();
        let owner = fixture.store.owner_witness(&fixture.row).unwrap();
        fixture
            .store
            .admit(&next, &[source], &[owner], "same-qualified-native-prefix")
            .unwrap();
        let next = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        fixture
            .store
            .rank_begin_apply(
                &next,
                ApplyInput {
                    cursor: "native-append",
                    version: "native-range-v1",
                    identity: &identity,
                    left: &root,
                    right: &root,
                },
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        fixture.store.restart(&previous).unwrap();
        while let Some(previous) = fixture.store.reload(&previous.spec.view_key).unwrap() {
            fixture.store.cleanup(&previous).unwrap();
        }
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let mut next = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        loop {
            let advance = fixture
                .store
                .rank_advance_apply(
                    &next,
                    "native-append",
                    &|a, b| {
                        let mut facts: crate::model::range_stage::Facts =
                            serde_json::from_str(a).map_err(io::Error::other)?;
                        facts
                            .append(serde_json::from_str(b).map_err(io::Error::other)?)
                            .map_err(io::Error::other)?;
                        serde_json::to_string(&facts).map_err(io::Error::other)
                    },
                    &|_, _| Ok(true),
                    &|| false,
                )
                .unwrap();
            assert_eq!(advance.outcome, Outcome::Written);
            if advance.complete.is_some() {
                break;
            }
            next = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        }
    }
    #[test]
    fn native_query_all_admitted_owners_and_new_closure_are_fenced() {
        let mut fixture = Fixture::new(0);
        let mut related = fixture.row.clone();
        related.key = "related".into();
        related.name = "related".into();
        fixture.store.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",params![related.key,related.last.unwrap(),related.harness,json(&related).unwrap()]).unwrap();
        let spec = fixture.spec();
        let job = fixture.store.begin(&spec).unwrap();
        let source = fixture.source();
        let owners = [
            fixture.store.owner_witness(&fixture.row).unwrap(),
            fixture.store.owner_witness(&related).unwrap(),
        ];
        assert_eq!(
            fixture
                .store
                .admit(&job, &[source], &owners, "all-owners")
                .unwrap(),
            Outcome::Written
        );
        let job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "finish",
                    "finished",
                    &serde_json::json!({}),
                    &[],
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        let job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert_eq!(
            fixture
                .store
                .qualify(&job, &serde_json::json!({}), &|_, _| Ok(false), &|| false)
                .unwrap(),
            Outcome::Conflict
        );
        assert!(
            fixture
                .store
                .ready(&spec.view_key, &|_, _| Ok(false))
                .unwrap()
                .is_none()
        );
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_catalog SET metadata=metadata||' ' WHERE session_key='related'",
                [],
            )
            .unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "finish",
                    "again",
                    &serde_json::json!({}),
                    &[],
                    &|| false
                )
                .unwrap(),
            Outcome::Conflict
        );
    }
    #[test]
    fn native_query_retired_ledger_witness_keeps_archive_and_custody_fences() {
        let mut fixture = Fixture::new(0);
        let path = fixture.row.sources[0].path.to_str().unwrap().to_owned();
        let present = fixture.source();
        fixture
            .store
            .connection
            .execute("DELETE FROM files WHERE path=?1", [&path])
            .unwrap();
        assert!(fixture.store.event_at(&present, false, 0).is_err());
        let archive_key = archive_index_key(&path);
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO meta(key,value) VALUES(?1,'retained-marker-one')",
                [&archive_key],
            )
            .unwrap();
        let source = fixture.source();
        assert!(source.ledger.is_none());
        assert_eq!(source.archive.as_deref(), Some("retained-marker-one"));
        assert!(fixture.store.source_header(&source).unwrap().is_none());
        assert!(fixture.store.events_page(&source, false, None).is_err());
        assert_eq!(fixture.store.event_at(&source, false, 0).unwrap(), None);
        assert_eq!(
            fixture.store.event_at(&source, false, usize::MAX).unwrap(),
            None
        );
        assert!(fixture.store.event_before_offset(&source, 0).is_err());
        assert!(fixture.store.source_time_at(&source, 0).is_err());
        assert!(fixture.store.source_time_before_offset(&source, 0).is_err());
        assert!(fixture.store.last_operation(&source, 0).is_err());

        fixture.write_full_index(&fixture.index);
        assert!(fixture.store.event_at(&source, false, 0).is_err());
        assert!(fixture.store.source_header(&source).is_err());
        fixture
            .store
            .connection
            .execute("DELETE FROM files WHERE path=?1", [&path])
            .unwrap();
        let mutation_source = fixture.source();
        assert_eq!(
            fixture.store.event_at(&mutation_source, false, 0).unwrap(),
            None
        );
        fixture
            .store
            .connection
            .execute(
                "INSERT OR REPLACE INTO session_source_mutations VALUES(?1,'changed after capture')",
                [&path],
            )
            .unwrap();
        assert!(fixture.store.event_at(&mutation_source, false, 0).is_err());

        let job = fixture.admitted();
        assert!(fixture.store.recheck(&job).unwrap());
        fixture
            .store
            .connection
            .execute(
                "UPDATE meta SET value='retained-marker-two' WHERE key=?1",
                [&archive_key],
            )
            .unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
        // Missing current metadata never asserts permission to read an archive
        // body; only the consumer's original-custody resolver can supply it.
    }
    #[test]
    fn native_query_any_order_busy_delta_matches_native_and_has_fixed_growth_work() {
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(0);
            let witness = fixture.source();
            let ledger = witness.ledger.unwrap();
            let path = witness.source.path.to_str().unwrap();
            let mut index = fixture.index.clone();
            index.busy = (0..=earlier)
                .map(|n| ((n as i64) * 700_000, (n as i64) * 700_000 + 1))
                .collect();
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path, &ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            for time in [-1000, 350_000, 300_000, 700_000, 1_000_000] {
                let mut oracle = index.busy.clone();
                oracle.push((time, time));
                let oracle = super::super::super::busy_merge(oracle);
                index.dirty_rows = Some(super::super::super::DirtyRows::begin(&index));
                super::super::super::activity(&mut index, Some(time));
                assert_eq!(
                    index.busy, oracle,
                    "native busy order earlier={earlier},time={time}"
                );
                let changes = index.dirty_rows.take().unwrap();
                assert!(changes.busy.len() <= 3);
                BUSY_FACT_VISITS.with(|n| n.set(0));
                let tx = fixture.store.connection.unchecked_transaction().unwrap();
                write_source_facts(&tx, path, &ledger, &index, &changes, false, Some(&ledger))
                    .unwrap();
                tx.commit().unwrap();
                assert!(BUSY_FACT_VISITS.with(Cell::get) <= 3);
                let mut actual = Vec::new();
                let mut after = None;
                loop {
                    let page = fixture.store.busy_page(&fixture.source(), after).unwrap();
                    actual.extend(page.rows);
                    after = page.after;
                    if page.complete {
                        break;
                    }
                }
                assert_eq!(actual, index.busy, "earlier={earlier},time={time}");
            }
        }
    }
    #[test]
    fn native_query_old_positional_busy_generation_is_not_qualified_by_partial_append() {
        let mut fixture = Fixture::new(0);
        let witness = fixture.source();
        let ledger = witness.ledger.unwrap();
        let path = witness.source.path.to_str().unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_source_facts SET generation=?1",
                [json(&ledger).unwrap()],
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute_batch("DROP TABLE native_source_busy_intervals; PRAGMA user_version=29;")
            .unwrap();
        assert!(matches!(
            init(&mut fixture.store.connection).unwrap(),
            Init::Ready
        ));
        assert!(
            !fixture
                .store
                .source_header(&fixture.source())
                .unwrap()
                .unwrap()
                .primitives_ready
        );
        let mut index = fixture.index.clone();
        index.dirty_rows = Some(super::super::super::DirtyRows::begin(&index));
        super::super::super::activity(&mut index, Some(-1000));
        let changes = index.dirty_rows.take().unwrap();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_source_facts(&tx, path, &ledger, &index, &changes, false, Some(&ledger)).unwrap();
        tx.commit().unwrap();
        assert!(
            !fixture
                .store
                .source_header(&fixture.source())
                .unwrap()
                .unwrap()
                .primitives_ready
        );
        assert!(fixture.store.busy_page(&fixture.source(), None).is_err());
    }
    #[test]
    fn native_query_pending_work_and_original_admitted_witness_resume_without_index_loads() {
        let mut fixture = Fixture::new(1024);
        let witness = fixture.source();
        let job = fixture.admitted();
        let owner = fixture
            .store
            .admitted_owner(&job, &crate::query_projection::identity(&fixture.row))
            .unwrap()
            .unwrap();
        assert_eq!(owner.key, fixture.row.key);
        let reads = super::super::super::FILE_READS.with(Cell::get);
        let page = fixture.store.pending_jobs(None).unwrap();
        assert!(page.complete);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, job.id);
        assert!(page.work.metadata_bytes < 4096);
        let page = fixture.store.admitted_sources_page(&job, None).unwrap();
        assert!(page.complete);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].ledger, witness.ledger);
        assert_eq!(page.rows[0].source, witness.source);
        assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
        fixture
            .store
            .connection
            .execute(
                "UPDATE meta SET value='new-coarse-marker' WHERE key='catalog_generation'",
                [],
            )
            .unwrap();
        assert_eq!(
            fixture
                .store
                .admitted_sources_page(&job, None)
                .unwrap()
                .rows
                .len(),
            1
        );
        fixture.store.connection.execute("INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at) VALUES(?1,1,'changed',0) ON CONFLICT(source_path) DO UPDATE SET revision=revision+1",[fixture.row.sources[0].path.to_str().unwrap()]).unwrap();
        assert!(fixture.store.admitted_sources_page(&job, None).is_err());
        assert_eq!(fixture.store.pending_jobs(None).unwrap().rows.len(), 1);
    }
    #[test]
    fn native_query_additive_migration_preserves_main_and_temp_and_public_standalone_schemas() {
        let mut fixture = Fixture::new(0);
        fixture.store.connection.execute_batch("CREATE TEMP TABLE native_migration_sentinel(value TEXT); INSERT INTO native_migration_sentinel VALUES('unchanged'); CREATE TABLE native_existing_sentinel(value TEXT); INSERT INTO native_existing_sentinel VALUES('unchanged');").unwrap();
        fixture
            .store
            .connection
            .pragma_update(None, "user_version", 28)
            .unwrap();
        assert!(matches!(
            init(&mut fixture.store.connection).unwrap(),
            Init::Ready
        ));
        for table in ["native_migration_sentinel", "native_existing_sentinel"] {
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(&format!("SELECT value FROM {table}"), [], |r| r
                        .get::<_, String>(0))
                    .unwrap(),
                "unchanged"
            );
        }
        assert_eq!(
            fixture
                .store
                .events_page(&fixture.source(), false, None)
                .unwrap()
                .rows,
            vec![(0, fixture.index.events[0].clone())]
        );
        let standalone = Connection::open_in_memory().unwrap();
        standalone
            .execute_batch(crate::causal_trace::SCHEMA)
            .unwrap();
        assert_eq!(
            standalone
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name='meta'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            standalone
                .query_row(
                    "SELECT (SELECT count(*) FROM sqlite_schema WHERE name IN ('native_source_child_operations','native_source_child_operation_facts','native_source_child_operation_parent'))+(SELECT count(*) FROM sqlite_temp_schema WHERE name IN ('native_source_child_operations','native_source_child_operation_facts','native_source_child_operation_parent'))",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            standalone.query_row(
                "SELECT (SELECT count(*) FROM sqlite_schema WHERE name GLOB 'native_original_*')+(SELECT count(*) FROM sqlite_temp_schema WHERE name GLOB 'native_original_*')",
                [],
                |row| row.get::<_, i64>(0),
            ).unwrap(),
            0,
        );
        assert_eq!(
            standalone.query_row(
                "SELECT (SELECT count(*) FROM sqlite_schema WHERE name GLOB 'native_orthogonal_*' OR name GLOB 'native_rank_candidate_*')+(SELECT count(*) FROM sqlite_temp_schema WHERE name GLOB 'native_orthogonal_*' OR name GLOB 'native_rank_candidate_*')",
                [], |row| row.get::<_, i64>(0),
            ).unwrap(), 0,
        );
    }

    #[test]
    fn native_query_private_five_reopen_installs_original_schema_without_rebinding_evidence() {
        private_original_schema_reopen(5);
    }

    #[test]
    fn native_query_private_seven_reopen_preserves_original_rows_and_installs_new_tables() {
        private_original_schema_reopen(7);
    }

    #[test]
    fn native_query_private_eight_reopen_preserves_original_rows_and_installs_event_tables() {
        private_original_schema_reopen(8);
    }

    #[test]
    fn native_query_private_nine_reopen_preserves_child_facts_and_installs_event_tables() {
        private_original_schema_reopen(9);
    }

    #[test]
    fn native_query_private_ten_reopen_preserves_event_rows_and_installs_policy_restart_tables() {
        private_original_schema_reopen(10);
    }

    #[test]
    fn native_query_private_eleven_reopen_preserves_classes_and_installs_harness_tables() {
        private_original_schema_reopen(11);
    }

    fn private_original_schema_reopen(previous_version: i64) {
        let mut fixture = Fixture::new(0);
        let source = fixture.source();
        let session_key = fixture.row.key.clone();
        let archive_key = archive_index_key(fixture.row.sources[0].path.to_str().unwrap());
        let header_key = format!("history_original_header:{}", fixture.row.key);
        let metadata = json(&fixture.row).unwrap();
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO session_history_catalog VALUES(?1,?2)",
                params![fixture.row.key, metadata],
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO session_history_slot_parts VALUES(?1,0,0,?2)",
                params![fixture.row.key, b"original-custody-part".as_slice()],
            )
            .unwrap();
        for key in [&archive_key, &header_key] {
            fixture
                .store
                .connection
                .execute(
                    "INSERT INTO meta(key,value) VALUES(?1,?2)",
                    params![key, metadata],
                )
                .unwrap();
        }
        if previous_version >= 7 {
            // Preserve every preceding Original table's partial staging rows.
            // Reopening installs schema, without decoding or qualifying them.
            fixture
                .store
                .connection
                .execute_batch(
                    "INSERT INTO native_original_repairs VALUES(X'07',24,'{}',3,'{}');
                 INSERT INTO native_original_records VALUES(X'07',0,8,'lexed');
                 INSERT INTO native_original_tokens VALUES(X'07',0,0,'{}');
                 INSERT INTO native_original_nodes VALUES(X'07',0,0,NULL,NULL,NULL,'{}',8);
                 INSERT INTO native_original_fields VALUES(X'07',0,1,1,2,0,1,'{}','{}',NULL,NULL);",
                )
                .unwrap();
        }
        if previous_version >= 8 {
            fixture
                .store
                .connection
                .execute_batch(
                    "INSERT INTO native_original_numbers VALUES(X'07',0,3,3,4,'{}',NULL);
                 INSERT INTO native_original_members VALUES(X'07',0,0,'field',1,'{}');
                 INSERT INTO native_original_cleanup VALUES(X'07',X'08',0);",
                )
                .unwrap();
        }
        if previous_version >= 10 {
            fixture
                .store
                .connection
                .execute_batch(
                    "INSERT INTO native_original_event_state VALUES(X'07',1);
                 INSERT INTO native_original_events VALUES(X'07',0,1,1,0,0,'{}');
                 INSERT INTO native_original_pending VALUES(X'07',1,0,0);
                 INSERT INTO native_original_tool_ids VALUES(X'07',1,0,1);",
                )
                .unwrap();
        }
        if previous_version >= 11 {
            fixture
                .store
                .connection
                .execute_batch(
                    "INSERT INTO native_original_classes VALUES(X'07',1,0,1,X'08',1);
                 INSERT INTO native_original_identities VALUES(X'07',0,1,1,'{}');
                 INSERT INTO native_original_retirements VALUES(X'07',X'08',0);",
                )
                .unwrap();
        }
        let mut added = vec![
            "native_original_channel_state",
            "native_original_extras",
            "native_original_signals",
            "native_original_positions",
            "native_original_literal",
            "native_original_timestamp",
            "native_original_comparisons",
            "native_original_copilot_body",
            "native_original_tag",
            "native_original_copilot_admission",
            "native_original_header",
            "native_original_activity",
            "native_original_busy",
            "native_original_record_time",
            "native_original_copilot_metadata",
            "native_original_copilot_usage_row",
            "native_original_harness",
            "native_original_claude_usage_step",
            "native_original_claude_usage",
            "native_original_claude_usage_records",
            "native_original_text_epochs",
            "native_original_text_views",
            "native_original_text_parts",
            "native_original_view_identities",
            "native_original_text_boundaries",
            "native_original_class_values",
            "native_original_class_literals",
            "native_original_text_policy",
            "native_original_object_epochs",
            "native_original_record_key_maps",
            "native_original_record_keys",
            "native_original_objects",
            "native_original_object_nodes",
            "native_physical_link_revisions",
            "native_physical_bootstrap",
            "native_physical_profiles",
            "native_physical_atoms",
            "native_physical_atom_parts",
            "native_physical_part_hashes",
            "native_physical_counts",
            "native_physical_pairs",
            "native_physical_bindings",
            "native_physical_orders",
            "native_physical_current_cursors",
            "native_physical_truncations",
            "native_physical_fragments",
            "native_physical_carry",
            "native_rank_candidate_nodes",
            "native_rank_candidate_bindings",
            "native_rank_candidate_roots",
            "native_rank_candidate_cursors",
            "native_rank_candidate_applies",
            "native_orthogonal_nodes",
            "native_orthogonal_roots",
            "native_orthogonal_work",
            "native_orthogonal_frames",
            "native_orthogonal_gc",
            "native_candidate_changes",
            "native_candidate_interests",
            "native_original_copilot_started",
            "native_original_copilot_seen",
        ];
        if previous_version < 11 {
            added.extend([
                "native_original_classes",
                "native_original_identities",
                "native_original_retirements",
            ]);
        }
        if previous_version < 10 {
            added.extend([
                "native_original_event_state",
                "native_original_events",
                "native_original_pending",
                "native_original_tool_ids",
            ]);
        }
        if previous_version < 8 {
            added.extend([
                "native_original_numbers",
                "native_original_members",
                "native_original_cleanup",
            ]);
        }
        if previous_version == 5 {
            added.extend([
                "native_original_repairs",
                "native_original_tokens",
                "native_original_records",
                "native_original_nodes",
                "native_original_fields",
            ]);
        }
        if previous_version < 9 {
            added.extend([
                "native_source_child_operations",
                "native_source_child_operation_facts",
            ]);
        }
        let job = fixture.admitted();
        let source_header = format!(
            "{:?}",
            fixture.store.source_header(&fixture.source()).unwrap()
        );
        let retained = |db: &Connection| {
            let history: String = db
                .query_row(
                    "SELECT metadata FROM session_history_catalog WHERE session_key=?1",
                    [&session_key],
                    |row| row.get(0),
                )
                .unwrap();
            let part: Vec<u8> = db.query_row(
                "SELECT bytes FROM session_history_slot_parts WHERE session_key=?1 AND slot=0 AND part=0",
                [&session_key], |row| row.get(0),
            ).unwrap();
            let markers: Vec<String> = [&archive_key, &header_key]
                .into_iter()
                .map(|key| {
                    db.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
                        row.get(0)
                    })
                    .unwrap()
                })
                .collect();
            (history, part, markers)
        };
        let before = retained(&fixture.store.connection);
        let reads = super::super::super::FILE_READS.with(Cell::get);
        // Exercise both actual reopen entry points against the prior private
        // version, whose native public schema and parser were already current.
        for via_native_store in [true, false] {
            for table in &added {
                fixture
                    .store
                    .connection
                    .execute_batch(&format!("DROP TABLE {table};"))
                    .unwrap();
            }
            if previous_version < 9 {
                fixture
                    .store
                    .connection
                    .execute_batch("DROP TRIGGER native_source_child_operation_cleanup;")
                    .unwrap();
            }
            fixture
                .store
                .connection
                .execute(
                    "UPDATE meta SET value=?2 WHERE key=?1",
                    params![SCHEMA_KEY, previous_version.to_string()],
                )
                .unwrap();
            assert_eq!(
                schema_version(&fixture.store.connection).unwrap(),
                Some(previous_version)
            );
            assert_eq!(
                versions(&fixture.store.connection).unwrap(),
                (28, Some(super::super::super::CACHE_VERSION))
            );
            if previous_version == 5 {
                assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'native_original_*'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
                );
            }
            let snapshot = crate::native_timing::cleanup_tests::snapshot(&fixture.store.connection);
            fixture.store = if via_native_store {
                NativeStore::open(&fixture.options).unwrap()
            } else {
                let (store, _) = SqliteStore::attempt_scoped(
                    &super::super::super::EventCache::path(&fixture.options.cache),
                    None,
                    true,
                )
                .unwrap_or_else(|_| panic!("private schema reopen must succeed"));
                NativeStore {
                    connection: store.connection,
                    options: fixture.options.clone(),
                }
            };
            assert_eq!(
                schema_version(&fixture.store.connection).unwrap(),
                Some(SCHEMA_VERSION)
            );
            assert_eq!(
                versions(&fixture.store.connection).unwrap(),
                (28, Some(super::super::super::CACHE_VERSION))
            );
            for (name, kind) in [
                ("native_original_repairs", "table"),
                ("native_original_tokens", "table"),
                ("native_original_records", "table"),
                ("native_original_nodes", "table"),
                ("native_original_fields", "table"),
                ("native_original_numbers", "table"),
                ("native_original_members", "table"),
                ("native_original_cleanup", "table"),
                ("native_original_retirements", "table"),
                ("native_original_classes", "table"),
                ("native_original_class_candidates", "index"),
                ("native_original_identities", "table"),
                ("native_original_node_parent", "index"),
                ("native_original_node_key", "index"),
                ("native_original_node_element", "index"),
                ("native_original_field_candidate", "index"),
                ("native_original_field_pending", "index"),
                ("native_original_number_pending", "index"),
                ("native_original_event_state", "table"),
                ("native_original_events", "table"),
                ("native_original_pending", "table"),
                ("native_original_tool_ids", "table"),
                ("native_original_event_identity", "index"),
                ("native_original_pending_open", "index"),
                ("native_original_plain_tool_ids", "index"),
                ("native_original_channel_state", "table"),
                ("native_original_extras", "table"),
                ("native_original_signals", "table"),
                ("native_original_positions", "table"),
                ("native_original_literal", "table"),
                ("native_original_timestamp", "table"),
                ("native_original_comparisons", "table"),
                ("native_original_comparison_right", "index"),
                ("native_original_copilot_body", "table"),
                ("native_original_tag", "table"),
                ("native_original_copilot_admission", "table"),
                ("native_original_header", "table"),
                ("native_original_activity", "table"),
                ("native_original_busy", "table"),
                ("native_original_record_time", "table"),
                ("native_original_copilot_metadata", "table"),
                ("native_original_copilot_usage_row", "table"),
                ("native_original_harness", "table"),
                ("native_original_claude_usage_step", "table"),
                ("native_original_claude_usage", "table"),
                ("native_original_claude_usage_records", "table"),
                ("native_original_text_epochs", "table"),
                ("native_original_text_views", "table"),
                ("native_original_text_parts", "table"),
                ("native_original_view_identities", "table"),
                ("native_original_text_boundaries", "table"),
                ("native_original_class_values", "table"),
                ("native_original_class_literals", "table"),
                ("native_original_text_policy", "table"),
                ("native_original_object_epochs", "table"),
                ("native_original_record_key_maps", "table"),
                ("native_original_record_keys", "table"),
                ("native_original_objects", "table"),
                ("native_original_object_nodes", "table"),
                ("native_physical_link_revisions", "table"),
                ("native_physical_bootstrap", "table"),
                ("native_physical_profiles", "table"),
                ("native_physical_profile_path", "index"),
                ("native_physical_atoms", "table"),
                ("native_physical_atom_bucket", "index"),
                ("native_physical_atom_parts", "table"),
                ("native_physical_part_hashes", "table"),
                ("native_physical_counts", "table"),
                ("native_physical_atom_insert", "trigger"),
                ("native_physical_atom_update", "trigger"),
                ("native_physical_atom_delete", "trigger"),
                ("native_physical_part_insert", "trigger"),
                ("native_physical_part_delete", "trigger"),
                ("native_physical_hash_insert", "trigger"),
                ("native_physical_hash_delete", "trigger"),
                ("native_physical_pairs", "table"),
                ("native_physical_reverse_pair", "index"),
                ("native_physical_bindings", "table"),
                ("native_physical_session_bucket", "index"),
                ("native_physical_orders", "table"),
                ("native_physical_current_cursors", "table"),
                ("native_physical_truncations", "table"),
                ("native_physical_fragments", "table"),
                ("native_physical_carry", "table"),
                ("native_rank_candidate_nodes", "table"),
                ("native_rank_candidate_bindings", "table"),
                ("native_rank_candidate_roots", "table"),
                ("native_rank_candidate_cursors", "table"),
                ("native_rank_candidate_applies", "table"),
                ("native_orthogonal_nodes", "table"),
                ("native_orthogonal_roots", "table"),
                ("native_orthogonal_work", "table"),
                ("native_orthogonal_frames", "table"),
                ("native_orthogonal_gc", "table"),
                ("native_candidate_changes", "table"),
                ("native_candidate_interests", "table"),
                ("native_candidate_change_path", "index"),
                ("native_rank_candidate_unused", "index"),
                ("native_rank_candidate_cursor_job", "index"),
                ("native_original_copilot_started", "table"),
                ("native_original_copilot_seen", "table"),
                ("native_original_signal_kind", "index"),
                ("native_original_signal_observation", "index"),
                ("native_original_regular_time", "index"),
                ("native_original_regular_offset", "index"),
                ("native_original_child_order", "index"),
                ("native_source_child_operations", "table"),
                ("native_source_child_operation_facts", "table"),
                ("native_source_child_operation_parent", "index"),
                ("native_source_child_operation_cleanup", "trigger"),
            ] {
                assert!(
                    fixture
                        .store
                        .connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1 AND type=?2)",
                            params![name, kind],
                            |row| row.get::<_, bool>(0),
                        )
                        .unwrap(),
                    "missing Original {kind} {name}"
                );
            }
            let mut reopened =
                crate::native_timing::cleanup_tests::snapshot(&fixture.store.connection);
            for table in &added {
                assert_eq!(reopened.remove(&format!("main.{table}")), Some(Vec::new()));
            }
            reopened
                .get_mut("main.sqlite_schema")
                .unwrap()
                .retain(|row| {
                    !added
                        .iter()
                        .any(|table| row[2] == rusqlite::types::Value::Text((*table).into()))
                        && !(previous_version < 9
                            && row[1]
                                == rusqlite::types::Value::Text(
                                    "native_source_child_operation_cleanup".into(),
                                ))
                });
            for row in reopened.get_mut("main.meta").unwrap() {
                if row[0] == rusqlite::types::Value::Text(SCHEMA_KEY.into()) {
                    row[1] = rusqlite::types::Value::Text(previous_version.to_string());
                }
            }
            assert_eq!(
                reopened, snapshot,
                "private migration changed existing MAIN/TEMP rows or schema"
            );
            if previous_version < 9 {
                assert!(
                    fixture
                        .store
                        .last_operation(&fixture.source(), 0)
                        .unwrap_err()
                        .to_string()
                        .contains("bounded repair")
                );
            } else {
                assert_eq!(
                    fixture
                        .store
                        .last_operation(&fixture.source(), 0)
                        .unwrap()
                        .0,
                    None
                );
            }
            assert!(fixture.store.recheck(&job).unwrap());
            let reloaded = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
            assert_eq!(
                (reloaded.id, reloaded.revision, reloaded.cursor),
                (job.id, job.revision, job.cursor.clone())
            );
            assert_eq!(
                format!(
                    "{:?}",
                    fixture.store.source_header(&fixture.source()).unwrap()
                ),
                source_header
            );
            assert_eq!(retained(&fixture.store.connection), before);
            assert_eq!(fixture.source().ledger, source.ledger);
            assert_eq!(
                fixture
                    .store
                    .events_page(&fixture.source(), false, None)
                    .unwrap()
                    .rows,
                vec![(0, fixture.index.events[0].clone())]
            );
            assert_eq!(
                fixture
                    .store
                    .events_page(&fixture.source(), true, None)
                    .unwrap()
                    .rows,
                vec![(0, fixture.index.extras[0].clone())]
            );
            assert_eq!(
                fixture
                    .store
                    .signals_page(&fixture.source(), None)
                    .unwrap()
                    .rows,
                vec![(0, fixture.index.signals[0].clone())]
            );
            assert_eq!(
                crate::history_projection::catalog(&fixture.store.connection, &fixture.row.key)
                    .unwrap(),
                Some(metadata.clone())
            );
        }
        assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
    }

    #[test]
    fn native_query_private_initialization_preserves_native_schema_and_existing_objects() {
        let mut fixture = Fixture::new(0);
        let ledger = fixture.source().ledger;
        fixture.store.connection.execute_batch("CREATE TEMP TABLE private_temp_sentinel(value TEXT); INSERT INTO private_temp_sentinel VALUES('unchanged'); CREATE TEMP TRIGGER private_temp_trigger AFTER INSERT ON private_temp_sentinel BEGIN SELECT 1; END;").unwrap();
        fixture
            .store
            .connection
            .execute("DELETE FROM meta WHERE key=?1", [SCHEMA_KEY])
            .unwrap();
        let objects = |db: &Connection| {
            db.prepare("SELECT name,type,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' UNION ALL SELECT name,type,sql FROM sqlite_temp_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name,type,sql").unwrap().query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        let before = objects(&fixture.store.connection);
        let reads = super::super::super::FILE_READS.with(Cell::get);
        assert!(
            current(&fixture.store.connection).unwrap(),
            "legacy native28 evidence stays readable before private initialization"
        );
        assert!(matches!(
            init(&mut fixture.store.connection).unwrap(),
            Init::Ready
        ));
        assert_eq!(objects(&fixture.store.connection), before);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            28
        );
        assert_eq!(
            schema_version(&fixture.store.connection).unwrap(),
            Some(SCHEMA_VERSION)
        );
        assert_eq!(fixture.source().ledger, ledger);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT value FROM private_temp_sentinel", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "unchanged"
        );
        assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
        fixture
            .store
            .connection
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                params![SCHEMA_KEY, (SCHEMA_VERSION + 1).to_string()],
            )
            .unwrap();
        assert!(matches!(
            init(&mut fixture.store.connection).unwrap(),
            Init::Newer
        ));
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            28
        );
        assert_eq!(objects(&fixture.store.connection), before);
    }

    #[test]
    fn native_query_unchanged_native_catalog_publication_keeps_repair_witnesses() {
        let mut fixture = Fixture::new(0);
        let job = fixture.admitted();
        let row = &fixture.row;
        let updated = fixture
            .store
            .connection
            .execute(
                &format!("{CATALOG_INSERT}(?1,'current',?2,?3,?4,?5,?6){CATALOG_CONFLICT}"),
                params![
                    row.key,
                    row.last.unwrap(),
                    row.harness,
                    row.repo,
                    row.parent,
                    json(row).unwrap()
                ],
            )
            .unwrap();
        assert_eq!(updated, 0);
        assert!(fixture.store.recheck(&job).unwrap());
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_catalog SET last_ms=last_ms+1 WHERE session_key=?1",
                [&row.key],
            )
            .unwrap();
        assert!(
            !fixture.store.recheck(&job).unwrap(),
            "an independent scalar selection change remains fenced"
        );
    }

    #[test]
    fn native_candidate_delta_captures_old_new_parents_identity_and_placement_before_overwrite() {
        for earlier in [0, 128, 1024, 8192] {
            let mut fixture = Fixture::new(earlier);
            let mut old = fixture.index.clone();
            old.events[0] = Event {
                k: Kind::Tool,
                parent: Some(11),
                u: Some("old-uuid".into()),
                o: 5,
                b: 7,
                t: Some(9),
                ..Event::default()
            };
            old.extras[0] = Event {
                k: Kind::Operation,
                parent: Some(17),
                o: 15,
                b: 2,
                t: Some(19),
                ..Event::default()
            };
            fixture.write_full_index(&old);
            let prior: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT max(change_id) FROM native_candidate_changes",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let mut new = old.clone();
            new.events[0].parent = Some(12);
            new.events[0].u = Some("new-uuid".into());
            new.events[0].o = 6;
            new.extras[0].parent = Some(18);
            new.extras[0].o = 16;
            new.signals[0].n = Some("low".into());
            new.events.push(Event {
                k: Kind::A,
                o: 80_000,
                t: Some(-30),
                ..Event::default()
            });
            let changes = super::super::super::DirtyRows::between(&old, &new);
            let source = fixture.source();
            let ledger = source.ledger.as_ref().unwrap();
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(
                &tx,
                source.source.path.to_str().unwrap(),
                ledger,
                &new,
                Some(&changes),
                Some(ledger),
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
            let source = fixture.source();
            let deltas = fixture
                .store
                .candidate_changes_page(&source, Some(prior))
                .unwrap();
            assert!(deltas.complete);
            assert_eq!(deltas.rows.len(), 1);
            let delta = &deltas.rows[0];
            assert!(!delta.reset);
            assert_eq!(delta.counts, (earlier + 2, 1, 1));
            assert_eq!(delta.before_generation, delta.generation);
            let interests = fixture
                .store
                .candidate_interests_page(&source, delta.id, None)
                .unwrap();
            assert!(interests.complete);
            assert_eq!(interests.rows.len(), 4);
            assert_eq!(interests.work.decoded_rows, 4);
            let point = &interests.rows[0];
            assert_eq!((point.channel, point.first, point.last), (0, 0, Some(0)));
            assert_eq!(
                point.before.as_ref().unwrap(),
                &serde_json::to_value(&old.events[0]).unwrap()
            );
            assert_eq!(
                point.after.as_ref().unwrap(),
                &serde_json::to_value(&new.events[0]).unwrap()
            );
            let suffix = &interests.rows[1];
            assert_eq!(
                (suffix.channel, suffix.first, suffix.last),
                (0, earlier + 1, Some(earlier + 2))
            );
            assert!(suffix.before.is_none() && suffix.after.is_none());
            assert_eq!(
                interests.rows[2].before.as_ref().unwrap(),
                &serde_json::to_value(&old.extras[0]).unwrap()
            );
            assert_eq!(
                interests.rows[2].after.as_ref().unwrap(),
                &serde_json::to_value(&new.extras[0]).unwrap()
            );
            assert_eq!(interests.rows[3].before.as_ref().unwrap()["n"], "high");
            assert_eq!(interests.rows[3].after.as_ref().unwrap()["n"], "low");
            // Immutable delta survives connection replacement and cannot be
            // interpreted through a different path with a reused native ID.
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            assert_eq!(
                fixture
                    .store
                    .candidate_interests_page(&source, delta.id, None)
                    .unwrap()
                    .rows
                    .len(),
                4
            );
            let mut wrong = source.clone();
            wrong.source.path = fixture.options.claude_home.join("projects/p/other.jsonl");
            wrong.ledger = None;
            wrong.claim = None;
            assert!(
                fixture
                    .store
                    .candidate_interests_page(&wrong, delta.id, None)
                    .is_err()
            );
        }
    }

    #[test]
    fn native_candidate_delta_ranges_rollback_and_source_change_fence() {
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(earlier);
            let original = fixture.source();
            let ledger = original.ledger.as_ref().unwrap();
            let prior: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT max(change_id) FROM native_candidate_changes",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let mut cut = fixture.index.clone();
            cut.events.truncate(0);
            cut.extras.truncate(0);
            cut.signals.truncate(0);
            let changes = super::super::super::DirtyRows::between(&fixture.index, &cut);
            {
                let tx = fixture.store.connection.unchecked_transaction().unwrap();
                write_index_changes(
                    &tx,
                    original.source.path.to_str().unwrap(),
                    ledger,
                    &cut,
                    Some(&changes),
                    Some(ledger),
                )
                .unwrap();
                // Cancellation before commit rolls back both row mutation and
                // all interests, including the monotonically allocated ID.
            }
            assert!(
                fixture
                    .store
                    .candidate_changes_page(&original, Some(prior))
                    .unwrap()
                    .rows
                    .is_empty()
            );
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(
                &tx,
                original.source.path.to_str().unwrap(),
                ledger,
                &cut,
                Some(&changes),
                Some(ledger),
            )
            .unwrap();
            tx.commit().unwrap();
            assert!(
                fixture
                    .store
                    .candidate_changes_page(&original, Some(prior))
                    .is_err()
            );
            let source = fixture.source();
            let deltas = fixture
                .store
                .candidate_changes_page(&source, Some(prior))
                .unwrap();
            assert_eq!(deltas.rows.len(), 1);
            let delta = &deltas.rows[0];
            let interests = fixture
                .store
                .candidate_interests_page(&source, delta.id, None)
                .unwrap();
            assert_eq!(
                interests.rows.len(),
                3,
                "cut enumerated old candidate history"
            );
            assert_eq!(interests.work.metadata_bytes, 3 * 64);
            for (channel, interest) in interests.rows.iter().enumerate() {
                assert_eq!(interest.channel, channel as u8);
                assert_eq!(interest.first, 0);
                assert!(
                    interest.last.is_none()
                        && interest.before.is_none()
                        && interest.after.is_none()
                );
            }
            assert_eq!(delta.counts, (0, 0, 0));
            assert!(!delta.reset);
        }
    }

    #[test]
    fn native_candidate_delta_link_changes_and_archive_removal_preserve_custody() {
        for archived in [false, true] {
            let fixture = Fixture::new(128);
            let source = fixture.source();
            let ledger = source.ledger.as_ref().unwrap();
            let old = fixture.index.clone();
            let mut new = old.clone();
            new.links.continued_in.remove("next");
            new.links.session_ids.insert("next".into());
            new.links.bridges.insert("literal:bridge".into());
            let prior: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT max(change_id) FROM native_candidate_changes",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            fixture.write_delta_index(&old, &new);
            let current = fixture.source();
            let change = fixture
                .store
                .candidate_changes_page(&current, Some(prior))
                .unwrap()
                .rows
                .remove(0);
            let links = fixture
                .store
                .candidate_interests_page(&current, change.id, None)
                .unwrap();
            assert_eq!(links.rows.len(), 3);
            assert!(links.rows.iter().all(|interest| interest.channel == 3));
            let observed: std::collections::BTreeMap<_, _> = links
                .rows
                .iter()
                .map(|interest| {
                    let before = interest.before.as_ref().unwrap();
                    let after = interest.after.as_ref().unwrap();
                    (
                        (before[0].as_str().unwrap(), before[1].as_str().unwrap()),
                        (before[2].as_bool().unwrap(), after[2].as_bool().unwrap()),
                    )
                })
                .collect();
            assert_eq!(observed[&("continued", "next")], (true, false));
            assert_eq!(observed[&("session", "next")], (false, true));
            assert_eq!(observed[&("bridge", "literal:bridge")], (false, true));
            let marker = ArchivedIndex {
                version: 1,
                parser_version: CACHE_VERSION,
                source: crate::SessionSourceRef {
                    root: "claude".into(),
                    path: "projects/p/chosen.jsonl".into(),
                    native_id: "chosen".into(),
                    offset: ledger.offset,
                    prefix_sha256: ledger.prefix,
                    tail_sha256: ledger.tail,
                },
                stat: ledger.stat.clone(),
                projection_version: 1,
                projection_generation: "retained-query-witness".into(),
                native_header_sha256: None,
                source_chunks_sha256: None,
                parent_marker_sha256: None,
                launch_calls_sha256: None,
            };
            let marker = json(&marker).unwrap();
            if archived {
                fixture
                    .store
                    .connection
                    .execute(
                        "INSERT INTO meta VALUES(?1,?2)",
                        params![
                            archive_index_key(source.source.path.to_str().unwrap()),
                            marker
                        ],
                    )
                    .unwrap();
            }
            let (mut writer, _) = SqliteStore::attempt_scoped(
                &super::super::super::EventCache::path(&fixture.options.cache),
                None,
                true,
            )
            .unwrap_or_else(|error| panic!("removal fixture: {error}"));
            assert_eq!(
                writer
                    .remove(&[(
                        source.source.path.to_string_lossy().into_owned(),
                        Some(ledger.clone())
                    )])
                    .unwrap(),
                Outcome::Written
            );
            let current = fixture
                .store
                .original_source_witness(&source.source)
                .unwrap();
            let changes = fixture
                .store
                .candidate_changes_page(&current, Some(change.id))
                .unwrap();
            assert_eq!(changes.rows.len(), 1);
            let removed = &changes.rows[0];
            assert!(removed.reset && removed.generation.is_none());
            assert_eq!(
                removed.before_generation,
                Some(source_fact_generation(ledger).unwrap())
            );
            assert_eq!(
                fixture
                    .store
                    .candidate_interests_page(&current, removed.id, None)
                    .unwrap()
                    .rows
                    .len(),
                3
            );
            let rows: usize = fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
                .unwrap();
            assert_eq!(rows, if archived { 130 } else { 0 });
            assert_eq!(
                fs::read(&source.source.path).unwrap(),
                b"fixture\n".repeat(129)
            );
            if archived {
                let retained: String = fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT value FROM meta WHERE key=?1",
                        [archive_index_key(source.source.path.to_str().unwrap())],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(retained, marker);
            }
            // The already-removed observation creates no new delta or archive
            // mutation, and keeps the actual original bytes untouched.
            assert_eq!(
                writer
                    .remove(&[(
                        source.source.path.to_string_lossy().into_owned(),
                        Some(ledger.clone())
                    )])
                    .unwrap(),
                Outcome::Written
            );
            assert!(
                fixture
                    .store
                    .candidate_changes_page(&current, Some(removed.id))
                    .unwrap()
                    .rows
                    .is_empty()
            );
        }
    }

    #[test]
    fn native_candidate_delta_full_replacement_retains_old_generation_range() {
        let fixture = Fixture::new(128);
        let source = fixture.source();
        let prior: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT max(change_id) FROM native_candidate_changes",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ledger = source.ledger.as_ref().unwrap();
        let replacement = FileIndex {
            events: vec![Event {
                k: Kind::U,
                o: 3,
                ..Event::default()
            }],
            ..FileIndex::default()
        };
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_index_changes(
            &tx,
            source.source.path.to_str().unwrap(),
            ledger,
            &replacement,
            None,
            Some(ledger),
        )
        .unwrap();
        tx.commit().unwrap();
        let source = fixture.source();
        let deltas = fixture
            .store
            .candidate_changes_page(&source, Some(prior))
            .unwrap();
        assert_eq!(deltas.rows.len(), 1);
        let delta = &deltas.rows[0];
        assert!(delta.reset);
        assert_eq!(
            delta.before_generation,
            Some(source_fact_generation(ledger).unwrap())
        );
        assert_eq!(delta.counts, (1, 0, 0));
        assert_eq!(
            fixture
                .store
                .candidate_interests_page(&source, delta.id, None)
                .unwrap()
                .rows
                .len(),
            3
        );
        assert!(fixture.store.event_at(&source, false, 1).unwrap().is_none());
        assert!(fixture.store.event_at(&source, true, 0).unwrap().is_none());
        assert_eq!(
            fixture.store.event_at(&source, false, 0).unwrap().unwrap(),
            replacement.events[0]
        );
    }

    #[test]
    fn native_query_old_private_initialization_does_not_index_growing_native_history() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let mut vm = Vec::new();
        for earlier in [0, 128, 1024, 8192] {
            let mut fixture = Fixture::new(earlier);
            fixture.store.connection.execute_batch("DROP TABLE native_source_event_positions; DELETE FROM meta WHERE key='native_query_schema'; UPDATE native_source_order_facts SET generation=replace(generation,'order-v2:','order-v1:');").unwrap();
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let observed = Arc::new(AtomicUsize::new(0));
            let counted = observed.clone();
            fixture.store.connection.progress_handler(
                1,
                Some(move || {
                    counted.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            );
            assert!(matches!(
                init(&mut fixture.store.connection).unwrap(),
                Init::Ready
            ));
            vm.push(observed.load(Ordering::Relaxed));
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM events WHERE extra=0", [], |r| r
                        .get::<_, usize>(0))
                    .unwrap(),
                earlier + 1
            );
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM native_source_event_positions",
                        [],
                        |r| r.get::<_, usize>(0)
                    )
                    .unwrap(),
                0
            );
            assert!(
                fixture
                    .store
                    .source_header(&fixture.source())
                    .unwrap()
                    .unwrap()
                    .time_seed
                    .is_none()
            );
            assert!(
                fixture
                    .store
                    .event_before_offset(&fixture.source(), 0)
                    .is_err()
            );
        }
        assert!(
            vm.windows(2).all(|pair| pair[0] == pair[1]),
            "private initializer SQLite VM work {vm:?}"
        );
    }

    #[test]
    fn source_roots_prepublication_stage_is_typed_durable_and_epoch_fenced() {
        let mut fixture = Fixture::new(0);
        let epoch = source_roots::Epoch {
            epoch: 17,
            base_generation: Some("catalog-one".into()),
            source_revision: 23,
            runtime: fixture.spec().runtime,
        };
        fixture.store.connection.execute("INSERT INTO catalog_semantic_stage(singleton,epoch,phase,generation,runtime,revision) VALUES(1,?1,'publication',?2,?3,?4)",params![epoch.epoch,epoch.base_generation,epoch.runtime,epoch.source_revision]).unwrap();
        let source = fixture.source();
        let mut stage = fixture
            .store
            .begin_source_roots(&epoch, &[source.clone()])
            .unwrap()
            .unwrap();
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT owners FROM native_query_jobs WHERE job=?1",
                    [stage.job.id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert!(
            fixture
                .store
                .checkpoint(
                    &stage.job,
                    "entries",
                    "bad-reader-checkpoint",
                    &serde_json::json!({}),
                    &[],
                    &|| false
                )
                .is_err()
        );
        let (outcome, sql) = fixture
            .store
            .checkpoint_source_roots(
                &stage,
                "entries",
                "source-event-0",
                &serde_json::json!({"epoch":epoch,"next":1}),
                &[],
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        assert_eq!(outcome, Outcome::Written);
        assert!(sql > 0);
        let before = stage.job.revision;
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        stage = fixture
            .store
            .begin_source_roots(&epoch, &[source.clone()])
            .unwrap()
            .unwrap();
        assert!(stage.job.revision > before);
        assert_eq!(stage.job.state["next"], 1);
        assert_eq!(stage.job.cursor, "source-event-0");
        for cancelled in [false, true] {
            fixture
                .store
                .connection
                .execute("UPDATE catalog_semantic_stage SET revision=revision+1", [])
                .unwrap();
            let before = stage.job.revision;
            let (outcome, sql) = fixture
                .store
                .checkpoint_source_roots(
                    &stage,
                    "finish",
                    "unqualified",
                    &serde_json::json!({}),
                    &[],
                    &|_, _| Ok(true),
                    &|| cancelled,
                )
                .unwrap();
            assert_eq!(
                outcome,
                if cancelled {
                    Outcome::Deferred
                } else {
                    Outcome::Conflict
                }
            );
            assert_eq!(
                fixture
                    .store
                    .reload(&stage.job.spec.view_key)
                    .unwrap()
                    .unwrap()
                    .revision,
                before
            );
            if !cancelled {
                assert!(sql > 0);
            }
            fixture
                .store
                .connection
                .execute(
                    "UPDATE catalog_semantic_stage SET revision=?1",
                    [epoch.source_revision],
                )
                .unwrap();
        }
        fixture
            .store
            .connection
            .execute("UPDATE catalog_semantic_stage SET phase='relations'", [])
            .unwrap();
        assert!(
            fixture
                .store
                .begin_source_roots(&epoch, &[source])
                .unwrap()
                .is_none()
        );
    }
}

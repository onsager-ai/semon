use std::{path::Path, str::FromStr};

use rusqlite::{Connection, OptionalExtension, ToSql, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    AuthoredBy, CanonicalTrace, CaptureResult, LogFilter, NewOccurrence, NewRawCarrierRecord,
    OccurrenceRecord, RawCarrierRecord, RepoSource, SemanticCore, TraceId, canonical,
};

/// Claude's per-line sequence stride, shared with its normalizer.
pub const CLAUDE_MAX_BLOCKS_PER_RECORD: i64 = 1024;

/// One complete source line replayed from a saved adapter cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawBackfillLine {
    /// Original line bytes, including the newline.
    pub bytes: Vec<u8>,
    /// Session derived by the adapter's live path.
    pub session: String,
    /// Raw line sequence derived by the adapter's live path.
    pub sequence: i64,
    /// Source timestamp derived by the adapter's live path.
    pub timestamp: i64,
}

/// Result of checking and optionally inserting one file's raw lines.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RawBackfillResult {
    /// Rows inserted, or rows that would be inserted for a dry run.
    pub inserted: usize,
    /// Lines with an existing row at the same key and with the same bytes.
    pub already_present: usize,
    /// First sequence whose existing key has only different bytes.
    pub misaligned_sequence: Option<i64>,
}

/// An exact raw line whose old session key may belong to a subagent.
pub struct RawSessionRekeyLine {
    /// The session used by the old adapter.
    pub old_session: String,
    /// The session derived by the current adapter.
    pub new_session: String,
    /// Source line ordinal.
    pub sequence: i64,
    /// Complete source line, including its newline.
    pub bytes: Vec<u8>,
}

/// Counts from one atomic session repair.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct SessionRepairCounts {
    /// Raw rows moved to the corrected session.
    pub raw_rekeyed: usize,
    /// Misplaced rows removed because exact bytes already exist at the new key.
    pub duplicates_deleted: usize,
    /// Old occurrences removed before replay.
    pub occurrences_deleted: usize,
    /// Occurrences present for this group after replay.
    pub occurrences_rebuilt: usize,
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS canonical_traces (
    trace_id       TEXT PRIMARY KEY NOT NULL CHECK(length(trace_id) = 64),
    canonical_json BLOB NOT NULL
) STRICT;

-- `session` and `sequence` locate the complete source line. Schema version 4
-- added the raw row's own `timestamp`, so session and time selectors remain
-- exact even for a line that projects no occurrence. Schema version 5 links
-- a line to every trace it projects through `raw_record_traces`.
-- `session`/`sequence` remain nullable because a v1/v2 database's existing
-- rows predate this link and cannot be backfilled reliably (trace_id alone
-- cannot recover which occurrence a raw row belonged to when a trace has
-- more than one). `timestamp` is nullable for legacy rows without a
-- recoverable timestamp. The line key is deliberately not a foreign key to
-- `occurrences`: enforcing it would prevent the raw-only rows schema v4
-- exists to retain, and would also couple occurrence survival to the
-- forensic region in the direction the design rejects.
CREATE TABLE IF NOT EXISTS raw_carrier_records (
    raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT,
    carrier       TEXT NOT NULL CHECK(length(trim(carrier)) > 0),
    raw_bytes     BLOB NOT NULL,
    session       TEXT,
    sequence      INTEGER,
    timestamp     INTEGER
) STRICT;

-- The occurrence region (schema version 2). One row per time a trace was
-- observed. Deliberately has NO foreign key to `raw_carrier_records`, and no
-- read path in this module joins to it: occurrences must survive deletion of
-- the forensic region intact (see docs/design/trace-identity-and-occurrences.md).
CREATE TABLE IF NOT EXISTS occurrences (
    occurrence_id   INTEGER PRIMARY KEY AUTOINCREMENT,
    trace_id        TEXT NOT NULL REFERENCES canonical_traces(trace_id),
    carrier         TEXT NOT NULL CHECK(length(trim(carrier)) > 0),
    session         TEXT NOT NULL,
    sequence        INTEGER NOT NULL CHECK(sequence >= 0),
    timestamp       INTEGER NOT NULL,
    repo            TEXT NOT NULL,
    repo_source     TEXT NOT NULL
                    CHECK(repo_source IN ('git-remote', 'cwd-basename', 'explicit-override', 'none')),
    parent_sequence INTEGER,
    agent           TEXT,
    authored_by     TEXT NOT NULL
                    CHECK(authored_by IN ('human', 'agent', 'harness', 'unknown')),
    UNIQUE (carrier, session, sequence)
) STRICT;

CREATE INDEX IF NOT EXISTS occurrences_by_repo_timestamp
    ON occurrences(repo, timestamp);
"#;

const RAW_RECORD_TRACES_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS raw_record_traces (
    raw_record_id INTEGER NOT NULL REFERENCES raw_carrier_records(raw_record_id) ON DELETE CASCADE,
    trace_id TEXT NOT NULL REFERENCES canonical_traces(trace_id),
    PRIMARY KEY(raw_record_id, trace_id)
) STRICT;
CREATE INDEX IF NOT EXISTS raw_record_traces_by_trace
    ON raw_record_traces(trace_id, raw_record_id);
"#;

/// Creates the index over raw source-line lookup columns.
///
/// Run separately from [`SCHEMA`]. For a pre-v4 database, the v4 table
/// rebuild creates it after the replacement table is renamed. For a v5
/// database, this statement also restores the index if the raw table was
/// deliberately dropped and recreated by [`TraceStore::open`].
const RAW_CARRIER_LINK_INDEX: &str = r#"
CREATE INDEX IF NOT EXISTS raw_carrier_records_by_carrier_session_sequence
    ON raw_carrier_records(carrier, session, sequence);
"#;

/// The `PRAGMA user_version` this build understands, and the version it
/// stamps onto a newly created (or additively migrated) database.
///
/// This is a compile-time constant, never a caller-supplied value: `PRAGMA
/// user_version` cannot be bound as a parameter in SQLite, so the statement
/// that sets it is built with `format!`. Keeping the formatted value pinned
/// to this constant (instead of anything derived from a caller or the
/// database) is what keeps that `format!` call safe.
///
/// Schema evolution starts with (1: `canonical_traces` + `raw_carrier_records`
/// only; 2: adds `occurrences`; 3: adds nullable `session`/`sequence` link
/// columns and an index to `raw_carrier_records`; 4: makes `trace_id`
/// nullable and adds the raw row's own `timestamp`; 5: moves trace links into
/// `raw_record_traces` and stores one raw row per source line), and read behavior
/// genuinely depends on which one is on disk — [`TraceStore::log`] requires
/// the `occurrences` table, and [`TraceStore::forget_forensic`]'s `Session`
/// and `Before` selectors require the link columns to select exactly rather
/// than by over-broad trace_id. Opening a database stamped with a version
/// *higher* than this build knows (i.e. written by a future build) is
/// therefore a hard error rather than being silently accepted: this build
/// cannot know what version-dependent behavior that future schema implies,
/// so proceeding could silently give wrong answers instead of failing
/// loudly. A version from 1 up to and including `SCHEMA_VERSION` is migrated
/// forward. Version 3's link columns are first added to older tables, then
/// version 4 rebuilds the raw table transactionally because SQLite cannot
/// remove `trace_id`'s `NOT NULL` constraint in place. Version 5 rebuilds it
/// again to remove that column; each rebuild advances its marker in its own
/// transaction. A fresh database (`user_version` 0) is created directly in
/// the current shape.
/// Version 6 adds local source ownership for replaceable occurrence projections.
/// Canonical traces, raw records and their forensic links are unchanged.
/// Version 7 adds private source-revision custody and hashes recorded ownership
/// keys; legacy custody is adopted only from an observed, verified source prefix.
const SCHEMA_VERSION: u32 = 7;

// Authoritative local capture custody belongs to the private forensic region.
// It records observed source revisions, never inferred native session lineage.
const CAPTURE_EVIDENCE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS capture_evidence_generations (
    generation_id INTEGER PRIMARY KEY AUTOINCREMENT,
    carrier TEXT NOT NULL CHECK(length(trim(carrier)) > 0),
    source_key_sha256 TEXT NOT NULL,
    consumed_bytes INTEGER NOT NULL DEFAULT 0 CHECK(consumed_bytes >= 0),
    prefix_sha256 TEXT,
    confirmed INTEGER NOT NULL DEFAULT 0 CHECK(confirmed IN (0, 1))
) STRICT;
CREATE INDEX IF NOT EXISTS capture_evidence_source
    ON capture_evidence_generations(carrier, source_key_sha256, generation_id);
CREATE TABLE IF NOT EXISTS capture_evidence_records (
    generation_id INTEGER NOT NULL REFERENCES capture_evidence_generations(generation_id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    raw_record_id INTEGER NOT NULL REFERENCES raw_carrier_records(raw_record_id) ON DELETE CASCADE,
    PRIMARY KEY(generation_id, sequence)
) STRICT;
CREATE INDEX IF NOT EXISTS capture_evidence_raw
    ON capture_evidence_records(raw_record_id);
"#;

fn capture_source_digest(source_key: &str) -> String {
    format!("{:x}", Sha256::digest(source_key.as_bytes()))
}

fn capture_generation(
    connection: &Connection,
    carrier: &str,
    source_digest: &str,
) -> Result<i64, StoreError> {
    let existing = connection.query_row(
        "SELECT generation_id FROM capture_evidence_generations WHERE carrier = ?1 AND source_key_sha256 = ?2 ORDER BY generation_id DESC LIMIT 1",
        params![carrier, source_digest], |row| row.get(0),
    ).optional()?;
    match existing {
        Some(generation) => Ok(generation),
        None => {
            connection.execute("INSERT INTO capture_evidence_generations (carrier, source_key_sha256) VALUES (?1, ?2)",params![carrier, source_digest])?;
            Ok(connection.last_insert_rowid())
        }
    }
}

const CAPTURE_SOURCE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS capture_source_occurrences (
    carrier TEXT NOT NULL,
    source_key TEXT NOT NULL,
    occurrence_id INTEGER NOT NULL REFERENCES occurrences(occurrence_id) ON DELETE CASCADE,
    PRIMARY KEY(carrier, source_key, occurrence_id)
) STRICT;
CREATE INDEX IF NOT EXISTS capture_sources_by_occurrence
    ON capture_source_occurrences(occurrence_id);
"#;

/// Errors returned by the trace store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Current source custody was interrupted or its retained bytes are unavailable.
    #[error("current source capture evidence is incomplete or unavailable")]
    IncompleteCaptureEvidence,

    /// SQLite rejected an operation.
    #[error("SQLite store error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Input or persisted semantic JSON was invalid.
    #[error("invalid semantic JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// RFC 8785 canonicalization failed.
    #[error("semantic JSON canonicalization failed: {0}")]
    Canonicalization(serde_json::Error),

    /// An integer could not be represented losslessly in the canonical domain.
    #[error("integer is outside the interoperable JCS domain: {0}")]
    NumberOutsideCanonicalDomain(String),

    /// A supplied or persisted trace id was not lowercase SHA-256 hex.
    #[error("invalid trace id: {0}")]
    InvalidTraceId(String),

    /// The producing carrier label was blank.
    #[error("carrier must not be blank")]
    BlankCarrier,

    /// Existing canonical bytes disagreed with bytes having the same digest.
    #[error("content hash collision for trace {0}")]
    HashCollision(TraceId),

    /// A session repair would add semantic content that was not already captured.
    #[error("session repair would add canonical traces; transaction rolled back")]
    RepairWouldAddCanonicalTraces,

    /// A stored or supplied `repo_source` was not one of the four known bases.
    #[error("invalid repo_source: {0}")]
    InvalidRepoSource(String),

    /// A stored or supplied `authored_by` was not one of the four known values.
    #[error("invalid authored_by: {0}")]
    InvalidAuthoredBy(String),

    /// The database's `PRAGMA user_version` names a schema this build does
    /// not understand.
    #[error(
        "database schema version {found} is newer than this build supports (up to {supported})"
    )]
    UnsupportedSchemaVersion {
        /// The version recorded on disk.
        found: u32,
        /// The highest version this build understands.
        supported: u32,
    },

    /// A filesystem operation on the store file failed, e.g. tightening its
    /// permissions.
    #[error("store file I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// A single-file SQLite trace store.
pub struct TraceStore {
    connection: Connection,
    capture_source: Option<(String, String)>,
}

/// Selects forensic rows by their own provenance. Backs
/// [`TraceStore::fetch_raw_carrier_records_for_occurrences`], which in turn
/// backs `semon forensic`'s `--session` and `--day` selectors.
#[derive(Clone, Copy, Debug)]
pub enum OccurrenceSelector<'a> {
    /// Every raw record captured under this session id.
    Session(&'a str),
    /// Every raw record with `timestamp` in `[start, end)`, nanoseconds since
    /// the Unix epoch.
    TimestampRange(i64, i64),
}

/// Selects `raw_carrier_records` rows to permanently delete via
/// [`TraceStore::forget_forensic`] (backing `semon forget --forensic`).
///
/// Unlike [`OccurrenceSelector`], which only narrows an ordinary read, a
/// value of this type drives an irreversible delete, so there is
/// deliberately no "everything" variant: the CLI layer must always supply
/// exactly one of these, never none (see `docs/design/forensic-retention-and-exposure.md`
/// and issue #20 — a selector is mandatory).
#[derive(Clone, Copy, Debug)]
pub enum ForgetSelector<'a> {
    /// Every raw line containing this trace; other traces on that line remain
    /// in the semantic and occurrence regions after deletion.
    Trace(&'a TraceId),
    /// Every raw record captured under this session id.
    Session(&'a str),
    /// Every raw record whose own timestamp is strictly before this many
    /// nanoseconds since the Unix epoch (a UTC day boundary, computed with
    /// [`crate::day_bounds_ns`]).
    Before(i64),
}

/// Builds the `raw_carrier_records` WHERE-clause fragment and bindings for
/// `selector`, shared between [`TraceStore::count_forensic_forget`] (a dry
/// read) and [`TraceStore::forget_forensic`] (the actual delete), so the two
/// can never drift apart on what counts as "matching".
///
/// `Session` and `Before` select each raw row's *own* `session` and
/// `timestamp`, not through trace links. A trace-scoped selection over-deletes
/// the moment one trace recurs across sessions. `Trace` alone stays
/// trace-scoped deliberately: the caller named the projected content itself,
/// and removing every capture of it, in every session, is what naming a
/// trace means. An unprojected raw row has no trace link and therefore cannot
/// match `Trace`. A raw row with no link (written before schema version 3,
/// `session IS NULL`) and no backfilled timestamp matches neither `Session`
/// nor `Before` — see
/// [`TraceStore::count_unlinked_raw_records`], which the CLI consults so a
/// `--session`/`--before` forget never *silently* leaves such a row
/// unreported.
fn forget_where_clause(selector: ForgetSelector<'_>) -> (&'static str, Vec<Box<dyn ToSql>>) {
    match selector {
        ForgetSelector::Trace(trace_id) => (
            "raw_record_id IN (SELECT raw_record_id FROM raw_record_traces WHERE trace_id = ?1)",
            vec![Box::new(trace_id.as_str().to_owned())],
        ),
        ForgetSelector::Session(session) => ("session = ?1", vec![Box::new(session.to_owned())]),
        ForgetSelector::Before(cutoff) => ("timestamp < ?1", vec![Box::new(cutoff)]),
    }
}

impl TraceStore {
    /// Opens or creates a store at `path` and initializes its schema.
    ///
    /// The forensic region can hold prompts, responses, source code,
    /// credentials, and machine paths (see
    /// `docs/design/forensic-retention-and-exposure.md`), so the store file
    /// is tightened to owner-only permissions (`0600` on Unix) every time it
    /// is opened — at first creation and on every subsequent open — rather
    /// than trusting whatever permissions it already carries on disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        let connection = Connection::open(path)?;
        #[cfg(unix)]
        secure_store_permissions(path)?;
        Self::from_connection(connection)
    }

    /// Creates an in-memory store, primarily useful to callers' tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, StoreError> {
        // Check the on-disk marker *before* touching schema: a version this
        // build does not understand is a hard error, and it must be one
        // that runs no DDL and leaves the file exactly as found (see the
        // comment on `SCHEMA_VERSION`).
        let user_version: u32 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version > SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchemaVersion {
                found: user_version,
                supported: SCHEMA_VERSION,
            });
        }

        // `SCHEMA` creates missing regions and indexes. Against an existing
        // raw table its `CREATE TABLE IF NOT EXISTS` is a no-op, so older
        // tables still need the migrations below to reach the v5 shape.
        connection.execute_batch(SCHEMA)?;
        if user_version > 0 && user_version < 4 {
            migrate_raw_carrier_records_v4(&mut connection)?;
        }
        if user_version > 0 && user_version < 5 {
            migrate_raw_carrier_records_v5(&mut connection)?;
        }
        connection.execute_batch(RAW_CARRIER_LINK_INDEX)?;
        connection.execute_batch(RAW_RECORD_TRACES_SCHEMA)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(CAPTURE_SOURCE_SCHEMA)?;
        transaction.execute_batch(CAPTURE_EVIDENCE_SCHEMA)?;
        if user_version < 7 {
            // Ownership is derived. Keep its key reconstructable from private
            // custody without retaining the original pathname in this index.
            let sources: Vec<(String, String)> = {
                let mut statement = transaction.prepare(
                    "SELECT DISTINCT carrier, source_key FROM capture_source_occurrences",
                )?;
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<_, _>>()?
            };
            for (carrier, source_key) in sources {
                transaction.execute("UPDATE capture_source_occurrences SET source_key = ?3 WHERE carrier = ?1 AND source_key = ?2", params![carrier, source_key, capture_source_digest(&source_key)])?;
            }
        }
        if user_version < SCHEMA_VERSION {
            transaction.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }
        transaction.commit()?;

        Ok(Self {
            connection,
            capture_source: None,
        })
    }

    /// Attribute occurrence writes during this operation to one explicit local
    /// source. This is structural projection ownership, never transcript content
    /// or transferable trace identity. Nested operations restore the prior owner.
    pub fn with_capture_source<T, E>(
        &mut self,
        carrier: &str,
        source_key: &str,
        operation: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        self.with_capture_source_digest(carrier, &capture_source_digest(source_key), operation)
    }

    /// Re-establish ownership using an authoritative custody source digest.
    /// This opaque local key is not native lineage or transferable trace identity.
    /// It permits rebuilding the derived ownership index without a pathname.
    pub fn with_capture_source_digest<T, E>(
        &mut self,
        carrier: &str,
        source_digest: &str,
        operation: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let previous = self
            .capture_source
            .replace((carrier.into(), source_digest.into()));
        let result = operation(self);
        self.capture_source = previous;
        result
    }

    /// Retire this source's derived occurrences before replaying changed bytes.
    /// Other explicitly recorded owners retain their rows. Unowned legacy rows
    /// are preserved because their source cannot be established retrospectively.
    /// Canonical traces, raw records and raw-to-trace links are never deleted.
    pub fn reset_capture_source(
        &mut self,
        carrier: &str,
        source_key: &str,
    ) -> Result<(), StoreError> {
        let source_digest = capture_source_digest(source_key);
        let transaction = self.connection.savepoint()?;
        transaction.execute(
            "DELETE FROM occurrences WHERE occurrence_id IN \
             (SELECT occurrence_id FROM capture_source_occurrences WHERE carrier = ?1 AND source_key = ?2) \
             AND NOT EXISTS (SELECT 1 FROM capture_source_occurrences owners \
             WHERE owners.occurrence_id = occurrences.occurrence_id \
             AND (owners.carrier != ?1 OR owners.source_key != ?2))",
            params![carrier, source_digest],
        )?;
        transaction.execute(
            "DELETE FROM capture_source_occurrences WHERE carrier = ?1 AND source_key = ?2",
            params![carrier, source_digest],
        )?;
        transaction.execute(
            "INSERT INTO capture_evidence_generations (carrier, source_key_sha256) VALUES (?1, ?2)",
            params![carrier, source_digest],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Whether explicit source custody has been recorded, including an empty revision.
    pub fn has_capture_source_custody(
        &self,
        carrier: &str,
        source_key: &str,
    ) -> Result<bool, StoreError> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM capture_evidence_generations WHERE carrier = ?1 AND source_key_sha256 = ?2)", params![carrier, capture_source_digest(source_key)], |row| row.get(0))?)
    }

    /// Adopt a verified legacy cursor prefix using only exact already-retained rows.
    /// The adapter has just observed these bytes at this source. This never inserts
    /// missing raw bytes or guesses older revisions, so explicit forensic deletion
    /// is respected. Missing rows leave forensic replay unavailable.
    pub fn adopt_retained_capture_prefix(
        &mut self,
        carrier: &str,
        source_key: &str,
        lines: &[RawBackfillLine],
        consumed_bytes: u64,
        prefix_sha256: &str,
    ) -> Result<(), StoreError> {
        let mut digest = Sha256::new();
        let mut total = 0u64;
        for line in lines {
            digest.update(&line.bytes);
            total += line.bytes.len() as u64;
        }
        if total != consumed_bytes || format!("{:x}", digest.finalize()) != prefix_sha256 {
            return Err(StoreError::IncompleteCaptureEvidence);
        }
        let consumed_bytes =
            i64::try_from(consumed_bytes).map_err(|_| StoreError::IncompleteCaptureEvidence)?;
        let transaction = self.connection.savepoint()?;
        let generation =
            capture_generation(&transaction, carrier, &capture_source_digest(source_key))?;
        for line in lines {
            let raw: Option<i64> = transaction.query_row("SELECT raw_record_id FROM raw_carrier_records WHERE carrier = ?1 AND session = ?2 AND sequence = ?3 AND raw_bytes = ?4", params![carrier, line.session, line.sequence, line.bytes], |row| row.get(0)).optional()?;
            if let Some(raw) = raw {
                transaction.execute("INSERT OR IGNORE INTO capture_evidence_records (generation_id, sequence, raw_record_id) VALUES (?1, ?2, ?3)", params![generation, line.sequence, raw])?;
            }
        }
        transaction.execute("UPDATE capture_evidence_generations SET consumed_bytes = ?2, prefix_sha256 = ?3, confirmed = 1 WHERE generation_id = ?1", params![generation, consumed_bytes, prefix_sha256])?;
        transaction.commit()?;
        Ok(())
    }

    /// Whether the cursor agrees with a confirmed local capture checkpoint.
    /// A missing checkpoint never retroactively guesses custody from raw rows.
    /// Deliberate raw deletion does not cause already consumed bytes to be recaptured.
    pub fn capture_source_cursor_matches(
        &self,
        carrier: &str,
        source_key: &str,
        consumed_bytes: u64,
        prefix_sha256: &str,
    ) -> Result<bool, StoreError> {
        let checkpoint: Option<(i64, Option<String>, bool)> = self.connection.query_row(
            "SELECT consumed_bytes, prefix_sha256, confirmed FROM capture_evidence_generations WHERE carrier = ?1 AND source_key_sha256 = ?2 ORDER BY generation_id DESC LIMIT 1",
            params![carrier, capture_source_digest(source_key)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        match checkpoint {
            Some((bytes, digest, confirmed)) => Ok(confirmed
                && u64::try_from(bytes).ok() == Some(consumed_bytes)
                && digest.as_deref() == Some(prefix_sha256)),
            None if consumed_bytes == 0 => {
                let owned: bool = self.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM capture_source_occurrences WHERE carrier = ?1 AND source_key = ?2)", params![carrier, capture_source_digest(source_key)], |row| row.get(0),
                )?;
                Ok(!owned)
            }
            None => Ok(false),
        }
    }

    /// Confirm the exact complete source prefix consumed by this capture.
    /// This is collector provenance; its digest is not a native relationship ID.
    pub fn checkpoint_capture_source(
        &mut self,
        carrier: &str,
        source_key: &str,
        consumed_bytes: u64,
        prefix_sha256: &str,
    ) -> Result<(), StoreError> {
        let consumed_bytes =
            i64::try_from(consumed_bytes).map_err(|_| StoreError::IncompleteCaptureEvidence)?;
        let transaction = self.connection.savepoint()?;
        let generation =
            capture_generation(&transaction, carrier, &capture_source_digest(source_key))?;
        transaction.execute(
            "UPDATE capture_evidence_generations SET consumed_bytes = ?2, prefix_sha256 = ?3, confirmed = 1 WHERE generation_id = ?1 AND (confirmed = 0 OR consumed_bytes != ?2 OR prefix_sha256 IS NOT ?3)",
            params![generation, consumed_bytes, prefix_sha256],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Explicit forensic access to the current source's retained complete prefix.
    /// Selects recorded custody, not insertion order or similarity. Missing legacy
    /// custody is unknown; interrupted checkpoints or forgotten bytes fail closed.
    /// Older generations and their raw rows remain retained independently.
    pub fn fetch_current_capture_source_raw(
        &self,
        carrier: &str,
        source_key: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let checkpoint: Option<(i64, i64, Option<String>, bool)> = self.connection.query_row(
            "SELECT generation_id, consumed_bytes, prefix_sha256, confirmed FROM capture_evidence_generations WHERE carrier = ?1 AND source_key_sha256 = ?2 ORDER BY generation_id DESC LIMIT 1",
            params![carrier, capture_source_digest(source_key)], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).optional()?;
        let Some((generation, expected_bytes, digest, confirmed)) = checkpoint else {
            return Ok(None);
        };
        if !confirmed {
            return Err(StoreError::IncompleteCaptureEvidence);
        }
        let mut statement = self.connection.prepare(
            "SELECT r.raw_bytes FROM capture_evidence_records evidence JOIN raw_carrier_records r ON r.raw_record_id = evidence.raw_record_id WHERE evidence.generation_id = ?1 ORDER BY evidence.sequence",
        )?;
        let mut bytes = Vec::new();
        for record in statement.query_map([generation], |row| row.get::<_, Vec<u8>>(0))? {
            bytes.extend(record?);
        }
        if i64::try_from(bytes.len()).ok() != Some(expected_bytes)
            || digest.as_deref() != Some(format!("{:x}", Sha256::digest(&bytes)).as_str())
        {
            return Err(StoreError::IncompleteCaptureEvidence);
        }
        Ok(Some(bytes))
    }

    /// Captures one projected block while preserving the existing one-block API.
    pub fn capture(
        &mut self,
        semantic_core: &SemanticCore,
        raw_record: NewRawCarrierRecord<'_>,
        occurrence: NewOccurrence<'_>,
    ) -> Result<CaptureResult, StoreError> {
        let (_, mut results) = self.capture_line(
            raw_record,
            occurrence.session,
            occurrence.sequence,
            occurrence.timestamp,
            &[(semantic_core.clone(), occurrence)],
        )?;
        Ok(results.remove(0))
    }

    /// Atomically captures one source line and all its projected blocks.
    ///
    /// The raw row is reused when its carrier, session, line sequence and
    /// bytes match an existing row. Each block keeps its own occurrence
    /// sequence. The returned id belongs to the shared raw row; results are
    /// in block order and report each trace id and canonical insert status.
    pub fn capture_line(
        &mut self,
        raw_record: NewRawCarrierRecord<'_>,
        session: &str,
        line_sequence: i64,
        timestamp: i64,
        blocks: &[(SemanticCore, NewOccurrence<'_>)],
    ) -> Result<(i64, Vec<CaptureResult>), StoreError> {
        if raw_record.carrier.trim().is_empty() {
            return Err(StoreError::BlankCarrier);
        }

        let transaction = self.connection.savepoint()?;
        let (raw_record_id, _) =
            resolve_raw_record(&transaction, raw_record, session, line_sequence, timestamp)?;
        if let Some((carrier, source_key)) = &self.capture_source
            && carrier == raw_record.carrier
        {
            let generation = capture_generation(&transaction, carrier, source_key)?;
            transaction.execute(
                "INSERT INTO capture_evidence_records (generation_id, sequence, raw_record_id) VALUES (?1, ?2, ?3) ON CONFLICT(generation_id, sequence) DO UPDATE SET raw_record_id = excluded.raw_record_id",
                params![generation, line_sequence, raw_record_id],
            )?;
            transaction.execute(
                "UPDATE capture_evidence_generations SET confirmed = 0 WHERE generation_id = ?1",
                [generation],
            )?;
        }
        let mut results = Vec::with_capacity(blocks.len());

        for (semantic_core, occurrence) in blocks {
            let canonical_json = semantic_core.canonical_json()?;
            let trace_id = canonical::content_hash(&canonical_json);

            let canonical_inserted = transaction.execute(
                "INSERT INTO canonical_traces (trace_id, canonical_json) VALUES (?1, ?2) \
             ON CONFLICT(trace_id) DO NOTHING",
                params![trace_id.as_str(), &canonical_json],
            )? == 1;

            if !canonical_inserted {
                let existing: Vec<u8> = transaction.query_row(
                    "SELECT canonical_json FROM canonical_traces WHERE trace_id = ?1",
                    [trace_id.as_str()],
                    |row| row.get(0),
                )?;
                if existing != canonical_json {
                    return Err(StoreError::HashCollision(trace_id));
                }
            }

            transaction.execute(
                "INSERT INTO occurrences \
                 (trace_id, carrier, session, sequence, timestamp, repo, repo_source, \
                  parent_sequence, agent, authored_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(carrier, session, sequence) DO UPDATE SET \
                 trace_id = excluded.trace_id, \
                 timestamp = excluded.timestamp, \
                 repo = excluded.repo, \
                 repo_source = excluded.repo_source, \
                 parent_sequence = excluded.parent_sequence, \
                 agent = excluded.agent, \
                 authored_by = excluded.authored_by",
                params![
                    trace_id.as_str(),
                    raw_record.carrier,
                    occurrence.session,
                    occurrence.sequence,
                    occurrence.timestamp,
                    occurrence.repo,
                    occurrence.repo_source.as_str(),
                    occurrence.parent_sequence,
                    occurrence.agent,
                    occurrence.authored_by.as_str(),
                ],
            )?;
            if let Some((carrier, source_key)) = &self.capture_source
                && carrier == raw_record.carrier
            {
                transaction.execute(
                    "INSERT OR IGNORE INTO capture_source_occurrences (carrier, source_key, occurrence_id) \
                     SELECT ?1, ?2, occurrence_id FROM occurrences \
                     WHERE carrier = ?1 AND session = ?3 AND sequence = ?4",
                    params![carrier, source_key, occurrence.session, occurrence.sequence],
                )?;
            }
            transaction.execute(
                "INSERT OR IGNORE INTO raw_record_traces (raw_record_id, trace_id) VALUES (?1, ?2)",
                params![raw_record_id, trace_id.as_str()],
            )?;
            results.push(CaptureResult::new(
                trace_id,
                canonical_inserted,
                raw_record_id,
            ));
        }

        transaction.commit()?;
        Ok((raw_record_id, results))
    }

    /// Repairs exact raw keys and rebuilds one carrier session family atomically.
    /// A dry run executes the same replay and rolls it back.
    pub fn repair_session_keys<E, F>(
        &mut self,
        carrier: &str,
        parent: &str,
        lines: &[RawSessionRekeyLine],
        dry_run: bool,
        replay: F,
    ) -> Result<SessionRepairCounts, E>
    where
        E: From<StoreError>,
        F: FnOnce(&mut Self) -> Result<(), E>,
    {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(StoreError::from)?;
        let result = (|| {
            let mut counts = SessionRepairCounts::default();
            let canonical_before: i64 = self
                .connection
                .query_row("SELECT count(*) FROM canonical_traces", [], |row| {
                    row.get(0)
                })
                .map_err(StoreError::from)?;
            for line in lines {
                let ids = {
                    let mut statement = self
                        .connection
                        .prepare(
                            "SELECT raw_record_id FROM raw_carrier_records \
                         WHERE carrier = ?1 AND session = ?2 AND sequence = ?3 AND raw_bytes = ?4 \
                         ORDER BY raw_record_id",
                        )
                        .map_err(StoreError::from)?;
                    statement
                        .query_map(
                            params![carrier, line.old_session, line.sequence, line.bytes],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(StoreError::from)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(StoreError::from)?
                };
                for id in ids {
                    let duplicate = self
                        .connection
                        .query_row(
                            "SELECT raw_record_id FROM raw_carrier_records \
                         WHERE carrier = ?1 AND session = ?2 AND sequence = ?3 AND raw_bytes = ?4 \
                         ORDER BY raw_record_id LIMIT 1",
                            params![carrier, line.new_session, line.sequence, line.bytes],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(StoreError::from)?;
                    if let Some(keep_id) = duplicate {
                        self.connection.execute(
                            "UPDATE capture_evidence_records SET raw_record_id = ?1 WHERE raw_record_id = ?2",
                            params![keep_id, id],
                        ).map_err(StoreError::from)?;
                        self.connection.execute(
                            "INSERT OR IGNORE INTO raw_record_traces (raw_record_id, trace_id) SELECT ?1, trace_id FROM raw_record_traces WHERE raw_record_id = ?2",
                            params![keep_id, id],
                        ).map_err(StoreError::from)?;
                        self.connection
                            .execute(
                                "DELETE FROM raw_carrier_records WHERE raw_record_id = ?1",
                                [id],
                            )
                            .map_err(StoreError::from)?;
                        counts.duplicates_deleted += 1;
                    } else {
                        self.connection.execute("UPDATE raw_carrier_records SET session = ?1 WHERE raw_record_id = ?2", params![line.new_session, id]).map_err(StoreError::from)?;
                        counts.raw_rekeyed += 1;
                    }
                }
            }
            let prefix = format!("{parent}/agent-");
            let predicate =
                "carrier = ?1 AND (session = ?2 OR substr(session, 1, length(?3)) = ?3)";
            counts.occurrences_deleted = self
                .connection
                .query_row(
                    &format!("SELECT count(*) FROM occurrences WHERE {predicate}"),
                    params![carrier, parent, prefix],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(StoreError::from)? as usize;
            self.connection
                .execute(
                    &format!("DELETE FROM occurrences WHERE {predicate}"),
                    params![carrier, parent, prefix],
                )
                .map_err(StoreError::from)?;
            replay(self)?;
            let canonical_after: i64 = self
                .connection
                .query_row("SELECT count(*) FROM canonical_traces", [], |row| {
                    row.get(0)
                })
                .map_err(StoreError::from)?;
            if canonical_after != canonical_before {
                return Err(StoreError::RepairWouldAddCanonicalTraces.into());
            }
            counts.occurrences_rebuilt = self
                .connection
                .query_row(
                    &format!("SELECT count(*) FROM occurrences WHERE {predicate}"),
                    params![carrier, parent, prefix],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(StoreError::from)? as usize;
            Ok(counts)
        })();
        if dry_run || result.is_err() {
            self.connection
                .execute_batch("ROLLBACK")
                .map_err(StoreError::from)?;
        } else {
            self.connection
                .execute_batch("COMMIT")
                .map_err(StoreError::from)?;
        }
        result
    }

    /// Captures one complete carrier record that has no transferable
    /// semantic projection.
    ///
    /// This writes exactly one forensic row with no trace links. It does
    /// not write `canonical_traces` or `occurrences`, so ordinary trace and
    /// log reads remain unable to observe the line. `session`, `sequence`,
    /// and `timestamp` are stored on the raw row itself so the forensic
    /// session/day selectors and their `forget` counterparts can still reach
    /// it.
    ///
    /// Returns the forensic record's local row identifier, reused on replay.
    pub fn capture_raw_only(
        &mut self,
        carrier: &str,
        raw_bytes: &[u8],
        session: &str,
        sequence: i64,
        timestamp: i64,
    ) -> Result<i64, StoreError> {
        let (id, _) = self.capture_line(
            NewRawCarrierRecord::new(carrier, raw_bytes),
            session,
            sequence,
            timestamp,
            &[],
        )?;
        Ok(id)
    }

    /// Checks every line against existing raw rows before inserting any of them.
    ///
    /// A conflicting key skips the whole file. Validation and insertion share
    /// one transaction, so a failed insert rolls back the entire file.
    pub fn backfill_raw_lines(
        &mut self,
        carrier: &str,
        lines: &[RawBackfillLine],
        dry_run: bool,
    ) -> Result<RawBackfillResult, StoreError> {
        if carrier.trim().is_empty() {
            return Err(StoreError::BlankCarrier);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut result = RawBackfillResult::default();
        let mut candidates = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let mut statement = transaction.prepare(
                "SELECT raw_bytes FROM raw_carrier_records \
                 WHERE carrier = ?1 AND session = ?2 AND sequence = ?3",
            )?;
            let mut rows = statement.query(params![carrier, line.session, line.sequence])?;
            let mut found_key = false;
            let mut found_bytes = false;
            while let Some(row) = rows.next()? {
                found_key = true;
                if row.get::<_, Vec<u8>>(0)? == line.bytes {
                    found_bytes = true;
                }
            }
            if found_bytes {
                result.already_present += 1;
            } else if found_key {
                result.misaligned_sequence.get_or_insert(line.sequence);
            } else {
                candidates.push(index);
            }
        }
        if result.misaligned_sequence.is_some() {
            return Ok(result);
        }
        if dry_run {
            result.inserted = candidates.len();
        } else {
            for index in candidates {
                let line = &lines[index];
                let (_, inserted) = resolve_raw_record(
                    &transaction,
                    NewRawCarrierRecord::new(carrier, &line.bytes),
                    &line.session,
                    line.sequence,
                    line.timestamp,
                )?;
                if inserted {
                    result.inserted += 1;
                } else {
                    result.already_present += 1;
                }
            }
            transaction.commit()?;
        }
        Ok(result)
    }

    /// Fetches a canonical trace by semantic content hash.
    ///
    /// This ordinary read queries only `canonical_traces` and its return type
    /// cannot contain raw carrier bytes.
    pub fn fetch_trace(&self, trace_id: &TraceId) -> Result<Option<CanonicalTrace>, StoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT trace_id, canonical_json FROM canonical_traces WHERE trace_id = ?1",
                [trace_id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?;

        row.map(decode_canonical_trace).transpose()
    }

    /// Lists canonical traces after an optional exclusive cursor.
    ///
    /// Results use ascending content-hash order, giving deterministic keyset
    /// pagination without implying that capture time participates in identity.
    /// A zero limit returns an empty page. This ordinary read queries only
    /// `canonical_traces` and never returns raw carrier bytes.
    pub fn list_traces(
        &self,
        after: Option<&TraceId>,
        limit: u32,
    ) -> Result<Vec<CanonicalTrace>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        if let Some(after) = after {
            let mut statement = self.connection.prepare(
                "SELECT trace_id, canonical_json FROM canonical_traces \
                 WHERE trace_id > ?1 ORDER BY trace_id ASC LIMIT ?2",
            )?;
            let mapped = statement.query_map(params![after.as_str(), i64::from(limit)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            for row in mapped {
                rows.push(row?);
            }
        } else {
            let mut statement = self.connection.prepare(
                "SELECT trace_id, canonical_json FROM canonical_traces \
                 ORDER BY trace_id ASC LIMIT ?1",
            )?;
            let mapped = statement.query_map([i64::from(limit)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            for row in mapped {
                rows.push(row?);
            }
        }

        rows.into_iter().map(decode_canonical_trace).collect()
    }

    /// Explicitly fetches forensic records for one canonical trace.
    ///
    /// This read path queries `raw_carrier_records` explicitly.
    /// Records are ordered by ascending local row id (capture insertion order).
    pub fn fetch_raw_carrier_records(
        &self,
        trace_id: &TraceId,
    ) -> Result<Vec<RawCarrierRecord>, StoreError> {
        self.fetch_raw_records(
            "r.raw_record_id IN (SELECT raw_record_id FROM raw_record_traces WHERE trace_id = ?1)",
            &[&trace_id.as_str()],
        )
    }

    /// Explicitly fetches forensic records whose own provenance matches
    /// `selector`.
    ///
    /// This is the bulk counterpart to [`TraceStore::fetch_raw_carrier_records`].
    /// It backs `semon forensic`'s `--session` and `--day` selectors (see
    /// `docs/design/forensic-retention-and-exposure.md`, Decision 3).
    ///
    /// This selects each raw row's own `session` or `timestamp` directly,
    /// including rows that have no trace or occurrence. It never first
    /// collects matching traces and then fetches every raw row that shares
    /// one's `trace_id`. That older approach over-read: a trace
    /// captured under two sessions would have `--session A` return its raw
    /// record from session B too, since both rows share one `trace_id`.
    /// Reading too much is not the same failure as forgetting too much, but
    /// it is the same bug, on the read side of the exact code path
    /// [`TraceStore::forget_forensic`]'s delete side had (see that method's
    /// selector, [`ForgetSelector`], and `forget_where_clause`'s doc
    /// comment for the measurement that caught it).
    ///
    /// A raw row written before schema version 3 has no link (`session IS
    /// NULL`) and matches neither selector here, the same as in
    /// `forget_where_clause` — see
    /// [`TraceStore::count_unlinked_raw_records`].
    ///
    /// Emitted records are ordered by ascending `raw_record_id` — capture
    /// (insertion) order.
    pub fn fetch_raw_carrier_records_for_occurrences(
        &self,
        selector: OccurrenceSelector<'_>,
    ) -> Result<Vec<RawCarrierRecord>, StoreError> {
        let (clause, bindings): (&str, Vec<Box<dyn ToSql>>) = match selector {
            OccurrenceSelector::Session(session) => {
                ("r.session = ?1", vec![Box::new(session.to_owned())])
            }
            OccurrenceSelector::TimestampRange(start, end) => (
                "r.timestamp >= ?1 AND r.timestamp < ?2",
                vec![Box::new(start), Box::new(end)],
            ),
        };

        let params: Vec<&dyn ToSql> = bindings.iter().map(AsRef::as_ref).collect();
        self.fetch_raw_records(clause, &params)
    }

    fn fetch_raw_records(
        &self,
        clause: &str,
        bindings: &[&dyn ToSql],
    ) -> Result<Vec<RawCarrierRecord>, StoreError> {
        let sql = format!(
            "SELECT r.raw_record_id, r.carrier, r.raw_bytes, r.session, r.sequence, r.timestamp, \
                    COALESCE((SELECT group_concat(trace_id, ',') FROM \
                        (SELECT trace_id FROM raw_record_traces \
                         WHERE raw_record_id = r.raw_record_id ORDER BY trace_id)), '') \
             FROM raw_carrier_records r WHERE {clause} ORDER BY r.raw_record_id ASC"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mapped = statement.query_map(bindings, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;

        let mut records = Vec::new();
        for row in mapped {
            let (id, carrier, bytes, session, sequence, timestamp, trace_ids) = row?;
            records.push(RawCarrierRecord::new(
                id,
                trace_ids
                    .split(',')
                    .filter(|id| !id.is_empty())
                    .map(TraceId::from_str)
                    .collect::<Result<Vec<_>, _>>()?,
                carrier,
                bytes,
                session,
                sequence,
                timestamp,
            ));
        }
        Ok(records)
    }

    /// Renders the occurrence log: occurrences joined to canonical traces,
    /// ordered by `(session, sequence)`.
    ///
    /// This is an ordinary read. It queries only `occurrences` and
    /// `canonical_traces` — never `raw_carrier_records`, directly or
    /// transitively — so the log renders identically whether or not the
    /// forensic region still exists.
    pub fn log(&self, filter: &LogFilter) -> Result<Vec<OccurrenceRecord>, StoreError> {
        let mut sql = String::from(
            "SELECT o.trace_id, o.carrier, o.session, o.sequence, o.timestamp, o.repo, \
                    o.repo_source, o.parent_sequence, o.agent, o.authored_by, c.canonical_json \
             FROM occurrences o JOIN canonical_traces c ON c.trace_id = o.trace_id \
             WHERE 1 = 1",
        );
        let mut bindings: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(repo) = filter.repo.as_deref() {
            sql.push_str(" AND o.repo = ?");
            bindings.push(Box::new(repo.to_owned()));
        }
        if let Some((start, end)) = filter.timestamp_range {
            sql.push_str(" AND o.timestamp >= ? AND o.timestamp < ?");
            bindings.push(Box::new(start));
            bindings.push(Box::new(end));
        }
        sql.push_str(" ORDER BY o.session ASC, o.sequence ASC");
        if let Some(limit) = filter.limit {
            sql.push_str(" LIMIT ?");
            bindings.push(Box::new(i64::from(limit)));
        }

        let mut statement = self.connection.prepare(&sql)?;
        let params: Vec<&dyn ToSql> = bindings.iter().map(AsRef::as_ref).collect();
        let mapped = statement.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Vec<u8>>(10)?,
            ))
        })?;

        let mut records = Vec::new();
        for row in mapped {
            let (
                trace_id,
                carrier,
                session,
                sequence,
                timestamp,
                repo,
                repo_source,
                parent_sequence,
                agent,
                authored_by,
                canonical_json,
            ) = row?;
            records.push(OccurrenceRecord {
                trace_id: TraceId::from_str(&trace_id)?,
                carrier,
                session,
                sequence,
                timestamp,
                repo,
                repo_source: RepoSource::from_str(&repo_source)?,
                parent_sequence,
                agent,
                authored_by: AuthoredBy::from_str(&authored_by)?,
                semantic_core: SemanticCore::from_json_slice(&canonical_json)?,
            });
        }
        Ok(records)
    }

    /// The inclusive `[earliest, latest]` occurrence timestamp (nanoseconds
    /// since the Unix epoch), optionally restricted to one `repo`, or `None`
    /// when no occurrence matches. Used to turn an empty `--day`-filtered
    /// `semon log` into a hint naming the days the store actually covers,
    /// rather than a bare "no occurrences" that can't be told apart from a
    /// genuinely quiet day.
    ///
    /// Like [`TraceStore::log`], this is an ordinary read over `occurrences`
    /// alone — it never touches `raw_carrier_records`.
    pub fn timestamp_span(&self, repo: Option<&str>) -> Result<Option<(i64, i64)>, StoreError> {
        let mut sql = String::from("SELECT MIN(timestamp), MAX(timestamp) FROM occurrences");
        let mut bindings: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(repo) = repo {
            sql.push_str(" WHERE repo = ?");
            bindings.push(Box::new(repo.to_owned()));
        }
        let params: Vec<&dyn ToSql> = bindings.iter().map(AsRef::as_ref).collect();
        let (min, max): (Option<i64>, Option<i64>) =
            self.connection.query_row(&sql, params.as_slice(), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        Ok(min.zip(max))
    }

    /// Counts how many `raw_carrier_records` rows match `selector`, without
    /// deleting anything.
    ///
    /// This is a dry read used to build the confirmation prompt for `semon
    /// forget --forensic` before anything irreversible happens — the
    /// operator sees the exact count [`TraceStore::forget_forensic`] would
    /// delete, computed by the identical WHERE clause.
    pub fn count_forensic_forget(&self, selector: ForgetSelector<'_>) -> Result<u64, StoreError> {
        let (clause, bindings) = forget_where_clause(selector);
        let sql = format!("SELECT COUNT(*) FROM raw_carrier_records WHERE {clause}");
        let params: Vec<&dyn ToSql> = bindings.iter().map(AsRef::as_ref).collect();
        let count: i64 = self
            .connection
            .query_row(&sql, params.as_slice(), |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    /// Permanently deletes `raw_carrier_records` rows matching `selector`.
    ///
    /// `canonical_traces` and `occurrences` are never touched by this
    /// method — it is the deletion counterpart to
    /// [`TraceStore::fetch_raw_carrier_records`] and
    /// [`TraceStore::fetch_raw_carrier_records_for_occurrences`], and its
    /// purpose is to make reachable the state
    /// `docs/design/trace-identity-and-occurrences.md` designed the
    /// occurrence region around: a store whose forensic region is gone but
    /// whose log is intact.
    ///
    /// A plain SQLite `DELETE` does not remove bytes from the file: deleted
    /// rows go onto the freelist and stay byte-for-byte readable on disk
    /// until their pages are reused or the file is vacuumed, which would be
    /// theatre for a command whose entire purpose is removing credentials
    /// and source text. So this method runs the delete under `PRAGMA
    /// secure_delete = ON` — SQLite overwrites deleted content with zeroes
    /// before the page is freed — and then `VACUUM`s the connection, which
    /// rewrites the database file from scratch without the freed pages at
    /// all. Either alone would already remove the bytes from the live file;
    /// doing both is defense in depth against relying on one mechanism's
    /// fine print.
    ///
    /// Returns the number of rows deleted.
    pub fn forget_forensic(&mut self, selector: ForgetSelector<'_>) -> Result<u64, StoreError> {
        let (clause, bindings) = forget_where_clause(selector);

        // Not wrapped in an explicit transaction: VACUUM refuses to run
        // inside one, and a single DELETE statement is already atomic on
        // its own in SQLite's default autocommit mode.
        self.connection
            .execute_batch("PRAGMA secure_delete = ON;")?;
        let sql = format!("DELETE FROM raw_carrier_records WHERE {clause}");
        let params: Vec<&dyn ToSql> = bindings.iter().map(AsRef::as_ref).collect();
        let deleted = self.connection.execute(&sql, params.as_slice())?;
        self.connection.execute_batch("VACUUM;")?;

        Ok(deleted as u64)
    }

    /// Counts `raw_carrier_records` rows with no `session`/`sequence` link —
    /// rows written by a build before schema version 3, which also have no
    /// timestamp after the schema-v4 backfill and which
    /// [`ForgetSelector::Session`] and [`ForgetSelector::Before`] cannot
    /// select (only [`ForgetSelector::Trace`] can still reach them, since it
    /// follows `raw_record_traces` rather than the provenance columns).
    ///
    /// This exists so `semon forget --forensic --session`/`--before` can
    /// report such rows rather than silently completing as though nothing
    /// was missed: a v1/v2 database's existing raw rows cannot be
    /// backfilled with a link, because `trace_id` alone cannot recover which
    /// occurrence a given raw row belonged to once a trace has more than
    /// one.
    pub fn count_unlinked_raw_records(&self) -> Result<u64, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM raw_carrier_records WHERE session IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }
}

/// Resolves a raw row by its source key and exact bytes. A rewritten line at
/// the same key deliberately inserts another row.
fn resolve_raw_record(
    transaction: &Connection,
    raw: NewRawCarrierRecord<'_>,
    session: &str,
    sequence: i64,
    timestamp: i64,
) -> Result<(i64, bool), StoreError> {
    let existing = transaction
        .query_row(
            "SELECT raw_record_id FROM raw_carrier_records \
             WHERE carrier = ?1 AND session = ?2 AND sequence = ?3 AND raw_bytes = ?4 \
             ORDER BY raw_record_id LIMIT 1",
            params![raw.carrier, session, sequence, raw.bytes],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok((id, false));
    }
    transaction.execute(
        "INSERT INTO raw_carrier_records (carrier, raw_bytes, session, sequence, timestamp) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![raw.carrier, raw.bytes, session, sequence, timestamp],
    )?;
    Ok((transaction.last_insert_rowid(), true))
}

/// Migrates the forensic region to schema version 4 in one transaction.
///
/// SQLite cannot remove `trace_id`'s `NOT NULL` constraint in place, so the
/// migration creates the replacement table, copies every row with its local
/// id intact, backfills each linked row's timestamp from its exact occurrence,
/// drops the old table, renames the replacement, recreates both indexes, and
/// advances `user_version` before committing. A failure at any step leaves
/// the v3 table and version marker intact.
fn migrate_raw_carrier_records_v4(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // A v1/v2 table lacks the link columns named by the copy and backfill.
    // Add them inside the same transaction before rebuilding. On a fresh or
    // v3 database this is an idempotent no-op.
    ensure_raw_carrier_link_columns(&transaction)?;
    transaction.execute_batch(
        "CREATE TABLE raw_carrier_records_v4 ( \
             raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT, \
             trace_id      TEXT REFERENCES canonical_traces(trace_id), \
             carrier       TEXT NOT NULL CHECK(length(trim(carrier)) > 0), \
             raw_bytes     BLOB NOT NULL, \
             session       TEXT, \
             sequence      INTEGER, \
             timestamp     INTEGER \
         ) STRICT; \
         INSERT INTO raw_carrier_records_v4 \
             (raw_record_id, trace_id, carrier, raw_bytes, session, sequence, timestamp) \
         SELECT raw_record_id, trace_id, carrier, raw_bytes, session, sequence, NULL \
         FROM raw_carrier_records; \
         UPDATE raw_carrier_records_v4 AS r \
         SET timestamp = o.timestamp \
         FROM occurrences AS o \
         WHERE o.carrier = r.carrier \
           AND o.session = r.session \
           AND o.sequence = r.sequence; \
         DROP TABLE raw_carrier_records; \
         ALTER TABLE raw_carrier_records_v4 RENAME TO raw_carrier_records; \
         CREATE INDEX raw_carrier_records_by_trace \
             ON raw_carrier_records(trace_id, raw_record_id);",
    )?;
    transaction.execute_batch(RAW_CARRIER_LINK_INDEX)?;
    transaction.execute_batch("PRAGMA user_version = 4;")?;
    transaction.commit()?;
    Ok(())
}

/// Rebuilds the forensic table without `trace_id`, preserving every old link
/// before merging byte-identical Claude rows from the same source line.
fn migrate_raw_carrier_records_v5(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TEMP TABLE raw_links_v5 AS \
             SELECT raw_record_id, trace_id FROM raw_carrier_records WHERE trace_id IS NOT NULL; \
         CREATE TABLE raw_carrier_records_v5 ( \
             raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT, \
             carrier TEXT NOT NULL CHECK(length(trim(carrier)) > 0), \
             raw_bytes BLOB NOT NULL, \
             session TEXT, sequence INTEGER, timestamp INTEGER \
         ) STRICT; \
         INSERT INTO raw_carrier_records_v5 \
             (raw_record_id, carrier, raw_bytes, session, sequence, timestamp) \
         SELECT raw_record_id, carrier, raw_bytes, session, sequence, timestamp \
         FROM raw_carrier_records; \
         DROP TABLE raw_carrier_records; \
         ALTER TABLE raw_carrier_records_v5 RENAME TO raw_carrier_records; \
         DROP INDEX IF EXISTS raw_carrier_records_by_trace; \
         CREATE INDEX raw_carrier_records_by_carrier_session_sequence \
             ON raw_carrier_records(carrier, session, sequence);",
    )?;
    transaction.execute_batch(RAW_RECORD_TRACES_SCHEMA)?;
    transaction.execute_batch(
        "INSERT INTO raw_record_traces (raw_record_id, trace_id) \
         SELECT raw_record_id, trace_id FROM raw_links_v5; \
         DROP TABLE raw_links_v5;",
    )?;

    // Snapshot the mapping before changing any sequence or deleting rows.
    let groups = {
        let mut statement = transaction.prepare(&format!(
            "SELECT r.raw_record_id, \
                 (SELECT MIN(k.raw_record_id) FROM raw_carrier_records k \
                  WHERE k.carrier = 'claude' AND k.session = r.session \
                    AND k.sequence >= (r.sequence / {CLAUDE_MAX_BLOCKS_PER_RECORD}) * {CLAUDE_MAX_BLOCKS_PER_RECORD} \
                    AND k.sequence < (r.sequence / {CLAUDE_MAX_BLOCKS_PER_RECORD} + 1) * {CLAUDE_MAX_BLOCKS_PER_RECORD} \
                    AND k.raw_bytes = r.raw_bytes), \
                 (r.sequence / {CLAUDE_MAX_BLOCKS_PER_RECORD}) * {CLAUDE_MAX_BLOCKS_PER_RECORD} \
             FROM raw_carrier_records r \
             WHERE r.carrier = 'claude' AND r.session IS NOT NULL \
               AND r.sequence IS NOT NULL AND r.sequence >= 0 \
             ORDER BY r.raw_record_id"
        ))?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (id, keep_id, _) in &groups {
        if id != keep_id {
            transaction.execute(
                "INSERT OR IGNORE INTO raw_record_traces (raw_record_id, trace_id) \
                 SELECT ?1, trace_id FROM raw_record_traces WHERE raw_record_id = ?2",
                params![keep_id, id],
            )?;
            transaction.execute(
                "DELETE FROM raw_carrier_records WHERE raw_record_id = ?1",
                [id],
            )?;
        }
    }
    for (id, keep_id, line_sequence) in groups {
        if id == keep_id {
            transaction.execute(
                "UPDATE raw_carrier_records SET sequence = ?1 WHERE raw_record_id = ?2",
                params![line_sequence, id],
            )?;
        }
    }
    transaction.execute_batch("PRAGMA user_version = 5;")?;
    transaction.commit()?;
    Ok(())
}

/// Adds the schema-version-3 `session`/`sequence` link columns to an
/// existing `raw_carrier_records` table that predates them.
///
/// A fresh database's `raw_carrier_records` already has these columns —
/// they're part of [`SCHEMA`]'s own `CREATE TABLE` — so this only ever does
/// anything against a v1/v2 database, where `CREATE TABLE IF NOT EXISTS`
/// no-ops because the table already exists. SQLite has no conditional `ALTER
/// TABLE ADD COLUMN IF NOT EXISTS`, so this checks each column's presence
/// via `PRAGMA table_info` first and only runs the `ALTER TABLE` that column
/// is actually missing, making the operation safe if a migration attempt is
/// retried.
fn ensure_raw_carrier_link_columns(connection: &Connection) -> Result<(), StoreError> {
    let mut has_session = false;
    let mut has_sequence = false;
    {
        let mut statement = connection.prepare("PRAGMA table_info(raw_carrier_records)")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            match row.get::<_, String>(1)?.as_str() {
                "session" => has_session = true,
                "sequence" => has_sequence = true,
                _ => {}
            }
        }
    }
    if !has_session {
        connection.execute_batch("ALTER TABLE raw_carrier_records ADD COLUMN session TEXT;")?;
    }
    if !has_sequence {
        connection.execute_batch("ALTER TABLE raw_carrier_records ADD COLUMN sequence INTEGER;")?;
    }
    Ok(())
}

/// Tightens the store file to owner-only read/write (`0600`), regardless of
/// whatever permissions it already had — including looser ones left by an
/// older build or an external tool. Called on every [`TraceStore::open`],
/// not only at first creation, so an existing store is tightened rather than
/// trusted (see `docs/design/forensic-retention-and-exposure.md`, Decision 2).
#[cfg(unix)]
fn secure_store_permissions(path: &Path) -> Result<(), StoreError> {
    use std::{fs, os::unix::fs::PermissionsExt};

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn decode_canonical_trace(row: (String, Vec<u8>)) -> Result<CanonicalTrace, StoreError> {
    let (trace_id, canonical_json) = row;
    Ok(CanonicalTrace {
        id: TraceId::from_str(&trace_id)?,
        semantic_core: SemanticCore::from_json_slice(&canonical_json)?,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::*;

    fn semantic(json: &str) -> SemanticCore {
        SemanticCore::from_json_slice(json.as_bytes()).unwrap()
    }

    /// A minimal, deterministic occurrence for tests that do not care about
    /// occurrence facts beyond satisfying the natural key.
    fn occurrence(session: &'static str, sequence: i64) -> NewOccurrence<'static> {
        NewOccurrence {
            session,
            sequence,
            timestamp: 0,
            repo: "",
            repo_source: RepoSource::None,
            parent_sequence: None,
            agent: None,
            authored_by: AuthoredBy::Unknown,
        }
    }

    fn counts(store: &TraceStore) -> (i64, i64) {
        let canonical = store
            .connection
            .query_row("SELECT count(*) FROM canonical_traces", [], |row| {
                row.get(0)
            })
            .unwrap();
        let raw = store
            .connection
            .query_row("SELECT count(*) FROM raw_carrier_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        (canonical, raw)
    }

    fn occurrence_count(store: &TraceStore) -> i64 {
        store
            .connection
            .query_row("SELECT count(*) FROM occurrences", [], |row| row.get(0))
            .unwrap()
    }

    fn link_count(store: &TraceStore) -> i64 {
        store
            .connection
            .query_row("SELECT count(*) FROM raw_record_traces", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn claude_line_links_three_blocks_replays_once_and_forgets_as_one_line() {
        let cores = [
            semantic(r#"{"block":"one"}"#),
            semantic(r#"{"block":"two"}"#),
            semantic(r#"{"block":"three"}"#),
        ];
        let blocks = cores
            .iter()
            .enumerate()
            .map(|(index, core)| (core.clone(), occurrence("session-a", 2048 + index as i64)))
            .collect::<Vec<_>>();
        let mut store = TraceStore::open_in_memory().unwrap();
        let raw = NewRawCarrierRecord::new("claude", b"three projected blocks\n");
        let (id, first) = store
            .capture_line(raw, "session-a", 2048, 42, &blocks)
            .unwrap();
        assert_eq!(counts(&store), (3, 1));
        assert_eq!(link_count(&store), 3);
        assert_eq!(occurrence_count(&store), 3);
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Session("session-a"))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Before(42))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Before(43))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-a"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::TimestampRange(
                    42, 43
                ))
                .unwrap()
                .len(),
            1
        );
        assert!(first.iter().all(CaptureResult::canonical_inserted));
        assert!(first.iter().all(|result| result.raw_record_id() == id));

        let (replayed_id, replayed) = store
            .capture_line(raw, "session-a", 2048, 42, &blocks)
            .unwrap();
        assert_eq!(replayed_id, id);
        assert!(replayed.iter().all(|result| !result.canonical_inserted()));
        assert_eq!(counts(&store), (3, 1));
        assert_eq!(link_count(&store), 3);
        assert_eq!(occurrence_count(&store), 3);

        let trace_ids = cores
            .iter()
            .map(|core| core.trace_id().unwrap())
            .collect::<Vec<_>>();
        let selected = store.fetch_raw_carrier_records(&trace_ids[1]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id(), id);
        let mut sorted_ids = trace_ids.clone();
        sorted_ids.sort();
        assert_eq!(selected[0].trace_ids(), sorted_ids);
        let foreign_keys: i64 = store
            .connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
        assert_eq!(
            store
                .forget_forensic(ForgetSelector::Trace(&trace_ids[1]))
                .unwrap(),
            1
        );
        assert_eq!(raw_record_count(&store), 0);
        assert_eq!(
            link_count(&store),
            0,
            "foreign-key cascade must delete every link"
        );
        assert_eq!(occurrence_count(&store), 3);
        assert_eq!(store.log(&LogFilter::default()).unwrap().len(), 3);
        assert!(store.fetch_trace(&trace_ids[0]).unwrap().is_some());
        assert!(store.fetch_trace(&trace_ids[2]).unwrap().is_some());
    }

    #[test]
    fn raw_line_resolution_reuses_identical_bytes_and_keeps_rewrites() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let id = store
            .capture_raw_only("claude", b"original", "session", 1024, 10)
            .unwrap();
        assert_eq!(
            store
                .capture_raw_only("claude", b"original", "session", 1024, 10)
                .unwrap(),
            id
        );
        let rewritten = store
            .capture_raw_only("claude", b"changed", "session", 1024, 11)
            .unwrap();
        assert_ne!(rewritten, id);
        assert_eq!(raw_record_count(&store), 2);
        assert_eq!(link_count(&store), 0);
        let rows = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session"))
            .unwrap();
        assert_eq!(
            rows.iter().map(RawCarrierRecord::bytes).collect::<Vec<_>>(),
            vec![b"original".as_slice(), b"changed".as_slice()]
        );
    }

    #[test]
    fn raw_backfill_checks_whole_file_and_rolls_back_failed_inserts() {
        let path = std::env::temp_dir().join(format!(
            "semon-backfill-store-{}.sqlite3",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut store = TraceStore::open(&path).unwrap();
        store
            .capture_raw_only("codex", b"same\n", "s", 0, 10)
            .unwrap();
        store
            .capture_raw_only("codex", b"old\n", "s", 2, 12)
            .unwrap();
        let line = |bytes: &[u8], sequence| RawBackfillLine {
            bytes: bytes.to_vec(),
            session: "s".into(),
            sequence,
            timestamp: 20 + sequence,
        };
        let conflicted = [line(b"same\n", 0), line(b"missing\n", 1), line(b"new\n", 2)];
        let result = store
            .backfill_raw_lines("codex", &conflicted, false)
            .unwrap();
        assert_eq!(result.misaligned_sequence, Some(2));
        assert_eq!(result.inserted, 0);
        assert_eq!(raw_record_count(&store), 2);

        let aligned = [line(b"same\n", 0), line(b"missing\n", 1)];
        let preview = store.backfill_raw_lines("codex", &aligned, true).unwrap();
        assert_eq!((preview.inserted, preview.already_present), (1, 1));
        assert_eq!(raw_record_count(&store), 2);
        assert_eq!(
            store.backfill_raw_lines("codex", &aligned, false).unwrap(),
            preview
        );
        assert_eq!(raw_record_count(&store), 3);
        assert_eq!(
            store
                .backfill_raw_lines("codex", &aligned, false)
                .unwrap()
                .inserted,
            0
        );

        store.connection.execute_batch("CREATE TRIGGER reject_backfill BEFORE INSERT ON raw_carrier_records WHEN NEW.sequence = 4 BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
        let failed = [line(b"first\n", 3), line(b"invalid\n", 4)];
        assert!(store.backfill_raw_lines("codex", &failed, false).is_err());
        assert_eq!(raw_record_count(&store), 3);
        assert_eq!(link_count(&store), 0);
        assert_eq!(occurrence_count(&store), 0);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn equivalent_encodings_and_insertion_orders_converge() {
        let first = semantic(r#"{"z":null,"steps":["plan","act"],"nested":{"b":2,"a":1},"n":1}"#);
        let second = semantic(
            r#"
            {
              "n": 1.0,
              "nested": { "a": 1, "b": 2 },
              "steps": ["\u0070lan", "act"],
              "z": null
            }
            "#,
        );

        let mut nested = Map::new();
        nested.insert("b".into(), json!(2));
        nested.insert("a".into(), json!(1));
        let mut inserted = Map::new();
        inserted.insert("steps".into(), json!(["plan", "act"]));
        inserted.insert("z".into(), Value::Null);
        inserted.insert("n".into(), json!(1));
        inserted.insert("nested".into(), Value::Object(nested));
        let third = SemanticCore::from_value(Value::Object(inserted)).unwrap();

        let expected_id = first.trace_id().unwrap();
        assert_eq!(second.trace_id().unwrap(), expected_id);
        assert_eq!(third.trace_id().unwrap(), expected_id);

        let mut store = TraceStore::open_in_memory().unwrap();
        for (index, (core, raw)) in [
            (&first, b"synthetic-one".as_slice()),
            (&second, b"synthetic-two".as_slice()),
            (&third, b"synthetic-three".as_slice()),
        ]
        .into_iter()
        .enumerate()
        {
            store
                .capture(
                    core,
                    NewRawCarrierRecord::new("carrier-a", raw),
                    occurrence("session-a", index as i64),
                )
                .unwrap();
        }

        assert_eq!(counts(&store), (1, 3));
        assert_eq!(occurrence_count(&store), 3);
    }

    #[test]
    fn genuinely_different_semantics_have_different_ids() {
        let succeeded = semantic(r#"{"outcome":"succeeded"}"#);
        let failed = semantic(r#"{"outcome":"failed"}"#);

        assert_ne!(succeeded.trace_id().unwrap(), failed.trace_id().unwrap());

        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture(
                &succeeded,
                NewRawCarrierRecord::new("carrier-a", b"record-a"),
                occurrence("session-a", 0),
            )
            .unwrap();
        store
            .capture(
                &failed,
                NewRawCarrierRecord::new("carrier-a", b"record-b"),
                occurrence("session-a", 1),
            )
            .unwrap();
        assert_eq!(counts(&store), (2, 2));
    }

    fn custody_raw(store: &mut TraceStore, source: &str, sequence: i64, bytes: &[u8]) -> i64 {
        store
            .with_capture_source("codex", source, |store| {
                store.capture_raw_only("codex", bytes, "custody-session", sequence, 0)
            })
            .unwrap()
    }

    fn confirm_custody(store: &mut TraceStore, source: &str, bytes: &[u8]) {
        store
            .checkpoint_capture_source(
                "codex",
                source,
                bytes.len() as u64,
                &format!("{:x}", Sha256::digest(bytes)),
            )
            .unwrap();
    }

    #[test]
    fn capture_custody_selects_restored_bytes_instead_of_latest_raw_insertion() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let source = "/fixture/restored";
        let original = custody_raw(&mut store, source, 0, b"A\n");
        confirm_custody(&mut store, source, b"A\n");
        store.reset_capture_source("codex", source).unwrap();
        let replacement = custody_raw(&mut store, source, 0, b"B\n");
        confirm_custody(&mut store, source, b"B\n");
        store.reset_capture_source("codex", source).unwrap();
        let restored = custody_raw(&mut store, source, 0, b"A\n");
        confirm_custody(&mut store, source, b"A\n");
        assert_eq!(original, restored);
        assert!(replacement > restored);
        assert_eq!(
            store
                .fetch_current_capture_source_raw("codex", source)
                .unwrap(),
            Some(b"A\n".to_vec())
        );
        assert_eq!(
            store
                .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(
                    "custody-session"
                ))
                .unwrap()
                .len(),
            2
        );
        let generations: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM capture_evidence_generations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generations, 3);
    }

    #[test]
    fn capture_custody_preserves_unknown_frames_and_independent_sources() {
        let mut store = TraceStore::open_in_memory().unwrap();
        custody_raw(&mut store, "/fixture/one", 0, b"unknown\n");
        custody_raw(&mut store, "/fixture/one", 1024, b"malformed\n");
        custody_raw(&mut store, "/fixture/one", 2048, b"\n");
        confirm_custody(&mut store, "/fixture/one", b"unknown\nmalformed\n\n");
        custody_raw(&mut store, "/fixture/two", 0, b"unknown\n");
        confirm_custody(&mut store, "/fixture/two", b"unknown\n");
        store.reset_capture_source("codex", "/fixture/one").unwrap();
        confirm_custody(&mut store, "/fixture/one", b"");
        assert_eq!(
            store
                .fetch_current_capture_source_raw("codex", "/fixture/one")
                .unwrap(),
            Some(Vec::new())
        );
        assert_eq!(
            store
                .fetch_current_capture_source_raw("codex", "/fixture/two")
                .unwrap(),
            Some(b"unknown\n".to_vec())
        );
        assert_eq!(
            store
                .fetch_current_capture_source_raw("claude", "/fixture/two")
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(
                    "custody-session"
                ))
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn capture_custody_missing_or_interrupted_evidence_never_guesses() {
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture_raw_only("codex", b"legacy\n", "custody-session", 0, 0)
            .unwrap();
        assert_eq!(
            store
                .fetch_current_capture_source_raw("codex", "/fixture/legacy")
                .unwrap(),
            None
        );
        custody_raw(&mut store, "/fixture/recorded", 0, b"recorded\n");
        assert!(matches!(
            store.fetch_current_capture_source_raw("codex", "/fixture/recorded"),
            Err(StoreError::IncompleteCaptureEvidence)
        ));
        confirm_custody(&mut store, "/fixture/recorded", b"recorded\n");
        let digest = format!("{:x}", Sha256::digest(b"recorded\n"));
        assert!(
            store
                .capture_source_cursor_matches("codex", "/fixture/recorded", 9, &digest)
                .unwrap()
        );
        store
            .forget_forensic(ForgetSelector::Session("custody-session"))
            .unwrap();
        assert!(matches!(
            store.fetch_current_capture_source_raw("codex", "/fixture/recorded"),
            Err(StoreError::IncompleteCaptureEvidence)
        ));
        assert!(
            store
                .capture_source_cursor_matches("codex", "/fixture/recorded", 9, &digest)
                .unwrap(),
            "forensic forget must not force an idle recapture"
        );
    }

    #[test]
    fn legacy_custody_adoption_does_not_recreate_forgotten_bytes() {
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture_raw_only("codex", b"legacy\n", "legacy-session", 0, 0)
            .unwrap();
        store
            .forget_forensic(ForgetSelector::Session("legacy-session"))
            .unwrap();
        let lines = [RawBackfillLine {
            bytes: b"legacy\n".to_vec(),
            session: "legacy-session".into(),
            sequence: 0,
            timestamp: 0,
        }];
        let digest = format!("{:x}", Sha256::digest(b"legacy\n"));
        store
            .adopt_retained_capture_prefix("codex", "/fixture/legacy", &lines, 7, &digest)
            .unwrap();
        assert_eq!(counts(&store), (0, 0));
        assert!(
            store
                .capture_source_cursor_matches("codex", "/fixture/legacy", 7, &digest)
                .unwrap()
        );
        assert!(matches!(
            store.fetch_current_capture_source_raw("codex", "/fixture/legacy"),
            Err(StoreError::IncompleteCaptureEvidence)
        ));
    }

    #[test]
    fn session_repair_preserves_custody_when_identical_raw_rows_merge() {
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture_raw_only("codex", b"A\n", "old", 0, 0)
            .unwrap();
        let kept = store
            .capture_raw_only("codex", b"A\n", "new", 0, 0)
            .unwrap();
        let digest = format!("{:x}", Sha256::digest(b"A\n"));
        store
            .adopt_retained_capture_prefix(
                "codex",
                "/fixture/repair",
                &[RawBackfillLine {
                    bytes: b"A\n".to_vec(),
                    session: "old".into(),
                    sequence: 0,
                    timestamp: 0,
                }],
                2,
                &digest,
            )
            .unwrap();
        store
            .repair_session_keys::<StoreError, _>(
                "codex",
                "old",
                &[RawSessionRekeyLine {
                    old_session: "old".into(),
                    new_session: "new".into(),
                    sequence: 0,
                    bytes: b"A\n".to_vec(),
                }],
                false,
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(
            store
                .fetch_current_capture_source_raw("codex", "/fixture/repair")
                .unwrap(),
            Some(b"A\n".to_vec())
        );
        let bound: i64 = store
            .connection
            .query_row(
                "SELECT raw_record_id FROM capture_evidence_records",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bound, kept);
        assert_eq!(counts(&store), (0, 1));
    }

    #[test]
    fn legacy_custody_adoption_selects_exact_retained_bytes_only() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let original = store
            .capture_raw_only("codex", b"A\n", "legacy-session", 0, 0)
            .unwrap();
        store
            .capture_raw_only("codex", b"B\n", "legacy-session", 0, 0)
            .unwrap();
        let lines = [RawBackfillLine {
            bytes: b"A\n".to_vec(),
            session: "legacy-session".into(),
            sequence: 0,
            timestamp: 0,
        }];
        let digest = format!("{:x}", Sha256::digest(b"A\n"));
        store
            .adopt_retained_capture_prefix("codex", "/fixture/legacy", &lines, 2, &digest)
            .unwrap();
        assert_eq!(
            store
                .fetch_current_capture_source_raw("codex", "/fixture/legacy")
                .unwrap(),
            Some(b"A\n".to_vec())
        );
        let retained: i64 = store
            .connection
            .query_row(
                "SELECT raw_record_id FROM capture_evidence_records",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, original);
        assert_eq!(counts(&store), (0, 2));
    }

    #[test]
    fn version_six_ownership_migration_hashes_recorded_keys_without_guessing_custody() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let source = "/fixture/version-six";
        let core = semantic(r#"{"goal":"known ownership"}"#);
        store
            .with_capture_source("codex", source, |store| {
                store.capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"opaque\n"),
                    occurrence("legacy-session", 0),
                )
            })
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE capture_source_occurrences SET source_key = ?1",
                [source],
            )
            .unwrap();
        store.connection.execute_batch("DROP TABLE capture_evidence_records; DROP TABLE capture_evidence_generations; PRAGMA user_version = 6;").unwrap();
        let mut upgraded = TraceStore::from_connection(store.connection).unwrap();
        let key: String = upgraded
            .connection
            .query_row(
                "SELECT source_key FROM capture_source_occurrences",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(key, capture_source_digest(source));
        assert_eq!(occurrence_count(&upgraded), 1);
        assert_eq!(counts(&upgraded), (1, 1));
        assert!(
            !upgraded
                .has_capture_source_custody("codex", source)
                .unwrap()
        );
        assert_eq!(
            upgraded
                .fetch_current_capture_source_raw("codex", source)
                .unwrap(),
            None
        );
        upgraded.reset_capture_source("codex", source).unwrap();
        assert_eq!(occurrence_count(&upgraded), 0);
        assert_eq!(counts(&upgraded), (1, 1));
    }

    #[test]
    fn custody_and_ownership_are_reconstructable_without_pathname_indexes() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let source = "/fixture/private-source";
        let core = semantic(r#"{"goal":"rebuild ownership"}"#);
        store
            .with_capture_source("codex", source, |store| {
                store.capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"A\n"),
                    occurrence("custody-session", 0),
                )
            })
            .unwrap();
        confirm_custody(&mut store, source, b"A\n");
        let digest: String = store
            .connection
            .query_row(
                "SELECT source_key_sha256 FROM capture_evidence_generations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        store
            .connection
            .execute_batch(
                "DELETE FROM capture_source_occurrences; DROP INDEX capture_evidence_source;",
            )
            .unwrap();
        store
            .with_capture_source_digest("codex", &digest, |store| {
                store.capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"A\n"),
                    occurrence("custody-session", 0),
                )
            })
            .unwrap();
        confirm_custody(&mut store, source, b"A\n");
        assert_eq!(
            store
                .fetch_current_capture_source_raw("codex", source)
                .unwrap(),
            Some(b"A\n".to_vec())
        );
        let rebuilt_key: String = store
            .connection
            .query_row(
                "SELECT source_key FROM capture_source_occurrences",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rebuilt_key, digest);
        assert_eq!(counts(&store), (1, 1));
        store.reset_capture_source("codex", source).unwrap();
        assert_eq!(occurrence_count(&store), 0);
        assert_eq!(counts(&store), (1, 1));
    }

    #[test]
    fn capture_custody_insert_failure_rolls_back_all_line_regions() {
        let mut store = TraceStore::open_in_memory().unwrap();
        store.connection.execute_batch("CREATE TRIGGER fail_custody BEFORE INSERT ON capture_evidence_records BEGIN SELECT RAISE(ABORT, 'synthetic custody failure'); END;").unwrap();
        let core = semantic(r#"{"goal":"custody rollback"}"#);
        let error = store
            .with_capture_source("codex", "/fixture/rollback", |store| {
                store.capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"private raw"),
                    occurrence("custody-session", 0),
                )
            })
            .unwrap_err();
        assert!(matches!(error, StoreError::Sqlite(_)));
        assert_eq!(counts(&store), (0, 0));
        assert_eq!(occurrence_count(&store), 0);
        let generations: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM capture_evidence_generations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generations, 0);
    }

    #[test]
    fn changing_raw_bytes_does_not_change_identity() {
        let core = semantic(r#"{"goal":"compile component","outcome":"done"}"#);
        let mut store = TraceStore::open_in_memory().unwrap();

        let first = store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-a", b"opaque variant one"),
                occurrence("session-a", 0),
            )
            .unwrap();
        let second = store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-b", b"opaque variant two"),
                occurrence("session-a", 1),
            )
            .unwrap();

        assert_eq!(first.trace_id(), second.trace_id());
        assert!(first.canonical_inserted());
        assert!(!second.canonical_inserted());
        assert_eq!(counts(&store), (1, 2));
    }

    #[test]
    fn failure_during_raw_insert_rolls_back_both_regions() {
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_raw_insert BEFORE INSERT ON raw_carrier_records \
                 BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;",
            )
            .unwrap();
        let core = semantic(r#"{"goal":"exercise rollback"}"#);

        let error = store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-a", b"synthetic raw bytes"),
                occurrence("session-a", 0),
            )
            .unwrap_err();

        assert!(matches!(error, StoreError::Sqlite(_)));
        assert_eq!(counts(&store), (0, 0));
        assert_eq!(occurrence_count(&store), 0);
    }

    #[test]
    fn repeat_capture_keeps_one_canonical_row_and_appends_raw_rows() {
        let core = semantic(r#"{"action":"inspect","result":"understood"}"#);
        let mut store = TraceStore::open_in_memory().unwrap();

        let first = store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-a", b"same record"),
                occurrence("session-a", 0),
            )
            .unwrap();
        let second = store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-a", b"same record"),
                occurrence("session-a", 1),
            )
            .unwrap();

        assert!(first.canonical_inserted());
        assert!(!second.canonical_inserted());
        assert_ne!(first.raw_record_id(), second.raw_record_id());
        assert_eq!(counts(&store), (1, 2));
    }

    #[test]
    fn raw_only_capture_writes_no_trace_or_occurrence_and_uses_its_own_selectors() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let raw_record_id = store
            .capture_raw_only("codex", b"unprojected metadata\n", "session-raw", 7, 150)
            .unwrap();

        assert!(raw_record_id > 0);
        assert_eq!(counts(&store), (0, 1));
        assert_eq!(occurrence_count(&store), 0);
        assert!(store.list_traces(None, 10).unwrap().is_empty());
        assert!(store.log(&LogFilter::default()).unwrap().is_empty());

        let by_session = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-raw"))
            .unwrap();
        assert_eq!(by_session.len(), 1);
        assert_eq!(by_session[0].trace_ids(), &[]);
        assert_eq!(by_session[0].bytes(), b"unprojected metadata\n");

        let by_time = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::TimestampRange(100, 200))
            .unwrap();
        assert_eq!(by_time, by_session);

        let unrelated_trace =
            TraceId::from_str("aa11223344556677889900112233445566778899001122334455667788990011")
                .unwrap();
        assert!(
            store
                .fetch_raw_carrier_records(&unrelated_trace)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Trace(&unrelated_trace))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Session("session-raw"))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Before(151))
                .unwrap(),
            1
        );
    }

    #[test]
    fn ordinary_reads_return_semantics_and_raw_requires_explicit_fetch() {
        let core = semantic(r#"{"subject":"synthetic work","state":"complete"}"#);
        let mut store = TraceStore::open_in_memory().unwrap();
        let capture = store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-a", b"forensic-only bytes"),
                occurrence("session-a", 0),
            )
            .unwrap();

        let fetched = store.fetch_trace(capture.trace_id()).unwrap().unwrap();
        let listed = store.list_traces(None, 10).unwrap();
        assert_eq!(fetched.semantic_core(), &core);
        assert_eq!(listed, vec![fetched]);

        let raw = store.fetch_raw_carrier_records(capture.trace_id()).unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].bytes(), b"forensic-only bytes");
    }

    #[test]
    fn capture_source_reset_preserves_shared_unowned_and_forensic_evidence() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let core = semantic(r#"{"goal":"source-owned projection"}"#);
        let capture = |store: &mut TraceStore, sequence| {
            store.capture(
                &core,
                NewRawCarrierRecord::new("codex", b"private-native-bytes"),
                occurrence("s", sequence),
            )
        };
        store
            .with_capture_source("codex", "/fixture/a", |store| capture(store, 0))
            .unwrap();
        store
            .with_capture_source("codex", "/fixture/a", |store| capture(store, 1))
            .unwrap();
        store
            .with_capture_source("codex", "/fixture/b", |store| capture(store, 1))
            .unwrap();
        capture(&mut store, 2).unwrap();
        let raw_before = raw_record_count(&store);
        let links_before = link_count(&store);
        store.reset_capture_source("claude", "/fixture/a").unwrap();
        assert_eq!(occurrence_count(&store), 3);
        store.reset_capture_source("codex", "/fixture/a").unwrap();
        assert_eq!(occurrence_count(&store), 2);
        store.reset_capture_source("codex", "/fixture/b").unwrap();
        assert_eq!(
            occurrence_count(&store),
            1,
            "unowned observations must remain"
        );
        assert_eq!(raw_record_count(&store), raw_before);
        assert_eq!(link_count(&store), links_before);
        assert_eq!(store.list_traces(None, 10).unwrap().len(), 1);
        let leaked: Result<(), StoreError> =
            store.with_capture_source("codex", "/fixture/error", |_| Err(StoreError::BlankCarrier));
        assert!(leaked.is_err());
        capture(&mut store, 3).unwrap();
        store
            .reset_capture_source("codex", "/fixture/error")
            .unwrap();
        assert_eq!(
            occurrence_count(&store),
            2,
            "owner must restore after an error"
        );
    }

    #[test]
    fn a_failed_source_ownership_write_rolls_back_the_entire_capture() {
        let mut store = TraceStore::open_in_memory().unwrap();
        store.connection.execute_batch("CREATE TRIGGER reject_owner BEFORE INSERT ON capture_source_occurrences BEGIN SELECT RAISE(ABORT, 'ownership failure'); END;").unwrap();
        let core = semantic(r#"{"goal":"atomic source ownership"}"#);
        let result = store.with_capture_source("codex", "/fixture/source", |store| {
            store.capture(
                &core,
                NewRawCarrierRecord::new("codex", b"native"),
                occurrence("s", 0),
            )
        });
        assert!(result.is_err());
        assert_eq!(raw_record_count(&store), 0);
        assert_eq!(occurrence_count(&store), 0);
        assert_eq!(link_count(&store), 0);
        assert!(store.list_traces(None, 10).unwrap().is_empty());
    }

    #[test]
    fn version_five_ownership_migration_does_not_guess_legacy_sources() {
        let path = unique_temp_db_path("source-ownership-migration");
        let mut store = TraceStore::open(&path).unwrap();
        let core = semantic(r#"{"goal":"legacy source remains unknown"}"#);
        store
            .capture(
                &core,
                NewRawCarrierRecord::new("codex", b"legacy-native"),
                occurrence("s", 0),
            )
            .unwrap();
        store
            .connection
            .execute_batch("DROP TABLE capture_source_occurrences; PRAGMA user_version = 5;")
            .unwrap();
        drop(store);
        let mut store = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        store
            .reset_capture_source("codex", "/fixture/source")
            .unwrap();
        assert_eq!(occurrence_count(&store), 1);
        assert_eq!(raw_record_count(&store), 1);
        assert_eq!(link_count(&store), 1);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn write_then_fetch_by_hash_round_trips_semantics() {
        let core = semantic(
            r#"{"goal":"test round trip","observations":["one","two"],"conclusion":null}"#,
        );
        let expected_id = core.trace_id().unwrap();
        let mut store = TraceStore::open_in_memory().unwrap();

        store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-a", b"synthetic payload"),
                occurrence("session-a", 0),
            )
            .unwrap();

        let stored = store.fetch_trace(&expected_id).unwrap().unwrap();
        assert_eq!(stored.id(), &expected_id);
        assert_eq!(stored.semantic_core(), &core);
    }

    #[test]
    fn list_uses_stable_hash_cursor_pagination() {
        let cores = [
            semantic(r#"{"item":"alpha"}"#),
            semantic(r#"{"item":"beta"}"#),
            semantic(r#"{"item":"gamma"}"#),
        ];
        let mut store = TraceStore::open_in_memory().unwrap();
        for (index, core) in cores.iter().enumerate() {
            store
                .capture(
                    core,
                    NewRawCarrierRecord::new("carrier-a", b"synthetic"),
                    occurrence("session-a", index as i64),
                )
                .unwrap();
        }

        let first_page = store.list_traces(None, 2).unwrap();
        let second_page = store
            .list_traces(first_page.last().map(CanonicalTrace::id), 2)
            .unwrap();

        assert_eq!(first_page.len(), 2);
        assert_eq!(second_page.len(), 1);
        assert!(first_page[0].id() < first_page[1].id());
        assert!(first_page[1].id() < second_page[0].id());
    }

    /// A unique path under the OS temp directory for a file-backed store.
    ///
    /// These tests need a real file (not `open_in_memory`) because the
    /// behavior under test is what a *second* `open()` sees on disk after
    /// the first connection is closed.
    fn unique_temp_db_path(label: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "semon-store-test-{label}-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn user_version(store: &TraceStore) -> u32 {
        store
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn store_file_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = unique_temp_db_path("perms-created");
        let store = TraceStore::open(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "store file must be created 0600, got {mode:o}");

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn loosened_store_permissions_are_retightened_on_reopen() {
        use std::os::unix::fs::PermissionsExt;

        let path = unique_temp_db_path("perms-retightened");
        {
            let store = TraceStore::open(&path).unwrap();
            drop(store);
        }

        // Simulate an existing store with looser permissions than this
        // policy requires — e.g. one created before this fix, or loosened by
        // some external tool.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loosened = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            loosened, 0o644,
            "test setup did not actually loosen permissions"
        );

        let store = TraceStore::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "reopening must re-tighten permissions rather than trust them, got {mode:o}"
        );

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fresh_database_is_stamped_with_current_schema_version() {
        let path = unique_temp_db_path("fresh-version");
        let store = TraceStore::open(&path).unwrap();

        assert_eq!(user_version(&store), SCHEMA_VERSION);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn v3_and_v4_migration_merge_only_identical_claude_lines_and_preserve_every_link() {
        for version in [3, 4] {
            let path = unique_temp_db_path(&format!("merge-v{version}"));
            let ids = (1..=5).map(|n| format!("{n:064x}")).collect::<Vec<_>>();
            {
                let connection = Connection::open(&path).unwrap();
                connection
                    .execute_batch(&format!(
                        "PRAGMA foreign_keys = ON; \
                         CREATE TABLE canonical_traces (trace_id TEXT PRIMARY KEY NOT NULL, canonical_json BLOB NOT NULL) STRICT; \
                         CREATE TABLE raw_carrier_records ( \
                             raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT, \
                             trace_id TEXT REFERENCES canonical_traces(trace_id), \
                             carrier TEXT NOT NULL, raw_bytes BLOB NOT NULL, \
                             session TEXT, sequence INTEGER{} \
                         ) STRICT; \
                         INSERT INTO canonical_traces VALUES \
                             ('{}', X'7b7d'), ('{}', X'7b7d'), ('{}', X'7b7d'), \
                             ('{}', X'7b7d'), ('{}', X'7b7d'); \
                         INSERT INTO raw_carrier_records \
                             (raw_record_id, trace_id, carrier, raw_bytes, session, sequence) VALUES \
                             (1, '{}', 'claude', X'73616d65', 'session', 1024), \
                             (2, '{}', 'claude', X'73616d65', 'session', 1025), \
                             (3, '{}', 'claude', X'73616d65', 'session', 1026), \
                             (4, '{}', 'claude', X'64696666', 'session', 1024), \
                             (5, '{}', 'codex', X'73616d65', 'session', 1025); \
                         PRAGMA user_version = {version};",
                        if version == 4 { ", timestamp INTEGER" } else { "" },
                        ids[0], ids[1], ids[2], ids[3], ids[4],
                        ids[0], ids[1], ids[2], ids[3], ids[4],
                    ))
                    .unwrap();
            }
            let store = TraceStore::open(&path).unwrap();
            assert_eq!(user_version(&store), SCHEMA_VERSION);
            assert_eq!(raw_record_count(&store), 3);
            assert_eq!(link_count(&store), 5);
            let rows = {
                let mut statement = store.connection.prepare(
                    "SELECT raw_record_id, carrier, sequence FROM raw_carrier_records ORDER BY raw_record_id"
                ).unwrap();
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };
            assert_eq!(
                rows,
                vec![
                    (1, "claude".to_owned(), 1024),
                    (4, "claude".to_owned(), 1024),
                    (5, "codex".to_owned(), 1025)
                ]
            );
            for (index, expected_raw_id) in [1, 1, 1, 4, 5].into_iter().enumerate() {
                let trace_id = TraceId::from_str(&ids[index]).unwrap();
                let records = store.fetch_raw_carrier_records(&trace_id).unwrap();
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].id(), expected_raw_id);
            }
            let before = store.connection.prepare("SELECT raw_record_id, trace_id FROM raw_record_traces ORDER BY raw_record_id, trace_id")
                .unwrap().query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
                .unwrap().collect::<Result<Vec<_>, _>>().unwrap();
            drop(store);
            let reopened = TraceStore::open(&path).unwrap();
            let after = reopened.connection.prepare("SELECT raw_record_id, trace_id FROM raw_record_traces ORDER BY raw_record_id, trace_id")
                .unwrap().query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
                .unwrap().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(after, before);
            assert_eq!(raw_record_count(&reopened), 3);
            drop(reopened);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn failed_v5_migration_keeps_v4_table_and_version() {
        let path = unique_temp_db_path("failed-v5");
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(
                "CREATE TABLE raw_carrier_records ( \
                    raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT, \
                    trace_id TEXT, carrier TEXT NOT NULL, raw_bytes BLOB NOT NULL, \
                    session TEXT, sequence INTEGER, timestamp INTEGER); \
                 INSERT INTO raw_carrier_records (trace_id, carrier, raw_bytes, session, sequence) \
                     VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'claude', X'61', 'session', 0); \
                 CREATE TABLE raw_record_traces (raw_record_id INTEGER, trace_id TEXT CHECK(0)); \
                 PRAGMA user_version = 4;"
            ).unwrap();
        }
        assert!(TraceStore::open(&path).is_err());
        let connection = Connection::open(&path).unwrap();
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        let columns = connection
            .prepare("PRAGMA table_info(raw_carrier_records)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.contains(&"trace_id".to_owned()));
        let bytes: Vec<u8> = connection
            .query_row("SELECT raw_bytes FROM raw_carrier_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(bytes, b"a");
        drop(connection);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn v3_migration_rebuilds_raw_without_row_loss_and_is_idempotent() {
        let path = unique_temp_db_path("v3-to-v4");
        let linked_trace = "aa11223344556677889900112233445566778899001122334455667788990011";
        let unlinked_trace = "bb11223344556677889900112233445566778899001122334455667788990011";

        // Build the fixture in the test itself using the exact v3 shape:
        // trace_id is NOT NULL and raw rows have no timestamp column.
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(&format!(
                    "PRAGMA foreign_keys = ON; \
                     CREATE TABLE canonical_traces ( \
                         trace_id TEXT PRIMARY KEY NOT NULL CHECK(length(trace_id) = 64), \
                         canonical_json BLOB NOT NULL \
                     ) STRICT; \
                     CREATE TABLE raw_carrier_records ( \
                         raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT, \
                         trace_id TEXT NOT NULL REFERENCES canonical_traces(trace_id), \
                         carrier TEXT NOT NULL CHECK(length(trim(carrier)) > 0), \
                         raw_bytes BLOB NOT NULL, \
                         session TEXT, \
                         sequence INTEGER \
                     ) STRICT; \
                     CREATE INDEX raw_carrier_records_by_trace \
                         ON raw_carrier_records(trace_id, raw_record_id); \
                     CREATE INDEX raw_carrier_records_by_carrier_session_sequence \
                         ON raw_carrier_records(carrier, session, sequence); \
                     CREATE TABLE occurrences ( \
                         occurrence_id INTEGER PRIMARY KEY AUTOINCREMENT, \
                         trace_id TEXT NOT NULL REFERENCES canonical_traces(trace_id), \
                         carrier TEXT NOT NULL CHECK(length(trim(carrier)) > 0), \
                         session TEXT NOT NULL, \
                         sequence INTEGER NOT NULL CHECK(sequence >= 0), \
                         timestamp INTEGER NOT NULL, \
                         repo TEXT NOT NULL, \
                         repo_source TEXT NOT NULL, \
                         parent_sequence INTEGER, \
                         agent TEXT, \
                         authored_by TEXT NOT NULL, \
                         UNIQUE (carrier, session, sequence) \
                     ) STRICT; \
                     CREATE INDEX occurrences_by_repo_timestamp \
                         ON occurrences(repo, timestamp); \
                     INSERT INTO canonical_traces VALUES \
                         ('{linked_trace}', X'7b7d'), \
                         ('{unlinked_trace}', X'7b7d'); \
                     INSERT INTO occurrences \
                         (trace_id, carrier, session, sequence, timestamp, repo, repo_source, \
                          parent_sequence, agent, authored_by) \
                     VALUES ('{linked_trace}', 'codex', 'linked-session', 4, 987654321, '', \
                             'none', NULL, NULL, 'unknown'); \
                     INSERT INTO raw_carrier_records \
                         (raw_record_id, trace_id, carrier, raw_bytes, session, sequence) \
                     VALUES (3, '{linked_trace}', 'codex', X'6c696e6b6564', \
                             'linked-session', 4), \
                            (8, '{unlinked_trace}', 'codex', X'756e6c696e6b6564', NULL, NULL); \
                     PRAGMA user_version = 3;"
                ))
                .unwrap();
        }

        let store = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        assert_eq!(raw_record_count(&store), 2, "migration must lose no rows");

        let migrated_rows = {
            let mut statement = store
                .connection
                .prepare(
                    "SELECT raw_record_id, session, sequence, timestamp, raw_bytes \
                     FROM raw_carrier_records ORDER BY raw_record_id",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(migrated_rows.len(), 2);
        assert_eq!(migrated_rows[0].0, 3);
        assert_eq!(migrated_rows[0].3, Some(987654321));
        assert_eq!(migrated_rows[0].4, b"linked");
        assert_eq!(migrated_rows[1].0, 8);
        assert_eq!(migrated_rows[1].3, None);
        assert_eq!(migrated_rows[1].4, b"unlinked");

        let columns = {
            let mut statement = store
                .connection
                .prepare("PRAGMA table_info(raw_carrier_records)")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(!columns.iter().any(|(name, _)| name == "trace_id"));
        assert!(columns.iter().any(|(name, _)| name == "timestamp"));

        drop(store);

        // A second open sees user_version 5 and leaves every migrated field
        // byte-for-byte unchanged.
        let reopened = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&reopened), SCHEMA_VERSION);
        let reopened_rows = {
            let mut statement = reopened
                .connection
                .prepare(
                    "SELECT raw_record_id, session, sequence, timestamp, raw_bytes \
                     FROM raw_carrier_records ORDER BY raw_record_id",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(reopened_rows, migrated_rows);

        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_a_database_with_an_older_known_version_migrates_it_forward() {
        let path = unique_temp_db_path("older-version");

        // Simulate a version-1 database: only the pre-occurrence tables,
        // stamped with version 1.
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE canonical_traces ( \
                         trace_id TEXT PRIMARY KEY NOT NULL, \
                         canonical_json BLOB NOT NULL \
                     );
                     CREATE TABLE raw_carrier_records ( \
                         raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT, \
                         trace_id TEXT NOT NULL, \
                         carrier TEXT NOT NULL, \
                         raw_bytes BLOB NOT NULL \
                     );
                     INSERT INTO canonical_traces VALUES ('cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', X'7b7d');
                     INSERT INTO raw_carrier_records (trace_id, carrier, raw_bytes) VALUES \
                         ('cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', 'codex', X'7631');
                     PRAGMA user_version = 1;",
                )
                .unwrap();
        }

        // Opening with this build must add `occurrences`, rebuild the raw
        // table into its v4 shape, and advance the marker without disturbing
        // the existing tables.
        let mut store = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&store), SCHEMA_VERSION);
        let legacy_id =
            TraceId::from_str("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
                .unwrap();
        assert_eq!(
            store.fetch_raw_carrier_records(&legacy_id).unwrap()[0].bytes(),
            b"v1"
        );
        let core = semantic(r#"{"kind":"intent","content":"post-migration capture"}"#);
        store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-a", b"post-migration"),
                occurrence("session-a", 0),
            )
            .unwrap();
        assert_eq!(occurrence_count(&store), 1);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn migrating_a_pre_v3_database_reports_its_unlinked_raw_rows_rather_than_hiding_them() {
        let path = unique_temp_db_path("pre-v3-unlinked");

        // Simulate a version-2 database: occurrences already exist, but
        // raw_carrier_records predates the session/sequence link columns —
        // exactly the shape every real store had before this schema
        // version, and the shape `ensure_raw_carrier_link_columns` exists
        // to add columns to via `ALTER TABLE`, since `CREATE TABLE IF NOT
        // EXISTS` no-ops against an already-existing table.
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE canonical_traces ( \
                         trace_id TEXT PRIMARY KEY NOT NULL, \
                         canonical_json BLOB NOT NULL \
                     );
                     CREATE TABLE raw_carrier_records ( \
                         raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT, \
                         trace_id TEXT NOT NULL, \
                         carrier TEXT NOT NULL, \
                         raw_bytes BLOB NOT NULL \
                     );
                     CREATE TABLE occurrences ( \
                         occurrence_id INTEGER PRIMARY KEY AUTOINCREMENT, \
                         trace_id TEXT NOT NULL, \
                         carrier TEXT NOT NULL, \
                         session TEXT NOT NULL, \
                         sequence INTEGER NOT NULL, \
                         timestamp INTEGER NOT NULL, \
                         repo TEXT NOT NULL, \
                         repo_source TEXT NOT NULL, \
                         parent_sequence INTEGER, \
                         agent TEXT, \
                         authored_by TEXT NOT NULL, \
                         UNIQUE (carrier, session, sequence) \
                     );
                     INSERT INTO canonical_traces VALUES ('aa11223344556677889900112233445566778899001122334455667788990011', X'7b7d');
                     INSERT INTO raw_carrier_records (trace_id, carrier, raw_bytes) \
                         VALUES ('aa11223344556677889900112233445566778899001122334455667788990011', 'codex', X'6c6567616379');
                     PRAGMA user_version = 2;",
                )
                .unwrap();
        }

        let mut store = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&store), SCHEMA_VERSION);

        // The pre-existing raw row cannot be backfilled with a link (its
        // trace_id alone can't tell us which occurrence it belonged to), so
        // it must show up as unlinked rather than silently vanish from
        // both counts.
        assert_eq!(store.count_unlinked_raw_records().unwrap(), 1);
        assert_eq!(raw_record_count(&store), 1);

        // Neither --session nor --before can reach it...
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Session("any-session"))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Before(i64::MAX))
                .unwrap(),
            0
        );
        // ...but --trace still can, since it matches by trace_id, not the link.
        let legacy_id =
            TraceId::from_str("aa11223344556677889900112233445566778899001122334455667788990011")
                .unwrap();
        assert_eq!(
            store.fetch_raw_carrier_records(&legacy_id).unwrap()[0].bytes(),
            b"legacy"
        );
        assert_eq!(
            store
                .count_forensic_forget(ForgetSelector::Trace(&legacy_id))
                .unwrap(),
            1
        );

        // A fresh capture after migration is linked immediately, and is not
        // counted as unlinked.
        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"post-migration, linked"}"#),
                NewRawCarrierRecord::new("codex", b"linked bytes"),
                occurrence("session-a", 0),
            )
            .unwrap();
        assert_eq!(store.count_unlinked_raw_records().unwrap(), 1);
        assert_eq!(raw_record_count(&store), 2);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_a_database_with_an_unsupported_higher_version_is_an_error() {
        let path = unique_temp_db_path("future-version");

        // First open creates the file and stamps it with this build's
        // schema version.
        let store = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&store), SCHEMA_VERSION);

        // Simulate a database already migrated by a future build.
        store
            .connection
            .execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .unwrap();
        drop(store);

        // Reopening must refuse rather than silently proceed: this build
        // cannot know what version-dependent behavior it is missing.
        let error = match TraceStore::open(&path) {
            Ok(_) => panic!("opening a newer-than-known database must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::UnsupportedSchemaVersion {
                found,
                supported,
            } if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ));

        // And it must not have touched the file: the marker this build did
        // not write stays exactly as found.
        let raw_version: u32 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw_version, SCHEMA_VERSION + 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn upsert_on_natural_key_updates_rather_than_duplicates() {
        let first_core = semantic(r#"{"kind":"intent","content":"first pass"}"#);
        let second_core = semantic(r#"{"kind":"intent","content":"second pass"}"#);
        let mut store = TraceStore::open_in_memory().unwrap();

        store
            .capture(
                &first_core,
                NewRawCarrierRecord::new("codex", b"pass one"),
                NewOccurrence {
                    timestamp: 100,
                    repo: "repo-one",
                    repo_source: RepoSource::GitRemote,
                    ..occurrence("session-a", 0)
                },
            )
            .unwrap();
        store
            .capture(
                &second_core,
                NewRawCarrierRecord::new("codex", b"pass two"),
                NewOccurrence {
                    timestamp: 200,
                    repo: "repo-two",
                    repo_source: RepoSource::CwdBasename,
                    ..occurrence("session-a", 0)
                },
            )
            .unwrap();

        assert_eq!(occurrence_count(&store), 1, "same key must not duplicate");
        assert_eq!(
            raw_record_count(&store),
            2,
            "rewritten bytes must remain as separate raw rows"
        );

        let rows = store.log(&LogFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].trace_id(), &second_core.trace_id().unwrap());
        assert_eq!(rows[0].timestamp(), 200);
        assert_eq!(rows[0].repo(), "repo-two");
        assert_eq!(rows[0].repo_source(), RepoSource::CwdBasename);
    }

    #[test]
    fn all_four_repo_sources_are_reachable() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let cases = [
            ("git-remote-repo", RepoSource::GitRemote),
            ("cwd-basename-repo", RepoSource::CwdBasename),
            ("override-repo", RepoSource::ExplicitOverride),
            ("", RepoSource::None),
        ];
        for (index, (repo, source)) in cases.iter().enumerate() {
            let core = semantic(&format!(r#"{{"kind":"intent","content":"case {index}"}}"#));
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"bytes"),
                    NewOccurrence {
                        repo,
                        repo_source: *source,
                        ..occurrence("session-a", index as i64)
                    },
                )
                .unwrap();
        }

        let rows = store.log(&LogFilter::default()).unwrap();
        let sources = rows
            .iter()
            .map(OccurrenceRecord::repo_source)
            .collect::<Vec<_>>();
        for (_, source) in cases {
            assert!(sources.contains(&source), "missing {source} in {rows:?}");
        }
    }

    #[test]
    fn parent_sequence_chains_and_is_null_for_the_first_occurrence() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let sequence_parents: [(i64, Option<i64>); 3] = [(0, None), (1, Some(0)), (2, Some(1))];
        for (sequence, parent_sequence) in sequence_parents {
            let core = semantic(&format!(
                r#"{{"kind":"intent","content":"seq {sequence}"}}"#
            ));
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"bytes"),
                    NewOccurrence {
                        parent_sequence,
                        ..occurrence("session-a", sequence)
                    },
                )
                .unwrap();
        }

        let rows = store.log(&LogFilter::default()).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].parent_sequence(), None);
        assert_eq!(rows[1].parent_sequence(), Some(0));
        assert_eq!(rows[2].parent_sequence(), Some(1));
    }

    #[test]
    fn fetch_raw_carrier_records_for_occurrences_selects_by_session() {
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"session a, seq 0"}"#),
                NewRawCarrierRecord::new("codex", b"a-0"),
                occurrence("session-a", 0),
            )
            .unwrap();
        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"session b, seq 0"}"#),
                NewRawCarrierRecord::new("codex", b"b-0"),
                occurrence("session-b", 0),
            )
            .unwrap();

        let records = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-a"))
            .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].bytes(), b"a-0");
    }

    #[test]
    fn fetch_raw_carrier_records_for_occurrences_selects_by_timestamp_range() {
        let mut store = TraceStore::open_in_memory().unwrap();
        for (sequence, timestamp) in [(0, 10), (1, 20), (2, 30)] {
            store
                .capture(
                    &semantic(&format!(
                        r#"{{"kind":"intent","content":"seq {sequence}"}}"#
                    )),
                    NewRawCarrierRecord::new("codex", format!("bytes-{sequence}").as_bytes()),
                    NewOccurrence {
                        timestamp,
                        ..occurrence("session-a", sequence)
                    },
                )
                .unwrap();
        }

        let records = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::TimestampRange(10, 30))
            .unwrap();

        let bytes: Vec<&[u8]> = records.iter().map(RawCarrierRecord::bytes).collect();
        assert_eq!(bytes, vec![b"bytes-0".as_slice(), b"bytes-1".as_slice()]);
    }

    #[test]
    fn fetch_raw_carrier_records_for_occurrences_emits_by_capture_order_not_grouped_by_trace() {
        // A trace ("alpha") occurs twice, interleaved with two other traces,
        // in this capture order: alpha, beta, alpha (again), gamma. This is
        // the shape that breaks a "select distinct trace ids, then fetch
        // each trace's records as a block" implementation: grouping by
        // trace would emit alpha's two records back to back at the position
        // of its first occurrence, silently moving beta's record to after
        // both of them. Emission must instead follow raw_record_id
        // (capture) order exactly — which is what makes alpha's own two
        // records land apart, with beta's record between them, rather than
        // adjacent.
        let mut store = TraceStore::open_in_memory().unwrap();
        let alpha = semantic(r#"{"kind":"intent","content":"alpha content"}"#);
        store
            .capture(
                &alpha,
                NewRawCarrierRecord::new("codex", b"alpha-first"),
                occurrence("session-a", 0),
            )
            .unwrap();
        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"beta content"}"#),
                NewRawCarrierRecord::new("codex", b"beta"),
                occurrence("session-a", 1),
            )
            .unwrap();
        store
            .capture(
                &alpha,
                NewRawCarrierRecord::new("codex", b"alpha-second"),
                occurrence("session-a", 2),
            )
            .unwrap();
        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"gamma content"}"#),
                NewRawCarrierRecord::new("codex", b"gamma"),
                occurrence("session-a", 3),
            )
            .unwrap();

        let records = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-a"))
            .unwrap();

        let bytes: Vec<&[u8]> = records.iter().map(RawCarrierRecord::bytes).collect();
        assert_eq!(
            bytes,
            vec![
                b"alpha-first".as_slice(),
                b"beta".as_slice(),
                b"alpha-second".as_slice(),
                b"gamma".as_slice(),
            ],
            "emission must follow capture order, not group each trace's records together"
        );

        let first = bytes
            .iter()
            .position(|value| *value == b"alpha-first".as_slice())
            .unwrap();
        let second = bytes
            .iter()
            .position(|value| *value == b"alpha-second".as_slice())
            .unwrap();
        assert_ne!(
            second,
            first + 1,
            "alpha's two records must not be contiguous: beta's record belongs between them"
        );
    }

    #[test]
    fn timestamp_span_covers_all_occurrences_or_only_one_repos() {
        let mut store = TraceStore::open_in_memory().unwrap();
        assert_eq!(store.timestamp_span(None).unwrap(), None);

        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"repo-a early"}"#),
                NewRawCarrierRecord::new("codex", b"repo-a-early"),
                NewOccurrence {
                    timestamp: 100,
                    repo: "repo-a",
                    repo_source: RepoSource::GitRemote,
                    authored_by: AuthoredBy::Human,
                    ..occurrence("session-a", 0)
                },
            )
            .unwrap();
        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"repo-b late"}"#),
                NewRawCarrierRecord::new("codex", b"repo-b-late"),
                NewOccurrence {
                    timestamp: 900,
                    repo: "repo-b",
                    repo_source: RepoSource::GitRemote,
                    authored_by: AuthoredBy::Human,
                    ..occurrence("session-b", 0)
                },
            )
            .unwrap();

        assert_eq!(store.timestamp_span(None).unwrap(), Some((100, 900)));
        assert_eq!(
            store.timestamp_span(Some("repo-a")).unwrap(),
            Some((100, 100))
        );
        assert_eq!(
            store.timestamp_span(Some("repo-nonexistent")).unwrap(),
            None
        );
    }

    #[test]
    fn log_renders_identically_after_the_raw_region_is_dropped() {
        let mut store = TraceStore::open_in_memory().unwrap();
        for index in 0..3 {
            let core = semantic(&format!(
                r#"{{"kind":"intent","content":"dropped raw case {index}"}}"#
            ));
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", format!("raw-{index}").as_bytes()),
                    NewOccurrence {
                        timestamp: index,
                        repo: "repo-a",
                        repo_source: RepoSource::GitRemote,
                        authored_by: AuthoredBy::Human,
                        ..occurrence("session-a", index)
                    },
                )
                .unwrap();
        }
        store
            .capture_raw_only(
                "codex",
                b"raw-only line with no occurrence",
                "session-a",
                99,
                99,
            )
            .unwrap();
        assert_eq!(raw_record_count(&store), 4);
        assert_eq!(occurrence_count(&store), 3);

        let render = |store: &TraceStore| -> String {
            store
                .log(&LogFilter::default())
                .unwrap()
                .iter()
                .map(crate::render_occurrence_line)
                .collect::<Vec<_>>()
                .join("\n")
        };

        let before = render(&store);
        assert!(!before.is_empty());

        // Drop the table entirely rather than emptying it. An emptied table
        // still lets a stray join silently return fewer rows — and, worse,
        // two independently-broken empty outputs can compare equal by
        // accident, which is a control that cannot fail on the property it
        // exists to test. Dropping the table turns any read of it, direct or
        // via a join, into a hard `no such table` error: the gate becomes
        // "cannot even be expressed", not merely "happens to produce the
        // same output".
        store
            .connection
            .execute("DROP TABLE raw_carrier_records", [])
            .unwrap();

        let after = render(&store);
        assert_eq!(
            before, after,
            "the log must render byte-for-byte identically once raw is dropped"
        );
    }

    /// The mirror of the test above: log and canonical reads must survive dropping
    /// `raw_carrier_records` intact, while both of the store's raw-reading
    /// methods — the ones `semon forensic` is built on — must fail hard
    /// rather than silently return nothing.
    ///
    /// This has to run against a single, never-reopened connection, exactly
    /// like the test above: [`TraceStore::open`] unconditionally re-runs the
    /// additive schema (every statement is `CREATE ... IF NOT EXISTS`) on
    /// every open, so a `DROP TABLE` made through a separate connection —
    /// which is what happens if this were driven through `semon log` /
    /// `semon forensic` as separate process-level
    /// invocations — gets silently healed (as an empty table) the moment
    /// the *next* command opens the store, before that command's own query
    /// ever runs. That is a real, load-bearing property of `open` (it is
    /// what lets an older on-disk schema upgrade in place), not a test bug;
    /// it just means this boundary can only be observed within one
    /// connection's lifetime, same as the test above.
    #[test]
    fn raw_reads_fail_but_log_and_canonical_reads_survive_dropping_the_raw_region() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let core = semantic(r#"{"kind":"intent","content":"boundary case"}"#);
        let trace_id = store
            .capture(
                &core,
                NewRawCarrierRecord::new("codex", b"boundary-bytes"),
                NewOccurrence {
                    repo: "repo-a",
                    repo_source: RepoSource::GitRemote,
                    authored_by: AuthoredBy::Human,
                    ..occurrence("session-a", 0)
                },
            )
            .unwrap()
            .trace_id()
            .clone();

        let before_log = store.log(&LogFilter::default()).unwrap();
        assert_eq!(before_log.len(), 1);
        let before_canonical = store.list_traces(None, 10).unwrap();
        assert_eq!(before_canonical.len(), 1);

        store
            .connection
            .execute("DROP TABLE raw_carrier_records", [])
            .unwrap();

        // Ordinary log and canonical reads stay exactly as they were.
        let after_log = store.log(&LogFilter::default()).unwrap();
        assert_eq!(before_log, after_log);
        let after_canonical = store.list_traces(None, 10).unwrap();
        assert_eq!(before_canonical, after_canonical);

        // Both of the reads `semon forensic` is built on must fail loudly —
        // not return an empty, silently-wrong result.
        assert!(
            store.fetch_raw_carrier_records(&trace_id).is_err(),
            "fetch_raw_carrier_records must fail once its table is dropped"
        );
        assert!(
            store
                .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-a"))
                .is_err(),
            "fetch_raw_carrier_records_for_occurrences must fail once its table is dropped"
        );
    }

    fn raw_record_count(store: &TraceStore) -> i64 {
        store
            .connection
            .query_row("SELECT count(*) FROM raw_carrier_records", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn forget_forensic_by_trace_deletes_only_that_traces_raw_rows() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let kept = semantic(r#"{"kind":"intent","content":"kept trace"}"#);
        let forgotten = semantic(r#"{"kind":"intent","content":"forgotten trace"}"#);
        let forgotten_id = forgotten.trace_id().unwrap();
        store
            .capture(
                &kept,
                NewRawCarrierRecord::new("codex", b"kept bytes"),
                occurrence("session-a", 0),
            )
            .unwrap();
        store
            .capture(
                &forgotten,
                NewRawCarrierRecord::new("codex", b"forgotten bytes"),
                occurrence("session-a", 1),
            )
            .unwrap();

        let count = store
            .count_forensic_forget(ForgetSelector::Trace(&forgotten_id))
            .unwrap();
        assert_eq!(count, 1);

        let deleted = store
            .forget_forensic(ForgetSelector::Trace(&forgotten_id))
            .unwrap();
        assert_eq!(deleted, 1);

        assert_eq!(raw_record_count(&store), 1);
        assert!(
            store
                .fetch_raw_carrier_records(&forgotten_id)
                .unwrap()
                .is_empty()
        );
        // canonical_traces and occurrences are untouched by forget.
        assert_eq!(counts(&store), (2, 1));
        assert_eq!(occurrence_count(&store), 2);
        assert_eq!(store.log(&LogFilter::default()).unwrap().len(), 2);
    }

    #[test]
    fn forget_forensic_by_session_leaves_the_same_trace_in_another_session_intact() {
        // The failure this test is shaped around: a trace shared between two
        // sessions used to be selected by trace_id, so forgetting one
        // session's raw records also deleted the *other* session's raw
        // record for that same content — measured on a real week of capture
        // as a 4.5x over-delete. `session`/`sequence` link each raw row to
        // its own capture, so this must no longer happen.
        let shared = semantic(r#"{"kind":"intent","content":"shared across two sessions"}"#);
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture(
                &shared,
                NewRawCarrierRecord::new("codex", b"session-a's own bytes"),
                occurrence("session-a", 0),
            )
            .unwrap();
        store
            .capture(
                &shared,
                NewRawCarrierRecord::new("codex", b"session-b's own bytes"),
                occurrence("session-b", 0),
            )
            .unwrap();
        // An unrelated trace in session-a, to prove the whole session is
        // reachable, not just the shared one.
        store
            .capture(
                &semantic(r#"{"kind":"intent","content":"session-a only"}"#),
                NewRawCarrierRecord::new("codex", b"session-a-only bytes"),
                occurrence("session-a", 1),
            )
            .unwrap();

        let count = store
            .count_forensic_forget(ForgetSelector::Session("session-a"))
            .unwrap();
        assert_eq!(count, 2, "exactly session-a's own two raw rows");

        let deleted = store
            .forget_forensic(ForgetSelector::Session("session-a"))
            .unwrap();
        assert_eq!(deleted, 2);

        // Session-b's raw record for the shared trace must survive, field
        // for field — not merely "some row remains".
        let shared_id = shared.trace_id().unwrap();
        let remaining = store.fetch_raw_carrier_records(&shared_id).unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "session-b's copy of the shared trace's raw record must survive"
        );
        assert_eq!(remaining[0].bytes(), b"session-b's own bytes");
        assert_eq!(remaining[0].trace_ids(), &[shared_id]);

        assert_eq!(raw_record_count(&store), 1);
        // occurrences and canonical traces are never touched by forget.
        assert_eq!(occurrence_count(&store), 3);
        assert_eq!(counts(&store).0, 2);
    }

    #[test]
    fn forget_forensic_before_deletes_the_pre_cutoff_capture_and_leaves_the_later_one() {
        // The failure this test is shaped around: "before" used to require
        // *every* occurrence of a trace to predate the cutoff, so a trace
        // that recurred after the cutoff kept its raw record from *before*
        // the cutoff too — measured on a real week as 52% of eligible
        // captures surviving while the command reported success. Deletion
        // must be per capture (the raw row's own linked occurrence), not
        // per trace.
        let recurring = semantic(r#"{"kind":"intent","content":"recurring content"}"#);
        let cutoff = 1_000;
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture(
                &recurring,
                NewRawCarrierRecord::new("codex", b"pre-cutoff capture"),
                NewOccurrence {
                    timestamp: cutoff - 1,
                    ..occurrence("session-a", 0)
                },
            )
            .unwrap();
        store
            .capture(
                &recurring,
                NewRawCarrierRecord::new("codex", b"post-cutoff capture"),
                NewOccurrence {
                    timestamp: cutoff,
                    ..occurrence("session-a", 1)
                },
            )
            .unwrap();

        let count = store
            .count_forensic_forget(ForgetSelector::Before(cutoff))
            .unwrap();
        assert_eq!(count, 1, "only the pre-cutoff capture counts");

        let deleted = store
            .forget_forensic(ForgetSelector::Before(cutoff))
            .unwrap();
        assert_eq!(deleted, 1);

        let recurring_id = recurring.trace_id().unwrap();
        let remaining = store.fetch_raw_carrier_records(&recurring_id).unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "the post-cutoff capture of the same trace must survive"
        );
        assert_eq!(remaining[0].bytes(), b"post-cutoff capture");

        // occurrences and canonical traces are never touched by forget.
        assert_eq!(occurrence_count(&store), 2);
        assert_eq!(counts(&store).0, 1);
    }

    #[test]
    fn fetch_raw_carrier_records_for_occurrences_by_session_excludes_a_shared_traces_other_session()
    {
        // The read-side mirror of the over-delete bug above: `semon
        // forensic --session` used to widen to every raw row sharing a
        // matched trace's trace_id, so it over-*read* another session's
        // forensic bytes. Same shared-trace fixture, this time checking the
        // read path.
        let shared = semantic(r#"{"kind":"intent","content":"shared for forensic read"}"#);
        let mut store = TraceStore::open_in_memory().unwrap();
        store
            .capture(
                &shared,
                NewRawCarrierRecord::new("codex", b"session-a's forensic bytes"),
                occurrence("session-a", 0),
            )
            .unwrap();
        store
            .capture(
                &shared,
                NewRawCarrierRecord::new("codex", b"session-b's forensic bytes"),
                occurrence("session-b", 0),
            )
            .unwrap();

        let session_a_records = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-a"))
            .unwrap();
        assert_eq!(session_a_records.len(), 1);
        assert_eq!(session_a_records[0].bytes(), b"session-a's forensic bytes");

        let session_b_records = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-b"))
            .unwrap();
        assert_eq!(session_b_records.len(), 1);
        assert_eq!(session_b_records[0].bytes(), b"session-b's forensic bytes");
    }

    #[test]
    fn forget_forensic_overwrites_deleted_bytes_on_disk() {
        // The representation that matters is the bytes of the file, not a
        // row count: a row-count assertion would pass even if the deleted
        // content merely sat, still readable, on SQLite's freelist. This
        // test captures a record containing a unique sentinel, forgets it,
        // closes the store, and asserts the sentinel is absent from the raw
        // database file bytes — proving `PRAGMA secure_delete = ON` plus
        // `VACUUM` actually removed it from disk rather than just from the
        // active b-tree.
        const SENTINEL: &[u8] = b"SEMON-FORGET-SENTINEL-8f3a";

        let path = unique_temp_db_path("forget-sentinel");
        let trace_id = {
            let mut store = TraceStore::open(&path).unwrap();
            let core = semantic(r#"{"kind":"intent","content":"sentinel-bearing capture"}"#);
            let trace_id = core.trace_id().unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", SENTINEL),
                    occurrence("session-a", 0),
                )
                .unwrap();
            drop(store);
            trace_id
        };

        let before_bytes = std::fs::read(&path).unwrap();
        assert!(
            before_bytes
                .windows(SENTINEL.len())
                .any(|window| window == SENTINEL),
            "test setup failure: sentinel must be present in the raw file bytes before forget"
        );

        {
            let mut store = TraceStore::open(&path).unwrap();
            let deleted = store
                .forget_forensic(ForgetSelector::Trace(&trace_id))
                .unwrap();
            assert_eq!(deleted, 1);
            drop(store);
        }

        let after_bytes = std::fs::read(&path).unwrap();
        assert!(
            !after_bytes
                .windows(SENTINEL.len())
                .any(|window| window == SENTINEL),
            "sentinel must be absent from the raw file bytes after forget: a plain DELETE \
             would leave it readable in a freed page"
        );

        let _ = std::fs::remove_file(&path);
    }
}

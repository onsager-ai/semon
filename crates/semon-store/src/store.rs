use std::{path::Path, str::FromStr};

use rusqlite::{Connection, OptionalExtension, ToSql, TransactionBehavior, params};
use thiserror::Error;

use crate::{
    AuthoredBy, CanonicalTrace, CaptureResult, LogFilter, NewOccurrence, NewRawCarrierRecord,
    OccurrenceRecord, RawCarrierRecord, RepoSource, SemanticCore, TraceId, canonical,
};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS canonical_traces (
    trace_id       TEXT PRIMARY KEY NOT NULL CHECK(length(trace_id) = 64),
    canonical_json BLOB NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS raw_carrier_records (
    raw_record_id INTEGER PRIMARY KEY AUTOINCREMENT,
    trace_id      TEXT NOT NULL REFERENCES canonical_traces(trace_id),
    carrier       TEXT NOT NULL CHECK(length(trim(carrier)) > 0),
    raw_bytes     BLOB NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS raw_carrier_records_by_trace
    ON raw_carrier_records(trace_id, raw_record_id);

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

/// The `PRAGMA user_version` this build understands, and the version it
/// stamps onto a newly created (or additively migrated) database.
///
/// This is a compile-time constant, never a caller-supplied value: `PRAGMA
/// user_version` cannot be bound as a parameter in SQLite, so the statement
/// that sets it is built with `format!`. Keeping the formatted value pinned
/// to this constant (instead of anything derived from a caller or the
/// database) is what keeps that `format!` call safe.
///
/// Two schema shapes exist now (1: `canonical_traces` + `raw_carrier_records`
/// only; 2: adds `occurrences`), and read behavior genuinely depends on which
/// one is on disk — [`TraceStore::log`] requires the `occurrences` table.
/// Opening a database stamped with a version *higher* than this build knows
/// (i.e. written by a future build) is therefore a hard error rather than
/// being silently accepted: this build cannot know what version-dependent
/// behavior that future schema implies, so proceeding could silently give
/// wrong answers instead of failing loudly. A version from 1 up to and
/// including `SCHEMA_VERSION` is migrated forward additively (every DDL
/// statement above is `CREATE ... IF NOT EXISTS`, so it only ever adds
/// structure) and the marker is advanced to `SCHEMA_VERSION`. A fresh
/// database (`user_version` 0, SQLite's default) is stamped with
/// `SCHEMA_VERSION` directly.
const SCHEMA_VERSION: u32 = 2;

/// Errors returned by the trace store.
#[derive(Debug, Error)]
pub enum StoreError {
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
}

/// A single-file SQLite trace store.
pub struct TraceStore {
    connection: Connection,
}

impl TraceStore {
    /// Opens or creates a store at `path` and initializes its schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_connection(Connection::open(path)?)
    }

    /// Creates an in-memory store, primarily useful to callers' tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StoreError> {
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

        // Every statement in `SCHEMA` is additive (`CREATE ... IF NOT
        // EXISTS`), so running it against a fresh database, an already
        // up-to-date one, or one still at an older known version is always
        // safe: it only ever adds structure.
        connection.execute_batch(SCHEMA)?;
        if user_version < SCHEMA_VERSION {
            connection.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }

        Ok(Self { connection })
    }

    /// Atomically writes a semantic trace, its raw carrier record, and the
    /// occurrence that observed it.
    ///
    /// The semantic core alone determines trace identity. If that identity
    /// already exists, the canonical insert is idempotent and this method
    /// still appends a new raw record for the new capture. The occurrence
    /// write is an UPSERT on `(carrier, session, sequence)`: a re-capture of
    /// the same source position updates that occurrence's fields in place
    /// rather than inserting a duplicate.
    pub fn capture(
        &mut self,
        semantic_core: &SemanticCore,
        raw_record: NewRawCarrierRecord<'_>,
        occurrence: NewOccurrence<'_>,
    ) -> Result<CaptureResult, StoreError> {
        if raw_record.carrier.trim().is_empty() {
            return Err(StoreError::BlankCarrier);
        }

        let canonical_json = semantic_core.canonical_json()?;
        let trace_id = canonical::content_hash(&canonical_json);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

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
            "INSERT INTO raw_carrier_records (trace_id, carrier, raw_bytes) \
             VALUES (?1, ?2, ?3)",
            params![trace_id.as_str(), raw_record.carrier, raw_record.bytes],
        )?;
        let raw_record_id = transaction.last_insert_rowid();

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

        transaction.commit()?;

        Ok(CaptureResult::new(
            trace_id,
            canonical_inserted,
            raw_record_id,
        ))
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
    /// This is the only public read path that queries `raw_carrier_records`.
    /// Records are ordered by ascending local row id (capture insertion order).
    pub fn fetch_raw_carrier_records(
        &self,
        trace_id: &TraceId,
    ) -> Result<Vec<RawCarrierRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT raw_record_id, trace_id, carrier, raw_bytes \
             FROM raw_carrier_records WHERE trace_id = ?1 \
             ORDER BY raw_record_id ASC",
        )?;
        let mapped = statement.query_map([trace_id.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;

        let mut records = Vec::new();
        for row in mapped {
            let (id, stored_trace_id, carrier, bytes) = row?;
            records.push(RawCarrierRecord::new(
                id,
                TraceId::from_str(&stored_trace_id)?,
                carrier,
                bytes,
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

    #[test]
    fn fresh_database_is_stamped_with_current_schema_version() {
        let path = unique_temp_db_path("fresh-version");
        let store = TraceStore::open(&path).unwrap();

        assert_eq!(user_version(&store), SCHEMA_VERSION);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_a_database_with_an_older_known_version_migrates_it_additively() {
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
                     PRAGMA user_version = 1;",
                )
                .unwrap();
        }

        // Opening with this build must additively add `occurrences` and
        // advance the marker, without disturbing the existing tables.
        let mut store = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&store), SCHEMA_VERSION);
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
}

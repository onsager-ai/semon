use std::{path::Path, str::FromStr};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

use crate::{
    CanonicalTrace, CaptureResult, NewRawCarrierRecord, RawCarrierRecord, SemanticCore, TraceId,
    canonical,
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
"#;

/// The `PRAGMA user_version` this build stamps onto a newly created database.
///
/// This is a compile-time constant, never a caller-supplied value: `PRAGMA
/// user_version` cannot be bound as a parameter in SQLite, so the statement
/// that sets it is built with `format!`. Keeping the formatted value pinned
/// to this constant (instead of anything derived from a caller or the
/// database) is what keeps that `format!` call safe.
const SCHEMA_VERSION: u32 = 1;

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
        connection.execute_batch(SCHEMA)?;

        // `PRAGMA user_version` is the one field whose job is to say which
        // schema shape is on disk, so initialization must not clobber it.
        // A brand-new database (SQLite defaults `user_version` to 0) gets
        // stamped with the version this build writes. Any other value —
        // including one *higher* than `SCHEMA_VERSION`, meaning a future
        // build already touched this file — is left exactly as found.
        //
        // We do not turn "higher than known" into a hard error here: the DDL
        // above is purely additive (`CREATE ... IF NOT EXISTS`) and never
        // drops or rewrites existing structures, so merely opening a
        // newer-than-known database cannot corrupt it. This store currently
        // has exactly one real schema shape and no migration machinery, so
        // there is no version-dependent read/write behavior yet to protect
        // against. Once a second schema version exists with behavior that
        // actually depends on which version is on disk, opening a database
        // stamped with a version this build does not understand should
        // become a hard error instead of silently proceeding.
        let user_version: u32 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version == 0 {
            connection.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }

        Ok(Self { connection })
    }

    /// Atomically writes a semantic trace and its raw carrier record.
    ///
    /// The semantic core alone determines identity. If that identity already
    /// exists, the canonical insert is idempotent and this method still appends
    /// a new raw record for the new capture.
    pub fn capture(
        &mut self,
        semantic_core: &SemanticCore,
        raw_record: NewRawCarrierRecord<'_>,
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
        for (core, raw) in [
            (&first, b"synthetic-one".as_slice()),
            (&second, b"synthetic-two".as_slice()),
            (&third, b"synthetic-three".as_slice()),
        ] {
            store
                .capture(core, NewRawCarrierRecord::new("carrier-a", raw))
                .unwrap();
        }

        assert_eq!(counts(&store), (1, 3));
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
            )
            .unwrap();
        store
            .capture(&failed, NewRawCarrierRecord::new("carrier-a", b"record-b"))
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
            )
            .unwrap();
        let second = store
            .capture(
                &core,
                NewRawCarrierRecord::new("carrier-b", b"opaque variant two"),
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
            )
            .unwrap_err();

        assert!(matches!(error, StoreError::Sqlite(_)));
        assert_eq!(counts(&store), (0, 0));
    }

    #[test]
    fn repeat_capture_keeps_one_canonical_row_and_appends_raw_rows() {
        let core = semantic(r#"{"action":"inspect","result":"understood"}"#);
        let mut store = TraceStore::open_in_memory().unwrap();

        let first = store
            .capture(&core, NewRawCarrierRecord::new("carrier-a", b"same record"))
            .unwrap();
        let second = store
            .capture(&core, NewRawCarrierRecord::new("carrier-a", b"same record"))
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
        for core in &cores {
            store
                .capture(core, NewRawCarrierRecord::new("carrier-a", b"synthetic"))
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
    fn fresh_database_is_stamped_with_schema_version_one() {
        let path = unique_temp_db_path("fresh-version");
        let store = TraceStore::open(&path).unwrap();

        assert_eq!(user_version(&store), 1);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_a_database_with_a_higher_user_version_preserves_it() {
        let path = unique_temp_db_path("preserved-version");

        // First open creates the file and stamps it with this build's
        // schema version.
        let store = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&store), 1);

        // Simulate a database already migrated by a future build.
        store
            .connection
            .execute_batch("PRAGMA user_version = 2;")
            .unwrap();
        drop(store);

        // Reopening must not clobber the marker this init routine did not
        // write: it is the one field that says which schema is on disk.
        let reopened = TraceStore::open(&path).unwrap();
        assert_eq!(user_version(&reopened), 2);

        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }
}

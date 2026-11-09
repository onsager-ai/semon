//! Fixtures shared by the unit tests.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, TimestampMillisecondArray,
};
use arrow::record_batch::RecordBatch;

use arrow::util::display::array_value_to_string;

use crate::error::Result;
use crate::parquet_io::{int_column, read_batches, string_column};
use crate::schema::{
    FACT_COMPACTION, FACT_LABEL, FACT_SESSION, FACT_STEP, FACT_TOOL_CALL, FACT_TURN, Kind,
    SESSION_COLUMN, Table, Type,
};
use crate::store::{Clock, Manifest, Store, TableBatch, Update};

pub(crate) struct TestClock(AtomicI64);

impl TestClock {
    pub(crate) fn new(now_ms: i64) -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(now_ms)))
    }

    pub(crate) fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Rows for `table`: `sessions` gives each session and its row count. Text values
/// embed `tag`, the column, the session and the row, so a test can tell which
/// publish a row came from. `ts_base` offsets every time column.
pub(crate) fn sample(
    table: &Table,
    sessions: &[(&str, usize)],
    tag: &str,
    ts_base: i64,
) -> RecordBatch {
    let mut rows: Vec<(&str, usize)> = Vec::new();
    for (session, count) in sessions {
        rows.extend((0..*count).map(|row| (*session, row)));
    }
    let columns: Vec<ArrayRef> = table
        .columns
        .iter()
        .map(|column| -> ArrayRef {
            match column.data_type {
                Type::Text => Arc::new(StringArray::from_iter_values(rows.iter().map(
                    |(session, row)| {
                        if column.name == SESSION_COLUMN {
                            (*session).to_owned()
                        } else if let Some(value) = column.values.first() {
                            (*value).to_owned()
                        } else {
                            format!("{}:{session}:{row}:{tag}", column.name)
                        }
                    },
                ))),
                Type::Integer => Arc::new(Int64Array::from_iter_values(
                    rows.iter().map(|(_, row)| *row as i64),
                )),
                Type::Float => Arc::new(Float64Array::from(vec![1.5; rows.len()])),
                Type::Boolean => Arc::new(BooleanArray::from(vec![true; rows.len()])),
                Type::Timestamp => Arc::new(
                    TimestampMillisecondArray::from_iter_values(
                        rows.iter().map(|(_, row)| ts_base + *row as i64),
                    )
                    .with_timezone("UTC"),
                ),
            }
        })
        .collect();
    RecordBatch::try_new(Arc::new(table.arrow_schema()), columns).expect("sample batch")
}

/// An update that recomputes `sessions` whole: `fact_step` and `fact_session` rows.
pub(crate) fn update(sessions: &[(&str, usize)], tag: &str, ts_base: i64) -> Update {
    let summary: Vec<(&str, usize)> = sessions.iter().map(|(session, _)| (*session, 1)).collect();
    Update {
        tables: vec![
            TableBatch {
                table: &FACT_STEP,
                batch: sample(&FACT_STEP, sessions, tag, ts_base),
            },
            TableBatch {
                table: &FACT_SESSION,
                batch: sample(&FACT_SESSION, &summary, tag, ts_base),
            },
        ],
        touched_sessions: sessions
            .iter()
            .map(|(session, _)| (*session).to_owned())
            .collect(),
        ..Update::default()
    }
}

/// The live `fact_step` rows as `(session_uid, _gen, step_uid)`.
pub(crate) fn live_steps<C: Clock>(store: &Store<C>) -> BTreeSet<(String, i64, String)> {
    let mut rows = BTreeSet::new();
    for file in store.manifest().files_of(FACT_STEP.name) {
        for batch in read_batches(&store.analytics_dir().join(&file.path), None).unwrap() {
            let sessions = string_column(&batch, SESSION_COLUMN).unwrap();
            let generations = int_column(&batch, "_gen").unwrap();
            let steps = string_column(&batch, "step_uid").unwrap();
            for row in 0..batch.num_rows() {
                let session = sessions.value(row);
                let generation = generations.value(row);
                if store.live_map().get(session) == Some(&generation) {
                    rows.insert((session.to_owned(), generation, steps.value(row).to_owned()));
                }
            }
        }
    }
    rows
}

/// An update that recomputes `sessions` whole in every session table, and appends one
/// label row. `sessions` gives the step and tool-call rows per session; the other
/// tables get one row per session.
pub(crate) fn update_all(sessions: &[(&str, usize)], tag: &str, ts_base: i64) -> Update {
    let summary: Vec<(&str, usize)> = sessions.iter().map(|(session, _)| (*session, 1)).collect();
    let batch = |table: &'static Table, rows: &[(&str, usize)]| TableBatch {
        table,
        batch: sample(table, rows, tag, ts_base),
    };
    Update {
        tables: vec![
            batch(&FACT_STEP, sessions),
            batch(&FACT_TOOL_CALL, sessions),
            batch(&FACT_TURN, summary.as_slice()),
            batch(&FACT_SESSION, summary.as_slice()),
            batch(&FACT_COMPACTION, summary.as_slice()),
            batch(&FACT_LABEL, [("label", 1)].as_slice()),
        ],
        touched_sessions: sessions
            .iter()
            .map(|(session, _)| (*session).to_owned())
            .collect(),
        ..Update::default()
    }
}

/// `batch` with the column `name` replaced.
pub(crate) fn replace_column(batch: &RecordBatch, name: &str, array: ArrayRef) -> RecordBatch {
    let index = batch.schema().index_of(name).unwrap();
    let mut columns = batch.columns().to_vec();
    columns[index] = array;
    RecordBatch::try_new(batch.schema(), columns).unwrap()
}

/// Incompressible hex strings, `length` characters each.
pub(crate) fn noise(rows: usize, length: usize, mut seed: u64) -> ArrayRef {
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let values: Vec<String> = (0..rows)
        .map(|_| {
            let mut text = String::with_capacity(length + 16);
            while text.len() < length {
                text.push_str(&format!("{:016x}", next()));
            }
            text.truncate(length);
            text
        })
        .collect();
    Arc::new(StringArray::from(values))
}

/// Every row of `table` that `manifest` and `live` name, as a multiset of all column
/// values (including `_gen`). Session tables count live rows only.
pub(crate) fn rows_of(
    analytics: &Path,
    manifest: &Manifest,
    live: &HashMap<String, i64>,
    table: &Table,
) -> Result<BTreeMap<String, usize>> {
    let mut rows = BTreeMap::new();
    for file in manifest.files_of(table.name) {
        for batch in read_batches(&analytics.join(&file.path), None)? {
            for row in 0..batch.num_rows() {
                if table.kind == Kind::Session {
                    let session = string_column(&batch, SESSION_COLUMN)?.value(row);
                    if live.get(session) != Some(&int_column(&batch, "_gen")?.value(row)) {
                        continue;
                    }
                }
                let mut text = String::new();
                for column in batch.columns() {
                    // Formatting a time zone needs chrono-tz; the raw milliseconds do.
                    match column.as_any().downcast_ref::<TimestampMillisecondArray>() {
                        Some(times) => text.push_str(&times.value(row).to_string()),
                        None => text.push_str(&array_value_to_string(column, row).unwrap()),
                    }
                    text.push('|');
                }
                *rows.entry(text).or_insert(0) += 1;
            }
        }
    }
    Ok(rows)
}

/// `rows_of` for every table of the store's current manifest.
pub(crate) fn all_rows<C: Clock>(
    store: &Store<C>,
    tables: &[&'static Table],
) -> Vec<BTreeMap<String, usize>> {
    tables
        .iter()
        .map(|table| {
            rows_of(
                store.analytics_dir(),
                store.manifest(),
                store.live_map(),
                table,
            )
            .unwrap()
        })
        .collect()
}

/// Every fact table: the session tables and the labels.
pub(crate) static FACT_TABLES: [&Table; 6] = [
    &FACT_STEP,
    &FACT_TOOL_CALL,
    &FACT_TURN,
    &FACT_SESSION,
    &FACT_COMPACTION,
    &FACT_LABEL,
];

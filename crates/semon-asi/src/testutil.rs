//! Fixtures shared by the unit tests.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, TimestampMillisecondArray,
};
use arrow::record_batch::RecordBatch;

use crate::parquet_io::{int_column, read_batches, string_column};
use crate::schema::{FACT_SESSION, FACT_STEP, SESSION_COLUMN, Table, Type};
use crate::store::{Clock, Store, TableBatch, Update};

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

//! One atomic origin publication cycle.
//!
//! The input has already admitted and priced a complete session generation. This
//! module only projects that evidence, retains frozen rows, and hands one
//! [`crate::store::Update`] to the generation store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, TimestampMillisecondArray,
};
use arrow::record_batch::RecordBatch;

use crate::error::Error;
use crate::input::{ANALYTICS_TRANSFORM_VERSION, AnalyticsInput, SessionChange};
use crate::parquet_io::read_batches;
use crate::projection::{Cell, Row, project};
use crate::schema::{Kind, TABLES, Table, Type};
use crate::store::{Clock, SourceMissing, Store, TableBatch, Update, Versions};

/// The error from reading an input or writing its projected generation.
#[derive(Debug)]
pub enum PublishError<E> {
    Input(E),
    Store(Error),
}

impl<E: std::fmt::Display> std::fmt::Display for PublishError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(error) => write!(f, "analytics input: {error}"),
            Self::Store(error) => error.fmt(f),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for PublishError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Input(error) => Some(error),
            Self::Store(error) => Some(error),
        }
    }
}

/// Publishes one coherent input batch as one store generation.
///
/// `versions` are the writer's current targets. They are manifest metadata,
/// not a claim that every retained session has those recorded versions. The
/// transform target is always the current pure projection version.
pub fn publish_cycle<C: Clock, I: AnalyticsInput>(
    store: &mut Store<C>,
    input: &mut I,
    namespace: &str,
    mut versions: Versions,
) -> std::result::Result<u64, PublishError<I::Error>> {
    versions.transform = ANALYTICS_TRANSFORM_VERSION;
    let checkpoint = store
        .manifest()
        .checkpoints
        .get(namespace)
        .map(String::as_str);
    let batch = input
        .read_admitted(checkpoint)
        .map_err(PublishError::Input)?;
    if batch.namespace != namespace {
        return Err(PublishError::Store(Error::invalid(
            "input returned a batch for another namespace",
        )));
    }
    update_for_batch(store, batch, versions)
        .and_then(|update| store.publish(&update))
        .map_err(PublishError::Store)
}

fn update_for_batch<C: Clock>(
    store: &Store<C>,
    batch: crate::input::AdmittedBatch,
    versions: Versions,
) -> crate::Result<Update> {
    let mut facts = BTreeMap::<&'static str, Vec<Row>>::new();
    let mut dimensions = BTreeMap::<&'static str, Vec<Row>>::new();
    let mut touched = BTreeSet::new();
    let mut removed = BTreeSet::new();
    let mut source_missing = BTreeMap::new();
    let mut source_restored = BTreeSet::new();

    for change in batch.changes {
        match change {
            SessionChange::Admitted(session) => {
                if !touched.insert(session.session_uid.clone()) {
                    return Err(Error::invalid("input admits a session more than once"));
                }
                let projected = project(&session)?;
                for (name, rows) in projected.tables {
                    let table = crate::schema::table(name).ok_or_else(|| {
                        Error::invalid(format!("unknown projection table {name}"))
                    })?;
                    match table.kind {
                        Kind::Session => facts.entry(name).or_default().extend(rows),
                        Kind::Dimension => dimensions.entry(name).or_default().extend(rows),
                        Kind::Append => {}
                    }
                }
            }
            SessionChange::Pending { .. } => {}
            SessionChange::SourceMissing {
                session_uid,
                retained_at_origin: _,
            } if store.live_map().contains_key(&session_uid) => {
                source_missing.insert(
                    session_uid,
                    SourceMissing {
                        detected_at: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_or(0, |elapsed| elapsed.as_millis() as i64),
                    },
                );
            }
            SessionChange::SourceMissing { .. } => {}
            SessionChange::SourceRestored { session_uid } => {
                source_restored.insert(session_uid);
            }
            SessionChange::Removed { session_uid } => {
                removed.insert(session_uid);
            }
        }
    }

    if touched.iter().any(|session| removed.contains(session)) {
        return Err(Error::invalid("input both admits and removes a session"));
    }
    for session in &removed {
        source_missing.remove(session);
        source_restored.remove(session);
    }

    let mut tables = Vec::new();
    for table in TABLES {
        if table.kind == Kind::Session {
            if let Some(rows) = facts.remove(table.name) {
                tables.push(TableBatch {
                    table,
                    batch: rows_to_batch(table, &rows)?,
                });
            }
        }
    }
    for table in TABLES {
        if table.kind != Kind::Dimension {
            continue;
        }
        let rows = merged_dimension_rows(store, table, dimensions.remove(table.name))?;
        if let Some(rows) = rows {
            tables.push(TableBatch {
                table,
                batch: rows_to_batch(table, &rows)?,
            });
        }
    }

    let mut checkpoints = BTreeMap::new();
    checkpoints.insert(batch.namespace, batch.checkpoint);
    Ok(Update {
        tables,
        touched_sessions: touched.into_iter().collect(),
        removed_sessions: removed.into_iter().collect(),
        source_missing,
        source_restored: source_restored.into_iter().collect(),
        checkpoints,
        durable_revision: store.manifest().durable_revision,
        versions,
    })
}

/// Returns `None` when this dimension table is byte-for-byte unchanged in row
/// order. Existing rows are retained so frozen sessions keep their references.
fn merged_dimension_rows<C: Clock>(
    store: &Store<C>,
    table: &'static Table,
    added: Option<Vec<Row>>,
) -> crate::Result<Option<Vec<Row>>> {
    let Some(added) = added else {
        return Ok(None);
    };
    let mut current = dimension_rows(store, table)?;
    let additions = deduplicate_dimension_rows(table, added)?;
    for row in additions {
        match dimension_row_position(table, &current, &row)? {
            Some(index) if current[index] == row => {}
            Some(index) => current[index] = row,
            None => current.push(row),
        }
    }
    Ok(Some(current))
}

fn dimension_rows<C: Clock>(store: &Store<C>, table: &'static Table) -> crate::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for file in store.manifest().files_of(table.name) {
        for batch in read_batches(&store.analytics_dir().join(&file.path), None)? {
            rows.extend(batch_to_rows(table, &batch)?);
        }
    }
    Ok(rows)
}

fn deduplicate_dimension_rows(table: &Table, rows: Vec<Row>) -> crate::Result<Vec<Row>> {
    let mut unique = Vec::new();
    for row in rows {
        match dimension_row_position(table, &unique, &row)? {
            Some(index) if unique[index] == row => {}
            Some(_) => {
                return Err(Error::invalid(format!(
                    "conflicting {} dimension rows in one input batch",
                    table.name
                )));
            }
            None => unique.push(row),
        }
    }
    Ok(unique)
}

fn dimension_row_position(
    table: &Table,
    rows: &[Row],
    candidate: &Row,
) -> crate::Result<Option<usize>> {
    if table.name == "dim_price" {
        return Ok(rows.iter().position(|row| row == candidate));
    }
    let key = candidate
        .get(table.columns[0].name)
        .ok_or_else(|| Error::invalid(format!("missing {} key", table.name)))?;
    Ok(rows
        .iter()
        .position(|row| row.get(table.columns[0].name) == Some(key)))
}

fn rows_to_batch(table: &'static Table, rows: &[Row]) -> crate::Result<RecordBatch> {
    let columns: crate::Result<Vec<ArrayRef>> = table
        .columns
        .iter()
        .map(|column| {
            let cells: crate::Result<Vec<&Cell>> = rows
                .iter()
                .map(|row| {
                    row.get(column.name).ok_or_else(|| {
                        Error::invalid(format!("missing {}.{}", table.name, column.name))
                    })
                })
                .collect();
            cells_to_array(column.data_type, cells?)
        })
        .collect();
    Ok(RecordBatch::try_new(
        Arc::new(table.arrow_schema()),
        columns?,
    )?)
}

fn cells_to_array(data_type: Type, cells: Vec<&Cell>) -> crate::Result<ArrayRef> {
    match data_type {
        Type::Text => Ok(Arc::new(StringArray::from_iter(
            cells
                .into_iter()
                .map(|cell| match cell {
                    Cell::Text(value) => Ok(value.as_deref()),
                    _ => Err(Error::invalid("projection cell has the wrong type")),
                })
                .collect::<crate::Result<Vec<_>>>()?,
        ))),
        Type::Integer => Ok(Arc::new(Int64Array::from(
            cells
                .into_iter()
                .map(|cell| match cell {
                    Cell::Integer(value) => Ok(*value),
                    _ => Err(Error::invalid("projection cell has the wrong type")),
                })
                .collect::<crate::Result<Vec<_>>>()?,
        ))),
        Type::Float => Ok(Arc::new(Float64Array::from(
            cells
                .into_iter()
                .map(|cell| match cell {
                    Cell::Float(value) => Ok(*value),
                    _ => Err(Error::invalid("projection cell has the wrong type")),
                })
                .collect::<crate::Result<Vec<_>>>()?,
        ))),
        Type::Boolean => Ok(Arc::new(BooleanArray::from(
            cells
                .into_iter()
                .map(|cell| match cell {
                    Cell::Boolean(value) => Ok(*value),
                    _ => Err(Error::invalid("projection cell has the wrong type")),
                })
                .collect::<crate::Result<Vec<_>>>()?,
        ))),
        Type::Timestamp => Ok(Arc::new(
            TimestampMillisecondArray::from(
                cells
                    .into_iter()
                    .map(|cell| match cell {
                        Cell::Timestamp(value) => Ok(*value),
                        _ => Err(Error::invalid("projection cell has the wrong type")),
                    })
                    .collect::<crate::Result<Vec<_>>>()?,
            )
            .with_timezone("UTC"),
        )),
    }
}

fn batch_to_rows(table: &'static Table, batch: &RecordBatch) -> crate::Result<Vec<Row>> {
    let mut rows = Vec::with_capacity(batch.num_rows());
    for index in 0..batch.num_rows() {
        let mut row = Row::new();
        for column in table.columns {
            let array = batch
                .column_by_name(column.name)
                .ok_or_else(|| Error::invalid(format!("missing {}.{}", table.name, column.name)))?;
            let value = match column.data_type {
                Type::Text => array.as_any().downcast_ref::<StringArray>().map(|array| {
                    Cell::Text((!array.is_null(index)).then(|| array.value(index).to_owned()))
                }),
                Type::Integer => array.as_any().downcast_ref::<Int64Array>().map(|array| {
                    Cell::Integer((!array.is_null(index)).then(|| array.value(index)))
                }),
                Type::Float => array
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .map(|array| Cell::Float((!array.is_null(index)).then(|| array.value(index)))),
                Type::Boolean => array.as_any().downcast_ref::<BooleanArray>().map(|array| {
                    Cell::Boolean((!array.is_null(index)).then(|| array.value(index)))
                }),
                Type::Timestamp => array
                    .as_any()
                    .downcast_ref::<TimestampMillisecondArray>()
                    .map(|array| {
                        Cell::Timestamp((!array.is_null(index)).then(|| array.value(index)))
                    }),
            }
            .ok_or_else(|| {
                Error::invalid(format!("invalid {}.{} type", table.name, column.name))
            })?;
            row.insert(column.name, value);
        }
        rows.push(row);
    }
    Ok(rows)
}

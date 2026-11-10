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
use crate::parquet_io::{int_column, read_batches, string_column};
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

    // ReplaceNamespace is a complete input inventory, but only an explicit
    // Removed change authorizes dropping already-published facts.
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
                source_restored.remove(&session_uid);
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
                source_missing.remove(&session_uid);
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

    let mut tables = dimension_updates(store, &dimensions, &facts, &touched, &removed)?;
    for table in TABLES {
        if table.kind == Kind::Session
            && let Some(rows) = facts.remove(table.name)
        {
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

fn dimension_updates<C: Clock>(
    store: &Store<C>,
    additions: &BTreeMap<&'static str, Vec<Row>>,
    facts: &BTreeMap<&'static str, Vec<Row>>,
    touched: &BTreeSet<String>,
    removed: &BTreeSet<String>,
) -> crate::Result<Vec<TableBatch>> {
    let current_sessions = dimension_rows(store, crate::schema::table("dim_session").unwrap())?;
    let retained_sessions = without_sessions(current_sessions, touched, removed)?;
    let complete_sessions = merge_dimension_rows(
        crate::schema::table("dim_session").unwrap(),
        retained_sessions,
        additions.get("dim_session").cloned().unwrap_or_default(),
    )?;
    let steps = complete_fact_rows(store, "fact_step", facts, touched, removed)?;
    let tools = complete_fact_rows(store, "fact_tool_call", facts, touched, removed)?;
    let session_facts = complete_fact_rows(store, "fact_session", facts, touched, removed)?;

    let mut updates = Vec::new();
    for table in TABLES {
        if table.kind != Kind::Dimension {
            continue;
        }
        let current = dimension_rows(store, table)?;
        let retained = match table.name {
            "dim_session" => without_sessions(current.clone(), touched, removed)?,
            "dim_machine" => rows_for_keys(current.clone(), &complete_sessions, "machine_key")?,
            "dim_harness" => rows_for_keys(current.clone(), &complete_sessions, "harness_key")?,
            "dim_repo" => rows_for_keys(current.clone(), &complete_sessions, "repo_key")?,
            "dim_tool" => rows_for_keys(current.clone(), &tools, "tool_key")?,
            "dim_model" => rows_for_keys(current.clone(), &steps, "model_key")?,
            "dim_price" => rows_for_keys(current.clone(), &steps, "price_version")?,
            "dim_date" => {
                let mut facts = steps.clone();
                facts.extend(session_facts.clone());
                rows_for_keys(current.clone(), &facts, "date_key")?
            }
            _ => return Err(Error::invalid(format!("unknown dimension {}", table.name))),
        };
        let complete = merge_dimension_rows(
            table,
            retained,
            additions.get(table.name).cloned().unwrap_or_default(),
        )?;
        if complete != current {
            updates.push(TableBatch {
                table,
                batch: rows_to_batch(table, &complete)?,
            });
        }
    }
    Ok(updates)
}

fn complete_fact_rows<C: Clock>(
    store: &Store<C>,
    name: &str,
    additions: &BTreeMap<&'static str, Vec<Row>>,
    touched: &BTreeSet<String>,
    removed: &BTreeSet<String>,
) -> crate::Result<Vec<Row>> {
    let table = crate::schema::table(name)
        .ok_or_else(|| Error::invalid(format!("unknown fact table {name}")))?;
    let mut rows = live_rows_without_sessions(store, table, touched, removed)?;
    if let Some(added) = additions.get(name) {
        rows.extend(added.clone());
    }
    Ok(rows)
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

fn live_rows_without_sessions<C: Clock>(
    store: &Store<C>,
    table: &'static Table,
    touched: &BTreeSet<String>,
    removed: &BTreeSet<String>,
) -> crate::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for file in store.manifest().files_of(table.name) {
        for batch in read_batches(&store.analytics_dir().join(&file.path), None)? {
            let sessions = string_column(&batch, "session_uid")?;
            let generations = int_column(&batch, "_gen")?;
            for (index, row) in batch_to_rows(table, &batch)?.into_iter().enumerate() {
                let session = sessions.value(index);
                if store.live_map().get(session) == Some(&generations.value(index))
                    && !touched.contains(session)
                    && !removed.contains(session)
                {
                    rows.push(row);
                }
            }
        }
    }
    Ok(rows)
}

fn without_sessions(
    rows: Vec<Row>,
    touched: &BTreeSet<String>,
    removed: &BTreeSet<String>,
) -> crate::Result<Vec<Row>> {
    Ok(rows
        .into_iter()
        .filter(|row| {
            let Some(Cell::Text(Some(session))) = row.get("session_uid") else {
                return true;
            };
            !touched.contains(session) && !removed.contains(session)
        })
        .collect::<Vec<_>>())
}

fn rows_for_keys(rows: Vec<Row>, references: &[Row], key: &str) -> crate::Result<Vec<Row>> {
    let keys: Vec<&Cell> = references
        .iter()
        .filter_map(|row| row.get(key))
        .filter(|cell| !matches!(cell, Cell::Integer(None)))
        .collect();
    Ok(rows
        .into_iter()
        .filter(|row| row.get(key).is_some_and(|value| keys.contains(&value)))
        .collect::<Vec<_>>())
}

fn merge_dimension_rows(
    table: &Table,
    mut rows: Vec<Row>,
    additions: Vec<Row>,
) -> crate::Result<Vec<Row>> {
    for row in deduplicate_dimension_rows(table, additions)? {
        match dimension_row_position(table, &rows, &row)? {
            Some(index) if rows[index] == row => {}
            Some(index) => rows[index] = row,
            None => rows.push(row),
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::convert::Infallible;
    use std::sync::Arc;

    use super::*;
    use crate::input::{
        AdmittedBatch, AdmittedSession, BillingUsage, ContextBand, Coverage, Model, ParentMethod,
        Price, PriceUnit, RateKind, Reconciliation, RecordedCost, RecordedVersions, Repo, Role,
        Session, SourceWitness, Status, Step, ToolCall, UsageOwner,
    };
    use crate::schema::DIM_TOOL;
    use crate::store::{Fault, Store, read_manifest};
    use crate::testutil::{TestClock, all_rows, live_steps};

    const NAMESPACE: &str = "m";

    struct QueuedInput {
        batches: VecDeque<AdmittedBatch>,
        checkpoints: Vec<Option<String>>,
    }

    impl QueuedInput {
        fn new(batches: impl IntoIterator<Item = AdmittedBatch>) -> Self {
            Self {
                batches: batches.into_iter().collect(),
                checkpoints: Vec::new(),
            }
        }
    }

    impl AnalyticsInput for QueuedInput {
        type Error = Infallible;

        fn read_admitted(
            &mut self,
            checkpoint: Option<&str>,
        ) -> std::result::Result<AdmittedBatch, Self::Error> {
            self.checkpoints.push(checkpoint.map(str::to_owned));
            Ok(self.batches.pop_front().expect("queued batch"))
        }
    }

    fn versions() -> Versions {
        Versions {
            transform: 999,
            rules: 7,
            price: "writer-price".into(),
        }
    }

    fn batch(
        checkpoint: &str,
        reconciliation: Reconciliation,
        changes: Vec<SessionChange>,
    ) -> AdmittedBatch {
        AdmittedBatch {
            namespace: NAMESPACE.into(),
            checkpoint: checkpoint.into(),
            reconciliation,
            changes,
        }
    }

    fn session(id: &str, step: Option<&str>, tool: &str) -> AdmittedSession {
        let uid = format!("{NAMESPACE}/codex/{id}");
        let price_version = format!("price-{id}");
        AdmittedSession {
            session_uid: uid.clone(),
            source_witness: SourceWitness::default(),
            versions: RecordedVersions {
                transform: 3,
                rules: 4,
                price: price_version.clone(),
                taxonomy: None,
                coverage: Coverage::default(),
            },
            session: Session {
                machine: NAMESPACE.into(),
                harness: "codex".into(),
                native_session_id: id.into(),
                role: Role::Solo,
                parent_session_uid: None,
                parent_method: ParentMethod::None,
                lane_root_uid: None,
                repo: Repo {
                    key: format!("repo-{id}"),
                    repo_id: Some(format!("example/{id}")),
                    local_path: None,
                    display_name: id.into(),
                },
                branch: None,
                status: Status::Ended,
                started_at: Some(1_700_000_000_000),
                ended_at: Some(1_700_000_001_000),
                reported_usd: None,
            },
            steps: step
                .into_iter()
                .map(|step| Step {
                    step_uid: format!("{uid}/s/{step}"),
                    turn_uid: format!("{uid}/t/turn"),
                    model: Model {
                        key: "model".into(),
                        name: "test".into(),
                        provider: Some("openai".into()),
                    },
                    timestamp: Some(1_700_000_000_100),
                    billing: BillingUsage::default(),
                    usage_owner: UsageOwner::Exclusive,
                    context_tokens: 0,
                    context_band: ContextBand::Under50k,
                    model_ms: None,
                    tool_ms: None,
                    cost: RecordedCost {
                        usd_est: None,
                        price_version: price_version.clone(),
                        components: BTreeMap::new(),
                    },
                    activity: None,
                    waste_kind: None,
                    trigger: None,
                    ok: Some(true),
                    tool_calls: vec![ToolCall {
                        ord: 0,
                        tool: tool.into(),
                        target_path: None,
                        command_tag: None,
                        ok: Some(true),
                        exit_code: Some(0),
                        tool_ms: None,
                    }],
                })
                .collect(),
            compactions: Vec::new(),
            usage_complete: true,
            prices: if step.is_some() {
                vec![Price {
                    price_version,
                    provider: "openai".into(),
                    model: "test".into(),
                    rate_kind: RateKind::Input,
                    tier: None,
                    speed: None,
                    service_tier: None,
                    threshold_tokens: None,
                    unit: PriceUnit::Mtok,
                    usd_per_unit: 1.0,
                    valid_from: 1_700_000_000_000,
                    valid_to: None,
                    learned_at: 1_700_000_000_000,
                }]
            } else {
                Vec::new()
            },
        }
    }

    fn open(dir: &std::path::Path) -> Store<Arc<TestClock>> {
        Store::open_with_clock(dir, TestClock::new(100)).unwrap()
    }

    #[test]
    fn first_publication_includes_empty_sessions_and_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([batch(
            "inc:1",
            Reconciliation::ReplaceNamespace,
            vec![
                SessionChange::Admitted(Box::new(session("one", Some("old"), "shell"))),
                SessionChange::Admitted(Box::new(session("empty", None, "unused"))),
            ],
        )]);

        assert_eq!(
            publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap(),
            1
        );
        assert_eq!(input.checkpoints, vec![None]);
        assert_eq!(store.manifest().checkpoints[NAMESPACE], "inc:1");
        assert_eq!(
            store.manifest().versions.transform,
            ANALYTICS_TRANSFORM_VERSION
        );
        assert!(store.live_map().contains_key("m/codex/empty"));
        assert_eq!(live_steps(&store).len(), 1);
    }

    #[test]
    fn incremental_replaces_whole_session_and_retains_pending_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let first = session("one", Some("old"), "shell");
        let changed = session("one", Some("new"), "shell");
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                Reconciliation::ReplaceNamespace,
                vec![SessionChange::Admitted(Box::new(first))],
            ),
            batch(
                "inc:2",
                Reconciliation::Incremental,
                vec![SessionChange::Admitted(Box::new(changed))],
            ),
            batch(
                "inc:3",
                Reconciliation::Incremental,
                vec![SessionChange::Pending {
                    session_uid: "m/codex/one".into(),
                }],
            ),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(input.checkpoints[1].as_deref(), Some("inc:1"));
        assert_eq!(
            live_steps(&store),
            [("m/codex/one".into(), 2, "m/codex/one/s/new".into())]
                .into_iter()
                .collect()
        );
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(live_steps(&store).len(), 1);
        assert_eq!(store.manifest().checkpoints[NAMESPACE], "inc:3");
    }

    #[test]
    fn source_loss_restoration_and_explicit_removal_have_distinct_effects() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                Reconciliation::ReplaceNamespace,
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("s"),
                    "shell",
                )))],
            ),
            batch(
                "inc:2",
                Reconciliation::Incremental,
                vec![SessionChange::SourceMissing {
                    session_uid: "m/codex/one".into(),
                    retained_at_origin: true,
                }],
            ),
            batch(
                "inc:3",
                Reconciliation::Incremental,
                vec![SessionChange::SourceRestored {
                    session_uid: "m/codex/one".into(),
                }],
            ),
            batch(
                "inc:4",
                Reconciliation::Incremental,
                vec![SessionChange::Removed {
                    session_uid: "m/codex/one".into(),
                }],
            ),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        let rows = live_steps(&store);
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(live_steps(&store), rows);
        assert!(store.manifest().source_missing.contains_key("m/codex/one"));
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(live_steps(&store), rows);
        assert!(store.manifest().source_missing.is_empty());
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert!(store.live_map().is_empty());
        assert!(live_steps(&store).is_empty());
    }

    #[test]
    fn replace_namespace_retains_absent_sessions_without_explicit_removal() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "old:1",
                Reconciliation::ReplaceNamespace,
                vec![
                    SessionChange::Admitted(Box::new(session("one", Some("old"), "shell"))),
                    SessionChange::Admitted(Box::new(session("frozen", Some("frozen"), "git"))),
                ],
            ),
            batch(
                "new:1",
                Reconciliation::ReplaceNamespace,
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("new"),
                    "shell",
                )))],
            ),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert!(store.live_map().contains_key("m/codex/frozen"));
        assert!(
            live_steps(&store)
                .iter()
                .any(|(_, _, step)| step == "m/codex/frozen/s/frozen")
        );
    }

    #[test]
    fn dimensions_keep_entries_referenced_only_by_frozen_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                Reconciliation::ReplaceNamespace,
                vec![
                    SessionChange::Admitted(Box::new(session("one", Some("old"), "shell"))),
                    SessionChange::Admitted(Box::new(session("frozen", Some("old"), "git"))),
                ],
            ),
            batch(
                "inc:2",
                Reconciliation::Incremental,
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("new"),
                    "cargo",
                )))],
            ),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        let tools = dimension_rows(&store, &DIM_TOOL).unwrap();
        assert!(
            tools
                .iter()
                .any(|row| { row.get("tool_key") == Some(&Cell::Text(Some("git".into()))) })
        );
        assert!(
            tools
                .iter()
                .any(|row| { row.get("tool_key") == Some(&Cell::Text(Some("cargo".into()))) })
        );
    }

    #[test]
    fn checkpoint_moves_only_with_a_successful_publish() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                Reconciliation::ReplaceNamespace,
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("old"),
                    "shell",
                )))],
            ),
            batch(
                "inc:2",
                Reconciliation::Incremental,
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("new"),
                    "shell",
                )))],
            ),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        let before = store.manifest().clone();
        store.fault = Some(Fault::BeforeRename);
        assert!(matches!(
            publish_cycle(&mut store, &mut input, NAMESPACE, versions()),
            Err(PublishError::Store(_))
        ));
        assert_eq!(store.manifest(), &before);
        assert_eq!(read_manifest(dir.path()).unwrap().as_ref(), Some(&before));
    }

    #[test]
    fn incremental_live_rows_equal_one_final_full_refresh() {
        let incremental_dir = tempfile::tempdir().unwrap();
        let full_dir = tempfile::tempdir().unwrap();
        let mut incremental = open(incremental_dir.path());
        let mut full = open(full_dir.path());
        let final_one = session("one", Some("new"), "cargo");
        let final_two = session("two", Some("two"), "git");
        let mut incremental_input = QueuedInput::new([
            batch(
                "inc:1",
                Reconciliation::ReplaceNamespace,
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("old"),
                    "shell",
                )))],
            ),
            batch(
                "inc:2",
                Reconciliation::Incremental,
                vec![
                    SessionChange::Admitted(Box::new(final_one.clone())),
                    SessionChange::Admitted(Box::new(final_two.clone())),
                ],
            ),
        ]);
        let mut full_input = QueuedInput::new([batch(
            "full:1",
            Reconciliation::ReplaceNamespace,
            vec![
                SessionChange::Admitted(Box::new(final_one)),
                SessionChange::Admitted(Box::new(final_two)),
            ],
        )]);

        publish_cycle(
            &mut incremental,
            &mut incremental_input,
            NAMESPACE,
            versions(),
        )
        .unwrap();
        publish_cycle(
            &mut incremental,
            &mut incremental_input,
            NAMESPACE,
            versions(),
        )
        .unwrap();
        publish_cycle(&mut full, &mut full_input, NAMESPACE, versions()).unwrap();
        assert_eq!(comparable_rows(&incremental), comparable_rows(&full));
    }

    fn comparable_rows(store: &Store<Arc<TestClock>>) -> Vec<BTreeMap<String, usize>> {
        all_rows(store, &TABLES)
            .into_iter()
            .map(|rows| {
                rows.into_iter()
                    .map(|(row, count)| (row_without_generation(&row), count))
                    .collect()
            })
            .collect()
    }

    fn row_without_generation(row: &str) -> String {
        let row = row.strip_suffix('|').expect("row delimiter");
        row[..row.rfind('|').expect("generation delimiter")].to_owned()
    }
}

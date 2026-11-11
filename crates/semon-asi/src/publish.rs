//! One atomic origin publication cycle.
//!
//! The input has already admitted and priced a complete session generation. This
//! module only projects that evidence, retains frozen rows, and hands one
//! [`crate::store::Update`] to the generation store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, TimestampMillisecondArray,
};
use arrow::record_batch::RecordBatch;

use crate::error::Error;
use crate::input::{
    ANALYTICS_TRANSFORM_VERSION, AnalyticsInput, InputBudget, SessionChange, encode_component,
};
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

/// Result of publishing one input chunk. Call again while incomplete, using
/// the next checkpoint already committed in the store manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublishProgress {
    pub generation: u64,
    pub complete: bool,
}

/// Publishes one coherent input batch as one store generation.
///
/// `versions` are the writer's current targets. They are manifest metadata,
/// not a claim that every retained session has those recorded versions. The
/// transform target is always the current pure projection version. A
/// `ReplaceNamespace` batch marks prior live sessions absent from its complete
/// namespace UID inventory source-missing; it retains their facts. Each call
/// publishes one chunk. The caller controls draining/yielding and the 2 s cadence.
pub fn publish_cycle<C: Clock, I: AnalyticsInput>(
    store: &mut Store<C>,
    input: &mut I,
    namespace: &str,
    mut versions: Versions,
    budget: InputBudget,
) -> std::result::Result<PublishProgress, PublishError<I::Error>> {
    versions.transform = ANALYTICS_TRANSFORM_VERSION;
    let checkpoint = store
        .manifest()
        .checkpoints
        .get(namespace)
        .map(String::as_str);
    let batch = input
        .read_admitted(checkpoint, budget)
        .map_err(PublishError::Input)?;
    if batch.namespace != namespace {
        return Err(PublishError::Store(Error::invalid(
            "input returned a batch for another namespace",
        )));
    }
    let complete = batch.complete;
    let generation = update_for_batch(store, batch, versions)
        .and_then(|update| store.publish(&update))
        .map_err(PublishError::Store)?;
    Ok(PublishProgress {
        generation,
        complete,
    })
}

fn update_for_batch<C: Clock>(
    store: &Store<C>,
    batch: crate::input::AdmittedBatch,
    versions: Versions,
) -> crate::Result<Update> {
    let mut facts = BTreeMap::<&'static str, Vec<Row>>::new();
    let mut dimensions = BTreeMap::<&'static str, Vec<Row>>::new();
    let mut provenance = BTreeMap::new();
    let mut touched = BTreeSet::new();
    let mut removed = BTreeSet::new();
    let mut source_missing = BTreeMap::new();
    let mut source_restored = BTreeSet::new();
    let mut currently_missing = store.manifest().source_missing.keys().cloned().collect();

    // ReplaceNamespace is a complete input inventory, but only an explicit
    // Removed change authorizes dropping already-published facts.
    for change in batch.changes {
        match change {
            SessionChange::Admitted(session) => {
                if !touched.insert(session.session_uid.clone()) {
                    return Err(Error::invalid("input admits a session more than once"));
                }
                let projected = project(&session)?;
                provenance.insert(
                    session.session_uid.clone(),
                    crate::provenance::Provenance {
                        source_witness: session.source_witness.clone(),
                        derivation: session.derivation.clone(),
                        versions: session.versions.clone(),
                        projected_at: store.now_ms(),
                    },
                );
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
            } => {
                mark_source_missing(
                    store,
                    &session_uid,
                    &mut currently_missing,
                    &mut source_missing,
                    &mut source_restored,
                );
            }
            SessionChange::SourceRestored { session_uid } => {
                currently_missing.remove(&session_uid);
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

    if let crate::input::Reconciliation::ReplaceNamespace { session_uids } = batch.reconciliation {
        let namespace_prefix = format!("{}/", encode_component(&batch.namespace));
        for session_uid in store.live_map().keys() {
            if session_uid.starts_with(&namespace_prefix)
                && !session_uids.contains(session_uid)
                && !removed.contains(session_uid)
            {
                mark_source_missing(
                    store,
                    session_uid,
                    &mut currently_missing,
                    &mut source_missing,
                    &mut source_restored,
                );
            }
        }
    }

    let mut tables = dimension_updates(store, &dimensions, &touched, &removed)?;
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
        provenance,
        touched_sessions: touched.into_iter().collect(),
        removed_sessions: removed.into_iter().collect(),
        source_missing,
        source_restored: source_restored.into_iter().collect(),
        checkpoints,
        durable_revision: store.manifest().durable_revision,
        versions,
    })
}

fn mark_source_missing<C: Clock>(
    store: &Store<C>,
    session_uid: &str,
    currently_missing: &mut BTreeSet<String>,
    source_missing: &mut BTreeMap<String, SourceMissing>,
    source_restored: &mut BTreeSet<String>,
) {
    if store.live_map().contains_key(session_uid)
        && currently_missing.insert(session_uid.to_owned())
    {
        source_restored.remove(session_uid);
        source_missing.insert(
            session_uid.to_owned(),
            SourceMissing {
                detected_at: store.now_ms(),
            },
        );
    }
}

fn dimension_updates<C: Clock>(
    store: &Store<C>,
    additions: &BTreeMap<&'static str, Vec<Row>>,
    touched: &BTreeSet<String>,
    removed: &BTreeSet<String>,
) -> crate::Result<Vec<TableBatch>> {
    let mut updates = Vec::new();
    for table in TABLES {
        if table.kind != Kind::Dimension {
            continue;
        }
        let current = dimension_rows(store, table)?;
        let retained = if table.name == "dim_session" {
            without_sessions(current.clone(), touched, removed)?
        } else {
            current.clone()
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

fn dimension_rows<C: Clock>(store: &Store<C>, table: &'static Table) -> crate::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for file in store.manifest().files_of(table.name) {
        for batch in read_batches(&store.analytics_dir().join(&file.path), None)? {
            rows.extend(batch_to_rows(table, &batch)?);
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

fn merge_dimension_rows(
    table: &Table,
    mut rows: Vec<Row>,
    additions: Vec<Row>,
) -> crate::Result<Vec<Row>> {
    if table.name == "dim_price" {
        rows.extend(additions);
        return deduplicate_dimension_rows(table, rows);
    }
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
            Some(index) if table.name == "dim_price" => {
                merge_price_observation(&mut unique[index], &row)?;
            }
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
        const KEY: [&str; 9] = [
            "price_version",
            "provider",
            "model",
            "rate_kind",
            "tier",
            "speed",
            "service_tier",
            "threshold_tokens",
            "unit",
        ];
        for key in KEY {
            if !candidate.contains_key(key) {
                return Err(Error::invalid(format!("missing dim_price key {key}")));
            }
        }
        return Ok(rows
            .iter()
            .position(|row| KEY.iter().all(|key| row.get(key) == candidate.get(key))));
    }
    let key = candidate
        .get(table.columns[0].name)
        .ok_or_else(|| Error::invalid(format!("missing {} key", table.name)))?;
    Ok(rows
        .iter()
        .position(|row| row.get(table.columns[0].name) == Some(key)))
}

// Fetch time is excluded from price_version. Merge each bound independently so
// observations commute across batches and publication cycles. Once closed, an
// interval cannot be reopened by an observation that still considers it current.
fn merge_price_observation(current: &mut Row, observation: &Row) -> crate::Result<()> {
    if current.get("usd_per_unit") != observation.get("usd_per_unit")
        || current.get("unit") != observation.get("unit")
    {
        return Err(Error::invalid(
            "conflicting dim_price rates for one logical key",
        ));
    }
    let (learned_at, valid_from, valid_to) = price_observation_timestamps(current)?;
    let (other_learned_at, other_valid_from, other_valid_to) =
        price_observation_timestamps(observation)?;
    let valid_to = match (valid_to, other_valid_to) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    current.insert(
        "learned_at",
        Cell::Timestamp(Some(learned_at.min(other_learned_at))),
    );
    current.insert(
        "valid_from",
        Cell::Timestamp(Some(valid_from.min(other_valid_from))),
    );
    current.insert("valid_to", Cell::Timestamp(valid_to));
    Ok(())
}

fn price_observation_timestamps(row: &Row) -> crate::Result<(i64, i64, Option<i64>)> {
    match (
        row.get("learned_at"),
        row.get("valid_from"),
        row.get("valid_to"),
    ) {
        (
            Some(Cell::Timestamp(Some(learned_at))),
            Some(Cell::Timestamp(Some(valid_from))),
            Some(Cell::Timestamp(valid_to)),
        ) => Ok((*learned_at, *valid_from, *valid_to)),
        _ => Err(Error::invalid("invalid dim_price observation timestamps")),
    }
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
        Session, SourceWitness, Status, Step, ToolCall, UNKNOWN_REPO_KEY, UsageOwner,
    };
    use crate::schema::{
        DIM_PRICE, DIM_REPO, DIM_TOOL, FACT_SESSION, FACT_STEP, FACT_TOOL_CALL, Kind,
    };
    use crate::store::{Fault, Store, read_live_map, read_manifest};
    use crate::testutil::{FACT_TABLES, TestClock, all_rows, live_steps, update};

    const NAMESPACE: &str = "m";

    struct QueuedInput {
        batches: VecDeque<AdmittedBatch>,
        checkpoints: Vec<Option<String>>,
        budgets: Vec<InputBudget>,
    }

    impl QueuedInput {
        fn new(batches: impl IntoIterator<Item = AdmittedBatch>) -> Self {
            Self {
                batches: batches.into_iter().collect(),
                checkpoints: Vec::new(),
                budgets: Vec::new(),
            }
        }
    }

    impl AnalyticsInput for QueuedInput {
        type Error = Infallible;

        fn read_admitted(
            &mut self,
            checkpoint: Option<&str>,
            budget: InputBudget,
        ) -> std::result::Result<AdmittedBatch, Self::Error> {
            self.checkpoints.push(checkpoint.map(str::to_owned));
            self.budgets.push(budget);
            Ok(self.batches.pop_front().expect("queued batch"))
        }
    }

    // Most tests exercise one small publication. Chunk scheduling tests call
    // the public function explicitly with their selected budget.
    fn publish_cycle<C: Clock, I: AnalyticsInput>(
        store: &mut Store<C>,
        input: &mut I,
        namespace: &str,
        versions: Versions,
    ) -> std::result::Result<u64, PublishError<I::Error>> {
        super::publish_cycle(store, input, namespace, versions, InputBudget::default())
            .map(|progress| progress.generation)
    }

    fn full_reconciliation() -> Reconciliation {
        Reconciliation::ReplaceNamespace {
            session_uids: BTreeSet::new(),
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
        mut reconciliation: Reconciliation,
        changes: Vec<SessionChange>,
    ) -> AdmittedBatch {
        if let Reconciliation::ReplaceNamespace { session_uids } = &mut reconciliation {
            session_uids.extend(changes.iter().filter_map(|change| match change {
                SessionChange::Admitted(session) => Some(session.session_uid.clone()),
                SessionChange::Pending { session_uid }
                | SessionChange::SourceMissing { session_uid, .. }
                | SessionChange::SourceRestored { session_uid } => Some(session_uid.clone()),
                SessionChange::Removed { .. } => None,
            }));
        }
        AdmittedBatch {
            namespace: NAMESPACE.into(),
            checkpoint: checkpoint.into(),
            complete: true,
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
            derivation: crate::input::Derivation::default(),
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

    fn remote_session(id: &str, local_path: &str) -> AdmittedSession {
        let mut admitted = session(id, Some("step"), "shell");
        admitted.session.repo.key = "repo-remote".into();
        admitted.session.repo.repo_id = Some("example/remote".into());
        admitted.session.repo.local_path = Some(local_path.into());
        admitted.session.repo.display_name = "remote".into();
        admitted
    }

    fn unknown_repo_session(id: &str, local_path: &str) -> AdmittedSession {
        let mut admitted = session(id, Some("step"), "shell");
        admitted.session.repo = Repo {
            key: UNKNOWN_REPO_KEY.into(),
            repo_id: None,
            local_path: Some(local_path.into()),
            display_name: "unknown".into(),
        };
        admitted
    }

    fn priced_session(id: &str, learned_at: i64, rate: f64) -> AdmittedSession {
        let mut admitted = session(id, Some("step"), "shell");
        admitted.versions.price = "same-version".into();
        admitted.steps[0].cost.price_version = "same-version".into();
        let price = &mut admitted.prices[0];
        price.price_version = "same-version".into();
        price.learned_at = learned_at;
        price.valid_from = learned_at;
        price.usd_per_unit = rate;
        admitted
    }

    fn publish_price_observations(observations: &[AdmittedSession], per_cycle: usize) -> Vec<Row> {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new(observations.chunks(per_cycle).enumerate().map(
            |(index, observations)| {
                batch(
                    &format!("inc:{index}"),
                    Reconciliation::Incremental,
                    observations
                        .iter()
                        .cloned()
                        .map(|session| SessionChange::Admitted(Box::new(session)))
                        .collect(),
                )
            },
        ));
        while !input.batches.is_empty() {
            let before_rows = dimension_rows(&store, &DIM_PRICE).unwrap();
            let before_files: Vec<_> = store.manifest().files_of("dim_price").cloned().collect();
            publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
            if dimension_rows(&store, &DIM_PRICE).unwrap() == before_rows {
                assert_eq!(
                    store
                        .manifest()
                        .files_of("dim_price")
                        .cloned()
                        .collect::<Vec<_>>(),
                    before_files,
                    "unchanged price state must not publish a dimension batch",
                );
            }
        }
        assert_eq!(store.live_map().len(), observations.len());
        dimension_rows(&store, &DIM_PRICE).unwrap()
    }

    #[test]
    fn partial_inventory_publishes_one_generation_and_reconciles_complete_uids_only() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(100);
        let mut store = Store::open_with_clock(dir.path(), clock.clone()).unwrap();
        let mut seed = update(
            &[
                ("m/codex/one", 1),
                ("m/codex/two", 1),
                ("m/codex/absent", 1),
                ("other/codex/kept", 1),
            ],
            "seed",
            0,
        );
        seed.checkpoints.insert("origin".into(), "origin:1".into());
        store.publish(&seed).unwrap();
        let mut first = batch(
            "inventory:1",
            Reconciliation::Incremental,
            vec![SessionChange::Admitted(Box::new(session(
                "one",
                Some("new"),
                "shell",
            )))],
        );
        first.complete = false;
        let last = batch(
            "incremental:2",
            Reconciliation::ReplaceNamespace {
                session_uids: BTreeSet::from(["m/codex/one".into(), "m/codex/two".into()]),
            },
            vec![SessionChange::Admitted(Box::new(session(
                "two",
                Some("two"),
                "git",
            )))],
        );
        let mut input = QueuedInput::new([first, last]);
        let budget = InputBudget {
            max_sessions: 1,
            max_steps: 1,
        };
        let first =
            super::publish_cycle(&mut store, &mut input, NAMESPACE, versions(), budget).unwrap();
        assert_eq!(
            first,
            PublishProgress {
                generation: 2,
                complete: false
            }
        );
        assert!(store.manifest().source_missing.is_empty());
        assert_eq!(store.manifest().checkpoints[NAMESPACE], "inventory:1");
        clock.advance(200);
        let last =
            super::publish_cycle(&mut store, &mut input, NAMESPACE, versions(), budget).unwrap();
        assert_eq!(
            last,
            PublishProgress {
                generation: 3,
                complete: true
            }
        );
        assert_eq!(input.budgets, vec![budget, budget]);
        assert_eq!(input.checkpoints, vec![None, Some("inventory:1".into())]);
        assert_eq!(store.manifest().source_missing.len(), 1);
        assert_eq!(
            store.manifest().source_missing["m/codex/absent"].detected_at,
            300
        );
        assert!(store.live_map().contains_key("m/codex/absent"));
        assert!(
            !store.manifest().source_missing.contains_key("m/codex/one"),
            "a UID from an earlier chunk is still present in the final inventory"
        );
        assert!(store.live_map().contains_key("other/codex/kept"));
        assert_eq!(
            store.manifest().checkpoints["origin"],
            "origin:1",
            "checkpoints for other namespaces are retained"
        );
    }

    #[test]
    fn final_inventory_explicit_removal_does_not_mark_deleted_rows_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        store
            .publish(&update(&[("m/codex/one", 1)], "seed", 0))
            .unwrap();
        let mut input = QueuedInput::new([batch(
            "complete:1",
            full_reconciliation(),
            vec![SessionChange::Removed {
                session_uid: "m/codex/one".into(),
            }],
        )]);
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert!(store.live_map().is_empty());
        assert!(store.manifest().source_missing.is_empty());
    }

    #[test]
    fn first_publication_includes_empty_sessions_and_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([batch(
            "inc:1",
            full_reconciliation(),
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
                full_reconciliation(),
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
                full_reconciliation(),
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
    fn source_missing_keeps_its_first_detection_until_a_restore() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(100);
        let mut store = Store::open_with_clock(dir.path(), clock.clone()).unwrap();
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                full_reconciliation(),
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("step"),
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
                vec![SessionChange::SourceMissing {
                    session_uid: "m/codex/one".into(),
                    retained_at_origin: true,
                }],
            ),
            batch(
                "inc:4",
                Reconciliation::Incremental,
                vec![SessionChange::SourceRestored {
                    session_uid: "m/codex/one".into(),
                }],
            ),
            batch(
                "inc:5",
                Reconciliation::Incremental,
                vec![SessionChange::SourceMissing {
                    session_uid: "m/codex/one".into(),
                    retained_at_origin: true,
                }],
            ),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(
            store.manifest().source_missing["m/codex/one"].detected_at,
            100
        );

        clock.advance(100);
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(
            store.manifest().source_missing["m/codex/one"].detected_at,
            100
        );

        clock.advance(100);
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert!(store.manifest().source_missing.is_empty());

        clock.advance(100);
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(
            store.manifest().source_missing["m/codex/one"].detected_at,
            400
        );
    }

    #[test]
    fn replace_namespace_marks_absent_sessions_missing_without_removal() {
        let dir = tempfile::tempdir().unwrap();
        let clock = TestClock::new(100);
        let mut store = Store::open_with_clock(dir.path(), clock.clone()).unwrap();
        store
            .publish(&update(&[("other/codex/one", 1)], "other", 0))
            .unwrap();
        let mut input = QueuedInput::new([
            batch(
                "old:1",
                full_reconciliation(),
                vec![
                    SessionChange::Admitted(Box::new(session("one", Some("old"), "shell"))),
                    SessionChange::Admitted(Box::new(session("frozen", Some("frozen"), "git"))),
                ],
            ),
            batch(
                "new:1",
                full_reconciliation(),
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("new"),
                    "shell",
                )))],
            ),
            batch(
                "new:2",
                full_reconciliation(),
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("newer"),
                    "shell",
                )))],
            ),
            batch(
                "inc:3",
                Reconciliation::Incremental,
                vec![
                    SessionChange::SourceRestored {
                        session_uid: "m/codex/frozen".into(),
                    },
                    SessionChange::Admitted(Box::new(session("frozen", Some("frozen"), "git"))),
                ],
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
        assert_eq!(
            store.manifest().source_missing["m/codex/frozen"].detected_at,
            100
        );
        assert!(
            !store
                .manifest()
                .source_missing
                .contains_key("other/codex/one")
        );

        clock.advance(100);
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(
            store.manifest().source_missing["m/codex/frozen"].detected_at,
            100
        );

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert!(
            !store
                .manifest()
                .source_missing
                .contains_key("m/codex/frozen")
        );
    }

    #[test]
    fn replace_namespace_matches_only_its_encoded_component() {
        let namespaces = ["a/b", "a%2Fb", "a", "a b", "机器"];
        let uids: Vec<_> = namespaces
            .iter()
            .flat_map(|namespace| {
                ["present", "absent"]
                    .map(|id| format!("{}/codex/{id}", encode_component(namespace)))
            })
            .collect();
        let sessions: Vec<_> = uids.iter().map(|uid| (uid.as_str(), 1)).collect();
        for namespace in namespaces {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(dir.path());
            store.publish(&update(&sessions, "seed", 0)).unwrap();
            let before_live = store.live_map().clone();
            let before_steps = live_steps(&store);
            let prefix = encode_component(namespace);
            let mut input = QueuedInput::new([AdmittedBatch {
                namespace: namespace.into(),
                checkpoint: "new:1".into(),
                complete: true,
                reconciliation: Reconciliation::ReplaceNamespace {
                    session_uids: BTreeSet::from([format!("{prefix}/codex/present")]),
                },
                changes: vec![SessionChange::Pending {
                    session_uid: format!("{prefix}/codex/present"),
                }],
            }]);

            publish_cycle(&mut store, &mut input, namespace, versions()).unwrap();
            assert_eq!(store.live_map(), &before_live);
            assert_eq!(live_steps(&store), before_steps);
            assert_eq!(
                store
                    .manifest()
                    .source_missing
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
                vec![format!("{prefix}/codex/absent")],
                "namespace {namespace:?}",
            );
            assert_eq!(store.manifest().checkpoints[namespace], "new:1");
        }
    }

    #[test]
    fn unknown_repositories_deduplicate_within_and_across_cycles() {
        for separate_cycles in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(dir.path());
            let one = SessionChange::Admitted(Box::new(unknown_repo_session("one", "/work/one")));
            let two = SessionChange::Admitted(Box::new(unknown_repo_session("two", "/work/two")));
            let batches = if separate_cycles {
                vec![
                    batch("inc:1", full_reconciliation(), vec![one]),
                    batch("inc:2", Reconciliation::Incremental, vec![two]),
                ]
            } else {
                vec![batch("inc:1", full_reconciliation(), vec![one, two])]
            };
            let mut input = QueuedInput::new(batches);
            publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
            let first_repo_files: Vec<_> = store.manifest().files_of("dim_repo").cloned().collect();
            if separate_cycles {
                publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
            }
            let repos = dimension_rows(&store, &DIM_REPO).unwrap();
            assert_eq!(repos.len(), 1);
            assert_eq!(
                repos[0]["repo_key"],
                Cell::Text(Some(UNKNOWN_REPO_KEY.into()))
            );
            assert_eq!(repos[0]["repo_path"], Cell::Text(None));
            assert_eq!(
                store
                    .manifest()
                    .files_of("dim_repo")
                    .cloned()
                    .collect::<Vec<_>>(),
                first_repo_files,
            );
            assert_eq!(store.live_map().len(), 2);
        }
    }

    #[test]
    fn shared_local_repositories_deduplicate_within_and_across_cycles() {
        for separate_cycles in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(dir.path());
            // The adapter has mapped both checkouts to their shared origin path.
            let changes = ["one", "two"].map(|id| {
                let mut admitted = session(id, Some("step"), "shell");
                admitted.session.repo = Repo {
                    key: "local-base".into(),
                    repo_id: None,
                    local_path: Some("/srv/base.git".into()),
                    display_name: "local".into(),
                };
                SessionChange::Admitted(Box::new(admitted))
            });
            let batches = if separate_cycles {
                changes
                    .into_iter()
                    .enumerate()
                    .map(|(index, change)| {
                        batch(
                            &format!("inc:{index}"),
                            Reconciliation::Incremental,
                            vec![change],
                        )
                    })
                    .collect()
            } else {
                vec![batch("inc:1", full_reconciliation(), changes.into())]
            };
            let mut input = QueuedInput::new(batches);
            publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
            let first_repo_files: Vec<_> = store.manifest().files_of("dim_repo").cloned().collect();
            if separate_cycles {
                publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
            }
            let repos = dimension_rows(&store, &DIM_REPO).unwrap();
            assert_eq!(repos.len(), 1);
            assert_eq!(repos[0]["repo_key"], Cell::Text(Some("local-base".into())));
            assert_eq!(
                repos[0]["repo_path"],
                Cell::Text(Some("/srv/base.git".into()))
            );
            assert_eq!(repos[0]["repo_id"], Cell::Text(None));
            assert_eq!(repos[0]["display_name"], Cell::Text(Some("local".into())));
            assert_eq!(
                store
                    .manifest()
                    .files_of("dim_repo")
                    .cloned()
                    .collect::<Vec<_>>(),
                first_repo_files,
            );
            assert_eq!(store.live_map().len(), 2);
            assert_eq!(live_steps(&store).len(), 2);
        }
    }

    #[test]
    fn unchanged_price_rates_keep_the_first_observation_in_either_order() {
        for separate_cycles in [false, true] {
            for reverse in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let mut store = open(dir.path());
                let mut early = priced_session("early", 100, 1.0);
                early.prices[0].valid_to = Some(150);
                let expected = project(&early).unwrap().tables["dim_price"].clone();
                let late = priced_session("late", 200, 1.0);
                let mut changes = vec![
                    SessionChange::Admitted(Box::new(early)),
                    SessionChange::Admitted(Box::new(late)),
                ];
                if reverse {
                    changes.reverse();
                }
                let batches = if separate_cycles {
                    changes
                        .into_iter()
                        .enumerate()
                        .map(|(index, change)| {
                            batch(
                                &format!("inc:{index}"),
                                Reconciliation::Incremental,
                                vec![change],
                            )
                        })
                        .collect()
                } else {
                    vec![batch("inc:1", full_reconciliation(), changes)]
                };
                let mut input = QueuedInput::new(batches);
                publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
                let first_price_files: Vec<_> =
                    store.manifest().files_of("dim_price").cloned().collect();
                if separate_cycles {
                    publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
                    if !reverse {
                        assert_eq!(
                            store
                                .manifest()
                                .files_of("dim_price")
                                .cloned()
                                .collect::<Vec<_>>(),
                            first_price_files,
                        );
                    }
                }
                assert_eq!(dimension_rows(&store, &DIM_PRICE).unwrap(), expected);
            }
        }
    }

    #[test]
    fn conflicting_price_rates_refuse_the_entire_cycle() {
        for separate_cycles in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = open(dir.path());
            let mut seed = QueuedInput::new([batch(
                "seed:1",
                full_reconciliation(),
                vec![SessionChange::Admitted(Box::new(session(
                    "seed", None, "shell",
                )))],
            )]);
            publish_cycle(&mut store, &mut seed, NAMESPACE, versions()).unwrap();
            let one = SessionChange::Admitted(Box::new(priced_session("one", 100, 1.0)));
            let two = SessionChange::Admitted(Box::new(priced_session("two", 200, 2.0)));
            let batches = if separate_cycles {
                vec![
                    batch("inc:1", Reconciliation::Incremental, vec![one]),
                    batch("inc:2", Reconciliation::Incremental, vec![two]),
                ]
            } else {
                vec![batch("inc:1", Reconciliation::Incremental, vec![one, two])]
            };
            let mut input = QueuedInput::new(batches);
            if separate_cycles {
                publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
            }
            let before = store.manifest().clone();
            let before_live = store.live_map().clone();
            let before_rows = all_rows(&store, &FACT_TABLES);
            assert!(matches!(
                publish_cycle(&mut store, &mut input, NAMESPACE, versions()),
                Err(PublishError::Store(Error::Invalid(message))) if message.contains("conflicting dim_price")
            ));
            assert_eq!(store.manifest(), &before);
            assert_eq!(store.live_map(), &before_live);
            assert_eq!(all_rows(&store, &FACT_TABLES), before_rows);
            assert_eq!(read_manifest(dir.path()).unwrap().unwrap(), before);
        }
    }

    #[test]
    fn price_identity_includes_every_logical_rate_selector() {
        let admitted = priced_session("one", 100, 1.0);
        let original = project(&admitted).unwrap().tables["dim_price"][0].clone();
        for (key, value) in [
            ("price_version", Cell::Text(Some("another-version".into()))),
            ("provider", Cell::Text(Some("another-provider".into()))),
            ("model", Cell::Text(Some("another-model".into()))),
            ("rate_kind", Cell::Text(Some("output".into()))),
            ("tier", Cell::Text(Some("long".into()))),
            ("speed", Cell::Text(Some("fast".into()))),
            ("service_tier", Cell::Text(Some("priority".into()))),
            ("threshold_tokens", Cell::Integer(Some(200_000))),
            ("unit", Cell::Text(Some("request".into()))),
        ] {
            let mut other = original.clone();
            other.insert(key, value);
            other.insert("usd_per_unit", Cell::Float(Some(2.0)));
            assert_eq!(
                merge_dimension_rows(&DIM_PRICE, vec![original.clone()], vec![other.clone()])
                    .unwrap(),
                vec![original.clone(), other],
                "selector {key}",
            );
        }
    }

    #[test]
    fn price_intervals_stay_closed_with_older_or_tied_open_observations() {
        for open_learned_at in [50, 100, 150] {
            let mut closed = priced_session("closed", 100, 1.0);
            closed.prices[0].valid_to = Some(200);
            let mut open = priced_session("open", open_learned_at, 1.0);
            open.prices[0].valid_from = 100;
            let mut expected = project(&closed).unwrap().tables["dim_price"][0].clone();
            expected.insert(
                "learned_at",
                Cell::Timestamp(Some(open_learned_at.min(100))),
            );
            for observations in [[closed.clone(), open.clone()], [open, closed]] {
                for per_cycle in [1, 2] {
                    assert_eq!(
                        publish_price_observations(&observations, per_cycle),
                        vec![expected.clone()],
                    );
                }
            }
        }
    }

    #[test]
    fn price_closures_keep_the_earlier_end_in_either_order() {
        let mut earlier_end = priced_session("earlier-end", 200, 1.0);
        earlier_end.prices[0].valid_from = 100;
        earlier_end.prices[0].valid_to = Some(250);
        let mut later_end = priced_session("later-end", 100, 1.0);
        later_end.prices[0].valid_to = Some(300);
        let mut expected = project(&later_end).unwrap().tables["dim_price"][0].clone();
        expected.insert("valid_to", Cell::Timestamp(Some(250)));
        for observations in [
            [earlier_end.clone(), later_end.clone()],
            [later_end, earlier_end],
        ] {
            for per_cycle in [1, 2] {
                assert_eq!(
                    publish_price_observations(&observations, per_cycle),
                    vec![expected.clone()],
                );
            }
        }
    }

    #[test]
    fn price_intervals_remain_open_when_every_observation_is_open() {
        let mut early_learned = priced_session("early-learned", 50, 1.0);
        early_learned.prices[0].valid_from = 150;
        let early_validity = priced_session("early-validity", 100, 1.0);
        let mut expected = project(&early_validity).unwrap().tables["dim_price"][0].clone();
        expected.insert("learned_at", Cell::Timestamp(Some(50)));
        for observations in [
            [early_learned.clone(), early_validity.clone()],
            [early_validity, early_learned],
        ] {
            for per_cycle in [1, 2] {
                assert_eq!(
                    publish_price_observations(&observations, per_cycle),
                    vec![expected.clone()],
                );
            }
        }
    }

    #[test]
    fn price_observation_permutations_agree_within_and_across_cycles() {
        let mut first = priced_session("first", 200, 1.0);
        first.prices[0].valid_from = 100;
        first.prices[0].valid_to = Some(300);
        let mut second = priced_session("second", 50, 1.0);
        second.prices[0].valid_from = 150;
        let mut third = priced_session("third", 150, 1.0);
        third.prices[0].valid_from = 80;
        third.prices[0].valid_to = Some(250);
        let observations = [first, second, third];
        let mut expected = project(&observations[0]).unwrap().tables["dim_price"][0].clone();
        expected.insert("learned_at", Cell::Timestamp(Some(50)));
        expected.insert("valid_from", Cell::Timestamp(Some(80)));
        expected.insert("valid_to", Cell::Timestamp(Some(250)));
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let ordered = order.map(|index| observations[index].clone());
            for per_cycle in [1, 2, 3] {
                assert_eq!(
                    publish_price_observations(&ordered, per_cycle),
                    vec![expected.clone()],
                );
            }
        }
        assert_eq!(
            deduplicate_dimension_rows(&DIM_PRICE, vec![expected.clone(), expected.clone()])
                .unwrap(),
            vec![expected],
        );
    }

    #[test]
    fn unchanged_complete_dimension_state_is_not_republished() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let admitted = priced_session("one", 100, 1.0);
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                full_reconciliation(),
                vec![SessionChange::Admitted(Box::new(admitted.clone()))],
            ),
            batch(
                "inc:2",
                Reconciliation::Incremental,
                vec![SessionChange::Admitted(Box::new(admitted))],
            ),
        ]);
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        let before: Vec<_> = store
            .manifest()
            .files
            .iter()
            .filter(|file| crate::schema::table(&file.table).unwrap().kind == Kind::Dimension)
            .cloned()
            .collect();
        assert!(!before.is_empty());
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        let after: Vec<_> = store
            .manifest()
            .files
            .iter()
            .filter(|file| crate::schema::table(&file.table).unwrap().kind == Kind::Dimension)
            .cloned()
            .collect();
        assert_eq!(after, before);
        assert_eq!(store.manifest().checkpoints[NAMESPACE], "inc:2");
    }

    #[test]
    fn matching_remote_repositories_deduplicate_within_one_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([batch(
            "inc:1",
            full_reconciliation(),
            vec![
                SessionChange::Admitted(Box::new(remote_session("one", "/work/one"))),
                SessionChange::Admitted(Box::new(remote_session("two", "/work/two"))),
            ],
        )]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        let repos = dimension_rows(&store, &DIM_REPO).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0]["repo_path"], Cell::Text(None));
    }

    #[test]
    fn matching_remote_repositories_stay_stable_across_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                full_reconciliation(),
                vec![SessionChange::Admitted(Box::new(remote_session(
                    "one",
                    "/work/one",
                )))],
            ),
            batch(
                "inc:2",
                Reconciliation::Incremental,
                vec![SessionChange::Admitted(Box::new(remote_session(
                    "two",
                    "/work/two",
                )))],
            ),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        assert_eq!(dimension_rows(&store, &DIM_REPO).unwrap().len(), 1);
    }

    #[test]
    fn dimensions_keep_entries_referenced_only_by_frozen_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                full_reconciliation(),
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
    fn empty_cycle_does_not_read_fact_files_for_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                full_reconciliation(),
                vec![SessionChange::Admitted(Box::new(session(
                    "one",
                    Some("step"),
                    "shell",
                )))],
            ),
            batch("inc:2", Reconciliation::Incremental, Vec::new()),
        ]);

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
        let fact_files: Vec<_> = store
            .manifest()
            .files
            .iter()
            .filter(|file| {
                crate::schema::table(&file.table).is_some_and(|table| table.kind == Kind::Session)
            })
            .map(|file| store.analytics_dir().join(&file.path))
            .collect();
        assert!(!fact_files.is_empty());
        for file in fact_files {
            std::fs::remove_file(file).unwrap();
        }

        publish_cycle(&mut store, &mut input, NAMESPACE, versions()).unwrap();
    }

    #[test]
    fn checkpoint_moves_only_with_a_successful_publish() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(dir.path());
        let mut input = QueuedInput::new([
            batch(
                "inc:1",
                full_reconciliation(),
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
        let before_live = store.live_map().clone();
        let before_checkpoints = before.checkpoints.clone();
        store.fault = Some(Fault::BeforeRename);
        assert!(matches!(
            publish_cycle(&mut store, &mut input, NAMESPACE, versions()),
            Err(PublishError::Store(_))
        ));
        assert_eq!(store.manifest(), &before);
        assert_eq!(store.live_map(), &before_live);
        let on_disk = read_manifest(dir.path()).unwrap().unwrap();
        assert_eq!(on_disk, before);
        assert_eq!(on_disk.checkpoints, before_checkpoints);
        assert_eq!(read_live_map(dir.path(), &on_disk).unwrap(), before_live);
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
                full_reconciliation(),
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
            full_reconciliation(),
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
        // Compare live facts and the dimensions those live sessions and facts
        // reference. Incremental publication deliberately retains unreferenced
        // dimension rows for bounded maintenance rather than pruning them here.
        assert_eq!(
            comparable_fact_rows(&incremental),
            comparable_fact_rows(&full)
        );
        assert_eq!(
            referenced_dimension_rows(&incremental),
            referenced_dimension_rows(&full)
        );
    }

    fn comparable_fact_rows(store: &Store<Arc<TestClock>>) -> Vec<BTreeMap<String, usize>> {
        all_rows(store, &FACT_TABLES)
            .into_iter()
            .map(|rows| {
                rows.into_iter()
                    .map(|(row, count)| (row_without_generation(&row), count))
                    .collect()
            })
            .collect()
    }

    fn referenced_dimension_rows(
        store: &Store<Arc<TestClock>>,
    ) -> BTreeMap<&'static str, Vec<Row>> {
        let sessions: Vec<Row> = dimension_rows(
            store,
            crate::schema::table("dim_session").unwrap(),
        )
        .unwrap()
        .into_iter()
        .filter(|row| {
            matches!(
                row.get("session_uid"),
                Some(Cell::Text(Some(session_uid))) if store.live_map().contains_key(session_uid)
            )
        })
        .collect();
        let steps = live_rows(store, &FACT_STEP);
        let tools = live_rows(store, &FACT_TOOL_CALL);
        let session_facts = live_rows(store, &FACT_SESSION);
        let mut dimensions = BTreeMap::new();
        dimensions.insert("dim_session", sessions.clone());
        dimensions.insert(
            "dim_machine",
            dimensions_for_keys(store, "dim_machine", &sessions, "machine_key"),
        );
        dimensions.insert(
            "dim_harness",
            dimensions_for_keys(store, "dim_harness", &sessions, "harness_key"),
        );
        dimensions.insert(
            "dim_repo",
            dimensions_for_keys(store, "dim_repo", &sessions, "repo_key"),
        );
        dimensions.insert(
            "dim_tool",
            dimensions_for_keys(store, "dim_tool", &tools, "tool_key"),
        );
        dimensions.insert(
            "dim_model",
            dimensions_for_keys(store, "dim_model", &steps, "model_key"),
        );
        dimensions.insert(
            "dim_price",
            dimensions_for_keys(store, "dim_price", &steps, "price_version"),
        );
        let mut dated = steps;
        dated.extend(session_facts);
        dimensions.insert(
            "dim_date",
            dimensions_for_keys(store, "dim_date", &dated, "date_key"),
        );
        dimensions
    }

    fn dimensions_for_keys(
        store: &Store<Arc<TestClock>>,
        dimension: &str,
        references: &[Row],
        key: &str,
    ) -> Vec<Row> {
        let keys: Vec<&Cell> = references
            .iter()
            .filter_map(|row| row.get(key))
            .filter(|cell| !matches!(cell, Cell::Integer(None)))
            .collect();
        dimension_rows(store, crate::schema::table(dimension).unwrap())
            .unwrap()
            .into_iter()
            .filter(|row| row.get(key).is_some_and(|value| keys.contains(&value)))
            .collect()
    }

    fn live_rows(store: &Store<Arc<TestClock>>, table: &'static Table) -> Vec<Row> {
        let mut rows = Vec::new();
        for file in store.manifest().files_of(table.name) {
            for batch in read_batches(&store.analytics_dir().join(&file.path), None).unwrap() {
                let sessions = crate::parquet_io::string_column(&batch, "session_uid").unwrap();
                let generations = crate::parquet_io::int_column(&batch, "_gen").unwrap();
                rows.extend(
                    batch_to_rows(table, &batch)
                        .unwrap()
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, row)| {
                            (store.live_map().get(sessions.value(index))
                                == Some(&generations.value(index)))
                            .then_some(row)
                        }),
                );
            }
        }
        rows
    }

    fn row_without_generation(row: &str) -> String {
        let row = row.strip_suffix('|').expect("row delimiter");
        row[..row.rfind('|').expect("generation delimiter")].to_owned()
    }
}

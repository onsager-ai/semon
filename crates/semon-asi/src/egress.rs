//! Governed receiver projection from an admitted, pinned local snapshot.
//! This is not a transport, import surface, or Hub export format.

use crate::{
    Manifest,
    governance::{
        self, Destination, ExportAuthority, GovernedRow, Policy, PolicyBundle, Principal, Scope,
        Source,
    },
    schema::{self, Kind, Table, Type},
};
use arrow::{
    array::{
        Array, BooleanArray, Float64Array, Int64Array, StringArray, TimestampMillisecondArray,
    },
    record_batch::RecordBatch,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::Path,
};

type Result<T> = std::result::Result<T, Failure>;
type Rows = BTreeMap<String, Vec<Map<String, Value>>>;

/// Private operator state, alongside analytics-namespace, never in custody bundles.
pub const HANDLES_FILE: &str = "analytics-egress-handles.sqlite";

/// Sanitized failure envelope: never attach parser, path, field or policy values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    Registry,
    Snapshot,
    Handles,
    Serialization,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Registry => "invalid local egress registry",
            Self::Snapshot => "invalid local analytics snapshot",
            Self::Handles => "local opaque handle registry unavailable",
            Self::Serialization => "egress serialization failed",
        })
    }
}
impl std::error::Error for Failure {}

/// Explicit operator-owned configuration, never accepted from a receiver.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    format: u32,
    namespace: String,
    sources: BTreeMap<String, Registration>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    source: Source,
    baseline: Policy,
    local: Policy,
}
impl Registry {
    /// Load only an explicitly selected local file. No implicit/default policy.
    pub fn load(path: &Path) -> Result<Self> {
        regular(path).map_err(|_| Failure::Registry)?;
        let registry: Self =
            serde_json::from_slice(&fs::read(path).map_err(|_| Failure::Registry)?)
                .map_err(|_| Failure::Registry)?;
        if registry.format != 1 || registry.namespace.is_empty() || registry.sources.is_empty() {
            return Err(Failure::Registry);
        }
        for registration in registry.sources.values() {
            PolicyBundle {
                baseline: registration.baseline.clone(),
                local: registration.local.clone(),
            }
            .content_version()
            .map_err(|_| Failure::Registry)?;
        }
        Ok(registry)
    }
}

/// Local persistent random mappings. Open explicitly; this never modifies facts.
/// Keep this file for the namespace lifetime, independent of store location.
pub struct Handles {
    connection: Connection,
    namespace: String,
}
impl Handles {
    /// `state_dir` is explicitly supplied by the operator, beside the namespace
    /// file, outside the disposable analytics-store directory.
    pub fn open(state_dir: &Path, namespace: &str) -> Result<Self> {
        directory(state_dir).map_err(|_| Failure::Handles)?;
        let path = state_dir.join(HANDLES_FILE);
        match fs::symlink_metadata(&path) {
            Ok(_) => regular(&path).map_err(|_| Failure::Handles)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                options.open(&path).map_err(|_| Failure::Handles)?;
            }
            Err(_) => return Err(Failure::Handles),
        }
        let connection = Connection::open(path).map_err(|_| Failure::Handles)?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS namespace (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS handles (kind TEXT NOT NULL, native TEXT NOT NULL, opaque TEXT NOT NULL UNIQUE, PRIMARY KEY(kind,native));").map_err(|_| Failure::Handles)?;
        connection
            .execute("INSERT OR IGNORE INTO namespace VALUES(1,?1)", [namespace])
            .map_err(|_| Failure::Handles)?;
        let stored: String = connection
            .query_row("SELECT value FROM namespace WHERE id=1", [], |row| {
                row.get(0)
            })
            .map_err(|_| Failure::Handles)?;
        if stored != namespace {
            return Err(Failure::Handles);
        }
        Ok(Self {
            connection,
            namespace: namespace.into(),
        })
    }
    /// Read an existing namespace-local mapping. Malformed handles and unknown
    /// mappings both return `None`; this never admits or creates an identity.
    pub fn resolve(&self, tag: &str, handle: &str) -> Result<Option<String>> {
        let Some(kind) = handle_kind(tag) else {
            return Ok(None);
        };
        let Some(opaque) = handle_hex(tag, handle) else {
            return Ok(None);
        };
        self.connection
            .query_row(
                "SELECT native FROM handles WHERE opaque=?1 AND kind=?2",
                params![opaque, kind],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| Failure::Handles)
    }

    /// Read an already published handle for a native cohort contributor.
    /// Unlike projection this never creates a mapping.
    pub fn existing(&self, tag: &str, native: &str) -> Result<Option<String>> {
        let Some(kind) = handle_kind(tag) else {
            return Ok(None);
        };
        self.connection
            .query_row(
                "SELECT opaque FROM handles WHERE kind=?1 AND native=?2",
                params![kind, native],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|value| value.map(|opaque| format!("{tag}:{opaque}")))
            .map_err(|_| Failure::Handles)
    }

    fn get(&self, kind: &str, native: &str) -> Result<String> {
        self.connection
            .execute(
                "INSERT OR IGNORE INTO handles VALUES(?1,?2,lower(hex(randomblob(32))))",
                params![kind, native],
            )
            .map_err(|_| Failure::Handles)?;
        self.connection
            .query_row(
                "SELECT opaque FROM handles WHERE kind=?1 AND native=?2",
                params![kind, native],
                |row| row.get(0),
            )
            .map_err(|_| Failure::Handles)
    }
}

fn handle_kind(tag: &str) -> Option<&'static str> {
    match tag {
        "src" => Some("source"),
        "ses" => Some("session_uid"),
        "stp" => Some("step_uid"),
        "trn" => Some("turn_uid"),
        "cmp" => Some("compaction_uid"),
        _ => None,
    }
}

pub(crate) fn handle_tag(field: &str) -> Option<&'static str> {
    match field {
        "source" => Some("src"),
        "session_uid" | "parent_session_uid" | "lane_root_uid" => Some("ses"),
        "step_uid" => Some("stp"),
        "turn_uid" => Some("trn"),
        "compaction_uid" => Some("cmp"),
        _ => None,
    }
}

pub(crate) fn handle_hex<'a>(tag: &str, handle: &'a str) -> Option<&'a str> {
    handle_kind(tag)?;
    let (actual, opaque) = handle.split_once(':')?;
    (actual == tag
        && opaque.len() == 64
        && opaque
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(opaque)
}

/// Native custody for one admitted session and its relation contributors.
/// Local only: paths and native identities must never enter receiver output.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedCohort {
    pub session_uid: String,
    pub source_ids: BTreeSet<String>,
    /// Exact pinned witnesses, keyed by contributing native session identity.
    pub witnesses: BTreeMap<String, crate::input::SourceWitness>,
}

/// All inputs are decoded during pinning; later publications cannot change them.
/// Private custody data and native keys have no serializer or Debug implementation.
pub struct Snapshot {
    namespace: String,
    rows: Rows,
    provenance: BTreeMap<String, crate::provenance::Provenance>,
}
impl Snapshot {
    pub fn pin(root: &Path, namespace_file: &Path, namespace: &str) -> Result<Self> {
        let read = || -> crate::Result<Self> {
            let _lock = crate::export::open_lock(root, false)?;
            regular(namespace_file)?;
            let manifest = crate::export::validate_snapshot(root)?;
            crate::export::require_identity(
                &fs::read_to_string(namespace_file)?,
                namespace,
                &manifest,
            )?;
            crate::export::require_namespace_ownership(root, &manifest, namespace)?;
            let live = crate::read_live_map(root, &manifest)?;
            if manifest.generation > 0 && manifest.live_map.is_none() {
                return Err(crate::Error::invalid("missing live map"));
            }
            if manifest
                .source_missing
                .keys()
                .any(|uid| !live.contains_key(uid))
            {
                return Err(crate::Error::invalid("invalid missing source"));
            }
            if let Some(path) = &manifest.live_map {
                let batches = checked_batches(
                    &root.join("analytics").join(path),
                    &schema::live_map_schema(),
                )?;
                let count: usize = batches.iter().map(RecordBatch::num_rows).sum();
                if count != live.len() {
                    return Err(crate::Error::invalid("duplicate live key"));
                }
            }
            let provenance = crate::provenance::read(root, &manifest)?
                .into_iter()
                .map(|(uid, (_, record))| (uid, record))
                .collect();
            let rows = read_rows(root, &manifest, &live)?;
            Ok(Self {
                namespace: namespace.into(),
                rows,
                provenance,
            })
        };
        read().map_err(|_| Failure::Snapshot)
    }

    /// No registry means no export, including an empty store. Principal identity
    /// is supplied by the trusted local caller, not by serialized policy input.
    pub fn project(
        &self,
        registry: Option<&Registry>,
        handles: &Handles,
        principal: &Principal,
        scope: &Scope,
    ) -> Result<Projection> {
        if handles.namespace != self.namespace {
            return Err(Failure::Registry);
        }
        self.admit(registry, Some(handles), principal, scope)
    }

    /// Return exactly the native cohorts admitted by receiver projection policy
    /// and session atomicity, without creating handles or consulting body UIDs.
    pub fn admitted_cohorts(
        &self,
        registry: Option<&Registry>,
        principal: &Principal,
        scope: &Scope,
    ) -> Result<Vec<AdmittedCohort>> {
        Ok(self
            .admit(registry, None, principal, scope)?
            .sessions
            .into_iter()
            .map(|session| session.cohort)
            .collect())
    }

    fn admit(
        &self,
        registry: Option<&Registry>,
        handles: Option<&Handles>,
        principal: &Principal,
        scope: &Scope,
    ) -> Result<Projection> {
        let registry = registry.ok_or(Failure::Registry)?;
        if registry.format != 1
            || registry.sources.is_empty()
            || scope.destination != Destination::Receiver
            || registry.namespace != self.namespace
            || scope.namespace != self.namespace
        {
            return Err(Failure::Registry);
        }
        let mut outcomes = LocalOutcomes::default();
        let mut eligible = BTreeMap::new();
        for (uid, provenance) in &self.provenance {
            let ids: BTreeSet<String> = provenance
                .source_witness
                .sources
                .iter()
                .map(|source| source.native_id.clone())
                .collect();
            if ids.is_empty()
                || ids.len() != provenance.source_witness.sources.len()
                || ids.iter().any(|id| !registry.sources.contains_key(id))
            {
                outcomes.refused_provenance += 1;
                continue;
            }
            eligible.insert(uid.clone(), ids);
        }
        // A session is atomic. Relations are checked against the complete cohort
        // on both ends; removal is iterated so no dangling relation can escape.
        let mut accepted = BTreeMap::new();
        loop {
            let mut removed = Vec::new();
            accepted.clear();
            for (uid, ids) in &eligible {
                match self.session(uid, ids, &eligible, registry, handles, (principal, scope))? {
                    Some(session) => {
                        accepted.insert(uid.clone(), session);
                    }
                    None => removed.push(uid.clone()),
                }
            }
            if removed.is_empty() {
                break;
            }
            outcomes.withheld += removed.len();
            for uid in removed {
                eligible.remove(&uid);
            }
        }
        outcomes.exported = accepted.len();
        Ok(Projection {
            namespace: self.namespace.clone(),
            sessions: accepted.into_values().collect(),
            outcomes,
        })
    }

    fn session(
        &self,
        uid: &str,
        ids: &BTreeSet<String>,
        eligible: &BTreeMap<String, BTreeSet<String>>,
        registry: &Registry,
        handles: Option<&Handles>,
        audience: (&Principal, &Scope),
    ) -> Result<Option<SessionExport>> {
        let (principal, scope) = audience;
        let mut cohort = ids.clone();
        let mut witnesses =
            BTreeMap::from([(uid.to_owned(), self.provenance[uid].source_witness.clone())]);
        let mut pending = Vec::new();
        for table in schema::TABLES
            .iter()
            .filter(|table| table.kind == Kind::Session || table.name == "dim_session")
        {
            for row in self
                .rows
                .get(table.name)
                .into_iter()
                .flatten()
                .filter(|row| row.get("session_uid").and_then(Value::as_str) == Some(uid))
            {
                if table.name == "dim_session" {
                    for field in ["parent_session_uid", "lane_root_uid"] {
                        if let Some(target) = row.get(field).and_then(Value::as_str) {
                            let Some(other) = eligible.get(target) else {
                                return Ok(None);
                            };
                            cohort.extend(other.iter().cloned());
                            witnesses.insert(
                                target.to_owned(),
                                self.provenance[target].source_witness.clone(),
                            );
                        }
                    }
                }
                pending.push((*table, row.clone()));
            }
        }
        if !pending
            .iter()
            .any(|(table, _)| table.name == "fact_session")
        {
            return Ok(None);
        }
        let authority = match authority(&cohort, registry, handles, principal, scope)? {
            Some(authority) => authority,
            None => return Ok(None),
        };
        // Policy vectors and completeness are coverage disclosures. This first
        // receiver contract withholds sessions without that separate permission.
        if !governance::coverage_allowed(&authority) {
            return Ok(None);
        }
        // Resolve only dependencies of this session, never copy dimension tables.
        let mut dependencies = Vec::new();
        for (_, row) in &pending {
            for (field, table) in [
                ("machine_key", "dim_machine"),
                ("harness_key", "dim_harness"),
                ("repo_key", "dim_repo"),
                ("model_key", "dim_model"),
                ("tool_key", "dim_tool"),
                ("date_key", "dim_date"),
            ] {
                if !governance::field_allowed(&authority, field) {
                    continue;
                }
                if let Some(value) = row.get(field).filter(|value| !value.is_null()) {
                    let dependency = (table, field, value.clone());
                    if !dependencies.contains(&dependency) {
                        dependencies.push(dependency);
                    }
                }
            }
        }
        for (table_name, field, value) in dependencies {
            let matches: Vec<_> = self
                .rows
                .get(table_name)
                .into_iter()
                .flatten()
                .filter(|row| row.get(field).is_some_and(|key| *key == value))
                .collect();
            if matches.len() != 1 {
                return Ok(None);
            }
            pending.push((
                schema::table(table_name).expect("declared dimension"),
                (*matches[0]).clone(),
            ));
        }
        // Local facts do not retain the exact composite rate-row identity.
        // Recorded costs/version remain facts; dim_price is deliberately withheld.
        let mut rows = Vec::new();
        for (table, mut row) in pending {
            for column in table.columns {
                if !governance::field_allowed(&authority, column.name) && column.nullable {
                    row.remove(column.name);
                }
            }
            let native = match governance::project_derived_row(&authority, table.name, &row) {
                Ok(native) => native,
                Err(_) => return Ok(None),
            };
            let projected = if let Some(handles) = handles {
                opaque_keys(&mut row, handles)?;
                governance::project_receiver_row(&authority, table.name, &row)
            } else {
                Ok(native)
            };
            match projected {
                Ok(row) => rows.push(ExportRow {
                    table: table.name,
                    row,
                }),
                Err(_) => return Ok(None),
            }
        }
        let mut policies = Vec::new();
        for id in &cohort {
            let Some(handles) = handles else { break };
            let registration = &registry.sources[id];
            policies.push(PolicyVersion {
                source_handle: format!("src:{}", handles.get("source", id)?),
                policy_version: PolicyBundle {
                    baseline: registration.baseline.clone(),
                    local: registration.local.clone(),
                }
                .content_version()
                .map_err(|_| Failure::Registry)?,
            });
        }
        Ok(Some(SessionExport {
            rows,
            policies,
            cohort: AdmittedCohort {
                session_uid: uid.into(),
                source_ids: cohort,
                witnesses,
            },
        }))
    }
}

fn authority(
    ids: &BTreeSet<String>,
    registry: &Registry,
    handles: Option<&Handles>,
    principal: &Principal,
    scope: &Scope,
) -> Result<Option<ExportAuthority>> {
    let mut policies = BTreeMap::new();
    let mut sources = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let registration = &registry.sources[id];
        let mut source = registration.source.clone();
        source.id = match handles {
            Some(handles) => handles.get("source", id)?,
            None => format!("{index:064x}"),
        };
        policies.insert(
            source.id.clone(),
            PolicyBundle {
                baseline: registration.baseline.clone(),
                local: registration.local.clone(),
            },
        );
        sources.push(source);
    }
    Ok(governance::evaluate(&policies, &sources, principal, scope).ok())
}

fn opaque_keys(row: &mut Map<String, Value>, handles: &Handles) -> Result<()> {
    for field in [
        "session_uid",
        "parent_session_uid",
        "lane_root_uid",
        "step_uid",
        "turn_uid",
        "compaction_uid",
    ] {
        if let Some(native) = row.get(field).and_then(Value::as_str) {
            let kind = match field {
                "parent_session_uid" | "lane_root_uid" => "session_uid",
                field => field,
            };
            let opaque = handles.get(kind, native)?;
            let tag = handle_tag(kind).expect("declared handle kind");
            let value = format!("{tag}:{opaque}");
            row.insert(field.into(), Value::String(value));
        }
    }
    Ok(())
}

/// Local-only diagnostic counts. Never included in serialized receiver output.
#[derive(Default, Debug)]
pub struct LocalOutcomes {
    pub exported: usize,
    pub withheld: usize,
    pub refused_provenance: usize,
}
pub struct Projection {
    pub(crate) namespace: String,
    sessions: Vec<SessionExport>,
    pub outcomes: LocalOutcomes,
}
struct SessionExport {
    cohort: AdmittedCohort,
    rows: Vec<ExportRow>,
    policies: Vec<PolicyVersion>,
}
struct ExportRow {
    table: &'static str,
    row: GovernedRow,
}
#[derive(Serialize)]
struct PolicyVersion {
    source_handle: String,
    policy_version: String,
}
impl Projection {
    /// Fresh typed JSON only; no source Parquet bytes, metadata or private custody.
    pub fn serialize_receiver(&self) -> Result<Vec<u8>> {
        let sessions = self
            .sessions
            .iter()
            .map(|session| {
                let rows = session
                    .rows
                    .iter()
                    .map(|row| {
                        let fields: Value = serde_json::from_str(
                            &governance::serialize_governed_row(&row.row)
                                .map_err(|_| Failure::Serialization)?,
                        )
                        .map_err(|_| Failure::Serialization)?;
                        Ok(serde_json::json!({"table": row.table, "fields": fields}))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(serde_json::json!({"rows": rows, "policy_versions": session.policies}))
            })
            .collect::<Result<Vec<_>>>()?;
        serde_json::to_vec(
            &serde_json::json!({"profile": "receiver_plain_v1", "sessions": sessions}),
        )
        .map_err(|_| Failure::Serialization)
    }
}

fn directory(path: &Path) -> crate::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(crate::Error::invalid("not a real directory"));
    }
    Ok(())
}
fn regular(path: &Path) -> crate::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(crate::Error::invalid("not a regular file"));
    }
    Ok(())
}
fn checked_batches(
    path: &Path,
    expected: &arrow::datatypes::Schema,
) -> crate::Result<Vec<RecordBatch>> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    if builder.schema().as_ref() != expected
        || builder
            .metadata()
            .file_metadata()
            .key_value_metadata()
            .into_iter()
            .flatten()
            .any(|entry| entry.key != "ARROW:schema")
    {
        return Err(crate::Error::invalid("unknown columns or metadata"));
    }
    builder
        .build()?
        .map(|batch| batch.map_err(crate::Error::from))
        .collect()
}
fn read_rows(
    root: &Path,
    manifest: &Manifest,
    live: &std::collections::HashMap<String, i64>,
) -> crate::Result<Rows> {
    let mut rows: Rows = BTreeMap::new();
    let mut paths = BTreeSet::new();
    for file in &manifest.files {
        if !paths.insert(&file.path) {
            return Err(crate::Error::invalid("duplicate file"));
        }
        let table = if file.table == "provenance" {
            &crate::provenance::TABLE
        } else {
            schema::table(&file.table).ok_or_else(|| crate::Error::invalid("unknown table"))?
        };
        let path = root.join("analytics").join(&file.path);
        if fs::metadata(&path)?.len() != file.bytes {
            return Err(crate::Error::invalid("file size mismatch"));
        }
        let batches = checked_batches(&path, &table.file_schema())?;
        if batches
            .iter()
            .map(|batch| batch.num_rows() as u64)
            .sum::<u64>()
            != file.rows
        {
            return Err(crate::Error::invalid("row count mismatch"));
        }
        for batch in batches {
            let generations = crate::parquet_io::int_column(&batch, "_gen")?;
            for index in 0..batch.num_rows() {
                let generation = generations.value(index);
                if generation < file.min_row_gen as i64 || generation > file.max_row_gen as i64 {
                    return Err(crate::Error::invalid("generation mismatch"));
                }
                if table.name == "provenance" || table.kind == Kind::Append {
                    continue;
                }
                if table.kind == Kind::Session {
                    let uid = crate::parquet_io::string_column(&batch, "session_uid")?.value(index);
                    if live.get(uid) != Some(&generation) {
                        continue;
                    }
                }
                let row = decode(table, &batch, index)?;
                rows.entry(table.name.into()).or_default().push(row);
            }
        }
    }
    Ok(rows)
}
fn decode(table: &Table, batch: &RecordBatch, index: usize) -> crate::Result<Map<String, Value>> {
    let mut row = Map::new();
    for column in table.columns {
        let array = batch
            .column_by_name(column.name)
            .ok_or_else(|| crate::Error::invalid("missing column"))?;
        let value = if array.is_null(index) {
            Value::Null
        } else {
            macro_rules! value {
                ($array:ty) => {
                    array
                        .as_any()
                        .downcast_ref::<$array>()
                        .ok_or_else(|| crate::Error::invalid("column type mismatch"))?
                        .value(index)
                };
            }
            match column.data_type {
                Type::Text => Value::from(value!(StringArray)),
                Type::Integer => Value::from(value!(Int64Array)),
                Type::Float => {
                    let number = value!(Float64Array);
                    if !number.is_finite() {
                        return Err(crate::Error::invalid("nonfinite measure"));
                    }
                    Value::from(number)
                }
                Type::Boolean => Value::from(value!(BooleanArray)),
                Type::Timestamp => Value::from(value!(TimestampMillisecondArray)),
            }
        };
        row.insert(column.name.into(), value);
    }
    Ok(row)
}

#[cfg(test)]
#[path = "egress_tests.rs"]
mod tests;

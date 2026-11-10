//! Deterministic, local policy evaluation and governed analytics export.
//!
//! This library does not authenticate callers or establish source provenance.
//! Its caller supplies an authenticated principal and the complete admitted
//! contributor cohort. Evaluation returns an opaque authority bound to those
//! values; it is the only way to create a serializable export row.

use crate::schema::{self, Class, Type};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    Public,
    Internal,
    Restricted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Destination {
    Receiver,
    Hub,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    Unknown,
    Unscanned,
    Unsupported,
    Failed,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScanEvidence {
    pub state: ScanState,
    pub detector_version: String,
    pub redactor_version: String,
    pub secret_findings: u32,
    pub pii_findings: u32,
}

/// A structured, exact export destination. It deliberately has no encoded-string
/// form, so namespace and machine components cannot collide at a delimiter.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub destination: Destination,
    pub namespace: String,
    pub machine: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EgressConsent {
    pub provider: String,
    pub model: String,
    pub evidence_classes: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub format: u32,
    pub sensitivity: Sensitivity,
    pub principals: BTreeSet<String>,
    pub groups: BTreeSet<String>,
    pub scopes: BTreeSet<Scope>,
    pub analytics_fields: BTreeSet<String>,
    pub allow_row_existence: bool,
    pub allow_coverage: bool,
    pub body_read: bool,
    pub provider_egress: Vec<EgressConsent>,
    pub max_ttl_seconds: u32,
    pub max_bytes: u64,
    pub max_requests_per_minute: u32,
}

#[derive(Clone, Debug)]
pub struct PolicyBundle {
    pub baseline: Policy,
    pub local: Policy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub owner: String,
    pub namespace: String,
    pub machine: String,
    pub floor: Sensitivity,
    pub scan: ScanEvidence,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub id: String,
    pub groups: BTreeSet<String>,
    pub namespace: String,
    pub machine: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GovernanceError {
    InvalidPolicy,
    InvalidSourceSet,
    Denied,
    InvalidRow,
}

impl std::fmt::Display for GovernanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidPolicy => "invalid policy",
            Self::InvalidSourceSet => "invalid source set",
            Self::Denied => "not authorized",
            Self::InvalidRow => "invalid export row",
        })
    }
}
impl std::error::Error for GovernanceError {}

struct ContributorGrant {
    witness: ContributorWitness,
    sensitivity: Sensitivity,
    fields: BTreeSet<String>,
    allow_row_existence: bool,
    allow_coverage: bool,
    body_read: bool,
    consents: BTreeSet<EgressConsent>,
    scan_complete_without_findings: bool,
}

/// One source's immutable evaluated-policy and scan witness. This is available
/// to trusted local publication/audit code only; it has no automatic serializer.
#[derive(Clone, Eq, PartialEq)]
pub struct ContributorWitness {
    source_id: String,
    policy_version: String,
    scan_state: ScanState,
    detector_version: String,
    redactor_version: String,
}

impl std::fmt::Debug for ContributorWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContributorWitness { private }")
    }
}

impl ContributorWitness {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    pub fn scan_state(&self) -> ScanState {
        self.scan_state
    }

    pub fn detector_version(&self) -> &str {
        &self.detector_version
    }

    pub fn redactor_version(&self) -> &str {
        &self.redactor_version
    }
}

/// Immutable local provenance bound to a governed row.
#[derive(Clone, Eq, PartialEq)]
pub struct ExportWitness {
    principal_id: String,
    scope: Scope,
    contributors: Vec<ContributorWitness>,
    coverage_allowed: bool,
}

impl std::fmt::Debug for ExportWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExportWitness { private }")
    }
}

impl ExportWitness {
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn contributors(&self) -> &[ContributorWitness] {
        &self.contributors
    }
}

/// Explicit, coverage-authorized provenance that may be serialized separately.
pub struct CoverageWitness {
    scope: Scope,
    contributors: Vec<ContributorWitness>,
}

impl std::fmt::Debug for CoverageWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CoverageWitness { private }")
    }
}

/// A non-forgeable result of evaluating a complete admitted contributor cohort.
///
/// The fields are private so callers cannot construct a permissive authority,
/// replay it to another destination, or drop a restrictive contributor.
pub struct ExportAuthority {
    witness: ExportWitness,
    contributors: Vec<ContributorGrant>,
}

impl std::fmt::Debug for ExportAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExportAuthority { private }")
    }
}

/// A row whose fields have passed the authority's exact destination and cohort.
/// Private provenance remains with the origin audit/publication layer and is
/// intentionally absent from this export envelope.
pub struct GovernedRow {
    fields: Map<String, Value>,
    witness: ExportWitness,
}

impl std::fmt::Debug for GovernedRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GovernedRow")
            .field("fields", &self.fields)
            .finish()
    }
}

impl Policy {
    /// Strict JSON parser: unknown fields and unsupported formats are rejected.
    pub fn from_json(input: &str) -> Result<Self, GovernanceError> {
        let policy: Self =
            serde_json::from_str(input).map_err(|_| GovernanceError::InvalidPolicy)?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn content_version(&self) -> Result<String, GovernanceError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|_| GovernanceError::InvalidPolicy)?;
        Ok(hash_value(&value))
    }

    fn validate(&self) -> Result<(), GovernanceError> {
        if self.format != 1
            || self.principals.iter().any(|value| !valid_subject(value))
            || self.groups.iter().any(|value| !valid_subject(value))
            || self.scopes.is_empty()
            || self.scopes.iter().any(|scope| !scope.valid())
            || self
                .analytics_fields
                .iter()
                .any(|field| !known_export_field(field))
            || self.provider_egress.iter().any(|consent| !consent.valid())
        {
            return Err(GovernanceError::InvalidPolicy);
        }
        Ok(())
    }
}

impl Scope {
    fn valid(&self) -> bool {
        valid_scope_component(&self.namespace) && valid_scope_component(&self.machine)
    }
}

impl EgressConsent {
    fn valid(&self) -> bool {
        valid_subject(&self.provider)
            && valid_subject(&self.model)
            && !self.evidence_classes.is_empty()
            && self
                .evidence_classes
                .iter()
                .all(|value| valid_subject(value))
    }
}

impl PolicyBundle {
    /// The version binds the full baseline/local policy pair, never an input label.
    pub fn content_version(&self) -> Result<String, GovernanceError> {
        self.baseline.validate()?;
        self.local.validate()?;
        Ok(hash_value(&serde_json::json!({
            "baseline": &self.baseline,
            "local": &self.local,
        })))
    }
}

/// Evaluate all policies for one authenticated principal and complete admitted cohort.
pub fn evaluate(
    policies: &BTreeMap<String, PolicyBundle>,
    sources: &[Source],
    principal: &Principal,
    scope: &Scope,
) -> Result<ExportAuthority, GovernanceError> {
    if sources.is_empty() || !scope.valid() || !valid_subject(&principal.id) {
        return Err(GovernanceError::InvalidSourceSet);
    }
    if principal.namespace != scope.namespace || principal.machine != scope.machine {
        return Err(GovernanceError::Denied);
    }

    let mut ids = BTreeSet::new();
    let mut contributors = Vec::with_capacity(sources.len());
    for source in sources {
        if !valid_source(source, scope) || !ids.insert(&source.id) {
            return Err(GovernanceError::InvalidSourceSet);
        }
        let bundle = policies
            .get(&source.id)
            .ok_or(GovernanceError::InvalidSourceSet)?;
        let grant = effective(bundle, source)?;
        let allowed = [&bundle.baseline, &bundle.local].iter().all(|policy| {
            policy.scopes.contains(scope)
                && (policy.principals.contains(&principal.id)
                    || !policy.groups.is_disjoint(&principal.groups))
        });
        if !allowed {
            return Err(GovernanceError::Denied);
        }
        contributors.push(grant);
    }

    let witness = ExportWitness {
        principal_id: principal.id.clone(),
        scope: scope.clone(),
        contributors: contributors
            .iter()
            .map(|grant| grant.witness.clone())
            .collect(),
        coverage_allowed: contributors.iter().all(|grant| grant.allow_coverage),
    };
    Ok(ExportAuthority {
        witness,
        contributors,
    })
}

/// Explicit provider/model/evidence-class consent from every contributor is required.
pub fn provider_egress_allowed(
    authority: &ExportAuthority,
    provider: &str,
    model: &str,
    evidence_class: &str,
) -> bool {
    valid_subject(provider)
        && valid_subject(model)
        && valid_subject(evidence_class)
        && authority.contributors.iter().all(|grant| {
            grant.consents.iter().any(|consent| {
                consent.provider == provider
                    && consent.model == model
                    && consent.evidence_classes.contains(evidence_class)
            })
        })
}

/// Hub is never a body destination. Receiver body use also needs completed scans,
/// zero findings, and unanimous policy consent; actual authorization/audit is later work.
pub fn body_read_allowed(authority: &ExportAuthority) -> bool {
    authority.witness.scope.destination == Destination::Receiver
        && authority
            .contributors
            .iter()
            .all(|grant| grant.body_read && grant.scan_complete_without_findings)
}

/// Coverage values, counts, source handles, and version vectors must be withheld
/// unless every contributor allows coverage disclosure.
pub fn coverage_allowed(authority: &ExportAuthority) -> bool {
    authority.witness.coverage_allowed
}

/// The strongest baseline, local, and source classification floor in the cohort.
/// This stays bound to the opaque authority and is never serialized with a row.
pub fn sensitivity_floor(authority: &ExportAuthority) -> Sensitivity {
    authority
        .contributors
        .iter()
        .map(|grant| grant.sensitivity)
        .max()
        .expect("an ExportAuthority always has a contributor")
}

/// Project a row through an opaque, exact evaluation result. A caller cannot use
/// an arbitrary policy, a different destination, or a reduced contributor list.
pub fn project_derived_row(
    authority: &ExportAuthority,
    table_name: &str,
    row: &Map<String, Value>,
) -> Result<GovernedRow, GovernanceError> {
    if authority.witness.principal_id.is_empty()
        || authority.contributors.is_empty()
        || authority
            .contributors
            .iter()
            .any(|grant| grant.witness.policy_version.is_empty() || !grant.allow_row_existence)
    {
        return Err(GovernanceError::Denied);
    }
    let table = schema::table(table_name).ok_or(GovernanceError::InvalidRow)?;
    // The schema describes fact_label's categorical intent, but #592 does not
    // yet own a versioned taxonomy registry. Refuse this export until that L1
    // contract can prove field/value/labeller membership.
    if table.name == "fact_label" {
        return Err(GovernanceError::Denied);
    }

    let fields = authority.contributors.iter().skip(1).fold(
        authority.contributors[0].fields.clone(),
        |allowed, grant| allowed.intersection(&grant.fields).cloned().collect(),
    );
    let mut projected = Map::new();
    for (name, value) in row {
        let column = table.column(name).ok_or(GovernanceError::InvalidRow)?;
        if column.class == Class::Body || !fields.contains(name) {
            return Err(GovernanceError::Denied);
        }
        validate_column_value(column, value, authority.witness.scope.destination)?;
        projected.insert(name.clone(), value.clone());
    }
    for column in table.columns {
        if !column.nullable
            && (!fields.contains(column.name) || !projected.contains_key(column.name))
        {
            return Err(GovernanceError::InvalidRow);
        }
    }
    Ok(GovernedRow {
        fields: projected,
        witness: authority.witness.clone(),
    })
}

/// Serialize only an already-governed row. Internal policy, scan, provenance,
/// source, and coverage metadata are never part of this envelope.
pub fn serialize_governed_row(row: &GovernedRow) -> Result<String, GovernanceError> {
    serde_json::to_string(&row.fields).map_err(|_| GovernanceError::InvalidRow)
}

/// Access immutable provenance only at the trusted local integration boundary.
pub fn local_witness(row: &GovernedRow) -> &ExportWitness {
    &row.witness
}

/// Coverage disclosure needs unanimous authorization and is deliberately
/// separate from row serialization.
pub fn coverage_witness(row: &GovernedRow) -> Result<CoverageWitness, GovernanceError> {
    if !row.witness.coverage_allowed {
        return Err(GovernanceError::Denied);
    }
    Ok(CoverageWitness {
        scope: row.witness.scope.clone(),
        contributors: row.witness.contributors.clone(),
    })
}

/// Serialize only an explicitly coverage-authorized witness. It omits the
/// authenticated principal and is never included in a governed-row envelope.
pub fn serialize_coverage_witness(witness: &CoverageWitness) -> Result<String, GovernanceError> {
    // Local provenance is retained verbatim; disclosure has its own export fence.
    let opaque = |value: &str| {
        valid_subject(value)
            && !credential_like(value)
            && value
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !valid_metadata_text(&witness.scope.namespace, false)
        || !valid_metadata_text(&witness.scope.machine, false)
        || witness.contributors.iter().any(|source| {
            !opaque(&source.source_id)
                || !opaque(&source.detector_version)
                || !opaque(&source.redactor_version)
        })
    {
        return Err(GovernanceError::Denied);
    }
    let contributors = witness
        .contributors
        .iter()
        .map(|contributor| {
            serde_json::json!({
                "source_id": contributor.source_id,
                "policy_version": contributor.policy_version,
                "scan_state": contributor.scan_state,
                "detector_version": contributor.detector_version,
                "redactor_version": contributor.redactor_version,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&serde_json::json!({
        "scope": witness.scope,
        "contributors": contributors,
    }))
    .map_err(|_| GovernanceError::InvalidRow)
}

fn effective(bundle: &PolicyBundle, source: &Source) -> Result<ContributorGrant, GovernanceError> {
    bundle.baseline.validate()?;
    bundle.local.validate()?;
    let policy_version = bundle.content_version()?;
    let sensitivity = bundle
        .baseline
        .sensitivity
        .max(bundle.local.sensitivity)
        .max(source.floor);
    Ok(ContributorGrant {
        witness: ContributorWitness {
            source_id: source.id.clone(),
            policy_version,
            scan_state: source.scan.state,
            detector_version: source.scan.detector_version.clone(),
            redactor_version: source.scan.redactor_version.clone(),
        },
        sensitivity,
        fields: bundle
            .baseline
            .analytics_fields
            .intersection(&bundle.local.analytics_fields)
            .cloned()
            .collect(),
        allow_row_existence: bundle.baseline.allow_row_existence
            && bundle.local.allow_row_existence,
        allow_coverage: bundle.baseline.allow_coverage && bundle.local.allow_coverage,
        body_read: bundle.baseline.body_read && bundle.local.body_read,
        consents: intersect_consents(
            &bundle.baseline.provider_egress,
            &bundle.local.provider_egress,
        ),
        scan_complete_without_findings: source.scan.state == ScanState::Completed
            && source.scan.secret_findings == 0
            && source.scan.pii_findings == 0,
    })
}

fn intersect_consents(
    baseline: &[EgressConsent],
    local: &[EgressConsent],
) -> BTreeSet<EgressConsent> {
    let mut common = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for baseline_consent in baseline {
        for local_consent in local.iter().filter(|local_consent| {
            local_consent.provider == baseline_consent.provider
                && local_consent.model == baseline_consent.model
        }) {
            let classes = baseline_consent
                .evidence_classes
                .intersection(&local_consent.evidence_classes);
            common
                .entry((
                    baseline_consent.provider.clone(),
                    baseline_consent.model.clone(),
                ))
                .or_default()
                .extend(classes.cloned());
        }
    }
    common
        .into_iter()
        .filter_map(|((provider, model), evidence_classes)| {
            (!evidence_classes.is_empty()).then_some(EgressConsent {
                provider,
                model,
                evidence_classes,
            })
        })
        .collect()
}

fn valid_source(source: &Source, scope: &Scope) -> bool {
    valid_subject(&source.id)
        && valid_subject(&source.owner)
        && source.namespace == scope.namespace
        && source.machine == scope.machine
        && valid_scope_component(&source.namespace)
        && valid_scope_component(&source.machine)
        && valid_subject(&source.scan.detector_version)
        && valid_subject(&source.scan.redactor_version)
}

fn known_export_field(name: &str) -> bool {
    schema::TABLES.iter().any(|table| {
        table.name != "fact_label"
            && table
                .column(name)
                .is_some_and(|column| column.class != Class::Body)
    })
}

fn validate_column_value(
    column: &schema::Column,
    value: &Value,
    destination: Destination,
) -> Result<(), GovernanceError> {
    validate_value(column.data_type, column.nullable, value, column.values)?;
    if value.is_null() {
        return Ok(());
    }
    let text = value.as_str();
    match column.class {
        Class::Path => match destination {
            Destination::Receiver => {
                if !text.is_some_and(valid_receiver_path) {
                    return Err(GovernanceError::Denied);
                }
            }
            Destination::Hub => {
                if !text.is_some_and(valid_path_token) {
                    return Err(GovernanceError::Denied);
                }
            }
        },
        Class::Key
            if column.data_type == Type::Text
                && !text.is_some_and(|text| valid_key(column.name, text)) =>
        {
            return Err(GovernanceError::Denied);
        }
        Class::Name if !text.is_some_and(|text| valid_name(column.name, text)) => {
            return Err(GovernanceError::Denied);
        }
        _ => {}
    }
    Ok(())
}

fn validate_value(
    ty: Type,
    nullable: bool,
    value: &Value,
    values: &[&str],
) -> Result<(), GovernanceError> {
    if value.is_null() {
        return if nullable {
            Ok(())
        } else {
            Err(GovernanceError::InvalidRow)
        };
    }
    let valid = match ty {
        Type::Text => value
            .as_str()
            .is_some_and(|text| text.len() <= 4096 && !text.as_bytes().contains(&0)),
        Type::Integer | Type::Timestamp => value.as_i64().is_some(),
        Type::Float => value.as_f64().is_some_and(f64::is_finite),
        Type::Boolean => value.as_bool().is_some(),
    };
    if !valid {
        return Err(GovernanceError::InvalidRow);
    }
    if !values.is_empty() && !value.as_str().is_some_and(|text| values.contains(&text)) {
        return Err(GovernanceError::InvalidRow);
    }
    Ok(())
}

fn valid_key(field: &str, value: &str) -> bool {
    if credential_like(value) || path_like(value) {
        return false;
    }
    match field {
        "session_uid" | "parent_session_uid" | "lane_root_uid" => valid_uid(value, "", 3),
        "turn_uid" => valid_uid(value, "t", 5),
        "step_uid" => valid_step_uid(value),
        "compaction_uid" => valid_uid(value, "c", 5),
        "repo_key" => valid_repository_key(value),
        "machine_key" | "harness_key" | "tool_key" | "model_key" | "price_version" => {
            valid_key_text(value)
        }
        // fact_label is refused before row validation. Unknown key contracts are
        // not accepted as generic text metadata.
        _ => false,
    }
}

fn valid_name(field: &str, value: &str) -> bool {
    match field {
        "display_name" => valid_display_name(value),
        "branch" => valid_branch(value),
        "model" | "provider" | "tool_key" | "harness_key" | "repo_id" | "tier" | "speed"
        | "service_tier" | "command_tag" => valid_metadata_text(value, false),
        // fact_label is refused above. Do not turn its textual categorical fields
        // into a generic "safe metadata" escape hatch.
        _ => false,
    }
}

fn valid_uid(value: &str, marker: &str, components: usize) -> bool {
    let parts = value.split('/').collect::<Vec<_>>();
    parts.len() == components
        && parts.iter().all(|part| valid_encoded_component(part))
        && (marker.is_empty() || parts.get(3) == Some(&marker))
}

fn valid_step_uid(value: &str) -> bool {
    let parts = value.split('/').collect::<Vec<_>>();
    parts.iter().all(|part| valid_encoded_component(part))
        && parts.get(3) == Some(&"s")
        && matches!(
            parts.as_slice(),
            [_, _, _, "s", "claude", _] | [_, _, _, "s", "codex", _, _, _, _]
        )
}

fn valid_repository_key(value: &str) -> bool {
    value == "unknown" || valid_remote_repository_id(value) || valid_local_repository_id(value)
}

/// `RepoId::Remote` is the native, sanitized `host[:port]/path` identity.
/// A local repository instead receives an opaque single-component ID; paths
/// remain path-class values and are never a repository key.
fn valid_remote_repository_id(value: &str) -> bool {
    let Some((host, path)) = value.split_once('/') else {
        return false;
    };
    !host.is_empty()
        && host.len() <= 253
        && host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
        && !path.is_empty()
        && value.len() <= 4096
        && !path.chars().any(char::is_control)
        && !path.contains(['?', '#'])
}

fn valid_local_repository_id(value: &str) -> bool {
    value.len() <= 256 && valid_encoded_component(value)
}

fn valid_key_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

fn valid_encoded_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 4096 {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 >= bytes.len()
                    || !bytes[index + 1].is_ascii_hexdigit()
                    || !bytes[index + 2].is_ascii_hexdigit()
                {
                    return false;
                }
                index += 3;
            }
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') => {
                index += 1;
            }
            _ => return false,
        }
    }
    true
}

fn valid_display_name(value: &str) -> bool {
    valid_metadata_text(value, true)
        && value.len() <= 160
        && value.chars().all(|character| {
            character.is_alphanumeric()
                || matches!(character, ' ' | '-' | '_' | '.' | '(' | ')' | '[' | ']')
        })
}

fn valid_branch(value: &str) -> bool {
    valid_metadata_text(value, false)
        && !value.starts_with('/')
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

fn valid_metadata_text(value: &str, allow_spaces: bool) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !credential_like(value)
        && !path_like(value)
        && value.chars().all(|character| {
            !character.is_control() && (allow_spaces || !character.is_whitespace())
        })
}

fn valid_receiver_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !credential_like(value)
        && !value.chars().any(char::is_control)
}

fn credential_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "password=",
        "secret=",
        "token=",
        "api_key=",
        "apikey=",
        "authorization:",
        "bearer ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || known_token_shape(value)
}

struct TokenShape {
    prefix: &'static str,
    min: usize,
    exact: bool,
    body: fn(u8) -> bool,
}

const TOKEN_SHAPES: &[TokenShape] = &[
    TokenShape {
        prefix: "sk-",
        min: 20,
        exact: false,
        body: token_char,
    },
    TokenShape {
        prefix: "ghp_",
        min: 20,
        exact: false,
        body: alnum,
    },
    TokenShape {
        prefix: "gho_",
        min: 20,
        exact: false,
        body: alnum,
    },
    TokenShape {
        prefix: "ghs_",
        min: 20,
        exact: false,
        body: alnum,
    },
    TokenShape {
        prefix: "ghu_",
        min: 20,
        exact: false,
        body: alnum,
    },
    TokenShape {
        prefix: "ghr_",
        min: 20,
        exact: false,
        body: alnum,
    },
    TokenShape {
        prefix: "github_pat_",
        min: 20,
        exact: false,
        body: alnum_underscore,
    },
    TokenShape {
        prefix: "AKIA",
        min: 16,
        exact: true,
        body: upper_digit,
    },
    TokenShape {
        prefix: "ASIA",
        min: 16,
        exact: true,
        body: upper_digit,
    },
    TokenShape {
        prefix: "xoxb-",
        min: 10,
        exact: false,
        body: alnum_dash,
    },
    TokenShape {
        prefix: "xoxp-",
        min: 10,
        exact: false,
        body: alnum_dash,
    },
    TokenShape {
        prefix: "xoxa-",
        min: 10,
        exact: false,
        body: alnum_dash,
    },
    TokenShape {
        prefix: "xoxr-",
        min: 10,
        exact: false,
        body: alnum_dash,
    },
    TokenShape {
        prefix: "xoxs-",
        min: 10,
        exact: false,
        body: alnum_dash,
    },
    TokenShape {
        prefix: "xapp-",
        min: 10,
        exact: false,
        body: alnum_dash,
    },
];

fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}
fn alnum(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}
fn alnum_underscore(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
fn upper_digit(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}
fn alnum_dash(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

fn known_token_shape(value: &str) -> bool {
    let bytes = value.as_bytes();
    for start in 0..bytes.len() {
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        for shape in TOKEN_SHAPES {
            let prefix = shape.prefix.as_bytes();
            if !bytes[start..].starts_with(prefix) {
                continue;
            }
            let body_start = start + prefix.len();
            let body = bytes[body_start..]
                .iter()
                .take_while(|byte| (shape.body)(**byte))
                .count();
            if body >= shape.min
                && (!shape.exact
                    || (body == shape.min
                        && bytes
                            .get(body_start + body)
                            .is_none_or(|next| !next.is_ascii_alphanumeric())))
            {
                return true;
            }
        }
    }
    false
}

fn path_like(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("~\\")
        || value.starts_with("file://")
        || value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
            && value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            && value
                .as_bytes()
                .get(2)
                .is_some_and(|byte| matches!(*byte, b'/' | b'\\'))
}

fn valid_path_token(value: &str) -> bool {
    value.strip_prefix("pt1_").is_some_and(|token| {
        token.len() == 22
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    })
}

fn valid_subject(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= 256
        && !value.chars().any(char::is_control)
}

fn valid_scope_component(value: &str) -> bool {
    valid_subject(value) && !value.contains(':')
}

fn hash_value(value: &Value) -> String {
    let digest = Sha256::digest(canonical_json(value).as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map.iter().collect();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(items) => {
            let mut canonical = items.iter().map(canonical_json).collect::<Vec<_>>();
            canonical.sort();
            format!("[{}]", canonical.join(","))
        }
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(destination: Destination) -> Scope {
        Scope {
            destination,
            namespace: "ns".into(),
            machine: "box".into(),
        }
    }

    fn policy(fields: &[&str], body_read: bool, consent: bool) -> Policy {
        Policy {
            format: 1,
            sensitivity: Sensitivity::Public,
            principals: ["alice".into()].into(),
            groups: BTreeSet::new(),
            scopes: [scope(Destination::Receiver), scope(Destination::Hub)].into(),
            analytics_fields: fields.iter().map(|field| (*field).into()).collect(),
            allow_row_existence: true,
            allow_coverage: true,
            body_read,
            provider_egress: consent
                .then(|| EgressConsent {
                    provider: "provider".into(),
                    model: "model".into(),
                    evidence_classes: ["step".into()].into(),
                })
                .into_iter()
                .collect(),
            max_ttl_seconds: 60,
            max_bytes: 4096,
            max_requests_per_minute: 4,
        }
    }

    fn bundle(fields: &[&str], body_read: bool, consent: bool) -> PolicyBundle {
        PolicyBundle {
            baseline: policy(fields, body_read, consent),
            local: policy(fields, body_read, consent),
        }
    }

    fn source(id: &str) -> Source {
        Source {
            id: id.into(),
            owner: "owner".into(),
            namespace: "ns".into(),
            machine: "box".into(),
            floor: Sensitivity::Public,
            scan: ScanEvidence {
                state: ScanState::Completed,
                detector_version: "detector-v1".into(),
                redactor_version: "redactor-v1".into(),
                secret_findings: 0,
                pii_findings: 0,
            },
        }
    }

    fn principal() -> Principal {
        Principal {
            id: "alice".into(),
            groups: BTreeSet::new(),
            namespace: "ns".into(),
            machine: "box".into(),
        }
    }

    fn authority(destination: Destination, fields: &[&str]) -> ExportAuthority {
        evaluate(
            &[("source".into(), bundle(fields, true, true))].into(),
            &[source("source")],
            &principal(),
            &scope(destination),
        )
        .unwrap()
    }

    fn repo_row(path: Value) -> Map<String, Value> {
        Map::from_iter([
            (
                "repo_key".into(),
                Value::String("github.com/onsager-ai/semon".into()),
            ),
            (
                "display_name".into(),
                Value::String("sample repository".into()),
            ),
            ("repo_path".into(), path),
        ])
    }

    fn session_uid() -> &'static str {
        "laptop/codex/native%2Fsession"
    }

    fn turn_uid() -> &'static str {
        "laptop/codex/native%2Fsession/t/turn%201"
    }

    fn step_uid() -> &'static str {
        "laptop/codex/native%2Fsession/s/codex/thread/native%2Fsession/turn/response"
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn strict_content_bound_policy_and_unsupported_l1_are_refused() {
        let json = r#"{"format":1,"sensitivity":"internal","principals":["alice"],"groups":[],"scopes":[{"destination":"receiver","namespace":"ns","machine":"box"}],"analytics_fields":["session_uid"],"allow_row_existence":true,"allow_coverage":true,"body_read":false,"provider_egress":[],"max_ttl_seconds":0,"max_bytes":0,"max_requests_per_minute":0}"#;
        let parsed = Policy::from_json(json).unwrap();
        assert!(parsed.content_version().unwrap().starts_with("sha256:"));
        assert!(Policy::from_json(&json.replace("\"format\":1", "\"format\":2")).is_err());
        assert!(
            Policy::from_json(&json.replace("\"format\":1", "\"format\":1,\"extra\":true"))
                .is_err()
        );
        assert!(Policy::from_json(&json.replace("\"session_uid\"", "\"field\"")).is_err());

        let grant = authority(Destination::Receiver, &["session_uid"]);
        let row = Map::from_iter([
            ("label_id".into(), Value::String("l1".into())),
            ("field".into(), Value::String("goal".into())),
            ("value".into(), Value::String("confidential prose".into())),
        ]);
        assert!(matches!(
            project_derived_row(&grant, "fact_label", &row),
            Err(GovernanceError::Denied)
        ));
    }

    #[test]
    fn identifiers_reject_credential_and_path_laundering_but_receiver_paths_are_typed() {
        let grant = authority(
            Destination::Receiver,
            &["repo_key", "display_name", "repo_path"],
        );
        assert!(
            project_derived_row(
                &grant,
                "dim_repo",
                &repo_row(Value::String("/home/example/project".into()))
            )
            .is_ok()
        );
        for (field, value) in [
            ("repo_key", "token=synthetic-secret"),
            ("repo_key", "/tmp/token=synthetic-secret"),
            ("display_name", "C:\\\\token=synthetic-secret"),
            ("repo_key", "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"),
        ] {
            let mut row = repo_row(Value::String("/home/example/project".into()));
            row.insert(field.into(), Value::String(value.into()));
            assert!(matches!(
                project_derived_row(&grant, "dim_repo", &row),
                Err(GovernanceError::Denied)
            ));
        }
        assert!(matches!(
            project_derived_row(
                &grant,
                "dim_repo",
                &repo_row(Value::String("/tmp/token=synthetic-secret".into()))
            ),
            Err(GovernanceError::Denied)
        ));
        let benign = Map::from_iter([
            ("repo_key".into(), Value::String("sk-short".into())),
            ("display_name".into(), Value::String("task-worker".into())),
            (
                "repo_path".into(),
                Value::String("/work/task-worker".into()),
            ),
        ]);
        assert!(project_derived_row(&grant, "dim_repo", &benign).is_ok());
        // Native repository paths are literal strings, unlike encoded UIDs.
        let mut literal = repo_row(Value::String("/work/repository".into()));
        literal.insert("repo_key".into(), Value::String("github.com/%GG".into()));
        let row = project_derived_row(&grant, "dim_repo", &literal).unwrap();
        let exported: Value = serde_json::from_str(&serialize_governed_row(&row).unwrap()).unwrap();
        assert_eq!(exported["repo_key"], "github.com/%GG");
        assert!(!valid_key("session_uid", "ns/claude/%GG"));
    }

    #[test]
    fn complete_grant_accepts_origin_local_and_remote_repository_rows() {
        let grant = authority(
            Destination::Receiver,
            &["repo_key", "display_name", "repo_path"],
        );
        for row in [
            Map::from_iter([
                (
                    "repo_key".into(),
                    Value::String("local-0123456789abcdef".into()),
                ),
                ("display_name".into(), Value::String("local".into())),
                ("repo_path".into(), Value::String("/work/local".into())),
            ]),
            Map::from_iter([
                (
                    "repo_key".into(),
                    Value::String("github.com/onsager-ai/semon".into()),
                ),
                (
                    "display_name".into(),
                    Value::String("remote repository".into()),
                ),
                ("repo_path".into(), Value::Null),
            ]),
        ] {
            assert!(project_derived_row(&grant, "dim_repo", &row).is_ok());
        }
    }

    #[test]
    fn typed_keys_accept_documented_uids_repositories_and_integer_dates() {
        let date_fields = [
            "date_key",
            "day_start",
            "year",
            "month",
            "day",
            "iso_week",
            "weekday",
        ];
        let date = object(serde_json::json!({
            "date_key": 20261109,
            "day_start": 1794182400000_i64,
            "year": 2026,
            "month": 11,
            "day": 9,
            "iso_week": 46,
            "weekday": 1,
        }));
        assert!(
            project_derived_row(
                &authority(Destination::Receiver, &date_fields),
                "dim_date",
                &date,
            )
            .is_ok()
        );

        let session_fields = [
            "session_uid",
            "date_key",
            "started_at",
            "ended_at",
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_5m_tokens",
            "cache_write_1h_tokens",
            "reasoning_tokens",
            "web_search_requests",
            "usd_est",
            "reported_usd",
            "wall_ms",
            "model_ms",
            "step_count",
            "failure_count",
            "usage_complete",
            "source_status",
            "computation_status",
            "exclusive_step_count",
            "unknown_step_count",
            "unclassified_step_count",
            "taxonomy_version",
            "transform_version",
            "rules_version",
            "price_version",
        ];
        let session = object(serde_json::json!({
            "session_uid": session_uid(), "date_key": 20261109, "started_at": 0_i64,
            "ended_at": null, "input_tokens": 0, "output_tokens": 0,
            "cache_read_tokens": 0, "cache_write_5m_tokens": 0,
            "cache_write_1h_tokens": 0, "reasoning_tokens": 0,
            "web_search_requests": 0, "usd_est": null, "reported_usd": null,
            "wall_ms": 0, "model_ms": 0, "step_count": 1, "failure_count": 0,
            "usage_complete": true, "source_status": "complete", "computation_status": "current",
            "exclusive_step_count": 1, "unknown_step_count": 0, "unclassified_step_count": 0,
            "taxonomy_version": null, "transform_version": 1, "rules_version": 1,
            "price_version": "pricing:v1",
        }));
        assert!(
            project_derived_row(
                &authority(Destination::Receiver, &session_fields),
                "fact_session",
                &session,
            )
            .is_ok()
        );

        let step_fields = [
            "step_uid",
            "turn_uid",
            "session_uid",
            "model_key",
            "repo_key",
            "date_key",
            "ts",
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_5m_tokens",
            "cache_write_1h_tokens",
            "reasoning_tokens",
            "web_search_requests",
            "speed",
            "service_tier",
            "prompt_size_tokens",
            "split_unknown",
            "usage_owner",
            "context_tokens",
            "context_band",
            "model_ms",
            "tool_ms",
            "usd_est",
            "input_usd",
            "cache_read_usd",
            "cache_write_usd",
            "output_usd",
            "price_status",
            "price_version",
            "activity",
            "waste_kind",
            "trigger",
            "ok",
        ];
        let step = object(serde_json::json!({
            "step_uid": step_uid(), "turn_uid": turn_uid(), "session_uid": session_uid(),
            "model_key": "anthropic/claude-sonnet-4-6",
            "repo_key": "github.com/onsager-ai/semon", "date_key": 20261109,
            "ts": 0_i64, "input_tokens": 0, "output_tokens": 0,
            "cache_read_tokens": 0, "cache_write_5m_tokens": 0,
            "cache_write_1h_tokens": 0, "reasoning_tokens": 0,
            "web_search_requests": 0, "speed": null, "service_tier": null,
            "prompt_size_tokens": 0, "split_unknown": false, "usage_owner": "exclusive",
            "context_tokens": 0, "context_band": "under_50k", "model_ms": null,
            "tool_ms": null, "usd_est": null, "input_usd": null, "cache_read_usd": null,
            "cache_write_usd": null, "output_usd": null, "price_status": "unpriced",
            "price_version": "pricing:v1", "activity": "implement", "waste_kind": null,
            "trigger": null, "ok": true,
        }));
        assert!(
            project_derived_row(
                &authority(Destination::Receiver, &step_fields),
                "fact_step",
                &step,
            )
            .is_ok()
        );

        let session_dimension_fields = [
            "session_uid",
            "machine_key",
            "harness_key",
            "role",
            "parent_session_uid",
            "parent_method",
            "lane_root_uid",
            "repo_key",
            "branch",
            "status",
            "started_at",
            "ended_at",
        ];
        let session_dimension = object(serde_json::json!({
            "session_uid": session_uid(), "machine_key": "laptop", "harness_key": "codex",
            "role": "worker", "parent_session_uid": "laptop/codex/parent%2Fsession",
            "parent_method": "native", "lane_root_uid": "laptop/codex/root%2Fsession",
            "repo_key": "example.test:8443/team/app", "branch": "feature/governance",
            "status": "active", "started_at": 0_i64, "ended_at": null,
        }));
        assert!(
            project_derived_row(
                &authority(Destination::Receiver, &session_dimension_fields),
                "dim_session",
                &session_dimension,
            )
            .is_ok()
        );
    }

    #[test]
    fn opaque_authority_requires_an_admitted_complete_cohort() {
        let policies = BTreeMap::new();
        assert!(matches!(
            evaluate(
                &policies,
                &[source("missing")],
                &principal(),
                &scope(Destination::Receiver)
            ),
            Err(GovernanceError::InvalidSourceSet)
        ));
        let grant = authority(
            Destination::Receiver,
            &["repo_key", "display_name", "repo_path"],
        );
        let row = project_derived_row(
            &grant,
            "dim_repo",
            &repo_row(Value::String("/home/example/project".into())),
        )
        .unwrap();
        assert_eq!(
            serialize_governed_row(&row).unwrap(),
            include_str!("../tests/fixtures/governance-receiver-row.json").trim()
        );
        assert_eq!(format!("{grant:?}"), "ExportAuthority { private }");
    }

    #[test]
    fn source_floor_is_part_of_the_bound_authority() {
        let policies = [("source".into(), bundle(&["session_uid"], false, false))].into();
        let mut admitted = source("source");
        admitted.floor = Sensitivity::Restricted;
        let grant = evaluate(
            &policies,
            &[admitted],
            &principal(),
            &scope(Destination::Receiver),
        )
        .unwrap();
        assert_eq!(sensitivity_floor(&grant), Sensitivity::Restricted);
    }

    #[test]
    fn destination_is_bound_and_hub_never_receives_bodies() {
        let receiver = authority(
            Destination::Receiver,
            &["repo_key", "display_name", "repo_path"],
        );
        assert!(body_read_allowed(&receiver));
        assert!(
            project_derived_row(
                &receiver,
                "dim_repo",
                &repo_row(Value::String("/home/example/project".into()))
            )
            .is_ok()
        );

        let hub = authority(Destination::Hub, &["repo_key", "display_name", "repo_path"]);
        assert!(!body_read_allowed(&hub));
        assert!(matches!(
            project_derived_row(
                &hub,
                "dim_repo",
                &repo_row(Value::String("/home/example/project".into()))
            ),
            Err(GovernanceError::Denied)
        ));
        let row = project_derived_row(
            &hub,
            "dim_repo",
            &repo_row(Value::String(format!("pt1_{}", "A".repeat(22)))),
        )
        .unwrap();
        assert_eq!(
            serialize_governed_row(&row).unwrap(),
            include_str!("../tests/fixtures/governance-hub-row.json").trim()
        );
    }

    #[test]
    fn coverage_denial_does_not_serialize_private_provenance() {
        let mut policy = policy(&["repo_key", "display_name", "repo_path"], false, true);
        policy.allow_coverage = false;
        let policies = [(
            "source".into(),
            PolicyBundle {
                baseline: policy.clone(),
                local: policy,
            },
        )]
        .into();
        let grant = evaluate(
            &policies,
            &[source("source")],
            &principal(),
            &scope(Destination::Receiver),
        )
        .unwrap();
        assert!(!coverage_allowed(&grant));
        let row = project_derived_row(
            &grant,
            "dim_repo",
            &repo_row(Value::String("/home/example/project".into())),
        )
        .unwrap();
        let serialized = serialize_governed_row(&row).unwrap();
        let debug = format!("{row:?}");
        for private in [
            "source",
            "detector",
            "redactor",
            "sha256",
            "policy_versions",
        ] {
            assert!(!serialized.contains(private));
            assert!(!debug.contains(private));
        }
        assert!(matches!(
            coverage_witness(&row),
            Err(GovernanceError::Denied)
        ));
    }

    #[test]
    fn governed_rows_retain_private_witness_and_export_coverage_only_when_allowed() {
        let grant = authority(
            Destination::Receiver,
            &["repo_key", "display_name", "repo_path"],
        );
        let row = project_derived_row(
            &grant,
            "dim_repo",
            &repo_row(Value::String("/home/example/project".into())),
        )
        .unwrap();
        let witness = local_witness(&row);
        assert_eq!(witness.principal_id(), "alice");
        assert_eq!(witness.scope().destination, Destination::Receiver);
        assert_eq!(witness.contributors().len(), 1);
        assert_eq!(witness.contributors()[0].source_id(), "source");
        assert_eq!(witness.contributors()[0].detector_version(), "detector-v1");
        assert_eq!(format!("{witness:?}"), "ExportWitness { private }");
        let coverage = coverage_witness(&row).unwrap();
        let serialized = serialize_coverage_witness(&coverage).unwrap();
        assert!(serialized.contains("source"));
        assert!(serialized.contains("detector-v1"));
        assert!(
            !serialize_governed_row(&row)
                .unwrap()
                .contains("detector-v1")
        );
    }

    #[test]
    fn coverage_export_refuses_sensitive_witness_without_losing_local_provenance() {
        for destination in [Destination::Receiver, Destination::Hub] {
            for field in ["source", "detector", "redactor"] {
                let mut contributor = source("source");
                match field {
                    "source" => contributor.id = "/home/private/session.jsonl".into(),
                    "detector" => {
                        contributor.scan.detector_version = "token=synthetic-secret".into()
                    }
                    _ => contributor.scan.redactor_version = "/home/private/redactor".into(),
                }
                let policies = [(
                    contributor.id.clone(),
                    bundle(&["repo_key", "display_name", "repo_path"], false, false),
                )]
                .into();
                let authority = evaluate(
                    &policies,
                    std::slice::from_ref(&contributor),
                    &principal(),
                    &scope(destination),
                )
                .unwrap();
                let row =
                    project_derived_row(&authority, "dim_repo", &repo_row(Value::Null)).unwrap();
                assert_eq!(
                    local_witness(&row).contributors()[0].source_id(),
                    contributor.id
                );
                let coverage = coverage_witness(&row).unwrap();
                assert!(matches!(
                    serialize_coverage_witness(&coverage),
                    Err(GovernanceError::Denied)
                ));
                assert_eq!(format!("{coverage:?}"), "CoverageWitness { private }");
            }
        }
    }

    #[test]
    fn unicode_names_and_native_remote_repository_paths_are_preserved() {
        for destination in [Destination::Receiver, Destination::Hub] {
            let grant = authority(destination, &["repo_key", "display_name", "repo_path"]);
            for repository in ["git.example/team/café", "git.example/team/a+b"] {
                let mut input = repo_row(Value::Null);
                input.insert("repo_key".into(), Value::String(repository.into()));
                input.insert("display_name".into(), Value::String("研发".into()));
                let row = project_derived_row(&grant, "dim_repo", &input).unwrap();
                let output: Value =
                    serde_json::from_str(&serialize_governed_row(&row).unwrap()).unwrap();
                assert_eq!(output["repo_key"], repository);
                assert_eq!(output["display_name"], "研发");
            }
        }
        assert!(valid_branch("功能/修复"));
        assert!(!valid_branch("/home/private"));
    }

    #[test]
    fn structured_scopes_match_exactly_without_delimiter_collisions() {
        let intended = Scope {
            destination: Destination::Receiver,
            namespace: "team/host".into(),
            machine: "gpu".into(),
        };
        let other = Scope {
            destination: Destination::Receiver,
            namespace: "team".into(),
            machine: "host/gpu".into(),
        };
        let mut policy = policy(&["session_uid"], false, false);
        policy.scopes = [intended.clone()].into();
        let policies = [(
            "source".into(),
            PolicyBundle {
                baseline: policy.clone(),
                local: policy,
            },
        )]
        .into();
        let mut source = source("source");
        source.namespace = intended.namespace.clone();
        source.machine = intended.machine.clone();
        let mut principal = principal();
        principal.namespace = intended.namespace.clone();
        principal.machine = intended.machine.clone();
        assert!(evaluate(&policies, &[source.clone()], &principal, &intended).is_ok());
        source.namespace = other.namespace.clone();
        source.machine = other.machine.clone();
        principal.namespace = other.namespace.clone();
        principal.machine = other.machine.clone();
        assert!(matches!(
            evaluate(&policies, &[source], &principal, &other),
            Err(GovernanceError::Denied)
        ));
    }

    #[test]
    fn all_contributors_must_consent_and_optional_remains_distinct_from_null() {
        let policies = BTreeMap::from([
            (
                "a".into(),
                bundle(&["repo_key", "display_name", "repo_path"], false, true),
            ),
            (
                "b".into(),
                bundle(&["repo_key", "display_name", "repo_path"], false, false),
            ),
        ]);
        let grant = evaluate(
            &policies,
            &[source("a"), source("b")],
            &principal(),
            &scope(Destination::Receiver),
        )
        .unwrap();
        assert!(!provider_egress_allowed(
            &grant, "provider", "model", "step"
        ));
        let partial = Map::from_iter([
            ("repo_key".into(), Value::String("repo-1".into())),
            ("display_name".into(), Value::String("repository".into())),
        ]);
        let partial =
            serialize_governed_row(&project_derived_row(&grant, "dim_repo", &partial).unwrap())
                .unwrap();
        assert!(!partial.contains("repo_path"));
        let null = serialize_governed_row(
            &project_derived_row(&grant, "dim_repo", &repo_row(Value::Null)).unwrap(),
        )
        .unwrap();
        assert!(null.contains("\"repo_path\":null"));
    }

    #[test]
    fn consent_intersects_evidence_classes_for_matching_provider_and_model() {
        let fields = ["session_uid"];
        let mut baseline = policy(&fields, false, false);
        baseline.provider_egress = vec![EgressConsent {
            provider: "provider".into(),
            model: "model".into(),
            evidence_classes: ["step".into(), "turn".into()].into(),
        }];
        let mut local = baseline.clone();
        local.provider_egress[0].evidence_classes = ["step".into()].into();
        let policies = [("source".into(), PolicyBundle { baseline, local })].into();
        let grant = evaluate(
            &policies,
            &[source("source")],
            &principal(),
            &scope(Destination::Receiver),
        )
        .unwrap();
        assert!(provider_egress_allowed(&grant, "provider", "model", "step"));
        assert!(!provider_egress_allowed(
            &grant, "provider", "model", "turn"
        ));
    }
}

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
    policy_version: String,
    sensitivity: Sensitivity,
    fields: BTreeSet<String>,
    allow_row_existence: bool,
    allow_coverage: bool,
    body_read: bool,
    consents: BTreeSet<EgressConsent>,
    scan_complete_without_findings: bool,
}

/// A non-forgeable result of evaluating a complete admitted contributor cohort.
///
/// The fields are private so callers cannot construct a permissive authority,
/// replay it to another destination, or drop a restrictive contributor.
pub struct ExportAuthority {
    scope: Scope,
    principal_id: String,
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

    Ok(ExportAuthority {
        scope: scope.clone(),
        principal_id: principal.id.clone(),
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
    authority.scope.destination == Destination::Receiver
        && authority
            .contributors
            .iter()
            .all(|grant| grant.body_read && grant.scan_complete_without_findings)
}

/// Coverage values, counts, source handles, and version vectors must be withheld
/// unless every contributor allows coverage disclosure.
pub fn coverage_allowed(authority: &ExportAuthority) -> bool {
    authority
        .contributors
        .iter()
        .all(|grant| grant.allow_coverage)
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
    if authority.principal_id.is_empty()
        || authority.contributors.is_empty()
        || authority
            .contributors
            .iter()
            .any(|grant| grant.policy_version.is_empty() || !grant.allow_row_existence)
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
        validate_column_value(column, value, authority.scope.destination)?;
        projected.insert(name.clone(), value.clone());
    }
    for column in table.columns {
        if !column.nullable
            && (!fields.contains(column.name) || !projected.contains_key(column.name))
        {
            return Err(GovernanceError::InvalidRow);
        }
    }
    Ok(GovernedRow { fields: projected })
}

/// Serialize only an already-governed row. Internal policy, scan, provenance,
/// source, and coverage metadata are never part of this envelope.
pub fn serialize_governed_row(row: &GovernedRow) -> Result<String, GovernanceError> {
    serde_json::to_string(&row.fields).map_err(|_| GovernanceError::InvalidRow)
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
        policy_version,
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
        consents: bundle
            .baseline
            .provider_egress
            .iter()
            .filter(|consent| bundle.local.provider_egress.contains(consent))
            .cloned()
            .collect(),
        scan_complete_without_findings: source.scan.state == ScanState::Completed
            && source.scan.secret_findings == 0
            && source.scan.pii_findings == 0,
    })
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
        Class::Key => {
            if !text.is_some_and(|text| valid_key(column.name, text)) {
                return Err(GovernanceError::Denied);
            }
        }
        Class::Name => {
            if !text.is_some_and(|text| valid_name(column.name, text)) {
                return Err(GovernanceError::Denied);
            }
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

fn valid_key(_field: &str, value: &str) -> bool {
    valid_metadata_text(value, false) && valid_identifier(value)
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

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn valid_display_name(value: &str) -> bool {
    valid_metadata_text(value, true)
        && value.len() <= 160
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
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
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
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
        "sk-",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
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
            ("repo_key".into(), Value::String("repo-1".into())),
            (
                "display_name".into(),
                Value::String("sample repository".into()),
            ),
            ("repo_path".into(), path),
        ])
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
}

//! Deterministic, local policy evaluation and typed export projection.
//!
//! This library does not authenticate callers or establish source provenance. A
//! future transport must supply a trusted principal and the complete admitted
//! contributor set. It does not read bodies, audit reads, cache data, or authorize
//! offline body access.

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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub namespace: String,
    pub machine: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    pub scopes: BTreeSet<String>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportProfile {
    Receiver,
    Hub,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluatedPolicy {
    pub source_id: String,
    pub version: String,
    pub sensitivity: Sensitivity,
    pub fields: BTreeSet<String>,
    pub allow_row_existence: bool,
    pub allow_coverage: bool,
    pub body_read: bool,
    pub max_ttl_seconds: u32,
    pub max_bytes: u64,
    pub max_requests_per_minute: u32,
}

/// Authorized output paired with the complete contributor policy-version vector.
#[derive(Clone, Debug, PartialEq)]
pub struct GovernedRow {
    pub fields: Map<String, Value>,
    pub policy_versions: Vec<String>,
    pub scan_versions: Vec<ScanVersion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanVersion {
    pub source_id: String,
    pub state: ScanState,
    pub detector_version: String,
    pub redactor_version: String,
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
        let canonical = canonical_json(&value);
        let digest = Sha256::digest(canonical.as_bytes());
        Ok(format!("sha256:{}", hex::encode(digest)))
    }

    fn validate(&self) -> Result<(), GovernanceError> {
        if self.format != 1
            || self.principals.iter().any(|s| s.trim().is_empty())
            || self.groups.iter().any(|s| s.trim().is_empty())
            || self.scopes.is_empty()
            || self.scopes.iter().any(|s| s.trim().is_empty())
            || self.analytics_fields.iter().any(|s| !known_field(s))
            || self.provider_egress.iter().any(|e| {
                e.provider.trim().is_empty()
                    || e.model.trim().is_empty()
                    || e.evidence_classes.is_empty()
                    || e.evidence_classes.iter().any(|s| s.trim().is_empty())
            })
        {
            return Err(GovernanceError::InvalidPolicy);
        }
        Ok(())
    }
}

impl PolicyBundle {
    /// The evaluated version binds both the organization baseline and local policy.
    pub fn content_version(&self) -> Result<String, GovernanceError> {
        self.baseline.validate()?;
        self.local.validate()?;
        let value = serde_json::json!({"baseline": &self.baseline, "local": &self.local});
        let digest = Sha256::digest(canonical_json(&value).as_bytes());
        Ok(format!("sha256:{}", hex::encode(digest)))
    }
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map.iter().collect();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(k, v)| format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(v)
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

fn known_field(name: &str) -> bool {
    schema::TABLES.iter().any(|table| {
        table
            .column(name)
            .is_some_and(|column| column.class != Class::Body)
    })
}

fn effective(bundle: &PolicyBundle, source_id: &str) -> Result<EvaluatedPolicy, GovernanceError> {
    bundle.baseline.validate()?;
    bundle.local.validate()?;
    let version = bundle.content_version()?;
    Ok(EvaluatedPolicy {
        source_id: source_id.to_owned(),
        version,
        sensitivity: bundle.baseline.sensitivity.max(bundle.local.sensitivity),
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
        max_ttl_seconds: bundle
            .baseline
            .max_ttl_seconds
            .min(bundle.local.max_ttl_seconds),
        max_bytes: bundle.baseline.max_bytes.min(bundle.local.max_bytes),
        max_requests_per_minute: bundle
            .baseline
            .max_requests_per_minute
            .min(bundle.local.max_requests_per_minute),
    })
}

/// Evaluate all source policies against a trusted principal and complete source set.
pub fn evaluate(
    policies: &BTreeMap<String, PolicyBundle>,
    sources: &[Source],
    principal: &Principal,
    scope: &Scope,
    destination: &str,
) -> Result<Vec<EvaluatedPolicy>, GovernanceError> {
    if sources.is_empty() || destination.trim().is_empty() {
        return Err(GovernanceError::InvalidSourceSet);
    }
    let mut ids = BTreeSet::new();
    let mut out = Vec::with_capacity(sources.len());
    for source in sources {
        if source.id.trim().is_empty()
            || source.owner.trim().is_empty()
            || !ids.insert(&source.id)
            || source.namespace != scope.namespace
            || source.machine != scope.machine
            || source.scan.detector_version.trim().is_empty()
            || source.scan.redactor_version.trim().is_empty()
        {
            return Err(GovernanceError::InvalidSourceSet);
        }
        let bundle = policies
            .get(&source.id)
            .ok_or(GovernanceError::InvalidSourceSet)?;
        let mut evaluated = effective(bundle, &source.id)?;
        evaluated.sensitivity = evaluated.sensitivity.max(source.floor);
        let scope_key = format!("{}:{}:{}", destination, scope.namespace, scope.machine);
        let allowed = [&bundle.baseline, &bundle.local].iter().all(|p| {
            p.scopes.contains(&scope_key)
                && (p.principals.contains(&principal.id)
                    || !p.groups.is_disjoint(&principal.groups))
        });
        if !allowed || principal.namespace != scope.namespace || principal.machine != scope.machine
        {
            return Err(GovernanceError::Denied);
        }
        out.push(evaluated);
    }
    Ok(out)
}

/// Explicit consent from every contributor is required for provider/model evidence egress.
pub fn provider_egress_allowed(
    policies: &BTreeMap<String, PolicyBundle>,
    sources: &[Source],
    provider: &str,
    model: &str,
    evidence_class: &str,
) -> bool {
    let mut ids = BTreeSet::new();
    !sources.is_empty()
        && sources.iter().all(|s| {
            !s.id.trim().is_empty()
                && !s.owner.trim().is_empty()
                && ids.insert(&s.id)
                && policies.get(&s.id).is_some_and(|b| {
                    b.baseline.validate().is_ok() && b.local.validate().is_ok() && {
                        [&b.baseline, &b.local].iter().all(|p| {
                            p.provider_egress.iter().any(|c| {
                                c.provider == provider
                                    && c.model == model
                                    && c.evidence_classes.contains(evidence_class)
                            })
                        })
                    }
                })
        })
}

/// A body may be considered only after complete successful scans and unanimous policy consent.
/// Findings are preserved as evidence; this API does not claim detection is complete.
pub fn body_read_allowed(evaluated: &[EvaluatedPolicy], sources: &[Source]) -> bool {
    !sources.is_empty()
        && evaluated.len() == sources.len()
        && sources
            .iter()
            .zip(evaluated)
            .all(|(s, p)| s.id == p.source_id)
        && evaluated.iter().all(|p| p.body_read)
        && sources.iter().all(|s| {
            s.scan.state == ScanState::Completed
                && s.scan.secret_findings == 0
                && s.scan.pii_findings == 0
        })
}

/// Coverage details may be returned only when every contributor permits disclosure.
pub fn coverage_allowed(evaluated: &[EvaluatedPolicy]) -> bool {
    !evaluated.is_empty() && evaluated.iter().all(|p| p.allow_coverage)
}

/// Project a derived row only when every contributor grants its existence and fields.
/// A denied field or row withholds the entire row; no partial total is produced.
pub fn project_derived_row(
    table: &str,
    row: &Map<String, Value>,
    profile: ExportProfile,
    evaluated: &[EvaluatedPolicy],
    sources: &[Source],
) -> Result<GovernedRow, GovernanceError> {
    if evaluated.is_empty()
        || evaluated.len() != sources.len()
        || sources
            .iter()
            .zip(evaluated)
            .any(|(s, p)| s.id != p.source_id)
        || evaluated
            .iter()
            .any(|p| !p.allow_row_existence || row.keys().any(|field| !p.fields.contains(field)))
    {
        return Err(GovernanceError::Denied);
    }
    // Intersect limits and field grants into one row authority.
    let fields = evaluated
        .iter()
        .skip(1)
        .fold(evaluated[0].fields.clone(), |acc, p| {
            acc.intersection(&p.fields).cloned().collect()
        });
    let merged = EvaluatedPolicy {
        source_id: String::new(),
        version: evaluated
            .iter()
            .map(|p| p.version.as_str())
            .collect::<Vec<_>>()
            .join(","),
        sensitivity: evaluated
            .iter()
            .map(|p| p.sensitivity)
            .max()
            .ok_or(GovernanceError::InvalidSourceSet)?,
        fields,
        allow_row_existence: true,
        allow_coverage: evaluated.iter().all(|p| p.allow_coverage),
        body_read: evaluated.iter().all(|p| p.body_read),
        max_ttl_seconds: evaluated
            .iter()
            .map(|p| p.max_ttl_seconds)
            .min()
            .unwrap_or(0),
        max_bytes: evaluated.iter().map(|p| p.max_bytes).min().unwrap_or(0),
        max_requests_per_minute: evaluated
            .iter()
            .map(|p| p.max_requests_per_minute)
            .min()
            .unwrap_or(0),
    };
    Ok(GovernedRow {
        fields: project_row(table, row, profile, &merged)?,
        policy_versions: evaluated.iter().map(|p| p.version.clone()).collect(),
        scan_versions: sources
            .iter()
            .map(|s| ScanVersion {
                source_id: s.id.clone(),
                state: s.scan.state,
                detector_version: s.scan.detector_version.clone(),
                redactor_version: s.scan.redactor_version.clone(),
            })
            .collect(),
    })
}

/// Validate and project a row to the current semon-asi schema allowlist.
pub fn project_row(
    table_name: &str,
    row: &Map<String, Value>,
    profile: ExportProfile,
    policy: &EvaluatedPolicy,
) -> Result<Map<String, Value>, GovernanceError> {
    let table = schema::table(table_name).ok_or(GovernanceError::InvalidRow)?;
    let mut projected = Map::new();
    for (name, value) in row {
        let column = table.column(name).ok_or(GovernanceError::InvalidRow)?;
        if column.class == Class::Body || !policy.fields.contains(name) {
            return Err(GovernanceError::Denied);
        }
        if column.class == Class::Name && !name_allowlist(name) {
            return Err(GovernanceError::Denied);
        }
        let value = value.clone();
        if column.class == Class::Name && value.as_str().is_some_and(credential_like) {
            return Err(GovernanceError::Denied);
        }
        if column.class == Class::Path {
            match profile {
                ExportProfile::Receiver => {
                    validate_value(column.data_type, column.nullable, &value, column.values)?
                }
                ExportProfile::Hub => {
                    if !value.is_null() {
                        let token = value.as_str().ok_or(GovernanceError::InvalidRow)?;
                        if !valid_path_token(token) {
                            return Err(GovernanceError::Denied);
                        }
                    }
                    validate_value(column.data_type, column.nullable, &value, column.values)?;
                }
            }
        } else {
            validate_value(column.data_type, column.nullable, &value, column.values)?;
        }
        projected.insert(name.clone(), value);
    }
    for column in table.columns {
        if !column.nullable
            && (!policy.fields.contains(column.name) || !projected.contains_key(column.name))
        {
            return Err(GovernanceError::InvalidRow);
        }
    }
    Ok(projected)
}

/// Serialize only a row that passed the destination profile and policy projection.
pub fn serialize_row(
    table: &str,
    row: &Map<String, Value>,
    profile: ExportProfile,
    policy: &EvaluatedPolicy,
) -> Result<String, GovernanceError> {
    let projected = project_row(table, row, profile, policy)?;
    serde_json::to_string(&projected).map_err(|_| GovernanceError::InvalidRow)
}

fn name_allowlist(name: &str) -> bool {
    matches!(
        name,
        "display_name"
            | "model"
            | "provider"
            | "tool_key"
            | "harness_key"
            | "branch"
            | "repo_id"
            | "tier"
            | "speed"
            | "service_tier"
            | "command_tag"
            | "labeller"
            | "field"
            | "value"
    )
}
fn credential_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "password=",
        "secret=",
        "token=",
        "api_key=",
        "authorization:",
        "bearer ",
        "sk-",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}
fn valid_path_token(s: &str) -> bool {
    s.strip_prefix("pt1_").is_some_and(|v| {
        v.len() == 22
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
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
            .is_some_and(|s| s.len() <= 4096 && !s.as_bytes().contains(&0)),
        Type::Integer | Type::Timestamp => value.as_i64().is_some(),
        Type::Float => value.as_f64().is_some_and(f64::is_finite),
        Type::Boolean => value.as_bool().is_some(),
    };
    if !valid {
        return Err(GovernanceError::InvalidRow);
    }
    if !values.is_empty() && !value.as_str().is_some_and(|s| values.contains(&s)) {
        return Err(GovernanceError::InvalidRow);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy(fields: &[&str], sensitivity: Sensitivity, body: bool, consent: bool) -> Policy {
        Policy {
            format: 1,
            sensitivity,
            principals: ["alice".into()].into(),
            groups: BTreeSet::new(),
            scopes: ["receiver:ns:box".into(), "hub:ns:box".into()].into(),
            analytics_fields: fields.iter().map(|s| s.to_string()).collect(),
            allow_row_existence: true,
            allow_coverage: true,
            body_read: body,
            provider_egress: if consent {
                vec![EgressConsent {
                    provider: "p".into(),
                    model: "m".into(),
                    evidence_classes: ["step".into()].into(),
                }]
            } else {
                vec![]
            },
            max_ttl_seconds: 60,
            max_bytes: 4096,
            max_requests_per_minute: 4,
        }
    }
    fn bundle(
        fields: &[&str],
        sensitivity: Sensitivity,
        body: bool,
        consent: bool,
    ) -> PolicyBundle {
        PolicyBundle {
            baseline: policy(fields, sensitivity, body, consent),
            local: policy(fields, sensitivity, body, consent),
        }
    }
    #[test]
    fn strict_policy_and_content_bound_version() {
        let json = r#"{"format":1,"sensitivity":"internal","principals":["alice"],"groups":[],"scopes":["receiver:ns:box"],"analytics_fields":["session_uid"],"allow_row_existence":true,"allow_coverage":true,"body_read":false,"provider_egress":[],"max_ttl_seconds":0,"max_bytes":0,"max_requests_per_minute":0}"#;
        let p = Policy::from_json(json).unwrap();
        assert!(p.content_version().unwrap().starts_with("sha256:"));
        assert!(Policy::from_json(&json.replace("\"format\":1", "\"format\":2")).is_err());
        assert!(
            Policy::from_json(&json.replace("\"format\":1", "\"format\":1,\"unexpected\":true"))
                .is_err()
        );
        let original = bundle(&["session_uid"], Sensitivity::Internal, false, false);
        let version = original.content_version().unwrap();
        let mut changed_baseline = original.clone();
        changed_baseline.baseline.sensitivity = Sensitivity::Restricted;
        assert_ne!(version, changed_baseline.content_version().unwrap());
    }
    #[test]
    fn source_floor_and_unknown_scans_are_honest() {
        let source = Source {
            id: "s".into(),
            owner: "o".into(),
            namespace: "ns".into(),
            machine: "box".into(),
            floor: Sensitivity::Restricted,
            scan: ScanEvidence {
                state: ScanState::Unknown,
                detector_version: "d1".into(),
                redactor_version: "r1".into(),
                secret_findings: 0,
                pii_findings: 0,
            },
        };
        let principal = Principal {
            id: "alice".into(),
            groups: BTreeSet::new(),
            namespace: "ns".into(),
            machine: "box".into(),
        };
        let got = evaluate(
            &[(
                "s".into(),
                bundle(&["session_uid"], Sensitivity::Public, true, false),
            )]
            .into(),
            &[source],
            &principal,
            &Scope {
                namespace: "ns".into(),
                machine: "box".into(),
            },
            "receiver",
        )
        .unwrap();
        assert_eq!(got[0].sensitivity, Sensitivity::Restricted);
        assert!(!body_read_allowed(
            &got,
            &[Source {
                id: "s".into(),
                owner: "o".into(),
                namespace: "ns".into(),
                machine: "box".into(),
                floor: Sensitivity::Restricted,
                scan: ScanEvidence {
                    state: ScanState::Unknown,
                    detector_version: "d1".into(),
                    redactor_version: "r1".into(),
                    secret_findings: 0,
                    pii_findings: 0
                },
            }]
        ));
    }
    #[test]
    fn contributor_provider_opt_out_and_hub_path_validation() {
        let p = BTreeMap::from([
            (
                "a".into(),
                bundle(
                    &["repo_path", "repo_key", "display_name"],
                    Sensitivity::Public,
                    false,
                    true,
                ),
            ),
            (
                "b".into(),
                bundle(
                    &["repo_path", "repo_key", "display_name"],
                    Sensitivity::Public,
                    false,
                    false,
                ),
            ),
        ]);
        let ss = vec![
            Source {
                id: "a".into(),
                owner: "o".into(),
                namespace: "n".into(),
                machine: "m".into(),
                floor: Sensitivity::Public,
                scan: ScanEvidence {
                    state: ScanState::Completed,
                    detector_version: "d".into(),
                    redactor_version: "r".into(),
                    secret_findings: 1,
                    pii_findings: 0,
                },
            },
            Source {
                id: "b".into(),
                owner: "o".into(),
                namespace: "n".into(),
                machine: "m".into(),
                floor: Sensitivity::Public,
                scan: ScanEvidence {
                    state: ScanState::Completed,
                    detector_version: "d".into(),
                    redactor_version: "r".into(),
                    secret_findings: 0,
                    pii_findings: 1,
                },
            },
        ];
        assert!(!provider_egress_allowed(&p, &ss, "p", "m", "step"));
        let ep = effective(&p["a"], "a").unwrap();
        let row = Map::from_iter([
            ("repo_key".into(), Value::String("r1".into())),
            (
                "display_name".into(),
                Value::String("sample repository".into()),
            ),
            (
                "repo_path".into(),
                Value::String("/home/example/project".into()),
            ),
        ]);
        assert_eq!(
            serialize_row("dim_repo", &row, ExportProfile::Receiver, &ep).unwrap(),
            include_str!("../tests/fixtures/governance-receiver-row.json").trim()
        );
        assert!(project_row("dim_repo", &row, ExportProfile::Hub, &ep).is_err());
        let row = Map::from_iter([
            ("repo_key".into(), Value::String("r1".into())),
            (
                "display_name".into(),
                Value::String("sample repository".into()),
            ),
            (
                "repo_path".into(),
                Value::String(format!("pt1_{}", "A".repeat(22))),
            ),
        ]);
        let serialized = serialize_row("dim_repo", &row, ExportProfile::Hub, &ep).unwrap();
        assert_eq!(
            serialized,
            include_str!("../tests/fixtures/governance-hub-row.json").trim()
        );
        assert!(serialized.contains("pt1_"));
        assert!(!serialized.contains("/home"));
        assert!(
            project_row(
                "dim_repo",
                &Map::from_iter([("body".into(), Value::String("secret".into()))]),
                ExportProfile::Receiver,
                &ep
            )
            .is_err()
        );
    }

    #[test]
    fn withheld_optional_is_distinct_from_null_and_rows_withhold_on_any_denial() {
        let mut p = effective(
            &bundle(
                &["repo_key", "display_name", "repo_path"],
                Sensitivity::Public,
                false,
                false,
            ),
            "s",
        )
        .unwrap();
        let partial = Map::from_iter([
            ("repo_key".into(), Value::String("r1".into())),
            ("display_name".into(), Value::String("repo".into())),
        ]);
        assert_eq!(
            project_row("dim_repo", &partial, ExportProfile::Receiver, &p).unwrap(),
            partial
        );
        let explicit_null = Map::from_iter([
            ("repo_key".into(), Value::String("r1".into())),
            ("display_name".into(), Value::String("repo".into())),
            ("repo_path".into(), Value::Null),
        ]);
        assert_eq!(
            project_row("dim_repo", &explicit_null, ExportProfile::Receiver, &p).unwrap()["repo_path"],
            Value::Null
        );
        p.allow_row_existence = false;
        assert_eq!(
            project_derived_row("dim_repo", &partial, ExportProfile::Receiver, &[p], &[]),
            Err(GovernanceError::Denied)
        );
    }
}

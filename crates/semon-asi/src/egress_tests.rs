use super::*;
use crate::{
    Store, TableBatch, Update,
    testutil::{provenance, replace_column, sample},
};
use std::sync::Arc;

const UID: &str = "origin/claude/private-log-stem";
fn policy() -> Policy {
    serde_json::from_value(serde_json::json!({
        "format":1,"sensitivity":"public","principals":["operator"],"groups":[],
        "scopes":[{"destination":"receiver","namespace":"origin","machine":"machine"}],
        "analytics_fields":schema::TABLES.iter().filter(|table| table.name != "fact_label").flat_map(|table| table.columns.iter().map(|column| column.name)).collect::<BTreeSet<_>>(),
        "allow_row_existence":true,"allow_coverage":true,"body_read":false,"provider_egress":[],"max_ttl_seconds":0,"max_bytes":0,"max_requests_per_minute":0
    })).unwrap()
}
fn registry() -> Registry {
    let registration = || Registration {
        source: Source {
            id: "ignored".into(),
            owner: "owner".into(),
            namespace: "origin".into(),
            machine: "machine".into(),
            floor: governance::Sensitivity::Public,
            scan: governance::ScanEvidence {
                state: governance::ScanState::Unknown,
                detector_version: "detector".into(),
                redactor_version: "redactor".into(),
                secret_findings: 0,
                pii_findings: 0,
            },
        },
        baseline: policy(),
        local: policy(),
    };
    Registry {
        format: 1,
        namespace: "origin".into(),
        sources: BTreeMap::from([
            ("native-a".into(), registration()),
            ("native-b".into(), registration()),
        ]),
    }
}
fn principal() -> Principal {
    Principal {
        id: "operator".into(),
        groups: BTreeSet::new(),
        namespace: "origin".into(),
        machine: "machine".into(),
    }
}
fn scope() -> Scope {
    Scope {
        destination: Destination::Receiver,
        namespace: "origin".into(),
        machine: "machine".into(),
    }
}
fn update(uid: &str) -> Update {
    let batch = sample(&schema::FACT_SESSION, &[(uid, 1)], "fixture", 0);
    let batch = replace_column(
        &batch,
        "price_version",
        Arc::new(StringArray::from(vec!["price1"])),
    );
    let batch = replace_column(&batch, "date_key", Arc::new(Int64Array::from(vec![None])));
    let batch = replace_column(
        &batch,
        "reported_usd",
        Arc::new(Float64Array::from(vec![None])),
    );
    let mut records = provenance(std::iter::once(uid));
    records.get_mut(uid).unwrap().source_witness = crate::input::SourceWitness {
        generation: "generation".into(),
        sources: ["native-a", "native-b"]
            .into_iter()
            .map(|id| crate::input::Source {
                native_id: id.into(),
                local_path: "/private/home/private-log-stem.jsonl".into(),
                consumed_offset: 42,
                prefix_sha256: "a".repeat(64),
            })
            .collect(),
    };
    Update {
        tables: vec![TableBatch {
            table: &schema::FACT_SESSION,
            batch,
        }],
        provenance: records,
        touched_sessions: vec![uid.into()],
        ..Update::default()
    }
}
fn setup() -> (tempfile::TempDir, Snapshot, Handles) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("analytics-namespace"), "origin\n").unwrap();
    let mut store = Store::open(root).unwrap();
    store.publish(&update(UID)).unwrap();
    drop(store);
    let snapshot = Snapshot::pin(root, &root.join("analytics-namespace"), "origin").unwrap();
    let handles = Handles::open(root, "origin").unwrap();
    (directory, snapshot, handles)
}
fn project(snapshot: &Snapshot, registry: &Registry, handles: &Handles) -> Projection {
    snapshot
        .project(Some(registry), handles, &principal(), &scope())
        .unwrap()
}
#[test]
fn complete_cohort_intersection_and_sanitized_bytes() {
    let (_directory, snapshot, handles) = setup();
    let mut registry = registry();
    let output = project(&snapshot, &registry, &handles);
    assert_eq!(output.outcomes.exported, 1);
    let bytes = output.serialize_receiver().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    for secret in [
        "private-log-stem",
        "/private/home",
        "native-a",
        "native-b",
        "generation",
    ] {
        assert!(!text.contains(secret));
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["sessions"][0]["policy_versions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        bytes,
        project(&snapshot, &registry, &handles)
            .serialize_receiver()
            .unwrap()
    );
    registry
        .sources
        .get_mut("native-b")
        .unwrap()
        .local
        .allow_row_existence = false;
    let denied = project(&snapshot, &registry, &handles);
    assert_eq!(denied.outcomes.withheld, 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&denied.serialize_receiver().unwrap()).unwrap()["sessions"],
        serde_json::json!([])
    );
    registry.sources.remove("native-b");
    assert_eq!(
        project(&snapshot, &registry, &handles)
            .outcomes
            .refused_provenance,
        1
    );
    let error = snapshot
        .project(None, &handles, &principal(), &scope())
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "invalid local egress registry");
}
#[test]
fn denied_nullable_is_omitted_required_is_withheld_and_null_zero_differ() {
    let (_directory, snapshot, handles) = setup();
    let mut registry = registry();
    let decode = |projection: Projection| {
        serde_json::from_slice::<Value>(&projection.serialize_receiver().unwrap()).unwrap()
    };
    let allowed = decode(project(&snapshot, &registry, &handles));
    let fields = &allowed["sessions"][0]["rows"][0]["fields"];
    assert_eq!(fields["reported_usd"], Value::Null);
    assert_eq!(fields["input_tokens"], 0);
    registry
        .sources
        .get_mut("native-b")
        .unwrap()
        .local
        .analytics_fields
        .remove("reported_usd");
    let omitted = decode(project(&snapshot, &registry, &handles));
    assert!(
        omitted["sessions"][0]["rows"][0]["fields"]
            .get("reported_usd")
            .is_none()
    );
    registry
        .sources
        .get_mut("native-b")
        .unwrap()
        .local
        .analytics_fields
        .remove("input_tokens");
    assert_eq!(project(&snapshot, &registry, &handles).outcomes.withheld, 1);
}
#[test]
fn coverage_is_separate_and_snapshot_is_pinned() {
    let (directory, snapshot, handles) = setup();
    let mut registry = registry();
    let original = project(&snapshot, &registry, &handles)
        .serialize_receiver()
        .unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    store.publish(&update("origin/claude/new-session")).unwrap();
    drop(store);
    assert_eq!(
        original,
        project(&snapshot, &registry, &handles)
            .serialize_receiver()
            .unwrap()
    );
    registry
        .sources
        .get_mut("native-b")
        .unwrap()
        .local
        .allow_coverage = false;
    assert_eq!(project(&snapshot, &registry, &handles).outcomes.withheld, 1);
    drop(handles);
    let reopened = Handles::open(directory.path(), "origin").unwrap();
    assert_eq!(
        original,
        project(&snapshot, &super::tests::registry(), &reopened)
            .serialize_receiver()
            .unwrap()
    );
}
#[test]
fn dependency_values_and_denied_lineage_never_escape() {
    let (_directory, mut snapshot, handles) = setup();
    let dimension: Map<String, Value> = schema::DIM_SESSION
        .columns
        .iter()
        .map(|column| {
            let value = match column.name {
                "session_uid" => Value::from(UID),
                "machine_key" => Value::from("machine"),
                "harness_key" => Value::from("claude"),
                "repo_key" => Value::from("unknown"),
                "parent_session_uid" => Value::from("origin/claude/denied-parent"),
                _ if column.nullable => Value::Null,
                _ => Value::from(column.values[0]),
            };
            (column.name.into(), value)
        })
        .collect();
    snapshot
        .rows
        .insert("dim_session".into(), vec![dimension.clone()]);
    snapshot.rows.insert(
        "dim_tool".into(),
        vec![
            serde_json::from_value(
                serde_json::json!({"tool_key":"denied-tool","display_name":"secret-unreferenced"}),
            )
            .unwrap(),
        ],
    );
    assert_eq!(
        project(&snapshot, &registry(), &handles).outcomes.withheld,
        1
    );
    snapshot.rows.remove("dim_session");
    let bytes = project(&snapshot, &registry(), &handles)
        .serialize_receiver()
        .unwrap();
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("secret-unreferenced")
    );
}
#[test]
fn credentials_and_unknown_metadata_fail_closed() {
    let (directory, mut snapshot, handles) = setup();
    snapshot.rows.get_mut("fact_session").unwrap()[0].insert(
        "price_version".into(),
        Value::from("sk-abcdefghijklmnopqrstuvwxyz0123456789"),
    );
    assert_eq!(
        project(&snapshot, &registry(), &handles).outcomes.withheld,
        1
    );
    let path = directory.path().join("unknown.parquet");
    let batch = sample(&schema::FACT_SESSION, &[(UID, 1)], "fixture", 0);
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("private".into(), "/private/home/private-log-stem".into());
    let schema = Arc::new(batch.schema().as_ref().clone().with_metadata(metadata));
    let batch = RecordBatch::try_new(schema, batch.columns().to_vec()).unwrap();
    crate::parquet_io::write_file(&path, &batch).unwrap();
    assert!(checked_batches(&path, &schema::FACT_SESSION.arrow_schema()).is_err());
    assert!(checked_batches(&path, &schema::live_map_schema()).is_err());
    for error in [
        Failure::Registry,
        Failure::Snapshot,
        Failure::Handles,
        Failure::Serialization,
    ] {
        assert!(!format!("{error:?} {error}").contains("private-log-stem"));
    }
}

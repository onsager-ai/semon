use super::*;
use semon_asi::{
    AdmittedBatch, AdmittedSession, AnalyticsInput, InputBudget, Reconciliation, SessionChange,
    Store, Versions,
    input::{
        Coverage, Derivation, ParentMethod, RecordedVersions, Repo, Role, Session, Status,
        UNKNOWN_REPO_KEY,
    },
    publish_cycle, schema,
};
use serde_json::json;
use std::{collections::BTreeSet, fs, path::PathBuf};

struct Fixture {
    state: PathBuf,
    index: PathBuf,
    binding: semon_sessions::analytics::Binding,
    registry: Registry,
    principal: Principal,
    handle: OpaqueId,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.state);
    }
}
struct Input(Option<AdmittedBatch>);
impl AnalyticsInput for Input {
    type Error = std::convert::Infallible;
    fn read_admitted(
        &mut self,
        _: Option<&str>,
        _: InputBudget,
    ) -> std::result::Result<AdmittedBatch, Self::Error> {
        Ok(self.0.take().unwrap())
    }
}
impl Fixture {
    fn new() -> Self {
        let state =
            std::env::temp_dir().join(format!("semon-resolver-{}", hex::encode(getrandom_bytes())));
        fs::create_dir(&state).unwrap();
        let index = state.join("sessions-index.sqlite3");
        let initialized = semon_sessions::analytics::initialize(&index, None, || false).unwrap();
        let binding = initialized.binding.clone();
        drop(initialized);
        let namespace = &binding.namespace;
        let scope = Scope {
            destination: Destination::Receiver,
            namespace: namespace.clone(),
            machine: "machine".into(),
        };
        let principal = Principal {
            id: "operator".into(),
            groups: BTreeSet::new(),
            namespace: namespace.clone(),
            machine: "machine".into(),
        };
        let policy = json!({
            "format":1,"sensitivity":"public","principals":["operator"],"groups":[],
            "scopes":[scope],"analytics_fields":schema::TABLES.iter().filter(|t| t.name != "fact_label").flat_map(|t| t.columns.iter().map(|c| c.name)).collect::<BTreeSet<_>>(),
            "allow_row_existence":true,"allow_coverage":true,"body_read":false,"provider_egress":[],
            "max_ttl_seconds":0,"max_bytes":0,"max_requests_per_minute":0
        });
        let registration = json!({"source":{"id":"ignored","owner":"owner","namespace":namespace,"machine":"machine","floor":"public",
            "scan":{"state":"unknown","detector_version":"detector","redactor_version":"redactor","secret_findings":0,"pii_findings":0}},"baseline":policy,"local":policy});
        let registry_path = state.join("registry.json");
        fs::write(&registry_path, serde_json::to_vec(&json!({"format":1,"namespace":namespace,"sources":{"native-a":registration,"native-b":registration}})).unwrap()).unwrap();
        let registry = Registry::load(&registry_path).unwrap();
        let uid = format!("{namespace}/codex/session");
        let admitted = AdmittedSession {
            session_uid: uid.clone(),
            source_witness: semon_asi::input::SourceWitness {
                generation: "source-generation".into(),
                sources: ["native-a", "native-b"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, id)| semon_asi::input::Source {
                        native_id: id.into(),
                        local_path: format!("/private/{id}"),
                        consumed_offset: 42 + i as u64,
                        prefix_sha256: format!("{i:064x}"),
                    })
                    .collect(),
            },
            derivation: Derivation::default(),
            versions: RecordedVersions {
                transform: 3,
                rules: 4,
                price: "price".into(),
                taxonomy: None,
                coverage: Coverage::default(),
            },
            session: Session {
                machine: "machine".into(),
                harness: "codex".into(),
                native_session_id: "session".into(),
                role: Role::Solo,
                parent_session_uid: None,
                parent_method: ParentMethod::None,
                lane_root_uid: None,
                repo: Repo {
                    key: UNKNOWN_REPO_KEY.into(),
                    repo_id: None,
                    local_path: None,
                    display_name: "unknown".into(),
                },
                branch: None,
                status: Status::Ended,
                started_at: None,
                ended_at: None,
                reported_usd: None,
            },
            steps: vec![],
            compactions: vec![],
            usage_complete: true,
            prices: vec![],
        };
        let mut store = Store::open(&state.join("analytics-store")).unwrap();
        store
            .bind_origin(namespace, &binding.origin_incarnation)
            .unwrap();
        let mut input = Input(Some(AdmittedBatch {
            namespace: namespace.clone(),
            checkpoint: "fixture".into(),
            complete: true,
            reconciliation: Reconciliation::Incremental,
            changes: vec![SessionChange::Admitted(Box::new(admitted))],
        }));
        publish_cycle(
            &mut store,
            &mut input,
            namespace,
            Versions::default(),
            InputBudget::default(),
        )
        .unwrap();
        drop(store);
        let snapshot = Snapshot::pin(
            &state.join("analytics-store"),
            &state.join("analytics-namespace"),
            namespace,
        )
        .unwrap();
        let handles = Handles::open(&state, namespace).unwrap();
        snapshot
            .project(Some(&registry), &handles, &principal, &scope)
            .unwrap();
        let handle = OpaqueId::new(&handles.existing("src", "native-a").unwrap().unwrap()).unwrap();
        Self {
            state,
            index,
            binding,
            registry,
            principal,
            handle,
        }
    }
    fn scope(&self) -> EnrollmentScope {
        EnrollmentScope {
            enrollment_id: opaque("enrollment"),
            machine: opaque("machine"),
            namespace: opaque(&self.binding.namespace),
            origin_incarnation: opaque(&self.binding.origin_incarnation),
            index_incarnation: opaque(&self.binding.index_incarnation),
            publisher_claim_epoch: Some(1),
            kinds: vec![crate::origin_read::EnrollmentKind::BodyChannel],
            verified_client_identity: opaque("client"),
        }
    }
    fn resolver<'a>(&'a self, generations: &'a dyn Generations) -> OriginHandleResolver<'a> {
        OriginHandleResolver::new(&self.index, &self.registry, &self.principal, generations)
    }
}
fn getrandom_bytes() -> [u8; 16] {
    let mut b = [0; 16];
    getrandom::fill(&mut b).unwrap();
    b
}
fn opaque(s: &str) -> OpaqueId {
    OpaqueId::new(s).unwrap()
}
struct Available;
impl Generations for Available {
    fn read(&self, cohorts: &[AdmittedCohort]) -> Result<CohortEvidence> {
        assert_eq!(cohorts.len(), 1);
        assert_eq!(cohorts[0].source_ids.len(), 2);
        Ok(CohortEvidence {
            activity_generation: opaque("activity"),
            transcript_generation: opaque("transcript"),
            chains: ["native-a", "native-b"]
                .into_iter()
                .map(|id| (id.into(), format!("chain-{id}")))
                .collect(),
        })
    }
}
#[test]
fn resolves_real_handles_and_maps_complete_pinned_cohort() {
    let f = Fixture::new();
    let source = f
        .resolver(&Available)
        .resolve(&f.scope(), &f.handle)
        .unwrap()
        .unwrap();
    assert_eq!(source.admitted_source, f.handle);
    assert_eq!(source.cohort.len(), 2);
    assert_eq!(source.activity_generation, opaque("activity"));
    assert_eq!(source.transcript_generation, opaque("transcript"));
    let handles = Handles::open(&f.state, &f.binding.namespace).unwrap();
    for (i, id) in ["native-a", "native-b"].into_iter().enumerate() {
        let expected = handles.existing("src", id).unwrap().unwrap();
        let member = source
            .cohort
            .iter()
            .find(|m| m.source_id == expected)
            .unwrap();
        assert_eq!(member.chain, format!("chain-{id}"));
        assert_eq!(member.prefix_sha256, format!("{i:064x}"));
        assert_eq!(member.generation, "source-generation");
        assert_eq!(member.consumed_boundary, 42 + i as u64);
    }
}
#[test]
fn misses_malformed_foreign_and_unadmitted_handles_are_indistinguishable() {
    let f = Fixture::new();
    let foreign = Fixture::new();
    let resolver = f.resolver(&Available);
    let mut handles = vec![
        opaque("src:bad"),
        opaque(&format!("src:{}", "a".repeat(65))),
        opaque(&format!("src:{}", "A".repeat(64))),
        opaque(&format!("ses:{}", "a".repeat(64))),
        opaque(&format!("src:{}", "f".repeat(64))),
        foreign.handle.clone(),
    ];
    // A genuine handle remains in the registry after its cohort is withheld.
    let mut denied = f.principal.clone();
    denied.id = "stranger".into();
    let outside = OriginHandleResolver::new(&f.index, &f.registry, &denied, &Available);
    assert!(outside.resolve(&f.scope(), &f.handle).unwrap().is_none());
    handles.push(opaque("src"));
    for handle in handles {
        assert!(resolver.resolve(&f.scope(), &handle).unwrap().is_none());
    }
    assert!(OpaqueId::new(&format!("src:{}", "a".repeat(129))).is_err());
    for field in 0..3 {
        let mut scope = f.scope();
        match field {
            0 => scope.namespace = opaque("foreign"),
            1 => scope.origin_incarnation = opaque("foreign"),
            _ => scope.index_incarnation = opaque("foreign"),
        };
        assert!(resolver.resolve(&scope, &f.handle).unwrap().is_none());
    }
}
#[test]
fn missing_generations_refuse_without_fabrication() {
    let f = Fixture::new();
    assert!(matches!(
        f.resolver(&UnavailableGenerations)
            .resolve(&f.scope(), &f.handle),
        Err(Refused::Unavailable)
    ));
}
#[test]
fn membership_is_stable_across_snapshot_changes() {
    let f = Fixture::new();
    let source = f
        .resolver(&Available)
        .resolve(&f.scope(), &f.handle)
        .unwrap()
        .unwrap();
    let mut newer = source.cohort.clone();
    for member in &mut newer {
        member.consumed_boundary += 100;
        member.prefix_sha256 = "f".repeat(64);
        member.generation = "new-generation".into();
        member.chain = "other-chain".into();
    }
    newer.reverse();
    assert_eq!(source.cohort_id, membership_id(&newer));
    newer.pop();
    assert_ne!(source.cohort_id, membership_id(&newer));
}

#[test]
fn missing_chain_evidence_refuses() {
    struct MissingChain;
    impl Generations for MissingChain {
        fn read(&self, cohorts: &[AdmittedCohort]) -> Result<CohortEvidence> {
            let mut evidence = Available.read(cohorts)?;
            evidence.chains.remove("native-b");
            Ok(evidence)
        }
    }
    let f = Fixture::new();
    assert!(matches!(
        f.resolver(&MissingChain).resolve(&f.scope(), &f.handle),
        Err(Refused::Unavailable)
    ));
}

#[test]
fn reading_binding_never_initializes_missing_state() {
    let path =
        std::env::temp_dir().join(format!("semon-binding-{}", hex::encode(getrandom_bytes())));
    assert!(
        semon_sessions::analytics::read_binding(&path.join("sessions-index.sqlite3"))
            .unwrap()
            .is_none()
    );
    assert!(!path.exists());
}

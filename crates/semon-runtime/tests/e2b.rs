use semon_runtime::{e2b::*, *};

fn prepared() -> (Session, OwnerId, OperationId) {
    let owner = OwnerId::new("owner-1").unwrap();
    let op = OperationId::new("create-1").unwrap();
    let mut session = Session::new(
        SessionId::new("session-1").unwrap(),
        Binding {
            owner: owner.clone(),
            workspace: WorkspaceId::new("workspace-1").unwrap(),
            cwd: "/home/user/workspace".into(),
            harness_home: "/home/user/.codex".into(),
            mirror: "managed-enrollment".into(),
            provider_credential: CredentialRef {
                owner: owner.clone(),
                name: CredentialId::new("provider-secret-reference").unwrap(),
            },
            model_credential: CredentialRef {
                owner: owner.clone(),
                name: CredentialId::new("model-secret-reference").unwrap(),
            },
        },
    )
    .unwrap();
    session
        .prepare_launch(&owner, 0, op.clone(), "private task payload".into())
        .unwrap();
    (session, owner, op)
}
fn labels(session: &Session, op: &OperationId) -> Labels {
    let LaunchPlan::ClaimCreate { labels, .. } =
        reconcile_launch("hub-prod", session, op, Inventory::Complete(&[])).unwrap()
    else {
        panic!("expected create claim")
    };
    labels
}
fn resource(labels: Labels, state: ResourceState) -> Resource {
    Resource {
        id: RuntimeId::new("sandbox-1").unwrap(),
        labels,
        state,
    }
}

#[test]
fn create_requires_a_persisted_claim_and_uncertain_outcomes_never_replay() {
    let (mut s, owner, op) = prepared();
    let initial = s.clone();
    let plan = reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&[])).unwrap();
    let LaunchPlan::ClaimCreate {
        revision,
        epoch,
        labels,
    } = plan
    else {
        panic!("expected claim")
    };
    assert_eq!((revision, epoch), (s.revision(), s.epoch()));
    let metadata = serde_json::to_string(&labels).unwrap();
    for private in [
        "private task payload",
        "provider-secret-reference",
        "model-secret-reference",
        "managed-enrollment",
        ".codex",
    ] {
        assert!(!metadata.contains(private));
    }
    s.operation_progress(&owner, epoch, &op, Progress::InFlight)
        .unwrap();
    initial.validate_successor(&s, &owner, epoch).unwrap();
    // A second controller cannot claim this persisted operation a second time.
    assert!(
        s.operation_progress(&owner, epoch, &op, Progress::InFlight)
            .is_err()
    );
    for progress in [Progress::InFlight, Progress::OutcomeUnknown] {
        if progress == Progress::OutcomeUnknown {
            s.operation_progress(&owner, epoch, &op, progress).unwrap();
        }
        let reloaded: Session = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(
            reconcile_launch("hub-prod", &reloaded, &op, Inventory::Complete(&[])),
            Ok(LaunchPlan::InspectAgain)
        );
    }
    // Eventual provider visibility lets the lost response be adopted, not retried.
    let resources = [resource(labels, ResourceState::Running)];
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&resources)),
        Ok(LaunchPlan::Adopt {
            id: resources[0].id.clone(),
            compute: Compute::Active
        })
    );
}

#[test]
fn incomplete_or_unreachable_inventory_cannot_establish_absence() {
    let (s, _, op) = prepared();
    for inventory in [Inventory::Incomplete, Inventory::Unavailable] {
        assert_eq!(
            reconcile_launch("hub-prod", &s, &op, inventory),
            Ok(LaunchPlan::InspectAgain)
        );
    }
    let resources = [resource(labels(&s, &op), ResourceState::Unknown)];
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&resources)),
        Ok(LaunchPlan::InspectAgain)
    );
}

#[test]
fn paused_resources_are_adopted_and_foreign_deployments_are_ignored() {
    let (s, _, op) = prepared();
    let owned = resource(labels(&s, &op), ResourceState::Paused);
    let mut foreign = owned.clone();
    foreign.id = RuntimeId::new("another-sandbox").unwrap();
    foreign
        .labels
        .insert("semon_deployment".into(), "other-hub".into());
    let resources = [foreign, owned.clone()];
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&resources)),
        Ok(LaunchPlan::Adopt {
            id: owned.id,
            compute: Compute::Paused
        })
    );
}

#[test]
fn duplicates_and_conflicting_ownership_refuse_adoption() {
    let (s, _, op) = prepared();
    let owned = resource(labels(&s, &op), ResourceState::Running);
    let mut second = owned.clone();
    second.id = RuntimeId::new("duplicate-sandbox").unwrap();
    assert_eq!(
        reconcile_launch(
            "hub-prod",
            &s,
            &op,
            Inventory::Complete(&[owned.clone(), second])
        ),
        Err(ReconcileError::DuplicateResources)
    );
    for key in [
        "semon_owner",
        "semon_workspace",
        "semon_epoch",
        "semon_version",
    ] {
        let mut wrong = owned.clone();
        wrong.labels.insert(key.into(), "wrong".into());
        assert_eq!(
            reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&[wrong])),
            Err(ReconcileError::OwnershipMismatch)
        );
    }
}

#[test]
fn bound_runtime_cannot_be_replaced_or_rebound_to_foreign_compute() {
    let (mut s, owner, op) = prepared();
    let owned = resource(labels(&s, &op), ResourceState::Running);
    s.bind_runtime(&owner, s.epoch(), owned.id.clone()).unwrap();
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&[])),
        Ok(LaunchPlan::InspectAgain)
    );
    let mut wrong = owned.clone();
    wrong.labels.clear();
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&[wrong])),
        Err(ReconcileError::OwnershipMismatch)
    );
    let mut duplicate = owned;
    duplicate.id = RuntimeId::new("unexpected-sandbox").unwrap();
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&[duplicate])),
        Err(ReconcileError::OwnershipMismatch)
    );
}

#[test]
fn stale_or_terminal_intent_is_rejected() {
    let (mut s, owner, op) = prepared();
    assert_eq!(
        reconcile_launch("../hub", &s, &op, Inventory::Complete(&[])),
        Err(ReconcileError::InvalidLaunch)
    );
    assert_eq!(
        reconcile_launch(
            "hub-prod",
            &s,
            &OperationId::new("not-persisted").unwrap(),
            Inventory::Complete(&[])
        ),
        Err(ReconcileError::InvalidLaunch)
    );
    s.set_desired(&owner, s.epoch(), Desired::Paused).unwrap();
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&[])),
        Err(ReconcileError::InvalidLaunch)
    );
    s.set_desired(&owner, s.epoch(), Desired::Active).unwrap();
    s.operation_progress(&owner, s.epoch(), &op, Progress::Failed)
        .unwrap();
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, Inventory::Complete(&[])),
        Err(ReconcileError::InvalidLaunch)
    );
}

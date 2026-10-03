use semon_runtime::*;

fn owner() -> OwnerId {
    OwnerId::new("owner-1").unwrap()
}
fn session() -> Session {
    Session::new(
        SessionId::new("logical-1").unwrap(),
        Binding {
            owner: owner(),
            workspace: WorkspaceId::new("workspace-1").unwrap(),
            cwd: "/workspace/project".into(),
            harness_home: "/home/user/.codex".into(),
            mirror: "stable-enrollment".into(),
            provider_credential: CredentialRef {
                owner: owner(),
                name: CredentialId::new("e2b").unwrap(),
            },
            model_credential: CredentialRef {
                owner: owner(),
                name: CredentialId::new("guest-device-auth").unwrap(),
            },
        },
    )
    .unwrap()
}

#[test]
fn reload_preserves_distinct_identities_and_prepared_manual_input() {
    let mut s = session();
    let op = OperationId::new("launch-1").unwrap();
    let old = s.clone();
    s.prepare_launch(&owner(), 0, op.clone(), "implement the task".into())
        .unwrap();
    old.validate_successor(&s, &owner(), 0).unwrap();
    s.bind_runtime(&owner(), 1, RuntimeId::new("e2b-sandbox-9").unwrap())
        .unwrap();
    s.bind_thread(&owner(), 1, ThreadId::new("codex-thread-2").unwrap())
        .unwrap();
    let bytes = serde_json::to_vec(&s).unwrap();
    let reloaded: Session = serde_json::from_slice(&bytes).unwrap();
    reloaded.validate().unwrap();
    assert_eq!(reloaded, s);
    assert_eq!(s.id().as_str(), "logical-1");
    assert_eq!(
        s.operations()[0].launch_input.as_deref(),
        Some("implement the task")
    );
    assert_eq!(s.operations()[0].progress, Progress::Prepared);
    assert_eq!(
        s.attempts()[0].runtime.as_ref().unwrap().as_str(),
        "e2b-sandbox-9"
    );
    assert_eq!(s.thread().unwrap().as_str(), "codex-thread-2");
}

#[test]
fn owner_epoch_and_writer_proof_are_required_for_replacement() {
    let mut s = session();
    s.prepare_launch(
        &owner(),
        0,
        OperationId::new("first").unwrap(),
        "task".into(),
    )
    .unwrap();
    let other = OwnerId::new("other").unwrap();
    assert_eq!(
        s.bind_thread(&other, 1, ThreadId::new("thread").unwrap()),
        Err(Error::WrongOwner)
    );
    assert_eq!(
        s.bind_thread(&owner(), 0, ThreadId::new("thread").unwrap()),
        Err(Error::StaleEpoch)
    );
    s.set_compute(&owner(), 1, Compute::Failed).unwrap();
    let new_op = OperationId::new("second").unwrap();
    assert_eq!(
        s.prepare_launch(&owner(), 1, new_op.clone(), "task".into()),
        Err(Error::WriterNotExcluded)
    );
    s.exclude_writer(&owner(), 1).unwrap();
    s.prepare_launch(&owner(), 1, new_op, "task".into())
        .unwrap();
    assert_eq!(s.epoch(), 2);
    assert_eq!(s.id().as_str(), "logical-1");
}

#[test]
fn uncertain_operations_cannot_be_replayed_and_destruction_stays_disabled() {
    let mut s = session();
    let op = OperationId::new("launch").unwrap();
    s.prepare_launch(&owner(), 0, op.clone(), "task".into())
        .unwrap();
    for progress in [Progress::InFlight, Progress::OutcomeUnknown] {
        s.operation_progress(&owner(), 1, &op, progress).unwrap();
    }
    assert_eq!(
        s.operation_progress(&owner(), 1, &op, Progress::InFlight),
        Err(Error::IllegalTransition)
    );
    s.operation_progress(&owner(), 1, &op, Progress::Succeeded)
        .unwrap();
    let caps = Capabilities {
        retained_filesystem: true,
        ..Default::default()
    };
    assert_eq!(
        caps.require(Action::SuspendMemory),
        Err(Error::Unsupported(Action::SuspendMemory))
    );
    assert_eq!(
        caps.require(Action::Destroy),
        Err(Error::NotQualified(Action::Destroy))
    );
}

#[test]
fn decoded_records_cannot_bypass_checked_mutations() {
    let old = session();
    for (field, value) in [
        ("compute", serde_json::json!("paused")),
        ("epoch", serde_json::json!(100)),
        ("input_revision", serde_json::json!(100)),
    ] {
        let mut raw = serde_json::to_value(&old).unwrap();
        raw[field] = value;
        raw["revision"] = 1.into();
        let forged: Session = serde_json::from_value(raw).unwrap();
        assert!(old.validate_successor(&forged, &owner(), 0).is_err());
    }
    let mut s = old.clone();
    s.set_harness(&owner(), 0, Harness::Idle).unwrap();
    old.validate_successor(&s, &owner(), 0).unwrap();
    assert_eq!(s.compute(), Compute::Absent);
    assert_eq!(s.desired(), Desired::Active);
    assert!(ThreadId::new("../auth.json").is_err());
}

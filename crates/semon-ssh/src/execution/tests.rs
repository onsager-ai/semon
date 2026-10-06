use super::*;
use semon_runtime::{
    Binding, CredentialId, CredentialRef, OperationId, RuntimeId, SessionId, ThreadId, WorkspaceId,
};
fn session(input: &str) -> Session {
    let owner = OwnerId::new("owner").unwrap();
    let mut s = Session::new(
        SessionId::new("synthetic-session").unwrap(),
        Binding {
            owner: owner.clone(),
            workspace: WorkspaceId::new("workspace").unwrap(),
            cwd: "/workspace/project".into(),
            harness_home: "/private/home".into(),
            mirror: "stable-enrollment".into(),
            provider_credential: CredentialRef {
                owner: owner.clone(),
                name: CredentialId::new("stable-ssh-connection").unwrap(),
            },
            model_credential: CredentialRef {
                owner: owner.clone(),
                name: CredentialId::new("separate-model-connection").unwrap(),
            },
        },
    )
    .unwrap();
    s.prepare_launch(
        &owner,
        0,
        OperationId::new("stable-operation").unwrap(),
        input.into(),
    )
    .unwrap();
    s.operation_progress(
        &owner,
        1,
        &OperationId::new("stable-operation").unwrap(),
        Progress::InFlight,
    )
    .unwrap();
    s.bind_runtime(&owner, 1, RuntimeId::new("stable-ssh-connection").unwrap())
        .unwrap();
    s.bind_thread(&owner, 1, ThreadId::new("synthetic-thread").unwrap())
        .unwrap();
    s
}
fn execution(input: &str, enrollment: &str) -> Execution {
    Execution::new(
        &session(input),
        &OwnerId::new("owner").unwrap(),
        enrollment,
        "11111111-1111-1111-1111-111111111111",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 120,
    )
    .unwrap()
}
#[test]
fn consent_identities_and_capabilities_are_separate() {
    let s = session("synthetic work");
    let e = execution("synthetic work", "00000000-0000-0000-0000-000000000001");
    assert_eq!(
        e.dispatch_id(),
        execution("synthetic work", "00000000-0000-0000-0000-000000000001").dispatch_id()
    );
    assert!(
        Execution::new(
            &s,
            &OwnerId::new("other-owner").unwrap(),
            "00000000-0000-0000-0000-000000000001",
            "11111111-1111-1111-1111-111111111111",
            u64::MAX
        )
        .is_err()
    );
    let mut coordinated = session("synthetic work");
    let mut receipt = Receipt {
        id: e.dispatch_id(),
        outcome: Outcome::Unknown,
        writer_excluded: false,
    };
    let before = coordinated.clone();
    e.record_dispatch(&mut coordinated, &receipt).unwrap();
    before
        .validate_successor(&coordinated, &OwnerId::new("owner").unwrap(), 1)
        .unwrap();
    assert_eq!(
        coordinated.operations()[0].progress,
        Progress::OutcomeUnknown
    );
    receipt.outcome = Outcome::Accepted;
    e.record_dispatch(&mut coordinated, &receipt).unwrap();
    assert_eq!(coordinated.operations()[0].progress, Progress::Succeeded);
    assert!(!coordinated.attempts()[0].writer_excluded);
    receipt.id = "77777777777777777777777777777777".into();
    assert!(e.record_dispatch(&mut coordinated, &receipt).is_err());
    assert_eq!(
        CAPABILITIES,
        Capabilities {
            dispatch: true,
            reconcile: true,
            credential_repair: true,
            revoke_execution: true,
            pause: false,
            snapshot: false,
            restore: false,
            replace: false,
            delete_compute: false
        }
    );
}
#[cfg(feature = "real-ssh-tests")]
async fn fixture(enrollment: &str) -> (Target, HostKey, Credential) {
    let target = Target {
        address: "127.0.0.1".parse().unwrap(),
        port: std::env::var("SEMON_SSH_PORT")
            .expect("isolated fixture required")
            .parse()
            .unwrap(),
        username: "fixture".into(),
    };
    let key = Credential(Zeroizing::new(
        std::fs::read_to_string(std::env::var("SEMON_SSH_KEY_FILE").unwrap()).unwrap(),
    ));
    let pin = crate::discover(&target).await.unwrap();
    let script = "import json,os,pathlib,subprocess,sys,time; os.umask(0o077); p=json.load(sys.stdin); d=pathlib.Path.home()/'.local/state/semon-ssh'/p['enrollment']; d.mkdir(mode=0o700,parents=True); f=d/'synthetic.py'; f.write_text(p['script']); subprocess.Popen(['python3',str(f),str(d)],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True); time.sleep(0.3)";
    let files = PrivateFiles::new(&pin, &key).unwrap();
    run(
        files.command(
            &target,
            &format!("python3 -c '{}'", script.replace('\'', "'\\''")),
        ),
        &serde_json::to_vec(
            &serde_json::json!({"enrollment":enrollment,"script":include_str!("synthetic.py")}),
        )
        .unwrap(),
        4096,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    (target, pin, key)
}
#[cfg(feature = "real-ssh-tests")]
async fn counts(
    target: &Target,
    pin: &HostKey,
    key: &Credential,
    enrollment: &str,
) -> serde_json::Value {
    let files = PrivateFiles::new(pin, key).unwrap();
    let script = "import json,pathlib,sys; p=json.load(sys.stdin); d=pathlib.Path.home()/'.local/state/semon-ssh'/p['enrollment']; print((d/'counts.json').read_text()); assert all('synthetic-model-secret' not in f.read_text() for f in (d/'execution').glob('*.json'))";
    let out = run(
        files.command(
            target,
            &format!("python3 -c '{}'", script.replace('\'', "'\\''")),
        ),
        &serde_json::to_vec(&serde_json::json!({"enrollment":enrollment})).unwrap(),
        4096,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    serde_json::from_slice(&out).unwrap()
}
#[cfg(feature = "real-ssh-tests")]
#[tokio::test]
async fn pinned_dispatch_receipts_lost_result_repair_revocation() {
    let enrollment = "00000000-0000-0000-0000-000000000001";
    let (target, pin, key) = fixture(enrollment).await;
    let e = execution("lost-reply", enrollment);
    let mut uncertain = session("lost-reply");
    uncertain
        .operation_progress(
            &OwnerId::new("owner").unwrap(),
            1,
            &OperationId::new("stable-operation").unwrap(),
            Progress::OutcomeUnknown,
        )
        .unwrap();
    let uncertain = Execution::new(
        &uncertain,
        &OwnerId::new("owner").unwrap(),
        enrollment,
        "11111111-1111-1111-1111-111111111111",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 120,
    )
    .unwrap();
    assert_eq!(
        uncertain
            .dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap_err(),
        Error::Execution
    );
    assert_eq!(
        e.dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap_err(),
        Error::Authority
    );
    e.authorize(&target, &pin, &key).await.unwrap();
    assert_eq!(
        e.dispatch(&target, &pin, &key, "stale-generation")
            .await
            .unwrap_err(),
        Error::Execution
    );
    let mut expired = execution("lost-reply", enrollment);
    expired.scope.expires = 1;
    assert_eq!(
        expired
            .dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap_err(),
        Error::Authority
    );
    assert_eq!(
        e.dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap()
            .outcome,
        Outcome::Unknown
    );
    assert_eq!(
        e.reconcile(&target, &pin, &key, &e.dispatch_id())
            .await
            .unwrap()
            .outcome,
        Outcome::Accepted
    );
    assert_eq!(
        e.dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap()
            .outcome,
        Outcome::Accepted
    );
    let changed_input = execution("changed work", enrollment);
    assert_eq!(
        changed_input
            .dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap_err(),
        Error::Execution
    );
    let mut foreign = execution("lost-reply", enrollment);
    foreign.scope.workspace = "foreign-workspace".into();
    assert_eq!(
        foreign
            .reconcile(&target, &pin, &key, &e.dispatch_id())
            .await
            .unwrap_err(),
        Error::Authority
    );
    let repair = "22222222222222222222222222222222";
    let model = ModelCredential(Zeroizing::new("synthetic-model-secret".into()));
    assert_eq!(
        e.repair(&target, &pin, &key, repair, 1, &model)
            .await
            .unwrap()
            .outcome,
        Outcome::Unknown
    );
    assert_eq!(
        e.repair(&target, &pin, &key, repair, 1, &model)
            .await
            .unwrap()
            .outcome,
        Outcome::Repaired
    );
    assert_eq!(
        counts(&target, &pin, &key, enrollment).await,
        serde_json::json!({"sends":1,"repairs":1})
    );
    let mut changed = pin.clone();
    changed.key.replace_range(40..44, "AAAA");
    assert_eq!(
        e.dispatch(&target, &changed, &key, "synthetic-generation")
            .await
            .unwrap_err(),
        Error::HostChanged
    );
    let revoked = e
        .revoke(&target, &pin, &key, "33333333333333333333333333333333")
        .await
        .unwrap();
    assert_eq!(revoked.outcome, Outcome::Revoked);
    assert!(revoked.writer_excluded);
    assert_eq!(
        e.authorize(&target, &pin, &key).await.unwrap_err(),
        Error::Authority
    );
    assert_eq!(
        e.dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap_err(),
        Error::Authority
    );
    assert_eq!(
        e.reconcile(&target, &pin, &key, "33333333333333333333333333333333")
            .await
            .unwrap(),
        revoked
    );
}
#[cfg(feature = "real-ssh-tests")]
#[tokio::test]
async fn cancellation_reconnect_and_writer_exclusion_never_replay() {
    let enrollment = "00000000-0000-0000-0000-000000000002";
    let (target, pin, key) = fixture(enrollment).await;
    let e = execution("slow-reply", enrollment);
    e.authorize(&target, &pin, &key).await.unwrap();
    let mut operation = Box::pin(e.dispatch(&target, &pin, &key, "synthetic-generation"));
    // Cancel only after the native synthetic writer has accepted the task.
    let inspect = async {
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(counts(&target, &pin, &key, enrollment).await["sends"], 1);
        assert_eq!(
            e.dispatch(&target, &pin, &key, "synthetic-generation")
                .await
                .unwrap_err(),
            Error::WriterBusy
        );
    };
    tokio::select! { result=&mut operation => panic!("unexpected early reply: {result:?}"), ()=inspect => {} }
    drop(operation);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        e.reconcile(&target, &pin, &key, &e.dispatch_id())
            .await
            .unwrap()
            .outcome,
        Outcome::Accepted
    );
    assert_eq!(
        e.dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap()
            .outcome,
        Outcome::Accepted
    );
    assert_eq!(counts(&target, &pin, &key, enrollment).await["sends"], 1);
    assert_eq!(
        e.observe(&target, &pin, &key).await.unwrap().generation,
        "synthetic-generation"
    );
    assert_eq!(
        e.reconnect(&target, &pin, &key, "66666666666666666666666666666666")
            .await
            .unwrap()
            .outcome,
        Outcome::Reconnected
    );
    assert_eq!(
        e.observe(&target, &pin, &key).await.unwrap().generation,
        "reconnected-generation"
    );
    e.revoke(&target, &pin, &key, "44444444444444444444444444444444")
        .await
        .unwrap();
}

#[cfg(feature = "real-ssh-tests")]
#[tokio::test]
async fn missing_native_evidence_is_unknown_and_never_replayed() {
    let enrollment = "00000000-0000-0000-0000-000000000003";
    let (target, pin, key) = fixture(enrollment).await;
    let e = execution("missing-result", enrollment);
    e.authorize(&target, &pin, &key).await.unwrap();
    assert_eq!(
        e.dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap()
            .outcome,
        Outcome::Unknown
    );
    assert_eq!(
        e.reconcile(&target, &pin, &key, &e.dispatch_id())
            .await
            .unwrap()
            .outcome,
        Outcome::Unknown
    );
    assert_eq!(
        e.dispatch(&target, &pin, &key, "synthetic-generation")
            .await
            .unwrap()
            .outcome,
        Outcome::Unknown
    );
    assert_eq!(counts(&target, &pin, &key, enrollment).await["sends"], 1);
    e.revoke(&target, &pin, &key, "55555555555555555555555555555555")
        .await
        .unwrap();
}

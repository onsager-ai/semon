#![cfg(feature = "e2b-process")]
use semon_runtime::{
    e2b::{process::*, *},
    *,
};
use std::{ffi::OsString, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "semon-e2b-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        std::fs::write(
            root.join("worker.py"),
            include_bytes!("fixtures/e2b-inventory.py"),
        )
        .unwrap();
        std::fs::write(root.join("mode"), mode).unwrap();
        Self { root }
    }
    async fn inspect(&self, duration: Duration) -> Result<OwnedInventory, ProcessError> {
        let python = ["/usr/bin/python3", "/usr/local/bin/python3"]
            .map(PathBuf::from)
            .into_iter()
            .find(|p| p.is_file())
            .unwrap();
        let worker = self.root.join("worker.py");
        let exclusions = [OsString::from("SEMON_TEST_COORDINATOR_SECRET")];
        let (session, operation) = session();
        InventoryWorker {
            python: &python,
            worker: &worker,
            lifetime: duration,
            environment_exclusions: &exclusions,
        }
        .inspect(
            "synthetic-provider-only-key",
            "hub-prod",
            &session,
            &operation,
        )
        .await
    }
    async fn assert_stopped(&self) {
        let pid = std::fs::read_to_string(self.root.join("pid")).unwrap();
        for _ in 0..100 {
            if !PathBuf::from(format!("/proc/{}", pid.trim())).exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("inventory fixture still running");
    }
    async fn validate(
        &self,
        key: &str,
        deployment: &str,
    ) -> Result<CredentialValidation, ProcessError> {
        let python = ["/usr/bin/python3", "/usr/local/bin/python3"]
            .map(PathBuf::from)
            .into_iter()
            .find(|p| p.is_file())
            .unwrap();
        let worker = self.root.join("worker.py");
        let exclusions = [OsString::from("SEMON_TEST_COORDINATOR_SECRET")];
        InventoryWorker {
            python: &python,
            worker: &worker,
            lifetime: Duration::from_secs(5),
            environment_exclusions: &exclusions,
        }
        .validate_credential(key, deployment, &OwnerId::new("owner-1").unwrap())
        .await
    }
}

#[tokio::test]
async fn basic_credential_validation_requires_no_fabricated_launch_and_returns_no_resources() {
    for (mode, expected) in [
        ("success", CredentialValidation::Accepted),
        ("rejected", CredentialValidation::Rejected),
        ("unavailable", CredentialValidation::Unavailable),
    ] {
        let f = Fixture::new(mode);
        assert_eq!(
            f.validate("synthetic-provider-only-key", "hub-prod").await,
            Ok(expected)
        );
        assert!(f.root.join("request-checked").is_file());
        f.assert_stopped().await;
    }
    for mode in [
        "validation_leak",
        "inventory_as_validation",
        "wrong_version",
    ] {
        let f = Fixture::new(mode);
        assert_eq!(
            f.validate("synthetic-provider-only-key", "hub-prod").await,
            Err(ProcessError::Protocol)
        );
        f.assert_stopped().await;
    }
    let f = Fixture::new("success");
    assert_eq!(
        f.validate("", "hub-prod").await,
        Err(ProcessError::Configuration)
    );
    assert_eq!(
        f.validate("synthetic-provider-only-key", "../hub").await,
        Err(ProcessError::Configuration)
    );
    assert!(!f.root.join("pid").exists());
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn session() -> (Session, OperationId) {
    let owner = OwnerId::new("owner-1").unwrap();
    let operation = OperationId::new("launch-1").unwrap();
    let credential = |name| CredentialRef {
        owner: owner.clone(),
        name: CredentialId::new(name).unwrap(),
    };
    let mut session = Session::new(
        SessionId::new("session-1").unwrap(),
        Binding {
            owner: owner.clone(),
            workspace: WorkspaceId::new("workspace-1").unwrap(),
            cwd: "/home/user/workspace".into(),
            harness_home: "/home/user/.codex".into(),
            mirror: "managed-enrollment".into(),
            provider_credential: credential("e2b"),
            model_credential: credential("codex"),
        },
    )
    .unwrap();
    session
        .prepare_launch(&owner, 0, operation.clone(), "private task".into())
        .unwrap();
    (session, operation)
}

#[tokio::test]
async fn private_request_and_paused_inventory_roundtrip_without_argument_credentials() {
    let f = Fixture::new("success");
    let inventory = f.inspect(Duration::from_secs(5)).await.unwrap();
    let (s, op) = session();
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, inventory.as_inventory()),
        Ok(LaunchPlan::Adopt {
            id: RuntimeId::new("sandbox-1").unwrap(),
            compute: Compute::Paused
        })
    );
    assert!(f.root.join("request-checked").is_file());
    f.assert_stopped().await;
}

#[tokio::test]
async fn partial_inventory_and_provider_failure_never_report_absence() {
    for mode in ["incomplete", "unavailable"] {
        let f = Fixture::new(mode);
        let inventory = f.inspect(Duration::from_secs(5)).await.unwrap();
        let (s, op) = session();
        assert_eq!(
            reconcile_launch("hub-prod", &s, &op, inventory.as_inventory()),
            Ok(LaunchPlan::InspectAgain)
        );
        f.assert_stopped().await;
    }
    let f = Fixture::new("foreign");
    let inventory = f.inspect(Duration::from_secs(5)).await.unwrap();
    let (s, op) = session();
    assert_eq!(
        reconcile_launch("hub-prod", &s, &op, inventory.as_inventory()),
        Err(ReconcileError::OwnershipMismatch)
    );
}

#[tokio::test]
async fn protocol_failures_are_sanitized_and_processes_reaped() {
    for mode in [
        "malformed",
        "oversize",
        "vendor_metadata",
        "bad_id",
        "wrong_state",
        "wrong_version",
        "no_resources",
        "mixed",
        "wrong_scope",
    ] {
        let f = Fixture::new(mode);
        let error = f.inspect(Duration::from_secs(5)).await.err().unwrap();
        assert_eq!(error, ProcessError::Protocol, "{mode}");
        assert!(!format!("{error:?}: {error}").contains("synthetic-vendor-secret"));
        f.assert_stopped().await;
    }
    let f = Fixture::new("nonzero");
    assert_eq!(
        f.inspect(Duration::from_secs(5)).await.err(),
        Some(ProcessError::Unavailable)
    );
    f.assert_stopped().await;
}

#[tokio::test]
async fn timeout_and_task_cancellation_kill_worker() {
    let f = Fixture::new("hang");
    assert_eq!(
        f.inspect(Duration::from_millis(100)).await.err(),
        Some(ProcessError::Timeout)
    );
    f.assert_stopped().await;
    let f = std::sync::Arc::new(Fixture::new("hang"));
    let run = {
        let f = f.clone();
        tokio::spawn(async move { f.inspect(Duration::from_secs(5)).await })
    };
    for _ in 0..100 {
        if f.root.join("pid").is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(f.root.join("pid").is_file());
    run.abort();
    assert!(matches!(run.await, Err(error) if error.is_cancelled()));
    f.assert_stopped().await;
}

#[tokio::test]
async fn invalid_configuration_or_withdrawn_launch_does_not_start_worker() {
    let f = Fixture::new("success");
    assert_eq!(
        f.inspect(Duration::from_secs(26)).await.err(),
        Some(ProcessError::Configuration)
    );
    assert!(!f.root.join("pid").exists());
    let (mut s, op) = session();
    s.set_desired(
        &OwnerId::new("owner-1").unwrap(),
        s.epoch(),
        Desired::Paused,
    )
    .unwrap();
    let worker = f.root.join("worker.py");
    let runtime = InventoryWorker {
        python: std::path::Path::new("/usr/bin/python3"),
        worker: &worker,
        lifetime: Duration::from_secs(5),
        environment_exclusions: &[],
    };
    assert_eq!(
        runtime
            .inspect("synthetic-provider-only-key", "hub-prod", &s, &op)
            .await
            .err(),
        Some(ProcessError::Configuration)
    );
    assert_eq!(
        runtime.inspect("", "hub-prod", &s, &op).await.err(),
        Some(ProcessError::Configuration)
    );
    assert!(!f.root.join("pid").exists());
}

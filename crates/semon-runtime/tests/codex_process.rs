#![cfg(feature = "codex-process")]
use semon_runtime::codex_auth::process::{DeviceLogin, ProcessError};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "semon-device-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("home")).unwrap();
        std::fs::write(
            root.join("codex"),
            include_bytes!("fixtures/codex-device.py"),
        )
        .unwrap();
        std::fs::set_permissions(root.join("codex"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::write(root.join("mode"), mode).unwrap();
        Self { root }
    }
    async fn start(&self, lifetime: Duration) -> Result<DeviceLogin, ProcessError> {
        DeviceLogin::start(
            &self.root.join("codex"),
            &self.root.join("home"),
            lifetime,
            &[
                "SEMON_TEST_COORDINATOR_SECRET",
                "DATABASE_URL",
                "PGPASSWORD",
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "E2B_API_KEY",
                "AWS_ACCESS_KEY_ID",
                "AWS_SECRET_ACCESS_KEY",
                "AWS_SESSION_TOKEN",
            ]
            .map(std::ffi::OsString::from),
        )
        .await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn early_and_unrelated_completions_require_matching_account_and_reaped_process() {
    let fixture = Fixture::new("success");
    let mut login = fixture.start(Duration::from_secs(5)).await.unwrap();
    assert_eq!(login.challenge().unwrap().user_code(), "FIXT-1234");
    login.wait().await.unwrap();
    assert!(fixture.root.join("home/auth.json").is_file());
    login.shutdown().await.unwrap(); // Cleanup must be idempotent.
}

#[tokio::test]
async fn malformed_frames_untrusted_url_and_vendor_secrets_fail_closed() {
    for (mode, expected) in [
        ("bad_url", ProcessError::Protocol),
        ("oversize", ProcessError::Protocol),
        ("rejected", ProcessError::Rejected),
        ("wrong_version", ProcessError::VersionMismatch),
    ] {
        let fixture = Fixture::new(mode);
        let error = fixture.start(Duration::from_secs(5)).await.err().unwrap();
        assert_eq!(error, expected);
        assert!(!error.to_string().contains("fixture-sensitive-error"));
    }
}

#[tokio::test]
async fn success_never_accepts_an_api_key_account() {
    let fixture = Fixture::new("account_api_key");
    let mut login = fixture.start(Duration::from_secs(5)).await.unwrap();
    assert_eq!(
        login.wait().await.err().unwrap(),
        ProcessError::AccountMismatch
    );
    login.shutdown().await.unwrap();
}

#[tokio::test]
async fn device_polling_has_a_fixed_deadline_and_can_be_cancelled() {
    let fixture = Fixture::new("pending");
    let mut login = fixture.start(Duration::from_millis(350)).await.unwrap();
    assert_eq!(login.wait().await.err().unwrap(), ProcessError::Timeout);
    login.shutdown().await.unwrap();
    let mut login = fixture.start(Duration::from_secs(5)).await.unwrap();
    login.shutdown().await.unwrap();
    assert!(!fixture.root.join("home/auth.json").exists());
}

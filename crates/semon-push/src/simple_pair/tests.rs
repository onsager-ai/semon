use super::*;
use crate::serve::{ConnectionAuthority, RegistryAuthenticator};
use std::sync::atomic::{AtomicU64, Ordering};
const MACHINE: &str = "machine-12345678abcdef01";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "semon-simple-pair-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        initialize(&dir).unwrap();
        Self(dir)
    }
    fn request(&self, code: &str, now: u64) -> Result<Response> {
        pair(
            &self.0,
            &serde_json::to_vec(&Request {
                version: 1,
                code: code.into(),
                machine_id: MACHINE.into(),
                hostname: "synthetic-host".into(),
            })
            .unwrap(),
            now,
        )
    }
    fn accepts(&self, token: &str) -> bool {
        RegistryAuthenticator::open(&self.0)
            .unwrap()
            .authenticated_enrollment(
                &mut ConnectionAuthority::default(),
                Some(&format!("Bearer {token}")),
                Kind::MirrorPush,
                100,
            )
            .is_some()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn issue_replace_expire_and_consume() {
    let f = Fixture::new();
    let (first, expiry) = issue(&f.0, 100).unwrap();
    assert_eq!(expiry, 700);
    let (second, _) = issue(&f.0, 101).unwrap();
    assert!(f.request(first.expose(), 102).is_err());
    let paired = f
        .request(
            &crate::pairing::display_code(&second)
                .expose()
                .to_lowercase(),
            103,
        )
        .unwrap();
    assert!(f.accepts(&paired.token));
    assert!(f.request(second.expose(), 104).is_err());
    let (third, _) = issue(&f.0, 105).unwrap();
    assert!(f.request(third.expose(), 705).is_err());
    assert!(f.request(third.expose(), 106).is_err());
}

#[test]
fn burns_and_pause_persist_across_store_reopen_and_clock_rollback() {
    let f = Fixture::new();
    for round in 0..3 {
        let (code, _) = issue(&f.0, 100 + round).unwrap();
        for _ in 0..3 {
            assert!(f.request("wrong", 100 + round).is_err());
        }
        assert!(f.request(code.expose(), 100 + round).is_err());
    }
    assert!(issue(&f.0, 103).is_err());
    assert!(issue(&f.0, 0).is_err());
    assert!(issue(&f.0, 1901).is_err());
    let (code, _) = issue(&f.0, 1902).unwrap();
    for _ in 0..3 {
        assert!(f.request("wrong", 1902).is_err());
    }
    assert!(f.request(code.expose(), 1902).is_err());
    assert!(issue(&f.0, 1903).is_err());
    assert_eq!(
        Registry::read_current(&f.0)
            .unwrap()
            .simple_pair
            .pause_until,
        3702
    );
}

#[test]
fn burns_outside_the_hour_do_not_pause_pairing() {
    let f = Fixture::new();
    for round in 0..3 {
        issue(&f.0, 100 + round * 3600).unwrap();
        for _ in 0..3 {
            assert!(f.request("wrong", 100 + round * 3600).is_err());
        }
    }
    assert!(issue(&f.0, 7301).is_ok());
}

#[test]
fn repair_and_both_unpair_paths_revoke_without_removing_logs() {
    let f = Fixture::new();
    let (code, _) = issue(&f.0, 100).unwrap();
    let first = f.request(code.expose(), 100).unwrap();
    let auth = RegistryAuthenticator::open(&f.0).unwrap();
    let handle = auth
        .authenticated_enrollment(
            &mut ConnectionAuthority::default(),
            Some(&format!("Bearer {}", first.token)),
            Kind::MirrorPush,
            100,
        )
        .unwrap()
        .authority;
    assert!(handle.is_current_at(100));
    assert!(!f.accepts("unknown"));
    let path = f.0.join(format!(
        "machines/{MACHINE}/claude/projects/synthetic.jsonl"
    ));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"sanitized retained source").unwrap();
    let (code, _) = issue(&f.0, 101).unwrap();
    let second = f.request(code.expose(), 101).unwrap();
    assert!(!f.accepts(&first.token));
    assert!(!handle.is_current_at(101));
    assert!(f.accepts(&second.token));
    assert!(
        Registry::read_current(&f.0)
            .unwrap()
            .simple_pair
            .audit
            .last()
            .unwrap()
            .replaced
    );
    unpair_token(&f.0, Some(&format!("Bearer {}", second.token)), 102).unwrap();
    assert!(!f.accepts(&second.token));
    let (code, _) = issue(&f.0, 103).unwrap();
    let third = f.request(code.expose(), 103).unwrap();
    unpair(&f.0, MACHINE, 104).unwrap();
    assert!(!f.accepts(&third.token));
    assert_eq!(std::fs::read(&path).unwrap(), b"sanitized retained source");
    let registry = Registry::read_current(&f.0).unwrap();
    assert_eq!(
        registry.machines()[0].owner_name.as_deref(),
        Some("synthetic-host")
    );
    assert!(
        !String::from_utf8(std::fs::read(f.0.join("enrollment/registry.json")).unwrap())
            .unwrap()
            .contains(&first.token)
    );
}

#[test]
fn request_is_strict_and_has_a_size_cap() {
    let f = Fixture::new();
    let (code, _) = issue(&f.0, 100).unwrap();
    let good = serde_json::to_value(Request {
        version: 1,
        code: code.expose().into(),
        machine_id: MACHINE.into(),
        hostname: "synthetic".into(),
    })
    .unwrap();
    for (field, value) in [
        ("extra", serde_json::json!(true)),
        ("version", serde_json::json!(2)),
        ("machine_id", serde_json::json!("../outside")),
        ("hostname", serde_json::json!("\n")),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(pair(&f.0, &serde_json::to_vec(&bad).unwrap(), 100).is_err());
    }
    assert!(pair(&f.0, &vec![b' '; 8193], 100).is_err());
    assert!(f.request(code.expose(), 100).is_ok());
}

#[test]
fn config_is_private_atomic_and_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let path = f.0.join("config/pair.json");
    let mut config = Config {
        url: "https://receiver.example".into(),
        token: "sanitized-first".into(),
        machine_id: MACHINE.into(),
    };
    config.save_at(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    config.token = "sanitized-second".into();
    config.save_at(&path).unwrap();
    assert_eq!(Config::load_at(&path).unwrap().token, "sanitized-second");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Config::load_at(&path).is_err());
    config.save_at(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn plain_http_is_only_accepted_on_loopback_and_redirects_are_disabled() {
    assert!(
        pair_machine("http://receiver.example", "invalid")
            .unwrap_err()
            .contains("loopback")
    );
    for url in [
        "http://127.0.0.1:8735",
        "http://localhost:8735",
        "http://[::1]:8735",
    ] {
        assert!(crate::check_url(url).is_ok());
    }
}

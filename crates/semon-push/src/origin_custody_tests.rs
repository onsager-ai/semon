use super::*;
use crate::{credentials::Kind, enrollment::Binding};
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "semon-origin-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn install(&self) -> String {
        self.install_sources(&[PathBuf::from("/native/approved.jsonl")])
            .unwrap()
    }
    fn install_sources(&self, sources: &[PathBuf]) -> Result<String> {
        let private = EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &ring::rand::SystemRandom::new(),
        )
        .unwrap();
        install(
            &self.0,
            "https://receiver.example:443",
            "laptop",
            &ActivationInput {
                receiver_id: "receiver".into(),
                machine: "laptop".into(),
                binding: Binding::Inapplicable,
                kinds: vec![(Kind::MirrorPush, false)],
                client_certificate_pins: vec![],
                valid_until: 1000,
            },
            &Activated {
                version: 1,
                enrollment_id: "enrollment".into(),
                credentials: vec![(Kind::MirrorPush, "secret-origin-bearer".into())],
            },
            private.as_ref(),
            sources,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn atomic_custody_integrity_permissions_and_secret_debug() {
    let f = Fixture::new();
    let name = f.install();
    let (config, credential) = load(&f.0, None).unwrap();
    assert!(!format!("{credential:?}").contains("secret-origin-bearer"));
    assert_eq!(
        consent(&f.0, &name, &config).unwrap(),
        [PathBuf::from("/native/approved.jsonl")]
    );
    assert_eq!(
        fs::metadata(&f.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for file in [
        "enrollment.json",
        "origin-key.pk8",
        "mirror-push.secret",
        "source-consent.json",
    ] {
        assert_eq!(
            fs::metadata(f.0.join(&name).join(file))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let path = f.0.join(&name).join("source-consent.json");
    let bytes = fs::read(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["sources"] = serde_json::json!(["/native/unapproved.jsonl"]);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(consent(&f.0, &name, &config).is_err());
    fs::write(&path, bytes).unwrap();
    let path = f.0.join(&name).join("enrollment.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["configuration"]["endpoint"] = "https://attacker.example:443".into();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load(&f.0, None).is_err());
}
#[test]
fn staged_credentials_are_never_a_half_installed_enrollment() {
    let f = Fixture::new();
    fs::DirBuilder::new().mode(0o700).create(&f.0).unwrap();
    let root = Directory::open(&f.0).unwrap();
    let stage = root.create_child("stage-interrupted").unwrap();
    stage
        .write("mirror-push.secret", b"interrupted-secret")
        .unwrap();
    assert!(load(&f.0, None).is_err());
    let name = f.install();
    load(&f.0, Some(&name)).unwrap();
    assert!(f.0.join("stage-interrupted/mirror-push.secret").exists());
}
#[test]
fn symlink_and_world_readable_custody_refuse() {
    let f = Fixture::new();
    let name = f.install();
    let secret = f.0.join(&name).join("mirror-push.secret");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load(&f.0, None).is_err());
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let moved = f.0.join(&name).join("moved.secret");
    fs::rename(&secret, &moved).unwrap();
    symlink(&moved, &secret).unwrap();
    assert!(load(&f.0, None).is_err());
}

#[test]
fn realistic_source_list_roundtrips_and_oversize_never_publishes() {
    let f = Fixture::new();
    let sources: Vec<_> = (0..5000)
        .map(|n| {
            PathBuf::from(format!(
                "/native/projects/a-realistic-project/sessions/rollout-{n:08}.jsonl"
            ))
        })
        .collect();
    let name = f.install_sources(&sources).unwrap();
    let (config, _) = load(&f.0, Some(&name)).unwrap();
    assert_eq!(consent(&f.0, &name, &config).unwrap(), sources);
    let oversized = Fixture::new();
    assert!(
        oversized
            .install_sources(&[PathBuf::from("x".repeat(FILE_CAP + 1))])
            .unwrap_err()
            .contains("4 MiB")
    );
    assert!(load(&oversized.0, None).is_err());
    assert!(
        !fs::read_dir(&oversized.0)
            .unwrap()
            .any(|entry| entry.unwrap().file_name().len() == 64)
    );
}
#[test]
fn install_explicitly_unlocks_even_with_duplicated_descriptor() {
    let f = Fixture::new();
    f.install();
    let duplicate = LOCK_DUPLICATE.with(|slot| slot.borrow_mut().take().unwrap());
    let root = Directory::open(&f.0).unwrap();
    let next = root
        .checked
        .open_file(Path::new("install.lock"), libc::O_RDWR)
        .unwrap();
    next.try_lock()
        .expect("install must unlock even while duplicate remains live");
    next.unlock().unwrap();
    drop(duplicate);
}
#[test]
fn config_roots_require_absolute_paths() {
    use std::ffi::OsStr;
    assert_eq!(
        configuration_directory(Some(OsStr::new("relative")), Some(OsStr::new("/home/test")))
            .unwrap(),
        PathBuf::from("/home/test/.config/semon/origin")
    );
    assert_eq!(
        configuration_directory(Some(OsStr::new("/config")), None).unwrap(),
        PathBuf::from("/config/semon/origin")
    );
    assert!(
        configuration_directory(
            Some(OsStr::new("relative")),
            Some(OsStr::new("relative-home"))
        )
        .is_err()
    );
    assert!(configuration_directory(None, None).is_err());
}

use super::*;
use crate::{credentials::Kind, enrollment::Binding};
use std::sync::atomic::{AtomicU64, Ordering};
const ENDPOINT: &str = "https://receiver.example:443";
const NOW: u64 = 1_800_000_000;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let f = Self(std::env::temp_dir().join(format!(
            "semon-origin-renewal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let private = EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &ring::rand::SystemRandom::new(),
        )
        .unwrap();
        install_at(
            &f.0,
            ENDPOINT,
            "laptop",
            &ActivationInput {
                receiver_id: "a".repeat(64),
                machine: "laptop".into(),
                binding: Binding::Inapplicable,
                kinds: vec![(Kind::MirrorPush, false)],
                client_certificate_pins: vec![],
                valid_until: NOW + wire::WINDOW - 1,
            },
            &Activated {
                version: 1,
                enrollment_id: "b".repeat(64),
                credentials: vec![(Kind::MirrorPush, "old-bearer".into())],
                client_certificate: None,
            },
            private.as_ref(),
            &[PathBuf::from("/native/approved")],
            NOW,
        )
        .unwrap();
        f
    }
    fn open(&self) -> Renewal {
        Renewal::open(&self.0, ENDPOINT).unwrap()
    }
    fn prepare(&self) -> Renewal {
        let mut r = self.open();
        r.prepare(
            Begun {
                version: 1,
                enrollment_id: "b".repeat(64),
                renewal_id: "c".repeat(64),
                receiver_nonce: crate::provision_wire::encode(&[7; 32]),
                expires_at: NOW + 600,
                generations: vec![Generation {
                    kind: Kind::MirrorPush,
                    generation: 2,
                }],
                enable_authorization_ids: vec![],
            },
            NOW,
        )
        .unwrap();
        r
    }
    fn delivery(&self, r: &Renewal) -> Delivered {
        let a = r.attempt.as_ref().unwrap();
        let application = crate::provision_wire::decode_fixed::<32>(&a.application_key).unwrap();
        Delivered {
            version: 1,
            enrollment_id: a.begun.enrollment_id.clone(),
            renewal_id: a.begun.renewal_id.clone(),
            credentials: vec![wire::DeliveredCredential {
                kind: Kind::MirrorPush,
                credential_id: "d".repeat(64),
                generation: 2,
                bearer: wire::bearer(
                    &application,
                    &a.begun.enrollment_id,
                    &a.begun.renewal_id,
                    &a.begun.generations[0],
                )
                .expose()
                .into(),
                valid_from: NOW,
                valid_until: NOW + 7_776_000,
            }],
            client_certificate: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn strict_thirty_day_boundary_and_expiry() {
    assert_eq!(window(NOW + wire::WINDOW + 1, NOW), (false, false));
    assert_eq!(window(NOW + wire::WINDOW, NOW), (true, false));
    assert_eq!(window(NOW + wire::WINDOW - 1, NOW), (true, true));
    assert_eq!(window(NOW, NOW), (true, false));
}
#[test]
fn prepared_commit_recovery_and_exhaustion_survive_restart() {
    let f = Fixture::new();
    let mut r = f.prepare();
    let first = r.finish(&[1; 32], NOW).unwrap();
    assert_eq!(first.action, Action::Commit);
    drop(r);
    assert!(load_at(&f.0, None, NOW).is_err());
    let mut r = f.open();
    let second = r.finish(&[2; 32], NOW).unwrap();
    assert_eq!(second.action, Action::Recover);
    assert_eq!(first.application_key, second.application_key);
    assert_ne!(first.proof, second.proof);
    drop(r);
    assert!(f.open().finish(&[3; 32], NOW).is_err());
}
#[test]
fn delivered_generation_is_atomic_retained_and_never_falls_back() {
    let f = Fixture::new();
    let (old, credential) = load_at(&f.0, None, NOW).unwrap();
    let crate::Credential::Enrolled { authority, .. } = credential else {
        panic!()
    };
    let mut r = f.prepare();
    r.finish(&[1; 32], NOW).unwrap();
    let delivered = f.delivery(&r);
    r.delivered(&delivered, NOW).unwrap();
    assert!(!authority.is_active());
    assert!(r.credentials(NOW).is_err());
    drop(r);
    let (config, _) = load_at(&f.0, None, NOW).unwrap();
    assert_eq!(config.generation, 2);
    assert_eq!(config.consent_sha256, old.consent_sha256);
    assert!(
        f.0.join(custody_name(ENDPOINT))
            .join("generation-1/mirror-push.secret")
            .exists()
    );
    let mut r = f.open();
    r.confirm(2).unwrap();
    assert_eq!(r.credentials(NOW).unwrap()[0].generation, 2);
    drop(r);
    assert!(load_at(&f.0, None, delivered.credentials[0].valid_until).is_err());
    let dir =
        f.0.join(custody_name(ENDPOINT))
            .join("generation-2/mirror-push.secret");
    fs::remove_file(dir).unwrap();
    assert!(load_at(&f.0, None, NOW).is_err());
}
#[test]
fn crash_points_never_mix_files_and_restart_reconciles_complete_generation() {
    for (point, count) in [
        ("before-stage-file", 0),
        ("after-stage-file", 0),
        ("after-stage-file", 1),
        ("after-stage-file", 2),
        ("after-stage-file", 3),
        ("stage-directory-fsync", 0),
        ("generation-rename", 0),
        ("generation-directory-fsync", 0),
        ("file-fsync", 0),
        ("before-rename", 0),
        ("after-rename", 0),
        ("directory-fsync", 0),
        ("file-fsync", 1),
        ("before-rename", 1),
        ("after-rename", 1),
        ("directory-fsync", 1),
        ("file-fsync", 2),
        ("before-rename", 2),
        ("after-rename", 2),
        ("directory-fsync", 2),
    ] {
        let f = Fixture::new();
        let mut r = f.prepare();
        r.finish(&[1; 32], NOW).unwrap();
        let delivered = f.delivery(&r);
        FAULT.with(|fault| *fault.borrow_mut() = Some((point, count)));
        assert!(r.delivered(&delivered, NOW).is_err(), "{point}/{count}");
        FAULT.with(|fault| *fault.borrow_mut() = None);
        drop(r);
        let mut r = f.open();
        if matches!(r.attempt.as_ref().unwrap().state, State::CommitReserved) {
            // Incomplete retained stage never selects; only a verified recovery may.
            assert!(load_at(&f.0, None, NOW).is_err());
            assert_eq!(r.finish(&[2; 32], NOW).unwrap().action, Action::Recover);
            r.delivered(&delivered, NOW).unwrap();
        }
        drop(r);
        let (config, _) = load_at(&f.0, None, NOW).unwrap();
        assert_eq!(config.generation, 2, "{point}/{count}");
    }
}
#[test]
fn disconnect_wins_before_concurrent_renewal_publication() {
    let f = Fixture::new();
    drop(f.prepare());
    let disconnect = Disconnect::open(&f.0, ENDPOINT).unwrap();
    let base = f.0.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tx.send(Renewal::open(&base, ENDPOINT).is_err()).unwrap();
    });
    assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
    worker.join().unwrap();
    disconnect.commit().unwrap();
    assert!(Renewal::open(&f.0, ENDPOINT).is_err());
    assert!(load_at(&f.0, None, NOW).is_err());
}
#[test]
fn invalid_delivery_and_expired_authentication_never_publish() {
    let f = Fixture::new();
    let mut r = f.prepare();
    r.finish(&[1; 32], NOW).unwrap();
    let mut delivered = f.delivery(&r);
    delivered.credentials[0].bearer = "substitution".into();
    assert!(r.delivered(&delivered, NOW).is_err());
    assert!(r.finish(&[2; 32], NOW + wire::WINDOW).is_err());
    assert_eq!(
        std::str::from_utf8(&r.dir.read("selected").unwrap()).unwrap(),
        "generation-1"
    );
}

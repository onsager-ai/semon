use super::*;
use crate::{
    credentials::Kind,
    enrollment::{Binding, Enrollment, KindScope},
};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair},
};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    dir: std::path::PathBuf,
    key: EcdsaKeyPair,
    secret: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "semon-renewal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(dir.join("receiver-audit"))
            .unwrap();
        let rng = SystemRandom::new();
        let private =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .unwrap();
        let key = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            private.as_ref(),
            &rng,
        )
        .unwrap();
        let secret = "old-private-credential".to_owned();
        let enrollment = Enrollment {
            enrollment_id: "a".repeat(64),
            receiver_id: "receiver".into(),
            machine: "origin".into(),
            sequence: 1,
            origin_key: key.public_key().as_ref().to_vec(),
            endpoint: "https://receiver.example:443".into(),
            enrolled_pin: None,
            binding: Binding::Inapplicable,
            client_certificate_pins: vec![],
            credential_kinds: vec![KindScope::new(Kind::MirrorPush, 1)],
            revoked_at: None,
        };
        let bearer = Bearer {
            credential_id: "b".repeat(64),
            enrollment_id: enrollment.enrollment_id.clone(),
            kind: Kind::MirrorPush,
            sha256: Secret::new(secret.clone()).hash(),
            generation: 1,
            valid_from: 0,
            valid_until: 4_000_000,
            revoked_at: None,
        };
        let registry = Registry::from_records(vec![enrollment], vec![bearer]).unwrap();
        let mut store = Store::initialize(&dir).unwrap();
        store.commit(0, &registry).unwrap();
        Self { dir, key, secret }
    }
    fn credentials(&self) -> Vec<Credential> {
        vec![Credential {
            kind: Kind::MirrorPush,
            generation: 1,
            bearer: self.secret.clone(),
        }]
    }
    fn begin(&self, receiver: &mut Receiver, now: u64) -> Result<Begun> {
        receiver.begin(
            &self.dir,
            &wire::Begin {
                version: 1,
                enrollment_id: "a".repeat(64),
                credentials: self.credentials(),
            },
            None,
            now,
        )
    }
    fn finish(
        &self,
        begun: &Begun,
        action: Action,
        application: [u8; 32],
        exporter: [u8; 32],
    ) -> wire::Finish {
        let authentication = vec![Generation {
            kind: Kind::MirrorPush,
            generation: 1,
        }];
        let signed = wire::possession_message(
            "receiver",
            "https://receiver.example:443",
            begun,
            &authentication,
            &exporter,
            action,
            &application,
        )
        .unwrap();
        let proof = self.key.sign(&SystemRandom::new(), &signed).unwrap();
        wire::Finish {
            version: 1,
            enrollment_id: begun.enrollment_id.clone(),
            renewal_id: begun.renewal_id.clone(),
            action,
            credentials: self.credentials(),
            application_key: provision_wire::encode(&application),
            proof: provision_wire::encode(proof.as_ref()),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
#[test]
fn exact_window_idempotent_begin_and_restart_rate_limit() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    assert!(f.begin(&mut receiver, 4_000_000 - wire::WINDOW).is_err());
    let now = 4_000_000 - wire::WINDOW + 1;
    let begun = f.begin(&mut receiver, now).unwrap();
    assert_eq!(begun, f.begin(&mut receiver, now + 1).unwrap());
    for index in 1..6 {
        receiver = Receiver::default();
        assert!(f.begin(&mut receiver, now + index).is_ok());
    }
    receiver = Receiver::default();
    assert!(f.begin(&mut receiver, now + 6).is_err());
    assert!(f.begin(&mut receiver, now + 3600).is_ok());
}
#[test]
fn lost_ack_recovers_identical_delivery_once_after_restart() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    let initial = receiver
        .finish(&f.dir, &commit, &[4; 32], None, now + 1)
        .unwrap();
    assert!(
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, now + 1)
            .is_err()
    );
    receiver = Receiver::default();
    assert_eq!(begun, f.begin(&mut receiver, now + 2).unwrap());
    let wrong_application = f.finish(&begun, Action::Recover, [5; 32], [6; 32]);
    assert!(
        receiver
            .finish(&f.dir, &wrong_application, &[6; 32], None, now + 2)
            .is_err()
    );
    let recover = f.finish(&begun, Action::Recover, [3; 32], [6; 32]);
    assert!(
        receiver
            .finish(&f.dir, &recover, &[7; 32], None, now + 2)
            .is_err()
    );
    assert_eq!(
        initial,
        receiver
            .finish(&f.dir, &recover, &[6; 32], None, now + 2)
            .unwrap()
    );
    assert!(
        receiver
            .finish(&f.dir, &recover, &[6; 32], None, now + 2)
            .is_err()
    );
    let registry = Registry::read_current(&f.dir).unwrap();
    assert_eq!(
        registry.enrollments[0].credential_kinds[0]
            .generations
            .current,
        2
    );
    assert_eq!(registry.credentials.len(), 2);
    assert_eq!(registry.renewal_receipts[0].deliveries, 2);
}
#[test]
fn exact_pending_deadline_cancel_restart_and_bad_bearer() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let begun = f.begin(&mut receiver, 2_000_000).unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    assert!(
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, begun.expires_at)
            .is_err()
    );
    let begun = f.begin(&mut receiver, 2_001_000).unwrap();
    let cancel = f.finish(&begun, Action::Cancel, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &cancel, &[4; 32], None, 2_001_001)
        .unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    assert!(
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, 2_001_001)
            .is_err()
    );
    let begun = f.begin(&mut receiver, 2_001_002).unwrap();
    receiver = Receiver::default();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    assert!(
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, 2_001_003)
            .is_err()
    );
    let mut bad = wire::Begin {
        version: 1,
        enrollment_id: "a".repeat(64),
        credentials: f.credentials(),
    };
    bad.credentials[0].bearer = "stolen-key-without-bearer".into();
    assert!(receiver.begin(&f.dir, &bad, None, 2_001_003).is_err());
}
#[test]
fn first_use_ends_recovery_and_preserves_new_authority() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
    let new_secret = wire::bearer(
        &[3; 32],
        &begun.enrollment_id,
        &begun.renewal_id,
        &begun.generations[0],
    );
    Store::authorize_cutover(&f.dir, Kind::MirrorPush, &new_secret, None, now + 1).unwrap();
    let recover = f.finish(&begun, Action::Recover, [3; 32], [4; 32]);
    assert!(
        receiver
            .finish(&f.dir, &recover, &[4; 32], None, now + 1)
            .is_err()
    );
    Store::open(&f.dir)
        .unwrap()
        .compact(&f.dir, now + 1)
        .unwrap();
    let registry = Registry::read_current(&f.dir).unwrap();
    assert!(registry.renewal_receipts.is_empty());
    assert_eq!(registry.credentials.len(), 1);
    assert!(matches!(
        registry
            .authorize(
                Kind::MirrorPush,
                Kind::MirrorPush,
                &new_secret,
                None,
                now + 1
            )
            .decision,
        Decision::Allowed { generation: 2, .. }
    ));
}

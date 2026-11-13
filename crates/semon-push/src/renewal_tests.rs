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
        Self::with_custody(false)
    }
    fn with_custody(certificate: bool) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "semon-renewal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        if certificate {
            crate::receiver_custody::ReceiverCustody::open_at_endpoint(
                &dir,
                100,
                "https://receiver.example:443",
            )
            .unwrap();
        } else {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(dir.join("receiver-audit"))
                .unwrap();
        }
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
        store
            .commit(store.snapshot().unwrap().revision(), &registry)
            .unwrap();
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
        self.finish_using(begun, action, application, exporter, self.credentials())
    }
    fn finish_using(
        &self,
        begun: &Begun,
        action: Action,
        application: [u8; 32],
        exporter: [u8; 32],
        credentials: Vec<Credential>,
    ) -> wire::Finish {
        let authentication = wire::identities(&credentials).unwrap();
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
            credentials,
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
fn enable_is_bound_at_begin_consumed_once_and_recovery_uses_old_set() {
    enabled_renewal(false);
    enabled_renewal(true);
    enable_after_begin_stays_pending();
}
fn enabled_renewal(cancel_bound: bool) {
    let f = Fixture::new();
    let store = Store::open(&f.dir).unwrap();
    let mut registry = store.snapshot().unwrap();
    let e = &mut registry.enrollments[0];
    e.binding = Binding::Admitted {
        namespace: "namespace".into(),
        origin_incarnation: "origin".into(),
        index_incarnation: "index".into(),
        publisher_claim_epoch: crate::enrollment::ClaimEpoch::Unassigned,
        issuer: crate::enrollment::Issuer {
            kind: crate::enrollment::IssuerKind::Receiver,
            origin_trust_acknowledged: true,
        },
        assertion_issuer_keys: vec![crate::enrollment::IssuerKey {
            key_id: "issuer".into(),
            public_key: [1; 32],
        }],
    };
    let mut enabled = KindScope::new(Kind::AnalyticsPublish, 7);
    enabled.available = false;
    enabled.enable_authorization = Some(crate::enrollment::EnableAuthorization {
        enrollment_id: e.enrollment_id.clone(),
        kind: Kind::AnalyticsPublish,
        owner: "owner".into(),
        authorization_id: "c".repeat(64),
        created_at: 100,
        consumed_renewal_id: None,
    });
    e.credential_kinds.push(enabled);
    // Build this fixture as initial custody; ordinary APIs cannot widen bindings.
    let initial =
        Registry::from_records(registry.enrollments.clone(), registry.credentials.clone()).unwrap();
    drop(store);
    let path = f.dir.join("enrollment/registry.json");
    fs::write(path, serde_json::to_vec(&initial).unwrap()).unwrap();
    let mut receiver = Receiver::default();
    let begun = f.begin(&mut receiver, 100).unwrap();
    assert_eq!(begun.enable_authorization_ids, vec!["c".repeat(64)]);
    assert_eq!(begun.generations.len(), 2);
    if cancel_bound {
        Store::open(&f.dir)
            .unwrap()
            .owner_update(
                &f.dir,
                Some(Kind::AnalyticsPublish),
                crate::enrollment::OwnerAction::Revoke,
                "owner",
                &begun.enrollment_id,
                101,
            )
            .unwrap();
        let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
        assert!(
            receiver
                .finish(&f.dir, &commit, &[4; 32], None, 101)
                .is_err()
        );
        let registry = Registry::read_current(&f.dir).unwrap();
        assert!(registry.renewal_receipts.is_empty());
        assert!(
            !registry.enrollments[0]
                .credential_kinds
                .iter()
                .find(|s| s.kind == Kind::AnalyticsPublish)
                .unwrap()
                .available
        );
        return;
    }
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    let initial = receiver
        .finish(&f.dir, &commit, &[4; 32], None, 101)
        .unwrap();
    let registry = Registry::read_current(&f.dir).unwrap();
    let scope = registry.enrollments[0]
        .credential_kinds
        .iter()
        .find(|s| s.kind == Kind::AnalyticsPublish)
        .unwrap();
    assert!(scope.available);
    assert!(scope.generations.retiring.is_none());
    assert_eq!(scope.generations.current, 8);
    assert_eq!(
        scope
            .enable_authorization
            .as_ref()
            .unwrap()
            .consumed_renewal_id
            .as_ref(),
        Some(&begun.renewal_id)
    );
    assert_eq!(
        registry.renewal_receipts[0].recovery_until,
        101 + wire::OVERLAP
    );
    assert_eq!(registry.renewal_receipts[0].authentication.len(), 1);
    let recover = f.finish(&begun, Action::Recover, [3; 32], [5; 32]);
    assert_eq!(
        initial,
        receiver
            .finish(&f.dir, &recover, &[5; 32], None, 102)
            .unwrap()
    );
}
fn enable_after_begin_stays_pending() {
    let f = Fixture::new();
    let original = Registry::read_current(&f.dir).unwrap();
    let mut enrollments = original.enrollments.clone();
    enrollments[0].binding = Binding::Admitted {
        namespace: "namespace".into(),
        origin_incarnation: "origin".into(),
        index_incarnation: "index".into(),
        publisher_claim_epoch: crate::enrollment::ClaimEpoch::Unassigned,
        issuer: crate::enrollment::Issuer {
            kind: crate::enrollment::IssuerKind::Receiver,
            origin_trust_acknowledged: true,
        },
        assertion_issuer_keys: vec![crate::enrollment::IssuerKey {
            key_id: "issuer".into(),
            public_key: [1; 32],
        }],
    };
    let mut scope = KindScope::new(Kind::AnalyticsPublish, 1);
    scope.available = false;
    enrollments[0].credential_kinds.push(scope);
    let registry = Registry::from_records(enrollments, original.credentials.clone()).unwrap();
    fs::write(
        f.dir.join("enrollment/registry.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
    let mut receiver = Receiver::default();
    let begun = f.begin(&mut receiver, 2_000_000).unwrap();
    Store::open(&f.dir)
        .unwrap()
        .owner_update(
            &f.dir,
            Some(Kind::AnalyticsPublish),
            crate::enrollment::OwnerAction::Enable,
            "owner",
            &begun.enrollment_id,
            2_000_001,
        )
        .unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, 2_000_002)
        .unwrap();
    let registry = Registry::read_current(&f.dir).unwrap();
    let scope = registry.enrollments[0]
        .credential_kinds
        .iter()
        .find(|s| s.kind == Kind::AnalyticsPublish)
        .unwrap();
    assert!(!scope.available);
    assert!(
        scope
            .enable_authorization
            .as_ref()
            .unwrap()
            .consumed_renewal_id
            .is_none()
    );
    assert_eq!(registry.renewal_receipts[0].credentials.len(), 1);
}
#[test]
fn pending_capacity_expires_before_admission_and_never_evicts_live_attempts() {
    let f = Fixture::new();
    let store = Store::open(&f.dir).unwrap();
    let original = store.snapshot().unwrap();
    drop(store);
    let mut enrollments = Vec::new();
    let mut credentials = Vec::new();
    for n in 1..=5 {
        let mut e = original.enrollments[0].clone();
        e.enrollment_id = format!("{n:064x}");
        e.machine = format!("origin-{n}");
        e.sequence = n;
        let mut b = original.credentials[0].clone();
        b.enrollment_id = e.enrollment_id.clone();
        b.credential_id = format!("{:064x}", n + 10);
        b.sha256 = Secret::new(format!("secret-{n}")).hash();
        enrollments.push(e);
        credentials.push(b);
    }
    let registry = Registry::from_records(enrollments, credentials).unwrap();
    fs::write(
        f.dir.join("enrollment/registry.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
    let mut receiver = Receiver::default();
    let request = |n| wire::Begin {
        version: 1,
        enrollment_id: format!("{n:064x}"),
        credentials: vec![Credential {
            kind: Kind::MirrorPush,
            generation: 1,
            bearer: format!("secret-{n}"),
        }],
    };
    let mut first = None;
    for n in 1..=4 {
        let begun = receiver
            .begin(&f.dir, &request(n), None, 2_000_000)
            .unwrap();
        if n == 1 {
            first = Some(begun);
        }
    }
    assert!(
        receiver
            .begin(&f.dir, &request(5), None, 2_000_001)
            .is_err()
    );
    assert_eq!(
        Some(
            receiver
                .begin(&f.dir, &request(1), None, 2_000_001)
                .unwrap()
        ),
        first
    );
    assert!(receiver.begin(&f.dir, &request(5), None, 2_000_600).is_ok());
    assert_eq!(receiver.pending.len(), 1);
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
fn delivery_failures_preserve_committed_reservations_without_exposing_secrets() {
    for failure in 1..=3 {
        let f = Fixture::new();
        let mut receiver = Receiver::default();
        let now = 2_000_000;
        let begun = f.begin(&mut receiver, now).unwrap();
        let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
        receiver.fail_delivery = failure;
        let result = receiver.finish(&f.dir, &commit, &[4; 32], None, now);
        if failure == 1 {
            assert!(result.is_err());
            assert!(
                Registry::read_current(&f.dir)
                    .unwrap()
                    .renewal_receipts
                    .is_empty()
            );
            receiver
                .finish(&f.dir, &commit, &[4; 32], None, now)
                .unwrap();
        } else if failure == 2 {
            assert!(result.is_err());
            assert_eq!(
                Registry::read_current(&f.dir).unwrap().renewal_receipts[0].deliveries,
                1
            );
            receiver = Receiver::default();
        } else {
            assert!(result.is_ok());
        }
        let recover = f.finish(&begun, Action::Recover, [3; 32], [5; 32]);
        let recovered = receiver.finish(&f.dir, &recover, &[5; 32], None, now + 1);
        assert_eq!(recovered.is_err(), failure == 3);
        assert_eq!(
            Registry::read_current(&f.dir).unwrap().renewal_receipts[0].deliveries,
            2
        );
        receiver = Receiver::default();
        assert!(
            receiver
                .finish(&f.dir, &recover, &[5; 32], None, now + 2)
                .is_err()
        );
    }
}
#[test]
fn registry_replace_failures_preserve_initial_and_recovery_reservations() {
    use crate::enrollment::Stage;
    for stage in [
        Stage::Written,
        Stage::Synced,
        Stage::Renamed,
        Stage::DirectorySynced,
    ] {
        for recovering in [false, true] {
            let f = Fixture::new();
            let mut receiver = Receiver::default();
            let now = 2_000_000;
            let begun = f.begin(&mut receiver, now).unwrap();
            let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
            let expected = if recovering {
                Some(
                    receiver
                        .finish(&f.dir, &commit, &[4; 32], None, now)
                        .unwrap(),
                )
            } else {
                None
            };
            let recover = f.finish(&begun, Action::Recover, [3; 32], [5; 32]);
            receiver.fail_registry = Some(stage);
            assert!(
                receiver
                    .finish(
                        &f.dir,
                        if recovering { &recover } else { &commit },
                        if recovering { &[5; 32] } else { &[4; 32] },
                        None,
                        now,
                    )
                    .is_err()
            );
            let replaced = matches!(stage, Stage::Renamed | Stage::DirectorySynced);
            let registry = Registry::read_current(&f.dir).unwrap();
            if !recovering && !replaced {
                assert!(registry.renewal_receipts.is_empty());
                assert_eq!(registry.credentials.len(), 1);
                receiver
                    .finish(&f.dir, &commit, &[4; 32], None, now)
                    .unwrap();
            } else {
                assert_eq!(registry.renewal_receipts.len(), 1);
                assert_eq!(
                    registry.renewal_receipts[0].deliveries,
                    if recovering && replaced { 2 } else { 1 }
                );
                assert_eq!(
                    registry.enrollments[0].credential_kinds[0]
                        .generations
                        .current,
                    2
                );
            }
            receiver = Receiver::default();
            let result = receiver.finish(&f.dir, &recover, &[5; 32], None, now + 1);
            if recovering && replaced {
                assert!(result.is_err());
            } else {
                let recovered = result.unwrap();
                if let Some(expected) = expected {
                    assert_eq!(expected, recovered);
                }
            }
            assert_eq!(
                Registry::read_current(&f.dir).unwrap().renewal_receipts[0].deliveries,
                2
            );
            assert!(
                receiver
                    .finish(&f.dir, &recover, &[5; 32], None, now + 2)
                    .is_err()
            );
        }
    }
}

#[test]
fn optional_only_renewal_preserves_certificate_for_later_required_enable() {
    let f = Fixture::with_custody(true);
    let custody = crate::receiver_custody::ReceiverCustody::open_at_endpoint(
        &f.dir,
        100,
        "https://receiver.example:443",
    )
    .unwrap();
    let mut registry = Registry::read_current(&f.dir).unwrap();
    let e = &mut registry.enrollments[0];
    e.receiver_id = custody.receiver_id().into();
    let pin = crate::client_ca::pin(f.key.public_key().as_ref()).unwrap();
    e.enrolled_pin = Some(pin.pin.clone());
    e.client_certificate_pins = vec![pin];
    let mut required = KindScope::new(Kind::AnalyticsPublish, 1);
    required.client_cert_required = true;
    e.credential_kinds.push(required);
    e.binding = Binding::Admitted {
        namespace: "namespace".into(),
        origin_incarnation: "origin".into(),
        index_incarnation: "index".into(),
        publisher_claim_epoch: crate::enrollment::ClaimEpoch::Unassigned,
        issuer: crate::enrollment::Issuer {
            kind: crate::enrollment::IssuerKind::Receiver,
            origin_trust_acknowledged: true,
        },
        assertion_issuer_keys: vec![crate::enrollment::IssuerKey {
            key_id: "issuer".into(),
            public_key: [1; 32],
        }],
    };
    let mut revoked = registry.credentials[0].clone();
    revoked.kind = Kind::AnalyticsPublish;
    revoked.credential_id = "c".repeat(64);
    revoked.sha256 = Secret::new("revoked-analytics".into()).hash();
    registry.credentials.push(revoked);
    let initial = Registry::from_records(registry.enrollments, registry.credentials).unwrap();
    fs::write(
        f.dir.join("enrollment/registry.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    Store::open(&f.dir)
        .unwrap()
        .owner_update(
            &f.dir,
            Some(Kind::AnalyticsPublish),
            crate::enrollment::OwnerAction::Revoke,
            "owner",
            &"a".repeat(64),
            100,
        )
        .unwrap();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let request = wire::Begin {
        version: 1,
        enrollment_id: "a".repeat(64),
        credentials: f.credentials(),
    };
    let begun = receiver.begin(&f.dir, &request, None, now).unwrap();
    // This fixture uses receiver custody's actual identity in proof construction.
    let sign = |begun: &Begun,
                action,
                application: [u8; 32],
                exporter: [u8; 32],
                credentials: Vec<Credential>| {
        let signed = wire::possession_message(
            custody.receiver_id(),
            "https://receiver.example:443",
            begun,
            &wire::identities(&credentials).unwrap(),
            &exporter,
            action,
            &application,
        )
        .unwrap();
        wire::Finish {
            version: 1,
            enrollment_id: begun.enrollment_id.clone(),
            renewal_id: begun.renewal_id.clone(),
            action,
            credentials,
            application_key: provision_wire::encode(&application),
            proof: provision_wire::encode(
                f.key.sign(&SystemRandom::new(), &signed).unwrap().as_ref(),
            ),
        }
    };
    let commit = sign(&begun, Action::Commit, [3; 32], [4; 32], f.credentials());
    let delivered: wire::Delivered = serde_json::from_slice(
        &receiver
            .finish(&f.dir, &commit, &[4; 32], None, now)
            .unwrap(),
    )
    .unwrap();
    use base64::Engine;
    let certificate = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(delivered.client_certificate.as_ref().unwrap())
        .unwrap();
    let identity = VerifiedClientIdentity::from_verified_der(
        &rustls::pki_types::CertificateDer::from(certificate),
    )
    .unwrap();
    let mirror = &delivered.credentials[0];
    let secret = Secret::new(mirror.bearer.clone());
    Store::authorize_cutover(
        &f.dir,
        Kind::MirrorPush,
        &secret,
        Some(identity.spki()),
        now + 1,
    )
    .unwrap();
    Store::open(&f.dir)
        .unwrap()
        .compact(&f.dir, now + 1)
        .unwrap();
    let compacted = Registry::read_current(&f.dir).unwrap();
    assert!(
        compacted.enrollments[0]
            .client_certificate_pins
            .iter()
            .all(|p| p.generation == 2)
    );
    Store::open(&f.dir)
        .unwrap()
        .owner_update(
            &f.dir,
            Some(Kind::AnalyticsPublish),
            crate::enrollment::OwnerAction::Enable,
            "owner",
            &begun.enrollment_id,
            now + 2,
        )
        .unwrap();
    let credentials = || {
        vec![Credential {
            kind: Kind::MirrorPush,
            generation: 2,
            bearer: mirror.bearer.clone(),
        }]
    };
    let request = wire::Begin {
        version: 1,
        enrollment_id: begun.enrollment_id.clone(),
        credentials: credentials(),
    };
    assert!(receiver.begin(&f.dir, &request, None, now + 2).is_err());
    let enabled = receiver
        .begin(&f.dir, &request, Some(&identity), now + 2)
        .unwrap();
    let commit = sign(&enabled, Action::Commit, [6; 32], [7; 32], credentials());
    let initial = receiver
        .finish(&f.dir, &commit, &[7; 32], Some(&identity), now + 2)
        .unwrap();
    let recover = sign(&enabled, Action::Recover, [6; 32], [8; 32], credentials());
    assert_eq!(
        receiver
            .finish(&f.dir, &recover, &[8; 32], Some(&identity), now + 3)
            .unwrap(),
        initial
    );
}

#[test]
fn unrelated_expiry_audit_failure_does_not_block_live_signed_cancel() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let expired = f.begin(&mut receiver, now).unwrap();
    // Add a second enrollment with independent bearer and a later deadline.
    let mut registry = Registry::read_current(&f.dir).unwrap();
    let mut e = registry.enrollments[0].clone();
    e.enrollment_id = "c".repeat(64);
    e.machine = "second".into();
    e.sequence = 2;
    let mut b = registry.credentials[0].clone();
    b.enrollment_id = e.enrollment_id.clone();
    b.credential_id = "d".repeat(64);
    b.sha256 = Secret::new("second-bearer".into()).hash();
    registry.enrollments.push(e);
    registry.credentials.push(b);
    registry.next_sequence = 3;
    Store::open(&f.dir)
        .unwrap()
        .commit(registry.revision(), &registry)
        .unwrap();
    let credentials = || {
        vec![Credential {
            kind: Kind::MirrorPush,
            generation: 1,
            bearer: "second-bearer".into(),
        }]
    };
    let live = receiver
        .begin(
            &f.dir,
            &wire::Begin {
                version: 1,
                enrollment_id: "c".repeat(64),
                credentials: credentials(),
            },
            None,
            now + 100,
        )
        .unwrap();
    let cancel = f.finish_using(&live, Action::Cancel, [3; 32], [4; 32], credentials());
    fs::rename(f.dir.join("receiver-audit"), f.dir.join("saved-audit")).unwrap();
    receiver
        .finish(&f.dir, &cancel, &[4; 32], None, expired.expires_at)
        .unwrap();
    assert!(receiver.pending.contains_key(&expired.enrollment_id));
    assert!(!receiver.pending.contains_key(&live.enrollment_id));
    let cancel_expired = f.finish(&expired, Action::Cancel, [3; 32], [4; 32]);
    assert!(
        receiver
            .finish(&f.dir, &cancel_expired, &[4; 32], None, expired.expires_at)
            .is_err()
    );
    fs::rename(f.dir.join("saved-audit"), f.dir.join("receiver-audit")).unwrap();
    receiver.expire(&f.dir, expired.expires_at).unwrap();
    assert!(receiver.pending.is_empty());
    assert!(
        fs::read_to_string(f.dir.join("receiver-audit/renewals.jsonl"))
            .unwrap()
            .contains("expired")
    );
}

#[test]
fn terminal_cancel_cutover_and_revoke_work_near_admission_with_audit_unavailable() {
    for committed in [false, true] {
        let f = Fixture::new();
        let mut receiver = Receiver::default();
        let now = 2_000_000;
        let begun = f.begin(&mut receiver, now).unwrap();
        if committed {
            let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
            receiver
                .finish(&f.dir, &commit, &[4; 32], None, now)
                .unwrap();
        }
        {
            let mut store = Store::open(&f.dir).unwrap();
            let mut registry = store.snapshot().unwrap();
            let sample = crate::enrollment::Tombstone {
                enrollment_id: format!("{:064x}", 1),
                machine: "m".repeat(63),
                sequence: 1,
                pin: None,
                high_water: [u64::MAX; 3],
            };
            let available = crate::enrollment::ADMISSION_CAP as usize
                - 4 * 8192
                - serde_json::to_vec(&registry).unwrap().len();
            let count = available / (serde_json::to_vec(&sample).unwrap().len() + 1);
            for n in 1..=count {
                let mut tombstone = sample.clone();
                tombstone.enrollment_id = format!("{n:064x}");
                registry.tombstones.push(tombstone);
            }
            assert!(registry.check_admission().is_ok());
            assert!(
                serde_json::to_vec(&registry).unwrap().len() + 4 * 8192
                    > crate::enrollment::ADMISSION_CAP as usize - 512
            );
            store.commit(registry.revision(), &registry).unwrap();
        }
        fs::rename(f.dir.join("receiver-audit"), f.dir.join("saved-audit")).unwrap();
        if committed {
            let secret = wire::bearer(
                &[3; 32],
                &begun.enrollment_id,
                &begun.renewal_id,
                &begun.generations[0],
            );
            Store::authorize_cutover(&f.dir, Kind::MirrorPush, &secret, None, now).unwrap();
            assert!(
                Registry::read_current(&f.dir).unwrap().enrollments[0].credential_kinds[0]
                    .generations
                    .retiring
                    .is_none()
            );
        } else {
            let cancel = f.finish(&begun, Action::Cancel, [3; 32], [4; 32]);
            receiver
                .finish(&f.dir, &cancel, &[4; 32], None, now)
                .unwrap();
            let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
            assert!(
                receiver
                    .finish(&f.dir, &commit, &[4; 32], None, now)
                    .is_err()
            );
        }
        Store::open(&f.dir)
            .unwrap()
            .owner_update(
                &f.dir,
                None,
                crate::enrollment::OwnerAction::Revoke,
                "owner",
                &begun.enrollment_id,
                now,
            )
            .unwrap();
        let registry = Registry::read_current(&f.dir).unwrap();
        assert!(registry.enrollments[0].revoked_at.is_some());
        assert!(serde_json::to_vec(&registry).unwrap().len() <= crate::enrollment::CAP as usize);
    }
}

#[test]
fn renewal_first_use_cuts_over_each_kind_independently() {
    let f = Fixture::new();
    let mut registry = Registry::read_current(&f.dir).unwrap();
    let e = &mut registry.enrollments[0];
    e.binding = Binding::Admitted {
        namespace: "namespace".into(),
        origin_incarnation: "origin".into(),
        index_incarnation: "index".into(),
        publisher_claim_epoch: crate::enrollment::ClaimEpoch::Unassigned,
        issuer: crate::enrollment::Issuer {
            kind: crate::enrollment::IssuerKind::Receiver,
            origin_trust_acknowledged: true,
        },
        assertion_issuer_keys: vec![crate::enrollment::IssuerKey {
            key_id: "issuer".into(),
            public_key: [1; 32],
        }],
    };
    e.credential_kinds
        .push(KindScope::new(Kind::AnalyticsPublish, 1));
    let mut second = registry.credentials[0].clone();
    second.credential_id = "c".repeat(64);
    second.kind = Kind::AnalyticsPublish;
    second.sha256 = Secret::new("analytics-secret".into()).hash();
    registry.credentials.push(second);
    let initial = Registry::from_records(registry.enrollments, registry.credentials).unwrap();
    fs::write(
        f.dir.join("enrollment/registry.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    let credentials = || {
        vec![
            Credential {
                kind: Kind::MirrorPush,
                generation: 1,
                bearer: f.secret.clone(),
            },
            Credential {
                kind: Kind::AnalyticsPublish,
                generation: 1,
                bearer: "analytics-secret".into(),
            },
        ]
    };
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = receiver
        .begin(
            &f.dir,
            &wire::Begin {
                version: 1,
                enrollment_id: "a".repeat(64),
                credentials: credentials(),
            },
            None,
            now,
        )
        .unwrap();
    let commit = f.finish_using(&begun, Action::Commit, [3; 32], [4; 32], credentials());
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
    for kind in [Kind::MirrorPush, Kind::AnalyticsPublish] {
        let target = begun.generations.iter().find(|g| g.kind == kind).unwrap();
        let secret = wire::bearer(&[3; 32], &begun.enrollment_id, &begun.renewal_id, target);
        Store::authorize_cutover(&f.dir, kind, &secret, None, now + 1).unwrap();
        let registry = Registry::read_current(&f.dir).unwrap();
        assert!(
            registry.enrollments[0]
                .credential_kinds
                .iter()
                .find(|s| s.kind == kind)
                .unwrap()
                .generations
                .retiring
                .is_none()
        );
        if kind == Kind::MirrorPush {
            let analytics = registry.enrollments[0]
                .credential_kinds
                .iter()
                .find(|s| s.kind == Kind::AnalyticsPublish)
                .unwrap();
            assert_eq!(
                analytics.generations.retiring.as_ref().unwrap().cutover_at,
                now + wire::OVERLAP
            );
            assert!(matches!(
                registry
                    .authorize(
                        Kind::AnalyticsPublish,
                        Kind::AnalyticsPublish,
                        &Secret::new("analytics-secret".into()),
                        None,
                        now + 1
                    )
                    .decision,
                Decision::Allowed { generation: 1, .. }
            ));
            let recover = f.finish_using(&begun, Action::Recover, [3; 32], [5; 32], credentials());
            assert!(
                receiver
                    .finish(&f.dir, &recover, &[5; 32], None, now + 1)
                    .is_err()
            );
        }
    }
}

#[test]
fn authority_mutations_refuse_without_consuming_pending_or_delivery() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    for mutation in 0..7 {
        let mut finish = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
        match mutation {
            0 => finish.credentials.clear(),
            1 => finish.credentials.push(Credential {
                kind: Kind::MirrorPush,
                generation: 1,
                bearer: f.secret.clone(),
            }),
            2 => finish.credentials.push(Credential {
                kind: Kind::AnalyticsPublish,
                generation: 1,
                bearer: f.secret.clone(),
            }),
            3 => finish.credentials[0].kind = Kind::BodyChannel,
            4 => finish.credentials[0].generation = 2,
            5 => finish.enrollment_id = "c".repeat(64),
            6 => finish.credentials[0].bearer = "foreign-bearer".into(),
            _ => unreachable!(),
        }
        assert!(
            receiver
                .finish(&f.dir, &finish, &[4; 32], None, now)
                .is_err()
        );
        let registry = Registry::read_current(&f.dir).unwrap();
        assert!(registry.renewal_receipts.is_empty());
        assert_eq!(registry.credentials.len(), 1);
        assert_eq!(f.begin(&mut receiver, now).unwrap(), begun);
    }
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
}

#[test]
fn unavailable_audit_blocks_release_and_compaction_but_keeps_committed_authority() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    let audit = f.dir.join("receiver-audit");
    let saved = f.dir.join("saved-audit");
    fs::rename(&audit, &saved).unwrap();
    assert!(
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, now)
            .is_err()
    );
    assert!(
        Registry::read_current(&f.dir)
            .unwrap()
            .renewal_receipts
            .is_empty()
    );
    fs::rename(&saved, &audit).unwrap();
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
    fs::rename(&audit, &saved).unwrap();
    let recover = f.finish(&begun, Action::Recover, [3; 32], [5; 32]);
    assert!(
        receiver
            .finish(&f.dir, &recover, &[5; 32], None, now)
            .is_err()
    );
    assert_eq!(
        Registry::read_current(&f.dir).unwrap().renewal_receipts[0].deliveries,
        1
    );
    assert!(
        Store::open(&f.dir)
            .unwrap()
            .compact(&f.dir, now + wire::OVERLAP)
            .is_err()
    );
    let registry = Registry::read_current(&f.dir).unwrap();
    assert_eq!(registry.credentials.len(), 2);
    assert_eq!(registry.renewal_receipts.len(), 1);
    fs::rename(&saved, &audit).unwrap();
    Store::open(&f.dir)
        .unwrap()
        .compact(&f.dir, now + wire::OVERLAP)
        .unwrap();
    assert_eq!(Registry::read_current(&f.dir).unwrap().credentials.len(), 1);
}

#[test]
fn ended_renewal_compaction_audits_before_every_replace_failure() {
    use crate::enrollment::{Error, Stage};
    for stage in [
        Stage::Written,
        Stage::Synced,
        Stage::Renamed,
        Stage::DirectorySynced,
    ] {
        let f = Fixture::new();
        let mut receiver = Receiver::default();
        let now = 2_000_000;
        let begun = f.begin(&mut receiver, now).unwrap();
        let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, now)
            .unwrap();
        let ended = now + wire::OVERLAP;
        {
            let mut store = Store::open(&f.dir).unwrap();
            assert!(
                store
                    .compact_with_fault(&f.dir, ended, |at| {
                        if at == stage {
                            Err(Error::Write)
                        } else {
                            Ok(())
                        }
                    })
                    .is_err()
            );
        }
        let audit = fs::read_to_string(
            f.dir
                .join("receiver-audit")
                .join(crate::generation_audit::FILE),
        )
        .unwrap();
        assert!(audit.contains(&"b".repeat(64)));
        let registry = Registry::read_current(&f.dir).unwrap();
        if matches!(stage, Stage::Renamed | Stage::DirectorySynced) {
            assert_eq!(registry.credentials.len(), 1);
            assert!(registry.renewal_receipts.is_empty());
        } else {
            assert_eq!(registry.credentials.len(), 2);
            assert_eq!(registry.renewal_receipts.len(), 1);
        }
        Store::open(&f.dir).unwrap().compact(&f.dir, ended).unwrap();
        let registry = Registry::read_current(&f.dir).unwrap();
        assert_eq!(registry.credentials.len(), 1);
        assert!(registry.renewal_receipts.is_empty());
        assert_eq!(
            registry.enrollments[0].credential_kinds[0]
                .generations
                .current,
            2
        );
    }
}

#[test]
fn competing_finishes_commit_only_first_application_key_and_replay_requires_recover() {
    for reverse in [false, true] {
        let f = Fixture::new();
        let mut receiver = Receiver::default();
        let now = 2_000_000;
        let begun = f.begin(&mut receiver, now).unwrap();
        let applications = if reverse {
            [[7; 32], [3; 32]]
        } else {
            [[3; 32], [7; 32]]
        };
        let winner = f.finish(&begun, Action::Commit, applications[0], [4; 32]);
        let loser = f.finish(&begun, Action::Commit, applications[1], [5; 32]);
        let delivered = receiver
            .finish(&f.dir, &winner, &[4; 32], None, now)
            .unwrap();
        assert!(
            receiver
                .finish(&f.dir, &loser, &[5; 32], None, now)
                .is_err()
        );
        assert_eq!(f.begin(&mut receiver, now).unwrap(), begun);
        assert!(
            receiver
                .finish(&f.dir, &winner, &[4; 32], None, now)
                .is_err()
        );
        let wrong = f.finish(&begun, Action::Recover, applications[1], [6; 32]);
        assert!(
            receiver
                .finish(&f.dir, &wrong, &[6; 32], None, now)
                .is_err()
        );
        let right = f.finish(&begun, Action::Recover, applications[0], [6; 32]);
        assert_eq!(
            receiver
                .finish(&f.dir, &right, &[6; 32], None, now)
                .unwrap(),
            delivered
        );
        let registry = Registry::read_current(&f.dir).unwrap();
        assert_eq!(registry.credentials.len(), 2);
        assert_eq!(registry.renewal_receipts.len(), 1);
        assert_eq!(
            registry.enrollments[0].credential_kinds[0]
                .generations
                .current,
            2
        );
    }
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
fn overlap_caps_at_old_expiry_and_exact_deadline_refuses_recovery() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 3_999_990;
    let begun = f.begin(&mut receiver, now).unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
    let registry = Registry::read_current(&f.dir).unwrap();
    assert_eq!(registry.renewal_receipts[0].recovery_until, 4_000_000);
    assert_eq!(
        registry.enrollments[0].credential_kinds[0]
            .generations
            .retiring
            .as_ref()
            .unwrap()
            .cutover_at,
        4_000_000
    );
    let recover = f.finish(&begun, Action::Recover, [3; 32], [4; 32]);
    assert!(
        receiver
            .finish(&f.dir, &recover, &[4; 32], None, 4_000_000)
            .is_err()
    );
    assert!(
        receiver
            .finish(&f.dir, &recover, &[4; 32], None, now)
            .is_err()
    );
}
#[test]
fn wrong_key_and_revoke_between_begin_and_finish_refuse() {
    let f = Fixture::new();
    let other = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    let mut wrong = other.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    wrong.credentials = f.credentials();
    assert!(
        receiver
            .finish(&f.dir, &wrong, &[4; 32], None, now)
            .is_err()
    );
    Store::open(&f.dir)
        .unwrap()
        .owner_update(
            &f.dir,
            None,
            crate::enrollment::OwnerAction::Revoke,
            "owner",
            &begun.enrollment_id,
            now,
        )
        .unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    assert!(
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, now)
            .is_err()
    );
    assert!(
        Registry::read_current(&f.dir)
            .unwrap()
            .renewal_receipts
            .is_empty()
    );
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
    let audit = fs::read_to_string(f.dir.join("receiver-audit/renewals.jsonl")).unwrap();
    assert!(audit.contains("first_use_cutover"));
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

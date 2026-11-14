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
        let mut store = if certificate {
            Store::open(&dir).unwrap()
        } else {
            Store::initialize(&dir).unwrap()
        };
        let revision = store.snapshot().unwrap().revision();
        store.commit(revision, &registry).unwrap();
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
    assert_eq!(
        receiver
            .handle_audited(
                &f.dir,
                wire::Request::Begin(request(5)),
                [4; 32],
                None,
                2_000_001
            )
            .unwrap_err(),
        Failure::Capacity
    );
    let summary = fs::read_to_string(f.dir.join("receiver-audit/renewal-refusals.json")).unwrap();
    assert!(summary.contains("\"class\":\"capacity\""));
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
    let certificate = Registry::read_current(&f.dir).unwrap().renewal_receipts[0]
        .client_certificate
        .clone()
        .unwrap();
    assert_eq!(
        delivered.client_certificate.as_deref(),
        Some(provision_wire::encode(&certificate).as_str())
    );
    let identity = crate::serve::renewal_test_identity(&certificate);
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

#[test]
fn bogus_requests_have_fixed_audit_budget_and_leave_legitimate_renewal_capacity() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let original = fs::read(f.dir.join("enrollment/registry.json")).unwrap();
    for i in 0..256 {
        let request = wire::Request::Begin(wire::Begin {
            version: 1,
            enrollment_id: if i % 2 == 0 {
                "a".repeat(64)
            } else {
                format!("{i:064x}")
            },
            credentials: vec![Credential {
                kind: Kind::MirrorPush,
                generation: 1,
                bearer: "bogus".into(),
            }],
        });
        assert_eq!(
            receiver
                .handle_audited(&f.dir, request, [4; 32], None, 2_000_000)
                .unwrap_err(),
            Failure::InvalidCredential
        );
    }
    assert_eq!(
        fs::read(f.dir.join("enrollment/registry.json")).unwrap(),
        original
    );
    assert!(!f.dir.join("receiver-audit/renewals.jsonl").exists());
    let summary = fs::read(
        f.dir
            .join("receiver-audit")
            .join(crate::generation_audit::REFUSAL_FILE),
    )
    .unwrap();
    assert!(summary.len() as u64 <= crate::generation_audit::REFUSAL_CAP);
    let summary: serde_json::Value = serde_json::from_slice(&summary).unwrap();
    assert_eq!(summary["classes"][0]["count"], 256);
    assert_eq!(summary["classes"][0]["last"]["class"], "invalid_credential");
    // Refusal custody cannot take the successful-event append lock either.
    let lock = crate::body_cache::CacheDirectory::open(&f.dir.join("receiver-audit"))
        .unwrap()
        .open_file(Path::new("renewal-refusals.lock"), libc::O_RDWR)
        .unwrap();
    lock.try_lock().unwrap();
    let begun = f.begin(&mut receiver, 2_000_000).unwrap();
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, 2_000_000)
        .unwrap();
    let recover = f.finish(&begun, Action::Recover, [3; 32], [5; 32]);
    receiver
        .finish(&f.dir, &recover, &[5; 32], None, 2_000_000)
        .unwrap();
    assert_eq!(
        Registry::read_current(&f.dir).unwrap().renewal_receipts[0].deliveries,
        2
    );
}

#[test]
fn local_failure_classes_survive_until_uniform_wire_dispatch() {
    let f = Fixture::new();
    let auth = crate::serve::RegistryAuthenticator::open(&f.dir).unwrap();
    for body in [br#"{"version":2}"#.as_slice(), b"{"] {
        assert_eq!(
            auth.renew_encoded(wire::Route::Begin, body, [4; 32], None, 2_000_000)
                .unwrap_err(),
            Failure::Malformed
        );
    }
    let summary: serde_json::Value = serde_json::from_slice(
        &fs::read(f.dir.join("receiver-audit/renewal-refusals.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(summary["classes"][0]["last"]["class"], "malformed");
    assert_eq!(summary["classes"][0]["count"], 2);
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    for _ in 0..6 {
        let begun = f.begin(&mut receiver, now).unwrap();
        let cancel = f.finish(&begun, Action::Cancel, [3; 32], [4; 32]);
        receiver
            .finish(&f.dir, &cancel, &[4; 32], None, now)
            .unwrap();
    }
    assert_eq!(f.begin(&mut receiver, now).unwrap_err(), Failure::Rate);
    let rate = wire::Request::Begin(wire::Begin {
        version: 1,
        enrollment_id: "a".repeat(64),
        credentials: f.credentials(),
    });
    assert_eq!(
        receiver
            .handle_audited(&f.dir, rate, [4; 32], None, now)
            .unwrap_err(),
        Failure::Rate
    );
    let summary = fs::read_to_string(f.dir.join("receiver-audit/renewal-refusals.json")).unwrap();
    assert!(summary.contains("\"class\":\"rate\""));
    assert!(summary.contains(&"a".repeat(64)));
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let begun = f.begin(&mut receiver, now).unwrap();
    receiver.fail_registry = Some(crate::enrollment::Stage::Renamed);
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    assert_eq!(
        receiver
            .handle_audited(&f.dir, wire::Request::Finish(commit), [4; 32], None, now)
            .unwrap_err(),
        Failure::CommitUncertain
    );
    let summary = fs::read_to_string(f.dir.join("receiver-audit/renewal-refusals.json")).unwrap();
    assert!(summary.contains("commit_uncertain"));
    assert!(summary.contains(&begun.renewal_id));
    assert!(summary.contains("\"generation\":2"));
    fs::rename(f.dir.join("enrollment"), f.dir.join("saved-enrollment")).unwrap();
    let request = wire::Request::Begin(wire::Begin {
        version: 1,
        enrollment_id: "a".repeat(64),
        credentials: f.credentials(),
    });
    assert_eq!(
        receiver
            .handle_audited(&f.dir, request, [4; 32], None, now)
            .unwrap_err(),
        Failure::Custody
    );
    let summary = fs::read_to_string(f.dir.join("receiver-audit/renewal-refusals.json")).unwrap();
    assert!(summary.contains("\"class\":\"custody\""));
    assert_eq!(Failure::from(Error::Full), Failure::Capacity);
    assert_eq!(
        Failure::from(Error::AuditUnavailable),
        Failure::AuditUnavailable
    );
}

#[test]
fn audit_append_faults_preserve_compaction_evidence_and_delivery_reservations() {
    use crate::generation_audit::{AppendFault, fail_next_append};
    for fault in [AppendFault::Partial, AppendFault::Sync] {
        for recovery in [false, true] {
            let f = Fixture::new();
            let mut receiver = Receiver::default();
            let now = 2_000_000;
            let begun = f.begin(&mut receiver, now).unwrap();
            if recovery {
                let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
                receiver
                    .finish(&f.dir, &commit, &[4; 32], None, now)
                    .unwrap();
            }
            let finish = f.finish(
                &begun,
                if recovery {
                    Action::Recover
                } else {
                    Action::Commit
                },
                [3; 32],
                [4; 32],
            );
            fail_next_append(fault);
            assert_eq!(
                receiver
                    .finish(&f.dir, &finish, &[4; 32], None, now)
                    .unwrap_err(),
                Failure::AuditUnavailable
            );
            let registry = Registry::read_current(&f.dir).unwrap();
            if recovery {
                assert_eq!(registry.renewal_receipts[0].deliveries, 1);
            } else {
                assert!(registry.renewal_receipts.is_empty());
                assert_eq!(
                    registry.enrollments[0].credential_kinds[0]
                        .generations
                        .current,
                    1
                );
            }
            let retry = receiver.finish(&f.dir, &finish, &[4; 32], None, now);
            match fault {
                AppendFault::Partial => assert_eq!(retry.unwrap_err(), Failure::AuditUnavailable),
                AppendFault::Sync => {
                    retry.unwrap();
                }
            }
        }
        let f = Fixture::new();
        let mut store = Store::open(&f.dir).unwrap();
        fail_next_append(fault);
        assert!(matches!(
            store.compact(&f.dir, 4_000_000),
            Err(Error::AuditUnavailable)
        ));
        assert_eq!(Registry::read_current(&f.dir).unwrap().credentials.len(), 1);
        let retry = store.compact(&f.dir, 4_000_000);
        match fault {
            AppendFault::Partial => {
                assert!(matches!(retry, Err(Error::AuditUnavailable)));
                assert_eq!(Registry::read_current(&f.dir).unwrap().credentials.len(), 1);
            }
            AppendFault::Sync => {
                retry.unwrap();
                assert!(
                    Registry::read_current(&f.dir)
                        .unwrap()
                        .credentials
                        .is_empty()
                );
            }
        }
    }
}

// Spawned through the Rust test runner so a real independent process owns the
// registry flock. stdin/stdout are barriers; there are no timing-based sleeps.
#[test]
fn registry_lock_child() {
    use std::io::{Read, Write};
    let Some(dir) = std::env::var_os("SEMON_RENEWAL_LOCK_CHILD") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mut store = Store::open(&dir).unwrap();
    writeln!(std::io::stdout(), "REGISTRY_LOCKED").unwrap();
    std::io::stdout().flush().unwrap();
    let mut release = [0];
    std::io::stdin().read_exact(&mut release).unwrap();
    match release[0] {
        b'r' => {
            store
                .owner_update(
                    &dir,
                    None,
                    crate::enrollment::OwnerAction::Revoke,
                    "owner",
                    &"a".repeat(64),
                    2_000_000,
                )
                .unwrap();
        }
        b'e' => {
            store
                .owner_update(
                    &dir,
                    Some(Kind::MirrorPush),
                    crate::enrollment::OwnerAction::Enable,
                    "owner",
                    &"a".repeat(64),
                    2_000_000,
                )
                .unwrap();
        }
        _ => {}
    }
}
struct RegistryChild(std::process::Child);
impl Drop for RegistryChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        // Never wait without a deadline, including while unwinding a failed test.
    }
}
const CHILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
fn child_ready(fd: std::os::fd::RawFd, events: libc::c_short, deadline: std::time::Instant) {
    let remaining = deadline
        .checked_duration_since(std::time::Instant::now())
        .expect("registry child IO timed out");
    let mut descriptor = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: descriptor points to one live pollfd; timeout is bounded to ten seconds.
    let ready = unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis().max(1) as i32) };
    assert!(ready > 0, "registry child IO timed out or poll failed");
}
fn hold_registry_in_child(dir: &Path) -> RegistryChild {
    use std::{io::Read, os::fd::AsRawFd};
    let mut child = RegistryChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "renewal::tests::registry_lock_child",
                "--nocapture",
            ])
            .env("SEMON_RENEWAL_LOCK_CHILD", dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let output = child.0.stdout.as_mut().unwrap();
    let deadline = std::time::Instant::now() + CHILD_TIMEOUT;
    let mut line = Vec::new();
    loop {
        child_ready(output.as_raw_fd(), libc::POLLIN, deadline);
        let mut byte = [0];
        assert_eq!(
            output.read(&mut byte).unwrap(),
            1,
            "child exited before acquiring registry lock"
        );
        line.push(byte[0]);
        assert!(line.len() <= 4096, "unexpected registry child output");
        if byte[0] == b'\n' {
            if String::from_utf8_lossy(&line).contains("REGISTRY_LOCKED") {
                break;
            }
            line.clear();
        }
    }
    child
}
fn release_registry_child(mut child: RegistryChild, action: u8) {
    use std::{io::Write, os::fd::AsRawFd};
    let deadline = std::time::Instant::now() + CHILD_TIMEOUT;
    let mut input = child.0.stdin.take().unwrap();
    child_ready(input.as_raw_fd(), libc::POLLOUT, deadline);
    input.write_all(&[action]).unwrap();
    drop(input);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "registry child failed: {status}");
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "registry child exit timed out"
        );
        std::thread::park_timeout(std::time::Duration::from_millis(10));
    }
}
#[test]
fn cross_process_registry_contention_refuses_and_retry_reads_owner_commit() {
    for revoke in [false, true] {
        let f = Fixture::new();
        let mut receiver = Receiver::default();
        let now = 2_000_000;
        let begun = f.begin(&mut receiver, now).unwrap();
        let child = hold_registry_in_child(&f.dir);
        assert_eq!(
            f.begin(&mut receiver, now).unwrap_err(),
            Failure::Contention
        );
        let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
        assert_eq!(
            receiver
                .finish(&f.dir, &commit, &[4; 32], None, now)
                .unwrap_err(),
            Failure::Contention
        );
        assert!(matches!(Store::open(&f.dir), Err(Error::InUse)));
        assert!(
            Registry::read_current(&f.dir)
                .unwrap()
                .renewal_receipts
                .is_empty()
        );
        release_registry_child(child, if revoke { b'r' } else { b'x' });
        if revoke {
            assert_eq!(
                receiver
                    .finish(&f.dir, &commit, &[4; 32], None, now)
                    .unwrap_err(),
                Failure::EndedAuthority
            );
            assert!(
                Registry::read_current(&f.dir)
                    .unwrap()
                    .renewal_receipts
                    .is_empty()
            );
        } else {
            assert_eq!(f.begin(&mut receiver, now).unwrap(), begun);
            receiver
                .finish(&f.dir, &commit, &[4; 32], None, now)
                .unwrap();
            // A distinct handler sees committed state after the lock is released.
            let mut other = Receiver::default();
            assert_eq!(f.begin(&mut other, now).unwrap(), begun);
            assert!(other.finish(&f.dir, &commit, &[4; 32], None, now).is_err());
            let recover = f.finish(&begun, Action::Recover, [3; 32], [5; 32]);
            let child = hold_registry_in_child(&f.dir);
            assert_eq!(
                other
                    .finish(&f.dir, &recover, &[5; 32], None, now)
                    .unwrap_err(),
                Failure::Contention
            );
            release_registry_child(child, b'x');
            other.finish(&f.dir, &recover, &[5; 32], None, now).unwrap();
            assert!(
                receiver
                    .finish(&f.dir, &recover, &[5; 32], None, now)
                    .is_err()
            );
            assert_eq!(
                Registry::read_current(&f.dir).unwrap().renewal_receipts[0].deliveries,
                2
            );
        }
    }
}

#[test]
fn held_renewal_transaction_blocks_competing_owner_enable_and_revoke() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    // Hold the same lock a begin/finish transaction holds, while another thread
    // tries both owner actions. Neither action may mutate a cached snapshot.
    let store = Store::open(&f.dir).unwrap();
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                for action in [
                    crate::enrollment::OwnerAction::Enable,
                    crate::enrollment::OwnerAction::Revoke,
                ] {
                    let result = Store::open(&f.dir).and_then(|mut store| {
                        store.owner_update(
                            &f.dir,
                            Some(Kind::MirrorPush),
                            action,
                            "owner",
                            &begun.enrollment_id,
                            now,
                        )
                    });
                    assert!(matches!(result, Err(Error::InUse)));
                }
            })
            .join()
            .unwrap();
    });
    drop(store);
    let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
    assert_eq!(
        Registry::read_current(&f.dir).unwrap().renewal_receipts[0].deliveries,
        1
    );
}

#[test]
fn three_kind_certificate_commit_recovery_and_fully_occupied_encoded_budget() {
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
    e.credential_kinds.clear();
    registry.credentials.clear();
    for (index, kind) in [Kind::AnalyticsPublish, Kind::BodyChannel, Kind::MirrorPush]
        .into_iter()
        .enumerate()
    {
        let mut scope = KindScope::new(kind, 1);
        scope.client_cert_required = true;
        e.credential_kinds.push(scope);
        registry.credentials.push(Bearer {
            credential_id: format!("{:064x}", index + 10),
            enrollment_id: e.enrollment_id.clone(),
            kind,
            sha256: Secret::new(format!("old-{}", kind.wire_name())).hash(),
            generation: 1,
            valid_from: 0,
            valid_until: 4_000_000,
            revoked_at: None,
        });
    }
    let initial = Registry::from_records(registry.enrollments, registry.credentials).unwrap();
    fs::write(
        f.dir.join("enrollment/registry.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    let certificate = custody
        .issue_client_certificate(f.key.public_key().as_ref(), &"a".repeat(64), 100, 4_000_000)
        .unwrap();
    let identity = crate::serve::renewal_test_identity(&certificate);
    let credentials = || {
        [Kind::AnalyticsPublish, Kind::BodyChannel, Kind::MirrorPush]
            .into_iter()
            .map(|kind| Credential {
                kind,
                generation: 1,
                bearer: format!("old-{}", kind.wire_name()),
            })
            .collect::<Vec<_>>()
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
            Some(&identity),
            now,
        )
        .unwrap();
    let sign = |action, exporter| {
        let credentials = credentials();
        let signed = wire::possession_message(
            custody.receiver_id(),
            "https://receiver.example:443",
            &begun,
            &wire::identities(&credentials).unwrap(),
            &exporter,
            action,
            &[3; 32],
        )
        .unwrap();
        wire::Finish {
            version: 1,
            enrollment_id: begun.enrollment_id.clone(),
            renewal_id: begun.renewal_id.clone(),
            action,
            credentials,
            application_key: provision_wire::encode(&[3; 32]),
            proof: provision_wire::encode(
                f.key.sign(&SystemRandom::new(), &signed).unwrap().as_ref(),
            ),
        }
    };
    let commit = sign(Action::Commit, [4; 32]);
    let delivered = receiver
        .finish(&f.dir, &commit, &[4; 32], Some(&identity), now)
        .unwrap();
    assert!(delivered.len() <= provision_wire::MAX_MESSAGE_BYTES);
    let message: wire::Delivered = serde_json::from_slice(&delivered).unwrap();
    assert_eq!(message.credentials.len(), 3);
    assert!(message.client_certificate.is_some());
    // Recovery from a restarted handler must regenerate every bearer and leaf.
    let mut receiver = Receiver::default();
    let recover = sign(Action::Recover, [5; 32]);
    assert_eq!(
        receiver
            .finish(&f.dir, &recover, &[5; 32], Some(&identity), now)
            .unwrap(),
        delivered
    );
    assert!(
        receiver
            .finish(&f.dir, &recover, &[5; 32], Some(&identity), now)
            .is_err()
    );
    let template = Registry::read_current(&f.dir).unwrap();
    assert_eq!(template.renewal_receipts[0].deliveries, 2);
    assert!(
        serde_json::to_vec(&template.renewal_receipts[0])
            .unwrap()
            .len()
            <= 8192
    );
    assert!(
        serde_json::to_vec(&template.renewal_admission[0])
            .unwrap()
            .len()
            <= 1024
    );
    // Combine real receipt/certificate records with six live generations per
    // enrollment, rolling admission, all tombstones and four reserved outcomes.
    let mut enrollments = vec![];
    let mut bearers = vec![];
    let mut receipts = vec![];
    let mut admission = vec![];
    for n in 1..=128u64 {
        let id = format!("{n:064x}");
        let mut e = template.enrollments[0].clone();
        e.enrollment_id = id.clone();
        e.machine = format!("machine-{n}");
        e.sequence = n;
        if let Binding::Admitted { namespace, .. } = &mut e.binding {
            *namespace = format!("namespace-{n}");
        }
        e.enrolled_pin = Some(crate::enrollment::Pin::Spki(
            Secret::new(format!("pin-{n}")).hash(),
        ));
        for pin in &mut e.client_certificate_pins {
            pin.pin = e.enrolled_pin.clone().unwrap();
        }
        enrollments.push(e);
        let mut receipt = template.renewal_receipts[0].clone();
        receipt.begun.enrollment_id = id.clone();
        receipt.begun.renewal_id = format!("{:064x}", n + 1000);
        for b in &template.credentials {
            let mut b = b.clone();
            b.enrollment_id = id.clone();
            b.sha256 = Secret::new(format!(
                "bearer-{n}-{}-{}",
                b.kind.wire_name(),
                b.generation
            ))
            .hash();
            b.credential_id = format!("{:064x}", n * 10 + b.kind.index() as u64 * 2 + b.generation);
            if b.generation == 2 {
                let issued = receipt
                    .credentials
                    .iter_mut()
                    .find(|issued| issued.kind == b.kind)
                    .unwrap();
                *issued = b.clone();
            }
            bearers.push(b);
        }
        receipts.push(receipt);
        admission.push(Admission {
            enrollment_id: id,
            creations: vec![now; 6],
        });
    }
    let mut full = Registry::from_records(enrollments, bearers).unwrap();
    full.latest = now;
    full.renewal_receipts = receipts;
    full.renewal_admission = admission;
    for n in 129..=640u64 {
        full.tombstones.push(crate::enrollment::Tombstone {
            enrollment_id: format!("{n:064x}"),
            machine: "m".repeat(63),
            sequence: n,
            pin: Some(crate::enrollment::Pin::Spki(
                Secret::new(format!("pin-{n}")).hash(),
            )),
            high_water: [u64::MAX; 3],
        });
    }
    full.next_sequence = 641;
    full.validate().unwrap();
    full.check_renewal_admission(4).unwrap();
    assert!(
        serde_json::to_vec(&full).unwrap().len() + 4 * 4096
            <= crate::enrollment::ADMISSION_CAP as usize
    );
}

#[test]
fn retry_after_cross_process_enable_reads_new_authorization_scope() {
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
    e.credential_kinds[0].available = false;
    e.credential_kinds
        .push(KindScope::new(Kind::AnalyticsPublish, 1));
    let mut analytics = registry.credentials[0].clone();
    registry.credentials[0].revoked_at = Some(100);
    analytics.kind = Kind::AnalyticsPublish;
    analytics.credential_id = "c".repeat(64);
    analytics.sha256 = Secret::new("analytics-old".into()).hash();
    registry.credentials.push(analytics);
    let initial = Registry::from_records(registry.enrollments, registry.credentials).unwrap();
    fs::write(
        f.dir.join("enrollment/registry.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    let credentials = || {
        vec![Credential {
            kind: Kind::AnalyticsPublish,
            generation: 1,
            bearer: "analytics-old".into(),
        }]
    };
    let request = || wire::Begin {
        version: 1,
        enrollment_id: "a".repeat(64),
        credentials: credentials(),
    };
    let mut receiver = Receiver::default();
    let child = hold_registry_in_child(&f.dir);
    assert_eq!(
        receiver
            .begin(&f.dir, &request(), None, 2_000_000)
            .unwrap_err(),
        Failure::Contention
    );
    release_registry_child(child, b'e');
    let begun = receiver.begin(&f.dir, &request(), None, 2_000_000).unwrap();
    assert_eq!(begun.generations.len(), 2);
    assert_eq!(begun.enable_authorization_ids.len(), 1);
    let commit = f.finish_using(&begun, Action::Commit, [3; 32], [4; 32], credentials());
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, 2_000_000)
        .unwrap();
    let registry = Registry::read_current(&f.dir).unwrap();
    assert_eq!(registry.renewal_receipts[0].deliveries, 1);
    assert!(
        registry.enrollments[0]
            .credential_kinds
            .iter()
            .all(|scope| scope.available && scope.generations.current == 2)
    );
}

#[test]
fn refused_request_io_does_not_hold_registry_authenticator_renewal_mutex() {
    use crate::generation_audit::{RefusalIo, set_refusal_io};
    use std::sync::mpsc;
    let f = Fixture::new();
    let auth = crate::serve::RegistryAuthenticator::open(&f.dir).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (legitimate_tx, legitimate_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let auth = &auth;
        let bogus = scope.spawn(move || {
            set_refusal_io(RefusalIo::Block {
                entered: entered_tx,
                release: release_rx,
            });
            auth.renew(
                wire::Request::Begin(wire::Begin {
                    version: 1,
                    enrollment_id: "a".repeat(64),
                    credentials: vec![Credential {
                        kind: Kind::MirrorPush,
                        generation: 1,
                        bearer: "bogus".into(),
                    }],
                }),
                [4; 32],
                None,
                2_000_000,
            )
        });
        entered_rx
            .recv_timeout(CHILD_TIMEOUT)
            .expect("bogus request never reached refusal IO");
        let credentials = f.credentials();
        scope.spawn(move || {
            legitimate_tx
                .send(auth.renew(
                    wire::Request::Begin(wire::Begin {
                        version: 1,
                        enrollment_id: "a".repeat(64),
                        credentials,
                    }),
                    [4; 32],
                    None,
                    2_000_000,
                ))
                .unwrap();
        });
        let legitimate = legitimate_rx.recv_timeout(std::time::Duration::from_secs(5));
        release_tx.send(()).unwrap();
        assert_eq!(
            bogus.join().unwrap().unwrap_err(),
            Failure::InvalidCredential
        );
        let bytes = legitimate
            .expect("legitimate renewal blocked behind refusal IO")
            .unwrap();
        let begun: Begun = serde_json::from_slice(&bytes).unwrap();
        let commit = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
        auth.renew(wire::Request::Finish(commit), [4; 32], None, 2_000_000)
            .unwrap();
    });
}

#[test]
fn interrupted_first_refusal_publication_leaves_no_empty_final_and_retries() {
    use crate::generation_audit::{REFUSAL_FILE, RefusalIo, set_refusal_io};
    let f = Fixture::new();
    let record = || crate::generation_audit::RefusalRecord {
        class: Failure::InvalidCredential,
        observed_at: 2_000_000,
        enrollment_id: "a".repeat(64),
        renewal_id: String::new(),
        generations: vec![],
        outcome: "refused".into(),
    };
    set_refusal_io(RefusalIo::Interrupt);
    assert!(matches!(
        crate::generation_audit::refusal(&f.dir, record()),
        Err(Error::AuditUnavailable)
    ));
    assert!(!f.dir.join("receiver-audit").join(REFUSAL_FILE).exists());
    assert!(f.dir.join("receiver-audit/renewal-refusals.stage").exists());
    crate::generation_audit::refusal(&f.dir, record()).unwrap();
    let path = f.dir.join("receiver-audit").join(REFUSAL_FILE);
    let summary: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(summary["classes"][0]["count"], 1);
    // Existing corrupt custody is preserved and returns an explicit local error;
    // write_refusal reports this failure on stderr, outside the renewal mutex.
    for bytes in [b"".as_slice(), b"corrupt"] {
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            crate::generation_audit::refusal(&f.dir, record()),
            Err(Error::AuditUnavailable)
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn authority_classes_and_all_eleven_aggregate_slots_fit_the_encoded_budget() {
    let f = Fixture::new();
    let auth = crate::serve::RegistryAuthenticator::open(&f.dir).unwrap();
    let request = |credentials| {
        wire::Request::Begin(wire::Begin {
            version: 1,
            enrollment_id: "a".repeat(64),
            credentials,
        })
    };
    let bogus = vec![Credential {
        kind: Kind::MirrorPush,
        generation: 1,
        bearer: "bogus".into(),
    }];
    assert_eq!(
        auth.renew(request(bogus), [4; 32], None, 2_000_000)
            .unwrap_err(),
        Failure::InvalidCredential
    );
    let mut mismatch = f.credentials();
    mismatch[0].generation = 2;
    assert_eq!(
        auth.renew(request(mismatch), [4; 32], None, 2_000_000)
            .unwrap_err(),
        Failure::ScopeMismatch
    );
    let begun: Begun = serde_json::from_slice(
        &auth
            .renew(request(f.credentials()), [4; 32], None, 2_000_000)
            .unwrap(),
    )
    .unwrap();
    let mut bad_key = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    bad_key.proof = provision_wire::encode(&[0; 64]);
    assert_eq!(
        auth.renew(wire::Request::Finish(bad_key), [4; 32], None, 2_000_000)
            .unwrap_err(),
        Failure::InvalidCredential
    );
    let cancel = f.finish(&begun, Action::Cancel, [3; 32], [4; 32]);
    auth.renew(wire::Request::Finish(cancel), [4; 32], None, 2_000_000)
        .unwrap();
    let replay = f.finish(&begun, Action::Commit, [3; 32], [4; 32]);
    assert_eq!(
        auth.renew(wire::Request::Finish(replay), [4; 32], None, 2_000_000)
            .unwrap_err(),
        Failure::ReplayTerminal
    );
    assert_eq!(
        auth.renew(request(f.credentials()), [4; 32], None, 4_000_000)
            .unwrap_err(),
        Failure::EndedAuthority
    );
    let path = f.dir.join("receiver-audit/renewal-refusals.json");
    let summary = fs::read_to_string(&path).unwrap();
    for class in [
        "invalid_credential",
        "scope_mismatch",
        "replay_terminal",
        "ended_authority",
    ] {
        assert!(summary.contains(class));
    }
    for class in [
        Failure::Malformed,
        Failure::InvalidCredential,
        Failure::EndedAuthority,
        Failure::ScopeMismatch,
        Failure::ReplayTerminal,
        Failure::Rate,
        Failure::Capacity,
        Failure::Custody,
        Failure::AuditUnavailable,
        Failure::CommitUncertain,
        Failure::Contention,
    ] {
        crate::generation_audit::refusal(
            &f.dir,
            crate::generation_audit::RefusalRecord {
                class,
                observed_at: u64::MAX,
                enrollment_id: "f".repeat(64),
                renewal_id: "f".repeat(64),
                generations: [Kind::AnalyticsPublish, Kind::BodyChannel, Kind::MirrorPush]
                    .into_iter()
                    .map(|kind| Generation {
                        kind,
                        generation: u64::MAX,
                    })
                    .collect(),
                outcome: "refused".into(),
            },
        )
        .unwrap();
    }
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.len() as u64 <= crate::generation_audit::REFUSAL_CAP);
    let summary: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary["classes"].as_array().unwrap().len(), 11);
}

#[test]
fn revoked_retired_and_wrong_certificate_keep_distinct_local_authority_class() {
    for ended in [false, true] {
        let f = Fixture::new();
        let mut registry = Registry::read_current(&f.dir).unwrap();
        if ended {
            registry.credentials[0].revoked_at = Some(100);
        } else {
            registry.enrollments[0].credential_kinds[0]
                .generations
                .current = 2;
            // No retiring slot accepts the old bearer. Keep a current record so
            // validation does not obscure the retired-generation diagnostic.
            let mut current = registry.credentials[0].clone();
            current.credential_id = "c".repeat(64);
            current.generation = 2;
            current.sha256 = Secret::new("current".into()).hash();
            registry.credentials.push(current);
        }
        let initial = Registry::from_records(registry.enrollments, registry.credentials).unwrap();
        fs::write(
            f.dir.join("enrollment/registry.json"),
            serde_json::to_vec(&initial).unwrap(),
        )
        .unwrap();
        let auth = crate::serve::RegistryAuthenticator::open(&f.dir).unwrap();
        assert_eq!(
            auth.renew(
                wire::Request::Begin(wire::Begin {
                    version: 1,
                    enrollment_id: "a".repeat(64),
                    credentials: f.credentials(),
                }),
                [4; 32],
                None,
                2_000_000
            )
            .unwrap_err(),
            Failure::EndedAuthority
        );
    }
    let f = Fixture::with_custody(true);
    let custody = crate::receiver_custody::ReceiverCustody::open_existing(&f.dir).unwrap();
    let certificate = custody
        .issue_client_certificate(f.key.public_key().as_ref(), &"a".repeat(64), 100, 4_000_000)
        .unwrap();
    let identity = crate::serve::renewal_test_identity(&certificate);
    // The registry has no pin for this presented certificate.
    let auth = crate::serve::RegistryAuthenticator::open(&f.dir).unwrap();
    assert_eq!(
        auth.renew(
            wire::Request::Begin(wire::Begin {
                version: 1,
                enrollment_id: "a".repeat(64),
                credentials: f.credentials(),
            }),
            [4; 32],
            Some(&identity),
            2_000_000
        )
        .unwrap_err(),
        Failure::InvalidCredential
    );
}

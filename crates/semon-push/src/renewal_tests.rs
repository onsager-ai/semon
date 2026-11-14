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
    fn finish(&self, begun: &Begun, application: [u8; 32], exporter: [u8; 32]) -> wire::Finish {
        self.finish_using(begun, application, exporter, self.credentials())
    }
    fn finish_using(
        &self,
        begun: &Begun,
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
            &application,
        )
        .unwrap();
        let proof = self.key.sign(&SystemRandom::new(), &signed).unwrap();
        wire::Finish {
            version: 1,
            enrollment_id: begun.enrollment_id.clone(),
            renewal_id: begun.renewal_id.clone(),
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
    let commit = f.finish_using(&begun, [3; 32], [4; 32], credentials());
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
            let recover = f.finish_using(&begun, [3; 32], [5; 32], credentials());
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
        let mut finish = f.finish(&begun, [3; 32], [4; 32]);
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
    let commit = f.finish(&begun, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
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
        let commit = f.finish(&begun, [3; 32], [4; 32]);
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
fn competing_finishes_commit_only_first_application_key() {
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
        let winner = f.finish(&begun, applications[0], [4; 32]);
        let loser = f.finish(&begun, applications[1], [5; 32]);
        let delivered = receiver
            .finish(&f.dir, &winner, &[4; 32], None, now)
            .unwrap();
        assert!(
            receiver
                .finish(&f.dir, &loser, &[5; 32], None, now)
                .is_err()
        );
        assert_eq!(f.begin(&mut receiver, now).unwrap(), begun);
        assert_eq!(
            receiver
                .finish(&f.dir, &winner, &[4; 32], None, now)
                .unwrap(),
            delivered
        );
        let wrong = f.finish(&begun, applications[1], [6; 32]);
        assert!(
            receiver
                .finish(&f.dir, &wrong, &[6; 32], None, now)
                .is_err()
        );
        let right = f.finish(&begun, applications[0], [6; 32]);
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
fn lost_response_retries_identically_after_restart_and_ten_minutes() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    let commit = f.finish(&begun, [3; 32], [4; 32]);
    let initial = receiver
        .finish(&f.dir, &commit, &[4; 32], None, now + 1)
        .unwrap();
    assert_eq!(
        receiver
            .finish(&f.dir, &commit, &[4; 32], None, now + 1)
            .unwrap(),
        initial
    );
    receiver = Receiver::default();
    assert_eq!(begun, f.begin(&mut receiver, now + 2).unwrap());
    let wrong_application = f.finish(&begun, [5; 32], [6; 32]);
    assert!(
        receiver
            .finish(&f.dir, &wrong_application, &[6; 32], None, now + 2)
            .is_err()
    );
    let recover = f.finish(&begun, [3; 32], [6; 32]);
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
    let retry = f.finish(&begun, [3; 32], [8; 32]);
    assert_eq!(
        receiver
            .finish(&f.dir, &retry, &[8; 32], None, now + 601)
            .unwrap(),
        initial
    );
    let registry = Registry::read_current(&f.dir).unwrap();
    assert_eq!(
        registry.enrollments[0].credential_kinds[0]
            .generations
            .current,
        2
    );
    assert_eq!(registry.credentials.len(), 2);
}

#[test]
fn overlap_caps_at_old_expiry_and_exact_deadline_refuses_recovery() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 3_999_990;
    let begun = f.begin(&mut receiver, now).unwrap();
    let commit = f.finish(&begun, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, now)
        .unwrap();
    let registry = Registry::read_current(&f.dir).unwrap();
    assert_eq!(registry.renewal_receipts[0].retry_until, 4_000_000);
    assert_eq!(
        registry.enrollments[0].credential_kinds[0]
            .generations
            .retiring
            .as_ref()
            .unwrap()
            .cutover_at,
        4_000_000
    );
    let recover = f.finish(&begun, [3; 32], [4; 32]);
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
    let mut wrong = other.finish(&begun, [3; 32], [4; 32]);
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
    let commit = f.finish(&begun, [3; 32], [4; 32]);
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
    let commit = f.finish(&begun, [3; 32], [4; 32]);
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
    let recover = f.finish(&begun, [3; 32], [4; 32]);
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
    let commit = f.finish(&begun, [3; 32], [4; 32]);
    receiver
        .finish(&f.dir, &commit, &[4; 32], None, 2_000_000)
        .unwrap();
    let recover = f.finish(&begun, [3; 32], [5; 32]);
    receiver
        .finish(&f.dir, &recover, &[5; 32], None, 2_000_000)
        .unwrap();
}

// Spawned through the Rust test runner so a real independent process owns the
// registry flock. stdin/stdout are barriers; there are no timing-based sleeps.

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
            .recv_timeout(std::time::Duration::from_secs(10))
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
        let commit = f.finish(&begun, [3; 32], [4; 32]);
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

#[test]
fn refusal_warning_latch_bounds_repeated_failures_and_concurrent_contention() {
    let f = Fixture::new();
    let directory = crate::body_cache::CacheDirectory::open(&f.dir.join("receiver-audit")).unwrap();
    let lock = directory
        .open_file(
            Path::new("renewal-refusals.lock"),
            libc::O_RDWR | libc::O_CREAT,
        )
        .unwrap();
    lock.try_lock().unwrap();
    let latch = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let latch = &latch;
            let dir = &f.dir;
            scope.spawn(move || {
                for _ in 0..32 {
                    let result = crate::generation_audit::refusal(
                        dir,
                        crate::generation_audit::RefusalRecord {
                            class: Failure::InvalidCredential,
                            observed_at: 2_000_000,
                            enrollment_id: String::new(),
                            renewal_id: String::new(),
                            generations: vec![],
                            outcome: "refused".into(),
                        },
                    );
                    assert!(matches!(result, Err(Error::InUse)));
                    assert!(!refusal_warning(latch, &result));
                }
            });
        }
    });
    assert!(
        !latch.load(Ordering::Relaxed),
        "contention consumed the storage warning"
    );
    let warnings = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let latch = &latch;
            let warnings = &warnings;
            scope.spawn(move || {
                for _ in 0..32 {
                    if refusal_warning(latch, &Err(Error::AuditUnavailable)) {
                        warnings.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    assert_eq!(warnings.load(Ordering::Relaxed), 1);
    assert!(!refusal_warning(&latch, &Ok(())));
    assert!(!refusal_warning(&latch, &Err(Error::AuditUnavailable)));
}

#[test]
fn hostile_identical_retries_do_not_write_registry_or_audit() {
    let f = Fixture::new();
    let mut receiver = Receiver::default();
    let now = 2_000_000;
    let begun = f.begin(&mut receiver, now).unwrap();
    let pending_registry = fs::read(f.dir.join("enrollment/registry.json")).unwrap();
    let pending_audit = fs::read(f.dir.join("receiver-audit/renewals.jsonl")).unwrap();
    for offset in 1..100 {
        assert_eq!(f.begin(&mut receiver, now + offset).unwrap(), begun);
    }
    assert_eq!(
        fs::read(f.dir.join("enrollment/registry.json")).unwrap(),
        pending_registry
    );
    assert_eq!(
        fs::read(f.dir.join("receiver-audit/renewals.jsonl")).unwrap(),
        pending_audit
    );
    let finish = f.finish(&begun, [3; 32], [4; 32]);
    let initial = receiver
        .finish(&f.dir, &finish, &[4; 32], None, now + 100)
        .unwrap();
    let committed_registry = fs::read(f.dir.join("enrollment/registry.json")).unwrap();
    let committed_audit = fs::read(f.dir.join("receiver-audit/renewals.jsonl")).unwrap();
    for offset in 601..701 {
        assert_eq!(f.begin(&mut receiver, now + offset).unwrap(), begun);
        let finish = f.finish(&begun, [3; 32], [5; 32]);
        assert_eq!(
            receiver
                .finish(&f.dir, &finish, &[5; 32], None, now + offset)
                .unwrap(),
            initial
        );
    }
    assert_eq!(
        fs::read(f.dir.join("enrollment/registry.json")).unwrap(),
        committed_registry
    );
    assert_eq!(
        fs::read(f.dir.join("receiver-audit/renewals.jsonl")).unwrap(),
        committed_audit
    );
    let registry = Registry::read_current(&f.dir).unwrap();
    assert_eq!(registry.credentials.len(), 2);
    assert_eq!(
        registry.enrollments[0].credential_kinds[0]
            .generations
            .current,
        2
    );
}

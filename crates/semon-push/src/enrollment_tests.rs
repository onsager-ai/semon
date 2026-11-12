use super::*;
use crate::credentials::{Authorization, Decision, Refusal, Retiring, Secret};
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "semon-enrollment-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn member(&self, name: &str) -> PathBuf {
        self.0.join(DIRECTORY).join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Clock(u64);
impl Clock {
    fn now(&self) -> u64 {
        self.0
    }
}
fn id(c: char) -> String {
    c.to_string().repeat(64)
}
fn pin() -> Pin {
    Pin::Spki([7; 32])
}
fn enrollment(kind: Kind) -> Enrollment {
    Enrollment {
        enrollment_id: id('a'),
        receiver_id: "receiver-1".into(),
        machine: "laptop".into(),
        sequence: 1,
        origin_key: vec![4; 65],
        endpoint: "https://receiver.example:443".into(),
        binding: if kind == Kind::MirrorPush {
            Binding::Inapplicable
        } else {
            Binding::Admitted {
                namespace: "asi-0123456789abcdef0123456789abcdef".into(),
                origin_incarnation: "origin-1".into(),
                index_incarnation: "index-1".into(),
                publisher_claim_epoch: ClaimEpoch::Unassigned,
                issuer: Issuer {
                    kind: IssuerKind::Receiver,
                    origin_trust_acknowledged: true,
                },
                assertion_issuer_keys: vec![IssuerKey {
                    key_id: "issuer-1".into(),
                    public_key: [4; 32],
                }],
            }
        },
        client_certificate_pins: vec![CertificatePin {
            pin: pin(),
            generation: 2,
            revoked_at: None,
        }],
        credential_kinds: vec![KindScope::new(kind, 2)],
        revoked_at: None,
    }
}
fn bearer(kind: Kind, secret: &Secret) -> Bearer {
    Bearer {
        credential_id: id('b'),
        enrollment_id: id('a'),
        kind,
        sha256: secret.hash(),
        generation: 2,
        valid_from: 10,
        valid_until: 100,
        revoked_at: None,
    }
}
fn registry(kind: Kind) -> (Registry, Secret) {
    let secret = Secret::new("private-test-bearer".into());
    (
        Registry::from_records(vec![enrollment(kind)], vec![bearer(kind, &secret)]).unwrap(),
        secret,
    )
}
fn refused(reason: Refusal) -> Authorization {
    Authorization::expected(Decision::Refused, Some(reason))
}
fn allowed(enrollment_id: String, kind: Kind, generation: u64) -> Authorization {
    Authorization::expected(
        Decision::Allowed {
            enrollment_id,
            kind,
            generation,
        },
        None,
    )
}
fn commit(store: &mut Store, next: &Registry) -> Result<()> {
    store.commit(store.snapshot()?.revision(), next)
}

#[test]
fn round_trip_restart_and_explicit_initialization() {
    let f = Fixture::new();
    assert!(matches!(Store::open(&f.0), Err(Error::Custody)));
    let mut store = Store::initialize(&f.0).unwrap();
    let (mut r, secret) = registry(Kind::BodyChannel);
    commit(&mut store, &r).unwrap();
    r.revision = 1;
    assert_eq!(store.snapshot().unwrap(), r);
    assert_eq!(
        fs::metadata(f.member(REGISTRY_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(f.0.join(DIRECTORY))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(
        !String::from_utf8(fs::read(f.member(REGISTRY_FILE)).unwrap())
            .unwrap()
            .contains(secret.expose())
    );
    drop(store);
    assert_eq!(Store::open(&f.0).unwrap().snapshot().unwrap(), r);
    assert!(Store::initialize(&f.0).is_err());
    fs::remove_file(f.member(REGISTRY_FILE)).unwrap();
    assert!(Store::open(&f.0).is_err());
}

#[test]
fn strict_document_refusals() {
    let (r, _) = registry(Kind::BodyChannel);
    let base = serde_json::to_value(&r).unwrap();
    let mutations: Vec<fn(&mut Value)> = vec![
        |v| v["unknown"] = json!(true),
        |v| v["format"] = json!(1),
        |v| v["enrollments"][0]["unknown"] = json!(true),
        |v| v["enrollments"][0]["credential_kinds"][0]["unknown"] = json!(true),
        |v| v["enrollments"][0]["credential_kinds"][0]["generations"]["unknown"] = json!(true),
        |v| v["enrollments"][0]["client_certificate_pins"][0]["unknown"] = json!(true),
        |v| v["enrollments"][0]["client_certificate_pins"][0]["pin"]["unknown"] = json!(true),
        |v| v["enrollments"][0]["binding"]["unknown"] = json!(true),
        |v| v["enrollments"][0]["binding"]["issuer"]["unknown"] = json!(true),
        |v| v["enrollments"][0]["binding"]["assertion_issuer_keys"][0]["unknown"] = json!(true),
        |v| v["credentials"][0]["unknown"] = json!(true),
        |v| v["enrollments"][0]["enrollment_id"] = json!("hostname"),
        |v| v["enrollments"][0]["receiver_id"] = json!(""),
        |v| v["enrollments"][0]["machine"] = json!("../laptop"),
        |v| v["enrollments"][0]["binding"]["namespace"] = json!("/tmp/path"),
        |v| v["enrollments"][0]["binding"]["origin_incarnation"] = json!(""),
        |v| v["enrollments"][0]["binding"]["index_incarnation"] = json!(""),
        |v| {
            v["enrollments"][0]["binding"]["publisher_claim_epoch"] =
                json!({"state":"assigned","epoch":0})
        },
        |v| {
            v["enrollments"][0]["binding"]["publisher_claim_epoch"] =
                json!({"state":"unassigned","epoch":42})
        },
        |v| v["enrollments"][0]["binding"]["issuer"]["origin_trust_acknowledged"] = json!(false),
        |v| v["enrollments"][0]["binding"]["assertion_issuer_keys"] = json!([]),
        |v| {
            let key = v["enrollments"][0]["binding"]["assertion_issuer_keys"][0].clone();
            v["enrollments"][0]["binding"]["assertion_issuer_keys"] = json!([key, key, key]);
        },
        |v| v["enrollments"][0]["binding"] = json!({"state":"inapplicable"}),
        |v| v["enrollments"][0]["credential_kinds"] = json!([]),
        |v| v["enrollments"][0]["credential_kinds"][0]["client_cert_required"] = json!(false),
        |v| v["enrollments"][0]["credential_kinds"][0]["kind"] = json!("body_read"),
        |v| v["enrollments"][0]["credential_kinds"][0]["generations"]["current"] = json!(0),
        |v| {
            v["enrollments"][0]["credential_kinds"][0]["generations"]["retiring"] =
                json!({"generation":2,"cutover_at":50})
        },
        |v| v["enrollments"][0]["client_certificate_pins"] = json!([]),
        |v| v["enrollments"][0]["client_certificate_pins"][0]["pin"]["type"] = json!("unverified"),
        |v| v["credentials"][0]["enrollment_id"] = json!(id('f')),
        |v| v["credentials"][0]["credential_id"] = json!(""),
        |v| v["credentials"][0]["generation"] = json!(3),
        |v| v["credentials"][0]["valid_until"] = json!(10),
        |v| v["credentials"][0]["kind"] = json!("mirror_push"),
        |v| {
            let e = v["enrollments"][0].clone();
            v["enrollments"].as_array_mut().unwrap().push(e);
        },
        |v| {
            let b = v["credentials"][0].clone();
            v["credentials"].as_array_mut().unwrap().push(b);
        },
        |v| {
            let k = v["enrollments"][0]["credential_kinds"][0].clone();
            v["enrollments"][0]["credential_kinds"]
                .as_array_mut()
                .unwrap()
                .push(k);
        },
    ];
    for (i, mutate) in mutations.iter().enumerate() {
        let mut value = base.clone();
        mutate(&mut value);
        assert!(
            Registry::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
            "mutation {i}"
        );
    }
    for bytes in [
        b"".as_slice(),
        b"{}",
        b"{",
        b"{\"format\":1,\"format\":1,\"enrollments\":[],\"credentials\":[]}",
    ] {
        assert!(Registry::parse(bytes).is_err());
    }
    assert!(Registry::parse(&vec![b' '; CAP as usize + 1]).is_err());
}

#[test]
fn duplicate_active_bindings_and_cross_enrollment_pins_refuse() {
    let (r, _) = registry(Kind::AnalyticsPublish);
    let mut other = r.enrollments[0].clone();
    other.enrollment_id = id('c');
    assert!(Registry::from_records(vec![r.enrollments[0].clone(), other.clone()], vec![]).is_err());
    other.machine = "desktop".into();
    assert!(Registry::from_records(vec![r.enrollments[0].clone(), other.clone()], vec![]).is_err());
    if let Binding::Admitted { namespace, .. } = &mut other.binding {
        *namespace = "other".into();
    }
    assert!(Registry::from_records(vec![r.enrollments[0].clone(), other.clone()], vec![]).is_err());
    other.client_certificate_pins[0].pin = Pin::Leaf([9; 32]);
    assert!(Registry::from_records(vec![r.enrollments[0].clone(), other], vec![]).is_ok());
}

#[test]
fn write_faults_are_atomic_before_and_after_rename() {
    for stage in [
        Stage::Written,
        Stage::Synced,
        Stage::Renamed,
        Stage::DirectorySynced,
    ] {
        let f = Fixture::new();
        let mut store = Store::initialize(&f.0).unwrap();
        let (r, _) = registry(Kind::MirrorPush);
        let result = store.write(&r, |at| {
            if at == stage {
                Err(Error::Write)
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        let renamed = matches!(stage, Stage::Renamed | Stage::DirectorySynced);
        if renamed {
            assert_eq!(store.snapshot(), Err(Error::CommitUncertain));
        }
        drop(store);
        let after = Store::open(&f.0).unwrap().snapshot().unwrap();
        assert_eq!(after, if renamed { r } else { Registry::empty() });
        assert!(fs::read_dir(f.0.join(DIRECTORY)).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("stage-")
        }));
    }
}

#[test]
fn lock_exclusion_and_explicit_unlock_with_duplicated_descriptor() {
    let f = Fixture::new();
    let first = Store::initialize(&f.0).unwrap();
    let inherited = first._lock.0.try_clone().unwrap();
    assert!(matches!(Store::open(&f.0), Err(Error::InUse)));
    drop(first);
    let again = Store::open(&f.0).unwrap();
    assert!(inherited.metadata().is_ok());
    drop(again);
    // Failure after acquiring the lock also releases it explicitly.
    fs::write(f.member(REGISTRY_FILE), b"invalid").unwrap();
    assert!(matches!(Store::open(&f.0), Err(Error::Invalid)));
    fs::write(
        f.member(REGISTRY_FILE),
        serde_json::to_vec(&Registry::empty()).unwrap(),
    )
    .unwrap();
    assert!(Store::open(&f.0).is_ok());
}

#[test]
fn pairing_owner_explicit_unlock_and_external_registry_transactions() {
    let f = Fixture::new();
    let first = Store::initialize(&f.0)
        .unwrap()
        .into_pairing_owner()
        .unwrap();
    let inherited = first._lock.0.try_clone().unwrap();
    let mut external = Store::open(&f.0).unwrap();
    assert!(matches!(first.snapshot(), Err(Error::InUse)));
    external.commit(0, &Registry::empty()).unwrap();
    drop(external);
    assert_eq!(first.snapshot().unwrap().revision(), 1);
    assert!(matches!(
        Store::open(&f.0).unwrap().into_pairing_owner(),
        Err(Error::InUse)
    ));
    drop(first);
    assert!(inherited.metadata().is_ok());
    let mut again = Store::open(&f.0).unwrap().into_pairing_owner().unwrap();
    // A stale expected revision re-reads under the lock and keeps the update.
    again
        .pairing_transaction(0, |snapshot| {
            assert_eq!(snapshot.revision(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(again.snapshot().unwrap().revision(), 2);
}

#[test]
fn insecure_or_missing_custody_never_appears_empty() {
    for member in [REGISTRY_FILE, LOCK_FILE] {
        let f = Fixture::new();
        drop(Store::initialize(&f.0).unwrap());
        fs::set_permissions(f.member(member), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(Store::open(&f.0), Err(Error::Custody)));
    }
    for member in [REGISTRY_FILE, LOCK_FILE] {
        let f = Fixture::new();
        drop(Store::initialize(&f.0).unwrap());
        let saved = f.0.join("saved");
        fs::rename(f.member(member), &saved).unwrap();
        symlink(&saved, f.member(member)).unwrap();
        assert!(matches!(Store::open(&f.0), Err(Error::Custody)));
        fs::remove_file(f.member(member)).unwrap();
        fs::hard_link(&saved, f.member(member)).unwrap();
        assert!(matches!(Store::open(&f.0), Err(Error::Custody)));
    }
    let f = Fixture::new();
    drop(Store::initialize(&f.0).unwrap());
    fs::set_permissions(f.0.join(DIRECTORY), fs::Permissions::from_mode(0o750)).unwrap();
    assert!(Store::open(&f.0).is_err());
    fs::set_permissions(f.0.join(DIRECTORY), fs::Permissions::from_mode(0o700)).unwrap();
    let saved = f.0.join("saved");
    fs::rename(f.0.join(DIRECTORY), &saved).unwrap();
    symlink(&saved, f.0.join(DIRECTORY)).unwrap();
    assert!(Store::open(&f.0).is_err());
}

#[test]
fn revocation_history_and_generations_are_terminal_through_real_commit() {
    let f = Fixture::new();
    let mut store = Store::initialize(&f.0).unwrap();
    let (mut r, secret) = registry(Kind::MirrorPush);
    commit(&mut store, &r).unwrap();
    r.credentials[0].revoked_at = Some(20);
    commit(&mut store, &r).unwrap();
    let mut cleared = r.clone();
    cleared.credentials[0].revoked_at = None;
    assert_eq!(commit(&mut store, &cleared), Err(Error::Terminal));
    r.enrollments[0].credential_kinds[0].available = false;
    commit(&mut store, &r).unwrap();
    cleared = r.clone();
    cleared.enrollments[0].credential_kinds[0].available = true;
    assert_eq!(commit(&mut store, &cleared), Err(Error::Terminal));
    r.enrollments[0].revoked_at = Some(22);
    commit(&mut store, &r).unwrap();
    cleared = r.clone();
    cleared.enrollments[0].revoked_at = None;
    assert_eq!(commit(&mut store, &cleared), Err(Error::Terminal));
    assert_eq!(commit(&mut store, &Registry::empty()), Err(Error::Terminal));
    assert_eq!(
        store.snapshot().unwrap().authorize(
            Kind::MirrorPush,
            Kind::MirrorPush,
            &secret,
            None,
            Clock(25).now()
        ),
        refused(Refusal::Revoked)
    );
    r.revision = store.snapshot().unwrap().revision();
    drop(store);
    assert_eq!(Store::open(&f.0).unwrap().snapshot().unwrap(), r);
}

#[test]
fn kind_route_certificate_and_validity_matrix() {
    let kinds = [Kind::MirrorPush, Kind::AnalyticsPublish, Kind::BodyChannel];
    let wrong = Secret::new("unknown-secret".into());
    for stored_kind in kinds {
        let (r, secret) = registry(stored_kind);
        for kind in kinds {
            for route in kinds {
                let result = r.authorize(kind, route, &secret, Some(&pin()), Clock(20).now());
                if kind == stored_kind && route == stored_kind {
                    assert!(matches!(result.decision, Decision::Allowed { .. }));
                } else {
                    assert_eq!(result, refused(Refusal::InvalidCredential));
                }
                assert_eq!(
                    r.authorize(kind, route, &wrong, Some(&pin()), 20),
                    refused(Refusal::InvalidCredential)
                );
            }
        }
        assert_eq!(
            r.authorize(stored_kind, stored_kind, &secret, None, 20),
            if stored_kind == Kind::BodyChannel {
                refused(Refusal::ClientCertificate)
            } else {
                allowed(id('a'), stored_kind, 2)
            }
        );
        for cert in [Pin::Spki([8; 32]), Pin::Leaf([7; 32])] {
            assert_eq!(
                r.authorize(stored_kind, stored_kind, &secret, Some(&cert), 20),
                refused(Refusal::ClientCertificate)
            );
        }
        for now in [0, 9, 100, 101] {
            assert_eq!(
                r.authorize(stored_kind, stored_kind, &secret, Some(&pin()), now),
                refused(Refusal::OutsideValidity)
            );
        }
        for now in [10, 99] {
            assert!(matches!(
                r.authorize(stored_kind, stored_kind, &secret, Some(&pin()), now),
                Authorization {
                    decision: Decision::Allowed { .. },
                    ..
                }
            ));
        }
        let mut required = r.clone();
        required.enrollments[0].credential_kinds[0].client_cert_required = true;
        assert_eq!(
            required.authorize(stored_kind, stored_kind, &secret, None, 20),
            refused(Refusal::ClientCertificate)
        );
        for level in 0..3 {
            let mut revoked = r.clone();
            match level {
                0 => revoked.enrollments[0].revoked_at = Some(99),
                1 => revoked.enrollments[0].credential_kinds[0].available = false,
                _ => revoked.credentials[0].revoked_at = Some(99),
            }
            assert_eq!(
                revoked.authorize(stored_kind, stored_kind, &secret, Some(&pin()), 20),
                refused(Refusal::Revoked)
            );
        }
    }
}

#[test]
fn per_kind_revocation_does_not_revoke_other_kinds_and_whole_overrides() {
    let (mut r, analytics) = registry(Kind::AnalyticsPublish);
    let body = Secret::new("body-secret".into());
    let mut b = bearer(Kind::BodyChannel, &body);
    b.credential_id = id('c');
    r.credentials.push(b);
    r.enrollments[0]
        .credential_kinds
        .push(KindScope::new(Kind::BodyChannel, 2));
    r.enrollments[0].credential_kinds[0].available = false;
    r.validate().unwrap();
    assert_eq!(
        r.authorize(
            Kind::AnalyticsPublish,
            Kind::AnalyticsPublish,
            &analytics,
            None,
            20
        ),
        refused(Refusal::Revoked)
    );
    assert!(matches!(
        r.authorize(
            Kind::BodyChannel,
            Kind::BodyChannel,
            &body,
            Some(&pin()),
            20
        ),
        Authorization {
            decision: Decision::Allowed { .. },
            ..
        }
    ));
    r.enrollments[0].revoked_at = Some(16);
    assert_eq!(
        r.authorize(
            Kind::BodyChannel,
            Kind::BodyChannel,
            &body,
            Some(&pin()),
            20
        ),
        refused(Refusal::Revoked)
    );
}

#[test]
fn overlap_cutover_and_same_enrollment_generation_certificate_binding() {
    let (mut r, current) = registry(Kind::BodyChannel);
    let old = Secret::new("previous-secret".into());
    let mut b = bearer(Kind::BodyChannel, &old);
    b.credential_id = id('c');
    b.generation = 1;
    r.credentials.push(b);
    r.enrollments[0].credential_kinds[0].generations.retiring = Some(Retiring {
        generation: 1,
        cutover_at: 50,
    });
    let old_pin = Pin::Leaf([3; 32]);
    r.enrollments[0]
        .client_certificate_pins
        .push(CertificatePin {
            pin: old_pin.clone(),
            generation: 1,
            revoked_at: None,
        });
    r.validate().unwrap();
    assert!(matches!(
        r.authorize(
            Kind::BodyChannel,
            Kind::BodyChannel,
            &old,
            Some(&old_pin),
            49
        ),
        Authorization {
            decision: Decision::Allowed { generation: 1, .. },
            ..
        }
    ));
    assert_eq!(
        r.authorize(
            Kind::BodyChannel,
            Kind::BodyChannel,
            &old,
            Some(&old_pin),
            50
        ),
        refused(Refusal::RetiredGeneration)
    );
    assert_eq!(
        r.authorize(
            Kind::BodyChannel,
            Kind::BodyChannel,
            &current,
            Some(&old_pin),
            49
        ),
        refused(Refusal::ClientCertificate)
    );
    assert_eq!(
        r.authorize(Kind::BodyChannel, Kind::BodyChannel, &old, Some(&pin()), 49),
        refused(Refusal::ClientCertificate)
    );
    let mut other = enrollment(Kind::BodyChannel);
    other.enrollment_id = id('d');
    other.machine = "desktop".into();
    if let Binding::Admitted { namespace, .. } = &mut other.binding {
        *namespace = "other".into();
    }
    other.client_certificate_pins[0].pin = Pin::Spki([9; 32]);
    r.enrollments.push(other);
    r.validate().unwrap();
    assert_eq!(
        r.authorize(
            Kind::BodyChannel,
            Kind::BodyChannel,
            &current,
            Some(&Pin::Spki([9; 32])),
            20
        ),
        refused(Refusal::ClientCertificate)
    );
}

#[test]
fn legacy_state_is_refused_without_opening_or_changing_bytes() {
    let f = Fixture::new();
    let path = f.0.join("tokens");
    let bytes = b"laptop secret-hash\n\0arbitrary";
    fs::write(&path, bytes).unwrap();
    assert_eq!(legacy_state(true), Err(Error::LegacyTokens));
    assert_eq!(refuse_legacy_tokens(&f.0), Err(Error::LegacyTokens));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::remove_file(&path).unwrap();
    symlink("missing", &path).unwrap();
    assert_eq!(refuse_legacy_tokens(&f.0), Err(Error::LegacyTokens));
    assert!(Error::LegacyTokens.to_string().contains("re-issue"));
}

#[test]
fn secrets_redact_all_formatting_and_registry_only_holds_hashes() {
    let (r, secret) = registry(Kind::MirrorPush);
    for text in [
        format!("{secret}"),
        format!("{secret:?}"),
        format!("{r:?}"),
        serde_json::to_string(&r).unwrap(),
        Error::Invalid.to_string(),
    ] {
        assert!(!text.contains(secret.expose()));
    }
    assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
    assert_ne!(random_id().unwrap(), random_id().unwrap());
}

#[test]
fn stale_snapshots_cannot_restore_replaced_pins_or_other_authority() {
    let f = Fixture::new();
    let mut store = Store::initialize(&f.0).unwrap();
    let (r, secret) = registry(Kind::BodyChannel);
    store.commit(0, &r).unwrap();
    let snapshot_a = store.snapshot().unwrap();
    drop(store);
    // A distinct transaction replaces P with Q and revokes the old bearer.
    let mut store = Store::open(&f.0).unwrap();
    let mut replacement = store.snapshot().unwrap();
    replacement.enrollments[0].client_certificate_pins[0].revoked_at = Some(20);
    let q = Pin::Spki([8; 32]);
    replacement.enrollments[0]
        .client_certificate_pins
        .push(CertificatePin {
            pin: q.clone(),
            generation: 2,
            revoked_at: None,
        });
    store.commit(replacement.revision(), &replacement).unwrap();
    let current = store.snapshot().unwrap();
    assert_eq!(
        current.authorize(
            Kind::BodyChannel,
            Kind::BodyChannel,
            &secret,
            Some(&pin()),
            25
        ),
        refused(Refusal::ClientCertificate)
    );
    assert!(matches!(
        current
            .authorize(Kind::BodyChannel, Kind::BodyChannel, &secret, Some(&q), 25)
            .decision,
        Decision::Allowed { .. }
    ));
    // The exact stale P->Q regression: unrelated enrollment addition cannot
    // restore P/remove Q, even though all the old generation checks would pass.
    let mut stale = snapshot_a.clone();
    let mut other = enrollment(Kind::MirrorPush);
    other.enrollment_id = id('d');
    other.machine = "desktop".into();
    other.client_certificate_pins.clear();
    stale.enrollments.push(other);
    assert_eq!(
        store.commit(snapshot_a.revision(), &stale),
        Err(Error::StaleSnapshot)
    );
    // Even a caller supplying the latest revision cannot clear pin tombstones.
    assert_eq!(
        store.commit(current.revision(), &stale),
        Err(Error::Terminal)
    );
    let mut restored = current.clone();
    restored.enrollments[0].client_certificate_pins[0].revoked_at = None;
    assert_eq!(
        store.commit(current.revision(), &restored),
        Err(Error::Terminal)
    );
    let mut revoked = current.clone();
    revoked.credentials[0].revoked_at = Some(26);
    store.commit(current.revision(), &revoked).unwrap();
    let after_revocation = store.snapshot().unwrap();
    assert_eq!(
        store.commit(current.revision(), &current),
        Err(Error::StaleSnapshot)
    );
    assert_eq!(
        store.commit(after_revocation.revision(), &current),
        Err(Error::Terminal)
    );
    // Retirement cannot be undone by an old generation snapshot.
    let mut retired = after_revocation.clone();
    retired.enrollments[0].credential_kinds[0]
        .generations
        .current = 3;
    retired.enrollments[0]
        .client_certificate_pins
        .push(CertificatePin {
            pin: Pin::Leaf([9; 32]),
            generation: 3,
            revoked_at: None,
        });
    store.commit(after_revocation.revision(), &retired).unwrap();
    let after_retirement = store.snapshot().unwrap();
    assert_eq!(
        store.commit(after_revocation.revision(), &after_revocation),
        Err(Error::StaleSnapshot)
    );
    assert_eq!(
        store.commit(after_retirement.revision(), &after_revocation),
        Err(Error::Terminal)
    );
    assert_eq!(store.snapshot().unwrap(), after_retirement);
}

#[test]
fn all_external_refusals_are_identical_with_local_diagnostics() {
    let (r, secret) = registry(Kind::BodyChannel);
    let unknown = Secret::new("unknown".into());
    let mut revoked = r.clone();
    revoked.credentials[0].revoked_at = Some(20);
    let mut retired = r.clone();
    retired.credentials[0].generation = 1;
    let cases = [
        (
            r.authorize(
                Kind::BodyChannel,
                Kind::BodyChannel,
                &unknown,
                Some(&pin()),
                25,
            ),
            Refusal::InvalidCredential,
        ),
        (
            r.authorize(
                Kind::MirrorPush,
                Kind::MirrorPush,
                &secret,
                Some(&pin()),
                25,
            ),
            Refusal::InvalidCredential,
        ),
        (
            revoked.authorize(
                Kind::BodyChannel,
                Kind::BodyChannel,
                &secret,
                Some(&pin()),
                25,
            ),
            Refusal::Revoked,
        ),
        (
            retired.authorize(
                Kind::BodyChannel,
                Kind::BodyChannel,
                &secret,
                Some(&pin()),
                25,
            ),
            Refusal::RetiredGeneration,
        ),
        (
            r.authorize(
                Kind::BodyChannel,
                Kind::BodyChannel,
                &secret,
                Some(&pin()),
                100,
            ),
            Refusal::OutsideValidity,
        ),
        (
            r.authorize(Kind::BodyChannel, Kind::BodyChannel, &secret, None, 25),
            Refusal::ClientCertificate,
        ),
    ];
    let debug = format!("{:?}", cases[0].0);
    let pretty = format!("{:#?}", cases[0].0);
    for (result, diagnostic) in cases {
        assert_eq!(result.decision, Decision::Refused);
        assert_eq!(result.local_diagnostic(), Some(diagnostic));
        assert_eq!(format!("{result:?}"), debug);
        assert_eq!(format!("{result:#?}"), pretty);
        for name in [
            "InvalidCredential",
            "Revoked",
            "RetiredGeneration",
            "OutsideValidity",
            "ClientCertificate",
        ] {
            assert!(!debug.contains(name));
            assert!(!pretty.contains(name));
            assert!(!format!("{:?}", result.local_diagnostic()).contains(name));
        }
    }
}

#[test]
fn revision_overflow_refuses_without_changing_custody() {
    let f = Fixture::new();
    let mut store = Store::initialize(&f.0).unwrap();
    let mut current = Registry::empty();
    current.revision = u64::MAX;
    // Seed the boundary through the existing private persistence test path.
    store.write(&current, |_| Ok(())).unwrap();
    let before = fs::read(f.member(REGISTRY_FILE)).unwrap();
    assert_eq!(store.commit(u64::MAX, &current), Err(Error::Invalid));
    assert_eq!(fs::read(f.member(REGISTRY_FILE)).unwrap(), before);
    assert_eq!(store.snapshot().unwrap(), current);
}

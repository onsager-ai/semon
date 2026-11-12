use super::tests::{Fixture, id, registry};
use super::*;
use crate::credentials::{Decision, Kind, Secret};
use std::{fs, os::unix::fs::DirBuilderExt};

fn audit(f: &Fixture) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(f.0.join("receiver-audit"))
        .unwrap();
}
#[test]
fn maximum_machine_tombstone_fits_and_never_authenticates() {
    let t = Tombstone {
        enrollment_id: "f".repeat(64),
        machine: "m".repeat(63),
        sequence: u64::MAX,
        pin: Some(Pin::Spki([255; 32])),
        high_water: [u64::MAX; 3],
    };
    assert!(semon_sessions::is_machine_name(&t.machine));
    assert!(serde_json::to_vec(&t).unwrap().len() < 512);
    let mut r = Registry::empty();
    r.tombstones.push(t.clone());
    assert!(matches!(
        r.owner_state(&t.machine, None, OwnerAction::Revoke, 1),
        OwnerState::Complete { .. }
    ));
    assert_eq!(
        r.authorize(
            Kind::MirrorPush,
            Kind::MirrorPush,
            &Secret::new("unknown".into()),
            None,
            1
        )
        .decision,
        Decision::Refused
    );
    let (mut next, _) = registry(Kind::MirrorPush);
    next.tombstones.push(t.clone());
    next.enrollments[0].enrollment_id = t.enrollment_id;
    assert!(next.validate().is_err());
}
#[test]
fn old_format_refuses_without_repair() {
    let f = Fixture::new();
    let store = Store::initialize(&f.0).unwrap();
    drop(store);
    let path = f.0.join(DIRECTORY).join(REGISTRY_FILE);
    let bytes =
        br#"{"format":1,"revision":0,"enrollments":[],"credentials":[],"pairing_receipts":[]}"#;
    fs::write(&path, bytes).unwrap();
    assert!(matches!(Store::open(&f.0), Err(Error::UnsupportedVersion)));
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(Error::UnsupportedVersion.to_string(), UNSUPPORTED_FORMAT);
}
#[test]
fn revoke_is_terminal_and_rerun_reports_state() {
    let f = Fixture::new();
    let mut store = Store::initialize(&f.0).unwrap();
    let (r, secret) = registry(Kind::MirrorPush);
    store.commit(0, &r).unwrap();
    assert_eq!(
        store.snapshot().unwrap().owner_state(
            "laptop",
            Some(Kind::MirrorPush),
            OwnerAction::Revoke,
            20
        ),
        OwnerState::LastKind
    );
    store
        .owner_update(&f.0, None, OwnerAction::Revoke, "owner", &id('a'), 20)
        .unwrap();
    let ended = store.snapshot().unwrap();
    assert_eq!(
        ended
            .authorize(Kind::MirrorPush, Kind::MirrorPush, &secret, None, 20)
            .decision,
        Decision::Refused
    );
    assert!(matches!(
        ended.owner_state("laptop", None, OwnerAction::Revoke, 20),
        OwnerState::Complete { .. }
    ));
    assert!(matches!(
        ended.owner_state("laptop", Some(Kind::MirrorPush), OwnerAction::Enable, 20),
        OwnerState::NoAuthority { .. }
    ));
    assert_eq!(store.snapshot().unwrap().revision(), ended.revision());
}
#[test]
fn audit_before_compaction_restart_and_no_resurrection() {
    let f = Fixture::new();
    audit(&f);
    let mut store = Store::initialize(&f.0).unwrap();
    let (r, secret) = registry(Kind::MirrorPush);
    store.commit(0, &r).unwrap();
    store
        .owner_update(&f.0, None, OwnerAction::Revoke, "owner", &id('a'), 20)
        .unwrap();
    // Audit unavailable leaves the full ended evidence intact.
    fs::rename(f.0.join("receiver-audit"), f.0.join("held-audit")).unwrap();
    assert_eq!(store.compact(&f.0, 20), Err(Error::AuditUnavailable));
    assert_eq!(store.snapshot().unwrap().credentials.len(), 1);
    fs::rename(f.0.join("held-audit"), f.0.join("receiver-audit")).unwrap();
    store.compact(&f.0, 20).unwrap();
    let current = store.snapshot().unwrap();
    assert!(current.credentials.is_empty());
    assert!(current.enrollments.is_empty());
    assert_eq!(
        current.tombstones[0].high_water[Kind::MirrorPush.index()],
        2
    );
    let evidence = fs::read_to_string(f.0.join("receiver-audit/generations.jsonl")).unwrap();
    assert!(evidence.contains("credential_generation_ended"));
    assert!(!evidence.contains(&hex::encode(secret.hash())));
    let revision = current.revision();
    store.compact(&f.0, 10).unwrap();
    assert_eq!(store.snapshot().unwrap().revision(), revision);
    drop(store);
    let current = Store::open(&f.0).unwrap().snapshot().unwrap();
    assert_eq!(
        current
            .authorize(Kind::MirrorPush, Kind::MirrorPush, &secret, None, 10)
            .decision,
        Decision::Refused
    );
}
#[test]
fn first_use_cutover_is_atomic_and_contended_request_refuses() {
    let f = Fixture::new();
    let mut store = Store::initialize(&f.0).unwrap();
    let (mut r, secret) = registry(Kind::MirrorPush);
    let mut old = r.credentials[0].clone();
    old.generation = 1;
    old.credential_id = id('c');
    old.sha256 = Secret::new("old".into()).hash();
    r.credentials.push(old);
    r.enrollments[0].credential_kinds[0].generations.retiring =
        Some(crate::credentials::Retiring {
            generation: 1,
            cutover_at: 90,
        });
    store.commit(0, &r).unwrap();
    assert!(matches!(
        Store::authorize_cutover(&f.0, Kind::MirrorPush, &secret, None, 20),
        Err(Error::InUse)
    ));
    assert!(
        store.snapshot().unwrap().enrollments[0].credential_kinds[0]
            .generations
            .retiring
            .is_some()
    );
    drop(store);
    let committed = Store::authorize_cutover(&f.0, Kind::MirrorPush, &secret, None, 20).unwrap();
    assert!(
        committed.enrollments[0].credential_kinds[0]
            .generations
            .retiring
            .is_none()
    );
    assert_eq!(
        committed
            .authorize(
                Kind::MirrorPush,
                Kind::MirrorPush,
                &Secret::new("old".into()),
                None,
                20
            )
            .decision,
        Decision::Refused
    );
    let again = Store::authorize_cutover(&f.0, Kind::MirrorPush, &secret, None, 20).unwrap();
    assert_eq!(again.revision(), committed.revision());
}

#[test]
fn enable_is_one_use_pending_and_revoke_cancels_it() {
    let f = Fixture::new();
    audit(&f);
    let mut store = Store::initialize(&f.0).unwrap();
    let (mut r, _) = registry(Kind::AnalyticsPublish);
    let mut second = r.credentials[0].clone();
    second.kind = Kind::MirrorPush;
    second.credential_id = id('c');
    second.sha256 = Secret::new("mirror".into()).hash();
    r.credentials.push(second);
    r.enrollments[0]
        .credential_kinds
        .push(KindScope::new(Kind::MirrorPush, 2));
    store.commit(0, &r).unwrap();
    store
        .owner_update(
            &f.0,
            Some(Kind::MirrorPush),
            OwnerAction::Revoke,
            "owner",
            &id('a'),
            20,
        )
        .unwrap();
    store
        .owner_update(
            &f.0,
            Some(Kind::MirrorPush),
            OwnerAction::Enable,
            "owner",
            &id('a'),
            20,
        )
        .unwrap();
    let pending = store.snapshot().unwrap();
    let auth = pending.enrollments[0].credential_kinds[1]
        .enable_authorization
        .clone()
        .unwrap();
    assert_eq!(auth.enrollment_id, id('a'));
    assert_eq!(auth.owner, "owner");
    assert_eq!(auth.consumed_renewal_id, None);
    assert!(matches!(
        pending.owner_state("laptop", Some(Kind::MirrorPush), OwnerAction::Enable, 20),
        OwnerState::Complete { .. }
    ));
    assert_eq!(store.snapshot().unwrap().revision(), pending.revision());
    store
        .owner_update(
            &f.0,
            Some(Kind::MirrorPush),
            OwnerAction::Revoke,
            "owner",
            &id('a'),
            21,
        )
        .unwrap();
    assert!(
        store.snapshot().unwrap().enrollments[0]
            .credential_kinds
            .iter()
            .all(|s| s.enable_authorization.is_none())
    );
}

#[test]
fn replacement_requires_a_fresh_confirmation() {
    let f = Fixture::new();
    let mut store = Store::initialize(&f.0).unwrap();
    let (r, _) = registry(Kind::MirrorPush);
    store.commit(0, &r).unwrap();
    let mut next = store.snapshot().unwrap();
    next.revoke_enrollment(&id('a'), None, 20);
    let mut replacement = r.enrollments[0].clone();
    replacement.enrollment_id = id('c');
    replacement.sequence = 2;
    replacement.client_certificate_pins.clear();
    next.enrollments.push(replacement);
    next.next_sequence = 3;
    let mut bearer = r.credentials[0].clone();
    bearer.credential_id = id('d');
    bearer.enrollment_id = id('c');
    bearer.sha256 = Secret::new("replacement".into()).hash();
    next.credentials.push(bearer);
    store.commit(next.revision(), &next).unwrap();
    let current = store.snapshot().unwrap();
    assert!(
        matches!(current.owner_state("laptop", None, OwnerAction::Revoke, 20), OwnerState::Confirm { enrollment_id, .. } if enrollment_id == id('c'))
    );
    assert_eq!(
        store.owner_update(&f.0, None, OwnerAction::Revoke, "owner", &id('a'), 20),
        Err(Error::StaleSnapshot)
    );
    assert_eq!(store.snapshot().unwrap(), current);
}

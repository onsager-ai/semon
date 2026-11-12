use super::*;
use crate::{
    body_assertion as assertion,
    body_cache::{BodyCache, Clock, Limits},
    receiver_acl::{Acls, Decision, Permission},
    receiver_audit::AuditStore,
};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "receiver-custody-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn open(&self) -> ReceiverCustody {
        ReceiverCustody::open_or_initialize(&self.0, "https://receiver.example:443").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct FixedClock;
impl Clock for FixedClock {
    fn now(&self) -> u64 {
        100
    }
}
fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}
fn id(n: u8) -> String {
    hex::encode([n; 32])
}

#[test]
fn first_run_reuses_identity_secrets_and_storage_outputs() {
    let f = Fixture::new();
    let custody = f.open();
    let issuer_path =
        f.0.join(SECRETS)
            .join(issuer_filename(&custody.issuer.key_id));
    let seed = fs::read(&issuer_path).unwrap();
    let cache_key = fs::read(custody.cache_key_path()).unwrap();
    assert_eq!(seed.len(), 32);
    assert_eq!(cache_key.len(), 32);
    assert_ne!(seed, cache_key);
    for directory in [
        SECRETS,
        CACHE_SECRETS,
        CACHE,
        AUDIT,
        ACL,
        enrollment::DIRECTORY,
    ] {
        assert_eq!(mode(&f.0.join(directory)), 0o700);
    }
    for file in [
        f.0.join(IDENTITY),
        issuer_path.clone(),
        custody.cache_key_path(),
        custody.acl_dir().join("owners.json"),
        custody.acl_dir().join(receiver_acl::ACL_FILE),
    ] {
        assert_eq!(mode(&file), 0o600);
    }
    assert_ne!(
        custody.cache_dir().parent(),
        custody.cache_key_path().parent()
    );
    assert_ne!(issuer_path.parent(), custody.cache_key_path().parent());
    assert_ne!(issuer_path.parent(), Some(custody.cache_dir().as_path()));
    assert_ne!(
        custody.cache_key_path().parent(),
        Some(custody.cache_dir().as_path())
    );
    let second = f.open();
    assert_eq!(custody.receiver_id(), second.receiver_id());
    assert_eq!(custody.owner(), second.owner());
    assert_eq!(custody.issuer(), second.issuer());
    assert_eq!(fs::read(issuer_path).unwrap(), seed);
    assert_eq!(fs::read(custody.cache_key_path()).unwrap(), cache_key);
    assert!(
        BodyCache::open(
            &custody.cache_dir(),
            &custody.cache_key_path(),
            FixedClock,
            Limits::default()
        )
        .is_ok()
    );
    assert!(AuditStore::open(&custody.audit_dir(), FixedClock, 1024 * 1024).is_ok());
    assert_eq!(ReceiverCustody::status(&f.0).identity, Piece::Valid);
    assert!(!format!("{custody:?}").contains(&hex::encode(seed)));
}

#[test]
fn client_ca_and_trust_are_generated_once_and_modified_trust_refuses() {
    let f = Fixture::new();
    let first = ReceiverCustody::open_at(&f.0, 1_800_000_000).unwrap();
    let key = fs::read(f.0.join(CLIENT_CA_SECRETS).join(CLIENT_CA_KEY)).unwrap();
    let cert = fs::read(f.0.join(CLIENT_CA_CERTIFICATE)).unwrap();
    let trust = fs::read(f.0.join(CLIENT_TRUST)).unwrap();
    assert_eq!(trust, crate::client_ca::pem(&cert));
    assert_eq!(mode(&f.0.join(CLIENT_CA_SECRETS)), 0o700);
    for name in [
        CLIENT_CA_CERTIFICATE,
        CLIENT_TRUST,
        "client-ca-secrets/client-ca.pk8",
    ] {
        assert_eq!(mode(&f.0.join(name)), 0o600);
    }
    let second = ReceiverCustody::open_at(&f.0, 1_800_000_100).unwrap();
    assert_eq!(first.receiver_id(), second.receiver_id());
    assert_eq!(
        fs::read(f.0.join(CLIENT_CA_SECRETS).join(CLIENT_CA_KEY)).unwrap(),
        key
    );
    assert_eq!(fs::read(f.0.join(CLIENT_CA_CERTIFICATE)).unwrap(), cert);
    fs::write(f.0.join(CLIENT_TRUST), b"hand edited").unwrap();
    assert_eq!(
        ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").unwrap_err(),
        Error::Invalid
    );
    assert_eq!(fs::read(f.0.join(CLIENT_TRUST)).unwrap(), b"hand edited");
    assert_eq!(
        fs::read(f.0.join(CLIENT_CA_SECRETS).join(CLIENT_CA_KEY)).unwrap(),
        key
    );
}
#[test]
fn old_receiver_refuses_without_deleting_or_repairing() {
    let f = Fixture::new();
    fs::write(f.0.join(IDENTITY), b"old receiver identity").unwrap();
    fs::set_permissions(f.0.join(IDENTITY), fs::Permissions::from_mode(0o600)).unwrap();
    let error =
        ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").unwrap_err();
    assert_eq!(
        error.to_string(),
        "receiver custody predates client certificates; initialize a fresh receiver directory"
    );
    assert_eq!(
        fs::read(f.0.join(IDENTITY)).unwrap(),
        b"old receiver identity"
    );
    assert!(!f.0.join(CLIENT_CA_SECRETS).exists());
}

#[test]
fn generated_key_signs_and_verifies_pinned_assertion() {
    let f = Fixture::new();
    let custody = f.open();
    let key = custody.load_signing_key().unwrap();
    let audience = assertion::Audience {
        origin_incarnation: "origin".into(),
        namespace: "work".into(),
    };
    let claims = assertion::Claims {
        issuer_key_id: custody.issuer.key_id.clone(),
        issued_at: 100,
        expiry: 120,
        audience: audience.clone(),
        claim_epoch: 1,
        nonce: [1; 32],
        subject_principal: custody.owner.principal.clone(),
        machine: "laptop".into(),
        namespace: "work".into(),
        acl_grant_version: "grant".into(),
        receiver_policy_version: "policy".into(),
        request_digest: [2; 32],
        deadline: 120,
        channel_binding: [3; 32],
    };
    let signed = assertion::sign(claims, &key, || 100).unwrap();
    let expected = assertion::Expected {
        audience: &audience,
        channel_binding: &[3; 32],
        deadline: 120,
        skew: 0,
    };
    assert!(
        assertion::verify(
            &signed,
            |id| (id == custody.issuer.key_id).then_some(custody.issuer.public_key),
            &expected,
            &mut assertion::ReplayCache::new(4),
            || 100
        )
        .is_ok()
    );
}

#[test]
fn corrupted_or_missing_pieces_never_regenerate_secrets() {
    for target in [
        IDENTITY,
        "cache-secrets/cache-master.key",
        "acl/owners.json",
        "acl/grants.json",
        "enrollment/registry.json",
    ] {
        let f = Fixture::new();
        let custody = f.open();
        let seed_path =
            f.0.join(SECRETS)
                .join(issuer_filename(&custody.issuer.key_id));
        let seed = fs::read(&seed_path).unwrap();
        fs::write(f.0.join(target), b"broken").unwrap();
        assert!(
            ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err(),
            "{target}"
        );
        assert_eq!(fs::read(&seed_path).unwrap(), seed);
    }
    for value in [
        serde_json::json!({"format":2,"receiver_id":id(1),"issuer_key_id":"00".repeat(16)}),
        serde_json::json!({"format":1,"receiver_id":id(1),"issuer_key_id":"00".repeat(16),"extra":true}),
    ] {
        let f = Fixture::new();
        f.open();
        fs::write(f.0.join(IDENTITY), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err());
    }
    let f = Fixture::new();
    let custody = f.open();
    fs::remove_file(custody.cache_key_path()).unwrap();
    assert!(ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err());
    assert!(!custody.cache_key_path().exists());
}

#[test]
fn modes_links_and_short_keys_refuse() {
    for target in [
        IDENTITY,
        SECRETS,
        CACHE,
        ACL,
        AUDIT,
        "cache-secrets/cache-master.key",
        "acl/grants.json",
    ] {
        let f = Fixture::new();
        f.open();
        let path = f.0.join(target);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err(),
            "{target}"
        );
    }
    for target in [
        IDENTITY,
        SECRETS,
        CACHE,
        ACL,
        AUDIT,
        "cache-secrets/cache-master.key",
    ] {
        let f = Fixture::new();
        f.open();
        let path = f.0.join(target);
        let moved = f.0.join("moved");
        fs::rename(&path, &moved).unwrap();
        symlink(&moved, &path).unwrap();
        assert!(
            ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err(),
            "{target}"
        );
    }
    for length in [0, 31, 33] {
        let f = Fixture::new();
        let custody = f.open();
        let path =
            f.0.join(SECRETS)
                .join(issuer_filename(&custody.issuer.key_id));
        fs::write(&path, vec![7; length]).unwrap();
        assert!(ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err());
        assert!(custody.load_signing_key().is_err());
    }
}

#[test]
fn crash_before_identity_refuses_without_replacing_keys() {
    let f = Fixture::new();
    assert!(matches!(
        ReceiverCustody::open_with_fault(&f.0, || Err(Error::Write)),
        Err(Error::Write)
    ));
    assert!(!f.0.join(IDENTITY).exists());
    let key = fs::read(f.0.join(CACHE_SECRETS).join(CACHE_KEY)).unwrap();
    assert!(matches!(
        ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443"),
        Err(Error::Incomplete)
    ));
    assert_eq!(
        fs::read(f.0.join(CACHE_SECRETS).join(CACHE_KEY)).unwrap(),
        key
    );
}

#[test]
fn owner_grants_are_pending_exact_and_revocable_per_enrollment() {
    let f = Fixture::new();
    let custody = f.open();
    let grants = owner_grants(custody.owner(), "laptop", "work", false).unwrap();
    let policy = receiver_acl::Policy::parse(&grants_document(&grants).unwrap()).unwrap();
    assert!(matches!(
        policy.decide(
            &custody.owner.principal,
            "laptop",
            "work",
            Permission::Analytics
        ),
        Decision::Granted(_)
    ));
    for (principal, machine, namespace, permission) in [
        ("stranger", "laptop", "work", Permission::Analytics),
        (
            &custody.owner.principal,
            "other",
            "work",
            Permission::Analytics,
        ),
        (
            &custody.owner.principal,
            "laptop",
            "other",
            Permission::Analytics,
        ),
        (
            &custody.owner.principal,
            "laptop",
            "work",
            Permission::BodyRead,
        ),
    ] {
        assert_eq!(
            policy.decide(principal, machine, namespace, permission),
            Decision::Denied
        );
    }
    let acls = Acls::new(custody.acl_dir());
    let decide = || {
        acls.decide(
            &custody.owner.principal,
            "laptop",
            "work",
            Permission::Analytics,
        )
    };
    assert_eq!(decide(), Decision::Denied);
    custody.install_owner_grants(&id(1), &grants).unwrap();
    custody.install_owner_grants(&id(1), &grants).unwrap();
    assert!(matches!(decide(), Decision::Granted(_)));
    assert_eq!(
        custody.install_owner_grants(&id(2), &grants),
        Err(Error::Conflict)
    );
    let wider = owner_grants(custody.owner(), "laptop", "work", true).unwrap();
    assert_eq!(
        custody.install_owner_grants(&id(1), &wider),
        Err(Error::Conflict)
    );
    let other = owner_grants(custody.owner(), "desktop", "personal", true).unwrap();
    custody.install_owner_grants(&id(2), &other).unwrap();
    custody.retire_owner_grants(&id(1)).unwrap();
    custody.retire_owner_grants(&id(1)).unwrap();
    assert_eq!(decide(), Decision::Denied);
    assert!(matches!(
        acls.decide(
            &custody.owner.principal,
            "desktop",
            "personal",
            Permission::BodyRead
        ),
        Decision::Granted(_)
    ));
    assert!(owner_grants(custody.owner(), "*", "work", true).is_err());
    let mut forged = other.clone();
    forged[0].principal = "pairing-participant".into();
    assert!(custody.install_owner_grants(&id(3), &forged).is_err());
    fs::write(custody.acl_dir().join(receiver_acl::ACL_FILE), b"malformed").unwrap();
    assert!(custody.retire_owner_grants(&id(2)).is_err());
    assert_eq!(
        fs::read(custody.acl_dir().join(receiver_acl::ACL_FILE)).unwrap(),
        b"malformed"
    );
}

#[test]
fn real_acl_transaction_unlocks_even_with_a_duplicated_descriptor() {
    let f = Fixture::new();
    let custody = f.open();
    let root = Directory::open(&f.0).unwrap();
    let acl = root.child(ACL).unwrap();
    let guard = acl.lock("owners.lock").unwrap();
    let inherited = guard.0.try_clone().unwrap();
    let grants = owner_grants(custody.owner(), "laptop", "work", false).unwrap();
    assert_eq!(
        custody.install_owner_grants(&id(1), &grants),
        Err(Error::InUse)
    );
    drop(guard);
    custody.install_owner_grants(&id(1), &grants).unwrap();
    drop(inherited);
}

#[test]
fn durable_redo_completes_acl_installation() {
    let f = Fixture::new();
    let custody = f.open();
    let acl = Directory::open(&custody.acl_dir()).unwrap();
    let grants = owner_grants(custody.owner(), "laptop", "work", true).unwrap();
    let ledger = GrantLedger {
        format: 1,
        enrollments: BTreeMap::from([(id(1), grants.clone())]),
        pending: Some(PolicyDocument { format: 1, grants }),
    };
    acl.publish("owners.json", &encode(&ledger).unwrap(), true)
        .unwrap();
    f.open();
    assert!(matches!(
        Acls::new(custody.acl_dir()).decide(
            &custody.owner.principal,
            "laptop",
            "work",
            Permission::BodyRead
        ),
        Decision::Granted(_)
    ));
    assert!(load_ledger(&acl).unwrap().pending.is_none());
}

#[test]
fn ownership_hardlinks_and_unknown_secret_files_refuse() {
    let f = Fixture::new();
    let custody = f.open();
    fs::hard_link(custody.cache_key_path(), f.0.join("extra-link")).unwrap();
    assert!(ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err());
    let f = Fixture::new();
    f.open();
    fs::write(f.0.join(SECRETS).join("unknown"), b"unknown").unwrap();
    assert!(ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err());
    // A different owner can be exercised when the runner is privileged.
    // SAFETY: geteuid has no arguments.
    if unsafe { libc::geteuid() } == 0 {
        use std::os::unix::ffi::OsStrExt;
        let f = Fixture::new();
        f.open();
        let path = CString::new(f.0.as_os_str().as_bytes()).unwrap();
        // SAFETY: live NUL-terminated pathname and scalar UID/GID.
        assert_eq!(unsafe { libc::chown(path.as_ptr(), 1, 0) }, 0);
        assert!(ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example:443").is_err());
        // SAFETY: restore our fixture ownership before cleanup.
        assert_eq!(unsafe { libc::chown(path.as_ptr(), 0, 0) }, 0);
    }
}

#[test]
fn directory_swap_cannot_split_file_and_directory_operations() {
    let f = Fixture::new();
    let original = f.0.join("original");
    let selected = f.0.join("selected");
    fs::DirBuilder::new().mode(0o700).create(&selected).unwrap();
    let dir = Directory::open_after_fd(&selected, || {
        fs::rename(&selected, &original).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&selected).unwrap();
    })
    .unwrap();
    dir.create("marker", b"private").unwrap();
    dir.create_child("child")
        .unwrap()
        .create("nested", b"private")
        .unwrap();
    assert_eq!(fs::read(original.join("marker")).unwrap(), b"private");
    assert_eq!(fs::read(original.join("child/nested")).unwrap(), b"private");
    assert!(fs::read_dir(&selected).unwrap().next().is_none());
}

#[test]
fn initialization_keeps_registry_with_locked_root_after_path_swap() {
    let f = Fixture::new();
    let selected = f.0.join("selected");
    let original = f.0.join("original");
    fs::DirBuilder::new().mode(0o700).create(&selected).unwrap();
    let custody = ReceiverCustody::open_with_hooks(
        &selected,
        || Ok(()),
        |_| {
            fs::rename(&selected, &original).unwrap();
            fs::DirBuilder::new().mode(0o700).create(&selected).unwrap();
        },
    )
    .unwrap();
    // Initialization and its final registry read must both use the locked inode.
    assert!(
        original
            .join(enrollment::DIRECTORY)
            .join(enrollment::REGISTRY_FILE)
            .is_file()
    );
    assert!(original.join(IDENTITY).is_file());
    assert!(fs::read_dir(&selected).unwrap().next().is_none());
    let reopened =
        ReceiverCustody::open_or_initialize(&original, "https://receiver.example:443").unwrap();
    assert_eq!(custody.receiver_id(), reopened.receiver_id());
    assert_eq!(custody.issuer(), reopened.issuer());
}

#[test]
fn initialization_explicitly_unlocks_with_inherited_descriptor() {
    let f = Fixture::new();
    let mut inherited = None;
    ReceiverCustody::open_with_hooks(
        &f.0,
        || Ok(()),
        |file| {
            inherited = Some(file.try_clone().unwrap());
        },
    )
    .unwrap();
    // A duplicated descriptor shares flock ownership, just like fork inheritance.
    // Keep it alive while the real initialization path acquires the lock again.
    f.open();
    drop(inherited);
}

#[test]
fn pending_retirement_recovers_before_and_after_acl_publication() {
    for already_published in [false, true] {
        let f = Fixture::new();
        let custody = f.open();
        let removed = owner_grants(custody.owner(), "laptop", "work", true).unwrap();
        let retained = owner_grants(custody.owner(), "desktop", "personal", false).unwrap();
        custody.install_owner_grants(&id(1), &removed).unwrap();
        custody.install_owner_grants(&id(2), &retained).unwrap();
        let acl = Directory::open(&custody.acl_dir()).unwrap();
        let mut ledger = load_ledger(&acl).unwrap();
        ledger.enrollments.remove(&id(1));
        ledger.pending = Some(PolicyDocument {
            format: 1,
            grants: retained,
        });
        acl.publish("owners.json", &encode(&ledger).unwrap(), true)
            .unwrap();
        if already_published {
            acl.publish(
                receiver_acl::ACL_FILE,
                &encode(ledger.pending.as_ref().unwrap()).unwrap(),
                true,
            )
            .unwrap();
        }
        f.open();
        let acls = Acls::new(custody.acl_dir());
        assert_eq!(
            acls.decide(
                &custody.owner.principal,
                "laptop",
                "work",
                Permission::Analytics
            ),
            Decision::Denied
        );
        assert!(matches!(
            acls.decide(
                &custody.owner.principal,
                "desktop",
                "personal",
                Permission::Analytics
            ),
            Decision::Granted(_)
        ));
        let ledger = load_ledger(&acl).unwrap();
        assert!(ledger.pending.is_none());
        assert!(!ledger.enrollments.contains_key(&id(1)));
        custody.retire_owner_grants(&id(1)).unwrap();
    }
}

#[test]
fn host_observations_never_claim_swap_or_backup_safety() {
    for observation in [Some(0), Some(1), None] {
        let status = host_custody(observation);
        assert_eq!(status.swap, Observation::Unknown);
        assert_eq!(status.backup_exclusions, Observation::Unknown);
    }
    assert_eq!(host_custody(Some(0)).core_dumps, Observation::Satisfied);
    assert_eq!(host_custody(Some(1)).core_dumps, Observation::Failed);
    assert_eq!(host_custody(None).core_dumps, Observation::Unknown);
}

#[test]
fn revoke_between_activation_and_install_never_restores_grants() {
    use crate::{
        credentials::Kind,
        enrollment::{
            Binding, ClaimEpoch, Enrollment, Issuer, IssuerKind, KindScope, Registry, Store,
        },
    };
    let fixture = Fixture::new();
    let custody = fixture.open();
    let enrollment = Enrollment {
        enrollment_id: id(1),
        receiver_id: custody.receiver_id().into(),
        machine: "laptop".into(),
        sequence: 1,
        origin_key: vec![4; 65],
        endpoint: "https://receiver.example:443".into(),
        enrolled_pin: None,
        binding: Binding::Admitted {
            namespace: "asi-0123456789abcdef0123456789abcdef".into(),
            origin_incarnation: "origin-1".into(),
            index_incarnation: "index-1".into(),
            publisher_claim_epoch: ClaimEpoch::Unassigned,
            issuer: Issuer {
                kind: IssuerKind::Receiver,
                origin_trust_acknowledged: true,
            },
            assertion_issuer_keys: vec![custody.issuer().clone()],
        },
        client_certificate_pins: vec![],
        credential_kinds: vec![KindScope::new(Kind::AnalyticsPublish, 1)],
        revoked_at: None,
    };
    let mut store = Store::open(&fixture.0).unwrap();
    let active = Registry::from_records(vec![enrollment], vec![]).unwrap();
    store
        .commit(store.snapshot().unwrap().revision(), &active)
        .unwrap();
    drop(store);
    let grants = owner_grants(
        custody.owner(),
        "laptop",
        "asi-0123456789abcdef0123456789abcdef",
        false,
    )
    .unwrap();
    // Deterministic interleaving: activation committed, revoker commits and retires,
    // then the delayed activation caller attempts installation.
    let mut store = Store::open(&fixture.0).unwrap();
    let mut revoked = store.snapshot().unwrap();
    revoked.enrollments[0].revoked_at = Some(100);
    store.commit(revoked.revision(), &revoked).unwrap();
    drop(store);
    custody.retire_owner_grants(&id(1)).unwrap();
    let before = fs::read(custody.acl_dir().join(receiver_acl::ACL_FILE)).unwrap();
    assert!(
        custody
            .install_current_owner_grants(&id(1), &grants)
            .is_err()
    );
    assert_eq!(
        fs::read(custody.acl_dir().join(receiver_acl::ACL_FILE)).unwrap(),
        before
    );
}

#[test]
fn recorded_endpoint_is_normalized_and_retained() {
    let f = Fixture::new();
    ReceiverCustody::open_or_initialize(&f.0, "https://receiver.example").unwrap();
    assert_eq!(
        enrollment::Registry::read_current(&f.0).unwrap().endpoint(),
        Some("https://receiver.example:443")
    );
    let before = fs::read(f.0.join(IDENTITY)).unwrap();
    ReceiverCustody::open_or_initialize(&f.0, "https://different.example").unwrap();
    assert_eq!(
        enrollment::Registry::read_current(&f.0).unwrap().endpoint(),
        Some("https://receiver.example:443")
    );
    assert_eq!(fs::read(f.0.join(IDENTITY)).unwrap(), before);
    assert!(
        crate::provision::Controller::open(&f.0, "https://different.example", "laptop".into())
            .err()
            .unwrap()
            .contains("Receiver directory records endpoint https://receiver.example:443.")
    );
}

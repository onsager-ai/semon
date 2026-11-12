use super::*;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "semon-pairing-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let mut store = Store::initialize(&path).unwrap();
        Core::initialize_gate(&mut store, 100).unwrap();
        Self(path)
    }
    fn core(&self) -> Core {
        Core::open(
            Store::open(&self.0).unwrap(),
            "https://desk.example",
            "receiver-1".into(),
        )
        .unwrap()
    }
    fn gate(&self) -> PathBuf {
        self.0.join(enrollment::DIRECTORY).join(GATE_FILE)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn input() -> ActivationInput {
    ActivationInput {
        machine: "laptop".into(),
        binding: Binding::Inapplicable,
        kinds: vec![(Kind::MirrorPush, false)],
        client_certificate_pins: vec![],
        valid_until: 10_000,
    }
}
fn transcript(created: &Created) -> Transcript {
    Transcript {
        version: FORMAT,
        session_id: created.session_id.clone(),
        endpoint: "https://desk.example:443".into(),
        exporter: vec![2; 32],
        origin_key: vec![3; 32],
        scope: input().commitment(),
        origin_nonce: [4; 32],
        receiver_nonce: created.receiver_nonce,
        origin_role: "origin".into(),
        receiver_role: "receiver".into(),
        profile: "public_tls".into(),
    }
}
fn evidence(t: &Transcript) -> VerifiedApproval {
    VerifiedApproval {
        session_id: t.session_id.clone(),
        origin_key: t.origin_key.clone(),
        transcript_digest: t.digest(),
        summary_digest: t.scope,
    }
}
fn confirmed(core: &mut Core, binding: &str, now: u64) -> (Transcript, ConfirmedKey) {
    let created = core.create_session(binding.into(), now).unwrap();
    let t = transcript(&created);
    let keys = derive_keys(&created.code, &t).unwrap();
    let origin = keys.origin_confirmation(&t);
    let attempt = core.reserve(binding, t.clone(), now).unwrap();
    let verified = attempt.verify(&origin).unwrap();
    let receiver = core.confirm(verified, now).unwrap();
    let key = keys.confirm_receiver(&t, &receiver).unwrap();
    (t, key)
}
fn burn(core: &mut Core, binding: &str, now: u64) {
    let c = core.create_session(binding.into(), now).unwrap();
    let t = transcript(&c);
    for _ in 0..3 {
        drop(core.reserve(binding, t.clone(), now).unwrap());
    }
    assert!(matches!(
        core.reserve(binding, t.clone(), now),
        Err(Error::Terminal)
    ));
    assert_eq!(core.state(&t.session_id, now).unwrap(), State::Rejected);
    assert!(core.sessions[&t.session_id].code.is_none());
}

#[test]
fn code_shape_normalization_and_invalid_inputs() {
    for _ in 0..64 {
        let code = generate_code().unwrap();
        assert_eq!(code.expose().len(), 10);
        assert!(code.expose().bytes().all(|b| ALPHABET.contains(&b)));
        assert_eq!(parse_code(code.expose()).unwrap().expose(), code.expose());
        let formatted = display_code(&code);
        assert_eq!(formatted.expose().len(), 11);
        assert_eq!(
            parse_code(&format!(" \t{}\n", formatted.expose().to_lowercase()))
                .unwrap()
                .expose(),
            code.expose()
        );
    }
    assert_eq!(parse_code("oooooooooo").unwrap().expose(), "0000000000");
    let with_one = parse_code("1000000001").unwrap();
    for alias in ["I00000000L", "l00000000i"] {
        assert_eq!(parse_code(alias).unwrap().expose(), with_one.expose());
    }
    for value in [
        "",
        "000000000",
        "00000000000",
        "0000-000000",
        "00000--00000",
        "00000 00000",
        "U000000000",
        "000000000U",
        "0000000002",
        "é000000000",
        "00000\n0000",
        "00000-00000-",
        "0000000000x",
    ] {
        assert!(parse_code(value).is_err(), "{value:?}");
    }
}
#[test]
fn check_catches_every_single_symbol_substitution_and_claimed_transpositions() {
    // All odd weights are invertible modulo 32: every single-symbol change,
    // including the check character, changes the checksum. Distinct data weights
    // differ by even numbers: some transpositions deliberately remain undetected.
    for seed in 0..32u8 {
        let data: Vec<u8> = (0..9).map(|i| (seed + i) & 31).collect();
        let mut code: Vec<u8> = data.iter().map(|v| ALPHABET[*v as usize]).collect();
        code.push(ALPHABET[check(&data) as usize]);
        for position in 0..10 {
            for replacement in ALPHABET {
                if code[position] == *replacement {
                    continue;
                }
                let mut changed = code.clone();
                changed[position] = *replacement;
                assert!(parse_code(std::str::from_utf8(&changed).unwrap()).is_err());
            }
        }
    }
    let mut detected = 0;
    let mut total = 0;
    for a in 0..32u8 {
        for b in 0..32u8 {
            if a == b {
                continue;
            }
            let mut values = [0; 9];
            values[0] = a;
            values[1] = b;
            let sum = check(&values);
            values.swap(0, 1);
            detected += usize::from(check(&values) != sum);
            total += 1;
        }
    }
    assert_eq!((detected, total), (960, 992)); // 30/31 of adjacent unequal pairs
    let mut values = [0; 9];
    values[0] = 16;
    let before = check(&values);
    values.swap(0, 1);
    assert_eq!(before, check(&values));
}
#[test]
fn attempt_budget_binding_disconnect_and_timeout() {
    let f = Fixture::new();
    let mut core = f.core();
    let c = core.create_session("connection".into(), 100).unwrap();
    let t = transcript(&c);
    assert!(core.reserve("foreign", t.clone(), 100).is_err());
    assert_eq!(core.sessions[&c.session_id].attempts, 0);
    let unknown = Transcript {
        session_id: "f".repeat(64),
        ..t.clone()
    };
    assert!(core.reserve("connection", unknown, 100).is_err());
    // Dropped tickets represent disconnect/timeouts; no refund.
    for _ in 0..3 {
        drop(core.reserve("connection", t.clone(), 100).unwrap());
    }
    assert!(core.reserve("connection", t, 100).is_err());
    assert_eq!(core.state(&c.session_id, 100).unwrap(), State::Rejected);
}
#[test]
fn concurrent_reservations_never_overshoot_and_burn_invalidates_outstanding_tickets() {
    let f = Fixture::new();
    let core = std::sync::Mutex::new(f.core());
    let c = core
        .lock()
        .unwrap()
        .create_session("c".into(), 100)
        .unwrap();
    let t = transcript(&c);
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let core = &core;
                let t = t.clone();
                scope.spawn(move || core.lock().unwrap().reserve("c", t, 100))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 3);
    let mut core = core.into_inner().unwrap();
    assert_eq!(core.state(&c.session_id, 100).unwrap(), State::Rejected);
    let keys = derive_keys(&c.code, &t).unwrap();
    let proof = keys.origin_confirmation(&t);
    let ticket = outcomes.into_iter().find_map(Result::ok).unwrap();
    assert!(core.confirm(ticket.verify(&proof).unwrap(), 100).is_err());
}
#[test]
fn exact_redeem_approve_absolute_windows_and_backward_time() {
    let f = Fixture::new();
    let mut core = f.core();
    let c = core.create_session("a".into(), 100).unwrap();
    assert_eq!(core.state(&c.session_id, 699).unwrap(), State::Pending);
    assert_eq!(core.state(&c.session_id, 1).unwrap(), State::Pending);
    assert_eq!(core.state(&c.session_id, 700).unwrap(), State::Expired);
    let (t, _) = confirmed(&mut core, "b", 700);
    assert_eq!(core.state(&t.session_id, 1299).unwrap(), State::Confirmed);
    assert!(core.approve(evidence(&t), 1300).is_err());
    assert_eq!(core.state(&t.session_id, 1300).unwrap(), State::Expired);
    // Confirmation at the last redeem second still cannot exceed absolute life.
    let c = core.create_session("c".into(), 1300).unwrap();
    let t = transcript(&c);
    let keys = derive_keys(&c.code, &t).unwrap();
    let v = core
        .reserve("c", t.clone(), 1899)
        .unwrap()
        .verify(&keys.origin_confirmation(&t))
        .unwrap();
    core.confirm(v, 1899).unwrap();
    core.approve(evidence(&t), 2498).unwrap();
    assert_eq!(core.state(&t.session_id, 2499).unwrap(), State::Approved);
    assert_eq!(core.state(&t.session_id, 2500).unwrap(), State::Expired);
    // Independently exercise the absolute branch, including an approved state.
    let c = core.create_session("d".into(), 2500).unwrap();
    let session = core.sessions.get_mut(&c.session_id).unwrap();
    session.confirmed = Some(3200);
    session.state = State::Approved;
    assert_eq!(core.state(&c.session_id, 3699).unwrap(), State::Approved);
    assert_eq!(core.state(&c.session_id, 3700).unwrap(), State::Expired);
}
#[test]
fn gate_cap_issuance_pause_restart_and_existing_attempts() {
    let f = Fixture::new();
    let mut core = f.core();
    let existing = core.create_session("existing".into(), 100).unwrap();
    let existing_t = transcript(&existing);
    burn(&mut core, "a", 100);
    burn(&mut core, "b", 100);
    burn(&mut core, "c", 100);
    assert_eq!(core.gate.pause_until, 1900);
    assert!(core.create_session("blocked".into(), 100).is_err());
    assert!(core.reserve("existing", existing_t, 100).is_ok());
    drop(core);
    let mut core = f.core();
    assert!(core.create_session("restart".into(), 1).is_err());
    assert!(Core::initialize_gate(&mut core.store, 1).is_err());
    assert!(core.create_session("before".into(), 1899).is_err());
    let a = core.create_session("after".into(), 1900).unwrap();
    core.cancel(&a.session_id, 1900).unwrap();
    let b = core.create_session("six".into(), 1900).unwrap();
    core.cancel(&b.session_id, 1900).unwrap();
    assert!(core.create_session("seven".into(), 1900).is_err());
    // Rolling hour removes the four entries at 100 exactly at 3700.
    assert!(core.create_session("next-hour".into(), 3700).is_ok());
    let f2 = Fixture::new();
    let mut core2 = f2.core();
    for i in 0..4 {
        core2.create_session(format!("cap-{i}"), 100).unwrap();
    }
    assert!(core2.create_session("cap-5".into(), 100).is_err());
    assert!(core2.create_session("after-expiry".into(), 700).is_ok());
}
#[test]
fn rolling_creation_bound_with_pending_and_burn_pause_interplay() {
    let f = Fixture::new();
    let mut core = f.core();
    let mut sessions: Vec<_> = (0..4)
        .map(|i| core.create_session(format!("initial-{i}"), 100).unwrap())
        .collect();
    let mut guesses = 0;
    for i in 0..6 {
        let t = transcript(&sessions[i]);
        let binding = core.sessions[&t.session_id].binding.clone();
        for _ in 0..3 {
            drop(core.reserve(&binding, t.clone(), 100).unwrap());
            guesses += 1;
        }
        assert!(core.reserve(&binding, t, 100).is_err());
        if i < 2 {
            sessions.push(
                core.create_session(format!("replacement-{i}"), 100)
                    .unwrap(),
            );
        }
        assert!(guesses <= 18);
    }
    assert_eq!(guesses, 18);
    assert_eq!(core.gate.creations.len(), 6);
    assert!(core.create_session("seventh".into(), 100).is_err());
    // Even after pause expires, the rolling creation limiter binds.
    assert!(core.create_session("still-seventh".into(), 1900).is_err());
    assert!(core.create_session("new-hour".into(), 3700).is_ok());
}
#[test]
fn corrupt_missing_unsupported_oversized_gate_and_custody_refuse() {
    for bytes in [
        b"".as_slice(),
        b"{",
        b"{}",
        b"{\"format\":2,\"latest\":100,\"creations\":[],\"burns\":[],\"pause_until\":0}",
        b"{\"format\":1,\"latest\":100,\"creations\":[101],\"burns\":[],\"pause_until\":0}",
    ] {
        let f = Fixture::new();
        fs::write(f.gate(), bytes).unwrap();
        assert!(
            Core::open(
                Store::open(&f.0).unwrap(),
                "https://desk.example",
                "r".into()
            )
            .is_err()
        );
    }
    let f = Fixture::new();
    let mut core = f.core();
    fs::write(f.gate(), b"bad").unwrap();
    assert!(core.create_session("a".into(), 100).is_err());
    drop(core);
    fs::write(f.gate(), vec![b' '; GATE_CAP as usize + 1]).unwrap();
    assert!(
        Core::open(
            Store::open(&f.0).unwrap(),
            "https://desk.example",
            "r".into()
        )
        .is_err()
    );
    fs::remove_file(f.gate()).unwrap();
    assert!(
        Core::open(
            Store::open(&f.0).unwrap(),
            "https://desk.example",
            "r".into()
        )
        .is_err()
    );
    for mode in [0o644, 0o640] {
        let f = Fixture::new();
        fs::set_permissions(f.gate(), fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            Core::open(
                Store::open(&f.0).unwrap(),
                "https://desk.example",
                "r".into()
            )
            .is_err()
        );
    }
    let f = Fixture::new();
    assert_eq!(
        fs::metadata(f.gate()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let saved = f.0.join("saved");
    fs::rename(f.gate(), &saved).unwrap();
    symlink(&saved, f.gate()).unwrap();
    assert!(
        Core::open(
            Store::open(&f.0).unwrap(),
            "https://desk.example",
            "r".into()
        )
        .is_err()
    );
    fs::remove_file(f.gate()).unwrap();
    fs::hard_link(&saved, f.gate()).unwrap();
    assert!(
        Core::open(
            Store::open(&f.0).unwrap(),
            "https://desk.example",
            "r".into()
        )
        .is_err()
    );
}
#[test]
fn wrong_code_and_every_transcript_field_mismatch_fail_without_receiver_mac() {
    let f = Fixture::new();
    let mut core = f.core();
    let c = core.create_session("c".into(), 100).unwrap();
    let t = transcript(&c);
    let key = derive_keys(&c.code, &t).unwrap();
    let valid = key.origin_confirmation(&t);
    let mut changed = t.clone();
    changed.scope[0] ^= 1;
    let ticket = core.reserve("c", changed, 100).unwrap();
    assert!(ticket.verify(&valid).is_err());
    assert_eq!(core.state(&c.session_id, 100).unwrap(), State::Pending);
    let wrong_code = generate_code().unwrap();
    let wrong = derive_keys(&wrong_code, &t)
        .unwrap()
        .origin_confirmation(&t);
    assert!(
        core.reserve("c", t.clone(), 100)
            .unwrap()
            .verify(&wrong)
            .is_err()
    );
    let mutations: Vec<fn(&mut Transcript)> = vec![
        |t| t.version += 1,
        |t| t.session_id.push('f'),
        |t| t.endpoint = "https://other.example:443".into(),
        |t| t.exporter[0] ^= 1,
        |t| t.origin_key[0] ^= 1,
        |t| t.scope[0] ^= 1,
        |t| t.origin_nonce[0] ^= 1,
        |t| t.receiver_nonce[0] ^= 1,
        |t| t.origin_role = "receiver".into(),
        |t| t.receiver_role = "origin".into(),
        |t| t.profile = "pinned_invitation".into(),
    ];
    for mutate in mutations {
        let mut changed = t.clone();
        mutate(&mut changed);
        assert!(verify(key.origin.as_ref(), &changed, &valid).is_err());
        assert_ne!(changed.digest(), t.digest());
    }
    assert_ne!(key.origin.as_ref(), key.receiver.as_ref());
    assert_ne!(key.origin.as_ref(), key.application.as_ref());
    let receiver = mac(key.receiver.as_ref(), &t);
    assert!(verify(key.origin.as_ref(), &t, &receiver).is_err());
    let mut other = t.clone();
    other.origin_nonce[0] ^= 1;
    assert_ne!(t.prefix(), other.prefix());
    assert_ne!(
        key.application.as_ref(),
        derive_keys(&c.code, &other).unwrap().application.as_ref()
    );
    assert!(key.confirm_receiver(&t, &valid).is_err());
}
#[test]
fn full_happy_path_and_interrupted_delivery_recovery_after_restart() {
    let f = Fixture::new();
    let mut core = f.core();
    let (t, key) = confirmed(&mut core, "c", 100);
    assert!(core.activate(&t.session_id, input(), 100).is_err());
    core.approve(evidence(&t), 100).unwrap();
    let id = core
        .commit_activation(&t.session_id, input(), 100, false)
        .unwrap()
        .enrollment_id;
    // Crash point after commit, before delivery; pending sessions are gone.
    drop(core);
    let mut core = f.core();
    assert!(core.sessions.is_empty());
    let first = core.deliver(&key, evidence(&t), 101).unwrap();
    assert_eq!(first.enrollment_id, id);
    let first_secret = first.credentials[0].1.expose().to_string();
    drop(first);
    drop(core);
    let mut core = f.core();
    let recovery = core.deliver(&key, evidence(&t), 102).unwrap();
    assert_eq!(recovery.enrollment_id, id);
    assert_eq!(recovery.credentials[0].1.expose(), first_secret);
    assert!(core.deliver(&key, evidence(&t), 103).is_err());
    assert_eq!(core.store.snapshot().unwrap().enrollments().len(), 1);
    let bytes = fs::read(
        f.0.join(enrollment::DIRECTORY)
            .join(enrollment::REGISTRY_FILE),
    )
    .unwrap();
    assert!(!String::from_utf8(bytes).unwrap().contains(&first_secret));
}
#[test]
fn cancellation_expiry_replay_competing_keys_and_duplicate_approval() {
    let f = Fixture::new();
    let mut core = f.core();
    let c = core.create_session("c".into(), 100).unwrap();
    let t = transcript(&c);
    let a = core.reserve("c", t.clone(), 100).unwrap();
    let mut other = t.clone();
    other.origin_key[0] ^= 1;
    let b = core.reserve("c", other.clone(), 100).unwrap();
    let key = derive_keys(&c.code, &t).unwrap();
    let other_key = derive_keys(&c.code, &other).unwrap();
    let va = a.verify(&key.origin_confirmation(&t)).unwrap();
    let vb = b.verify(&other_key.origin_confirmation(&other)).unwrap();
    core.confirm(va, 100).unwrap();
    assert!(core.confirm(vb, 100).is_err());
    assert!(core.approve(evidence(&other), 100).is_err());
    core.approve(evidence(&t), 100).unwrap();
    assert!(core.approve(evidence(&t), 100).is_err());
    core.cancel(&t.session_id, 100).unwrap();
    assert!(core.activate(&t.session_id, input(), 100).is_err());
    assert!(core.cancel(&t.session_id, 100).is_err());
    let c = core.create_session("d".into(), 100).unwrap();
    let t = transcript(&c);
    let k = derive_keys(&c.code, &t).unwrap();
    let verified = core
        .reserve("d", t.clone(), 100)
        .unwrap()
        .verify(&k.origin_confirmation(&t))
        .unwrap();
    assert!(core.confirm(verified, 700).is_err());
    assert!(core.store.snapshot().unwrap().enrollments().is_empty());
}
#[test]
fn activation_before_commit_failure_leaves_no_enrollment_and_scope_cannot_change() {
    let f = Fixture::new();
    let mut core = f.core();
    let (t, _) = confirmed(&mut core, "c", 100);
    core.approve(evidence(&t), 100).unwrap();
    let mut changed = input();
    changed.machine = "other".into();
    assert!(core.activate(&t.session_id, changed, 100).is_err());
    assert!(core.store.snapshot().unwrap().enrollments().is_empty());
    assert_eq!(core.state(&t.session_id, 100).unwrap(), State::Approved);
    core.activate(&t.session_id, input(), 100).unwrap();
    assert!(core.activate(&t.session_id, input(), 100).is_err());
}
#[test]
fn revoked_enrollment_never_reused_or_redelivered() {
    let f = Fixture::new();
    let mut core = f.core();
    let (t, key) = confirmed(&mut core, "c", 100);
    core.approve(evidence(&t), 100).unwrap();
    let id = core
        .activate(&t.session_id, input(), 100)
        .unwrap()
        .enrollment_id;
    let mut snapshot = core.store.snapshot().unwrap();
    snapshot.enrollments[0].revoked_at = Some(101);
    core.store.commit(snapshot.revision(), &snapshot).unwrap();
    assert!(core.deliver(&key, evidence(&t), 101).is_err());
    let (t2, _) = confirmed(&mut core, "d", 101);
    core.approve(evidence(&t2), 101).unwrap();
    let id2 = core
        .activate(&t2.session_id, input(), 101)
        .unwrap()
        .enrollment_id;
    assert_ne!(id, id2);
    assert!(
        core.store.snapshot().unwrap().enrollments()[0]
            .revoked_at
            .is_some()
    );
}
#[test]
fn per_kind_secrets_are_separate_and_fresh_snapshot_preserves_unrelated_updates() {
    let f = Fixture::new();
    let mut core = f.core();
    let before = core.store.snapshot().unwrap();
    let (t, key) = confirmed(&mut core, "c", 100);
    core.approve(evidence(&t), 100).unwrap();
    core.activate(&t.session_id, input(), 100).unwrap();
    assert_eq!(
        core.store.commit(before.revision(), &before),
        Err(Error::StaleSnapshot)
    );
    let delivery = core.deliver(&key, evidence(&t), 100).unwrap();
    let app = key.application.as_ref();
    let kinds = [Kind::MirrorPush, Kind::AnalyticsPublish, Kind::BodyChannel];
    let hashes: BTreeSet<_> = kinds
        .into_iter()
        .map(|k| bearer_secret(app, &delivery.enrollment_id, k).hash())
        .collect();
    assert_eq!(hashes.len(), 3);
    let snapshot = core.store.snapshot().unwrap();
    assert_eq!(snapshot.credentials().len(), 1);
    assert_eq!(
        snapshot.credentials()[0].sha256,
        delivery.credentials[0].1.hash()
    );
}
#[test]
fn debug_never_exposes_codes_keys_nonces_or_bearers() {
    let f = Fixture::new();
    let mut core = f.core();
    let c = core.create_session("c".into(), 100).unwrap();
    let t = transcript(&c);
    let keys = derive_keys(&c.code, &t).unwrap();
    let proof = keys.origin_confirmation(&t);
    let a = core.reserve("c", t.clone(), 100).unwrap();
    for text in [
        format!("{c:?}"),
        format!("{t:?}"),
        format!("{keys:?}"),
        format!("{proof:?}"),
        format!("{a:?}"),
        format!("{core:?}"),
        format!("{:?}", evidence(&t)),
        format!("{:?}", input()),
    ] {
        assert!(text.ends_with("(<redacted>)"));
        assert!(!text.contains(c.code.expose()));
    }
    let v = a.verify(&proof).unwrap();
    assert_eq!(format!("{v:?}"), "VerifiedAttempt(<redacted>)");
    let m = core.confirm(v, 100).unwrap();
    let key = keys.confirm_receiver(&t, &m).unwrap();
    assert_eq!(format!("{key:?}"), "ConfirmedKey(<redacted>)");
    core.approve(evidence(&t), 100).unwrap();
    core.activate(&t.session_id, input(), 100).unwrap();
    let delivery = core.deliver(&key, evidence(&t), 100).unwrap();
    assert_eq!(format!("{delivery:?}"), "Delivery(<redacted>)");
}
#[test]
fn pbkdf2_confirmation_derivation_cost() {
    let f = Fixture::new();
    let mut core = f.core();
    let c = core.create_session("cost".into(), 100).unwrap();
    let t = transcript(&c);
    let started = std::time::Instant::now();
    let _keys = derive_keys(&c.code, &t).unwrap();
    eprintln!(
        "PAIRING_KDF_COST_MS={} iterations={PBKDF2_ITERATIONS}",
        started.elapsed().as_millis()
    );
}

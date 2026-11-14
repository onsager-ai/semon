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
            },
            NOW,
        )
        .unwrap();
        r
    }
    fn receiver(&self, r: &Renewal) -> PathBuf {
        use crate::enrollment::{Enrollment, KindScope, Registry, Store};
        let path = self.0.join("test-receiver");
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path.join("receiver-audit"))
            .unwrap();
        let enrollment = Enrollment {
            enrollment_id: r.config.enrollment_id.clone(),
            receiver_id: r.config.receiver_id.clone(),
            machine: r.config.machine.clone(),
            sequence: 1,
            origin_key: r.key.public_key().as_ref().to_vec(),
            endpoint: ENDPOINT.into(),
            enrolled_pin: None,
            binding: Binding::Inapplicable,
            client_certificate_pins: vec![],
            credential_kinds: vec![KindScope::new(Kind::MirrorPush, 1)],
            revoked_at: None,
        };
        let bearer = crate::credentials::Bearer {
            credential_id: "e".repeat(64),
            enrollment_id: enrollment.enrollment_id.clone(),
            kind: Kind::MirrorPush,
            sha256: crate::credentials::Secret::new("old-bearer".into()).hash(),
            generation: 1,
            valid_from: NOW - 1,
            valid_until: r.config.scope.valid_until,
            revoked_at: None,
        };
        let registry = Registry::from_records(vec![enrollment], vec![bearer]).unwrap();
        let mut store = Store::initialize(&path).unwrap();
        let revision = store.snapshot().unwrap().revision();
        store.commit(revision, &registry).unwrap();
        path
    }
    fn receiver_delivery(&self, r: &mut Renewal) -> (PathBuf, Delivered) {
        let dir = self.receiver(r);
        let mut receiver = crate::renewal::Receiver::default();
        let begun = receiver
            .handle(
                &dir,
                wire::Request::Begin(wire::Begin {
                    version: 1,
                    enrollment_id: r.config.enrollment_id.clone(),
                    credentials: r.credentials(NOW).unwrap(),
                }),
                [1; 32],
                None,
                NOW,
            )
            .unwrap_or_else(|_| panic!("begin refused"));
        r.prepare(serde_json::from_slice(&begun).unwrap(), NOW)
            .unwrap();
        let finish = r.finish(&[2; 32], NOW).unwrap();
        let bytes = receiver
            .handle(&dir, wire::Request::Finish(finish), [2; 32], None, NOW)
            .unwrap_or_else(|_| panic!("finish refused"));
        (dir, serde_json::from_slice(&bytes).unwrap())
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
fn origin_proof_and_retry_match_receiver_commit_contract() {
    let f = Fixture::new();
    let mut origin = f.open();
    let receiver_dir = f.receiver(&origin);
    let mut receiver = crate::renewal::Receiver::default();
    let begin = wire::Begin {
        version: 1,
        enrollment_id: origin.config.enrollment_id.clone(),
        credentials: origin.credentials(NOW).unwrap(),
    };
    let begun = receiver
        .handle(
            &receiver_dir,
            wire::Request::Begin(begin),
            [1; 32],
            None,
            NOW,
        )
        .unwrap_or_else(|_| panic!("begin refused"));
    origin
        .prepare(serde_json::from_slice(&begun).unwrap(), NOW)
        .unwrap();
    let request = origin.finish(&[2; 32], NOW).unwrap();
    // Receiver commits, but the initial delivery is deliberately lost.
    receiver
        .handle(
            &receiver_dir,
            wire::Request::Finish(request),
            [2; 32],
            None,
            NOW,
        )
        .unwrap_or_else(|_| panic!("commit refused"));
    drop(origin);
    let mut origin = f.open();
    let request = origin.finish(&[3; 32], NOW).unwrap();
    let bytes = receiver
        .handle(
            &receiver_dir,
            wire::Request::Finish(request),
            [3; 32],
            None,
            NOW,
        )
        .unwrap_or_else(|_| panic!("recover refused"));
    let delivered: Delivered = serde_json::from_slice(&bytes).unwrap();
    origin.delivered(&delivered, NOW).unwrap();
    drop(origin);
    assert_eq!(load_at(&f.0, None, NOW).unwrap().0.generation, 2);
}

#[test]
fn revoked_receiver_refuses_recovery_without_origin_fallback() {
    let f = Fixture::new();
    let mut origin = f.open();
    let receiver_dir = f.receiver(&origin);
    let mut receiver = crate::renewal::Receiver::default();
    let begin = wire::Begin {
        version: 1,
        enrollment_id: origin.config.enrollment_id.clone(),
        credentials: origin.credentials(NOW).unwrap(),
    };
    let begun = receiver
        .handle(
            &receiver_dir,
            wire::Request::Begin(begin),
            [1; 32],
            None,
            NOW,
        )
        .unwrap_or_else(|_| panic!("begin refused"));
    origin
        .prepare(serde_json::from_slice(&begun).unwrap(), NOW)
        .unwrap();
    let request = origin.finish(&[2; 32], NOW).unwrap();
    receiver
        .handle(
            &receiver_dir,
            wire::Request::Finish(request),
            [2; 32],
            None,
            NOW,
        )
        .unwrap_or_else(|_| panic!("commit refused"));
    let mut store = crate::enrollment::Store::open(&receiver_dir).unwrap();
    let mut registry = store.snapshot().unwrap();
    let revision = registry.revision();
    registry.revoke_enrollment(&origin.config.enrollment_id, None, NOW);
    store.commit(revision, &registry).unwrap();
    drop(store);
    drop(origin);
    let mut origin = f.open();
    let request = origin.finish(&[3; 32], NOW).unwrap();
    assert!(
        receiver
            .handle(
                &receiver_dir,
                wire::Request::Finish(request),
                [3; 32],
                None,
                NOW
            )
            .is_err()
    );
    drop(origin);
    // A generic renewal refusal alone does not prove application revocation.
    assert!(load_at(&f.0, None, NOW).is_ok());
}

#[test]
fn pending_attempt_preserves_selected_application_authority() {
    let f = Fixture::new();
    let (_, credential) = load_at(&f.0, None, NOW).unwrap();
    let crate::Credential::Enrolled { authority, .. } = credential else {
        panic!()
    };
    let mut r = f.prepare();
    assert!(authority.is_active());
    assert_eq!(load_at(&f.0, None, NOW).unwrap().0.generation, 1);
    r.finish(&[1; 32], NOW).unwrap();
    assert!(authority.is_active());
    assert_eq!(load_at(&f.0, None, NOW).unwrap().0.generation, 1);
    r.finish(&[2; 32], NOW).unwrap();
    assert!(authority.is_active());
    assert!(load_at(&f.0, None, r.config.scope.valid_until).is_err());
}

#[test]
fn same_object_public_retry_reconciles_published_recovery_before_expiry() {
    let f = Fixture::new();
    let mut r = f.prepare();
    r.finish(&[1; 32], NOW).unwrap();
    r.finish(&[2; 32], NOW).unwrap();
    let delivery = f.delivery(&r);
    FAULT.with(|fault| *fault.borrow_mut() = Some(("generation-directory-fsync", 0)));
    assert!(r.delivered(&delivery, NOW).is_err());
    FAULT.with(|fault| *fault.borrow_mut() = None);
    assert!(r.attempt.as_ref().unwrap().state == State::Prepared);
    // The public entry point reconciles a complete staged generation locally.
    r.renew().unwrap();
    assert_eq!(r.config.generation, 2);
    assert!(r.attempt.as_ref().unwrap().state == State::Installed);
    assert_eq!(load_at(&f.0, None, NOW).unwrap().0.generation, 2);
}

#[test]
fn renewal_flow_reads_fresh_clock_at_each_boundary() {
    use std::{cell::RefCell, rc::Rc};
    let f = Fixture::new();
    let mut r = f.open();
    let begun = Begun {
        version: 1,
        enrollment_id: "b".repeat(64),
        renewal_id: "c".repeat(64),
        receiver_nonce: crate::provision_wire::encode(&[7; 32]),
        expires_at: NOW + 600,
        generations: vec![Generation {
            kind: Kind::MirrorPush,
            generation: 2,
        }],
    };
    let readings = Rc::new(RefCell::new(Vec::new()));
    let next = std::cell::Cell::new(NOW);
    let clock = || {
        let value = next.get();
        next.set(value + 1);
        readings.borrow_mut().push(value);
        Ok(value)
    };
    r.exchange = Some(Box::new(move |r, begin, clock| {
        if begin {
            r.credentials(clock()?)?;
            Ok(Response::Begun(begun.clone()))
        } else {
            let finish = r.finish(&[9; 32], clock()?)?;
            let application =
                crate::provision_wire::decode_fixed::<32>(&finish.application_key).unwrap();
            let target = Generation {
                kind: Kind::MirrorPush,
                generation: 2,
            };
            Ok(Response::Delivered(Delivered {
                version: 1,
                enrollment_id: finish.enrollment_id.clone(),
                renewal_id: finish.renewal_id.clone(),
                credentials: vec![wire::DeliveredCredential {
                    kind: Kind::MirrorPush,
                    credential_id: "d".repeat(64),
                    generation: 2,
                    bearer: wire::bearer(
                        &application,
                        &finish.enrollment_id,
                        &finish.renewal_id,
                        &target,
                    )
                    .expose()
                    .into(),
                    valid_from: NOW + 3,
                    valid_until: NOW + 3 + 7_776_000,
                }],
                client_certificate: None,
            }))
        }
    }));
    r.renew_with_clock(&clock).unwrap();
    assert_eq!(*readings.borrow(), vec![NOW, NOW + 1, NOW + 2, NOW + 3]);
    assert_eq!(r.config.generation, 2);
}

#[test]
fn strict_thirty_day_boundary_and_expiry() {
    assert_eq!(window(NOW + wire::WINDOW + 1, NOW), (false, false));
    assert_eq!(window(NOW + wire::WINDOW, NOW), (true, false));
    assert_eq!(window(NOW + wire::WINDOW - 1, NOW), (true, true));
    assert_eq!(window(NOW, NOW), (true, false));
}
#[test]
fn prepared_identical_retries_survive_restart_and_ten_minutes() {
    let f = Fixture::new();
    let mut r = f.prepare();
    let first = r.finish(&[1; 32], NOW).unwrap();
    drop(r);
    assert!(load_at(&f.0, None, NOW).is_ok());
    let mut r = f.open();
    let second = r.finish(&[2; 32], NOW).unwrap();
    assert_eq!(first.application_key, second.application_key);
    assert_ne!(first.proof, second.proof);
    drop(r);
    assert!(f.open().finish(&[3; 32], NOW + 601).is_ok());
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
    assert!(authority.is_active());
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
        if matches!(r.attempt.as_ref().unwrap().state, State::Prepared) {
            // Incomplete retained stage never selects; only a verified recovery may.
            assert_eq!(load_at(&f.0, None, NOW).unwrap().0.generation, 1);
            r.finish(&[2; 32], NOW).unwrap();
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

#[test]
fn disconnect_during_stalled_renewal_does_not_wait_for_install_lock() {
    use std::sync::mpsc;
    use std::time::Duration;
    let f = Fixture::new();
    let base = f.0.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut r = Renewal::open(&base, ENDPOINT).unwrap();
        r.exchange = Some(Box::new(move |_, _, _| {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Err("stalled exchange released".into())
        }));
        assert!(r.renew_with_clock(&|| Ok(NOW)).is_err());
        finished_tx.send(()).unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    Disconnect::open(&f.0, ENDPOINT).unwrap().commit().unwrap();
    assert!(load_at(&f.0, None, NOW).is_err());
    release_tx.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.join().unwrap();
}

#[test]
fn scheduler_stall_keeps_application_ack_and_disconnect_live() {
    use std::sync::mpsc;
    use std::time::Duration;
    let f = Fixture::new();
    let (_, credential) = load_at(&f.0, None, NOW).unwrap();
    let crate::Credential::Enrolled { authority, .. } = &credential else {
        panic!()
    };
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let authority = authority.clone();
    let options = crate::PushOptions {
        url: format!("http://{}", server.server_addr()),
        credential,
        sessions: crate::Options::default(),
        state: f.0.join("push.json"),
    };
    let mut cycle = crate::renewal_trigger::Cycle::new(authority.clone(), ENDPOINT.into(), true);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut release_rx = Some(release_rx);
    cycle.configure = Some(Box::new(move |renewal| {
        let entered_tx = entered_tx.clone();
        let release_rx = release_rx.take().unwrap();
        renewal.exchange = Some(Box::new(move |_, _, _| {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Err("injected stalled renewal".into())
        }));
    }));
    let base = f.0.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        cycle.tick(&base, &|| Ok(NOW), &mut |_| {});
        done_tx.send(()).unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (ack_tx, ack_rx) = mpsc::channel();
    let application = std::thread::spawn(move || {
        let client = crate::Client::new(&options).unwrap();
        ack_tx
            .send(client.post("facts", "{}".into()).is_ok())
            .unwrap();
    });
    let request = server
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    request
        .respond(tiny_http::Response::from_string("{}"))
        .unwrap();
    assert!(ack_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    application.join().unwrap();
    assert_eq!(authority.confirmed_generation(), 1);
    Disconnect::open(&f.0, ENDPOINT).unwrap().commit().unwrap();
    assert!(!authority.is_active());
    release_tx.send(()).unwrap();
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.join().unwrap();
    assert!(load_at(&f.0, None, NOW).is_err());
}

#[test]
fn idle_watch_clock_drives_boundary_retries_and_exact_notices() {
    use std::sync::atomic::AtomicUsize;
    let f = Fixture::new();
    let (config, credential) = load_at(&f.0, None, NOW).unwrap();
    let crate::Credential::Enrolled { authority, .. } = credential else {
        panic!()
    };
    let mut cycle = crate::renewal_trigger::Cycle::new(authority, ENDPOINT.into(), true);
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    cycle.configure = Some(Box::new(move |renewal| {
        let observed = observed.clone();
        renewal.exchange = Some(Box::new(move |_, _, _| {
            observed.fetch_add(1, Ordering::Relaxed);
            Err("temporary refusal".into())
        }));
    }));
    let clock = AtomicU64::new(config.scope.valid_until - wire::WINDOW);
    let mut lines = Vec::new();
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    clock.fetch_add(1, Ordering::Relaxed);
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        lines,
        [
            "Could not renew receiver receiver.example:443 credentials; 30 days remain. Check the receiver is reachable; renewal will retry."
        ]
    );
    clock.fetch_add(59, Ordering::Relaxed);
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    clock.fetch_add(1, Ordering::Relaxed);
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(lines.len(), 1);
    clock.store(
        config.scope.valid_until - 7 * 86_400 - 60,
        Ordering::Relaxed,
    );
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(lines.len(), 2);
    clock.fetch_add(60, Ordering::Relaxed);
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(
        lines.len(),
        3,
        "severity change overrides the daily reminder interval"
    );
    assert_eq!(
        lines[2],
        "Could not renew receiver receiver.example:443 credentials; 7 days remain. Check the receiver is reachable; renewal will retry."
    );
    clock.fetch_add(60, Ordering::Relaxed);
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(lines.len(), 3);
    clock.fetch_add(86_400, Ordering::Relaxed);
    cycle.tick(&f.0, &|| Ok(clock.load(Ordering::Relaxed)), &mut |line| {
        lines.push(line)
    });
    assert_eq!(lines.len(), 4);
}

#[test]
fn idle_scheduler_recovers_install_and_notices_only_confirmed_first_use() {
    let f = Fixture::new();
    let (_, credential) = load_at(&f.0, None, NOW).unwrap();
    let crate::Credential::Enrolled { authority, .. } = credential else {
        panic!()
    };
    let mut renewal = f.prepare();
    let delivered = f.delivery(&renewal);
    renewal.delivered(&delivered, NOW).unwrap();
    drop(renewal);
    let mut cycle = crate::renewal_trigger::Cycle::new(authority.clone(), ENDPOINT.into(), true);
    let mut lines = Vec::new();
    cycle.tick(&f.0, &|| Ok(NOW), &mut |line| lines.push(line));
    assert!(lines.is_empty());
    authority.application_success(1);
    cycle.tick(&f.0, &|| Ok(NOW + 60), &mut |line| lines.push(line));
    assert!(lines.is_empty());
    // An actual successful application request selects generation 2 and reports
    // its authenticated use to the same Authority shared with the scheduler.
    let (_, credential) = load_at(&f.0, None, NOW).unwrap();
    let crate::Credential::Enrolled {
        token,
        sources,
        identity,
        ..
    } = credential
    else {
        panic!()
    };
    let credential = crate::Credential::Enrolled {
        token,
        sources,
        identity,
        authority: authority.clone(),
    };
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let options = crate::PushOptions {
        url: format!("http://{}", server.server_addr()),
        credential,
        sessions: crate::Options::default(),
        state: f.0.join("push.json"),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let application = std::thread::spawn(move || {
        let client = crate::Client::new(&options).unwrap();
        tx.send(client.post("facts", "{}".into()).is_ok()).unwrap();
    });
    let request = server
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    request
        .respond(tiny_http::Response::from_string("{}"))
        .unwrap();
    assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
    application.join().unwrap();
    FAULT.with(|fault| *fault.borrow_mut() = Some(("before-rename", 0)));
    cycle.tick(&f.0, &|| Ok(NOW + 120), &mut |line| lines.push(line));
    assert!(
        lines.is_empty(),
        "uncertain durable confirmation cannot announce success"
    );
    FAULT.with(|fault| *fault.borrow_mut() = None);
    cycle.tick(&f.0, &|| Ok(NOW + 121), &mut |line| lines.push(line));
    assert_eq!(
        lines,
        ["Receiver receiver.example:443 credentials renewed."]
    );
    cycle.tick(&f.0, &|| Ok(NOW + 180), &mut |line| lines.push(line));
    assert_eq!(lines.len(), 1);
    drop(cycle);
    assert!(!f.open().pending());
}

#[test]
fn application_admission_keeps_request_valid_during_publication() {
    let f = Fixture::new();
    let (_, credential) = load_at(&f.0, None, NOW).unwrap();
    let crate::Credential::Enrolled { authority, .. } = credential else {
        panic!()
    };
    let mut r = f.open();
    let (receiver_dir, delivered) = f.receiver_delivery(&mut r);
    let (admission, selected, _) = authority.admit().unwrap();
    let overlapping = authority.delivery().unwrap();
    assert_eq!(selected.token().unwrap().expose(), "old-bearer");
    let started = std::time::Instant::now();
    r.delivered(&delivered, NOW).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(authority.is_active());
    // Existing admitted work retains its credential and can finish while a new
    // request immediately selects the published successor.
    assert_eq!(selected.token().unwrap().expose(), "old-bearer");
    let (next_admission, selected, _) = authority.admit().unwrap();
    assert_eq!(
        selected.token().unwrap().expose(),
        delivered.credentials[0].bearer
    );
    // The admitted old request remains valid remotely until first use of the
    // successor cuts it over. Local publication itself does not revoke it.
    let registry = crate::enrollment::Registry::read_current(&receiver_dir).unwrap();
    let old = crate::credentials::Secret::new("old-bearer".into());
    assert!(matches!(
        registry
            .authorize(Kind::MirrorPush, Kind::MirrorPush, &old, None, NOW)
            .decision,
        crate::credentials::Decision::Allowed { .. }
    ));
    let disconnect = Disconnect::open(&f.0, ENDPOINT).unwrap();
    drop(admission);
    drop(overlapping);
    drop(next_admission);
    disconnect.commit().unwrap();
    assert!(!authority.is_active());
}

#[test]
fn admission_reloads_publication_and_disconnect_during_reload_refuses() {
    use std::sync::mpsc;
    use std::time::Duration;
    for disconnect in [false, true] {
        let f = Fixture::new();
        let (_, credential) = load_at(&f.0, None, NOW).unwrap();
        let crate::Credential::Enrolled { authority, .. } = credential else {
            panic!()
        };
        let mut r = f.open();
        let (_, delivered) = f.receiver_delivery(&mut r);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            ADMISSION_BARRIER.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |point| {
                    entered_tx.send(point.to_owned()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                }))
            });
            let result = authority
                .admit()
                .map(|(_guard, credential, _)| credential.token().unwrap().expose().to_owned());
            ADMISSION_BARRIER.with(|hook| *hook.borrow_mut() = None);
            result_tx.send(result).unwrap();
        });
        assert_eq!(
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "selected"
        );
        r.delivered(&delivered, NOW).unwrap();
        resume_tx.send(()).unwrap();
        assert_eq!(
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "reload"
        );
        if disconnect {
            Disconnect::open(&f.0, ENDPOINT).unwrap().commit().unwrap();
        }
        resume_tx.send(()).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        if disconnect {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), delivered.credentials[0].bearer);
        }
        worker.join().unwrap();
    }
}

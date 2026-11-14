use super::*;
use crate::{credentials::Kind, enrollment::Binding};
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "semon-origin-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn install(&self) -> String {
        self.install_sources(&[PathBuf::from("/native/approved.jsonl")])
            .unwrap()
    }
    fn install_sources(&self, sources: &[PathBuf]) -> Result<String> {
        self.install_endpoint("https://receiver.example:443", sources)
    }
    fn install_endpoint(&self, endpoint: &str, sources: &[PathBuf]) -> Result<String> {
        let private = EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &ring::rand::SystemRandom::new(),
        )
        .unwrap();
        install(
            &self.0,
            endpoint,
            "laptop",
            &ActivationInput {
                receiver_id: "receiver".into(),
                machine: "laptop".into(),
                binding: Binding::Inapplicable,
                kinds: vec![(Kind::MirrorPush, false)],
                client_certificate_pins: vec![],
                valid_until: 1000,
            },
            &Activated {
                version: 1,
                enrollment_id: "enrollment".into(),
                credentials: vec![(Kind::MirrorPush, "secret-origin-bearer".into())],
                client_certificate: None,
            },
            private.as_ref(),
            sources,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn atomic_custody_integrity_permissions_and_secret_debug() {
    let f = Fixture::new();
    let name = f.install();
    let (config, credential) = load(&f.0, None).unwrap();
    assert!(!format!("{credential:?}").contains("secret-origin-bearer"));
    assert_eq!(
        consent(&f.0, &name, &config).unwrap(),
        [PathBuf::from("/native/approved.jsonl")]
    );
    assert_eq!(
        fs::metadata(&f.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for file in [
        "enrollment.json",
        "origin-key.pk8",
        "mirror-push.secret",
        "source-consent.json",
    ] {
        assert_eq!(
            fs::metadata(f.0.join(&name).join(file))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let path = f.0.join(&name).join("source-consent.json");
    let bytes = fs::read(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["sources"] = serde_json::json!(["/native/unapproved.jsonl"]);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(consent(&f.0, &name, &config).is_err());
    fs::write(&path, bytes).unwrap();
    let path = f.0.join(&name).join("enrollment.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["configuration"]["endpoint"] = "https://attacker.example:443".into();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load(&f.0, None).is_err());
}
#[test]
fn invalid_client_certificate_never_publishes_origin_custody() {
    let now = 1_800_000_000;
    let key = rcgen::KeyPair::generate().unwrap();
    let other = rcgen::KeyPair::generate().unwrap();
    let (ca, _) = crate::client_ca::generate(now).unwrap();
    let until = now + 3600;
    let scope = ActivationInput {
        receiver_id: "receiver".into(),
        machine: "laptop".into(),
        binding: Binding::Inapplicable,
        kinds: vec![(Kind::MirrorPush, true)],
        client_certificate_pins: vec![crate::client_ca::pin(key.public_key_raw()).unwrap()],
        valid_until: until,
    };
    let mut missing_eku = rcgen::CertificateParams::default();
    missing_eku.not_before = time::OffsetDateTime::from_unix_timestamp(now as i64 - 300).unwrap();
    missing_eku.not_after = time::OffsetDateTime::from_unix_timestamp(until as i64).unwrap();
    missing_eku.is_ca = rcgen::IsCa::ExplicitNoCa;
    let missing_eku = missing_eku.self_signed(&key).unwrap().der().to_vec();
    for certificate in [
        crate::client_ca::issue(&ca, now, other.public_key_raw(), "enrollment", now, until)
            .unwrap(),
        crate::client_ca::issue(&ca, now, key.public_key_raw(), "enrollment", now, until + 1)
            .unwrap(),
        missing_eku,
    ] {
        let f = Fixture::new();
        let delivery = Activated {
            version: 1,
            enrollment_id: "enrollment".into(),
            credentials: vec![(Kind::MirrorPush, "secret".into())],
            client_certificate: Some(crate::provision_wire::encode(&certificate)),
        };
        assert!(
            install(
                &f.0,
                "https://receiver.example:443",
                "laptop",
                &scope,
                &delivery,
                &key.serialize_der(),
                &[]
            )
            .is_err()
        );
        assert!(!f.0.exists());
    }
}

#[test]
fn certificate_clock_refuses_future_install_and_expired_load() {
    let now = 1_800_000_000;
    let key = rcgen::KeyPair::generate().unwrap();
    let (ca, _) = crate::client_ca::generate(now).unwrap();
    let until = now + 3600;
    let certificate =
        crate::client_ca::issue(&ca, now, key.public_key_raw(), "enrollment", now, until).unwrap();
    let scope = ActivationInput {
        receiver_id: "receiver".into(),
        machine: "laptop".into(),
        binding: Binding::Inapplicable,
        kinds: vec![(Kind::MirrorPush, true)],
        client_certificate_pins: vec![crate::client_ca::pin(key.public_key_raw()).unwrap()],
        valid_until: until,
    };
    let delivery = Activated {
        version: 1,
        enrollment_id: "enrollment".into(),
        credentials: vec![(Kind::MirrorPush, "secret".into())],
        client_certificate: Some(crate::provision_wire::encode(&certificate)),
    };
    let f = Fixture::new();
    assert!(
        install_at(
            &f.0,
            "https://receiver.example:443",
            "laptop",
            &scope,
            &delivery,
            &key.serialize_der(),
            &[],
            now - 601
        )
        .unwrap_err()
        .contains("not yet valid")
    );
    assert!(!f.0.exists());
    install_at(
        &f.0,
        "https://receiver.example:443",
        "laptop",
        &scope,
        &delivery,
        &key.serialize_der(),
        &[],
        now,
    )
    .unwrap();
    assert!(
        load_at(&f.0, Some("https://receiver.example:443"), until)
            .err()
            .unwrap()
            .contains("expired")
    );
    assert!(
        load_at(&f.0, Some("https://receiver.example:443"), now - 601)
            .err()
            .unwrap()
            .contains("not yet valid")
    );
    assert!(load_at(&f.0, Some("https://receiver.example:443"), now).is_ok());
}

#[test]
fn staged_credentials_are_never_a_half_installed_enrollment() {
    let f = Fixture::new();
    fs::DirBuilder::new().mode(0o700).create(&f.0).unwrap();
    let root = Directory::open(&f.0).unwrap();
    let stage = root.create_child("stage-interrupted").unwrap();
    stage
        .write("mirror-push.secret", b"interrupted-secret")
        .unwrap();
    assert!(load(&f.0, None).is_err());
    f.install();
    load(&f.0, Some("https://receiver.example:443")).unwrap();
    assert!(f.0.join("stage-interrupted/mirror-push.secret").exists());
}
#[test]
fn symlink_and_world_readable_custody_refuse() {
    let f = Fixture::new();
    let name = f.install();
    let secret = f.0.join(&name).join("mirror-push.secret");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load(&f.0, None).is_err());
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let moved = f.0.join(&name).join("moved.secret");
    fs::rename(&secret, &moved).unwrap();
    symlink(&moved, &secret).unwrap();
    assert!(load(&f.0, None).is_err());
}

#[test]
fn realistic_source_list_roundtrips_and_oversize_never_publishes() {
    let f = Fixture::new();
    let sources: Vec<_> = (0..5000)
        .map(|n| {
            PathBuf::from(format!(
                "/native/projects/a-realistic-project/sessions/rollout-{n:08}.jsonl"
            ))
        })
        .collect();
    let name = f.install_sources(&sources).unwrap();
    let (config, _) = load(&f.0, Some("https://receiver.example:443")).unwrap();
    assert_eq!(consent(&f.0, &name, &config).unwrap(), sources);
    let oversized = Fixture::new();
    assert!(
        oversized
            .install_sources(&[PathBuf::from("x".repeat(FILE_CAP + 1))])
            .unwrap_err()
            .contains("4 MiB")
    );
    assert!(load(&oversized.0, None).is_err());
    assert!(
        !fs::read_dir(&oversized.0)
            .unwrap()
            .any(|entry| entry.unwrap().file_name().len() == 64)
    );
}
#[test]
fn install_explicitly_unlocks_even_with_duplicated_descriptor() {
    let f = Fixture::new();
    f.install();
    let duplicate = LOCK_DUPLICATE.with(|slot| slot.borrow_mut().take().unwrap());
    let root = Directory::open(&f.0).unwrap();
    let next = root
        .checked
        .open_file(Path::new("install.lock"), libc::O_RDWR)
        .unwrap();
    next.try_lock()
        .expect("install must unlock even while duplicate remains live");
    next.unlock().unwrap();
    drop(duplicate);
}
#[test]
fn config_roots_require_absolute_paths() {
    use std::ffi::OsStr;
    assert_eq!(
        configuration_directory(Some(OsStr::new("relative")), Some(OsStr::new("/home/test")))
            .unwrap(),
        PathBuf::from("/home/test/.config/semon/origin")
    );
    assert_eq!(
        configuration_directory(Some(OsStr::new("/config")), None).unwrap(),
        PathBuf::from("/config/semon/origin")
    );
    assert!(
        configuration_directory(
            Some(OsStr::new("relative")),
            Some(OsStr::new("relative-home"))
        )
        .is_err()
    );
    assert!(configuration_directory(None, None).is_err());
}

#[test]
fn url_selector_uses_pairing_normalization_and_never_accepts_hashes() {
    let f = Fixture::new();
    let name = f.install();
    assert_eq!(
        name,
        crate::wire::sha256_hex(b"https://receiver.example:443")
    );
    for url in [
        "https://receiver.example",
        "https://RECEIVER.EXAMPLE/",
        "https://receiver.example:443/",
    ] {
        let (config, _) = load(&f.0, Some(url)).unwrap();
        assert_eq!(receiver_endpoint(url).unwrap(), config.endpoint);
        assert!(!Disconnect::open(&f.0, url).unwrap().is_disconnected());
    }
    for (url, canonical) in [
        ("https://[::1]/", "https://[::1]:443"),
        ("https://[2001:DB8::1]:443", "https://[2001:db8::1]:443"),
        (
            "https://Receiver.Example:8735/",
            "https://receiver.example:8735",
        ),
    ] {
        assert_eq!(receiver_endpoint(url).unwrap(), canonical);
        assert_eq!(receiver_host(url).unwrap(), &canonical[8..]);
    }
    for invalid in [
        name.as_str(),
        "https://user:secret@receiver.example",
        "https://receiver.example?secret=value",
        "https://receiver.example/#secret",
        "http://receiver.example",
        "https://receiver.example/path",
    ] {
        assert!(receiver_endpoint(invalid).is_err());
        assert!(load(&f.0, Some(invalid)).is_err());
        assert!(Disconnect::open(&f.0, invalid).is_err());
    }
}

#[test]
fn ipv6_url_selects_the_same_enrolled_custody_for_push_and_disconnect() {
    let f = Fixture::new();
    let endpoint = receiver_endpoint("https://[2001:DB8::1]/").unwrap();
    let name = f.install_endpoint(&endpoint, &[]).unwrap();
    assert_eq!(name, crate::wire::sha256_hex(b"https://[2001:db8::1]:443"));
    for url in ["https://[2001:DB8::1]", "https://[2001:db8::1]:443/"] {
        let (config, _) = load(&f.0, Some(url)).unwrap();
        assert_eq!(config.endpoint, endpoint);
        assert!(!Disconnect::open(&f.0, url).unwrap().is_disconnected());
    }
    assert!(load(&f.0, Some("https://[2001:db8::1]:8735")).is_err());
    Disconnect::open(&f.0, "https://[2001:DB8::1]/")
        .unwrap()
        .commit()
        .unwrap();
    assert!(load(&f.0, Some(&endpoint)).is_err());
}

#[test]
fn disconnect_preserves_custody_cancels_delivery_and_is_state_idempotent() {
    let f = Fixture::new();
    let name = f.install();
    let (config, _) = load(&f.0, None).unwrap();
    let authority = Authority::new(&f.0, &config).unwrap();
    let stop = crate::Stop::new().with_authority(authority.clone());
    let files = [
        "enrollment.json",
        "origin-key.pk8",
        "mirror-push.secret",
        "source-consent.json",
    ];
    let before: Vec<_> = files
        .iter()
        .map(|p| fs::read(f.0.join(&name).join(p)).unwrap())
        .collect();
    assert!(!stop.is_stopped());
    // Dropping the unconfirmed operation changes nothing.
    drop(Disconnect::open(&f.0, "https://receiver.example").unwrap());
    assert!(authority.is_active());
    let operation = Disconnect::open(&f.0, "https://RECEIVER.example:443/").unwrap();
    assert!(!operation.is_disconnected());
    operation.commit().unwrap();
    assert!(stop.is_stopped());
    assert!(!authority.is_active());
    assert!(load(&f.0, None).is_err());
    let operation = Disconnect::open(&f.0, "https://receiver.example").unwrap();
    assert!(operation.is_disconnected());
    operation.commit().unwrap();
    for (file, bytes) in files.iter().zip(before) {
        assert_eq!(fs::read(f.0.join(&name).join(file)).unwrap(), bytes);
    }
    assert_eq!(
        consent(&f.0, &name, &config).unwrap(),
        [PathBuf::from("/native/approved.jsonl")]
    );
}

#[test]
fn disconnect_wins_install_lock_against_concurrent_publication() {
    let f = Fixture::new();
    f.install();
    let operation = Disconnect::open(&f.0, "https://receiver.example").unwrap();
    let base = f.0.clone();
    let (finished, completed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        // A delivery already prepared before disconnect cannot obtain the install lock.
        let other = Fixture(base);
        let result = other.install_sources(&[]);
        // The parent retains ownership of fixture cleanup.
        std::mem::forget(other);
        finished.send(result).unwrap();
    });
    assert_eq!(
        completed
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap_err(),
        "origin custody is in use"
    );
    worker.join().unwrap();
    operation.commit().unwrap();
    // A delayed installation cannot overwrite the disabled selection, either.
    assert!(f.install_sources(&[]).is_err());
    assert!(
        Disconnect::open(&f.0, "https://receiver.example")
            .unwrap()
            .is_disconnected()
    );
    assert!(load(&f.0, None).is_err());
}

#[test]
fn disconnect_unknown_and_insecure_state_fail_closed() {
    let f = Fixture::new();
    assert_eq!(
        Disconnect::open(&f.0, "https://UNKNOWN.example/")
            .err()
            .unwrap(),
        "receiver unknown.example:443 was not found; use the HTTPS URL given to connect as --receiver URL, then retry."
    );
    let name = f.install();
    assert!(
        Disconnect::open(&f.0, "https://other.example")
            .err()
            .unwrap()
            .contains("was not found")
    );
    let (config, _) = load(&f.0, None).unwrap();
    let authority = Authority::new(&f.0, &config).unwrap();
    // Interrupted disabled staging is inactive; it neither enables nor disables a channel.
    let dir = Directory::open(&f.0).unwrap().child(&name).unwrap();
    dir.write("disabled-stage-interrupted", b"disabled\n")
        .unwrap();
    assert!(authority.is_active());
    dir.write("disabled", b"corrupt").unwrap();
    assert!(!authority.is_active());
    assert_eq!(
        Disconnect::open(&f.0, "https://receiver.example")
            .err()
            .unwrap(),
        DISCONNECT_UNCERTAIN
    );
    assert!(load(&f.0, None).is_err());
}

#[test]
fn disconnect_stops_waiting_for_active_delivery_without_new_work() {
    let f = Fixture::new();
    f.install();
    let (config, _) = load(&f.0, None).unwrap();
    let stop = crate::Stop::new().with_authority(Authority::new(&f.0, &config).unwrap());
    let waiting = stop.clone();
    let (ready, started) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (finished, completed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = waiting.run("delivery", move || {
            ready.send(()).unwrap();
            released
                .recv_timeout(std::time::Duration::from_secs(15))
                .unwrap();
            7
        });
        finished.send(result).unwrap();
    });
    started
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    Disconnect::open(&f.0, "https://receiver.example")
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(
        completed
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        None
    );
    worker.join().unwrap();
    assert!(stop.is_stopped());
    assert_eq!(
        stop.run("later-delivery", || panic!(
            "disabled delivery must not start"
        ))
        .unwrap(),
        None::<()>
    );
    release.send(()).unwrap();
}

#[test]
fn public_loader_client_refuses_delivery_after_disconnect() {
    let f = Fixture::new();
    f.install();
    let (config, credential) = load(&f.0, None).unwrap();
    assert!(
        matches!(&credential, crate::Credential::Enrolled { sources, .. } if sources == &[PathBuf::from("/native/approved.jsonl")])
    );
    let client = crate::Client::new(&crate::PushOptions {
        url: config.endpoint,
        credential,
        sessions: Default::default(),
        state: f.0.join("push-state"),
    })
    .unwrap();
    Disconnect::open(&f.0, "https://receiver.example")
        .unwrap()
        .commit()
        .unwrap();
    assert!(matches!(
        client.post("facts", "{}".into()),
        Err(crate::Failure::Stopped)
    ));
}

#[test]
fn disconnect_aborts_slow_upload_and_drains_transport_before_success() {
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        time::Duration,
    };
    let f = Fixture::new();
    f.install();
    let (_, credential) = load(&f.0, None).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, started) = std::sync::mpsc::channel();
    let (drain, draining) = std::sync::mpsc::channel();
    let (observed, received) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        let mut chunk = [0; 1024];
        let first = reader.read(&mut chunk).unwrap();
        ready.send(()).unwrap();
        draining.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut total = first;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => total += count,
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(error) => panic!("upload did not abort: {error}"),
            }
        }
        observed.send(total).unwrap();
    });
    let client = crate::Client::new(&crate::PushOptions {
        url: format!("http://{address}"),
        credential,
        sessions: Default::default(),
        state: f.0.join("push-state"),
    })
    .unwrap();
    let (finished, uploaded) = std::sync::mpsc::channel();
    let upload = std::thread::spawn(move || {
        finished
            .send(client.post("append", "x".repeat(32 * 1024 * 1024)).is_err())
            .unwrap()
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    let base = f.0.clone();
    let (committed, disconnected) = std::sync::mpsc::channel();
    let disconnect = std::thread::spawn(move || {
        committed
            .send(
                Disconnect::open(&base, "https://receiver.example")
                    .unwrap()
                    .commit(),
            )
            .unwrap()
    });
    // Let the transport consume already-buffered bytes. Success must drain the
    // request's shared lock, and the body reader must abort before its full size.
    let marker =
        f.0.join(custody_name("https://receiver.example:443"))
            .join("disabled");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "disabled publication timed out"
        );
        std::thread::yield_now();
    }
    drain.send(()).unwrap();
    disconnected
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .unwrap();
    assert!(uploaded.recv_timeout(Duration::from_secs(5)).unwrap());
    assert!(received.recv_timeout(Duration::from_secs(5)).unwrap() < 32 * 1024 * 1024);
    upload.join().unwrap();
    disconnect.join().unwrap();
    server.join().unwrap();
}

#[test]
fn disconnect_success_waits_for_existing_delivery_guard() {
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    f.install();
    let (_, credential) = load(&f.0, None).unwrap();
    let crate::Credential::Enrolled { authority, .. } = credential else {
        panic!("enrolled authority required")
    };
    let delivery = authority.delivery().unwrap();
    let base = f.0.clone();
    let (finished, completed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        finished
            .send(
                Disconnect::open(&base, "https://receiver.example")
                    .unwrap()
                    .commit(),
            )
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while authority.is_active() {
        assert!(Instant::now() < deadline, "disabled publication timed out");
        std::thread::yield_now();
    }
    assert!(matches!(
        completed.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(delivery);
    completed
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert!(authority.delivery().is_err());
}

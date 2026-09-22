use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use age::x25519;
use ed25519_dalek::SigningKey;
use semon_relay::{
    AuthError, EnvelopeOutcome, Frame, FrameKey, FrameMode, HttpTransport, MAX_REQUEST_BODY_BYTES,
    MachineIdentity, ReceiveError, ReceiveOutcome, Receiver, RequestSigner, RequestVerifier,
    Sender, ServeConfig, TlsFiles, Transport, ZERO_CHAIN, chain_line, decrypt_envelope,
    decrypt_frame, encrypt_envelope, encrypt_frame, enroll_machine, generate_data_key, init,
    serve_configured, takeover_session_encrypted, validate_serve_config, verify_encrypted_frames,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "semon-relay-security-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn signing(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn encrypted_frame(
    key: &[u8; 32],
    machine: &str,
    session: &str,
    epoch: u64,
    seq: u64,
    previous: &[u8; 32],
    line: &[u8],
) -> (Frame, [u8; 32]) {
    let chain = chain_line(previous, line);
    let frame_key = FrameKey {
        session: session.into(),
        stream: "main".into(),
        generation: 0,
        epoch,
        seq,
    };
    let payload = encrypt_frame(key, &frame_key, machine, &chain, line).unwrap();
    (
        Frame::encrypted(
            frame_key,
            machine.into(),
            1,
            2,
            "synthetic-boot".into(),
            payload,
        ),
        chain,
    )
}

fn source_file(temp: &TempDir, bytes: &[u8]) -> (PathBuf, PathBuf, PathBuf) {
    let projects = temp.path().join("projects");
    let project = projects.join("synthetic-project");
    fs::create_dir_all(&project).unwrap();
    let source = project.join("session-a.jsonl");
    fs::write(&source, bytes).unwrap();
    let state = temp.path().join("state/relay.json");
    (projects, source, state)
}

#[cfg(unix)]
#[test]
fn identities_initialize_once_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new("identity-init");
    let config = temp.path().join("config/semon");
    let first = init(&config).unwrap();
    assert!(first.age_created);
    assert!(first.signing_created);
    let identity = MachineIdentity::load(&config).unwrap();
    let public = identity.age.to_public().to_string();
    let signing_public = identity.signing.verifying_key();

    let second = init(&config).unwrap();
    assert!(!second.age_created);
    assert!(!second.signing_created);
    let reloaded = MachineIdentity::load(&config).unwrap();
    assert_eq!(reloaded.age.to_public().to_string(), public);
    assert_eq!(reloaded.signing.verifying_key(), signing_public);
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(config.join("identity.age"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(config.join("signing.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    fs::set_permissions(
        config.join("signing.key"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(MachineIdentity::load(&config).is_err());
}

#[test]
fn envelope_and_frame_crypto_round_trip_and_bind_aad() {
    let first = x25519::Identity::generate();
    let second = x25519::Identity::generate();
    let recovery = x25519::Identity::generate();
    let outsider = x25519::Identity::generate();
    let recipients = vec![first.to_public(), second.to_public(), recovery.to_public()];
    let key = generate_data_key();
    let envelope = encrypt_envelope(&key, &recipients).unwrap();

    assert_eq!(decrypt_envelope(&envelope, &first).unwrap(), key);
    assert_eq!(decrypt_envelope(&envelope, &second).unwrap(), key);
    assert_eq!(decrypt_envelope(&envelope, &recovery).unwrap(), key);
    assert!(decrypt_envelope(&envelope, &outsider).is_err());

    let machine = "machine-a";
    let line = b"known plaintext marker 8b63f4a1\n";
    let chain = chain_line(&ZERO_CHAIN, line);
    let frame_key = FrameKey {
        session: "session-a".into(),
        stream: "main".into(),
        generation: 0,
        epoch: 0,
        seq: 0,
    };
    let payload = encrypt_frame(&key, &frame_key, machine, &chain, line).unwrap();
    assert_eq!(
        decrypt_frame(&key, &frame_key, machine, &payload).unwrap(),
        (chain, line.to_vec())
    );
    let mut moved_seq = frame_key.clone();
    moved_seq.seq = 1;
    assert!(decrypt_frame(&key, &moved_seq, machine, &payload).is_err());
    let mut moved_session = frame_key;
    moved_session.session = "session-b".into();
    assert!(decrypt_frame(&key, &moved_session, machine, &payload).is_err());
}

#[test]
fn encrypted_duplicate_uses_tag_and_collision_is_rejected() {
    let temp = TempDir::new("encrypted-idempotence");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let key = generate_data_key();
    let recipient = x25519::Identity::generate();
    let envelope = encrypt_envelope(&key, &[recipient.to_public()]).unwrap();
    receiver.acquire("session", "machine-a").unwrap();
    receiver
        .put_envelope("session", "machine-a", &envelope, false, false)
        .unwrap();
    let (first, _) = encrypted_frame(&key, "machine-a", "session", 0, 0, &ZERO_CHAIN, b"same\n");
    let (resend, _) = encrypted_frame(&key, "machine-a", "session", 0, 0, &ZERO_CHAIN, b"same\n");
    assert_ne!(
        first.encrypted_payload().unwrap().nonce,
        resend.encrypted_payload().unwrap().nonce
    );
    assert_eq!(receiver.accept(&first).unwrap(), ReceiveOutcome::Stored);
    assert_eq!(receiver.accept(&resend).unwrap(), ReceiveOutcome::Duplicate);

    let (collision, _) = encrypted_frame(
        &key,
        "machine-a",
        "session",
        0,
        0,
        &ZERO_CHAIN,
        b"different\n",
    );
    assert!(matches!(
        receiver.accept(&collision),
        Err(ReceiveError::Collision)
    ));
}

#[test]
fn receiver_storage_contains_no_plaintext_and_client_verifies_chain() {
    let temp = TempDir::new("ciphertext-storage");
    let root = temp.path().join("receiver");
    let receiver = Receiver::open(&root).unwrap();
    let identity = x25519::Identity::generate();
    let key = generate_data_key();
    let envelope = encrypt_envelope(&key, &[identity.to_public()]).unwrap();
    receiver.acquire("session", "machine-a").unwrap();
    receiver
        .put_envelope("session", "machine-a", &envelope, false, false)
        .unwrap();
    let marker = b"PLAINTEXT-NEVER-STORED-51d75b\n";
    let (first, first_chain) =
        encrypted_frame(&key, "machine-a", "session", 0, 0, &ZERO_CHAIN, marker);
    let (second, _) = encrypted_frame(
        &key,
        "machine-a",
        "session",
        0,
        1,
        &first_chain,
        b"second\n",
    );
    receiver.accept(&first).unwrap();
    receiver.accept(&second).unwrap();

    let frames = receiver.list_frames("session").unwrap();
    let tips = verify_encrypted_frames("session", frames, &key).unwrap();
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].seq, 1);
    assert_eq!(tips[0].mode, FrameMode::Encrypted);

    let mut stored = Vec::new();
    collect_files(&root, &mut stored);
    assert!(
        stored
            .iter()
            .all(|bytes| !bytes.windows(marker.len()).any(|window| window == marker))
    );
}

#[test]
fn rewrap_adds_a_recipient_and_is_audited() {
    let temp = TempDir::new("rewrap");
    let root = temp.path().join("receiver");
    let receiver = Receiver::open(&root).unwrap();
    let first = x25519::Identity::generate();
    let recovery = x25519::Identity::generate();
    let newly_enrolled = x25519::Identity::generate();
    let key = generate_data_key();
    receiver.acquire("session", "machine-a").unwrap();
    let original = encrypt_envelope(&key, &[first.to_public(), recovery.to_public()]).unwrap();
    assert_eq!(
        receiver
            .put_envelope("session", "machine-a", &original, false, false)
            .unwrap(),
        EnvelopeOutcome::Stored
    );
    assert!(decrypt_envelope(&original, &newly_enrolled).is_err());

    let replacement = encrypt_envelope(
        &key,
        &[
            first.to_public(),
            recovery.to_public(),
            newly_enrolled.to_public(),
        ],
    )
    .unwrap();
    assert_eq!(
        receiver
            .put_envelope("session", "machine-a", &replacement, true, false)
            .unwrap(),
        EnvelopeOutcome::Replaced
    );
    assert_eq!(
        decrypt_envelope(&replacement, &newly_enrolled).unwrap(),
        key
    );
    let log = fs::read_to_string(root.join("rewraps.jsonl")).unwrap();
    assert!(log.contains("\"session\":\"session\""));
    assert!(log.contains("replacement_sha256"));
}

#[test]
fn receiver_refuses_mixed_session_modes_and_encrypted_fencing_still_orphans() {
    let temp = TempDir::new("mixed-and-fenced");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let identity = x25519::Identity::generate();
    let key = generate_data_key();
    let envelope = encrypt_envelope(&key, &[identity.to_public()]).unwrap();
    receiver.acquire("session", "machine-a").unwrap();
    receiver
        .put_envelope("session", "machine-a", &envelope, false, false)
        .unwrap();
    let (first, first_chain) =
        encrypted_frame(&key, "machine-a", "session", 0, 0, &ZERO_CHAIN, b"zero\n");
    receiver.accept(&first).unwrap();
    let plaintext = Frame::plaintext(
        FrameKey {
            seq: 1,
            ..first.key.clone()
        },
        "machine-a".into(),
        chain_line(&first_chain, b"one\n"),
        1,
        2,
        "boot".into(),
        b"one\n".to_vec(),
    );
    assert!(matches!(
        receiver.accept(&plaintext),
        Err(ReceiveError::MixedMode { .. })
    ));

    receiver.takeover("session", "machine-b", 0, true).unwrap();
    let (orphan, _) = encrypted_frame(&key, "machine-a", "session", 0, 1, &first_chain, b"late\n");
    assert_eq!(
        receiver.accept_orphan(&orphan).unwrap(),
        ReceiveOutcome::Stored
    );
    let stored = receiver
        .read_orphan("session", "main", 0, 0, 1)
        .unwrap()
        .unwrap();
    let (_, line) = decrypt_frame(
        &key,
        &stored.key,
        &stored.machine,
        stored.encrypted_payload().unwrap(),
    )
    .unwrap();
    assert_eq!(line, b"late\n");
}

#[test]
fn signed_requests_reject_unenrolled_bad_stale_replayed_and_borrowed_ids() {
    let enrolled = RequestSigner::new(signing(1));
    let unenrolled = RequestSigner::new(signing(2));
    let mut verifier =
        RequestVerifier::new([(enrolled.machine().to_owned(), signing(1).verifying_key())]);
    let body = br#"{"machine":"placeholder"}"#;
    let now = 1_000_000;

    let untrusted = unenrolled
        .sign_at("POST", "/v1/test", body, now, [1; 16])
        .unwrap();
    assert!(matches!(
        verifier.verify(
            "POST",
            "/v1/test",
            body,
            &untrusted,
            unenrolled.machine(),
            now
        ),
        Err(AuthError::Unenrolled(_))
    ));

    let mut bad = enrolled
        .sign_at("POST", "/v1/test", body, now, [2; 16])
        .unwrap();
    bad.signature.replace_range(0..2, "00");
    assert!(matches!(
        verifier.verify("POST", "/v1/test", body, &bad, enrolled.machine(), now),
        Err(AuthError::Signature)
    ));

    let stale = enrolled
        .sign_at("POST", "/v1/test", body, 0, [3; 16])
        .unwrap();
    assert!(matches!(
        verifier.verify("POST", "/v1/test", body, &stale, enrolled.machine(), now),
        Err(AuthError::Stale)
    ));

    let replay = enrolled
        .sign_at("POST", "/v1/test", body, now, [4; 16])
        .unwrap();
    verifier
        .verify("POST", "/v1/test", body, &replay, enrolled.machine(), now)
        .unwrap();
    assert!(matches!(
        verifier.verify("POST", "/v1/test", body, &replay, enrolled.machine(), now),
        Err(AuthError::Replay)
    ));

    let borrowed = enrolled
        .sign_at("POST", "/v1/test", body, now, [5; 16])
        .unwrap();
    assert!(matches!(
        verifier.verify("POST", "/v1/test", body, &borrowed, "borrowed-id", now),
        Err(AuthError::MachineMismatch)
    ));
}

#[test]
fn non_loopback_requires_tls_enrollment_and_encrypted_mode() {
    let temp = TempDir::new("non-loopback-gates");
    let root = temp.path().join("receiver");
    fs::create_dir_all(&root).unwrap();
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8734);
    let no_tls = ServeConfig::default();
    assert!(matches!(
        validate_serve_config(address, &root, &no_tls),
        Err(ReceiveError::NonLoopbackTls)
    ));
    let tls = TlsFiles {
        certificate: fixture("test-cert.pem"),
        private_key: fixture("test-key.pem"),
    };
    let secure = ServeConfig {
        tls: Some(tls.clone()),
        insecure_plaintext: false,
    };
    assert!(matches!(
        validate_serve_config(address, &root, &secure),
        Err(ReceiveError::NonLoopbackMachines(_))
    ));
    enroll_machine(
        &root,
        &hex::encode(signing(1).verifying_key().as_bytes()),
        "machine-a",
    )
    .unwrap();
    let plaintext = ServeConfig {
        tls: Some(tls),
        insecure_plaintext: true,
    };
    assert!(matches!(
        validate_serve_config(address, &root, &plaintext),
        Err(ReceiveError::NonLoopbackPlaintext)
    ));
    assert!(validate_serve_config(address, &root, &secure).is_ok());
}

#[test]
fn sender_refuses_an_unpinned_tls_endpoint() {
    let signer = RequestSigner::new(signing(1));
    assert!(matches!(
        HttpTransport::secure(
            "https://127.0.0.1:8734/v1/frames",
            Duration::from_secs(1),
            signer,
            None,
        ),
        Err(semon_relay::TransportError::MissingTlsPin(_))
    ));
}

#[test]
fn unenrolled_large_declared_body_is_rejected_without_being_read() {
    let temp = TempDir::new("preauth-body");
    let port = start_http_receiver(temp.path().join("receiver"));
    let stream = declared_request(port, &"00".repeat(32), MAX_REQUEST_BODY_BYTES + 1);
    assert_eq!(response_status(stream), 401);
}

#[test]
fn enrolled_oversized_body_is_rejected_while_another_body_stalls() {
    let temp = TempDir::new("body-limit-workers");
    let root = temp.path().join("receiver");
    let signing = signing(21);
    enroll_machine(
        &root,
        &hex::encode(signing.verifying_key().as_bytes()),
        "machine-a",
    )
    .unwrap();
    let machine = RequestSigner::new(signing).machine().to_owned();
    let port = start_http_receiver(root);

    let _stalled = declared_request(port, &machine, 1024 * 1024);
    thread::sleep(Duration::from_millis(100));
    let oversized = declared_request(port, &machine, MAX_REQUEST_BODY_BYTES + 1);
    assert_eq!(response_status(oversized), 413);
}

#[test]
fn https_round_trip_covers_encryption_takeover_and_orphans() {
    let temp = TempDir::new("https-round-trip");
    let receiver_root = temp.path().join("receiver");
    fs::create_dir_all(&receiver_root).unwrap();
    let signing_a = signing(10);
    let signing_b = signing(11);
    let age_a = x25519::Identity::generate();
    let age_b = x25519::Identity::generate();
    let recovery = x25519::Identity::generate();
    let recipients = vec![age_a.to_public(), age_b.to_public(), recovery.to_public()];
    for (key, name) in [(&signing_a, "machine-a"), (&signing_b, "machine-b")] {
        enroll_machine(
            &receiver_root,
            &hex::encode(key.verifying_key().as_bytes()),
            name,
        )
        .unwrap();
    }
    let port = unused_port();
    let listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let root = receiver_root.clone();
    thread::spawn(move || {
        serve_configured(
            listen,
            &root,
            ServeConfig {
                tls: Some(TlsFiles {
                    certificate: fixture("test-cert.pem"),
                    private_key: fixture("test-key.pem"),
                }),
                insecure_plaintext: false,
            },
        )
        .unwrap();
    });
    thread::sleep(Duration::from_millis(100));
    let endpoint = format!("https://127.0.0.1:{port}/v1/frames");
    let transport_a = HttpTransport::secure(
        endpoint.clone(),
        Duration::from_secs(5),
        RequestSigner::new(signing_a.clone()),
        Some(&fixture("test-cert.pem")),
    )
    .unwrap();
    let machine_a = RequestSigner::new(signing_a.clone()).machine().to_owned();
    let old = TempDir::new("https-old");
    let (old_projects, old_source, old_state) = source_file(&old, b"zero\none\n");
    let mut sender_a = Sender::encrypted(machine_a.clone(), age_a.clone(), recipients.clone());
    let report = sender_a
        .run_pass(&old_projects, &old_state, &transport_a)
        .unwrap();
    assert!(!report.had_failures(), "{report:?}");

    let transport_b = HttpTransport::secure(
        endpoint,
        Duration::from_secs(5),
        RequestSigner::new(signing_b.clone()),
        Some(&fixture("test-cert.pem")),
    )
    .unwrap();
    let machine_b = RequestSigner::new(signing_b).machine().to_owned();
    let new = TempDir::new("https-new");
    let (new_projects, new_source, new_state) = source_file(&new, b"zero\none\n");
    let takeover = takeover_session_encrypted(
        &new_projects,
        &new_state,
        "session-a",
        &machine_b,
        true,
        &age_b,
        &transport_b,
    )
    .unwrap();
    assert_eq!(takeover.row.epoch, 1);
    fs::OpenOptions::new()
        .append(true)
        .open(&new_source)
        .unwrap()
        .write_all(b"two\n")
        .unwrap();
    let mut sender_b = Sender::encrypted(machine_b.clone(), age_b, recipients);
    assert!(
        !sender_b
            .run_pass(&new_projects, &new_state, &transport_b)
            .unwrap()
            .had_failures()
    );

    fs::OpenOptions::new()
        .append(true)
        .open(old_source)
        .unwrap()
        .write_all(b"late\n")
        .unwrap();
    let stale = sender_a
        .run_pass(&old_projects, &old_state, &transport_a)
        .unwrap();
    assert!(stale.streams[0].fenced);
    assert_eq!(stale.streams[0].orphan_lines_acked, 1);
    assert_eq!(transport_b.list_orphans(&machine_b).unwrap()[0].frames, 1);

    let envelope = transport_b
        .get_envelope("session-a", &machine_b)
        .unwrap()
        .unwrap();
    let key = decrypt_envelope(&envelope, &recovery).unwrap();
    let frames = transport_b.list_frames("session-a", &machine_b).unwrap();
    let tips = verify_encrypted_frames("session-a", frames, &key).unwrap();
    assert_eq!(tips[0].seq, 2);
    assert_eq!(tips[0].epoch, 1);
}

#[test]
fn http_transport_parses_lease_status_larger_than_four_kibibytes() {
    let temp = TempDir::new("large-lease-status");
    let root = temp.path().join("receiver");
    let signing = signing(31);
    enroll_machine(
        &root,
        &hex::encode(signing.verifying_key().as_bytes()),
        "machine-a",
    )
    .unwrap();
    let signer = RequestSigner::new(signing);
    let machine = signer.machine().to_owned();
    let receiver = Receiver::open(&root).unwrap();
    for index in 0..120 {
        receiver
            .acquire(
                &format!("synthetic-session-{index:03}-with-a-long-name"),
                &machine,
            )
            .unwrap();
    }
    drop(receiver);

    let port = start_http_receiver(root);
    let transport = HttpTransport::secure(
        format!("http://127.0.0.1:{port}/v1/frames"),
        Duration::from_secs(5),
        signer,
        None,
    )
    .unwrap();
    let status = transport.lease_status(None, &machine).unwrap();
    assert_eq!(status.rows.len(), 120);
    assert!(format!("{status:?}").len() > 4096);
}

fn collect_files(path: &Path, output: &mut Vec<Vec<u8>>) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            collect_files(&entry.path(), output);
        } else {
            output.push(fs::read(entry.path()).unwrap());
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn start_http_receiver(root: PathBuf) -> u16 {
    let port = unused_port();
    let listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    thread::spawn(move || {
        serve_configured(listen, &root, ServeConfig::default()).unwrap();
    });
    thread::sleep(Duration::from_millis(100));
    port
}

fn declared_request(port: u16, machine: &str, content_length: usize) -> TcpStream {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "POST /v1/lease/status HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Connection: close\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {content_length}\r\n\
         X-Semon-Machine: {machine}\r\n\
         X-Semon-Timestamp: {}\r\n\
         X-Semon-Nonce: {}\r\n\
         X-Semon-Signature: {}\r\n\r\n",
        unix_ms(),
        "00".repeat(16),
        "00".repeat(64),
    )
    .unwrap();
    stream.flush().unwrap();
    stream
}

fn response_status(mut stream: TcpStream) -> u16 {
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("invalid HTTP response: {response:?}"))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn unused_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

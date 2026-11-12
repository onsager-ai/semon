//! Actual HTTP/1.1 provisioning over TLS, with deterministic local clock/terminal.
use super::*;
use crate::{pairing, provision, provision_wire as wire};
use ring::signature::KeyPair;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use std::sync::atomic::{AtomicU64, Ordering};

type Client = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;
struct Harness {
    dir: auth_tests::PrivateDir,
    shared: Arc<Shared>,
    clock: Arc<AtomicU64>,
    codes: Arc<Mutex<HashMap<String, crate::credentials::Secret>>>,
}
impl Harness {
    fn new() -> Self {
        let dir = auth_tests::PrivateDir::new();
        let controller =
            provision::Controller::open(&dir.0, "https://localhost:443", "laptop".into()).unwrap();
        let clock = Arc::new(AtomicU64::new(100));
        let codes = Arc::new(Mutex::new(HashMap::new()));
        let clock_hook = Arc::clone(&clock);
        let code_hook = Arc::clone(&codes);
        let shared = Arc::new(Shared {
            receiver: Receiver::new(dir.0.clone()).unwrap(),
            tokens: Authenticator::open(&dir.0).unwrap(),
            tls: Some(
                tls_config(
                    &TlsFiles {
                        certificate: auth_tests::fixture("test-cert.pem"),
                        private_key: auth_tests::fixture("test-key.pem"),
                    },
                    &dir.0,
                )
                .unwrap(),
            ),
            limits: Limits {
                linger: Duration::ZERO,
                ..Limits::default()
            },
            admission: Mutex::new(Admission::default()),
            provisioning: Some(Arc::new(Mutex::new(controller))),
            provisioning_clock: Some(Arc::new(move || clock_hook.load(Ordering::Relaxed))),
            provisioning_timeout: Some(Arc::new(std::sync::atomic::AtomicBool::new(false))),
            provisioning_terminal: Some(Arc::new(move |id, code| {
                code_hook
                    .lock()
                    .unwrap()
                    .insert(id.into(), pairing::parse_code(code.expose()).unwrap());
                Ok(())
            })),
        });
        Self {
            dir,
            shared,
            clock,
            codes,
        }
    }
    fn connect(&self) -> (Client, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let mut slot = Slot::admit(&self.shared, "127.0.0.1".parse().unwrap()).unwrap();
        let worker = thread::spawn(move || serve_connection(&mut slot, server, Instant::now()));
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(
                CertificateDer::from_pem_slice(
                    &fs::read(auth_tests::fixture("test-cert.pem")).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connection = rustls::ClientConnection::new(
            Arc::new(config),
            ServerName::try_from("localhost").unwrap(),
        )
        .unwrap();
        (rustls::StreamOwned::new(connection, socket), worker)
    }
    fn input(&self) -> pairing::ActivationInput {
        pairing::ActivationInput {
            machine: "laptop".into(),
            binding: crate::enrollment::Binding::Inapplicable,
            kinds: vec![(Kind::MirrorPush, false)],
            client_certificate_pins: vec![],
            valid_until: 1000,
        }
    }
}
fn post(client: &mut Client, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    write!(client, "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
    client.write_all(body).unwrap();
    client.flush().unwrap();
    response(client)
}
fn response(client: &mut Client) -> (u16, Vec<u8>) {
    let mut head = vec![];
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        client.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .unwrap()
        .parse()
        .unwrap();
    let mut body = vec![0; length];
    client.read_exact(&mut body).unwrap();
    (status, body)
}
fn open(client: &mut Client) -> wire::Opened {
    let (status, body) = post(client, "/v1/provision/open", br#"{"version":1}"#);
    assert_eq!(status, 200);
    serde_json::from_slice(&body).unwrap()
}
fn wrong_confirmation(id: &str) -> Vec<u8> {
    let mut key = [1; 65];
    key[0] = 4;
    wire::encode_message(&wire::Confirm {
        version: 1,
        session_id: id.into(),
        origin_key: wire::encode(&key),
        origin_nonce: wire::encode(&[2; 32]),
        scope_commitment: wire::encode(&[3; 32]),
        origin_mac: wire::encode(&[0; 32]),
    })
    .unwrap()
}
#[test]
fn http_wrong_code_disconnect_cancel_replay_and_creation_gate() {
    let h = Harness::new();
    let (mut client, worker) = h.connect();
    let session = open(&mut client);
    let exporter = client
        .conn
        .export_keying_material([0; 32], wire::EXPORTER_LABEL, Some(&[]))
        .unwrap();
    let (status, _) = post(
        &mut client,
        "/v1/provision/confirm",
        &wrong_confirmation(&session.session_id),
    );
    assert_eq!(status, 401);
    drop(client);
    worker.join().unwrap();
    let controller = h.shared.provisioning.as_ref().unwrap();
    assert_eq!(
        controller.lock().unwrap().pending(100)[0].1,
        pairing::State::Pending
    );
    // Lost response/disconnect cannot turn a refused attempt into a confirmed one;
    // the old session is foreign on every new TLS connection.
    let (mut foreign, b) = h.connect();
    assert_eq!(
        post(
            &mut foreign,
            "/v1/provision/confirm",
            &wrong_confirmation(&session.session_id)
        )
        .0,
        401
    );
    drop(foreign);
    b.join().unwrap();
    {
        let mut controller = controller.lock().unwrap();
        assert!(!controller.reserve_probe(&session.session_id, exporter, 100));
        assert!(!controller.reserve_probe(&session.session_id, exporter, 100));
        assert!(controller.reserve_probe(&session.session_id, exporter, 100));
    }
    for _ in 0..5 {
        let (mut client, worker) = h.connect();
        let session = open(&mut client);
        let cancel = wire::encode_message(&wire::Cancel {
            version: 1,
            session_id: session.session_id,
        })
        .unwrap();
        assert_eq!(post(&mut client, "/v1/provision/cancel", &cancel).0, 200);
        assert_eq!(post(&mut client, "/v1/provision/cancel", &cancel).0, 401);
        drop(client);
        worker.join().unwrap();
    }
    let (mut refused, worker) = h.connect();
    assert_eq!(
        post(&mut refused, "/v1/provision/open", br#"{"version":1}"#).0,
        401
    );
    drop(refused);
    worker.join().unwrap();
    assert!(
        crate::enrollment::Registry::read_current(&h.dir.0)
            .unwrap()
            .enrollments()
            .is_empty()
    );
}
#[test]
fn http_timeout_spends_attempt_without_real_time_wait_or_refund() {
    let h = Harness::new();
    let (mut client, worker) = h.connect();
    let session = open(&mut client);
    let exporter = client
        .conn
        .export_keying_material([0; 32], wire::EXPORTER_LABEL, Some(&[]))
        .unwrap();
    h.shared
        .provisioning_timeout
        .as_ref()
        .unwrap()
        .store(true, Ordering::Relaxed);
    assert_eq!(
        post(
            &mut client,
            "/v1/provision/confirm",
            &wrong_confirmation(&session.session_id)
        )
        .0,
        401
    );
    drop(client);
    worker.join().unwrap();
    let mut controller = h.shared.provisioning.as_ref().unwrap().lock().unwrap();
    assert!(!controller.reserve_probe(&session.session_id, exporter, 100));
    assert!(!controller.reserve_probe(&session.session_id, exporter, 100));
    assert!(controller.reserve_probe(&session.session_id, exporter, 100));
}
#[test]
fn provisioning_inflight_cap_is_separate_and_released_without_session_creation() {
    let h = Harness::new();
    // Distinct IPs avoid the independently enforced unauthenticated per-IP limit.
    let mut slots = Vec::new();
    for i in 1..=wire::MAX_CONNECTIONS {
        let ip = format!("192.0.2.{i}").parse().unwrap();
        let mut slot = Slot::admit(&h.shared, ip).unwrap();
        assert!(slot.admit_provisioning());
        slots.push(slot);
    }
    let mut extra = Slot::admit(&h.shared, "198.51.100.1".parse().unwrap()).unwrap();
    assert!(!extra.admit_provisioning());
    assert!(
        h.shared
            .provisioning
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .pending(100)
            .is_empty()
    );
    slots.pop();
    assert!(extra.admit_provisioning());
    // Total admission still has room for a non-provisioning route connection.
    assert!(Slot::admit(&h.shared, "203.0.113.1".parse().unwrap()).is_some());
}
#[test]
fn http_tls_pairing_activates_and_mirror_stays_serving() {
    let h = Harness::new();
    let (mut client, worker) = h.connect();
    let opened = open(&mut client);
    let exporter = client
        .conn
        .export_keying_material([0; 32], wire::EXPORTER_LABEL, Some(&[]))
        .unwrap();
    let random = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        &random,
    )
    .unwrap();
    let key = ring::signature::EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &random,
    )
    .unwrap();
    let input = h.input();
    let transcript = pairing::Transcript {
        version: 1,
        session_id: opened.session_id.clone(),
        endpoint: opened.endpoint,
        exporter: exporter.to_vec(),
        origin_key: key.public_key().as_ref().to_vec(),
        scope: input.commitment(),
        origin_nonce: [2; 32],
        receiver_nonce: wire::decode_fixed(&opened.receiver_nonce).unwrap(),
        origin_role: "origin".into(),
        receiver_role: "receiver".into(),
        profile: "public_tls".into(),
    };
    let code = h
        .codes
        .lock()
        .unwrap()
        .remove(&transcript.session_id)
        .unwrap();
    let keys = pairing::derive_keys(&code, &transcript).unwrap();
    let confirm = wire::Confirm {
        version: 1,
        session_id: transcript.session_id.clone(),
        origin_key: wire::encode(&transcript.origin_key),
        origin_nonce: wire::encode(&transcript.origin_nonce),
        scope_commitment: wire::encode(&transcript.scope),
        origin_mac: wire::encode(keys.origin_confirmation(&transcript).expose()),
    };
    let (status, body) = post(
        &mut client,
        "/v1/provision/confirm",
        &wire::encode_message(&confirm).unwrap(),
    );
    assert_eq!(status, 200);
    let confirmed: wire::Confirmed = serde_json::from_slice(&body).unwrap();
    let _key = keys
        .confirm_receiver(
            &transcript,
            &pairing::Mac::from_bytes(wire::decode_fixed(&confirmed.receiver_mac).unwrap()),
        )
        .unwrap();
    let signature = key
        .sign(&random, &wire::possession_message(&transcript))
        .unwrap();
    let approve = wire::Approve {
        version: 1,
        session_id: transcript.session_id,
        proof: wire::encode(signature.as_ref()),
        input,
    };
    let (status, body) = post(
        &mut client,
        "/v1/provision/approve",
        &wire::encode_message(&approve).unwrap(),
    );
    assert_eq!(status, 200);
    let activated: wire::Activated = serde_json::from_slice(&body).unwrap();
    // Another route continues on the same shared listener while provisioning is enabled.
    let (mut mirror, other) = h.connect();
    write!(mirror,"POST /v1/mirror/status HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",activated.credentials[0].1).unwrap();
    mirror.flush().unwrap();
    let (status, _) = response(&mut mirror);
    assert_ne!(status, 401);
    drop(mirror);
    other.join().unwrap();
    drop(client);
    worker.join().unwrap();
    assert_eq!(
        crate::enrollment::Registry::read_current(&h.dir.0)
            .unwrap()
            .enrollments()
            .len(),
        1
    );
}
#[test]
fn http_expiry_and_cross_connection_cancel_are_generic_and_do_not_activate() {
    let h = Harness::new();
    let (mut first, a) = h.connect();
    let session = open(&mut first);
    let (mut foreign, b) = h.connect();
    let cancel = wire::Cancel {
        version: 1,
        session_id: session.session_id.clone(),
    };
    let (status, body) = post(
        &mut foreign,
        "/v1/provision/cancel",
        &wire::encode_message(&cancel).unwrap(),
    );
    assert_eq!(status, 401);
    assert_eq!(body, br#"{"error":"request refused"}"#);
    drop(foreign);
    b.join().unwrap();
    h.clock.store(700, Ordering::Relaxed);
    let (status, _) = post(
        &mut first,
        "/v1/provision/cancel",
        &wire::encode_message(&cancel).unwrap(),
    );
    assert_eq!(status, 401);
    drop(first);
    a.join().unwrap();
    assert!(
        crate::enrollment::Registry::read_current(&h.dir.0)
            .unwrap()
            .enrollments()
            .is_empty()
    );
}

#[test]
fn http_analytics_activation_is_explicitly_unavailable_without_side_effects() {
    let h = Harness::new();
    let (mut client, worker) = h.connect();
    let created = open(&mut client);
    let registry = crate::enrollment::Registry::read_current(&h.dir.0).unwrap();
    let acl = fs::read(
        h.dir
            .0
            .join(crate::receiver_custody::ACL)
            .join(crate::receiver_acl::ACL_FILE),
    )
    .unwrap();
    let mut input = h.input();
    input.kinds = vec![(Kind::AnalyticsPublish, false)];
    let approve = wire::Approve {
        version: 1,
        session_id: created.session_id,
        proof: wire::encode(&[0; 64]),
        input,
    };
    let (status, body) = post(
        &mut client,
        "/v1/provision/approve",
        &wire::encode_message(&approve).unwrap(),
    );
    assert_eq!(status, 409);
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("available after origin connect / producer admission lands")
    );
    drop(client);
    worker.join().unwrap();
    assert_eq!(
        crate::enrollment::Registry::read_current(&h.dir.0).unwrap(),
        registry
    );
    assert_eq!(
        fs::read(
            h.dir
                .0
                .join(crate::receiver_custody::ACL)
                .join(crate::receiver_acl::ACL_FILE)
        )
        .unwrap(),
        acl
    );
}

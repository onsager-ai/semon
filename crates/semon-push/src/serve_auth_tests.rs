use super::*;
use crate::{
    credentials::{Bearer, Secret},
    enrollment::{Binding, CertificatePin, Enrollment, KindScope, Registry, Store},
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use std::{
    io::Cursor,
    sync::atomic::{AtomicU64, Ordering},
};

#[test]
fn revocation_between_requests_on_one_live_connection() {
    let dir = PrivateDir::new();
    install(&dir.0, &records(false));
    let mut options = ServeOptions::new(&dir.0);
    options.listen = "127.0.0.1:0".parse().unwrap();
    let server = Server::bind(&options).unwrap().with_limits(Limits {
        linger: Duration::ZERO,
        ..Limits::default()
    });
    let mut client = TcpStream::connect(server.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let (socket, _) = server.listener.accept().unwrap();
    let shared = Arc::new(Shared {
        receiver: server.receiver,
        tokens: server.tokens,
        tls: server.tls,
        limits: server.limits,
        admission: Mutex::new(Admission::default()),
    });
    let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
    let worker = thread::spawn(move || serve_connection(&mut slot, socket, Instant::now()));
    let request = b"POST /v1/mirror/status HTTP/1.1\r\nAuthorization: Bearer test-bearer\r\nContent-Length: 2\r\n\r\n{}";
    client.write_all(request).unwrap();
    fn response(client: &mut TcpStream) -> u16 {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            client.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        let head = String::from_utf8(head).unwrap();
        let length: usize = head
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .unwrap()
            .parse()
            .unwrap();
        client.read_exact(&mut vec![0; length]).unwrap();
        head.split_whitespace().nth(1).unwrap().parse().unwrap()
    }
    assert_ne!(response(&mut client), 401);
    let mut store = Store::open(&dir.0).unwrap();
    let mut next = store.snapshot().unwrap();
    next.credentials[0].revoked_at = Some(100);
    store.commit(next.revision(), &next).unwrap();
    client.write_all(request).unwrap();
    assert_eq!(response(&mut client), 401);
    drop(client);
    worker.join().unwrap();
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct PrivateDir(PathBuf);
impl PrivateDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "semon-listener-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        prepare_private_dir(&dir, true).unwrap();
        Self(dir)
    }
}
impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn identity() -> VerifiedClientIdentity {
    let bytes = fs::read(fixture("client-auth/client.pem")).unwrap();
    let cert = CertificateDer::from_pem_slice(&bytes).unwrap();
    VerifiedClientIdentity::from_verified_der(&cert).unwrap()
}
fn records(cert_required: bool) -> Registry {
    let enrollment = Enrollment {
        enrollment_id: "a".repeat(64),
        receiver_id: "receiver-1".into(),
        machine: "laptop".into(),
        binding: Binding::Inapplicable,
        client_certificate_pins: vec![CertificatePin {
            pin: identity().spki().clone(),
            generation: 1,
            revoked_at: None,
        }],
        credential_kinds: vec![KindScope {
            client_cert_required: cert_required,
            ..KindScope::new(Kind::MirrorPush, 1)
        }],
        revoked_at: None,
    };
    let bearer = Bearer {
        credential_id: "b".repeat(64),
        enrollment_id: enrollment.enrollment_id.clone(),
        kind: Kind::MirrorPush,
        sha256: Secret::new("test-bearer".into()).hash(),
        generation: 1,
        valid_from: 0,
        valid_until: u64::MAX,
        revoked_at: None,
    };
    Registry::from_records(vec![enrollment], vec![bearer]).unwrap()
}
fn install(dir: &Path, registry: &Registry) {
    let mut store = Store::initialize(dir).unwrap();
    store.commit(0, registry).unwrap();
}
fn authorize(
    auth: &RegistryAuthenticator,
    conn: &mut ConnectionAuthority,
    kind: Kind,
) -> Option<AuthenticatedEnrollment> {
    auth.authenticated_enrollment(conn, Some("Bearer test-bearer"), kind, 100)
}
#[test]
fn current_registry_and_connection_binding_and_revision() {
    let dir = PrivateDir::new();
    install(&dir.0, &records(false));
    let auth = RegistryAuthenticator::open(&dir.0).unwrap();
    let mut conn = ConnectionAuthority::default();
    let first = authorize(&auth, &mut conn, Kind::MirrorPush).unwrap();
    assert!(first.revocation.is_current());
    assert!(authorize(&auth, &mut conn, Kind::AnalyticsPublish).is_none());
    assert!(authorize(&auth, &mut conn, Kind::BodyChannel).is_none());
    let mut store = Store::open(&dir.0).unwrap();
    let mut next = store.snapshot().unwrap();
    next.enrollments[0].revoked_at = Some(100);
    store.commit(next.revision(), &next).unwrap();
    assert!(!first.revocation.is_current());
    assert!(authorize(&auth, &mut conn, Kind::MirrorPush).is_none());
}
#[test]
fn certificate_and_credential_must_match_and_no_switch() {
    let dir = PrivateDir::new();
    let mut registry = records(true);
    let mut second = registry.enrollments[0].clone();
    second.enrollment_id = "c".repeat(64);
    second.machine = "desktop".into();
    second.client_certificate_pins.clear();
    second.credential_kinds[0].client_cert_required = false;
    let mut credential = registry.credentials[0].clone();
    credential.credential_id = "d".repeat(64);
    credential.enrollment_id = second.enrollment_id.clone();
    credential.sha256 = Secret::new("second-bearer".into()).hash();
    registry.enrollments.push(second);
    registry.credentials.push(credential);
    install(&dir.0, &registry);
    let auth = RegistryAuthenticator::open(&dir.0).unwrap();
    let mut conn = ConnectionAuthority::default();
    assert!(authorize(&auth, &mut conn, Kind::MirrorPush).is_none());
    conn.verified_client_identity = Some(identity());
    assert!(
        auth.authenticated_enrollment(&mut conn, None, Kind::MirrorPush, 100)
            .is_none()
    );
    assert!(authorize(&auth, &mut conn, Kind::MirrorPush).is_some());
    conn.verified_client_identity = None;
    assert!(
        auth.authenticated_enrollment(
            &mut conn,
            Some("Bearer second-bearer"),
            Kind::MirrorPush,
            100
        )
        .is_none()
    );
    let mut other = ConnectionAuthority::default();
    assert!(
        auth.authenticated_enrollment(
            &mut other,
            Some("Bearer second-bearer"),
            Kind::MirrorPush,
            100
        )
        .is_some()
    );
    let foreign = fs::read(fixture("client-auth/foreign.pem")).unwrap();
    conn.verified_client_identity = Some(
        VerifiedClientIdentity::from_verified_der(
            &CertificateDer::from_pem_slice(&foreign).unwrap(),
        )
        .unwrap(),
    );
    assert!(authorize(&auth, &mut conn, Kind::MirrorPush).is_none());
}
#[test]
fn mixed_custody_refuses_startup_without_touching_files() {
    let dir = PrivateDir::new();
    install(&dir.0, &records(false));
    let registry_path = dir.0.join("enrollment/registry.json");
    let before = fs::read(&registry_path).unwrap();
    let tokens = b"untouched legacy bytes";
    fs::write(dir.0.join("tokens"), tokens).unwrap();
    assert!(Server::bind(&ServeOptions::new(&dir.0)).is_err());
    assert_eq!(fs::read(registry_path).unwrap(), before);
    assert_eq!(fs::read(dir.0.join("tokens")).unwrap(), tokens);
}
fn tls_pair(
    client_cert: Option<(&str, &str)>,
) -> (rustls::ServerConnection, rustls::ClientConnection) {
    use std::os::unix::fs::PermissionsExt;
    let dir = PrivateDir::new();
    let trust = dir.0.join(CLIENT_TRUST_FILE);
    fs::copy(fixture("client-auth/ca.pem"), &trust).unwrap();
    fs::set_permissions(trust, fs::Permissions::from_mode(0o600)).unwrap();
    let server = tls_config(
        &TlsFiles {
            certificate: fixture("test-cert.pem"),
            private_key: fixture("test-key.pem"),
        },
        &dir.0,
    )
    .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(&fs::read(fixture("test-cert.pem")).unwrap()).unwrap())
        .unwrap();
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots);
    let client = match client_cert {
        None => builder.with_no_client_auth(),
        Some((cert, key)) => builder
            .with_client_auth_cert(
                vec![CertificateDer::from_pem_slice(&fs::read(fixture(cert)).unwrap()).unwrap()],
                PrivateKeyDer::from_pem_slice(&fs::read(fixture(key)).unwrap()).unwrap(),
            )
            .unwrap(),
    };
    (
        rustls::ServerConnection::new(server).unwrap(),
        rustls::ClientConnection::new(Arc::new(client), ServerName::try_from("localhost").unwrap())
            .unwrap(),
    )
}
fn handshake(
    server: &mut rustls::ServerConnection,
    client: &mut rustls::ClientConnection,
) -> Result<(), rustls::Error> {
    for _ in 0..16 {
        let mut bytes = Vec::new();
        client.write_tls(&mut bytes).unwrap();
        if !bytes.is_empty() {
            server.read_tls(&mut Cursor::new(bytes)).unwrap();
            server.process_new_packets()?;
        }
        let mut bytes = Vec::new();
        server.write_tls(&mut bytes).unwrap();
        if !bytes.is_empty() {
            client.read_tls(&mut Cursor::new(bytes)).unwrap();
            client.process_new_packets()?;
        }
        if !server.is_handshaking() && !client.is_handshaking() {
            return Ok(());
        }
    }
    panic!("handshake made no progress");
}
#[test]
fn optional_verified_tls_and_foreign_refusal_and_known_pins() {
    let (mut server, mut client) = tls_pair(None);
    handshake(&mut server, &mut client).unwrap();
    assert!(server.peer_certificates().is_none());
    let (mut server, mut client) = tls_pair(Some((
        "client-auth/client.pem",
        "client-auth/client-key.pem",
    )));
    handshake(&mut server, &mut client).unwrap();
    let verified =
        VerifiedClientIdentity::from_verified_der(&server.peer_certificates().unwrap()[0]).unwrap();
    let expected = "b8699d8cd2e33c9b795b1c59868fd78cdc5423ab0600362882d6ee8125e26608";
    let Pin::Spki(hash) = verified.spki() else {
        panic!("SPKI tag")
    };
    assert_eq!(
        hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        expected
    );
    assert_eq!(verified, identity());
    let (mut server, mut client) = tls_pair(Some((
        "client-auth/foreign.pem",
        "client-auth/foreign-key.pem",
    )));
    assert!(handshake(&mut server, &mut client).is_err());
}
#[test]
fn unknown_routes_refuse_with_no_body_read_and_one_response_class() {
    let dir = PrivateDir::new();
    let shared = Arc::new(Shared {
        receiver: Receiver::new(dir.0.clone()).unwrap(),
        tokens: Authenticator::open(&dir.0).unwrap(),
        tls: None,
        limits: Limits::default(),
        admission: Mutex::new(Admission::default()),
    });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    // Body remains in the socket, rather than in Connection's buffer.
    client
        .write_all(b"POST /unknown HTTP/1.1\r\nContent-Length: 4\r\n\r\nbody")
        .unwrap();
    let mut conn = Connection {
        stream: Stream::Plain(Timed {
            socket,
            deadline: Instant::now() + Duration::from_secs(10),
            gap: Duration::from_secs(10),
        }),
        authority: ConnectionAuthority::default(),
        head_only: false,
        buffer: vec![],
        limits: Limits::default(),
    };
    let head = conn.read_head(Some(Instant::now())).unwrap();
    assert!(conn.buffer.is_empty());
    let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
    assert!(matches!(
        answer(&shared, &mut slot, &mut conn, &head),
        Next::Refuse(401, "request refused", _)
    ));
    let mut body = [0; 4];
    conn.stream.read_exact(&mut body).unwrap();
    assert_eq!(&body, b"body");
    for path in [
        "/v1/asi/x",
        "/v1/analytics/x",
        "/v1/body/x",
        "/v1/mirror/status",
    ] {
        let head = Head {
            method: "POST".into(),
            target: path.into(),
            content_length: Some(0),
            ..Head::default()
        };
        assert!(matches!(
            answer(&shared, &mut slot, &mut conn, &head),
            Next::Refuse(401, "request refused", _)
        ));
    }
}

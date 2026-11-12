//! Origin half of public-CA TLS provisioning. No bootstrap trust profile.
use crate::{credentials::Kind, enrollment::Binding, pairing, provision_wire as wire};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{EcdsaKeyPair, KeyPair},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, String>;
const REFUSED: &str =
    "receiver refused provisioning (wrong code, expired session or invalid approval)";

/// Option C's producer-startup seam. Option B can replace this decision without
/// changing provisioning or creating a namespace as a side effect of pairing.
/// Single swap point for explicit initialization option B, tracked in #652.
/// Call before opening any transport; state_dir is beside the materialized index.
pub fn producer_admission(kinds: &[Kind], state_dir: &std::path::Path) -> Result<()> {
    if kinds.iter().any(|k| *k != Kind::MirrorPush) {
        if semon_sessions::read_analytics_namespace(state_dir)?.is_none() {
            return Err("run `semon analytics materialize` first, then connect".into());
        }
        return Err("analytics/body enrollment available after producer admission / client-certificate issuance lands".into());
    }
    Ok(())
}

/// Implementations must obtain the code from a non-echoing controlling terminal.
/// Summary is called only after mutual code confirmation; false cancels approval.
pub trait Terminal {
    fn code(&mut self) -> Result<crate::credentials::Secret>;
    fn approve(&mut self, receiver: &str, receiver_id: &str, machine: &str) -> Result<bool>;
    /// Exact currently discovered native files approved in the summary.
    fn sources(&self) -> Vec<std::path::PathBuf>;
}

pub(crate) struct Connection {
    pub(crate) stream: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    pub(crate) host: String,
    pub(crate) exporter: [u8; 32],
    #[cfg(test)]
    pub(crate) fault: Option<&'static str>,
}
impl Connection {
    pub(crate) fn open(endpoint: &str, roots: rustls::RootCertStore) -> Result<Self> {
        let url = reqwest::Url::parse(endpoint).map_err(|_| "invalid receiver URL")?;
        let host = url.host_str().ok_or("receiver URL has no host")?.to_owned();
        let port = url
            .port_or_known_default()
            .ok_or("receiver URL has no port")?;
        let addresses = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| "receiver DNS lookup failed")?;
        let started = Instant::now();
        let mut connected = None;
        for address in addresses {
            let remaining = remaining_response(&|| started.elapsed())?;
            if let Ok(socket) = TcpStream::connect_timeout(&address, remaining) {
                connected = Some(socket);
                break;
            }
        }
        let socket = connected.ok_or("receiver TCP connection failed or timed out")?;
        let timeout = Some(Duration::from_secs(wire::READ_SECONDS));
        socket.set_read_timeout(timeout).map_err(io_error)?;
        socket.set_write_timeout(timeout).map_err(io_error)?;
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| "TLS 1.3 unavailable")?
        .with_root_certificates(roots)
        .with_no_client_auth();
        config.resumption = rustls::client::Resumption::disabled();
        let name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|_| "invalid receiver hostname")?;
        let mut conn = rustls::ClientConnection::new(Arc::new(config), name)
            .map_err(|_| "TLS configuration failed")?;
        let mut socket = socket;
        while conn.is_handshaking() {
            conn.complete_io(&mut socket).map_err(io_error)?;
        }
        let exporter = conn
            .export_keying_material([0; 32], wire::EXPORTER_LABEL, Some(&[]))
            .map_err(|_| "TLS exporter unavailable")?;
        Ok(Self {
            stream: rustls::StreamOwned::new(conn, socket),
            host: format!("{host}:{port}"),
            exporter,
            #[cfg(test)]
            fault: None,
        })
    }
    pub(crate) fn post<T: DeserializeOwned>(
        &mut self,
        path: &str,
        message: &impl Serialize,
    ) -> Result<T> {
        let start = Instant::now();
        self.post_with_clock(path, message, &|| start.elapsed())
    }
    fn post_with_clock<T: DeserializeOwned>(
        &mut self,
        path: &str,
        message: &impl Serialize,
        elapsed: &dyn Fn() -> Duration,
    ) -> Result<T> {
        let mut socket = DeadlineSocket {
            socket: &mut self.stream.sock,
            elapsed,
        };
        let mut stream = rustls::Stream::new(&mut self.stream.conn, &mut socket);
        let bytes = Zeroizing::new(
            wire::encode_message(message).map_err(|_| "invalid provisioning request")?,
        );
        write!(stream, "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", self.host, bytes.len()).map_err(io_error)?;
        stream.write_all(&bytes).map_err(io_error)?;
        stream.flush().map_err(io_error)?;
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            remaining_response(elapsed)?;
            if head.len() >= 8192 {
                return Err("oversized receiver headers".into());
            }
            let mut byte = [0];
            stream.read_exact(&mut byte).map_err(io_error)?;
            head.push(byte[0]);
        }
        let head = std::str::from_utf8(&head).map_err(|_| "invalid receiver headers")?;
        if head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            != Some("200")
        {
            return Err(REFUSED.into());
        }
        let mut length = None;
        for line in head.lines().skip(1) {
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("transfer-encoding") {
                    return Err("unsupported receiver framing".into());
                }
                if name.eq_ignore_ascii_case("content-length") {
                    if length.is_some() {
                        return Err("duplicate receiver length".into());
                    }
                    length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .map_err(|_| "invalid receiver length")?,
                    );
                }
            }
        }
        let length = length
            .filter(|n| *n <= wire::MAX_MESSAGE_BYTES)
            .ok_or("invalid receiver length")?;
        let mut body = Zeroizing::new(vec![0; length]);
        let mut offset = 0;
        while offset < body.len() {
            remaining_response(elapsed)?;
            let count = stream.read(&mut body[offset..]).map_err(io_error)?;
            if count == 0 {
                return Err("receiver response interrupted".into());
            }
            offset += count;
        }
        #[cfg(test)]
        {
            if path.ends_with("/approve") && self.fault == Some("delivery") {
                return Err("injected delivery interruption".into());
            }
            if (path.ends_with("/open") && self.fault == Some("receiver_id"))
                || (path.ends_with("/confirm") && self.fault == Some("receiver_mac"))
            {
                let mut value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                if path.ends_with("/open") {
                    value["receiver_id"] = "substituted".into();
                } else {
                    value["receiver_mac"] = wire::encode(&[0; 32]).into();
                }
                return serde_json::from_value(value)
                    .map_err(|_| "invalid injected response".into());
            }
        }
        serde_json::from_slice(&body).map_err(|_| "invalid receiver response".into())
    }
}

/// Bound each ciphertext operation, including rustls's internal fragment reads.
struct DeadlineSocket<'a> {
    socket: &'a mut TcpStream,
    elapsed: &'a dyn Fn() -> Duration,
}
impl DeadlineSocket<'_> {
    fn remaining(&self) -> std::io::Result<Duration> {
        remaining_response(self.elapsed).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "receiver response deadline exceeded",
            )
        })
    }
}
impl Read for DeadlineSocket<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket.set_read_timeout(Some(self.remaining()?))?;
        self.socket.read(bytes)
    }
}
impl Write for DeadlineSocket<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.socket.set_write_timeout(Some(self.remaining()?))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.set_write_timeout(Some(self.remaining()?))?;
        self.socket.flush()
    }
}

fn io_error(_: std::io::Error) -> String {
    "receiver TLS connection interrupted or certificate validation failed".into()
}
fn roots() -> rustls::RootCertStore {
    rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

pub fn connect(
    endpoint: &str,
    machine: &str,
    terminal: &mut dyn Terminal,
    custody: &std::path::Path,
    now: &dyn Fn() -> u64,
) -> Result<String> {
    let endpoint = pairing::normalize_endpoint(endpoint)
        .map_err(|_| "connect requires an HTTPS receiver URL without path, credentials or query")?;
    connect_with(&endpoint, machine, terminal, custody, now, &mut || {
        Connection::open(&endpoint, roots())
    })
}

pub(crate) fn connect_with(
    endpoint: &str,
    machine: &str,
    terminal: &mut dyn Terminal,
    custody: &std::path::Path,
    now: &dyn Fn() -> u64,
    transport: &mut dyn FnMut() -> Result<Connection>,
) -> Result<String> {
    if !semon_sessions::is_machine_name(machine) {
        return Err("invalid machine name; use --machine NAME".into());
    }
    producer_admission(&[Kind::MirrorPush], custody)?;
    let mut connection = transport()?;
    let opened: wire::Opened = connection.post(
        "/v1/provision/open",
        &wire::Open {
            version: wire::VERSION,
        },
    )?;
    if opened.version != wire::VERSION
        || opened.endpoint != endpoint
        || opened.receiver_id.is_empty()
    {
        return Err("invalid receiver identity".into());
    }
    let code = terminal.code()?;
    // Local check digit validation precedes confirm, so malformed input spends no attempt.
    let code = pairing::parse_code(code.expose())
        .map_err(|_| "invalid pairing code/check digit; no attempt submitted")?;
    let random = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &random)
            .map_err(|_| "origin key generation failed")?;
    let private = Zeroizing::new(pkcs8.as_ref().to_vec());
    let key = EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        &private,
        &random,
    )
    .map_err(|_| "origin key generation failed")?;
    let input = pairing::ActivationInput {
        receiver_id: opened.receiver_id.clone(),
        machine: machine.into(),
        binding: Binding::Inapplicable,
        kinds: vec![(Kind::MirrorPush, false)],
        client_certificate_pins: vec![],
        valid_until: now().saturating_add(wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS),
    };
    let mut nonce = [0; 32];
    random
        .fill(&mut nonce)
        .map_err(|_| "origin randomness failed")?;
    let confirm = wire::Confirm {
        version: wire::VERSION,
        session_id: opened.session_id.clone(),
        origin_key: wire::encode(key.public_key().as_ref()),
        origin_nonce: wire::encode(&nonce),
        scope_commitment: wire::encode(&input.commitment()),
        origin_mac: String::new(),
    };
    let transcript = confirm
        .transcript(
            endpoint.into(),
            wire::decode_fixed(&opened.receiver_nonce).map_err(|_| "invalid receiver nonce")?,
            connection.exporter,
        )
        .map_err(|_| "invalid receiver transcript")?;
    let keys =
        pairing::derive_keys(&code, &transcript).map_err(|_| "invalid pairing transcript")?;
    let confirm = wire::Confirm {
        origin_mac: wire::encode(keys.origin_confirmation(&transcript).expose()),
        ..confirm
    };
    let response: wire::Confirmed = connection.post("/v1/provision/confirm", &confirm)?;
    if response.version != wire::VERSION {
        return Err("invalid receiver version".into());
    }
    let confirmed = keys
        .confirm_receiver(
            &transcript,
            &pairing::Mac::from_bytes(
                wire::decode_fixed(&response.receiver_mac)
                    .map_err(|_| "invalid receiver confirmation")?,
            ),
        )
        .map_err(|_| "receiver confirmation failed; nothing installed")?;
    if !terminal.approve(endpoint, &opened.receiver_id, machine)? {
        let _: Result<wire::Cancelled> = connection.post(
            "/v1/provision/cancel",
            &wire::Cancel {
                version: wire::VERSION,
                session_id: opened.session_id,
            },
        );
        return Err("enrollment declined; nothing installed".into());
    }
    let signature = key
        .sign(&random, &wire::possession_message(&transcript))
        .map_err(|_| "origin approval signature failed")?;
    let approval = wire::Approve {
        version: wire::VERSION,
        session_id: opened.session_id,
        proof: wire::encode(signature.as_ref()),
        input,
    };
    let delivery: wire::Activated = match connection.post("/v1/provision/approve", &approval) {
        Ok(delivery) => delivery,
        Err(error) if error == REFUSED => return Err(error),
        Err(_) => {
            drop(connection);
            let mut recovery = transport()?;
            let mut signed = b"semon-asi-recovery-pop-v1\0".to_vec();
            signed.extend_from_slice(&transcript.digest());
            signed.extend_from_slice(transcript.session_id.as_bytes());
            signed.extend_from_slice(&recovery.exporter);
            let signature = key
                .sign(&random, &signed)
                .map_err(|_| "origin recovery signature failed")?;
            recovery.post(
                "/v1/provision/recover",
                &wire::Recover {
                    version: wire::VERSION,
                    transcript: wire::RecoveryTranscript {
                        session_id: transcript.session_id.clone(),
                        endpoint: transcript.endpoint.clone(),
                        exporter: wire::encode(&transcript.exporter),
                        origin_key: wire::encode(&transcript.origin_key),
                        scope_commitment: wire::encode(&transcript.scope),
                        origin_nonce: wire::encode(&transcript.origin_nonce),
                        receiver_nonce: wire::encode(&transcript.receiver_nonce),
                    },
                    application_key: wire::encode(confirmed.application_key()),
                    proof: wire::encode(signature.as_ref()),
                },
            )?
        }
    };
    if delivery.version != wire::VERSION
        || delivery.credentials.len() != 1
        || delivery.credentials[0].0 != Kind::MirrorPush
    {
        return Err("invalid delivered credential kinds".into());
    }
    crate::origin_custody::install(
        custody,
        endpoint,
        machine,
        &approval.input,
        &delivery,
        &private,
        &terminal.sources(),
    )
}

fn remaining_response(elapsed: &dyn Fn() -> Duration) -> Result<Duration> {
    Duration::from_secs(wire::READ_SECONDS)
        .checked_sub(elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| "receiver response deadline exceeded".into())
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    #[test]
    fn socket_fragments_share_one_total_budget() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        peer.write_all(b"fragmented-record").unwrap();
        let ticks = std::cell::Cell::new(0);
        let clock = || {
            let tick = ticks.get();
            ticks.set(tick + 1);
            Duration::from_secs(tick * 4)
        };
        let mut deadline = DeadlineSocket {
            socket: &mut socket,
            elapsed: &clock,
        };
        let mut byte = [0];
        for _ in 0..3 {
            assert_eq!(deadline.read(&mut byte).unwrap(), 1);
        }
        assert_eq!(
            deadline.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(
            deadline.write(b"x").unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
    }
    #[test]
    fn producer_preflight_is_read_only_and_refuses_before_pairing() {
        use std::os::unix::fs::PermissionsExt;
        let state =
            std::env::temp_dir().join(format!("semon-producer-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        for kind in [Kind::AnalyticsPublish, Kind::BodyChannel] {
            assert_eq!(
                producer_admission(&[kind], &state).unwrap_err(),
                "run `semon analytics materialize` first, then connect"
            );
        }
        assert!(!state.exists());
        assert!(producer_admission(&[Kind::MirrorPush], &state).is_ok());
        assert!(!state.exists());
        std::fs::create_dir(&state).unwrap();
        let path = state.join("analytics-namespace");
        let namespace = format!("origin-{}", "a".repeat(64));
        std::fs::write(&path, &namespace).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        for kind in [Kind::AnalyticsPublish, Kind::BodyChannel] {
            assert_eq!(
                producer_admission(&[kind], &state).unwrap_err(),
                "analytics/body enrollment available after producer admission / client-certificate issuance lands"
            );
        }
        assert_eq!(std::fs::read_to_string(path).unwrap(), namespace);
        assert_eq!(std::fs::read_dir(&state).unwrap().count(), 1);
        std::fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn elapsed_response_budget_is_not_reset_per_byte() {
        let elapsed = std::cell::Cell::new(Duration::ZERO);
        assert!(remaining_response(&|| elapsed.get()).is_ok());
        elapsed.set(Duration::from_secs(wire::READ_SECONDS));
        assert!(remaining_response(&|| elapsed.get()).is_err());
    }
}

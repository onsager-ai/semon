//! `semon receive`'s listener: the mirror protocol over HTTP/1.1, plain on
//! loopback or TLS with an operator-supplied certificate and key. It serves
//! mirror routes to [`Receiver`], protected provisioning, and the closed reverse
//! body poll/response routes when body custody is configured.
//!
//! Each connection has its own thread, up to [`Limits::connections`] in
//! all. Until one of its requests passes the token check, a connection also
//! counts against its address (an IPv6 address by its /64), which may hold
//! [`Limits::connections_per_ip`]; one more over either is closed as soon
//! as it is accepted. Once authenticated, it counts only in the total, so
//! several machines behind one address (an SSH tunnel, a NAT) each keep
//! their connection. A request's token is checked, and its
//! declared length against [`MAX_BODY_BYTES`], before any of its body is
//! read. Every read and write of the socket, the TLS handshake's included,
//! runs against a deadline (see [`Limits`]), so a client that trickles bytes
//! can't hold a connection past it. Semon never opens a port in a firewall
//! or obtains a certificate.

use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Write},
    net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

#[path = "serve_auth.rs"]
mod auth;
#[path = "serve_body.rs"]
mod body;
use crate::credentials::Kind;
use auth::Authenticator;
pub use auth::{
    AuthenticatedEnrollment, ConnectionAuthority, CredentialHandle, RegistryAuthenticator,
    VerifiedClientIdentity,
};

use crate::{
    mirror::{DEFAULT_MAX_BYTES, Endpoint, MAX_BODY_BYTES, Receiver},
    tokens::prepare_private_dir,
};

/// Where `semon receive` listens unless told otherwise.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8735";
/// The largest request head (request line and headers).
pub const HEAD_MAX_BYTES: usize = 16 * 1024;
/// The most header lines in one request.
const MAX_HEADERS: usize = 64;
/// What an early close reads and drops at most, so the client gets the
/// answer rather than a reset.
const LINGER_BYTES: usize = 256 * 1024;

/// How many connections are served, and how long each may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Connections served at once (32).
    pub connections: usize,
    /// Connections not yet authenticated from one IP address (an IPv6
    /// address by its /64) at once (4).
    pub connections_per_ip: usize,
    /// From accepting a connection to the end of its first request's head,
    /// the TLS handshake included; for a later request, from its first byte
    /// (10 s).
    pub head: Duration,
    /// The longest wait for the next request on a kept-alive connection
    /// (30 s).
    pub idle: Duration,
    /// The longest a single read or write may wait (30 s).
    pub gap: Duration,
    /// From the end of a request's head to the end of its body (120 s).
    pub body: Duration,
    /// To write a response (30 s).
    pub write: Duration,
    /// How long an early close reads and drops what the client still sends
    /// (2 s).
    pub linger: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connections: 32,
            connections_per_ip: 4,
            head: Duration::from_secs(10),
            idle: Duration::from_secs(30),
            gap: Duration::from_secs(30),
            body: Duration::from_secs(120),
            write: Duration::from_secs(30),
            linger: Duration::from_secs(2),
        }
    }
}

impl Limits {
    /// These limits with no zero in them: a count is at least 1 and a
    /// duration at least 1 ms (a zero timeout can't be set on a socket).
    /// [`Limits::linger`] may be zero.
    pub fn clamped(self) -> Self {
        let at_least = |duration: Duration| duration.max(Duration::from_millis(1));
        Self {
            connections: self.connections.max(1),
            connections_per_ip: self.connections_per_ip.max(1),
            head: at_least(self.head),
            idle: at_least(self.idle),
            gap: at_least(self.gap),
            body: at_least(self.body),
            write: at_least(self.write),
            linger: self.linger,
        }
    }
}

/// What a connection counts against: its IPv4 address, or its IPv6
/// address's /64 (one host usually holds a whole /64). An IPv4-mapped IPv6
/// address counts as the IPv4 one.
fn ip_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        },
    }
}

/// An operator-supplied certificate chain and private key, PEM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsFiles {
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// The receiver's directory: `tokens` and `machines/`.
    pub dir: PathBuf,
    pub listen: SocketAddr,
    pub tls: Option<TlsFiles>,
    /// The most bytes one machine's copy may hold ([`DEFAULT_MAX_BYTES`]).
    pub max_bytes: u64,
}

impl ServeOptions {
    /// `dir` on the default loopback address, without TLS, with the default
    /// cap.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            listen: DEFAULT_LISTEN.parse().expect("the default address parses"),
            tls: None,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// Who is connected: in all, and, not yet authenticated, from each address.
#[derive(Default)]
struct Admission {
    open: usize,
    provisioning: usize,
    per_ip: HashMap<IpAddr, usize>,
}

#[cfg(test)]
type ProvisioningTerminal = dyn Fn(&str, &crate::credentials::Secret) -> Result<(), crate::provision_wire::Refused>
    + Send
    + Sync;

struct Shared {
    receiver: Receiver,
    tokens: Authenticator,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    admission: Mutex<Admission>,
    body: Option<crate::body_host::BodyHost<'static>>,
    provisioning: Option<Arc<Mutex<crate::provision::Controller>>>,
    #[cfg(test)]
    provisioning_clock: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
    #[cfg(test)]
    provisioning_timeout: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    provisioning_terminal: Option<Arc<ProvisioningTerminal>>,
}

impl Shared {
    fn admission(&self) -> std::sync::MutexGuard<'_, Admission> {
        self.admission
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A connection's place under the limits, given back when it is dropped.
struct Slot {
    shared: Arc<Shared>,
    /// The address it counts against, until it authenticates.
    ip: Option<IpAddr>,
    provisioning: bool,
}

fn release_ip(admission: &mut Admission, ip: IpAddr) {
    if let Some(count) = admission.per_ip.get_mut(&ip) {
        *count -= 1;
        if *count == 0 {
            admission.per_ip.remove(&ip);
        }
    }
}

impl Slot {
    fn admit(shared: &Arc<Shared>, ip: IpAddr) -> Option<Self> {
        let ip = ip_key(ip);
        let mut admission = shared.admission();
        let from_ip = admission.per_ip.get(&ip).copied().unwrap_or(0);
        if admission.open >= shared.limits.connections
            || from_ip >= shared.limits.connections_per_ip
        {
            return None;
        }
        admission.open += 1;
        admission.per_ip.insert(ip, from_ip + 1);
        Some(Self {
            shared: Arc::clone(shared),
            ip: Some(ip),
            provisioning: false,
        })
    }

    fn admit_provisioning(&mut self) -> bool {
        if self.provisioning {
            return true;
        }
        let mut admission = self.shared.admission();
        if admission.provisioning >= crate::provision_wire::MAX_CONNECTIONS {
            return false;
        }
        admission.provisioning += 1;
        self.provisioning = true;
        true
    }

    /// A request on this connection passed the token check: from now on it
    /// counts in the total only, not against its address.
    fn authenticated(&mut self) {
        if let Some(ip) = self.ip.take() {
            release_ip(&mut self.shared.admission(), ip);
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut admission = self.shared.admission();
        admission.open = admission.open.saturating_sub(1);
        if self.provisioning {
            admission.provisioning -= 1;
        }
        if let Some(ip) = self.ip.take() {
            release_ip(&mut admission, ip);
        }
    }
}

/// A bound receiver, ready to [`Server::run`].
pub struct Server {
    listener: TcpListener,
    credential_count: usize,
    receiver: Receiver,
    tokens: Authenticator,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    provisioning: Option<Arc<Mutex<crate::provision::Controller>>>,
}

impl Server {
    /// Checks the options and the directory (created 0700 when missing; an
    /// existing one others can read is refused), and binds.
    pub fn bind(options: &ServeOptions) -> Result<Self, String> {
        let dir = &options.dir;
        if !options.listen.ip().is_loopback() && options.tls.is_none() {
            return Err("non-loopback receiver requires --tls-cert and --tls-key".into());
        }
        prepare_private_dir(dir, true)?;
        let tokens = Authenticator::open(dir)?;
        let credential_count = tokens.count()?;
        let tls = options
            .tls
            .as_ref()
            .map(|files| tls_config(files, dir))
            .transpose()?;
        let listener = TcpListener::bind(options.listen)
            .map_err(|error| format!("cannot listen on {}: {error}", options.listen))?;
        Ok(Self {
            listener,
            credential_count,
            receiver: Receiver::new(dir.clone())
                .map_err(|error| error.to_string())?
                .with_max_bytes(options.max_bytes),
            tokens,
            tls,
            limits: Limits::default(),
            provisioning: None,
        })
    }

    pub fn with_provisioning(
        mut self,
        controller: Arc<Mutex<crate::provision::Controller>>,
    ) -> Result<Self, String> {
        if self.tls.is_none() {
            return Err("pairing requires TLS".into());
        }
        self.provisioning = Some(controller);
        Ok(self)
    }

    /// Other limits than the defaults (tests use short ones), with any
    /// zero raised to the least usable value ([`Limits::clamped`]).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits.clamped();
        self
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// `https` with TLS, `http` without.
    pub fn scheme(&self) -> &'static str {
        if self.tls.is_some() { "https" } else { "http" }
    }

    /// Number of configured credentials observed at startup.
    pub fn tokens(&self) -> usize {
        self.credential_count
    }

    /// Serves until the process ends.
    pub fn run(self) -> Result<(), String> {
        let body = if self.tls.is_some() {
            crate::body_host::BodyHost::configured(
                self.receiver.dir(),
                crate::body_broker::Config {
                    max_connections: self.limits.connections,
                    poll_share_percent: 50,
                },
                &crate::body_host::CLOCK,
            )
            .map_err(|_| "body channel unavailable".to_owned())?
        } else {
            None
        };
        let shared = Arc::new(Shared {
            receiver: self.receiver,
            tokens: self.tokens,
            tls: self.tls,
            limits: self.limits,
            admission: Mutex::new(Admission::default()),
            body,
            provisioning: self.provisioning,
            #[cfg(test)]
            provisioning_clock: None,
            #[cfg(test)]
            provisioning_timeout: None,
            #[cfg(test)]
            provisioning_terminal: None,
        });
        for accepted in self.listener.incoming() {
            let socket = match accepted {
                Ok(socket) => socket,
                Err(error) => {
                    // Out of descriptors, or a connection reset before it
                    // was accepted: wait a little and go on.
                    eprintln!("semon receive: accept: {error}");
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            let accepted = Instant::now();
            let Ok(peer) = socket.peer_addr() else {
                continue;
            };
            // Over a limit: closed at once, never queued.
            let Some(slot) = Slot::admit(&shared, peer.ip()) else {
                drop(socket);
                continue;
            };
            // The slot is given back when the connection ends, or if its
            // thread can't start.
            let spawned = thread::Builder::new()
                .name("semon-receive".into())
                .spawn(move || {
                    let mut slot = slot;
                    serve_connection(&mut slot, socket, accepted);
                });
            if let Err(error) = spawned {
                eprintln!("semon receive: cannot start a connection thread: {error}");
            }
        }
        Ok(())
    }
}

fn tls_config(files: &TlsFiles, dir: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    let read = |path: &Path| {
        fs::read(path).map_err(|error| format!("cannot read TLS file {}: {error}", path.display()))
    };
    let (certificate, private_key) = (read(&files.certificate)?, read(&files.private_key)?);
    let chain = CertificateDer::pem_slice_iter(&certificate)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", files.certificate.display()))?;
    if chain.is_empty() {
        return Err(format!(
            "{} holds no PEM certificate",
            files.certificate.display()
        ));
    }
    let key = PrivateKeyDer::from_pem_slice(&private_key)
        .map_err(|error| format!("{}: {error}", files.private_key.display()))?;
    let trust = client_trust(dir)?;
    let verifier = match trust {
        Some(roots) => rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .allow_unauthenticated()
        .build()
        .map_err(|e| e.to_string())?,
        None => rustls::server::WebPkiClientVerifier::no_client_auth(),
    };
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| error.to_string())?
    .with_client_cert_verifier(verifier)
    .with_single_cert(chain, key)
    .map_err(|error| format!("the TLS certificate and key: {error}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// The sole client-trust input: a private, owned, non-symlink PEM root bundle.
const CLIENT_TRUST_FILE: &str = "client-trust.pem";
fn client_trust(dir: &Path) -> Result<Option<rustls::RootCertStore>, String> {
    use rustls::pki_types::{CertificateDer, pem::PemObject};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let path = dir.join(CLIENT_TRUST_FILE);
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let meta = file.metadata().map_err(|e| e.to_string())?;
    // SAFETY: geteuid has no memory preconditions.
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o777 != 0o600
    {
        return Err(format!(
            "{} must be an owned private 0600 regular file",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("client trust bundle too large".into());
    }
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(&bytes) {
        roots
            .add(cert.map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    if roots.is_empty() {
        return Err("client trust bundle contains no certificates".into());
    }
    Ok(Some(roots))
}

/// The socket, with a deadline that every read and write honors: before
/// each, its timeout is set to what is left of the deadline (at most
/// `gap`), and once the deadline has passed each fails with `TimedOut`.
/// rustls reads through it too, so a TLS record, or a handshake, trickled
/// in a byte at a time still ends at the deadline.
struct Timed {
    #[cfg(test)]
    bytes_read: usize,
    socket: TcpStream,
    deadline: Instant,
    gap: Duration,
}

impl Timed {
    fn remaining(&self) -> io::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(remaining.min(self.gap))
    }
}

impl Read for Timed {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let timeout = self.remaining()?;
        self.socket.set_read_timeout(Some(timeout))?;
        let read = self.socket.read(buffer)?;
        #[cfg(test)]
        {
            self.bytes_read += read;
        }
        Ok(read)
    }
}

impl Write for Timed {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let timeout = self.remaining()?;
        self.socket.set_write_timeout(Some(timeout))?;
        self.socket.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

enum Stream {
    Plain(Timed),
    Tls(Box<rustls::StreamOwned<rustls::ServerConnection, Timed>>),
}

impl Stream {
    fn timed(&mut self) -> &mut Timed {
        match self {
            Self::Plain(timed) => timed,
            Self::Tls(stream) => &mut stream.sock,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

/// One request's head, as far as the receiver cares.
#[derive(Debug, Default, PartialEq, Eq)]
struct Head {
    method: String,
    target: String,
    content_length: Option<u64>,
    transfer_encoding: bool,
    /// The connection ends after this request.
    close: bool,
    expect_continue: bool,
    /// An `Expect` other than `100-continue`.
    expect_other: bool,
    authorization: Option<String>,
    content_type: Option<String>,
}

/// Why no request head was read.
#[derive(Debug, PartialEq, Eq)]
enum NoHead {
    /// The client closed the connection, or it failed, between requests.
    Closed,
    /// Time ran out; `started` when part of a request had arrived.
    TimedOut { started: bool },
    /// The head is malformed (400), too large (431) or not HTTP/1 (505).
    Refused(u16, &'static str),
}

struct Connection {
    stream: Stream,
    head_only: bool,
    authority: ConnectionAuthority,
    /// Bytes read and not yet used.
    buffer: Vec<u8>,
    limits: Limits,
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

impl Connection {
    /// Reads once, by `deadline`.
    fn read_more(&mut self, deadline: Instant, remaining: usize) -> io::Result<usize> {
        self.stream.timed().deadline = deadline;
        let mut chunk = [0u8; 16 * 1024];
        let bound = remaining.min(chunk.len());
        let read = self.stream.read(&mut chunk[..bound])?;
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(read)
    }

    /// The next request's head. The first request's (and the TLS
    /// handshake) must be in within [`Limits::head`] of `accepted`; a later
    /// one may wait [`Limits::idle`] to start, then has [`Limits::head`].
    fn read_head(&mut self, accepted: Option<Instant>) -> Result<Head, NoHead> {
        let now = Instant::now();
        let wait = now + self.limits.idle;
        let mut deadline = match accepted {
            Some(accepted) => Some(accepted + self.limits.head),
            None if !self.buffer.is_empty() => Some(now + self.limits.head),
            None => None,
        };
        let mut searched = 0;
        loop {
            if let Some(at) = find(&self.buffer[searched..], b"\r\n\r\n") {
                let end = searched + at + 4;
                if end > HEAD_MAX_BYTES {
                    return Err(NoHead::Refused(431, "the request head is too large"));
                }
                let head = parse_head(&self.buffer[..end]);
                self.buffer.drain(..end);
                return head.map_err(|(status, message)| NoHead::Refused(status, message));
            }
            if self.buffer.len() >= HEAD_MAX_BYTES {
                return Err(NoHead::Refused(431, "the request head is too large"));
            }
            searched = self.buffer.len().saturating_sub(3);
            self.stream.timed().deadline = deadline.unwrap_or(wait);
            let mut byte = [0u8; 1];
            let read = self.stream.read(&mut byte);
            if let Ok(1) = read {
                self.buffer.push(byte[0]);
            }
            match read {
                Ok(0) => return Err(NoHead::Closed),
                Ok(_) => {
                    let head = self.limits.head;
                    deadline.get_or_insert_with(|| Instant::now() + head);
                }
                Err(error) if is_timeout(&error) => {
                    return Err(NoHead::TimedOut {
                        started: !self.buffer.is_empty(),
                    });
                }
                Err(_) => return Err(NoHead::Closed),
            }
        }
    }

    /// Exactly `length` body bytes, within [`Limits::body`].
    fn read_body(&mut self, length: usize) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + self.limits.body;
        while self.buffer.len() < length {
            if self.read_more(deadline, length - self.buffer.len())? == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
        let rest = self.buffer.split_off(length);
        Ok(std::mem::replace(&mut self.buffer, rest))
    }

    /// Writes `bytes`, within [`Limits::write`].
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.timed().deadline = Instant::now() + self.limits.write;
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }

    fn respond(
        &mut self,
        status: u16,
        body: &Value,
        close: bool,
        headers: &[(&str, &str)],
    ) -> io::Result<()> {
        let body = body.to_string();
        let mut out = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n",
            reason(status),
            body.len()
        );
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        if close {
            out.push_str("Connection: close\r\n");
        }
        out.push_str("\r\n");
        if !self.head_only {
            out.push_str(&body);
        }
        self.send(out.as_bytes())
    }

    /// Answers `status` and closes the connection.
    fn refuse(mut self, status: u16, message: &str, headers: &[(&str, &str)]) {
        let _ = self.respond(status, &json!({"error": message}), true, headers);
        self.close();
    }

    /// Ends the connection: TLS close_notify, no more writes, then what the
    /// client still sends is read and dropped for [`Limits::linger`], so it
    /// reads the answer before the socket closes rather than a reset.
    fn close(mut self) {
        let linger = self.limits.linger;
        if let Stream::Tls(stream) = &mut self.stream {
            stream.sock.deadline = Instant::now() + linger;
            stream.conn.send_close_notify();
            let _ = stream.flush();
        }
        let socket = &mut self.stream.timed().socket;
        let _ = socket.shutdown(Shutdown::Write);
        let deadline = Instant::now() + linger;
        let mut sink = [0u8; 16 * 1024];
        let mut drained = 0;
        while drained < LINGER_BYTES {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || socket.set_read_timeout(Some(remaining)).is_err() {
                break;
            }
            match socket.read(&mut sink) {
                Ok(0) | Err(_) => break,
                Ok(read) => drained += read,
            }
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        417 => "Expectation Failed",
        431 => "Request Header Fields Too Large",
        505 => "HTTP Version Not Supported",
        507 => "Insufficient Storage",
        _ => "Internal Server Error",
    }
}

/// A header name's characters (RFC 9110 `tchar`).
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn parse_head(bytes: &[u8]) -> Result<Head, (u16, &'static str)> {
    const MALFORMED: (u16, &str) = (400, "the request head is malformed");
    let text = std::str::from_utf8(bytes).map_err(|_| MALFORMED)?;
    if text
        .bytes()
        .any(|byte| (byte < 0x20 && !matches!(byte, b'\r' | b'\n' | b'\t')) || byte == 0x7f)
    {
        return Err(MALFORMED);
    }
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or_default();
    if request.contains(['\r', '\n']) {
        return Err(MALFORMED);
    }
    let mut request = request.split(' ');
    let (Some(method), Some(target), Some(version), None) = (
        request.next(),
        request.next(),
        request.next(),
        request.next(),
    ) else {
        return Err(MALFORMED);
    };
    if method.is_empty() || !method.bytes().all(is_token_byte) || target.is_empty() {
        return Err(MALFORMED);
    }
    let http10 = match version {
        "HTTP/1.1" => false,
        "HTTP/1.0" => true,
        _ if version.starts_with("HTTP/") => {
            return Err((505, "only HTTP/1.1 is served"));
        }
        _ => return Err(MALFORMED),
    };
    let mut head = Head {
        method: method.to_owned(),
        target: target.to_owned(),
        ..Head::default()
    };
    let mut keep_alive = false;
    let mut count = 0;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        count += 1;
        if count > MAX_HEADERS {
            return Err((431, "too many request headers"));
        }
        // Bare carriage returns and line feeds, and obsolete line folding,
        // are refused.
        if line.contains(['\r', '\n']) || line.starts_with([' ', '\t']) {
            return Err(MALFORMED);
        }
        let (name, value) = line.split_once(':').ok_or(MALFORMED)?;
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            return Err(MALFORMED);
        }
        let value = value.trim_matches([' ', '\t']);
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(MALFORMED);
                }
                let length = value.parse::<u64>().map_err(|_| MALFORMED)?;
                if head.content_length.is_some_and(|known| known != length) {
                    return Err(MALFORMED);
                }
                head.content_length = Some(length);
            }
            "transfer-encoding" => head.transfer_encoding = true,
            "connection" => {
                for option in value.split(',').map(str::trim) {
                    if option.eq_ignore_ascii_case("close") {
                        head.close = true;
                    } else if option.eq_ignore_ascii_case("keep-alive") {
                        keep_alive = true;
                    }
                }
            }
            "expect" => {
                if value.eq_ignore_ascii_case("100-continue") {
                    head.expect_continue = true;
                } else {
                    head.expect_other = true;
                }
            }
            "content-type" => {
                if head.content_type.is_some() {
                    return Err(MALFORMED);
                }
                head.content_type = Some(value.to_owned());
            }
            "authorization" => {
                if head.authorization.is_some() {
                    return Err(MALFORMED);
                }
                head.authorization = Some(value.to_owned());
            }
            _ => {}
        }
    }
    if http10 && !keep_alive {
        head.close = true;
    }
    Ok(head)
}

fn serve_connection(slot: &mut Slot, socket: TcpStream, accepted: Instant) {
    let owner = Arc::clone(&slot.shared);
    let shared = &*owner;
    let _ = socket.set_nodelay(true);
    let limits = shared.limits;
    let timed = Timed {
        #[cfg(test)]
        bytes_read: 0,
        socket,
        deadline: accepted + limits.head,
        gap: limits.gap,
    };
    let stream = match &shared.tls {
        Some(config) => match rustls::ServerConnection::new(Arc::clone(config)) {
            Ok(tls) => Stream::Tls(Box::new(rustls::StreamOwned::new(tls, timed))),
            Err(_) => return,
        },
        None => Stream::Plain(timed),
    };
    let mut connection = Connection {
        stream,
        head_only: false,
        authority: ConnectionAuthority::default(),
        buffer: Vec::new(),
        limits,
    };
    let mut first = Some(accepted);
    loop {
        let head = match connection.read_head(first.take()) {
            Ok(head) => head,
            Err(NoHead::Refused(status, message)) => {
                return connection.refuse(status, message, &[]);
            }
            Err(NoHead::TimedOut { started: true }) => {
                return connection.refuse(408, "the request took too long", &[]);
            }
            Err(NoHead::TimedOut { started: false } | NoHead::Closed) => {
                return connection.close();
            }
        };
        if let Stream::Tls(stream) = &connection.stream {
            connection.authority.verified_client_identity =
                match stream.conn.peer_certificates().and_then(|c| c.first()) {
                    Some(cert) => match VerifiedClientIdentity::from_verified_der(cert) {
                        Ok(identity) => Some(identity),
                        Err(_) => return connection.close(),
                    },
                    None => None,
                };
        }
        connection.head_only = head.method == "HEAD";
        match answer(shared, slot, &mut connection, &head) {
            Next::Continue if !head.close => {}
            Next::Continue | Next::Close => return connection.close(),
            Next::Refuse(status, message, headers) => {
                return connection.refuse(status, message, headers);
            }
        }
    }
}

enum Next {
    /// Answered: read the next request unless this one closes.
    Continue,
    /// The connection can't go on (its body was cut short, or the answer
    /// couldn't be written).
    Close,
    /// Answer this and close, the body unread.
    Refuse(u16, &'static str, &'static [(&'static str, &'static str)]),
}

fn authority_refusal() -> Next {
    Next::Refuse(401, "request refused", &[("WWW-Authenticate", "Bearer")])
}
fn classify_route(path: &str) -> Option<(Kind, Option<Endpoint>)> {
    if let Some(endpoint) = Endpoint::from_path(path) {
        Some((Kind::MirrorPush, Some(endpoint)))
    } else if path.starts_with("/v1/analytics/") {
        Some((Kind::AnalyticsPublish, None))
    } else if path.starts_with("/v1/body/") {
        Some((Kind::BodyChannel, None))
    } else {
        None
    }
}

/// Separate bounded dispatcher, enabled only through local owner authorization.
/// No machine credential, cookie or body route can authorize this surface.
fn answer_provisioning(slot: &mut Slot, connection: &mut Connection, head: &Head) -> Next {
    use crate::provision_wire::{self, Request, Route};
    let refused = || Next::Refuse(401, "request refused", &[]);
    let renewal_route = crate::renewal_wire::Route::from_path(&head.target);
    let route = Route::from_path(&head.target);
    if route.is_none() && renewal_route.is_none() {
        return refused();
    }
    let Stream::Tls(stream) = &connection.stream else {
        return refused();
    };
    if stream.conn.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
        return refused();
    }
    let exporter = stream.conn.export_keying_material(
        [0u8; provision_wire::EXPORTER_LENGTH],
        provision_wire::EXPORTER_LABEL,
        Some(&[]),
    );
    if exporter.is_err()
        || head.method != "POST"
        || head.content_type.as_deref() != Some("application/json")
        || head.transfer_encoding
        || head.expect_other
        || !head
            .content_length
            .is_some_and(|n| n <= provision_wire::MAX_MESSAGE_BYTES as u64)
        || !slot.admit_provisioning()
    {
        return refused();
    }
    if head.expect_continue && connection.send(b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
        return Next::Close;
    }
    let saved = connection.limits.body;
    connection.limits.body = Duration::from_secs(provision_wire::READ_SECONDS);
    #[cfg(test)]
    let injected_timeout = slot
        .shared
        .provisioning_timeout
        .as_ref()
        .is_some_and(|flag| flag.swap(false, std::sync::atomic::Ordering::Relaxed));
    #[cfg(not(test))]
    let injected_timeout = false;
    let body = if injected_timeout {
        Err(io::ErrorKind::TimedOut.into())
    } else {
        connection.read_body(head.content_length.unwrap_or(0) as usize)
    };
    connection.limits.body = saved;
    let Ok(body) = body else {
        if renewal_route.is_none()
            && let Ok(exporter) = exporter
            && let Some(controller) = slot.shared.provisioning.as_ref()
            && let Ok(mut controller) = controller.lock()
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |now| now.as_secs());
            #[cfg(test)]
            let now = slot
                .shared
                .provisioning_clock
                .as_ref()
                .map_or(now, |clock| clock());
            controller.malformed(exporter, now);
        }
        return refused();
    };
    let body = zeroize::Zeroizing::new(body);
    let Ok(exporter) = exporter else {
        return refused();
    };
    if let Some(route) = renewal_route {
        let Ok(request) = crate::renewal_wire::Request::parse(route, &body) else {
            return refused();
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |now| now.as_secs());
        #[cfg(test)]
        let now = slot
            .shared
            .provisioning_clock
            .as_ref()
            .map_or(now, |clock| clock());
        let Ok(reply) = slot.shared.tokens.registry().renew(
            request,
            exporter,
            connection.authority.verified_client_identity.as_ref(),
            now,
        ) else {
            return refused();
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{}\r\n",
            reply.len(),
            if head.close {
                "Connection: close\r\n"
            } else {
                ""
            }
        );
        return if connection
            .send(response.as_bytes())
            .and_then(|()| connection.send(&reply))
            .is_ok()
        {
            Next::Continue
        } else {
            Next::Close
        };
    }
    let Some(route) = route else {
        return refused();
    };
    let Some(controller) = slot.shared.provisioning.as_ref() else {
        return refused();
    };
    let mut controller = match controller.lock() {
        Ok(controller) => controller,
        Err(_) => return refused(),
    };
    let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(now) => now.as_secs(),
        Err(_) => return refused(),
    };
    #[cfg(test)]
    let now = slot
        .shared
        .provisioning_clock
        .as_ref()
        .map_or(now, |clock| clock());
    let request = match Request::parse(route, &body) {
        Ok(request) => request,
        Err(_) => {
            controller.malformed(exporter, now);
            return refused();
        }
    };
    let clock = || {
        let current = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(now, |now| now.as_secs());
        #[cfg(test)]
        let current = slot
            .shared
            .provisioning_clock
            .as_ref()
            .map_or(current, |clock| clock());
        current
    };
    let reply = controller.handle_with_clock(request, exporter, &clock, |id, code| {
        #[cfg(test)]
        if let Some(terminal) = &slot.shared.provisioning_terminal {
            return terminal(id, code);
        }
        use std::io::IsTerminal;
        use std::os::unix::fs::OpenOptionsExt;
        let mut terminal = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open("/dev/tty")
            .map_err(|_| provision_wire::Refused)?;
        if !terminal.is_terminal() {
            return Err(provision_wire::Refused);
        }
        writeln!(terminal, "Pairing {id}: {}", code.expose()).map_err(|_| provision_wire::Refused)
    });
    let unavailable = controller.take_unavailable();
    drop(controller);
    if let Some(message) = unavailable {
        return Next::Refuse(409, message, &[]);
    }
    let Ok(reply) = reply else {
        return refused();
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{}\r\n",
        reply.len(),
        if head.close {
            "Connection: close\r\n"
        } else {
            ""
        }
    );
    if connection
        .send(head.as_bytes())
        .and_then(|()| connection.send(&reply))
        .is_err()
    {
        Next::Close
    } else {
        Next::Continue
    }
}

/// Checks a request in order (endpoint, method, framing, token, size) and,
/// only once all pass, reads its body and answers it.
fn answer(shared: &Shared, slot: &mut Slot, connection: &mut Connection, head: &Head) -> Next {
    if head.target.starts_with("/v1/provision/") || head.target.starts_with("/v1/renew/") {
        return answer_provisioning(slot, connection, head);
    }
    if head.transfer_encoding {
        return Next::Refuse(
            411,
            "a Content-Length is required; chunked bodies are not accepted",
            &[],
        );
    }
    if head.expect_other {
        return Next::Refuse(417, "only Expect: 100-continue is understood", &[]);
    }
    let Some((kind, endpoint)) = classify_route(&head.target) else {
        return authority_refusal();
    };
    if kind != Kind::MirrorPush && shared.tls.is_none() {
        return authority_refusal();
    }
    let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(now) => now.as_secs(),
        Err(_) => return authority_refusal(),
    };
    #[cfg(test)]
    let now = shared
        .provisioning_clock
        .as_ref()
        .map_or(now, |clock| clock());
    if kind == Kind::BodyChannel {
        return body::answer_body(shared, slot, connection, head, now);
    }
    let Some(authenticated) = shared.tokens.registry().authenticated_enrollment(
        &mut connection.authority,
        head.authorization.as_deref(),
        kind,
        now,
    ) else {
        return authority_refusal();
    };
    // Classified routes are closed until their owning slices install handlers.
    let Some(endpoint) = endpoint else {
        return authority_refusal();
    };
    if head.method != "POST" {
        return Next::Refuse(
            405,
            "the mirror protocol is POST only",
            &[("Allow", "POST")],
        );
    }
    let Some(length) = head.content_length else {
        return Next::Refuse(411, "a Content-Length is required", &[]);
    };
    slot.authenticated();
    if length > MAX_BODY_BYTES as u64 {
        return Next::Refuse(413, "the body is over the 6 MiB limit", &[]);
    }
    if head.expect_continue && connection.send(b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
        return Next::Close;
    }
    let body = match connection.read_body(length as usize) {
        Ok(body) => body,
        Err(error) if is_timeout(&error) => {
            return Next::Refuse(408, "the request took too long", &[]);
        }
        Err(_) => return Next::Close,
    };
    let machine = authenticated.machine;
    let reply = shared.receiver.handle(&machine, endpoint, &body);
    if reply.status >= 500 {
        eprintln!("semon receive: {machine}: {}", reply.body);
    }
    let headers = if authenticated.renewal_due {
        &[("Semon-Renewal", "due")][..]
    } else {
        &[]
    };
    match connection.respond(reply.status, &reply.body, head.close, headers) {
        Ok(()) => Next::Continue,
        Err(_) => Next::Close,
    }
}

#[cfg(test)]
#[path = "serve_auth_tests.rs"]
mod auth_tests;
#[cfg(test)]
#[path = "provision_http_tests.rs"]
mod provision_http_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_is_parsed_strictly() {
        let head = parse_head(
            b"POST /v1/mirror/append HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer abc\r\nContent-Length: 12\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        assert_eq!(head.method, "POST");
        assert_eq!(head.target, "/v1/mirror/append");
        assert_eq!(head.content_length, Some(12));
        assert_eq!(head.authorization.as_deref(), Some("Bearer abc"));
        assert!(head.close);
        assert!(
            parse_head(b"POST / HTTP/1.0\r\n\r\n").unwrap().close,
            "HTTP/1.0 closes by default"
        );
        let bad: [&[u8]; 12] = [
            b"POST /\r\n\r\n",
            b"POST  / HTTP/1.1\r\n\r\n",
            b"POST / HTTP/1.1\r\nContent-Length: +5\r\n\r\n",
            b"POST / HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n",
            b"POST / HTTP/1.1\r\nBad Name: 1\r\n\r\n",
            b"POST / HTTP/1.1\r\nA: 1\r\n folded\r\n\r\n",
            b"POST / HTTP/1.1\r\nA: 1\nB: 2\r\n\r\n",
            b"POST / HTTP/1.1\r\nA: \x01\r\n\r\n",
            b"POST / HTTP/1.1\r\nAuthorization: a\r\nAuthorization: b\r\n\r\n",
            b"POST / HTTP/1.1\r\n\xff: 1\r\n\r\n",
            b"POST / HTTP/1.1\r\nA: 1\rB: 2\r\n\r\n",
            b"POST /\r HTTP/1.1\r\n\r\n",
        ];
        for bad in bad {
            assert_eq!(
                parse_head(bad).map(|_| ()).unwrap_err().0,
                400,
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert_eq!(parse_head(b"POST / HTTP/2.0\r\n\r\n").unwrap_err().0, 505);
        let many: String = (0..=MAX_HEADERS).map(|n| format!("X-{n}: 1\r\n")).collect();
        let many = format!("POST / HTTP/1.1\r\n{many}\r\n");
        assert_eq!(parse_head(many.as_bytes()).unwrap_err().0, 431);
    }

    #[test]
    fn an_ipv6_peer_counts_by_its_64() {
        let key = |text: &str| ip_key(text.parse().unwrap());
        assert_eq!(key("2001:db8::1"), key("2001:db8::ffff:ffff:1"));
        assert_eq!(key("2001:db8::1"), key("2001:db8:0:0:1234::"));
        assert_ne!(key("2001:db8::1"), key("2001:db8:0:1::1"));
        assert_eq!(key("::ffff:192.0.2.7"), key("192.0.2.7"));
        assert_ne!(key("192.0.2.7"), key("192.0.2.8"));
    }

    #[test]
    fn zero_limits_are_raised_to_usable_ones() {
        let zero = Limits {
            connections: 0,
            connections_per_ip: 0,
            head: Duration::ZERO,
            idle: Duration::ZERO,
            gap: Duration::ZERO,
            body: Duration::ZERO,
            write: Duration::ZERO,
            linger: Duration::ZERO,
        }
        .clamped();
        assert_eq!(zero.connections, 1);
        assert_eq!(zero.connections_per_ip, 1);
        for duration in [zero.head, zero.idle, zero.gap, zero.body, zero.write] {
            assert!(!duration.is_zero());
        }
        assert_eq!(Limits::default().clamped(), Limits::default());
    }
}

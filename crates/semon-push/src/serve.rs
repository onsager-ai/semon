//! `semon receive`'s listener: the mirror protocol over HTTP/1.1, plain on
//! loopback or TLS with an operator-supplied certificate and key. It serves
//! `POST /v1/mirror/append` and `POST /v1/mirror/facts` to [`Receiver`] and
//! nothing else.
//!
//! Each connection has its own thread, up to [`Limits::connections`] in
//! all and [`Limits::connections_per_ip`] from one address; one more is
//! closed as soon as it is accepted. A request's token is checked, and its
//! declared length against [`MAX_BODY_BYTES`], before any of its body is
//! read. Every read and write of the socket, the TLS handshake's included,
//! runs against a deadline (see [`Limits`]), so a client that trickles bytes
//! can't hold a connection past it. Semon never opens a port in a firewall
//! or obtains a certificate.

use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    mirror::{DEFAULT_MAX_BYTES, Endpoint, MAX_BODY_BYTES, Receiver},
    tokens::{Tokens, check_tokens_mode, prepare_private_dir, tokens_path},
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
    /// Connections served at once from one IP address (4).
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

/// The listen rule, as `semon-relay receive` has it: any loopback address;
/// any other only with TLS and at least one token. Plain HTTP off loopback
/// is always refused.
pub fn check_listen(
    listen: SocketAddr,
    tls: bool,
    tokens: usize,
    tokens_file: &Path,
) -> Result<(), String> {
    if listen.ip().is_loopback() {
        return Ok(());
    }
    if !tls {
        return Err(
            "non-loopback receiver requires --tls-cert and --tls-key; plain HTTP is served on loopback only"
                .into(),
        );
    }
    if tokens == 0 {
        return Err(format!(
            "non-loopback receiver requires at least one token in {} (semon receive token add NAME --dir DIR)",
            tokens_file.display()
        ));
    }
    Ok(())
}

/// Who is connected: in all, and from each address.
#[derive(Default)]
struct Admission {
    open: usize,
    per_ip: HashMap<IpAddr, usize>,
}

struct Shared {
    receiver: Receiver,
    tokens: Tokens,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
    admission: Mutex<Admission>,
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
    ip: IpAddr,
}

impl Slot {
    fn admit(shared: &Arc<Shared>, ip: IpAddr) -> Option<Self> {
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
            ip,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut admission = self.shared.admission();
        admission.open = admission.open.saturating_sub(1);
        if let Some(count) = admission.per_ip.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                admission.per_ip.remove(&self.ip);
            }
        }
    }
}

/// A bound receiver, ready to [`Server::run`].
pub struct Server {
    listener: TcpListener,
    receiver: Receiver,
    tokens: Tokens,
    tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
}

impl Server {
    /// Checks the options and the directory (created 0700 when missing; an
    /// existing one others can read is refused), and binds.
    pub fn bind(options: &ServeOptions) -> Result<Self, String> {
        let dir = &options.dir;
        prepare_private_dir(dir, true)?;
        check_tokens_mode(dir)?;
        let tokens = Tokens::open(dir)?;
        check_listen(
            options.listen,
            options.tls.is_some(),
            tokens.len(),
            &tokens_path(dir),
        )?;
        let tls = options.tls.as_ref().map(tls_config).transpose()?;
        let listener = TcpListener::bind(options.listen)
            .map_err(|error| format!("cannot listen on {}: {error}", options.listen))?;
        Ok(Self {
            listener,
            receiver: Receiver::new(dir.clone()).with_max_bytes(options.max_bytes),
            tokens,
            tls,
            limits: Limits::default(),
        })
    }

    /// Other limits than the defaults (tests use short ones).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// `https` with TLS, `http` without.
    pub fn scheme(&self) -> &'static str {
        if self.tls.is_some() { "https" } else { "http" }
    }

    /// How many machines have a token now.
    pub fn tokens(&self) -> usize {
        self.tokens.len()
    }

    /// Serves until the process ends.
    pub fn run(self) -> Result<(), String> {
        let shared = Arc::new(Shared {
            receiver: self.receiver,
            tokens: self.tokens,
            tls: self.tls,
            limits: self.limits,
            admission: Mutex::new(Admission::default()),
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
                    serve_connection(&slot.shared, socket, accepted);
                    drop(slot);
                });
            if let Err(error) = spawned {
                eprintln!("semon receive: cannot start a connection thread: {error}");
            }
        }
        Ok(())
    }
}

fn tls_config(files: &TlsFiles) -> Result<Arc<rustls::ServerConfig>, String> {
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
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| error.to_string())?
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .map_err(|error| format!("the TLS certificate and key: {error}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// The socket, with a deadline that every read and write honors: before
/// each, its timeout is set to what is left of the deadline (at most
/// `gap`), and once the deadline has passed each fails with `TimedOut`.
/// rustls reads through it too, so a TLS record, or a handshake, trickled
/// in a byte at a time still ends at the deadline.
struct Timed {
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
        self.socket.read(buffer)
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
    fn read_more(&mut self, deadline: Instant) -> io::Result<usize> {
        self.stream.timed().deadline = deadline;
        let mut chunk = [0u8; 16 * 1024];
        let read = self.stream.read(&mut chunk)?;
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
            match self.read_more(deadline.unwrap_or(wait)) {
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
            if self.read_more(deadline)? == 0 {
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
        out.push_str(&body);
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

fn serve_connection(shared: &Shared, socket: TcpStream, accepted: Instant) {
    let _ = socket.set_nodelay(true);
    let limits = shared.limits;
    let timed = Timed {
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
        match answer(shared, &mut connection, &head) {
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

/// Checks a request in order (endpoint, method, framing, token, size) and,
/// only once all pass, reads its body and answers it.
fn answer(shared: &Shared, connection: &mut Connection, head: &Head) -> Next {
    let Some(endpoint) = Endpoint::from_path(&head.target) else {
        return Next::Refuse(404, "no such endpoint", &[]);
    };
    if head.method != "POST" {
        return Next::Refuse(
            405,
            "the mirror protocol is POST only",
            &[("Allow", "POST")],
        );
    }
    if head.transfer_encoding {
        return Next::Refuse(
            411,
            "a Content-Length is required; chunked bodies are not accepted",
            &[],
        );
    }
    let Some(length) = head.content_length else {
        return Next::Refuse(411, "a Content-Length is required", &[]);
    };
    if head.expect_other {
        return Next::Refuse(417, "only Expect: 100-continue is understood", &[]);
    }
    let Some(machine) = shared.tokens.authorize(head.authorization.as_deref()) else {
        return Next::Refuse(
            401,
            "the token is unknown or revoked",
            &[("WWW-Authenticate", "Bearer")],
        );
    };
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
    let reply = shared.receiver.handle(&machine, endpoint, &body);
    if reply.status >= 500 {
        eprintln!("semon receive: {machine}: {}", reply.body);
    }
    match connection.respond(reply.status, &reply.body, head.close, &[]) {
        Ok(()) => Next::Continue,
        Err(_) => Next::Close,
    }
}

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
    fn only_loopback_serves_plain_http() {
        let file = Path::new("/receiver/tokens");
        let at = |text: &str| text.parse::<SocketAddr>().unwrap();
        assert!(check_listen(at("127.0.0.1:8735"), false, 0, file).is_ok());
        assert!(check_listen(at("[::1]:8735"), false, 0, file).is_ok());
        let plain = check_listen(at("0.0.0.0:8735"), false, 3, file).unwrap_err();
        assert!(plain.contains("--tls-cert"), "{plain}");
        let untokened = check_listen(at("192.0.2.1:8735"), true, 0, file).unwrap_err();
        assert!(untokened.contains("at least one token"), "{untokened}");
        assert!(check_listen(at("0.0.0.0:8735"), true, 1, file).is_ok());
    }
}

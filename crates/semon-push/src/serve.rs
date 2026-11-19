//! `semon receive`'s listener: the push protocol over HTTP/1.1, plain on
//! a proxy-facing address behind an HTTPS proxy. It serves push routes to
//! [`Receiver`], pairing, and the read-only Viewer.
//!
//! Each connection has its own thread, up to [`Limits::connections`] in
//! all. Until one of its requests passes the token check, a connection also
//! counts against its address (an IPv6 address by its /64), which may hold
//! [`Limits::connections_per_ip`]; one more over either is closed as soon
//! as it is accepted. Once authenticated, it counts only in the total, so
//! several machines behind one address (an SSH tunnel, a NAT) each keep
//! their connection. A request's token is checked, and its
//! declared length against [`MAX_BODY_BYTES`], before any of its body is
//! read. Every read and write of the socket, runs against a deadline (see [`Limits`]), so a client that trickles bytes
//! can't hold a connection past it. Semon never opens a port in a firewall
//! or obtains a certificate.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

#[path = "serve_auth.rs"]
mod auth;
use auth::Authenticator;

use crate::receiver::{DEFAULT_MAX_BYTES, Endpoint, MAX_BODY_BYTES, Receiver};

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
    /// for a later request, from its first byte
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

#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// The receiver's directory: `tokens` and `machines/`.
    pub dir: PathBuf,
    pub listen: SocketAddr,
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
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// Who is connected: in all, and, not yet authenticated, from each address.
#[derive(Default)]
struct Admission {
    open: usize,
    viewers: usize,
    per_ip: HashMap<IpAddr, usize>,
}

struct Shared {
    receiver: Receiver,
    tokens: Authenticator,
    limits: Limits,
    admission: Mutex<Admission>,
    viewer: Option<Arc<semon_sessions::Viewer>>,
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
    viewer: bool,
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
            viewer: false,
        })
    }

    // Browser work must not consume the unauthenticated share of one tunnel
    // address. Four Viewer requests may run, leaving the remaining total for push.
    fn admit_viewer(&mut self) -> bool {
        let mut admission = self.shared.admission();
        if admission.viewers >= 4 {
            return false;
        }
        admission.viewers += 1;
        self.viewer = true;
        if let Some(ip) = self.ip.take() {
            release_ip(&mut admission, ip);
        }
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
        if self.viewer {
            admission.viewers -= 1;
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
    limits: Limits,
    viewer: Option<Arc<semon_sessions::Viewer>>,
}

impl Server {
    /// Checks the options and the directory (created 0700 when missing; an
    /// existing one others can read is refused), and binds.
    pub fn bind(options: &ServeOptions) -> Result<Self, String> {
        let dir = &options.dir;
        crate::simple_pair::initialize(dir)?;
        let tokens = Authenticator::open(dir)?;
        let credential_count = tokens.count()?;
        let listener = TcpListener::bind(options.listen)
            .map_err(|error| format!("cannot listen on {}: {error}", options.listen))?;
        Ok(Self {
            listener,
            credential_count,
            receiver: Receiver::new(dir.clone())
                .map_err(|error| error.to_string())?
                .with_max_bytes(options.max_bytes),
            tokens,
            limits: Limits::default(),
            viewer: None,
        })
    }

    /// Attach the existing Viewer handler to this listener.
    pub fn with_viewer(mut self, viewer: semon_sessions::Viewer) -> Result<Self, String> {
        self.viewer = Some(Arc::new(viewer));
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

    /// Plain HTTP; HTTPS terminates at the proxy.
    pub fn scheme(&self) -> &'static str {
        "http"
    }

    /// Number of configured credentials observed at startup.
    pub fn tokens(&self) -> usize {
        self.credential_count
    }

    /// Serves until the process ends.
    pub fn run(self) -> Result<(), String> {
        let shared = Arc::new(Shared {
            receiver: self.receiver,
            tokens: self.tokens,
            limits: self.limits,
            admission: Mutex::new(Admission::default()),
            viewer: self.viewer,
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

/// One request's head, as far as the receiver cares.
#[derive(Debug, Default, PartialEq, Eq)]
struct Head {
    headers: Vec<(String, String)>,
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
    stream: Timed,
    head_only: bool,
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
        self.stream.deadline = deadline;
        let mut chunk = [0u8; 16 * 1024];
        let bound = remaining.min(chunk.len());
        let read = self.stream.read(&mut chunk[..bound])?;
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(read)
    }

    /// The next request's head. The first request must be in within [`Limits::head`] of `accepted`; a later
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
            self.stream.deadline = deadline.unwrap_or(wait);
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
        self.read_body_until(length, Instant::now() + self.limits.body)
    }
    fn read_body_until(&mut self, length: usize, deadline: Instant) -> io::Result<Vec<u8>> {
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
        self.stream.deadline = Instant::now() + self.limits.write;
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

    /// Ends the connection: no more writes, then what the
    /// client still sends is read and dropped for [`Limits::linger`], so it
    /// reads the answer before the socket closes rather than a reset.
    fn close(mut self) {
        let linger = self.limits.linger;
        let socket = &mut self.stream.socket;
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
        head.headers.push((name.to_owned(), value.to_owned()));
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
        socket,
        deadline: accepted + limits.head,
        gap: limits.gap,
    };
    let mut connection = Connection {
        stream: timed,
        head_only: false,
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
        if head.target != "/v1/pair"
            && head.target != "/v1/unpair"
            && !head.target.starts_with("/v1/push/")
            && let Some(viewer) = &shared.viewer
        {
            if !slot.admit_viewer() {
                return connection.refuse(503, "request refused", &[]);
            }
            return serve_viewer(viewer, connection, head);
        }
        connection.head_only = head.method == "HEAD";
        match answer(shared, slot, &mut connection, &head) {
            Next::Continue if !head.close => {}
            Next::Continue | Next::Close => return connection.close(),
            Next::Refuse(status, message, headers) => {
                log_refusal(&head, status);
                return connection.refuse(status, message, headers);
            }
        }
    }
}

// The receiver parser reads exactly through the head, leaving the body on the
// socket. tiny_http retains the Viewer's body framing and response behavior.
fn serve_viewer(viewer: &semon_sessions::Viewer, mut connection: Connection, head: Head) {
    let timed = &mut connection.stream;
    let Ok(writer) = timed.socket.try_clone() else {
        return;
    };
    let _ = timed.socket.set_read_timeout(Some(connection.limits.gap));
    let _ = writer.set_write_timeout(Some(connection.limits.write));
    let peer = timed.socket.peer_addr().ok();
    let Ok(reader) = timed.socket.try_clone() else {
        return;
    };
    let Ok(method) = head.method.parse::<tiny_http::Method>() else {
        return;
    };
    let Ok(mut headers) = head
        .headers
        .into_iter()
        .map(|(name, value)| tiny_http::Header::from_bytes(name, value))
        .collect::<Result<Vec<_>, _>>()
    else {
        return;
    };
    headers.retain(|h| !h.field.equiv("Connection"));
    headers.push(tiny_http::Header::from_bytes("Connection", "close").unwrap());
    if let Ok(request) = tiny_http::new_request(
        false,
        method,
        head.target,
        tiny_http::HTTPVersion(1, 1),
        headers,
        peer,
        reader,
        writer,
    ) {
        viewer.handle_received(request);
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

// Use only fixed route labels: an unknown target can contain secrets or control bytes.
fn log_refusal(head: &Head, status: u16) {
    let endpoint = match head.target.as_str() {
        "/v1/pair" => "/v1/pair",
        "/v1/unpair" => "/v1/unpair",
        "/v1/push/append" => "/v1/push/append",
        "/v1/push/facts" => "/v1/push/facts",
        "/v1/push/status" => "/v1/push/status",
        _ => "unknown",
    };
    let reason = match status {
        401 if head.authorization.is_none() => "missing_token",
        401 => "unknown_token",
        404 => "unknown_endpoint",
        403 if head.target == "/v1/pair" || head.target == "/v1/unpair" => "pairing_refused",
        _ => "request_refused",
    };
    eprintln!("semon receive: endpoint={endpoint} status={status} reason={reason}");
}

fn authority_refusal() -> Next {
    Next::Refuse(401, "request refused", &[("WWW-Authenticate", "Bearer")])
}
fn answer_pair(shared: &Shared, slot: &mut Slot, connection: &mut Connection, head: &Head) -> Next {
    let refused = || Next::Refuse(403, "pairing refused", &[]);
    if (head.target == "/v1/pair" && head.authorization.is_some())
        || head.method != "POST"
        || head.transfer_encoding
        || head.expect_other
    {
        return refused();
    }
    let Some(length) = head.content_length.filter(|n| *n <= 8192) else {
        return refused();
    };
    if head.expect_continue && connection.send(b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
        return Next::Close;
    }
    let Ok(body) = connection.read_body(length as usize) else {
        return refused();
    };
    let Ok(now) = crate::simple_pair::now() else {
        return refused();
    };
    let reply = if head.target == "/v1/pair" {
        let Ok(reply) = shared.tokens.registry().pair(&body, now) else {
            return refused();
        };
        serde_json::to_value(&reply).expect("pair response serializes")
    } else {
        if shared
            .tokens
            .registry()
            .unpair(head.authorization.as_deref(), now)
            .is_err()
        {
            return authority_refusal();
        }
        json!({"version": 1})
    };
    slot.authenticated();
    match connection.respond(200, &reply, head.close, &[]) {
        Ok(()) => Next::Continue,
        Err(_) => Next::Close,
    }
}

/// Checks a request in order (endpoint, framing, token, method, size) and,
/// only once all pass, reads its body and answers it.
fn answer(shared: &Shared, slot: &mut Slot, connection: &mut Connection, head: &Head) -> Next {
    if head.target == "/v1/pair" || head.target == "/v1/unpair" {
        return answer_pair(shared, slot, connection, head);
    }
    let Some(endpoint) = Endpoint::from_path(&head.target) else {
        return Next::Refuse(404, "unknown endpoint", &[]);
    };
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
    let Some(machine) = shared
        .tokens
        .registry()
        .authenticate(head.authorization.as_deref())
    else {
        return authority_refusal();
    };
    if head.method != "POST" {
        return Next::Refuse(405, "the push protocol is POST only", &[("Allow", "POST")]);
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
    let reply = shared.receiver.handle(&machine, endpoint, &body);
    if let Some(viewer) = &shared.viewer {
        viewer.record_push_storage(
            &machine,
            reply.status,
            reply.body["code"].as_str(),
            matches!(endpoint, Endpoint::Append | Endpoint::Facts),
        );
    }
    if reply.status >= 500 {
        eprintln!("semon receive: {machine}: {}", reply.body);
    } else if reply.status >= 400 {
        log_refusal(head, reply.status);
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
            b"POST /v1/push/append HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer abc\r\nContent-Length: 12\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        assert_eq!(head.method, "POST");
        assert_eq!(head.target, "/v1/push/append");
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

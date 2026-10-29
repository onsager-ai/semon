use std::{
    fs,
    io::{self, Read},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        Weak,
        atomic::{AtomicU8, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

#[cfg(test)]
thread_local! {
    static INITIAL_CATALOG_WAIT_STARTED: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
}

use crate::{
    Node, Options, collect_with_index,
    events::EventCache,
    model,
    native_evidence::{NativeEvidence, Snapshot, Work},
    refresh::{Key, RefreshPool},
    union::{Refresh, ViewerCore},
};

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub sessions: Options,
    /// Several machines' homes to serve as one view instead of `sessions`,
    /// each with a stable key. Empty: `sessions` alone.
    pub machines: Vec<(String, Options)>,
    /// Received machines to follow as well
    /// ([`ViewerCore::with_received`]). With them, `machines` alone (which
    /// may be empty) are the fixed machines, and `sessions` isn't served
    /// unless it is among them.
    pub received: Option<crate::ReceivedMachines>,
    pub listen: String,
}
/// How often a machine in [`Refresh::Background`] is checked for changed
/// logs: a stat pass over the files and directories its models were built
/// from.
const CHECK_EVERY: Duration = Duration::from_millis(250);
/// How often a machine in [`Refresh::OnInvalidate`] is checked all the
/// same, in case a writer changed its logs without invalidating it.
pub(crate) const SAFETY_EVERY: Duration = Duration::from_secs(30);
/// The least time from the start of one rebuild of a machine's models to
/// the start of the next: changes within it are one rebuild.
pub(crate) const REBUILD_SPACING: Duration = Duration::from_secs(1);
/// A queued check this late means the refresh pool has fallen behind: a
/// read of that machine still answers from its last model, and nudges the
/// pool to start a worker if it has room for one.
pub(crate) const OVERDUE_AFTER: Duration = REBUILD_SPACING;
/// A machine is no longer checked after this long without a read, and a
/// refresh pool's thread stops after this long with nothing to run.
pub(crate) const IDLE_AFTER: Duration = Duration::from_secs(30);
/// While background rebuilds fail, reads answer from the last model built
/// until they have been seen failing this long; then each read answers the
/// error (500) at once, without waiting for a build, for as long as the
/// builds fail.
pub(crate) const FAILING_AFTER: Duration = Duration::from_secs(3);
/// A view whose queued check is this late (counted from when it was due or
/// queued, whichever is later), or whose check has run this long since it
/// took `work`, is stalled: its builds hang, or every worker its pool may
/// start is held by builds that do. Each read then answers that (500) at once, so a
/// frozen machine is never shown as a model that silently stopped moving.
pub(crate) const STALLED_AFTER: Duration = Duration::from_secs(60);

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// One machine's viewer: the pages, assets and API routes over one set of
/// agent homes, with the in-memory model, caches and live refresh behind
/// them. [`ViewerCore`] serves one or several of these.
///
/// Every method takes `&self`, and reads run at once from many threads:
/// - `shown` holds the models reads answer from, each behind an `Arc`. A
///   read takes its lock only to clone the `Arc`, then answers from that
///   snapshot alone; a rebuild takes it only to swap a new one in. The
///   `ETag` is the snapshot's version.
/// - `evidence` owns the source index, texts and native query cohort. Its
///   `work` lock serializes producers from their stat pass to
///   its swap, so one machine rebuilds once at a time. Outside
///   [`Refresh::OnRead`] an API read takes it only when there is nothing
///   built to answer from yet: the first model read, or the first V1 tree
///   read ([`MachineView::refresh_first`]), which may wait behind a model
///   rebuild. A page URL the built model lacks is checked against a fresh
///   model too ([`MachineView::page_exists`]). Once the model or tree is
///   built, a read of it answers at once and never waits on `work`: whether
///   the view went idle, its check is late, or its rebuilds fail, the read
///   at most queues a check ([`MachineView::note_read`]). Once rebuilds have
///   been seen failing for [`FAILING_AFTER`], or the view has stalled for
///   [`STALLED_AFTER`], it answers that error instead, at once.
/// - `files` holds the transcript paths and Codex harness offsets the V1
///   routes look up, held for a lookup or an insert, never for a file read.
/// - `live` is the view's refresh state, held for a few field reads, and
///   around it the refresh pool's queue lock for an insert or a removal.
///
/// Viewer locks are taken in the order evidence.work, files, shown, live,
/// and the refresh pool's last of all. Native query publication has its own
/// short snapshot lock and never takes the Viewer locks or starts its workers.
pub(crate) struct MachineView {
    pub(crate) evidence: Arc<NativeEvidence>,
    /// Its [`Refresh`] mode ([`Refresh::code`]).
    mode: AtomicU8,
    shown: RwLock<Shown>,
    live: Live,
    /// This view, for its pool's queue: an entry never keeps a view alive.
    me: Weak<MachineView>,
    #[cfg(test)]
    hooks: tests::Hooks,
}

/// The snapshots reads answer from.
#[derive(Default)]
struct Shown {
    tree: Option<Arc<TreeCache>>,
}
/// A machine's background refresh: its place in the pool, and what its
/// checks found.
struct Live {
    state: Mutex<LiveState>,
    /// Signalled when a check of it ends.
    changed: Condvar,
}

/// Where a view's background check is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// Not queued: the view is idle, closed, or refreshed on read.
    Idle,
    /// Queued in its pool once, under this key, due at its instant.
    Queued(Key),
    /// A worker is checking it; the worker queues the next check.
    Running,
}

struct LiveState {
    /// The pool whose workers check this view.
    pool: Arc<RefreshPool>,
    slot: Slot,
    /// Never queued again, and a queued check is dropped.
    closed: bool,
    /// Its logs changed since the last check began, as the embedding
    /// server said ([`MachineView::invalidate`]).
    invalidated: bool,
    /// The last read that answered from a snapshot.
    read_at: Option<Instant>,
    /// When the last rebuild of either started, as of the last check.
    built_at: Option<Instant>,
    /// Each kind's last refresh error, printed once; `None` after a
    /// refresh of it that worked. A failing tree never fails the model.
    error: [Option<String>; 2],
    /// When each kind's refreshes were first seen failing, while they fail.
    failing_since: [Option<Instant>; 2],
    /// Each kind's recorded error predates an idle spell: no build has been
    /// tried since, so it isn't answered until a check reports again.
    unconfirmed: [bool; 2],
    /// When the running check took `work` and started, while one runs.
    running_since: Option<Instant>,
    /// [`STALLED_AFTER`], but for tests.
    stalled_after: Duration,
    /// [`IDLE_AFTER`], but for tests.
    idle_after: Duration,
    /// [`SAFETY_EVERY`], but for tests.
    safety_every: Duration,
}

/// What a machine builds from its logs, each refreshed and failing on its
/// own: the model, and the V1 tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Catalog = 0,
    Tree = 1,
}

impl Kind {
    fn other(self) -> Self {
        match self {
            Kind::Catalog => Kind::Tree,
            Kind::Tree => Kind::Catalog,
        }
    }
}

impl LiveState {
    /// Read in the last [`IDLE_AFTER`]: a view kept checked.
    fn active(&self) -> bool {
        self.read_at
            .is_some_and(|read| read.elapsed() < self.idle_after)
    }

    /// Notes how a refresh of `kind` went. Reads go on answering from the
    /// last one built while it fails, until [`FAILING_AFTER`] after the
    /// first failure seen (not after that build started: a long build that
    /// fails once gets the whole grace); each check tries again, and its
    /// error is printed once, not on every check.
    fn record(&mut self, kind: Kind, result: &io::Result<()>) {
        let at = kind as usize;
        self.unconfirmed[at] = false;
        match result {
            Ok(()) => {
                self.error[at] = None;
                self.failing_since[at] = None;
            }
            Err(error) => {
                let message = error.to_string();
                if self.error[at].as_deref() != Some(message.as_str()) {
                    eprintln!("semon sessions viewer: {message}");
                }
                self.error[at] = Some(message);
                self.failing_since[at] = self.failing_since[at].or(Some(Instant::now()));
            }
        }
    }
}

// An embedding server shares one core between threads and calls it from
// many at once: it must stay `Send` and `Sync`.
const _: fn() = || {
    fn shared<T: Send + Sync>() {}
    shared::<ViewerCore>();
    shared::<MachineView>();
};

/// The loopback server around a [`ViewerCore`]: the per-run token and its
/// cookie, the exact local Host check and GET only.
struct Viewer {
    control: Option<Mutex<Arc<semon_control::codex::Codex>>>,
    bootstrap_used: Mutex<bool>,
    core: ViewerCore,
    token: String,
    bootstrap: String,
    port: u16,
}

/// Headers the viewer sends on every response, whatever its status. A server
/// embedding [`ViewerCore`] sends them too: the page relies on this CSP (no
/// inline script or style), nothing it serves may be cached or framed, and
/// every body is exactly the type it is sent as. An attachment's image is
/// the one exception, as [`ViewerReply::headers`] says.
pub const SECURITY_HEADERS: [(&str, &str); 5] = [
    ("Cache-Control", "no-store"),
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self'",
    ),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
    ("X-Content-Type-Options", "nosniff"),
];

/// Headers for an attachment's image from `/api/attachment`: its URL names
/// its content (`v`), so the browser may keep it, for this viewer alone, but
/// only for a day: a screenshot is content, and redacting the logs doesn't
/// clear a copy already fetched. Opened on its own, the image is a document
/// that may load nothing, run nothing and be framed by nothing.
pub(crate) const ATTACHMENT_HEADERS: [(&str, &str); 5] = [
    ("Cache-Control", "private, max-age=86400"),
    (
        "Content-Security-Policy",
        "default-src 'none'; style-src 'unsafe-inline'; sandbox",
    ),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
    ("X-Content-Type-Options", "nosniff"),
];

/// One answer from [`ViewerCore::respond`]: the status, the body's content
/// type, the body and any endpoint-specific `ETag`. Native session reads
/// carry their qualified source generations in the response body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerReply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub etag: Option<String>,
}

impl ViewerReply {
    /// The headers to send with this reply besides `Content-Type` and
    /// `ETag`: [`SECURITY_HEADERS`], except for an attachment's image (a 200
    /// whose type is one of the four native selected image types,
    /// which nothing else is sent as), which may be cached and gets a CSP of
    /// its own. A server that sends [`SECURITY_HEADERS`] on every reply
    /// instead still serves images correctly, only never from the cache.
    pub fn headers(&self) -> &'static [(&'static str, &'static str)] {
        if self.status == 200 && crate::attachments::is_served_type(self.content_type) {
            &ATTACHMENT_HEADERS
        } else {
            &SECURITY_HEADERS
        }
    }
}

struct TreeCache {
    roots: Vec<Node>,
    json: String,
    snapshot: Snapshot,
}
/// The viewer's page: every screen's URL serves it.
const PAGE: &str = crate::shell::PAGE_HTML;

/// The mockup's two families, vendored (D1): Instrument Sans and JetBrains
/// Mono, each in its latin and latin-ext subsets.
pub(crate) const FONTS: [(&str, &[u8]); 4] = [
    (
        "instrument-sans-latin",
        include_bytes!("fonts/instrument-sans/latin.woff2"),
    ),
    (
        "instrument-sans-latin-ext",
        include_bytes!("fonts/instrument-sans/latin-ext.woff2"),
    ),
    (
        "jetbrains-mono-latin",
        include_bytes!("fonts/jetbrains-mono/latin.woff2"),
    ),
    (
        "jetbrains-mono-latin-ext",
        include_bytes!("fonts/jetbrains-mono/latin-ext.woff2"),
    ),
];
fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn listen_addr(value: &str) -> io::Result<SocketAddr> {
    let address = if value.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("127.0.0.1:{value}")
    } else if let Some(port) = value.strip_prefix("localhost:") {
        format!("127.0.0.1:{port}")
    } else {
        value.to_owned()
    };
    let address: SocketAddr = address
        .parse()
        .map_err(|_| invalid_input("invalid --listen address"))?;
    if address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(invalid_input("--listen must bind 127.0.0.1"));
    }
    Ok(address)
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Serves the session tree and transcripts on IPv4 loopback until interrupted.
pub fn serve(options: ServeOptions) -> io::Result<()> {
    let address = listen_addr(&options.listen)?;
    let server = Server::http(address).map_err(io::Error::other)?;
    serve_server(options, server)
}

/// Serves on an already bound loopback listener. Binding to port zero lets
/// callers obtain an assigned port without racing another listener.
pub fn serve_listener(options: ServeOptions, listener: std::net::TcpListener) -> io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(invalid_input("the viewer listener must be loopback"));
    }
    let server = Server::from_listener(listener, None).map_err(io::Error::other)?;
    serve_server(options, server)
}

/// Retained control entry point; unavailable in the collection configuration.
/// Refuses before binding a listener or starting native observation.
pub fn serve_with_control(
    options: ServeOptions,
    control: Arc<semon_control::codex::Codex>,
) -> io::Result<()> {
    semon_control::require_collection_control()?;
    let address = listen_addr(&options.listen)?;
    let server = Server::http(address).map_err(io::Error::other)?;
    serve_server_control(options, server, Some(control))
}
fn serve_server(options: ServeOptions, server: Server) -> io::Result<()> {
    serve_server_control(options, server, None)
}
fn serve_server_control(
    options: ServeOptions,
    server: Server,
    control: Option<Arc<semon_control::codex::Codex>>,
) -> io::Result<()> {
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| invalid_input("expected TCP listener"))?
        .port();
    let mut core = match options.received {
        Some(received) => ViewerCore::with_received(options.machines, received),
        None if options.machines.is_empty() => ViewerCore::new(options.sessions),
        None => ViewerCore::with_machines(options.machines),
    };
    // Reads answer from the last built model; a rebuild never holds one up.
    core.set_refresh(Refresh::Background);
    let token = random_token()?;
    let bootstrap = if control.is_some() {
        random_token()?
    } else {
        token.clone()
    };
    let viewer = Arc::new(Viewer {
        core,
        token,
        bootstrap,
        bootstrap_used: Mutex::new(false),
        control: control.map(Mutex::new),
        port,
    });
    let server = Arc::new(server);
    println!("http://127.0.0.1:{port}/?t={}", viewer.bootstrap);
    // A few requests at once, so a long transcript page doesn't hold up a
    // poll. recv blocks; between requests only the refresh pool runs, and
    // it checks a machine only until IDLE_AFTER its last read.
    let (done, stopped) = std::sync::mpsc::channel();
    for _ in 0..SERVE_THREADS {
        let (server, viewer, done) = (server.clone(), viewer.clone(), done.clone());
        thread::spawn(move || {
            let error = loop {
                match server.recv() {
                    // A request that panics fails alone (dropping it
                    // answers an empty 500) and this thread serves on,
                    // rather than the server losing a thread for good.
                    Ok(request) => {
                        let viewer = &viewer;
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                            viewer.handle(request)
                        }))
                        .is_err()
                        {
                            eprintln!("semon sessions viewer: a request panicked");
                        }
                    }
                    Err(error) => break error,
                }
            };
            let _ = done.send(error);
        });
    }
    drop(done);
    // The listener failing ends the server, as it did with one thread.
    Err(stopped
        .recv()
        .unwrap_or_else(|_| io::Error::other("every viewer thread stopped")))
}

/// How many requests the local server answers at once.
const SERVE_THREADS: usize = 4;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("static HTTP header")
}

fn respond(request: Request, reply: ViewerReply, cookie: Option<&str>) {
    let headers = reply.headers();
    let mut response = Response::from_data(reply.body).with_status_code(StatusCode(reply.status));
    if let Some(etag) = reply.etag.as_deref() {
        response.add_header(header("ETag", etag));
    }
    response.add_header(header("Content-Type", reply.content_type));
    for (name, value) in headers {
        response.add_header(header(name, value));
    }
    if let Some(token) = cookie {
        response.add_header(header(
            "Set-Cookie",
            &format!("semon_session={token}; HttpOnly; SameSite=Strict; Path=/"),
        ));
    }
    let _ = request.respond(response);
}

fn request_header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str())
}

pub(crate) fn decoded(value: &str) -> Option<String> {
    let mut result = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
                result.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            b'%' => return None,
            b'+' => {
                result.push(b' ');
                index += 1;
            }
            byte => {
                result.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(result).ok()
}

pub(crate) fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then_some(value)
    })
}

fn authorized(request: &Request, query: &str, token: &str, port: u16) -> Option<bool> {
    if request
        .headers()
        .iter()
        .filter(|header| header.field.equiv("Host"))
        .count()
        != 1
    {
        return None;
    }
    let host = request_header(request, "Host")?;
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return None;
    }
    if request.method() != &Method::Get {
        return None;
    }
    let cookie = request_header(request, "Cookie").is_some_and(|cookies| {
        cookies
            .split(';')
            .any(|part| part.trim().strip_prefix("semon_session=") == Some(token))
    });
    let query_token = query_value(query, "t").is_some_and(|value| value == token);
    (cookie || query_token).then_some(query_token)
}

fn control_reply(status: u16, value: Value) -> ViewerReply {
    ViewerReply {
        status,
        content_type: "application/json; charset=utf-8",
        body: value.to_string().into_bytes(),
        etag: None,
    }
}
fn control_authorized(request: &Request, token: &str, port: u16) -> bool {
    let headers = request.headers();
    if ["Host", "Origin", "Cookie", "Content-Type"]
        .iter()
        .any(|name| headers.iter().filter(|h| h.field.equiv(name)).count() != 1)
    {
        return false;
    }
    let host = request_header(request, "Host").unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return false;
    }
    if request_header(request, "Origin") != Some(format!("http://{host}").as_str()) {
        return false;
    }
    let tokens: Vec<_> = request_header(request, "Cookie")
        .unwrap_or("")
        .split(';')
        .filter_map(|s| s.trim().strip_prefix("semon_session="))
        .collect();
    tokens == [token]
}

impl Viewer {
    fn handle(&self, mut request: Request) {
        let url = request.url().to_owned();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        if request.method() == &Method::Post && path == "/api/control" {
            let authorized = control_authorized(&request, &self.token, self.port);
            let reply = if authorized {
                self.control_write(&mut request)
            } else {
                control_reply(403, serde_json::json!({"error":"Forbidden"}))
            };
            respond(request, reply, None);
            return;
        }
        // Credentials are transport-only. Reject duplicate/invalid URL credentials
        // even with a cookie, before spending a one-time control bootstrap nonce.
        let credentials: Vec<_> = query
            .split('&')
            .filter(|part| {
                let name = part.split_once('=').map_or(*part, |(name, _)| name);
                decoded(name).as_deref() == Some("t")
            })
            .collect();
        if !credentials.is_empty()
            && (credentials.len() != 1
                || credentials[0].strip_prefix("t=") != Some(self.bootstrap.as_str()))
        {
            respond(
                request,
                control_reply(403, serde_json::json!({"error":"Forbidden"})),
                None,
            );
            return;
        }
        let accepted = if request.method() == &Method::Post && path == "/api/viewer/home" {
            control_authorized(&request, &self.token, self.port).then_some(false)
        } else if self.control.is_some() {
            // A spent URL nonce is not a cookie secret and never authorizes writes.
            authorized(&request, "", &self.token, self.port).or_else(|| {
                authorized(&request, query, &self.bootstrap, self.port).filter(|set| *set)
            })
        } else {
            authorized(&request, query, &self.token, self.port)
        };
        let Some(set_cookie) = accepted else {
            let forbidden = ViewerReply {
                status: 403,
                content_type: "text/plain; charset=utf-8",
                body: b"Forbidden".to_vec(),
                etag: None,
            };
            respond(request, forbidden, None);
            return;
        };
        if set_cookie && self.control.is_some() {
            let mut used = self
                .bootstrap_used
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if *used {
                respond(
                    request,
                    control_reply(
                        403,
                        serde_json::json!({"error":"Control bootstrap URL already used"}),
                    ),
                    None,
                );
                return;
            }
            *used = true;
        }
        let routing_query = query
            .split('&')
            .filter(|part| !part.starts_with("t="))
            .collect::<Vec<_>>()
            .join("&");
        let query = routing_query.as_str();
        let if_none_match = request_header(&request, "If-None-Match").map(str::to_owned);
        let mut reply = if path == "/api/viewer/home" && request.method() == &Method::Post {
            let mut bytes = Vec::new();
            let body = request
                .as_reader()
                .take(512 * 1024 + 1)
                .read_to_end(&mut bytes);
            let value = serde_json::from_slice::<Value>(&bytes);
            if body.is_err()
                || bytes.len() > 512 * 1024
                || value
                    .as_ref()
                    .ok()
                    .and_then(|value| value["seen"].as_array())
                    .is_none()
            {
                control_reply(
                    400,
                    serde_json::json!({"error":"Invalid Home read arguments"}),
                )
            } else {
                let value = value.unwrap();
                let joined = format!(
                    "{}{}seen={}",
                    query,
                    if query.is_empty() { "" } else { "&" },
                    percent_encode(&value["seen"].to_string())
                );
                self.core.respond("GET", path, &joined, None)
            }
        } else if path == "/api/session-control" && semon_control::COLLECTION_CONTROL_PAUSED {
            control_reply(
                200,
                serde_json::json!({"control":null,"control_available":false,
                "reason":"Two-way control is paused in the collection configuration"}),
            )
        } else if path == "/api/session-control" {
            let identity = self
                .core
                .respond("GET", "/api/session-identity", query, None);
            let selected = serde_json::from_slice::<Value>(&identity.body).unwrap_or(Value::Null);
            let status = self
                .control
                .as_ref()
                .map(|control| {
                    control
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .snapshot()
                })
                .filter(|snapshot| {
                    selected["identity"]["source_key"] == "local"
                        && selected["identity"]["harness"] == "codex"
                        && selected["identity"]["owner_qualification"] != "provisional"
                        && selected["identity"]["native_id"]
                            .as_str()
                            .is_some_and(|id| snapshot["thread"].as_str() == Some(id))
                });
            control_reply(200, serde_json::json!({"control":status}))
        } else {
            self.core
                .respond("GET", path, query, if_none_match.as_deref())
        };
        if path == "/api/viewer/home"
            && reply.status == 200
            && self.control.is_some()
            && !semon_control::COLLECTION_CONTROL_PAUSED
        {
            let mut body: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
            if body.is_object() {
                body["launch_control"] = self
                    .control
                    .as_ref()
                    .and_then(|control| {
                        control
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .launch_snapshot()
                    })
                    .unwrap_or(Value::Null);
                reply = control_reply(200, body);
            }
        }
        respond(request, reply, set_cookie.then_some(&self.token));
    }
    fn control_write(&self, request: &mut Request) -> ViewerReply {
        if semon_control::COLLECTION_CONTROL_PAUSED {
            return control_reply(
                503,
                serde_json::json!({"error":{"code":"capability_unavailable",
                "message":"Two-way control is paused in the collection configuration"}}),
            );
        }
        if request_header(request, "Content-Type") != Some("application/json")
            || request.body_length().is_none_or(|n| n > 65536)
        {
            return control_reply(400, serde_json::json!({"error":"Invalid control body"}));
        }
        let mut bytes = Vec::new();
        if request
            .as_reader()
            .take(65537)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > 65536
        {
            return control_reply(400, serde_json::json!({"error":"Invalid control body"}));
        }
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return control_reply(400, serde_json::json!({"error":"Invalid control JSON"}));
        };
        let owner = self.control.as_ref().unwrap();
        let mut connection = owner.lock().unwrap_or_else(PoisonError::into_inner);
        let driver = connection.clone();
        if value["op"] == "reconnect" {
            if value["generation"] != driver.snapshot()["generation"] {
                return control_reply(409, serde_json::json!({"error":"Stale connection"}));
            }
            return match driver.reconnect() {
                Ok(next) => {
                    *connection = next;
                    control_reply(200, serde_json::json!({"reconnected":true}))
                }
                Err(_) => control_reply(
                    409,
                    serde_json::json!({"error":"Native reconnect failed; no write replayed"}),
                ),
            };
        }
        drop(connection);
        // HTTP has no Unix peer credentials. Zero means unknown, not root authority.
        // Authority is the isolated executor plus exact authenticated browser transport.
        let source = semon_control::Source {
            window: "local-viewer".into(),
            peer_pid: 0,
            peer_uid: 0,
        };
        match driver.command(&value, source) {
            Ok(mut receipt) => {
                let id = value["id"].as_str().unwrap_or("");
                // Confirm only this operation's native acceptance; the receipt
                // grants no control identity or unrelated operation history.
                let actions = driver
                    .operation_receipt(id)
                    .map(|action| serde_json::Map::from_iter([(id.to_owned(), action)]))
                    .unwrap_or_default();
                receipt["snapshot"] = serde_json::json!({"actions":actions});
                control_reply(200, receipt)
            }
            Err(error) => control_reply(409, serde_json::json!({"error":error.to_string()})),
        }
    }
}
/// When a background check is next due, after one that started at
/// `started` and a last rebuild that started at `built_at`: [`CHECK_EVERY`]
/// after it, and never before [`REBUILD_SPACING`] after that rebuild.
fn next_check(started: Instant, built_at: Option<Instant>) -> Instant {
    let next = started + CHECK_EVERY;
    built_at.map_or(next, |built| next.max(built + REBUILD_SPACING))
}

impl MachineView {
    /// A view over the agent homes, `/proc` and cache that `options` name,
    /// refreshed before each read until [`MachineView::set_refresh`], and
    /// checked in the background by [`RefreshPool::shared`] until
    /// [`MachineView::set_pool`]. Nothing is read until the first request.
    pub(crate) fn new(options: Options) -> Arc<Self> {
        Self::from_evidence(NativeEvidence::new(options))
    }

    /// Attach Viewer production to an existing native owner without a second
    /// Event Index or query cohort. The host owns its admission and lifetime.
    pub(crate) fn from_evidence(evidence: Arc<NativeEvidence>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            evidence,
            mode: AtomicU8::new(Refresh::OnRead.code()),
            shown: RwLock::default(),
            live: Live {
                state: Mutex::new(LiveState {
                    pool: RefreshPool::shared(),
                    slot: Slot::Idle,
                    closed: false,
                    invalidated: false,
                    read_at: None,
                    built_at: None,
                    error: [None, None],
                    failing_since: [None; 2],
                    unconfirmed: [false; 2],
                    running_since: None,
                    stalled_after: STALLED_AFTER,
                    idle_after: IDLE_AFTER,
                    safety_every: SAFETY_EVERY,
                }),
                changed: Condvar::new(),
            },
            me: me.clone(),
            #[cfg(test)]
            hooks: tests::Hooks::default(),
        })
    }

    /// A host invalidation or running producer prevents an old complete
    /// publication from proving current ownership or absence.
    pub(crate) fn catalog_observation_pending(&self) -> bool {
        let state = lock(&self.live.state);
        state.invalidated || matches!(state.slot, Slot::Running)
    }

    /// The homes, cache and facts this view reads.
    pub(crate) fn options(&self) -> &Options {
        &self.evidence.options
    }

    /// When reads are brought up to date with the logs: see [`Refresh`].
    pub(crate) fn set_refresh(&self, refresh: Refresh) {
        self.mode.store(refresh.code(), Ordering::Relaxed);
    }

    fn refresh(&self) -> Refresh {
        Refresh::from_code(self.mode.load(Ordering::Relaxed))
    }

    /// The pool whose workers check this view in the background. A check
    /// queued in the one before moves to it.
    pub(crate) fn set_pool(&self, pool: Arc<RefreshPool>) {
        let mut state = lock(&self.live.state);
        self.set_pool_state(&mut state, pool);
    }

    fn set_pool_state(&self, state: &mut LiveState, pool: Arc<RefreshPool>) {
        if let Slot::Queued(key) = state.slot {
            state.pool.remove(key);
            state.slot = Slot::Idle;
            state.pool = pool;
            self.queue(state, key.0, true);
        } else {
            state.pool = pool;
        }
    }

    /// Whether a snapshot's logs changed since it was built: a stat pass.
    fn changed(&self, snapshot: &Snapshot) -> bool {
        #[cfg(test)]
        self.hooks.stats.fetch_add(1, Ordering::SeqCst);
        snapshot.changed(&self.evidence.options)
    }
    fn shown_tree(&self) -> Option<Arc<TreeCache>> {
        read_lock(&self.shown).tree.clone()
    }

    /// Rebuilds the native tree if anything it was built from changed. The
    /// caller holds `work`.
    fn refresh_tree_locked(&self, work: &mut Work) -> io::Result<Arc<TreeCache>> {
        if let Some(tree) = self.shown_tree()
            && !{
                #[cfg(test)]
                self.hooks.stats.fetch_add(1, Ordering::SeqCst);
                tree.snapshot.metadata_changed(&self.evidence.options)
            }
        {
            return Ok(tree);
        }
        work.built_at = Some(Instant::now());
        #[cfg(test)]
        if self.hooks.tree_failing.load(Ordering::SeqCst) {
            return Err(io::Error::other("a tree build failed on purpose"));
        }
        if work.events.is_none() {
            work.events = Some(
                EventCache::open_scoped(&self.evidence.options.cache).map_err(io::Error::other)?,
            );
        }
        let index = work.events.as_mut().expect("native index loaded");
        let roots = collect_with_index(&self.evidence.options, index)?;
        let json = crate::render_json(&roots);
        let snapshot = Snapshot::capture(&self.evidence.options, &roots, index);
        let tree = Arc::new(TreeCache {
            roots,
            json,
            snapshot,
        });
        write_lock(&self.shown).tree = Some(tree.clone());
        Ok(tree)
    }

    /// Schedule native Catalog work through its evidence owner. Viewer retains
    /// only refresh policy and test instrumentation, never Catalog derivation.
    fn refresh_catalog_locked(&self, work: &mut Work, now: i64) -> io::Result<bool> {
        #[cfg(test)]
        {
            if work.catalog_snapshot.is_some() {
                self.hooks.stats.fetch_add(1, Ordering::SeqCst);
            }
            self.evidence
                .refresh_catalog_with(work, now, || self.hooks.building())
        }
        #[cfg(not(test))]
        self.evidence.refresh_catalog(work, now)
    }
    /// The V1 tree, rebuilt first if anything changed.
    fn fresh_tree(&self) -> io::Result<Arc<TreeCache>> {
        if let Some(tree) = self.shown_tree()
            && !self.changed(&tree.snapshot)
        {
            return Ok(tree);
        }
        self.refresh_tree_locked(&mut lock(&self.evidence.work))
    }
    /// The native tree snapshot a request answers from.
    fn served_tree(&self) -> io::Result<Arc<TreeCache>> {
        if self.refresh() == Refresh::OnRead {
            return self.fresh_tree();
        }
        if let Some(tree) = self.shown_tree() {
            self.note_read(Kind::Tree)?;
            return Ok(tree);
        }
        self.refresh_first(Kind::Tree)?;
        self.shown_tree()
            .ok_or_else(|| io::Error::other("no tree was built"))
    }

    /// Whether this refresh kind owns a shown snapshot. Catalog publication
    /// belongs to native evidence, never this view.
    fn is_shown(&self, kind: Kind) -> bool {
        let shown = read_lock(&self.shown);
        match kind {
            Kind::Catalog => false,
            Kind::Tree => shown.tree.is_some(),
        }
    }

    /// Refreshes the native Catalog or tree if its logs changed. The caller holds
    /// `work`.
    fn refresh_kind(&self, work: &mut Work, kind: Kind) -> io::Result<()> {
        match kind {
            Kind::Catalog => self.refresh_catalog_locked(work, model::now_ms()).map(drop),
            Kind::Tree => self.refresh_tree_locked(work).map(drop),
        }
    }

    /// Notes a read of the shown model or tree (`kind`), makes sure a check
    /// of the view is coming, and says whether the shown one answers it
    /// (`Ok`) or the error of its failing rebuilds does. It takes only the
    /// `live` lock (and under it the pool's, as [`MachineView::queue`]
    /// does), never `work`, so it never waits for a check or a rebuild:
    /// - a closed view is checked no more;
    /// - a view a worker is checking is queued again by that worker;
    /// - a queued check stays queued. One [`OVERDUE_AFTER`] late, the pool
    ///   has fallen behind: it is nudged to start a worker if it has room
    ///   for one, or past its size if every worker's check has run
    ///   [`OVERFLOW_AFTER`](crate::refresh::OVERFLOW_AFTER)
    ///   ([`RefreshPool::nudge`]); otherwise the check waits its turn and
    ///   the read answers from the last model;
    /// - a view not queued (idle, or invalidated while idle) is queued for
    ///   a check at once, which rebuilds it if its logs changed.
    ///
    /// Rebuilds that fail leave the last one served, and each check tries
    /// again ([`LiveState::record`]). They aren't hidden behind an ever
    /// older snapshot, though: [`FAILING_AFTER`] after the first failure
    /// seen, a read answers their error (500) at once, until a check's
    /// build works. An error recorded before an idle spell isn't answered
    /// until the check this read queues reports. Nor is a view that stalls
    /// hidden: once its check is [`STALLED_AFTER`] late, or has run that
    /// long, a read answers "rebuilds stalled" (500) at once. A closed view
    /// answers from what it has.
    fn note_read(&self, kind: Kind) -> io::Result<()> {
        self.note_read_with(kind, true)
    }

    fn note_read_with(&self, kind: Kind, prepare: bool) -> io::Result<()> {
        let mut state = lock(&self.live.state);
        self.note_read_state(&mut state, kind, prepare, Instant::now())
    }

    fn note_read_state(
        &self,
        state: &mut LiveState,
        kind: Kind,
        prepare: bool,
        now: Instant,
    ) -> io::Result<()> {
        state.read_at = Some(now);
        if state.closed {
            return Ok(());
        }
        let (slot, stalled_after) = (state.slot, state.stalled_after);
        let stalled = match slot {
            Slot::Running => state
                .running_since
                .is_some_and(|since| now.saturating_duration_since(since) >= stalled_after),
            Slot::Queued(key) => {
                // Due no earlier than it was queued ([`MachineView::queue_by`]).
                let late = now.saturating_duration_since(key.0);
                if late >= OVERDUE_AFTER {
                    state.pool.nudge();
                }
                late >= stalled_after
            }
            Slot::Idle => {
                if prepare || state.invalidated {
                    state.unconfirmed = [true; 2];
                    self.queue(state, now, false);
                }
                false
            }
        };
        if stalled {
            return Err(io::Error::other("rebuilds stalled"));
        }
        let at = kind as usize;
        match (&state.error[at], state.failing_since[at]) {
            (Some(message), Some(since))
                if !state.unconfirmed[at]
                    && now.saturating_duration_since(since) >= FAILING_AFTER =>
            {
                Err(io::Error::other(message.clone()))
            }
            _ => Ok(()),
        }
    }

    /// Native Catalog demand always uses the shared bounded producer, including
    /// the default library mode. Explicit tree reads retain their own policy.
    fn native_catalog_only(&self) -> bool {
        self.evidence.catalog_demand.load(Ordering::Relaxed) && !self.is_shown(Kind::Catalog)
    }

    /// Demand for the persisted metadata catalog must not synchronously load
    /// all event indices just because this process has no shown model yet.
    pub(crate) fn note_catalog_read(&self) -> io::Result<()> {
        self.evidence.catalog_demand.store(true, Ordering::Relaxed);
        self.note_read(Kind::Catalog)
    }

    /// An already qualified persisted read needs updates after invalidation,
    /// without starting a cold producer merely because this owner is new.
    pub(crate) fn note_catalog_interest(&self) -> io::Result<()> {
        self.evidence.catalog_demand.store(true, Ordering::Relaxed);
        let pending = crate::events::store::catalog_work_pending(&self.evidence.options)?;
        self.note_read_with(Kind::Catalog, pending)
    }

    /// Wait for the first readable persisted Catalog publication, using the
    /// existing source owner and refresh worker. A warm persisted page returns
    /// before creating a cold demand. The caller must wait off its reactor.
    pub(crate) fn wait_until_initial_catalog(&self, deadline: Instant) -> io::Result<()> {
        self.wait_for_initial_catalog(deadline, None)
    }

    pub(crate) fn wait_until_initial_catalog_in_pool(
        &self,
        pool: Arc<RefreshPool>,
        deadline: Instant,
    ) -> io::Result<()> {
        self.wait_for_initial_catalog(deadline, Some(pool))
    }

    fn wait_for_initial_catalog(
        &self,
        deadline: Instant,
        mut pool: Option<Arc<RefreshPool>>,
    ) -> io::Result<()> {
        let mut state = lock(&self.live.state);
        let mut demanded = false;
        loop {
            if state.closed {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "session catalog source is closed",
                ));
            }
            let ready = crate::session_catalog_page_ready(&self.evidence.options)?;
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "initial session catalog was not published before the deadline",
                ));
            }
            if ready {
                return Ok(());
            }
            let at = Kind::Catalog as usize;
            let stalled = match state.slot {
                Slot::Running => state.running_since.is_some_and(|since| {
                    now.saturating_duration_since(since) >= state.stalled_after
                }),
                Slot::Queued(key) => now.saturating_duration_since(key.0) >= state.stalled_after,
                Slot::Idle => false,
            };
            if stalled {
                return Err(io::Error::other("rebuilds stalled"));
            }
            if let (Some(message), Some(since)) = (&state.error[at], state.failing_since[at])
                && !state.unconfirmed[at]
                && now.saturating_duration_since(since) >= FAILING_AFTER
            {
                return Err(io::Error::other(message.clone()));
            }
            if !demanded {
                if let Some(pool) = pool.take()
                    && !Arc::ptr_eq(&state.pool, &pool)
                {
                    self.set_pool_state(&mut state, pool);
                }
                self.evidence.catalog_demand.store(true, Ordering::Relaxed);
                self.note_read_state(&mut state, Kind::Catalog, true, now)?;
                demanded = true;
                #[cfg(test)]
                INITIAL_CATALOG_WAIT_STARTED.with(|started| {
                    if let Some(send) = started.borrow_mut().take() {
                        let _ = send.send(());
                    }
                });
            }
            let wait = deadline.saturating_duration_since(now);
            let (next, _) = self
                .live
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }

    /// A read's own refresh of the kind it wants, when none is built yet
    /// (the first read of a view, outside [`Refresh::OnRead`]), and of the
    /// other kind if that is shown, after which the view's next background
    /// check is queued. It waits for `work`: a warm or a check building
    /// meanwhile finishes first. Only the wanted kind's error is the
    /// read's.
    fn refresh_first(&self, kind: Kind) -> io::Result<()> {
        #[cfg(test)]
        self.hooks.refreshes_first.fetch_add(1, Ordering::SeqCst);
        let mut work = lock(&self.evidence.work);
        // Logs invalidated before this refresh starts, it sees.
        let invalidated = std::mem::take(&mut lock(&self.live.state).invalidated);
        let started = Instant::now();
        let wanted = self.refresh_kind(&mut work, kind);
        let other = kind.other();
        let others = self
            .is_shown(other)
            .then(|| self.refresh_kind(&mut work, other));
        let built_at = work.built_at;
        if self.evidence.catalog_pending(&work) {
            lock(&self.live.state).invalidated = true;
        }
        drop(work);
        let mut state = lock(&self.live.state);
        state.built_at = built_at;
        if let Some(result) = &others {
            state.record(other, result);
        }
        if wanted.is_ok() {
            state.record(kind, &wanted);
            state.read_at = Some(Instant::now());
            let failing = others.as_ref().is_some_and(Result::is_err);
            let due = self.next_due(&state, started, built_at, failing);
            self.queue(&mut state, due, true);
        } else {
            state.invalidated |= invalidated;
        }
        wanted
    }

    /// When this view's next background check is due, after one that
    /// started at `started`: [`next_check`] in [`Refresh::Background`]. In
    /// [`Refresh::OnInvalidate`] the same if it was invalidated meanwhile
    /// or a rebuild failed, and [`SAFETY_EVERY`] later otherwise.
    fn next_due(
        &self,
        state: &LiveState,
        started: Instant,
        built_at: Option<Instant>,
        failing: bool,
    ) -> Instant {
        if self.refresh() == Refresh::OnInvalidate && !state.invalidated && !failing {
            started + state.safety_every
        } else {
            next_check(started, built_at)
        }
    }

    /// Queues this view's next check at `at` in its pool: moved there if
    /// `exact`, and otherwise only ever earlier. A view is queued once at
    /// most, and never while a worker checks it (the worker queues the
    /// next), once it is closed, or while it is refreshed on read.
    fn queue(&self, state: &mut LiveState, at: Instant, exact: bool) {
        self.queue_by(state, at, exact, false);
    }

    /// [`MachineView::queue`], from the worker that just checked this view
    /// when `by_worker`: that worker is free again at once, so it never
    /// counts as a busy one the pool must start another thread for.
    fn queue_by(&self, state: &mut LiveState, at: Instant, exact: bool, by_worker: bool) {
        if state.closed || (self.refresh() == Refresh::OnRead && !self.native_catalog_only()) {
            return;
        }
        // Never due before it is queued: after a long build the next check
        // is due in the past, and would read as late (to a read's nudge, and
        // as a stall) the moment it is queued.
        let at = at.max(Instant::now());
        let replacing = match state.slot {
            Slot::Running => return,
            Slot::Queued(key) if !exact && key.0 <= at => return,
            Slot::Queued(key) => Some(key),
            Slot::Idle => None,
        };
        state.slot = Slot::Queued(state.pool.queue(replacing, at, self.me.clone(), by_worker));
    }

    /// The embedding server's word that this machine's logs changed. Its
    /// next check is queued as soon as [`REBUILD_SPACING`] after the last
    /// rebuild allows, however often this is called until then: the calls
    /// of one second are one rebuild. A view no one read in the last
    /// [`IDLE_AFTER`] isn't rebuilt for it (its next read answers from the
    /// last model and queues a check at once), but a check it still has
    /// queued is moved up, so its reads never answer from before the change
    /// for long. Refreshed on read, it changes nothing.
    pub(crate) fn invalidate(&self) {
        self.evidence.request_metadata_audit();
        self.invalidate_journaled();
    }

    pub(crate) fn invalidate_journaled(&self) {
        if self.refresh() == Refresh::OnRead && !self.native_catalog_only() {
            return;
        }
        let mut state = lock(&self.live.state);
        if state.closed {
            return;
        }
        state.invalidated = true;
        if state.active() || matches!(state.slot, Slot::Queued(_)) {
            let now = Instant::now();
            let at = state
                .built_at
                .map_or(now, |built| now.max(built + REBUILD_SPACING));
            self.queue(&mut state, at, false);
        }
    }

    /// Runs this view's queued check `key`, on one of its pool's workers.
    /// An entry the view no longer holds (moved or dropped since the worker
    /// took it) is skipped, and so is a view that is closed or went idle.
    pub(crate) fn run_queued(&self, key: Key) {
        {
            let mut state = lock(&self.live.state);
            if state.slot != Slot::Queued(key) {
                return;
            }
            if state.closed
                || !state.active()
                || (self.refresh() == Refresh::OnRead && !self.native_catalog_only())
            {
                state.slot = Slot::Idle;
                return;
            }
            state.slot = Slot::Running;
        }
        /// A check that panics leaves the last model served and the next
        /// check queued, as a failed rebuild does; its worker is replaced.
        struct Checking<'a>(&'a MachineView);
        impl Drop for Checking<'_> {
            fn drop(&mut self) {
                if thread::panicking() {
                    let view = self.0;
                    let mut state = lock(&view.live.state);
                    view.checked(&mut state, Instant::now() + REBUILD_SPACING);
                }
            }
        }
        let checking = Checking(self);
        self.tick(key.0);
        drop(checking);
    }

    /// One background check: rebuilds what is shown if its logs changed,
    /// never sooner than [`REBUILD_SPACING`] after the last rebuild, and
    /// queues the next. The model and the tree are refreshed, and fail,
    /// each on its own.
    fn tick(&self, due: Instant) {
        let _timing = crate::native_timing::Phase::new(&self.evidence.options.cache, "worker_tick");
        let entered = crate::native_timing::enabled().then(Instant::now);
        let mut work = lock(&self.evidence.work);
        let started = Instant::now();
        if let Some(entered) = entered {
            let producer = crate::native_timing::id(&crate::events::EventCache::path(
                &self.evidence.options.cache,
            ));
            eprintln!(
                "semon sessions native: producer={producer} queue_late_us={} work_lock_wait_us={}",
                entered.saturating_duration_since(due).as_micros(),
                started.duration_since(entered).as_micros()
            );
        }
        // A stall counts from here, with `work` taken: a warm or a page's
        // fresh check holding it meanwhile isn't this view's build hanging.
        lock(&self.live.state).running_since = Some(started);
        // Cold discovery steps are continuations of one bounded observation,
        // not repeated rebuilds. Requeue each step behind already due work.
        let discovery_only = self.evidence.catalog_continuation(&work)
            && !self.is_shown(Kind::Catalog)
            && !self.is_shown(Kind::Tree);
        if !discovery_only
            && let Some(built) = work.built_at
            && started < built + REBUILD_SPACING
        {
            drop(work);
            let mut state = lock(&self.live.state);
            state.built_at = Some(built);
            self.checked(&mut state, built + REBUILD_SPACING);
            return;
        }
        // Logs invalidated before this check starts, it sees.
        lock(&self.live.state).invalidated = false;
        let results: Vec<_> = [Kind::Catalog, Kind::Tree]
            .into_iter()
            .filter(|kind| {
                self.is_shown(*kind)
                    || (matches!(kind, Kind::Catalog)
                        && self.evidence.catalog_demand.load(Ordering::Relaxed))
            })
            .map(|kind| {
                let result = if matches!(kind, Kind::Catalog) && !self.is_shown(kind) {
                    self.refresh_catalog_locked(&mut work, model::now_ms())
                        .map(drop)
                } else {
                    self.refresh_kind(&mut work, kind)
                };
                (kind, result)
            })
            .collect();
        let pending = self.evidence.catalog_pending(&work);
        let built_at = work.built_at;
        drop(work);
        let mut state = lock(&self.live.state);
        state.built_at = built_at;
        for (kind, result) in &results {
            state.record(*kind, result);
        }
        let failing = results.iter().any(|(_, result)| result.is_err());
        state.invalidated |= pending;
        let due = if pending && !failing {
            Instant::now()
        } else {
            self.next_due(&state, started, built_at, failing)
        };
        self.checked(&mut state, due);
    }

    /// A worker's check of this view ends: the next is queued at `due`
    /// while the view is read, and [`MachineView::close`] is told.
    fn checked(&self, state: &mut LiveState, due: Instant) {
        state.slot = Slot::Idle;
        state.running_since = None;
        if state.active() {
            self.queue_by(state, due, true, true);
        }
        self.live.changed.notify_all();
    }

    /// No check of this view is queued again, and a queued one is dropped.
    fn stop(state: &mut LiveState) {
        state.closed = true;
        if let Slot::Queued(key) = state.slot {
            state.pool.remove(key);
            state.slot = Slot::Idle;
        }
    }

    /// Stops this view's background checks without waiting: it is no
    /// longer served. [`MachineView::close`] still waits out one running.
    pub(crate) fn retire(&self) {
        self.evidence.retire();
        let mut state = lock(&self.live.state);
        Self::stop(&mut state);
        self.live.changed.notify_all();
    }

    /// Stops this view's background checks and waits out a check or a
    /// rebuild in progress; none starts again. The caller makes sure no
    /// read is in progress or starts after, as [`ViewerCore::close`] does:
    /// a read would still build a model it has none of.
    pub(crate) fn close(&self) {
        self.evidence.retire();
        let mut state = lock(&self.live.state);
        Self::stop(&mut state);
        self.live.changed.notify_all();
        while state.slot == Slot::Running {
            state = self
                .live
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
        // A rebuild a read started has finished too.
        drop(lock(&self.evidence.work));
    }

    fn tree_json(&self) -> io::Result<String> {
        Ok(self.served_tree()?.json.clone())
    }

    /// Answers one request: `path` and `query` split at the `?`, still
    /// percent-encoded, and the request's `If-None-Match`. Only GET is
    /// answered (405 otherwise); every URL the viewer uses is a GET. The
    /// caller authenticates first: the core serves whoever it is handed.
    pub(crate) fn respond(
        &self,
        method: &str,
        path: &str,
        query: &str,
        _if_none_match: Option<&str>,
    ) -> ViewerReply {
        let text = "text/plain; charset=utf-8";
        let reply = |status, body: &[u8]| ViewerReply {
            status,
            content_type: text,
            body: body.to_vec(),
            etag: None,
        };
        if method != "GET" {
            return reply(405, b"Method not allowed");
        }
        let answer = self
            .route(path, query)
            .map(|(status, content_type, body)| (status, content_type, body, None));
        match answer {
            Ok((status, content_type, body, etag)) => ViewerReply {
                status,
                content_type,
                body,
                etag,
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => reply(404, b"Not found"),
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                reply(400, b"Invalid request")
            }
            Err(error) => {
                eprintln!("semon sessions viewer: {error}");
                reply(500, b"Internal error")
            }
        }
    }
    fn route(&self, path: &str, _query: &str) -> io::Result<(u16, &'static str, Vec<u8>)> {
        let html = "text/html; charset=utf-8";
        let json = "application/json; charset=utf-8";
        match path {
            "/" | "/analytics" | "/sessions" | "/machines" => Ok((200, html, PAGE.into())),
            "/viewer.js" => Ok((
                200,
                "text/javascript; charset=utf-8",
                crate::shell::VIEWER_JS.into(),
            )),
            "/viewer.css" => Ok((
                200,
                "text/css; charset=utf-8",
                crate::shell::VIEWER_CSS.into(),
            )),
            "/mark.svg" => Ok((200, "image/svg+xml", include_str!("mark.svg").into())),
            _ if crate::HARNESS_ICONS
                .iter()
                .any(|(icon_path, _)| *icon_path == path) =>
            {
                let svg = crate::HARNESS_ICONS
                    .iter()
                    .find_map(|(icon_path, svg)| (*icon_path == path).then_some(*svg))
                    .expect("icon path was matched in HARNESS_ICONS");
                Ok((200, "image/svg+xml", svg.as_bytes().to_vec()))
            }
            "/favicon.svg" => Ok((200, "image/svg+xml", include_str!("favicon.svg").into())),
            "/shell.js" => Ok((
                200,
                "text/javascript; charset=utf-8",
                crate::shell::JS.into(),
            )),
            "/shell.css" => Ok((200, "text/css; charset=utf-8", crate::shell::CSS.into())),
            "/api/tree" => Ok((200, json, self.tree_json()?.into_bytes())),
            _ if path.starts_with("/fonts/") => {
                let name = path["/fonts/".len()..]
                    .strip_suffix(".woff2")
                    .ok_or(io::ErrorKind::NotFound)?;
                FONTS
                    .iter()
                    .find(|(font, _)| *font == name)
                    .map(|(_, bytes)| (200, "font/woff2", bytes.to_vec()))
                    .ok_or_else(|| io::ErrorKind::NotFound.into())
            }
            _ => Err(io::ErrorKind::NotFound.into()),
        }
    }
}

impl Drop for MachineView {
    /// Its queued check leaves the pool with it: a pool's queue holds only
    /// views that are alive.
    fn drop(&mut self) {
        let state = self
            .live
            .state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if let Slot::Queued(key) = state.slot {
            state.pool.remove(key);
        }
    }
}

impl MachineView {
    /// The V1 tree's roots, as a request is answered.
    pub(crate) fn tree_roots(&self) -> io::Result<Vec<Node>> {
        Ok(self.served_tree()?.roots.clone())
    }
}

/// Whether `path` is a page URL that names something: `/machines/<id>`, by its prefix.
pub(crate) fn is_named_page(path: &str) -> bool {
    path.starts_with("/machines/")
}

/// A page URL's path segments, decoded; `None` when one doesn't decode.
pub(crate) fn page_parts(path: &str) -> Option<Vec<String>> {
    path.trim_start_matches('/')
        .split('/')
        .map(decoded)
        .collect()
}
pub(crate) fn percent_encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        io::{Read, Write},
        net::TcpStream,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, UNIX_EPOCH},
    };

    use serde_json::json;

    use super::*;
    use crate::{native_evidence::Stamp, tx};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A view's test hooks, seen from any thread (a background rebuild runs
    /// on a refresh pool's).
    #[derive(Default)]
    pub(super) struct Hooks {
        /// How many times the model was built.
        builds: AtomicU64,
        /// How many stat passes were taken over its logs.
        pub(super) stats: AtomicU64,
        /// How many times a read refreshed it itself (outside OnRead).
        pub(super) refreshes_first: AtomicU64,
        /// Runs in each build, after the files are stamped.
        building: Mutex<Option<Hook>>,
        /// Builds fail while set.
        failing: AtomicBool,
        /// V1 tree builds fail while set.
        pub(super) tree_failing: AtomicBool,
    }

    type Hook = Arc<dyn Fn() + Send + Sync>;

    impl Hooks {
        pub(super) fn building(&self) -> io::Result<()> {
            self.builds.fetch_add(1, Ordering::SeqCst);
            let hook = lock(&self.building).clone();
            if let Some(hook) = hook {
                hook();
            }
            if self.failing.load(Ordering::SeqCst) {
                return Err(io::Error::other("a build failed on purpose"));
            }
            Ok(())
        }

        fn builds(&self) -> u64 {
            self.builds.load(Ordering::SeqCst)
        }

        /// Holds every build from now on until the returned sender is
        /// dropped; the receiver hears when one starts.
        fn hold(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (started, starts) = mpsc::channel();
            let (release, released) = mpsc::channel::<()>();
            let (started, released) = (Mutex::new(started), Mutex::new(released));
            *lock(&self.building) = Some(Arc::new(move || {
                let _ = lock(&started).send(());
                let _ = lock(&released).recv();
            }));
            (starts, release)
        }
    }

    struct Fixture {
        root: PathBuf,
        options: Options,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-viewer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let options = Options {
                claude_home: root.join("claude"),
                claude_json: root.join(".claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                since: Duration::from_secs(86400),
                session: None,
                facts: None,
                scan_window: false,
            };
            fs::create_dir_all(&options.proc_root).unwrap();
            fs::write(options.proc_root.join("locks"), "").unwrap();
            Self { root, options }
        }

        fn write(&self, relative: &str, content: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            path
        }

        fn claude(&self, id: &str, records: &[Value]) -> PathBuf {
            let content = if records.is_empty() {
                String::new()
            } else {
                records
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n"
            };
            self.write(&format!("claude/projects/project/{id}.jsonl"), &content)
        }

        fn codex(&self, id: &str, records: &[Value]) -> PathBuf {
            let mut lines = vec![
                json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":{"id":id,"cwd":"/tmp/project"}}),
            ];
            lines.extend_from_slice(records);
            let content = lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            self.write(
                &format!("codex/sessions/2026/09/24/rollout-{id}.jsonl"),
                &content,
            )
        }

        /// A view with a refresh pool of its own, so that tests running
        /// side by side don't share workers unless they mean to.
        fn viewer(&self) -> Arc<MachineView> {
            let view = MachineView::new(self.options.clone());
            view.set_pool(RefreshPool::new(2));
            view
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn claude_record(id: &str, blocks: Vec<Value>) -> Value {
        json!({"type":"assistant","timestamp":"2026-09-24T00:00:00Z","sessionId":id,"message":{"content":blocks}})
    }

    fn http(fixture: &Fixture, request: &str) -> String {
        http_with_publication(fixture, request, None, false)
    }

    fn http_with_publication(
        fixture: &Fixture,
        request: &str,
        expected_session: Option<&str>,
        hold_busy: bool,
    ) -> String {
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let viewer = Viewer {
            control: None,
            bootstrap_used: Mutex::new(false),
            core: ViewerCore::new(fixture.options.clone()),
            token: "0123456789abcdef0123456789abcdef".into(),
            bootstrap: "0123456789abcdef0123456789abcdef".into(),
            port,
        };
        if let Some(expected_session) = expected_session {
            // This fixture owns a known warm publication. Prepare it directly,
            // without scheduling an unrelated producer between qualification
            // and the HTTP request. Busy-index behavior has its own held fixture.
            let view = viewer.core.machine_views().into_iter().next().unwrap();
            let mut work = lock(&view.evidence.work);
            eventually("HTTP session fixture publication", || {
                view.refresh_catalog_locked(&mut work, model::now_ms())
                    .unwrap();
                let page = crate::session_catalog_page(&fixture.options, "local", "limit=60");
                let page_body: Value = serde_json::from_slice(&page.body).ok()?;
                let located = crate::session_bookmark(
                    &fixture.options,
                    "local",
                    &format!("machine=local&sid={expected_session}"),
                );
                let located_body: Value = serde_json::from_slice(&located.body).ok()?;
                (page.status == 200
                    && page_body["completeness"]["state"] == "complete"
                    && located.status == 200
                    && located_body["unavailable"] == 0
                    && located_body["matches"].as_array().is_some_and(|matches| {
                        matches.iter().any(|item| {
                            item["source_key"] == "local" && item["catalog_key"] == expected_session
                        })
                    }))
                .then_some(())
            });
        }
        let busy_view = hold_busy.then(|| viewer.core.machine_views().into_iter().next().unwrap());
        let _busy = busy_view
            .as_ref()
            .map(|view| view.evidence.work.lock().unwrap());
        let request = request.replace("PORT", &port.to_string());
        let worker = thread::spawn(move || {
            viewer.handle(server.recv().unwrap());
            viewer.core.close();
        });
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        // Keep the producer busy through the response, then let close() drain it.
        drop(_busy);
        worker.join().unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    #[test]
    fn collection_control_refuses_authenticated_stale_and_malformed_writes_without_reads() {
        let fixture = Fixture::new();
        fixture.claude(
            "private",
            &[json!({"type":"user","sessionId":"private","message":{"content":"Do not read"}})],
        );
        for body in [
            r#"{"op":"send","thread":"stale","text":"never deliver"}"#,
            "malformed",
        ] {
            let response = http(
                &fixture,
                &format!(
                    "POST /api/control HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\nOrigin: http://127.0.0.1:PORT\r\nCookie: semon_session=0123456789abcdef0123456789abcdef\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
            );
            assert!(response.starts_with("HTTP/1.1 503"), "{response}");
            assert!(response.contains("capability_unavailable"), "{response}");
        }
        let forbidden = http(
            &fixture,
            "POST /api/control HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        assert!(forbidden.starts_with("HTTP/1.1 403"));
        let status = http(
            &fixture,
            "GET /api/session-control?sid=stale&t=0123456789abcdef0123456789abcdef HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\nConnection: close\r\n\r\n",
        );
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        assert!(status.contains("\"control\":null"));
        assert!(status.contains("\"control_available\":false"));
        assert!(!fixture.options.cache.exists());
        assert!(!crate::events::EventCache::path(&fixture.options.cache).exists());
    }

    #[test]
    fn http_session_bootstrap_is_consumed_before_routing() {
        let fixture = Fixture::new();
        fixture.claude("bootstrap-session", &[
            json!({"type":"user","sessionId":"bootstrap-session","uuid":"u","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"Bootstrap transcript"}}),
        ]);
        let token = "0123456789abcdef0123456789abcdef";
        let cold = http_with_publication(
            &fixture,
            &format!(
                "GET /session/local/bootstrap-session?t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\nConnection: close\r\n\r\n"
            ),
            None,
            true,
        );
        assert!(cold.starts_with("HTTP/1.1 200"), "{cold}");
        let (cold_headers, cold_body) = cold.split_once("\r\n\r\n").unwrap();
        assert!(!cold_body.contains(token), "credential leaked into content");
        assert!(cold_body.contains("<!doctype html") || cold_body.contains("<!DOCTYPE html"));
        let cookie = cold_headers
            .lines()
            .find(|line| line.starts_with("Set-Cookie:"))
            .unwrap();
        assert!(cookie.contains(token), "{cookie}");
        for header in cold_headers.lines().skip(1) {
            if !header.starts_with("Set-Cookie:") {
                assert!(
                    !header.contains(token),
                    "credential leaked into header: {header}"
                );
            }
        }
        let continuation = http_with_publication(
            &fixture,
            &format!(
                "GET /session/local/bootstrap-session HTTP/1.1\r\nHost: localhost:PORT\r\nCookie: semon_session={token}\r\nConnection: close\r\n\r\n"
            ),
            Some("bootstrap-session"),
            false,
        );
        assert!(continuation.starts_with("HTTP/1.1 200"), "{continuation}");
        assert!(continuation.contains("<!doctype html") || continuation.contains("<!DOCTYPE html"));
        for query in [
            "t=invalid".to_owned(),
            format!("t={token}&t={token}"),
            format!("t={token}&t=invalid"),
            format!("t={token}&%74={token}"),
        ] {
            for cookie in [String::new(), format!("Cookie: semon_session={token}\r\n")] {
                let wire = http(
                    &fixture,
                    &format!(
                        "GET /session/local/bootstrap-session?{query} HTTP/1.1\r\nHost: localhost:PORT\r\n{cookie}Connection: close\r\n\r\n"
                    ),
                );
                assert!(wire.starts_with("HTTP/1.1 403"), "{wire}");
            }
        }
    }

    #[test]
    fn direct_session_shell_is_available_during_busy_index() {
        let fixture = Fixture::new();
        fixture.claude(
            "readiness",
            &[json!({
                "type":"user",
                "sessionId":"readiness",
                "uuid":"u",
                "timestamp":"2026-10-01T00:00:00Z",
                "message":{"role":"user","content":"Cold native observation"}
            })],
        );
        let core = ViewerCore::new(fixture.options.clone());
        let view = core.machine_views().into_iter().next().unwrap();
        let work = view.evidence.work.lock().unwrap();
        let shell = core.respond("GET", "/session/local/readiness", "", None);
        assert_eq!(shell.status, 200);
        assert!(shell.content_type.starts_with("text/html"));
        drop(work);
        core.close();
    }

    #[test]
    fn http_guards_and_headers() {
        let fixture = Fixture::new();
        let token = "0123456789abcdef0123456789abcdef";
        let good = http(
            &fixture,
            &format!("GET /?t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n"),
        );
        assert!(good.starts_with("HTTP/1.1 200"));
        assert!(good.contains("JavaScript is required"));
        for header in [
            "Cache-Control: no-store",
            "Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'",
            "X-Frame-Options: DENY",
            "Referrer-Policy: no-referrer",
            "HttpOnly; SameSite=Strict",
        ] {
            assert!(good.contains(header), "missing {header}");
        }
        assert!(http(&fixture, &format!("GET /api/tree HTTP/1.1\r\nHost: localhost:PORT\r\nCookie: semon_session={token}\r\n\r\n")).starts_with("HTTP/1.1 200"));
        for request in [
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n".to_owned(),
            format!("GET /?t={token} HTTP/1.1\r\nHost: evil.test:PORT\r\n\r\n"),
            format!(
                "POST /?t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\nContent-Length: 0\r\n\r\n"
            ),
            "GET /?t=wrong HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n".to_owned(),
            format!(
                "GET /?t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\nHost: evil.test:PORT\r\n\r\n"
            ),
        ] {
            assert!(http(&fixture, &request).starts_with("HTTP/1.1 403"));
        }
        let random = random_token().unwrap();
        assert_eq!(random.len(), 32);
        assert!(random.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn late_native_name_refreshes_catalog_without_moving_transcript_projection() {
        let fixture = Fixture::new();
        let id = "00000000-0000-0000-0000-000000000077";
        fixture.codex(id, &[]);
        let sources =
            crate::SessionSources::with_machines(vec![(String::new(), fixture.options.clone())]);
        let evidence = &sources.machines[0].1;
        let prepare = || {
            let mut work = lock(&evidence.work);
            for _ in 0..16 {
                if evidence
                    .refresh_catalog(&mut work, model::now_ms())
                    .unwrap()
                {
                    return;
                }
            }
            panic!("single-source native Catalog publication did not complete");
        };
        let row = || {
            let reply = crate::session_catalog_page(&fixture.options, "fixture", "limit=100");
            assert_eq!(reply.status, 200);
            let page: Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(page["completeness"]["state"], "complete");
            assert_eq!(page["items"].as_array().unwrap().len(), 1);
            page["items"][0].clone()
        };
        let turns = || {
            let db = rusqlite::Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
            let mut selected = db.prepare("SELECT metadata FROM session_trace_turns WHERE session_key=?1 ORDER BY turn_id").unwrap();
            selected
                .query_map([id], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>()
        };
        prepare();
        let before = row();
        let before_turns = turns();
        fixture.write("codex/session_index.jsonl", &(json!({"id":id,"thread_name":"Meaningful late name","updated_at":"2026-10-10T00:00:00Z"}).to_string() + "\n"));
        prepare();
        let after = row();
        assert_eq!(after["name"], "Meaningful late name");
        assert_ne!(before["name"], after["name"]);
        assert_eq!(before["transcript"], after["transcript"]);
        assert_eq!(before["sources"], after["sources"]);
        assert_eq!(before_turns, turns());
        sources.close();
    }

    #[test]
    fn native_transcript_json_preserves_security_headers_and_generation_fencing() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        fixture.claude(
            "xss",
            &[json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"xss","origin":{"kind":"human"},
                "message":{"role":"user","content":"<script>alert(1)</script>"}})],
        );
        model::observe_catalog(
            &fixture.options,
            &mut EventCache::open_scoped(&fixture.options.cache).unwrap(),
            &mut false,
            &mut model::Texts::default(),
            model::now_ms(),
        )
        .unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        let get = |extra: &str, query: &str| {
            http(
                &fixture,
                &format!(
                    "GET /api/session-transcript?machine=local&sid=xss&after=0&limit=1&t={token}{query} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n{extra}\r\n"
                ),
            )
        };
        let wire = get("", "");
        assert!(wire.starts_with("HTTP/1.1 200"));
        for header in [
            "Content-Type: application/json; charset=utf-8",
            "Cache-Control: no-store",
            "Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'",
            "X-Frame-Options: DENY",
            "Referrer-Policy: no-referrer",
        ] {
            assert!(wire.contains(header), "missing {header}");
        }
        let body = wire.split_once("\r\n\r\n").unwrap().1;
        let selected: Value = serde_json::from_str(body).unwrap();
        for key in [
            "api",
            "identity",
            "session",
            "projection",
            "range",
            "entries",
            "content_observation",
        ] {
            assert!(selected.get(key).is_some(), "missing {key}");
        }
        assert_eq!(selected["identity"]["source_key"], "local");
        assert_eq!(selected["identity"]["catalog_key"], "xss");
        assert_eq!(selected["entries"][0]["text"], "<script>alert(1)</script>");
        assert_eq!(selected["entries"][0]["freshness"]["state"], "cached");
        let generation = selected["projection"]["generation"].as_str().unwrap();
        assert_eq!(generation.len(), 64);
        assert!(get("", &format!("&generation={generation}")).starts_with("HTTP/1.1 200"));
        let stale = get("", &format!("&generation={}", "0".repeat(64)));
        assert!(stale.starts_with("HTTP/1.1 409"));
        assert!(
            !stale.contains("<script>"),
            "stale observations expose no content"
        );
        let refused = http(
            &fixture,
            "GET /api/session-transcript?machine=local&sid=xss&after=0&limit=1 HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n",
        );
        assert!(refused.starts_with("HTTP/1.1 403"));
    }

    #[test]
    fn native_source_api_version_is_served_without_model_preparation() {
        let fixture = Fixture::new();
        let core = ViewerCore::new(fixture.options.clone());
        let reply = core.respond("GET", "/api/session-sources", "", None);
        assert_eq!(reply.status, 200);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["api"], 1);
        core.close();
    }

    /// Live polling (#40 M3): `?since=` answers 304 while nothing changed,
    /// without parsing a line; any appended line changes the version, even
    /// one that changes nothing else in the model, and the rebuild parses
    /// that line only and reads no other file's metadata.
    #[test]
    fn native_catalog_generations_reuse_indexed_evidence_and_read_only_changed_records() {
        let fixture = lane_fixture();
        fixture.claude(
            "other",
            &[
                json!({"type":"user","timestamp":"2026-09-24T00:02:00Z","sessionId":"other","origin":{"kind":"human"},
                    "message":{"role":"user","content":"another lane"}}),
            ],
        );
        let codex = fixture.codex(
            "run",
            &[
                json!({"type":"response_item","timestamp":"2026-09-24T00:03:00Z",
                    "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"a codex run"}]}}),
            ],
        );
        fixture.write(
            "claude/projects/project/lane/subagents/agent-sub.jsonl",
            &(json!({"type":"user","timestamp":"2026-09-24T00:04:00Z","sessionId":"lane","agentId":"sub","isSidechain":true,
                "message":{"role":"user","content":"check the lane"}})
            .to_string()
                + "\n"),
        );
        let meta = fixture.write(
            "claude/projects/project/lane/subagents/agent-sub.meta.json",
            r#"{"agentType":"general-purpose","description":"Checker"}"#,
        );
        let viewer = fixture.viewer();
        let version = |page: &Value| page["generation"].as_str().unwrap().to_owned();
        let parsed = || crate::events::PARSED.with(std::cell::Cell::get);
        let metas = || crate::events::CLAUDE_SIDECAR_READS.with(std::cell::Cell::get);
        let headers = || crate::events::CODEX_HEADER_READS.with(std::cell::Cell::get);
        let marks = |page: &Value| -> Value {
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    (
                        row["key"].as_str().unwrap().to_owned(),
                        Value::String(row["source_refs"].to_string()),
                    )
                })
                .collect::<serde_json::Map<_, _>>()
                .into()
        };
        let append = |path: &Path, line: Value| {
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{line}").unwrap();
        };
        let first = publish_native(&viewer);
        let marks1 = marks(&first);
        assert!(
            marks1["lane"].is_string() && marks1["other"].is_string() && marks1["run"].is_string()
        );
        assert_eq!(first["completeness"]["state"], "complete");
        let v1 = version(&first);
        let (lines, reads, header_reads) = (parsed(), metas(), headers());
        for _ in 0..3 {
            let poll = publish_native(&viewer);
            assert_eq!(version(&poll), v1);
        }
        assert_eq!(
            (parsed(), metas(), headers()),
            (lines, reads, header_reads),
            "idle polls parse nothing"
        );

        let path = fixture.root.join("claude/projects/project/lane.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"user","timestamp":"2026-09-24T00:05:00Z","sessionId":"lane","origin":{"kind":"human"},
                "message":{"role":"user","content":"one more"}})
        )
        .unwrap();
        drop(file);
        let changed = publish_native(&viewer);
        assert_ne!(version(&changed), v1);
        let v2 = version(&changed);
        assert_ne!(v2, v1, "an appended line changes the version");

        assert_eq!(parsed(), lines + 1, "only the appended line is parsed");
        assert_eq!(metas(), reads, "no unchanged file's metadata is read again");
        assert_eq!(
            headers(),
            header_reads,
            "unchanged Codex headers stay indexed"
        );
        // Only the grown transcript's mark moves: the page tails only it.
        let marks2 = marks(&changed);
        assert_ne!(marks2["lane"], marks1["lane"]);
        assert_eq!(marks2["other"], marks1["other"]);
        assert_eq!(marks2["run"], marks1["run"]);
        assert_eq!(version(&publish_native(&viewer)), v2);
        assert_eq!(parsed(), lines + 1);

        // A line that changes nothing else in the model (the same timestamp
        // as the one before, no entry of its own) still gives a new version.
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"system","subtype":"turn_duration","timestamp":"2026-09-24T00:05:00Z","sessionId":"lane"})
        )
        .unwrap();
        drop(file);
        let quiet = publish_native(&viewer);
        assert_ne!(
            version(&quiet),
            v2,
            "any appended line changes the generation"
        );
        let v3 = version(&quiet);
        assert_ne!(v3, v2);
        assert_eq!(parsed(), lines + 2);
        assert_eq!(version(&publish_native(&viewer)), v3);

        // Codex growth parses only the new event and binds its first-record
        // header to the new ledger, with a final raw-header recheck.
        append(
            &codex,
            json!({"type":"response_item","timestamp":"2026-09-24T00:06:00Z",
                "payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}),
        );
        let grown = publish_native(&viewer);
        assert_ne!(version(&grown), v3);
        assert_eq!(parsed(), lines + 3);
        assert_eq!(
            metas(),
            reads,
            "a Codex file that grew does not reread unchanged Claude sidecars"
        );
        assert_eq!(headers(), header_reads + 2);
        // A subagent's .meta.json that changes is read again, and only it.
        fs::write(
            &meta,
            r#"{"agentType":"general-purpose","description":"Checker, renamed"}"#,
        )
        .unwrap();
        let renamed = publish_native(&viewer);
        assert_ne!(version(&renamed), version(&grown));
        assert_eq!(metas(), reads + 1, "only the changed .meta.json is read");
        assert_eq!(headers(), header_reads + 2);
        assert_eq!(parsed(), lines + 3);
    }

    /// A process that dies mid-call writes no line, but its running step is
    /// no longer running: the transcript's mark moves, so the page tails it.
    #[cfg(unix)]
    #[test]
    fn a_process_that_dies_mid_call_moves_its_transcripts_mark() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        fixture.claude(
            "lane",
            &[
                json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"lane","origin":{"kind":"human"},
                    "message":{"role":"user","content":"run the suite"}}),
                json!({"type":"assistant","timestamp":"2026-09-24T00:01:00Z","sessionId":"lane",
                    "message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}),
            ],
        );
        let mut fields = vec!["0"; 20];
        fields[19] = "777";
        let stat = fixture.write(
            "proc/100/stat",
            &format!("100 (claude) {}\n", fields.join(" ")),
        );
        fixture.write(
            "claude/sessions/100.json",
            &json!({"pid":100,"sessionId":"lane","procStart":777,"status":"busy","name":"lane"})
                .to_string(),
        );
        let viewer = fixture.viewer();
        publish_native(&viewer);
        let selected = native_selected(&viewer, "lane");
        assert_eq!(selected["session"]["details"]["state"], "work");
        let running: Vec<_> = selected["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["live"] == true)
            .collect();
        assert_eq!(running.len(), 1, "one running call or yielded step");
        fs::remove_file(stat).unwrap();
        publish_native(&viewer);
        let stopped = native_selected(&viewer, "lane");
        assert_ne!(
            stopped["projection"]["errors_generation"], selected["projection"]["errors_generation"],
            "process ending updates the qualified runtime observation"
        );
        assert_ne!(stopped["session"]["details"]["state"], "work");
        assert_ne!(
            stopped["session"]["details"]["content_generation"],
            selected["session"]["details"]["content_generation"],
            "runtime projection changed"
        );
        assert_eq!(
            stopped["projection"]["generation"], selected["projection"]["generation"],
            "no source line was written"
        );
        assert!(
            stopped["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["live"] != true)
        );
        assert!(
            stopped["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["unfinished"] == true && entry["ok"] == false)
        );
        assert_retired_routes_absent(&viewer);
    }

    /// The same for a Codex command that outlived its yield: its process
    /// dies before the next poll writes anything, and the step it is stops
    /// running all the same.
    #[cfg(unix)]
    #[test]
    fn a_codex_process_that_dies_mid_yield_moves_its_transcripts_mark() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        let result = json!({"wall_time_seconds":1.0,"session_id":4242,"output":"watching\n"});
        fixture.codex(
            "run",
            &[
                json!({"type":"event_msg","timestamp":"2026-09-24T00:00:10Z","payload":{"type":"task_started","turn_id":"watch-turn"}}),
                json!({"type":"response_item","timestamp":"2026-09-24T00:00:10Z","payload":{"type":"message","role":"user",
                    "content":[{"type":"input_text","text":"watch the checks"}]}}),
                json!({"type":"response_item","timestamp":"2026-09-24T00:01:00Z","payload":{"type":"custom_tool_call","call_id":"start","name":"exec",
                    "input":"const r = await tools.exec_command({cmd:\"make watch\",yield_time_ms:1000});\ntext(JSON.stringify(r));"}}),
                json!({"type":"response_item","timestamp":"2026-09-24T00:01:01Z","payload":{"type":"custom_tool_call_output","call_id":"start",
                    "output":[{"type":"input_text","text":"Script completed\nWall time 1.0 seconds\nOutput:\n"},{"type":"input_text","text":result.to_string()}]}}),
            ],
        );
        let lock = fixture.write("codex/thread-writer-locks/run.lock", "");
        let (major, minor, ino) = crate::lock_identity(&lock).unwrap();
        fixture.write(
            "proc/locks",
            &format!("2: FLOCK  ADVISORY  WRITE 1234 {major:02x}:{minor:02x}:{ino} 0 EOF\n"),
        );
        let viewer = fixture.viewer();
        publish_native(&viewer);
        let selected = native_selected(&viewer, "run");
        assert_eq!(selected["session"]["details"]["state"], "work");
        let running: Vec<_> = selected["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["live"] == true)
            .collect();
        assert_eq!(running.len(), 1, "one running call or yielded step");
        fixture.write("proc/locks", "");
        publish_native(&viewer);
        let stopped = native_selected(&viewer, "run");
        assert_ne!(
            stopped["projection"]["errors_generation"], selected["projection"]["errors_generation"],
            "process ending updates the qualified runtime observation"
        );
        assert_ne!(stopped["session"]["details"]["state"], "work");
        assert_ne!(
            stopped["session"]["details"]["content_generation"],
            selected["session"]["details"]["content_generation"],
            "runtime projection changed"
        );
        assert_eq!(
            stopped["projection"]["generation"], selected["projection"]["generation"],
            "no source line was written"
        );
        assert!(
            stopped["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["live"] != true)
        );
        assert!(
            stopped["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["unfinished"] == true && entry["ok"] == false)
        );
        assert_retired_routes_absent(&viewer);
    }

    /// Two ways a poll could miss or waste a build: a line that lands while a
    /// build runs is seen by the next poll (the files are stamped before the
    /// build reads them), and a lock anywhere else on the machine doesn't
    /// rebuild (only the Codex writer locks' lines of /proc/locks count).
    #[cfg(unix)]
    #[test]
    fn a_line_during_a_build_is_seen_and_other_locks_dont_rebuild() {
        let fixture = lane_fixture();
        let writer_lock = fixture.write("codex/thread-writer-locks/run.lock", "");
        let path = fixture.root.join("claude/projects/project/lane.jsonl");
        let viewer = fixture.viewer();
        let builds = || crate::model::BUILDS.with(std::cell::Cell::get);
        let first = publish_native(&viewer);
        let v1 = first["generation"].clone();

        // Appended after the scan read the file, before the build is done.
        let late = path.clone();
        crate::model::AFTER_SCAN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let mut file = fs::OpenOptions::new().append(true).open(&late).unwrap();
                writeln!(
                    file,
                    "{}",
                    json!({"type":"user","timestamp":"2026-09-24T00:07:00Z","sessionId":"lane","origin":{"kind":"human"},
                        "message":{"role":"user","content":"landed mid-build"}})
                )
                .unwrap();
            }));
        });
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"user","timestamp":"2026-09-24T00:06:00Z","sessionId":"lane","origin":{"kind":"human"},
                "message":{"role":"user","content":"before the build"}})
        )
        .unwrap();
        drop(file);
        let during = {
            let mut work = lock(&viewer.evidence.work);
            viewer.evidence.refresh_catalog(&mut work, model::now_ms())
        };
        if let Err(error) = during {
            assert_eq!(
                error.kind(),
                io::ErrorKind::WouldBlock,
                "concurrent source append must be fenced"
            );
        } else {
            let reply = crate::session_transcript_range(
                viewer.options(),
                "fixture",
                "sid=lane&limit=100",
                None,
            );
            assert_eq!(
                reply.status, 409,
                "an append beyond the accepted publication requires resynchronization"
            );
            assert_eq!(body_of(&reply)["resynchronize"], true);
            assert!(!String::from_utf8_lossy(&reply.body).contains("landed mid-build"));
        }
        let next = publish_native(&viewer);
        assert_ne!(
            next["generation"], v1,
            "the concurrent line is observed on the next publication"
        );
        let selected = native_selected(&viewer, "lane");
        assert!(selected.to_string().contains("landed mid-build"));
        let v3 = next["generation"].clone();
        assert_eq!(publish_native(&viewer)["generation"], v3);

        // A lock on some other file: no build.
        let before = builds();
        fixture.write(
            "proc/locks",
            "1: POSIX  ADVISORY  WRITE 999 08:01:424242 0 EOF\n",
        );
        assert_eq!(publish_native(&viewer)["generation"], v3);
        assert_eq!(
            builds(),
            before,
            "an unrelated lock rebuilt native semantics"
        );
        // The writer lock taken: a build.
        let (major, minor, ino) = crate::lock_identity(&writer_lock).unwrap();
        fixture.write(
            "proc/locks",
            &format!("1: POSIX  ADVISORY  WRITE 999 08:01:424242 0 EOF\n2: FLOCK  ADVISORY  WRITE 1234 {major:02x}:{minor:02x}:{ino} 0 EOF\n"),
        );
        publish_native(&viewer);
        assert_eq!(
            builds(),
            before + 1,
            "the writer lock didn't refresh native semantics"
        );
    }

    const HEADERS: [&str; 5] = [
        "Cache-Control: no-store",
        "Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'",
        "X-Frame-Options: DENY",
        "Referrer-Policy: no-referrer",
        "X-Content-Type-Options: nosniff",
    ];

    fn get(fixture: &Fixture, path: &str) -> String {
        let token = "0123456789abcdef0123456789abcdef";
        let separator = if path.contains('?') { '&' } else { '?' };
        http(
            fixture,
            &format!("GET {path}{separator}t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n"),
        )
    }

    /// A lane with a turn, for the page and transcript routes.
    fn lane_fixture() -> Fixture {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        fixture.claude(
            "lane",
            &[
                json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"lane","origin":{"kind":"human"},
                    "message":{"role":"user","content":"<script>alert(1)</script>"}}),
                json!({"type":"assistant","timestamp":"2026-09-24T00:01:00Z","sessionId":"lane",
                    "message":{"role":"assistant","content":[{"type":"text","text":"<img src=x onerror=alert(2)>"}]}}),
            ],
        );
        fixture
    }

    #[test]
    fn every_screen_url_serves_the_page_and_unknown_ones_404() {
        let fixture = lane_fixture();
        let viewer = fixture.viewer();
        publish_native(&viewer);
        let selected = native_selected(&viewer, "lane");
        let turn = selected["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|entry| entry["turn"].as_str())
            .unwrap()
            .to_owned();
        for path in [
            "/".to_owned(),
            "/analytics".into(),
            "/sessions".into(),
            "/machines".into(),
            "/machines/testbox".into(),
            "/session/local/lane".into(),
            format!("/session/local/lane/trace/{turn}"),
        ] {
            let wire = get(&fixture, &path);
            assert!(wire.starts_with("HTTP/1.1 200"), "{path}");
            assert!(
                wire.contains("Content-Type: text/html; charset=utf-8"),
                "{path}"
            );
            assert!(wire.ends_with(PAGE), "{path}");
            for header in HEADERS {
                assert!(wire.contains(header), "{path}: missing {header}");
            }
        }
        for path in [
            "/machines/elsewhere",
            "/s/claude/lane",
            "/s/claude/nobody",
            "/trace/claude/lane/no-such-turn",
            "/timeline",
            "/fonts/nope.woff2",
            "/elsewhere",
        ] {
            let wire = get(&fixture, path);
            assert!(wire.starts_with("HTTP/1.1 404"), "{path}");
            for header in HEADERS {
                assert!(wire.contains(header), "{path}: missing {header}");
            }
        }
        // The token guards pages as it guards the API.
        assert!(
            http(
                &fixture,
                "GET /analytics HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n"
            )
            .starts_with("HTTP/1.1 403")
        );
    }

    #[test]
    fn assets_and_fonts_carry_the_headers() {
        let fixture = lane_fixture();
        publish_native(&fixture.viewer());
        for (path, content_type) in [
            ("/viewer.js", "text/javascript; charset=utf-8"),
            ("/viewer.css", "text/css; charset=utf-8"),
            ("/shell.js", "text/javascript; charset=utf-8"),
            ("/shell.css", "text/css; charset=utf-8"),
            ("/mark.svg", "image/svg+xml"),
            ("/harness/claude-code.svg", "image/svg+xml"),
            ("/harness/codex.svg", "image/svg+xml"),
            ("/harness/codex-black.svg", "image/svg+xml"),
            ("/harness/opencode-light.svg", "image/svg+xml"),
            ("/harness/opencode-dark.svg", "image/svg+xml"),
            ("/favicon.svg", "image/svg+xml"),
            ("/fonts/instrument-sans-latin.woff2", "font/woff2"),
            ("/fonts/instrument-sans-latin-ext.woff2", "font/woff2"),
            ("/fonts/jetbrains-mono-latin.woff2", "font/woff2"),
            ("/fonts/jetbrains-mono-latin-ext.woff2", "font/woff2"),
            ("/api/sessions?sid=lane", "application/json; charset=utf-8"),
            (
                "/api/session-transcript?sid=lane&limit=100",
                "application/json; charset=utf-8",
            ),
            ("/api/tree", "application/json; charset=utf-8"),
        ] {
            let wire = get(&fixture, path);
            assert!(wire.starts_with("HTTP/1.1 200"), "{path}");
            assert!(
                wire.contains(&format!("Content-Type: {content_type}")),
                "{path}"
            );
            for header in HEADERS {
                assert!(wire.contains(header), "{path}: missing {header}");
            }
        }
        // Every font the stylesheet names is served, and nothing else is fetched.
        let css = include_str!("viewer.css");
        let urls: Vec<&str> = css
            .split("url(\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        assert_eq!(urls.len(), 10);
        for url in urls {
            let name = url
                .strip_prefix("/fonts/")
                .unwrap()
                .strip_suffix(".woff2")
                .unwrap();
            assert!(FONTS.iter().any(|(font, _)| *font == name), "{url}");
        }
        for (_, bytes) in FONTS {
            assert_eq!(&bytes[..4], b"wOF2");
        }
    }

    #[test]
    fn harness_icon_routes_serve_exact_bytes_and_unknown_paths_404() {
        let fixture = lane_fixture();
        for (path, svg) in crate::HARNESS_ICONS {
            let wire = get(&fixture, path);
            assert!(wire.starts_with("HTTP/1.1 200"), "{path}");
            assert!(
                wire.contains("Content-Type: image/svg+xml"),
                "{path}: wrong content type"
            );
            let body = wire.split_once("\r\n\r\n").unwrap().1;
            assert_eq!(body.as_bytes(), svg.as_bytes(), "{path}: body changed");
            for header in HEADERS {
                assert!(wire.contains(header), "{path}: missing {header}");
            }
        }
        for path in ["/harness/nope.svg", "/harness/../viewer.js"] {
            let wire = get(&fixture, path);
            assert!(wire.starts_with("HTTP/1.1 404"), "{path}");
            for header in HEADERS {
                assert!(wire.contains(header), "{path}: missing {header}");
            }
        }
    }

    #[test]
    fn shell_font_api_matches_the_font_routes() {
        let fixture = lane_fixture();
        let viewer = fixture.viewer();
        for name in crate::shell::FONT_FILES {
            let library_bytes = crate::shell::font(name).expect(name);
            let (status, content_type, route_bytes) = viewer
                .route(&format!("/fonts/{name}"), "")
                .expect("font route");
            assert_eq!(status, 200, "{name}");
            assert_eq!(content_type, "font/woff2", "{name}");
            assert_eq!(route_bytes.as_slice(), library_bytes, "{name}");
        }
        assert!(crate::shell::font("unknown.woff2").is_none());
        assert!(crate::shell::font("instrument-sans-latin").is_none());
    }

    fn application_script(js: &str) -> String {
        // Only the pinned Preact core may contain its generic HTML/style implementation.
        // Application sources are AST-checked in ui/security-check.mjs; the VNode guard
        // rejects these props at runtime. Keep all other bundle checks and CSP intact.
        let (before, runtime_and_after) = js
            .split_once("  // node_modules/preact/dist/preact.module.js\n")
            .expect("pinned Preact core boundary");
        let (runtime, after) = runtime_and_after
            .split_once("\n  // ")
            .expect("end of pinned Preact core module");
        assert_eq!(runtime.matches("innerHTML").count(), 3);
        assert_eq!(runtime.matches("cssText").count(), 2);
        format!("{before}\n  // {after}")
    }

    #[test]
    fn runtime_sink_exemption_stops_before_application_modules() {
        let js = crate::shell::VIEWER_JS;
        let app = application_script(js);
        for module in [
            "src/lib/account.ts",
            "src/lib/security.ts",
            "src/lib/AccountMenu.tsx",
            "src/lib/account-chrome.tsx",
            "src/lib/shell.tsx",
            "src/lib/recent.tsx",
            "src/lib/panel.tsx",
            "src/lib/facets.tsx",
            "src/lib/screens.tsx",
            "src/lib/richtext.tsx",
            "src/lib/tool-details.tsx",
            "src/lib/tool-step.tsx",
            "src/lib/analytics.tsx",
            "src/lib/layout.ts",
            "src/lib/sentence.tsx",
            "src/lib/transcript.tsx",
            "src/lib/geometry.ts",
            "src/lib/select.tsx",
            "src/lib/tooltip.tsx",
            "src/lib/api.ts",
            "src/app/catalogViewer.tsx",
            "src/app/overviewPages.tsx",
            "src/app/runtimeObservation.ts",
            "src/state/catalog-selection.ts",
        ] {
            assert!(
                app.contains(&format!("  // {module}\n")),
                "{module} must be scanned"
            );
        }
        assert!(
            !js.contains("  // src/app/controlObservation.ts\n"),
            "paused control observation must not enter the collection bundle"
        );
        let injected = js.replacen(
            "  // src/lib/account.ts\n",
            "  // src/lib/account.ts\n  probe.innerHTML = hostile;\n",
            1,
        );
        assert!(application_script(&injected).contains("innerHTML"));
    }

    #[test]
    fn the_page_has_no_inline_script_style_or_html_injection() {
        let lower = PAGE.to_ascii_lowercase();
        assert_eq!(lower.matches("<script").count(), 1);
        assert!(lower.contains("<script src=\"/viewer.js\" defer></script>"));
        assert!(!lower.contains("<style"));
        assert!(!lower.contains(" style="));
        assert!(!lower.contains("javascript:"));
        let handler = lower
            .split(|c: char| c.is_whitespace())
            .any(|word| word.starts_with("on") && word.contains('='));
        assert!(!handler, "no inline event handlers");
        let js = crate::shell::VIEWER_JS;
        let application = application_script(js);
        for banned in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
            "setAttribute(\"style\"",
            "cssText",
        ] {
            assert!(
                !application.contains(banned),
                "application viewer.js uses {banned}"
            );
            if banned != "innerHTML" && banned != "cssText" {
                assert!(!js.contains(banned), "viewer.js uses {banned}");
            }
        }
        assert!(js.contains("textContent"));
        // No native tooltips: a tip is `data-tip` (tooltip.js). The browser check scans the screens it renders; this covers
        // the ones it does not open (menus, the errors stepper, listboxes).
        for (name, script) in [("viewer.js", js), ("shell.js", crate::shell::JS)] {
            // The document title names a history destination; element titles create forbidden native tooltips.
            let script = script.replace("document.title =", "documentTitle =");
            for banned in [
                ".title =",
                ".title=",
                "setAttribute(\"title\"",
                "setAttribute('title'",
                "svgEl(\"title\"",
                "createElementNS(SVGNS, \"title\"",
            ] {
                assert!(
                    !script.contains(banned),
                    "{name} sets a native title: {banned}"
                );
            }
            // DOM attributes are checked by ui/security-check.mjs on the TypeScript AST.
            // Ordinary model/view-model `title` fields are data, not native tooltip attributes.
        }
        for banned in ["@import", "http://", "https://"] {
            assert!(
                !crate::shell::VIEWER_CSS.contains(banned),
                "viewer.css uses {banned}"
            );
        }
    }

    #[test]
    fn native_transcript_content_is_served_as_data() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        publish_native(&view);
        let wire = get(&fixture, "/api/session-transcript?sid=lane&limit=100");
        assert!(wire.starts_with("HTTP/1.1 200"));
        let page: Value = serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap();
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries[0]["k"], "h");
        assert_eq!(entries[1]["text"], "<img src=x onerror=alert(2)>");
        assert!(
            serde_json::to_string(&entries[0])
                .unwrap()
                .contains("<script>alert(1)</script>")
        );
        for query in [
            "/api/session-transcript",
            "/api/session-transcript?sid=lane&after=x",
            "/api/session-transcript?sid=lane&limit=0",
        ] {
            assert!(get(&fixture, query).starts_with("HTTP/1.1 400"), "{query}");
        }
        assert!(get(&fixture, "/api/session-transcript?sid=nobody").starts_with("HTTP/1.1 404"));
        assert_retired_routes_absent(&view);
    }

    // ---- Attachments: images a prompt attaches, named by /api/tx and served by /api/attachment ----

    /// A PNG, JPEG, GIF and WebP (VP8X) as far as their headers: 1×1, 3×2
    /// (its frame header after an APP0 segment), 2×3 and 4×5.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0";
    const JPEG: &[u8] =
        b"\xFF\xD8\xFF\xE0\0\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0\xFF\xC0\0\x11\x08\0\x02\0\x03\x03";
    const GIF: &[u8] = b"GIF89a\x02\0\x03\0\0\0\0";
    const WEBP: &[u8] = b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\x10\0\0\0\x03\0\0\x04\0\0";
    const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";

    fn b64(bytes: &[u8]) -> String {
        crate::attachments::tests::encode(bytes)
    }

    /// The `v` a URL names for base64 `data`.
    fn v(data: &str) -> String {
        crate::attachments::tests::version_of(data)
    }

    fn image_block(media: &str, data: String) -> Value {
        json!({"type":"image","source":{"type":"base64","media_type":media,"data":data}})
    }

    /// Each line's byte offset in a file the fixture wrote.
    fn offsets(path: &Path) -> Vec<u64> {
        let text = fs::read(path).unwrap();
        let mut out = vec![0];
        for (at, byte) in text.iter().enumerate() {
            if *byte == b'\n' && at + 1 < text.len() {
                out.push(at as u64 + 1);
            }
        }
        out
    }

    /// `pics`: your message with an image and its `[Image #1]` placeholder,
    /// a reply, a queued command with a JPEG, a message that is only an
    /// image, a message whose images can't be served, a tool result with an
    /// image beside it (not a prompt), and a message with a GIF and a WebP.
    /// `other`: a second lane.
    fn images_fixture() -> (Fixture, Vec<u64>) {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        let user = |second: u32, content: Value| {
            json!({"type":"user","timestamp":format!("2026-09-24T00:00:{second:02}Z"),"sessionId":"pics",
                "origin":{"kind":"human"},"message":{"role":"user","content":content}})
        };
        let path = fixture.claude(
            "pics",
            &[
                user(0, json!([image_block("image/png", b64(PNG)), {"type":"text","text":"[Image #1] The top bar is bloated"}])),
                json!({"type":"assistant","timestamp":"2026-09-24T00:00:01Z","sessionId":"pics",
                    "message":{"role":"assistant","content":[{"type":"text","text":"Looking."}]}}),
                json!({"type":"attachment","timestamp":"2026-09-24T00:00:02Z","sessionId":"pics",
                    "attachment":{"type":"queued_command","origin":{"kind":"human"},
                        "prompt":[image_block("image/jpeg", b64(JPEG)), {"type":"text","text":"and this one"}]}}),
                user(3, json!([image_block("image/png", b64(PNG))])),
                user(4, json!([
                    image_block("image/svg+xml", b64(SVG)),
                    image_block("image/png", b64(SVG)),
                    image_block("text/html", b64(PNG)),
                    image_block("image/png", format!("{}*AAA", &b64(PNG)[..8])),
                    {"type":"image","source":{"type":"url","url":"https://example.com/a.png"}},
                    {"type":"text","text":"bad ones"},
                ])),
                json!({"type":"assistant","timestamp":"2026-09-24T00:00:05Z","sessionId":"pics",
                    "message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/tmp/a.png"}}]}}),
                json!({"type":"user","timestamp":"2026-09-24T00:00:06Z","sessionId":"pics",
                    "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"read"}, image_block("image/png", b64(PNG))]}}),
                user(7, json!([image_block("image/gif", b64(GIF)), image_block("image/webp", b64(WEBP)), {"type":"text","text":"a gif and a webp"}])),
            ],
        );
        fixture.claude(
            "other",
            &[json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"other","origin":{"kind":"human"},
                "message":{"role":"user","content":"hello"}})],
        );
        let lines = offsets(&path);
        (fixture, lines)
    }

    fn publish_native(view: &MachineView) -> Value {
        let mut work = lock(&view.evidence.work);
        while !view
            .refresh_catalog_locked(&mut work, model::now_ms())
            .unwrap()
        {}
        drop(work);
        let reply = crate::session_catalog_page(view.options(), "fixture", "limit=100");
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        body_of(&reply)
    }

    fn native_selected(view: &MachineView, sid: &str) -> Value {
        let reply = crate::session_transcript_range(
            view.options(),
            "fixture",
            &format!("sid={sid}&after=0&limit=100"),
            None,
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        body_of(&reply)
    }

    fn assert_retired_routes_absent(view: &MachineView) {
        for route in [
            "/api/model",
            "/api/tx",
            "/api/transcript",
            "/api/entry",
            "/api/analytics",
        ] {
            assert_eq!(
                view.respond("GET", route, "sid=lane", None).status,
                404,
                "{route}"
            );
        }
    }

    fn native_image_entries(core: &MachineView, sid: &str) -> (String, Vec<Value>) {
        model::observe_catalog(
            core.options(),
            &mut EventCache::open_scoped(&core.options().cache).unwrap(),
            &mut false,
            &mut model::Texts::default(),
            model::now_ms(),
        )
        .unwrap();
        let reply = crate::session_transcript_range(
            core.options(),
            "fixture",
            &format!("sid={sid}&after=0&limit=100"),
            None,
        );
        assert_eq!(reply.status, 200);
        let body = String::from_utf8(reply.body).unwrap();
        let page: Value = serde_json::from_str(&body).unwrap();
        assert!(!page["range"]["has_more"].as_bool().unwrap_or(false));
        (body, page["entries"].as_array().unwrap().clone())
    }

    /// A servable image's native transcript reference.
    fn served_ref(o: u64, b: usize, media: &str, bytes: &[u8], size: (u32, u32)) -> Value {
        json!({"b":b,"type":media,"size":bytes.len(),"v":v(&b64(bytes)),"w":size.0,"h":size.1,"o":o})
    }

    #[test]
    fn a_prompts_images_are_named_by_reference_never_inlined() {
        let (fixture, at) = images_fixture();
        let core = fixture.viewer();
        let (body, entries) = native_image_entries(&core, "pics");
        // No bytes, no placeholder, no "[image]" text in the page.
        for bytes in [PNG, JPEG, GIF, WEBP] {
            assert!(!body.contains(&b64(bytes)[..12]), "{body}");
        }
        assert!(
            !body.contains("[Image #") && !body.contains("[image]"),
            "{body}"
        );
        let images: Vec<&Value> = entries
            .iter()
            .filter_map(|entry| entry.get("img"))
            .collect();
        assert_eq!(
            images,
            [
                &json!([served_ref(at[0], 0, "image/png", PNG, (1, 1))]),
                &json!([served_ref(at[2], 0, "image/jpeg", JPEG, (3, 2))]),
                &json!([served_ref(at[3], 0, "image/png", PNG, (1, 1))]),
                &json!(
                    (0..5)
                        .map(|b| json!({"b":b,"na":true,"o":at[4]}))
                        .collect::<Vec<_>>()
                ),
                &json!([
                    served_ref(at[7], 0, "image/gif", GIF, (2, 3)),
                    served_ref(at[7], 1, "image/webp", WEBP, (4, 5))
                ]),
            ],
            "{entries:?}"
        );
        // Image prompts retain their shared native handoff text.
        let mut query = crate::Query::new(fixture.options.clone());
        let session = query.call("get_session", &json!({"id":"pics"})).unwrap();
        let briefs: Vec<&str> = session["session"]["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|handoff| handoff["kind"] == "ask" && handoff["to"] == "pics")
            .map(|handoff| handoff["brief"].as_str().unwrap())
            .collect();
        assert_eq!(
            briefs,
            [
                "The top bar is bloated",
                "and this one",
                "",
                "bad ones",
                "a gif and a webp"
            ]
        );
        for entry in entries.iter().filter(|entry| entry.get("img").is_some()) {
            assert_eq!(entry["k"], "h", "{entry}");
        }
    }

    fn native_image_query(core: &MachineView, sid: &str, offset: u64, block: usize) -> String {
        let (body, entries) = native_image_entries(core, sid);
        let page: Value = serde_json::from_str(&body).unwrap();
        let (entry, ordinal) = entries
            .iter()
            .find_map(|entry| {
                entry["img"]
                    .as_array()?
                    .iter()
                    .enumerate()
                    .find_map(|(ordinal, image)| {
                        (image["o"] == offset && image["b"] == block).then_some((entry, ordinal))
                    })
            })
            .expect("the selected native image reference");
        format!(
            "machine=local&sid={sid}&after={}&limit=1&generation={}&part=attachment&image={ordinal}",
            entry["slot"],
            page["projection"]["generation"].as_str().unwrap()
        )
    }

    #[test]
    fn an_attachment_is_served_only_as_a_generation_fenced_native_image() {
        let (fixture, at) = images_fixture();
        let core = fixture.viewer();
        let fetch =
            |query: &str| crate::session_entry_field(&fixture.options, "local", query, None);
        for (offset, block, media, bytes) in [
            (at[0], 0, "image/png", PNG),
            (at[2], 0, "image/jpeg", JPEG),
            (at[3], 0, "image/png", PNG),
            (at[7], 0, "image/gif", GIF),
            (at[7], 1, "image/webp", WEBP),
        ] {
            let query = native_image_query(&core, "pics", offset, block);
            let reply = fetch(&query);
            assert_eq!(
                (reply.status, reply.content_type, reply.body.as_slice()),
                (200, media, bytes),
                "{media}"
            );
            assert_eq!(reply.headers(), ATTACHMENT_HEADERS.as_slice());
        }
        // SVG, SVG declared as PNG, non-image types, malformed base64 and remote URLs remain unavailable.
        for block in 0..5 {
            let query = native_image_query(&core, "pics", at[4], block);
            let reply = fetch(&query);
            assert_eq!(reply.status, 404, "{query}");
            assert_eq!(reply.headers(), SECURITY_HEADERS.as_slice());
        }
        let good = native_image_query(&core, "pics", at[0], 0);
        assert_ne!(fetch(&good.replace("sid=pics", "sid=other")).status, 200);
        assert_eq!(fetch(&good.replace("sid=pics", "sid=nobody")).status, 404);
        assert_eq!(fetch(&good.replace("image=0", "image=1")).status, 404);
        // Non-prompt entries, including a tool result beside an image, cannot expose image bytes.
        let (body, entries) = native_image_entries(&core, "pics");
        let page: Value = serde_json::from_str(&body).unwrap();
        let generation = page["projection"]["generation"].as_str().unwrap();
        for entry in entries.iter().filter(|entry| entry.get("img").is_none()) {
            let query = format!(
                "sid=pics&after={}&limit=1&generation={generation}&part=attachment&image=0",
                entry["slot"]
            );
            assert_eq!(fetch(&query).status, 404, "{entry}");
        }
        for query in [
            good.replace("sid=pics&", ""),
            good.replace("after=0&", ""),
            good.replace("limit=1", "limit=2"),
            good.replace(&format!("generation={generation}&"), ""),
            good.replace("after=0", "after=x"),
            good.replace("after=0", "after=-1"),
            good.replace("after=0", "after=99999999999999999999999"),
            good.replace("image=0", "image=64"),
            good.replace("image=0", "image=x"),
        ] {
            assert_eq!(fetch(&query).status, 400, "{query}");
        }
        assert_eq!(
            fetch(&good.replace(generation, &"0".repeat(64))).status,
            409
        );
        let viewer = ViewerCore::new(fixture.options.clone());
        assert_eq!(
            viewer
                .respond("POST", "/api/session-entry", &good, None)
                .status,
            405
        );
        viewer.close();
        let wire = get(&fixture, &format!("/api/session-entry?{good}"));
        assert!(wire.starts_with("HTTP/1.1 200"), "{wire}");
        for header in [
            "Content-Type: image/png",
            "X-Content-Type-Options: nosniff",
            "Cache-Control: private, max-age=86400",
            "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox",
            "X-Frame-Options: DENY",
            "Referrer-Policy: no-referrer",
        ] {
            assert!(wire.contains(header), "missing {header}");
        }
        assert!(!wire.contains("no-store"));
        let missing_query = native_image_query(&core, "pics", at[4], 0);
        let missing = get(&fixture, &format!("/api/session-entry?{missing_query}"));
        assert!(missing.starts_with("HTTP/1.1 404"));
        for header in HEADERS {
            assert!(missing.contains(header), "missing {header}");
        }
        assert!(
            http(
                &fixture,
                &format!("GET /api/session-entry?{good} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n")
            )
            .starts_with("HTTP/1.1 403")
        );
    }

    /// A line whose image changed under the same offset (a file replaced,
    /// or rewritten by redaction) is a new URL: the old one is refused.
    #[test]
    fn a_rewritten_line_is_a_new_url() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        let line = |bytes: &[u8]| {
            json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"swap","origin":{"kind":"human"},
                "message":{"role":"user","content":[image_block("image/gif", b64(bytes))]}})
        };
        let other: &[u8] = b"GIF89a\x07\0\x03\0\0\0\0";
        fixture.claude("swap", &[line(GIF)]);
        let core = fixture.viewer();
        let old = native_image_query(&core, "swap", 0, 0);
        assert_eq!(
            crate::session_entry_field(&fixture.options, "local", &old, None).status,
            200
        );
        let path = fixture.claude("swap", &[line(other)]);
        assert_eq!(offsets(&path), [0]);
        let (_, entries) = native_image_entries(&core, "swap");
        let image = &entries.iter().find_map(|entry| entry.get("img")).unwrap()[0];
        assert_eq!(image["v"], v(&b64(other)));
        assert_eq!(image["w"], 7);
        assert_eq!(
            crate::session_entry_field(&fixture.options, "local", &old, None).status,
            409
        );
        let new = native_image_query(&core, "swap", 0, 0);
        assert_eq!(
            crate::session_entry_field(&fixture.options, "local", &new, None).body,
            other
        );
    }

    /// At the cap, an image is served; a byte over, it isn't. Both lines are
    /// longer than a page reads of one line, and are still named.
    #[test]
    fn an_image_at_the_cap_is_served_and_one_over_it_is_not() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        let cap = crate::attachments::IMAGE_MAX;
        let mut exact = PNG.to_vec();
        exact.resize(cap, 0);
        let mut over = exact.clone();
        over.push(0);
        let (exact, over) = (b64(&exact), b64(&over));
        let line = |second: u32, data: &str| {
            json!({"type":"user","timestamp":format!("2026-09-24T00:00:{second:02}Z"),"sessionId":"big","origin":{"kind":"human"},
                "message":{"role":"user","content":[image_block("image/png", data.to_owned()), {"type":"text","text":"large"}]}})
        };
        let path = fixture.claude("big", &[line(0, &exact), line(1, &over)]);
        let at = offsets(&path);
        assert!(at[1] > tx::LINE_MAX);
        let core = fixture.viewer();
        let (_, entries) = native_image_entries(&core, "big");
        let images: Vec<&Value> = entries
            .iter()
            .filter_map(|entry| entry.get("img"))
            .collect();
        assert_eq!(
            images,
            [
                &json!([{"b":0,"type":"image/png","size":cap,"v":v(&exact),"w":1,"h":1,"o":0}]),
                &json!([{"b":0,"na":true,"o":at[1]}]),
            ]
        );
        let query = native_image_query(&core, "big", 0, 0);
        let served = crate::session_entry_field(&fixture.options, "local", &query, None);
        assert_eq!((served.status, served.body.len()), (200, cap));
        let refused = native_image_query(&core, "big", at[1], 0);
        assert_eq!(
            crate::session_entry_field(&fixture.options, "local", &refused, None).status,
            404
        );
    }

    #[test]
    fn a_codex_prompts_images_are_its_data_urls() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        let path = fixture.codex(
            "cdx",
            &[json!({"type":"response_item","timestamp":"2026-09-24T00:00:01Z","payload":{"type":"message","role":"user","content":[
                {"type":"input_text","text":"<image name=[Image #1]>"},
                {"type":"input_image","image_url":format!("data:image/png;base64,{}", b64(PNG))},
                {"type":"input_text","text":"</image>"},
                {"type":"input_image","image_url":"/home/me/shot.png"},
                {"type":"input_text","text":"why is this red"},
            ]}})],
        );
        let at = offsets(&path);
        let core = fixture.viewer();
        let (body, entries) = native_image_entries(&core, "cdx");
        assert!(
            !body.contains("<image") && !body.contains("[Image #"),
            "{body}"
        );
        let prompt = entries.iter().find(|entry| entry["k"] == "u").unwrap();
        assert_eq!(prompt["text"], "why is this red");
        assert_eq!(
            prompt["img"],
            json!([served_ref(at[1], 1, "image/png", PNG, (1, 1)), {"b":3,"na":true,"o":at[1]}])
        );
        let query = native_image_query(&core, "cdx", at[1], 1);
        let reply = crate::session_entry_field(&fixture.options, "local", &query, None);
        assert_eq!(
            (reply.status, reply.content_type, reply.body.as_slice()),
            (200, "image/png", PNG)
        );
        let local = native_image_query(&core, "cdx", at[1], 3);
        assert_eq!(
            crate::session_entry_field(&fixture.options, "local", &local, None).status,
            404
        );
    }

    /// The model's per-session `calls` and `errors` are the totals `/api/tx`
    /// reports, so a page needn't fetch a transcript to count.
    #[test]
    fn native_selected_counts_match_tools_and_error_annotations() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        fixture.claude(
            "counted",
            &[
                json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"counted","origin":{"kind":"human"},
                    "message":{"role":"user","content":"run two"}}),
                json!({"type":"assistant","timestamp":"2026-09-24T00:01:00Z","sessionId":"counted",
                    "message":{"role":"assistant","content":[
                        {"type":"tool_use","id":"good","name":"Bash","input":{"command":"true"}},
                        {"type":"tool_use","id":"bad","name":"Bash","input":{"command":"false"}}]}}),
                json!({"type":"user","timestamp":"2026-09-24T00:02:00Z","sessionId":"counted","toolUseResult":{},
                    "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"good","content":"ok","is_error":false}]}}),
                json!({"type":"user","timestamp":"2026-09-24T00:03:00Z","sessionId":"counted","toolUseResult":{},
                    "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bad","content":"exit 1","is_error":true}]}}),
            ],
        );
        let view = fixture.viewer();
        publish_native(&view);
        let page = native_selected(&view, "counted");
        let details = &page["session"]["details"];
        assert_eq!(details["calls"], 2);
        assert_eq!(details["errors"], 1);
        assert_eq!(
            page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["k"] == "tool")
                .count(),
            2
        );
        assert_eq!(
            page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["k"] == "tool" && entry["ok"] == false)
                .count(),
            1
        );
        let selected = body_of(&crate::session_catalog_page(
            &fixture.options,
            "fixture",
            "sid=counted",
        ));
        assert_eq!(selected["items"][0]["details"], *details);
        let annotations = crate::session_annotations(
            &fixture.options,
            "fixture",
            "sid=counted&kind=errors&limit=100&scope=retained_history",
        );
        assert_eq!(annotations.status, 200);
        assert_eq!(body_of(&annotations)["total"], details["errors"]);
    }

    /// A session whose failed steps span three pages: 450 calls in three
    /// turns, failing at calls 3, 210 (never answered) and 449, and 17 and
    /// 300 as well (`is_error`).
    fn faults_fixture() -> Fixture {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        let at = |second: usize| {
            format!(
                "2026-09-24T{:02}:{:02}:{:02}Z",
                second / 3600,
                second / 60 % 60,
                second % 60
            )
        };
        let mut records = Vec::new();
        for call in 0..450 {
            if call % 150 == 0 {
                records.push(json!({"type":"user","timestamp":at(call * 4),"sessionId":"faults","origin":{"kind":"human"},
                    "message":{"role":"user","content":format!("batch {}", call / 150)}}));
            }
            let id = format!("toolu-{call}");
            records.push(json!({"type":"assistant","timestamp":at(call * 4 + 1),"sessionId":"faults",
                "message":{"role":"assistant","content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":format!("step {call}")}}]}}));
            if call == 210 {
                continue;
            }
            let failed = [3, 17, 300, 449].contains(&call);
            records.push(json!({"type":"user","timestamp":at(call * 4 + 2),"sessionId":"faults","toolUseResult":{},
                "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"out","is_error":failed}]}}));
        }
        fixture.claude("faults", &records);
        fixture
    }

    /// `/api/tx?errors=1` lists exactly the steps the errors badge counts,
    /// each at a slot whose page (`after=` it, or `before=` one past it)
    /// holds that failed step, in transcript order across pages.
    #[test]
    fn signals_have_a_versioned_positional_index_without_a_page_anchor() {
        let fixture = Fixture::new();
        fixture.codex("signals", &[
            json!({"timestamp":"2026-09-24T06:00:00Z","type":"session_meta","payload":{"id":"signals","cwd":"/work/proj"}}),
            json!({"timestamp":"2026-09-24T06:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Work"}}),
            json!({"timestamp":"2026-09-24T06:00:02Z","type":"event_msg","payload":{"type":"context_compacted"}}),
        ]);
        let view = fixture.viewer();
        publish_native(&view);
        let read = |query: &str| crate::session_annotations(&fixture.options, "fixture", query);
        let reply = read("sid=signals&kind=signals&limit=100&scope=retained_history");
        assert_eq!(reply.status, 200);
        let body = body_of(&reply);
        assert_eq!(body["total"], 1);
        assert_eq!(body["entries"].as_array().unwrap().len(), 1);
        let generation = body["generation"].as_str().unwrap();
        let cached = read(&format!(
            "sid=signals&kind=signals&limit=100&generation={generation}&scope=retained_history"
        ));
        assert_eq!(cached.status, 200);
        assert_eq!(body_of(&cached), body);
        for query in [
            "sid=signals&kind=unknown&scope=retained_history",
            "sid=signals&kind=signals&limit=0&scope=retained_history",
            "sid=signals&kind=signals&after=1&scope=retained_history",
        ] {
            assert_eq!(read(query).status, 400);
        }
    }

    #[test]
    fn failed_steps_are_listed_where_the_badge_counts_them() {
        let fixture = faults_fixture();
        let view = fixture.viewer();
        publish_native(&view);
        let reply = crate::session_annotations(
            &fixture.options,
            "fixture",
            "sid=faults&kind=errors&limit=100&scope=retained_history",
        );
        assert_eq!(reply.status, 200);
        let annotations = body_of(&reply);
        assert_eq!(annotations["total"], 5);
        assert_eq!(
            annotations["total"],
            native_selected(&view, "faults")["session"]["details"]["errors"]
        );
        assert!(annotations["next_cursor"].is_null());
        let slots: Vec<_> = annotations["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["slot"].as_u64().unwrap())
            .collect();
        let mut failed = Vec::new();
        let mut after = 0;
        let mut generation = None;
        loop {
            let mut query = format!("sid=faults&after={after}&limit=100");
            if let Some(ref value) = generation {
                query.push_str(&format!("&generation={value}"));
            }
            let reply = crate::session_transcript_range(&fixture.options, "fixture", &query, None);
            assert_eq!(reply.status, 200);
            let page = body_of(&reply);
            generation = Some(
                page["projection"]["generation"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            failed.extend(
                page["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|entry| entry["k"] == "tool" && entry["ok"] == false)
                    .map(|entry| entry["slot"].as_u64().unwrap()),
            );
            let Some(next) = page["range"]["next"].as_u64() else {
                break;
            };
            after = next;
        }
        assert_eq!(slots, failed);
        assert_eq!(slots.len(), 5);
        assert!(
            slots.last().unwrap() - slots.first().unwrap() > 400,
            "failures span pages"
        );
        for (index, slot) in slots.into_iter().enumerate() {
            let reply = crate::session_transcript_range(
                &fixture.options,
                "fixture",
                &format!("sid=faults&after={slot}&limit=1"),
                None,
            );
            assert_eq!(reply.status, 200);
            let page = body_of(&reply);
            assert_eq!(page["entries"][0]["slot"], slot);
            assert_eq!(page["entries"][0]["ok"], false);
            if index == 2 {
                assert_eq!(page["entries"][0]["unfinished"], true);
            }
        }
    }

    #[test]
    fn the_error_list_refuses_bad_requests_and_unknown_sessions() {
        let fixture = faults_fixture();
        let view = fixture.viewer();
        publish_native(&view);
        for query in [
            "kind=errors&scope=retained_history",
            "sid=faults&kind=",
            "sid=faults&kind=unknown&scope=retained_history",
            "sid=faults&kind=errors&after=3&scope=retained_history",
            "sid=%zz&kind=errors&scope=retained_history",
            "sid=faults&kind=errors&generation=%zz&scope=retained_history",
            "sid=faults&kind=errors&limit=101&scope=retained_history",
        ] {
            let reply = crate::session_annotations(&fixture.options, "fixture", query);
            assert_eq!((reply.status, reply.etag), (400, None), "{query}");
        }
        let first = crate::session_annotations(
            &fixture.options,
            "fixture",
            "sid=faults&kind=errors&scope=retained_history",
        );
        assert_eq!(first.status, 200);
        let generation = body_of(&first)["generation"].as_str().unwrap().to_owned();
        let missing = crate::session_annotations(
            &fixture.options,
            "fixture",
            &format!("sid=nobody&kind=errors&generation={generation}&scope=retained_history"),
        );
        assert_eq!(missing.status, 404);
    }

    /// The list carries the model's `ETag`: 304 while it holds (by
    /// `If-None-Match` or `since=`), a new list once a failure lands.
    #[test]
    fn native_error_annotation_generation_changes_only_with_selected_evidence() {
        let fixture = faults_fixture();
        let view = fixture.viewer();
        publish_native(&view);
        let read = || {
            let reply = crate::session_annotations(
                &fixture.options,
                "fixture",
                "sid=faults&kind=errors&limit=100&scope=retained_history",
            );
            assert_eq!(reply.status, 200);
            body_of(&reply)
        };
        let first = read();
        let generation = first["generation"].as_str().unwrap().to_owned();
        assert_eq!(
            read(),
            first,
            "unchanged qualified error evidence reuses its generation"
        );
        let path = fixture.root.join("claude/projects/project/faults.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", json!({"type":"assistant","timestamp":"2026-09-24T02:00:00Z","sessionId":"faults",
            "message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu-late","name":"Bash","input":{"command":"late"}}]}})).unwrap();
        writeln!(file, "{}", json!({"type":"user","timestamp":"2026-09-24T02:00:01Z","sessionId":"faults","toolUseResult":{},
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu-late","content":"no","is_error":true}]}})).unwrap();
        drop(file);
        publish_native(&view);
        let moved = read();
        assert_ne!(moved["generation"], generation);
        assert_eq!(moved["total"], 6);
        let rows = moved["entries"].as_array().unwrap();
        assert_eq!(rows.len(), 6);
        let total = native_selected(&view, "faults")["projection"]["total"]
            .as_u64()
            .unwrap();
        assert_eq!(rows[5]["slot"].as_u64().unwrap() + 1, total);
        assert_eq!(
            native_selected(&view, "faults")["session"]["details"]["errors"],
            6
        );
        let stale = crate::session_annotations(
            &fixture.options,
            "fixture",
            &format!(
                "sid=faults&kind=errors&limit=100&generation={generation}&scope=retained_history"
            ),
        );
        assert_eq!(stale.status, 409);
    }

    /// The list stops at its cap and says so; the count stays whole.
    #[test]
    fn native_error_annotations_page_with_whole_counts_and_fenced_continuation() {
        let fixture = faults_fixture();
        let view = fixture.viewer();
        publish_native(&view);
        let read = |query: &str| {
            let reply = crate::session_annotations(&fixture.options, "fixture", query);
            assert_eq!(reply.status, 200);
            body_of(&reply)
        };
        let whole = read("sid=faults&kind=errors&limit=100&scope=retained_history");
        let capped = read("sid=faults&kind=errors&limit=2&scope=retained_history");
        assert_eq!(capped["total"], 5);
        assert_eq!(
            capped["entries"].as_array().unwrap(),
            &whole["entries"].as_array().unwrap()[..2]
        );
        assert_eq!(capped["next_cursor"], 2);
        let tail = read(&format!(
            "sid=faults&kind=errors&limit=100&after=2&generation={}&scope=retained_history",
            capped["generation"].as_str().unwrap()
        ));
        assert_eq!(tail["total"], 5);
        assert_eq!(
            tail["entries"].as_array().unwrap(),
            &whole["entries"].as_array().unwrap()[2..]
        );
        assert!(tail["next_cursor"].is_null());
        let exact = read("sid=faults&kind=errors&limit=5&scope=retained_history");
        assert_eq!(exact["entries"], whole["entries"]);
        assert!(exact["next_cursor"].is_null());
    }

    /// The core answers without a transport: the same bodies, statuses and
    /// `ETag` the loopback server sends, GET only, and the headers it sends
    /// are the ones a server embedding the core is given.
    #[test]
    fn native_capabilities_advertise_source_selection_without_preparing_evidence() {
        let fixture = lane_fixture();
        for count in [1, 2] {
            let core = ViewerCore::with_machines(
                (0..count)
                    .map(|index| (format!("source-{index}"), fixture.options.clone()))
                    .collect(),
            );
            let reply = core.respond("GET", "/api/session-capabilities", "machine=source-0", None);
            assert_eq!(reply.status, 200);
            assert_eq!(body_of(&reply)["source_candidates"], count > 1);
            for view in core.machine_views() {
                assert_eq!(view.hooks.builds(), 0);
                assert!(lock(&view.evidence.work).events.is_none());
                assert!(!view.evidence.catalog_demand.load(Ordering::Relaxed));
            }
            core.close();
        }
    }

    #[test]
    fn the_core_answers_without_a_transport() {
        let fixture = lane_fixture();
        let core = ViewerCore::new(fixture.options.clone());
        publish_native(&core.machine_views()[0]);
        let page = core.respond("GET", "/analytics", "", None);
        assert_eq!(
            (
                page.status,
                page.content_type,
                page.body.as_slice(),
                page.etag
            ),
            (200, "text/html; charset=utf-8", PAGE.as_bytes(), None)
        );
        let script = core.respond("GET", "/viewer.js", "", None);
        assert_eq!(script.body, crate::shell::VIEWER_JS.as_bytes());
        let catalog = core.respond("GET", "/api/sessions", "sid=lane", None);
        assert_eq!(catalog.status, 200);
        assert!(catalog.etag.is_none());
        let body = body_of(&catalog);
        let descriptor = &body["items"][0]["transcript"];
        let generation = descriptor["generation"].as_str().unwrap();
        let transcript = core.respond(
            "GET",
            "/api/session-transcript",
            &format!("sid=lane&generation={generation}&after=0&limit=100"),
            None,
        );
        assert_eq!(transcript.status, 200);
        assert_eq!(
            body_of(&transcript)["projection"]["total"],
            descriptor["total"]
        );
        assert_eq!(
            core.respond("GET", "/api/session-transcript", "sid=lane&after=x", None)
                .status,
            400
        );
        assert_eq!(
            core.respond(
                "GET",
                "/api/session-transcript",
                "sid=lane&generation=stale",
                None
            )
            .status,
            409
        );
        for retired in [
            "/api/model",
            "/api/tx",
            "/api/transcript",
            "/api/entry",
            "/api/analytics",
        ] {
            assert_eq!(core.respond("GET", retired, "sid=lane", None).status, 404);
        }
        assert_eq!(core.respond("GET", "/elsewhere", "", None).status, 404);
        for method in ["POST", "HEAD", "PUT", "DELETE"] {
            let refused = core.respond(method, "/", "", None);
            assert_eq!(refused.status, 405, "{method}");
        }
        // The loopback server sends exactly these, and the same body.
        let wire = get(&fixture, "/analytics");
        assert!(wire.ends_with(PAGE));
        for (name, value) in SECURITY_HEADERS {
            assert!(wire.contains(&format!("{name}: {value}")), "{name}");
        }
        assert_eq!(HEADERS.len(), SECURITY_HEADERS.len());
        for (header, (name, value)) in HEADERS.iter().zip(SECURITY_HEADERS) {
            assert_eq!(*header, format!("{name}: {value}"));
        }
    }

    #[test]
    fn public_shell_exports_match_viewer_core_routes() {
        let fixture = lane_fixture();
        let core = crate::ViewerCore::new(fixture.options.clone());

        let script = core.respond("GET", "/viewer.js", "", None);
        assert_eq!(script.body, crate::shell::VIEWER_JS.as_bytes());

        publish_native(&core.machine_views()[0]);
        let selected = native_selected(&core.machine_views()[0], "lane");
        let turn = selected["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|entry| entry["turn"].as_str())
            .unwrap();

        let paths = [
            "/".to_owned(),
            "/analytics".into(),
            "/sessions".into(),
            "/machines".into(),
            "/machines/testbox".into(),
            "/session/local/lane".into(),
            format!("/session/local/lane/trace/{turn}"),
        ];
        for path in paths {
            assert!(crate::shell::is_page_path(&path), "{path}");
            let reply = core.respond("GET", &path, "", None);
            assert_eq!(reply.status, 200, "{path}");
            assert_eq!(reply.body, crate::shell::PAGE_HTML.as_bytes(), "{path}");
        }

        // The predicate checks dynamic route shapes without querying the model.
        assert!(crate::shell::is_page_path("/machines/not-in-model"));
        for path in [
            "/timeline",
            "/s/claude/not-in-model",
            "/trace/claude/not-in-model/turn",
            "/api/model",
            "/api/analytics",
            "/viewer.js",
            "/viewer.css",
            "/nonexistent.txt",
            "/api/tx",
        ] {
            assert!(!crate::shell::is_page_path(path), "{path}");
        }
    }

    #[test]
    fn non_loopback_listen_is_refused() {
        for address in ["0.0.0.0:1234", "192.168.1.1:1234", "[::1]:1234"] {
            assert!(listen_addr(address).is_err());
        }
        assert_eq!(listen_addr("1234").unwrap().to_string(), "127.0.0.1:1234");
        assert_eq!(
            listen_addr("localhost:1234").unwrap().to_string(),
            "127.0.0.1:1234"
        );
    }

    #[test]
    fn native_transcript_pages_keep_source_order_and_malformed_gaps() {
        let fixture = Fixture::new();
        let path = fixture.claude("many", &[]);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        for number in 1..=430 {
            if number == 221 {
                writeln!(file, "{{bad json").unwrap();
            } else {
                writeln!(file,"{}",json!({"type":"user","sessionId":"many","uuid":format!("message-{number}"),"timestamp":"2026-09-24T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":[{"type":"text","text":format!("message {number}")}]}})).unwrap();
            }
        }
        drop(file);
        let view = fixture.viewer();
        publish_native(&view);
        let mut after = 0;
        let mut entries = Vec::new();
        let mut generation = None;
        loop {
            let mut query = format!("sid=many&after={after}&limit=100");
            if let Some(ref value) = generation {
                query.push_str(&format!("&generation={value}"));
            }
            let reply = crate::session_transcript_range(&fixture.options, "fixture", &query, None);
            assert_eq!(reply.status, 200);
            let page = body_of(&reply);
            let rows = page["entries"].as_array().unwrap();
            assert!(rows.len() <= 100);
            assert!(
                page["observation"]["source_bytes"].as_u64().unwrap()
                    <= page["limits"]["source_bytes"].as_u64().unwrap()
            );
            generation = Some(
                page["projection"]["generation"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            entries.extend(rows.iter().cloned());
            let Some(next) = page["range"]["next"].as_u64() else {
                break;
            };
            after = next;
        }
        let texts: Vec<_> = entries
            .iter()
            .filter(|entry| entry["k"] == "u" || entry["k"] == "h")
            .map(|entry| entry["text"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            texts,
            (1..=430)
                .filter(|number| *number != 221)
                .map(|number| format!("message {number}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry["k"] == "end"
                    && entry["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("unreadable lines")))
                .count(),
            1,
            "the malformed source record remains a visible gap"
        );
        let mut offsets: Vec<_> = entries
            .iter()
            .filter_map(|entry| entry["provenance"]["offset"].as_u64())
            .collect();
        let observed = offsets.clone();
        offsets.sort();
        assert_eq!(
            observed, offsets,
            "native paging preserves original source order"
        );
        assert_eq!(offsets.first(), Some(&0));
        assert!(offsets.last().unwrap() > offsets.first().unwrap());
    }

    #[test]
    fn claude_blocks_and_child_links() {
        let fixture = Fixture::new();
        let prompt = "/tmp/handoff-prompt.md";
        fixture.claude("parent", &[claude_record("parent", vec![
            json!({"type":"text","text":"hello"}),
            json!({"type":"thinking","thinking":"reasoning"}),
            json!({"type":"tool_use","id":"task-1","name":"Task","input":{"prompt":"inspect"}}),
            json!({"type":"tool_result","tool_use_id":"task-1","content":"done"}),
            json!({"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":format!("codex exec < {prompt}")}}),
        ])]);
        fixture.write(
            "claude/projects/project/parent/subagents/agent-child.jsonl",
            &format!("{}\n", claude_record("child", vec![])),
        );
        fixture.write(
            "claude/projects/project/parent/subagents/agent-child.meta.json",
            &json!({"description":"worker","toolUseId":"task-1"}).to_string(),
        );
        fixture.codex("run", &[json!({"type":"event_msg","payload":{"type":"user_message","message":format!("Semon-Parent: claude:parent\nSemon-Handoff: {prompt}\nPrompt")}})]);
        let viewer = fixture.viewer();
        publish_native(&viewer);
        let page = native_selected(&viewer, "parent");
        let entries = page["entries"].as_array().unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry["k"] == "think" && entry["text"] == "reasoning")
        );
        let mut query = crate::Query::new(fixture.options.clone());
        let selected = query.call("get_session", &json!({"id":"parent"})).unwrap();
        let children = selected["session"]["children"].as_array().unwrap();
        assert_eq!(children.len(), 2);
        assert!(children.contains(&json!("child")));
        assert!(children.contains(&json!("run")));
        let handoffs = selected["session"]["handoffs"].as_array().unwrap();
        assert!(
            handoffs
                .iter()
                .any(|handoff| handoff["from"] == "parent" && handoff["to"] == "child")
        );
        assert!(
            handoffs
                .iter()
                .any(|handoff| handoff["from"] == "parent" && handoff["to"] == "run")
        );
    }

    #[test]
    fn transcript_keeps_every_run_launched_by_one_call() {
        let fixture = Fixture::new();
        fixture.claude("parent", &[claude_record("parent", vec![
            json!({"type":"tool_use","id":"launch","name":"Bash","input":{"command":"codex exec < /tmp/one.md; codex exec < /tmp/two.md","run_in_background":true}}),
        ])]);
        for id in ["one", "two"] {
            fixture.codex(id, &[json!({"type":"event_msg","payload":{"type":"user_message","message":format!("Semon-Parent: claude:parent\nSemon-Handoff: /tmp/{id}.md\nPrompt")}})]);
        }
        let viewer = fixture.viewer();
        publish_native(&viewer);
        let mut query = crate::Query::new(fixture.options.clone());
        let selected = query.call("get_session", &json!({"id":"parent"})).unwrap();
        assert_eq!(selected["session"]["children"], json!(["one", "two"]));
        let launches: std::collections::BTreeSet<_> = selected["session"]["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|handoff| handoff["from"] == "parent")
            .filter_map(|handoff| handoff["to"].as_str())
            .collect();
        assert_eq!(launches, std::collections::BTreeSet::from(["one", "two"]));
        let page = native_selected(&viewer, "parent");
        assert!(
            page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["k"] == "tool" || entry["k"] == "h")
        );
    }

    #[test]
    fn codex_blocks_and_first_turn_harness_messages() {
        let fixture = Fixture::new();
        let mut records = vec![
            json!({"type":"event_msg","payload":{"type":"user_message","message":"<recommended_plugins>injected</recommended_plugins>"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<recommended_plugins>injected</recommended_plugins>"}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"real prompt"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"real prompt"}]}}),
            json!({"type":"event_msg","payload":{"type":"agent_reasoning","text":"summary"}}),
            json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"summary block"}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"<later>tag</later>"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<later>tag</later>"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"reply"}]}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"call-1","arguments":"ls"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"{\"output\":\"file\",\"metadata\":{\"exit_code\":0}}"}}),
        ];
        for (index, record) in records.iter_mut().enumerate() {
            record["timestamp"] = json!(format!("2026-09-24T00:00:{:02}Z", index + 1));
        }
        fixture.codex("codex", &records);
        let viewer = fixture.viewer();
        publish_native(&viewer);
        let page = native_selected(&viewer, "codex");
        let entries = page["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry["k"] == "harness"));
        assert!(
            entries
                .iter()
                .any(|entry| (entry["k"] == "u" || entry["k"] == "h")
                    && entry["text"] == "<later>tag</later>")
        );
        for kind in ["a", "think", "tool"] {
            assert!(
                entries.iter().any(|entry| entry["k"] == kind),
                "missing {kind}"
            );
        }
        assert!(
            entries
                .iter()
                .any(|entry| entry["k"] == "think" && entry["text"] == "summary block")
        );
        let tool = entries.iter().find(|entry| entry["k"] == "tool").unwrap();
        assert_eq!(tool["out"], "file");
        assert_eq!(tool["ok"], true);
    }

    #[test]
    fn native_xss_payloads_are_text_and_tool_expansion_is_bounded() {
        let fixture = Fixture::new();
        fixture.write("claude/sessions/secret.key", "SECRET_KEY_NEVER_READ");
        let long = "x".repeat(tx::PREVIEW_MAX + 1);
        fixture.claude("xss",&[
            json!({"type":"user","sessionId":"xss","uuid":"prompt","timestamp":"2026-09-24T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"test safe rendering"}}),
            claude_record("xss",vec![json!({"type":"text","text":"<script>alert(1)</script>"}),json!({"type":"tool_use","id":"call","name":"Bash","input":{"command":format!("\"><img onerror=alert(1)>{long}")}})]),
            json!({"type":"user","sessionId":"xss","uuid":"result","timestamp":"2026-09-24T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":format!("<script>{long}")}]}}),
        ]);
        let view = fixture.viewer();
        publish_native(&view);
        let page = native_selected(&view, "xss");
        let entries = page["entries"].as_array().unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry["text"] == "<script>alert(1)</script>")
        );
        let tool = entries.iter().find(|entry| entry["k"] == "tool").unwrap();
        assert!(tool["more"].as_array().unwrap().contains(&json!("out")));
        assert_eq!(
            tool["out"],
            format!("<script>{}…", "x".repeat(tx::PREVIEW_MAX - 8))
        );
        let slot = tool["slot"].as_u64().unwrap();
        let generation = page["projection"]["generation"].as_str().unwrap();
        let reply = crate::session_entry_field(
            &fixture.options,
            "fixture",
            &format!("sid=xss&after={slot}&limit=1&generation={generation}&part=output"),
            None,
        );
        assert_eq!(reply.status, 200);
        let expanded = body_of(&reply);
        let output = expanded["entries"][0]["out"].as_str().unwrap();
        assert!(output.contains("<script>"));
        assert_eq!(output, format!("<script>{long}"));
        assert!(output.len() > tx::PREVIEW_MAX && output.len() <= tx::FULL_MAX);
        assert_eq!(expanded["entries"][0]["outputTruncated"], false);
        assert_eq!(
            crate::session_entry_field(
                &fixture.options,
                "fixture",
                "sid=xss&after=0&limit=101&part=output",
                None
            )
            .status,
            400
        );
        let token = "0123456789abcdef0123456789abcdef";
        let wire = http(
            &fixture,
            &format!(
                "GET /api/session-transcript?sid=xss&limit=100&t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n"
            ),
        );
        assert!(wire.starts_with("HTTP/1.1 200"), "{wire}");
        let body = wire.split_once("\r\n\r\n").unwrap().1;
        assert!(body.contains("<script>alert(1)</script>"));
        assert!(!wire.contains("SECRET_KEY_NEVER_READ"));
        assert!(!application_script(crate::shell::VIEWER_JS).contains("innerHTML"));
        assert!(crate::shell::VIEWER_JS.contains("textContent"));
    }

    #[test]
    fn codex_subagent_and_live_claude_links() {
        let fixture = Fixture::new();
        fixture.codex("root", &[]);
        fixture.write("codex/sessions/2026/09/24/rollout-child.jsonl", &format!("{}\n", json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":{"id":"child","session_id":"root","parent_thread_id":"root","thread_source":"subagent","agent_nickname":"worker"}})));
        let viewer = fixture.viewer();
        publish_native(&viewer);
        let mut query = crate::Query::new(fixture.options.clone());
        let selected = query.call("get_session", &json!({"id":"root"})).unwrap();
        assert_eq!(selected["session"]["children"], json!(["child"]));

        let mut fields = vec!["0"; 20];
        fields[19] = "123";
        fixture.write(
            "proc/100/stat",
            &format!("100 (agent) {}\n", fields.join(" ")),
        );
        fixture.write(
            "claude/sessions/100.json",
            &json!({"pid":100,"sessionId":"live","procStart":123,"status":"busy"}).to_string(),
        );
        fixture.claude(
            "live",
            &[claude_record(
                "live",
                vec![json!({"type":"text","text":"now"})],
            )],
        );
        publish_native(&viewer);
        let page = native_selected(&viewer, "live");
        assert_eq!(page["session"]["details"]["live_observation"], true);
        let selected = query.call("get_session", &json!({"id":"live"})).unwrap();
        assert_eq!(selected["session"]["alive"], true);
    }

    #[test]
    fn transcript_last_page_of_large_file_reads_bounded_bytes() {
        let fixture = Fixture::new();
        let path = fixture.claude("large", &[]);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        let padding = "x".repeat(15_000);
        let mut written = 0u64;
        let mut number = 0u64;
        while written < 8 * 1024 * 1024 {
            let line = json!({"type":"user","sessionId":"large","timestamp":"2026-09-24T00:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":[{"type":"text","text":format!("line {number} {padding}")}]}}).to_string();
            written += line.len() as u64 + 1;
            writeln!(file, "{line}").unwrap();
            number += 1;
        }
        drop(file);
        assert!(fs::metadata(&path).unwrap().len() >= 8 * 1024 * 1024);
        let view = fixture.viewer();
        publish_native(&view);
        let reply =
            crate::session_transcript_range(&fixture.options, "fixture", "sid=large&limit=1", None);
        assert_eq!(reply.status, 200);
        let page = body_of(&reply);
        assert_eq!(page["entries"].as_array().unwrap().len(), 1);
        let last = &page["entries"][0];
        assert!(
            last["text"]
                .as_str()
                .unwrap()
                .starts_with(&format!("line {} ", number - 1))
        );
        let bytes_read = page["observation"]["source_bytes"].as_u64().unwrap();
        assert!(bytes_read <= page["limits"]["source_bytes"].as_u64().unwrap());
        assert!(
            bytes_read < 32 * 1024,
            "one selected native record read {bytes_read} bytes from 8 MB history"
        );
        assert_retired_routes_absent(&view);
    }

    #[cfg(unix)]
    #[test]
    fn warm_tree_and_idle_polling_read_no_transcript_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let path = fixture.claude(
            "warm",
            &[claude_record(
                "warm",
                vec![json!({"type":"text","text":"hello"})],
            )],
        );
        let viewer = fixture.viewer();
        let warm = viewer.tree_json().unwrap();
        assert!(warm.contains("warm"));
        // Nothing changes from here on: revoke read access so that a
        // regression which reopens and reparses this file (instead of
        // trusting the in-memory tree cache) turns into a hard failure
        // rather than a silent extra read. Repeated calls stand in for a
        // client polling an idle server between requests.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let polls = [
            viewer.tree_json().unwrap(),
            viewer.tree_json().unwrap(),
            viewer.tree_json().unwrap(),
        ];
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        for poll in polls {
            assert_eq!(poll, warm);
        }
    }

    /// A machine named `host` with one session, `sid`, and a relay from a
    /// sender whose logs are gone: a stub only this machine knows.
    #[cfg(unix)]
    #[test]
    fn a_refused_archive_root_replaced_by_a_directory_invalidates_the_snapshot() {
        let fixture = Fixture::new();
        let target = fixture.root.join("disposable-outside");
        let root = fixture.options.codex_home.join("archived_sessions");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &root).unwrap();
        let snapshot =
            Snapshot::capture_pids(&fixture.options, BTreeSet::new(), std::iter::empty());
        assert!(snapshot.watched.contains_key(root.parent().unwrap()));
        fs::remove_file(&root).unwrap();
        // Keep the target inode and mtime: watching only the resolved target
        // would miss this change from refused link to accepted directory.
        fs::rename(&target, &root).unwrap();
        assert!(snapshot.changed(&fixture.options));
    }

    fn machine(host: &str, sid: &str) -> Fixture {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", &format!("{host}\n"));
        fixture.claude(
            sid,
            &[
                json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":sid,"origin":{"kind":"human"},
                    "message":{"role":"user","content":format!("work on {host}")}}),
                json!({"type":"user","timestamp":"2026-09-24T00:01:00Z","sessionId":sid,
                    "origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/9.sock","name":"ghost","msg_id":format!("m-ghost-{host}-{sid}"),"body":"hello"},
                    "message":{"role":"user","content":"hello"}}),
                json!({"type":"assistant","timestamp":"2026-09-24T00:02:00Z","sessionId":sid,
                    "message":{"role":"assistant","content":[{"type":"text","text":format!("done on {host}")}]}}),
            ],
        );
        fixture
    }

    fn body_of(reply: &ViewerReply) -> Value {
        serde_json::from_slice(&reply.body).unwrap()
    }

    /// `/api/tree` stamps each node with `last_activity_age_seconds`, whole
    /// seconds between the node's last record and the moment of the build.
    /// Two builds that straddle a wall-clock second boundary legitimately
    /// differ there and nowhere else; zero the number, as [`without_now`]
    /// does for the model. A body with no number at all is an error: it would
    /// turn this into a silent no-op.
    fn without_age(body: &[u8]) -> String {
        const KEY: &str = "\"last_activity_age_seconds\":";
        let text = String::from_utf8(body.to_vec()).unwrap();
        let mut out = String::new();
        let mut rest = text.as_str();
        let mut seen = 0;
        while let Some((head, tail)) = rest.split_once(KEY) {
            let tail = tail.trim_start();
            let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            out.push_str(head);
            out.push_str(KEY);
            out.push(' ');
            if digits > 0 {
                out.push('0');
                seen += 1;
            } else {
                // A node with no timestamp is `null`, and stays so.
                assert!(
                    tail.starts_with("null"),
                    "age is neither a number nor null: {}",
                    tail.chars().take(20).collect::<String>()
                );
            }
            rest = &tail[digits..];
        }
        assert!(seen > 0, "no last_activity_age_seconds in {text}");
        out.push_str(rest);
        out
    }

    #[test]
    fn session_controls_bind_machine_keys_even_when_hostnames_collide() {
        let first = machine("same-host", "first-session");
        let second = machine("same-host", "second-session");
        let core = ViewerCore::with_machines(vec![
            ("first-key".into(), first.options.clone()),
            ("second-key".into(), second.options.clone()),
        ]);
        let cold_reads = crate::events::FILE_READS.with(std::cell::Cell::get);
        assert_eq!(
            core.session_machine_key("first-session").unwrap(),
            None,
            "cold control lookup must wait for publication"
        );
        assert_eq!(
            crate::events::FILE_READS.with(std::cell::Cell::get),
            cold_reads,
            "cold control lookup decoded source history"
        );
        publish_core_native(&core);
        assert_eq!(
            core.session_machine_key("first-session")
                .unwrap()
                .as_deref(),
            Some("first-key")
        );
        assert_eq!(
            core.session_machine_key("second-session")
                .unwrap()
                .as_deref(),
            Some("second-key")
        );
        assert_eq!(core.session_machine_key("missing").unwrap(), None);
        let identity = core
            .session_view_identity("first-session")
            .unwrap()
            .unwrap();
        assert_eq!(identity.0, "first-key");
        let alias = format!("first-session@{}", identity.1);
        assert_eq!(
            core.session_view_identity(&alias).unwrap(),
            Some(identity.clone())
        );

        let inventory = body_of(&core.respond("GET", "/api/session-sources", "", None));
        assert!(
            inventory["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["source_key"] == identity.0)
        );
        assert_eq!(core.session_view_identity("missing").unwrap(), None);

        let duplicate = machine("same-host", "first-session");
        let ambiguous = ViewerCore::with_machines(vec![
            ("first-key".into(), first.options.clone()),
            ("duplicate-key".into(), duplicate.options.clone()),
        ]);
        assert_eq!(
            ambiguous.session_machine_key("first-session").unwrap(),
            None
        );
        core.close();
        assert_eq!(core.session_machine_key("first-session").unwrap(), None);
    }

    #[test]
    fn published_control_ownership_reads_only_selected_role_metadata_and_no_models() {
        for unrelated in [0, 256, 2048] {
            let first = machine("same-host", "selected");
            let second = machine("same-host", "peer");
            for n in 0..unrelated {
                let id = format!("unrelated-{n}");
                second.claude(
                    &id,
                    &[serde_json::json!({
                        "type":"user", "sessionId":id, "uuid":id,
                        "timestamp":"2026-10-01T00:00:00Z",
                        "message":{"role":"user","content":"unrelated evidence"}
                    })],
                );
            }
            let core = ViewerCore::with_machines(vec![
                ("first-key".into(), first.options.clone()),
                ("second-key".into(), second.options.clone()),
            ]);
            for view in core.machine_views() {
                let mut work = lock(&view.evidence.work);
                while !view
                    .evidence
                    .refresh_catalog(&mut work, model::now_ms())
                    .unwrap()
                {}
            }
            let before = crate::events::FILE_READS.with(std::cell::Cell::get);
            let builds = crate::model::BUILDS.with(std::cell::Cell::get);
            assert_eq!(
                core.session_machine_key("selected").unwrap().as_deref(),
                Some("first-key")
            );
            assert_eq!(
                core.session_machine_key("peer").unwrap().as_deref(),
                Some("second-key")
            );
            assert_eq!(core.session_machine_key("missing").unwrap(), None);
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - before;
            eprintln!("published control ownership unrelated={unrelated} index_loads={loads}");
            // Generation-bound ledgers and sidecar metadata qualify ownership
            // without any full event-index loads, including the selected one.
            assert_eq!(loads, 0);
            assert_eq!(crate::model::BUILDS.with(std::cell::Cell::get), builds);

            let connection =
                rusqlite::Connection::open(EventCache::path(&first.options.cache)).unwrap();
            connection
                .execute(
                    "UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='selected'",
                    [],
                )
                .unwrap();
            assert_eq!(
                core.session_machine_key("selected").unwrap(),
                None,
                "retained history cannot select a writable current source"
            );
        }
    }

    #[test]
    fn partial_control_ownership_cannot_hide_a_duplicate_native_session() {
        let first = machine("same-host", "selected");
        let second = machine("same-host", "peer");
        let core = ViewerCore::with_machines(vec![
            ("first-key".into(), first.options.clone()),
            ("second-key".into(), second.options.clone()),
        ]);
        for view in core.machine_views() {
            let mut work = lock(&view.evidence.work);
            while !view
                .evidence
                .refresh_catalog(&mut work, model::now_ms())
                .unwrap()
            {}
        }
        second.claude(
            "selected",
            &[serde_json::json!({
                "type":"user", "sessionId":"selected", "uuid":"duplicate",
                "timestamp":"2026-10-01T00:00:00Z",
                "message":{"role":"user","content":"a newly discovered duplicate"}
            })],
        );
        let connection =
            rusqlite::Connection::open(EventCache::path(&second.options.cache)).unwrap();
        connection
            .execute(
                "UPDATE meta SET value='partial' WHERE key='catalog_completeness'",
                [],
            )
            .unwrap();
        let before = crate::events::FILE_READS.with(std::cell::Cell::get);
        assert_eq!(core.session_machine_key("selected").unwrap(), None);
        assert_eq!(
            crate::events::FILE_READS.with(std::cell::Cell::get),
            before,
            "partial ownership lookup loaded history to infer completeness"
        );
    }

    #[test]
    fn one_machine_native_reads_preserve_content_across_embedding_modes() {
        let fixture = machine("laptop", "lane");
        let standalone = ViewerCore::new(fixture.options.clone());
        let mut embedded =
            ViewerCore::with_machines(vec![("some-key".into(), fixture.options.clone())]);
        let mut background =
            ViewerCore::with_machines(vec![("some-key".into(), fixture.options.clone())]);
        background.set_refresh(Refresh::Background);
        for core in [&standalone, &embedded, &background] {
            publish_core_native(core);
        }
        assert_eq!(
            embedded.session_machine_key("lane").unwrap().as_deref(),
            Some("some-key")
        );
        let single = body_of(&standalone.respond(
            "GET",
            "/api/session-transcript",
            "sid=lane&limit=100",
            None,
        ));
        for core in [&embedded, &background] {
            let selected = body_of(&core.respond(
                "GET",
                "/api/session-transcript",
                "machine=some-key&sid=lane&limit=100",
                None,
            ));
            let canonical_entries = |page: &Value| {
                page["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| {
                        let mut entry = entry.clone();
                        let id = entry["entry_id"].as_str().unwrap();
                        assert_eq!(id.len(), 64);
                        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
                        entry.as_object_mut().unwrap().remove("entry_id");
                        entry
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(canonical_entries(&single), canonical_entries(&selected));
            assert_eq!(single["range"], selected["range"]);
            assert_eq!(single["projection"], selected["projection"]);
            assert_eq!(selected["identity"]["source_key"], "some-key");
            assert_eq!(
                selected["identity"]["native_ids"],
                single["identity"]["native_ids"]
            );
        }
        for path in ["/api/tree", "/machines/laptop", "/viewer.js"] {
            let a = standalone.respond("GET", path, "", None);
            for core in [&embedded, &background] {
                let b = core.respond("GET", path, "", None);
                assert_eq!((a.status, a.content_type), (b.status, b.content_type));
                if path == "/api/tree" {
                    assert_eq!(without_age(&a.body), without_age(&b.body));
                } else {
                    assert_eq!(a.body, b.body);
                }
            }
        }
        // Source namespace belongs to each host: standalone uses local, while
        // an embedder retains its supplied key even with only one source.
        for (core, source) in [
            (&standalone, "local"),
            (&embedded, "some-key"),
            (&background, "some-key"),
        ] {
            assert_eq!(core.respond("GET", "/s/claude/lane", "", None).status, 404);
            let canonical = format!("/session/{source}/lane");
            let page = core.respond("GET", &canonical, "", None);
            assert_eq!((page.status, page.body.as_slice()), (200, PAGE.as_bytes()));
        }
        assert!(
            body_of(&embedded.respond("GET", "/api/viewer/chrome", "", None))["admin"].is_null()
        );
        embedded.set_admin_link(crate::AdminLink::new("Manage machines", "/admin/machines"));
        assert_eq!(
            body_of(&embedded.respond("GET", "/api/viewer/chrome", "", None))["admin"],
            json!({"label":"Manage machines","href":"/admin/machines"})
        );
        standalone.close();
        embedded.close();
        background.close();
    }

    #[test]
    fn account_nav_and_management_are_per_request_native_chrome() {
        let fixture = lane_fixture();
        let mut core = crate::ViewerCore::new(fixture.options.clone());
        let plain = core.respond("GET", "/api/viewer/chrome", "", None);
        assert_eq!(plain.status, 200);
        let plain_model: Value = serde_json::from_slice(&plain.body).unwrap();
        assert!(plain_model["account"].is_null());
        assert_eq!(plain_model["navigation"]["paths"]["machines"], "/machines");

        let menu = |name: &str| {
            crate::AccountMenu::new(
                name,
                "reader@example.invalid",
                "R",
                Some("/avatars/reader.png"),
                vec![crate::AccountWorkspace {
                    name: "Research".into(),
                    role: "Owner".into(),
                    current: true,
                    switch_href: "/workspaces/research".into(),
                }],
                vec![
                    crate::AccountLink {
                        label: "Profile".into(),
                        href: "/account/profile".into(),
                        method: crate::LinkMethod::default(),
                        danger: false,
                    },
                    crate::AccountLink {
                        label: "Sign out".into(),
                        href: "/account/sign-out".into(),
                        method: crate::LinkMethod::Post,
                        danger: true,
                    },
                ],
            )
            .unwrap()
        };
        core.set_account(Some(menu("Morgan Lee")));
        assert!(core.set_nav_override("machines", "/account/workspaces"));
        let first = core.respond("GET", "/api/viewer/chrome", "", None);
        assert_eq!(first.status, 200);
        let first_model: Value = serde_json::from_slice(&first.body).unwrap();
        assert_eq!(first_model["account"]["name"], "Morgan Lee");
        assert_eq!(first_model["account"]["login"], "reader@example.invalid");
        assert_eq!(first_model["account"]["initials"], "R");
        assert_eq!(first_model["account"]["avatar_href"], "/avatars/reader.png");
        assert_eq!(first_model["account"]["workspaces"][0]["name"], "Research");
        assert_eq!(first_model["account"]["workspaces"][0]["role"], "Owner");
        assert_eq!(first_model["account"]["workspaces"][0]["current"], true);
        assert_eq!(
            first_model["account"]["workspaces"][0]["switch_href"],
            "/workspaces/research"
        );
        assert_eq!(first_model["account"]["links"][0]["label"], "Profile");
        assert_eq!(
            first_model["account"]["links"][0]["href"],
            "/account/profile"
        );
        assert_eq!(first_model["account"]["links"][0]["danger"], false);
        assert_eq!(first_model["account"]["links"][0]["method"], "get");
        assert_eq!(first_model["account"]["links"][1]["method"], "post");
        assert_eq!(
            first_model["navigation"]["paths"],
            json!({"machines":"/account/workspaces"})
        );
        assert!(first.etag.is_none());
        let reads = crate::events::FILE_READS.with(std::cell::Cell::get);
        core.set_admin_link(crate::AdminLink::new("Manage machines", "/admin/machines"));
        let managed = body_of(&core.respond("GET", "/api/viewer/chrome", "", None));
        assert_eq!(
            managed["admin"],
            json!({"label":"Manage machines","href":"/admin/machines"})
        );
        core.set_account(Some(menu("Another account")));
        let second = core.respond("GET", "/api/viewer/chrome", "", None);
        assert_eq!(second.status, 200);
        let second_model: Value = serde_json::from_slice(&second.body).unwrap();
        assert_eq!(second_model["account"]["name"], "Another account");
        assert_eq!(second_model["admin"], managed["admin"]);
        assert!(second.etag.is_none());
        // Per request, without touching the shared core: its own extras.
        let mut extras = crate::Extras::default();
        extras.set_account(Some(menu("Per request")));
        let own = core.respond_with(&extras, "GET", "/api/viewer/chrome", "", None);
        let own_model = body_of(&own);
        assert_eq!(own_model["account"]["name"], "Per request");
        assert_eq!(own_model["navigation"]["paths"]["machines"], "/machines");
        assert!(own_model["admin"].is_null());
        assert!(own.etag.is_none());
        assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), reads);
        let shared = body_of(&core.respond("GET", "/api/viewer/chrome", "", None));
        assert_eq!(shared["account"]["name"], "Another account");
        assert_eq!(core.extras().clone(), {
            let mut set = crate::Extras::default();
            set.set_account(Some(menu("Another account")));
            set.set_admin_link(crate::AdminLink::new("Manage machines", "/admin/machines"));
            assert!(set.set_nav_override("machines", "/account/workspaces"));
            set
        });
    }

    #[test]
    fn several_machines_share_native_evidence_and_answer_for_their_own() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let sources = Arc::new(crate::SessionSources::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]));
        let core = ViewerCore::from_sources(sources.clone());
        let mut query = crate::Query::from_sources(sources);
        let home = native_home(&core);
        assert_eq!(machine_ids(&home), ["alpha", "bravo"]);
        assert_eq!(home_owner(&home, "lane-a").as_deref(), Some("alpha"));
        assert_eq!(home_owner(&home, "lane-b").as_deref(), Some("bravo"));
        let mut stubs = BTreeSet::new();
        for (source, sid, machine) in [("a", "lane-a", "alpha"), ("b", "lane-b", "bravo")] {
            let reply = core.respond(
                "GET",
                "/api/session-transcript",
                &format!("machine={source}&sid={sid}&limit=100"),
                None,
            );
            assert_eq!(reply.status, 200);
            assert_eq!(body_of(&reply)["identity"]["source_key"], source);
            assert!(String::from_utf8_lossy(&reply.body).contains(&format!("done on {machine}")));
            let details = query.call("get_session", &json!({"id":sid})).unwrap();
            let session = &details["session"];
            assert_eq!(session["machine"], machine);
            assert!(session["busy"].is_array());
            for turn in session["turns"].as_array().unwrap() {
                assert_eq!(turn["sid"], sid);
            }
            for handoff in session["handoffs"].as_array().unwrap() {
                for end in ["from", "to"] {
                    if let Some(id) = handoff[end]
                        .as_str()
                        .filter(|id| *id != "you" && *id != sid)
                    {
                        let endpoint = query.call("get_session", &json!({"id":id})).unwrap();
                        assert_eq!(endpoint["session"]["machine"], machine);
                        assert!(id.ends_with(&format!("@{machine}")));
                        stubs.insert(id.to_owned());
                    }
                }
            }
        }
        assert_eq!(
            stubs.len(),
            2,
            "gone senders remain separate native relationships"
        );
        assert_eq!(
            core.respond(
                "GET",
                "/api/session-transcript",
                "machine=b&sid=nobody",
                None
            )
            .status,
            404
        );
        for (path, status) in [
            ("/session/a/lane-a", 200),
            ("/session/b/lane-b", 200),
            ("/s/claude/nobody", 404),
            ("/machines/alpha", 200),
            ("/machines/bravo", 200),
            ("/machines/gamma", 404),
            ("/timeline", 404),
            ("/analytics", 200),
        ] {
            assert_eq!(core.respond("GET", path, "", None).status, status, "{path}");
        }
        let before = body_of(&core.respond("GET", "/api/sessions", "machine=b&sid=lane-b", None));
        say(&bravo, "lane-b", 1, "more");
        publish_core_native(&core);
        let after = body_of(&core.respond("GET", "/api/sessions", "machine=b&sid=lane-b", None));
        assert_ne!(before["generation"], after["generation"]);
        core.close();
    }

    #[test]
    fn a_session_id_two_machines_share_requires_explicit_native_source_authority() {
        let (alpha, bravo) = (machine("alpha", "same"), machine("bravo", "same"));
        let core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        publish_core_native(&core);
        assert_eq!(core.session_machine_key("same").unwrap(), None);
        let missing_scope =
            core.respond("GET", "/api/session-transcript", "sid=same&limit=100", None);
        assert_eq!(missing_scope.status, 400);
        assert_eq!(
            body_of(&missing_scope)["error"]["code"],
            "machine_scope_required"
        );
        for (source, name) in [("a", "alpha"), ("b", "bravo")] {
            let selected = core.respond(
                "GET",
                "/api/session-transcript",
                &format!("machine={source}&sid=same&limit=100"),
                None,
            );
            assert_eq!(selected.status, 200);
            assert_eq!(body_of(&selected)["identity"]["source_key"], source);
            assert!(String::from_utf8_lossy(&selected.body).contains(&format!("done on {name}")));
        }
        assert_eq!(core.respond("GET", "/s/claude/same", "", None).status, 404);
        for source in ["a", "b"] {
            let page = core.respond("GET", &format!("/session/{source}/same"), "", None);
            assert_eq!((page.status, page.body.as_slice()), (200, PAGE.as_bytes()));
        }
        let analytics = analytics_of(&core, "range=7d");
        assert_eq!(
            analytics["machine_labels"],
            json!({"a":"alpha","b":"bravo"})
        );
        core.close();
    }

    /// `ms` (epoch ms, UTC) as RFC 3339, as the logs write it.
    fn iso(ms: i64) -> String {
        let days = ms.div_euclid(86_400_000);
        let of_day = ms.rem_euclid(86_400_000);
        // Civil from days (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
            of_day / 3_600_000,
            of_day / 60_000 % 60,
            of_day / 1000 % 60,
            of_day % 1000
        )
    }

    const MINUTE: i64 = 60_000;
    const DAY: i64 = 86_400_000;

    /// A machine whose model keeps its default day (`all` off), with a
    /// session per `(id, how long ago it started, minutes it ran)`: your
    /// message, then a reply that many minutes later (at most five, or the
    /// two lines are two busy intervals of no length).
    fn dated(host: &str, now: i64, sessions: &[(&str, i64, i64)]) -> Fixture {
        let mut fixture = Fixture::new();
        fixture.options.all = false;
        fixture.write("proc/sys/kernel/hostname", &format!("{host}\n"));
        for (sid, ago, minutes) in sessions {
            let start = now - ago;
            fixture.claude(
                sid,
                &[
                    json!({"type":"user","timestamp":iso(start),"sessionId":sid,"origin":{"kind":"human"},
                        "message":{"role":"user","content":format!("work on {sid}")}}),
                    json!({"type":"assistant","timestamp":iso(start + minutes * MINUTE),"sessionId":sid,
                        "message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}),
                ],
            );
        }
        fixture
    }

    fn publish_core_native(core: &ViewerCore) {
        assert_eq!(
            core.respond("GET", "/api/session-sources", "", None).status,
            200
        );
        for view in core.machine_views() {
            publish_native(&view);
            assert_retired_routes_absent(&view);
        }
    }

    fn selected_id(source: &str, key: &str) -> String {
        json!([source, key]).to_string()
    }

    fn analytics_of(core: &ViewerCore, query: &str) -> Value {
        let reply = core.respond("GET", "/api/viewer/analytics", query, None);
        assert_eq!(
            reply.status,
            200,
            "{query}: {}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(reply.content_type, "application/json; charset=utf-8");
        body_of(&reply)
    }

    /// #102: Analytics' 7 d and 30 d reach past the model's day. The model
    /// leaves out a session from five days ago; the week counts it, and the
    /// month counts one from twenty days ago too. The week before counts one
    /// from ten days ago.
    #[test]
    fn native_analytics_ranges_preserve_all_supported_history() {
        let now = model::now_ms();
        let fixture = dated(
            "testbox",
            now,
            &[
                ("today", 60 * MINUTE, 2),
                ("older", 5 * DAY, 4),
                ("before", 10 * DAY, 3),
                ("month", 20 * DAY, 5),
            ],
        );
        let core = ViewerCore::new(fixture.options.clone());
        publish_core_native(&core);
        let catalog = body_of(&core.respond("GET", "/api/sessions", "sid=older", None));
        assert_eq!(catalog["items"][0]["key"], "older");
        let home = body_of(&core.respond("GET", "/api/viewer/home", "", None));
        let day = analytics_of(&core, "range=24h");
        let week = analytics_of(&core, "range=7d");
        let month = analytics_of(&core, "range=30d");
        for (answer, range, started, minutes) in [
            (&day, "24h", 1, 2),
            (&week, "7d", 2, 6),
            (&month, "30d", 4, 14),
        ] {
            assert_eq!(answer["range"], range);
            assert_eq!(answer["version"], home["version"], "{range}");
            assert_eq!(answer["current"]["started"], started, "{range}");
            assert_eq!(answer["current"]["turns"], started, "{range}");
            assert_eq!(answer["current"]["agent_ms"], minutes * MINUTE, "{range}");
        }
        assert!(
            week["sessions"]
                .get(selected_id("local", "older"))
                .is_some()
        );
        assert!(day["sessions"].get(selected_id("local", "older")).is_none());
        let busy: Vec<&str> = week["top"]["busy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["sid"].as_str().unwrap())
            .collect();
        assert_eq!(
            busy,
            [selected_id("local", "older"), selected_id("local", "today")]
        );
        // The previous period: the 7 days before the week hold `before`.
        assert_eq!(week["previous"]["started"], 1);
        assert_eq!(week["previous"]["agent_ms"], 3 * MINUTE);
        assert_eq!(day["previous"]["started"], 0);
        assert_eq!(month["previous"]["started"], 0);
        assert_eq!(week["facets"]["machine"], json!(["local"]));
    }

    #[test]
    fn analytics_refuses_anything_but_a_range_and_the_filters() {
        let fixture = dated("testbox", model::now_ms(), &[("today", 60 * MINUTE, 2)]);
        let core = ViewerCore::new(fixture.options.clone());
        publish_core_native(&core);
        for query in [
            "",
            "range=1d",
            "range=7d&range=7d",
            "range=7d&range=30d",
            "range=7d&extra=1",
            "range=7d&machine=",
            "range=7d&repo=%zz",
            "repo=harbor",
            "range=7d&",
        ] {
            let reply = core.respond("GET", "/api/viewer/analytics", query, None);
            assert_eq!(reply.status, 400, "{query:?}");
            assert_eq!(reply.etag, None);
        }
        assert_eq!(
            core.respond("POST", "/api/viewer/analytics", "range=7d", None)
                .status,
            405
        );
        assert!(!crate::shell::is_page_path("/api/viewer/analytics"));
    }

    /// Repeated native polls preserve the published generation; an append
    /// changes that generation and the corresponding activity totals.
    #[test]
    fn native_analytics_polls_are_generation_fenced_when_logs_change() {
        let now = model::now_ms();
        let fixture = dated("testbox", now, &[("today", 30 * MINUTE, 2)]);
        let core = ViewerCore::new(fixture.options.clone());
        publish_core_native(&core);
        let first = analytics_of(&core, "range=7d");
        let version = first["version"].as_str().unwrap().to_owned();
        assert_eq!(version.len(), 64);
        let again = analytics_of(&core, "range=7d");
        assert_eq!(again["version"], version);
        assert_eq!(again["current"], first["current"]);
        let month = analytics_of(&core, "range=30d");
        assert_eq!(
            month["version"], version,
            "same qualified source publication"
        );
        assert_eq!(month["range"], "30d");
        let path = fixture.root.join("claude/projects/project/today.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"assistant","timestamp":iso(now - 26 * MINUTE),"sessionId":"today",
            "message":{"role":"assistant","content":[{"type":"text","text":"later"}]}})
        )
        .unwrap();
        drop(file);
        publish_core_native(&core);
        let answer = analytics_of(&core, "range=7d");
        assert_ne!(answer["version"], version);
        assert_eq!(answer["current"]["agent_ms"], 4 * MINUTE);
        let home = body_of(&core.respond("GET", "/api/viewer/home", "", None));
        assert_eq!(answer["version"], home["version"]);
        assert_eq!(
            analytics_of(&core, "range=7d")["version"],
            answer["version"]
        );
    }

    /// A filter value no session has is answered, empty, with a 200.
    #[test]
    fn analytics_answers_a_filter_no_session_has_with_nothing() {
        let fixture = dated("testbox", model::now_ms(), &[("today", 60 * MINUTE, 2)]);
        let core = ViewerCore::new(fixture.options.clone());
        publish_core_native(&core);
        let known = analytics_of(&core, "range=7d&machine=local");
        assert_eq!(known["current"]["started"], 1);
        for query in [
            "range=7d&repo=nowhere",
            "range=7d&machine=elsewhere",
            "range=7d&model=x",
        ] {
            let answer = analytics_of(&core, query);
            assert_eq!(answer["current"]["started"], 0, "{query}");
            assert_eq!(answer["top"]["busy"], json!([]), "{query}");
            // The filters' values are still the range's.
            assert_eq!(answer["facets"]["machine"], json!(["local"]), "{query}");
        }
    }

    /// What the 60-day rows cost in memory, on a fixture shaped like a busy
    /// machine: ten sessions a day for seventy days, in four repos and two
    /// models, each eight turns of your message, a tool call, its result and
    /// a reply, twenty minutes apart. The measurement is written to the
    /// test's real stdout (and as a CI notice); the bound guards it.
    #[test]
    fn analytics_rows_memory_on_a_busy_fixture() {
        let now = model::now_ms();
        let mut fixture = Fixture::new();
        fixture.options.all = false;
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        let repos = ["harbor", "atlas", "quill", "ledger"];
        for repo in repos {
            fs::create_dir_all(fixture.root.join("work").join(repo).join(".git")).unwrap();
        }
        let models = ["claude-opus-5-5", "claude-sonnet-5"];
        let usage = json!({"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":5000,"cache_creation_input_tokens":0});
        for day in 0..70i64 {
            for n in 0..10i64 {
                let sid = format!("s-{day:02}-{n}");
                let cwd = fixture
                    .root
                    .join("work")
                    .join(repos[usize::try_from(n).unwrap() % repos.len()]);
                let cwd = cwd.to_string_lossy();
                let model = models[usize::try_from(n).unwrap() % models.len()];
                let start = now - day * DAY - n * 47 * MINUTE - 60 * MINUTE;
                let mut lines = Vec::new();
                for turn in 0..8i64 {
                    let t = start + turn * 20 * MINUTE;
                    let tool = format!("t-{sid}-{turn}");
                    lines.extend([
                        json!({"type":"user","timestamp":iso(t),"sessionId":sid,"cwd":cwd,"origin":{"kind":"human"},
                            "message":{"role":"user","content":format!("turn {turn}")}}),
                        json!({"type":"assistant","timestamp":iso(t + 3 * MINUTE),"sessionId":sid,"cwd":cwd,
                            "message":{"id":format!("m-{sid}-{turn}"),"model":model,"role":"assistant","usage":usage,
                                "content":[{"type":"tool_use","id":tool,"name":"Bash","input":{"command":"true"}}]}}),
                        json!({"type":"user","timestamp":iso(t + 6 * MINUTE),"sessionId":sid,"cwd":cwd,
                            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":tool,"content":"ok"}]}}),
                        json!({"type":"assistant","timestamp":iso(t + 9 * MINUTE),"sessionId":sid,"cwd":cwd,
                            "message":{"id":format!("r-{sid}-{turn}"),"model":model,"role":"assistant","usage":usage,
                                "content":[{"type":"text","text":"done"}]}}),
                    ]);
                }
                fixture.claude(&sid, &lines);
            }
        }
        // Measure the active native aggregator before output-window trimming.
        fixture.options.all = true;
        let mut cache = EventCache::open_scoped(&fixture.options.cache).unwrap();
        let built = model::build_query(
            &fixture.options,
            &mut cache,
            &mut false,
            &mut model::Texts::default(),
            now,
        )
        .unwrap();
        let rows = &crate::analytics::activity(
            &built.sessions,
            &built.tx,
            &built.turns,
            &built.handoffs,
            now,
        );
        let bytes = crate::analytics::heap_bytes(rows);
        let intervals: usize = rows.values().map(|row| row.busy.len()).sum();
        let turns: usize = rows.values().map(|row| row.turns.len()).sum();
        let days: usize = rows.values().map(|row| row.cost_by_day.len()).sum();
        let line = format!(
            "analytics rows: {} of 700 sessions kept, {intervals} busy intervals, {turns} turns, \
             {days} cost days, {bytes} bytes ({} per session)",
            rows.len(),
            bytes / rows.len().max(1)
        );
        // Past the test harness's capture, so a passing run shows it.
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}\n::notice title=Analytics memory::{line}");
        drop(out);
        // Sixty days of ten a day, and the two of day 60 whose last line is
        // still inside the window.
        assert!((600..=610).contains(&rows.len()), "{line}");
        // Those well inside it keep every interval, turn and cost day; at
        // its edge only what is inside is kept.
        let inside = rows.values().filter(|row| row.start >= now - 59 * DAY);
        assert!(
            inside
                .clone()
                .all(|row| row.busy.len() == 8 && row.turns.len() == 8),
            "{line}"
        );
        assert!(
            inside.clone().all(|row| !row.cost_by_day.is_empty()),
            "{line}"
        );
        assert!(inside.count() >= 590, "{line}");
        assert!(bytes < 1024 * rows.len(), "{line}");
        // Each row's vectors hold no spare capacity (`shrink_to_fit`), so
        // what is measured is what they need.
        for (id, row) in rows {
            assert_eq!(row.busy.capacity(), row.busy.len(), "{id} busy");
            assert_eq!(row.turns.capacity(), row.turns.len(), "{id} turns");
            assert_eq!(
                row.cost_by_day.capacity(),
                row.cost_by_day.len(),
                "{id} cost days"
            );
            assert_eq!(row.waits.capacity(), row.waits.len(), "{id} waits");
            assert_eq!(
                row.answered.capacity(),
                row.answered.len(),
                "{id} answered waits"
            );
        }
    }

    /// Across machines: every machine's sessions, each under its machine's
    /// id, and the union's version.
    #[test]
    fn analytics_counts_every_machine_and_filters_by_one() {
        let now = model::now_ms();
        let alpha = dated("alpha", now, &[("a-old", 5 * DAY, 4)]);
        let bravo = dated("bravo", now, &[("b-new", 60 * MINUTE, 2)]);
        let core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        publish_core_native(&core);
        let home = body_of(&core.respond("GET", "/api/viewer/home", "", None));
        let week = analytics_of(&core, "range=7d");
        assert_eq!(week["version"], home["version"]);
        assert_eq!(week["current"]["started"], 2);
        assert_eq!(week["facets"]["machine"], json!(["a", "b"]));
        let machines: Vec<&Value> = week["breakdown"]["machine"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| &group["machine"])
            .collect();
        assert_eq!(machines, [&json!("a"), &json!("b")]);
        let bravo_only = analytics_of(&core, "range=7d&machine=b");
        assert_eq!(bravo_only["current"]["started"], 1);
        assert_eq!(bravo_only["current"]["agent_ms"], 2 * MINUTE);
        let day = analytics_of(&core, "range=24h");
        assert_eq!(day["current"]["started"], 1);
        assert!(day["sessions"].get(selected_id("a", "a-old")).is_none());
        // One machine through `with_machines` answers as it does alone.
        let alone = ViewerCore::with_machines(vec![("k".into(), alpha.options.clone())]);
        let single = ViewerCore::new(alpha.options.clone());
        publish_core_native(&alone);
        publish_core_native(&single);
        let (a, b) = (
            analytics_of(&alone, "range=30d"),
            analytics_of(&single, "range=30d"),
        );
        assert_eq!(a["current"], b["current"]);
        assert_ne!(
            a["version"], b["version"],
            "different authorized source keys"
        );
        assert_eq!(a["facets"]["machine"], json!(["k"]));
        assert_eq!(b["facets"]["machine"], json!(["local"]));
        assert_eq!(a["machine_labels"]["k"], "alpha");
        assert_eq!(b["machine_labels"]["local"], "alpha");
    }

    #[test]
    fn two_machines_with_one_hostname_stay_two_and_offline_facts_say_so() {
        let (first, second) = (machine("twin", "lane-1"), machine("twin", "lane-2"));
        let facts = second.root.join("facts.json");
        let mut recorded = crate::local_facts(&second.options)
            .unwrap()
            .offline(1_790_000_000_000);
        recorded.hostname = "twin".into();
        crate::write_facts(&facts, &recorded).unwrap();
        let mut offline = second.options.clone();
        offline.facts = Some(facts);
        let core = ViewerCore::with_machines(vec![
            ("one".into(), first.options.clone()),
            ("two".into(), offline),
        ]);
        let inventory = body_of(&core.respond("GET", "/api/session-sources", "", None));
        assert_eq!(
            inventory["items"],
            json!([
                {"source_key":"one", "label":"twin"},
                {"source_key":"two", "label":"twin~two"}
            ])
        );
        for view in core.machine_views() {
            publish_native(&view);
            assert_retired_routes_absent(&view);
        }
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        assert_eq!(reply.status, 200);
        let native = body_of(&reply);
        let machines = native["machines"].as_array().unwrap();
        assert_eq!(machines[0]["key"], "one");
        assert_eq!(machines[1]["key"], "two");
        assert_eq!(machines[0]["label"], "twin");
        assert_eq!(machines[1]["label"], "twin~two");
        assert_eq!(machines[0]["up"], true);
        assert_eq!(machines[1]["up"], false);
        assert_eq!(machines[1]["last"], 1_790_000_000_000_i64);
        let selected = core.respond(
            "GET",
            "/api/session-transcript",
            "machine=two&sid=lane-2&limit=100",
            None,
        );
        assert_eq!(selected.status, 200);
        assert_eq!(body_of(&selected)["identity"]["source_key"], "two");
        assert_eq!(
            core.respond("GET", "/machines/twin~two", "", None).status,
            200
        );
    }

    /// A receiver's copy of machine `name` under `dir/machines/`: one
    /// session, `sid`, and a `facts.json` naming `host` when given.
    fn receive(dir: &Path, name: &str, host: Option<&str>, sid: &str) -> PathBuf {
        let root = dir.join("machines").join(name);
        let path = root.join(format!("claude/projects/project/{sid}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lines = [
            json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":sid,"origin":{"kind":"human"},
                "message":{"role":"user","content":format!("work on {name}")}}),
            json!({"type":"assistant","timestamp":"2026-09-24T00:02:00Z","sessionId":sid,
                "message":{"role":"assistant","content":[{"type":"text","text":format!("done on {name}")}]}}),
        ];
        fs::write(&path, lines.map(|line| format!("{line}\n")).concat()).unwrap();
        if let Some(host) = host {
            crate::write_facts(
                &root.join("facts.json"),
                &crate::Facts {
                    version: crate::FACTS_VERSION,
                    hostname: host.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        root
    }

    /// Everything under `dir`, links not followed: each file's bytes, each
    /// link's target, each directory.
    fn contents(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    out.insert(path.clone(), b"/".to_vec());
                    walk(&path, out);
                } else if kind.is_file() {
                    out.insert(path.clone(), fs::read(&path).unwrap());
                } else {
                    let target = fs::read_link(&path).unwrap();
                    out.insert(path, target.to_string_lossy().as_bytes().to_vec());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, &mut out);
        out
    }

    /// Where received machines are read from, and the options they're read
    /// with: `local`'s window, and a cache under `receiver`, outside DIR.
    fn received(receiver: &Fixture, local: &Options) -> crate::ReceivedMachines {
        crate::ReceivedMachines::new(
            receiver.root.join("received"),
            &Options {
                cache: receiver.root.join("state/index.json"),
                ..local.clone()
            },
        )
    }

    fn native_home(core: &ViewerCore) -> Value {
        publish_core_native(core);
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        assert_eq!(reply.status, 200);
        body_of(&reply)
    }

    fn machine_ids(home: &Value) -> Vec<String> {
        home["machines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|machine| machine["label"].as_str().unwrap().to_owned())
            .collect()
    }

    fn home_owner(home: &Value, sid: &str) -> Option<String> {
        let source = home["recent"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["catalog_key"] == sid)?["source_key"]
            .as_str()?;
        home["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|machine| machine["key"] == source)?["label"]
            .as_str()
            .map(str::to_owned)
    }

    #[test]
    fn received_machines_join_this_one_each_tagged_and_answering_for_its_own() {
        let local = machine("home", "lane-home");
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        receive(&dir, "alpha", Some("alpha"), "lane-alpha");
        receive(&dir, "bravo", None, "lane-bravo");
        let delta = receive(&dir, "delta", None, "lane-delta");
        fs::write(delta.join("facts.json"), "{not json").unwrap();
        fs::write(dir.join("machines/stray"), "not a machine").unwrap();
        let before = contents(&dir);

        let core = ViewerCore::with_received(
            vec![(String::new(), local.options.clone())],
            received(&receiver, &local.options),
        );
        publish_core_native(&core);
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        assert_eq!(reply.status, 200);
        let model = body_of(&reply);
        assert_eq!(machine_ids(&model), ["home", "alpha", "bravo", "delta"]);
        let machines = model["machines"].as_array().unwrap();
        assert_eq!(machines[0]["up"], true);
        assert_eq!(machines[1]["up"], true);
        // No facts, or facts that can't be read: still shown, offline, by
        // the directory's name.
        for machine in &machines[2..] {
            assert_eq!(machine["up"], false, "{machine}");
            assert!(machine["last"].is_i64(), "{machine}");
            assert_eq!(machine["label"], machine["key"]);
        }
        for (sid, machine) in [
            ("lane-home", "home"),
            ("lane-alpha", "alpha"),
            ("lane-bravo", "bravo"),
            ("lane-delta", "delta"),
        ] {
            assert_eq!(home_owner(&model, sid).unwrap(), machine, "{sid}");
        }

        // The tree has every machine's roots, each with its machine.
        let tree = body_of(&core.respond("GET", "/api/tree", "", None));
        let roots: BTreeMap<&str, &str> = tree["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|root| {
                (
                    root["id"].as_str().unwrap(),
                    root["machine"].as_str().unwrap(),
                )
            })
            .collect();
        for (sid, machine) in [
            ("lane-home", "home"),
            ("lane-alpha", "alpha"),
            ("lane-bravo", "bravo"),
        ] {
            assert_eq!(roots.get(sid), Some(&machine), "{roots:?}");
        }

        // A received machine's transcripts open, from its own files.
        let tx = core.respond(
            "GET",
            "/api/session-transcript",
            "machine=bravo&sid=lane-bravo&limit=100",
            None,
        );
        assert_eq!(tx.status, 200);
        assert!(String::from_utf8_lossy(&tx.body).contains("done on bravo"));
        let transcript = core.respond(
            "GET",
            "/api/session-transcript",
            "machine=alpha&sid=lane-alpha&limit=100",
            None,
        );
        assert_eq!(transcript.status, 200);
        assert!(String::from_utf8_lossy(&transcript.body).contains("done on alpha"));
        for path in [
            "/session/alpha/lane-alpha",
            "/machines/bravo",
            "/machines/delta",
        ] {
            let reply = core.respond("GET", path, "", None);
            assert_eq!(
                (reply.status, reply.body.as_slice()),
                (200, PAGE.as_bytes()),
                "{path}"
            );
        }
        assert_eq!(core.respond("GET", "/s/claude/lane-alpha", "", None).status, 404);
        assert_eq!(core.respond("GET", "/machines/stray", "", None).status, 404);
        // DIR is only read.
        assert_eq!(contents(&dir), before);

        // A machine received while serving is served from the next request.
        let version = model["version"].as_str().unwrap().to_owned();
        receive(&dir, "charlie", Some("charlie"), "lane-charlie");
        let model = native_home(&core);
        assert_ne!(model["version"], version);
        assert_eq!(
            machine_ids(&model),
            ["home", "alpha", "bravo", "charlie", "delta"]
        );
        assert_eq!(home_owner(&model, "lane-charlie").unwrap(), "charlie");
        // Facts that arrive later bring the machine online.
        crate::write_facts(
            &dir.join("machines/bravo/facts.json"),
            &crate::Facts {
                version: crate::FACTS_VERSION,
                hostname: "bravo-host".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let model = native_home(&core);
        assert_eq!(
            machine_ids(&model),
            ["home", "alpha", "bravo-host", "charlie", "delta"]
        );
        assert_eq!(model["machines"][2]["up"], true);
        assert_eq!(home_owner(&model, "lane-bravo").unwrap(), "bravo-host");
        // A machine removed is no longer served.
        fs::remove_dir_all(dir.join("machines/charlie")).unwrap();
        let model = native_home(&core);
        assert_eq!(
            machine_ids(&model),
            ["home", "alpha", "bravo-host", "delta"]
        );
        assert!(home_owner(&model, "lane-charlie").is_none());

        // --no-local: the received machines only.
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &local.options));
        let model = native_home(&core);
        assert_eq!(machine_ids(&model), ["alpha", "bravo-host", "delta"]);
        assert!(home_owner(&model, "lane-home").is_none());
    }

    #[test]
    fn a_received_machine_with_this_machines_hostname_stays_distinct() {
        let local = machine("twin", "lane-here");
        let receiver = Fixture::new();
        receive(
            &receiver.root.join("received"),
            "twin",
            Some("twin"),
            "lane-there",
        );
        let core = ViewerCore::with_received(
            vec![(String::new(), local.options.clone())],
            received(&receiver, &local.options),
        );
        let model = native_home(&core);
        assert_eq!(machine_ids(&model), ["twin", "twin~twin"]);
        assert_eq!(home_owner(&model, "lane-here").unwrap(), "twin");
        assert_eq!(home_owner(&model, "lane-there").unwrap(), "twin~twin");
        assert_eq!(
            core.respond("GET", "/machines/twin~twin", "", None).status,
            200
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_a_machine_directory_is_ignored() {
        use std::os::unix::fs::symlink;
        // A home outside DIR, whose every session is a secret.
        let outside = machine("outside", "secret-claude");
        outside.codex("secret-codex", &[]);
        outside.write(
            "claude/sessions/77.json",
            &json!({"pid":77,"sessionId":"secret-pid","procStart":5,"status":"busy"}).to_string(),
        );
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");

        // A transcript, a project directory and a pid record that link out.
        let alpha = receive(&dir, "alpha", None, "lane-alpha");
        crate::write_facts(
            &alpha.join("facts.json"),
            &crate::Facts {
                version: crate::FACTS_VERSION,
                hostname: "alpha".into(),
                proc_starts: BTreeMap::from([(77, 5)]),
                ..Default::default()
            },
        )
        .unwrap();
        let projects = outside.root.join("claude/projects/project");
        symlink(
            projects.join("secret-claude.jsonl"),
            alpha.join("claude/projects/project/leak.jsonl"),
        )
        .unwrap();
        symlink(&projects, alpha.join("claude/projects/linked")).unwrap();
        fs::create_dir_all(alpha.join("claude/sessions")).unwrap();
        symlink(
            outside.root.join("claude/sessions/77.json"),
            alpha.join("claude/sessions/77.json"),
        )
        .unwrap();
        // An archive root behind a link must not expose external rollouts.
        fs::create_dir_all(alpha.join("codex")).unwrap();
        symlink(
            outside.root.join("codex/sessions"),
            alpha.join("codex/archived_sessions"),
        )
        .unwrap();
        // Homes behind a link, and facts behind one.
        let bravo = receive(&dir, "bravo", Some("bravo"), "lane-bravo");
        symlink(outside.root.join("codex"), bravo.join("codex")).unwrap();
        let charlie = dir.join("machines/charlie");
        fs::create_dir_all(&charlie).unwrap();
        symlink(outside.root.join("claude"), charlie.join("claude")).unwrap();
        let facts = outside.root.join("facts.json");
        crate::write_facts(
            &facts,
            &crate::Facts {
                version: crate::FACTS_VERSION,
                hostname: "outside".into(),
                ..Default::default()
            },
        )
        .unwrap();
        symlink(&facts, charlie.join("facts.json")).unwrap();
        // A machine directory that is a link.
        symlink(&outside.root, dir.join("machines/evil")).unwrap();
        let before = contents(&dir);

        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &outside.options));
        publish_core_native(&core);
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        assert_eq!(reply.status, 200);
        let text = String::from_utf8_lossy(&reply.body).into_owned();
        let model = body_of(&reply);
        assert_eq!(machine_ids(&model), ["alpha", "bravo", "charlie"]);
        assert_eq!(model["machines"][2]["up"], false);
        assert!(home_owner(&model, "lane-alpha").is_some());
        assert!(home_owner(&model, "lane-bravo").is_some());
        assert!(
            !text.contains("secret") && !text.contains("outside"),
            "{text}"
        );
        let tree = core.respond("GET", "/api/tree", "", None);
        assert_eq!(tree.status, 200);
        let tree = String::from_utf8_lossy(&tree.body).into_owned();
        assert!(tree.contains("lane-alpha"));
        assert!(
            !tree.contains("secret") && !tree.contains("outside"),
            "{tree}"
        );
        for machine in ["alpha", "bravo", "charlie"] {
            for sid in ["secret-claude", "secret-codex", "leak"] {
                let reply = core.respond(
                    "GET",
                    "/api/session-transcript",
                    &format!("machine={machine}&sid={sid}&limit=100"),
                    None,
                );
                assert_eq!(reply.status, 404, "{machine}:{sid}");
                assert!(!String::from_utf8_lossy(&reply.body).contains("SECRET"));
            }
        }
        assert_eq!(core.respond("GET", "/machines/evil", "", None).status, 404);
        assert_eq!(contents(&dir), before);
    }

    /// `facts` as machine `name`'s received facts.
    fn received_facts(dir: &Path, name: &str, facts: crate::Facts) -> PathBuf {
        let path = dir.join("machines").join(name).join("facts.json");
        crate::write_facts(&path, &facts).unwrap();
        path
    }

    fn facts_of(host: &str) -> crate::Facts {
        crate::Facts {
            version: crate::FACTS_VERSION,
            hostname: host.into(),
            ..Default::default()
        }
    }

    #[test]
    fn unchanged_heartbeats_reuse_native_received_evidence() {
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        for index in 0..6 {
            let name = format!("machine-{index}");
            receive(&dir, &name, Some(&name), &format!("lane-{index}"));
        }
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
        let home = native_home(&core);
        assert_eq!(home["machines"].as_array().unwrap().len(), 6);
        let builds = crate::model::BUILDS.with(|builds| builds.get());
        let started = Instant::now();
        for _ in 0..3 {
            for index in 0..6 {
                let name = format!("machine-{index}");
                received_facts(&dir, &name, facts_of(&name));
            }
            let home = native_home(&core);
            assert_eq!(home["machines"].as_array().unwrap().len(), 6);
        }
        eprintln!(
            "18 received heartbeats and three six-machine polls: {:?}",
            started.elapsed()
        );
        assert_eq!(
            crate::model::BUILDS.with(|builds| builds.get()),
            builds,
            "identical heartbeat contents never trigger another build"
        );
    }

    #[test]
    fn a_session_this_machine_has_is_left_out_of_a_received_one_not_refused() {
        // This machine pushed to itself, or its logs were copied.
        let local = machine("home", "same");
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        receive(&dir, "alpha", Some("alpha"), "same");
        receive(&dir, "alpha", None, "lane-alpha");
        let core = ViewerCore::with_received(
            vec![(String::new(), local.options.clone())],
            received(&receiver, &local.options),
        );
        publish_core_native(&core);
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let model = body_of(&reply);
        assert_eq!(machine_ids(&model), ["home", "alpha"]);
        assert_eq!(home_owner(&model, "same").unwrap(), "home");
        assert_eq!(home_owner(&model, "lane-alpha").unwrap(), "alpha");
        assert_eq!(
            core.session_machine_key("same").unwrap().as_deref(),
            Some("")
        );
        let transcript = core.respond(
            "GET",
            "/api/session-transcript",
            "machine=local&sid=same&limit=100",
            None,
        );
        assert_eq!(transcript.status, 200);
        assert!(String::from_utf8_lossy(&transcript.body).contains("done on home"));
        assert!(!String::from_utf8_lossy(&transcript.body).contains("done on alpha"));
        let selected: Value = body_of(&transcript);
        let mut query = crate::Query::new(local.options.clone());
        let details = query.call("get_session", &json!({"id":"same"})).unwrap();
        for turn in details["session"]["turns"].as_array().unwrap() {
            assert_eq!(turn["sid"], selected["identity"]["catalog_key"]);
        }
        let tree = body_of(&core.respond("GET", "/api/tree", "", None));
        let same: Vec<&Value> = tree["roots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|root| root["id"] == "same")
            .collect();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0]["machine"], "home");
        assert_eq!(core.respond("GET", "/s/claude/same", "", None).status, 404);
    }

    #[test]
    fn a_pid_record_over_the_cap_is_absent() {
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        let alpha = receive(&dir, "alpha", None, "lane-alpha");
        received_facts(
            &dir,
            "alpha",
            crate::Facts {
                proc_starts: BTreeMap::from([(77, 5), (78, 5)]),
                ..facts_of("alpha")
            },
        );
        let record = |sid: &str, pid: u32| {
            json!({"pid": pid, "sessionId": sid, "procStart": 5, "status": "busy"}).to_string()
        };
        fs::create_dir_all(alpha.join("claude/sessions")).unwrap();
        fs::write(
            alpha.join("claude/sessions/77.json"),
            record("small-pid", 77),
        )
        .unwrap();
        let mut big = record("big-pid", 78);
        big.push_str(&" ".repeat(crate::RECORD_MAX as usize));
        fs::write(alpha.join("claude/sessions/78.json"), big).unwrap();
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
        let tree =
            String::from_utf8_lossy(&core.respond("GET", "/api/tree", "", None).body).into_owned();
        assert!(tree.contains("small-pid"), "{tree}");
        assert!(!tree.contains("big-pid"), "{tree}");

        let at_most = receiver.write("record.json", &"x".repeat(1024));
        assert_eq!(
            crate::read_regular_at_most(&at_most, 1024).unwrap().len(),
            1024
        );
        assert_eq!(
            crate::read_regular_at_most(&at_most, 1023)
                .unwrap_err()
                .kind(),
            io::ErrorKind::FileTooLarge
        );
    }

    #[test]
    fn a_received_hostname_that_cant_name_a_machine_gives_way_to_its_directory() {
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        for (name, host) in [
            ("alpha", "alpha-host"),
            ("gamma", "bad~host"),
            ("delta", "has space"),
            ("echo", "\u{1b}[31mred"),
            ("foxtrot", ""),
        ] {
            receive(&dir, name, None, &format!("lane-{name}"));
            received_facts(&dir, name, facts_of(host));
        }
        let long = "x".repeat(254);
        receive(&dir, "golf", None, "lane-golf");
        received_facts(&dir, "golf", facts_of(&long));
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
        let model = native_home(&core);
        assert_eq!(
            machine_ids(&model),
            ["alpha-host", "delta", "echo", "foxtrot", "gamma", "golf"]
        );
        assert_eq!(home_owner(&model, "lane-gamma").unwrap(), "gamma");
        let text = serde_json::to_string(&model).unwrap();
        assert!(!text.contains("bad~host") && !text.contains("\u{1b}"));
    }

    #[test]
    fn a_machine_whose_facts_stopped_arriving_is_offline_since_they_were_written() {
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        let mut written = BTreeMap::new();
        for name in ["fresh", "stale"] {
            let root = receive(&dir, name, None, &format!("lane-{name}"));
            let facts = received_facts(
                &dir,
                name,
                crate::Facts {
                    proc_starts: BTreeMap::from([(77, 5)]),
                    ..facts_of(name)
                },
            );
            fs::create_dir_all(root.join("claude/sessions")).unwrap();
            fs::write(
                root.join("claude/sessions/77.json"),
                json!({"pid": 77, "sessionId": format!("lane-{name}"), "procStart": 5, "status": "busy"})
                    .to_string(),
            )
            .unwrap();
            written.insert(name, facts);
        }
        let ten_minutes_ago = std::time::SystemTime::now() - Duration::from_secs(600);
        fs::File::options()
            .append(true)
            .open(&written["stale"])
            .unwrap()
            .set_modified(ten_minutes_ago)
            .unwrap();
        let last = i64::try_from(
            ten_minutes_ago
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
        let model = native_home(&core);
        assert_eq!(machine_ids(&model), ["fresh", "stale"]);
        assert_eq!(model["machines"][0]["up"], true);
        assert_eq!(model["machines"][1]["up"], false);
        assert_eq!(model["machines"][1]["last"], last);
        // Its process no longer counts as running.
        let tree = body_of(&core.respond("GET", "/api/tree", "", None));
        let pid = |sid: &str| {
            tree["roots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|root| root["id"] == sid)
                .map(|root| root["pid"].clone())
                .unwrap()
        };
        assert_eq!(pid("lane-fresh"), 77);
        assert_eq!(pid("lane-stale"), Value::Null);
        // New facts bring it back.
        received_facts(
            &dir,
            "stale",
            crate::Facts {
                proc_starts: BTreeMap::from([(77, 5)]),
                ..facts_of("stale")
            },
        );
        let model = native_home(&core);
        assert_eq!(model["machines"][1]["up"], true);
    }

    #[test]
    fn only_machine_names_are_served() {
        let (longest, too_long) = ("x".repeat(63), "x".repeat(64));
        for name in ["a", "alpha-1", "0", longest.as_str()] {
            assert!(crate::is_machine_name(name), "{name}");
        }
        for name in [
            "",
            "Alpha",
            "a.b",
            "a~b",
            "a_b",
            "a b",
            ".hidden",
            "..",
            too_long.as_str(),
            "é",
        ] {
            assert!(!crate::is_machine_name(name), "{name}");
        }
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        for name in ["alpha", "Upper", "a~b", "a.b", ".hidden"] {
            receive(&dir, name, Some(name), &format!("lane-{}", name.len()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let odd = dir
                .join("machines")
                .join(std::ffi::OsStr::from_bytes(b"bad\xff"));
            fs::create_dir_all(odd.join("claude/projects/project")).unwrap();
            fs::copy(
                dir.join("machines/alpha/claude/projects/project/lane-5.jsonl"),
                odd.join("claude/projects/project/odd.jsonl"),
            )
            .unwrap();
        }
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
        publish_core_native(&core);
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        assert_eq!(reply.status, 200);
        let model = body_of(&reply);
        assert_eq!(machine_ids(&model), ["alpha"]);
        assert_eq!(
            model["recent"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["catalog_key"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["lane-5"]
        );
        // One received machine: the tree still names it.
        let tree = body_of(&core.respond("GET", "/api/tree", "", None));
        for root in tree["roots"].as_array().unwrap() {
            assert_eq!(root["machine"], "alpha", "{root}");
        }
    }

    #[test]
    fn machine_ids_stay_unique_whatever_the_hostnames() {
        let machines = [
            ("", machine("h", "s0")),
            ("a", machine("h~3", "s1")),
            ("b", machine("h~c", "s2")),
            ("c", machine("h", "s3")),
        ];
        let core = ViewerCore::with_machines(
            machines
                .iter()
                .map(|(key, fixture)| ((*key).to_owned(), fixture.options.clone()))
                .collect(),
        );
        publish_core_native(&core);
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(machine_ids(&body_of(&reply)), ["h", "h~3", "h~c", "h~3~2"]);
    }

    /// Appends an assistant line saying `text` to session `sid`'s log.
    fn say(fixture: &Fixture, sid: &str, second: usize, text: &str) {
        let path = fixture
            .root
            .join(format!("claude/projects/project/{sid}.jsonl"));
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"assistant","timestamp":format!("2026-09-24T00:03:{:02}Z", second % 60),"sessionId":sid,
                "message":{"role":"assistant","content":[{"type":"text","text":text}]}})
        )
        .unwrap();
    }

    fn native_api(view: &Arc<MachineView>, path: &str, query: &str) -> ViewerReply {
        ViewerCore::with_test_view(view.clone()).respond("GET", path, query, None)
    }

    fn version_of(reply: &ViewerReply) -> String {
        body_of(reply)["generation"].as_str().unwrap().to_owned()
    }

    /// Asks `answer` every 20 ms until it gives something, for at most 10 s.
    fn eventually<T>(what: &str, mut answer: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(found) = answer() {
                return found;
            }
            assert!(Instant::now() < deadline, "{what}: not within 10 s");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// What a read may take at most once a model is built, whatever a
    /// rebuild or a check is doing: it answers from a snapshot in memory.
    /// The property is structural (no read waits on `work`), so the bound
    /// is loose, for a busy runner.
    const AT_ONCE: Duration = Duration::from_secs(2);

    /// How long a read may take before it is taken for one that waits on a
    /// held build, which would never answer.
    const HUNG_AFTER: Duration = Duration::from_secs(5);

    /// Runs `read` on a thread of its own, and fails unless it answered
    /// within [`AT_ONCE`]. A read that waits on a held build fails the test
    /// after [`HUNG_AFTER`] rather than hanging it.
    fn at_once<T: Send + 'static>(what: &str, read: impl FnOnce() -> T + Send + 'static) -> T {
        let (sent, answer) = mpsc::channel();
        let asked = Instant::now();
        thread::spawn(move || {
            let _ = sent.send(read());
        });
        let answer = match answer.recv_timeout(HUNG_AFTER) {
            Ok(answer) => answer,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("{what}: no answer within {HUNG_AFTER:?}: the read waits on a build")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!("{what}: the read panicked"),
        };
        let took = asked.elapsed();
        assert!(took < AT_ONCE, "{what}: waited {took:?}");
        answer
    }

    /// `view`'s answer to a GET of `path?query`, [`at_once`].
    fn ask(what: &str, view: &Arc<MachineView>, path: &str, query: &str) -> ViewerReply {
        let (view, path, query) = (view.clone(), path.to_owned(), query.to_owned());
        at_once(what, move || {
            if path == "/api/sessions" || path.starts_with("/api/session-") {
                native_api(&view, &path, &query)
            } else {
                view.respond("GET", &path, &query, None)
            }
        })
    }

    /// Whether `view`'s transcript of `lane` holds `text`, [`at_once`].
    fn serves_at_once(what: &str, view: &Arc<MachineView>, text: &str) -> bool {
        String::from_utf8_lossy(
            &ask(what, view, "/api/session-transcript", "sid=lane&limit=100").body,
        )
        .contains(text)
    }

    /// Background refresh (#29): while a rebuild runs, every read answers
    /// at once from the model before it, with that model's `ETag`: the
    /// model, each form of `/api/tx` (a page, `after=`, `turn=`, errors),
    /// an attachment and the V1 tree. An id that snapshot lacks is the
    /// usual 404. The rebuilt model answers once it is done, and a poll
    /// never sees the version before it again.
    #[test]
    fn native_reads_during_observation_keep_cached_metadata_and_refuse_stale_bodies() {
        let fixture = lane_fixture();
        let (view, v1) = warm_background(&fixture);
        let before = native_selected(&view, "lane");
        let projection = before["projection"]["generation"]
            .as_str()
            .unwrap()
            .to_owned();
        let errors = before["projection"]["errors_generation"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(view.respond("GET", "/api/tree", "", None).status, 200);
        let reads = view.hooks.refreshes_first.load(Ordering::SeqCst);
        let (starts, release) = view.hooks.hold();
        say(&fixture, "lane", 1, "written while the rebuild waits");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the native observer rebuilds the changed log");
        let during = ask(
            "cached Catalog during observation",
            &view,
            "/api/sessions",
            "",
        );
        assert_eq!(during.status, 200);
        assert_eq!(body_of(&during)["generation"], v1);
        for after in [0, 1] {
            let reply = ask(
                "qualified selected prefix during observation",
                &view,
                "/api/session-transcript",
                &format!(
                    "sid=lane&after={after}&limit=1&generation={projection}&errors_generation={errors}"
                ),
            );
            assert_eq!(
                reply.status, 409,
                "a rejected local generation never becomes blank accepted content"
            );
            assert_eq!(body_of(&reply)["error"]["retryable"], true);
            assert_eq!(body_of(&reply)["resynchronize"], true);
            assert!(!String::from_utf8_lossy(&reply.body).contains("while the rebuild waits"));
        }
        let missing = ask(
            "missing selected body during observation",
            &view,
            "/api/session-transcript",
            "sid=nobody&limit=100",
        );
        assert_eq!(missing.status, 404);
        let attachment = ask(
            "qualified native attachment during observation",
            &view,
            "/api/session-entry",
            &format!("sid=lane&after=0&limit=1&generation={projection}&part=attachment&image=0"),
        );
        assert_eq!(
            attachment.status, 409,
            "reject the stale selected source before image expansion"
        );
        let tree = ask(
            "cached native tree during observation",
            &view,
            "/api/tree",
            "",
        );
        assert_eq!(tree.status, 200);
        assert_eq!(view.hooks.refreshes_first.load(Ordering::SeqCst), reads);
        drop(release);
        let after = eventually("the next qualified native generation", || {
            let reply = native_api(&view, "/api/sessions", "");
            (reply.status == 200 && body_of(&reply)["generation"] != v1).then_some(reply)
        });
        let v2 = version_of(&after);
        assert_ne!(v1, v2);
        assert!(serves(&view, "while the rebuild waits"));
        for _ in 0..5 {
            let reply = native_api(&view, "/api/sessions", "");
            assert_eq!(reply.status, 200);
            assert_eq!(body_of(&reply)["generation"], v2, "a poll went backward");
        }
        assert_retired_routes_absent(&view);
        view.close();
    }

    /// Background refresh (#29): reads with no change rebuild nothing, and
    /// the changes of one second after a rebuild are one rebuild.
    #[test]
    fn changes_within_a_second_are_one_rebuild_and_none_without_a_change() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        view.set_refresh(Refresh::Background);
        let v1 = warm(&view);
        assert_eq!(view.hooks.builds(), 1);
        let idle = |version: &str| {
            let until = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < until {
                let poll = native_api(&view, "/api/sessions", "");
                assert_eq!(poll.status, 200);
                assert_eq!(body_of(&poll)["generation"], version);
                thread::sleep(Duration::from_millis(50));
            }
        };
        idle(&v1);
        assert_eq!(view.hooks.builds(), 1, "no change, no rebuild");

        // One line: one rebuild, seen by a later read.
        say(&fixture, "lane", 0, "line 0");
        let v2 = eventually("the first line", || {
            let reply = native_api(&view, "/api/sessions", "");
            (reply.status == 200 && body_of(&reply)["generation"] != v1).then(|| version_of(&reply))
        });
        assert_eq!(view.hooks.builds(), 2);
        // Nineteen more, each its own change, within the second after that
        // rebuild: one rebuild takes them all.
        for number in 1..20 {
            say(&fixture, "lane", number, &format!("line {number}"));
            thread::sleep(Duration::from_millis(10));
        }
        let page = eventually("the nineteen lines", || {
            let page = native_api(&view, "/api/session-transcript", "sid=lane&limit=100");
            let done = String::from_utf8_lossy(&page.body).contains("\"line 19\"");
            done.then_some(page)
        });
        let text = String::from_utf8_lossy(&page.body);
        assert!((1..20).all(|number| text.contains(&format!("\"line {number}\""))));
        assert_eq!(
            view.hooks.builds(),
            3,
            "nineteen changes in a second, one rebuild"
        );
        let v3 = version_of(&native_api(&view, "/api/sessions", ""));
        assert_ne!(v3, v2);
        idle(&v3);
        assert_eq!(view.hooks.builds(), 3, "no change, no rebuild");
        view.close();
    }

    /// Background refresh (#29): many threads read one shared core while
    /// two machines' logs grow. Every answer is one whole snapshot (its
    /// `ETag` is its version, and a transcript page never has fewer entries
    /// than the model read before it counted), no thread sees an older
    /// snapshot after a newer one, and the last lines are served.
    #[test]
    fn concurrent_native_reads_keep_whole_generation_fenced_pages_while_logs_grow() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let mut core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        core.set_refresh(Refresh::Background);
        let core = Arc::new(core);
        publish_core_native(&core);
        let writing = Arc::new(AtomicBool::new(true));
        let reader_success: Arc<[AtomicBool; 8]> =
            Arc::new(std::array::from_fn(|_| AtomicBool::new(false)));
        let readers:Vec<_>=(0..8).map(|reader| {
            let(core,writing,reader_success)=(core.clone(),writing.clone(),reader_success.clone());
            thread::spawn(move|| {
                let mut seen=[0_u64;2];
                let mut reads=0;
                let mut reported=false;
                while writing.load(Ordering::SeqCst)||reads<10 {
                    for (at,(source,sid)) in [("a","lane-a"),("b","lane-b")].into_iter().enumerate() {
                        let catalog=core.respond("GET","/api/sessions",&format!("machine={source}&sid={sid}"),None);
                        assert_eq!(catalog.status,200);
                        let snapshot=body_of(&catalog);
                        let descriptor=&snapshot["items"][0]["transcript"];
                        let slots=descriptor["total"].as_u64().unwrap();
                        assert!(slots>=seen[at],"{sid}: Catalog moved backward");
                        let generation=descriptor["generation"].as_str().unwrap();
                        let errors_generation=descriptor["errors_generation"].as_str().unwrap();
                        let page=core.respond("GET","/api/session-transcript",&format!("machine={source}&sid={sid}&limit=100&generation={generation}&errors_generation={errors_generation}"),None);
                        if page.status==409 {
                            assert_eq!(body_of(&page)["error"]["retryable"],true);
                            continue;
                        }
                        assert_eq!(page.status,200);
                        let page=body_of(&page);
                        assert_eq!(page["identity"]["source_key"],source);
                        assert_eq!(page["identity"]["catalog_key"],sid);
                        assert_eq!(page["projection"]["generation"],generation);
                        assert_eq!(page["projection"]["errors_generation"],errors_generation);
                        let total=page["projection"]["total"].as_u64().unwrap();
                        assert_eq!(total,slots,"one complete selected generation");
                        assert!(total>=seen[at]);
                        assert_eq!(page["entries"].as_array().unwrap().len() as u64,total);
                        seen[at]=total;
                    }
                    if !reported && seen.iter().all(|total| *total>0) {
                        reader_success[reader].store(true,Ordering::SeqCst);
                        reported=true;
                    }
                    reads+=1;
                }
                seen
            })
        }).collect();
        for number in 0..40 {
            say(&alpha, "lane-a", number, &format!("line {number}"));
            say(&bravo, "lane-b", number, &format!("line {number}"));
            thread::sleep(Duration::from_millis(25));
        }
        eventually("last lines in both qualified sources", || {
            (tx_has(&core, "a", "lane-a", "line 39")
                && tx_has(&core, "b", "lane-b", "line 39")
                && reader_success
                    .iter()
                    .all(|success| success.load(Ordering::SeqCst)))
            .then_some(())
        });
        writing.store(false, Ordering::SeqCst);
        for reader in readers {
            assert!(
                reader
                    .join()
                    .expect("a native reader failed")
                    .iter()
                    .all(|total| *total > 0)
            );
        }
        core.close();
        assert_eq!(
            core.respond(
                "GET",
                "/api/session-transcript",
                "machine=a&sid=lane-a",
                None
            )
            .status,
            404
        );
        assert_eq!(
            core.respond("GET", "/api/sessions", "machine=b", None)
                .status,
            404
        );
    }

    /// A background view whose first model is built and whose refresher has
    /// looked for over a second: past the spacing after that first build.
    fn warm_background(fixture: &Fixture) -> (Arc<MachineView>, String) {
        let view = fixture.viewer();
        view.set_refresh(Refresh::Background);
        let version = warm(&view);
        (view, version)
    }

    /// A view over `fixture` in `refresh`, checked by `pool`'s workers.
    fn pooled(fixture: &Fixture, refresh: Refresh, pool: &Arc<RefreshPool>) -> Arc<MachineView> {
        let view = MachineView::new(fixture.options.clone());
        view.set_pool(pool.clone());
        view.set_refresh(refresh);
        view
    }

    /// Builds `view`'s first model and reads it for over a second, past
    /// the spacing after that first build; the model's version.
    fn warm(view: &Arc<MachineView>) -> String {
        let version = eventually("first qualified native Catalog", || {
            let reply = native_api(view, "/api/sessions", "");
            let page: Value = serde_json::from_slice(&reply.body).ok()?;
            (reply.status == 200 && page["completeness"]["state"] == "complete")
                .then(|| page["generation"].as_str().unwrap().to_owned())
        });
        let until = Instant::now() + REBUILD_SPACING + Duration::from_millis(200);
        while Instant::now() < until {
            assert_eq!(native_api(view, "/api/sessions", "").status, 200);
            thread::sleep(Duration::from_millis(50));
        }
        version
    }

    fn serves(view: &Arc<MachineView>, text: &str) -> bool {
        let page = native_api(view, "/api/session-transcript", "sid=lane&limit=100");
        page.status == 200 && String::from_utf8_lossy(&page.body).contains(text)
    }

    fn native_snapshot(view: &MachineView) -> Option<BTreeMap<PathBuf, Option<Stamp>>> {
        let work = match view.evidence.work.try_lock() {
            Ok(work) => work,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        work.catalog_snapshot
            .as_ref()
            .map(|snapshot| snapshot.watched.clone())
    }

    /// Observe a complete committed publication without requesting a refresh.
    /// A busy work lock is not a publication identity, and changed watcher
    /// metadata alone does not prove that the replacement worker published.
    fn native_complete_publication(view: &MachineView) -> Option<String> {
        let work = match view.evidence.work.try_lock() {
            Ok(work) => work,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if view.evidence.catalog_pending(&work) {
            return None;
        }
        let cache = work.events.as_ref()?;
        if !cache.qualified_catalog().ok()? {
            return None;
        }
        cache.session_catalog_generation()
    }

    /// The documented bound: a line is served within the rebuild spacing
    /// and one check of when it lands, plus a build (given 750 ms here),
    /// even when it lands just after a rebuild.
    #[test]
    fn a_change_is_served_within_the_staleness_bound() {
        let fixture = lane_fixture();
        let (view, _) = warm_background(&fixture);
        let bound = REBUILD_SPACING + CHECK_EVERY + Duration::from_millis(750);
        for (second, text) in [(1, "first of two"), (2, "right after a rebuild")] {
            let written = Instant::now();
            say(&fixture, "lane", second, text);
            eventually(text, || serves(&view, text).then_some(()));
            let took = written.elapsed();
            assert!(
                took < bound,
                "{text}: served after {took:?}, over {bound:?}"
            );
        }
        view.close();
    }

    /// After IDLE_AFTER without a read the view is no longer checked. The
    /// next read, its logs changed meanwhile, answers at once from the last
    /// model and queues the view's checks again; the rebuild that check
    /// runs, held here, never holds up a read, and once it is done the
    /// change is served. (Before, that read rebuilt the view itself and
    /// waited for the build.)
    #[test]
    fn after_an_idle_spell_the_first_read_answers_at_once_and_queues_a_check() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        view.set_refresh(Refresh::Background);
        lock(&view.live.state).idle_after = Duration::from_millis(300);
        let v1 = warm(&view);
        let running = || lock(&view.live.state).slot != Slot::Idle;
        assert!(running());
        eventually("the checks stop when idle", || (!running()).then_some(()));
        thread::sleep(REBUILD_SPACING + Duration::from_millis(100));
        let reads = view.hooks.refreshes_first.load(Ordering::SeqCst);
        let (starts, release) = view.hooks.hold();
        say(&fixture, "lane", 1, "written while idle");
        let first = ask(
            "the first read after an idle spell",
            &view,
            "/api/sessions",
            "",
        );
        assert_eq!(first.status, 200, "the model before the idle spell answers");
        assert!(running(), "the read queued the view's checks again");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the queued check rebuilds the view");
        for _ in 0..3 {
            let during = ask(
                "a read during the check's rebuild",
                &view,
                "/api/sessions",
                "",
            );
            assert_eq!(during.status, 200);
            let page = serves_at_once(
                "a page during the check's rebuild",
                &view,
                "written while idle",
            );
            assert!(!page, "served before the rebuild ended");
        }
        assert_eq!(
            view.hooks.refreshes_first.load(Ordering::SeqCst),
            reads,
            "a read refreshed first"
        );
        drop(release);
        eventually("the change, once rebuilt", || {
            serves(&view, "written while idle").then_some(())
        });
        assert_ne!(version_of(&native_api(&view, "/api/sessions", "")), v1);
        view.close();
    }

    /// A check that panics mid-build ends its worker, and the pool starts
    /// another in its place: the view's last model stays served, its next
    /// check is queued, and the new worker (no read) rebuilds it.
    #[test]
    fn a_worker_that_panics_is_replaced() {
        let fixture = lane_fixture();
        let pool = RefreshPool::new(1);
        let view = pooled(&fixture, Refresh::Background, &pool);
        let _v1 = warm(&view);
        // A build counts when it starts. Only a complete committed generation
        // proves publication; a transient busy lock cannot be the baseline.
        let shown = || native_complete_publication(&view);
        let before = eventually("initial complete native publication", shown);
        let started = pool.peak_and_started().1;
        let armed = Arc::new(AtomicBool::new(true));
        let trigger = armed.clone();
        *lock(&view.hooks.building) = Some(Arc::new(move || {
            if trigger.swap(false, Ordering::SeqCst) {
                panic!("a rebuild panicked on purpose");
            }
        }));
        say(&fixture, "lane", 1, "after the panic");
        eventually("the panic, and a worker in its place", || {
            (!armed.load(Ordering::SeqCst) && pool.peak_and_started().1 > started).then_some(())
        });
        let during = native_api(&view, "/api/sessions", "");
        assert_eq!(during.status, 200);
        // No read from here until the new worker has rebuilt it.
        eventually("a rebuild by the new worker", || {
            // The replacement may finish before the preceding read returns.
            // Wait for its published result, not an additional future build.
            shown().filter(|generation| generation != &before).map(drop)
        });
        assert!(serves(&view, "after the panic"));
        assert!(pool.threads() <= 1);
        assert_eq!(
            pool.peak_and_started().0,
            1,
            "never more threads than its size"
        );
        view.close();
    }

    #[test]
    fn qualified_catalog_interest_avoids_cold_work_and_observes_invalidation() {
        let fixture = lane_fixture();
        let publisher = MachineView::new(fixture.options.clone());
        publish_native(&publisher);
        publisher.close();
        let generation = || {
            let page = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
            assert_eq!(page.status, 200);
            let body: Value = serde_json::from_slice(&page.body).unwrap();
            body["generation"].clone()
        };
        let before = generation();
        let pool = RefreshPool::new(1);
        let view = pooled(&fixture, Refresh::OnInvalidate, &pool);
        for _ in 0..8 {
            view.note_catalog_interest().unwrap();
            assert_eq!(generation(), before);
        }
        assert_eq!(view.hooks.builds(), 0);
        assert_eq!(pool.entries_for(&view), 0);
        assert!(lock(&view.evidence.work).events.is_none());
        say(&fixture, "lane", 1, "after qualified interest");
        view.invalidate();
        eventually("a watched source changes after invalidation", || {
            (generation() != before).then_some(())
        });
        assert!(view.hooks.builds() > 0);
        assert!(pool.threads() <= 1);
        assert!(pool.entries_for(&view) <= 1);
        view.close();
    }

    #[test]
    fn catalog_demand_refreshes_sources_without_showing_a_compatibility_model() {
        for (mode, scan_window) in [
            (Refresh::Background, false),
            (Refresh::OnRead, false),
            (Refresh::OnInvalidate, true),
            (Refresh::OnRead, true),
        ] {
            let mut fixture = lane_fixture();
            fixture.options.scan_window = scan_window;
            let pool = RefreshPool::new(1);
            let view = pooled(&fixture, mode, &pool);
            view.note_catalog_read().unwrap();
            let generation = || {
                let reply = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
                let body: Value = serde_json::from_slice(&reply.body).ok()?;
                (reply.status == 200 && body["completeness"]["state"] == "complete")
                    .then(|| body["generation"].clone())
            };
            let before = eventually("catalog-only complete observation", generation);
            assert_retired_routes_absent(&view);
            say(&fixture, "lane", 1, "catalog-only append");
            view.invalidate();
            view.note_catalog_read().unwrap();
            eventually("catalog-only appended generation", || {
                generation().filter(|next| next != &before)
            });
            let reply = crate::session_transcript_range(
                &fixture.options,
                "fixture",
                "sid=lane&after=0&limit=100",
                None,
            );
            assert_eq!(reply.status, 200);
            assert!(String::from_utf8_lossy(&reply.body).contains("catalog-only append"));
            assert_retired_routes_absent(&view);
            assert_retired_routes_absent(&view);
            view.close();
        }
    }

    #[test]
    fn a_changed_catalog_child_reuses_only_its_relationship_component() {
        for change in ["append", "sidecar"] {
            for unrelated in [0, 256] {
                let fixture = Fixture::new();
                fixture.claude("parent", &[
                    json!({"type":"user","sessionId":"parent","uuid":"parent-u","origin":{"kind":"human"},"cwd":"/tmp/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"parent request"}}),
                    json!({"type":"assistant","sessionId":"parent","uuid":"parent-task","timestamp":"2026-10-01T00:01:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"task-child","name":"Task","input":{"prompt":"inspect child"}}]}}),
                ]);
                let child = fixture.write("claude/projects/project/parent/subagents/agent-child.jsonl", &(json!({"type":"user","sessionId":"parent","agentId":"child","isSidechain":true,"uuid":"child-u","cwd":"/tmp/project","timestamp":"2026-10-01T00:02:00Z","message":{"role":"user","content":"inspect child"}}).to_string()+"\n"));
                fixture.write(
                    "claude/projects/project/parent/subagents/agent-child.meta.json",
                    r#"{"toolUseId":"task-child","description":"Child inspection"}"#,
                );
                for n in 0..unrelated {
                    let id = format!("unrelated-{n:04}");
                    fixture.claude(&id, &[json!({"type":"user","sessionId":id,"uuid":id,"origin":{"kind":"human"},"cwd":"/tmp/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"unrelated prompt"}})]);
                }
                let mut cache = EventCache::open_scoped(&fixture.options.cache).unwrap();
                model::observe_catalog(
                    &fixture.options,
                    &mut cache,
                    &mut false,
                    &mut model::Texts::default(),
                    model::now_ms(),
                )
                .unwrap();
                let snapshot =
                    Snapshot::capture_pids(&fixture.options, BTreeSet::new(), cache.paths());
                if change == "append" {
                    writeln!(fs::OpenOptions::new().append(true).open(&child).unwrap(), "{}", json!({"type":"assistant","sessionId":"parent","agentId":"child","isSidechain":true,"uuid":"child-a","timestamp":"2026-10-01T00:03:00Z","message":{"role":"assistant","content":[{"type":"text","text":"bounded child update"}]}})).unwrap();
                } else {
                    fixture.write(
                        "claude/projects/project/parent/subagents/agent-child.meta.json",
                        r#"{"toolUseId":"task-child","description":"Updated child inspection","model":"updated-model"}"#,
                    );
                }
                let mut work = Work::default();
                work.events = Some(EventCache::open_scoped(&fixture.options.cache).unwrap());
                work.catalog_snapshot = Some(snapshot);
                let view = fixture.viewer();
                let before = crate::events::FILE_READS.with(std::cell::Cell::get);
                assert!(
                    view.refresh_catalog_locked(&mut work, model::now_ms())
                        .unwrap()
                );
                assert_eq!(
                    crate::events::FILE_READS.with(std::cell::Cell::get) - before,
                    2,
                    "unrelated={unrelated}"
                );
                let reply = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
                let page: Value = serde_json::from_slice(&reply.body).unwrap();
                assert_eq!(page["completeness"]["state"], "complete");
                if change == "sidecar" {
                    let child = page["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|row| row["key"] == "child")
                        .unwrap();
                    assert_eq!(child["name"], "Updated child inspection");
                    assert_eq!(child["model"], "updated-model");
                }
                if change == "sidecar" {
                    let cache = work.events.as_mut().unwrap();
                    let inputs = [
                        crate::native_evidence::catalog_input(
                            &fixture.root.join("claude/projects/project/parent.jsonl"),
                            &fixture.options,
                        )
                        .unwrap(),
                        crate::native_evidence::catalog_input(&child, &fixture.options).unwrap(),
                    ];
                    let mut prepared =
                        model::prepare_sources(&fixture.options, cache, &inputs, model::now_ms())
                            .unwrap();
                    let generation = cache.session_catalog_generation();
                    prepared
                        .rows
                        .iter_mut()
                        .find(|row| row.key == "child")
                        .unwrap()
                        .sources[0]
                        .native_role
                        .as_mut()
                        .unwrap()["Agent"]["parent"] = json!("different-parent");
                    assert_eq!(cache.publish_closed_catalog(&prepared).unwrap(), None);
                    assert_eq!(cache.session_catalog_generation(), generation);
                }
                assert_retired_routes_absent(&view);
                view.close();
            }
        }
    }

    #[test]
    fn a_complete_bounded_cold_catalog_derives_its_sources_once() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        let mut work = Work::default();
        let before = crate::model::BUILDS.with(std::cell::Cell::get);
        while !view
            .refresh_catalog_locked(&mut work, model::now_ms())
            .unwrap()
        {}
        assert_eq!(crate::model::BUILDS.with(std::cell::Cell::get) - before, 1);
        let reply = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
        let page: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(page["completeness"]["state"], "complete");
        assert!(work.catalog_snapshot.is_some());
        assert_retired_routes_absent(&view);
        view.close();
    }

    #[test]
    fn a_bounded_cold_catalog_cannot_acknowledge_a_competing_publication() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        let options = fixture.options.clone();
        *lock(&view.hooks.building) = Some(Arc::new(move || {
            let mut other = EventCache::open_scoped(&options.cache).unwrap();
            model::observe_catalog(
                &options,
                &mut other,
                &mut false,
                &mut model::Texts::default(),
                model::now_ms(),
            )
            .unwrap();
        }));
        let mut work = Work::default();
        let error = view
            .refresh_catalog_locked(&mut work, model::now_ms())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(work.catalog_snapshot.is_none());
        let connection =
            rusqlite::Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let published: String = connection
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut other = EventCache::open_scoped(&fixture.options.cache).unwrap();
        model::observe_catalog(
            &fixture.options,
            &mut other,
            &mut false,
            &mut model::Texts::default(),
            model::now_ms(),
        )
        .unwrap();
        assert_eq!(
            other.session_catalog_generation().as_deref(),
            Some(published.as_str())
        );
        view.close();
    }

    #[test]
    fn a_changed_catalog_parent_reloads_only_old_and_new_relationship_components() {
        for unrelated in [0, 256] {
            let fixture = Fixture::new();
            let record = |id: &str, parent: Option<&str>| {
                json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":id,"parent_thread_id":parent,"thread_source":if parent.is_some() {"subagent"} else {"cli"},"cwd":"/tmp/project"}}).to_string() + "\n"
            };
            for id in ["old-parent", "new-parent"] {
                fixture.write(
                    &format!("codex/sessions/2026/10/01/rollout-{id}.jsonl"),
                    &record(id, None),
                );
            }
            let child = fixture.write(
                "codex/sessions/2026/10/01/rollout-child.jsonl",
                &record("child", Some("old-parent")),
            );
            for n in 0..unrelated {
                let id = format!("unrelated-{n:04}");
                fixture.claude(&id, &[json!({"type":"user","sessionId":id,"uuid":id,"origin":{"kind":"human"},"cwd":"/tmp/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"unrelated"}})]);
            }
            let mut cache = EventCache::open_scoped(&fixture.options.cache).unwrap();
            model::observe_catalog(
                &fixture.options,
                &mut cache,
                &mut false,
                &mut model::Texts::default(),
                model::now_ms(),
            )
            .unwrap();
            let snapshot = Snapshot::capture_pids(&fixture.options, BTreeSet::new(), cache.paths());
            fs::write(&child, record("child", Some("new-parent"))).unwrap();
            let mut work = Work::default();
            work.events = Some(EventCache::open_scoped(&fixture.options.cache).unwrap());
            work.catalog_snapshot = Some(snapshot);
            let view = fixture.viewer();
            let before = crate::events::FILE_READS.with(std::cell::Cell::get);
            assert!(
                view.refresh_catalog_locked(&mut work, model::now_ms())
                    .unwrap()
            );
            let reads = crate::events::FILE_READS.with(std::cell::Cell::get) - before;
            assert!(
                reads <= 3,
                "unrelated={unrelated}: parent rewrite loaded {reads} indexes"
            );
            let catalog =
                rusqlite::Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
            let parent: String = catalog.query_row("SELECT parent_key FROM session_catalog WHERE session_key='child' AND lifecycle='current'", [], |row| row.get(0)).unwrap();
            assert_eq!(parent, "new-parent");
            let page = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
            assert_eq!(
                serde_json::from_slice::<Value>(&page.body).unwrap()["completeness"]["state"],
                "complete"
            );
            assert_retired_routes_absent(&view);
            // The previous parent is required even when the new parent and
            // child alone can derive a valid-looking replacement relationship.
            fs::write(&child, record("child", Some("old-parent"))).unwrap();
            let input = |id: &str| crate::inputs::Input {
                root: crate::inputs::InputRoot::Codex,
                path: format!("sessions/2026/10/01/rollout-{id}.jsonl"),
            };
            let cache = work.events.as_mut().unwrap();
            let omitted = model::prepare_sources(
                &fixture.options,
                cache,
                &[input("child"), input("old-parent")],
                model::now_ms(),
            )
            .unwrap();
            assert_eq!(
                cache.publish_closed_catalog(&omitted).unwrap(),
                None,
                "an omitted old parent qualified a closed publication"
            );
            let prepared = model::prepare_sources(
                &fixture.options,
                cache,
                &[input("child"), input("old-parent"), input("new-parent")],
                model::now_ms(),
            )
            .unwrap();
            let mut competing = EventCache::open_scoped(&fixture.options.cache).unwrap();
            model::observe_catalog(
                &fixture.options,
                &mut competing,
                &mut false,
                &mut model::Texts::default(),
                model::now_ms(),
            )
            .unwrap();
            assert_eq!(
                cache.publish_closed_catalog(&prepared).unwrap(),
                Some(crate::events::Outcome::Conflict),
                "a competing generation was overwritten"
            );
            view.close();
        }
    }

    #[test]
    fn a_new_isolated_catalog_source_does_not_reload_unrelated_event_indexes() {
        for unrelated in [0, 256] {
            let fixture = Fixture::new();
            for n in 0..unrelated {
                let id = format!("existing-{n:04}");
                fixture.claude(&id, &[json!({"type":"user","sessionId":id,"uuid":id,"origin":{"kind":"human"},"cwd":"/tmp/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"existing prompt"}})]);
            }
            let mut cache = EventCache::open_scoped(&fixture.options.cache).unwrap();
            model::observe_catalog(
                &fixture.options,
                &mut cache,
                &mut false,
                &mut model::Texts::default(),
                model::now_ms(),
            )
            .unwrap();
            let snapshot = Snapshot::capture_pids(&fixture.options, BTreeSet::new(), cache.paths());
            fixture.claude("new-isolated", &[json!({"type":"user","sessionId":"new-isolated","uuid":"new-isolated","origin":{"kind":"human"},"cwd":"/tmp/project","timestamp":"2026-10-02T00:00:00Z","message":{"role":"user","content":"new prompt"}})]);
            let mut work = Work::default();
            work.events = Some(EventCache::open_scoped(&fixture.options.cache).unwrap());
            work.catalog_snapshot = Some(snapshot);
            let view = fixture.viewer();
            let before = crate::events::FILE_READS.with(std::cell::Cell::get);
            assert!(
                view.refresh_catalog_locked(&mut work, model::now_ms())
                    .unwrap()
            );
            let reads = crate::events::FILE_READS.with(std::cell::Cell::get) - before;
            assert!(
                reads <= 1,
                "unrelated={unrelated}: adding one isolated source loaded {reads} event indexes"
            );
            let reply = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
            let page: Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(page["completeness"]["state"], "complete");
            let catalog =
                rusqlite::Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
            let count: i64 = catalog
                .query_row(
                    "SELECT COUNT(*) FROM session_catalog WHERE lifecycle='current'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, i64::from(unrelated) + 1);
            assert!(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["key"] == "new-isolated")
            );
            assert_retired_routes_absent(&view);
            view.close();
        }
    }

    #[test]
    fn a_changed_isolated_catalog_source_does_not_reload_unrelated_event_indexes() {
        for unrelated in [0, 256] {
            let fixture = Fixture::new();
            for n in 0..=unrelated {
                let id = format!("isolated-{n:04}");
                fixture.claude(&id, &[json!({"type":"user","sessionId":id,"uuid":id,"origin":{"kind":"human"},"cwd":"/tmp/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"initial prompt"}})]);
            }
            let mut cache = EventCache::open_scoped(&fixture.options.cache).unwrap();
            model::observe_catalog(
                &fixture.options,
                &mut cache,
                &mut false,
                &mut model::Texts::default(),
                model::now_ms(),
            )
            .unwrap();
            let snapshot = Snapshot::capture_pids(&fixture.options, BTreeSet::new(), cache.paths());
            say(&fixture, "isolated-0000", 1, "one changed source");
            let mut work = Work::default();
            work.events = Some(EventCache::open_scoped(&fixture.options.cache).unwrap());
            work.catalog_snapshot = Some(snapshot);
            let view = fixture.viewer();
            let before = crate::events::FILE_READS.with(std::cell::Cell::get);
            assert!(
                view.refresh_catalog_locked(&mut work, model::now_ms())
                    .unwrap()
            );
            assert_eq!(
                crate::events::FILE_READS.with(std::cell::Cell::get) - before,
                1,
                "unrelated={unrelated}: changed-source publication loads only its own event rows"
            );
            let reply = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
            let page: Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(page["completeness"]["state"], "complete");
            assert_retired_routes_absent(&view);
            view.close();
        }
    }

    #[test]
    #[cfg(unix)]
    fn catalog_refresh_detects_same_size_rewrites_with_restored_modification_time() {
        let fixture = Fixture::new();
        let record = json!({"type":"user","sessionId":"rewrite","uuid":"rewrite","cwd":"/tmp/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"old prompt"}});
        let path = fixture.claude("rewrite", &[record]);
        let pool = RefreshPool::new(1);
        let view = pooled(&fixture, Refresh::OnInvalidate, &pool);
        let generation = || {
            let reply = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
            let body: Value = serde_json::from_slice(&reply.body).ok()?;
            (reply.status == 200 && body["completeness"]["state"] == "complete")
                .then(|| body["generation"].clone())
        };
        view.note_catalog_read().unwrap();
        let before = eventually("complete Catalog before rewrite", generation);
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let old = fs::read_to_string(&path).unwrap();
        let replacement = old.replace("old prompt", "new prompt");
        assert_eq!(old.len(), replacement.len());
        fs::write(&path, replacement).unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        view.invalidate();
        view.note_catalog_read().unwrap();
        eventually(
            "Catalog observes an equal-size rewrite with restored mtime",
            || generation().filter(|next| next != &before),
        );
        let reply = crate::session_transcript_range(
            &fixture.options,
            "fixture",
            "sid=rewrite&after=0&limit=100",
            None,
        );
        assert_eq!(reply.status, 200);
        assert!(String::from_utf8_lossy(&reply.body).contains("new prompt"));
        assert_retired_routes_absent(&view);
        view.close();
    }

    #[test]
    fn initial_catalog_page_precedes_global_native_observation() {
        for unrelated in [0, 256, 2048] {
            let fixture = Fixture::new();
            fixture.write("proc/sys/kernel/hostname", "producer-fixture\n");
            for n in 0..60 {
                let id = format!("selected-{n:05}");
                fixture.claude(&id,&[json!({"type":"user","sessionId":id,"uuid":id,"cwd":"/tmp/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"selected first page"}})]);
            }
            for n in 0..unrelated {
                let id = format!("unrelated-{n:05}");
                let path=fixture.claude(&id,&[json!({"type":"user","sessionId":id,"uuid":id,"cwd":"/tmp/project","timestamp":"2026-09-01T00:00:00Z","message":{"role":"user","content":"unrelated old history"}})]);
                fs::File::open(path)
                    .unwrap()
                    .set_times(
                        fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)),
                    )
                    .unwrap();
            }
            let pool = RefreshPool::new(1);
            let view = pooled(&fixture, Refresh::OnInvalidate, &pool);
            let options = fixture.options.clone();
            let start = Instant::now();
            let (published, first_page) = mpsc::channel();
            let (release, released) = mpsc::channel::<()>();
            let released = Mutex::new(released);
            *lock(&view.hooks.building) = Some(Arc::new(move || {
                let page = crate::session_catalog_page(&options, "fixture", "limit=60");
                assert_eq!(page.status, 200);
                let body: Value = serde_json::from_slice(&page.body).unwrap();
                assert_eq!(body["completeness"]["state"], "partial");
                let items = body["items"].as_array().unwrap();
                assert_eq!(items.len(), 60);
                assert!(
                    items
                        .iter()
                        .all(|item| item["key"].as_str().unwrap().starts_with("selected-"))
                );
                let connection =
                    rusqlite::Connection::open(EventCache::path(&options.cache)).unwrap();
                let observed: i64 = connection
                    .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(observed, if unrelated == 0 { 60 } else { 64 });
                assert!(
                    start.elapsed() <= AT_ONCE,
                    "bounded initial page took {:?}",
                    start.elapsed()
                );
                eprintln!(
                    "production-first-page unrelated={unrelated} latency_us={} response_bytes={} native_sources={observed}",
                    start.elapsed().as_micros(),
                    page.body.len()
                );
                published.send(()).unwrap();
                let _ = lock(&released).recv();
            }));
            view.note_catalog_read().unwrap();
            first_page
                .recv_timeout(HUNG_AFTER)
                .expect("bounded cold page was not published");
            let reading = view.clone();
            let page = at_once(
                "foreground Catalog while global observation is held",
                move || native_api(&reading, "/api/sessions", "limit=60"),
            );
            assert_eq!(page.status, 200);
            assert_eq!(body_of(&page)["completeness"]["state"], "partial");
            assert_eq!(pool.running(), 1);
            let identity =
                crate::session_catalog_identity(&fixture.options, "fixture", "selected-00000")
                    .unwrap()
                    .unwrap();
            assert_eq!(
                identity.owner_qualification,
                crate::CatalogOwnerQualification::Provisional
            );
            assert!(identity.native_id.is_none());
            assert!(
                crate::source_projection_ready(
                    &fixture.options,
                    "claude",
                    "projects/project/selected-00000.jsonl",
                )
                .unwrap()
                .is_none(),
                "partial browsing cannot authorize archive eviction"
            );
            let options = fixture.options.clone();
            let focused = Instant::now();
            let answer = at_once("focused native Query while history is held", move || {
                crate::Query::with_machines(vec![("fixture".into(), options)])
                    .call_selected(
                        "fixture",
                        "selected-00000",
                        "read_transcript",
                        &json!({"id":"selected-00000","limit":1}),
                        None,
                        crate::SessionSourceReadMode::LocalThenProvider,
                    )
                    .unwrap()
            });
            assert!(answer.to_string().contains("selected first page"));
            assert_eq!(pool.running(), 1);
            eprintln!(
                "progressive-focused-query unrelated={unrelated} elapsed_us={} response_bytes={} background_running=1 completeness=partial owner_qualification=provisional",
                focused.elapsed().as_micros(),
                answer.to_string().len()
            );
            drop(release);
            // Historical ingestion scales with inventory size. Its bounded
            // steps commit Index offsets and partial Catalog publications while
            // foreground reads remain responsive. Require complete collection
            // qualification and worker cleanup before the existing production
            // stalled-worker boundary; the foreground deadline stays separate.
            let background = Instant::now();
            let connection =
                rusqlite::Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
            let mut observed: i64 = connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
                .unwrap();
            let deadline = background + STALLED_AFTER;
            loop {
                assert!(pool.threads() <= 1);
                assert!(pool.entries_for(&view) <= 1);
                let indexed: i64 = connection
                    .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
                    .unwrap();
                assert!(
                    indexed >= observed,
                    "cold observation lost committed source evidence"
                );
                if indexed > observed {
                    observed = indexed;
                }
                let page = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
                let body: Value = serde_json::from_slice(&page.body).unwrap();
                if page.status == 200
                    && body["completeness"]["state"] == "complete"
                    && pool.running() == 0
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "cold Catalog did not complete before the production stall boundary {STALLED_AFTER:?}: indexed={observed}, running={}",
                    pool.running()
                );
                thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(observed, 60 + unrelated);
            eprintln!(
                "production-background-observation unrelated={unrelated} elapsed_us={} native_sources={observed} running={}",
                background.elapsed().as_micros(),
                pool.running()
            );
            let page = crate::session_catalog_page(&fixture.options, "fixture", "limit=60");
            let body: Value = serde_json::from_slice(&page.body).unwrap();
            assert_eq!(body["completeness"]["state"], "complete");
            assert!(
                crate::session_catalog_identity(&fixture.options, "fixture", "selected-00000")
                    .unwrap()
                    .is_some()
            );
            view.close();
        }
    }

    /// Fifty views read at once are checked by the pool's three threads,
    /// never more, and each is queued once at most. With no read after the
    /// first, each is rebuilt by the pool alone (a read can't do it for
    /// them), and serves the new line.
    #[test]
    fn fifty_views_are_checked_by_the_pools_threads_alone() {
        let fixture = lane_fixture();
        let pool = RefreshPool::new(3);
        let views: Vec<_> = (0..50)
            .map(|_| {
                let view = pooled(&fixture, Refresh::Background, &pool);
                publish_native(&view);
                assert_eq!(native_api(&view, "/api/sessions", "").status, 200);
                view
            })
            .collect();
        // A build counts when it starts: the model it shows is what ends it.
        let shown = |view: &Arc<MachineView>| native_snapshot(view);
        let before: Vec<_> = views.iter().map(shown).collect();
        let builds: Vec<_> = views.iter().map(|view| view.hooks.builds()).collect();
        say(&fixture, "lane", 1, "seen by fifty views");
        eventually("a rebuild of every view, with no read", || {
            assert!(pool.threads() <= 3, "{} threads", pool.threads());
            assert!(pool.queued() <= views.len());
            for view in &views {
                assert!(pool.entries_for(view) <= 1);
            }
            views
                .iter()
                .zip(builds.iter().zip(&before))
                .all(|(view, (builds, before))| {
                    view.hooks.builds() > *builds
                        && shown(view).is_some_and(|snapshot| Some(&snapshot) != before.as_ref())
                })
                .then_some(())
        });
        for view in &views {
            assert!(serves(view, "seen by fifty views"));
        }
        let (peak, _) = pool.peak_and_started();
        assert!((1..=3).contains(&peak), "{peak} threads at once");
        for view in &views {
            view.close();
        }
        assert_eq!(pool.queued(), 0);
    }

    /// A burst of due checks is spread over the pool's workers. With a pool
    /// of two and two views invalidated at once, the first one's build
    /// held, the other is rebuilt by the other worker with no read in
    /// between: first with one thread waiting (the pool grows), then with
    /// both waiting on the views' 30 s safety checks (the wake reaches the
    /// second).
    #[test]
    fn a_burst_of_due_checks_uses_every_worker() {
        let (held_logs, free_logs) = (lane_fixture(), lane_fixture());
        let pool = RefreshPool::new(2);
        let held = pooled(&held_logs, Refresh::OnInvalidate, &pool);
        let free = pooled(&free_logs, Refresh::OnInvalidate, &pool);
        warm(&held);
        warm(&free);
        for round in 1..=2 {
            eventually("every worker waiting", || {
                (pool.threads() == round && pool.waiting() == round).then_some(())
            });
            let (starts, release) = held.hooks.hold();
            let builds = free.hooks.builds();
            let reads = free.hooks.refreshes_first.load(Ordering::SeqCst);
            say(&held_logs, "lane", round, "held");
            say(&free_logs, "lane", round, "free");
            let invalidated = Instant::now();
            held.invalidate();
            free.invalidate();
            starts
                .recv_timeout(Duration::from_secs(10))
                .expect("a worker rebuilds the held view");
            eventually("the free view, rebuilt by the other worker", || {
                (free.hooks.builds() > builds).then_some(())
            });
            let took = invalidated.elapsed();
            assert!(
                took < Duration::from_millis(900),
                "round {round}: rebuilt {took:?} after its invalidation"
            );
            assert_eq!(
                free.hooks.refreshes_first.load(Ordering::SeqCst),
                reads,
                "a read rebuilt it"
            );
            drop(release);
            eventually("the held view's check done", || {
                (lock(&held.live.state).slot != Slot::Running).then_some(())
            });
            // Past the spacing, so the next round's checks are due at once.
            thread::sleep(REBUILD_SPACING + CHECK_EVERY);
        }
        assert_eq!(pool.peak_and_started().0, 2);
        held.close();
        free.close();
    }

    /// A view is queued once however often it is read or invalidated
    /// while its check waits (here behind a build that holds the pool's
    /// only worker).
    #[test]
    fn a_view_is_never_queued_twice() {
        let (blocker, lane) = (lane_fixture(), lane_fixture());
        let pool = RefreshPool::new(1);
        let busy = pooled(&blocker, Refresh::Background, &pool);
        warm(&busy);
        let pushed = pooled(&lane, Refresh::OnInvalidate, &pool);
        let watched = pooled(&lane, Refresh::Background, &pool);
        for view in [&pushed, &watched] {
            warm(view);
        }
        let (starts, release) = busy.hooks.hold();
        say(&blocker, "lane", 1, "holds the only worker");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the only worker rebuilds the blocker");
        // A slow suite can cross OVERFLOW_AFTER while this loop reads SQLite.
        // Overdue reads then admit an overflow worker, so the warm-up/build
        // channel alone cannot prove the other views are still queued. Hold
        // dispatch, while retaining real admission, replacement and nudges.
        let dispatch = pool.hold_dispatch();
        assert_eq!(pool.running(), 1);
        for _ in 0..100 {
            pushed.invalidate();
            for view in [&pushed, &watched] {
                native_api(view, "/api/sessions", "");
            }
        }
        assert_eq!(pool.entries_for(&pushed), 1);
        assert_eq!(pool.entries_for(&watched), 1);
        assert_eq!(pool.entries_for(&busy), 0, "running, not queued");
        assert_eq!(pool.queued(), 2);
        drop(dispatch);
        drop(release);
        for view in [&busy, &pushed, &watched] {
            view.close();
        }
    }

    /// Reproduce the old observation's missing lifecycle condition without a
    /// wall-clock wait: recovery legitimately takes a queued view while the
    /// nominal sole worker is held. Its queue count is then zero, not one.
    #[test]
    fn overflow_dispatch_changes_queue_counts_without_duplicate_admission() {
        let (blocker, lane) = (lane_fixture(), lane_fixture());
        let pool = RefreshPool::with_overflow_after(1, Duration::ZERO);
        let busy = pooled(&blocker, Refresh::Background, &pool);
        let pushed = pooled(&lane, Refresh::OnInvalidate, &pool);
        warm(&busy);
        warm(&pushed);
        let (busy_starts, busy_release) = busy.hooks.hold();
        say(&blocker, "lane", 1, "held ordinary worker");
        busy.invalidate();
        busy_starts.recv_timeout(HUNG_AFTER).unwrap();
        let (pushed_starts, pushed_release) = pushed.hooks.hold();
        say(&lane, "lane", 1, "held overflow worker");
        pushed.invalidate();
        assert_eq!(pool.entries_for(&pushed), 1);
        pool.nudge();
        pushed_starts.recv_timeout(HUNG_AFTER).unwrap();
        assert_eq!(pool.running(), 2);
        assert_eq!(pool.entries_for(&pushed), 0);
        assert_eq!(lock(&pushed.live.state).slot, Slot::Running);
        for _ in 0..100 {
            pushed.invalidate();
            native_api(&pushed, "/api/sessions", "");
            assert_eq!(pool.entries_for(&pushed), 0);
        }
        eprintln!(
            "queue lifecycle: ordinary worker held, overflow running=2, selected queued=0; no duplicate admission"
        );
        drop(pushed_release);
        drop(busy_release);
        busy.close();
        pushed.close();
    }

    /// Invalidated twenty times in half a second, a view rebuilds at most
    /// twice (one rebuild at once, one a second after it for what came
    /// meanwhile) and serves the last line; with no more invalidations it
    /// rebuilds no more.
    #[test]
    fn twenty_invalidations_in_a_second_are_at_most_two_rebuilds() {
        let fixture = lane_fixture();
        let pool = RefreshPool::new(2);
        let view = pooled(&fixture, Refresh::OnInvalidate, &pool);
        warm(&view);
        let before = view.hooks.builds();
        for number in 0..20 {
            say(&fixture, "lane", number, &format!("line {number}"));
            view.invalidate();
            thread::sleep(Duration::from_millis(25));
        }
        eventually("the last line", || {
            serves(&view, "\"line 19\"").then_some(())
        });
        let rebuilds = view.hooks.builds() - before;
        assert!(
            (1..=2).contains(&rebuilds),
            "{rebuilds} rebuilds for twenty invalidations"
        );
        thread::sleep(REBUILD_SPACING + CHECK_EVERY);
        view.respond("GET", "/api/model", "", None);
        assert_eq!(view.hooks.builds() - before, rebuilds, "none without one");
        view.close();
    }

    /// Refreshed on invalidation, a view read for over a second takes no
    /// stat pass until it is invalidated (a background one takes several);
    /// then it takes one, and serves the change.
    #[test]
    fn on_invalidate_takes_no_stat_pass_until_invalidated() {
        let fixture = lane_fixture();
        let pool = RefreshPool::new(2);
        let pushed = pooled(&fixture, Refresh::OnInvalidate, &pool);
        let watched = pooled(&fixture, Refresh::Background, &pool);
        let versions = [warm(&pushed), warm(&watched)];
        fn stats(view: &MachineView) -> u64 {
            view.hooks.stats.load(Ordering::SeqCst)
        }
        let (quiet, busy) = (stats(&pushed), stats(&watched));
        let until = Instant::now() + REBUILD_SPACING + Duration::from_millis(500);
        while Instant::now() < until {
            for (view, version) in [&pushed, &watched].into_iter().zip(&versions) {
                let reply = native_api(view, "/api/sessions", "");
                assert_eq!(reply.status, 200);
                assert_eq!(body_of(&reply)["generation"], *version);
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(stats(&pushed), quiet, "a stat pass with no invalidation");
        assert!(stats(&watched) > busy, "the background view never looked");
        say(&fixture, "lane", 1, "announced");
        pushed.invalidate();
        eventually("the announced line", || {
            serves(&pushed, "announced").then_some(())
        });
        assert!(stats(&pushed) > quiet);
        pushed.close();
        watched.close();
    }

    /// Refreshed on invalidation, a view that is read is still checked
    /// every SAFETY_EVERY (half a second here), so a change nobody
    /// announced is served all the same.
    #[test]
    fn on_invalidate_still_catches_a_change_nobody_announced() {
        let fixture = lane_fixture();
        let pool = RefreshPool::new(2);
        let view = pooled(&fixture, Refresh::OnInvalidate, &pool);
        let safety = Duration::from_millis(500);
        lock(&view.live.state).safety_every = safety;
        warm(&view);
        let builds = view.hooks.builds();
        let written = Instant::now();
        say(&fixture, "lane", 1, "never announced");
        eventually("the unannounced line", || {
            serves(&view, "never announced").then_some(())
        });
        let took = written.elapsed();
        let bound = safety + CHECK_EVERY + Duration::from_millis(750);
        assert!(took < bound, "served after {took:?}, over {bound:?}");
        assert_eq!(view.hooks.builds(), builds + 1);
        view.close();
    }

    /// When the pool falls behind (its only worker held by another view's
    /// build), a read answers from the last model at once however late its
    /// view's check is, OVERDUE_AFTER and past it, and never builds: the
    /// view's own builds are held too, so a read that built would hang
    /// there. The pool never starts a thread past its size for it, and the
    /// change is served once the worker is free. (Before, a read that found
    /// its check OVERDUE_AFTER late rebuilt the view itself and waited.)
    #[test]
    fn an_overdue_check_never_holds_up_a_read() {
        let (blocker, lane) = (lane_fixture(), lane_fixture());
        let pool = RefreshPool::new(1);
        let busy = pooled(&blocker, Refresh::Background, &pool);
        let view = pooled(&lane, Refresh::Background, &pool);
        warm(&busy);
        let v1 = warm(&view);
        let reads = view.hooks.refreshes_first.load(Ordering::SeqCst);
        let (busy_starts, busy_release) = busy.hooks.hold();
        say(&blocker, "lane", 1, "holds the only worker");
        busy_starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the only worker rebuilds the blocker");
        let (starts, release) = view.hooks.hold();
        say(&lane, "lane", 1, "while the pool is busy");
        let overdue = || match lock(&view.live.state).slot {
            Slot::Queued(key) => Instant::now() >= key.0 + OVERDUE_AFTER,
            Slot::Idle | Slot::Running => false,
        };
        let until = Instant::now() + OVERDUE_AFTER + CHECK_EVERY + Duration::from_millis(500);
        let mut late = 0;
        while Instant::now() < until {
            late += usize::from(overdue());
            let reply = ask("a read with its check late", &view, "/api/sessions", "");
            assert_eq!(reply.status, 200, "the cached Catalog answers");
            assert_eq!(body_of(&reply)["generation"], v1);
            let page = serves_at_once(
                "a page with its check late",
                &view,
                "while the pool is busy",
            );
            assert!(!page, "served before its check ran");
            thread::sleep(Duration::from_millis(50));
        }
        assert!(late > 0, "the view's check never came OVERDUE_AFTER late");
        assert!(starts.try_recv().is_err(), "a read built the view");
        assert_eq!(
            view.hooks.refreshes_first.load(Ordering::SeqCst),
            reads,
            "a read refreshed first"
        );
        assert_eq!(pool.threads(), 1);
        assert_eq!(pool.peak_and_started().0, 1);
        drop(release);
        drop(busy_release);
        eventually("the line, once the worker is free", || {
            serves(&view, "while the pool is busy").then_some(())
        });
        busy.close();
        view.close();
    }

    /// A cold selected read waits for the first Catalog publication without
    /// holding Work, and a warm read answers while the later build is held.
    #[test]
    fn a_cold_native_read_waits_for_publication_without_holding_work() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        view.set_refresh(Refresh::Background);
        let (starts, release) = view.hooks.hold();
        let observing = lock(&view.evidence.work);
        let (wait_started_send, wait_started_receive) = mpsc::channel();
        let (first_send, first_receive) = mpsc::channel();
        let reader = view.clone();
        let request = thread::spawn(move || {
            INITIAL_CATALOG_WAIT_STARTED.with(|started| {
                *started.borrow_mut() = Some(wait_started_send);
            });
            let reply = native_api(&reader, "/api/session-transcript", "sid=lane&limit=100");
            let _ = first_send.send(reply);
        });
        let wait_started = wait_started_receive.recv_timeout(HUNG_AFTER).is_ok();
        drop(observing);
        let producer_started = starts.recv_timeout(Duration::from_secs(10)).is_ok();
        assert!(wait_started, "cold read did not enter its publication wait");
        assert!(producer_started, "the native producer did not start");
        assert_eq!(view.hooks.refreshes_first.load(Ordering::SeqCst), 0);
        let during = ask(
            "cold selected read during observation",
            &view,
            "/api/session-transcript",
            "sid=lane&limit=100",
        );
        assert_eq!(
            during.status, 200,
            "bounded discovery already published qualified selected evidence"
        );
        let selected = body_of(&during);
        assert_eq!(selected["identity"]["catalog_key"], "lane");
        assert_eq!(selected["projection"]["total"], 2);
        assert_eq!(selected["entries"].as_array().unwrap().len(), 2);
        assert_eq!(
            selected["entries"][1]["text"],
            "<img src=x onerror=alert(2)>"
        );
        drop(release);
        let first = first_receive.recv_timeout(Duration::from_secs(10)).ok();
        assert_eq!(first.as_ref().map(|reply| reply.status), Some(200));
        assert!(request.join().is_ok());
        eventually("the first qualified native read", || {
            let reply = native_api(&view, "/api/session-transcript", "sid=lane&limit=100");
            (reply.status == 200).then_some(())
        });
        assert_eq!(view.hooks.builds(), 1);
        assert_eq!(view.hooks.refreshes_first.load(Ordering::SeqCst), 0);
        assert_retired_routes_absent(&view);
        view.close();
    }

    /// Closing a view drops its queued check at once, and waits out a
    /// check in flight: here one view's check waits behind another's held
    /// build on the pool's only worker.
    #[test]
    fn close_drops_a_queued_check_and_waits_out_a_running_one() {
        let (busy_logs, queued_logs) = (lane_fixture(), lane_fixture());
        let pool = RefreshPool::new(1);
        let busy = pooled(&busy_logs, Refresh::Background, &pool);
        let queued = pooled(&queued_logs, Refresh::Background, &pool);
        warm(&busy);
        warm(&queued);
        let (starts, release) = busy.hooks.hold();
        say(&busy_logs, "lane", 1, "in flight");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the only worker rebuilds the busy view");
        assert_eq!(pool.entries_for(&queued), 1, "queued behind it");
        let closing = Instant::now();
        queued.close();
        assert!(
            closing.elapsed() < Duration::from_millis(500),
            "close waited for a queued check"
        );
        assert_eq!(pool.entries_for(&queued), 0);
        let builds = queued.hooks.builds();
        say(&queued_logs, "lane", 1, "after close");
        let (closed, done) = mpsc::channel();
        let closer = {
            let busy = busy.clone();
            thread::spawn(move || {
                busy.close();
                let _ = closed.send(());
            })
        };
        assert!(
            done.recv_timeout(Duration::from_millis(300)).is_err(),
            "close returned during a check in flight"
        );
        drop(release);
        done.recv_timeout(Duration::from_secs(10))
            .expect("close returns once the check ends");
        closer.join().expect("close panicked");
        assert_eq!(pool.queued(), 0);
        thread::sleep(REBUILD_SPACING + CHECK_EVERY);
        assert_eq!(queued.hooks.builds(), builds, "a closed view was rebuilt");
    }

    /// An embedding server's core in OnInvalidate: `invalidate` names a
    /// machine by its key, and that machine's change is served.
    #[test]
    fn a_core_invalidates_a_machine_by_its_key() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let mut core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        core.set_refresh(Refresh::OnInvalidate);
        core.set_refresh_pool(RefreshPool::new(1));
        publish_core_native(&core);
        assert!(!core.invalidate("c"), "no machine is served as c");
        say(&alpha, "lane-a", 1, "pushed to alpha");
        assert!(core.invalidate("a"));
        eventually("alpha's pushed line", || {
            let page = core.respond(
                "GET",
                "/api/session-transcript",
                "machine=a&sid=lane-a&limit=100",
                None,
            );
            String::from_utf8_lossy(&page.body)
                .contains("pushed to alpha")
                .then_some(())
        });
        core.close();
        assert!(!core.invalidate("a"), "a closed core invalidates nothing");
    }

    /// Builds, stat passes and reads' own refreshes of a view so far: what a
    /// request that refreshes nothing leaves as it was.
    fn refresh_counts(view: &MachineView) -> (u64, u64, u64) {
        (
            view.hooks.builds(),
            view.hooks.stats.load(Ordering::SeqCst),
            view.hooks.refreshes_first.load(Ordering::SeqCst),
        )
    }

    /// Makes `view` go idle quickly: no read for 300 ms and it is no longer
    /// checked, and a read one is checked every 200 ms until then.
    fn quick_idle(view: &MachineView) {
        let mut state = lock(&view.live.state);
        state.idle_after = Duration::from_millis(300);
        state.safety_every = Duration::from_millis(200);
    }

    /// Waits until no background check of `view` is queued or running.
    fn until_idle(view: &MachineView) {
        eventually("the view's checks stop", || {
            (lock(&view.live.state).slot == Slot::Idle).then_some(())
        });
    }

    fn tx_has(core: &ViewerCore, source: &str, sid: &str, text: &str) -> bool {
        let reply = core.respond(
            "GET",
            "/api/session-transcript",
            &format!("machine={source}&sid={sid}&limit=100"),
            None,
        );
        reply.status == 200 && String::from_utf8_lossy(&reply.body).contains(text)
    }

    #[test]
    fn native_permalinks_use_indexed_identity_without_workspace_models() {
        let (alpha, bravo) = (machine("twin", "lane-a"), machine("twin", "lane-b"));
        for fixture in [&alpha, &bravo] {
            let observer = crate::SessionCatalogObserver::new(fixture.options.clone());
            observer.changed().unwrap();
            eventually("native permalink publication", || {
                let reply = crate::session_catalog_page(&fixture.options, "source", "limit=1");
                let body: Value = serde_json::from_slice(&reply.body).ok()?;
                (reply.status == 200 && body["completeness"]["state"] == "complete").then_some(())
            });
            observer.close();
        }
        let mut query = crate::Query::new(alpha.options.clone());
        let details = query.call("get_session", &json!({"id":"lane-a"})).unwrap();
        let turn = details["session"]["turns"][0]["id"].as_str().unwrap();
        let core = Arc::new(ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]));
        let views = core.machine_views();
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        for path in [
            "/session/a/lane-a".to_owned(),
            "/session/b/lane-b".into(),
            "/machines/twin".into(),
            "/machines/twin~b".into(),
            format!("/session/a/lane-a/trace/{turn}"),
        ] {
            let reply = core.respond("GET", &path, "", None);
            assert_eq!(
                reply.status,
                200,
                "{path}: {}",
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(reply.body, PAGE.as_bytes());
        }
        for path in [
            "/s/claude/missing",
            "/s/codex/lane-a",
            "/machines/missing",
            "/trace/claude/lane-a/missing-turn",
        ] {
            assert_eq!(core.respond("GET", path, "", None).status, 404, "{path}");
        }
        assert_eq!(
            core.respond("GET", "/session/a/lane-a/extra", "", None)
                .status,
            400
        );
        assert_eq!(crate::events::CACHE_READS.with(|reads| reads.get()), 0);
        for view in &views {
            assert_retired_routes_absent(view);
            assert!(lock(&view.evidence.work).events.is_none());
            assert_eq!(refresh_counts(view), (0, 0, 0));
        }
        // Identity may be newly discovered by an in-flight native producer.
        // The shell answers at once and lets focused APIs await publication.
        let observing = lock(&views[0].evidence.work);
        let during = {
            let core = core.clone();
            at_once("native permalink during observation", move || {
                core.respond("GET", "/session/a/brand-new", "", None)
            })
        };
        assert_eq!(
            (during.status, during.body.as_slice()),
            (200, PAGE.as_bytes())
        );
        drop(observing);
        core.close();
    }

    /// Outside OnRead a page never waits for a build, nor takes a stat pass:
    /// before the first model it is the shell (its own /api/model builds
    /// the model), and once a model is built it answers from that one even
    /// when a change was announced. Only the API routes refresh: once a
    /// model is built, an API read answers from it at once too, and queues
    /// the check that rebuilds it. A URL the built model lacks takes the
    /// exact route: a fresh check, then 404.
    #[test]
    fn a_page_never_waits_for_a_build_only_the_api_refreshes() {
        let fixture = lane_fixture();
        let mut core = ViewerCore::new(fixture.options.clone());
        core.set_refresh_pool(RefreshPool::new(1));
        core.set_refresh(Refresh::OnInvalidate);
        let view = core.machine_views().remove(0);
        let core = Arc::new(core);
        quick_idle(&view);
        let page = |path: &str| {
            let reply = core.respond("GET", path, "", None);
            assert_eq!(reply.status, 200, "{path}");
            assert_eq!(reply.content_type, "text/html; charset=utf-8", "{path}");
            assert_eq!(reply.body, PAGE.as_bytes(), "{path}");
        };
        // No model yet: every page is the shell, and nothing is read.
        for path in ["/", "/analytics", "/machines/testbox"] {
            page(path);
        }
        page("/session/local/lane");
        let api =
            crate::session_transcript_range(&fixture.options, "local", "sid=lane&limit=100", None);
        assert_eq!(api.status, 503);
        assert_eq!(body_of(&api)["error"]["retryable"], true);
        assert_eq!(core.respond("GET", "/viewer.js", "", None).status, 200);
        assert_eq!(refresh_counts(&view), (0, 0, 0), "a page read the logs");

        let generation = warm(&view);
        let selected = native_selected(&view, "lane");
        let turn = selected["entries"][0]["turn"].as_str().unwrap().to_owned();
        assert_eq!(view.hooks.builds(), 1);

        // Idle, then a change announced: pages take no notice of it.
        until_idle(&view);
        say(&fixture, "lane", 1, "announced while idle");
        assert!(core.invalidate(""));
        let before = refresh_counts(&view);
        for path in [
            "/session/local/lane".to_owned(),
            "/machines/testbox".into(),
            format!("/session/local/lane/trace/{turn}"),
            "/analytics".into(),
        ] {
            page(&path);
        }
        assert_eq!(core.respond("GET", "/viewer.css", "", None).status, 200);
        assert_eq!(
            refresh_counts(&view),
            before,
            "a page refreshed a model whose logs changed"
        );
        assert!(lock(&view.live.state).invalidated, "a page took the change");

        // The API read answers at once from the built model, and queues the
        // check that rebuilds it; the change is served after that check.
        let reply = {
            let core = core.clone();
            at_once("an API read after an announced change", move || {
                core.respond("GET", "/api/sessions", "sid=lane", None)
            })
        };
        assert_eq!(reply.status, 200);
        assert_eq!(body_of(&reply)["generation"], generation);
        assert!(!String::from_utf8_lossy(&reply.body).contains("announced while idle"));
        eventually("the announced change", || {
            tx_has(&core, "local", "lane", "announced while idle").then_some(())
        });
        // A readable commit can precede the worker releasing its work lock.
        until_idle(&view);
        assert_eq!(view.hooks.builds(), before.0 + 1);
        assert_eq!(
            view.hooks.refreshes_first.load(Ordering::SeqCst),
            before.2,
            "an API read refreshed first"
        );
        // A URL the built model lacks: checked against a fresh one, 404.
        for path in [
            "/s/claude/nobody",
            "/machines/elsewhere",
            "/s/codex/lane",
            "/timeline",
        ] {
            assert_eq!(core.respond("GET", path, "", None).status, 404, "{path}");
        }
        core.close();
    }

    /// The same across several machines: a page named in the union of the
    /// built models, or asked before a machine has one, is the shell at
    /// once; neither machine is built or checked for it.
    #[test]
    fn a_page_across_machines_never_waits_for_a_build() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let mut core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        core.set_refresh_pool(RefreshPool::new(1));
        core.set_refresh(Refresh::OnInvalidate);
        let views = core.machine_views();
        for view in &views {
            quick_idle(view);
        }
        let shell = |path: &str| {
            let reply = core.respond("GET", path, "", None);
            assert_eq!(
                (reply.status, reply.body.as_slice()),
                (200, PAGE.as_bytes()),
                "{path}"
            );
        };
        shell("/machines/alpha");
        shell("/session/b/lane-b");
        let api =
            crate::session_transcript_range(&bravo.options, "b", "sid=lane-b&limit=100", None);
        assert_eq!(api.status, 503);
        assert_eq!(body_of(&api)["error"]["retryable"], true);
        for view in &views {
            assert_eq!(refresh_counts(view), (0, 0, 0), "a page read the logs");
        }
        for view in &views {
            warm(view);
        }
        for view in &views {
            until_idle(view);
        }
        say(&alpha, "lane-a", 1, "announced while idle");
        say(&bravo, "lane-b", 1, "announced while idle");
        assert!(core.invalidate("a") && core.invalidate("b"));
        let before: Vec<_> = views.iter().map(|view| refresh_counts(view)).collect();
        for path in ["/session/a/lane-a", "/session/b/lane-b", "/machines/bravo"] {
            shell(path);
        }
        let after: Vec<_> = views.iter().map(|view| refresh_counts(view)).collect();
        assert_eq!(after, before, "a page refreshed a machine");
        eventually("bravo's announced change", || {
            tx_has(&core, "b", "lane-b", "announced while idle").then_some(())
        });
        assert_eq!(core.respond("GET", "/machines/gamma", "", None).status, 404);
        core.close();
    }

    /// A native shell opens immediately while source observation is running.
    /// Focused client APIs resolve its identity after publication; the page
    /// must never join a full-model build or return a premature missing id.
    fn a_new_session_opens_during_native_publication(
        mut core: ViewerCore,
        key: &str,
        fixture: &Fixture,
    ) {
        core.set_refresh_pool(RefreshPool::new(1));
        core.set_refresh(Refresh::OnInvalidate);
        let core = Arc::new(core);
        let views = core.machine_views();
        for view in &views {
            quick_idle(view);
        }
        for view in &views {
            publish_native(view);
        }
        for view in &views {
            until_idle(view);
        }
        fixture.claude(
            "brand-new",
            &[
                json!({"type":"user","timestamp":"2026-09-24T00:04:00Z","sessionId":"brand-new","origin":{"kind":"human"},
                    "message":{"role":"user","content":"a new session"}}),
            ],
        );
        assert!(core.invalidate(key));
        // The machine's view is the first: `key` is its only or first key.
        let (starts, release) = views[0].hooks.hold();
        let warming = {
            let view = views[0].clone();
            thread::spawn(move || publish_native(&view))
        };
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the warm builds");

        let source = if key.is_empty() { "local" } else { key };
        let page = {
            let core = core.clone();
            let path = format!("/session/{source}/brand-new");
            thread::spawn(move || core.respond("GET", &path, "", None))
        };
        thread::sleep(Duration::from_millis(300));
        assert!(
            page.is_finished(),
            "native shell waited for a workspace model"
        );
        let page = page.join().expect("the page request panicked");
        assert_eq!((page.status, page.body.as_slice()), (200, PAGE.as_bytes()));
        drop(release);
        let published = warming.join().expect("native publication panicked");
        assert!(
            published["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["key"] == "brand-new")
        );
        assert_eq!(
            core.respond("GET", &format!("/session/{source}/not-even-new"), "", None)
                .status,
            404
        );
        core.close();
    }

    #[test]
    fn a_new_session_opens_during_its_warm_on_one_machine() {
        let fixture = lane_fixture();
        a_new_session_opens_during_native_publication(
            ViewerCore::new(fixture.options.clone()),
            "",
            &fixture,
        );
    }

    #[test]
    fn a_new_session_opens_during_its_warm_across_machines() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        a_new_session_opens_during_native_publication(core, "a", &alpha);
    }

    /// Asks `view` for `path?query` until [`FAILING_AFTER`] has passed
    /// `since` by ten reads: each answers at once, `before` while it was
    /// answered inside FAILING_AFTER and 500 once it was asked past it. A
    /// read is judged by when it was asked and answered, not by a loop's
    /// bound, so a stalled runner can't flip it. Returns how many reads
    /// came before.
    fn two_phases(
        view: &Arc<MachineView>,
        since: Instant,
        path: &str,
        query: &str,
        before: u16,
        also: impl Fn(),
    ) -> usize {
        let (mut inside, mut past) = (0, 0);
        while past < 10 {
            assert!(
                since.elapsed() < FAILING_AFTER + Duration::from_secs(20),
                "{path}: no 500 past FAILING_AFTER"
            );
            let asked = since.elapsed();
            let reply = ask(path, view, path, query);
            let answered = since.elapsed();
            if answered < FAILING_AFTER {
                assert_eq!(reply.status, before, "{path} answered at {answered:?}");
                inside += 1;
            } else if asked >= FAILING_AFTER {
                if path == "/api/sessions" || path.starts_with("/api/session-") {
                    assert_eq!(reply.status, 200, "qualified history remains readable");
                    assert_eq!(
                        body_of(&reply)["refresh"]["state"],
                        "unavailable",
                        "{path} asked at {asked:?}"
                    );
                } else {
                    assert_eq!(reply.status, 500, "{path} asked at {asked:?}");
                }
                past += 1;
            }
            also();
            thread::sleep(Duration::from_millis(50));
        }
        inside
    }

    /// Background builds that keep failing aren't hidden forever behind
    /// the last model, and never make a read wait: for FAILING_AFTER from
    /// the first failure seen each read answers from the last model at
    /// once, and after that the error (500), at once too, while each check
    /// tries the build again. The change is served once builds work.
    #[test]
    fn background_builds_that_keep_failing_answer_the_error_at_once() {
        let fixture = lane_fixture();
        let (view, v1) = warm_background(&fixture);
        let reads = view.hooks.refreshes_first.load(Ordering::SeqCst);
        view.hooks.failing.store(true, Ordering::SeqCst);
        say(&fixture, "lane", 1, "while builds fail");
        let since = eventually("a failed rebuild", || {
            lock(&view.live.state).failing_since[Kind::Catalog as usize]
        });
        let builds = view.hooks.builds();
        let inside = two_phases(&view, since, "/api/sessions", "", 200, || {
            let page = ask(
                "/api/session-transcript",
                &view,
                "/api/session-transcript",
                "sid=lane&limit=100",
            );
            assert_eq!(page.status, 409);
            assert_eq!(body_of(&page)["error"]["code"], "stale_projection");
            assert_eq!(body_of(&page)["resynchronize"], true);
        });
        assert!(inside > 0, "no read inside FAILING_AFTER");
        assert_eq!(
            body_of(&ask(
                "/api/session-transcript",
                &view,
                "/api/session-transcript",
                "sid=lane&limit=100"
            ))["refresh"]["state"],
            "unavailable"
        );
        assert!(
            view.hooks.builds() > builds,
            "the checks stopped trying the build"
        );
        assert_eq!(
            view.hooks.refreshes_first.load(Ordering::SeqCst),
            reads,
            "a read refreshed first"
        );
        view.hooks.failing.store(false, Ordering::SeqCst);
        eventually("native publication recovers", || {
            let reply = native_api(&view, "/api/sessions", "");
            (reply.status == 200 && body_of(&reply)["generation"] != v1).then_some(())
        });
        assert!(serves(&view, "while builds fail"));
        view.close();
    }

    /// FAILING_AFTER counts from the first failure seen, not from the
    /// start of the build that failed: a build that takes longer than
    /// FAILING_AFTER and fails once leaves the last model served, at once,
    /// for the whole grace after it.
    #[test]
    fn a_long_build_that_fails_once_gets_the_whole_grace() {
        let fixture = lane_fixture();
        let (view, v1) = warm_background(&fixture);
        view.hooks.failing.store(true, Ordering::SeqCst);
        let (starts, release) = view.hooks.hold();
        say(&fixture, "lane", 1, "while a long build fails");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the check builds");
        thread::sleep(FAILING_AFTER + Duration::from_millis(500));
        // The failure is seen after this: FAILING_AFTER from here, reads
        // answer the last model, whatever `failing_since` says.
        let released = Instant::now();
        drop(release);
        eventually("the long build's failure", || {
            lock(&view.live.state).failing_since[Kind::Catalog as usize]
        });
        let mut reads = 0;
        while released.elapsed() + Duration::from_millis(500) < FAILING_AFTER {
            let reply = ask("/api/sessions", &view, "/api/sessions", "");
            assert_eq!(
                reply.status,
                200,
                "the error answered {:?} after the failure",
                released.elapsed()
            );
            let page = body_of(&reply);
            assert_eq!(page["generation"], v1);
            assert_eq!(
                page["refresh"]["state"], "updating",
                "a completed failure receives its full grace"
            );
            reads += 1;
            thread::sleep(Duration::from_millis(50));
        }
        assert!(reads > 0, "no read inside the grace");
        view.hooks.failing.store(false, Ordering::SeqCst);
        view.close();
    }

    /// An error recorded before an idle spell is stale: no build has been
    /// tried since. The first read after the spell answers the last model
    /// at once and queues a check, and so does every read while that
    /// check's build runs (held here); once it reports the builds still
    /// fail, reads answer the error again.
    #[test]
    fn after_an_idle_spell_a_stale_error_waits_for_the_next_check() {
        let fixture = lane_fixture();
        let (view, v1) = warm_background(&fixture);
        view.hooks.failing.store(true, Ordering::SeqCst);
        say(&fixture, "lane", 1, "while builds fail");
        eventually("the error", || {
            (body_of(&native_api(&view, "/api/sessions", ""))["refresh"]["state"] == "unavailable")
                .then_some(())
        });
        lock(&view.live.state).idle_after = Duration::from_millis(300);
        until_idle(&view);
        let (starts, release) = view.hooks.hold();
        let first = ask(
            "the first read after an idle spell",
            &view,
            "/api/sessions",
            "",
        );
        assert_eq!(first.status, 200);
        assert_eq!(body_of(&first)["generation"], v1);
        assert_eq!(
            body_of(&first)["refresh"]["state"],
            "updating",
            "a stale error answered"
        );
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the queued check builds");
        for _ in 0..3 {
            let during = ask("a read during the check", &view, "/api/sessions", "");
            assert_eq!(during.status, 200);
            assert_eq!(body_of(&during)["generation"], v1);
            assert_eq!(
                body_of(&during)["refresh"]["state"],
                "updating",
                "a stale error answered"
            );
        }
        drop(release);
        eventually("the error, once the check failed again", || {
            (body_of(&ask("/api/sessions", &view, "/api/sessions", ""))["refresh"]["state"]
                == "unavailable")
                .then_some(())
        });
        view.hooks.failing.store(false, Ordering::SeqCst);
        view.close();
    }

    /// A view whose check has run STALLED_AFTER (a second here) is stalled:
    /// its build hangs (held here). Before that, reads answer the last
    /// model at once; after it, "rebuilds stalled" (500) at once. Once the
    /// build ends the rebuilt model answers.
    #[test]
    fn a_hung_build_answers_stalled_at_once() {
        let fixture = lane_fixture();
        let (view, v1) = warm_background(&fixture);
        let stalled = Duration::from_secs(1);
        lock(&view.live.state).stalled_after = stalled;
        let (starts, release) = view.hooks.hold();
        say(&fixture, "lane", 1, "after the hang");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the check builds");
        let since = lock(&view.live.state).running_since.expect("a check runs");
        let (mut inside, mut past) = (0, 0);
        while past < 5 {
            assert!(since.elapsed() < stalled + Duration::from_secs(20));
            let asked = since.elapsed();
            let reply = ask("a read during a hung build", &view, "/api/sessions", "");
            let answered = since.elapsed();
            if answered < stalled {
                assert_eq!(reply.status, 200);
                assert_eq!(body_of(&reply)["generation"], v1);
                assert_eq!(
                    body_of(&reply)["refresh"]["state"],
                    "updating",
                    "answered at {answered:?}"
                );
                inside += 1;
            } else if asked >= stalled {
                assert_eq!(reply.status, 200);
                assert_eq!(body_of(&reply)["generation"], v1);
                assert_eq!(
                    body_of(&reply)["refresh"]["state"],
                    "unavailable",
                    "asked at {asked:?}"
                );
                past += 1;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(inside > 0, "no read before the stall");
        drop(release);
        eventually("the new qualified native Catalog", || {
            let reply = native_api(&view, "/api/sessions", "");
            (reply.status == 200 && body_of(&reply)["generation"] != v1).then_some(())
        });
        assert!(serves(&view, "after the hang"));
        view.close();
    }

    #[cfg(unix)]
    #[test]
    fn initial_catalog_waiters_time_out_while_raw_writer_holds_the_shared_pool() {
        use sha2::{Digest, Sha256};
        use std::os::fd::AsRawFd;

        let blocked = lane_fixture();
        blocked.claude(
            "held",
            &[json!({
                "type": "user",
                "timestamp": "2026-09-24T00:00:00Z",
                "sessionId": "held",
                "message": {"role": "user", "content": "held source"}
            })],
        );
        let input = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/held.jsonl".into(),
        };
        let lock_key = crate::source_change::lock_key(&blocked.options, &input);
        let raw_writer =
            crate::source_change::SourceLock::acquire(&blocked.options, &lock_key, true).unwrap();
        let lock_path = blocked
            .options
            .cache
            .with_extension("source-locks")
            .join(format!("{:x}", Sha256::digest(lock_key.as_bytes())));
        let probe_exclusive = || {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .ok()?;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
            if result == 0 {
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
                Some(false)
            } else {
                Some(std::io::Error::last_os_error().raw_os_error() == Some(libc::EAGAIN))
            }
        };
        let raw_lock_held_before_wait = probe_exclusive();

        let available = lane_fixture();
        let pool = RefreshPool::new(2);
        let blocked_observer =
            crate::SessionCatalogObserver::with_pool(blocked.options.clone(), pool.clone());
        let available_observer =
            crate::SessionCatalogObserver::with_pool(available.options.clone(), pool.clone());
        let blocked_view = blocked_observer.shared_view();
        let sources = Arc::new(crate::SessionSources::from_catalog_observers(vec![
            ("held".into(), &blocked_observer),
            ("available".into(), &available_observer),
        ]));

        let barrier = Arc::new(std::sync::Barrier::new(4));
        let short_deadline = Instant::now() + Duration::from_secs(3);
        let available_deadline = Instant::now() + Duration::from_secs(10);
        let (send, receive) = mpsc::channel();
        let mut waiters = Vec::new();
        for name in ["held-1", "held-2"] {
            let sources = sources.clone();
            let pool = pool.clone();
            let barrier = barrier.clone();
            let send = send.clone();
            waiters.push(thread::spawn(move || {
                barrier.wait();
                let result = sources.wait_until_initial_catalog("held", pool, short_deadline);
                let _ = send.send((name, Instant::now(), result));
            }));
        }
        {
            let sources = sources.clone();
            let pool = pool.clone();
            let barrier = barrier.clone();
            let send = send.clone();
            waiters.push(thread::spawn(move || {
                barrier.wait();
                let result =
                    sources.wait_until_initial_catalog("available", pool, available_deadline);
                let _ = send.send(("available", Instant::now(), result));
            }));
        }
        drop(send);
        barrier.wait();

        let collect_by = Instant::now() + Duration::from_secs(12);
        let mut replies = BTreeMap::new();
        while replies.len() < 3 {
            let remaining = collect_by.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match receive.recv_timeout(remaining) {
                Ok((name, completed, result)) => {
                    replies.insert(name, (completed, result));
                }
                Err(_) => break,
            }
        }

        let raw_lock_held_with_waiters = probe_exclusive();
        let blocked_ready_with_waiters = crate::session_catalog_page_ready(&blocked.options);
        let available_ready_with_waiters = crate::session_catalog_page_ready(&available.options);
        let (blocked_producer_running, blocked_producer_has_error) =
            match blocked_view.live.state.try_lock() {
                Ok(state) => (
                    state.slot == Slot::Running,
                    state.error[Kind::Catalog as usize].is_some(),
                ),
                Err(_) => (false, true),
            };
        let blocked_queue_entries = pool.entries_for(&blocked_view);
        let held_1_completed_by_deadline = replies
            .get("held-1")
            .is_some_and(|(completed, _)| *completed <= short_deadline + Duration::from_secs(2));
        let held_2_completed_by_deadline = replies
            .get("held-2")
            .is_some_and(|(completed, _)| *completed <= short_deadline + Duration::from_secs(2));

        // Release the real writer before cleanup, joining waiters, or assertions.
        drop(raw_writer);
        let published_after_release = sources.wait_until_initial_catalog(
            "held",
            pool.clone(),
            Instant::now() + Duration::from_secs(10),
        );
        sources.close();
        for waiter in waiters {
            waiter.join().unwrap();
        }

        let timed_out = |name| matches!(replies.get(name), Some((_, Err(error))) if error.kind() == std::io::ErrorKind::TimedOut);
        assert_eq!(pool.size(), 2);
        assert_eq!(raw_lock_held_before_wait, Some(true));
        assert_eq!(raw_lock_held_with_waiters, Some(true));
        assert!(
            timed_out("held-1"),
            "first waiter did not time out while Raw was held"
        );
        assert!(
            timed_out("held-2"),
            "second waiter did not time out while Raw was held"
        );
        assert!(
            held_1_completed_by_deadline,
            "first waiter did not return within two seconds after its deadline while Raw was held"
        );
        assert!(
            held_2_completed_by_deadline,
            "second waiter did not return within two seconds after its deadline while Raw was held"
        );
        assert!(!blocked_ready_with_waiters.unwrap());
        assert!(available_ready_with_waiters.unwrap());
        assert!(matches!(replies.get("available"), Some((_, Ok(true)))));
        assert!(
            blocked_producer_running,
            "the held source producer did not remain blocked"
        );
        assert!(
            !blocked_producer_has_error,
            "Raw contention was recorded as a producer error"
        );
        assert_eq!(
            blocked_queue_entries, 0,
            "a second producer was queued for the held owner"
        );
        assert!(published_after_release.unwrap());
        let (peak, _) = pool.peak_and_started();
        assert!(
            peak <= pool.size(),
            "refresh workers exceeded the shared pool size"
        );
    }

    #[cfg(unix)]
    #[test]
    fn session_home_waits_for_first_catalog_publication_without_holding_raw_writer() {
        use sha2::{Digest, Sha256};
        use std::os::fd::AsRawFd;

        let fixture = lane_fixture();
        let home_generation = |cache: &Path| -> Option<String> {
            let db = rusqlite::Connection::open_with_flags(
                EventCache::path(cache),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .ok()?;
            let encoded: String = db
                .query_row(
                    "SELECT summary FROM session_overview WHERE session_key='lane'",
                    [],
                    |row| row.get(0),
                )
                .ok()?;
            let summary: crate::overview::Summary = serde_json::from_str(&encoded).ok()?;
            Some(crate::overview::content_generation(&summary))
        };
        let mut warm_options = fixture.options.clone();
        warm_options.cache = fixture.root.join("warm-cache");
        let warm_core = ViewerCore::new(warm_options.clone());
        let warm_view = warm_core.machine_views().into_iter().next().unwrap();
        let catalog_warmed = warm_view
            .wait_until_initial_catalog(Instant::now() + Duration::from_secs(15))
            .is_ok();
        let expected_generation = home_generation(&warm_options.cache);
        warm_core.close();

        let mut cold_options = fixture.options.clone();
        cold_options.cache = fixture.root.join("cold-cache");
        let input = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/lane.jsonl".into(),
        };
        let lock_key = crate::source_change::lock_key(&cold_options, &input);
        let raw_writer =
            crate::source_change::SourceLock::acquire(&cold_options, &lock_key, true).unwrap();
        let lock_path = cold_options
            .cache
            .with_extension("source-locks")
            .join(format!("{:x}", Sha256::digest(lock_key.as_bytes())));
        let probe_shared = || {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .ok()?;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
            if result == 0 {
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
                Some(false)
            } else {
                Some(io::Error::last_os_error().raw_os_error() == Some(libc::EAGAIN))
            }
        };
        let raw_held_before_request = probe_shared();
        let core = Arc::new(ViewerCore::new(cold_options.clone()));
        let view = core.machine_views().into_iter().next().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let request_query = expected_generation
            .as_ref()
            .map(|generation| format!("sid=lane&generation={generation}&scope=retained_history"));
        let cold_direct_reply = request_query.as_ref().map(|query| {
            crate::session_home_details_with_mode(
                &cold_options,
                "local",
                query,
                None,
                crate::SessionSourceReadMode::ProviderOnly,
            )
        });
        let (send, receive) = mpsc::channel();
        let (wait_started_send, wait_started_receive) = mpsc::channel();
        let waiting_core = core.clone();
        let first_query = request_query.clone();
        let request = thread::spawn(move || {
            INITIAL_CATALOG_WAIT_STARTED.with(|started| {
                *started.borrow_mut() = Some(wait_started_send);
            });
            let reply = first_query.map_or_else(
                || crate::catalog::error(500, "test_setup", "Home summary unavailable.", false),
                |query| {
                    waiting_core.respond_with_deadline(
                        waiting_core.extras(),
                        "GET",
                        "/api/session-home",
                        &query,
                        None,
                        deadline,
                    )
                },
            );
            let _ = send.send((Instant::now(), reply));
        });
        let first_wait_started = wait_started_receive
            .recv_timeout(Duration::from_secs(4))
            .is_ok();
        let first_reply = receive.recv_timeout(Duration::from_secs(6)).ok();
        let returned_while_raw_held = first_reply.is_some();
        let raw_held_at_deadline = probe_shared();
        let catalog_ready_at_deadline = crate::session_catalog_page_ready(&cold_options).ok();
        let producer_running_at_deadline = view
            .live
            .state
            .try_lock()
            .is_ok_and(|state| state.slot == Slot::Running);

        let (success_send, success_receive) = mpsc::channel();
        let (success_wait_send, success_wait_receive) = mpsc::channel();
        let success_core = core.clone();
        let success_query = request_query.clone();
        let success_request = thread::spawn(move || {
            INITIAL_CATALOG_WAIT_STARTED.with(|started| {
                *started.borrow_mut() = Some(success_wait_send);
            });
            let reply = success_query.map_or_else(
                || crate::catalog::error(500, "test_setup", "Home summary unavailable.", false),
                |query| {
                    success_core.respond_with_deadline(
                        success_core.extras(),
                        "GET",
                        "/api/session-home",
                        &query,
                        None,
                        Instant::now() + Duration::from_secs(10),
                    )
                },
            );
            let _ = success_send.send(reply);
        });
        let success_wait_started = success_wait_receive
            .recv_timeout(Duration::from_secs(4))
            .is_ok();

        // Release only after both cold API requests have reached their waiter.
        drop(raw_writer);
        let published = view
            .wait_until_initial_catalog(Instant::now() + Duration::from_secs(10))
            .is_ok();
        let first_reply =
            first_reply.or_else(|| receive.recv_timeout(Duration::from_secs(10)).ok());
        let success_reply = success_receive.recv_timeout(Duration::from_secs(10)).ok();
        let request_joined = request.join().is_ok();
        let success_request_joined = success_request.join().is_ok();
        core.close();

        assert!(
            catalog_warmed,
            "warm source did not publish its test summary"
        );
        assert!(
            expected_generation.is_some(),
            "warm Home summary was absent"
        );
        assert_eq!(
            cold_direct_reply.as_ref().map(|reply| reply.status),
            Some(503)
        );
        assert_eq!(raw_held_before_request, Some(true));
        assert!(
            first_wait_started,
            "deadline request did not reach catalog wait"
        );
        assert!(
            success_wait_started,
            "success request did not reach catalog wait"
        );
        assert!(
            returned_while_raw_held,
            "request remained blocked past its deadline while Raw was held"
        );
        let (completed, reply) = first_reply.unwrap();
        assert!(
            completed >= deadline,
            "cold response returned before its deadline"
        );
        assert!(
            completed <= deadline + Duration::from_secs(2),
            "cold response exceeded the request deadline by more than two seconds"
        );
        assert_eq!(reply.status, 503);
        let error: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(error["error"]["code"], "catalog_unavailable");
        assert_eq!(error["error"]["retryable"], true);
        assert_eq!(raw_held_at_deadline, Some(true));
        assert_eq!(catalog_ready_at_deadline, Some(false));
        assert!(
            producer_running_at_deadline,
            "cold producer was not still blocked"
        );
        assert!(request_joined);
        assert!(success_request_joined);
        assert!(published, "Catalog did not publish after Raw was released");
        assert_eq!(success_reply.as_ref().map(|reply| reply.status), Some(200));
    }

    /// A view whose check waits STALLED_AFTER (two seconds here) behind
    /// another view's hung build, on a pool that may not grow, is stalled
    /// too: its reads answer the last model at once, then "rebuilds
    /// stalled" (500) at once, until the worker is free and rebuilds it.
    #[test]
    fn a_check_stuck_behind_a_hung_pool_answers_stalled_at_once() {
        let (blocker, lane) = (lane_fixture(), lane_fixture());
        let pool = RefreshPool::with_overflow_after(1, Duration::from_secs(3600));
        let busy = pooled(&blocker, Refresh::Background, &pool);
        let view = pooled(&lane, Refresh::Background, &pool);
        warm(&busy);
        let v1 = warm(&view);
        lock(&view.live.state).stalled_after = Duration::from_secs(2);
        let (busy_starts, busy_release) = busy.hooks.hold();
        say(&blocker, "lane", 1, "hangs the only worker");
        busy_starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the only worker rebuilds the blocker");
        say(&lane, "lane", 1, "behind the hung pool");
        let first = ask("a read behind the hung pool", &view, "/api/sessions", "");
        assert_eq!(first.status, 200);
        assert_eq!(body_of(&first)["generation"], v1);
        assert_eq!(body_of(&first)["refresh"]["state"], "updating");
        let mut stalled = 0;
        let deadline = Instant::now() + Duration::from_secs(20);
        while stalled < 5 {
            assert!(Instant::now() < deadline, "no stall within 20 s");
            let reply = ask("a read behind the hung pool", &view, "/api/sessions", "");
            assert_eq!(reply.status, 200);
            assert_eq!(body_of(&reply)["generation"], v1);
            match body_of(&reply)["refresh"]["state"].as_str().unwrap() {
                "updating" => assert_eq!(stalled, 0, "healthy observation after the stall"),
                "unavailable" => stalled += 1,
                state => panic!("{state}"),
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(pool.peak_and_started().0, 1, "the pool grew");
        drop(busy_release);
        eventually("the line, once the worker is free", || {
            serves(&view, "behind the hung pool").then_some(())
        });
        busy.close();
        view.close();
    }

    /// Builds that hang hold every worker of a pool (its only one here),
    /// but not the other machines' checks: once every worker's check has
    /// run OVERFLOW_AFTER, a read that finds its check overdue starts a
    /// thread past the pool's size (never more than twice it), which
    /// rebuilds that view while the hang goes on. No read waits meanwhile.
    /// Once the hang ends, the pool shrinks back to its size.
    #[test]
    fn a_hung_pool_grows_for_a_late_check_then_shrinks() {
        let (blocker, lane) = (lane_fixture(), lane_fixture());
        let pool = RefreshPool::with_overflow_after(1, Duration::from_millis(300));
        let busy = pooled(&blocker, Refresh::Background, &pool);
        let view = pooled(&lane, Refresh::Background, &pool);
        warm(&busy);
        warm(&view);
        let (busy_starts, busy_release) = busy.hooks.hold();
        say(&blocker, "lane", 1, "hangs the only worker");
        busy_starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the only worker rebuilds the blocker");
        say(&lane, "lane", 1, "past the hung pool");
        eventually("the line, rebuilt by an extra thread", || {
            serves_at_once("a read behind the hung pool", &view, "past the hung pool").then_some(())
        });
        let (peak, _) = pool.peak_and_started();
        assert_eq!(peak, 2, "the pool grew, to twice its size");
        assert!(pool.threads() <= 2);
        drop(busy_release);
        eventually("the pool back to its size", || {
            // Reads keep the views checked meanwhile.
            serves_at_once("a read after the hang", &view, "past the hung pool");
            (pool.threads() == 1).then_some(())
        });
        busy.close();
        view.close();
    }

    /// A backlog is not a hang: four views whose builds take two seconds
    /// each, all changing and read all the time, keep the pool's only
    /// worker busy and their checks seconds late, past OVERFLOW_AFTER (three
    /// seconds here). No build has run that long, so the pool never grows.
    #[test]
    fn a_backlog_never_grows_the_pool() {
        let fixtures: Vec<_> = (0..4).map(|_| lane_fixture()).collect();
        let pool = RefreshPool::with_overflow_after(1, Duration::from_secs(3));
        let views: Vec<_> = fixtures
            .iter()
            .map(|fixture| {
                let view = pooled(fixture, Refresh::Background, &pool);
                warm(&view);
                *lock(&view.hooks.building) =
                    Some(Arc::new(|| thread::sleep(Duration::from_secs(2))));
                view
            })
            .collect();
        let until = Instant::now() + Duration::from_secs(10);
        let mut line = 0;
        let mut late = Duration::ZERO;
        while Instant::now() < until {
            line += 1;
            for (fixture, view) in fixtures.iter().zip(&views) {
                say(fixture, "lane", line, &format!("line {line}"));
                if let Slot::Queued(key) = lock(&view.live.state).slot {
                    late = late.max(Instant::now().saturating_duration_since(key.0));
                }
                let reply = ask("a read in a backlog", view, "/api/sessions", "");
                assert_eq!(reply.status, 200);
            }
            assert!(pool.threads() <= 1, "{} threads", pool.threads());
            thread::sleep(Duration::from_millis(300));
        }
        assert!(
            late >= Duration::from_secs(3),
            "no check fell OVERFLOW_AFTER behind: at most {late:?}"
        );
        assert_eq!(pool.peak_and_started().0, 1, "a backlog grew the pool");
        for view in &views {
            *lock(&view.hooks.building) = None;
            view.close();
        }
    }

    /// Lateness counts from when a check was queued: after a build longer
    /// than STALLED_AFTER (a second here) the next check is due in the past,
    /// and must not read as a stall, or overflow the pool, the moment it is
    /// queued. Reads after the long build answer the rebuilt model.
    #[test]
    fn a_long_build_is_not_a_stall_once_it_ends() {
        let fixture = lane_fixture();
        let pool = RefreshPool::with_overflow_after(1, Duration::from_millis(300));
        let view = pooled(&fixture, Refresh::Background, &pool);
        let v1 = warm(&view);
        let before = eventually("initial native worker snapshot", || native_snapshot(&view));
        lock(&view.live.state).stalled_after = Duration::from_secs(1);
        *lock(&view.hooks.building) = Some(Arc::new(|| thread::sleep(Duration::from_secs(3))));
        let builds = view.hooks.builds();
        say(&fixture, "lane", 1, "after a long build");
        // No read while it builds: a read then would rightly answer the stall.
        eventually("the long build", || {
            native_snapshot(&view)
                .filter(|snapshot| snapshot != &before)
                .map(drop)
        });
        assert!(view.hooks.builds() > builds);
        *lock(&view.hooks.building) = None;
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            let reply = ask("a read after a long build", &view, "/api/sessions", "");
            assert_eq!(reply.status, 200, "the rebuilt native Catalog answers");
            let page = body_of(&reply);
            assert_ne!(page["generation"], v1);
            assert_eq!(
                page["refresh"]["state"], "updating",
                "completed work is not a refresh stall"
            );
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(pool.peak_and_started().0, 1, "a long build grew the pool");
        view.close();
    }

    /// Refreshed on read, the reads that find one change share its one
    /// rebuild: the others wait for it, then answer the same new model.
    #[test]
    fn simultaneous_native_reads_coalesce_one_source_observation() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        let v1 = warm(&view);
        assert_eq!(view.hooks.builds(), 1);
        let (starts, release) = view.hooks.hold();
        say(&fixture, "lane", 1, "one change");
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let view = view.clone();
                thread::spawn(move || native_api(&view, "/api/sessions", ""))
            })
            .collect();
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the shared native observer rebuilds");
        for reader in readers {
            let reply = reader.join().expect("a reader failed");
            assert_eq!(reply.status, 200);
            assert_eq!(body_of(&reply)["generation"], v1);
        }
        assert_eq!(view.hooks.builds(), 2, "eight reads, one observation");
        assert!(lock(&view.live.state).pool.entries_for(&view) <= 1);
        drop(release);
        let v2 = eventually("the new native generation", || {
            let reply = native_api(&view, "/api/sessions", "");
            let generation = body_of(&reply)["generation"].as_str().unwrap().to_owned();
            (generation != v1).then_some(generation)
        });
        assert_ne!(v1, v2);
        assert_eq!(view.hooks.builds(), 2);
        view.close();
    }

    /// The model and the V1 tree fail on their own: a tree that won't
    /// build leaves the last tree served on `/api/tree` for FAILING_AFTER,
    /// then answers the error, each at once, while `/api/model` goes on
    /// serving a model that builds.
    #[test]
    fn a_failing_tree_leaves_native_catalog_served() {
        let fixture = lane_fixture();
        let (view, _) = warm_background(&fixture);
        assert_eq!(view.respond("GET", "/api/tree", "", None).status, 200);
        let reads = view.hooks.refreshes_first.load(Ordering::SeqCst);
        view.hooks.tree_failing.store(true, Ordering::SeqCst);
        say(&fixture, "lane", 1, "while the tree fails");
        let since = eventually("the tree's failed rebuild", || {
            lock(&view.live.state).failing_since[Kind::Tree as usize]
        });
        let inside = two_phases(&view, since, "/api/tree", "", 200, || {
            let reply = native_api(&view, "/api/sessions", "");
            assert_eq!(reply.status, 200, "the model with the new line");
        });
        assert!(inside > 0, "no tree read inside FAILING_AFTER");
        assert!(serves(&view, "while the tree fails"));
        assert_eq!(
            view.hooks.refreshes_first.load(Ordering::SeqCst),
            reads,
            "a read refreshed first"
        );
        view.hooks.tree_failing.store(false, Ordering::SeqCst);
        eventually("the tree again", || {
            (view.respond("GET", "/api/tree", "", None).status == 200).then_some(())
        });
        view.close();
    }
}

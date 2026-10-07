use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Seek, SeekFrom},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard,
        RwLockWriteGuard, TryLockError, Weak,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::{
    Index, Node, Options, codex_id_from_filename, codex_meta, collect_with_index,
    events::EventCache,
    field, file_list, is_agent_output,
    model::{self, Built, Texts},
    proc_start, read_index,
    refresh::{Key, RefreshPool},
    save_index, tx,
    union::{Refresh, ViewerCore},
    user_text,
};

const PAGE_ENTRIES: usize = 200;
const PAGE_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_BYTES: usize = 64 * 1024;
const EXPAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINE: usize = EXPAND_BYTES + 1024 * 1024;

static BUILD_TIMING: OnceLock<bool> = OnceLock::new();

fn build_timing_enabled() -> bool {
    *BUILD_TIMING.get_or_init(|| {
        std::env::var("SEMON_BUILD_TIMING")
            .map(|value| value == "1")
            .unwrap_or(false)
    })
}

fn timing_line(total: u32, timings: &[(&'static str, u32)]) -> String {
    use std::fmt::Write as _;

    let mut line = format!("semon sessions viewer: build total={total}");
    for &(name, elapsed) in timings {
        let _ = write!(&mut line, " {name}={elapsed}");
    }
    line
}

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

#[derive(Serialize)]
struct TranscriptEntry {
    offset: u64,
    block: usize,
    kind: String,
    text: String,
    name: Option<String>,
    tool_id: Option<String>,
    link: Option<String>,
    links: Vec<String>,
    collapsed: bool,
    truncated: bool,
}

#[derive(Serialize)]
struct TranscriptPage {
    entries: Vec<TranscriptEntry>,
    before: Option<u64>,
    end: u64,
    live: bool,
    children: Vec<ChildLink>,
}

#[derive(Serialize)]
struct ChildLink {
    harness: String,
    id: String,
    label: Option<String>,
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

/// The lock if it is free, poisoned or not.
fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
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
/// - `work` holds what a rebuild changes (the index, the event cache, the
///   texts, when each was saved). A rebuild holds it from its stat pass to
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
/// Locks are only ever taken in the order work, files, shown, live, and
/// the refresh pool's last of all.
pub(crate) struct MachineView {
    options: Options,
    /// Its [`Refresh`] mode ([`Refresh::code`]).
    mode: AtomicU8,
    /// A persisted-catalog reader needs coherent background publication even
    /// before this process has shown its first compatibility model.
    catalog_demand: AtomicBool,
    work: Mutex<Work>,
    shown: RwLock<Shown>,
    files: Mutex<Files>,
    live: Live,
    /// This view, for its pool's queue: an entry never keeps a view alive.
    me: Weak<MachineView>,
    #[cfg(test)]
    hooks: tests::Hooks,
}

/// What a rebuild changes, and only a rebuild.
#[derive(Default)]
struct Work {
    index: Option<Index>,
    index_dirty: bool,
    last_save: Option<Instant>,
    events: Option<EventCache>,
    texts: Texts,
    /// When the last rebuild of either model started.
    built_at: Option<Instant>,
}

/// The snapshots reads answer from.
#[derive(Default)]
struct Shown {
    model: Option<Arc<ModelCache>>,
    tree: Option<Arc<TreeCache>>,
}

/// Transcript files by session, and each Codex file's harness offsets.
#[derive(Default)]
struct Files {
    /// Background viewers walk on an unknown id at most once per second.
    lookup_at: Option<Instant>,
    paths: BTreeMap<(String, String), PathBuf>,
    known: BTreeSet<PathBuf>,
    harness: BTreeMap<PathBuf, ((u64, u64), BTreeSet<u64>)>,
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
    Model = 0,
    Tree = 1,
}

impl Kind {
    fn other(self) -> Self {
        match self {
            Kind::Model => Kind::Tree,
            Kind::Tree => Kind::Model,
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
/// type, the body and, for `/api/model` and `/api/tx?errors=1`, its `ETag`.
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
    /// whose type is one of the four image types `/api/attachment` serves,
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified_ns: u128,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(Stamp {
        size: metadata.len(),
        modified_ns: metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    })
}

struct Snapshot {
    watched: BTreeMap<PathBuf, Option<Stamp>>,
    /// The Codex writer-lock files' identities, and the `/proc/locks` lines
    /// that name them. Other locks on the machine (a browser's, SQLite's)
    /// come and go all the time and change nothing here.
    lock_ids: BTreeSet<(u64, u64, u64)>,
    locks: Option<String>,
    pids: BTreeMap<u32, Option<u64>>,
}

/// A `/proc/locks` line's `major:minor:inode`, as `lock_pid` reads it.
fn lock_key(line: &str) -> Option<(u64, u64, u64)> {
    let words: Vec<_> = line.split_whitespace().collect();
    let parts: Vec<_> = words.get(5)?.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        u64::from_str_radix(parts[0], 16).ok()?,
        u64::from_str_radix(parts[1], 16).ok()?,
        parts[2].parse().ok()?,
    ))
}

/// The lines of `/proc/locks` on the writer-lock files, and nothing else.
fn writer_locks(options: &Options, ids: &BTreeSet<(u64, u64, u64)>) -> Option<String> {
    let locks = fs::read_to_string(options.proc_root.join("locks")).ok()?;
    Some(
        locks
            .lines()
            .filter(|line| lock_key(line).is_some_and(|key| ids.contains(&key)))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn lock_ids(options: &Options) -> BTreeSet<(u64, u64, u64)> {
    let mut ids = BTreeSet::new();
    #[cfg(unix)]
    if let Ok(entries) = fs::read_dir(options.codex_home.join("thread-writer-locks")) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "lock")
                && let Ok(id) = crate::lock_identity(&entry.path())
            {
                ids.insert(id);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = options;
    ids
}

struct TreeCache {
    roots: Vec<Node>,
    json: String,
    snapshot: Snapshot,
}

struct ModelCache {
    built: Arc<Built>,
    snapshot: Snapshot,
}

/// A routed response: status, content type, body and an optional `ETag`.
type Routed = (u16, &'static str, Vec<u8>, Option<String>);

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

fn watch_tree(path: &Path, suffixes: &[&str], watched: &mut BTreeMap<PathBuf, Option<Stamp>>) {
    watched.insert(path.to_owned(), stamp(path));
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        if let Some(parent) = path.parent() {
            watched
                .entry(parent.to_owned())
                .or_insert_with(|| stamp(parent));
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // A symbolic link is neither: it is never followed.
        if kind.is_dir() {
            watch_tree(&path, suffixes, watched);
        } else if kind.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| suffixes.iter().any(|suffix| name.ends_with(suffix)))
        {
            watched.insert(path.clone(), stamp(&path));
        }
    }
}

/// Whether `path` is a regular file, itself: a link to one isn't.
fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn node_pids(nodes: &[Node], output: &mut BTreeSet<u32>) {
    for node in nodes {
        if let Some(pid) = node.pid {
            output.insert(pid);
        }
        node_pids(&node.children, output);
    }
}

impl Snapshot {
    fn capture(options: &Options, roots: &[Node], index: &Index) -> Self {
        let mut ids = BTreeSet::new();
        node_pids(roots, &mut ids);
        Self::capture_pids(options, ids, index.paths())
    }

    fn capture_pids<'a>(
        options: &Options,
        ids: BTreeSet<u32>,
        paths: impl Iterator<Item = &'a str>,
    ) -> Self {
        let mut watched = BTreeMap::new();
        watch_tree(
            &options.claude_home.join("projects"),
            &[".jsonl", ".meta.json"],
            &mut watched,
        );
        watch_tree(
            &options.claude_home.join("sessions"),
            &[".json"],
            &mut watched,
        );
        for root in crate::inputs::codex_rollout_dirs(options) {
            watch_tree(&root, &[".jsonl"], &mut watched);
        }
        watch_tree(
            &options.codex_home.join("thread-writer-locks"),
            &[".lock"],
            &mut watched,
        );
        let copilot_root = options.copilot_home.join("session-state");
        watched.insert(copilot_root.clone(), stamp(&copilot_root));
        if let Ok(entries) = fs::read_dir(&copilot_root) {
            for entry in entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            {
                let dir = entry.path();
                watched.insert(dir.clone(), stamp(&dir));
                let file = dir.join("events.jsonl");
                watched.insert(file.clone(), stamp(&file));
            }
        }
        watched.insert(options.claude_json.clone(), stamp(&options.claude_json));
        for path in paths {
            let path = PathBuf::from(path);
            watched.entry(path.clone()).or_insert_with(|| stamp(&path));
        }
        // With a facts file, it is the machine: nothing here reads `/proc`.
        if let Some(facts) = &options.facts {
            watched.insert(facts.clone(), stamp(facts));
            return Self {
                watched,
                lock_ids: BTreeSet::new(),
                locks: None,
                pids: BTreeMap::new(),
            };
        }
        let pids = ids
            .into_iter()
            .map(|pid| (pid, proc_start(&options.proc_root, pid)))
            .collect();
        let lock_ids = lock_ids(options);
        Self {
            watched,
            locks: writer_locks(options, &lock_ids),
            lock_ids,
            pids,
        }
    }

    fn changed(&self, options: &Options) -> bool {
        if self
            .watched
            .iter()
            .any(|(path, original)| stamp(path) != *original)
        {
            return true;
        }
        if options.facts.is_some() {
            return false;
        }
        writer_locks(options, &self.lock_ids) != self.locks
            || self
                .pids
                .iter()
                .any(|(pid, original)| proc_start(&options.proc_root, *pid) != *original)
    }
}

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

/// Serves the existing local viewer with one opt-in native control connection.
pub fn serve_with_control(
    options: ServeOptions,
    control: Arc<semon_control::codex::Codex>,
) -> io::Result<()> {
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
            let authorized =
                self.control.is_some() && control_authorized(&request, &self.token, self.port);
            let reply = if authorized {
                self.control_write(&mut request)
            } else {
                control_reply(403, serde_json::json!({"error":"Forbidden"}))
            };
            respond(request, reply, None);
            return;
        }
        let accepted = if self.control.is_some() {
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
        let if_none_match = request_header(&request, "If-None-Match").map(str::to_owned);
        let reply = if let Some(control) = self.control.as_ref().filter(|_| path == "/api/model") {
            // Full model while control is opted in. The existing poller owns both revisions.
            let mut reply = self.core.respond("GET", path, "", None);
            if reply.status == 200
                && let Ok(mut model) = serde_json::from_slice::<Value>(&reply.body)
            {
                model["control"] = control
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .snapshot();
                reply.body = serde_json::to_vec(&model).unwrap_or_default();
                reply.etag = None;
            }
            reply
        } else {
            self.core
                .respond("GET", path, query, if_none_match.as_deref())
        };
        respond(request, reply, set_cookie.then_some(&self.token));
    }
    fn control_write(&self, request: &mut Request) -> ViewerReply {
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
            Ok(receipt) => control_reply(200, receipt),
            Err(error) => control_reply(409, serde_json::json!({"error":error.to_string()})),
        }
    }
}

/// How a read gets a machine's model.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Reading {
    /// As a request is answered, in the view's [`Refresh`] mode.
    Served,
    /// Brought up to date first, built at `now` if it must be rebuilt.
    At(i64),
}

/// The session a transcript file holds, as the V1 routes name it: the
/// harness, and the id (a Codex file's is read from its first line).
fn transcript_key(options: &Options, path: &Path) -> Option<(String, String)> {
    if path.starts_with(options.copilot_home.join("session-state")) {
        let rel = path.strip_prefix(&options.copilot_home).ok()?.to_str()?;
        if !crate::is_input_path("copilot", rel) {
            return None;
        }
        let id = path.parent()?.file_name()?.to_str()?;
        crate::copilot::header(path, id).ok()??;
        return Some(("copilot".into(), id.into()));
    }
    let harness = if path.starts_with(options.claude_home.join("projects")) {
        "claude"
    } else {
        "codex"
    };
    let id = if harness == "claude" {
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "subagents")
        {
            stem.strip_prefix("agent-").unwrap_or(stem).to_owned()
        } else {
            stem.to_owned()
        }
    } else {
        codex_meta(path)
            .ok()
            .flatten()
            .and_then(|meta| field(&meta, "id").map(str::to_owned))
            .or_else(|| codex_id_from_filename(path))
            .unwrap_or_default()
    };
    (!id.is_empty()).then(|| (harness.to_owned(), id))
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
        Arc::new_cyclic(|me| Self {
            options,
            mode: AtomicU8::new(Refresh::OnRead.code()),
            catalog_demand: AtomicBool::new(false),
            work: Mutex::default(),
            shown: RwLock::default(),
            files: Mutex::default(),
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

    /// The homes, cache and facts this view reads.
    pub(crate) fn options(&self) -> &Options {
        &self.options
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
        if let Slot::Queued(key) = state.slot {
            state.pool.remove(key);
            state.slot = Slot::Idle;
            state.pool = pool;
            self.queue(&mut state, key.0, true);
        } else {
            state.pool = pool;
        }
    }

    /// Whether a snapshot's logs changed since it was built: a stat pass.
    fn changed(&self, snapshot: &Snapshot) -> bool {
        #[cfg(test)]
        self.hooks.stats.fetch_add(1, Ordering::SeqCst);
        snapshot.changed(&self.options)
    }

    fn shown_model(&self) -> Option<Arc<ModelCache>> {
        read_lock(&self.shown).model.clone()
    }

    fn shown_tree(&self) -> Option<Arc<TreeCache>> {
        read_lock(&self.shown).tree.clone()
    }

    /// Rebuilds the V1 tree if anything it was built from changed. The
    /// caller holds `work`.
    fn refresh_tree_locked(&self, work: &mut Work) -> io::Result<Arc<TreeCache>> {
        if let Some(tree) = self.shown_tree()
            && !self.changed(&tree.snapshot)
        {
            self.persist_if_due(work)?;
            return Ok(tree);
        }
        work.built_at = Some(Instant::now());
        #[cfg(test)]
        if self.hooks.tree_failing.load(Ordering::SeqCst) {
            return Err(io::Error::other("a tree build failed on purpose"));
        }
        let index = work
            .index
            .get_or_insert_with(|| read_index(&self.options.cache));
        let roots = collect_with_index(&self.options, index, &mut work.index_dirty)?;
        let json = crate::render_json(&roots);
        let snapshot = Snapshot::capture(&self.options, &roots, index);
        let paths = snapshot
            .watched
            .iter()
            .filter_map(|(path, value)| value.is_some().then_some(path.clone()))
            .collect::<Vec<_>>();
        self.update_files(paths);
        let tree = Arc::new(TreeCache {
            roots,
            json,
            snapshot,
        });
        write_lock(&self.shown).tree = Some(tree.clone());
        self.persist_if_due(work)?;
        Ok(tree)
    }

    fn persist_if_due(&self, work: &mut Work) -> io::Result<()> {
        if work.index_dirty
            && work
                .last_save
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(30))
        {
            save_index(
                &self.options.cache,
                work.index.as_ref().expect("index loaded"),
            )?;
            work.index_dirty = false;
            work.last_save = Some(Instant::now());
        }
        Ok(())
    }

    /// Rebuilds the model at `now` if its logs or facts changed. The caller
    /// holds `work`.
    fn refresh_model_locked(&self, work: &mut Work, now: i64) -> io::Result<Arc<ModelCache>> {
        if let Some(model) = self.shown_model()
            && !self.changed(&model.snapshot)
        {
            return Ok(model);
        }
        work.built_at = Some(Instant::now());
        if work.events.is_none() {
            work.events = Some(match EventCache::open_scoped(&self.options.cache) {
                Ok(cache) => cache,
                Err(error) if self.catalog_demand.load(Ordering::Relaxed) => {
                    return Err(io::Error::other(error));
                }
                Err(_) => EventCache::open(&self.options.cache),
            });
        }
        let cache = work.events.as_mut().expect("event cache initialized");
        // Cold catalog demand publishes a small explicit source batch before the
        // compatibility model reads unrelated native history. Source discovery is
        // still a filesystem metadata walk; incomplete native lineage cannot grant
        // control authority until the subsequent complete reconciliation.
        if self.catalog_demand.load(Ordering::Relaxed)
            && !self.options.scan_window
            && cache.is_scoped()
            && cache.session_catalog_generation().is_none()
        {
            let mut inputs = crate::inputs::inputs(&self.options)?;
            inputs.retain(|input| input.path.ends_with(".jsonl"));
            inputs.sort_by_cached_key(|input| {
                std::cmp::Reverse(
                    fs::metadata(input.full_path(&self.options))
                        .and_then(|metadata| metadata.modified())
                        .ok(),
                )
            });
            inputs.truncate(64);
            if !inputs.is_empty() {
                let prepared = model::prepare_sources(&self.options, cache, &inputs, now)?;
                match cache.publish_partial_catalog(&prepared)? {
                    crate::events::Outcome::Written => {}
                    _ => {
                        return Err(io::Error::other(
                            "initial native catalog publication changed; retry observation",
                        ));
                    }
                }
                cache.end_scan();
            }
        }
        // The index commits each file's change as it reads it: nothing is
        // left to save.
        let mut dirty = false;
        if self.options.facts.is_none() {
            cache.refresh_reported_runs(&self.options.claude_json, now, &mut dirty);
        }
        // The files are stamped before the build reads them: a line that
        // lands while it runs is then a change the next check sees, never
        // one that is neither parsed nor noticed. (A file created meanwhile
        // changes its directory's stamp.)
        let mut snapshot = Snapshot::capture_pids(&self.options, BTreeSet::new(), cache.paths());
        #[cfg(test)]
        self.hooks.building()?;
        let build_started = Instant::now();
        let built = model::build(&self.options, cache, &mut dirty, &mut work.texts, now)?;
        if build_timing_enabled() {
            let total = u32::try_from(build_started.elapsed().as_millis()).unwrap_or(u32::MAX);
            eprintln!("{}", timing_line(total, &built.timings));
        }
        if self.options.facts.is_none() {
            snapshot.pids = built
                .pids
                .iter()
                .map(|pid| (*pid, proc_start(&self.options.proc_root, *pid)))
                .collect();
        }
        let model = Arc::new(ModelCache {
            built: Arc::new(built),
            snapshot,
        });
        write_lock(&self.shown).model = Some(model.clone());
        Ok(model)
    }

    /// The model, rebuilt at `now` first if its logs or facts changed.
    fn fresh_model(&self, now: i64) -> io::Result<Arc<ModelCache>> {
        // Unchanged, as most reads find it: a stat pass and no lock held
        // but for cloning the `Arc`. Concurrent reads don't wait on each
        // other, only on a rebuild of a change they found.
        if let Some(model) = self.shown_model()
            && !self.changed(&model.snapshot)
        {
            return Ok(model);
        }
        self.refresh_model_locked(&mut lock(&self.work), now)
    }

    /// The V1 tree, rebuilt first if anything changed.
    fn fresh_tree(&self) -> io::Result<Arc<TreeCache>> {
        if let Some(tree) = self.shown_tree()
            && !self.changed(&tree.snapshot)
        {
            if let Some(mut work) = try_lock(&self.work) {
                self.persist_if_due(&mut work)?;
            }
            return Ok(tree);
        }
        self.refresh_tree_locked(&mut lock(&self.work))
    }

    /// The model a request answers from. Refreshed on read, it is
    /// [`MachineView::fresh_model`]. Otherwise it is the last one built, at
    /// once, whatever a rebuild in progress or a late check is doing, or the
    /// error of rebuilds failing for [`FAILING_AFTER`] or stalled for
    /// [`STALLED_AFTER`], also at once ([`MachineView::note_read`]). Only a
    /// view with no model yet waits, for its first build
    /// ([`MachineView::refresh_first`]).
    fn served_model(&self) -> io::Result<Arc<ModelCache>> {
        if self.refresh() == Refresh::OnRead {
            return self.fresh_model(model::now_ms());
        }
        if let Some(model) = self.shown_model() {
            self.note_read(Kind::Model)?;
            return Ok(model);
        }
        self.refresh_first(Kind::Model)?;
        self.shown_model()
            .ok_or_else(|| io::Error::other("no model was built"))
    }

    /// The V1 tree a request answers from, as [`MachineView::served_model`].
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

    /// Whether the model or the tree has been built, and so is kept fresh.
    fn is_shown(&self, kind: Kind) -> bool {
        let shown = read_lock(&self.shown);
        match kind {
            Kind::Model => shown.model.is_some(),
            Kind::Tree => shown.tree.is_some(),
        }
    }

    /// Rebuilds the model or the tree if its logs changed. The caller holds
    /// `work`.
    fn refresh_kind(&self, work: &mut Work, kind: Kind) -> io::Result<()> {
        match kind {
            Kind::Model => self.refresh_model_locked(work, model::now_ms()).map(drop),
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
        let mut state = lock(&self.live.state);
        let now = Instant::now();
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
                state.unconfirmed = [true; 2];
                self.queue(&mut state, now, false);
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

    /// Demand for the persisted metadata catalog must not synchronously load
    /// all event indices just because this process has no shown model yet.
    pub(crate) fn note_catalog_read(&self) -> io::Result<()> {
        self.catalog_demand.store(true, Ordering::Relaxed);
        self.note_read(Kind::Model)
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
        let mut work = lock(&self.work);
        // Logs invalidated before this refresh starts, it sees.
        let invalidated = std::mem::take(&mut lock(&self.live.state).invalidated);
        let started = Instant::now();
        let wanted = self.refresh_kind(&mut work, kind);
        let other = kind.other();
        let others = self
            .is_shown(other)
            .then(|| self.refresh_kind(&mut work, other));
        let built_at = work.built_at;
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
        if state.closed || self.refresh() == Refresh::OnRead {
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
        if self.refresh() == Refresh::OnRead {
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
            if state.closed || !state.active() || self.refresh() == Refresh::OnRead {
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
        self.tick();
        drop(checking);
    }

    /// One background check: rebuilds what is shown if its logs changed,
    /// never sooner than [`REBUILD_SPACING`] after the last rebuild, and
    /// queues the next. The model and the tree are refreshed, and fail,
    /// each on its own.
    fn tick(&self) {
        let mut work = lock(&self.work);
        let started = Instant::now();
        // A stall counts from here, with `work` taken: a warm or a page's
        // fresh check holding it meanwhile isn't this view's build hanging.
        lock(&self.live.state).running_since = Some(started);
        if let Some(built) = work.built_at
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
        let results: Vec<_> = [Kind::Model, Kind::Tree]
            .into_iter()
            .filter(|kind| {
                self.is_shown(*kind)
                    || (matches!(kind, Kind::Model) && self.catalog_demand.load(Ordering::Relaxed))
            })
            .map(|kind| (kind, self.refresh_kind(&mut work, kind)))
            .collect();
        let built_at = work.built_at;
        drop(work);
        let mut state = lock(&self.live.state);
        state.built_at = built_at;
        for (kind, result) in &results {
            state.record(*kind, result);
        }
        let failing = results.iter().any(|(_, result)| result.is_err());
        let due = self.next_due(&state, started, built_at, failing);
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
        Self::stop(&mut lock(&self.live.state));
    }

    /// Stops this view's background checks and waits out a check or a
    /// rebuild in progress; none starts again. The caller makes sure no
    /// read is in progress or starts after, as [`ViewerCore::close`] does:
    /// a read would still build a model it has none of.
    pub(crate) fn close(&self) {
        let mut state = lock(&self.live.state);
        Self::stop(&mut state);
        while state.slot == Slot::Running {
            state = self
                .live
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
        // A rebuild a read started has finished too.
        drop(lock(&self.work));
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
        if_none_match: Option<&str>,
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
        let answer = if path == "/api/model" {
            self.model(query, if_none_match)
        } else if path == "/api/tx"
            && query.split('&').any(|part| {
                part == "errors"
                    || part.starts_with("errors=")
                    || part == "signals"
                    || part.starts_with("signals=")
            })
        {
            self.tx_errors(query, if_none_match)
        } else {
            self.route(path, query)
                .map(|(status, content_type, body)| (status, content_type, body, None))
        };
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

    /// `/api/model`: the whole model, or 304 when the client's `ETag` or
    /// `?since=` version is still current.
    fn model(&self, query: &str, if_none_match: Option<&str>) -> io::Result<Routed> {
        let json = "application/json; charset=utf-8";
        let cache = self.served_model()?;
        let built = &cache.built;
        let etag = format!("\"{}\"", built.version);
        let since = query_value(query, "since").and_then(decoded);
        if if_none_match == Some(etag.as_str()) || since.as_deref() == Some(built.version.as_str())
        {
            return Ok((304, json, Vec::new(), Some(etag)));
        }
        Ok((
            200,
            json,
            built.json(model::now_ms()).into_bytes(),
            Some(etag),
        ))
    }

    /// `/api/tx?sid=&before=|after=|turn=`: a page of one session's
    /// transcript, from the model's index and the source lines it points
    /// to. No lock is held while the lines are read.
    fn tx(&self, query: &str) -> io::Result<String> {
        let sid = query_value(query, "sid")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("sid"))?;
        let index = |key: &str| {
            query_value(query, key)
                .map(|value| value.parse::<usize>().map_err(|_| invalid_input(key)))
                .transpose()
        };
        let turn = query_value(query, "turn").and_then(decoded);
        let anchor = tx::Anchor::of(index("before")?, index("after")?, turn)
            .ok_or_else(|| invalid_input("before, after and turn are exclusive"))?;
        let cache = self.served_model()?;
        tx::page(&cache.built, &sid, &anchor, model::now_ms())
    }

    /// `/api/tx?sid=&errors=1`: where the session's failed steps are
    /// ([`tx::errors`]), or 304 when the client's `ETag` or `?since=` version
    /// is still current. It names no page: `before`, `after` and `turn` are
    /// refused with it.
    fn tx_errors(&self, query: &str, if_none_match: Option<&str>) -> io::Result<Routed> {
        let json = "application/json; charset=utf-8";
        let signals = query_value(query, "signals").is_some();
        if query_value(query, if signals { "signals" } else { "errors" }) != Some("1")
            || (signals && query_value(query, "errors").is_some())
        {
            return Err(invalid_input("errors"));
        }
        if ["before", "after", "turn"]
            .iter()
            .any(|key| query_value(query, key).is_some())
        {
            return Err(invalid_input("errors names no page"));
        }
        let sid = query_value(query, "sid")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("sid"))?;
        let since = query_value(query, "since")
            .map(|value| decoded(value).ok_or_else(|| invalid_input("since")))
            .transpose()?;
        let cache = self.served_model()?;
        let built = &cache.built;
        if !built.tx.contains_key(&sid) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let etag = format!("\"{}\"", built.version);
        if if_none_match == Some(etag.as_str()) || since.as_deref() == Some(built.version.as_str())
        {
            return Ok((304, json, Vec::new(), Some(etag)));
        }
        Ok((
            200,
            json,
            if signals {
                tx::signals(built, &sid)?
            } else {
                tx::errors(built, &sid)?
            }
            .into_bytes(),
            Some(etag),
        ))
    }

    /// `/api/attachment?sid=&o=&b=&v=`: one image a prompt of session `sid`
    /// attaches, as `/api/tx` names it (its line's offset `o`, its block
    /// `b`, the version of its content `v`), decoded, sent as its exact
    /// type. Its URL names its content, so it may be cached (for a day); a line
    /// rewritten under the same offset no longer matches `v`, and is a 404
    /// until the page reads its new URL. The offset must be one of the
    /// session's own prompts in the model (a message of yours, or a prompt
    /// shown as one); the line is read back from that prompt's file, and the
    /// block must be an image [`crate::attachments::image`] serves: one of
    /// the four raster types (never SVG), whose bytes start with that type's
    /// signature, inline as valid base64, at most
    /// [`crate::attachments::IMAGE_MAX`] decoded. Anything else is 404, a
    /// malformed request 400.
    fn attachment(&self, query: &str) -> io::Result<(u16, &'static str, Vec<u8>)> {
        let sid = query_value(query, "sid")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("sid"))?;
        let number = |key: &str| {
            query_value(query, key)
                .filter(|value| {
                    (1..=20).contains(&value.len())
                        && value.bytes().all(|byte| byte.is_ascii_digit())
                })
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| invalid_input(key))
        };
        let offset = number("o")?;
        let block = usize::try_from(number("b")?).map_err(|_| invalid_input("b"))?;
        let version = query_value(query, "v")
            .filter(|value| {
                value.len() == 16
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .and_then(|value| u64::from_str_radix(value, 16).ok())
            .ok_or_else(|| invalid_input("v"))?;
        let cache = self.served_model()?;
        let built = &cache.built;
        let transcript = built.tx.get(&sid).ok_or(io::ErrorKind::NotFound)?;
        let mut files: Vec<&model::SlotFile> = Vec::new();
        for slot in &transcript.slots {
            let prompt = match &slot.kind {
                model::SlotKind::U => true,
                model::SlotKind::H(id) => model::is_ask(id),
                _ => false,
            };
            if !prompt || slot.offset != offset {
                continue;
            }
            if let Some(file) = slot.file.as_deref()
                && !files.iter().any(|seen| seen.path == file.path)
            {
                files.push(file);
            }
        }
        for file in files {
            if let Some((media, bytes)) = file.prompts.image(&file.path, offset, block, version) {
                return Ok((200, media, bytes));
            }
        }
        Err(io::ErrorKind::NotFound.into())
    }

    /// Whether a page URL names something in the model: `/machines/<id>`,
    /// `/s/<harness>/<id>` and `/trace/<harness>/<id>/<turn>`. Checked
    /// against the model brought up to date first (a stat pass, and a
    /// rebuild if the logs changed), whatever the refresh mode: outside
    /// OnRead only a URL the built model lacks comes here, and it may name
    /// a session newer than that model, even one a warm in progress has
    /// already taken the invalidation for. Such a check waits for that
    /// warm, then finds the session.
    fn page_exists(&self, path: &str) -> io::Result<bool> {
        let parts = page_parts(path).ok_or_else(|| invalid_input("path"))?;
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let cache = self.fresh_model(model::now_ms())?;
        Ok(has_page(&cache.built, &parts))
    }

    fn route(&self, path: &str, query: &str) -> io::Result<(u16, &'static str, Vec<u8>)> {
        let html = "text/html; charset=utf-8";
        let json = "application/json; charset=utf-8";
        match path {
            "/" | "/timeline" | "/analytics" | "/sessions" | "/machines" => {
                Ok((200, html, PAGE.into()))
            }
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
            "/api/transcript" => Ok((200, json, serde_json::to_vec(&self.transcript(query)?)?)),
            "/api/tx" => Ok((200, json, self.tx(query)?.into_bytes())),
            "/api/entry" => Ok((200, json, self.expand(query)?.into_bytes())),
            "/api/attachment" => self.attachment(query),
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
            _ if is_named_page(path) => {
                if self.page_exists(path)? {
                    Ok((200, html, PAGE.into()))
                } else {
                    Err(io::ErrorKind::NotFound.into())
                }
            }
            _ => Err(io::ErrorKind::NotFound.into()),
        }
    }

    fn transcript_path(&self, harness: &str, id: &str) -> io::Result<Option<PathBuf>> {
        if id.is_empty() || id.len() > 256 || !matches!(harness, "claude" | "codex" | "copilot") {
            return Ok(None);
        }
        let key = (harness.to_owned(), id.to_owned());
        let known = lock(&self.files).paths.get(&key).cloned();
        let machine = crate::facts::MachineFacts::of(&self.options);
        if let Some(path) = known
            && regular(&path)
            && (harness != "codex" || machine.codex_rollout_is_current(&self.options, &path))
        {
            return Ok(Some(path));
        }
        if self.refresh() == Refresh::Background {
            let mut files = lock(&self.files);
            if files
                .lookup_at
                .is_some_and(|when| when.elapsed() < REBUILD_SPACING)
            {
                return Ok(None);
            }
            // Reserve this pass before walking so concurrent unknown ids do
            // not each walk every machine's homes. On-read mode stays fresh.
            files.lookup_at = Some(Instant::now());
        }
        // Listed without the lock: another lookup isn't held up by a walk.
        let mut paths = Vec::new();
        file_list(
            &self.options.claude_home.join("projects"),
            &mut paths,
            "jsonl",
        )?;
        let mut codex_paths = Vec::new();
        for root in crate::inputs::codex_rollout_dirs(&self.options) {
            file_list(&root, &mut codex_paths, "jsonl")?;
        }
        paths.extend(
            codex_paths
                .into_iter()
                .filter(|path| machine.codex_rollout_is_current(&self.options, path)),
        );
        paths.extend(
            crate::inputs::inputs(&self.options)?
                .into_iter()
                .filter(|input| input.root == crate::inputs::InputRoot::Copilot)
                .map(|input| input.full_path(&self.options)),
        );
        self.update_files(paths);
        Ok(lock(&self.files).paths.get(&key).cloned())
    }

    /// Brings the transcript paths in line with `paths`, keeping the
    /// `.jsonl` files that are regular files. The files are looked at, and
    /// a new Codex file's first line read, without the `files` lock: it is
    /// held only to find which paths are new and to store the result.
    fn update_files(&self, paths: impl IntoIterator<Item = PathBuf>) {
        let (paths, new) = self.files_to_key(paths);
        let keys = self.keys_of(&new);
        self.store_files(paths, &new, keys);
    }

    /// The regular `.jsonl` files among `paths`, and those of them not yet
    /// known: the ones this update keys.
    fn files_to_key(
        &self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> (BTreeSet<PathBuf>, BTreeSet<PathBuf>) {
        let machine = crate::facts::MachineFacts::of(&self.options);
        let codex_dirs = crate::inputs::codex_rollout_dirs(&self.options);
        let paths: BTreeSet<_> = paths
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl") && regular(path))
            .filter(|path| {
                !codex_dirs.iter().any(|root| path.starts_with(root))
                    || machine.codex_rollout_is_current(&self.options, path)
            })
            .collect();
        let new = paths
            .difference(&lock(&self.files).known)
            .cloned()
            .collect();
        (paths, new)
    }

    fn keys_of(&self, new: &BTreeSet<PathBuf>) -> Vec<((String, String), PathBuf)> {
        new.iter()
            .filter_map(|path| transcript_key(&self.options, path).map(|key| (key, path.clone())))
            .collect()
    }

    /// Stores one update. Other updates may have stored in between, from
    /// older or newer listings, so a path is known afterwards only if it
    /// still was or this update looked at it (`looked`): a path no update
    /// has keyed stays new, and the next one keys it.
    fn store_files(
        &self,
        paths: BTreeSet<PathBuf>,
        looked: &BTreeSet<PathBuf>,
        keys: Vec<((String, String), PathBuf)>,
    ) {
        let mut files = lock(&self.files);
        files.paths.retain(|_, path| paths.contains(path));
        for (key, path) in keys {
            files.paths.insert(key, path);
        }
        let known = paths
            .into_iter()
            .filter(|path| files.known.contains(path) || looked.contains(path))
            .collect();
        files.known = known;
    }

    fn harness_offsets_cached(&self, path: &Path) -> io::Result<BTreeSet<u64>> {
        let metadata = fs::metadata(path)?;
        #[cfg(unix)]
        let identity = (metadata.dev(), metadata.ino());
        #[cfg(not(unix))]
        let identity: (u64, u64) = (0, 0);
        let cached = lock(&self.files)
            .harness
            .get(path)
            .filter(|(old_identity, _)| *old_identity == identity)
            .map(|(_, offsets)| offsets.clone());
        if let Some(offsets) = cached {
            return Ok(offsets);
        }
        let offsets = harness_offsets(path)?;
        lock(&self.files)
            .harness
            .insert(path.into(), (identity, offsets.clone()));
        Ok(offsets)
    }

    fn transcript(&self, query: &str) -> io::Result<TranscriptPage> {
        let harness = query_value(query, "harness")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("harness"))?;
        let id = query_value(query, "id")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("id"))?;
        let path = self
            .transcript_path(&harness, &id)?
            .ok_or(io::ErrorKind::NotFound)?;
        // Needed so liveness and child links are correct even when a
        // transcript page is opened directly, without a prior `/api/tree`
        // poll to warm the cache in this process. Refreshed on read, it is
        // cheap when nothing changed (a stat pass, no reparsing).
        let tree = self.served_tree()?;
        let node = find_node(&tree.roots, &harness, &id);
        let live = node.is_some_and(|node| node.pid.is_some() || node.state == "running");
        let links = node.map(tool_links).unwrap_or_default();
        let children = node
            .map(|node| {
                node.children
                    .iter()
                    .map(|child| ChildLink {
                        harness: child.harness.clone(),
                        id: child.id.clone(),
                        label: child.label.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let before = query_value(query, "before")
            .map(|value| value.parse::<u64>().map_err(|_| invalid_input("before")))
            .transpose()?;
        let after = query_value(query, "after")
            .map(|value| value.parse::<u64>().map_err(|_| invalid_input("after")))
            .transpose()?;
        if before.is_some() && after.is_some() {
            return Err(invalid_input("before and after are exclusive"));
        }
        let harness_offsets = if harness == "codex" {
            self.harness_offsets_cached(&path)?
        } else {
            BTreeSet::new()
        };
        let (mut page, _) = if let Some(after) = after {
            read_after(&path, &harness, after, &links, &harness_offsets)?
        } else {
            read_page(&path, &harness, before, &links, &harness_offsets)?
        };
        page.live = live;
        page.children = children;
        Ok(page)
    }

    /// `/api/entry`: one entry in full. With `sid`, `slot` and
    /// `as=in|out|diff|script`, one part of a transcript tool entry for "View all".
    fn expand(&self, query: &str) -> io::Result<String> {
        if let Some(part) = query_value(query, "as") {
            let sid = query_value(query, "sid")
                .and_then(decoded)
                .ok_or_else(|| invalid_input("sid"))?;
            let slot = query_value(query, "slot")
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| invalid_input("slot"))?;
            let cache = self.served_model()?;
            return tx::full_slot(&cache.built, &sid, slot, part);
        }
        let harness = query_value(query, "harness")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("harness"))?;
        let id = query_value(query, "id")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("id"))?;
        let offset = query_value(query, "offset")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| invalid_input("offset"))?;
        let block = query_value(query, "block")
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| invalid_input("block"))?;
        let path = self
            .transcript_path(&harness, &id)?
            .ok_or(io::ErrorKind::NotFound)?;
        let mut file = crate::sealed::LogFile::open(&path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = io::BufReader::new(file).take((MAX_LINE + 1) as u64);
        let mut bytes = Vec::new();
        io::BufRead::read_until(&mut reader, b'\n', &mut bytes)?;
        if bytes.len() > MAX_LINE || !bytes.ends_with(b"\n") {
            return Err(invalid_input("oversized or incomplete entry"));
        }
        let record: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_input("entry"))?;
        let source = expandable_text(&record, &harness, block)
            .ok_or_else(|| invalid_input("entry is not expandable"))?;
        let (text, truncated) = truncate(&source, EXPAND_BYTES);
        Ok(serde_json::json!({"text":text,"truncated":truncated}).to_string())
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
    /// This machine's model, read as `read` says.
    pub(crate) fn built(&self, read: Reading) -> io::Result<Arc<Built>> {
        let cache = match read {
            Reading::Served => self.served_model()?,
            Reading::At(now) => self.fresh_model(now)?,
        };
        Ok(cache.built.clone())
    }

    /// The model built last, as it is: no stat pass, no build, no lock but
    /// the one that clones the `Arc`. `None` before the first build.
    pub(crate) fn shown_built(&self) -> Option<Arc<Built>> {
        self.shown_model().map(|model| model.built.clone())
    }

    /// Brings the model up to date now, on the calling thread: built if
    /// there is none, rebuilt if its logs or facts changed (a stat pass
    /// tells), and the V1 tree with it if one is shown. It clears an
    /// invalidation it covers, as a background check does, so the next read
    /// after an idle spell answers at once. It is not a read: the view isn't
    /// marked read, so it starts no background checks. A closed view is left
    /// as it is. It holds `work` while it builds, as any rebuild does: the
    /// first read of a view with no model yet waits for it, then finds it
    /// done; a read once a model is shown never waits.
    pub(crate) fn warm(&self) -> io::Result<()> {
        if lock(&self.live.state).closed {
            return Ok(());
        }
        let mut work = lock(&self.work);
        // Logs invalidated before this refresh starts, it sees.
        let invalidated = std::mem::take(&mut lock(&self.live.state).invalidated);
        let model = self.refresh_kind(&mut work, Kind::Model);
        let tree = self
            .is_shown(Kind::Tree)
            .then(|| self.refresh_kind(&mut work, Kind::Tree));
        let built_at = work.built_at;
        drop(work);
        let mut state = lock(&self.live.state);
        state.built_at = built_at;
        state.record(Kind::Model, &model);
        if let Some(result) = &tree {
            state.record(Kind::Tree, result);
        }
        if model.is_err() {
            state.invalidated |= invalidated;
        }
        model
    }

    /// The V1 tree's roots, as a request is answered.
    pub(crate) fn tree_roots(&self) -> io::Result<Vec<Node>> {
        Ok(self.served_tree()?.roots.clone())
    }

    /// Whether this machine has a transcript file for a V1 tree node.
    pub(crate) fn has_transcript(&self, harness: &str, id: &str) -> io::Result<bool> {
        Ok(self.transcript_path(harness, id)?.is_some())
    }
}

/// Whether `path` is a page URL that names something: `/machines/<id>`,
/// `/s/<harness>/<id>` or `/trace/<harness>/<id>/<turn>`, by its prefix.
pub(crate) fn is_named_page(path: &str) -> bool {
    path.starts_with("/machines/") || path.starts_with("/s/") || path.starts_with("/trace/")
}

/// A page URL's path segments, decoded; `None` when one doesn't decode.
pub(crate) fn page_parts(path: &str) -> Option<Vec<String>> {
    path.trim_start_matches('/')
        .split('/')
        .map(decoded)
        .collect()
}

/// Whether one machine's `built` model has the page these decoded path
/// segments name (see [`page_parts`]).
pub(crate) fn has_page(built: &Built, parts: &[&str]) -> bool {
    match parts {
        ["machines", id] => *id == built.machine_id,
        ["s", harness, id] => has_session_page(built, harness, id),
        ["trace", harness, id, turn] => has_trace_page(built, harness, id, turn),
        _ => false,
    }
}

/// Whether `built` has the session page `/s/<harness>/<id>`.
pub(crate) fn has_session_page(built: &Built, harness: &str, id: &str) -> bool {
    built
        .sessions
        .get(id)
        .is_some_and(|session| session.harness == harness)
        || (harness == "claude" && id.starts_with("unsent:") && built.tx.contains_key(id))
}

/// Whether `built` has the trace page `/trace/<harness>/<id>/<turn>`.
pub(crate) fn has_trace_page(built: &Built, harness: &str, id: &str, turn: &str) -> bool {
    built
        .sessions
        .get(id)
        .is_some_and(|session| session.harness == harness)
        && built.tx.get(id).is_some_and(|transcript| {
            transcript
                .slots
                .iter()
                .any(|slot| slot.turn.as_deref() == Some(turn))
        })
}

fn find_node<'a>(nodes: &'a [Node], harness: &str, id: &str) -> Option<&'a Node> {
    for node in nodes {
        if node.harness == harness && node.id == id {
            return Some(node);
        }
        if let Some(found) = find_node(&node.children, harness, id) {
            return Some(found);
        }
    }
    None
}

fn tool_links(node: &Node) -> BTreeMap<String, Vec<String>> {
    let mut links = BTreeMap::<String, Vec<String>>::new();
    for child in &node.children {
        if let Some(tool) = &child.via_tool {
            links.entry(tool.id.clone()).or_default().push(format!(
                "/s/{}/{}",
                child.harness,
                percent_encode(&child.id)
            ));
        }
    }
    links
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

fn harness_offsets(path: &Path) -> io::Result<BTreeSet<u64>> {
    use io::BufRead;
    let mut reader = io::BufReader::new(crate::sealed::LogFile::open(path)?);
    let mut offsets = BTreeSet::new();
    let mut offset = 0;
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        if line.len() > MAX_LINE {
            break;
        }
        if let Ok(record) = serde_json::from_slice::<Value>(&line) {
            if is_agent_output(&record) {
                break;
            }
            if let Some(message) = user_text(&record) {
                if starts_with_tag(message) {
                    offsets.insert(offset);
                } else {
                    break;
                }
            }
        }
        offset += line.len() as u64;
        line.clear();
    }
    Ok(offsets)
}

fn starts_with_tag(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('<') else {
        return false;
    };
    let Some((tag, _)) = rest.split_once('>') else {
        return false;
    };
    !tag.is_empty()
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn truncate(value: &str, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value.into(), false);
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].into(), true)
}

fn entry(
    position: (u64, usize),
    kind: &str,
    text: String,
    name: Option<String>,
    tool_id: Option<String>,
    link: Option<String>,
    collapsed: bool,
) -> TranscriptEntry {
    let (offset, block) = position;
    let limit = if matches!(kind, "tool_use" | "tool_result") {
        PREVIEW_BYTES
    } else {
        PAGE_BYTES / 2
    };
    let (text, truncated) = truncate(&text, limit);
    TranscriptEntry {
        offset,
        block,
        kind: kind.into(),
        text,
        name,
        tool_id,
        links: link.iter().cloned().collect(),
        link,
        collapsed,
        truncated,
    }
}

fn content_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                field(part, "text")
                    .or_else(|| field(part, "content"))
                    .map(str::to_owned)
                    .unwrap_or_else(|| part.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

fn expandable_text(record: &Value, harness: &str, block: usize) -> Option<String> {
    if harness == "copilot" {
        let data = &record["data"];
        if field(record, "type") == Some("tool.execution_complete") {
            return crate::tx::result_text(record, 0);
        }
        if block == 0 {
            return field(data, "content").map(str::to_owned);
        }
        return data
            .get("toolRequests")?
            .as_array()?
            .get(block - 1)?
            .get("arguments")
            .map(content_text);
    }
    if harness == "claude" {
        let content = record.get("message")?.get("content")?;
        let item = if let Some(blocks) = content.as_array() {
            blocks.get(block)?
        } else if block == 0 {
            content
        } else {
            return None;
        };
        return match field(item, "type") {
            Some("tool_use") => item.get("input").map(content_text),
            Some("tool_result") => item.get("content").map(content_text),
            Some("text") => field(item, "text").map(str::to_owned),
            Some("thinking") => field(item, "thinking").map(str::to_owned),
            _ if item.is_string() => Some(content_text(item)),
            _ => None,
        };
    }
    if field(record, "type") == Some("event_msg") && block == 0 {
        let payload = record.get("payload")?;
        return field(payload, "message")
            .or_else(|| field(payload, "text"))
            .map(str::to_owned);
    }
    if field(record, "type") != Some("response_item") {
        return None;
    }
    let payload = record.get("payload")?;
    match field(payload, "type") {
        Some("function_call" | "custom_tool_call") if block == 0 => payload
            .get("arguments")
            .or_else(|| payload.get("input"))
            .map(content_text),
        Some("function_call_output" | "custom_tool_call_output") if block == 0 => {
            payload.get("output").map(content_text)
        }
        Some("reasoning") => payload
            .get("summary")?
            .as_array()?
            .get(block)
            .and_then(|part| field(part, "text").map(str::to_owned)),
        _ => payload
            .get("content")?
            .as_array()?
            .get(block)
            .and_then(|part| field(part, "text").map(str::to_owned)),
    }
}

fn entries(
    record: &Value,
    harness: &str,
    offset: u64,
    links: &BTreeMap<String, Vec<String>>,
    harness_message: bool,
) -> Vec<TranscriptEntry> {
    let mut result = Vec::new();
    if harness == "copilot" {
        let data = &record["data"];
        match field(record, "type") {
            Some("user.message" | "assistant.message") => {
                if let Some(content) = field(data, "content").filter(|content| !content.is_empty())
                {
                    result.push(entry(
                        (offset, 0),
                        if field(record, "type") == Some("user.message") {
                            "user"
                        } else {
                            "assistant"
                        },
                        content.into(),
                        None,
                        field(data, "messageId").map(str::to_owned),
                        None,
                        false,
                    ));
                }
                if field(record, "type") == Some("assistant.message") {
                    for (block, request) in data
                        .get("toolRequests")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        result.push(entry(
                            (offset, block + 1),
                            "tool_use",
                            request
                                .get("arguments")
                                .map(content_text)
                                .unwrap_or_default(),
                            field(request, "name").map(str::to_owned),
                            field(request, "toolCallId").map(str::to_owned),
                            None,
                            true,
                        ));
                    }
                }
            }
            Some("tool.execution_complete") => result.push(entry(
                (offset, 0),
                "tool_result",
                crate::tx::result_text(record, 0).unwrap_or_default(),
                None,
                field(data, "toolCallId").map(str::to_owned),
                None,
                true,
            )),
            _ => {}
        }
        return result;
    }
    if harness == "claude" {
        let Some(role) = field(record, "type").filter(|role| matches!(*role, "user" | "assistant"))
        else {
            return result;
        };
        let Some(message) = record.get("message") else {
            return result;
        };
        let Some(content) = message.get("content") else {
            return result;
        };
        let blocks: Vec<&Value> = match content {
            Value::Array(blocks) => blocks.iter().collect(),
            _ => vec![content],
        };
        for (block, item) in blocks.into_iter().enumerate() {
            match field(item, "type") {
                Some("text") => result.push(entry(
                    (offset, block),
                    role,
                    field(item, "text").unwrap_or("").into(),
                    None,
                    None,
                    None,
                    false,
                )),
                Some("thinking") => result.push(entry(
                    (offset, block),
                    "thinking",
                    field(item, "thinking").unwrap_or("").into(),
                    None,
                    None,
                    None,
                    true,
                )),
                Some("tool_use") => {
                    let id = field(item, "id").map(str::to_owned);
                    let child_links = id
                        .as_ref()
                        .and_then(|id| links.get(id))
                        .cloned()
                        .unwrap_or_default();
                    let link = child_links.first().cloned();
                    let mut call = entry(
                        (offset, block),
                        "tool_use",
                        item.get("input").map(content_text).unwrap_or_default(),
                        field(item, "name").map(str::to_owned),
                        id,
                        link,
                        true,
                    );
                    call.links = child_links;
                    result.push(call);
                }
                Some("tool_result") => result.push(entry(
                    (offset, block),
                    "tool_result",
                    item.get("content").map(content_text).unwrap_or_default(),
                    None,
                    field(item, "tool_use_id").map(str::to_owned),
                    None,
                    true,
                )),
                _ if item.is_string() => result.push(entry(
                    (offset, block),
                    role,
                    content_text(item),
                    None,
                    None,
                    None,
                    false,
                )),
                _ => {}
            }
        }
    } else {
        let payload = &record["payload"];
        match field(record, "type") {
            Some("response_item") => match field(payload, "type") {
                Some("function_call" | "custom_tool_call") => result.push(entry(
                    (offset, 0),
                    "tool_use",
                    payload
                        .get("arguments")
                        .or_else(|| payload.get("input"))
                        .map(content_text)
                        .unwrap_or_default(),
                    field(payload, "name").map(str::to_owned),
                    field(payload, "call_id").map(str::to_owned),
                    None,
                    true,
                )),
                Some("function_call_output" | "custom_tool_call_output") => result.push(entry(
                    (offset, 0),
                    "tool_result",
                    payload.get("output").map(content_text).unwrap_or_default(),
                    None,
                    field(payload, "call_id").map(str::to_owned),
                    None,
                    true,
                )),
                Some("reasoning") => {
                    if let Some(summary) = payload.get("summary").and_then(Value::as_array) {
                        for (block, part) in summary.iter().enumerate() {
                            if let Some(text) = field(part, "text") {
                                result.push(entry(
                                    (offset, block),
                                    "reasoning",
                                    text.into(),
                                    None,
                                    None,
                                    None,
                                    true,
                                ));
                            }
                        }
                    }
                }
                _ => {
                    if let Some(role) =
                        field(payload, "role").filter(|role| matches!(*role, "user" | "assistant"))
                        && let Some(content) = payload.get("content").and_then(Value::as_array)
                    {
                        for (block, part) in content.iter().enumerate() {
                            if let Some(text) = field(part, "text") {
                                result.push(entry(
                                    (offset, block),
                                    if harness_message && role == "user" {
                                        "harness"
                                    } else {
                                        role
                                    },
                                    text.into(),
                                    None,
                                    None,
                                    None,
                                    harness_message,
                                ));
                            }
                        }
                    }
                }
            },
            Some("event_msg") => {
                let kind = match field(payload, "type") {
                    Some("user_message") => {
                        if harness_message {
                            "harness"
                        } else {
                            "user"
                        }
                    }
                    Some("agent_message") => "assistant",
                    Some("agent_reasoning" | "agent_reasoning_raw_content") => "reasoning",
                    _ => return result,
                };
                let text = field(payload, "message")
                    .or_else(|| field(payload, "text"))
                    .unwrap_or("");
                result.push(entry(
                    (offset, 0),
                    kind,
                    text.into(),
                    None,
                    None,
                    None,
                    matches!(kind, "reasoning" | "harness"),
                ));
            }
            _ => {}
        }
    }
    result
}

const READ_CHUNK: u64 = 64 * 1024;

struct ReverseReader {
    file: crate::sealed::LogFile,
    length: u64,
    cursor: u64,
    buffer_start: u64,
    buffer: Vec<u8>,
    bytes_read: usize,
}

impl ReverseReader {
    fn open(path: &Path, cursor: u64) -> io::Result<Self> {
        let file = crate::sealed::LogFile::open(path)?;
        let length = file.metadata()?.len();
        if cursor > length {
            return Err(invalid_input("before beyond end"));
        }
        Ok(Self {
            file,
            length,
            cursor,
            buffer_start: 0,
            buffer: Vec::new(),
            bytes_read: 0,
        })
    }

    fn byte_at(&mut self, position: u64) -> io::Result<u8> {
        if self.buffer.is_empty()
            || position < self.buffer_start
            || position >= self.buffer_start + self.buffer.len() as u64
        {
            let start = position / READ_CHUNK * READ_CHUNK;
            let length = (self.length - start).min(READ_CHUNK) as usize;
            self.buffer.resize(length, 0);
            self.file.seek(SeekFrom::Start(start))?;
            self.file.read_exact(&mut self.buffer)?;
            self.bytes_read += length;
            self.buffer_start = start;
        }
        Ok(self.buffer[(position - self.buffer_start) as usize])
    }

    fn complete_end(&mut self) -> io::Result<u64> {
        let mut position = self.cursor;
        while position > 0 {
            if self.byte_at(position - 1)? == b'\n' {
                self.cursor = position;
                return Ok(position);
            }
            let local_end = (position - self.buffer_start) as usize;
            if let Some(index) = self.buffer[..local_end]
                .iter()
                .rposition(|byte| *byte == b'\n')
            {
                self.cursor = self.buffer_start + index as u64 + 1;
                return Ok(self.cursor);
            }
            position = self.buffer_start;
        }
        self.cursor = 0;
        Ok(0)
    }

    fn previous_line(&mut self) -> io::Result<Option<(u64, Vec<u8>, bool)>> {
        if self.cursor == 0 {
            return Ok(None);
        }
        let mut end = self.cursor;
        if self.byte_at(end - 1)? == b'\n' {
            end -= 1;
        }
        let mut position = end;
        let mut parts = Vec::new();
        let mut size = 0;
        let mut oversized = false;
        loop {
            if position == 0 {
                self.cursor = 0;
                break;
            }
            self.byte_at(position - 1)?;
            let local_end = (position - self.buffer_start) as usize;
            if let Some(index) = self.buffer[..local_end]
                .iter()
                .rposition(|byte| *byte == b'\n')
            {
                let part = &self.buffer[index + 1..local_end];
                size += part.len();
                if size <= MAX_LINE {
                    parts.push(part.to_vec());
                } else {
                    oversized = true;
                }
                self.cursor = self.buffer_start + index as u64 + 1;
                break;
            }
            size += local_end;
            if size <= MAX_LINE {
                parts.push(self.buffer[..local_end].to_vec());
            } else {
                oversized = true;
                parts.clear();
            }
            position = self.buffer_start;
        }
        if oversized {
            return Ok(Some((self.cursor, Vec::new(), true)));
        }
        parts.reverse();
        Ok(Some((self.cursor, parts.concat(), false)))
    }
}

fn read_page(
    path: &Path,
    harness: &str,
    before: Option<u64>,
    links: &BTreeMap<String, Vec<String>>,
    harness_messages: &BTreeSet<u64>,
) -> io::Result<(TranscriptPage, usize)> {
    let length = fs::metadata(path)?.len();
    let mut reader = ReverseReader::open(path, before.unwrap_or(length))?;
    let end = if before.is_some() {
        reader.cursor
    } else {
        reader.complete_end()?
    };
    if before.is_some() && end > 0 && reader.byte_at(end - 1)? != b'\n' {
        return Err(invalid_input("before is not a line boundary"));
    }
    let mut groups = Vec::<Vec<TranscriptEntry>>::new();
    let mut count = 0;
    let mut bytes = 0;
    while reader.cursor > 0 && count < PAGE_ENTRIES && bytes < PAGE_BYTES {
        let saved = reader.cursor;
        let Some((offset, raw, oversized)) = reader.previous_line()? else {
            break;
        };
        let mut group = if oversized {
            vec![entry(
                (offset, 0),
                "malformed",
                format!("[malformed record at byte {offset}]"),
                None,
                None,
                None,
                false,
            )]
        } else if let Ok(record) = serde_json::from_slice::<Value>(&raw) {
            if record.is_object() {
                entries(
                    &record,
                    harness,
                    offset,
                    links,
                    harness_messages.contains(&offset),
                )
            } else {
                vec![entry(
                    (offset, 0),
                    "malformed",
                    format!("[malformed record at byte {offset}]"),
                    None,
                    None,
                    None,
                    false,
                )]
            }
        } else {
            vec![entry(
                (offset, 0),
                "malformed",
                format!("[malformed record at byte {offset}]"),
                None,
                None,
                None,
                false,
            )]
        };
        let group_bytes = serde_json::to_vec(&group)?.len();
        if !groups.is_empty()
            && (count + group.len() > PAGE_ENTRIES || bytes + group_bytes > PAGE_BYTES)
        {
            reader.cursor = saved;
            break;
        }
        if group_bytes > PAGE_BYTES || group.len() > PAGE_ENTRIES {
            group.truncate(PAGE_ENTRIES);
            for item in &mut group {
                if item.text.len() > 1024 {
                    item.text = truncate(&item.text, 1024).0;
                    item.truncated = true;
                }
            }
        }
        count += group.len();
        bytes += group_bytes;
        groups.push(group);
    }
    groups.reverse();
    Ok((
        TranscriptPage {
            entries: groups.into_iter().flatten().collect(),
            before: (reader.cursor > 0).then_some(reader.cursor),
            end,
            live: false,
            children: Vec::new(),
        },
        reader.bytes_read,
    ))
}

fn read_after(
    path: &Path,
    harness: &str,
    after: u64,
    links: &BTreeMap<String, Vec<String>>,
    harness_messages: &BTreeSet<u64>,
) -> io::Result<(TranscriptPage, usize)> {
    let mut file = crate::sealed::LogFile::open(path)?;
    let length = file.metadata()?.len();
    if after > length {
        return Err(invalid_input("after beyond end"));
    }
    file.seek(SeekFrom::Start(after))?;
    let mut reader = io::BufReader::with_capacity(READ_CHUNK as usize, file);
    let mut end = after;
    let mut entries_out = Vec::new();
    let mut page_bytes = 0;
    let mut raw = Vec::new();
    while end < length && entries_out.len() < PAGE_ENTRIES && page_bytes < PAGE_BYTES {
        raw.clear();
        let size = io::BufRead::read_until(&mut reader, b'\n', &mut raw)?;
        if size == 0 || !raw.ends_with(b"\n") {
            break;
        }
        let offset = end;
        let group = if raw.len() > MAX_LINE {
            vec![entry(
                (offset, 0),
                "malformed",
                format!("[malformed record at byte {offset}]"),
                None,
                None,
                None,
                false,
            )]
        } else if let Ok(record) = serde_json::from_slice::<Value>(&raw) {
            if record.is_object() {
                entries(
                    &record,
                    harness,
                    offset,
                    links,
                    harness_messages.contains(&offset),
                )
            } else {
                vec![entry(
                    (offset, 0),
                    "malformed",
                    format!("[malformed record at byte {offset}]"),
                    None,
                    None,
                    None,
                    false,
                )]
            }
        } else {
            vec![entry(
                (offset, 0),
                "malformed",
                format!("[malformed record at byte {offset}]"),
                None,
                None,
                None,
                false,
            )]
        };
        let group_bytes = serde_json::to_vec(&group)?.len();
        if !entries_out.is_empty()
            && (entries_out.len() + group.len() > PAGE_ENTRIES
                || page_bytes + group_bytes > PAGE_BYTES)
        {
            break;
        }
        entries_out.extend(group);
        end += size as u64;
        page_bytes += group_bytes;
    }
    Ok((
        TranscriptPage {
            entries: entries_out,
            before: None,
            end,
            live: false,
            children: Vec::new(),
        },
        (end - after) as usize,
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use serde_json::json;

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn formats_build_timing_line() {
        assert_eq!(
            timing_line(123, &[("scan", 20), ("index_clone", 3), ("sessions", 8)]),
            "semon sessions viewer: build total=123 scan=20 index_clone=3 sessions=8"
        );
    }

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
        let request = request.replace("PORT", &port.to_string());
        let worker = thread::spawn(move || viewer.handle(server.recv().unwrap()));
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        worker.join().unwrap();
        String::from_utf8_lossy(&response).into_owned()
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
    fn model_is_json_with_headers_etag_and_304() {
        let fixture = Fixture::new();
        fixture.write("proc/sys/kernel/hostname", "testbox\n");
        fixture.claude(
            "xss",
            &[json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":"xss","origin":{"kind":"human"},
                "message":{"role":"user","content":"<script>alert(1)</script>"}})],
        );
        let token = "0123456789abcdef0123456789abcdef";
        let get = |extra: &str, query: &str| {
            http(
                &fixture,
                &format!(
                    "GET /api/model?t={token}{query} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n{extra}\r\n"
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
        let model: Value = serde_json::from_str(body).unwrap();
        for key in [
            "version", "now", "machine", "sessions", "handoffs", "turns", "busy",
        ] {
            assert!(model.get(key).is_some(), "missing {key}");
        }
        assert_eq!(model["handoffs"][0]["brief"], "<script>alert(1)</script>");
        let version = model["version"].as_str().unwrap();
        let etag = format!("\"{version}\"");
        assert!(wire.contains(&format!("ETag: {etag}")));
        let cached = get(&format!("If-None-Match: {etag}\r\n"), "");
        assert!(cached.starts_with("HTTP/1.1 304"));
        assert!(cached.split_once("\r\n\r\n").unwrap().1.is_empty());
        assert!(get("", &format!("&since={version}")).starts_with("HTTP/1.1 304"));
        assert!(get("", "&since=stale").starts_with("HTTP/1.1 200"));
        let refused = http(
            &fixture,
            "GET /api/model HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n",
        );
        assert!(refused.starts_with("HTTP/1.1 403"));
    }

    #[test]
    fn model_api_version_is_served() {
        let fixture = Fixture::new();
        let core = ViewerCore::new(fixture.options.clone());
        let reply = core.respond("GET", "/api/model", "", None);
        assert_eq!(reply.status, 200);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["api"], crate::MODEL_API);
    }

    /// Live polling (#40 M3): `?since=` answers 304 while nothing changed,
    /// without parsing a line; any appended line changes the version, even
    /// one that changes nothing else in the model, and the rebuild parses
    /// that line only and reads no other file's metadata.
    #[test]
    fn polls_are_304_until_a_line_lands_and_the_rebuild_reads_only_it() {
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
        let version = |routed: &Routed| {
            serde_json::from_slice::<Value>(&routed.2).unwrap()["version"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let parsed = || crate::events::PARSED.with(std::cell::Cell::get);
        let metas = || crate::model::META_READS.with(std::cell::Cell::get);
        let marks =
            |routed: &Routed| serde_json::from_slice::<Value>(&routed.2).unwrap()["tx"].clone();
        let append = |path: &Path, line: Value| {
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{line}").unwrap();
        };
        let first = viewer.model("", None).unwrap();
        let marks1 = marks(&first);
        assert!(
            marks1["lane"].is_string() && marks1["other"].is_string() && marks1["run"].is_string()
        );
        assert_eq!(first.0, 200);
        let v1 = version(&first);
        let (lines, reads) = (parsed(), metas());
        for _ in 0..3 {
            let poll = viewer.model(&format!("since={v1}"), None).unwrap();
            assert_eq!(poll.0, 304);
            assert!(poll.2.is_empty());
            assert_eq!(poll.3, Some(format!("\"{v1}\"")));
        }
        assert_eq!(
            (parsed(), metas()),
            (lines, reads),
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
        let changed = viewer.model(&format!("since={v1}"), None).unwrap();
        assert_eq!(changed.0, 200);
        let v2 = version(&changed);
        assert_ne!(v2, v1, "an appended line changes the version");
        assert_eq!(changed.3, Some(format!("\"{v2}\"")));
        assert_eq!(parsed(), lines + 1, "only the appended line is parsed");
        assert_eq!(metas(), reads, "no unchanged file's metadata is read again");
        // Only the grown transcript's mark moves: the page tails only it.
        let marks2 = marks(&changed);
        assert_ne!(marks2["lane"], marks1["lane"]);
        assert_eq!(marks2["other"], marks1["other"]);
        assert_eq!(marks2["run"], marks1["run"]);
        assert_eq!(viewer.model(&format!("since={v2}"), None).unwrap().0, 304);
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
        let quiet = viewer.model(&format!("since={v2}"), None).unwrap();
        assert_eq!(quiet.0, 200, "any appended line changes the version");
        let v3 = version(&quiet);
        assert_ne!(v3, v2);
        assert_eq!(parsed(), lines + 2);
        assert_eq!(viewer.model(&format!("since={v3}"), None).unwrap().0, 304);

        // A Codex file that grows keeps its session_meta: its line is parsed,
        // its metadata isn't read again.
        append(
            &codex,
            json!({"type":"response_item","timestamp":"2026-09-24T00:06:00Z",
                "payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}),
        );
        let grown = viewer.model(&format!("since={v3}"), None).unwrap();
        assert_eq!(grown.0, 200);
        assert_eq!(parsed(), lines + 3);
        assert_eq!(
            metas(),
            reads,
            "a Codex file that grew keeps its session_meta"
        );
        // A subagent's .meta.json that changes is read again, and only it.
        fs::write(
            &meta,
            r#"{"agentType":"general-purpose","description":"Checker, renamed"}"#,
        )
        .unwrap();
        let renamed = viewer
            .model(&format!("since={}", version(&grown)), None)
            .unwrap();
        assert_eq!(renamed.0, 200);
        assert_eq!(metas(), reads + 1, "only the changed .meta.json is read");
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
        let model = |routed: &Routed| serde_json::from_slice::<Value>(&routed.2).unwrap();
        let alive = model(&viewer.model("", None).unwrap());
        assert_eq!(alive["sessions"]["lane"]["state"], "work");
        let mark = alive["tx"]["lane"].as_str().unwrap().to_owned();
        assert!(mark.ends_with(".1"), "one running call: {mark}");
        fs::remove_file(stat).unwrap();
        let version = alive["version"].as_str().unwrap();
        let dead = viewer.model(&format!("since={version}"), None).unwrap();
        assert_eq!(dead.0, 200, "the process ending is a change");
        let dead = model(&dead);
        assert_ne!(dead["sessions"]["lane"]["state"], "work");
        let after = dead["tx"]["lane"].as_str().unwrap();
        assert!(after.ends_with(".0"), "no running call: {after}");
        assert_eq!(
            after.rsplit_once('.').unwrap().0,
            mark.rsplit_once('.').unwrap().0,
            "no line was written"
        );
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
        let model = |routed: &Routed| serde_json::from_slice::<Value>(&routed.2).unwrap();
        let alive = model(&viewer.model("", None).unwrap());
        assert_eq!(alive["sessions"]["run"]["state"], "work");
        let mark = alive["tx"]["run"].as_str().unwrap().to_owned();
        assert!(mark.ends_with(".1"), "one running step: {mark}");
        fixture.write("proc/locks", "");
        let version = alive["version"].as_str().unwrap();
        let dead = viewer.model(&format!("since={version}"), None).unwrap();
        assert_eq!(dead.0, 200, "the process ending is a change");
        let dead = model(&dead);
        assert_ne!(dead["sessions"]["run"]["state"], "work");
        let after = dead["tx"]["run"].as_str().unwrap();
        assert!(after.ends_with(".0"), "no running step: {after}");
        assert_eq!(
            after.rsplit_once('.').unwrap().0,
            mark.rsplit_once('.').unwrap().0,
            "no line was written"
        );
    }

    /// Two ways a poll could miss or waste a build: a line that lands while a
    /// build runs is seen by the next poll (the files are stamped before the
    /// build reads them), and a lock anywhere else on the machine doesn't
    /// rebuild (only the Codex writer locks' lines of /proc/locks count).
    #[cfg(unix)]
    #[test]
    fn a_line_during_a_build_is_seen_and_other_locks_dont_rebuild() {
        let fixture = lane_fixture();
        let lock = fixture.write("codex/thread-writer-locks/run.lock", "");
        let path = fixture.root.join("claude/projects/project/lane.jsonl");
        let viewer = fixture.viewer();
        let builds = || crate::model::BUILDS.with(std::cell::Cell::get);
        let version = |routed: &Routed| {
            serde_json::from_slice::<Value>(&routed.2).unwrap()["version"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let v1 = version(&viewer.model("", None).unwrap());

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
        let during = viewer.model(&format!("since={v1}"), None).unwrap();
        assert_eq!(during.0, 200);
        assert!(!String::from_utf8_lossy(&during.2).contains("landed mid-build"));
        let v2 = version(&during);
        let next = viewer.model(&format!("since={v2}"), None).unwrap();
        assert_eq!(next.0, 200, "the line that landed mid-build is a change");
        assert!(String::from_utf8_lossy(&next.2).contains("landed mid-build"));
        let v3 = version(&next);
        assert_eq!(viewer.model(&format!("since={v3}"), None).unwrap().0, 304);

        // A lock on some other file: no build.
        let before = builds();
        fixture.write(
            "proc/locks",
            "1: POSIX  ADVISORY  WRITE 999 08:01:424242 0 EOF\n",
        );
        assert_eq!(viewer.model(&format!("since={v3}"), None).unwrap().0, 304);
        assert_eq!(builds(), before, "an unrelated lock rebuilt the model");
        // The writer lock taken: a build.
        let (major, minor, ino) = crate::lock_identity(&lock).unwrap();
        fixture.write(
            "proc/locks",
            &format!("1: POSIX  ADVISORY  WRITE 999 08:01:424242 0 EOF\n2: FLOCK  ADVISORY  WRITE 1234 {major:02x}:{minor:02x}:{ino} 0 EOF\n"),
        );
        viewer.model(&format!("since={v3}"), None).unwrap();
        assert_eq!(
            builds(),
            before + 1,
            "the writer lock didn't rebuild the model"
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
        let model: Value = serde_json::from_str(
            &viewer
                .model("", None)
                .map(|routed| String::from_utf8(routed.2).unwrap())
                .unwrap(),
        )
        .unwrap();
        let turn = model["turns"][0]["id"].as_str().unwrap().to_owned();
        for path in [
            "/".to_owned(),
            "/timeline".into(),
            "/sessions".into(),
            "/machines".into(),
            "/machines/testbox".into(),
            "/s/claude/lane".into(),
            format!("/s/claude/lane?turn={turn}"),
            format!("/trace/claude/lane/{turn}"),
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
            "/s/claude/nobody",
            "/s/codex/lane",
            "/s/claude/lane/more",
            "/trace/claude/lane/no-such-turn",
            "/trace/claude/nobody/x",
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
                "GET /timeline HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n"
            )
            .starts_with("HTTP/1.1 403")
        );
    }

    #[test]
    fn assets_and_fonts_carry_the_headers() {
        let fixture = lane_fixture();
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
            ("/api/model", "application/json; charset=utf-8"),
            ("/api/tx?sid=lane", "application/json; charset=utf-8"),
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
            "src/lib/attachments.tsx",
            "src/lib/session-menu.tsx",
            "src/lib/sheets.tsx",
            "src/lib/analytics.tsx",
            "src/lib/layout.ts",
            "src/lib/trace.tsx",
            "src/lib/sentence.tsx",
            "src/lib/transcript.tsx",
            "src/lib/viewer-bar.tsx",
            "src/lib/placeholder.tsx",
            "src/lib/geometry.ts",
            "src/lib/select.tsx",
            "src/lib/tooltip.tsx",
            "src/lib/model.ts",
            "src/lib/live.ts",
            "src/lib/routes.ts",
            "src/lib/paging.ts",
            "src/lib/ordering.ts",
        ] {
            assert!(
                app.contains(&format!("  // {module}\n")),
                "{module} must be scanned"
            );
        }
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
    fn transcript_content_is_served_as_data() {
        let fixture = lane_fixture();
        let wire = get(&fixture, "/api/tx?sid=lane");
        let page: Value = serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap();
        let entries = page["entries"].as_array().unwrap();
        assert_eq!(entries[0]["k"], "h");
        assert_eq!(entries[1]["text"], "<img src=x onerror=alert(2)>");
        let model = get(&fixture, "/api/model");
        let model: Value = serde_json::from_str(model.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(model["handoffs"][0]["brief"], "<script>alert(1)</script>");
        for query in [
            "/api/tx",
            "/api/tx?sid=lane&before=x",
            "/api/tx?sid=lane&before=1&after=1",
        ] {
            assert!(get(&fixture, query).starts_with("HTTP/1.1 400"), "{query}");
        }
        assert!(get(&fixture, "/api/tx?sid=nobody").starts_with("HTTP/1.1 404"));
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

    fn tx_entries(core: &MachineView, sid: &str) -> (String, Vec<Value>) {
        let reply = core.respond("GET", "/api/tx", &format!("sid={sid}"), None);
        assert_eq!(reply.status, 200);
        let body = String::from_utf8(reply.body).unwrap();
        let page: Value = serde_json::from_str(&body).unwrap();
        (body, page["entries"].as_array().unwrap().clone())
    }

    /// A servable image's reference, as `/api/tx` names it.
    fn served_ref(o: u64, b: usize, media: &str, bytes: &[u8], size: (u32, u32)) -> Value {
        json!({"b":b,"type":media,"size":bytes.len(),"v":v(&b64(bytes)),"w":size.0,"h":size.1,"o":o})
    }

    #[test]
    fn a_prompts_images_are_named_by_reference_never_inlined() {
        let (fixture, at) = images_fixture();
        let core = fixture.viewer();
        let (body, entries) = tx_entries(&core, "pics");
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
        // Every entry with images is your message; its text is the model's, without the placeholder.
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        let briefs: Vec<&str> = model["handoffs"]
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

    #[test]
    fn an_attachment_is_served_only_as_an_indexed_prompts_image() {
        let (fixture, at) = images_fixture();
        let core = fixture.viewer();
        let fetch = |query: &str| core.respond("GET", "/api/attachment", query, None);
        let url = |sid: &str, o: u64, b: usize, data: &str| {
            format!("sid={sid}&o={o}&b={b}&v={}", v(data))
        };
        for (o, b, media, bytes) in [
            (at[0], 0, "image/png", PNG),
            (at[2], 0, "image/jpeg", JPEG),
            (at[3], 0, "image/png", PNG),
            (at[7], 0, "image/gif", GIF),
            (at[7], 1, "image/webp", WEBP),
        ] {
            let reply = fetch(&url("pics", o, b, &b64(bytes)));
            assert_eq!(
                (reply.status, reply.content_type, reply.body.as_slice()),
                (200, media, bytes),
                "{media}"
            );
            assert_eq!(reply.headers(), ATTACHMENT_HEADERS.as_slice());
        }
        let png = b64(PNG);
        let refused = [
            // Another session, or none: the offset must be one of this session's prompts.
            url("other", at[0], 0, &png),
            url("nobody", at[0], 0, &png),
            // A line that is no prompt (the reply, the tool result beside its image), or no line.
            url("pics", at[1], 0, &png),
            url("pics", at[6], 1, &png),
            url("pics", at[0] + 1, 0, &png),
            // A block that is text, or past the end.
            url("pics", at[0], 1, &png),
            url("pics", at[0], 99, &png),
            // Content other than the URL names: another image's version.
            url("pics", at[0], 0, &b64(JPEG)),
            // SVG, an SVG declared as PNG, a type that isn't an image, malformed base64, a URL.
            url("pics", at[4], 0, &b64(SVG)),
            url("pics", at[4], 1, &b64(SVG)),
            url("pics", at[4], 2, &png),
            url("pics", at[4], 3, &format!("{}*AAA", &png[..8])),
            url("pics", at[4], 4, ""),
        ];
        for query in &refused {
            let reply = fetch(query);
            assert_eq!(reply.status, 404, "{query}");
            assert_eq!(reply.content_type, "text/plain; charset=utf-8", "{query}");
            assert_eq!(reply.headers(), SECURITY_HEADERS.as_slice(), "{query}");
        }
        let good = v(&png);
        for query in [
            format!("o=0&b=0&v={good}"),
            format!("sid=pics&b=0&v={good}"),
            format!("sid=pics&o={}&v={good}", at[0]),
            format!("sid=pics&o={}&b=0", at[0]),
            format!("sid=pics&o=x&b=0&v={good}"),
            format!("sid=pics&o=-1&b=0&v={good}"),
            format!("sid=pics&o=+0&b=0&v={good}"),
            format!("sid=pics&o=0&b=1e2&v={good}"),
            format!("sid=pics&o=0&b=&v={good}"),
            format!("sid=pics&o=99999999999999999999999&b=0&v={good}"),
            format!("sid=pics&o=0&b=0&v={}", good.to_uppercase()),
            format!("sid=pics&o=0&b=0&v={}", &good[..15]),
            format!("sid=pics&o=0&b=0&v={good}0"),
            "sid=pics&o=0&b=0&v=zzzzzzzzzzzzzzzz".into(),
        ] {
            assert_eq!(fetch(&query).status, 400, "{query}");
        }
        assert_eq!(
            core.respond(
                "POST",
                "/api/attachment",
                &url("pics", at[0], 0, &png),
                None
            )
            .status,
            405
        );
        // On the wire: the exact type, nosniff, cached privately for a day, a CSP of its own; nothing else is cached.
        let wire = get(
            &fixture,
            &format!("/api/attachment?{}", url("pics", at[0], 0, &png)),
        );
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
        let missing = get(
            &fixture,
            &format!("/api/attachment?{}", url("pics", at[4], 0, &b64(SVG))),
        );
        assert!(missing.starts_with("HTTP/1.1 404"));
        for header in HEADERS {
            assert!(missing.contains(header), "missing {header}");
        }
        // The token guards it as it guards the rest.
        assert!(
            http(
                &fixture,
                &format!(
                    "GET /api/attachment?{} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n",
                    url("pics", at[0], 0, &png)
                )
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
        let old = format!("sid=swap&o=0&b=0&v={}", v(&b64(GIF)));
        assert_eq!(
            core.respond("GET", "/api/attachment", &old, None).status,
            200
        );
        let path = fixture.claude("swap", &[line(other)]);
        assert_eq!(offsets(&path), [0]);
        let (_, entries) = tx_entries(&core, "swap");
        let image = &entries.iter().find_map(|entry| entry.get("img")).unwrap()[0];
        assert_eq!(image["v"], v(&b64(other)));
        assert_eq!(image["w"], 7);
        assert_eq!(
            core.respond("GET", "/api/attachment", &old, None).status,
            404
        );
        let new = format!("sid=swap&o=0&b=0&v={}", v(&b64(other)));
        assert_eq!(
            core.respond("GET", "/api/attachment", &new, None).body,
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
        let (_, entries) = tx_entries(&core, "big");
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
        let served = core.respond(
            "GET",
            "/api/attachment",
            &format!("sid=big&o=0&b=0&v={}", v(&exact)),
            None,
        );
        assert_eq!((served.status, served.body.len()), (200, cap));
        let refused = format!("sid=big&o={}&b=0&v={}", at[1], v(&over));
        assert_eq!(
            core.respond("GET", "/api/attachment", &refused, None)
                .status,
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
        let (body, entries) = tx_entries(&core, "cdx");
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
        let query = format!("sid=cdx&o={}&b=1&v={}", at[1], v(&b64(PNG)));
        let reply = core.respond("GET", "/api/attachment", &query, None);
        assert_eq!(
            (reply.status, reply.content_type, reply.body.as_slice()),
            (200, "image/png", PNG)
        );
        let local = format!("sid=cdx&o={}&b=3&v={}", at[1], v("/home/me/shot.png"));
        assert_eq!(
            core.respond("GET", "/api/attachment", &local, None).status,
            404
        );
    }

    /// The model's per-session `calls` and `errors` are the totals `/api/tx`
    /// reports, so a page needn't fetch a transcript to count.
    #[test]
    fn the_models_counts_are_the_transcript_pages_totals() {
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
        let body = |path: &str| -> Value {
            let wire = get(&fixture, path);
            serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap()
        };
        let model = body("/api/model");
        let page = body("/api/tx?sid=counted");
        assert_eq!(page["calls"], 2);
        assert_eq!(page["errors"], 1);
        let session = &model["sessions"]["counted"];
        assert_eq!(session["calls"], page["calls"]);
        assert_eq!(session["errors"], page["errors"]);
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
        let core = fixture.viewer();
        let reply = core.respond("GET", "/api/tx", "sid=signals&signals=1", None);
        assert_eq!(reply.status, 200);
        let body = body_of(&reply);
        assert_eq!(body["signals"], 1);
        assert_eq!(body["slots"].as_array().unwrap().len(), 1);
        let query = format!(
            "sid=signals&signals=1&since={}",
            body["version"].as_str().unwrap()
        );
        assert_eq!(core.respond("GET", "/api/tx", &query, None).status, 304);
        for query in [
            "sid=signals&signals=0",
            "sid=signals&signals=1&before=2",
            "sid=signals&signals=1&errors=1",
        ] {
            assert_eq!(core.respond("GET", "/api/tx", query, None).status, 400);
        }
    }

    #[test]
    fn failed_steps_are_listed_where_the_badge_counts_them() {
        let fixture = faults_fixture();
        let core = fixture.viewer();
        let body = |core: &MachineView, query: &str| -> Value {
            let reply = core.respond("GET", "/api/tx", query, None);
            assert_eq!(reply.status, 200, "{query}");
            serde_json::from_slice(&reply.body).unwrap()
        };
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        let list = body(&core, "sid=faults&errors=1");
        assert_eq!(list["errors"], 5);
        assert_eq!(list["errors"], model["sessions"]["faults"]["errors"]);
        assert_eq!(list["truncated"], false);
        assert_eq!(list["version"], model["version"]);
        let slots: Vec<u64> = list["slots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|slot| slot.as_u64().unwrap())
            .collect();
        // Every failed step of the whole transcript, read page by page.
        let mut failed = Vec::new();
        let mut page = body(&core, "sid=faults");
        assert_eq!(list["total"], page["total"]);
        loop {
            let entries = page["entries"].as_array().unwrap();
            let mut here: Vec<u64> = entries
                .iter()
                .filter(|entry| entry["k"] == "tool" && entry["ok"] == false)
                .map(|entry| entry["slot"].as_u64().unwrap())
                .collect();
            here.append(&mut failed);
            failed = here;
            if page["from"] == 0 {
                break;
            }
            page = body(&core, &format!("sid=faults&before={}", page["from"]));
        }
        assert_eq!(slots, failed);
        assert_eq!(slots.len(), 5);
        assert!(
            slots.last().unwrap() - slots.first().unwrap() > 2 * tx::PAGE_ENTRIES as u64,
            "the failures span pages: {slots:?}"
        );
        for slot in slots {
            let after = body(&core, &format!("sid=faults&after={slot}"));
            let first = &after["entries"][0];
            assert_eq!(
                (first["slot"].as_u64(), &first["ok"]),
                (Some(slot), &json!(false))
            );
            let before = body(&core, &format!("sid=faults&before={}", slot + 1));
            let last = before["entries"].as_array().unwrap().last().unwrap();
            assert_eq!(
                (last["slot"].as_u64(), &last["ok"]),
                (Some(slot), &json!(false))
            );
        }
        // The one never answered is among them, as the badge counts it.
        let unfinished = body(&core, "sid=faults&errors=1")["slots"][2]
            .as_u64()
            .unwrap();
        let page = body(&core, &format!("sid=faults&after={unfinished}"));
        assert_eq!(page["entries"][0]["unfinished"], true);
    }

    #[test]
    fn the_error_list_refuses_bad_requests_and_unknown_sessions() {
        let fixture = faults_fixture();
        let core = fixture.viewer();
        for query in [
            "errors=1",
            "sid=faults&errors=0",
            "sid=faults&errors",
            "sid=faults&errors=",
            "sid=faults&errors=yes",
            "sid=faults&errors=1&before=3",
            "sid=faults&errors=1&after=3",
            "sid=faults&errors=1&turn=x",
            "sid=%zz&errors=1",
            "sid=faults&errors=1&since=%zz",
        ] {
            let reply = core.respond("GET", "/api/tx", query, None);
            assert_eq!((reply.status, reply.etag), (400, None), "{query}");
        }
        assert_eq!(
            core.respond("GET", "/api/tx", "sid=nobody&errors=1", None)
                .status,
            404
        );
        // Not even a matching ETag answers for a session that isn't there.
        let etag = core
            .respond("GET", "/api/tx", "sid=faults&errors=1", None)
            .etag
            .unwrap();
        let reply = core.respond("GET", "/api/tx", "sid=nobody&errors=1", Some(&etag));
        assert_eq!(reply.status, 404);
        assert!(get(&fixture, "/api/tx?sid=faults&errors=2").starts_with("HTTP/1.1 400"));
        assert!(get(&fixture, "/api/tx?sid=nobody&errors=1").starts_with("HTTP/1.1 404"));
    }

    /// The list carries the model's `ETag`: 304 while it holds (by
    /// `If-None-Match` or `since=`), a new list once a failure lands.
    #[test]
    fn the_error_list_is_304_until_the_model_moves() {
        let fixture = faults_fixture();
        let core = fixture.viewer();
        let first = core.respond("GET", "/api/tx", "sid=faults&errors=1", None);
        assert_eq!(first.status, 200);
        let etag = first.etag.clone().unwrap();
        let version = body_of(&first)["version"].as_str().unwrap().to_owned();
        assert_eq!(etag, format!("\"{version}\""));
        let cached = core.respond("GET", "/api/tx", "sid=faults&errors=1", Some(&etag));
        assert_eq!((cached.status, cached.body.len()), (304, 0));
        assert_eq!(cached.etag.as_deref(), Some(etag.as_str()));
        let since = core.respond(
            "GET",
            "/api/tx",
            &format!("sid=faults&errors=1&since={version}"),
            None,
        );
        assert_eq!((since.status, since.body.len()), (304, 0));
        // Over the wire too, with the header.
        let token = "0123456789abcdef0123456789abcdef";
        let wire = http(
            &fixture,
            &format!(
                "GET /api/tx?sid=faults&errors=1&t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\nIf-None-Match: {etag}\r\n\r\n"
            ),
        );
        assert!(wire.starts_with("HTTP/1.1 304"), "{wire}");
        assert!(wire.contains(&format!("ETag: {etag}")));
        // One more failed call lands: a new version and a longer list.
        let path = fixture.root.join("claude/projects/project/faults.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", json!({"type":"assistant","timestamp":"2026-09-24T02:00:00Z","sessionId":"faults",
            "message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu-late","name":"Bash","input":{"command":"late"}}]}})).unwrap();
        writeln!(file, "{}", json!({"type":"user","timestamp":"2026-09-24T02:00:01Z","sessionId":"faults","toolUseResult":{},
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu-late","content":"no","is_error":true}]}})).unwrap();
        drop(file);
        let mut moved = None;
        for _ in 0..50 {
            let reply = core.respond("GET", "/api/tx", "sid=faults&errors=1", Some(&etag));
            if reply.status == 200 {
                moved = Some(reply);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let moved = moved.expect("the list never moved");
        assert_ne!(moved.etag.as_deref(), Some(etag.as_str()));
        let list = body_of(&moved);
        assert_eq!(list["errors"], 6);
        let slots = list["slots"].as_array().unwrap();
        assert_eq!(slots.len(), 6);
        assert_eq!(
            slots[5].as_u64().unwrap() + 1,
            list["total"].as_u64().unwrap()
        );
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(model["sessions"]["faults"]["errors"], 6);
    }

    /// The list stops at its cap and says so; the count stays whole.
    #[test]
    fn the_error_list_is_capped_and_says_so() {
        assert_eq!(tx::ERRORS_MAX, 10_000);
        let fixture = faults_fixture();
        let core = fixture.viewer();
        let whole: Value = serde_json::from_str(
            &tx::errors(&core.built(Reading::At(model::now_ms())).unwrap(), "faults").unwrap(),
        )
        .unwrap();
        let capped: Value = serde_json::from_str(
            &tx::errors_limited(
                &core.built(Reading::At(model::now_ms())).unwrap(),
                "faults",
                2,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(capped["errors"], 5);
        assert_eq!(capped["truncated"], true);
        assert_eq!(
            capped["slots"].as_array().unwrap()[..],
            whole["slots"].as_array().unwrap()[..2]
        );
        assert_eq!(whole["truncated"], false);
        let exact: Value = serde_json::from_str(
            &tx::errors_limited(
                &core.built(Reading::At(model::now_ms())).unwrap(),
                "faults",
                5,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(exact["truncated"], false);
        assert!(matches!(
            tx::errors(&core.built(Reading::At(model::now_ms())).unwrap(), "nobody")
                .map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        ));
    }

    /// The core answers without a transport: the same bodies, statuses and
    /// `ETag` the loopback server sends, GET only, and the headers it sends
    /// are the ones a server embedding the core is given.
    #[test]
    fn the_core_answers_without_a_transport() {
        let fixture = lane_fixture();
        let core = fixture.viewer();
        let page = core.respond("GET", "/timeline", "", None);
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
        let model = core.respond("GET", "/api/model", "", None);
        assert_eq!(model.status, 200);
        let etag = model.etag.clone().unwrap();
        let body: Value = serde_json::from_slice(&model.body).unwrap();
        assert_eq!(etag, format!("\"{}\"", body["version"].as_str().unwrap()));
        let cached = core.respond("GET", "/api/model", "", Some(&etag));
        assert_eq!((cached.status, cached.body.len()), (304, 0));
        assert_eq!(
            core.respond("GET", "/api/tx", "sid=lane&before=x", None)
                .status,
            400
        );
        assert_eq!(core.respond("GET", "/elsewhere", "", None).status, 404);
        for method in ["POST", "HEAD", "PUT", "DELETE"] {
            let refused = core.respond(method, "/", "", None);
            assert_eq!(refused.status, 405, "{method}");
        }
        // The loopback server sends exactly these, and the same body.
        let wire = get(&fixture, "/timeline");
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

        let model = core.respond("GET", "/api/model", "", None);
        assert_eq!(model.status, 200);
        let model: Value = serde_json::from_slice(&model.body).unwrap();
        let turn = model["turns"][0]["id"].as_str().unwrap();

        let paths = [
            "/".to_owned(),
            "/timeline".into(),
            "/analytics".into(),
            "/sessions".into(),
            "/machines".into(),
            "/machines/testbox".into(),
            "/s/claude/lane".into(),
            format!("/trace/claude/lane/{turn}"),
        ];
        for path in paths {
            assert!(crate::shell::is_page_path(&path), "{path}");
            let reply = core.respond("GET", &path, "", None);
            assert_eq!(reply.status, 200, "{path}");
            assert_eq!(reply.body, crate::shell::PAGE_HTML.as_bytes(), "{path}");
        }

        // The predicate checks dynamic route shapes without querying the model.
        assert!(crate::shell::is_page_path("/s/claude/not-in-model"));
        for path in [
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
    fn transcript_pages_backward_with_malformed_lines() {
        let fixture = Fixture::new();
        let path = fixture.claude("many", &[]);
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        for number in 1..=430 {
            if number == 221 {
                writeln!(file, "{{bad json").unwrap();
            } else {
                writeln!(file, "{}", json!({"type":"user","message":{"content":[{"type":"text","text":format!("message {number}")}]}})).unwrap();
            }
        }
        let viewer = fixture.viewer();
        let mut before = None;
        let mut entries = Vec::new();
        loop {
            let query = format!(
                "harness=claude&id=many{}",
                before
                    .map(|cursor| format!("&before={cursor}"))
                    .unwrap_or_default()
            );
            let page = viewer.transcript(&query).unwrap();
            assert!(page.entries.len() <= PAGE_ENTRIES);
            assert!(serde_json::to_vec(&page).unwrap().len() <= PAGE_BYTES + 1024);
            entries.extend(page.entries);
            before = page.before;
            if before.is_none() {
                break;
            }
        }
        assert_eq!(entries.len(), 430);
        let malformed = entries
            .iter()
            .find(|entry| entry.kind == "malformed")
            .unwrap();
        assert_eq!(
            malformed.text,
            format!("[malformed record at byte {}]", malformed.offset)
        );
        assert_eq!(entries.iter().map(|entry| entry.offset).min(), Some(0));
        assert!(entries.iter().map(|entry| entry.offset).max().unwrap() > malformed.offset);
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
        let page = viewer.transcript("harness=claude&id=parent").unwrap();
        assert_eq!(page.children.len(), 2);
        assert!(
            page.entries
                .iter()
                .any(|item| item.kind == "thinking" && item.collapsed)
        );
        assert!(page.entries.iter().any(|item| item.kind == "tool_result" && item.tool_id.as_deref() == Some("task-1")));
        assert_eq!(
            page.entries
                .iter()
                .find(|item| item.tool_id.as_deref() == Some("task-1") && item.kind == "tool_use")
                .unwrap()
                .link
                .as_deref(),
            Some("/s/claude/child")
        );
        assert_eq!(
            page.entries
                .iter()
                .find(|item| item.tool_id.as_deref() == Some("bash-1"))
                .unwrap()
                .link
                .as_deref(),
            Some("/s/codex/run")
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
        let page = viewer.transcript("harness=claude&id=parent").unwrap();
        let call = page
            .entries
            .iter()
            .find(|item| item.tool_id.as_deref() == Some("launch"))
            .unwrap();
        assert_eq!(call.links, ["/s/codex/one", "/s/codex/two"]);
        assert_eq!(call.link.as_deref(), Some("/s/codex/one"));
        assert_eq!(page.children.len(), 2);
    }

    #[test]
    fn codex_blocks_and_first_turn_harness_messages() {
        let fixture = Fixture::new();
        fixture.codex("codex", &[
            json!({"type":"event_msg","payload":{"type":"user_message","message":"<recommended_plugins>injected</recommended_plugins>"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"real prompt"}}),
            json!({"type":"event_msg","payload":{"type":"agent_reasoning","text":"summary"}}),
            json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"summary block"}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"<later>tag</later>"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"reply"}]}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"call-1","arguments":"ls"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"file"}}),
        ]);
        let viewer = fixture.viewer();
        let page = viewer.transcript("harness=codex&id=codex").unwrap();
        assert!(
            page.entries
                .iter()
                .any(|item| item.kind == "harness" && item.collapsed)
        );
        assert!(
            page.entries
                .iter()
                .any(|item| item.kind == "user" && item.text == "<later>tag</later>")
        );
        for kind in ["assistant", "reasoning", "tool_use", "tool_result"] {
            assert!(
                page.entries.iter().any(|item| item.kind == kind),
                "missing {kind}"
            );
        }
        assert!(
            page.entries
                .iter()
                .any(|item| item.kind == "reasoning" && item.text == "summary block")
        );
    }

    #[test]
    fn xss_payloads_are_text_and_tool_expansion_is_capped() {
        let fixture = Fixture::new();
        fixture.write("claude/sessions/secret.key", "SECRET_KEY_NEVER_READ");
        let long = "x".repeat(PREVIEW_BYTES + 1);
        fixture.claude("xss", &[claude_record("xss", vec![
            json!({"type":"text","text":"<script>alert(1)</script>"}),
            json!({"type":"tool_use","id":"call","name":"Bash","input":{"value":format!("\"><img onerror=alert(1)>{long}")}}),
            json!({"type":"tool_result","tool_use_id":"call","content":format!("<script>{long}")}),
        ])]);
        let viewer = fixture.viewer();
        let page = viewer.transcript("harness=claude&id=xss").unwrap();
        assert!(
            page.entries
                .iter()
                .any(|item| item.text == "<script>alert(1)</script>")
        );
        assert!(page.entries.iter().filter(|item| item.truncated).count() >= 2);
        let output = page
            .entries
            .iter()
            .find(|item| item.kind == "tool_result")
            .unwrap();
        let expanded = viewer
            .expand(&format!(
                "harness=claude&id=xss&offset={}&block={}",
                output.offset, output.block
            ))
            .unwrap();
        assert!(expanded.contains("<script>"));
        assert!(expanded.len() > PREVIEW_BYTES);
        let token = "0123456789abcdef0123456789abcdef";
        let wire = http(
            &fixture,
            &format!(
                "GET /api/transcript?harness=claude&id=xss&t={token} HTTP/1.1\r\nHost: 127.0.0.1:PORT\r\n\r\n"
            ),
        );
        let body = wire.split_once("\r\n\r\n").unwrap().1;
        assert!(body.contains("<script>alert(1)</script>"));
        assert!(!wire.contains("SECRET_KEY_NEVER_READ"));
        let (capped, truncated) = truncate(&"x".repeat(EXPAND_BYTES + 1), EXPAND_BYTES);
        assert!(truncated);
        assert_eq!(capped.len(), EXPAND_BYTES);
        assert!(!application_script(crate::shell::VIEWER_JS).contains("innerHTML"));
        assert!(crate::shell::VIEWER_JS.contains("textContent"));
    }

    #[test]
    fn codex_subagent_and_live_claude_links() {
        let fixture = Fixture::new();
        fixture.codex("root", &[]);
        fixture.write("codex/sessions/2026/09/24/rollout-child.jsonl", &format!("{}\n", json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":{"id":"child","session_id":"root","parent_thread_id":"root","thread_source":"subagent","agent_nickname":"worker"}})));
        let viewer = fixture.viewer();
        let page = viewer.transcript("harness=codex&id=root").unwrap();
        assert_eq!(page.children.len(), 1);
        assert_eq!(page.children[0].id, "child");

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
        let page = viewer.transcript("harness=claude&id=live").unwrap();
        assert!(page.live);
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
            let line = json!({"type":"user","message":{"content":[{"type":"text","text":format!("line {number} {padding}")}]}}).to_string();
            written += line.len() as u64 + 1;
            writeln!(file, "{line}").unwrap();
            number += 1;
        }
        drop(file);
        assert!(fs::metadata(&path).unwrap().len() >= 8 * 1024 * 1024);
        let (page, bytes_read) =
            read_page(&path, "claude", None, &BTreeMap::new(), &BTreeSet::new()).unwrap();
        assert!(!page.entries.is_empty());
        assert!(page.entries.len() <= PAGE_ENTRIES);
        assert!(
            bytes_read <= PAGE_BYTES + READ_CHUNK as usize,
            "last page of an 8 MB file read {bytes_read} bytes, expected at most {}",
            PAGE_BYTES + READ_CHUNK as usize
        );
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

    /// `now` moves between two calls; everything else must not.
    fn without_now(body: &[u8]) -> String {
        let text = String::from_utf8(body.to_vec()).unwrap();
        let (head, rest) = text.split_once("\"now\":").unwrap();
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        format!("{head}\"now\":0{rest}")
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
        let model: serde_json::Value =
            serde_json::from_slice(&core.respond("GET", "/api/model", "", None).body).unwrap();
        assert_eq!(
            model["sessions"]["first-session"]["machine"].as_str(),
            Some(identity.1.as_str())
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
    fn one_machine_through_with_machines_is_todays_viewer_byte_for_byte() {
        let fixture = machine("laptop", "lane");
        let today = fixture.viewer();
        let mut core =
            ViewerCore::with_machines(vec![("some-key".into(), fixture.options.clone())]);
        assert_eq!(
            core.session_machine_key("lane").unwrap().as_deref(),
            Some("some-key")
        );
        // `--serve`'s mode answers the same bytes.
        let mut background =
            ViewerCore::with_machines(vec![("some-key".into(), fixture.options.clone())]);
        background.set_refresh(Refresh::Background);
        for (path, query) in [
            ("/api/model", ""),
            ("/api/tx", "sid=lane"),
            ("/api/tx", "sid=lane&errors=1"),
            ("/api/tree", ""),
            ("/s/claude/lane", ""),
            ("/machines/laptop", ""),
            ("/viewer.js", ""),
        ] {
            let a = today.respond("GET", path, query, None);
            for b in [
                core.respond("GET", path, query, None),
                background.respond("GET", path, query, None),
            ] {
                assert_eq!(
                    (a.status, a.content_type, a.etag.clone()),
                    (b.status, b.content_type, b.etag.clone()),
                    "{path}"
                );
                if path == "/api/model" {
                    assert_eq!(without_now(&a.body), without_now(&b.body));
                } else if path == "/api/tree" {
                    assert_eq!(without_age(&a.body), without_age(&b.body), "{path}");
                } else {
                    assert_eq!(a.body, b.body, "{path}");
                }
            }
        }
        background.close();
        // No admin link unless the embedder sets one, and then first.
        assert!(
            !String::from_utf8_lossy(&core.respond("GET", "/api/model", "", None).body)
                .contains("\"admin\"")
        );
        core.set_admin_link(crate::AdminLink::new("Manage machines", "/admin/machines"));
        let linked = core.respond("GET", "/api/model", "", None);
        assert!(linked.body.starts_with(
            b"{\"admin\":{\"label\":\"Manage machines\",\"href\":\"/admin/machines\"},\"api\":1,\"version\":"
        ));
    }

    #[test]
    fn account_and_nav_are_per_request_model_fields_outside_the_version() {
        let fixture = lane_fixture();
        let mut core = crate::ViewerCore::new(fixture.options.clone());
        let plain = core.respond("GET", "/api/model", "", None);
        assert_eq!(plain.status, 200);
        let plain_model: Value = serde_json::from_slice(&plain.body).unwrap();
        assert!(plain_model.get("account").is_none());
        assert!(plain_model.get("nav").is_none());

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
        let first = core.respond("GET", "/api/model", "", None);
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
            first_model["nav"],
            json!({"machines":"/account/workspaces"})
        );
        let version = first_model["version"].as_str().unwrap();
        let etag = first.etag.clone().unwrap();
        assert_eq!(etag, format!("\"{version}\""));

        core.set_account(Some(menu("Another account")));
        let second = core.respond("GET", "/api/model", "", None);
        assert_eq!(second.status, 200);
        let second_model: Value = serde_json::from_slice(&second.body).unwrap();
        assert_eq!(second_model["account"]["name"], "Another account");
        assert_eq!(second_model["version"], version);
        assert_eq!(second.etag.as_deref(), Some(etag.as_str()));

        let unchanged = core.respond("GET", "/api/model", "", Some(&etag));
        assert_eq!(unchanged.status, 304);
        assert!(unchanged.body.is_empty());
        assert_eq!(unchanged.etag.as_deref(), Some(etag.as_str()));

        // Per request, without touching the shared core: its own extras.
        let mut extras = crate::Extras::default();
        extras.set_account(Some(menu("Per request")));
        let own = core.respond_with(&extras, "GET", "/api/model", "", None);
        let own_model = body_of(&own);
        assert_eq!(own_model["account"]["name"], "Per request");
        assert!(own_model.get("nav").is_none());
        assert_eq!(own_model["version"], version);
        assert_eq!(own.etag.as_deref(), Some(etag.as_str()));
        let shared = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(shared["account"]["name"], "Another account");
        assert_eq!(core.extras().clone(), {
            let mut set = crate::Extras::default();
            set.set_account(Some(menu("Another account")));
            assert!(set.set_nav_override("machines", "/account/workspaces"));
            set
        });
    }

    #[test]
    fn several_machines_serve_one_model_and_each_answers_for_its_own() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        let reply = core.respond("GET", "/api/model", "", None);
        assert_eq!(reply.status, 200);
        let model = body_of(&reply);
        let ids: Vec<&str> = model["machines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["alpha", "bravo"]);
        assert_eq!(model["machine"]["id"], "alpha");
        assert_eq!(model["sessions"]["lane-a"]["machine"], "alpha");
        assert_eq!(model["sessions"]["lane-b"]["machine"], "bravo");
        assert!(model["busy"]["alpha"].is_array() && model["busy"]["bravo"].is_array());
        // Each machine's stub for the gone sender is its own: never merged.
        let sessions = model["sessions"].as_object().unwrap();
        let stubs: Vec<(&String, &Value)> =
            sessions.iter().filter(|(_, s)| s["stub"] == true).collect();
        assert_eq!(stubs.len(), 2, "{sessions:?}");
        for (id, stub) in &stubs {
            assert!(
                id.ends_with(&format!("@{}", stub["machine"].as_str().unwrap())),
                "{id}"
            );
        }
        // Every handoff and turn names a session the union serves.
        for handoff in model["handoffs"].as_array().unwrap() {
            for end in ["from", "to"] {
                if let Some(id) = handoff[end].as_str().filter(|id| *id != "you") {
                    assert!(sessions.contains_key(id), "{end} {id}");
                }
            }
        }
        for turn in model["turns"].as_array().unwrap() {
            assert!(sessions.contains_key(turn["sid"].as_str().unwrap()));
        }
        // Each machine answers for its own sessions.
        let tx = core.respond("GET", "/api/tx", "sid=lane-b", None);
        assert_eq!(tx.status, 200);
        assert!(String::from_utf8_lossy(&tx.body).contains("done on bravo"));
        assert_eq!(
            core.respond("GET", "/api/tx", "sid=nobody", None).status,
            404
        );
        for (path, status) in [
            ("/s/claude/lane-a", 200),
            ("/s/claude/lane-b", 200),
            ("/s/claude/nobody", 404),
            ("/machines/alpha", 200),
            ("/machines/bravo", 200),
            ("/machines/gamma", 404),
            ("/timeline", 200),
            ("/analytics", 200),
        ] {
            assert_eq!(core.respond("GET", path, "", None).status, status, "{path}");
        }
        // The ETag covers every machine: a line on either is a new version.
        let version = model["version"].as_str().unwrap().to_owned();
        assert_eq!(
            core.respond("GET", "/api/model", &format!("since={version}"), None)
                .status,
            304
        );
        let path = bravo.root.join("claude/projects/project/lane-b.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{}", json!({"type":"user","timestamp":"2026-09-24T00:05:00Z","sessionId":"lane-b","origin":{"kind":"human"},"message":{"role":"user","content":"more"}})).unwrap();
        drop(file);
        let moved = core.respond("GET", "/api/model", &format!("since={version}"), None);
        assert_eq!(moved.status, 200);
        assert_ne!(body_of(&moved)["version"], version.as_str());
    }

    #[test]
    fn a_session_id_two_machines_share_is_refused_not_picked() {
        let (alpha, bravo) = (machine("alpha", "same"), machine("bravo", "same"));
        let core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        let model = core.respond("GET", "/api/model", "", None);
        assert_eq!(model.status, 409);
        let ids = body_of(&model)["ids"].clone();
        assert!(
            ids.as_array().unwrap().iter().any(|id| id == "same"),
            "{ids}"
        );
        assert_eq!(core.respond("GET", "/api/tx", "sid=same", None).status, 409);
        assert_eq!(core.respond("GET", "/s/claude/same", "", None).status, 404);
        assert_eq!(
            core.respond("GET", "/api/analytics", "range=7d", None)
                .status,
            409
        );
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

    fn analytics_of(core: &ViewerCore, query: &str) -> Value {
        let reply = core.respond("GET", "/api/analytics", query, None);
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
    fn analytics_ranges_count_what_the_models_day_leaves_out() {
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert!(model["sessions"].get("today").is_some());
        assert!(model["sessions"].get("older").is_none(), "{model}");
        let day = analytics_of(&core, "range=24h");
        let week = analytics_of(&core, "range=7d");
        let month = analytics_of(&core, "range=30d");
        for (answer, range, started, minutes) in [
            (&day, "24h", 1, 2),
            (&week, "7d", 2, 6),
            (&month, "30d", 4, 14),
        ] {
            assert_eq!(answer["range"], range);
            assert_eq!(answer["version"], model["version"], "{range}");
            assert_eq!(answer["current"]["started"], started, "{range}");
            assert_eq!(answer["current"]["turns"], started, "{range}");
            assert_eq!(answer["current"]["agent_ms"], minutes * MINUTE, "{range}");
        }
        assert!(week["sessions"].get("older").is_some());
        assert!(day["sessions"].get("older").is_none());
        let busy: Vec<&str> = week["top"]["busy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["sid"].as_str().unwrap())
            .collect();
        assert_eq!(busy, ["older", "today"]);
        // The previous period: the 7 days before the week hold `before`.
        assert_eq!(week["previous"]["started"], 1);
        assert_eq!(week["previous"]["agent_ms"], 3 * MINUTE);
        assert_eq!(day["previous"]["started"], 0);
        assert_eq!(month["previous"]["started"], 0);
        assert_eq!(week["facets"]["machine"], json!(["testbox"]));
    }

    #[test]
    fn analytics_refuses_anything_but_a_range_and_the_filters() {
        let fixture = dated("testbox", model::now_ms(), &[("today", 60 * MINUTE, 2)]);
        let core = ViewerCore::new(fixture.options.clone());
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
            let reply = core.respond("GET", "/api/analytics", query, None);
            assert_eq!(reply.status, 400, "{query:?}");
            assert_eq!(reply.etag, None);
        }
        assert_eq!(
            core.respond("POST", "/api/analytics", "range=7d", None)
                .status,
            405
        );
        assert!(!crate::shell::is_page_path("/api/analytics"));
    }

    /// A poll with the answer's `ETag` is a 304 until the logs change; then
    /// the answer is computed again, once, and has a new `ETag`.
    #[test]
    fn analytics_polls_are_304_until_the_logs_change() {
        let now = model::now_ms();
        let fixture = dated("testbox", now, &[("today", 30 * MINUTE, 2)]);
        let core = ViewerCore::new(fixture.options.clone());
        let first = core.respond("GET", "/api/analytics", "range=7d", None);
        assert_eq!(first.status, 200);
        let etag = first.etag.clone().expect("an ETag");
        let again = core.respond("GET", "/api/analytics", "range=7d", Some(&etag));
        assert_eq!((again.status, again.body.len()), (304, 0));
        assert_eq!(again.etag.as_deref(), Some(etag.as_str()));
        // Another range, or other filters, is another answer.
        let month = core.respond("GET", "/api/analytics", "range=30d", Some(&etag));
        assert_eq!(month.status, 200);
        assert_ne!(month.etag.as_deref(), Some(etag.as_str()));
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
        let moved = eventually("a new answer after the logs changed", || {
            let reply = core.respond("GET", "/api/analytics", "range=7d", Some(&etag));
            (reply.status == 200).then_some(reply)
        });
        let answer = body_of(&moved);
        // The busy interval now runs to the new line, 26 minutes ago.
        assert_eq!(answer["current"]["agent_ms"], 4 * MINUTE);
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(answer["version"], model["version"]);
        let latest = moved.etag.clone().unwrap();
        assert_eq!(
            core.respond("GET", "/api/analytics", "range=7d", Some(&latest))
                .status,
            304
        );
    }

    /// A filter value no session has is answered, empty, with a 200.
    #[test]
    fn analytics_answers_a_filter_no_session_has_with_nothing() {
        let fixture = dated("testbox", model::now_ms(), &[("today", 60 * MINUTE, 2)]);
        let core = ViewerCore::new(fixture.options.clone());
        let known = analytics_of(&core, "range=7d&machine=testbox");
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
            assert_eq!(answer["facets"]["machine"], json!(["testbox"]), "{query}");
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
        let built = MachineView::new(fixture.options.clone())
            .built(Reading::At(now))
            .unwrap();
        let rows = &built.activity;
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        let week = analytics_of(&core, "range=7d");
        assert_eq!(week["version"], model["version"]);
        assert_eq!(week["current"]["started"], 2);
        assert_eq!(week["facets"]["machine"], json!(["alpha", "bravo"]));
        let machines: Vec<&Value> = week["breakdown"]["machine"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| &group["machine"])
            .collect();
        assert_eq!(machines, [&json!("alpha"), &json!("bravo")]);
        let bravo_only = analytics_of(&core, "range=7d&machine=bravo");
        assert_eq!(bravo_only["current"]["started"], 1);
        assert_eq!(bravo_only["current"]["agent_ms"], 2 * MINUTE);
        let day = analytics_of(&core, "range=24h");
        assert_eq!(day["current"]["started"], 1);
        assert!(day["sessions"].get("a-old").is_none());
        // One machine through `with_machines` answers as it does alone.
        let alone = ViewerCore::with_machines(vec![("k".into(), alpha.options.clone())]);
        let single = ViewerCore::new(alpha.options.clone());
        let (a, b) = (
            analytics_of(&alone, "range=30d"),
            analytics_of(&single, "range=30d"),
        );
        assert_eq!(a["current"], b["current"]);
        assert_eq!(a["version"], b["version"]);
        assert_eq!(a["facets"]["machine"], json!(["alpha"]));
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        let machines = model["machines"].as_array().unwrap();
        assert_eq!(machines[0]["id"], "twin");
        assert_eq!(machines[1]["id"], "twin~two");
        assert_eq!(machines[1]["name"], "twin");
        assert_eq!(machines[0]["up"], true);
        assert!(machines[0].get("last").is_none());
        assert_eq!(machines[1]["up"], false);
        assert_eq!(machines[1]["last"], 1_790_000_000_000_i64);
        assert_eq!(model["sessions"]["lane-2"]["machine"], "twin~two");
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

    /// The model's machines: `machines`, or with one machine `machine`.
    fn machine_ids(model: &Value) -> Vec<String> {
        match model["machines"].as_array() {
            Some(machines) => machines
                .iter()
                .map(|machine| machine["id"].as_str().unwrap().to_owned())
                .collect(),
            None => vec![model["machine"]["id"].as_str().unwrap().to_owned()],
        }
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
        let reply = core.respond("GET", "/api/model", "", None);
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
            assert_eq!(machine["name"], machine["id"]);
        }
        for (sid, machine) in [
            ("lane-home", "home"),
            ("lane-alpha", "alpha"),
            ("lane-bravo", "bravo"),
            ("lane-delta", "delta"),
        ] {
            assert_eq!(model["sessions"][sid]["machine"], machine, "{sid}");
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
        let tx = core.respond("GET", "/api/tx", "sid=lane-bravo", None);
        assert_eq!(tx.status, 200);
        assert!(String::from_utf8_lossy(&tx.body).contains("done on bravo"));
        let transcript = core.respond(
            "GET",
            "/api/transcript",
            "harness=claude&id=lane-alpha",
            None,
        );
        assert_eq!(transcript.status, 200);
        assert!(String::from_utf8_lossy(&transcript.body).contains("done on alpha"));
        for path in ["/s/claude/lane-alpha", "/machines/bravo", "/machines/delta"] {
            assert_eq!(core.respond("GET", path, "", None).status, 200, "{path}");
        }
        assert_eq!(core.respond("GET", "/machines/stray", "", None).status, 404);
        // DIR is only read.
        assert_eq!(contents(&dir), before);

        // A machine received while serving is served from the next request.
        let version = model["version"].as_str().unwrap().to_owned();
        receive(&dir, "charlie", Some("charlie"), "lane-charlie");
        let reply = core.respond("GET", "/api/model", &format!("since={version}"), None);
        assert_eq!(reply.status, 200);
        let model = body_of(&reply);
        assert_eq!(
            machine_ids(&model),
            ["home", "alpha", "bravo", "charlie", "delta"]
        );
        assert_eq!(model["sessions"]["lane-charlie"]["machine"], "charlie");
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(
            machine_ids(&model),
            ["home", "alpha", "bravo-host", "charlie", "delta"]
        );
        assert_eq!(model["machines"][2]["up"], true);
        assert_eq!(model["sessions"]["lane-bravo"]["machine"], "bravo-host");
        // A machine removed is no longer served.
        fs::remove_dir_all(dir.join("machines/charlie")).unwrap();
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(
            machine_ids(&model),
            ["home", "alpha", "bravo-host", "delta"]
        );
        assert!(model["sessions"].get("lane-charlie").is_none());

        // --no-local: the received machines only.
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &local.options));
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(machine_ids(&model), ["alpha", "bravo-host", "delta"]);
        assert!(model["sessions"].get("lane-home").is_none());
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(machine_ids(&model), ["twin", "twin~twin"]);
        assert_eq!(model["sessions"]["lane-here"]["machine"], "twin");
        assert_eq!(model["sessions"]["lane-there"]["machine"], "twin~twin");
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
        let reply = core.respond("GET", "/api/model", "", None);
        assert_eq!(reply.status, 200);
        let text = String::from_utf8_lossy(&reply.body).into_owned();
        let model = body_of(&reply);
        assert_eq!(machine_ids(&model), ["alpha", "bravo", "charlie"]);
        assert_eq!(model["machines"][2]["up"], false);
        assert!(model["sessions"].get("lane-alpha").is_some());
        assert!(model["sessions"].get("lane-bravo").is_some());
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
        for (path, query) in [
            ("/api/tx", "sid=secret-claude"),
            ("/api/transcript", "harness=claude&id=secret-claude"),
            ("/api/transcript", "harness=claude&id=leak"),
            ("/api/transcript", "harness=codex&id=secret-codex"),
            ("/machines/evil", ""),
        ] {
            assert_eq!(
                core.respond("GET", path, query, None).status,
                404,
                "{path}?{query}"
            );
        }
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
    fn background_unknown_transcripts_share_one_listing_interval() {
        let fixture = Fixture::new();
        let view = MachineView::new(fixture.options.clone());
        view.set_refresh(Refresh::Background);
        assert!(view.transcript_path("claude", "unknown").unwrap().is_none());
        let path = fixture.root.join("claude/projects/project/later.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{}\n").unwrap();
        assert!(
            view.transcript_path("claude", "later").unwrap().is_none(),
            "another unknown id shares the listing instead of walking again"
        );
        lock(&view.files).lookup_at = Some(Instant::now() - REBUILD_SPACING);
        assert_eq!(view.transcript_path("claude", "later").unwrap(), Some(path));
        view.close();
    }

    #[test]
    fn unchanged_heartbeats_do_not_rebuild_received_machine_models() {
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        for index in 0..6 {
            let name = format!("machine-{index}");
            receive(&dir, &name, Some(&name), &format!("lane-{index}"));
        }
        let core = ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
        assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
        let builds = crate::model::BUILDS.with(|builds| builds.get());
        let started = Instant::now();
        for _ in 0..3 {
            for index in 0..6 {
                let name = format!("machine-{index}");
                received_facts(&dir, &name, facts_of(&name));
            }
            assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
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
        let reply = core.respond("GET", "/api/model", "", None);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let model = body_of(&reply);
        assert_eq!(machine_ids(&model), ["home", "alpha"]);
        assert_eq!(model["sessions"]["same"]["machine"], "home");
        assert_eq!(model["sessions"]["lane-alpha"]["machine"], "alpha");
        for turn in model["turns"].as_array().unwrap() {
            assert!(
                model["sessions"]
                    .get(turn["sid"].as_str().unwrap())
                    .is_some()
            );
        }
        let tx = core.respond("GET", "/api/tx", "sid=same", None);
        assert_eq!(tx.status, 200);
        assert!(String::from_utf8_lossy(&tx.body).contains("done on home"));
        let transcript = core.respond("GET", "/api/transcript", "harness=claude&id=same", None);
        assert_eq!(transcript.status, 200);
        assert!(String::from_utf8_lossy(&transcript.body).contains("done on home"));
        let tree = body_of(&core.respond("GET", "/api/tree", "", None));
        let same: Vec<&Value> = tree["roots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|root| root["id"] == "same")
            .collect();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0]["machine"], "home");
        assert_eq!(core.respond("GET", "/s/claude/same", "", None).status, 200);
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        assert_eq!(
            machine_ids(&model),
            ["alpha-host", "delta", "echo", "foxtrot", "gamma", "golf"]
        );
        assert_eq!(model["sessions"]["lane-gamma"]["machine"], "gamma");
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
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
        let model = body_of(&core.respond("GET", "/api/model", "", None));
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
        let reply = core.respond("GET", "/api/model", "", None);
        assert_eq!(reply.status, 200);
        let model = body_of(&reply);
        assert_eq!(machine_ids(&model), ["alpha"]);
        assert_eq!(
            model["sessions"]
                .as_object()
                .unwrap()
                .keys()
                .filter(|id| !id.contains('@'))
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
        let reply = core.respond("GET", "/api/model", "", None);
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

    fn version_of(reply: &ViewerReply) -> String {
        body_of(reply)["version"].as_str().unwrap().to_owned()
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
        at_once(what, move || view.respond("GET", &path, &query, None))
    }

    /// Whether `view`'s transcript of `lane` holds `text`, [`at_once`].
    fn serves_at_once(what: &str, view: &Arc<MachineView>, text: &str) -> bool {
        String::from_utf8_lossy(&ask(what, view, "/api/tx", "sid=lane").body).contains(text)
    }

    /// Background refresh (#29): while a rebuild runs, every read answers
    /// at once from the model before it, with that model's `ETag`: the
    /// model, each form of `/api/tx` (a page, `after=`, `turn=`, errors),
    /// an attachment and the V1 tree. An id that snapshot lacks is the
    /// usual 404. The rebuilt model answers once it is done, and a poll
    /// never sees the version before it again.
    #[test]
    fn a_read_during_a_rebuild_answers_from_the_last_model_at_once() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        view.set_refresh(Refresh::Background);
        let first = view.respond("GET", "/api/model", "", None);
        assert_eq!(first.status, 200);
        let v1 = version_of(&first);
        let turn = body_of(&first)["turns"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(view.respond("GET", "/api/tree", "", None).status, 200);
        let reads = view.hooks.refreshes_first.load(Ordering::SeqCst);
        let (starts, release) = view.hooks.hold();
        say(&fixture, "lane", 1, "written while the rebuild waits");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the refresher rebuilds a changed log");
        let during = ask("/api/model", &view, "/api/model", &format!("since={v1}"));
        assert_eq!(during.status, 304, "the model before the rebuild answers");
        assert_eq!(during.etag, Some(format!("\"{v1}\"")));
        for (query, status) in [
            ("sid=lane".to_owned(), 200),
            ("sid=lane&after=0".into(), 200),
            (format!("sid=lane&turn={turn}"), 200),
            (format!("sid=lane&errors=1&since={v1}"), 304),
            ("sid=lane&turn=not-in-the-snapshot".into(), 404),
            ("sid=nobody".into(), 404),
        ] {
            let page = ask(&query, &view, "/api/tx", &query);
            assert_eq!(page.status, status, "{query}");
            assert!(
                !String::from_utf8_lossy(&page.body).contains("while the rebuild waits"),
                "{query}"
            );
        }
        let attachment = ask(
            "/api/attachment",
            &view,
            "/api/attachment",
            "sid=lane&o=0&b=0&v=0000000000000000",
        );
        assert_eq!(attachment.status, 404, "no image there, but an answer");
        let tree = ask("/api/tree", &view, "/api/tree", "");
        assert_eq!(tree.status, 200);
        assert_eq!(
            view.hooks.refreshes_first.load(Ordering::SeqCst),
            reads,
            "a read refreshed first"
        );
        drop(release);
        let after = eventually("the rebuilt model", || {
            let reply = view.respond("GET", "/api/model", &format!("since={v1}"), None);
            (reply.status == 200).then_some(reply)
        });
        let v2 = version_of(&after);
        assert_ne!(v2, v1);
        assert_eq!(after.etag, Some(format!("\"{v2}\"")));
        let page = view.respond("GET", "/api/tx", "sid=lane", None);
        assert!(String::from_utf8_lossy(&page.body).contains("while the rebuild waits"));
        for _ in 0..5 {
            let poll = view.respond("GET", "/api/model", &format!("since={v2}"), None);
            assert_eq!(poll.status, 304, "a poll went back to an older version");
        }
        view.close();
    }

    /// Background refresh (#29): reads with no change rebuild nothing, and
    /// the changes of one second after a rebuild are one rebuild.
    #[test]
    fn changes_within_a_second_are_one_rebuild_and_none_without_a_change() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        view.set_refresh(Refresh::Background);
        let v1 = version_of(&view.respond("GET", "/api/model", "", None));
        assert_eq!(view.hooks.builds(), 1);
        let idle = |version: &str| {
            let until = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < until {
                let poll = view.respond("GET", "/api/model", &format!("since={version}"), None);
                assert_eq!(poll.status, 304);
                thread::sleep(Duration::from_millis(50));
            }
        };
        idle(&v1);
        assert_eq!(view.hooks.builds(), 1, "no change, no rebuild");

        // One line: one rebuild, seen by a later read.
        say(&fixture, "lane", 0, "line 0");
        let v2 = eventually("the first line", || {
            let reply = view.respond("GET", "/api/model", &format!("since={v1}"), None);
            (reply.status == 200).then(|| version_of(&reply))
        });
        assert_eq!(view.hooks.builds(), 2);
        // Nineteen more, each its own change, within the second after that
        // rebuild: one rebuild takes them all.
        for number in 1..20 {
            say(&fixture, "lane", number, &format!("line {number}"));
            thread::sleep(Duration::from_millis(10));
        }
        let page = eventually("the nineteen lines", || {
            let page = view.respond("GET", "/api/tx", "sid=lane", None);
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
        let v3 = version_of(&view.respond("GET", "/api/model", "", None));
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
    fn concurrent_reads_see_whole_snapshots_while_logs_grow() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let mut core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        core.set_refresh(Refresh::Background);
        let core = Arc::new(core);
        assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
        let writing = Arc::new(AtomicBool::new(true));
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let (core, writing) = (core.clone(), writing.clone());
                thread::spawn(move || {
                    let mut seen = [0_u64; 2];
                    let mut reads = 0;
                    while writing.load(Ordering::SeqCst) || reads < 10 {
                        let reply = core.respond("GET", "/api/model", "", None);
                        assert_eq!(reply.status, 200);
                        let model = body_of(&reply);
                        assert_eq!(
                            reply.etag,
                            Some(format!("\"{}\"", model["version"].as_str().unwrap()))
                        );
                        for (at, sid) in ["lane-a", "lane-b"].into_iter().enumerate() {
                            let slots: u64 = model["tx"][sid]
                                .as_str()
                                .unwrap()
                                .split('.')
                                .next()
                                .unwrap()
                                .parse()
                                .unwrap();
                            assert!(slots >= seen[at], "{sid}: {slots} after {}", seen[at]);
                            let page = core.respond("GET", "/api/tx", &format!("sid={sid}"), None);
                            assert_eq!(page.status, 200);
                            let total = body_of(&page)["total"].as_u64().unwrap();
                            assert!(total >= slots, "{sid}: a page of {total} after {slots}");
                            seen[at] = total;
                        }
                        reads += 1;
                    }
                    seen
                })
            })
            .collect();
        for number in 0..40 {
            say(&alpha, "lane-a", number, &format!("line {number}"));
            say(&bravo, "lane-b", number, &format!("line {number}"));
            thread::sleep(Duration::from_millis(25));
        }
        eventually("the last lines", || {
            ["lane-a", "lane-b"]
                .iter()
                .all(|sid| {
                    let page = core.respond("GET", "/api/tx", &format!("sid={sid}"), None);
                    String::from_utf8_lossy(&page.body).contains("\"line 39\"")
                })
                .then_some(())
        });
        writing.store(false, Ordering::SeqCst);
        for reader in readers {
            let seen = reader.join().expect("a reader failed");
            assert!(seen.iter().all(|total| *total > 0));
        }
        // Closed: no call reads a file again.
        core.close();
        assert_eq!(core.respond("GET", "/api/model", "", None).status, 404);
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
    fn warm(view: &MachineView) -> String {
        let version = version_of(&view.respond("GET", "/api/model", "", None));
        let until = Instant::now() + REBUILD_SPACING + Duration::from_millis(200);
        while Instant::now() < until {
            view.respond("GET", "/api/model", &format!("since={version}"), None);
            thread::sleep(Duration::from_millis(50));
        }
        version
    }

    fn serves(view: &MachineView, text: &str) -> bool {
        let page = view.respond("GET", "/api/tx", "sid=lane", None);
        String::from_utf8_lossy(&page.body).contains(text)
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
        let v1 = version_of(&view.respond("GET", "/api/model", "", None));
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
            "/api/model",
            &format!("since={v1}"),
        );
        assert_eq!(first.status, 304, "the model before the idle spell answers");
        assert!(running(), "the read queued the view's checks again");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the queued check rebuilds the view");
        for _ in 0..3 {
            let during = ask(
                "a read during the check's rebuild",
                &view,
                "/api/model",
                &format!("since={v1}"),
            );
            assert_eq!(during.status, 304);
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
        assert_ne!(version_of(&view.respond("GET", "/api/model", "", None)), v1);
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
        let v1 = warm(&view);
        // A build counts when it starts: the model it shows is what ends it.
        let shown = || view.shown_model().map(|model| model.built.version.clone());
        let before = shown();
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
        let during = view.respond("GET", "/api/model", &format!("since={v1}"), None);
        assert!(matches!(during.status, 200 | 304), "{}", during.status);
        // No read from here until the new worker has rebuilt it.
        eventually("a rebuild by the new worker", || {
            // The replacement may finish before the preceding read returns.
            // Wait for its published result, not an additional future build.
            (shown() != before).then_some(())
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
            let view = MachineView::new(fixture.options.clone());
            view.catalog_demand.store(true, Ordering::Relaxed);
            let options = fixture.options.clone();
            let start = Instant::now();
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
                eprintln!(
                    "production-first-page unrelated={unrelated} latency_us={} response_bytes={} native_sources={observed}",
                    start.elapsed().as_micros(),
                    page.body.len()
                );
            }));
            view.warm().unwrap();
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
                assert_eq!(view.respond("GET", "/api/model", "", None).status, 200);
                view
            })
            .collect();
        // A build counts when it starts: the model it shows is what ends it.
        let shown =
            |view: &Arc<MachineView>| view.shown_model().map(|model| model.built.version.clone());
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
                    view.hooks.builds() > *builds && shown(view) != *before
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
            assert_eq!(view.respond("GET", "/api/model", "", None).status, 200);
        }
        let (starts, release) = busy.hooks.hold();
        say(&blocker, "lane", 1, "holds the only worker");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the only worker rebuilds the blocker");
        for _ in 0..100 {
            pushed.invalidate();
            for view in [&pushed, &watched] {
                view.respond("GET", "/api/model", "", None);
            }
        }
        assert_eq!(pool.entries_for(&pushed), 1);
        assert_eq!(pool.entries_for(&watched), 1);
        assert_eq!(pool.entries_for(&busy), 0, "running, not queued");
        assert_eq!(pool.queued(), 2);
        drop(release);
        for view in [&busy, &pushed, &watched] {
            view.close();
        }
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
                let reply = view.respond("GET", "/api/model", &format!("since={version}"), None);
                assert_eq!(reply.status, 304);
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
            let reply = ask(
                "a read with its check late",
                &view,
                "/api/model",
                &format!("since={v1}"),
            );
            assert_eq!(reply.status, 304, "the last model answers");
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

    /// With no model built yet, the first read waits for the first build,
    /// as before (here it is held for 300 ms and more); the reads after it
    /// answer from that model and build nothing.
    #[test]
    fn with_no_model_yet_a_read_waits_for_the_first_build() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        view.set_refresh(Refresh::Background);
        let (starts, release) = view.hooks.hold();
        let first = {
            let view = view.clone();
            thread::spawn(move || view.respond("GET", "/api/tx", "sid=lane", None).status)
        };
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the read builds the first model");
        thread::sleep(Duration::from_millis(300));
        assert!(!first.is_finished(), "a read answered with no model built");
        drop(release);
        assert_eq!(first.join().expect("the read panicked"), 200);
        assert_eq!(view.hooks.refreshes_first.load(Ordering::SeqCst), 1);
        assert_eq!(view.respond("GET", "/api/model", "", None).status, 200);
        assert_eq!(view.hooks.refreshes_first.load(Ordering::SeqCst), 1);
        assert_eq!(view.hooks.builds(), 1);
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
        assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
        assert!(!core.invalidate("c"), "no machine is served as c");
        say(&alpha, "lane-a", 1, "pushed to alpha");
        assert!(core.invalidate("a"));
        eventually("alpha's pushed line", || {
            let page = core.respond("GET", "/api/tx", "sid=lane-a", None);
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

    fn tx_has(core: &ViewerCore, sid: &str, text: &str) -> bool {
        let page = core.respond("GET", "/api/tx", &format!("sid={sid}"), None);
        String::from_utf8_lossy(&page.body).contains(text)
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
        for path in [
            "/",
            "/timeline",
            "/s/claude/lane",
            "/s/claude/nobody",
            "/machines/testbox",
            "/trace/claude/lane/some-turn",
        ] {
            page(path);
        }
        assert_eq!(core.respond("GET", "/viewer.js", "", None).status, 200);
        assert_eq!(refresh_counts(&view), (0, 0, 0), "a page read the logs");

        // The page's own model read builds it.
        let model = core.respond("GET", "/api/model", "", None);
        assert_eq!(model.status, 200);
        let turn = body_of(&model)["turns"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(view.hooks.builds(), 1);

        // Idle, then a change announced: pages take no notice of it.
        until_idle(&view);
        say(&fixture, "lane", 1, "announced while idle");
        assert!(core.invalidate(""));
        let before = refresh_counts(&view);
        for path in [
            "/s/claude/lane".to_owned(),
            "/machines/testbox".into(),
            format!("/trace/claude/lane/{turn}"),
            "/timeline".into(),
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
                core.respond("GET", "/api/tx", "sid=lane", None)
            })
        };
        assert_eq!(reply.status, 200);
        assert!(!String::from_utf8_lossy(&reply.body).contains("announced while idle"));
        eventually("the announced change", || {
            tx_has(&core, "lane", "announced while idle").then_some(())
        });
        assert_eq!(view.hooks.builds(), before.0 + 1);
        assert_eq!(
            view.hooks.refreshes_first.load(Ordering::SeqCst),
            before.2,
            "an API read refreshed first"
        );
        // A URL the built model lacks: checked against a fresh one, 404.
        for path in ["/s/claude/nobody", "/machines/elsewhere", "/s/codex/lane"] {
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
        for path in ["/s/claude/lane-b", "/machines/alpha", "/s/claude/nobody"] {
            shell(path);
        }
        for view in &views {
            assert_eq!(refresh_counts(view), (0, 0, 0), "a page read the logs");
        }
        assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
        for view in &views {
            until_idle(view);
        }
        say(&alpha, "lane-a", 1, "announced while idle");
        say(&bravo, "lane-b", 1, "announced while idle");
        assert!(core.invalidate("a") && core.invalidate("b"));
        let before: Vec<_> = views.iter().map(|view| refresh_counts(view)).collect();
        for path in ["/s/claude/lane-a", "/s/claude/lane-b", "/machines/bravo"] {
            shell(path);
        }
        let after: Vec<_> = views.iter().map(|view| refresh_counts(view)).collect();
        assert_eq!(after, before, "a page refreshed a machine");
        eventually("bravo's announced change", || {
            tx_has(&core, "lane-b", "announced while idle").then_some(())
        });
        assert_eq!(core.respond("GET", "/machines/gamma", "", None).status, 404);
        core.close();
    }

    /// `warm` builds every machine's model on the calling thread without
    /// counting as a read (no background check starts), and the first read
    /// answers from it at once. After an idle spell `warm_machine` rebuilds
    /// the one machine it names and looks at no other, and clears that
    /// machine's invalidation; `warm` rebuilds only the machines whose logs
    /// changed. The reads after them answer at once with the changes.
    /// Closed, neither builds anything.
    #[test]
    fn warm_builds_every_machine_without_a_read_and_the_first_read_answers_at_once() {
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
        core.warm().expect("both machines build");
        for view in &views {
            assert_eq!(view.hooks.builds(), 1);
            let state = lock(&view.live.state);
            assert_eq!(state.slot, Slot::Idle, "a warm started background checks");
            assert!(state.read_at.is_none(), "a warm counted as a read");
        }
        assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
        for view in &views {
            assert_eq!(view.hooks.builds(), 1, "the first read rebuilt");
            assert_eq!(view.hooks.refreshes_first.load(Ordering::SeqCst), 0);
        }

        for view in &views {
            until_idle(view);
        }
        // Alpha's change is announced; bravo's isn't. Warming alpha alone
        // rebuilds alpha and doesn't look at bravo.
        say(&alpha, "lane-a", 1, "warmed while idle");
        say(&bravo, "lane-b", 1, "never announced");
        assert!(core.invalidate("a"));
        let bravo_before = refresh_counts(&views[1]);
        assert!(core.warm_machine("a").expect("alpha rebuilds"));
        assert_eq!(views[0].hooks.builds(), 2, "alpha's change, one rebuild");
        assert_eq!(
            refresh_counts(&views[1]),
            bravo_before,
            "warming alpha looked at bravo"
        );
        assert!(
            !lock(&views[0].live.state).invalidated,
            "the warm covered it"
        );
        assert!(
            !core.warm_machine("c").unwrap(),
            "no machine is served as c"
        );
        // Warming them all: bravo's change is found by its stat pass, and
        // alpha, unchanged since, isn't rebuilt again.
        core.warm().expect("bravo rebuilds");
        assert_eq!(views[0].hooks.builds(), 2);
        assert_eq!(views[1].hooks.builds(), 2, "bravo's change, one rebuild");
        let reads: Vec<_> = views
            .iter()
            .map(|view| view.hooks.refreshes_first.load(Ordering::SeqCst))
            .collect();
        assert!(tx_has(&core, "lane-a", "warmed while idle"));
        assert!(tx_has(&core, "lane-b", "never announced"));
        for (view, reads) in views.iter().zip(reads) {
            assert_eq!(
                view.hooks.refreshes_first.load(Ordering::SeqCst),
                reads,
                "a read after the warm refreshed first"
            );
        }
        assert_eq!(views[0].hooks.builds(), 2);
        assert_eq!(views[1].hooks.builds(), 2);

        core.close();
        say(&alpha, "lane-a", 2, "after close");
        core.warm().expect("a closed core warms nothing");
        assert!(!core.warm_machine("a").unwrap(), "a closed core warmed a");
        assert_eq!(views[0].hooks.builds(), 2, "a closed core built");
    }

    /// A session newer than the built model, on a view that went idle, is
    /// opened while a warm that already took its invalidation is still
    /// building it. The page, which the built model lacks, is checked
    /// against a fresh model: it waits for that warm, then finds the
    /// session (200), where the model before the warm would answer 404.
    fn a_new_session_opens_during_its_warm(mut core: ViewerCore, key: &str, fixture: &Fixture) {
        core.set_refresh_pool(RefreshPool::new(1));
        core.set_refresh(Refresh::OnInvalidate);
        let core = Arc::new(core);
        let views = core.machine_views();
        for view in &views {
            quick_idle(view);
        }
        assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
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
            let (core, key) = (core.clone(), key.to_owned());
            thread::spawn(move || core.warm_machine(&key))
        };
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the warm builds");
        assert!(
            !lock(&views[0].live.state).invalidated,
            "the warm took the invalidation"
        );
        let page = {
            let core = core.clone();
            thread::spawn(move || core.respond("GET", "/s/claude/brand-new", "", None).status)
        };
        thread::sleep(Duration::from_millis(300));
        assert!(
            !page.is_finished(),
            "the page answered from the model before the warm"
        );
        drop(release);
        assert_eq!(page.join().expect("the page request panicked"), 200);
        assert!(
            warming
                .join()
                .expect("the warm panicked")
                .expect("it built")
        );
        assert_eq!(
            core.respond("GET", "/s/claude/not-even-new", "", None)
                .status,
            404
        );
        core.close();
    }

    #[test]
    fn a_new_session_opens_during_its_warm_on_one_machine() {
        let fixture = lane_fixture();
        a_new_session_opens_during_its_warm(ViewerCore::new(fixture.options.clone()), "", &fixture);
    }

    #[test]
    fn a_new_session_opens_during_its_warm_across_machines() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let core = ViewerCore::with_machines(vec![
            ("a".into(), alpha.options.clone()),
            ("b".into(), bravo.options.clone()),
        ]);
        a_new_session_opens_during_its_warm(core, "a", &alpha);
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
                assert_eq!(reply.status, 500, "{path} asked at {asked:?}");
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
            lock(&view.live.state).failing_since[Kind::Model as usize]
        });
        let builds = view.hooks.builds();
        let inside = two_phases(
            &view,
            since,
            "/api/model",
            &format!("since={v1}"),
            304,
            || {
                let page = ask("/api/tx", &view, "/api/tx", "sid=lane");
                assert!(matches!(page.status, 200 | 500), "{}", page.status);
            },
        );
        assert!(inside > 0, "no read inside FAILING_AFTER");
        assert_eq!(ask("/api/tx", &view, "/api/tx", "sid=lane").status, 500);
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
        eventually("the model again", || {
            (view
                .respond("GET", "/api/model", &format!("since={v1}"), None)
                .status
                == 200)
                .then_some(())
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
            lock(&view.live.state).failing_since[Kind::Model as usize]
        });
        let mut reads = 0;
        while released.elapsed() + Duration::from_millis(500) < FAILING_AFTER {
            let reply = ask("/api/model", &view, "/api/model", &format!("since={v1}"));
            assert_eq!(
                reply.status,
                304,
                "the error answered {:?} after the failure",
                released.elapsed()
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
            (view.respond("GET", "/api/model", "", None).status == 500).then_some(())
        });
        lock(&view.live.state).idle_after = Duration::from_millis(300);
        until_idle(&view);
        let (starts, release) = view.hooks.hold();
        let first = ask(
            "the first read after an idle spell",
            &view,
            "/api/model",
            &format!("since={v1}"),
        );
        assert_eq!(first.status, 304, "a stale error answered");
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("the queued check builds");
        for _ in 0..3 {
            let during = ask(
                "a read during the check",
                &view,
                "/api/model",
                &format!("since={v1}"),
            );
            assert_eq!(during.status, 304, "a stale error answered");
        }
        drop(release);
        eventually("the error, once the check failed again", || {
            (ask("/api/model", &view, "/api/model", "").status == 500).then_some(())
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
            let reply = ask(
                "a read during a hung build",
                &view,
                "/api/model",
                &format!("since={v1}"),
            );
            let answered = since.elapsed();
            if answered < stalled {
                assert_eq!(reply.status, 304, "answered at {answered:?}");
                inside += 1;
            } else if asked >= stalled {
                assert_eq!(reply.status, 500, "asked at {asked:?}");
                past += 1;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(inside > 0, "no read before the stall");
        drop(release);
        eventually("the rebuilt model", || {
            (view
                .respond("GET", "/api/model", &format!("since={v1}"), None)
                .status
                == 200)
                .then_some(())
        });
        assert!(serves(&view, "after the hang"));
        view.close();
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
        let first = ask(
            "a read behind the hung pool",
            &view,
            "/api/model",
            &format!("since={v1}"),
        );
        assert_eq!(first.status, 304, "the last model answers first");
        let mut stalled = 0;
        let deadline = Instant::now() + Duration::from_secs(20);
        while stalled < 5 {
            assert!(Instant::now() < deadline, "no stall within 20 s");
            let reply = ask(
                "a read behind the hung pool",
                &view,
                "/api/model",
                &format!("since={v1}"),
            );
            match reply.status {
                304 => assert_eq!(stalled, 0, "the last model answered after the stall"),
                500 => stalled += 1,
                status => panic!("{status}"),
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
                assert_eq!(view.respond("GET", "/api/model", "", None).status, 200);
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
                let reply = ask("a read in a backlog", view, "/api/model", "");
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
        lock(&view.live.state).stalled_after = Duration::from_secs(1);
        *lock(&view.hooks.building) = Some(Arc::new(|| thread::sleep(Duration::from_secs(3))));
        let builds = view.hooks.builds();
        say(&fixture, "lane", 1, "after a long build");
        // No read while it builds: a read then would rightly answer the stall.
        eventually("the long build", || {
            view.shown_model()
                .filter(|model| model.built.version != v1)
                .map(drop)
        });
        assert!(view.hooks.builds() > builds);
        *lock(&view.hooks.building) = None;
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            let reply = ask(
                "a read after a long build",
                &view,
                "/api/model",
                &format!("since={v1}"),
            );
            assert_eq!(reply.status, 200, "the rebuilt model answers");
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(pool.peak_and_started().0, 1, "a long build grew the pool");
        view.close();
    }

    /// Refreshed on read, the reads that find one change share its one
    /// rebuild: the others wait for it, then answer the same new model.
    #[test]
    fn on_read_the_reads_that_find_a_change_share_one_rebuild() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        let v1 = version_of(&view.respond("GET", "/api/model", "", None));
        assert_eq!(view.hooks.builds(), 1);
        let (starts, release) = view.hooks.hold();
        say(&fixture, "lane", 1, "one change");
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let view = view.clone();
                thread::spawn(move || version_of(&view.respond("GET", "/api/model", "", None)))
            })
            .collect();
        starts
            .recv_timeout(Duration::from_secs(10))
            .expect("a read rebuilds");
        thread::sleep(Duration::from_millis(200));
        assert_eq!(view.hooks.builds(), 2, "eight reads, one rebuild");
        drop(release);
        let versions: BTreeSet<String> = readers
            .into_iter()
            .map(|reader| reader.join().expect("a reader failed"))
            .collect();
        assert_eq!(versions.len(), 1, "{versions:?}");
        assert!(!versions.contains(&v1));
        assert_eq!(view.hooks.builds(), 2);
    }

    /// Three lookups' updates of the transcript paths, interleaved (each
    /// is a listing, then the new paths read without the lock, then a
    /// store): an older listing's store between two newer ones never
    /// leaves a new file known without its session, which would 404 it
    /// until restart.
    #[test]
    fn interleaved_path_updates_never_lose_a_new_file() {
        let fixture = lane_fixture();
        let view = fixture.viewer();
        let file = fixture.root.join("claude/projects/project/lane.jsonl");
        // C listed before the file was there; B and A after it.
        let (c_paths, c_new) = view.files_to_key(Vec::new());
        let (b_paths, b_new) = view.files_to_key(vec![file.clone()]);
        let b_keys = view.keys_of(&b_new);
        view.store_files(b_paths, &b_new, b_keys);
        // To A the file isn't new any more: B knows it.
        let (a_paths, a_new) = view.files_to_key(vec![file.clone()]);
        assert!(a_new.is_empty());
        let a_keys = view.keys_of(&a_new);
        let c_keys = view.keys_of(&c_new);
        view.store_files(c_paths, &c_new, c_keys);
        view.store_files(a_paths, &a_new, a_keys);
        {
            let files = lock(&view.files);
            let key = ("claude".to_owned(), "lane".to_owned());
            assert!(
                files.paths.contains_key(&key) || !files.known.contains(&file),
                "the file is known, but no session is"
            );
        }
        assert_eq!(
            view.transcript_path("claude", "lane").unwrap(),
            Some(file),
            "the next lookup finds it"
        );
    }

    /// The model and the V1 tree fail on their own: a tree that won't
    /// build leaves the last tree served on `/api/tree` for FAILING_AFTER,
    /// then answers the error, each at once, while `/api/model` goes on
    /// serving a model that builds.
    #[test]
    fn a_failing_tree_leaves_the_model_served() {
        let fixture = lane_fixture();
        let (view, v1) = warm_background(&fixture);
        assert_eq!(view.respond("GET", "/api/tree", "", None).status, 200);
        let reads = view.hooks.refreshes_first.load(Ordering::SeqCst);
        view.hooks.tree_failing.store(true, Ordering::SeqCst);
        say(&fixture, "lane", 1, "while the tree fails");
        let since = eventually("the tree's failed rebuild", || {
            lock(&view.live.state).failing_since[Kind::Tree as usize]
        });
        let inside = two_phases(&view, since, "/api/tree", "", 200, || {
            let reply = view.respond("GET", "/api/model", &format!("since={v1}"), None);
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
    #[test]
    fn core_model_delta_handshake_works_for_one_and_multiple_machines() {
        for multiple in [false, true] {
            let fixture = Fixture::new();
            let other = Fixture::new();
            let record = |id: &str, text: &str| json!({"type":"user","timestamp":"2026-09-24T00:00:00Z","sessionId":id,"origin":{"kind":"human"},"message":{"role":"user","content":text}});
            for i in 0..30 {
                let id = format!("session-{i}");
                fixture.claude(&id, &[record(&id, "Unchanged prompt")]);
            }
            let core = if multiple {
                ViewerCore::with_machines(vec![
                    ("first".into(), fixture.options.clone()),
                    ("second".into(), other.options.clone()),
                ])
            } else {
                ViewerCore::new(fixture.options.clone())
            };
            let first = core.respond("GET", "/api/model", "delta=1", None);
            let before = body_of(&first);
            let version = before["version"].as_str().unwrap();
            assert_eq!(
                core.respond(
                    "GET",
                    "/api/model",
                    &format!("delta=1&since={version}"),
                    None
                )
                .status,
                304
            );
            fixture.claude("session-0", &[record("session-0", "Changed prompt")]);
            let reply = core.respond(
                "GET",
                "/api/model",
                &format!("delta=1&since={version}"),
                None,
            );
            let delta = body_of(&reply);
            assert_eq!(reply.status, 200);
            assert_eq!(delta["delta"], 1);
            assert_eq!(delta["from"], version);
            assert!(
                !delta["collections"]["tx"]["set"]
                    .as_object()
                    .unwrap()
                    .is_empty()
            );
            assert!(reply.body.len() < first.body.len() / 2);
            let unknown =
                body_of(&core.respond("GET", "/api/model", "delta=1&since=unknown", None));
            assert!(unknown.get("sessions").is_some());
            assert!(unknown.get("delta").is_none());
            core.close();
        }
    }
}

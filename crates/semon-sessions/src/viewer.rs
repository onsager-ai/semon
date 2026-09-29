use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Seek, SeekFrom},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
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
    proc_start, read_index, save_index, tx,
    union::ViewerCore,
    user_text,
};

const PAGE_ENTRIES: usize = 200;
const PAGE_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_BYTES: usize = 64 * 1024;
const EXPAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINE: usize = EXPAND_BYTES + 1024 * 1024;

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

/// One machine's viewer: the pages, assets and API routes over one set of
/// agent homes, with the in-memory model, caches and live refresh behind
/// them. [`ViewerCore`] serves one or several of these.
pub(crate) struct MachineView {
    options: Options,
    index: Option<Index>,
    index_dirty: bool,
    events: Option<EventCache>,
    events_dirty: bool,
    events_saved: Option<Instant>,
    last_save: Option<Instant>,
    tree: Option<TreeCache>,
    model: Option<ModelCache>,
    texts: Texts,
    paths: BTreeMap<(String, String), PathBuf>,
    known_paths: BTreeSet<PathBuf>,
    harness: BTreeMap<PathBuf, ((u64, u64), BTreeSet<u64>)>,
}

// An embedding server moves a core to a blocking thread and shares it
// behind a lock: it must stay `Send`.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<ViewerCore>();
    send::<MachineView>();
};

/// The loopback server around a [`ViewerCore`]: the per-run token and its
/// cookie, the exact local Host check and GET only.
struct Viewer {
    core: ViewerCore,
    token: String,
    port: u16,
}

/// Headers the viewer sends on every response, whatever its status. A server
/// embedding [`ViewerCore`] sends them too: the page relies on this CSP (no
/// inline script or style), and nothing it serves may be cached or framed.
pub const SECURITY_HEADERS: [(&str, &str); 4] = [
    ("Cache-Control", "no-store"),
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self'",
    ),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
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
    built: Built,
    snapshot: Snapshot,
}

/// A routed response: status, content type, body and an optional `ETag`.
type Routed = (u16, &'static str, Vec<u8>, Option<String>);

/// The viewer's page: every screen's URL serves it.
const PAGE: &str = include_str!("viewer.html");

/// The mockup's three families, vendored (D1): Instrument Sans, JetBrains
/// Mono and Source Serif 4, each in its latin and latin-ext subsets.
pub(crate) const FONTS: [(&str, &[u8]); 6] = [
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
    (
        "source-serif-4-latin",
        include_bytes!("fonts/source-serif-4/latin.woff2"),
    ),
    (
        "source-serif-4-latin-ext",
        include_bytes!("fonts/source-serif-4/latin-ext.woff2"),
    ),
];

fn watch_tree(path: &Path, suffixes: &[&str], watched: &mut BTreeMap<PathBuf, Option<Stamp>>) {
    watched.insert(path.to_owned(), stamp(path));
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
        watch_tree(
            &options.codex_home.join("sessions"),
            &[".jsonl"],
            &mut watched,
        );
        watch_tree(
            &options.codex_home.join("thread-writer-locks"),
            &[".lock"],
            &mut watched,
        );
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
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| invalid_input("expected TCP listener"))?
        .port();
    let mut viewer = Viewer {
        core: match options.received {
            Some(received) => ViewerCore::with_received(options.machines, received),
            None if options.machines.is_empty() => ViewerCore::new(options.sessions),
            None => ViewerCore::with_machines(options.machines),
        },
        token: random_token()?,
        port,
    };
    println!("http://127.0.0.1:{port}/?t={}", viewer.token);
    loop {
        // recv blocks. No timer or scanner runs between requests.
        let request = server.recv()?;
        viewer.handle(request);
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("static HTTP header")
}

fn respond(
    request: Request,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    cookie: Option<&str>,
    etag: Option<&str>,
) {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    if let Some(etag) = etag {
        response.add_header(header("ETag", etag));
    }
    response.add_header(header("Content-Type", content_type));
    for (name, value) in SECURITY_HEADERS {
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

impl Viewer {
    fn handle(&mut self, request: Request) {
        let url = request.url().to_owned();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let Some(set_cookie) = authorized(&request, query, &self.token, self.port) else {
            respond(
                request,
                403,
                "text/plain; charset=utf-8",
                b"Forbidden".to_vec(),
                None,
                None,
            );
            return;
        };
        let if_none_match = request_header(&request, "If-None-Match").map(str::to_owned);
        let reply = self
            .core
            .respond("GET", path, query, if_none_match.as_deref());
        respond(
            request,
            reply.status,
            reply.content_type,
            reply.body,
            set_cookie.then_some(&self.token),
            reply.etag.as_deref(),
        );
    }
}

impl MachineView {
    /// A view over the agent homes, `/proc` and cache that `options` name.
    /// Nothing is read until the first request.
    pub(crate) fn new(options: Options) -> Self {
        Self {
            options,
            index: None,
            index_dirty: false,
            events: None,
            events_dirty: false,
            events_saved: None,
            last_save: None,
            tree: None,
            model: None,
            texts: Texts::default(),
            paths: BTreeMap::new(),
            known_paths: BTreeSet::new(),
            harness: BTreeMap::new(),
        }
    }

    /// The homes, cache and facts this view reads.
    pub(crate) fn options(&self) -> &Options {
        &self.options
    }

    fn update_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let paths: BTreeSet<_> = paths
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl") && regular(path))
            .collect();
        self.paths.retain(|_, path| paths.contains(path));
        self.known_paths.retain(|path| paths.contains(path));
        for path in paths.difference(&self.known_paths) {
            let harness = if path.starts_with(self.options.claude_home.join("projects")) {
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
            if !id.is_empty() {
                self.paths.insert((harness.into(), id), path.clone());
            }
        }
        self.known_paths = paths;
    }

    fn refresh_tree(&mut self) -> io::Result<()> {
        if self
            .tree
            .as_ref()
            .is_some_and(|tree| !tree.snapshot.changed(&self.options))
        {
            return self.persist_if_due();
        }
        if self.index.is_none() {
            self.index = Some(read_index(&self.options.cache));
        }
        let index = self.index.as_mut().expect("index loaded");
        let roots = collect_with_index(&self.options, index, &mut self.index_dirty)?;
        let json = crate::render_json(&roots);
        let snapshot = Snapshot::capture(&self.options, &roots, index);
        let paths = snapshot
            .watched
            .iter()
            .filter_map(|(path, value)| value.is_some().then_some(path.clone()))
            .collect::<Vec<_>>();
        self.update_paths(paths);
        self.tree = Some(TreeCache {
            roots,
            json,
            snapshot,
        });
        self.persist_if_due()
    }

    fn persist_if_due(&mut self) -> io::Result<()> {
        if self.index_dirty
            && self
                .last_save
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(30))
        {
            save_index(
                &self.options.cache,
                self.index.as_ref().expect("index loaded"),
            )?;
            self.index_dirty = false;
            self.last_save = Some(Instant::now());
        }
        Ok(())
    }

    fn tree_json(&mut self) -> io::Result<String> {
        self.refresh_tree()?;
        Ok(self.tree.as_ref().expect("tree loaded").json.clone())
    }

    /// Answers one request: `path` and `query` split at the `?`, still
    /// percent-encoded, and the request's `If-None-Match`. Only GET is
    /// answered (405 otherwise); every URL the viewer uses is a GET. The
    /// caller authenticates first: the core serves whoever it is handed.
    pub(crate) fn respond(
        &mut self,
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
        } else if path == "/api/tx" && query_value(query, "errors").is_some() {
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

    fn refresh_model(&mut self) -> io::Result<()> {
        self.refresh_model_at(model::now_ms())
    }

    /// Rebuilds the model at `now` if its logs or facts changed.
    fn refresh_model_at(&mut self, now: i64) -> io::Result<()> {
        if self
            .model
            .as_ref()
            .is_some_and(|model| !model.snapshot.changed(&self.options))
        {
            return self.persist_events_if_due();
        }
        let path = EventCache::path(&self.options.cache);
        let cache = self.events.get_or_insert_with(|| EventCache::read(&path));
        if self.options.facts.is_none() {
            cache.refresh_reported_runs(&self.options.claude_json, now, &mut self.events_dirty);
        }
        // The files are stamped before the build reads them: a line that
        // lands while it runs is then a change the next poll sees, never one
        // that is neither parsed nor noticed. (A file created meanwhile
        // changes its directory's stamp.)
        let mut snapshot = Snapshot::capture_pids(&self.options, BTreeSet::new(), cache.paths());
        let built = model::build(
            &self.options,
            cache,
            &mut self.events_dirty,
            &mut self.texts,
            now,
        )?;
        if self.options.facts.is_none() {
            snapshot.pids = built
                .pids
                .iter()
                .map(|pid| (*pid, proc_start(&self.options.proc_root, *pid)))
                .collect();
        }
        self.model = Some(ModelCache { built, snapshot });
        self.persist_events_if_due()
    }

    /// The event cache is saved at most every 30 s, like V1's index.
    fn persist_events_if_due(&mut self) -> io::Result<()> {
        if self.events_dirty
            && self
                .events_saved
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(30))
            && let Some(cache) = &self.events
        {
            cache.save(&EventCache::path(&self.options.cache))?;
            self.events_dirty = false;
            self.events_saved = Some(Instant::now());
        }
        Ok(())
    }

    /// `/api/model`: the whole model, or 304 when the client's `ETag` or
    /// `?since=` version is still current.
    fn model(&mut self, query: &str, if_none_match: Option<&str>) -> io::Result<Routed> {
        let json = "application/json; charset=utf-8";
        self.refresh_model()?;
        let built = &self.model.as_ref().expect("model loaded").built;
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
    /// transcript.
    fn tx(&mut self, query: &str) -> io::Result<String> {
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
        self.refresh_model()?;
        let built = &self.model.as_ref().expect("model loaded").built;
        tx::page(built, &sid, &anchor, model::now_ms())
    }

    /// `/api/tx?sid=&errors=1`: where the session's failed steps are
    /// ([`tx::errors`]), or 304 when the client's `ETag` or `?since=` version
    /// is still current. It names no page: `before`, `after` and `turn` are
    /// refused with it.
    fn tx_errors(&mut self, query: &str, if_none_match: Option<&str>) -> io::Result<Routed> {
        let json = "application/json; charset=utf-8";
        if query_value(query, "errors") != Some("1") {
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
        self.refresh_model()?;
        let built = &self.model.as_ref().expect("model loaded").built;
        if !built.tx.contains_key(&sid) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let etag = format!("\"{}\"", built.version);
        if if_none_match == Some(etag.as_str()) || since.as_deref() == Some(built.version.as_str())
        {
            return Ok((304, json, Vec::new(), Some(etag)));
        }
        Ok((200, json, tx::errors(built, &sid)?.into_bytes(), Some(etag)))
    }

    /// Whether a page URL names something in the model: `/machines/<id>`,
    /// `/s/<harness>/<id>` and `/trace/<harness>/<id>/<turn>`.
    fn page_exists(&mut self, path: &str) -> io::Result<bool> {
        let parts: Option<Vec<String>> = path
            .trim_start_matches('/')
            .split('/')
            .map(decoded)
            .collect();
        let parts = parts.ok_or_else(|| invalid_input("path"))?;
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        self.refresh_model()?;
        let built = &self.model.as_ref().expect("model loaded").built;
        Ok(match parts.as_slice() {
            ["machines", id] => *id == built.machine_id,
            ["s", harness, id] => has_session_page(built, harness, id),
            ["trace", harness, id, turn] => has_trace_page(built, harness, id, turn),
            _ => false,
        })
    }

    fn route(&mut self, path: &str, query: &str) -> io::Result<(u16, &'static str, Vec<u8>)> {
        let html = "text/html; charset=utf-8";
        let json = "application/json; charset=utf-8";
        match path {
            "/" | "/timeline" | "/analytics" | "/sessions" | "/machines" => {
                Ok((200, html, PAGE.into()))
            }
            "/viewer.js" => Ok((
                200,
                "text/javascript; charset=utf-8",
                include_str!("viewer.js").into(),
            )),
            "/viewer.css" => Ok((
                200,
                "text/css; charset=utf-8",
                include_str!("viewer.css").into(),
            )),
            "/mark.svg" => Ok((200, "image/svg+xml", include_str!("mark.svg").into())),
            "/favicon.svg" => Ok((200, "image/svg+xml", include_str!("favicon.svg").into())),
            "/shell.js" => Ok((
                200,
                "text/javascript; charset=utf-8",
                include_str!("shell.js").into(),
            )),
            "/shell.css" => Ok((
                200,
                "text/css; charset=utf-8",
                include_str!("shell.css").into(),
            )),
            "/api/tree" => Ok((200, json, self.tree_json()?.into_bytes())),
            "/api/transcript" => Ok((200, json, serde_json::to_vec(&self.transcript(query)?)?)),
            "/api/tx" => Ok((200, json, self.tx(query)?.into_bytes())),
            "/api/entry" => Ok((200, json, self.expand(query)?.into_bytes())),
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
            _ if path.starts_with("/machines/")
                || path.starts_with("/s/")
                || path.starts_with("/trace/") =>
            {
                if self.page_exists(path)? {
                    Ok((200, html, PAGE.into()))
                } else {
                    Err(io::ErrorKind::NotFound.into())
                }
            }
            _ => Err(io::ErrorKind::NotFound.into()),
        }
    }

    fn transcript_path(&mut self, harness: &str, id: &str) -> io::Result<Option<PathBuf>> {
        if id.is_empty() || id.len() > 256 || !matches!(harness, "claude" | "codex") {
            return Ok(None);
        }
        if let Some(path) = self.paths.get(&(harness.into(), id.into()))
            && regular(path)
        {
            return Ok(Some(path.clone()));
        }
        let mut paths = Vec::new();
        file_list(
            &self.options.claude_home.join("projects"),
            &mut paths,
            "jsonl",
        )?;
        file_list(
            &self.options.codex_home.join("sessions"),
            &mut paths,
            "jsonl",
        )?;
        self.update_paths(paths);
        Ok(self.paths.get(&(harness.into(), id.into())).cloned())
    }

    fn harness_offsets_cached(&mut self, path: &Path) -> io::Result<BTreeSet<u64>> {
        let metadata = fs::metadata(path)?;
        #[cfg(unix)]
        let identity = (metadata.dev(), metadata.ino());
        #[cfg(not(unix))]
        let identity = (0, 0);
        if let Some((old_identity, offsets)) = self.harness.get(path)
            && *old_identity == identity
        {
            return Ok(offsets.clone());
        }
        let offsets = harness_offsets(path)?;
        self.harness
            .insert(path.into(), (identity, offsets.clone()));
        Ok(offsets)
    }

    fn transcript(&mut self, query: &str) -> io::Result<TranscriptPage> {
        let harness = query_value(query, "harness")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("harness"))?;
        let id = query_value(query, "id")
            .and_then(decoded)
            .ok_or_else(|| invalid_input("id"))?;
        let path = self
            .transcript_path(&harness, &id)?
            .ok_or(io::ErrorKind::NotFound)?;
        // Cheap when nothing changed (a stat pass, no reparsing): needed so
        // liveness and child links are correct even when a transcript page
        // is opened directly, without a prior `/api/tree` poll to warm the
        // cache in this process.
        self.refresh_tree()?;
        let node = self
            .tree
            .as_ref()
            .and_then(|tree| find_node(&tree.roots, &harness, &id));
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
    fn expand(&mut self, query: &str) -> io::Result<String> {
        if let Some(part) = query_value(query, "as") {
            let sid = query_value(query, "sid")
                .and_then(decoded)
                .ok_or_else(|| invalid_input("sid"))?;
            let slot = query_value(query, "slot")
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| invalid_input("slot"))?;
            self.refresh_model()?;
            let built = &self.model.as_ref().expect("model loaded").built;
            return tx::full_slot(built, &sid, slot, part);
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
        let mut file = fs::File::open(path)?;
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

impl MachineView {
    /// This machine's model, rebuilt first if its logs or facts changed.
    pub(crate) fn built(&mut self) -> io::Result<&Built> {
        self.built_at(model::now_ms())
    }

    /// [`MachineView::built`], a rebuild taking `now` as its clock.
    pub(crate) fn built_at(&mut self, now: i64) -> io::Result<&Built> {
        self.refresh_model_at(now)?;
        Ok(&self.model.as_ref().expect("model loaded").built)
    }

    /// The model last built, if any.
    pub(crate) fn last_built(&self) -> Option<&Built> {
        self.model.as_ref().map(|model| &model.built)
    }

    /// The V1 tree's roots, rebuilt first if anything changed.
    pub(crate) fn tree_roots(&mut self) -> io::Result<Vec<Node>> {
        self.refresh_tree()?;
        Ok(self.tree.as_ref().expect("tree loaded").roots.clone())
    }

    /// Whether this machine has a transcript file for a V1 tree node.
    pub(crate) fn has_transcript(&mut self, harness: &str, id: &str) -> io::Result<bool> {
        Ok(self.transcript_path(harness, id)?.is_some())
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

fn tool_links(node: &Node) -> BTreeMap<String, String> {
    node.children
        .iter()
        .filter_map(|child| {
            child.via_tool.as_ref().map(|tool| {
                (
                    tool.id.clone(),
                    format!("/s/{}/{}", child.harness, percent_encode(&child.id)),
                )
            })
        })
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

fn harness_offsets(path: &Path) -> io::Result<BTreeSet<u64>> {
    use io::BufRead;
    let mut reader = io::BufReader::new(fs::File::open(path)?);
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
    links: &BTreeMap<String, String>,
    harness_message: bool,
) -> Vec<TranscriptEntry> {
    let mut result = Vec::new();
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
                    let link = id.as_ref().and_then(|id| links.get(id)).cloned();
                    result.push(entry(
                        (offset, block),
                        "tool_use",
                        item.get("input").map(content_text).unwrap_or_default(),
                        field(item, "name").map(str::to_owned),
                        id,
                        link,
                        true,
                    ));
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
    file: fs::File,
    length: u64,
    cursor: u64,
    buffer_start: u64,
    buffer: Vec<u8>,
    bytes_read: usize,
}

impl ReverseReader {
    fn open(path: &Path, cursor: u64) -> io::Result<Self> {
        let file = fs::File::open(path)?;
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
    links: &BTreeMap<String, String>,
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
    links: &BTreeMap<String, String>,
    harness_messages: &BTreeSet<u64>,
) -> io::Result<(TranscriptPage, usize)> {
    let mut file = fs::File::open(path)?;
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
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::Duration,
    };

    use serde_json::json;

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

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

        fn viewer(&self) -> MachineView {
            MachineView::new(self.options.clone())
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
        let mut viewer = Viewer {
            core: ViewerCore::new(fixture.options.clone()),
            token: "0123456789abcdef0123456789abcdef".into(),
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
        let mut viewer = fixture.viewer();
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
        let mut viewer = fixture.viewer();
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
        let mut viewer = fixture.viewer();
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
        let mut viewer = fixture.viewer();
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

    const HEADERS: [&str; 4] = [
        "Cache-Control: no-store",
        "Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'",
        "X-Frame-Options: DENY",
        "Referrer-Policy: no-referrer",
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
        let mut viewer = fixture.viewer();
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
            ("/favicon.svg", "image/svg+xml"),
            ("/fonts/instrument-sans-latin.woff2", "font/woff2"),
            ("/fonts/instrument-sans-latin-ext.woff2", "font/woff2"),
            ("/fonts/jetbrains-mono-latin.woff2", "font/woff2"),
            ("/fonts/jetbrains-mono-latin-ext.woff2", "font/woff2"),
            ("/fonts/source-serif-4-latin.woff2", "font/woff2"),
            ("/fonts/source-serif-4-latin-ext.woff2", "font/woff2"),
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
        assert_eq!(urls.len(), 14);
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
    fn shell_font_api_matches_the_font_routes() {
        let fixture = lane_fixture();
        let mut viewer = fixture.viewer();
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
        let js = include_str!("viewer.js");
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
            assert!(!js.contains(banned), "viewer.js uses {banned}");
        }
        assert!(js.contains("textContent"));
        for banned in ["@import", "http://", "https://"] {
            assert!(
                !include_str!("viewer.css").contains(banned),
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
    fn failed_steps_are_listed_where_the_badge_counts_them() {
        let fixture = faults_fixture();
        let mut core = fixture.viewer();
        let body = |core: &mut MachineView, query: &str| -> Value {
            let reply = core.respond("GET", "/api/tx", query, None);
            assert_eq!(reply.status, 200, "{query}");
            serde_json::from_slice(&reply.body).unwrap()
        };
        let model = body_of(&core.respond("GET", "/api/model", "", None));
        let list = body(&mut core, "sid=faults&errors=1");
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
        let mut page = body(&mut core, "sid=faults");
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
            page = body(&mut core, &format!("sid=faults&before={}", page["from"]));
        }
        assert_eq!(slots, failed);
        assert_eq!(slots.len(), 5);
        assert!(
            slots.last().unwrap() - slots.first().unwrap() > 2 * tx::PAGE_ENTRIES as u64,
            "the failures span pages: {slots:?}"
        );
        for slot in slots {
            let after = body(&mut core, &format!("sid=faults&after={slot}"));
            let first = &after["entries"][0];
            assert_eq!(
                (first["slot"].as_u64(), &first["ok"]),
                (Some(slot), &json!(false))
            );
            let before = body(&mut core, &format!("sid=faults&before={}", slot + 1));
            let last = before["entries"].as_array().unwrap().last().unwrap();
            assert_eq!(
                (last["slot"].as_u64(), &last["ok"]),
                (Some(slot), &json!(false))
            );
        }
        // The one never answered is among them, as the badge counts it.
        let unfinished = body(&mut core, "sid=faults&errors=1")["slots"][2]
            .as_u64()
            .unwrap();
        let page = body(&mut core, &format!("sid=faults&after={unfinished}"));
        assert_eq!(page["entries"][0]["unfinished"], true);
    }

    #[test]
    fn the_error_list_refuses_bad_requests_and_unknown_sessions() {
        let fixture = faults_fixture();
        let mut core = fixture.viewer();
        for query in [
            "errors=1",
            "sid=faults&errors=0",
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
        let mut core = fixture.viewer();
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
        let mut core = fixture.viewer();
        let whole: Value =
            serde_json::from_str(&tx::errors(core.built().unwrap(), "faults").unwrap()).unwrap();
        let capped: Value =
            serde_json::from_str(&tx::errors_limited(core.built().unwrap(), "faults", 2).unwrap())
                .unwrap();
        assert_eq!(capped["errors"], 5);
        assert_eq!(capped["truncated"], true);
        assert_eq!(
            capped["slots"].as_array().unwrap()[..],
            whole["slots"].as_array().unwrap()[..2]
        );
        assert_eq!(whole["truncated"], false);
        let exact: Value =
            serde_json::from_str(&tx::errors_limited(core.built().unwrap(), "faults", 5).unwrap())
                .unwrap();
        assert_eq!(exact["truncated"], false);
        assert!(matches!(
            tx::errors(core.built().unwrap(), "nobody").map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        ));
    }

    /// The core answers without a transport: the same bodies, statuses and
    /// `ETag` the loopback server sends, GET only, and the headers it sends
    /// are the ones a server embedding the core is given.
    #[test]
    fn the_core_answers_without_a_transport() {
        let fixture = lane_fixture();
        let mut core = fixture.viewer();
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
        assert_eq!(script.body, include_bytes!("viewer.js"));
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
        let mut viewer = fixture.viewer();
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
            json!({"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":format!("codex {prompt}")}}),
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
        let mut viewer = fixture.viewer();
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
        let mut viewer = fixture.viewer();
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
        let mut viewer = fixture.viewer();
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
        assert!(!include_str!("viewer.js").contains("innerHTML"));
        assert!(include_str!("viewer.js").contains("textContent"));
    }

    #[test]
    fn codex_subagent_and_live_claude_links() {
        let fixture = Fixture::new();
        fixture.codex("root", &[]);
        fixture.write("codex/sessions/2026/09/24/rollout-child.jsonl", &format!("{}\n", json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":{"id":"child","session_id":"root","parent_thread_id":"root","thread_source":"subagent","agent_nickname":"worker"}})));
        let mut viewer = fixture.viewer();
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
        let mut viewer = fixture.viewer();
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

    #[test]
    fn one_machine_through_with_machines_is_todays_viewer_byte_for_byte() {
        let fixture = machine("laptop", "lane");
        let mut today = fixture.viewer();
        let mut core =
            ViewerCore::with_machines(vec![("some-key".into(), fixture.options.clone())]);
        for (path, query) in [
            ("/api/model", ""),
            ("/api/tx", "sid=lane"),
            ("/api/tx", "sid=lane&errors=1"),
            ("/api/tree", ""),
            ("/s/claude/lane", ""),
            ("/machines/laptop", ""),
            ("/viewer.js", ""),
        ] {
            let (a, b) = (
                today.respond("GET", path, query, None),
                core.respond("GET", path, query, None),
            );
            assert_eq!(
                (a.status, a.content_type, a.etag.clone()),
                (b.status, b.content_type, b.etag.clone()),
                "{path}"
            );
            if path == "/api/model" {
                assert_eq!(without_now(&a.body), without_now(&b.body));
            } else {
                assert_eq!(a.body, b.body, "{path}");
            }
        }
        // No admin link unless the embedder sets one, and then first.
        assert!(
            !String::from_utf8_lossy(&core.respond("GET", "/api/model", "", None).body)
                .contains("\"admin\"")
        );
        core.set_admin_link(crate::AdminLink::new("Manage machines", "/admin/machines"));
        let linked = core.respond("GET", "/api/model", "", None);
        assert!(linked.body.starts_with(
            b"{\"admin\":{\"label\":\"Manage machines\",\"href\":\"/admin/machines\"},\"version\":"
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
    }

    #[test]
    fn several_machines_serve_one_model_and_each_answers_for_its_own() {
        let (alpha, bravo) = (machine("alpha", "lane-a"), machine("bravo", "lane-b"));
        let mut core = ViewerCore::with_machines(vec![
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
        let mut core = ViewerCore::with_machines(vec![
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
        let mut core = ViewerCore::with_machines(vec![
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

        let mut core = ViewerCore::with_received(
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
        let mut core = ViewerCore::with_received(Vec::new(), received(&receiver, &local.options));
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
        let mut core = ViewerCore::with_received(
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

        let mut core = ViewerCore::with_received(Vec::new(), received(&receiver, &outside.options));
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
    fn a_session_this_machine_has_is_left_out_of_a_received_one_not_refused() {
        // This machine pushed to itself, or its logs were copied.
        let local = machine("home", "same");
        let receiver = Fixture::new();
        let dir = receiver.root.join("received");
        receive(&dir, "alpha", Some("alpha"), "same");
        receive(&dir, "alpha", None, "lane-alpha");
        let mut core = ViewerCore::with_received(
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
        let mut core =
            ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
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
        let mut core =
            ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
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
        let mut core =
            ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
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
        let mut core =
            ViewerCore::with_received(Vec::new(), received(&receiver, &receiver.options));
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
        let mut core = ViewerCore::with_machines(
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
}

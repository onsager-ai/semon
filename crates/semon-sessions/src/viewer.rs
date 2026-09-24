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
    Index, Node, Options, codex_id_from_filename, codex_meta, collect_with_index, field, file_list,
    is_agent_output, proc_start, read_index, save_index, user_text,
};

const PAGE_ENTRIES: usize = 200;
const PAGE_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_BYTES: usize = 64 * 1024;
const EXPAND_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINE: usize = EXPAND_BYTES + 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub sessions: Options,
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

struct Viewer {
    options: Options,
    token: String,
    port: u16,
    index: Option<Index>,
    index_dirty: bool,
    last_save: Option<Instant>,
    tree: Option<TreeCache>,
    paths: BTreeMap<(String, String), PathBuf>,
    known_paths: BTreeSet<PathBuf>,
    harness: BTreeMap<PathBuf, ((u64, u64), BTreeSet<u64>)>,
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
    locks: Option<String>,
    pids: BTreeMap<u32, Option<u64>>,
}

struct TreeCache {
    roots: Vec<Node>,
    json: String,
    snapshot: Snapshot,
}

fn watch_tree(path: &Path, suffixes: &[&str], watched: &mut BTreeMap<PathBuf, Option<Stamp>>) {
    watched.insert(path.to_owned(), stamp(path));
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            watch_tree(&path, suffixes, watched);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| suffixes.iter().any(|suffix| name.ends_with(suffix)))
        {
            watched.insert(path.clone(), stamp(&path));
        }
    }
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
        for path in index.paths() {
            let path = PathBuf::from(path);
            watched.entry(path.clone()).or_insert_with(|| stamp(&path));
        }
        let mut ids = BTreeSet::new();
        node_pids(roots, &mut ids);
        let pids = ids
            .into_iter()
            .map(|pid| (pid, proc_start(&options.proc_root, pid)))
            .collect();
        Self {
            watched,
            locks: fs::read_to_string(options.proc_root.join("locks")).ok(),
            pids,
        }
    }

    fn changed(&self, options: &Options) -> bool {
        self.watched
            .iter()
            .any(|(path, original)| stamp(path) != *original)
            || fs::read_to_string(options.proc_root.join("locks")).ok() != self.locks
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
    let mut viewer = Viewer::new(options.sessions, random_token()?, port);
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

fn respond(request: Request, status: u16, content_type: &str, body: String, cookie: Option<&str>) {
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    for (name, value) in [
        ("Content-Type", content_type),
        ("Cache-Control", "no-store"),
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self'",
        ),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
    ] {
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

fn decoded(value: &str) -> Option<String> {
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

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
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
    fn new(options: Options, token: String, port: u16) -> Self {
        Self {
            options,
            token,
            port,
            index: None,
            index_dirty: false,
            last_save: None,
            tree: None,
            paths: BTreeMap::new(),
            known_paths: BTreeSet::new(),
            harness: BTreeMap::new(),
        }
    }

    fn update_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let paths: BTreeSet<_> = paths
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl") && path.is_file())
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

    fn handle(&mut self, request: Request) {
        let url = request.url().to_owned();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let Some(set_cookie) = authorized(&request, query, &self.token, self.port) else {
            respond(
                request,
                403,
                "text/plain; charset=utf-8",
                "Forbidden".into(),
                None,
            );
            return;
        };
        let answer = self.route(path, query);
        let (status, content_type, body) = match answer {
            Ok(answer) => answer,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (404, "text/plain; charset=utf-8", "Not found".into())
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                (400, "text/plain; charset=utf-8", "Invalid request".into())
            }
            Err(error) => {
                eprintln!("semon sessions viewer: {error}");
                (500, "text/plain; charset=utf-8", "Internal error".into())
            }
        };
        respond(
            request,
            status,
            content_type,
            body,
            set_cookie.then_some(&self.token),
        );
    }

    fn route(&mut self, path: &str, query: &str) -> io::Result<(u16, &'static str, String)> {
        let html = "text/html; charset=utf-8";
        let json = "application/json; charset=utf-8";
        match path {
            "/" => Ok((200, html, include_str!("viewer.html").into())),
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
            "/api/tree" => Ok((200, json, self.tree_json()?)),
            "/api/transcript" => Ok((200, json, serde_json::to_string(&self.transcript(query)?)?)),
            "/api/entry" => Ok((200, json, self.expand(query)?)),
            _ if path.starts_with("/s/") => {
                let mut parts = path[3..].split('/');
                let harness = parts
                    .next()
                    .and_then(decoded)
                    .ok_or_else(|| invalid_input("harness"))?;
                let id = parts
                    .next()
                    .and_then(decoded)
                    .ok_or_else(|| invalid_input("id"))?;
                if parts.next().is_some() || self.transcript_path(&harness, &id)?.is_none() {
                    return Err(io::ErrorKind::NotFound.into());
                }
                Ok((200, html, include_str!("viewer.html").into()))
            }
            _ => Err(io::ErrorKind::NotFound.into()),
        }
    }

    fn transcript_path(&mut self, harness: &str, id: &str) -> io::Result<Option<PathBuf>> {
        if id.is_empty() || id.len() > 256 || !matches!(harness, "claude" | "codex") {
            return Ok(None);
        }
        if let Some(path) = self.paths.get(&(harness.into(), id.into()))
            && path.is_file()
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

    fn expand(&mut self, query: &str) -> io::Result<String> {
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

fn percent_encode(value: &str) -> String {
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
                codex_home: root.join("codex"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                since: Duration::from_secs(86400),
                session: None,
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

        fn viewer(&self) -> Viewer {
            Viewer::new(
                self.options.clone(),
                "0123456789abcdef0123456789abcdef".into(),
                0,
            )
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
        let mut viewer = fixture.viewer();
        viewer.port = port;
        let request = request.replace("PORT", &port.to_string());
        let worker = thread::spawn(move || viewer.handle(server.recv().unwrap()));
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        worker.join().unwrap();
        response
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
}

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, BufRead, BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod analytics;
mod catalog;
mod catalog_observer;
pub use catalog::{
    CatalogFreshness, CatalogIdentityError, CatalogReadScope, CatalogSessionIdentity,
    CatalogSourceObservation, SessionSourceRef, session_catalog_history_identity,
    session_catalog_identity, session_catalog_page,
};
pub use catalog_observer::SessionCatalogObserver;
mod claude_usage;
pub mod copilot;
pub use claude_usage::ClaudeUsageEvidence;
mod attachments;
pub mod comparison;
mod events;
mod facts;
mod handoff;
pub mod harness;
mod inputs;
pub mod json_string;
mod mcp;
mod model;
mod parent_links;
pub mod pricing;
mod query;
mod read_path;
pub(crate) use read_path::open_input;
mod model_delta;
mod native_field;
mod read_capabilities;
mod source_inventory;
pub use source_inventory::SessionSourceInventory;
mod received;
mod refresh;
mod repo;
pub mod sealed;
pub mod shell;
mod slot_projection;
pub use read_capabilities::{SessionReadEndpoints, session_catalog_capabilities};
mod transcript_range;
pub use transcript_range::{
    SessionSourceReadMode, session_entry_field, session_entry_field_with_mode,
    session_transcript_range, session_transcript_range_with_mode,
};
mod source_reader;
mod tx;
pub use source_reader::{SESSION_SOURCE_CHUNK_MAX, SessionSourceRange, SessionSourceReader};
mod union;
mod viewer;
pub use facts::{
    FACTS_VERSION, Facts, FactsSource, RUN_VARIABLES, ReportedModelUsage, ReportedRunSnapshot,
    local_facts, read_facts, write_facts,
};
pub use harness::{HARNESS_ICONS, HARNESSES, HarnessDefinition, harness};
pub use inputs::{Input, InputRoot, inputs, is_input_path};
pub use mcp::serve_mcp;
pub use model::{MODEL_API, model_json, model_json_at};
pub use query::{DEFAULT_WINDOW, Query, QueryError, QueryTool, query_tools};
pub use received::{ReceivedMachines, is_machine_name};
pub use refresh::RefreshPool;
pub use union::{
    AccountLink, AccountMenu, AccountWorkspace, AdminLink, Extras, LinkMethod, Refresh, ViewerCore,
};
mod remote;
pub use remote::{RemoteLease, collect_remote};
pub use viewer::{
    SECURITY_HEADERS, ServeOptions, ViewerReply, serve, serve_listener, serve_with_control,
};

#[derive(Clone, Debug)]
pub struct Options {
    pub claude_home: PathBuf,
    /// Claude Code's sibling state file. Only allowlisted last-run fields
    /// are read; the default is the `.claude.json` beside `claude_home`.
    /// With CLAUDE_CONFIG_DIR, the native settings file is inside that directory.
    pub claude_json: PathBuf,
    pub codex_home: PathBuf,
    /// Read-only Copilot CLI home; only session-state/<id>/events.jsonl is read.
    pub copilot_home: PathBuf,
    pub proc_root: PathBuf,
    pub cache: PathBuf,
    pub all: bool,
    pub since: Duration,
    pub session: Option<String>,
    /// A facts file ([`Facts`]) to take the machine's side of the model
    /// from, in place of `proc_root`, `/etc/hostname`, `$HOME` and the
    /// working directories on this disk. For logs copied from another
    /// machine.
    pub facts: Option<PathBuf>,
    /// Index only the log files modified within the window (`since`, unless
    /// `all`), instead of every file. A cold build on a large home then
    /// reads weeks, not years, of logs; but a link whose other end is in an
    /// older file isn't made, and that end shows as a stub or unlinked. What
    /// is read is returned whole: the window doesn't trim sessions, turns,
    /// handoffs or busy intervals as well. Off for the viewer and
    /// `--model-json`; the agent read surface turns it on.
    pub scan_window: bool,
}

/// Semon's own state directory: `$XDG_STATE_HOME/semon`, or
/// `~/.local/state/semon`.
fn default_state_dir(home: &Path) -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"))
        .join("semon")
}

/// [`default_state_dir`] for this process's `$HOME`.
pub(crate) fn state_dir() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    default_state_dir(&home)
}

fn native_homes(
    home: &Path,
    claude_config: Option<&std::ffi::OsStr>,
    codex_config: Option<&std::ffi::OsStr>,
) -> (PathBuf, PathBuf, PathBuf) {
    let claude_config = claude_config.filter(|root| !root.is_empty());
    let (claude, settings) = match claude_config {
        Some(root) => (
            PathBuf::from(root),
            PathBuf::from(root).join(".claude.json"),
        ),
        None => (home.join(".claude"), home.join(".claude.json")),
    };
    let codex = codex_config
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    (claude, settings, codex)
}

impl Default for Options {
    fn default() -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let cache = default_state_dir(&home).join("sessions-index.json");
        let (claude_home, claude_json, codex_home) = native_homes(
            &home,
            env::var_os("CLAUDE_CONFIG_DIR").as_deref(),
            env::var_os("CODEX_HOME").as_deref(),
        );
        Self {
            claude_home,
            claude_json,
            codex_home,
            copilot_home: env::var_os("COPILOT_HOME")
                .filter(|root| !root.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".copilot")),
            proc_root: PathBuf::from("/proc"),
            cache,
            all: false,
            since: Duration::from_secs(24 * 60 * 60),
            session: None,
            facts: None,
            scan_window: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
    pub reasoning_output: u64,
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
}

/// JSON schema v1. `last_activity` and `first_activity` are source RFC 3339 strings;
/// `last_activity_age_seconds` is measured at collection time. Children are nested.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<RemoteLease>,
    pub harness: String,
    pub kind: String,
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    pub state: String,
    pub pid: Option<u32>,
    pub models: Vec<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub first_activity: Option<String>,
    pub last_activity: Option<String>,
    pub last_activity_age_seconds: Option<u64>,
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_usage: Option<ClaudeUsageEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copilot: Option<copilot::CopilotEvidence>,
    pub malformed_lines: u64,
    pub open_tools: Vec<ToolCall>,
    pub claude_link: Option<String>,
    pub via_tool: Option<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_source: Option<String>,
    pub unlinked: bool,
    pub children: Vec<Node>,
}

impl Node {
    fn new(id: String, harness: &str, kind: &str) -> Self {
        Self {
            id,
            machine: None,
            lease: None,
            harness: harness.into(),
            kind: kind.into(),
            label: None,
            agent_type: None,
            state: "ended".into(),
            pid: None,
            models: Vec::new(),
            cwd: None,
            branch: None,
            first_activity: None,
            last_activity: None,
            last_activity_age_seconds: None,
            tokens: Tokens::default(),
            claude_usage: None,
            copilot: None,
            malformed_lines: 0,
            open_tools: Vec::new(),
            claude_link: None,
            via_tool: None,
            parent_source: None,
            unlinked: false,
            children: Vec::new(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Index {
    version: u32,
    files: BTreeMap<String, Entry>,
}

impl Index {
    pub(crate) fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

#[derive(Serialize, Deserialize)]
struct Entry {
    dev: u64,
    ino: u64,
    offset: u64,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_ns: u128,
    summary: Summary,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Summary {
    malformed_lines: u64,
    line_count: u64,
    first: Option<String>,
    last: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
    models: BTreeSet<String>,
    usage_by_id: BTreeMap<String, Tokens>,
    #[serde(default)]
    usage_records: claude_usage::Records,
    #[serde(default)]
    usage_models: BTreeMap<String, String>,
    tools: BTreeMap<String, String>,
    closed_tools: BTreeSet<String>,
    codex_tokens: Tokens,
    #[serde(default)]
    codex_native_usage: Option<events::CodexNativeUsage>,
    // Never persisted: a marker can carry a handoff prompt path, and the
    // on-disk cache must not retain session content (risk:secret).
    #[serde(skip)]
    marker: Option<Marker>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Marker {
    harness: String,
    parent_id: String,
    tool_id: Option<String>,
    handoff: Option<String>,
}

impl Summary {
    fn tokens(&self, harness: &str) -> Tokens {
        if harness == "codex" {
            return self.codex_native_usage.as_ref().map_or_else(
                || self.codex_tokens.clone(),
                events::CodexNativeUsage::tokens,
            );
        }
        let mut total = Tokens::default();
        for usage in self.usage_by_id.values() {
            total.input += usage.input;
            total.cached_input += usage.cached_input;
            total.output += usage.output;
            total.reasoning_output += usage.reasoning_output;
            total.total += usage.total;
        }
        total
    }

    fn open_tools(&self) -> Vec<ToolCall> {
        self.tools
            .iter()
            .filter(|(id, _)| !self.closed_tools.contains(*id))
            .map(|(id, name)| ToolCall {
                id: id.clone(),
                name: name.clone(),
            })
            .collect()
    }
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The most of a pid record or a subagent's metadata that is read: a
/// larger one is taken as absent.
pub(crate) const RECORD_MAX: u64 = 1024 * 1024;

/// A pid record's or a subagent metadata file's bytes: at most
/// [`RECORD_MAX`], from a regular file, a symbolic link not followed (the
/// input set has none; see `inputs`). A larger file reads as absent, with
/// one warning per path.
pub(crate) fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    static WARNED: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());
    let read = read_regular_at_most(path, RECORD_MAX);
    if let Err(error) = &read
        && error.kind() == io::ErrorKind::FileTooLarge
        && WARNED
            .lock()
            .is_ok_and(|mut warned| warned.insert(path.to_owned()))
    {
        eprintln!(
            "semon: {} is over {RECORD_MAX} bytes; it is ignored",
            path.display()
        );
    }
    read
}

/// A regular file's bytes, at most `max` of them. The path must name a
/// regular file itself, not a link to one, and the file opened must be that
/// file (the same device and inode, from the opened file's own metadata):
/// otherwise `InvalidInput`. A larger file is `FileTooLarge`.
pub(crate) fn read_regular_at_most(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let linked = fs::symlink_metadata(path)?;
    if !linked.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if linked.len() > max {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    let file = open_input(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    #[cfg(unix)]
    {
        if (opened.dev(), opened.ino()) != (linked.dev(), linked.ino()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    Ok(bytes)
}

fn file_list(root: &Path, output: &mut Vec<PathBuf>, suffix: &str) -> io::Result<()> {
    if fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(());
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            file_list(&path, output, suffix)?;
        } else if ty.is_file() && path.extension().is_some_and(|ext| ext == suffix) {
            output.push(path);
        }
    }
    output.sort();
    Ok(())
}

pub(crate) fn read_index(path: &Path) -> Index {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
        .filter(|index| index.version == 5)
        .unwrap_or_else(|| Index {
            version: 5,
            ..Index::default()
        })
}

pub(crate) fn save_index(path: &Path, index: &Index) -> io::Result<()> {
    save_json(path, index)
}

/// Writes a cache file buffered, to a private temporary file renamed over the
/// old one, so a reader never sees a half-written cache.
pub(crate) fn save_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    save_json_file(path, value).map(drop)
}

/// Keep the descriptor of the exact published bytes for derived-view custody.
/// Looking up the path after rename could instead observe another writer's file.
pub(crate) fn save_json_file(path: &Path, value: &impl Serialize) -> io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Unique per call, not only per process: two threads may save one file
    // at once (a background rebuild, and a request writing facts), and each
    // must rename a whole file of its own.
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let save = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.{save}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&temporary)?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    let mut writer = io::BufWriter::new(file);
    let written = serde_json::to_writer(&mut writer, value)
        .map_err(io::Error::from)
        .and_then(|()| writer.flush());
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let file = writer.into_inner().map_err(|error| error.into_error())?;
    fs::rename(&temporary, path)?;
    Ok(file)
}

fn summarize(
    path: &Path,
    harness: &str,
    index: &mut Index,
    dirty: &mut bool,
) -> io::Result<Summary> {
    let metadata = fs::metadata(path)?;
    let modified_ns = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    #[cfg(unix)]
    let (dev, ino) = (metadata.dev(), metadata.ino());
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    let key = path.to_string_lossy().into_owned();
    let prior = index.files.remove(&key);
    let mut summary = Summary::default();
    let mut offset = 0;
    if let Some(entry) = prior {
        if entry.dev == dev
            && entry.ino == ino
            && entry.size == metadata.len()
            && entry.modified_ns == modified_ns
        {
            let mut result = entry.summary.clone();
            {
                // The marker is never persisted (it can carry a handoff
                // prompt path), so it must be recomputed even when the rest
                // of the file is unchanged. This is a bounded first-turn
                // scan, not a reread of the whole file.
                result.marker = read_first_marker(path)?;
            }
            index.files.insert(key, entry);
            return Ok(result);
        }
        if entry.dev == dev
            && entry.ino == ino
            && entry.offset <= metadata.len()
            && (metadata.len() > entry.size || entry.modified_ns == modified_ns)
        {
            summary = entry.summary;
            offset = entry.offset;
        }
    }
    *dirty = true;
    let mut file = sealed::LogFile::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let length = reader.read_until(b'\n', &mut line)?;
        if length == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            break;
        }
        offset += length as u64;
        summary.line_count += 1;
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            summary.malformed_lines += 1;
            continue;
        };
        if !record.is_object() {
            summary.malformed_lines += 1;
            continue;
        }
        if harness == "claude" {
            update_claude(&mut summary, &record);
        } else {
            update_codex(&mut summary, &record);
        }
    }
    summary.marker = read_first_marker(path)?;
    index.files.insert(
        key,
        Entry {
            dev,
            ino,
            offset,
            size: metadata.len(),
            modified_ns,
            summary: summary.clone(),
        },
    );
    Ok(summary)
}

fn update_activity(summary: &mut Summary, record: &Value) {
    if let Some(timestamp) = field(record, "timestamp").or_else(|| {
        record
            .get("payload")
            .and_then(|payload| field(payload, "timestamp"))
    }) {
        if summary.first.is_none() {
            summary.first = Some(timestamp.into());
        }
        summary.last = Some(timestamp.into());
    }
}

fn update_claude(summary: &mut Summary, record: &Value) {
    update_activity(summary, record);
    if let Some(cwd) = field(record, "cwd") {
        summary.cwd = Some(cwd.into());
    }
    if let Some(branch) = field(record, "gitBranch") {
        summary.branch = Some(branch.into());
    }
    let Some(message) = record.get("message") else {
        return;
    };
    if let Some(model) = field(message, "model") {
        summary.models.insert(model.into());
    }
    if let (Some(id), Some(usage)) = (field(message, "id"), message.get("usage")) {
        if field(record, "type") == Some("assistant")
            && let Some(uuid) = field(record, "uuid")
        {
            summary
                .usage_records
                .entry(id.into())
                .or_default()
                .insert(uuid.into());
        }
        if let Some(model) = field(message, "model") {
            summary.usage_models.insert(id.into(), model.into());
        }
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let creation = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let read = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        summary.usage_by_id.insert(
            id.into(),
            Tokens {
                input: input + creation + read,
                cached_input: read,
                output,
                reasoning_output: 0,
                total: input + creation + read + output,
            },
        );
    }
    if let Some(blocks) = message.get("content").and_then(Value::as_array) {
        for block in blocks {
            match field(block, "type") {
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (field(block, "id"), field(block, "name")) {
                        summary.tools.insert(id.into(), name.into());
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = field(block, "tool_use_id") {
                        summary.closed_tools.insert(id.into());
                    }
                }
                _ => {}
            }
        }
    }
}

fn user_text(record: &Value) -> Option<&str> {
    if field(record, "type") == Some("user") {
        let content = &record["message"]["content"];
        return content.as_str().or_else(|| {
            content
                .as_array()?
                .iter()
                .find_map(|part| field(part, "text"))
        });
    }
    let payload = record.get("payload")?;
    if field(record, "type") == Some("response_item") && field(payload, "role") == Some("user") {
        let content = payload.get("content")?.as_array()?;
        return content.iter().find_map(|part| field(part, "text"));
    }
    if field(record, "type") == Some("event_msg") && field(payload, "type") == Some("user_message")
    {
        return field(payload, "message");
    }
    None
}

fn is_agent_output(record: &Value) -> bool {
    if field(record, "type") == Some("assistant") {
        return true;
    }
    let payload = &record["payload"];
    match field(record, "type") {
        Some("response_item") => {
            matches!(field(payload, "role"), Some("assistant" | "agent"))
                || matches!(
                    field(payload, "type"),
                    Some("function_call" | "custom_tool_call")
                )
        }
        Some("event_msg") => matches!(
            field(payload, "type"),
            Some("agent_message" | "agent_reasoning" | "agent_reasoning_raw_content")
        ),
        _ => false,
    }
}

fn parse_marker(message: &str) -> Option<Marker> {
    let mut lines = message.lines();
    let first = lines.next()?.strip_prefix("Semon-Parent: ")?.trim();
    let (key, tool_id) = parent_links::parse(first)?;
    let (harness, parent_id) = key.split_once(':')?;
    let handoff = lines
        .next()
        .and_then(|line| line.strip_prefix("Semon-Handoff: "))
        .filter(|path| path.starts_with('/'))
        .map(str::to_owned);
    Some(Marker {
        harness: harness.into(),
        parent_id: parent_id.into(),
        tool_id,
        handoff,
    })
}

fn update_codex(summary: &mut Summary, record: &Value) {
    update_activity(summary, record);
    let payload = &record["payload"];
    match field(record, "type") {
        Some("session_meta") => {
            if field(payload, "forked_from_id").is_some() {
                summary
                    .codex_native_usage
                    .get_or_insert_with(Default::default);
            }
            if let Some(cwd) = field(payload, "cwd") {
                summary.cwd = Some(cwd.into());
            }
            if let Some(branch) = payload.get("git").and_then(|git| field(git, "branch")) {
                summary.branch = Some(branch.into());
            }
        }
        Some("turn_context") => {
            if let Some(model) = field(payload, "model") {
                summary.models.insert(model.into());
            }
        }
        Some("token_usage_record") => {
            // Tree summaries preserve available request counts without asserting
            // model attribution; their native model labels are tracked separately.
            events::CodexNativeUsage::observe(
                &mut summary.codex_native_usage,
                payload,
                "unknown".to_owned(),
                None,
            );
        }
        Some("event_msg") if field(payload, "type") == Some("token_count") => {
            if let Some(usage) = payload
                .get("info")
                .and_then(|info| info.get("total_token_usage"))
            {
                summary.codex_tokens = Tokens {
                    input: usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cached_input: usage
                        .get("cached_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    output: usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    reasoning_output: usage
                        .get("reasoning_output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    total: usage
                        .get("total_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                };
            }
        }
        _ => {}
    }
}

fn read_first_marker(path: &Path) -> io::Result<Option<Marker>> {
    let mut reader = BufReader::new(sealed::LogFile::open(path)?);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        if let Ok(record) = serde_json::from_slice::<Value>(&line)
            && record.is_object()
        {
            if is_agent_output(&record) {
                return Ok(None);
            }
            if let Some(message) = user_text(&record)
                && let Some(marker) = parse_marker(message)
            {
                return Ok(Some(marker));
            }
        }
        line.clear();
    }
    Ok(None)
}

fn codex_meta(path: &Path) -> io::Result<Option<Value>> {
    let mut reader = BufReader::new(sealed::LogFile::open(path)?);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    let meta = serde_json::from_slice::<Value>(&line).ok().filter(|value| {
        value.is_object()
            && field(value, "type") == Some("session_meta")
            && field(&value["payload"], "id").is_some()
    });
    Ok(meta.map(|value| value["payload"].clone()))
}

fn codex_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?.strip_prefix("rollout-")?;
    let id = stem
        .len()
        .checked_sub(36)
        .and_then(|start| stem.get(start..))
        .filter(|id| {
            id.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
        });
    Some(id.unwrap_or(stem).to_owned())
}

pub(crate) fn proc_start(root: &Path, pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let close = stat.rfind(')')?;
    stat.get(close + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(unix)]
fn lock_identity(path: &Path) -> io::Result<(u64, u64, u64)> {
    let meta = fs::metadata(path)?;
    let dev = meta.dev();
    Ok((libc_major(dev), libc_minor(dev), meta.ino()))
}

#[cfg(unix)]
fn libc_major(dev: u64) -> u64 {
    ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff)
}
#[cfg(unix)]
fn libc_minor(dev: u64) -> u64 {
    (dev & 0xff) | ((dev >> 12) & !0xff)
}

fn lock_pid(locks: &str, major: u64, minor: u64, ino: u64) -> Option<u32> {
    for line in locks.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.len() < 7 || words[1] != "FLOCK" {
            continue;
        }
        let parts: Vec<_> = words[5].split(':').collect();
        if parts.len() != 3 {
            continue;
        }
        let same = u64::from_str_radix(parts[0], 16).ok() == Some(major)
            && u64::from_str_radix(parts[1], 16).ok() == Some(minor)
            && parts[2].parse::<u64>().ok() == Some(ino);
        if same {
            return words[4].parse().ok();
        }
    }
    None
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn modified_recently(path: &Path, cutoff: u64) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .is_some_and(|time| time.as_secs() >= cutoff)
}

fn apply_summary(node: &mut Node, summary: &Summary, now: u64) {
    node.models = summary.models.iter().cloned().collect();
    node.cwd = summary.cwd.clone().or_else(|| node.cwd.take());
    node.branch = summary.branch.clone().or_else(|| node.branch.take());
    node.first_activity = summary.first.clone();
    node.last_activity = summary.last.clone().or_else(|| node.last_activity.take());
    node.last_activity_age_seconds = node
        .last_activity
        .as_deref()
        .and_then(parse_rfc3339)
        .map(|timestamp| now.saturating_sub(timestamp));
    node.tokens = summary.tokens(&node.harness);
    node.malformed_lines = summary.malformed_lines;
    node.open_tools = summary.open_tools();
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn parse_rfc3339(value: &str) -> Option<u64> {
    let date = value.get(..10)?;
    let time = value.get(11..19)?;
    let year: i64 = date.get(..4)?.parse().ok()?;
    let month: i64 = date.get(5..7)?.parse().ok()?;
    let day: i64 = date.get(8..10)?.parse().ok()?;
    let hour: i64 = time.get(..2)?.parse().ok()?;
    let minute: i64 = time.get(3..5)?.parse().ok()?;
    let second: i64 = time.get(6..8)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let tail = value.get(19..)?;
    let zone = tail.find(['Z', '+', '-'])?;
    let offset = match tail.as_bytes().get(zone)? {
        b'Z' => 0,
        sign => {
            let tz = tail.get(zone + 1..)?;
            let hours: i64 = tz.get(..2)?.parse().ok()?;
            let minutes: i64 = tz.get(3..5)?.parse().ok()?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            (hours * 3600 + minutes * 60) * if *sign == b'+' { 1 } else { -1 }
        }
    };
    (days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second - offset)
        .try_into()
        .ok()
}

fn make_tree(
    key: &str,
    flat: &mut BTreeMap<String, Node>,
    parents: &BTreeMap<String, String>,
    visiting: &mut BTreeSet<String>,
) -> Option<Node> {
    if !visiting.insert(key.into()) {
        return None;
    }
    let mut node = flat.remove(key)?;
    let children: Vec<_> = parents
        .iter()
        .filter(|(_, parent)| parent.as_str() == key)
        .map(|(child, _)| child.clone())
        .collect();
    for child in children {
        if let Some(child) = make_tree(&child, flat, parents, visiting) {
            node.children.push(child);
        }
    }
    node.children
        .sort_by(|a, b| b.last_activity.cmp(&a.last_activity).then(a.id.cmp(&b.id)));
    visiting.remove(key);
    Some(node)
}

fn has_live_process(node: &Node) -> bool {
    node.pid.is_some() || node.children.iter().any(has_live_process)
}

/// Collects a read-only, metadata-only session tree and writes the metadata cache.
pub fn collect(options: &Options) -> io::Result<Vec<Node>> {
    let mut index = read_index(&options.cache);
    let mut dirty = false;
    let result = collect_with_index(options, &mut index, &mut dirty)?;
    if dirty || !options.cache.exists() {
        save_index(&options.cache, &index)?;
    }
    Ok(result)
}

pub(crate) fn collect_with_index(
    options: &Options,
    index: &mut Index,
    dirty: &mut bool,
) -> io::Result<Vec<Node>> {
    let now = unix_now();
    let cutoff = now.saturating_sub(options.since.as_secs());
    let mut flat = BTreeMap::<String, Node>::new();
    let mut parents = BTreeMap::<String, String>::new();
    let mut claude_paths = BTreeMap::<String, PathBuf>::new();
    let mut summaries = BTreeMap::<String, Summary>::new();

    let machine = facts::MachineFacts::of(options);
    let sessions = options.claude_home.join("sessions");
    if let Ok(entries) = fs::read_dir(sessions) {
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(pid_text) = name.strip_suffix(".json") else {
                continue;
            };
            if pid_text.is_empty() || !pid_text.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let Ok(pid) = pid_text.parse::<u32>() else {
                continue;
            };
            let Ok(bytes) = read_regular(&entry.path()) else {
                continue;
            };
            let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if !record.is_object() {
                continue;
            }
            let Some(id) = field(&record, "sessionId") else {
                continue;
            };
            let start = record
                .get("procStart")
                .and_then(Value::as_u64)
                .or_else(|| field(&record, "procStart").and_then(|value| value.parse().ok()));
            if start.is_none() || machine.proc_start(options, pid) != start {
                continue;
            }
            let key = format!("claude:{id}");
            let node = flat
                .entry(key)
                .or_insert_with(|| Node::new(id.into(), "claude", "session"));
            node.state = field(&record, "status").unwrap_or("unknown").into();
            node.pid = Some(pid);
            node.label = field(&record, "name").map(str::to_owned);
            node.cwd = field(&record, "cwd").map(str::to_owned);
            node.last_activity_age_seconds = record
                .get("statusUpdatedAt")
                .and_then(Value::as_u64)
                .map(|ms| now.saturating_sub(ms / 1000));
            node.claude_link = field(&record, "bridgeSessionId")
                .map(|value| format!("https://claude.ai/code/{value}"));
        }
    }

    let mut claude_files = Vec::new();
    file_list(
        &options.claude_home.join("projects"),
        &mut claude_files,
        "jsonl",
    )?;
    for path in claude_files {
        if !options.all && options.session.is_none() && !modified_recently(&path, cutoff) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let is_agent = stem.starts_with("agent-")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "subagents");
        let id = if is_agent {
            stem.strip_prefix("agent-").unwrap_or(stem)
        } else {
            stem
        };
        let key = format!("claude:{id}");
        let summary = summarize(&path, "claude", index, dirty).ok();
        let node = flat.entry(key.clone()).or_insert_with(|| {
            Node::new(
                id.into(),
                "claude",
                if is_agent { "subagent" } else { "session" },
            )
        });
        if is_agent {
            let meta_path = path.with_extension("meta.json");
            if let Some(meta) = read_regular(&meta_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .filter(Value::is_object)
            {
                node.agent_type = field(&meta, "agentType").map(str::to_owned);
                node.label = field(&meta, "description")
                    .or_else(|| field(&meta, "agentType"))
                    .map(str::to_owned);
                node.cwd = field(&meta, "worktreePath").map(str::to_owned);
                node.branch = field(&meta, "worktreeBranch").map(str::to_owned);
                let parent_id = path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str());
                if let Some(parent_id) = parent_id {
                    let parent_key = format!("claude:{parent_id}");
                    parents.insert(key.clone(), parent_key);
                    if let Some(tool_id) = field(&meta, "toolUseId") {
                        node.via_tool = Some(ToolCall {
                            id: tool_id.into(),
                            name: "Task".into(),
                        });
                    }
                }
                node.state = "done".into();
            } else {
                node.state = "unknown".into();
            }
        } else if summary.is_some() {
            claude_paths.insert(id.into(), path.clone());
        }
        if let Some(summary) = summary {
            apply_summary(node, &summary, now);
            summaries.insert(key, summary);
        } else {
            node.state = "unknown".into();
        }
    }

    for (key, parent_key) in &parents {
        let Some(node) = flat.get(key) else {
            continue;
        };
        if node.kind != "subagent" || node.state == "unknown" {
            continue;
        }
        let running = flat
            .get(parent_key)
            .is_some_and(|parent| parent.pid.is_some())
            && node.via_tool.as_ref().is_some_and(|tool| {
                summaries.get(parent_key).is_some_and(|summary| {
                    summary.tools.contains_key(&tool.id) && !summary.closed_tools.contains(&tool.id)
                })
            });
        if running && let Some(node) = flat.get_mut(key) {
            node.state = "running".into();
        }
    }

    let recorded = matches!(machine, facts::MachineFacts::Recorded(_));
    let locks = if recorded {
        None
    } else {
        fs::read_to_string(options.proc_root.join("locks")).ok()
    };
    let mut held_threads = BTreeMap::<String, u32>::new();
    if let facts::MachineFacts::Recorded(facts) = &machine {
        held_threads.extend(facts.codex_locks.clone());
    }
    #[cfg(unix)]
    if let (Some(locks), Ok(entries)) = (
        &locks,
        fs::read_dir(options.codex_home.join("thread-writer-locks")),
    ) {
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".lock")) else {
                continue;
            };
            if let Ok((major, minor, ino)) = lock_identity(&entry.path())
                && let Some(pid) = lock_pid(locks, major, minor, ino)
            {
                held_threads.insert(id.into(), pid);
            }
        }
    }
    let mut codex_files = Vec::new();
    for root in inputs::codex_rollout_dirs(options) {
        file_list(&root, &mut codex_files, "jsonl")?;
    }
    for path in codex_files {
        if !machine.codex_rollout_is_current(options, &path) {
            continue;
        }
        if !options.all && options.session.is_none() && !modified_recently(&path, cutoff) {
            continue;
        }
        let (meta, metadata_unreadable) = match codex_meta(&path) {
            Ok(meta) => (meta, false),
            Err(_) => (None, true),
        };
        let id = meta
            .as_ref()
            .and_then(|meta| field(meta, "id"))
            .map(str::to_owned)
            .or_else(|| codex_id_from_filename(&path))
            .or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let key = format!("codex:{id}");
        let is_agent = meta.as_ref().is_some_and(|meta| {
            field(meta, "thread_source") == Some("subagent")
                || field(meta, "parent_thread_id").is_some()
        });
        let mut node = Node::new(id, "codex", if is_agent { "subagent" } else { "session" });
        node.cwd = meta
            .as_ref()
            .and_then(|meta| field(meta, "cwd"))
            .map(str::to_owned);
        node.branch = meta
            .as_ref()
            .and_then(|meta| meta.get("git"))
            .and_then(|git| field(git, "branch"))
            .map(str::to_owned);
        node.label = match (
            meta.as_ref().and_then(|meta| field(meta, "agent_nickname")),
            meta.as_ref().and_then(|meta| field(meta, "agent_path")),
        ) {
            (Some(name), Some(path)) => Some(format!("{name} ({path})")),
            (Some(name), None) => Some(name.into()),
            (None, Some(path)) => Some(path.into()),
            _ => None,
        };
        node.state = match machine.codex_lock(options, locks.as_deref(), &node.id) {
            facts::Lock::Held(pid) => {
                node.pid = Some(pid);
                "running".into()
            }
            facts::Lock::Free => "ended".into(),
            facts::Lock::Unknown => "unknown".into(),
        };
        let summary = if metadata_unreadable {
            None
        } else {
            summarize(&path, "codex", index, dirty).ok()
        };
        if let Some(summary) = &summary {
            apply_summary(&mut node, summary, now);
            summaries.insert(key.clone(), summary.clone());
        } else {
            node.state = "unknown".into();
        }
        if meta.is_none() {
            node.cwd = None;
            node.branch = None;
        }
        if let Some(parent_id) = meta
            .as_ref()
            .and_then(|meta| field(meta, "parent_thread_id"))
        {
            parents.insert(key.clone(), format!("codex:{parent_id}"));
        } else if let Some(marker) = summary.as_ref().and_then(|summary| summary.marker.as_ref()) {
            let parent_key = format!("{}:{}", marker.harness, marker.parent_id);
            if flat.contains_key(&parent_key) {
                if let Some(prompt) = &marker.handoff {
                    if let Some(parent_path) = claude_paths.get(&marker.parent_id) {
                        node.via_tool = handoff::find(
                            std::slice::from_ref(parent_path),
                            prompt,
                            node.cwd.as_deref(),
                            node.first_activity.as_deref().and_then(events::parse_ms),
                        );
                    }
                } else if let Some(tool_id) = &marker.tool_id {
                    node.via_tool = summaries
                        .get(&parent_key)
                        .and_then(|summary| summary.tools.get(tool_id))
                        .map(|name| ToolCall {
                            id: tool_id.clone(),
                            name: name.clone(),
                        });
                }
                // General markers are resolved after every harness node is known.
            } else {
                node.unlinked = true;
            }
        } else {
            node.unlinked = true;
        }
        flat.insert(key, node);
    }
    for (id, pid) in held_threads {
        let key = format!("codex:{id}");
        flat.entry(key).or_insert_with(|| {
            let mut node = Node::new(id, "codex", "session");
            node.state = "running".into();
            node.pid = Some(pid);
            node
        });
    }

    let known = flat.keys().cloned().collect();
    let processes = flat
        .iter()
        .filter_map(|(key, node)| {
            node.pid.map(|pid| {
                (
                    key.clone(),
                    parent_links::Process {
                        pid,
                        start: machine.proc_start(options, pid),
                    },
                )
            })
        })
        .collect();
    let markers = summaries
        .iter()
        .filter_map(|(key, summary)| summary.marker.clone().map(|marker| (key.clone(), marker)))
        .collect();
    let links = parent_links::resolve(
        options,
        &machine,
        &processes,
        &markers,
        &known,
        &parents,
        i64::try_from(now.saturating_mul(1000)).unwrap_or(i64::MAX),
    );
    for (child, link) in links {
        parents.insert(child.clone(), link.parent.clone());
        let via_tool = link.call.as_ref().and_then(|call| {
            summaries
                .get(&link.parent)?
                .tools
                .get(call)
                .map(|name| ToolCall {
                    id: call.clone(),
                    name: name.clone(),
                })
        });
        if let Some(node) = flat.get_mut(&child) {
            node.parent_source = Some(link.source);
            node.unlinked = false;
            if via_tool.is_some() {
                node.via_tool = via_tool;
            }
        }
    }

    let copilot_inputs: Vec<_> = inputs::inputs(options)?
        .into_iter()
        .filter(|input| input.root == inputs::InputRoot::Copilot)
        .collect();
    if !copilot_inputs.is_empty() {
        let mut cache = events::EventCache::open(&options.cache);
        let mut changed = false;
        for input in copilot_inputs {
            let path = input.full_path(options);
            let id = input
                .path
                .split('/')
                .nth(1)
                .expect("allowlisted session id");
            if copilot::header(&path, id)?.is_none() {
                continue;
            }
            let summary = events::scan_file(&path, "copilot", &mut cache, &mut changed)?;
            let mut node = Node::new(id.into(), "copilot", "session");
            node.state = "unknown".into();
            node.cwd = summary.cwd.clone();
            node.tokens = summary.tokens("copilot");
            node.models = summary
                .signals
                .iter()
                .filter(|signal| signal.k == events::SignalKind::Model)
                .filter_map(|signal| signal.n.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            node.copilot = summary.copilot.as_ref().map(|index| index.evidence());
            node.first_activity = summary
                .copilot
                .as_ref()
                .and_then(|index| index.first.clone());
            node.last_activity = summary
                .copilot
                .as_ref()
                .and_then(|index| index.last.clone());
            node.last_activity_age_seconds = node
                .last_activity
                .as_deref()
                .and_then(parse_rfc3339)
                .map(|last| now.saturating_sub(last));
            node.open_tools = summary
                .events
                .iter()
                .filter(|event| event.k == events::Kind::Tool && event.r.is_none())
                .filter_map(|event| {
                    Some(ToolCall {
                        id: event.id.clone()?,
                        name: event.n.clone()?,
                    })
                })
                .collect();
            flat.insert(format!("copilot:{id}"), node);
        }
    }
    apply_claude_usage(&mut flat, &summaries);
    if let Some(id) = &options.session {
        let key = if id.starts_with("claude:")
            || id.starts_with("codex:")
            || id.starts_with("copilot:")
        {
            id.clone()
        } else if flat.contains_key(&format!("claude:{id}")) {
            format!("claude:{id}")
        } else if flat.contains_key(&format!("copilot:{id}")) {
            format!("copilot:{id}")
        } else {
            format!("codex:{id}")
        };
        return Ok(make_tree(&key, &mut flat, &parents, &mut BTreeSet::new())
            .into_iter()
            .collect());
    }
    let root_keys: Vec<_> = flat
        .keys()
        .filter(|key| {
            !parents
                .get(*key)
                .is_some_and(|parent| flat.contains_key(parent))
        })
        .cloned()
        .collect();
    let mut roots = Vec::new();
    for key in root_keys {
        if let Some(root) = make_tree(&key, &mut flat, &parents, &mut BTreeSet::new())
            && (options.all
                || has_live_process(&root)
                || root
                    .last_activity_age_seconds
                    .is_some_and(|age| age <= options.since.as_secs()))
        {
            roots.push(root);
        }
    }
    roots.sort_by(|a, b| b.last_activity.cmp(&a.last_activity).then(a.id.cmp(&b.id)));
    Ok(roots)
}

pub fn render_json(nodes: &[Node]) -> String {
    serde_json::to_string_pretty(&serde_json::json!({"schema_version": 1, "roots": nodes}))
        .expect("serializable nodes")
}

pub fn render_text(nodes: &[Node]) -> String {
    fn draw(node: &Node, depth: usize, output: &mut String) {
        if node.kind == "machine" {
            output.push_str(&format!("{}machine {}\n", "  ".repeat(depth), node.id));
            for child in &node.children {
                draw(child, depth + 1, output);
            }
            return;
        }
        let home = env::var("HOME").unwrap_or_default();
        let cwd = node
            .cwd
            .as_deref()
            .map(|cwd| {
                cwd.strip_prefix(&home)
                    .map(|rest| format!("~{rest}"))
                    .unwrap_or_else(|| cwd.into())
            })
            .unwrap_or_else(|| "?".into());
        let age = node
            .last_activity_age_seconds
            .map(|seconds| {
                if seconds < 60 {
                    format!("{seconds}s")
                } else if seconds < 3600 {
                    format!("{}m", seconds / 60)
                } else if seconds < 86400 {
                    format!("{}h", seconds / 3600)
                } else {
                    format!("{}d", seconds / 86400)
                }
            })
            .unwrap_or_else(|| "?".into());
        let tools = node
            .open_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let label = node.label.as_deref().unwrap_or(&node.id);
        let via = node
            .via_tool
            .as_ref()
            .map(|tool| format!(" via {}:{}", tool.name, tool.id))
            .unwrap_or_default();
        let link = node
            .claude_link
            .as_ref()
            .map(|link| format!(" {link}"))
            .unwrap_or_default();
        let unlinked = if node.unlinked { " unlinked" } else { "" };
        let malformed = if node.malformed_lines > 0 {
            format!(" malformed:{}", node.malformed_lines)
        } else {
            String::new()
        };
        let mut line = format!(
            "{}{} {} {} [{}] models={} cwd={} branch={} age={} in={} out={} open={}{}{}{}{}{}",
            "  ".repeat(depth),
            node.harness,
            node.kind,
            label,
            node.state,
            node.models.join(","),
            cwd,
            node.branch.as_deref().unwrap_or("?"),
            age,
            node.tokens.input,
            node.tokens.output,
            node.open_tools.len(),
            if tools.is_empty() {
                String::new()
            } else {
                format!("({tools})")
            },
            via,
            link,
            unlinked,
            malformed
        );
        if node.claude_usage.is_some() {
            line.push_str(" usage=exclusive-observations fresh=unknown copied-owner=unknown");
        }
        if let Some(agent_type) = &node.agent_type {
            line.push_str(&format!(" agentType={agent_type}"));
        }
        if let Some(machine) = &node.machine {
            line.push_str(&format!(" machine={machine}"));
            if let Some(lease) = &node.lease {
                line.push_str(&format!(" epoch={}", lease.epoch));
            }
        }
        output.extend(line.chars().map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        }));
        output.push('\n');
        for child in &node.children {
            draw(child, depth + 1, output);
        }
    }
    let mut output = String::new();
    for node in nodes {
        draw(node, 0, &mut output);
    }
    if output.is_empty() {
        output.push_str("(no sessions)\n");
    }
    output
}

pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .ok_or_else(|| format!("invalid duration: {value}"))?;
    let amount = value[..split]
        .parse::<u64>()
        .map_err(|_| format!("invalid duration: {value}"))?;
    let unit = &value[split..];
    let factor: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => return Err(format!("invalid duration: {value}")),
    };
    amount
        .checked_mul(factor)
        .map(Duration::from_secs)
        .ok_or_else(|| format!("duration too large: {value}"))
}

fn apply_claude_usage(flat: &mut BTreeMap<String, Node>, summaries: &BTreeMap<String, Summary>) {
    let shared = claude_usage::shared(
        summaries
            .iter()
            .map(|(key, summary)| (key.as_str(), &summary.usage_records)),
    );
    for (key, ids) in shared {
        if let (Some(node), Some(summary)) = (flat.get_mut(&key), summaries.get(&key))
            && let Some(evidence) = claude_usage::evidence(
                summary.usage_by_id.iter().map(|(id, tokens)| {
                    (id, tokens, summary.usage_models.get(id).map(String::as_str))
                }),
                &ids,
            )
        {
            node.tokens = evidence.exclusive.clone();
            node.claude_usage = Some(evidence);
        }
    }
}

/// Open a native input read-only, binding every directory component without
/// following symlinks and rejecting FIFOs and other non-regular files.
pub fn open_read_only_input(path: &Path) -> io::Result<fs::File> {
    open_input(path)
}

#[cfg(test)]
mod native_home_tests {
    use super::*;
    use std::ffi::OsStr;
    #[test]
    fn configured_roots_discover_isolated_native_transcripts_read_only() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            env::temp_dir().join(format!("semon-native-homes-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let claude_config = root.join("claude-config");
        let codex_config = root.join("codex-config");
        let (claude_home, claude_json, codex_home) = native_homes(
            &root.join("unused-fallback"),
            Some(claude_config.as_os_str()),
            Some(codex_config.as_os_str()),
        );
        // Guard the test's read surface before discovery, even if resolution regresses.
        assert!(
            claude_home.starts_with(&root)
                && claude_json.starts_with(&root)
                && codex_home.starts_with(&root)
        );
        let claude_id = "00000000-0000-4000-8000-000000000001";
        let codex_id = "00000000-0000-4000-8000-000000000002";
        let claude_path = claude_home
            .join("projects/fixture-repo")
            .join(format!("{claude_id}.jsonl"));
        let codex_path = codex_home
            .join("sessions/2026/10/03")
            .join(format!("rollout-2026-10-03T07-49-53-{codex_id}.jsonl"));
        std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
        let claude = include_str!(
            "../../../tests/fixtures/compatibility/claude-2.1.288/initial-transcript.jsonl"
        )
        .replace("native-claude-compat", claude_id);
        let codex = include_str!(
            "../../../tests/fixtures/compatibility/codex-0.159.0-alpha.3/initial-rollout.jsonl"
        )
        .replace("native-codex-compat", codex_id);
        std::fs::write(&claude_path, &claude).unwrap();
        std::fs::write(&codex_path, &codex).unwrap();
        std::fs::write(&claude_json, b"{}").unwrap();
        let options = Options {
            claude_home,
            claude_json: claude_json.clone(),
            codex_home,
            copilot_home: root.join("copilot"),
            proc_root: root.join("empty-proc"),
            cache: root.join("cache/index.json"),
            all: true,
            ..Options::default()
        };
        let nodes = collect(&options).unwrap();
        assert!(
            nodes
                .iter()
                .any(|node| node.harness == "claude" && node.id == claude_id)
        );
        assert!(
            nodes
                .iter()
                .any(|node| node.harness == "codex" && node.id == codex_id)
        );
        assert_eq!(std::fs::read(&claude_path).unwrap(), claude.as_bytes());
        assert_eq!(std::fs::read(&codex_path).unwrap(), codex.as_bytes());
        assert_eq!(std::fs::read(&claude_json).unwrap(), b"{}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_homes_use_the_observed_native_settings_layout() {
        let home = Path::new("/fixture/fallback");
        assert_eq!(
            native_homes(
                home,
                Some(OsStr::new("/fixture/claude-config")),
                Some(OsStr::new("/fixture/codex-config"))
            ),
            (
                PathBuf::from("/fixture/claude-config"),
                PathBuf::from("/fixture/claude-config/.claude.json"),
                PathBuf::from("/fixture/codex-config")
            )
        );
        assert_eq!(
            native_homes(home, None, None),
            (
                home.join(".claude"),
                home.join(".claude.json"),
                home.join(".codex")
            )
        );
        assert_eq!(
            native_homes(home, Some(OsStr::new("")), Some(OsStr::new(""))),
            native_homes(home, None, None)
        );
    }
}

mod retention;
pub use retention::{SourceProjectionReady, source_projection_ready};

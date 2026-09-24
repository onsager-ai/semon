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

#[derive(Clone, Debug)]
pub struct Options {
    pub claude_home: PathBuf,
    pub codex_home: PathBuf,
    pub proc_root: PathBuf,
    pub cache: PathBuf,
    pub all: bool,
    pub since: Duration,
    pub session: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let cache = env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("semon/sessions-index.json");
        Self {
            claude_home: home.join(".claude"),
            codex_home: home.join(".codex"),
            proc_root: PathBuf::from("/proc"),
            cache,
            all: false,
            since: Duration::from_secs(24 * 60 * 60),
            session: None,
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
    pub harness: String,
    pub kind: String,
    pub label: Option<String>,
    pub state: String,
    pub pid: Option<u32>,
    pub models: Vec<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub first_activity: Option<String>,
    pub last_activity: Option<String>,
    pub last_activity_age_seconds: Option<u64>,
    pub tokens: Tokens,
    pub malformed_lines: u64,
    pub open_tools: Vec<ToolCall>,
    pub claude_link: Option<String>,
    pub via_tool: Option<ToolCall>,
    pub unlinked: bool,
    pub children: Vec<Node>,
}

impl Node {
    fn new(id: String, harness: &str, kind: &str) -> Self {
        Self {
            id,
            harness: harness.into(),
            kind: kind.into(),
            label: None,
            state: "ended".into(),
            pid: None,
            models: Vec::new(),
            cwd: None,
            branch: None,
            first_activity: None,
            last_activity: None,
            last_activity_age_seconds: None,
            tokens: Tokens::default(),
            malformed_lines: 0,
            open_tools: Vec::new(),
            claude_link: None,
            via_tool: None,
            unlinked: false,
            children: Vec::new(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    version: u32,
    files: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    dev: u64,
    ino: u64,
    offset: u64,
    summary: Summary,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Summary {
    malformed_lines: u64,
    first: Option<String>,
    last: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
    models: BTreeSet<String>,
    usage_by_id: BTreeMap<String, Tokens>,
    tools: BTreeMap<String, String>,
    closed_tools: BTreeSet<String>,
    codex_tokens: Tokens,
    #[serde(skip)]
    marker: Option<Marker>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Marker {
    claude_id: String,
    tool_id: Option<String>,
    handoff: Option<String>,
}

impl Summary {
    fn tokens(&self, harness: &str) -> Tokens {
        if harness == "codex" {
            return self.codex_tokens.clone();
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

fn file_list(root: &Path, output: &mut Vec<PathBuf>, suffix: &str) -> io::Result<()> {
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

fn read_index(path: &Path) -> Index {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
        .filter(|index| index.version == 2)
        .unwrap_or_else(|| Index {
            version: 2,
            ..Index::default()
        })
}

fn save_index(path: &Path, index: &Index) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut file, index)?;
    file.flush()
}

fn summarize(path: &Path, harness: &str, index: &mut Index) -> io::Result<Summary> {
    let metadata = fs::metadata(path)?;
    #[cfg(unix)]
    let (dev, ino) = (metadata.dev(), metadata.ino());
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    let key = path.to_string_lossy().into_owned();
    let prior = index.files.remove(&key);
    let mut summary = Summary::default();
    let mut offset = 0;
    if let Some(entry) = prior
        && entry.dev == dev
        && entry.ino == ino
        && entry.offset <= metadata.len()
    {
        summary = entry.summary;
        offset = entry.offset;
    }
    let mut file = fs::File::open(path)?;
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
    if harness == "codex" {
        summary.marker = read_first_marker(path)?;
    }
    index.files.insert(
        key,
        Entry {
            dev,
            ino,
            offset,
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
    let first = lines.next()?.strip_prefix("Semon-Parent: claude:")?.trim();
    let (claude_id, tool_id) = match first.split_once(':') {
        Some((session, tool)) => (session, Some(tool.to_owned())),
        None => (first, None),
    };
    if claude_id.is_empty() {
        return None;
    }
    let handoff = lines
        .next()
        .and_then(|line| line.strip_prefix("Semon-Handoff: "))
        .filter(|path| path.starts_with('/'))
        .map(str::to_owned);
    Some(Marker {
        claude_id: claude_id.into(),
        tool_id,
        handoff,
    })
}

fn update_codex(summary: &mut Summary, record: &Value) {
    update_activity(summary, record);
    let payload = &record["payload"];
    match field(record, "type") {
        Some("session_meta") => {
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
    let mut reader = BufReader::new(fs::File::open(path)?);
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
    let mut reader = BufReader::new(fs::File::open(path)?);
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

fn proc_start(root: &Path, pid: u32) -> Option<u64> {
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

fn matches_handoff(path: &Path, prompt: &str) -> io::Result<Option<ToolCall>> {
    let mut file = BufReader::new(fs::File::open(path)?);
    let mut line = Vec::new();
    while file.read_until(b'\n', &mut line)? != 0 {
        if let Ok(record) = serde_json::from_slice::<Value>(&line)
            && let Some(blocks) = record
                .get("message")
                .and_then(|msg| msg.get("content"))
                .and_then(Value::as_array)
        {
            for block in blocks {
                if field(block, "type") != Some("tool_use") {
                    continue;
                }
                let Some(name) = field(block, "name") else {
                    continue;
                };
                if name != "Bash" && name != "Skill" {
                    continue;
                }
                if block
                    .get("input")
                    .is_some_and(|input| input.to_string().contains(prompt))
                    && let Some(id) = field(block, "id")
                {
                    return Ok(Some(ToolCall {
                        id: id.into(),
                        name: name.into(),
                    }));
                }
            }
        }
        line.clear();
    }
    Ok(None)
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
    let now = unix_now();
    let cutoff = now.saturating_sub(options.since.as_secs());
    let mut index = read_index(&options.cache);
    let mut flat = BTreeMap::<String, Node>::new();
    let mut parents = BTreeMap::<String, String>::new();
    let mut claude_paths = BTreeMap::<String, PathBuf>::new();
    let mut summaries = BTreeMap::<String, Summary>::new();

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
            let Ok(bytes) = fs::read(entry.path()) else {
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
            if start.is_none() || proc_start(&options.proc_root, pid) != start {
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
        let summary = summarize(&path, "claude", &mut index).ok();
        let node = flat.entry(key.clone()).or_insert_with(|| {
            Node::new(
                id.into(),
                "claude",
                if is_agent { "subagent" } else { "session" },
            )
        });
        if is_agent {
            let meta_path = path.with_extension("meta.json");
            if let Some(meta) = fs::read(&meta_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .filter(Value::is_object)
            {
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

    let locks = fs::read_to_string(options.proc_root.join("locks")).ok();
    let mut held_threads = BTreeMap::<String, u32>::new();
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
    file_list(
        &options.codex_home.join("sessions"),
        &mut codex_files,
        "jsonl",
    )?;
    for path in codex_files {
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
        #[cfg(unix)]
        {
            node.state = match &locks {
                None => "unknown".into(),
                Some(locks) => match lock_identity(
                    &options
                        .codex_home
                        .join("thread-writer-locks")
                        .join(format!("{}.lock", node.id)),
                ) {
                    Ok((major, minor, ino)) => match lock_pid(locks, major, minor, ino) {
                        Some(pid) => {
                            node.pid = Some(pid);
                            "running".into()
                        }
                        None => "ended".into(),
                    },
                    Err(error) if error.kind() == io::ErrorKind::NotFound => "ended".into(),
                    Err(_) => "unknown".into(),
                },
            };
        }
        #[cfg(not(unix))]
        {
            node.state = "unknown".into();
        }
        let summary = if metadata_unreadable {
            None
        } else {
            summarize(&path, "codex", &mut index).ok()
        };
        if let Some(summary) = &summary {
            apply_summary(&mut node, summary, now);
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
            let parent_key = format!("claude:{}", marker.claude_id);
            if flat.contains_key(&parent_key) {
                if let Some(prompt) = &marker.handoff {
                    if let Some(parent_path) = claude_paths.get(&marker.claude_id) {
                        node.via_tool = matches_handoff(parent_path, prompt).ok().flatten();
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
                parents.insert(key.clone(), parent_key);
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
    save_index(&options.cache, &index)?;

    if let Some(id) = &options.session {
        let key = if id.starts_with("claude:") || id.starts_with("codex:") {
            id.clone()
        } else if flat.contains_key(&format!("claude:{id}")) {
            format!("claude:{id}")
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
        let line = format!(
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

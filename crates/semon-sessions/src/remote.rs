//! Encrypted relay views. Only summaries and verification watermarks cross
//! the cache boundary; decrypted records and data keys never do.
use super::*;
use semon_relay::{
    FRAME_PAGE_MAX_BYTES, FRAME_PAGE_MAX_FRAMES, FrameMode, MachineIdentity, StreamTip, Transport,
    decrypt_envelope, verify_encrypted_frame,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Default, Serialize, Deserialize)]
struct RemoteIndex {
    version: u32,
    namespace: String,
    #[serde(default)]
    deletion_revision: String,
    streams: BTreeMap<String, RemoteEntry>,
    #[serde(default)]
    sidecars: BTreeMap<String, RemoteSidecars>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct RemoteSidecars {
    manifest: String,
    session: String,
    conflicted: bool,
    metadata: BTreeMap<String, Sidecar>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Sidecar {
    agent_type: Option<String>,
    claude_link: Option<String>,
    label: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
    tool: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct RemoteEntry {
    session: String,
    stream: String,
    generation: u64,
    seq: u64,
    epoch: u64,
    chain: [u8; 32],
    tag: [u8; 32],
    machine: String,
    summary: Summary,
    parent: Option<String>,
    parent_tool: Option<String>,
    marker_done: bool,
    label: Option<String>,
    parent_source: Option<String>,
    #[serde(default)]
    unavailable: bool,
}
impl RemoteEntry {
    fn tip(&self) -> StreamTip {
        StreamTip {
            stream: self.stream.clone(),
            generation: self.generation,
            epoch: self.epoch,
            seq: self.seq,
            mode: FrameMode::Encrypted,
            chain: Some(self.chain),
            tag: Some(self.tag),
        }
    }
    fn record(&mut self, value: &Value) {
        self.summary.line_count += 1;
        let harness = if self.stream.starts_with("codex/") {
            "codex"
        } else {
            "claude"
        };
        if harness == "codex" {
            update_codex(&mut self.summary, value);
        } else {
            update_claude(&mut self.summary, value);
        }
        if !self.marker_done {
            if let Some(text) = user_text(value)
                && let Some(marker) = parse_marker(text)
            {
                // Persist identifiers only; a Semon-Handoff path is never cached.
                self.parent = Some(format!("{}:{}", marker.harness, marker.parent_id));
                self.parent_tool = marker.tool_id;
                self.parent_source = Some("marker".into());
                self.marker_done = true;
            } else if is_agent_output(value) {
                self.marker_done = true;
            }
        }
        if harness == "codex" {
            let payload = &value["payload"];
            if field(value, "type") == Some("session_meta") {
                if let Some(parent) = field(payload, "parent_thread_id")
                    .or_else(|| {
                        payload
                            .pointer("/source/subagent/thread_spawn/parent_thread_id")
                            .and_then(Value::as_str)
                    })
                    .or_else(|| {
                        payload
                            .pointer("/source/subagent/parent_thread_id")
                            .and_then(Value::as_str)
                    })
                {
                    self.parent = Some(format!("codex:{parent}"));
                    self.parent_source = Some("native".into());
                }
                self.label = field(payload, "agent_nickname").map(str::to_owned);
            }
            if field(value, "type") == Some("response_item") {
                match field(payload, "type") {
                    Some("function_call" | "custom_tool_call") => {
                        if let (Some(id), Some(name)) =
                            (field(payload, "call_id"), field(payload, "name"))
                        {
                            self.summary.tools.insert(id.into(), name.into());
                        }
                    }
                    Some("function_call_output" | "custom_tool_call_output") => {
                        if let Some(id) = field(payload, "call_id") {
                            self.summary.closed_tools.insert(id.into());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn remote_error(error: impl std::fmt::Display) -> io::Error {
    invalid(error.to_string())
}
fn stream_key(session: &str, stream: &str, generation: u64) -> String {
    serde_json::to_string(&(session, stream, generation)).expect("string tuple")
}
fn cache_path(path: &Path, namespace: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".remote.{}.json", namespace));
    PathBuf::from(name)
}

/// Collect an encrypted relay tree with machine roots. Requests remain signed
/// by the supplied transport; only enrolled age recipients can decrypt frames.
/// The incremental cache is a private, endpoint/identity-scoped metadata file
/// beside `options.cache`. Local homes and process information are not read.
pub fn collect_remote(
    options: &Options,
    identity: &MachineIdentity,
    transport: &impl Transport,
) -> io::Result<Vec<Node>> {
    let machine = identity.fingerprint();
    let namespace = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{machine}", transport.deletion_scope()))
    );
    let path = cache_path(&options.cache, &namespace);
    let mut index = read_regular_at_most(&path, 64 * 1024 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RemoteIndex>(&bytes).ok())
        .filter(|index| index.version == 3 && index.namespace == namespace)
        .unwrap_or(RemoteIndex {
            version: 3,
            namespace,
            deletion_revision: String::new(),
            streams: BTreeMap::new(),
            sidecars: BTreeMap::new(),
        });
    let old = serde_json::to_vec(&index).map_err(remote_error)?;
    let mut current = BTreeSet::new();
    let (status, observed_ms, revision) = transport
        .lease_observation(&machine)
        .map_err(remote_error)?;
    if index.deletion_revision != revision {
        index.streams.clear();
        index.deletion_revision = revision;
    }
    let sessions = transport
        .list_envelope_sessions(&machine)
        .map_err(remote_error)?;
    for session in sessions {
        let envelope = transport
            .get_envelope(&session, &machine)
            .map_err(remote_error)?
            .ok_or_else(|| invalid("remote session envelope is missing"))?;
        let key = decrypt_envelope(&envelope, &identity.age).map_err(remote_error)?;
        let tips = transport
            .lease_tips(&session, &machine)
            .map_err(remote_error)?
            .tips;
        // Old generations remain archived; the view follows each stream's latest.
        let mut latest = BTreeMap::<String, StreamTip>::new();
        for tip in tips {
            if tip.mode != FrameMode::Encrypted || tip.tag.is_none() {
                return Err(invalid("remote view requires encrypted frames"));
            }
            if latest
                .get(&tip.stream)
                .is_none_or(|old| old.generation < tip.generation)
            {
                latest.insert(tip.stream.clone(), tip);
            }
        }
        for tip in latest.into_values() {
            let cache_key = stream_key(&session, &tip.stream, tip.generation);
            current.insert(cache_key.clone());
            let existing = index.streams.get(&cache_key).cloned();
            let mut entry = existing.filter(|entry| {
                entry.seq <= tip.seq
                    && (entry.seq != tip.seq
                        || (entry.tag == tip.tag.unwrap_or_default() && entry.epoch == tip.epoch))
            });
            if entry
                .as_ref()
                .is_some_and(|entry| !entry.unavailable && entry.seq == tip.seq)
            {
                continue;
            }
            if entry.as_ref().is_some_and(|entry| entry.unavailable) {
                entry = None;
            }
            let mut previous = entry.as_ref().map(RemoteEntry::tip);
            let mut cursor = previous.as_ref().map(|tip| tip.seq);
            let mut unavailable = false;
            loop {
                let page = transport
                    .list_frame_page(
                        &session,
                        &machine,
                        &tip.stream,
                        tip.generation,
                        cursor,
                        FRAME_PAGE_MAX_BYTES,
                        FRAME_PAGE_MAX_FRAMES,
                    )
                    .map_err(remote_error)?;
                if page.frames.len() > FRAME_PAGE_MAX_FRAMES {
                    return Err(invalid("remote frame page exceeds its frame limit"));
                }
                if page.frames.is_empty() {
                    unavailable = true;
                    break;
                }
                for frame in page.frames {
                    if frame.key.stream != tip.stream || frame.key.generation != tip.generation {
                        return Err(invalid("remote frame page changed stream or generation"));
                    }
                    if frame.key.seq > tip.seq {
                        break;
                    }
                    let author = frame.machine.clone();
                    let line = match verify_encrypted_frame(&session, &key, &mut previous, frame) {
                        Ok(line) => line,
                        Err(semon_relay::VerifyError::Gap { .. }) => {
                            unavailable = true;
                            break;
                        }
                        Err(error) => return Err(remote_error(error)),
                    };
                    let verified = previous.as_ref().expect("verified frame has a tip");
                    let row = entry.get_or_insert_with(|| RemoteEntry {
                        session: session.clone(),
                        stream: tip.stream.clone(),
                        generation: tip.generation,
                        seq: verified.seq,
                        epoch: verified.epoch,
                        chain: verified.chain.unwrap_or_default(),
                        tag: verified.tag.unwrap_or_default(),
                        machine: author.clone(),
                        summary: Summary::default(),
                        parent: None,
                        parent_tool: None,
                        marker_done: false,
                        label: None,
                        parent_source: None,
                        unavailable: false,
                    });
                    row.seq = verified.seq;
                    row.epoch = verified.epoch;
                    row.chain = verified
                        .chain
                        .ok_or_else(|| invalid("verified chain missing"))?;
                    row.tag = verified
                        .tag
                        .ok_or_else(|| invalid("verified tag missing"))?;
                    row.machine = author;
                    match serde_json::from_slice::<Value>(&line) {
                        Ok(value) if value.is_object() => row.record(&value),
                        _ => row.summary.malformed_lines += 1,
                    }
                }
                if unavailable {
                    break;
                }
                if previous
                    .as_ref()
                    .is_some_and(|verified| verified.seq == tip.seq)
                {
                    break;
                }
                match page.next {
                    Some(next)
                        if cursor.is_none_or(|old| next > old)
                            && previous
                                .as_ref()
                                .is_some_and(|verified| verified.seq == next) =>
                    {
                        cursor = Some(next);
                    }
                    _ => return Err(invalid("remote frame page cursor did not advance")),
                }
            }
            if unavailable {
                index.streams.insert(
                    cache_key,
                    RemoteEntry {
                        session: session.clone(),
                        stream: tip.stream.clone(),
                        generation: tip.generation,
                        seq: tip.seq,
                        epoch: tip.epoch,
                        chain: [0; 32],
                        tag: tip.tag.unwrap_or_default(),
                        machine: String::new(),
                        summary: Summary::default(),
                        parent: None,
                        parent_tool: None,
                        marker_done: false,
                        label: None,
                        parent_source: None,
                        unavailable: true,
                    },
                );
                continue;
            }
            let verified = previous.ok_or_else(|| invalid("remote tip missing"))?;
            if verified.tag != tip.tag || verified.epoch != tip.epoch || verified.seq != tip.seq {
                return Err(invalid("remote verified tip does not match receiver tip"));
            }
            index.streams.insert(
                cache_key,
                entry.ok_or_else(|| invalid("remote summary missing"))?,
            );
        }
    }
    index.streams.retain(|key, _| current.contains(key));
    update_sidecars(&mut index, &machine, identity, transport)?;
    let nodes = assemble(options, &index, &status, observed_ms);
    if old != serde_json::to_vec(&index).map_err(remote_error)? || !path.exists() {
        save_json(&path, &index)?;
    }
    Ok(nodes)
}
fn update_sidecars(
    index: &mut RemoteIndex,
    machine: &str,
    identity: &MachineIdentity,
    transport: &impl Transport,
) -> io::Result<()> {
    let roots = semon_relay::list_snapshot_roots(machine, transport).map_err(remote_error)?;
    let mut current = BTreeSet::new();
    for root in roots {
        let history =
            semon_relay::list_snapshot_heads(&root, machine, transport).map_err(remote_error)?;
        let heads = history["heads"]
            .as_array()
            .ok_or_else(|| invalid("snapshot heads missing"))?;
        let manifests = history["manifests"]
            .as_array()
            .ok_or_else(|| invalid("snapshot manifests missing"))?;
        let Some(wire) = manifests
            .iter()
            .rev()
            .find(|wire| heads.contains(&wire["id"]))
        else {
            continue;
        };
        let id = field(wire, "id").ok_or_else(|| invalid("snapshot manifest id missing"))?;
        current.insert(root.clone());
        if index
            .sidecars
            .get(&root)
            .is_some_and(|cached| cached.manifest == id && cached.conflicted == (heads.len() > 1))
        {
            continue;
        }
        let encrypted = transport
            .snapshot_request(
                "/v1/snapshots/get",
                &serde_json::json!({"root":root,"machine":machine,"kind":"manifest","id":id}),
            )
            .map_err(remote_error)?;
        if field(&encrypted, "id") != Some(id) {
            return Err(invalid("snapshot manifest substitution"));
        }
        let clear = semon_relay::inspect_snapshot_manifest(&encrypted, &identity.age)
            .map_err(remote_error)?;
        let Some(session) = field(&clear, "session") else {
            index.sidecars.remove(&root);
            continue;
        };
        if field(&clear, "root") != Some(root.as_str()) {
            return Err(invalid("snapshot root substitution"));
        }
        let mut row = RemoteSidecars {
            manifest: id.into(),
            session: session.into(),
            conflicted: heads.len() > 1,
            metadata: BTreeMap::new(),
        };
        for entry in clear["entries"]
            .as_array()
            .ok_or_else(|| invalid("snapshot entries missing"))?
        {
            let Some(path) = field(entry, "path") else {
                continue;
            };
            let name = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let agent = name
                .strip_prefix("agent-")
                .and_then(|name| name.strip_suffix(".meta.json"));
            let runtime = name.strip_suffix(".json").is_some_and(|pid| {
                !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit())
            });
            if agent.is_none() && name != "custom-title.json" && !runtime {
                continue;
            }
            let Some(bytes) =
                semon_relay::read_snapshot_file(&root, id, path, machine, &identity.age, transport)
                    .map_err(remote_error)?
            else {
                continue;
            };
            if bytes.len() as u64 > RECORD_MAX {
                return Err(invalid("snapshot metadata file is too large"));
            }
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("invalid sidecar metadata JSON"))?;
            if runtime && field(&value, "sessionId") != Some(session) {
                continue;
            }
            let node = agent.map_or_else(
                || format!("claude:{session}"),
                |id| format!("claude:{session}/agent:{id}"),
            );
            let previous = row.metadata.entry(node).or_default();
            let parsed = Sidecar {
                agent_type: field(&value, "agentType").map(str::to_owned),
                claude_link: field(&value, "bridgeSessionId")
                    .filter(|id| {
                        !id.is_empty()
                            && id
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    })
                    .map(|id| format!("https://claude.ai/code/{id}")),
                label: field(&value, "description")
                    .or_else(|| field(&value, "agentType"))
                    .or_else(|| field(&value, "customTitle"))
                    .or_else(|| field(&value, "title"))
                    .or_else(|| field(&value, "name"))
                    .map(str::to_owned),
                cwd: field(&value, "worktreePath").map(str::to_owned),
                branch: field(&value, "worktreeBranch").map(str::to_owned),
                tool: field(&value, "toolUseId").map(str::to_owned),
            };
            previous.label = parsed.label.or_else(|| previous.label.take());
            previous.agent_type = parsed.agent_type.or_else(|| previous.agent_type.take());
            previous.claude_link = parsed.claude_link.or_else(|| previous.claude_link.take());
            previous.cwd = parsed.cwd.or_else(|| previous.cwd.take());
            previous.branch = parsed.branch.or_else(|| previous.branch.take());
            previous.tool = parsed.tool.or_else(|| previous.tool.take());
        }
        index.sidecars.insert(root, row);
    }
    index.sidecars.retain(|root, _| current.contains(root));
    Ok(())
}

fn assemble(
    options: &Options,
    index: &RemoteIndex,
    status: &semon_relay::LeaseStatus,
    observed_ms: u64,
) -> Vec<Node> {
    let now = unix_now();
    let mut flat = BTreeMap::new();
    let mut parents = BTreeMap::new();
    let mut closed_calls = BTreeMap::new();
    for entry in index.streams.values() {
        let codex = entry.stream.starts_with("codex/");
        let agent = entry
            .stream
            .strip_prefix("subagents/agent-")
            .and_then(|name| name.strip_suffix(".jsonl"));
        let id = agent.unwrap_or(&entry.session).to_owned();
        let harness = if codex { "codex" } else { "claude" };
        let key = if agent.is_some() {
            format!("claude:{}/agent:{id}", entry.session)
        } else {
            format!("{harness}:{id}")
        };
        let mut node = Node::new(
            id,
            harness,
            if agent.is_some() || entry.parent_source.as_deref() == Some("native") {
                "subagent"
            } else {
                "session"
            },
        );
        apply_summary(&mut node, &entry.summary, now);
        node.label = entry.label.clone();
        node.parent_source = entry.parent_source.clone();
        let lease = status.rows.iter().find(|row| row.session == entry.session);
        node.machine = Some(lease.map_or_else(
            || entry.machine.clone(),
            |lease| lease.holder_machine.clone(),
        ));
        node.lease = lease.map(|lease| RemoteLease {
            holder_machine: lease.holder_machine.clone(),
            epoch: lease.epoch,
            last_renewal_ms: lease
                .lease_expires_at_ms
                .saturating_sub(semon_relay::LEASE_DURATION_MS),
            expires_at_ms: lease.lease_expires_at_ms,
            active: lease.lease_expires_at_ms > observed_ms,
        });
        node.state = match node.lease.as_ref() {
            Some(lease) if lease.active => "running",
            Some(_) => "ended",
            None => "unknown",
        }
        .into();
        if entry.unavailable {
            node.state = "unknown".into();
        }
        let parent = agent
            .map(|_| format!("claude:{}", entry.session))
            .or_else(|| entry.parent.clone());
        if let Some(parent) = parent {
            parents.insert(key.clone(), parent);
        }
        node.via_tool = entry.parent_tool.as_ref().map(|id| ToolCall {
            id: id.clone(),
            name: "Task".into(),
        });
        node.unlinked = codex && !parents.contains_key(&key);
        closed_calls.insert(key.clone(), entry.summary.closed_tools.clone());
        flat.insert(key, node);
    }
    for row in index.sidecars.values() {
        for (key, meta) in &row.metadata {
            if let Some(node) = flat.get_mut(key) {
                if row.conflicted {
                    node.label = None;
                    node.state = "unknown".into();
                    continue;
                }
                node.agent_type = meta.agent_type.clone();
                node.claude_link = meta.claude_link.clone().or_else(|| node.claude_link.take());
                node.label = meta.label.clone().or_else(|| node.label.take());
                node.cwd = meta.cwd.clone().or_else(|| node.cwd.take());
                node.branch = meta.branch.clone().or_else(|| node.branch.take());
                if let Some(id) = &meta.tool {
                    node.via_tool = Some(ToolCall {
                        id: id.clone(),
                        name: "Task".into(),
                    });
                }
            }
        }
    }
    for row in index.sidecars.values().filter(|row| !row.conflicted) {
        for (child, meta) in &row.metadata {
            let Some(tool) = &meta.tool else {
                continue;
            };
            let matches = index
                .streams
                .values()
                .filter(|entry| {
                    entry.session == row.session
                        && !entry.stream.starts_with("codex/")
                        && entry.summary.tools.contains_key(tool)
                })
                .map(|entry| {
                    match entry
                        .stream
                        .strip_prefix("subagents/agent-")
                        .and_then(|name| name.strip_suffix(".jsonl"))
                    {
                        Some(id) => format!("claude:{}/agent:{id}", entry.session),
                        None => format!("claude:{}", entry.session),
                    }
                })
                .collect::<BTreeSet<_>>();
            if matches.len() == 1 {
                parents.insert(
                    child.clone(),
                    matches.into_iter().next().expect("one parent"),
                );
            } else {
                parents.remove(child);
                if let Some(node) = flat.get_mut(child) {
                    node.unlinked = true;
                    node.state = "unknown".into();
                }
            }
        }
    }
    // Cyclic handoff markers must not make whole sessions disappear.
    for key in flat.keys().cloned().collect::<Vec<_>>() {
        let mut seen = BTreeSet::new();
        let mut cursor = key.as_str();
        while let Some(parent) = parents.get(cursor) {
            if !seen.insert(cursor.to_owned()) {
                parents.remove(&key);
                if let Some(node) = flat.get_mut(&key) {
                    node.unlinked = true;
                }
                break;
            }
            cursor = parent;
        }
    }
    let conflicted: BTreeSet<_> = index
        .sidecars
        .values()
        .filter(|row| row.conflicted)
        .flat_map(|row| row.metadata.keys().cloned())
        .collect();
    for (child, parent) in &parents {
        if conflicted.contains(child) {
            continue;
        }
        let Some(parent_node) = flat.get(parent) else {
            if let Some(child) = flat.get_mut(child) {
                child.unlinked = true;
            }
            continue;
        };
        let active = parent_node.lease.as_ref().is_some_and(|lease| lease.active);
        let calls = parent_node.open_tools.clone();
        if let Some(child) = flat.get_mut(child) {
            if let Some(tool) = child.via_tool.as_mut()
                && let Some(call) = calls.iter().find(|call| call.id == tool.id)
            {
                tool.name = call.name.clone();
            }
            if child.harness == "claude" && child.kind == "subagent" {
                child.state = if child
                    .via_tool
                    .as_ref()
                    .is_some_and(|tool| calls.iter().any(|call| call.id == tool.id))
                    && active
                {
                    "running"
                } else if child.via_tool.as_ref().is_some_and(|tool| {
                    closed_calls
                        .get(parent)
                        .is_some_and(|closed| closed.contains(&tool.id))
                }) {
                    "done"
                } else {
                    "unknown"
                }
                .into();
            }
        }
    }
    let machines: BTreeSet<_> = index
        .streams
        .values()
        .map(|entry| entry.machine.as_str())
        .collect();
    for machine in machines {
        let summaries: BTreeMap<_, _> = index
            .streams
            .values()
            .filter(|entry| entry.machine == machine && !entry.stream.starts_with("codex/"))
            .map(|entry| {
                let key = entry
                    .stream
                    .strip_prefix("subagents/agent-")
                    .and_then(|name| name.strip_suffix(".jsonl"))
                    .map_or_else(
                        || format!("claude:{}", entry.session),
                        |id| format!("claude:{}/agent:{id}", entry.session),
                    );
                (key, entry.summary.clone())
            })
            .collect();
        apply_claude_usage(&mut flat, &summaries);
    }
    let keys: Vec<_> = if let Some(session) = &options.session {
        flat.keys()
            .filter(|key| {
                key.as_str() == session
                    || flat[*key].id == *session
                    || format!("{}:{}", flat[*key].harness, flat[*key].id) == *session
            })
            .cloned()
            .collect()
    } else {
        flat.keys()
            .filter(|key| {
                !parents
                    .get(*key)
                    .is_some_and(|parent| flat.contains_key(parent))
            })
            .cloned()
            .collect()
    };
    let mut machines = BTreeMap::<String, Node>::new();
    for key in keys {
        if let Some(root) = make_tree(&key, &mut flat, &parents, &mut BTreeSet::new()) {
            let recent = tree_visible(&root, options.since.as_secs());
            if options.session.is_none() && !options.all && !recent {
                continue;
            }
            let id = root.machine.clone().unwrap_or_else(|| "unknown".into());
            machines
                .entry(id.clone())
                .or_insert_with(|| Node::new(id, "machine", "machine"))
                .children
                .push(root);
        }
    }
    machines.into_values().collect()
}
fn tree_visible(node: &Node, since: u64) -> bool {
    node.lease.as_ref().is_some_and(|lease| lease.active)
        || node
            .last_activity_age_seconds
            .is_some_and(|age| age <= since)
        || node.children.iter().any(|child| tree_visible(child, since))
}

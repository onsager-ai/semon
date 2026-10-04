use semon_relay::{
    DataKey, Frame, FrameKey, FrameMode, FramePage, LeaseRow, LeaseStatus, MachineIdentity,
    StreamTip, TakeoverResult, Transport, TransportError, ZERO_CHAIN, chain_line, encrypt_envelope,
    encrypt_frame,
};
use semon_sessions::{Node, Options, collect_remote, render_json};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    identity: MachineIdentity,
    transport: Stub,
    options: Options,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-remote-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let config = root.join("keys");
        semon_relay::init(&config).unwrap();
        let identity = MachineIdentity::load(&config).unwrap();
        let key = semon_relay::generate_data_key();
        let envelope = encrypt_envelope(&key, &[identity.age.to_public()]).unwrap();
        let options = Options {
            cache: root.join("cache/index.json"),
            all: true,
            ..Default::default()
        };
        Self {
            root,
            identity,
            options,
            transport: Stub {
                key,
                envelope,
                frames: RefCell::default(),
                leases: RefCell::default(),
                calls: RefCell::default(),
                scope: Cell::new("receiver-a"),
                revision: Cell::new("initial"),
                now: Cell::new(1000),
                snapshots: RefCell::default(),
                snapshot_gets: Cell::new(0),
                forked: Cell::new(false),
            },
        }
    }
    fn add(&self, session: &str, stream: &str, record: Value, machine: &str) {
        let mut frames = self.transport.frames.borrow_mut();
        let existing = frames.entry((session.into(), stream.into())).or_default();
        let previous = existing
            .last()
            .map(|frame| {
                semon_relay::decrypt_frame(
                    &self.transport.key,
                    &frame.key,
                    &frame.machine,
                    frame.encrypted_payload().unwrap(),
                )
                .unwrap()
                .0
            })
            .unwrap_or(ZERO_CHAIN);
        let mut line = serde_json::to_vec(&record).unwrap();
        line.push(b'\n');
        let chain = chain_line(&previous, &line);
        let key = FrameKey {
            session: session.into(),
            stream: stream.into(),
            generation: 0,
            epoch: 0,
            seq: existing.len() as u64,
        };
        let payload = encrypt_frame(&self.transport.key, &key, machine, &chain, &line).unwrap();
        existing.push(Frame::encrypted(
            key,
            machine.into(),
            1,
            1,
            "boot".into(),
            payload,
        ));
        self.transport.leases.borrow_mut().insert(
            session.into(),
            LeaseRow {
                session: session.into(),
                epoch: 0,
                holder_machine: machine.into(),
                lease_expires_at_ms: 2000,
            },
        );
    }
    fn collect(&self) -> Vec<Node> {
        collect_remote(&self.options, &self.identity, &self.transport).unwrap()
    }
    fn cache(&self) -> String {
        fs::read_dir(self.root.join("cache"))
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect::<Vec<_>>()
            .join("")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Stub {
    key: DataKey,
    envelope: Vec<u8>,
    frames: RefCell<BTreeMap<(String, String), Vec<Frame>>>,
    leases: RefCell<BTreeMap<String, LeaseRow>>,
    calls: RefCell<Vec<Option<u64>>>,
    scope: Cell<&'static str>,
    revision: Cell<&'static str>,
    now: Cell<u64>,
    snapshots: RefCell<BTreeMap<String, semon_relay::SnapshotPacket>>,
    snapshot_gets: Cell<usize>,
    forked: Cell<bool>,
}
impl Transport for Stub {
    fn send(&self, _: &Frame) -> Result<u64, TransportError> {
        unreachable!()
    }
    fn deletion_scope(&self) -> String {
        self.scope.get().into()
    }
    fn lease_observation(&self, _: &str) -> Result<(LeaseStatus, u64, String), TransportError> {
        Ok((
            LeaseStatus {
                rows: self.leases.borrow().values().cloned().collect(),
                takeovers: vec![],
            },
            self.now.get(),
            self.revision.get().into(),
        ))
    }
    fn list_envelope_sessions(&self, _: &str) -> Result<Vec<String>, TransportError> {
        Ok(self.leases.borrow().keys().cloned().collect())
    }
    fn get_envelope(&self, _: &str, _: &str) -> Result<Option<Vec<u8>>, TransportError> {
        Ok(Some(self.envelope.clone()))
    }
    fn lease_tips(&self, session: &str, _: &str) -> Result<TakeoverResult, TransportError> {
        let tips = self
            .frames
            .borrow()
            .iter()
            .filter(|((id, _), _)| id == session)
            .map(|((_, stream), frames)| {
                let frame = frames.last().unwrap();
                StreamTip {
                    stream: stream.clone(),
                    generation: 0,
                    epoch: 0,
                    seq: frame.key.seq,
                    mode: FrameMode::Encrypted,
                    chain: None,
                    tag: Some(frame.encrypted_payload().unwrap().tag),
                }
            })
            .collect();
        Ok(TakeoverResult {
            row: self.leases.borrow()[session].clone(),
            tips,
        })
    }
    fn list_frame_page(
        &self,
        session: &str,
        _: &str,
        stream: &str,
        _: u64,
        after: Option<u64>,
        _: usize,
        _: usize,
    ) -> Result<FramePage, TransportError> {
        self.calls.borrow_mut().push(after);
        let frames = self.frames.borrow();
        let mut page = frames[&(session.into(), stream.into())]
            .iter()
            .filter(|frame| after.is_none_or(|seq| frame.key.seq > seq))
            .take(2)
            .cloned()
            .collect::<Vec<_>>();
        let next = page
            .last()
            .filter(|frame| {
                frames[&(session.into(), stream.into())]
                    .last()
                    .unwrap()
                    .key
                    .seq
                    > frame.key.seq
            })
            .map(|frame| frame.key.seq);
        Ok(FramePage {
            frames: std::mem::take(&mut page),
            next,
        })
    }
    fn snapshot_request(&self, route: &str, request: &Value) -> Result<Value, TransportError> {
        let packets = self.snapshots.borrow();
        match route {
            "/v1/snapshots/roots" => {
                Ok(json!({"roots":packets.keys().collect::<Vec<_>>(),"next_after":null}))
            }
            "/v1/snapshots/list" => {
                assert_eq!(request["heads_only"], true);
                let packet = &packets[request["root"].as_str().unwrap()];
                let mut metadata = packet.manifest.clone();
                metadata.as_object_mut().unwrap().remove("ciphertext");
                metadata.as_object_mut().unwrap().remove("blobs");
                let mut heads = vec![packet.manifest["id"].clone()];
                if self.forked.get() {
                    heads.push(json!("another-head"));
                }
                Ok(json!({"heads":heads,"manifests":[metadata],"next_after":null}))
            }
            "/v1/snapshots/get" => {
                self.snapshot_gets.set(self.snapshot_gets.get() + 1);
                let packet = &packets[request["root"].as_str().unwrap()];
                if request["kind"] == "manifest" {
                    Ok(packet.manifest.clone())
                } else {
                    Ok(json!({"ciphertext":packet.blobs[request["id"].as_str().unwrap()]}))
                }
            }
            _ => panic!("unexpected route {route}"),
        }
    }
}
#[test]
fn native_codex_fork_relay_uses_request_counts_and_rebuilds_old_summaries() {
    let fixture = Fixture::new();
    let source = include_str!(
        "../../../tests/fixtures/compatibility/codex-0.159.0-alpha.3/fork/child-turn-rollout.jsonl"
    );
    let records: Vec<Value> = source
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for record in &records[..2] {
        fixture.add("child", "codex/rollout.jsonl", record.clone(), "machine-a");
    }
    assert_eq!(fixture.collect()[0].children[0].tokens.total, 0);
    for record in &records[2..] {
        fixture.add("child", "codex/rollout.jsonl", record.clone(), "machine-a");
    }
    let tree = fixture.collect();
    let child = &tree[0].children[0];
    assert_eq!(child.tokens.input, 5);
    assert_eq!(child.tokens.output, 3);
    assert_eq!(child.tokens.total, 8);
    let duplicate = records
        .iter()
        .find(|record| record["type"] == "token_usage_record")
        .unwrap();
    fixture.add(
        "child",
        "codex/rollout.jsonl",
        duplicate.clone(),
        "machine-a",
    );
    assert_eq!(fixture.collect()[0].children[0].tokens, child.tokens);
    let cache_path = fs::read_dir(fixture.root.join("cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut old: Value = serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
    old["version"] = json!(1);
    for entry in old["streams"].as_object_mut().unwrap().values_mut() {
        entry["summary"]
            .as_object_mut()
            .unwrap()
            .remove("codex_native_usage");
    }
    fs::write(&cache_path, serde_json::to_vec(&old).unwrap()).unwrap();
    let before = fixture.transport.calls.borrow().len();
    assert_eq!(fixture.collect()[0].children[0].tokens, child.tokens);
    assert_eq!(fixture.transport.calls.borrow()[before], None);
    assert!(!fixture.cache().contains("SEMON_SYNTHETIC_FORK_CHILD"));
    assert_eq!(
        serde_json::from_str::<Value>(&fixture.cache()).unwrap()["version"],
        3
    );
}

#[test]
fn metadata_tree_links_machines_incrementally_without_retaining_content() {
    let fixture = Fixture::new();
    fixture.add("parent","main",json!({"type":"assistant","timestamp":"2026-10-01T00:00:00Z","cwd":"/repo","message":{"id":"m","model":"claude-sonnet","usage":{"input_tokens":3,"output_tokens":2},"content":[{"type":"tool_use","id":"task","name":"Task","input":{"prompt":"SECRET-INPUT"}},{"type":"text","text":"SECRET-OUTPUT"}]}}),"machine-a");
    fixture.add("child","codex/2026/10/01/rollout.jsonl",json!({"type":"session_meta","payload":{"id":"child","cwd":"/repo","git":{"branch":"main"}}}),"machine-b");
    fixture.add("child","codex/2026/10/01/rollout.jsonl",json!({"type":"event_msg","payload":{"type":"user_message","message":"Semon-Parent: claude:parent:task\nSemon-Handoff: /SECRET-PROMPT-PATH\nSECRET-BODY"}}),"machine-b");
    let tree = fixture.collect();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].id, "machine-a");
    let parent = &tree[0].children[0];
    assert_eq!(parent.tokens.total, 5);
    assert_eq!(parent.state, "running");
    let child = &parent.children[0];
    assert_eq!(child.id, "child");
    assert_eq!(child.machine.as_deref(), Some("machine-b"));
    assert_eq!(child.via_tool.as_ref().unwrap().name, "Task");
    let initial = fixture.transport.calls.borrow().len();
    fixture.collect();
    assert_eq!(fixture.transport.calls.borrow().len(), initial);
    fixture.add(
        "child",
        "codex/2026/10/01/rollout.jsonl",
        json!({"type":"turn_context","payload":{"model":"gpt-6"}}),
        "machine-b",
    );
    let tree = fixture.collect();
    assert_eq!(tree[0].children[0].children[0].models, vec!["gpt-6"]);
    assert_eq!(fixture.transport.calls.borrow().last(), Some(&Some(1)));
    let cache = fixture.cache();
    let output = render_json(&tree);
    for secret in [
        "SECRET-INPUT",
        "SECRET-OUTPUT",
        "SECRET-PROMPT-PATH",
        "SECRET-BODY",
        "ciphertext",
        "signing.key",
    ] {
        assert!(!cache.contains(secret));
        assert!(!output.contains(secret));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in fs::read_dir(fixture.root.join("cache")).unwrap() {
            assert_eq!(
                fs::metadata(entry.unwrap().path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    fixture.transport.now.set(3000);
    assert_eq!(fixture.collect()[0].children[0].state, "ended");
    let prior = fixture.transport.calls.borrow().len();
    fixture.transport.scope.set("receiver-b");
    fixture.collect();
    assert!(fixture.transport.calls.borrow().len() > prior);
    assert_eq!(fixture.transport.calls.borrow()[prior], None);
}
#[test]
fn ciphertext_tampering_is_rejected_and_never_updates_the_cache() {
    let fixture = Fixture::new();
    fixture.add(
        "s",
        "main",
        json!({"type":"user","message":{"content":"SECRET-BODY"}}),
        "m",
    );
    fixture
        .transport
        .frames
        .borrow_mut()
        .get_mut(&("s".into(), "main".into()))
        .unwrap()[0]
        .key
        .session = "substitution".into();
    assert!(collect_remote(&fixture.options, &fixture.identity, &fixture.transport).is_err());
    assert!(!fixture.root.join("cache").exists());
}
#[test]
fn deletion_revision_rebuilds_summaries_and_gaps_are_unknown() {
    let fixture = Fixture::new();
    fixture.add(
        "s",
        "main",
        json!({"type":"assistant","message":{"model":"old-model"}}),
        "m",
    );
    fixture.add(
        "s",
        "main",
        json!({"type":"assistant","message":{"model":"new-model"}}),
        "m",
    );
    assert_eq!(fixture.collect()[0].children[0].models.len(), 2);
    fixture
        .transport
        .frames
        .borrow_mut()
        .get_mut(&("s".into(), "main".into()))
        .unwrap()
        .remove(0);
    fixture.transport.revision.set("deleted");
    let tree = fixture.collect();
    assert_eq!(tree[0].children[0].state, "unknown");
    assert!(tree[0].children[0].models.is_empty());
    assert!(!fixture.cache().contains("old-model"));
}

#[test]
fn encrypted_sidecars_supply_only_allowed_labels_and_exact_task_links() {
    let fixture = Fixture::new();
    fixture.add("parent","main",json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"task","name":"Agent","input":{"secret":"SECRET-INPUT"}}]}}),"machine-a");
    fixture.add("parent","subagents/agent-child.jsonl",json!({"type":"assistant","message":{"model":"claude-haiku","content":[{"type":"text","text":"SECRET-SIDECAR-TRANSCRIPT"}]}}),"machine-a");
    let source = fixture.root.join("sidecars");
    fs::create_dir_all(source.join("subagents")).unwrap();
    fs::write(source.join("subagents/agent-child.meta.json"),serde_json::to_vec(&json!({"toolUseId":"task","description":"Review tests","agentType":"Explore","worktreeBranch":"review","unexpected":"SECRET-SIDECAR-UNALLOWED"})).unwrap()).unwrap();
    fs::write(
        source.join("custom-title.json"),
        br#"{"customTitle":"Release checklist"}"#,
    )
    .unwrap();
    let root = semon_relay::snapshot_root_id("sidecars");
    let packet = semon_relay::SnapshotCache::default()
        .capture(
            &source,
            &root,
            "machine-a",
            None,
            1,
            1,
            "boot",
            Some(0),
            Some("parent"),
            &[fixture.identity.age.to_public()],
        )
        .unwrap();
    fixture
        .transport
        .snapshots
        .borrow_mut()
        .insert(root, packet);
    let tree = fixture.collect();
    let parent = &tree[0].children[0];
    let child = &parent.children[0];
    assert_eq!(parent.label.as_deref(), Some("Release checklist"));
    assert_eq!(child.label.as_deref(), Some("Review tests"));
    assert_eq!(child.agent_type.as_deref(), Some("Explore"));
    assert_eq!(child.state, "running");
    assert_eq!(child.via_tool.as_ref().unwrap().name, "Agent");
    let before = fixture.transport.snapshot_gets.get();
    fixture.collect();
    assert_eq!(fixture.transport.snapshot_gets.get(), before);
    assert!(!fixture.cache().contains("SECRET-SIDECAR-UNALLOWED"));
    assert!(!render_json(&tree).contains("SECRET-SIDECAR-TRANSCRIPT"));
    fixture.add("parent","main",json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"task","content":"SECRET-RESULT"}]}}),"machine-a");
    assert_eq!(fixture.collect()[0].children[0].children[0].state, "done");
    fixture.transport.forked.set(true);
    let conflicted = fixture.collect();
    assert_eq!(conflicted[0].children[0].children[0].state, "unknown");
}

#[test]
fn native_claude_copied_usage_remote_keeps_owner_unknown_after_restart() {
    let fixture = Fixture::new();
    for (session, text) in [
        (
            "native-claude-parent",
            include_str!(
                "../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/resumed-transcript.jsonl"
            ),
        ),
        (
            "native-claude-child",
            include_str!(
                "../../../tests/fixtures/compatibility/claude-2.1.288/lifecycle/forked-transcript.jsonl"
            ),
        ),
    ] {
        for line in text.lines() {
            fixture.add(
                session,
                "main",
                serde_json::from_str(line).unwrap(),
                "machine-a",
            );
        }
    }
    for _ in 0..2 {
        let nodes = fixture.collect();
        let sessions = &nodes[0].children;
        let p = sessions
            .iter()
            .find(|n| n.id == "native-claude-parent")
            .unwrap();
        let c = sessions
            .iter()
            .find(|n| n.id == "native-claude-child")
            .unwrap();
        assert_eq!((p.tokens.input, c.tokens.input), (0, 5));
        assert_eq!(
            p.claude_usage.as_ref().unwrap().shared,
            c.claude_usage.as_ref().unwrap().shared
        );
        assert!(c.claude_usage.as_ref().unwrap().fresh.is_none());
        assert!(c.claude_usage.as_ref().unwrap().shared_owner.is_none());
    }
}

#[test]
fn claude_usage_records_do_not_establish_cross_machine_copy_ownership() {
    let fixture = Fixture::new();
    let record = json!({"type":"assistant","uuid":"same-uuid","message":{"id":"same-api","model":"model","usage":{"input_tokens":5,"output_tokens":3},"content":[]}});
    fixture.add("a", "main", record.clone(), "machine-a");
    fixture.add("b", "main", record, "machine-b");
    for machine in fixture.collect() {
        assert_eq!(machine.children[0].tokens.input, 5);
        assert!(machine.children[0].claude_usage.is_none());
    }
}

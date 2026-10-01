use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use age::x25519;
use semon_relay::{
    EnvelopeOutcome, FRAME_PAGE_MAX_BYTES, FRAME_PAGE_MAX_FRAMES, Frame, FrameContent, FrameKey,
    FrameMode, FramePage, LeaseRow, LeaseStatus, OrphanSummary, ReceiveError, Receiver, RelayState,
    RestoreError, Sender, StreamTip, TakeoverResult, Transport, TransportError, ZERO_CHAIN,
    chain_line, load_state, restore_session, restore_session_encrypted,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "semon-relay-restore-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ReceiverTransport<'a>(&'a Receiver);

impl Transport for ReceiverTransport<'_> {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.0.accept(frame).map_err(transport_error)?;
        Ok(frame.key.seq)
    }

    fn send_orphan(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.0.accept_orphan(frame).map_err(transport_error)?;
        Ok(frame.key.seq)
    }

    fn lease_status(
        &self,
        session: Option<&str>,
        _machine: &str,
    ) -> Result<LeaseStatus, TransportError> {
        self.0.lease_status(session).map_err(transport_error)
    }

    fn acquire(&self, session: &str, machine: &str) -> Result<LeaseRow, TransportError> {
        self.0.acquire(session, machine).map_err(transport_error)
    }

    fn renew(&self, session: &str, machine: &str, epoch: u64) -> Result<LeaseRow, TransportError> {
        self.0
            .renew(session, machine, epoch)
            .map_err(transport_error)
    }

    fn takeover(
        &self,
        session: &str,
        machine: &str,
        expected_epoch: u64,
        force: bool,
    ) -> Result<TakeoverResult, TransportError> {
        self.0
            .takeover(session, machine, expected_epoch, force)
            .map_err(transport_error)
    }

    fn lease_tips(&self, session: &str, _machine: &str) -> Result<TakeoverResult, TransportError> {
        self.0.lease_tips(session).map_err(transport_error)
    }

    fn list_orphans(&self, _machine: &str) -> Result<Vec<OrphanSummary>, TransportError> {
        self.0.list_orphans().map_err(transport_error)
    }

    fn put_envelope(
        &self,
        session: &str,
        machine: &str,
        envelope: &[u8],
        replace: bool,
        force: bool,
    ) -> Result<(), TransportError> {
        let _: EnvelopeOutcome = self
            .0
            .put_envelope(session, machine, envelope, replace, force)
            .map_err(transport_error)?;
        Ok(())
    }

    fn get_envelope(
        &self,
        session: &str,
        _machine: &str,
    ) -> Result<Option<Vec<u8>>, TransportError> {
        self.0.get_envelope(session).map_err(transport_error)
    }

    fn list_frames(&self, session: &str, _machine: &str) -> Result<Vec<Frame>, TransportError> {
        self.0.list_frames(session).map_err(transport_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn list_frame_page(
        &self,
        session: &str,
        _machine: &str,
        stream: &str,
        generation: u64,
        after_seq: Option<u64>,
        limit_bytes: usize,
        limit_frames: usize,
    ) -> Result<FramePage, TransportError> {
        self.0
            .list_frame_page(
                session,
                stream,
                generation,
                after_seq,
                limit_bytes,
                limit_frames,
            )
            .map_err(transport_error)
    }
}

fn transport_error(error: ReceiveError) -> TransportError {
    match error {
        ReceiveError::Fenced { current_epoch, .. } => TransportError::Fenced { current_epoch },
        ReceiveError::NotHolder { current_epoch, .. } => {
            TransportError::NotHolder { current_epoch }
        }
        error => TransportError::Unavailable(error.to_string()),
    }
}

fn source_session(temp: &TempDir, main: &[u8], subagent: &[u8]) -> (PathBuf, PathBuf) {
    let projects = temp.path().join("source-projects");
    let project = projects.join("old-slug");
    fs::create_dir_all(project.join("session-a/subagents")).unwrap();
    fs::write(project.join("session-a.jsonl"), main).unwrap();
    fs::write(
        project.join("session-a/subagents/agent-worker.jsonl"),
        subagent,
    )
    .unwrap();
    (projects, temp.path().join("source-state/relay.json"))
}

fn target_paths(projects: &Path, cwd: &Path) -> (PathBuf, PathBuf) {
    let slug = cwd.to_str().unwrap().replace('/', "-");
    let project = projects.join(slug);
    (
        project.join("session-a.jsonl"),
        project.join("session-a/subagents/agent-worker.jsonl"),
    )
}

const MAIN_TRANSCRIPT: &[u8] = br#"{"type":"assistant","cwd":"/old/worktree","gitBranch":"feature/synthetic","message":{"content":[{"type":"tool_use","id":"tool-pending","name":"Read","input":{"secret":"NEVER-REPORT-THIS-INPUT"}},{"type":"tool_use","id":"tool-done","name":"Bash","input":{"command":"NEVER-REPORT-THIS-COMMAND"}}]},"text":"NEVER-REPORT-THIS-TEXT"}
{"type":"user","cwd":"/old/worktree","gitBranch":"feature/synthetic","message":{"content":[{"type":"tool_result","tool_use_id":"tool-done","content":"NEVER-REPORT-THIS-RESULT"}]}}
"#;
const SUBAGENT_TRANSCRIPT: &[u8] =
    br#"{"type":"assistant","message":{"content":[{"type":"text","text":"SIDECHAIN-SECRET"}]}}
"#;

#[test]
fn plaintext_round_trip_restores_main_and_subagent_and_redacts_report() {
    let temp = TempDir::new("plaintext-round-trip");
    let (source_projects, source_state) =
        source_session(&temp, MAIN_TRANSCRIPT, SUBAGENT_TRANSCRIPT);
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();

    let projects = temp.path().join("restore-projects");
    let state = temp.path().join("restore-state/relay.json");
    let cwd = temp.path().join("different-target-worktree");
    fs::create_dir_all(&cwd).unwrap();
    let report = restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();

    let (main, subagent) = target_paths(&projects, &cwd);
    assert_eq!(fs::read(main).unwrap(), MAIN_TRANSCRIPT);
    assert_eq!(fs::read(subagent).unwrap(), SUBAGENT_TRANSCRIPT);
    assert_eq!(report.new_epoch, Some(1));
    assert_eq!(report.lines, 3);
    assert_eq!(report.unfinished_tool_calls.len(), 1);
    assert_eq!(report.unfinished_tool_calls[0].name, "Read");
    assert_eq!(report.unfinished_tool_calls[0].id, "tool-pending");
    assert_eq!(report.recorded_cwd.as_deref(), Some("/old/worktree"));
    assert_eq!(
        report.recorded_git_branch.as_deref(),
        Some("feature/synthetic")
    );
    let text = report.to_text();
    let json = report.to_json().to_string();
    for secret in [
        "NEVER-REPORT-THIS-INPUT",
        "NEVER-REPORT-THIS-COMMAND",
        "NEVER-REPORT-THIS-TEXT",
        "NEVER-REPORT-THIS-RESULT",
        "SIDECHAIN-SECRET",
    ] {
        assert!(!text.contains(secret));
        assert!(!json.contains(secret));
    }
    assert!(text.contains(&format!(
        "cd {} && claude --resume session-a",
        cwd.display()
    )));
    assert_takeover_state(&state, 1, 2);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(target_paths(&projects, &cwd).0)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(target_paths(&projects, &cwd).1.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn synthetic_stream_spanning_many_pages_restores_byte_identically() {
    let temp = TempDir::new("many-pages");
    let mut main = Vec::new();
    for index in 0..200 {
        let prefix = format!("{index:03}:");
        main.extend_from_slice(prefix.as_bytes());
        main.extend(std::iter::repeat_n(b'x', 4095 - prefix.len()));
        main.push(b'\n');
    }
    let (source_projects, source_state) = source_session(&temp, &main, b"side\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();

    let mut pages = 0;
    let mut cursor = None;
    loop {
        let page = receiver
            .list_frame_page(
                "session-a",
                "main",
                0,
                cursor,
                FRAME_PAGE_MAX_BYTES,
                FRAME_PAGE_MAX_FRAMES,
            )
            .unwrap();
        pages += 1;
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert!(pages >= 4, "synthetic stream used only {pages} pages");

    let projects = temp.path().join("restore-projects");
    let state = temp.path().join("restore-state/relay.json");
    let cwd = temp.path().join("target");
    fs::create_dir_all(&cwd).unwrap();
    restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();
    assert_eq!(fs::read(target_paths(&projects, &cwd).0).unwrap(), main);
}

#[test]
fn encrypted_round_trip_restores_subagent_and_outsider_cannot_unwrap() {
    let temp = TempDir::new("encrypted-round-trip");
    let (source_projects, source_state) =
        source_session(&temp, MAIN_TRANSCRIPT, SUBAGENT_TRANSCRIPT);
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    let old_identity = x25519::Identity::generate();
    let new_identity = x25519::Identity::generate();
    let recipients = vec![old_identity.to_public(), new_identity.to_public()];
    Sender::encrypted("machine-a", old_identity, recipients)
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();

    let projects = temp.path().join("restore-projects");
    let state = temp.path().join("restore-state/relay.json");
    let cwd = temp.path().join("encrypted-target-worktree");
    fs::create_dir_all(&cwd).unwrap();
    let report = restore_session_encrypted(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &new_identity,
        &transport,
    )
    .unwrap();
    let (main, subagent) = target_paths(&projects, &cwd);
    assert_eq!(fs::read(main).unwrap(), MAIN_TRANSCRIPT);
    assert_eq!(fs::read(subagent).unwrap(), SUBAGENT_TRANSCRIPT);
    assert_eq!(report.new_epoch, Some(1));

    let outsider = x25519::Identity::generate();
    let outsider_cwd = temp.path().join("outsider-worktree");
    fs::create_dir_all(&outsider_cwd).unwrap();
    let error = restore_session_encrypted(
        &temp.path().join("outsider-projects"),
        &temp.path().join("outsider-state/relay.json"),
        &outsider_cwd,
        "session-a",
        "machine-c",
        true,
        false,
        &outsider,
        &transport,
    )
    .unwrap_err();
    assert!(matches!(error, RestoreError::Envelope(_)));
}

#[test]
fn existing_targets_must_be_prefixes_and_other_slugs_are_refused() {
    let temp = TempDir::new("prefix-rules");
    let main = b"one\ntwo\n";
    let (source_projects, source_state) = source_session(&temp, main, b"side\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();
    let projects = temp.path().join("restore-projects");
    let state = temp.path().join("restore-state/relay.json");
    let cwd = temp.path().join("target-worktree");
    fs::create_dir_all(&cwd).unwrap();

    restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();
    let main_path = target_paths(&projects, &cwd).0;
    let identical = restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();
    assert_eq!(identical.lines_written, 0);

    fs::write(&main_path, b"one\n").unwrap();
    let extended = restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();
    assert_eq!(fs::read(&main_path).unwrap(), main);
    assert_eq!(extended.lines_written, 1);

    fs::write(&main_path, b"different\n").unwrap();
    let before_epoch = receiver.lease_status(Some("session-a")).unwrap().rows[0].epoch;
    assert!(matches!(
        restore_session(
            &projects,
            &state,
            &cwd,
            "session-a",
            "machine-b",
            true,
            false,
            &transport,
        ),
        Err(RestoreError::Prefix { .. })
    ));
    assert_eq!(
        receiver.lease_status(Some("session-a")).unwrap().rows[0].epoch,
        before_epoch
    );

    fs::write(&main_path, main).unwrap();
    let other = projects.join("another-slug/session-a.jsonl");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, main).unwrap();
    assert!(matches!(
        restore_session(
            &projects,
            &state,
            &cwd,
            "session-a",
            "machine-b",
            true,
            false,
            &transport,
        ),
        Err(RestoreError::OtherSlug { .. })
    ));
}

#[test]
fn each_stream_restores_only_its_highest_generation() {
    let temp = TempDir::new("highest-generation");
    let (source_projects, source_state) = source_session(&temp, b"old\n", b"side\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();
    fs::write(
        source_projects.join("old-slug/session-a.jsonl"),
        b"replacement\n",
    )
    .unwrap();
    Sender::with_machine("machine-a")
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();

    let projects = temp.path().join("restore-projects");
    let state = temp.path().join("restore-state/relay.json");
    let cwd = temp.path().join("target");
    fs::create_dir_all(&cwd).unwrap();
    let report = restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();

    assert_eq!(
        fs::read(target_paths(&projects, &cwd).0).unwrap(),
        b"replacement\n"
    );
    assert_eq!(
        report
            .streams
            .iter()
            .find(|stream| stream.stream == "main")
            .unwrap()
            .generation,
        1
    );
}

#[test]
fn report_lists_only_this_sessions_orphans_by_fenced_epoch() {
    let temp = TempDir::new("orphan-report");
    let (source_projects, source_state) = source_session(&temp, b"zero\n", b"side\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    let mut sender = Sender::with_machine("machine-a");
    sender
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();
    receiver
        .takeover("session-a", "intermediate-machine", 0, true)
        .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(source_projects.join("old-slug/session-a.jsonl"))
        .unwrap()
        .write_all(b"late\n")
        .unwrap();
    sender
        .run_pass(&source_projects, &source_state, &transport)
        .unwrap();

    let projects = temp.path().join("restore-projects");
    let state = temp.path().join("restore-state/relay.json");
    let cwd = temp.path().join("target");
    fs::create_dir_all(&cwd).unwrap();
    let report = restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();

    assert_eq!(report.orphans.len(), 1);
    assert_eq!(report.orphans[0].fenced_epoch, 0);
    assert_eq!(report.orphans[0].frames, 1);
}

struct FrameListTransport {
    frames: Vec<Frame>,
}

impl Transport for FrameListTransport {
    fn send(&self, _frame: &Frame) -> Result<u64, TransportError> {
        unreachable!("an incomplete restore never sends")
    }

    fn list_frames(&self, _session: &str, _machine: &str) -> Result<Vec<Frame>, TransportError> {
        Ok(self.frames.clone())
    }

    fn lease_tips(&self, session: &str, _machine: &str) -> Result<TakeoverResult, TransportError> {
        let frame = self
            .frames
            .iter()
            .max_by_key(|frame| frame.key.seq)
            .expect("test transport has frames");
        let (mode, chain, tag) = match &frame.content {
            FrameContent::Plaintext { chain, .. } => (FrameMode::Plaintext, Some(*chain), None),
            FrameContent::Encrypted(payload) => (FrameMode::Encrypted, None, Some(payload.tag)),
        };
        Ok(TakeoverResult {
            row: LeaseRow {
                session: session.to_owned(),
                epoch: 0,
                holder_machine: "machine-a".into(),
                lease_expires_at_ms: 0,
            },
            tips: vec![StreamTip {
                stream: frame.key.stream.clone(),
                generation: frame.key.generation,
                epoch: frame.key.epoch,
                seq: frame.key.seq,
                mode,
                chain,
                tag,
            }],
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn list_frame_page(
        &self,
        _session: &str,
        _machine: &str,
        stream: &str,
        generation: u64,
        after_seq: Option<u64>,
        _limit_bytes: usize,
        _limit_frames: usize,
    ) -> Result<FramePage, TransportError> {
        let mut frames = self
            .frames
            .iter()
            .filter(|frame| {
                frame.key.stream == stream
                    && frame.key.generation == generation
                    && after_seq.is_none_or(|after| frame.key.seq > after)
            })
            .cloned()
            .collect::<Vec<_>>();
        frames.sort_by_key(|frame| frame.key.seq);
        let more = frames.len() > 1;
        frames.truncate(1);
        Ok(FramePage {
            next: more.then(|| frames[0].key.seq),
            frames,
        })
    }
}

#[test]
fn a_gap_on_a_page_boundary_is_refused_or_writes_only_the_verified_prefix() {
    let temp = TempDir::new("gaps");
    let first = plaintext_frame(0, 0, &ZERO_CHAIN, b"zero\n");
    let third = plaintext_frame(2, 0, &frame_chain(&first), b"two\n");
    let transport = FrameListTransport {
        frames: vec![first, third],
    };
    let projects = temp.path().join("projects");
    let state = temp.path().join("state/relay.json");
    let cwd = temp.path().join("target");
    fs::create_dir_all(&cwd).unwrap();

    assert!(matches!(
        restore_session(
            &projects,
            &state,
            &cwd,
            "session-a",
            "machine-b",
            true,
            false,
            &transport,
        ),
        Err(RestoreError::Gap {
            expected: 1,
            actual: 2,
            ..
        })
    ));
    assert!(!target_paths(&projects, &cwd).0.exists());

    let report = restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        true,
        &transport,
    )
    .unwrap();
    assert!(report.incomplete);
    assert_eq!(report.new_epoch, None);
    assert_eq!(report.streams[0].truncated_at_seq, Some(1));
    assert_eq!(
        fs::read(target_paths(&projects, &cwd).0).unwrap(),
        b"zero\n"
    );
    assert!(!state.exists());
}

#[test]
fn chain_break_is_refused_even_with_allow_gaps() {
    let temp = TempDir::new("chain-break");
    let mut broken = plaintext_frame(0, 0, &ZERO_CHAIN, b"zero\n");
    if let semon_relay::FrameContent::Plaintext { chain, .. } = &mut broken.content {
        *chain = [9; 32];
    }
    let transport = FrameListTransport {
        frames: vec![broken],
    };
    let cwd = temp.path().join("target");
    fs::create_dir_all(&cwd).unwrap();
    assert!(matches!(
        restore_session(
            &temp.path().join("projects"),
            &temp.path().join("state/relay.json"),
            &cwd,
            "session-a",
            "machine-b",
            true,
            true,
            &transport,
        ),
        Err(RestoreError::Chain { seq: 0, .. })
    ));
}

struct AppendDuringTakeover<'a> {
    receiver: &'a Receiver,
    frame: Mutex<Option<Frame>>,
}

impl Transport for AppendDuringTakeover<'_> {
    fn send(&self, _frame: &Frame) -> Result<u64, TransportError> {
        unreachable!("restore does not send frames")
    }

    fn lease_tips(&self, session: &str, _machine: &str) -> Result<TakeoverResult, TransportError> {
        self.receiver.lease_tips(session).map_err(transport_error)
    }

    fn takeover(
        &self,
        session: &str,
        machine: &str,
        expected_epoch: u64,
        force: bool,
    ) -> Result<TakeoverResult, TransportError> {
        if let Some(frame) = self.frame.lock().unwrap().take() {
            self.receiver.accept(&frame).map_err(transport_error)?;
        }
        self.receiver
            .takeover(session, machine, expected_epoch, force)
            .map_err(transport_error)
    }

    fn list_frames(&self, session: &str, _machine: &str) -> Result<Vec<Frame>, TransportError> {
        self.receiver.list_frames(session).map_err(transport_error)
    }
}

#[test]
fn append_between_fetch_and_cas_is_restored_after_fencing() {
    let temp = TempDir::new("takeover-race");
    let (source_projects, source_state) = source_session(&temp, b"zero\n", b"side\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let initial = ReceiverTransport(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&source_projects, &source_state, &initial)
        .unwrap();
    let first = receiver
        .read_frame("session-a", "main", 0, 0, 0)
        .unwrap()
        .unwrap();
    let raced = plaintext_frame(1, 0, &frame_chain(&first), b"raced\n");
    let transport = AppendDuringTakeover {
        receiver: &receiver,
        frame: Mutex::new(Some(raced)),
    };
    let projects = temp.path().join("restore-projects");
    let state = temp.path().join("restore-state/relay.json");
    let cwd = temp.path().join("target");
    fs::create_dir_all(&cwd).unwrap();

    let report = restore_session(
        &projects,
        &state,
        &cwd,
        "session-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();

    assert_eq!(
        fs::read(target_paths(&projects, &cwd).0).unwrap(),
        b"zero\nraced\n"
    );
    assert_eq!(report.new_epoch, Some(1));
    assert_takeover_state(&state, 1, 2);
}

fn plaintext_frame(seq: u64, epoch: u64, previous: &[u8; 32], line: &[u8]) -> Frame {
    Frame::plaintext(
        FrameKey {
            session: "session-a".into(),
            stream: "main".into(),
            generation: 0,
            epoch,
            seq,
        },
        "machine-a".into(),
        chain_line(previous, line),
        1,
        2,
        "synthetic-boot".into(),
        line.to_vec(),
    )
}

fn frame_chain(frame: &Frame) -> [u8; 32] {
    *frame.plaintext_content().unwrap().0
}

fn assert_takeover_state(path: &Path, epoch: u64, streams: usize) {
    let state: RelayState = load_state(path).unwrap();
    assert_eq!(state.streams.len(), streams);
    assert!(state.streams.values().all(|stream| stream.epoch == epoch));
}

#[test]
fn codex_rollout_discovery_rewrite_restore_and_writer_lock() {
    let temp = TempDir::new("codex-rollout");
    let source = temp.path().join("source/sessions");
    let relative = "2026/10/01/rollout-2026-10-01T10-00-00-thread-a.jsonl";
    let path = source.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let meta = b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-a\",\"cwd\":\"/synthetic/source\",\"git\":{\"branch\":\"synthetic\"}}}\n";
    let mut first = meta.to_vec();
    first.extend_from_slice(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"call_id\":\"call-a\",\"name\":\"exec_command\"}}\n");
    fs::write(&path, &first).unwrap();
    fs::write(source.join("state_5.sqlite"), b"not a transcript").unwrap();
    fs::write(path.parent().unwrap().join("rollout-invalid.jsonl"), meta).unwrap();
    let streams = semon_relay::discover_streams(&source).unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].stream, format!("codex/{relative}"));
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    let state = temp.path().join("source-state.json");
    let mut sender = Sender::with_machine("machine-a");
    let (claude_root, _) = source_session(&temp, b"synthetic main\n", b"synthetic child\n");
    sender.add_root(claude_root);
    sender.set_session_filter(["thread-a".to_owned()].into_iter().collect());
    assert!(
        !sender
            .run_pass(&source, &state, &transport)
            .unwrap()
            .had_failures()
    );
    // Same-path rewrite is a generation bump, not a second stream.
    let mut rewritten = meta.to_vec();
    rewritten.extend_from_slice(b"{\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"call_id\":\"pending\",\"name\":\"apply_patch\"}}\n");
    fs::write(&path, &rewritten).unwrap();
    assert!(
        !sender
            .run_pass(&source, &state, &transport)
            .unwrap()
            .had_failures()
    );
    let target = temp.path().join("target/sessions");
    let target_state = temp.path().join("target-state.json");
    let cwd = temp.path().join("target work");
    fs::create_dir_all(&cwd).unwrap();
    let lockdir = temp.path().join("target/thread-writer-locks");
    fs::create_dir_all(&lockdir).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lockdir.join("thread-a.lock"))
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
    }
    #[cfg(not(unix))]
    lock.lock().unwrap();
    let err = restore_session(
        &target,
        &target_state,
        &cwd,
        "thread-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap_err();
    assert!(err.to_string().contains("active writer"));
    assert!(!target.join(relative).exists());
    drop(lock);
    // A caller's live SQLite remains byte-identical throughout restore.
    let database = temp.path().join("target/state_5.sqlite");
    fs::write(&database, b"target-local-index").unwrap();
    let report = restore_session(
        &target,
        &target_state,
        &cwd,
        "thread-a",
        "machine-b",
        true,
        false,
        &transport,
    )
    .unwrap();
    assert_eq!(report.harness, "codex");
    assert_eq!(report.streams[0].generation, 1);
    assert_eq!(fs::read(target.join(relative)).unwrap(), rewritten);
    assert_eq!(fs::read(database).unwrap(), b"target-local-index");
    assert_eq!(report.recorded_cwd.as_deref(), Some("/synthetic/source"));
    assert_eq!(report.recorded_git_branch.as_deref(), Some("synthetic"));
    assert_eq!(report.unfinished_tool_calls.len(), 1);
    assert_eq!(report.unfinished_tool_calls[0].id, "pending");
    assert!(
        report
            .next_step()
            .unwrap()
            .contains("codex resume thread-a")
    );
    assert!(!report.to_text().contains("apply_patch arguments"));
    assert!(
        !Sender::with_machine("machine-b")
            .run_pass(&target, &target_state, &transport)
            .unwrap()
            .had_failures()
    );
}

#[test]
fn encrypted_codex_restore_rejects_symlink_and_preserves_rollout_bytes() {
    let temp = TempDir::new("codex-encrypted");
    let source = temp.path().join("source/sessions");
    let relative = "2024/02/29/rollout-2024-02-29T12-00-00-thread-b.jsonl";
    let path = source.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let content =
        b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-b\",\"cwd\":\"/synthetic\"}}\n";
    fs::write(&path, content).unwrap();
    let invalid = source.join("2023/02/29/rollout-invalid-thread-b.jsonl");
    fs::create_dir_all(invalid.parent().unwrap()).unwrap();
    fs::write(invalid, content).unwrap();
    assert_eq!(
        semon_relay::discover_codex_streams(&source).unwrap().len(),
        1
    );
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport(&receiver);
    let old = x25519::Identity::generate();
    let new = x25519::Identity::generate();
    let recipients = vec![old.to_public(), new.to_public()];
    Sender::encrypted("machine-a", old, recipients)
        .run_pass(&source, &temp.path().join("source-state.json"), &transport)
        .unwrap();
    let target = temp.path().join("target/sessions");
    fs::create_dir_all(&target).unwrap();
    let state = temp.path().join("target-state.json");
    let cwd = temp.path().join("work");
    fs::create_dir_all(&cwd).unwrap();
    #[cfg(unix)]
    {
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, target.join("2024")).unwrap();
        assert!(matches!(
            restore_session_encrypted(
                &target,
                &state,
                &cwd,
                "thread-b",
                "machine-b",
                true,
                false,
                &new,
                &transport
            ),
            Err(RestoreError::TargetType(_))
        ));
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(target.join("2024")).unwrap();
    }
    let report = restore_session_encrypted(
        &target,
        &state,
        &cwd,
        "thread-b",
        "machine-b",
        true,
        false,
        &new,
        &transport,
    )
    .unwrap();
    assert_eq!(fs::read(target.join(relative)).unwrap(), content);
    assert!(report.next_step().unwrap().contains("CODEX_HOME="));
    assert_eq!(report.codex_home, Some(temp.path().join("target")));
    assert!(
        report.to_json()["compatibility"]
            .as_str()
            .unwrap()
            .contains("0.159.0-alpha.3")
    );
}

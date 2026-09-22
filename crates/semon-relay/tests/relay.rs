use std::{
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use semon_relay::{
    Clock, Frame, FrameContent, FrameKey, LeaseRow, LeaseStatus, OrphanSummary, ReceiveError,
    ReceiveOutcome, Receiver, RelayError, Sender, TakeoverCommandError, TakeoverResult, Transport,
    TransportError, ZERO_CHAIN, chain_line, discover_streams, initialize_takeover_state,
    load_state, run_pass, takeover_session, validate_loopback,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "semon-relay-{name}-{}-{sequence}",
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

#[derive(Default)]
struct RecordingTransport {
    frames: Mutex<Vec<Frame>>,
}

impl Transport for RecordingTransport {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.frames.lock().unwrap().push(frame.clone());
        Ok(frame.key.seq)
    }
}

struct LocalTransport<'a>(&'a Receiver);

impl Transport for LocalTransport<'_> {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.0
            .accept(frame)
            .map_err(|error| TransportError::Unavailable(error.to_string()))?;
        Ok(frame.key.seq)
    }

    fn acquire(&self, session: &str, machine: &str) -> Result<LeaseRow, TransportError> {
        self.0
            .acquire(session, machine)
            .map_err(|error| TransportError::Unavailable(error.to_string()))
    }
}

fn source_file(temp: &TempDir, bytes: &[u8]) -> (PathBuf, PathBuf, PathBuf) {
    let projects = temp.path().join("projects");
    let project = projects.join("synthetic-project");
    fs::create_dir_all(&project).unwrap();
    let source = project.join("session-a.jsonl");
    fs::write(&source, bytes).unwrap();
    let state = temp.path().join("state/relay.json");
    (projects, source, state)
}

fn frame(seq: u64, previous: &[u8; 32], line: &[u8]) -> Frame {
    Frame::plaintext(
        FrameKey {
            session: "synthetic-session".into(),
            stream: "main".into(),
            generation: 0,
            epoch: 0,
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

fn frame_line(frame: &Frame) -> &[u8] {
    frame.plaintext_content().unwrap().1
}

#[derive(Default)]
struct FakeClock(AtomicU64);

impl FakeClock {
    fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::Relaxed);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

struct ReceiverTransport<'a> {
    receiver: &'a Receiver,
    live_calls: AtomicUsize,
}

impl<'a> ReceiverTransport<'a> {
    fn new(receiver: &'a Receiver) -> Self {
        Self {
            receiver,
            live_calls: AtomicUsize::new(0),
        }
    }
}

impl Transport for ReceiverTransport<'_> {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.live_calls.fetch_add(1, Ordering::Relaxed);
        self.receiver
            .accept(frame)
            .map_err(transport_error_from_receive)?;
        Ok(frame.key.seq)
    }

    fn send_orphan(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.receiver
            .accept_orphan(frame)
            .map_err(transport_error_from_receive)?;
        Ok(frame.key.seq)
    }

    fn lease_status(
        &self,
        session: Option<&str>,
        _machine: &str,
    ) -> Result<LeaseStatus, TransportError> {
        self.receiver
            .lease_status(session)
            .map_err(transport_error_from_receive)
    }

    fn acquire(&self, session: &str, machine: &str) -> Result<LeaseRow, TransportError> {
        self.receiver
            .acquire(session, machine)
            .map_err(transport_error_from_receive)
    }

    fn renew(&self, session: &str, machine: &str, epoch: u64) -> Result<LeaseRow, TransportError> {
        self.receiver
            .renew(session, machine, epoch)
            .map_err(transport_error_from_receive)
    }

    fn takeover(
        &self,
        session: &str,
        machine: &str,
        expected_epoch: u64,
        force: bool,
    ) -> Result<TakeoverResult, TransportError> {
        self.receiver
            .takeover(session, machine, expected_epoch, force)
            .map_err(transport_error_from_receive)
    }

    fn lease_tips(&self, session: &str, _machine: &str) -> Result<TakeoverResult, TransportError> {
        self.receiver
            .lease_tips(session)
            .map_err(transport_error_from_receive)
    }

    fn list_orphans(&self, _machine: &str) -> Result<Vec<OrphanSummary>, TransportError> {
        self.receiver
            .list_orphans()
            .map_err(transport_error_from_receive)
    }
}

fn transport_error_from_receive(error: ReceiveError) -> TransportError {
    match error {
        ReceiveError::Fenced { current_epoch, .. } => TransportError::Fenced { current_epoch },
        ReceiveError::NotHolder { current_epoch, .. } => {
            TransportError::NotHolder { current_epoch }
        }
        error => TransportError::Unavailable(error.to_string()),
    }
}

struct AppendBeforeTakeover<'a> {
    receiver: &'a Receiver,
    frame: Mutex<Option<Frame>>,
}

impl Transport for AppendBeforeTakeover<'_> {
    fn send(&self, _frame: &Frame) -> Result<u64, TransportError> {
        unreachable!("takeover test does not send through this transport")
    }

    fn lease_tips(&self, session: &str, _machine: &str) -> Result<TakeoverResult, TransportError> {
        self.receiver
            .lease_tips(session)
            .map_err(transport_error_from_receive)
    }

    fn takeover(
        &self,
        session: &str,
        machine: &str,
        expected_epoch: u64,
        force: bool,
    ) -> Result<TakeoverResult, TransportError> {
        if let Some(frame) = self.frame.lock().unwrap().take() {
            self.receiver
                .accept(&frame)
                .map_err(transport_error_from_receive)?;
        }
        self.receiver
            .takeover(session, machine, expected_epoch, force)
            .map_err(transport_error_from_receive)
    }
}

#[test]
fn frames_complete_line_verbatim_and_waits_on_partial_line() {
    let temp = TempDir::new("framing");
    let complete = "{\"text\":\"héllo\"}\n".as_bytes();
    let mut contents = complete.to_vec();
    contents.extend_from_slice("{\"partial\":\"界".as_bytes());
    let (projects, _, state) = source_file(&temp, &contents);
    let transport = RecordingTransport::default();

    let report = run_pass(&projects, &state, &transport).unwrap();

    assert_eq!(report.totals().0, 1);
    let frames = transport.frames.lock().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frame_line(&frames[0]), complete);
    assert_eq!(frames[0].key.seq, 0);
}

#[test]
fn duplicate_is_no_op_and_different_bytes_are_rejected() {
    let temp = TempDir::new("idempotence");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    receiver.acquire("synthetic-session", "machine-a").unwrap();
    let original = frame(0, &ZERO_CHAIN, b"one\n");
    assert_eq!(receiver.accept(&original).unwrap(), ReceiveOutcome::Stored);
    let stored = receiver
        .read_frame("synthetic-session", "main", 0, 0, 0)
        .unwrap()
        .unwrap();

    assert_eq!(
        receiver.accept(&original).unwrap(),
        ReceiveOutcome::Duplicate
    );
    assert_eq!(
        receiver
            .read_frame("synthetic-session", "main", 0, 0, 0)
            .unwrap()
            .unwrap(),
        stored
    );
    let collision = frame(0, &ZERO_CHAIN, b"different\n");
    assert!(matches!(
        receiver.accept(&collision),
        Err(ReceiveError::Collision)
    ));
}

#[test]
fn receiver_reports_the_first_sequence_hole() {
    let temp = TempDir::new("gap");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    receiver.acquire("synthetic-session", "machine-a").unwrap();
    let mut previous = ZERO_CHAIN;
    for seq in 0..=5 {
        let current = frame(seq, &previous, format!("line-{seq}\n").as_bytes());
        previous = frame_chain(&current);
        receiver.accept(&current).unwrap();
    }
    let sequence_seven = frame(7, &previous, b"line-7\n");
    assert!(matches!(
        receiver.accept(&sequence_seven),
        Err(ReceiveError::Gap {
            expected: 6,
            actual: 7
        })
    ));
}

#[test]
fn receiver_rejects_a_chain_break() {
    let temp = TempDir::new("chain");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    receiver.acquire("synthetic-session", "machine-a").unwrap();
    let mut broken = frame(0, &ZERO_CHAIN, b"one\n");
    if let FrameContent::Plaintext { chain, .. } = &mut broken.content {
        *chain = [9; 32];
    }
    assert!(matches!(
        receiver.accept(&broken),
        Err(ReceiveError::Chain { seq: 0 })
    ));
}

#[test]
fn receiver_reconstructs_an_uncheckpointed_receipt_after_restart() {
    let temp = TempDir::new("receiver-restart");
    let root = temp.path().join("receiver");
    let first = frame(0, &ZERO_CHAIN, b"one\n");
    let first_receiver = Receiver::open(&root).unwrap();
    first_receiver
        .acquire("synthetic-session", "machine-a")
        .unwrap();
    first_receiver.accept(&first).unwrap();

    let receiver = Receiver::open(&root).unwrap();
    let second = frame(1, &frame_chain(&first), b"two\n");
    assert_eq!(receiver.accept(&second).unwrap(), ReceiveOutcome::Stored);
}

#[test]
fn receiver_continues_from_m1_frame_files_without_machine_metadata() {
    let temp = TempDir::new("m1-storage-upgrade");
    let root = temp.path().join("receiver");
    let first = frame(0, &ZERO_CHAIN, b"one\n");
    let path = root
        .join(hex::encode("synthetic-session"))
        .join(hex::encode("main"))
        .join("generation-0/epoch-0/frames/00000000000000000000.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({
            "boot_id": first.boot_id,
            "chain": hex::encode(frame_chain(&first)),
            "epoch": 0,
            "generation": 0,
            "line": hex::encode(frame_line(&first)),
            "sender_mono_ns": first.sender_mono_ns,
            "sender_wall_ns": first.sender_wall_ns,
            "seq": 0,
            "session": "synthetic-session",
            "stream": "main"
        }))
        .unwrap(),
    )
    .unwrap();

    let receiver = Receiver::open(root).unwrap();
    receiver.acquire("synthetic-session", "machine-a").unwrap();
    let second = frame(1, &frame_chain(&first), b"two\n");
    assert_eq!(receiver.accept(&second).unwrap(), ReceiveOutcome::Stored);
}

struct DropEverySecondAck<'a> {
    receiver: &'a Receiver,
    calls: AtomicUsize,
}

impl Transport for DropEverySecondAck<'_> {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.receiver
            .accept(frame)
            .map_err(|error| TransportError::Unavailable(error.to_string()))?;
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if call.is_multiple_of(2) {
            Err(TransportError::Unavailable("synthetic dropped ack".into()))
        } else {
            Ok(frame.key.seq)
        }
    }

    fn acquire(&self, session: &str, machine: &str) -> Result<LeaseRow, TransportError> {
        self.receiver
            .acquire(session, machine)
            .map_err(|error| TransportError::Unavailable(error.to_string()))
    }
}

#[test]
fn dropped_acks_retransmit_without_skipping_the_watermark() {
    let temp = TempDir::new("retransmit");
    let (projects, _, state_path) = source_file(&temp, b"zero\none\ntwo\nthree\nfour\nfive\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = DropEverySecondAck {
        receiver: &receiver,
        calls: AtomicUsize::new(0),
    };
    let mut previous = None;
    for _ in 0..12 {
        let _ = run_pass(&projects, &state_path, &transport).unwrap();
        let state = load_state(&state_path).unwrap();
        let acked = state
            .streams
            .get(&("session-a".into(), "main".into()))
            .unwrap()
            .acked;
        if let (Some(before), Some(after)) = (previous, acked) {
            assert!(after >= before);
        }
        previous = acked;
        if acked == Some(5) {
            break;
        }
    }
    assert_eq!(previous, Some(5));
    for seq in 0..6 {
        assert!(
            receiver
                .read_frame("session-a", "main", 0, 0, seq)
                .unwrap()
                .is_some()
        );
    }
}

struct Unreachable {
    calls: AtomicUsize,
}

impl Transport for Unreachable {
    fn send(&self, _frame: &Frame) -> Result<u64, TransportError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(TransportError::Unavailable(
            "synthetic receiver outage".into(),
        ))
    }
}

#[test]
fn unreachable_receiver_leaves_watermark_and_source_unchanged() {
    let temp = TempDir::new("backpressure");
    let contents = (0..2_000)
        .map(|index| format!("synthetic-{index}\n"))
        .collect::<String>();
    let (projects, source, state_path) = source_file(&temp, contents.as_bytes());
    let before = fs::read(&source).unwrap();
    let transport = Unreachable {
        calls: AtomicUsize::new(0),
    };

    let report = run_pass(&projects, &state_path, &transport).unwrap();

    assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    assert_eq!(report.streams[0].backlog_lines, 2_000);
    assert_eq!(fs::read(&source).unwrap(), before);
    let state = load_state(&state_path).unwrap();
    assert_eq!(
        state
            .streams
            .get(&("session-a".into(), "main".into()))
            .unwrap()
            .acked,
        None
    );
}

#[test]
fn resident_sender_measures_lag_from_the_first_failed_attempt() {
    let temp = TempDir::new("first-observed");
    let (projects, _, state_path) = source_file(&temp, b"one\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = DropEverySecondAck {
        receiver: &receiver,
        calls: AtomicUsize::new(1),
    };
    let mut sender = Sender::default();
    assert!(
        sender
            .run_pass(&projects, &state_path, &transport)
            .unwrap()
            .had_failures()
    );
    thread::sleep(Duration::from_millis(20));
    let report = sender.run_pass(&projects, &state_path, &transport).unwrap();
    assert!(!report.had_failures());
    assert!(report.lag.max >= Duration::from_millis(20));
}

struct ToggleTransport {
    available: AtomicBool,
    frames: Mutex<Vec<Frame>>,
}

impl Transport for ToggleTransport {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        if !self.available.load(Ordering::Relaxed) {
            return Err(TransportError::Unavailable(
                "synthetic receiver outage".into(),
            ));
        }
        self.frames.lock().unwrap().push(frame.clone());
        Ok(frame.key.seq)
    }
}

#[test]
fn failed_pass_observation_covers_its_visible_backlog() {
    let temp = TempDir::new("failed-pass-observation");
    let (projects, source, state_path) = source_file(&temp, b"zero\none\ntwo\n");
    let transport = ToggleTransport {
        available: AtomicBool::new(false),
        frames: Mutex::new(Vec::new()),
    };
    let mut sender = Sender::default();
    assert!(
        sender
            .run_pass(&projects, &state_path, &transport)
            .unwrap()
            .had_failures()
    );

    let outage = Duration::from_millis(30);
    thread::sleep(outage);
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(source)
        .unwrap()
        .write_all(b"three\n")
        .unwrap();
    transport.available.store(true, Ordering::Relaxed);

    let report = sender.run_pass(&projects, &state_path, &transport).unwrap();

    assert!(!report.had_failures());
    assert_eq!(report.totals().0, 4);
    assert!(report.lag.p50 >= outage);
    let frames = transport.frames.lock().unwrap();
    assert_eq!(frames.len(), 4);
    assert!(
        frames[..3]
            .iter()
            .all(|frame| frame.sender_wall_ns == frames[0].sender_wall_ns)
    );
    assert!(frames[3].sender_wall_ns > frames[0].sender_wall_ns);
    assert!(frames[3].sender_mono_ns > frames[0].sender_mono_ns);
}

struct DelayedTransport {
    delay: Duration,
    frames: Mutex<Vec<Frame>>,
}

impl Transport for DelayedTransport {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        thread::sleep(self.delay);
        self.frames.lock().unwrap().push(frame.clone());
        Ok(frame.key.seq)
    }
}

#[test]
fn backfill_lag_starts_when_the_pass_observes_the_stream() {
    let temp = TempDir::new("pass-observation");
    let (projects, _, state_path) = source_file(&temp, b"zero\none\ntwo\nthree\n");
    let transport = DelayedTransport {
        delay: Duration::from_millis(5),
        frames: Mutex::new(Vec::new()),
    };

    let report = run_pass(&projects, &state_path, &transport).unwrap();

    assert!(report.pass_duration >= Duration::from_millis(20));
    assert!(report.lag.max >= Duration::from_millis(20));
    let frames = transport.frames.lock().unwrap();
    assert_eq!(frames.len(), 4);
    assert!(
        frames
            .iter()
            .all(|frame| frame.sender_wall_ns == frames[0].sender_wall_ns)
    );
    assert!(
        frames
            .iter()
            .all(|frame| frame.sender_mono_ns == frames[0].sender_mono_ns)
    );
}

#[test]
fn resident_sender_resumes_at_offset_without_reading_the_prefix() {
    let temp = TempDir::new("offset-resume");
    let contents = (0..1_000)
        .map(|index| format!("synthetic-record-{index:04}\n"))
        .collect::<String>();
    let (projects, source, state_path) = source_file(&temp, contents.as_bytes());
    let transport = RecordingTransport::default();
    let mut sender = Sender::default();
    sender.run_pass(&projects, &state_path, &transport).unwrap();

    let idle = sender.run_pass(&projects, &state_path, &transport).unwrap();
    let last_line_len = "synthetic-record-0999\n".len() as u64;
    assert_eq!(idle.totals().0, 0);
    assert_eq!(idle.source_bytes_read(), last_line_len);
    assert!(idle.source_bytes_read() < contents.len() as u64 / 100);

    let appended = b"appended\n";
    let mut file = fs::OpenOptions::new().append(true).open(source).unwrap();
    use std::io::Write;
    file.write_all(appended).unwrap();
    let appended_report = sender.run_pass(&projects, &state_path, &transport).unwrap();
    assert_eq!(appended_report.totals().0, 1);
    assert_eq!(
        appended_report.source_bytes_read(),
        last_line_len + appended.len() as u64
    );
}

#[test]
fn replacement_mid_follow_falls_back_and_starts_a_generation() {
    let temp = TempDir::new("follow-replacement");
    let (projects, source, state_path) = source_file(&temp, b"zero\none\ntwo\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = LocalTransport(&receiver);
    let mut sender = Sender::default();
    sender.run_pass(&projects, &state_path, &transport).unwrap();

    fs::rename(&source, source.with_extension("jsonl.old")).unwrap();
    fs::write(&source, b"replacement-zero\nreplacement-one\n").unwrap();
    let report = sender.run_pass(&projects, &state_path, &transport).unwrap();

    assert_eq!(report.streams[0].generation, 1);
    assert!(report.source_bytes_read() >= b"replacement-zero\n".len() as u64);
}

#[test]
fn truncation_mid_follow_falls_back_and_starts_a_generation() {
    let temp = TempDir::new("follow-truncation");
    let (projects, source, state_path) = source_file(&temp, b"zero\none\ntwo\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = LocalTransport(&receiver);
    let mut sender = Sender::default();
    sender.run_pass(&projects, &state_path, &transport).unwrap();

    fs::write(&source, b"zero\n").unwrap();
    let report = sender.run_pass(&projects, &state_path, &transport).unwrap();

    assert_eq!(report.streams[0].generation, 1);
}

#[test]
fn internal_same_inode_rewrite_waits_for_startup_full_check() {
    let temp = TempDir::new("known-rewrite-limit");
    let (projects, source, state_path) = source_file(&temp, b"zero\none\ntwo\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = LocalTransport(&receiver);
    let mut resident = Sender::default();
    resident
        .run_pass(&projects, &state_path, &transport)
        .unwrap();

    // Same inode and length, with the last acknowledged line left intact.
    fs::write(&source, b"ZERO\none\ntwo\n").unwrap();
    let resident_report = resident
        .run_pass(&projects, &state_path, &transport)
        .unwrap();
    assert_eq!(resident_report.streams[0].generation, 0);

    let startup_report = run_pass(&projects, &state_path, &transport).unwrap();
    assert_eq!(startup_report.streams[0].generation, 1);
}

enum Rewrite {
    InPlace,
    Truncate,
    RenameRecreate,
}

fn assert_new_generation(kind: Rewrite, label: &str) {
    let temp = TempDir::new(label);
    let original = b"zero\none\ntwo\nthree\n";
    let (projects, source, state_path) = source_file(&temp, original);
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = LocalTransport(&receiver);
    assert_eq!(
        run_pass(&projects, &state_path, &transport)
            .unwrap()
            .totals()
            .0,
        4
    );

    match kind {
        Rewrite::InPlace => fs::write(&source, b"zero\none\nrewritten\nthree\n").unwrap(),
        Rewrite::Truncate => fs::write(&source, b"zero\none\n").unwrap(),
        Rewrite::RenameRecreate => {
            fs::rename(&source, source.with_extension("jsonl.old")).unwrap();
            fs::write(&source, b"replacement-zero\nreplacement-one\n").unwrap();
        }
    }

    let report = run_pass(&projects, &state_path, &transport).unwrap();
    assert_eq!(report.streams[0].generation, 1);
    let old = receiver
        .read_frame("session-a", "main", 0, 0, 2)
        .unwrap()
        .unwrap();
    assert_eq!(frame_line(&old), b"two\n");
    assert!(
        receiver
            .read_frame("session-a", "main", 1, 0, 0)
            .unwrap()
            .is_some()
    );
}

#[test]
fn changed_acked_prefix_starts_a_new_generation() {
    assert_new_generation(Rewrite::InPlace, "rewrite");
}

#[test]
fn truncation_starts_a_new_generation() {
    assert_new_generation(Rewrite::Truncate, "truncate");
}

#[test]
fn rename_then_recreate_starts_a_new_generation() {
    assert_new_generation(Rewrite::RenameRecreate, "rename-recreate");
}

#[test]
fn subagent_streams_are_discovered_and_shipped_independently() {
    let temp = TempDir::new("subagents");
    let (projects, source, state_path) = source_file(&temp, b"main\n");
    let session_dir = source.parent().unwrap().join("session-a/subagents");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(session_dir.join("agent-worker.jsonl"), b"side\n").unwrap();
    fs::write(session_dir.join("agent-worker.meta.json"), b"{}\n").unwrap();
    let discovered = discover_streams(&projects).unwrap();
    assert_eq!(discovered.len(), 2);
    assert!(
        discovered
            .iter()
            .any(|stream| stream.stream == "subagents/agent-worker.jsonl")
    );

    let transport = RecordingTransport::default();
    let report = run_pass(&projects, &state_path, &transport).unwrap();
    assert_eq!(report.streams.len(), 2);
    let frames = transport.frames.lock().unwrap();
    assert!(frames.iter().any(|frame| frame.key.stream == "main"));
    assert!(
        frames
            .iter()
            .any(|frame| frame.key.stream == "subagents/agent-worker.jsonl")
    );
}

#[test]
fn receiver_refuses_non_loopback_bind() {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8734);
    assert!(matches!(
        validate_loopback(address),
        Err(ReceiveError::NonLoopback(found)) if found == address
    ));
    assert!(validate_loopback(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8734)).is_ok());
}

#[test]
fn acquire_renew_and_expiry_use_the_receiver_clock() {
    let temp = TempDir::new("lease-clock");
    let clock = Arc::new(FakeClock::default());
    clock.set(1_000);
    let receiver = Receiver::open_with_clock(temp.path().join("receiver"), clock.clone()).unwrap();

    let acquired = receiver.acquire("session", "machine-a").unwrap();
    assert_eq!(acquired.epoch, 0);
    assert_eq!(acquired.holder_machine, "machine-a");
    assert_eq!(acquired.lease_expires_at_ms, 181_000);

    clock.set(61_000);
    let renewed = receiver.renew("session", "machine-a", 0).unwrap();
    assert_eq!(renewed.lease_expires_at_ms, 241_000);
    let refused = receiver.acquire("session", "machine-b").unwrap();
    assert_eq!(refused, renewed);

    clock.set(240_999);
    assert!(matches!(
        receiver.takeover("session", "machine-b", 0, false),
        Err(ReceiveError::LeaseActive { .. })
    ));
    clock.set(241_000);
    let takeover = receiver.takeover("session", "machine-b", 0, false).unwrap();
    assert_eq!(takeover.row.epoch, 1);
    assert_eq!(takeover.row.holder_machine, "machine-b");
    assert!(!receiver.lease_status(Some("session")).unwrap().takeovers[0].forced);
}

#[test]
fn forced_takeover_is_logged_and_persists() {
    let temp = TempDir::new("forced-takeover");
    let root = temp.path().join("receiver");
    let clock = Arc::new(FakeClock::default());
    let receiver = Receiver::open_with_clock(&root, clock.clone()).unwrap();
    receiver.acquire("session", "machine-a").unwrap();

    assert!(matches!(
        receiver.takeover("session", "machine-b", 0, false),
        Err(ReceiveError::LeaseActive { .. })
    ));
    let takeover = receiver.takeover("session", "machine-b", 0, true).unwrap();
    assert_eq!(takeover.row.epoch, 1);
    let status = receiver.lease_status(Some("session")).unwrap();
    assert_eq!(status.takeovers.len(), 1);
    assert!(status.takeovers[0].forced);
    assert_eq!(status.takeovers[0].previous_holder, "machine-a");

    drop(receiver);
    let reopened = Receiver::open_with_clock(&root, clock).unwrap();
    let status = reopened.lease_status(Some("session")).unwrap();
    assert_eq!(status.rows[0], takeover.row);
    assert_eq!(status.takeovers.len(), 1);
    assert!(status.takeovers[0].forced);
}

#[test]
fn takeover_compare_and_swap_has_exactly_one_winner() {
    let temp = TempDir::new("takeover-cas");
    let clock = Arc::new(FakeClock::default());
    let receiver =
        Arc::new(Receiver::open_with_clock(temp.path().join("receiver"), clock.clone()).unwrap());
    receiver.acquire("session", "machine-a").unwrap();
    clock.set(180_000);
    let barrier = Arc::new(Barrier::new(3));
    let handles = ["machine-b", "machine-c"].map(|machine| {
        let receiver = receiver.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            receiver.takeover("session", machine, 0, false)
        })
    });
    barrier.wait();
    let results = handles.map(|handle| handle.join().unwrap());

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ReceiveError::TakeoverConflict { .. })))
            .count(),
        1
    );
}

#[test]
fn takeover_precheck_refuses_a_missing_local_stream_without_advancing_epoch() {
    let old = TempDir::new("takeover-missing-old");
    let target = TempDir::new("takeover-missing-target");
    let (old_projects, _, old_state) = source_file(&old, b"zero\n");
    let receiver = Receiver::open(old.path().join("receiver")).unwrap();
    let transport = ReceiverTransport::new(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&old_projects, &old_state, &transport)
        .unwrap();
    let target_projects = target.path().join("projects");
    fs::create_dir_all(target_projects.join("empty-project")).unwrap();
    let target_state = target.path().join("state/relay.json");

    let error = takeover_session(
        &target_projects,
        &target_state,
        "session-a",
        "machine-b",
        true,
        &transport,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        TakeoverCommandError::Precheck(RelayError::TakeoverStreamMissing {
            ref stream,
            ..
        }) if stream == "main"
    ));
    assert_eq!(
        receiver.lease_status(Some("session-a")).unwrap().rows[0].epoch,
        0
    );
    assert!(!target_state.exists());
}

#[test]
fn takeover_precheck_refuses_a_mismatched_prefix_without_advancing_epoch() {
    let old = TempDir::new("takeover-mismatch-old");
    let target = TempDir::new("takeover-mismatch-target");
    let (old_projects, _, old_state) = source_file(&old, b"zero\n");
    let receiver = Receiver::open(old.path().join("receiver")).unwrap();
    let transport = ReceiverTransport::new(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&old_projects, &old_state, &transport)
        .unwrap();
    let (target_projects, _, target_state) = source_file(&target, b"ZERO\n");

    let error = takeover_session(
        &target_projects,
        &target_state,
        "session-a",
        "machine-b",
        true,
        &transport,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        TakeoverCommandError::Precheck(RelayError::TakeoverPrefix {
            ref stream,
            ..
        }) if stream == "main"
    ));
    assert_eq!(
        receiver.lease_status(Some("session-a")).unwrap().rows[0].epoch,
        0
    );
    assert!(!target_state.exists());
}

#[test]
fn takeover_race_reports_behind_after_cas_without_writing_sender_state() {
    let old = TempDir::new("takeover-race-old");
    let target = TempDir::new("takeover-race-target");
    let (old_projects, _, old_state) = source_file(&old, b"zero\n");
    let receiver = Receiver::open(old.path().join("receiver")).unwrap();
    let initial_transport = ReceiverTransport::new(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&old_projects, &old_state, &initial_transport)
        .unwrap();
    let first = receiver
        .read_frame("session-a", "main", 0, 0, 0)
        .unwrap()
        .unwrap();
    let injected = Frame::plaintext(
        FrameKey {
            seq: 1,
            ..first.key.clone()
        },
        "machine-a".into(),
        chain_line(&frame_chain(&first), b"raced\n"),
        first.sender_wall_ns.saturating_add(1),
        first.sender_mono_ns.saturating_add(1),
        first.boot_id,
        b"raced\n".to_vec(),
    );
    let transport = AppendBeforeTakeover {
        receiver: &receiver,
        frame: Mutex::new(Some(injected)),
    };
    let (target_projects, _, target_state) = source_file(&target, b"zero\n");

    let error = takeover_session(
        &target_projects,
        &target_state,
        "session-a",
        "machine-b",
        true,
        &transport,
    )
    .unwrap_err();

    assert!(matches!(
        &error,
        TakeoverCommandError::PostCommitBehind {
            stream,
            behind: 1,
            epoch: 1,
            ..
        } if stream == "main"
    ));
    assert!(error.to_string().contains(
        "local copy is 1 frames behind the receiver tip for stream session-a/main; the epoch is now 1; bring the copy up to date (M4 restore) before starting the sender"
    ));
    assert_eq!(
        receiver.lease_status(Some("session-a")).unwrap().rows[0].epoch,
        1
    );
    assert!(!target_state.exists());
}

#[test]
fn fenced_sender_moves_its_unacknowledged_tail_to_orphans() {
    let temp = TempDir::new("sender-fenced");
    let (projects, source, state_path) = source_file(&temp, b"zero\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let transport = ReceiverTransport::new(&receiver);
    let mut sender = Sender::with_machine("machine-a");
    sender.run_pass(&projects, &state_path, &transport).unwrap();

    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(b"one\ntwo\n")
        .unwrap();
    receiver
        .takeover("session-a", "machine-b", 0, true)
        .unwrap();

    let report = sender.run_pass(&projects, &state_path, &transport).unwrap();
    assert!(report.streams[0].fenced);
    assert_eq!(report.streams[0].orphan_lines_acked, 2);
    assert!(
        receiver
            .read_frame("session-a", "main", 0, 0, 1)
            .unwrap()
            .is_none()
    );
    let orphan = receiver
        .read_orphan("session-a", "main", 0, 0, 1)
        .unwrap()
        .unwrap();
    assert_eq!(frame_line(&orphan), b"one\n");
    assert_eq!(receiver.list_orphans().unwrap()[0].frames, 2);
    let state = load_state(&state_path).unwrap();
    let stream = state
        .streams
        .get(&("session-a".into(), "main".into()))
        .unwrap();
    assert_eq!(stream.acked, Some(0));
    assert_eq!(stream.orphaned, Some(2));
    assert!(stream.fenced);
}

#[test]
fn new_epoch_continues_sequence_and_chain_from_the_live_tip() {
    let old = TempDir::new("epoch-old");
    let new = TempDir::new("epoch-new");
    let contents = b"zero\none\n";
    let (old_projects, _, old_state) = source_file(&old, contents);
    let receiver = Receiver::open(old.path().join("receiver")).unwrap();
    let transport = ReceiverTransport::new(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&old_projects, &old_state, &transport)
        .unwrap();

    let takeover = receiver
        .takeover("session-a", "machine-b", 0, true)
        .unwrap();
    let (new_projects, new_source, new_state) = source_file(&new, contents);
    initialize_takeover_state(&new_projects, &new_state, "session-a", &takeover).unwrap();
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(new_source)
        .unwrap()
        .write_all(b"two\n")
        .unwrap();
    Sender::with_machine("machine-b")
        .run_pass(&new_projects, &new_state, &transport)
        .unwrap();

    let continued = receiver
        .read_frame("session-a", "main", 0, 1, 2)
        .unwrap()
        .unwrap();
    assert_eq!(frame_line(&continued), b"two\n");
    let gap = Frame::plaintext(
        FrameKey {
            session: "session-a".into(),
            stream: "main".into(),
            generation: 0,
            epoch: 1,
            seq: 4,
        },
        "machine-b".into(),
        chain_line(&frame_chain(&continued), b"four\n"),
        0,
        0,
        "synthetic".into(),
        b"four\n".to_vec(),
    );
    assert!(matches!(
        receiver.accept(&gap),
        Err(ReceiveError::Gap {
            expected: 3,
            actual: 4
        })
    ));
}

#[test]
fn receiver_reconstructs_continuity_across_epochs_after_restart() {
    let temp = TempDir::new("epoch-restart");
    let root = temp.path().join("receiver");
    let first = frame(0, &ZERO_CHAIN, b"zero\n");
    let second = Frame::plaintext(
        FrameKey {
            epoch: 1,
            seq: 1,
            ..first.key.clone()
        },
        "machine-b".into(),
        chain_line(&frame_chain(&first), b"one\n"),
        2,
        2,
        "synthetic".into(),
        b"one\n".to_vec(),
    );
    let receiver = Receiver::open(&root).unwrap();
    receiver.acquire("synthetic-session", "machine-a").unwrap();
    receiver.accept(&first).unwrap();
    receiver
        .takeover("synthetic-session", "machine-b", 0, true)
        .unwrap();
    receiver.accept(&second).unwrap();
    drop(receiver);

    let receiver = Receiver::open(&root).unwrap();
    let third = Frame::plaintext(
        FrameKey {
            seq: 2,
            ..second.key.clone()
        },
        second.machine.clone(),
        chain_line(&frame_chain(&second), b"two\n"),
        3,
        3,
        second.boot_id.clone(),
        b"two\n".to_vec(),
    );
    assert_eq!(receiver.accept(&third).unwrap(), ReceiveOutcome::Stored);
}

#[test]
fn restarted_stale_sender_checks_the_register_before_live_shipping() {
    let temp = TempDir::new("startup-fence");
    let (projects, source, state_path) = source_file(&temp, b"zero\n");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    let initial_transport = ReceiverTransport::new(&receiver);
    Sender::with_machine("machine-a")
        .run_pass(&projects, &state_path, &initial_transport)
        .unwrap();
    receiver
        .takeover("session-a", "machine-b", 0, true)
        .unwrap();
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(source)
        .unwrap()
        .write_all(b"late\n")
        .unwrap();

    let restarted_transport = ReceiverTransport::new(&receiver);
    let report = Sender::with_machine("machine-a")
        .run_pass(&projects, &state_path, &restarted_transport)
        .unwrap();

    assert_eq!(restarted_transport.live_calls.load(Ordering::Relaxed), 0);
    assert!(report.streams[0].fenced);
    let orphan = receiver
        .read_orphan("session-a", "main", 0, 0, 1)
        .unwrap()
        .unwrap();
    assert_eq!(frame_line(&orphan), b"late\n");
}

#[test]
fn sender_wall_clock_does_not_control_lease_expiry() {
    let temp = TempDir::new("sender-clock-skew");
    let clock = Arc::new(FakeClock::default());
    let receiver = Receiver::open_with_clock(temp.path().join("receiver"), clock.clone()).unwrap();
    receiver.acquire("synthetic-session", "machine-a").unwrap();
    let mut skewed = frame(0, &ZERO_CHAIN, b"one\n");
    skewed.sender_wall_ns = u64::MAX;
    receiver.accept(&skewed).unwrap();

    assert!(matches!(
        receiver.takeover("synthetic-session", "machine-b", 0, false),
        Err(ReceiveError::LeaseActive { .. })
    ));
    clock.set(180_000);
    assert!(
        receiver
            .takeover("synthetic-session", "machine-b", 0, false)
            .is_ok()
    );
}

#[test]
fn current_epoch_frame_from_a_non_holder_is_rejected() {
    let temp = TempDir::new("wrong-holder");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
    receiver.acquire("synthetic-session", "machine-a").unwrap();
    receiver
        .takeover("synthetic-session", "machine-b", 0, true)
        .unwrap();
    let mut wrong_holder = frame(0, &ZERO_CHAIN, b"one\n");
    wrong_holder.key.epoch = 1;

    assert!(matches!(
        receiver.accept(&wrong_holder),
        Err(ReceiveError::NotHolder {
            current_epoch: 1,
            ..
        })
    ));
}

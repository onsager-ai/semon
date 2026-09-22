use std::{
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use semon_relay::{
    Frame, FrameKey, ReceiveError, ReceiveOutcome, Receiver, Sender, Transport, TransportError,
    ZERO_CHAIN, chain_line, discover_streams, load_state, run_pass, validate_loopback,
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
    Frame {
        key: FrameKey {
            session: "synthetic-session".into(),
            stream: "main".into(),
            generation: 0,
            epoch: 0,
            seq,
        },
        chain: chain_line(previous, line),
        sender_wall_ns: 1,
        sender_mono_ns: 2,
        boot_id: "synthetic-boot".into(),
        line: line.to_vec(),
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
    assert_eq!(frames[0].line, complete);
    assert_eq!(frames[0].key.seq, 0);
}

#[test]
fn duplicate_is_no_op_and_different_bytes_are_rejected() {
    let temp = TempDir::new("idempotence");
    let receiver = Receiver::open(temp.path().join("receiver")).unwrap();
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
    let mut previous = ZERO_CHAIN;
    for seq in 0..=5 {
        let current = frame(seq, &previous, format!("line-{seq}\n").as_bytes());
        previous = current.chain;
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
    let mut broken = frame(0, &ZERO_CHAIN, b"one\n");
    broken.chain = [9; 32];
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
    Receiver::open(&root).unwrap().accept(&first).unwrap();

    let receiver = Receiver::open(&root).unwrap();
    let second = frame(1, &first.chain, b"two\n");
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
    assert_eq!(old.line, b"two\n");
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

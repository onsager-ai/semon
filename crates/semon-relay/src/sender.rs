use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    net::IpAddr,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use reqwest::{StatusCode, blocking::Client};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    DiscoveredStream, Frame, FrameKey, LEASE_RENEW_INTERVAL_MS, LeaseRow, LeaseStatus,
    OrphanSummary, RelayState, StreamState, StreamTip, TakeoverResult, ZERO_CHAIN, chain_line,
    discover_streams, discovery::DiscoveryError, lease::LeaseValueError, load_state, save_state,
    state::StateError,
};

type StreamKey = (String, String);
type ObservationKey = (String, String, u64);

/// A deliberately single-frame seam: the source file, not memory, is the queue.
pub trait Transport {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError>;

    fn send_orphan(&self, frame: &Frame) -> Result<u64, TransportError> {
        self.send(frame)
    }

    fn lease_status(
        &self,
        _session: Option<&str>,
        _machine: &str,
    ) -> Result<LeaseStatus, TransportError> {
        Ok(LeaseStatus::default())
    }

    fn acquire(&self, session: &str, machine: &str) -> Result<LeaseRow, TransportError> {
        Ok(LeaseRow {
            session: session.to_owned(),
            epoch: 0,
            holder_machine: machine.to_owned(),
            lease_expires_at_ms: u64::MAX,
        })
    }

    fn renew(&self, session: &str, machine: &str, epoch: u64) -> Result<LeaseRow, TransportError> {
        Ok(LeaseRow {
            session: session.to_owned(),
            epoch,
            holder_machine: machine.to_owned(),
            lease_expires_at_ms: u64::MAX,
        })
    }

    fn takeover(
        &self,
        _session: &str,
        _machine: &str,
        _expected_epoch: u64,
        _force: bool,
    ) -> Result<TakeoverResult, TransportError> {
        Err(TransportError::Unavailable(
            "transport does not implement takeover".into(),
        ))
    }

    fn lease_tips(&self, _session: &str, _machine: &str) -> Result<TakeoverResult, TransportError> {
        Err(TransportError::Unavailable(
            "transport does not implement lease tips".into(),
        ))
    }

    fn list_orphans(&self, _machine: &str) -> Result<Vec<OrphanSummary>, TransportError> {
        Ok(Vec::new())
    }
}

/// The blocking HTTP transport used by the command-line sender.
pub struct HttpTransport {
    client: Client,
    endpoint: reqwest::Url,
}

impl HttpTransport {
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Result<Self, TransportError> {
        let endpoint = endpoint.into();
        let url = reqwest::Url::parse(&endpoint)
            .map_err(|error| TransportError::InvalidEndpoint(error.to_string()))?;
        let address = url
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .ok_or_else(|| TransportError::NonLoopbackEndpoint(endpoint.clone()))?;
        if url.scheme() != "http" || !address.is_loopback() {
            return Err(TransportError::NonLoopbackEndpoint(endpoint));
        }
        Ok(Self {
            client: Client::builder()
                .timeout(timeout)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            endpoint: url,
        })
    }

    fn post(&self, path: &str, value: &Value) -> Result<Value, TransportError> {
        let mut endpoint = self.endpoint.clone();
        endpoint.set_path(path);
        endpoint.set_query(None);
        let response = self
            .client
            .post(endpoint)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(value)?)
            .send()?;
        let status = response.status();
        let mut body = response.text().unwrap_or_default();
        body.truncate(4096);
        if !status.is_success() {
            return Err(rejection(status, &body));
        }
        Ok(serde_json::from_str(&body)?)
    }
}

impl Transport for HttpTransport {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        let value = self.post("/v1/frames", &frame.to_value())?;
        value
            .get("acked")
            .and_then(Value::as_u64)
            .ok_or_else(|| TransportError::InvalidAck(value.to_string()))
    }

    fn send_orphan(&self, frame: &Frame) -> Result<u64, TransportError> {
        let value = self.post("/v1/orphans", &frame.to_value())?;
        value
            .get("acked")
            .and_then(Value::as_u64)
            .ok_or_else(|| TransportError::InvalidAck(value.to_string()))
    }

    fn lease_status(
        &self,
        session: Option<&str>,
        machine: &str,
    ) -> Result<LeaseStatus, TransportError> {
        let value = self.post(
            "/v1/lease/status",
            &Value::Object(Map::from_iter([
                ("machine".into(), Value::String(machine.to_owned())),
                (
                    "session".into(),
                    session.map_or(Value::Null, |value| Value::String(value.to_owned())),
                ),
            ])),
        )?;
        Ok(LeaseStatus::from_value(&value)?)
    }

    fn acquire(&self, session: &str, machine: &str) -> Result<LeaseRow, TransportError> {
        let value = self.post("/v1/lease/acquire", &lease_request(session, machine, []))?;
        Ok(LeaseRow::from_value(&value)?)
    }

    fn renew(&self, session: &str, machine: &str, epoch: u64) -> Result<LeaseRow, TransportError> {
        let value = self.post(
            "/v1/lease/renew",
            &lease_request(session, machine, [("epoch", epoch.into())]),
        )?;
        Ok(LeaseRow::from_value(&value)?)
    }

    fn takeover(
        &self,
        session: &str,
        machine: &str,
        expected_epoch: u64,
        force: bool,
    ) -> Result<TakeoverResult, TransportError> {
        let value = self.post(
            "/v1/lease/takeover",
            &lease_request(
                session,
                machine,
                [
                    ("expected_epoch", expected_epoch.into()),
                    ("force", force.into()),
                ],
            ),
        )?;
        Ok(TakeoverResult::from_value(&value)?)
    }

    fn lease_tips(&self, session: &str, machine: &str) -> Result<TakeoverResult, TransportError> {
        let value = self.post("/v1/lease/tips", &lease_request(session, machine, []))?;
        Ok(TakeoverResult::from_value(&value)?)
    }

    fn list_orphans(&self, machine: &str) -> Result<Vec<OrphanSummary>, TransportError> {
        let value = self.post(
            "/v1/orphans/list",
            &Value::Object(Map::from_iter([(
                "machine".into(),
                Value::String(machine.to_owned()),
            )])),
        )?;
        value
            .as_array()
            .ok_or_else(|| TransportError::InvalidResponse(value.to_string()))?
            .iter()
            .map(OrphanSummary::from_value)
            .collect::<Result<_, _>>()
            .map_err(TransportError::LeaseValue)
    }
}

fn lease_request<const N: usize>(session: &str, machine: &str, extra: [(&str, Value); N]) -> Value {
    let mut object = Map::from_iter([
        ("machine".into(), Value::String(machine.to_owned())),
        ("session".into(), Value::String(session.to_owned())),
    ]);
    object.extend(
        extra
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value)),
    );
    Value::Object(object)
}

fn rejection(status: StatusCode, body: &str) -> TransportError {
    let value = serde_json::from_str::<Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str);
    let current_epoch = value
        .as_ref()
        .and_then(|value| value.get("current_epoch"))
        .and_then(Value::as_u64);
    match (code, current_epoch) {
        (Some("fenced"), Some(current_epoch)) => TransportError::Fenced { current_epoch },
        (Some("not_holder"), Some(current_epoch)) => TransportError::NotHolder { current_epoch },
        _ => TransportError::Rejected {
            status,
            body: body.trim().to_owned(),
        },
    }
}

/// A delivery failure. Callers retain the source bytes and retry later.
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("receiver request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("cannot encode or decode receiver JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cannot decode lease response: {0}")]
    LeaseValue(#[from] LeaseValueError),
    #[error("receiver fenced this sender; current epoch is {current_epoch}")]
    Fenced { current_epoch: u64 },
    #[error("receiver says another machine holds the current epoch {current_epoch}")]
    NotHolder { current_epoch: u64 },
    #[error("receiver answered {status}: {body}")]
    Rejected { status: StatusCode, body: String },
    #[error("receiver returned an invalid acknowledgement: {0}")]
    InvalidAck(String),
    #[error("receiver returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("receiver unavailable: {0}")]
    Unavailable(String),
    #[error("invalid receiver endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("receiver endpoint must be an HTTP loopback IP URL: {0}")]
    NonLoopbackEndpoint(String),
}

/// Fatal local errors from a sender pass.
#[derive(Debug, Error)]
pub enum RelayError {
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("cannot read source stream {path}: {source}")]
    Source { path: PathBuf, source: io::Error },
    #[error("system clock is before the Unix epoch")]
    WallClock,
    #[error("cannot read Linux boot id: {0}")]
    BootId(io::Error),
    #[error("Linux boot id is empty")]
    EmptyBootId,
    #[error("cannot read Linux monotonic uptime: {0}")]
    Monotonic(io::Error),
    #[error("invalid Linux monotonic uptime: {0}")]
    InvalidMonotonic(String),
    #[error("stream generation overflowed for {session}/{stream}")]
    GenerationOverflow { session: String, stream: String },
    #[error("cannot read machine identity {path}: {source}")]
    MachineIdentity { path: PathBuf, source: io::Error },
    #[error("machine identity {0} is empty")]
    EmptyMachineIdentity(PathBuf),
    #[error("lease request failed for session {session}: {source}")]
    Lease {
        session: String,
        source: TransportError,
    },
    #[error("takeover state cannot find local stream {session}/{stream}")]
    TakeoverStreamMissing { session: String, stream: String },
    #[error(
        "local stream {session}/{stream} does not match receiver tip generation {generation} sequence {seq}"
    )]
    TakeoverPrefix {
        session: String,
        stream: String,
        generation: u64,
        seq: u64,
    },
    #[error(
        "local copy is {behind} frames behind the receiver tip for stream {session}/{stream} (local frames: {local_frames}, receiver tip sequence: {tip_seq})"
    )]
    TakeoverBehind {
        session: String,
        stream: String,
        local_frames: u64,
        tip_seq: u64,
        behind: u64,
    },
}

/// Failure while performing the preflight, CAS, and post-CAS takeover checks.
#[derive(Debug, Error)]
pub enum TakeoverCommandError {
    #[error("cannot read receiver tips before takeover: {0}")]
    Tips(#[source] TransportError),
    #[error("takeover refused before changing the epoch: {0}")]
    Precheck(#[source] RelayError),
    #[error("takeover compare-and-swap failed: {0}")]
    Cas(#[source] TransportError),
    #[error(
        "local copy is {behind} frames behind the receiver tip for stream {session}/{stream}; the epoch is now {epoch}; bring the copy up to date (M4 restore) before starting the sender"
    )]
    PostCommitBehind {
        session: String,
        stream: String,
        behind: u64,
        epoch: u64,
    },
    #[error(
        "takeover committed epoch {epoch}, but sender state was not written: {source}; bring the copy up to date (M4 restore) before starting the sender"
    )]
    PostCommit { epoch: u64, source: RelayError },
}

/// Distribution of observation-to-acknowledgement latency.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LagSummary {
    pub samples: u64,
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
}

/// Results for one independently advancing stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamReport {
    pub session: String,
    pub stream: String,
    pub generation: u64,
    pub epoch: u64,
    pub fenced: bool,
    pub lines_acked: u64,
    pub orphan_lines_acked: u64,
    pub bytes_acked: u64,
    pub backlog_lines: u64,
    pub backlog_bytes: u64,
    pub source_bytes_read: u64,
    pub lag: LagSummary,
    pub failure: Option<String>,
}

/// Results for one scan of the projects root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PassReport {
    pub streams: Vec<StreamReport>,
    pub lag: LagSummary,
    pub pass_duration: Duration,
}

/// Process-resident sender state used by follow mode.
#[derive(Default)]
pub struct Sender {
    /// Failed passes retain byte ranges, never one observation per line.
    observations: BTreeMap<ObservationKey, VecDeque<ObservationWindow>>,
    /// A stream receives a full prefix re-hash the first time this process sees it.
    startup_validated: BTreeSet<StreamKey>,
    machine: Option<String>,
    leases: BTreeMap<String, HeldLease>,
}

struct HeldLease {
    epoch: u64,
    renewed_at: Instant,
}

enum LeaseDecision {
    Held(u64),
    Fenced(u64),
    Unavailable(String),
}

#[derive(Clone)]
struct Observation {
    instant: Instant,
    wall_ns: u64,
    mono_ns: u64,
    boot_id: String,
}

struct ObservationWindow {
    through_offset: u64,
    observation: Observation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

struct StreamSnapshot {
    size: u64,
    identity: FileIdentity,
}

struct PreparedStream {
    file: File,
    observed_size: u64,
    validation_bytes_read: u64,
}

struct PrefixCheck {
    chain: [u8; 32],
    offset: u64,
    last_line_start: u64,
    last_line_hash: [u8; 32],
    bytes_read: u64,
}

struct PrefixScan {
    check: Option<PrefixCheck>,
    complete_lines: u64,
}

struct SendContext<'a, T> {
    state_path: &'a Path,
    transport: &'a T,
    machine: &'a str,
    pass_observation: &'a Observation,
    observations: &'a mut BTreeMap<ObservationKey, VecDeque<ObservationWindow>>,
    fenced_notice: Option<u64>,
}

impl PassReport {
    pub fn had_failures(&self) -> bool {
        self.streams.iter().any(|stream| stream.failure.is_some())
    }

    pub fn totals(&self) -> (u64, u64, u64, u64) {
        self.streams.iter().fold((0, 0, 0, 0), |totals, item| {
            (
                totals.0 + item.lines_acked,
                totals.1 + item.bytes_acked,
                totals.2 + item.backlog_lines,
                totals.3 + item.backlog_bytes,
            )
        })
    }

    pub fn source_bytes_read(&self) -> u64 {
        self.streams
            .iter()
            .map(|stream| stream.source_bytes_read)
            .sum()
    }
}

/// Makes one bounded-memory pass and atomically persists each stream's watermark.
pub fn run_pass(
    projects_root: &Path,
    state_path: &Path,
    transport: &impl Transport,
) -> Result<PassReport, RelayError> {
    Sender::default().run_pass(projects_root, state_path, transport)
}

/// Reads the stable local machine identity used by frames and lease calls.
pub fn read_machine_identity() -> Result<String, RelayError> {
    let path = PathBuf::from("/etc/machine-id");
    let value = fs::read_to_string(&path).map_err(|source| RelayError::MachineIdentity {
        path: path.clone(),
        source,
    })?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(RelayError::EmptyMachineIdentity(path))
    } else {
        Ok(value)
    }
}

/// Verifies that local carrier files contain every receiver tip.
pub fn verify_takeover_source(
    projects_root: &Path,
    session: &str,
    tips: &[StreamTip],
) -> Result<(), RelayError> {
    verified_takeover_streams(projects_root, session, tips, 0).map(drop)
}

/// Runs a read-only preflight, performs the CAS, then verifies and saves state.
pub fn takeover_session(
    projects_root: &Path,
    state_path: &Path,
    session: &str,
    machine: &str,
    force: bool,
    transport: &impl Transport,
) -> Result<TakeoverResult, TakeoverCommandError> {
    let snapshot = transport
        .lease_tips(session, machine)
        .map_err(TakeoverCommandError::Tips)?;
    verify_takeover_source(projects_root, session, &snapshot.tips)
        .map_err(TakeoverCommandError::Precheck)?;
    let takeover = transport
        .takeover(session, machine, snapshot.row.epoch, force)
        .map_err(TakeoverCommandError::Cas)?;
    if let Err(error) = initialize_takeover_state(projects_root, state_path, session, &takeover) {
        return match error {
            RelayError::TakeoverBehind {
                session,
                stream,
                behind,
                ..
            } => Err(TakeoverCommandError::PostCommitBehind {
                session,
                stream,
                behind,
                epoch: takeover.row.epoch,
            }),
            source => Err(TakeoverCommandError::PostCommit {
                epoch: takeover.row.epoch,
                source,
            }),
        };
    }
    Ok(takeover)
}

/// Adopts already verified receiver watermarks after a successful takeover.
pub fn initialize_takeover_state(
    projects_root: &Path,
    state_path: &Path,
    session: &str,
    takeover: &TakeoverResult,
) -> Result<(), RelayError> {
    let verified =
        verified_takeover_streams(projects_root, session, &takeover.tips, takeover.row.epoch)?;
    let mut state = load_state(state_path)?;
    state
        .streams
        .retain(|(known_session, _), _| known_session != session);
    state.streams.extend(verified);
    save_state(state_path, &state)?;
    Ok(())
}

fn verified_takeover_streams(
    projects_root: &Path,
    session: &str,
    receiver_tips: &[StreamTip],
    epoch: u64,
) -> Result<BTreeMap<(String, String), StreamState>, RelayError> {
    let streams = discover_streams(projects_root)?
        .into_iter()
        .filter(|stream| stream.session == session)
        .map(|stream| (stream.stream.clone(), stream))
        .collect::<BTreeMap<_, _>>();
    let mut tips = BTreeMap::<String, &StreamTip>::new();
    for tip in receiver_tips {
        if tips
            .get(&tip.stream)
            .is_none_or(|known| tip.generation > known.generation)
        {
            tips.insert(tip.stream.clone(), tip);
        }
    }
    for stream in tips.keys() {
        if !streams.contains_key(stream) {
            return Err(RelayError::TakeoverStreamMissing {
                session: session.to_owned(),
                stream: stream.clone(),
            });
        }
    }

    let mut verified = BTreeMap::new();
    for (name, stream) in streams {
        let mut file = File::open(&stream.path).map_err(|source| RelayError::Source {
            path: stream.path.clone(),
            source,
        })?;
        let metadata = file.metadata().map_err(|source| RelayError::Source {
            path: stream.path.clone(),
            source,
        })?;
        let identity = file_identity(&metadata);
        let mut entry = StreamState::new(stream.path.clone());
        entry.epoch = epoch;
        entry.device = identity.device;
        entry.inode = identity.inode;
        if let Some(tip) = tips.get(&name) {
            let scan = scan_prefix(&mut file, &stream.path, tip.seq)?;
            let Some(check) = scan.check else {
                let receiver_frames = tip.seq.saturating_add(1);
                return Err(RelayError::TakeoverBehind {
                    session: session.to_owned(),
                    stream: name,
                    local_frames: scan.complete_lines,
                    tip_seq: tip.seq,
                    behind: receiver_frames.saturating_sub(scan.complete_lines),
                });
            };
            if check.chain != tip.chain {
                return Err(RelayError::TakeoverPrefix {
                    session: session.to_owned(),
                    stream: name,
                    generation: tip.generation,
                    seq: tip.seq,
                });
            }
            entry.acked = Some(tip.seq);
            entry.chain = tip.chain;
            entry.generation = tip.generation;
            entry.offset = check.offset;
            entry.last_line_start = Some(check.last_line_start);
            entry.last_line_hash = Some(check.last_line_hash);
        }
        verified.insert((session.to_owned(), stream.stream), entry);
    }
    Ok(verified)
}

impl Sender {
    pub fn with_machine(machine: impl Into<String>) -> Self {
        Self {
            machine: Some(machine.into()),
            ..Self::default()
        }
    }

    pub fn run_pass(
        &mut self,
        projects_root: &Path,
        state_path: &Path,
        transport: &impl Transport,
    ) -> Result<PassReport, RelayError> {
        if self.machine.is_none() {
            self.machine = Some(read_machine_identity()?);
        }
        let machine = self.machine.clone().expect("machine identity was set");
        let pass_started = Instant::now();
        let pass_observation = Observation {
            instant: pass_started,
            wall_ns: wall_time_ns()?,
            mono_ns: monotonic_time_ns()?,
            boot_id: read_boot_id()?,
        };
        let discovered = discover_streams(projects_root)?;
        let mut streams = Vec::with_capacity(discovered.len());
        for stream in discovered {
            let metadata = fs::metadata(&stream.path).map_err(|source| RelayError::Source {
                path: stream.path.clone(),
                source,
            })?;
            streams.push((
                stream,
                StreamSnapshot {
                    size: metadata.len(),
                    identity: file_identity(&metadata),
                },
            ));
        }

        let live_streams = streams
            .iter()
            .map(|(stream, _)| (stream.session.clone(), stream.stream.clone()))
            .collect::<BTreeSet<_>>();
        self.startup_validated
            .retain(|key| live_streams.contains(key));
        self.observations.retain(|(session, stream, _), _| {
            live_streams.contains(&(session.clone(), stream.clone()))
        });

        let mut state = load_state(state_path)?;
        let sessions = streams
            .iter()
            .map(|(stream, _)| stream.session.clone())
            .collect::<BTreeSet<_>>();
        self.leases.retain(|session, _| sessions.contains(session));
        let decisions = sessions
            .into_iter()
            .map(|session| {
                let decision = self.ensure_session_lease(&session, &state, transport, &machine);
                (session, decision)
            })
            .collect::<BTreeMap<_, _>>();
        let mut reports = Vec::with_capacity(streams.len());
        let mut all_lags = LagAccumulator::default();
        for (stream, snapshot) in streams {
            let stream_key = (stream.session.clone(), stream.stream.clone());
            let require_full_check = !self.startup_validated.contains(&stream_key);
            let prepared = prepare_stream(
                &stream,
                &snapshot,
                &mut state,
                state_path,
                require_full_check,
            )?;
            self.startup_validated.insert(stream_key.clone());
            let decision = decisions
                .get(&stream.session)
                .expect("every discovered session has a lease decision");
            let mut fenced_now = None;
            {
                let entry = state
                    .streams
                    .get_mut(&stream_key)
                    .expect("prepare_stream inserted state");
                match decision {
                    LeaseDecision::Held(epoch) if !entry.fenced && entry.acked.is_none() => {
                        entry.epoch = *epoch;
                    }
                    LeaseDecision::Held(epoch) if !entry.fenced && entry.epoch < *epoch => {
                        entry.fenced = true;
                        fenced_now = Some(*epoch);
                    }
                    LeaseDecision::Fenced(current_epoch) if !entry.fenced => {
                        entry.fenced = true;
                        fenced_now = Some(*current_epoch);
                    }
                    _ => {}
                }
            }
            if let Some(current_epoch) = fenced_now {
                let entry = state
                    .streams
                    .get(&stream_key)
                    .expect("stream state still exists");
                eprintln!(
                    "semon-relay FENCED session={} stream={} old_epoch={} current_epoch={}; the old agent may still be running and its git side effects are not fenced",
                    stream.session, stream.stream, entry.epoch, current_epoch
                );
                save_state(state_path, &state)?;
            }
            let current_generation = state
                .streams
                .get(&stream_key)
                .expect("prepare_stream inserted state")
                .generation;
            self.observations.retain(|(session, name, generation), _| {
                session != &stream.session
                    || name != &stream.stream
                    || *generation == current_generation
            });
            let (report, lags) = match decision {
                LeaseDecision::Unavailable(failure) => {
                    let entry = state
                        .streams
                        .get(&stream_key)
                        .expect("prepare_stream inserted state");
                    (
                        StreamReport {
                            session: stream.session.clone(),
                            stream: stream.stream.clone(),
                            generation: entry.generation,
                            epoch: entry.epoch,
                            fenced: entry.fenced,
                            lines_acked: 0,
                            orphan_lines_acked: 0,
                            bytes_acked: 0,
                            backlog_lines: 0,
                            backlog_bytes: 0,
                            source_bytes_read: prepared.validation_bytes_read,
                            lag: LagSummary::default(),
                            failure: Some(failure.clone()),
                        },
                        LagAccumulator::default(),
                    )
                }
                LeaseDecision::Held(_) | LeaseDecision::Fenced(_) => send_stream(
                    &stream,
                    prepared,
                    &mut state,
                    SendContext {
                        state_path,
                        transport,
                        machine: &machine,
                        pass_observation: &pass_observation,
                        observations: &mut self.observations,
                        fenced_notice: fenced_now,
                    },
                )?,
            };
            reports.push(report);
            all_lags.merge(&lags);
        }
        Ok(PassReport {
            streams: reports,
            lag: all_lags.summary(),
            pass_duration: pass_started.elapsed(),
        })
    }

    fn ensure_session_lease(
        &mut self,
        session: &str,
        state: &RelayState,
        transport: &impl Transport,
        machine: &str,
    ) -> LeaseDecision {
        if let Some(held) = self.leases.get(session) {
            if held.renewed_at.elapsed() < Duration::from_millis(LEASE_RENEW_INTERVAL_MS) {
                return LeaseDecision::Held(held.epoch);
            }
            return match transport.renew(session, machine, held.epoch) {
                Ok(row) => {
                    self.leases.insert(
                        session.to_owned(),
                        HeldLease {
                            epoch: row.epoch,
                            renewed_at: Instant::now(),
                        },
                    );
                    LeaseDecision::Held(row.epoch)
                }
                Err(TransportError::Fenced { current_epoch })
                | Err(TransportError::NotHolder { current_epoch }) => {
                    self.leases.remove(session);
                    LeaseDecision::Fenced(current_epoch)
                }
                Err(error) => LeaseDecision::Unavailable(error.to_string()),
            };
        }

        let status = match transport.lease_status(Some(session), machine) {
            Ok(status) => status,
            Err(error) => return LeaseDecision::Unavailable(error.to_string()),
        };
        let row = match status.rows.into_iter().next() {
            Some(row) if row.holder_machine != machine => row,
            _ => match transport.acquire(session, machine) {
                Ok(row) => row,
                Err(error) => return LeaseDecision::Unavailable(error.to_string()),
            },
        };
        let local_is_stale = state.streams.iter().any(|((known_session, _), stream)| {
            known_session == session
                && (stream.fenced || (stream.acked.is_some() && stream.epoch < row.epoch))
        });
        if row.holder_machine != machine {
            return if local_is_stale {
                LeaseDecision::Fenced(row.epoch)
            } else {
                LeaseDecision::Unavailable(format!(
                    "lease is held by machine {} at epoch {}",
                    row.holder_machine, row.epoch
                ))
            };
        }
        if local_is_stale {
            return LeaseDecision::Fenced(row.epoch);
        }
        self.leases.insert(
            session.to_owned(),
            HeldLease {
                epoch: row.epoch,
                renewed_at: Instant::now(),
            },
        );
        LeaseDecision::Held(row.epoch)
    }
}

fn prepare_stream(
    stream: &DiscoveredStream,
    snapshot: &StreamSnapshot,
    state: &mut RelayState,
    state_path: &Path,
    require_full_check: bool,
) -> Result<PreparedStream, RelayError> {
    let key = (stream.session.clone(), stream.stream.clone());
    let original = state.streams.get(&key).cloned();
    let mut entry = original
        .clone()
        .unwrap_or_else(|| StreamState::new(stream.path.clone()));
    entry.path.clone_from(&stream.path);

    let mut file = File::open(&stream.path).map_err(|source| RelayError::Source {
        path: stream.path.clone(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| RelayError::Source {
        path: stream.path.clone(),
        source,
    })?;
    let identity = file_identity(&metadata);
    let observed_size = if identity == snapshot.identity {
        snapshot.size
    } else {
        // A replacement after the pass snapshot was not present at pass start.
        0
    };
    let mut validation_bytes_read = 0;

    if let Some(watermark) = entry.acked {
        let cheap_valid = if require_full_check {
            false
        } else {
            let (valid, bytes_read) =
                cheap_prefix_valid(&mut file, &stream.path, &metadata, identity, &entry)?;
            validation_bytes_read += bytes_read;
            valid
        };
        if !cheap_valid {
            let check = hash_prefix(&mut file, &stream.path, watermark)?;
            validation_bytes_read += check
                .as_ref()
                .map_or(metadata.len(), |item| item.bytes_read);
            if let Some(check) = check.filter(|check| check.chain == entry.chain) {
                entry.offset = check.offset;
                entry.last_line_start = Some(check.last_line_start);
                entry.last_line_hash = Some(check.last_line_hash);
                entry.device = identity.device;
                entry.inode = identity.inode;
            } else {
                reset_generation(stream, &mut entry, identity)?;
            }
        }
    } else {
        entry.chain = ZERO_CHAIN;
        entry.offset = 0;
        entry.device = identity.device;
        entry.inode = identity.inode;
        entry.last_line_start = None;
        entry.last_line_hash = None;
    }

    state.streams.insert(key, entry.clone());
    if original.as_ref() != Some(&entry) || !state_path.exists() {
        save_state(state_path, state)?;
    }
    Ok(PreparedStream {
        file,
        observed_size,
        validation_bytes_read,
    })
}

fn cheap_prefix_valid(
    file: &mut File,
    path: &Path,
    metadata: &Metadata,
    identity: FileIdentity,
    state: &StreamState,
) -> Result<(bool, u64), RelayError> {
    if identity.device != state.device
        || identity.inode != state.inode
        || metadata.len() < state.offset
    {
        return Ok((false, 0));
    }
    let (Some(start), Some(expected_hash)) = (state.last_line_start, state.last_line_hash) else {
        return Ok((false, 0));
    };
    let Some(length) = state.offset.checked_sub(start) else {
        return Ok((false, 0));
    };
    if length == 0 {
        return Ok((false, 0));
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|source| source_error(path, source))?;
    let mut line = Vec::with_capacity(length.min(1024 * 1024) as usize);
    file.take(length)
        .read_to_end(&mut line)
        .map_err(|source| source_error(path, source))?;
    let valid = line.len() as u64 == length
        && line.last() == Some(&b'\n')
        && line_hash(&line) == expected_hash;
    Ok((valid, line.len() as u64))
}

fn hash_prefix(
    file: &mut File,
    path: &Path,
    watermark: u64,
) -> Result<Option<PrefixCheck>, RelayError> {
    Ok(scan_prefix(file, path, watermark)?.check)
}

fn scan_prefix(file: &mut File, path: &Path, watermark: u64) -> Result<PrefixScan, RelayError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|source| source_error(path, source))?;
    let mut reader = BufReader::new(file);
    let mut chain = ZERO_CHAIN;
    let mut offset = 0_u64;
    let mut bytes_read = 0_u64;
    for seq in 0..=watermark {
        let start = offset;
        let line = match read_line(&mut reader, path)? {
            LineRead::Complete(line) => line,
            LineRead::Incomplete(_) | LineRead::End => {
                return Ok(PrefixScan {
                    check: None,
                    complete_lines: seq,
                });
            }
        };
        bytes_read += line.len() as u64;
        offset += line.len() as u64;
        chain = chain_line(&chain, &line);
        if seq == watermark {
            return Ok(PrefixScan {
                check: Some(PrefixCheck {
                    chain,
                    offset,
                    last_line_start: start,
                    last_line_hash: line_hash(&line),
                    bytes_read,
                }),
                complete_lines: seq.saturating_add(1),
            });
        }
    }
    Ok(PrefixScan {
        check: None,
        complete_lines: 0,
    })
}

fn reset_generation(
    stream: &DiscoveredStream,
    state: &mut StreamState,
    identity: FileIdentity,
) -> Result<(), RelayError> {
    state.generation =
        state
            .generation
            .checked_add(1)
            .ok_or_else(|| RelayError::GenerationOverflow {
                session: stream.session.clone(),
                stream: stream.stream.clone(),
            })?;
    state.acked = None;
    state.chain = ZERO_CHAIN;
    state.offset = 0;
    state.device = identity.device;
    state.inode = identity.inode;
    state.last_line_start = None;
    state.last_line_hash = None;
    state.orphaned = None;
    Ok(())
}

fn send_stream<T: Transport>(
    stream: &DiscoveredStream,
    prepared: PreparedStream,
    state: &mut RelayState,
    context: SendContext<'_, T>,
) -> Result<(StreamReport, LagAccumulator), RelayError> {
    let SendContext {
        state_path,
        transport,
        machine,
        pass_observation,
        observations,
        fenced_notice,
    } = context;
    let key = (stream.session.clone(), stream.stream.clone());
    let mut entry = state
        .streams
        .get(&key)
        .cloned()
        .expect("prepare_stream inserted state");
    let starting_state = entry.clone();
    let observation_key = (
        stream.session.clone(),
        stream.stream.clone(),
        entry.generation,
    );
    let mut file = prepared.file;
    file.seek(SeekFrom::Start(entry.offset))
        .map_err(|source| source_error(&stream.path, source))?;
    let mut reader = BufReader::new(file);
    let mut seq = entry
        .acked
        .map_or(0, |watermark| watermark.saturating_add(1));
    let mut chain = entry.chain;
    let mut position = entry.offset;
    let mut delivering = !entry.fenced;
    let mut orphaning = entry.fenced;
    let mut lines_acked = 0_u64;
    let mut orphan_lines_acked = 0_u64;
    let mut bytes_acked = 0_u64;
    let mut backlog_lines = 0_u64;
    let mut backlog_bytes = 0_u64;
    let mut source_bytes_read = prepared.validation_bytes_read;
    let mut lags = LagAccumulator::default();
    let mut failure = fenced_notice.map(|current_epoch| {
        format!(
            "sender fenced at epoch {}; current epoch is {current_epoch}; tail is being retained as orphan data",
            entry.epoch
        )
    });

    loop {
        let line_start = position;
        let line = match read_line(&mut reader, &stream.path)? {
            LineRead::Complete(line) => line,
            LineRead::Incomplete(count) => {
                source_bytes_read += count;
                break;
            }
            LineRead::End => break,
        };
        source_bytes_read += line.len() as u64;
        position += line.len() as u64;
        chain = chain_line(&chain, &line);

        let observed = if let Some(pending) =
            observations.get(&observation_key).and_then(|windows| {
                windows
                    .iter()
                    .find(|window| position <= window.through_offset)
            }) {
            pending.observation.clone()
        } else if position <= prepared.observed_size {
            pass_observation.clone()
        } else {
            Observation::now(&pass_observation.boot_id)?
        };
        let frame = Frame {
            key: FrameKey {
                session: stream.session.clone(),
                stream: stream.stream.clone(),
                generation: entry.generation,
                epoch: entry.epoch,
                seq,
            },
            machine: machine.to_owned(),
            chain,
            sender_wall_ns: observed.wall_ns,
            sender_mono_ns: observed.mono_ns,
            boot_id: observed.boot_id.clone(),
            line: line.clone(),
        };

        if delivering {
            match transport.send(&frame) {
                Ok(acked) if acked == seq => {
                    let acknowledged_after = observed.instant.elapsed();
                    entry.acked = Some(seq);
                    entry.chain = chain;
                    entry.offset = position;
                    entry.last_line_start = Some(line_start);
                    entry.last_line_hash = Some(line_hash(&line));
                    lines_acked += 1;
                    bytes_acked += line.len() as u64;
                    lags.record(acknowledged_after);
                    clear_acknowledged_observations(observations, &observation_key, entry.offset);
                }
                Ok(acked) => {
                    remember_failed_pass(
                        observations,
                        observation_key.clone(),
                        entry.offset,
                        prepared.observed_size,
                        pass_observation,
                        position,
                        &observed,
                    );
                    delivering = false;
                    failure = Some(format!(
                        "receiver acknowledged sequence {acked}, expected {seq}"
                    ));
                }
                Err(TransportError::Fenced { current_epoch })
                | Err(TransportError::NotHolder { current_epoch }) => {
                    entry.fenced = true;
                    delivering = false;
                    orphaning = true;
                    eprintln!(
                        "semon-relay FENCED session={} stream={} old_epoch={} current_epoch={}; the old agent may still be running and its git side effects are not fenced",
                        stream.session, stream.stream, entry.epoch, current_epoch
                    );
                    failure = Some(format!(
                        "sender fenced at epoch {}; current epoch is {current_epoch}; tail is being retained as orphan data",
                        entry.epoch
                    ));
                }
                Err(error) => {
                    remember_failed_pass(
                        observations,
                        observation_key.clone(),
                        entry.offset,
                        prepared.observed_size,
                        pass_observation,
                        position,
                        &observed,
                    );
                    delivering = false;
                    failure = Some(error.to_string());
                }
            }
        }
        if orphaning && entry.orphaned.is_none_or(|orphaned| seq > orphaned) {
            match transport.send_orphan(&frame) {
                Ok(acked) if acked == seq => {
                    entry.orphaned = Some(seq);
                    orphan_lines_acked += 1;
                }
                Ok(acked) => {
                    orphaning = false;
                    failure = Some(format!(
                        "orphan receiver acknowledged sequence {acked}, expected {seq}"
                    ));
                }
                Err(error) => {
                    orphaning = false;
                    failure = Some(format!("orphan upload failed: {error}"));
                }
            }
        }
        if position > entry.offset {
            backlog_lines += 1;
            backlog_bytes += line.len() as u64;
        }
        seq = seq.saturating_add(1);
    }

    state.streams.insert(key, entry.clone());
    // A crash before this save merely causes already-stored frames to be
    // retransmitted. Receiver idempotence makes batching safe, while avoiding
    // a state-file fsync for every line in a historical backlog.
    if entry != starting_state {
        save_state(state_path, state)?;
    }

    let report = StreamReport {
        session: stream.session.clone(),
        stream: stream.stream.clone(),
        generation: entry.generation,
        epoch: entry.epoch,
        fenced: entry.fenced,
        lines_acked,
        orphan_lines_acked,
        bytes_acked,
        backlog_lines,
        backlog_bytes,
        source_bytes_read,
        lag: lags.summary(),
        failure,
    };
    Ok((report, lags))
}

fn remember_failed_pass(
    observations: &mut BTreeMap<ObservationKey, VecDeque<ObservationWindow>>,
    key: ObservationKey,
    acknowledged_offset: u64,
    observed_size: u64,
    pass_observation: &Observation,
    failed_line_end: u64,
    failed_observation: &Observation,
) {
    remember_observation_through(
        observations,
        key.clone(),
        acknowledged_offset,
        observed_size,
        pass_observation,
    );
    // A line can become complete after the pass snapshot. Its own observation
    // still has to survive a failed attempt even though it is beyond that size.
    remember_observation_through(
        observations,
        key,
        acknowledged_offset,
        failed_line_end,
        failed_observation,
    );
}

fn remember_observation_through(
    observations: &mut BTreeMap<ObservationKey, VecDeque<ObservationWindow>>,
    key: ObservationKey,
    acknowledged_offset: u64,
    through_offset: u64,
    observation: &Observation,
) {
    if through_offset <= acknowledged_offset {
        return;
    }
    let windows = observations.entry(key).or_default();
    if windows
        .back()
        .is_none_or(|window| window.through_offset < through_offset)
    {
        windows.push_back(ObservationWindow {
            through_offset,
            observation: observation.clone(),
        });
    }
}

fn clear_acknowledged_observations(
    observations: &mut BTreeMap<ObservationKey, VecDeque<ObservationWindow>>,
    key: &ObservationKey,
    acknowledged_offset: u64,
) {
    let empty = observations.get_mut(key).is_some_and(|windows| {
        while windows
            .front()
            .is_some_and(|window| window.through_offset <= acknowledged_offset)
        {
            windows.pop_front();
        }
        windows.is_empty()
    });
    if empty {
        observations.remove(key);
    }
}

impl Observation {
    fn now(boot_id: &str) -> Result<Self, RelayError> {
        Ok(Self {
            instant: Instant::now(),
            wall_ns: wall_time_ns()?,
            mono_ns: monotonic_time_ns()?,
            boot_id: boot_id.to_owned(),
        })
    }
}

enum LineRead {
    Complete(Vec<u8>),
    Incomplete(u64),
    End,
}

fn read_line(reader: &mut impl BufRead, path: &Path) -> Result<LineRead, RelayError> {
    let mut line = Vec::new();
    let read = reader
        .read_until(b'\n', &mut line)
        .map_err(|source| source_error(path, source))?;
    if read == 0 {
        Ok(LineRead::End)
    } else if line.last() == Some(&b'\n') {
        Ok(LineRead::Complete(line))
    } else {
        Ok(LineRead::Incomplete(line.len() as u64))
    }
}

#[cfg(test)]
fn read_complete_line(
    reader: &mut impl BufRead,
    path: &Path,
) -> Result<Option<Vec<u8>>, RelayError> {
    match read_line(reader, path)? {
        LineRead::Complete(line) => Ok(Some(line)),
        LineRead::Incomplete(_) | LineRead::End => Ok(None),
    }
}

fn source_error(path: &Path, source: io::Error) -> RelayError {
    RelayError::Source {
        path: path.to_path_buf(),
        source,
    }
}

fn file_identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn line_hash(line: &[u8]) -> [u8; 32] {
    Sha256::digest(line).into()
}

struct LagAccumulator {
    // Logarithmic nanosecond buckets keep measurement memory constant even
    // when a pass drains an arbitrarily large on-disk backlog.
    buckets: [u64; 65],
    samples: u64,
    max: Duration,
}

impl Default for LagAccumulator {
    fn default() -> Self {
        Self {
            buckets: [0; 65],
            samples: 0,
            max: Duration::ZERO,
        }
    }
}

impl LagAccumulator {
    fn record(&mut self, duration: Duration) {
        let nanos = duration.as_nanos().min(u64::MAX as u128) as u64;
        let bucket = if nanos == 0 {
            0
        } else {
            1 + (63 - nanos.leading_zeros()) as usize
        };
        self.buckets[bucket] += 1;
        self.samples += 1;
        self.max = self.max.max(duration);
    }

    fn summary(&self) -> LagSummary {
        if self.samples == 0 {
            return LagSummary::default();
        }
        LagSummary {
            samples: self.samples,
            p50: self.percentile(50).min(self.max),
            p95: self.percentile(95).min(self.max),
            max: self.max,
        }
    }

    fn merge(&mut self, other: &Self) {
        for (target, source) in self.buckets.iter_mut().zip(other.buckets) {
            *target += source;
        }
        self.samples += other.samples;
        self.max = self.max.max(other.max);
    }

    fn percentile(&self, percentile: u64) -> Duration {
        let rank = (self.samples * percentile).div_ceil(100);
        let mut seen = 0_u64;
        for (index, count) in self.buckets.iter().enumerate() {
            seen += count;
            if seen >= rank {
                let nanos = match index {
                    0 => 0,
                    64 => u64::MAX,
                    _ => (1_u64 << index) - 1,
                };
                return Duration::from_nanos(nanos);
            }
        }
        self.max
    }
}

fn wall_time_ns() -> Result<u64, RelayError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RelayError::WallClock)?
        .as_nanos();
    Ok(nanos.min(u64::MAX as u128) as u64)
}

fn read_boot_id() -> Result<String, RelayError> {
    let value =
        fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(RelayError::BootId)?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(RelayError::EmptyBootId)
    } else {
        Ok(value)
    }
}

fn monotonic_time_ns() -> Result<u64, RelayError> {
    // Linux exposes CLOCK_BOOTTIME-compatible uptime without another crate.
    let value = fs::read_to_string("/proc/uptime").map_err(RelayError::Monotonic)?;
    let seconds = value
        .split_whitespace()
        .next()
        .ok_or_else(|| RelayError::InvalidMonotonic(value.clone()))?;
    decimal_seconds_to_ns(seconds).ok_or(RelayError::InvalidMonotonic(value))
}

fn decimal_seconds_to_ns(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = whole.parse::<u64>().ok()?;
    let mut fraction = fraction.bytes().take(9).collect::<Vec<_>>();
    if !fraction.iter().all(u8::is_ascii_digit) {
        return None;
    }
    fraction.resize(9, b'0');
    let fraction = std::str::from_utf8(&fraction).ok()?.parse::<u64>().ok()?;
    whole.checked_mul(1_000_000_000)?.checked_add(fraction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Read};

    #[test]
    fn parses_proc_uptime_without_floating_point() {
        assert_eq!(decimal_seconds_to_ns("12.34"), Some(12_340_000_000));
        assert_eq!(decimal_seconds_to_ns("0.000000001"), Some(1));
        assert_eq!(decimal_seconds_to_ns("broken"), None);
    }

    struct TwoBytesAtATime<'a>(&'a [u8]);

    impl Read for TwoBytesAtATime<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.0.len().min(buffer.len()).min(2);
            buffer[..count].copy_from_slice(&self.0[..count]);
            self.0 = &self.0[count..];
            Ok(count)
        }
    }

    #[test]
    fn byte_framing_handles_utf8_split_across_reads() {
        let bytes = "é界\npartial".as_bytes();
        let mut reader = BufReader::with_capacity(2, TwoBytesAtATime(bytes));
        assert_eq!(
            read_complete_line(&mut reader, Path::new("synthetic")).unwrap(),
            Some("é界\n".as_bytes().to_vec())
        );
        assert_eq!(
            read_complete_line(&mut reader, Path::new("synthetic")).unwrap(),
            None
        );
    }

    #[test]
    fn histogram_percentiles_do_not_exceed_the_exact_maximum() {
        let mut lags = LagAccumulator::default();
        lags.record(Duration::from_millis(5));
        let summary = lags.summary();
        assert_eq!(summary.p50, Duration::from_millis(5));
        assert_eq!(summary.p95, Duration::from_millis(5));
        assert_eq!(summary.max, Duration::from_millis(5));
    }
}

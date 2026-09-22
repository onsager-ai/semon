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
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    DiscoveredStream, Frame, FrameKey, RelayState, StreamState, ZERO_CHAIN, chain_line,
    discover_streams, discovery::DiscoveryError, load_state, save_state, state::StateError,
};

type StreamKey = (String, String);
type ObservationKey = (String, String, u64);

/// A deliberately single-frame seam: the source file, not memory, is the queue.
pub trait Transport {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError>;
}

/// The blocking HTTP transport used by the command-line sender.
pub struct HttpTransport {
    client: Client,
    endpoint: String,
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
            endpoint,
        })
    }
}

impl Transport for HttpTransport {
    fn send(&self, frame: &Frame) -> Result<u64, TransportError> {
        let response = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&frame.to_value())?)
            .send()?;
        let status = response.status();
        let mut body = response.text().unwrap_or_default();
        body.truncate(1024);
        if !status.is_success() {
            return Err(TransportError::Rejected {
                status,
                body: body.trim().to_owned(),
            });
        }
        let value: Value = serde_json::from_str(&body)?;
        value
            .get("acked")
            .and_then(Value::as_u64)
            .ok_or_else(|| TransportError::InvalidAck(body))
    }
}

/// A delivery failure. Callers retain the source bytes and retry later.
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("receiver request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("cannot encode or decode receiver JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("receiver answered {status}: {body}")]
    Rejected { status: StatusCode, body: String },
    #[error("receiver returned an invalid acknowledgement: {0}")]
    InvalidAck(String),
    #[error("receiver unavailable: {0}")]
    Unavailable(String),
    #[error("invalid receiver endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("M1 receiver endpoint must be an HTTP loopback IP URL: {0}")]
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
    pub lines_acked: u64,
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

impl Sender {
    pub fn run_pass(
        &mut self,
        projects_root: &Path,
        state_path: &Path,
        transport: &impl Transport,
    ) -> Result<PassReport, RelayError> {
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
            let (report, lags) = send_stream(
                &stream,
                prepared,
                &mut state,
                state_path,
                transport,
                &pass_observation,
                &mut self.observations,
            )?;
            reports.push(report);
            all_lags.merge(&lags);
        }
        Ok(PassReport {
            streams: reports,
            lag: all_lags.summary(),
            pass_duration: pass_started.elapsed(),
        })
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
            LineRead::Incomplete(_) => return Ok(None),
            LineRead::End => return Ok(None),
        };
        bytes_read += line.len() as u64;
        offset += line.len() as u64;
        chain = chain_line(&chain, &line);
        if seq == watermark {
            return Ok(Some(PrefixCheck {
                chain,
                offset,
                last_line_start: start,
                last_line_hash: line_hash(&line),
                bytes_read,
            }));
        }
    }
    Ok(None)
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
    Ok(())
}

fn send_stream(
    stream: &DiscoveredStream,
    prepared: PreparedStream,
    state: &mut RelayState,
    state_path: &Path,
    transport: &impl Transport,
    pass_observation: &Observation,
    observations: &mut BTreeMap<ObservationKey, VecDeque<ObservationWindow>>,
) -> Result<(StreamReport, LagAccumulator), RelayError> {
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
    let mut delivering = true;
    let mut lines_acked = 0_u64;
    let mut bytes_acked = 0_u64;
    let mut backlog_lines = 0_u64;
    let mut backlog_bytes = 0_u64;
    let mut source_bytes_read = prepared.validation_bytes_read;
    let mut lags = LagAccumulator::default();
    let mut failure = None;

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

        if delivering {
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
                    epoch: 0,
                    seq,
                },
                chain,
                sender_wall_ns: observed.wall_ns,
                sender_mono_ns: observed.mono_ns,
                boot_id: observed.boot_id.clone(),
                line: line.clone(),
            };
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
        lines_acked,
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

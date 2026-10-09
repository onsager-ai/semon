//! `semon push`: a client of the mirror protocol (docs/mirror-protocol.md).
//! It keeps a receiver's copy of this machine's session logs in step with
//! the originals, appending as they grow, and sends the machine's facts.
//!
//! - Only the builder's input files are read (`semon_sessions::inputs`).
//! - Every byte sent is redacted first ([`redact`]).
//! - A JSONL log is sent in complete lines only; its last, incomplete line
//!   waits until it is complete.
//! - Where each file stands is kept in a private state file, so an
//!   interrupted push resumes. One push at a time holds it ([`StateLock`]).
//! - A push, `--watch` included, ends cleanly on a [`Stop`], and its token
//!   can come from memory ([`Credential::Memory`]), so an app can run it
//!   in-process.
//!
//! The receiving side is here too, as the protocol's reference
//! implementation: [`mirror::Receiver`] over a directory, its tokens
//! ([`tokens`]) and `semon receive`'s listener ([`serve`]).

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt, fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use semon_sessions::{Facts, FactsSource, Input, Options};
use serde::{Deserialize, Serialize};

mod lock;
pub mod mirror;
pub mod redact;
pub mod serve;
mod status;
mod stop;
pub mod tokens;
pub mod wire;

use lock::Hold;
pub use lock::StateLock;
pub use stop::Stop;

use wire::{Append, CHUNK_BYTES, HEAD_BYTES, Length, base64_encode, head_sha256, sha256_hex};

/// A line longer than this is redacted and sent in pieces of this size.
const LINE_CAP: usize = 64 * 1024 * 1024;
/// Whole-file inputs (`*.json`) larger than this are skipped.
const WHOLE_CAP: u64 = 4 * 1024 * 1024;
/// How often `--watch` looks for new lines, and sends facts.
const PASS_EVERY: Duration = Duration::from_secs(2);
const FACTS_EVERY: Duration = Duration::from_secs(10);

#[cfg(test)]
thread_local! {
    static INPUT_OPENS: std::cell::RefCell<BTreeMap<PathBuf, usize>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct PushOptions {
    /// The receiver's base URL; requests go to `<url>/v1/mirror/…`.
    pub url: String,
    pub credential: Credential,
    pub sessions: Options,
    /// Where to keep the cursor state (0600). A push holds the lock beside
    /// it ([`StateLock`]) while it runs.
    pub state: PathBuf,
}

/// Where a push's bearer token comes from.
#[derive(Clone, Debug)]
pub enum Credential {
    /// A 0600 file, read once when the push starts ([`read_token`]).
    File(PathBuf),
    /// A token an embedding app holds in memory, from its OS credential
    /// store for instance. It is never written to disk.
    Memory(Token),
}

impl Credential {
    fn token(&self) -> Result<Token> {
        match self {
            Self::File(path) => read_token(path).map(Token),
            Self::Memory(token) => Ok(token.clone()),
        }
    }
}

/// A push's bearer token. It leaves this crate only in the `Authorization`
/// header of a request to the receiver: it has no `Display`, its `Debug`
/// shows `Token(<redacted>)`, and nothing here writes it to a file or a log.
/// Its memory is not zeroised when dropped, and the HTTP client keeps its
/// own copy of the header while a request runs.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// A token as a token file holds it: surrounding whitespace is trimmed,
    /// and an empty token, or one with a space or a control character, is
    /// refused.
    pub fn new(token: &str) -> Result<Self> {
        let token = token.trim();
        if usable_token(token) {
            Ok(Self(token.to_owned()))
        } else {
            Err("the token is empty, or holds a space or a control character".to_owned())
        }
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

fn usable_token(token: &str) -> bool {
    !token.is_empty()
        && !token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
}

/// The default state file for a receiver URL, under the XDG state
/// directory.
pub fn default_state_path(url: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"));
    // Keyed by the URL as a push uses it ([`check_url`]), so spellings of
    // one receiver share one state file and one lock.
    let key = check_url(url).unwrap_or_else(|_| url.to_owned());
    base.join("semon/push")
        .join(format!("{}.json", &sha256_hex(key.as_bytes())[..16]))
}

/// Reads a bearer token from `path`, refusing a file that anyone but its
/// owner can read or write.
pub fn read_token(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!(
                "{} is mode {mode:03o}; the token file must be 0600 (chmod 600 it)",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    let token = fs::read_to_string(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .trim()
        .to_owned();
    if !usable_token(&token) {
        return Err(format!("{} holds no usable token", path.display()));
    }
    Ok(token)
}

/// Checks the receiver URL: https, or http on a loopback address only, and
/// no query or fragment. Returns it normalised: the scheme and host in
/// lower case, and no trailing slash.
pub fn check_url(url: &str) -> Result<String> {
    let url = url.trim_end_matches('/');
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("{url} is not a URL"))?;
    let scheme = scheme.to_ascii_lowercase();
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let path = &rest[host.len()..];
    let host = host.to_ascii_lowercase();
    let host = host.as_str();
    let normal = format!("{scheme}://{host}{path}");
    let hostname = host
        .rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
        .map_or(host, |(name, _)| name);
    if host.is_empty() || host.contains('@') {
        return Err(format!("{url}: no host, or credentials in the URL"));
    }
    if url.contains(['?', '#']) {
        return Err(format!("{url}: no query or fragment allowed"));
    }
    match scheme.as_str() {
        "https" => Ok(normal),
        "http" if loopback(hostname) => Ok(normal),
        "http" => Err(format!(
            "{url}: plain http is allowed to a loopback address only"
        )),
        _ => Err(format!("{url}: the URL must be https")),
    }
}

/// `localhost`, `[::1]` or any address in 127.0.0.0/8.
fn loopback(host: &str) -> bool {
    host == "localhost"
        || host == "[::1]"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct FileState {
    /// Bytes of the original acknowledged by the receiver.
    sent: u64,
    #[serde(default)]
    generation: String,
    #[serde(default)]
    confirmed: bool,
    #[serde(default)]
    redacted_head: String,
    /// SHA-256 of the original's first `min(4096, sent)` bytes: a change
    /// means the file was rewritten, and is sent again whole.
    raw_head: String,
    #[serde(default)]
    len: u64,
    #[serde(default)]
    mtime_ns: Option<i128>,
    #[serde(default)]
    dev: u64,
    #[serde(default)]
    ino: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FileStat {
    len: u64,
    mtime_ns: Option<i128>,
    dev: u64,
    ino: u64,
}

impl FileStat {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        let (dev, ino) = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let (dev, ino) = (0, 0);

        Self {
            len: metadata.len(),
            mtime_ns: metadata_mtime_ns(metadata),
            dev,
            ino,
        }
    }
}

impl FileState {
    fn has_stat(&self) -> bool {
        self.mtime_ns.is_some()
    }

    fn matches_stat(&self, stat: FileStat) -> bool {
        self.len == stat.len
            && self.mtime_ns == stat.mtime_ns
            && self.dev == stat.dev
            && self.ino == stat.ino
    }

    fn set_stat(&mut self, stat: FileStat) {
        self.len = stat.len;
        self.mtime_ns = stat.mtime_ns;
        self.dev = stat.dev;
        self.ino = stat.ino;
    }
}

#[cfg(unix)]
fn metadata_mtime_ns(metadata: &fs::Metadata) -> Option<i128> {
    use std::os::unix::fs::MetadataExt;
    Some(i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()))
}

#[cfg(not(unix))]
fn metadata_mtime_ns(metadata: &fs::Metadata) -> Option<i128> {
    use std::time::UNIX_EPOCH;
    let Ok(modified) = metadata.modified() else {
        return None;
    };
    match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => Some(i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX)),
        Err(error) => Some(-i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX)),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    version: u32,
    url: String,
    files: BTreeMap<String, FileState>,
}

/// What one pass sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub bytes: u64,
    pub replaced: usize,
}

enum Answer {
    Ok(Length),
    Conflict(Length),
}

/// Learned only from successful append ACKs, and reset on each Client.
#[repr(u8)]
enum GenerationSupport {
    Unknown,
    Legacy,
    Supported,
}

pub struct Client {
    url: String,
    token: Token,
    http: reqwest::blocking::Client,
    stop: Stop,
    lock: Option<Arc<StateLock>>,
    state_path: PathBuf,
    state: State,
    chunk: usize,
    heads: BTreeMap<String, Vec<u8>>,
    generation_support: AtomicU8,
    progress: Arc<Mutex<status::Progress>>,
    completed_inventory: Option<BTreeSet<String>>,
    committed_inventory: Option<BTreeSet<String>>,
}

impl Client {
    pub fn new(options: &PushOptions) -> Result<Self> {
        let url = check_url(&options.url)?;
        let token = options.credential.token()?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("semon-push/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| error.to_string())?;
        let state = fs::read(&options.state)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<State>(&bytes).ok())
            .filter(|state| state.version == 1 && state.url == url)
            .unwrap_or(State {
                version: 1,
                url: url.clone(),
                files: BTreeMap::new(),
            });
        Ok(Self {
            url,
            token,
            http,
            stop: Stop::new(),
            lock: None,
            state_path: options.state.clone(),
            state,
            chunk: CHUNK_BYTES,
            heads: BTreeMap::new(),
            generation_support: AtomicU8::new(GenerationSupport::Unknown as u8),
            progress: Arc::new(Mutex::new(status::Progress::new(wire::new_generation()?))),
            completed_inventory: None,
            committed_inventory: None,
        })
    }

    /// Smaller chunks, for tests.
    #[doc(hidden)]
    pub fn with_chunk(mut self, chunk: usize) -> Self {
        self.chunk = chunk.max(1);
        self
    }

    /// Ends passes and requests early once `stop` is stopped.
    pub fn with_stop(mut self, stop: Stop) -> Self {
        self.stop = stop;
        self
    }

    /// Holds `lock` from each request's own thread too, so a request a stop
    /// leaves running keeps the state locked until it ends.
    pub(crate) fn with_lock(mut self, lock: Arc<StateLock>) -> Self {
        self.lock = Some(lock);
        self
    }

    /// One request to the receiver, made on a thread of its own so that a
    /// stop doesn't wait for it (up to the 120 s timeout). A request a stop
    /// leaves behind finishes there, holding the state lock (see
    /// [`StateLock`]), and its answer is dropped.
    fn post(&self, route: &str, body: String) -> std::result::Result<(u16, String), Failure> {
        self.post_wait(route, body, |delay| self.stop.sleep(delay))
    }

    fn post_wait(
        &self,
        route: &str,
        body: String,
        mut wait: impl FnMut(Duration) -> bool,
    ) -> std::result::Result<(u16, String), Failure> {
        let mut retries = 0;
        loop {
            let request = self
                .http
                .post(format!("{}/v1/mirror/{route}", self.url))
                .bearer_auth(self.token.expose())
                .header("content-type", "application/json")
                .body(body.clone());
            let lock = self.lock.clone();
            let answer = self.stop.run(route, move || {
                let _lock = lock;
                request.send().map(|response| {
                    let status = response.status().as_u16();
                    let retry = response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| {
                            value.parse::<u64>().ok().or_else(|| {
                                httpdate::parse_http_date(value).ok().map(|date| {
                                    date.duration_since(std::time::SystemTime::now())
                                        .unwrap_or_default()
                                        .as_secs()
                                })
                            })
                        })
                        .unwrap_or(2);
                    (status, response.text().unwrap_or_default(), retry)
                })
            });
            let (status, text, retry) = match answer {
                Ok(Some(Ok(answer))) => answer,
                Ok(Some(Err(error))) => return Err(Failure::Remote(format!("{route}: {error}"))),
                Ok(None) => return Err(Failure::Stopped),
                Err(error) => return Err(Failure::Remote(error)),
            };
            if route == "append" && status == 429 {
                self.progress
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .phase = wire::SyncPhase::WaitingToRetry;
            }
            if status == 429 && error_kind(&text).as_deref() == Some("rate_limited") {
                retries += 1;
                if retries == 2 {
                    eprintln!("semon push: rate limited; retrying after {retry} seconds");
                }
                if wait(Duration::from_secs(retry)) {
                    return Err(Failure::Stopped);
                }
                continue;
            }
            return match status {
                401 | 403 => Err(Failure::Remote(format!(
                    "{route}: the receiver refused the token ({status})"
                ))),
                413 if error_kind(&text).as_deref() == Some("storage_limit") => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                    Err(Failure::Remote(format!(
                        "storage is full (used {} of {}); nothing more is sent until space is freed; nothing already sent was deleted",
                        value["used"], value["limit"]
                    )))
                }
                423 if error_kind(&text).as_deref() == Some("paused") => Err(Failure::Remote(
                    "pushes are paused by the owner; they resume when the owner resumes them"
                        .into(),
                )),
                _ => Ok((status, text)),
            };
        }
    }

    /// Sends the machine's facts.
    pub fn send_facts(&self, facts: &Facts) -> Result<()> {
        self.post_facts(facts).map_err(Failure::message)
    }

    fn post_facts(&self, facts: &Facts) -> std::result::Result<(), Failure> {
        let mut prepared = facts.clone();
        prepared.codex_provisional_rollouts = None;
        {
            // Progress snapshot and facts fence share the status allocator.
            let mut context = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            context.sequence += 1;
            prepared.mirror_observation_id = Some(context.observation_id.clone());
            prepared.mirror_sequence = Some(context.sequence);
        }
        let body =
            serde_json::to_string(&prepared).map_err(|error| Failure::Remote(error.to_string()))?;
        match self.post("facts", body)? {
            (200..=299, _) => Ok(()),
            (status, text) => Err(Failure::Remote(format!("facts: {status} {}", text.trim()))),
        }
    }

    fn append(&self, append: &Append) -> std::result::Result<Answer, Failure> {
        let body =
            serde_json::to_string(append).map_err(|error| Failure::Remote(error.to_string()))?;
        let (status, text) = self.post("append", body)?;
        let parsed = || {
            serde_json::from_str::<Length>(&text)
                .map_err(|_| Failure::Remote(format!("append: {status} with an unreadable body")))
        };
        match status {
            200..=299 => {
                let ack = parsed()?;
                let supported = append
                    .generation
                    .as_deref()
                    .is_some_and(|generation| ack.generation.as_deref() == Some(generation));
                self.generation_support.store(
                    if supported {
                        GenerationSupport::Supported
                    } else {
                        GenerationSupport::Legacy
                    } as u8,
                    Ordering::Relaxed,
                );
                Ok(Answer::Ok(ack))
            }
            409 => Ok(Answer::Conflict(parsed()?)),
            _ => Err(Failure::Remote(format!(
                "append {}/{}: {status} {}",
                append.root,
                append.path,
                text.trim()
            ))),
        }
    }

    fn save(&self) -> Result<()> {
        write_private(
            &self.state_path,
            &serde_json::to_vec(&self.state).map_err(|e| e.to_string())?,
        )
        .map_err(|error| format!("{}: {error}", self.state_path.display()))
    }

    /// Brings the receiver up to date with every input file once. A file
    /// that can't be read now is reported and tried again next pass; any
    /// error from the receiver ends the pass. A stop ends it too, as `Ok`
    /// with what was sent before it: a file it caught midway keeps its last
    /// recorded place, and the rest wait for the next push.
    pub fn pass(&mut self, sessions: &Options) -> Result<Report> {
        self.completed_inventory = None;
        {
            let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            progress.clear_targets();
            progress.inventory_complete = false;
            progress.phase = wire::SyncPhase::Syncing;
        }
        let mut report = Report::default();
        let mut touched = BTreeSet::new();
        let mut selected = BTreeMap::<String, (Input, FileStat, status::TargetRank)>::new();
        let mut foreground = VecDeque::new();
        let mut history = VecDeque::new();
        let mut foreground_turns = 0;
        let mut local_failure = false;
        self.discover(sessions, &mut selected, &mut foreground, &mut history)?;
        let mut discovered = Instant::now();
        loop {
            if self.stop.is_stopped() {
                break;
            }
            if discovered.elapsed() >= PASS_EVERY {
                self.discover(sessions, &mut selected, &mut foreground, &mut history)?;
                discovered = Instant::now();
            }
            if foreground.is_empty() && history.is_empty() {
                self.discover(sessions, &mut selected, &mut foreground, &mut history)?;
                if foreground.is_empty() && history.is_empty() {
                    break;
                }
                discovered = Instant::now();
            }
            let use_foreground =
                !foreground.is_empty() && (history.is_empty() || foreground_turns < 3);
            let key = if use_foreground {
                foreground_turns += 1;
                foreground.pop_front().expect("foreground")
            } else {
                foreground_turns = 0;
                history.pop_front().expect("history")
            };
            let (input, target, rank) = selected.get(&key).expect("selected");
            let path = input.full_path(sessions);
            let log = input.path.ends_with(".jsonl");
            let outcome = if log {
                self.sync_log(input, &path, target.len)
            } else {
                self.sync_whole(input, &path)
            };
            match outcome {
                Ok((bytes, replaced)) => {
                    if bytes > 0 || replaced {
                        touched.insert(key.clone());
                        report.bytes += bytes;
                        report.replaced += usize::from(replaced);
                    }
                    self.update_target(input, target.len, *rank, None);
                    // A scheduling turn is one complete redaction batch. A
                    // split long line remains one indivisible checkpoint.
                    if log
                        && bytes > 0
                        && self
                            .state
                            .files
                            .get(&key)
                            .is_some_and(|f| f.sent < target.len)
                    {
                        if use_foreground {
                            foreground.push_back(key);
                        } else {
                            history.push_back(key);
                        }
                    } else if log {
                        // An incomplete frozen tail is not a transferable target.
                        let sent = self.state.files.get(&key).map_or(0, |f| f.sent);
                        self.update_target(input, sent.min(target.len), *rank, None);
                    }
                }
                Err(Failure::Local(error)) => {
                    local_failure = true;
                    eprintln!("semon push: {key}: {error}");
                }
                Err(Failure::Remote(error)) => {
                    self.save()?;
                    return Err(error);
                }
                Err(Failure::Stopped) => break,
            }
        }
        report.files = touched.len();
        self.save()?;
        if !local_failure && !self.stop.is_stopped() {
            self.completed_inventory = Some(
                selected
                    .values()
                    .filter(|(input, _, _)| input.root == semon_sessions::InputRoot::Codex)
                    .map(|(input, _, _)| input.path.clone())
                    .collect(),
            );
        }
        self.refresh_phase();
        Ok(report)
    }

    fn refresh_phase(&self) {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        if self.completed_inventory.is_some()
            && self.completed_inventory == self.committed_inventory
            && progress.inventory_complete
            && progress.targets.values().all(|t| {
                t.acked_bytes == t.target_bytes
                    && self
                        .state
                        .files
                        .get(&format!("{}/{}", t.root, t.path))
                        .is_some_and(|f| f.confirmed)
            })
        {
            progress.phase = wire::SyncPhase::UpToDate;
        }
    }

    fn discover(
        &mut self,
        sessions: &Options,
        selected: &mut BTreeMap<String, (Input, FileStat, status::TargetRank)>,
        foreground: &mut VecDeque<String>,
        history: &mut VecDeque<String>,
    ) -> Result<()> {
        let runtime = semon_sessions::local_runtime_facts(sessions).map_err(|e| e.to_string())?;
        let mut fresh = Vec::new();
        let inputs = semon_sessions::inputs(sessions).map_err(|e| e.to_string())?;
        let present = inputs.iter().map(Self::key).collect::<BTreeSet<_>>();
        selected.retain(|key, _| present.contains(key));
        foreground.retain(|key| present.contains(key));
        history.retain(|key| present.contains(key));
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prune_targets(&present);
        for input in inputs {
            let key = Self::key(&input);
            if selected.contains_key(&key) {
                continue;
            }
            let stat = FileStat::from_metadata(
                &fs::metadata(input.full_path(sessions)).map_err(|e| e.to_string())?,
            );
            let live = input.root == semon_sessions::InputRoot::Codex
                && !runtime.codex_locks.is_empty()
                && semon_sessions::codex_native_id_prefix(&input.full_path(sessions), stat.len)
                    .ok()
                    .flatten()
                    .is_some_and(|id| runtime.codex_locks.contains_key(&id));
            fresh.push((input, stat, live));
        }
        // Recent files first, deterministic ties; small files win equal time.
        fresh.sort_by(|(a, sa, la), (b, sb, lb)| {
            lb.cmp(la)
                .then(sb.mtime_ns.cmp(&sa.mtime_ns))
                .then(sa.len.cmp(&sb.len))
                .then(a.cmp(b))
        });
        let mut new_foreground = VecDeque::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i128;
        for (input, stat, live) in fresh {
            let key = Self::key(&input);
            let changed = self
                .state
                .files
                .get(&key)
                .is_some_and(|f| !f.matches_stat(stat));
            let recent = stat
                .mtime_ns
                .is_some_and(|mtime| now.saturating_sub(mtime) <= 600_000_000_000);
            let is_foreground = live || changed || recent || input.path.ends_with(".json");
            let rank = status::TargetRank {
                live,
                foreground: is_foreground,
                modified_ns: stat.mtime_ns,
                small: std::cmp::Reverse(stat.len),
            };
            if is_foreground {
                new_foreground.push_back(key.clone());
            } else {
                history.push_back(key.clone());
            }
            self.update_target(&input, stat.len, rank, Some(stat));
            selected.insert(key, (input, stat, rank));
        }
        // Refill vacancies after deletion/moves without rescanning files or
        // promoting old ACKs. Only discovery examines the complete frozen set.
        for (input, stat, rank) in selected.values() {
            let missing = !self
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .targets
                .contains_key(&Self::key(input));
            if missing {
                self.update_target(input, stat.len, *rank, Some(*stat));
            }
        }
        new_foreground.append(foreground);
        *foreground = new_foreground;
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        progress.inventory_complete = selected.len() == progress.targets.len();
        progress.phase = wire::SyncPhase::Syncing;
        Ok(())
    }

    fn update_target(
        &self,
        input: &Input,
        target: u64,
        rank: status::TargetRank,
        observed: Option<FileStat>,
    ) {
        let key = Self::key(input);
        let known = self.state.files.get(&key);
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        progress.admit_target(
            key,
            wire::SyncTarget {
                root: input.root.as_str().into(),
                path: input.path.clone(),
                generation: known.map_or_else(String::new, |f| f.generation.clone()),
                target_bytes: target,
                acked_bytes: known
                    .filter(|f| f.confirmed && observed.is_none_or(|stat| f.matches_stat(stat)))
                    .map_or(0, |f| f.sent.min(target)),
                head_sha256: known
                    .filter(|f| !f.redacted_head.is_empty())
                    .map_or_else(|| head_sha256(&[]), |f| f.redacted_head.clone()),
            },
            rank,
        );
    }

    fn key(input: &Input) -> String {
        format!("{}/{}", input.root.as_str(), input.path)
    }

    fn send(&self, append: &Append) -> std::result::Result<Answer, Failure> {
        self.append(append)
    }

    fn needs_generation_probe(&self, file: &FileState) -> bool {
        (!file.confirmed || file.generation.is_empty())
            && self.generation_support.load(Ordering::Relaxed) != GenerationSupport::Legacy as u8
    }

    /// A small JSON file, sent whole whenever its content changes.
    fn sync_whole(&mut self, input: &Input, path: &Path) -> Synced {
        let stat = FileStat::from_metadata(&fs::metadata(path).map_err(local)?);
        let key = Self::key(input);
        let known = self.state.files.get(&key);
        if known.is_some_and(|file| {
            file.has_stat()
                && file.matches_stat(stat)
                && file.sent == stat.len
                && !self.needs_generation_probe(file)
        }) {
            return Ok((0, false));
        }
        if stat.len > WHOLE_CAP {
            return Ok((0, false));
        }
        let raw = read_input(path).map_err(local)?;
        let digest = sha256_hex(&raw);
        if self.state.files.get(&key).is_some_and(|file| {
            file.raw_head == digest
                && file.sent == raw.len() as u64
                && !self.needs_generation_probe(file)
        }) {
            let file = self.state.files.get_mut(&key).expect("known file");
            file.set_stat(stat);
            return Ok((0, false));
        }
        let mut data = raw.clone();
        redact::redact(&mut data);
        if let Some(known) = self
            .state
            .files
            .get(&key)
            .filter(|file| file.raw_head == digest && file.sent == raw.len() as u64)
        {
            let generation = if known.generation.is_empty() {
                wire::new_generation().map_err(Failure::Local)?
            } else {
                known.generation.clone()
            };
            let bind = Append {
                generation: Some(generation.clone()),
                root: input.root.as_str().into(),
                path: input.path.clone(),
                offset: raw.len() as u64,
                head_sha256: head_sha256(&data),
                bytes: String::new(),
                replace: false,
            };
            match self.send(&bind)? {
                Answer::Ok(ack) if ack.length == raw.len() as u64 => {
                    let file = self.state.files.get_mut(&key).expect("known file");
                    file.confirmed = ack.generation.as_deref() == Some(generation.as_str());
                    file.generation = generation;
                    file.redacted_head = bind.head_sha256;
                    file.set_stat(stat);
                    self.save().map_err(Failure::Local)?;
                    return Ok((0, false));
                }
                Answer::Ok(_) => {
                    return Err(Failure::Remote("whole file ACK length disagrees".into()));
                }
                // No accepted bind: use the existing full replacement path with
                // a new generation rather than retrying an unproved prefix.
                Answer::Conflict(_) => {}
            }
        }
        let generation = wire::new_generation().map_err(Failure::Local)?;
        let append = Append {
            generation: Some(generation.clone()),
            root: input.root.as_str().into(),
            path: input.path.clone(),
            offset: 0,
            head_sha256: head_sha256(&data),
            bytes: base64_encode(&data),
            replace: true,
        };
        let ack = match self.send(&append)? {
            Answer::Ok(ack) if ack.length == raw.len() as u64 => ack,
            Answer::Ok(_) => return Err(Failure::Remote("whole file ACK length disagrees".into())),
            Answer::Conflict(length) => {
                return Err(Failure::Remote(format!(
                    "a replace was refused (receiver at {})",
                    length.length
                )));
            }
        };
        self.state.files.insert(
            key,
            FileState {
                sent: raw.len() as u64,
                confirmed: ack.generation.as_deref() == Some(generation.as_str()),
                generation,
                redacted_head: append.head_sha256.clone(),
                raw_head: digest,
                len: stat.len,
                mtime_ns: stat.mtime_ns,
                dev: stat.dev,
                ino: stat.ino,
            },
        );
        self.save().map_err(Failure::Local)?;
        Ok((raw.len() as u64, true))
    }

    /// A JSONL log: its new complete lines, appended; the whole file again
    /// if it was rewritten or the receiver's copy differs.
    fn sync_log(&mut self, input: &Input, path: &Path, target: u64) -> Synced {
        let key = Self::key(input);
        let stat = FileStat::from_metadata(&fs::metadata(path).map_err(local)?);
        let known = self.state.files.get(&key).cloned().unwrap_or_default();
        if known.has_stat()
            && known.matches_stat(stat)
            && known.sent == stat.len
            && !self.needs_generation_probe(&known)
        {
            return Ok((0, false));
        }
        let mut sent = known.sent;
        let mut replace = false;
        let stat_changed = !known.has_stat() || !known.matches_stat(stat);
        let identity_changed = known.has_stat() && (known.dev != stat.dev || known.ino != stat.ino);
        let shortened = known.has_stat() && stat.len < known.len;
        let head_changed = stat_changed
            && !identity_changed
            && !shortened
            && sent > 0
            && sent <= stat.len
            && raw_head(path, sent).map_err(Failure::Local)? != known.raw_head;
        if sent > stat.len
            || shortened
            || identity_changed
            || head_changed
            || (stat_changed && known.has_stat() && stat.len == known.len && sent > 0)
        {
            sent = 0;
            replace = true;
        }
        let mut generation = if replace || known.generation.is_empty() {
            wire::new_generation().map_err(Failure::Local)?
        } else {
            known.generation.clone()
        };
        let mut confirmed = known.confirmed && !replace;
        let mut head = if replace {
            Vec::new()
        } else {
            self.heads.get(&key).cloned().unwrap_or_default()
        };
        let mut total = 0;
        let mut replaced = false;
        let mut conflicts = 0;
        let mut acknowledged_raw_head = None;
        loop {
            if self.stop.is_stopped() {
                return Err(Failure::Stopped);
            }
            let (mut data, end) = complete_lines_to(path, sent, self.chunk, target.min(stat.len))
                .map_err(Failure::Local)?;
            if data.is_empty() && !replace && !self.needs_generation_probe(&known) {
                break;
            }
            if replace {
                head.clear();
            }
            if head.is_empty() && sent > 0 {
                head = redacted_prefix(path, sent).map_err(Failure::Local)?;
            }
            let batch_raw_head = raw_head(path, end).map_err(Failure::Local)?;
            redact::redact(&mut data);
            let mut offset = sent;
            let mut conflict = None;
            // An empty replace still empties the receiver's copy.
            let pieces: Vec<&[u8]> = if data.is_empty() {
                vec![data.as_slice()]
            } else {
                data.chunks(self.chunk).collect()
            };
            for piece in pieces {
                if head.len() < HEAD_BYTES {
                    let take = piece.len().min(HEAD_BYTES - head.len());
                    head.extend_from_slice(&piece[..take]);
                }
                let append = Append {
                    generation: Some(generation.clone()),
                    root: input.root.as_str().into(),
                    path: input.path.clone(),
                    offset,
                    head_sha256: head_sha256(&head),
                    bytes: base64_encode(piece),
                    replace,
                };
                match self.send(&append)? {
                    Answer::Ok(ack) if ack.length == offset + piece.len() as u64 => {
                        confirmed = ack.generation.as_deref() == Some(generation.as_str());
                        let length = ack.length;
                        replaced |= replace;
                        replace = false;
                        offset = length;
                        total += piece.len() as u64;
                        self.heads.insert(key.clone(), head.clone());
                    }
                    Answer::Ok(ack) => {
                        let length = ack.length;
                        return Err(Failure::Remote(format!(
                            "the receiver reports {length} bytes after an append to {}",
                            offset + piece.len() as u64
                        )));
                    }
                    Answer::Conflict(length) => {
                        conflict = Some(length);
                        break;
                    }
                }
            }
            if let Some(theirs) = conflict {
                conflicts += 1;
                if conflicts > 3 {
                    return Err(Failure::Remote(format!(
                        "the receiver keeps disagreeing about {key}"
                    )));
                }
                // Resume from the receiver's length when its copy is ours up
                // to a line boundary; otherwise send the file again whole.
                if resumable(path, target.min(stat.len), &theirs).map_err(Failure::Local)? {
                    sent = theirs.length;
                    replace = false;
                    if let Some(receiver_generation) =
                        theirs.generation.filter(|g| wire::is_generation(g))
                    {
                        generation = receiver_generation;
                        confirmed = true;
                    } else {
                        confirmed = false;
                    }
                } else {
                    sent = 0;
                    replace = true;
                    confirmed = false;
                    generation = wire::new_generation().map_err(Failure::Local)?;
                }
                head = if replace {
                    Vec::new()
                } else {
                    redacted_prefix(path, sent).map_err(Failure::Local)?
                };
                continue;
            }
            sent = end;
            acknowledged_raw_head = Some(batch_raw_head);
            break;
        }
        let digest = if total > 0 || replaced || sent != known.sent {
            if let Some(head) = acknowledged_raw_head {
                head
            } else {
                raw_head(path, sent).map_err(Failure::Local)?
            }
        } else {
            known.raw_head
        };
        self.state.files.insert(
            key,
            FileState {
                sent,
                generation,
                confirmed,
                redacted_head: head_sha256(&head),
                raw_head: digest,
                len: stat.len,
                mtime_ns: stat.mtime_ns,
                dev: stat.dev,
                ino: stat.ino,
            },
        );
        self.save().map_err(Failure::Local)?;
        Ok((total, replaced))
    }
}

/// Why a file wasn't brought up to date.
#[derive(Debug)]
enum Failure {
    /// Reading the local file failed: try again next pass.
    Local(String),
    /// The receiver failed or refused.
    Remote(String),
    /// The push was stopped.
    Stopped,
}

impl Failure {
    fn message(self) -> String {
        match self {
            Self::Local(message) | Self::Remote(message) => message,
            Self::Stopped => "stopped".to_owned(),
        }
    }
}

type Synced = std::result::Result<(u64, bool), Failure>;

fn local(error: io::Error) -> Failure {
    Failure::Local(error.to_string())
}

fn open_input(path: &Path) -> io::Result<fs::File> {
    let file = semon_sessions::open_read_only_input(path)?;
    #[cfg(test)]
    INPUT_OPENS.with(|opens| {
        *opens.borrow_mut().entry(path.to_owned()).or_default() += 1;
    });
    Ok(file)
}

fn read_input(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_input(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
fn reset_input_opens() {
    INPUT_OPENS.with(|opens| opens.borrow_mut().clear());
}

#[cfg(test)]
fn input_opens() -> BTreeMap<PathBuf, usize> {
    INPUT_OPENS.with(|opens| opens.borrow().clone())
}

/// SHA-256 of the file's first `min(4096, upto)` raw bytes.
fn raw_head(path: &Path, upto: u64) -> Result<String> {
    if upto == 0 {
        return Ok(sha256_hex(&[]));
    }
    let mut buffer = vec![0; upto.min(HEAD_BYTES as u64) as usize];
    let mut file = open_input(path).map_err(|e| e.to_string())?;
    file.read_exact(&mut buffer).map_err(|e| e.to_string())?;
    Ok(sha256_hex(&buffer))
}

/// The first `min(4096, len)` bytes of the file as sent (redacted), where
/// `len` is a line boundary. Redaction works on whole lines, so the lines
/// covering those bytes are read.
fn redacted_prefix(path: &Path, len: u64) -> Result<Vec<u8>> {
    let want = len.min(HEAD_BYTES as u64) as usize;
    if want == 0 {
        return Ok(Vec::new());
    }
    let file = open_input(path).map_err(|e| e.to_string())?;
    let mut reader = file.take(len.min(LINE_CAP as u64));
    let mut raw = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
        if raw.len() >= want
            && let Some(newline) = raw[want - 1..].iter().position(|byte| *byte == b'\n')
        {
            raw.truncate(want + newline);
            break;
        }
    }
    redact::redact(&mut raw);
    raw.truncate(want);
    Ok(raw)
}

/// Whether the receiver's copy (`theirs`) is ours, redacted, up to a line
/// boundary.
fn resumable(path: &Path, size: u64, theirs: &Length) -> Result<bool> {
    let length = theirs.length;
    if length == 0 {
        return Ok(theirs.head_sha256.as_deref() == Some(head_sha256(&[]).as_str()));
    }
    if length > size {
        return Ok(false);
    }
    let mut file = open_input(path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(length - 1))
        .map_err(|e| e.to_string())?;
    let mut last = [0];
    file.read_exact(&mut last).map_err(|e| e.to_string())?;
    if last[0] != b'\n' {
        return Ok(false);
    }
    let ours = head_sha256(&redacted_prefix(path, length)?);
    Ok(theirs.head_sha256.as_deref() == Some(ours.as_str()))
}

/// The raw bytes from `from` to the end of the last complete line within
/// `chunk` bytes, and where they end. A first line longer than `chunk` is
/// read whole, up to [`LINE_CAP`]; one longer than that is cut there.
/// Nothing when no line is complete yet.
fn complete_lines_to(path: &Path, from: u64, chunk: usize, upto: u64) -> Result<(Vec<u8>, u64)> {
    let mut file = open_input(path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(from))
        .map_err(|e| e.to_string())?;
    let mut file = file.take(upto.saturating_sub(from));
    let mut data = Vec::new();
    let mut buffer = vec![0; chunk.clamp(1, 1 << 20)];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
        if (data.len() >= chunk && data.contains(&b'\n')) || data.len() >= LINE_CAP {
            break;
        }
    }
    let window = &data[..data.len().min(chunk)];
    let end = if let Some(newline) = window.iter().rposition(|byte| *byte == b'\n') {
        newline + 1
    } else if let Some(newline) = data.iter().position(|byte| *byte == b'\n') {
        newline + 1
    } else if data.len() >= LINE_CAP {
        data.len()
    } else {
        0
    };
    data.truncate(end);
    Ok((data, from + end as u64))
}

/// Writes `bytes` to `path` with mode 0600, atomically. A missing directory
/// is created 0700; an existing one's mode is left alone.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use io::Write;
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)
}

/// Creates a missing directory 0700; an existing one's mode is left alone.
fn create_private_dir(directory: &Path) -> io::Result<()> {
    if directory.as_os_str().is_empty() || directory.exists() {
        return Ok(());
    }
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// `semon push`: one pass and one facts post, or, with `watch`, a pass every
/// 2 s and facts every 10 s until the process ends. [`push_until`] with a
/// [`Stop`] no one stops.
pub fn push(options: &PushOptions, watch: bool) -> Result<()> {
    push_until(options, watch, &Stop::new())
}

/// [`push`], ended by `stop`: it returns `Ok(())` within about 40 ms of
/// [`Stop::stop`], from the sleep between passes, a pass, a request to the
/// receiver or a facts collection (see [`Stop`]). What the receiver
/// acknowledged is in the state file by then, which is only ever replaced
/// whole ([`write_private`]).
///
/// It holds the state file's [`StateLock`] while it runs, and fails at once
/// when another push holds it. Work a stop left running keeps holding the
/// lock until that work ends (at most the 120 s request timeout, or the
/// facts collection in progress), so a push started again right after a
/// stop can fail with a "still finishing" error; retry it shortly.
///
/// Call it from a plain thread, not from async code: the HTTP client is
/// `reqwest::blocking`, which panics inside a tokio runtime (in an async
/// Tauri command, say; use `spawn_blocking` or `std::thread::spawn`).
pub fn push_until(options: &PushOptions, watch: bool, stop: &Stop) -> Result<()> {
    // Declared first, so it is dropped last, after the client and the
    // facts worker have let go of their shares.
    let hold = Hold::new(StateLock::acquire(&options.state)?);
    let mut client = Client::new(options)?
        .with_stop(stop.clone())
        .with_lock(hold.share());
    let heartbeat =
        status::Heartbeat::start(options, stop, hold.share(), Arc::clone(&client.progress))?;
    let result = push_running(options, watch, stop, &hold, &mut client);
    if let Some(error) = heartbeat.fatal() {
        return Err(error);
    }
    result
}

fn push_running(
    options: &PushOptions,
    watch: bool,
    stop: &Stop,
    hold: &Hold,
    client: &mut Client,
) -> Result<()> {
    let initial_pass = client.pass(&options.sessions);
    if let Err(error) = &initial_pass
        && error.contains("refused the token")
    {
        return Err(error.clone());
    }
    if !watch && let Err(error) = &initial_pass {
        return Err(error.clone());
    }
    let report = initial_pass.as_ref().copied().unwrap_or_default();
    if stop.is_stopped() {
        return Ok(());
    }
    eprintln!(
        "semon push: {} files, {} bytes{}",
        report.files,
        report.bytes,
        if report.replaced > 0 {
            format!(", {} sent again whole", report.replaced)
        } else {
            String::new()
        }
    );
    if !watch {
        let sessions = options.sessions.clone();
        let lock = hold.share();
        let Some(facts) = stop.run("facts", move || {
            let _lock = lock;
            semon_sessions::local_facts(&sessions).map_err(|error| error.to_string())
        })?
        else {
            return Ok(());
        };
        let mut facts = facts?;
        if client.completed_inventory.is_none() {
            return Ok(());
        }
        facts.codex_rollouts = client.completed_inventory.clone();
        facts.codex_provisional_rollouts = None;
        return match client.post_facts(&facts) {
            Err(Failure::Stopped) => Ok(()),
            result => result.map_err(Failure::message),
        };
    }

    let facts = FactsWorker::start(&options.sessions, hold.share())?;
    let mut backoff = WatchBackoff::default();
    let mut pass = initial_pass;
    let mut last_facts = Instant::now() - FACTS_EVERY;
    loop {
        if let Err(error) = &pass
            && error.contains("refused the token")
        {
            return Err(error.clone());
        }
        if stop.is_stopped() {
            return Ok(());
        }
        // An idle pass made no request, so it cannot prove the receiver
        // resumed accepting appends. Keep the restriction until a real ack.
        let idle = matches!(&pass, Ok(report) if report.files == 0);
        let messages = if idle && backoff.state.is_some() {
            Vec::new()
        } else {
            backoff.observe(pass.as_ref().err().map(String::as_str))
        };
        for message in messages {
            eprintln!("semon push: {message}");
        }
        {
            let phase = match backoff.state {
                Some("paused") => wire::SyncPhase::Paused,
                Some("storage full") => wire::SyncPhase::StorageFull,
                _ if pass.is_err() => wire::SyncPhase::WaitingToRetry,
                _ => {
                    client
                        .progress
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .phase
                }
            };
            client
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .phase = phase;
        }
        if backoff.state.is_none()
            && let Some(end) = after_pass(pass, stop)
        {
            return end;
        }
        if backoff.state != Some("paused")
            && client.completed_inventory.is_some()
            && last_facts.elapsed() >= FACTS_EVERY
        {
            match facts.collect(stop)? {
                None => return Ok(()),
                Some(Ok(mut current)) => {
                    current.codex_rollouts = client.completed_inventory.clone();
                    current.codex_provisional_rollouts = None;
                    match client.post_facts(&current) {
                        Ok(()) => {
                            client.committed_inventory = client.completed_inventory.clone();
                            client.refresh_phase();
                        }
                        Err(Failure::Stopped) => return Ok(()),
                        Err(failure) => {
                            let error = failure.message();
                            if error.contains("refused the token") {
                                return Err(error);
                            }
                            if !(backoff.state == Some("storage full")
                                && error.starts_with("storage is full"))
                            {
                                for message in backoff.observe(Some(&error)) {
                                    eprintln!("semon push: {message}");
                                }
                            }
                            if backoff.state.is_none() {
                                eprintln!("semon push: {error}");
                            }
                        }
                    }
                }
                Some(Err(error)) => eprintln!("semon push: {error}"),
            }
            last_facts = Instant::now();
        }
        let deadline = Instant::now() + backoff.delay();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            if stop.sleep(remaining.min(FACTS_EVERY)) {
                return Ok(());
            }
        }
        pass = client.pass(&options.sessions);
    }
}

fn error_kind(text: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

#[derive(Default)]
struct WatchBackoff {
    state: Option<&'static str>,
    delay: Duration,
}
impl WatchBackoff {
    fn observe(&mut self, error: Option<&str>) -> Vec<String> {
        let next = error.and_then(|error| {
            if error.starts_with("storage is full") {
                Some("storage full")
            } else if error.starts_with("pushes are paused") {
                Some("paused")
            } else {
                self.state
            }
        });
        let mut messages = Vec::new();
        if next != self.state {
            if let Some(previous) = self.state {
                messages.push(
                    if previous == "paused" {
                        "resumed"
                    } else {
                        "sending again"
                    }
                    .into(),
                );
            }
            if next.is_some() {
                messages.push(error.unwrap().into());
            }
            self.delay = Duration::from_secs(60);
        } else if next.is_some() {
            self.delay = (self.delay * 2).min(Duration::from_secs(600));
        }
        self.state = next;
        messages
    }
    fn delay(&self) -> Duration {
        if self.state.is_some() {
            self.delay
        } else {
            PASS_EVERY
        }
    }
}

/// What `--watch` does with a pass's result: `Some` ends the push with it.
/// A refused token ends it as an error even when a stop came with the
/// refusal; otherwise a stop ends it cleanly, and other errors are reported
/// and tried again next pass.
fn after_pass(result: Result<Report>, stop: &Stop) -> Option<Result<()>> {
    match result {
        Err(error) if error.contains("refused the token") => Some(Err(error)),
        _ if stop.is_stopped() => Some(Ok(())),
        Ok(report) => {
            if report.files > 0 {
                eprintln!("semon push: {} files, {} bytes", report.files, report.bytes);
            }
            None
        }
        Err(error) => {
            eprintln!("semon push: {error}");
            None
        }
    }
}

/// Collects `--watch`'s facts on a thread of its own, where its
/// [`FactsSource`] lives, so a stop doesn't wait for a collection (the
/// first can read every log's metadata). A collection a stop leaves behind
/// finishes there, holding the state lock; the thread ends when this is
/// dropped.
struct FactsWorker {
    ask: mpsc::Sender<()>,
    answers: mpsc::Receiver<Result<Facts>>,
}

impl FactsWorker {
    fn start(sessions: &Options, lock: Arc<StateLock>) -> Result<Self> {
        let sessions = sessions.clone();
        Self::start_with(lock, move || {
            let mut source = FactsSource::new(&sessions);
            move || source.facts().map_err(|error| error.to_string())
        })
    }

    fn start_with<F: FnMut() -> Result<Facts> + Send + 'static>(
        lock: Arc<StateLock>,
        make: impl FnOnce() -> F + Send + 'static,
    ) -> Result<Self> {
        let (ask, asked) = mpsc::channel::<()>();
        let (answer, answers) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("semon-push-facts".to_owned())
            .spawn(move || {
                let _lock = lock;
                let mut collect = make();
                for () in asked {
                    let facts = collect();
                    if answer.send(facts).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| format!("facts: {error}"))?;
        Ok(Self { ask, answers })
    }

    /// The machine's current facts, or `None` when stopped first. `Err`
    /// only when the thread is gone.
    fn collect(&self, stop: &Stop) -> Result<Option<Result<Facts>>> {
        if stop.is_stopped() {
            return Ok(None);
        }
        self.ask
            .send(())
            .map_err(|_| "facts: its thread ended".to_owned())?;
        stop.wait("facts", &self.answers)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };

    use serde_json::json;
    use tiny_http::{Header, Response, Server};

    use crate::wire::base64_decode;

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const TOKEN: &str = "test-token-0123456789";
    const LARGE_LINE_BYTES: usize = 2 * 1024 * 1024;

    struct Fixture {
        root: PathBuf,
        options: Options,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-push-idle-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let options = Options {
                claude_home: root.join("claude"),
                claude_json: root.join(".claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                since: Duration::from_secs(86_400),
                session: None,
                facts: None,
                scan_window: false,
            };
            let fixture = Self { root, options };
            fixture.write("token", TOKEN.as_bytes());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    fixture.root.join("token"),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
            fixture
        }

        fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }

        fn append(&self, path: &Path, contents: &[u8]) {
            use std::io::Write;
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            file.write_all(contents).unwrap();
        }

        fn push_options(&self, url: &str) -> PushOptions {
            PushOptions {
                url: url.to_owned(),
                credential: Credential::File(self.root.join("token")),
                sessions: self.options.clone(),
                state: self.root.join("state/push.json"),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct Receiver {
        url: String,
        server: Arc<Server>,
        appends: Arc<Mutex<Vec<Append>>>,
    }

    impl Receiver {
        fn len(&self) -> usize {
            self.appends.lock().unwrap().len()
        }

        fn from(&self, index: usize) -> Vec<Append> {
            self.appends.lock().unwrap()[index..].to_vec()
        }
    }

    impl Drop for Receiver {
        fn drop(&mut self) {
            self.server.unblock();
        }
    }

    fn receiver() -> Receiver {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let appends = Arc::new(Mutex::new(Vec::<Append>::new()));
        let (worker, shared) = (Arc::clone(&server), Arc::clone(&appends));
        thread::spawn(move || {
            for mut request in worker.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let response = if request.url() == "/v1/mirror/append" {
                    match serde_json::from_str::<Append>(&body) {
                        Ok(append) => {
                            let length = append.offset
                                + base64_decode(&append.bytes).map_or(0, |bytes| bytes.len())
                                    as u64;
                            let generation = append.generation.clone();
                            shared.lock().unwrap().push(append);
                            Response::from_data(
                                json!({"length": length, "generation": generation})
                                    .to_string()
                                    .into_bytes(),
                            )
                            .with_status_code(tiny_http::StatusCode(200))
                        }
                        Err(_) => Response::from_data(Vec::new())
                            .with_status_code(tiny_http::StatusCode(400)),
                    }
                } else {
                    Response::from_data(Vec::new()).with_status_code(tiny_http::StatusCode(404))
                };
                let response = response
                    .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
                let _ = request.respond(response);
            }
        });
        Receiver {
            url: format!("http://127.0.0.1:{port}"),
            server,
            appends,
        }
    }

    fn generation_receiver(
        fixture: &Fixture,
    ) -> (
        Receiver,
        Arc<std::sync::atomic::AtomicBool>,
        Arc<mirror::Receiver>,
    ) {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let appends = Arc::new(Mutex::new(Vec::<Append>::new()));
        let upgraded = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let directory = fixture.root.join("receiver");
        fs::create_dir_all(&directory).unwrap();
        let reference = Arc::new(mirror::Receiver::new(directory));
        let (worker, recorded, enabled, sink) = (
            Arc::clone(&server),
            Arc::clone(&appends),
            Arc::clone(&upgraded),
            Arc::clone(&reference),
        );
        thread::spawn(move || {
            for mut request in worker.incoming_requests() {
                assert_eq!(request.url(), "/v1/mirror/append");
                let mut body = Vec::new();
                request.as_reader().read_to_end(&mut body).unwrap();
                let mut append: Append = serde_json::from_slice(&body).unwrap();
                recorded.lock().unwrap().push(append.clone());
                if !enabled.load(Ordering::SeqCst) {
                    // Emulate a receiver predating the optional generation field:
                    // real storage is written, without a generation marker/echo.
                    append.generation = None;
                }
                let reply = sink.append("fixture", &append);
                request
                    .respond(
                        Response::from_string(reply.body.to_string())
                            .with_status_code(reply.status),
                    )
                    .unwrap();
            }
        });
        (
            Receiver {
                url,
                server,
                appends,
            },
            upgraded,
            reference,
        )
    }

    #[test]
    fn old_generation_acks_persist_then_rebind_whole_and_log_after_receiver_upgrade() {
        let fixture = Fixture::new();
        let log = fixture.write("claude/projects/-work/old.jsonl", b"{}\n");
        let whole = fixture.write(
            "claude/sessions/42.json",
            b"{\"pid\":42,\"status\":\"idle\"}",
        );
        let (receiver, upgraded, reference) = generation_receiver(&fixture);
        let options = fixture.push_options(&receiver.url);
        let mut client = Client::new(&options).unwrap();
        assert_eq!(client.pass(&fixture.options).unwrap().files, 2);
        assert_eq!(receiver.len(), 2);
        assert_eq!(client.state.files.len(), 2);
        assert!(
            client
                .state
                .files
                .values()
                .all(|f| !f.confirmed && wire::is_generation(&f.generation))
        );
        for _ in 0..3 {
            reset_input_opens();
            assert_eq!(client.pass(&fixture.options).unwrap(), Report::default());
            assert!(input_opens().is_empty());
        }
        assert_eq!(
            receiver.len(),
            2,
            "negative support must keep old idle passes cheap"
        );
        drop(client);
        // A fresh client gets one probe, then negatively caches the old receiver.
        let mut client = Client::new(&options).unwrap();
        for _ in 0..3 {
            assert_eq!(client.pass(&fixture.options).unwrap(), Report::default());
        }
        assert_eq!(receiver.len(), 3);
        let attempts = receiver.from(2);
        assert_eq!(attempts.len(), 1);
        assert!(decode(&attempts[0]).is_empty() && !attempts[0].replace);
        drop(client);
        upgraded.store(true, Ordering::SeqCst);
        let mut client = Client::new(&options).unwrap();
        assert_eq!(client.pass(&fixture.options).unwrap(), Report::default());
        let binds = receiver.from(3);
        assert_eq!(
            binds.len(),
            2,
            "restart qualifies both saved unconfirmed cursor kinds"
        );
        for bind in &binds {
            assert!(decode(bind).is_empty() && !bind.replace);
            let source = if bind.path.ends_with(".jsonl") {
                &log
            } else {
                &whole
            };
            let raw = fs::read(source).unwrap();
            assert_eq!(bind.offset, raw.len() as u64);
            assert_eq!(bind.head_sha256, head_sha256(&raw));
            assert_eq!(
                fs::read(
                    reference
                        .machine_dir("fixture")
                        .join("claude")
                        .join(&bind.path)
                )
                .unwrap(),
                raw
            );
        }
        assert!(client.state.files.values().all(|f| f.confirmed));
        let persisted: State = serde_json::from_slice(&fs::read(&options.state).unwrap()).unwrap();
        assert!(persisted.files.values().all(|f| f.confirmed));
        reset_input_opens();
        assert_eq!(client.pass(&fixture.options).unwrap(), Report::default());
        assert!(input_opens().is_empty());
        assert_eq!(receiver.len(), 5);
    }

    #[test]
    fn positive_data_ack_promotes_legacy_generation_cache_without_restart() {
        let fixture = Fixture::new();
        let log = fixture.write("claude/projects/-work/growing.jsonl", b"{}\n");
        fixture.write("claude/sessions/42.json", b"{\"pid\":42}");
        let (receiver, upgraded, reference) = generation_receiver(&fixture);
        let mut client = Client::new(&fixture.push_options(&receiver.url)).unwrap();
        assert_eq!(client.pass(&fixture.options).unwrap().files, 2);
        upgraded.store(true, Ordering::SeqCst);
        fixture.append(&log, b"{\"new\":true}\n");
        let report = client.pass(&fixture.options).unwrap();
        assert_eq!(report.bytes, b"{\"new\":true}\n".len() as u64);
        assert_eq!(report.replaced, 0);
        assert_eq!(client.pass(&fixture.options).unwrap(), Report::default());
        let sent = receiver.from(2);
        assert_eq!(
            sent.len(),
            2,
            "data echo must enable the remaining whole-file bind"
        );
        let data = sent.iter().find(|a| !decode(a).is_empty()).unwrap();
        assert_eq!(data.offset, 3);
        assert_eq!(decode(data), b"{\"new\":true}\n");
        let bind = sent.iter().find(|a| decode(a).is_empty()).unwrap();
        assert_eq!(bind.path, "sessions/42.json");
        assert!(sent.iter().all(|a| !a.replace));
        assert!(client.state.files.values().all(|f| f.confirmed));
        assert_eq!(
            fs::read(
                reference
                    .machine_dir("fixture")
                    .join("claude/projects/-work/growing.jsonl")
            )
            .unwrap(),
            fs::read(log).unwrap()
        );
    }

    #[test]
    fn missing_resumable_proof_requires_replacement_even_for_an_empty_copy() {
        let fixture = Fixture::new();
        let path = fixture.write("claude/projects/-work/empty.jsonl", b"");
        let unknown = Length {
            length: 0,
            generation: None,
            head_sha256: None,
        };
        assert!(!resumable(&path, 0, &unknown).unwrap());
        let known = Length {
            head_sha256: Some(head_sha256(&[])),
            ..unknown
        };
        assert!(resumable(&path, 0, &known).unwrap());
    }

    #[test]
    fn capped_status_admits_new_acked_native_source_while_history_is_still_pending() {
        let fixture = Fixture::new();
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let options =
            fixture.push_options(&format!("http://{}", server.server_addr().to_ip().unwrap()));
        let lock = Arc::new(StateLock::acquire(&options.state).unwrap());
        let stop = Stop::new();
        let mut client = Client::new(&options)
            .unwrap()
            .with_stop(stop.clone())
            .with_lock(Arc::clone(&lock))
            .with_chunk(64);
        for index in 0..wire::MAX_STATUS_TARGETS - 1 {
            let relative = format!("projects/-work/cold-{index:04}.jsonl");
            let path = fixture.write(&format!("claude/{relative}"), b"{}\n");
            fs::File::open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                .unwrap();
            let stat = FileStat::from_metadata(&fs::metadata(&path).unwrap());
            // Prior confirmed cursors make this bounded fixture cheap. Their
            // missing receiver markers still cannot prove a provisional source.
            let mut file = FileState {
                sent: 3,
                raw_head: sha256_hex(b"{}\n"),
                generation: "b".repeat(32),
                confirmed: true,
                redacted_head: head_sha256(b"{}\n"),
                ..FileState::default()
            };
            file.set_stat(stat);
            client
                .state
                .files
                .insert(format!("claude/{relative}"), file);
        }
        let history = fixture.write("claude/projects/-work/pending.jsonl", &b"{}\n".repeat(200));
        fs::File::open(&history)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        let recent = fixture.root.join("codex/sessions/fresh.jsonl");
        let fresh_bytes =
            b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"fresh-native\"}}\n".to_vec();
        let mirror_dir = fixture.root.join("receiver");
        fs::create_dir_all(&mirror_dir).unwrap();
        let receiver = Arc::new(mirror::Receiver::new(&mirror_dir));
        let sink = Arc::clone(&receiver);
        let listener = Arc::clone(&server);
        let (waiting, first_pending) = mpsc::channel();
        let (proof, proven) = mpsc::channel();
        let blocked = Arc::new(Mutex::new(Vec::<tiny_http::Request>::new()));
        let held = Arc::clone(&blocked);
        let http =
            thread::spawn(move || {
                let mut initial: Option<(tiny_http::Request, Append)> = None;
                let mut history_acks = 0;
                let mut fresh_acked = false;
                let mut cold_after_fresh = 0;
                for mut request in listener.incoming_requests() {
                    if request.method() == &tiny_http::Method::Head {
                        assert_eq!(request.url(), "/v1/mirror/status");
                        request
                            .respond(
                                Response::empty(405)
                                    .with_header(Header::from_bytes("Allow", "POST").unwrap()),
                            )
                            .unwrap();
                        continue;
                    }
                    let mut body = Vec::new();
                    request.as_reader().read_to_end(&mut body).unwrap();
                    match request.url() {
                        "/v1/mirror/append" => {
                            let append: Append = serde_json::from_slice(&body).unwrap();
                            if append.path == "projects/-work/pending.jsonl"
                                && history_acks == 0
                                && initial.is_none()
                            {
                                initial = Some((request, append));
                                waiting.send(()).unwrap();
                                continue;
                            }
                            if append.path == "projects/-work/pending.jsonl" && history_acks >= 4 {
                                held.lock().unwrap().push(request);
                                continue;
                            }
                            let answer = sink.append("fixture", &append);
                            assert_eq!(answer.status, 200);
                            if append.path == "projects/-work/pending.jsonl" {
                                history_acks += 1;
                                if fresh_acked {
                                    cold_after_fresh += 1;
                                }
                            }
                            if append.path == "sessions/fresh.jsonl"
                                && base64_decode(&append.bytes).unwrap().last() == Some(&b'\n')
                            {
                                fresh_acked = true;
                            }
                            request
                                .respond(Response::from_string(answer.body.to_string()))
                                .unwrap();
                        }
                        "/v1/mirror/status" => {
                            let observation: wire::Status = serde_json::from_slice(&body).unwrap();
                            let answer = sink.handle("fixture", mirror::Endpoint::Status, &body);
                            assert_eq!(answer.status, 200);
                            if let Some((pending, append)) = initial.take() {
                                assert_eq!(
                                    observation.targets.len(),
                                    wire::MAX_STATUS_TARGETS,
                                    "initial manifest is already full before fresh discovery"
                                );
                                assert_eq!(history_acks, 0);
                                request
                                    .respond(Response::from_string(answer.body.to_string()))
                                    .unwrap();
                                fs::create_dir_all(recent.parent().unwrap()).unwrap();
                                fs::write(&recent, &fresh_bytes).unwrap();
                                thread::sleep(Duration::from_millis(2100));
                                let accepted = sink.append("fixture", &append);
                                assert_eq!(accepted.status, 200);
                                history_acks = 1;
                                pending
                                    .respond(Response::from_string(accepted.body.to_string()))
                                    .unwrap();
                            } else {
                                let verified =
                                    answer.body["confirmed"].as_array().unwrap().iter().any(
                                        |target| {
                                            target["root"] == "codex"
                                                && target["path"] == "sessions/fresh.jsonl"
                                        },
                                    );
                                request
                                    .respond(Response::from_string(answer.body.to_string()))
                                    .unwrap();
                                if verified {
                                    assert!(
                                        fresh_acked
                                            && cold_after_fresh >= 2
                                            && !held.lock().unwrap().is_empty()
                                    );
                                    let path = sink
                                        .machine_dir("fixture")
                                        .join("codex/sessions/fresh.jsonl");
                                    assert_eq!(fs::read(&path).unwrap(), fresh_bytes);
                                    assert_eq!(
                                        semon_sessions::codex_native_id_prefix(
                                            &path,
                                            fresh_bytes.len() as u64
                                        )
                                        .unwrap()
                                        .as_deref(),
                                        Some("fresh-native")
                                    );
                                    proof.send(observation).unwrap();
                                }
                            }
                        }
                        route => panic!("unexpected route before complete history: {route}"),
                    }
                }
            });
        let progress = Arc::clone(&client.progress);
        let sessions = fixture.options.clone();
        let sender = thread::spawn(move || client.pass(&sessions));
        first_pending.recv_timeout(Duration::from_secs(10)).unwrap();
        let heartbeat =
            status::Heartbeat::start(&options, &stop, Arc::clone(&lock), progress).unwrap();
        let observation = proven.recv_timeout(Duration::from_secs(25)).unwrap();
        assert_eq!(observation.targets.len(), wire::MAX_STATUS_TARGETS);
        assert!(!observation.inventory_complete);
        assert_eq!(observation.targets[0].path, "sessions/fresh.jsonl");
        assert!(
            observation.targets[0].acked_bytes > 0
                && wire::is_generation(&observation.targets[0].generation)
        );
        assert!(
            !receiver
                .machine_dir("fixture")
                .join(mirror::FACTS_FILE)
                .exists(),
            "backfill must not commit early full facts"
        );
        assert!(
            fs::metadata(
                receiver
                    .machine_dir("fixture")
                    .join("claude/projects/-work/pending.jsonl")
            )
            .unwrap()
            .len()
                < fs::metadata(history).unwrap().len()
        );
        stop.stop();
        sender.join().unwrap().unwrap();
        drop(heartbeat);
        blocked.lock().unwrap().clear();
        drop(lock);
        server.unblock();
        http.join().unwrap();
    }

    #[test]
    fn limits_have_clear_messages_and_rate_limit_retries_the_same_body() {
        let fixture = Fixture::new();
        let server = Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr());
        let worker = thread::spawn(move || {
            let mut bodies = Vec::new();
            for (status, body) in [
                (413, r#"{"error":"storage_limit","used":12,"limit":20}"#),
                (423, r#"{"error":"paused"}"#),
                (429, r#"{"error":"rate_limited"}"#),
                (429, r#"{"error":"rate_limited"}"#),
                (200, "{}"),
                (413, r#"{"error":"body_too_large"}"#),
            ] {
                let mut request = server.recv().unwrap();
                let mut input = String::new();
                request.as_reader().read_to_string(&mut input).unwrap();
                bodies.push(input);
                request
                    .respond(
                        Response::from_string(body)
                            .with_status_code(status)
                            .with_header(Header::from_bytes("Retry-After", "17").unwrap()),
                    )
                    .unwrap();
            }
            bodies
        });
        let client = Client::new(&fixture.push_options(&url)).unwrap();
        let mut waits = Vec::new();
        let mut request = || {
            client.post_wait("append", "same request".into(), |duration| {
                waits.push(duration);
                false
            })
        };
        assert!(
            request()
                .err()
                .unwrap()
                .message()
                .contains("storage is full (used 12 of 20)")
        );
        assert!(
            request()
                .err()
                .unwrap()
                .message()
                .contains("paused by the owner")
        );
        assert_eq!(request().unwrap().0, 200);
        assert_eq!(
            request().unwrap().0,
            413,
            "oversized body is distinct from storage full"
        );
        assert_eq!(waits, vec![Duration::from_secs(17); 2]);
        assert!(
            worker
                .join()
                .unwrap()
                .iter()
                .all(|body| body == "same request")
        );
    }

    #[test]
    fn watch_does_not_exit_on_the_first_paused_or_storage_full_pass() {
        for (status, body) in [
            (423, r#"{"error":"paused"}"#),
            (413, r#"{"error":"storage_limit","used":12,"limit":20}"#),
        ] {
            let fixture = Fixture::new();
            fixture.write("claude/projects/-work/transcript.jsonl", b"hello\n");
            let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
            let url = format!("http://{}", server.server_addr());
            let options = fixture.push_options(&url);
            let stop = Stop::new();
            let stopped = stop.clone();
            let (ended, result) = mpsc::channel();
            let push = thread::spawn(move || {
                ended.send(push_until(&options, true, &stopped)).unwrap();
            });
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let mut request = server
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap()
                    .expect("initial append or optional status");
                if request.method() == &tiny_http::Method::Head {
                    assert_eq!(request.url(), "/v1/mirror/status");
                    request
                        .respond(
                            Response::empty(405)
                                .with_header(Header::from_bytes("Allow", "POST").unwrap()),
                        )
                        .unwrap();
                    continue;
                }
                match request.url() {
                    "/v1/mirror/append" => {
                        request
                            .respond(Response::from_string(body).with_status_code(status))
                            .unwrap();
                        break;
                    }
                    "/v1/mirror/status" => {
                        let mut input = String::new();
                        request.as_reader().read_to_string(&mut input).unwrap();
                        let observation: wire::Status = serde_json::from_str(&input).unwrap();
                        assert!(observation.runtime.codex_rollouts.is_none());
                        request
                            .respond(Response::from_string("{\"confirmed\":[]}"))
                            .unwrap();
                    }
                    route => panic!("unexpected initial route: {route}"),
                }
            }
            // Restricted appends retain retry backoff while lightweight fresh
            // status continues. A partial pass must not publish full inventory.
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let mut request = server
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap()
                    .expect("restricted-pass status heartbeat");
                if request.method() == &tiny_http::Method::Head {
                    assert_eq!(request.url(), "/v1/mirror/status");
                    request
                        .respond(
                            Response::empty(405)
                                .with_header(Header::from_bytes("Allow", "POST").unwrap()),
                        )
                        .unwrap();
                    continue;
                }
                assert_eq!(
                    request.url(),
                    "/v1/mirror/status",
                    "no append retry or premature full facts while restricted"
                );
                let mut input = String::new();
                request.as_reader().read_to_string(&mut input).unwrap();
                let observation: wire::Status = serde_json::from_str(&input).unwrap();
                assert!(observation.runtime.codex_rollouts.is_none());
                assert!(
                    observation
                        .targets
                        .iter()
                        .all(|target| target.acked_bytes == 0)
                );
                request
                    .respond(Response::from_string("{\"confirmed\":[]}"))
                    .unwrap();
                let expected = if status == 413 {
                    wire::SyncPhase::StorageFull
                } else {
                    wire::SyncPhase::Paused
                };
                if observation.phase == expected {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "restriction was not reflected in fresh status"
                );
            }
            assert!(
                server
                    .recv_timeout(Duration::from_millis(100))
                    .unwrap()
                    .is_none(),
                "a successful status must not clear append backoff"
            );
            assert!(matches!(
                result.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            stop.stop();
            assert!(result.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
            push.join().unwrap();
        }
    }

    #[test]
    fn watch_limit_backoff_logs_only_transitions_and_recovers() {
        let mut state = WatchBackoff::default();
        let full = "storage is full (used 12 of 20)";
        assert_eq!(state.observe(Some(full)), vec![full]);
        assert_eq!(state.delay(), Duration::from_secs(60));
        for expected in [120, 240, 480, 600, 600] {
            assert!(state.observe(Some(full)).is_empty());
            assert_eq!(state.delay(), Duration::from_secs(expected));
        }
        assert_eq!(state.observe(None), vec!["sending again"]);
        assert_eq!(state.delay(), PASS_EVERY);
        let paused = "pushes are paused by the owner";
        assert_eq!(state.observe(Some(paused)), vec![paused]);
        assert!(state.observe(Some(paused)).is_empty());
        assert_eq!(state.observe(None), vec!["resumed"]);
        assert!(state.observe(None).is_empty());
    }

    fn decode(append: &Append) -> Vec<u8> {
        base64_decode(&append.bytes).unwrap()
    }

    #[test]
    fn unchanged_logs_are_not_opened_and_growth_uses_the_cached_head() {
        let fixture = Fixture::new();
        let receiver = receiver();
        let mut paths = Vec::new();
        for index in 0..50 {
            let contents = if index == 0 {
                let mut contents = vec![b'x'; LARGE_LINE_BYTES];
                contents.push(b'\n');
                contents
            } else {
                format!("file-{index:02}\n").into_bytes()
            };
            paths.push(fixture.write(
                &format!("claude/projects/-work/transcript-{index:02}.jsonl"),
                &contents,
            ));
        }

        let mut client = Client::new(&fixture.push_options(&receiver.url)).unwrap();
        reset_input_opens();
        let report = client.pass(&fixture.options).unwrap();
        assert_eq!(report.files, 50);
        assert!(receiver.len() >= 50);

        let after_initial = receiver.len();
        reset_input_opens();
        assert_eq!(client.pass(&fixture.options).unwrap().files, 0);
        assert!(input_opens().is_empty(), "an idle pass opened a transcript");
        assert_eq!(receiver.len(), after_initial, "an idle pass sent data");

        let appended = b"new line\n";
        fixture.append(&paths[0], appended);
        reset_input_opens();
        let report = client.pass(&fixture.options).unwrap();
        assert_eq!(report.files, 1);
        assert_eq!(report.bytes, appended.len() as u64);
        let opens = input_opens();
        assert_eq!(opens.len(), 1, "only the growing transcript should open");
        assert!(
            matches!(opens.get(&paths[0]).copied(), Some(1..=4)),
            "the cached head must avoid a fifth open: {opens:?}"
        );
        let sent = receiver.from(after_initial);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].offset, (LARGE_LINE_BYTES + 1) as u64);
        assert_eq!(decode(&sent[0]), appended);
        assert!(!sent[0].replace);

        let rewrite_path = &paths[18];
        let before = fs::metadata(rewrite_path).unwrap().modified().unwrap();
        let rewritten = b"edit-18\n";
        assert_eq!(rewritten.len(), b"file-18\n".len());
        fs::write(rewrite_path, rewritten).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while fs::metadata(rewrite_path).unwrap().modified().unwrap() == before
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        let after = fs::metadata(rewrite_path).unwrap().modified().unwrap();
        assert_ne!(after, before, "the fixture rewrite must change mtime");

        let before_rewrite = receiver.len();
        reset_input_opens();
        let report = client.pass(&fixture.options).unwrap();
        assert_eq!(report.replaced, 1);
        assert_eq!(report.bytes, rewritten.len() as u64);
        let sent = receiver.from(before_rewrite);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].replace);
        assert_eq!(decode(&sent[0]), rewritten);
        let opens = input_opens();
        assert_eq!(opens.len(), 1, "only the rewritten transcript should open");
        assert!(opens.contains_key(rewrite_path));

        reset_input_opens();
        assert_eq!(client.pass(&fixture.options).unwrap().files, 0);
        assert!(input_opens().is_empty());
    }

    #[test]
    fn state_without_stat_fields_loads_and_gets_one_full_check() {
        let fixture = Fixture::new();
        let path = fixture.write("claude/projects/-work/old.jsonl", b"already sent\n");
        let server = Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr());
        let mirror = fixture.root.join("mirror");
        fs::create_dir_all(&mirror).unwrap();
        let reference = mirror::Receiver::new(&mirror);
        let bytes = b"already sent\n";
        let existing = Append {
            root: "claude".into(),
            path: "projects/-work/old.jsonl".into(),
            offset: 0,
            head_sha256: head_sha256(bytes),
            bytes: base64_encode(bytes),
            replace: false,
            generation: None,
        };
        assert_eq!(reference.append("legacy", &existing).status, 200);
        let worker = thread::spawn(move || {
            let mut request = server
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .expect("legacy generation negotiation");
            assert_eq!(request.url(), "/v1/mirror/append");
            let mut input = String::new();
            request.as_reader().read_to_string(&mut input).unwrap();
            let append: Append = serde_json::from_str(&input).unwrap();
            assert_eq!(append.offset, bytes.len() as u64);
            assert!(
                decode(&append).is_empty(),
                "an unchanged legacy cursor must not reupload history"
            );
            assert!(!append.replace);
            assert_eq!(append.head_sha256, head_sha256(bytes));
            assert!(
                append
                    .generation
                    .as_deref()
                    .is_some_and(wire::is_generation)
            );
            let answer = reference.append("legacy", &append);
            assert_eq!(answer.status, 200);
            assert_eq!(
                answer.body["generation"],
                append.generation.as_deref().unwrap()
            );
            request
                .respond(
                    Response::from_string(answer.body.to_string()).with_status_code(answer.status),
                )
                .unwrap();
            assert_eq!(
                fs::read(
                    reference
                        .machine_dir("legacy")
                        .join("claude/projects/-work/old.jsonl")
                )
                .unwrap(),
                bytes.as_slice()
            );
        });
        let key = "claude/projects/-work/old.jsonl";
        let files = BTreeMap::from([(
            key.to_owned(),
            json!({
                "sent": b"already sent\n".len(),
                "raw_head": sha256_hex(b"already sent\n")
            }),
        )]);
        let old_state = json!({
            "version": 1,
            "url": url,
            "files": files
        });
        let options = fixture.push_options(&url);
        fs::create_dir_all(options.state.parent().unwrap()).unwrap();
        fs::write(&options.state, old_state.to_string()).unwrap();

        let mut client = Client::new(&options).unwrap();
        assert_eq!(client.state.files[key].sent, b"already sent\n".len() as u64);
        assert!(!client.state.files[key].has_stat());
        reset_input_opens();
        assert_eq!(client.pass(&fixture.options).unwrap().files, 0);
        assert!(input_opens().get(&path).copied().unwrap_or_default() > 0);
        assert!(client.state.files[key].has_stat());
        assert!(client.state.files[key].confirmed);
        assert_eq!(client.state.files[key].sent, bytes.len() as u64);
        worker.join().unwrap();
        reset_input_opens();
        assert_eq!(client.pass(&fixture.options).unwrap().files, 0);
        assert!(
            input_opens().is_empty(),
            "qualified legacy cursors should use the stat cache on the next pass"
        );
    }

    #[test]
    fn a_refused_token_ends_the_watch_as_an_error_even_with_a_stop() {
        fn refused<T>() -> Result<T> {
            Err("append: the receiver refused the token (401)".to_owned())
        }
        fn failed() -> Result<Report> {
            Err("append: 500 down".to_owned())
        }
        let sent = || {
            Ok(Report {
                files: 1,
                bytes: 5,
                replaced: 0,
            })
        };

        let stopped = Stop::new();
        stopped.stop();
        assert_eq!(after_pass(refused(), &stopped), Some(refused()));
        assert_eq!(after_pass(failed(), &stopped), Some(Ok(())));
        assert_eq!(after_pass(sent(), &stopped), Some(Ok(())));

        let running = Stop::new();
        assert_eq!(after_pass(refused(), &running), Some(refused()));
        assert_eq!(after_pass(failed(), &running), None);
        assert_eq!(after_pass(sent(), &running), None);
    }
}

#[cfg(test)]
mod sync_status_tests {
    use super::*;
    use tiny_http::{Response, Server};

    fn answer_status_head(request: tiny_http::Request) -> Option<tiny_http::Request> {
        if request.method() == &tiny_http::Method::Head {
            assert_eq!(request.url(), "/v1/mirror/status");
            request
                .respond(
                    Response::empty(405)
                        .with_header(tiny_http::Header::from_bytes("Allow", "POST").unwrap()),
                )
                .unwrap();
            None
        } else {
            Some(request)
        }
    }

    #[test]
    fn blocked_full_facts_does_not_block_fresh_heartbeat_or_release_its_lock() {
        let root =
            std::env::temp_dir().join(format!("semon-heartbeat-facts-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let observations = Arc::new(Mutex::new(Vec::<wire::Status>::new()));
        let observed = Arc::clone(&observations);
        let listener = Arc::clone(&server);
        let http = thread::spawn(move || {
            for request in listener.incoming_requests() {
                let Some(mut request) = answer_status_head(request) else {
                    continue;
                };
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                observed
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&body).unwrap());
                request.respond(Response::from_string("{}")).unwrap();
            }
        });
        let options = PushOptions {
            url,
            credential: Credential::Memory(Token::new("test-status-token").unwrap()),
            sessions: Options {
                claude_home: root.join("claude"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("cache"),
                ..Options::default()
            },
            state: root.join("push.json"),
        };
        let lock = Arc::new(StateLock::acquire(&options.state).unwrap());
        let stop = Stop::new();
        let (release, blocked) = mpsc::channel();
        let (entered, waiting) = mpsc::channel();
        let worker = FactsWorker::start_with(Arc::clone(&lock), move || {
            move || {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(Facts::default())
            }
        })
        .unwrap();
        worker.ask.send(()).unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        let heartbeat = status::Heartbeat::start(
            &options,
            &stop,
            Arc::clone(&lock),
            Arc::new(Mutex::new(status::Progress::default())),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(25);
        while observations.lock().unwrap().len() < 2 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let records = observations.lock().unwrap();
        assert!(records[1].observed_at_ms > records[0].observed_at_ms);
        assert!(records.iter().all(|s| s.runtime.codex_rollouts.is_none()));
        drop(records);
        stop.stop();
        drop(heartbeat);
        drop(worker);
        drop(lock);
        assert!(StateLock::acquire(&options.state).is_err());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if StateLock::acquire(&options.state).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        server.unblock();
        http.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ending_heartbeat_cancels_rate_limit_wait_and_releases_only_its_ownership() {
        let root =
            std::env::temp_dir().join(format!("semon-status-rate-stop-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let listener = Arc::clone(&server);
        let (seen, received) = mpsc::channel();
        let http = thread::spawn(move || {
            for request in listener.incoming_requests() {
                let Some(request) = answer_status_head(request) else {
                    continue;
                };
                request
                    .respond(
                        Response::from_string("{\"error\":\"rate_limited\"}")
                            .with_status_code(429)
                            .with_header(
                                tiny_http::Header::from_bytes("Retry-After", "60").unwrap(),
                            ),
                    )
                    .unwrap();
                seen.send(()).unwrap();
            }
        });
        let options = PushOptions {
            url: format!("http://{}", server.server_addr().to_ip().unwrap()),
            credential: Credential::Memory(Token::new("test-status-token").unwrap()),
            sessions: Options {
                claude_home: root.join("claude"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("cache"),
                ..Options::default()
            },
            state: root.join("push.json"),
        };
        let stop = Stop::new();
        let lock = Arc::new(StateLock::acquire(&options.state).unwrap());
        let heartbeat = status::Heartbeat::start(
            &options,
            &stop,
            Arc::clone(&lock),
            Arc::new(Mutex::new(status::Progress::default())),
        )
        .unwrap();
        received.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(heartbeat);
        drop(lock);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if StateLock::acquire(&options.state).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!stop.is_stopped());
        server.unblock();
        http.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applied_status_with_lost_ack_advances_sequence_on_the_next_fresh_submission() {
        let root =
            std::env::temp_dir().join(format!("semon-status-lost-ack-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let listener = Arc::clone(&server);
        let (seen, received) = mpsc::channel();
        let http = thread::spawn(move || {
            let mut previous = 0;
            for request in listener.incoming_requests() {
                let Some(mut request) = answer_status_head(request) else {
                    continue;
                };
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let status: wire::Status = serde_json::from_str(&body).unwrap();
                assert!(status.sequence > previous);
                previous = status.sequence;
                seen.send(status).unwrap();
                if previous == 1 {
                    drop(request);
                } else {
                    request.respond(Response::from_string("{}")).unwrap();
                }
            }
        });
        let options = PushOptions {
            url: format!("http://{}", server.server_addr().to_ip().unwrap()),
            credential: Credential::Memory(Token::new("test-status-token").unwrap()),
            sessions: Options {
                claude_home: root.join("claude"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("cache"),
                ..Options::default()
            },
            state: root.join("push.json"),
        };
        let stop = Stop::new();
        let lock = Arc::new(StateLock::acquire(&options.state).unwrap());
        let heartbeat = status::Heartbeat::start(
            &options,
            &stop,
            Arc::clone(&lock),
            Arc::new(Mutex::new(status::Progress::default())),
        )
        .unwrap();
        let first = received.recv_timeout(Duration::from_secs(3)).unwrap();
        let next = received.recv_timeout(Duration::from_secs(15)).unwrap();
        assert_eq!(first.observation_id, next.observation_id);
        assert_eq!(next.sequence, 2);
        assert!(next.observed_at_ms > first.observed_at_ms);
        stop.stop();
        drop(heartbeat);
        drop(lock);
        server.unblock();
        http.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idle_completion_uses_only_current_targets_and_survives_retained_legacy_cursors() {
        let options = PushOptions {
            url: "http://127.0.0.1:1".into(),
            credential: Credential::Memory(Token::new("test-status-token").unwrap()),
            sessions: Options::default(),
            state: std::env::temp_dir().join("semon-phase-unwritten.json"),
        };
        let mut client = Client::new(&options).unwrap();
        client.completed_inventory = Some(BTreeSet::from(["sessions/current.jsonl".into()]));
        client.committed_inventory = client.completed_inventory.clone();
        client
            .state
            .files
            .insert("codex/sessions/deleted.jsonl".into(), FileState::default());
        client.state.files.insert(
            "codex/sessions/current.jsonl".into(),
            FileState {
                confirmed: true,
                sent: 3,
                ..FileState::default()
            },
        );
        client.progress.lock().unwrap().inventory_complete = true;
        client.progress.lock().unwrap().targets.insert(
            "codex/sessions/current.jsonl".into(),
            wire::SyncTarget {
                root: "codex".into(),
                path: "sessions/current.jsonl".into(),
                generation: "a".repeat(32),
                target_bytes: 3,
                acked_bytes: 3,
                head_sha256: head_sha256(b"{}\n"),
            },
        );
        client.refresh_phase();
        assert_eq!(
            client.progress.lock().unwrap().phase,
            wire::SyncPhase::UpToDate
        );
        client.progress.lock().unwrap().phase = wire::SyncPhase::Syncing;
        client.refresh_phase();
        assert_eq!(
            client.progress.lock().unwrap().phase,
            wire::SyncPhase::UpToDate
        );
        client
            .progress
            .lock()
            .unwrap()
            .targets
            .values_mut()
            .next()
            .unwrap()
            .target_bytes += 3;
        client.progress.lock().unwrap().phase = wire::SyncPhase::Syncing;
        client.refresh_phase();
        assert_eq!(
            client.progress.lock().unwrap().phase,
            wire::SyncPhase::Syncing
        );
    }
}

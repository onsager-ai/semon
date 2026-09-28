//! `semon push`: a client of the mirror protocol (docs/mirror-protocol.md).
//! It keeps a receiver's copy of this machine's session logs in step with
//! the originals, appending as they grow, and sends the machine's facts.
//!
//! - Only the builder's input files are read (`semon_sessions::inputs`).
//! - Every byte sent is redacted first ([`redact`]).
//! - A JSONL log is sent in complete lines only; its last, incomplete line
//!   waits until it is complete.
//! - Where each file stands is kept in a private state file, so an
//!   interrupted push resumes.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use semon_sessions::{Facts, FactsSource, Input, Options};
use serde::{Deserialize, Serialize};

pub mod redact;
pub mod wire;

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

pub struct PushOptions {
    /// The receiver's base URL; requests go to `<url>/v1/mirror/…`.
    pub url: String,
    pub token_file: PathBuf,
    pub sessions: Options,
    /// Where to keep the cursor state (0600).
    pub state: PathBuf,
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
    base.join("semon/push")
        .join(format!("{}.json", &sha256_hex(url.as_bytes())[..16]))
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
    if token.is_empty()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(format!("{} holds no usable token", path.display()));
    }
    Ok(token)
}

/// Checks the receiver URL: https, or http on a loopback address only, and
/// no query or fragment. Returns it without a trailing slash.
pub fn check_url(url: &str) -> Result<String> {
    let url = url.trim_end_matches('/');
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("{url} is not a URL"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
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
    match scheme {
        "https" => Ok(url.to_owned()),
        "http" if loopback(hostname) => Ok(url.to_owned()),
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

#[derive(Clone, Copy)]
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
    Ok(u64),
    Conflict(Length),
}

pub struct Client {
    url: String,
    token: String,
    http: reqwest::blocking::Client,
    state_path: PathBuf,
    state: State,
    chunk: usize,
    heads: BTreeMap<String, Vec<u8>>,
}

impl Client {
    pub fn new(options: &PushOptions) -> Result<Self> {
        let url = check_url(&options.url)?;
        let token = read_token(&options.token_file)?;
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
            state_path: options.state.clone(),
            state,
            chunk: CHUNK_BYTES,
            heads: BTreeMap::new(),
        })
    }

    /// Smaller chunks, for tests.
    #[doc(hidden)]
    pub fn with_chunk(mut self, chunk: usize) -> Self {
        self.chunk = chunk.max(1);
        self
    }

    fn post(&self, route: &str, body: String) -> Result<(u16, String)> {
        let response = self
            .http
            .post(format!("{}/v1/mirror/{route}", self.url))
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .map_err(|error| format!("{route}: {error}"))?;
        let status = response.status().as_u16();
        let text = response.text().unwrap_or_default();
        match status {
            401 | 403 => Err(format!(
                "{route}: the receiver refused the token ({status})"
            )),
            _ => Ok((status, text)),
        }
    }

    /// Sends the machine's facts.
    pub fn send_facts(&self, facts: &Facts) -> Result<()> {
        let body = serde_json::to_string(facts).map_err(|error| error.to_string())?;
        match self.post("facts", body)? {
            (200..=299, _) => Ok(()),
            (status, text) => Err(format!("facts: {status} {}", text.trim())),
        }
    }

    fn append(&self, append: &Append) -> Result<Answer> {
        let body = serde_json::to_string(append).map_err(|error| error.to_string())?;
        let (status, text) = self.post("append", body)?;
        let parsed = || {
            serde_json::from_str::<Length>(&text)
                .map_err(|_| format!("append: {status} with an unreadable body"))
        };
        match status {
            200..=299 => Ok(Answer::Ok(parsed()?.length)),
            409 => Ok(Answer::Conflict(parsed()?)),
            _ => Err(format!(
                "append {}/{}: {status} {}",
                append.root,
                append.path,
                text.trim()
            )),
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
    /// error from the receiver ends the pass.
    pub fn pass(&mut self, sessions: &Options) -> Result<Report> {
        let mut report = Report::default();
        let mut dirty = false;
        let inputs = semon_sessions::inputs(sessions).map_err(|error| error.to_string())?;
        for input in inputs {
            let path = input.full_path(sessions);
            let key = Self::key(&input);
            let previous = self.state.files.get(&key).cloned();
            let outcome = if input.path.ends_with(".jsonl") {
                self.sync_log(&input, &path)
            } else {
                self.sync_whole(&input, &path)
            };
            match outcome {
                Ok((bytes, replaced)) => {
                    if bytes > 0 || replaced {
                        report.files += 1;
                        report.bytes += bytes;
                        report.replaced += usize::from(replaced);
                    }
                    dirty |= previous != self.state.files.get(&key).cloned();
                }
                Err(Failure::Local(error)) => {
                    eprintln!(
                        "semon push: {}/{}: {error}",
                        input.root.as_str(),
                        input.path
                    );
                }
                Err(Failure::Remote(error)) => {
                    self.save()?;
                    return Err(error);
                }
            }
        }
        if dirty {
            self.save()?;
        }
        Ok(report)
    }

    fn key(input: &Input) -> String {
        format!("{}/{}", input.root.as_str(), input.path)
    }

    fn send(&self, append: &Append) -> std::result::Result<Answer, Failure> {
        self.append(append).map_err(Failure::Remote)
    }

    /// A small JSON file, sent whole whenever its content changes.
    fn sync_whole(&mut self, input: &Input, path: &Path) -> Synced {
        let stat = FileStat::from_metadata(&fs::metadata(path).map_err(local)?);
        let key = Self::key(input);
        let known = self.state.files.get(&key);
        if known
            .is_some_and(|file| file.has_stat() && file.matches_stat(stat) && file.sent == stat.len)
        {
            return Ok((0, false));
        }
        if stat.len > WHOLE_CAP {
            return Ok((0, false));
        }
        let raw = read_input(path).map_err(local)?;
        let digest = sha256_hex(&raw);
        if self
            .state
            .files
            .get(&key)
            .is_some_and(|file| file.raw_head == digest && file.sent == raw.len() as u64)
        {
            let file = self.state.files.get_mut(&key).expect("known file");
            file.set_stat(stat);
            return Ok((0, false));
        }
        let mut data = raw.clone();
        redact::redact(&mut data);
        let append = Append {
            root: input.root.as_str().into(),
            path: input.path.clone(),
            offset: 0,
            head_sha256: head_sha256(&data),
            bytes: base64_encode(&data),
            replace: true,
        };
        if let Answer::Conflict(length) = self.send(&append)? {
            return Err(Failure::Remote(format!(
                "a replace of {}/{} was refused (receiver at {})",
                append.root, append.path, length.length
            )));
        }
        self.state.files.insert(
            key,
            FileState {
                sent: raw.len() as u64,
                raw_head: digest,
                len: stat.len,
                mtime_ns: stat.mtime_ns,
                dev: stat.dev,
                ino: stat.ino,
            },
        );
        Ok((raw.len() as u64, true))
    }

    /// A JSONL log: its new complete lines, appended; the whole file again
    /// if it was rewritten or the receiver's copy differs.
    fn sync_log(&mut self, input: &Input, path: &Path) -> Synced {
        let key = Self::key(input);
        let stat = FileStat::from_metadata(&fs::metadata(path).map_err(local)?);
        let known = self.state.files.get(&key).cloned().unwrap_or_default();
        if known.has_stat() && known.matches_stat(stat) && known.sent == stat.len {
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
        if sent > stat.len || shortened || identity_changed || head_changed {
            sent = 0;
            replace = true;
        }
        let mut head = if replace {
            Vec::new()
        } else {
            self.heads.get(&key).cloned().unwrap_or_default()
        };
        let mut total = 0;
        let mut replaced = false;
        let mut conflicts = 0;
        loop {
            let (mut data, end) = complete_lines(path, sent, self.chunk).map_err(Failure::Local)?;
            if data.is_empty() && !replace {
                break;
            }
            if replace {
                head.clear();
            }
            if !data.is_empty() && head.is_empty() && sent > 0 {
                head = redacted_prefix(path, sent).map_err(Failure::Local)?;
            }
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
                    root: input.root.as_str().into(),
                    path: input.path.clone(),
                    offset,
                    head_sha256: head_sha256(&head),
                    bytes: base64_encode(piece),
                    replace,
                };
                match self.send(&append)? {
                    Answer::Ok(length) if length == offset + piece.len() as u64 => {
                        replaced |= replace;
                        replace = false;
                        offset = length;
                        total += piece.len() as u64;
                        self.heads.insert(key.clone(), head.clone());
                    }
                    Answer::Ok(length) => {
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
                if resumable(path, stat.len, &theirs).map_err(Failure::Local)? {
                    sent = theirs.length;
                    replace = false;
                } else {
                    sent = 0;
                    replace = true;
                }
                head = if replace {
                    Vec::new()
                } else {
                    redacted_prefix(path, sent).map_err(Failure::Local)?
                };
                continue;
            }
            sent = end;
            if data.is_empty() {
                break;
            }
        }
        let digest = if total > 0 || replaced || sent != known.sent {
            raw_head(path, sent).map_err(Failure::Local)?
        } else {
            known.raw_head
        };
        self.state.files.insert(
            key,
            FileState {
                sent,
                raw_head: digest,
                len: stat.len,
                mtime_ns: stat.mtime_ns,
                dev: stat.dev,
                ino: stat.ino,
            },
        );
        Ok((total, replaced))
    }
}

/// Why a file wasn't brought up to date.
enum Failure {
    /// Reading the local file failed: try again next pass.
    Local(String),
    /// The receiver failed or refused.
    Remote(String),
}

type Synced = std::result::Result<(u64, bool), Failure>;

fn local(error: io::Error) -> Failure {
    Failure::Local(error.to_string())
}

fn open_input(path: &Path) -> io::Result<fs::File> {
    let file = fs::File::open(path)?;
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
        return Ok(true);
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
fn complete_lines(path: &Path, from: u64, chunk: usize) -> Result<(Vec<u8>, u64)> {
    let mut file = open_input(path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(from))
        .map_err(|e| e.to_string())?;
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

/// Writes `bytes` to `path` with mode 0600, in a 0700 directory, atomically.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
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

/// `semon push`: one pass and one facts post, or, with `watch`, a pass every
/// 2 s and facts every 10 s until interrupted.
pub fn push(options: &PushOptions, watch: bool) -> Result<()> {
    let mut client = Client::new(options)?;
    let report = client.pass(&options.sessions)?;
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
        let facts = semon_sessions::local_facts(&options.sessions).map_err(|e| e.to_string())?;
        client.send_facts(&facts)?;
        return Ok(());
    }

    let mut facts = FactsSource::new(&options.sessions);
    let result = (|| {
        let initial = facts.facts().map_err(|error| error.to_string())?;
        client.send_facts(&initial)?;
        let mut last_facts = Instant::now();
        loop {
            thread::sleep(PASS_EVERY);
            match client.pass(&options.sessions) {
                Ok(report) if report.files > 0 => {
                    eprintln!("semon push: {} files, {} bytes", report.files, report.bytes);
                }
                Ok(_) => {}
                Err(error) if error.contains("refused the token") => return Err(error),
                Err(error) => eprintln!("semon push: {error}"),
            }
            if last_facts.elapsed() >= FACTS_EVERY {
                match facts.facts() {
                    Ok(current) => {
                        if let Err(error) = client.send_facts(&current) {
                            if error.contains("refused the token") {
                                return Err(error);
                            }
                            eprintln!("semon push: {error}");
                        }
                    }
                    Err(error) => eprintln!("semon push: {error}"),
                }
                last_facts = Instant::now();
            }
        }
    })();
    let flush = facts.flush().map_err(|error| error.to_string());
    match (result, flush) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
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
                codex_home: root.join("codex"),
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
                token_file: self.root.join("token"),
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
                            shared.lock().unwrap().push(append);
                            Response::from_data(json!({"length": length}).to_string().into_bytes())
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
        assert_eq!(
            opens.get(&paths[0]),
            Some(&4),
            "the cached head should avoid a fifth open"
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
        let url = "http://127.0.0.1:1";
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
        let options = fixture.push_options(url);
        fs::create_dir_all(options.state.parent().unwrap()).unwrap();
        fs::write(&options.state, old_state.to_string()).unwrap();

        let mut client = Client::new(&options).unwrap();
        assert_eq!(client.state.files[key].sent, b"already sent\n".len() as u64);
        assert!(!client.state.files[key].has_stat());
        reset_input_opens();
        assert_eq!(client.pass(&fixture.options).unwrap().files, 0);
        assert!(input_opens().get(&path).copied().unwrap_or_default() > 0);
        assert!(client.state.files[key].has_stat());
    }
}

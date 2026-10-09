//! The receiving side of the mirror protocol (docs/mirror-protocol.md): the
//! reference implementation, over a directory. [`Receiver::append`] and
//! [`Receiver::facts`] each take a parsed request and return the documented
//! answer; [`Receiver::handle`] parses a request body first. Who the machine
//! is comes from the caller (`semon receive` takes it from the token), and a
//! request writes under that machine's directory only:
//!
//! - `DIR/machines/<name>/claude/…` and `DIR/machines/<name>/codex/…`: the
//!   copies of the input files, at their paths under each root;
//! - `DIR/machines/<name>/facts.json`: the machine's last facts.
//!
//! Every answer of 200 is durable: the file's data is on disk (fsync) before
//! it is given. An append that fails partway is cut back off, so the copy is
//! as it was. A replace is a rename, so the copy is the old one or the new
//! one, never a mix; when only the directory's sync after the rename fails,
//! the answer is 500 with the new copy in place, and it is counted as such. Appends to one file are serialized; different files
//! proceed in parallel. Each machine's copy may hold at most
//! [`Receiver::with_max_bytes`] bytes (20 GiB by default): a request that
//! would grow it past that gets 507. The receiver never follows a symbolic link under `DIR`: it
//! creates the directories itself, and refuses a request whose path meets a
//! link.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use semon_sessions::{Facts, is_input_path, is_machine_name};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::wire::{
    Append, CHUNK_BYTES, HEAD_BYTES, MAX_STATUS_TARGETS, Status, base64_decode, is_generation,
    sha256_hex,
};

/// Where each machine's copy lives under the receiver's directory.
pub const MACHINES_DIR: &str = "machines";
/// A machine's facts, in its directory.
pub const FACTS_FILE: &str = "facts.json";
/// The largest request body accepted: one append's 4 MiB of file bytes is
/// 5.34 MiB as base64, plus the JSON around it, rounded up to 6 MiB.
pub const MAX_BODY_BYTES: usize = 6 * 1024 * 1024;

/// The most bytes one machine's copy may hold unless told otherwise.
pub const DEFAULT_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// The two requests of the protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// `POST <url>/v1/mirror/append`
    Append,
    /// `POST <url>/v1/mirror/facts`
    Facts,
    /// Optional partial observation; never commits full facts.
    Status,
}

impl Endpoint {
    /// The endpoint an HTTP request path names, exactly.
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "/v1/mirror/append" => Some(Self::Append),
            "/v1/mirror/facts" => Some(Self::Facts),
            "/v1/mirror/status" => Some(Self::Status),
            _ => None,
        }
    }
}

/// An answer: its HTTP status and JSON body.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    fn new(status: u16, body: Value) -> Self {
        Self { status, body }
    }

    fn error(status: u16, message: impl Into<String>) -> Self {
        Self::new(status, json!({"error": message.into()}))
    }
}

/// Why a request wasn't applied, other than a 409.
#[derive(Debug)]
enum Refusal {
    /// 400: the body or the path is invalid.
    Invalid(String),
    /// 413: more file bytes than one request may carry.
    TooLarge(String),
    /// 507: the machine's copy would grow past its cap.
    Full(String),
    /// 500: the receiver's own storage failed.
    Storage(io::Error),
}

impl From<io::Error> for Refusal {
    fn from(error: io::Error) -> Self {
        Self::Storage(error)
    }
}

/// A write that failed, and whether it changed the copy anyway.
#[derive(Debug)]
struct Failed {
    refusal: Refusal,
    changed: bool,
}

impl From<io::Error> for Failed {
    fn from(error: io::Error) -> Self {
        Self {
            refusal: Refusal::Storage(error),
            changed: false,
        }
    }
}

impl From<Refusal> for Failed {
    fn from(refusal: Refusal) -> Self {
        Self {
            refusal,
            changed: false,
        }
    }
}

impl Refusal {
    fn reply(self) -> Reply {
        match self {
            Self::Invalid(message) => Reply::error(400, message),
            Self::TooLarge(message) => Reply::error(413, message),
            Self::Full(message) => Reply::error(507, message),
            Self::Storage(error) => Reply::error(500, format!("the receiver's storage: {error}")),
        }
    }
}

/// A receiver's directory, the per-file locks that give each file one
/// writer at a time, and how many bytes each machine's copy holds.
pub struct Receiver {
    dir: PathBuf,
    locks: Locks,
    max_bytes: u64,
    usage: Mutex<HashMap<String, u64>>,
}

impl Receiver {
    /// A receiver writing under `dir`, which must exist. What each machine's
    /// copy already holds is counted now, by a walk of `DIR/machines/`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let usage = measure(&dir.join(MACHINES_DIR));
        Self {
            dir,
            locks: Locks::default(),
            max_bytes: DEFAULT_MAX_BYTES,
            usage: Mutex::new(usage),
        }
    }

    /// Caps each machine's copy at `bytes`.
    pub fn with_max_bytes(mut self, bytes: u64) -> Self {
        self.max_bytes = bytes;
        self
    }

    /// The bytes `machine`'s copy holds, as counted.
    pub fn used_bytes(&self, machine: &str) -> u64 {
        self.usage().get(machine).copied().unwrap_or(0)
    }

    fn usage(&self) -> MutexGuard<'_, HashMap<String, u64>> {
        self.usage.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts `growth` more bytes for `machine`, or refuses (507) when that
    /// would pass the cap.
    fn reserve(&self, machine: &str, growth: u64) -> Result<(), Refusal> {
        if growth == 0 {
            return Ok(());
        }
        let mut usage = self.usage();
        let used = usage.entry(machine.to_owned()).or_default();
        if used.saturating_add(growth) > self.max_bytes {
            return Err(Refusal::Full(format!(
                "{machine}'s copy holds {used} bytes; {growth} more would pass its {}-byte cap",
                self.max_bytes
            )));
        }
        *used += growth;
        Ok(())
    }

    /// Counts `bytes` fewer for `machine`.
    fn release(&self, machine: &str, bytes: u64) {
        if let Some(used) = self.usage().get_mut(machine) {
            *used = used.saturating_sub(bytes);
        }
    }

    /// Runs `write` with `grow` bytes reserved for `machine`: they stay
    /// counted, and `shrink` bytes are released, only if it changed the copy.
    ///
    /// A write that fails having changed the copy anyway (a replace renamed
    /// before its directory sync failed) counts as having succeeded.
    fn counted<T>(
        &self,
        machine: &str,
        grow: u64,
        shrink: u64,
        write: impl FnOnce() -> Result<T, Failed>,
    ) -> Result<T, Refusal> {
        self.reserve(machine, grow)?;
        match write() {
            Ok(value) => {
                self.release(machine, shrink);
                Ok(value)
            }
            Err(Failed { refusal, changed }) => {
                self.release(machine, if changed { shrink } else { grow });
                Err(refusal)
            }
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `DIR/machines/<machine>`.
    pub fn machine_dir(&self, machine: &str) -> PathBuf {
        self.dir.join(MACHINES_DIR).join(machine)
    }

    /// Parses `body` as `endpoint`'s request and answers it for `machine`.
    /// A body that isn't the request's JSON is a 400.
    pub fn handle(&self, machine: &str, endpoint: Endpoint, body: &[u8]) -> Reply {
        if body.len() > MAX_BODY_BYTES {
            return Reply::error(
                413,
                format!("the body is over the {MAX_BODY_BYTES}-byte limit"),
            );
        }
        match endpoint {
            Endpoint::Append => match serde_json::from_slice::<Append>(body) {
                Ok(append) => self.append(machine, &append),
                Err(error) => Reply::error(400, format!("the body is not an append: {error}")),
            },
            Endpoint::Status => match serde_json::from_slice::<Status>(body) {
                Ok(status) => self.status(machine, &status),
                Err(error) => Reply::error(400, format!("the body is not status: {error}")),
            },
            Endpoint::Facts => match serde_json::from_slice::<Facts>(body) {
                Ok(facts) => self.facts(machine, &facts),
                Err(error) => Reply::error(400, format!("the body is not facts: {error}")),
            },
        }
    }

    /// `POST /v1/mirror/append` for `machine`: 200 `{length}` when applied,
    /// 409 `{length, head_sha256}` with this copy's length and head when
    /// `offset` isn't its length or the head doesn't match, 400 for an
    /// invalid body or path, 413 for more than 4 MiB of file bytes.
    pub fn append(&self, machine: &str, request: &Append) -> Reply {
        self.try_append(machine, request)
            .unwrap_or_else(Refusal::reply)
    }

    /// `POST /v1/mirror/facts` for `machine`: stores them atomically, and
    /// answers 200 `{}`.
    pub fn facts(&self, machine: &str, facts: &Facts) -> Reply {
        self.try_facts(machine, facts)
            .unwrap_or_else(Refusal::reply)
    }

    fn try_append(&self, machine: &str, request: &Append) -> Result<Reply, Refusal> {
        check_machine(machine)?;
        let (root, path) = (request.root.as_str(), request.path.as_str());
        if !is_input_path(root, path) {
            return Err(Refusal::Invalid(format!(
                "{root}/{path} is not a mirrored input path"
            )));
        }
        let bytes = base64_decode(&request.bytes)
            .ok_or_else(|| Refusal::Invalid("bytes is not standard base64".into()))?;
        if bytes.len() > CHUNK_BYTES {
            return Err(Refusal::TooLarge(format!(
                "{} file bytes; one request carries at most {CHUNK_BYTES}",
                bytes.len()
            )));
        }
        if !is_sha256_hex(&request.head_sha256) {
            return Err(Refusal::Invalid(
                "head_sha256 is not a lowercase hex SHA-256".into(),
            ));
        }
        if request
            .generation
            .as_ref()
            .is_some_and(|g| !is_generation(g))
        {
            return Err(Refusal::Invalid("invalid transfer generation".into()));
        }
        if request.replace && request.offset != 0 {
            return Err(Refusal::Invalid("a replace starts at offset 0".into()));
        }
        if request.offset.checked_add(bytes.len() as u64).is_none() {
            return Err(Refusal::Invalid("offset is out of range".into()));
        }

        let key = format!("{machine}/{root}/{path}");
        self.locks
            .with(&key, || self.write_append(machine, request, &bytes))
    }

    /// Applies a checked append, holding its file's lock.
    fn write_append(
        &self,
        machine: &str,
        request: &Append,
        bytes: &[u8],
    ) -> Result<Reply, Refusal> {
        let mut parts = vec![MACHINES_DIR, machine, request.root.as_str()];
        parts.extend(request.path.split('/'));
        let (name, directories) = parts.split_last().expect("a path has a file name");
        let parent = self.directories(directories)?;
        let target = parent.join(name);

        let existing = open_existing(&target)?;
        let (length, head) = match &existing {
            Some(file) => (file.metadata()?.len(), read_head(file)?),
            None => (0, Vec::new()),
        };
        let marker_base = self.machine_dir(machine).join("sync-generations");
        let marker_dir =
            if request.generation.is_some() || fs::symlink_metadata(&marker_base).is_ok() {
                self.directories(&[MACHINES_DIR, machine, "sync-generations"])?
            } else {
                marker_base
            };
        let marker_name = format!(
            "{}.json",
            sha256_hex(format!("{}/{}", request.root, request.path).as_bytes())
        );
        let marker_path = marker_dir.join(&marker_name);
        let previous = read_generation(&marker_path)?;
        let current_generation = previous
            .filter(|g| {
                g.length == length
                    && g.head_sha256 == sha256_hex(&head)
                    && existing
                        .as_ref()
                        .and_then(|f| f.metadata().ok())
                        .is_some_and(|m| crate::FileStat::from_metadata(&m) == g.identity)
            })
            .map(|g| g.generation);
        let conflict = || {
            Reply::new(
                409,
                serde_json::to_value(crate::wire::Length {
                    length,
                    head_sha256: Some(sha256_hex(&head)),
                    generation: current_generation.clone(),
                })
                .expect("length serializes"),
            )
        };
        if !request.replace && request.offset != length {
            return Ok(conflict());
        }
        if !request.replace
            && current_generation.is_some()
            && request.generation.is_some()
            && request.generation != current_generation
        {
            return Ok(conflict());
        }
        // The head of the copy as it will be: the first min(4096, offset +
        // len) bytes of what it holds up to `offset`, then `bytes`.
        let mut next = if request.replace {
            Vec::new()
        } else {
            head.clone()
        };
        let take = bytes.len().min(HEAD_BYTES - next.len());
        next.extend_from_slice(&bytes[..take]);
        if sha256_hex(&next) != request.head_sha256 {
            return Ok(conflict());
        }

        let new_length = bytes.len() as u64 + if request.replace { 0 } else { request.offset };
        let old_marker = fs::symlink_metadata(&marker_path).map_or(0, |m| m.len());
        let old_total = length.saturating_add(old_marker);
        // Reserve metadata before invalidation. Refused growth leaves proof intact.
        let reserved_total = new_length.saturating_add(if request.generation.is_some() {
            1024
        } else {
            0
        });
        let reservation = reserved_total.saturating_sub(old_total);
        self.reserve(machine, reservation)?;
        let outcome = (|| -> Result<(), Refusal> {
            // Invalidate durably before admitted data mutation.
            match fs::remove_file(&marker_path) {
                Ok(()) => sync_dir(&marker_dir)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            if request.replace {
                replace_file(&parent, name, bytes).map_err(|e| e.refusal)?;
            } else {
                let created = existing.is_none();
                let mut file = match existing {
                    Some(file) => file,
                    None => create_new(&target)?,
                };
                append_durably(
                    &mut file,
                    request.offset,
                    bytes,
                    created.then_some(parent.as_path()),
                )
                .map_err(|e| e.refusal)?;
            }
            if let Some(generation) = &request.generation {
                let marker = Generation {
                    generation: generation.clone(),
                    length: new_length,
                    head_sha256: sha256_hex(&next),
                    identity: crate::FileStat::from_metadata(&fs::metadata(&target)?),
                };
                let bytes =
                    serde_json::to_vec(&marker).map_err(|e| Refusal::Invalid(e.to_string()))?;
                if bytes.len() > 1024 {
                    return Err(Refusal::TooLarge("generation metadata too large".into()));
                }
                replace_file(&marker_dir, &marker_name, &bytes).map_err(|e| e.refusal)?;
            }
            Ok(())
        })();
        // Settle from bounded owned-path metadata even after a partial failure.
        let actual = fs::symlink_metadata(&target)
            .map_or(0, |m| m.len())
            .saturating_add(fs::symlink_metadata(&marker_path).map_or(0, |m| m.len()));
        let release = if actual >= old_total {
            reservation.saturating_sub(actual - old_total)
        } else {
            reservation.saturating_add(old_total - actual)
        };
        self.release(machine, release);
        outcome?;
        Ok(Reply::new(
            200,
            serde_json::to_value(crate::wire::Length {
                length: new_length,
                head_sha256: None,
                generation: request.generation.clone(),
            })
            .expect("length serializes"),
        ))
    }

    /// Stores partial observations separately. Returned confirmed targets have
    /// been checked under the same locks as append, including a durable marker.
    pub fn status(&self, machine: &str, status: &Status) -> Reply {
        self.try_status(machine, status)
            .unwrap_or_else(Refusal::reply)
    }

    fn try_status(&self, machine: &str, status: &Status) -> Result<Reply, Refusal> {
        check_machine(machine)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        if status.version != 1
            || !is_generation(&status.observation_id)
            || status.sequence == 0
            || status.targets.len() > MAX_STATUS_TARGETS
            || status.observed_at_ms > now.saturating_add(60_000)
            || status.observed_at_ms < now.saturating_sub(60_000)
            || status.runtime.mirror_observation_id.is_some()
            || status.runtime.mirror_sequence.is_some()
            || status.runtime.codex_rollouts.is_some()
            || status.runtime.codex_provisional_rollouts.is_some()
            || !status.runtime.repos.is_empty()
            || !status.runtime.reported_runs.is_empty()
        {
            return Err(Refusal::Invalid("invalid partial observation".into()));
        }
        let mut unique = std::collections::BTreeSet::new();
        for target in &status.targets {
            if !is_input_path(&target.root, &target.path)
                || !unique.insert((&target.root, &target.path))
                || target.acked_bytes > target.target_bytes
                || !is_sha256_hex(&target.head_sha256)
                || !(is_generation(&target.generation)
                    || (target.generation.is_empty() && target.acked_bytes == 0))
            {
                return Err(Refusal::Invalid("invalid sync target".into()));
            }
        }
        self.locks.with(&format!("{machine}/status.json"), || {
            let dir = self.directories(&[MACHINES_DIR, machine])?;
            let fence = read_stored_facts(&dir.join(FACTS_FILE))?;
            if fence.as_ref().is_some_and(|facts| facts.mirror_observation_id.as_ref() == Some(&status.observation_id) && facts.mirror_sequence.is_some_and(|seq| status.sequence <= seq)) {
                return Ok(Reply::new(409, json!({"error":"stale_observation"})));
            }
            let prior = read_stored_status(&dir.join("status.json"))?;
            let mut retired = prior.as_ref().map_or_else(Vec::new, |p| p.retired.clone());
            retired.retain(|(_, at)| *at >= now.saturating_sub(60_000));
            if retired.iter().any(|(id, _)| id == &status.observation_id) { return Ok(Reply::new(409, json!({"error":"stale_observation"}))); }
            if let Some(prior) = prior {
                if status.observed_at_ms <= prior.observation.observed_at_ms || (status.observation_id == prior.observation.observation_id && status.sequence <= prior.observation.sequence) {
                    return Ok(Reply::new(409, json!({"error":"stale_observation"})));
                }
                if status.observation_id != prior.observation.observation_id { retired.push((prior.observation.observation_id, prior.observation.observed_at_ms)); }
            }
            if retired.len() > 64 { return Err(Refusal::Invalid("too many recent observation epochs".into())); }
            let mut confirmed = Vec::new();
            for (index, target) in status.targets.iter().enumerate() {
                if target.acked_bytes == 0 {
                    continue;
                }
                let key = format!("{machine}/{}/{}", target.root, target.path);
                let verified = self.locks.with(&key, || -> Result<bool, Refusal> {
                    let marker_dir =
                        self.directories(&[MACHINES_DIR, machine, "sync-generations"])?;
                    let marker_path = marker_dir.join(format!(
                        "{}.json",
                        sha256_hex(format!("{}/{}", target.root, target.path).as_bytes())
                    ));
                    let Some(marker) = read_generation(&marker_path)? else {
                        return Ok(false);
                    };
                    if marker.generation != target.generation || marker.length < target.acked_bytes
                    {
                        return Ok(false);
                    }
                    let mut parts = vec![MACHINES_DIR, machine, target.root.as_str()];
                    parts.extend(target.path.split('/'));
                    let (name, dirs) = parts.split_last().expect("target path");
                    let path = self.directories(dirs)?.join(name);
                    let Some(file) = open_existing(&path)? else {
                        return Ok(false);
                    };
                    let metadata = file.metadata()?;
                    if metadata.len() != marker.length || crate::FileStat::from_metadata(&metadata) != marker.identity {
                        return Ok(false);
                    }
                    let head = read_head(&file)?;
                    if sha256_hex(&head) != marker.head_sha256
                        || sha256_hex(&head[..head.len().min(target.acked_bytes as usize)])
                            != target.head_sha256
                    {
                        return Ok(false);
                    }
                    if target.path.ends_with(".jsonl") {
                        let mut log = semon_sessions::sealed::LogFile::open(&path)?;
                        log.seek(SeekFrom::Start(target.acked_bytes - 1))?;
                        let mut last = [0];
                        log.read_exact(&mut last)?;
                        if last[0] != b'\n' {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                })?;
                if verified {
                    confirmed.push(index);
                }
            }
            let stored = StoredStatus {
                observation: status.clone(),
                confirmed,
                retired,
            };
            let bytes = serde_json::to_vec(&stored).map_err(|e| Refusal::Invalid(e.to_string()))?;
            if bytes.len() > MAX_BODY_BYTES {
                return Err(Refusal::TooLarge("stored status too large".into()));
            }
            let old = fs::symlink_metadata(dir.join("status.json")).map_or(0, |m| m.len());
            let new = bytes.len() as u64;
            self.counted(machine, new.saturating_sub(old), old.saturating_sub(new), || replace_file(&dir, "status.json", &bytes))?;
            Ok(Reply::new(200, json!({"confirmed": stored.confirmed.iter().map(|index| &status.targets[*index]).collect::<Vec<_>>()})))
        })
    }

    fn try_facts(&self, machine: &str, facts: &Facts) -> Result<Reply, Refusal> {
        check_machine(machine)?;
        if facts.codex_provisional_rollouts.is_some() {
            return Err(Refusal::Invalid(
                "authoritative facts cannot carry a provisional overlay".into(),
            ));
        }
        let bytes =
            serde_json::to_vec(facts).map_err(|error| Refusal::Invalid(error.to_string()))?;
        let paired = match (&facts.mirror_observation_id, facts.mirror_sequence) {
            (None, None) => true,
            (Some(id), Some(sequence)) => is_generation(id) && sequence > 0,
            _ => false,
        };
        if !paired {
            return Err(Refusal::Invalid("invalid full-facts fence".into()));
        }
        self.locks.with(&format!("{machine}/status.json"), || {
            let dir = self.directories(&[MACHINES_DIR, machine])?;
            let prior = read_stored_status(&dir.join("status.json"))?;
            if let Some(id) = &facts.mirror_observation_id {
                if prior
                    .as_ref()
                    .is_some_and(|p| p.retired.iter().any(|(old, _)| old == id))
                {
                    return Ok(Reply::new(409, json!({"error":"stale_observation"})));
                }
                if read_stored_facts(&dir.join(FACTS_FILE))?.is_some_and(|p| {
                    p.mirror_observation_id.as_ref() == Some(id)
                        && p.mirror_sequence >= facts.mirror_sequence
                }) {
                    return Ok(Reply::new(409, json!({"error":"stale_observation"})));
                }
            }
            let reply = self.locks.with(&format!("{machine}/{FACTS_FILE}"), || {
                self.write_facts(machine, &bytes)
            })?;
            if let Some(mut prior) = prior {
                prior.confirmed.clear();
                let updated =
                    serde_json::to_vec(&prior).map_err(|e| Refusal::Invalid(e.to_string()))?;
                let old = fs::symlink_metadata(dir.join("status.json")).map_or(0, |m| m.len());
                let new = updated.len() as u64;
                self.counted(
                    machine,
                    new.saturating_sub(old),
                    old.saturating_sub(new),
                    || replace_file(&dir, "status.json", &updated),
                )?;
            }
            Ok(reply)
        })
    }

    fn write_facts(&self, machine: &str, bytes: &[u8]) -> Result<Reply, Refusal> {
        let dir = self.directories(&[MACHINES_DIR, machine])?;
        match fs::symlink_metadata(dir.join(FACTS_FILE)) {
            Ok(meta) if meta.is_dir() => {
                return Err(Refusal::Invalid(format!("{FACTS_FILE} is a directory")));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let old = fs::symlink_metadata(dir.join(FACTS_FILE)).map_or(0, |meta| meta.len());
        let new = bytes.len() as u64;
        // A rename replaces a link at the name; it never writes through one.
        self.counted(
            machine,
            new.saturating_sub(old),
            old.saturating_sub(new),
            || replace_file(&dir, FACTS_FILE, bytes),
        )?;
        Ok(Reply::new(200, json!({})))
    }

    /// `DIR/<parts…>`, each directory created (0700) when missing. A part
    /// that is a symbolic link, or not a directory, refuses the request.
    fn directories(&self, parts: &[&str]) -> Result<PathBuf, Refusal> {
        let mut dir = self.dir.clone();
        for part in parts {
            let parent = dir.clone();
            dir.push(part);
            match fs::symlink_metadata(&dir) {
                Ok(meta) => check_directory(&meta, part)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => match create_dir(&dir) {
                    Ok(()) => sync_dir(&parent)?,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        check_directory(&fs::symlink_metadata(&dir)?, part)?;
                    }
                    Err(error) => return Err(error.into()),
                },
                Err(error) => return Err(error.into()),
            }
        }
        Ok(dir)
    }
}

#[derive(Serialize, Deserialize)]
struct Generation {
    identity: crate::FileStat,
    generation: String,
    length: u64,
    head_sha256: String,
}
#[derive(Serialize, Deserialize)]
struct StoredStatus {
    observation: Status,
    confirmed: Vec<usize>,
    #[serde(default)]
    retired: Vec<(String, i64)>,
}
fn read_stored_status(path: &Path) -> Result<Option<StoredStatus>, Refusal> {
    read_control(path)
}
fn read_stored_facts(path: &Path) -> Result<Option<Facts>, Refusal> {
    read_control(path)
}
fn read_control<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Refusal> {
    let Some(file) = open_existing(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(Refusal::TooLarge("stored observation too large".into()));
    }
    let decoded = serde_json::from_slice(&bytes)
        .map_err(|_| Refusal::Invalid("stored observation unavailable".into()))?;
    Ok(Some(decoded))
}
fn read_generation(path: &Path) -> Result<Option<Generation>, Refusal> {
    let Some(file) = open_existing(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(4096).read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice::<Generation>(&bytes).ok())
}

fn check_machine(machine: &str) -> Result<(), Refusal> {
    if is_machine_name(machine) {
        Ok(())
    } else {
        Err(Refusal::Invalid(format!(
            "{machine:?} is not a machine name"
        )))
    }
}

fn check_directory(meta: &fs::Metadata, part: &str) -> Result<(), Refusal> {
    if meta.file_type().is_symlink() {
        Err(Refusal::Invalid(format!(
            "{part} is a symbolic link on the receiver"
        )))
    } else if !meta.is_dir() {
        Err(Refusal::Invalid(format!(
            "{part} is not a directory on the receiver"
        )))
    } else {
        Ok(())
    }
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The copy at `path`, when there is one: a regular file, opened for
/// reading and writing. A symbolic link, or anything but a regular file,
/// refuses the request.
fn open_existing(path: &Path) -> Result<Option<File>, Refusal> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if meta.file_type().is_symlink() {
        return Err(Refusal::Invalid(format!(
            "{name} is a symbolic link on the receiver"
        )));
    }
    if !meta.is_file() {
        return Err(Refusal::Invalid(format!(
            "{name} is not a regular file on the receiver"
        )));
    }
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    // The name must still be that file, not a link swapped in since.
    if !same_file(&file.metadata()?, &fs::symlink_metadata(path)?) {
        return Err(Refusal::Invalid(format!(
            "{name} changed while it was opened"
        )));
    }
    Ok(Some(file))
}

#[cfg(unix)]
fn same_file(opened: &fs::Metadata, named: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    named.is_file() && opened.dev() == named.dev() && opened.ino() == named.ino()
}

#[cfg(not(unix))]
fn same_file(_opened: &fs::Metadata, named: &fs::Metadata) -> bool {
    named.is_file()
}

/// A new file, 0600. `create_new` never follows a link at the name.
fn create_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn create_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// The file's first `min(4096, len)` bytes.
fn read_head(file: &File) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(HEAD_BYTES);
    let mut reader = file;
    reader.seek(SeekFrom::Start(0))?;
    reader.take(HEAD_BYTES as u64).read_to_end(&mut head)?;
    Ok(head)
}

/// Writes `bytes` at `offset` (the file's length) and syncs them, and the
/// directory `created_in` when the file is new. On any failure the file is
/// cut back to `offset` and synced, so a partial write never stays; only if
/// that fails too has the copy changed.
fn append_durably(
    file: &mut File,
    offset: u64,
    bytes: &[u8],
    created_in: Option<&Path>,
) -> Result<(), Failed> {
    let result = (|| -> io::Result<()> {
        file.seek(SeekFrom::Start(offset))?;
        write_bytes(file, bytes)?;
        sync_data(file)?;
        if let Some(parent) = created_in {
            sync_dir(parent)?;
        }
        Ok(())
    })();
    let Err(error) = result else {
        return Ok(());
    };
    let restored = file.set_len(offset).and_then(|()| file.sync_data()).is_ok();
    Err(Failed {
        refusal: Refusal::Storage(error),
        changed: !restored,
    })
}

/// A failure the tests inject into the next matching step.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// A write stops after this many bytes and fails.
    WriteAfter(usize),
    /// An append's data sync fails.
    SyncData,
    /// A directory sync fails.
    SyncDir,
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
}

/// The injected fault, once, if it is of the kind `matches` accepts.
#[cfg(test)]
fn take_fault(matches: impl Fn(Fault) -> bool) -> Option<Fault> {
    FAULT.with(|cell| {
        let fault = cell.get().filter(|fault| matches(*fault))?;
        cell.set(None);
        Some(fault)
    })
}

fn write_bytes(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    #[cfg(test)]
    {
        if let Some(Fault::WriteAfter(limit)) =
            take_fault(|fault| matches!(fault, Fault::WriteAfter(_)))
        {
            file.write_all(&bytes[..limit.min(bytes.len())])?;
            return Err(io::Error::other("an injected write failure"));
        }
    }
    file.write_all(bytes)
}

fn sync_data(file: &File) -> io::Result<()> {
    #[cfg(test)]
    {
        if take_fault(|fault| fault == Fault::SyncData).is_some() {
            return Err(io::Error::other("an injected sync failure"));
        }
    }
    file.sync_data()
}

/// A replace's temporary file, `.<name>.<pid>.<n>.tmp`, which a crash can
/// leave behind. No input file's name has this shape.
fn is_leftover(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(".tmp")
}

/// Each machine's bytes under `machines`: its `claude/` and `codex/` trees
/// and its facts, never following a link. Leftover temporary files are
/// removed on the way, and not counted.
fn measure(machines: &Path) -> HashMap<String, u64> {
    let mut usage = HashMap::new();
    let Ok(entries) = fs::read_dir(machines) else {
        return usage;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_machine_name(&name) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let dir = entry.path();
        // A facts replace's leftovers sit beside facts.json.
        for inner in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let leftover = inner.file_name().to_str().is_some_and(is_leftover);
            if leftover && inner.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = fs::remove_file(inner.path());
            }
        }
        let bytes: u64 = [
            "claude",
            "codex",
            FACTS_FILE,
            "status.json",
            "sync-generations",
        ]
        .iter()
        .map(|part| tree_size(&dir.join(part), 0))
        .sum();
        usage.insert(name, bytes);
    }
    usage
}

fn tree_size(path: &Path, depth: usize) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        let name = path.file_name().and_then(|name| name.to_str());
        if name.is_some_and(is_leftover) {
            let _ = fs::remove_file(path);
            return 0;
        }
        return meta.len();
    }
    if !meta.is_dir() || depth > 32 {
        return 0;
    }
    fs::read_dir(path).map_or(0, |entries| {
        entries
            .flatten()
            .map(|entry| tree_size(&entry.path(), depth + 1))
            .sum()
    })
}

/// Replaces `parent/name` with `bytes`: a new file beside it, synced, then
/// renamed over it, and the directory synced. A failure before the rename
/// leaves the copy as it was; one after it (the directory sync) has
/// changed it.
fn replace_file(parent: &Path, name: &str, bytes: &[u8]) -> Result<(), Failed> {
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    let mut renamed = false;
    let result = (|| -> io::Result<()> {
        let mut file = create_new(&temporary)?;
        write_bytes(&mut file, bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, parent.join(name))?;
        renamed = true;
        sync_dir(parent)
    })();
    let Err(error) = result else {
        return Ok(());
    };
    if !renamed {
        let _ = fs::remove_file(&temporary);
    }
    Err(Failed {
        refusal: Refusal::Storage(error),
        changed: renamed,
    })
}

/// Makes a directory's entries durable.
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(test)]
    {
        if take_fault(|fault| fault == Fault::SyncDir).is_some() {
            return Err(io::Error::other("an injected directory sync failure"));
        }
    }
    sync_dir_now(dir)
}

#[cfg(unix)]
fn sync_dir_now(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir_now(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// One lock per key, held while a file is written. An entry lives only
/// while someone holds or waits for it.
#[derive(Default)]
struct Locks {
    held: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Locks {
    fn map(&self) -> MutexGuard<'_, HashMap<String, Arc<Mutex<()>>>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `write` holding `key`'s lock.
    fn with<T>(&self, key: &str, write: impl FnOnce() -> T) -> T {
        let entry = Arc::clone(self.map().entry(key.to_owned()).or_default());
        let result = {
            let _writer = entry.lock().unwrap_or_else(PoisonError::into_inner);
            write()
        };
        let mut map = self.map();
        // The map's reference and ours: nobody else holds or waits.
        if Arc::strong_count(&entry) == 2 {
            map.remove(key);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{base64_encode, head_sha256};

    const LOG: &str = "projects/-work/lane.jsonl";

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn receiver() -> (Dir, Receiver) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "semon-mirror-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let receiver = Receiver::new(&dir);
        (Dir(dir), receiver)
    }

    /// An append of `bytes` at `offset` onto a copy whose first bytes are
    /// `before`.
    fn append(path: &str, before: &[u8], offset: u64, bytes: &[u8], replace: bool) -> Append {
        let mut head = if replace { Vec::new() } else { before.to_vec() };
        head.extend_from_slice(bytes);
        Append {
            generation: None,
            root: "claude".into(),
            path: path.into(),
            offset,
            head_sha256: head_sha256(&head),
            bytes: base64_encode(bytes),
            replace,
        }
    }

    fn copy(receiver: &Receiver, machine: &str, relative: &str) -> Vec<u8> {
        fs::read(receiver.machine_dir(machine).join(relative)).unwrap_or_default()
    }

    fn length(reply: &Reply) -> u64 {
        reply.body["length"].as_u64().unwrap()
    }

    #[test]
    fn appends_and_replaces_are_applied_with_200() {
        let (_dir, receiver) = receiver();
        let first = b"{\"a\":1}\n";
        let reply = receiver.append("laptop", &append(LOG, b"", 0, first, false));
        assert_eq!(reply, Reply::new(200, json!({"length": first.len()})));
        let second = b"{\"b\":2}\n";
        let reply = receiver.append(
            "laptop",
            &append(LOG, first, first.len() as u64, second, false),
        );
        assert_eq!(reply.status, 200);
        assert_eq!(length(&reply), (first.len() + second.len()) as u64);
        let path = format!("claude/{LOG}");
        assert_eq!(
            copy(&receiver, "laptop", &path),
            [&first[..], &second[..]].concat()
        );

        // A replace empties the copy first, even a longer one.
        let shorter = b"{}\n";
        let reply = receiver.append("laptop", &append(LOG, b"", 0, shorter, true));
        assert_eq!(reply, Reply::new(200, json!({"length": shorter.len()})));
        assert_eq!(copy(&receiver, "laptop", &path), shorter);
        // An empty replace empties it.
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"", true));
        assert_eq!(reply, Reply::new(200, json!({"length": 0})));
        assert_eq!(copy(&receiver, "laptop", &path), b"");
        // A whole-file input, and a Codex rollout.
        let pid = b"{\"pid\":12}";
        let reply = receiver.append("laptop", &append("sessions/12.json", b"", 0, pid, true));
        assert_eq!(reply.status, 200);
        assert_eq!(copy(&receiver, "laptop", "claude/sessions/12.json"), pid);
        let mut rollout = append(
            "sessions/2026/09/29/rollout-x.jsonl",
            b"",
            0,
            b"{}\n",
            false,
        );
        rollout.root = "codex".into();
        assert_eq!(receiver.append("laptop", &rollout).status, 200);
        assert_eq!(
            copy(
                &receiver,
                "laptop",
                "codex/sessions/2026/09/29/rollout-x.jsonl"
            ),
            b"{}\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(receiver.machine_dir("laptop").join(&path)), 0o600);
            assert_eq!(
                mode(receiver.machine_dir("laptop").join("claude/projects")),
                0o700
            );
        }
    }

    #[test]
    fn a_wrong_offset_or_head_is_409_with_the_copys_length_and_head() {
        let (_dir, receiver) = receiver();
        let text: Vec<u8> = (0..500)
            .flat_map(|n| format!("{{\"line\":{n}}}\n").into_bytes())
            .collect();
        assert!(text.len() > HEAD_BYTES);
        assert_eq!(
            receiver
                .append("laptop", &append(LOG, b"", 0, &text, false))
                .status,
            200
        );
        let expected = json!({"length": text.len(), "head_sha256": head_sha256(&text)});

        // Behind, ahead, and at 0 without replace.
        for offset in [0, 10, text.len() as u64 - 1, text.len() as u64 + 1] {
            let reply = receiver.append("laptop", &append(LOG, &text, offset, b"x\n", false));
            assert_eq!(reply, Reply::new(409, expected.clone()), "offset {offset}");
        }
        // The right offset, but a head that isn't this copy's.
        let mut other = text.clone();
        other[0] = b'[';
        let reply = receiver.append(
            "laptop",
            &append(LOG, &other, text.len() as u64, b"x\n", false),
        );
        assert_eq!(reply, Reply::new(409, expected.clone()));
        // A replace whose hash doesn't cover its own bytes.
        let mut bad = append(LOG, b"", 0, b"new\n", true);
        bad.head_sha256 = head_sha256(b"old\n");
        assert_eq!(receiver.append("laptop", &bad), Reply::new(409, expected));
        assert_eq!(copy(&receiver, "laptop", &format!("claude/{LOG}")), text);

        // A missing copy has length 0 and the empty hash.
        let reply = receiver.append(
            "laptop",
            &append("projects/-w/new.jsonl", b"", 5, b"x\n", false),
        );
        assert_eq!(
            reply,
            Reply::new(409, json!({"length": 0, "head_sha256": head_sha256(b"")}))
        );
        assert!(
            !receiver
                .machine_dir("laptop")
                .join("claude/projects/-w/new.jsonl")
                .exists()
        );
    }

    #[test]
    fn bad_roots_paths_and_bodies_are_400() {
        let (dir, receiver) = receiver();
        let good = append(LOG, b"", 0, b"{}\n", false);
        let with = |change: &dyn Fn(&mut Append)| {
            let mut request = good.clone();
            change(&mut request);
            receiver.append("laptop", &request).status
        };
        assert_eq!(with(&|a| a.root = "home".into()), 400);
        assert_eq!(with(&|a| a.root = "../claude".into()), 400);
        for path in [
            "projects/../../../etc/passwd.jsonl",
            "../projects/x.jsonl",
            "/etc/x.jsonl",
            "projects\\x.jsonl",
            "projects//x.jsonl",
            "projects/./x.jsonl",
            "settings.json",
            "sessions/12.key",
            "projects/x.txt",
        ] {
            assert_eq!(with(&|a| a.path = path.into()), 400, "{path}");
        }
        assert_eq!(with(&|a| a.bytes = "not base64!".into()), 400);
        assert_eq!(with(&|a| a.head_sha256 = "ABC".into()), 400);
        assert_eq!(with(&|a| a.head_sha256 = a.head_sha256.to_uppercase()), 400);
        let mut late = append(LOG, b"", 3, b"{}\n", true);
        late.head_sha256 = head_sha256(b"{}\n");
        assert_eq!(
            receiver.append("laptop", &late).status,
            400,
            "a replace starts at 0"
        );
        assert_eq!(with(&|a| a.offset = u64::MAX), 400);
        assert_eq!(
            receiver.append("../x", &good).status,
            400,
            "a bad machine name"
        );
        // Nothing was written outside the machine's directory.
        let entries: Vec<_> = fs::read_dir(&dir.0).unwrap().collect();
        assert!(entries.is_empty(), "{entries:?}");

        let bodies: [&[u8]; 7] = [
            b"",
            b"{",
            b"[]",
            b"null",
            b"{\"root\":\"claude\"}",
            b"{\"root\":\"claude\",\"path\":\"projects/x.jsonl\",\"offset\":-1,\"head_sha256\":\"\",\"bytes\":\"\"}",
            b"\xff\xfe",
        ];
        for body in bodies {
            let reply = receiver.handle("laptop", Endpoint::Append, body);
            assert_eq!(reply.status, 400, "{}", String::from_utf8_lossy(body));
            assert!(reply.body["error"].is_string());
            assert_eq!(receiver.handle("laptop", Endpoint::Facts, body).status, 400);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_is_never_followed() {
        use std::os::unix::fs::symlink;
        let (dir, receiver) = receiver();
        let outside = dir.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("lane.jsonl"), b"theirs\n").unwrap();
        let machine = receiver.machine_dir("laptop");
        fs::create_dir_all(machine.join("claude")).unwrap();

        // A directory on the way that is a link.
        symlink(&outside, machine.join("claude/projects")).unwrap();
        let reply = receiver.append(
            "laptop",
            &append("projects/lane.jsonl", b"", 0, b"x\n", false),
        );
        assert_eq!(reply.status, 400, "{reply:?}");
        fs::remove_file(machine.join("claude/projects")).unwrap();

        // The file itself a link, to an existing file or to none.
        fs::create_dir_all(machine.join("claude/projects/-w")).unwrap();
        symlink(
            outside.join("lane.jsonl"),
            machine.join("claude/projects/-w/a.jsonl"),
        )
        .unwrap();
        symlink(
            outside.join("new.jsonl"),
            machine.join("claude/projects/-w/b.jsonl"),
        )
        .unwrap();
        for (name, offset) in [("a", 7), ("a", 0), ("b", 0)] {
            let path = format!("projects/-w/{name}.jsonl");
            for replace in [false, true] {
                let offset = if replace { 0 } else { offset };
                let before: &[u8] = if offset == 7 { b"theirs\n" } else { b"" };
                let reply =
                    receiver.append("laptop", &append(&path, before, offset, b"x\n", replace));
                assert_eq!(reply.status, 400, "{path} replace={replace}: {reply:?}");
            }
        }
        assert_eq!(fs::read(outside.join("lane.jsonl")).unwrap(), b"theirs\n");
        assert!(!outside.join("new.jsonl").exists());

        // The machine's own directory a link.
        symlink(&outside, receiver.machine_dir("desk")).unwrap();
        let reply = receiver.append("desk", &append(LOG, b"", 0, b"x\n", false));
        assert_eq!(reply.status, 400);
        let facts = Facts {
            version: 2,
            hostname: "desk".into(),
            ..Facts::default()
        };
        assert_eq!(receiver.facts("desk", &facts).status, 400);
        assert!(!outside.join(FACTS_FILE).exists());
    }

    #[test]
    fn more_than_4_mib_of_file_bytes_is_413() {
        let (_dir, receiver) = receiver();
        let bytes = vec![b'x'; CHUNK_BYTES + 1];
        let reply = receiver.append("laptop", &append(LOG, b"", 0, &bytes, false));
        assert_eq!(reply.status, 413);
        let reply = receiver.handle("laptop", Endpoint::Append, &vec![b' '; MAX_BODY_BYTES + 1]);
        assert_eq!(reply.status, 413);
        // Exactly 4 MiB fits in the body cap.
        let bytes = vec![b'x'; CHUNK_BYTES];
        let body = serde_json::to_vec(&append(LOG, b"", 0, &bytes, false)).unwrap();
        assert!(body.len() <= MAX_BODY_BYTES);
        let reply = receiver.handle("laptop", Endpoint::Append, &body);
        assert_eq!(reply, Reply::new(200, json!({"length": CHUNK_BYTES})));
    }

    #[test]
    fn a_machine_writes_only_its_own_copy() {
        let (_dir, receiver) = receiver();
        assert_eq!(
            receiver
                .append("desk", &append(LOG, b"", 0, b"desk\n", false))
                .status,
            200
        );
        // The same path from another machine is another file.
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"laptop\n", false));
        assert_eq!(reply, Reply::new(200, json!({"length": 7})));
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"x\n", true));
        assert_eq!(reply.status, 200);
        let path = format!("claude/{LOG}");
        assert_eq!(copy(&receiver, "desk", &path), b"desk\n");
        assert_eq!(copy(&receiver, "laptop", &path), b"x\n");
    }

    #[test]
    fn facts_round_trip_and_are_replaced_whole() {
        let (_dir, receiver) = receiver();
        let facts = Facts {
            version: 2,
            hostname: "laptop".into(),
            home: Some("/home/me".into()),
            proc_starts: [(4242, 7788)].into(),
            codex_locks: [("thread".to_owned(), 4250)].into(),
            repos: [
                ("/home/me/work/app".to_owned(), Some("app".to_owned())),
                ("/tmp".to_owned(), None),
            ]
            .into(),
            ..Facts::default()
        };
        let body = serde_json::to_vec(&facts).unwrap();
        let reply = receiver.handle("laptop", Endpoint::Facts, &body);
        assert_eq!(reply, Reply::new(200, json!({})));
        let path = receiver.machine_dir("laptop").join(FACTS_FILE);
        assert_eq!(semon_sessions::read_facts(&path).unwrap(), facts);

        let later = Facts {
            proc_starts: Default::default(),
            ..facts
        };
        assert_eq!(receiver.facts("laptop", &later).status, 200);
        assert_eq!(semon_sessions::read_facts(&path).unwrap(), later);
        let mut leftovers: Vec<_> = fs::read_dir(receiver.machine_dir("laptop"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        leftovers.sort();
        #[cfg(unix)]
        let expected = [FACTS_FILE, "facts.json.sources.sqlite"].as_slice();
        #[cfg(not(unix))]
        let expected = [FACTS_FILE].as_slice();
        assert_eq!(
            leftovers, expected,
            "only the original and derived projection remain"
        );
    }

    #[test]
    fn a_write_that_fails_partway_leaves_the_copy_as_it_was() {
        let (_dir, receiver) = receiver();
        let path = format!("claude/{LOG}");
        assert_eq!(
            receiver
                .append("laptop", &append(LOG, b"", 0, b"one\n", false))
                .status,
            200
        );
        // The next write stops after two bytes and fails.
        FAULT.with(|fault| fault.set(Some(Fault::WriteAfter(2))));
        let reply = receiver.append("laptop", &append(LOG, b"one\n", 4, b"two\n", false));
        assert_eq!(reply.status, 500, "{reply:?}");
        assert_eq!(copy(&receiver, "laptop", &path), b"one\n", "cut back");
        assert_eq!(receiver.used_bytes("laptop"), 4, "not counted");
        // A new file that fails is left empty, which is length 0.
        FAULT.with(|fault| fault.set(Some(Fault::WriteAfter(1))));
        let reply = receiver.append(
            "laptop",
            &append("projects/-w/new.jsonl", b"", 0, b"x\n", false),
        );
        assert_eq!(reply.status, 500);
        assert_eq!(
            copy(&receiver, "laptop", "claude/projects/-w/new.jsonl"),
            b""
        );
        // The retry applies.
        let reply = receiver.append("laptop", &append(LOG, b"one\n", 4, b"two\n", false));
        assert_eq!(reply, Reply::new(200, json!({"length": 8})));
        assert_eq!(copy(&receiver, "laptop", &path), b"one\ntwo\n");
    }

    #[test]
    fn a_failed_sync_is_cut_back_and_a_renamed_replace_counts_as_the_new_copy() {
        let (_dir, receiver) = receiver();
        let path = format!("claude/{LOG}");
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"one\n", false));
        assert_eq!(reply.status, 200);
        // The data sync fails: the append is cut back off.
        FAULT.with(|fault| fault.set(Some(Fault::SyncData)));
        let reply = receiver.append("laptop", &append(LOG, b"one\n", 4, b"two\n", false));
        assert_eq!(reply.status, 500, "{reply:?}");
        assert_eq!(copy(&receiver, "laptop", &path), b"one\n");
        assert_eq!(receiver.used_bytes("laptop"), 4);
        // The directory sync after a replace's rename fails: 500, but the
        // new copy is in place and counted.
        FAULT.with(|fault| fault.set(Some(Fault::SyncDir)));
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"replaced!\n", true));
        assert_eq!(reply.status, 500, "{reply:?}");
        assert_eq!(copy(&receiver, "laptop", &path), b"replaced!\n");
        assert_eq!(receiver.used_bytes("laptop"), 10);
        FAULT.with(|fault| fault.set(Some(Fault::SyncDir)));
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"x\n", true));
        assert_eq!(reply.status, 500);
        assert_eq!(copy(&receiver, "laptop", &path), b"x\n");
        assert_eq!(receiver.used_bytes("laptop"), 2);
        // A replace whose write fails before the rename leaves the copy,
        // its count, and no temporary file.
        FAULT.with(|fault| fault.set(Some(Fault::WriteAfter(3))));
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"longer\n", true));
        assert_eq!(reply.status, 500);
        assert_eq!(copy(&receiver, "laptop", &path), b"x\n");
        assert_eq!(receiver.used_bytes("laptop"), 2);
        let parent = receiver.machine_dir("laptop").join("claude/projects/-work");
        let names: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["lane.jsonl"]);
    }

    #[test]
    fn leftover_temporary_files_are_removed_at_start_and_not_counted() {
        let (dir, receiver) = receiver();
        assert_eq!(
            receiver
                .append("laptop", &append(LOG, b"", 0, b"one\n", false))
                .status,
            200
        );
        let machine = receiver.machine_dir("laptop");
        let facts = Facts {
            version: 2,
            hostname: "laptop".into(),
            ..Facts::default()
        };
        assert_eq!(receiver.facts("laptop", &facts).status, 200);
        let facts_bytes = fs::metadata(machine.join(FACTS_FILE)).unwrap().len();
        // What a crash mid-replace leaves.
        let leftovers = [
            machine.join("claude/projects/-work/.lane.jsonl.77.0.tmp"),
            machine.join(".facts.json.77.1.tmp"),
        ];
        for leftover in &leftovers {
            fs::write(leftover, vec![b'x'; 1000]).unwrap();
        }
        let again = Receiver::new(&dir.0);
        assert_eq!(again.used_bytes("laptop"), 4 + facts_bytes);
        for leftover in &leftovers {
            assert!(!leftover.exists(), "{}", leftover.display());
        }
        assert_eq!(copy(&again, "laptop", &format!("claude/{LOG}")), b"one\n");
    }

    #[test]
    fn a_machine_past_its_cap_gets_507() {
        let (dir, receiver) = receiver();
        let receiver = receiver.with_max_bytes(10);
        let path = format!("claude/{LOG}");
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"12345678", false));
        assert_eq!(reply.status, 200);
        let reply = receiver.append("laptop", &append(LOG, b"12345678", 8, b"abcd", false));
        assert_eq!(reply.status, 507, "{reply:?}");
        assert!(reply.body["error"].as_str().unwrap().contains("cap"));
        assert_eq!(copy(&receiver, "laptop", &path), b"12345678");
        // Another machine has its own cap.
        assert_eq!(
            receiver
                .append("desk", &append(LOG, b"", 0, b"12345678", false))
                .status,
            200
        );
        // A replace that shrinks the copy is always taken, and frees room.
        let reply = receiver.append("laptop", &append(LOG, b"", 0, b"12", true));
        assert_eq!(reply.status, 200);
        assert_eq!(receiver.used_bytes("laptop"), 2);
        let reply = receiver.append("laptop", &append(LOG, b"12", 2, b"abcdefgh", false));
        assert_eq!(reply, Reply::new(200, json!({"length": 10})));
        // A new receiver counts what is already there.
        let again = Receiver::new(&dir.0).with_max_bytes(10);
        assert_eq!(again.used_bytes("laptop"), 10);
        assert_eq!(again.used_bytes("desk"), 8);
        let reply = again.append("laptop", &append(LOG, b"12abcdefgh", 10, b"!", false));
        assert_eq!(reply.status, 507);
        // Facts count too.
        let facts = Facts {
            version: 2,
            hostname: "x".repeat(64),
            ..Facts::default()
        };
        assert_eq!(again.facts("desk", &facts).status, 507);
    }

    #[test]
    fn appends_to_one_file_are_serialized() {
        let (_dir, receiver) = receiver();
        let receiver = Arc::new(receiver);
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let receiver = Arc::clone(&receiver);
                std::thread::spawn(move || {
                    let line = format!("{{\"w\":{worker}}}\n").into_bytes();
                    let mut applied = 0;
                    while applied < 20 {
                        let path = receiver.machine_dir("laptop").join(format!("claude/{LOG}"));
                        let current = fs::read(&path).unwrap_or_default();
                        let request = append(LOG, &current, current.len() as u64, &line, false);
                        let reply = receiver.append("laptop", &request);
                        match reply.status {
                            200 => applied += 1,
                            409 => {}
                            status => panic!("{status}: {reply:?}"),
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let copy = copy(&receiver, "laptop", &format!("claude/{LOG}"));
        let lines: Vec<&[u8]> = copy.split_inclusive(|byte| *byte == b'\n').collect();
        assert_eq!(
            lines.len(),
            8 * 20,
            "every applied append is whole, none lost"
        );
        assert!(
            receiver.locks.map().is_empty(),
            "no lock outlives its writers"
        );
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use crate::wire::{Status, SyncPhase, SyncTarget, base64_encode, head_sha256};

    fn fixture() -> (PathBuf, Receiver) {
        let dir = std::env::temp_dir().join(format!(
            "semon-status-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let receiver = Receiver::new(&dir);
        (dir, receiver)
    }
    fn observation(bytes: &[u8], generation: &str) -> Status {
        Status {
            version: 1,
            observation_id: "a".repeat(32),
            sequence: 1,
            observed_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
            phase: SyncPhase::Syncing,
            runtime: Facts {
                version: semon_sessions::FACTS_VERSION,
                ..Facts::default()
            },
            inventory_complete: true,
            targets: vec![SyncTarget {
                root: "codex".into(),
                path: "sessions/native.jsonl".into(),
                generation: generation.into(),
                target_bytes: bytes.len() as u64,
                acked_bytes: bytes.len() as u64,
                head_sha256: head_sha256(bytes),
            }],
        }
    }
    fn request(bytes: &[u8], generation: &str) -> Append {
        Append {
            root: "codex".into(),
            path: "sessions/native.jsonl".into(),
            generation: Some(generation.into()),
            offset: 0,
            replace: true,
            bytes: base64_encode(bytes),
            head_sha256: head_sha256(bytes),
        }
    }
    #[test]
    fn proof_is_generation_bound_and_durable_and_never_commits_facts() {
        let (dir, receiver) = fixture();
        let bytes = b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"native-id\"}}\n";
        let generation = "b".repeat(32);
        let append = request(bytes, &generation);
        assert_eq!(
            receiver.append("laptop", &append).body["generation"],
            generation
        );
        let status = observation(bytes, &generation);
        assert_eq!(
            receiver.status("laptop", &status).body["confirmed"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(!receiver.machine_dir("laptop").join(FACTS_FILE).exists());
        let resumed = Receiver::new(&dir);
        let mut next = status.clone();
        next.sequence += 1;
        next.observed_at_ms += 1;
        assert_eq!(
            resumed.status("laptop", &next).body["confirmed"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        next.sequence += 1;
        next.observed_at_ms += 1;
        next.targets[0].generation = "c".repeat(32);
        assert!(
            resumed.status("laptop", &next).body["confirmed"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn interrupted_metadata_commit_and_same_head_rewrite_fail_closed() {
        let (dir, receiver) = fixture();
        let bytes = b"{}\n";
        let generation = "b".repeat(32);
        receiver.append("laptop", &request(bytes, &generation));
        // Simulate a crash after durable bytes but before the generation marker.
        let marker = receiver
            .machine_dir("laptop")
            .join("sync-generations")
            .join(format!(
                "{}.json",
                sha256_hex(b"codex/sessions/native.jsonl")
            ));
        fs::remove_file(&marker).unwrap();
        let status = observation(bytes, &generation);
        assert!(
            Receiver::new(&dir).status("laptop", &status).body["confirmed"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        receiver.append("laptop", &request(bytes, &generation));
        // Replacing a copy with identical bytes still changes its identity.
        let path = receiver
            .machine_dir("laptop")
            .join("codex/sessions/native.jsonl");
        fs::rename(&path, path.with_extension("retained")).unwrap();
        fs::write(&path, bytes).unwrap();
        let mut next = status;
        next.sequence += 1;
        next.observed_at_ms += 1;
        assert!(
            receiver.status("laptop", &next).body["confirmed"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn stale_heartbeats_cannot_restore_old_run_liveness_or_unknown_inventory() {
        let (dir, receiver) = fixture();
        let mut status = observation(b"{}\n", &"b".repeat(32));
        assert_eq!(receiver.status("laptop", &status).status, 200);
        assert_eq!(receiver.status("laptop", &status).status, 409);
        status.observation_id = "d".repeat(32);
        status.observed_at_ms += 1;
        assert_eq!(receiver.status("laptop", &status).status, 200);
        status.observation_id = "a".repeat(32);
        status.sequence += 1;
        status.observed_at_ms += 1;
        assert_eq!(receiver.status("laptop", &status).status, 409);
        status.observation_id = "d".repeat(32);
        status.runtime.codex_rollouts = Some(Default::default());
        assert_eq!(receiver.status("laptop", &status).status, 400);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn full_facts_fences_delayed_status_and_permits_later_verified_source_changes() {
        let (dir, receiver) = fixture();
        let bytes = b"{}\n";
        let generation = "b".repeat(32);
        receiver.append("laptop", &request(bytes, &generation));
        let status = observation(bytes, &generation);
        assert_eq!(receiver.status("laptop", &status).status, 200);
        let facts = Facts {
            version: semon_sessions::FACTS_VERSION,
            codex_rollouts: Some(Default::default()),
            mirror_observation_id: Some(status.observation_id.clone()),
            mirror_sequence: Some(2),
            ..Facts::default()
        };
        assert_eq!(receiver.facts("laptop", &facts).status, 200);
        let persisted: StoredStatus = serde_json::from_slice(
            &fs::read(receiver.machine_dir("laptop").join("status.json")).unwrap(),
        )
        .unwrap();
        assert!(persisted.confirmed.is_empty());
        let mut delayed = status.clone();
        delayed.observed_at_ms += 1;
        assert_eq!(receiver.status("laptop", &delayed).status, 409);
        let mut archive = request(bytes, &"c".repeat(32));
        archive.path = "archived_sessions/native.jsonl".into();
        assert_eq!(receiver.append("laptop", &archive).status, 200);
        let mut later = status;
        later.sequence = 3;
        later.observed_at_ms += 2;
        later.targets[0].path = archive.path;
        later.targets[0].generation = archive.generation.unwrap();
        assert_eq!(
            receiver.status("laptop", &later).body["confirmed"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let committed: Facts = serde_json::from_slice(
            &fs::read(receiver.machine_dir("laptop").join(FACTS_FILE)).unwrap(),
        )
        .unwrap();
        assert!(committed.codex_rollouts.unwrap().is_empty());
        let orphan = Facts {
            mirror_observation_id: Some(later.observation_id.clone()),
            ..Facts::default()
        };
        assert_eq!(receiver.facts("laptop", &orphan).status, 400);
        later.sequence = 4;
        later.runtime.mirror_sequence = Some(1);
        assert_eq!(receiver.status("laptop", &later).status, 400);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn generation_and_status_metadata_obey_quota_and_refused_growth_keeps_proof() {
        let (dir, receiver) = fixture();
        let receiver = receiver.with_max_bytes(4096);
        let generation = "b".repeat(32);
        let bytes = b"{}\n";
        assert_eq!(
            receiver
                .append("laptop", &request(bytes, &generation))
                .status,
            200
        );
        let before = receiver.used_bytes("laptop");
        assert!(before > bytes.len() as u64);
        assert_eq!(Receiver::new(&dir).used_bytes("laptop"), before);
        let mut grow = request(&vec![b'x'; 4096], &generation);
        grow.offset = 3;
        grow.replace = false;
        grow.head_sha256 = head_sha256(&[bytes.as_slice(), &vec![b'x'; 4096]].concat());
        assert_eq!(receiver.append("laptop", &grow).status, 507);
        assert_eq!(receiver.used_bytes("laptop"), before);
        assert_eq!(
            receiver
                .status("laptop", &observation(bytes, &generation))
                .body["confirmed"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            Receiver::new(&dir).used_bytes("laptop"),
            receiver.used_bytes("laptop")
        );
        let zero = receiver.with_max_bytes(0);
        assert_eq!(zero.append("empty", &request(b"", &generation)).status, 507);
        let mut status = observation(b"", &generation);
        status.targets.clear();
        assert_eq!(zero.status("empty", &status).status, 507);
        assert_eq!(zero.used_bytes("empty"), 0);
        assert!(!zero.machine_dir("empty").join("status.json").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_line_intermediate_bytes_and_duplicate_targets_are_unproved() {
        let (dir, receiver) = fixture();
        let bytes = b"{\"partial\":";
        let generation = "b".repeat(32);
        receiver.append("laptop", &request(bytes, &generation));
        let mut status = observation(bytes, &generation);
        assert!(
            receiver.status("laptop", &status).body["confirmed"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        status.targets.push(status.targets[0].clone());
        assert_eq!(receiver.status("laptop", &status).status, 400);
        fs::remove_dir_all(dir).unwrap();
    }
}

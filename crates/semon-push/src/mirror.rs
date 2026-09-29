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
//! it is given, and an append that fails partway is cut back off, so the
//! copy is as it was. Appends to one file are serialized; different files
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
use serde_json::{Value, json};

use crate::wire::{Append, CHUNK_BYTES, HEAD_BYTES, base64_decode, sha256_hex};

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
}

impl Endpoint {
    /// The endpoint an HTTP request path names, exactly.
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "/v1/mirror/append" => Some(Self::Append),
            "/v1/mirror/facts" => Some(Self::Facts),
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
    /// counted, and `shrink` bytes are released, only if it succeeds.
    fn counted<T>(
        &self,
        machine: &str,
        grow: u64,
        shrink: u64,
        write: impl FnOnce() -> Result<T, Refusal>,
    ) -> Result<T, Refusal> {
        self.reserve(machine, grow)?;
        let result = write();
        self.release(machine, if result.is_ok() { shrink } else { grow });
        result
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
        let conflict = || {
            Reply::new(
                409,
                json!({"length": length, "head_sha256": sha256_hex(&head)}),
            )
        };
        if !request.replace && request.offset != length {
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
        let (grow, shrink) = (
            new_length.saturating_sub(length),
            length.saturating_sub(new_length),
        );
        self.counted(machine, grow, shrink, || {
            if request.replace {
                replace_file(&parent, name, bytes)?;
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
                )?;
            }
            Ok(())
        })?;
        Ok(Reply::new(200, json!({"length": new_length})))
    }

    fn try_facts(&self, machine: &str, facts: &Facts) -> Result<Reply, Refusal> {
        check_machine(machine)?;
        let bytes =
            serde_json::to_vec(facts).map_err(|error| Refusal::Invalid(error.to_string()))?;
        self.locks.with(&format!("{machine}/{FACTS_FILE}"), || {
            self.write_facts(machine, &bytes)
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
            || Ok(replace_file(&dir, FACTS_FILE, bytes)?),
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
/// cut back to `offset`, so a partial write never stays.
fn append_durably(
    file: &mut File,
    offset: u64,
    bytes: &[u8],
    created_in: Option<&Path>,
) -> io::Result<()> {
    let result = (|| {
        file.seek(SeekFrom::Start(offset))?;
        write_bytes(file, bytes)?;
        file.sync_data()?;
        if let Some(parent) = created_in {
            sync_dir(parent)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = file.set_len(offset).and_then(|()| file.sync_data());
    }
    result
}

#[cfg(test)]
thread_local! {
    /// Makes the next write stop after this many bytes and fail.
    static FAIL_WRITE_AFTER: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

fn write_bytes(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    #[cfg(test)]
    {
        if let Some(limit) = FAIL_WRITE_AFTER.with(std::cell::Cell::take) {
            file.write_all(&bytes[..limit.min(bytes.len())])?;
            return Err(io::Error::other("an injected write failure"));
        }
    }
    file.write_all(bytes)
}

/// Each machine's bytes under `machines`: its `claude/` and `codex/` trees
/// and its facts, never following a link.
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
        let bytes: u64 = ["claude", "codex", FACTS_FILE]
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
/// renamed over it, and the directory synced.
fn replace_file(parent: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = create_new(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, parent.join(name))?;
        sync_dir(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Makes a directory's entries durable.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
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
        let leftovers: Vec<_> = fs::read_dir(receiver.machine_dir("laptop"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, [FACTS_FILE], "no temporary file is left");
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
        FAIL_WRITE_AFTER.with(|limit| limit.set(Some(2)));
        let reply = receiver.append("laptop", &append(LOG, b"one\n", 4, b"two\n", false));
        assert_eq!(reply.status, 500, "{reply:?}");
        assert_eq!(copy(&receiver, "laptop", &path), b"one\n", "cut back");
        assert_eq!(receiver.used_bytes("laptop"), 4, "not counted");
        // A new file that fails is left empty, which is length 0.
        FAIL_WRITE_AFTER.with(|limit| limit.set(Some(1)));
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

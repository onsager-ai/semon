//! Read-only persisted Copilot CLI collection. Native homes are never modified.
//! Full raw frames reach private custody before durable cursor advancement;
//! replacements retire source-owned projections and retain previous raw evidence.
#![forbid(unsafe_code)]
use semon_store::{
    AuthoredBy, NewOccurrence, NewRawCarrierRecord, RepoSource, SemanticCore, TraceStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Native carrier label. Raw source identities never enter ordinary trace reads.
pub const CARRIER: &str = "copilot";
static TEMP: AtomicU64 = AtomicU64::new(0);

/// Capture failures: source format and custody failures stop advancement.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Filesystem failure.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Malformed private cursor state.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Private store failure.
    #[error(transparent)]
    Store(#[from] semon_store::StoreError),
    /// Invalid collector configuration or future state format.
    #[error("{0}")]
    Config(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    ordinal: u64,
    digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    version: u32,
    files: BTreeMap<String, Cursor>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            files: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: std::time::SystemTime,
    created: Option<std::time::SystemTime>,
    identity: (u64, u64, i64, i64),
}
fn stamp(file: &File) -> io::Result<Stamp> {
    let m = file.metadata()?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino(), m.ctime(), m.ctime_nsec())
    };
    #[cfg(not(unix))]
    let identity = (0, 0, 0, 0);
    Ok(Stamp {
        size: m.len(),
        modified: m.modified()?,
        created: m.created().ok(),
        identity,
    })
}

/// Restart-safe collector. One instance holds the cursor lock throughout use.
pub struct Collector {
    home: PathBuf,
    state_path: PathBuf,
    state: State,
    store: TraceStore,
    unchanged: BTreeMap<String, Stamp>,
    _lock: semon_push::StateLock,
}
impl Collector {
    /// Open private Semon state outside the native home. Source home stays read-only.
    pub fn open(home: &Path, state: &Path, store: &Path) -> Result<Self, Error> {
        if !cfg!(target_os = "linux") {
            return Err(Error::Config(
                "Copilot CLI saved-state support is validated on Linux only".into(),
            ));
        }
        reject_links(home)?;
        let home = absolute(home)?;
        for output in [state, store] {
            if absolute(output)?.starts_with(&home) {
                return Err(Error::Config(
                    "Semon cursor/store must be outside the native Copilot home".into(),
                ));
            }
        }
        private_dir(state.parent().unwrap_or(Path::new(".")))?;
        private_dir(store.parent().unwrap_or(Path::new(".")))?;
        let lock = semon_push::StateLock::acquire(state).map_err(Error::Config)?;
        let loaded = match fs::read(state) {
            Ok(bytes) => serde_json::from_slice::<State>(&bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        if loaded.version != 1 {
            return Err(Error::Config(format!(
                "unsupported Copilot cursor version {}",
                loaded.version
            )));
        }
        Ok(Self {
            home,
            state_path: state.to_owned(),
            state: loaded,
            store: TraceStore::open(store)?,
            unchanged: BTreeMap::new(),
            _lock: lock,
        })
    }
    /// One bounded pass over authoritative event files; unchanged sources are stat-only.
    pub fn collect(&mut self) -> Result<usize, Error> {
        let mut count = 0;
        for path in candidate_files(&self.home)? {
            count += self.process(&path)?;
        }
        Ok(count)
    }
    /// Read-only explicit access to the private retained store, for custody inspection.
    pub fn store(&self) -> &TraceStore {
        &self.store
    }
    fn process(&mut self, path: &Path) -> Result<usize, Error> {
        let mut file = semon_sessions::open_read_only_input(path)?;
        let current = stamp(&file)?;
        let key = path.to_string_lossy().into_owned();
        if self.unchanged.get(&key) == Some(&current) {
            return Ok(0);
        }
        let id = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::Config("invalid Copilot session directory".into()))?;
        let mut saved = self.state.files.get(&key).cloned().unwrap_or_default();
        let mut prefix = Sha256::new();
        if saved.offset <= current.size {
            io::copy(&mut (&mut file).take(saved.offset), &mut prefix)?;
        }
        let digest = format!("{:x}", prefix.clone().finalize());
        let matches = saved.offset <= current.size
            && (saved.offset == 0 || saved.digest == digest)
            && self
                .store
                .capture_source_cursor_matches(CARRIER, &key, saved.offset, &digest)?;
        if !matches {
            self.store.reset_capture_source(CARRIER, &key)?;
            saved = Cursor::default();
            prefix = Sha256::new();
            self.state.files.insert(key.clone(), saved.clone());
            save(&self.state_path, &self.state)?;
        }
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(&mut file);
        let mut first = Vec::new();
        reader.read_until(b'\n', &mut first)?;
        if !first.ends_with(b"\n") {
            self.unchanged.insert(key, current);
            return Ok(0);
        }
        semon_sessions::copilot::validate_start(&serde_json::from_slice::<Value>(&first)?, id)?;
        drop(reader);
        file.seek(SeekFrom::Start(0))?;
        // Exact persisted parent event identities are rederived only when growing.
        // Ambiguous identities are mapped to None, never the latest occurrence.
        let mut parents: BTreeMap<String, Option<i64>> = BTreeMap::new();
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new((&mut file).take(saved.offset));
        let mut ordinal = 0u64;
        loop {
            let mut line = Vec::new();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if let Ok(record) = serde_json::from_slice::<Value>(&line) {
                remember(
                    &mut parents,
                    &record,
                    project(&record).first().map(|_| sequence(ordinal, 0)),
                );
            }
            ordinal += 1;
        }
        drop(reader);
        file.seek(SeekFrom::Start(saved.offset))?;
        let session = format!("copilot:{id}");
        let mut reader = BufReader::new(&mut file);
        let mut count = 0;
        // Batches cap cursor write amplification without risking uncommitted raw bytes.
        while count < 1000 {
            let mut raw = Vec::new();
            let length = reader.read_until(b'\n', &mut raw)?;
            if length == 0 || !raw.ends_with(b"\n") {
                break;
            }
            let record = serde_json::from_slice::<Value>(&raw)
                .ok()
                .filter(Value::is_object);
            let time = record
                .as_ref()
                .and_then(|r| r.get("timestamp"))
                .and_then(Value::as_str)
                .and_then(parse_time)
                .unwrap_or(0);
            let mut blocks = record.as_ref().map(project).unwrap_or_default();
            let duplicate = record
                .as_ref()
                .and_then(|r| r["id"].as_str())
                .is_some_and(|id| parents.contains_key(id));
            if duplicate {
                blocks.clear();
            }
            let parent = record
                .as_ref()
                .and_then(|r| r["parentId"].as_str())
                .and_then(|id| parents.get(id))
                .copied()
                .flatten();
            let cores: Vec<_> = blocks
                .iter()
                .map(|(core, _)| SemanticCore::from_value(core.clone()))
                .collect::<Result<_, _>>()?;
            let captures: Vec<_> = cores
                .iter()
                .zip(&blocks)
                .enumerate()
                .map(|(block, (core, (_, author)))| {
                    (
                        core.clone(),
                        NewOccurrence {
                            session: &session,
                            sequence: sequence(saved.ordinal, block),
                            timestamp: time,
                            repo: "",
                            repo_source: RepoSource::None,
                            // Sibling blocks share only the exact persisted
                            // parentId. Array order is not an execution edge.
                            parent_sequence: parent,
                            agent: None,
                            authored_by: *author,
                        },
                    )
                })
                .collect();
            self.store.with_capture_source(CARRIER, &key, |store| {
                store.capture_line(
                    NewRawCarrierRecord::new(CARRIER, &raw),
                    &session,
                    sequence(saved.ordinal, 0),
                    time,
                    &captures,
                )
            })?;
            if let Some(record) = record {
                remember(
                    &mut parents,
                    &record,
                    (!blocks.is_empty()).then(|| sequence(saved.ordinal, 0)),
                );
            }
            prefix.update(&raw);
            saved.offset += length as u64;
            saved.ordinal += 1;
            count += 1;
        }
        if count > 0 {
            saved.digest = format!("{:x}", prefix.finalize());
            self.store
                .checkpoint_capture_source(CARRIER, &key, saved.offset, &saved.digest)?;
            self.state.files.insert(key.clone(), saved.clone());
            save(&self.state_path, &self.state)?;
        }
        // A partial frame still requires reading on its next append; idle does not.
        if count < 1000 || saved.offset == current.size {
            self.unchanged.insert(key, current);
        }
        Ok(count)
    }
}
fn sequence(ordinal: u64, block: usize) -> i64 {
    (ordinal as i64) * 1024 + block as i64
}
fn remember(parents: &mut BTreeMap<String, Option<i64>>, record: &Value, sequence: Option<i64>) {
    if let Some(id) = record["id"].as_str() {
        parents
            .entry(id.into())
            .and_modify(|value| *value = None)
            .or_insert(sequence);
    }
}
fn parse_time(text: &str) -> Option<i64> {
    semon_sessions::copilot::timestamp_ns(text)
}
/// Discover only session-state/<id>/events.jsonl. Links and auxiliary stores are excluded.
pub fn candidate_files(home: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let root = home.join("session-state");
    if fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(files);
    }
    let entries = match fs::read_dir(&root) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("events.jsonl");
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
fn reject_links(path: &Path) -> io::Result<()> {
    let full = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut prefix = PathBuf::new();
    for component in full.components() {
        prefix.push(component);
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
            _ => {}
        }
    }
    Ok(())
}
fn absolute(path: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut tail = Vec::new();
    let mut current = path.as_path();
    while !current.exists() {
        tail.push(current.file_name().ok_or(io::ErrorKind::InvalidInput)?);
        current = current.parent().ok_or(io::ErrorKind::InvalidInput)?;
    }
    let mut full = current.canonicalize()?;
    for name in tail.into_iter().rev() {
        full.push(name);
    }
    Ok(full)
}
fn private_dir(path: &Path) -> io::Result<()> {
    let existed = path.exists();
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !existed {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}
fn save(path: &Path, state: &State) -> Result<(), Error> {
    let temp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        serde_json::to_writer(&mut file, state)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn project(record: &Value) -> Vec<(Value, AuthoredBy)> {
    let data = &record["data"];
    match record["type"].as_str() {
        Some("user.message") => data["content"]
            .as_str()
            .map(|content| {
                vec![(
                    json!({"kind":"intent","content":content}),
                    AuthoredBy::Unknown,
                )]
            })
            .unwrap_or_default(),
        Some("assistant.message") => {
            let mut blocks = Vec::new();
            if let Some(content) = data["content"].as_str().filter(|text| !text.is_empty()) {
                blocks.push((
                    json!({"kind":"outcome","content":content}),
                    AuthoredBy::Agent,
                ));
            }
            for request in data["toolRequests"]
                .as_array()
                .into_iter()
                .flatten()
                .take(1023)
            {
                if let (Some(_id), Some(name)) =
                    (request["toolCallId"].as_str(), request["name"].as_str())
                {
                    blocks.push((
                        json!({"kind":"action","action":name,"path":null}),
                        AuthoredBy::Agent,
                    ));
                }
            }
            blocks
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;

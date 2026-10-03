use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process,
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

use semon_store::RepoSource;
use serde_json::{Map, Value};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::{AdapterError, NormalizeContext};

/// State document version. Bumped from 1 to 2 alongside the occurrence
/// region: `session_repos` entries now carry a `repo_source` beside the
/// `repo` string, and each file cursor carries the additional occurrence
/// derivation state described on [`FileCursor`].
const VERSION: i64 = 2;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable cursor and cross-file repository attribution state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CursorState {
    files: BTreeMap<String, FileCursor>,
    session_repos: BTreeMap<String, (String, RepoSource)>,
}

impl CursorState {
    pub(crate) fn files(&self) -> &BTreeMap<String, FileCursor> {
        &self.files
    }
    pub(crate) fn take_file(&mut self, key: &str) -> FileCursor {
        self.files.remove(key).unwrap_or_default()
    }

    pub(crate) fn put_file(&mut self, key: String, cursor: FileCursor) {
        self.files.insert(key, cursor);
    }

    pub(crate) fn session_repos(&self) -> &BTreeMap<String, (String, RepoSource)> {
        &self.session_repos
    }

    pub(crate) fn remember_repo(&mut self, session_id: &str, repo: &str, repo_source: RepoSource) {
        self.session_repos
            .insert(session_id.to_owned(), (repo.to_owned(), repo_source));
    }

    fn from_value(value: Value) -> Result<Self, AdapterError> {
        let object = value
            .as_object()
            .ok_or_else(|| AdapterError::State("state document is not a JSON object".into()))?;
        let version = object.get("version").and_then(Value::as_i64);

        // Versions this build does not recognize split into two cases, and
        // they are not symmetric.
        //
        // An *older* (or missing/unparsable) version — an existing installed
        // adapter upgraded to this build, still carrying its version-1
        // cursor — is not an error: resetting to a fresh cursor and
        // replaying every unconsumed file from offset zero is safe *because*
        // of this very PR. The occurrence write upserts on
        // `(carrier, session, sequence)` and canonical traces are
        // content-addressed, so a from-scratch replay reproduces identical
        // rows rather than duplicating anything; it costs one re-read, not
        // a correctness risk. Erroring here (as this used to) would instead
        // wedge every upgraded installation until someone deleted the state
        // file by hand.
        //
        // A version *newer* than this build understands is a different kind
        // of unknown: a future build may have written state shapes (or
        // occurrence-derivation semantics) this build cannot correctly
        // interpret, so silently resetting could silently misbehave instead
        // of just re-reading. That stays a hard error. This mirrors
        // `TraceStore::open`'s handling of `PRAGMA user_version` in
        // `semon-store/src/store.rs`: older is recoverable, newer is not.
        match version {
            Some(version) if version > VERSION => {
                return Err(AdapterError::State(format!(
                    "unsupported state version: {version} (this build understands up to {VERSION})"
                )));
            }
            Some(VERSION) => {}
            _ => return Ok(Self::default()),
        }

        let files = match object.get("files") {
            None => BTreeMap::new(),
            Some(Value::Object(files)) => files
                .iter()
                .map(|(key, value)| Ok((key.clone(), FileCursor::from_value(value)?)))
                .collect::<Result<_, AdapterError>>()?,
            Some(_) => return Err(AdapterError::State("files is not a JSON object".into())),
        };
        let session_repos = match object.get("session_repos") {
            None => BTreeMap::new(),
            Some(Value::Object(repos)) => repos
                .iter()
                .map(|(key, value)| {
                    let entry = value.as_object().ok_or_else(|| {
                        AdapterError::State("session_repos entry is not a JSON object".into())
                    })?;
                    let repo = entry
                        .get("repo")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let repo_source = entry
                        .get("repo_source")
                        .and_then(Value::as_str)
                        .unwrap_or("none");
                    let repo_source = RepoSource::from_str(repo_source).map_err(|error| {
                        AdapterError::State(format!("invalid session_repos repo_source: {error}"))
                    })?;
                    Ok((key.clone(), (repo, repo_source)))
                })
                .collect::<Result<_, AdapterError>>()?,
            Some(_) => {
                return Err(AdapterError::State(
                    "session_repos is not a JSON object".into(),
                ));
            }
        };
        Ok(Self {
            files,
            session_repos,
        })
    }

    fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            (
                "files".into(),
                Value::Object(Map::from_iter(
                    self.files
                        .iter()
                        .map(|(key, value)| (key.clone(), value.to_value())),
                )),
            ),
            (
                "session_repos".into(),
                Value::Object(Map::from_iter(self.session_repos.iter().map(
                    |(key, (repo, repo_source))| {
                        (
                            key.clone(),
                            Value::Object(Map::from_iter([
                                ("repo".into(), Value::String(repo.clone())),
                                (
                                    "repo_source".into(),
                                    Value::String(repo_source.as_str().to_owned()),
                                ),
                            ])),
                        )
                    },
                ))),
            ),
            ("version".into(), Value::Number(VERSION.into())),
        ]))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileCursor {
    pub(crate) offset: u64,
    /// SHA-256 of exactly the complete bytes committed at `offset`.
    pub(crate) prefix_sha256: Option<String>,
    session_id: String,
    parent_session_id: Option<String>,
    agent: Option<String>,
    repo: String,
    repo_source: RepoSource,
    cwd: String,
    calls: Map<String, Value>,
    /// Latched once this file is seen to carry the `item_completed` item
    /// stream; never reset back to `false` (see `NormalizeContext`).
    has_item_stream: bool,
    /// The zero-based ordinal to assign to the next source line read from
    /// this file. This is the file's own line counter, not a write-time
    /// counter: it always equals the count of `\n`-terminated lines already
    /// consumed from the start of the file, so replaying the file from
    /// scratch (as happens on truncation, see `process_file`) regenerates
    /// identical ordinals for identical source lines.
    pub(crate) next_line_ordinal: u64,
    /// For each session id seen in this file, the `sequence` (line ordinal)
    /// of the most recent record from that session that actually projected
    /// an occurrence. Used to derive `parent_sequence`: never a "last row
    /// written" counter, but a value fully re-derivable by replaying the
    /// file from its start, which is what keeps it correct across the
    /// truncation-reset path (this map resets to empty along with the rest
    /// of the file's cursor whenever the file has shrunk below the saved
    /// offset).
    ///
    /// Keyed by session id (not a single scalar) because one file can carry
    /// more than one session's records — Codex's separate history file
    /// interleaves entries from every session.
    pub(crate) last_projected_sequence: BTreeMap<String, i64>,
}

impl Default for FileCursor {
    fn default() -> Self {
        Self {
            offset: 0,
            prefix_sha256: None,
            session_id: String::new(),
            parent_session_id: None,
            agent: None,
            repo: String::new(),
            repo_source: RepoSource::None,
            cwd: String::new(),
            calls: Map::new(),
            has_item_stream: false,
            next_line_ordinal: 0,
            last_projected_sequence: BTreeMap::new(),
        }
    }
}

impl FileCursor {
    pub(crate) fn context(&self) -> NormalizeContext {
        NormalizeContext {
            session_id: self.session_id.clone(),
            parent_session_id: self.parent_session_id.clone(),
            agent: self.agent.clone(),
            repo: self.repo.clone(),
            repo_source: self.repo_source,
            cwd: self.cwd.clone(),
            calls: self.calls.clone(),
            has_item_stream: self.has_item_stream,
        }
    }

    pub(crate) fn update_context(&mut self, context: &NormalizeContext) {
        self.session_id.clone_from(&context.session_id);
        self.parent_session_id
            .clone_from(&context.parent_session_id);
        self.agent.clone_from(&context.agent);
        self.repo.clone_from(&context.repo);
        self.repo_source = context.repo_source;
        self.cwd.clone_from(&context.cwd);
        self.calls.clone_from(&context.calls);
        self.has_item_stream = context.has_item_stream;
    }

    fn from_value(value: &Value) -> Result<Self, AdapterError> {
        let object = value
            .as_object()
            .ok_or_else(|| AdapterError::State("file cursor is not a JSON object".into()))?;
        let offset = object.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let string = |key| {
            object
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let calls = object
            .get("calls")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let has_item_stream = object
            .get("has_item_stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let repo_source = object
            .get("repo_source")
            .and_then(Value::as_str)
            .unwrap_or("none");
        let repo_source = RepoSource::from_str(repo_source)
            .map_err(|error| AdapterError::State(format!("invalid repo_source: {error}")))?;
        let next_line_ordinal = object
            .get("next_line_ordinal")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let last_projected_sequence = match object.get("last_projected_sequence") {
            None => BTreeMap::new(),
            Some(Value::Object(entries)) => entries
                .iter()
                .filter_map(|(key, value)| value.as_i64().map(|value| (key.clone(), value)))
                .collect(),
            Some(_) => {
                return Err(AdapterError::State(
                    "last_projected_sequence is not a JSON object".into(),
                ));
            }
        };
        Ok(Self {
            prefix_sha256: object
                .get("prefix_sha256")
                .and_then(Value::as_str)
                .map(str::to_owned),
            offset,
            session_id: string("session_id"),
            parent_session_id: object
                .get("parent_session_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            agent: object
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_owned),
            repo: string("repo"),
            repo_source,
            cwd: string("cwd"),
            calls,
            has_item_stream,
            next_line_ordinal,
            last_projected_sequence,
        })
    }

    fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("calls".into(), sort_json(Value::Object(self.calls.clone()))),
            ("cwd".into(), Value::String(self.cwd.clone())),
            ("has_item_stream".into(), Value::Bool(self.has_item_stream)),
            (
                "last_projected_sequence".into(),
                Value::Object(Map::from_iter(
                    self.last_projected_sequence
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::Number((*value).into()))),
                )),
            ),
            (
                "next_line_ordinal".into(),
                Value::Number(self.next_line_ordinal.into()),
            ),
            ("offset".into(), Value::Number(self.offset.into())),
            (
                "prefix_sha256".into(),
                self.prefix_sha256
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            ("repo".into(), Value::String(self.repo.clone())),
            (
                "repo_source".into(),
                Value::String(self.repo_source.as_str().to_owned()),
            ),
            ("session_id".into(), Value::String(self.session_id.clone())),
            (
                "parent_session_id".into(),
                self.parent_session_id
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            (
                "agent".into(),
                self.agent.clone().map_or(Value::Null, Value::String),
            ),
        ]))
    }
}

/// Loads cursor state, returning an empty version-one state when absent.
pub fn load_state(path: &Path) -> Result<CursorState, AdapterError> {
    match fs::read(path) {
        Ok(bytes) => CursorState::from_value(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CursorState::default()),
        Err(error) => Err(error.into()),
    }
}

/// Atomically saves cursor state beside the destination path.
pub fn save_state(path: &Path, state: &CursorState) -> Result<(), AdapterError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("state");
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{name}.{}.{}", process::id(), sequence));
    let result = (|| {
        let mut open_options = OpenOptions::new();
        open_options.write(true).create_new(true);
        #[cfg(unix)]
        open_options.mode(0o600);
        let mut file = open_options.open(&temporary)?;
        serde_json::to_writer(&mut file, &state.to_value())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok::<_, AdapterError>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(Map::from_iter(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_json(value))),
            ))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_json).collect()),
        value => value,
    }
}

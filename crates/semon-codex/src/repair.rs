use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use semon_store::{RawSessionRekeyLine, TraceStore};
use serde_json::Value;

use crate::{
    AdapterError, CARRIER, CursorState, NormalizeContext, ProcessOptions, derive_line,
    process_file_until, same_path, state,
};

/// Totals from the one-off Codex subagent key repair.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct RepairReport {
    /// Parent session groups with tracked subagent files.
    pub groups_found: usize,
    /// Groups repaired or checked in a dry run.
    pub groups_repaired: usize,
    /// Skipped parent sessions and their reasons.
    pub groups_skipped: Vec<(String, String)>,
    /// Raw rows moved to the subagent label.
    pub raw_rekeyed: usize,
    /// Misplaced raw rows removed because the new key already had exact bytes.
    pub duplicates_deleted: usize,
    /// Old Codex occurrences removed before replay.
    pub occurrences_deleted: usize,
    /// Codex occurrences present after replay.
    pub occurrences_rebuilt: usize,
}

#[derive(Clone)]
struct TrackedFile {
    path: PathBuf,
    offset: u64,
}

#[derive(Default)]
struct Group {
    parent: Option<TrackedFile>,
    subagents: Vec<TrackedFile>,
}

enum FileIdentity {
    Parent(String),
    Subagent(String),
}

fn file_identity(
    path: &Path,
    saved: &state::FileCursor,
) -> Result<Option<FileIdentity>, AdapterError> {
    let mut first = Vec::new();
    match File::open(path) {
        Ok(file) => {
            BufReader::new(file).read_until(b'\n', &mut first)?;
            let Ok(record) = serde_json::from_slice::<Value>(&first) else {
                return Ok(None);
            };
            if record.get("type").and_then(Value::as_str) != Some("session_meta") {
                return Ok(None);
            }
            let Some(payload) = record.get("payload") else {
                return Ok(None);
            };
            let parent = payload
                .get("session_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| payload.get("id").and_then(Value::as_str));
            let Some(parent) = parent else {
                return Ok(None);
            };
            let subagent = payload.get("thread_source").and_then(Value::as_str) == Some("subagent")
                && payload
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id != parent);
            Ok(Some(if subagent {
                FileIdentity::Subagent(parent.to_owned())
            } else {
                FileIdentity::Parent(parent.to_owned())
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let context = saved.context();
            let session = context.session_id();
            let (parent, thread) = match session.split_once("/agent-") {
                Some((parent, thread)) => (parent, thread),
                None => (session, session),
            };
            let stem = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if parent.is_empty() || !stem.starts_with("rollout-") {
                return Ok(None);
            }
            Ok(Some(if stem.ends_with(thread) {
                if session.contains("/agent-") {
                    FileIdentity::Subagent(parent.to_owned())
                } else {
                    FileIdentity::Parent(parent.to_owned())
                }
            } else {
                FileIdentity::Subagent(parent.to_owned())
            }))
        }
        Err(error) => Err(error.into()),
    }
}

fn complete_lines(path: &Path, offset: u64) -> Result<Vec<Vec<u8>>, AdapterError> {
    let mut reader = BufReader::new(File::open(path)?.take(offset));
    let mut consumed = 0;
    let mut lines = Vec::new();
    while consumed < offset {
        let mut line = Vec::new();
        let count = reader.read_until(b'\n', &mut line)?;
        if count == 0 || !line.ends_with(b"\n") {
            return Err(AdapterError::State(format!(
                "cursor is not at a complete line in {}",
                path.display()
            )));
        }
        consumed += count as u64;
        lines.push(line);
    }
    Ok(lines)
}

/// Repairs tracked subagent prefixes and replays each complete parent group.
/// The real cursor state is never changed, including during a dry run.
pub fn repair_subagent_keys(
    state: &CursorState,
    store: &mut TraceStore,
    history_path: &Path,
    repo_override: &str,
    dry_run: bool,
) -> Result<RepairReport, AdapterError> {
    let mut groups = BTreeMap::<String, Group>::new();
    for (path, saved) in state.files() {
        if saved.offset == 0 || same_path(Path::new(path), history_path)? {
            continue;
        }
        let Some(identity) = file_identity(Path::new(path), saved)? else {
            continue;
        };
        let file = TrackedFile {
            path: PathBuf::from(path),
            offset: saved.offset,
        };
        match identity {
            FileIdentity::Parent(parent) => {
                groups.entry(parent).or_default().parent = Some(file);
            }
            FileIdentity::Subagent(parent) => {
                groups.entry(parent).or_default().subagents.push(file);
            }
        }
    }

    let mut report = RepairReport::default();
    for (parent, mut group) in groups {
        if group.subagents.is_empty() {
            continue;
        }
        report.groups_found += 1;
        let Some(parent_file) = group.parent.take() else {
            report
                .groups_skipped
                .push((parent, "parent file is not in cursor state".into()));
            continue;
        };
        group
            .subagents
            .sort_by(|left, right| left.path.cmp(&right.path));
        let files = std::iter::once(&parent_file).chain(group.subagents.iter());
        let problem = files
            .filter_map(|file| match fs::metadata(&file.path) {
                Ok(metadata) if metadata.len() < file.offset => Some(format!(
                    "{} is shorter than its saved offset {}",
                    file.path.display(),
                    file.offset
                )),
                Ok(_) => None,
                Err(error) => Some(format!("{}: {error}", file.path.display())),
            })
            .next();
        if let Some(reason) = problem {
            report.groups_skipped.push((parent, reason));
            continue;
        }

        let mut rekeys = Vec::new();
        for file in &group.subagents {
            let mut context = NormalizeContext::default();
            for (ordinal, line) in complete_lines(&file.path, file.offset)?
                .into_iter()
                .enumerate()
            {
                let derived = derive_line(
                    &line,
                    ordinal as u64,
                    &mut context,
                    false,
                    state.session_repos(),
                    repo_override,
                );
                rekeys.push(RawSessionRekeyLine {
                    old_session: parent.clone(),
                    new_session: derived.session,
                    sequence: derived.sequence,
                    bytes: line,
                });
            }
        }
        let mut replay_state = state.clone();
        let mut replay_files = Vec::with_capacity(group.subagents.len() + 1);
        replay_files.push(parent_file);
        replay_files.extend(group.subagents);
        for file in &replay_files {
            replay_state.take_file(&file.path.to_string_lossy());
        }
        let options = ProcessOptions {
            state_path: Path::new(""),
            history_path,
            repo_override,
            batch_size: 100,
            max_batch_bytes: 1_000_000,
        };
        let counts = store.repair_session_keys(CARRIER, &parent, &rekeys, dry_run, |store| {
            for file in &replay_files {
                let source_key = crate::resolved(&file.path)?.to_string_lossy().into_owned();
                store.with_capture_source(CARRIER, &source_key, |store| {
                    process_file_until(
                        &file.path,
                        &mut replay_state,
                        store,
                        &options,
                        Some(file.offset),
                        false,
                    )
                })?;
            }
            Ok::<(), AdapterError>(())
        })?;
        report.groups_repaired += 1;
        report.raw_rekeyed += counts.raw_rekeyed;
        report.duplicates_deleted += counts.duplicates_deleted;
        report.occurrences_deleted += counts.occurrences_deleted;
        report.occurrences_rebuilt += counts.occurrences_rebuilt;
    }
    Ok(report)
}

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    sync::atomic::{AtomicU64, Ordering},
};

use age::x25519;
use serde_json::{Map, Value};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::{
    CryptoError, DataKey, FRAME_PAGE_MAX_BYTES, FRAME_PAGE_MAX_FRAMES, FrameContent, FrameMode,
    OrphanSummary, RelayError, StreamTip, TakeoverCommandError, TakeoverResult, Transport,
    TransportError, VerifyError, ZERO_CHAIN, chain_line, decrypt_envelope,
    initialize_takeover_state, sender::verify_encrypted_frame, takeover_session,
    takeover_session_encrypted,
};

const TAKEOVER_PREFLIGHT_RETRIES: usize = 8;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoredStream {
    pub stream: String,
    pub generation: u64,
    pub lines: u64,
    pub lines_written: u64,
    pub truncated_at_seq: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnfinishedToolCall {
    pub name: String,
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitBranchCheck {
    pub expected: String,
    pub actual: Option<String>,
    pub matches: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreReport {
    pub session: String,
    pub harness: String,
    pub codex_home: Option<PathBuf>,
    pub cwd: PathBuf,
    pub streams: Vec<RestoredStream>,
    pub lines: u64,
    pub lines_written: u64,
    pub new_epoch: Option<u64>,
    pub recorded_cwd: Option<String>,
    pub recorded_git_branch: Option<String>,
    pub git_branch_check: Option<GitBranchCheck>,
    pub unfinished_tool_calls: Vec<UnfinishedToolCall>,
    pub orphans: Vec<OrphanSummary>,
    pub incomplete: bool,
}

impl RestoreReport {
    pub fn next_step(&self) -> Option<String> {
        (!self.incomplete).then(|| {
            let command = if let Some(home) = &self.codex_home {
                format!(
                    "CODEX_HOME={} codex resume",
                    shell_quote(&home.to_string_lossy())
                )
            } else {
                "claude --resume".into()
            };
            format!(
                "cd {} && {} {}",
                shell_quote(&self.cwd.to_string_lossy()),
                command,
                shell_quote(&self.session)
            )
        })
    }

    pub fn to_json(&self) -> Value {
        let streams = self
            .streams
            .iter()
            .map(|stream| {
                Value::Object(Map::from_iter([
                    ("generation".into(), stream.generation.into()),
                    ("lines".into(), stream.lines.into()),
                    ("lines_written".into(), stream.lines_written.into()),
                    ("stream".into(), Value::String(stream.stream.clone())),
                    (
                        "truncated_at_seq".into(),
                        stream.truncated_at_seq.map_or(Value::Null, Value::from),
                    ),
                ]))
            })
            .collect();
        let unfinished = self
            .unfinished_tool_calls
            .iter()
            .map(|call| {
                Value::Object(Map::from_iter([
                    ("id".into(), Value::String(call.id.clone())),
                    ("name".into(), Value::String(call.name.clone())),
                ]))
            })
            .collect();
        let orphans = self
            .orphans
            .iter()
            .map(|orphan| {
                Value::Object(Map::from_iter([
                    ("fenced_epoch".into(), orphan.fenced_epoch.into()),
                    ("first_seq".into(), orphan.first_seq.into()),
                    ("frames".into(), orphan.frames.into()),
                    ("generation".into(), orphan.generation.into()),
                    ("last_seq".into(), orphan.last_seq.into()),
                    ("stream".into(), Value::String(orphan.stream.clone())),
                ]))
            })
            .collect();
        let branch_check = self.git_branch_check.as_ref().map_or(Value::Null, |check| {
            Value::Object(Map::from_iter([
                (
                    "actual".into(),
                    check
                        .actual
                        .as_ref()
                        .map_or(Value::Null, |value| Value::String(value.clone())),
                ),
                ("expected".into(), Value::String(check.expected.clone())),
                ("matches".into(), Value::Bool(check.matches)),
            ]))
        });
        Value::Object(Map::from_iter([
            (
                "cwd".into(),
                Value::String(self.cwd.to_string_lossy().into_owned()),
            ),
            ("git_branch_check".into(), branch_check),
            ("incomplete".into(), Value::Bool(self.incomplete)),
            ("lines".into(), self.lines.into()),
            ("lines_written".into(), self.lines_written.into()),
            (
                "new_epoch".into(),
                self.new_epoch.map_or(Value::Null, Value::from),
            ),
            (
                "next_step".into(),
                self.next_step().map_or(Value::Null, Value::String),
            ),
            ("orphans".into(), Value::Array(orphans)),
            (
                "recorded_cwd".into(),
                self.recorded_cwd
                    .as_ref()
                    .map_or(Value::Null, |value| Value::String(value.clone())),
            ),
            (
                "recorded_git_branch".into(),
                self.recorded_git_branch
                    .as_ref()
                    .map_or(Value::Null, |value| Value::String(value.clone())),
            ),
            ("session".into(), Value::String(self.session.clone())),
            ("harness".into(), Value::String(self.harness.clone())),
            (
                "codex_home".into(),
                self.codex_home.as_ref().map_or(Value::Null, |home| {
                    Value::String(home.to_string_lossy().into_owned())
                }),
            ),
            (
                "compatibility".into(),
                if self.harness == "codex" {
                    Value::String("synthetic rollout-only resume tested with codex-cli 0.159.0-alpha.3; complex threads and older versions untested; SQLite state is not restored".into())
                } else {
                    Value::Null
                },
            ),
            ("streams".into(), Value::Array(streams)),
            ("unfinished_tool_calls".into(), Value::Array(unfinished)),
            (
                "not_restored".into(),
                Value::Array(
                    (if self.harness == "codex" {
                        vec![
                            "SQLite databases",
                            "authentication and configuration",
                            "attachments and memory",
                        ]
                    } else {
                        vec!["*.meta.json", "custom-title.json"]
                    })
                    .into_iter()
                    .map(|value| Value::String(value.into()))
                    .collect(),
                ),
            ),
            (
                "standing_limits".into(),
                Value::Array(
                    [
                        "uncommitted worktree changes are not restored",
                        "the old agent may still be running",
                        "the old agent's git side effects are not fenced",
                    ]
                    .into_iter()
                    .map(|value| Value::String(value.into()))
                    .collect(),
                ),
            ),
        ]))
    }

    pub fn to_text(&self) -> String {
        let mut lines = vec![format!(
            "restore session={} complete={} streams={} lines={} lines_written={} epoch={}",
            self.session,
            !self.incomplete,
            self.streams.len(),
            self.lines,
            self.lines_written,
            self.new_epoch
                .map_or_else(|| "none".into(), |epoch| epoch.to_string())
        )];
        for stream in &self.streams {
            let truncated = stream
                .truncated_at_seq
                .map_or_else(String::new, |seq| format!(" truncated_at_seq={seq}"));
            lines.push(format!(
                "restored stream={} generation={} lines={} lines_written={}{}",
                stream.stream, stream.generation, stream.lines, stream.lines_written, truncated
            ));
        }
        if self.harness == "codex" {
            lines.push("Codex compatibility: synthetic rollout-only resume tested with 0.159.0-alpha.3; SQLite indexes are not restored or overwritten; prefer a fresh CODEX_HOME to avoid stale rollout paths".into());
        }
        lines.push(format!(
            "recorded cwd={} gitBranch={}",
            display_optional(&self.recorded_cwd),
            display_optional(&self.recorded_git_branch)
        ));
        if let Some(check) = &self.git_branch_check
            && !check.matches
        {
            lines.push(format!(
                "WARNING: target cwd is not a git worktree on recorded branch {}; actual branch={}",
                check.expected,
                display_optional(&check.actual)
            ));
        }
        for call in &self.unfinished_tool_calls {
            lines.push(format!("unfinished tool name={} id={}", call.name, call.id));
        }
        for orphan in &self.orphans {
            lines.push(format!(
                "orphan stream={} generation={} fenced_epoch={} frames={} first_seq={} last_seq={}",
                orphan.stream,
                orphan.generation,
                orphan.fenced_epoch,
                orphan.frames,
                orphan.first_seq,
                orphan.last_seq
            ));
        }
        lines.push(if self.harness == "codex" { "not restored: SQLite databases, authentication, configuration, attachments and memory" } else { "not restored: *.meta.json and custom-title.json" }.into());
        lines.push("limit: uncommitted worktree changes are not restored".into());
        lines.push("limit: the old agent may still be running".into());
        lines.push("limit: the old agent's git side effects are not fenced".into());
        if self.incomplete {
            lines.push(
                "restore stopped at one or more gaps; takeover was not attempted and the session must not be resumed"
                    .into(),
            );
        } else if let Some(next_step) = self.next_step() {
            lines.push(next_step);
        }
        lines.join("\n")
    }
}

fn display_optional(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("unknown")
}

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("session id is not a safe file name: {0}")]
    Session(String),
    #[error("receiver returned no live frames for session {0}")]
    NoFrames(String),
    #[error("receiver returned a frame for session {actual} while restoring {expected}")]
    WrongSession { expected: String, actual: String },
    #[error("receiver returned unsupported restore stream {0}")]
    Stream(String),
    #[error("plaintext frame found while restoring encrypted session {0}")]
    Plaintext(String),
    #[error("encrypted frame found while restoring loopback-plaintext session {0}")]
    Encrypted(String),
    #[error("sequence gap for {stream} generation {generation}: expected {expected}, got {actual}")]
    Gap {
        stream: String,
        generation: u64,
        expected: u64,
        actual: u64,
    },
    #[error("epoch moved backwards for {stream} generation {generation} at sequence {seq}")]
    Epoch {
        stream: String,
        generation: u64,
        seq: u64,
    },
    #[error("chain break for {stream} generation {generation} at sequence {seq}")]
    Chain {
        stream: String,
        generation: u64,
        seq: u64,
    },
    #[error("cannot decrypt {stream} generation {generation} sequence {seq}: {source}")]
    Decrypt {
        stream: String,
        generation: u64,
        seq: u64,
        source: Box<CryptoError>,
    },
    #[error("cannot retrieve {what} for session {session}: {source}")]
    Transport {
        what: &'static str,
        session: String,
        source: TransportError,
    },
    #[error("session {0} has no data-key envelope")]
    EnvelopeMissing(String),
    #[error("cannot unwrap session data key: {0}")]
    Envelope(#[source] Box<CryptoError>),
    #[error("encrypted frame verification failed: {0}")]
    Verification(String),
    #[error("invalid paginated frame response: {0}")]
    Page(String),
    #[error(
        "receiver tip has no verified frame for {stream} generation {generation} sequence {seq}"
    )]
    Tip {
        stream: String,
        generation: u64,
        seq: u64,
    },
    #[error("target cwd is not valid UTF-8: {0:?}")]
    Cwd(PathBuf),
    #[error("cannot access restore path {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("session {session} already exists under another project slug: {path}")]
    OtherSlug { session: String, path: PathBuf },
    #[error("restore target is not a regular file: {0}")]
    TargetType(PathBuf),
    #[error("existing target for stream {stream} is not a byte prefix of receiver content: {path}")]
    Prefix { stream: String, path: PathBuf },
    #[error("takeover failed: {0}")]
    Takeover(#[source] Box<TakeoverCommandError>),
    #[error("old holder kept advancing during {TAKEOVER_PREFLIGHT_RETRIES} takeover preflights")]
    TakeoverBusy,
    #[error("post-takeover lease is not held by this machine at the committed epoch")]
    PostTakeoverLease,
    #[error("post-takeover receiver tips do not match the verified restored files")]
    PostTakeoverTips,
    #[error("cannot initialize sender state after takeover: {0}")]
    State(#[source] Box<RelayError>),
}

#[derive(Clone, Copy)]
enum RestoreMode<'a> {
    Plaintext,
    Encrypted(&'a x25519::Identity),
}

struct VerifiedSession {
    streams: Vec<VerifiedStream>,
    had_gaps: bool,
    metadata: TranscriptMetadata,
}

#[derive(Clone)]
struct VerifiedStream {
    stream: String,
    generation: u64,
    tip: Option<StreamTip>,
    truncated_at_seq: Option<u64>,
}

struct StagedStream {
    temporary: PathBuf,
    target: PathBuf,
    verified: VerifiedStream,
    total_lines: u64,
}

impl Drop for StagedStream {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
    }
}

struct TemporaryGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TemporaryGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn restore_session(
    projects_root: &Path,
    state_path: &Path,
    cwd: &Path,
    session: &str,
    machine: &str,
    force: bool,
    allow_gaps: bool,
    transport: &impl Transport,
) -> Result<RestoreReport, RestoreError> {
    restore(
        projects_root,
        state_path,
        cwd,
        session,
        machine,
        force,
        allow_gaps,
        RestoreMode::Plaintext,
        transport,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn restore_session_encrypted(
    projects_root: &Path,
    state_path: &Path,
    cwd: &Path,
    session: &str,
    machine: &str,
    force: bool,
    allow_gaps: bool,
    identity: &x25519::Identity,
    transport: &impl Transport,
) -> Result<RestoreReport, RestoreError> {
    restore(
        projects_root,
        state_path,
        cwd,
        session,
        machine,
        force,
        allow_gaps,
        RestoreMode::Encrypted(identity),
        transport,
    )
}

#[allow(clippy::too_many_arguments)]
fn restore(
    projects_root: &Path,
    state_path: &Path,
    cwd: &Path,
    session: &str,
    machine: &str,
    force: bool,
    allow_gaps: bool,
    mode: RestoreMode<'_>,
    transport: &impl Transport,
) -> Result<RestoreReport, RestoreError> {
    validate_session(session)?;
    let initial =
        transport
            .lease_tips(session, machine)
            .map_err(|source| RestoreError::Transport {
                what: "restore layout",
                session: session.into(),
                source,
            })?;
    let codex = initial
        .tips
        .iter()
        .any(|tip| tip.stream.starts_with("codex/"));
    if codex
        && initial
            .tips
            .iter()
            .any(|tip| !tip.stream.starts_with("codex/"))
    {
        return Err(RestoreError::Stream("mixed harness streams".into()));
    }
    // Hold the local writer lock through verification, installation and takeover.
    let _writer = if codex {
        Some(lock_codex_writer(projects_root, session)?)
    } else {
        None
    };
    let cwd = absolute_path(cwd)?;
    let orphans = transport
        .list_orphans(machine)
        .map_err(|source| RestoreError::Transport {
            what: "orphans",
            session: session.to_owned(),
            source,
        })?
        .into_iter()
        .filter(|orphan| orphan.session == session)
        .collect::<Vec<_>>();
    let data_key = match mode {
        RestoreMode::Plaintext => None,
        RestoreMode::Encrypted(identity) => {
            let envelope = transport
                .get_envelope(session, machine)
                .map_err(|source| RestoreError::Transport {
                    what: "data-key envelope",
                    session: session.to_owned(),
                    source,
                })?
                .ok_or_else(|| RestoreError::EnvelopeMissing(session.to_owned()))?;
            Some(
                decrypt_envelope(&envelope, identity)
                    .map_err(|error| RestoreError::Envelope(Box::new(error)))?,
            )
        }
    };
    let (mut verified, mut write_reports) = fetch_and_materialize(
        projects_root,
        &cwd,
        session,
        machine,
        data_key.as_ref(),
        allow_gaps,
        transport,
    )?;
    if verified.had_gaps {
        return Ok(build_report(
            session,
            cwd,
            &verified,
            &write_reports,
            None,
            orphans,
            true,
        ));
    }

    let mut preflight_attempts = 0;
    let epoch = loop {
        let takeover = if let RestoreMode::Encrypted(identity) = mode {
            takeover_session_encrypted(
                projects_root,
                state_path,
                session,
                machine,
                force,
                identity,
                transport,
            )
        } else {
            takeover_session(
                projects_root,
                state_path,
                session,
                machine,
                force,
                transport,
            )
        };
        match takeover {
            Ok(takeover) => break takeover.row.epoch,
            Err(TakeoverCommandError::Precheck(RelayError::TakeoverBehind { .. }))
                if preflight_attempts < TAKEOVER_PREFLIGHT_RETRIES =>
            {
                preflight_attempts += 1;
                let fetched = fetch_and_materialize(
                    projects_root,
                    &cwd,
                    session,
                    machine,
                    data_key.as_ref(),
                    false,
                    transport,
                )?;
                verified = fetched.0;
                merge_write_reports(&mut write_reports, fetched.1);
            }
            Err(TakeoverCommandError::Precheck(RelayError::TakeoverBehind { .. })) => {
                return Err(RestoreError::TakeoverBusy);
            }
            Err(TakeoverCommandError::PostCommitBehind { epoch, .. }) => {
                let fetched = fetch_and_materialize(
                    projects_root,
                    &cwd,
                    session,
                    machine,
                    data_key.as_ref(),
                    false,
                    transport,
                )?;
                verified = fetched.0;
                merge_write_reports(&mut write_reports, fetched.1);
                initialize_after_race(
                    projects_root,
                    state_path,
                    session,
                    machine,
                    epoch,
                    &verified,
                    transport,
                )?;
                break epoch;
            }
            Err(error) => return Err(RestoreError::Takeover(Box::new(error))),
        }
    };

    Ok(build_report(
        session,
        cwd,
        &verified,
        &write_reports,
        Some(epoch),
        orphans,
        false,
    ))
}

fn fetch_and_materialize(
    projects_root: &Path,
    cwd: &Path,
    session: &str,
    machine: &str,
    data_key: Option<&DataKey>,
    allow_gaps: bool,
    transport: &impl Transport,
) -> Result<(VerifiedSession, Vec<RestoredStream>), RestoreError> {
    let receiver =
        transport
            .lease_tips(session, machine)
            .map_err(|source| RestoreError::Transport {
                what: "live frame tips",
                session: session.to_owned(),
                source,
            })?;
    let selected = select_highest_generations(&receiver.tips);
    if selected.is_empty() {
        return Err(RestoreError::NoFrames(session.to_owned()));
    }

    let codex = selected.keys().any(|stream| stream.starts_with("codex/"));
    if codex && (selected.len() != 1 || selected.keys().any(|s| !s.starts_with("codex/"))) {
        return Err(RestoreError::Stream(
            "Codex requires exactly one rollout stream".into(),
        ));
    }
    let project_dir = if codex {
        projects_root.to_path_buf()
    } else {
        projects_root.join(cwd_slug(cwd)?)
    };
    if !codex {
        refuse_other_slug(projects_root, &project_dir, session)?;
    } else if projects_root.exists() {
        for existing in crate::discover_codex_streams(projects_root)
            .map_err(|error| RestoreError::Verification(error.to_string()))?
        {
            if existing.session == session && !selected.contains_key(&existing.stream) {
                return Err(RestoreError::OtherSlug {
                    session: session.into(),
                    path: existing.path,
                });
            }
        }
    }
    if codex {
        private_codex_parents(projects_root, projects_root)?;
    } else {
        create_private_dir(projects_root)?;
        create_private_dir(&project_dir)?;
    }

    let mut staged = Vec::with_capacity(selected.len());
    let mut metadata = TranscriptMetadata {
        codex,
        codex_home: codex
            .then(|| absolute_path(projects_root.parent().unwrap_or(projects_root)))
            .transpose()?,
        ..TranscriptMetadata::default()
    };
    let mut had_gaps = false;
    for (stream, receiver_tip) in selected {
        validate_stream(&stream)?;
        let generation = receiver_tip.generation;
        let target = stream_path(&project_dir, session, &stream)?;
        let parent = target.parent().expect("restore path has a parent");
        if codex {
            private_codex_parents(&project_dir, parent)?;
        } else if parent != project_dir {
            create_private_dir(&project_dir.join(session))?;
        }
        create_private_dir(parent)?;
        let (temporary, mut output) = create_staging_file(&target)?;
        let mut temporary_guard = TemporaryGuard {
            path: temporary.clone(),
            armed: true,
        };

        let mut expected = 0_u64;
        let mut previous_epoch = None;
        let mut previous_chain = ZERO_CHAIN;
        let mut plaintext_tip = None;
        let mut encrypted_tip = None;
        let mut truncated_at_seq = None;
        let mut cursor = None;
        let mut lines = 0_u64;
        let mut stop = false;
        while !stop {
            let page = transport
                .list_frame_page(
                    session,
                    machine,
                    &stream,
                    generation,
                    cursor,
                    FRAME_PAGE_MAX_BYTES,
                    FRAME_PAGE_MAX_FRAMES,
                )
                .map_err(|source| RestoreError::Transport {
                    what: "live frame page",
                    session: session.to_owned(),
                    source,
                })?;
            let page_last = page.frames.last().map(|frame| frame.key.seq);
            for frame in page.frames {
                if frame.key.session != session {
                    return Err(RestoreError::WrongSession {
                        expected: session.to_owned(),
                        actual: frame.key.session,
                    });
                }
                if frame.key.stream != stream || frame.key.generation != generation {
                    return Err(RestoreError::Page(format!(
                        "requested {stream} generation {generation}, got {} generation {}",
                        frame.key.stream, frame.key.generation
                    )));
                }
                if frame.key.seq != expected {
                    if allow_gaps && frame.key.seq > expected {
                        truncated_at_seq = Some(expected);
                        had_gaps = true;
                        stop = true;
                        break;
                    }
                    return Err(RestoreError::Gap {
                        stream: stream.clone(),
                        generation,
                        expected,
                        actual: frame.key.seq,
                    });
                }
                if previous_epoch.is_some_and(|epoch| frame.key.epoch < epoch) {
                    return Err(RestoreError::Epoch {
                        stream: stream.clone(),
                        generation,
                        seq: frame.key.seq,
                    });
                }
                let seq = frame.key.seq;
                let epoch = frame.key.epoch;
                let line = if let Some(key) = data_key {
                    if !matches!(&frame.content, FrameContent::Encrypted(_)) {
                        return Err(RestoreError::Plaintext(session.to_owned()));
                    }
                    verify_encrypted_frame(session, key, &mut encrypted_tip, frame)
                        .map_err(restore_verify_error)?
                } else {
                    let FrameContent::Plaintext { chain, line } = frame.content else {
                        return Err(RestoreError::Encrypted(session.to_owned()));
                    };
                    if chain != chain_line(&previous_chain, &line) {
                        return Err(RestoreError::Chain {
                            stream: stream.clone(),
                            generation,
                            seq,
                        });
                    }
                    previous_chain = chain;
                    plaintext_tip = Some(StreamTip {
                        stream: stream.clone(),
                        generation,
                        epoch,
                        seq,
                        mode: FrameMode::Plaintext,
                        chain: Some(chain),
                        tag: None,
                    });
                    line
                };
                output.write_all(&line).map_err(|source| RestoreError::Io {
                    path: temporary.clone(),
                    source,
                })?;
                if codex && seq == 0 {
                    let value: Value = serde_json::from_slice(&line).map_err(|_| {
                        RestoreError::Verification("invalid Codex session_meta".into())
                    })?;
                    if value["type"] != "session_meta"
                        || value["payload"]["id"].as_str() != Some(session)
                        || !stream.ends_with(&format!("-{session}.jsonl"))
                    {
                        return Err(RestoreError::WrongSession {
                            expected: session.into(),
                            actual: value["payload"]["id"].as_str().unwrap_or("missing").into(),
                        });
                    }
                }
                metadata.ingest(&stream, &line);
                previous_epoch = Some(epoch);
                lines = lines.saturating_add(1);
                expected = expected.saturating_add(1);
                if seq == receiver_tip.seq {
                    stop = true;
                    break;
                }
            }
            if stop {
                break;
            }
            match page.next {
                Some(next)
                    if page_last == Some(next) && cursor.is_none_or(|cursor| next > cursor) =>
                {
                    cursor = Some(next);
                }
                Some(_) => {
                    return Err(RestoreError::Page(
                        "receiver returned a non-advancing frame cursor".into(),
                    ));
                }
                None => break,
            }
        }

        output.sync_all().map_err(|source| RestoreError::Io {
            path: temporary.clone(),
            source,
        })?;
        drop(output);
        let tip = encrypted_tip.or(plaintext_tip);
        if truncated_at_seq.is_none()
            && !tip
                .as_ref()
                .is_some_and(|tip| restore_tip_matches(tip, receiver_tip))
        {
            return Err(RestoreError::Tip {
                stream: stream.clone(),
                generation,
                seq: receiver_tip.seq,
            });
        }
        validate_existing_prefix(&target, &temporary, &stream)?;
        temporary_guard.armed = false;
        staged.push(StagedStream {
            temporary,
            target,
            verified: VerifiedStream {
                stream,
                generation,
                tip,
                truncated_at_seq,
            },
            total_lines: lines,
        });
    }

    metadata.finish();
    let mut streams = Vec::with_capacity(staged.len());
    let mut reports = Vec::with_capacity(staged.len());
    for mut stage in staged {
        let lines_written = commit_staged(&mut stage)?;
        reports.push(RestoredStream {
            stream: stage.verified.stream.clone(),
            generation: stage.verified.generation,
            lines: stage.total_lines,
            lines_written,
            truncated_at_seq: stage.verified.truncated_at_seq,
        });
        streams.push(stage.verified.clone());
    }
    Ok((
        VerifiedSession {
            streams,
            had_gaps,
            metadata,
        },
        reports,
    ))
}

fn select_highest_generations(tips: &[StreamTip]) -> BTreeMap<String, &StreamTip> {
    let mut selected = BTreeMap::new();
    for tip in tips {
        if selected
            .get(&tip.stream)
            .is_none_or(|known: &&StreamTip| tip.generation > known.generation)
        {
            selected.insert(tip.stream.clone(), tip);
        }
    }
    selected
}

fn restore_tip_matches(actual: &StreamTip, expected: &StreamTip) -> bool {
    actual.stream == expected.stream
        && actual.generation == expected.generation
        && actual.epoch == expected.epoch
        && actual.seq == expected.seq
        && actual.mode == expected.mode
        && match expected.mode {
            FrameMode::Plaintext => actual.chain == expected.chain,
            FrameMode::Encrypted => actual.tag == expected.tag,
        }
}

fn restore_verify_error(error: VerifyError) -> RestoreError {
    match error {
        VerifyError::Gap {
            stream,
            generation,
            expected,
            actual,
        } => RestoreError::Gap {
            stream,
            generation,
            expected,
            actual,
        },
        VerifyError::Epoch {
            stream,
            generation,
            seq,
        } => RestoreError::Epoch {
            stream,
            generation,
            seq,
        },
        VerifyError::Decrypt {
            stream,
            generation,
            seq,
            source,
        } => RestoreError::Decrypt {
            stream,
            generation,
            seq,
            source,
        },
        VerifyError::Chain {
            stream,
            generation,
            seq,
        } => RestoreError::Chain {
            stream,
            generation,
            seq,
        },
        error => RestoreError::Verification(error.to_string()),
    }
}

fn merge_write_reports(existing: &mut Vec<RestoredStream>, latest: Vec<RestoredStream>) {
    for report in latest {
        if let Some(known) = existing
            .iter_mut()
            .find(|known| known.stream == report.stream)
        {
            known.generation = report.generation;
            known.lines = report.lines;
            known.lines_written = known.lines_written.saturating_add(report.lines_written);
            known.truncated_at_seq = report.truncated_at_seq;
        } else {
            existing.push(report);
        }
    }
    existing.sort_by(|left, right| left.stream.cmp(&right.stream));
}

fn initialize_after_race(
    projects_root: &Path,
    state_path: &Path,
    session: &str,
    machine: &str,
    epoch: u64,
    verified: &VerifiedSession,
    transport: &impl Transport,
) -> Result<(), RestoreError> {
    let receiver =
        transport
            .lease_tips(session, machine)
            .map_err(|source| RestoreError::Transport {
                what: "post-takeover tips",
                session: session.to_owned(),
                source,
            })?;
    if receiver.row.epoch != epoch || receiver.row.holder_machine != machine {
        return Err(RestoreError::PostTakeoverLease);
    }
    let mut receiver_tips = BTreeMap::<String, &StreamTip>::new();
    for tip in &receiver.tips {
        if receiver_tips
            .get(&tip.stream)
            .is_none_or(|known| tip.generation > known.generation)
        {
            receiver_tips.insert(tip.stream.clone(), tip);
        }
    }
    let mut tips = Vec::with_capacity(receiver_tips.len());
    for (stream, receiver_tip) in receiver_tips {
        let restored = verified
            .streams
            .iter()
            .find(|candidate| {
                candidate.stream == stream && candidate.generation == receiver_tip.generation
            })
            .and_then(|candidate| candidate.tip.as_ref())
            .ok_or(RestoreError::PostTakeoverTips)?;
        if restored.epoch != receiver_tip.epoch
            || restored.seq != receiver_tip.seq
            || restored.mode != receiver_tip.mode
            || (receiver_tip.mode == FrameMode::Plaintext && restored.chain != receiver_tip.chain)
            || (receiver_tip.mode == FrameMode::Encrypted && restored.tag != receiver_tip.tag)
        {
            return Err(RestoreError::PostTakeoverTips);
        }
        tips.push(restored.clone());
    }
    initialize_takeover_state(
        projects_root,
        state_path,
        session,
        &TakeoverResult {
            row: receiver.row,
            tips,
        },
    )
    .map_err(|error| RestoreError::State(Box::new(error)))
}

fn build_report(
    session: &str,
    cwd: PathBuf,
    verified: &VerifiedSession,
    streams: &[RestoredStream],
    new_epoch: Option<u64>,
    orphans: Vec<OrphanSummary>,
    incomplete: bool,
) -> RestoreReport {
    let metadata = &verified.metadata;
    let git_branch_check = metadata
        .git_branch
        .as_ref()
        .map(|branch| check_git_branch(&cwd, branch));
    RestoreReport {
        session: session.to_owned(),
        harness: if metadata.codex { "codex" } else { "claude" }.into(),
        codex_home: metadata.codex_home.clone(),
        cwd,
        streams: streams.to_vec(),
        lines: streams.iter().map(|stream| stream.lines).sum(),
        lines_written: streams.iter().map(|stream| stream.lines_written).sum(),
        new_epoch,
        recorded_cwd: metadata.cwd.clone(),
        recorded_git_branch: metadata.git_branch.clone(),
        git_branch_check,
        unfinished_tool_calls: metadata.unfinished.clone(),
        orphans,
        incomplete,
    }
}

#[derive(Default)]
struct TranscriptMetadata {
    codex: bool,
    codex_home: Option<PathBuf>,
    cwd: Option<String>,
    git_branch: Option<String>,
    uses: Vec<UnfinishedToolCall>,
    results: BTreeSet<String>,
    unfinished: Vec<UnfinishedToolCall>,
}

impl TranscriptMetadata {
    fn ingest(&mut self, stream: &str, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if stream == "main"
            && let Some(object) = value.as_object()
        {
            if let Some(cwd) = object.get("cwd").and_then(Value::as_str) {
                self.cwd = Some(cwd.to_owned());
            }
            if let Some(branch) = object.get("gitBranch").and_then(Value::as_str) {
                self.git_branch = Some(branch.to_owned());
            }
        }
        if stream.starts_with("codex/") {
            self.codex = true;
            let payload = &value["payload"];
            if matches!(
                value["type"].as_str(),
                Some("session_meta" | "turn_context")
            ) {
                if let Some(cwd) = payload["cwd"].as_str() {
                    self.cwd = Some(cwd.into());
                }
                if let Some(branch) = payload["git"]["branch"].as_str() {
                    self.git_branch = Some(branch.into());
                }
            }
            match payload["type"].as_str() {
                Some("function_call" | "custom_tool_call") => {
                    if let (Some(id), Some(name)) =
                        (payload["call_id"].as_str(), payload["name"].as_str())
                    {
                        self.uses.push(UnfinishedToolCall {
                            id: id.into(),
                            name: name.into(),
                        });
                    }
                }
                Some("function_call_output" | "custom_tool_call_output") => {
                    if let Some(id) = payload["call_id"].as_str() {
                        self.results.insert(id.into());
                    }
                }
                _ => {}
            }
        }
        find_tool_blocks(&value, self);
    }

    fn finish(&mut self) {
        self.unfinished = self
            .uses
            .iter()
            .filter(|call| !self.results.contains(&call.id))
            .cloned()
            .collect();
    }
}

fn find_tool_blocks(value: &Value, metadata: &mut TranscriptMetadata) {
    match value {
        Value::Object(object) => {
            match object.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (
                        object.get("id").and_then(Value::as_str),
                        object.get("name").and_then(Value::as_str),
                    ) {
                        metadata.uses.push(UnfinishedToolCall {
                            name: name.to_owned(),
                            id: id.to_owned(),
                        });
                    }
                    return;
                }
                Some("tool_result") => {
                    if let Some(id) = object.get("tool_use_id").and_then(Value::as_str) {
                        metadata.results.insert(id.to_owned());
                    }
                    return;
                }
                _ => {}
            }
            for child in object.values() {
                find_tool_blocks(child, metadata);
            }
        }
        Value::Array(array) => {
            for child in array {
                find_tool_blocks(child, metadata);
            }
        }
        _ => {}
    }
}

fn check_git_branch(cwd: &Path, expected: &str) -> GitBranchCheck {
    let actual = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .env("HOME", cwd)
        .env("XDG_CONFIG_HOME", cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|branch| branch.trim().to_owned())
        .filter(|branch| !branch.is_empty());
    GitBranchCheck {
        matches: actual.as_deref() == Some(expected),
        expected: expected.to_owned(),
        actual,
    }
}

fn validate_session(session: &str) -> Result<(), RestoreError> {
    let path = Path::new(session);
    if session.is_empty()
        || path.file_name().and_then(|value| value.to_str()) != Some(session)
        || matches!(session, "." | "..")
    {
        Err(RestoreError::Session(session.to_owned()))
    } else {
        Ok(())
    }
}

fn validate_stream(stream: &str) -> Result<(), RestoreError> {
    if stream
        .strip_prefix("codex/")
        .is_some_and(crate::discovery::valid_codex_path)
    {
        return Ok(());
    }
    if stream == "main" {
        return Ok(());
    }
    let Some(name) = stream.strip_prefix("subagents/") else {
        return Err(RestoreError::Stream(stream.to_owned()));
    };
    if !name.starts_with("agent-")
        || !name.ends_with(".jsonl")
        || name.contains('/')
        || matches!(name, "." | "..")
    {
        Err(RestoreError::Stream(stream.to_owned()))
    } else {
        Ok(())
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, RestoreError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|source| RestoreError::Io {
                path: path.to_path_buf(),
                source,
            })
    }
}

fn cwd_slug(cwd: &Path) -> Result<String, RestoreError> {
    cwd.to_str()
        .map(|cwd| cwd.replace('/', "-"))
        .ok_or_else(|| RestoreError::Cwd(cwd.to_path_buf()))
}

fn stream_path(project_dir: &Path, session: &str, stream: &str) -> Result<PathBuf, RestoreError> {
    if let Some(path) = stream.strip_prefix("codex/") {
        validate_stream(stream)?;
        return Ok(project_dir.join(path));
    }
    match stream {
        "main" => Ok(project_dir.join(format!("{session}.jsonl"))),
        _ => {
            validate_stream(stream)?;
            let name = stream
                .strip_prefix("subagents/")
                .expect("validated subagent stream");
            Ok(project_dir.join(session).join("subagents").join(name))
        }
    }
}

fn refuse_other_slug(
    projects_root: &Path,
    target_project: &Path,
    session: &str,
) -> Result<(), RestoreError> {
    let entries = match fs::read_dir(projects_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(RestoreError::Io {
                path: projects_root.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| RestoreError::Io {
            path: projects_root.to_path_buf(),
            source,
        })?;
        if entry.path() == target_project {
            continue;
        }
        let candidate = entry.path().join(format!("{session}.jsonl"));
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                return Err(RestoreError::OtherSlug {
                    session: session.to_owned(),
                    path: candidate,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(RestoreError::Io {
                    path: candidate,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), RestoreError> {
    fs::create_dir_all(path).map_err(|source| RestoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        RestoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(())
}

fn create_staging_file(target: &Path) -> Result<(PathBuf, File), RestoreError> {
    let parent = target.parent().expect("restore path has a parent");
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{name}.restore.{}.{}", process::id(), sequence));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options
        .open(&temporary)
        .map_err(|source| RestoreError::Io {
            path: temporary.clone(),
            source,
        })?;
    Ok((temporary, file))
}

fn validate_existing_prefix(
    target: &Path,
    staged: &Path,
    stream: &str,
) -> Result<Option<u64>, RestoreError> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(RestoreError::TargetType(target.to_path_buf()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RestoreError::Io {
                path: target.to_path_buf(),
                source,
            });
        }
    }

    let mut existing = File::open(target).map_err(|source| RestoreError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let mut restored = File::open(staged).map_err(|source| RestoreError::Io {
        path: staged.to_path_buf(),
        source,
    })?;
    let mut offset = 0_u64;
    let mut existing_buf = [0_u8; 64 * 1024];
    let mut restored_buf = [0_u8; 64 * 1024];
    loop {
        let count = existing
            .read(&mut existing_buf)
            .map_err(|source| RestoreError::Io {
                path: target.to_path_buf(),
                source,
            })?;
        if count == 0 {
            return Ok(Some(offset));
        }
        match restored.read_exact(&mut restored_buf[..count]) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(RestoreError::Prefix {
                    stream: stream.to_owned(),
                    path: target.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(RestoreError::Io {
                    path: staged.to_path_buf(),
                    source,
                });
            }
        }
        if existing_buf[..count] != restored_buf[..count] {
            return Err(RestoreError::Prefix {
                stream: stream.to_owned(),
                path: target.to_path_buf(),
            });
        }
        offset = offset.saturating_add(count as u64);
    }
}

fn commit_staged(stage: &mut StagedStream) -> Result<u64, RestoreError> {
    let parent = stage.target.parent().expect("restore target has a parent");
    match validate_existing_prefix(&stage.target, &stage.temporary, &stage.verified.stream)? {
        None => {
            fs::hard_link(&stage.temporary, &stage.target).map_err(|source| RestoreError::Io {
                path: stage.target.clone(),
                source,
            })?;
            fs::remove_file(&stage.temporary).map_err(|source| RestoreError::Io {
                path: stage.temporary.clone(),
                source,
            })?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| RestoreError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            Ok(stage.total_lines)
        }
        Some(existing_len) => {
            let staged_len = fs::metadata(&stage.temporary)
                .map_err(|source| RestoreError::Io {
                    path: stage.temporary.clone(),
                    source,
                })?
                .len();
            if existing_len == staged_len {
                return Ok(0);
            }
            let mut restored = File::open(&stage.temporary).map_err(|source| RestoreError::Io {
                path: stage.temporary.clone(),
                source,
            })?;
            restored
                .seek(SeekFrom::Start(existing_len))
                .map_err(|source| RestoreError::Io {
                    path: stage.temporary.clone(),
                    source,
                })?;
            let mut options = OpenOptions::new();
            options.append(true);
            let mut target = options
                .open(&stage.target)
                .map_err(|source| RestoreError::Io {
                    path: stage.target.clone(),
                    source,
                })?;
            let mut lines_written = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = restored
                    .read(&mut buffer)
                    .map_err(|source| RestoreError::Io {
                        path: stage.temporary.clone(),
                        source,
                    })?;
                if count == 0 {
                    break;
                }
                target
                    .write_all(&buffer[..count])
                    .map_err(|source| RestoreError::Io {
                        path: stage.target.clone(),
                        source,
                    })?;
                lines_written = lines_written.saturating_add(
                    buffer[..count]
                        .iter()
                        .filter(|byte| **byte == b'\n')
                        .count() as u64,
                );
            }
            target.sync_all().map_err(|source| RestoreError::Io {
                path: stage.target.clone(),
                source,
            })?;
            #[cfg(unix)]
            fs::set_permissions(&stage.target, fs::Permissions::from_mode(0o600)).map_err(
                |source| RestoreError::Io {
                    path: stage.target.clone(),
                    source,
                },
            )?;
            Ok(lines_written)
        }
    }
}

fn private_codex_parents(root: &Path, parent: &Path) -> Result<(), RestoreError> {
    let mut path = root.to_path_buf();
    for part in std::iter::once(None).chain(
        parent
            .strip_prefix(root)
            .expect("validated relative rollout path")
            .components()
            .map(Some),
    ) {
        if let Some(part) = part {
            path.push(part);
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                return Err(RestoreError::TargetType(path));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => create_private_dir(&path)?,
            Err(source) => return Err(RestoreError::Io { path, source }),
        }
    }
    Ok(())
}

// Closing one descriptor is insufficient if a concurrent child inherited a duplicate before exec.
// Unlock the shared open-file description explicitly when restore leaves any success/error path.
struct CodexWriterGuard {
    file: File,
}
impl Drop for CodexWriterGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn lock_codex_writer(root: &Path, session: &str) -> Result<CodexWriterGuard, RestoreError> {
    if root.file_name().and_then(|name| name.to_str()) != Some("sessions") {
        return Err(RestoreError::Verification(
            "Codex restore target must be CODEX_HOME/sessions".into(),
        ));
    }
    let home = root
        .parent()
        .ok_or_else(|| RestoreError::TargetType(root.into()))?;
    let locks = home.join("thread-writer-locks");
    private_codex_parents(home, &locks)?;
    let path = locks.join(format!("{session}.lock"));
    if fs::symlink_metadata(&path)
        .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
    {
        return Err(RestoreError::TargetType(path));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(&path).map_err(|source| RestoreError::Io {
        path: path.clone(),
        source,
    })?;
    file.try_lock().map_err(|_| {
        RestoreError::Verification("Codex thread has an active writer; restore refused".into())
    })?;
    Ok(CodexWriterGuard { file })
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

#[cfg(all(test, unix))]
mod codex_writer_tests {
    use super::*;
    #[test]
    fn restore_guard_unlocks_even_with_an_inherited_file_description() {
        let base = std::env::temp_dir().join(format!(
            "semon-codex-writer-{}-{}",
            process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("sessions");
        let guard = lock_codex_writer(&root, "synthetic-thread").unwrap();
        // dup/fork share the OS open-file description. Keep that duplicate alive across guard drop.
        let inherited = guard.file.try_clone().unwrap();
        assert!(lock_codex_writer(&root, "synthetic-thread").is_err());
        drop(guard);
        let next = lock_codex_writer(&root, "synthetic-thread").unwrap();
        drop(next);
        drop(inherited);
        fs::remove_dir_all(base).unwrap();
    }
}

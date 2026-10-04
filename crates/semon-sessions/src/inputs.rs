//! The input set: every file the model builder reads, and nothing else.
//! Copying exactly these, plus the machine's [`crate::Facts`], is enough to
//! rebuild the same model elsewhere. `*.key` files are never in it.
//!
//! - `claude/projects/**/*.jsonl`: transcripts, and subagents' under
//!   `<session>/subagents/agent-<id>.jsonl`;
//! - `claude/projects/**/subagents/agent-<id>.meta.json`: a subagent's
//!   metadata;
//! - `claude/sessions/<pid>.json`: a running Claude process's record;
//! - `codex/sessions/**/*.jsonl` and `codex/archived_sessions/**/*.jsonl`: Codex rollouts.

use std::{fs, io, path::PathBuf};

use crate::Options;

/// Which agent home a relative path is under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputRoot {
    Claude,
    Codex,
    Copilot,
}

impl InputRoot {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
        }
    }

    pub fn parse(root: &str) -> Option<Self> {
        match root {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    /// This root's directory in `options`.
    pub fn home(self, options: &Options) -> &PathBuf {
        match self {
            Self::Claude => &options.claude_home,
            Self::Codex => &options.codex_home,
            Self::Copilot => &options.copilot_home,
        }
    }
}

/// One input file: its root and its path relative to that root, with `/`
/// separators.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Input {
    pub root: InputRoot,
    pub path: String,
}

impl Input {
    pub fn full_path(&self, options: &Options) -> PathBuf {
        let mut full = self.root.home(options).clone();
        full.extend(self.path.split('/'));
        full
    }
}

/// The longest relative path, and the deepest, an input may have.
const MAX_PATH: usize = 1024;
const MAX_DEPTH: usize = 12;

/// Whether `rel` is a path under `root` ("claude" or "codex") that the
/// builder reads. Pure: it touches no file. Rejects an absolute path, an
/// empty, `.` or `..` component, a backslash or control character, and
/// anything outside the input patterns.
pub fn is_input_path(root: &str, rel: &str) -> bool {
    let Some(root) = InputRoot::parse(root) else {
        return false;
    };
    if rel.is_empty()
        || rel.len() > MAX_PATH
        || rel.starts_with('/')
        || rel
            .bytes()
            .any(|byte| byte == b'\\' || byte.is_ascii_control())
    {
        return false;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() > MAX_DEPTH
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
        // A sealed log's segments live in `<log>.seal/`: nothing under one
        // is an input.
        || parts[..parts.len() - 1]
            .iter()
            .any(|part| crate::sealed::is_seal_dir_name(part))
    {
        return false;
    }
    let name = parts[parts.len() - 1];
    match (root, parts.as_slice()) {
        (InputRoot::Claude, ["sessions", _]) => name
            .strip_suffix(".json")
            .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit())),
        (InputRoot::Claude, ["projects", .., parent, _]) => {
            jsonl(name)
                || (*parent == "subagents"
                    && name
                        .strip_prefix("agent-")
                        .and_then(|rest| rest.strip_suffix(".meta.json"))
                        .is_some_and(|id| !id.is_empty()))
        }
        (InputRoot::Claude, ["projects", _]) => jsonl(name),
        (InputRoot::Codex, ["sessions" | "archived_sessions", .., _]) => jsonl(name),
        (InputRoot::Copilot, ["session-state", _, "events.jsonl"]) => true,
        _ => false,
    }
}

fn jsonl(name: &str) -> bool {
    name.strip_suffix(".jsonl")
        .is_some_and(|stem| !stem.is_empty())
}

/// Pinned Codex stores active and archived plain JSONL beneath these roots.
/// Compressed rollouts and database files are not supported viewing inputs.
pub(crate) fn codex_rollout_dirs(options: &Options) -> [PathBuf; 2] {
    [
        options.codex_home.join("sessions"),
        options.codex_home.join("archived_sessions"),
    ]
}

/// Every input file under the homes `options` names, sorted. Directories are
/// walked without following symbolic links, and only regular files whose
/// path passes [`is_input_path`] are listed.
pub fn inputs(options: &Options) -> io::Result<Vec<Input>> {
    let mut found = Vec::new();
    for (root, top) in [
        (InputRoot::Claude, "projects"),
        (InputRoot::Claude, "sessions"),
        (InputRoot::Codex, "sessions"),
        (InputRoot::Codex, "archived_sessions"),
        (InputRoot::Copilot, "session-state"),
    ] {
        walk(
            root,
            &root.home(options).join(top),
            top.to_owned(),
            &mut found,
        )?;
    }
    found.sort();
    Ok(found)
}

fn walk(
    root: InputRoot,
    dir: &std::path::Path,
    rel: String,
    found: &mut Vec<Input>,
) -> io::Result<()> {
    if fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = format!("{rel}/{name}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if path.split('/').count() < MAX_DEPTH {
                walk(root, &entry.path(), path, found)?;
            }
        } else if kind.is_file() && is_input_path(root.as_str(), &path) {
            found.push(Input { root, path });
        }
    }
    Ok(())
}

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

#[cfg(test)]
thread_local! { pub(crate) static DISCOVERY_ENTRIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
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

/// A traversal cursor owned by the native producer. Each step bounds both
/// accepted sources and directory entries, including unrelated files. ReadDir
/// handles are retained only along the current path (at most MAX_DEPTH), so a
/// large flat directory does not become an in-memory listing.
pub(crate) struct Discovery {
    roots: std::vec::IntoIter<(InputRoot, String)>,
    stack: Vec<DiscoveryDir>,
    complete: bool,
}
struct DiscoveryDir {
    root: InputRoot,
    relative: String,
    entries: fs::ReadDir,
}
pub(crate) struct DiscoveryPage {
    pub(crate) inputs: Vec<Input>,
    pub(crate) examined: usize,
    pub(crate) complete: bool,
}
impl Default for Discovery {
    fn default() -> Self {
        Self {
            roots: vec![
                (InputRoot::Claude, "projects".into()),
                (InputRoot::Claude, "sessions".into()),
                (InputRoot::Codex, "sessions".into()),
                (InputRoot::Codex, "archived_sessions".into()),
                (InputRoot::Copilot, "session-state".into()),
            ]
            .into_iter(),
            stack: Vec::new(),
            complete: false,
        }
    }
}
impl Discovery {
    fn open(&mut self, options: &Options, root: InputRoot, relative: String) -> io::Result<()> {
        let path = root.home(options).join(&relative);
        // Match native discovery's custody boundary: never follow symlinks.
        if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Ok(());
        }
        match fs::read_dir(path) {
            Ok(entries) => self.stack.push(DiscoveryDir {
                root,
                relative,
                entries,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }
    pub(crate) fn next(
        &mut self,
        options: &Options,
        sources: usize,
        entries: usize,
    ) -> io::Result<DiscoveryPage> {
        if sources == 0 || entries == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Native discovery requires a positive budget",
            ));
        }
        let mut page = DiscoveryPage {
            inputs: Vec::new(),
            examined: 0,
            complete: self.complete,
        };
        while !self.complete && page.inputs.len() < sources && page.examined < entries {
            if self.stack.is_empty() {
                let Some((root, relative)) = self.roots.next() else {
                    self.complete = true;
                    break;
                };
                page.examined += 1;
                self.open(options, root, relative)?;
                continue;
            }
            let directory = self.stack.last_mut().expect("directory exists");
            let Some(entry) = directory.entries.next() else {
                self.stack.pop();
                continue;
            };
            page.examined += 1;
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let root = directory.root;
            let relative = format!("{}/{name}", directory.relative);
            let kind = entry.file_type()?;
            if kind.is_dir() && relative.split('/').count() < MAX_DEPTH {
                self.open(options, root, relative)?;
            } else if kind.is_file()
                && relative.ends_with(".jsonl")
                && is_input_path(root.as_str(), &relative)
            {
                page.inputs.push(Input {
                    root,
                    path: relative,
                });
            }
        }
        page.inputs.sort();
        page.complete = self.complete;
        #[cfg(test)]
        DISCOVERY_ENTRIES.with(|count| count.set(count.get() + page.examined as u64));
        Ok(page)
    }
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

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        options: Options,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-discovery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let options = Options {
                claude_home: root.join("claude"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                ..Options::default()
            };
            Self { root, options }
        }
        fn write(&self, path: &str) {
            let path = self.root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "native bytes are not read during discovery").unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[test]
    fn discovery_bounds_unrelated_directory_work_and_resumes_without_losing_sources() {
        let fixture = Fixture::new();
        for index in 0..1000 {
            fixture.write(&format!("claude/projects/project/unrelated-{index}.txt"));
        }
        for index in 0..43 {
            fixture.write(&format!("claude/projects/project/session-{index}.jsonl"));
        }
        fixture.write("claude/projects/project/session-1/subagents/agent-child.jsonl");
        fixture.write("claude/projects/project/session-1/subagents/agent-child.meta.json");
        fixture.write("codex/archived_sessions/2026/rollout.jsonl");
        fixture.write("copilot/session-state/session/events.jsonl");
        let expected: Vec<_> = inputs(&fixture.options)
            .unwrap()
            .into_iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .collect();
        let mut discovery = Discovery::default();
        let mut found = Vec::new();
        let mut pages = 0;
        loop {
            let page = discovery.next(&fixture.options, 3, 17).unwrap();
            assert!(page.examined <= 17);
            assert!(page.inputs.len() <= 3);
            pages += 1;
            found.extend(page.inputs);
            if page.complete {
                break;
            }
            assert!(
                pages < 200,
                "traversal must make progress even through unrelated entries"
            );
        }
        assert!(pages > 50, "unrelated entries must consume the work budget");
        found.sort();
        assert_eq!(found, expected);
        let done = discovery.next(&fixture.options, 3, 17).unwrap();
        assert!(done.complete && done.inputs.is_empty() && done.examined == 0);
    }
    #[cfg(unix)]
    #[test]
    fn discovery_never_follows_directory_or_file_symlinks() {
        let fixture = Fixture::new();
        fixture.write("outside/hidden.jsonl");
        fixture.write("claude/projects/real/session.jsonl");
        std::os::unix::fs::symlink(
            fixture.root.join("outside"),
            fixture.options.claude_home.join("projects/linked"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            fixture.root.join("outside/hidden.jsonl"),
            fixture.options.claude_home.join("projects/link.jsonl"),
        )
        .unwrap();
        let page = Discovery::default()
            .next(&fixture.options, 64, 512)
            .unwrap();
        assert!(page.complete);
        assert_eq!(
            page.inputs,
            vec![Input {
                root: InputRoot::Claude,
                path: "projects/real/session.jsonl".into()
            }]
        );
    }
}

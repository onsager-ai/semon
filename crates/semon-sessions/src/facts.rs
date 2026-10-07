//! What the model takes from the machine rather than from the log files:
//! the hostname, the home directory, which processes are alive, which Codex
//! runs hold their writer lock, and the repository of each working
//! directory. [`local_facts`] reads them here; [`Options::facts`] makes the
//! builder take them from a file instead, so a copy of the log files built
//! elsewhere gives the same model as the original.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, Read},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::{Options, events::EventCache, lock_pid, model, proc_start};

pub(crate) mod source_authority;

pub const FACTS_VERSION: u32 = 2;

/// The environment variables Semon reads from a session's process, by exact
/// name: Ostrom's run contract (its `docs/loops.md`, "The run environment is
/// a contract"). Every other entry of the environment is dropped as it is
/// parsed, never kept, logged or compared beyond its name.
pub const RUN_VARIABLES: [&str; 3] = ["OSTROM_RUN_ID", "OSTROM_WORK_ORDER_ID", "SEMON_PARENT"];

/// The most of `/proc/<pid>/environ` that is read. A larger environment
/// isn't read at all: a cut one could hold a cut value.
const ENVIRON_MAX: u64 = 256 * 1024;

/// The machine's side of a model, as JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    pub version: u32,
    /// What the model names the machine by.
    pub hostname: String,
    /// `$HOME`, which paths in tool summaries are shortened against.
    pub home: Option<String>,
    /// The start time (`/proc/<pid>/stat` field 22) of each process a Claude
    /// `sessions/<pid>.json` names, when that process exists. A pid file is
    /// live when its `procStart` equals this.
    pub proc_starts: BTreeMap<u32, u64>,
    /// Current native Codex plain-rollout paths, relative to its home.
    /// None is an older snapshot with unknown source selection; Some(empty)
    /// explicitly selects no sources. Retired mirrored bytes stay retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_rollouts: Option<BTreeSet<String>>,
    /// Each Codex thread whose writer lock is held, with the holder's pid.
    pub codex_locks: BTreeMap<String, u32>,
    /// The repository (its directory's name) of every working directory the
    /// logs name, or null when it's in none.
    pub repos: BTreeMap<String, Option<String>>,
    /// Set by a receiver that has stopped hearing from the machine: when it
    /// was last seen (epoch ms). The machine then counts as offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<i64>,
    /// Each live session process's run ids ([`RUN_VARIABLES`] only), by
    /// pid: empty when its environment holds none. A pid is missing when
    /// its environment couldn't be read whole.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runs: BTreeMap<u32, BTreeMap<String, String>>,
    /// Parent pid chains observed for live known session processes, nearest first.
    /// Optional in v2 facts, so existing receivers remain compatible.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub process_ancestors: BTreeMap<u32, Vec<u32>>,
    /// Extracted run snapshots from the sibling Claude state file. The
    /// reader retains only the session/run fields and model usage allowlist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reported_runs: Vec<ReportedRunSnapshot>,
}

/// The allowlisted values of one `projects.*` last-run record. The project
/// path is intentionally not retained. `captureAt` is generated locally so
/// a later comparison can stop at the exact snapshot boundary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedRunSnapshot {
    pub last_session_id: String,
    pub last_start_time: i64,
    pub last_cost: Option<f64>,
    pub last_duration: Option<u64>,
    pub last_api_duration: Option<u64>,
    pub last_tool_duration: Option<u64>,
    pub last_lines_added: Option<u64>,
    pub last_lines_removed: Option<u64>,
    #[serde(default)]
    pub last_model_usage: BTreeMap<String, ReportedModelUsage>,
    pub capture_at: i64,
}

/// The explicitly allowed fields within one `lastModelUsage` value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReportedModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_search_requests: u64,
    pub cost_usd: Option<f64>,
}

impl Facts {
    /// The same machine, offline since `last_seen` (epoch ms): its hostname,
    /// home and repositories, and no live process or held lock.
    pub fn offline(&self, last_seen: i64) -> Self {
        Self {
            proc_starts: BTreeMap::new(),
            codex_locks: BTreeMap::new(),
            runs: BTreeMap::new(),
            offline_since: Some(last_seen),
            ..self.clone()
        }
    }
}

/// Reads this machine's facts for the homes `options` names. The working
/// directories come from the same metadata scan the model builder runs (its
/// event cache is used and saved, as `model_json` does); repositories are
/// found by walking up from each one to a `.git` (a linked worktree resolves
/// to the repository it was made from).
pub fn local_facts(options: &Options) -> io::Result<Facts> {
    let mut source = FactsSource::new(options);
    let facts = source.facts()?;
    source.flush()?;
    Ok(facts)
}

/// Computes local facts while keeping the event index open between calls.
/// The index commits each file's change as it reads it.
pub struct FactsSource {
    options: crate::Options,
    cache: EventCache,
    dirty: bool,
}

impl FactsSource {
    /// Opens the event index once for repeated facts collection.
    pub fn new(options: &crate::Options) -> Self {
        Self {
            options: options.clone(),
            cache: EventCache::open(&options.cache),
            dirty: false,
        }
    }

    /// Reads this machine's current facts, using the in-memory event cache.
    pub fn facts(&mut self) -> io::Result<Facts> {
        self.cache.refresh_reported_runs(
            &self.options.claude_json,
            model::now_ms(),
            &mut self.dirty,
        );
        collect_facts(&self.options, &mut self.cache, &mut self.dirty)
    }

    /// Nothing is left to save: the event index commits each file's change
    /// as it reads it. Kept so callers that flush at the end still build.
    pub fn flush(&mut self) -> io::Result<()> {
        self.dirty = false;
        Ok(())
    }
}

fn collect_facts(
    options: &crate::Options,
    cache: &mut EventCache,
    dirty: &mut bool,
) -> io::Result<Facts> {
    let cwds = model::working_dirs(options, cache, dirty)?;
    let repos = cwds
        .into_iter()
        .map(|cwd| {
            let repo = model::repo_of(&cwd, env_home().as_deref().map(Path::new));
            (cwd, repo)
        })
        .collect();
    let mut proc_starts = BTreeMap::new();
    for pid in claude_pids(options) {
        if let Some(start) = proc_start(&options.proc_root, pid) {
            proc_starts.insert(pid, start);
        }
    }
    let codex_locks = codex_locks(options);
    // Run ids of the processes the model counts as live: a Claude pid file
    // whose start time matches, and each Codex writer lock's holder.
    let machine = MachineFacts::Local;
    let mut runs = BTreeMap::new();
    let live_pids: Vec<_> = model::pid_files(options, &machine)
        .into_iter()
        .filter(|pid| pid.alive)
        .collect();
    for pid in &live_pids {
        if pid.alive
            && let Some(run) = machine.run(options, pid.pid, pid.start)
        {
            runs.insert(pid.pid, run);
        }
    }
    for pid in codex_locks.values() {
        if let Some(run) = machine.run(options, *pid, None) {
            runs.insert(*pid, run);
        }
    }
    let process_ancestors = live_pids
        .iter()
        .map(|pid| pid.pid)
        .chain(codex_locks.values().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|pid| {
            machine
                .ancestors(options, pid)
                .map(|ancestors| (pid, ancestors))
        })
        .collect();
    Ok(Facts {
        process_ancestors,
        version: FACTS_VERSION,
        hostname: model::local_hostname(options),
        home: env_home(),
        proc_starts,
        codex_locks,
        codex_rollouts: Some(
            crate::inputs(options)?
                .into_iter()
                .filter(|input| input.root == crate::InputRoot::Codex)
                .map(|input| input.path)
                .collect(),
        ),
        repos,
        offline_since: None,
        runs,
        reported_runs: cache.reported_runs().cloned().collect(),
    })
}

pub(crate) fn env_home() -> Option<String> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| home.to_string_lossy().into_owned())
}

/// The pids of `claude/sessions/<pid>.json`. Only those names: `*.key`
/// files are never opened.
fn claude_pids(options: &Options) -> BTreeSet<u32> {
    let Ok(entries) = fs::read_dir(options.claude_home.join("sessions")) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let pid = name.to_str()?.strip_suffix(".json")?;
            (!pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| pid.parse().ok())
                .flatten()
        })
        .collect()
}

fn codex_locks(options: &Options) -> BTreeMap<String, u32> {
    let mut held = BTreeMap::new();
    #[cfg(unix)]
    if let (Ok(locks), Ok(entries)) = (
        fs::read_to_string(options.proc_root.join("locks")),
        fs::read_dir(options.codex_home.join("thread-writer-locks")),
    ) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".lock")) else {
                continue;
            };
            if let Ok((major, minor, ino)) = crate::lock_identity(&entry.path())
                && let Some(pid) = lock_pid(&locks, major, minor, ino)
            {
                held.insert(id.to_owned(), pid);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = options;
    held
}

/// Where the builder learns about the machine: this one, or a facts file.
pub(crate) enum MachineFacts {
    Local,
    Recorded(Box<Facts>),
}

/// A Codex writer lock's state, as the V1 tree reports it.
pub(crate) enum Lock {
    Held(u32),
    Free,
    Unknown,
}

impl MachineFacts {
    /// A facts file that is missing or unreadable reads as a machine with
    /// nothing running and nothing known.
    pub(crate) fn of(options: &Options) -> Self {
        match &options.facts {
            None => Self::Local,
            Some(path) => Self::Recorded(Box::new(read_facts(path).unwrap_or_default())),
        }
    }

    /// Current-source selection is collector evidence, never a guessed move.
    pub(crate) fn codex_rollout_is_current(&self, options: &Options, path: &Path) -> bool {
        let Self::Recorded(facts) = self else {
            return true;
        };
        let Some(paths) = &facts.codex_rollouts else {
            return true;
        };
        let Ok(relative) = path.strip_prefix(&options.codex_home) else {
            return false;
        };
        let relative = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        crate::is_input_path("codex", &relative) && paths.contains(&relative)
    }

    pub(crate) fn proc_start(&self, options: &Options, pid: u32) -> Option<u64> {
        match self {
            Self::Local => proc_start(&options.proc_root, pid),
            Self::Recorded(facts) => facts.proc_starts.get(&pid).copied(),
        }
    }

    pub(crate) fn hostname(&self, options: &Options) -> String {
        match self {
            Self::Local => model::local_hostname(options),
            Self::Recorded(facts) if facts.hostname.trim().is_empty() => "localhost".into(),
            Self::Recorded(facts) => facts.hostname.trim().to_owned(),
        }
    }

    /// When the machine was last seen, if its facts say it's offline.
    pub(crate) fn offline_since(&self) -> Option<i64> {
        match self {
            Self::Local => None,
            Self::Recorded(facts) => facts.offline_since,
        }
    }

    pub(crate) fn home(&self) -> Option<String> {
        match self {
            Self::Local => env_home(),
            Self::Recorded(facts) => facts.home.clone(),
        }
    }

    /// `None` when the repository comes from walking the local disk.
    pub(crate) fn recorded_repo(&self, cwd: &str) -> Option<Option<String>> {
        match self {
            Self::Local => None,
            Self::Recorded(facts) => Some(facts.repos.get(cwd).cloned().flatten()),
        }
    }

    /// Codex threads whose writer lock is held, with the holder's pid.
    /// `None` when this machine's `/proc/locks` can't be read: then nothing
    /// is known about any lock.
    pub(crate) fn codex_lock_pids(&self, options: &Options) -> Option<BTreeMap<String, u32>> {
        match self {
            Self::Local => fs::metadata(options.proc_root.join("locks"))
                .is_ok()
                .then(|| codex_locks(options)),
            Self::Recorded(facts) => Some(facts.codex_locks.clone()),
        }
    }

    pub(crate) fn ancestors(&self, options: &Options, pid: u32) -> Option<Vec<u32>> {
        match self {
            Self::Recorded(facts) => facts.process_ancestors.get(&pid).cloned(),
            Self::Local => {
                let initial = proc_start(&options.proc_root, pid)?;
                let mut ancestors = Vec::new();
                let mut seen = BTreeSet::new();
                let mut current = pid;
                // A bounded /proc walk, no environ or command line reads here.
                while ancestors.len() < 64 && seen.insert(current) {
                    let text = fs::read_to_string(
                        options.proc_root.join(current.to_string()).join("stat"),
                    )
                    .ok()?;
                    let tail = text.rsplit_once(')')?.1;
                    let parent = tail.split_whitespace().nth(1)?.parse::<u32>().ok()?;
                    if parent == 0 {
                        break;
                    }
                    ancestors.push(parent);
                    current = parent;
                }
                (proc_start(&options.proc_root, pid) == Some(initial)).then_some(ancestors)
            }
        }
    }

    /// A live process's run ids. On this machine they are read from
    /// `/proc/<pid>/environ`; when `start` is given, the process must still
    /// have that start time after the read, or it isn't the one the record
    /// names. Recorded facts give what the machine recorded.
    pub(crate) fn run(
        &self,
        options: &Options,
        pid: u32,
        start: Option<u64>,
    ) -> Option<BTreeMap<String, String>> {
        match self {
            Self::Local => {
                let run = read_run(&options.proc_root, pid)?;
                start
                    .is_none_or(|start| proc_start(&options.proc_root, pid) == Some(start))
                    .then_some(run)
            }
            Self::Recorded(facts) => facts.runs.get(&pid).cloned(),
        }
    }

    /// Whether a Codex thread has a writer-lock file. `None` for recorded
    /// facts, which name only the held locks.
    pub(crate) fn codex_lock_file(&self, options: &Options, id: &str) -> Option<bool> {
        match self {
            // An id is a file name, never a path.
            Self::Local => Some(
                !id.is_empty()
                    && !id.starts_with('.')
                    && !id.contains(['/', '\\'])
                    && options
                        .codex_home
                        .join("thread-writer-locks")
                        .join(format!("{id}.lock"))
                        .is_file(),
            ),
            Self::Recorded(_) => None,
        }
    }

    /// One Codex thread's writer lock, for the V1 tree.
    pub(crate) fn codex_lock(&self, options: &Options, locks: Option<&str>, id: &str) -> Lock {
        match self {
            Self::Recorded(facts) => match facts.codex_locks.get(id) {
                Some(pid) => Lock::Held(*pid),
                None => Lock::Free,
            },
            Self::Local => local_codex_lock(options, locks, id),
        }
    }
}

#[cfg(unix)]
fn local_codex_lock(options: &Options, locks: Option<&str>, id: &str) -> Lock {
    let Some(locks) = locks else {
        return Lock::Unknown;
    };
    match crate::lock_identity(
        &options
            .codex_home
            .join("thread-writer-locks")
            .join(format!("{id}.lock")),
    ) {
        Ok((major, minor, ino)) => match lock_pid(locks, major, minor, ino) {
            Some(pid) => Lock::Held(pid),
            None => Lock::Free,
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Lock::Free,
        Err(_) => Lock::Unknown,
    }
}

#[cfg(not(unix))]
fn local_codex_lock(_: &Options, _: Option<&str>, _: &str) -> Lock {
    Lock::Unknown
}

/// The run ids in `<proc_root>/<pid>/environ`. `None` when it can't be
/// read, is larger than [`ENVIRON_MAX`], or holds a run id that isn't UTF-8.
pub(crate) fn read_run(proc_root: &Path, pid: u32) -> Option<BTreeMap<String, String>> {
    let file = fs::File::open(proc_root.join(pid.to_string()).join("environ")).ok()?;
    // One buffer, big enough for the whole read: a growing one would leave
    // copies of the environment in freed memory that the wipe below never
    // reaches.
    let mut environ = Vec::with_capacity(ENVIRON_MAX as usize + 1);
    let read = file.take(ENVIRON_MAX + 1).read_to_end(&mut environ);
    let run = (read.is_ok() && environ.len() as u64 <= ENVIRON_MAX)
        .then(|| run_of(&environ))
        .flatten();
    wipe(&mut environ);
    run
}

/// Zeroes a buffer that held a process environment, so nothing else of it
/// outlives the parse in this process's memory. Volatile writes and a fence
/// keep the optimiser from dropping the stores as dead before the free.
/// Best-effort: the kernel's copy, and anything read before, are out of
/// reach.
fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, aligned, exclusive reference to a u8.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// The [`RUN_VARIABLES`] entries of a NUL-separated environment, by exact
/// name; the first of a repeated name wins, as `getenv` has it. Every other
/// entry is skipped by its name alone.
fn run_of(environ: &[u8]) -> Option<BTreeMap<String, String>> {
    let mut run = BTreeMap::new();
    for entry in environ.split(|byte| *byte == 0) {
        let Some(equals) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let Some(name) = RUN_VARIABLES
            .into_iter()
            .find(|name| entry[..equals] == *name.as_bytes())
        else {
            continue;
        };
        if run.contains_key(name) {
            continue;
        }
        let value = std::str::from_utf8(&entry[equals + 1..]).ok()?;
        if name == "SEMON_PARENT" && crate::parent_links::parse(value).is_none() {
            continue;
        }
        run.insert(name.to_owned(), value.to_owned());
    }
    Some(run)
}

/// Reads a facts file written by [`write_facts`] (or received as JSON).
pub fn read_facts(path: &Path) -> io::Result<Facts> {
    let mut file = fs::File::open(path)?;
    let before = source_authority::Identity::of(&file.metadata()?).ok();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let facts = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    // This legacy/full observation already reads all facts. It may repair a
    // missing derived index; focused reads never use it as a fallback.
    if let Some(identity) = before
        && source_authority::Identity::of(&file.metadata()?)
            .ok()
            .as_ref()
            == Some(&identity)
        && source_authority::CurrentSources::open(path).is_err()
    {
        let _ = source_authority::publish(path, &facts, &identity);
    }
    Ok(facts)
}

/// Writes facts as JSON, atomically.
pub fn write_facts(path: &Path, facts: &Facts) -> io::Result<()> {
    let file = crate::save_json_file(path, facts)?;
    if let Ok(metadata) = file.metadata()
        && let Ok(identity) = source_authority::Identity::of(&metadata)
    {
        // Facts remain the original record even if optional projection I/O
        // fails. Readers reject the old index's mismatched file identity.
        let _ = source_authority::publish(path, facts, &identity);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn a_wipe_zeroes_every_byte() {
        let mut bytes = b"OSTROM_RUN_ID=r1\0SECRET=x".to_vec();
        wipe(&mut bytes);
        assert!(bytes.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn only_the_run_variables_are_kept_by_exact_name() {
        let run = run_of(
            b"A=1\0OSTROM_RUN_ID=r1\0OSTROM_RUN_ID=later\0ostrom_run_id=lower\0OSTROM_RUN_ID_X=no\0\
              =empty\0NOEQUALS\0OSTROM_WORK_ORDER_ID=\0SECRET=OSTROM_RUN_ID=x",
        )
        .unwrap();
        assert_eq!(
            run,
            BTreeMap::from([
                ("OSTROM_RUN_ID".to_owned(), "r1".to_owned()),
                ("OSTROM_WORK_ORDER_ID".to_owned(), String::new()),
            ])
        );
        assert_eq!(run_of(b"PATH=/bin\0"), Some(BTreeMap::new()));
        assert_eq!(run_of(b""), Some(BTreeMap::new()));
        assert_eq!(run_of(b"OSTROM_RUN_ID=\xff\0"), None);
        // A non-UTF-8 entry that isn't a run variable is only skipped.
        assert_eq!(
            run_of(b"BLOB=\xff\0OSTROM_RUN_ID=r2"),
            Some(BTreeMap::from([(
                "OSTROM_RUN_ID".to_owned(),
                "r2".to_owned()
            )]))
        );
    }

    #[test]
    fn facts_source_reads_the_event_cache_once_for_repeated_facts() {
        let root = env::temp_dir().join(format!(
            "semon-facts-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let options = crate::Options {
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
        fs::create_dir_all(options.proc_root.join("sys/kernel")).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        fs::write(options.proc_root.join("sys/kernel/hostname"), "test-host\n").unwrap();
        fs::create_dir_all(&options.claude_home).unwrap();
        fs::create_dir_all(&options.codex_home).unwrap();
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let mut source = FactsSource::new(&options);
        let first = source.facts().unwrap();
        let second = source.facts().unwrap();
        assert_eq!(first, second);
        crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 1));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn facts_carry_only_allowlisted_claude_run_values() {
        let root = env::temp_dir().join(format!(
            "semon-facts-reported-runs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let options = crate::Options {
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
        fs::create_dir_all(options.proc_root.join("sys/kernel")).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        fs::write(options.proc_root.join("sys/kernel/hostname"), "fixture\n").unwrap();
        fs::create_dir_all(&options.claude_home).unwrap();
        fs::create_dir_all(&options.codex_home).unwrap();
        fs::write(
            &options.claude_json,
            serde_json::json!({
                "oauthAccount":{"emailAddress":"fixture-facts@example.invalid","accountUuid":"fixture-facts-account"},
                "projects":{"/private/project/path":{
                    "lastSessionId":"fixture-run","lastStartTime":1234,"lastCost":0.5,
                    "lastDuration":10,"lastAPIDuration":7,"lastToolDuration":2,
                    "lastLinesAdded":3,"lastLinesRemoved":1,"privateProjectValue":"project-secret",
                    "lastModelUsage":{"claude-opus-5[1m]":{
                        "inputTokens":10,"outputTokens":20,"thinkingTokens":4,
                        "cacheReadInputTokens":30,"cacheCreationInputTokens":5,
                        "webSearchRequests":1,"costUSD":0.5,"privateModelValue":"model-secret"
                    }}
                }}
            })
            .to_string(),
        )
        .unwrap();

        let facts = local_facts(&options).unwrap();
        assert_eq!(facts.reported_runs.len(), 1);
        let serialized = serde_json::to_string(&facts).unwrap();
        for secret in [
            "fixture-facts@example.invalid",
            "fixture-facts-account",
            "/private/project/path",
            "project-secret",
            "model-secret",
        ] {
            assert!(!serialized.contains(secret));
        }
        assert!(serialized.contains("fixture-run"));
        assert!(serialized.contains("inputTokens"));
        fs::remove_dir_all(root).unwrap();
    }

    /// Runs the JSON event cache captured before the SQLite index (a1) are
    /// in the facts again once it is imported: `~/.claude.json` has since
    /// overwritten them, so nothing else could give them back.
    #[test]
    fn reported_runs_from_before_the_index_show_again() {
        let root = env::temp_dir().join(format!(
            "semon-facts-legacy-runs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let options = crate::Options {
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
        fs::create_dir_all(options.proc_root.join("sys/kernel")).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        fs::write(options.proc_root.join("sys/kernel/hostname"), "fixture\n").unwrap();
        fs::create_dir_all(&options.claude_home).unwrap();
        fs::create_dir_all(&options.codex_home).unwrap();
        // Today's `~/.claude.json` holds only the latest run.
        fs::write(
            &options.claude_json,
            serde_json::json!({"projects":{"/p":{"lastSessionId":"latest-run","lastStartTime":2000}}})
                .to_string(),
        )
        .unwrap();
        // The JSON event cache still holds an earlier one.
        fs::write(
            crate::events::EventCache::legacy_path(&options.cache),
            serde_json::json!({
                "version": 14,
                "files": {},
                "reported_runs": {"earlier-run": {"1000": {
                    "lastSessionId":"earlier-run","lastStartTime":1000,"lastCost":0.25,"captureAt":1500
                }}}
            })
            .to_string(),
        )
        .unwrap();

        let facts = local_facts(&options).unwrap();
        let ids: BTreeSet<&str> = facts
            .reported_runs
            .iter()
            .map(|run| run.last_session_id.as_str())
            .collect();
        assert_eq!(ids, BTreeSet::from(["earlier-run", "latest-run"]));
        assert!(!crate::events::EventCache::legacy_path(&options.cache).exists());
        // And they stay: the next process reads them from the index.
        let again = local_facts(&options).unwrap();
        assert_eq!(again.reported_runs.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }
}

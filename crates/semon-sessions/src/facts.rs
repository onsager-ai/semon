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

pub const FACTS_VERSION: u32 = 1;

/// The environment variables Semon reads from a session's process, by exact
/// name: Ostrom's run contract (its `docs/loops.md`, "The run environment is
/// a contract"). Every other entry of the environment is dropped as it is
/// parsed, never kept, logged or compared beyond its name.
pub const RUN_VARIABLES: [&str; 2] = ["OSTROM_RUN_ID", "OSTROM_WORK_ORDER_ID"];

/// The most of `/proc/<pid>/environ` that is read. A larger environment
/// isn't read at all: a cut one could hold a cut value.
const ENVIRON_MAX: u64 = 256 * 1024;

/// The machine's side of a model, as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
/// found by walking up from each one to a `.git`.
pub fn local_facts(options: &Options) -> io::Result<Facts> {
    let cache_path = EventCache::path(&options.cache);
    let mut cache = EventCache::read(&cache_path);
    let mut dirty = false;
    let cwds = model::working_dirs(options, &mut cache, &mut dirty)?;
    if dirty {
        cache.save(&cache_path)?;
    }
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
    for pid in model::pid_files(options, &machine) {
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
    Ok(Facts {
        version: FACTS_VERSION,
        hostname: model::local_hostname(options),
        home: env_home(),
        proc_starts,
        codex_locks,
        repos,
        offline_since: None,
        runs,
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
    Recorded(Facts),
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
            Some(path) => Self::Recorded(read_facts(path).unwrap_or_default()),
        }
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
    let mut environ = Vec::new();
    let read = file.take(ENVIRON_MAX + 1).read_to_end(&mut environ);
    let run = (read.is_ok() && environ.len() as u64 <= ENVIRON_MAX)
        .then(|| run_of(&environ))
        .flatten();
    // Nothing else of the environment outlives the parse.
    environ.fill(0);
    run
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
        run.insert(name.to_owned(), value.to_owned());
    }
    Some(run)
}

/// Reads a facts file written by [`write_facts`] (or received as JSON).
pub fn read_facts(path: &Path) -> io::Result<Facts> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Writes facts as JSON, atomically.
pub fn write_facts(path: &Path, facts: &Facts) -> io::Result<()> {
    crate::save_json(path, facts)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

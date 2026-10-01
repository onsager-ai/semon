//! [`ReceivedMachines`]: the machines a mirror receiver keeps under one
//! directory, `DIR/machines/<name>/`, each with its `claude/` and `codex/`
//! homes (the input files, as `semon_sessions::inputs` names them) and its
//! `facts.json`.
//!
//! DIR is only read: never written, and no symbolic link is followed out of
//! a machine's directory. A machine directory that is a link is ignored, and
//! so is a home whose root (`claude/`, `claude/projects/`,
//! `claude/sessions/`, `codex/`, `codex/sessions/`) is one. Below the roots,
//! the builder lists regular files and directories only, never links.
//!
//! A machine's directory is named as [`is_machine_name`] says; any other
//! entry is ignored, with a warning.
//!
//! What the reader keeps of its own (each machine's metadata cache, and the
//! facts it serves) goes beside the `--cache` file, under
//! `received/<DIR's hash>/<name>/`. A machine's facts are copied there,
//! checked: a `facts.json` that is missing, not a regular file or unreadable
//! gives an offline machine named after its directory, with a warning, and
//! so does a hostname that can't name a machine. Facts not rewritten for
//! [`STALE_AFTER`] make the machine offline since they were written.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{FACTS_VERSION, Facts, Options, model::fnv};

/// The largest `facts.json` read. A larger one reads as unreadable.
const FACTS_MAX: u64 = 16 * 1024 * 1024;

/// How old a received `facts.json` may be before its machine counts as
/// offline. `semon push --watch` rewrites it every 10 seconds.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(120);

/// Whether `name` can name a received machine: 1 to 63 of `a-z`, `0-9` and
/// `-`. A receiver names machines' directories with it, and the viewer
/// serves only directories so named.
pub fn is_machine_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Whether a received hostname can name a machine in the union: 1 to 253
/// bytes, with no control character, whitespace or `~` (the union's own
/// separator).
fn is_hostname(name: &str) -> bool {
    (1..=253).contains(&name.len())
        && !name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '~')
}

/// The machines under `DIR/machines/`, as a receiver writes them. A
/// [`crate::ViewerCore`] made [`with_received`](crate::ViewerCore::with_received)
/// follows them: a machine added while it serves is served from the next
/// request on.
#[derive(Clone, Debug)]
pub struct ReceivedMachines {
    dir: PathBuf,
    options: Options,
    state: PathBuf,
}

/// A `facts.json` as one pass saw it, without reading it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FactsSeen {
    Missing,
    /// There, but not a regular file (a link, a directory).
    NotFile,
    /// A regular file: its length, modification time (ns), device and
    /// inode.
    File(u64, u128, u64, u64),
}

/// What one pass saw of a machine's directory: which homes can be read,
/// and its facts file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) claude: bool,
    pub(crate) codex: bool,
    pub(crate) facts: FactsSeen,
    /// The facts file is older than [`STALE_AFTER`].
    pub(crate) stale: bool,
}

/// Every entry of `DIR/machines/`, by name: a machine, or `None` for an
/// entry that isn't one (a file, a link, a name that can't be a machine's).
/// An entry that isn't a machine is keyed by its name escaped for a
/// terminal (`{:?}`), which no machine's name can be.
pub(crate) type Listing = BTreeMap<String, Option<Seen>>;

impl ReceivedMachines {
    /// The machines under `dir/machines/`. Each is read with `options`' window
    /// (`since`, `all`, `session`, `scan_window`); its homes, facts and
    /// cache are its own. Nothing is read until a core asks.
    pub fn new(dir: impl Into<PathBuf>, options: &Options) -> Self {
        let dir = dir.into();
        let identity = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        let state = options
            .cache
            .parent()
            .unwrap_or(Path::new("."))
            .join("received")
            .join(format!("{:016x}", fnv(&identity.to_string_lossy())));
        Self {
            dir,
            options: options.clone(),
            state,
        }
    }

    /// The directory the machines are under: `<dir>/machines/`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// One pass over `DIR/machines/`: file types and the facts file's stamp
    /// only, nothing opened.
    pub(crate) fn scan(&self) -> Listing {
        let mut listing = BTreeMap::new();
        let Ok(entries) = fs::read_dir(self.dir.join("machines")) else {
            return listing;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            let Some(name) = file_name
                .to_str()
                .filter(|name| is_dir && is_machine_name(name))
            else {
                listing.insert(format!("{file_name:?}"), None);
                continue;
            };
            let root = entry.path();
            let claude = root.join("claude");
            let codex = root.join("codex");
            let facts = fs::symlink_metadata(root.join("facts.json"));
            let stale = facts.as_ref().is_ok_and(|metadata| {
                metadata
                    .modified()
                    .is_ok_and(|modified| modified + STALE_AFTER < now)
            });
            let seen = Seen {
                claude: !is_link(&claude)
                    && !is_link(&claude.join("projects"))
                    && !is_link(&claude.join("sessions")),
                codex: !is_link(&codex) && !is_link(&codex.join("sessions")),
                facts: match facts {
                    Err(_) => FactsSeen::Missing,
                    Ok(metadata) if !metadata.is_file() => FactsSeen::NotFile,
                    Ok(metadata) => {
                        let (dev, ino) = identity(&metadata);
                        FactsSeen::File(
                            metadata.len(),
                            metadata
                                .modified()
                                .ok()
                                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                                .map_or(0, |time| time.as_nanos()),
                            dev,
                            ino,
                        )
                    }
                },
                stale,
            };
            listing.insert(name.to_owned(), Some(seen));
        }
        listing
    }

    /// Machine `name`'s options: its homes under DIR (or nowhere, for a home
    /// behind a link), and its cache and facts under this reader's own
    /// directory. `proc_root` and `claude_json` are never read with facts.
    pub(crate) fn options(&self, name: &str, seen: &Seen) -> Options {
        let root = self.dir.join("machines").join(name);
        let state = self.state.join(name);
        // Never created: a home that can't be read reads as empty.
        let nowhere = state.join("none");
        Options {
            claude_home: if seen.claude {
                root.join("claude")
            } else {
                nowhere.clone()
            },
            claude_json: nowhere.clone(),
            codex_home: if seen.codex {
                root.join("codex")
            } else {
                nowhere.clone()
            },
            proc_root: nowhere,
            cache: state.join("sessions-index.json"),
            facts: Some(state.join("facts.json")),
            ..self.options.clone()
        }
    }

    /// Copies machine `name`'s facts to where its options read them, and
    /// says on stderr what was wrong with them, if anything: missing or
    /// unreadable facts give an offline machine named `name`, a hostname
    /// that can't name a machine gives way to `name`, and stale facts are
    /// offline since they were written. Each warning is given once while
    /// its cause lasts (`warned` holds those given).
    pub(crate) fn copy_facts(
        &self,
        name: &str,
        seen: &Seen,
        hosts: &[String],
        warned: &mut BTreeSet<String>,
    ) {
        let mut warn = |kind: &str, now: bool, message: &dyn Fn() -> String| {
            let key = format!("{kind}:{name}");
            if !now {
                warned.remove(&key);
            } else if warned.insert(key) {
                eprintln!("semon: received machine {name}: {}", message());
            }
        };
        let root = self.dir.join("machines").join(name);
        let read = match seen.facts {
            FactsSeen::Missing => Err("it has no facts.json".to_owned()),
            FactsSeen::NotFile => Err("its facts.json is not a regular file".to_owned()),
            FactsSeen::File(..) => read_facts(&root.join("facts.json"))
                .map_err(|error| format!("its facts.json can't be read ({error})")),
        };
        warn("facts", read.is_err(), &|| {
            format!(
                "{}; it is shown as offline, with nothing running",
                read.as_ref().err().cloned().unwrap_or_default()
            )
        });
        let facts = match read {
            Ok(mut facts) => {
                let hostname = facts.hostname.trim().to_owned();
                let valid = is_hostname(&hostname);
                warn("hostname", !valid, &|| {
                    format!("its hostname {hostname:?} can't name a machine; it is shown as {name}")
                });
                facts.hostname = if valid { hostname } else { name.to_owned() };
                warn("local", hosts.contains(&facts.hostname), &|| {
                    format!(
                        "it has this machine's hostname, {}; it is served as a separate machine \
                         (--no-local leaves this one out)",
                        facts.hostname
                    )
                });
                let stale = seen.stale && facts.offline_since.is_none();
                warn("stale", stale, &|| {
                    format!(
                        "its facts are over {} seconds old; it is shown as offline since they \
                         were written",
                        STALE_AFTER.as_secs()
                    )
                });
                if stale {
                    facts = facts.offline(modified_ms(&root.join("facts.json")));
                }
                facts
            }
            Err(_) => Facts {
                version: FACTS_VERSION,
                hostname: name.to_owned(),
                ..Facts::default()
            }
            .offline(modified_ms(&root)),
        };
        let copy = self.state.join(name).join("facts.json");
        // A heartbeat refreshes the receiver's liveness stamp, but identical
        // normalized facts must not rewrite this copy and rebuild every model.
        if read_facts(&copy).is_ok_and(|previous| previous == facts) {
            return;
        }
        if let Err(error) = crate::write_facts(&copy, &facts) {
            eprintln!("semon: received machine {name}: can't keep its facts: {error}");
        }
    }
}

/// A file's modification time (epoch ms), or 0.
fn modified_ms(path: &Path) -> i64 {
    fs::symlink_metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|time| i64::try_from(time.as_millis()).ok())
        .unwrap_or(0)
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

#[cfg(unix)]
fn identity(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn identity(_: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// A received `facts.json`, at most [`FACTS_MAX`] bytes, read from the
/// regular file its path names (checked on the opened file). A file
/// replaced between the check and the open is read again, up to three
/// times.
fn read_facts(path: &Path) -> io::Result<Facts> {
    let mut attempts = 0;
    let bytes = loop {
        attempts += 1;
        match crate::read_regular_at_most(path, FACTS_MAX) {
            Err(error) if error.kind() == io::ErrorKind::InvalidInput && attempts < 3 => {}
            read => break read?,
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

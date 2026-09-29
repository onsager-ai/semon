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
//! What the reader keeps of its own (each machine's metadata cache, and the
//! facts it serves) goes beside the `--cache` file, under
//! `received/<DIR's hash>/<name>/`. A machine's facts are copied there,
//! checked: a `facts.json` that is missing, not a regular file or unreadable
//! gives an offline machine named after its directory, with a warning.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::{FACTS_VERSION, Facts, Options, model::fnv};

/// The largest `facts.json` read. A larger one reads as unreadable.
const FACTS_MAX: u64 = 16 * 1024 * 1024;

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
    /// A regular file: its length, modification time (ns) and inode.
    File(u64, u128, u64),
}

/// What one pass saw of a machine's directory: which homes can be read,
/// and its facts file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) claude: bool,
    pub(crate) codex: bool,
    pub(crate) facts: FactsSeen,
}

/// Every entry of `DIR/machines/`, by name: a machine, or `None` for an
/// entry that isn't one (a file, a link, a name that can't be a machine's).
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
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // A hidden entry is the receiver's own (a temporary file, say).
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if !is_dir || !machine_name(&name) {
                listing.insert(name, None);
                continue;
            }
            let root = entry.path();
            let claude = root.join("claude");
            let codex = root.join("codex");
            let seen = Seen {
                claude: !is_link(&claude)
                    && !is_link(&claude.join("projects"))
                    && !is_link(&claude.join("sessions")),
                codex: !is_link(&codex) && !is_link(&codex.join("sessions")),
                facts: match fs::symlink_metadata(root.join("facts.json")) {
                    Err(_) => FactsSeen::Missing,
                    Ok(metadata) if !metadata.is_file() => FactsSeen::NotFile,
                    Ok(metadata) => FactsSeen::File(
                        metadata.len(),
                        metadata
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map_or(0, |time| time.as_nanos()),
                        inode(&metadata),
                    ),
                },
            };
            listing.insert(name, Some(seen));
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
    /// unreadable facts give an offline machine named `name`.
    pub(crate) fn copy_facts(&self, name: &str, seen: &Seen, hosts: &[String]) {
        let root = self.dir.join("machines").join(name);
        let path = root.join("facts.json");
        let read = match seen.facts {
            FactsSeen::Missing => Err("it has no facts.json".to_owned()),
            FactsSeen::NotFile => Err("its facts.json is not a regular file".to_owned()),
            FactsSeen::File(_, _, inode) => read_facts(&path, inode)
                .map_err(|error| format!("its facts.json can't be read ({error})")),
        };
        let facts = match read {
            Ok(mut facts) => {
                if facts.hostname.trim().is_empty() {
                    facts.hostname = name.to_owned();
                }
                if hosts.iter().any(|host| *host == facts.hostname.trim()) {
                    eprintln!(
                        "semon: received machine {name} has this machine's hostname, {}; \
                         it is served as a separate machine (--no-local leaves this one out)",
                        facts.hostname.trim()
                    );
                }
                facts
            }
            Err(reason) => {
                eprintln!(
                    "semon: received machine {name}: {reason}; it is shown as offline, \
                     with nothing running"
                );
                let last_seen = fs::metadata(&root)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .and_then(|time| i64::try_from(time.as_millis()).ok())
                    .unwrap_or(0);
                Facts {
                    version: FACTS_VERSION,
                    hostname: name.to_owned(),
                    ..Facts::default()
                }
                .offline(last_seen)
            }
        };
        if let Err(error) = crate::write_facts(&self.state.join(name).join("facts.json"), &facts) {
            eprintln!("semon: received machine {name}: can't keep its facts: {error}");
        }
    }
}

/// A machine directory's name: a single path component, not hidden, with
/// no control characters.
fn machine_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

#[cfg(unix)]
fn inode(metadata: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(metadata)
}

#[cfg(not(unix))]
fn inode(_: &fs::Metadata) -> u64 {
    0
}

/// A received `facts.json`, at most [`FACTS_MAX`] bytes: the regular file
/// the pass saw (`inode`), not whatever a link put there since.
fn read_facts(path: &Path, expected: u64) -> io::Result<Facts> {
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || inode(&metadata) != expected {
        return Err(io::Error::other("it changed while being read"));
    }
    let mut bytes = Vec::new();
    file.take(FACTS_MAX + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > FACTS_MAX {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "too large"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

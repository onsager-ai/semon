//! Launch calls shared by the tree and model, kept in memory only.

use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

#[cfg(test)]
use std::fs;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde_json::Value;

use crate::{ToolCall, events, field};

/// Invariant: the scan cache holds at most this many parents, evicting LRU.
const MAX_PARENTS: usize = 256;
/// Invariant: each parent retains at most this many calls, keeping the newest.
const MAX_CALLS: usize = 4096;
/// Inputs are transient, but must not grow the cache without a byte limit.
const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
const CLOCK_SKEW_MS: i64 = 2000;

#[cfg(test)]
thread_local! {
    pub(crate) static PARSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    identity: (u64, u64),
}

impl Stamp {
    fn read(file: &crate::sealed::LogFile) -> io::Result<Self> {
        let meta = file.metadata()?;
        #[cfg(unix)]
        let identity = (meta.dev(), meta.ino());
        #[cfg(not(unix))]
        let identity = (0, 0);
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            identity,
        })
    }
}

struct Call {
    tool: ToolCall,
    time: Option<i64>,
    input: String,
}

struct Parent {
    stamp: Stamp,
    /// All bytes read, including the unfinished line retained in `partial`.
    offset: u64,
    partial: Vec<u8>,
    /// An oversized record is skipped through its next newline.
    discarding: bool,
    calls: VecDeque<Call>,
    used: u64,
    input_bytes: usize,
}

impl Parent {
    fn scan(&mut self, mut file: crate::sealed::LogFile, stamp: Stamp) -> io::Result<()> {
        file.seek(SeekFrom::Start(self.offset))?;
        // Read only the statted snapshot; a writer's next append is scanned next time.
        let mut reader = BufReader::new(file.take(stamp.len - self.offset));
        loop {
            let room = MAX_INPUT_BYTES + 1 - self.partial.len();
            let read = reader
                .by_ref()
                .take(room as u64)
                .read_until(b'\n', &mut self.partial)?;
            self.offset += read as u64;
            #[cfg(test)]
            BYTES.with(|count| count.set(count.get() + read as u64));
            if read == 0 {
                break;
            }
            let complete = self.partial.last() == Some(&b'\n');
            if self.discarding || self.partial.len() > MAX_INPUT_BYTES {
                self.discarding = !complete;
                self.partial.clear();
                // Avoid retaining a multi-megabyte allocation for an ignored line.
                self.partial.shrink_to(8192);
                continue;
            }
            // Logs are append-only within a generation. A complete JSON value
            // at EOF is also a record; keep an incomplete value for the next append.
            if self.partial.last() != Some(&b'\n')
                && serde_json::from_slice::<Value>(&self.partial).is_err()
            {
                break;
            }
            if [b"Semon-Handoff".as_slice(), b"codex".as_slice()]
                .iter()
                .any(|needle| {
                    self.partial
                        .windows(needle.len())
                        .any(|part| part == *needle)
                })
            {
                #[cfg(test)]
                PARSES.with(|count| count.set(count.get() + 1));
                if let Ok(record) = serde_json::from_slice::<Value>(&self.partial)
                    && let Some(blocks) = record["message"]["content"].as_array()
                {
                    for block in blocks {
                        if field(block, "type") != Some("tool_use") {
                            continue;
                        }
                        let Some(name @ ("Bash" | "Skill")) = field(block, "name") else {
                            continue;
                        };
                        let (Some(id), Some(input)) = (field(block, "id"), block.get("input"))
                        else {
                            continue;
                        };
                        let input = input.to_string();
                        if input
                            .len()
                            .saturating_add(id.len())
                            .saturating_add(name.len())
                            > MAX_INPUT_BYTES
                        {
                            continue;
                        }
                        self.input_bytes += input.len() + id.len() + name.len();
                        self.calls.push_back(Call {
                            tool: ToolCall {
                                id: id.into(),
                                name: name.into(),
                            },
                            time: events::record_time(&record),
                            input,
                        });
                        while self.calls.len() > MAX_CALLS || self.input_bytes > MAX_INPUT_BYTES {
                            if let Some(old) = self.calls.pop_front() {
                                self.input_bytes -=
                                    old.input.len() + old.tool.id.len() + old.tool.name.len();
                            }
                        }
                    }
                }
            }
            self.partial.clear();
        }
        self.stamp = stamp;
        Ok(())
    }
}

#[derive(Default)]
struct ScanCache {
    parents: BTreeMap<PathBuf, Parent>,
    tick: u64,
}

impl ScanCache {
    fn parent(&mut self, path: &Path) -> io::Result<&Parent> {
        let file = crate::sealed::LogFile::open(path)?;
        let stamp = Stamp::read(&file)?;
        self.tick += 1;
        let reset = self.parents.get(path).is_none_or(|parent| {
            parent.stamp.identity != stamp.identity
                || stamp.len < parent.stamp.len
                || (stamp.len == parent.stamp.len && stamp.modified != parent.stamp.modified)
        });
        if reset {
            if !self.parents.contains_key(path) && self.parents.len() == MAX_PARENTS {
                let oldest = self
                    .parents
                    .iter()
                    .min_by_key(|(_, parent)| parent.used)
                    .map(|(path, _)| path.clone());
                if let Some(oldest) = oldest {
                    self.parents.remove(&oldest);
                }
            }
            self.parents.insert(
                path.into(),
                Parent {
                    stamp,
                    offset: 0,
                    partial: Vec::new(),
                    discarding: false,
                    calls: VecDeque::new(),
                    used: self.tick,
                    input_bytes: 0,
                },
            );
        }
        let parent = self.parents.get_mut(path).expect("parent inserted above");
        parent.used = self.tick;
        if reset || parent.stamp != stamp {
            // An unsuccessful read must be retried from scratch, rather than
            // returning a partially refreshed set of calls on the next lookup.
            if let Err(error) = parent.scan(file, stamp) {
                self.parents.remove(path);
                return Err(error);
            }
        }
        Ok(&self.parents[path])
    }

    fn find(
        &mut self,
        paths: &[PathBuf],
        prompt: &str,
        cwd: Option<&str>,
        start: Option<i64>,
    ) -> Option<ToolCall> {
        let mut named = Vec::new();
        let mut fallback = Vec::new();
        let prompt_name = Path::new(prompt).file_name().and_then(|name| name.to_str());
        let cwd = cwd.filter(|cwd| {
            Path::new(cwd).components().count() >= 3
                && std::env::var_os("HOME").is_none_or(|home| Path::new(cwd) != Path::new(&home))
        });
        for path in paths {
            let Ok(parent) = self.parent(path) else {
                continue;
            };
            if let Some(call) = parent.calls.iter().find(|call| call.input.contains(prompt)) {
                return Some(call.tool.clone());
            }
            for call in &parent.calls {
                if prompt_name.is_some_and(|name| matches_segment(&call.input, name)) {
                    named.push(call.tool.clone());
                }
                if let (Some(cwd), Some(start), Some(time)) = (cwd, start, call.time)
                    && time <= start.saturating_add(CLOCK_SKEW_MS)
                    && matches_cwd(&call.input, cwd)
                {
                    fallback.push((time, call.tool.clone()));
                }
            }
        }
        // A filename still identifies a variable-built prompt path. Neither
        // filename nor worktree evidence chooses arbitrarily between launches.
        if named.len() == 1 {
            named.pop()
        } else if named.is_empty() {
            fallback.sort_by_key(|(time, _)| *time);
            let (time, call) = fallback.pop()?;
            if fallback
                .last()
                .is_some_and(|(other, _)| time.saturating_sub(*other) <= CLOCK_SKEW_MS)
            {
                None
            } else {
                Some(call)
            }
        } else {
            None
        }
    }
}

/// A shell path component ends at slash, whitespace or shell punctuation.
/// Dots, hyphens and underscores stay in the name, so `foo` cannot join `foobar`.
fn matches_segment(input: &str, needle: &str) -> bool {
    let boundary =
        |character: char| character.is_whitespace() || "/\\\"'`;|&(){}[]<>=$,:".contains(character);
    let contains = |needle: &str| {
        !needle.is_empty()
            && input.match_indices(needle).any(|(at, _)| {
                input[..at].chars().next_back().is_none_or(boundary)
                    && input[at + needle.len()..]
                        .chars()
                        .next()
                        .is_none_or(boundary)
            })
    };
    contains(needle)
}

fn matches_cwd(input: &str, cwd: &str) -> bool {
    let cwd = cwd.trim_end_matches('/');
    matches_segment(input, cwd)
        || cwd
            .rsplit('/')
            .next()
            .is_some_and(|name| matches_segment(input, name))
}

/// Literal prompt first, then an unambiguous prompt name or worktree match.
pub(crate) fn find(
    paths: &[PathBuf],
    prompt: &str,
    cwd: Option<&str>,
    start: Option<i64>,
) -> Option<ToolCall> {
    static CACHE: OnceLock<Mutex<ScanCache>> = OnceLock::new();
    CACHE
        .get_or_init(|| Mutex::new(ScanCache::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .find(paths, prompt, cwd, start)
}

#[cfg(test)]
mod tests;

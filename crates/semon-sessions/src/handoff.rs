//! Launch calls shared by the tree and model, kept in memory only.

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde_json::Value;

use crate::{ToolCall, events, field};

/// Invariant: the scan cache holds at most this many parents, evicting LRU.
const MAX_PARENTS: usize = 256;
/// Invariant: each parent retains at most this many calls, keeping the newest.
const MAX_CALLS: usize = 4096;
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
    fn read(path: &Path) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
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
    calls: VecDeque<Call>,
    used: u64,
}

impl Parent {
    fn scan(&mut self, path: &Path, stamp: Stamp) -> io::Result<()> {
        let mut file = crate::sealed::LogFile::open(path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        // Read only the statted snapshot; a writer's next append is scanned next time.
        let mut reader = BufReader::new(file.take(stamp.len - self.offset));
        loop {
            let read = reader.read_until(b'\n', &mut self.partial)?;
            self.offset += read as u64;
            #[cfg(test)]
            BYTES.with(|count| count.set(count.get() + read as u64));
            if read == 0 {
                break;
            }
            if self.partial.last() != Some(&b'\n') {
                break;
            }
            if [b"Semon-Handoff".as_slice(), b"codex exec".as_slice()]
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
                        self.calls.push_back(Call {
                            tool: ToolCall {
                                id: id.into(),
                                name: name.into(),
                            },
                            time: events::record_time(&record),
                            input: input.to_string(),
                        });
                        if self.calls.len() > MAX_CALLS {
                            self.calls.pop_front();
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
        let stamp = Stamp::read(path)?;
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
                    calls: VecDeque::new(),
                    used: self.tick,
                },
            );
        }
        let parent = self.parents.get_mut(path).expect("parent inserted above");
        parent.used = self.tick;
        if reset || parent.stamp != stamp {
            // An unsuccessful read must be retried from scratch, rather than
            // returning a partially refreshed set of calls on the next lookup.
            if let Err(error) = parent.scan(path, stamp) {
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
        let mut fallback: Option<(i64, ToolCall)> = None;
        for path in paths {
            let Ok(parent) = self.parent(path) else {
                continue;
            };
            if let Some(call) = parent.calls.iter().find(|call| call.input.contains(prompt)) {
                return Some(call.tool.clone());
            }
            if let (Some(cwd), Some(start)) = (cwd, start) {
                for call in &parent.calls {
                    if let Some(time) = call.time
                        && time <= start.saturating_add(CLOCK_SKEW_MS)
                        && matches_cwd(&call.input, cwd)
                        && fallback.as_ref().is_none_or(|(latest, _)| time >= *latest)
                    {
                        fallback = Some((time, call.tool.clone()));
                    }
                }
            }
        }
        fallback.map(|(_, tool)| tool)
    }
}

/// A shell path component ends at slash, whitespace or shell punctuation.
/// Dots, hyphens and underscores stay in the name, so `foo` cannot join `foobar`.
fn matches_cwd(input: &str, cwd: &str) -> bool {
    let cwd = cwd.trim_end_matches('/');
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
    contains(cwd) || cwd.rsplit('/').next().is_some_and(contains)
}

/// Literal prompt first, then the latest eligible call naming the worktree.
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

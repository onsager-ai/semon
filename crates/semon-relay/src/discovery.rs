use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use thiserror::Error;

/// One append-only Claude Code carrier stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredStream {
    pub session: String,
    pub stream: String,
    pub path: PathBuf,
}

/// Errors while walking the deliberately narrow Claude Code layout.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("cannot inspect projects root {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("two files claim relay stream {session}/{stream}: {first} and {second}")]
    Duplicate {
        session: String,
        stream: String,
        first: PathBuf,
        second: PathBuf,
    },
}

/// Discovers main transcripts and subagent JSONL streams, but never sidecars.
pub fn discover_streams(root: &Path) -> Result<Vec<DiscoveredStream>, DiscoveryError> {
    let mut found = BTreeMap::<(String, String), PathBuf>::new();
    for project in entries(root)? {
        let project = project.map_err(|source| DiscoveryError::Read {
            path: root.to_path_buf(),
            source,
        })?;
        let project_path = project.path();
        if !file_type(&project)?.is_dir() {
            continue;
        }
        for entry in entries(&project_path)? {
            let entry = entry.map_err(|source| DiscoveryError::Read {
                path: project_path.clone(),
                source,
            })?;
            let path = entry.path();
            let kind = file_type(&entry)?;
            if kind.is_file() && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            {
                let Some(session) = path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .map(str::to_owned)
                else {
                    continue;
                };
                insert(&mut found, &session, "main", path)?;
                continue;
            }
            if !kind.is_dir() {
                continue;
            }
            let Some(session) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let subagents = path.join("subagents");
            if !subagents.is_dir() {
                continue;
            }
            for subagent in entries(&subagents)? {
                let subagent = subagent.map_err(|source| DiscoveryError::Read {
                    path: subagents.clone(),
                    source,
                })?;
                let subagent_path = subagent.path();
                if !file_type(&subagent)?.is_file() {
                    continue;
                }
                let Some(name) = subagent_path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if name.starts_with("agent-") && name.ends_with(".jsonl") {
                    insert(
                        &mut found,
                        session,
                        &format!("subagents/{name}"),
                        subagent_path,
                    )?;
                }
            }
        }
    }
    for stream in discover_codex_streams(root)? {
        insert(&mut found, &stream.session, &stream.stream, stream.path)?;
    }
    Ok(found
        .into_iter()
        .map(|((session, stream), path)| DiscoveredStream {
            session,
            stream,
            path,
        })
        .collect())
}

/// Narrows discovered streams to only the named sessions, keeping every
/// stream that belongs to one (a main transcript and any subagent files),
/// so a filter selects whole sessions rather than individual files.
pub fn filter_by_session(
    streams: Vec<DiscoveredStream>,
    sessions: &BTreeSet<String>,
) -> Vec<DiscoveredStream> {
    streams
        .into_iter()
        .filter(|stream| sessions.contains(&stream.session))
        .collect()
}

fn entries(path: &Path) -> Result<fs::ReadDir, DiscoveryError> {
    fs::read_dir(path).map_err(|source| DiscoveryError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn file_type(entry: &fs::DirEntry) -> Result<fs::FileType, DiscoveryError> {
    entry.file_type().map_err(|source| DiscoveryError::Read {
        path: entry.path(),
        source,
    })
}

fn insert(
    found: &mut BTreeMap<(String, String), PathBuf>,
    session: &str,
    stream: &str,
    path: PathBuf,
) -> Result<(), DiscoveryError> {
    let key = (session.to_owned(), stream.to_owned());
    if let Some(first) = found.insert(key, path.clone()) {
        return Err(DiscoveryError::Duplicate {
            session: session.to_owned(),
            stream: stream.to_owned(),
            first,
            second: path,
        });
    }
    Ok(())
}

/// Narrow rollout discovery under an explicitly selected Codex sessions root.
/// SQLite, sidecars, malformed metadata and symlinks are never carrier streams.
pub fn discover_codex_streams(root: &Path) -> Result<Vec<DiscoveredStream>, DiscoveryError> {
    let mut found = BTreeMap::new();
    fn walk(
        root: &Path,
        path: &Path,
        depth: usize,
        found: &mut BTreeMap<(String, String), PathBuf>,
    ) -> Result<(), DiscoveryError> {
        for entry in entries(path)? {
            let entry = entry.map_err(|source| DiscoveryError::Read {
                path: path.to_path_buf(),
                source,
            })?;
            let kind = file_type(&entry)?;
            let path = entry.path();
            if depth < 3 {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if kind.is_dir()
                    && name.len() == if depth == 0 { 4 } else { 2 }
                    && name.bytes().all(|b| b.is_ascii_digit())
                {
                    walk(root, &path, depth + 1, found)?;
                }
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("walk is under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                if !valid_codex_path(&relative) {
                    continue;
                }
                let file = fs::File::open(&path).map_err(|source| DiscoveryError::Read {
                    path: path.clone(),
                    source,
                })?;
                let mut line = String::new();
                BufReader::new(file)
                    .take(1024 * 1024)
                    .read_line(&mut line)
                    .map_err(|source| DiscoveryError::Read {
                        path: path.clone(),
                        source,
                    })?;
                let Ok(meta) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if meta["type"] != "session_meta" {
                    continue;
                }
                let Some(id) = meta["payload"]["id"].as_str() else {
                    continue;
                };
                if id.is_empty()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    || !relative.ends_with(&format!("-{id}.jsonl"))
                {
                    continue;
                }
                insert(found, id, &format!("codex/{relative}"), path)?;
            }
        }
        Ok(())
    }
    walk(root, root, 0, &mut found)?;
    Ok(found
        .into_iter()
        .map(|((session, stream), path)| DiscoveredStream {
            session,
            stream,
            path,
        })
        .collect())
}

pub(crate) fn valid_codex_path(path: &str) -> bool {
    let p = path.split('/').collect::<Vec<_>>();
    let year = p.first().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let month = p.get(1).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
    let day = p.get(2).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        1..=12 => 31,
        _ => 0,
    };
    year > 0
        && day > 0
        && day <= days
        && p.len() == 4
        && p[0].len() == 4
        && p[0].bytes().all(|b| b.is_ascii_digit())
        && p[1].len() == 2
        && p[1].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n))
        && p[2].len() == 2
        && p[2].parse::<u8>().is_ok_and(|n| (1..=31).contains(&n))
        && p[3].starts_with("rollout-")
        && p[3].ends_with(".jsonl")
        && p[3]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

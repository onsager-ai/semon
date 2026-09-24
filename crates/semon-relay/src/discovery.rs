use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
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

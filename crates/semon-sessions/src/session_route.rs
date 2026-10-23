//! Public, source-qualified session addresses. Hosts supply authorized metadata;
//! neither a URL nor a candidate grants permission to read native bytes.
use std::{collections::BTreeMap, io};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRoute {
    pub source: Option<String>,
    pub key: String,
    pub turn: Option<String>,
    pub legacy_harness: Option<String>,
    pub anchor: Option<String>,
    pub controls: BTreeMap<String, String>,
}

fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "Malformed session address")
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !matches!(value, "." | "..")
        && !value.chars().any(char::is_control)
}

/// Decode once, after splitting on literal slashes. '+' is literal in a path.
pub fn decode(value: &str) -> io::Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = input.next().and_then(|b| (b as char).to_digit(16));
            let low = input.next().and_then(|b| (b as char).to_digit(16));
            bytes.push((high.ok_or_else(malformed)? * 16 + low.ok_or_else(malformed)?) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| malformed())
}

pub fn segment(value: &str) -> String {
    crate::native_html::path_segment(value)
}

pub fn session_path(source: &str, key: &str) -> String {
    format!("/session/{}/{}", segment(source), segment(key))
}

pub fn trace_path(source: &str, key: &str, turn: &str) -> String {
    format!("{}/trace/{}", session_path(source, key), segment(turn))
}

impl SessionRoute {
    pub fn parse(path: &str, query: &str) -> io::Result<Self> {
        if path.len() > 32768 || query.len() > 8192 {
            return Err(malformed());
        }
        let parts = path
            .strip_prefix('/')
            .ok_or_else(malformed)?
            .split('/')
            .map(decode)
            .collect::<io::Result<Vec<_>>>()?;
        if parts.iter().any(|part| !identifier(part)) {
            return Err(malformed());
        }
        let strings: Vec<_> = parts.iter().map(String::as_str).collect();
        let (source, key, turn, legacy) = match strings.as_slice() {
            ["session", source, key] => (Some((*source).into()), *key, None, None),
            ["session", source, key, "trace", turn] => {
                (Some((*source).into()), *key, Some((*turn).into()), None)
            }
            ["s", harness, key]
                if matches!(*harness, "native" | "codex" | "claude" | "copilot") =>
            {
                (None, *key, None, Some((*harness).into()))
            }
            ["trace", harness, key, turn]
                if matches!(*harness, "native" | "codex" | "claude" | "copilot") =>
            {
                (None, *key, Some((*turn).into()), Some((*harness).into()))
            }
            _ => return Err(malformed()),
        };
        let mut route = Self {
            source,
            key: key.into(),
            turn,
            legacy_harness: legacy,
            anchor: None,
            controls: BTreeMap::new(),
        };
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = decode(&name.replace('+', " "))?;
            let value = decode(&value.replace('+', " "))?;
            if value.chars().any(char::is_control) {
                return Err(malformed());
            }
            if name == "machine" && route.legacy_harness.is_some() {
                if route.source.is_some() || !identifier(&value) {
                    return Err(malformed());
                }
                route.source = Some(value);
                continue;
            }
            if name == "turn" && route.legacy_harness.is_some() && route.turn.is_none() {
                if route.anchor.is_some() || !identifier(&value) {
                    return Err(malformed());
                }
                route.anchor = Some(value);
                continue;
            }
            let valid = match name.as_str() {
                "harness" => matches!(value.as_str(), "" | "codex" | "claude" | "copilot"),
                "repo" | "model" | "q" => value.len() <= 4096,
                "no_repo" | "all_machines" | "native" => value == "1",
                "after" | "field_chunk" => {
                    !value.is_empty()
                        && value.bytes().all(|b| b.is_ascii_digit())
                        && value
                            .parse::<u64>()
                            .is_ok_and(|n| n <= 9_007_199_254_740_991)
                }
                "generation" => !value.is_empty() && value.len() <= 4096,
                "part" => matches!(value.as_str(), "output" | "script" | "attachment"),
                _ => false,
            };
            if !valid || route.controls.insert(name, value).is_some() {
                return Err(malformed());
            }
        }
        if route.controls.contains_key("repo") && route.controls.contains_key("no_repo")
            || route.controls.contains_key("field_chunk") && route.controls.contains_key("part")
        {
            return Err(malformed());
        }
        if route
            .controls
            .get("after")
            .is_some_and(|after| after.parse::<u64>().unwrap_or(0) > 0)
            && !route.controls.contains_key("generation")
            || (route.controls.contains_key("field_chunk") || route.controls.contains_key("part"))
                && (!route.controls.contains_key("after")
                    || !route.controls.contains_key("generation"))
        {
            return Err(malformed());
        }
        Ok(route)
    }

    pub fn canonical(&self, source: &str, key: &str) -> String {
        let mut path = match &self.turn {
            Some(turn) => trace_path(source, key, turn),
            None => session_path(source, key),
        };
        if !self.controls.is_empty() {
            path.push('?');
            path.push_str(
                &self
                    .controls
                    .iter()
                    .map(|(key, value)| format!("{}={}", segment(key), segment(value)))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        if let Some(anchor) = &self.anchor {
            path.push('#');
            path.push_str(&segment(anchor));
        }
        path
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Candidate {
    pub source_key: String,
    pub catalog_key: String,
    pub harness: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    Valid(usize),
    NotFound,
    Ambiguous,
    Mismatch,
}

/// Candidates must already be authorized, unrevoked and from the active scope.
/// Exact opaque keys win; provenance and display labels never select a reader.
pub fn resolve(route: &SessionRoute, candidates: &[Candidate]) -> Resolution {
    resolve_candidates(route, candidates, false)
}

fn resolve_candidates(route: &SessionRoute, candidates: &[Candidate], indexed: bool) -> Resolution {
    let matches: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            (candidate.catalog_key == route.key
                || indexed && route.legacy_harness.is_some() && route.key.starts_with("unsent:"))
                && route
                    .source
                    .as_ref()
                    .is_none_or(|source| *source == candidate.source_key)
        })
        .collect();
    match matches.as_slice() {
        [] => Resolution::NotFound,
        [(index, candidate)] => {
            if route
                .legacy_harness
                .as_ref()
                .is_some_and(|h| h != "native" && *h != candidate.harness)
            {
                Resolution::Mismatch
            } else {
                Resolution::Valid(*index)
            }
        }
        _ => Resolution::Ambiguous,
    }
}

/// Only call with results from the authorized, complete session_bookmark index.
/// The historical unsent target is an indexed bookmark, never a canonical alias.
pub fn resolve_bookmark(route: &SessionRoute, candidates: &[Candidate]) -> Resolution {
    resolve_candidates(route, candidates, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_route_shared_vectors() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/session-route-vectors.json"
        ))
        .unwrap();
        for vector in vectors.as_array().unwrap() {
            let route = SessionRoute::parse(
                vector["path"].as_str().unwrap(),
                vector["query"].as_str().unwrap_or(""),
            );
            if vector["outcome"] == "malformed" {
                assert!(route.is_err(), "{vector}");
                continue;
            }
            let route = route.unwrap_or_else(|error| panic!("{vector}: {error}"));
            let candidates: Vec<Candidate> =
                serde_json::from_value(vector["candidates"].clone()).unwrap();
            let result = if vector["indexed_bookmark"] == true {
                resolve_bookmark(&route, &candidates)
            } else {
                resolve(&route, &candidates)
            };
            let outcome = match result {
                Resolution::Valid(_) => "valid",
                Resolution::NotFound => "not_found",
                Resolution::Ambiguous => "ambiguous",
                Resolution::Mismatch => "mismatch",
            };
            assert_eq!(outcome, vector["outcome"], "{vector}");
            if let Resolution::Valid(index) = result {
                assert_eq!(
                    route.canonical(
                        &candidates[index].source_key,
                        &candidates[index].catalog_key
                    ),
                    vector["canonical"],
                    "{vector}"
                );
            }
        }
    }
}

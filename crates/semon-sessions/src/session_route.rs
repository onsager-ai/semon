//! Public, source-qualified session addresses. Hosts supply authorized metadata;
//! neither a URL nor a candidate grants permission to read native bytes.
use std::{collections::BTreeMap, io};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRoute {
    pub source: String,
    pub key: String,
    pub turn: Option<String>,
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRecordAddress {
    pub(crate) path: String,
    pub(crate) offset: u64,
    pub(crate) block: i64,
    pub(crate) occurrence: i64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeEntryAddress {
    pub(crate) record: NativeRecordAddress,
    pub(crate) binding: String,
    pub(crate) block: u32,
    pub(crate) entry_id: String,
}

impl NativeEntryAddress {
    pub(crate) fn parse(value: &str) -> io::Result<Self> {
        let address: Self = serde_json::from_str(value).map_err(|_| malformed())?;
        let ordinal = |value| (0..=9_007_199_254_740_991).contains(&value);
        if !identifier(&address.record.path)
            || address.record.offset > 9_007_199_254_740_991
            || !ordinal(address.record.block)
            || !ordinal(address.record.occurrence)
            || address.binding.len() != 64
            || !address.binding.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !identifier(&address.entry_id)
        {
            return Err(malformed());
        }
        Ok(address)
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedPeerAddress {
    pub(crate) trace: String,
    pub(crate) witness: String,
}

impl RetainedPeerAddress {
    pub(crate) fn parse(value: &str) -> io::Result<Self> {
        let peer: Self = serde_json::from_str(value).map_err(|_| malformed())?;
        if !identifier(&peer.trace)
            || peer.witness.len() != 64
            || !peer.witness.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(malformed());
        }
        Ok(peer)
    }
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
        let (source, key, turn) = match strings.as_slice() {
            ["session", source, key] => (*source, *key, None),
            ["session", source, key, "trace", turn] => (*source, *key, Some((*turn).into())),
            _ => return Err(malformed()),
        };
        let mut route = Self {
            source: source.into(),
            key: key.into(),
            turn,
            controls: BTreeMap::new(),
        };
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = decode(&name.replace('+', " "))?;
            let value = decode(&value.replace('+', " "))?;
            if value.chars().any(char::is_control) {
                return Err(malformed());
            }
            let valid = match name.as_str() {
                "harness" => matches!(value.as_str(), "" | "codex" | "claude" | "copilot"),
                "repo" | "model" | "q" => value.len() <= 4096,
                "no_repo" | "all_machines" => value == "1",
                "scope" => matches!(value.as_str(), "current" | "retained_history"),
                "after" | "field_chunk" => {
                    !value.is_empty()
                        && value.bytes().all(|b| b.is_ascii_digit())
                        && value
                            .parse::<u64>()
                            .is_ok_and(|n| n <= 9_007_199_254_740_991)
                }
                "generation" => !value.is_empty() && value.len() <= 4096,
                "address" => NativeEntryAddress::parse(&value).is_ok(),
                "peer" => RetainedPeerAddress::parse(&value).is_ok(),
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
                && !route.controls.contains_key("address")
                && (!route.controls.contains_key("after")
                    || !route.controls.contains_key("generation"))
            || route.controls.contains_key("address")
                && (route.controls.contains_key("after")
                    || route.controls.contains_key("generation"))
        {
            return Err(malformed());
        }
        if route.controls.contains_key("peer")
            && (route.controls.get("scope").map(String::as_str) != Some("retained_history")
                || (route.turn.is_some() && !route.controls.contains_key("address"))
                || route.controls.contains_key("generation"))
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
        path
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Candidate {
    pub source_key: String,
    pub catalog_key: String,
    pub harness: String,
}

impl From<crate::CatalogSessionIdentity> for Candidate {
    fn from(identity: crate::CatalogSessionIdentity) -> Self {
        Self {
            source_key: identity.source_key,
            catalog_key: identity.catalog_key,
            harness: identity.harness,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    Valid(usize),
    NotFound,
    Ambiguous,
}

/// Candidates must already be authorized, unrevoked and from the active scope.
/// Exact opaque keys win; provenance and display labels never select a reader.
pub fn resolve(route: &SessionRoute, candidates: &[Candidate]) -> Resolution {
    let matches: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate.catalog_key == route.key && route.source == candidate.source_key
        })
        .collect();
    match matches.as_slice() {
        [] => Resolution::NotFound,
        [(index, _)] => Resolution::Valid(*index),
        _ => Resolution::Ambiguous,
    }
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
            let result = resolve(&route, &candidates);
            let outcome = match result {
                Resolution::Valid(_) => "valid",
                Resolution::NotFound => "not_found",
                Resolution::Ambiguous => "ambiguous",
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

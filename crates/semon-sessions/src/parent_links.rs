//! Exact observed parent links, persisted without prompt text or paths.
use crate::{Marker, Options, facts::MachineFacts};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_LINKS: usize = 4096;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Link {
    pub(crate) parent: String,
    pub(crate) call: Option<String>,
    pub(crate) source: String,
    observed: i64,
}
#[derive(Clone, Copy)]
pub(crate) struct Process {
    pub(crate) pid: u32,
    pub(crate) start: Option<u64>,
}

/// Parses only identifiers, never a prompt path or arbitrary environment text.
pub(crate) fn parse(value: &str) -> Option<(String, Option<String>)> {
    if value.len() > 512 {
        return None;
    }
    let mut fields = value.split(':');
    let harness = fields.next()?;
    if !matches!(harness, "claude" | "codex") {
        return None;
    }
    let id = fields.next()?;
    let identifier = |value: &str| {
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
    };
    if !identifier(id) {
        return None;
    }
    let call = fields.next();
    if fields.next().is_some() || call.is_some_and(|call| !identifier(call)) {
        return None;
    }
    Some((format!("{harness}:{id}"), call.map(str::to_owned)))
}
fn priority(source: &str) -> usize {
    match source {
        "marker" => 3,
        "environment" => 2,
        "ancestry" => 1,
        _ => 0,
    }
}

pub(crate) fn resolve(
    options: &Options,
    facts: &MachineFacts,
    processes: &BTreeMap<String, Process>,
    markers: &BTreeMap<String, Marker>,
    known: &BTreeSet<String>,
    native: &BTreeMap<String, String>,
    now: i64,
) -> BTreeMap<String, Link> {
    let path = options.cache.with_extension("parent-links.json");
    let mut links: BTreeMap<String, Link> = crate::read_regular_at_most(&path, MAX_FILE_BYTES)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    links.retain(|child, link| {
        parse(child).is_some()
            && parse(&link.parent).is_some()
            && child != &link.parent
            && priority(&link.source) > 0
            && link
                .call
                .as_ref()
                .is_none_or(|call| parse(&format!("codex:id:{call}")).is_some())
    });
    let previous = serde_json::to_vec(&links).ok();
    // A pid shared by several sessions does not establish which one launched
    // a child. Only a unique known live session is ancestry evidence.
    let mut by_pid: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    for (key, process) in processes {
        if process
            .start
            .is_some_and(|start| facts.proc_start(options, process.pid) != Some(start))
        {
            continue;
        }
        by_pid.entry(process.pid).or_default().push(key);
    }
    let mut offer = |child: &str, parent: String, call: Option<String>, source: &str| {
        if child == parent || !known.contains(&parent) {
            return;
        }
        let update = links
            .get(child)
            .is_none_or(|old| priority(source) >= priority(&old.source));
        if update {
            let unchanged = links.get(child).is_some_and(|old| {
                old.parent == parent && old.call == call && old.source == source
            });
            if !unchanged {
                links.insert(
                    child.into(),
                    Link {
                        parent,
                        call,
                        source: source.into(),
                        observed: now,
                    },
                );
            }
        }
    };
    for (child, process) in processes {
        if process
            .start
            .is_some_and(|start| facts.proc_start(options, process.pid) != Some(start))
        {
            continue;
        }
        // Native parent-thread/subagent metadata already establishes a
        // parent; the explicit SEMON_PARENT value can still add a call id.
        let has_native = native.contains_key(child);
        for ancestor in facts
            .ancestors(options, process.pid)
            .unwrap_or_default()
            .into_iter()
            .filter(|_| !has_native)
        {
            if let Some(parents) = by_pid.get(&ancestor) {
                if let [parent] = parents.as_slice() {
                    offer(child, (*parent).into(), None, "ancestry");
                }
                break;
            }
        }
        if let Some(run) = facts.run(options, process.pid, process.start)
            && let Some(value) = run.get("SEMON_PARENT")
            && let Some((parent, call)) = parse(value)
        {
            offer(child, parent, call, "environment");
        }
    }
    for (child, marker) in markers {
        offer(
            child,
            format!("{}:{}", marker.harness, marker.parent_id),
            marker.tool_id.clone(),
            "marker",
        );
    }
    // Validate against both observed and existing native links. Cycles never
    // become a tree, regardless of whether the input was a marker or a cache.
    let graph: BTreeMap<String, String> = native
        .iter()
        .map(|(a, b)| (a.clone(), b.clone()))
        .chain(links.iter().map(|(a, b)| (a.clone(), b.parent.clone())))
        .collect();
    links.retain(|child, _| {
        let mut at = child.as_str();
        let mut seen = BTreeSet::new();
        while let Some(parent) = graph.get(at) {
            if !seen.insert(at) {
                return false;
            }
            at = parent;
        }
        true
    });
    while links.len() > MAX_LINKS {
        let oldest = links
            .iter()
            .min_by_key(|(_, link)| link.observed)
            .map(|(key, _)| key.clone())
            .unwrap();
        links.remove(&oldest);
    }
    if serde_json::to_vec(&links).ok() != previous {
        // Persistence is a cache: a read-only machine still reports the exact
        // currently observed links and may retry persistence on its next scan.
        let _ = crate::save_json(&path, &links);
    }
    links.retain(|child, link| known.contains(child) && known.contains(&link.parent));
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_are_bounded_and_paths_or_extra_fields_are_rejected() {
        assert_eq!(
            parse("codex:child:call_1"),
            Some(("codex:child".into(), Some("call_1".into())))
        );
        for value in [
            "claude:",
            "unknown:id",
            "codex:/secret/path",
            "codex:id:call:extra",
            "codex:id:../path",
        ] {
            assert!(parse(value).is_none(), "{value}");
        }
    }
}

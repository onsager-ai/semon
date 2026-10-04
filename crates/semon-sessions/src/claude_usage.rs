//! Copied usage is evidence without an established original session owner.
//! Match only a persisted assistant record UUID AND API message identity.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Tokens;

pub(crate) type Records = BTreeMap<String, BTreeSet<String>>;
pub(crate) type Shared = BTreeMap<String, String>;

/// Primary token fields contain only exclusive observations when this is present.
/// Neither an exclusive observation nor a copied record establishes a native bill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeUsageEvidence {
    pub observed: Tokens,
    pub exclusive: Tokens,
    pub shared: BTreeMap<String, Tokens>,
    pub shared_models: BTreeMap<String, Option<String>>,
    /// Native persistence does not establish the original branch owner.
    pub fresh: Option<Tokens>,
    pub shared_owner: Option<String>,
}

/// Owners are modeled physical sessions/continuations, scoped to one machine.
/// Message IDs alone, filenames, ordering and text never establish copying.
pub(crate) fn shared<'a>(
    owners: impl IntoIterator<Item = (&'a str, &'a Records)>,
) -> BTreeMap<String, Shared> {
    let mut identities: BTreeMap<&str, BTreeMap<&str, BTreeSet<&str>>> = BTreeMap::new();
    for (owner, records) in owners {
        for (message, uuids) in records {
            for uuid in uuids {
                identities
                    .entry(message)
                    .or_default()
                    .entry(uuid)
                    .or_default()
                    .insert(owner);
            }
        }
    }
    let mut out: BTreeMap<String, Shared> = BTreeMap::new();
    for (message, records) in identities {
        let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for owners in records.values().filter(|owners| owners.len() > 1) {
            for owner in owners {
                edges.entry(owner).or_default().extend(owners);
            }
        }
        let mut seen = BTreeSet::new();
        for owner in edges.keys() {
            if seen.contains(owner) {
                continue;
            }
            let mut component = BTreeSet::new();
            let mut pending = vec![*owner];
            while let Some(owner) = pending.pop() {
                if component.insert(owner) {
                    pending.extend(edges.get(owner).into_iter().flatten().copied());
                }
            }
            seen.extend(component.iter().copied());
            let uuid = records
                .iter()
                .find(|(_, owners)| owners.len() > 1 && !owners.is_disjoint(&component))
                .map(|(uuid, _)| *uuid)
                .expect("shared component has an exact record identity");
            let key = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&(message, uuid)).expect("identity serializes"))
            );
            for owner in component {
                out.entry(owner.to_owned())
                    .or_default()
                    .insert(message.to_owned(), key.clone());
            }
        }
    }
    out
}

pub(crate) fn evidence<'a>(
    usage: impl IntoIterator<Item = (&'a String, &'a Tokens, Option<&'a str>)>,
    shared: &Shared,
) -> Option<ClaudeUsageEvidence> {
    if shared.is_empty() {
        return None;
    }
    let mut observed = Tokens::default();
    let mut exclusive = Tokens::default();
    let mut copied = BTreeMap::new();
    let mut shared_models = BTreeMap::new();
    for (id, tokens, model) in usage {
        add(&mut observed, tokens);
        if let Some(key) = shared.get(id) {
            copied.insert(key.clone(), tokens.clone());
            shared_models.insert(key.clone(), model.map(str::to_owned));
        } else {
            add(&mut exclusive, tokens);
        }
    }
    Some(ClaudeUsageEvidence {
        observed,
        exclusive,
        shared: copied,
        shared_models,
        fresh: None,
        shared_owner: None,
    })
}

fn add(total: &mut Tokens, tokens: &Tokens) {
    total.input += tokens.input;
    total.cached_input += tokens.cached_input;
    total.output += tokens.output;
    total.reasoning_output += tokens.reasoning_output;
    total.total += tokens.total;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_assistant_record_and_message_identity_establish_copying() {
        let a = Records::from([(
            "api".into(),
            BTreeSet::from(["uuid".into(), "stream-block".into()]),
        )]);
        let b = Records::from([("api".into(), BTreeSet::from(["uuid".into()]))]);
        let other_uuid = Records::from([("api".into(), BTreeSet::from(["different".into()]))]);
        let other_api = Records::from([("different".into(), BTreeSet::from(["uuid".into()]))]);
        let found = shared([("a", &a), ("b", &b), ("c", &other_uuid), ("d", &other_api)]);
        assert_eq!(found.len(), 2);
        assert_eq!(found["a"], found["b"]);
        assert!(shared([("a", &a), ("a", &b)]).is_empty());
        let streamed = Records::from([("api".into(), BTreeSet::from(["stream-block".into()]))]);
        let groups = shared([
            ("a", &a),
            ("b", &b),
            ("streamed", &streamed),
            ("c", &other_uuid),
        ]);
        assert_eq!(groups["a"], groups["b"]);
        assert_eq!(groups["b"], groups["streamed"]);
        assert!(!groups.contains_key("c"));
    }
}

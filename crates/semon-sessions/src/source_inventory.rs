//! Configured source metadata, independent of native history and machine facts.
use std::{collections::BTreeMap, ops::Bound};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::viewer::{ViewerReply, decoded};

/// Build once when an authorized source configuration changes, then reuse for
/// bounded keyset reads. Labels describe configuration, never observed health.
pub struct SessionSourceInventory {
    sources: BTreeMap<String, (String, usize)>,
    generation: String,
    valid: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    api: u32,
    generation: String,
    after: String,
}

impl SessionSourceInventory {
    pub fn new(sources: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut rows = BTreeMap::new();
        let mut valid = true;
        for (index, (key, label)) in sources.into_iter().enumerate() {
            valid &= !key.is_empty() && key.len() <= 4096 && label.len() <= 4096;
            if rows.insert(key, (label, index)).is_some() {
                valid = false;
            }
        }
        let generation = format!("{:x}", Sha256::digest(serde_json::to_vec(&rows).unwrap()));
        Self {
            sources: rows,
            generation,
            valid,
        }
    }

    pub(crate) fn valid(&self) -> bool {
        self.valid
    }

    pub(crate) fn index(&self, key: &str) -> Option<usize> {
        self.sources.get(key).map(|(_, index)| *index)
    }

    /// API 1: limit 1..=100 (default 60), cursor bound to the configuration.
    /// The caller must authorize the complete configured inventory first.
    pub fn page(&self, query: &str) -> ViewerReply {
        let invalid = || {
            crate::catalog::error(
                400,
                "invalid_arguments",
                "Use a limit from 1 to 100 and a valid source cursor.",
                false,
            )
        };
        if !self.valid {
            return crate::catalog::error(
                409,
                "source_inventory_unavailable",
                "Configured source keys must be unique, nonempty, and bounded.",
                false,
            );
        }
        let mut limit = None;
        let mut cursor = None;
        for parameter in query.split('&').filter(|part| !part.is_empty()) {
            let Some((key, value)) = parameter.split_once('=') else {
                return invalid();
            };
            let Some(value) = decoded(value) else {
                return invalid();
            };
            match key {
                "limit" if limit.is_none() => {
                    limit = value.parse::<usize>().ok();
                    if !limit.is_some_and(|value| (1..=100).contains(&value)) {
                        return invalid();
                    }
                }
                "cursor" if cursor.is_none() && value.len() <= 16384 => {
                    let Ok(parsed) = serde_json::from_str::<Cursor>(&value) else {
                        return invalid();
                    };
                    if parsed.api != 1 || parsed.after.is_empty() || parsed.after.len() > 4096 {
                        return invalid();
                    }
                    if parsed.generation != self.generation {
                        return crate::catalog::error(
                            409,
                            "stale_cursor",
                            "The configured source inventory changed. Restart this list.",
                            true,
                        );
                    }
                    cursor = Some(parsed);
                }
                _ => return invalid(),
            }
        }
        let limit = limit.unwrap_or(60);
        let after = cursor.as_ref().map(|cursor| cursor.after.as_str());
        let start = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut page = self
            .sources
            .range::<str, _>((start, Bound::Unbounded))
            .take(limit + 1);
        let mut items = Vec::with_capacity(limit);
        for _ in 0..limit {
            let Some((key, (label, _))) = page.next() else {
                break;
            };
            items.push(json!({"source_key":key,"label":label}));
        }
        let next_cursor = if page.next().is_some() {
            let after = items.last().unwrap()["source_key"]
                .as_str()
                .unwrap()
                .to_owned();
            Some(
                serde_json::to_string(&Cursor {
                    api: 1,
                    generation: self.generation.clone(),
                    after,
                })
                .unwrap(),
            )
        } else {
            None
        };
        ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: json!({"api":1,"items":items,"next_cursor":next_cursor})
                .to_string()
                .into_bytes(),
            etag: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_sources_page_without_history_and_bind_configuration() {
        let inventory = SessionSourceInventory::new(
            (0..20001).map(|index| (format!("source-{index:05}"), format!("Configured {index}"))),
        );
        let first: serde_json::Value =
            serde_json::from_slice(&inventory.page("limit=2").body).unwrap();
        assert_eq!(first["items"][0]["source_key"], "source-00000");
        let query = format!(
            "limit=2&cursor={}",
            crate::viewer::percent_encode(first["next_cursor"].as_str().unwrap())
        );
        let second: serde_json::Value =
            serde_json::from_slice(&inventory.page(&query).body).unwrap();
        assert_eq!(second["items"][0]["source_key"], "source-00002");
        assert_eq!(inventory.index("source-20000"), Some(20000));
        let changed = SessionSourceInventory::new([("source-00000".into(), "changed".into())]);
        assert_eq!(changed.page(&query).status, 409);
        for bad in [
            "limit=0",
            "limit=101",
            "limit=2&limit=3",
            "cursor=%GG",
            "machine=x",
            "cursor={}",
        ] {
            assert_eq!(inventory.page(bad).status, 400, "{bad}");
        }
        let duplicate = SessionSourceInventory::new([
            ("same".into(), "one".into()),
            ("same".into(), "two".into()),
        ]);
        assert_eq!(duplicate.page("").status, 409);
    }
}

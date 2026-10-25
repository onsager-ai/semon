//! Borrowed Tree dimension values shared by physical storage writers.
//! This is a row delta, not a complete persisted generation or a source reader.
use crate::native_tree::Summary;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct TreeRows<'a> {
    pub(crate) codex_turn: String,
    labels: [(&'static str, Option<&'a str>); 4],
    pub(crate) models: &'a BTreeSet<String>,
    pub(crate) tools: &'a BTreeMap<String, String>,
    pub(crate) closed_tools: &'a BTreeSet<String>,
    pub(crate) usage_models: &'a BTreeMap<String, String>,
}

impl<'a> TreeRows<'a> {
    pub(crate) fn new(summary: &'a Summary) -> serde_json::Result<Self> {
        Ok(Self {
            codex_turn: serde_json::to_string(&summary.codex_turn)?,
            labels: [
                ("first", summary.first.as_deref()),
                ("last", summary.last.as_deref()),
                ("cwd", summary.cwd.as_deref()),
                ("branch", summary.branch.as_deref()),
            ],
            models: &summary.models,
            tools: &summary.tools,
            closed_tools: &summary.closed_tools,
            usage_models: &summary.usage_models,
        })
    }

    pub(crate) fn labels(&self) -> impl Iterator<Item = (&'static str, &'a str)> + '_ {
        self.labels.iter().filter_map(|&(key, value)| value.map(|value| (key, value)))
    }
}

// Qualification recipe only: production keeps its existing SQLite witness
// until every persisted-generation reader can supply the same typed rows.
// These tags are the existing witness's domain bytes, not backend query SQL.
#[cfg(test)]
pub(crate) mod witness {
    use sha2::{Digest, Sha256};

    pub(crate) fn digest<'a>(
        labels: impl IntoIterator<Item = (&'a str, &'a str)>,
        models: impl IntoIterator<Item = &'a str>,
        tools: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> String {
        let mut digest = Sha256::new();
        // Callers supply persisted rows in SQLite BINARY (UTF-8 byte) order.
        let families = [
            ("SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label", labels.into_iter().map(|(key, value)| serde_json::to_string(&[key, value]).unwrap()).collect::<Vec<_>>()),
            ("SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY model", models.into_iter().map(|model| serde_json::to_string(&[model]).unwrap()).collect()),
            ("SELECT json_array(tool_id,name) FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id", tools.into_iter().map(|(id, name)| serde_json::to_string(&[id, name]).unwrap()).collect()),
        ];
        for (domain, rows) in families {
            digest.update(domain.as_bytes());
            for row in rows {
                digest.update((row.len() as u64).to_be_bytes());
                digest.update(row.as_bytes());
            }
        }
        format!("{:x}", digest.finalize())
    }
}

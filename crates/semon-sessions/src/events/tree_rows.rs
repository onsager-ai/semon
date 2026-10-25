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
        self.labels
            .iter()
            .filter_map(|&(key, value)| value.map(|value| (key, value)))
    }
}

// Canonical Tree witness recipe shared by every physical backend. Each backend
// supplies its persisted rows in UTF-8 byte order (SQLite BINARY, PostgreSQL
// bytea); the recipe owns row encoding and digest framing. String arrays
// always serialize. The domain tags
// remain the original witness's fixed bytes, not backend query SQL.
pub(crate) mod witness {
    use sha2::{Digest, Sha256};

    const LABELS: &str = "SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label";
    const MODELS: &str =
        "SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY model";
    const TOOLS: &str = "SELECT json_array(tool_id,name) FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id";

    /// Incremental canonical Tree witness writer.  A physical reader begins a
    /// family, then supplies one persisted row at a time in its binary order.
    /// SQLite may pass its already-canonical `json_array` value directly;
    /// typed backends use the typed helpers, which serialize to the same bytes.
    pub(crate) struct Writer(Sha256);

    impl Writer {
        pub(crate) fn new() -> Self {
            Self(Sha256::new())
        }

        pub(crate) fn labels(&mut self) {
            self.0.update(LABELS.as_bytes());
        }

        pub(crate) fn models(&mut self) {
            self.0.update(MODELS.as_bytes());
        }

        pub(crate) fn tools(&mut self) {
            self.0.update(TOOLS.as_bytes());
        }

        /// Accept an already encoded SQLite `json_array` row.  This is also
        /// the common framing operation used by typed serializers.
        pub(crate) fn encoded(&mut self, row: &str) {
            self.0.update((row.len() as u64).to_be_bytes());
            self.0.update(row.as_bytes());
        }

        #[cfg(any(feature = "postgres", test))]
        pub(crate) fn label(&mut self, key: &str, value: &str) {
            self.encoded(&serde_json::to_string(&[key, value]).unwrap());
        }

        #[cfg(any(feature = "postgres", test))]
        pub(crate) fn model(&mut self, model: &str) {
            self.encoded(&serde_json::to_string(&[model]).unwrap());
        }

        #[cfg(any(feature = "postgres", test))]
        pub(crate) fn tool(&mut self, id: &str, name: &str) {
            self.encoded(&serde_json::to_string(&[id, name]).unwrap());
        }

        pub(crate) fn finish(self) -> String {
            format!("{:x}", self.0.finalize())
        }
    }

    #[cfg(test)]
    pub(crate) fn digest<'a>(
        labels: impl IntoIterator<Item = (&'a str, &'a str)>,
        models: impl IntoIterator<Item = &'a str>,
        tools: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> String {
        let mut writer = Writer::new();
        writer.labels();
        for (key, value) in labels {
            writer.label(key, value);
        }
        writer.models();
        for model in models {
            writer.model(model);
        }
        writer.tools();
        for (id, name) in tools {
            writer.tool(id, name);
        }
        writer.finish()
    }
}

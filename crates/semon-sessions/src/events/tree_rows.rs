//! Borrowed Tree dimension values shared by physical storage writers.
//! This is a row delta, not a complete persisted generation or a source reader.
use crate::native_tree::Summary;
use std::collections::{BTreeMap, BTreeSet};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TreeSummaryRecord {
    pub(crate) version: u32,
    pub(crate) parser_version: u32,
    pub(crate) ledger: String,
    #[serde(default)]
    pub(crate) projection_witness: Option<String>,
    #[serde(default)]
    pub(crate) interpretation_witness: Option<String>,
    pub(crate) summary: Summary,
}

impl TreeSummaryRecord {
    /// One current codec for both physical readers. Absence of either witness
    /// is not permission to return a complete Current summary.
    pub(crate) fn decode_current(
        encoded: &[u8],
        ledger: &str,
        projection: Option<&str>,
        interpretation: &str,
    ) -> Option<Self> {
        if encoded.len() > 1048576 {
            return None;
        }
        let record: Self = serde_json::from_slice(encoded).ok()?;
        if record.version != 3
            || record.parser_version != super::CACHE_VERSION
            || record.ledger != ledger
            || projection.is_none()
            || record.projection_witness.as_deref() != projection
            || record.interpretation_witness.as_deref() != Some(interpretation)
        {
            return None;
        }
        Some(record)
    }

    pub(crate) fn new(
        ledger: String,
        projection_witness: Option<String>,
        interpretation: String,
        summary: &Summary,
        normalized_usage: bool,
    ) -> Self {
        Self {
            version: 3,
            parser_version: super::CACHE_VERSION,
            ledger,
            projection_witness,
            interpretation_witness: Some(interpretation),
            summary: summary.compact(normalized_usage),
        }
    }
}

pub(crate) fn interpretation(values: &[Option<String>; 3]) -> serde_json::Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(values)?)))
}

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

// One current authenticated stream. Both engines traverse their persisted
// fixed-width addresses, never sort a materialized dimension. Namespace is
// bound by the captured header and physical owner, not by an interchangeable
// logical source's witness bytes.
pub(crate) mod witness {
    use sha2::{Digest, Sha256};

    const LABELS: &str = "semon.tree.witness.4.labels";
    const MODELS: &str = "semon.tree.witness.4.models";
    const TOOLS: &str = "semon.tree.witness.4.tools";

    pub(crate) fn address(domain: &str, key: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(domain.as_bytes());
        hash.update(key);
        hash.finalize().into()
    }

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
        pub(crate) fn tool(&mut self, id: &str, name: Option<&str>, closed: bool) {
            self.encoded(&serde_json::to_string(&(id, name, i64::from(closed))).unwrap());
        }

        pub(crate) fn finish(self) -> String {
            format!("tree4:{:x}", self.0.finalize())
        }
    }

    #[cfg(test)]
    pub(crate) fn digest<'a>(
        labels: impl IntoIterator<Item = (&'a str, &'a str)>,
        models: impl IntoIterator<Item = &'a str>,
        tools: impl IntoIterator<Item = (&'a str, Option<&'a str>, bool)>,
    ) -> String {
        let mut writer = Writer::new();
        writer.labels();
        for (key, value) in labels {
            writer.label(key, value);
        }
        writer.models();
        let mut models: Vec<_> = models.into_iter().collect();
        models.sort_by_key(|key| address("tree.model", key.as_bytes()));
        for model in models {
            writer.model(model);
        }
        writer.tools();
        let mut tools: Vec<_> = tools.into_iter().collect();
        tools.sort_by_key(|(id, _, _)| address("tree.tool", id.as_bytes()));
        for (id, name, closed) in tools {
            writer.tool(id, name, closed);
        }
        writer.finish()
    }
}

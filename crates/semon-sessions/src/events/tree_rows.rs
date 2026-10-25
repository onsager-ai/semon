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
// bytea); the recipe owns row encoding and digest framing. The domain tags
// remain the original witness's fixed bytes, not backend query SQL.
pub(crate) mod witness {
    use sha2::{Digest, Sha256};

    pub(crate) fn digest<'a>(
        labels: impl IntoIterator<Item = (&'a str, &'a str)>,
        models: impl IntoIterator<Item = &'a str>,
        tools: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> serde_json::Result<String> {
        let mut digest = Sha256::new();
        let mut family = |domain: &str, rows: Vec<String>| {
            digest.update(domain.as_bytes());
            for row in rows {
                digest.update((row.len() as u64).to_be_bytes());
                digest.update(row.as_bytes());
            }
        };
        family(
            "SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label",
            labels
                .into_iter()
                .map(|(key, value)| serde_json::to_string(&[key, value]))
                .collect::<serde_json::Result<_>>()?,
        );
        family(
            "SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY model",
            models
                .into_iter()
                .map(|model| serde_json::to_string(&[model]))
                .collect::<serde_json::Result<_>>()?,
        );
        family(
            "SELECT json_array(tool_id,name) FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id",
            tools
                .into_iter()
                .map(|(id, name)| serde_json::to_string(&[id, name]))
                .collect::<serde_json::Result<_>>()?,
        );
        Ok(format!("{:x}", digest.finalize()))
    }

    #[cfg(test)]
    mod tests {
        // The recipe must reproduce the SQLite json_array witness bytes exactly,
        // including control, NUL, escape and non-BMP content.
        #[test]
        fn tree_witness_recipe_matches_sqlite_json_array_bytes() {
            use sha2::{Digest, Sha256};
            let c = rusqlite::Connection::open_in_memory().unwrap();
            c.execute_batch("CREATE TABLE native_tree_labels(projection_id,label,value);CREATE TABLE native_tree_models(projection_id,model);CREATE TABLE native_tree_tools(projection_id,tool_id,name,closed);").unwrap();
            let odd = "a\0b\u{1}\u{1f}\t\n\r\u{8}\u{c}\"\\/\u{7f}\u{2028}é🦀";
            let labels = [("branch", odd), ("cwd", "/w"), ("first", "🦀"), ("last", "")];
            for (label, value) in labels {
                c.execute("INSERT INTO native_tree_labels VALUES(7,?1,?2)", [label, value]).unwrap();
            }
            for model in ["m\0", "m🦀", "Z"] {
                c.execute("INSERT INTO native_tree_models VALUES(7,?1)", [model]).unwrap();
            }
            for (id, name, closed) in [
                ("t\0", Some(odd), 0),
                ("t🦀", Some("n"), 0),
                ("u", Some("closed"), 1),
                ("v", None, 0),
            ] {
                c.execute(
                    "INSERT INTO native_tree_tools VALUES(7,?1,?2,?3)",
                    rusqlite::params![id, name, closed],
                )
                .unwrap();
            }
            let mut oracle = Sha256::new();
            for sql in [
                "SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label",
                "SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY model",
                "SELECT json_array(tool_id,name) FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id",
            ] {
                oracle.update(sql.as_bytes());
                let mut statement = c.prepare(sql).unwrap();
                let mut rows = statement.query([7]).unwrap();
                while let Some(row) = rows.next().unwrap() {
                    let value: String = row.get(0).unwrap();
                    oracle.update((value.len() as u64).to_be_bytes());
                    oracle.update(value.as_bytes());
                }
            }
            let mut models = ["m\0", "m🦀", "Z"];
            models.sort_unstable();
            let actual = super::digest(
                labels,
                models,
                [("t\0", odd), ("t🦀", "n")],
            )
            .unwrap();
            assert_eq!(actual, format!("{:x}", oracle.finalize()));
        }
    }
}

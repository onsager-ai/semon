//! Producer-owned session metadata. This reader never changes the input file.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
};

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

const MAX_LINE: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub system: String,
    pub repository: String,
    pub ticket: String,
    pub issue: Option<u64>,
    pub pr: Option<u64>,
    pub title: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub path: PathBuf,
    pub valid: u64,
    pub rejected: u64,
}

#[derive(Debug)]
pub struct Annotations {
    pub status: Status,
    records: BTreeMap<(String, String), (DateTime<FixedOffset>, Annotation)>,
}

// Required nullable fields use Value: serde's Option would accept a missing field.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    v: u64,
    harness: String,
    native_id: String,
    system: String,
    repository: String,
    ticket: String,
    issue: serde_json::Value,
    pr: serde_json::Value,
    title: serde_json::Value,
    updated_at: String,
}

impl Record {
    fn validate(self) -> Option<(String, String, DateTime<FixedOffset>, Annotation)> {
        fn number(value: serde_json::Value) -> Option<Option<u64>> {
            if value.is_null() {
                Some(None)
            } else {
                value.as_u64().filter(|n| *n > 0).map(Some)
            }
        }
        if self.v != 1
            || !matches!(self.harness.as_str(), "claude" | "codex")
            || self.native_id.is_empty()
            || self.system.is_empty()
            || self.ticket.is_empty()
        {
            return None;
        }
        let (owner, name) = self.repository.split_once('/')?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        let title = match self.title {
            serde_json::Value::Null => None,
            serde_json::Value::String(title) if title.chars().count() <= 200 => Some(title),
            _ => return None,
        };
        let updated_at = DateTime::parse_from_rfc3339(&self.updated_at).ok()?;
        Some((
            self.harness,
            self.native_id,
            updated_at,
            Annotation {
                system: self.system,
                repository: self.repository,
                ticket: self.ticket,
                issue: number(self.issue)?,
                pr: number(self.pr)?,
                title,
                source: "annotation".into(),
            },
        ))
    }
}

pub fn default_path(home: &Path, xdg_state: Option<&std::ffi::OsStr>) -> PathBuf {
    xdg_state
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"))
        .join("ostrom/semon-annotations.jsonl")
}

impl Annotations {
    /// Missing files have zero records. Other I/O errors remain observable.
    pub fn read(path: &Path) -> io::Result<Self> {
        match File::open(path) {
            Ok(file) => Self::from_reader(path, BufReader::new(file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Self::from_reader(path, io::empty())
            }
            Err(error) => Err(error),
        }
    }

    /// Add presentation metadata to exact native keys, without changing lineage.
    pub fn apply(&self, nodes: &mut [crate::Node]) {
        for node in nodes {
            if let Some(annotation) = self.get(&node.harness, &node.id) {
                if node.label.as_ref().is_none_or(|label| label.is_empty()) {
                    node.label.clone_from(&annotation.title);
                }
                node.annotation = Some(annotation.clone());
            }
            self.apply(&mut node.children);
        }
    }

    pub fn get(&self, harness: &str, native_id: &str) -> Option<&Annotation> {
        self.records
            .get(&(harness.to_owned(), native_id.to_owned()))
            .map(|(_, annotation)| annotation)
    }

    fn from_reader(path: &Path, mut reader: impl BufRead) -> io::Result<Self> {
        let mut result = Self {
            status: Status {
                path: path.into(),
                valid: 0,
                rejected: 0,
            },
            records: BTreeMap::new(),
        };
        let mut line = Vec::with_capacity(MAX_LINE);
        let mut oversized = false;
        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                // No newline: the producer has not completed this record yet.
                break;
            }
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let length = newline.unwrap_or(buffer.len());
            if !oversized {
                if line.len() + length > MAX_LINE {
                    oversized = true;
                    line.clear();
                } else {
                    line.extend_from_slice(&buffer[..length]);
                }
            }
            reader.consume(length + usize::from(newline.is_some()));
            if newline.is_none() {
                continue;
            }
            let record = (!oversized)
                .then(|| serde_json::from_slice::<Record>(&line).ok())
                .flatten()
                .and_then(Record::validate);
            if let Some((harness, native_id, updated_at, annotation)) = record {
                result.status.valid += 1;
                let entry = result.records.entry((harness, native_id));
                match entry {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert((updated_at, annotation));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry)
                        if updated_at >= entry.get().0 =>
                    {
                        entry.insert((updated_at, annotation));
                    }
                    _ => {}
                }
            } else {
                result.status.rejected += 1;
            }
            line.clear();
            oversized = false;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn record() -> Value {
        json!({"v":1,"harness":"codex","native_id":"native","system":"ostrom",
            "repository":"owner/repo","ticket":"9","issue":null,"pr":364,
            "title":"Fix route ranking","updated_at":"2026-11-19T04:52:01Z"})
    }
    fn read(input: &[u8]) -> Annotations {
        Annotations::from_reader(Path::new("fixture.jsonl"), input).unwrap()
    }
    #[test]
    fn rejects_each_missing_and_mistyped_field_and_unknown_fields() {
        for key in record().as_object().unwrap().keys() {
            let mut missing = record();
            missing.as_object_mut().unwrap().remove(key);
            assert_eq!(
                read(format!("{missing}\n").as_bytes()).status.rejected,
                1,
                "{key}"
            );
            let mut mistyped = record();
            mistyped[key] = json!([]);
            assert_eq!(
                read(format!("{mistyped}\n").as_bytes()).status.rejected,
                1,
                "{key}"
            );
        }
        let mut unknown = record();
        unknown["extra"] = json!(true);
        assert_eq!(read(format!("{unknown}\n").as_bytes()).status.rejected, 1);
    }
    #[test]
    fn rejects_invalid_values() {
        for (key, value) in [
            ("v", json!(2)),
            ("v", json!(1.0)),
            ("harness", json!("copilot")),
            ("native_id", json!("")),
            ("system", json!("")),
            ("ticket", json!("")),
            ("repository", json!("repo")),
            ("repository", json!("/repo")),
            ("repository", json!("owner/")),
            ("repository", json!("a/b/c")),
            ("issue", json!(0)),
            ("pr", json!(-1)),
            ("issue", json!(1.5)),
            ("title", json!("x".repeat(201))),
            ("updated_at", json!("2026-11-19T04:52:01")),
            ("updated_at", json!("2026-02-30T04:52:01Z")),
        ] {
            let mut invalid = record();
            invalid[key] = value;
            assert_eq!(
                read(format!("{invalid}\n").as_bytes()).status.rejected,
                1,
                "{key}"
            );
        }
        assert_eq!(read(b"not json\n\xff\n\n").status.rejected, 3);
    }
    #[test]
    fn latest_timestamp_wins_and_equal_instants_use_later_line() {
        let mut later = record();
        later["title"] = json!("latest");
        later["updated_at"] = json!("2026-11-19T05:52:01.123+01:00");
        let mut tie = later.clone();
        tie["title"] = json!("tie");
        tie["updated_at"] = json!("2026-11-19T04:52:01.123Z");
        let records = read(format!("{later}\n{}\n{tie}\n", record()).as_bytes());
        assert_eq!(records.status.valid, 3);
        assert_eq!(
            records.get("codex", "native").unwrap().title.as_deref(),
            Some("tie")
        );
        assert!(records.get("claude", "native").is_none());
    }
    #[test]
    fn partial_lines_are_ignored_and_oversize_lines_are_drained() {
        let valid = record();
        let input = format!("{}\n{valid}\n{valid}", "x".repeat(MAX_LINE + 1));
        let records = read(input.as_bytes());
        assert_eq!((records.status.valid, records.status.rejected), (1, 1));
        assert_eq!(read(&vec![b'x'; MAX_LINE * 3]).status.rejected, 0);
        let mut boundary = format!("{valid}");
        boundary.push_str(&" ".repeat(MAX_LINE - boundary.len()));
        assert_eq!(read(format!("{boundary}\n").as_bytes()).status.valid, 1);
    }
    #[test]
    fn nullable_values_unicode_titles_and_native_keys_are_preserved() {
        let mut value = record();
        value["title"] = json!("界".repeat(200));
        value["issue"] = json!(42);
        value["pr"] = Value::Null;
        let records = read(format!("{value}\n").as_bytes());
        let annotation = records.get("codex", "native").unwrap();
        assert_eq!(annotation.issue, Some(42));
        assert_eq!(annotation.pr, None);
        value["title"] = Value::Null;
        value["harness"] = json!("claude");
        assert_eq!(read(format!("{value}\n").as_bytes()).status.valid, 1);
    }
    #[test]
    fn metadata_preserves_lineage_state_and_native_names() {
        let records = read(format!("{}\n", record()).as_bytes());
        let mut root = crate::Node::new("native".into(), "codex", "codex-run");
        root.label = Some("Native name".into());
        root.state = "running".into();
        root.parent_source = Some("native-parent".into());
        root.children
            .push(crate::Node::new("child".into(), "codex", "subagent"));
        let mut nodes = vec![root];
        let before = serde_json::to_value(&nodes).unwrap();
        records.apply(&mut nodes);
        assert_eq!(nodes[0].label.as_deref(), Some("Native name"));
        assert!(nodes[0].children[0].annotation.is_none());
        let mut after = serde_json::to_value(&nodes).unwrap();
        after[0].as_object_mut().unwrap().remove("annotation");
        assert_eq!(before, after);
    }

    #[test]
    fn default_requires_absolute_xdg_state() {
        let home = Path::new("/home/test");
        for state in [None, Some(std::ffi::OsStr::new("relative"))] {
            assert_eq!(
                default_path(home, state),
                home.join(".local/state/ostrom/semon-annotations.jsonl")
            );
        }
        assert_eq!(
            default_path(home, Some(std::ffi::OsStr::new("/state"))),
            Path::new("/state/ostrom/semon-annotations.jsonl")
        );
    }
}

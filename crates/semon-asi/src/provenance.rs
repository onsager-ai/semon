//! Local custody records. This artifact is deliberately absent from `schema::TABLES`.
//! Its JSON encoding is a local persistence format, never a governed egress format.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use arrow::{array::StringArray, record_batch::RecordBatch};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    input::{Derivation, RecordedVersions, SourceWitness},
    parquet_io::{int_column, read_batches, string_column},
    schema::{Class, Column, Kind, Table, Type},
    store::{Manifest, read_live_map},
};

/// Private storage declaration, shared only by publication and compaction.
pub(crate) static TABLE: Table = Table {
    name: "provenance",
    grain: "One local admitted session generation",
    kind: Kind::Session,
    sort_time: None,
    columns: &[
        Column::new(
            "session_uid",
            Type::Text,
            false,
            Class::Key,
            "Local session identity",
        ),
        Column::new(
            "record",
            Type::Text,
            false,
            Class::Body,
            "Local custody record; never queryable or exported as analytics",
        ),
    ],
};

/// Complete provenance of an admitted generation, without consulting today's Catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source_witness: SourceWitness,
    pub derivation: Derivation,
    pub versions: RecordedVersions,
    /// Time the analytics producer projected this admitted generation (UTC milliseconds).
    pub projected_at: i64,
}

pub(crate) fn batch(records: &BTreeMap<String, Provenance>) -> Result<RecordBatch> {
    let encoded: Vec<String> = records
        .values()
        .map(serde_json::to_string)
        .collect::<std::result::Result<_, _>>()?;
    Ok(RecordBatch::try_new(
        Arc::new(TABLE.arrow_schema()),
        vec![
            Arc::new(StringArray::from_iter_values(
                records.keys().map(String::as_str),
            )),
            Arc::new(StringArray::from(encoded)),
        ],
    )?)
}

/// Read and validate the one-to-one binding to a pinned manifest's live map.
/// This local API is not registered with the SQL engine or any egress serializer.
pub fn read(root: &Path, manifest: &Manifest) -> Result<BTreeMap<String, (i64, Provenance)>> {
    let live = read_live_map(root, manifest)?;
    let mut records = BTreeMap::new();
    for file in manifest.files_of(TABLE.name) {
        for batch in read_batches(&root.join("analytics").join(&file.path), None)? {
            let sessions = string_column(&batch, "session_uid")?;
            let generations = int_column(&batch, "_gen")?;
            let values = string_column(&batch, "record")?;
            for row in 0..batch.num_rows() {
                let uid = sessions.value(row);
                let generation = generations.value(row);
                if live.get(uid) == Some(&generation) {
                    let record = serde_json::from_str(values.value(row))?;
                    if records
                        .insert(uid.to_owned(), (generation, record))
                        .is_some()
                    {
                        return Err(crate::Error::invalid("duplicate live session provenance"));
                    }
                }
            }
        }
    }
    if records.len() != live.len() {
        return Err(crate::Error::invalid(
            "live session lacks generation-bound provenance",
        ));
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CompactionPolicy, Store, Update,
        store::{Fault, RETIREMENT_DELAY_MS},
        testutil::{TestClock, update},
    };

    const A: &str = "origin/claude/a";
    const B: &str = "origin/claude/b";

    #[test]
    fn generations_faults_compaction_retirement_and_restart_preserve_binding() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let clock = TestClock::new(0);
        let mut store = Store::open_with_clock(root, clock.clone()).unwrap();
        let first = update(&[(A, 1), (B, 1)], "first", 0);
        store.publish(&first).unwrap();
        let original = read(root, store.manifest()).unwrap();
        assert_eq!(original.len(), 2);
        let mut replacement = update(&[(A, 2)], "replacement", 0);
        replacement
            .provenance
            .get_mut(A)
            .unwrap()
            .source_witness
            .generation = "replacement".into();
        for (fault, expected) in [(Fault::BeforeRename, 1), (Fault::AfterRename, 2)] {
            store.fault = Some(fault);
            assert!(store.publish(&replacement).is_err());
            let manifest = crate::read_manifest(root).unwrap().unwrap();
            let records = read(root, &manifest).unwrap();
            assert_eq!(records[A].0, expected);
            assert_eq!(records[B], original[B]);
            assert_eq!(
                records[A].1.source_witness.generation == "replacement",
                expected == 2
            );
        }
        store.reload_manifest().unwrap();
        let current = read(root, store.manifest()).unwrap();
        store
            .publish(&Update {
                removed_sessions: vec![B.into()],
                ..Update::default()
            })
            .unwrap();
        let policy = CompactionPolicy {
            level0_max_files: 0,
            ..CompactionPolicy::default()
        };
        while store.compact(&policy).unwrap().is_some() {}
        let compacted = read(root, store.manifest()).unwrap();
        assert_eq!(compacted.len(), 1);
        assert_eq!(compacted[A], current[A]);
        for entry in store.manifest().files_of(TABLE.name) {
            assert_eq!(entry.rows, 1);
            assert_eq!(entry.dead, Some(0));
        }
        clock.advance(RETIREMENT_DELAY_MS + 1);
        let retired = store.retire().unwrap();
        assert!(retired.iter().any(|path| path.starts_with("provenance/")));
        assert_eq!(read(root, store.manifest()).unwrap(), compacted);
        drop(store);
        let reopened = Store::open_with_clock(root, clock).unwrap();
        assert_eq!(read(root, reopened.manifest()).unwrap(), compacted);
    }

    #[test]
    fn a_touched_session_requires_exactly_one_typed_record() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open_with_clock(directory.path(), TestClock::new(0)).unwrap();
        let mut invalid = update(&[(A, 1)], "a", 0);
        invalid.provenance.clear();
        assert!(store.publish(&invalid).is_err());
        assert_eq!(store.manifest().generation, 0);
        assert!(!store.analytics_dir().join(TABLE.name).exists());
    }
}

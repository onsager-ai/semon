//! Local custody records. This artifact is deliberately absent from `schema::TABLES`.
//! Its JSON encoding is a local persistence format, never a governed egress format.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

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

/// Validate keys without decoding custody payloads. Historical dead rows are
/// permitted until compaction; only exact live `(session_uid, _gen)` pairs count.
pub(crate) fn validate(
    analytics: &Path,
    manifest: &Manifest,
    live: &HashMap<String, i64>,
) -> Result<()> {
    let checked = (|| {
        let mut seen = HashSet::with_capacity(live.len());
        let mut paths = HashSet::new();
        for file in manifest.files_of(TABLE.name) {
            if !paths.insert(&file.path) {
                return Err(crate::Error::invalid("duplicate provenance file entry"));
            }
            let path = analytics.join(&file.path);
            if std::fs::metadata(&path)?.len() != file.bytes {
                return Err(crate::Error::invalid(
                    "provenance file size differs from manifest",
                ));
            }
            let mut rows = 0u64;
            crate::parquet_io::visit_batches(&path, Some(&["session_uid", "_gen"]), |batch| {
                let sessions = string_column(&batch, "session_uid")?;
                let generations = int_column(&batch, "_gen")?;
                for row in 0..batch.num_rows() {
                    rows += 1;
                    let uid = sessions.value(row);
                    let generation = generations.value(row);
                    if generation < file.min_row_gen as i64 || generation > file.max_row_gen as i64
                    {
                        return Err(crate::Error::invalid(
                            "provenance generation outside manifest range",
                        ));
                    }
                    if live.get(uid) == Some(&generation) && !seen.insert(uid.to_owned()) {
                        return Err(crate::Error::invalid("duplicate live provenance record"));
                    }
                }
                Ok(())
            })?;
            if rows != file.rows {
                return Err(crate::Error::invalid(
                    "provenance row count differs from manifest",
                ));
            }
        }
        if seen.len() != live.len() {
            return Err(crate::Error::invalid(
                "live session lacks generation-bound provenance",
            ));
        }
        Ok(())
    })();
    checked.map_err(|error: crate::Error| {
        crate::Error::invalid(format!(
            "invalid publication provenance: {error}; preserve or export the store before recovery"
        ))
    })
}

/// Read and validate the one-to-one binding to a pinned manifest's live map.
/// This local API is not registered with the SQL engine or any egress serializer.
pub fn read(root: &Path, manifest: &Manifest) -> Result<BTreeMap<String, (i64, Provenance)>> {
    let live = read_live_map(root, manifest)?;
    validate(&root.join("analytics"), manifest, &live)?;
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
    fn open_and_reload_refuse_corrupt_provenance_without_recovery_deletions() {
        for corruption in [
            "missing",
            "entry",
            "duplicate",
            "generation",
            "size",
            "rows",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let mut store = Store::open_with_clock(root, TestClock::new(0)).unwrap();
            store.publish(&update(&[(A, 1)], "a", 0)).unwrap();
            let adopted = store.manifest().clone();
            let mut manifest = adopted.clone();
            let file = manifest
                .files
                .iter_mut()
                .find(|file| file.table == TABLE.name)
                .unwrap();
            let path = root.join("analytics").join(&file.path);
            match corruption {
                "missing" => std::fs::remove_file(&path).unwrap(),
                "entry" => manifest.files.retain(|file| file.table != TABLE.name),
                "duplicate" | "generation" => {
                    let original = read_batches(&path, None).unwrap().remove(0);
                    let changed = if corruption == "duplicate" {
                        arrow::compute::concat_batches(
                            &original.schema(),
                            &[original.clone(), original],
                        )
                        .unwrap()
                    } else {
                        crate::testutil::replace_column(
                            &original,
                            "_gen",
                            Arc::new(arrow::array::Int64Array::from(vec![2])),
                        )
                    };
                    let written = crate::parquet_io::write_file(&path, &changed).unwrap();
                    file.bytes = written.bytes;
                    file.rows = written.rows;
                    if corruption == "generation" {
                        file.max_row_gen = 2;
                    }
                }
                "size" => file.bytes += 1,
                "rows" => file.rows += 1,
                _ => unreachable!(),
            }
            let manifest_path = root.join("analytics/manifest.json");
            let bytes = serde_json::to_vec(&manifest).unwrap();
            std::fs::write(&manifest_path, &bytes).unwrap();
            // Recovery must not delete even a clearly uncommitted leftover on refusal.
            let orphan = root.join("analytics/provenance/g999.parquet");
            std::fs::write(&orphan, "preserve").unwrap();
            let error = store.reload_manifest().unwrap_err().to_string();
            assert!(
                error.contains("preserve or export"),
                "{corruption}: {error}"
            );
            assert_eq!(store.manifest(), &adopted);
            assert_eq!(std::fs::read(&manifest_path).unwrap(), bytes);
            drop(store);
            let error = Store::open_with_clock(root, TestClock::new(0))
                .err()
                .unwrap()
                .to_string();
            assert!(
                error.contains("preserve or export"),
                "{corruption}: {error}"
            );
            assert_eq!(std::fs::read(&manifest_path).unwrap(), bytes);
            assert_eq!(std::fs::read(&orphan).unwrap(), b"preserve");
            assert!(root.join("analytics/fact_session/g1.parquet").exists());
            assert_eq!(path.exists(), corruption != "missing");
        }
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

//! Read-only coverage aggregation over one published analytics snapshot.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use arrow::array::Array;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::parquet_io::{int_column, string_column, visit_batches};
use crate::schema::{FACT_SESSION, GEN_COLUMN, SESSION_COLUMN};
use crate::store::{Manifest, Versions, read_live_map, read_manifest};

const COVERAGE_COLUMNS: &[&str] = &[
    SESSION_COLUMN,
    GEN_COLUMN,
    "transform_version",
    "rules_version",
    "price_version",
    "taxonomy_version",
    "source_status",
    "computation_status",
    "step_count",
    "exclusive_step_count",
    "unknown_step_count",
    "unclassified_step_count",
];

/// Recorded versions and statuses that define one published coverage group.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CoverageKey {
    pub transform_version: i64,
    pub rules_version: i64,
    pub price_version: String,
    pub taxonomy_version: Option<i64>,
    pub source_status: String,
    pub computation_status: String,
}

/// Aggregated live rows for one [`CoverageKey`].
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CoverageTotals {
    pub session_count: u64,
    pub step_count: u64,
    pub exclusive_step_count: u64,
    pub unknown_step_count: u64,
    pub unclassified_step_count: u64,
}

/// A coverage group and its measures.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CoverageGroup {
    #[serde(flatten)]
    pub key: CoverageKey,
    #[serde(flatten)]
    pub totals: CoverageTotals,
}

/// One immutable published view of session coverage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CoverageReport {
    pub scope: &'static str,
    pub derivation_state: &'static str,
    pub manifest_generation: u64,
    pub writer_targets: Versions,
    pub source_missing_uids: u64,
    pub groups: Vec<CoverageGroup>,
}

/// Reads one manifest, its live map and only its named `fact_session` files.
///
/// `None` means no manifest has been published at `root`. This never opens the
/// writer store or its lock and does not create files or directories.
pub fn read(root: &Path) -> Result<Option<CoverageReport>> {
    let Some(manifest) = read_manifest(root)? else {
        return Ok(None);
    };
    validate_live_map(root, &manifest)?;
    let live = read_live_map(root, &manifest)?;
    validate_source_missing(&manifest, &live)?;
    let mut groups = BTreeMap::new();
    for entry in manifest.files_of(FACT_SESSION.name) {
        let path = published_file(root, &entry.path, FACT_SESSION.name)?;
        visit_batches(&path, Some(COVERAGE_COLUMNS), |batch| {
            aggregate_batch(&batch, &live, &mut groups)
        })?;
    }
    Ok(Some(CoverageReport {
        scope: "PUBLISHED live-row coverage",
        derivation_state: "native pending/failed/frozen derivation state is not part of this report",
        manifest_generation: manifest.generation,
        writer_targets: manifest.versions,
        source_missing_uids: u64::try_from(manifest.source_missing.len())
            .map_err(|_| Error::invalid("source-missing UID count does not fit in u64"))?,
        groups: groups
            .into_iter()
            .map(|(key, totals)| CoverageGroup { key, totals })
            .collect(),
    }))
}

fn validate_live_map(root: &Path, manifest: &Manifest) -> Result<()> {
    match &manifest.live_map {
        Some(path) => {
            published_file(root, path, "live")?;
        }
        None if manifest.generation > 0
            || manifest
                .files
                .iter()
                .any(|file| file.table.starts_with("fact_")) =>
        {
            return Err(Error::invalid(
                "published manifest has facts or a generation but no live-map reference",
            ));
        }
        None => {}
    }
    Ok(())
}

fn validate_source_missing(manifest: &Manifest, live: &HashMap<String, i64>) -> Result<()> {
    if let Some(uid) = manifest
        .source_missing
        .keys()
        .find(|uid| !live.contains_key(uid.as_str()))
    {
        return Err(Error::invalid(format!(
            "source-missing UID {uid} is not live in the published live map"
        )));
    }
    Ok(())
}

fn published_file(root: &Path, relative: &str, table: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    let mut components = relative.components();
    let parent = components.next();
    let file = components.next();
    if !matches!(parent, Some(Component::Normal(parent)) if parent.to_str() == Some(table))
        || !matches!(file, Some(Component::Normal(_)))
        || components.next().is_some()
        || relative
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("parquet")
    {
        return Err(Error::invalid(format!(
            "published {table} path is invalid: {relative}",
            relative = relative.display()
        )));
    }
    let analytics = root.join("analytics").canonicalize()?;
    let path = analytics.join(relative).canonicalize()?;
    if !path.starts_with(&analytics) {
        return Err(Error::invalid(format!(
            "published {table} path escapes the analytics directory"
        )));
    }
    Ok(path)
}

fn aggregate_batch(
    batch: &arrow::record_batch::RecordBatch,
    live: &HashMap<String, i64>,
    groups: &mut BTreeMap<CoverageKey, CoverageTotals>,
) -> Result<()> {
    let session_uids = string_column(batch, SESSION_COLUMN)?;
    let generations = int_column(batch, GEN_COLUMN)?;
    let transforms = int_column(batch, "transform_version")?;
    let rules = int_column(batch, "rules_version")?;
    let prices = string_column(batch, "price_version")?;
    let taxonomies = int_column(batch, "taxonomy_version")?;
    let sources = string_column(batch, "source_status")?;
    let computations = string_column(batch, "computation_status")?;
    let steps = int_column(batch, "step_count")?;
    let exclusive_steps = int_column(batch, "exclusive_step_count")?;
    let unknown_steps = int_column(batch, "unknown_step_count")?;
    let unclassified_steps = int_column(batch, "unclassified_step_count")?;

    for row in 0..batch.num_rows() {
        if live.get(session_uids.value(row)) != Some(&generations.value(row)) {
            continue;
        }
        let key = CoverageKey {
            transform_version: transforms.value(row),
            rules_version: rules.value(row),
            price_version: prices.value(row).to_owned(),
            taxonomy_version: (!taxonomies.is_null(row)).then(|| taxonomies.value(row)),
            source_status: sources.value(row).to_owned(),
            computation_status: computations.value(row).to_owned(),
        };
        let totals = groups.entry(key).or_default();
        totals.session_count = add(totals.session_count, 1, "session_count")?;
        totals.step_count = add_value(totals.step_count, steps.value(row), "step_count")?;
        totals.exclusive_step_count = add_value(
            totals.exclusive_step_count,
            exclusive_steps.value(row),
            "exclusive_step_count",
        )?;
        totals.unknown_step_count = add_value(
            totals.unknown_step_count,
            unknown_steps.value(row),
            "unknown_step_count",
        )?;
        totals.unclassified_step_count = add_value(
            totals.unclassified_step_count,
            unclassified_steps.value(row),
            "unclassified_step_count",
        )?;
    }
    Ok(())
}

fn add_value(total: u64, value: i64, column: &str) -> Result<u64> {
    let value = u64::try_from(value)
        .map_err(|_| Error::invalid(format!("fact_session.{column} is negative")))?;
    add(total, value, column)
}

fn add(total: u64, value: u64, column: &str) -> Result<u64> {
    total
        .checked_add(value)
        .ok_or_else(|| Error::invalid(format!("fact_session.{column} overflowed coverage")))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use arrow::array::{ArrayRef, Int64Array, StringArray};

    use super::*;
    use crate::schema::FACT_SESSION;
    use crate::store::{SourceMissing, Store, TableBatch, Update, Versions};
    use crate::testutil::{TestClock, replace_column, sample};

    struct Row<'a> {
        session_uid: &'a str,
        transform: i64,
        rules: i64,
        price: &'a str,
        taxonomy: Option<i64>,
        source: &'a str,
        computation: &'a str,
        steps: i64,
        exclusive_steps: i64,
        unknown_steps: i64,
        unclassified_steps: i64,
    }

    fn sessions(rows: &[Row<'_>]) -> arrow::record_batch::RecordBatch {
        let sessions: Vec<(&str, usize)> = rows.iter().map(|row| (row.session_uid, 1)).collect();
        let mut batch = sample(&FACT_SESSION, &sessions, "coverage", 0);
        let text = |values: Vec<String>| -> ArrayRef { Arc::new(StringArray::from(values)) };
        let integer = |values: Vec<Option<i64>>| -> ArrayRef { Arc::new(Int64Array::from(values)) };
        batch = replace_column(
            &batch,
            "transform_version",
            integer(rows.iter().map(|row| Some(row.transform)).collect()),
        );
        batch = replace_column(
            &batch,
            "rules_version",
            integer(rows.iter().map(|row| Some(row.rules)).collect()),
        );
        batch = replace_column(
            &batch,
            "price_version",
            text(rows.iter().map(|row| row.price.to_owned()).collect()),
        );
        batch = replace_column(
            &batch,
            "taxonomy_version",
            integer(rows.iter().map(|row| row.taxonomy).collect()),
        );
        batch = replace_column(
            &batch,
            "source_status",
            text(rows.iter().map(|row| row.source.to_owned()).collect()),
        );
        batch = replace_column(
            &batch,
            "computation_status",
            text(rows.iter().map(|row| row.computation.to_owned()).collect()),
        );
        for (column, values) in [
            (
                "step_count",
                rows.iter().map(|row| row.steps).collect::<Vec<_>>(),
            ),
            (
                "exclusive_step_count",
                rows.iter()
                    .map(|row| row.exclusive_steps)
                    .collect::<Vec<_>>(),
            ),
            (
                "unknown_step_count",
                rows.iter().map(|row| row.unknown_steps).collect::<Vec<_>>(),
            ),
            (
                "unclassified_step_count",
                rows.iter()
                    .map(|row| row.unclassified_steps)
                    .collect::<Vec<_>>(),
            ),
        ] {
            batch = replace_column(
                &batch,
                column,
                integer(values.into_iter().map(Some).collect()),
            );
        }
        batch
    }

    fn update(rows: &[Row<'_>], targets: Versions) -> Update {
        Update {
            tables: vec![TableBatch {
                table: &FACT_SESSION,
                batch: sessions(rows),
            }],
            provenance: crate::testutil::provenance(rows.iter().map(|row| row.session_uid)),
            touched_sessions: rows.iter().map(|row| row.session_uid.to_owned()).collect(),
            versions: targets,
            ..Update::default()
        }
    }

    #[test]
    fn reports_live_rows_by_recorded_versions_and_status() {
        let directory = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = Store::open_with_clock(directory.path(), clock).unwrap();
        store
            .publish(&update(
                &[
                    Row {
                        session_uid: "origin/a",
                        transform: 1,
                        rules: 2,
                        price: "price-old",
                        taxonomy: Some(3),
                        source: "complete",
                        computation: "current",
                        steps: 4,
                        exclusive_steps: 3,
                        unknown_steps: 1,
                        unclassified_steps: 2,
                    },
                    Row {
                        session_uid: "origin/b",
                        transform: 4,
                        rules: 5,
                        price: "price-retained",
                        taxonomy: None,
                        source: "missing",
                        computation: "frozen",
                        steps: 6,
                        exclusive_steps: 5,
                        unknown_steps: 2,
                        unclassified_steps: 1,
                    },
                ],
                Versions {
                    transform: 1,
                    rules: 2,
                    price: "price-old".into(),
                },
            ))
            .unwrap();
        let mut replacement = update(
            &[Row {
                session_uid: "origin/a",
                transform: 7,
                rules: 8,
                price: "price-new",
                taxonomy: Some(9),
                source: "complete",
                computation: "pending",
                steps: 10,
                exclusive_steps: 9,
                unknown_steps: 4,
                unclassified_steps: 3,
            }],
            Versions {
                transform: 10,
                rules: 11,
                price: "price-target".into(),
            },
        );
        replacement.source_missing =
            BTreeMap::from([("origin/b".into(), SourceMissing { detected_at: 12 })]);
        store.publish(&replacement).unwrap();

        let report = read(directory.path()).unwrap().unwrap();
        assert_eq!(report.manifest_generation, 2);
        assert_eq!(report.writer_targets, replacement.versions);
        assert_eq!(report.source_missing_uids, 1);
        assert_eq!(
            report.groups,
            vec![
                CoverageGroup {
                    key: CoverageKey {
                        transform_version: 4,
                        rules_version: 5,
                        price_version: "price-retained".into(),
                        taxonomy_version: None,
                        source_status: "missing".into(),
                        computation_status: "frozen".into(),
                    },
                    totals: CoverageTotals {
                        session_count: 1,
                        step_count: 6,
                        exclusive_step_count: 5,
                        unknown_step_count: 2,
                        unclassified_step_count: 1,
                    },
                },
                CoverageGroup {
                    key: CoverageKey {
                        transform_version: 7,
                        rules_version: 8,
                        price_version: "price-new".into(),
                        taxonomy_version: Some(9),
                        source_status: "complete".into(),
                        computation_status: "pending".into(),
                    },
                    totals: CoverageTotals {
                        session_count: 1,
                        step_count: 10,
                        exclusive_step_count: 9,
                        unknown_step_count: 4,
                        unclassified_step_count: 3,
                    },
                },
            ]
        );
    }

    #[test]
    fn accepts_an_empty_manifest_without_a_live_map() {
        let directory = tempfile::tempdir().unwrap();
        let analytics = directory.path().join("analytics");
        std::fs::create_dir(&analytics).unwrap();
        std::fs::write(
            analytics.join("manifest.json"),
            serde_json::to_vec(&Manifest::empty()).unwrap(),
        )
        .unwrap();

        let report = read(directory.path()).unwrap().unwrap();
        assert_eq!(report.manifest_generation, 0);
        assert!(report.groups.is_empty());
    }

    #[test]
    fn refuses_published_facts_without_a_live_map() {
        let directory = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = Store::open_with_clock(directory.path(), clock).unwrap();
        store
            .publish(&update(
                &[Row {
                    session_uid: "origin/a",
                    transform: 1,
                    rules: 1,
                    price: "price",
                    taxonomy: None,
                    source: "complete",
                    computation: "current",
                    steps: 1,
                    exclusive_steps: 1,
                    unknown_steps: 0,
                    unclassified_steps: 0,
                }],
                Versions::default(),
            ))
            .unwrap();
        let mut manifest = read_manifest(directory.path()).unwrap().unwrap();
        manifest.live_map = None;
        std::fs::write(
            directory.path().join("analytics/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = read(directory.path()).unwrap_err();
        assert!(error.to_string().contains("no live-map reference"));
    }

    #[test]
    fn refuses_source_missing_uids_that_are_not_live() {
        let directory = tempfile::tempdir().unwrap();
        let clock = TestClock::new(0);
        let mut store = Store::open_with_clock(directory.path(), clock).unwrap();
        store
            .publish(&update(
                &[Row {
                    session_uid: "origin/a",
                    transform: 1,
                    rules: 1,
                    price: "price",
                    taxonomy: None,
                    source: "complete",
                    computation: "current",
                    steps: 1,
                    exclusive_steps: 1,
                    unknown_steps: 0,
                    unclassified_steps: 0,
                }],
                Versions::default(),
            ))
            .unwrap();
        let mut manifest = read_manifest(directory.path()).unwrap().unwrap();
        manifest
            .source_missing
            .insert("origin/not-live".into(), SourceMissing { detected_at: 12 });
        std::fs::write(
            directory.path().join("analytics/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = read(directory.path()).unwrap_err();
        assert!(error.to_string().contains("origin/not-live is not live"));
    }
}

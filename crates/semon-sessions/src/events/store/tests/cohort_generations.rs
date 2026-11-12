use super::*;
use crate::analytics::Binding;
use crate::origin_projection::{CohortGenerations, CohortReadError, read_cohort_generations};
use semon_asi::{egress::AdmittedCohort, input::SourceWitness};
use std::collections::BTreeMap;

struct Fixture {
    root: PathBuf,
    options: crate::Options,
    path: PathBuf,
    binding: Binding,
    cohorts: Vec<AdmittedCohort>,
    store: SqliteStore,
}
impl Fixture {
    fn new() -> Self {
        let root = scratch("cohort-generations");
        let (options, rows, slots) = catalog_native_fixture(&root, 4, false);
        let path = super::super::super::EventCache::path(&options.cache);
        let (mut store, _) = opened(&path);
        catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
        let identity = crate::analytics_namespace::create(path.parent().unwrap()).unwrap();
        let binding = identity.binding(analytics_index_incarnation(&path).unwrap());
        let mut input = SqliteAnalyticsInput::open(&path, &binding.namespace).unwrap();
        let batch = input.read_admitted(None, InputBudget::default()).unwrap();
        let cohorts = batch
            .changes
            .into_iter()
            .filter_map(|change| {
                let SessionChange::Admitted(session) = change else {
                    return None;
                };
                Some(AdmittedCohort {
                    session_uid: session.session_uid.clone(),
                    source_ids: session
                        .source_witness
                        .sources
                        .iter()
                        .map(|s| s.native_id.clone())
                        .collect(),
                    witnesses: BTreeMap::from([(
                        session.session_uid.clone(),
                        session.source_witness,
                    )]),
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(cohorts.len(), 4);
        // Markers are index evidence; archive byte checks are separately tested
        // by archive retention. Use real archived chains for their identities.
        let archive = crate::archive::Archive::new(root.join("chain-fixture"));
        for row in &rows {
            for source in &row.sources {
                let bytes = fs::read(&source.path).unwrap();
                let chain = archive
                    .preserve(
                        &source.native_id,
                        &mut bytes.as_slice(),
                        crate::archive::NATIVE_REDACTOR,
                    )
                    .unwrap()
                    .unwrap();
                assert!(chain.length >= source.offset);
                let input = crate::native_evidence::source_input(&source.path, &options).unwrap();
                let marker = serde_json::json!({
                    "version": 7, "chain_id": chain.chain_id, "parser_version": CACHE_VERSION,
                    "source": {"root": input.root.as_str(), "path": input.path, "native_id": source.native_id,
                        "offset": source.offset, "prefix_sha256": source.prefix_sha256, "tail_sha256": source.tail_sha256},
                    "stat": {"dev": source.dev, "ino": source.ino, "size": chain.length,
                        "modified_ns": source.modified_ns, "changed_ns": source.changed_ns},
                    "projection_version": crate::slot_projection::VERSION, "projection_generation": "fixture"
                });
                store
                    .connection
                    .execute(
                        "INSERT OR REPLACE INTO meta VALUES(?1,?2)",
                        params![
                            archive_index_key(source.path.to_str().unwrap()),
                            marker.to_string()
                        ],
                    )
                    .unwrap();
            }
        }
        Self {
            root,
            options,
            path,
            binding,
            cohorts,
            store,
        }
    }
    fn read(&self, cohorts: &[AdmittedCohort]) -> Result<CohortGenerations, CohortReadError> {
        read_cohort_generations(&self.path, &self.binding, cohorts)
    }
    fn saved_markers(&self) -> BTreeMap<String, String> {
        self.store
            .connection
            .prepare("SELECT key,value FROM meta WHERE key GLOB 'archive_event_index:*'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    fn repin(&mut self) {
        let mut input = SqliteAnalyticsInput::open(&self.path, &self.binding.namespace).unwrap();
        self.cohorts = input
            .read_admitted(None, InputBudget::default())
            .unwrap()
            .changes
            .into_iter()
            .filter_map(|change| {
                let SessionChange::Admitted(session) = change else {
                    return None;
                };
                Some(AdmittedCohort {
                    session_uid: session.session_uid.clone(),
                    source_ids: session
                        .source_witness
                        .sources
                        .iter()
                        .map(|s| s.native_id.clone())
                        .collect(),
                    witnesses: BTreeMap::from([(
                        session.session_uid.clone(),
                        session.source_witness,
                    )]),
                })
            })
            .collect();
    }
    fn refresh_markers(
        &self,
        rows: &[crate::model::summary::CatalogRow],
        saved: &BTreeMap<String, String>,
    ) {
        for source in rows.iter().flat_map(|row| &row.sources) {
            let key = archive_index_key(source.path.to_str().unwrap());
            let mut marker: serde_json::Value = serde_json::from_str(&saved[&key]).unwrap();
            marker["source"]["offset"] = source.offset.into();
            marker["source"]["prefix_sha256"] = serde_json::to_value(source.prefix_sha256).unwrap();
            marker["stat"]["size"] = source.size.into();
            self.store
                .connection
                .execute(
                    "INSERT OR REPLACE INTO meta VALUES(?1,?2)",
                    params![key, marker.to_string()],
                )
                .unwrap();
        }
    }
    fn first_source(&self) -> &semon_asi::input::Source {
        &self.cohorts[0].witnesses.values().next().unwrap().sources[0]
    }
    fn mutate_marker(&self, mutate: impl FnOnce(&mut serde_json::Value)) {
        let key = archive_index_key(&self.first_source().local_path);
        let text: String = self
            .store
            .connection
            .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r.get(0))
            .unwrap();
        let mut value = serde_json::from_str(&text).unwrap();
        mutate(&mut value);
        self.store
            .connection
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                params![key, value.to_string()],
            )
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn aggregates_exact_membership_and_stable_chain_evidence_without_database_changes() {
    let f = Fixture::new();
    f.store
        .connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let before = fs::read(&f.path).unwrap();
    let modified = fs::metadata(&f.path).unwrap().modified().unwrap();
    let singleton = f.read(&f.cohorts[..1]).unwrap();
    let all = f.read(&f.cohorts).unwrap();
    assert_ne!(singleton, all);
    assert_ne!(
        singleton.activity_generation,
        singleton.transcript_generation
    );
    let combined = AdmittedCohort {
        session_uid: f.cohorts[0].session_uid.clone(),
        source_ids: f
            .cohorts
            .iter()
            .flat_map(|c| c.source_ids.clone())
            .collect(),
        witnesses: f.cohorts.iter().flat_map(|c| c.witnesses.clone()).collect(),
    };
    assert_eq!(all, f.read(&[combined]).unwrap());
    let mut reversed = f.cohorts.clone();
    reversed.reverse();
    assert_eq!(all, f.read(&reversed).unwrap());
    reversed.extend(f.cohorts.clone());
    assert_eq!(all, f.read(&reversed).unwrap());
    for generation in [&all.activity_generation, &all.transcript_generation] {
        assert_eq!(generation.len(), 64);
        assert!(generation.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert_eq!(
        all.chains.keys().cloned().collect::<BTreeSet<_>>(),
        f.cohorts
            .iter()
            .flat_map(|c| c.source_ids.clone())
            .collect()
    );
    let source = f.first_source();
    let marker = archived_index(&f.store.connection, &source.local_path)
        .unwrap()
        .unwrap();
    assert_eq!(
        all.chains[&source.native_id].chain_id,
        marker.chain_id.unwrap()
    );
    assert_eq!(before, fs::read(&f.path).unwrap());
    assert_eq!(modified, fs::metadata(&f.path).unwrap().modified().unwrap());
    assert_eq!(
        fs::metadata(sidecar(&f.path, "-wal"))
            .map(|m| m.len())
            .unwrap_or(0),
        0
    );

    // A publication for an unrelated owner does not affect a singleton.
    let key: String = f
        .store
        .connection
        .query_row(
            "SELECT session_key FROM session_catalog_sources WHERE native_id=?1",
            [f.cohorts[1].source_ids.iter().next().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    f.store
        .connection
        .execute(
            "UPDATE session_activity_projections SET generation='unrelated' WHERE session_key=?1",
            [&key],
        )
        .unwrap();
    assert_eq!(singleton, f.read(&f.cohorts[..1]).unwrap());
    let changed = f.read(&f.cohorts).unwrap();
    assert_ne!(all.activity_generation, changed.activity_generation);
    assert_eq!(all.transcript_generation, changed.transcript_generation);
    f.store
        .connection
        .execute(
            "UPDATE session_slot_projections SET generation='new-slot' WHERE session_key=?1",
            [&key],
        )
        .unwrap();
    assert_ne!(
        changed.transcript_generation,
        f.read(&f.cohorts).unwrap().transcript_generation
    );
}

#[test]
fn refuses_malformed_and_stale_cohorts() {
    let f = Fixture::new();
    assert_eq!(f.read(&[]), Err(CohortReadError::Malformed));
    let mut cohort = f.cohorts[0].clone();
    cohort.witnesses.clear();
    assert_eq!(f.read(&[cohort]), Err(CohortReadError::Malformed));
    let mut cohort = f.cohorts[0].clone();
    cohort.source_ids.clear();
    assert_eq!(f.read(&[cohort]), Err(CohortReadError::Malformed));
    let mut changed = f.cohorts[0].clone();
    changed.witnesses.values_mut().next().unwrap().generation = "changed".into();
    assert_eq!(f.read(&[changed.clone()]), Err(CohortReadError::Stale));
    assert_eq!(
        f.read(&[f.cohorts[0].clone(), changed]),
        Err(CohortReadError::Malformed)
    );
    let mut unknown = f.cohorts[0].clone();
    let witness: SourceWitness = unknown.witnesses.values().next().unwrap().clone();
    unknown.session_uid = "unknown/claude/session".into();
    unknown.witnesses = BTreeMap::from([(unknown.session_uid.clone(), witness)]);
    assert_eq!(f.read(&[unknown]), Err(CohortReadError::NotIndexed));
}

#[test]
fn refuses_missing_and_disagreeing_archive_markers() {
    let f = Fixture::new();
    let selected = &f.cohorts[..1];
    let prefix = serde_json::to_value([0_u8; 32]).unwrap();
    f.mutate_marker(|m| m["source"]["prefix_sha256"] = prefix);
    assert_eq!(f.read(selected), Err(CohortReadError::Stale));
    let expected_prefix = f.first_source().prefix_sha256.clone();
    let bytes = (0..32)
        .map(|i| u8::from_str_radix(&expected_prefix[i * 2..i * 2 + 2], 16).unwrap())
        .collect::<Vec<_>>();
    f.mutate_marker(|m| m["source"]["prefix_sha256"] = serde_json::to_value(bytes).unwrap());
    f.mutate_marker(|m| m["stat"]["size"] = 0.into());
    assert_eq!(f.read(selected), Err(CohortReadError::Stale));
    f.mutate_marker(|m| m["stat"]["size"] = u64::MAX.into());
    f.mutate_marker(|m| m["source"]["offset"] = 0.into());
    assert_eq!(f.read(selected), Err(CohortReadError::Stale));
    f.store
        .connection
        .execute(
            "DELETE FROM meta WHERE key=?1",
            [archive_index_key(&f.first_source().local_path)],
        )
        .unwrap();
    assert_eq!(f.read(selected), Err(CohortReadError::NotIndexed));
}

#[test]
fn refuses_conflicting_chain_fields_and_stale_rules_or_prices() {
    let f = Fixture::new();
    let key = archive_index_key(&f.first_source().local_path);
    let marker: String = f
        .store
        .connection
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r.get(0))
        .unwrap();
    // A marker cannot name two chains. The meta primary key separately prevents
    // two exact-path marker rows; duplicate JSON fields must also refuse.
    let contradictory = marker.replacen(
        '{',
        "{\"chain_id\":\"00000000000000000000000000000000\",",
        1,
    );
    f.store
        .connection
        .execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![key, contradictory],
        )
        .unwrap();
    assert_eq!(f.read(&f.cohorts[..1]), Err(CohortReadError::Stale));
    f.store
        .connection
        .execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![key, marker],
        )
        .unwrap();
    f.store
        .connection
        .execute(
            "UPDATE session_activity_projections SET rules_version=0",
            [],
        )
        .unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Stale));
    f.store.connection.execute("UPDATE session_activity_projections SET rules_version=?1,pricing_fingerprint='outdated'", [crate::events::session_activity::ACTIVITY_RULES_VERSION]).unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Stale));
}

#[test]
fn resolves_native_aliases_by_exact_identity_and_refuses_duplicate_identity() {
    let f = Fixture::new();
    let before = f.read(&f.cohorts[..1]).unwrap();
    let native = &f.first_source().native_id;
    let other: String = f
        .store
        .connection
        .query_row(
            "SELECT session_key FROM session_catalog_sources WHERE native_id<>?1 LIMIT 1",
            [native],
            |r| r.get(0),
        )
        .unwrap();
    f.store.connection.execute(
        "INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES(?1,'codex',?2,'alias-path')",
        params![other, native],
    ).unwrap();
    assert_eq!(before, f.read(&f.cohorts[..1]).unwrap());
    let (key, metadata): (String, String) = f.store.connection.query_row(
        "SELECT c.session_key,c.metadata FROM session_catalog c JOIN session_catalog_sources s USING(session_key) WHERE s.native_id=?1 AND s.source_path=?2",
        params![native, f.first_source().local_path], |r| Ok((r.get(0)?,r.get(1)?)),
    ).unwrap();
    f.store.connection.execute(
        "INSERT INTO session_catalog(session_key,last_ms,harness,metadata) SELECT 'duplicate',last_ms,harness,?1 FROM session_catalog WHERE session_key=?2",
        params![metadata, key],
    ).unwrap();
    f.store.connection.execute(
        "INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES('duplicate','claude',?1,'duplicate-path')",
        [native],
    ).unwrap();
    assert_eq!(f.read(&f.cohorts[..1]), Err(CohortReadError::Malformed));
}

#[test]
fn append_republication_changes_generations_and_old_witness_is_stale() {
    use std::io::Write;
    let mut f = Fixture::new();
    let before = f.read(&f.cohorts).unwrap();
    let old = f.cohorts.clone();
    let markers = f.saved_markers();
    let source = old
        .iter()
        .flat_map(|c| &c.witnesses)
        .find(|(uid, _)| uid.contains("/codex/"))
        .unwrap()
        .1
        .sources[0]
        .clone();
    writeln!(fs::OpenOptions::new().append(true).open(&source.local_path).unwrap(), "{}", serde_json::json!({"type":"turn_context","timestamp":"2026-09-24T00:02:00Z","payload":{"model":"gpt-6-sol"}})).unwrap();
    let (rows, slots) = catalog_fixture_projection(&f.options);
    catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    assert_eq!(f.read(&old), Err(CohortReadError::Stale));
    // Restore fixture archive markers at the newly published indexed prefix.
    // This test exercises index evidence; archive extension verification belongs
    // to the archive writer and caller.
    f.refresh_markers(&rows, &markers);
    f.repin();
    let after = f.read(&f.cohorts).unwrap();
    assert_ne!(before.activity_generation, after.activity_generation);
    assert_ne!(before.transcript_generation, after.transcript_generation);
    assert_eq!(f.read(&old), Err(CohortReadError::Stale));
}

#[test]
fn successful_rules_and_pricing_republication_changes_only_activity() {
    let mut f = Fixture::new();
    let before = f.read(&f.cohorts).unwrap();
    let markers = f.saved_markers();
    let (mut rows, slots) = catalog_fixture_projection(&f.options);
    // Model the recomputed classification/cost result carried by a current
    // publication after replacing obsolete rules/pricing headers.
    let activity = &mut rows
        .iter_mut()
        .find_map(|row| row.session_activity.as_mut())
        .unwrap()
        .activity;
    activity.aggregate.exclusive_cost_complete = !activity.aggregate.exclusive_cost_complete;
    f.store.connection.execute("UPDATE session_activity_projections SET rules_version=0,pricing_fingerprint='obsolete'", []).unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Stale));
    catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    f.refresh_markers(&rows, &markers);
    let after = f.read(&f.cohorts).unwrap();
    assert_ne!(before.activity_generation, after.activity_generation);
    assert_eq!(before.transcript_generation, after.transcript_generation);
}

#[test]
fn refuses_marker_for_a_different_embedded_file() {
    let f = Fixture::new();
    for path in [
        "projects/other/different.jsonl",
        "../different.jsonl",
        "/projects/different.jsonl",
        "",
    ] {
        f.mutate_marker(|m| m["source"]["path"] = path.into());
        assert_eq!(f.read(&f.cohorts[..1]), Err(CohortReadError::Stale));
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

#[test]
fn leaves_database_and_wal_content_unchanged_with_open_writer_and_closed_index() {
    let f = Fixture::new();
    let wal = sidecar(&f.path, "-wal");
    let database = fs::read(&f.path).unwrap();
    let frames = fs::read(&wal).unwrap();
    assert!(!frames.is_empty());
    f.read(&f.cohorts).unwrap();
    assert_eq!(database, fs::read(&f.path).unwrap());
    assert_eq!(frames, fs::read(&wal).unwrap());

    // Copy only SQLite files into a closed index with existing WAL frames.
    // Shared-memory sidecars may be created/refreshed by ordinary SQLite reads.
    let closed = f.root.join("closed.sqlite");
    fs::copy(&f.path, &closed).unwrap();
    fs::copy(&wal, sidecar(&closed, "-wal")).unwrap();
    read_cohort_generations(&closed, &f.binding, &f.cohorts).unwrap();
    assert_eq!(database, fs::read(&closed).unwrap());
    assert_eq!(frames, fs::read(sidecar(&closed, "-wal")).unwrap());
    // Reopening with existing sidecars has the same content guarantee.
    read_cohort_generations(&closed, &f.binding, &f.cohorts).unwrap();
    assert_eq!(frames, fs::read(sidecar(&closed, "-wal")).unwrap());

    f.store
        .connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let cold = f.root.join("cold.sqlite");
    fs::copy(&f.path, &cold).unwrap();
    let bytes = fs::read(&cold).unwrap();
    assert!(!sidecar(&cold, "-shm").exists());
    read_cohort_generations(&cold, &f.binding, &f.cohorts).unwrap();
    assert_eq!(bytes, fs::read(&cold).unwrap());
    assert_eq!(
        fs::read(sidecar(&cold, "-wal")).unwrap_or_default(),
        Vec::<u8>::new()
    );
}

#[test]
fn refuses_unqualified_indexes_and_pending_projections() {
    let f = Fixture::new();
    let mut binding = f.binding.clone();
    binding.index_incarnation = "0".repeat(32);
    assert_eq!(
        read_cohort_generations(&f.path, &binding, &f.cohorts),
        Err(CohortReadError::Unsupported)
    );
    let missing = f.root.join("missing.sqlite");
    assert_eq!(
        read_cohort_generations(&missing, &f.binding, &f.cohorts),
        Err(CohortReadError::Unavailable)
    );
    assert!(!missing.exists());
    let source = f.first_source();
    f.store
        .connection
        .execute(
            "INSERT INTO session_catalog_invalidations VALUES(?1,1,'changed',0)",
            [&source.local_path],
        )
        .unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Stale));
    f.store
        .connection
        .execute("DELETE FROM session_catalog_invalidations", [])
        .unwrap();
    f.store
        .connection
        .execute(
            "UPDATE session_activity_projections SET pricing_fingerprint='pending'",
            [],
        )
        .unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Stale));
    f.store
        .connection
        .execute(
            "UPDATE session_activity_projections SET pricing_fingerprint=?1",
            [&crate::pricing::snapshot().fingerprint],
        )
        .unwrap();
    f.store
        .connection
        .execute(
            "UPDATE session_slot_projections SET source_generation=NULL",
            [],
        )
        .unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Stale));
    f.store
        .connection
        .pragma_update(None, "user_version", 99999)
        .unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Unsupported));
}

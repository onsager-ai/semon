use super::*;
use crate::analytics::Binding;
use crate::origin_projection::{CohortGenerations, CohortReadError, read_cohort_generations};
use semon_asi::{egress::AdmittedCohort, input::SourceWitness};
use std::collections::BTreeMap;

struct Fixture {
    root: PathBuf,
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
                let marker = serde_json::json!({
                    "version": 7, "chain_id": chain.chain_id, "parser_version": CACHE_VERSION,
                    "source": {"root": row.harness, "path": "fixture.jsonl", "native_id": source.native_id,
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
            path,
            binding,
            cohorts,
            store,
        }
    }
    fn read(&self, cohorts: &[AdmittedCohort]) -> Result<CohortGenerations, CohortReadError> {
        read_cohort_generations(&self.path, &self.binding, cohorts)
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
fn aggregates_exact_membership_and_stable_chain_evidence_without_writes() {
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
        fs::metadata(f.path.with_extension("sqlite-wal"))
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
    let root = scratch("cohort-append");
    let (options, rows, slots) = catalog_native_fixture(&root, 2, false);
    let path = crate::events::EventCache::path(&options.cache);
    let (mut store, _) = opened(&path);
    catalog_write(&mut store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    let mut input = SqliteAnalyticsInput::open(&path, "fixture").unwrap();
    let before = input.read_admitted(None, InputBudget::default()).unwrap();
    let source = rows.iter().find(|r| r.harness == "codex").unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&source.sources[0].path).unwrap(), "{}", serde_json::json!({"type":"turn_context","timestamp":"2026-09-24T00:02:00Z","payload":{"model":"gpt-6-sol"}})).unwrap();
    let (new_rows, slots) = catalog_fixture_projection(&options);
    catalog_write(&mut store, &new_rows, &slots, false, CatalogWriter::Auto).unwrap();
    let after = input.read_admitted(None, InputBudget::default()).unwrap();
    let admitted = |batch: semon_asi::input::AdmittedBatch| {
        batch
            .changes
            .into_iter()
            .find_map(|c| match c {
                SessionChange::Admitted(s) if s.session.harness == "codex" => Some(s),
                _ => None,
            })
            .unwrap()
    };
    let before = admitted(before);
    let after = admitted(after);
    assert_ne!(before.source_witness, after.source_witness);
    assert_ne!(
        before.derivation.activity_generation,
        after.derivation.activity_generation
    );
    drop(input);
    drop(store);
    fs::remove_dir_all(root).unwrap();
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

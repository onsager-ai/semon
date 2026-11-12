use super::*;
use crate::analytics::Binding;
use crate::origin_projection::{CohortGenerations, CohortReadError, read_cohort_generations};
use semon_asi::{egress::AdmittedCohort, input::SourceWitness};
use std::collections::BTreeMap;

fn admit_sources(
    store: &mut SqliteStore,
    options: &crate::Options,
    rows: &[crate::model::summary::CatalogRow],
) {
    let archive = crate::archive::Archive::new(crate::archive::standalone_dir(&options.cache));
    for source in rows.iter().flat_map(|row| &row.sources) {
        let input = crate::native_evidence::source_input(&source.path, options).unwrap();
        let native = crate::archive::NativeSource {
            home: input.root.home(options).clone(),
            root: input.root.as_str().to_owned(),
            path: input.path,
            native_id: source.native_id.clone(),
            ledger: Ledger {
                stat: Stat {
                    dev: source.dev,
                    ino: source.ino,
                    size: source.size,
                    modified_ns: source.modified_ns,
                    changed_ns: source.changed_ns,
                },
                offset: source.offset,
                prefix: source.prefix_sha256,
                tail: source.tail_sha256,
            },
            sidecar: None,
            chain_id: String::new(),
        };
        let admitted = archive
            .admit_native(
                &mut crate::sealed::LogFile::open(&source.path).unwrap(),
                &native,
            )
            .unwrap();
        assert!(admitted.is_some());
        store.record_chain_admission(source.path.to_str().unwrap(), &source.native_id, admitted);
    }
}

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
        admit_sources(&mut store, &options, &rows);
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
    fn first_source(&self) -> &semon_asi::input::Source {
        &self.cohorts[0].witnesses.values().next().unwrap().sources[0]
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
    assert_eq!(
        all.chains[&source.native_id].chain_id,
        cohort_chain(&f.store.connection, source).unwrap()
    );
    assert_eq!(
        f.store
            .connection
            .query_row(
                "SELECT count(*) FROM meta WHERE key GLOB 'archive_event_index:*'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
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
fn refuses_missing_and_disagreeing_chain_evidence() {
    let f = Fixture::new();
    let selected = &f.cohorts[..1];
    for assignment in [
        "prefix_sha256='wrong'",
        "recorded_size=0",
        "consumed_offset=0",
        "native_id='other'",
        "chain_id='invalid'",
    ] {
        f.store
            .connection
            .execute_batch("SAVEPOINT evidence")
            .unwrap();
        f.store
            .connection
            .execute(
                &format!("UPDATE native_chain_evidence SET {assignment} WHERE source_path=?1"),
                [&f.first_source().local_path],
            )
            .unwrap();
        assert_eq!(
            cohort_chain(&f.store.connection, f.first_source()),
            Err(CohortReadError::Stale)
        );
        f.store
            .connection
            .execute_batch("ROLLBACK TO evidence; RELEASE evidence")
            .unwrap();
    }
    f.store
        .connection
        .execute(
            "DELETE FROM native_chain_evidence WHERE source_path=?1",
            [&f.first_source().local_path],
        )
        .unwrap();
    assert_eq!(f.read(selected), Err(CohortReadError::NotIndexed));
}

#[test]
fn refuses_conflicting_chain_fields_and_stale_rules_or_prices() {
    let f = Fixture::new();
    // The exact-path primary key refuses a second candidate.
    assert!(f.store.connection.execute("INSERT INTO native_chain_evidence SELECT * FROM native_chain_evidence WHERE source_path=?1", [&f.first_source().local_path]).is_err());
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
    admit_sources(&mut f.store, &f.options, &rows);
    catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    assert_eq!(f.read(&old), Err(CohortReadError::Stale));
    f.repin();
    let after = f.read(&f.cohorts).unwrap();
    assert_ne!(before.activity_generation, after.activity_generation);
    assert_ne!(before.transcript_generation, after.transcript_generation);
    assert_eq!(
        before.chains[&source.native_id].chain_id,
        after.chains[&source.native_id].chain_id
    );
    let fresh = f
        .cohorts
        .iter()
        .flat_map(|c| c.witnesses.values())
        .flat_map(|w| &w.sources)
        .find(|s| s.native_id == source.native_id)
        .unwrap();
    assert!(fresh.consumed_offset > source.consumed_offset);
    assert_ne!(fresh.prefix_sha256, source.prefix_sha256);
    assert_eq!(
        cohort_chain(&f.store.connection, fresh).unwrap(),
        before.chains[&source.native_id].chain_id
    );
    assert_eq!(
        cohort_chain(&f.store.connection, &source),
        Err(CohortReadError::Stale)
    );
    assert_eq!(f.read(&old), Err(CohortReadError::Stale));
}

#[test]
fn successful_rules_and_pricing_republication_changes_only_activity() {
    let mut f = Fixture::new();
    let before = f.read(&f.cohorts).unwrap();
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
    admit_sources(&mut f.store, &f.options, &rows);
    catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    let after = f.read(&f.cohorts).unwrap();
    assert_ne!(before.activity_generation, after.activity_generation);
    assert_eq!(before.transcript_generation, after.transcript_generation);
}

#[test]
fn requires_exact_source_path_and_ignores_retention_markers() {
    let mut f = Fixture::new();
    let before = f.read(&f.cohorts).unwrap();
    let key = archive_index_key(&f.first_source().local_path);
    let (rows, slots) = catalog_fixture_projection(&f.options);
    // A well-formed marker of the supported format that carries no chain identity.
    let source = rows
        .iter()
        .flat_map(|row| &row.sources)
        .find(|source| source.path.to_str() == Some(f.first_source().local_path.as_str()))
        .unwrap();
    let input = crate::native_evidence::source_input(&source.path, &f.options).unwrap();
    let marker = serde_json::json!({
        "version": 7, "chain_id": null, "parser_version": CACHE_VERSION,
        "source": {"root": input.root.as_str(), "path": input.path, "native_id": source.native_id,
            "offset": source.offset, "prefix_sha256": source.prefix_sha256, "tail_sha256": source.tail_sha256},
        "stat": {"dev": source.dev, "ino": source.ino, "size": source.size,
            "modified_ns": source.modified_ns, "changed_ns": source.changed_ns},
        "projection_version": crate::slot_projection::VERSION, "projection_generation": "fixture"
    })
    .to_string();
    f.store
        .connection
        .execute("INSERT INTO meta VALUES(?1,?2)", [&key, &marker])
        .unwrap();
    assert_eq!(before, f.read(&f.cohorts).unwrap());
    admit_sources(&mut f.store, &f.options, &rows);
    catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
    assert_eq!(
        f.store
            .connection
            .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r
                .get::<_, String>(0))
            .unwrap(),
        marker
    );
    f.store
        .connection
        .execute(
            "UPDATE native_chain_evidence SET source_path='other/' || source_path",
            [],
        )
        .unwrap();
    assert_eq!(f.read(&f.cohorts), Err(CohortReadError::NotIndexed));
}

#[test]
fn native_scan_carries_first_seen_admission_into_source_publication() {
    let root = scratch("scan-chain-publication");
    let (mut options, _, _) = catalog_native_fixture(&root, 2, false);
    let inputs = crate::inputs::inputs(&options)
        .unwrap()
        .into_iter()
        .filter(|input| input.path.ends_with(".jsonl"))
        .collect::<Vec<_>>();
    let mut cache = crate::events::EventCache::open_scoped(&options.cache).unwrap();
    let prepared =
        crate::model::prepare_sources(&options, &mut cache, &inputs, CATALOG_FIXTURE_NOW).unwrap();
    assert_eq!(
        cache.publish_partial_catalog(&prepared).unwrap(),
        Outcome::Written
    );
    let path = crate::events::EventCache::path(&options.cache);
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM native_chain_evidence", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM meta WHERE key GLOB 'archive_event_index:*'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(connection);
    let facts = crate::Facts {
        version: crate::FACTS_VERSION,
        hostname: "synthetic-chain".into(),
        codex_rollouts: Some(
            inputs
                .iter()
                .filter(|input| input.root == crate::InputRoot::Codex)
                .map(|input| input.path.clone())
                .collect(),
        ),
        ..Default::default()
    };
    options.facts = Some(root.join("facts.json"));
    let intent = crate::SessionFactsWrite::begin(&options, "", &facts).unwrap();
    crate::write_facts(options.facts.as_ref().unwrap(), &facts).unwrap();
    intent.complete().unwrap();
    let prepared =
        crate::model::prepare_sources(&options, &mut cache, &inputs, CATALOG_FIXTURE_NOW).unwrap();
    assert_eq!(
        cache.publish_partial_catalog(&prepared).unwrap(),
        Outcome::Written
    );
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM native_chain_evidence", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        0
    );
    drop(connection);
    drop(cache);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skipped_or_failed_admission_removes_evidence_only_at_publication() {
    for reason in ["skipped", "failed"] {
        let mut f = Fixture::new();
        let (rows, slots) = catalog_fixture_projection(&f.options);
        let before = f.read(&f.cohorts).unwrap();
        // These scan outcomes all carry no archive result to publication.
        for source in rows.iter().flat_map(|row| &row.sources) {
            f.store
                .record_chain_admission(source.path.to_str().unwrap(), &source.native_id, None);
        }
        assert_eq!(before, f.read(&f.cohorts).unwrap(), "{reason}");
        f.store.connection.execute_batch("CREATE TEMP TRIGGER chain_publication_failure BEFORE DELETE ON native_chain_evidence BEGIN SELECT RAISE(ABORT,'chain evidence'); END").unwrap();
        assert!(catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).is_err());
        assert_eq!(before, f.read(&f.cohorts).unwrap(), "{reason}");
        f.store
            .connection
            .execute_batch("DROP TRIGGER chain_publication_failure")
            .unwrap();
        catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
        assert_eq!(
            f.read(&f.cohorts),
            Err(CohortReadError::NotIndexed),
            "{reason}"
        );
    }
}

#[test]
fn rewrite_and_truncation_republication_refuse_old_prefixes() {
    for truncate in [false, true] {
        let mut f = Fixture::new();
        let source = f.first_source().clone();
        let mut bytes = fs::read(&source.local_path).unwrap();
        if truncate {
            let end = bytes[..bytes.len() - 1]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .unwrap()
                + 1;
            bytes.truncate(end);
        } else {
            let position = bytes
                .windows(b"Native".len())
                .position(|window| window == b"Native")
                .unwrap();
            bytes[position] = b'M';
        }
        fs::write(&source.local_path, bytes).unwrap();
        let (rows, slots) = catalog_fixture_projection(&f.options);
        admit_sources(&mut f.store, &f.options, &rows);
        catalog_write(&mut f.store, &rows, &slots, false, CatalogWriter::Auto).unwrap();
        assert_eq!(
            cohort_chain(&f.store.connection, &source),
            Err(CohortReadError::Stale)
        );
        assert_eq!(f.read(&f.cohorts), Err(CohortReadError::Stale));
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

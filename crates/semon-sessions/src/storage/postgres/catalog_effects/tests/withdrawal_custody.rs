use super::super::snapshot;
use super::*;
use crate::events::{EventCache, Outcome};
use crate::storage::{
    catalog::CatalogSnapshot, native_events::NativeEventStore, postgres::txn::TxnKind,
};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

#[test]
fn pg_catalog127_republish_then_withdraw_preserves_original_search_authority() {
    let data = build_oracle(true, false);
    let namespace = ns("withdrawal-republished-original-search");
    let source_path = data.rows[0].sources[0].path.clone();
    let mut options = Options {
        source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap(),
        claude_home: data.root.join("claude"),
        claude_json: data.root.join("claude.json"),
        codex_home: data.root.join("codex"),
        copilot_home: data.root.join("copilot"),
        proc_root: data.root.join("proc"),
        cache: data.root.join("withdrawal-republish-source-index.sqlite3"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    let key = data.rows[0].key.clone();
    let original_row = data.rows[0].clone();
    let (pool, _) = fixture(|_| {});
    let control = PgCancellation::default();
    let (initial_generation, _) =
        bind_custody_fixture(&pool, &namespace, &options, &data.rows, &data.projections);

    let host = crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap();
    let config = host
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    options.source = host.with_store(config).unwrap();
    let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    assert_eq!(
        cache.session_catalog_generation().as_deref(),
        Some(initial_generation.as_str())
    );

    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&source_path)
        .unwrap();
    for line in [
        serde_json::json!({"type":"custom-title","sessionId":"one","customTitle":"sanitized updated title"}),
        serde_json::json!({"type":"user","sessionId":"one","uuid":"withdrawal-update-user","parentUuid":"a1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:03Z","message":{"role":"user","content":"sanitized updated input"}}),
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"withdrawal-update-answer","parentUuid":"withdrawal-update-user","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:04Z","message":{"id":"withdrawal-update-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized updated answer"}]}}),
    ] {
        writeln!(file, "{line}").unwrap();
    }
    drop(file);
    cache
        .ingest(&"f".repeat(64), &source_path, "claude")
        .unwrap();
    let input = crate::native_evidence::catalog_input(&source_path, &options).unwrap();
    let updated =
        crate::model::prepare_sources(&options, &mut cache, &[input], 1790812804000).unwrap();
    assert_eq!(updated.rows.len(), 1);
    let updated_row = updated.rows[0].clone();
    assert_eq!(updated_row.key, key);
    let original_last = original_row.last.unwrap();
    let updated_last = updated_row.last.unwrap();
    assert!(updated_last > original_last);

    let search_state = |generation: &str| {
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let documents = tx
                .driver
                .query(
                    "SELECT scope,fields,complete FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 ORDER BY scope",
                    &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &key.as_bytes()],
                )
                .map_err(sqlstate::error)?
                .iter()
                .map(|row| {
                    (
                        row.get::<_, Vec<u8>>(0),
                        (row.get::<_, Vec<u8>>(1), row.get::<_, bool>(2)),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let postings = tx
                .driver
                .query(
                    "SELECT gram,last_ms FROM semon_pg_native_catalog_search_memberships WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 ORDER BY gram",
                    &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &key.as_bytes()],
                )
                .map_err(sqlstate::error)?
                .iter()
                .map(|row| (row.get::<_, Vec<u8>>(0), row.get::<_, i64>(1)))
                .collect::<Vec<_>>();
            Ok((documents, postings))
        })
        .unwrap()
    };
    let original_search = search_state(&initial_generation);
    assert_eq!(original_search.0.len(), 2);
    assert_eq!(
        original_search.0[b"current".as_slice()],
        original_search.0[b"retained_history".as_slice()]
    );
    assert!(original_search.0[b"retained_history".as_slice()].1);
    assert!(!original_search.1.is_empty());
    assert!(
        original_search
            .1
            .iter()
            .all(|(_, last_ms)| *last_ms == original_last)
    );

    let mut republish_attempts = 0;
    let updated_generation = loop {
        republish_attempts += 1;
        let (outcome, generation) = cache.publish_partial_catalog_fenced(&updated).unwrap();
        match outcome {
            Outcome::Written => break generation.expect("updated catalog generation"),
            Outcome::Deferred => assert!(
                republish_attempts < 16,
                "republish custody did not make progress"
            ),
            other => panic!("ordinary republish returned {other:?}"),
        }
    };
    assert_ne!(updated_generation, initial_generation);
    let updated_search = search_state(&updated_generation);
    assert_eq!(updated_search.0.len(), 2);
    assert_ne!(
        updated_search.0[b"retained_history".as_slice()].0,
        original_search.0[b"retained_history".as_slice()].0
    );
    assert!(!updated_search.1.is_empty());
    assert!(
        updated_search
            .1
            .iter()
            .all(|(_, last_ms)| *last_ms == updated_last)
    );

    let authority = |generation: &str| {
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let rows = tx
                .driver
                .query(
                    "SELECT o.owner_generation,o.original,o.last_ms,h.last_ms,h.metadata FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.key_digest=sha256($3::bytea) AND o.session_key=$3",
                    &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &key.as_bytes()],
                )
                .map_err(sqlstate::error)?;
            rows.first()
                .map(|row| {
                    Ok((
                        String::from_utf8(row.get(0))
                            .map_err(|_| crate::storage::StorageError::ScopeChanged)?,
                        row.get::<_, bool>(1),
                        row.get::<_, i64>(2),
                        row.get::<_, i64>(3),
                        row.get::<_, Vec<u8>>(4),
                    ))
                })
                .transpose()
        })
        .unwrap()
    };
    let updated_authority = authority(&updated_generation).expect("original History owner");
    assert_eq!(updated_authority.0, updated_generation);
    assert!(updated_authority.1);
    assert_eq!(updated_authority.2, original_last);
    assert_eq!(updated_authority.3, original_last);
    let updated_header: CatalogRow = serde_json::from_slice(&updated_authority.4).unwrap();
    let mut expected_original = original_row.clone();
    expected_original.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
    assert_eq!(
        super::custody_row_content(std::slice::from_ref(&updated_header)),
        super::custody_row_content(std::slice::from_ref(&expected_original))
    );

    let catalog_snapshot = || {
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let tables = tx
                .driver
                .query(
                    "SELECT relname::text FROM pg_class WHERE relnamespace=current_schema()::regnamespace AND relkind='r' AND relname LIKE 'semon_pg_native_catalog_%' ORDER BY relname",
                    &[],
                )
                .map_err(sqlstate::error)?;
            tables
                .iter()
                .map(|row| {
                    let name: String = row.get(0);
                    let values = tx
                        .driver
                        .query(
                            &format!("SELECT to_jsonb(t)::text FROM \"{name}\" t ORDER BY to_jsonb(t)::text"),
                            &[],
                        )
                        .map_err(sqlstate::error)?;
                    Ok((
                        name,
                        values.iter().map(|row| row.get(0)).collect::<Vec<String>>(),
                    ))
                })
                .collect::<Result<Vec<_>, crate::storage::StorageError>>()
        })
        .unwrap()
    };

    let original_history_metadata = updated_authority.4.clone();
    let mut tampered_history: CatalogRow =
        serde_json::from_slice(&original_history_metadata).unwrap();
    tampered_history.repo = Some("tampered-history-repo".into());
    let tampered_history_metadata = serde_json::to_vec(&tampered_history).unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let changed = tx
            .driver
            .execute(
                "UPDATE semon_pg_native_catalog_history_headers SET metadata=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND metadata=$5",
                &[
                    &namespace.as_str().as_bytes(),
                    &updated_generation.as_bytes(),
                    &key.as_bytes(),
                    &tampered_history_metadata,
                    &original_history_metadata,
                ],
            )
            .map_err(sqlstate::error)?;
        assert_eq!(changed, 1);
        Ok(())
    })
    .unwrap();
    let archived = source_path.with_file_name("republished-original.withdrawn.jsonl");
    fs::rename(&source_path, &archived).unwrap();
    let seen = BTreeSet::new();
    let before_tampered_withdrawal = catalog_snapshot();
    let mut dirty = false;
    let error = cache
        .retain(&seen, &mut dirty)
        .expect_err("withdrawal must reject History metadata that disagrees with its mirrors");
    assert_eq!(
        error.to_string(),
        crate::storage::StorageError::ScopeChanged.to_string()
    );
    let head = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            super::super::super::model_cache::generation(tx)
        })
        .unwrap();
    assert_eq!(head.as_deref(), Some(updated_generation.as_str()));
    assert_eq!(catalog_snapshot(), before_tampered_withdrawal);
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let changed = tx
            .driver
            .execute(
                "UPDATE semon_pg_native_catalog_history_headers SET metadata=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND metadata=$5",
                &[
                    &namespace.as_str().as_bytes(),
                    &updated_generation.as_bytes(),
                    &key.as_bytes(),
                    &original_history_metadata,
                    &tampered_history_metadata,
                ],
            )
            .map_err(sqlstate::error)?;
        assert_eq!(changed, 1);
        Ok(())
    })
    .unwrap();

    let mut attempts = 0;
    let withdrawn_generation = loop {
        attempts += 1;
        let before_attempt = catalog_snapshot();
        let mut dirty = false;
        if let Err(error) = cache.retain(&seen, &mut dirty) {
            let head = pool
                .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
                    super::super::super::model_cache::generation(tx)
                })
                .unwrap();
            let after_attempt = catalog_snapshot();
            assert_eq!(head.as_deref(), Some(updated_generation.as_str()));
            assert_eq!(after_attempt, before_attempt);
            assert_eq!(
                error.to_string(),
                crate::storage::StorageError::ScopeChanged.to_string()
            );
            drop(cache);
            fs::remove_dir_all(&data.root).unwrap();
            panic!("republished Current withdrawal must succeed; verifier refused it: {error}");
        }
        let head = pool
            .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
                super::super::super::model_cache::generation(tx)
            })
            .unwrap();
        if head.as_deref() != Some(updated_generation.as_str()) {
            break head.expect("withdrawal catalog generation");
        }
        assert!(
            dirty,
            "bounded History custody should request a continuation"
        );
        assert!(attempts < 16, "withdrawal custody did not make progress");
    };

    assert_eq!(
        cache.session_catalog_generation().as_deref(),
        Some(withdrawn_generation.as_str())
    );
    let withdrawn_search = search_state(&withdrawn_generation);
    assert_eq!(withdrawn_search.0.len(), 1);
    assert_eq!(
        withdrawn_search.0[b"retained_history".as_slice()],
        original_search.0[b"retained_history".as_slice()]
    );
    assert_eq!(withdrawn_search.1, original_search.1);
    let withdrawn_authority = authority(&withdrawn_generation).expect("retained original owner");
    assert_eq!(withdrawn_authority.0, updated_generation);
    assert!(withdrawn_authority.1);
    assert_eq!(withdrawn_authority.2, original_last);
    assert_eq!(withdrawn_authority.3, original_last);
    let retained_header: CatalogRow = serde_json::from_slice(&withdrawn_authority.4).unwrap();
    assert_eq!(
        super::custody_row_content(std::slice::from_ref(&retained_header)),
        super::custody_row_content(std::slice::from_ref(&expected_original))
    );
    snapshot::with_generation(&pool, &namespace, &control, &withdrawn_generation, |view| {
        assert!(view.catalog(&key, false)?.is_none());
        let retained = view
            .catalog(&key, true)?
            .ok_or(crate::storage::StorageError::ScopeChanged)?;
        assert!(view.history_qualified(&retained).unwrap());
        Ok(())
    })
    .unwrap();
    drop(cache);
    fs::remove_dir_all(&data.root).unwrap();
}

#[test]
fn pg_catalog127_current_only_withdrawal_resumes_bounded_history_before_retained_search() {
    let data = build_oracle(true, false);
    let namespace = ns("withdrawal-current-only-custody");
    let source = data.rows[0].sources[0].path.clone();
    let second_source = source.with_file_name("two.jsonl");
    let third_source = source.with_file_name("three.jsonl");
    let mut options = Options {
        source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap(),
        claude_home: data.root.join("claude"),
        claude_json: data.root.join("claude.json"),
        codex_home: data.root.join("codex"),
        copilot_home: data.root.join("copilot"),
        proc_root: data.root.join("proc"),
        cache: data.root.join("withdrawal-custody-source-index.sqlite3"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };

    let mut file = fs::OpenOptions::new().append(true).open(&source).unwrap();
    let mut parent = "a1".to_owned();
    for turn in 0..75 {
        let user = format!("withdrawal-custody-user-{turn}");
        let answer = format!("withdrawal-custody-answer-{turn}");
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"user","sessionId":"one","uuid":user,"parentUuid":parent,"cwd":"/synthetic/project","timestamp":"2026-10-01T00:02:00Z","message":{"role":"user","content":format!("sanitized withdrawal prompt {turn}")}})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"assistant","sessionId":"one","uuid":answer,"parentUuid":user,"cwd":"/synthetic/project","timestamp":"2026-10-01T00:02:00Z","message":{"id":answer,"role":"assistant","model":"claude-test","content":[{"type":"text","text":format!("sanitized withdrawal answer {turn}")}]}})
        )
        .unwrap();
        parent = answer;
    }
    drop(file);
    let second_contents = fs::read_to_string(&source)
        .unwrap()
        .replace("\"sessionId\":\"one\"", "\"sessionId\":\"two\"");
    fs::write(&second_source, second_contents).unwrap();
    let third_contents = fs::read_to_string(&source)
        .unwrap()
        .replace("\"sessionId\":\"one\"", "\"sessionId\":\"three\"");
    fs::write(&third_source, third_contents).unwrap();

    let mut source_cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let inputs = [
        crate::native_evidence::catalog_input(&source, &options).unwrap(),
        crate::native_evidence::catalog_input(&second_source, &options).unwrap(),
        crate::native_evidence::catalog_input(&third_source, &options).unwrap(),
    ];
    let prepared =
        crate::model::prepare_sources(&options, &mut source_cache, &inputs, 1790812802000).unwrap();
    assert_eq!(prepared.rows.len(), 3);
    assert!(
        prepared
            .transcripts
            .iter()
            .all(|projection| projection.slots.len() > 64),
        "each fixture recipe exceeds one 64-slot custody attempt"
    );
    let bytes = prepared
        .rows
        .iter()
        .map(|row| (row.key.clone(), fs::read(&row.sources[0].path).unwrap()))
        .collect::<BTreeMap<_, _>>();
    let (pool, _) = fixture(|_| {});
    let control = PgCancellation::default();
    let (before, rows) = bind_custody_fixture(
        &pool,
        &namespace,
        &options,
        &prepared.rows,
        &prepared.transcripts,
    );
    let keys = rows
        .iter()
        .map(|row| Sha256::digest(row.key.as_bytes()).to_vec())
        .collect::<Vec<_>>();

    let had_history = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            tx.driver
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=ANY($3::bytea[]))",
                    &[&namespace.as_str().as_bytes(), &before.as_bytes(), &keys],
                )
                .map(|row| row.get::<_, bool>(0))
                .map_err(sqlstate::error)
        })
        .unwrap();
    assert!(!had_history, "the source row starts as Current-only");

    let host = crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap();
    let config = host
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    options.source = host.with_store(config).unwrap();
    let mut cache =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    for (at, row) in rows.iter().enumerate() {
        cache
            .ingest(
                &format!("{:064x}", at + 100),
                &row.sources[0].path,
                "claude",
            )
            .unwrap();
    }
    let paths = rows
        .iter()
        .map(|row| (row.key.clone(), row.sources[0].path.clone()))
        .collect::<BTreeMap<_, _>>();
    let untouched_key = rows
        .iter()
        .find(|row| row.sources[0].path.as_path() == third_source.as_path())
        .unwrap()
        .key
        .clone();
    let removed_keys = rows
        .iter()
        .filter(|row| row.key.as_str() != untouched_key.as_str())
        .map(|row| row.key.clone())
        .collect::<BTreeSet<_>>();
    let mut archives = BTreeMap::new();
    for row in rows.iter().filter(|row| removed_keys.contains(&row.key)) {
        let path = &row.sources[0].path;
        let archived = path.with_file_name(format!(
            "{}.withdrawn",
            path.file_name().unwrap().to_string_lossy()
        ));
        fs::rename(path, &archived).unwrap();
        archives.insert(row.key.clone(), archived);
    }

    let untouched_source = &paths[&untouched_key];
    let seen = BTreeSet::from([untouched_source.to_str().unwrap().to_owned()]);
    let mut dirty = false;
    cache.retain(&seen, &mut dirty).unwrap();
    assert!(
        dirty,
        "a deferred removal remains pending for the next step"
    );
    assert_eq!(
        cache.session_catalog_generation().as_deref(),
        Some(before.as_str()),
        "partial custody leaves Current and its head in place"
    );
    let source_paths = rows
        .iter()
        .filter(|row| removed_keys.contains(&row.key))
        .map(|row| row.sources[0].path.to_str().unwrap().as_bytes().to_vec())
        .collect::<Vec<_>>();
    let (progress, has_partial_owner, has_removal_claim) = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let progress = tx
                .driver
                .query(
                    "SELECT session_key FROM semon_pg_native_catalog_history_progress WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=ANY($2::bytea[]) ORDER BY key_digest",
                    &[&namespace.as_str().as_bytes(), &keys],
                )
                .map_err(sqlstate::error)?
                .iter()
                .map(|row| String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged))
                .collect::<Result<Vec<_>, _>>()?;
            let row = tx
                .driver
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation<>$2 AND key_digest=ANY($3::bytea[])), EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=ANY($4::bytea[]) AND reason='removed')",
                    &[
                        &namespace.as_str().as_bytes(),
                        &before.as_bytes(),
                        &keys,
                        &source_paths,
                    ],
                )
                .map_err(sqlstate::error)?;
            Ok((progress, row.get::<_, bool>(0), row.get::<_, bool>(1)))
        })
        .unwrap();
    assert_eq!(progress.len(), 1, "one original owner advances per attempt");
    assert!(!has_partial_owner, "partial custody has no History owner");
    assert!(!has_removal_claim, "partial custody has no removal claim");
    let resumed_key = &progress[0];
    let later_key = rows
        .iter()
        .find(|row| {
            row.key.as_str() != resumed_key.as_str() && row.key.as_str() != untouched_key.as_str()
        })
        .unwrap()
        .key
        .clone();
    let resumed_source = &paths[resumed_key];
    let seen_resumed = BTreeSet::from([resumed_source.to_str().unwrap().to_owned()]);
    let seen_resumed_and_untouched = BTreeSet::from([
        resumed_source.to_str().unwrap().to_owned(),
        untouched_source.to_str().unwrap().to_owned(),
    ]);

    fs::rename(&archives[resumed_key], resumed_source).unwrap();
    cache
        .ingest(&"a".repeat(64), resumed_source, "claude")
        .unwrap();
    let mut dirty = false;
    cache
        .retain(&seen_resumed_and_untouched, &mut dirty)
        .unwrap();
    assert!(
        dirty,
        "the still-missing owner remains queued after reappearance"
    );
    assert_eq!(
        cache.session_catalog_generation().as_deref(),
        Some(before.as_str()),
        "reappearance cancels only that path from the pending batch"
    );

    let mut attempts = 1;
    let later_generation = loop {
        let mut dirty = false;
        cache
            .retain(&seen_resumed_and_untouched, &mut dirty)
            .unwrap();
        attempts += 1;
        let generation = cache.session_catalog_generation().unwrap();
        if generation != before {
            break generation;
        }
        assert!(dirty, "the still-missing owner retries after Deferred");
        assert!(attempts < 16, "bounded custody did not make progress");
    };
    assert!(attempts > 1, "custody resumes across bounded attempts");
    assert_eq!(fs::read(&archives[&later_key]).unwrap(), bytes[&later_key]);
    snapshot::with_generation(&pool, &namespace, &control, &later_generation, |snapshot| {
        assert!(snapshot.catalog(resumed_key, false)?.is_some());
        assert!(snapshot.catalog(&later_key, false)?.is_none());
        let history = snapshot
            .catalog(&later_key, true)?
            .ok_or(StorageError::ScopeChanged)?;
        assert!(snapshot.history_qualified(&history).unwrap());
        snapshot::query::qualify_context_candidate(snapshot, &options, &history, true)
    })
    .unwrap();

    let query = format!(
        "scope=retained_history&limit=50&q={}",
        crate::viewer::percent_encode(&later_key.to_lowercase())
    );
    let reply = snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &later_generation,
        |snapshot| {
            crate::catalog::read_page_snapshot(
                &options,
                namespace.as_str(),
                &crate::catalog::Request::parse(&query).map_err(|_| StorageError::ScopeChanged)?,
                snapshot,
            )
            .map_err(|_| StorageError::ScopeChanged)
        },
    )
    .unwrap();
    assert_eq!(reply.status, 200);
    let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert!(
        page["items"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["key"].as_str() == Some(later_key.as_str()))
        }),
        "the qualified History owner is searchable in retained scope"
    );

    // Re-ingesting the reappeared source observes its new ctime. Until the
    // ordinary producer publishes that changed source witness, a direct
    // withdrawal of this old Current row must conflict. Use an untouched
    // Current owner to exercise a second fresh bounded custody advance.
    let untouched_archive = untouched_source.with_file_name(format!(
        "{}.withdrawn",
        untouched_source.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(untouched_source, &untouched_archive).unwrap();
    archives.insert(untouched_key.clone(), untouched_archive);
    let mut dirty = false;
    cache.retain(&seen_resumed, &mut dirty).unwrap();
    assert!(
        dirty,
        "a later removal starts another bounded custody advance"
    );
    assert_eq!(
        cache.session_catalog_generation().as_deref(),
        Some(later_generation.as_str())
    );
    let mut attempts = 1;
    let generation = loop {
        let mut dirty = false;
        cache.retain(&seen_resumed, &mut dirty).unwrap();
        attempts += 1;
        let generation = cache.session_catalog_generation().unwrap();
        if generation != later_generation {
            break generation;
        }
        assert!(dirty, "the later removal remains queued after Deferred");
        assert!(attempts < 16, "later custody did not make progress");
    };
    assert!(attempts > 1, "later removal resumes bounded custody");
    for row in &rows {
        snapshot::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            if row.key.as_str() == resumed_key.as_str() {
                assert!(snapshot.catalog(&row.key, false)?.is_some());
            } else {
                assert!(snapshot.catalog(&row.key, false)?.is_none());
                let history = snapshot
                    .catalog(&row.key, true)?
                    .ok_or(StorageError::ScopeChanged)?;
                assert!(snapshot.history_qualified(&history).unwrap());
                snapshot::query::qualify_context_candidate(snapshot, &options, &history, true)?;
            }
            Ok(())
        })
        .unwrap();
        let actual = if row.key.as_str() == resumed_key.as_str() {
            fs::read(&paths[&row.key]).unwrap()
        } else {
            fs::read(&archives[&row.key]).unwrap()
        };
        assert_eq!(actual, bytes[&row.key]);
    }

    let retired_paths = [&paths[&later_key], &paths[&untouched_key]]
        .iter()
        .map(|path| path.to_str().unwrap().as_bytes().to_vec())
        .collect::<Vec<_>>();
    let (resumed_changed, resumed_removed, retired_pending) = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let row = tx
                .driver
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='changed'), EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed'), EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=ANY($3::bytea[]))",
                    &[
                        &namespace.as_str().as_bytes(),
                        &resumed_source.to_str().unwrap().as_bytes(),
                        &retired_paths,
                    ],
                )
                .map_err(sqlstate::error)?;
            Ok((
                row.get::<_, bool>(0),
                row.get::<_, bool>(1),
                row.get::<_, bool>(2),
            ))
        })
        .unwrap();
    assert!(resumed_changed, "reappeared source keeps its changed claim");
    assert!(
        !resumed_removed,
        "reappeared source is not claimed as removed"
    );
    assert!(!retired_pending, "withdrawn sources have no pending claim");

    drop(cache);
    drop(source_cache);
    fs::remove_dir_all(data.root).unwrap();
}

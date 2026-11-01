use super::super::snapshot;
use super::*;
use crate::events::EventCache;
use crate::storage::{
    catalog::CatalogSnapshot, native_events::NativeEventStore, postgres::txn::TxnKind,
};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

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
            row.key.as_str() != resumed_key.as_str()
                && row.key.as_str() != untouched_key.as_str()
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
    assert!(!resumed_removed, "reappeared source is not claimed as removed");
    assert!(!retired_pending, "withdrawn sources have no pending claim");

    drop(cache);
    drop(source_cache);
    fs::remove_dir_all(data.root).unwrap();
}

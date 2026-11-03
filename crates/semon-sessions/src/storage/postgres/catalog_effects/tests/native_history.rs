use super::super::super::native_keys;
use super::super::{observations, snapshot};
use super::*;
use crate::events::{EventCache, Outcome};
use crate::storage::{catalog::CatalogSnapshot, native_events::NativeEventStore};
use sha2::Digest;
use std::collections::BTreeSet;
use std::io::Write;

pub(super) fn advance(options: &Options, row: &CatalogRow, operation: &str) -> String {
    let path = &row.sources[0].path;
    writeln!(fs::OpenOptions::new().append(true).open(path).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":row.native_ids[0],"uuid":operation,"timestamp":"2026-10-01T00:01:00Z","cwd":"/synthetic/project","message":{"role":"user","content":operation}})).unwrap();
    let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    cache
        .ingest(
            &format!("{:x}", Sha256::digest(operation.as_bytes())),
            path,
            "claude",
        )
        .unwrap();
    let input = crate::native_evidence::catalog_input(path, options).unwrap();
    let prepared =
        crate::model::prepare_sources(options, &mut cache, &[input], 1790812860000).unwrap();
    for _ in 0..256 {
        let (outcome, generation) = cache.publish_partial_catalog_fenced(&prepared).unwrap();
        if let Some(generation) = generation {
            assert_eq!(outcome, Outcome::Written);
            return generation;
        }
        assert_eq!(outcome, Outcome::Deferred);
    }
    panic!("bounded original custody did not finish");
}

#[test]
fn pg_verified_continuation_qualifies_incomplete_composite_identity() {
    let data = build_oracle(true, false);
    let (pool, _) = fixture(|_| {});
    let namespace = ns("verified-composite-identity");
    let control = PgCancellation::default();
    let mut options = Options {
        source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap(),
        claude_home: data.root.join("claude"),
        claude_json: data.root.join("claude.json"),
        codex_home: data.root.join("codex"),
        copilot_home: data.root.join("copilot"),
        proc_root: data.root.join("proc"),
        cache: data.root.join("composite-index.json"),
        all: true,
        ..Default::default()
    };
    let old_path = data.rows[0].sources[0].path.clone();
    let live_path = options.claude_home.join("projects/project/resume.jsonl");
    writeln!(fs::File::create(&live_path).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"resume","session_id":"one","uuid":"resume-user","timestamp":"2026-10-01T00:00:02Z","cwd":"/synthetic/project","message":{"role":"user","content":"sanitized resumed source"}})).unwrap();
    options
        .source
        .config(&options.cache)
        .unwrap()
        .create_native_store()
        .unwrap();
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut sqlite = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let initial =
        crate::model::prepare_sources(&options, &mut sqlite, &inputs, 1790812803000).unwrap();
    assert_eq!(initial.rows.len(), 1);
    assert_eq!(initial.rows[0].sources.len(), 2);
    drop(sqlite);
    bind_custody_fixture(
        &pool,
        &namespace,
        &options,
        &initial.rows,
        &initial.transcripts,
    );
    let config = options
        .source
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    options.source = options.source.with_store(config).unwrap();
    let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    fs::rename(
        &old_path,
        data.root.join("retained-composite-original.jsonl"),
    )
    .unwrap();
    cache
        .retain(
            &BTreeSet::from([live_path.to_string_lossy().into_owned()]),
            &mut false,
        )
        .unwrap();
    let input = crate::native_evidence::catalog_input(&live_path, &options).unwrap();
    let path = live_path.to_string_lossy().into_owned();
    let prior = cache
        .stored_ledger_batch(std::slice::from_ref(&path))
        .unwrap()
        .remove(0)
        .unwrap();
    let from = fs::metadata(&live_path).unwrap().len();
    let writer =
        crate::SessionSourceWrite::begin(&options, &input, Some(&"a".repeat(32)), from).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&live_path).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"resume","session_id":"one","uuid":"continued-user","parentUuid":"resume-user","timestamp":"2026-10-01T00:00:03Z","cwd":"/synthetic/project","message":{"role":"user","content":"sanitized verified continuation"}})).unwrap();
    writer
        .complete(fs::metadata(&live_path).unwrap().len())
        .unwrap();
    let accepted = crate::events::store::source_write_store(&options)
        .unwrap()
        .append_evidence(&path)
        .unwrap()
        .expect("the completed native writer owns exact append evidence");
    let receipt = cache
        .ingest(&format!("{:064x}", 128), &live_path, "claude")
        .unwrap();
    assert!(!receipt.replayed);
    let current = cache.stored_ledger_batch(&[path]).unwrap().remove(0).unwrap();
    let continuation = crate::storage::catalog::VerifiedSourceContinuation::from_scan(
        &live_path,
        &prior,
        &current,
        &accepted,
        &crate::events::Stat::of(&live_path).unwrap(),
    )
    .expect("this request scanned the exact accepted continuation");
    let mut continued =
        crate::model::prepare_sources(&options, &mut cache, &[input], 1790812804000).unwrap();
    assert_eq!(continued.rows.len(), 1);
    assert_eq!(continued.rows[0].sources.len(), 1);
    // Configured model preparation is a committed Index reader. Its explicit
    // producer carries the scanner proof in this request's typed publication
    // envelope; it cannot recover continuation authority from persisted rows.
    continued
        .relationships
        .presentation
        .as_mut()
        .unwrap()
        .set_source_continuations(vec![continuation])
        .unwrap();
    assert_eq!(
        continued
            .presentation_observations()
            .unwrap()
            .verified_continuations()
            .unwrap()
            .len(),
        1,
        "the ordinary publication must carry the real request-local scanner continuation"
    );
    let mut generation = None;
    for _ in 0..4 {
        let (outcome, published) = cache.publish_partial_catalog_fenced(&continued).unwrap();
        if published.is_some() {
            assert_eq!(outcome, Outcome::Written);
            generation = published;
            break;
        }
        assert_eq!(outcome, Outcome::Deferred);
    }
    let generation = generation.expect("ordinary verified-continuation publication");
    let key = &continued.rows[0].key;
    let owner =
        snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |view| {
            let owner = view.owner(key)?.unwrap();
            assert!(!owner.original);
            assert!(owner.has_current);
            let history = view.catalog(key, true)?.unwrap();
            assert_eq!(history.sources.len(), 2);
            assert!(history.sources.iter().any(|source| source.path == old_path));
            Ok(owner.generation)
        })
        .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute("UPDATE semon_pg_native_catalog_history_headers SET incomplete=true WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3", &[&namespace.as_str().as_bytes(), &owner.as_bytes(), &key.as_bytes()]).map_err(sqlstate::error)?, 1);
        Ok(())
    })
    .unwrap();
    snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |view| {
        let history = view.catalog(key, true)?.unwrap();
        let request =
            crate::catalog::Request::parse(&format!("sid={}", crate::viewer::percent_encode(key)))
                .unwrap();
        let current = view.rows(&request).unwrap().remove(0);
        assert!(
            !view.history_qualified(&history).unwrap(),
            "incomplete recipes cannot qualify full History"
        );
        assert!(
            view.composite_history_identity_qualified(&current, &history)
                .unwrap(),
            "verified continuation permits compact identity for an exact non-original composite"
        );
        Ok(())
    })
    .unwrap();
    fs::remove_dir_all(&data.root).unwrap();
}

#[test]
fn pg_withdrawal_preserves_current_and_older_original_custody() {
    for case in ["current-only", "older-original"] {
        let data = build_oracle(true, false);
        let (pool, _) = fixture(|_| {});
        let namespace = ns(&format!("withdrawal-{case}"));
        let control = PgCancellation::default();
        let mut options = Options {
            source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap(),
            claude_home: data.root.join("claude"),
            claude_json: data.root.join("claude.json"),
            codex_home: data.root.join("codex"),
            copilot_home: data.root.join("copilot"),
            proc_root: data.root.join("proc"),
            cache: data.root.join("index.json"),
            all: true,
            ..Default::default()
        };
        let mut sqlite_options = options.clone();
        sqlite_options.cache = data.root.join("withdrawal-sqlite-index.json");
        let mut sqlite =
            EventCache::open_scoped(&sqlite_options.cache, &sqlite_options.source).unwrap();
        let inputs = crate::inputs::inputs(&sqlite_options).unwrap();
        let prepared =
            crate::model::prepare_sources(&sqlite_options, &mut sqlite, &inputs, 1790812802000)
                .unwrap();
        let (_, mut sqlite_generation) = sqlite.publish_partial_catalog_fenced(&prepared).unwrap();
        let (mut generation, rows, _) =
            bind_custody_fixture(&pool, &namespace, &options, &data.rows, &data.projections);
        let config = options
            .source
            .store_config(crate::storage::StoreBackend::Postgres {
                pool: pool.clone(),
                control: control.clone(),
            })
            .unwrap();
        options.source = options.source.with_store(config).unwrap();
        let original = &rows[0];
        let path = &original.sources[0].path;
        let key = &original.key;
        let host = super::businessflow::body_host(&options, &rows);
        let read_body = |generation: &str, host: &super::businessflow::BodyHost| {
            let store = snapshot::query::QueryCatalog {
                captured: false,
                pool: pool.clone(),
                namespace: namespace.clone(),
                control: control.clone(),
                generation: generation.to_owned(),
                options: options.clone(),
            };
            crate::Query::new(options.clone())
                .call_bound_selected_at(
                    "read_transcript",
                    &serde_json::json!({"id":key,"limit":100}),
                    1790812860000,
                    &store,
                    host,
                    None,
                )
                .unwrap()
        };
        let expected_body = read_body(&generation, &host);
        if case == "older-original" {
            generation = advance(&options, original, "withdrawal-newer-current");
            let prepared =
                crate::model::prepare_sources(&sqlite_options, &mut sqlite, &inputs, 1790812860000)
                    .unwrap();
            sqlite_generation = sqlite
                .publish_catalog_projection_fenced(
                    &crate::slot_projection::Publication {
                        rows: &prepared.rows,
                        transcripts: Some(&prepared.transcripts),
                        observations: None,
                    },
                    sqlite_generation.as_deref(),
                )
                .unwrap();
        }
        let (mut expected, header, slots, old_owner) =
            snapshot::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
                let owner = snapshot.owner(key)?;
                let history = case == "older-original";
                let (row, header, slots) = snapshot.selected(key, history, 0, 64)?.unwrap();
                if history {
                    let current = snapshot.catalog(key, false)?.unwrap();
                    assert_ne!(current.sources, row.sources);
                    assert_ne!(current.last, row.last);
                    assert!(owner.as_ref().unwrap().original);
                } else {
                    assert!(owner.is_none());
                }
                Ok((row, header, slots, owner.map(|owner| owner.generation)))
            })
            .unwrap();
        expected.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        let open = || {
            super::super::super::index_store::open(pool.clone(), namespace.clone(), control.clone())
                .unwrap()
                .0
        };
        let mut index = open();
        let path_string = path.to_str().unwrap().to_owned();
        let ledger = index
            .file_ledgers()
            .unwrap()
            .into_iter()
            .find(|(path, _)| path == &path_string)
            .unwrap()
            .1
            .unwrap();
        let removal = vec![(path_string, Some(ledger))];
        // The captured host independently owns the exact old bytes. Source
        // withdrawal must not require their continued presence at the live path.
        fs::rename(path, data.root.join("retained-native.jsonl")).unwrap();
        sqlite.retain(&BTreeSet::new(), &mut false).unwrap();
        assert!(
            sqlite
                .publish_catalog_projection_fenced(
                    &crate::slot_projection::Publication {
                        rows: &[],
                        transcripts: Some(&[]),
                        observations: None,
                    },
                    sqlite_generation.as_deref(),
                )
                .unwrap()
                .is_some()
        );
        let sqlite_config = sqlite_options.source.config(&sqlite_options.cache).unwrap();
        let sqlite_owner =
            crate::events::store::configured::NativeOwnership::admit(&sqlite_config).unwrap();
        let sqlite_db = crate::storage::sqlite::source_journal::SourceJournal::owned(sqlite_owner)
            .connection()
            .unwrap();
        let sqlite_metadata: String = sqlite_db.query_row(
            "SELECT metadata FROM session_history_catalog WHERE namespace=semon_namespace() AND session_key=?1",
            [key],
            |row| row.get(0),
        ).unwrap();
        let sqlite_retained: CatalogRow = serde_json::from_str(&sqlite_metadata).unwrap();
        assert_eq!(
            serde_json::to_value(&sqlite_retained.sources).unwrap(),
            serde_json::to_value(&expected.sources).unwrap()
        );
        assert_eq!(sqlite_retained.last, expected.last);
        assert_eq!(sqlite_retained.lifecycle, expected.lifecycle);
        let result = index.remove_files(&removal).unwrap();
        assert_eq!(result, Outcome::Written);
        let successor = pool
            .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
                super::super::super::model_cache::generation(tx)
            })
            .unwrap()
            .unwrap();
        assert_ne!(successor, generation);
        drop(index);
        let mut index = open();
        let replay = index.remove_files(&removal).unwrap();
        assert_eq!(replay, Outcome::Written);
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            assert_eq!(super::super::super::model_cache::generation(tx)?.as_deref(), Some(successor.as_str()));
            super::super::search::verify_state(tx, &successor)?;
            let ns = namespace.as_str().as_bytes();
            let documents = tx.driver.query("SELECT scope,fields,complete FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3", &[&ns, &successor.as_bytes(), &key.as_bytes()]).map_err(sqlstate::error)?;
            let (fields, grams, complete) = crate::catalog_search::prepared_fields_port(&expected).unwrap();
            assert_eq!(documents.len(), 1);
            assert_eq!(documents[0].get::<_, Vec<u8>>(0), b"retained_history");
            assert_eq!(documents[0].get::<_, Vec<u8>>(1), fields.as_bytes());
            assert_eq!(documents[0].get::<_, bool>(2), complete);
            let postings = tx.driver.query("SELECT gram,last_ms FROM semon_pg_native_catalog_search_memberships WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3 ORDER BY gram", &[&ns, &successor.as_bytes(), &key.as_bytes()]).map_err(sqlstate::error)?;
            let actual = postings.iter().map(|posting| {
                assert_eq!(posting.get::<_, i64>(1), expected.last.unwrap_or(0));
                String::from_utf8(posting.get(0)).unwrap()
            }).collect::<BTreeSet<_>>();
            assert_eq!(actual, grams);
            Ok(())
        }).unwrap();
        snapshot::with_admitted_snapshot(&pool, &namespace, &control, &successor, |snapshot| {
            assert!(snapshot.catalog(key, false)?.is_none());
            let owner = snapshot.owner(key)?.unwrap();
            assert!(owner.original);
            assert!(!owner.has_current);
            if let Some(old_owner) = &old_owner {
                assert_eq!(
                    &owner.generation, old_owner,
                    "reuse immutable older original"
                );
            }
            let (retained, retained_header, retained_slots) =
                snapshot.selected(key, true, 0, 64)?.unwrap();
            assert_eq!(
                serde_json::to_value(&retained).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
            assert_eq!(retained_header, header);
            assert_eq!(retained_slots, slots);
            snapshot::query::qualify_context_candidate(snapshot, &options, &retained, true)?;
            pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
                for source in &retained.sources {
                    super::super::selected_reader::native_proof(tx, &successor, source)?;
                }
                Ok(())
            })?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            read_body(&successor, &host),
            expected_body,
            "{case} retained body after restart"
        );
        fs::remove_dir_all(&data.root).unwrap();
    }
}

#[test]
fn pg_native_history_observation_follows_original_custody_and_refuses_catalog_only_authority() {
    let data = build_oracle(true, false);
    let (pool, _) = fixture(|_| {});
    let namespace = ns("native-history-observation");
    let control = PgCancellation::default();
    let mut options = Options {
        source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap(),
        claude_home: data.root.join("claude"),
        claude_json: data.root.join("claude.json"),
        codex_home: data.root.join("codex"),
        copilot_home: data.root.join("copilot"),
        proc_root: data.root.join("proc"),
        cache: data.root.join("index.json"),
        all: true,
        ..Default::default()
    };
    let (original_generation, rows, _) =
        bind_custody_fixture(&pool, &namespace, &options, &data.rows, &data.projections);
    let config = options
        .source
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    options.source = options.source.with_store(config.clone()).unwrap();
    let original = &rows[0];
    let host = super::businessflow::body_host(&options, &rows);
    let second = advance(&options, original, "native-history-second");
    let (history, owner, continued) =
        snapshot::with_generation(&pool, &namespace, &control, &second, |snapshot| {
            let history = snapshot.catalog(&original.key, true)?.unwrap();
            snapshot::query::qualify_context_candidate(snapshot, &options, &history, true)?;
            Ok((
                history,
                snapshot.owner(&original.key)?.unwrap().generation,
                snapshot.catalog(&original.key, false)?.unwrap(),
            ))
        })
        .unwrap();
    assert_eq!(history.sources, original.sources);
    assert_ne!(history.sources, continued.sources);
    let latest = advance(&options, &continued, "native-history-third");
    // Reopen the native store and read the inherited immutable owner.
    drop(EventCache::open_scoped(&options.cache, &options.source).unwrap());
    let key = observations::history_key(&owner, &serde_json::to_vec(&history).unwrap());
    let reference = pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let reference = native_keys::read_meta(tx, key.as_bytes(), 512)?.unwrap();
        let refs: i64 = tx.driver.query_one("SELECT count(*) FROM semon_pg_native_selected_refs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0);
        assert_eq!(refs, 0, "ordinary native custody needs no transferred proof references");
        Ok(reference)
    }).unwrap();
    snapshot::with_generation(&pool, &namespace, &control, &latest, |snapshot| {
        assert_eq!(
            snapshot
                .owner(&original.key)?
                .as_ref()
                .map(|owner| owner.generation.as_str()),
            Some(owner.as_str())
        );
        snapshot::query::qualify_context_candidate(snapshot, &options, &history, true)
    })
    .unwrap();
    let store = snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: latest.clone(),
        options: options.clone(),
    };
    let fields = crate::Query::new(options.clone())
        .call_bound_selected_at(
            "get_session",
            &serde_json::json!({"id":original.key}),
            1790812860000,
            &store,
            &host,
            None,
        )
        .unwrap();
    assert!(fields.to_string().contains("sanitized"));

    snapshot::with_admitted_snapshot(&pool, &namespace, &control, &latest, |_| {
        let mut checked = false;
        let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver.execute("DELETE FROM semon_pg_native_catalog_proofs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND source_path=$3", &[&namespace.as_str().as_bytes(),&latest.as_bytes(),&history.sources[0].path.to_str().unwrap().as_bytes()]).map_err(sqlstate::error)?;
            assert!(matches!(super::super::selected_reader::native_proof(tx, &latest, &history.sources[0]), Err(StorageError::ScopeChanged)));
            checked = true;
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
        assert!(checked);
        Ok(())
    }).unwrap();

    for fault in [
        "missing_reference",
        "admission",
        "stream",
        "version",
        "original_operation",
        "custodian_operation",
        "header",
        "source_generation",
    ] {
        let mut checked = false;
        let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            match fault {
                "missing_reference" => native_keys::delete_meta(tx, key.as_bytes())?,
                "admission" | "stream" | "version" => {
                    let mut changed: observations::HistoryReference = serde_json::from_slice(&reference).unwrap();
                    match fault {
                        "admission" => changed.receipt.0 += 1,
                        "stream" => changed.receipt.1 = "d".repeat(64),
                        "version" => changed.version += 1,
                        _ => unreachable!(),
                    }
                    native_keys::put_meta(tx, key.as_bytes(), &serde_json::to_vec(&changed).unwrap())?;
                }
                "original_operation" | "custodian_operation" => {
                    let generation = if fault == "original_operation" { &original_generation } else { &owner };
                    tx.driver.execute("DELETE FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&ns,&generation.as_bytes()]).map_err(sqlstate::error)?;
                }
                "header" => {
                    tx.driver.execute("UPDATE semon_pg_native_catalog_history_headers SET incomplete=true WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3", &[&ns,&owner.as_bytes(),&original.key.as_bytes()]).map_err(sqlstate::error)?;
                }
                "source_generation" => {
                    tx.driver.execute("UPDATE semon_pg_native_catalog_history_headers SET source_generation=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3", &[&ns,&owner.as_bytes(),&original.key.as_bytes(),&"0".repeat(64).as_bytes()]).map_err(sqlstate::error)?;
                }
                _ => unreachable!(),
            }
            snapshot::in_transaction(tx, &latest, |snapshot| {
                if fault == "missing_reference" {
                    assert!(snapshot.row_runtime_binding(&history).unwrap().is_some(), "original Catalog publication remains genuine");
                }
                assert!(matches!(snapshot::query::qualify_context_candidate(snapshot, &options, &history, true), Err(StorageError::ScopeChanged)), "{fault}");
                Ok(())
            })?;
            checked = true;
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
        assert!(checked, "{fault}");
        snapshot::with_generation(&pool, &namespace, &control, &latest, |snapshot| {
            snapshot::query::qualify_context_candidate(snapshot, &options, &history, true)
        })
        .unwrap();
    }
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let saved: observations::HistoryReference = serde_json::from_slice(&reference).unwrap();
        observations::issue_history(
            tx,
            &owner,
            &serde_json::to_vec(&history).unwrap(),
            saved.lifecycle,
            saved.receipt,
        )?;
        let mut changed: observations::HistoryReference =
            serde_json::from_slice(&reference).unwrap();
        changed.receipt.0 += 1;
        assert_eq!(
            observations::issue_history(
                tx,
                &owner,
                &serde_json::to_vec(&history).unwrap(),
                changed.lifecycle,
                changed.receipt
            ),
            Err(StorageError::ScopeChanged)
        );
        Ok(())
    })
    .unwrap();
    fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_catalog127_retained_details_follow_the_original_trace_family_and_later_carry() {
    fn selected_details(
        snapshot: &snapshot::Snapshot<'_, '_>,
        row: &CatalogRow,
    ) -> Result<Option<serde_json::Value>, StorageError> {
        snapshot
            .selected_details(
                &row.key,
                &row.sources,
                &mut |_, _| serde_json::json!({"state":null,"observation":"unavailable"}),
            )
            .map_err(|error| {
                error
                    .get_ref()
                    .and_then(|error| error.downcast_ref::<StorageError>())
                    .copied()
                    .unwrap_or(StorageError::Unavailable)
            })
    }
    fn trace_counts(
        tx: &mut PgTxn<'_>,
        generation: &str,
        key: &str,
    ) -> Result<Vec<i64>, StorageError> {
        let namespace = tx.namespace.as_str().as_bytes();
        let key_digest = Sha256::digest(key.as_bytes()).to_vec();
        [
            "catalog_trace_headers",
            "catalog_trace_turns",
            "catalog_trace_edges",
            "catalog_trace_bookmarks",
        ]
        .iter()
        .map(|table| {
            tx.driver
                .query_one(
                    &format!(
                        "SELECT count(*) FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4"
                    ),
                    &[
                        &namespace,
                        &generation.as_bytes(),
                        &key_digest,
                        &key.as_bytes(),
                    ],
                )
                .map(|row| row.get(0))
                .map_err(sqlstate::error)
        })
        .collect()
    }
    fn trace_family(
        tx: &mut PgTxn<'_>,
        generation: &str,
        key: &str,
    ) -> Result<Vec<u8>, StorageError> {
        let namespace = tx.namespace.as_str().as_bytes();
        let key_digest = Sha256::digest(key.as_bytes()).to_vec();
        let header = tx
            .driver
            .query_one(
                "SELECT trace_generation,source_generation,metadata FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4",
                &[
                    &namespace,
                    &generation.as_bytes(),
                    &key_digest,
                    &key.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?;
        let header = (
            header.get::<_, Vec<u8>>(0),
            header.get::<_, Vec<u8>>(1),
            header.get::<_, Vec<u8>>(2),
        );
        let turns = tx
            .driver
            .query(
                "SELECT turn_digest,turn_id,start_id,metadata FROM semon_pg_native_catalog_trace_turns WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 ORDER BY turn_digest",
                &[
                    &namespace,
                    &generation.as_bytes(),
                    &key_digest,
                    &key.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?
            .into_iter()
            .map(|row| {
                (
                    row.get::<_, Vec<u8>>(0),
                    row.get::<_, Vec<u8>>(1),
                    row.get::<_, Option<Vec<u8>>>(2),
                    row.get::<_, Vec<u8>>(3),
                )
            })
            .collect::<Vec<_>>();
        let edges = tx
            .driver
            .query(
                "SELECT edge_digest,edge_id,metadata FROM semon_pg_native_catalog_trace_edges WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 ORDER BY edge_digest",
                &[
                    &namespace,
                    &generation.as_bytes(),
                    &key_digest,
                    &key.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?
            .into_iter()
            .map(|row| {
                (
                    row.get::<_, Vec<u8>>(0),
                    row.get::<_, Vec<u8>>(1),
                    row.get::<_, Vec<u8>>(2),
                )
            })
            .collect::<Vec<_>>();
        let bookmarks = tx
            .driver
            .query(
                "SELECT target_digest,target,edge_digest,edge_id FROM semon_pg_native_catalog_trace_bookmarks WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 ORDER BY target_digest,edge_digest",
                &[
                    &namespace,
                    &generation.as_bytes(),
                    &key_digest,
                    &key.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?
            .into_iter()
            .map(|row| {
                (
                    row.get::<_, Vec<u8>>(0),
                    row.get::<_, Vec<u8>>(1),
                    row.get::<_, Vec<u8>>(2),
                    row.get::<_, Vec<u8>>(3),
                )
            })
            .collect::<Vec<_>>();
        serde_json::to_vec(&(header, turns, edges, bookmarks))
            .map_err(|_| StorageError::ScopeChanged)
    }
    fn admit_generation(
        tx: &mut PgTxn<'_>,
        generation: &str,
        operation_id: &[u8],
    ) -> Result<(), StorageError> {
        let namespace = tx.namespace.as_str().as_bytes().to_vec();
        let payload = b"retained-details-fixture".to_vec();
        tx.driver
            .execute(
                "INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,$2,$3,$4,1)",
                &[
                    &namespace,
                    &operation_id,
                    &payload,
                    &generation.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?;
        Ok(())
    }

    let data = build_oracle(true, false);
    let (pool, _) = fixture(|_| {});
    let namespace = ns("retained-details-carry");
    let control = PgCancellation::default();
    let options = Options {
        source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap(),
        claude_home: data.root.join("claude"),
        claude_json: data.root.join("claude.json"),
        codex_home: data.root.join("codex"),
        copilot_home: data.root.join("copilot"),
        proc_root: data.root.join("proc"),
        cache: data.root.join("index.json"),
        all: true,
        ..Default::default()
    };
    let (original_generation, rows, _) =
        bind_custody_fixture(&pool, &namespace, &options, &data.rows, &data.projections);
    let original = &rows[0];
    let original_details = snapshot::with_generation(
        &pool,
        &namespace,
        &control,
        &original_generation,
        |snapshot| selected_details(snapshot, original)?.ok_or(StorageError::ScopeChanged),
    )
    .unwrap();
    let content_generation = original_details["content_generation"]
        .as_str()
        .expect("the original trace has a content generation")
        .to_owned();
    assert_eq!(original_details["live_observation"], true);
    let original_counts = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            trace_counts(tx, &original_generation, &original.key)
        })
        .unwrap();
    let original_family = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            trace_family(tx, &original_generation, &original.key)
        })
        .unwrap();
    assert_eq!(original_counts[0], 1);
    assert!(original_counts[1] + original_counts[2] + original_counts[3] > 0);

    let empty_rows = Vec::new();
    let empty_transcripts = Vec::new();
    let withdrawal = crate::slot_projection::Publication {
        rows: &empty_rows,
        transcripts: Some(&empty_transcripts),
        observations: None,
    };
    let withdrawn = vec![Sha256::digest(original.key.as_bytes()).to_vec()];
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert!(super::super::history::preserve(
            tx,
            L2,
            &original_generation,
            &withdrawal,
            &withdrawn,
        )?);
        admit_generation(tx, L2, b"retained-details-first")
    })
    .unwrap();
    let first_details = snapshot::with_generation(&pool, &namespace, &control, L2, |snapshot| {
        selected_details(snapshot, original)?.ok_or(StorageError::ScopeChanged)
    })
    .unwrap();
    assert_eq!(first_details["live_observation"], false);
    assert_eq!(first_details["content_generation"], content_generation);
    assert_eq!(
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            trace_counts(tx, L2, &original.key)
        })
        .unwrap(),
        original_counts,
        "History captures the complete prior trace family"
    );
    let first_family = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            trace_family(tx, L2, &original.key)
        })
        .unwrap();
    assert_eq!(
        first_family, original_family,
        "History capture preserves the original header and child payloads"
    );

    let later = "3333333333333333333333333333333333333333333333333333333333333333";
    let successor = "4444444444444444444444444444444444444444444444444444444444444444";
    let source_generation = crate::retention::source_generation(&original.sources).unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        super::super::carry::unmentioned(tx, later, L2, &[], &[])?;
        admit_generation(tx, later, b"retained-details-later")
    })
    .unwrap();
    let later_family = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            trace_family(tx, later, &original.key)
        })
        .unwrap();
    assert_eq!(
        later_family, original_family,
        "later History carry preserves the complete header and child payloads"
    );

    let orphan_target = "5555555555555555555555555555555555555555555555555555555555555555";
    let orphan_result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        tx.driver
            .execute(
                "DELETE FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3",
                &[&namespace.as_str().as_bytes(), &later.as_bytes(), &original.key.as_bytes()],
            )
            .map_err(sqlstate::error)?;
        super::super::carry::unmentioned(tx, orphan_target, later, &[], &[])?;
        Ok(())
    });
    assert_eq!(orphan_result, Err(StorageError::ScopeChanged));

    let successor_turn = "successor-turn";
    let successor_turn_metadata = br#"{"kind":"successor"}"#;
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let mut successor_header: serde_json::Value = serde_json::from_slice(
            &tx.driver
                .query_one(
                    "SELECT metadata FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3",
                    &[&namespace.as_str().as_bytes(), &later.as_bytes(), &original.key.as_bytes()],
                )
                .map_err(sqlstate::error)?
                .get::<_, Vec<u8>>(0),
        )
        .map_err(|_| StorageError::ScopeChanged)?;
        successor_header["successor_marker"] = serde_json::json!("staged");
        let successor_header = serde_json::to_vec(&successor_header)
            .map_err(|_| StorageError::ScopeChanged)?;
        tx.driver
            .execute(
                "DELETE FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3",
                &[&namespace.as_str().as_bytes(), &later.as_bytes(), &original.key.as_bytes()],
            )
            .map_err(sqlstate::error)?;
        tx.driver
            .execute(
                "INSERT INTO semon_pg_native_catalog_trace_headers(namespace,generation,key_digest,session_key,trace_generation,source_generation,metadata) VALUES($1,$2,sha256($3::bytea),$3,$4,$5,$6)",
                &[
                    &namespace.as_str().as_bytes(),
                    &successor.as_bytes(),
                    &original.key.as_bytes(),
                    &successor.as_bytes(),
                    &source_generation.as_bytes(),
                    &successor_header,
                ],
            )
            .map_err(sqlstate::error)?;
        tx.driver
            .execute(
                "INSERT INTO semon_pg_native_catalog_trace_turns(namespace,generation,key_digest,session_key,turn_digest,turn_id,metadata) VALUES($1,$2,sha256($3::bytea),$3,sha256($4::bytea),$4,$5)",
                &[
                    &namespace.as_str().as_bytes(),
                    &successor.as_bytes(),
                    &original.key.as_bytes(),
                    &successor_turn.as_bytes(),
                    &successor_turn_metadata.as_slice(),
                ],
            )
            .map_err(sqlstate::error)?;
        super::super::carry::unmentioned(tx, successor, later, &[], &[])?;
        admit_generation(tx, successor, b"retained-details-successor")
    })
    .unwrap();
    let mut changed = original.clone();
    changed.sources[0].prefix_sha256[0] ^= 1;
    let changed_metadata = serde_json::to_vec(&changed).unwrap();
    let parent_key = changed
        .parent
        .as_deref()
        .map(|parent| parent.as_bytes().to_vec());
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        tx.driver
            .execute(
                "INSERT INTO semon_pg_native_catalog_rows(namespace,generation,key_digest,session_key,parent_key,metadata,last_ms,harness,repo,model) VALUES($1,$2,sha256($3::bytea),$3,$4,$5,$6,$7,$8,$9)",
                &[
                    &namespace.as_str().as_bytes(),
                    &successor.as_bytes(),
                    &changed.key.as_bytes(),
                    &parent_key,
                    &changed_metadata,
                    &changed.last.unwrap_or(0),
                    &changed.harness.as_bytes(),
                    &changed.repo.as_deref().unwrap_or("").as_bytes(),
                    &changed.model.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?;
        Ok(())
    })
    .unwrap();
    snapshot::with_generation(&pool, &namespace, &control, successor, |snapshot| {
        let retained = snapshot
            .history_display_catalog(&original.key)?
            .ok_or(StorageError::ScopeChanged)?;
        assert_eq!(retained.sources, original.sources);
        let retained_details =
            selected_details(snapshot, &retained)?.ok_or(StorageError::ScopeChanged)?;
        assert_eq!(retained_details["content_generation"], content_generation);
        assert!(
            selected_details(snapshot, &changed)?.is_none(),
            "the changed Current cohort cannot borrow History's trace"
        );
        Ok(())
    })
    .unwrap();
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(
            trace_counts(tx, successor, &original.key)?,
            vec![1, 1, 0, 0],
            "the successor keeps its complete trace family without History children"
        );
        let header = tx
            .driver
            .query_one(
                "SELECT metadata,trace_generation,source_generation FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3",
                &[&namespace.as_str().as_bytes(), &successor.as_bytes(), &original.key.as_bytes()],
            )
            .map_err(sqlstate::error)?;
        let metadata: serde_json::Value =
            serde_json::from_slice(&header.get::<_, Vec<u8>>(0))
                .map_err(|_| StorageError::ScopeChanged)?;
        assert_eq!(metadata["successor_marker"], "staged");
        assert_eq!(
            header.get::<_, Vec<u8>>(1),
            successor.as_bytes().to_vec()
        );
        assert_eq!(
            header.get::<_, Vec<u8>>(2),
            source_generation.as_bytes().to_vec()
        );
        let turns = tx
            .driver
            .query(
                "SELECT turn_id,metadata FROM semon_pg_native_catalog_trace_turns WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 ORDER BY turn_digest",
                &[&namespace.as_str().as_bytes(), &successor.as_bytes(), &original.key.as_bytes()],
            )
            .map_err(sqlstate::error)?;
        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0].get::<_, Vec<u8>>(0),
            successor_turn.as_bytes().to_vec()
        );
        assert_eq!(
            turns[0].get::<_, Vec<u8>>(1),
            successor_turn_metadata.to_vec()
        );
        Ok(())
    })
    .unwrap();
    fs::remove_dir_all(data.root).unwrap();
}

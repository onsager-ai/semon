use super::super::snapshot;
use super::*;
use crate::storage::{
    catalog::{CatalogObservationOwner as Owner, CatalogSnapshot},
    native_events::NativeEventStore,
    query::{QueryRecipeLoadError as Error, QueryRecipeResidence},
};

// A completed semantic publication schedules bounded cleanup. A Written
// begin may only advance that cleanup; establish an actual stage before the
// shared publication fixture starts consuming batches.
fn begin_fixture_stage(cache: &mut crate::events::EventCache, options: &Options, paths: &[String]) {
    let runtime = crate::native_evidence::query_runtime_generation(options).unwrap();
    for _ in 0..256 {
        assert_eq!(
            cache
                .begin_semantic_stage_with_journal(paths, &runtime, true)
                .unwrap(),
            crate::events::Outcome::Written
        );
        if cache.semantic_batch(64).unwrap().is_some() {
            return;
        }
    }
    panic!("semantic cleanup did not establish the successor stage");
}

fn advance_imported_fixture(
    config: &crate::storage::StoreConfig,
    options: &Options,
) -> (String, Vec<CatalogRow>) {
    let old = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/sanitized/catalog.jsonl".into(),
    };
    // Claude session identity follows the filename; relocate the source while
    // preserving that identity for the native successor.
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/relocated/catalog.jsonl".into(),
    };
    let old_path = old.full_path(options);
    let path = input.full_path(options);
    let mut body = fs::read_to_string(&old_path).unwrap();
    body.push_str(&format!("{}\n", serde_json::json!({"type":"user","sessionId":"catalog","uuid":"query-recipe-import-next","parentUuid":"recipe-import-successor","timestamp":"2026-10-01T00:02:00Z","cwd":"/sanitized","message":{"role":"user","content":"recipe imported History successor"}})));
    fs::remove_file(&old_path).unwrap();
    let mut cache = crate::storage::transfer::reported::tests::cache(config);
    cache
        .retire_scoped_sources(
            options,
            std::slice::from_ref(&old),
            &std::collections::BTreeSet::new(),
            &crate::facts::MachineFacts::Local,
        )
        .unwrap();
    begin_fixture_stage(
        &mut cache,
        options,
        &[old_path.to_str().unwrap().to_owned()],
    );
    assert!(
        crate::storage::transfer::reported::tests::finish_catalog(
            &mut cache,
            options,
            std::slice::from_ref(&old),
        )
        .is_empty()
    );
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, body).unwrap();
    cache.ingest(&"9".repeat(64), &path, "claude").unwrap();
    cache.flush_cold_writes().unwrap();
    begin_fixture_stage(&mut cache, options, &[path.to_str().unwrap().to_owned()]);
    let rows = crate::storage::transfer::reported::tests::finish_catalog(
        &mut cache,
        options,
        std::slice::from_ref(&input),
    );
    let generation = cache.session_catalog_generation().unwrap();
    (generation, rows)
}

#[test]
fn pg_query_recipe_loader_native_exact_owner_grant_and_payload() {
    let data = build_oracle(true, false);
    let (pool, _) = fixture(|_| {});
    let namespace = ns("query-recipe-native");
    let control = PgCancellation::default();
    let config = crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        },
        namespace: namespace.clone(),
        expected_stream_epoch: Some("c".repeat(64)),
    };
    config.create_native_store().unwrap();
    let options = Options {
        source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64))
            .unwrap()
            .with_store(config)
            .unwrap(),
        claude_home: data.root.join("claude"),
        claude_json: data.root.join("claude.json"),
        codex_home: data.root.join("codex"),
        copilot_home: data.root.join("copilot"),
        proc_root: data.root.join("proc"),
        cache: data.root.join("index.json"),
        all: true,
        ..Default::default()
    };
    // Both ordinary current producers still publish inline metadata with a
    // NULL external cell. Authenticate that representation before exercising
    // the inactive detached writer below.
    let sqlite_config = crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Sqlite {
            database: crate::events::EventCache::path(&options.cache),
        },
        namespace: ns("catalog-effects-oracle"),
        expected_stream_epoch: Some("c".repeat(64)),
    };
    let sqlite_reader = crate::storage::NativeReaderBinding::Semantic(
        crate::events::store::configured::NativeOwnership::restore(&sqlite_config).unwrap(),
    );
    let mut db = sqlite_reader.connection().unwrap();
    let tx = db.transaction().unwrap();
    let metadata: String = tx.query_row(
        "SELECT metadata FROM session_catalog WHERE namespace=semon_namespace() AND session_key=?1",
        [&data.rows[0].key], |record| record.get(0),
    ).unwrap();
    let mut inline: CatalogRow = serde_json::from_str(&metadata).unwrap();
    assert!(!crate::query_projection::SelectedRecipe::detached(&inline));
    assert_eq!(
        crate::storage::sqlite::proof_generations::attach_query_recipe(
            &tx,
            &sqlite_reader,
            Owner::Catalog,
            &mut inline,
        )
        .unwrap(),
        0
    );
    assert!(inline.query_recipe.is_none());
    assert_eq!(serde_json::to_string(&inline).unwrap(), metadata);
    tx.commit().unwrap();
    drop(db);

    let inline_namespace = ns("query-recipe-inline-native");
    let inline_config = crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        },
        namespace: inline_namespace.clone(),
        expected_stream_epoch: Some("c".repeat(64)),
    };
    inline_config.create_native_store().unwrap();
    let mut inline_options = options.clone();
    inline_options.source =
        crate::storage::HostSource::new(inline_namespace.clone(), "c".repeat(64))
            .unwrap()
            .with_store(inline_config)
            .unwrap();
    let mut inline_cache =
        crate::events::EventCache::open_scoped(&inline_options.cache, &inline_options.source)
            .unwrap();
    let path = &data.rows[0].sources[0].path;
    inline_cache
        .ingest(&"b".repeat(64), path, "claude")
        .unwrap();
    let inline_input = crate::native_evidence::catalog_input(path, &inline_options).unwrap();
    let inline_prepared = crate::model::prepare_sources(
        &inline_options,
        &mut inline_cache,
        &[inline_input],
        1790812802000,
    )
    .unwrap();
    let (outcome, inline_generation) = inline_cache
        .publish_partial_catalog_fenced(&inline_prepared)
        .unwrap();
    assert_eq!(outcome, crate::events::Outcome::Written);
    let inline_generation = inline_generation.unwrap();
    let inline_key = &inline_prepared.rows[0].key;
    snapshot::with_admitted_snapshot(
        &pool,
        &inline_namespace,
        &control,
        &inline_generation,
        |view| {
            let mut selected = view.catalog(inline_key, false)?.unwrap();
            let metadata = serde_json::to_string(&selected).unwrap();
            assert!(!crate::query_projection::SelectedRecipe::detached(
                &selected
            ));
            assert_eq!(view.attach_query_recipe(&mut selected, false)?, 0);
            assert!(selected.query_recipe.is_none());
            assert_eq!(serde_json::to_string(&selected).unwrap(), metadata);
            let paths = vec![path.to_str().unwrap().to_owned()];
            let mut tx = view.tx.lock().map_err(|_| StorageError::Unavailable)?;
            assert_eq!(
                super::super::super::source_lifecycle::component_paths(&mut tx, &paths, &[], &[])?,
                Some(paths)
            );
            Ok(())
        },
    )
    .unwrap();
    for unexpected in ["{".to_owned(), "x".repeat(1048576)] {
        let result = pool.transaction(&inline_namespace, TxnKind::Write, &control, |tx| {
            tx.driver.execute("UPDATE semon_pg_native_catalog_rows SET query_recipe=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&inline_namespace.as_str().as_bytes(), &inline_generation.as_bytes(), &unexpected.as_bytes()]).map_err(sqlstate::error)?;
            snapshot::in_transaction(tx, &inline_generation, |view| {
                let mut selected = view.catalog(inline_key, false)?.unwrap();
                assert!(view.attach_query_recipe(&mut selected, false).is_err());
                Ok(())
            })?;
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
    }
    drop(inline_cache);
    let mut cache =
        crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let path = &data.rows[0].sources[0].path;
    cache.ingest(&"a".repeat(64), path, "claude").unwrap();
    let input = crate::native_evidence::catalog_input(path, &options).unwrap();
    let mut prepared =
        crate::model::prepare_sources(&options, &mut cache, &[input], 1790812802000).unwrap();
    let expected = prepared.rows[0].query_recipe.clone().unwrap();
    let compact = crate::query_projection::SelectedRecipe::prepare(&prepared.rows[0]).unwrap();
    prepared.rows[0].query = serde_json::from_str::<CatalogRow>(&compact.metadata)
        .unwrap()
        .query;
    let (outcome, generation) = cache.publish_partial_catalog_fenced(&prepared).unwrap();
    assert_eq!(outcome, crate::events::Outcome::Written);
    let generation = generation.unwrap();
    let row = &prepared.rows[0];
    snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |view| {
        let selected = view.catalog(&row.key, false)?.unwrap();
        let observation = view
            .query_recipe_observation(Owner::Catalog, &selected)
            .unwrap();
        assert_eq!(
            view.load_query_recipe(&observation).unwrap(),
            Some(expected.clone())
        );
        Ok(())
    })
    .unwrap();
    // Exercise the missing-cell refusal after the producer stored its payload.
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute("UPDATE semon_pg_native_catalog_rows SET query_recipe=NULL WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3", &[&namespace.as_str().as_bytes(),&generation.as_bytes(),&row.key.as_bytes()]).map_err(sqlstate::error)?, 1);
        Ok(())
    }).unwrap();
    let token =
        snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |view| {
            // Retained display does not move a physical Current selection to History.
            let mut display = row.clone();
            display.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
            let token = view
                .query_recipe_observation(Owner::Catalog, &display)
                .unwrap();
            assert!(matches!(
                token.residence,
                QueryRecipeResidence::NativeCatalog { .. }
            ));
            assert!(matches!(
                view.load_query_recipe(&token),
                Err(Error::MissingPayload)
            ));
            assert!(view.attach_query_recipe(&mut display, false).is_err());
            assert!(view.attach_query_recipe(&mut display, true).is_err());
            assert!(view.query_recipe_observation(Owner::History, row).is_err());
            Ok(token)
        })
        .unwrap();
    let payload = compact.query_recipe.unwrap();
    // The only physical seam supplies the nullable payload, after normal
    // publication authenticated the exact compact metadata and native receipt.
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute("UPDATE semon_pg_native_catalog_rows SET query_recipe=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3 AND metadata=$5", &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &row.key.as_bytes(), &payload.as_bytes(), &compact.metadata.as_bytes()]).map_err(sqlstate::error)?, 1);
        Ok(())
    }).unwrap();
    snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |view| {
        assert_eq!(
            view.load_query_recipe(&token).unwrap(),
            Some(expected.clone())
        );
        assert_eq!(view.selected_query_recipe(&token).unwrap(), expected);
        let mut selected = view.catalog(&row.key, false)?.unwrap();
        let metadata = serde_json::to_string(&selected).unwrap();
        assert_eq!(
            view.attach_query_recipe(&mut selected, false)?,
            payload.len()
        );
        assert_eq!(selected.query_recipe.as_ref(), Some(&expected));
        assert_eq!(serde_json::to_string(&selected).unwrap(), metadata);
        // A declared reference cannot evade authentication by also carrying
        // inline-looking values, even when a typed payload is already attached.
        let mut mixed = selected.clone();
        let mixed_query = mixed.query.as_mut().unwrap();
        mixed_query["selected"]["session"] = expected.session.clone();
        mixed_query["selected"]["turns"] = serde_json::json!(expected.turns);
        mixed_query["selected"]["edges"] = serde_json::json!(expected.edges);
        assert!(view.attach_query_recipe(&mut mixed, false).is_err());
        assert!(
            crate::storage::query::history_window(&selected, None)
                .unwrap()
                .member
        );
        Ok(())
    })
    .unwrap();
    let port = snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
        captured: false,
    };
    assert_eq!(
        crate::storage::query::QueryCatalogStore::selected_recipe(&port, &token).unwrap(),
        Some(expected.clone())
    );
    // Replacing the actual stored reference with a complete inline-shaped
    // object preserves source/summary fields but invalidates its receipt.
    let mut replaced = row.clone();
    replaced.query.as_mut().unwrap()["selected"] = serde_json::to_value(&expected).unwrap();
    assert!(!crate::query_projection::SelectedRecipe::detached(
        &replaced
    ));
    let replaced_metadata = serde_json::to_vec(&replaced).unwrap();
    for payload in [None, Some(b"{".to_vec()), Some(vec![b'x'; 1048576])] {
        let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver.execute("UPDATE semon_pg_native_catalog_rows SET metadata=$3,query_recipe=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &replaced_metadata, &payload]).map_err(sqlstate::error)?;
            snapshot::in_transaction(tx, &generation, |view| {
                let mut selected = view.catalog(&row.key, false)?.unwrap();
                assert_eq!(selected.sources, row.sources);
                assert_eq!(selected.query.as_ref().unwrap()["rows"], row.query.as_ref().unwrap()["rows"]);
                assert!(!crate::query_projection::SelectedRecipe::detached(&selected));
                assert!(view.query_recipe_observation(Owner::Catalog, &selected).is_err());
                assert!(view.attach_query_recipe(&mut selected, false).is_err());
                Ok(())
            })?;
            let paths = vec![path.to_str().unwrap().to_owned()];
            assert!(super::super::super::source_lifecycle::component_paths(tx, &paths, &[], &[]).is_err());
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
    }
    for fault in [
        "corrupt",
        "digest",
        "bound",
        "grant",
        "admission",
        "stream",
        "owner",
        "namespace",
    ] {
        let mut checked = false;
        let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            match fault {
                "corrupt" | "digest" | "bound" => {
                    let bad = match fault {
                        "corrupt" => "{".to_owned(),
                        "digest" => serde_json::to_string(&crate::query_projection::SelectedRecipe { session: serde_json::json!({}), turns: vec![], edges: vec![] }).unwrap(),
                        _ => "x".repeat(1048576),
                    };
                    tx.driver.execute("UPDATE semon_pg_native_catalog_rows SET query_recipe=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&ns, &generation.as_bytes(), &bad.as_bytes()]).map_err(sqlstate::error)?;
                }
                "grant" => {
                    let key = format!("catalog-native-observation:{}", crate::storage::proof_codec::digest(token.metadata.as_bytes()));
                    super::super::super::native_keys::delete_meta(tx, key.as_bytes())?;
                }
                "admission" => { tx.driver.execute("UPDATE semon_pg_native_semantic_runtime SET admission=admission+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?; }
                "stream" => { tx.driver.execute("UPDATE semon_pg_native_semantic_runtime SET stream_epoch=$2 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns, &"d".repeat(64).as_bytes()]).map_err(sqlstate::error)?; }
                "owner" => { tx.driver.execute("DELETE FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&ns, &generation.as_bytes()]).map_err(sqlstate::error)?; }
                "namespace" => { super::super::super::native_keys::put_meta(tx, b"source-unenrolled", b"1")?; }
                _ => unreachable!(),
            }
            snapshot::in_transaction(tx, &generation, |view| {
                let error = view.load_query_recipe(&token).unwrap_err();
                match fault {
                    "corrupt" | "digest" => assert!(matches!(error, Error::CorruptPayload), "{fault}: {error}"),
                    "bound" => assert!(matches!(error, Error::Storage(StorageError::HistoryRetentionBounds)), "{error}"),
                    _ => assert!(matches!(error, Error::ObservationChanged), "{fault}: {error}"),
                }
                let mut selected = row.clone();
                assert!(view.attach_query_recipe(&mut selected, false).is_err(), "{fault}");
                Ok(())
            })?;
            checked = true;
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
        assert!(checked, "{fault}");
    }
    // A successor pins the old physical snapshot; a fresh History selection
    // acquires its own custody reference instead of borrowing Current's grant.
    let successor =
        snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |held| {
            let next =
                super::native_history::replace_source(&options, row, "query-recipe-successor");
            assert_eq!(
                held.load_query_recipe(&token).unwrap(),
                Some(expected.clone())
            );
            let mut selected = held.catalog(&row.key, false)?.unwrap();
            held.attach_query_recipe(&mut selected, false)?;
            assert_eq!(selected.query_recipe.as_ref(), Some(&expected));
            snapshot::with_admitted_snapshot(&pool, &namespace, &control, &next, |view| {
                let history = view.catalog(&row.key, true)?.unwrap();
                let history_token = view
                    .query_recipe_observation(Owner::History, &history)
                    .unwrap();
                assert!(matches!(
                    history_token.residence,
                    QueryRecipeResidence::NativeHistory { .. }
                ));
                assert_eq!(
                    view.load_query_recipe(&history_token).unwrap(),
                    Some(expected.clone())
                );
                assert!(matches!(
                    view.load_query_recipe(&token),
                    Err(Error::ObservationChanged)
                ));
                Ok(())
            })?;
            Ok(next)
        })
        .unwrap();
    // History carries the exact original cell; remove it for the same refusal.
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute("UPDATE semon_pg_native_catalog_history_headers h SET query_recipe=NULL FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.session_key=$3 AND h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key", &[&namespace.as_str().as_bytes(),&successor.as_bytes(),&row.key.as_bytes()]).map_err(sqlstate::error)?, 1);
        Ok(())
    }).unwrap();
    let history_token =
        snapshot::with_admitted_snapshot(&pool, &namespace, &control, &successor, |view| {
            let history = view.catalog(&row.key, true)?.unwrap();
            let token = view
                .query_recipe_observation(Owner::History, &history)
                .unwrap();
            assert!(matches!(
                view.load_query_recipe(&token),
                Err(Error::MissingPayload)
            ));
            Ok(token)
        })
        .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute("UPDATE semon_pg_native_catalog_history_headers h SET query_recipe=$4 FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.session_key=$3 AND h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key AND h.metadata=$5", &[&namespace.as_str().as_bytes(), &successor.as_bytes(), &row.key.as_bytes(), &payload.as_bytes(), &history_token.metadata.as_bytes()]).map_err(sqlstate::error)?, 1);
        Ok(())
    }).unwrap();
    let history_owner =
        snapshot::with_admitted_snapshot(&pool, &namespace, &control, &successor, |view| {
            assert_eq!(
                view.load_query_recipe(&history_token).unwrap(),
                Some(expected.clone())
            );
            Ok(view.owner(&row.key)?.unwrap().generation)
        })
        .unwrap();
    let key =
        super::super::observations::history_key(&history_owner, history_token.metadata.as_bytes());
    let mut checked = false;
    let result = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        super::super::super::native_keys::delete_meta(tx, key.as_bytes())?;
        snapshot::in_transaction(tx, &successor, |view| {
            assert!(matches!(
                view.load_query_recipe(&history_token),
                Err(Error::ObservationChanged)
            ));
            Ok(())
        })?;
        checked = true;
        Err::<(), _>(StorageError::Conflict)
    });
    assert_eq!(result, Err(StorageError::Conflict));
    assert!(checked);
    fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_query_recipe_loader_imported_keeps_original_binding_and_rechecks_receipt() {
    use crate::storage::sqlite::proof_generations::with_prepared_query_recipe_for_test;
    use crate::storage::transfer::reported::tests::{
        apply, root, seed_catalog_input, sqlite, stage,
    };
    use rusqlite::OptionalExtension;
    let root = root();
    let source = sqlite(&root, "query-recipe-import");
    let (options, input) = seed_catalog_input(&source, &root);
    // Change representation before the real producer records its exact model
    // witness. Authority is still authored only by ordinary publication.
    use std::io::Write;
    let path = input.full_path(&options);
    let path = &path;
    writeln!(fs::OpenOptions::new().append(true).open(path).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"catalog","uuid":"recipe-import-successor","parentUuid":"u129","timestamp":"2026-10-01T00:01:00Z","cwd":"/sanitized","message":{"role":"user","content":"recipe import successor"}})).unwrap();
    let mut cache = crate::storage::transfer::reported::tests::cache(&source);
    cache.ingest(&"8".repeat(64), path, "claude").unwrap();
    cache.flush_cold_writes().unwrap();
    let paths = vec![path.to_str().unwrap().to_owned()];
    let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    assert_eq!(
        cache
            .begin_semantic_stage_with_journal(&paths, &runtime, true)
            .unwrap(),
        crate::events::Outcome::Written
    );
    let input = crate::native_evidence::catalog_input(path, &options).unwrap();
    let mut published = None;
    for _ in 0..256 {
        let batch = cache.semantic_batch(64).unwrap().unwrap();
        if batch.paths.is_empty() {
            continue;
        }
        let mut model = crate::model::prepare_semantic_batch(
            &options,
            &mut cache,
            std::slice::from_ref(&input),
            &batch,
            1_790_000_000_000,
        )
        .unwrap();
        let mut payloads = Vec::new();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            for row in &mut model.prepared_mut_for_test().rows {
                let expected = row.query_recipe.clone().unwrap();
                let compact = crate::query_projection::SelectedRecipe::prepare(row).unwrap();
                row.query = serde_json::from_str::<CatalogRow>(&compact.metadata)
                    .unwrap()
                    .query;
                payloads.push((row.clone(), compact, expected));
            }
        }
        assert_eq!(
            cache
                .record_semantic_batch(&batch, model.prepared())
                .unwrap(),
            crate::events::Outcome::Written
        );
        if batch.phase != crate::model::relation_stage::Phase::Publication {
            continue;
        }
        let mut complete = false;
        for _ in 0..256 {
            // Author the payload at its immutable INSERT from this exact
            // prepared model; all ordinary publication checks still run.
            let result = with_prepared_query_recipe_for_test(|| {
                cache.publish_semantic_catalog(&batch, model.prepared())
            })
            .unwrap();
            if result.0 == crate::events::Outcome::Written {
                assert!(result.1.is_some());
                complete = true;
                break;
            }
            assert_eq!(result, (crate::events::Outcome::Deferred, None));
        }
        assert!(complete);
        cache.finish_semantic_stage(batch.epoch).unwrap();
        assert!(!payloads.is_empty());
        published = Some(payloads);
        break;
    }
    let payloads = published.expect("compact immutable publication completed");
    let (row, compact, expected) = &payloads[0];
    let payload = compact.query_recipe.as_ref().unwrap();
    let reader = crate::storage::NativeReaderBinding::Semantic(
        crate::events::store::configured::NativeOwnership::restore(&source).unwrap(),
    );
    let mut db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&source).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let tx = db.transaction().unwrap();
    let binding = reader.runtime_binding_at(&tx).unwrap();
    let source_observation = crate::storage::sqlite::proof_generations::query_recipe_observation(
        &tx,
        &binding,
        Owner::Catalog,
        row,
    )
    .unwrap();
    let mut selected = row.clone();
    assert_eq!(
        crate::storage::sqlite::proof_generations::attach_query_recipe(
            &tx,
            &reader,
            Owner::Catalog,
            &mut selected,
        )
        .unwrap(),
        payload.len(),
    );
    assert_eq!(selected.query_recipe.as_ref(), Some(expected));
    assert_eq!(serde_json::to_string(&selected).unwrap(), compact.metadata);
    assert!(
        crate::storage::sqlite::proof_generations::attach_query_recipe(
            &tx,
            &reader,
            Owner::History,
            &mut selected,
        )
        .is_err()
    );
    let mut replaced = row.clone();
    replaced.query.as_mut().unwrap()["selected"] = serde_json::to_value(expected).unwrap();
    assert!(!crate::query_projection::SelectedRecipe::detached(
        &replaced
    ));
    let replaced_metadata = serde_json::to_string(&replaced).unwrap();
    for payload in [None, Some("{".to_owned()), Some("x".repeat(1048576))] {
        tx.execute_batch("SAVEPOINT inline_shape_corruption")
            .unwrap();
        // Corrupt only this fixture's mutable cell, including a pair that a
        // normal SQLite writer rejects at its CHECK constraint.
        tx.pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        tx.execute(
            "UPDATE main.session_catalog SET metadata=?3,query_recipe=?4 WHERE namespace=?1 AND session_key=?2",
            rusqlite::params![source.namespace.as_str(), row.key, replaced_metadata, payload],
        ).unwrap();
        let mut selected: CatalogRow = serde_json::from_str(&replaced_metadata).unwrap();
        assert_eq!(selected.sources, row.sources);
        assert_eq!(
            selected.query.as_ref().unwrap()["rows"],
            row.query.as_ref().unwrap()["rows"]
        );
        assert!(
            crate::storage::sqlite::proof_generations::query_recipe_observation(
                &tx,
                &binding,
                Owner::Catalog,
                &selected,
            )
            .is_err()
        );
        assert!(
            crate::storage::sqlite::proof_generations::attach_query_recipe(
                &tx,
                &reader,
                Owner::Catalog,
                &mut selected,
            )
            .is_err()
        );
        tx.execute_batch("ROLLBACK TO inline_shape_corruption; RELEASE inline_shape_corruption")
            .unwrap();
        tx.pragma_update(None, "ignore_check_constraints", false)
            .unwrap();
    }
    let source_proof = match source_observation.residence {
        QueryRecipeResidence::Immutable { generation, .. } => generation,
        _ => panic!("ordinary semantic publication must own the immutable recipe"),
    };
    let current_payload: Option<String> = tx
        .query_row(
            "SELECT query_recipe FROM main.session_catalog WHERE namespace=?1 AND session_key=?2 AND metadata=?3",
            rusqlite::params![source.namespace.as_str(), row.key, compact.metadata],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        current_payload.is_none(),
        "the production Current writer stays inactive"
    );
    let immutable_payload: Option<String> = tx
        .query_row(
            "SELECT query_recipe FROM main.storage_proof_generations WHERE namespace=?1 AND session_key=?2 AND metadata=?3 AND id=?4",
            rusqlite::params![source.namespace.as_str(), row.key, compact.metadata, source_proof.0],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(immutable_payload.as_ref(), Some(payload));
    let original_history: Option<String> = tx
        .query_row(
            "SELECT metadata FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
            rusqlite::params![source.namespace.as_str(), row.key],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert!(
        original_history.is_none(),
        "compact publication is the first original owner"
    );
    tx.commit().unwrap();
    drop(db);
    drop(cache);
    // The held snapshot spans the ordinary retirement and successor stages.
    let (pool, _) = fixture(|limits| {
        limits.idle_in_tx_timeout = Duration::from_secs(30);
        limits.txn_deadline = Duration::from_secs(30);
    });
    let control = PgCancellation::default();
    let config = crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        },
        namespace: source.namespace.clone(),
        expected_stream_epoch: source.expected_stream_epoch.clone(),
    };
    config.create_native_store().unwrap();
    let manifest = stage(&source, &config, &[193; 32]);
    apply(&config, &manifest);
    let generation = config
        .transfer_inbox()
        .admit_catalog_from(&source.transfer_inbox(), &manifest, &options)
        .unwrap();
    let namespace = &config.namespace;
    let token = snapshot::with_admitted_snapshot(&pool, namespace, &control, &generation, |view| {
        assert!(view.owner(&row.key)?.is_none());
        let mut selected = view.catalog(&row.key, false)?.unwrap();
        let token = view
            .query_recipe_observation(Owner::Catalog, &selected)
            .unwrap();
        assert!(matches!(
            &token.residence,
            QueryRecipeResidence::Immutable { generation, .. } if generation == &source_proof
        ));
        assert_eq!(token.original, binding);
        assert_eq!(Some(token.destination.clone()), view.runtime_binding());
        assert_eq!(token.original.namespace, source.namespace);
        assert_eq!(
            token.original.stream_epoch,
            source.expected_stream_epoch.as_ref().unwrap().as_str()
        );
        assert_eq!(
            view.load_query_recipe(&token).unwrap().as_ref(),
            Some(expected)
        );
        assert_eq!(&view.selected_query_recipe(&token).unwrap(), expected);
        view.attach_query_recipe(&mut selected, false)?;
        assert_eq!(selected.query_recipe.as_ref(), Some(expected));
        assert_eq!(serde_json::to_string(&selected).unwrap(), compact.metadata);
        let paths = vec![selected.sources[0].path.to_str().unwrap().to_owned()];
        let discovered = {
            let mut tx = view.tx.lock().map_err(|_| StorageError::Unavailable)?;
            super::super::super::source_lifecycle::component_paths(&mut tx, &paths, &[], &[])?
        };
        assert_eq!(discovered, Some(paths));
        assert_eq!(
            token.original, binding,
            "decoding preserves producer coordinates"
        );
        Ok(token)
    })
    .unwrap();
    let mut pg_options = options.clone();
    pg_options.source = crate::storage::HostSource::new(
        namespace.clone(),
        config.expected_stream_epoch.clone().unwrap(),
    )
    .unwrap()
    .with_store(config.clone())
    .unwrap();
    let port = snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: pg_options.clone(),
        captured: false,
    };
    assert_eq!(
        crate::storage::query::QueryCatalogStore::selected_recipe(&port, &token)
            .unwrap()
            .as_ref(),
        Some(expected)
    );
    for sql in [
        "UPDATE semon_pg_native_selected_generations SET query_recipe=NULL WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "UPDATE semon_pg_native_selected_generations SET query_recipe='{'::bytea WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "UPDATE semon_pg_native_selected_generations SET prefix=repeat('0',64)::bytea WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "UPDATE semon_pg_native_selected_presentations SET metadata='{}'::bytea WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        "DELETE FROM semon_pg_native_selected_refs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND kind='current'::bytea",
        "DELETE FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND position('native_transfer_v1:catalog-owner:'::bytea in key)=1",
        "UPDATE semon_pg_native_semantic_runtime SET admission=admission+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
    ] {
        let mut checked = false;
        let result = pool.transaction(namespace, TxnKind::Write, &control, |tx| {
            tx.driver
                .execute(sql, &[&namespace.as_str().as_bytes()])
                .map_err(sqlstate::error)?;
            snapshot::in_transaction(tx, &generation, |view| {
                let error = view.load_query_recipe(&token).unwrap_err();
                if sql.contains("query_recipe=NULL") {
                    assert!(matches!(error, Error::MissingPayload));
                } else if sql.contains("query_recipe='{'") {
                    assert!(matches!(error, Error::CorruptPayload));
                } else {
                    assert!(matches!(error, Error::ObservationChanged));
                }
                let mut selected = row.clone();
                assert!(view.attach_query_recipe(&mut selected, false).is_err());
                Ok(())
            })?;
            let paths = vec![row.sources[0].path.to_str().unwrap().to_owned()];
            assert!(
                super::super::super::source_lifecycle::component_paths(tx, &paths, &[], &[])
                    .is_err()
            );
            checked = true;
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
        assert!(checked);
    }
    // A genuine native successor hands the imported original to immutable
    // History, preserving its existing producer proof and detached payload.
    snapshot::with_admitted_snapshot(&pool, namespace, &control, &generation, |held| {
        let mut selected = held.catalog(&row.key, false)?.unwrap();
        let (next, continued) = advance_imported_fixture(&config, &pg_options);
        assert_eq!(continued.len(), 1);
        assert_eq!(continued[0].key, selected.key);
        assert_ne!(continued[0].sources, selected.sources);

        assert_eq!(
            held.load_query_recipe(&token).unwrap().as_ref(),
            Some(expected)
        );
        held.attach_query_recipe(&mut selected, false)?;
        assert_eq!(selected.query_recipe.as_ref(), Some(expected));
        snapshot::with_admitted_snapshot(&pool, namespace, &control, &next, |view| {
            let mut history = view.catalog(&row.key, true)?.unwrap();
            let history_token = view
                .query_recipe_observation(Owner::History, &history)
                .unwrap();
            match (&token.residence, &history_token.residence) {
                (
                    QueryRecipeResidence::Immutable {
                        generation: original,
                        publication: original_publication,
                        ..
                    },
                    QueryRecipeResidence::Immutable {
                        owner: crate::storage::query::QueryRecipeOwner::History,
                        generation: retained,
                        publication: retained_publication,
                    },
                ) => {
                    assert_eq!(original, retained);
                    assert_eq!(original_publication, retained_publication);
                }
                _ => panic!("imported original lost its immutable History residence"),
            }
            assert_eq!(history_token.original, binding);
            assert_eq!(
                view.load_query_recipe(&history_token).unwrap().as_ref(),
                Some(expected)
            );
            view.attach_query_recipe(&mut history, true)?;
            assert_eq!(history.query_recipe.as_ref(), Some(expected));
            Ok(())
        })?;
        Ok(())
    })
    .unwrap();
    fs::remove_dir_all(root).unwrap();
}

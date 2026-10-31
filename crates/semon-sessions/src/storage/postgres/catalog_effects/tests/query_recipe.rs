use super::super::snapshot;
use super::*;
use crate::storage::{
    catalog::{CatalogObservationOwner as Owner, CatalogSnapshot},
    native_events::NativeEventStore,
    query::{QueryRecipeLoadError as Error, QueryRecipeResidence},
};

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
            let next = super::native_history::advance(&options, row, "query-recipe-successor");
            assert_eq!(
                held.load_query_recipe(&token).unwrap(),
                Some(expected.clone())
            );
            snapshot::with_admitted_snapshot(&pool, &namespace, &control, &next, |view| {
                let history = view.catalog(&row.key, true)?.unwrap();
                let history_token = view
                    .query_recipe_observation(Owner::History, &history)
                    .unwrap();
                assert!(matches!(
                    history_token.residence,
                    QueryRecipeResidence::NativeHistory { .. }
                ));
                assert!(matches!(
                    view.load_query_recipe(&token),
                    Err(Error::ObservationChanged)
                ));
                Ok(())
            })?;
            Ok(next)
        })
        .unwrap();
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
    use crate::storage::transfer::reported::tests::{apply, root, seed_catalog, sqlite, stage};
    let root = root();
    let source = sqlite(&root, "query-recipe-import");
    let (options, initial) = seed_catalog(&source, &root);
    // Change representation before the real producer records its exact model
    // witness. Authority is still authored only by ordinary publication.
    use std::io::Write;
    let path = &initial[0].sources[0].path;
    writeln!(fs::OpenOptions::new().append(true).open(path).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"catalog","uuid":"recipe-import-successor","parentUuid":"u129","timestamp":"2026-10-01T00:01:00Z","cwd":"/sanitized","message":{"role":"user","content":"recipe import successor"}})).unwrap();
    let mut cache = crate::storage::transfer::reported::tests::cache(&source);
    cache.ingest(&"8".repeat(64), path, "claude").unwrap();
    cache.flush_cold_writes().unwrap();
    let paths = vec![path.to_str().unwrap().to_owned()];
    assert_eq!(
        cache
            .begin_semantic_stage_with_journal(&paths, "query-recipe-import", true)
            .unwrap(),
        crate::events::Outcome::Written
    );
    let input = crate::native_evidence::catalog_input(path, &options).unwrap();
    let mut published = None;
    for _ in 0..256 {
        let Some(batch) = cache.semantic_batch(64).unwrap() else {
            // Completing the seeded publication leaves bounded semantic GC.
            // begin advances those ordinary pages before creating the next
            // stage, so Written alone does not yet promise a batch.
            assert_eq!(
                cache
                    .begin_semantic_stage_with_journal(&paths, "query-recipe-import", true)
                    .unwrap(),
                crate::events::Outcome::Written
            );
            continue;
        };
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
            let result = cache
                .publish_semantic_catalog(&batch, model.prepared())
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
    crate::storage::sqlite::proof_generations::query_recipe_observation(
        &tx,
        &binding,
        Owner::Catalog,
        row,
    )
    .unwrap();
    assert_eq!(tx.execute("UPDATE session_catalog SET query_recipe=?1 WHERE namespace=?2 AND session_key=?3 AND metadata=?4", rusqlite::params![payload, source.namespace.as_str(), row.key, compact.metadata]).unwrap(), 1);
    assert!(tx.execute("UPDATE storage_proof_generations SET query_recipe=?1 WHERE namespace=?2 AND session_key=?3 AND metadata=?4", rusqlite::params![payload, source.namespace.as_str(), row.key, compact.metadata]).unwrap() > 0);
    tx.commit().unwrap();
    drop(db);
    drop(cache);
    let (pool, _) = fixture(|_| {});
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
        let selected = view.catalog(&row.key, false)?.unwrap();
        let token = view
            .query_recipe_observation(Owner::Catalog, &selected)
            .unwrap();
        assert!(matches!(
            token.residence,
            QueryRecipeResidence::Immutable { .. }
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
                Ok(())
            })?;
            checked = true;
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
        assert!(checked);
    }
    // A genuine native successor hands the imported original to immutable
    // History, preserving its existing producer proof and detached payload.
    snapshot::with_admitted_snapshot(&pool, namespace, &control, &generation, |held| {
        let selected = held.catalog(&row.key, false)?.unwrap();
        let continued =
            crate::storage::transfer::reported::tests::advance_catalog(&config, &pg_options);
        assert_eq!(continued.len(), 1);
        assert_eq!(continued[0].key, selected.key);
        assert_ne!(continued[0].sources, selected.sources);
        let next = crate::events::EventCache::open_configured(&config)
            .unwrap()
            .session_catalog_generation()
            .unwrap();
        assert_eq!(
            held.load_query_recipe(&token).unwrap().as_ref(),
            Some(expected)
        );
        snapshot::with_admitted_snapshot(&pool, namespace, &control, &next, |view| {
            let history = view.catalog(&row.key, true)?.unwrap();
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
            Ok(())
        })?;
        Ok(())
    })
    .unwrap();
    fs::remove_dir_all(root).unwrap();
}

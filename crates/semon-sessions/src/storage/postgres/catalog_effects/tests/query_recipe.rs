use super::*;
use super::super::snapshot;
use crate::storage::{catalog::{CatalogObservationOwner as Owner, CatalogSnapshot}, query::{QueryRecipeLoadError as Error, QueryRecipeResidence}, native_events::NativeEventStore};

#[test]
fn pg_query_recipe_loader_native_exact_owner_grant_and_payload() {
    let data = build_oracle(true, false);
    let (pool, _) = fixture(|_| {});
    let namespace = ns("query-recipe-native");
    let control = PgCancellation::default();
    let config = crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Postgres { pool: pool.clone(), control: control.clone() },
        namespace: namespace.clone(), expected_stream_epoch: Some("c".repeat(64)),
    };
    config.create_native_store().unwrap();
    let options = Options {
        source: crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap().with_store(config).unwrap(),
        claude_home: data.root.join("claude"), claude_json: data.root.join("claude.json"),
        codex_home: data.root.join("codex"), copilot_home: data.root.join("copilot"),
        proc_root: data.root.join("proc"), cache: data.root.join("index.json"), all: true, ..Default::default()
    };
    let mut cache = crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let path = &data.rows[0].sources[0].path;
    cache.ingest(&"a".repeat(64), path, "claude").unwrap();
    let input = crate::native_evidence::catalog_input(path, &options).unwrap();
    let mut prepared = crate::model::prepare_sources(&options, &mut cache, &[input], 1790812802000).unwrap();
    let expected = prepared.rows[0].query_recipe.clone().unwrap();
    let compact = crate::query_projection::SelectedRecipe::prepare(&prepared.rows[0]).unwrap();
    prepared.rows[0].query = serde_json::from_str::<CatalogRow>(&compact.metadata).unwrap().query;
    let (outcome, generation) = cache.publish_partial_catalog_fenced(&prepared).unwrap();
    assert_eq!(outcome, crate::events::Outcome::Written);
    let generation = generation.unwrap();
    let row = &prepared.rows[0];
    let token = snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |view| {
        // Retained display does not move a physical Current selection to History.
        let mut display = row.clone();
        display.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        let token = view.query_recipe_observation(Owner::Catalog, &display).unwrap();
        assert!(matches!(token.residence, QueryRecipeResidence::NativeCatalog { .. }));
        assert!(matches!(view.load_query_recipe(&token), Err(Error::MissingPayload)));
        assert!(view.query_recipe_observation(Owner::History, row).is_err());
        Ok(token)
    }).unwrap();
    let payload = compact.query_recipe.unwrap();
    // The only physical seam supplies the nullable payload, after normal
    // publication authenticated the exact compact metadata and native receipt.
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute("UPDATE semon_pg_native_catalog_rows SET query_recipe=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND session_key=$3 AND metadata=$5", &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &row.key.as_bytes(), &payload.as_bytes(), &compact.metadata.as_bytes()]).map_err(sqlstate::error)?, 1);
        Ok(())
    }).unwrap();
    snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |view| {
        assert_eq!(view.load_query_recipe(&token).unwrap(), Some(expected.clone()));
        assert_eq!(view.selected_query_recipe(&token).unwrap(), expected);
        Ok(())
    }).unwrap();
    for fault in ["corrupt", "digest", "bound", "grant", "admission", "stream", "owner", "namespace"] {
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
    snapshot::with_admitted_snapshot(&pool, &namespace, &control, &generation, |held| {
        let next = super::native_history::advance(&options, row, "query-recipe-successor");
        assert_eq!(held.load_query_recipe(&token).unwrap(), Some(expected.clone()));
        snapshot::with_admitted_snapshot(&pool, &namespace, &control, &next, |view| {
            let history = view.catalog(&row.key, true)?.unwrap();
            let history_token = view.query_recipe_observation(Owner::History, &history).unwrap();
            assert!(matches!(history_token.residence, QueryRecipeResidence::NativeHistory { .. }));
            assert!(matches!(view.load_query_recipe(&token), Err(Error::ObservationChanged)));
            Ok(())
        })?;
        Ok(())
    }).unwrap();
    fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_query_recipe_loader_imported_keeps_original_binding_and_rechecks_receipt() {
    use crate::storage::transfer::reported::tests::{root, sqlite, seed_catalog, stage, apply};
    let root = root();
    let source = sqlite(&root, "query-recipe-import");
    let (options, rows) = seed_catalog(&source, &root);
    let (pool, _) = fixture(|_| {});
    let control = PgCancellation::default();
    let config = crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Postgres { pool: pool.clone(), control: control.clone() },
        namespace: source.namespace.clone(), expected_stream_epoch: source.expected_stream_epoch.clone(),
    };
    config.create_native_store().unwrap();
    let manifest = stage(&source, &config, &[193; 32]);
    apply(&config, &manifest);
    let generation = config.transfer_inbox().admit_catalog_from(&source.transfer_inbox(), &manifest, &options).unwrap();
    let namespace = &config.namespace;
    let token = snapshot::with_admitted_snapshot(&pool, namespace, &control, &generation, |view| {
        let selected = view.catalog(&rows[0].key, false)?.unwrap();
        let token = view.query_recipe_observation(Owner::Catalog, &selected).unwrap();
        assert!(matches!(token.residence, QueryRecipeResidence::Immutable { .. }));
        assert_eq!(token.original.namespace, source.namespace);
        assert_eq!(token.original.stream_epoch, source.expected_stream_epoch.as_ref().unwrap().as_str());
        assert!(matches!(view.load_query_recipe(&token), Err(Error::MissingPayload)), "inactive producer has no detached payload");
        Ok(token)
    }).unwrap();
    for sql in [
        "DELETE FROM semon_pg_native_selected_refs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND kind='current'::bytea",
        "DELETE FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND position('native_transfer_v1:catalog-owner:'::bytea in key)=1",
        "UPDATE semon_pg_native_semantic_runtime SET admission=admission+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
    ] {
        let mut checked = false;
        let result = pool.transaction(namespace, TxnKind::Write, &control, |tx| {
            tx.driver.execute(sql, &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
            snapshot::in_transaction(tx, &generation, |view| {
                assert!(matches!(view.load_query_recipe(&token), Err(Error::ObservationChanged)));
                Ok(())
            })?;
            checked = true;
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(result, Err(StorageError::Conflict));
        assert!(checked);
    }
    fs::remove_dir_all(root).unwrap();
}

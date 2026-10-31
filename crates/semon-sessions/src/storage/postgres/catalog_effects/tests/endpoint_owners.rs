use super::super::{observations, snapshot};
use super::*;
use crate::storage::{
    catalog::CatalogSnapshot,
    transfer::reported::tests::{
        advance_catalog, advance_catalog_with_retirement_observer, apply, root, seed_catalog,
        sqlite, stage,
    },
};

#[test]
fn pg_selected_endpoint_requires_its_physical_owner_authority() {
    let root = root();
    let source = sqlite(&root, "selected-endpoint-physical-owner");
    let (mut options, original) = seed_catalog(&source, &root);
    let continued = advance_catalog(&source, &options);
    let (pool, _) = fixture(|limits| {
        limits.idle_timeout = Duration::from_secs(1);
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
    let manifest = stage(&source, &config, &[177; 32]);
    apply(&config, &manifest);
    let generation = config
        .transfer_inbox()
        .admit_catalog_from(&source.transfer_inbox(), &manifest, &options)
        .unwrap();
    let namespace = &config.namespace;
    options.source = crate::storage::HostSource::new(
        namespace.clone(),
        config.expected_stream_epoch.clone().unwrap(),
    )
    .unwrap()
    .with_store(config.clone())
    .unwrap();

    for (key, history) in [(&original[0].key, true), (&continued[0].key, false)] {
        let candidate =
            snapshot::with_generation(&pool, namespace, &control, &generation, |snapshot| {
                let row = snapshot.catalog(key, history)?.unwrap();
                snapshot::query::qualify_context_candidate(snapshot, &options, &row, history)?;
                Ok(row)
            })
            .unwrap();
        let (own, opposite) = if history {
            (b"history".as_slice(), b"current".as_slice())
        } else {
            (b"current".as_slice(), b"history".as_slice())
        };
        let ids = pool
            .transaction(namespace, TxnKind::Write, &control, |tx| {
                let ns = namespace.as_str().as_bytes();
                let ids = tx.driver.query("SELECT id FROM semon_pg_native_selected_refs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND session_key=$2 AND kind=$3 LIMIT 257", &[&ns,&candidate.key.as_bytes(),&own]).map_err(sqlstate::error)?;
                let ids = ids
                    .into_iter()
                    .map(|row| row.get::<_, Vec<u8>>(0))
                    .collect::<Vec<_>>();
                assert!(!ids.is_empty() && ids.len() <= 256);
                tx.driver.execute("UPDATE semon_pg_native_selected_refs SET kind=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND session_key=$2 AND kind=$3", &[&ns,&candidate.key.as_bytes(),&own,&opposite]).map_err(sqlstate::error)?;
                Ok(ids)
            })
            .unwrap();
        snapshot::with_generation(
            &pool,
            namespace,
            &control,
            &generation,
            |snapshot| {
                // Opposite-owner authority still proves this exact publication.
                assert!(snapshot.row_runtime_binding(&candidate).unwrap().is_some());
                assert!(matches!(
                    snapshot::query::qualify_context_candidate(
                        snapshot, &options, &candidate, history,
                    ),
                    Err(StorageError::ScopeChanged)
                ));
                Ok(())
            },
        )
        .unwrap();
        pool.transaction(namespace, TxnKind::Write, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            for id in &ids {
                tx.driver.execute("UPDATE semon_pg_native_selected_refs SET kind=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND session_key=$2 AND id=$3 AND kind=$5", &[&ns,&candidate.key.as_bytes(),&id,&own,&opposite]).map_err(sqlstate::error)?;
            }
            Ok(())
        })
        .unwrap();
        snapshot::with_generation(&pool, namespace, &control, &generation, |snapshot| {
            snapshot::query::qualify_context_candidate(snapshot, &options, &candidate, history)
        })
        .unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pg_imported_current_handoff_preserves_its_proof_and_old_current_reads() {
    let root = root();
    let source = sqlite(&root, "imported-current-native-history");
    let (mut options, original) = seed_catalog(&source, &root);
    let (pool, _) = fixture(|limits| {
        limits.idle_timeout = Duration::from_secs(1);
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
    let manifest = stage(&source, &config, &[178; 32]);
    apply(&config, &manifest);
    let generation = config
        .transfer_inbox()
        .admit_catalog_from(&source.transfer_inbox(), &manifest, &options)
        .unwrap();
    let namespace = &config.namespace;
    options.source = crate::storage::HostSource::new(
        namespace.clone(),
        config.expected_stream_epoch.clone().unwrap(),
    )
    .unwrap()
    .with_store(config.clone())
    .unwrap();

    let current = &original[0];
    let original_id =
        snapshot::with_generation(&pool, namespace, &control, &generation, |snapshot| {
            assert!(snapshot.catalog(&current.key, true)?.is_none());
            snapshot::query::qualify_context_candidate(snapshot, &options, current, false)?;
            let (_, _, origin) = snapshot
                .row_observation(
                    current,
                    Some(crate::storage::catalog::CatalogObservationOwner::Catalog),
                )
                .map_err(|_| StorageError::ScopeChanged)?
                .unwrap();
            let observations::Origin::Imported(id) = origin else {
                panic!("genuine transferred Current proof")
            };
            Ok(id)
        })
        .unwrap();
    let host = super::businessflow::body_host(&options, std::slice::from_ref(current));
    let old_store = snapshot::query::QueryCatalog {
        captured: true,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
    };
    let old_source_path = current.sources[0].path.to_string_lossy().into_owned();
    // The ordinary producer retires this imported standalone source before
    // ingesting the same native identity from its successor path.
    let continued = advance_catalog_with_retirement_observer(&config, &options, || {
        let (head, pending, mutation, invalidation) = pool
            .transaction(namespace, TxnKind::ReadSnapshot, &control, |tx| {
                let ns = namespace.as_str().as_bytes();
                let row = tx
                    .driver
                    .query_one(
                        "SELECT catalog_generation,semantic_pending,EXISTS(SELECT 1 FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2),EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed') FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
                        &[&ns, &old_source_path.as_bytes()],
                    )
                    .map_err(sqlstate::error)?;
                Ok((
                    row.get::<_, Option<String>>(0),
                    row.get::<_, bool>(1),
                    row.get::<_, bool>(2),
                    row.get::<_, bool>(3),
                ))
            })
            .unwrap();
        assert_eq!(
            head.as_deref(),
            Some(generation.as_str()),
            "deferred withdrawal preserves the imported Current head"
        );
        assert!(
            pending,
            "deferred withdrawal marks semantic catch-up pending"
        );
        assert!(
            mutation,
            "deferred withdrawal records the exact source intent"
        );
        assert!(invalidation, "deferred withdrawal records source removal");
    });
    assert_eq!(continued.len(), 1);
    assert_eq!(continued[0].key, current.key);
    assert_ne!(continued[0].sources, current.sources);
    let successor = crate::events::EventCache::open_configured(&config)
        .unwrap()
        .session_catalog_generation()
        .unwrap();
    snapshot::with_generation(&pool, namespace, &control, &successor, |snapshot| {
        let history = snapshot.catalog(&current.key, true)?.unwrap();
        assert_eq!(history.sources, current.sources);
        snapshot::query::qualify_context_candidate(snapshot, &options, &history, true)?;
        let (_, _, origin) = snapshot
            .row_observation(
                &history,
                Some(crate::storage::catalog::CatalogObservationOwner::History),
            )
            .map_err(|_| StorageError::ScopeChanged)?
            .unwrap();
        let observations::Origin::Imported(id) = origin else {
            panic!("History preserves original imported proof")
        };
        assert_eq!(id.0, original_id.0);
        Ok(())
    })
    .unwrap();
    // The new association must leave old Current snapshots and recipes readable.
    snapshot::with_generation(&pool, namespace, &control, &generation, |snapshot| {
        snapshot::query::qualify_context_candidate(snapshot, &options, current, false)?;
        assert!(snapshot.selected(&current.key, false, 0, 1)?.is_some());
        Ok(())
    })
    .unwrap();
    let body = crate::Query::new(options.clone())
        .call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":current.key,"limit":1}),
            1790812860000,
            &old_store,
            &host,
            None,
        )
        .unwrap();
    assert!(body.to_string().contains("sanitized"));
    fs::remove_dir_all(root).unwrap();
}

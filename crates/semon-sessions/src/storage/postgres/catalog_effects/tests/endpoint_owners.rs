use super::*;
use crate::storage::{
    catalog::CatalogSnapshot,
    transfer::reported::tests::{advance_catalog, apply, root, seed_catalog, sqlite, stage},
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
        let candidate = super::super::snapshot::with_generation(
            &pool,
            namespace,
            &control,
            &generation,
            |snapshot| {
                let row = snapshot.catalog(key, history)?.unwrap();
                super::super::snapshot::query::qualify_context_candidate(
                    snapshot, &options, &row, history,
                )?;
                Ok(row)
            },
        )
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
        super::super::snapshot::with_generation(
            &pool,
            namespace,
            &control,
            &generation,
            |snapshot| {
                // Opposite-owner authority still proves this exact publication.
                assert!(snapshot.row_runtime_binding(&candidate).unwrap().is_some());
                assert!(matches!(
                    super::super::snapshot::query::qualify_context_candidate(
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
        super::super::snapshot::with_generation(
            &pool,
            namespace,
            &control,
            &generation,
            |snapshot| {
                super::super::snapshot::query::qualify_context_candidate(
                    snapshot, &options, &candidate, history,
                )
            },
        )
        .unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

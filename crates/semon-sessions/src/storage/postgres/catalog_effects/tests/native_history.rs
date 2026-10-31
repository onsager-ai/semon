use super::super::super::native_keys;
use super::super::{observations, snapshot};
use super::*;
use crate::events::{EventCache, Outcome};
use crate::storage::{catalog::CatalogSnapshot, native_events::NativeEventStore};
use sha2::Digest;
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
    let (original_generation, rows) =
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
                snapshot.owner(&original.key)?.unwrap(),
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
            snapshot.owner(&original.key)?.as_deref(),
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

//! Deferred withdrawal is durable authority only for its exact absent stage claim.
use super::*;
use crate::events::{EventCache, Outcome};
use crate::storage::native_events::NativeEventStore;

#[test]
fn pg_deferred_withdrawal_continuation_exact_stage_and_full_value_replay() {
    qualify(false);
}

#[test]
fn pg_deferred_withdrawal_continuation_refuses_stale_mismatched_and_raw_authority() {
    qualify(true);
}

fn qualify(refusals: bool) {
    let data = build_oracle(true, false);
    let (pool, _) = fixture(|_| {});
    let namespace = ns(if refusals {
        "deferred-refusal"
    } else {
        "deferred-exact"
    });
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
    let config = options
        .source
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    config.create_native_store().unwrap();
    let (mut store, _) =
        super::super::super::index_store::open(pool.clone(), namespace.clone(), control.clone())
            .unwrap();
    options.source = options.source.with_store(config).unwrap();
    let a = options.claude_home.join("projects/project/one.jsonl");
    let b = options.claude_home.join("projects/project/two.jsonl");
    fs::write(
        &b,
        format!(
            "{}\n",
            serde_json::json!({
                "type":"user", "sessionId":"two", "session_id":"one", "uuid":"u2",
                "cwd":"/synthetic/project", "timestamp":"2026-10-01T00:00:02Z",
                "message":{"role":"user","content":"sanitized composite continuation"}
            })
        ),
    )
    .unwrap();
    let paths = vec![
        a.to_str().unwrap().to_owned(),
        b.to_str().unwrap().to_owned(),
    ];
    let inputs = paths
        .iter()
        .map(|path| {
            crate::native_evidence::catalog_input(std::path::Path::new(path), &options).unwrap()
        })
        .collect::<Vec<_>>();
    let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    for (at, path) in paths.iter().enumerate() {
        cache
            .ingest(
                &format!("{:064x}", at + 1),
                std::path::Path::new(path),
                "claude",
            )
            .unwrap();
    }
    let prepared =
        crate::model::prepare_sources(&options, &mut cache, &inputs, 1790812803000).unwrap();
    assert_eq!(prepared.rows.len(), 1);
    assert_eq!(prepared.rows[0].sources.len(), 2);
    let (outcome, generation) = cache.publish_partial_catalog_fenced(&prepared).unwrap();
    assert_eq!(outcome, Outcome::Written);
    let generation = generation.unwrap();
    let removal = vec![(paths[1].clone(), store.ledger(&paths[1]).unwrap())];
    let retained = data.root.join("retained-two.jsonl");
    fs::rename(&b, &retained).unwrap();
    assert_eq!(store.remove_files(&removal).unwrap(), Outcome::Written);
    assert_eq!(
        store.session_catalog_generation().unwrap().as_deref(),
        Some(generation.as_str())
    );
    assert_eq!(store.remove_files(&removal).unwrap(), Outcome::Written);
    assert!(
        cache
            .capture_source_continuations(&options, Some(&paths), false)
            .is_err(),
        "the pending intent grants no authority without a Publication stage"
    );
    let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    cache.begin_semantic_stage(&paths, &runtime).unwrap();
    let (batch, model) = loop {
        let batch = cache.semantic_batch(64).unwrap().unwrap();
        let inputs = batch
            .paths
            .iter()
            .map(|path| {
                crate::native_evidence::catalog_input(std::path::Path::new(path), &options).unwrap()
            })
            .collect::<Vec<_>>();
        let model = cache
            .prepare_semantic_model_batch(&options, &inputs, &batch, 1790812803000)
            .unwrap()
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            break (batch, model);
        }
    };
    let captures = cache
        .capture_source_continuations(&options, Some(&paths), false)
        .unwrap();
    let removed = captures
        .iter()
        .find(|capture| capture.path == paths[1])
        .unwrap();
    assert!(removed.pending_mutation);
    assert!(removed.physical.is_none());
    assert_eq!(removed.ledger, removal[0].1);
    let authority = removed.deferred_withdrawal.as_ref().unwrap();
    assert_eq!(authority.stage_epoch, batch.epoch);
    assert_eq!(authority.source_revision, batch.source_revision);
    if refusals {
        for fault in [
            "claim",
            "stage-revision",
            "intent",
            "generic",
            "retired",
            "raw",
            "present",
        ] {
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                let ns = namespace.as_str().as_bytes();
                let path = paths[1].as_bytes();
                match fault {
                    "claim" => { tx.driver.execute("UPDATE semon_pg_source_invalidations SET revision=revision+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path]).map_err(sqlstate::error)?; }
                    "stage-revision" => { tx.driver.execute("UPDATE semon_pg_source_state SET revision=revision+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?; }
                    "intent" | "generic" => {
                        let mut mismatched = removal.clone();
                        mismatched[0].1.as_mut().unwrap().offset -= 1;
                        let request = encoded(&("source-withdrawal-v1", &mismatched))?;
                        let evidence = if fault == "intent" {
                            encoded(&("withdrawal-deferred-v1", &request))?
                        } else {
                            b"{}".to_vec()
                        };
                        tx.driver.execute("UPDATE semon_pg_native_source_mutations SET evidence=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path, &evidence]).map_err(sqlstate::error)?;
                    }
                    "retired" => super::super::super::native_keys::put_meta(tx, format!("source-retired:{}", paths[1]).as_bytes(), b"null")?,
                    "raw" => super::super::super::native_keys::put_meta(tx, format!("raw-write-inflight:{}", paths[1]).as_bytes(), b"{}")?,
                    "present" => (),
                    _ => unreachable!(),
                }
                Ok(())
            }).unwrap();
            if fault == "present" {
                fs::rename(&retained, &b).unwrap();
            }
            assert!(
                cache
                    .capture_source_continuations(&options, Some(&paths), false)
                    .is_err(),
                "capture must refuse {fault}"
            );
            let refused = cache.publish_semantic_catalog(&batch, model.prepared());
            assert!(
                refused.is_err() || matches!(refused, Ok((Outcome::Conflict, None))),
                "publication must refuse {fault}: {refused:?}"
            );
            assert_eq!(
                store.session_catalog_generation().unwrap().as_deref(),
                Some(generation.as_str())
            );
            if fault == "present" {
                fs::rename(&b, &retained).unwrap();
            }
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                let ns = namespace.as_str().as_bytes();
                let path = paths[1].as_bytes();
                tx.driver.execute("UPDATE semon_pg_source_state SET revision=$2 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns, &batch.source_revision]).map_err(sqlstate::error)?;
                tx.driver.execute("UPDATE semon_pg_source_invalidations SET revision=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path, &removed.claim.unwrap()]).map_err(sqlstate::error)?;
                tx.driver.execute("UPDATE semon_pg_native_source_mutations SET evidence=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path, &authority.intent]).map_err(sqlstate::error)?;
                super::super::super::native_keys::put_meta(tx, format!("source-retired:{}", paths[1]).as_bytes(), &authority.retired)?;
                super::super::super::native_keys::delete_meta(tx, format!("raw-write-inflight:{}", paths[1]).as_bytes())?;
                Ok(())
            }).unwrap();
        }
    }
    let ack = loop {
        let ack = cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        if ack.0 != Outcome::Deferred {
            break ack;
        }
        assert_eq!(
            store.session_catalog_generation().unwrap().as_deref(),
            Some(generation.as_str())
        );
    };
    assert_eq!(ack.0, Outcome::Written);
    assert_ne!(ack.1.as_ref(), Some(&generation));
    assert_eq!(
        cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap(),
        ack
    );
    cache.finish_semantic_stage(batch.epoch).unwrap();
    assert_eq!(store.remove_files(&removal).unwrap(), Outcome::Written);
    let mut changed = removal.clone();
    changed[0].1.as_mut().unwrap().offset -= 1;
    assert_eq!(store.remove_files(&changed).unwrap(), Outcome::Conflict);
    // Replay remains independent of the now-published head and live absence.
    fs::rename(&retained, &b).unwrap();
    assert_eq!(store.remove_files(&removal).unwrap(), Outcome::Written);
    assert_eq!(
        cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap(),
        ack
    );
    super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        ack.1.as_deref().unwrap(),
        |snapshot| {
            let current = snapshot.catalog("one", false)?.unwrap();
            assert_eq!(current.sources.len(), 1);
            assert_eq!(current.sources[0].path, a);
            assert!(
                snapshot.owner("one")?.is_some(),
                "withdrawn source has retained custody"
            );
            Ok(())
        },
    )
    .unwrap();
}

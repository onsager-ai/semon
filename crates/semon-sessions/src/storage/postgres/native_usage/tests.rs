use super::*;
use crate::{
    events::{
        Ledger, Stat,
        store::native_usage::{self as shared, algorithm},
    },
    storage::postgres::{
        PgCancellation, PgPool, Reconciliation,
        tests::{fixture, identity, limits, ns},
        txn::TxnKind,
    },
};

fn ledger(offset: u64) -> Ledger {
    Ledger {
        stat: Stat {
            dev: 1,
            ino: 2,
            size: offset,
            modified_ns: 3,
            changed_ns: Some(4),
        },
        offset,
        prefix: [5; 32],
        tail: [6; 32],
    }
}

fn rows(tx: &mut PgTxn<'_>) -> Result<Vec<Vec<String>>, StorageError> {
    let mut result = Vec::new();
    for query in [
        "SELECT source_id::text,encode(source_path,'hex'),convert_from(receipt,'UTF8'),checkpoint::text,parser_version::text,usage_ready::int::text FROM semon_pg_native_usage_sources ORDER BY semon_pg_native_usage_sources.source_id",
        "SELECT source_id::text,native_offset::text,encode(identity,'hex'),encode(payload,'hex'),encode(record,'hex'),encode(outcome,'hex'),encode(model_outcome,'hex') FROM semon_pg_native_usage_facts ORDER BY semon_pg_native_usage_facts.source_id,semon_pg_native_usage_facts.native_offset",
        "SELECT source_id::text,encode(scope,'hex'),encode(identity,'hex'),native_offset::text FROM semon_pg_native_usage_identities ORDER BY semon_pg_native_usage_identities.source_id,scope,identity",
        "SELECT source_id::text,encode(kind,'hex'),encode(dimension,'hex'),encode(value,'hex') FROM semon_pg_native_usage_dimensions ORDER BY semon_pg_native_usage_dimensions.source_id,kind,dimension",
    ] {
        for row in tx.driver.query(query, &[]).map_err(sqlstate::error)? {
            result.push((0..row.len()).map(|i| row.get(i)).collect());
        }
    }
    Ok(result)
}

#[test]
fn pg_native_usage_shared_parser_generation_dimensions_replay_restart_and_rollback() {
    let root = std::env::temp_dir().join(format!(
        "pg95-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("codex.jsonl");
    let usage = |turn: &str, input: u64| serde_json::json!({"type":"token_usage_record","timestamp":"2026-09-29T00:00:01Z","payload":{"thread_id":"thread\0🦀","session_id":"session\0🦀","turn_id":turn,"response_id":format!("response-{turn}"),"usage":{"input_tokens":input,"output_tokens":3,"total_tokens":input+3,"cached_input_tokens":1,"cache_write_input_tokens":2,"reasoning_output_tokens":0}}});
    let body = format!(
        "{}\n{}\n{}\n{}\n",
        serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5"}}),
        usage("one", 11),
        usage("one", 11),
        usage("one", 12)
    );
    std::fs::write(&path, &body).unwrap();
    let index = shared::parser_fixture(&path, "codex");
    assert_eq!(index.native_usage_observations.as_ref().unwrap().len(), 3);
    let mut oracle = rusqlite::Connection::open_in_memory().unwrap();
    for ddl in shared::schema(false) {
        oracle.execute_batch(ddl).unwrap();
    }
    let source_path = "same/path\0🦀";
    let first = ledger(body.len() as u64);
    assert_eq!(
        shared::sqlite_fixture(&mut oracle, source_path, None, &first, false, &index),
        1
    );
    let expected = sqlite_rows(&oracle);
    assert!(
        expected
            .iter()
            .any(|r| r.iter().any(|v| v == "6163636570746564")),
        "accepted fact required"
    );
    assert!(
        expected
            .iter()
            .any(|r| r.iter().any(|v| v == "6475706c6963617465")),
        "duplicate fact required"
    );
    assert!(
        expected
            .iter()
            .any(|r| r.iter().any(|v| v == "636f6e666c696374")),
        "conflict fact required"
    );
    assert!(
        expected
            .iter()
            .any(|r| r.iter().any(|v| v == "636f73745f646179")),
        "day dimensions required"
    );
    let appended_body = format!("{body}{}\n", usage("two", 17));
    std::fs::write(&path, &appended_body).unwrap();
    let mut appended = index.clone();
    shared::checkpoint(&mut appended);
    shared::parser_append_fixture(&path, "codex", &mut appended, first.offset);
    let second = ledger(appended_body.len() as u64);
    assert_eq!(
        shared::sqlite_fixture(
            &mut oracle,
            source_path,
            Some(&first),
            &second,
            true,
            &appended
        ),
        1
    );
    let appended_expected = sqlite_rows(&oracle);
    // The SQLite oracle is fully materialized before the PG operation phase.
    drop(oracle);
    let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
    let namespace = ns("usage\0🦀");
    let control = PgCancellation::default();
    let apply = |tx: &mut PgTxn<'_>| {
        tx.driver.execute("INSERT INTO semon_pg_native_tree_repairs(namespace,source_path,receipt,source_id,cursor,projection_id,last_model,tree_root,state,last_attempt) VALUES($1,$2,$3,NULL,0,NULL,NULL,NULL,$4,0) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET state=excluded.state", &[&tx.namespace.as_str().as_bytes(),&source_path.as_bytes(),&b"old".as_slice(),&b"blocked_evidence".as_slice()]).map_err(sqlstate::error)?;
        let mut effects = Usage::new(tx)?;
        let id = algorithm::begin(&mut effects, source_path, None, &first, false)?;
        algorithm::write(&mut effects, source_path, &first, &index)?;
        assert_eq!(
            text(
                tx.driver
                    .query_one("SELECT state FROM semon_pg_native_tree_repairs", &[])
                    .map_err(sqlstate::error)?
                    .get(0)
            )?,
            "ready"
        );
        Ok(serde_json::json!({"source":id}))
    };
    // Fail after each actual physical effect, including between the source and
    // current writes, all facts/identities, dimensions, repair unblock and ready.
    let mut reached_end = false;
    for point in 1..=40 {
        FAIL_AFTER.with(|flag| flag.set(Some(point)));
        let failed = pool.apply_operation(&namespace, &identity(), &control, |tx| {
            apply(tx)?;
            reached_end = true;
            Err(StorageError::Unavailable)
        });
        FAIL_AFTER.with(|flag| flag.set(None));
        assert_eq!(failed, Err(StorageError::Unavailable));
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            assert!(rows(tx)?.is_empty());
            for table in [
                "semon_pg_native_usage_current",
                "semon_pg_native_tree_repairs",
                "semon_pg_source_state",
                "semon_pg_operations",
            ] {
                assert_eq!(
                    tx.driver
                        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
                        .map_err(sqlstate::error)?
                        .get::<_, i64>(0),
                    0
                );
            }
            Ok(())
        })
        .unwrap();
        if reached_end {
            break;
        }
    }
    assert!(reached_end, "all physical fault points must execute");
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, |tx| {
            apply(tx)?;
            Err(StorageError::Unavailable)
        }),
        Err(StorageError::Unavailable)
    );
    assert_eq!(
        pool.reconcile(&namespace, &identity(), &control),
        Ok(Reconciliation::Absent)
    );
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert!(rows(tx)?.is_empty());
        Ok(())
    })
    .unwrap();
    crate::storage::postgres::tests::LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, apply),
        Err(StorageError::CommitUncertain)
    );
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, |_| panic!(
            "receipt replay must have no effects"
        )),
        Ok(serde_json::json!({"source":1}))
    );
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(rows(tx)?, expected);
        let before = rows(tx)?;
        let mut effects = Usage::new(tx)?;
        assert_eq!(
            algorithm::begin(&mut effects, source_path, Some(&first), &first, false)?,
            1
        );
        algorithm::write(&mut effects, source_path, &first, &index)?;
        assert_eq!(rows(tx)?, before);
        Ok(())
    })
    .unwrap();
    drop(pool);
    let pool = PgPool::open(factory, limits()).unwrap();
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(rows(tx)?, expected);
        Ok(())
    })
    .unwrap();
    std::fs::write(
        &path,
        body.replace("\"input_tokens\":11", "\"input_tokens\":13"),
    )
    .unwrap();
    let conflicting = shared::parser_fixture(&path, "codex");
    assert_eq!(
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            let mut effects = Usage::new(tx)?;
            algorithm::begin(&mut effects, source_path, Some(&first), &first, true)?;
            algorithm::write(&mut effects, source_path, &first, &conflicting)
        }),
        Err(StorageError::Conflict)
    );
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(rows(tx)?, expected);
        Ok(())
    })
    .unwrap();
    pool.transaction(&ns("foreign\0🦀"), TxnKind::Write, &control, |tx| {
        assert!(rows(tx)?.is_empty());
        apply(tx)?;
        Ok(())
    })
    .unwrap();
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(rows(tx)?, expected);
        Ok(())
    })
    .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let mut effects = Usage::new(tx)?;
        assert_eq!(
            algorithm::begin(&mut effects, source_path, Some(&first), &second, true)?,
            1
        );
        algorithm::write(&mut effects, source_path, &second, &appended)?;
        assert_eq!(rows(tx)?, appended_expected);
        Ok(())
    })
    .unwrap();
    // Replacement creates a generation; append reuses it, with full u64 checkpoint.
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let mut effects = Usage::new(tx)?;
        let next = ledger(u64::MAX);
        assert_eq!(
            algorithm::begin(&mut effects, source_path, Some(&second), &next, false)?,
            2
        );
        algorithm::write(&mut effects, source_path, &next, &index)?;
        assert_eq!(
            algorithm::begin(&mut effects, source_path, Some(&next), &next, true)?,
            2
        );
        Ok(())
    })
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn sqlite_rows(db: &rusqlite::Connection) -> Vec<Vec<String>> {
    let mut result = Vec::new();
    for query in [
        "SELECT cast(source_id AS TEXT),lower(hex(source_path)),receipt,cast(checkpoint AS TEXT),cast(parser_version AS TEXT),cast(usage_ready AS TEXT) FROM native_usage_sources ORDER BY source_id",
        "SELECT cast(source_id AS TEXT),cast(native_offset AS TEXT),lower(hex(identity)),lower(hex(payload)),lower(hex(record)),lower(hex(outcome)),lower(hex(model_outcome)) FROM native_usage_facts ORDER BY source_id,native_offset",
        "SELECT cast(source_id AS TEXT),lower(hex(scope)),lower(hex(identity)),cast(native_offset AS TEXT) FROM native_usage_identities ORDER BY source_id,scope,identity",
        "SELECT cast(source_id AS TEXT),lower(hex(kind)),lower(hex(dimension)),lower(hex(value)) FROM native_usage_dimensions ORDER BY source_id,kind,dimension",
    ] {
        let mut statement = db.prepare(query).unwrap();
        let columns = statement.column_count();
        result.extend(
            statement
                .query_map([], |r| {
                    (0..columns)
                        .map(|i| r.get(i))
                        .collect::<rusqlite::Result<Vec<String>>>()
                })
                .unwrap()
                .map(Result::unwrap),
        );
    }
    result
}

fn parsed(harness: &str, body: &str) -> crate::events::FileIndex {
    let path = std::env::temp_dir().join(format!(
        "pg95-parser-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, body).unwrap();
    let index = shared::parser_fixture(&path, harness);
    std::fs::remove_file(path).unwrap();
    index
}

#[test]
fn pg_native_usage_real_parser_unavailable_model_and_unsigned_counters() {
    let usage = serde_json::json!({"type":"token_usage_record","timestamp":"2026-09-29T00:00:01Z","payload":{"thread_id":"t\0🦀","session_id":"s","turn_id":"turn","response_id":"response","usage":{"input_tokens":u64::MAX-3,"output_tokens":3,"total_tokens":u64::MAX,"cached_input_tokens":1,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}});
    let context = serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5"}});
    let body = format!("{usage}\n{context}\n{usage}\n{usage}\n");
    let index = parsed("codex", &body);
    let end = ledger(body.len() as u64);
    let mut oracle = rusqlite::Connection::open_in_memory().unwrap();
    for ddl in shared::schema(false) {
        oracle.execute_batch(ddl).unwrap();
    }
    shared::sqlite_fixture(&mut oracle, "path\0🦀", None, &end, false, &index);
    let expected = sqlite_rows(&oracle);
    drop(oracle);
    let (pool, _) = fixture(|_| {});
    pool.transaction(&ns("outcomes\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
        let mut effects = Usage::new(tx)?;
        algorithm::begin(&mut effects,"path\0🦀",None,&end,false)?;
        algorithm::write(&mut effects,"path\0🦀",&end,&index)?;
        assert_eq!(rows(tx)?,expected);
        let outcomes = tx.driver.query("SELECT outcome,model_outcome FROM semon_pg_native_usage_facts ORDER BY native_offset",&[]).map_err(sqlstate::error)?.iter().map(|r|Ok((text(r.get(0))?,text(r.get(1))?))).collect::<Result<Vec<_>,StorageError>>()?;
        assert_eq!(outcomes,vec![("accepted".into(),"unavailable".into()),("conflict".into(),"accepted".into()),("conflict".into(),"duplicate".into())]);
        Ok(())
    }).unwrap();
}

#[test]
fn pg_native_usage_claude_copilot_do_not_create_native_codex_facts() {
    let cases = [
        (
            "claude",
            serde_json::json!({"type":"assistant","uuid":"record\0🦀","message":{"id":"m","role":"assistant","model":"claude-test","content":[],"usage":{"input_tokens":3,"output_tokens":2}}}),
        ),
        (
            "copilot",
            serde_json::json!({"type":"session.start","data":{"sessionId":"s\0🦀"}}),
        ),
    ];
    let (pool, _) = fixture(|_| {});
    for (harness, record) in cases {
        let index = parsed(harness, &format!("{record}\n"));
        assert!(index.native_usage_observations.is_none());
        pool.transaction(
            &ns(harness),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                algorithm::write(&mut Usage::new(tx)?, "same", &ledger(1), &index)?;
                assert!(rows(tx)?.is_empty());
                Ok(())
            },
        )
        .unwrap();
    }
}

#[test]
fn pg_native_usage_concurrent_generation_and_fact_replay_are_namespace_serialized() {
    let body = format!(
        "{}\n",
        serde_json::json!({"type":"token_usage_record","payload":{"thread_id":"t","session_id":"s","turn_id":"turn","response_id":"response","usage":{"input_tokens":1,"output_tokens":0,"total_tokens":1,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}})
    );
    let index = std::sync::Arc::new(parsed("codex", &body));
    let end = ledger(body.len() as u64);
    let (pool, _) = fixture(|_| {});
    let namespace = ns("race\0🦀");
    pool.transaction(
        &namespace,
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            let mut effects = Usage::new(tx)?;
            algorithm::begin(&mut effects, "path", None, &end, false)?;
            algorithm::write(&mut effects, "path", &end, &index)
        },
    )
    .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let (pool, index, end, namespace, barrier) = (
                pool.clone(),
                index.clone(),
                end.clone(),
                namespace.clone(),
                barrier.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                pool.transaction(
                    &namespace,
                    TxnKind::Write,
                    &PgCancellation::default(),
                    |tx| {
                        let mut effects = Usage::new(tx)?;
                        assert_eq!(
                            algorithm::begin(&mut effects, "path", Some(&end), &end, true)?,
                            1
                        );
                        algorithm::write(&mut effects, "path", &end, &index)
                    },
                )
                .unwrap();
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
    pool.transaction(
        &namespace,
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            for table in [
                "semon_pg_native_usage_sources",
                "semon_pg_native_usage_facts",
            ] {
                assert_eq!(
                    tx.driver
                        .query_one(&format!("SELECT count(*) FROM {table}"), &[])
                        .map_err(sqlstate::error)?
                        .get::<_, i64>(0),
                    1
                );
            }
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn pg_native_usage_warm_revocation_and_manifest_drift_refuse_before_family_effects() {
    for ddl in [
        "ALTER TABLE semon_pg_native_usage_facts DISABLE ROW LEVEL SECURITY",
        "ALTER TABLE semon_pg_native_usage_dimensions ALTER COLUMN value DROP NOT NULL",
        "REVOKE ALL ON semon_pg_native_usage_facts FROM PUBLIC; REVOKE ALL ON semon_pg_native_usage_facts FROM CURRENT_USER",
    ] {
        let (pool, factory) = fixture(|limits| limits.max_connections = 1);
        let namespace = ns("warm\0🦀");
        let control = PgCancellation::default();
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |_| Ok(()))
            .unwrap();
        factory.client().batch_execute(ddl).unwrap();
        assert_eq!(
            pool.apply_operation(&namespace, &identity(), &control, |_| panic!(
                "drift before receipt and family effects"
            )),
            Err(StorageError::ScopeChanged)
        );
    }
}

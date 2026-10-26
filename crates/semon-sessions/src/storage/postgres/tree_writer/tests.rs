use super::*;
use crate::storage::postgres::{
    PgCancellation, PgPool,
    tests::{fixture, limits, ns},
    tree_checkpoint::{
        Snapshot,
        tests::{
            Fixture, OracleValue as Value, append_fixture, install, install_without_tree,
            oracle_witness, parser_fixture,
        },
    },
    txn::TxnKind,
};

fn source(tx: &mut PgTxn<'_>, path: &str) -> Result<i64, StorageError> {
    Ok(tx.driver.query_one("SELECT source_id FROM semon_pg_native_usage_current WHERE namespace=$1 AND source_path=$2", &[&tx.namespace.as_str().as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?.get(0))
}
fn input(data: &Fixture) -> &Summary {
    data.sources
        .iter()
        .find(|(path, _, _)| path == &data.path)
        .unwrap()
        .2
        .tree_summary
        .as_ref()
        .unwrap()
}
fn stage_fixture(tx: &mut PgTxn<'_>, data: &Fixture) -> Result<(i64, String), StorageError> {
    let source = source(tx, &data.path)?;
    stage(tx, &data.path, &data.ledger, None, source, input(data))
}

// Compare actual PG rows against the independently produced SQLite IndexStore,
// normalizing only the physical projection ID. No input-Summary oracle.
fn assert_dimensions(tx: &mut PgTxn<'_>, data: &Fixture, id: i64) {
    let header = &data
        .rows
        .iter()
        .find(|(f, _)| *f == "tree_headers")
        .unwrap()
        .1;
    let physical = header
        .iter()
        .find(|row| {
            row.iter()
                .any(|(k, v)| k == "source_path" && v == &Value::Text(data.path.clone()))
        })
        .unwrap();
    let oracle_id = physical
        .iter()
        .find_map(|(k, v)| {
            if k == "projection_id" {
                if let Value::Integer(n) = v {
                    Some(*n)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap();
    for (family, columns) in [
        ("tree_labels", vec!["label", "value"]),
        ("tree_models", vec!["model"]),
        ("tree_tools", vec!["tool_id", "name", "closed"]),
        ("tree_usage_models", vec!["message_id", "model"]),
    ] {
        let rows = tx.driver.query(&format!("SELECT {} FROM semon_pg_native_{family} WHERE namespace=$1 AND projection_id=$2 ORDER BY {}",columns.join(","),columns[0]),&[&tx.namespace.as_str().as_bytes(),&id]).unwrap();
        let actual = rows
            .iter()
            .map(|r| {
                columns
                    .iter()
                    .enumerate()
                    .map(|(at, key)| {
                        if *key == "closed" {
                            Value::Integer(i64::from(r.get::<_, bool>(at)))
                        } else {
                            r.get::<_, Option<Vec<u8>>>(at)
                                .map(|v| Value::Text(String::from_utf8(v).unwrap()))
                                .unwrap_or(Value::Null)
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut expected = data
            .rows
            .iter()
            .find(|(f, _)| *f == family)
            .unwrap()
            .1
            .iter()
            .filter(|row| {
                row.iter()
                    .any(|(k, v)| k == "projection_id" && v == &Value::Integer(oracle_id))
            })
            .map(|row| {
                columns
                    .iter()
                    .map(|key| {
                        row.iter()
                            .find(|(k, _)| k.as_str() == *key)
                            .unwrap()
                            .1
                            .clone()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        expected.sort_by(|a, b| match (&a[0], &b[0]) {
            (Value::Text(a), Value::Text(b)) => a.as_bytes().cmp(b.as_bytes()),
            _ => panic!("text dimension key"),
        });
        assert_eq!(actual, expected, "{family}");
    }
}

#[test]
fn pg_tree_writer_real_parser_dimensions_witness_restart_completion_closed() {
    for harness in ["claude", "codex"] {
        let data = parser_fixture(harness);
        let expected = oracle_witness(&data);
        let (pool, factory) = fixture(|_| {});
        let (id, digest) = pool
            .transaction(
                &ns("tree\0🦀"),
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| {
                    install_without_tree(tx, &data)?;
                    let staged = stage_fixture(tx, &data)?;
                    assert_dimensions(tx, &data, staged.0);
                    Ok(staged)
                },
            )
            .unwrap();
        assert_eq!(digest, expected);
        // There is no scalar owner, so no complete/readable header or current
        // scalar may be manufactured, even for an otherwise valid Tree.
        assert!(
            Snapshot::restore(
                pool.clone(),
                ns("tree\0🦀"),
                &data.path,
                &data.ledger,
                PgCancellation::default()
            )
            .is_err()
        );
        drop(pool);
        let pool = PgPool::open(factory, limits()).unwrap();
        pool.transaction(&ns("tree\0🦀"),TxnKind::ReadSnapshot,&PgCancellation::default(),|tx| {
            assert_dimensions(tx,&data,id);
            assert_eq!(persisted_witness(tx,id)?,expected);
            let row = tx.driver.query_one("SELECT complete,work_pending FROM semon_pg_native_tree_generations WHERE projection_id=$1",&[&id]).map_err(sqlstate::error)?;
            assert!(!row.get::<_,bool>(0));
            assert!(row.get::<_,bool>(1));
            assert_eq!(tx.driver.query_one("SELECT count(*) FROM semon_pg_native_tree_headers",&[]).unwrap().get::<_,i64>(0),0);
            assert_eq!(tx.driver.query_one("SELECT count(*) FROM semon_pg_native_meta",&[]).unwrap().get::<_,i64>(0),0);
            Ok(())
        }).unwrap();
        std::fs::remove_dir_all(data.root).unwrap();
    }
}

fn state(tx: &mut PgTxn<'_>) -> Vec<(String, Vec<serde_json::Value>)> {
    // All native tables plus source-state, operation and receipt families; JSON
    // bytea encoding compares the exact persisted bytes without printing them.
    let tables = tx.driver.query("SELECT table_name FROM information_schema.tables WHERE table_schema=current_schema() AND table_name LIKE 'semon_pg_%' ORDER BY table_name",&[]).unwrap();
    tables
        .iter()
        .map(|r| {
            let table: String = r.get(0);
            let rows = tx
                .driver
                .query(
                    &format!("SELECT to_jsonb(t) FROM {table} t ORDER BY to_jsonb(t)::text"),
                    &[],
                )
                .unwrap()
                .iter()
                .map(|r| r.get(0))
                .collect();
            (table, rows)
        })
        .collect()
}

#[test]
fn pg_tree_writer_every_effect_fault_rolls_back_usage_dimensions_and_foreign_source() {
    let data = parser_fixture("claude");
    let (pool, _) = fixture(|_| {});
    for namespace in ["tree\0🦀", "tree\0é"] {
        pool.transaction(
            &ns(namespace),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| install_without_tree(tx, &data),
        )
        .unwrap();
    }
    let baseline = |namespace: &str| {
        pool.transaction(
            &ns(namespace),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| Ok(state(tx)),
        )
        .unwrap()
    };
    let own = baseline("tree\0🦀");
    let foreign = baseline("tree\0é");
    let mut boundaries = 0;
    for fault in 1..64 {
        FAIL_AFTER.with(|p| p.set(Some(fault)));
        let result = pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                // A preceding NativeUsage replacement is part of the SAME borrowed
                // transaction and must roll back with each failing Tree effect.
                let index = &data
                    .sources
                    .iter()
                    .find(|(path, _, _)| path == &data.path)
                    .unwrap()
                    .2;
                let mut usage = crate::storage::postgres::native_usage::Usage::new(tx)?;
                crate::events::store::native_usage::algorithm::begin(
                    &mut usage,
                    &data.path,
                    None,
                    &data.ledger,
                    false,
                )?;
                crate::events::store::native_usage::algorithm::write(
                    &mut usage,
                    &data.path,
                    &data.ledger,
                    index,
                )?;
                stage_fixture(tx, &data)
            },
        );
        let remaining = FAIL_AFTER.with(|p| p.replace(None));
        if result.is_ok() {
            assert!(remaining.is_some());
            break;
        }
        boundaries += 1;
        assert_eq!(baseline("tree\0🦀"), own);
        assert_eq!(baseline("tree\0é"), foreign);
    }
    assert!(
        boundaries >= 7,
        "must fault generation and each emitted dimension"
    );
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_same_source_retains_closed_names_models_and_replacement_is_new() {
    let data = parser_fixture("claude");
    let (pool, _) = fixture(|_| {});
    pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
        install(tx,&data)?;
        let original: i64 = tx.driver.query_one("SELECT projection_id FROM semon_pg_native_tree_headers WHERE source_path=$1",&[&data.path.as_bytes()]).unwrap().get(0);
        let before = state(tx);
        tx.driver.batch_execute("SAVEPOINT staged").unwrap();
        let source = source(tx,&data.path)?;
        let mut delta = Summary::default();
        delta.closed_tools.insert("tool\0🦀".into());
        delta.closed_tools.insert("null-name\0🦀".into());
        delta.tools.insert("new\0\u{1f}'🦀".into(),"Read\n\t🦀".into());
        delta.models.insert("new-model🦀".into());
        delta.cwd=Some("/control\0\u{1f}\n🦀".into());
        let (id,digest) = stage(tx,&data.path,&data.ledger,Some(&data.ledger),source,&delta)?;
        assert_eq!(id,original);
        let old = tx.driver.query_one("SELECT name,closed FROM semon_pg_native_tree_tools WHERE projection_id=$1 AND tool_id=$2",&[&id,&"tool\0🦀".as_bytes()]).unwrap();
        assert_eq!(old.get::<_,Vec<u8>>(0),"Bash'\0🦀".as_bytes());
        assert!(old.get::<_,bool>(1));
        assert!(tx.driver.query_one("SELECT name FROM semon_pg_native_tree_tools WHERE projection_id=$1 AND tool_id=$2",&[&id,&"null-name\0🦀".as_bytes()]).unwrap().get::<_,Option<Vec<u8>>>(0).is_none());
        assert!(tx.driver.query_one("SELECT count(*) FROM semon_pg_native_tree_models WHERE projection_id=$1",&[&id]).unwrap().get::<_,i64>(0)>=2);
        // The persisted digest includes old models/labels and open tools, not
        // merely the rows supplied by this delta.
        assert_eq!(digest,persisted_witness(tx,id)?);
        // Incomplete staged rows can never be promoted by another stage call.
        assert!(stage(tx,&data.path,&data.ledger,Some(&data.ledger),source,&delta).is_err());
        tx.driver.batch_execute("ROLLBACK TO SAVEPOINT staged").unwrap();
        assert_eq!(state(tx),before);
        // Real shared NativeUsage replacement, rather than editing a source ID.
        let index = &data.sources.iter().find(|(path,_,_)|path==&data.path).unwrap().2;
        let mut usage = crate::storage::postgres::native_usage::Usage::new(tx)?;
        let replacement = crate::events::store::native_usage::algorithm::begin(&mut usage,&data.path,None,&data.ledger,false)?;
        crate::events::store::native_usage::algorithm::write(&mut usage,&data.path,&data.ledger,index)?;
        assert_ne!(replacement,source);
        let (id,_) = stage(tx,&data.path,&data.ledger,Some(&data.ledger),replacement,input(&data))?;
        assert_ne!(id,original);
        assert_dimensions(tx,&data,id);
        Ok(())
    }).unwrap();
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_stale_source_ledger_namespace_and_readiness_refuse_before_effects() {
    let data = parser_fixture("codex");
    let (pool, _) = fixture(|_| {});
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install_without_tree(tx, &data),
    )
    .unwrap();
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            let before = state(tx);
            let id = source(tx, &data.path)?;
            assert!(stage(tx, &data.path, &data.ledger, None, id + 1, input(&data)).is_err());
            let mut wrong = data.ledger.clone();
            wrong.offset += 1;
            assert!(stage(tx, &data.path, &wrong, None, id, input(&data)).is_err());
            assert!(
                stage(
                    tx,
                    &data.path,
                    &data.ledger,
                    Some(&data.ledger),
                    id,
                    input(&data)
                )
                .is_err()
            );
            assert_eq!(state(tx), before);
            tx.driver
                .batch_execute("UPDATE semon_pg_native_usage_sources SET usage_ready=false")
                .unwrap();
            let before = state(tx);
            assert!(stage(tx, &data.path, &data.ledger, None, id, input(&data)).is_err());
            assert_eq!(state(tx), before);
            Ok(())
        },
    )
    .unwrap();
    assert!(
        pool.transaction(
            &ns("tree\0é"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| stage_fixture(tx, &data)
        )
        .is_err()
    );
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_witness_charges_before_decode_and_obeys_cancel_deadline() {
    let data = parser_fixture("claude");
    let (pool, _) = fixture(|_| {});
    let (id, _) = pool
        .transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                install_without_tree(tx, &data)?;
                stage_fixture(tx, &data)
            },
        )
        .unwrap();
    for (rows, bytes) in [(0, usize::MAX), (usize::MAX, 0)] {
        DECODED.with(|p| p.set(0));
        let control = selected_read::SelectedReadControl::default().with_limits(rows, bytes);
        let _scope = selected_read::Scope::enter(&control);
        assert!(
            pool.transaction(
                &ns("tree\0🦀"),
                TxnKind::ReadSnapshot,
                &PgCancellation::default(),
                |tx| persisted_witness(tx, id)
            )
            .is_err()
        );
        assert_eq!(DECODED.with(|p| p.get()), 0);
    }
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            let before = state(tx);
            tx.deadline = std::time::Instant::now();
            assert!(stage_fixture(tx, &data).is_err());
            assert_eq!(state(tx), before);
            // Restore only this test transaction's artificial deadline.
            tx.deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            Ok(())
        },
    )
    .unwrap();
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert!(
        pool.transaction(&ns("tree\0🦀"), TxnKind::Write, &cancelled, |tx| {
            stage_fixture(tx, &data)
        })
        .is_err()
    );
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_real_append_reuses_generation_matches_sqlite_closed_history() {
    for harness in ["claude", "codex"] {
        let initial = parser_fixture(harness);
        let appended = append_fixture(&initial, harness);
        assert!(appended.ledger.offset > initial.ledger.offset);
        let expected = oracle_witness(&appended);
        let (pool, _) = fixture(|_| {});
        pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
            install(tx,&initial)?;
            let original: i64 = tx.driver.query_one("SELECT projection_id FROM semon_pg_native_tree_headers WHERE source_path=$1",&[&initial.path.as_bytes()]).unwrap().get(0);
            let old_source = source(tx,&initial.path)?;
            let index = &appended.sources.iter().find(|(path,_,_)|path==&appended.path).unwrap().2;
            let mut usage = crate::storage::postgres::native_usage::Usage::new(tx)?;
            let reused = crate::events::store::native_usage::algorithm::begin(&mut usage,&initial.path,Some(&initial.ledger),&appended.ledger,true)?;
            crate::events::store::native_usage::algorithm::write(&mut usage,&appended.path,&appended.ledger,index)?;
            assert_eq!(reused,old_source);
            // Test-only physical file import models the preceding core family.
            // No production file writer/ledger ACK is introduced by Tree.
            let l = &appended.ledger;
            tx.driver.execute("UPDATE semon_pg_native_files SET dev=$2::text::numeric,ino=$3::text::numeric,size=$4::text::numeric,mtime_ns=$5,ctime_ns=$6,resume_at=$7::text::numeric,head_sha256=$8,tail_sha256=$9 WHERE path=$1",&[&appended.path.as_bytes(),&l.stat.dev.to_string(),&l.stat.ino.to_string(),&l.stat.size.to_string(),&l.stat.modified_ns.to_be_bytes().as_slice(),&l.stat.changed_ns.map(|v|v.to_be_bytes().to_vec()),&l.offset.to_string(),&l.prefix.as_slice(),&l.tail.as_slice()]).map_err(sqlstate::error)?;
            let (id,digest) = stage(tx,&appended.path,l,Some(&initial.ledger),reused,input(&appended))?;
            assert_eq!(id,original);
            assert_eq!(digest,expected);
            assert_dimensions(tx,&appended,id);
            if harness=="claude" {
                let old = tx.driver.query_one("SELECT name,closed FROM semon_pg_native_tree_tools WHERE projection_id=$1 AND tool_id=$2",&[&id,&"tool\0🦀".as_bytes()]).unwrap();
                assert_eq!(old.get::<_,Vec<u8>>(0),"Bash'\0🦀".as_bytes());
                assert!(old.get::<_,bool>(1));
            }
            Ok(())
        }).unwrap();
        std::fs::remove_dir_all(initial.root).unwrap();
    }
}

#[test]
fn pg_tree_writer_warm_schema_drift_refuses_before_family_callback() {
    let data = parser_fixture("claude");
    let (pool, factory) = fixture(|b| b.max_connections = 1);
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install_without_tree(tx, &data),
    )
    .unwrap();
    factory
        .client()
        .batch_execute("ALTER TABLE semon_pg_native_tree_labels NO FORCE ROW LEVEL SECURITY")
        .unwrap();
    let mut entered = false;
    assert!(
        pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                entered = true;
                stage_fixture(tx, &data)
            }
        )
        .is_err()
    );
    assert!(!entered);
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_lost_family_ack_reconciles_without_ingest_receipt() {
    use crate::storage::postgres::{OperationIdentity, Reconciliation, tests::LOSE_ACK};
    let data = parser_fixture("codex");
    let (pool, _) = fixture(|_| {});
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install_without_tree(tx, &data),
    )
    .unwrap();
    let identity = OperationIdentity {
        operation_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        kind: "stage-tree-family-only",
        request_digest: &[19; 32],
    };
    LOSE_ACK.with(|p| p.set(true));
    let result = pool.apply_operation(&ns("tree\0🦀"),&identity,&PgCancellation::default(),|tx| {
        let (projection,witness) = stage_fixture(tx,&data)?;
        Ok(serde_json::json!({"staged_projection":projection,"witness":witness,"complete":false}))
    });
    assert_eq!(result, Err(StorageError::CommitUncertain));
    let Reconciliation::Committed(receipt) = pool
        .reconcile(&ns("tree\0🦀"), &identity, &PgCancellation::default())
        .unwrap()
    else {
        panic!("family must reconcile");
    };
    assert_eq!(receipt["complete"], false);
    assert_eq!(
        pool.apply_operation(
            &ns("tree\0🦀"),
            &identity,
            &PgCancellation::default(),
            |_| panic!("must not repeat family effects")
        )
        .unwrap(),
        receipt
    );
    assert!(
        pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| stage_fixture(tx, &data)
        )
        .is_err()
    );
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            assert_eq!(
                tx.driver
                    .query_one("SELECT count(*) FROM semon_pg_native_tree_generations", &[])
                    .unwrap()
                    .get::<_, i64>(0),
                1
            );
            assert_eq!(
                tx.driver
                    .query_one("SELECT count(*) FROM semon_pg_source_receipts", &[])
                    .unwrap()
                    .get::<_, i64>(0),
                0
            );
            assert_eq!(
                tx.driver
                    .query_one("SELECT count(*) FROM semon_pg_native_tree_headers", &[])
                    .unwrap()
                    .get::<_, i64>(0),
                0
            );
            Ok(())
        },
    )
    .unwrap();
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_competing_family_writers_stage_once() {
    let data = parser_fixture("claude");
    let (pool, _) = fixture(|_| {});
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install_without_tree(tx, &data),
    )
    .unwrap();
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            pool.transaction(
                &ns("tree\0🦀"),
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| stage_fixture(tx, &data),
            )
        });
        let second = scope.spawn(|| {
            barrier.wait();
            pool.transaction(
                &ns("tree\0🦀"),
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| stage_fixture(tx, &data),
            )
        });
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(StorageError::Conflict)))
            .count(),
        1
    );
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_witness_budget_and_mid_write_cancel_rollback_before_decode() {
    let data = parser_fixture("claude");
    let (pool, _) = fixture(|_| {});
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install_without_tree(tx, &data),
    )
    .unwrap();
    let baseline = || {
        pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| Ok(state(tx)),
        )
        .unwrap()
    };
    let before = baseline();
    for (rows, bytes) in [(0, usize::MAX), (usize::MAX, 0)] {
        DECODED.with(|p| p.set(0));
        {
            let control = selected_read::SelectedReadControl::default().with_limits(rows, bytes);
            let _scope = selected_read::Scope::enter(&control);
            assert!(
                pool.transaction(
                    &ns("tree\0🦀"),
                    TxnKind::Write,
                    &PgCancellation::default(),
                    |tx| stage_fixture(tx, &data)
                )
                .is_err()
            );
        }
        assert_eq!(DECODED.with(|p| p.get()), 0);
        assert_eq!(baseline(), before);
    }
    DECODED.with(|p| p.set(0));
    {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        let control = selected_read::SelectedReadControl::new(
            std::time::Instant::now() + std::time::Duration::from_secs(20),
            cancelled,
        );
        let _scope = selected_read::Scope::enter(&control);
        selected_read::set_hook("pg_tree_writer_witness", 0, move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst)
        });
        assert!(
            pool.transaction(
                &ns("tree\0🦀"),
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| stage_fixture(tx, &data)
            )
            .is_err()
        );
    }
    assert_eq!(DECODED.with(|p| p.get()), 0);
    assert_eq!(baseline(), before);
    std::fs::remove_dir_all(data.root).unwrap();
}

fn publish_fixture(
    tx: &mut PgTxn<'_>,
    data: &Fixture,
    previous: Option<&Ledger>,
) -> Result<bool, StorageError> {
    let id = source(tx, &data.path)?;
    let index = &data
        .sources
        .iter()
        .find(|(p, _, _)| p == &data.path)
        .unwrap()
        .2;
    publish(
        tx,
        &data.path,
        &data.ledger,
        previous,
        id,
        input(data),
        index.native_usage_observations.is_some(),
    )
}
fn oracle_record(data: &Fixture) -> serde_json::Value {
    let db = rusqlite::Connection::open(crate::events::EventCache::path(
        &data.root.join("index.json"),
    ))
    .unwrap();
    let path = data.path.strip_suffix("\0🦀").unwrap();
    let encoded: String = db
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("native_tree_summary:{path}")],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&encoded).unwrap()
}
fn scalar(tx: &mut PgTxn<'_>, data: &Fixture) -> serde_json::Value {
    let bytes: Vec<u8> = tx
        .driver
        .query_one(
            "SELECT value FROM semon_pg_native_meta WHERE key=$1",
            &[&format!("native_tree_summary:{}", data.path).as_bytes()],
        )
        .unwrap()
        .get(0);
    serde_json::from_slice(&bytes).unwrap()
}
fn interpretation_inputs(pool: &PgPool, data: &Fixture) {
    apply_interpretation_inputs(pool, interpretation_values(data));
}
fn interpretation_values(data: &Fixture) -> (Option<String>, Option<String>) {
    use rusqlite::OptionalExtension;
    let db = rusqlite::Connection::open(crate::events::EventCache::path(
        &data.root.join("index.json"),
    ))
    .unwrap();
    let value = |key: &str| {
        db.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .unwrap()
    };
    (
        value("pricing_recompute_target"),
        value("catalog_semantic_recipe"),
    )
}
fn apply_interpretation_inputs(pool: &PgPool, (pricing, recipe): (Option<String>, Option<String>)) {
    if pricing.is_some() {
        super::super::PgStoreMetadata::open(pool.clone(), &PgCancellation::default())
            .unwrap()
            .set_pricing_target(&[1; 32], 0, pricing.as_deref(), &PgCancellation::default())
            .unwrap();
    }
    if let Some(recipe) = recipe {
        pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                tx.driver
                    .execute(
                        "INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3)",
                        &[
                            &tx.namespace.as_str().as_bytes(),
                            &b"catalog_semantic_recipe".as_slice(),
                            &recipe.as_bytes(),
                        ],
                    )
                    .map_err(sqlstate::error)?;
                Ok(())
            },
        )
        .unwrap();
    }
}

#[test]
fn pg_native_keys134_current_witness_long_keys_real_producer_without_sqlite() {
    const CHILD: &str = "SEMON_TREE134_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "storage::postgres::tree_writer::tests::pg_native_keys134_current_witness_long_keys_real_producer_without_sqlite", "--nocapture"])
            .env(CHILD, "1").status().unwrap();
        assert!(status.success(), "isolated Tree witness qualification");
        return;
    }
    // All real SQLite parsing, persistence, scalar and lazy oracles precede PG.
    let data = super::super::tree_checkpoint::tests::parser_fixture_keys("claude", true, true);
    let expected = oracle_record(&data);
    let expected_witness = oracle_witness(&data);
    assert!(expected_witness.starts_with("tree4:"));
    assert!(data.oracle.models.iter().any(|key| key.len() > 2704));
    assert!(
        data.oracle
            .tools
            .iter()
            .any(|(key, name)| key.len() > 2704 && name.len() > 2704)
    );
    let expected_tools = data
        .oracle
        .tools
        .keys()
        .map(|id| (id.clone(), data.oracle.tool_name(id).unwrap()))
        .collect::<Vec<_>>();
    let interpretation = interpretation_values(&data);
    use std::sync::atomic::{AtomicUsize, Ordering};
    static OPENS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn deny(
        _: *mut rusqlite::ffi::sqlite3,
        _: *mut *mut std::ffi::c_char,
        _: *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::ffi::c_int {
        OPENS.fetch_add(1, Ordering::SeqCst);
        rusqlite::ffi::SQLITE_AUTH
    }
    struct NoSqlite;
    impl Drop for NoSqlite {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_cancel_auto_extension(Some(deny));
            }
            assert_eq!(
                OPENS.load(Ordering::SeqCst),
                0,
                "PG attempted SQLite access"
            );
        }
    }
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_auto_extension(Some(deny)) },
        rusqlite::ffi::SQLITE_OK
    );
    std::thread::spawn(|| assert!(rusqlite::Connection::open_in_memory().is_err()))
        .join()
        .unwrap();
    assert_eq!(OPENS.swap(0, Ordering::SeqCst), 1);
    let guard = NoSqlite;
    let (pool, factory) = fixture(|_| {});
    apply_interpretation_inputs(&pool, interpretation);
    let projection = pool.transaction(&ns("tree\0🦀"), TxnKind::Write, &PgCancellation::default(), |tx| {
        install_without_tree(tx, &data)?;
        assert!(publish_fixture(tx, &data, None)?);
        assert!(scalar(tx, &data) == expected);
        let projection: i64 = tx.driver.query_one("SELECT projection_id FROM semon_pg_native_tree_headers WHERE namespace=$1 AND source_path=$2", &[&tx.namespace.as_str().as_bytes(), &data.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
        assert_dimensions(tx, &data, projection);
        assert_eq!(persisted_witness(tx, projection)?, expected_witness);
        Ok(projection)
    }).unwrap();
    pool.transaction(
        &ns("foreign\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            install_without_tree(tx, &data)?;
            let (projection, actual) = stage_fixture(tx, &data)?;
            assert_eq!(actual, expected_witness);
            assert_dimensions(tx, &data, projection);
            Ok(())
        },
    )
    .unwrap();
    // Simulate a forced physical address collision after admission. The real
    // writer must compare heap keys, and the complete transaction rolls back.
    assert!(pool.transaction(&ns("tree\0🦀"), TxnKind::Write, &PgCancellation::default(), |tx| {
        tx.driver.batch_execute("ALTER TABLE semon_pg_native_tree_tools ALTER COLUMN tool_id_digest DROP EXPRESSION").map_err(sqlstate::error)?;
        tx.driver.execute("UPDATE semon_pg_native_tree_tools SET tool_id=$1 WHERE projection_id=$2 AND octet_length(tool_id)>2704", &[&b"wrong-heap-key".as_slice(), &projection]).map_err(sqlstate::error)?;
        Writer { tx }.values(projection, input(&data))
    }).is_err());
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            assert_eq!(persisted_witness(tx, projection)?, expected_witness);
            Ok(())
        },
    )
    .unwrap();
    drop(pool);
    let reopened = PgPool::open(factory, limits()).unwrap();
    let mut summary = Snapshot::restore(
        reopened,
        ns("tree\0🦀"),
        &data.path,
        &data.ledger,
        PgCancellation::default(),
    )
    .unwrap();
    summary.load_shared_usage().unwrap();
    assert_eq!(summary.models, data.oracle.models);
    assert_eq!(
        serde_json::to_value(&summary.usage_by_id).unwrap(),
        serde_json::to_value(&data.oracle.usage_by_id).unwrap()
    );
    for (id, name) in expected_tools {
        assert_eq!(summary.tool_name(&id).unwrap(), name);
    }
    drop(guard);
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_keys134_tool_selected_physical_rows_and_bytes_ignore_unrelated_history() {
    let data = super::super::tree_checkpoint::tests::parser_fixture_keys("claude", false, true);
    let (tool, name) = data
        .oracle
        .tools
        .iter()
        .find(|(key, _)| key.len() > 2704)
        .unwrap();
    let (pool, _) = fixture(|_| {});
    let namespace = ns("tree\0🦀");
    let control = PgCancellation::default();
    let projection = pool
        .transaction(&namespace, TxnKind::Write, &control, |tx| {
            install_without_tree(tx, &data)?;
            Ok(stage_fixture(tx, &data)?.0)
        })
        .unwrap();
    let mut previous = 0_i64;
    for noise in [0_i64, 256, 2048] {
        let (visited, bytes) = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            if noise > previous {
                tx.driver.execute("INSERT INTO semon_pg_native_tree_tools(namespace,projection_id,tool_id,name,closed) SELECT $1,10000+n,$2,$3,false FROM generate_series($4::bigint,$5::bigint) n", &[&namespace.as_str().as_bytes(), &tool.as_bytes(), &name.as_bytes(), &previous, &(noise-1)]).map_err(sqlstate::error)?;
            }
            let query = super::super::tree_checkpoint::TOOL_NAME_LENGTH_SQL;
            let args: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&namespace.as_str().as_bytes(), &projection, &tool.as_bytes()];
            let plan: serde_json::Value = tx.driver.query_one(&format!("EXPLAIN (ANALYZE, FORMAT JSON) {query}"), args).map_err(sqlstate::error)?.get(0);
            fn physical_rows(plan: &serde_json::Value) -> u64 {
                let removed = plan.get("Rows Removed by Filter").and_then(serde_json::Value::as_u64).unwrap_or(0)
                    + plan.get("Rows Removed by Index Recheck").and_then(serde_json::Value::as_u64).unwrap_or(0);
                match plan.get("Plans").and_then(serde_json::Value::as_array) {
                    Some(children) => removed + children.iter().map(physical_rows).sum::<u64>(),
                    None => (removed + plan["Actual Rows"].as_u64().unwrap()) * plan["Actual Loops"].as_u64().unwrap(),
                }
            }
            let bytes: i64 = tx.driver.query_one(query, args).map_err(sqlstate::error)?.get(0);
            Ok((physical_rows(&plan[0]["Plan"]), bytes))
        }).unwrap();
        assert!(
            visited <= 4,
            "noise={noise} actual selected SQL rows={visited}"
        );
        assert_eq!(bytes as usize, name.len());
        eprintln!("keys134_tool noise={noise} sql_rows={visited} heap_name_bytes={bytes}");
        previous = noise;
    }
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_keys134_tree_warm_generated_and_index_drift_refuses_before_effects() {
    let data = parser_fixture("claude");
    for alteration in [
        "ALTER TABLE semon_pg_native_tree_models ALTER COLUMN model_digest DROP EXPRESSION",
        "ALTER TABLE semon_pg_native_tree_tools ALTER COLUMN tool_id_digest DROP EXPRESSION",
        "DROP INDEX semon_pg_native_tree_open_tools",
    ] {
        let (pool, factory) = fixture(|_| {});
        pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                install_without_tree(tx, &data)?;
                stage_fixture(tx, &data)?;
                Ok(())
            },
        )
        .unwrap();
        factory.client().batch_execute(alteration).unwrap();
        let mut entered = false;
        assert!(
            pool.transaction(
                &ns("tree\0🦀"),
                TxnKind::Write,
                &PgCancellation::default(),
                |_| {
                    entered = true;
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!entered, "drift reached payload callback");
    }
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_keys132_tree_real_sqlite_long_path_publication_and_restart() {
    // Finish the real SQLite parser/store oracle before creating a PG pool.
    // This retained path-specific oracle keeps models/tools short. Worker134
    // separately exercises real long model/tool/name inputs.
    let data = super::super::tree_checkpoint::tests::parser_fixture_path("claude", true);
    let expected = oracle_record(&data);
    assert!(data.path.len() > 2704);
    let interpretation = interpretation_values(&data);
    let expected_witness = oracle_witness(&data);
    let expected_tools = data
        .oracle
        .tools
        .keys()
        .map(|id| (id.clone(), data.oracle.tool_name(id).unwrap()))
        .collect::<Vec<_>>();
    let (pool, factory) = fixture(|_| {});
    apply_interpretation_inputs(&pool, interpretation);
    pool.transaction(&ns("tree\0🦀"), TxnKind::Write, &PgCancellation::default(), |tx| {
        install_without_tree(tx, &data)?;
        assert!(publish_fixture(tx, &data, None)?);
        assert!(scalar(tx, &data) == expected);
        let projection: i64 = tx.driver.query_one(
            "SELECT projection_id FROM semon_pg_native_tree_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path_digest=semon_pg_native_digest($1::bytea,$2::bytea,'tree.path') AND semon_pg_native_key_matches(source_path,sha256($2::bytea),$2)",
            &[&tx.namespace.as_str().as_bytes(), &data.path.as_bytes()],
        ).map_err(sqlstate::error)?.get(0);
        assert_dimensions(tx, &data, projection);
        let before = state(tx);
        assert!(publish_fixture(tx, &data, Some(&data.ledger))?);
        assert!(state(tx) == before, "long-path replay has no row changes");
        Ok(())
    }).unwrap();
    pool.transaction(
        &ns("foreign\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            install_without_tree(tx, &data)?;
            let (projection, actual) = stage_fixture(tx, &data)?;
            assert_eq!(actual, expected_witness);
            assert_dimensions(tx, &data, projection);
            Ok(())
        },
    )
    .unwrap();
    drop(pool);
    let reopened = PgPool::open(factory, limits()).unwrap();
    let mut summary = Snapshot::restore(
        reopened,
        ns("tree\0🦀"),
        &data.path,
        &data.ledger,
        PgCancellation::default(),
    )
    .unwrap();
    summary.load_shared_usage().unwrap();
    assert_eq!(summary.models, data.oracle.models);
    assert_eq!(
        serde_json::to_value(&summary.usage_by_id).unwrap(),
        serde_json::to_value(&data.oracle.usage_by_id).unwrap()
    );
    for (tool, name) in expected_tools {
        assert_eq!(summary.tool_name(&tool).unwrap(), name);
    }
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_keys132_tree_header_actual_sql_noise_rows_and_heap_bytes() {
    let data = super::super::tree_checkpoint::tests::parser_fixture_path("claude", true);
    let interpretation = interpretation_values(&data);
    let (pool, _) = fixture(|_| {});
    apply_interpretation_inputs(&pool, interpretation);
    let namespace = ns("tree\0🦀");
    let control = PgCancellation::default();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        install_without_tree(tx, &data)?;
        assert!(publish_fixture(tx, &data, None)?);
        Ok(())
    })
    .unwrap();
    let mut previous = 0_i64;
    for noise in [0_i64, 256, 2048] {
        let measurement = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            if noise > previous {
                tx.driver.execute("INSERT INTO semon_pg_native_tree_headers(namespace,source_path,file_id,projection_id,ledger) SELECT $1,convert_to('keys132-unrelated-'||n::text,'UTF8'),1,1,$2 FROM generate_series($3::bigint,$4::bigint) n", &[&namespace.as_str().as_bytes(), &b"unrelated".as_slice(), &previous, &(noise-1)]).map_err(sqlstate::error)?;
            }
            assert!(tx.driver.query_one(HEADER_EXISTS_SQL, &[&namespace.as_str().as_bytes(), &data.path.as_bytes()]).map_err(sqlstate::error)?.get::<_,bool>(0));
            let row = tx.driver.query_one(&format!("EXPLAIN (ANALYZE, FORMAT JSON) {HEADER_EXISTS_SQL}"), &[&namespace.as_str().as_bytes(), &data.path.as_bytes()]).map_err(sqlstate::error)?;
            let plan: serde_json::Value = row.get(0);
            fn visited(plan: &serde_json::Value) -> u64 {
                let removed = plan.get("Rows Removed by Filter").and_then(serde_json::Value::as_u64).unwrap_or(0)
                    + plan.get("Rows Removed by Index Recheck").and_then(serde_json::Value::as_u64).unwrap_or(0);
                match plan.get("Plans").and_then(serde_json::Value::as_array) {
                    Some(children) => removed + children.iter().map(visited).sum::<u64>(),
                    None => (removed + plan["Actual Rows"].as_u64().unwrap()) * plan["Actual Loops"].as_u64().unwrap(),
                }
            }
            let bytes: i64 = tx.driver.query_one("SELECT octet_length(source_path)::bigint FROM semon_pg_native_tree_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path_digest=semon_pg_native_digest($1::bytea,$2::bytea,'tree.path') AND semon_pg_native_key_matches(source_path,sha256($2::bytea),$2)", &[&namespace.as_str().as_bytes(), &data.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
            Ok((visited(&plan[0]["Plan"]), bytes))
        }).unwrap();
        assert!(
            measurement.0 <= 4,
            "noise={noise} selected physical rows={}",
            measurement.0
        );
        assert_eq!(measurement.1 as usize, data.path.len());
        eprintln!(
            "keys132_tree_header noise={noise} sql_rows={} heap_path_bytes={}",
            measurement.0, measurement.1
        );
        previous = noise;
    }
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_native_keys132_tree_warm_generated_and_index_drift_refuse_before_callback() {
    for alteration in [
        "ALTER TABLE semon_pg_native_tree_headers DROP COLUMN source_path_digest CASCADE; ALTER TABLE semon_pg_native_tree_headers ADD COLUMN source_path_digest bytea GENERATED ALWAYS AS (sha256(source_path)) STORED NOT NULL",
        "DROP INDEX semon_pg_native_tree_repair_queue; CREATE INDEX semon_pg_native_tree_repair_queue ON semon_pg_native_tree_repairs(namespace_digest,last_attempt,state,source_path_digest)",
        "ALTER TABLE semon_pg_native_tree_usage_models NO FORCE ROW LEVEL SECURITY",
    ] {
        let (pool, factory) = fixture(|_| {});
        let namespace = ns("keys132-drift");
        pool.transaction(
            &namespace,
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |_| Ok(()),
        )
        .unwrap();
        factory.client().batch_execute(alteration).unwrap();
        let mut entered = false;
        assert!(
            pool.transaction(
                &namespace,
                TxnKind::Write,
                &PgCancellation::default(),
                |_| {
                    entered = true;
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!entered, "warm Tree contract drift precedes every callback");
    }
}

#[test]
fn pg_tree_publish_real_scalar_sqlite_oracle_noop_repair_and_restart() {
    for harness in ["claude", "codex"] {
        let data = parser_fixture(harness);
        let expected = oracle_record(&data);
        let (pool, factory) = fixture(|_| {});
        interpretation_inputs(&pool, &data);
        pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
            install_without_tree(tx,&data)?;
            assert!(publish_fixture(tx,&data,None)?);
            let actual=scalar(tx,&data);
            assert!(actual==expected,"all scalar fields equal independent SQLite record");
            let id:i64=tx.driver.query_one("SELECT projection_id FROM semon_pg_native_tree_headers WHERE source_path=$1",&[&data.path.as_bytes()]).unwrap().get(0);
            assert_dimensions(tx,&data,id);
            let before=state(tx);
            assert!(publish_fixture(tx,&data,Some(&data.ledger))?);
            assert!(state(tx)==before,"noop has no persisted changes");
            tx.driver.execute("DELETE FROM semon_pg_native_meta WHERE key=$1",&[&format!("native_tree_summary:{}",data.path).as_bytes()]).unwrap();
            assert!(publish_fixture(tx,&data,Some(&data.ledger))?);
            assert!(scalar(tx,&data)==expected,"representation repair");
            Ok(())
        }).unwrap();
        drop(pool);
        let reopened = PgPool::open(factory, limits()).unwrap();
        let mut summary = Snapshot::restore(
            reopened,
            ns("tree\0🦀"),
            &data.path,
            &data.ledger,
            PgCancellation::default(),
        )
        .unwrap();
        summary.load_shared_usage().unwrap();
        for tool in data.oracle.tools.keys() {
            assert_eq!(
                summary.tool_name(tool).unwrap(),
                data.oracle.tool_name(tool).unwrap()
            );
        }
        assert!(
            serde_json::to_value(summary.usage_by_id).unwrap()
                == serde_json::to_value(&data.oracle.usage_by_id).unwrap()
        );
        std::fs::remove_dir_all(data.root).unwrap();
    }
}

#[test]
fn pg_tree_publish_every_effect_rollback_and_pricing_epoch_changes_interpretation() {
    let data = parser_fixture("codex");
    let (pool, _) = fixture(|_| {});
    interpretation_inputs(&pool, &data);
    for namespace in ["tree\0🦀", "foreign"] {
        pool.transaction(
            &ns(namespace),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| install_without_tree(tx, &data),
        )
        .unwrap();
    }
    let baseline = |namespace: &str| {
        pool.transaction(
            &ns(namespace),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| Ok(state(tx)),
        )
        .unwrap()
    };
    let own = baseline("tree\0🦀");
    let foreign = baseline("foreign");
    let mut count = 0;
    for fault in 1..64 {
        FAIL_AFTER.with(|p| p.set(Some(fault)));
        let result = pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| publish_fixture(tx, &data, None),
        );
        let remaining = FAIL_AFTER.with(|p| p.replace(None));
        if result.is_ok() {
            assert!(remaining.is_some());
            break;
        }
        count += 1;
        assert!(
            baseline("tree\0🦀") == own,
            "all native/store bytes rollback"
        );
        assert!(baseline("foreign") == foreign, "foreign bytes unchanged");
    }
    let dimensions: usize = data
        .rows
        .iter()
        .filter(|(family, _)| {
            matches!(
                *family,
                "tree_labels" | "tree_models" | "tree_tools" | "tree_usage_models"
            )
        })
        .map(|(_, rows)| rows.len())
        .sum();
    assert_eq!(
        count,
        dimensions + 5,
        "each independent oracle dimension plus generation/scalar/marker/complete/header fault"
    );
    let before = pool
        .transaction(
            &ns("tree\0🦀"),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| Ok(scalar(tx, &data)),
        )
        .unwrap();
    let host =
        super::super::PgStoreMetadata::open(pool.clone(), &PgCancellation::default()).unwrap();
    let revision = pool
        .transaction(
            &ns("tree\0🦀"),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |tx| Ok(super::super::store_metadata::read(tx)?.0),
        )
        .unwrap();
    host.set_pricing_target(
        &[2; 32],
        revision,
        Some("pricing-new\0🦀"),
        &PgCancellation::default(),
    )
    .unwrap();
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            assert!(publish_fixture(tx, &data, Some(&data.ledger))?);
            let after = scalar(tx, &data);
            assert_ne!(
                before["interpretation_witness"],
                after["interpretation_witness"]
            );
            assert_eq!(before["projection_witness"], after["projection_witness"]);
            assert!(
                before["summary"] == after["summary"],
                "pricing metadata cannot invent native facts"
            );
            Ok(())
        },
    )
    .unwrap();
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_publish_real_append_and_replacement_complete_without_seeded_headers() {
    for harness in ["claude", "codex"] {
        let initial = parser_fixture(harness);
        let (pool, _) = fixture(|_| {});
        interpretation_inputs(&pool, &initial);
        let first=pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
            install_without_tree(tx,&initial)?;assert!(publish_fixture(tx,&initial,None)?);
            Ok(tx.driver.query_one("SELECT projection_id FROM semon_pg_native_tree_headers WHERE source_path=$1",&[&initial.path.as_bytes()]).unwrap().get::<_,i64>(0))
        }).unwrap();
        let appended = append_fixture(&initial, harness);
        let expected = oracle_record(&appended);
        pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
            let index=&appended.sources.iter().find(|(p,_,_)|p==&appended.path).unwrap().2;
            let mut usage=crate::storage::postgres::native_usage::Usage::new(tx)?;
            crate::events::store::native_usage::algorithm::begin(&mut usage,&appended.path,Some(&initial.ledger),&appended.ledger,true)?;
            crate::events::store::native_usage::algorithm::write(&mut usage,&appended.path,&appended.ledger,index)?;
            let l=&appended.ledger;
            tx.driver.execute("UPDATE semon_pg_native_files SET dev=$2::text::numeric,ino=$3::text::numeric,size=$4::text::numeric,mtime_ns=$5,ctime_ns=$6,resume_at=$7::text::numeric,head_sha256=$8,tail_sha256=$9 WHERE path=$1",&[&appended.path.as_bytes(),&l.stat.dev.to_string(),&l.stat.ino.to_string(),&l.stat.size.to_string(),&l.stat.modified_ns.to_be_bytes().as_slice(),&l.stat.changed_ns.map(|v|v.to_be_bytes().to_vec()),&l.offset.to_string(),&l.prefix.as_slice(),&l.tail.as_slice()]).map_err(sqlstate::error)?;
            assert!(publish_fixture(tx,&appended,Some(&initial.ledger))?);
            assert!(scalar(tx,&appended)==expected,"appended scalar equals SQLite oracle");
            let id:i64=tx.driver.query_one("SELECT projection_id FROM semon_pg_native_tree_headers WHERE source_path=$1",&[&appended.path.as_bytes()]).unwrap().get(0);
            assert_eq!(first,id);assert_dimensions(tx,&appended,id);
            let mut usage=crate::storage::postgres::native_usage::Usage::new(tx)?;
            crate::events::store::native_usage::algorithm::begin(&mut usage,&appended.path,None,&appended.ledger,false)?;
            crate::events::store::native_usage::algorithm::write(&mut usage,&appended.path,&appended.ledger,index)?;
            assert!(publish_fixture(tx,&appended,Some(&appended.ledger))?);
            let replacement:i64=tx.driver.query_one("SELECT projection_id FROM semon_pg_native_tree_headers WHERE source_path=$1",&[&appended.path.as_bytes()]).unwrap().get(0);
            assert_ne!(replacement,first);
            assert_dimensions(tx,&appended,replacement);
            Ok(())
        }).unwrap();
        std::fs::remove_dir_all(initial.root).unwrap();
    }
}

#[test]
fn pg_tree_publish_oversize_scalar_is_unreadable_and_exact_receipt_repairs() {
    let data = parser_fixture("codex");
    let (pool, _) = fixture(|_| {});
    interpretation_inputs(&pool, &data);
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            install_without_tree(tx, &data)?;
            let id = source(tx, &data.path)?;
            let mut big = input(&data).clone();
            big.codex_turn.last = Some(crate::codex_lifecycle::TurnLifecycle {
                thread_id: "x".repeat(1048576),
                turn_id: "oversize".into(),
                phase: crate::codex_lifecycle::TurnPhase::Completed,
                offset: 0,
                source_generation: String::new(),
            });
            assert!(!publish(
                tx,
                &data.path,
                &data.ledger,
                None,
                id,
                &big,
                true
            )?);
            assert_eq!(
                tx.driver
                    .query_one("SELECT count(*) FROM semon_pg_native_tree_headers", &[])
                    .unwrap()
                    .get::<_, i64>(0),
                0
            );
            assert_eq!(
                metadata(tx, format!("native_tree_failure:{}", data.path).as_bytes())?,
                Some(header_ledger(&data.ledger))
            );
            Ok(())
        },
    )
    .unwrap();
    assert!(
        Snapshot::restore(
            pool.clone(),
            ns("tree\0🦀"),
            &data.path,
            &data.ledger,
            PgCancellation::default()
        )
        .is_err()
    );
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            assert!(
                stage_fixture(tx, &data).is_err(),
                "ordinary duplicate stage remains refused"
            );
            assert!(publish_fixture(tx, &data, None)?);
            assert!(
                metadata(tx, format!("native_tree_failure:{}", data.path).as_bytes())?.is_none()
            );
            Ok(())
        },
    )
    .unwrap();
    assert!(
        Snapshot::restore(
            pool,
            ns("tree\0🦀"),
            &data.path,
            &data.ledger,
            PgCancellation::default()
        )
        .is_ok()
    );
    std::fs::remove_dir_all(data.root).unwrap();
}

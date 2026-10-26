use super::*;
use crate::storage::postgres::{
    PgCancellation, Reconciliation,
    tests::{LOSE_ACK, fixture, identity, ns},
    txn::TxnKind,
};

fn fixture_root() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "pg104-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

fn parsed(
    root: &std::path::Path,
    harness: &str,
    phase: usize,
) -> (FileIndex, Vec<(String, String)>) {
    let source = root.join(format!("{harness}.jsonl"));
    let body = match harness {
        "claude" => format!(
            "{}\n{}\n{}\n",
            serde_json::json!({"type":"user","sessionId":"own","session_id":format!("copy-{phase}\0🦀"),"uuid":"u","message":{"role":"user","content":"sanitized"}}),
            serde_json::json!({"type":"continued-in","continuedInSessionId":format!("continued-{phase}\0🦀")}),
            serde_json::json!({"type":"bridge-session","bridgeSessionId":format!("bridge-{phase}'\0🦀")})
        ),
        "codex" => format!(
            "{}\n{}\n",
            serde_json::json!({"type":"session_meta","payload":{"id":format!("native-{phase}\0🦀"),"parent_thread_id":format!("parent-{phase}\0🦀"),"thread_source":if phase == 2 {"guardian_review"} else {"cli"}}}),
            serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5"}})
        ),
        "copilot" => format!(
            "{}\n",
            serde_json::json!({"type":"session.start","id":"start","timestamp":"2026-09-29T00:00:00Z","data":{"sessionId":"s🦀","context":{"cwd":"/w🦀"}}})
        ),
        _ => unreachable!(),
    };
    std::fs::write(&source, body).unwrap();
    crate::events::store::native_commit::parser_oracle(
        &root.join(format!("{harness}.json")),
        &source,
        harness,
    )
}

fn read(tx: &mut PgTxn<'_>, path: &str) -> Result<Vec<(String, String)>, StorageError> {
    tx.driver.query("SELECT harness,native_id FROM semon_pg_native_native_dependencies WHERE namespace=$1 AND source_path=$2 ORDER BY harness,native_id", &[&tx.namespace.as_str().as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?.into_iter().map(|row| {
        let decode = |bytes| String::from_utf8(bytes).map_err(|_| StorageError::Unavailable);
        Ok((decode(row.get(0))?,decode(row.get(1))?))
    }).collect()
}

#[test]
fn pg_native_keys128_dependencies_real_sqlite_long_opaque_identity() {
    use sha2::{Digest, Sha256};
    let root = fixture_root();
    let key = (0u32..56)
        .map(|i| format!("{:x}", Sha256::digest(i.to_be_bytes())))
        .collect::<String>();
    let native = format!("{key}\0🦀");
    let source = root.join("long.jsonl");
    std::fs::write(&source, format!("{}\n", serde_json::json!({"type":"session_meta","payload":{"id":native,"parent_thread_id":native,"thread_source":"cli"}}))).unwrap();
    let (index, expected) = crate::events::store::native_commit::parser_oracle(
        &root.join("cache.json"),
        &source,
        "codex",
    );
    assert!(expected.iter().any(|(_, id)| id.len() > 2704));
    let path = format!("/{native}");
    let (pool, _) = fixture(|l| l.max_connections = 1);
    let control = PgCancellation::default();
    for namespace in [ns("keys128-dependency"), ns("keys128-foreign")] {
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            relationships(tx, &path, &index, false)?;
            assert_eq!(read(tx, &path)?, expected);
            relationships(tx, &path, &index, false)?;
            assert_eq!(read(tx, &path)?, expected);
            Ok(())
        })
        .unwrap();
        assert!(pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver.batch_execute("ALTER TABLE semon_pg_native_native_dependencies ALTER COLUMN source_path_digest DROP EXPRESSION; UPDATE semon_pg_native_native_dependencies SET source_path=decode('00','hex')").map_err(sqlstate::error)?;
            relationships(tx, &path, &index, false)
        }).is_err());
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            assert_eq!(read(tx, &path)?, expected);
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn pg_native_commit_relationships_real_parser_replacement_restart_oracle() {
    let root = fixture_root();
    // Every oracle runs its real SQLite IndexStore before any PG operation.
    let mut phases = Vec::new();
    for harness in ["claude", "codex", "copilot"] {
        for phase in 0..3 {
            let (index, expected) = parsed(&root, harness, phase);
            if harness == "claude" {
                assert_eq!(expected.len(), 3);
            }
            if harness == "codex" {
                assert_eq!(expected.len(), if phase == 2 { 1 } else { 2 });
            }
            if harness == "copilot" {
                assert!(expected.is_empty());
            }
            phases.push((harness, phase, index, expected));
        }
    }
    let (pool, factory) = fixture(|limits| limits.max_connections = 1);
    let namespace = ns("relationships\0🦀");
    let control = PgCancellation::default();
    let path = "logical'\0🦀";
    for (harness, phase, index, expected) in phases {
        // A different operation per parser generation; the receipt is explicitly
        // a relationship-family receipt, never an IndexStore/ingest ACK.
        let operation_id = format!("{:064x}", 100 + phase + harness.len() * 10);
        let digest = [phase as u8; 32];
        let operation = super::super::OperationIdentity {
            operation_id: &operation_id,
            kind: "relationships",
            request_digest: &digest,
        };
        let receipt = pool
            .apply_operation(&namespace, &operation, &control, |tx| {
                relationships(tx, path, &index, true)?;
                assert_eq!(read(tx, path)?, expected);
                Ok(serde_json::json!({"relationships":true}))
            })
            .unwrap();
        assert_eq!(
            pool.apply_operation(&namespace, &operation, &control, |_| panic!(
                "original receipt must prevent replay"
            )),
            Ok(receipt)
        );
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            assert_eq!(read(tx, path)?, expected);
            let files: i64 = tx
                .driver
                .query_one("SELECT count(*) FROM semon_pg_native_files", &[])
                .map_err(sqlstate::error)?
                .get(0);
            assert_eq!(files, 0, "partial family must not move a file ledger");
            Ok(())
        })
        .unwrap();
    }
    let bounds = pool.inner.limits;
    drop(pool);
    let reopened = super::super::PgPool::open(factory, bounds).unwrap();
    reopened
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            assert!(
                read(tx, path)?.is_empty(),
                "last Copilot generation replaces all managed dependencies"
            );
            Ok(())
        })
        .unwrap();
    drop(reopened);
}

#[test]
fn pg_native_commit_relationships_faults_foreign_namespace_and_original_ack() {
    let root = fixture_root();
    let (index, expected) = parsed(&root, "claude", 0);
    let (pool, _) = fixture(|limits| limits.max_connections = 1);
    let namespace = ns("dependency\0🦀");
    let foreign = ns("dependency-other\0🦀");
    let control = PgCancellation::default();
    let path = "same'\0🦀";
    // Family has lock, clear, each insertion, second clear and invalidation.
    // Each induced failure must roll back rows, revision and original receipt.
    for point in 1..=7 {
        FAIL_AFTER.with(|flag| flag.set(Some(point)));
        assert_eq!(
            pool.apply_operation(&namespace, &identity(), &control, |tx| {
                relationships(tx, path, &index, true)?;
                Ok(serde_json::json!({"family":true}))
            }),
            Err(StorageError::Unavailable)
        );
        FAIL_AFTER.with(|flag| flag.set(None));
        assert_eq!(
            pool.reconcile(&namespace, &identity(), &control).unwrap(),
            Reconciliation::Absent
        );
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            assert!(read(tx, path)?.is_empty());
            let revisions: i64 = tx
                .driver
                .query_one(
                    "SELECT coalesce(sum(revision),0)::bigint FROM semon_pg_source_state",
                    &[],
                )
                .map_err(sqlstate::error)?
                .get(0);
            assert_eq!(revisions, 0);
            Ok(())
        })
        .unwrap();
    }
    pool.transaction(&foreign, TxnKind::Write, &control, |tx| {
        relationships(tx, path, &index, true)?;
        Ok(())
    })
    .unwrap();
    LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, |tx| {
            relationships(tx, path, &index, true)?;
            Ok(serde_json::json!({"family":true}))
        }),
        Err(StorageError::CommitUncertain)
    );
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, |_| panic!(
            "uncertain ACK retry cannot reparse Raw or repeat effects"
        ))
        .unwrap(),
        serde_json::json!({"family":true})
    );
    for scope in [&namespace, &foreign] {
        pool.transaction(scope, TxnKind::ReadSnapshot, &control, |tx| {
            assert_eq!(read(tx, path)?, expected);
            let revision: i64 = tx
                .driver
                .query_one("SELECT revision FROM semon_pg_source_state", &[])
                .map_err(sqlstate::error)?
                .get(0);
            assert_eq!(revision, 1);
            Ok(())
        })
        .unwrap();
    }
    drop(pool);
}

#[test]
fn pg_native_commit_relationships_precancel_and_warm_drift_before_effects() {
    let root = fixture_root();
    let (index, expected) = parsed(&root, "claude", 0);
    let (pool, _) = fixture(|limits| limits.max_connections = 1);
    let namespace = ns("warm-dependencies\0🦀");
    let path = "same\0🦀";
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &cancelled, |_| {
            panic!("cancelled request cannot enter native effects")
        }),
        Err(StorageError::Busy)
    );
    let control = PgCancellation::default();
    assert_eq!(
        pool.reconcile(&namespace, &identity(), &control).unwrap(),
        Reconciliation::Absent
    );
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        relationships(tx, path, &index, true)
    })
    .unwrap();
    pool.controlled_schema_change(&control, |tx| {
        tx.driver
            .batch_execute(
                "CREATE INDEX pg104_dependency_drift ON semon_pg_native_native_dependencies(harness)",
            )
            .map_err(sqlstate::error)
    })
    .unwrap();
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, |_| {
            panic!("warm schema drift cannot enter native effects")
        }),
        Err(StorageError::ScopeChanged)
    );
    // Test-only inspection bypasses the deliberately damaged manifest. The
    // production operation above always uses verified transaction admission.
    pool.transaction_inner(&namespace, TxnKind::ReadSnapshot, &control, false, |tx| {
        assert_eq!(read(tx, path)?, expected);
        let row = tx.driver.query_one("SELECT revision,(SELECT count(*) FROM semon_pg_operations) FROM semon_pg_source_state", &[]).map_err(sqlstate::error)?;
        assert_eq!(row.get::<_, i64>(0), 1);
        assert_eq!(row.get::<_, i64>(1), 0);
        Ok(())
    })
    .unwrap();
}

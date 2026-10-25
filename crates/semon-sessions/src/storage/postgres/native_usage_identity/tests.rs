use super::*;
use crate::storage::postgres::{
    PgCancellation, PgPool, Reconciliation,
    tests::{fixture, identity, limits, ns},
    txn::TxnKind,
};
use std::sync::{Arc, Barrier};

type IdentityRow = (i64, String, String);

fn insert(tx: &mut PgTxn<'_>, row: &IdentityRow) -> Result<(), StorageError> {
    tx.driver.execute("INSERT INTO semon_pg_native_usage_identity(namespace,file_id,message_id,record_uuid) VALUES($1,$2,$3,$4)", &[&tx.namespace.as_str().as_bytes(), &row.0, &row.1.as_bytes(), &row.2.as_bytes()]).map_err(sqlstate::error)?;
    Ok(())
}

fn copied(tx: &mut PgTxn<'_>) -> Result<Vec<IdentityRow>, StorageError> {
    tx.driver.query("SELECT file_id,message_id,record_uuid FROM semon_pg_native_tree_copied_identities ORDER BY file_id,message_id,record_uuid", &[]).map_err(sqlstate::error)?.iter().map(|r| {
        Ok((r.get(0),String::from_utf8(r.get(1)).map_err(|_|StorageError::Unavailable)?,String::from_utf8(r.get(2)).map_err(|_|StorageError::Unavailable)?))
    }).collect()
}

fn sqlite_copied(c: &rusqlite::Connection) -> Vec<IdentityRow> {
    c.prepare("SELECT file_id,message_id,record_uuid FROM native_tree_copied_identities ORDER BY file_id,message_id,record_uuid").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

#[test]
fn pg_native_copied_identity_real_parser_sqlite_oracle_restart_rollback_and_ack() {
    use crate::events::{EventCache, scan_file};
    let root = std::env::temp_dir().join(format!(
        "pg90-copies-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let cache_path = root.join("index.json");
    let mut cache = EventCache::open(&cache_path).unwrap();
    // The actual shared Claude parser supplies both identity keys. No prepared
    // source, artificial membership, complete FileIndex JSON or Raw body in PG.
    let line = serde_json::json!({"type":"assistant","uuid":"record\0🦀","timestamp":"2026-09-29T00:00:01Z","message":{"id":"message\0🦀","role":"assistant","model":"claude-test","content":[{"type":"text","text":"Raw-only fixture body"}],"usage":{"input_tokens":3,"output_tokens":2}}});
    for i in 1..=3 {
        let path = root.join(format!("{i}.jsonl"));
        std::fs::write(&path, format!("{line}\n")).unwrap();
        scan_file(&path, "claude", &mut cache, &mut false).unwrap();
    }
    cache.flush_cold_writes().unwrap();
    drop(cache);
    let sqlite = rusqlite::Connection::open(EventCache::path(&cache_path)).unwrap();
    let rows = sqlite.prepare("SELECT file_id,message_id,record_uuid FROM usage_identity ORDER BY file_id,message_id,record_uuid").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<rusqlite::Result<Vec<IdentityRow>>>().unwrap();
    assert_eq!(
        rows.len(),
        3,
        "fixture must execute actual usage identities"
    );
    assert!(
        rows.iter()
            .all(|r| r.1 == "message\0🦀" && r.2 == "record\0🦀")
    );
    let expected = sqlite_copied(&sqlite);
    assert_eq!(expected.len(), 3);
    let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
    let control = PgCancellation::default();
    let namespace = ns("copies\0🦀");
    // Fault after all peer effects: both identity tables, the namespace lock row
    // and the original durable operation placeholder must roll back together.
    let failed = pool.apply_operation(&namespace, &identity(), &control, |tx| {
        for row in &rows {
            insert(tx, row)?;
        }
        assert_eq!(copied(tx)?, expected);
        Err(StorageError::Unavailable)
    });
    assert_eq!(failed, Err(StorageError::Unavailable));
    assert_eq!(
        pool.reconcile(&namespace, &identity(), &control),
        Ok(Reconciliation::Absent)
    );
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert!(copied(tx)?.is_empty());
        for table in ["semon_pg_native_usage_identity", "semon_pg_source_state"] {
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
    crate::storage::postgres::tests::LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, |tx| {
            for row in &rows {
                insert(tx, row)?;
            }
            Ok(serde_json::json!({"copied":copied(tx)?.len()}))
        }),
        Err(StorageError::CommitUncertain)
    );
    assert_eq!(
        pool.apply_operation(&namespace, &identity(), &control, |_| panic!(
            "original receipt must replay without effects"
        )),
        Ok(serde_json::json!({"copied":3}))
    );
    drop(pool);
    let pool = PgPool::open(factory.clone(), limits()).unwrap();
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert_eq!(copied(tx)?, expected);
        Ok(())
    })
    .unwrap();
    // Warm pooled connections cannot see another namespace's peers, including
    // colliding logical IDs. The singleton is not copied until its own peer.
    pool.transaction(&ns("copies\0é"), TxnKind::Write, &control, |tx| {
        insert(tx, &rows[0])?;
        assert!(copied(tx)?.is_empty());
        Ok(())
    })
    .unwrap();
    for row in rows.iter().take(2) {
        sqlite
            .execute("DELETE FROM usage_identity WHERE file_id=?1", [row.0])
            .unwrap();
        let expected = sqlite_copied(&sqlite);
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver
                .execute(
                    "DELETE FROM semon_pg_native_usage_identity WHERE file_id=$1",
                    &[&row.0],
                )
                .map_err(sqlstate::error)?;
            assert_eq!(copied(tx)?, expected);
            Ok(())
        })
        .unwrap();
    }
    drop(sqlite);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pg_native_copied_identity_concurrent_peers_are_serialized_in_owner_transaction() {
    let (pool, _) = fixture(|_| {});
    let barrier = Arc::new(Barrier::new(2));
    let handles = (1..=2)
        .map(|id| {
            let pool = pool.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                pool.transaction(
                    &ns("race\0🦀"),
                    TxnKind::Write,
                    &PgCancellation::default(),
                    |tx| insert(tx, &(id, "m\0🦀".into(), "r\0🦀".into())),
                )
                .unwrap();
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
    pool.transaction(
        &ns("race\0🦀"),
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            assert_eq!(copied(tx)?.len(), 2);
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn pg_native_copied_identity_trigger_and_function_drift_refuses_before_effects() {
    for ddl in [
        "ALTER TABLE semon_pg_native_usage_identity DISABLE TRIGGER semon_pg_native_identity_copies",
        "DROP TRIGGER semon_pg_native_identity_lock ON semon_pg_native_usage_identity",
        "ALTER FUNCTION semon_pg_native_identity_copies() SECURITY DEFINER",
        "ALTER FUNCTION semon_pg_native_identity_lock() RESET search_path",
        "CREATE OR REPLACE FUNCTION semon_pg_native_identity_copies() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN RETURN NULL; END; $$",
    ] {
        let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
        pool.transaction(
            &ns("warm"),
            TxnKind::ReadSnapshot,
            &PgCancellation::default(),
            |_| Ok(()),
        )
        .unwrap();
        factory.client().batch_execute(ddl).unwrap();
        assert_eq!(
            pool.apply_operation(
                &ns("warm"),
                &identity(),
                &PgCancellation::default(),
                |_| panic!("drift must refuse before receipt or native effects")
            ),
            Err(StorageError::ScopeChanged),
            "{ddl}"
        );
        assert!(matches!(
            PgPool::open(factory, limits()),
            Err(StorageError::ScopeChanged)
        ));
    }
}

#[test]
fn pg_native_tree_witness_bytea_rows_match_real_parser_sqlite_oracle() {
    use crate::events::{EventCache, scan_file, tree_rows::witness};
    use sha2::{Digest, Sha256};
    let root = std::env::temp_dir().join(format!(
        "pg93-witness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let cache_path = root.join("index.json");
    let mut cache = EventCache::open(&cache_path).unwrap();
    // Actual shared Claude parser rows; PG receives only typed Tree dimensions.
    let lines = [
        serde_json::json!({"type":"user","uuid":"u\0🦀","sessionId":"s🦀","cwd":"/w\0🦀","gitBranch":"b🦀","timestamp":"2026-09-29T00:00:00Z","message":{"role":"user","content":"first\0🦀"}}),
        serde_json::json!({"type":"assistant","uuid":"a\0🦀","sessionId":"s🦀","cwd":"/w\0🦀","gitBranch":"b🦀","timestamp":"2026-09-29T00:00:01Z","message":{"id":"m\0🦀","role":"assistant","model":"claude-🦀","content":[{"type":"tool_use","id":"toolu\0🦀","name":"Bash\0🦀","input":{}}],"usage":{"input_tokens":3,"output_tokens":2}}}),
    ];
    let path = root.join("1.jsonl");
    std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[1])).unwrap();
    scan_file(&path, "claude", &mut cache, &mut false).unwrap();
    cache.flush_cold_writes().unwrap();
    drop(cache);
    let sqlite = rusqlite::Connection::open(EventCache::path(&cache_path)).unwrap();
    let projection: i64 = sqlite
        .query_row(
            "SELECT projection_id FROM native_tree_labels ORDER BY projection_id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let strings = |sql: &str, columns: usize| -> Vec<Vec<String>> {
        let mut statement = sqlite.prepare(sql).unwrap();
        statement
            .query_map([projection], |r| {
                (0..columns).map(|i| r.get(i)).collect::<rusqlite::Result<_>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let labels = strings("SELECT label,value FROM native_tree_labels WHERE projection_id=?1", 2);
    let models = strings("SELECT model FROM native_tree_models WHERE projection_id=?1", 1);
    let tools: Vec<(String, Option<String>, bool)> = sqlite
        .prepare("SELECT tool_id,name,closed FROM native_tree_tools WHERE projection_id=?1")
        .unwrap()
        .query_map([projection], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(labels.iter().any(|l| l[1] == "/w\0🦀"), "fixture must execute actual cwd label");
    assert!(models.iter().any(|m| m[0] == "claude-🦀"));
    assert!(tools.iter().any(|t| t.0 == "toolu\0🦀"));
    // Independent oracle: the original SQLite json_array witness bytes.
    let mut oracle = Sha256::new();
    for sql in [
        "SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label",
        "SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY model",
        "SELECT json_array(tool_id,name) FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id",
    ] {
        oracle.update(sql.as_bytes());
        for row in strings(sql, 1) {
            oracle.update((row[0].len() as u64).to_be_bytes());
            oracle.update(row[0].as_bytes());
        }
    }
    let oracle = format!("{:x}", oracle.finalize());
    let (pool, _factory) = fixture(|bounds| bounds.max_connections = 1);
    let control = PgCancellation::default();
    let namespace = ns("witness\0🦀");
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let ns = tx.namespace.as_str().as_bytes().to_vec();
        // Insert in reverse order; the reader alone must establish byte order.
        for l in labels.iter().rev() {
            tx.driver.execute("INSERT INTO semon_pg_native_tree_labels(namespace,projection_id,label,value) VALUES($1,$2,$3,$4)", &[&ns, &projection, &l[0].as_bytes(), &l[1].as_bytes()]).map_err(sqlstate::error)?;
        }
        for m in models.iter().rev() {
            tx.driver.execute("INSERT INTO semon_pg_native_tree_models(namespace,projection_id,model) VALUES($1,$2,$3)", &[&ns, &projection, &m[0].as_bytes()]).map_err(sqlstate::error)?;
        }
        for t in tools.iter().rev() {
            tx.driver.execute("INSERT INTO semon_pg_native_tree_tools(namespace,projection_id,tool_id,name,closed) VALUES($1,$2,$3,$4,$5)", &[&ns, &projection, &t.0.as_bytes(), &t.1.as_deref().map(str::as_bytes), &t.2]).map_err(sqlstate::error)?;
        }
        Ok(())
    })
    .unwrap();
    let pg_witness = |namespace: &crate::storage::LogicalSourceNamespace| {
        pool.transaction(namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let text = |b: Vec<u8>| String::from_utf8(b).map_err(|_| StorageError::Unavailable);
            let mut families = Vec::new();
            for sql in [
                "SELECT label,value FROM semon_pg_native_tree_labels WHERE projection_id=$1 ORDER BY label",
                "SELECT model,model FROM semon_pg_native_tree_models WHERE projection_id=$1 ORDER BY model",
                "SELECT tool_id,name FROM semon_pg_native_tree_tools WHERE projection_id=$1 AND NOT closed AND name IS NOT NULL ORDER BY tool_id",
            ] {
                let mut rows = Vec::new();
                for row in tx.driver.query(sql, &[&projection]).map_err(sqlstate::error)? {
                    tx.check()?;
                    rows.push((text(row.get(0))?, text(row.get(1))?));
                }
                families.push(rows);
            }
            witness::digest(
                families[0].iter().map(|(k, v)| (k.as_str(), v.as_str())),
                families[1].iter().map(|(m, _)| m.as_str()),
                families[2].iter().map(|(k, v)| (k.as_str(), v.as_str())),
            )
            .map_err(|_| StorageError::Unavailable)
        })
    };
    assert_eq!(pg_witness(&namespace), Ok(oracle.clone()));
    // A warm connection in another namespace sees none of these rows.
    assert_ne!(pg_witness(&ns("witness\0é")), Ok(oracle));
}

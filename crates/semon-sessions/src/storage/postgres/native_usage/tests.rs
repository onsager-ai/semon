use super::*;
use crate::{events::{Ledger, Stat, store::native_usage::{self as shared, algorithm}}, storage::postgres::{PgCancellation, PgPool, Reconciliation, tests::{fixture, identity, limits, ns}, txn::TxnKind}};

fn ledger(offset: u64) -> Ledger {
    Ledger { stat: Stat { dev: 1, ino: 2, size: offset, modified_ns: 3, changed_ns: Some(4) }, offset, prefix: [5;32], tail: [6;32] }
}

fn rows(tx: &mut PgTxn<'_>) -> Result<Vec<Vec<String>>, StorageError> {
    let mut result = Vec::new();
    for query in [
        "SELECT source_id::text,encode(source_path,'hex'),convert_from(receipt,'UTF8'),checkpoint::text,parser_version::text,usage_ready::int::text FROM semon_pg_native_usage_sources ORDER BY source_id",
        "SELECT source_id::text,native_offset::text,encode(identity,'hex'),encode(payload,'hex'),encode(record,'hex'),encode(outcome,'hex'),encode(model_outcome,'hex') FROM semon_pg_native_usage_facts ORDER BY source_id,native_offset",
        "SELECT source_id::text,encode(scope,'hex'),encode(identity,'hex'),native_offset::text FROM semon_pg_native_usage_identities ORDER BY source_id,scope,identity",
        "SELECT source_id::text,encode(kind,'hex'),encode(dimension,'hex'),encode(value,'hex') FROM semon_pg_native_usage_dimensions ORDER BY source_id,kind,dimension",
    ] {
        for row in tx.driver.query(query, &[]).map_err(sqlstate::error)? {
            result.push((0..row.len()).map(|i| row.get(i)).collect());
        }
    }
    Ok(result)
}

#[test]
fn pg_native_usage_shared_parser_generation_dimensions_replay_restart_and_rollback() {
    let root = std::env::temp_dir().join(format!("pg95-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("codex.jsonl");
    let usage = |turn: &str, input: u64| serde_json::json!({"type":"token_usage_record","timestamp":"2026-09-29T00:00:01Z","payload":{"thread_id":"thread\0🦀","session_id":"session\0🦀","turn_id":turn,"response_id":format!("response-{turn}"),"usage":{"input_tokens":input,"output_tokens":3,"total_tokens":input+3,"cached_input_tokens":1,"cache_write_input_tokens":2,"reasoning_output_tokens":0}}});
    let body = format!("{}\n{}\n{}\n{}\n",serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5"}}),usage("one",11),usage("one",11),usage("one",12));
    std::fs::write(&path,&body).unwrap();
    let index = shared::parser_fixture(&path,"codex");
    assert_eq!(index.native_usage_observations.as_ref().unwrap().len(),3);
    assert!(!index.codex_native_usage.as_ref().unwrap().by_model.is_empty());
    let mut oracle = rusqlite::Connection::open_in_memory().unwrap();
    for ddl in shared::schema(false) { oracle.execute_batch(ddl).unwrap(); }
    let source_path = "same/path\0🦀";
    let first = ledger(body.len() as u64);
    assert_eq!(shared::sqlite_fixture(&mut oracle,source_path,None,&first,false,&index),1);
    let expected = sqlite_rows(&oracle);
    // The SQLite oracle is fully materialized before the PG operation phase.
    drop(oracle);
    let (pool,factory) = fixture(|bounds| bounds.max_connections=1);
    let namespace = ns("usage\0🦀");
    let control = PgCancellation::default();
    let apply = |tx: &mut PgTxn<'_>| {
        let mut effects = Usage::new(tx)?;
        let id = algorithm::begin(&mut effects,source_path,None,&first,false)?;
        algorithm::write(&mut effects,source_path,&first,&index)?;
        Ok(serde_json::json!({"source":id}))
    };
    assert_eq!(pool.apply_operation(&namespace,&identity(),&control,|tx| { apply(tx)?; Err(StorageError::Unavailable) }),Err(StorageError::Unavailable));
    assert_eq!(pool.reconcile(&namespace,&identity(),&control),Ok(Reconciliation::Absent));
    pool.transaction(&namespace,TxnKind::ReadSnapshot,&control,|tx| { assert!(rows(tx)?.is_empty()); Ok(()) }).unwrap();
    crate::storage::postgres::tests::LOSE_ACK.with(|flag|flag.set(true));
    assert_eq!(pool.apply_operation(&namespace,&identity(),&control,apply),Err(StorageError::CommitUncertain));
    assert_eq!(pool.apply_operation(&namespace,&identity(),&control,|_|panic!("receipt replay must have no effects")),Ok(serde_json::json!({"source":1})));
    pool.transaction(&namespace,TxnKind::Write,&control,|tx| {
        assert_eq!(rows(tx)?,expected);
        let before = rows(tx)?;
        let mut effects = Usage::new(tx)?;
        assert_eq!(algorithm::begin(&mut effects,source_path,Some(&first),&first,false)?,1);
        algorithm::write(&mut effects,source_path,&first,&index)?;
        assert_eq!(rows(tx)?,before);
        Ok(())
    }).unwrap();
    drop(pool);
    let pool = PgPool::open(factory,limits()).unwrap();
    pool.transaction(&namespace,TxnKind::ReadSnapshot,&control,|tx| { assert_eq!(rows(tx)?,expected); Ok(()) }).unwrap();
    pool.transaction(&ns("foreign\0🦀"),TxnKind::Write,&control,|tx| { assert!(rows(tx)?.is_empty()); apply(tx)?; Ok(()) }).unwrap();
    pool.transaction(&namespace,TxnKind::ReadSnapshot,&control,|tx| { assert_eq!(rows(tx)?,expected); Ok(()) }).unwrap();
    // Replacement creates a generation; append reuses it, with full u64 checkpoint.
    pool.transaction(&namespace,TxnKind::Write,&control,|tx| {
        let mut effects = Usage::new(tx)?;
        let next = ledger(u64::MAX);
        assert_eq!(algorithm::begin(&mut effects,source_path,Some(&first),&next,false)?,2);
        algorithm::write(&mut effects,source_path,&next,&index)?;
        assert_eq!(algorithm::begin(&mut effects,source_path,Some(&next),&next,true)?,2);
        Ok(())
    }).unwrap();
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
        result.extend(statement.query_map([],|r|(0..columns).map(|i|r.get(i)).collect::<rusqlite::Result<Vec<String>>>()).unwrap().map(Result::unwrap));
    }
    result
}

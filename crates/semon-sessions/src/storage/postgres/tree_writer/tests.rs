use super::*;
use crate::storage::postgres::{
    PgCancellation, PgPool,
    tests::{fixture, limits, ns},
    tree_checkpoint::{Snapshot, tests::{Fixture, OracleValue as Value, install, install_without_tree, oracle_witness, parser_fixture}},
    txn::TxnKind,
};

fn source(tx: &mut PgTxn<'_>, path: &str) -> Result<i64, StorageError> {
    Ok(tx.driver.query_one("SELECT source_id FROM semon_pg_native_usage_current WHERE namespace=$1 AND source_path=$2", &[&tx.namespace.as_str().as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?.get(0))
}
fn input(data: &Fixture) -> &Summary {
    data.sources.iter().find(|(path,_,_)|path==&data.path).unwrap().2.tree_summary.as_ref().unwrap()
}
fn stage_fixture(tx: &mut PgTxn<'_>, data: &Fixture) -> Result<(i64,String),StorageError> {
    let source = source(tx, &data.path)?;
    stage(tx,&data.path,&data.ledger,None,source,input(data))
}

// Compare actual PG rows against the independently produced SQLite IndexStore,
// normalizing only the physical projection ID. No input-Summary oracle.
fn assert_dimensions(tx: &mut PgTxn<'_>, data: &Fixture, id: i64) {
    let header = &data.rows.iter().find(|(f,_)|*f=="tree_headers").unwrap().1;
    let physical = header.iter().find(|row|row.iter().any(|(k,v)|k=="source_path" && v==&Value::Text(data.path.clone()))).unwrap();
    let oracle_id = physical.iter().find_map(|(k,v)|if k=="projection_id" {if let Value::Integer(n)=v {Some(*n)} else {None}} else {None}).unwrap();
    for (family,columns) in [("tree_labels",vec!["label","value"]),("tree_models",vec!["model"]),("tree_tools",vec!["tool_id","name","closed"]),("tree_usage_models",vec!["message_id","model"])] {
        let rows = tx.driver.query(&format!("SELECT {} FROM semon_pg_native_{family} WHERE namespace=$1 AND projection_id=$2 ORDER BY {}",columns.join(","),columns[0]),&[&tx.namespace.as_str().as_bytes(),&id]).unwrap();
        let actual = rows.iter().map(|r|columns.iter().enumerate().map(|(at,key)| {
            if *key=="closed" { Value::Integer(i64::from(r.get::<_,bool>(at))) }
            else {r.get::<_,Option<Vec<u8>>>(at).map(|v|Value::Text(String::from_utf8(v).unwrap())).unwrap_or(Value::Null)}
        }).collect::<Vec<_>>()).collect::<Vec<_>>();
        let mut expected = data.rows.iter().find(|(f,_)|*f==family).unwrap().1.iter().filter(|row|row.iter().any(|(k,v)|k=="projection_id" && v==&Value::Integer(oracle_id))).map(|row|columns.iter().map(|key|row.iter().find(|(k,_)|k==key).unwrap().1.clone()).collect::<Vec<_>>()).collect::<Vec<_>>();
        expected.sort_by(|a,b|match (&a[0],&b[0]) { (Value::Text(a),Value::Text(b))=>a.as_bytes().cmp(b.as_bytes()),_=>panic!("text dimension key") });
        assert_eq!(actual,expected,"{family}");
    }
}

#[test]
fn pg_tree_writer_real_parser_dimensions_witness_restart_completion_closed() {
    for harness in ["claude","codex"] {
        let data = parser_fixture(harness);
        let expected = oracle_witness(&data);
        let (pool,factory) = fixture(|_|{});
        let (id,digest) = pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
            install_without_tree(tx,&data)?;
            let staged = stage_fixture(tx,&data)?;
            assert_dimensions(tx,&data,staged.0);
            Ok(staged)
        }).unwrap();
        assert_eq!(digest,expected);
        // There is no scalar owner, so no complete/readable header or current
        // scalar may be manufactured, even for an otherwise valid Tree.
        assert!(Snapshot::restore(pool.clone(),ns("tree\0🦀"),&data.path,&data.ledger,PgCancellation::default()).is_err());
        drop(pool);
        let pool = PgPool::open(factory,limits()).unwrap();
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

fn state(tx: &mut PgTxn<'_>) -> Vec<(String,Vec<serde_json::Value>)> {
    // All native tables plus source-state, operation and receipt families; JSON
    // bytea encoding compares the exact persisted bytes without printing them.
    let tables = tx.driver.query("SELECT table_name FROM information_schema.tables WHERE table_schema=current_schema() AND table_name LIKE 'semon_pg_%' ORDER BY table_name",&[]).unwrap();
    tables.iter().map(|r| {
        let table: String = r.get(0);
        let rows = tx.driver.query(&format!("SELECT to_jsonb(t) FROM {table} t ORDER BY to_jsonb(t)::text"),&[]).unwrap().iter().map(|r|r.get(0)).collect();
        (table,rows)
    }).collect()
}

#[test]
fn pg_tree_writer_every_effect_fault_rolls_back_usage_dimensions_and_foreign_source() {
    let data = parser_fixture("claude");
    let (pool,_) = fixture(|_|{});
    for namespace in ["tree\0🦀","tree\0é"] {
        pool.transaction(&ns(namespace),TxnKind::Write,&PgCancellation::default(),|tx|install_without_tree(tx,&data)).unwrap();
    }
    let baseline = |namespace:&str|pool.transaction(&ns(namespace),TxnKind::ReadSnapshot,&PgCancellation::default(),|tx|Ok(state(tx))).unwrap();
    let own = baseline("tree\0🦀");
    let foreign = baseline("tree\0é");
    let mut boundaries = 0;
    for fault in 1..64 {
        FAIL_AFTER.with(|p|p.set(Some(fault)));
        let result = pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|stage_fixture(tx,&data));
        let remaining = FAIL_AFTER.with(|p|p.replace(None));
        if result.is_ok() {
            assert!(remaining.is_some());
            break;
        }
        boundaries += 1;
        assert_eq!(baseline("tree\0🦀"),own);
        assert_eq!(baseline("tree\0é"),foreign);
    }
    assert!(boundaries>=7,"must fault generation and each emitted dimension");
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_stale_source_ledger_namespace_and_readiness_refuse_before_effects() {
    let data = parser_fixture("codex");
    let (pool,_) = fixture(|_|{});
    pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|install_without_tree(tx,&data)).unwrap();
    pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
        let before = state(tx);
        let id = source(tx,&data.path)?;
        assert!(stage(tx,&data.path,&data.ledger,None,id+1,input(&data)).is_err());
        let mut wrong = data.ledger.clone();
        wrong.offset+=1;
        assert!(stage(tx,&data.path,&wrong,None,id,input(&data)).is_err());
        assert!(stage(tx,&data.path,&data.ledger,Some(&data.ledger),id,input(&data)).is_err());
        assert_eq!(state(tx),before);
        tx.driver.batch_execute("UPDATE semon_pg_native_usage_sources SET usage_ready=false").unwrap();
        let before = state(tx);
        assert!(stage(tx,&data.path,&data.ledger,None,id,input(&data)).is_err());
        assert_eq!(state(tx),before);
        Ok(())
    }).unwrap();
    assert!(pool.transaction(&ns("tree\0é"),TxnKind::Write,&PgCancellation::default(),|tx|stage_fixture(tx,&data)).is_err());
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_writer_witness_charges_before_decode_and_obeys_cancel_deadline() {
    let data = parser_fixture("claude");
    let (pool,_) = fixture(|_|{});
    let (id,_) = pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|{install_without_tree(tx,&data)?;stage_fixture(tx,&data)}).unwrap();
    for (rows,bytes) in [(0,usize::MAX),(usize::MAX,0)] {
        DECODED.with(|p|p.set(0));
        let control = selected_read::SelectedReadControl::default().with_limits(rows,bytes);
        let _scope = selected_read::Scope::enter(&control);
        assert!(pool.transaction(&ns("tree\0🦀"),TxnKind::ReadSnapshot,&PgCancellation::default(),|tx|persisted_witness(tx,id)).is_err());
        assert_eq!(DECODED.with(|p|p.get()),0);
    }
    pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx| {
        let before = state(tx);
        tx.deadline=std::time::Instant::now();
        assert!(stage_fixture(tx,&data).is_err());
        assert_eq!(state(tx),before);
        // Restore only this test transaction's artificial deadline.
        tx.deadline=std::time::Instant::now()+std::time::Duration::from_secs(1);
        Ok(())
    }).unwrap();
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert!(pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&cancelled,|tx|stage_fixture(tx,&data)).is_err());
    std::fs::remove_dir_all(data.root).unwrap();
}

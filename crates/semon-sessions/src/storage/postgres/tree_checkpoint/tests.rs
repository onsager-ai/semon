use super::*;
use crate::{events::{EventCache, scan_file}, storage::postgres::tests::{fixture, ns, limits}};
use rusqlite::types::Value;

struct Fixture {
    root: std::path::PathBuf,
    path: String,
    ledger: Ledger,
    oracle: Summary,
    rows: Vec<(&'static str, Vec<Vec<(String, Value)>>)>,
}
fn parser_fixture(harness: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!("pg100-{}-{}-{harness}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&root).unwrap();
    let cache_path = root.join("index.json");
    let path = root.join("one.jsonl");
    let body = match harness {
        "claude" => format!("{}\n{}\n{}\n", serde_json::json!({"type":"user","uuid":"u\0🦀","cwd":"/w\0🦀","gitBranch":"b'🦀","timestamp":"2026-09-29T00:00:00Z","message":{"role":"user","content":"Raw fixture"}}), serde_json::json!({"type":"assistant","uuid":"record\0🦀","timestamp":"2026-09-29T00:00:01Z","message":{"id":"message\0🦀","role":"assistant","model":"claude-🦀","content":[{"type":"tool_use","id":"tool\0🦀","name":"Bash'\0🦀","input":{}}],"usage":{"input_tokens":3,"output_tokens":2}}}), serde_json::json!({"type":"assistant","uuid":"r2","timestamp":"2026-09-29T00:00:02Z","message":{"id":"m2","role":"assistant","model":"claude-🦀","content":[{"type":"tool_use","id":"open\0🦀","name":"Read","input":{}}]}})),
        "codex" => format!("{}\n{}\n", serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5","cwd":"/w🦀"}}), serde_json::json!({"type":"token_usage_record","timestamp":"2026-09-29T00:00:01Z","payload":{"thread_id":"t\0🦀","session_id":"s\0🦀","turn_id":"turn","response_id":"response","usage":{"input_tokens":11,"output_tokens":3,"total_tokens":14,"cached_input_tokens":1,"reasoning_output_tokens":0}}})),
        "copilot" => format!("{}\n", serde_json::json!({"type":"session.start","id":"start","timestamp":"2026-09-29T00:00:00Z","data":{"sessionId":"s🦀","context":{"cwd":"/w🦀"}}})),
        _ => unreachable!(),
    };
    std::fs::write(&path, &body).unwrap();
    let mut cache = EventCache::open(&cache_path).unwrap();
    scan_file(&path, harness, &mut cache, &mut false).unwrap();
    // An independently parsed copy gives Claude copied-usage identities.
    if harness == "claude" {
        let peer = root.join("two.jsonl"); std::fs::write(&peer, &body).unwrap();
        scan_file(&peer, harness, &mut cache, &mut false).unwrap();
    }
    cache.flush_cold_writes().unwrap(); drop(cache);
    let mut cache = EventCache::open(&cache_path).unwrap();
    let index = scan_file(&path, harness, &mut cache, &mut false).unwrap();
    let mut oracle = index.tree_summary.clone().unwrap();
    oracle.load_shared_usage().unwrap();
    let sqlite = rusqlite::Connection::open(EventCache::path(&cache_path)).unwrap();
    let ledger = sqlite.query_row("SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1", [path.to_str().unwrap()], |r| Ok(Ledger {stat: crate::events::Stat {dev:r.get::<_,i64>(0)? as u64,ino:r.get::<_,i64>(1)? as u64,size:r.get::<_,i64>(2)? as u64,modified_ns:u128::from_be_bytes(r.get::<_,Vec<u8>>(3)?.try_into().unwrap()),changed_ns:r.get::<_,Option<Vec<u8>>>(4)?.map(|b|i128::from_be_bytes(b.try_into().unwrap()))},offset:r.get::<_,i64>(5)? as u64,prefix:r.get::<_,Vec<u8>>(6)?.try_into().unwrap(),tail:r.get::<_,Vec<u8>>(7)?.try_into().unwrap()})).unwrap();
    let mut rows = Vec::new();
    for family in ["files", "usage", "usage_identity", "usage_sources", "usage_current", "tree_generations", "tree_headers", "tree_models", "tree_labels", "tree_tools", "tree_usage_models"] {
        let table = if family.starts_with("usage_") && family != "usage_identity" || family.starts_with("tree_") { format!("native_{family}") } else { family.into() };
        let mut stmt = sqlite.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let names = stmt.column_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
        let data = stmt.query_map([], |r| names.iter().enumerate().map(|(i,n)| Ok((n.clone(), r.get(i)?))).collect::<rusqlite::Result<Vec<_>>>()).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        rows.push((family, data));
    }
    drop(sqlite); drop(cache);
    Fixture {root, path:path.to_str().unwrap().into(), ledger, oracle, rows}
}
// Test-only physical import of the independently persisted parser oracle rows.
// No SQLite connections survive into the PG operation/read phase.
fn install(tx: &mut PgTxn<'_>, fixture: &Fixture) -> Result<(), StorageError> {
    for (family, rows) in &fixture.rows {
        let table = format!("semon_pg_native_{family}");
        let columns = tx.driver.query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 AND column_name NOT IN ('namespace','namespace_digest') ORDER BY ordinal_position", &[&table]).map_err(sqlstate::error)?;
        for row in rows {
            let mut parameters: Vec<Option<Vec<u8>>> = vec![Some(tx.namespace.as_str().as_bytes().to_vec())];
            let mut names = vec!["namespace".to_string()]; let mut binds = vec!["$1".to_string()];
            for col in &columns {
                let name: String = col.get(0); let kind: String = col.get(1);
                let value = &row.iter().find(|(n,_)| *n==name).expect("same current physical column").1;
                let bytes = match value { Value::Null=>None,Value::Blob(v)=>Some(v.clone()),Value::Text(v)=>Some(v.as_bytes().to_vec()),Value::Integer(v)=>Some(if kind=="numeric" {(*v as u64).to_string()} else {v.to_string()}.into_bytes()),_=>panic!("unexpected fixture value") };
                parameters.push(bytes); names.push(name);
                let at=parameters.len(); binds.push(if kind=="bytea" {format!("${at}")} else {format!("convert_from(${at},'UTF8')::{kind}")});
            }
            let refs = parameters.iter().map(|p| p as &(dyn tokio_postgres::types::ToSql+Sync)).collect::<Vec<_>>();
            tx.driver.execute(&format!("INSERT INTO {table}({}) VALUES({})", names.join(","),binds.join(",")), &refs).map_err(sqlstate::error)?;
        }
    }
    Ok(())
}
fn restore(pool: &PgPool, data: &Fixture) -> Summary {
    Snapshot::restore(pool.clone(),ns("tree\0🦀"),&data.path,&data.ledger,PgCancellation::default()).unwrap()
}
fn assert_oracle(actual: &mut Summary, expected: &Summary) {
    assert_eq!(actual.first,expected.first); assert_eq!(actual.last,expected.last);
    assert_eq!(actual.cwd,expected.cwd); assert_eq!(actual.branch,expected.branch);
    assert_eq!(serde_json::to_value(&actual.codex_turn).unwrap(),serde_json::to_value(&expected.codex_turn).unwrap());
    assert_eq!(actual.models,expected.models); assert_eq!(actual.tools,expected.tools);
    actual.load_shared_usage().unwrap();
    assert_eq!(actual.usage_records,expected.usage_records);
    assert_eq!(serde_json::to_value(&actual.usage_by_id).unwrap(),serde_json::to_value(&expected.usage_by_id).unwrap());
    assert_eq!(actual.usage_models,expected.usage_models);
}
#[test]
fn pg_tree_checkpoint_real_parsers_restart_existing_summary_consumers() {
    for harness in ["claude","codex","copilot"] {
        let data=parser_fixture(harness);
        let (pool,factory)=fixture(|b|b.max_connections=1);
        pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|install(tx,&data)).unwrap();
        let mut summary=restore(&pool,&data); assert_oracle(&mut summary,&data.oracle);
        assert_eq!(summary.tool_name("missing").unwrap(),None);
        if harness=="claude" {assert_eq!(summary.tool_name("tool\0🦀").unwrap(),Some("Bash'\0🦀".into())); assert!(!summary.usage_records.is_empty());}
        drop(summary);drop(pool);
        let pool=PgPool::open(factory,limits()).unwrap();
        assert_oracle(&mut restore(&pool,&data),&data.oracle);
        assert!(Snapshot::restore(pool,ns("tree\0é"),&data.path,&data.ledger,PgCancellation::default()).is_err());
        std::fs::remove_dir_all(data.root).unwrap();
    }
}
#[test]
fn pg_tree_checkpoint_stale_authority_rebind_and_selected_limits() {
    let data=parser_fixture("claude");let (pool,_)=fixture(|b|b.max_connections=1);
    pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|install(tx,&data)).unwrap();
    let summary=restore(&pool,&data);let checkpoint=summary.stored.unwrap();
    let control=crate::selected_read::SelectedReadControl::default().with_limits(1,1);
    {let _scope=crate::selected_read::Scope::enter(&control);assert!(checkpoint.shared_usage().is_err());}
    let mut rebound=checkpoint.clone();rebound.bind_configured(crate::storage::StoreConfig{backend:crate::storage::StoreBackend::Sqlite{database:data.root.join("must-not-open.sqlite")},namespace:ns("tree\0🦀"),expected_stream_epoch:None});
    assert!(rebound.tool_name("missing").is_err());assert!(!data.root.join("must-not-open.sqlite").exists());
    for mutation in ["UPDATE semon_pg_native_files SET resume_at=resume_at+1", "UPDATE semon_pg_native_tree_headers SET projection_id=projection_id+100", "UPDATE semon_pg_native_tree_generations SET complete=false", "UPDATE semon_pg_native_usage_sources SET parser_version=parser_version+1", "UPDATE semon_pg_native_usage_sources SET receipt='bad'::bytea", "DELETE FROM semon_pg_native_usage_current"] {
        pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|{
            tx.driver.batch_execute("SAVEPOINT changed").map_err(sqlstate::error)?;
            tx.driver.batch_execute(mutation).map_err(sqlstate::error)?;
            // Independent reader cannot see an uncommitted mutation. Test the
            // exact same authority predicate inside this transaction instead.
            if let TreeCheckpoint::Pg(snapshot)=&checkpoint {assert!(snapshot.authority(tx,true).is_err());}
            tx.driver.batch_execute("ROLLBACK TO SAVEPOINT changed").map_err(sqlstate::error)?;Ok(())
        }).unwrap();
    }
    checkpoint.shared_usage().unwrap();
    pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|{tx.driver.batch_execute("UPDATE semon_pg_native_files SET size=size+1").map_err(sqlstate::error)}).unwrap();
    assert!(checkpoint.tool_name("missing").is_err());assert!(checkpoint.shared_usage().is_err());
    std::fs::remove_dir_all(data.root).unwrap();
}

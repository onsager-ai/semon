use super::*;
use crate::{events::{EventCache, scan_file, Stat}, storage::postgres::{PgCancellation, tests::{fixture, ns, identity}, txn::TxnKind}};
use rusqlite::types::Value;
use std::collections::BTreeMap;

type Cells = BTreeMap<String, Option<Vec<u8>>>;
const FAMILIES: &[&str] = &["files", "events", "signals", "usage", "usage_identity", "codex_usage", "tree_copied_identities"];
fn root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("pg107-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&root).unwrap(); root
}
fn ledger(db: &rusqlite::Connection, path: &str) -> Ledger {
    db.query_row("SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1",[path],|r|Ok(Ledger { stat:Stat {dev:r.get::<_,i64>(0)? as u64,ino:r.get::<_,i64>(1)? as u64,size:r.get::<_,i64>(2)? as u64,modified_ns:u128::from_be_bytes(r.get::<_,Vec<u8>>(3)?.try_into().unwrap()),changed_ns:r.get::<_,Option<Vec<u8>>>(4)?.map(|b|i128::from_be_bytes(b.try_into().unwrap()))},offset:r.get::<_,i64>(5)? as u64,prefix:r.get::<_,Vec<u8>>(6)?.try_into().unwrap(),tail:r.get::<_,Vec<u8>>(7)?.try_into().unwrap()})).unwrap()
}
fn body(harness: &str, append: bool) -> String {
    let mut rows = match (harness,append) {
        ("claude",false) => vec![serde_json::json!({"type":"user","uuid":"u","cwd":"/w\u{0}🦀","message":{"role":"user","content":"sanitized"}}),serde_json::json!({"type":"assistant","uuid":"record\u{0}🦀","message":{"id":"message\u{0}🦀","role":"assistant","model":"claude-🦀","content":[{"type":"tool_use","id":"tool\u{0}🦀","name":"Bash","input":{}}],"usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":1}}})],
        ("claude",true) => vec![serde_json::json!({"type":"user","uuid":"reply","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool\u{0}🦀","content":"sanitized"}]}})],
        ("codex",false) => vec![serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5","cwd":"/w🦀"}}),serde_json::json!({"type":"token_usage_record","timestamp":"2026-09-29T00:00:01Z","payload":{"thread_id":"t\u{0}🦀","session_id":"s","turn_id":"turn","response_id":"response","usage":{"input_tokens":11,"output_tokens":3,"total_tokens":14,"cached_input_tokens":1,"reasoning_output_tokens":0}}})],
        ("codex",true) => vec![serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"sanitized"}})],
        ("copilot",false) => vec![serde_json::json!({"type":"session.start","id":"start","timestamp":"2026-09-29T00:00:00Z","data":{"sessionId":"s🦀","context":{"cwd":"/w🦀"}}})],
        ("copilot",true) => vec![serde_json::json!({"type":"user.message","id":"u","timestamp":"2026-09-29T00:00:01Z","data":{"content":"sanitized"}})],
        _ => unreachable!(),
    };
    if harness == "codex" && !append {
        rows.push(serde_json::json!({"type":"compacted","timestamp":"2026-09-29T00:00:02Z"}));
        rows.push(serde_json::json!({"type":"event_msg","timestamp":"2026-09-29T00:00:03Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":19,"output_tokens":7,"cached_input_tokens":2,"reasoning_output_tokens":3,"total_tokens":26}}}}));
        rows.push(serde_json::json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"sanitized"}]}}));
    }
    rows.into_iter().map(|v|format!("{v}\n")).collect()
}
struct Phase { index:FileIndex, ledger:Ledger, rows:Vec<(String,Vec<Cells>)> }
fn phase(cache_path:&std::path::Path,path:&std::path::Path,harness:&str) -> Phase {
    let mut cache = EventCache::open(cache_path).unwrap();
    let index = scan_file(path,harness,&mut cache,&mut false).unwrap();
    cache.flush_cold_writes().unwrap(); drop(cache);
    let db = rusqlite::Connection::open(EventCache::path(cache_path)).unwrap();
    let ledger = ledger(&db,path.to_str().unwrap());
    let rows = FAMILIES.iter().map(|family| {
        let table = if *family == "tree_copied_identities" {"native_tree_copied_identities"} else {family};
        let mut stmt = db.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let names = stmt.column_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
        let data = stmt.query_map([],|r| names.iter().enumerate().filter(|(_,n)| n.as_str() != "file_id").map(|(i,n)| {
            let v:Value=r.get(i)?;
            let bytes=match v {Value::Null=>None,Value::Blob(v)=>Some(v),Value::Text(v)=>Some(v.into_bytes()),Value::Integer(v)=>Some(v.to_string().into_bytes()),_=>panic!("unexpected value")};
            Ok((n.clone(),bytes))
        }).collect::<rusqlite::Result<Cells>>()).unwrap().map(Result::unwrap).collect();
        ((*family).into(),data)
    }).collect();
    Phase {index,ledger,rows}
}
fn assert_rows(tx:&mut PgTxn<'_>,phase:&Phase) -> Result<(),StorageError> {
    for (family,expected) in &phase.rows {
        let table=format!("semon_pg_native_{family}");
        let columns=tx.driver.query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 AND column_name NOT IN ('namespace','namespace_digest','file_id') ORDER BY ordinal_position", &[&table]).map_err(sqlstate::error)?;
        let select=columns.iter().map(|c| { let n:String=c.get(0);let t:String=c.get(1); if t=="bytea" {n} else {format!("{n}::text")} }).collect::<Vec<_>>().join(",");
        let actual=tx.driver.query(&format!("SELECT {select} FROM {table} WHERE namespace=$1"), &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
        let mut actual=actual.into_iter().map(|r| columns.iter().enumerate().map(|(i,c)| {let n:String=c.get(0);let t:String=c.get(1);let v=if t=="bytea" {r.get::<_,Option<Vec<u8>>>(i)} else {r.get::<_,Option<String>>(i).map(String::into_bytes)};(n,v)}).collect::<Cells>()).collect::<Vec<_>>();
        let mut expected=expected.clone();
        for row in &mut expected { for c in &columns {let n:String=c.get(0);let t:String=c.get(1); if let Some(Some(v))=row.get_mut(&n) {if t=="boolean" {*v=if v==b"0" {b"false".to_vec()} else {b"true".to_vec()};} else if t=="numeric" {if let Ok(vs)=String::from_utf8(v.clone()) {if let Ok(i)=vs.parse::<i64>() {*v=(i as u64).to_string().into_bytes();}}}} }}
        actual.sort();expected.sort();assert_eq!(actual,expected,"family {family}");
    }
    Ok(())
}
#[test]
fn pg_native_core_real_parser_initial_append_replacement_restart_oracle() {
    for harness in ["claude","codex","copilot"] {
        let root=root();let path=root.join("source🦀.jsonl");let cache=root.join("cache.json");
        std::fs::write(&path,body(harness,false)).unwrap(); let first=phase(&cache,&path,harness);
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(body(harness,true).as_bytes()).unwrap();let second=phase(&cache,&path,harness);
        let changes=DirtyRows::between(&first.index,&second.index);
        if harness == "claude" { assert!(first.index.events.iter().any(|e|e.r.is_none())); assert!(second.index.events.iter().any(|e|e.r.is_some()), "append must resolve the existing tool reply"); }
        if harness == "codex" { assert!(!first.index.signals.is_empty()); assert!(first.rows.iter().any(|(family,rows)|family=="codex_usage" && !rows.is_empty())); }

        std::fs::write(&path,body(harness,false)).unwrap();let replacement=phase(&cache,&path,harness);
        let (pool,factory)=fixture(|l|l.max_connections=1);let namespace=ns("core\0🦀");let control=PgCancellation::default();let path=path.to_str().unwrap();
        let id=pool.transaction(&namespace,TxnKind::Write,&control,|tx|{let id=write(tx,path,None,&first.ledger,&first.index,None)?;assert_rows(tx,&first)?;Ok(id)}).unwrap();
        pool.transaction(&namespace,TxnKind::Write,&control,|tx|{assert_eq!(write(tx,path,Some(&first.ledger),&second.ledger,&second.index,Some(&changes))?,id);assert_rows(tx,&second)}).unwrap();
        assert_eq!(pool.transaction(&namespace,TxnKind::Write,&control,|tx|write(tx,path,Some(&first.ledger),&second.ledger,&second.index,Some(&changes))),Err(StorageError::Conflict));
        pool.transaction(&namespace,TxnKind::Write,&control,|tx|{write(tx,path,Some(&second.ledger),&replacement.ledger,&replacement.index,None)?;assert_rows(tx,&replacement)}).unwrap();
        let limits=pool.inner.limits;drop(pool);let reopened=super::super::PgPool::open(factory,limits).unwrap();
        reopened.transaction(&namespace,TxnKind::ReadSnapshot,&control,|tx|assert_rows(tx,&replacement)).unwrap();
    }
}
#[test]
fn pg_native_core_every_effect_rollback_and_original_family_ack() {
    let root=root();let path=root.join("source.jsonl");let cache=root.join("cache.json");std::fs::write(&path,body("claude",false)).unwrap();let first=phase(&cache,&path,"claude");
    let (pool,_)=fixture(|l|l.max_connections=1);let namespace=ns("fault-core\0🦀");let control=PgCancellation::default();let path=path.to_str().unwrap();
    // Sweep each mutation/mid-row boundary until the first non-failing run.
    let mut completed=false;
    for point in 1..100 {
        FAIL_AFTER.with(|f|f.set(Some(point)));
        let result=pool.transaction(&namespace,TxnKind::Write,&control,|tx|write(tx,path,None,&first.ledger,&first.index,None));
        FAIL_AFTER.with(|f|f.set(None));
        if result.is_ok() { completed=true;break; }
        assert_eq!(result,Err(StorageError::Unavailable));
        pool.transaction(&namespace,TxnKind::ReadSnapshot,&control,|tx| {for family in FAMILIES {let n:i64=tx.driver.query_one(&format!("SELECT count(*) FROM semon_pg_native_{family} WHERE namespace=$1"),&[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0);assert_eq!(n,0,"rollback {family} point {point}");}Ok(())}).unwrap();
    }
    assert!(completed);
    // This durable receipt is explicitly core-family-only, never native-ingest ACK.
    super::super::tests::LOSE_ACK.with(|f|f.set(true));
    let op=identity();
    assert_eq!(pool.apply_operation(&namespace,&op,&control,|tx| {write(tx,path,Some(&first.ledger),&first.ledger,&first.index,None)?;Ok(serde_json::json!({"core_family_only":true}))}),Err(StorageError::CommitUncertain));
    pool.apply_operation(&namespace,&op,&control,|_|panic!("original family receipt must prevent Raw or writer replay")).unwrap();
}

fn snapshot(tx:&mut PgTxn<'_>) -> Result<Vec<(String,Vec<String>)>,StorageError> {
    FAMILIES.iter().map(|family| {
        let rows=tx.driver.query(&format!("SELECT row_to_json(t)::text FROM semon_pg_native_{family} t WHERE namespace=$1 ORDER BY row_to_json(t)::text"),&[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
        Ok((family.to_string(),rows.into_iter().map(|r|r.get(0)).collect()))
    }).collect()
}
#[test]
fn pg_native_core_replacement_every_boundary_preserves_existing_bytes() {
    let root=root();let path=root.join("source.jsonl");let cache=root.join("cache.json");std::fs::write(&path,body("claude",false)).unwrap();let first=phase(&cache,&path,"claude");
    let (pool,_)=fixture(|_|{});let namespace=ns("replace-fault\0🦀");let control=PgCancellation::default();let path=path.to_str().unwrap();
    pool.transaction(&namespace,TxnKind::Write,&control,|tx|write(tx,path,None,&first.ledger,&first.index,None)).unwrap();
    let before=pool.transaction(&namespace,TxnKind::ReadSnapshot,&control,snapshot).unwrap();
    let mut next=first.ledger.clone();next.offset+=1;next.stat.size+=1;next.tail=[42;32];
    let mut completed=false;
    for point in 1..100 {
        FAIL_AFTER.with(|f|f.set(Some(point)));
        let result=pool.transaction(&namespace,TxnKind::Write,&control,|tx|write(tx,path,Some(&first.ledger),&next,&first.index,None));
        FAIL_AFTER.with(|f|f.set(None));
        if result.is_ok() {completed=true;break;}
        assert_eq!(result,Err(StorageError::Unavailable));
        assert_eq!(pool.transaction(&namespace,TxnKind::ReadSnapshot,&control,snapshot).unwrap(),before,"boundary {point}");
    }
    assert!(completed);
}
#[test]
fn pg_native_core_unsigned_nul_and_copied_identity_foreign_warm_pool() {
    let root=root();let path=root.join("source.jsonl");let cache=root.join("cache.json");std::fs::write(&path,body("claude",false)).unwrap();let mut first=phase(&cache,&path,"claude");
    let edge=Ledger {stat:Stat {dev:u64::MAX,ino:u64::MAX-1,size:u64::MAX,modified_ns:u128::MAX,changed_ns:Some(i128::MIN)},offset:u64::MAX,prefix:[1;32],tail:[2;32]};
    first.index.events[0].o=u64::MAX;
    let (pool,_)=fixture(|l|l.max_connections=1);let own=ns("own\0🦀");let foreign=ns("foreign\0🦀");let control=PgCancellation::default();let path="same'\0🦀";
    let foreign_before=pool.transaction(&foreign,TxnKind::Write,&control,|tx| {write(tx,path,None,&edge,&first.index,None)?;snapshot(tx)}).unwrap();
    pool.transaction(&own,TxnKind::Write,&control,|tx| {
        let id=write(tx,path,None,&edge,&first.index,None)?;
        let r=tx.driver.query_one("SELECT dev::text,ino::text,size::text,resume_at::text,mtime_ns,ctime_ns,path FROM semon_pg_native_files WHERE namespace=$1 AND file_id=$2",&[&own.as_str().as_bytes(),&id]).map_err(sqlstate::error)?;
        for (i,v) in [u64::MAX,u64::MAX-1,u64::MAX,u64::MAX].iter().enumerate() {assert_eq!(r.get::<_,String>(i),v.to_string());}
        assert_eq!(r.get::<_,Vec<u8>>(4),u128::MAX.to_be_bytes());assert_eq!(r.get::<_,Vec<u8>>(5),i128::MIN.to_be_bytes());assert_eq!(r.get::<_,Vec<u8>>(6),path.as_bytes());
        let offset:String=tx.driver.query_one("SELECT o::text FROM semon_pg_native_events WHERE namespace=$1 AND file_id=$2 AND NOT extra AND seq=0",&[&own.as_str().as_bytes(),&id]).map_err(sqlstate::error)?.get(0);assert_eq!(offset,u64::MAX.to_string());
        write(tx,"copy\0🦀",None,&edge,&first.index,None)?;
        let count:i64=tx.driver.query_one("SELECT count(*) FROM semon_pg_native_tree_copied_identities WHERE namespace=$1",&[&own.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0);assert_eq!(count,2);
        write(tx,"copy\0🦀",Some(&edge),&edge,&FileIndex::default(),None)?;
        let count:i64=tx.driver.query_one("SELECT count(*) FROM semon_pg_native_tree_copied_identities WHERE namespace=$1",&[&own.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0);assert_eq!(count,0);
        Ok(())
    }).unwrap();
    assert_eq!(pool.transaction(&foreign,TxnKind::ReadSnapshot,&control,snapshot).unwrap(),foreign_before);
    // Scoped static separation only: no claim of whole-backend instrumentation.
    assert!(!include_str!("../native_core.rs").contains("rusqlite"));
}
#[test]
fn pg_native_core_competing_append_exact_ledger_cas() {
    let root=root();let path=root.join("source.jsonl");let cache=root.join("cache.json");std::fs::write(&path,body("claude",false)).unwrap();let first=phase(&cache,&path,"claude");
    let (pool,_)=fixture(|_|{});let namespace=ns("core-race\0🦀");let path="race\0🦀";
    pool.transaction(&namespace,TxnKind::Write,&PgCancellation::default(),|tx|write(tx,path,None,&first.ledger,&first.index,None)).unwrap();
    let mut next=first.ledger.clone();next.offset+=1;next.stat.size+=1;
    let index=std::sync::Arc::new(first.index);let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles=(0..2).map(|_| {let (pool,namespace,index,old,next,barrier)=(pool.clone(),namespace.clone(),index.clone(),first.ledger.clone(),next.clone(),barrier.clone());std::thread::spawn(move || {barrier.wait();pool.transaction(&namespace,TxnKind::Write,&PgCancellation::default(),|tx|write(tx,path,Some(&old),&next,&index,Some(&DirtyRows::default())))})}).collect::<Vec<_>>();
    let results=handles.into_iter().map(|h|h.join().unwrap()).collect::<Vec<_>>();assert_eq!(results.iter().filter(|r|r.is_ok()).count(),1);assert_eq!(results.iter().filter(|r|**r==Err(StorageError::Conflict)).count(),1);
}

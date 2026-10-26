use super::*;
use crate::{events::{EventCache,FileIndex},storage::{native_events::NativeEventStore,StoreBackend,StoreConfig}};
use std::sync::atomic::{AtomicUsize,Ordering};

static SQLITE_OPENS:AtomicUsize=AtomicUsize::new(0);
unsafe extern "C" fn deny_sqlite(_: *mut rusqlite::ffi::sqlite3,_:*mut *mut std::ffi::c_char,_:*const rusqlite::ffi::sqlite3_api_routines)->std::ffi::c_int{
    SQLITE_OPENS.fetch_add(1,Ordering::SeqCst);rusqlite::ffi::SQLITE_AUTH
}
struct Guard;
impl Guard {fn new()->Self{
    assert_eq!(unsafe{rusqlite::ffi::sqlite3_auto_extension(Some(deny_sqlite))},rusqlite::ffi::SQLITE_OK);
    std::thread::spawn(||assert!(rusqlite::Connection::open_in_memory().is_err())).join().unwrap();
    assert_eq!(SQLITE_OPENS.swap(0,Ordering::SeqCst),1);Self
}}
impl Drop for Guard {fn drop(&mut self){
    unsafe{rusqlite::ffi::sqlite3_cancel_auto_extension(Some(deny_sqlite));}
    assert_eq!(SQLITE_OPENS.load(Ordering::SeqCst),0,"PG phase attempted SQLite open");
}}

fn fingerprint(index:&FileIndex)->serde_json::Value{
    serde_json::json!({
        "events":index.events,"extras":index.extras,"signals":index.signals,
        "pending":index.pending,"tool_ids":index.tool_ids,"yields":index.yields,
        "busy":index.busy,"links":index.links,"entrypoint":index.entrypoint,
        "title":index.title,"agent_name":index.agent_name,"last_model":index.last_model,
        "failed":index.failed,"first":index.first,"last":index.last,"cwd":index.cwd,"branch":index.branch,
        "usage":index.usage_by_id,"tokens":index.codex_tokens,"tokens_by_model":index.codex_tokens_by_model,
        "codex_usage":index.codex_usage_events,"native_usage":index.codex_native_usage,
        "copilot":index.copilot,"rate_limits":index.rate_limits,
        "marker":index.marker_index,"launch":index.handoff_index,"header":index.codex_header,"sidecar":index.claude_sidecar,
        "chunks":index.source_chunks.as_ref().map(|c|(&c.hashes,c.fingerprint())),
        "tree":index.tree_summary.as_ref().map(|s|s.compact(index.native_usage_observations.is_some())),
    })
}

#[test]
fn pg_native126_real_event_cache_sqlite_oracle_restart_and_exact_replay(){
    const CHILD:&str="SEMON_NATIVE126_CHILD";
    if std::env::var_os(CHILD).is_none(){
        let status=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","storage::postgres::native_ingest_tests::pg_native126_real_event_cache_sqlite_oracle_restart_and_exact_replay","--nocapture"]).env(CHILD,"1").status().unwrap();
        assert!(status.success(),"isolated actual PG native pipeline");return;
    }
    struct Case {root:std::path::PathBuf,path:std::path::PathBuf,harness:&'static str,body:Vec<u8>,oracle:serde_json::Value,consumed:u64}
    let mut cases=Vec::new();
    for harness in ["claude","codex","copilot"]{
        let (root,body)=if harness=="copilot"{
            let root=std::env::temp_dir().join(format!("native126-copilot-{}",std::process::id()));std::fs::create_dir_all(&root).unwrap();
            (root,format!("{}\n",serde_json::json!({"id":"start","type":"session.start","timestamp":"2026-10-26T00:00:00Z","data":{"sessionId":"copilot126","context":{"cwd":"/sanitized"}}})).into_bytes())
        }else{
            let data=tree_checkpoint::tests::parser_fixture(harness);
            let body=std::fs::read(data.root.join("one.jsonl")).unwrap();(data.root,body)
        };
        let path=root.join("raw126.jsonl");std::fs::write(&path,&body).unwrap();
        let config=StoreConfig {backend:StoreBackend::Sqlite {database:root.join("current126.sqlite3")},namespace:tests::ns("native126")};
        let cap=crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap();
        let mut sqlite=EventCache::open_semantic(cap).unwrap();
        let receipt=sqlite.ingest(&"1".repeat(64),&path,harness).unwrap();
        let oracle=crate::events::scan_file(&path,harness,&mut sqlite,&mut false).unwrap();
        let oracle=fingerprint(&oracle);drop(sqlite);
        cases.push(Case {root,path,harness,body,oracle,consumed:receipt.consumed_bytes});
    }
    let _guard=Guard::new();
    for case in cases{
        let (pool,factory)=tests::fixture(|_|{});let namespace=tests::ns("native126");let control=PgCancellation::default();
        let mut pg=EventCache::open_pg_native(pool.clone(),namespace.clone(),control.clone()).unwrap();
        let id="1".repeat(64);
        let receipt=pg.ingest(&id,&case.path,case.harness).unwrap();assert!(!receipt.replayed);assert_eq!(receipt.consumed_bytes,case.consumed);
        let index=crate::events::scan_file(&case.path,case.harness,&mut pg,&mut false).unwrap();
        assert!(fingerprint(&index)==case.oracle,"actual native metadata/usage/Tree/aux/proofs differ for {}",case.harness);
        drop(index);drop(pg);
        let mut restarted=EventCache::open_pg_native(pool.clone(),namespace.clone(),control.clone()).unwrap();
        let noop=restarted.ingest(&"2".repeat(64),&case.path,case.harness).unwrap();assert_eq!(noop.consumed_bytes,receipt.consumed_bytes);
        assert!(restarted.ingest(&id,&case.path,"invalid").is_err());
        assert!(restarted.ingest(&id,&case.root.join("other"),case.harness).is_err());
        std::fs::write(&case.path,[case.body.as_slice(),b"incomplete tail"].concat()).unwrap();
        let replay=restarted.ingest(&id,&case.path,case.harness).unwrap();assert!(replay.replayed);assert_eq!(replay.consumed_bytes,receipt.consumed_bytes);
        std::fs::remove_file(&case.path).unwrap();
        drop(restarted);
        let mut recovered=EventCache::open_pg_native(pool,namespace,control).unwrap();
        let replay=recovered.ingest(&id,&case.path,case.harness).unwrap();assert!(replay.replayed);assert_eq!(replay.consumed_bytes,receipt.consumed_bytes);
        drop(recovered);drop(factory);
    }
}

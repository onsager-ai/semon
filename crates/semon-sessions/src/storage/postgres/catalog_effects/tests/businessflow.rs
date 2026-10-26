//! The ordinary native model must run after PG ingest, not be copied from the
//! SQLite oracle. Full admitted publication/selected qualification is separate.
use super::*;
use crate::events::EventCache;

#[test]
fn pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite() {
    const NAME: &str = "storage::postgres::catalog_effects::tests::businessflow::pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite";
    const CHILD: &str = "SEMON_VERTICAL140_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"]).env(CHILD,"1")
            .status().unwrap().success());
        return;
    }
    // Producer parity precedes the process-wide rejection phase. No model
    // output from the oracle is supplied to the PG producer.
    let oracle = build_oracle(true);
    let options = Options {
        claude_home: oracle.root.join("claude"),
        claude_json: oracle.root.join("claude.json"),
        codex_home: oracle.root.join("codex"),
        copilot_home: oracle.root.join("copilot"),
        proc_root: oracle.root.join("proc"),
        cache: oracle.root.join("never-open.sqlite3"),
        all: true,
        since: Duration::from_secs(86400),
        session: None, facts: None, scan_window: false,
    };
    let inputs = crate::inputs::inputs(&options).unwrap();
    let raw = inputs.iter().map(|i| fs::read(i.full_path(&options)).unwrap()).collect::<Vec<_>>();
    let _guard = crate::storage::postgres::native_ingest_tests::Guard::new();
    let (pool, _) = fixture(|_| {});
    let namespace = ns("vertical140\0🦀");
    let control = PgCancellation::default();
    let mut pg = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    for (at,input) in inputs.iter().enumerate() {
        pg.ingest(&format!("{:064x}",at+1), &input.full_path(&options), input.root.as_str()).unwrap();
    }
    let prepared = crate::model::prepare_sources(&options,&mut pg,&inputs,1790812802000).unwrap();
    assert_eq!(prepared.rows.len(), oracle.rows.len());
    assert_eq!(serde_json::to_value(&prepared.rows).unwrap(), serde_json::to_value(&oracle.rows).unwrap());
    assert_eq!(prepared.transcripts.len(), oracle.projections.len());
    for (actual,expected) in prepared.transcripts.iter().zip(&oracle.projections) {
        assert_eq!(serde_json::to_value(&actual.slots).unwrap(),serde_json::to_value(&expected.slots).unwrap());
    }
    let binding = pg.query_runtime_binding().unwrap().unwrap();
    drop(pg);
    let mut resumed = EventCache::open_pg_native(pool,namespace,control).unwrap();
    let after = resumed.query_runtime_binding().unwrap().unwrap();
    assert_eq!(binding.admission,after.admission);
    assert_eq!(binding.stream_epoch,after.stream_epoch);
    let repeated = crate::model::prepare_sources(&options,&mut resumed,&inputs,1790812802000).unwrap();
    assert_eq!(serde_json::to_value(&repeated.rows).unwrap(),serde_json::to_value(&prepared.rows).unwrap());
    for (input,original) in inputs.iter().zip(raw) {
        assert_eq!(fs::read(input.full_path(&options)).unwrap(),original);
    }
    assert!(!options.cache.exists());
}

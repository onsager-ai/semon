//! The ordinary native model must run after PG ingest, not be copied from the
//! SQLite oracle. Full admitted publication/selected qualification is separate.
use super::*;
use crate::events::EventCache;

fn producer_rows(rows: &[CatalogRow]) -> serde_json::Value {
    let mut value = serde_json::to_value(rows).unwrap();
    for row in value.as_array_mut().unwrap() {
        if let Some(query) = row.get_mut("query").and_then(serde_json::Value::as_object_mut) {
            query.remove("runtime");
        }
    }
    value
}

#[test]
fn pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite() {
    const NAME: &str = "storage::postgres::catalog_effects::tests::businessflow::pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite";
    const CHILD: &str = "SEMON_VERTICAL140_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD, "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    // Producer parity precedes the process-wide rejection phase. No model
    // output from the oracle is supplied to the PG producer.
    let mut oracle = build_oracle(true);
    let options = Options {
        claude_home: oracle.root.join("claude"),
        claude_json: oracle.root.join("claude.json"),
        codex_home: oracle.root.join("codex"),
        copilot_home: oracle.root.join("copilot"),
        proc_root: oracle.root.join("proc"),
        cache: oracle.root.join("never-open.sqlite3"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    let codex = options.codex_home.join("sessions/one.jsonl");
    let copilot = options.copilot_home.join("session-state/copilot140/events.jsonl");
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    fs::create_dir_all(copilot.parent().unwrap()).unwrap();
    fs::write(&codex, [
        serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"codex140","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z"}}),
        serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5","cwd":"/synthetic/project"}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized codex input"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(&copilot, [
        serde_json::json!({"id":"start","type":"session.start","timestamp":"2026-10-01T00:00:00Z","data":{"sessionId":"copilot140","context":{"cwd":"/synthetic/project"}}}),
        serde_json::json!({"id":"user","type":"user.message","timestamp":"2026-10-01T00:00:01Z","data":{"content":"sanitized copilot input"}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut sqlite = EventCache::open_scoped(&oracle.root.join("oracle-all140")).unwrap();
    let expected = crate::model::prepare_sources(&options, &mut sqlite, &inputs, 1790812802000).unwrap();
    assert_eq!(expected.rows.iter().map(|row| row.harness.as_str()).collect::<std::collections::BTreeSet<_>>(), std::collections::BTreeSet::from(["claude","codex","copilot"]));
    oracle.rows = expected.rows;
    oracle.projections = expected.transcripts;
    drop(sqlite);
    let raw = inputs
        .iter()
        .map(|i| fs::read(i.full_path(&options)).unwrap())
        .collect::<Vec<_>>();
    let _guard = crate::storage::postgres::native_ingest_tests::Guard::new();
    let (pool, _) = fixture(|_| {});
    let namespace = ns("vertical140\0🦀");
    let control = PgCancellation::default();
    let mut pg =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    for (at, input) in inputs.iter().enumerate() {
        pg.ingest(
            &format!("{:064x}", at + 1),
            &input.full_path(&options),
            input.root.as_str(),
        )
        .unwrap();
    }
    let prepared =
        crate::model::prepare_sources(&options, &mut pg, &inputs, 1790812802000).unwrap();
    assert_eq!(prepared.rows.len(), oracle.rows.len());
    assert_eq!(
        producer_rows(&prepared.rows),
        producer_rows(&oracle.rows)
    );
    assert_eq!(prepared.transcripts.len(), oracle.projections.len());
    for (actual, expected) in prepared.transcripts.iter().zip(&oracle.projections) {
        assert_eq!(
            serde_json::to_value(&actual.slots).unwrap(),
            serde_json::to_value(&expected.slots).unwrap()
        );
    }
    let binding = pg.query_runtime_binding().unwrap().unwrap();
    let runtime = binding.witness(&crate::native_evidence::query_runtime_generation(&options).unwrap()).unwrap();
    for row in &prepared.rows {
        if let Some(query) = &row.query {
            assert_eq!(query["runtime"].as_str(), Some(runtime.as_str()));
        }
    }
    drop(pg);
    let mut resumed = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let after = resumed.query_runtime_binding().unwrap().unwrap();
    assert_eq!(binding.admission, after.admission);
    assert_eq!(binding.stream_epoch, after.stream_epoch);
    let repeated =
        crate::model::prepare_sources(&options, &mut resumed, &inputs, 1790812802000).unwrap();
    assert_eq!(
        serde_json::to_value(&repeated.rows).unwrap(),
        serde_json::to_value(&prepared.rows).unwrap()
    );
    for (input, original) in inputs.iter().zip(&raw) {
        assert_eq!(&fs::read(input.full_path(&options)).unwrap(), original);
    }
    use std::io::Write;
    let path = inputs[0].full_path(&options);
    let append = format!("{}\n", serde_json::json!({"type":"user","sessionId":"one","uuid":"u2","parentUuid":"a1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":"sanitized continuation"}}));
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(append.as_bytes()).unwrap();
    let receipt = resumed.ingest(&format!("{:064x}",128), &path, "claude").unwrap();
    let continued = crate::model::prepare_sources(&options, &mut resumed, &inputs, 1790812803000).unwrap();
    assert_eq!(continued.rows.len(), prepared.rows.len());
    assert_eq!(continued.rows[0].key, prepared.rows[0].key);
    assert_eq!(receipt.consumed_bytes, raw[0].len() as u64 + append.len() as u64);
    assert_eq!(continued.rows[0].sources[0].offset, receipt.consumed_bytes);
    assert_eq!(fs::read(&path).unwrap(), [raw[0].as_slice(), append.as_bytes()].concat());
    assert!(!options.cache.exists());
    let (mut cache, _) = crate::storage::postgres::index_store::open(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let key = "opaque\0🦀".repeat(512);
    cache.save_session_descriptions(&[(key.clone(),1,"inputs".into(),"metadata".into())]).unwrap();
    assert_eq!(cache.session_description(&key,1,"inputs").unwrap().as_deref(),Some("metadata"));
    assert!(cache.session_description(&key,2,"inputs").unwrap().is_none());
    assert!(cache.session_description(&key,1,"other").unwrap().is_none());
    assert!(cache.save_session_descriptions(&[(key.clone(),1,"inputs".into(),"x".repeat(1024*1024))]).is_err());
    assert_eq!(cache.session_description(&key,1,"inputs").unwrap().as_deref(),Some("metadata"));
    let collision = ns("vertical140\0🦀other");
    let (other, _) = crate::storage::postgres::index_store::open(pool.clone(), collision.clone(), control.clone()).unwrap();
    assert!(other.session_description(&key,1,"inputs").unwrap().is_none());
    let other_binding = other.query_runtime_binding().unwrap().unwrap();
    assert_ne!(binding.stream_epoch,other_binding.stream_epoch);
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert!(matches!(crate::storage::postgres::index_store::open(pool, collision, cancelled),Err(StorageError::Busy)));
}

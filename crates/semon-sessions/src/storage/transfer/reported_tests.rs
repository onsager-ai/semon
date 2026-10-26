use crate::facts::ReportedRunSnapshot;
use crate::storage::native_events::NativeEventStore;

use super::*;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(crate) fn sqlite(root: &Path, name: &str) -> StoreConfig {
    let config = StoreConfig {
        backend: StoreBackend::Sqlite {
            database: root.join(format!("{name}.sqlite3")),
        },
        namespace: super::super::super::LogicalSourceNamespace::new("transfer225").unwrap(),
        expected_stream_epoch: Some("a".repeat(64)),
    };
    config.create_native_store().unwrap();
    crate::events::store::configured::NativeOwnership::admit(&config).unwrap();
    config
}

pub(crate) fn observe(config: &StoreConfig, input: &Path, start: i64, capture: i64) {
    std::fs::write(
        input,
        serde_json::json!({"projects":{"/sanitized":{
            "lastSessionId":"原生\0id", "lastStartTime":start,
            "lastDuration":u64::MAX, "lastCost":if start == 200 { None } else { Some(0.25) },
            "lastModelUsage":{"模型\0":{"inputTokens":u64::MAX,"outputTokens":31}}
        }}})
        .to_string(),
    )
    .unwrap();
    let mut cache = cache(config);
    cache
        .refresh_reported_runs(input, capture, &mut false)
        .unwrap();
}

pub(crate) fn read(config: &StoreConfig) -> Vec<ReportedRunSnapshot> {
    let mut cache = cache(config);
    cache
        .select_reported_runs(&BTreeSet::from(["原生\0id".into()]))
        .unwrap();
    cache.reported_runs().cloned().collect()
}

pub(crate) fn cache(config: &StoreConfig) -> crate::events::EventCache {
    match &config.backend {
        StoreBackend::Sqlite { .. } => crate::events::EventCache::open_semantic(
            super::super::super::sqlite::semantic::SemanticStaging::admit(config).unwrap(),
        )
        .unwrap(),
        #[cfg(feature = "postgres")]
        StoreBackend::Postgres { .. } => {
            crate::events::EventCache::open_configured(config).unwrap()
        }
    }
}

pub(crate) fn stage(
    source: &StoreConfig,
    target: &StoreConfig,
    operation: &[u8; 32],
) -> SnapshotManifest {
    let source = source.transfer_inbox();
    // Each call commits at most one bounded page; reconstruct the handle
    // between calls to exercise durable capture resumption.
    let manifest = loop {
        match source.capture(operation).unwrap() {
            CaptureProgress::Capturing { .. } => continue,
            CaptureProgress::Ready(manifest) => break manifest,
        }
    };
    let checkpoint = source.checkpoint(&manifest).unwrap();
    let target = target.transfer_inbox();
    target.begin(&manifest).unwrap();
    for sequence in 1..=checkpoint.sequence {
        let page = source.staged_page(&manifest, sequence).unwrap();
        assert_eq!(
            target.stage_page(&manifest, &page).unwrap(),
            target.stage_page(&manifest, &page).unwrap()
        );
    }
    manifest
}

pub(crate) fn root() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "transfer228-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

pub(crate) fn apply(target: &StoreConfig, manifest: &SnapshotManifest) {
    let pages = target
        .transfer_inbox()
        .checkpoint(manifest)
        .unwrap()
        .sequence;
    for sequence in if pages == 0 { 0..=0 } else { 1..=pages } {
        let first = target
            .transfer_inbox()
            .apply_page(manifest, sequence)
            .unwrap();
        assert_eq!(
            first,
            target
                .transfer_inbox()
                .apply_page(manifest, sequence)
                .unwrap()
        );
    }
}

#[test]
fn current_transfer225_reported_more_than_one_page_restart_reverse_and_fences() {
    let root = root();
    let source = sqlite(&root, "source");
    let target = sqlite(&root, "target");
    let reverse = sqlite(&root, "reverse");
    let input = root.join(".claude.json");
    for start in 1..=200 {
        observe(&source, &input, start, start + 1000);
    }
    let expected = read(&source);
    assert_eq!(expected.len(), 200);
    // Keep an already-open ordinary writer before the durable source freeze.
    let mut opened = cache(&source);
    assert!(matches!(
        source.transfer_inbox().capture(&[1; 32]).unwrap(),
        CaptureProgress::Capturing { .. }
    ));
    std::fs::write(&input,serde_json::json!({"projects":{"/sanitized":{"lastSessionId":"原生\0id","lastStartTime":400,"lastCost":0.5}}}).to_string()).unwrap();
    assert!(
        opened
            .refresh_reported_runs(&input, 2000, &mut false)
            .is_err()
    );
    match crate::storage::sqlite::semantic::SemanticStaging::admit(&source) {
        Err(_) => {}
        Ok(admitted) => match crate::events::EventCache::open_semantic(admitted) {
            Err(_) => {}
            Ok(mut future) => assert!(
                future
                    .refresh_reported_runs(&input, 2001, &mut false)
                    .is_err(),
                "future writers must not adopt a frozen epoch"
            ),
        },
    }
    let manifest = stage(&source, &target, &[1; 32]);
    let pages = target
        .transfer_inbox()
        .checkpoint(&manifest)
        .unwrap()
        .sequence;
    assert!(pages >= 5);
    for sequence in 1..=pages {
        let page = target
            .transfer_inbox()
            .staged_page(&manifest, sequence)
            .unwrap();
        assert!(page.records.len() <= MAX_PAGE_ROWS);
        assert!(page.encoded().unwrap().len() <= MAX_PAGE_BYTES);
    }
    let db = rusqlite::Connection::open(target.backend.sqlite_database().unwrap()).unwrap();
    db.execute_batch("CREATE TRIGGER transfer231_apply_fault BEFORE INSERT ON meta WHEN NEW.key GLOB 'native_transfer_v1:*:applied:*' BEGIN SELECT RAISE(ABORT,'injected page receipt failure'); END;").unwrap();
    assert!(target.transfer_inbox().apply_page(&manifest, 1).is_err());
    assert!(
        read(&target).is_empty(),
        "authority and incoming marker must roll back with the receipt"
    );
    db.execute_batch("DROP TRIGGER transfer231_apply_fault")
        .unwrap();
    drop(db);
    target.transfer_inbox().apply_page(&manifest, 1).unwrap();
    assert!(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&target).is_err(),
        "unfinished authority is not readable"
    );
    apply(&target, &manifest);
    assert_eq!(read(&target), expected);
    observe(&target, &input, 300, 1300);
    let postwrite = read(&target);
    assert_eq!(postwrite.len(), 201);
    let reverse_manifest = stage(&target, &reverse, &[2; 32]);
    apply(&reverse, &reverse_manifest);
    assert_eq!(read(&reverse), postwrite);
    let db = rusqlite::Connection::open(source.backend.sqlite_database().unwrap()).unwrap();
    let sql = format!(
        "SELECT {} FROM reported_runs WHERE namespace=?1 ORDER BY session_id,start",
        crate::events::store::RUN_COLUMNS
    );
    let original = db
        .prepare(&sql)
        .unwrap()
        .query_map([source.namespace.as_str()], crate::events::store::read_run)
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        original, expected,
        "reverse and rejected old writers must preserve every original source row"
    );
    drop(db);
    assert!(input.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_reported_authoritative_foreign_corrupt_and_occupied_nonmutation() {
    let root = root();
    let source = sqlite(&root, "source");
    let target = sqlite(&root, "target");
    let corrupt = sqlite(&root, "corrupt");
    let input = root.join(".claude.json");
    observe(&source, &input, 100, 101);
    observe(&source, &input, 200, 202);
    observe(&target, &input, 900, 901);
    let occupied = read(&target);
    let manifest = stage(&source, &target, &[6; 32]);
    assert!(matches!(
        target.transfer_inbox().apply_page(&manifest, 1),
        Err(StorageError::Conflict)
    ));
    assert_eq!(read(&target), occupied);
    let mut foreign = manifest.clone();
    foreign.namespace = "foreign".into();
    foreign.seal().unwrap();
    assert!(matches!(
        target.transfer_inbox().apply_page(&foreign, 1),
        Err(StorageError::NamespaceMismatch)
    ));
    assert_eq!(read(&target), occupied);
    let mut broken = manifest.clone();
    broken
        .categories
        .iter_mut()
        .find(|c| c.category == Category::ReportedFacts)
        .unwrap()
        .digest = [7; 32];
    broken.seal().unwrap();
    let inbox = corrupt.transfer_inbox();
    inbox.begin(&broken).unwrap();
    let checkpoint = source.transfer_inbox().checkpoint(&manifest).unwrap();
    for sequence in 1..=checkpoint.sequence {
        let page = source
            .transfer_inbox()
            .staged_page(&manifest, sequence)
            .unwrap();
        inbox.stage_page(&broken, &page).unwrap();
    }
    assert!(
        inbox.apply_page(&broken, 1).is_err(),
        "final digest mismatch must refuse before even the stamp becomes authoritative"
    );
    assert!(read(&corrupt).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_empty_authority_completes_without_leaving_incoming() {
    let root = root();
    let source = sqlite(&root, "empty-source");
    let target = sqlite(&root, "empty-target");
    let manifest = stage(&source, &target, &[91; 32]);
    assert!(
        manifest
            .categories
            .iter()
            .all(|category| category.rows == 0)
    );
    apply(&target, &manifest);
    assert!(read(&target).is_empty());
    observe(&target, &root.join(".claude.json"), 1, 10);
    assert_eq!(read(&target).len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn seed_native(config: &StoreConfig, path: &Path, count: usize, operation: char) {
    let body = (0..count).map(|i| format!("{}\n", serde_json::json!({"type":"user","uuid":format!("u{i}"),"message":{"role":"user","content":"sanitized"}}))).collect::<String>();
    std::fs::write(path, body).unwrap();
    let mut cache = cache(config);
    cache
        .ingest(&operation.to_string().repeat(64), path, "claude")
        .unwrap();
    cache.flush_cold_writes().unwrap();
}

pub(crate) fn seed_native_usage(config: &StoreConfig, path: &Path, count: usize) {
    let body=(0..count).map(|i| format!("{}\n",serde_json::json!({"type":"assistant","uuid":format!("r{i}"),"timestamp":"2026-01-01T00:00:00Z","message":{"id":format!("m{i}"),"role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"sanitized"}],"usage":{"input_tokens":11,"output_tokens":7,"cache_creation_input_tokens":3,"cache_read_input_tokens":2}}}))).collect::<String>();
    std::fs::write(path,body).unwrap();
    let mut cache=cache(config);cache.ingest(&"4".repeat(64),path,"claude").unwrap();cache.flush_cold_writes().unwrap();
}

pub(crate) fn sqlite_core(config: &StoreConfig, path: &Path) -> serde_json::Value {
    let cap = crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
    let store = crate::storage::sqlite::configured_index::open_semantic(cap).unwrap();
    let (ledger, index) = store.load_file(path.to_str().unwrap()).unwrap().unwrap();
    serde_json::json!({"ledger":crate::events::store::header_ledger(&ledger),"events":index.events,"extras":index.extras})
}

#[test]
fn current_transfer225_native_rows_real_ingest_paged_absent_raw_and_reverse() {
    let root = root();
    let source = sqlite(&root, "native-source");
    let target = sqlite(&root, "native-target");
    let reverse = sqlite(&root, "native-reverse");
    let path = root.join("native.jsonl");
    seed_native(&source, &path, 130, '1');
    let expected = sqlite_core(&source, &path);
    assert_eq!(expected["events"].as_array().unwrap().len(), 130);
    let manifest = stage(&source, &target, &[95; 32]);
    assert_eq!(
        manifest
            .categories
            .iter()
            .find(|c| c.category == Category::Events)
            .unwrap()
            .rows,
        130
    );
    std::fs::remove_file(&path).unwrap();
    apply(&target, &manifest);
    assert_eq!(sqlite_core(&target, &path), expected);
    let manifest = stage(&target, &reverse, &[96; 32]);
    apply(&reverse, &manifest);
    assert_eq!(sqlite_core(&reverse, &path), expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_usage_multi_page_absent_raw_exact_native_reader() {
    let root=root();let source=sqlite(&root,"usage-source");let target=sqlite(&root,"usage-target");let reverse=sqlite(&root,"usage-reverse");
    let path=root.join("usage.jsonl");seed_native_usage(&source,&path,130);
    let load=|config:&StoreConfig| {
        let cap=crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
        let store=crate::storage::sqlite::configured_index::open_semantic(cap).unwrap();
        let (ledger,index)=store.load_file(path.to_str().unwrap()).unwrap().unwrap();
        serde_json::json!({"ledger":crate::events::store::header_ledger(&ledger),"events":index.events,"extras":index.extras,"usage":index.usage(),"signals":index.signals,"chunks":index.source_chunks.as_ref().unwrap().hashes,"launch":index.handoff_index})
    };
    let expected=load(&source);assert_eq!(expected["usage"].as_object().unwrap().len(),130);
    let manifest=stage(&source,&target,&[99;32]);
    let pages=source.transfer_inbox().checkpoint(&manifest).unwrap().sequence;
    let mut message=0;let mut identities=0;
    for sequence in 1..=pages {let page=source.transfer_inbox().staged_page(&manifest,sequence).unwrap();assert!(page.records.len()<=MAX_PAGE_ROWS);for record in page.records {if let TransferRecord::NativeRow{kind,..}=record {match kind {super::super::NativeRowKind::MessageUsage=>message+=1,super::super::NativeRowKind::CopiedUsage=>identities+=1,_=>{}}}}}
    assert_eq!(message,130);assert_eq!(identities,130);
    std::fs::remove_file(&path).unwrap();apply(&target,&manifest);assert_eq!(load(&target),expected);
    let manifest=stage(&target,&reverse,&[100;32]);apply(&reverse,&manifest);assert_eq!(load(&reverse),expected);
    std::fs::remove_dir_all(root).unwrap();
}

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
            .unwrap_or_else(|error| panic!("apply page {sequence}/{pages} failed: {error:?}"));
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
    std::fs::write(path, body).unwrap();
    let mut cache = cache(config);
    cache.ingest(&"4".repeat(64), path, "claude").unwrap();
    cache.flush_cold_writes().unwrap();
}

pub(crate) fn seed_catalog(
    config: &StoreConfig,
    root: &Path,
) -> (crate::Options, Vec<crate::model::summary::CatalogRow>) {
    let options = crate::Options {
        source: crate::storage::HostSource::new(
            config.namespace.clone(),
            config.expected_stream_epoch.clone().unwrap(),
        )
        .unwrap()
        .with_store(config.clone())
        .unwrap(),
        claude_home: root.join("claude"),
        claude_json: root.join("claude.json"),
        codex_home: root.join("codex"),
        copilot_home: root.join("copilot"),
        proc_root: root.join("proc"),
        cache: root.join("unused.sqlite3"),
        all: true,
        ..Default::default()
    };
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/sanitized/catalog.jsonl".into(),
    };
    let path = input.full_path(&options);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(options.proc_root.join("sys/kernel")).unwrap();
    std::fs::write(
        options.proc_root.join("sys/kernel/hostname"),
        "sanitized-transfer-host\n",
    )
    .unwrap();
    let body = (0..130).map(|i| format!("{}\n",serde_json::json!({"type":"user","sessionId":"catalog","uuid":format!("u{i}"),"parentUuid":if i==0 {None} else {Some(format!("u{}",i-1))},"timestamp":"2026-10-01T00:00:00Z","cwd":"/sanitized","message":{"role":"user","content":format!("sanitized {i}")}}))).collect::<String>();
    std::fs::write(&path, body).unwrap();
    let mut cache = cache(config);
    cache.ingest(&"7".repeat(64), &path, "claude").unwrap();
    cache.flush_cold_writes().unwrap();
    let paths = vec![path.to_str().unwrap().to_owned()];
    assert_eq!(
        cache
            .begin_semantic_stage_with_journal(&paths, "transfer-real-model", true)
            .unwrap(),
        crate::events::Outcome::Written
    );
    for _ in 0..256 {
        let batch = cache.semantic_batch(64).unwrap().unwrap();
        if batch.paths.is_empty() {
            continue;
        }
        let model = cache
            .prepare_semantic_model_batch(
                &options,
                std::slice::from_ref(&input),
                &batch,
                1_790_000_000_000,
            )
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            assert!(!model.prepared().rows.is_empty());
            assert!(
                model
                    .prepared()
                    .transcripts
                    .iter()
                    .map(|p| p.slots.len())
                    .sum::<usize>()
                    > 64
            );
            assert_eq!(
                cache
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                crate::events::Outcome::Written
            );
            let key = &model.prepared().rows[0].key;
            cache.save_session_description(
                key,
                1,
                "transfer-inputs",
                "sanitized transfer description",
            );
            cache.publish_session_descriptions();
            assert_eq!(
                cache
                    .session_description(key, 1, "transfer-inputs")
                    .as_deref(),
                Some("sanitized transfer description")
            );
            return (options, model.prepared().rows.clone());
        }
    }
    panic!("real source Catalog did not publish");
}

#[test]
fn current_transfer225_real_catalog_materializes_and_reverses_without_granting_publication() {
    let root = root();
    let source = sqlite(&root, "catalog-source");
    let target = sqlite(&root, "catalog-target");
    let reverse = sqlite(&root, "catalog-reverse");
    let (options, expected) = seed_catalog(&source, &root);
    let manifest = stage(&source, &target, &[118; 32]);
    assert!(
        manifest
            .categories
            .iter()
            .find(|c| c.category == Category::Slots)
            .unwrap()
            .rows
            > 64
    );
    std::fs::remove_dir_all(&options.claude_home).unwrap();
    apply(&target, &manifest);
    assert_eq!(
        cache(&target)
            .session_description(&expected[0].key, 1, "transfer-inputs")
            .as_deref(),
        Some("sanitized transfer description")
    );
    let owner =
        crate::events::store::configured::NativeOwnership::restore_transfer(&target).unwrap();
    let db = crate::storage::sqlite::source_journal::SourceJournal::owned(owner)
        .connection()
        .unwrap();
    let actual = crate::storage::sqlite::catalog::rows(
        &db,
        &crate::catalog::Request::parse("limit=60").unwrap(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert!(
        target.catalog_generation().is_err(),
        "content transfer cannot manufacture publication authority"
    );
    let backwards = stage(&target, &reverse, &[119; 32]);
    for category in [
        Category::CatalogCurrent,
        Category::Recipes,
        Category::Slots,
        Category::Trace,
        Category::Search,
    ] {
        assert_eq!(
            manifest
                .categories
                .iter()
                .find(|c| c.category == category)
                .unwrap(),
            backwards
                .categories
                .iter()
                .find(|c| c.category == category)
                .unwrap()
        );
    }
    apply(&reverse, &backwards);
    assert_eq!(
        cache(&reverse)
            .session_description(&expected[0].key, 1, "transfer-inputs")
            .as_deref(),
        Some("sanitized transfer description")
    );
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn sqlite_core(config: &StoreConfig, path: &Path) -> serde_json::Value {
    let cap = crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
    let store = crate::storage::sqlite::configured_index::open_semantic(cap).unwrap();
    let (ledger, index) = store.load_file(path.to_str().unwrap()).unwrap().unwrap();
    serde_json::json!({"ledger":crate::events::store::header_ledger(&ledger),"events":index.events,"extras":index.extras})
}

#[test]
fn current_transfer225_unavailable_usage_zero_checkpoint_keeps_exact_chunk_ledger() {
    let root = root();
    let source = sqlite(&root, "usage-unavailable-source");
    let target = sqlite(&root, "usage-unavailable-target");
    let path = root.join("unavailable.jsonl");
    seed_native(&source, &path, 130, '8');
    let cap = crate::storage::sqlite::semantic::SemanticStaging::admit(&source).unwrap();
    let store = crate::storage::sqlite::configured_index::open_semantic(cap).unwrap();
    let (ledger, _) = store.load_file(path.to_str().unwrap()).unwrap().unwrap();
    drop(store);
    let manifest = stage(&source, &target, &[122; 32]);
    let pages = source
        .transfer_inbox()
        .checkpoint(&manifest)
        .unwrap()
        .sequence;
    let mut witnessed = false;
    for sequence in 1..=pages {
        for record in source
            .transfer_inbox()
            .staged_page(&manifest, sequence)
            .unwrap()
            .records
        {
            if let TransferRecord::NativeSource(mut native) = record {
                assert!(!native.usage_ready);
                assert_eq!(native.checkpoint, 0);
                assert!(ledger.offset > 0);
                native_pages::validate_source_ledger(&native, &ledger).unwrap();
                let mut header: serde_json::Value =
                    serde_json::from_str(&native.chunks_header).unwrap();
                header["consumed_offset"] = (ledger.offset + 1).into();
                native.chunks_header = serde_json::to_string(&header).unwrap();
                assert!(native_pages::validate_source_ledger(&native, &ledger).is_err());
                witnessed = true;
            }
        }
    }
    assert!(witnessed);
    apply(&target, &manifest);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_occupied_description_refused_before_native_pages_commit() {
    let root = root();
    let source = sqlite(&root, "occupied-source");
    let target = sqlite(&root, "occupied-target");
    let path = root.join("native.jsonl");
    seed_native_usage(&source, &path, 130);
    let mut writer = cache(&target);
    writer.save_session_description("existing", 1, "inputs", "owned destination");
    writer.publish_session_descriptions();
    assert_eq!(
        writer
            .session_description("existing", 1, "inputs")
            .as_deref(),
        Some("owned destination")
    );
    drop(writer);
    let manifest = stage(&source, &target, &[122; 32]);
    assert_eq!(
        target.transfer_inbox().apply_page(&manifest, 1),
        Err(StorageError::Conflict)
    );
    let owner =
        crate::events::store::configured::NativeOwnership::restore_transfer(&target).unwrap();
    let db = crate::storage::sqlite::source_journal::SourceJournal::owned(owner)
        .connection()
        .unwrap();
    assert_eq!(
        crate::events::store::read_session_description(&db, "existing", 1, "inputs")
            .unwrap()
            .as_deref(),
        Some("owned destination")
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM main.files WHERE namespace=?1",
            [target.namespace.as_str()],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
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
    let root = root();
    let source = sqlite(&root, "usage-source");
    let target = sqlite(&root, "usage-target");
    let reverse = sqlite(&root, "usage-reverse");
    let path = root.join("usage.jsonl");
    seed_native_usage(&source, &path, 130);
    let load = |config: &StoreConfig| {
        let cap = crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
        let store = crate::storage::sqlite::configured_index::open_semantic(cap).unwrap();
        let (ledger, index) = store.load_file(path.to_str().unwrap()).unwrap().unwrap();
        serde_json::json!({"ledger":crate::events::store::header_ledger(&ledger),"events":index.events,"extras":index.extras,"usage":index.usage(),"signals":index.signals.iter().map(|s|(&s.k,s.o,s.t,s.at,&s.n,s.v)).collect::<Vec<_>>(),"chunks":index.source_chunks.as_ref().unwrap().hashes,"launch":index.handoff_index})
    };
    let expected = load(&source);
    assert_eq!(expected["usage"].as_object().unwrap().len(), 130);
    let manifest = stage(&source, &target, &[99; 32]);
    let pages = source
        .transfer_inbox()
        .checkpoint(&manifest)
        .unwrap()
        .sequence;
    let mut message = 0;
    let mut identities = 0;
    for sequence in 1..=pages {
        let page = source
            .transfer_inbox()
            .staged_page(&manifest, sequence)
            .unwrap();
        assert!(page.records.len() <= MAX_PAGE_ROWS);
        for record in page.records {
            if let TransferRecord::NativeRow { kind, .. } = record {
                match kind {
                    super::super::NativeRowKind::MessageUsage => message += 1,
                    super::super::NativeRowKind::CopiedUsage => identities += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(message, 130);
    assert_eq!(identities, 130);
    std::fs::remove_file(&path).unwrap();
    apply(&target, &manifest);
    assert_eq!(load(&target), expected);
    let manifest = stage(&target, &reverse, &[100; 32]);
    apply(&reverse, &manifest);
    assert_eq!(load(&reverse), expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_normalized_usage_facts_cross_pages_and_reverse() {
    let root = root();
    let source = sqlite(&root, "normalized-source");
    let target = sqlite(&root, "normalized-target");
    let reverse = sqlite(&root, "normalized-reverse");
    let path = root.join("native-codex.jsonl");
    seed_codex_usage(&source, &path);
    let load = |config: &StoreConfig| {
        let cap = crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
        let reader = crate::storage::sqlite::configured_index::open_semantic(cap).unwrap();
        let (ledger, index) = reader.load_file(path.to_str().unwrap()).unwrap().unwrap();
        serde_json::json!({"ledger":crate::events::store::header_ledger(&ledger),"usage":index.codex_usage(),"events":index.codex_usage_events(),"chunks":index.source_chunks.as_ref().unwrap().hashes})
    };
    let expected = load(&source);
    assert!(!expected["usage"].as_object().unwrap().is_empty());
    let manifest = stage(&source, &target, &[101; 32]);
    let pages = source
        .transfer_inbox()
        .checkpoint(&manifest)
        .unwrap()
        .sequence;
    let mut facts = 0;
    for sequence in 1..=pages {
        for record in source
            .transfer_inbox()
            .staged_page(&manifest, sequence)
            .unwrap()
            .records
        {
            if matches!(
                record,
                TransferRecord::NativeRow {
                    kind: super::super::NativeRowKind::UsageFact,
                    ..
                }
            ) {
                facts += 1;
            }
        }
    }
    assert_eq!(facts, 70);
    std::fs::remove_file(&path).unwrap();
    apply(&target, &manifest);
    assert_eq!(load(&target), expected);
    let manifest = stage(&target, &reverse, &[102; 32]);
    apply(&reverse, &manifest);
    assert_eq!(load(&reverse), expected);
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn seed_codex_usage(config: &StoreConfig, path: &Path) {
    let mut body = format!(
        "{}\n",
        serde_json::json!({"type":"session_meta","payload":{"id":"transfer-thread","cwd":"/sanitized"}})
    );
    body.push_str(&format!(
        "{}\n",
        serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5"}})
    ));
    for i in 0..70 {
        body.push_str(&format!("{}\n",serde_json::json!({"type":"token_usage_record","payload":{"thread_id":"transfer-thread","session_id":"transfer-session","turn_id":format!("turn-{i}"),"response_id":format!("response-{i}"),"usage":{"input_tokens":11,"output_tokens":3,"total_tokens":14,"cached_input_tokens":1,"cache_write_input_tokens":2,"reasoning_output_tokens":0}}})));
    }
    std::fs::write(path, body).unwrap();
    let mut cache = cache(config);
    cache.ingest(&"5".repeat(64), path, "codex").unwrap();
    cache.flush_cold_writes().unwrap();
    drop(cache);
}

//! The ordinary native model must run after PG ingest, not be copied from the
//! SQLite oracle. Full admitted publication/selected qualification is separate.
use super::*;
use crate::events::EventCache;
use crate::storage::native_events::NativeEventStore;

fn producer_rows(rows: &[CatalogRow]) -> serde_json::Value {
    let mut value = serde_json::to_value(rows).unwrap();
    for row in value.as_array_mut().unwrap() {
        if let Some(query) = row
            .get_mut("query")
            .and_then(serde_json::Value::as_object_mut)
        {
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
    // Use the ordinary harness contracts, including Copilot's UUID directory
    // and versioned session.start payload (model/tests.rs fixtures).
    let codex_id = "01234567-89ab-4cde-8123-456789abcdef";
    let copilot_id = "11234567-89ab-4cde-8123-456789abcdef";
    let codex = options
        .codex_home
        .join(format!("sessions/2026/10/01/rollout-{codex_id}.jsonl"));
    let copilot = options
        .copilot_home
        .join(format!("session-state/{copilot_id}/events.jsonl"));
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    fs::create_dir_all(copilot.parent().unwrap()).unwrap();
    fs::write(&codex, [
        serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":codex_id,"cwd":"/synthetic/project","originator":"codex_exec","thread_source":"user","timestamp":"2026-10-01T00:00:00Z"}}),
        serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5","cwd":"/synthetic/project"}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized codex input"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(&copilot, [
        serde_json::json!({"id":"start","type":"session.start","timestamp":"2026-10-01T00:00:00Z","data":{"version":1,"copilotVersion":"1.0.91","sessionId":copilot_id,"context":{"cwd":"/synthetic/project"}}}),
        serde_json::json!({"id":"user","type":"user.message","timestamp":"2026-10-01T00:00:01Z","data":{"content":"sanitized copilot input"}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut sqlite = EventCache::open_scoped(&oracle.root.join("oracle-all140")).unwrap();
    let expected =
        crate::model::prepare_sources(&options, &mut sqlite, &inputs, 1790812802000).unwrap();
    assert_eq!(
        expected
            .rows
            .iter()
            .map(|row| row.harness.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["claude", "codex", "copilot"])
    );
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
    assert_eq!(producer_rows(&prepared.rows), producer_rows(&oracle.rows));
    assert_eq!(prepared.transcripts.len(), oracle.projections.len());
    for (actual, expected) in prepared.transcripts.iter().zip(&oracle.projections) {
        assert_eq!(
            serde_json::to_value(&actual.slots).unwrap(),
            serde_json::to_value(&expected.slots).unwrap()
        );
    }
    let binding = pg.query_runtime_binding().unwrap().unwrap();
    let runtime = binding
        .witness(&crate::native_evidence::query_runtime_generation(&options).unwrap())
        .unwrap();
    for row in &prepared.rows {
        if let Some(query) = &row.query {
            assert_eq!(query["runtime"].as_str(), Some(runtime.as_str()));
        }
    }
    // Drive the SAME ordinary queued producer against durable PG staging.
    let paths = inputs.iter().map(|input| input.full_path(&options).to_string_lossy().into_owned()).collect::<Vec<_>>();
    let stage_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    assert_eq!(pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(), crate::events::Outcome::Written);
    let mut stages = Vec::new();
    let (published, semantic_batch, semantic_model) = loop {
        let batch = pg.semantic_batch(64).unwrap().unwrap();
        stages.push(batch.phase);
        // Reopen mid-stage: resume must preserve the exact phase and epoch.
        if batch.phase == crate::model::relation_stage::Phase::Identity {
            drop(pg);
            pg = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
            assert_eq!(pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(), crate::events::Outcome::Written);
            let resumed = pg.semantic_batch(64).unwrap().unwrap();
            assert_eq!(resumed.epoch, batch.epoch);
            assert_eq!(resumed.phase, batch.phase);
        }
        let model = pg.prepare_semantic_model_batch(&options, &inputs, &batch, 1790812802000).unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let ack = pg.publish_semantic_catalog(&batch, model.prepared()).unwrap();
            assert_eq!(pg.publish_semantic_catalog(&batch, model.prepared()).unwrap(), ack);
            break (ack, batch, model);
        }
    };
    assert_eq!(stages.len(), 4);
    assert_eq!(producer_rows(&semantic_model.prepared().rows), producer_rows(&oracle.rows));
    assert_eq!(pg.finish_semantic_stage(semantic_batch.epoch).unwrap(), crate::events::Outcome::Written);
    assert_eq!(published.0, crate::events::Outcome::Written);
    let generation = published.1.unwrap();
    // A lost response retries the identical full request, even though its
    // expected head and acknowledged claims have advanced.
    assert_eq!(
        pg.publish_partial_catalog_fenced(&prepared)
            .unwrap()
            .1
            .as_deref(),
        Some(generation.as_str())
    );
    let mut competing = prepared.rows.clone();
    competing[0].name.push_str(" competing publisher");
    assert!(
        pg.publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &competing,
                transcripts: Some(&prepared.transcripts)
            },
            prepared.base_generation.as_deref()
        )
        .is_err()
    );
    let page = super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            crate::catalog::read_page_snapshot(
                &options,
                "",
                &crate::catalog::Request::parse("limit=20").unwrap(),
                snapshot,
            )
            .map_err(|_| StorageError::ScopeChanged)
        },
    )
    .unwrap();
    assert_eq!(page.status, 200);
    let page: serde_json::Value = serde_json::from_slice(&page.body).unwrap();
    assert!(page.to_string().contains(&prepared.rows[0].key));
    let query_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(), namespace: namespace.clone(), control: control.clone(),
        generation: generation.clone(), options: options.clone(),
    };
    let mut query = crate::Query::with_machines(vec![(namespace.as_str().into(), options.clone())]);
    let query_list = query.call_bound_catalog_at("list_sessions", &serde_json::json!({}), 1790812802000, &query_store).unwrap();
    assert_eq!(query_list["total"].as_u64(), Some(prepared.rows.len() as u64));
    super::super::snapshot::with_admitted_generation(&pool, &namespace, &control, &generation, |snapshot| {
        for row in &prepared.rows {
            let observation = snapshot.transcript_observation(&row.key, &generation, false).map_err(|_| StorageError::Unavailable)?.unwrap();
            assert!(observation["total"].as_u64().is_some());
            let details = snapshot.selected_details(&row.key, &row.sources, &mut |_| serde_json::json!({"observation":"unavailable"})).map_err(|_| StorageError::Unavailable)?;
            assert!(details.is_some());
        }
        Ok(())
    }).unwrap();
    // Only the request-owned host can grant fixture bytes. The PG reader adds
    // persisted native proof verification to the ordinary scalar-field parser.
    struct Host {
        source: crate::SessionSourceRef,
        bytes: std::sync::Arc<[u8]>,
    }
    impl crate::SessionSourceReader for Host {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            if source != &self.source
                || expected.is_some_and(|g| g != "authorized-fixture")
                || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX
            {
                return Err(std::io::Error::other("fixture authority mismatch"));
            }
            let at = usize::try_from(offset).map_err(std::io::Error::other)?;
            let end = self.bytes.len().min(at.saturating_add(max));
            let bytes = self
                .bytes
                .get(at..end)
                .ok_or_else(|| std::io::Error::other("fixture range"))?;
            Ok(crate::SessionSourceRange {
                generation: "authorized-fixture".into(),
                length: self.bytes.len() as u64,
                offset,
                bytes: std::sync::Arc::from(bytes),
                cached: false,
            })
        }
    }
    let (projection, slot) = prepared
        .transcripts
        .iter()
        .find_map(|projection| {
            projection
                .slots
                .iter()
                .find(|slot| {
                    matches!(slot.recipe, crate::slot_projection::Recipe::U)
                        && slot.field.is_some()
                        && slot.file.is_some()
                })
                .map(|slot| (projection, slot))
        })
        .expect("ordinary producer must capture a selected native user field");
    let selected = prepared
        .rows
        .iter()
        .find(|row| row.key == projection.key)
        .unwrap();
    let source = selected
        .sources
        .iter()
        .find(|source| source.path == slot.file.as_ref().unwrap().path)
        .unwrap();
    let input = inputs
        .iter()
        .find(|input| input.full_path(&options) == source.path)
        .unwrap();
    let reference = crate::SessionSourceRef {
        root: input.root.as_str().to_owned(),
        path: input.path.clone(),
        native_id: source.native_id.clone(),
        offset: source.offset,
        prefix_sha256: source.prefix_sha256,
        tail_sha256: source.tail_sha256,
    };
    let host = Host {
        source: reference.clone(),
        bytes: std::sync::Arc::from(fs::read(&source.path).unwrap()),
    };
    let field = slot.field.as_ref().unwrap();
    let expected_field =
        crate::native_field::preview(&host, &reference, field, "prompt", &mut (1024 * 1024))
            .unwrap();
    let read_selected = || {
        super::super::selected_reader::with_reader(
            &pool,
            &namespace,
            &control,
            &generation,
            source,
            &reference,
            &host,
            |reader| {
                crate::native_field::preview(
                    reader,
                    &reference,
                    field,
                    "prompt",
                    &mut (1024 * 1024),
                )
            },
        )
        .unwrap()
    };
    assert_eq!(read_selected(), expected_field);
    drop(pg);
    let mut resumed =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let after = resumed.query_runtime_binding().unwrap().unwrap();
    assert_eq!(binding.admission, after.admission);
    assert_eq!(binding.stream_epoch, after.stream_epoch);
    assert_eq!(
        resumed.session_catalog_generation().as_deref(),
        Some(generation.as_str())
    );
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
    let append = format!(
        "{}\n",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"u2","parentUuid":"a1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":"sanitized continuation"}})
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(append.as_bytes())
        .unwrap();
    let receipt = resumed
        .ingest(&format!("{:064x}", 128), &path, "claude")
        .unwrap();
    let continued =
        crate::model::prepare_sources(&options, &mut resumed, &inputs, 1790812803000).unwrap();
    assert_eq!(continued.rows.len(), prepared.rows.len());
    assert_eq!(continued.rows[0].key, prepared.rows[0].key);
    assert_eq!(
        receipt.consumed_bytes,
        raw[0].len() as u64 + append.len() as u64
    );
    assert_eq!(
        continued
            .rows
            .iter()
            .flat_map(|row| &row.sources)
            .find(|source| source.path == path)
            .unwrap()
            .offset,
        receipt.consumed_bytes
    );
    let continued_generation = resumed
        .publish_partial_catalog_fenced(&continued)
        .unwrap()
        .1
        .unwrap();
    assert_ne!(continued_generation, generation);
    assert_eq!(
        read_selected(),
        expected_field,
        "retained selected proof must survive a newer publication"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        [raw[0].as_slice(), append.as_bytes()].concat()
    );
    assert!(!options.cache.exists());
    let (mut cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let key = "opaque\0🦀".repeat(512);
    cache
        .save_session_descriptions(&[(key.clone(), 1, "inputs".into(), "metadata".into())])
        .unwrap();
    assert_eq!(
        cache
            .session_description(&key, 1, "inputs")
            .unwrap()
            .as_deref(),
        Some("metadata")
    );
    assert!(
        cache
            .session_description(&key, 2, "inputs")
            .unwrap()
            .is_none()
    );
    assert!(
        cache
            .session_description(&key, 1, "other")
            .unwrap()
            .is_none()
    );
    assert!(
        cache
            .save_session_descriptions(&[(
                key.clone(),
                1,
                "inputs".into(),
                "x".repeat(1024 * 1024)
            )])
            .is_err()
    );
    assert_eq!(
        cache
            .session_description(&key, 1, "inputs")
            .unwrap()
            .as_deref(),
        Some("metadata")
    );
    let collision = ns("vertical140\0🦀other");
    let (other, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        collision.clone(),
        control.clone(),
    )
    .unwrap();
    assert!(
        other
            .session_description(&key, 1, "inputs")
            .unwrap()
            .is_none()
    );
    let other_binding = other.query_runtime_binding().unwrap().unwrap();
    assert_ne!(binding.stream_epoch, other_binding.stream_epoch);
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert!(matches!(
        crate::storage::postgres::index_store::open(pool, collision, cancelled),
        Err(StorageError::Busy)
    ));
}

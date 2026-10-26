//! The ordinary native model must run after PG ingest, not be copied from the
//! SQLite oracle, admitted publication and lease-bound Query bodies/fields.
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
    use std::io::Write as _;
    writeln!(fs::OpenOptions::new().append(true).open(&codex).unwrap(), "{}",
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"related-spawn","arguments":"{\"task_name\":\"worker\",\"message\":\"go\"}"}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&codex).unwrap(), "{}",
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"related-spawn","output":"{\"task_name\":\"worker\"}"}})).unwrap();
    let child_id = "21234567-89ab-4cde-8123-456789abcdef";
    let child = options
        .codex_home
        .join(format!("sessions/2026/10/01/rollout-{child_id}.jsonl"));
    fs::write(&child, [
        serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":child_id,"cwd":"/synthetic/project","originator":"codex_exec","parent_thread_id":codex_id,"thread_source":"subagent","agent_path":"root/worker","agent_nickname":"worker","timestamp":"2026-10-01T00:00:00Z"}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized related codex input"}]}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"sanitized related codex reply"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    let inputs = crate::inputs::inputs(&options).unwrap();
    let mut sqlite_options = options.clone();
    sqlite_options.cache = oracle.root.join("oracle-all140");
    let mut sqlite = EventCache::open_scoped(&sqlite_options.cache).unwrap();
    let expected =
        crate::model::prepare_sources(&sqlite_options, &mut sqlite, &inputs, 1790812802000)
            .unwrap();
    assert_eq!(
        expected
            .rows
            .iter()
            .map(|row| row.harness.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["claude", "codex", "copilot"])
    );
    sqlite.publish_partial_catalog_fenced(&expected).unwrap();
    let expected_bodies = expected
        .rows
        .iter()
        .map(|row| {
            let body = crate::query::transcript::read(
                &sqlite_options,
                &row.key,
                &serde_json::json!({"id":row.key,"limit":100}),
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
                1790812802000,
            )
            .unwrap()
            .expect("SQLite ordinary body");
            assert!(body.to_string().contains("sanitized"), "ordinary body missing for {} {} {:?}", row.harness, row.kind, row.native_ids);
            let session = crate::query::transcript::session(
                &sqlite_options,
                &row.key,
                &serde_json::json!({"id":row.key}),
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
                1790812802000,
            )
            .unwrap()
            .expect("SQLite ordinary session fields");
            (
                row.key.clone(),
                serde_json::json!({"body":body,"fields":session}),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    oracle.rows = expected.rows;
    oracle.projections = expected.transcripts;
    drop(sqlite);
    let raw = inputs
        .iter()
        .map(|i| fs::read(i.full_path(&options)).unwrap())
        .collect::<Vec<_>>();
    let _guard = crate::storage::postgres::native_ingest_tests::Guard::new();
    let (pool, _) = fixture(|limits| limits.max_connections = 1);
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
    let paths = inputs
        .iter()
        .map(|input| input.full_path(&options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let stage_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    assert!(
        pg.begin_semantic_stage(
            &(0..65).map(|i| format!("oversize-{i}")).collect::<Vec<_>>(),
            &stage_runtime
        )
        .is_err()
    );
    assert_eq!(
        pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(),
        crate::events::Outcome::Written
    );
    let mut stages = Vec::new();
    let (published, semantic_batch, semantic_model) = loop {
        let batch = pg.semantic_batch(64).unwrap().unwrap();
        stages.push(batch.phase);
        // Reopen mid-stage: resume must preserve the exact phase and epoch.
        if batch.phase == crate::model::relation_stage::Phase::Identity {
            drop(pg);
            pg = EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone())
                .unwrap();
            assert_eq!(
                pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(),
                crate::events::Outcome::Written
            );
            let resumed = pg.semantic_batch(64).unwrap().unwrap();
            assert_eq!(resumed.epoch, batch.epoch);
            assert_eq!(resumed.phase, batch.phase);
        }
        let model = pg
            .prepare_semantic_model_batch(&options, &inputs, &batch, 1790812802000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let mut mismatched = batch.clone();
            mismatched
                .context
                .companions
                .push("altered full context".into());
            assert!(
                pg.publish_semantic_catalog(&mismatched, model.prepared())
                    .is_err()
            );
            let ack = pg
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            assert_eq!(
                pg.publish_semantic_catalog(&batch, model.prepared())
                    .unwrap(),
                ack
            );
            break (ack, batch, model);
        }
    };
    assert_eq!(stages.len(), 4);
    assert_eq!(
        producer_rows(&semantic_model.prepared().rows),
        producer_rows(&oracle.rows)
    );
    assert_eq!(
        pg.finish_semantic_stage(semantic_batch.epoch).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(published.0, crate::events::Outcome::Written);
    let generation = published.1.unwrap();
    let component = pg
        .catalog_component_paths(
            &[inputs
                .iter()
                .find(|input| input.full_path(&options) == codex)
                .unwrap()
                .clone()],
            &[],
            &[("codex".into(), codex_id.into())],
            &options,
        )
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|input| input.full_path(&options))
        .collect::<Vec<_>>();
    assert!(component.contains(&codex));
    assert!(component.contains(&child));
    // A lost response retries the identical full request, even though its
    // expected head and acknowledged claims have advanced.
    assert_eq!(
        pg.publish_semantic_catalog(&semantic_batch, semantic_model.prepared())
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
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
    };
    let mut query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(namespace.as_str().into(), options.clone())]),
    ));
    let query_list = query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812802000,
            &query_store,
        )
        .unwrap();
    assert_eq!(
        query_list["total"].as_u64(),
        Some(prepared.rows.len() as u64)
    );
    super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            for row in &prepared.rows {
                let observation = snapshot
                    .transcript_observation(&row.key, &generation, false)
                    .map_err(|_| StorageError::Unavailable)?
                    .unwrap();
                assert!(observation["total"].as_u64().is_some());
                let details = snapshot
                    .selected_details(
                        &row.key,
                        &row.sources,
                        &mut |_| serde_json::json!({"observation":"unavailable"}),
                    )
                    .map_err(|_| StorageError::Unavailable)?;
                assert!(details.is_some());
            }
            Ok(())
        },
    )
    .unwrap();
    struct BodyHost(
        std::collections::BTreeMap<
            (String, String),
            (crate::SessionSourceRef, std::sync::Arc<[u8]>),
        >,
    );
    impl crate::SessionSourceReader for BodyHost {
        fn read_range(
            &self,
            reference: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            let (saved, bytes) = self
                .0
                .get(&(reference.root.clone(), reference.path.clone()))
                .ok_or_else(|| std::io::Error::other("host denied"))?;
            if saved != reference
                || expected.is_some_and(|pin| pin != "body-fixture")
                || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX
            {
                return Err(std::io::Error::other("host denied"));
            }
            let start = usize::try_from(offset).map_err(std::io::Error::other)?;
            let end = bytes.len().min(start.saturating_add(max));
            Ok(crate::SessionSourceRange {
                generation: "body-fixture".into(),
                length: bytes.len() as u64,
                offset,
                bytes: std::sync::Arc::from(
                    bytes
                        .get(start..end)
                        .ok_or_else(|| std::io::Error::other("host range"))?,
                ),
                cached: false,
            })
        }
    }
    let body_host = |rows: &[CatalogRow]| {
        BodyHost(
            rows.iter()
                .flat_map(|row| {
                    row.sources.iter().map(|source| {
                        let path = crate::catalog::source_relative(&options, &row.harness, source)
                            .unwrap();
                        let reference = crate::SessionSourceRef {
                            root: row.harness.clone(),
                            path: path.clone(),
                            native_id: source.native_id.clone(),
                            offset: source.offset,
                            prefix_sha256: source.prefix_sha256,
                            tail_sha256: source.tail_sha256,
                        };
                        (
                            (row.harness.clone(), path),
                            (
                                reference,
                                std::sync::Arc::from(fs::read(&source.path).unwrap()),
                            ),
                        )
                    })
                })
                .collect(),
        )
    };
    let retained_host = body_host(&prepared.rows);
    let read_bodies = |query: &mut crate::Query,
                       store: &super::super::snapshot::query::QueryCatalog,
                       host: &BodyHost| {
        prepared
            .rows
            .iter()
            .map(|row| {
                let body = query
                    .call_bound_selected_at(
                        "read_transcript",
                        &serde_json::json!({"id":row.key,"limit":100}),
                        1790812802000,
                        store,
                        host,
                    )
                    .unwrap();
                let fields = query
                    .call_bound_selected_at(
                        "get_session",
                        &serde_json::json!({"id":row.key}),
                        1790812802000,
                        store,
                        host,
                    )
                    .unwrap();
                (
                    row.key.clone(),
                    serde_json::json!({"body":body,"fields":fields}),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies
    );
    struct DeniedHost;
    impl crate::SessionSourceReader for DeniedHost {
        fn read_range(
            &self,
            _: &crate::SessionSourceRef,
            _: Option<&str>,
            _: u64,
            _: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            Err(std::io::Error::other("host denied"))
        }
    }
    assert!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":prepared.rows[0].key}),
                1790812802000,
                &query_store,
                &DeniedHost
            )
            .is_err(),
        "Catalog metadata must not grant Raw or return blank success"
    );

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
    let restarted_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
    };
    let mut restarted_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(namespace.as_str().into(), options.clone())]),
    ));
    assert_eq!(
        read_bodies(&mut restarted_query, &restarted_store, &retained_host),
        expected_bodies
    );
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
    let append_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    resumed
        .begin_semantic_stage(&paths, &append_runtime)
        .unwrap();
    let continued_generation = loop {
        let batch = resumed.semantic_batch(64).unwrap().unwrap();
        let model = resumed
            .prepare_semantic_model_batch(&options, &inputs, &batch, 1790812803000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let ack = resumed
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            resumed.finish_semantic_stage(batch.epoch).unwrap();
            break ack.1.unwrap();
        }
    };
    let next_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: continued_generation.clone(),
        options: options.clone(),
    };
    let next_host = body_host(&continued.rows);
    let next_bodies = read_bodies(&mut restarted_query, &next_store, &next_host);
    assert!(
        next_bodies
            .values()
            .any(|body| body.to_string().contains("sanitized continuation"))
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "old authorized body must survive successor publication"
    );
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
    let input_path = options.claude_home.join("projects/p/source.meta.json").to_string_lossy().into_owned();
    let input_revision = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        super::super::super::source_journal::invalidate(tx,&input_path)?;
        Ok(tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&namespace.as_str().as_bytes(),&input_path.as_bytes()]).map_err(sqlstate::error)?.get::<_,i64>(0))
    }).unwrap();
    assert_eq!(cache.resolve_catalog_input(&input_path,input_revision+1,None,None).unwrap(),crate::events::Outcome::Conflict);
    assert_eq!(cache.resolve_catalog_input(&input_path,input_revision,Some(("codex",codex_id)),Some(codex.to_str().unwrap())).unwrap(),crate::events::Outcome::Written);
    assert_eq!(cache.resolve_catalog_input(&input_path,input_revision,Some(("codex",codex_id)),Some(codex.to_str().unwrap())).unwrap(),crate::events::Outcome::Written,"exact lost-ACK replay");
    assert_eq!(cache.resolve_catalog_input(&input_path,input_revision,Some(("codex",child_id)),None).unwrap(),crate::events::Outcome::Conflict,"a changed full request cannot replay the ACK");
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

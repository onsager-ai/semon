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
    let claude_parent = options.claude_home.join("projects/project/one.jsonl");
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related-agent-call","parentUuid":"a1","timestamp":"2026-10-01T00:00:01Z","message":{"id":"related-agent-call","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"claude-related-spawn","name":"Agent","input":{"description":"sanitized agent task","prompt":"sanitized agent prompt"}}]}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related-agent-result","parentUuid":"related-agent-call","timestamp":"2026-10-01T00:00:02Z","toolUseResult":{"status":"async_launched","agentId":"related154"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"claude-related-spawn","content":"launched","is_error":false}]}})).unwrap();
    let claude_agent = options
        .claude_home
        .join("projects/project/one/subagents/agent-related154.jsonl");
    fs::create_dir_all(claude_agent.parent().unwrap()).unwrap();
    fs::write(&claude_agent,[
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related-user","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized agent prompt"}}),
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related-answer","parentUuid":"related-user","timestamp":"2026-10-01T00:00:02Z","message":{"id":"related-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized agent answer"}]}}),
    ].iter().map(|line|format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(claude_agent.with_extension("meta.json"),serde_json::json!({"agentType":"general-purpose","description":"sanitized agent task","toolUseId":"claude-related-spawn"}).to_string()).unwrap();
    // Larger indexed history with a fixed small receiver append below.
    let padding=format!("{}\n",serde_json::json!({"type":"system","subtype":"turn_duration","sessionId":"one","padding":"x".repeat(8192)}));
    let mut history=fs::OpenOptions::new().append(true).open(&claude_parent).unwrap();
    for _ in 0..256 {history.write_all(padding.as_bytes()).unwrap();}history.sync_all().unwrap();drop(history);
    let inputs = crate::inputs::inputs(&options)
        .unwrap()
        .into_iter()
        .filter(|input| {
            crate::native_evidence::catalog_input(&input.full_path(&options), &options).is_some()
        })
        .collect::<Vec<_>>();
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
            assert!(
                body.to_string().contains("sanitized"),
                "ordinary body missing for {} {} {:?}",
                row.harness,
                row.kind,
                row.native_ids
            );
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
    let (pool, factory) = fixture(|limits| limits.max_connections = 1);
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
        if batch.phase == crate::model::relation_stage::Phase::Relations {
            use crate::model::relation_stage::Selector;
            let selected = pg
                .semantic_select(
                    batch.epoch,
                    &Selector::ClaudeCall {
                        id: "claude-related-spawn".into(),
                    },
                )
                .unwrap();
            assert_eq!(selected.count, 1);
            assert!(
                selected
                    .endpoint
                    .unwrap()
                    .row
                    .native_ids
                    .iter()
                    .any(|id| id == "one")
            );
            assert!(selected.location.is_some());
            assert_eq!(
                pg.semantic_select(
                    batch.epoch,
                    &Selector::ClaudeReceipt {
                        id: "absent-receipt".into()
                    }
                )
                .unwrap()
                .count,
                0
            );
            assert_eq!(
                pg.semantic_select(
                    batch.epoch,
                    &Selector::ClaudeName {
                        name: "absent-native-name".into()
                    }
                )
                .unwrap()
                .count,
                0
            );
        }
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
    type BodySources = std::collections::BTreeMap<
        (String, String),
        (crate::SessionSourceRef, std::sync::Arc<[u8]>),
    >;
    struct BodyHost(BodySources);
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
    let raw_store=|| crate::storage::postgres::index_store::open(pool.clone(),namespace.clone(),control.clone()).unwrap().0;
    assert!(raw[0].len()>2*1024*1024,"trusted append fixture must have a large old prefix");
    let io_before=crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get);
    let hash_before=crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get);
    let writer=crate::SessionSourceWrite::begin_with_store(&options,&inputs[0],Some("0123456789abcdef0123456789abcdef"),raw[0].len() as u64,raw_store(),None).unwrap();
    let raw_intent=writer.intent();
    assert!(raw_intent.continuation.is_some(),"native durable continuation must qualify trusted append");
    pool.transaction(&namespace,TxnKind::ReadSnapshot,&control,|tx| {
        assert!(crate::storage::postgres::native_keys::read_meta(tx,format!("raw-write-intent:{}",raw_intent.operation).as_bytes(),131072)?.is_some());
        Ok(())
    }).unwrap();
    drop(writer); // Fault: durable SQL intent, no Raw bytes yet.
    assert_eq!(fs::read(&path).unwrap(),raw[0]);
    let mut mismatch=raw_intent.clone();mismatch.from+=1;
    assert!(crate::SessionSourceWrite::begin_with_store(&options,&inputs[0],None,0,raw_store(),Some(mismatch)).is_err());
    let writer=crate::SessionSourceWrite::begin_with_store(&options,&inputs[0],None,0,raw_store(),Some(raw_intent.clone())).unwrap();
    let mut raw_file=fs::OpenOptions::new().append(true).open(&path).unwrap();
    raw_file.write_all(append.as_bytes()).unwrap();raw_file.sync_all().unwrap();drop(raw_file);
    drop(writer); // Fault: durable bytes, completion transaction not attempted.
    let written=raw[0].len() as u64+append.len() as u64;
    assert_eq!(resumed.ingest(&format!("{:064x}",158),&path,"claude").err(),Some(StorageError::Conflict),"unacknowledged Raw bytes cannot enter native SQL");
    let writer=crate::SessionSourceWrite::begin_with_store(&options,&inputs[0],None,0,raw_store(),Some(raw_intent.clone())).unwrap();
    writer.complete(written).unwrap();
    // Fault: SQL committed but the receiver lost the ACK. The exact request
    // reconciles before head/ledger CAS and reproduces the same completion.
    let writer=crate::SessionSourceWrite::begin_with_store(&options,&inputs[0],None,0,raw_store(),Some(raw_intent.clone())).unwrap();
    writer.complete(written).unwrap();
    let read_bytes=crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get)-io_before;
    let hash_bytes=crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get)-hash_before;
    assert!(read_bytes<512*1024,"trusted SourceWrite read {read_bytes} bytes");
    assert!(hash_bytes<512*1024,"trusted SourceWrite hashed {hash_bytes} bytes");
    let c=raw_intent.continuation.as_ref().unwrap();
    let mut source=crate::sealed::LogFile::open(&path).unwrap();
    assert_eq!(crate::source_chunks::continue_prefix(&mut source,&c.state,c.offset,c.prefix,written,||Err(std::io::Error::new(std::io::ErrorKind::Interrupted,"cancel"))).unwrap_err().kind(),std::io::ErrorKind::Interrupted);
    eprintln!("sourcewrite160 old_bytes={} delta={} read_bytes={read_bytes} hash_bytes={hash_bytes} history_io_under_sql_lock=0",raw[0].len(),append.len());
    let mut stale=raw_intent;stale.operation.push_str("-stale");stale.head=Some("stale-head".into());
    assert!(crate::SessionSourceWrite::begin_with_store(&options,&inputs[0],None,0,raw_store(),Some(stale)).is_err());
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
    let input_path = options
        .claude_home
        .join("projects/p/source.meta.json")
        .to_string_lossy()
        .into_owned();
    let mut facts_options = options.clone();
    facts_options.facts = Some(oracle.root.join("facts-dispatch-only"));
    cache
        .resume_runtime_input(&facts_options, &input_path)
        .unwrap();
    assert!(!options.cache.exists());
    let input_revision = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        super::super::super::source_journal::invalidate(tx,&input_path)?;
        Ok(tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&namespace.as_str().as_bytes(),&input_path.as_bytes()]).map_err(sqlstate::error)?.get::<_,i64>(0))
    }).unwrap();
    assert_eq!(
        cache
            .resolve_catalog_input(&input_path, input_revision + 1, None, None)
            .unwrap(),
        crate::events::Outcome::Conflict
    );
    assert_eq!(
        cache
            .resolve_catalog_input(
                &input_path,
                input_revision,
                Some(("codex", codex_id)),
                Some(codex.to_str().unwrap())
            )
            .unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        cache
            .resolve_catalog_input(
                &input_path,
                input_revision,
                Some(("codex", codex_id)),
                Some(codex.to_str().unwrap())
            )
            .unwrap(),
        crate::events::Outcome::Written,
        "exact lost-ACK replay"
    );
    assert_eq!(
        cache
            .resolve_catalog_input(&input_path, input_revision, Some(("codex", child_id)), None)
            .unwrap(),
        crate::events::Outcome::Conflict,
        "a changed full request cannot replay the ACK"
    );
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
    // Disappearance withdraws Current membership, without consuming the host's
    // independently retained bytes or any older Catalog/native proof.
    let missing = copilot.to_str().unwrap().to_owned();
    let missing_ledger = cache.ledger(&missing).unwrap().unwrap();
    let retained_copy = oracle.root.join("retained-copilot156.jsonl");
    fs::rename(&copilot, &retained_copy).unwrap();
    let removal = vec![(missing.clone(), Some(missing_ledger.clone()))];
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written
    );
    let withdrawn_generation = cache.session_catalog_generation().unwrap().unwrap();
    assert_ne!(withdrawn_generation, continued_generation);
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written,
        "lost removal ACK"
    );
    assert_eq!(
        cache.session_catalog_generation().unwrap().as_deref(),
        Some(withdrawn_generation.as_str())
    );
    let mut stale = missing_ledger;
    stale.offset += 1;
    assert_eq!(
        cache
            .remove_files(&[(missing.clone(), Some(stale))])
            .unwrap(),
        crate::events::Outcome::Conflict
    );
    let withdrawn_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: withdrawn_generation.clone(),
        options: options.clone(),
    };
    let current = query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812802000,
            &withdrawn_store,
        )
        .unwrap();
    assert_eq!(
        current["total"].as_u64(),
        Some((continued.rows.len() - 1) as u64)
    );
    let removed_key = continued
        .rows
        .iter()
        .find(|r| r.harness == "copilot")
        .unwrap()
        .key
        .clone();
    assert!(!current.to_string().contains(&removed_key));
    assert!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":removed_key,"limit":100}),
                1790812802000,
                &withdrawn_store,
                &retained_host
            )
            .is_err()
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies
    );
    drop(cache);
    let (mut cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "withdrawal preserves authorized old body after reopen"
    );
    assert_eq!(
        fs::read(&retained_copy).unwrap(),
        retained_host
            .0
            .values()
            .find(|(r, _)| r.root == "copilot")
            .unwrap()
            .1
            .as_ref()
    );
    let archived = options
        .codex_home
        .join(format!("archived_sessions/rollout-{codex_id}.jsonl"));
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    let codex_bytes = fs::read(&codex).unwrap();
    fs::rename(&codex, &archived).unwrap();
    let moved_stat = crate::events::Stat::of(&archived).unwrap();
    let mut moved_file = crate::sealed::LogFile::open(&archived).unwrap();
    let mut timing = crate::native_timing::Measurements::new(false);
    assert_eq!(
        cache
            .relocate_codex(
                archived.to_str().unwrap(),
                &moved_stat,
                codex_id,
                &mut moved_file,
                &mut timing
            )
            .unwrap()
            .as_deref(),
        codex.to_str()
    );
    assert_eq!(
        cache
            .relocate_codex(
                archived.to_str().unwrap(),
                &moved_stat,
                codex_id,
                &mut moved_file,
                &mut timing
            )
            .unwrap()
            .as_deref(),
        codex.to_str(),
        "exact relocation ACK replay"
    );
    assert_eq!(fs::read(&archived).unwrap(), codex_bytes);
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "old authorized body survives native relocation"
    );
    drop(cache);
    let moved_inputs = crate::inputs::inputs(&options)
        .unwrap()
        .into_iter()
        .filter(|i| {
            crate::native_evidence::catalog_input(&i.full_path(&options), &options).is_some()
        })
        .collect::<Vec<_>>();
    let moved_paths = moved_inputs
        .iter()
        .map(|i| i.full_path(&options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut moved_cache =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let moved_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    moved_cache
        .begin_semantic_stage(&moved_paths, &moved_runtime)
        .unwrap();
    let (moved_generation, moved_rows) = loop {
        let batch = moved_cache.semantic_batch(64).unwrap().unwrap();
        let model = moved_cache
            .prepare_semantic_model_batch(&options, &moved_inputs, &batch, 1790812803000)
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let rows = model.prepared().rows.clone();
            let ack = moved_cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            moved_cache.finish_semantic_stage(batch.epoch).unwrap();
            break (ack.1.unwrap(), rows);
        }
    };
    assert_eq!(
        moved_rows
            .iter()
            .filter(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
            .count(),
        1
    );
    let moved_owner = moved_rows
        .iter()
        .find(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
        .unwrap();
    assert_eq!(
        moved_owner.key,
        continued
            .rows
            .iter()
            .find(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
            .unwrap()
            .key
    );
    let moved_store = super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: moved_generation,
        options: options.clone(),
    };
    let moved_host = body_host(&moved_rows);
    assert_eq!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":moved_owner.key,"limit":100}),
                1790812802000,
                &moved_store,
                &moved_host
            )
            .unwrap(),
        expected_bodies[&moved_owner.key]["body"]
    );
    drop(moved_cache);
    let mut moved_cache =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&archived).unwrap(),"{}",serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:03Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"sanitized relocation continuation"}]}})).unwrap();
    moved_cache
        .ingest(&format!("{:064x}", 156), &archived, "codex")
        .unwrap();
    let appended =
        crate::model::prepare_sources(&options, &mut moved_cache, &moved_inputs, 1790812804000)
            .unwrap();
    assert!(appended.rows.iter().any(|r| r.key == moved_owner.key));
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies
    );
    let (_cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
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
        crate::storage::postgres::index_store::open(pool.clone(), collision, cancelled),
        Err(StorageError::Busy)
    ));
    let facts_path=oracle.root.join("positive-facts158.json");
    let mut recorded=crate::Facts {version:2,hostname:"source-write-before158".into(),..Default::default()};
    crate::save_json_file(&facts_path,&recorded).unwrap();
    let mut facts_options=options.clone();facts_options.facts=Some(facts_path.clone());
    let before_runtime=crate::native_evidence::query_runtime_generation(&facts_options).unwrap();
    let from=fs::metadata(&path).unwrap().len();
    let writer=crate::SessionSourceWrite::begin_with_store(&facts_options,&inputs[0],None,from,raw_store(),None).unwrap();
    let facts_intent=writer.intent();
    let extra=format!("{}\n",serde_json::json!({"type":"user","sessionId":"one","uuid":"positive-facts158","parentUuid":"u2","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:04Z","message":{"role":"user","content":"positive Facts source continuation"}}));
    let mut file=fs::OpenOptions::new().append(true).open(&path).unwrap();file.write_all(extra.as_bytes()).unwrap();file.sync_all().unwrap();drop(file);
    writer.complete(from+extra.len() as u64).unwrap();
    let replay=crate::SessionSourceWrite::begin_with_store(&facts_options,&inputs[0],None,0,raw_store(),Some(facts_intent)).unwrap();replay.complete(from+extra.len() as u64).unwrap();
    let pid_input=crate::Input {root:crate::InputRoot::Claude,path:"sessions/4242.json".into()};
    fs::create_dir_all(options.claude_home.join("sessions")).unwrap();
    let pid_write=crate::SessionSourceWrite::begin_with_store(&facts_options,&pid_input,None,0,raw_store(),None).unwrap();
    let pid_bytes=serde_json::to_vec(&serde_json::json!({"sessionId":"one","procStart":1234,"startedAt":1790812800000i64,"status":"waiting","waitingFor":"input needed"})).unwrap();
    fs::write(pid_input.full_path(&facts_options),&pid_bytes).unwrap();fs::OpenOptions::new().write(true).open(pid_input.full_path(&facts_options)).unwrap().sync_all().unwrap();
    pid_write.complete(pid_bytes.len() as u64).unwrap();
    recorded.hostname="source-write-after158".into();recorded.proc_starts.insert(4242,1234);
    let facts_writer=crate::SessionFactsWrite::begin_with_store(&facts_options,&recorded,true,raw_store(),None).unwrap();
    let facts_request=facts_writer.intent();drop(facts_writer); // Intent/no Facts bytes.
    let facts_writer=crate::SessionFactsWrite::begin_with_store(&facts_options,&recorded,true,raw_store(),Some(facts_request.clone())).unwrap();
    crate::save_json_file(&facts_path,&recorded).unwrap();drop(facts_writer); // Bytes/no SQL completion.
    let facts_writer=crate::SessionFactsWrite::begin_with_store(&facts_options,&recorded,true,raw_store(),Some(facts_request.clone())).unwrap();facts_writer.complete().unwrap();
    let facts_writer=crate::SessionFactsWrite::begin_with_store(&facts_options,&recorded,true,raw_store(),Some(facts_request.clone())).unwrap();facts_writer.complete().unwrap(); // Lost ACK.
    let mut stale_facts=facts_request.clone();stale_facts.operation.push_str("-stale");stale_facts.head=Some("stale".into());
    assert!(crate::SessionFactsWrite::begin_with_store(&facts_options,&recorded,true,raw_store(),Some(stale_facts)).is_err());
    let mut wrong=facts_request;wrong.before.hostname.push_str("-wrong");
    assert!(crate::SessionFactsWrite::begin_with_store(&facts_options,&recorded,true,raw_store(),Some(wrong)).is_err());
    assert!(crate::model::pid_files(&facts_options,&crate::facts::MachineFacts::Recorded(Box::new(recorded.clone()))).iter().any(|p|p.pid==4242&&p.session=="one"&&p.alive));
    let runtime=crate::native_evidence::query_runtime_generation(&facts_options).unwrap();assert_ne!(before_runtime,runtime);
    moved_cache.ingest(&format!("{:064x}",159),&path,"claude").unwrap();
    moved_cache.begin_semantic_stage(&moved_inputs.iter().map(|i|i.full_path(&facts_options).to_string_lossy().into_owned()).collect::<Vec<_>>(),&runtime).unwrap();
    let facts_generation=loop {
        let batch=moved_cache.semantic_batch(64).unwrap().unwrap();
        let model=moved_cache.prepare_semantic_model_batch(&facts_options,&moved_inputs,&batch,1790812805000).unwrap();
        if batch.phase==crate::model::relation_stage::Phase::Publication {
            assert!(serde_json::to_string(&model.prepared().rows).unwrap().contains("source-write-after158"));
            let ack=moved_cache.publish_semantic_catalog(&batch,model.prepared()).unwrap();moved_cache.finish_semantic_stage(batch.epoch).unwrap();break ack.1.unwrap();
        }
    };
    let facts_store=super::super::snapshot::query::QueryCatalog {pool:pool.clone(),namespace:namespace.clone(),control:control.clone(),generation:facts_generation,options:facts_options.clone()};
    let mut facts_query=crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(vec![(namespace.as_str().into(),facts_options.clone())])));
    let list=facts_query.call_bound_catalog_at("list_sessions",&serde_json::json!({}),1790812805000,&facts_store).unwrap();
    assert!(list.to_string().contains("source-write-after158"));
    assert_eq!(fs::read(&path).unwrap(),[raw[0].as_slice(),append.as_bytes(),extra.as_bytes()].concat());
    assert!(!options.cache.exists());
    // (a) Ordinary incremental stages grow Current beyond 64 owners. Each
    // stage names only its changed paths; unmentioned owners carry forward.
    let stage_publish = |cache: &mut EventCache, inputs: &[crate::Input], at: i64| {
        let paths = inputs
            .iter()
            .map(|i| i.full_path(&facts_options).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        cache.begin_semantic_stage(&paths, &runtime).unwrap();
        loop {
            let batch = cache.semantic_batch(64).unwrap().unwrap();
            let model = cache
                .prepare_semantic_model_batch(&facts_options, inputs, &batch, at)
                .unwrap();
            if batch.phase == crate::model::relation_stage::Phase::Publication {
                let ack = cache
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap();
                cache.finish_semantic_stage(batch.epoch).unwrap();
                break (ack.1.unwrap(), batch, model);
            }
        }
    };
    let bulk = (0..70)
        .map(|i| {
            let input = crate::Input {
                root: crate::InputRoot::Claude,
                path: format!("projects/project/bulk169-{i:03}.jsonl").into(),
            };
            let body = serde_json::json!({"type":"user","sessionId":format!("bulk169-{i:03}"),"uuid":format!("bulk-{i}"),"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized bulk input"}});
            fs::write(input.full_path(&facts_options), format!("{body}\n")).unwrap();
            input
        })
        .collect::<Vec<_>>();
    for (at, input) in bulk.iter().enumerate() {
        moved_cache
            .ingest(
                &format!("{:064x}", 1000 + at),
                &input.full_path(&facts_options),
                "claude",
            )
            .unwrap();
    }
    let mut bulk_generation = String::new();
    for chunk in bulk.chunks(30) {
        bulk_generation = stage_publish(&mut moved_cache, chunk, 1790812806000).0;
    }
    let store_at = |generation: &str| super::super::snapshot::query::QueryCatalog {
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.to_owned(),
        options: facts_options.clone(),
    };
    let total = list["total"].as_u64().unwrap() + 70;
    assert!(total > 64);
    let bulk_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812806000,
            &store_at(&bulk_generation),
        )
        .unwrap();
    assert_eq!(bulk_list["total"].as_u64(), Some(total));
    assert!(bulk_list.to_string().contains("source-write-after158"));
    let bulk_page = super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &bulk_generation,
        |snapshot| {
            crate::catalog::read_page_snapshot(
                &facts_options,
                "",
                &crate::catalog::Request::parse("limit=20").unwrap(),
                snapshot,
            )
            .map_err(|_| StorageError::ScopeChanged)
        },
    )
    .unwrap();
    assert_eq!(bulk_page.status, 200);
    // (b) A small partial update republishes one owner; every unrelated
    // Current row, Source and proof stays byte-identical, as do Facts bytes.
    let facts_bytes = fs::read(&facts_path).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(bulk[5].full_path(&facts_options)).unwrap(),"{}",serde_json::json!({"type":"user","sessionId":"bulk169-005","uuid":"bulk-5b","parentUuid":"bulk-5","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized bulk continuation"}})).unwrap();
    moved_cache
        .ingest(
            &format!("{:064x}", 1100),
            &bulk[5].full_path(&facts_options),
            "claude",
        )
        .unwrap();
    let (partial_generation, partial_batch, partial_model) =
        stage_publish(&mut moved_cache, &bulk[5..6], 1790812807000);
    assert_ne!(partial_generation, bulk_generation);
    // Unrelated means every owner and proof not reading `path`.
    let unrelated = |a: &str, b: &str, path: &str| {
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            let mut differences = Vec::new();
            let owner = "session_key NOT IN (SELECT session_key FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation IN ($2,$3) AND source_path=$4)";
            for (table, columns, owner) in [
                ("catalog_rows", "session_key,metadata", owner),
                (
                    "catalog_sources",
                    "session_key,source_path,metadata,native_digest",
                    owner,
                ),
                (
                    "catalog_proofs",
                    "source_path,page_generation,usage_generation,usage_ready",
                    "source_path<>$4",
                ),
            ] {
                let count: i64 = tx.driver.query_one(&format!("SELECT count(*) FROM (((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND {owner}) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND {owner})) UNION ALL ((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND {owner}) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND {owner}))) d"), &[&ns, &a.as_bytes(), &b.as_bytes(), &path.as_bytes()]).map_err(|_| StorageError::Unavailable)?.get(0);
                differences.push(count);
            }
            Ok(differences)
        })
        .unwrap()
    };
    let changed = bulk[5].full_path(&facts_options);
    assert_eq!(
        unrelated(
            &bulk_generation,
            &partial_generation,
            changed.to_str().unwrap()
        ),
        vec![0, 0, 0]
    );
    assert_eq!(fs::read(&facts_path).unwrap(), facts_bytes);
    let partial_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812807000,
            &store_at(&partial_generation),
        )
        .unwrap();
    assert_eq!(partial_list["total"].as_u64(), Some(total));
    assert!(partial_list.to_string().contains("source-write-after158"));
    // (c) An isolated Source withdraws by bounded pages. A Source of a mixed
    // or connected owner refuses and leaves Current untouched: removal and
    // the owner's semantic republication are separate commits today.
    let (mut store, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let agent = claude_agent.to_str().unwrap().to_owned();
    let agent_ledger = store.ledger(&agent).unwrap().unwrap();
    assert!(store.remove_files(&[(agent, Some(agent_ledger))]).is_err());
    assert_eq!(
        store.session_catalog_generation().unwrap().as_deref(),
        Some(partial_generation.as_str())
    );
    let gone = bulk[1].full_path(&facts_options);
    let gone_path = gone.to_str().unwrap().to_owned();
    let gone_removal = vec![(gone_path.clone(), store.ledger(&gone_path).unwrap())];
    fs::rename(&gone, oracle.root.join("retained-bulk169.jsonl")).unwrap();
    assert_eq!(
        store.remove_files(&gone_removal).unwrap(),
        crate::events::Outcome::Written
    );
    let withdrawn = store.session_catalog_generation().unwrap().unwrap();
    assert_ne!(withdrawn, partial_generation);
    assert_eq!(
        unrelated(&partial_generation, &withdrawn, &gone_path),
        vec![0, 0, 0],
        "withdrawal of an isolated owner keeps unrelated Current"
    );
    // A successor publication, then exact old requests replay their own
    // recorded generations without republishing or moving the head.
    writeln!(fs::OpenOptions::new().append(true).open(bulk[2].full_path(&facts_options)).unwrap(),"{}",serde_json::json!({"type":"user","sessionId":"bulk169-002","uuid":"bulk-2b","parentUuid":"bulk-2","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":"sanitized successor"}})).unwrap();
    moved_cache
        .ingest(
            &format!("{:064x}", 1101),
            &bulk[2].full_path(&facts_options),
            "claude",
        )
        .unwrap();
    let successor = stage_publish(&mut moved_cache, &bulk[2..3], 1790812808000).0;
    assert_ne!(successor, withdrawn);
    assert_eq!(
        moved_cache
            .publish_semantic_catalog(&partial_batch, partial_model.prepared())
            .unwrap()
            .1
            .as_deref(),
        Some(partial_generation.as_str())
    );
    assert_eq!(
        store.remove_files(&gone_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        store.session_catalog_generation().unwrap().as_deref(),
        Some(successor.as_str())
    );
    let successor_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812808000,
            &store_at(&successor),
        )
        .unwrap();
    assert_eq!(successor_list["total"].as_u64(), Some(total - 1));
    // (d) Restart: reopened stores read the newest Current and the old
    // selected bodies exactly as before.
    drop(store);
    drop(moved_cache);
    let reopened =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        reopened.session_catalog_generation().as_deref(),
        Some(successor.as_str())
    );
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies,
        "old selected bodies after incremental publication and restart"
    );
    // Fresh Pool: new connections run the cold binding audit, then read the
    // committed head, every older committed generation's cohort (retained
    // history merged as in SQLite) and the old selected bodies.
    drop(reopened);
    let started = std::time::Instant::now();
    let cold = crate::storage::postgres::PgPool::new(
        factory.clone(),
        crate::storage::postgres::tests::limits(),
    )
    .unwrap();
    let cold_cache =
        EventCache::open_pg_native(cold.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        cold_cache.session_catalog_generation().as_deref(),
        Some(successor.as_str())
    );
    let cold_store = |generation: &str, options: &Options| {
        super::super::snapshot::query::QueryCatalog {
            pool: cold.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: generation.to_owned(),
            options: options.clone(),
        }
    };
    let first = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812809000,
            &cold_store(&successor, &facts_options),
        )
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(8), "cold first page");
    assert_eq!(first["total"].as_u64(), Some(total - 1));
    for (at, expected) in [
        (&bulk_generation, total),
        (&partial_generation, total),
        (&withdrawn, total - 1),
    ] {
        let listed = facts_query
            .call_bound_catalog_at(
                "list_sessions",
                &serde_json::json!({}),
                1790812809000,
                &cold_store(at, &facts_options),
            )
            .unwrap();
        assert_eq!(listed["total"].as_u64(), Some(expected), "older cohort");
    }
    assert_eq!(
        read_bodies(
            &mut query,
            &cold_store(&generation, &options),
            &retained_host
        ),
        expected_bodies,
        "old selected bodies through a fresh Pool"
    );
}

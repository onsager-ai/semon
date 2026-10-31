//! Synthetic recovery diagnostics, not production latency or memory budgets.
//! Run alone with --test-threads=1 --nocapture for interpretable process counters.
//! Complete selected Index decoding may precede provider callbacks; these measurements
//! do not establish history-independent work or per-request peak memory. Separate
//! SelectedReadControl regressions qualify request-scoped limits and cancellation.

use super::*;
use std::{collections::BTreeMap, sync::mpsc, time::Instant};

fn recovery_catalog_row(
    db: &rusqlite::Connection,
    table: &str,
    key: &str,
) -> crate::model::summary::CatalogRow {
    let metadata: String = db
        .query_row(
            &format!("SELECT metadata FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1"),
            [key],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&metadata).unwrap()
}

fn recovery_catalog_metadata(db: &rusqlite::Connection, table: &str, key: &str) -> String {
    db.query_row(
        &format!(
            "SELECT metadata FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1"
        ),
        [key],
        |row| row.get(0),
    )
    .unwrap()
}

fn recovery_observation_grant(
    db: &rusqlite::Connection,
    key: &str,
    owner: &str,
) -> Option<(String, i64, String)> {
    use rusqlite::OptionalExtension;
    db.query_row(
        "SELECT metadata_digest,admission,stream FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key=?1 AND owner_kind=?2",
        rusqlite::params![key, owner],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .unwrap()
}

fn recovery_proof_snapshot(
    db: &rusqlite::Connection,
    key: &str,
) -> Vec<(String, String, String, String, i64, String)> {
    let mut statement = db
        .prepare(
            "SELECT r.kind,r.id,g.publication,g.proof,g.admission,g.stream FROM storage_proof_refs r JOIN storage_proof_generations g ON g.namespace=r.namespace AND g.id=r.id AND g.session_key=r.session_key WHERE r.namespace=semon_namespace() AND r.session_key=?1 ORDER BY r.kind,r.id",
        )
        .unwrap();
    statement
        .query_map([key], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn recovery_stage_observation_grants(db: &rusqlite::Connection) -> Vec<(String, String)> {
    let mut statement = db
        .prepare(
            "SELECT key,value FROM main.meta WHERE namespace=semon_namespace() AND key LIKE 'catalog_native_observation:%' AND value IS NOT NULL ORDER BY key",
        )
        .unwrap();
    let grants = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    // A producer Stage proof binds the exact preimage. Keep the complete scoped
    // set so publication cannot silently rewrite any retained proof grant.
    grants
}

fn recovery_stage_provenance(db: &rusqlite::Connection, key: &str) -> Vec<String> {
    let mut statement = db
        .prepare(
            "SELECT json_array(g.id,g.publication,g.proof,g.admission,g.stream,g.metadata,g.slots,g.prefix,r.base,r.manifest,r.digest,v.metadata,p.epoch,p.base_generation,p.source_revision,p.claims,p.metadata) FROM storage_proof_generations g JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key WHERE g.namespace=semon_namespace() AND g.session_key=?1 ORDER BY g.id,p.epoch",
        )
        .unwrap();
    statement
        .query_map([key], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn recovery_publication_snapshot(db: &rusqlite::Connection, key: &str) -> Vec<(String, String)> {
    let mut statement = db
        .prepare(
            "SELECT 'overview',summary FROM main.session_overview WHERE session_key=?1 UNION ALL SELECT 'header',metadata FROM main.session_trace_headers WHERE session_key=?1 UNION ALL SELECT 'turn',metadata FROM main.session_trace_turns WHERE session_key=?1 UNION ALL SELECT 'edge',metadata FROM main.session_trace_edges WHERE session_key=?1 ORDER BY 1,2",
        )
        .unwrap();
    statement
        .query_map([key], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn publish(home: &Home) {
    crate::model::observe_catalog(
        &home.options,
        &mut crate::events::EventCache::open_scoped(&home.options.cache, &home.options.source)
            .unwrap(),
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
}

fn recovery_selected_transcript(home: &Home, key: &str) -> crate::ViewerReply {
    crate::session_transcript_range_with_mode(
        &home.options,
        "local",
        &format!("sid={key}&after=0&limit=1"),
        None,
        crate::SessionSourceReadMode::LocalThenProvider,
    )
}

fn publish_semantic(home: &Home) {
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let mut work = crate::viewer::lock(&evidence.work);
    let mut complete = false;
    for _ in 0..128 {
        if evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    assert!(complete, "the semantic producer fixture must converge");
    drop(work);
    evidence.retire();
}

fn publish_semantic_stage(home: &Home, input: &crate::Input) {
    use crate::model::relation_stage::Phase;
    let config = home.options.source.config(&home.options.cache).unwrap();
    let mut cache = crate::events::EventCache::open_semantic(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap();
    let paths = vec![
        input
            .full_path(&home.options)
            .to_string_lossy()
            .into_owned(),
    ];
    let runtime = crate::native_evidence::query_runtime_generation(&home.options).unwrap();
    crate::storage::transfer::reported::tests::begin_active_catalog(
        &mut cache, &paths, &runtime, false,
    );
    let mut published = false;
    for _ in 0..128 {
        let batch = cache.semantic_batch(64).unwrap().unwrap();
        if batch.paths.is_empty() {
            if batch.phase == Phase::Publication {
                assert!(
                    published,
                    "the real model must publish before Stage cleanup"
                );
                assert_eq!(
                    cache.finish_semantic_stage(batch.epoch).unwrap(),
                    crate::events::Outcome::Written
                );
                return;
            }
            continue;
        }
        let model = cache
            .prepare_semantic_model_batch(&home.options, std::slice::from_ref(input), &batch, NOW)
            .unwrap();
        if batch.phase == Phase::Publication {
            assert_eq!(
                cache
                    .publish_semantic_catalog(&batch, model.prepared())
                    .unwrap()
                    .0,
                crate::events::Outcome::Written
            );
            published = true;
        }
        cache.end_scan();
    }
    panic!("the real semantic Stage fixture must converge");
}

fn shared_home(prefix: usize, leading_user: bool) -> (Home, String) {
    let home = Home::new("recovery-shared");
    for sid in ["first", "second"] {
        let mut records = Vec::new();
        if leading_user {
            records.push(human(sid, ts(1, 0), "shared owner prompt"));
        }
        records.extend((0..prefix).map(|n| {
            assistant(
                sid,
                ts(1, 1),
                vec![text(&format!("{n} {}", "history ".repeat(64)))],
            )
        }));
        records.push(assistant(
            sid,
            ts(23, 58),
            vec![
                tool(
                    &format!("sent-{sid}"),
                    "SendMessage",
                    json!({"to":"remote","message":format!("sent from {sid}")}),
                ),
                tool(
                    &format!("lost-{sid}"),
                    "SendMessage",
                    json!({"to":"missing","message":format!("lost from {sid}")}),
                ),
            ],
        ));
        for (kind, success) in [("sent", true), ("lost", false)] {
            records.push(json!({"type":"user","sessionId":sid,"timestamp":ts(23,59),
                "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("{kind}-{sid}"),"content":kind}]},
                "toolUseResult":{"success":success,"message":kind}}));
        }
        home.top(sid, &records);
    }
    home.top(
        "independent",
        &[human("independent", ts(23, 59), "independent prompt")],
    );
    publish(&home);
    let listing = Query::from_sources(admitted(home.options.clone()))
        .call_at("list_sessions", &json!({}), NOW)
        .unwrap();
    let key = listing["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "stub")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (home, key)
}

struct Archives(BTreeMap<String, SelectedArchive>);

impl Archives {
    fn take(home: &Home, key: &str) -> Self {
        let refs = crate::session_query_sources(&home.options, key).unwrap();
        assert_eq!(refs.len(), 2, "the shared fallback must reach both owners");
        Self(
            refs.into_iter()
                .map(|source| {
                    let path = home.options.claude_home.join(&source.path);
                    let bytes = Arc::from(fs::read(&path).unwrap());
                    fs::remove_file(path).unwrap();
                    (
                        source.path.clone(),
                        SelectedArchive {
                            source,
                            bytes,
                            calls: std::sync::atomic::AtomicUsize::new(0),
                            read_bytes: AtomicU64::new(0),
                            fault: 0,
                        },
                    )
                })
                .collect(),
        )
    }

    fn work(&self) -> (usize, u64) {
        self.0.values().fold((0, 0), |(calls, bytes), archive| {
            (
                calls + archive.calls.load(Ordering::Relaxed),
                bytes + archive.read_bytes.load(Ordering::Relaxed),
            )
        })
    }
}

impl crate::SessionSourceReader for Archives {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        self.0
            .get(&source.path)
            .expect("unrelated archive requested")
            .read_range(source, expected, offset, max)
    }
}

fn process_memory() -> Value {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kib = |name: &str| {
        status.lines().find_map(|line| {
            line.strip_prefix(name)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
    };
    // Same allocator diagnostics as examples/query-workload.rs. Endpoints are
    // not heap peaks; VmHWM is the whole process's lifetime high-water mark.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let heap = {
        // SAFETY: mallinfo2 takes no pointers and reads allocator counters.
        let heap = unsafe { libc::mallinfo2() };
        json!({"allocated":heap.uordblks.saturating_add(heap.hblkhd),
            "reserved":heap.arena.saturating_add(heap.hblkhd)})
    };
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    let heap = Value::Null;
    json!({"rss_kib":kib("VmRSS:"),"process_lifetime_hwm_kib":kib("VmHWM:"),"heap_endpoint_bytes":heap})
}

fn process_io() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/self/io").ok()?;
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.trim().parse().ok())
    };
    Some((field("rchar:")?, field("syscr:")?))
}

#[test]
fn recovery_shared_owner_fallback_scope_measurement() {
    for prefix in [0, 128, 1024] {
        let (home, stub) = shared_home(prefix, true);
        let mut oracle = Query::from_sources(admitted(home.options.clone()));
        let mut cases = Vec::new();
        for key in [&stub, "unsent:missing"] {
            // Native unsent owners have transcript slots but no Session row.
            // Preserve that exact get_session error as well as supported pages.
            let detail = oracle.call_at("get_session", &json!({"id":key}), NOW);
            if key == "unsent:missing" {
                assert_eq!(detail.as_ref().unwrap_err().code, "unknown_session");
            } else {
                assert!(detail.is_ok());
            }
            cases.push((key.to_owned(), "get_session", json!({"id":key}), detail));
            // Walk every native page, including the terminal empty page. Exact
            // equality pins anchors, total, order and every supported entry.
            let mut after = 0;
            loop {
                let args = json!({"id":key,"after":after,"limit":1});
                let expected = oracle.call_at("read_transcript", &args, NOW).unwrap();
                let done = expected["entries"].as_array().unwrap().is_empty();
                let next = expected["to"].as_u64().unwrap();
                cases.push((key.to_owned(), "read_transcript", args, Ok(expected)));
                if done {
                    break;
                }
                assert!(next > after, "native paging must advance");
                after = next;
            }
        }
        let archives = Archives::take(&home, &stub);
        let source_bytes: usize = archives.0.values().map(|archive| archive.bytes.len()).sum();
        for (key, tool, args, expected) in cases {
            let mut query = Query::from_sources(admitted(home.options.clone()));
            let before_memory = process_memory();
            let before_io = process_io();
            let before_work = archives.work();
            let indexes = crate::events::FILE_READS.with(std::cell::Cell::get);
            let started = Instant::now();
            let actual = query.call_selected_at(
                "local",
                &key,
                tool,
                &args,
                Some(&archives),
                crate::SessionSourceReadMode::ProviderOnly,
                NOW,
            );
            let elapsed_us = started.elapsed().as_micros();
            let io = before_io
                .zip(process_io())
                .map(|(before, after)| json!({"rchar":after.0-before.0,"syscr":after.1-before.1}));
            let work = archives.work();
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - indexes;
            assert_eq!(actual, expected, "prefix={prefix} {tool} {args}");
            assert!(
                loads >= 2,
                "measurement must actually reach the complete shared Index fallback"
            );
            assert!(
                query.native.is_some(),
                "fallback must derive selected native evidence"
            );
            for archive in archives.0.values() {
                assert!(!home.options.claude_home.join(&archive.source.path).exists());
            }
            eprintln!(
                "recovery_selected_fallback {}",
                json!({"prefix_records_per_owner":prefix,
                "selected_source_bytes":source_bytes,"selected_sources":2,"tool":tool,"args":args,
                "elapsed_us":elapsed_us,"full_index_loads":loads,"provider_calls":work.0-before_work.0,
                "provider_bytes":work.1-before_work.1,"process_io_delta":io,
                "memory_before":before_memory,"memory_after":process_memory(),
                "memory_scope":"process endpoints and lifetime HWM, includes fixture/oracle; not per-request peaks"})
            );
        }
    }
}

struct HeldProvider {
    entered: mpsc::SyncSender<()>,
    cancel: std::sync::Mutex<mpsc::Receiver<()>>,
    started: std::sync::atomic::AtomicBool,
}

/// A larger selected closure diagnoses availability at the production cap and
/// at a private test allowance. It changes no production limit or release gate.
#[test]
fn recovery_large_shared_owner_preparation_budget_measurement() {
    let (home, key) = shared_home(8192, true);
    let mut oracle = Query::from_sources(admitted(home.options.clone()));
    let complete_args = json!({"id":key,"after":0,"limit":10});
    let complete = oracle
        .call_at("read_transcript", &complete_args, NOW)
        .unwrap();
    let total = complete["total"].as_u64().unwrap();
    assert!(
        total > 0 && total <= 10,
        "the shared virtual transcript must remain a small complete result"
    );
    assert_eq!(complete["entries"].as_array().unwrap().len() as u64, total);
    let tools = [
        ("get_session", json!({"id":key})),
        ("read_transcript", json!({"id":key,"after":0,"limit":1})),
        (
            "read_transcript",
            json!({"id":key,"before":total,"limit":1}),
        ),
        ("read_transcript", complete_args),
    ];
    let expected: Vec<_> = tools
        .iter()
        .map(|(tool, args)| oracle.call_at(tool, args, NOW).unwrap())
        .collect();
    assert_eq!(expected[3], complete);
    drop(oracle);
    let archives = Archives::take(&home, &key);
    let generation = catalog_generation(&home);
    let source_bytes: usize = archives.0.values().map(|archive| archive.bytes.len()).sum();
    for accounting_mib in [64, 128] {
        for ((tool, args), expected) in tools.iter().zip(&expected) {
            let control = if accounting_mib == 64 {
                crate::SelectedReadControl::default()
            } else {
                crate::SelectedReadControl::default().with_limits(131_072, 128 * 1024 * 1024)
            };
            let mut query = Query::from_sources(admitted(home.options.clone()));
            let memory_before = process_memory();
            let io_before = process_io();
            let work_before = archives.work();
            let indexes = crate::events::FILE_READS.with(std::cell::Cell::get);
            let started = Instant::now();
            let actual = {
                let _scope = crate::selected_read::Scope::enter(&control);
                query.call_selected_at(
                    "local",
                    &key,
                    tool,
                    args,
                    Some(&archives),
                    crate::SessionSourceReadMode::ProviderOnly,
                    NOW,
                )
            };
            let elapsed_us = started.elapsed().as_micros();
            let (classification, error) = match actual {
                Ok(actual) => {
                    assert_eq!(
                        &actual, expected,
                        "{accounting_mib} MiB {tool} {args}: admitted result differs from complete native oracle"
                    );
                    assert!(
                        query.native.is_some(),
                        "diagnostic did not reach complete selected preparation"
                    );
                    ("complete", Value::Null)
                }
                Err(error) => {
                    assert_eq!(
                        error.code, "unavailable",
                        "unexpected failure at {accounting_mib} MiB: {error}"
                    );
                    (
                        "unavailable",
                        json!({"code":error.code,"message":error.message}),
                    )
                }
            };
            let process_io_delta = io_before
                .zip(process_io())
                .map(|(before, after)| json!({"rchar":after.0-before.0,"syscr":after.1-before.1}));
            let work = archives.work();
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - indexes;
            if classification == "complete" {
                assert!(
                    loads >= 2,
                    "both selected native owner Indexes must be reached"
                );
            }
            assert_eq!(catalog_generation(&home), generation);
            for archive in archives.0.values() {
                assert!(!home.options.claude_home.join(&archive.source.path).exists());
            }
            eprintln!(
                "recovery_large_shared_budget {}",
                json!({
                    "prefix_records_per_owner":8192,"selected_sources":2,"selected_source_bytes":source_bytes,
                    "accounting_mib":accounting_mib,"production_default":accounting_mib==64,
                    "row_limit":131_072,"deadline_seconds":20,"tool":tool,"args":args,
                    "classification":classification,"error":error,"elapsed_us":elapsed_us,
                    "full_index_loads":loads,"provider_calls":work.0-work_before.0,
                    "provider_bytes":work.1-work_before.1,"process_io_delta":process_io_delta,
                    "memory_before":memory_before,"memory_after":process_memory(),
                    "memory_scope":"process endpoints and lifetime HWM include fixture/oracle and prior cases; not per-request peaks",
                    "scope":"synthetic shared native owners; 128 MiB is cfg(test) only; unavailable is diagnostic evidence, not journey acceptance"
                })
            );
        }
    }
}

#[test]
fn recovery_native_long_session_pages_and_tool_expansion_measurement() {
    for prefix in [0, 128, 1024] {
        let home = Home::new("recovery-native-pages");
        let mut records = vec![human("long", ts(1, 0), "long session prompt")];
        for n in 0..prefix {
            let mut record = assistant("long", ts(1, 1), vec![text(&format!("reply {n}"))]);
            record["uuid"] = json!(format!("reply-{n}"));
            record["message"]["id"] = json!(format!("message-{n}"));
            records.push(record);
        }
        let output = "complete native tool output \"quoted\"\n".repeat(8192);
        records.push(assistant(
            "long",
            ts(23, 58),
            vec![tool(
                "native-call",
                "Bash",
                json!({"command":"printf synthetic"}),
            )],
        ));
        records.push(json!({"type":"user","sessionId":"long","timestamp":ts(23,59),
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"native-call","content":output}]}}));
        home.top("long", &records);
        publish(&home);
        let config = home.options.source.config(&home.options.cache).unwrap();
        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let source_path: String = db
            .query_row(
                "SELECT source_path FROM session_catalog_sources WHERE session_key='long' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let raw = std::fs::read(&source_path).unwrap();
        let generations = || -> (String, String, String) {
            db.query_row("SELECT json_extract(c.metadata,'$.query.source_generation'),json_extract(o.summary,'$.source_generation'),t.source_generation FROM session_catalog c JOIN session_overview o USING(session_key) JOIN session_trace_headers t USING(session_key) WHERE c.session_key='long'",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap()
        };
        let before = generations();
        assert_eq!(before.0, before.1);
        assert_eq!(before.0, before.2);
        let observation_grant = || {
            let metadata: String = db
                .query_row(
                    "SELECT metadata FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key='long'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let mut row: crate::model::summary::CatalogRow =
                serde_json::from_str(&metadata).unwrap();
            row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
            let expected = crate::storage::proof_codec::digest(
                serde_json::to_string(&row).unwrap().as_bytes(),
            );
            let grant: (String, i64, String) = db
                .query_row(
                    "SELECT metadata_digest,admission,stream FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='long' AND owner_kind='catalog'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(grant.0, expected, "exact Catalog metadata grant");
            grant
        };
        let grant_before = observation_grant();
        let mut journal = config.source_observations().unwrap();
        let paths: Vec<String> = Vec::new();
        let operation = crate::storage::source_observations::SourceOperation::capture(
            config.namespace.as_str(),
            "discover",
            &serde_json::to_vec(&(&paths, true)).unwrap(),
        );
        journal.discover(&operation, &paths, true).unwrap();
        assert_eq!(
            journal.backfill_state().unwrap().as_deref(),
            Some("draining")
        );
        db.execute_batch("UPDATE main.session_slot_projections SET version=5 WHERE namespace=semon_namespace() AND session_key='long'; INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope('catalog_completeness'),'catalog_completeness','partial');").unwrap();
        crate::invalidate_session_sources(
            &home.options,
            &[crate::Input {
                root: crate::InputRoot::Claude,
                path: "projects/-work-proj/long.jsonl".into(),
            }],
        )
        .unwrap();
        let revision: i64 = db
            .query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [&source_path],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            crate::overview_repair_sources(&home.options, "long")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            crate::session_transcript_range(
                &home.options,
                "local",
                "sid=long&after=0&limit=1",
                None
            )
            .status,
            503
        );
        assert!(crate::request_session_projection_upgrade(&home.options, "long").unwrap());
        assert_eq!(
            db.query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [&source_path],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            revision
        );
        assert!(
            crate::repair_session_overview(
                &home.options,
                "local",
                "long",
                None,
                crate::SessionSourceReadMode::LocalThenProvider
            )
            .unwrap()
        );
        assert!(
            crate::overview_repair_sources(&home.options, "long")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            db.query_row(
                "SELECT version FROM session_slot_projections WHERE session_key='long'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
            crate::slot_projection::VERSION
        );
        assert_eq!(generations(), before);
        let grant_after = observation_grant();
        // Upgrading recipes with unchanged sources and runtime reproduces the
        // same admitted Query observation and preserves its exact row identity.
        assert_eq!(
            grant_after.0, grant_before.0,
            "unchanged admitted Query row digest"
        );
        assert_eq!(grant_after.1, grant_before.1, "repair preserves admission");
        assert_eq!(grant_after.2, grant_before.2, "repair preserves stream");
        assert_eq!(std::fs::read(&source_path).unwrap(), raw);
        eprintln!("slot_upgrade stage=selected_publication version=6 raw=unchanged");
        let indexes = crate::events::FILE_READS.with(std::cell::Cell::get);
        let started = Instant::now();
        let mut after = 0;
        let mut slots = std::collections::BTreeSet::new();
        let mut generation = None;
        let mut field = None;
        let mut texts = Vec::new();
        let mut pages = 0;
        let mut source_bytes = 0;
        loop {
            let pin = generation
                .as_ref()
                .map(|value| format!("&generation={value}"))
                .unwrap_or_default();
            crate::transcript_range::LAST_LOCAL_FAILURE.with(|stage| stage.set(None));
            let reply = crate::session_transcript_range(
                &home.options,
                "local",
                &format!("sid=long&after={after}&limit=17{pin}"),
                None,
            );
            let stage = crate::transcript_range::last_selected_read_stage();
            let local_stage = crate::transcript_range::LAST_LOCAL_FAILURE
                .with(|stage| stage.get().map(|stage| stage.name()))
                .unwrap_or("none");
            assert_eq!(
                reply.status, 200,
                "selected range refused at closed stage {stage:?}; local source stage {local_stage}"
            );
            let page: Value = serde_json::from_slice(&reply.body).unwrap();
            let current = page["projection"]["generation"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(generation.get_or_insert(current.clone()), &current);
            assert_eq!(page["observation"]["incomplete_entries"], 0);
            source_bytes += page["observation"]["source_bytes"].as_u64().unwrap();
            let entries = page["entries"].as_array().unwrap();
            pages += 1;
            if entries.is_empty() {
                break;
            }
            for entry in entries {
                let slot = entry["slot"].as_u64().unwrap();
                assert!(slots.insert(slot), "duplicate native slot across pages");
                after = slot + 1;
                if matches!(entry["k"].as_str(), Some("h" | "u" | "a")) {
                    texts.push(entry["text"].as_str().unwrap().to_owned());
                }
                if entry["k"] == "tool" {
                    field = Some((slot, entry["field"]["chunks"].as_u64().unwrap()));
                }
            }
        }
        let page_us = started.elapsed().as_micros();
        assert_eq!(
            slots.len(),
            prefix + 2,
            "every prompt, reply and tool slot must be returned"
        );
        let expected_texts: Vec<_> = std::iter::once("long session prompt".to_owned())
            .chain((0..prefix).map(|n| format!("reply {n}")))
            .collect();
        assert_eq!(
            texts, expected_texts,
            "paging must preserve every native text in order"
        );
        let (slot, chunks) = field.expect("tool result must remain expandable");
        let started = Instant::now();
        let mut expanded = String::new();
        for chunk in 0..chunks {
            let reply = crate::session_entry_field(
                &home.options,
                "local",
                &format!(
                    "sid=long&after={slot}&limit=1&generation={}&field_chunk={chunk}",
                    generation.as_ref().unwrap()
                ),
                None,
            );
            assert_eq!(reply.status, 200);
            let part: Value = serde_json::from_slice(&reply.body).unwrap();
            expanded.push_str(part["text"].as_str().unwrap());
        }
        assert_eq!(
            expanded, output,
            "tool expansion must preserve complete content"
        );
        let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - indexes;
        assert_eq!(
            loads, 0,
            "native slot paging must not load complete Indexes"
        );
        eprintln!(
            "recovery_native_pages {}",
            json!({"prefix_records":prefix,"pages":pages,
            "slots":slots.len(),"paging_us":page_us,"paging_source_bytes":source_bytes,
            "field_chunks":chunks,"field_bytes":expanded.len(),"field_expansion_us":started.elapsed().as_micros(),
            "full_index_loads":loads,"scope":"public native range and field APIs; synthetic local source, no browser"})
        );
    }
}

impl crate::SessionSourceReader for HeldProvider {
    fn read_range(
        &self,
        _: &crate::SessionSourceRef,
        _: Option<&str>,
        _: u64,
        _: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        // A fixture-owned whole-operation deadline prevents stranded workers.
        // This demonstrates host callback cancellation, not Index cancellation.
        if !self.started.swap(true, Ordering::Relaxed) {
            self.entered.try_send(()).unwrap();
            let _ = self
                .cancel
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
        }
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "fixture request cancelled",
        ))
    }
}

#[test]
fn recovery_held_fallback_preserves_independent_reads_and_host_cancellation() {
    let (home, key) = shared_home(1024, true);
    let sources = admitted(home.options.clone());
    let mut reference = Query::from_sources(sources.clone());
    let expected_metadata = reference
        .call_catalog_at(
            "list_sessions",
            &json!({}),
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            NOW,
        )
        .unwrap();
    let args = json!({"id":"independent","after":0,"limit":1});
    let expected_transcript = reference
        .call_selected_at(
            "local",
            "independent",
            "read_transcript",
            &args,
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            NOW,
        )
        .unwrap();
    let _archives = Archives::take(&home, &key);
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let provider = HeldProvider {
        entered: entered_tx,
        cancel: std::sync::Mutex::new(cancel_rx),
        started: std::sync::atomic::AtomicBool::new(false),
    };
    std::thread::scope(|scope| {
        let fallback_sources = sources.clone();
        let fallback = scope.spawn(move || {
            let mut query = Query::from_sources(fallback_sources);
            let indexes = crate::events::FILE_READS.with(std::cell::Cell::get);
            let result = query.call_selected_at(
                "local",
                &key,
                "get_session",
                &json!({"id":key}),
                Some(&provider),
                crate::SessionSourceReadMode::ProviderOnly,
                NOW,
            );
            (
                result,
                crate::events::FILE_READS.with(std::cell::Cell::get) - indexes,
            )
        });
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("fallback must reach provider I/O");
        let (done_tx, done_rx) = mpsc::channel();
        let core_sources = sources.clone();
        scope.spawn(move || {
            let mut query = Query::from_sources(core_sources);
            let started = Instant::now();
            let metadata = query.call_catalog_at(
                "list_sessions",
                &json!({}),
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
                NOW,
            );
            let metadata_us = started.elapsed().as_micros();
            let started = Instant::now();
            let transcript = query.call_selected_at(
                "local",
                "independent",
                "read_transcript",
                &args,
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
                NOW,
            );
            done_tx
                .send((
                    metadata,
                    transcript,
                    metadata_us,
                    started.elapsed().as_micros(),
                ))
                .unwrap();
        });
        let core = done_rx.recv_timeout(Duration::from_secs(4));
        let cancel_started = Instant::now();
        cancel_tx.send(()).unwrap();
        let (cancelled, loads) = fallback.join().unwrap();
        let cancel_us = cancel_started.elapsed().as_micros();
        let (metadata, transcript, metadata_us, transcript_us) =
            core.expect("independent core reads blocked behind held fallback");
        assert_eq!(metadata.unwrap(), expected_metadata);
        assert_eq!(transcript.unwrap(), expected_transcript);
        assert!(
            cancelled.is_err(),
            "cancelled provider read returned successful content"
        );
        assert!(loads >= 2, "held request must reach shared Index fallback");
        eprintln!(
            "recovery_held_fallback {}",
            json!({"prefix_records_per_owner":1024,
            "full_index_loads_before_cancel":loads,"metadata_us":metadata_us,"transcript_us":transcript_us,
            "host_callback_cancel_us":cancel_us,"error_code":cancelled.unwrap_err().code,
            "scope":"same admitted sources; provider I/O held, not CPU contention; 4s/5s fixture deadlock deadlines, not release budgets"})
        );
    });
}

#[test]
fn recovery_selected_preparation_terminal_failures_never_replay_or_publish() {
    for failure in ["before", "deadline", "decode", "cpu", "rows", "bytes"] {
        // Without a leading user record, marker lookup reaches the first
        // assistant without provider I/O; cancel inside event ordering.
        let (home, key) = shared_home(128, failure != "cpu");
        let args = json!({"id":key});
        let expected = Query::from_sources(admitted(home.options.clone()))
            .call_at("get_session", &args, NOW)
            .unwrap();
        let archives = Archives::take(&home, &key);
        let generation = catalog_generation(&home);
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(failure == "before"));
        let deadline = if failure == "deadline" {
            Instant::now()
        } else {
            Instant::now() + Duration::from_secs(20)
        };
        let mut control = crate::SelectedReadControl::new(deadline, cancelled.clone());
        if failure == "rows" {
            control = control.with_limits(8, usize::MAX);
        }
        if failure == "bytes" {
            control = control.with_limits(usize::MAX, 1024);
        }
        if failure == "decode" || failure == "cpu" {
            let flag = cancelled.clone();
            crate::selected_read::set_hook(
                if failure == "decode" {
                    "selected_event"
                } else {
                    "history_event"
                },
                if failure == "decode" { 8 } else { 1 },
                move || flag.store(true, Ordering::Release),
            );
        }
        let mut query = Query::from_sources(admitted(home.options.clone()));
        {
            let _scope = crate::selected_read::Scope::enter(&control);
            assert!(
                query
                    .call_selected_at(
                        "local",
                        &key,
                        "get_session",
                        &args,
                        Some(&archives),
                        crate::SessionSourceReadMode::ProviderOnly,
                        NOW
                    )
                    .is_err(),
                "{failure}"
            );
        }
        assert_eq!(
            archives.work(),
            (0, 0),
            "{failure}: terminal preparation fell back to source bytes"
        );
        assert_eq!(
            catalog_generation(&home),
            generation,
            "{failure}: publication changed"
        );
        assert!(
            query.native.is_none(),
            "failed preparation installed partial native evidence"
        );
        assert_eq!(
            query
                .call_selected_at(
                    "local",
                    &key,
                    "get_session",
                    &args,
                    Some(&archives),
                    crate::SessionSourceReadMode::ProviderOnly,
                    NOW
                )
                .unwrap(),
            expected,
            "{failure}: healthy next request lost complete native semantics"
        );
        for archive in archives.0.values() {
            assert!(!home.options.claude_home.join(&archive.source.path).exists());
        }
    }
}

#[test]
fn recovery_selected_nested_scopes_share_budget_and_restore_after_failure() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let control = crate::SelectedReadControl::default().with_limits(1, 1024);
    {
        let _outer = crate::selected_read::Scope::enter(&control);
        db.query_row("SELECT 'first'", [], |row| {
            crate::selected_read::row(row, 32)
        })
        .unwrap();
        {
            let _inner = crate::selected_read::Scope::enter(&crate::SelectedReadControl::default());
            assert!(
                db.query_row("SELECT 'second'", [], |row| crate::selected_read::row(
                    row, 32
                ))
                .is_err()
            );
        }
        assert_eq!(
            crate::selected_read::check().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
    assert!(!crate::selected_read::active());
    db.query_row("SELECT 'healthy'", [], |row| {
        crate::selected_read::row(row, 32)
    })
    .unwrap();
}

#[test]
fn recovery_selected_sql_progress_observes_request_cancellation() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let control = crate::SelectedReadControl::new(
        Instant::now() + Duration::from_secs(20),
        cancelled.clone(),
    );
    let _scope = crate::selected_read::Scope::enter(&control);
    crate::selected_read::install(&db).unwrap();
    cancelled.store(true, Ordering::Release);
    assert!(db.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0)).is_err());
}

#[test]
fn recovery_selected_repair_cancellation_rolls_back_publication() {
    for ready in [true, false] {
        let (home, _) = shared_home(16, true);
        let config = home.options.source.config(&home.options.cache).unwrap();
        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        if !ready {
            db.execute(
                "UPDATE main.session_slot_projections SET version=5 WHERE namespace=semon_namespace() AND session_key='first'",
                [],
            )
            .unwrap();
        }
        db.execute(
            "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_namespace(),'overview_needed:first','1')",
            [],
        )
        .unwrap();
        let metadata: String = db
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key='first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let original_publication = recovery_publication_snapshot(&db, "first");
        let generation = catalog_generation(&home);
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let control =
            crate::SelectedReadControl::new(Instant::now() + Duration::from_secs(20), flag.clone());
        crate::selected_read::set_hook("selected_publication", 0, move || {
            flag.store(true, Ordering::Release)
        });
        let error = crate::repair_session_overview_with_control(
            &home.options,
            "local",
            "first",
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            &control,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(catalog_generation(&home), generation);
        assert_eq!(
            recovery_publication_snapshot(&db, "first"),
            original_publication
        );
        assert_eq!(
            db.query_row(
                "SELECT metadata FROM session_catalog WHERE session_key='first'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            metadata
        );
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key='overview_needed:first')",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        crate::repair_session_overview(
            &home.options,
            "local",
            "first",
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
        )
        .unwrap();
    }
}

#[test]
fn recovery_tampered_preimage_refuses_before_producer_reads_or_writes() {
    let home = Home::new("recovery-tampered-preimage");
    home.top(
        "first",
        &[human("first", ts(1, 0), "tampered preimage refusal")],
    );
    publish_semantic(&home);
    let config = home.options.source.config(&home.options.cache).unwrap();
    let db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    db.execute(
        "UPDATE main.session_catalog SET metadata=json_remove(metadata,'$.query') WHERE namespace=semon_namespace() AND session_key='first'",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_namespace(),'overview_needed:first','1')",
        [],
    )
    .unwrap();

    let metadata_before = recovery_catalog_metadata(&db, "session_catalog", "first");
    let grant_before = recovery_observation_grant(&db, "first", "catalog").unwrap();
    let proof_snapshot = recovery_proof_snapshot(&db, "first");
    let stage_grants_before = recovery_stage_observation_grants(&db);
    let publication_before = recovery_publication_snapshot(&db, "first");
    let generation_before = catalog_generation(&home);
    let file_reads_before = crate::events::FILE_READS.with(std::cell::Cell::get);
    let source_read_bytes_before = crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get);
    let direct_source_reads_before = crate::model::DIRECT_SOURCE_READS.with(std::cell::Cell::get);

    let error = crate::repair_session_overview(
        &home.options,
        "local",
        "first",
        None,
        crate::SessionSourceReadMode::LocalThenProvider,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(
        error.to_string(),
        crate::storage::StorageError::ScopeChanged.to_string()
    );
    assert_eq!(
        recovery_catalog_metadata(&db, "session_catalog", "first"),
        metadata_before
    );
    assert_eq!(
        recovery_observation_grant(&db, "first", "catalog"),
        Some(grant_before)
    );
    assert_eq!(recovery_proof_snapshot(&db, "first"), proof_snapshot);
    assert_eq!(recovery_stage_observation_grants(&db), stage_grants_before);
    assert_eq!(
        recovery_publication_snapshot(&db, "first"),
        publication_before
    );
    assert_eq!(catalog_generation(&home), generation_before);
    assert!(db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace=semon_namespace() AND key='overview_needed:first' AND value='1')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap());
    assert_eq!(
        crate::events::FILE_READS.with(std::cell::Cell::get),
        file_reads_before
    );
    assert_eq!(
        crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get),
        source_read_bytes_before
    );
    assert_eq!(
        crate::model::DIRECT_SOURCE_READS.with(std::cell::Cell::get),
        direct_source_reads_before
    );
}

#[test]
fn recovery_selected_ledger_fence_precedes_complete_index_decode() {
    let (home, _) = shared_home(128, true);
    let config = home.options.source.config(&home.options.cache).unwrap();
    let db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let encoded: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='first'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row: crate::model::summary::CatalogRow = serde_json::from_str(&encoded).unwrap();
    db.execute(
        "UPDATE main.files SET head_sha256=zeroblob(32) WHERE namespace=semon_namespace() AND path=?1",
        [row.sources[0].path.to_str().unwrap()],
    )
    .unwrap();
    let before = crate::events::FILE_READS.with(std::cell::Cell::get);
    let _scope = crate::selected_read::Scope::enter(&crate::SelectedReadControl::default());
    assert!(
        crate::events::EventCache::from_selected(
            &db,
            &row.sources,
            &std::collections::BTreeSet::from(["first".to_owned()])
        )
        .is_err()
    );
    assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), before);
}

#[test]
fn recovery_history_helper_contract_updates_only_authorized_owner_for_reused_key() {
    let home = Home::new("recovery-history-owner");
    home.top(
        "reused",
        &[human("reused", ts(1, 0), "original native prompt")],
    );
    publish(&home);
    publish_semantic(&home);
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/-work-proj/reused.jsonl".into(),
    };
    publish_semantic_stage(&home, &input);
    publish(&home);
    std::fs::remove_file(
        home.options
            .claude_home
            .join("projects/-work-proj/reused.jsonl"),
    )
    .unwrap();
    publish_semantic(&home);
    home.top(
        "reused",
        &[human("reused", ts(1, 0), "replacement native prompt")],
    );
    publish_semantic(&home);
    home.process(30, 777);
    home.pid_file(30, "reused", "busy", 777);
    publish_semantic(&home);
    publish_semantic_stage(&home, &input);
    publish(&home);

    let config = home.options.source.config(&home.options.cache).unwrap();
    let mut db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let history = recovery_catalog_row(&db, "session_history_catalog", "reused");
    let current = recovery_catalog_row(&db, "session_catalog", "reused");
    assert_ne!(
        crate::retention::source_generation(&history.sources).unwrap(),
        crate::retention::source_generation(&current.sources).unwrap(),
        "the real producer must retain a divergent Current row for this key"
    );
    let proof_refs = recovery_proof_snapshot(&db, "reused");
    assert!(proof_refs.iter().any(|proof| proof.0 == "current"));
    assert!(proof_refs.iter().any(|proof| proof.0 == "history"));
    let current_capture: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='reused' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let history_capture: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='reused' AND owner_kind='history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(current_capture.is_none());
    assert!(history_capture.is_none());
    let retained_history = crate::model::summary::CatalogRow {
        lifecycle: crate::model::summary::CatalogLifecycle::Retained,
        ..history.clone()
    };
    let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
    let tx = db.transaction().unwrap();
    let current_binding = binding.runtime_binding_at(&tx).unwrap();
    assert!(
        crate::storage::sqlite::proof_generations::observation_binding_with_observations(
            &tx,
            &current_binding,
            &retained_history,
            crate::storage::catalog::CatalogObservationOwner::History,
        )
        .is_ok()
    );
    tx.rollback().unwrap();
    // This is actual History metadata, independently retained by the producer.
    // Removing its immutable root cannot be bypassed by either native grant.
    let proof = recovery_proof_snapshot(&db, "reused")
        .into_iter()
        .find(|proof| proof.0 == "history")
        .unwrap();
    let (base, manifest): (String, String) = db.query_row(
        "SELECT base,manifest FROM storage_proof_presentation_roots WHERE namespace=semon_namespace() AND digest=?1",
        [&proof.3], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    db.execute("DELETE FROM storage_proof_presentation_roots WHERE namespace=semon_namespace() AND digest=?1", [&proof.3]).unwrap();
    let tx = db.transaction().unwrap();
    assert!(
        crate::storage::sqlite::proof_generations::observation_binding_with_observations(
            &tx,
            &current_binding,
            &retained_history,
            crate::storage::catalog::CatalogObservationOwner::History,
        )
        .is_err()
    );
    tx.rollback().unwrap();
    db.execute(
        "INSERT INTO storage_proof_presentation_roots VALUES(semon_namespace(),?1,?2,?3,?4)",
        rusqlite::params![proof.2, base, manifest, proof.3],
    )
    .unwrap();

    let history_query = history.query.as_ref().unwrap();
    let current_query = current.query.as_ref().unwrap();
    assert_ne!(history_query["runtime"], current_query["runtime"]);
    // This focused helper-contract test supplies a genuine producer runtime
    // witness from the later Current publication while retaining History's
    // own original row and source generation.
    let mut repaired_query = history_query.clone();
    // The runtime witness comes from the producer's later Current publication;
    // the History observation and its source generation remain independently
    // bound to the retained original.
    repaired_query["runtime"] = current_query["runtime"].clone();
    assert_eq!(
        repaired_query["source_generation"],
        crate::retention::source_generation(&history.sources).unwrap()
    );

    let catalog_before = recovery_catalog_metadata(&db, "session_catalog", "reused");
    let history_before = recovery_catalog_metadata(&db, "session_history_catalog", "reused");
    let catalog_grant_before = recovery_observation_grant(&db, "reused", "catalog").unwrap();
    let history_grant_before = recovery_observation_grant(&db, "reused", "history").unwrap();
    let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let current_binding = binding.runtime_binding_at(&tx).unwrap();
    crate::storage::sqlite::proof_generations::publish_repaired_query_observation(
        &tx,
        &current_binding,
        crate::storage::catalog::CatalogObservationOwner::History,
        &history,
        &repaired_query,
    )
    .unwrap();
    tx.commit().unwrap();

    assert_eq!(
        recovery_catalog_metadata(&db, "session_catalog", "reused"),
        catalog_before,
        "repairing History must leave the divergent Current row byte for byte intact"
    );
    let history_after = recovery_catalog_row(&db, "session_history_catalog", "reused");
    assert_eq!(history_after.query.as_ref(), Some(&repaired_query));
    assert_ne!(
        recovery_catalog_metadata(&db, "session_history_catalog", "reused"),
        history_before
    );
    let catalog_grant_after = recovery_observation_grant(&db, "reused", "catalog").unwrap();
    assert_eq!(catalog_grant_after, catalog_grant_before);
    let mut normalized_history = history.clone();
    normalized_history.query = Some(repaired_query);
    normalized_history.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let expected_history_digest = crate::storage::proof_codec::digest(
        serde_json::to_string(&normalized_history)
            .unwrap()
            .as_bytes(),
    );
    let history_grant_after = recovery_observation_grant(&db, "reused", "history").unwrap();
    assert_eq!(history_grant_after.0, expected_history_digest);
    assert_eq!(history_grant_after.1, history_grant_before.1);
    assert_eq!(history_grant_after.2, history_grant_before.2);
}

#[test]
fn recovery_ordinary_direct_capture_transfers_to_producer_retired_history() {
    let home = Home::new("recovery-direct-history-transfer");
    home.top(
        "lead",
        &[
            human("lead", ts(1, 0), "direct-only retained prompt"),
            assistant("lead", ts(1, 1), vec![text("retained answer")]),
        ],
    );
    publish(&home);

    let args = json!({"id":"lead","after":0,"limit":10});
    let expected = home.query().call_at("read_transcript", &args, NOW).unwrap();
    let archive = selected_archive(&home, 0);
    let config = home.options.source.config(&home.options.cache).unwrap();
    let mut db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();

    let current = recovery_catalog_row(&db, "session_catalog", "lead");
    assert!(recovery_proof_snapshot(&db, "lead").is_empty());
    let direct_capture: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='lead' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let direct_capture = direct_capture.unwrap();
    let before_grant = recovery_observation_grant(&db, "lead", "catalog").unwrap();

    fs::remove_file(home.options.claude_home.join(&archive.source.path)).unwrap();
    publish(&home);

    let retained = recovery_catalog_row(&db, "session_history_catalog", "lead");
    assert_eq!(
        retained.lifecycle,
        crate::model::summary::CatalogLifecycle::Retained
    );
    assert_eq!(retained.sources, current.sources);
    assert_eq!(retained.query, current.query);
    assert!(recovery_proof_snapshot(&db, "lead").is_empty());

    let history_capture: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='lead' AND owner_kind='history'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history_capture.as_deref(), Some(direct_capture.as_str()));
    let history_grant = recovery_observation_grant(&db, "lead", "history").unwrap();
    let mut normalized_history = retained.clone();
    normalized_history.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    assert_eq!(
        history_grant.0,
        crate::storage::proof_codec::digest(
            serde_json::to_string(&normalized_history)
                .unwrap()
                .as_bytes()
        )
    );
    assert_eq!(history_grant.1, before_grant.1);
    assert_eq!(history_grant.2, before_grant.2);

    let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
    let tx = db.transaction().unwrap();
    let current_binding = binding.runtime_binding_at(&tx).unwrap();
    assert!(
        !crate::storage::sqlite::proof_generations::imported_catalog(&tx, &current_binding)
            .unwrap()
    );
    let (original_binding, observations) =
        crate::storage::sqlite::proof_generations::observation_binding_with_observations(
            &tx,
            &current_binding,
            &retained,
            crate::storage::catalog::CatalogObservationOwner::History,
        )
        .unwrap();
    assert_eq!(original_binding.namespace, current_binding.namespace);
    assert_eq!(original_binding.admission, current_binding.admission);
    assert_eq!(original_binding.stream_epoch, current_binding.stream_epoch);
    assert_eq!(
        observations.runtime_for(&retained).as_deref(),
        retained.query.as_ref().unwrap()["runtime"].as_str()
    );
    tx.rollback().unwrap();

    let mut query = Query::from_sources(admitted(home.options.clone()));
    assert_eq!(
        query
            .call_selected_at(
                "local",
                "lead",
                "read_transcript",
                &args,
                Some(&archive),
                crate::SessionSourceReadMode::ProviderOnly,
                NOW,
            )
            .unwrap(),
        expected,
        "the retained selected read must use the transferred original binding"
    );
}

#[test]
fn recovery_stage_only_preimage_authenticates_before_inserting_current_grant() {
    let home = Home::new("recovery-stage-only");
    home.top(
        "first",
        &[human("first", ts(1, 0), "stage-only query repair")],
    );
    publish_semantic(&home);
    // A closed single-source component normally takes the direct publisher.
    // Explicitly drive the admitted Stage with the producer's real indexed
    // Claude input so its immutable receipt, presentation and grant exist.
    publish_semantic_stage(
        &home,
        &crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/-work-proj/first.jsonl".into(),
        },
    );
    let config = home.options.source.config(&home.options.cache).unwrap();
    let mut db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let selected = recovery_catalog_row(&db, "session_catalog", "first");
    let proof_snapshot = recovery_proof_snapshot(&db, "first");
    assert!(
        !proof_snapshot.is_empty(),
        "fixture must have immutable Stage proof"
    );
    let provenance_before = recovery_stage_provenance(&db, "first");
    assert!(!provenance_before.is_empty());
    let stage_grants_before = recovery_stage_observation_grants(&db);
    let preimage_stage_digest = crate::storage::proof_codec::digest(
        recovery_catalog_metadata(&db, "session_catalog", "first").as_bytes(),
    );
    assert!(
        stage_grants_before
            .iter()
            .any(|(grant_key, _)| grant_key.ends_with(&preimage_stage_digest))
    );
    let direct_grant_before = recovery_observation_grant(&db, "first", "catalog").unwrap();
    assert_eq!(
        db.execute(
            "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
        )
        .unwrap(),
        1
    );
    db.execute(
        "UPDATE main.session_slot_projections SET version=5 WHERE namespace=semon_namespace() AND session_key='first'",
        [],
    )
    .unwrap();
    home.process(31, 778);
    home.pid_file(31, "first", "busy", 778);
    assert_eq!(recovery_observation_grant(&db, "first", "catalog"), None);
    assert_eq!(recovery_observation_grant(&db, "first", "history"), None);

    let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let current_binding = binding.runtime_binding_at(&tx).unwrap();
    assert_eq!(current_binding.namespace, config.namespace);
    assert!(stage_grants_before.iter().any(|(key, value)| {
        key == &format!("catalog_native_observation:{preimage_stage_digest}")
            && value
                == &format!(
                    "{}:{}",
                    current_binding.admission, current_binding.stream_epoch
                )
    }));
    assert_eq!(
        crate::storage::sqlite::proof_generations::observation_binding(
            &tx,
            &current_binding,
            &selected
        )
        .unwrap()
        .admission,
        current_binding.admission,
        "the exact immutable Stage proof must authorize after the direct slot is removed"
    );
    tx.rollback().unwrap();
    assert_eq!(recovery_observation_grant(&db, "first", "catalog"), None);

    assert!(
        crate::repair_session_overview(
            &home.options,
            "local",
            "first",
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
        )
        .unwrap()
    );

    let repaired = recovery_catalog_row(&db, "session_catalog", "first");
    assert_ne!(
        repaired.query.as_ref().unwrap()["runtime"],
        selected.query.as_ref().unwrap()["runtime"],
        "runtime evidence changed in the real producer"
    );
    assert_eq!(
        repaired.query.as_ref().unwrap()["source_generation"],
        crate::retention::source_generation(&selected.sources).unwrap()
    );
    let expected_direct_digest = {
        let mut normalized = repaired.clone();
        normalized.lifecycle = crate::model::summary::CatalogLifecycle::Current;
        crate::storage::proof_codec::digest(serde_json::to_string(&normalized).unwrap().as_bytes())
    };
    let direct_grant_after = recovery_observation_grant(&db, "first", "catalog").unwrap();
    assert_eq!(direct_grant_after.0, expected_direct_digest);
    assert_eq!(direct_grant_after.1, direct_grant_before.1);
    assert_eq!(direct_grant_after.2, direct_grant_before.2);
    assert_eq!(recovery_proof_snapshot(&db, "first"), proof_snapshot);
    assert_eq!(recovery_stage_provenance(&db, "first"), provenance_before);
    assert_eq!(recovery_stage_observation_grants(&db), stage_grants_before);
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let resolved = crate::storage::sqlite::proof_generations::observation_binding(
        &tx,
        &current_binding,
        &repaired,
    )
    .unwrap();
    assert_eq!(resolved.admission, current_binding.admission);
    assert_eq!(resolved.stream_epoch, current_binding.stream_epoch);
    tx.rollback().unwrap();
}

#[test]
fn recovery_stage_upgrades_direct_authority_then_reuses_immutable_publication() {
    let home = Home::new("recovery-stage-reuse");
    home.top(
        "first",
        &[
            human("first", ts(1, 0), "Publication reuse prompt"),
            assistant("first", ts(1, 1), vec![text("Publication reuse answer")]),
        ],
    );
    publish(&home);
    let config = home.options.source.config(&home.options.cache).unwrap();
    let db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let generation = || {
        db.query_row(
        "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
        [], |row| row.get::<_, String>(0),
    ).unwrap()
    };
    let direct_generation = generation();
    assert!(recovery_proof_snapshot(&db, "first").is_empty());
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/-work-proj/first.jsonl".into(),
    };
    publish_semantic_stage(&home, &input);
    let immutable_generation = generation();
    assert_ne!(immutable_generation, direct_generation);
    let provenance = recovery_stage_provenance(&db, "first");
    let refs = recovery_proof_snapshot(&db, "first");
    assert!(!provenance.is_empty());
    let grants = recovery_stage_observation_grants(&db);
    publish_semantic_stage(&home, &input);
    assert_eq!(generation(), immutable_generation);
    assert_eq!(recovery_stage_provenance(&db, "first"), provenance);
    assert_eq!(recovery_proof_snapshot(&db, "first"), refs);
    assert_eq!(recovery_stage_observation_grants(&db), grants);
    assert_eq!(recovery_selected_transcript(&home, "first").status, 200);
}

#[test]
fn recovery_index_ahead_repriced_snapshot_uses_bound_global_runtime() {
    use crate::storage::catalog::CatalogObservationOwner;
    use crate::storage::catalog::CatalogSnapshot;
    use crate::storage::sqlite::proof_generations::prepare_repriced_observation;

    let home = Home::new("recovery-index-ahead-runtime");
    home.top(
        "first",
        &[
            human("first", ts(1, 0), "Index ahead prompt"),
            assistant("first", ts(1, 1), vec![text("Index ahead answer")]),
        ],
    );
    publish(&home);
    let config = home.options.source.config(&home.options.cache).unwrap();
    let before_refresh = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let direct_before_refresh: Option<String> = before_refresh
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(direct_before_refresh.is_some());
    drop(before_refresh);
    // The ordinary refresh may publish a closed component directly. Drive
    // the real Stage explicitly before asserting immutable proof ownership.
    publish_semantic(&home);
    publish_semantic_stage(
        &home,
        &crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/-work-proj/first.jsonl".into(),
        },
    );

    let page = crate::session_catalog_page(&home.options, "local", "sid=first");
    assert_eq!(page.status, 200);
    let config = home.options.source.config(&home.options.cache).unwrap();
    let mut db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let selected = recovery_catalog_row(&db, "session_catalog", "first");
    let direct_capture_after_stage: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        direct_capture_after_stage.is_none(),
        "semantic Stage keeps runtime observations in its immutable presentation proof"
    );
    let proof_snapshot = recovery_proof_snapshot(&db, "first");
    assert!(proof_snapshot.iter().any(|proof| proof.0 == "current"));
    let metadata_before_identical_publish =
        recovery_catalog_metadata(&db, "session_catalog", "first");
    publish(&home);
    assert_eq!(
        recovery_catalog_metadata(&db, "session_catalog", "first"),
        metadata_before_identical_publish,
        "the ordinary producer republishes the identical selected row"
    );
    let direct_capture_after_identical_publish: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(direct_capture_after_identical_publish.is_none());
    assert_eq!(recovery_selected_transcript(&home, "first").status, 200);
    db.execute(
        "UPDATE storage_catalog_native_observation_grants SET direct_capture='invalid direct capture' WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
        [],
    )
    .unwrap();
    assert_eq!(
        recovery_selected_transcript(&home, "first").status,
        200,
        "a valid exact semantic owner remains authoritative over a stale direct value"
    );
    db.execute(
        "UPDATE storage_catalog_native_observation_grants SET direct_capture=NULL WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
        [],
    )
    .unwrap();
    let hostname = home.options.proc_root.join("sys/kernel/hostname");
    let original_hostname = fs::read(&hostname).unwrap();
    fs::write(&hostname, "recovery-index-ahead-runtime-drift\n").unwrap();
    publish(&home);
    let changed_row = recovery_catalog_row(&db, "session_catalog", "first");
    assert_ne!(
        recovery_catalog_metadata(&db, "session_catalog", "first"),
        metadata_before_identical_publish
    );
    assert_eq!(
        crate::retention::source_generation(&changed_row.sources).unwrap(),
        crate::retention::source_generation(&selected.sources).unwrap(),
        "Facts drift keeps the Index source generation"
    );
    assert_ne!(
        changed_row.query.as_ref().unwrap()["runtime"],
        selected.query.as_ref().unwrap()["runtime"],
        "the ordinary producer captures the changed global runtime"
    );
    let fresh_direct_capture: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(fresh_direct_capture.is_some());
    assert_eq!(recovery_selected_transcript(&home, "first").status, 200);
    fs::write(&hostname, original_hostname).unwrap();
    assert_eq!(recovery_selected_transcript(&home, "first").status, 503);
    publish(&home);
    assert_eq!(recovery_selected_transcript(&home, "first").status, 200);
    // Restoring hostname bytes changes its stat-based global observation.
    // Capture that observation through the real Stage before repricing it.
    publish_semantic_stage(
        &home,
        &crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/-work-proj/first.jsonl".into(),
        },
    );
    let selected = recovery_catalog_row(&db, "session_catalog", "first");
    let proof_snapshot = recovery_proof_snapshot(&db, "first");
    let proof_provenance = recovery_stage_provenance(&db, "first");
    let direct_capture_before_reprice: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(direct_capture_before_reprice.is_none());
    let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
    let tx = db.transaction().unwrap();
    let current_binding = binding.runtime_binding_at(&tx).unwrap();
    let (_, observations) =
        crate::storage::sqlite::proof_generations::observation_binding_with_observations(
            &tx,
            &current_binding,
            &selected,
            crate::storage::catalog::CatalogObservationOwner::Catalog,
        )
        .unwrap();
    let captured_source_runtime =
        crate::native_evidence::query_runtime_generation(&home.options).unwrap();
    assert_eq!(
        observations.source_runtime.as_deref(),
        Some(captured_source_runtime.as_str()),
        "the authenticated Stage publication must capture the current raw global observation"
    );
    let mut cost = selected.cost.clone();
    cost.usd = Some(cost.usd.unwrap_or_default() + 1.0);
    prepare_repriced_observation(
        &tx,
        &current_binding,
        CatalogObservationOwner::Catalog,
        &selected,
        &cost,
        selected.query.as_ref(),
    )
    .unwrap()
    .publish(&tx)
    .unwrap();
    tx.commit().unwrap();
    let repriced = recovery_catalog_row(&db, "session_catalog", "first");
    assert_ne!(repriced.cost.usd, selected.cost.usd);
    assert_eq!(repriced.query, selected.query);
    assert_eq!(recovery_proof_snapshot(&db, "first"), proof_snapshot);
    assert_eq!(recovery_stage_provenance(&db, "first"), proof_provenance);
    let direct_capture_after_reprice: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(direct_capture_after_reprice.is_none());

    let path = repriced.sources[0].path.to_string_lossy().into_owned();
    assert_eq!(
        db.execute(
            "UPDATE main.files SET size=size+1,resume_at=resume_at+1 WHERE namespace=semon_namespace() AND path=?1",
            [&path],
        )
        .unwrap(),
        1
    );
    assert!(
        crate::storage::sqlite::catalog::runtime_source_cwds_if_current(&db, &repriced.sources)
            .unwrap()
            .is_none()
    );

    let tx = db.transaction().unwrap();
    let snapshot = crate::storage::sqlite::catalog::admitted_snapshot(&binding, &tx).unwrap();
    let global = snapshot.runtime_generation(&home.options).unwrap();
    assert_eq!(global, captured_source_runtime);
    let captured_runtime = selected
        .query
        .as_ref()
        .and_then(|query| query["runtime"].as_str())
        .unwrap();
    assert_eq!(
        snapshot
            .runtime_observation(
                &home.options,
                &repriced,
                crate::storage::catalog::CatalogObservationOwner::Catalog,
                &global
            )
            .unwrap()
            .as_deref(),
        Some(captured_runtime),
        "the authenticated publication must preserve its captured runtime after a price-only rebind and Index advance"
    );

    let hostname = home.options.proc_root.join("sys/kernel/hostname");
    let original_hostname = fs::read(&hostname).unwrap();
    fs::write(&hostname, "recovery-index-ahead-runtime-drift\n").unwrap();
    let changed_global = snapshot.runtime_generation(&home.options).unwrap();
    let refused = snapshot.runtime_observation(
        &home.options,
        &repriced,
        crate::storage::catalog::CatalogObservationOwner::Catalog,
        &changed_global,
    );
    fs::write(&hostname, original_hostname).unwrap();
    assert_ne!(changed_global, global);
    assert!(
        refused.is_err(),
        "a changed global runtime must not reuse the publication's captured row witness"
    );
    tx.rollback().unwrap();
}

#[test]
fn recovery_direct_capture_does_not_mask_a_damaged_semantic_owner() {
    let home = Home::new("recovery-direct-damaged-proof");
    home.top(
        "first",
        &[
            human("first", ts(1, 0), "proof owner prompt"),
            assistant("first", ts(1, 1), vec![text("proof owner answer")]),
        ],
    );
    publish(&home);
    publish_semantic(&home);
    publish_semantic_stage(
        &home,
        &crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/-work-proj/first.jsonl".into(),
        },
    );
    publish(&home);

    let config = home.options.source.config(&home.options.cache).unwrap();
    let db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let before = recovery_catalog_row(&db, "session_catalog", "first");
    let hostname = home.options.proc_root.join("sys/kernel/hostname");
    let original_hostname = fs::read(&hostname).unwrap();
    fs::write(&hostname, "recovery-direct-damaged-proof-drift\n").unwrap();
    publish(&home);
    let after = recovery_catalog_row(&db, "session_catalog", "first");
    assert_eq!(
        crate::retention::source_generation(&before.sources).unwrap(),
        crate::retention::source_generation(&after.sources).unwrap()
    );
    assert_ne!(
        before.query.as_ref().unwrap()["runtime"],
        after.query.as_ref().unwrap()["runtime"]
    );
    let direct_capture: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(direct_capture.is_some());
    assert_eq!(recovery_selected_transcript(&home, "first").status, 200);

    fs::write(&hostname, "recovery-direct-witness-stale\n").unwrap();
    assert_eq!(recovery_selected_transcript(&home, "first").status, 503);
    fs::write(&hostname, b"recovery-direct-damaged-proof-drift\n").unwrap();

    let current_id = recovery_proof_snapshot(&db, "first")
        .into_iter()
        .find(|proof| proof.0 == "current")
        .unwrap()
        .1;
    assert_eq!(
        db.execute(
            "DELETE FROM storage_proof_presentations WHERE namespace=semon_namespace() AND id=?1 AND session_key='first'",
            [&current_id],
        )
        .unwrap(),
        1
    );
    assert_eq!(
        recovery_selected_transcript(&home, "first").status,
        503,
        "a damaged exact semantic owner blocks fallback to a valid direct capture"
    );
    fs::write(&hostname, original_hostname).unwrap();
}

#[test]
fn recovery_immutable_corruption_blocks_selected_bytes_and_preimage_publication() {
    use crate::storage::catalog::CatalogObservationOwner;
    use crate::storage::sqlite::proof_generations::{
        ObservationPreparationError, prepare_repriced_observation,
    };

    for corruption in ["root", "entry", "generation"] {
        let home = Home::new("recovery-immutable-preimage");
        home.top(
            "first",
            &[
                human("first", ts(1, 0), "immutable owner prompt"),
                assistant("first", ts(1, 1), vec![text("immutable owner answer")]),
            ],
        );
        publish(&home);
        publish_semantic_stage(
            &home,
            &crate::Input {
                root: crate::InputRoot::Claude,
                path: "projects/-work-proj/first.jsonl".into(),
            },
        );
        publish(&home);
        let config = home.options.source.config(&home.options.cache).unwrap();
        let mut db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let row = recovery_catalog_row(&db, "session_catalog", "first");
        assert!(recovery_observation_grant(&db, "first", "catalog").is_some());
        let archive = SelectedArchive {
            source: crate::SessionSourceRef {
                root: "claude".into(),
                path: "projects/-work-proj/first.jsonl".into(),
                native_id: row.sources[0].native_id.clone(),
                offset: row.sources[0].offset,
                prefix_sha256: row.sources[0].prefix_sha256,
                tail_sha256: row.sources[0].tail_sha256,
            },
            bytes: Arc::from(fs::read(&row.sources[0].path).unwrap()),
            calls: std::sync::atomic::AtomicUsize::new(0),
            read_bytes: AtomicU64::new(0),
            fault: 0,
        };
        let selected = || {
            crate::session_transcript_range_with_mode(
                &home.options,
                "local",
                "sid=first&after=0&limit=1",
                Some(&archive),
                crate::SessionSourceReadMode::ProviderOnly,
            )
        };
        assert_eq!(selected().status, 200);
        assert!(archive.calls.load(Ordering::Relaxed) > 0);
        archive.calls.store(0, Ordering::Relaxed);
        let proof = recovery_proof_snapshot(&db, "first")
            .into_iter()
            .find(|proof| proof.0 == "current")
            .unwrap();
        match corruption {
            "root" => {
                let (base, manifest): (String, String) = db
                    .query_row(
                        "SELECT base,manifest FROM storage_proof_presentation_roots WHERE namespace=semon_namespace() AND digest=?1",
                        [&proof.3],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap();
                db.execute(
                    "DELETE FROM storage_proof_presentation_roots WHERE namespace=semon_namespace() AND digest=?1",
                    [&proof.3],
                )
                .unwrap();
                db.execute(
                    "INSERT INTO storage_proof_presentation_roots VALUES(semon_namespace(),?1,?2,?3,?4)",
                    rusqlite::params![proof.2, format!("{base}-damaged"), manifest, proof.3],
                )
                .unwrap();
            }
            "entry" => {
                db.execute(
                    "DELETE FROM storage_proof_presentations WHERE namespace=semon_namespace() AND id=?1 AND session_key='first'",
                    [&proof.1],
                )
                .unwrap();
                db.execute(
                    "INSERT INTO storage_proof_presentations VALUES(semon_namespace(),?1,'first','null')",
                    [&proof.1],
                )
                .unwrap();
            }
            "generation" => {
                db.execute(
                    "DELETE FROM storage_proof_generations WHERE namespace=semon_namespace() AND id=?1 AND session_key='first'",
                    [&proof.1],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let authority_snapshot = |db: &rusqlite::Connection| {
            db.prepare(
                "SELECT 'grant',json_array(owner_kind,metadata_digest,admission,stream,direct_capture) FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first'
                 UNION ALL SELECT 'reference',json_array(kind,id) FROM storage_proof_refs WHERE namespace=semon_namespace() AND session_key='first'
                 UNION ALL SELECT 'generation',json_array(id,publication,proof,admission,stream,metadata,slots,prefix) FROM storage_proof_generations WHERE namespace=semon_namespace() AND session_key='first'
                 UNION ALL SELECT 'presentation',json_array(id,metadata) FROM storage_proof_presentations WHERE namespace=semon_namespace() AND session_key='first'
                 UNION ALL SELECT 'root',json_array(publication,base,manifest,digest) FROM storage_proof_presentation_roots WHERE namespace=semon_namespace()
                 UNION ALL SELECT 'source',json_array(id,path,ledger) FROM storage_proof_sources WHERE namespace=semon_namespace() AND session_key='first'
                 ORDER BY 1,2",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        let authority_before = authority_snapshot(&db);
        assert_eq!(selected().status, 503, "{corruption}");
        assert_eq!(archive.calls.load(Ordering::Relaxed), 0, "{corruption}");
        assert_eq!(authority_snapshot(&db), authority_before, "{corruption}");

        // Force genuine selected preparation, rather than the ready/no-op path.
        db.execute(
            "UPDATE main.session_slot_projections SET version=5 WHERE namespace=semon_namespace() AND session_key='first'",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_namespace(),'overview_needed:first','1')",
            [],
        )
        .unwrap();
        let metadata_before = recovery_catalog_metadata(&db, "session_catalog", "first");
        let publication_before = recovery_publication_snapshot(&db, "first");
        let stage_grants_before = recovery_stage_observation_grants(&db);
        let generation_before = catalog_generation(&home);
        let reads_before = (
            crate::events::FILE_READS.with(std::cell::Cell::get),
            crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get),
            crate::model::DIRECT_SOURCE_READS.with(std::cell::Cell::get),
        );
        assert!(
            crate::repair_session_overview(
                &home.options,
                "local",
                "first",
                Some(&archive),
                crate::SessionSourceReadMode::ProviderOnly,
            )
            .is_err(),
            "{corruption}"
        );
        let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
        let tx = db.transaction().unwrap();
        let current = binding.runtime_binding_at(&tx).unwrap();
        let mut cost = row.cost.clone();
        cost.usd = Some(cost.usd.unwrap_or_default() + 1.0);
        assert!(matches!(
            prepare_repriced_observation(
                &tx,
                &current,
                CatalogObservationOwner::Catalog,
                &row,
                &cost,
                row.query.as_ref(),
            ),
            Err(ObservationPreparationError::Evidence(
                crate::storage::StorageError::ScopeChanged
            ))
        ));
        tx.commit().unwrap();
        assert_eq!(archive.calls.load(Ordering::Relaxed), 0, "{corruption}");
        assert_eq!(
            (
                crate::events::FILE_READS.with(std::cell::Cell::get),
                crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get),
                crate::model::DIRECT_SOURCE_READS.with(std::cell::Cell::get),
            ),
            reads_before,
            "{corruption}"
        );
        assert_eq!(authority_snapshot(&db), authority_before, "{corruption}");
        assert_eq!(
            recovery_catalog_metadata(&db, "session_catalog", "first"),
            metadata_before
        );
        assert_eq!(
            recovery_publication_snapshot(&db, "first"),
            publication_before
        );
        assert_eq!(recovery_stage_observation_grants(&db), stage_grants_before);
        assert_eq!(catalog_generation(&home), generation_before);
        assert!(db
            .query_row(
                "SELECT version=5 FROM main.session_slot_projections WHERE namespace=semon_namespace() AND session_key='first'",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap());
        assert!(db
            .query_row(
                "SELECT value='1' FROM main.meta WHERE namespace=semon_namespace() AND key='overview_needed:first'",
                [],
                |r| r.get::<_, bool>(0),
            )
            .unwrap());
    }
}

#[test]
fn recovery_ordinary_index_advance_supersedes_the_old_semantic_source() {
    let home = Home::new("recovery-direct-index-advance");
    let original_records = [
        human("first", ts(1, 0), "initial Index source"),
        assistant("first", ts(1, 1), vec![text("initial answer")]),
    ];
    home.top("first", &original_records);
    publish(&home);
    publish_semantic(&home);
    publish_semantic_stage(
        &home,
        &crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/-work-proj/first.jsonl".into(),
        },
    );
    let config = home.options.source.config(&home.options.cache).unwrap();
    let db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let before = recovery_catalog_row(&db, "session_catalog", "first");

    home.top(
        "first",
        &[
            original_records[0].clone(),
            original_records[1].clone(),
            human("first", ts(1, 2), "advanced Index source"),
            assistant("first", ts(1, 3), vec![text("advanced answer")]),
        ],
    );
    publish(&home);

    let after = recovery_catalog_row(&db, "session_catalog", "first");
    assert_ne!(
        crate::retention::source_generation(&before.sources).unwrap(),
        crate::retention::source_generation(&after.sources).unwrap(),
        "the ordinary producer advances the indexed source generation"
    );
    let direct_capture: Option<String> = db
        .query_row(
            "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(direct_capture.is_some());
    assert_eq!(recovery_selected_transcript(&home, "first").status, 200);
    assert!(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_proof_refs f JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key WHERE f.namespace=semon_namespace() AND f.session_key='first' AND f.kind='current' AND g.prefix=?1)",
        [crate::retention::source_generation(&before.sources).unwrap()],
        |row| row.get::<_, bool>(0),
    ).unwrap(), "the prior immutable Current ref survives the ordinary Index advance");
    let direct_metadata = recovery_catalog_metadata(&db, "session_catalog", "first");
    let old_refs = recovery_proof_snapshot(&db, "first");
    publish_semantic_stage(
        &home,
        &crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/-work-proj/first.jsonl".into(),
        },
    );
    assert_eq!(
        recovery_catalog_metadata(&db, "session_catalog", "first"),
        direct_metadata
    );
    assert_ne!(recovery_proof_snapshot(&db, "first"), old_refs);
    let direct_capture: Option<String> = db.query_row(
        "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key='first' AND owner_kind='catalog'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(direct_capture.is_none());
    assert_eq!(recovery_selected_transcript(&home, "first").status, 200);
}

#[test]
fn recovery_query_row_and_grant_roll_back_when_grant_update_fails() {
    let home = Home::new("recovery-grant-rollback");
    home.top("first", &[human("first", ts(1, 0), "atomic query repair")]);
    publish_semantic(&home);
    let config = home.options.source.config(&home.options.cache).unwrap();
    let db = crate::storage::sqlite::semantic::Stage::open(
        crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
    )
    .unwrap()
    .into_test_connection();
    let selected = recovery_catalog_row(&db, "session_catalog", "first");
    let metadata_before = recovery_catalog_metadata(&db, "session_catalog", "first");
    let grant_before = recovery_observation_grant(&db, "first", "catalog").unwrap();
    let proof_snapshot = recovery_proof_snapshot(&db, "first");
    let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
    let tx = db.unchecked_transaction().unwrap();
    let current_binding = binding.runtime_binding_at(&tx).unwrap();
    crate::storage::sqlite::proof_generations::observation_binding(
        &tx,
        &current_binding,
        &selected,
    )
    .unwrap();
    tx.rollback().unwrap();
    db.execute(
        "UPDATE main.session_slot_projections SET version=5 WHERE namespace=semon_namespace() AND session_key='first'",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO main.meta(namespace,key,value) VALUES(semon_namespace(),'overview_needed:first','1') ON CONFLICT(namespace,key) DO UPDATE SET value='1'",
        [],
    )
    .unwrap();
    db.execute_batch(
        "CREATE TRIGGER recovery_fail_catalog_grant_update BEFORE UPDATE ON storage_catalog_native_observation_grants WHEN OLD.session_key='first' AND OLD.owner_kind='catalog' BEGIN SELECT RAISE(ABORT,'recovery test grant update failure'); END;",
    )
    .unwrap();
    home.process(32, 779);
    home.pid_file(32, "first", "busy", 779);

    let error = crate::repair_session_overview(
        &home.options,
        "local",
        "first",
        None,
        crate::SessionSourceReadMode::LocalThenProvider,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(
        error.to_string(),
        crate::storage::StorageError::Unavailable.to_string(),
        "the row had valid authority and the grant update trigger caused the refusal"
    );
    assert_eq!(
        recovery_catalog_metadata(&db, "session_catalog", "first"),
        metadata_before,
        "the row update must roll back when its matching grant write fails"
    );
    assert_eq!(
        recovery_observation_grant(&db, "first", "catalog"),
        Some(grant_before)
    );
    assert_eq!(recovery_proof_snapshot(&db, "first"), proof_snapshot);
    assert_eq!(
        db.query_row(
            "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='overview_needed:first'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "1",
        "other repair publication effects must share the transaction"
    );
}

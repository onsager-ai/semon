//! Synthetic recovery diagnostics, not production latency or memory budgets.
//! Run alone with --test-threads=1 --nocapture for interpretable process counters.
//! Complete selected Index decoding may precede provider callbacks; these measurements
//! do not establish history-independent work or per-request peak memory. Separate
//! SelectedReadControl regressions qualify request-scoped limits and cancellation.

use super::*;
use std::{collections::BTreeMap, sync::mpsc, time::Instant};

fn publish(home: &Home) {
    crate::model::observe_catalog(
        &home.options,
        &mut crate::events::EventCache::open_scoped(&home.options.cache).unwrap(),
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
}

fn shared_home(prefix: usize) -> (Home, String) {
    let home = Home::new("recovery-shared");
    for sid in ["first", "second"] {
        let mut records = vec![human(sid, ts(1, 0), "shared owner prompt")];
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
        let (home, stub) = shared_home(prefix);
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
    let (home, key) = shared_home(8192);
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
            let reply = crate::session_transcript_range(
                &home.options,
                "local",
                &format!("sid=long&after={after}&limit=17{pin}"),
                None,
            );
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
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
    let (home, key) = shared_home(1024);
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
        let (home, key) = shared_home(128);
        if failure == "cpu" {
            // No leading user record: native marker lookup reaches the first
            // assistant without provider I/O. Cancel inside event ordering.
            for sid in ["first", "second"] {
                let path = home
                    .options
                    .claude_home
                    .join(format!("projects/-work-proj/{sid}.jsonl"));
                let text = fs::read_to_string(&path).unwrap();
                fs::write(path, text.split_once('\n').unwrap().1).unwrap();
            }
            publish(&home);
        }
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
        let (home, _) = shared_home(16);
        let db = rusqlite::Connection::open(crate::events::EventCache::path(&home.options.cache))
            .unwrap();
        if !ready {
            db.execute("UPDATE session_catalog SET metadata=json_remove(metadata,'$.query') WHERE session_key='first'", []).unwrap();
        }
        db.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('overview_needed:first','1')",
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
        let publication = || {
            let mut statement = db.prepare("SELECT 'overview',summary FROM session_overview WHERE session_key='first' UNION ALL SELECT 'header',metadata FROM session_trace_headers WHERE session_key='first' UNION ALL SELECT 'turn',metadata FROM session_trace_turns WHERE session_key='first' UNION ALL SELECT 'edge',metadata FROM session_trace_edges WHERE session_key='first' ORDER BY 1,2").unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let original_publication = publication();
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
        assert_eq!(publication(), original_publication);
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
fn recovery_selected_ledger_fence_precedes_complete_index_decode() {
    let (home, _) = shared_home(128);
    let db =
        rusqlite::Connection::open(crate::events::EventCache::path(&home.options.cache)).unwrap();
    let encoded: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='first'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row: crate::model::summary::CatalogRow = serde_json::from_str(&encoded).unwrap();
    db.execute(
        "UPDATE files SET head_sha256=zeroblob(32) WHERE path=?1",
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

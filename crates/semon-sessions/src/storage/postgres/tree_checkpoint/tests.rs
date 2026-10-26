use super::*;
use crate::{
    events::{EventCache, scan_file},
    storage::postgres::tests::{fixture, limits, ns},
};
use rusqlite::types::Value;
pub(crate) use rusqlite::types::Value as OracleValue;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

type OracleRows = Vec<(&'static str, Vec<Vec<(String, Value)>>)>;

fn cancellation_fixture() -> (Fixture, PgPool, Arc<super::super::tests::Factory>) {
    let data = parser_fixture("claude");
    let (pool, factory) = fixture(|b| {
        b.max_connections = 1;
        b.acquire_timeout = Duration::from_secs(6);
        b.statement_timeout = Duration::from_secs(6);
        b.lock_timeout = Duration::from_secs(6);
        b.txn_deadline = Duration::from_secs(8);
    });
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install(tx, &data),
    )
    .unwrap();
    (data, pool, factory)
}

fn lock_table(factory: &super::super::tests::Factory, table: &str) -> ::postgres::Client {
    let mut client = factory.client();
    client
        .batch_execute(&format!(
            "BEGIN; LOCK TABLE {table} IN ACCESS EXCLUSIVE MODE"
        ))
        .unwrap();
    client
}

fn wait_for_lock(client: &mut ::postgres::Client, table: &str) -> (i32, String) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // The lock holder is in a transaction: activity/query text otherwise
        // stays cached from an earlier poll while wait events remain live.
        // Capture the actual blocked command (including schema admission), so
        // the completion observer can require that same PID and exact query.
        client
            .batch_execute("SELECT pg_stat_clear_snapshot()")
            .unwrap();
        let blocked = client.query_opt("SELECT a.pid,a.query FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.relation=$1::text::regclass AND NOT l.granted AND a.state='active' AND a.wait_event_type='Lock' LIMIT 1", &[&table]).unwrap();
        if let Some(row) = blocked {
            return (row.get(0), row.get(1));
        }
        assert!(
            Instant::now() < deadline,
            "real checkpoint SQL never blocked"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn held_cancel_diagnostic(client: &mut ::postgres::Client, pid: i32, expected: &str, stage: &str) {
    let row = client.query_one("SELECT state,wait_event_type,wait_event,backend_xid IS NOT NULL,xact_start IS NOT NULL,left(query,80),length(query),query=$2,(SELECT count(*) FROM pg_locks WHERE pid=$1 AND granted),(SELECT count(*) FROM pg_locks WHERE pid=$1 AND NOT granted) FROM pg_stat_activity WHERE pid=$1", &[&pid, &expected]).unwrap();
    eprintln!(
        "HELD_CANCEL stage={stage} pid={pid} state={:?} wait={:?}/{:?} xid={} xact={} sql={:?} len={} same={} locks={}/{}",
        row.get::<_, Option<String>>(0),
        row.get::<_, Option<String>>(1),
        row.get::<_, Option<String>>(2),
        row.get::<_, bool>(3),
        row.get::<_, bool>(4),
        row.get::<_, String>(5),
        row.get::<_, i32>(6),
        row.get::<_, bool>(7),
        row.get::<_, i64>(8),
        row.get::<_, i64>(9),
    );
}

struct HeldCancelFactory {
    real: Arc<super::super::tests::Factory>,
    started: mpsc::SyncSender<()>,
    release: Arc<tokio::sync::Notify>,
    held: Arc<AtomicBool>,
}
impl super::super::PgConnectionFactory for HeldCancelFactory {
    fn connect(&self) -> super::super::ConnectFuture<'_> {
        Box::pin(async move {
            let mut connection = super::super::PgConnectionFactory::connect(&*self.real).await?;
            let cancel = connection.client.cancel.clone();
            let started = self.started.clone();
            let release = self.release.clone();
            let held = self.held.clone();
            connection.client.cancel = Arc::new(move || {
                let cancel = cancel.clone();
                let started = started.clone();
                let release = release.clone();
                let held = held.clone();
                Box::pin(async move {
                    // Hold exactly the observed transport. Later cancellation
                    // during schema admission must not refill a blocking test
                    // channel or wait for an already-consumed release permit.
                    if !held.swap(true, Ordering::AcqRel) {
                        eprintln!("HELD_CANCEL transport=first");
                        started.send(()).unwrap();
                        release.notified().await;
                    } else {
                        eprintln!("HELD_CANCEL transport=repeated");
                    }
                    cancel().await
                })
            });
            Ok(connection)
        })
    }
}

#[test]
fn pg_tree_current_request_cancels_blocked_summary_and_holds_worker_and_pool() {
    let (data, original_pool, real) = cancellation_fixture();
    drop(original_pool);
    let (started, cancelling) = mpsc::sync_channel(1);
    let release = Arc::new(tokio::sync::Notify::new());
    let pool = PgPool::open(
        Arc::new(HeldCancelFactory {
            real: real.clone(),
            started,
            release: release.clone(),
            held: Arc::new(AtomicBool::new(false)),
        }),
        super::super::PoolLimits {
            max_connections: 1,
            acquire_timeout: Duration::from_secs(6),
            statement_timeout: Duration::from_secs(6),
            lock_timeout: Duration::from_secs(6),
            txn_deadline: Duration::from_secs(8),
            cancel_timeout: Duration::from_secs(2),
            ..limits()
        },
    )
    .unwrap();
    let old_cancel = Arc::new(AtomicBool::new(false));
    let old = selected_read::SelectedReadControl::new(
        Instant::now() + Duration::from_secs(10),
        old_cancel.clone(),
    );
    let host = PgCancellation::default();
    let mut summary = {
        let _scope = selected_read::Scope::enter(&old);
        Snapshot::restore(
            pool.clone(),
            ns("tree\0🦀"),
            &data.path,
            &data.ledger,
            host.clone(),
        )
        .unwrap()
    };
    old_cancel.store(true, Ordering::Release);
    let mut lock = lock_table(&real, "semon_pg_native_tree_tools");
    let cancelled = Arc::new(AtomicBool::new(false));
    let current = selected_read::SelectedReadControl::new(
        Instant::now() + Duration::from_secs(10),
        cancelled.clone(),
    );
    let (done, finished) = mpsc::sync_channel(1);
    let begin = Instant::now();
    std::thread::scope(|threads| {
        let worker = threads.spawn(|| {
            let _outer = selected_read::Scope::enter(&current);
            // An expired/cancelled inner scope must not replace the live outer.
            let nested = selected_read::SelectedReadControl::new(
                Instant::now(),
                Arc::new(AtomicBool::new(true)),
            );
            let _inner = selected_read::Scope::enter(&nested);
            let result = summary.tool_name("tool\0🦀");
            done.send(()).unwrap();
            result
        });
        let (pid, blocked_query) = wait_for_lock(&mut lock, "semon_pg_native_tree_tools");
        held_cancel_diagnostic(&mut lock, pid, &blocked_query, "blocked");
        cancelled.store(true, Ordering::Release);
        cancelling.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            !host.is_cancelled(),
            "request cancellation must not cancel the host"
        );
        // Let SQL complete while the inline cancel transport remains pending.
        lock.batch_execute("ROLLBACK").unwrap();
        held_cancel_diagnostic(&mut lock, pid, &blocked_query, "unlocked");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if lock.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND state='idle in transaction' AND query=$2)", &[&pid, &blocked_query]).unwrap().get::<_, bool>(0) {
                break;
            }
            if Instant::now() >= deadline {
                held_cancel_diagnostic(&mut lock, pid, &blocked_query, "timeout");
            }
            assert!(
                Instant::now() < deadline,
                "query did not finish during held cancel"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(pool.accounted(), 1, "pending I/O must retain the pool slot");
        assert!(
            matches!(finished.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "worker returned before cancellation I/O stopped"
        );
        held_cancel_diagnostic(&mut lock, pid, &blocked_query, "owned-before-release");
        release.notify_one();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        eprintln!("HELD_CANCEL worker=interrupted-after-release");
    });
    assert!(begin.elapsed() < Duration::from_secs(4));
    assert_eq!(pool.accounted(), 0, "tainted connection must be retired");
    // Same deferred checkpoint, new request, fresh connection; neither prior
    // request cancellation may poison it or mutate the independent host token.
    assert_oracle(&mut summary, &data.oracle);
    assert_eq!(
        summary.tool_name("tool\0🦀").unwrap(),
        Some("Bash'\0🦀".into())
    );
    host.cancel();
    assert!(summary.tool_name("tool\0🦀").is_err());
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_request_deadline_interrupts_real_usage_and_capture_queries() {
    let (data, pool, factory) = cancellation_fixture();
    for table in [
        "semon_pg_native_tree_copied_identities",
        "semon_pg_native_tree_headers",
    ] {
        let mut summary = restore(&pool, &data);
        let mut lock = lock_table(&factory, table);
        let begin = Instant::now();
        std::thread::scope(|threads| {
            let worker = threads.spawn(|| {
                let control = selected_read::SelectedReadControl::new(
                    Instant::now() + Duration::from_millis(1500),
                    Arc::new(AtomicBool::new(false)),
                );
                let _scope = selected_read::Scope::enter(&control);
                if table.ends_with("headers") {
                    Snapshot::restore(
                        pool.clone(),
                        ns("tree\0🦀"),
                        &data.path,
                        &data.ledger,
                        PgCancellation::default(),
                    )
                    .map(|_| ())
                } else {
                    summary.load_shared_usage()
                }
            });
            wait_for_lock(&mut lock, table);
            assert_eq!(pool.accounted(), 1);
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
            assert!(
                begin.elapsed() < Duration::from_secs(4),
                "request must expire before 6s statement/lock/acquire limits"
            );
            assert_eq!(pool.accounted(), 0);
            lock.batch_execute("ROLLBACK").unwrap();
        });
        assert_oracle(&mut restore(&pool, &data), &data.oracle);
    }
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_request_deadline_bounds_acquire_and_precancel_keeps_capacity() {
    let (data, pool, _) = cancellation_fixture();
    let summary = restore(&pool, &data);
    let lease = pool.acquire(&PgCancellation::default()).unwrap();
    let control = selected_read::SelectedReadControl::new(
        Instant::now() + Duration::from_millis(100),
        Arc::new(AtomicBool::new(false)),
    );
    let begin = Instant::now();
    {
        let _scope = selected_read::Scope::enter(&control);
        assert_eq!(
            summary.tool_name("tool\0🦀").unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
    assert!(begin.elapsed() < Duration::from_secs(1));
    assert_eq!(pool.accounted(), 1);
    drop(lease);
    let control = selected_read::SelectedReadControl::new(
        Instant::now() + Duration::from_secs(1),
        Arc::new(AtomicBool::new(true)),
    );
    {
        let _scope = selected_read::Scope::enter(&control);
        assert_eq!(
            summary.tool_name("tool\0🦀").unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(
            Snapshot::restore(
                pool.clone(),
                ns("tree\0🦀"),
                &data.path,
                &data.ledger,
                PgCancellation::default()
            )
            .is_err()
        );
    }
    assert_eq!(pool.accounted(), 0);
    assert_eq!(
        summary.tool_name("tool\0🦀").unwrap(),
        Some("Bash'\0🦀".into())
    );
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_host_cancellation_interrupts_io_without_cancelling_request() {
    let (data, pool, factory) = cancellation_fixture();
    let host = PgCancellation::default();
    let summary = Snapshot::restore(
        pool.clone(),
        ns("tree\0🦀"),
        &data.path,
        &data.ledger,
        host.clone(),
    )
    .unwrap();
    let current = selected_read::SelectedReadControl::default();
    let mut lock = lock_table(&factory, "semon_pg_native_tree_tools");
    let begin = Instant::now();
    std::thread::scope(|threads| {
        let worker = threads.spawn(|| {
            let _scope = selected_read::Scope::enter(&current);
            summary.tool_name("tool\0🦀")
        });
        wait_for_lock(&mut lock, "semon_pg_native_tree_tools");
        host.cancel();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(begin.elapsed() < Duration::from_secs(3));
        assert!(current.check().is_ok());
        assert_eq!(pool.accounted(), 0);
        lock.batch_execute("ROLLBACK").unwrap();
    });
    assert_oracle(&mut restore(&pool, &data), &data.oracle);
    std::fs::remove_dir_all(data.root).unwrap();
}

pub(crate) struct Fixture {
    pub(crate) root: std::path::PathBuf,
    pub(crate) path: String,
    pub(crate) ledger: Ledger,
    pub(crate) oracle: Summary,
    pub(crate) sources: Vec<(String, Ledger, crate::events::FileIndex)>,
    pub(crate) rows: OracleRows,
}
fn read_ledger(sqlite: &rusqlite::Connection, path: &str) -> Ledger {
    sqlite.query_row("SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1", [path], |r| Ok(Ledger {stat: crate::events::Stat {dev:r.get::<_,i64>(0)? as u64,ino:r.get::<_,i64>(1)? as u64,size:r.get::<_,i64>(2)? as u64,modified_ns:u128::from_be_bytes(r.get::<_,Vec<u8>>(3)?.try_into().unwrap()),changed_ns:r.get::<_,Option<Vec<u8>>>(4)?.map(|b|i128::from_be_bytes(b.try_into().unwrap()))},offset:r.get::<_,i64>(5)? as u64,prefix:r.get::<_,Vec<u8>>(6)?.try_into().unwrap(),tail:r.get::<_,Vec<u8>>(7)?.try_into().unwrap()} )).unwrap()
}
pub(crate) fn parser_fixture(harness: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "pg100-{}-{}-{harness}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let cache_path = root.join("index.json");
    let path = root.join("one.jsonl");
    let body = match harness {
        "claude" => format!(
            "{}\n{}\n{}\n",
            serde_json::json!({"type":"user","uuid":"u\0🦀","cwd":"/w\0🦀","gitBranch":"b'🦀","timestamp":"2026-09-29T00:00:00Z","message":{"role":"user","content":"Raw fixture"}}),
            serde_json::json!({"type":"assistant","uuid":"record\0🦀","timestamp":"2026-09-29T00:00:01Z","message":{"id":"message\0🦀","role":"assistant","model":"claude-🦀","content":[{"type":"tool_use","id":"tool\0🦀","name":"Bash'\0🦀","input":{}}],"usage":{"input_tokens":3,"output_tokens":2}}}),
            serde_json::json!({"type":"assistant","uuid":"r2","timestamp":"2026-09-29T00:00:02Z","message":{"id":"m2","role":"assistant","model":"claude-🦀","content":[{"type":"tool_use","id":"open\0🦀","name":"Read","input":{}}]}})
        ),
        "codex" => format!(
            "{}\n{}\n",
            serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5","cwd":"/w🦀"}}),
            serde_json::json!({"type":"token_usage_record","timestamp":"2026-09-29T00:00:01Z","payload":{"thread_id":"t\0🦀","session_id":"s\0🦀","turn_id":"turn","response_id":"response","usage":{"input_tokens":11,"output_tokens":3,"total_tokens":14,"cached_input_tokens":1,"reasoning_output_tokens":0}}})
        ),
        "copilot" => format!(
            "{}\n",
            serde_json::json!({"type":"session.start","id":"start","timestamp":"2026-09-29T00:00:00Z","data":{"sessionId":"s🦀","context":{"cwd":"/w🦀"}}})
        ),
        _ => unreachable!(),
    };
    std::fs::write(&path, &body).unwrap();
    let mut cache = EventCache::open(&cache_path).unwrap();
    scan_file(&path, harness, &mut cache, &mut false).unwrap();
    // An independently parsed copy gives Claude copied-usage identities.
    if harness == "claude" {
        let peer = root.join("two.jsonl");
        std::fs::write(&peer, &body).unwrap();
        scan_file(&peer, harness, &mut cache, &mut false).unwrap();
    }
    cache.flush_cold_writes().unwrap();
    drop(cache);
    capture_fixture(root, harness)
}
fn capture_fixture(root: std::path::PathBuf, harness: &str) -> Fixture {
    let cache_path = root.join("index.json");
    let path = root.join("one.jsonl");
    let mut cache = EventCache::open(&cache_path).unwrap();
    let index = scan_file(&path, harness, &mut cache, &mut false).unwrap();
    cache.flush_cold_writes().unwrap();
    let mut oracle = index.tree_summary.clone().unwrap();
    oracle.load_shared_usage().unwrap();
    let sqlite = rusqlite::Connection::open(EventCache::path(&cache_path)).unwrap();
    let ledger = read_ledger(&sqlite, path.to_str().unwrap());
    // The compact scalar and SQLite's lazy restore omit closed IDs. Capture
    // their persisted dimension oracle before closing SQLite for the PG phase.
    oracle.closed_tools = sqlite
        .prepare("SELECT t.tool_id FROM native_tree_tools t JOIN native_tree_headers h USING(projection_id) WHERE h.source_path=?1 AND t.closed=1 ORDER BY t.tool_id")
        .unwrap()
        .query_map([path.to_str().unwrap()], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    // Verify the persisted scalar oracle before any PG connection exists.
    let stored: String = sqlite
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("native_tree_summary:{}", path.to_str().unwrap())],
            |r| r.get(0),
        )
        .unwrap();
    let stored: crate::events::tree_rows::TreeSummaryRecord =
        serde_json::from_str(&stored).unwrap();
    assert_eq!(
        serde_json::to_value(&stored.summary).unwrap(),
        serde_json::to_value(
            index
                .tree_summary
                .as_ref()
                .unwrap()
                .compact(index.native_usage_observations.is_some())
        )
        .unwrap()
    );
    assert!(stored.summary.line_count > 0);
    let mut rows: OracleRows = Vec::new();
    for family in [
        "files",
        "usage",
        "usage_identity",
        "usage_sources",
        "usage_current",
        "tree_generations",
        "tree_headers",
        "tree_models",
        "tree_labels",
        "tree_tools",
        "tree_usage_models",
    ] {
        let table = if family.starts_with("usage_") && family != "usage_identity"
            || family.starts_with("tree_")
        {
            format!("native_{family}")
        } else {
            family.into()
        };
        let mut stmt = sqlite.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let names = stmt
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let data = stmt
            .query_map([], |r| {
                names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| Ok((n.clone(), r.get(i)?)))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        rows.push((family, data));
    }
    let source_paths = sqlite
        // Retained UsageSource generations are not additional Current files.
        // Replaying them as duplicate paths violates Core's exact ledger CAS.
        .prepare("SELECT source_path FROM native_usage_current ORDER BY source_id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let sources = source_paths
        .into_iter()
        .map(|path| {
            let ledger = read_ledger(&sqlite, &path);
            let mut index = crate::events::FileIndex::default();
            index.tree_summary.get_or_insert_default();
            if harness == "codex" {
                index.native_usage_observations.get_or_insert_default();
            }
            crate::events::store::native_usage::parser_append_fixture(
                std::path::Path::new(&path),
                harness,
                &mut index,
                0,
            );
            (format!("{path}\0🦀"), ledger, index)
        })
        .collect();
    drop(sqlite);
    drop(cache);
    // PG's logical path is lossless bytea; the real fixture's ledger and
    // parser rows stay identical while its logical carrier includes NUL.
    for (_, family_rows) in &mut rows {
        for row in family_rows {
            for (name, value) in row.iter_mut() {
                if (name == "path" || name == "source_path")
                    && let Value::Text(path) = value
                {
                    path.push_str("\0🦀");
                }
            }
        }
    }
    Fixture {
        root,
        path: format!("{}\0🦀", path.to_str().unwrap()),
        ledger,
        oracle,
        sources,
        rows,
    }
}
pub(crate) fn append_fixture(data: &Fixture, harness: &str) -> Fixture {
    use std::io::Write;
    let body = match harness {
        "claude" => format!(
            "{}\n{}\n",
            serde_json::json!({"type":"user","uuid":"result🦀","timestamp":"2026-09-29T00:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool\0🦀","content":"done"}]}}),
            serde_json::json!({"type":"assistant","uuid":"new-record🦀","timestamp":"2026-09-29T00:00:04Z","message":{"id":"new-message🦀","role":"assistant","model":"claude-new🦀","content":[{"type":"tool_use","id":"new-tool\0\u{1f}🦀","name":"Write\n\t'🦀","input":{}}],"usage":{"input_tokens":8,"output_tokens":5}}})
        ),
        "codex" => format!(
            "{}\n{}\n",
            serde_json::json!({"type":"turn_context","payload":{"model":"gpt-new🦀","cwd":"/new🦀"}}),
            serde_json::json!({"type":"token_usage_record","timestamp":"2026-09-29T00:00:03Z","payload":{"thread_id":"t\0🦀","session_id":"s\0🦀","turn_id":"new-turn","response_id":"new-response","usage":{"input_tokens":13,"output_tokens":5,"total_tokens":18,"cached_input_tokens":2,"reasoning_output_tokens":0}}})
        ),
        _ => unreachable!(),
    };
    std::fs::OpenOptions::new()
        .append(true)
        .open(data.root.join("one.jsonl"))
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    capture_fixture(data.root.clone(), harness)
}
// Test-only physical import of the independently persisted parser oracle rows.
// No SQLite connections survive into the PG operation/read phase.
pub(crate) fn install(tx: &mut PgTxn<'_>, fixture: &Fixture) -> Result<(), StorageError> {
    publish_current(tx, fixture, None, false)
}
fn publish_current(
    tx: &mut PgTxn<'_>,
    fixture: &Fixture,
    previous: Option<&Fixture>,
    append: bool,
) -> Result<(), StorageError> {
    // Real production Core + Usage + Tree writers; SQLite is closed before PG.
    let prior = |path: &str| {
        previous.and_then(|f| {
            f.sources
                .iter()
                .find(|(p, _, _)| p == path)
                .map(|(_, ledger, _)| ledger)
        })
    };
    for (path, ledger, index) in &fixture.sources {
        super::super::native_core::write(tx, path, prior(path), ledger, index, None)?;
    }
    {
        let mut effects = super::super::native_usage::Usage::new(tx)?;
        for (path, ledger, index) in &fixture.sources {
            crate::events::store::native_usage::algorithm::begin(
                &mut effects,
                path,
                prior(path),
                ledger,
                append,
            )?;
            crate::events::store::native_usage::algorithm::write(
                &mut effects,
                path,
                ledger,
                index,
            )?;
        }
    }
    for (path, ledger, index) in &fixture.sources {
        let source: i64 = tx.driver.query_one("SELECT source_id FROM semon_pg_native_usage_current WHERE namespace=$1 AND source_path=$2 LIMIT 1", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?.get(0);
        let summary = index
            .tree_summary
            .as_ref()
            .ok_or(StorageError::Unavailable)?;
        if !super::super::tree_writer::publish(
            tx,
            path,
            ledger,
            prior(path),
            source,
            summary,
            index.native_usage_observations.is_some(),
        )? {
            return Err(StorageError::Unavailable);
        }
    }
    Ok(())
}
pub(crate) fn install_without_tree(
    tx: &mut PgTxn<'_>,
    fixture: &Fixture,
) -> Result<(), StorageError> {
    install_families(tx, fixture, false)
}
pub(crate) fn oracle_witness(data: &Fixture) -> String {
    let db = rusqlite::Connection::open(EventCache::path(&data.root.join("index.json"))).unwrap();
    let path = data.path.strip_suffix("\0🦀").unwrap();
    let encoded: String = db
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("native_tree_summary:{path}")],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str::<serde_json::Value>(&encoded).unwrap()["projection_witness"]
        .as_str()
        .unwrap()
        .into()
}
fn install_families(tx: &mut PgTxn<'_>, fixture: &Fixture, tree: bool) -> Result<(), StorageError> {
    {
        let mut effects = super::super::native_usage::Usage::new(tx)?;
        for (path, ledger, index) in &fixture.sources {
            crate::events::store::native_usage::algorithm::begin(
                &mut effects,
                path,
                None,
                ledger,
                false,
            )?;
            crate::events::store::native_usage::algorithm::write(
                &mut effects,
                path,
                ledger,
                index,
            )?;
        }
    }
    for (family, rows) in &fixture.rows {
        if matches!(*family, "usage_sources" | "usage_current") {
            continue;
        }
        if !tree && family.starts_with("tree_") {
            continue;
        }
        let table = format!("semon_pg_native_{family}");
        let columns = tx.driver.query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 AND is_generated='NEVER' AND column_name NOT IN ('namespace','namespace_digest') ORDER BY ordinal_position", &[&table]).map_err(sqlstate::error)?;
        for row in rows {
            let mut parameters: Vec<Option<Vec<u8>>> =
                vec![Some(tx.namespace.as_str().as_bytes().to_vec())];
            let mut names = vec!["namespace".to_string()];
            let mut binds = vec!["$1".to_string()];
            for col in &columns {
                let name: String = col.get(0);
                let kind: String = col.get(1);
                let value = &row
                    .iter()
                    .find(|(n, _)| *n == name)
                    .expect("same current physical column")
                    .1;
                let bytes = match value {
                    Value::Null => None,
                    Value::Blob(v) => Some(v.clone()),
                    Value::Text(v) => Some(v.as_bytes().to_vec()),
                    Value::Integer(v) => Some(
                        if kind == "numeric" {
                            (*v as u64).to_string()
                        } else {
                            v.to_string()
                        }
                        .into_bytes(),
                    ),
                    _ => panic!("unexpected fixture value"),
                };
                parameters.push(bytes);
                names.push(name);
                let at = parameters.len();
                binds.push(if kind == "bytea" {
                    format!("${at}")
                } else {
                    format!("convert_from(${at},'UTF8')::{kind}")
                });
            }
            let refs = parameters
                .iter()
                .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
                .collect::<Vec<_>>();
            tx.driver
                .execute(
                    &format!(
                        "INSERT INTO {table}({}) VALUES({})",
                        names.join(","),
                        binds.join(",")
                    ),
                    &refs,
                )
                .map_err(sqlstate::error)?;
        }
    }
    Ok(())
}
fn restore(pool: &PgPool, data: &Fixture) -> Summary {
    Snapshot::restore(
        pool.clone(),
        ns("tree\0🦀"),
        &data.path,
        &data.ledger,
        PgCancellation::default(),
    )
    .unwrap()
}
fn assert_oracle(actual: &mut Summary, expected: &Summary) {
    assert_eq!(actual.line_count, expected.line_count);
    assert_eq!(actual.malformed_lines, expected.malformed_lines);
    assert_eq!(
        serde_json::to_value(actual.compact(false)).unwrap(),
        serde_json::to_value(expected.compact(false)).unwrap()
    );
    assert_eq!(actual.first, expected.first);
    assert_eq!(actual.last, expected.last);
    assert_eq!(actual.cwd, expected.cwd);
    assert_eq!(actual.branch, expected.branch);
    assert_eq!(
        serde_json::to_value(&actual.codex_turn).unwrap(),
        serde_json::to_value(&expected.codex_turn).unwrap()
    );
    assert_eq!(actual.models, expected.models);
    assert_eq!(actual.tools, expected.tools);
    assert_eq!(actual.closed_tools, expected.closed_tools);
    actual.load_shared_usage().unwrap();
    assert_eq!(actual.usage_records, expected.usage_records);
    assert_eq!(
        serde_json::to_value(&actual.usage_by_id).unwrap(),
        serde_json::to_value(&expected.usage_by_id).unwrap()
    );
    assert_eq!(actual.usage_models, expected.usage_models);
}
#[test]
fn pg_tree_checkpoint_real_parsers_restart_existing_summary_consumers() {
    for harness in ["claude", "codex"] {
        let data = parser_fixture(harness);
        let (pool, factory) = fixture(|b| b.max_connections = 1);
        pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| install(tx, &data),
        )
        .unwrap();
        let mut summary = restore(&pool, &data);
        assert_oracle(&mut summary, &data.oracle);
        assert_eq!(summary.tool_name("missing").unwrap(), None);
        if harness == "claude" {
            assert_eq!(
                summary.tool_name("tool\0🦀").unwrap(),
                Some("Bash'\0🦀".into())
            );
            assert!(!summary.usage_records.is_empty());
        }
        drop(summary);
        drop(pool);
        let pool = PgPool::open(factory, limits()).unwrap();
        assert_oracle(&mut restore(&pool, &data), &data.oracle);
        assert!(
            Snapshot::restore(
                pool,
                ns("tree\0é"),
                &data.path,
                &data.ledger,
                PgCancellation::default()
            )
            .is_err()
        );
        std::fs::remove_dir_all(data.root).unwrap();
    }
}
#[test]
fn pg_tree_checkpoint_stale_authority_rebind_and_selected_limits() {
    let data = parser_fixture("claude");
    let (pool, _) = fixture(|b| b.max_connections = 1);
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install(tx, &data),
    )
    .unwrap();
    let summary = restore(&pool, &data);
    let checkpoint = summary.stored.unwrap();
    DECODED.with(|value| value.set(0));
    let control = crate::selected_read::SelectedReadControl::default().with_limits(10, 65);
    {
        let _scope = crate::selected_read::Scope::enter(&control);
        assert!(checkpoint.shared_usage().is_err());
    }
    assert_eq!(DECODED.with(|value| value.get()), 0);
    let mut rebound = checkpoint.clone();
    rebound.bind_configured(crate::storage::StoreConfig {
        backend: crate::storage::StoreBackend::Sqlite {
            database: data.root.join("must-not-open.sqlite"),
        },
        namespace: ns("tree\0🦀"),
        expected_stream_epoch: None,
    });
    assert!(rebound.tool_name("missing").is_err());
    assert!(!data.root.join("must-not-open.sqlite").exists());
    for mutation in [
        "UPDATE semon_pg_native_files SET resume_at=resume_at+1",
        "UPDATE semon_pg_native_tree_headers SET projection_id=projection_id+100",
        "UPDATE semon_pg_native_tree_generations SET complete=false",
        "UPDATE semon_pg_native_usage_sources SET parser_version=parser_version+1",
        "UPDATE semon_pg_native_usage_sources SET receipt='bad'::bytea",
        "DELETE FROM semon_pg_native_usage_current",
    ] {
        pool.transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                tx.driver
                    .batch_execute("SAVEPOINT changed")
                    .map_err(sqlstate::error)?;
                tx.driver.batch_execute(mutation).map_err(sqlstate::error)?;
                // Independent reader cannot see an uncommitted mutation. Test the
                // exact same authority predicate inside this transaction instead.
                if let TreeCheckpoint::Pg(snapshot) = &checkpoint {
                    assert!(snapshot.authority(tx, true).is_err());
                }
                tx.driver
                    .batch_execute("ROLLBACK TO SAVEPOINT changed")
                    .map_err(sqlstate::error)?;
                Ok(())
            },
        )
        .unwrap();
    }
    checkpoint.shared_usage().unwrap();
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            tx.driver
                .batch_execute("UPDATE semon_pg_native_files SET size=size+1")
                .map_err(sqlstate::error)
        },
    )
    .unwrap();
    assert!(checkpoint.tool_name("missing").is_err());
    assert!(checkpoint.shared_usage().is_err());
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_checkpoint_cancel_deadline_closed_nullable_and_unsigned_tokens() {
    let data = parser_fixture("claude");
    let (pool, factory) = fixture(|b| b.max_connections = 1);
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| install(tx, &data),
    )
    .unwrap();
    let mut summary = restore(&pool, &data);
    let checkpoint = summary.stored.clone().unwrap();
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |_| {
            assert_eq!(
                checkpoint.tool_name("missing").unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            Ok(())
        },
    )
    .unwrap();
    // Remove the oracle's carrier before PG callbacks; deferred PG consumers
    // cannot depend on the known SQLite store or create it as a fallback.
    let sqlite_path = EventCache::path(&data.root.join("index.json"));
    std::fs::rename(&sqlite_path, data.root.join("oracle-held")).unwrap();
    assert!(!sqlite_path.exists());
    pool.transaction(&ns("tree\0🦀"),TxnKind::Write,&PgCancellation::default(),|tx|{
        tx.driver.execute("UPDATE semon_pg_native_tree_tools SET closed=true WHERE tool_id=$1", &[&"tool\0🦀".as_bytes()]).map_err(sqlstate::error)?;
        tx.driver.execute("INSERT INTO semon_pg_native_tree_tools(namespace,projection_id,tool_id,name,closed) SELECT namespace,projection_id,$1,NULL,true FROM semon_pg_native_tree_headers WHERE source_path=$2", &[&"null\0🦀".as_bytes(),&data.path.as_bytes()]).map_err(sqlstate::error)?;
        tx.driver.batch_execute("UPDATE semon_pg_native_usage SET input=18446744073709551615,total=18446744073709551615").map_err(sqlstate::error)?;Ok(())
    }).unwrap();
    assert!(checkpoint.tool_name("tool\0🦀").is_err());
    assert!(checkpoint.tool_name("null\0🦀").is_err());
    assert!(summary.load_shared_usage().is_err());
    // Repair through the real publisher before admitting this generation again.
    pool.transaction(&ns("tree\0🦀"), TxnKind::Write, &PgCancellation::default(), |tx| {
        let source: i64 = tx.driver.query_one("SELECT source_id FROM semon_pg_native_usage_current WHERE namespace=$1 AND source_path=$2", &[&tx.namespace.as_str().as_bytes(), &data.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
        assert!(super::super::tree_writer::publish(tx, &data.path, &data.ledger, Some(&data.ledger), source, &data.oracle, false)?);
        Ok(())
    }).unwrap();
    let mut summary = restore(&pool, &data);
    assert_eq!(
        summary.tool_name("tool\0🦀").unwrap(),
        Some("Bash'\0🦀".into())
    );
    assert_eq!(summary.tool_name("null\0🦀").unwrap(), None);
    summary.load_shared_usage().unwrap();
    assert!(
        summary
            .usage_by_id
            .values()
            .any(|v| v.input == u64::MAX && v.total == u64::MAX)
    );
    let restored = restore(&pool, &data);
    assert!(!restored.tools.contains_key("tool\0🦀"));
    assert!(!sqlite_path.exists());
    let control = PgCancellation::default();
    control.cancel();
    assert!(
        Snapshot::restore(
            pool.clone(),
            ns("tree\0🦀"),
            &data.path,
            &data.ledger,
            control
        )
        .is_err()
    );
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let control = crate::selected_read::SelectedReadControl::new(
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        cancelled,
    );
    {
        let _scope = crate::selected_read::Scope::enter(&control);
        assert_eq!(
            checkpoint.shared_usage().unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
    }
    let control = crate::selected_read::SelectedReadControl::new(
        std::time::Instant::now(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    {
        let _scope = crate::selected_read::Scope::enter(&control);
        assert_eq!(
            checkpoint.shared_usage().unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
    }
    // Read admission still uses the existing manifest verifier on a warm pool.
    factory
        .client()
        .batch_execute("ALTER TABLE semon_pg_native_tree_tools NO FORCE ROW LEVEL SECURITY")
        .unwrap();
    assert!(checkpoint.tool_name("tool\0🦀").is_err());
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_checkpoint_real_copilot_has_no_tree_authority() {
    let root = std::env::temp_dir().join(format!(
        "pg100-copilot-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("copilot.jsonl");
    let line = serde_json::json!({"type":"session.start","data":{"sessionId":"s\0🦀"}});
    std::fs::write(&path, format!("{line}\n")).unwrap();
    let cache_path = root.join("index.json");
    let mut cache = EventCache::open(&cache_path).unwrap();
    let index = scan_file(&path, "copilot", &mut cache, &mut false).unwrap();
    assert!(
        index.tree_summary.is_none(),
        "Copilot uses its own native metadata; do not invent a Tree generation"
    );
    cache.flush_cold_writes().unwrap();
    drop(cache);
    let sqlite = rusqlite::Connection::open(EventCache::path(&cache_path)).unwrap();
    let ledger = read_ledger(&sqlite, path.to_str().unwrap());
    drop(sqlite);
    let (pool, _) = fixture(|_| {});
    assert!(
        Snapshot::restore(
            pool,
            ns("tree\0🦀"),
            path.to_str().unwrap(),
            &ledger,
            PgCancellation::default()
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pg_tree_scalar119_published_record_and_authority_refuse_without_payload() {
    let (data, pool, _) = cancellation_fixture();
    let summary = restore(&pool, &data);
    let TreeCheckpoint::Pg(snapshot) = summary.stored.as_ref().unwrap() else {
        panic!("PG checkpoint")
    };
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::Write,
        &PgCancellation::default(),
        |tx| {
            let key = format!("native_tree_summary:{}", data.path);
            let original: Vec<u8> = tx
                .driver
                .query_one(
                    "SELECT value FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2 LIMIT 1",
                    &[&tx.namespace.as_str().as_bytes(), &key.as_bytes()],
                )
                .map_err(sqlstate::error)?
                .get(0);
            let base: serde_json::Value = serde_json::from_slice(&original).unwrap();
            let mut invalid = vec![b"{".to_vec(), vec![b' '; 1048577]];
            for field in [
                "version",
                "parser_version",
                "ledger",
                "projection_witness",
                "interpretation_witness",
            ] {
                let mut record = base.clone();
                record[field] = if field.ends_with("version") {
                    serde_json::json!(0)
                } else {
                    serde_json::json!("wrong")
                };
                invalid.push(serde_json::to_vec(&record).unwrap());
                let mut record = base.clone();
                record.as_object_mut().unwrap().remove(field);
                invalid.push(serde_json::to_vec(&record).unwrap());
            }
            for bytes in invalid {
                tx.driver
                    .batch_execute("SAVEPOINT scalar119")
                    .map_err(sqlstate::error)?;
                tx.driver
                    .execute(
                        "UPDATE semon_pg_native_meta SET value=$3 WHERE namespace=$1 AND key=$2",
                        &[&tx.namespace.as_str().as_bytes(), &key.as_bytes(), &bytes],
                    )
                    .map_err(sqlstate::error)?;
                assert!(snapshot.validate(tx).is_err());
                // Initial capture must refuse too, without relying on a prior
                // scalar digest. These are the production capture predicates.
                DECODED.with(|n| n.set(0));
                assert!(
                    snapshot
                        .authority(tx, false)
                        .and_then(|(file, projection, _)| snapshot.scalar(tx, file, projection))
                        .is_err()
                );
                assert_eq!(DECODED.with(|n| n.get()), 0);
                tx.driver
                    .batch_execute("ROLLBACK TO SAVEPOINT scalar119")
                    .map_err(sqlstate::error)?;
            }
            // A newly published valid scalar can be captured, but cannot
            // silently replace a scalar already held by a deferred reader.
            let mut changed = base.clone();
            changed["summary"]["line_count"] = serde_json::json!(999);
            let changed = serde_json::to_vec(&changed).unwrap();
            tx.driver
                .execute(
                    "UPDATE semon_pg_native_meta SET value=$3 WHERE namespace=$1 AND key=$2",
                    &[&tx.namespace.as_str().as_bytes(), &key.as_bytes(), &changed],
                )
                .map_err(sqlstate::error)?;
            assert!(snapshot.validate(tx).is_err());
            tx.driver
                .execute(
                    "UPDATE semon_pg_native_meta SET value=$3 WHERE namespace=$1 AND key=$2",
                    &[
                        &tx.namespace.as_str().as_bytes(),
                        &key.as_bytes(),
                        &original,
                    ],
                )
                .map_err(sqlstate::error)?;
            for mutation in [
                "UPDATE semon_pg_native_files SET dev=dev+1",
                "UPDATE semon_pg_native_files SET ino=ino+1",
                "UPDATE semon_pg_native_files SET size=size+1",
                "UPDATE semon_pg_native_files SET resume_at=resume_at+1",
                "UPDATE semon_pg_native_files SET mtime_ns=decode(repeat('00',16),'hex')",
                "UPDATE semon_pg_native_files SET ctime_ns=NULL",
                "UPDATE semon_pg_native_files SET head_sha256=decode(repeat('00',32),'hex')",
                "UPDATE semon_pg_native_files SET tail_sha256=decode(repeat('00',32),'hex')",
                "UPDATE semon_pg_native_tree_headers SET ledger='wrong'::bytea",
                "UPDATE semon_pg_native_tree_generations SET version=0",
                "UPDATE semon_pg_native_tree_generations SET complete=false",
                "UPDATE semon_pg_native_tree_generations SET work_pending=true",
                "UPDATE semon_pg_native_usage_sources SET checkpoint=checkpoint+1",
                "UPDATE semon_pg_native_usage_sources SET parser_version=0",
                "UPDATE semon_pg_native_usage_sources SET receipt='wrong'::bytea",
                "DELETE FROM semon_pg_native_usage_current",
                "DELETE FROM semon_pg_native_tree_headers",
                "DELETE FROM semon_pg_native_meta",
                "UPDATE semon_pg_native_tree_labels SET value='drift'::bytea",
                "UPDATE semon_pg_native_tree_models SET model='drift'::bytea",
            ] {
                tx.driver
                    .batch_execute("SAVEPOINT scalar119")
                    .map_err(sqlstate::error)?;
                tx.driver.batch_execute(mutation).map_err(sqlstate::error)?;
                assert!(snapshot.validate(tx).is_err(), "{mutation}");
                assert!(
                    snapshot
                        .authority(tx, false)
                        .and_then(|(file, projection, _)| snapshot.scalar(tx, file, projection))
                        .is_err(),
                    "initial capture: {mutation}"
                );
                tx.driver
                    .batch_execute("ROLLBACK TO SAVEPOINT scalar119")
                    .map_err(sqlstate::error)?;
            }
            snapshot.validate(tx)?;
            Ok(())
        },
    )
    .unwrap();
    assert_oracle(&mut restore(&pool, &data), &data.oracle);
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_scalar119_committed_interpretation_drift_refuses_deferred_payload() {
    for dependency in ["pricing", "recipe", "fingerprint", "failure"] {
        let (data, pool, _) = cancellation_fixture();
        let mut summary = restore(&pool, &data);
        if dependency == "pricing" {
            super::super::PgStoreMetadata::open(pool.clone(), &PgCancellation::default())
                .unwrap()
                .set_pricing_target(
                    &[119; 32],
                    0,
                    Some("new-pricing"),
                    &PgCancellation::default(),
                )
                .unwrap();
        } else {
            pool.transaction(&ns("tree\0🦀"), TxnKind::Write, &PgCancellation::default(), |tx| {
                if dependency == "fingerprint" {
                    tx.driver.batch_execute("UPDATE semon_pg_native_files SET codex_pricing_fingerprint='new-fingerprint'::bytea").map_err(sqlstate::error)?;
                } else {
                    let key = if dependency == "recipe" { "catalog_semantic_recipe".into() } else { format!("native_tree_failure:{}", data.path) };
                    super::super::native_keys::put_meta(tx, key.as_bytes(), b"drift")?;
                }
                Ok(())
            }).unwrap();
        }
        assert!(summary.tool_name("tool\0🦀").is_err(), "{dependency}");
        assert!(summary.load_shared_usage().is_err(), "{dependency}");
        assert!(
            Snapshot::restore(
                pool,
                ns("tree\0🦀"),
                &data.path,
                &data.ledger,
                PgCancellation::default()
            )
            .is_err(),
            "{dependency}"
        );
        std::fs::remove_dir_all(data.root).unwrap();
    }
}

#[test]
fn pg_tree_scalar119_equal_ids_isolate_unsupported_record_without_rewriting() {
    let data = parser_fixture("claude");
    let sqlite_path = EventCache::path(&data.root.join("index.json"));
    std::fs::rename(&sqlite_path, data.root.join("oracle-held")).unwrap();
    let (pool, _) = fixture(|b| b.max_connections = 1);
    let mut identities = Vec::new();
    for namespace in [ns("tree\0🦀"), ns("tree\0é")] {
        let ids = pool
            .transaction(&namespace, TxnKind::Write, &PgCancellation::default(), |tx| {
                install(tx, &data)?;
                let row = tx.driver.query_one("SELECT file_id,projection_id FROM semon_pg_native_tree_headers WHERE namespace=$1 AND source_path=$2 LIMIT 1", &[&tx.namespace.as_str().as_bytes(), &data.path.as_bytes()]).map_err(sqlstate::error)?;
                Ok((row.get::<_, i64>(0), row.get::<_, i64>(1)))
            })
            .unwrap();
        identities.push(ids);
    }
    assert_eq!(identities[0], identities[1], "actual physical IDs collide");
    let mut first = restore(&pool, &data);
    let mut other = Snapshot::restore(
        pool.clone(),
        ns("tree\0é"),
        &data.path,
        &data.ledger,
        PgCancellation::default(),
    )
    .unwrap();
    assert_oracle(&mut other, &data.oracle);
    let key = format!("native_tree_summary:{}", data.path);
    let unsupported = pool
        .transaction(
            &ns("tree\0🦀"),
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                let encoded: Vec<u8> = tx.driver.query_one("SELECT value FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2 LIMIT 1", &[&tx.namespace.as_str().as_bytes(), &key.as_bytes()]).map_err(sqlstate::error)?.get(0);
                let mut record: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
                record["version"] = serde_json::json!(2);
                let unsupported = serde_json::to_vec(&record).unwrap();
                tx.driver.execute("UPDATE semon_pg_native_meta SET value=$3 WHERE namespace=$1 AND key=$2", &[&tx.namespace.as_str().as_bytes(), &key.as_bytes(), &unsupported]).map_err(sqlstate::error)?;
                Ok(unsupported)
            },
        )
        .unwrap();
    DECODED.with(|n| n.set(0));
    assert!(first.tool_name("tool\0🦀").is_err());
    assert!(first.load_shared_usage().is_err());
    assert!(
        Snapshot::restore(
            pool.clone(),
            ns("tree\0🦀"),
            &data.path,
            &data.ledger,
            PgCancellation::default()
        )
        .is_err()
    );
    assert_eq!(DECODED.with(|n| n.get()), 0);
    pool.transaction(
        &ns("tree\0🦀"),
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            let after: Vec<u8> = tx
                .driver
                .query_one(
                    "SELECT value FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2 LIMIT 1",
                    &[&tx.namespace.as_str().as_bytes(), &key.as_bytes()],
                )
                .map_err(sqlstate::error)?
                .get(0);
            assert_eq!(
                after, unsupported,
                "unsupported bytes must remain untouched"
            );
            Ok(())
        },
    )
    .unwrap();
    assert_oracle(&mut other, &data.oracle);
    assert_eq!(
        other.tool_name("tool\0🦀").unwrap(),
        Some("Bash'\0🦀".into())
    );
    assert!(
        !sqlite_path.exists(),
        "no hidden SQLite in either PG namespace"
    );
    std::fs::remove_dir_all(data.root).unwrap();
}

#[test]
fn pg_tree_scalar119_real_producers_append_replacement_restart_and_history_bounds() {
    for harness in ["claude", "codex"] {
        // Complete all independent SQLite production/oracle phases first.
        let initial = parser_fixture(harness);
        let appended = append_fixture(&initial, harness);
        std::fs::write(initial.root.join("one.jsonl"), "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"replacement\"}}\nnot-json\n").unwrap();
        let replacement = capture_fixture(initial.root.clone(), harness);
        let sqlite_path = EventCache::path(&initial.root.join("index.json"));
        std::fs::rename(&sqlite_path, initial.root.join("oracle-held")).unwrap();
        let (mut pool, factory) = fixture(|_| {});
        let mut previous = None;
        let mut old = None::<Summary>;
        let mut previous_ids = None;
        for (data, append) in [(&initial, false), (&appended, true), (&replacement, false)] {
            pool.transaction(
                &ns("tree\0🦀"),
                TxnKind::Write,
                &PgCancellation::default(),
                |tx| publish_current(tx, data, previous, append),
            )
            .unwrap();
            let ids = pool
                .transaction(
                    &ns("tree\0🦀"),
                    TxnKind::ReadSnapshot,
                    &PgCancellation::default(),
                    |tx| {
                        let row = tx.driver.query_one("SELECT g.source_id,h.projection_id FROM semon_pg_native_tree_headers h JOIN semon_pg_native_tree_generations g USING(namespace,projection_id) WHERE h.namespace=$1 AND h.source_path=$2 LIMIT 1", &[&tx.namespace.as_str().as_bytes(), &data.path.as_bytes()]).map_err(sqlstate::error)?;
                        Ok((row.get::<_, i64>(0), row.get::<_, i64>(1)))
                    },
                )
                .unwrap();
            if let Some(previous_ids) = previous_ids {
                if append {
                    assert_eq!(ids, previous_ids, "append retains the qualified generation");
                } else {
                    assert_ne!(ids, previous_ids, "replacement creates a new generation");
                }
            }
            previous_ids = Some(ids);
            if let Some(mut old) = old.take() {
                assert!(old.tool_name("missing").is_err());
                assert!(old.load_shared_usage().is_err());
            }
            let mut actual = restore(&pool, data);
            assert_oracle(&mut actual, &data.oracle);
            old = Some(actual);
            previous = Some(data);
            drop(pool);
            pool = PgPool::open(factory.clone(), limits()).unwrap();
            assert_oracle(&mut restore(&pool, data), &data.oracle);
            assert!(!sqlite_path.exists(), "PG phase must not reopen SQLite");
        }
        let mut baseline = None;
        for count in [0_i64, 256, 2048] {
            // Nonselected dimension noise is not an oracle or a published Current.
            pool.transaction(&ns("tree\0🦀"), TxnKind::Write, &PgCancellation::default(), |tx| {
                tx.driver.execute("INSERT INTO semon_pg_native_tree_models(namespace,projection_id,model) SELECT $1,100000+n,convert_to('unrelated','UTF8') FROM generate_series(1,$2::bigint) n ON CONFLICT DO NOTHING", &[&tx.namespace.as_str().as_bytes(), &count]).map_err(sqlstate::error)?;
                Ok(())
            }).unwrap();
            DECODED.with(|n| n.set(0));
            assert_oracle(&mut restore(&pool, &replacement), &replacement.oracle);
            let decoded = DECODED.with(|n| n.get());
            assert_eq!(decoded, *baseline.get_or_insert(decoded));
        }
        assert!(!sqlite_path.exists());
        std::fs::remove_dir_all(initial.root).unwrap();
    }
}

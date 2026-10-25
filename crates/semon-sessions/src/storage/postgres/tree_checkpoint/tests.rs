use super::*;
use crate::{
    events::{EventCache, scan_file},
    storage::postgres::tests::{fixture, limits, ns},
};
use rusqlite::types::Value;
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

fn wait_for_lock(client: &mut ::postgres::Client, table: &str) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let blocked = client.query_opt("SELECT a.pid FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE l.relation=$1::regclass AND NOT l.granted AND a.wait_event_type='Lock' LIMIT 1", &[&table]).unwrap();
        if let Some(row) = blocked {
            return row.get(0);
        }
        assert!(
            Instant::now() < deadline,
            "real checkpoint SQL never blocked"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct HeldCancelFactory {
    real: Arc<super::super::tests::Factory>,
    started: mpsc::SyncSender<()>,
    release: Arc<tokio::sync::Notify>,
}
impl super::super::PgConnectionFactory for HeldCancelFactory {
    fn connect(&self) -> super::super::ConnectFuture<'_> {
        Box::pin(async move {
            let mut connection = super::super::PgConnectionFactory::connect(&*self.real).await?;
            let cancel = connection.client.cancel.clone();
            let started = self.started.clone();
            let release = self.release.clone();
            connection.client.cancel = Arc::new(move || {
                let cancel = cancel.clone();
                let started = started.clone();
                let release = release.clone();
                Box::pin(async move {
                    started.send(()).unwrap();
                    release.notified().await;
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
        let pid = wait_for_lock(&mut lock, "semon_pg_native_tree_tools");
        cancelled.store(true, Ordering::Release);
        cancelling.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            !host.is_cancelled(),
            "request cancellation must not cancel the host"
        );
        // Let SQL complete while the inline cancel transport remains pending.
        lock.batch_execute("ROLLBACK").unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if lock.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND state='idle in transaction' AND query LIKE 'SELECT coalesce(octet_length(name)%')", &[&pid]).unwrap().get::<_, bool>(0) {
                break;
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
        release.notify_one();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
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

struct Fixture {
    root: std::path::PathBuf,
    path: String,
    ledger: Ledger,
    oracle: Summary,
    sources: Vec<(String, Ledger, crate::events::FileIndex)>,
    rows: OracleRows,
}
fn read_ledger(sqlite: &rusqlite::Connection, path: &str) -> Ledger {
    sqlite.query_row("SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1", [path], |r| Ok(Ledger {stat: crate::events::Stat {dev:r.get::<_,i64>(0)? as u64,ino:r.get::<_,i64>(1)? as u64,size:r.get::<_,i64>(2)? as u64,modified_ns:u128::from_be_bytes(r.get::<_,Vec<u8>>(3)?.try_into().unwrap()),changed_ns:r.get::<_,Option<Vec<u8>>>(4)?.map(|b|i128::from_be_bytes(b.try_into().unwrap()))},offset:r.get::<_,i64>(5)? as u64,prefix:r.get::<_,Vec<u8>>(6)?.try_into().unwrap(),tail:r.get::<_,Vec<u8>>(7)?.try_into().unwrap()} )).unwrap()
}
fn parser_fixture(harness: &str) -> Fixture {
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
    let mut cache = EventCache::open(&cache_path).unwrap();
    let index = scan_file(&path, harness, &mut cache, &mut false).unwrap();
    let mut oracle = index.tree_summary.clone().unwrap();
    oracle.load_shared_usage().unwrap();
    let sqlite = rusqlite::Connection::open(EventCache::path(&cache_path)).unwrap();
    let ledger = read_ledger(&sqlite, path.to_str().unwrap());
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
        .prepare("SELECT source_path FROM native_usage_sources ORDER BY source_id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let sources = source_paths
        .into_iter()
        .map(|path| {
            let ledger = read_ledger(&sqlite, &path);
            let index = crate::events::store::native_usage::parser_fixture(
                std::path::Path::new(&path),
                harness,
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
// Test-only physical import of the independently persisted parser oracle rows.
// No SQLite connections survive into the PG operation/read phase.
fn install(tx: &mut PgTxn<'_>, fixture: &Fixture) -> Result<(), StorageError> {
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
        let table = format!("semon_pg_native_{family}");
        let columns = tx.driver.query("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 AND column_name NOT IN ('namespace','namespace_digest') ORDER BY ordinal_position", &[&table]).map_err(sqlstate::error)?;
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
    assert_eq!(
        checkpoint.tool_name("tool\0🦀").unwrap(),
        Some("Bash'\0🦀".into())
    );
    assert_eq!(checkpoint.tool_name("null\0🦀").unwrap(), None);
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

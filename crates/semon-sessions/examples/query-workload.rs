//! Deterministic fixed-scope, multi-machine native Query read workload.
//! Run each size in a fresh process: cargo run --release --locked -p
//! semon-sessions --example query-workload -- 1000 4
use semon_sessions::{
    Options, Query, RefreshPool, SessionCatalogObserver, SessionSourceReadMode, SessionSources,
};
use serde_json::json;
use std::{
    fs, io,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn rss_kib() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

// RSS retains allocator pages after native preparation. Distinguish those pages
// from live allocations on the GNU libc workload host; this is not a product
// cache-size estimate and is unavailable on other allocators.
fn heap_bytes() -> Option<serde_json::Value> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: mallinfo2 takes no pointers and reads the allocator's counters.
        let heap = unsafe { libc::mallinfo2() };
        Some(json!({
            "allocated":heap.uordblks.saturating_add(heap.hblkhd),
            "reserved":heap.arena.saturating_add(heap.hblkhd)
        }))
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        None
    }
}

fn read_counters() -> Option<(u64, u64)> {
    let value = fs::read_to_string("/proc/self/io").ok()?;
    let field = |key: &str| {
        value
            .lines()
            .find_map(|line| line.strip_prefix(key)?.trim().parse::<u64>().ok())
    };
    Some((field("rchar:")?, field("syscr:")?))
}

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let history: usize = args
        .get(1)
        .map_or(Ok(100), |n| n.parse())
        .expect("history count");
    let machines: usize = args
        .get(2)
        .map_or(Ok(4), |n| n.parse())
        .expect("machine count");
    assert!(machines > 0);
    let root = std::env::temp_dir().join(format!("semon-query-workload-{}", std::process::id()));
    fs::create_dir(&root)?;
    let mut inputs = Vec::new();
    for machine in 0..machines {
        let home = root.join(machine.to_string());
        let claude_home = home.join("claude");
        let projects = claude_home.join("projects/project");
        fs::create_dir_all(&projects)?;
        let proc_root = home.join("proc");
        fs::create_dir(&proc_root)?;
        fs::write(proc_root.join("locks"), "")?;
        for session in 0..=history {
            let sid = if machine == 0 && session == 0 {
                "selected".to_owned()
            } else {
                format!("history-{machine}-{session}")
            };
            let lines: String = (0..if sid == "selected" { 120 } else { 2 }).map(|turn| {
                json!({"type":"user","sessionId":sid,"uuid":format!("{sid}-{turn}"),
                    "parentUuid":null,"cwd":"/synthetic/project", "timestamp":"2026-10-01T00:00:00Z",
                    "message":{"role":"user","content":format!("synthetic prompt {turn}")}}).to_string() + "\n"
            }).collect();
            fs::write(projects.join(format!("{sid}.jsonl")), lines)?;
        }
        inputs.push((
            machine.to_string(),
            Options {
                source: semon_sessions::storage::HostSource::load_or_create(&home.join("source.json"))?,
                claude_home,
                claude_json: home.join(".claude.json"),
                codex_home: home.join("codex"),
                copilot_home: home.join("copilot"),
                proc_root,
                cache: home.join("index.json"),
                all: true,
                since: Duration::from_secs(86400),
                session: None,
                facts: None,
                scan_window: false,
            },
        ));
    }
    let started = Instant::now();
    let pool = RefreshPool::new(2);
    let observers: Vec<_> = inputs
        .iter()
        .map(|(_, options)| SessionCatalogObserver::with_pool(options.clone(), pool.clone()))
        .collect();
    for observer in &observers {
        observer.demand()?;
    }
    let mut catalog_bytes = 0;
    for ((machine, options), observer) in inputs.iter().zip(&observers) {
        loop {
            let reply = semon_sessions::session_catalog_page(options, machine, "limit=60");
            let body: serde_json::Value = serde_json::from_slice(&reply.body)?;
            if reply.status == 200 && body["completeness"]["state"] == "complete" {
                catalog_bytes += reply.body.len();
                break;
            }
            assert!(reply.status == 200 || reply.status == 503, "{body}");
            observer.demand()?;
            thread::sleep(Duration::from_millis(20));
        }
    }
    let cold_ms = started.elapsed().as_secs_f64() * 1000.;
    // A committed Catalog is visible before its worker releases temporary
    // publication payloads. Qualify resident heap after the actual work drains;
    // retain cold publication latency as its separate observed measurement.
    let drain = Instant::now();
    while pool.running() != 0 {
        if drain.elapsed() > Duration::from_secs(30) {
            return Err(io::Error::other("native workload workers did not drain"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let cold_drain_ms = drain.elapsed().as_secs_f64() * 1000.;
    let cold_rss = rss_kib();
    let cold_heap = heap_bytes();
    let sources = Arc::new(SessionSources::from_catalog_observers(
        inputs
            .iter()
            .zip(&observers)
            .map(|((key, _), observer)| (key.clone(), observer))
            .collect(),
    ));
    let mut query = Query::from_sources(sources.clone());
    let mut samples = Vec::new();
    let mut bytes = 0;
    let reads_before = read_counters();
    eprintln!("WARM_BEGIN");
    for _ in 0..101 {
        let started = Instant::now();
        let reply = query
            .call_selected(
                "0",
                "selected",
                "read_transcript",
                &json!({"id":"selected", "before":120, "limit":60}),
                None,
                SessionSourceReadMode::LocalThenProvider,
            )
            .map_err(io::Error::other)?;
        assert_eq!(reply["entries"].as_array().unwrap().len(), 60);
        assert_eq!((&reply["from"], &reply["to"]), (&json!(60), &json!(120)));
        let encoded = serde_json::to_vec(&reply)?;
        samples.push(started.elapsed().as_secs_f64() * 1000.);
        if bytes != 0 {
            assert_eq!(bytes, encoded.len());
        }
        bytes = encoded.len();
    }
    eprintln!("WARM_END");
    let reads = reads_before
        .zip(read_counters())
        .map(|(before, after)| json!({"rchar":after.0-before.0,"syscr":after.1-before.1}));
    let first_tx_ms = samples[0];
    samples.remove(0);
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"history_per_machine":history,"machines":machines,
        "cold_catalog_complete_ms":cold_ms,"cold_worker_drain_ms":cold_drain_ms,"catalog_page_bytes":catalog_bytes,"cold_rss_kib":cold_rss,"cold_heap_bytes":cold_heap,
        "first_tx_ms":first_tx_ms,"warm_tx_p50_ms":samples[50],"warm_tx_p95_ms":samples[95],
        "tx_bytes":bytes,"warm_rss_kib":rss_kib(),"warm_heap_bytes":heap_bytes(),"selected_records":120,"warm_samples":100,
        "warm_read_counters":reads,"active_workers":pool.running(),"queued_checks":pool.queued()})
    );
    sources.close();
    drop(query);
    drop(sources);
    drop(observers);
    fs::remove_dir_all(root)?;
    Ok(())
}

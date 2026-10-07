//! Deterministic fixed-scope, multi-machine Viewer read workload (#29/#53/#319).
//! Run each size in a fresh process: cargo run --release --locked -p
//! semon-sessions --example query-workload -- 1000 4
use semon_sessions::{Options, Refresh, ViewerCore};
use serde_json::json;
use std::{
    fs, io,
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
    let mut core = ViewerCore::with_machines(inputs);
    core.set_refresh(Refresh::OnInvalidate);
    let started = Instant::now();
    let cold = core.respond("GET", "/api/model", "", None);
    assert_eq!(cold.status, 200);
    let cold_ms = started.elapsed().as_secs_f64() * 1000.;
    let cold_rss = rss_kib();
    let mut samples = Vec::new();
    let mut bytes = 0;
    let reads_before = read_counters();
    eprintln!("WARM_BEGIN");
    for _ in 0..101 {
        let started = Instant::now();
        let reply = core.respond("GET", "/api/tx", "sid=selected&before=120", None);
        assert_eq!(reply.status, 200);
        samples.push(started.elapsed().as_secs_f64() * 1000.);
        if bytes != 0 {
            assert_eq!(bytes, reply.body.len());
        }
        bytes = reply.body.len();
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
        "cold_model_ms":cold_ms,"cold_model_bytes":cold.body.len(),"cold_rss_kib":cold_rss,
        "first_tx_ms":first_tx_ms,"warm_tx_p50_ms":samples[50],"warm_tx_p95_ms":samples[95],
        "tx_bytes":bytes,"warm_rss_kib":rss_kib(),"selected_records":120,"warm_samples":100,
        "warm_read_counters":reads})
    );
    core.close();
    drop(core);
    fs::remove_dir_all(root)?;
    Ok(())
}

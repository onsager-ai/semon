//! Seed deterministic history, then measure full-model and focused first-list
//! reads in fresh child processes so seed allocations cannot conceal cold RSS.
//! cargo run --release --locked -p semon-sessions --example catalog-workload -- 1000 4
use semon_sessions::{Options, ViewerCore};
use serde_json::json;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn options(root: &Path, machine: usize) -> Options {
    let home = root.join(machine.to_string());
    Options {
        claude_home: home.join("claude"),
        claude_json: home.join(".claude.json"),
        codex_home: home.join("codex"),
        copilot_home: home.join("copilot"),
        proc_root: home.join("proc"),
        cache: home.join("index.json"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    }
}

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

fn first_read(root: &Path, machines: usize, mode: &str) {
    let before = read_counters();
    let started = Instant::now();
    let core = ViewerCore::with_machines(
        (0..machines)
            .map(|machine| (machine.to_string(), options(root, machine)))
            .collect(),
    );
    let (path, query) = if mode == "catalog" {
        ("/api/sessions", "machine=0&limit=60")
    } else {
        ("/api/model", "")
    };
    let reply = core.respond("GET", path, query, None);
    let cold_ms = started.elapsed().as_secs_f64() * 1000.;
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let reads = before
        .zip(read_counters())
        .map(|(before, after)| json!({"rchar":after.0-before.0,"syscr":after.1-before.1}));
    let cold_rss = rss_kib();
    let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    if mode == "catalog" {
        assert_eq!(body["items"].as_array().unwrap().len(), 60);
        assert_eq!(body["items"][0]["key"], "history-0-000000");
        assert_eq!(body["items"][59]["key"], "history-0-000059");
    }
    let mut samples = Vec::new();
    let warm_samples = if mode == "catalog" { 20 } else { 1 };
    for _ in 0..warm_samples {
        let started = Instant::now();
        let warm = core.respond("GET", path, query, None);
        samples.push(started.elapsed().as_secs_f64() * 1000.);
        assert_eq!(warm.status, 200);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"mode":mode,"machines":machines,"cold_ms":cold_ms,"response_bytes":reply.body.len(),"cold_rss_kib":cold_rss,"cold_read_counters":reads,"warm_p50_ms":samples[warm_samples/2],"warm_p95_ms":samples[warm_samples-1],"warm_rss_kib":rss_kib(),"warm_samples":warm_samples,"scope":"first 60 rows on machine 0"})
    );
    core.close();
}

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|value| value == "--read") {
        first_read(
            Path::new(&args[2]),
            args[3].parse().expect("machine count"),
            &args[4],
        );
        return Ok(());
    }
    let history: usize = args
        .get(1)
        .map_or(Ok(100), |value| value.parse())
        .expect("history count");
    let machines: usize = args
        .get(2)
        .map_or(Ok(4), |value| value.parse())
        .expect("machine count");
    assert!(history >= 60 && machines > 0);
    let root: PathBuf =
        std::env::temp_dir().join(format!("semon-catalog-workload-{}", std::process::id()));
    fs::create_dir(&root)?;
    for machine in 0..machines {
        let options = options(&root, machine);
        let projects = options.claude_home.join("projects/project");
        fs::create_dir_all(&projects)?;
        fs::create_dir_all(&options.proc_root)?;
        fs::write(options.proc_root.join("locks"), "")?;
        for index in 0..history {
            let id = format!("history-{machine}-{index:06}");
            let records:String=(0..2).map(|turn|json!({"type":"user","sessionId":id,"uuid":format!("{id}-{turn}"),"parentUuid":null,"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":format!("synthetic prompt {turn}")}}).to_string()+"\n").collect();
            fs::write(projects.join(format!("{id}.jsonl")), records)?;
        }
    }
    let core = ViewerCore::with_machines(
        (0..machines)
            .map(|machine| (machine.to_string(), options(&root, machine)))
            .collect(),
    );
    core.warm()?;
    core.close();
    drop(core);
    for mode in ["model", "catalog"] {
        let status = Command::new(std::env::current_exe()?)
            .arg("--read")
            .arg(&root)
            .arg(machines.to_string())
            .arg(mode)
            .status()?;
        assert!(status.success(), "{mode} child failed");
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

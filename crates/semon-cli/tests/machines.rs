//! `semon sessions --machines DIR`: this machine's homes and the machines a
//! receiver wrote under `DIR/machines/`, through the `semon` binary. DIR is
//! only read; the window rules are the local ones.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// `ago` before now, as an RFC 3339 UTC time.
fn ts(ago: Duration) -> String {
    let seconds = (SystemTime::now() - ago)
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

const MINUTE: Duration = Duration::from_secs(60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

struct Root(PathBuf);

impl Root {
    /// This machine, `homebox`, with the Claude session `lane-home`; under
    /// `received/machines/`, `alpha` (facts, the Claude session
    /// `lane-alpha`) and `bravo` (no facts, the Codex run `lane-bravo`).
    /// The sessions ran minutes ago; bravo's run, `bravo_age` ago.
    fn new(bravo_age: Duration) -> Self {
        let root = Self(std::env::temp_dir().join(format!(
            "semon-machines-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        root.write("proc/locks", "");
        root.write("proc/sys/kernel/hostname", "homebox\n");
        root.claude("claude", "lane-home");
        root.claude("received/machines/alpha/claude", "lane-alpha");
        root.write(
            "received/machines/alpha/facts.json",
            &json!({"version": 2, "hostname": "alpha", "home": null, "proc_starts": {},
                "codex_locks": {}, "repos": {}})
            .to_string(),
        );
        let codex = |time: String, kind: &str, payload: Value| {
            json!({"timestamp": time, "type": kind, "payload": payload}).to_string() + "\n"
        };
        root.write(
            "received/machines/bravo/codex/sessions/2026/09/24/rollout-lane-bravo.jsonl",
            &[
                codex(
                    ts(bravo_age + 3 * MINUTE),
                    "session_meta",
                    json!({"id": "lane-bravo", "cwd": "/work/proj", "originator": "codex_exec", "thread_source": "user"}),
                ),
                codex(
                    ts(bravo_age + 2 * MINUTE),
                    "response_item",
                    json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Fix the lexer"}]}),
                ),
                codex(
                    ts(bravo_age + MINUTE),
                    "response_item",
                    json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Fixed on bravo"}]}),
                ),
            ]
            .concat(),
        );
        root
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn claude(&self, home: &str, sid: &str) -> PathBuf {
        let lines = [
            json!({"type": "user", "timestamp": ts(3 * MINUTE), "sessionId": sid,
                "cwd": "/work/proj", "origin": {"kind": "human"},
                "message": {"role": "user", "content": format!("Start {sid}")}}),
            json!({"type": "assistant", "timestamp": ts(2 * MINUTE), "sessionId": sid,
                "cwd": "/work/proj",
                "message": {"role": "assistant", "content": [{"type": "text", "text": format!("Done {sid}")}]}}),
        ];
        self.write(
            &format!("{home}/projects/-work-proj/{sid}.jsonl"),
            &lines.map(|line| format!("{line}\n")).concat(),
        )
    }

    fn dir(&self) -> String {
        self.0.join("received").to_string_lossy().into_owned()
    }

    fn semon(&self, args: &[&str]) -> Output {
        let home = |path: &str| self.0.join(path).to_string_lossy().into_owned();
        Command::new(env!("CARGO_BIN_EXE_semon"))
            .args(args)
            .args(["--claude-home", &home("claude")])
            .args(["--codex-home", &home("codex")])
            .args(["--proc-root", &home("proc")])
            .args(["--cache", &home("state/index.json")])
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.semon(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Everything under DIR, links not followed.
    fn received(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    out.insert(path.clone(), b"/".to_vec());
                    walk(&path, out);
                } else {
                    out.insert(path.clone(), fs::read(&path).unwrap_or_default());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.0.join("received"), &mut out);
        out
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn machine_ids(model: &Value) -> Vec<&str> {
    model["machines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|machine| machine["id"].as_str().unwrap())
        .collect()
}

/// Each tree root's id, with its machine.
fn roots(tree: &Value) -> BTreeMap<String, String> {
    tree["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| {
            (
                root["id"].as_str().unwrap().to_owned(),
                root["machine"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn machines_shows_this_machine_and_every_received_one_and_only_reads_dir() {
    let root = Root::new(MINUTE);
    let dir = root.dir();
    let dir = dir.as_str();
    let before = root.received();

    let output = root.semon(&["sessions", "--model-json", "--all", "--machines", dir]);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("received machine bravo") && stderr.contains("offline"),
        "{stderr}"
    );
    let model: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(machine_ids(&model), ["homebox", "alpha", "bravo"]);
    assert_eq!(model["machines"][1]["up"], true);
    assert_eq!(model["machines"][2]["up"], false);
    for (sid, machine) in [
        ("lane-home", "homebox"),
        ("lane-alpha", "alpha"),
        ("lane-bravo", "bravo"),
    ] {
        assert_eq!(model["sessions"][sid]["machine"], machine, "{sid}");
    }

    let tree = root.json(&["sessions", "--json", "--all", "--machines", dir]);
    assert_eq!(tree["schema_version"], 1);
    let expected: BTreeMap<String, String> = [
        ("lane-home", "homebox"),
        ("lane-alpha", "alpha"),
        ("lane-bravo", "bravo"),
    ]
    .into_iter()
    .map(|(sid, machine)| (sid.to_owned(), machine.to_owned()))
    .collect();
    assert_eq!(roots(&tree), expected);

    // --no-local leaves this machine out.
    let model = root.json(&[
        "sessions",
        "--model-json",
        "--all",
        "--machines",
        dir,
        "--no-local",
    ]);
    assert_eq!(machine_ids(&model), ["alpha", "bravo"]);
    assert!(model["sessions"].get("lane-home").is_none());

    // DIR is only read: the reader's own files go beside the cache.
    assert_eq!(root.received(), before);
    assert!(root.0.join("state/received").is_dir());
}

#[test]
fn received_machines_keep_the_local_window_rules() {
    // bravo's run was last written to three days ago.
    let root = Root::new(3 * DAY);
    let dir = root.dir();
    let dir = dir.as_str();
    let old = SystemTime::now() - 3 * DAY;
    fs::File::options()
        .append(true)
        .open(
            root.0
                .join("received/machines/bravo/codex/sessions/2026/09/24/rollout-lane-bravo.jsonl"),
        )
        .unwrap()
        .set_modified(old)
        .unwrap();
    let recent = roots(&root.json(&["sessions", "--json", "--since", "1d", "--machines", dir]));
    assert!(recent.contains_key("lane-home") && recent.contains_key("lane-alpha"));
    assert!(!recent.contains_key("lane-bravo"), "{recent:?}");
    let all = roots(&root.json(&["sessions", "--json", "--all", "--machines", dir]));
    assert_eq!(all.get("lane-bravo").map(String::as_str), Some("bravo"));
}

#[test]
fn machines_flags_are_checked() {
    let root = Root::new(MINUTE);
    let dir = root.dir();
    let dir = dir.as_str();
    for (args, message) in [
        (
            vec!["sessions", "--machines", dir],
            "--machines works with --serve, --model-json or --json",
        ),
        (
            vec!["sessions", "--json", "--watch", "--machines", dir],
            "--machines works with --serve, --model-json or --json",
        ),
        (
            vec!["sessions", "--model-json", "--no-local"],
            "--no-local requires --machines",
        ),
        (
            vec![
                "sessions",
                "--model-json",
                "--machine",
                dir,
                "--machines",
                dir,
            ],
            "--machine and --machines are exclusive",
        ),
    ] {
        let output = root.semon(&args);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // Nothing to show at all is an error, not an empty model.
    let empty = root.0.join("empty").to_string_lossy().into_owned();
    let output = root.semon(&[
        "sessions",
        "--model-json",
        "--machines",
        empty.as_str(),
        "--no-local",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no machines"));
}

//! Prints native Query results of a fixture home written by `tests/ui/fixture.mjs`, at
//! the fixture's pinned `now`: a way to check the fixture without a browser.
//!
//! `SEMON_SAMPLE_HOME=… SEMON_SAMPLE_NOW=… cargo test -p semon-sessions
//! --test sample_query -- --ignored --nocapture`

use std::{env, path::PathBuf, time::Duration};

use semon_sessions::{Options, Query};

#[test]
#[ignore = "reads SEMON_SAMPLE_HOME, written by tests/ui/fixture.mjs"]
fn print_sample_query() {
    let root = PathBuf::from(env::var("SEMON_SAMPLE_HOME").expect("SEMON_SAMPLE_HOME"));
    let now = env::var("SEMON_SAMPLE_NOW")
        .expect("SEMON_SAMPLE_NOW")
        .parse()
        .expect("epoch ms");
    let options = Options {
        claude_home: root.join("claude"),
        claude_json: root.join(".claude.json"),
        codex_home: root.join("codex"),
        copilot_home: root.join("copilot"),
        proc_root: root.join("proc"),
        cache: root.join("index.json"),
        all: false,
        since: Duration::from_secs(24 * 60 * 60),
        session: None,
        facts: None,
        scan_window: false,
    };
    let mut query = Query::new(options);
    let listed = query
        .call_at("list_sessions", &serde_json::json!({"limit":1000}), now)
        .expect("native session list");
    assert_eq!(
        listed["truncated"], false,
        "fixture exceeds the inspection bound"
    );
    let selected: Vec<_> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| {
            query
                .call_at("get_session", &serde_json::json!({"id":session["id"]}), now)
                .expect("native session details")
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({"list_sessions":listed,"get_session":selected})
    );
}

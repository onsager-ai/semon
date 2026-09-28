//! Prints the model of a fixture home written by `tests/ui/fixture.mjs`, at
//! the fixture's pinned `now`: a way to check the fixture without a browser.
//!
//! `SEMON_SAMPLE_HOME=… SEMON_SAMPLE_NOW=… cargo test -p semon-sessions
//! --test sample_model -- --ignored --nocapture`

use std::{env, path::PathBuf, time::Duration};

use semon_sessions::{Options, model_json_at};

#[test]
#[ignore = "reads SEMON_SAMPLE_HOME, written by tests/ui/fixture.mjs"]
fn print_sample_model() {
    let root = PathBuf::from(env::var("SEMON_SAMPLE_HOME").expect("SEMON_SAMPLE_HOME"));
    let now = env::var("SEMON_SAMPLE_NOW")
        .expect("SEMON_SAMPLE_NOW")
        .parse()
        .expect("epoch ms");
    let options = Options {
        claude_home: root.join("claude"),
        claude_json: root.join(".claude.json"),
        codex_home: root.join("codex"),
        proc_root: root.join("proc"),
        cache: root.join("index.json"),
        all: false,
        since: Duration::from_secs(24 * 60 * 60),
        session: None,
        facts: None,
        scan_window: false,
    };
    println!("{}", model_json_at(&options, now).expect("model"));
}

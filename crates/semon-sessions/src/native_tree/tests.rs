use super::*;
use crate::{Options, collect};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Home {
    root: PathBuf,
    options: Options,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-native-tree-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let options = Options {
            claude_home: root.join("claude"),
            claude_json: root.join("settings.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            all: true,
            since: Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
        };
        fs::create_dir_all(&options.proc_root).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        Self { root, options }
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn cache(&self) -> events::EventCache {
        events::EventCache::open_scoped(&self.options.cache).unwrap()
    }

    fn observe(
        &self,
        path: &Path,
        cache: &mut events::EventCache,
        evidence: &mut Evidence,
    ) -> Summary {
        read(
            &self.options,
            path,
            "claude",
            "parent",
            cache,
            &mut false,
            evidence,
        )
        .unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn line(value: Value) -> String {
    format!("{value}\n")
}

#[test]
fn warm_tree_does_not_decode_event_rows_or_fill_the_producer_cache_as_history_grows() {
    for count in [4, 4096] {
        let home = Home::new();
        let text = line(
            json!({"type":"assistant","message":{"model":"native","content":[{"type":"tool_use","id":"open","name":"Read"}]}}),
        ) + &line(json!({"type":"user","message":{"content":"private native text"}}))
            .repeat(count);
        let path = home.write("claude/projects/repo/parent.jsonl", &text);
        let expected = serde_json::to_value(collect(&home.options).unwrap()).unwrap();
        let db = rusqlite::Connection::open(events::EventCache::path(&home.options.cache)).unwrap();
        let rows: i64 = db
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, (count + 1) as i64);
        // A tree must not decode transcript rows, even an unsupported row tag.
        db.execute("UPDATE events SET k='unsupported' WHERE seq=0", [])
            .unwrap();
        let reads = events::FILE_READS.with(std::cell::Cell::get);
        let parsed = events::PARSED.with(std::cell::Cell::get);
        for _ in 0..3 {
            let mut cache = home.cache();
            assert_eq!(
                serde_json::to_value(crate::collect_with_index(&home.options, &mut cache).unwrap())
                    .unwrap(),
                expected
            );
            assert_eq!(
                cache.paths().count(),
                0,
                "metadata cannot become a partial producer index"
            );
            assert_eq!(events::FILE_READS.with(std::cell::Cell::get), reads);
            assert_eq!(events::PARSED.with(std::cell::Cell::get), parsed);
        }
        assert_eq!(fs::read(path).unwrap(), text.as_bytes());
    }
}

#[test]
fn tree_append_loads_only_the_changed_source_and_preserves_incremental_event_rows() {
    let home = Home::new();
    let path = home.write("claude/projects/repo/parent.jsonl", &line(json!({"type":"assistant","message":{"id":"one","model":"native","usage":{"output_tokens":2}}})));
    home.write(
        "claude/projects/repo/other.jsonl",
        &line(json!({"type":"user","message":{"content":"other"}})),
    );
    collect(&home.options).unwrap();
    let mut cache = home.cache();
    crate::collect_with_index(&home.options, &mut cache).unwrap();
    assert_eq!(cache.paths().count(), 0);
    let reads = events::FILE_READS.with(std::cell::Cell::get);
    let parsed = events::PARSED.with(std::cell::Cell::get);
    fs::OpenOptions::new().append(true).open(&path).unwrap()
        .write_all(line(json!({"type":"assistant","message":{"id":"two","usage":{"output_tokens":3},"content":[{"type":"tool_use","id":"open","name":"Read"}]}})).as_bytes()).unwrap();
    let nodes = crate::collect_with_index(&home.options, &mut cache).unwrap();
    let parent = nodes.iter().find(|node| node.id == "parent").unwrap();
    assert_eq!(parent.tokens.output, 5);
    assert_eq!(parent.models, ["native"]);
    assert_eq!(parent.open_tools[0].id, "open");
    assert_eq!(events::FILE_READS.with(std::cell::Cell::get), reads + 1);
    assert_eq!(events::PARSED.with(std::cell::Cell::get), parsed + 1);
    assert_eq!(cache.paths().collect::<Vec<_>>(), [path.to_str().unwrap()]);
    assert_eq!(cache.file_index(&path).unwrap().events.len(), 1);
}

#[test]
fn warm_tree_metadata_keeps_eof_marker_generation_fences() {
    let _clock = events::fixed_change_time_for_test(0);
    for eof in [false, true] {
        let home = Home::new();
        let value =
            json!({"type":"user","message":{"content":"Semon-Parent: claude:old"}}).to_string();
        let text = if eof { value } else { value + "\n" };
        let path = home.write("claude/projects/repo/parent.jsonl", &text);
        home.observe(&path, &mut home.cache(), &mut Evidence::default());
        let mut cache = home.cache();
        let mut evidence = Evidence::default();
        let reads = events::FILE_READS.with(std::cell::Cell::get);
        assert_eq!(
            home.observe(&path, &mut cache, &mut evidence)
                .marker
                .unwrap()
                .parent_id,
            "old"
        );
        assert_eq!(events::FILE_READS.with(std::cell::Cell::get), reads);
        assert_eq!(cache.paths().count(), 0);
        evidence.finish(&home.options).unwrap();
        let stamp = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, text.replace("claude:old", "claude:new")).unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(stamp))
            .unwrap();
        assert!(evidence.finish(&home.options).is_err());
        // A metadata-only hit never weakens selected-record verification.
        let mut evidence = Evidence::default();
        assert!(
            read(
                &home.options,
                &path,
                "claude",
                "parent",
                &mut cache,
                &mut false,
                &mut evidence
            )
            .is_err()
        );
    }
}

#[test]
fn tree_restart_preserves_raw_activity_all_models_and_historical_tool_accounting() {
    let home = Home::new();
    let legacy = b"historical tree metadata; retain unchanged";
    fs::write(&home.options.cache, legacy).unwrap();
    let text = "invalid JSON\n[]\n".to_owned()
        + &line(json!({"type":"assistant","uuid":"one","timestamp":"2026-09-24T00:00:00.123000+00:00","cwd":"/work/repo","gitBranch":"native","message":{"id":"same","model":"z","usage":{"input_tokens":20,"output_tokens":1},"content":[{"type":"tool_result","tool_use_id":"reused","content":"SECRET-RESULT"}]}}))
        + &line(json!({"type":"assistant","uuid":"two","timestamp":"not-a-time","message":{"id":"same","usage":{"input_tokens":1,"output_tokens":4},"content":[{"type":"tool_use","id":"reused","name":"Bash","input":{"command":"SECRET-INPUT"}}]}}))
        + &line(json!({"type":"assistant","uuid":"three","message":{"id":"other","model":"a","usage":{"input_tokens":2,"output_tokens":3},"content":[{"type":"tool_use","id":"reused","name":"Skill"},{"type":"tool_use","id":"open","name":"Read","input":{"path":"SECRET-PATH"}}]}}))
        + &json!({"timestamp":"2027-01-01T00:00:00Z","message":{"model":"eof","id":"tail","usage":{"input_tokens":999}}}).to_string();
    let path = home.write("claude/projects/repo/parent.jsonl", &text);
    let old_reads = events::CACHE_READS.with(std::cell::Cell::get);
    for iteration in 0..2 {
        let parsed = events::PARSED.with(std::cell::Cell::get);
        let reads = events::FILE_READS.with(std::cell::Cell::get);
        let nodes = collect(&home.options).unwrap();
        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        assert_eq!(
            node.first_activity.as_deref(),
            Some("2026-09-24T00:00:00.123000+00:00")
        );
        assert_eq!(node.last_activity.as_deref(), Some("not-a-time"));
        assert_eq!(node.last_activity_age_seconds, None);
        assert_eq!(node.models, ["a", "z"]);
        assert_eq!(node.malformed_lines, 2);
        assert_eq!(
            (node.tokens.input, node.tokens.output, node.tokens.total),
            (3, 7, 10)
        );
        assert_eq!(
            node.open_tools,
            [ToolCall {
                id: "open".into(),
                name: "Read".into()
            }]
        );
        assert_eq!(node.cwd.as_deref(), Some("/work/repo"));
        if iteration == 1 {
            assert_eq!(events::PARSED.with(std::cell::Cell::get), parsed);
            assert_eq!(events::FILE_READS.with(std::cell::Cell::get), reads);
        }
    }
    assert_eq!(
        events::CACHE_READS.with(std::cell::Cell::get),
        old_reads,
        "tree must not restore the full-workspace cache"
    );
    let connection =
        rusqlite::Connection::open(events::EventCache::path(&home.options.cache)).unwrap();
    let encoded: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("native_tree_summary:{}", path.display())],
            |row| row.get(0),
        )
        .unwrap();
    let summary: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(summary["summary"]["line_count"], 5);
    assert_eq!(summary["summary"]["usage_models"]["same"], "z");
    assert_eq!(
        summary["summary"]["usage_records"]["same"],
        json!(["one", "two"])
    );
    assert!(!encoded.contains("SECRET") && !encoded.contains("marker"));
    assert_eq!(fs::read(&home.options.cache).unwrap(), legacy);
    assert_eq!(fs::read(path).unwrap(), text.as_bytes());
}

#[test]
fn missing_corrupt_old_and_wrong_generation_tree_metadata_repair_without_event_replay() {
    let home = Home::new();
    let text = line(
        json!({"type":"assistant","timestamp":"2026-09-24T00:00:00Z","message":{"id":"one","model":"native","usage":{"input_tokens":1,"output_tokens":2}}}),
    );
    let path = home.write("claude/projects/repo/parent.jsonl", &text);
    assert_eq!(collect(&home.options).unwrap()[0].tokens.total, 3);
    let connection =
        rusqlite::Connection::open(events::EventCache::path(&home.options.cache)).unwrap();
    let key = format!("native_tree_summary:{}", path.display());
    let parsed = events::PARSED.with(std::cell::Cell::get);
    for change in [
        "DELETE FROM meta WHERE key=?1",
        "UPDATE meta SET value='{' WHERE key=?1",
        "UPDATE meta SET value=json_set(value,'$.version',99) WHERE key=?1",
        "UPDATE meta SET value=json_set(value,'$.ledger','wrong') WHERE key=?1",
        "UPDATE meta SET value=printf('%1048577s','x') WHERE key=?1",
    ] {
        connection.execute(change, [&key]).unwrap();
        let nodes = collect(&home.options).unwrap();
        assert_eq!(nodes[0].tokens.total, 3);
        assert_eq!(nodes[0].models, ["native"]);
        assert_eq!(events::PARSED.with(std::cell::Cell::get), parsed);
    }
    connection
        .execute("DELETE FROM meta WHERE key=?1", [&key])
        .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(line(json!({"message":{"id":"two","usage":{"output_tokens":4}}})).as_bytes())
        .unwrap();
    assert_eq!(collect(&home.options).unwrap()[0].tokens.total, 7);
    assert_eq!(events::PARSED.with(std::cell::Cell::get), parsed + 1);
}

#[test]
fn tree_publication_rechecks_prior_sources_and_provisional_eof_evidence() {
    let _clock = events::fixed_change_time_for_test(0);
    for eof in [false, true] {
        let home = Home::new();
        let value =
            json!({"type":"user","message":{"content":"Semon-Parent: claude:old"}}).to_string();
        let text = if eof { value } else { value + "\n" };
        let path = home.write("claude/projects/repo/parent.jsonl", &text);
        let legacy = crate::read_first_marker(&path).unwrap().unwrap();
        assert_eq!(legacy.parent_id, "old");
        let mut cache = home.cache();
        let mut evidence = Evidence::default();
        assert_eq!(
            home.observe(&path, &mut cache, &mut evidence)
                .marker
                .unwrap()
                .parent_id,
            "old"
        );
        evidence.finish(&home.options).unwrap();
        let stamp = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, text.replace("claude:old", "claude:new")).unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(stamp))
            .unwrap();
        assert!(evidence.finish(&home.options).is_err());
    }
}

#[test]
fn tree_indexed_launches_link_eof_markers_and_keep_prompt_inputs_out_of_metadata() {
    let home = Home::new();
    let call = json!({"timestamp":"2026-09-24T00:00:00Z","type":"assistant","message":{"content":[{"type":"tool_use","id":"launch","name":"Bash","input":{"command":"codex exec < /private/SECRET-PROMPT"}}]}}).to_string();
    home.write("claude/projects/repo/parent.jsonl", &call);
    let child = line(json!({"timestamp":"2026-09-24T00:00:01Z","type":"session_meta","payload":{"id":"child","cwd":"/work/repo"}}))
        + &json!({"timestamp":"2026-09-24T00:00:02Z","type":"event_msg","payload":{"type":"user_message","message":"Semon-Parent: claude:parent\nSemon-Handoff: /private/SECRET-PROMPT"}}).to_string();
    home.write("codex/sessions/child.jsonl", &child);
    for iteration in 0..2 {
        let parsed = events::PARSED.with(std::cell::Cell::get);
        let nodes = collect(&home.options).unwrap();
        let parent = nodes.iter().find(|node| node.id == "parent").unwrap();
        let child = parent
            .children
            .iter()
            .find(|node| node.id == "child")
            .unwrap();
        assert_eq!(child.parent_source.as_deref(), Some("marker"));
        assert_eq!(
            child.via_tool,
            Some(ToolCall {
                id: "launch".into(),
                name: "Bash".into()
            })
        );
        if iteration == 1 {
            assert_eq!(events::PARSED.with(std::cell::Cell::get), parsed);
        }
    }
    let db = fs::read(events::EventCache::path(&home.options.cache)).unwrap();
    assert!(!String::from_utf8_lossy(&db).contains("SECRET-PROMPT"));
    assert!(!home.options.cache.exists());
}

#[test]
fn tree_replaces_summary_on_inode_change_instead_of_reusing_stale_tool_or_usage_state() {
    let home = Home::new();
    let path = home.write("claude/projects/repo/parent.jsonl", &line(json!({"message":{"id":"one","model":"old","usage":{"output_tokens":7},"content":[{"type":"tool_result","tool_use_id":"reused"}]}})));
    assert_eq!(collect(&home.options).unwrap()[0].tokens.output, 7);
    let replacement = home.write("replacement.jsonl", &line(json!({"message":{"id":"one","model":"new","usage":{"output_tokens":2},"content":[{"type":"tool_use","id":"reused","name":"Bash"}]}})));
    fs::rename(replacement, &path).unwrap();
    let nodes = collect(&home.options).unwrap();
    assert_eq!(nodes[0].tokens.output, 2);
    assert_eq!(nodes[0].models, ["new"]);
    assert_eq!(
        nodes[0].open_tools,
        [ToolCall {
            id: "reused".into(),
            name: "Bash".into()
        }]
    );
}

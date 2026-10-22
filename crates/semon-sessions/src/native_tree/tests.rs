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

fn explicit_usage(id: u64, input: u64) -> Value {
    json!({"type":"token_usage_record","timestamp":"2026-10-01T00:00:00Z","payload":{
        "thread_id":"native","session_id":"session","turn_id":format!("turn-{id}"),
        "response_id":format!("response-{id}-{}","x".repeat(96)),
        "usage":{"input_tokens":input,"output_tokens":0,"total_tokens":input,
        "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}})
}

#[test]
fn native_usage_memory_only_reader_keeps_deduplication_until_a_durable_checkpoint() {
    let home = Home::new();
    let text = line(json!({"type":"session_meta","payload":{"id":"native"}}))
        + &line(json!({"type":"turn_context","payload":{"model":"gpt-6-sol"}}))
        + &line(explicit_usage(1, 7));
    let path = home.write("codex/sessions/2026/10/01/rollout-native.jsonl", &text);
    let mut cache = events::EventCache::default();
    assert_eq!(
        events::scan_file(&path, "codex", &mut cache, &mut false)
            .unwrap()
            .tokens("codex")
            .input,
        7
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(
            (line(explicit_usage(1, 7))
                + &line(explicit_usage(1, 99))
                + &line(explicit_usage(2, 11)))
                .as_bytes(),
        )
        .unwrap();
    let index = events::scan_file(&path, "codex", &mut cache, &mut false).unwrap();
    assert_eq!(index.tokens("codex").input, 18);
    assert_eq!(
        index.tree_summary.as_ref().unwrap().tokens("codex").input,
        18
    );
    assert!(!events::EventCache::path(&home.options.cache).exists());
}

#[test]
fn native_usage_append_work_depends_on_touched_rows_and_pricing_dimensions() {
    let mut writes = None;
    for count in [8, 512, 8192] {
        let home = Home::new();
        let mut text = line(json!({"type":"session_meta","payload":{"id":"native"}}))
            + &line(json!({"type":"turn_context","payload":{"model":"gpt-6-sol"}}));
        for n in 0..count {
            text += &line(explicit_usage(n, 7));
        }
        let path = home.write("codex/sessions/2026/10/01/rollout-native.jsonl", &text);
        let baseline = {
            let index = events::scan_file(&path, "codex", &mut home.cache(), &mut false).unwrap();
            json!(index.codex_cost().finish())
        };
        let db = rusqlite::Connection::open(events::EventCache::path(&home.options.cache)).unwrap();
        db.execute_batch("CREATE TRIGGER facts_immutable BEFORE UPDATE ON native_usage_facts BEGIN SELECT RAISE(ABORT,'old fact rewritten'); END;").unwrap();
        let restored =
            events::store::native_usage::RESTORED_DIMENSION_ROWS.with(std::cell::Cell::get);
        let index = events::scan_file(&path, "codex", &mut home.cache(), &mut false).unwrap();
        assert_eq!(
            json!(index.codex_cost().finish()),
            baseline,
            "cost changed across serialization"
        );
        assert_eq!(
            events::store::native_usage::RESTORED_DIMENSION_ROWS.with(std::cell::Cell::get)
                - restored,
            3
        );
        assert_eq!(index.native_usage_history_sizes(), Some((0, 0)));
        drop(index);
        let loads = events::FILE_READS.with(std::cell::Cell::get);
        let mut evidence = Evidence::default();
        let tree = read(
            &home.options,
            &path,
            "codex",
            "native",
            &mut home.cache(),
            &mut false,
            &mut evidence,
        )
        .unwrap();
        evidence.finish(&home.options).unwrap();
        assert_eq!(tree.tokens("codex").input, count * 7);
        assert_eq!(events::FILE_READS.with(std::cell::Cell::get), loads);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(line(explicit_usage(count, 11)).as_bytes())
            .unwrap();
        let before = events::INDEX_WRITTEN_ROWS.with(std::cell::Cell::get);
        let index = events::scan_file(&path, "codex", &mut home.cache(), &mut false).unwrap();
        let changed = events::INDEX_WRITTEN_ROWS.with(std::cell::Cell::get) - before;
        assert_eq!(index.tokens("codex").input, count * 7 + 11);
        assert_eq!(
            db.query_row("SELECT count(*) FROM native_usage_facts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            count as i64 + 1
        );
        assert!(
            changed < 128,
            "append changed {changed} rows for {count} reports"
        );
        if let Some(previous) = writes {
            assert!(changed <= previous + 4, "append writes grew with history");
        }
        writes = Some(changed);
        let plan:String=db.query_row("EXPLAIN QUERY PLAN SELECT native_offset FROM native_usage_identities WHERE source_id=?1 AND scope='model' AND identity=?2",rusqlite::params![1,"synthetic"],|r|r.get(3)).unwrap();
        assert!(
            plan.contains("PRIMARY KEY"),
            "identity lookup scans history: {plan}"
        );
        eprintln!(
            "usage-scaling history={count} native_restore_rows=3 native_restore_identities=0 native_restore_events=0 tree_index_loads=0 append_sql_changed_rows={changed} appended_facts=1 identity_plan={plan}"
        );
    }
}

#[test]
fn native_usage_tree_dimensions_keep_root_small_and_restore_only_active_tools() {
    for count in [8, 4096] {
        let home = Home::new();
        let mut text = String::new();
        for n in 0..count {
            text += &line(
                json!({"type":"assistant","uuid":format!("uuid-{n}"),"message":{"id":format!("msg-{n}"),"model":"native","usage":{"input_tokens":1},"content":[{"type":"tool_use","id":format!("tool-{n}"),"name":"Read"}]}}),
            );
            if n >= 2 {
                text += &line(
                    json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":format!("tool-{n}"),"content":"synthetic"}]}}),
                );
            }
        }
        let path = home.write("claude/projects/repo/parent.jsonl", &text);
        let nodes = collect(&home.options).unwrap();
        assert_eq!(nodes[0].tokens.input, count);
        assert_eq!(nodes[0].open_tools.len(), 2);
        let db = rusqlite::Connection::open(events::EventCache::path(&home.options.cache)).unwrap();
        let bytes: i64 = db
            .query_row(
                "SELECT octet_length(value) FROM meta WHERE key=?1",
                [format!("native_tree_summary:{}", path.display())],
                |r| r.get(0),
            )
            .unwrap();
        assert!(bytes < 2048);
        let parsed = events::PARSED.with(std::cell::Cell::get);
        let reads = events::FILE_READS.with(std::cell::Cell::get);
        let restarted = collect(&home.options).unwrap();
        assert_eq!(restarted[0].tokens.input, count);
        assert_eq!(events::PARSED.with(std::cell::Cell::get), parsed);
        assert_eq!(events::FILE_READS.with(std::cell::Cell::get), reads);
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(line(json!({"type":"assistant","message":{"id":"msg-0","usage":{"input_tokens":7},"content":[{"type":"tool_result","tool_use_id":"tool-0"},{"type":"tool_use","id":"tool-2","name":"Skill"}]}})).as_bytes()).unwrap();
        let appended = collect(&home.options).unwrap();
        assert_eq!(appended[0].tokens.input, count + 6);
        assert_eq!(
            appended[0].open_tools.len(),
            1,
            "a historical closed ID reopened"
        );
        assert_eq!(appended[0].open_tools[0].id, "tool-1");
        assert_eq!(
            db.query_row("SELECT count(*) FROM native_tree_tools", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            count as i64
        );
        eprintln!(
            "tree-dimensions history={count} root_bytes={bytes} active_tools=2 cold_index_loads=0"
        );
    }
}

#[test]
fn native_usage_facts_preserve_duplicate_conflict_unknown_model_and_incarnation_scope() {
    let home = Home::new();
    let unknown = explicit_usage(1, 7);
    let mut text = line(json!({"type":"session_meta","payload":{"id":"native"}}));
    text += &line(unknown.clone());
    text += &line(json!({"type":"turn_context","payload":{"model":"unknown-model"}}));
    text += &line(unknown);
    text += &line(explicit_usage(1, 99));
    text += &line(explicit_usage(2, 0));
    let path = home.write("codex/sessions/2026/10/01/rollout-native.jsonl", &text);
    {
        let mut cache = home.cache();
        let index = events::scan_file(&path, "codex", &mut cache, &mut false).unwrap();
        assert_eq!(index.tokens("codex").input, 7);
        assert_eq!(
            index.tree_summary.as_ref().unwrap().tokens("codex").input,
            7
        );
    }
    let db = rusqlite::Connection::open(events::EventCache::path(&home.options.cache)).unwrap();
    let source = || {
        db.query_row(
            "SELECT source_id FROM native_usage_current WHERE source_path=?1",
            [path.to_str().unwrap()],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    let original = source();
    let outcomes: Vec<(String,String)> = db.prepare("SELECT outcome,model_outcome FROM native_usage_facts WHERE source_id=?1 ORDER BY native_offset").unwrap().query_map([original],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(
        outcomes,
        [
            ("accepted".into(), "unavailable".into()),
            ("conflict".into(), "accepted".into()),
            ("conflict".into(), "conflict".into()),
            ("accepted".into(), "accepted".into())
        ]
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(line(explicit_usage(1, 7)).as_bytes())
        .unwrap();
    {
        let mut cache = home.cache();
        let index = events::scan_file(&path, "codex", &mut cache, &mut false).unwrap();
        assert_eq!(index.tokens("codex").input, 7);
        assert_eq!(
            index.tree_summary.as_ref().unwrap().tokens("codex").input,
            7
        );
    }
    assert_eq!(source(), original, "append changed incarnation");
    fs::write(
        &path,
        line(json!({"type":"session_meta","payload":{"id":"native","forked_from_id":"parent"}}))
            + &line(json!({"type":"turn_context","payload":{"model":"unknown-model"}}))
            + &line(explicit_usage(1, u64::MAX)),
    )
    .unwrap();
    {
        let mut cache = home.cache();
        let index = events::scan_file(&path, "codex", &mut cache, &mut false).unwrap();
        assert_eq!(index.tokens("codex").input, u64::MAX);
    }
    assert_ne!(
        source(),
        original,
        "replacement retained the old incarnation"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM native_usage_facts WHERE source_id=?1",
            [original],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        5,
        "replacement lost original provenance"
    );
    let mut cache = home.cache();
    assert_eq!(
        events::scan_file(&path, "codex", &mut cache, &mut false)
            .unwrap()
            .tokens("codex")
            .input,
        u64::MAX
    );
}

#[test]
fn current_claude_missing_projection_repairs_in_bounded_slices() {
    let home = Home::new();
    let mut text = String::new();
    for n in 0..8192 {
        text += &line(
            json!({"type":"assistant","uuid":format!("u-{n}"),"message":{"id":format!("m-{n}"),"model":"claude-sonnet-4-5","usage":{"input_tokens":3},"content":[{"type":"tool_use","id":format!("t-{n}"),"name":"Read"}]}}),
        );
        text += &line(
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":format!("t-{n}")} ]}}),
        );
    }
    text += &line(json!({"type":"assistant","message":{"id":"m-0","usage":{"input_tokens":9}}}));
    let path = home.write("claude/projects/repo/parent.jsonl", &text);
    collect(&home.options).unwrap();
    let db = rusqlite::Connection::open(events::EventCache::path(&home.options.cache)).unwrap();
    db.execute(
        "DELETE FROM meta WHERE key LIKE 'native_tree_summary:%'",
        [],
    )
    .unwrap();
    let revision: String = db
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_source_revision'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let error = events::scan_file(&path, "claude", &mut home.cache(), &mut false).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    let mut steps = 0;
    let mut previous = 0;
    loop {
        let pending = home.cache().advance_tree_repair(&home.options).unwrap();
        steps += 1;
        let cursor: i64 = db
            .query_row("SELECT cursor FROM native_tree_repairs", [], |r| r.get(0))
            .unwrap();
        assert!(cursor - previous <= 1024 * 1024 + 32 * 1024 * 1024);
        previous = cursor;
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='catalog_source_revision'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            revision
        );
        if !pending {
            break;
        }
        assert!(steps < 128);
    }
    assert!(steps > 16);
    let nodes = collect(&home.options).unwrap();
    assert_eq!(nodes[0].tokens.input, 8192 * 3 + 6);
    assert!(nodes[0].open_tools.is_empty());
    let root: i64 = db
        .query_row(
            "SELECT octet_length(value) FROM meta WHERE key=?1",
            [format!("native_tree_summary:{}", path.display())],
            |r| r.get(0),
        )
        .unwrap();
    assert!(root < 2048);
    eprintln!(
        "claude-projection history=8192 root_bytes={root} repair_steps={steps} max_records_per_slice=512"
    );
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
    assert_eq!(summary["version"], 3);
    assert_eq!(summary["summary"]["usage_models"], json!({}));
    assert_eq!(
        connection
            .query_row(
                "SELECT model FROM native_tree_usage_models WHERE message_id='same'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "z"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM usage_identity WHERE message_id='same'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
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

use super::*;
use serde_json::json;
use std::{
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "semon-handoff-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn call(id: &str, command: &str) -> String {
    format!(
        "{}\n",
        json!({"timestamp":"2026-09-24T00:00:00Z","message":{"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":command}}]}})
    )
}

#[test]
fn unchanged_and_appended_scans_parse_only_complete_candidate_lines() {
    let home = Home::new();
    let first = call("first", "codex exec -C /work/foo");
    let second = call("second", "codex exec -C /work/bar");
    let split = second.len() / 2;
    let initial = format!("not JSON, no launch marker\n{first}{}", &second[..split]);
    let path = home.file("parent", &initial);
    let mut cache = ScanCache::default();
    let bytes = BYTES.with(|count| count.get());
    let parses = PARSES.with(|count| count.get());
    let parent = cache.parent(&path).unwrap();
    assert_eq!(parent.calls.len(), 1);
    assert_eq!(parent.partial, &second.as_bytes()[..split]);
    assert_eq!(
        BYTES.with(|count| count.get()) - bytes,
        initial.len() as u64
    );
    assert_eq!(PARSES.with(|count| count.get()) - parses, 1);
    cache.parent(&path).unwrap();
    assert_eq!(
        BYTES.with(|count| count.get()) - bytes,
        initial.len() as u64
    );
    assert_eq!(PARSES.with(|count| count.get()) - parses, 1);
    let appended = format!("{}unrelated invalid JSON\n", &second[split..]);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(appended.as_bytes())
        .unwrap();
    let parent = cache.parent(&path).unwrap();
    assert_eq!(parent.calls.len(), 2);
    assert!(parent.partial.is_empty());
    assert_eq!(parent.offset, (initial.len() + appended.len()) as u64);
    assert_eq!(BYTES.with(|count| count.get()) - bytes, parent.offset);
    assert_eq!(PARSES.with(|count| count.get()) - parses, 2);
}

#[test]
fn cache_caps_are_invariants_and_evict_lru_parents_and_oldest_calls() {
    let home = Home::new();
    let mut cache = ScanCache::default();
    let paths: Vec<_> = (0..MAX_PARENTS)
        .map(|index| home.file(&format!("parent-{index}"), ""))
        .collect();
    for path in &paths {
        cache.parent(path).unwrap();
        assert!(cache.parents.len() <= MAX_PARENTS);
    }
    cache.parent(&paths[0]).unwrap();
    let extra = home.file("extra", "");
    cache.parent(&extra).unwrap();
    assert_eq!(cache.parents.len(), MAX_PARENTS);
    assert!(cache.parents.contains_key(&paths[0]));
    assert!(!cache.parents.contains_key(&paths[1]));
    assert!(cache.parents.contains_key(&extra));
    let records: String = (0..MAX_CALLS + 7)
        .map(|index| call(&index.to_string(), "codex exec"))
        .collect();
    let path = home.file("calls", &records);
    let parent = cache.parent(&path).unwrap();
    assert_eq!(parent.calls.len(), MAX_CALLS);
    assert_eq!(parent.calls.front().unwrap().tool.id, "7");
    assert_eq!(
        parent.calls.back().unwrap().tool.id,
        (MAX_CALLS + 6).to_string()
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(call("newest", "codex exec").as_bytes())
        .unwrap();
    let parent = cache.parent(&path).unwrap();
    assert_eq!(parent.calls.len(), MAX_CALLS);
    assert_eq!(parent.calls.front().unwrap().tool.id, "8");
    assert_eq!(parent.calls.back().unwrap().tool.id, "newest");
    assert!(cache.parents.len() <= MAX_PARENTS);
}

#[test]
fn changed_same_size_truncated_and_replaced_parents_reset_the_scan() {
    let home = Home::new();
    let path = home.file("parent", &call("old", "codex exec"));
    let mut cache = ScanCache::default();
    cache.parent(&path).unwrap();
    let old_stamp = cache.parents[&path].stamp;
    fs::write(&path, call("new", "codex exec")).unwrap();
    fs::File::open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(old_stamp.modified.unwrap() + std::time::Duration::from_secs(1)),
        )
        .unwrap();
    assert_eq!(
        cache.parent(&path).unwrap().calls.front().unwrap().tool.id,
        "new"
    );
    fs::write(&path, "").unwrap();
    assert!(cache.parent(&path).unwrap().calls.is_empty());
    let replacement = home.file("replacement", &call("replacement", "codex exec"));
    fs::rename(replacement, &path).unwrap();
    assert_eq!(
        cache.parent(&path).unwrap().calls.front().unwrap().tool.id,
        "replacement"
    );
}

#[test]
fn literal_priority_clock_skew_and_path_segments() {
    let home = Home::new();
    let path = home.file(
        "parent",
        &(call("literal", "codex exec < /tmp/p.md")
            + &call("cwd", "codex exec -C /work/semon-wt-foo")),
    );
    let mut cache = ScanCache::default();
    let start = events::parse_ms("2026-09-23T23:59:58Z").unwrap();
    assert_eq!(
        cache
            .find(
                std::slice::from_ref(&path),
                "/tmp/p.md",
                Some("/work/semon-wt-foo"),
                Some(start - 1)
            )
            .unwrap()
            .id,
        "literal"
    );
    assert_eq!(
        cache
            .find(
                std::slice::from_ref(&path),
                "/tmp/absent.md",
                Some("/work/semon-wt-foo"),
                Some(start)
            )
            .unwrap()
            .id,
        "cwd"
    );
    assert!(
        cache
            .find(
                std::slice::from_ref(&path),
                "/tmp/absent.md",
                Some("/work/semon-wt-foo"),
                Some(start - 1)
            )
            .is_none()
    );
    assert!(
        cache
            .find(
                std::slice::from_ref(&path),
                "/tmp/absent.md",
                Some("/work/semon-wt-foo"),
                None
            )
            .is_none()
    );
    assert!(matches_cwd(
        "codex exec -C 'semon-wt-foo'",
        "/work/semon-wt-foo"
    ));
    assert!(matches_cwd(
        "codex exec -C /else/semon-wt-foo/subdir",
        "/work/semon-wt-foo"
    ));
    for text in [
        "codex exec -C /work/semon-wt-foobar",
        "codex exec -C /work/semon-wt-foo.old",
        "codex exec -C /work/pre-semon-wt-foo",
    ] {
        assert!(!matches_cwd(text, "/work/semon-wt-foo"), "{text}");
    }
    assert!(!matches_cwd("codex exec", "/"));
}

#[test]
fn literal_match_in_another_file_beats_cwd_and_only_launch_tools_are_candidates() {
    let home = Home::new();
    let cwd = home.file("cwd", &call("cwd", "codex exec -C /work/foo"));
    let literal = home.file("literal", &format!("{}\n", json!({"timestamp":"2026-09-24T00:00:00Z","message":{"content":[
        {"type":"tool_use","id":"ignored","name":"Read","input":{"path":"/tmp/p.md","note":"Semon-Handoff"}},
        {"type":"tool_use","id":"skill","name":"Skill","input":{"args":"Semon-Handoff: /tmp/p.md"}}
    ]}})));
    let mut cache = ScanCache::default();
    let start = events::parse_ms("2026-09-24T00:00:08Z").unwrap();
    assert_eq!(
        cache
            .find(
                &[cwd, literal.clone()],
                "/tmp/p.md",
                Some("/work/foo"),
                Some(start)
            )
            .unwrap(),
        ToolCall {
            id: "skill".into(),
            name: "Skill".into()
        }
    );
    assert_eq!(cache.parent(&literal).unwrap().calls.len(), 1);
}

#[test]
fn prompt_names_override_ambiguous_worktrees_and_generic_cwds_do_not_link() {
    let home = Home::new();
    let path = home.file(
        "parent",
        &(call(
            "one",
            "P=$S/codex-one-prompt.md; codex -C /work/repo exec - < $P",
        ) + &call(
            "two",
            "P=$S/codex-two-prompt.md; codex -C /work/repo exec - < $P",
        )),
    );
    let mut cache = ScanCache::default();
    let start = events::parse_ms("2026-09-24T00:00:01Z").unwrap();
    assert_eq!(
        cache
            .find(
                std::slice::from_ref(&path),
                "/tmp/codex-one-prompt.md",
                Some("/work/repo"),
                Some(start)
            )
            .unwrap()
            .id,
        "one"
    );
    assert!(
        cache
            .find(
                std::slice::from_ref(&path),
                "/tmp/absent.md",
                Some("/work/repo"),
                Some(start)
            )
            .is_none()
    );
    assert!(
        cache
            .find(
                std::slice::from_ref(&path),
                "/tmp/absent.md",
                Some("/work"),
                Some(start)
            )
            .is_none()
    );
    assert!(
        cache
            .find(
                &[path],
                "/tmp/one-prompt.md",
                Some("/work/repo"),
                Some(start)
            )
            .is_none()
    );
}

#[test]
fn final_complete_record_without_newline_and_literal_codex_wrapper_are_read() {
    let home = Home::new();
    let record = call("wrapper", "codex -C /work/repo -m model exec - < /tmp/p.md");
    let path = home.file("parent", record.trim_end());
    let mut cache = ScanCache::default();
    assert_eq!(
        cache.find(&[path], "/tmp/p.md", None, None).unwrap().id,
        "wrapper"
    );
}

#[test]
fn retained_call_inputs_have_a_byte_budget() {
    let home = Home::new();
    let command = format!("codex {}", "x".repeat(MAX_INPUT_BYTES / 2));
    let path = home.file(
        "parent",
        &(call("one", &command) + &call("two", &command) + &call("three", &command)),
    );
    let mut cache = ScanCache::default();
    let parent = cache.parent(&path).unwrap();
    assert!(parent.input_bytes <= MAX_INPUT_BYTES);
    assert_eq!(parent.calls.len(), 1);
    assert_eq!(parent.calls.front().unwrap().tool.id, "three");
}

#[test]
fn oversized_unfinished_records_are_bounded_and_skip_to_next_record() {
    let home = Home::new();
    let path = home.file("parent", &"codex".repeat(MAX_INPUT_BYTES / 5 + 100));
    let mut cache = ScanCache::default();
    let parent = cache.parent(&path).unwrap();
    assert!(parent.discarding);
    assert!(parent.partial.len() <= MAX_INPUT_BYTES);
    assert!(parent.partial.capacity() <= MAX_INPUT_BYTES + 1);
    assert!(parent.calls.is_empty());
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(format!("\n{}", call("good", "codex exec -C /work/project")).as_bytes())
        .unwrap();
    let parent = cache.parent(&path).unwrap();
    assert!(!parent.discarding);
    assert!(parent.partial.is_empty());
    assert_eq!(parent.calls.len(), 1);
    assert_eq!(parent.calls[0].tool.id, "good");
}

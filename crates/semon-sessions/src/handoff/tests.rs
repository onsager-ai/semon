use super::*;
use crate::{Options, events, query_source};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, path::PathBuf};

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
    fn options(&self) -> Options {
        Options {
            claude_home: self.0.clone(),
            claude_json: self.0.join("settings.json"),
            codex_home: self.0.join("codex"),
            copilot_home: self.0.join("copilot"),
            proc_root: self.0.join("proc"),
            cache: self.0.join("index.json"),
            all: true,
            ..Options::default()
        }
    }

    fn find(
        &self,
        paths: &[PathBuf],
        prompt: &str,
        cwd: Option<&str>,
        start: Option<i64>,
    ) -> Option<ToolCall> {
        let options = self.options();
        let mut cache = events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let mut indexed = Vec::new();
        for path in paths {
            let index = events::scan_file(path, "claude", &mut cache, &mut false).unwrap();
            let source = cache.source(path, "parent").unwrap();
            indexed.push((path, index, source));
        }
        let sources = query_source::Sources::capture_native(
            &options,
            indexed
                .iter()
                .map(|(_, index, source)| (source.clone(), index.source_chunks.clone())),
        )
        .unwrap();
        let records = sources.reader();
        records.verify_all().unwrap();
        let mut matcher = Matcher::new(prompt, cwd, start);
        let mut selected = None;
        'parents: for (path, index, _) in &indexed {
            for location in index.handoff_index.as_ref().unwrap().calls() {
                if let Some(tool) = matcher.consider(&records.launch(path, location).unwrap()) {
                    selected = Some(tool);
                    break 'parents;
                }
            }
        }
        records.finish().unwrap();
        selected.or_else(|| matcher.finish())
    }

    fn count(&self, path: &Path) -> usize {
        let mut cache = events::EventCache::open_scoped(&self.options().cache, &self.options().source).unwrap();
        events::scan_file(path, "claude", &mut cache, &mut false)
            .unwrap()
            .handoff_index
            .as_ref()
            .unwrap()
            .calls()
            .len()
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
fn literal_priority_clock_skew_and_path_segments() {
    let home = Home::new();
    let path = home.file(
        "parent",
        &(call("literal", "codex exec < /tmp/p.md")
            + &call("cwd", "codex exec -C /work/semon-wt-foo")),
    );
    let start = events::parse_ms("2026-09-23T23:59:58Z").unwrap();
    assert_eq!(
        home.find(
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
        home.find(
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
        home.find(
            std::slice::from_ref(&path),
            "/tmp/absent.md",
            Some("/work/semon-wt-foo"),
            Some(start - 1)
        )
        .is_none()
    );
    assert!(
        home.find(
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
    let start = events::parse_ms("2026-09-24T00:00:08Z").unwrap();
    assert_eq!(
        home.find(
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
    assert_eq!(home.count(&literal), 1);
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
    let start = events::parse_ms("2026-09-24T00:00:01Z").unwrap();
    assert_eq!(
        home.find(
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
        home.find(
            std::slice::from_ref(&path),
            "/tmp/absent.md",
            Some("/work/repo"),
            Some(start)
        )
        .is_none()
    );
    assert!(
        home.find(
            std::slice::from_ref(&path),
            "/tmp/absent.md",
            Some("/work"),
            Some(start)
        )
        .is_none()
    );
    assert!(
        home.find(
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
    assert_eq!(
        home.find(&[path], "/tmp/p.md", None, None).unwrap().id,
        "wrapper"
    );
}

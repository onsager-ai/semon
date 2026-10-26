use super::*;
use crate::events::EventCache;
use crate::storage::sqlite::catalog::{SQL_STEPS, rows};
use crate::{Refresh, ViewerCore};
use rusqlite::{Connection, types::Value as SqlValue};
use std::time::Duration;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    options: Options,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-catalog-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("proc")).unwrap();
        fs::write(root.join("proc/locks"), "").unwrap();
        let options = Options {
            source: crate::Options::default().source,
            claude_home: root.join("claude"),
            claude_json: root.join(".claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            all: true,
            since: Duration::from_secs(86400),
            session: None,
            facts: Some(root.join("facts.json")),
            scan_window: false,
        };
        crate::write_facts(
            options.facts.as_ref().unwrap(),
            &crate::Facts {
                version: crate::FACTS_VERSION,
                hostname: "synthetic-catalog".into(),
                repos: std::collections::BTreeMap::from([(
                    "/synthetic/project".into(),
                    Some("project".into()),
                )]),
                ..crate::Facts::default()
            },
            &options.source,
        )
        .unwrap();
        Self { root, options }
    }
    fn source(&self, id: &str) -> PathBuf {
        let path = self
            .options
            .claude_home
            .join(format!("projects/project/{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path,format!("{}\n",json!({"type":"user","sessionId":id,"uuid":id,"parentUuid":null,"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"synthetic prompt"}}))).unwrap();
        path
    }
    fn codex_source(&self, id: &str, parent: Option<&str>, modified: u64) -> String {
        let relative = format!("sessions/2026/10/01/rollout-{id}.jsonl");
        let path = self.options.codex_home.join(&relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
                &path,
                format!(
                    "{}\n",
                    json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":id,"parent_thread_id":parent,"thread_source":if parent.is_some() {"subagent"} else {"cli"},"cwd":"/synthetic/project"}})
                ),
            )
            .unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(modified)),
            )
            .unwrap();
        relative
    }
    /// The admitted current writer: the namespace binding and writer roles
    /// of the production source-journal connection.
    fn db(&self) -> Connection {
        let config = self.options.source.config(&self.options.cache).unwrap();
        crate::storage::sqlite::source_journal::SourceJournal(config)
            .connection()
            .unwrap()
    }
    fn publish(&self, count: usize) {
        for id in 0..count {
            self.source(&format!("session-{id:05}"));
        }
        self.publish_existing();
    }
    fn publish_existing(&self) {
        let evidence = crate::native_evidence::NativeEvidence::new(self.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        while !evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap()
        {}
        drop(work);
        evidence.retire();
    }
    fn body(&self, query: &str) -> (u16, Value) {
        // Projection assertions observe the exact publication under test.
        // Viewer transport now explicitly schedules its native producer.
        let reply = session_catalog_page(&self.options, "local", query);
        (reply.status, serde_json::from_slice(&reply.body).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn catalog_runtime_churn_does_not_starve_unrelated_history_or_new_sessions() {
    let fixture = Fixture::new();
    let paths = (0..40)
        .map(|n| fixture.source(&format!("history-{n:03}")))
        .collect::<Vec<_>>();
    let mut cache =
        EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
    for path in &paths {
        crate::events::scan_file(path, "claude", &mut cache, &mut false).unwrap();
    }
    let paths = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    cache
        .begin_semantic_stage(
            &paths,
            &crate::native_evidence::query_runtime_generation(&fixture.options).unwrap(),
        )
        .unwrap();
    let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
    let mut work = crate::viewer::lock(&evidence.work);
    work.events = Some(cache);
    let relative = fixture.codex_source("new-during-history", None, 1);
    let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative]));
    let mut complete = false;
    for turn in 0..256 {
        if turn % 2 == 0 {
            facts
                .codex_locks
                .insert("unrelated-running-thread".into(), 42);
        } else {
            facts.codex_locks.remove("unrelated-running-thread");
        }
        let intent = crate::SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();
        intent.complete().unwrap();
        if evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    let db = fixture.db();
    let history: i64 = db
        .query_row(
            "SELECT count(*) FROM session_catalog WHERE lifecycle='current' AND harness='claude'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let new_session: i64 = db.query_row("SELECT count(*) FROM session_catalog WHERE lifecycle='current' AND session_key='new-during-history'", [], |r| r.get(0)).unwrap();
    assert!(
        complete && history == 40 && new_session == 1,
        "unrelated runtime churn starved publication: complete={complete} history={history} new_session={new_session}"
    );
    drop(work);
    evidence.retire();
}

#[test]
fn catalog_runtime_change_revalidates_unpublished_staged_owners() {
    for global in [false, true] {
        let fixture = Fixture::new();
        let relative = fixture.codex_source("affected", None, 1);
        let path = fixture.options.codex_home.join(&relative);
        let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
        facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative]));
        let intent = crate::SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();
        intent.complete().unwrap();
        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        evidence
            .prepare_events(&mut work, crate::model::now_ms())
            .unwrap();
        let cache = work.events.as_mut().unwrap();
        crate::events::scan_file(&path, "codex", cache, &mut false).unwrap();
        cache
            .begin_semantic_stage(
                &[path.to_string_lossy().into_owned()],
                &crate::native_evidence::query_runtime_generation(&fixture.options).unwrap(),
            )
            .unwrap();
        let db = fixture.db();
        let mut staged = false;
        for _ in 0..64 {
            evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap();
            staged = db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources WHERE identity_done=1 AND publication_done=0)", [], |r| r.get(0)).unwrap();
            if staged {
                break;
            }
        }
        assert!(staged, "fixture did not reach an unpublished identity");
        let epoch: i64 = db
            .query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r.get(0))
            .unwrap();
        if global {
            facts.hostname = "changed-machine".into();
        } else {
            facts.codex_locks.insert("affected".into(), 42);
            facts.proc_starts.insert(42, 7);
        }
        let intent = crate::SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();
        intent.complete().unwrap();
        let dirty: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources s JOIN session_catalog_invalidations i USING(source_path) WHERE i.revision IS NOT s.claim)", [], |r| r.get(0)).unwrap();
        assert!(
            dirty,
            "runtime change did not journal the unpublished owner: global={global}"
        );
        evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap();
        let next: i64 = db
            .query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r.get(0))
            .unwrap();
        assert!(next > epoch, "affected staged identity was reused");
        let mut complete = false;
        for _ in 0..128 {
            if evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
            {
                complete = true;
                break;
            }
        }
        assert!(complete, "affected runtime publication did not recover");
        drop(work);
        evidence.retire();
        let (status, page) = fixture.body("sid=affected&limit=1");
        assert_eq!(status, 200, "{page}");
        assert_eq!(
            page["items"][0]["facts_observation"]["state"], "cached",
            "{page}"
        );
    }
}

#[test]
fn catalog_runtime_unjournaled_observations_keep_the_epoch_fence() {
    let fixture = Fixture::new();
    let path = fixture.source("unobserved-runtime");
    let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
    let mut work = crate::viewer::lock(&evidence.work);
    evidence
        .prepare_events(&mut work, crate::model::now_ms())
        .unwrap();
    let cache = work.events.as_mut().unwrap();
    crate::events::scan_file(&path, "claude", cache, &mut false).unwrap();
    cache
        .begin_semantic_stage(
            &[path.to_string_lossy().into_owned()],
            &crate::native_evidence::query_runtime_generation(&fixture.options).unwrap(),
        )
        .unwrap();
    evidence
        .refresh_catalog(&mut work, crate::model::now_ms())
        .unwrap();
    let db = fixture.db();
    let epoch: i64 = db
        .query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r.get(0))
        .unwrap();
    let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
    facts.codex_locks.insert("unobserved-thread".into(), 42);
    crate::write_facts(
        fixture.options.facts.as_ref().unwrap(),
        &facts,
        &crate::Options::default().source,
    )
    .unwrap();
    evidence
        .refresh_catalog(&mut work, crate::model::now_ms())
        .unwrap();
    let next: i64 = db
        .query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r.get(0))
        .unwrap();
    assert!(
        next > epoch,
        "an unjournaled runtime change bypassed its epoch fence"
    );
    drop(work);
    evidence.retire();
}

#[test]
fn native_usage_history_restart_converges_without_repair_revision_churn() {
    use std::io::Write;

    for count in [8, 8192] {
        let fixture = Fixture::new();
        let relative = fixture.codex_source("usage-history", None, 1);
        let path = fixture.options.codex_home.join(&relative);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"turn_context","payload":{"model":"gpt-6-sol"}})
        )
        .unwrap();
        for n in 0..count {
            let record = json!({"type":"token_usage_record","timestamp":"2026-10-01T00:00:00Z","payload":{
                    "thread_id":"usage-history","session_id":"native-session","turn_id":format!("turn-{n:08}"),
                    "response_id":format!("response-{n:08}-{}", "x".repeat(96)),"future_field":{"ignored":true},
                    "usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12,
                        "cached_input_tokens":3,"cache_write_input_tokens":0,"reasoning_output_tokens":1}}});
            writeln!(file, "{record}").unwrap();
            if n == 0 {
                writeln!(file, "{record}").unwrap();
            }
        }
        writeln!(file, "{}", json!({"type":"token_usage_record","payload":{
                "thread_id":"usage-history","session_id":"native-session","turn_id":"zero","response_id":"zero",
                "usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0,
                    "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}})).unwrap();
        drop(file);
        let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
        facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative]));
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();

        // Persist, release every Index, and reopen before unchanged repair.
        {
            let mut cache =
                EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
            let index = crate::events::scan_file(&path, "codex", &mut cache, &mut false).unwrap();
            assert_eq!(index.tokens("codex").input, count * 10);
        }
        let db = fixture.db();
        let revision = || {
            db.query_row(
                "SELECT revision FROM storage_source_journal WHERE namespace=semon_namespace()",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
        };
        let before = revision();
        for _ in 0..3 {
            let mut cache =
                EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
            let index = crate::events::scan_file(&path, "codex", &mut cache, &mut false).unwrap();
            assert_eq!(index.tokens("codex").input, count * 10);
            assert_eq!(
                revision(),
                before,
                "unchanged metadata repair advanced source revision"
            );
        }

        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        let mut complete = false;
        for _ in 0..128 {
            if evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
            {
                complete = true;
                break;
            }
        }
        assert!(
            complete,
            "usage history prevented bounded Catalog convergence"
        );
        assert!(!evidence.catalog_pending(&work));
        drop(work);
        evidence.retire();
        let (status, page) = fixture.body("scope=retained_history&limit=60");
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let root_bytes: i64 = db
            .query_row(
                "SELECT octet_length(value) FROM meta WHERE key=?1",
                [format!("native_tree_summary:{}", path.display())],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            root_bytes < 8192,
            "tree root grew with usage history: {root_bytes}"
        );
        // Save a real semantic claim, evict the Tree representation during
        // identity work, and require that same epoch to reach publication.
        // This exercises the original witness_claim failure chain, beyond
        // a successful small single-batch Catalog request.
        let staged = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&staged.work);
        staged
            .prepare_events(&mut work, crate::model::now_ms())
            .unwrap();
        assert_eq!(
            work.events
                .as_mut()
                .unwrap()
                .begin_semantic_stage(
                    &[path.to_string_lossy().into_owned()],
                    &crate::native_evidence::query_runtime_generation(&fixture.options).unwrap()
                )
                .unwrap(),
            crate::events::Outcome::Written
        );
        let epoch: i64 = db
            .query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r.get(0))
            .unwrap();
        let stable_revision = revision();
        let mut phases = std::collections::BTreeSet::new();
        let mut repaired = false;
        let mut finished = false;
        for _ in 0..256 {
            let phase: Option<String> = db
                .query_row("SELECT phase FROM catalog_semantic_stage", [], |r| r.get(0))
                .ok();
            if let Some(phase) = phase {
                phases.insert(phase.clone());
                if phase == "identity" && !repaired {
                    db.execute(
                        "UPDATE meta SET value='malformed representation' WHERE key=?1",
                        [format!("native_tree_summary:{}", path.display())],
                    )
                    .unwrap();
                    // Evict every in-memory Index before forcing repair.
                    work.events.as_mut().unwrap().release_published_indexes();
                    repaired = true;
                }
                assert_eq!(
                    db.query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    epoch,
                    "representation repair reset the semantic epoch"
                );
            }
            let complete = match staged.refresh_catalog(&mut work, crate::model::now_ms()) {
                Ok(complete) => complete,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && error.to_string().contains("projection repair pending") =>
                {
                    false
                }
                Err(error) => panic!("semantic repair failed: {error}"),
            };
            assert_eq!(
                revision(),
                stable_revision,
                "representation repair invalidated its saved claim; source_claim={:?}",
                work.events
                    .as_ref()
                    .unwrap()
                    .catalog_claims(&[path.to_string_lossy().into_owned()])
                    .unwrap()[0]
                    .1
            );
            if complete {
                finished = true;
                break;
            }
        }
        let remaining: (Option<String>, i64, i64, i64) = db.query_row("SELECT (SELECT phase FROM catalog_semantic_stage),(SELECT count(*) FROM catalog_semantic_sources WHERE publication_done=0 OR ack_done=0),(SELECT count(*) FROM session_catalog_invalidations),(SELECT count(*) FROM storage_semantic_model_batches)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert!(
            finished && repaired,
            "semantic representation repair did not finish: phase/pending/claims/models={remaining:?}"
        );
        for phase in ["indexing", "identity", "relations", "publication"] {
            assert!(
                phases.contains(phase),
                "missing semantic phase {phase}: {phases:?}"
            );
        }
        drop(work);
        staged.retire();
        let (status, body) = fixture.body("scope=retained_history&limit=60");
        assert_eq!(status, 200);
        let owner = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == "usage-history")
            .unwrap();
        let saved: String = db
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key='usage-history'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let saved: crate::model::summary::CatalogRow = serde_json::from_str(&saved).unwrap();
        let query_state = saved.query.as_ref().map(|q| &q["state"]);
        let generation_matches = saved.query.as_ref().is_some_and(|q| {
            q["source_generation"] == crate::retention::source_generation(&saved.sources).unwrap()
        });
        assert_eq!(
            owner["status"]["state"], "off",
            "native usage records cannot prove turn completion during staged repair: lifecycle={:?}, saved_state={query_state:?}, generation_matches={generation_matches}, transport_state={:?}, observation={:?}",
            saved.lifecycle, owner["state"], owner["status"]["observation"]
        );
        eprintln!(
            "usage-projection count={count} root_bytes={root_bytes} semantic_phases={phases:?} stable_epoch={epoch} revision_before={before} revision_after={}",
            revision()
        );
    }
}

#[test]
fn workspace_search_resumes_after_each_bounded_scan_frontier() {
    let a = Fixture::new();
    let b = Fixture::new();
    a.publish(600);
    let mut paths = std::collections::BTreeSet::new();
    for key in ["session-00000", "session-00550", "session-00999"] {
        paths.insert(b.codex_source(key, None, 1));
    }
    let path = b.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(path).unwrap();
    facts.codex_rollouts = Some(paths);
    crate::write_facts(path, &facts, &crate::Options::default().source).unwrap();
    b.publish_existing();
    let sources = [
        crate::OverviewSource {
            key: "a".into(),
            label: "Alpha".into(),
            options: a.options.clone(),
            up: false,
        },
        crate::OverviewSource {
            key: "b".into(),
            label: "Beta".into(),
            options: b.options.clone(),
            up: false,
        },
    ];
    let base = "scope=retained_history&q=session&harness=codex&limit=2";
    let mut query = base.to_owned();
    let mut found = Vec::new();
    let mut partial = false;
    let mut cursors = std::collections::BTreeSet::new();
    for _ in 0..12 {
        let result = session_catalog_union(&sources, "workspace", &query);
        assert_eq!(
            result.status,
            200,
            "{}",
            String::from_utf8_lossy(&result.body)
        );
        let value: Value = serde_json::from_slice(&result.body).unwrap();
        partial |= value["search"]["partial"] == true;
        found.extend(
            value["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["key"].as_str().unwrap().to_owned()),
        );
        let Some(cursor) = value["next_cursor"].as_str() else {
            break;
        };
        assert!(
            cursors.insert(cursor.to_owned()),
            "scan frontier must advance"
        );
        query = format!("{base}&cursor={}", crate::viewer::percent_encode(cursor));
    }
    assert!(
        partial,
        "fixture must exhaust the existing 512-candidate search budget"
    );
    assert_eq!(
        found,
        vec!["session-00000", "session-00550", "session-00999"]
    );
}

#[test]
fn workspace_union_pages_duplicate_ids_and_timestamps_without_native_reads() {
    let a = Fixture::new();
    let b = Fixture::new();
    a.publish(8);
    b.publish(8);
    let sources = vec![
        crate::OverviewSource {
            key: "a".into(),
            label: "Alpha".into(),
            options: a.options.clone(),
            up: false,
        },
        crate::OverviewSource {
            key: "b".into(),
            label: "Beta".into(),
            options: b.options.clone(),
            up: false,
        },
    ];
    let mut query = "scope=retained_history&limit=3".to_owned();
    let mut identities = Vec::new();
    let mut first_cursor = None;
    for _ in 0..20 {
        let result = session_catalog_union(&sources, "workspace-a", &query);
        assert_eq!(
            result.status,
            200,
            "{}",
            String::from_utf8_lossy(&result.body)
        );
        let value: Value = serde_json::from_slice(&result.body).unwrap();
        for item in value["items"].as_array().unwrap() {
            assert!(item.get("details").is_none());
            identities.push((
                item["last"].as_i64().unwrap_or(0),
                item["key"].as_str().unwrap().to_owned(),
                item["machine"]["key"].as_str().unwrap().to_owned(),
            ));
        }
        let Some(cursor) = value["next_cursor"].as_str() else {
            break;
        };
        first_cursor.get_or_insert_with(|| cursor.to_owned());
        query = format!(
            "scope=retained_history&limit=3&cursor={}",
            crate::viewer::percent_encode(cursor)
        );
    }
    assert_eq!(identities.len(), 16);
    let mut expected = identities.clone();
    expected.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    expected.dedup();
    assert_eq!(identities, expected);
    let cursor = crate::viewer::percent_encode(&first_cursor.unwrap());
    for (workspace, extra) in [
        ("workspace-b", ""),
        ("workspace-a", "&harness=codex"),
        ("workspace-a", "&scope=current"),
    ] {
        let result = session_catalog_union(
            &sources,
            workspace,
            &format!("limit=3&cursor={cursor}{extra}"),
        );
        assert_eq!(result.status, 400);
    }
    assert_eq!(
        session_catalog_union(&sources[..1], "workspace-a", &query).status,
        409
    );
    let empty = session_catalog_union(&[], "empty", "scope=retained_history");
    assert_eq!(empty.status, 200);
    let empty: Value = serde_json::from_slice(&empty.body).unwrap();
    assert_eq!(empty["items"], json!([]));
    assert_eq!(empty["next_cursor"], Value::Null);
    let single = session_catalog_union(
        &sources[..1],
        "workspace-a",
        "scope=retained_history&limit=100",
    );
    assert_eq!(single.status, 200);
    let single: Value = serde_json::from_slice(&single.body).unwrap();
    assert_eq!(single["items"].as_array().unwrap().len(), 8);
    let filtered = session_catalog_union(&sources, "workspace-a", "harness=codex");
    assert_eq!(filtered.status, 200);
    let filtered: Value = serde_json::from_slice(&filtered.body).unwrap();
    assert_eq!(filtered["items"], json!([]));
    let searching = session_catalog_union(&sources, "workspace-a", "q=session&limit=100");
    assert_eq!(
        searching.status,
        200,
        "{}",
        String::from_utf8_lossy(&searching.body)
    );
    let searching: Value = serde_json::from_slice(&searching.body).unwrap();
    assert_eq!(searching["items"].as_array().unwrap().len(), 16);
    // Raw body contents change behind an unchanged published Catalog. Lists
    // still read metadata; selected detail/restore is not consulted.
    fs::write(a.source("session-00000"), "not a native log").unwrap();
    let listed = session_catalog_union(&sources, "workspace-a", "limit=100");
    assert_eq!(listed.status, 200);
    let listed: Value = serde_json::from_slice(&listed.body).unwrap();
    assert_eq!(listed["items"].as_array().unwrap().len(), 16);
    let mut missing = sources[0].clone();
    missing.key = "missing".into();
    missing.options.cache = a.root.join("missing-index");
    assert_eq!(
        session_catalog_union(&[sources[0].clone(), missing], "workspace-a", "limit=3").status,
        503
    );
    let many: Vec<_> = (0..65).map(|_| sources[0].clone()).collect();
    assert_eq!(session_catalog_union(&many, "workspace-a", "").status, 503);
}

#[test]
fn workspace_union_cursor_generation_snapshot_fails_closed_on_change_or_missing_store() {
    let a = Fixture::new();
    a.publish(8);
    let sources = [crate::OverviewSource {
        key: "a".into(),
        label: "Alpha".into(),
        options: a.options.clone(),
        up: false,
    }];
    let first = session_catalog_union(&sources, "workspace", "limit=3");
    assert_eq!(
        first.status,
        200,
        "{}",
        String::from_utf8_lossy(&first.body)
    );
    let first: Value = serde_json::from_slice(&first.body).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let query = format!("limit=3&cursor={}", crate::viewer::percent_encode(&cursor));

    let matching = session_catalog_union(&sources, "workspace", &query);
    assert_eq!(
        matching.status,
        200,
        "{}",
        String::from_utf8_lossy(&matching.body)
    );

    a.source("new-after-cursor");
    a.publish_existing();
    assert_eq!(
        session_catalog_union(&sources, "workspace", &query).status,
        409,
        "a changed Catalog generation must refuse the old cursor"
    );

    let mut missing = sources[0].clone();
    missing.options.cache = a.root.join("missing-index.json");
    assert_eq!(
        session_catalog_union(&[missing], "workspace", &query).status,
        503,
        "a missing configured store must fail closed during generation lookup"
    );
}

fn cursor_query(cursor: &Value) -> String {
    format!(
        "cursor={}",
        crate::viewer::percent_encode(cursor.as_str().unwrap())
    )
}

fn native_refresh(fixture: &Fixture) -> Value {
    let sources = std::sync::Arc::new(crate::SessionSources::with_machines(vec![(
        String::new(),
        fixture.options.clone(),
    )]));
    let result = crate::Query::from_sources(sources.clone())
        .call("list_sessions", &json!({}))
        .unwrap();
    sources.close();
    result
}

fn indexed_rows(connection: &Connection, path: &str) -> Vec<Vec<SqlValue>> {
    let id: i64 = connection
        .query_row("SELECT file_id FROM files WHERE path=?1", [path], |row| {
            row.get(0)
        })
        .unwrap();
    let mut all = Vec::new();
    for table in [
        "files",
        "events",
        "signals",
        "usage_identity",
        "usage",
        "codex_usage",
    ] {
        let mut statement = connection
            .prepare(&format!(
                "SELECT * FROM {table} WHERE file_id=?1 ORDER BY 1,2"
            ))
            .unwrap();
        let columns = statement.column_count();
        all.extend(
            statement
                .query_map([id], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<Vec<SqlValue>>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        );
    }
    all
}

#[test]
fn archived_event_index_survives_absence_without_current_membership() {
    let fixture = Fixture::new();
    fixture.publish(2);
    let relative = "projects/project/session-00000.jsonl";
    let path = fixture.options.claude_home.join(relative);
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    for line in [
        json!({"type":"assistant","uuid":"indexed-tool","timestamp":"2026-10-01T00:00:01Z","message":{"id":"indexed-message","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":2},"content":[{"type":"tool_use","id":"retained-call","name":"Bash","input":{"command":"echo private-native-body-synthetic"}}]}}),
        json!({"type":"user","uuid":"indexed-result","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"retained-call","content":"private-native-result-synthetic"}]}}),
    ] {
        writeln!(file, "{line}").unwrap();
    }
    drop(file);
    native_refresh(&fixture);
    let proof = session_source_proof(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let connection = fixture.db();
    let before = indexed_rows(&connection, path.to_str().unwrap());
    assert!(
        before.len() >= 5,
        "fixture includes event and token metadata"
    );
    assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    crate::retain_source_event_index(&fixture.options, &proof, &ready).unwrap();
    assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    let marker: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key GLOB 'archive_event_index:*'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!marker.contains("private-native-body-synthetic"));
    assert!(!marker.contains("private-native-result-synthetic"));
    fs::remove_file(&path).unwrap();
    fs::remove_file(
        fixture
            .options
            .claude_home
            .join("projects/project/session-00001.jsonl"),
    )
    .unwrap();
    for _ in 0..2 {
        assert_eq!(native_refresh(&fixture)["sessions"], json!([]));
        assert_eq!(indexed_rows(&connection, path.to_str().unwrap()), before);
        let count: i64 = connection
            .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "ordinary disappearance still removes unmarked indexes"
        );
        assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    }
    let retained = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    assert_eq!(retained.lifecycle, "retained");
    assert_eq!(retained.source, ready.source);
    assert_eq!(fixture.body("").1["items"], json!([]));
}

#[test]
fn archive_eviction_refuses_changed_or_missing_native_byte_proofs() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let proof = session_source_proof(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    crate::retain_source_event_index(&fixture.options, &proof, &ready).unwrap();
    let connection = fixture.db();
    let mut statement = connection
        .prepare("SELECT key,value FROM meta WHERE key GLOB 'native_source_chunks:*'")
        .unwrap();
    let records: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(records.len(), 2, "one proof header and one hash page");
    for (key, encoded) in records {
        connection
            .execute("UPDATE meta SET value=value || ' ' WHERE key=?1", [&key])
            .unwrap();
        if key.ends_with("header") {
            assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
        } else {
            // A differently encoded but identical page remains covered by
            // the bound header's digest fingerprint.
            assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
        }
        connection
            .execute("DELETE FROM meta WHERE key=?1", [&key])
            .unwrap();
        assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
        assert_eq!(
            crate::retain_source_event_index(&fixture.options, &proof, &ready),
            Err(CatalogIdentityError::ScopeChanged)
        );
        connection
            .execute(
                "INSERT INTO meta VALUES (semon_meta_scope(?1),?1,?2)",
                rusqlite::params![key, encoded],
            )
            .unwrap();
        assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    }
}

#[test]
fn archive_eviction_binds_parent_marker_metadata_and_preserves_prior_rows() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let path = fixture.options.claude_home.join(relative);
    let proof = session_source_proof(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    crate::retain_source_event_index(&fixture.options, &proof, &ready).unwrap();
    let connection = fixture.db();
    let key = format!("native_parent_marker:{}", path.display());
    let encoded: String = connection
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |row| {
            row.get(0)
        })
        .unwrap();
    connection
        .execute("UPDATE meta SET value=value || ' ' WHERE key=?1", [&key])
        .unwrap();
    assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    connection
        .execute("DELETE FROM meta WHERE key=?1", [&key])
        .unwrap();
    assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::ScopeChanged)
    );
    connection
        .execute(
            "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
            rusqlite::params![key, encoded],
        )
        .unwrap();
    assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    let archive_key = format!("archive_event_index:{}", path.display());
    let encoded: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [&archive_key],
            |row| row.get(0),
        )
        .unwrap();
    let mut marker: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(marker["version"], 6);
    marker["version"] = json!(4);
    marker
        .as_object_mut()
        .unwrap()
        .remove("parent_marker_sha256");
    marker
        .as_object_mut()
        .unwrap()
        .remove("launch_calls_sha256");
    connection
        .execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            rusqlite::params![archive_key, marker.to_string()],
        )
        .unwrap();
    let unsupported = crate::native_timing::cleanup_tests::snapshot(&connection);
    assert_eq!(
        crate::source_event_index_retained(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::Unavailable)
    );
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::Unavailable)
    );
    // A writable producer now discards obsolete derived contracts; these
    // assertions exercise the focused custody boundary before that rebuild.
    assert_eq!(
        crate::native_timing::cleanup_tests::snapshot(&connection),
        unsupported
    );
    // Restore the current fixture to exercise ordinary retained custody.
    connection
        .execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            rusqlite::params![archive_key, encoded],
        )
        .unwrap();
    assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    let before = indexed_rows(&connection, path.to_str().unwrap());
    fs::remove_file(&path).unwrap();
    for _ in 0..2 {
        assert_eq!(native_refresh(&fixture)["sessions"], json!([]));
        assert_eq!(indexed_rows(&connection, path.to_str().unwrap()), before);
        let count: i64 = connection
            .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "current custody preserves the existing index");
    }
}

#[test]
fn archived_event_index_marker_does_not_cover_a_new_native_generation() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let path = fixture.options.claude_home.join(relative);
    let proof = session_source_proof(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    crate::retain_source_event_index(&fixture.options, &proof, &ready).unwrap();
    use std::io::Write;
    writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}", json!({"type":"user","uuid":"new-prefix","message":{"role":"user","content":"new generation"}})).unwrap();
    native_refresh(&fixture);
    assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::ScopeChanged)
    );
    let connection = fixture.db();
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM meta WHERE key GLOB 'archive_event_index:*'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    fs::remove_file(path).unwrap();
    native_refresh(&fixture);
    let count: i64 = connection
        .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn archive_eviction_binds_native_launch_metadata_and_preserves_prior_rows() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let path = fixture.options.claude_home.join(relative);
    let proof = session_source_proof(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    crate::retain_source_event_index(&fixture.options, &proof, &ready).unwrap();
    let connection = fixture.db();
    let key = format!(
        "native_launch_calls:{:x}:header",
        Sha256::digest(path.to_str().unwrap().as_bytes())
    );
    let encoded: String = connection
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |row| {
            row.get(0)
        })
        .unwrap();
    connection
        .execute("UPDATE meta SET value=value || ' ' WHERE key=?1", [&key])
        .unwrap();
    assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    connection
        .execute("DELETE FROM meta WHERE key=?1", [&key])
        .unwrap();
    assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::ScopeChanged)
    );
    connection
        .execute(
            "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2)",
            rusqlite::params![key, encoded],
        )
        .unwrap();
    assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    let archive_key = format!("archive_event_index:{}", path.display());
    let encoded: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [&archive_key],
            |row| row.get(0),
        )
        .unwrap();
    let mut marker: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(marker["version"], 6);
    marker["version"] = json!(5);
    marker
        .as_object_mut()
        .unwrap()
        .remove("launch_calls_sha256");
    connection
        .execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            rusqlite::params![archive_key, marker.to_string()],
        )
        .unwrap();
    let unsupported = crate::native_timing::cleanup_tests::snapshot(&connection);
    assert_eq!(
        crate::source_event_index_retained(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::Unavailable)
    );
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::Unavailable)
    );
    // A writable producer now discards obsolete derived contracts; these
    // assertions exercise the focused custody boundary before that rebuild.
    assert_eq!(
        crate::native_timing::cleanup_tests::snapshot(&connection),
        unsupported
    );
    connection
        .execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            rusqlite::params![archive_key, encoded],
        )
        .unwrap();
    assert!(crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
    let before = indexed_rows(&connection, path.to_str().unwrap());
    fs::remove_file(&path).unwrap();
    for _ in 0..2 {
        assert_eq!(native_refresh(&fixture)["sessions"], json!([]));
        assert_eq!(indexed_rows(&connection, path.to_str().unwrap()), before);
        assert_eq!(
            connection
                .query_row::<i64, _, _>("SELECT count(*) FROM files", [], |row| row.get(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn archived_event_index_refuses_mixed_projection_and_ledger_proofs() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let proof = session_source_proof(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let mut changed = proof.clone();
    changed.ino = changed.ino.wrapping_add(1);
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &changed, &ready),
        Err(CatalogIdentityError::ScopeChanged)
    );
    let mut changed = ready.clone();
    changed.projection_generation = "0".repeat(64);
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &changed),
        Err(CatalogIdentityError::ScopeChanged)
    );
    let mut changed = proof.clone();
    changed.source.path = "../outside.jsonl".into();
    let mut projection = ready.clone();
    projection.source = changed.source.clone();
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &changed, &projection),
        Err(CatalogIdentityError::InvalidArguments)
    );
    let missing = Fixture::new();
    assert_eq!(
        crate::retain_source_event_index(&missing.options, &proof, &ready),
        Err(CatalogIdentityError::Unavailable)
    );
    assert!(!EventCache::path(&missing.options.cache).exists());
    assert!(!crate::source_event_index_retained(&fixture.options, &proof, &ready).unwrap());
}

#[test]
fn archived_event_index_requires_decodable_rows_and_supported_parser() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let proof = session_source_proof(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let connection = fixture.db();
    connection
        .execute("UPDATE files SET pending='malformed'", [])
        .unwrap();
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::Unavailable)
    );
    connection
        .execute("UPDATE files SET pending='{}'", [])
        .unwrap();
    connection
        .execute("UPDATE meta SET value='0' WHERE key='cache_version'", [])
        .unwrap();
    let unsupported = crate::native_timing::cleanup_tests::snapshot(&connection);
    assert_eq!(
        crate::retain_source_event_index(&fixture.options, &proof, &ready),
        Err(CatalogIdentityError::Unavailable)
    );
    assert!(EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).is_err());
    assert_eq!(
        crate::native_timing::cleanup_tests::snapshot(&connection),
        unsupported
    );
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM meta WHERE key GLOB 'archive_event_index:*'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn selected_claude_parent_keeps_codex_unknown_and_idle_child_ranges_readable() {
    use std::io::Write;
    for completed in [false, true] {
        let fixture = Fixture::new();
        let parent = fixture.source("parent");
        writeln!(fs::OpenOptions::new().append(true).open(parent).unwrap(), "{}", json!({
                "type":"assistant","sessionId":"parent","timestamp":"2026-10-01T00:01:00Z",
                "message":{"id":"call","role":"assistant","content":[{"type":"tool_use","id":"launch","name":"Bash","input":{"command":"codex exec"}}]}
            })).unwrap();
        let relative = fixture.codex_source("child", None, 1);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(fixture.options.codex_home.join(&relative))
            .unwrap();
        writeln!(file, "{}", json!({"type":"response_item","timestamp":"2026-10-01T00:02:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Semon-Parent: claude:parent:launch\nInspect child"}]}})).unwrap();
        if completed {
            for kind in ["task_started", "task_complete"] {
                writeln!(file, "{}", json!({"type":"event_msg","timestamp":"2026-10-01T00:03:00Z","payload":{"type":kind,"turn_id":"owned"}})).unwrap();
            }
        }
        drop(file);
        let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
        facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative]));
        facts.codex_locks.insert("child".into(), 42);
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();
        fixture.publish_existing();
        let range = crate::transcript_range::read(
            &fixture.options,
            "source",
            "sid=parent&scope=retained_history&limit=100",
            None,
            crate::SessionSourceReadMode::ProviderOnly,
        )
        .unwrap_or_else(|error| panic!("source-shaped selected parent failed: {error}"));
        assert_eq!(
            range.status,
            200,
            "{}",
            String::from_utf8_lossy(&range.body)
        );
        let body: Value = serde_json::from_slice(&range.body).unwrap();
        assert!(!body["entries"].as_array().unwrap().is_empty());
        let edge = body["relationship_context"]["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["to"] == "child")
            .unwrap();
        assert_eq!(edge["status"], if completed { "idle" } else { "off" });
        assert_eq!(body["session"]["harness"], "claude");
    }
}

#[test]
fn selected_details_are_body_free_exact_and_bound_to_the_original_source_generation() {
    let fixture = Fixture::new();
    fixture.publish(2);
    let (status, list) = fixture.body("limit=2&scope=retained_history");
    assert_eq!(status, 200);
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("details").is_none())
    );
    let (status, selected) = fixture.body("sid=session-00000&scope=retained_history");
    assert_eq!(status, 200);
    let details = &selected["items"][0]["details"];
    assert_eq!(details["turns"], 1);
    assert_eq!(details["copied_usage"], false);
    assert_eq!(details["tokens_by_model"], json!({}));
    assert_eq!(details["rate_limits"], Value::Null);
    assert_eq!(details["lane"], true);
    assert_eq!(details["role"], false);
    assert!(details["busy"].is_array());
    assert_eq!(details["content_generation"].as_str().unwrap().len(), 64);
    assert!(!details.to_string().contains("synthetic prompt"));
    let transcript = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=session-00000&scope=retained_history&limit=1",
        None,
    );
    assert_eq!(transcript.status, 200);
    let transcript: Value = serde_json::from_slice(&transcript.body).unwrap();
    assert_eq!(transcript["session"]["details"], *details);
    assert_eq!(
        selected["items"][0]["transcript"]["generation"],
        transcript["projection"]["generation"]
    );
    assert_eq!(
        selected["items"][0]["transcript"]["total"],
        transcript["projection"]["total"]
    );
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("transcript").is_none())
    );
    assert_eq!(
        transcript["relationship_context"]["turns"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        transcript["relationship_context"]["turns"][0]["id"],
        transcript["entries"][0]["turn"]
    );
    // Let the ordinary observer withdraw the disappeared source and retain its
    // published metadata, rather than bypassing the guarded Catalog writer.
    fs::remove_file(
        fixture
            .options
            .claude_home
            .join("projects/project/session-00000.jsonl"),
    )
    .unwrap();
    fixture.publish_existing();
    assert_eq!(
        fixture.body("sid=session-00000&scope=retained_history").1["items"][0]["details"]["live_observation"],
        false,
        "retained metadata cannot claim current activity"
    );
    // Publish changed provenance without a replacement trace. The existing
    // trace must not supply details for this new source cohort.
    let db = fixture.db();
    let mut row: crate::model::summary::CatalogRow = serde_json::from_str(
        &db.query_row("SELECT metadata FROM session_catalog WHERE namespace=semon_namespace() AND session_key='session-00000'", [], |r| r.get::<_, String>(0)).unwrap(),
    ).unwrap();
    row.sources.clear();
    row.trace = None;
    let config = fixture
        .options
        .source
        .config(&fixture.options.cache.with_extension("sqlite3"))
        .unwrap();
    let owner = crate::events::store::configured::NativeOwnership::restore(&config).unwrap();
    let writer = crate::events::store::configured::Writer::owned(owner).unwrap();
    let generation = writer.catalog_generation().unwrap();
    let rows = [row];
    assert_eq!(
        writer
            .publish_catalog(
                &crate::slot_projection::Publication {
                    rows: &rows,
                    transcripts: None
                },
                generation.as_deref(),
                None,
                false,
            )
            .unwrap()
            .unwrap()
            .0,
        crate::events::Outcome::Written
    );
    let (status, changed) = fixture.body("sid=session-00000&scope=retained_history");
    assert_eq!(status, 200);
    assert!(
        changed["items"][0].get("details").is_none(),
        "changed native provenance must not supply recorded details"
    );
    assert_eq!(
        fixture.body("sid=session-00001&scope=retained_history").1["items"][0]["details"]["turns"],
        1
    );
}

#[test]
fn home_native_previews_preserve_question_answers_and_archive_generation_without_caching_text() {
    let fixture = Fixture::new();
    let path = fixture.source("home");
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file,"{}",json!({"type":"assistant","sessionId":"home","uuid":"question","parentUuid":"home","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"ask-native","name":"AskUserQuestion","input":{"questions":[{"question":"NATIVE_PRIVATE_QUESTION","options":[{"label":"Proceed"}]}]}}]}})).unwrap();
    writeln!(file,"{}",json!({"type":"user","sessionId":"home","uuid":"answer","parentUuid":"question","timestamp":"2026-10-01T00:00:02Z","toolUseResult":{"answers":{"NATIVE_PRIVATE_QUESTION":"NATIVE_PRIVATE_ANSWER"}},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"ask-native","content":"User has answered your questions"}]}})).unwrap();
    drop(file);
    fixture.publish(1);
    let source = crate::OverviewSource {
        key: "source".into(),
        label: "Machine".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let output: Value = serde_json::from_slice(
        &crate::session_overview_reply(&[source], "home", "", 1790812810000)
            .unwrap()
            .body,
    )
    .unwrap();
    let row = output["answered"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["event"]["ask"] == "question")
        .unwrap();
    let question_id = row["event"]["id"].as_str().unwrap();
    assert_eq!(row["event"]["status"], "done");
    assert_eq!(row["event"]["at"], 1790812801000_i64);
    assert_eq!(row["event"]["done"], 1790812802000_i64);
    assert_eq!(row["event"]["brief"], "");
    let trace_query = format!(
        "sid=home&scope=retained_history&turn={}",
        row["event"]["turn"].as_str().unwrap()
    );
    let trace = crate::session_causal_trace(&fixture.options, "source", &trace_query);
    assert_eq!(
        trace.status,
        200,
        "{}",
        String::from_utf8_lossy(&trace.body)
    );
    let trace_body: Value = serde_json::from_slice(&trace.body).unwrap();
    assert!(
        trace_body["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["id"] == question_id)
    );
    assert!(!String::from_utf8_lossy(&trace.body).contains("NATIVE_PRIVATE_"));
    assert_eq!(
        crate::session_causal_trace(
            &fixture.options,
            "source",
            &trace_query.replace("retained_history", "current")
        )
        .status,
        400
    );
    assert!(
        row["session"]["content"].is_null(),
        "private recipes must stay server-side"
    );
    let query = format!(
        "sid=home&scope=retained_history&generation={}&events={}",
        row["session"]["content_generation"].as_str().unwrap(),
        crate::viewer::percent_encode(&json!([question_id]).to_string())
    );
    let read = crate::session_home_details_with_mode(
        &fixture.options,
        "source",
        &query,
        None,
        crate::SessionSourceReadMode::ProviderOnly,
    );
    assert_eq!(read.status, 200, "{}", String::from_utf8_lossy(&read.body));
    let details: Value = serde_json::from_slice(&read.body).unwrap();
    assert_eq!(
        details["events"][question_id]["brief"],
        "NATIVE_PRIVATE_QUESTION"
    );
    assert_eq!(
        details["events"][question_id]["answer"],
        json!(["NATIVE_PRIVATE_ANSWER"])
    );
    let db = fixture.db();
    let saved: String = db
        .query_row(
            "SELECT summary FROM session_overview WHERE session_key='home'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!saved.contains("NATIVE_PRIVATE_"));
    struct Reader(Vec<u8>);
    impl crate::SessionSourceReader for Reader {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            assert!(max <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert_eq!(source.native_id, "home");
            assert!(expected.is_none_or(|value| value == "immutable-original"));
            let end = (offset as usize + max).min(self.0.len());
            Ok(crate::SessionSourceRange {
                generation: "immutable-original".into(),
                length: self.0.len() as u64,
                offset,
                bytes: std::sync::Arc::from(&self.0[offset as usize..end]),
                cached: false,
            })
        }
    }
    let reader = Reader(fs::read(&path).unwrap());
    fs::remove_file(&path).unwrap();
    let archived_trace = crate::session_causal_trace(&fixture.options, "source", &trace_query);
    assert_eq!(archived_trace.status, 200);
    let archived_trace: Value = serde_json::from_slice(&archived_trace.body).unwrap();
    assert_eq!(archived_trace["turns"], trace_body["turns"]);
    assert_eq!(archived_trace["handoffs"], trace_body["handoffs"]);
    let archived = crate::session_home_details_with_mode(
        &fixture.options,
        "source",
        &query,
        Some(&reader),
        crate::SessionSourceReadMode::LocalThenProvider,
    );
    assert_eq!(
        archived.status,
        200,
        "{}",
        String::from_utf8_lossy(&archived.body)
    );
    let archived: Value = serde_json::from_slice(&archived.body).unwrap();
    for field in ["api", "generation", "session", "events", "incomplete"] {
        assert_eq!(archived[field], details[field], "{field}");
    }
    for field in [
        "source_key",
        "read_scope",
        "catalog_key",
        "harness",
        "native_id",
        "native_ids",
        "generation",
    ] {
        assert_eq!(
            archived["identity"][field], details["identity"][field],
            "{field}"
        );
    }
    assert_eq!(
        archived["identity"]["source_refs"][0]["source"],
        details["identity"]["source_refs"][0]["source"]
    );
    assert_eq!(
        archived["identity"]["source_refs"][0]["state"],
        "unavailable"
    );
    assert!(!path.exists());
    // A missing current overview must be repairable after native eviction,
    // using the retained selected source provenance.
    let metadata: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='home'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut selected: crate::model::summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
    for source in &mut selected.sources {
        source.native_role = None;
    }
    db.execute(
        "UPDATE session_catalog SET metadata=?1 WHERE session_key='home'",
        [serde_json::to_string(&selected).unwrap()],
    )
    .unwrap();
    db.execute(
        "UPDATE session_slot_projections SET source_generation=?1 WHERE session_key='home'",
        [crate::retention::source_generation(&selected.sources).unwrap()],
    )
    .unwrap();
    db.execute_batch("DELETE FROM session_overview; DELETE FROM session_trace_headers; DELETE FROM session_trace_turns; DELETE FROM session_trace_edges; INSERT OR REPLACE INTO meta SELECT namespace,'overview_needed:'||session_key,'1' FROM session_catalog;").unwrap();
    let migrated =
        crate::events::EventCache::open_scoped(&fixture.options.cache, &fixture.options.source)
            .unwrap();
    drop(migrated);
    assert!(
        crate::overview_repair_candidates(&fixture.options, 8)
            .unwrap()
            .contains(&"home".into())
    );
    assert!(
        crate::repair_session_overview(
            &fixture.options,
            "source",
            "home",
            Some(&reader),
            crate::SessionSourceReadMode::LocalThenProvider
        )
        .unwrap()
    );
    assert!(
        !crate::overview_repair_candidates(&fixture.options, 8)
            .unwrap()
            .contains(&"home".into())
    );
    let repaired_trace = crate::session_causal_trace(&fixture.options, "source", &trace_query);
    assert_eq!(
        repaired_trace.status,
        200,
        "{}",
        String::from_utf8_lossy(&repaired_trace.body)
    );
    let repaired: Value = serde_json::from_slice(&repaired_trace.body).unwrap();
    assert_eq!(repaired["turns"], trace_body["turns"]);
    assert_eq!(repaired["handoffs"], trace_body["handoffs"]);
    let updated = query.replace(
        row["session"]["content_generation"].as_str().unwrap(),
        repaired["previews"]["home"].as_str().unwrap(),
    );
    let recovered = crate::session_home_details_with_mode(
        &fixture.options,
        "source",
        &updated,
        Some(&reader),
        crate::SessionSourceReadMode::LocalThenProvider,
    );
    assert_eq!(
        recovered.status,
        200,
        "{}",
        String::from_utf8_lossy(&recovered.body)
    );
    let recovered: Value = serde_json::from_slice(&recovered.body).unwrap();
    assert_eq!(recovered["events"], details["events"]);
    assert_eq!(recovered["incomplete"], false);
    assert!(!path.exists());
    // A replacement observed at the same path and offsets cannot borrow
    // the old publication's private recipes, even before its new overview
    // is published. Reject before issuing any provider reads.
    db.execute("UPDATE session_catalog SET metadata=json_set(metadata,'$.sources[0].prefix_sha256',json(?1)) WHERE session_key='home'", [json!(vec![9;32]).to_string()]).unwrap();
    assert_eq!(
        crate::session_home_details_with_mode(
            &fixture.options,
            "source",
            &updated,
            Some(&reader),
            crate::SessionSourceReadMode::ProviderOnly
        )
        .status,
        409
    );
}

#[test]
fn archived_overview_repair_reuses_exact_indexes_for_large_native_sources() {
    use std::io::Write;
    let fixture = Fixture::new();
    let path = fixture.source("large-original");
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    let padding = "x".repeat(14 * 1024 * 1024);
    for _ in 0..5 {
        writeln!(file, "{}", json!({"type":"system","padding":padding})).unwrap();
    }
    drop(file);
    fixture.publish(1);
    let original = fs::read(&path).unwrap();
    assert!(original.len() > 64 * 1024 * 1024);
    struct Reader {
        bytes: Vec<u8>,
        read: std::sync::atomic::AtomicUsize,
    }
    impl crate::SessionSourceReader for Reader {
        fn read_range(
            &self,
            _: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            assert!(expected.is_none_or(|generation| generation == "large-original"));
            let end = (offset as usize + max).min(self.bytes.len());
            self.read
                .fetch_add(end - offset as usize, std::sync::atomic::Ordering::Relaxed);
            Ok(crate::SessionSourceRange {
                generation: "large-original".into(),
                length: self.bytes.len() as u64,
                offset,
                bytes: std::sync::Arc::from(&self.bytes[offset as usize..end]),
                cached: false,
            })
        }
    }
    let reader = Reader {
        bytes: original,
        read: std::sync::atomic::AtomicUsize::new(0),
    };
    fs::remove_file(&path).unwrap();
    let db = fixture.db();
    db.execute_batch("DELETE FROM session_overview; DELETE FROM session_trace_headers; DELETE FROM session_trace_turns; DELETE FROM session_trace_edges; INSERT OR REPLACE INTO meta SELECT namespace,'overview_needed:'||session_key,'1' FROM session_catalog;").unwrap();
    drop(EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap());
    let refs = crate::overview_repair_sources(&fixture.options, "large-original")
        .unwrap()
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert!(refs[0].offset > 64 * 1024 * 1024);
    assert!(
        crate::repair_session_overview(
            &fixture.options,
            "source",
            "large-original",
            Some(&reader),
            crate::SessionSourceReadMode::ProviderOnly
        )
        .unwrap()
    );
    assert!(
        reader.read.load(std::sync::atomic::Ordering::Relaxed) < 65536,
        "metadata migration must not copy unrelated large native bodies"
    );
    assert!(
        !path.exists(),
        "selected migration cannot restore native files"
    );
    let (_, selected) = fixture.body("sid=large-original&scope=retained_history");
    assert_eq!(selected["items"][0]["details"]["turns"], 1);
}

#[test]
fn large_families_repair_one_owner_without_replacing_related_publications() {
    let fixture = Fixture::new();
    fixture.source("root");
    for i in 0..80 {
        fixture.source(&format!("run-{i:03}"));
    }
    fixture.publish(0);
    let db = fixture.db();
    db.execute("UPDATE session_catalog SET parent_key='root',metadata=json_set(metadata,'$.parent','root') WHERE session_key LIKE 'run-%'",[]).unwrap();
    db.execute_batch("DELETE FROM session_overview; DELETE FROM session_trace_headers; DELETE FROM session_trace_turns; DELETE FROM session_trace_edges; INSERT OR REPLACE INTO meta SELECT namespace,'overview_needed:'||session_key,'1' FROM session_catalog;").unwrap();
    drop(EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap());
    let refs = crate::overview_repair_sources(&fixture.options, "root")
        .unwrap()
        .unwrap();
    assert_eq!(refs.len(), 64);
    assert!(
        crate::repair_session_overview(
            &fixture.options,
            "source",
            "root",
            None,
            crate::SessionSourceReadMode::LocalThenProvider
        )
        .unwrap()
    );
    let header: String = db
        .query_row(
            "SELECT metadata FROM session_trace_headers WHERE session_key='root'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&header).unwrap()["relationships_incomplete"],
        true
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM session_trace_headers", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1,
        "selected migration cannot publish its partial related owners"
    );
    assert!(
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='overview_needed:run-079')",
            [],
            |row| row.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert!(
        crate::repair_session_overview(
            &fixture.options,
            "source",
            "run-079",
            None,
            crate::SessionSourceReadMode::LocalThenProvider
        )
        .unwrap()
    );
    assert_eq!(
        db.query_row(
            "SELECT metadata FROM session_trace_headers WHERE session_key='root'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        header,
        "a child's narrow closure cannot overwrite its parent's wider publication"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM session_catalog", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        81
    );
}
#[test]
fn exact_model_and_missing_repository_filters_bind_cursor_scope() {
    let fixture = Fixture::new();
    fixture.publish(4);
    let db = fixture.db();
    db.execute("UPDATE session_catalog SET repo=NULL, metadata=json_set(metadata,'$.repo',NULL,'$.model','exact') WHERE session_key IN ('session-00000','session-00002')", []).unwrap();
    db.execute("UPDATE session_catalog SET metadata=json_set(metadata,'$.model','exact-other') WHERE session_key='session-00001'", []).unwrap();
    let response = session_catalog_page(
        &fixture.options,
        "source",
        "scope=retained_history&model=exact&no_repo=1&limit=1",
    );
    assert_eq!(response.status, 200);
    let value: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(value["items"].as_array().unwrap().len(), 1);
    assert_eq!(value["items"][0]["key"], "session-00000");
    assert!(value["items"][0]["repo"].is_null());
    let query = format!(
        "scope=retained_history&model=exact&no_repo=1&limit=1&{}",
        cursor_query(&value["next_cursor"])
    );
    let next: Value =
        serde_json::from_slice(&session_catalog_page(&fixture.options, "source", &query).body)
            .unwrap();
    assert_eq!(next["items"][0]["key"], "session-00002");
    assert_eq!(
        session_catalog_page(
            &fixture.options,
            "source",
            &query.replace("model=exact", "model=exact-other")
        )
        .status,
        400
    );
    assert_eq!(
        session_catalog_page(&fixture.options, "source", "no_repo=1&repo=project").status,
        400
    );
}
#[test]
fn annotation_pages_locate_unloaded_failures_without_reading_native_bodies() {
    let fixture = Fixture::new();
    let path = fixture.source("faults");
    let mut lines = fs::read_to_string(&path).unwrap();
    for index in 0..206 {
        lines.push_str(&format!("{}\n{}\n", json!({"type":"assistant","sessionId":"faults","uuid":format!("call-{index}"),"timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":format!("tool-{index}"),"name":"Bash","input":{"command":"synthetic failed call"}}]}}), json!({"type":"user","sessionId":"faults","uuid":format!("reply-{index}"),"timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("tool-{index}"),"is_error":true,"content":"failed synthetic output"}]}})));
    }
    fs::write(&path, lines).unwrap();
    fixture.publish_existing();
    fs::remove_file(path).unwrap();
    let query = "sid=faults&scope=retained_history&kind=errors&limit=100";
    let response = crate::session_annotations(&fixture.options, "source", query);
    assert_eq!(response.status, 200);
    let first: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(first["total"], 206);
    assert_eq!(first["entries"].as_array().unwrap().len(), 100);
    assert_eq!(first["next_cursor"], 100);
    assert!(
        !response
            .body
            .windows(23)
            .any(|bytes| bytes == b"failed synthetic output")
    );
    let second_query = format!(
        "{query}&after=100&generation={}",
        first["generation"].as_str().unwrap()
    );
    let second: Value = serde_json::from_slice(
        &crate::session_annotations(&fixture.options, "source", &second_query).body,
    )
    .unwrap();
    assert_eq!(second["entries"].as_array().unwrap().len(), 100);
    assert!(
        second["entries"][0]["slot"].as_u64().unwrap()
            > first["entries"][99]["slot"].as_u64().unwrap()
    );
    assert_eq!(
        crate::session_annotations(
            &fixture.options,
            "source",
            &(query.to_owned() + "&after=100")
        )
        .status,
        400
    );
    assert_eq!(
        crate::session_annotations(
            &fixture.options,
            "source",
            &(query.to_owned() + "&generation=wrong")
        )
        .status,
        409
    );
    assert_eq!(
        crate::session_annotations(
            &fixture.options,
            "source",
            &(query.to_owned() + "&kind=signals")
        )
        .status,
        400
    );
}
#[test]
fn turn_bookmarks_use_indexed_retained_slots_without_native_body_reads() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let db = fixture.db();
    let turn: String = db.query_row("SELECT json_extract(metadata,'$.turn') FROM session_slots WHERE session_key='session-00000' AND json_extract(metadata,'$.turn') IS NOT NULL ORDER BY slot LIMIT 1", [], |row| row.get(0)).unwrap();
    let expected: i64 = db.query_row("SELECT slot FROM session_slots WHERE session_key='session-00000' AND json_extract(metadata,'$.turn')=?1 ORDER BY slot LIMIT 1", [&turn], |row| row.get(0)).unwrap();
    let range = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=session-00000&scope=retained_history&after=0&limit=1",
        None,
    );
    assert_eq!(range.status, 200);
    let range: Value = serde_json::from_slice(&range.body).unwrap();
    fs::remove_file(
        fixture
            .options
            .claude_home
            .join("projects/project/session-00000.jsonl"),
    )
    .unwrap();
    let query = format!(
        "sid=session-00000&turn={}&scope=retained_history",
        crate::viewer::percent_encode(&turn)
    );
    let reply = crate::session_turn_anchor(&fixture.options, "source", &query);
    assert_eq!(reply.status, 200);
    let value: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(value["slot"], expected);
    assert_eq!(value["source_key"], "source");
    assert_eq!(value["generation"], range["projection"]["generation"]);
    // A turn address is qualified by the same immutable projection as a
    // range. Concurrent replacement rejects the old address before I/O.
    db.execute(
        "UPDATE session_slot_projections SET generation=?1 WHERE session_key='session-00000'",
        ["e".repeat(64)],
    )
    .unwrap();
    let stale_range = format!(
        "sid=session-00000&scope=retained_history&after={expected}&limit=1&generation={}",
        value["generation"].as_str().unwrap()
    );
    assert_eq!(
        crate::session_transcript_range(&fixture.options, "source", &stale_range, None).status,
        409
    );
    assert_eq!(
        crate::session_turn_anchor(&fixture.options, "source", &(query.clone() + "&turn=other"))
            .status,
        400
    );
    assert_eq!(
        crate::session_turn_anchor(
            &fixture.options,
            "source",
            &query.replace("retained_history", "current")
        )
        .status,
        400
    );
    db.execute(
        "UPDATE meta SET value='newer' WHERE key='slot_projection_catalog_generation'",
        [],
    )
    .unwrap();
    assert_eq!(
        crate::session_turn_anchor(&fixture.options, "source", &query).status,
        409
    );
}
#[test]
fn exact_source_proof_requires_current_native_mapping_and_consumed_ledger() {
    let fixture = Fixture::new();
    fixture.publish(100);
    let path = "projects/project/session-00000.jsonl";
    let proof = session_source_proof(&fixture.options, "claude", path)
        .unwrap()
        .unwrap();
    assert_eq!(proof.source.native_id, "session-00000");
    assert_eq!(proof.source.offset, proof.size);
    assert_eq!(proof.freshness.state, "cached");
    assert!(
        session_source_proof(&fixture.options, "claude", "projects/project/missing.jsonl")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        session_source_proof(&fixture.options, "claude", "../bad.jsonl"),
        Err(CatalogIdentityError::InvalidArguments)
    );
    let connection = fixture.db();
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT session_key FROM session_catalog_sources WHERE source_path=?1 ORDER BY session_key LIMIT 1",[fixture.options.claude_home.join(path).to_str().unwrap()],|row|row.get(3)).unwrap();
    assert!(plan.contains("session_catalog_source_path"), "{plan}");
    // Ledger progress without catalog publication must not reuse its former
    // native mapping or manufacture a proof for a new consumed boundary.
    connection
        .execute(
            "UPDATE files SET resume_at=resume_at-1 WHERE path=?1",
            [fixture.options.claude_home.join(path).to_str().unwrap()],
        )
        .unwrap();
    assert_eq!(
        session_source_proof(&fixture.options, "claude", path),
        Err(CatalogIdentityError::Unavailable)
    );
    connection
        .execute(
            "UPDATE files SET resume_at=resume_at+1 WHERE path=?1",
            [fixture.options.claude_home.join(path).to_str().unwrap()],
        )
        .unwrap();
    fs::remove_file(fixture.options.facts.as_ref().unwrap()).unwrap();
    assert_eq!(
        session_source_proof(&fixture.options, "claude", path),
        Err(CatalogIdentityError::Unavailable)
    );
}

#[test]
fn current_catalog_index_work_is_constant_with_unrelated_retained_history() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let connection = fixture.db();
    let query = "SELECT metadata FROM session_catalog INDEXED BY session_catalog_current_harness_repo WHERE lifecycle='current' AND harness='claude' AND repo='project' ORDER BY last_ms DESC,session_key ASC LIMIT 3";
    let measure = || {
        let mut statement = connection.prepare(query).unwrap();
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (
            values,
            statement.get_status(rusqlite::StatementStatus::VmStep),
        )
    };
    connection.execute("INSERT INTO session_catalog(namespace,session_key,lifecycle,last_ms,harness,repo,metadata) VALUES(semon_namespace(),'initial-retained','retained',0,'claude','project','{}')", []).unwrap();
    let indexes = ["order", "harness", "repo", "harness_repo", "parent"];
    for suffix in indexes {
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name=?1",
                [format!("session_catalog_current_{suffix}")],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.ends_with("WHERE lifecycle='current'"), "{sql}");
    }
    let small = measure();
    let transaction = connection.unchecked_transaction().unwrap();
    for id in 0..20_000 {
        transaction.execute("INSERT INTO session_catalog(namespace,session_key,lifecycle,last_ms,harness,repo,metadata) VALUES(semon_namespace(),?1,'retained',0,'claude','project','{}')", [format!("retained-{id:05}")]).unwrap();
    }
    transaction.commit().unwrap();
    let large = measure();
    assert_eq!(small, large);
    let plan: String = connection
        .query_row(&format!("EXPLAIN QUERY PLAN {query}"), [], |row| row.get(3))
        .unwrap();
    assert!(plan.contains("session_catalog_current_harness_repo"));
    assert!(!plan.contains("SCAN"));
    eprintln!(
        "current catalog SQL VM steps: one retained={}, 20001 retained={}",
        small.1, large.1
    );
}

#[test]
fn exact_source_proof_retains_incomplete_prefix_and_rejects_source_replacement() {
    use sha2::Digest;
    let fixture = Fixture::new();
    let native = fixture.source("partial");
    let complete = fs::read(&native).unwrap();
    let mut bytes = complete.clone();
    bytes.extend_from_slice(b"{\"type\":\"user\"");
    fs::write(&native, &bytes).unwrap();
    fixture.publish_existing();
    let proof = session_source_proof(&fixture.options, "claude", "projects/project/partial.jsonl")
        .unwrap()
        .unwrap();
    assert_eq!(proof.source.offset, complete.len() as u64);
    assert_eq!(proof.size, bytes.len() as u64);
    assert_eq!(proof.freshness.state, "incomplete");
    assert_eq!(
        proof.source.prefix_sha256,
        <[u8; 32]>::from(sha2::Sha256::digest(&complete))
    );
    let replacement = native.with_extension("replacement");
    fs::write(&replacement, &bytes).unwrap();
    fs::rename(replacement, native).unwrap();
    assert_eq!(
        session_source_proof(&fixture.options, "claude", "projects/project/partial.jsonl"),
        Err(CatalogIdentityError::Unavailable)
    );
}

#[test]
fn exact_source_proof_work_stays_fixed_as_unrelated_native_manifest_grows() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
    let mut steps = Vec::new();
    for count in [100, 20_000] {
        facts.codex_rollouts = Some(
            (0..count)
                .map(|index| format!("sessions/{index:08}.jsonl"))
                .collect(),
        );
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();
        SQL_STEPS.with(|value| value.set(0));
        let proof = session_source_proof(
            &fixture.options,
            "claude",
            "projects/project/session-00000.jsonl",
        )
        .unwrap()
        .unwrap();
        assert_eq!(proof.source.native_id, "session-00000");
        steps.push(SQL_STEPS.with(|value| value.get()));
        let mut samples = Vec::new();
        for _ in 0..31 {
            let start = std::time::Instant::now();
            assert!(
                session_source_proof(
                    &fixture.options,
                    "claude",
                    "projects/project/session-00000.jsonl"
                )
                .unwrap()
                .is_some()
            );
            samples.push(start.elapsed().as_micros());
        }
        samples.sort_unstable();
        eprintln!(
            "exact source proof manifest={count} facts_bytes={} median_us={} samples=31",
            fs::metadata(fixture.options.facts.as_ref().unwrap())
                .unwrap()
                .len(),
            samples[15]
        );
    }
    assert_eq!(steps[0], steps[1]);
    eprintln!("exact source proof selected catalog VM steps={steps:?}");
}

#[test]
fn retained_projection_survives_source_disappearance_and_reappearance() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let before = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    let connection = fixture.db();
    fs::remove_file(fixture.options.claude_home.join(relative)).unwrap();
    fixture.publish_existing();
    let retained = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    assert_eq!(retained.lifecycle, "retained");
    assert_eq!(before.source, retained.source);
    assert_eq!(before.projection_generation, retained.projection_generation);
    fixture.source("session-00000");
    fixture.publish_existing();
    let current = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    assert_eq!(current.lifecycle, "current");
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_sources WHERE lifecycle='current'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn source_projection_readiness_defers_oversized_metadata_and_source_context() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let relative = "projects/project/session-00000.jsonl";
    let connection = fixture.db();
    let encoded: String = connection
        .query_row("SELECT metadata FROM session_catalog", [], |row| row.get(0))
        .unwrap();
    let mut row: CatalogRow = serde_json::from_str(&encoded).unwrap();
    row.name = "x".repeat(1024 * 1024);
    connection
        .execute(
            "UPDATE session_catalog SET metadata=?1",
            [serde_json::to_string(&row).unwrap()],
        )
        .unwrap();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_none()
    );
    row = serde_json::from_str(&encoded).unwrap();
    row.sources = vec![row.sources[0].clone(); 65];
    connection
        .execute(
            "UPDATE session_catalog SET metadata=?1",
            [serde_json::to_string(&row).unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE session_slot_projections SET source_generation=?1",
            [crate::retention::source_generation(&row.sources).unwrap()],
        )
        .unwrap();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_none()
    );
}

#[test]
fn source_projection_readiness_never_initializes_and_rejects_stale_headers() {
    let fixture = Fixture::new();
    let relative = "projects/project/session-00000.jsonl";
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_none()
    );
    assert!(!EventCache::path(&fixture.options.cache).exists());
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", "../outside.jsonl").is_err()
    );
    fixture.publish(1);
    let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
        .unwrap()
        .unwrap();
    assert_eq!(ready.projection_version, crate::slot_projection::VERSION);
    let connection = fixture.db();
    connection.execute("INSERT INTO session_catalog_invalidations(namespace,source_path,revision,reason,observed_at) VALUES(semon_namespace(),?1,1,'changed',0)", [fixture.options.claude_home.join(relative).to_str().unwrap()]).unwrap();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_none()
    );
    connection
        .execute("DELETE FROM session_catalog_invalidations", [])
        .unwrap();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_some()
    );
    let hash:String=connection.query_row("SELECT source_generation FROM session_slot_projections WHERE session_key='session-00000'",[],|row|row.get(0)).unwrap();
    connection
        .execute(
            "UPDATE session_slot_projections SET source_generation=NULL",
            [],
        )
        .unwrap();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_none()
    );
    connection
        .execute(
            "UPDATE session_slot_projections SET source_generation=?1,version=version+1",
            [hash],
        )
        .unwrap();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_none()
    );
    connection
        .execute("UPDATE session_slot_projections SET version=version-1", [])
        .unwrap();
    connection
        .execute(
            "UPDATE meta SET value='not-coherent' WHERE key='slot_projection_catalog_generation'",
            [],
        )
        .unwrap();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .is_none()
    );
}

#[test]
fn scoped_catalog_identity_preserves_native_ambiguity_and_source_authority() {
    let fixture = Fixture::new();
    fixture.publish(2);
    crate::events::CACHE_READS.with(|reads| reads.set(0));
    let selected = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
        .unwrap()
        .unwrap();
    assert_eq!(selected.source_key, "stable-source");
    assert_eq!(selected.native_id.as_deref(), Some("session-00000"));
    assert_eq!(selected.freshness.state, "cached");
    assert_eq!(selected.source_refs.len(), 1);
    assert_eq!(selected.machine_label.as_deref(), Some("synthetic-catalog"));
    assert!(
        session_catalog_identity(&fixture.options, "stable-source", "missing")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        session_catalog_identity(&fixture.options, "", "session-00000"),
        Err(CatalogIdentityError::InvalidArguments)
    );
    let connection = fixture.db();
    let read = |key: &str| -> Value {
        let text: String = connection
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    // Model a published multi-native continuation using two valid scoped
    // source ledgers. No arbitrary first native target may become authority.
    let mut row = read("session-00000");
    let second = read("session-00001");
    row["sources"]
        .as_array_mut()
        .unwrap()
        .extend(second["sources"].as_array().unwrap().iter().cloned());
    row["native_ids"] = json!(["session-00000", "session-00001"]);
    connection
        .execute(
            "UPDATE session_catalog SET metadata=?1 WHERE session_key='session-00000'",
            [row.to_string()],
        )
        .unwrap();
    let ambiguous = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
        .unwrap()
        .unwrap();
    assert_eq!(ambiguous.native_id, None);
    assert_eq!(ambiguous.native_ids, ["session-00000", "session-00001"]);
    fs::remove_file(
        fixture
            .options
            .claude_home
            .join("projects/project/session-00000.jsonl"),
    )
    .unwrap();
    let missing = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
        .unwrap()
        .unwrap();
    assert_eq!(missing.freshness.state, "incomplete");
    assert_eq!(missing.source_refs[0].state, "unavailable");
    assert_eq!(missing.facts_observation.state, "cached");
    assert_eq!(missing.native_selection, None);
    let mut changed = fixture.options.clone();
    changed.claude_home = fixture.root.join("different-root");
    assert_eq!(
        session_catalog_identity(&changed, "stable-source", "session-00000"),
        Err(CatalogIdentityError::ScopeChanged)
    );
    crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
}

#[test]
fn focused_catalog_preserves_missing_facts_and_unknown_native_manifest() {
    let fixture = Fixture::new();
    let relative = "sessions/2026/10/01/rollout-codex-session.jsonl";
    let path = fixture.options.codex_home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("{}\n", json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"codex-session","cwd":"/synthetic/project"}}))).unwrap();
    fixture.publish(1);
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(facts_path).unwrap();
    let read = || session_catalog_identity(&fixture.options, "stable-source", "codex-session");
    // Legacy None means unknown selection, not current source authority.
    assert!(facts.codex_rollouts.is_none());
    assert_eq!(read().unwrap().unwrap().freshness.state, "incomplete");
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(read().unwrap().unwrap().freshness.state, "cached");
    facts.codex_rollouts = Some(std::collections::BTreeSet::new());
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(read(), Err(CatalogIdentityError::ScopeChanged));
    let history =
        session_catalog_history_identity(&fixture.options, "stable-source", "codex-session")
            .unwrap()
            .unwrap();
    assert_eq!(history.read_scope, CatalogReadScope::RetainedHistory);
    assert_eq!(history.native_selection.unwrap().state, "retired");
    assert_eq!(history.source_refs[0].source.native_id, "codex-session");
    assert_eq!(history.source_refs[0].state, "cached");
    let (status, page) = fixture.body("scope=retained_history&sid=codex-session");
    assert_eq!(status, 200);
    assert_eq!(page["read_scope"], "retained_history");
    assert_eq!(page["items"][0]["native_selection"]["state"], "retired");

    fs::write(facts_path, b"corrupt facts").unwrap();
    let corrupt = read().unwrap().unwrap();
    assert_eq!(corrupt.freshness.state, "unavailable");
    assert_eq!(corrupt.machine_label, None);
    assert_eq!(corrupt.facts_observation.state, "unavailable");
    assert_eq!(corrupt.native_selection.unwrap().state, "unavailable");
    assert_eq!(corrupt.source_refs[0].state, "cached");
    let history =
        session_catalog_history_identity(&fixture.options, "stable-source", "codex-session")
            .unwrap()
            .unwrap();
    assert_eq!(history.freshness.state, "cached");
    assert_eq!(history.facts_observation.state, "unavailable");
    assert_eq!(history.native_selection.unwrap().state, "unavailable");
    assert_eq!(
        fixture.body("sid=session-00000").1["items"][0]["freshness"]["state"],
        "unavailable"
    );
    fs::remove_file(facts_path).unwrap();
    assert_eq!(read().unwrap().unwrap().freshness.state, "unavailable");
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    facts.version = 999;
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(read().unwrap().unwrap().freshness.state, "unavailable");
}

#[test]
fn unavailable_content_does_not_erase_current_machine_and_native_selection_observation() {
    let fixture = Fixture::new();
    let relative = "sessions/2026/10/01/rollout-codex-session.jsonl";
    let path = fixture.options.codex_home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{}\n", json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"codex-session","cwd":"/synthetic/project"}}))).unwrap();
    fixture.publish(1);
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(facts_path).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    fs::remove_file(path).unwrap();
    let read = || session_catalog_identity(&fixture.options, "stable-source", "codex-session");
    let missing = read().unwrap().unwrap();
    assert_eq!(missing.freshness.state, "unavailable");
    assert_eq!(missing.source_refs[0].state, "unavailable");
    assert_eq!(missing.facts_observation.state, "cached");
    assert_eq!(missing.native_selection.unwrap().state, "cached");
    facts.codex_rollouts = None;
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(
        read().unwrap().unwrap().native_selection.unwrap().state,
        "incomplete"
    );
    facts.codex_rollouts = Some(std::collections::BTreeSet::new());
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(read(), Err(CatalogIdentityError::ScopeChanged));
}

#[test]
fn cursors_bind_retained_read_intent_and_unknown_scope_is_rejected() {
    let fixture = Fixture::new();
    fixture.publish(3);
    let (status, first) = fixture.body("limit=1&scope=retained_history");
    assert_eq!(status, 200);
    let cursor = crate::viewer::percent_encode(first["next_cursor"].as_str().unwrap());
    assert_eq!(fixture.body(&format!("limit=1&cursor={cursor}")).0, 400);
    assert_eq!(
        fixture
            .body(&format!("limit=1&scope=retained_history&cursor={cursor}"))
            .0,
        200
    );
    assert!(Request::parse("scope=unknown").is_err());
    let current = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
        .unwrap()
        .unwrap();
    assert_eq!(current.read_scope, CatalogReadScope::Current);
}

#[test]
fn selected_attachments_keep_large_image_bytes_out_of_browsing_and_validate_the_slot() {
    let fixture = Fixture::new();
    let path = fixture.source("images");
    let mut image = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0".to_vec();
    image.resize(512 * 1024, 0);
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in image.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            encoded.push(if i > chunk.len() {
                '='
            } else {
                alphabet[((n >> shift) & 63) as usize] as char
            });
        }
    }
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", json!({"type":"user","sessionId":"images","uuid":"native-image","parentUuid":"images","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":encoded}},{"type":"text","text":"Inspect this image"}]}})).unwrap();
    drop(file);
    fixture.publish(0);
    let range = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=images&scope=retained_history&after=0&limit=100",
        None,
    );
    assert_eq!(range.status, 200);
    let page: Value = serde_json::from_slice(&range.body).unwrap();
    assert!(!String::from_utf8_lossy(&range.body).contains(&encoded[..100]));
    let selected = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["img"].is_array())
        .unwrap();
    assert_eq!(selected["img"][0]["size"], image.len());
    assert!(page["observation"]["source_bytes"].as_u64().unwrap() <= 1024 * 1024);
    let generation = page["projection"]["generation"].as_str().unwrap();
    let query = format!(
        "sid=images&scope=retained_history&after={}&limit=1&generation={generation}&part=attachment",
        selected["slot"]
    );
    let read = |query: &str| crate::session_entry_field(&fixture.options, "source", query, None);
    assert_eq!(read(&format!("{query}&image=64")).status, 400);
    assert_eq!(read(&format!("{query}&image=1")).status, 404);
    assert_ne!(
        read(&format!(
            "{}&image=0",
            query.replace("sid=images", "sid=other")
        ))
        .status,
        200
    );
    assert_eq!(
        read(&format!(
            "{}&image=0",
            query.replace(generation, &"0".repeat(64))
        ))
        .status,
        409
    );
    let reply = read(&format!("{query}&image=0"));
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type, "image/png");
    assert_eq!(reply.body, image);
    assert!(
        reply
            .headers()
            .iter()
            .any(|(name, value)| *name == "Cache-Control" && *value == "private, max-age=86400")
    );
}

#[test]
fn selected_script_uses_the_recorded_slot_and_rejects_unqualified_reads() {
    let fixture = Fixture::new();
    let relative = "sessions/2026/10/01/rollout-script-session.jsonl";
    let path = fixture.options.codex_home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let script = format!("// private recorded script\n{}", "🦀".repeat(20000));
    let output = format!("native failed result\n{}", "🦀".repeat(20000));
    let records = [
        json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"script-session","cwd":"/synthetic/project"}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"custom_tool_call","call_id":"script-call","name":"exec","input":script}}),
        json!({"type":"event_msg","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"exec-1","command":["/bin/sh","-lc","false"],"exit_code":1,"duration":{"secs":1,"nanos":0},"aggregated_output":output}}}),
        json!({"type":"response_item","timestamp":"2026-10-01T00:00:03Z","payload":{"type":"custom_tool_call_output","call_id":"script-call","output":"Script completed"}}),
    ];
    fs::write(
        &path,
        records
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(facts_path).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    fixture.publish(0);
    let query = "sid=script-session&scope=retained_history&after=0&limit=1";
    let reply = crate::session_transcript_range(&fixture.options, "source", query, None);
    assert_eq!(reply.status, 200);
    let page: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(page["session"]["details"]["calls"], 1);
    assert_eq!(page["session"]["details"]["errors"], 1);
    assert!(page["entries"][0].get("scriptText").is_none());
    let generation = page["projection"]["generation"].as_str().unwrap();
    let read = |query: &str| crate::session_entry_field(&fixture.options, "source", query, None);
    assert_eq!(read(&format!("{query}&part=script")).status, 400);
    assert_eq!(
        read(&format!(
            "{query}&part=script&generation={}",
            "0".repeat(64)
        ))
        .status,
        409
    );
    assert_eq!(
        read(&format!("{query}&part=arbitrary&generation={generation}")).status,
        400
    );
    let reply = read(&format!("{query}&part=script&generation={generation}"));
    assert_eq!(reply.status, 200);
    let expanded: Value = serde_json::from_slice(&reply.body).unwrap();
    let entry = &expanded["entries"][0];
    assert_eq!(entry["entry_id"], page["entries"][0]["entry_id"]);
    assert_eq!(entry["provenance"], page["entries"][0]["provenance"]);
    let text = entry["scriptText"].as_str().unwrap();
    assert!(script.starts_with(text));
    assert!(text.len() <= 64 * 1024);
    assert_eq!(entry["scriptTruncated"], true);
    assert!(expanded["observation"]["source_bytes"].as_u64().unwrap() <= 1024 * 1024);
    assert_eq!(read(&format!("{query}&part=output")).status, 400);
    assert_eq!(
        read(&format!(
            "{query}&part=output&generation={}",
            "0".repeat(64)
        ))
        .status,
        409
    );
    let reply = read(&format!("{query}&part=output&generation={generation}"));
    assert_eq!(reply.status, 200);
    let expanded: Value = serde_json::from_slice(&reply.body).unwrap();
    let entry = &expanded["entries"][0];
    assert_eq!(entry["entry_id"], page["entries"][0]["entry_id"]);
    assert_eq!(entry["provenance"], page["entries"][0]["provenance"]);
    let text = entry["out"].as_str().unwrap();
    assert!(output.starts_with(text.trim_end_matches('…')));
    assert!(text.len() <= 64 * 1024 + 3);
    assert_eq!(entry["outputTruncated"], true);
    assert!(expanded["observation"]["source_bytes"].as_u64().unwrap() <= 1024 * 1024);
}

#[test]
fn selected_output_expansion_keeps_native_blocks_under_source_generation() {
    const FIXTURE: &str = include_str!(
        "../../../../../tests/fixtures/compatibility/codex-native-a9b0815/parser-cases.jsonl"
    );
    const MANIFEST: &str = include_str!(
        "../../../../../tests/fixtures/compatibility/codex-native-a9b0815/manifest.json"
    );
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let expected = manifest["expected_calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "image-unknown")
        .unwrap();
    let fixture = Fixture::new();
    let relative = "sessions/2026/09/24/rollout-parser-cases.jsonl";
    let path = fixture.options.codex_home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, FIXTURE).unwrap();
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(facts_path).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    fixture.publish(0);

    let page = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=parser-cases&scope=retained_history&after=0&limit=100",
        None,
    );
    assert_eq!(page.status, 200);
    let page: Value = serde_json::from_slice(&page.body).unwrap();
    let generation = page["projection"]["generation"].as_str().unwrap();
    let call_offset = expected["offset"].as_u64().unwrap();
    let original = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["provenance"]["offset"] == call_offset)
        .unwrap()
        .clone();
    let slot = original["slot"].as_u64().unwrap();
    let db = fixture.db();
    // This publication has a current projection. Retained-history reads
    // may select its qualified fallback without a separate history row.
    assert_eq!(
        page["projection"]["version"],
        crate::slot_projection::VERSION
    );
    let saved: String = db
            .query_row(
                "SELECT s.metadata FROM session_slots s JOIN session_slot_projections p USING(session_key) WHERE s.session_key='parser-cases' AND s.slot=?1 AND p.version=?2 AND p.source_generation IS NOT NULL",
                rusqlite::params![slot as i64, crate::slot_projection::VERSION],
                |row| row.get(0),
            )
            .unwrap();
    let saved: crate::slot_projection::SavedSlot = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved.offset, call_offset);
    assert_eq!(saved.source_paths(), vec![path.as_path()]);
    assert!(matches!(
        &saved.recipe,
        crate::slot_projection::Recipe::Tool {
            result_source: true,
            ..
        }
    ));
    let query = format!(
        "sid=parser-cases&scope=retained_history&after={slot}&limit=1&generation={generation}&part=output"
    );
    let read = |query: &str| crate::session_entry_field(&fixture.options, "source", query, None);
    assert_eq!(
        read(&query.replace(&format!("&generation={generation}"), "")).status,
        400
    );
    assert_eq!(
        read(&query.replace(generation, &"0".repeat(64))).status,
        409
    );
    let expanded = read(&query);
    assert_eq!(expanded.status, 200);
    let expanded: Value = serde_json::from_slice(&expanded.body).unwrap();
    assert_eq!(expanded["projection"]["generation"], generation);
    assert_eq!(expanded["identity"], page["identity"]);
    assert_eq!(expanded["freshness"], page["freshness"]);
    assert_eq!(
        expanded["content_observation"]["sources"][0]["source"],
        page["content_observation"]["sources"][0]["source"]
    );
    let entry = &expanded["entries"][0];
    assert_eq!(entry["entry_id"], original["entry_id"]);
    assert_eq!(entry["provenance"], original["provenance"]);
    assert_eq!(entry["outputBlocks"].as_array().unwrap().len(), 4);
    assert_eq!(entry["outputBlocks"][0]["role"], "header");
    assert_eq!(entry["outputBlocks"][1]["fallback"], "[image]");
    assert_eq!(
        entry["outputBlocks"][2]["fallback"],
        "[unsupported output block]"
    );
    assert_eq!(entry["outputBlocks"][3]["role"], "body");
    assert!(entry["out"].as_str().unwrap().contains("after image"));
    assert!(expanded["observation"]["source_bytes"].as_u64().unwrap() <= 1024 * 1024);
}

#[test]
fn native_turn_context_signals_have_distinct_stable_current_and_history_entries() {
    let fixture = Fixture::new();
    let relative = "sessions/2026/10/01/rollout-signal-session.jsonl";
    let path = fixture.options.codex_home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let records = [
        json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"signal-session","cwd":"/synthetic/project"}}),
        json!({"type":"turn_context","timestamp":"2026-10-01T00:00:01Z","payload":{"model":"gpt-6-luna","approval_policy":"on-request","sandbox_policy":{"type":"workspace-write"}}}),
    ];
    fs::write(
        &path,
        records
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(facts_path).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    fixture.publish(0);
    let read = |scope: &str| {
        let reply = crate::session_transcript_range(
            &fixture.options,
            "source",
            &format!("sid=signal-session&scope={scope}&after=0&limit=100"),
            None,
        );
        assert_eq!(reply.status, 200);
        serde_json::from_slice::<Value>(&reply.body).unwrap()
    };
    let current = read("current");
    let signals = |page: &Value| -> Vec<Value> {
        page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["k"] == "signal")
            .cloned()
            .collect()
    };
    let first = signals(&current);
    assert_eq!(first.len(), 3);
    assert_eq!(
        first
            .iter()
            .map(|entry| entry["entry_id"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    assert!(
        first
            .iter()
            .all(|entry| entry["provenance"]["offset"] == first[0]["provenance"]["offset"])
    );
    assert_eq!(first, signals(&read("retained_history")));
    let connection = fixture.db();
    let stored: String = connection
        .query_row(
            "SELECT generation FROM session_slot_projections WHERE session_key='signal-session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(current["projection"]["generation"], stored);
    assert_eq!(
        crate::session_transcript_range(
            &fixture.options,
            "source",
            &format!("sid=signal-session&generation={stored}"),
            None
        )
        .status,
        409
    );
    fixture.publish(20);
    assert_eq!(first, signals(&read("current")));
    // Partial source retirement merges retained recipes by their original
    // identity; distinct signals must not collapse during that merge.
    fs::remove_file(path).unwrap();
    fixture.publish(20);
    let retained = signals(&read("retained_history"));
    assert_eq!(retained.len(), 3);
    assert_eq!(
        first
            .iter()
            .map(|entry| &entry["entry_id"])
            .collect::<Vec<_>>(),
        retained
            .iter()
            .map(|entry| &entry["entry_id"])
            .collect::<Vec<_>>()
    );
}

#[test]
fn selected_observation_does_not_report_ready_after_a_lost_publication_cas() {
    let fixture = Fixture::new();
    fixture.source("selected");
    fixture.publish_existing();
    let db = fixture.db();
    db.execute(
        "DELETE FROM session_overview WHERE session_key='selected'",
        [],
    )
    .unwrap();
    fixture.source("unrelated");
    let options = fixture.options.clone();
    crate::overview_repair::BEFORE_PUBLICATION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let evidence = crate::native_evidence::NativeEvidence::new(options);
            let mut work = crate::viewer::lock(&evidence.work);
            while !evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
            {}
            drop(work);
            evidence.retire();
        }));
    });
    let observed = crate::session_overview_observe(
        &fixture.options,
        "source",
        "sid=selected&scope=retained_history",
    );
    assert_eq!(observed.status, 503);
    let observed: Value = serde_json::from_slice(&observed.body).unwrap();
    assert_eq!(observed["error"]["code"], "selected_observation");
    assert_eq!(observed["error"]["retryable"], true);
    assert!(observed.get("state").is_none());
    // A later request can observe the committed producer revision and read
    // it normally; the rejected repair did not publish partial metadata.
    assert_eq!(
        crate::session_overview_observe(
            &fixture.options,
            "source",
            "sid=selected&scope=retained_history"
        )
        .status,
        200
    );
    assert_eq!(
        crate::session_transcript_range(
            &fixture.options,
            "source",
            "sid=selected&scope=retained_history&limit=1",
            None
        )
        .status,
        200
    );
}

#[test]
fn transcript_snapshot_survives_publication_after_selected_observation() {
    use std::io::Write;
    for scope in ["current", "retained_history"] {
        let fixture = Fixture::new();
        let path = fixture.source("selected");
        writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}", json!({"type":"assistant","sessionId":"selected","uuid":"answer","parentUuid":"selected","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"selected answer"}]}})).unwrap();
        fixture.publish_existing();
        let observation = crate::session_overview_observe(
            &fixture.options,
            "source",
            "sid=selected&scope=retained_history",
        );
        assert_eq!(observation.status, 200);
        let before = session_catalog_identity(&fixture.options, "source", "selected")
            .unwrap()
            .unwrap();

        // Force a real, atomic producer commit between selected identity
        // and recipe loading. The former two-connection read returns 409
        // here, even though neither publication is stale or incomplete.
        fixture.source("unrelated");
        let options = fixture.options.clone();
        crate::transcript_range::AFTER_SELECTION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let evidence = crate::native_evidence::NativeEvidence::new(options);
                let mut work = crate::viewer::lock(&evidence.work);
                while !evidence
                    .refresh_catalog(&mut work, crate::model::now_ms())
                    .unwrap()
                {}
                drop(work);
                evidence.retire();
            }));
        });
        let query = format!("sid=selected&scope={scope}&after=0&limit=1");
        let page = crate::session_transcript_range(&fixture.options, "source", &query, None);
        assert_eq!(page.status, 200, "{}", String::from_utf8_lossy(&page.body));
        let page: Value = serde_json::from_slice(&page.body).unwrap();
        assert_eq!(page["identity"]["generation"], before.generation);
        assert_eq!(page["entries"][0]["text"], "synthetic prompt");
        assert_eq!(page["range"]["next"], 1);
        let after = session_catalog_identity(&fixture.options, "source", "selected")
            .unwrap()
            .unwrap();
        assert_ne!(after.generation, before.generation);
        let generation = page["projection"]["generation"].as_str().unwrap();
        let continuation = crate::session_transcript_range(
            &fixture.options,
            "source",
            &format!("sid=selected&scope={scope}&after=1&limit=1&generation={generation}"),
            None,
        );
        assert_eq!(continuation.status, 200);
        let continuation: Value = serde_json::from_slice(&continuation.body).unwrap();
        assert_eq!(continuation["identity"]["generation"], after.generation);
        assert_eq!(continuation["entries"][0]["text"], "selected answer");

        // This is still a fence, not permission to serve an incoherent
        // producer snapshot merely because the caller has no cursor.
        let db = fixture.db();
        db.execute(
            "UPDATE meta SET value='unpublished' WHERE key='slot_projection_catalog_generation'",
            [],
        )
        .unwrap();
        let rejected = crate::session_transcript_range(&fixture.options, "source", &query, None);
        assert_eq!(rejected.status, 409);
        let rejected: Value = serde_json::from_slice(&rejected.body).unwrap();
        assert_eq!(rejected["error"]["code"], "stale_projection");
        assert!(rejected.get("entries").is_none());
    }
}

#[test]
fn transcript_snapshot_keeps_original_recipes_during_an_active_append() {
    use std::io::Write;
    struct Original(Vec<u8>);
    impl crate::SessionSourceReader for Original {
        fn read_range(
            &self,
            source: &SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            let generation: String = source
                .prefix_sha256
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert!(expected.is_none_or(|expected| expected == generation));
            assert!(source.offset <= self.0.len() as u64);
            assert!(offset + max as u64 <= source.offset);
            Ok(crate::SessionSourceRange {
                generation,
                length: source.offset,
                offset,
                bytes: self.0[offset as usize..offset as usize + max]
                    .to_vec()
                    .into(),
                cached: true,
            })
        }
    }
    for scope in ["current", "retained_history"] {
        let fixture = Fixture::new();
        let path = fixture.source("selected");
        fixture.publish_existing();
        assert_eq!(
            crate::session_overview_observe(
                &fixture.options,
                "source",
                "sid=selected&scope=retained_history"
            )
            .status,
            200
        );
        let original = Original(fs::read(&path).unwrap());
        let before = session_catalog_identity(&fixture.options, "source", "selected")
            .unwrap()
            .unwrap();
        let options = fixture.options.clone();
        crate::transcript_range::AFTER_SELECTION.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}", json!({"type":"assistant","sessionId":"selected","uuid":"appended","parentUuid":"selected","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"new native answer"}]}})).unwrap();
                    let evidence = crate::native_evidence::NativeEvidence::new(options);
                    let mut work = crate::viewer::lock(&evidence.work);
                    while !evidence
                        .refresh_catalog(&mut work, crate::model::now_ms())
                        .unwrap()
                    {}
                    drop(work);
                    evidence.retire();
                }));
            });
        let query = format!("sid=selected&scope={scope}&after=0&limit=100");
        let page =
            crate::session_transcript_range(&fixture.options, "source", &query, Some(&original));
        assert_eq!(page.status, 200, "{}", String::from_utf8_lossy(&page.body));
        let page: Value = serde_json::from_slice(&page.body).unwrap();
        assert_eq!(page["identity"]["generation"], before.generation);
        assert_eq!(page["projection"]["total"], 1);
        assert_eq!(page["entries"].as_array().unwrap().len(), 1);
        assert_eq!(page["entries"][0]["text"], "synthetic prompt");
        assert_eq!(page["content_observation"]["state"], "cached");

        assert_eq!(
            crate::session_overview_observe(
                &fixture.options,
                "source",
                "sid=selected&scope=retained_history"
            )
            .status,
            200
        );
        let fresh = crate::session_transcript_range(&fixture.options, "source", &query, None);
        assert_eq!(fresh.status, 200);
        let fresh: Value = serde_json::from_slice(&fresh.body).unwrap();
        assert_ne!(fresh["identity"]["generation"], before.generation);
        assert_eq!(fresh["projection"]["total"], 2);
        assert_eq!(fresh["entries"][1]["text"], "new native answer");
        let generation = fresh["projection"]["generation"].as_str().unwrap();
        let continuation =
            format!("sid=selected&scope={scope}&after=1&limit=1&generation={generation}");
        assert_eq!(
            crate::session_transcript_range(&fixture.options, "source", &continuation, None).status,
            200
        );
        let old = page["projection"]["generation"].as_str().unwrap();
        let stale = crate::session_transcript_range(
            &fixture.options,
            "source",
            &continuation.replace(generation, old),
            None,
        );
        assert_eq!(stale.status, 409);
        let stale: Value = serde_json::from_slice(&stale.body).unwrap();
        assert_eq!(stale["error"]["code"], "stale_projection");
        assert!(stale.get("entries").is_none());
        let stale_errors = crate::session_transcript_range(
            &fixture.options,
            "source",
            &format!("{continuation}&errors_generation=wrong"),
            None,
        );
        assert_eq!(stale_errors.status, 409);
    }
}

#[test]
fn selected_slot_ranges_are_native_bounded_and_stable_across_unrelated_history() {
    let fixture = Fixture::new();
    let path = fixture.source("selected");
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file,"{}",json!({"type":"assistant","sessionId":"selected","uuid":"answer-1","parentUuid":"selected","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"selected answer unique body"}]}})).unwrap();
    writeln!(file,"{}",json!({"type":"assistant","sessionId":"selected","uuid":"large-answer","parentUuid":"answer-1","timestamp":"2026-10-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"large answer unique body ".repeat(16000)}]}})).unwrap();
    drop(file);
    fixture.publish(2);
    crate::events::CACHE_READS.with(|reads| reads.set(0));
    let get = |query: &str| {
        let reply = crate::session_transcript_range(&fixture.options, "source", query, None);
        (
            reply.status,
            serde_json::from_slice::<Value>(&reply.body).unwrap(),
        )
    };
    let (status, first) = get("sid=selected&after=0&limit=2");
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["range"], json!({"first":0,"end":2,"next":2}));
    assert_eq!(first["entries"][0]["k"], "u");
    assert_eq!(first["entries"][0]["text"], "synthetic prompt");
    assert_eq!(first["entries"][1]["text"], "selected answer unique body");
    assert_eq!(
        first["entries"][1]["provenance"]["native_event_id"],
        "answer-1"
    );
    let (_, latest) = get("sid=selected&limit=1");
    assert_eq!(latest["range"]["first"], 2);
    assert_eq!(latest["freshness"]["state"], "cached");
    assert_eq!(latest["entries"][0]["clipped"], true);
    assert!(latest["observation"]["source_bytes"].as_u64().unwrap() <= 4096 + 11);
    let generation = latest["projection"]["generation"].as_str().unwrap();
    let chunks = latest["entries"][0]["field"]["chunks"].as_u64().unwrap();
    let mut expanded = String::new();
    for chunk in 0..chunks {
        let query =
            format!("sid=selected&after=2&limit=1&generation={generation}&field_chunk={chunk}");
        let reply = crate::session_entry_field(&fixture.options, "source", &query, None);
        assert_eq!(reply.status, 200);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert!(body["observation"]["source_bytes"].as_u64().unwrap() <= 64 * 1024 + 11);
        expanded.push_str(body["text"].as_str().unwrap());
    }
    assert_eq!(expanded, "large answer unique body ".repeat(16000));
    assert_eq!(get("sid=selected&after=%GG").0, 400);
    assert_eq!(get("sid=selected&scope=unknown").0, 400);
    assert_eq!(get("sid=selected&generation=wrong").0, 409);
    assert_eq!(get("sid=selected&after=99").0, 400);
    assert_eq!(get("sid=missing").0, 404);
    crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
    fixture.publish(50);
    crate::events::CACHE_READS.with(|reads| reads.set(0));
    let (_, unchanged) = get("sid=selected&after=0&limit=2");
    assert_eq!(
        first["projection"]["generation"],
        unchanged["projection"]["generation"]
    );
    assert_eq!(
        first["entries"][1]["entry_id"],
        unchanged["entries"][1]["entry_id"]
    );
    crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
    let connection = fixture.db();
    let leaked:i64=connection.query_row("SELECT COUNT(*) FROM session_slots WHERE metadata LIKE '%selected answer unique body%' OR metadata LIKE '%large answer unique body%'",[],|row|row.get(0)).unwrap();
    assert_eq!(leaked, 0);
    let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT metadata FROM session_slots WHERE session_key='selected' AND slot>=0 AND slot<2 ORDER BY slot",[],|row|row.get(3)).unwrap();
    assert!(
        plan.contains("SEARCH session_slots USING PRIMARY KEY"),
        "{plan}"
    );
}

#[test]
fn selected_provider_ranges_render_native_records_without_restoring_sources() {
    let fixture = Fixture::new();
    let path = fixture.source("archived");
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file,"{}",json!({"type":"assistant","sessionId":"archived","uuid":"archived-answer","parentUuid":"archived","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"retained native answer"}]}})).unwrap();
    drop(file);
    fixture.publish(1);
    let bytes = fs::read(&path).unwrap();
    let ready = crate::source_projection_ready(
        &fixture.options,
        "claude",
        "projects/project/archived.jsonl",
    )
    .unwrap()
    .unwrap();
    assert_eq!(ready.lifecycle, "current");
    // Simulate a rewrite after ledger observation but before producer field
    // capture. The original generation must never receive new-file spans
    // or UUIDs; the immutable original below remains provider-readable.
    let connection = fixture.db();
    let metadata: String = connection
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='archived'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row: CatalogRow = serde_json::from_str(&metadata).unwrap();
    let original = &row.sources[0];
    assert!(
        crate::native_field::capture(original, 0, 0, &crate::slot_projection::Recipe::U, None)
            .is_some()
    );
    assert!(crate::native_field::read_source_record(original, 0, None).is_some());
    let mut unqualified = original.clone();
    unqualified.changed_ns = None;
    assert!(crate::native_field::read_source_record(&unqualified, 0, None).is_none());
    let mut partial = original.clone();
    partial.offset = bytes.iter().position(|byte| *byte == b'\n').unwrap() as u64;
    assert!(crate::native_field::read_source_record(&partial, 0, None).is_none());
    fs::remove_file(&path).unwrap();
    fs::write(
        &path,
        format!(
            "{}\n",
            json!({"uuid":"replacement", "message":{"content":"replacement ".repeat(400)}})
        ),
    )
    .unwrap();
    assert!(
        crate::native_field::capture(original, 0, 0, &crate::slot_projection::Recipe::U, None)
            .is_none()
    );
    assert!(crate::native_field::read_source_record(original, 0, None).is_none());
    fs::remove_file(&path).unwrap();
    drop(connection);
    // A reopened native producer retires the missing source index. History
    // metadata and immutable native recipe generation must survive it.
    fixture.publish_existing();
    let retained = crate::source_projection_ready(
        &fixture.options,
        "claude",
        "projects/project/archived.jsonl",
    )
    .unwrap()
    .unwrap();
    assert_eq!(retained.lifecycle, "retained");
    assert_eq!(retained.source, ready.source);
    assert_eq!(retained.projection_generation, ready.projection_generation);
    let connection = fixture.db();
    let removed: i64 = connection
        .query_row(
            "SELECT count(*) FROM files WHERE path=?1",
            [path.to_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        removed, 0,
        "retained provenance must not pretend to be a current file ledger"
    );
    let retired:i64=connection.query_row("SELECT count(*) FROM session_catalog_sources WHERE session_key='archived' AND lifecycle='retained'",[],|row|row.get(0)).unwrap();
    assert_eq!(retired, 1);
    struct Provider {
        bytes: Vec<u8>,
        changed: bool,
        cached: bool,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl crate::SessionSourceReader for Provider {
        fn read_range(
            &self,
            source: &SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            assert_eq!(source.root, "claude");
            assert_eq!(source.native_id, "archived");
            assert_eq!(source.path, "projects/project/archived.jsonl");
            assert_eq!(source.offset, self.bytes.len() as u64);
            use sha2::{Digest, Sha256};
            let full: [u8; 32] = Sha256::digest(&self.bytes).into();
            assert_eq!(source.prefix_sha256, full);
            let generation: String = full.iter().map(|byte| format!("{byte:02x}")).collect();
            if let Some(expected) = expected {
                assert_eq!(expected, generation);
            }
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            let offset = usize::try_from(offset).unwrap();
            Ok(crate::SessionSourceRange {
                generation: if self.changed && call > 0 {
                    "changed".into()
                } else {
                    generation
                },
                length: self.bytes.len() as u64,
                offset: offset as u64,
                bytes: std::sync::Arc::from(
                    &self.bytes[offset..offset.saturating_add(max).min(self.bytes.len())],
                ),
                cached: self.cached,
            })
        }
    }
    let provider = Provider {
        bytes: bytes.clone(),
        changed: false,
        cached: true,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    crate::events::CACHE_READS.with(|reads| reads.set(0));
    let reply = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=archived&after=0&limit=2",
        Some(&provider),
    );
    assert_eq!(reply.status, 200);
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(body["entries"][1]["text"], "retained native answer");
    assert_eq!(body["identity"]["freshness"]["state"], "unavailable");
    assert_eq!(body["entries"][1]["freshness"]["state"], "cached");
    assert_eq!(provider.calls.load(Ordering::Relaxed), 2);
    assert!(!path.exists());
    assert_eq!(body["content_observation"]["state"], "cached");
    let fresh = Provider {
        bytes: bytes.clone(),
        changed: false,
        cached: false,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let reply = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=archived&after=0&limit=2",
        Some(&fresh),
    );
    assert_eq!(reply.status, 200);
    let fresh_body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(fresh_body["content_observation"]["state"], "available");
    assert_eq!(fresh_body["identity"]["freshness"]["state"], "unavailable");
    let generation = fresh_body["projection"]["generation"].as_str().unwrap();
    let reply = crate::session_entry_field(
        &fixture.options,
        "source",
        &format!("sid=archived&after=1&limit=1&generation={generation}&field_chunk=0"),
        Some(&fresh),
    );
    assert_eq!(reply.status, 200);
    let field: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(field["text"], "retained native answer");
    assert_eq!(field["content_observation"]["state"], "available");
    assert_eq!(field["field"]["layers"], 1);
    let changed = Provider {
        bytes,
        changed: true,
        cached: true,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let reply = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=archived&after=0&limit=2",
        Some(&changed),
    );
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 409);
    assert_eq!(body["error"]["code"], "stale_projection");
    assert_eq!(body["resynchronize"], true);
    assert!(
        body.get("entries").is_none(),
        "changed provider generations publish no partial page"
    );
    crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
    assert!(!path.exists());
}

#[test]
fn unsupported_storage_is_observable_at_product_read_entrypoints() {
    for change in [
        "PRAGMA user_version=2147483647",
        "DELETE FROM meta WHERE key='cache_version'",
        "UPDATE meta SET value='invalid' WHERE key='cache_version'",
    ] {
        let fixture = Fixture::new();
        fixture.publish(1);
        let path = EventCache::path(&fixture.options.cache);
        let db = fixture.db();
        db.execute_batch(change).unwrap();
        drop(db);
        let original = fs::read(&path).unwrap();
        assert!(crate::collect(&fixture.options).is_err());
        assert!(crate::local_facts(&fixture.options).is_err());
        assert_eq!(fixture.body("").0, 503);
        assert_eq!(
            crate::session_transcript_range(
                &fixture.options,
                "source",
                "sid=session-00000&after=0&limit=1",
                None
            )
            .status,
            503
        );
        let mut query = crate::Query::new(fixture.options.clone());
        assert!(query.call("list_sessions", &json!({})).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn unsupported_recipe_stays_unavailable_without_implicit_upgrade() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let connection = fixture.db();
    connection
        .execute(
            "UPDATE session_slot_projections SET version=3 WHERE session_key='session-00000'",
            [],
        )
        .unwrap();
    let before = crate::native_timing::cleanup_tests::snapshot(&connection);
    let cache = EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
    assert!(cache.qualified_catalog().unwrap());
    let unavailable = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=session-00000&after=0&limit=1",
        None,
    );
    assert_eq!(unavailable.status, 503);
    assert_eq!(
        crate::native_timing::cleanup_tests::snapshot(&connection),
        before
    );
}

#[test]
fn source_observer_publishes_first_push_without_a_viewer_core() {
    let fixture = Fixture::new();
    fixture.source("first-push");
    let observer = crate::SessionCatalogObserver::new(fixture.options.clone());
    assert!(!EventCache::path(&fixture.options.cache).exists());
    observer.changed().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let reply = session_catalog_page(&fixture.options, "configured", "sid=first-push");
        if reply.status == 200 {
            let body: Value = serde_json::from_slice(&reply.body).unwrap();
            if body["items"]
                .as_array()
                .is_some_and(|items| items.len() == 1)
            {
                assert_eq!(body["items"][0]["key"], "first-push");
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "source publication did not complete"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    observer.close();
    assert!(EventCache::path(&fixture.options.cache).exists());
    let mut core = ViewerCore::new(fixture.options.clone());
    assert!(core.demand_catalog_source("local").unwrap());
    core.set_refresh(Refresh::OnInvalidate);
    assert!(!core.demand_catalog_source("wrong").unwrap());
    assert!(core.demand_catalog_source("local").unwrap());
    core.close();
    assert!(!core.demand_catalog_source("local").unwrap());
}

#[test]
fn default_local_viewer_advertises_a_nonempty_stable_catalog_source() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let core = ViewerCore::new(fixture.options.clone());
    let inventory = core.respond("GET", "/api/session-sources", "limit=1", None);
    assert_eq!(inventory.status, 200);
    let inventory: Value = serde_json::from_slice(&inventory.body).unwrap();
    assert_eq!(
        inventory["items"],
        serde_json::json!([{"source_key":"local","label":"synthetic-catalog"}])
    );
    assert!(inventory["next_cursor"].is_null());
    let capability = core.respond("GET", "/api/session-capabilities", "", None);
    assert_eq!(capability.status, 200);
    let body: Value = serde_json::from_slice(&capability.body).unwrap();
    assert_eq!(body["source_key"], "local");
    for (path, query) in [
        ("/api/session-identity", "sid=session-00000"),
        (
            "/api/session-transcript",
            "machine=local&sid=session-00000&after=0&limit=1",
        ),
    ] {
        let reply = core.respond("GET", path, query, None);
        if reply.status != 200 {
            let code = serde_json::from_slice::<Value>(&reply.body)
                .ok()
                .and_then(|body| body["error"]["code"].as_str().map(str::to_owned));
            eprintln!(
                "native diagnostic route={path} status={} code={code:?}",
                reply.status
            );
        }
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["identity"]["source_key"], "local");
    }
    let reply = core.respond(
        "GET",
        "/api/sessions",
        "machine=local&sid=session-00000",
        None,
    );
    assert_eq!(reply.status, 200);
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(body["machine"], "local");
    assert_eq!(
        core.respond(
            "GET",
            "/api/session-identity",
            "machine=wrong&sid=session-00000",
            None
        )
        .status,
        404
    );
    core.close();
}

#[test]
fn complete_native_edit_diff_is_not_an_incomplete_output_preview() {
    use std::io::Write;
    let fixture = Fixture::new();
    let path = fixture.source("edit-session");
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    for record in [
        json!({"type":"assistant","sessionId":"edit-session","uuid":"edit-call","parentUuid":"edit-session","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit-tool","name":"Edit","input":{"file_path":"/synthetic/project/main.rs","old_string":"before","new_string":"after"}}]}}),
        json!({"type":"user","sessionId":"edit-session","uuid":"edit-result","parentUuid":"edit-call","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit-tool","content":"The file was updated successfully."}]}}),
    ] {
        writeln!(file, "{record}").unwrap();
    }
    drop(file);
    fixture.publish(0);
    let reply = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=edit-session&limit=100",
        None,
    );
    assert_eq!(reply.status, 200);
    let page: Value = serde_json::from_slice(&reply.body).unwrap();
    let edit = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "Edit")
        .unwrap();
    assert_eq!(edit["diff"], json!([["del", "-before"], ["add", "+after"]]));
    assert_eq!(edit["clipped"], false);
    assert_eq!(edit["field"]["complete"], true);
    assert!(edit.get("out").is_none());
}

#[test]
fn final_masked_thinking_is_pending_only_for_qualified_current_work() {
    use std::io::Write;
    for live in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.source("thinking-session");
        writeln!(fs::OpenOptions::new().append(true).open(path).unwrap(), "{}", json!({"type":"assistant","sessionId":"thinking-session","uuid":"thinking","parentUuid":"thinking-session","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":""}]}})).unwrap();
        if live {
            let facts_path = fixture.options.facts.as_ref().unwrap();
            let mut facts = crate::read_facts(facts_path).unwrap();
            facts.proc_starts.insert(99, 777);
            crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
            fs::create_dir_all(fixture.options.claude_home.join("sessions")).unwrap();
            fs::write(
                fixture.options.claude_home.join("sessions/99.json"),
                json!({"pid":99,"sessionId":"thinking-session","procStart":777,"status":"busy"})
                    .to_string(),
            )
            .unwrap();
        }
        fixture.publish(0);
        let read = |query| {
            let reply = crate::session_transcript_range(&fixture.options, "local", query, None);
            assert_eq!(reply.status, 200);
            serde_json::from_slice::<Value>(&reply.body).unwrap()
        };
        let page = read("sid=thinking-session&scope=retained_history&after=0&limit=100");
        let entry = page["entries"].as_array().unwrap().last().unwrap();
        assert_eq!(entry["k"], "think");
        assert_eq!(entry["pending"].as_bool(), live.then_some(true));
        let earlier = read("sid=thinking-session&scope=retained_history&after=0&limit=1");
        assert!(earlier["entries"][0].get("pending").is_none());
        let db = fixture.db();
        db.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='thinking-session'", []).unwrap();
        let retired = read("sid=thinking-session&scope=retained_history&after=0&limit=100");
        assert!(
            retired["entries"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()
                .get("pending")
                .is_none()
        );
    }
}

#[test]
fn native_pending_calls_use_qualified_activity_without_persisting_live_authority() {
    for live in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.source("pending-call");
        use std::io::Write;
        writeln!(fs::OpenOptions::new().append(true).open(path).unwrap(), "{}", json!({"type":"assistant","sessionId":"pending-call","uuid":"call","parentUuid":"pending-call","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"pending-tool","name":"Bash","input":{"command":"cargo test"}}]}})).unwrap();
        if live {
            let facts_path = fixture.options.facts.as_ref().unwrap();
            let mut facts = crate::read_facts(facts_path).unwrap();
            facts.proc_starts.insert(99, 777);
            crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
            fs::create_dir_all(fixture.options.claude_home.join("sessions")).unwrap();
            fs::write(
                fixture.options.claude_home.join("sessions/99.json"),
                json!({"pid":99,"sessionId":"pending-call","procStart":777,"status":"busy"})
                    .to_string(),
            )
            .unwrap();
        }
        fixture.publish(0);
        let errors = || {
            let reply = crate::session_annotations(
                &fixture.options,
                "local",
                "sid=pending-call&scope=retained_history&kind=errors&limit=100",
            );
            assert_eq!(reply.status, 200);
            serde_json::from_slice::<Value>(&reply.body).unwrap()
        };
        let before_errors = errors();
        assert_eq!(before_errors["total"], if live { 0 } else { 1 });
        let descriptor = crate::session_catalog_page(
            &fixture.options,
            "local",
            "sid=pending-call&scope=retained_history",
        );
        assert_eq!(descriptor.status, 200);
        let descriptor: Value = serde_json::from_slice(&descriptor.body).unwrap();
        assert_eq!(
            descriptor["items"][0]["transcript"]["errors_generation"],
            before_errors["generation"]
        );
        if live {
            // Read-only retained custody cannot inherit a recorded live PID.
            let db = fixture.db();
            db.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='pending-call'", []).unwrap();
            let retained = errors();
            assert_eq!(retained["total"], 1);
            assert_ne!(retained["generation"], before_errors["generation"]);
            db.execute("UPDATE session_catalog SET lifecycle='current',metadata=json_set(metadata,'$.lifecycle','current') WHERE session_key='pending-call'", []).unwrap();
            assert_eq!(errors()["generation"], before_errors["generation"]);
        }
        let read = || {
            let reply = crate::session_transcript_range(
                &fixture.options,
                "local",
                "sid=pending-call&scope=retained_history&after=0&limit=100",
                None,
            );
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
            let page: Value = serde_json::from_slice(&reply.body).unwrap();
            page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["k"] == "tool")
                .unwrap()
                .clone()
        };
        let entry = read();
        if live {
            assert_eq!(entry["live"], true);
            assert!(entry.get("ok").is_none());
            assert_eq!(entry["since"], entry["observed_at"]);
            assert!(entry["unfinished"].is_null());
            let facts_path = fixture.options.facts.as_ref().unwrap();
            let mut facts = crate::read_facts(facts_path).unwrap();
            facts.proc_starts.clear();
            crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
            fixture.publish(0);
        }
        let stopped = read();
        assert_eq!(stopped["unfinished"], true);
        assert_eq!(stopped["ok"], false);
        assert!(stopped["live"].is_null());
        let stopped_errors = errors();
        assert_eq!(stopped_errors["total"], 1);
        if live {
            assert_ne!(stopped_errors["generation"], before_errors["generation"]);
            let stale = crate::session_annotations(
                &fixture.options,
                "local",
                &format!(
                    "sid=pending-call&scope=retained_history&kind=errors&limit=100&generation={}",
                    before_errors["generation"].as_str().unwrap()
                ),
            );
            assert_eq!(stale.status, 409);
            let stale_range = crate::session_transcript_range(
                &fixture.options,
                "local",
                &format!(
                    "sid=pending-call&scope=retained_history&after=0&limit=100&errors_generation={}",
                    before_errors["generation"].as_str().unwrap()
                ),
                None,
            );
            assert_eq!(stale_range.status, 409);
            let current_range = crate::session_transcript_range(
                &fixture.options,
                "local",
                &format!(
                    "sid=pending-call&scope=retained_history&after=0&limit=100&errors_generation={}",
                    stopped_errors["generation"].as_str().unwrap()
                ),
                None,
            );
            assert_eq!(current_range.status, 200);
            let current_range: Value = serde_json::from_slice(&current_range.body).unwrap();
            assert_eq!(
                current_range["projection"]["errors_generation"],
                stopped_errors["generation"]
            );
        }
        let db = fixture.db();
        let stored: String = db.query_row("SELECT metadata FROM session_slots WHERE session_key='pending-call' AND json_extract(metadata,'$.recipe.outcome')='Pending'", [], |row|row.get(0)).unwrap();
        assert!(!stored.contains("\"Live\""));
        assert!(!stored.contains("\"since\""));
    }
}

#[test]
fn native_tool_outputs_use_bounded_string_fields_and_preserve_call_context() {
    for harness in [
        "claude",
        "claude-block",
        "codex",
        "codex-encoded",
        "codex-object",
    ] {
        let fixture = Fixture::new();
        let body = "native tool output line \"quoted\" \n".repeat(15000);
        let key = if harness.starts_with("claude") {
            "claude-tools"
        } else {
            "codex-tools"
        };
        let (path, records) = if harness.starts_with("claude") {
            let content = if harness == "claude-block" {
                json!([{"type":"text","text":body}])
            } else {
                json!(body)
            };
            (
                fixture.source(key),
                vec![
                    json!({"type":"assistant","sessionId":key,"uuid":"native-call","parentUuid":key,"timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"native-tool","name":"Bash","input":{"command":"printf fixture"}}]}}),
                    json!({"type":"user","sessionId":key,"uuid":"native-result","parentUuid":"native-call","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"native-tool","content":content}]}}),
                ],
            )
        } else {
            let relative = "sessions/2026/10/01/rollout-codex-tools.jsonl";
            let path = fixture.options.codex_home.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let facts_path = fixture.options.facts.as_ref().unwrap();
            let mut facts = crate::read_facts(facts_path).unwrap();
            facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
            crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
            let output = match harness {
                "codex-encoded" => {
                    json!(json!({"output":body,"metadata":{"exit_code":0}}).to_string())
                }
                "codex-object" => json!({"output":body,"metadata":{"exit_code":0}}),
                _ => json!(body),
            };
            (
                path,
                vec![
                    json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":key,"cwd":"/synthetic/project"}}),
                    json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call","name":"exec_command","call_id":"native-tool","arguments":"{\"cmd\":\"printf fixture\"}"}}),
                    json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"native-tool","output":output}}),
                ],
            )
        };
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        for record in records {
            writeln!(file, "{record}").unwrap();
        }
        drop(file);
        fixture.publish(1);
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let reply = crate::session_transcript_range(
            &fixture.options,
            "source",
            &format!("sid={key}&after=0&limit=100"),
            None,
        );
        assert_eq!(
            reply.status,
            200,
            "{harness}: {}",
            String::from_utf8_lossy(&reply.body)
        );
        let page: Value = serde_json::from_slice(&reply.body).unwrap();
        let entry = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["k"] == "tool")
            .expect("native tool slot");
        assert_eq!(entry["field"]["name"], "out", "{harness}: {entry}");
        assert!(
            entry["ok"] == true || entry["ok"].is_null(),
            "{harness}: {entry}"
        );
        assert!(
            entry["unfinished"].is_null(),
            "recorded output is not a missing result: {harness}"
        );
        assert!(
            entry["arg"]
                .as_str()
                .is_some_and(|arg| arg.contains("printf fixture")),
            "{harness}: {entry}"
        );
        assert!(
            entry["out"]
                .as_str()
                .unwrap()
                .starts_with("native tool output line")
        );
        assert!(page["observation"]["source_bytes"].as_u64().unwrap() < 16 * 1024);
        let generation = page["projection"]["generation"].as_str().unwrap();
        let slot = entry["slot"].as_u64().unwrap();
        let chunks = entry["field"]["chunks"].as_u64().unwrap();
        let mut expanded = String::new();
        for chunk in 0..chunks {
            let reply = crate::session_entry_field(
                &fixture.options,
                "source",
                &format!(
                    "sid={key}&after={slot}&limit=1&generation={generation}&field_chunk={chunk}"
                ),
                None,
            );
            assert_eq!(reply.status, 200);
            let part: Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(part["field"]["name"], "out");
            assert_eq!(
                part["provenance"], entry["field"]["provenance"],
                "{harness}: native result anchor"
            );
            assert_ne!(
                part["provenance"]["offset"], entry["provenance"]["offset"],
                "tool calls and results retain distinct native positions"
            );
            assert_eq!(
                part["field"]["layers"],
                if harness == "codex-encoded" { 2 } else { 1 }
            );
            assert_eq!(part["content_observation"]["state"], "cached");
            assert_eq!(
                part["content_observation"]["sources"][0]["generation_hash"]
                    .as_str()
                    .unwrap()
                    .len(),
                64
            );
            expanded.push_str(part["text"].as_str().unwrap());
        }
        assert_eq!(expanded, body);
        crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
    }
}

#[test]
fn mixed_local_and_provider_reads_never_fallback_for_replaced_sources() {
    let fixture = Fixture::new();
    let path = fixture.source("mixed");
    fixture.publish(1);
    struct Reject(std::sync::atomic::AtomicUsize);
    impl crate::SessionSourceReader for Reject {
        fn read_range(
            &self,
            _: &SessionSourceRef,
            _: Option<&str>,
            _: u64,
            _: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no authorized archive",
            ))
        }
    }
    let provider = Reject(std::sync::atomic::AtomicUsize::new(0));
    let get = || {
        crate::session_transcript_range_with_mode(
            &fixture.options,
            "source",
            "sid=mixed&after=0&limit=1",
            Some(&provider),
            crate::SessionSourceReadMode::LocalThenProvider,
        )
    };
    let first = get();
    assert_eq!(first.status, 200);
    let body: Value = serde_json::from_slice(&first.body).unwrap();
    assert_eq!(body["entries"][0]["text"], "synthetic prompt");
    let identity = body["entries"][0]["entry_id"].clone();
    let provenance = body["entries"][0]["provenance"].clone();
    assert_eq!(provider.0.load(Ordering::Relaxed), 0);
    fs::remove_file(&path).unwrap();
    fs::write(&path, "{}\n").unwrap();
    let changed = get();
    let body: Value = serde_json::from_slice(&changed.body).unwrap();
    assert_eq!(changed.status, 409);
    assert_eq!(body["error"]["code"], "stale_projection");
    assert_eq!(body["resynchronize"], true);
    assert!(body.get("entries").is_none());
    assert_eq!(
        provider.0.load(Ordering::Relaxed),
        0,
        "replacement must resynchronize, not restore old source bytes"
    );
    fs::remove_file(&path).unwrap();
    let missing = get();
    let body: Value = serde_json::from_slice(&missing.body).unwrap();
    assert_eq!(missing.status, 200);
    assert_eq!(body["entries"][0]["entry_id"], identity);
    assert_eq!(body["entries"][0]["provenance"], provenance);
    assert_eq!(body["entries"][0]["freshness"]["state"], "incomplete");
    assert_eq!(
        provider.0.load(Ordering::Relaxed),
        1,
        "only absent top-level source requests host reader"
    );
}

struct PinnedOriginal {
    source: SessionSourceRef,
    bytes: Vec<u8>,
    calls: std::sync::atomic::AtomicUsize,
    change_at: Option<usize>,
}
impl crate::SessionSourceReader for PinnedOriginal {
    fn read_range(
        &self,
        source: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        use sha2::{Digest, Sha256};
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        let generation = format!("archive:{:x}", Sha256::digest(&self.bytes));
        if source != &self.source
            || expected.is_some_and(|expected| expected != generation)
            || self.change_at.is_some_and(|at| call >= at)
        {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        assert!(max <= crate::SESSION_SOURCE_CHUNK_MAX);
        assert!(offset + max as u64 <= source.offset);
        assert_eq!(
            <[u8; 32]>::from(Sha256::digest(&self.bytes[..source.offset as usize])),
            source.prefix_sha256
        );
        Ok(crate::SessionSourceRange {
            generation,
            length: self.bytes.len() as u64,
            offset,
            bytes: self.bytes[offset as usize..offset as usize + max]
                .to_vec()
                .into(),
            cached: true,
        })
    }
}

fn local_only_codex_fixture() -> (Fixture, PathBuf) {
    use std::io::Write;
    let fixture = Fixture::new();
    let relative = fixture.codex_source("local-proof", None, 1);
    let path = fixture.options.codex_home.join(&relative);
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", json!({"type":"response_item","timestamp":"2026-10-01T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"local original prompt"}]}})).unwrap();
    for _ in 0..100 {
        writeln!(
            file,
            "{}",
            json!({"type":"synthetic_padding","payload":"x".repeat(32 * 1024)})
        )
        .unwrap();
    }
    writeln!(file, "{}", json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"local original answer"}]}})).unwrap();
    drop(file);
    let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.clone()]));
    crate::write_facts(
        fixture.options.facts.as_ref().unwrap(),
        &facts,
        &crate::Options::default().source,
    )
    .unwrap();
    let mut cache =
        EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
    let prepared = crate::model::prepare_sources(
        &fixture.options,
        &mut cache,
        &[crate::inputs::Input {
            root: crate::inputs::InputRoot::Codex,
            path: relative,
        }],
        crate::model::now_ms(),
    )
    .unwrap();
    assert_eq!(
        cache.publish_partial_catalog(&prepared).unwrap(),
        crate::events::Outcome::Written
    );
    drop(cache);
    assert_eq!(
        session_catalog_identity(&fixture.options, "source", "local-proof")
            .unwrap()
            .unwrap()
            .owner_qualification,
        CatalogOwnerQualification::Provisional
    );
    (fixture, path)
}

#[test]
fn selected_web_field_and_mcp_proofs_share_the_recipe_snapshot() {
    for reader in ["web", "field", "mcp"] {
        let (fixture, path) = local_only_codex_fixture();
        let first = crate::session_transcript_range_with_mode(
            &fixture.options,
            "source",
            "sid=local-proof&after=0&limit=1",
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
        );
        assert_eq!(first.status, 200);
        let first: Value = serde_json::from_slice(&first.body).unwrap();
        let generation = first["projection"]["generation"].as_str().unwrap();
        let cache = EventCache::path(&fixture.options.cache);
        let collect: Box<dyn FnOnce()> = Box::new(move || {
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(2))
                .unwrap();
            // Commit collection after recipe selection but before opening
            // any lazy proof header. Only the request's WAL snapshot keeps
            // this publication readable now.
            Connection::open(cache).unwrap().execute(
                    "DELETE FROM meta WHERE key LIKE 'native_source_chunks:%' OR key LIKE 'native_published_chunks:%' OR key LIKE 'native_proof_tree:%' OR key LIKE 'native_proof_repair:%'", []
                ).unwrap();
        });
        if reader == "mcp" {
            crate::query_source::BEFORE_FOCUSED_CAPTURE
                .with(|hook| *hook.borrow_mut() = Some(collect));
            let mut query =
                crate::Query::with_machines(vec![("source".into(), fixture.options.clone())]);
            let result = query
                .call_selected(
                    "source",
                    "local-proof",
                    "read_transcript",
                    &json!({"id":"local-proof","after":0,"limit":1}),
                    None,
                    crate::SessionSourceReadMode::LocalOrPinnedProvider,
                )
                .unwrap();
            assert!(
                result.to_string().contains("local original prompt"),
                "{result}"
            );
            assert!(
                crate::query_source::BEFORE_FOCUSED_CAPTURE.with(|hook| hook.borrow().is_none())
            );
        } else {
            crate::transcript_range::AFTER_SELECTION
                .with(|hook| *hook.borrow_mut() = Some(collect));
            let request = format!(
                "sid=local-proof&after=1&limit=1&generation={generation}{}",
                if reader == "field" {
                    "&field_chunk=0"
                } else {
                    ""
                }
            );
            let reply = crate::session_transcript_range_with_mode(
                &fixture.options,
                "source",
                &request,
                None,
                crate::SessionSourceReadMode::LocalOrPinnedProvider,
            );
            assert_eq!(
                reply.status,
                200,
                "{reader}: {}",
                String::from_utf8_lossy(&reply.body)
            );
            assert!(String::from_utf8_lossy(&reply.body).contains("local original answer"));
        }
        let fresh = crate::session_transcript_range_with_mode(
            &fixture.options,
            "source",
            "sid=local-proof&after=0&limit=1",
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
        );
        assert_eq!(
            fresh.status, 409,
            "a new request must not reuse the old snapshot"
        );
    }
}

#[test]
fn index_leading_catalog_keeps_published_codex_pages_and_query_readable() {
    use std::io::Write;
    for mutation in ["append", "copy"] {
        let (fixture, path) = local_only_codex_fixture();
        let get = |after: usize, generation: Option<&str>| {
            crate::session_transcript_range_with_mode(
                &fixture.options,
                "source",
                &format!(
                    "sid=local-proof&after={after}&limit=1{}",
                    generation
                        .map(|g| format!("&generation={g}"))
                        .unwrap_or_default()
                ),
                None,
                crate::SessionSourceReadMode::LocalOrPinnedProvider,
            )
        };
        let before = get(0, None);
        assert_eq!(before.status, 200);
        let before: Value = serde_json::from_slice(&before.body).unwrap();
        let generation = before["projection"]["generation"].as_str().unwrap();
        for append in 0..3 {
            if mutation == "copy" && append == 0 {
                fs::rename(&path, path.with_extension("saved")).unwrap();
                fs::copy(path.with_extension("saved"), &path).unwrap();
            }
            writeln!(
                fs::OpenOptions::new().append(true).open(&path).unwrap(),
                "{}",
                json!({"type":"synthetic_append","iteration":append})
            )
            .unwrap();
            let mut cache =
                EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
            // Commit Index/proofs and deliberately defer Catalog publication.
            let _prepared = crate::model::prepare_sources(
                &fixture.options,
                &mut cache,
                &[crate::Input {
                    root: crate::InputRoot::Codex,
                    path: path
                        .strip_prefix(&fixture.options.codex_home)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .into(),
                }],
                crate::model::now_ms(),
            )
            .unwrap();
            drop(cache);
            crate::events::FILE_READS.with(|n| n.set(0));
            crate::events::HASHED_BYTES.with(|n| n.set(0));
            crate::source_chunks::READ_BYTES.with(|n| n.set(0));
            for (after, text) in [(0, "local original prompt"), (1, "local original answer")] {
                let page = get(after, Some(generation));
                assert_eq!(
                    page.status,
                    200,
                    "{mutation}/{append}: {}",
                    String::from_utf8_lossy(&page.body)
                );
                let body: Value = serde_json::from_slice(&page.body).unwrap();
                assert_eq!(body["entries"][0]["text"], text);
                // Native message scalars support the same generation-bound
                // field expansion as the initially rendered preview.
                let expanded = crate::session_entry_field_with_mode(
                    &fixture.options,
                    "source",
                    &format!(
                        "sid=local-proof&after={after}&limit=1&generation={generation}&field_chunk=0"
                    ),
                    None,
                    crate::SessionSourceReadMode::LocalOrPinnedProvider,
                );
                assert_eq!(
                    expanded.status,
                    200,
                    "{mutation}: {}",
                    String::from_utf8_lossy(&expanded.body)
                );
                let expanded: Value = serde_json::from_slice(&expanded.body).unwrap();
                assert_eq!(expanded["text"], text);
            }
            let mut query =
                crate::Query::with_machines(vec![("source".into(), fixture.options.clone())]);
            for tool in ["get_session", "read_transcript"] {
                let args = if tool == "get_session" {
                    json!({"id":"local-proof"})
                } else {
                    json!({"id":"local-proof","after":0,"limit":1})
                };
                let value = query
                    .call_selected(
                        "source",
                        "local-proof",
                        tool,
                        &args,
                        None,
                        crate::SessionSourceReadMode::LocalOrPinnedProvider,
                    )
                    .unwrap();
                assert!(value.get("error").is_none(), "{tool}: {value}");
                assert!(
                    value.to_string().contains("local original prompt"),
                    "{tool}: {value}"
                );
            }
            assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), 0);
            assert_eq!(crate::events::HASHED_BYTES.with(std::cell::Cell::get), 0);
            assert!(
                crate::source_chunks::READ_BYTES.with(std::cell::Cell::get)
                    < 32 * crate::source_chunks::SIZE as u64
            );
        }
        // Preserved proofs still reject changed original bytes.
        let mut bytes = fs::read(&path).unwrap();
        let at = bytes
            .windows(b"local original prompt".len())
            .position(|s| s == b"local original prompt")
            .unwrap();
        bytes[at] = b'X';
        fs::write(&path, bytes).unwrap();
        assert_eq!(get(0, Some(generation)).status, 409);
    }
}

#[test]
fn local_only_provisional_codex_pages_use_persisted_covering_proofs() {
    use std::io::Write;
    for mutation in ["append", "copy", "touch", "legacy"] {
        let (fixture, path) = local_only_codex_fixture();
        let get = |query: &str, mode| {
            crate::session_transcript_range_with_mode(&fixture.options, "source", query, None, mode)
        };
        let query = "sid=local-proof&scope=retained_history&after=0&limit=1";
        let before = get(query, crate::SessionSourceReadMode::LocalThenProvider);
        assert_eq!(before.status, 200);
        let before: Value = serde_json::from_slice(&before.body).unwrap();
        let original = fs::read(&path).unwrap();
        assert!(original.len() > 3 * 1024 * 1024);
        match mutation {
            "append" => fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"{\"synthetic_append\":true}\n")
                .unwrap(),
            "copy" => {
                fs::rename(&path, path.with_extension("saved")).unwrap();
                fs::write(&path, &original).unwrap();
            }
            "touch" => fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(2))
                .unwrap(),
            "legacy" => {
                let mut db = fixture.db();
                let text: String = db
                    .query_row(
                        "SELECT metadata FROM session_catalog WHERE session_key='local-proof'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                let mut row: CatalogRow = serde_json::from_str(&text).unwrap();
                crate::events::store::chunk_proofs::legacy_change_token_for_test(
                    &EventCache::path(&fixture.options.cache),
                    path.to_str().unwrap(),
                    &crate::source_carrier::ledger(&row.sources[0]),
                );
                row.sources[0].changed_ns = None;
                let slots: Vec<String> = db.prepare("SELECT metadata FROM session_slots WHERE session_key='local-proof' ORDER BY slot").unwrap()
                        .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
                let generation = crate::slot_projection::generation(&slots, &row.sources).unwrap();
                let source_generation = crate::retention::source_generation(&row.sources).unwrap();
                let tx = db.transaction().unwrap();
                tx.execute(
                    "UPDATE session_catalog SET metadata=?1 WHERE session_key='local-proof'",
                    [serde_json::to_string(&row).unwrap()],
                )
                .unwrap();
                tx.execute("UPDATE session_slot_projections SET generation=?1,source_generation=?2 WHERE session_key='local-proof'", rusqlite::params![generation, source_generation]).unwrap();
                tx.commit().unwrap();
            }
            _ => unreachable!(),
        }
        let strict = get(query, crate::SessionSourceReadMode::LocalThenProvider);
        assert_eq!(
            strict.status, 409,
            "regression must reject at deployed strict path: {mutation}"
        );
        let strict: Value = serde_json::from_slice(&strict.body).unwrap();
        assert_eq!(strict["fence_reason"], "local_source_revision");
        assert_eq!(
            strict["local_revision_reason"],
            match mutation {
                "append" => "size_grew",
                "copy" => "identity",
                "legacy" => "missing_change_token",
                _ => "change_time",
            }
        );
        for scope in ["current", "retained_history"] {
            crate::source_chunks::READ_BYTES.with(|n| n.set(0));
            crate::source_chunks::READ_BLOCKS.with(|blocks| blocks.borrow_mut().clear());
            crate::events::SOURCE_HASHED_BYTES.with(|n| n.set(0));
            crate::events::HASHED_BYTES.with(|n| n.set(0));
            let query = format!("sid=local-proof&scope={scope}&after=0&limit=1");
            let page = get(&query, crate::SessionSourceReadMode::LocalOrPinnedProvider);
            assert_eq!(
                page.status,
                200,
                "{mutation}/{scope}: {}",
                String::from_utf8_lossy(&page.body)
            );
            let page: Value = serde_json::from_slice(&page.body).unwrap();
            assert_eq!(page["entries"][0]["text"], "local original prompt");
            assert_eq!(
                page["entries"][0]["entry_id"],
                before["entries"][0]["entry_id"]
            );
            assert_eq!(
                page["entries"][0]["provenance"],
                before["entries"][0]["provenance"]
            );
            assert_eq!(crate::events::HASHED_BYTES.with(std::cell::Cell::get), 0);
            assert!(
                crate::source_chunks::READ_BYTES.with(std::cell::Cell::get)
                    <= 4 * crate::source_chunks::SIZE as u64
            );
            crate::source_chunks::READ_BLOCKS
                .with(|blocks| assert_eq!(*blocks.borrow(), std::collections::BTreeSet::from([0])));
            assert!(
                crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get)
                    <= 4 * crate::source_chunks::SIZE as u64
            );
            let generation = page["projection"]["generation"].as_str().unwrap();
            let next =
                format!("sid=local-proof&scope={scope}&after=1&limit=1&generation={generation}");
            let continuation = get(&next, crate::SessionSourceReadMode::LocalOrPinnedProvider);
            assert_eq!(continuation.status, 200);
            let continuation: Value = serde_json::from_slice(&continuation.body).unwrap();
            assert_eq!(continuation["entries"][0]["text"], "local original answer");
            let stale = get(
                &next.replace(generation, "stale"),
                crate::SessionSourceReadMode::LocalOrPinnedProvider,
            );
            assert_eq!(stale.status, 409);
        }
        if mutation != "legacy" {
            let observed = crate::session_overview_observe(
                &fixture.options,
                "source",
                "sid=local-proof&scope=retained_history",
            );
            assert_eq!(observed.status, 200);
            // Observe acknowledges metadata; it does not relabel the carrier.
            let identity = session_catalog_identity(&fixture.options, "source", "local-proof")
                .unwrap()
                .unwrap();
            assert_eq!(identity.source_refs[0].state, "stale");
            let mut query =
                crate::Query::with_machines(vec![("source".into(), fixture.options.clone())]);
            for tool in ["get_session", "read_transcript"] {
                let args = if tool == "get_session" {
                    json!({"id":"local-proof"})
                } else {
                    json!({"id":"local-proof","after":0,"limit":1})
                };
                crate::events::FILE_READS.with(|n| n.set(0));
                crate::events::HASHED_BYTES.with(|n| n.set(0));
                let data = query
                    .call_selected(
                        "source",
                        "local-proof",
                        tool,
                        &args,
                        None,
                        crate::SessionSourceReadMode::LocalOrPinnedProvider,
                    )
                    .unwrap();
                assert_eq!(
                    crate::events::FILE_READS.with(std::cell::Cell::get),
                    0,
                    "focused {tool} must not decode the complete Event Index"
                );
                assert_eq!(
                    crate::events::HASHED_BYTES.with(std::cell::Cell::get),
                    0,
                    "focused {tool} must not hash the complete prefix"
                );
                assert!(data.get("error").is_none(), "{tool}: {data}");
                assert!(
                    data.to_string().contains("local original prompt"),
                    "{tool}: {data}"
                );
            }
        }
    }
}

#[test]
fn local_only_provisional_codex_refuses_corruption_truncation_races_and_missing_proofs() {
    for mutation in ["rewrite", "truncate", "race", "missing_proofs"] {
        let (fixture, path) = local_only_codex_fixture();
        match mutation {
            "rewrite" => {
                let mut bytes = fs::read(&path).unwrap();
                let at = bytes
                    .windows(b"local original prompt".len())
                    .position(|b| b == b"local original prompt")
                    .unwrap();
                bytes[at] = b'X';
                fs::write(&path, bytes).unwrap();
            }
            "truncate" => fs::write(&path, b"{}\n").unwrap(),
            "race" => {
                let raced = path.clone();
                crate::transcript_range::AFTER_LOCAL_READ.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        use std::io::Write;
                        fs::OpenOptions::new()
                            .append(true)
                            .open(raced)
                            .unwrap()
                            .write_all(b"race")
                            .unwrap();
                    }))
                });
            }
            "missing_proofs" => {
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(2))
                    .unwrap();
                fixture
                    .db()
                    .execute(
                        "DELETE FROM meta WHERE key LIKE 'native_source_chunks:%'",
                        [],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let page = crate::session_transcript_range_with_mode(
            &fixture.options,
            "source",
            "sid=local-proof&after=0&limit=2",
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
        );
        assert_eq!(page.status, 409, "{mutation}");
        let page: Value = serde_json::from_slice(&page.body).unwrap();
        assert_eq!(page["fence_reason"], "local_source_revision");
        assert!(
            page.get("entries").is_none(),
            "no partial success: {mutation}"
        );
    }
}

#[test]
fn pinned_provider_never_hides_a_concurrent_local_read_mutation() {
    use std::io::Write;
    let (fixture, path) = local_only_codex_fixture();
    let identity = session_catalog_identity(&fixture.options, "source", "local-proof")
        .unwrap()
        .unwrap();
    let provider = PinnedOriginal {
        source: identity.source_refs[0].source.clone(),
        bytes: fs::read(&path).unwrap(),
        calls: std::sync::atomic::AtomicUsize::new(0),
        change_at: None,
    };
    crate::transcript_range::AFTER_LOCAL_READ.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            fs::OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"race")
                .unwrap();
        }))
    });
    let page = crate::session_transcript_range_with_mode(
        &fixture.options,
        "source",
        "sid=local-proof&after=0&limit=1",
        Some(&provider),
        crate::SessionSourceReadMode::LocalOrPinnedProvider,
    );
    assert_eq!(page.status, 409);
    let page: Value = serde_json::from_slice(&page.body).unwrap();
    assert_eq!(page["fence_reason"], "local_source_revision");
    assert!(page.get("entries").is_none());
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
}

#[test]
fn pinned_provider_recovers_stale_codex_pages_and_fences_continuations() {
    use std::io::Write;
    for scope in ["current", "retained_history"] {
        for mutation in ["append", "replace", "legacy"] {
            let fixture = Fixture::new();
            let relative = fixture.codex_source("pinned", None, 1);
            let path = fixture.options.codex_home.join(&relative);
            let answer = |text: &str| json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}});
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file, "{}", json!({"type":"response_item","timestamp":"2026-10-01T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"original pinned prompt"}]}})).unwrap();
            writeln!(file, "{}", answer("original pinned answer")).unwrap();
            drop(file);
            let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
            facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.clone()]));
            crate::write_facts(
                fixture.options.facts.as_ref().unwrap(),
                &facts,
                &crate::Options::default().source,
            )
            .unwrap();
            // Match a provisional selected owner while complete discovery
            // is still pending, with real source-qualified paged recipes.
            {
                let mut cache =
                    EventCache::open_scoped(&fixture.options.cache, &fixture.options.source)
                        .unwrap();
                let prepared = crate::model::prepare_sources(
                    &fixture.options,
                    &mut cache,
                    &[crate::inputs::Input {
                        root: crate::inputs::InputRoot::Codex,
                        path: relative,
                    }],
                    crate::model::now_ms(),
                )
                .unwrap();
                assert_eq!(
                    cache.publish_partial_catalog(&prepared).unwrap(),
                    crate::events::Outcome::Written
                );
            }
            let before = session_catalog_identity(&fixture.options, "source", "pinned")
                .unwrap()
                .unwrap();
            assert_eq!(
                before.owner_qualification,
                CatalogOwnerQualification::Provisional
            );
            let original = fs::read(&path).unwrap();
            let provider = PinnedOriginal {
                source: before.source_refs[0].source.clone(),
                // An independently qualified archived object can extend
                // beyond the selected consumed prefix. Never read its tail.
                bytes: [original.as_slice(), b"{\"unconsumed\":true}"].concat(),
                calls: std::sync::atomic::AtomicUsize::new(0),
                change_at: None,
            };
            let query = format!("sid=pinned&scope={scope}&after=0&limit=1");
            let get = |query: &str, mode| {
                crate::session_transcript_range_with_mode(
                    &fixture.options,
                    "source",
                    query,
                    Some(&provider),
                    mode,
                )
            };
            let initial = get(&query, crate::SessionSourceReadMode::LocalThenProvider);
            assert_eq!(initial.status, 200);
            let initial: Value = serde_json::from_slice(&initial.body).unwrap();
            match mutation {
                "append" => {
                    writeln!(
                        fs::OpenOptions::new().append(true).open(&path).unwrap(),
                        "{}",
                        answer("natural appended answer")
                    )
                    .unwrap();
                }
                "replace" => {
                    fs::rename(&path, path.with_extension("saved")).unwrap();
                    fs::write(&path, "{}\n").unwrap();
                }
                "legacy" => {
                    // A supported persisted recipe predating the physical
                    // change-time witness has no local generation proof.
                    let mut db = fixture.db();
                    let text: String = db
                        .query_row(
                            "SELECT metadata FROM session_catalog WHERE session_key='pinned'",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    let mut row: CatalogRow = serde_json::from_str(&text).unwrap();
                    row.sources[0].changed_ns = None;
                    let slots: Vec<String> = db.prepare("SELECT metadata FROM session_slots WHERE session_key='pinned' ORDER BY slot").unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
                    let generation =
                        crate::slot_projection::generation(&slots, &row.sources).unwrap();
                    let source_generation =
                        crate::retention::source_generation(&row.sources).unwrap();
                    let tx = db.transaction().unwrap();
                    tx.execute(
                        "UPDATE session_catalog SET metadata=?1 WHERE session_key='pinned'",
                        [serde_json::to_string(&row).unwrap()],
                    )
                    .unwrap();
                    tx.execute("UPDATE session_slot_projections SET generation=?1,source_generation=?2 WHERE session_key='pinned'", rusqlite::params![generation, source_generation]).unwrap();
                    tx.commit().unwrap();
                }
                _ => unreachable!(),
            }
            let stale = get(&query, crate::SessionSourceReadMode::LocalThenProvider);
            assert_eq!(stale.status, 409, "{scope}/{mutation}");
            let stale: Value = serde_json::from_slice(&stale.body).unwrap();
            assert_eq!(stale["fence_reason"], "local_source_revision");
            assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
            // Metadata-only observation succeeds in the same stale cache;
            // it does not establish that the transcript body was readable.
            assert_eq!(
                crate::session_annotations(
                    &fixture.options,
                    "source",
                    "sid=pinned&scope=retained_history&kind=errors&limit=1"
                )
                .status,
                200
            );
            crate::events::CACHE_READS.with(|reads| reads.set(0));
            crate::events::PARSED.with(|parsed| parsed.set(0));
            crate::events::SOURCE_HASHED_BYTES.with(|bytes| bytes.set(0));
            let recovered = get(&query, crate::SessionSourceReadMode::LocalOrPinnedProvider);
            assert_eq!(
                recovered.status,
                200,
                "{scope}/{mutation}: {}",
                String::from_utf8_lossy(&recovered.body)
            );
            let recovered: Value = serde_json::from_slice(&recovered.body).unwrap();
            assert_eq!(recovered["entries"][0]["text"], "original pinned prompt");
            assert!(!recovered.to_string().contains("unconsumed"));
            assert!(
                recovered["content_observation"]["sources"][0]["length"]
                    .as_u64()
                    .unwrap()
                    >= provider.source.offset
            );
            assert_eq!(
                recovered["entries"][0]["entry_id"],
                initial["entries"][0]["entry_id"]
            );
            assert_eq!(
                recovered["entries"][0]["provenance"],
                initial["entries"][0]["provenance"]
            );
            if mutation != "legacy" {
                assert_eq!(recovered["projection"], initial["projection"]);
            }
            assert!(recovered["observation"]["source_bytes"].as_u64().unwrap() <= 8192);
            crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
            crate::events::PARSED.with(|parsed| assert_eq!(parsed.get(), 0));
            crate::events::SOURCE_HASHED_BYTES
                .with(|bytes| assert!(bytes.get() <= 2 * crate::source_chunks::SIZE as u64));
            let generation = recovered["projection"]["generation"].as_str().unwrap();
            let continuation =
                format!("sid=pinned&scope={scope}&after=1&limit=1&generation={generation}");
            let next = get(
                &continuation,
                crate::SessionSourceReadMode::LocalOrPinnedProvider,
            );
            assert_eq!(next.status, 200);
            let next: Value = serde_json::from_slice(&next.body).unwrap();
            assert_eq!(next["entries"][0]["text"], "original pinned answer");
            let calls = provider.calls.load(Ordering::Relaxed);
            let stale = get(
                &continuation.replace(generation, "wrong"),
                crate::SessionSourceReadMode::LocalOrPinnedProvider,
            );
            assert_eq!(stale.status, 409);
            assert_eq!(provider.calls.load(Ordering::Relaxed), calls);
            if mutation == "append" {
                // Publishing the natural append admits its new recipes;
                // the old projection cursor still cannot select them.
                fixture.publish_existing();
                let fresh = get(
                    &format!("sid=pinned&scope={scope}&after=0&limit=100"),
                    crate::SessionSourceReadMode::LocalOrPinnedProvider,
                );
                assert_eq!(fresh.status, 200);
                let fresh: Value = serde_json::from_slice(&fresh.body).unwrap();
                assert!(
                    fresh["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["text"] == "natural appended answer")
                );
                assert_ne!(fresh["projection"]["generation"], generation);
                assert_eq!(
                    get(
                        &continuation,
                        crate::SessionSourceReadMode::LocalOrPinnedProvider
                    )
                    .status,
                    409
                );
                assert_eq!(provider.calls.load(Ordering::Relaxed), calls);
            }
        }
    }
}

#[test]
fn pinned_provider_rejects_changed_original_without_publishing_a_partial_page() {
    use std::io::Write;
    let fixture = Fixture::new();
    let path = fixture.source("pinned");
    writeln!(fs::OpenOptions::new().append(true).open(&path).unwrap(), "{}", json!({"type":"assistant","sessionId":"pinned","uuid":"answer","parentUuid":"pinned","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"original answer"}]}})).unwrap();
    fixture.publish_existing();
    let source = session_catalog_identity(&fixture.options, "source", "pinned")
        .unwrap()
        .unwrap()
        .source_refs[0]
        .source
        .clone();
    let mut provider = PinnedOriginal {
        source,
        bytes: fs::read(&path).unwrap(),
        calls: std::sync::atomic::AtomicUsize::new(0),
        change_at: Some(1),
    };
    fs::write(&path, "{}\n").unwrap();
    for wrong_reference in [false, true] {
        provider.calls.store(0, Ordering::Relaxed);
        if wrong_reference {
            provider.source.prefix_sha256[0] ^= 1;
        }
        let reply = crate::session_transcript_range_with_mode(
            &fixture.options,
            "source",
            "sid=pinned&after=0&limit=2",
            Some(&provider),
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
        );
        assert_eq!(reply.status, 409);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["fence_reason"], "source_generation");
        assert!(body.get("entries").is_none());
    }
}

#[test]
fn pinned_provider_keeps_each_carrier_fixed_after_its_first_bytes() {
    let fixture = Fixture::new();
    let archived = fixture.source("archived");
    let live = fixture.source("live");
    fixture.publish_existing();
    let db = fixture.db();
    let mut sources = Vec::new();
    for key in ["archived", "live"] {
        let text: String = db
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        sources.extend(serde_json::from_str::<CatalogRow>(&text).unwrap().sources);
    }
    let source = session_catalog_identity(&fixture.options, "source", "archived")
        .unwrap()
        .unwrap()
        .source_refs[0]
        .source
        .clone();
    let provider = PinnedOriginal {
        source,
        bytes: fs::read(&archived).unwrap(),
        calls: std::sync::atomic::AtomicUsize::new(0),
        change_at: None,
    };
    fs::write(&archived, "{}\n").unwrap();
    crate::transcript_range::with_source_reader(
        &fixture.options,
        "claude",
        &sources,
        Some(&provider),
        crate::SessionSourceReadMode::LocalOrPinnedProvider,
        |reader, references| {
            let archive_ref = &references[&archived];
            let part = reader.read_range(archive_ref, None, 0, 8)?;
            assert_eq!(&*part.bytes, &provider.bytes[..8]);
            let live_ref = &references[&live];
            let local = reader.read_range(live_ref, None, 0, 8)?;
            assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
            // Even another before-read mismatch must not switch an
            // already admitted local carrier over to an original.
            fs::write(&live, "{}\n")?;
            assert_eq!(
                reader
                    .read_range(live_ref, Some(&local.generation), 8, 8)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
            let next = reader.read_range(archive_ref, Some(&part.generation), 8, 8)?;
            assert_eq!(&*next.bytes, &provider.bytes[8..16]);
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn current_page_excludes_retained_rows_with_bounded_work_and_history_keeps_them() {
    let fixture = Fixture::new();
    fixture.publish(2);
    let connection = fixture.db();
    connection.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='session-00001'",[]).unwrap();
    SQL_STEPS.with(|steps| steps.set(0));
    let (status, before) = fixture.body("limit=1");
    assert_eq!(status, 200);
    let baseline = SQL_STEPS.with(|steps| steps.get());
    assert_eq!(before["items"].as_array().unwrap().len(), 1);
    assert_eq!(before["items"][0]["key"], "session-00000");
    connection.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO session_catalog(namespace,session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) SELECT namespace,'retained-'||printf('%05d',x),'retained',last_ms,harness,repo,parent_key,json_set(metadata,'$.key','retained-'||printf('%05d',x)) FROM n,session_catalog WHERE session_key='session-00001'").unwrap();
    SQL_STEPS.with(|steps| steps.set(0));
    let (status, after) = fixture.body("limit=1");
    assert_eq!(status, 200);
    assert_eq!(after["items"], before["items"]);
    assert_eq!(SQL_STEPS.with(|steps| steps.get()), baseline);
    let (status, history) = fixture.body("scope=retained_history&limit=2");
    assert_eq!(status, 200);
    assert_eq!(history["items"].as_array().unwrap().len(), 2);
    assert!(history["next_cursor"].is_string());
    // Local absence does not revoke native-current authority by itself.
    assert!(
        session_catalog_identity(&fixture.options, "source", "session-00001")
            .unwrap()
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn legacy_source_metadata_does_not_invent_a_local_generation_proof() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let connection = fixture.db();
    let metadata: String = connection
        .query_row("SELECT metadata FROM session_catalog LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let row: crate::model::summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
    let mut source = row.sources[0].clone();
    assert_eq!(source_state(&source), "cached");
    source.changed_ns = None;
    let legacy = serde_json::to_value(&source).unwrap();
    assert!(legacy.get("changed_ns").is_none());
    let migrated: CatalogSource = serde_json::from_value(legacy).unwrap();
    assert_eq!(source, migrated);
    assert_eq!(source_state(&migrated), "stale");
}

#[cfg(unix)]
#[test]
fn restored_mtime_rewrite_invalidates_local_ranges_and_rebuilds_observation() {
    use std::io::Write;
    let fixture = Fixture::new();
    fixture.publish(1);
    let path = fixture
        .options
        .claude_home
        .join("projects/project/session-00000.jsonl");
    let before = fs::metadata(&path).unwrap();
    let original = fs::read_to_string(&path).unwrap();
    let rewritten = original.replace("synthetic prompt", "rewritten prompt");
    assert_eq!(original.len(), rewritten.len());
    let old = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=session-00000&after=0&limit=1",
        None,
    );
    let old: Value = serde_json::from_slice(&old.body).unwrap();
    assert_eq!(old["entries"][0]["text"], "synthetic prompt");
    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.write_all(rewritten.as_bytes()).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    let after = fs::metadata(&path).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        (
            before.dev(),
            before.ino(),
            before.len(),
            before.modified().unwrap()
        ),
        (
            after.dev(),
            after.ino(),
            after.len(),
            after.modified().unwrap()
        )
    );
    assert_ne!(
        crate::events::change_time_ns(&before),
        crate::events::change_time_ns(&after)
    );
    let stale = session_catalog_identity(&fixture.options, "source", "session-00000")
        .unwrap()
        .unwrap();
    assert_eq!(stale.source_refs[0].state, "stale");
    let range = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=session-00000&after=0&limit=1",
        None,
    );
    assert_eq!(range.status, 409);
    let range: Value = serde_json::from_slice(&range.body).unwrap();
    assert_eq!(range["error"]["code"], "stale_projection");
    assert_eq!(range["resynchronize"], true);
    assert!(!range.to_string().contains("rewritten prompt"));
    fixture.publish_existing();
    let fresh = crate::session_transcript_range(
        &fixture.options,
        "source",
        "sid=session-00000&after=0&limit=1",
        None,
    );
    let fresh: Value = serde_json::from_slice(&fresh.body).unwrap();
    assert_eq!(fresh["entries"][0]["text"], "rewritten prompt");
    assert_eq!(fresh["freshness"]["state"], "cached");
    assert!(fresh["projection"]["generation"].is_string());
    assert_ne!(
        fresh["projection"]["generation"],
        old["projection"]["generation"]
    );
}

#[test]
fn list_status_reuses_native_facts_and_rejects_runtime_or_source_changes() {
    let fixture = Fixture::new();
    fixture.publish(1);
    native_refresh(&fixture);
    let connection = fixture.db();
    let metadata: String = connection
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='session-00000'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let original: Value = serde_json::from_str(&metadata).unwrap();
    assert!(original["query"].is_object());
    let save = |row: &Value| {
        connection.execute(
                "UPDATE session_catalog SET lifecycle=?1,metadata=?2 WHERE session_key='session-00000'",
                rusqlite::params![row["lifecycle"].as_str().unwrap(), row.to_string()],
            ).unwrap();
    };
    let status = |scope: &str| {
        let before = crate::events::CACHE_READS.with(|reads| reads.get());
        let (code, body) = fixture.body(scope);
        assert_eq!(code, 200);
        assert_eq!(body["capabilities"]["runtime_status"], true);
        let row = &body["items"][0];
        assert!(row.get("query").is_none());
        assert!(row.get("details").is_none());
        assert!(row.get("transcript").is_none());
        assert!(!body.to_string().contains("synthetic prompt"));
        assert_eq!(crate::events::CACHE_READS.with(|reads| reads.get()), before);
        row["status"].clone()
    };
    crate::events::CACHE_READS.with(|reads| reads.set(0));
    for state in ["work", "wait", "idle", "done", "err", "new", "read", "off"] {
        let mut row = original.clone();
        row["query"]["state"] = json!(state);
        save(&row);
        assert_eq!(status(""), json!({"state":state,"observation":"current"}));
        row["lifecycle"] = json!("retained");
        save(&row);
        let expected = if matches!(state, "work" | "wait") {
            "off"
        } else {
            state
        };
        assert_eq!(
            status("scope=retained_history"),
            json!({"state":expected,"observation":"retained"})
        );
    }
    for (field, value) in [
        ("runtime", json!("old-runtime")),
        ("source_generation", json!("old-source")),
        ("version", json!(0)),
    ] {
        let mut row = original.clone();
        row["query"][field] = value;
        save(&row);
        assert_eq!(
            status(""),
            json!({"state":null,"observation":"unavailable"})
        );
        let (_, body) = fixture.body("");
        assert_eq!(
            body["items"][0]["facts_observation"]["state"],
            if field == "runtime" {
                "stale"
            } else {
                "cached"
            }
        );
    }
    save(&original);
    let facts = fixture.options.facts.as_ref().unwrap();
    let mut changed: Value = serde_json::from_slice(&fs::read(facts).unwrap()).unwrap();
    changed["hostname"] = json!("changed-runtime-input");
    fs::write(facts, changed.to_string()).unwrap();
    assert_eq!(
        status(""),
        json!({"state":null,"observation":"unavailable"})
    );
    let (_, unavailable) = fixture.body("");
    assert_eq!(
        unavailable["items"][0]["facts_observation"]["state"],
        "unavailable"
    );
    // A valid atomic facts publication restores source authority while its
    // existing complete Catalog still carries the prior runtime fence.
    crate::write_facts(
        facts,
        &serde_json::from_value::<crate::Facts>(changed).unwrap(),
        &crate::Options::default().source,
    )
    .unwrap();
    let (_, pending) = fixture.body("");
    assert_eq!(pending["completeness"]["state"], "complete");
    assert_eq!(pending["freshness"], "stale");
    assert_eq!(pending["items"][0]["facts_observation"]["state"], "stale");
    fixture.publish_existing();
    assert_ne!(status("")["state"], Value::Null);
    let (_, refreshed) = fixture.body("");
    assert_eq!(refreshed["freshness"], "cached");
    assert_eq!(
        refreshed["items"][0]["facts_observation"]["state"],
        "cached"
    );
    let source = fixture
        .options
        .claude_home
        .join("projects/project/session-00000.jsonl");
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new().append(true).open(source).unwrap(),
        "{{}}"
    )
    .unwrap();
    assert_eq!(
        status(""),
        json!({"state":null,"observation":"unavailable"})
    );
    // Each status read above checks that it loads no Event Index rows.
}

#[test]
fn family_and_unjoined_trace_extent_share_source_runtime_and_retained_qualification() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.options.claude_home.join("sessions")).unwrap();
    fs::write(
            fixture.options.claude_home.join("sessions/77.json"),
            json!({"pid":77,"sessionId":"session-00001","procStart":777,"status":"busy","name":"Family child"}).to_string(),
        )
        .unwrap();
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut initial_facts = crate::read_facts(facts_path).unwrap();
    initial_facts.proc_starts.insert(77, 777);
    crate::write_facts(
        facts_path,
        &initial_facts,
        &crate::Options::default().source,
    )
    .unwrap();
    fixture.publish(2);
    native_refresh(&fixture);
    let db = fixture.db();
    let (turn_id, turn_metadata): (String, String) = db.query_row(
            "SELECT turn_id,metadata FROM session_trace_turns WHERE session_key='session-00000' ORDER BY turn_id LIMIT 1",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
    let mut turn: Value = serde_json::from_str(&turn_metadata).unwrap();
    turn["sent"] = json!(["qualified-peer-relay"]);
    db.execute(
            "UPDATE session_trace_turns SET metadata=?1 WHERE session_key='session-00000' AND turn_id=?2",
            rusqlite::params![turn.to_string(), turn_id],
        ).unwrap();
    db.execute(
            "INSERT INTO session_trace_edges(namespace,session_key,edge_id,metadata) VALUES(semon_namespace(),'session-00000','qualified-peer-relay',?1)",
            [json!({"id":"qualified-peer-relay","kind":"relay","from":"session-00000","to":"session-00001","at":1790812800000i64,"status":"done"}).to_string()],
        ).unwrap();
    let trace_query = format!(
        "sid=session-00000&scope=retained_history&turn={}",
        crate::viewer::percent_encode(&turn_id)
    );
    let trace = || {
        let reply = crate::session_causal_trace(&fixture.options, "local", &trace_query);
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        serde_json::from_slice::<Value>(&reply.body).unwrap()
    };
    let activity_reads = crate::events::CACHE_READS.with(std::cell::Cell::get);
    let peer_header: String = db
        .query_row(
            "SELECT metadata FROM session_trace_headers WHERE session_key='session-00001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut peer_header: Value = serde_json::from_str(&peer_header).unwrap();
    peer_header["busy"] = json!([[10, 20], [30, 40]]);
    let save_peer_header = |header: &Value| {
        db.execute(
            "UPDATE session_trace_headers SET metadata=?1 WHERE session_key='session-00001'",
            [header.to_string()],
        )
        .unwrap();
    };
    save_peer_header(&peer_header);
    assert_eq!(
        trace()["participant_summaries"]["session-00001"]["observed_extent"]["recorded_activity"],
        json!({"state":"available","kind":"timestamp_clusters","intervals":[[10,20],[30,40]]})
    );
    let mut empty = peer_header.clone();
    empty["busy"] = json!([]);
    save_peer_header(&empty);
    assert_eq!(
        trace()["participant_summaries"]["session-00001"]["observed_extent"]["recorded_activity"],
        json!({"state":"available","kind":"timestamp_clusters","intervals":[]})
    );
    db.execute(
        "UPDATE session_trace_headers SET metadata=?1 WHERE session_key='session-00001'",
        ["{"],
    )
    .unwrap();
    assert_eq!(
        trace()["participant_summaries"]["session-00001"]["observed_extent"]["recorded_activity"],
        json!({"state":"unavailable"})
    );
    for busy in [
        Value::Null,
        json!(vec![(0, 0); 257]),
        json!([[20, 10]]),
        json!([[10, 20], [15, 30]]),
        json!(["x".repeat(16 * 1024)]),
    ] {
        let mut invalid = peer_header.clone();
        invalid["busy"] = busy;
        save_peer_header(&invalid);
        let unavailable = trace();
        assert_eq!(
            unavailable["participant_summaries"]["session-00001"]["observed_extent"]["recorded_activity"],
            json!({"state":"unavailable"})
        );
        // Activity availability never rewrites causal missing-context facts.
        assert_eq!(unavailable["incomplete"], true);
        assert_eq!(unavailable["incomplete_only_missing_receiving_turns"], true);
    }
    let mut oversized = peer_header.clone();
    oversized["padding"] = json!("x".repeat(1024 * 1024));
    save_peer_header(&oversized);
    assert_eq!(
        trace()["participant_summaries"]["session-00001"]["observed_extent"]["recorded_activity"],
        json!({"state":"unavailable"})
    );
    save_peer_header(&peer_header);
    assert_eq!(
        crate::events::CACHE_READS.with(std::cell::Cell::get),
        activity_reads
    );
    assert_eq!(trace()["incomplete_only_missing_receiving_turns"], true);
    for tid in ["ambiguous-one", "ambiguous-two"] {
        db.execute(
                "INSERT INTO session_trace_turns(namespace,session_key,turn_id,start_id,metadata) VALUES(semon_namespace(),'session-00001',?1,'qualified-peer-relay','{}')",
                [tid],
            ).unwrap();
    }
    let ambiguous = trace();
    assert_eq!(ambiguous["incomplete"], true);
    assert_eq!(ambiguous["incomplete_only_missing_receiving_turns"], false);
    assert!(ambiguous["sessions"].get("session-00001").is_none());
    db.execute("DELETE FROM session_trace_turns WHERE session_key='session-00001' AND start_id='qualified-peer-relay'", []).unwrap();
    let header: String = db
        .query_row(
            "SELECT metadata FROM session_trace_headers WHERE session_key='session-00000'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut incomplete_header: Value = serde_json::from_str(&header).unwrap();
    incomplete_header["relationships_incomplete"] = json!(true);
    db.execute(
        "UPDATE session_trace_headers SET metadata=?1 WHERE session_key='session-00000'",
        [incomplete_header.to_string()],
    )
    .unwrap();
    assert_eq!(trace()["incomplete_only_missing_receiving_turns"], false);
    incomplete_header["relationships_incomplete"] = json!(false);
    incomplete_header["content_generation"] = Value::Null;
    db.execute(
        "UPDATE session_trace_headers SET metadata=?1 WHERE session_key='session-00000'",
        [incomplete_header.to_string()],
    )
    .unwrap();
    let no_preview = trace();
    assert_eq!(no_preview["previews_incomplete"], true);
    assert_eq!(no_preview["incomplete_only_missing_receiving_turns"], false);
    db.execute(
        "UPDATE session_trace_headers SET metadata=?1 WHERE session_key='session-00000'",
        [header],
    )
    .unwrap();
    let text: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='session-00001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut original: Value = serde_json::from_str(&text).unwrap();
    original["parent"] = json!("session-00000");
    let save = |row: &Value| {
        db.execute(
                "UPDATE session_catalog SET parent_key='session-00000',lifecycle=?1,metadata=?2 WHERE session_key='session-00001'",
                rusqlite::params![row["lifecycle"].as_str().unwrap(), row.to_string()],
            )
            .unwrap();
        db.execute(
            "UPDATE session_history_catalog SET metadata=?1 WHERE session_key='session-00001'",
            [row.to_string()],
        )
        .unwrap();
    };
    let status = || {
        let reads = crate::events::CACHE_READS.with(std::cell::Cell::get);
        let (code, body) = fixture.body("sid=session-00000");
        assert_eq!(code, 200);
        let family = &body["items"][0]["details"]["family"];
        assert_eq!(family["state"], "complete");
        let child = family["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == "session-00001")
            .unwrap();
        for private in ["query", "sources", "_status_catalog"] {
            assert!(child.get(private).is_none());
        }
        assert!(!body.to_string().contains("synthetic prompt"));
        let trace = crate::session_causal_trace(&fixture.options, "local", &trace_query);
        assert_eq!(
            trace.status,
            200,
            "{}",
            String::from_utf8_lossy(&trace.body)
        );
        let trace: Value = serde_json::from_slice(&trace.body).unwrap();
        let extent = &trace["participant_summaries"]["session-00001"]["observed_extent"];
        assert_eq!(extent["scope"], "whole_session");
        assert_eq!(extent["status"], child["status"]);
        assert_eq!(
            extent["recorded_activity"],
            if child["status"]["observation"] == "unavailable" {
                json!({"state":"unavailable"})
            } else {
                json!({"state":"available","kind":"timestamp_clusters","intervals":[[10,20],[30,40]]})
            }
        );
        assert_eq!(trace["incomplete"], true);
        assert_eq!(
            trace["incomplete_only_missing_receiving_turns"],
            child["status"]["observation"] != "unavailable"
        );
        assert!(trace["sessions"].get("session-00001").is_none());
        assert!(
            trace["turns"]
                .as_array()
                .unwrap()
                .iter()
                .all(|turn| turn["sid"] == "session-00000")
        );
        for private in ["query", "sources", "busy", "wait_edges", "turns"] {
            assert!(
                trace["participant_summaries"]["session-00001"]
                    .get(private)
                    .is_none()
            );
        }
        assert!(!trace.to_string().contains("synthetic prompt"));
        assert_eq!(crate::events::CACHE_READS.with(std::cell::Cell::get), reads);
        child["status"].clone()
    };
    for state in ["work", "wait", "idle", "done", "err", "new", "read", "off"] {
        let mut row = original.clone();
        row["query"]["state"] = json!(state);
        save(&row);
        assert_eq!(status(), json!({"state":state,"observation":"current"}));
        row["lifecycle"] = json!("retained");
        save(&row);
        let retained = if matches!(state, "work" | "wait") {
            "off"
        } else {
            state
        };
        assert_eq!(status(), json!({"state":retained,"observation":"retained"}));
    }
    for (field, value) in [
        ("source_generation", json!("old-source")),
        ("runtime", json!("old-runtime")),
        ("version", json!(0)),
    ] {
        let mut row = original.clone();
        row["query"]["state"] = json!("work");
        row["query"][field] = value;
        save(&row);
        assert_eq!(status(), json!({"state":null,"observation":"unavailable"}));
    }
    original["query"]["state"] = json!("work");
    save(&original);
    let path = fixture.options.facts.as_ref().unwrap();
    let facts = crate::read_facts(path).unwrap();
    let mut changed = facts.clone();
    changed.proc_starts.insert(77, 778);
    crate::write_facts(path, &changed, &crate::Options::default().source).unwrap();
    assert_eq!(status(), json!({"state":null,"observation":"unavailable"}));
    crate::write_facts(path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(status(), json!({"state":"work","observation":"current"}));
    let mut changed = facts.clone();
    changed.hostname = "new-family-runtime".into();
    crate::write_facts(path, &changed, &crate::Options::default().source).unwrap();
    assert_eq!(status(), json!({"state":null,"observation":"unavailable"}));
    crate::write_facts(path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(status(), json!({"state":"work","observation":"current"}));
    fs::write(path, "{}").unwrap();
    assert_eq!(status(), json!({"state":null,"observation":"unavailable"}));
    crate::write_facts(path, &facts, &crate::Options::default().source).unwrap();
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(
                fixture
                    .options
                    .claude_home
                    .join("projects/project/session-00001.jsonl")
            )
            .unwrap(),
        "{{}}"
    )
    .unwrap();
    assert_eq!(status(), json!({"state":null,"observation":"unavailable"}));
}

#[test]
fn unjoined_trace_activity_headers_share_the_existing_decoded_metadata_budget() {
    let fixture = Fixture::new();
    fixture.publish(6);
    native_refresh(&fixture);
    let db = fixture.db();
    let (turn_id, encoded): (String,String) = db.query_row("SELECT turn_id,metadata FROM session_trace_turns WHERE session_key='session-00000' ORDER BY turn_id LIMIT 1", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    let ids: Vec<_> = (1..6).map(|n| format!("peer-relay-{n}")).collect();
    let mut turn: Value = serde_json::from_str(&encoded).unwrap();
    turn["sent"] = json!(ids);
    db.execute("UPDATE session_trace_turns SET metadata=?1 WHERE session_key='session-00000' AND turn_id=?2", rusqlite::params![turn.to_string(),turn_id]).unwrap();
    for n in 1..6 {
        let key = format!("session-{n:05}");
        let id = format!("peer-relay-{n}");
        db.execute("INSERT INTO session_trace_edges(namespace,session_key,edge_id,metadata) VALUES(semon_namespace(),'session-00000',?1,?2)", rusqlite::params![id,json!({"id":id,"kind":"relay","from":"session-00000","to":key,"at":1790812800000i64,"status":"done"}).to_string()]).unwrap();
        let encoded: String = db
            .query_row(
                "SELECT metadata FROM session_trace_headers WHERE session_key=?1",
                [&key],
                |row| row.get(0),
            )
            .unwrap();
        let mut header: Value = serde_json::from_str(&encoded).unwrap();
        header["busy"] = json!([[10, 20]]);
        header["padding"] = json!("x".repeat(900_000));
        let encoded = header.to_string();
        assert!(encoded.len() < 1024 * 1024);
        db.execute(
            "UPDATE session_trace_headers SET metadata=?1 WHERE session_key=?2",
            rusqlite::params![encoded, key],
        )
        .unwrap();
    }
    let reads = crate::events::CACHE_READS.with(std::cell::Cell::get);
    let assert_budget = || {
        let reply = crate::session_causal_trace(
            &fixture.options,
            "local",
            &format!(
                "sid=session-00000&scope=retained_history&turn={}",
                crate::viewer::percent_encode(&turn_id)
            ),
        );
        assert_eq!(reply.status, 413);
    };
    assert_budget();
    for table in ["session_catalog", "session_history_catalog"] {
        db.execute(&format!("UPDATE {table} SET metadata=json_set(metadata,'$.query.runtime','unavailable-runtime') WHERE session_key!='session-00000'"), []).unwrap();
    }
    assert_eq!(
        fixture.body("sid=session-00001").1["items"][0]["status"]["observation"],
        "unavailable"
    );
    assert_budget();
    db.execute("UPDATE session_trace_headers SET source_generation='mismatched-source' WHERE session_key!='session-00000'", []).unwrap();
    assert_budget();
    assert_eq!(crate::events::CACHE_READS.with(std::cell::Cell::get), reads);
}

#[test]
fn family_status_does_not_treat_a_retired_native_selection_as_current() {
    let fixture = Fixture::new();
    let parent = fixture.codex_source("family-parent", None, 1);
    let child = fixture.codex_source("family-child", Some("family-parent"), 2);
    let path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(path).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([parent.clone(), child]));
    crate::write_facts(path, &facts, &crate::Options::default().source).unwrap();
    fixture.publish_existing();
    let db = fixture.db();
    let text: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='family-child'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut row: Value = serde_json::from_str(&text).unwrap();
    row["query"]["state"] = json!("work");
    let save = |row: &Value| {
        db.execute(
            "UPDATE session_catalog SET lifecycle=?1,metadata=?2 WHERE session_key='family-child'",
            rusqlite::params![row["lifecycle"].as_str().unwrap(), row.to_string()],
        )
        .unwrap();
    };
    save(&row);
    let status = || {
        let (code, body) = fixture.body("sid=family-parent");
        assert_eq!(code, 200);
        body["items"][0]["details"]["family"]["runs"][0]["status"].clone()
    };
    assert_eq!(status(), json!({"state":"work","observation":"current"}));
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([parent]));
    crate::write_facts(path, &facts, &crate::Options::default().source).unwrap();
    assert_eq!(status(), json!({"state":null,"observation":"unavailable"}));
    row["lifecycle"] = json!("retained");
    save(&row);
    assert_eq!(status(), json!({"state":"off","observation":"retained"}));
}

#[test]
fn focused_cold_reads_are_indexed_metadata_and_coherent_keysets() {
    let fixture = Fixture::new();
    fixture.publish(9);
    crate::events::CACHE_READS.with(|reads| reads.set(0));
    let (status, first) = fixture.body("limit=3");
    assert_eq!(status, 200);
    assert_eq!(first["freshness"], "cached");
    assert!(
        first.get("refresh").is_none(),
        "projection reads do not own refresh scheduling"
    );
    let first_items = first["items"].as_array().unwrap();
    assert_eq!(first_items.len(), 3);
    assert_eq!(first_items[0]["key"], "session-00000");
    assert_eq!(first_items[2]["key"], "session-00002");
    assert!(
        first_items
            .iter()
            .all(|row| row["freshness"]["state"] == "cached"
                && row.get("sources").is_none()
                && row.get("activity").is_none())
    );
    let (status, second) =
        fixture.body(&format!("limit=3&{}", cursor_query(&first["next_cursor"])));
    assert_eq!(status, 200);
    assert_eq!(second["items"][0]["key"], "session-00003");
    assert_eq!(second["items"][2]["key"], "session-00005");
    let (status, last) = fixture.body(&format!("limit=3&{}", cursor_query(&second["next_cursor"])));
    assert_eq!(status, 200);
    assert_eq!(last["items"][2]["key"], "session-00008");
    assert!(last["next_cursor"].is_null());
    let (status, wrong_scope) = fixture.body(&format!(
        "harness=claude&{}",
        cursor_query(&first["next_cursor"])
    ));
    assert_eq!(status, 400);
    assert_eq!(wrong_scope["error"]["code"], "cursor_scope_mismatch");
    assert_eq!(fixture.body("harness=codex").1["items"], json!([]));
    let (status, selected) = fixture.body("sid=session-00008");
    assert_eq!(status, 200);
    assert_eq!(selected["items"].as_array().unwrap().len(), 1);
    assert_eq!(selected["items"][0]["key"], "session-00008");
    assert_eq!(fixture.body("sid=absent").0, 404);
    assert_eq!(fixture.body("sid=session-00008&harness=claude").0, 400);
    let (status, searched) = fixture.body("q=session-00008");
    assert_eq!(status, 200, "{searched}");
    assert_eq!(searched["items"].as_array().unwrap().len(), 1);
    assert_eq!(searched["items"][0]["key"], "session-00008");
    assert_eq!(searched["search"]["partial"], false);
    assert_eq!(fixture.body("q=session&q=other").0, 400);
    assert_eq!(fixture.body("q=bad%0Aquery").0, 400);
    assert_eq!(fixture.body("q=%GG").0, 400);
    assert_eq!(fixture.body("sid=session-00008&q=session").0, 400);
    assert!(
        fixture.body("q=missing").1["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(fixture.body("limit=1").1["search"].is_null());
    assert_eq!(
        fixture.body("harness=claude&repo=project").1["items"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
    crate::events::CACHE_READS.with(|reads| {
        assert_eq!(
            reads.get(),
            0,
            "catalog requests may not load the full event index"
        )
    });

    // An append/republication moves the generation; the old page cannot
    // silently select a changed logical or source snapshot.
    fixture.source("new-session");
    fixture.publish_existing();
    let (status, stale) = fixture.body(&cursor_query(&first["next_cursor"]));
    assert_eq!(status, 409);
    assert_eq!(stale["error"]["code"], "stale_cursor");
    assert_eq!(stale["reset_cursor"], true);
}

#[test]
fn focused_catalog_preserves_source_disappearance_and_access_boundaries() {
    let fixture = Fixture::new();
    fixture.publish(2);
    let source = fixture.source("session-00000");
    assert_eq!(
        fixture.body("limit=1").1["items"][0]["freshness"]["state"],
        "stale"
    );
    fs::remove_file(&source).unwrap();
    let (status, missing) = fixture.body("limit=1");
    assert_eq!(status, 200);
    assert_eq!(missing["items"][0]["freshness"]["state"], "unavailable");
    assert_eq!(
        missing["items"][0]["source_refs"][0]["state"],
        "unavailable"
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", &source).unwrap();
        assert_eq!(
            fixture.body("limit=1").1["items"][0]["freshness"]["state"],
            "unavailable"
        );
    }
    let mut changed = fixture.options.clone();
    changed.claude_home = fixture.root.join("another-claude");
    let core = ViewerCore::new(changed);
    let reply = core.respond("GET", "/api/sessions", "limit=1", None);
    assert_eq!(reply.status, 503);
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(body["error"]["code"], "catalog_scope_changed");
    assert!(
        body.get("items").is_none(),
        "metadata from a prior configuration is not source authority"
    );
    // The refused read still schedules native observation. Drain its
    // actual worker before the fixture removes its private source roots.
    core.close();
}

#[test]
fn focused_catalog_requires_an_unambiguous_machine_scope_and_bounded_arguments() {
    let fixture = Fixture::new();
    fixture.publish(1);
    let core = ViewerCore::with_machines(vec![
        ("one".into(), fixture.options.clone()),
        ("two".into(), fixture.options.clone()),
    ]);
    let body = |query| {
        let reply = core.respond("GET", "/api/sessions", query, None);
        (
            reply.status,
            serde_json::from_slice::<Value>(&reply.body).unwrap(),
        )
    };
    assert_eq!(body("").1["error"]["code"], "machine_scope_required");
    assert_eq!(body("machine=one").0, 200);
    assert_eq!(body("machine=unknown").0, 404);
    for query in [
        "limit=0",
        "limit=101",
        "limit=-1",
        "harness=unknown",
        "repo=",
        "cursor=nope",
        "repo=%FF",
    ] {
        assert_eq!(body(query).0, 400, "{query}");
    }
    let empty = Fixture::new();
    // The projection reader itself stays read-only. Viewer transport
    // explicitly demands its native producer, covered separately.
    assert_eq!(
        session_catalog_page(&empty.options, "local", "").status,
        503
    );
    assert!(
        !EventCache::path(&empty.options.cache).exists(),
        "a read-only catalog miss must not create an index"
    );
    core.close(); // Drain demanded native work before removing its scratch custody.
}

#[test]
fn successive_scoped_batches_preserve_unrelated_durable_source_membership() {
    let fixture = Fixture::new();
    let inputs: Vec<_> = ["one", "two"]
        .into_iter()
        .map(|id| {
            fixture.source(id);
            crate::inputs::Input {
                root: crate::inputs::InputRoot::Claude,
                path: format!("projects/project/{id}.jsonl"),
            }
        })
        .collect();
    let mut cache =
        EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
    let first = crate::model::prepare_sources(
        &fixture.options,
        &mut cache,
        &inputs,
        crate::model::now_ms(),
    )
    .unwrap();
    assert_eq!(
        cache.publish_partial_catalog(&first).unwrap(),
        crate::events::Outcome::Written
    );
    let second = crate::model::prepare_sources(
        &fixture.options,
        &mut cache,
        &inputs[..1],
        crate::model::now_ms(),
    )
    .unwrap();
    assert_eq!(
        cache.publish_partial_catalog(&second).unwrap(),
        crate::events::Outcome::Written
    );
    let connection = fixture.db();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(cache.paths().count(), 2);
    let body = fixture.body("sid=two").1;
    assert_eq!(body["items"][0]["freshness"]["state"], "cached");
}

#[test]
fn scoped_publication_rejects_newer_dirty_claim_and_keeps_unrelated_work() {
    let fixture = Fixture::new();
    fixture.source("selected");
    let input = crate::inputs::Input {
        root: crate::inputs::InputRoot::Claude,
        path: "projects/project/selected.jsonl".into(),
    };
    let mut cache =
        EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
    let prepared = crate::model::prepare_sources(
        &fixture.options,
        &mut cache,
        std::slice::from_ref(&input),
        crate::model::now_ms(),
    )
    .unwrap();
    let connection = fixture.db();
    connection
        .execute(
            "UPDATE session_catalog_invalidations SET revision=revision+1 WHERE source_path=?1",
            [input.full_path(&fixture.options).to_string_lossy().as_ref()],
        )
        .unwrap();
    assert_eq!(
        cache.publish_partial_catalog(&prepared).unwrap(),
        crate::events::Outcome::Conflict
    );
    assert_eq!(fixture.body("").0, 503);
    connection
        .execute(
            "INSERT INTO session_catalog_invalidations VALUES (semon_namespace(),'unrelated',999,'changed',0)",
            [],
        )
        .unwrap();
    let prepared = crate::model::prepare_sources(
        &fixture.options,
        &mut cache,
        &[input],
        crate::model::now_ms(),
    )
    .unwrap();
    assert_eq!(
        cache.publish_partial_catalog(&prepared).unwrap(),
        crate::events::Outcome::Written
    );
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_invalidations WHERE source_path='unrelated'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        pending, 1,
        "partial publication cannot ACK another source's work"
    );
}

#[test]
fn native_codex_archive_unarchive_reuses_the_indexed_generation() {
    use crate::{Input, InputRoot, SessionFactsWrite};
    use std::collections::BTreeSet;
    for unrelated in [0, 256, 2048] {
        let fixture = Fixture::new();
        let active = "sessions/2026/10/01/rollout-native-session.jsonl";
        let archived = "archived_sessions/2026/10/01/rollout-native-session.jsonl";
        let path = fixture.options.codex_home.join(active);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path,format!("{}\n{}\n",json!({"type":"session_meta","payload":{"id":"native-session","cwd":"/synthetic/project"}}),json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"synthetic prompt"}]}}))).unwrap();
        let facts_path = fixture.options.facts.as_ref().unwrap();
        let mut facts = crate::read_facts(facts_path).unwrap();
        facts.codex_rollouts = Some(BTreeSet::from([active.to_owned()]));
        crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
        for id in 0..unrelated {
            fixture.source(&format!("unrelated-{id:05}"));
        }
        fixture.publish_existing();
        let db = fixture.db();
        let original: i64 = db
            .query_row(
                "SELECT file_id FROM files WHERE path=?1",
                [path.to_string_lossy()],
                |row| row.get(0),
            )
            .unwrap();
        for (from, to, append) in [(active, archived, false), (archived, active, true)] {
            let old = fixture.options.codex_home.join(from);
            let new = fixture.options.codex_home.join(to);
            fs::create_dir_all(new.parent().unwrap()).unwrap();
            fs::rename(&old, &new).unwrap();
            if append {
                use std::io::Write;
                writeln!(fs::OpenOptions::new().append(true).open(&new).unwrap(),"{}",json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic appended response"}]}})).unwrap();
            }
            facts.codex_rollouts = Some(BTreeSet::from([to.to_owned()]));
            let intent = SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
            crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
            intent.complete().unwrap();
            crate::invalidate_session_sources(
                &fixture.options,
                &[Input {
                    root: InputRoot::Codex,
                    path: to.into(),
                }],
            )
            .unwrap();
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get);
            let records = crate::events::PARSED.with(std::cell::Cell::get);
            let started = std::time::Instant::now();
            let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
            let mut work = crate::viewer::lock(&evidence.work);
            let mut steps = 0;
            while !evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
            {
                steps += 1;
                assert!(steps < 8);
            }
            assert!(started.elapsed() < Duration::from_secs(10));
            assert_eq!(
                db.query_row(
                    "SELECT file_id FROM files WHERE path=?1",
                    [new.to_string_lossy()],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                original,
                "move replaced the indexed generation"
            );
            assert_eq!(
                crate::events::FILE_READS.with(std::cell::Cell::get) - loads,
                1,
                "move loaded unrelated indexes"
            );
            assert_eq!(
                crate::events::PARSED.with(std::cell::Cell::get) - records,
                u64::from(append),
                "move reparsed history"
            );
            let current = crate::session_transcript_range(
                &fixture.options,
                "local",
                "sid=native-session&limit=100",
                None,
            );
            let history = crate::session_transcript_range(
                &fixture.options,
                "local",
                "sid=native-session&scope=retained_history&limit=100",
                None,
            );
            assert_eq!(
                (current.status, history.status),
                (200, 200),
                "native relocation must not create unavailable retained originals"
            );
            let current: Value = serde_json::from_slice(&current.body).unwrap();
            let history: Value = serde_json::from_slice(&history.body).unwrap();
            assert_eq!(
                current["entries"].as_array().unwrap().len(),
                history["entries"].as_array().unwrap().len(),
                "native relocation duplicated historical records"
            );
            let rows: i64=db.query_row("SELECT count(*) FROM session_catalog WHERE session_key='native-session' AND lifecycle='current'",[],|row|row.get(0)).unwrap();
            assert_eq!(rows, 1);
            let selected: String=db.query_row("SELECT source_path FROM session_catalog_sources WHERE session_key='native-session' AND lifecycle='current'",[],|row|row.get(0)).unwrap();
            assert_eq!(selected, new.to_string_lossy());
            eprintln!(
                "native-move unrelated={unrelated} appended={append} loads=1 records={} elapsed_us={}",
                u64::from(append),
                started.elapsed().as_micros()
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn new_source_publication_keeps_its_facts_fence_after_native_preparation() {
    use sha2::{Digest, Sha256};
    use std::os::fd::AsRawFd;
    let fixture = Fixture::new();
    fixture.source("selected");
    let lock_path = fixture
        .options
        .cache
        .with_extension("source-locks")
        .join(format!("{:x}", Sha256::digest(b"facts")));
    let locked = lock_path.clone();
    crate::events::BEFORE_CATALOG_PUBLICATION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(locked)
                .unwrap();
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                -1,
                "facts writer entered the native preparation/publication gap"
            );
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }))
    });
    fixture.publish_existing();
    assert!(
        crate::events::BEFORE_CATALOG_PUBLICATION.with(|hook| hook.borrow().is_none()),
        "publication hook did not run"
    );
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "publication kept the facts fence after completion"
    );
    drop(file);
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(facts_path).unwrap();
    facts.hostname = "updated-after-publication".into();
    let intent = crate::SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    intent.complete().unwrap();
    let db = fixture.db();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM session_catalog_invalidations",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "facts write lost the newly published dependency"
    );
    fixture.publish_existing();
    let (_, body) = fixture.body("sid=selected");
    assert_eq!(body["items"][0]["facts_observation"]["state"], "cached");
}

#[test]
fn qualified_read_interest_resumes_a_durable_revision_after_owner_loss() {
    use std::io::Write;
    let fixture = Fixture::new();
    let path = fixture.source("selected");
    fixture.source("unrelated");
    fixture.publish_existing();
    let db = fixture.db();
    let unrelated: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='unrelated'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let from = fs::metadata(&path).unwrap().len();
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/project/selected.jsonl".into(),
    };
    let intent = crate::SessionSourceWrite::begin(
        &fixture.options,
        &input,
        Some("0123456789abcdef0123456789abcdef"),
        from,
    )
    .unwrap();
    let line = format!(
        "{}\n",
        json!({"type":"user","sessionId":"selected","uuid":"later","parentUuid":"selected","timestamp":"2026-10-01T00:01:00Z","message":{"role":"user","content":"after owner loss"}})
    );
    let mut raw = fs::OpenOptions::new().append(true).open(&path).unwrap();
    raw.write_all(line.as_bytes()).unwrap();
    raw.sync_all().unwrap();
    intent.complete(from + line.len() as u64).unwrap();
    let pool = crate::RefreshPool::new(1);
    let observer = crate::SessionCatalogObserver::with_pool(fixture.options.clone(), pool.clone());
    // No Push notification reaches the new observer. Persisted read interest
    // must find the unacknowledged revision left by the previous owner.
    observer.watch().unwrap();
    let started = std::time::Instant::now();
    loop {
        let pending: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        if !pending {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "qualified observer lost durable work"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    observer.close();
    assert_eq!(
        db.query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='unrelated'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        unrelated
    );
    let response =
        crate::session_transcript_range(&fixture.options, "local", "sid=selected&limit=100", None);
    assert_eq!(response.status, 200);
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(body["entries"].as_array().unwrap().len(), 2);
    assert!(pool.threads() <= 1);
}

#[test]
fn native_relocation_history_witness_survives_failed_publication_and_an_append() {
    use crate::{Input, InputRoot, SessionFactsWrite, SessionSourceWrite};
    use std::{collections::BTreeSet, io::Write};
    let fixture = Fixture::new();
    let active = "sessions/2026/10/01/rollout-replay.jsonl";
    let archived = "archived_sessions/2026/10/01/rollout-replay.jsonl";
    let old = fixture.options.codex_home.join(active);
    let new = fixture.options.codex_home.join(archived);
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::write(&old, format!("{}\n{}\n",json!({"type":"session_meta","payload":{"id":"move-replay","cwd":"/synthetic/project"}}),json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]}}))).unwrap();
    let facts_path = fixture.options.facts.as_ref().unwrap();
    let mut facts = crate::read_facts(facts_path).unwrap();
    facts.codex_rollouts = Some(BTreeSet::from([active.to_owned()]));
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    fixture.publish_existing();
    let db = fixture.db();
    let file_id: i64 = db
        .query_row(
            "SELECT file_id FROM files WHERE path=?1",
            [old.to_string_lossy()],
            |row| row.get(0),
        )
        .unwrap();
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    fs::rename(&old, &new).unwrap();
    facts.codex_rollouts = Some(BTreeSet::from([archived.to_owned()]));
    let intent = SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
    crate::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    intent.complete().unwrap();
    db.execute_batch("CREATE TRIGGER reject_move BEFORE UPDATE ON session_catalog BEGIN SELECT RAISE(ABORT,'injected publication failure'); END;").unwrap();
    {
        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        assert!(
            evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .is_err()
        );
    }
    assert_eq!(
        db.query_row(
            "SELECT file_id FROM files WHERE path=?1",
            [new.to_string_lossy()],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        file_id
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM meta WHERE key LIKE 'native_catalog_relocation:%'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(db.query_row("SELECT source_path FROM session_catalog_sources WHERE session_key='move-replay' AND lifecycle='current'",[],|row|row.get::<_,String>(0)).unwrap(),old.to_string_lossy());
    // The Index move committed; Catalog did not. A later append and a new
    // producer must advance and replay the same witness, not retain Raw twice.
    let from = fs::metadata(&new).unwrap().len();
    let input = Input {
        root: InputRoot::Codex,
        path: archived.into(),
    };
    let intent = SessionSourceWrite::begin(
        &fixture.options,
        &input,
        Some("0123456789abcdef0123456789abcdef"),
        from,
    )
    .unwrap();
    let line = format!(
        "{}\n",
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"second"}]}})
    );
    let mut raw = fs::OpenOptions::new().append(true).open(&new).unwrap();
    raw.write_all(line.as_bytes()).unwrap();
    raw.sync_all().unwrap();
    intent.complete(from + line.len() as u64).unwrap();
    db.execute_batch("DROP TRIGGER reject_move").unwrap();
    let parsed = crate::events::PARSED.with(std::cell::Cell::get);
    fixture.publish_existing();
    assert_eq!(crate::events::PARSED.with(std::cell::Cell::get) - parsed, 1);
    for scope in ["", "&scope=retained_history"] {
        let response = crate::session_transcript_range(
            &fixture.options,
            "local",
            &format!("sid=move-replay&limit=100{scope}"),
            None,
        );
        assert_eq!(response.status, 200);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["entries"].as_array().unwrap().len(), 2, "{body}");
    }
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM meta WHERE key LIKE 'native_catalog_relocation:%'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM session_catalog_invalidations",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn runtime_changes_publish_only_indexed_dependencies_and_heartbeat_reuses_catalog() {
    let id = "12345678-1234-1234-1234-123456789012";
    for unrelated in [0, 256, 2048] {
        let fixture = Fixture::new();
        for n in 0..unrelated {
            fixture.source(&format!("unrelated-{n:05}"));
        }
        let relative = "sessions/2026/10/01/rollout-runtime.jsonl";
        let path = fixture.options.codex_home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{}\n{}\n",json!({"type":"session_meta","payload":{"id":id,"cwd":"/synthetic/project"}}),json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"synthetic prompt"}]}}))).unwrap();
        let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
        facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
        facts.native_names.push(crate::NativeName {
            harness: "codex".into(),
            native_id: id.into(),
            name: "Before".into(),
            source: "state_5.sqlite".into(),
            updated_at: None,
            observed_at: 1,
            freshness: "fresh".into(),
        });
        let write = crate::SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();
        write.complete().unwrap();
        fixture.publish_existing();
        let db = fixture.db();
        let metadata = || {
            db.prepare("SELECT session_key,metadata FROM session_catalog WHERE lifecycle='current' ORDER BY session_key").unwrap().query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).unwrap().collect::<Result<std::collections::BTreeMap<_,_>,_>>().unwrap()
        };
        let loads = crate::events::FILE_READS.with(std::cell::Cell::get);
        fixture.publish_existing();
        assert_eq!(
            crate::events::FILE_READS.with(std::cell::Cell::get),
            loads,
            "warm restart loaded history"
        );
        let original = metadata();
        assert!(original.values().all(|text| {
            let row: Value = serde_json::from_str(text).unwrap();
            row["query"]["runtime"]
                .as_str()
                .is_some_and(|runtime| runtime.starts_with("scoped-v1:"))
        }));
        for phase in 0..3 {
            match phase {
                0 => facts.native_names[0].name = "After".into(),
                1 => {
                    facts.codex_locks.insert(id.into(), 42);
                    facts.proc_starts.insert(42, 7);
                }
                _ => {
                    facts.runs.insert(
                        42,
                        std::collections::BTreeMap::from([(
                            "OSTROM_RUN_ID".into(),
                            "synthetic-run".into(),
                        )]),
                    );
                }
            }
            let write = crate::SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
            crate::write_facts(
                fixture.options.facts.as_ref().unwrap(),
                &facts,
                &crate::Options::default().source,
            )
            .unwrap();
            write.complete().unwrap();
            let before = crate::events::FILE_READS.with(std::cell::Cell::get);
            let parsed = crate::events::PARSED.with(std::cell::Cell::get);
            let started = std::time::Instant::now();
            fixture.publish_existing();
            let elapsed = started.elapsed();
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - before;
            assert_eq!(loads, 1, "runtime update loaded unrelated evidence");
            assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
            let after = metadata();
            for (key, value) in &original {
                if key != id {
                    assert_eq!(&after[key], value, "unrelated runtime publication changed");
                }
            }
            assert!(elapsed < Duration::from_secs(10));
            let (_, page) = fixture.body("limit=100");
            assert!(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["facts_observation"]["state"] == "cached"),
                "unrelated runtime fence became stale: {page}"
            );
            eprintln!(
                "runtime-scope unrelated={unrelated} phase={phase} loads={loads} records=0 elapsed_us={}",
                elapsed.as_micros()
            );
        }
        let before = metadata();
        facts.mirror_sequence = Some(123);
        facts.mirror_observation_id = Some("heartbeat".into());
        facts.native_names[0].observed_at = 2;
        let write = crate::SessionFactsWrite::begin(&fixture.options, &facts).unwrap();
        crate::write_facts(
            fixture.options.facts.as_ref().unwrap(),
            &facts,
            &fixture.options.source,
        )
        .unwrap();
        write.complete().unwrap();
        let loads = crate::events::FILE_READS.with(std::cell::Cell::get);
        fixture.publish_existing();
        assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), loads);
        assert_eq!(metadata(), before, "heartbeat rebuilt Catalog metadata");
    }
}

#[test]
fn source_intent_reserves_write_before_reads_and_keeps_catalog_readers_responsive() {
    let fixture = Fixture::new();
    let path = fixture.source("selected");
    fixture.publish_existing();
    let options = fixture.options.clone();
    let db_path = EventCache::path(&options.cache);
    crate::events::store::BEFORE_SOURCE_INVALIDATION.with(|hook| *hook.borrow_mut()=Some(Box::new(move || {
            let db=Connection::open(db_path).unwrap(); db.busy_timeout(Duration::ZERO).unwrap();
            let failure=db.execute_batch("BEGIN IMMEDIATE").unwrap_err();
            assert!(matches!(failure,rusqlite::Error::SqliteFailure(error,_) if error.code==rusqlite::ErrorCode::DatabaseBusy));
            assert_eq!(session_catalog_page(&options,"local","sid=selected").status,200,"a reserved journal write blocked a foreground Catalog reader");
        })));
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/project/selected.jsonl".into(),
    };
    let from = fs::metadata(&path).unwrap().len();
    let intent = crate::SessionSourceWrite::begin(
        &fixture.options,
        &input,
        Some("1".repeat(32).as_str()),
        from,
    )
    .unwrap();
    use std::io::Write;
    let line = format!("{}\n", json!({"type":"system","sessionId":"selected"}));
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(line.as_bytes()).unwrap();
    file.sync_all().unwrap();
    intent.complete(from + line.len() as u64).unwrap();
    fixture.publish_existing();
    assert!(
        crate::source_projection_ready(&fixture.options, "claude", &input.path)
            .unwrap()
            .is_some()
    );
}

#[test]
fn pid_mutations_replay_without_rebuilding_unrelated_catalog_rows() {
    for unrelated in [0, 256, 2048] {
        let fixture = Fixture::new();
        for n in 0..unrelated {
            fixture.source(&format!("history-{n:05}"));
        }
        fixture.source("selected-a");
        fixture.source("selected-b");
        fs::create_dir_all(fixture.options.claude_home.join("sessions")).unwrap();
        fixture.publish_existing();
        let db = fixture.db();
        let metadata = || {
            db.prepare("SELECT session_key,metadata FROM session_catalog ORDER BY session_key")
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<std::collections::BTreeMap<_, _>, _>>()
                .unwrap()
        };
        let original = metadata();
        let input = crate::Input {
            root: crate::InputRoot::Claude,
            path: "sessions/4242.json".into(),
        };
        let path = input.full_path(&fixture.options);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for phase in 0..3 {
            let intent =
                crate::SessionSourceWrite::begin(&fixture.options, &input, None, 0).unwrap();
            fs::write(&path,json!({"sessionId":if phase==0 {"selected-a"} else {"selected-b"},"name":format!("runtime-{phase}")}).to_string()).unwrap();
            if phase == 1 {
                drop(intent); // Raw committed, process lost before Index intent completion.
                let _fence =
                    crate::source_change::SourceLock::acquire(&fixture.options, "facts", false)
                        .unwrap();
                crate::events::store::resume_runtime_input(
                    &fixture.options,
                    path.to_str().unwrap(),
                )
                .unwrap();
                crate::events::store::resume_runtime_input(
                    &fixture.options,
                    path.to_str().unwrap(),
                )
                .unwrap();
            } else {
                intent.complete(fs::metadata(&path).unwrap().len()).unwrap();
            }
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get);
            let parsed = crate::events::PARSED.with(std::cell::Cell::get);
            let start = std::time::Instant::now();
            fixture.publish_existing();
            assert_eq!(
                crate::events::FILE_READS.with(std::cell::Cell::get) - loads,
                if phase == 1 { 2 } else { 1 },
                "PID phase={phase} unrelated={unrelated}"
            );
            assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
            let after = metadata();
            for (key, value) in &original {
                if key.starts_with("history-") {
                    assert_eq!(
                        &after[key], value,
                        "PID mutation invalidated an unrelated scoped recipe"
                    );
                }
            }
            let (_, page) = fixture.body("limit=100");
            assert!(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["facts_observation"]["state"] == "cached"),
                "PID phase={phase} page={page}"
            );
            assert!(start.elapsed() < Duration::from_secs(10));
            eprintln!(
                "pid-scope unrelated={unrelated} phase={phase} loads={} records=0 elapsed_us={}",
                if phase == 1 { 2 } else { 1 },
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
fn local_runtime_changes_rebuild_only_the_indexed_session() {
    for unrelated in [0, 256, 2048] {
        let mut fixture = Fixture::new();
        fixture.options.facts = None;
        for n in 0..unrelated {
            fixture.source(&format!("history-{n:05}"));
        }
        fixture.source("selected");
        fixture.publish_existing();
        let db_path = EventCache::path(&fixture.options.cache);
        let metadata = || {
            let db = Connection::open(&db_path).unwrap();
            db.prepare("SELECT session_key,metadata FROM session_catalog ORDER BY session_key")
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<std::collections::BTreeMap<_, _>, _>>()
                .unwrap()
        };
        let original = metadata();
        let pid = fixture.options.claude_home.join("sessions/4242.json");
        fs::create_dir_all(pid.parent().unwrap()).unwrap();
        for phase in 0..3 {
            match phase {
                    0 => fs::write(&pid, json!({"sessionId":"selected","procStart":1,"name":"focused runtime"}).to_string()).unwrap(),
                    1 => fs::write(&fixture.options.claude_json,json!({"projects":{"project":{"lastSessionId":"selected","lastStartTime":1,"lastCost":1.25}}}).to_string()).unwrap(),
                    _ => fs::remove_file(&pid).unwrap(),
                }
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get);
            let parsed = crate::events::PARSED.with(std::cell::Cell::get);
            let start = std::time::Instant::now();
            fixture.publish_existing();
            assert_eq!(
                crate::events::FILE_READS.with(std::cell::Cell::get) - loads,
                1,
                "local runtime loaded unrelated Event Indexes"
            );
            assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
            let after = metadata();
            for (key, value) in &original {
                if key != "selected" {
                    assert_eq!(
                        &after[key], value,
                        "local runtime rebuilt unrelated Catalog rows"
                    );
                }
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            eprintln!(
                "local-runtime-scope unrelated={unrelated} phase={phase} loads=1 records=0 elapsed_us={}",
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
fn large_cold_source_checkpoints_complete_records_and_restarts_without_reparsing_prefixes() {
    let fixture = Fixture::new();
    let path = fixture.source("large");
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    let line = format!(
        "{}\n",
        json!({"type":"system","subtype":"turn_duration","sessionId":"large","padding":"x".repeat(32*1024)})
    );
    for _ in 0..512 {
        file.write_all(line.as_bytes()).unwrap();
    }
    file.sync_all().unwrap();
    drop(file);
    let total = fs::metadata(&path).unwrap().len();
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/project/large.jsonl".into(),
    };
    let db_path = EventCache::path(&fixture.options.cache);
    let mut previous = 0;
    let mut steps = 0;
    let mut records = 0;
    while previous < total {
        // A new owner on every step exercises durable discovery and parser
        // checkpoints, without any in-memory snapshot or index reuse.
        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        let before = crate::events::PARSED.with(std::cell::Cell::get);
        let hashed = crate::events::HASHED_BYTES.with(std::cell::Cell::get);
        let start = std::time::Instant::now();
        let complete = evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap();
        let db = Connection::open(&db_path).unwrap();
        let (offset, size): (u64, u64) = db
            .query_row(
                "SELECT resume_at,size FROM files WHERE path=?1",
                [path.to_string_lossy()],
                |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
            )
            .unwrap();
        assert!(offset > previous, "backfill checkpoint made no progress");
        assert!(
            offset - previous <= 4 * 1024 * 1024 + line.len() as u64,
            "one cold step parsed unbounded history"
        );
        assert_eq!(size, offset.min(total));
        assert_eq!(
            crate::events::HASHED_BYTES.with(std::cell::Cell::get),
            hashed,
            "restart rehashed the committed historical prefix"
        );
        records += crate::events::PARSED.with(std::cell::Cell::get) - before;
        assert_eq!(work.events.as_ref().unwrap().resident_index_payloads(), 0);
        let body = fixture.body("sid=large").1;
        assert_eq!(
            body["completeness"]["state"],
            if complete { "complete" } else { "partial" }
        );
        eprintln!(
            "cold-byte-step step={steps} offset={offset} bytes={} elapsed_us={} resident_index_payloads=0",
            offset - previous,
            start.elapsed().as_micros()
        );
        previous = offset;
        steps += 1;
        assert!(steps <= 8);
    }
    assert_eq!(records, 513, "restarted cold steps reparsed records");
    assert!(steps >= 4);
    let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
    let mut work = crate::viewer::lock(&evidence.work);
    let parsed = crate::events::PARSED.with(std::cell::Cell::get);
    let loads = crate::events::FILE_READS.with(std::cell::Cell::get);
    let mut complete = false;
    for _ in 0..8 {
        if evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "qualified restart did not finish its bounded metadata audit"
    );
    assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
    assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), loads);
    assert!(
        crate::source_projection_ready(&fixture.options, input.root.as_str(), &input.path)
            .unwrap()
            .is_some()
    );
}

#[test]
fn newly_indexed_claude_lineage_expands_only_its_existing_owner() {
    for unrelated in [0, 256, 2048] {
        let fixture = Fixture::new();
        fixture.source("root");
        for n in 0..unrelated {
            fixture.source(&format!("unrelated-{n:05}"));
        }
        fixture.publish_existing();
        let input = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/cleared.jsonl".into(),
        };
        let intent = crate::SessionSourceWrite::begin(
            &fixture.options,
            &input,
            Some("0123456789abcdef0123456789abcdef"),
            0,
        )
        .unwrap();
        let bytes = format!(
            "{}\n",
            json!({"type":"user","sessionId":"cleared","session_id":"root","uuid":"cleared","timestamp":"2026-10-01T00:01:00Z","message":{"role":"user","content":"cleared prompt"}})
        );
        fs::write(input.full_path(&fixture.options), bytes.as_bytes()).unwrap();
        intent.complete(bytes.len() as u64).unwrap();
        let loads = crate::events::FILE_READS.with(std::cell::Cell::get);
        let derived = crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get);
        let discovery = crate::inputs::DISCOVERY_ENTRIES.with(std::cell::Cell::get);
        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        assert!(
            evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
        );
        assert_eq!(
            crate::events::FILE_READS.with(std::cell::Cell::get) - loads,
            1
        );
        assert_eq!(
            crate::inputs::DISCOVERY_ENTRIES.with(std::cell::Cell::get),
            discovery
        );
        assert!(crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get) - derived <= 3);
        let db = fixture.db();
        let rows: Vec<String> = db.prepare("SELECT metadata FROM session_catalog WHERE lifecycle='current' AND session_key IN ('root','cleared')").unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
        assert_eq!(rows.len(), 1);
        let row: Value = serde_json::from_str(&rows[0]).unwrap();
        assert_eq!(row["native_ids"].as_array().unwrap().len(), 2);
        assert_eq!(row["sources"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn generic_metadata_audit_preserves_rows_and_acknowledges_only_its_complete_observation() {
    for unrelated in [0, 256] {
        let fixture = Fixture::new();
        fixture.source("selected");
        for n in 0..unrelated {
            fixture.source(&format!("unrelated-{n:05}"));
        }
        fixture.publish_existing();
        let db = fixture.db();
        let read = || {
            db.query_row(
                "SELECT value FROM meta WHERE key='catalog_observed_at'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
        };
        let before = read();
        let observed = before.parse::<i64>().unwrap() + 1000;
        let rows = db.query_row("SELECT group_concat(metadata) FROM (SELECT metadata FROM session_catalog ORDER BY session_key)", [], |row| row.get::<_,String>(0)).unwrap();
        let counts = || {
            (
                crate::events::FILE_READS.with(std::cell::Cell::get),
                crate::events::PARSED.with(std::cell::Cell::get),
                crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get),
                crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get),
                crate::events::CATALOG_PUBLISHED_ROWS.with(std::cell::Cell::get),
            )
        };
        let start = counts();
        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        evidence.request_metadata_audit_if_idle();
        evidence.request_metadata_audit_if_idle();
        let mut work = crate::viewer::lock(&evidence.work);
        let mut steps = 0;
        loop {
            steps += 1;
            let complete = evidence.refresh_catalog(&mut work, observed).unwrap();
            if complete {
                break;
            }
            assert_eq!(
                read(),
                before,
                "an unfinished audit advanced the cohort clock"
            );
            assert!(steps < 32, "metadata audit did not make bounded progress");
        }
        assert_eq!(read(), observed.to_string());
        assert_eq!(counts(), start, "metadata audit rebuilt session history");
        assert_eq!(rows, db.query_row("SELECT group_concat(metadata) FROM (SELECT metadata FROM session_catalog ORDER BY session_key)", [], |row| row.get::<_,String>(0)).unwrap());
        assert!(!evidence.catalog_pending(&work));
    }
}

#[test]
fn cold_codex_parent_context_survives_separate_unrelated_batches() {
    let fixture = Fixture::new();
    let child = fixture.codex_source("family-child", Some("family-parent"), 1);
    let parent = fixture.codex_source("family-parent", None, 3);
    for n in 0..128 {
        let path = fixture.source(&format!("unrelated-{n:03}"));
        fs::File::open(path)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(2)),
            )
            .unwrap();
    }
    let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
    facts.codex_rollouts = Some(std::collections::BTreeSet::from([child, parent]));
    crate::write_facts(
        fixture.options.facts.as_ref().unwrap(),
        &facts,
        &crate::Options::default().source,
    )
    .unwrap();
    fixture.publish_existing();
    let db = fixture.db();
    assert_eq!(db.query_row("SELECT parent_key FROM session_catalog WHERE session_key='family-child' AND lifecycle='current'", [], |row| row.get::<_, Option<String>>(0)).unwrap().as_deref(), Some("family-parent"));
}

#[test]
fn cold_codex_family_crossing_batches_preserves_every_native_parent() {
    let fixture = Fixture::new();
    let mut paths = std::collections::BTreeSet::new();
    for n in 0..80 {
        paths.insert(fixture.codex_source(
            &format!("family-child-{n:03}"),
            Some("family-parent"),
            1,
        ));
    }
    // The newest cohort includes the parent and only part of its children.
    // The remaining children's own headers must resolve that already indexed
    // parent when subsequent source batches are published.
    paths.insert(fixture.codex_source("family-parent", None, 2));
    let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
    facts.codex_rollouts = Some(paths);
    crate::write_facts(
        fixture.options.facts.as_ref().unwrap(),
        &facts,
        &crate::Options::default().source,
    )
    .unwrap();
    let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
    let mut work = crate::viewer::lock(&evidence.work);
    let mut complete = false;
    for _ in 0..16 {
        if evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .unwrap()
        {
            complete = true;
            break;
        }
    }
    drop(work);
    evidence.retire();
    assert!(
        complete,
        "connected cold family did not finish bounded steps"
    );
    let db = fixture.db();
    let parents = db.prepare("SELECT session_key,parent_key FROM session_catalog WHERE lifecycle='current' AND session_key LIKE 'family-child-%' ORDER BY session_key").unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(parents.len(), 80);
    for (child, parent) in parents {
        assert_eq!(
            parent.as_deref(),
            Some("family-parent"),
            "{child} lost its native parent across source batches"
        );
    }
}

#[test]
fn journal_append_and_warm_restart_ignore_unrelated_history() {
    use std::io::Write;
    let mut baseline = None;
    for unrelated in [0, 256, 2048] {
        let fixture = Fixture::new();
        let path = fixture.source("selected");
        for n in 0..unrelated {
            fixture.source(&format!("unrelated-{n:05}"));
        }
        fixture.publish_existing();
        let input = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/selected.jsonl".into(),
        };
        let from = fs::metadata(&path).unwrap().len();
        let intent = crate::SessionSourceWrite::begin(
            &fixture.options,
            &input,
            Some("0123456789abcdef0123456789abcdef"),
            from,
        )
        .unwrap();
        let line = format!(
            "{}\n",
            json!({"type":"assistant","sessionId":"selected","uuid":"answer","parentUuid":"selected","timestamp":"2026-10-01T00:01:00Z","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}})
        );
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(line.as_bytes()).unwrap();
        file.sync_all().unwrap();
        intent.complete(from + line.len() as u64).unwrap();
        let discovery = crate::inputs::DISCOVERY_ENTRIES.with(std::cell::Cell::get);
        let semantic = crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get);
        let published = crate::events::CATALOG_PUBLISHED_ROWS.with(std::cell::Cell::get);
        let read_bytes = crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get);
        let parse_bytes = crate::events::PARSED_BYTES.with(std::cell::Cell::get);
        let reads = crate::events::FILE_READS.with(std::cell::Cell::get);
        let parsed = crate::events::PARSED.with(std::cell::Cell::get);
        let hashed = crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get);
        let full_prefix = crate::events::HASHED_BYTES.with(std::cell::Cell::get);
        let started = std::time::Instant::now();
        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        assert!(
            evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
        );
        let elapsed = started.elapsed();
        let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - reads;
        let records = crate::events::PARSED.with(std::cell::Cell::get) - parsed;
        let hash_bytes = crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get) - hashed;
        let discovery = crate::inputs::DISCOVERY_ENTRIES.with(std::cell::Cell::get) - discovery;
        let semantic = crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get) - semantic;
        let published =
            crate::events::CATALOG_PUBLISHED_ROWS.with(std::cell::Cell::get) - published;
        let read_bytes = crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get) - read_bytes;
        let parse_bytes = crate::events::PARSED_BYTES.with(std::cell::Cell::get) - parse_bytes;
        assert_eq!(
            crate::events::HASHED_BYTES.with(std::cell::Cell::get),
            full_prefix,
            "qualified append rehashed a historical prefix"
        );
        let work_bytes = (hash_bytes, read_bytes, parse_bytes);
        if let Some(baseline) = baseline {
            assert_eq!(
                work_bytes, baseline,
                "append work grew with unrelated history"
            );
        } else {
            baseline = Some(work_bytes);
        }
        assert_eq!((discovery, semantic, published), (0, 1, 1));
        assert_eq!(work.events.as_ref().unwrap().resident_index_payloads(), 0);
        assert_eq!(loads, 1, "loaded unrelated Event Index history");
        assert_eq!(records, 1, "reparsed unrelated native history");
        assert!(elapsed < Duration::from_secs(10));
        assert_eq!(work.events.as_ref().unwrap().paths().count(), 1);
        drop(work);
        evidence.retire();
        let before = crate::events::FILE_READS.with(std::cell::Cell::get);
        let restarted = std::time::Instant::now();
        let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
        let mut work = crate::viewer::lock(&evidence.work);
        assert!(
            evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
        );
        assert_eq!(
            crate::events::FILE_READS.with(std::cell::Cell::get),
            before,
            "restart rebuilt historical indexes"
        );
        assert_eq!(work.events.as_ref().unwrap().paths().count(), 0);
        assert_eq!(fixture.body("sid=selected").0, 200);
        let restart = restarted.elapsed();
        assert!(restart < Duration::from_secs(2));
        eprintln!(
            "journal-append unrelated={unrelated} loads={loads} records={records} hashed_bytes={hash_bytes} full_prefix_hash_bytes=0 read_bytes={read_bytes} parsed_bytes={parse_bytes} discovery_entries={discovery} semantic_sources={semantic} published_rows={published} resident_index_payloads=0 elapsed_us={} restart_loads=0 restart_usable_us={}",
            elapsed.as_micros(),
            restart.as_micros()
        );
    }
}

#[test]
fn cold_scoped_native_publication_is_visible_before_unrelated_source_reads() {
    for unrelated in [0, 2048] {
        let fixture = Fixture::new();
        let inputs: Vec<_> = (0..60)
            .map(|n| {
                let id = format!("selected-{n:05}");
                fixture.source(&id);
                crate::inputs::Input {
                    root: crate::inputs::InputRoot::Claude,
                    path: format!("projects/project/{id}.jsonl"),
                }
            })
            .collect();
        for n in 0..unrelated {
            fixture.source(&format!("unrelated-{n:05}"));
        }
        let mut cache =
            EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
        assert_eq!(cache.paths().count(), 0);
        assert!(
            crate::model::prepare_sources(
                &fixture.options,
                &mut cache,
                &vec![inputs[0].clone(); 65],
                crate::model::now_ms()
            )
            .is_err()
        );
        assert_eq!(
            cache.paths().count(),
            0,
            "over-budget source batches must fail before indexing"
        );
        let start = std::time::Instant::now();
        let prepared = crate::model::prepare_sources(
            &fixture.options,
            &mut cache,
            &inputs,
            crate::model::now_ms(),
        )
        .unwrap();
        assert_eq!(prepared.rows.len(), 60);
        assert_eq!(
            cache.publish_partial_catalog(&prepared).unwrap(),
            crate::events::Outcome::Written
        );
        let (status, body) = fixture.body("limit=60");
        assert_eq!(status, 200);
        assert_eq!(body["items"].as_array().unwrap().len(), 60);
        assert_eq!(body["completeness"]["state"], "partial");
        assert_eq!(body["freshness"], "updating");
        assert_eq!(cache.paths().count(), 60);
        let connection = fixture.db();
        let observed: i64 = connection
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            observed, 60,
            "unrelated native sources must remain unobserved before first page"
        );
        let identity = session_catalog_identity(&fixture.options, "source", "selected-00000")
            .unwrap()
            .unwrap();
        assert_eq!(identity.native_id, None);
        assert_eq!(
            identity.owner_qualification,
            CatalogOwnerQualification::Provisional
        );
        assert!(
            session_catalog_history_identity(&fixture.options, "source", "selected-00000")
                .unwrap()
                .is_some()
        );
        eprintln!(
            "scoped-cold-first-page unrelated={unrelated} elapsed_us={} bytes={} native_sources={observed}",
            start.elapsed().as_micros(),
            serde_json::to_vec(&body).unwrap().len()
        );
        // A fresh lazy producer restores no global file indexes or runs.
        let reopened =
            EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap();
        assert_eq!(reopened.paths().count(), 0);
    }
}

#[test]
fn selected_native_index_preserves_membership_and_native_semantics() {
    let fixture = Fixture::new();
    fixture.publish(3);
    let connection = fixture.db();
    let transaction = connection.unchecked_transaction().unwrap();
    let metadata = rows(&transaction, &Request::parse("sid=session-00000").unwrap()).unwrap();
    let source = &metadata[0];
    let ids = source.native_ids.iter().cloned().collect();
    let selected = EventCache::from_selected(&transaction, &source.sources, &ids).unwrap();
    assert_eq!(selected.paths().count(), 1);
    let now = crate::model::now_ms();
    let builds = crate::model::BUILDS.with(std::cell::Cell::get);
    let reads = crate::events::FILE_READS.with(std::cell::Cell::get);
    let mut query = crate::Query::new(fixture.options.clone());
    let page = query
        .call_selected_at(
            "",
            &source.key,
            "read_transcript",
            &json!({"id":source.key}),
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            now,
        )
        .unwrap();
    assert_eq!(crate::model::BUILDS.with(std::cell::Cell::get), builds);
    assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), reads);
    assert_eq!(
        page,
        json!({
            "sid": "session-00000", "from": 0, "to": 1, "total": 1,
            "calls": 0, "errors": 0, "window_start": null,
            "entries": [{"k": "u", "text": "synthetic prompt", "turn": "session-00000:session-00000:0"}]
        })
    );
    let full = crate::model::build_query(
        &fixture.options,
        &mut EventCache::open_scoped(&fixture.options.cache, &fixture.options.source).unwrap(),
        &mut false,
        &mut crate::model::Texts::default(),
        now,
    )
    .unwrap();
    assert!(!crate::pricing::snapshot().fingerprint.is_empty());
    assert_eq!(
        serde_json::to_value(&full.sessions[&source.key]).unwrap(),
        json!({
        "name": "project", "harness": "claude", "lane": true,
        "machine": "synthetic-catalog", "state": "done", "model": "—",
        "tokens": [0.0, 0.0, 0.0], "tokens_by_model": {},
        "cost": {"usd": 0.0, "unpriced_models": [], "split_unknown_messages": 0,
            "by_model": {}, "by_day": {},
            "pricing_fingerprint": crate::pricing::snapshot().fingerprint},
        "reported_runs": [], "cost_check": [], "repo": "project",
        "start": 1790812800000_i64, "last": 1790812800000_i64,
        "busy": [[1790812800000_i64, 1790812800000_i64]], "calls": 0, "errors": 0
        })
    );

    assert_eq!(selected.paths().count(), 1);
    let files: i64 = connection
        .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
        .unwrap();
    let catalog: i64 = connection
        .query_row("SELECT count(*) FROM session_catalog", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        (files, catalog),
        (3, 3),
        "scoped native projection must not prune unrelated global rows"
    );
    let mut conflicting = source.sources.clone();
    conflicting[0].offset += 1;
    assert!(EventCache::from_selected(&transaction, &conflicting, &ids).is_err());
    conflicting.push(source.sources[0].clone());
    assert!(EventCache::from_selected(&transaction, &conflicting, &ids).is_err());
}

#[test]
fn focused_catalog_demand_publishes_without_waiting_for_an_initial_model() {
    let fixture = Fixture::new();
    fixture.source("first");
    let mut core = ViewerCore::new(fixture.options.clone());
    core.set_refresh(Refresh::Background);
    let first = core.respond("GET", "/api/sessions", "", None);
    assert!(matches!(first.status, 200 | 503));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = core.respond("GET", "/api/sessions", "", None);
        if response.status == 200 {
            let body: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(body["items"][0]["key"], "first");
            assert_eq!(body["refresh"]["state"], "updating");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "catalog demand must start initial background publication"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    core.close();
}

#[test]
fn focused_catalog_keyset_server_work_is_constant_with_unrelated_history() {
    let fixture = Fixture::new();
    fixture.publish(100);
    let connection = fixture.db();
    let request = Request::parse("limit=3&harness=claude&repo=project")
        .ok()
        .unwrap();
    SQL_STEPS.with(|steps| steps.set(0));
    let small = rows(&connection, &request).unwrap();
    let small_steps = SQL_STEPS.with(|steps| steps.get());
    // Add unrelated, equal-time metadata to expose an accidental OFFSET
    // or whole-catalog preparation without reading any transcript bodies.
    let encoded: String = connection
        .query_row("SELECT metadata FROM session_catalog LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut row: CatalogRow = serde_json::from_str(&encoded).unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    for id in 100..20_000 {
        row.key = format!("session-{id:05}");
        transaction.execute("INSERT INTO session_catalog(namespace,session_key,last_ms,harness,repo,metadata)VALUES(semon_namespace(),?1,?2,?3,?4,?5)",rusqlite::params![row.key,row.last.unwrap_or(0),row.harness,row.repo,serde_json::to_string(&row).unwrap()]).unwrap();
    }
    transaction.commit().unwrap();
    SQL_STEPS.with(|steps| steps.set(0));
    let large = rows(&connection, &request).unwrap();
    let large_steps = SQL_STEPS.with(|steps| steps.get());
    assert_eq!(
        serde_json::to_value(small).unwrap(),
        serde_json::to_value(large).unwrap()
    );
    assert_eq!(small_steps, large_steps);
    // A late cursor in a huge equal-time group must seek its key directly.
    let request = Request {
        require_complete: false,
        scope: CatalogReadScope::Current,
        limit: 3,
        sid: None,
        harness: Some("claude".into()),
        repo: Some("project".into()),
        model: None,
        q: None,
        cursor: Some(Cursor {
            inclusive: false,
            scope: CatalogReadScope::Current,
            version: 1,
            generation: "0".repeat(64),
            machine: "".into(),
            harness: Some("claude".into()),
            repo: Some("project".into()),
            model: None,
            q: None,
            last: row.last.unwrap_or(0),
            key: "session-19000".into(),
        }),
    };
    SQL_STEPS.with(|steps| steps.set(0));
    let late = rows(&connection, &request).unwrap();
    let late_steps = SQL_STEPS.with(|steps| steps.get());
    assert_eq!(late[0].key, "session-19001");
    assert!(
        late_steps < 1000,
        "a late equal-time cursor must not scan earlier metadata: {late_steps}"
    );
    println!(
        "focused catalog SQL VM steps: 100 rows={small_steps}, 20000 rows={large_steps}, late cursor={late_steps}"
    );
}

#[test]
fn resumable_catalog_keeps_current_old_family_outside_viewer_scan_window() {
    let mut fixture = Fixture::new();
    let mut paths = std::collections::BTreeSet::new();
    for n in 0..80 {
        paths.insert(fixture.codex_source(&format!("old-child-{n:03}"), Some("old-parent"), 1));
    }
    let parent = fixture.codex_source("old-parent", None, 2);
    paths.insert(parent.clone());
    let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
    facts.codex_rollouts = Some(paths.clone());
    crate::write_facts(
        fixture.options.facts.as_ref().unwrap(),
        &facts,
        &crate::Options::default().source,
    )
    .unwrap();
    fixture.options.all = false;
    fixture.options.scan_window = true;
    fixture.options.since = Duration::from_secs(1);
    let now = crate::model::now_ms();
    assert!(crate::model::scan_cutoff(&fixture.options, now).unwrap() > 2000);
    let evidence = crate::native_evidence::NativeEvidence::new(fixture.options.clone());
    let mut complete = false;
    for _ in 0..128 {
        if evidence
            .refresh_catalog(&mut crate::viewer::lock(&evidence.work), now)
            .unwrap()
        {
            complete = true;
            break;
        }
        assert!(
            crate::source_projection_ready(&fixture.options, "codex", &parent)
                .unwrap()
                .is_none(),
            "partial old family became archive-ready"
        );
    }
    evidence.retire();
    assert!(
        complete,
        "Catalog staging inherited the viewer window and lost old current sources"
    );
    let db = fixture.db();
    let metadata=db.prepare("SELECT metadata FROM session_catalog WHERE lifecycle='current' AND harness='codex' ORDER BY session_key").unwrap().query_map([],|row|row.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(
        metadata.len(),
        81,
        "old current owners were omitted or retired"
    );
    for text in metadata {
        let row: CatalogRow = serde_json::from_str(&text).unwrap();
        assert_eq!(row.sources.len(), 1);
        let source = &row.sources[0];
        assert_eq!(source.offset, fs::metadata(&source.path).unwrap().len());
        assert_eq!(crate::catalog::source_state(source), "cached");
        if row.key != "old-parent" {
            assert_eq!(row.parent.as_deref(), Some("old-parent"));
            assert_eq!(
                row.query.as_ref().unwrap()["rows"][&row.key]["summary"]["parent"],
                json!("old-parent")
            );
        } else {
            assert_eq!(
                row.query.as_ref().unwrap()["rows"][&row.key]["summary"]["children"]
                    .as_array()
                    .unwrap()
                    .len(),
                80
            );
        }
    }
    for path in paths {
        assert!(
            crate::source_projection_ready(&fixture.options, "codex", &path)
                .unwrap()
                .is_some(),
            "qualified current source {path} lost its original projection"
        );
    }
}

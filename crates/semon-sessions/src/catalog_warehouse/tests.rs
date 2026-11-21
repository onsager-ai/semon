use super::*;
use crate::{
    Options,
    events::{EventCache, scan_file},
    storage::sqlite,
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

struct Fixture {
    dir: PathBuf,
    options: Options,
    namespace: Namespace,
    db: PathBuf,
    cache: EventCache,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "semon-warehouse-list-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(dir.join("proc")).unwrap();
        fs::write(dir.join("proc/locks"), "").unwrap();
        let options = Options {
            claude_home: dir.join("claude"),
            claude_json: dir.join("claude.json"),
            codex_home: dir.join("codex"),
            copilot_home: dir.join("copilot"),
            proc_root: dir.join("proc"),
            cache: dir.join("cache.json"),
            all: true,
            since: Duration::from_secs(86_400),
            session: None,
            facts: None,
            index_recovery: Default::default(),
            annotations: dir.join("annotations.jsonl"),
            scan_window: false,
        };
        let namespace = Namespace::new(name).unwrap();
        let mut cache = EventCache::open_scoped(&options.cache).unwrap();
        sqlite::bind(&mut cache, &namespace).unwrap();
        let db = EventCache::path(&options.cache);
        Self {
            dir,
            options,
            namespace,
            db,
            cache,
        }
    }

    fn claude(&mut self, id: &str, title: &str) -> PathBuf {
        let path = self
            .options
            .claude_home
            .join(format!("projects/proj/{id}.jsonl"));
        let content = format!(
            "{}\n{}\n",
            json!({"type":"user","uuid":id,"sessionId":id,"timestamp":"2026-09-24T00:00:00Z","message":{"role":"user","content":title}}),
            json!({"type":"custom-title","customTitle":title,"sessionId":id})
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        scan_file(&path, "claude", &mut self.cache, &mut false).unwrap();
        path
    }

    fn codex(&mut self, id: &str) -> PathBuf {
        let path = self
            .options
            .codex_home
            .join(format!("sessions/2026/09/24/rollout-{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{}\n", json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":{"id":id,"cwd":"/work/synthetic","originator":"codex_exec","thread_source":"user"}}))).unwrap();
        scan_file(&path, "codex", &mut self.cache, &mut false).unwrap();
        path
    }

    fn titleless_claude(&mut self, id: &str) -> PathBuf {
        let path = self
            .options
            .claude_home
            .join(format!("projects/-work-semon/{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{}\n",json!({"type":"assistant","uuid":id,"sessionId":id,"timestamp":"2026-09-24T00:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"assistant output"}]}}))).unwrap();
        scan_file(&path, "claude", &mut self.cache, &mut false).unwrap();
        path
    }

    fn copilot(&mut self, id: &str) -> PathBuf {
        let path = self
            .options
            .copilot_home
            .join(format!("session-state/{id}/events.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path,format!("{}\n",json!({"type":"session.start","id":"start","timestamp":"2026-09-24T00:00:00Z","data":{"version":1,"copilotVersion":"1.0.91","sessionId":id}}))).unwrap();
        scan_file(&path, "copilot", &mut self.cache, &mut false).unwrap();
        path
    }

    fn source(&self, paths: &[&Path]) -> Source<'_> {
        Source {
            namespace: self.namespace.clone(),
            path: &self.db,
            inventory: paths
                .iter()
                .map(|path| {
                    (
                        path.to_str().unwrap().to_owned(),
                        Availability::Expected(Stat::of(path).ok()),
                    )
                })
                .collect(),
            repos: BTreeMap::new(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn request(query: &str) -> Request {
    Request::parse(query).unwrap_or_else(|_| panic!("invalid fixture query {query}"))
}

fn keys(page: &Page) -> Vec<(&str, &str)> {
    page.rows
        .iter()
        .map(|row| (row.position.key.as_str(), row.position.source.as_str()))
        .collect()
}

#[test]
fn warehouse_union_keeps_ties_coverage_and_committed_cursor_witnesses() {
    let mut a = Fixture::new("union-a");
    let mut b = Fixture::new("union-b");
    let a_session = a.claude("same", "Shared title");
    let b_session = b.claude("same", "Shared title");
    let b_other = b.claude("later", "Different title");
    let mut sources = [b.source(&[&b_session, &b_other]), a.source(&[&a_session])];
    sources[0]
        .inventory
        .insert("unavailable".into(), Availability::Unavailable);
    sources[1]
        .inventory
        .insert("pending".into(), Availability::Expected(None));
    let filters = request("limit=1&harness=claude&no_repo=1");
    let first = read_list(&sources, "workspace", &filters, None).unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(
        first.coverage,
        Coverage {
            committed: 3,
            pending: 1,
            unavailable: 1
        }
    );
    assert_eq!(keys(&first), [("later", "union-b")]);
    let second = read_list(&sources, "workspace", &filters, first.next.as_ref()).unwrap();
    assert_eq!(keys(&second), [("same", "union-a")]);
    let third = read_list(&sources, "workspace", &filters, second.next.as_ref()).unwrap();
    assert_eq!(keys(&third), [("same", "union-b")]);
    assert!(third.next.is_none());
    assert!(matches!(
        read_list(&sources, "other", &filters, first.next.as_ref()),
        Err(ReadError::Scope)
    ));
    assert!(matches!(
        read_list(
            &sources,
            "workspace",
            &request("limit=1&harness=codex"),
            first.next.as_ref()
        ),
        Err(ReadError::Scope)
    ));
    sources[1].inventory.remove("pending");
    assert!(matches!(
        read_list(&sources, "workspace", &filters, first.next.as_ref()),
        Err(ReadError::Restart)
    ));
    assert_eq!(
        read_list(&sources, "workspace", &request("q=shared&limit=3"), None)
            .unwrap()
            .rows
            .len(),
        2
    );
    assert!(matches!(
        identity(&sources[1], CatalogReadScope::RetainedHistory, "same"),
        Err(ReadError::RetainedUnverified)
    ));
}

#[test]
fn warehouse_list_and_identity_compare_to_current_catalog() {
    let mut fixture = Fixture::new("list-oracle");
    let claude = fixture.claude("same", "Shared title");
    let codex = fixture.codex("same");
    let reserved = fixture.claude("claude:same", "Reserved key");
    let copilot_id = "01234567-89ab-4cde-8123-456789abcdef";
    let copilot = fixture.copilot(copilot_id);
    let titleless = fixture.titleless_claude("titleless");
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let source = fixture.source(&[&claude, &codex, &reserved, &copilot, &titleless]);
    for query in [
        "limit=20",
        "harness=codex",
        "harness=copilot",
        "q=reserved",
        "q=semon",
        "model=codex",
        "no_repo=1",
        "repo=nonexistent",
    ] {
        let actual = read_list(
            std::slice::from_ref(&source),
            "workspace",
            &request(query),
            None,
        )
        .unwrap();
        let expected =
            crate::session_catalog_page(&fixture.options, fixture.namespace.as_str(), query);
        assert_eq!(expected.status, 200, "{query}");
        let body: serde_json::Value = serde_json::from_slice(&expected.body).unwrap();
        let oracle: Vec<_> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["key"].as_str().unwrap(),
                    row["harness"].as_str().unwrap(),
                    row["name"].as_str().unwrap(),
                    row["model"].as_str().unwrap(),
                    row["repo"].as_str(),
                    row["branch"].as_str(),
                    row["parent"].as_str(),
                )
            })
            .collect();
        let observed: Vec<_> = actual
            .rows
            .iter()
            .map(|row| {
                (
                    row.position.key.as_str(),
                    row.harness.as_str(),
                    row.title.as_str(),
                    row.model.as_str(),
                    row.repo.as_deref(),
                    row.branch.as_deref(),
                    row.parent.as_deref(),
                )
            })
            .collect();
        assert_eq!(observed, oracle, "{query}");
    }
    for key in [":claude:same", "claude:same", "codex:same", copilot_id] {
        let expected =
            crate::session_catalog_identity(&fixture.options, fixture.namespace.as_str(), key)
                .unwrap()
                .unwrap();
        let (_, harness, native_id) = identity(&source, CatalogReadScope::Current, key)
            .unwrap()
            .unwrap();
        assert_eq!(harness, expected.harness);
        assert_eq!(Some(native_id), expected.native_id);
    }
}

#[test]
fn warehouse_actual_reader_plans_use_indexed_exact_probes_and_local_usage() {
    let fixture = Fixture::new("reader-plan");
    let db = Connection::open(&fixture.db).unwrap();
    list_functions(&db).unwrap();
    fn plan(db: &Connection, sql: &str, parameters: impl rusqlite::Params) -> String {
        db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(parameters, |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n")
    }
    let list = plan(
        &db,
        LIST,
        params![
            "[]",
            "reader-plan",
            "{}",
            None::<String>,
            None::<String>,
            None::<String>,
            None::<i64>,
            None::<String>,
            false,
            41_i64
        ],
    );
    let exact = plan(&db, EXACT, params![":claude:same", "[]", "reader-plan"]);
    let usage = plan(&db, USAGE, ["[1]", "[]"]);
    for plan in [&list, &exact, &usage] {
        assert!(
            !plan.contains("CORRELATED SCALAR SUBQUERY")
                && !plan.contains("RECURSIVE STEP\nSCAN lineage"),
            "{plan}"
        );
    }
    assert!(exact.contains("session_native_key"), "{exact}");
    assert!(
        !exact
            .lines()
            .any(|line| line == "SCAN d" || line.starts_with("SCAN dim_session")),
        "{exact}"
    );
    assert!(
        usage.contains("lineage_component") && usage.contains("usage_message"),
        "{usage}"
    );
    let parents = plan(&db, PARENTS, ["[1]", "[]"]);
    assert!(!parents.contains("CORRELATED SCALAR SUBQUERY"), "{parents}");
}

#[test]
fn warehouse_search_resumes_at_the_first_unexamined_candidate() {
    let mut fixture = Fixture::new("search-frontier");
    let mut paths = Vec::new();
    for number in 0..=crate::catalog_search::MAX_CANDIDATES {
        let title = if number == crate::catalog_search::MAX_CANDIDATES {
            "Needle"
        } else {
            "Other"
        };
        paths.push(fixture.claude(&format!("row-{number:04}"), title));
    }
    let paths: Vec<_> = paths.iter().map(PathBuf::as_path).collect();
    let source = [fixture.source(&paths)];
    let query = request("q=needle&limit=1");
    let first = read_list(&source, "workspace", &query, None).unwrap();
    assert!(first.rows.is_empty());
    assert_eq!(first.examined, crate::catalog_search::MAX_CANDIDATES);
    let second = read_list(&source, "workspace", &query, first.next.as_ref()).unwrap();
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].title, "Needle");
    assert!(second.next.is_none());
}

#[test]
fn warehouse_reads_keep_verified_growth_and_exclude_unadmitted_replacement() {
    use std::io::Write;
    let mut fixture = Fixture::new("replacement-witness");
    let original = fixture.claude("original", "Original");
    let peer = fixture.claude("peer", "Peer");
    let query = request("limit=1");
    let page = |source| read_list(&[source], "workspace", &query, None).unwrap();
    let first = page(fixture.source(&[&original, &peer]));
    let cursor = first.next.as_ref().unwrap();
    let before = Stat::of(&original).unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(&original).unwrap();
    writeln!(file, "{}", json!({"type":"compact_boundary"})).unwrap();
    drop(file);
    let after = Stat::of(&original).unwrap();
    let db = Connection::open(&fixture.db).unwrap();
    db.execute(
        "INSERT INTO session_source_mutations VALUES(?1,?2)",
        params![
            original.to_str(),
            serde_json::to_string(&AppendEvidence {
                generation: "1".repeat(32),
                before,
                after
            })
            .unwrap()
        ],
    )
    .unwrap();
    let sources = [fixture.source(&[&original, &peer])];
    assert_eq!(
        page(fixture.source(&[&original, &peer])).coverage.committed,
        2
    );
    assert!(
        identity(&sources[0], CatalogReadScope::Current, "original")
            .unwrap()
            .is_some()
    );
    assert!(read_list(&sources, "workspace", &query, Some(cursor)).is_ok());

    let replacement = fixture.dir.join("replacement");
    fs::copy(&original, &replacement).unwrap();
    fs::rename(replacement, &original).unwrap();
    let sources = [fixture.source(&[&original, &peer])];
    let pending = page(fixture.source(&[&original, &peer]));
    assert_eq!(
        pending.coverage,
        Coverage {
            committed: 1,
            pending: 1,
            unavailable: 0
        }
    );
    assert_eq!(keys(&pending), [("peer", "replacement-witness")]);
    assert!(
        identity(&sources[0], CatalogReadScope::Current, "original")
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        read_list(&sources, "workspace", &query, Some(cursor)),
        Err(ReadError::Restart)
    ));
}

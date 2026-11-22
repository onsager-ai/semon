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

fn read_list(
    sources: &[Source<'_>],
    workspace: &str,
    request: &Request,
    cursor: Option<&Continuation>,
) -> Result<Page, ReadError> {
    super::read_list(sources, workspace, request, cursor, sources.len() > 1)
}

fn semantic_transcript(
    options: &Options,
    source: &str,
    query: &str,
    reader: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
) -> crate::ViewerReply {
    crate::transcript_range::session_transcript_range_with_gate(
        options,
        source,
        query,
        reader,
        mode,
        || Ok(()),
    )
}

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
            options: &self.options,
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
            mode: crate::SessionSourceReadMode::LocalThenProvider,
            current_provider: None,
            focused: None,
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
fn workspace_catalog_reports_a_cold_source_alongside_committed_rows() {
    let mut warm = Fixture::new("workspace-warm");
    let mut cold = Fixture::new("workspace-cold");
    warm.claude("ready", "Committed title");
    let mut sources = [
        super::host_source(&warm.options, warm.namespace.as_str()),
        super::host_source(&cold.options, cold.namespace.as_str()),
    ];
    sources[1].options.cache = cold.dir.join("unopened.json");
    let reply = crate::session_catalog_union(&sources, "workspace", "limit=10");
    let json: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{json}");
    assert_eq!(json["coverage"]["committed"], 1);
    assert_eq!(json["coverage"]["pending"], 1);
    assert_eq!(json["completeness"]["state"], "partial");
    assert_eq!(json["items"][0]["key"], "ready");

    cold.claude("later", "Newly committed title");
    sources[1].options.cache = cold.options.cache.clone();
    let reply = crate::session_catalog_union(&sources, "workspace", "limit=10");
    let json: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{json}");
    assert_eq!(json["coverage"]["committed"], 2);
    assert_eq!(json["coverage"]["pending"], 0);
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
    append_native(
        &claude,
        json!({"type":"assistant","uuid":"effort","sessionId":"same","effort":"high",
        "perTurnEffort":" MAX ","timestamp":"2026-09-24T00:00:02Z",
        "message":{"role":"assistant","content":[{"type":"text","text":"Response"}]}}),
    );
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
            super::super::read_page(&fixture.options, &fixture.namespace, &request(query)).unwrap();
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
                    row["effort"].as_str(),
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
                    row.effort.as_deref(),
                )
            })
            .collect();
        assert_eq!(observed, oracle, "{query}");
    }
    let focused = Source::focused(
        fixture.namespace.clone(),
        &fixture.db,
        &fixture.options,
        None,
    )
    .unwrap();
    for (harness, native, key) in [
        ("claude", "same", ":claude:same"),
        ("claude", "claude:same", "claude:same"),
        ("codex", "same", "codex:same"),
        ("copilot", copilot_id, copilot_id),
    ] {
        let aliases = resolve_native(&focused, CatalogReadScope::Current, harness, native).unwrap();
        let resolved = focused_identity(&focused, CatalogReadScope::Current, key)
            .unwrap()
            .unwrap();
        assert_eq!(aliases.as_slice(), std::slice::from_ref(&resolved));
        assert_eq!(resolved.native_id.as_deref(), Some(native));
        assert_eq!(resolved.harness, harness);
        assert_eq!(resolved.source_refs.len(), 1);
        assert_eq!(resolved.source_refs[0].source.native_id, native);
        assert_eq!(resolved.generation.len(), 64);
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
    let detail = plan(
        &db,
        warehouse::DETAIL,
        rusqlite::named_params! {
            ":source": 1, ":start": 0, ":end": 100, ":predecessors": "{}",
            ":entry_block": None::<i64>, ":entry_occurrence": None::<i64>, ":time": i64::MIN,
            ":pos": i64::MIN, ":block": i64::MIN, ":occurrence": i64::MIN, ":limit": 10,
        },
    );
    assert!(
        detail.contains("record_position") && detail.contains("record_uuid"),
        "{detail}"
    );
    assert!(
        !detail.contains("CORRELATED SCALAR SUBQUERY") && !detail.contains("SCAN fact_record"),
        "{detail}"
    );
    let parents = plan(
        &db,
        include_str!("../events/store/warehouse_parent_offer.sql"),
        params![1, false],
    );
    let children = plan(
        &db,
        include_str!("../events/store/warehouse_parent_children.sql"),
        [1],
    );
    assert!(
        parents.contains("claude_call_identity") && parents.contains("process_capture_pid"),
        "{parents}"
    );
    assert!(
        children.contains("session_parent_name") && children.contains("agent_storage_parent"),
        "{children}"
    );
    for query in [&parents, &children] {
        assert!(
            !query
                .lines()
                .any(|line| matches!(line, "SCAN m" | "SCAN g" | "SCAN active" | "SCAN carrier")),
            "{query}"
        );
    }
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
fn warehouse_list_transport_uses_committed_index_while_publication_is_absent() {
    let mut fixture = Fixture::new("warehouse-list-transport");
    fixture.claude("first", "First title");
    fixture.claude("second", "Second title");
    let db = Connection::open(&fixture.db).unwrap();
    let semantic_rows: i64 = db
        .query_row("SELECT count(*) FROM session_catalog", [], |row| row.get(0))
        .unwrap();
    assert_eq!(semantic_rows, 0);
    let configured = [crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    }];
    let get = |query| {
        let reply = http::list(&configured, "fixture", query, false);
        (
            reply.status,
            serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap(),
        )
    };
    let (status, first) = get("limit=1");
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["total"], 2);
    assert_eq!(first["coverage"]["committed"], 2);
    assert_eq!(first["items"][0]["key"], "first");
    assert_eq!(
        first["items"][0]["source_refs"][0]["source"]["native_id"],
        "first"
    );
    let cursor = first["next_cursor"].as_str().unwrap();
    let query = crate::native_html::query(&[("limit", "1"), ("cursor", cursor)]);
    let (status, second) = get(&query);
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["items"][0]["key"], "second");
    assert!(second["next_cursor"].is_null());
    fixture.claude("third", "Third title");
    let (status, stale) = get(&query);
    assert_eq!(status, 409, "{stale}");
    assert_eq!(stale["error"]["code"], "stale_cursor");
}

#[test]
fn warehouse_list_transport_reads_one_commit_while_ingestion_advances() {
    use std::sync::mpsc::sync_channel;

    let mut fixture = Fixture::new("list-transport-concurrent");
    fixture.claude("first", "First title");
    let pending = fixture
        .options
        .claude_home
        .join("projects/proj/second.jsonl");
    fs::write(
        &pending,
        format!("{}\n", prompt("second", 1, "Second title")),
    )
    .unwrap();
    let configured = [crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    }];
    let (opened, snapshot) = sync_channel(0);
    let (committed, continue_read) = sync_channel(0);
    let old = std::thread::scope(|scope| {
        let configured = &configured;
        let reader = scope.spawn(move || {
            AFTER_LIST_SNAPSHOT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    opened.send(()).unwrap();
                    continue_read.recv_timeout(Duration::from_secs(5)).unwrap();
                }));
            });
            http::list(configured, "fixture", "limit=10", false)
        });
        snapshot.recv_timeout(Duration::from_secs(5)).unwrap();
        scan_file(&pending, "claude", &mut fixture.cache, &mut false).unwrap();
        committed.send(()).unwrap();
        reader.join().unwrap()
    });
    let first: serde_json::Value = serde_json::from_slice(&old.body).unwrap();
    assert_eq!(old.status, 200, "{first}");
    assert_eq!(first["total"], 1);
    assert_eq!(
        first["coverage"],
        json!({"committed":1,"pending":1,"unavailable":0})
    );
    assert_eq!(first["items"][0]["key"], "first");
    let fresh = http::list(&configured, "fixture", "limit=10", false);
    let second: serde_json::Value = serde_json::from_slice(&fresh.body).unwrap();
    assert_eq!(fresh.status, 200, "{second}");
    assert_eq!(second["total"], 2);
    assert_eq!(
        second["coverage"],
        json!({"committed":2,"pending":0,"unavailable":0})
    );
    assert_eq!(second["items"][0]["key"], "second");
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

fn append_native(path: &Path, record: serde_json::Value) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{record}").unwrap();
}

fn prompt(id: &str, second: usize, text: &str) -> serde_json::Value {
    json!({"type":"user","uuid":format!("{id}-{second}"),"sessionId":id,
        "timestamp":format!("2026-09-24T00:{:02}:{:02}Z",second / 60,second % 60),
        "message":{"role":"user","content":text}})
}

fn selected_page(
    fixture: &Fixture,
    paths: &[&Path],
    key: &str,
    limit: usize,
    cursor: Option<&selected::Cursor>,
) -> Result<Option<selected::Component>, ReadError> {
    selected::page(
        &fixture.options,
        &fixture.source(paths),
        CatalogReadScope::Current,
        key,
        limit,
        cursor,
    )
}

#[test]
fn warehouse_focused_reads_require_only_the_selected_committed_component() {
    let mut fixture = Fixture::new("focused-authority");
    let path = fixture.claude("selected", "Selected prompt");
    let unrelated = fixture.claude("unrelated", "Unrelated prompt");
    fs::remove_file(unrelated).unwrap();
    let source = Source::focused(
        fixture.namespace.clone(),
        &fixture.db,
        &fixture.options,
        None,
    )
    .unwrap();
    assert!(source.inventory.is_empty());
    let identity = identity(&source, CatalogReadScope::Current, "selected")
        .unwrap()
        .unwrap();
    assert_eq!(identity.2, "selected");
    let page = selected::latest_page(&fixture.options, &source, "selected", 10, None)
        .unwrap()
        .unwrap();
    let prompts = page
        .records
        .iter()
        .filter_map(selected::Record::text)
        .collect::<Vec<_>>();
    assert_eq!(prompts, ["Selected prompt"]);
    let replacement = path.with_extension("replacement");
    fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
    fs::rename(replacement, path).unwrap();
    assert!(
        super::identity(&source, CatalogReadScope::Current, "selected")
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        selected::latest_page(&fixture.options, &source, "selected", 10, None),
        Err(ReadError::Restart)
    ));
}

#[test]
fn warehouse_latest_window_pages_earlier_with_the_same_committed_prefix() {
    let mut fixture = Fixture::new("latest-window");
    let path = fixture.claude("native", "Oldest prompt");
    for second in 1..=4 {
        append_native(&path, prompt("native", second, &format!("Prompt {second}")));
    }
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let text = |page: selected::Component| {
        page.records
            .iter()
            .filter_map(selected::Record::text)
            .collect::<Vec<_>>()
    };
    let latest = |fixture: &Fixture, cursor: Option<&selected::Cursor>| {
        selected::latest_page(
            &fixture.options,
            &fixture.source(&[&path]),
            "native",
            2,
            cursor,
        )
        .unwrap()
        .unwrap()
    };
    let mut tail = latest(&fixture, None);
    let tail_cursor = tail.next.take().unwrap();
    assert_eq!(text(tail), ["Prompt 3", "Prompt 4"]);
    append_native(&path, prompt("native", 5, "Newly committed"));
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    assert_eq!(
        text(latest(&fixture, Some(&tail_cursor))),
        ["Prompt 1", "Prompt 2"]
    );
    assert_eq!(
        text(latest(&fixture, None)),
        ["Prompt 4", "Newly committed"]
    );
    assert!(matches!(
        selected_page(&fixture, &[&path], "native", 2, Some(&tail_cursor)),
        Err(ReadError::Restart)
    ));
}

#[test]
fn warehouse_transcript_transport_keeps_earlier_windows_and_exact_addresses_across_appends() {
    let mut fixture = Fixture::new("transcript-transport");
    let path = fixture.claude("native", "Oldest prompt");
    for second in 1..=4 {
        append_native(&path, prompt("native", second, &format!("Prompt {second}")));
    }
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let read = |parameters: &[(&str, &str)]| {
        let query = crate::native_html::query(parameters);
        let reply = http::transcript(&configured, &query, None, None);
        (
            reply.status,
            serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap(),
        )
    };
    let latest = || {
        let (status, page) = read(&[("sid", "native"), ("limit", "2")]);
        assert_eq!(status, 200, "{page}");
        page
    };
    let tail = latest();
    assert_eq!(tail["projection"], 7);
    let exact = http::identity(&configured, "sid=native");
    assert_eq!(exact.status, 200);
    let exact: serde_json::Value = serde_json::from_slice(&exact.body).unwrap();
    assert_eq!(
        tail["identity"]["generation"],
        exact["identity"]["generation"]
    );
    assert_eq!(tail["entries"][0]["text"], "Prompt 3");
    assert_eq!(tail["entries"][1]["text"], "Prompt 4");
    assert!(tail["entries"][1].get("slot").is_none());
    let address = tail["entries"][1]["address"].to_string();
    let turn = tail["entries"][1]["turn"].as_str().unwrap();
    let anchor = http::anchor(
        &configured,
        &crate::native_html::query(&[("sid", "native"), ("turn", turn)]),
        None,
        None,
    );
    let body: serde_json::Value = serde_json::from_slice(&anchor.body).unwrap();
    assert_eq!(anchor.status, 200, "{body}");
    assert_eq!(body["address"], tail["entries"][1]["address"]);

    for second in 5..=7 {
        append_native(&path, prompt("native", second, &format!("Prompt {second}")));
        assert_eq!(
            latest()["entries"][1]["text"],
            format!("Prompt {}", second - 1)
        );
        scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
        assert_eq!(latest()["entries"][1]["text"], format!("Prompt {second}"));
    }
    let (status, earlier) = read(&[
        ("sid", "native"),
        ("limit", "2"),
        ("cursor", tail["earlier_cursor"].as_str().unwrap()),
    ]);
    assert_eq!(status, 200, "{earlier}");
    assert_eq!(earlier["position"], "earlier");
    assert!(tail["session"].is_object());
    assert!(earlier["session"].is_null());
    assert_eq!(earlier["entries"][0]["text"], "Prompt 1");
    assert_eq!(earlier["entries"][1]["text"], "Prompt 2");
    let (status, exact) = read(&[("sid", "native"), ("address", &address)]);
    assert_eq!(status, 200, "{exact}");
    assert_eq!(exact["position"], "address");
    assert_eq!(
        exact["entries"][0]["entry_id"],
        tail["entries"][1]["entry_id"]
    );

    let mut text = "π🗺".repeat(25_000);
    text.push_str(" end");
    let image = b"\x89PNG\r\n\x1a\nexample";
    let mut record = prompt("native", 8, &text);
    record["message"]["content"] = json!([
        {"type":"text","text":text},
        {"type":"image","source":{"type":"base64","media_type":"image/png",
            "data":crate::attachments::tests::encode(image)}},
    ]);
    append_native(&path, record);
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let page = latest();
    let long = &page["entries"][1];
    let chunks = long["field"]["chunks"].as_u64().unwrap();
    assert!(chunks > 1);
    assert!(long.get("img").is_some());
    let long_address = long["address"].to_string();
    let expand = |arguments: &[(&str, &str)]| {
        let mut params = vec![("sid", "native"), ("address", long_address.as_str())];
        params.extend_from_slice(arguments);
        http::entry(&configured, &crate::native_html::query(&params), None, None)
    };
    let mut reconstructed = String::new();
    for chunk in 0..chunks {
        let reply = expand(&[("field_chunk", &chunk.to_string())]);
        let value: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(reply.status, 200, "{value}");
        reconstructed.push_str(value["entry"]["text"].as_str().unwrap());
    }
    assert_eq!(reconstructed, text);
    let attachment = expand(&[("part", "attachment"), ("image", "0")]);
    assert_eq!(attachment.status, 200);
    assert_eq!(attachment.content_type, "image/png");
    assert_eq!(attachment.body, image);

    let mut oversized_image = image.to_vec();
    oversized_image.resize(1024 * 1024, 0);
    let long_text = "x".repeat(768 * 1024 + 1) + "verified tail";
    let mut mixed = prompt("native", 9, &long_text);
    mixed["message"]["content"] = json!([
        {"type":"image","source":{"type":"base64","media_type":"image/png",
            "data":crate::attachments::tests::encode(&oversized_image)}},
        {"type":"text","text":long_text},
        {"type":"image","source":{"type":"base64","media_type":"image/png",
            "data":crate::attachments::tests::encode(image)}},
    ]);
    append_native(&path, mixed);
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let page = latest();
    let mixed = &page["entries"][1];
    assert_eq!(mixed["img"].as_array().unwrap().len(), 2);
    let last_chunk = (mixed["field"]["chunks"].as_u64().unwrap() - 1).to_string();
    let mixed_address = mixed["address"].to_string();
    let expand_mixed = |arguments: &[(&str, &str)]| {
        let mut params = vec![("sid", "native"), ("address", mixed_address.as_str())];
        params.extend_from_slice(arguments);
        http::entry(&configured, &crate::native_html::query(&params), None, None)
    };
    let last = expand_mixed(&[("field_chunk", &last_chunk)]);
    let value: serde_json::Value = serde_json::from_slice(&last.body).unwrap();
    assert_eq!(last.status, 200, "{value}");
    assert_eq!(value["entry"]["text"], "xverified tail");
    for (index, expected) in [("0", oversized_image.as_slice()), ("1", image.as_slice())] {
        let attachment = expand_mixed(&[("part", "attachment"), ("image", index)]);
        assert_eq!(attachment.status, 200);
        assert_eq!(attachment.content_type, "image/png");
        assert_eq!(attachment.body.as_slice(), expected);
    }

    let replacement = path.with_extension("replacement");
    fs::write(
        &replacement,
        format!("{}\n", prompt("native", 0, "Replacement")),
    )
    .unwrap();
    fs::rename(replacement, &path).unwrap();
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    assert_eq!(read(&[("sid", "native"), ("address", &address)]).0, 409);
}

#[test]
fn warehouse_failed_tool_marker_opens_the_rendered_entry() {
    let mut fixture = Fixture::new("native-failure-marker");
    let path = fixture.claude("native", "Original prompt");
    append_native(
        &path,
        json!({"type":"assistant","uuid":"call","sessionId":"native",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Checking the result"},
            {"type":"tool_use","id":"command","name":"Bash","input":{"command":"pwd"}}
        ]}}),
    );
    append_native(
        &path,
        json!({"type":"user","uuid":"reply","sessionId":"native",
        "timestamp":"2026-09-24T00:00:02Z","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"command","content":"failure","is_error":true}
        ]}}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let parse = |reply: crate::viewer::ViewerReply| {
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(reply.status, 200, "{body}");
        body
    };
    let page = parse(http::transcript(&configured, "sid=native", None, None));
    let tool = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["k"] == "tool")
        .unwrap();
    assert_eq!(tool["ok"], false);
    let query = crate::native_html::query(&[
        ("sid", "native"),
        ("kind", "errors"),
        ("generation", page["errors_generation"].as_str().unwrap()),
    ]);
    let markers = parse(http::annotations(&configured, &query, None, None));
    assert_eq!(markers["total"], 1);
    assert_eq!(markers["entries"][0]["address"], tool["address"]);
    let address = markers["entries"][0]["address"].to_string();
    let query = crate::native_html::query(&[("sid", "native"), ("address", &address)]);
    let exact = parse(http::transcript(&configured, &query, None, None));
    assert!(
        exact["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["address"] == tool["address"])
    );
}

#[test]
fn warehouse_component_cursor_pages_members_and_pins_unvisited_prefixes() {
    let mut fixture = Fixture::new("component-cursor");
    let mut paths = Vec::new();
    for index in 0..70 {
        let id = format!("member-{index:02}");
        let path = fixture
            .options
            .claude_home
            .join(format!("projects/proj/{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                json!({"type":"bridge-session","bridgeSessionId":"many-members"}),
                prompt(&id, index, &id)
            ),
        )
        .unwrap();
        scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
        paths.push(path);
    }
    let inventory: Vec<_> = paths.iter().map(PathBuf::as_path).collect();
    let owners = read_list(
        &[fixture.source(&inventory)],
        "workspace",
        &request("limit=2"),
        None,
    )
    .unwrap();
    assert_eq!(owners.total, 1);
    let key = &owners.rows[0].position.key;
    let first = selected_page(&fixture, &inventory, key, 1, None)
        .unwrap()
        .unwrap();
    assert_eq!(first.member_count, 70);
    assert_eq!(first.harness, "claude");
    assert_eq!(&first.native_id, key);
    assert_eq!(first.next.as_ref().unwrap().touched_count(), 1);
    let token = serde_json::to_string(&first.next).unwrap();
    let cursor: selected::Cursor = serde_json::from_str(&token).unwrap();

    // The later page opens a fresh snapshot after this database commit. This
    // source has no receipt in the cursor, and its new record sorts in range.
    append_native(&paths[69], prompt("member-69", 71, "after first page"));
    scan_file(&paths[69], "claude", &mut fixture.cache, &mut false).unwrap();
    let path = paths[1].clone();
    let cache = fixture.options.cache.clone();
    selected::AFTER_BODY.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            append_native(&path, prompt("member-01", 72, "during body fetch"));
            let mut cache = EventCache::open_scoped(&cache).unwrap();
            scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        }))
    });
    let second = selected_page(&fixture, &inventory, key, 8, Some(&cursor))
        .unwrap()
        .unwrap();
    assert!(second.next.as_ref().unwrap().touched_count() <= 8);
    let mut addresses = std::collections::BTreeSet::new();
    let mut text = Vec::new();
    let mut last = None;
    let mut page = second;
    for record in first.records {
        assert!(addresses.insert((
            record.address.path,
            record.address.offset,
            record.address.block
        )));
    }
    loop {
        for record in page.records {
            assert!(addresses.insert((
                record.address.path.clone(),
                record.address.offset,
                record.address.block
            )));
            if let Some(value) = record.text() {
                if value == "member-69" {
                    last = Some(record.address);
                }
                text.push(value);
            }
        }
        let Some(cursor) = page.next else { break };
        assert!(cursor.touched_count() <= 8);
        let cursor = serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        page = selected_page(&fixture, &inventory, key, 8, Some(&cursor))
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        text,
        (0..70)
            .map(|i| format!("member-{i:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(addresses.len(), 140);
    let record = selected::record_at(
        &fixture.options,
        &fixture.source(&inventory),
        key,
        &last.unwrap(),
        Some(&cursor),
    )
    .unwrap()
    .unwrap();
    assert_eq!(record.text().as_deref(), Some("member-69"));

    let fresh = selected_page(&fixture, &inventory, key, 100, None)
        .unwrap()
        .unwrap();
    let tail = selected_page(&fixture, &inventory, key, 100, fresh.next.as_ref())
        .unwrap()
        .unwrap();
    assert!(
        tail.records
            .iter()
            .any(|record| record.text().as_deref() == Some("after first page"))
    );
    assert!(
        tail.records
            .iter()
            .any(|record| record.text().as_deref() == Some("during body fetch"))
    );

    let late = fixture.claude("late-member", "Late member");
    append_native(
        &late,
        json!({"type":"continued-in","continuedInSessionId":"member-69"}),
    );
    scan_file(&late, "claude", &mut fixture.cache, &mut false).unwrap();
    let mut inventory = inventory;
    inventory.push(&late);
    assert!(matches!(
        selected_page(&fixture, &inventory, key, 8, Some(&cursor)),
        Err(ReadError::Restart)
    ));
}

#[test]
fn warehouse_component_turn_crosses_a_continuation_at_the_selected_prefix() {
    let mut fixture = Fixture::new("component-cross-source-turn");
    let original = fixture.claude("original", "First prompt");
    let resumed = original.with_file_name("resumed.jsonl");
    let original_text = fs::read_to_string(&original).unwrap();
    let mut copied: serde_json::Value =
        serde_json::from_str(original_text.lines().next().unwrap()).unwrap();
    copied["sessionId"] = json!("resumed");
    fs::write(&resumed, format!("{copied}\n")).unwrap();
    append_native(
        &original,
        json!({"type":"continued-in","sessionId":"original",
        "continuedInSessionId":"resumed","timestamp":"2026-09-24T00:00:01Z"}),
    );
    append_native(
        &resumed,
        json!({"type":"assistant","uuid":"resumed-reply","sessionId":"resumed",
        "timestamp":"2026-09-24T00:00:03Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Reply after resuming"}
        ]}}),
    );
    scan_file(&original, "claude", &mut fixture.cache, &mut false).unwrap();
    scan_file(&resumed, "claude", &mut fixture.cache, &mut false).unwrap();
    let inventory = [original.as_path(), resumed.as_path()];
    let first = selected_page(&fixture, &inventory, "original", 1, None)
        .unwrap()
        .unwrap();
    assert_eq!(first.member_count, 2);
    let cursor = first.next.as_ref().unwrap();

    // This prompt sorts before the reply but did not exist when the page opened.
    append_native(&original, prompt("original", 2, "A later commit"));
    scan_file(&original, "claude", &mut fixture.cache, &mut false).unwrap();
    let source = fixture.source(&inventory);
    let evidence = |id, cursor| {
        selected::evidence_at(
            &fixture.options,
            &source,
            "original",
            selected::Evidence::Event(id),
            cursor,
        )
        .unwrap()
        .unwrap()
    };
    let prompt = evidence("original", Some(cursor));
    let selected = evidence("resumed-reply", Some(cursor));
    let turn = selected.causal.turn.as_ref().unwrap();
    assert_eq!(turn.address, prompt.address);
    assert_eq!(Some(&turn.id), prompt.prompt_anchor.as_ref());
    let continued = selected_page(&fixture, &inventory, "original", 10, Some(cursor))
        .unwrap()
        .unwrap();
    let reply = continued
        .records
        .iter()
        .find(|record| record.address == selected.address)
        .unwrap();
    assert_eq!(reply.causal.turn, selected.causal.turn);
    let fresh = evidence("resumed-reply", None);
    assert_eq!(
        fresh.causal.turn.unwrap().address,
        evidence("original-2", None).address
    );

    // An opaque input line after the reply resets the open prompt at its
    // inherited time. It does not affect the earlier page's snapshot.
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new().append(true).open(&resumed).unwrap(),
        "invalid native line"
    )
    .unwrap();
    append_native(
        &resumed,
        json!({"type":"assistant","uuid":"after-gap","sessionId":"resumed",
        "timestamp":"2026-09-24T00:00:04Z","message":{"role":"assistant","content":[
            {"type":"text","text":"After the gap"}
        ]}}),
    );
    scan_file(&resumed, "claude", &mut fixture.cache, &mut false).unwrap();
    assert!(
        selected::evidence_at(
            &fixture.options,
            &fixture.source(&inventory),
            "original",
            selected::Evidence::Event("after-gap"),
            None
        )
        .unwrap()
        .unwrap()
        .causal
        .turn
        .is_none()
    );
}

#[test]
fn warehouse_component_refuses_rewrites_replacements_and_truncation() {
    for (case, change) in [
        "rewrite",
        "replacement",
        "truncation",
        "custody",
        "incarnation",
        "proof",
        "during-body",
        "prefix-receipt",
    ]
    .into_iter()
    .enumerate()
    {
        let mut fixture = Fixture::new(&format!("component-refusal-{case}"));
        let path = fixture.claude("native", "Original content");
        let first = selected_page(&fixture, &[&path], "native", 1, None)
            .unwrap()
            .unwrap();
        let cursor = first.next.as_ref().unwrap();
        let mut source = fixture.source(&[&path]);
        match change {
            "rewrite" => {
                let raw = fs::read_to_string(&path).unwrap();
                fs::write(&path, raw.replace("Original", "Modified")).unwrap();
            }
            "replacement" => {
                let new = path.with_extension("new");
                fs::copy(&path, &new).unwrap();
                fs::rename(new, &path).unwrap();
            }
            "truncation" => fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(1)
                .unwrap(),
            "custody" => {
                source
                    .inventory
                    .insert(path.to_str().unwrap().into(), Availability::Unavailable);
            }
            "incarnation" => {
                Connection::open(&fixture.db).unwrap().execute(
                "UPDATE meta SET value='00000000000000000000000000000000' WHERE key='incarnation'", [],
            ).unwrap();
            }
            "proof" => {
                let db = Connection::open(&fixture.db).unwrap();
                let proof = format!(
                    "native_source_chunks:{:x}:header",
                    Sha256::digest(path.to_str().unwrap().as_bytes())
                );
                assert_eq!(
                    db.execute("DELETE FROM meta WHERE key=?1", [proof])
                        .unwrap(),
                    1
                );
            }
            "prefix-receipt" => {
                Connection::open(&fixture.db)
                    .unwrap()
                    .execute(
                        "UPDATE warehouse_prefix SET ledger=json_set(ledger,'$.prefix[0]',1-CAST(json_extract(ledger,'$.prefix[0]')=1 AS INTEGER))",
                        [],
                    )
                    .unwrap();
            }
            "during-body" => {
                let path = path.clone();
                selected::AFTER_BODY.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        let raw = fs::read_to_string(&path).unwrap();
                        fs::write(&path, raw.replace("Original", "Modified")).unwrap();
                        append_native(&path, prompt("native", 3, "unqualified growth"));
                    }))
                });
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                selected::page(
                    &fixture.options,
                    &source,
                    CatalogReadScope::Current,
                    "native",
                    1,
                    Some(cursor)
                ),
                Err(ReadError::Restart)
            ),
            "{change}"
        );
    }
    let mut fixture = Fixture::new("component-unverified-history");
    let path = fixture.claude("native", "Uncertified current source");
    assert!(matches!(
        selected::page(
            &fixture.options,
            &fixture.source(&[&path]),
            CatalogReadScope::RetainedHistory,
            "native",
            1,
            None
        ),
        Err(ReadError::RetainedUnverified)
    ));
}

#[test]
fn warehouse_component_text_address_and_field_match_the_current_reader() {
    let mut fixture = Fixture::new("component-oracle");
    let path = fixture.claude("native", "Original title");
    append_native(
        &path,
        json!({"type":"assistant","uuid":"answer","sessionId":"native",
        "timestamp":"2026-09-24T00:00:05Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Qualified answer"}
        ]}}),
    );
    let codex = fixture.codex("codex-native");
    for (role, part, text, second) in [
        ("user", "input_text", "Codex prompt", 1),
        ("assistant", "output_text", "Codex answer", 2),
    ] {
        append_native(
            &codex,
            json!({"type":"response_item","timestamp":format!("2026-09-24T00:00:0{second}Z"),
            "payload":{"type":"message","role":role,"content":[{"type":part,"text":text}]}}),
        );
    }
    let copilot_id = "01234567-89ab-4cde-8123-456789abcdef";
    let copilot = fixture.copilot(copilot_id);
    for (kind, text, second) in [
        ("user.message", "Copilot prompt", 1),
        ("assistant.message", "Copilot answer", 2),
    ] {
        append_native(
            &copilot,
            json!({"type":kind,"id":format!("event-{second}"),
            "timestamp":format!("2026-09-24T00:00:0{second}Z"),"data":{"content":text}}),
        );
    }
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    for (key, path, prompt, answer) in [
        (
            "native",
            path.as_path(),
            "Original title",
            "Qualified answer",
        ),
        (
            "codex-native",
            codex.as_path(),
            "Codex prompt",
            "Codex answer",
        ),
        (
            copilot_id,
            copilot.as_path(),
            "Copilot prompt",
            "Copilot answer",
        ),
    ] {
        let actual = selected_page(&fixture, &[path], key, 100, None)
            .unwrap()
            .unwrap();
        if key == "native" {
            assert_eq!(actual.title.as_deref(), Some(prompt));
            assert!(actual.model.is_none());
        }
        let mode = crate::SessionSourceReadMode::LocalOrPinnedProvider;
        let oracle = semantic_transcript(
            &fixture.options,
            fixture.namespace.as_str(),
            &format!("sid={key}&after=0&limit=100"),
            None,
            mode,
        );
        assert_eq!(
            oracle.status,
            200,
            "{}",
            String::from_utf8_lossy(&oracle.body)
        );
        let oracle: serde_json::Value = serde_json::from_slice(&oracle.body).unwrap();
        let expected: Vec<_> = oracle["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| {
                entry["text"]
                    .as_str()
                    .is_some_and(|text| text == prompt || text == answer)
            })
            .collect();
        assert_eq!(expected.len(), 2);
        let items: Vec<_> = actual
            .records
            .iter()
            .filter_map(|record| record.text().map(|text| (record, text)))
            .collect();
        assert_eq!(items.len(), expected.len());
        for ((record, text), entry) in items.into_iter().zip(&expected) {
            assert_eq!(record.entry_anchor.as_deref(), entry["entry_id"].as_str());
            assert_eq!(entry["text"], text);
            assert_eq!(entry["provenance"]["source"]["path"], record.address.path);
            assert_eq!(entry["provenance"]["offset"], record.address.offset);
            assert_eq!(entry["provenance"]["block"], record.address.block);
            let expanded = selected::record_at(
                &fixture.options,
                &fixture.source(&[path]),
                key,
                &record.address,
                None,
            )
            .unwrap()
            .unwrap();
            let reply = crate::transcript_range::session_entry_field_with_gate(
                &fixture.options,
                fixture.namespace.as_str(),
                &format!(
                    "sid={key}&after={}&limit=1&generation={}&field_chunk=0",
                    entry["slot"],
                    oracle["projection"]["generation"].as_str().unwrap()
                ),
                None,
                mode,
                || Ok(()),
            );
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
            let field: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(field["text"].as_str(), expanded.text().as_deref());
        }
        let turn = expected[0]["turn"].as_str().unwrap();
        let reply = crate::transcript_range::session_turn_anchor_with_gate(
            &fixture.options,
            fixture.namespace.as_str(),
            &format!("sid={key}&turn={}", crate::viewer::percent_encode(turn)),
            || Ok(()),
        );
        assert_eq!(reply.status, 200);
        let anchor: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(anchor["slot"], expected[0]["slot"]);
        let prompt_record = actual
            .records
            .iter()
            .find(|record| record.text().as_deref() == Some(prompt))
            .unwrap();
        assert_eq!(prompt_record.prompt_anchor.as_deref(), Some(turn));
        let address = &prompt_record.address;
        assert_eq!(
            selected::record_at(
                &fixture.options,
                &fixture.source(&[path]),
                key,
                address,
                None
            )
            .unwrap()
            .unwrap()
            .text()
            .as_deref(),
            Some(prompt)
        );
    }
}

#[test]
fn warehouse_operation_and_activity_anchors_match_native_evidence() {
    use crate::events::session_activity::ActivityStepId;
    let mut fixture = Fixture::new("operation-activity-anchors");
    let claude = fixture.claude("native", "Review this");
    append_native(
        &claude,
        json!({"type":"assistant","uuid":"usage-uuid","sessionId":"native",
        "timestamp":"2026-09-24T00:00:03Z","message":{"id":"activity-message","role":"assistant",
        "model":"claude-sonnet-4-6","content":[{"type":"text","text":"Review complete"}],
        "usage":{"input_tokens":100,"output_tokens":10}}}),
    );
    let codex = fixture.codex("codex-native");
    let turn_offset = fs::metadata(&codex).unwrap().len();
    append_native(
        &codex,
        json!({"type":"event_msg","timestamp":"2026-09-24T00:00:00Z",
        "payload":{"type":"task_started","turn_id":"owned-turn"}}),
    );
    let call = fs::metadata(&codex).unwrap().len();
    append_native(
        &codex,
        json!({"type":"response_item","timestamp":"2026-09-24T00:00:01Z",
        "payload":{"type":"custom_tool_call","call_id":"exec","name":"exec","input":"tools.exec_command({cmd:'false'})"}}),
    );
    let operation = fs::metadata(&codex).unwrap().len();
    append_native(
        &codex,
        json!({"type":"event_msg","timestamp":"2026-09-24T00:00:02Z",
        "payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"operation",
            "command":["/bin/sh","-c","false"],"exit_code":1,"aggregated_output":"failed"}}}),
    );
    append_native(
        &codex,
        json!({"type":"response_item","timestamp":"2026-09-24T00:00:03Z",
        "payload":{"type":"custom_tool_call_output","call_id":"exec","output":"done"}}),
    );
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();

    let db = Connection::open(&fixture.db).unwrap();
    let generation: String = db
        .query_row(
            "SELECT generation FROM session_activity_projections WHERE session_key='native'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut anchor = crate::transcript_range::StepAnchor {
        generation,
        ordinal: 0,
        id: ActivityStepId::Claude("activity-message".into()),
    };
    let source = fixture.source(&[&claude, &codex]);
    let component = selected::activity_anchor(&source, "native", &anchor)
        .unwrap()
        .unwrap();
    let found = &component.records[0];
    assert_eq!(found.text().as_deref(), Some("Review complete"));
    let oracle = |key| {
        let reply = semantic_transcript(
            &fixture.options,
            fixture.namespace.as_str(),
            &format!("sid={key}&after=0&limit=100"),
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
        );
        assert_eq!(reply.status, 200);
        serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap()
    };
    let transcript = oracle("native");
    let entry = transcript["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Review complete")
        .unwrap();
    assert_eq!(found.entry_anchor.as_deref(), entry["entry_id"].as_str());
    anchor.ordinal = 1;
    assert!(
        selected::activity_anchor(&source, "native", &anchor)
            .unwrap()
            .is_none()
    );
    anchor.ordinal = 0;
    anchor.generation.push_str("-stale");
    assert!(
        selected::activity_anchor(&source, "native", &anchor)
            .unwrap()
            .is_none()
    );

    let page = selected_page(&fixture, &[&codex], "codex-native", 100, None)
        .unwrap()
        .unwrap();
    let found = page
        .records
        .iter()
        .find(|record| record.address.offset == operation)
        .unwrap();
    assert_eq!(found.operation.unwrap().parent, call);
    let (row, _) = focused_metadata(&source, "codex-native").unwrap().unwrap();
    let details = row.details.unwrap();
    assert_eq!(details["calls"], 1);
    assert_eq!(details["errors"], 1);
    assert_eq!(details["tool_calls"], json!({"exec_command":1}));
    let lifecycle = &details["turn_lifecycle"];
    assert_eq!(lifecycle["phase"], "active");
    assert_eq!(lifecycle["turn_id"], "owned-turn");
    assert_eq!(lifecycle["offset"], turn_offset);
    assert_eq!(lifecycle["source_generation"].as_str().unwrap().len(), 64);
    let transcript = oracle("codex-native");
    let entry = transcript["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["script"] == call)
        .unwrap();
    assert_eq!(found.entry_anchor.as_deref(), entry["entry_id"].as_str());

    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let body = |reply: crate::viewer::ViewerReply| {
        let value: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(reply.status, 200, "{value}");
        value
    };
    let page = body(http::transcript(&configured, "sid=native", None, None));
    let turn = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Review this")
        .unwrap()["turn"]
        .as_str()
        .unwrap();
    let home_query = |generation: &str| {
        crate::native_html::query(&[
            ("sid", "native"),
            ("scope", "current"),
            ("generation", generation),
            ("session_fields", "[\"detail\"]"),
            ("turns", &json!([turn]).to_string()),
        ])
    };
    let generation = page["identity"]["generation"].as_str().unwrap();
    let home = body(http::home(&configured, &home_query(generation), None, None));
    assert_eq!(home["turns"][turn], "Review this");
    assert_eq!(home["session"]["detail"], "Review this");
    assert_eq!(home["session"]["detail_from"], "you");
    assert_eq!(home["incomplete"], false);
    assert_eq!(
        http::home(&configured, &home_query(&"0".repeat(64)), None, None).status,
        409
    );
    let usage_entry = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Review complete")
        .unwrap();
    let activity = |generation: &str| {
        let step = json!(anchor.id).to_string();
        let query = crate::native_html::query(&[
            ("sid", "native"),
            ("generation", generation),
            ("ordinal", "0"),
            ("step", &step),
        ]);
        body(http::activity_anchor(&configured, &query, None, None))
    };
    assert_eq!(activity(&anchor.generation)["available"], false);
    let generation = anchor.generation.strip_suffix("-stale").unwrap();
    let evidence = activity(generation);
    assert_eq!(evidence["available"], true);
    assert_eq!(evidence["address"], usage_entry["address"]);

    let page = body(http::transcript(
        &configured,
        "sid=codex-native",
        None,
        None,
    ));
    let operation = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["script"] == call)
        .unwrap();
    assert_eq!(operation["ok"], false);
    assert_eq!(operation["entry_id"], entry["entry_id"]);
    let markers = body(http::annotations(
        &configured,
        "sid=codex-native&kind=errors",
        None,
        None,
    ));
    assert_eq!(markers["total"], 1);
    assert_eq!(markers["entries"][0]["address"], operation["address"]);
    let address = operation["address"].to_string();
    let query = crate::native_html::query(&[
        ("sid", "codex-native"),
        ("address", &address),
        ("part", "script"),
    ]);
    let script = body(http::entry(&configured, &query, None, None));
    assert_eq!(
        script["entry"]["scriptText"],
        "tools.exec_command({cmd:'false'})"
    );
}

#[test]
fn warehouse_component_cursor_restarts_after_receipt_expiry() {
    let mut fixture = Fixture::new("component-expiry");
    let path = fixture.claude("native", "Original title");
    let quiet = fixture.claude("quiet", "Keep newest quiet receipt");
    let inventory = [&path as &Path, &quiet];
    let first = selected_page(&fixture, &inventory, "native", 1, None)
        .unwrap()
        .unwrap();
    let cursor = first.next.unwrap();
    let cutoff = crate::model::now_ms() - warehouse::PREFIX_WINDOW_MS - 1;
    let mut expired = serde_json::to_value(&cursor).unwrap();
    expired["opened_at"] = json!(cutoff);
    let expired = serde_json::from_value(expired).unwrap();
    assert!(matches!(
        selected_page(&fixture, &inventory, "native", 1, Some(&expired)),
        Err(ReadError::Restart)
    ));

    let db = Connection::open(&fixture.db).unwrap();
    db.execute("UPDATE warehouse_prefix SET committed_at=?1", [cutoff])
        .unwrap();
    // The first cursor's watermark includes the quiet receipt, so advance the
    // native member to the watermark before creating the cursor we will prune.
    append_native(&path, prompt("native", 1, "Intermediate"));
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let recent = selected_page(&fixture, &inventory, "native", 1, None)
        .unwrap()
        .unwrap()
        .next
        .unwrap();
    db.execute("UPDATE warehouse_prefix SET committed_at=?1", [cutoff])
        .unwrap();
    append_native(&path, prompt("native", 2, "Newest"));
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM warehouse_prefix", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
    assert!(matches!(
        selected_page(&fixture, &inventory, "native", 1, Some(&recent)),
        Err(ReadError::Restart)
    ));
    let latest = selected_page(&fixture, &inventory, "native", 10, None)
        .unwrap()
        .unwrap();
    assert!(
        latest
            .records
            .iter()
            .any(|record| record.text().as_deref() == Some("Newest"))
    );
    assert!(
        selected_page(&fixture, &inventory, "quiet", 1, None)
            .unwrap()
            .is_some()
    );
}

#[test]
fn warehouse_component_tool_results_use_the_selected_prefix() {
    let mut fixture = Fixture::new("component-late-result");
    let path = fixture.claude("native", "Original title");
    let at = fs::metadata(&path).unwrap().len();
    let peer = fixture.claude("peer", "Peer title");
    let receipt_offset = fs::metadata(&peer).unwrap().len();
    append_native(
        &peer,
        json!({"type":"user","timestamp":"2026-09-24T00:00:03Z","sessionId":"peer",
        "origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/111.sock","name":"native",
            "fromMode":"prompting","msg_id":"correlation","body":"hello","verifiedPeerPid":111,"verifiedPeerProcStart":"999"},
        "message":{"role":"user","content":"<cross-session-message from=\"uds:/run/user/1000/cc-socks/111.sock\" from-name=\"native\">hello</cross-session-message>"}}),
    );
    scan_file(&peer, "claude", &mut fixture.cache, &mut false).unwrap();
    append_native(
        &path,
        json!({"type":"assistant","uuid":"call","sessionId":"native",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"tool-1","name":"SendMessage","input":{"to":"peer","message":"hello"}}
        ]}}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let first = selected_page(&fixture, &[&path, &peer], "native", 1, None)
        .unwrap()
        .unwrap();
    let call = selected::Address {
        path: "projects/proj/native.jsonl".into(),
        offset: at,
        block: 0,
        occurrence: 0,
    };
    append_native(
        &path,
        json!({"type":"user","uuid":"result-event","sessionId":"native","timestamp":"2026-09-24T00:00:02Z",
        "toolUseResult":{"success":true,"msg_id":"correlation"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"qualified result"}]}}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let source = fixture.source(&[&path, &peer]);
    assert!(
        selected::tool_results(
            &fixture.options,
            &source,
            "native",
            &call,
            first.next.as_ref()
        )
        .unwrap()
        .unwrap()
        .is_empty()
    );
    let db = Connection::open(&fixture.db).unwrap();
    let (record_occurrence, call_occurrence): (i64, i64) = db
        .query_row(
            "SELECT r.occurrence,c.occurrence FROM fact_tool_call c
         JOIN fact_record r ON r.source=c.source AND r.pos=c.pos AND r.block=c.block
         WHERE c.call_id='tool-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(call.occurrence, record_occurrence);
    assert_ne!(record_occurrence, call_occurrence);
    let continued = selected_page(&fixture, &[&path, &peer], "native", 10, first.next.as_ref())
        .unwrap()
        .unwrap();
    let previously_committed = continued
        .records
        .iter()
        .find(|record| record.address == call)
        .unwrap();
    assert_eq!(previously_committed.causal.calls.len(), 1);
    assert_eq!(previously_committed.causal.calls[0].id, "tool-1");
    assert_eq!(previously_committed.causal.calls[0].tool, "SendMessage");
    assert_eq!(previously_committed.causal.calls[0].block, 0);
    assert!(previously_committed.causal.calls[0].replies.is_empty());
    let fresh = selected::record_at(&fixture.options, &source, "native", &call, None)
        .unwrap()
        .unwrap();
    let resolved_call = selected::evidence_at(
        &fixture.options,
        &source,
        "native",
        selected::Evidence::Call("tool-1"),
        first.next.as_ref(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(resolved_call.address, call);
    assert!(resolved_call.causal.calls[0].replies.is_empty());
    assert!(
        selected::evidence_at(
            &fixture.options,
            &source,
            "native",
            selected::Evidence::Event("result-event"),
            first.next.as_ref()
        )
        .unwrap()
        .is_none()
    );
    let resolved_result = selected::evidence_at(
        &fixture.options,
        &source,
        "native",
        selected::Evidence::Event("result-event"),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        resolved_result.native["message"]["content"][0]["content"],
        "qualified result"
    );
    assert_eq!(fresh.causal.calls[0].replies.len(), 1);
    let latest = selected::latest_page(&fixture.options, &source, "native", 10, None)
        .unwrap()
        .unwrap();
    let rendered = selected::render::entries(
        &latest,
        fixture.namespace.as_str(),
        "native",
        crate::model::now_ms(),
    );
    let tool = rendered
        .iter()
        .find(|entry| entry["name"] == "SendMessage")
        .unwrap();
    assert_eq!(tool["address"]["record"], json!(call));
    assert_eq!(tool["ok"], true);
    assert!(tool.to_string().contains("qualified result"));

    assert_eq!(fresh.causal.turn, previously_committed.causal.turn);
    assert!(fresh.causal.turn.is_some());
    let latest = selected::tool_results(&fixture.options, &source, "native", &call, None)
        .unwrap()
        .unwrap();
    assert_eq!(latest.len(), 1);
    let receipts = &latest[0].relay_receipts;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].path, peer.to_str().unwrap());
    assert_eq!(receipts[0].offset, receipt_offset);
    assert_eq!(receipts[0].block, 0);
    assert!(receipts[0].at.is_some());
    assert_eq!(
        latest[0].native["message"]["content"][0]["content"],
        "qualified result"
    );
}

#[test]
fn warehouse_relay_turn_carries_across_pages_at_the_selected_prefix() {
    let mut fixture = Fixture::new("relay-turn-placement");
    let path = fixture.claude("receiver", "First prompt");
    let receipt = fs::metadata(&path).unwrap().len();
    append_native(
        &path,
        json!({"type":"user","uuid":"receipt","sessionId":"receiver",
        "timestamp":"2026-09-24T00:00:02Z","origin":{"kind":"peer","from":"uds:/fixture.sock",
        "name":"peer","msg_id":"receipt-id","body":"Relay body"},
        "message":{"role":"user","content":"<cross-session-message from=\"uds:/fixture.sock\">Relay body</cross-session-message>"}}),
    );
    append_native(
        &path,
        json!({"type":"assistant","uuid":"reply","sessionId":"receiver",
        "timestamp":"2026-09-24T00:00:04Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Reply to the relay"}
        ]}}),
    );
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let oracle = semantic_transcript(
        &fixture.options,
        fixture.namespace.as_str(),
        "sid=receiver&after=0&limit=100",
        None,
        crate::SessionSourceReadMode::LocalOrPinnedProvider,
    );
    assert_eq!(oracle.status, 200);
    let transcript: serde_json::Value = serde_json::from_slice(&oracle.body).unwrap();
    let entries = transcript["entries"].as_array().unwrap();
    let reply = entries
        .iter()
        .find(|entry| entry["text"] == "Reply to the relay")
        .unwrap();
    let first = selected_page(&fixture, &[&path], "receiver", 3, None)
        .unwrap()
        .unwrap();
    let relay = first
        .records
        .iter()
        .find(|record| record.address.offset == receipt)
        .unwrap();
    assert_eq!(relay.prompt_anchor.as_deref(), reply["turn"].as_str());
    append_native(&path, prompt("receiver", 3, "Later commit"));
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let page = selected_page(&fixture, &[&path], "receiver", 1, first.next.as_ref())
        .unwrap()
        .unwrap();
    let turn = page.records[0].causal.turn.as_ref().unwrap();
    assert_eq!(turn.address, relay.address);
    assert_eq!(Some(turn.id.as_str()), reply["turn"].as_str());
}

struct RelayFixtureProvider(BTreeMap<String, Vec<u8>>);
impl crate::SessionSourceReader for RelayFixtureProvider {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> std::io::Result<crate::SessionSourceRange> {
        use std::io::ErrorKind::PermissionDenied;
        let bytes = self.0.get(&source.path).ok_or(PermissionDenied)?;
        let prefix = bytes
            .get(..source.offset as usize)
            .ok_or(PermissionDenied)?;
        let generation = format!("fixture-{}-{}", source.native_id, source.offset);
        let expected_root = if source.path.starts_with("projects/") {
            "claude"
        } else {
            "codex"
        };
        if source.root != expected_root
            || <[u8; 32]>::from(Sha256::digest(prefix)) != source.prefix_sha256
            || expected.is_some_and(|expected| expected != generation)
        {
            return Err(PermissionDenied.into());
        }
        let start = offset as usize;
        let part = prefix
            .get(start..start + max.min(32))
            .ok_or(PermissionDenied)?;
        Ok(crate::SessionSourceRange {
            generation,
            length: source.offset,
            offset,
            bytes: part.into(),
            cached: false,
        })
    }
}

#[test]
fn warehouse_parent_relay_matches_body_then_time_at_the_selected_prefix() {
    let mut fixture = Fixture::new("parent-relay-fallback");
    let root = fixture.claude("root", "Delegate");
    append_native(
        &root,
        json!({"type":"assistant","sessionId":"root","timestamp":"2026-09-24T00:00:01Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"agent","name":"Agent","input":{"prompt":"Work"}}]}}),
    );
    let child = root.with_extension("").join("subagents/agent-worker.jsonl");
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::write(
        child.with_extension("meta.json"),
        json!({"agentType":"general-purpose","toolUseId":"agent"}).to_string(),
    )
    .unwrap();
    fs::write(&child, format!("{}\n", prompt("root", 2, "Work"))).unwrap();
    for (id, offset, text) in [("body", 3, "same body"), ("time", 6, "sender version")] {
        append_native(
            &child,
            json!({"type":"assistant","sessionId":"root","timestamp":format!("2026-09-24T00:00:{offset:02}Z"),
            "message":{"role":"assistant","content":[{"type":"tool_use","id":id,"name":"SendMessage","input":{"to":"main","message":text}}]}}),
        );
        append_native(
            &child,
            json!({"type":"user","sessionId":"root","timestamp":format!("2026-09-24T00:00:{:02}Z",offset+2),
            "toolUseResult":{"success":true},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"queued"}]}}),
        );
    }
    scan_file(&root, "claude", &mut fixture.cache, &mut false).unwrap();
    scan_file(&child, "claude", &mut fixture.cache, &mut false).unwrap();
    let paths = [root.as_path(), child.as_path()];
    let first = selected_page(&fixture, &paths, "worker", 1, None)
        .unwrap()
        .unwrap();
    let mut offsets = Vec::new();
    for (time, body) in [(4, "same body"), (7, "receiver version")] {
        offsets.push(fs::metadata(&root).unwrap().len());
        append_native(
            &root,
            json!({"type":"user","sessionId":"root","timestamp":format!("2026-09-24T00:00:{time:02}Z"),
            "origin":{"kind":"peer","senderTaskId":"worker","from":"worker","body":body},
            "message":{"role":"user","content":format!("<agent-message from=\"worker\">{body}</agent-message>")}}),
        );
    }
    scan_file(&root, "claude", &mut fixture.cache, &mut false).unwrap();
    let source = fixture.source(&paths);
    let mut call_addresses = Vec::new();
    for (id, offset) in [("body", offsets[0]), ("time", offsets[1])] {
        let call = selected::evidence_at(
            &fixture.options,
            &source,
            "worker",
            selected::Evidence::Call(id),
            None,
        )
        .unwrap()
        .unwrap();
        let read = |cursor| {
            selected::tool_results(&fixture.options, &source, "worker", &call.address, cursor)
                .unwrap()
                .unwrap()
        };
        assert!(read(first.next.as_ref())[0].relay_receipts.is_empty());
        let current = read(None);
        assert_eq!(current[0].relay_receipts.len(), 1);
        assert_eq!(current[0].relay_receipts[0].offset, offset);
        call_addresses.push(call.address);
    }

    let provider = RelayFixtureProvider(BTreeMap::from([
        ("projects/proj/root.jsonl".into(), fs::read(&root).unwrap()),
        (
            "projects/proj/root/subagents/agent-worker.jsonl".into(),
            fs::read(&child).unwrap(),
        ),
    ]));
    for (path, hidden) in [
        (&root, root.with_extension("fixture-original")),
        (&child, child.with_extension("fixture-original")),
    ] {
        fs::rename(path, hidden).unwrap();
    }
    let mut hosted = fixture.source(&paths);
    hosted.current_provider = Some(&provider);
    for (address, offset) in call_addresses.iter().zip(offsets) {
        let selected = selected::tool_results(&fixture.options, &hosted, "worker", address, None)
            .unwrap()
            .unwrap();
        assert_eq!(selected[0].relay_receipts.len(), 1);
        assert_eq!(selected[0].relay_receipts[0].offset, offset);
    }
}

#[test]
fn warehouse_trace_follows_a_send_from_a_continuation_and_a_peer_reply() {
    let mut fixture = Fixture::new("trace-cross-member-relay");
    let original = fixture.claude("original", "Begin work");
    let resumed = original.with_file_name("resumed.jsonl");
    let mut copied: serde_json::Value = serde_json::from_str(
        fs::read_to_string(&original)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    copied["sessionId"] = json!("resumed");
    fs::write(&resumed, format!("{copied}\n")).unwrap();
    append_native(
        &original,
        json!({"type":"continued-in","sessionId":"original",
        "continuedInSessionId":"resumed","timestamp":"2026-09-24T00:00:01Z"}),
    );
    let peer = fixture.claude("peer", "Peer prompt");
    let last = fixture.claude("last", "Last prompt");
    let append_relay = |sender: &Path, native, receiver: &Path, target, correlation, at: usize| {
        append_native(
            sender,
            json!({"type":"assistant","uuid":correlation,"sessionId":native,
            "timestamp":format!("2026-09-24T00:00:{at:02}Z"),"message":{"role":"assistant","content":[
                {"type":"tool_use","id":correlation,"name":"SendMessage","input":{"to":target,"message":"hello"}}
            ]}}),
        );
        append_native(
            sender,
            json!({"type":"user","sessionId":native,
            "timestamp":format!("2026-09-24T00:00:{:02}Z",at+1),"toolUseResult":{"success":true,"msg_id":correlation},
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":correlation,"content":"sent"}]}}),
        );
        append_native(
            receiver,
            json!({"type":"user","sessionId":target,
            "timestamp":format!("2026-09-24T00:00:{:02}Z",at+2),"origin":{"kind":"peer",
                "from":"uds:/run/user/1000/cc-socks/111.sock","name":native,"fromMode":"prompting",
                "msg_id":correlation,"body":"hello","verifiedPeerPid":111,"verifiedPeerProcStart":"999"},
            "message":{"role":"user","content":"<cross-session-message from=\"uds:/run/user/1000/cc-socks/111.sock\">hello</cross-session-message>"}}),
        );
    };
    append_relay(&resumed, "resumed", &peer, "peer", "delivery-one", 2);
    append_relay(&peer, "peer", &last, "last", "delivery-two", 5);
    append_native(
        &last,
        json!({"type":"assistant","uuid":"last-reply","sessionId":"last",
        "timestamp":"2026-09-24T00:00:08Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Finished"}
        ]}}),
    );
    let paths = [
        original.as_path(),
        resumed.as_path(),
        peer.as_path(),
        last.as_path(),
    ];
    for path in paths {
        scan_file(path, "claude", &mut fixture.cache, &mut false).unwrap();
    }
    let component = selected_page(&fixture, &paths, "original", 20, None)
        .unwrap()
        .unwrap();
    let record = component
        .records
        .iter()
        .find(|record| record.text().as_deref() == Some("Begin work"))
        .unwrap();
    let turn = &record.causal.turn.as_ref().unwrap().id;
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let query = crate::native_html::query(&[("sid", "original"), ("turn", turn)]);
    let reply = http::trace(&configured, &query, None, None);
    let trace: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{trace}");
    assert!(
        trace["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["from"] == "original"
                && edge["to"] == "peer"
                && edge["status"] == "done"),
        "{trace}"
    );
    assert!(
        trace["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["from"] == "peer" && edge["to"] == "last" && edge["status"] == "done"),
        "{trace}"
    );
    let turns = trace["turns"].as_array().unwrap();
    assert_eq!(turns[0]["last"], true, "{trace}");
    let receiving = turns.iter().find(|turn| turn["sid"] == "last").unwrap();
    assert_eq!(receiving["last"], true, "{trace}");
    assert_eq!(receiving["end"]["why"], "replied", "{trace}");
}

#[test]
fn warehouse_trace_codex_end_belongs_to_selected_turn() {
    let mut fixture = Fixture::new("trace-codex-end");
    let path = fixture.codex("trace-codex");
    for (second, kind, id) in [
        (1, "task_started", "first"),
        (3, "task_complete", "first"),
        (4, "task_started", "second"),
        (6, "turn_aborted", "second"),
    ] {
        append_native(
            &path,
            json!({"type":"event_msg","timestamp":format!("2026-09-24T00:00:{second:02}Z"),
            "payload":{"type":kind,"turn_id":id,"reason":"interrupted"}}),
        );
        if kind == "task_started" {
            append_native(
                &path,
                json!({"type":"response_item","timestamp":format!("2026-09-24T00:00:{:02}Z", second + 1),
                "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":id}]}}),
            );
        }
    }
    scan_file(&path, "codex", &mut fixture.cache, &mut false).unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let component = selected_page(&fixture, &[&path], "trace-codex", 20, None)
        .unwrap()
        .unwrap();
    for (prompt, expected, last) in [
        ("first", "completed", false),
        ("second", "interrupted", true),
    ] {
        let record = component
            .records
            .iter()
            .find(|record| record.text().as_deref() == Some(prompt))
            .unwrap();
        let turn = &record.causal.turn.as_ref().unwrap().id;
        let query = crate::native_html::query(&[("sid", "trace-codex"), ("turn", turn)]);
        let response = http::trace(&configured, &query, None, None);
        let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(response.status, 200, "{body}");
        assert_eq!(body["turns"][0]["end"]["why"], expected, "{body}");
        assert_eq!(body["turns"][0]["last"], last);
    }
}

#[test]
fn warehouse_trace_copilot_pairs_the_terminal_and_prioritizes_abort() {
    let mut fixture = Fixture::new("trace-copilot-end");
    let key = "01234567-89ab-4cde-8123-456789abcdef";
    let path = fixture.copilot(key);
    let event = |second: u32, kind, data| {
        append_native(
            &path,
            json!({"type":kind,"id":format!("event-{second}"),
                "timestamp":format!("2026-09-24T00:00:{second:02}Z"),"data":data}),
        );
    };
    // Copilot 1.0.90 cancellation records omit turnId on abort. The surrounding
    // native start and end records must establish its owner.
    event(
        1,
        "user.message",
        json!({"content":"First","messageId":"prompt-1","turnId":"0","interactionId":"one"}),
    );
    event(
        2,
        "assistant.turn_start",
        json!({"turnId":"0","interactionId":"one"}),
    );
    event(
        3,
        "assistant.message",
        json!({"content":"Partial","turnId":"0","originatingMessageId":"prompt-1"}),
    );
    event(4, "abort", json!({"reason":"user"}));
    event(5, "assistant.turn_end", json!({"turnId":"0"}));
    event(
        6,
        "user.message",
        json!({"content":"Second","messageId":"prompt-2","turnId":"1","interactionId":"two"}),
    );
    event(
        7,
        "assistant.turn_start",
        json!({"turnId":"1","interactionId":"two"}),
    );
    event(
        8,
        "assistant.message",
        json!({"content":"Second answer","turnId":"1","originatingMessageId":"prompt-2"}),
    );
    event(9, "assistant.turn_end", json!({"turnId":"0"}));
    scan_file(&path, "copilot", &mut fixture.cache, &mut false).unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let component = selected_page(&fixture, &[&path], key, 20, None)
        .unwrap()
        .unwrap();
    let read = |prompt| {
        let record = component
            .records
            .iter()
            .find(|record| record.text().as_deref() == Some(prompt))
            .unwrap();
        let turn = &record.causal.turn.as_ref().unwrap().id;
        let query = crate::native_html::query(&[("sid", key), ("turn", turn)]);
        let response = http::trace(&configured, &query, None, None);
        let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(response.status, 200, "{body}");
        body
    };
    let interrupted = read("First");
    assert_eq!(interrupted["turns"][0]["end"]["why"], "interrupted");
    assert_eq!(interrupted["incomplete"], false);
    let unpaired = read("Second");
    assert_eq!(unpaired["turns"][0]["end"]["why"], "unavailable");
    assert_eq!(unpaired["incomplete"], true);
    event(10, "assistant.turn_end", json!({"turnId":"1"}));
    scan_file(&path, "copilot", &mut fixture.cache, &mut false).unwrap();
    let replied = read("Second");
    assert_eq!(replied["turns"][0]["end"]["why"], "replied");
    assert_eq!(replied["incomplete"], false);
}

#[test]
fn warehouse_trace_uses_the_latest_completed_claude_run() {
    let mut fixture = Fixture::new("claude-trace-runs");
    let root = fixture.claude("root", "Delegate");
    append_native(
        &root,
        json!({"type":"assistant","sessionId":"root","timestamp":"2026-09-24T00:00:01Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"agent","name":"Agent","input":{"prompt":"Work"}}]}}),
    );
    let child = root.with_extension("").join("subagents/agent-worker.jsonl");
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::write(
        child.with_extension("meta.json"),
        json!({"agentType":"general-purpose","toolUseId":"agent"}).to_string(),
    )
    .unwrap();
    fs::write(&child, format!("{}\n", prompt("root", 2, "Work"))).unwrap();
    append_native(
        &child,
        json!({"type":"assistant","sessionId":"root","timestamp":"2026-09-24T00:00:03Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"handback","name":"SubagentHandback","input":{"message":"First return"}}]}}),
    );
    append_native(&child, prompt("root", 4, "Resume"));
    for (at, status, summary) in [
        (5, "completed", "Late first notification"),
        (6, "failed", "Second run failed"),
    ] {
        append_native(
            &root,
            prompt(
                "root",
                at,
                &format!(
                    "<task-notification><tool-use-id>agent</tool-use-id><status>{status}</status><summary>{summary}</summary></task-notification>"
                ),
            ),
        );
    }
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let launch = crate::model::spawn_turn_id("claude", "worker");
    let query = crate::native_html::query(&[("sid", "worker"), ("turn", &launch)]);
    let reply = http::trace(&configured, &query, None, None);
    let trace: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{trace}");
    assert_eq!(trace["handoffs"][0]["status"], "err", "{trace}");
    assert_eq!(trace["handoffs"][0]["result"], "Second run failed");
    assert_eq!(trace["turns"][0]["end"]["why"], "handoff_failed");
    assert!(
        trace["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["role"] == "return"
                && row["catalog_key"] == "root"
                && row["reason"].is_string())
    );
}

#[test]
fn warehouse_legacy_receipts_resolve_selected_turn_and_completion_only() {
    let mut fixture = Fixture::new("legacy-relay-detail");
    let root = fixture.claude("root", "Delegate");
    append_native(
        &root,
        json!({"type":"assistant","sessionId":"root","timestamp":"2026-09-24T00:00:01Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"agent","name":"Agent","input":{"prompt":"Work"}}]}}),
    );
    let child = root.with_extension("").join("subagents/agent-worker.jsonl");
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::write(
        child.with_extension("meta.json"),
        json!({"agentType":"general-purpose","toolUseId":"agent"}).to_string(),
    )
    .unwrap();
    fs::write(&child, format!("{}\n", prompt("root", 2, "Work"))).unwrap();
    let send_offset = fs::metadata(&child).unwrap().len();
    append_native(
        &child,
        json!({"type":"assistant","sessionId":"root","timestamp":"2026-09-24T00:00:03Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"progress","name":"SendMessage",
        "input":{"to":"main","message":"Still working"}}]}}),
    );
    append_native(
        &child,
        json!({"type":"user","sessionId":"root","timestamp":"2026-09-24T00:00:05Z",
        "toolUseResult":{"success":true},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"progress","content":"queued"}]}}),
    );
    let old_call_offset = fs::metadata(&root).unwrap().len();
    append_native(
        &root,
        json!({"type":"assistant","sessionId":"root","timestamp":"2026-09-24T00:00:02Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"before-relay","name":"Read","input":{"file_path":"/work/before"}}]}}),
    );
    let mut offsets = Vec::new();
    for (time, body) in [(4, "Still working"), (7, "Work finished")] {
        offsets.push(fs::metadata(&root).unwrap().len());
        append_native(
            &root,
            json!({"type":"user","uuid":format!("receipt-{time}"),"sessionId":"root",
            "timestamp":format!("2026-09-24T00:00:{time:02}Z"),"message":{"role":"user",
            "content":format!("<agent-message from=\"worker\">{body}</agent-message>")}}),
        );
    }
    append_native(
        &root,
        json!({"type":"assistant","uuid":"reply","sessionId":"root","timestamp":"2026-09-24T00:00:08Z",
        "message":{"role":"assistant","content":[{"type":"text","text":"Thanks for the update"}]}}),
    );
    let proc_stat = fixture.options.proc_root.join("42/stat");
    fs::create_dir_all(proc_stat.parent().unwrap()).unwrap();
    let mut fields = ["0"; 20];
    fields[19] = "777";
    fs::write(proc_stat, format!("42 (claude) {}\n", fields.join(" "))).unwrap();
    let pid_home = fixture.options.claude_home.join("sessions");
    fs::create_dir_all(&pid_home).unwrap();
    fs::write(
        pid_home.join("42.json"),
        json!({"pid":42,"sessionId":"root","procStart":777,"status":"idle"}).to_string(),
    )
    .unwrap();
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let paths = [root.as_path(), child.as_path()];
    let detail = selected_page(&fixture, &paths, "root", 100, None)
        .unwrap()
        .unwrap();
    let reply = detail
        .records
        .iter()
        .find(|record| record.text().as_deref() == Some("Thanks for the update"))
        .unwrap();
    let turn = reply.causal.turn.as_ref().unwrap();
    assert_eq!(turn.address.offset, offsets[0]);
    let oracle = semantic_transcript(
        &fixture.options,
        fixture.namespace.as_str(),
        "sid=root&after=0&limit=100",
        None,
        crate::SessionSourceReadMode::LocalOrPinnedProvider,
    );
    assert_eq!(oracle.status, 200);
    let oracle: serde_json::Value = serde_json::from_slice(&oracle.body).unwrap();
    let reply = oracle["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Thanks for the update")
        .unwrap();
    assert_eq!(reply["turn"].as_str(), Some(turn.id.as_str()));
    let detail = selected_page(&fixture, &paths, "worker", 100, None)
        .unwrap()
        .unwrap();
    assert_eq!(detail.legacy_completion.unwrap().offset, offsets[1]);
    let db = Connection::open(&fixture.db).unwrap();
    let state: String = db.query_row("SELECT json_extract(metadata,'$.state') FROM session_trace_headers WHERE session_key='worker'",
        [], |row| row.get(0)).unwrap();
    assert_eq!(state, "done");

    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let launch = crate::model::spawn_turn_id("claude", "worker");
    let query = crate::native_html::query(&[("sid", "worker"), ("turn", &launch)]);
    let reply = http::trace(&configured, &query, None, None);
    let trace: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{trace}");
    assert_eq!(trace["handoffs"][0]["from"], "root");
    assert_eq!(trace["handoffs"][0]["result"], "Work finished");
    assert!(
        trace["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["role"] == "return" && row["catalog_key"] == "root")
    );
    let address = &trace["evidence"][1]["address"];
    assert_eq!(address["record"]["path"], "projects/proj/root.jsonl");
    let query = crate::native_html::query(&[("sid", "root"), ("address", &address.to_string())]);
    let reply = http::transcript(&configured, &query, None, None);
    let transcript: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{transcript}");
    assert!(
        transcript["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["address"] == *address && entry["name"] == "Agent")
    );

    // The first receipt starts a turn because its body matches the child's send.
    // Count the unanswered call before that receipt, but leave the live call open.
    append_native(
        &root,
        json!({"type":"assistant","sessionId":"root","timestamp":"2026-09-24T00:00:09Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"after-relay","name":"Glob","input":{"pattern":"*.rs"}}]}}),
    );
    fs::write(
        pid_home.join("42.json"),
        json!({"pid":42,"sessionId":"root","procStart":777,"status":"busy"}).to_string(),
    )
    .unwrap();
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let (row, _) = focused_metadata(&fixture.source(&paths), "root")
        .unwrap()
        .unwrap();
    assert_eq!(row.status, Some("work"));
    assert_eq!(row.details.unwrap()["errors"], 1);
    let reply = http::annotations(&configured, "sid=root&kind=errors", None, None);
    let errors: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{errors}");
    assert_eq!(errors["total"], 1, "{errors}");
    assert_eq!(
        errors["entries"][0]["address"]["record"]["offset"],
        old_call_offset
    );
    let identity = http::identity(&configured, "sid=root");
    assert_eq!(identity.status, 200);
    let identity: serde_json::Value = serde_json::from_slice(&identity.body).unwrap();
    let generation = identity["identity"]["generation"].as_str().unwrap();
    let query = crate::native_html::query(&[
        ("sid", "root"),
        ("scope", "current"),
        ("generation", generation),
        ("session_fields", "[\"detail\"]"),
    ]);
    let home = http::home(&configured, &query, None, None);
    let detail: serde_json::Value = serde_json::from_slice(&home.body).unwrap();
    assert_eq!(home.status, 200, "{detail}");
    assert_eq!(detail["session"]["detail"], "Still working");
    assert_eq!(detail["session"]["detail_from"], "worker");

    // Removing the selected proof blocks prevents native result reads, but
    // the list still derives its child status from process and metadata only.
    for path in [&root, &child] {
        let proof = format!(
            "native_source_chunks:{:x}:header",
            Sha256::digest(path.to_str().unwrap().as_bytes())
        );
        assert_eq!(
            db.execute("DELETE FROM meta WHERE key=?1", [proof])
                .unwrap(),
            1
        );
    }
    assert!(matches!(
        selected_page(&fixture, &paths, "worker", 100, None),
        Err(ReadError::Restart)
    ));
    let list = read_list(
        &[fixture.source(&paths)],
        "workspace",
        &request("limit=10"),
        None,
    )
    .unwrap();
    assert_eq!(
        list.rows
            .iter()
            .find(|row| row.position.key == "worker")
            .unwrap()
            .status,
        Some("work")
    );

    let child_relative = "projects/proj/root/subagents/agent-worker.jsonl";
    let provider = RelayFixtureProvider(BTreeMap::from([
        ("projects/proj/root.jsonl".into(), fs::read(&root).unwrap()),
        (child_relative.into(), fs::read(&child).unwrap()),
    ]));
    fs::rename(&child, child.with_extension("fixture-original")).unwrap();
    fs::write(
        &child,
        format!("{}\n", prompt("root", 10, "Replacement work")),
    )
    .unwrap();
    scan_file(&child, "claude", &mut fixture.cache, &mut false).unwrap();
    let identity = warehouse::retained_identity(&db, &fixture.namespace, "worker").unwrap();
    let source = fixture.source(&paths);
    let retained = selected::retained_turn(&source, "worker", &launch, &provider).unwrap();
    let address: crate::session_route::NativeEntryAddress = serde_json::from_value(
        selected::render::turn_address(&retained, source.namespace.as_str(), "worker", &launch)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(address.record.offset, 0);
    let opened = selected::retained_addressed_window(
        &source,
        "worker",
        &address.record,
        &address.binding,
        &provider,
    )
    .unwrap();
    assert_eq!(
        selected::render::turn_address(&opened, source.namespace.as_str(), "worker", &launch),
        Some(json!(address)),
    );
    let sent = selected::Address {
        path: child_relative.into(),
        offset: send_offset,
        block: 0,
        occurrence: 0,
    };
    let relay = selected::retained_peer_relay_address(
        &source,
        "worker",
        &identity.witness,
        "root",
        (&sent, 0),
        Some(&provider),
    )
    .unwrap();
    assert_eq!(relay.record.offset, offsets[0]);
    let query = crate::native_html::query(&[
        ("sid", "worker"),
        ("scope", "retained_history"),
        ("turn", &launch),
    ]);
    let reply = http::trace(&configured, &query, None, Some(&provider));
    let trace: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{trace}");
    assert_eq!(trace["handoffs"][0]["status"], "done");
    assert_eq!(trace["handoffs"][0]["result"], "Work finished");
    assert_eq!(trace["turns"][0]["end"]["why"], "returned");
    let relay_turn = crate::model::legacy_relay_turn_id("progress");
    assert_eq!(trace["turns"][0]["sent"], json!([relay_turn]));
    let evidence = trace["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["turn"] == relay_turn)
        .unwrap();
    assert_eq!(evidence["address"], json!(relay));
    assert_eq!(evidence["peer"]["witness"], identity.witness);
    let query = crate::native_html::query(&[
        ("sid", "root"),
        ("scope", "retained_history"),
        ("address", &evidence["address"].to_string()),
        ("peer", &evidence["peer"].to_string()),
    ]);
    let reply = http::transcript(&configured, &query, None, Some(&provider));
    let transcript: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{transcript}");
    assert!(
        transcript["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["address"] == evidence["address"] && entry["turn"] == relay_turn)
    );
    let preview = |generation: &str, address: &serde_json::Value| {
        let query = crate::native_html::query(&[
            ("sid", "root"),
            ("scope", "retained_history"),
            ("generation", generation),
            ("session_fields", "[]"),
            ("events", &json!([relay_turn]).to_string()),
            ("address", &address.to_string()),
            ("peer", &evidence["peer"].to_string()),
        ]);
        http::home(&configured, &query, None, Some(&provider))
    };
    let generation = trace["previews"]["root"].as_str().unwrap();
    let response = preview(generation, &evidence["address"]);
    let home: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(response.status, 200, "{home}");
    assert_eq!(
        home["events"][relay_turn.as_str()]["brief"],
        "Still working"
    );
    assert_eq!(home["incomplete"], false);
    let mut wrong_entry = evidence["address"].clone();
    wrong_entry["block"] = json!(1);
    assert_eq!(preview(generation, &wrong_entry).status, 409);
    assert_eq!(preview(&"0".repeat(64), &evidence["address"]).status, 409);
    let opened = selected::retained_peer_window(
        &source,
        "root",
        selected::PeerBinding {
            trace: "worker",
            witness: &identity.witness,
        },
        16,
        None,
        Some((&relay.record, &relay.binding)),
        Some(&provider),
    )
    .unwrap();
    let rendered = selected::render::entries(&opened, source.namespace.as_str(), "root", 0);
    assert!(
        rendered
            .iter()
            .any(|entry| entry["entry_id"] == relay.entry_id)
    );
    assert!(matches!(
        selected::retained_peer_relay_address(
            &source,
            "worker",
            &identity.witness,
            "root",
            (&sent, 1),
            Some(&provider),
        ),
        Err(ReadError::PeerUnavailable(_))
    ));
    fs::rename(&root, root.with_extension("fixture-original")).unwrap();
    fixture.claude("root", "Replacement parent");
    let identity = http::identity(&configured, "sid=root&scope=retained_history");
    let metadata: serde_json::Value = serde_json::from_slice(&identity.body).unwrap();
    assert_eq!(identity.status, 200, "{metadata}");
    let generation = metadata["identity"]["generation"].as_str().unwrap();
    let query = crate::native_html::query(&[
        ("sid", "root"),
        ("scope", "retained_history"),
        ("generation", generation),
        ("session_fields", "[\"detail\"]"),
    ]);
    let reply = http::home(&configured, &query, None, Some(&provider));
    let detail: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{detail}");
    assert_eq!(detail["session"]["detail"], "Still working");
    assert_eq!(detail["session"]["detail_from"], "worker");
}

#[test]
fn warehouse_current_provider_pins_the_selected_original_across_pages() {
    use crate::{SessionSourceRange, SessionSourceReader, SessionSourceRef};
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Provider {
        data: Vec<u8>,
        generation: AtomicUsize,
        change_during_body: AtomicBool,
        calls: AtomicUsize,
    }

    impl SessionSourceReader for Provider {
        fn read_range(
            &self,
            source: &SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<SessionSourceRange> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if source.root != "claude"
                || source.path != "projects/proj/native.jsonl"
                || source.native_id != "native"
                || source.offset != self.data.len() as u64
                || source.prefix_sha256 != <[u8; 32]>::from(Sha256::digest(&self.data))
                || offset + max as u64 > source.offset
            {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let version = self.generation.load(Ordering::Relaxed);
            let generation = format!("opaque-generation-{version}");
            if expected.is_some_and(|expected| expected != generation) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let generation = if max > 1 && self.change_during_body.load(Ordering::Relaxed) {
                "changed-during-body".into()
            } else {
                generation
            };
            let start = offset as usize;
            let end = start + max.min(32);
            Ok(SessionSourceRange {
                generation,
                length: source.offset,
                offset,
                bytes: self.data[start..end].into(),
                cached: false,
            })
        }
    }

    let mut fixture = Fixture::new("current-provider");
    let path = fixture.claude("native", "Original prompt");
    append_native(&path, prompt("native", 1, "Second prompt"));
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let provider = Provider {
        data: fs::read(&path).unwrap(),
        generation: AtomicUsize::new(1),
        change_during_body: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    };
    let page = |source: &Source<'_>, limit, cursor| {
        selected::page(
            &fixture.options,
            source,
            CatalogReadScope::Current,
            "native",
            limit,
            cursor,
        )
    };
    let mut local = fixture.source(&[&path]);
    local.current_provider = Some(&provider);
    let first_local = page(&local, 1, None).unwrap().unwrap();
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);

    let original = path.with_extension("fixture-original");
    fs::rename(&path, &original).unwrap();
    let mut hosted = fixture.source(&[&path]);
    assert!(page(&hosted, 1, None).unwrap().is_none());
    hosted.current_provider = Some(&provider);
    assert!(matches!(
        page(&hosted, 1, first_local.next.as_ref()),
        Err(ReadError::Restart)
    ));
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);

    let first = page(&hosted, 1, None).unwrap().unwrap();
    let serialized = serde_json::to_string(&first.next).unwrap();
    assert!(!serialized.contains("opaque-generation"));
    let cursor: selected::Cursor = serde_json::from_str(&serialized).unwrap();
    let continued = page(&hosted, 10, Some(&cursor)).unwrap().unwrap();
    let answer = continued
        .records
        .iter()
        .find(|record| record.text().as_deref() == Some("Second prompt"))
        .unwrap();
    let exact = selected::record_at(
        &fixture.options,
        &hosted,
        "native",
        &answer.address,
        Some(&cursor),
    )
    .unwrap()
    .unwrap();
    assert_eq!(exact.text().as_deref(), Some("Second prompt"));

    provider.generation.store(2, Ordering::Relaxed);
    assert!(matches!(
        page(&hosted, 10, Some(&cursor)),
        Err(ReadError::Restart)
    ));
    provider.generation.store(1, Ordering::Relaxed);
    provider.change_during_body.store(true, Ordering::Relaxed);
    assert!(matches!(page(&hosted, 10, None), Err(ReadError::Restart)));
    provider.change_during_body.store(false, Ordering::Relaxed);

    let mut denied = fixture.source(&[&path]);
    denied.current_provider = Some(&provider);
    denied
        .inventory
        .insert(path.to_str().unwrap().to_owned(), Availability::Unavailable);
    let calls = provider.calls.load(Ordering::Relaxed);
    assert!(page(&denied, 1, None).unwrap().is_none());
    assert_eq!(provider.calls.load(Ordering::Relaxed), calls);

    let restored = path.clone();
    selected::AFTER_BODY.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || fs::rename(original, restored).unwrap()));
    });
    assert!(matches!(page(&hosted, 10, None), Err(ReadError::Restart)));
}

#[test]
fn warehouse_retained_trace_opens_only_the_observed_peer_prefix() {
    use crate::{SessionSourceRange, SessionSourceReader, SessionSourceRef};
    use std::{
        io,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct PeerProvider {
        bytes: Vec<u8>,
        calls: AtomicUsize,
    }
    impl SessionSourceReader for PeerProvider {
        fn read_range(
            &self,
            source: &SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<SessionSourceRange> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if source.root != "claude"
                || source.path != "projects/proj/peer.jsonl"
                || source.native_id != "peer"
                || source.offset != self.bytes.len() as u64
                || source.prefix_sha256 != <[u8; 32]>::from(Sha256::digest(&self.bytes))
                || expected.is_some_and(|generation| generation != "fixture-peer")
            {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let start = usize::try_from(offset).map_err(io::Error::other)?;
            let end = (start + max).min(self.bytes.len());
            Ok(SessionSourceRange {
                generation: "fixture-peer".into(),
                length: source.offset,
                offset,
                bytes: self.bytes[start..end].into(),
                cached: false,
            })
        }
    }

    let mut fixture = Fixture::new("retained-trace-peer");
    let root = fixture.claude("root", "Original trace");
    let peer = fixture.claude("peer", "Original peer");
    let sent = selected::Address {
        path: "projects/proj/root.jsonl".into(),
        offset: fs::metadata(&root).unwrap().len(),
        block: 0,
        occurrence: 0,
    };
    let receipt_offset = fs::metadata(&peer).unwrap().len();
    append_native(
        &root,
        json!({"type":"assistant","uuid":"call","sessionId":"root",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"send","name":"SendMessage","input":{"to":"peer","message":"hello"}}
        ]}}),
    );
    append_native(
        &root,
        json!({"type":"user","sessionId":"root",
        "timestamp":"2026-09-24T00:00:02Z","toolUseResult":{"success":true,"msg_id":"delivery"},
        "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"send","content":"sent"}]}}),
    );
    append_native(
        &peer,
        json!({"type":"user","sessionId":"peer","timestamp":"2026-09-24T00:00:03Z",
        "origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/111.sock","name":"root",
            "fromMode":"prompting","msg_id":"delivery","body":"hello","verifiedPeerPid":111,"verifiedPeerProcStart":"999"},
        "message":{"role":"user","content":"<cross-session-message from=\"uds:/run/user/1000/cc-socks/111.sock\" from-name=\"root\">hello</cross-session-message>"}}),
    );
    append_native(
        &peer,
        json!({"type":"assistant","uuid":"reply-call","sessionId":"peer",
        "timestamp":"2026-09-24T00:00:04Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"reply","name":"SendMessage","input":{"to":"root","message":"reply"}}
        ]}}),
    );
    append_native(
        &peer,
        json!({"type":"user","sessionId":"peer",
        "timestamp":"2026-09-24T00:00:05Z","toolUseResult":{"success":true,"msg_id":"return-delivery"},
        "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"reply","content":"sent"}]}}),
    );
    append_native(
        &root,
        json!({"type":"user","sessionId":"root","timestamp":"2026-09-24T00:00:06Z",
        "origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/111.sock","name":"peer",
            "fromMode":"prompting","msg_id":"return-delivery","body":"reply","verifiedPeerPid":111,"verifiedPeerProcStart":"999"},
        "message":{"role":"user","content":"<cross-session-message from=\"uds:/run/user/1000/cc-socks/111.sock\" from-name=\"peer\">reply</cross-session-message>"}}),
    );
    let publish = |fixture: &mut Fixture| {
        crate::model::observe_catalog(
            &fixture.options,
            &mut fixture.cache,
            &mut false,
            &mut crate::model::Texts::default(),
            crate::model::now_ms(),
        )
        .unwrap();
    };
    publish(&mut fixture);
    let original = RelayFixtureProvider(BTreeMap::from([
        ("projects/proj/root.jsonl".into(), fs::read(&root).unwrap()),
        ("projects/proj/peer.jsonl".into(), fs::read(&peer).unwrap()),
    ]));
    let component = selected_page(&fixture, &[&root, &peer], "root", 20, None)
        .unwrap()
        .unwrap();
    let turn = component
        .records
        .iter()
        .find(|record| record.text().as_deref() == Some("Original trace"))
        .and_then(|record| record.causal.turn.as_ref())
        .unwrap()
        .id
        .clone();
    let provider = PeerProvider {
        bytes: fs::read(&peer).unwrap(),
        calls: AtomicUsize::new(0),
    };
    let address = selected::Address {
        path: "projects/proj/peer.jsonl".into(),
        offset: 0,
        block: 0,
        occurrence: 0,
    };
    fixture.claude("root", "Replacement trace");
    let db = Connection::open(&fixture.db).unwrap();
    let root_identity = warehouse::retained_identity(&db, &fixture.namespace, "root").unwrap();
    let source = fixture.source(&[&root, &peer]);
    assert!(matches!(
        selected::retained_record_at(&fixture.options, &source, "peer", &address, &provider),
        Err(ReadError::RetainedUnverified)
    ));
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
    let open = |fixture: &Fixture, witness: &str, key: &str| {
        selected::retained_peer_record_at(
            &fixture.source(&[&root, &peer]),
            "root",
            witness,
            key,
            &address,
            Some(&provider),
        )
    };
    assert!(matches!(
        open(&fixture, "replacement", "peer"),
        Err(ReadError::PeerUnavailable(_))
    ));
    assert!(matches!(
        open(&fixture, &root_identity.witness, "other"),
        Err(ReadError::PeerUnavailable(_))
    ));
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        open(&fixture, &root_identity.witness, "peer")
            .unwrap()
            .unwrap()
            .text()
            .as_deref(),
        Some("Original peer")
    );

    append_native(&peer, prompt("peer", 4, "After the trace"));
    publish(&mut fixture);
    assert_eq!(
        open(&fixture, &root_identity.witness, "peer")
            .unwrap()
            .unwrap()
            .text()
            .as_deref(),
        Some("Original peer")
    );
    let locally_proven = selected::retained_peer_record_at(
        &fixture.source(&[&root, &peer]),
        "root",
        &root_identity.witness,
        "peer",
        &address,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(locally_proven.text().as_deref(), Some("Original peer"));
    assert_eq!(locally_proven.reference.offset, provider.bytes.len() as u64);
    let relay = |fixture: &Fixture, block, provider: Option<&dyn SessionSourceReader>| {
        selected::retained_peer_relay_address(
            &fixture.source(&[&root, &peer]),
            "root",
            &root_identity.witness,
            "peer",
            (&sent, block),
            provider,
        )
    };
    let observed = relay(&fixture, 0, None).unwrap();
    assert_eq!(observed.record.path, address.path);
    assert_eq!(observed.record.offset, receipt_offset);
    assert!(matches!(
        relay(&fixture, 1, None),
        Err(ReadError::PeerUnavailable(_))
    ));
    fixture.claude("peer", "Replacement peer");
    let archived = relay(&fixture, 0, Some(&provider)).unwrap();
    assert_eq!(archived.record, observed.record);
    assert_eq!(archived.binding, observed.binding);
    assert_eq!(archived.entry_id, observed.entry_id);
    let source = fixture.source(&[&root, &peer]);
    let binding = selected::PeerBinding {
        trace: "root",
        witness: &root_identity.witness,
    };
    let opened = selected::retained_peer_window(
        &source,
        "peer",
        binding,
        32,
        None,
        Some((&archived.record, &archived.binding)),
        Some(&provider),
    )
    .unwrap();
    assert_eq!(opened.binding, archived.binding);
    let entries = selected::render::entries(&opened, source.namespace.as_str(), "peer", 0);
    assert!(
        entries
            .iter()
            .any(|entry| entry["entry_id"] == archived.entry_id)
    );
    let earlier = selected::retained_peer_window(
        &source,
        "peer",
        binding,
        32,
        opened.next.as_ref(),
        None,
        Some(&provider),
    )
    .unwrap();
    assert!(
        earlier
            .records
            .iter()
            .any(|record| record.text().as_deref() == Some("Original peer"))
    );
    assert!(
        earlier
            .records
            .iter()
            .all(|record| record.text().as_deref() != Some("Replacement peer"))
    );

    assert_eq!(
        open(&fixture, &root_identity.witness, "peer")
            .unwrap()
            .unwrap()
            .text()
            .as_deref(),
        Some("Original peer")
    );

    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let query = crate::native_html::query(&[
        ("sid", "root"),
        ("scope", "retained_history"),
        ("turn", &turn),
    ]);
    let reply = http::trace(&configured, &query, None, Some(&original));
    let trace: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{trace}");
    let peer_evidence = trace["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["catalog_key"] == "peer" && row["address"].is_object())
        .unwrap();
    let query = crate::native_html::query(&[
        ("sid", "peer"),
        ("scope", "retained_history"),
        ("turn", peer_evidence["turn"].as_str().unwrap()),
        ("address", &peer_evidence["address"].to_string()),
        ("peer", &peer_evidence["peer"].to_string()),
    ]);
    let reply = http::trace(&configured, &query, None, Some(&original));
    let continuation: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{continuation}");
    assert_eq!(&continuation["evidence"][0], peer_evidence);
    let return_edge = trace["handoffs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge["from"] == "peer" && edge["to"] == "root" && edge["status"] == "done")
        .unwrap_or_else(|| panic!("Missing return edge: {trace}"));
    let evidence = trace["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["catalog_key"] == "root" && row["turn"] == return_edge["target_turn"])
        .unwrap();
    assert!(evidence["peer"].is_null());
    let query = crate::native_html::query(&[
        ("sid", "root"),
        ("scope", "retained_history"),
        ("address", &evidence["address"].to_string()),
    ]);
    let reply = http::transcript(&configured, &query, None, Some(&original));
    let transcript: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{transcript}");
    assert!(
        transcript["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["address"] == evidence["address"])
    );

    db.execute(
        "UPDATE warehouse_identity SET witness='' WHERE catalog_key='peer' AND retained=1",
        [],
    )
    .unwrap();
    let calls = provider.calls.load(Ordering::Relaxed);
    assert!(
        matches!(open(&fixture, &root_identity.witness, "peer"), Err(ReadError::PeerUnavailable(reason)) if reason.contains("custody"))
    );
    assert_eq!(provider.calls.load(Ordering::Relaxed), calls);
}

struct OriginalNativeProvider {
    data: Vec<u8>,
    calls: std::sync::atomic::AtomicUsize,
}
impl crate::SessionSourceReader for OriginalNativeProvider {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> std::io::Result<crate::SessionSourceRange> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if source.root != "claude"
            || source.path != "projects/proj/native.jsonl"
            || source.native_id != "native"
            || source.offset != self.data.len() as u64
            || source.prefix_sha256 != <[u8; 32]>::from(Sha256::digest(&self.data))
            || expected.is_some_and(|generation| generation != "fixture-original")
        {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        let start = offset as usize;
        let end = (start + max).min(self.data.len());
        Ok(crate::SessionSourceRange {
            generation: "fixture-original".into(),
            length: source.offset,
            offset,
            bytes: self.data[start..end].into(),
            cached: false,
        })
    }
}

#[test]
fn warehouse_retained_address_reads_only_the_certified_original_from_provider() {
    let mut fixture = Fixture::new("retained-component-provider");
    let path = fixture.claude("native", "Original native prompt");
    let provider = OriginalNativeProvider {
        data: fs::read(&path).unwrap(),
        calls: Default::default(),
    };
    let archive =
        crate::archive::Archive::new(crate::archive::standalone_dir(&fixture.options.cache));
    archive
        .preserve(
            path.to_str().unwrap(),
            &mut provider.data.as_slice(),
            crate::archive::NATIVE_REDACTOR,
        )
        .unwrap();
    let address = selected::Address {
        path: "projects/proj/native.jsonl".into(),
        offset: 0,
        block: 0,
        occurrence: 0,
    };
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    assert!(matches!(
        selected::retained_record_at(
            &fixture.options,
            &fixture.source(&[&path]),
            "native",
            &address,
            &provider
        ),
        Err(ReadError::RetainedUnverified)
    ));
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    fixture.claude("native", "Replacement prompt");
    let replacement = fs::read(&path).unwrap();
    archive
        .preserve(
            path.to_str().unwrap(),
            &mut replacement.as_slice(),
            crate::archive::NATIVE_REDACTOR,
        )
        .unwrap();
    let retained = selected::retained_record_at(
        &fixture.options,
        &fixture.source(&[&path]),
        "native",
        &address,
        &provider,
    )
    .unwrap()
    .unwrap();
    assert_eq!(retained.text().as_deref(), Some("Original native prompt"));
    let missing = selected::Address {
        offset: provider.data.len() as u64,
        ..address
    };
    assert!(
        selected::retained_record_at(
            &fixture.options,
            &fixture.source(&[&path]),
            "native",
            &missing,
            &provider
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let source = fixture.source(&[&path]);
    let retained_identity = focused_identity(&source, CatalogReadScope::RetainedHistory, "native")
        .unwrap()
        .unwrap();
    let current_alias =
        resolve_native(&source, CatalogReadScope::Current, "claude", "native").unwrap();
    let retained_alias = resolve_native(
        &source,
        CatalogReadScope::RetainedHistory,
        "claude",
        "native",
    )
    .unwrap();
    assert_eq!(
        retained_alias.as_slice(),
        std::slice::from_ref(&retained_identity)
    );
    assert_eq!(current_alias.len(), 1);
    assert_eq!(current_alias[0].catalog_key, retained_identity.catalog_key);
    assert_ne!(current_alias[0].generation, retained_identity.generation);
    assert_ne!(
        current_alias[0].source_refs[0].source.prefix_sha256,
        retained_identity.source_refs[0].source.prefix_sha256
    );
    assert_eq!(
        retained_identity.source_refs[0].source.offset,
        provider.data.len() as u64
    );
    assert_eq!(
        retained_identity.source_refs[0].source.prefix_sha256,
        <[u8; 32]>::from(Sha256::digest(&provider.data))
    );
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let latest = selected::retained_latest_page(&source, "native", 1, None, &provider).unwrap();
    assert_eq!(
        latest.records[0].text().as_deref(),
        Some("Original native prompt")
    );
    let entries = selected::render::entries(&latest, fixture.namespace.as_str(), "native", 0);
    let address = entries[0]["address"].to_string();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    for (scope, alias) in [
        ("current", &current_alias[0]),
        ("retained_history", &retained_identity),
    ] {
        let query = crate::native_html::query(&[
            ("machine", &configured.key),
            ("harness", "claude"),
            ("native_id", "native"),
            ("scope", scope),
        ]);
        let response = http::resolve_native(&configured, &query);
        let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(response.status, 200, "{body}");
        assert_eq!(body["resolution"]["state"], "resolved");
        assert_eq!(body["resolution"]["generation"], alias.generation);
        assert_eq!(body["resolution"]["catalog_key"], alias.catalog_key);
    }
    for (scope, alias) in [
        ("current", &current_alias[0]),
        ("retained_history", &retained_identity),
    ] {
        let query = crate::native_html::query(&[("sid", "native"), ("scope", scope)]);
        let metadata = http::list(std::slice::from_ref(&configured), "fixture", &query, false);
        let identity = http::identity(&configured, &query);
        let locate = http::locate(&configured, &query);
        assert_eq!(
            (metadata.status, identity.status, locate.status),
            (200, 200, 200)
        );
        let metadata: serde_json::Value = serde_json::from_slice(&metadata.body).unwrap();
        let identity: serde_json::Value = serde_json::from_slice(&identity.body).unwrap();
        let locate: serde_json::Value = serde_json::from_slice(&locate.body).unwrap();
        assert_eq!(identity["identity"]["generation"], alias.generation);
        assert_eq!(metadata["generation"], alias.generation);
        assert_eq!(locate["matches"][0]["catalog_key"], alias.catalog_key);
    }
    let query = crate::native_html::query(&[
        ("sid", "native"),
        ("scope", "retained_history"),
        ("address", &address),
    ]);
    let reply = http::transcript(&configured, &query, None, Some(&provider));
    let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{body}");
    assert_eq!(body["entries"][0]["entry_id"], entries[0]["entry_id"]);
    assert_eq!(body["entries"][0]["text"], "Original native prompt");
    assert_eq!(body["session"]["name"], "Original native prompt");
    for query in [
        "scope=retained_history",
        "scope=retained_history&sid=native",
    ] {
        let listed = http::list(std::slice::from_ref(&configured), "fixture", query, false);
        let listed_body: serde_json::Value = serde_json::from_slice(&listed.body).unwrap();
        assert_eq!(listed.status, 200, "{listed_body}");
        assert_eq!(listed_body["items"][0]["name"], "Original native prompt");
        assert_eq!(listed_body["items"][0]["lifecycle"], "retained");
    }
    let pending = http::transcript(&configured, &query, None, None);
    assert_eq!(pending.status, 503);
    let pending: serde_json::Value = serde_json::from_slice(&pending.body).unwrap();
    assert_eq!(pending["error"]["code"], "retained_source_preparing");
    crate::archive::seekable::prepare(&archive, &path, &retained_identity.source_refs[0].source)
        .unwrap();
    let local = http::transcript(&configured, &query, None, None);
    let local_body: serde_json::Value = serde_json::from_slice(&local.body).unwrap();
    assert_eq!(local.status, 200, "{local_body}");
    assert_eq!(local_body["entries"][0]["text"], "Original native prompt");
    let first = selected::retained_page(&source, "native", 1, None, &provider).unwrap();
    assert_eq!(first.member_count, 1);
    let cursor = first.next.unwrap();
    let next = selected::retained_page(&source, "native", 1, Some(&cursor), &provider).unwrap();
    assert_eq!(
        next.records[0].text().as_deref(),
        Some("Original native prompt")
    );
    assert!(next.next.is_none());
    let denied = OriginalNativeProvider {
        data: vec![],
        calls: Default::default(),
    };
    assert_eq!(
        http::transcript(&configured, &query, None, Some(&denied)).status,
        503
    );
    assert!(matches!(
        selected::retained_page(&source, "native", 1, Some(&cursor), &denied),
        Err(ReadError::Restart)
    ));
}

#[test]
fn warehouse_sparse_fields_use_exact_provider_prefixes_in_both_read_scopes() {
    let mut fixture = Fixture::new("sparse-fields-provider");
    let path = fixture.claude("native", "Original prompt");
    let mut image = b"\x89PNG\r\n\x1a\nexample".to_vec();
    image.resize(1024 * 1024, 0);
    append_native(
        &path,
        json!({"type":"assistant","uuid":"read-call","sessionId":"native",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"large-read","name":"Read","input":{"file_path":"/work/note"}}
        ]}}),
    );
    append_native(
        &path,
        json!({"type":"user","uuid":"read-result","sessionId":"native",
        "timestamp":"2026-09-24T00:00:02Z","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"large-read","content":[
                {"type":"text","text":"y".repeat(1152 * 1024 + 1) + "result tail"}
            ]}
        ]}}),
    );
    let mut mixed = prompt("native", 3, "");
    mixed["message"]["content"] = json!([
        {"type":"image","source":{"type":"base64","media_type":"image/png",
            "data":crate::attachments::tests::encode(&image)}},
        {"type":"text","text":"x".repeat(768 * 1024 + 1) + "verified tail"},
    ]);
    append_native(&path, mixed);
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let provider = OriginalNativeProvider {
        data: fs::read(&path).unwrap(),
        calls: Default::default(),
    };
    let archive =
        crate::archive::Archive::new(crate::archive::standalone_dir(&fixture.options.cache));
    archive
        .preserve(
            path.to_str().unwrap(),
            &mut provider.data.as_slice(),
            crate::archive::NATIVE_REDACTOR,
        )
        .unwrap();
    let admit = |fixture: &mut Fixture| {
        crate::model::observe_catalog(
            &fixture.options,
            &mut fixture.cache,
            &mut false,
            &mut crate::model::Texts::default(),
            crate::model::now_ms(),
        )
        .unwrap();
    };
    admit(&mut fixture);
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let mode = crate::SessionSourceReadMode::ProviderOnly;
    let read = |scope: &str, params: &[(&str, &str)], expanded, reader: &OriginalNativeProvider| {
        let mut fields = vec![("sid", "native"), ("scope", scope)];
        fields.extend_from_slice(params);
        http::transcript_with_mode(
            &configured,
            &crate::native_html::query(&fields),
            Some(reader),
            mode,
            expanded,
        )
    };
    let check = |scope| {
        let page = read(scope, &[("limit", "4")], false, &provider);
        let body: serde_json::Value = serde_json::from_slice(&page.body).unwrap();
        assert_eq!(page.status, 200, "{body}");
        let entries = body["entries"].as_array().unwrap();
        let output = entries
            .iter()
            .find(|entry| entry["field"]["name"] == "out")
            .unwrap_or_else(|| panic!("missing sparse output: {body}"));
        let result_address = output["address"].to_string();
        let result_last = (output["field"]["chunks"].as_u64().unwrap() - 1).to_string();
        let chunk = read(
            scope,
            &[("address", &result_address), ("field_chunk", &result_last)],
            true,
            &provider,
        );
        let result: serde_json::Value = serde_json::from_slice(&chunk.body).unwrap();
        assert_eq!(chunk.status, 200, "{result}");
        assert_eq!(result["entry"]["field"]["name"], "out");
        assert!(
            result["entry"]["text"]
                .as_str()
                .unwrap()
                .ends_with("yresult tail")
        );
        let entry = entries
            .iter()
            .find(|entry| entry["field"]["name"] == "text")
            .unwrap_or_else(|| panic!("missing sparse prompt: {body}"));
        let mut encoded = entry["address"].clone();
        let address = encoded.to_string();
        let last = (entry["field"]["chunks"].as_u64().unwrap() - 1).to_string();
        let chunk = read(
            scope,
            &[("address", &address), ("field_chunk", &last)],
            true,
            &provider,
        );
        let body: serde_json::Value = serde_json::from_slice(&chunk.body).unwrap();
        assert_eq!(chunk.status, 200, "{body}");
        assert_eq!(body["entry"]["text"], "xverified tail");
        let attachment = read(
            scope,
            &[
                ("address", &address),
                ("part", "attachment"),
                ("image", "0"),
            ],
            true,
            &provider,
        );
        assert_eq!(
            (attachment.status, attachment.content_type),
            (200, "image/png")
        );
        assert_eq!(attachment.body, image);
        encoded["entry_id"] = json!("wrong-entry");
        let refused = read(
            scope,
            &[("address", &encoded.to_string()), ("field_chunk", &last)],
            true,
            &provider,
        );
        assert!((400..500).contains(&refused.status));
        let mut changed = provider.data.clone();
        *changed.last_mut().unwrap() = b' ';
        let changed = OriginalNativeProvider {
            data: changed,
            calls: Default::default(),
        };
        let refused = read(
            scope,
            &[("address", &address), ("field_chunk", &last)],
            true,
            &changed,
        );
        assert!(refused.status >= 400);
        let body: serde_json::Value = serde_json::from_slice(&refused.body).unwrap();
        assert!(body.get("entry").is_none());
    };
    check("current");
    assert!(provider.calls.load(std::sync::atomic::Ordering::Relaxed) > 0);
    let replacement = fixture.claude("native", "Replacement prompt");
    let bytes = fs::read(&replacement).unwrap();
    archive
        .preserve(
            replacement.to_str().unwrap(),
            &mut bytes.as_slice(),
            crate::archive::NATIVE_REDACTOR,
        )
        .unwrap();
    check("retained_history");
}

#[test]
fn warehouse_retained_tool_expansion_reads_the_frozen_reply() {
    let mut fixture = Fixture::new("retained-tool-reply");
    let path = fixture.claude("native", "Original prompt");
    append_native(
        &path,
        json!({"type":"assistant","uuid":"call","sessionId":"native",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"command","name":"Bash","input":{"command":"pwd"}}
        ]}}),
    );
    append_native(
        &path,
        json!({"type":"user","uuid":"reply","sessionId":"native",
        "timestamp":"2026-09-24T00:00:02Z","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"command","content":"original output"}
        ]}}),
    );
    append_native(
        &path,
        json!({"type":"assistant","uuid":"stopped","sessionId":"native",
        "timestamp":"2026-09-24T00:00:03Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"incomplete","name":"Bash","input":{"command":"ls"}}
        ]}}),
    );
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let archive =
        crate::archive::Archive::new(crate::archive::standalone_dir(&fixture.options.cache));
    archive
        .preserve(
            path.to_str().unwrap(),
            &mut fs::File::open(&path).unwrap(),
            crate::archive::NATIVE_REDACTOR,
        )
        .unwrap();
    fixture.claude("native", "Replacement prompt");
    let source = fixture.source(&[&path]);
    let identity = focused_identity(&source, CatalogReadScope::RetainedHistory, "native")
        .unwrap()
        .unwrap();
    crate::archive::seekable::prepare(&archive, &path, &identity.source_refs[0].source).unwrap();
    let reader = ArchiveReader::new(&fixture.options);
    let component = selected::retained_latest_page(&source, "native", 32, None, &reader).unwrap();
    let call = component
        .records
        .iter()
        .find(|record| !record.causal.calls.is_empty())
        .unwrap();
    let reply = call.causal.calls[0].replies.first().unwrap();
    assert_eq!(
        component.reply_records[&(call.address.path.clone(), reply.o)]["message"]["content"][0]["content"],
        "original output"
    );
    let entries = selected::render::entries(&component, source.namespace.as_str(), "native", 0);
    let pending = entries
        .iter()
        .find(|entry| entry["unfinished"] == true)
        .unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let reply = http::annotations(
        &configured,
        "sid=native&scope=retained_history&kind=errors",
        None,
        None,
    );
    let markers: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{markers}");
    assert_eq!(markers["total"], 1);
    assert_eq!(markers["entries"][0]["address"], pending["address"]);
    let call = entries
        .iter()
        .find(|entry| entry["k"] == "tool" && entry["ok"] == true)
        .unwrap();
    let address: crate::session_route::NativeEntryAddress =
        serde_json::from_value(call["address"].clone()).unwrap();
    let addressed = selected::retained_addressed_window(
        &source,
        "native",
        &address.record,
        &address.binding,
        &reader,
    )
    .unwrap();
    let Some(selected::render::Expansion::Json(entry)) = selected::render::expand(
        &addressed,
        source.namespace.as_str(),
        "native",
        &address,
        selected::render::Part::Output,
    ) else {
        panic!("frozen tool reply did not expand");
    };
    assert_eq!(entry["out"], "original output");
}

#[test]
fn warehouse_list_prices_claude_usage_with_the_existing_pricing_calculator() {
    let mut fixture = Fixture::new("warehouse-list-prices");
    let path = fixture.claude("native", "Price fixture");
    append_native(
        &path,
        json!({"type":"assistant","uuid":"priced-reply","sessionId":"native",
        "timestamp":"2026-09-24T00:00:03Z","message":{"id":"priced-message","role":"assistant",
        "model":"claude-sonnet-4-6","content":[{"type":"text","text":"Response"}],
        "usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":20}}}),
    );
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let page = read_list(
        &[fixture.source(&[&path])],
        "workspace",
        &request("limit=1"),
        None,
    )
    .unwrap();
    let cost = page.rows[0].cost.as_ref().unwrap();
    let expected =
        crate::session_catalog_page(&fixture.options, fixture.namespace.as_str(), "limit=1");
    assert_eq!(expected.status, 200);
    let oracle: serde_json::Value = serde_json::from_slice(&expected.body).unwrap();
    assert!(cost.priced_tokens > 0);
    assert_eq!(
        serde_json::to_value(cost).unwrap(),
        oracle["items"][0]["cost"]
    );
}

#[test]
fn warehouse_selected_family_prices_descendants_and_deduplicates_shared_steps() {
    let mut fixture = Fixture::new("selected-family-price");
    let root = fixture.claude("root", "Work");
    let child = fixture.claude("child", "Semon-Parent: claude:root\nChild work");
    let leaf = fixture.claude("leaf", "Semon-Parent: claude:child\nLeaf work");
    let paths = [root.as_path(), child.as_path(), leaf.as_path()];
    let observation = |id, uuid, message, second, input, model| {
        json!({"type":"assistant","uuid":uuid,"sessionId":id,
            "timestamp":format!("2026-09-24T00:00:{second:02}Z"),
            "message":{"id":message,"role":"assistant","model":model,
                "content":[{"type":"text","text":"Response"}],
                "usage":{"input_tokens":input,"output_tokens":10}}})
    };
    let model = "claude-sonnet-4-6";
    for (id, path) in [("root", &root), ("child", &child), ("leaf", &leaf)] {
        append_native(
            path,
            observation(
                id,
                format!("{id}-unique"),
                format!("{id}-message"),
                1,
                100,
                model,
            ),
        );
    }
    let observe = |fixture: &mut Fixture| {
        crate::model::observe_catalog(
            &fixture.options,
            &mut fixture.cache,
            &mut false,
            &mut crate::model::Texts::default(),
            crate::model::now_ms(),
        )
        .unwrap();
    };
    let oracle = |fixture: &Fixture| {
        Connection::open(&fixture.db).unwrap().query_row(
            "SELECT json_extract(metadata,'$.family_usd') FROM session_trace_headers WHERE session_key='root'",
            [], |row| row.get::<_, Option<f64>>(0),
        ).unwrap()
    };
    observe(&mut fixture);
    let source = Source::focused(
        fixture.namespace.clone(),
        &fixture.db,
        &fixture.options,
        None,
    )
    .unwrap();
    let (metadata, _) = focused_metadata(&source, "child").unwrap().unwrap();
    assert_eq!(metadata.parent.as_deref(), Some("root"));
    let details = metadata.details.unwrap();
    assert_eq!(details["family"]["state"], "complete");
    assert_eq!(details["family"]["ancestors"][0]["key"], "root");
    assert_eq!(details["family"]["runs"][0]["key"], "leaf");
    let child_family = family_price(&fixture.source(&paths), "child")
        .unwrap()
        .unwrap();
    assert_eq!(details["family"]["usd"], json!(child_family.usd));
    assert_eq!(details["family"]["shared_usd"], json!(child_family.shared));
    assert!(
        details["family"]["runs"][0]["cost"]["usd"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    let family = family_price(&fixture.source(&paths), "root")
        .unwrap()
        .unwrap();
    assert!(family.usd.unwrap() > 0.0);
    assert!((family.usd.unwrap() - oracle(&fixture).unwrap()).abs() < 1e-12);
    assert_eq!(family.shared, Default::default());
    assert!(matches!(
        family_price(&fixture.source(&paths[..2]), "root"),
        Err(ReadError::Restart)
    ));

    for (id, path, input) in [
        ("root", &root, 10),
        ("child", &child, 20),
        ("leaf", &leaf, 30),
    ] {
        let uuid = if id == "leaf" { "shared-b" } else { "shared-a" };
        append_native(
            path,
            observation(id, uuid.into(), "shared-message".into(), 2, input, model),
        );
    }
    // The child's second UUID connects the leaf to the root's invocation.
    append_native(
        &child,
        observation(
            "child",
            "shared-b".into(),
            "shared-message".into(),
            3,
            200,
            model,
        ),
    );
    observe(&mut fixture);
    let family = family_price(&fixture.source(&paths), "root")
        .unwrap()
        .unwrap();
    assert_eq!(family.usd, oracle(&fixture));
    assert!(family.usd.is_none());
    let mut expected = crate::pricing::CostBuild::with_table(crate::pricing::snapshot());
    expected.message(&crate::events::MessageUsage {
        model: Some(model.into()),
        billing: crate::events::BillingUsage {
            input: 10,
            output: 10,
            ..Default::default()
        },
        ..Default::default()
    });
    assert!((family.shared.known - expected.finish().usd.unwrap()).abs() < 1e-12);
    assert_eq!(family.shared.unknown_steps, 0);

    append_native(
        &root,
        observation(
            "root",
            "shared-a".into(),
            "shared-message".into(),
            4,
            10,
            "unknown-fixture-model",
        ),
    );
    observe(&mut fixture);
    let family = family_price(&fixture.source(&paths), "root")
        .unwrap()
        .unwrap();
    assert_eq!(family.shared.known, 0.0);
    assert_eq!(family.shared.unknown_steps, 1);
}

#[test]
fn warehouse_list_qualifies_codex_prompt_title_for_search() {
    let mut fixture = Fixture::new("list-prompt-title");
    let parent = fixture.codex("parent");
    let path = fixture
        .options
        .codex_home
        .join("sessions/2026/09/24/rollout-child.jsonl");
    fs::write(&path, []).unwrap();
    append_native(
        &path,
        json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z",
        "payload":{"id":"child","cwd":"/work/synthetic","originator":"codex_exec","thread_source":"subagent",
            "parent_thread_id":"parent","agent_nickname":"Worker"}}),
    );
    append_native(
        &path,
        json!({"type":"response_item","timestamp":"2026-09-24T00:00:01Z",
        "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Check the parsing\nand result correlation"}]}}),
    );
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let query = "q=check%20the%20parsing&limit=10";
    let actual = read_list(
        &[fixture.source(&[&path, &parent])],
        "workspace",
        &request(query),
        None,
    )
    .unwrap();
    assert_eq!(actual.rows.len(), 1);
    assert_eq!(
        actual.rows[0].title,
        "Check the parsing and result correlation"
    );
    let ordinary = read_list(
        &[fixture.source(&[&path, &parent])],
        "workspace",
        &request("limit=10"),
        None,
    )
    .unwrap();
    let child = ordinary
        .rows
        .iter()
        .find(|row| row.position.key == "child")
        .unwrap();
    assert_ne!(child.title, actual.rows[0].title);
    let expected =
        crate::session_catalog_page(&fixture.options, fixture.namespace.as_str(), "sid=child");
    assert_eq!(expected.status, 200);
    let oracle: serde_json::Value = serde_json::from_slice(&expected.body).unwrap();
    let expected = oracle["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "child")
        .unwrap();
    assert_eq!(expected["name"], actual.rows[0].title);
    assert_eq!(
        expected["parent"].as_str(),
        actual.rows[0].parent.as_deref()
    );
}

#[test]
fn warehouse_home_previews_a_native_human_ask() {
    let mut fixture = Fixture::new("home-ask");
    let path = fixture.claude("native", "Initial prompt");
    let offset = fs::metadata(&path).unwrap().len();
    append_native(
        &path,
        json!({"type":"user","uuid":"human-followup","sessionId":"native",
            "timestamp":"2026-09-24T00:00:01Z","origin":{"kind":"human"},
            "message":{"role":"user","content":"Review the second file"}}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let page = http::transcript(&configured, "sid=native", None, None);
    assert_eq!(page.status, 200);
    let page: serde_json::Value = serde_json::from_slice(&page.body).unwrap();
    let ask = crate::model::ask_id("native", offset);
    let generation = page["identity"]["generation"].as_str().unwrap();
    let context = &page["relationship_context"];
    assert_eq!(context["state"], "complete");
    assert_eq!(context["handoffs"][0]["id"], ask);
    assert_eq!(context["previews"]["native"], generation);
    let query = crate::native_html::query(&[
        ("sid", "native"),
        ("scope", "current"),
        ("generation", generation),
        ("events", &json!([ask]).to_string()),
    ]);
    let home = http::home(&configured, &query, None, None);
    assert_eq!(home.status, 200);
    let home: serde_json::Value = serde_json::from_slice(&home.body).unwrap();
    assert_eq!(home["events"][&ask]["brief"], "Review the second file");
    assert_eq!(home["session"]["detail_from"], "you");
    assert_eq!(home["incomplete"], false);
}

#[test]
fn warehouse_home_question_origin_uses_the_call_turn() {
    let mut fixture = Fixture::new("home-question-origin");
    let path = fixture.claude("native", "First prompt");
    append_native(&path, prompt("native", 1, "Review the fixture"));
    let offset = fs::metadata(&path).unwrap().len();
    append_native(
        &path,
        json!({"type":"assistant","uuid":"question","sessionId":"native",
        "timestamp":"2026-09-24T00:00:02Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"question-tool","name":"AskUserQuestion","input":{"questions":[
                {"question":"Which file?","header":"File","multiSelect":false,"options":[
                    {"label":"First","description":"Review the first file"},
                    {"label":"Second","description":"Review the second file"}]}
            ]}}
        ]}}),
    );
    append_native(&path, prompt("native", 3, "Latest prompt"));
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let identity = http::identity(&configured, "sid=native");
    assert_eq!(identity.status, 200);
    let identity: serde_json::Value = serde_json::from_slice(&identity.body).unwrap();
    let generation = identity["identity"]["generation"].as_str().unwrap();
    let question = crate::model::question_id("native", offset, Some("question-tool"));
    let feed = crate::session_overview_reply(
        std::slice::from_ref(&configured),
        "home",
        "",
        1_790_208_004_000,
    )
    .unwrap();
    let feed: serde_json::Value = serde_json::from_slice(&feed.body).unwrap();
    let event = &feed["answered"][0];
    assert_eq!(event["event"]["id"], question, "{feed}");
    assert_eq!(event["session"]["content_generation"], generation);
    let query = crate::native_html::query(&[
        ("sid", "native"),
        ("scope", "current"),
        ("generation", generation),
        ("events", &json!([question]).to_string()),
    ]);
    let home = http::home(&configured, &query, None, None);
    let value: serde_json::Value = serde_json::from_slice(&home.body).unwrap();
    assert_eq!(home.status, 200, "{value}");
    assert_eq!(value["events"][&question]["brief"], "Which file?");
    assert_eq!(value["events"][&question]["origin"], "Review the fixture");
}

#[test]
fn warehouse_home_completes_codex_questions_on_the_next_prompt_and_previews_results() {
    let mut fixture = Fixture::new("home-codex-events");
    let path = fixture.codex("native");
    let codex = |second, payload| {
        json!({"type":"response_item",
        "timestamp":format!("2026-09-24T00:00:{second:02}Z"),"payload":payload})
    };
    let user = |second, text| {
        codex(
            second,
            json!({"type":"message","role":"user",
        "content":[{"type":"input_text","text":text}]}),
        )
    };
    append_native(&path, user(1, "Review the fixture"));
    let question_offset = fs::metadata(&path).unwrap().len();
    append_native(
        &path,
        codex(
            2,
            json!({"type":"function_call","name":"request_user_input",
        "call_id":"ask","arguments":json!({"questions":[{"question":"Which file?"}]}).to_string()}),
        ),
    );
    append_native(
        &path,
        codex(
            3,
            json!({"type":"function_call_output","call_id":"ask",
        "output":"{\"accepted\":true}"}),
        ),
    );
    append_native(&path, user(4, "Use the first file"));
    let result_offset = fs::metadata(&path).unwrap().len();
    append_native(
        &path,
        codex(
            5,
            json!({"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"Review complete"}]}),
        ),
    );
    scan_file(&path, "codex", &mut fixture.cache, &mut false).unwrap();
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let question = crate::model::question_id("native", question_offset, Some("ask"));
    let result = crate::model::result_id("native", result_offset, 0);
    let home = |query| {
        let reply = crate::session_overview_reply(
            std::slice::from_ref(&configured),
            "home",
            query,
            1_790_208_006_000,
        )
        .unwrap();
        serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap()
    };
    let feed = home("");
    assert_eq!(feed["answered"][0]["event"]["id"], question, "{feed}");
    assert_eq!(feed["answered"][0]["event"]["done"], 1_790_208_004_000_i64);
    assert_eq!(feed["inbox"][0]["event"]["id"], result, "{feed}");
    let generation = feed["inbox"][0]["session"]["content_generation"]
        .as_str()
        .unwrap();
    let events = json!([question, result]).to_string();
    let query = crate::native_html::query(&[
        ("sid", "native"),
        ("scope", "current"),
        ("generation", generation),
        ("events", &events),
    ]);
    let reply = http::home(&configured, &query, None, None);
    let detail: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{detail}");
    assert_eq!(detail["events"][&question]["brief"], "Which file?");
    assert_eq!(detail["events"][&question]["origin"], "Review the fixture");
    assert_eq!(detail["events"][&result]["brief"], "Review complete");
    assert_eq!(detail["events"][&result]["origin"], "Use the first file");
    let seen = json!([json!([configured.key, result]).to_string()]).to_string();
    let filtered = home(&crate::native_html::query(&[("seen", &seen)]));
    assert_eq!(filtered["inbox"], json!([]));
    assert_eq!(filtered["answered"][0]["event"]["id"], question);
}

#[test]
fn warehouse_home_result_preserves_the_human_turn_through_its_spawn_notification() {
    let mut fixture = Fixture::new("home-result-boundary");
    let case = |fixture: &mut Fixture, key: &str, origin: &str, content: &str| {
        let path = fixture.claude(key, "Review the fixture");
        let mut ask = prompt(key, 1, "Review the second file");
        ask["origin"] = json!({"kind":"human"});
        append_native(&path, ask);
        append_native(
            &path,
            json!({"type":"assistant","uuid":"spawn-call","sessionId":key,
            "timestamp":"2026-09-24T00:00:02Z","message":{"role":"assistant","content":[
                {"type":"tool_use","id":"spawn","name":"Agent","input":{"prompt":"Review"}}
            ]}}),
        );
        append_native(
            &path,
            json!({"type":"user","uuid":"incoming","sessionId":key,
            "timestamp":"2026-09-24T00:00:03Z","origin":{"kind":origin},
            "message":{"role":"user","content":content}}),
        );
        let offset = fs::metadata(&path).unwrap().len();
        append_native(
            &path,
            json!({"type":"assistant","uuid":"result","sessionId":key,
            "timestamp":"2026-09-24T00:00:04Z","message":{"role":"assistant","content":[
                {"type":"text","text":"Review complete"}
            ]}}),
        );
        scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
        let db = Connection::open(&fixture.db).unwrap();
        let owner: i64 = db
            .query_row(
                "SELECT source FROM dim_session WHERE native_id=?1",
                [key],
                |row| row.get(0),
            )
            .unwrap();
        let actual = db
            .query_row(
                include_str!("home_result_feed.sql"),
                params![owner, 0, crate::events::AGM_FINAL],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .unwrap();
        (actual, (key.to_owned(), i64::try_from(offset).unwrap()))
    };
    let notification = "<task-notification><tool-use-id>spawn</tool-use-id><status>completed</status></task-notification>";
    let (actual, expected) = case(&mut fixture, "own", "task-notification", notification);
    assert_eq!(actual, Some(expected));
    let unrelated = notification.replace(">spawn<", ">other<");
    let (actual, _) = case(&mut fixture, "unrelated", "task-notification", &unrelated);
    assert!(actual.is_none());
}

#[test]
fn warehouse_list_runtime_rejects_pid_reuse_and_tracks_wait() {
    let mut fixture = Fixture::new("list-runtime");
    let path = fixture.claude("native", "Runtime fixture");
    append_native(
        &path,
        json!({"type":"assistant","uuid":"pending","sessionId":"native",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"pending-tool","name":"Read","input":{"file_path":"/work/note"}}
        ]}}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let pids = fixture.options.claude_home.join("sessions");
    fs::create_dir_all(&pids).unwrap();
    let stat = fixture.options.proc_root.join("42/stat");
    fs::create_dir_all(stat.parent().unwrap()).unwrap();
    let write_start = |start| {
        let mut fields = vec!["0".to_owned(); 20];
        fields[19] = format!("{start}");
        fs::write(&stat, format!("42 (claude) {}\n", fields.join(" "))).unwrap();
    };
    write_start(777);
    let write_pid = |status| {
        fs::write(
            pids.join("42.json"),
            json!({"pid":42,"sessionId":"native","procStart":777,"status":status}).to_string(),
        )
        .unwrap()
    };
    let read = || {
        read_list(
            &[fixture.source(&[&path])],
            "workspace",
            &request("limit=10"),
            None,
        )
        .unwrap()
        .rows[0]
            .status
    };
    write_pid("busy");
    assert_eq!(read(), Some("work"));
    let selected = || {
        focused_metadata(&fixture.source(&[&path]), "native")
            .unwrap()
            .unwrap()
            .0
    };
    assert_eq!(selected().details.unwrap()["activity"]["tool"], "Read");
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().to_owned(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let page = http::transcript(&configured, "sid=native", None, None);
    assert_eq!(page.status, 200);
    let page: serde_json::Value = serde_json::from_slice(&page.body).unwrap();
    let query = crate::native_html::query(&[
        ("sid", "native"),
        ("scope", "current"),
        (
            "generation",
            page["identity"]["generation"].as_str().unwrap(),
        ),
        ("session_fields", "[\"activity\"]"),
    ]);
    let home_activity = || {
        let reply = http::home(&configured, &query, None, None);
        assert_eq!(reply.status, 200);
        serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap()["session"]["activity"]
            .clone()
    };
    let activity = home_activity();
    assert_eq!(activity[0], "Read");
    assert_eq!(activity[1], "/work/note");
    assert_eq!(
        activity[3],
        selected().details.unwrap()["activity"]["since"]
    );
    write_pid("waiting");
    assert!(home_activity().is_null());
    assert!(selected().details.unwrap()["activity"].is_null());
    assert_eq!(read(), Some("wait"));
    write_start(778);
    assert_eq!(read(), Some("done"));
}

#[test]
fn warehouse_selected_runtime_uses_qualified_parent_and_child_run_evidence() {
    let mut fixture = Fixture::new("child-runtime");
    let root = fixture.claude("root", "Delegate work");
    append_native(
        &root,
        json!({"type":"assistant","uuid":"launch","sessionId":"root",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"agent-call","name":"Agent","input":{"prompt":"Do the work"}}
        ]}}),
    );
    let child = root.with_extension("").join("subagents/agent-worker.jsonl");
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::write(
        child.with_extension("meta.json"),
        json!({"agentType":"general-purpose",
        "description":"The worker","toolUseId":"agent-call"})
        .to_string(),
    )
    .unwrap();
    fs::write(&child, format!("{}\n", prompt("root", 2, "Do the work"))).unwrap();
    let pids = fixture.options.claude_home.join("sessions");
    fs::create_dir_all(&pids).unwrap();
    let stat = fixture.options.proc_root.join("42/stat");
    fs::create_dir_all(stat.parent().unwrap()).unwrap();
    let write_start = |start| {
        let mut fields = vec!["0".to_owned(); 20];
        fields[19] = format!("{start}");
        fs::write(&stat, format!("42 (claude) {}\n", fields.join(" "))).unwrap();
    };
    write_start(777);
    fs::write(
        pids.join("42.json"),
        json!({"pid":42,"sessionId":"root","procStart":777,
        "status":"idle"})
        .to_string(),
    )
    .unwrap();

    let read = |fixture: &mut Fixture, list: &str, detail: &str| {
        crate::model::observe_catalog(
            &fixture.options,
            &mut fixture.cache,
            &mut false,
            &mut crate::model::Texts::default(),
            crate::model::now_ms(),
        )
        .unwrap();
        let page = read_list(
            &[fixture.source(&[&root, &child])],
            "workspace",
            &request("limit=10"),
            None,
        )
        .unwrap();
        let row = page
            .rows
            .iter()
            .find(|row| row.position.key == "worker")
            .unwrap();
        assert_eq!(row.parent.as_deref(), Some("root"));
        assert_eq!(row.status, Some(list));
        let source = fixture.source(&[&root, &child]);
        let (row, _) = focused_metadata(&source, "worker").unwrap().unwrap();
        assert_eq!(row.status, Some(detail));
        let oracle: String = Connection::open(&fixture.db).unwrap().query_row(
            "SELECT json_extract(metadata,'$.state') FROM session_trace_headers WHERE session_key='worker'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(row.status, Some(oracle.as_str()));
    };
    read(&mut fixture, "work", "work");
    append_native(
        &root,
        json!({"type":"user","sessionId":"root","timestamp":"2026-09-24T00:00:03Z",
        "origin":{"kind":"task-notification"},"message":{"role":"user","content":
        "<task-notification><task-id>worker</task-id><tool-use-id>agent-call</tool-use-id><status>failed</status><result>failed</result></task-notification>"}}),
    );
    read(&mut fixture, "work", "err");
    append_native(&child, prompt("root", 4, "Try again"));
    read(&mut fixture, "work", "work");
    let mut missing_parent = fixture.source(&[&root, &child]);
    missing_parent
        .inventory
        .insert(root.to_str().unwrap().into(), Availability::Unavailable);
    let page = read_list(&[missing_parent], "workspace", &request("limit=10"), None).unwrap();
    assert_eq!(
        page.rows
            .iter()
            .find(|row| row.position.key == "worker")
            .unwrap()
            .status,
        None
    );
    write_start(778);
    read(&mut fixture, "done", "done");
}

#[test]
fn warehouse_launch_without_prompt_stays_at_the_selected_head() {
    let mut fixture = Fixture::new("promptless-launch");
    let root = fixture.claude("root", "Delegate work");
    let child = root.with_extension("").join("subagents/agent-worker.jsonl");
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::write(
        child.with_extension("meta.json"),
        json!({"agentType":"general-purpose",
        "toolUseId":"agent"})
        .to_string(),
    )
    .unwrap();
    fs::write(
        &child,
        format!(
            "{}\n",
            json!({"type":"assistant","uuid":"reply","sessionId":"root",
        "timestamp":"2026-09-24T00:00:02Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Started without a prompt"}]}})
        ),
    )
    .unwrap();
    append_native(
        &child,
        json!({"type":"assistant","uuid":"reply-two","sessionId":"root",
        "timestamp":"2026-09-24T00:00:04Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Continued without a prompt"}]}}),
    );
    scan_file(&child, "claude", &mut fixture.cache, &mut false).unwrap();
    let paths = [root.as_path(), child.as_path()];
    let first = selected_page(&fixture, &paths, "worker", 1, None)
        .unwrap()
        .unwrap();
    let turn = first.records[0].causal.turn.as_ref().unwrap();
    assert_eq!(turn.id, crate::model::spawn_turn_id("claude", "worker"));
    assert_eq!(turn.address, first.records[0].address);
    append_native(&child, prompt("root", 3, "First actual prompt"));
    scan_file(&child, "claude", &mut fixture.cache, &mut false).unwrap();
    let next = selected_page(&fixture, &paths, "worker", 1, first.next.as_ref())
        .unwrap()
        .unwrap();
    assert_eq!(next.records[0].causal.turn.as_ref(), Some(turn));
}

#[test]
fn warehouse_prompt_parent_matches_the_authorized_launch_address() {
    let mut fixture = Fixture::new("prompt-launch");
    let parent = fixture.claude("parent", "Launch fixture");
    append_native(
        &parent,
        json!({"type":"assistant","uuid":"launcher","sessionId":"parent",
        "timestamp":"2026-09-24T00:00:01Z","message":{"role":"assistant","content":[
          {"type":"tool_use","id":"launch","name":"Bash","input":{"command":"codex exec /work/synthetic/prompt.txt"}}
        ]}}),
    );
    scan_file(&parent, "claude", &mut fixture.cache, &mut false).unwrap();
    let child = fixture.codex("child");
    append_native(
        &child,
        json!({"type":"response_item","timestamp":"2026-09-24T00:00:02Z",
      "payload":{"type":"message","role":"user","content":[{"type":"input_text",
        "text":"Semon-Parent: claude:parent\nSemon-Handoff: /work/synthetic/prompt.txt\nReview the fixture"}]}}),
    );
    scan_file(&child, "codex", &mut fixture.cache, &mut false).unwrap();
    let mut source = fixture.source(&[&parent, &child]);
    let current = selected::page(
        &fixture.options,
        &source,
        CatalogReadScope::Current,
        "child",
        10,
        None,
    )
    .unwrap()
    .unwrap();
    let expected = crate::model::spawn_turn_id("codex", "child");
    assert!(current.records.iter().any(|record| {
        record.prompt_anchor.as_deref() == Some(expected.as_str())
            && record
                .causal
                .turn
                .as_ref()
                .is_some_and(|turn| turn.handoff && turn.address == record.address)
    }));
    let launch = selected::launch_call(
        &selected::ReadView::open(&source).unwrap(),
        &source,
        "child",
    )
    .unwrap()
    .unwrap();
    assert_eq!(launch.id, "launch");
    assert_eq!(launch.name, "Bash");
    source
        .inventory
        .insert(parent.to_str().unwrap().into(), Availability::Unavailable);
    assert!(
        selected::launch_call(
            &selected::ReadView::open(&source).unwrap(),
            &source,
            "child"
        )
        .unwrap()
        .is_none()
    );

    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let provider = RelayFixtureProvider(BTreeMap::from([
        (
            "projects/proj/parent.jsonl".into(),
            fs::read(&parent).unwrap(),
        ),
        (
            "sessions/2026/09/24/rollout-child.jsonl".into(),
            fs::read(&child).unwrap(),
        ),
    ]));
    append_native(
        &parent,
        json!({"type":"assistant","uuid":"later-launcher","sessionId":"parent",
          "timestamp":"2026-09-24T00:00:03Z","message":{"role":"assistant","content":[
          {"type":"tool_use","id":"later-launch","name":"Bash","input":{"command":"codex exec /work/synthetic/prompt.txt"}}
        ]}}),
    );
    scan_file(&parent, "claude", &mut fixture.cache, &mut false).unwrap();
    fixture.codex("child");
    let db = Connection::open(&fixture.db).unwrap();
    let identity = warehouse::retained_identity(&db, &fixture.namespace, "child").unwrap();
    let source = fixture.source(&[&parent, &child]);
    let (address, block, retained) = selected::retained_peer_launch_call(
        &source,
        "child",
        &identity.witness,
        "parent",
        Some(&provider),
    )
    .unwrap();
    assert_eq!(retained, launch);
    assert_eq!(address.path, "projects/proj/parent.jsonl");
    assert_eq!(block, 0);
    let configured = crate::OverviewSource {
        key: fixture.namespace.as_str().into(),
        label: "Fixture".into(),
        options: fixture.options.clone(),
        up: true,
    };
    let query = crate::native_html::query(&[
        ("sid", "child"),
        ("scope", "retained_history"),
        ("turn", &expected),
    ]);
    let response = http::trace(&configured, &query, None, Some(&provider));
    let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(response.status, 200, "{body}");
    assert_eq!(body["projection"], 7);
    assert_eq!(body["turns"][0]["id"], expected);
    assert_eq!(body["handoffs"][0]["from"], "parent");
    let peer = &body["evidence"][1];
    assert_eq!(peer["address"]["record"], json!(address));
    assert_eq!(peer["peer"]["witness"], identity.witness);
    let address = peer["address"].to_string();
    let peer = peer["peer"].to_string();
    let query = crate::native_html::query(&[
        ("sid", "parent"),
        ("scope", "retained_history"),
        ("address", &address),
        ("peer", &peer),
    ]);
    let response = http::transcript(&configured, &query, None, Some(&provider));
    let reopened: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(response.status, 200, "{reopened}");
    assert!(
        reopened["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["address"] == body["evidence"][1]["address"] })
    );
    assert!(matches!(
        selected::retained_peer_launch_call(
            &source,
            "child",
            &"0".repeat(64),
            "parent",
            Some(&provider)
        ),
        Err(ReadError::PeerUnavailable(_))
    ));
}

#[test]
fn warehouse_retained_signal_survives_append_and_prefix_expiry() {
    let mut fixture = Fixture::new("retained-signal-expiry");
    let path = fixture.claude("native", "Original prompt");
    let offset = fs::metadata(&path).unwrap().len();
    append_native(
        &path,
        json!({"type":"system","subtype":"compact_boundary",
        "sessionId":"native","timestamp":"2026-09-24T00:00:01Z"}),
    );
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let archive =
        crate::archive::Archive::new(crate::archive::standalone_dir(&fixture.options.cache));
    archive
        .preserve(
            path.to_str().unwrap(),
            &mut fs::File::open(&path).unwrap(),
            crate::archive::NATIVE_REDACTOR,
        )
        .unwrap();
    let db = Connection::open(&fixture.db).unwrap();
    let (source_id, revision): (i64, i64) = db
        .query_row(
            "SELECT m.source,m.revision FROM warehouse_identity i
         JOIN warehouse_identity_member m ON m.identity=i.id WHERE i.catalog_key='native'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    db.execute(
        "UPDATE warehouse_prefix SET committed_at=?1 WHERE source=?2",
        params![
            crate::model::now_ms() - warehouse::PREFIX_WINDOW_MS - 1,
            source_id
        ],
    )
    .unwrap();
    append_native(
        &path,
        json!({"type":"system","subtype":"compact_boundary",
        "sessionId":"native","timestamp":"2026-09-24T00:00:02Z",
        "compactMetadata":{"trigger":"auto","preTokens":123}}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let expired: bool = db
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM warehouse_prefix WHERE revision=?1)",
            [revision],
            |row| row.get(0),
        )
        .unwrap();
    assert!(expired);
    fixture.claude("native", "Replacement prompt");
    let source = fixture.source(&[&path]);
    let identity = focused_identity(&source, CatalogReadScope::RetainedHistory, "native")
        .unwrap()
        .unwrap();
    crate::archive::seekable::prepare(&archive, &path, &identity.source_refs[0].source).unwrap();
    let page = selected::retained_latest_page(
        &source,
        "native",
        32,
        None,
        &ArchiveReader::new(&fixture.options),
    )
    .unwrap();
    let signal = page
        .records
        .iter()
        .find(|record| record.address.offset == offset)
        .unwrap()
        .causal
        .signals
        .iter()
        .find(|slot| slot.signal.k == crate::events::SignalKind::Compact)
        .unwrap();
    assert_eq!(signal.signal.n, None);
    assert_eq!(signal.signal.v, None);
}

#[test]
fn warehouse_component_continuation_preserves_signal_before_late_enrichment() {
    let mut fixture = Fixture::new("component-signal-as-of");
    let path = fixture.claude("native", "Signal fixture");
    let compaction = fs::metadata(&path).unwrap().len();
    append_native(
        &path,
        json!({"type":"system","subtype":"compact_boundary",
        "sessionId":"native","timestamp":"2026-09-24T00:00:01Z"}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let first = selected_page(&fixture, &[&path], "native", 1, None)
        .unwrap()
        .unwrap();
    let cursor = first.next.unwrap();
    append_native(
        &path,
        json!({"type":"system","subtype":"compact_boundary",
        "sessionId":"native","timestamp":"2026-09-24T00:00:02Z",
        "compactMetadata":{"trigger":"auto","preTokens":123}}),
    );
    scan_file(&path, "claude", &mut fixture.cache, &mut false).unwrap();
    let address = selected::Address {
        path: "projects/proj/native.jsonl".into(),
        offset: compaction,
        block: 0,
        occurrence: 0,
    };
    let source = fixture.source(&[&path]);
    let as_of = selected_page(&fixture, &[&path], "native", 10, Some(&cursor))
        .unwrap()
        .unwrap();
    let original = as_of
        .records
        .iter()
        .find(|record| record.address == address)
        .unwrap();
    let fresh = selected::record_at(&fixture.options, &source, "native", &address, None)
        .unwrap()
        .unwrap();
    let old = &original.causal.signals[0];
    let latest = &fresh.causal.signals[0];
    assert_eq!(old.signal.k, crate::events::SignalKind::Compact);
    assert_eq!(old.signal.n, None);
    assert_eq!(latest.signal.n.as_deref(), Some("auto"));
    assert_eq!(latest.signal.v, Some(123));
    assert_eq!(old.id, latest.id);
}

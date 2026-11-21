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
            current_provider: None,
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
        let oracle = crate::session_transcript_range_with_mode(
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
            let reply = crate::session_entry_field_with_mode(
                &fixture.options,
                fixture.namespace.as_str(),
                &format!(
                    "sid={key}&after={}&limit=1&generation={}&field_chunk=0",
                    entry["slot"],
                    oracle["projection"]["generation"].as_str().unwrap()
                ),
                None,
                mode,
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
        let reply = crate::session_turn_anchor(
            &fixture.options,
            fixture.namespace.as_str(),
            &format!("sid={key}&turn={}", crate::viewer::percent_encode(turn)),
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
    let found = selected::activity_anchor(&fixture.options, &source, "native", &anchor, None)
        .unwrap()
        .unwrap();
    assert_eq!(found.text().as_deref(), Some("Review complete"));
    let oracle = |key| {
        let reply = crate::session_transcript_range_with_mode(
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
        selected::activity_anchor(&fixture.options, &source, "native", &anchor, None)
            .unwrap()
            .is_none()
    );
    anchor.ordinal = 0;
    anchor.generation.push_str("-stale");
    assert!(
        selected::activity_anchor(&fixture.options, &source, "native", &anchor, None)
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
    assert_eq!(found.operation_parent, Some(call));
    let transcript = oracle("codex-native");
    let entry = transcript["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["script"] == call)
        .unwrap();
    assert_eq!(found.entry_anchor.as_deref(), entry["entry_id"].as_str());
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
    let oracle = crate::session_transcript_range_with_mode(
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

    struct Provider(BTreeMap<String, Vec<u8>>);
    impl crate::SessionSourceReader for Provider {
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
            if source.root != "claude"
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
    let provider = Provider(BTreeMap::from([
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
fn warehouse_retained_address_reads_only_the_certified_original_from_provider() {
    use crate::{SessionSourceRange, SessionSourceReader, SessionSourceRef};
    use std::io;

    struct Original {
        data: Vec<u8>,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl SessionSourceReader for Original {
        fn read_range(
            &self,
            source: &SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<SessionSourceRange> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if source.root != "claude"
                || source.path != "projects/proj/native.jsonl"
                || source.native_id != "native"
                || source.offset != self.data.len() as u64
                || source.prefix_sha256 != <[u8; 32]>::from(Sha256::digest(&self.data))
                || expected.is_some_and(|generation| generation != "fixture-original")
            {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let start = offset as usize;
            let end = (start + max).min(self.data.len());
            Ok(SessionSourceRange {
                generation: "fixture-original".into(),
                length: source.offset,
                offset,
                bytes: self.data[start..end].into(),
                cached: false,
            })
        }
    }

    let mut fixture = Fixture::new("retained-component-provider");
    let path = fixture.claude("native", "Original native prompt");
    let provider = Original {
        data: fs::read(&path).unwrap(),
        calls: Default::default(),
    };
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
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    let source = fixture.source(&[&path]);
    let first = selected::retained_page(&source, "native", 1, None, &provider).unwrap();
    assert_eq!(first.member_count, 1);
    let cursor = first.next.unwrap();
    let next = selected::retained_page(&source, "native", 1, Some(&cursor), &provider).unwrap();
    assert_eq!(
        next.records[0].text().as_deref(),
        Some("Original native prompt")
    );
    assert!(next.next.is_none());
    let denied = Original {
        data: vec![],
        calls: Default::default(),
    };
    assert!(matches!(
        selected::retained_page(&source, "native", 1, Some(&cursor), &denied),
        Err(ReadError::Restart)
    ));
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
    let expected =
        crate::session_catalog_page(&fixture.options, fixture.namespace.as_str(), "limit=10");
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
fn warehouse_list_runtime_rejects_pid_reuse_and_tracks_wait() {
    let mut fixture = Fixture::new("list-runtime");
    let path = fixture.claude("native", "Runtime fixture");
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
    write_pid("waiting");
    assert_eq!(read(), Some("wait"));
    write_start(778);
    assert_eq!(read(), Some("done"));
}

#[test]
fn warehouse_list_runtime_uses_qualified_parent_and_child_run_evidence() {
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

    let read = |fixture: &mut Fixture, expected: &str| {
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
        assert_eq!(row.status, Some(expected));
        let oracle: String = Connection::open(&fixture.db).unwrap().query_row(
            "SELECT json_extract(metadata,'$.state') FROM session_trace_headers WHERE session_key='worker'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(row.status, Some(oracle.as_str()));
    };
    read(&mut fixture, "work");
    append_native(
        &root,
        json!({"type":"user","sessionId":"root","timestamp":"2026-09-24T00:00:03Z",
        "origin":{"kind":"task-notification"},"message":{"role":"user","content":
        "<task-notification><task-id>worker</task-id><tool-use-id>agent-call</tool-use-id><status>failed</status><result>failed</result></task-notification>"}}),
    );
    read(&mut fixture, "err");
    append_native(&child, prompt("root", 4, "Try again"));
    read(&mut fixture, "work");
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
    read(&mut fixture, "done");
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
    let launch = selected::launch_call(&source, "child").unwrap().unwrap();
    assert_eq!(launch.id, "launch");
    assert_eq!(launch.name, "Bash");
    source
        .inventory
        .insert(parent.to_str().unwrap().into(), Availability::Unavailable);
    assert!(selected::launch_call(&source, "child").unwrap().is_none());
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

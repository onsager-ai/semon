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
    let detail = plan(
        &db,
        warehouse::DETAIL,
        rusqlite::named_params! {
            ":source": 1, ":start": 0, ":end": 100, ":component": 1, ":view": 1,
            ":entry_block": None::<i64>, ":entry_occurrence": None::<i64>, ":time": i64::MIN,
            ":pos": i64::MIN, ":block": i64::MIN, ":occurrence": i64::MIN, ":limit": 10,
        },
    );
    assert!(
        detail.contains("record_position") && detail.contains("record_uuid"),
        "{detail}"
    );
    assert!(
        !detail.contains("TEMP B-TREE") && !detail.contains("SCAN fact_record"),
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
    crate::model::observe_catalog(
        &fixture.options,
        &mut fixture.cache,
        &mut false,
        &mut crate::model::Texts::default(),
        crate::model::now_ms(),
    )
    .unwrap();
    let actual = selected_page(&fixture, &[&path], "native", 100, None)
        .unwrap()
        .unwrap();
    assert_eq!(actual.title.as_deref(), Some("Original title"));
    assert!(actual.model.is_none());
    let mode = crate::SessionSourceReadMode::LocalOrPinnedProvider;
    let oracle = crate::session_transcript_range_with_mode(
        &fixture.options,
        fixture.namespace.as_str(),
        "sid=native&after=0&limit=100",
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
            matches!(
                entry["text"].as_str(),
                Some("Original title" | "Qualified answer")
            )
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
        assert_eq!(entry["text"], text);
        assert_eq!(entry["provenance"]["source"]["path"], record.address.path);
        assert_eq!(entry["provenance"]["offset"], record.address.offset);
        assert_eq!(entry["provenance"]["block"], record.address.block);
        let expanded = selected::record_at(
            &fixture.options,
            &fixture.source(&[&path]),
            "native",
            &record.address,
            None,
        )
        .unwrap()
        .unwrap();
        let reply = crate::session_entry_field_with_mode(
            &fixture.options,
            fixture.namespace.as_str(),
            &format!(
                "sid=native&after={}&limit=1&generation={}&field_chunk=0",
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
        &format!("sid=native&turn={}", crate::viewer::percent_encode(turn)),
    );
    assert_eq!(reply.status, 200);
    let anchor: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(anchor["slot"], expected[0]["slot"]);
    let address = &actual
        .records
        .iter()
        .find(|record| record.text().as_deref() == Some("Original title"))
        .unwrap()
        .address;
    assert_eq!(
        selected::record_at(
            &fixture.options,
            &fixture.source(&[&path]),
            "native",
            address,
            None
        )
        .unwrap()
        .unwrap()
        .text()
        .as_deref(),
        Some("Original title")
    );
}

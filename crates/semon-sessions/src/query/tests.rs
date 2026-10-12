//! The read surface's tools on a synthetic home, their JSON pinned: one
//! session per state, and a stalled one per reason code.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::{Value, json};

use super::*;
use crate::ViewerCore;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn admitted(options: Options) -> Arc<SessionSources> {
    Arc::new(SessionSources::with_machines(vec![(
        String::new(),
        options,
    )]))
}

fn catalog_generation(home: &Home) -> Option<String> {
    crate::events::EventCache::open_scoped(&home.options.cache)
        .unwrap()
        .session_catalog_generation()
}

#[test]
fn native_producer_catalog_rejects_rewrites_after_indexing() {
    let home = Home::new("catalog-derivation");
    home.top("lead", &[user("lead", ts(1, 0), "old evidence")]);
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    let mut texts = crate::model::Texts::default();
    crate::model::observe_catalog(&home.options, &mut cache, &mut false, &mut texts, NOW).unwrap();
    let generation = catalog_generation(&home);
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    crate::model::AFTER_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let bytes = fs::read_to_string(&path).unwrap();
            std::thread::sleep(Duration::from_millis(2));
            fs::write(path, bytes.replace("old evidence", "new evidence")).unwrap();
        }));
    });
    assert!(
        crate::model::observe_catalog(&home.options, &mut cache, &mut false, &mut texts, NOW)
            .is_err()
    );
    assert_eq!(catalog_generation(&home), generation);
}

#[test]
fn native_producer_scoped_preparation_rejects_deleted_evidence() {
    let home = Home::new("scoped-derivation");
    home.top("lead", &[user("lead", ts(1, 0), "old evidence")]);
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    let input = crate::inputs::Input {
        root: crate::inputs::InputRoot::Claude,
        path: "projects/-work-proj/lead.jsonl".to_owned(),
    };
    let path = input.full_path(&home.options);
    crate::model::AFTER_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || fs::remove_file(path).unwrap()));
    });
    assert!(crate::model::prepare_sources(&home.options, &mut cache, &[input], NOW).is_err());
    assert_eq!(catalog_generation(&home), None);
}

#[test]
fn query_producer_rejects_source_changes_after_indexing_before_publication() {
    use std::io::Write;
    for mutation in 0..3 {
        let home = Home::new("producer-race");
        home.top("lead", &[user("lead", ts(1, 0), "old evidence")]);
        let mut query = Query::from_sources(admitted(home.options.clone()));
        query.call_at("list_sessions", &json!({}), NOW).unwrap();
        let generation = catalog_generation(&home);
        assert!(generation.is_some());
        let path = home
            .options
            .claude_home
            .join("projects/-work-proj/lead.jsonl");
        writeln!(
            fs::OpenOptions::new().append(true).open(&path).unwrap(),
            "{}",
            assistant("lead", ts(1, 1), vec![text("appended answer")])
        )
        .unwrap();
        crate::model::AFTER_SCAN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let before = fs::metadata(&path).unwrap();
                let bytes = fs::read_to_string(&path).unwrap();
                assert!(bytes.contains("old evidence"));
                // Distinguish the mutation on filesystems with coarse ctime.
                std::thread::sleep(Duration::from_millis(2));
                match mutation {
                    0 => fs::write(&path, bytes.replace("old evidence", "new evidence")).unwrap(),
                    1 => {
                        let replacement = path.with_extension("replacement");
                        fs::write(&replacement, bytes).unwrap();
                        fs::rename(replacement, &path).unwrap();
                    }
                    _ => fs::remove_file(&path).unwrap(),
                }
                if mutation < 2 {
                    fs::File::options()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_modified(before.modified().unwrap())
                        .unwrap();
                }
            }));
        });
        let result = query.call_at("list_sessions", &json!({}), NOW);
        crate::model::AFTER_SCAN.with(|hook| assert!(hook.borrow().is_none(), "race must run"));
        assert!(
            result.is_err(),
            "mutation {mutation} must reject the cohort"
        );
        assert_eq!(result.unwrap_err().code, "io");
        assert_eq!(catalog_generation(&home), generation);
    }
}

#[test]
fn query_producer_rechecks_cached_and_unread_sources_before_publication() {
    use std::io::Write;
    for mutation in 0..2 {
        let home = Home::new("producer-completion");
        home.top("lead", &[user("lead", ts(1, 0), "old evidence")]);
        // A source with no body derivation still contributes native identity.
        home.top("empty", &[]);
        let mut query = Query::from_sources(admitted(home.options.clone()));
        query
            .call_at("get_session", &json!({"id":"lead"}), NOW)
            .unwrap();
        let generation = catalog_generation(&home);
        let path = home
            .options
            .claude_home
            .join("projects/-work-proj/lead.jsonl");
        writeln!(
            fs::OpenOptions::new().append(true).open(&path).unwrap(),
            "{}",
            assistant("lead", ts(1, 1), vec![text("appended answer")])
        )
        .unwrap();
        let empty = home
            .options
            .claude_home
            .join("projects/-work-proj/empty.jsonl");
        crate::model::AFTER_QUERY_DERIVATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                if mutation == 0 {
                    let before = fs::metadata(&path).unwrap();
                    let bytes = fs::read_to_string(&path).unwrap();
                    fs::write(&path, bytes.replace("old evidence", "new evidence")).unwrap();
                    fs::File::options()
                        .write(true)
                        .open(path)
                        .unwrap()
                        .set_modified(before.modified().unwrap())
                        .unwrap();
                } else {
                    fs::remove_file(empty).unwrap();
                }
            }));
        });
        assert_eq!(
            query
                .call_at("get_session", &json!({"id":"lead"}), NOW)
                .unwrap_err()
                .code,
            "io"
        );
        assert_eq!(catalog_generation(&home), generation);
        // A failed derivation is not installed as the next query snapshot.
        let recovered = query
            .call_at("get_session", &json!({"id":"lead"}), NOW)
            .unwrap();
        if mutation == 0 {
            assert!(recovered.to_string().contains("new evidence"));
        }
    }
}

#[test]
fn query_producer_allows_verified_append_without_extending_the_cohort() {
    use std::io::Write;
    let home = Home::new("producer-append");
    home.top("lead", &[user("lead", ts(1, 0), "old evidence")]);
    let before = home
        .query()
        .call_at("get_session", &json!({"id":"lead"}), NOW)
        .unwrap();
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    crate::model::AFTER_SCAN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            writeln!(
                fs::OpenOptions::new().append(true).open(path).unwrap(),
                "{}",
                assistant("lead", ts(1, 1), vec![text("appended answer")])
            )
            .unwrap();
        }));
    });
    let mut query = home.query();
    assert_eq!(
        query
            .call_at("get_session", &json!({"id":"lead"}), NOW)
            .unwrap(),
        before
    );
    assert_ne!(
        query
            .call_at("get_session", &json!({"id":"lead"}), NOW)
            .unwrap(),
        before
    );
}

#[test]
fn query_producer_preserves_large_native_tool_record_derivation() {
    crate::model::DIRECT_SOURCE_READS.with(|reads| reads.set(0));
    let home = Home::new("producer-large-record");
    let mut record = assistant(
        "lead",
        ts(1, 0),
        vec![tool(
            "t1",
            "Agent",
            json!({"description":"Scan parser", "prompt":"Find parser panics", "subagent_type":"general-purpose"}),
        )],
    );
    home.top(
        "lead",
        &[user("lead", ts(0, 0), "Fix parser"), record.clone()],
    );
    home.agent(
        "lead",
        "scan",
        "t1",
        &[user("scan", ts(1, 1), "Find parser panics")],
    );
    let expected = home
        .query()
        .call_at("get_session", &json!({"id":"lead"}), NOW)
        .unwrap();
    record["padding"] = json!("x".repeat(crate::tx::LINE_MAX as usize + 1024));
    home.top("lead", &[user("lead", ts(0, 0), "Fix parser"), record]);
    let answer = home
        .query()
        .call_at("get_session", &json!({"id":"lead"}), NOW)
        .unwrap();
    assert!(answer.to_string().contains("Find parser panics"));
    assert_eq!(answer, expected);
    crate::model::DIRECT_SOURCE_READS.with(|reads| assert_eq!(reads.get(), 0));
    // Full Model preparation now shares the verified native body reader too.
    assert_eq!(
        ViewerCore::new(home.options.clone())
            .respond("GET", "/api/model", "", None)
            .status,
        200
    );
    crate::model::DIRECT_SOURCE_READS.with(|reads| assert_eq!(reads.get(), 0));
}

/// A retired native cohort cannot be refreshed after the source mutation.
/// This also reproduces an active call racing a writer after its index build.
fn retained_query(home: &Home) -> Query {
    let sources = admitted(home.options.clone());
    let mut query = Query::from_sources(sources.clone());
    query.call_at("list_sessions", &json!({}), NOW).unwrap();
    sources.close();
    query
}

#[test]
fn metadata_and_selected_session_reads_do_not_hydrate_unrelated_prompt_history() {
    for count in [4, 128] {
        let home = Home::new("prompt-read-scope");
        home.top(
            "selected",
            &[human("selected", ts(1, 0), "selected evidence")],
        );
        let mut history = vec![human("history", ts(1, 0), "history title")];
        history.extend((0..count).map(|_| human("history", ts(1, 1), &"x".repeat(64 * 1024))));
        home.top("history", &history);
        let path = home
            .options
            .claude_home
            .join("projects/-work-proj/history.jsonl");
        let original = fs::read(&path).unwrap();
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
        // Indexing native records still reads their evidence. Once indexed,
        // metadata observation and an unrelated selected session must not read
        // every old prompt again to populate unused handoff briefs.
        let before = crate::source_chunks::READ_BYTES.with(std::cell::Cell::get);
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
        let mut query = home.query();
        assert_eq!(
            query.call_at("list_sessions", &json!({}), NOW).unwrap()["sessions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        query
            .call_at("stalls", &json!({"idle_minutes":10}), NOW)
            .unwrap();
        let selected = query
            .call_at("get_session", &json!({"id":"selected"}), NOW)
            .unwrap();
        assert_eq!(
            selected["session"]["handoffs"][0]["brief"],
            "selected evidence"
        );
        let bytes = crate::source_chunks::READ_BYTES.with(std::cell::Cell::get) - before;
        println!("{count} unrelated prompts: {bytes} native body/proof bytes");
        assert!(
            bytes <= 12 * crate::source_chunks::SIZE as u64,
            "unrelated history hydrated: {bytes} bytes"
        );
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn limited_search_reads_prompt_briefs_only_when_their_slots_are_visited() {
    let home = Home::new("prompt-search-scope");
    let mut history = vec![human("lead", ts(1, 0), "history title")];
    history.extend((0..128).map(|_| human("lead", ts(1, 1), &"x".repeat(64 * 1024))));
    history.push(human("lead", ts(1, 2), "selected needle"));
    home.top("lead", &history);
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let mut query = home.query();
    let before = crate::source_chunks::READ_BYTES.with(std::cell::Cell::get);
    let result = query
        .call_at("find", &json!({"text":"selected needle", "limit":1}), NOW)
        .unwrap();
    assert_eq!(result["matches"].as_array().unwrap().len(), 1);
    assert_eq!(result["matches"][0]["part"], "brief");
    assert_eq!(result["matches"][0]["snippet"], "selected needle");
    assert_eq!(result["truncated_by"], "limit");
    let bytes = crate::source_chunks::READ_BYTES.with(std::cell::Cell::get) - before;
    println!("limited search over 128 old prompts: {bytes} native body/proof bytes");
    assert!(
        bytes <= 4 * crate::source_chunks::SIZE as u64,
        "search hydrated unvisited prompts: {bytes} bytes"
    );
}

#[test]
fn source_backed_prompt_briefs_preserve_native_caps_and_completion_fences() {
    let _time = crate::events::fixed_change_time_for_test(0);
    let home = Home::new("prompt-brief-integrity");
    home.top("lead", &[human("lead", ts(1, 0), &"é".repeat(50_000))]);
    let sources = admitted(home.options.clone());
    let core = ViewerCore::from_sources(sources.clone());
    let model: Value =
        serde_json::from_slice(&core.respond("GET", "/api/model", "", None).body).unwrap();
    let mut query = Query::from_sources(sources.clone());
    let selected = query
        .call_at("get_session", &json!({"id":"lead"}), NOW)
        .unwrap();
    assert_eq!(
        selected["session"]["handoffs"][0]["brief"],
        model["handoffs"][0]["brief"]
    );
    sources.close();
    let data = &query.evidence().parts[0].data;
    let records = data.sources.reader();
    let brief = data.ask_briefs.values().next().unwrap();
    assert_eq!(brief.read(&records).unwrap(), model["handoffs"][0]["brief"]);
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    let metadata = fs::metadata(&path).unwrap();
    let stat = crate::events::Stat::from_metadata(&metadata).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    fs::write(&path, raw.replacen('é', "ê", 1)).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(metadata.modified().unwrap())
        .unwrap();
    assert_eq!(
        crate::events::Stat::from_metadata(&fs::metadata(&path).unwrap()).unwrap(),
        stat
    );
    assert!(
        records.finish().is_err(),
        "completion must reject changed cached prompt bytes"
    );
    for (name, args) in [
        ("get_session", json!({"id":"lead"})),
        ("find", json!({"text":"é"})),
    ] {
        assert_eq!(query.call_at(name, &args, NOW).unwrap_err().code, "io");
    }
}

#[test]
fn native_projection_reuse_rebuilds_unqualified_and_corrupt_recipes_from_evidence() {
    for mutation in [
        "UPDATE session_slot_projections SET version=version-1",
        "UPDATE session_slot_projections SET source_generation='wrong'",
        "UPDATE session_slot_projections SET generation='wrong'",
        "UPDATE meta SET value='wrong' WHERE key='slot_projection_catalog_generation'",
        "DELETE FROM session_slots WHERE slot=0",
        "UPDATE session_slots SET metadata=json_set(metadata,'$.native_event_id','wrong')",
        "UPDATE session_slots SET metadata='{'",
    ] {
        let home = Home::new("projection-repair");
        home.top(
            "lead",
            &[
                human("lead", ts(1, 0), "native evidence"),
                human("lead", ts(1, 1), "later evidence"),
            ],
        );
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        let observe = |cache: &mut crate::events::EventCache| {
            crate::model::observe_catalog(
                &home.options,
                cache,
                &mut false,
                &mut crate::model::Texts::default(),
                NOW,
            )
            .unwrap();
        };
        observe(&mut cache);
        let db = rusqlite::Connection::open(crate::events::EventCache::path(&home.options.cache))
            .unwrap();
        let slots = || {
            db.prepare(
                "SELECT session_key,slot,metadata FROM session_slots ORDER BY session_key,slot",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        };
        let original = slots();
        assert!(!original.is_empty());
        let before = home
            .query()
            .call_at("get_session", &json!({"id":"lead"}), NOW)
            .unwrap();
        db.execute(mutation, []).unwrap();
        observe(&mut cache);
        assert_eq!(slots(), original, "failed to repair {mutation}");
        assert_eq!(
            home.query()
                .call_at("get_session", &json!({"id":"lead"}), NOW)
                .unwrap(),
            before
        );
    }
}

#[test]
fn native_tools_refuse_changed_bytes_even_with_indistinguishable_file_stamps() {
    let _time = crate::events::fixed_change_time_for_test(0);
    let home = Home::new("coarse-native-generation");
    home.top(
        "lead",
        &[assistant("lead", ts(1, 0), vec![text("old evidence")])],
    );
    let mut query = retained_query(&home);
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    let before = fs::metadata(&path).unwrap();
    let stamp = crate::events::Stat::from_metadata(&before).unwrap();
    let bytes = fs::read_to_string(&path).unwrap();
    fs::write(&path, bytes.replace("old evidence", "new evidence")).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    assert_eq!(
        crate::events::Stat::from_metadata(&fs::metadata(&path).unwrap()).unwrap(),
        stamp
    );
    for (tool, args) in [
        ("read_transcript", json!({"id":"lead"})),
        ("find", json!({"text":"new evidence"})),
    ] {
        assert_eq!(query.call_at(tool, &args, NOW).unwrap_err().code, "io");
    }
}

#[test]
fn query_source_reads_allow_append_without_reading_beyond_the_cohort() {
    use std::io::Write;
    let home = fixture();
    let mut query = retained_query(&home);
    let args = json!({"id":"lead"});
    let before = query.call_at("read_transcript", &args, NOW).unwrap();
    let search = json!({"text":"new evidence"});
    let missing = query.call_at("find", &search, NOW).unwrap();
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        file,
        "{}",
        assistant("lead", ts(2, 0), vec![text("new evidence")])
    )
    .unwrap();
    assert_eq!(
        query.call_at("read_transcript", &args, NOW).unwrap(),
        before
    );
    assert_eq!(query.call_at("find", &search, NOW).unwrap(), missing);
    assert!(
        !home.query().call_at("find", &search, NOW).unwrap()["matches"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn query_source_reads_reject_interior_rewrites_even_with_restored_mtime() {
    let home = Home::new("rewritten");
    home.top(
        "lead",
        &[assistant(
            "lead",
            ts(1, 0),
            vec![text(
                &("x".repeat(5000) + "old evidence" + &"y".repeat(5000)),
            )],
        )],
    );
    let mut query = retained_query(&home);
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    let before = fs::metadata(&path).unwrap();
    let bytes = fs::read_to_string(&path).unwrap();
    fs::write(&path, bytes.replace("old evidence", "new evidence")).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), before.len());
    assert_eq!(
        query
            .call_at("read_transcript", &json!({"id":"lead"}), NOW)
            .unwrap_err()
            .code,
        "io"
    );
    assert_eq!(
        query
            .call_at("find", &json!({"text":"new evidence"}), NOW)
            .unwrap_err()
            .code,
        "io"
    );
}

#[test]
fn query_source_reads_reject_replacement_and_deletion() {
    for delete in [false, true] {
        let home = fixture();
        let mut query = retained_query(&home);
        let path = home
            .options
            .claude_home
            .join("projects/-work-proj/lead.jsonl");
        let before = fs::metadata(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        // Keep the original inode alive to avoid inode reuse in the fixture.
        fs::rename(&path, path.with_extension("retired")).unwrap();
        if !delete {
            fs::write(&path, bytes).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(before.modified().unwrap())
                .unwrap();
        }
        assert_eq!(
            query
                .call_at("read_transcript", &json!({"id":"lead"}), NOW)
                .unwrap_err()
                .code,
            "io"
        );
        assert_eq!(
            query
                .call_at("find", &json!({"text":"parser"}), NOW)
                .unwrap_err()
                .code,
            "io"
        );
    }
}

#[test]
fn query_source_paging_checks_only_touched_sources_and_rechecks_cached_lines() {
    let home = fixture();
    let mut query = retained_query(&home);
    let before = query
        .call_at("read_transcript", &json!({"id":"lead"}), NOW)
        .unwrap();
    fs::remove_file(
        home.options
            .claude_home
            .join("projects/-work-proj/done.jsonl"),
    )
    .unwrap();
    assert_eq!(
        query
            .call_at("read_transcript", &json!({"id":"lead"}), NOW)
            .unwrap(),
        before
    );

    let data = &query.evidence().parts[0].data;
    let records = data.sources.reader();
    let mut read = |path: &std::path::Path, offset| records.read(path, offset);
    let mut lines = tx::Lines::provider(&mut read);
    let slot = data.tx["lead"]
        .slots
        .iter()
        .find(|slot| matches!(slot.kind, SlotKind::A))
        .unwrap();
    let texts = tx::slot_texts(&mut lines, slot);
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    let bytes = fs::read_to_string(&path).unwrap();
    fs::write(&path, bytes.replace("On it.", "Oh no.")).unwrap();
    assert_eq!(
        tx::slot_texts(&mut lines, slot),
        texts,
        "exercise the pager's native record cache"
    );
    assert!(
        records.finish().is_err(),
        "cached bytes cannot hide a changed source at completion"
    );
}

#[test]
fn query_source_paging_preserves_large_prompts_and_cached_image_ranges() {
    let home = Home::new("images");
    let mut png = vec![0xFF; 4 * 1024 * 1024];
    png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let encoded = crate::attachments::tests::encode(&png);
    let record = json!({"type":"user","sessionId":"images","timestamp":ts(1,0),"message":{"role":"user","content":[
        text("Inspect both images"),
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":encoded}},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":encoded}},
    ]}});
    let raw = record.to_string().replace('/', "\\/") + "\n";
    assert!(raw.len() as u64 > tx::LINE_MAX);
    home.write("claude/projects/-work-proj/images.jsonl", &raw);
    let sources = admitted(home.options.clone());
    // Independently captured from the retired served pager at 3b2e549.
    // Pin both range versions and the full five-tool transcript response.
    let expected = json!({"sid":"images","from":0,"to":1,"total":1,"calls":0,"errors":0,
        "entries":[{"k":"u","text":"Inspect both images","img":[
            {"b":1,"type":"image/png","size":4194304,"v":"dadd81fa9dd82382","o":0},
            {"b":2,"type":"image/png","size":4194304,"v":"dadd81fa9dd82382","o":0}
        ],"turn":"images:images:0"}],"window_start":null});
    assert_eq!(expected["entries"][0]["img"].as_array().unwrap().len(), 2);
    let mut query = Query::from_sources(sources);
    let before = crate::model::COMPAT_BUILDS.with(|count| count.get());
    let actual = query
        .call_at("read_transcript", &json!({"id":"images"}), NOW)
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        crate::model::COMPAT_BUILDS.with(|count| count.get()),
        before
    );
    let data = &query.evidence().parts[0].data;
    let slot = data.tx["images"]
        .slots
        .iter()
        .find(|slot| slot.file.is_some())
        .unwrap();
    let records = data.sources.reader();
    let (prompt, bytes) = records
        .prompt(slot.file.as_deref().unwrap(), slot.offset)
        .unwrap();
    assert!(
        bytes < 1024,
        "authenticated prompt reads must retain image-range elision"
    );
    assert_eq!(
        prompt["message"]["content"][0]["text"],
        "Inspect both images"
    );
    assert_eq!(prompt["message"]["content"][1]["source"]["data"], "");
    records.finish().unwrap();
}

#[test]
fn query_source_reads_preserve_log_sealing_without_rebuilding_the_cohort() {
    let home = Home::new("sealed");
    home.top(
        "lead",
        &[assistant(
            "lead",
            ts(1, 0),
            vec![text(&"source evidence ".repeat(1000))],
        )],
    );
    let mut query = retained_query(&home);
    let args = json!({"id":"lead"});
    let before = query.call_at("read_transcript", &args, NOW).unwrap();
    let search = json!({"text":"source evidence"});
    let found = query.call_at("find", &search, NOW).unwrap();
    let path = home
        .options
        .claude_home
        .join("projects/-work-proj/lead.jsonl");
    assert!(
        crate::sealed::seal(&path, 1, u64::MAX)
            .unwrap()
            .added
            .is_some()
    );
    assert_eq!(
        query.call_at("read_transcript", &args, NOW).unwrap(),
        before
    );
    assert_eq!(query.call_at("find", &search, NOW).unwrap(), found);
}

/// 2026-09-24T00:00:00Z.
const BASE: i64 = 1_790_208_000_000;
/// A day later: every session in the home has been quiet for hours.
const NOW: i64 = BASE + 86_400_000;

fn at(hour: i64, minute: i64) -> i64 {
    BASE + (hour * 60 + minute) * 60_000
}

fn ts(hour: i64, minute: i64) -> String {
    format!("2026-09-24T{hour:02}:{minute:02}:00.000Z")
}

struct Home {
    root: PathBuf,
    options: Options,
}

impl Home {
    fn new(host: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "semon-query-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let options = Options {
            claude_home: root.join("claude"),
            claude_json: root.join(".claude.json"),
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
        let home = Self { root, options };
        home.write("proc/locks", "");
        home.write("proc/sys/kernel/hostname", &format!("{host}\n"));
        home
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn lines(&self, relative: &str, records: &[Value]) -> PathBuf {
        let text: String = records.iter().map(|record| format!("{record}\n")).collect();
        self.write(relative, &text)
    }

    fn top(&self, id: &str, records: &[Value]) {
        self.lines(&format!("claude/projects/-work-proj/{id}.jsonl"), records);
    }

    fn agent(&self, parent: &str, agent: &str, tool_use: &str, records: &[Value]) {
        self.lines(
            &format!("claude/projects/-work-proj/{parent}/subagents/agent-{agent}.jsonl"),
            records,
        );
        self.write(
            &format!("claude/projects/-work-proj/{parent}/subagents/agent-{agent}.meta.json"),
            &json!({"agentType": "general-purpose", "description": format!("task {agent}"), "toolUseId": tool_use})
                .to_string(),
        );
    }

    /// A process whose start time (`/proc/<pid>/stat` field 22) is `start`.
    fn process(&self, pid: u32, start: u64) {
        let mut fields = vec!["0".to_owned(); 20];
        fields[19] = start.to_string();
        self.write(
            &format!("proc/{pid}/stat"),
            &format!("{pid} (claude) {}\n", fields.join(" ")),
        );
    }

    /// Claude's `sessions/<pid>.json` for `session`, recording `start`.
    fn pid_file(&self, pid: u32, session: &str, status: &str, start: u64) {
        self.write(
            &format!("claude/sessions/{pid}.json"),
            &json!({"pid": pid, "sessionId": session, "procStart": start, "status": status, "name": format!("{session}-lane")})
                .to_string(),
        );
    }

    fn query(&self) -> Query {
        Query::new(self.options.clone())
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn human(sid: &str, time: String, text: &str) -> Value {
    json!({"type": "user", "timestamp": time, "sessionId": sid, "cwd": "/work/proj", "origin": {"kind": "human"}, "message": {"role": "user", "content": text}})
}

fn user(sid: &str, time: String, text: &str) -> Value {
    json!({"type": "user", "timestamp": time, "sessionId": sid, "cwd": "/work/proj", "message": {"role": "user", "content": text}})
}

fn assistant(sid: &str, time: String, blocks: Vec<Value>) -> Value {
    json!({"type": "assistant", "timestamp": time.clone(), "sessionId": sid, "cwd": "/work/proj",
        "message": {"id": format!("m-{sid}-{time}"), "model": "claude-opus-5-5", "role": "assistant", "content": blocks,
            "usage": {"input_tokens": 1000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 2000, "output_tokens": 500}}})
}

fn text(value: &str) -> Value {
    json!({"type": "text", "text": value})
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

fn codex_line(time: String, kind: &str, payload: Value) -> Value {
    json!({"timestamp": time, "type": kind, "payload": payload})
}

/// One session per state: `lead` works (and its subagent `scan` with it),
/// `asker` waits on a question, `crashed`'s process died while its pid file
/// said busy, `done` ended, and the Codex run `cx` ended.
fn fixture() -> Home {
    let home = Home::new("testbox");
    home.top(
        "lead",
        &[
            human("lead", ts(1, 0), "Port the parser"),
            assistant("lead", ts(1, 1), vec![text("On it.")]),
            assistant(
                "lead",
                ts(1, 2),
                vec![tool(
                    "t1",
                    "Agent",
                    json!({"description": "scan", "prompt": "Scan the parser for panics"}),
                )],
            ),
            assistant(
                "lead",
                ts(1, 3),
                vec![tool(
                    "b1",
                    "Bash",
                    json!({"command": "cargo test -p parser"}),
                )],
            ),
        ],
    );
    home.process(30, 777);
    home.pid_file(30, "lead", "busy", 777);
    home.agent(
        "lead",
        "scan",
        "t1",
        &[
            user("scan", ts(1, 2), "Scan the parser for panics"),
            assistant("scan", ts(1, 4), vec![text("Found one in lexer.rs")]),
        ],
    );
    home.top(
        "asker",
        &[
            human("asker", ts(2, 0), "Pick a name"),
            assistant(
                "asker",
                ts(2, 1),
                vec![tool(
                    "q1",
                    "AskUserQuestion",
                    json!({"questions": [{"question": "Which name?", "header": "Name", "multiSelect": false,
                        "options": [{"label": "semon"}, {"label": "ostrom"}]}]}),
                )],
            ),
        ],
    );
    home.process(31, 777);
    home.pid_file(31, "asker", "idle", 777);
    home.top(
        "crashed",
        &[
            human("crashed", ts(3, 0), "Refactor the store"),
            assistant(
                "crashed",
                ts(3, 1),
                vec![tool("b2", "Bash", json!({"command": "cargo build"}))],
            ),
        ],
    );
    // Its pid file says busy, but no process 32 runs.
    home.pid_file(32, "crashed", "busy", 555);
    home.top(
        "done",
        &[
            human("done", ts(4, 0), "Summarize the design doc"),
            assistant(
                "done",
                ts(4, 1),
                vec![text("The design keeps the frontend and replaces the data.")],
            ),
        ],
    );
    home.lines(
        "codex/sessions/2026/09/24/rollout-cx.jsonl",
        &[
            codex_line(
                ts(5, 0),
                "session_meta",
                json!({"id": "cx", "cwd": "/work/proj", "originator": "codex_exec", "thread_source": "user"}),
            ),
            codex_line(
                ts(5, 1),
                "response_item",
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Implement the lexer fix"}]}),
            ),
            codex_line(
                ts(5, 2),
                "response_item",
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Fixed the lexer panic"}]}),
            ),
        ],
    );
    home
}

/// The open question's handoff id and each session's first turn id, from
/// independent captured evidence: they are hashes of source ids.
fn question_id(model: &Value) -> String {
    model["handoffs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|handoff| handoff["kind"] == "toyou" && handoff["from"] == "asker")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn turn_id(model: &Value, sid: &str) -> String {
    model["turns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|turn| turn["sid"] == sid)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Every session's summary, pinned.
fn summary(id: &str, question: &str) -> Value {
    let tokens = json!({"input": 1000, "cached": 2000, "output": 500});
    let base = json!({
        "id": id, "harness": "claude", "kind": "session", "model": "opus-5.5", "repo": null, "branch": null,
        "machine": "testbox", "parent": null, "children": [], "pid": null, "alive": null, "exit": null,
        "open_question": null, "tokens": tokens, "run": null, "turns_truncated": false,
    });
    let fields = match id {
        "lead" => {
            json!({"name": "lead-lane", "children": ["scan"], "state": "working", "start": at(1, 0),
            "last_activity": at(1, 3), "pid": 30, "alive": true,
            "tokens": {"input": 3000, "cached": 6000, "output": 1500}})
        }
        "scan" => {
            json!({"kind": "subagent", "name": "task scan", "parent": "lead", "state": "working",
            "start": at(1, 2), "last_activity": at(1, 4)})
        }
        "asker" => json!({"name": "asker-lane", "state": "waiting-question", "start": at(2, 0),
            "last_activity": at(2, 1), "pid": 31, "alive": true,
            "open_question": {"text": "Which name?", "asked_at": at(2, 1), "handoff": question}}),
        "crashed" => {
            json!({"name": "proj", "state": "ended", "start": at(3, 0), "last_activity": at(3, 1),
            "pid": 32, "alive": false})
        }
        "done" => {
            json!({"name": "proj", "state": "ended", "start": at(4, 0), "last_activity": at(4, 1)})
        }
        "cx" => {
            json!({"harness": "codex", "kind": "codex-run", "name": "Codex run", "model": "codex",
            "state": "ended", "start": at(5, 0), "last_activity": at(5, 2),
            "tokens": {"input": 0, "cached": 0, "output": 0}})
        }
        _ => panic!("no session {id}"),
    };
    let mut out = base;
    for (key, value) in fields.as_object().unwrap() {
        out[key] = value.clone();
    }
    out
}

fn call(query: &mut Query, tool: &str, args: Value) -> Value {
    query
        .call_at(tool, &args, NOW)
        .unwrap_or_else(|error| panic!("{tool} {args}: {error}"))
}

fn failure(query: &mut Query, tool: &str, args: Value) -> &'static str {
    match query.call_at(tool, &args, NOW) {
        Ok(value) => panic!("{tool} {args} succeeded: {value}"),
        Err(error) => error.code,
    }
}

#[test]
fn list_sessions_summarizes_every_session_newest_first() {
    let home = fixture();
    let question = question_id(&fixture_relation_reference());
    let mut query = home.query();
    let all = call(&mut query, "list_sessions", json!({}));
    let order = ["cx", "done", "crashed", "asker", "scan", "lead"];
    assert_eq!(
        all,
        json!({
            "now": NOW,
            "window_start": null,
            "total": 6,
            "truncated": false,
            "sessions": order.iter().map(|id| summary(id, &question)).collect::<Vec<_>>(),
        })
    );
    let mut ids = |args: Value| -> Vec<String> {
        call(&mut query, "list_sessions", args)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(json!({"state": "working"})), ["scan", "lead"]);
    assert_eq!(ids(json!({"state": "waiting-question"})), ["asker"]);
    assert_eq!(ids(json!({"state": "ended"})), ["cx", "done", "crashed"]);
    assert!(ids(json!({"state": "idle"})).is_empty());
    assert_eq!(ids(json!({"harness": "codex"})), ["cx"]);
    assert_eq!(ids(json!({"parent": "lead"})), ["scan"]);
    assert!(ids(json!({"parent": "scan"})).is_empty());
    assert!(ids(json!({"repo": "semon"})).is_empty());
    assert_eq!(
        ids(json!({"since": "2026-09-24T03:00:00Z"})),
        ["cx", "done", "crashed"]
    );
    // 20 hours before NOW is 04:00.
    assert_eq!(ids(json!({"since": "20h"})), ["cx", "done"]);
    let limited = call(&mut query, "list_sessions", json!({"limit": 2}));
    assert_eq!(
        (&limited["total"], &limited["truncated"]),
        (&json!(6), &json!(true))
    );
    assert_eq!(limited["sessions"].as_array().unwrap().len(), 2);
    let empty = call(&mut query, "list_sessions", json!({"state": "error"}));
    assert_eq!(
        empty,
        json!({"now": NOW, "window_start": null, "total": 0, "truncated": false, "sessions": []})
    );
    assert_eq!(
        failure(&mut query, "list_sessions", json!({"parent": "nobody"})),
        "unknown_session"
    );
}

#[test]
fn get_session_adds_busy_turns_and_handoffs_from_native_evidence() {
    let home = fixture();
    let model = fixture_relation_reference();
    let question = question_id(&model);
    let mut query = home.query();
    let got = call(&mut query, "get_session", json!({"id": "asker"}));
    let of = |list: &str, keep: &dyn Fn(&Value) -> bool| -> Value {
        Value::Array(
            model[list]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| keep(item))
                .cloned()
                .collect(),
        )
    };
    let turns = of("turns", &|turn| turn["sid"] == "asker");
    let handoffs = of("handoffs", &|handoff| {
        handoff["from"] == "asker" || handoff["to"] == "asker"
    });
    let mut expected = summary("asker", &question);
    expected["busy"] = json!([[at(2, 0), at(2, 1)]]);
    expected["turns"] = turns.clone();
    expected["handoffs"] = handoffs.clone();
    assert_eq!(
        got,
        json!({"now": NOW, "window_start": null, "session": expected})
    );
    // The independently captured turn and handoffs: your message, then the question.
    assert_eq!(turns.as_array().unwrap().len(), 1);
    assert_eq!(turns[0]["end"]["why"], "toyou");
    let kinds: Vec<&str> = handoffs
        .as_array()
        .unwrap()
        .iter()
        .map(|handoff| handoff["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["ask", "toyou"]);
    assert_eq!(handoffs[1]["status"], "wait");

    let lead = call(&mut query, "get_session", json!({"id": "lead"}));
    assert_eq!(lead["session"]["children"], json!(["scan"]));
    let spawn = lead["session"]["handoffs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|handoff| handoff["kind"] == "spawn")
        .unwrap();
    assert_eq!(
        (&spawn["from"], &spawn["to"]),
        (&json!("lead"), &json!("scan"))
    );
    assert_eq!(
        failure(&mut query, "get_session", json!({"id": "nobody"})),
        "unknown_session"
    );
}

#[test]
fn read_transcript_pages_as_the_viewer_does() {
    let home = fixture();
    let model = fixture_relation_reference();
    let ask = turn_id(&model, "done");
    let mut query = home.query();
    let page = call(&mut query, "read_transcript", json!({"id": "done"}));
    assert_eq!(
        page,
        json!({"sid": "done", "from": 0, "to": 2, "total": 2, "calls": 0, "errors": 0, "entries": [
            {"k": "h", "id": ask, "turn": ask},
            {"k": "a", "text": "The design keeps the frontend and replaces the data."},
        ], "window_start": null})
    );
    let sources = admitted(home.options.clone());
    let evidence = &sources.machines[0].1;
    let mut complete = false;
    {
        let mut work = evidence.work.lock().unwrap();
        for _ in 0..16 {
            if evidence.refresh_catalog(&mut work, NOW).unwrap() {
                complete = true;
                break;
            }
        }
    }
    assert!(
        complete,
        "native fixture Catalog publication did not complete"
    );
    // Selected Viewer reads expose the same entries and paging positions,
    // together with generation-bound native provenance.
    for args in [
        json!({"id": "done"}),
        json!({"id": "scan"}),
        json!({"id": "done", "after": 1}),
        json!({"id": "done", "turn": ask}),
    ] {
        let ours = call(&mut query, "read_transcript", args.clone());
        let sid = args["id"].as_str().unwrap();
        let mut url = format!("sid={sid}&after={}&limit=100", ours["from"]);
        if let Some(turn) = args["turn"].as_str() {
            let anchor = crate::session_turn_anchor(
                &home.options,
                "fixture",
                &format!("sid={sid}&turn={turn}"),
            );
            assert_eq!(anchor.status, 200);
            let anchor: Value = serde_json::from_slice(&anchor.body).unwrap();
            assert_eq!(anchor["slot"], ours["from"]);
            url.push_str(&format!(
                "&generation={}",
                anchor["generation"].as_str().unwrap()
            ));
        }
        let served = crate::session_transcript_range(&home.options, "fixture", &url, None);
        assert_eq!(served.status, 200, "{url}");
        let served: Value = serde_json::from_slice(&served.body).unwrap();
        assert_eq!(served["api"], 1);
        assert_eq!(served["range"]["first"], ours["from"]);
        assert_eq!(served["range"]["end"], ours["to"]);
        assert_eq!(served["projection"]["total"], ours["total"]);
        assert_eq!(served["freshness"]["state"], "cached");
        let mut entries = served["entries"].clone();
        for (offset, entry) in entries.as_array_mut().unwrap().iter_mut().enumerate() {
            assert_eq!(
                entry["slot"].as_u64().unwrap(),
                ours["from"].as_u64().unwrap() + offset as u64
            );
            assert_eq!(entry["entry_id"].as_str().unwrap().len(), 64);
            assert!(entry["provenance"].is_object());
            assert_eq!(entry["freshness"]["state"], "cached");
            if entry.get("field").is_some() {
                assert_eq!(entry["field"]["name"], "text");
                assert_eq!(entry["field"]["complete"], true);
                assert_eq!(entry["clipped"], false);
                entry.as_object_mut().unwrap().remove("field");
                entry.as_object_mut().unwrap().remove("clipped");
            }
            let expected = &ours["entries"][offset];
            if expected.get("turn").is_none() {
                assert_eq!(entry["turn"], turn_id(&model, sid));
                entry.as_object_mut().unwrap().remove("turn");
            }
            if entry["k"] == "h" && sid == "done" {
                assert_eq!(entry["text"], "Summarize the design doc");
                entry.as_object_mut().unwrap().remove("text");
            }
            for field in ["slot", "observed_at", "freshness", "entry_id", "provenance"] {
                entry.as_object_mut().unwrap().remove(field);
            }
        }
        assert_eq!(entries, ours["entries"], "{url}");
    }
    let last = call(
        &mut query,
        "read_transcript",
        json!({"id": "done", "limit": 1}),
    );
    assert_eq!(
        (&last["from"], &last["to"], &last["entries"]),
        (
            &json!(1),
            &json!(2),
            &json!([{"k": "a", "text": "The design keeps the frontend and replaces the data.", "turn": ask}])
        )
    );
    let before = call(
        &mut query,
        "read_transcript",
        json!({"id": "done", "before": 1}),
    );
    assert_eq!(
        before["entries"],
        json!([{"k": "h", "id": ask, "turn": ask}])
    );
    assert_eq!(
        failure(&mut query, "read_transcript", json!({"id": "nobody"})),
        "unknown_session"
    );
    assert_eq!(
        failure(
            &mut query,
            "read_transcript",
            json!({"id": "done", "turn": "nope"})
        ),
        "unknown_turn"
    );
    assert_eq!(
        failure(
            &mut query,
            "read_transcript",
            json!({"id": "done", "before": 1, "after": 0})
        ),
        "invalid_arguments"
    );
}

#[test]
fn find_searches_entries_newest_first_within_its_bounds() {
    let home = fixture();
    let model = fixture_relation_reference();
    let (cx, scan) = (turn_id(&model, "cx"), turn_id(&model, "scan"));
    let mut query = home.query();
    let mut found = call(&mut query, "find", json!({"text": "LEXER"}));
    let scanned = found["scanned_bytes"].as_u64().unwrap();
    let written: u64 = ["claude/projects", "codex/sessions"]
        .iter()
        .flat_map(|dir| walk(&home.root.join(dir)))
        .map(|path| fs::metadata(path).unwrap().len())
        .sum();
    assert!(
        scanned > 0 && scanned <= written,
        "{scanned} of {written} bytes"
    );
    found["scanned_bytes"] = json!("counted");
    assert_eq!(
        found,
        json!({
            "now": NOW,
            "window_start": null,
            "text": "LEXER",
            "matches": [
                {"session": "cx", "position": 1, "turn": cx, "kind": "a", "part": "text", "at": at(5, 2),
                    "snippet": "Fixed the lexer panic"},
                {"session": "cx", "position": 0, "turn": cx, "kind": "u", "part": "text", "at": at(5, 1),
                    "snippet": "Implement the lexer fix"},
                {"session": "scan", "position": 1, "turn": scan, "kind": "a", "part": "text", "at": at(1, 4),
                    "snippet": "Found one in lexer.rs"},
            ],
            "sessions": [
                {"id": "cx", "name": "Codex run", "repo": null, "last_activity": at(5, 2), "fields": [], "entries": 2},
                {"id": "scan", "name": "task scan", "repo": null, "last_activity": at(1, 4), "fields": [], "entries": 1},
            ],
            "scanned_bytes": "counted",
            "truncated": false,
            "truncated_by": null,
        })
    );
    // Handoffs are searched by their text in the model; tool calls by
    // their input and output.
    let brief = call(&mut query, "find", json!({"text": "panics"}));
    let hits: Vec<(&str, &str, &str)> = brief["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| {
            (
                hit["session"].as_str().unwrap(),
                hit["kind"].as_str().unwrap(),
                hit["part"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        hits,
        [("scan", "h", "brief"), ("lead", "h", "brief")],
        "{brief}"
    );
    let command = call(&mut query, "find", json!({"text": "cargo build"}));
    assert_eq!(command["matches"][0]["kind"], "tool");
    assert_eq!(command["matches"][0]["name"], "Bash");
    assert_eq!(command["matches"][0]["part"], "in");
    // A session's name is matched too.
    let named = call(&mut query, "find", json!({"text": "task sc"}));
    assert_eq!(named["matches"], json!([]));
    assert_eq!(
        named["sessions"],
        json!([{"id": "scan", "name": "task scan", "repo": null, "last_activity": at(1, 4), "fields": ["name"], "entries": 0}])
    );
    // Bounds: a result limit, a byte budget, and since.
    let limited = call(&mut query, "find", json!({"text": "lexer", "limit": 1}));
    assert_eq!(limited["matches"].as_array().unwrap().len(), 1);
    assert_eq!(
        (&limited["truncated"], &limited["truncated_by"]),
        (&json!(true), &json!("limit"))
    );
    let budget = call(&mut query, "find", json!({"text": "lexer", "max_bytes": 1}));
    assert_eq!(budget["matches"].as_array().unwrap().len(), 1);
    assert_eq!(budget["truncated_by"], "bytes");
    let recent = call(
        &mut query,
        "find",
        json!({"text": "lexer", "since": "2026-09-24T05:01:30Z"}),
    );
    assert_eq!(recent["matches"].as_array().unwrap().len(), 1);
    assert_eq!(recent["matches"][0]["position"], 1);
    let none = call(&mut query, "find", json!({"text": "no such words"}));
    assert_eq!(
        (&none["matches"], &none["sessions"], &none["truncated"]),
        (&json!([]), &json!([]), &json!(false))
    );
    assert_eq!(
        failure(&mut query, "find", json!({"text": ""})),
        "invalid_arguments"
    );
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
    out
}

#[test]
fn stalls_give_one_closed_reason_per_session() {
    let home = fixture();
    let question = question_id(&fixture_relation_reference());
    let mut query = home.query();
    let stalls = call(&mut query, "stalls", json!({"idle_minutes": 60}));
    let row = |id: &str, reason: &str, last: i64| {
        let mut row = summary(id, &question);
        row["stall_reason"] = json!(reason);
        row["idle_ms"] = json!(NOW - last);
        row
    };
    assert_eq!(
        stalls,
        json!({
            "now": NOW,
            "window_start": null,
            "idle_minutes": 60,
            "rule": STALL_RULE,
            "stalls": [
                row("lead", "no-output", at(1, 3)),
                row("scan", "no-output", at(1, 4)),
                row("asker", "waiting-question", at(2, 1)),
                row("crashed", "process-gone", at(3, 1)),
            ],
        })
    );
    let recent = call(
        &mut query,
        "stalls",
        json!({"idle_minutes": 60, "since": "22h"}),
    );
    let ids: Vec<&str> = recent["stalls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["asker", "crashed"]);
    // Nothing has been quiet for a year.
    let none = call(&mut query, "stalls", json!({"idle_minutes": 525_600}));
    assert_eq!(none["stalls"], json!([]));
    assert_eq!(
        failure(&mut query, "stalls", json!({})),
        "invalid_arguments"
    );
}

#[test]
fn arguments_are_checked_against_the_schema() {
    let home = fixture();
    let mut query = home.query();
    assert_eq!(failure(&mut query, "nope", json!({})), "unknown_tool");
    for (tool, args) in [
        ("list_sessions", json!([])),
        ("list_sessions", json!({"state": "sleeping"})),
        ("list_sessions", json!({"limit": 0})),
        ("list_sessions", json!({"limit": "ten"})),
        ("list_sessions", json!({"since": "yesterday"})),
        ("list_sessions", json!({"bogus": 1})),
        ("get_session", json!({})),
        ("get_session", json!({"id": 5})),
        ("read_transcript", json!({"id": "done", "limit": 201})),
        ("stalls", json!({"idle_minutes": 1.5})),
    ] {
        assert_eq!(
            failure(&mut query, tool, args.clone()),
            "invalid_arguments",
            "{tool} {args}"
        );
    }
    // A null is an argument not given, and a whole number is an integer.
    let listed = call(
        &mut query,
        "list_sessions",
        json!({"state": null, "limit": 2.0}),
    );
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(call(&mut query, "list_sessions", Value::Null)["total"], 6);
}

#[test]
fn several_machines_answer_as_one_model() {
    let first = fixture();
    let second = Home::new("desktop");
    second.top(
        "other",
        &[
            human("other", ts(6, 0), "Review the lexer change"),
            assistant("other", ts(6, 1), vec![text("Looks right.")]),
        ],
    );
    let mut query = Query::with_machines(vec![
        ("a".into(), first.options.clone()),
        ("b".into(), second.options.clone()),
    ]);
    let listed = call(
        &mut query,
        "list_sessions",
        json!({"since": "2026-09-24T05:00:00Z"}),
    );
    let rows: Vec<(&str, &str)> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["machine"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("other", "desktop"), ("cx", "testbox")]);
    let page = call(&mut query, "read_transcript", json!({"id": "other"}));
    assert_eq!(page["entries"][1]["text"], "Looks right.");
    let found = call(&mut query, "find", json!({"text": "lexer change"}));
    assert_eq!(found["matches"][0]["session"], "other");
    assert_eq!(found["matches"][0]["part"], "brief");
    assert_eq!(
        failure(&mut query, "get_session", json!({"id": "nobody"})),
        "unknown_session"
    );
    assert_eq!(
        failure(&mut query, "read_transcript", json!({"id": "nobody"})),
        "unknown_session"
    );
}

/// `ms` (epoch ms, UTC) as RFC 3339, as the logs write it.
fn iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let of_day = ms.rem_euclid(86_400_000);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1000 % 60,
        of_day % 1000
    )
}

#[test]
fn iso_writes_what_the_logs_parse() {
    for ms in [BASE, at(13, 7) + 250, 951_782_400_000, 1_709_164_800_999] {
        assert_eq!(crate::events::parse_ms(&iso(ms)), Some(ms), "{}", iso(ms));
    }
}

const DAY_MS: i64 = 86_400_000;

/// A home on the real clock: `recent` wrote an hour ago; `old` wrote 60
/// days ago, and so did its file.
fn aged() -> Home {
    let home = Home::new("testbox");
    let now = now_ms();
    let session = |id: &str, at: i64| {
        [
            human(id, iso(at), &format!("Work on {id}")),
            assistant(id, iso(at + 60_000), vec![text(&format!("{id} is done"))]),
        ]
    };
    home.top("recent", &session("recent", now - 3_600_000));
    home.top("old", &session("old", now - 60 * DAY_MS));
    let old = home.root.join("claude/projects/-work-proj/old.jsonl");
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(60 * 86_400))
        .unwrap();
    home
}

fn windowed(home: &Home, all: bool) -> Query {
    Query::new(Options {
        all,
        since: DEFAULT_WINDOW,
        ..home.options.clone()
    })
}

fn ids_of(answer: &Value) -> Vec<&str> {
    answer["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect()
}

/// Whether the event index holds a file's index: whether a build read it.
fn indexed(home: &Home, file: &str) -> bool {
    crate::events::EventCache::open(&home.options.cache)
        .paths()
        .any(|path| path.ends_with(file))
}

#[test]
fn the_window_bounds_what_is_read_and_says_where_it_starts() {
    let home = aged();
    let mut query = windowed(&home, false);
    let listed = query.call("list_sessions", &json!({})).unwrap();
    assert_eq!(ids_of(&listed), ["recent"]);
    assert_eq!(
        listed["now"].as_i64().unwrap() - listed["window_start"].as_i64().unwrap(),
        30 * DAY_MS
    );
    assert!(indexed(&home, "recent.jsonl"));
    assert!(
        !indexed(&home, "old.jsonl"),
        "a file outside the window is not read"
    );
    // Every tool names the window.
    for (tool, args) in [
        ("get_session", json!({"id": "recent"})),
        ("read_transcript", json!({"id": "recent"})),
        ("find", json!({"text": "recent", "since": "10d"})),
        ("stalls", json!({"idle_minutes": 1, "since": "30d"})),
    ] {
        let answer = query.call(tool, &args).unwrap();
        assert!(answer["window_start"].is_i64(), "{tool}: {answer}");
    }
    assert_eq!(
        query
            .call("get_session", &json!({"id": "old"}))
            .unwrap_err()
            .code,
        "unknown_session"
    );
    // --all reads everything and has no start; a windowed build after it
    // keeps the older file's index for the next full build.
    let mut all = windowed(&home, true);
    let everything = all.call("list_sessions", &json!({})).unwrap();
    assert_eq!(ids_of(&everything), ["recent", "old"]);
    assert_eq!(everything["window_start"], Value::Null);
    assert!(indexed(&home, "old.jsonl"));
    let mut again = windowed(&home, false);
    assert_eq!(
        ids_of(&again.call("list_sessions", &json!({})).unwrap()),
        ["recent"]
    );
    assert!(indexed(&home, "old.jsonl"));
}

#[test]
fn a_since_before_the_window_is_refused_not_cut_short() {
    let home = aged();
    let mut query = windowed(&home, false);
    let before = iso(now_ms() - 40 * DAY_MS);
    for (tool, args) in [
        ("list_sessions", json!({"since": "40d"})),
        ("list_sessions", json!({"since": before})),
        ("find", json!({"text": "old", "since": "31d"})),
        ("stalls", json!({"idle_minutes": 5, "since": "6w"})),
    ] {
        let error = query.call(tool, &args).unwrap_err();
        assert_eq!(error.code, "outside_window", "{tool} {args}");
        let start = error.window_start.unwrap();
        assert!((now_ms() - 30 * DAY_MS - start).abs() < 60_000);
        assert_eq!(error.to_json()["error"]["window_start"], start);
    }
    let inside = query
        .call("list_sessions", &json!({"since": "29d"}))
        .unwrap();
    assert_eq!(ids_of(&inside), ["recent"]);
    // With --all there is no window to reach outside of.
    let mut all = windowed(&home, true);
    let old = all.call("list_sessions", &json!({"since": "90d"})).unwrap();
    assert_eq!(ids_of(&old), ["recent", "old"]);
}

/// `path`'s modification time, `ago_ms` before now.
fn age(path: &std::path::Path, ago_ms: i64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(
            std::time::SystemTime::now() - Duration::from_millis(u64::try_from(ago_ms).unwrap()),
        )
        .unwrap();
}

#[test]
fn the_window_start_itself_is_inside_the_window() {
    let home = aged();
    let mut query = windowed(&home, false);
    let now = now_ms();
    let listed = query.call_at("list_sessions", &json!({}), now).unwrap();
    let start = listed["window_start"].as_i64().unwrap();
    // Built on this call's clock: the start is exactly 30 days back.
    assert_eq!(start, now - 30 * DAY_MS);
    for since in [json!("30d"), json!(iso(start))] {
        let answer = query
            .call_at("list_sessions", &json!({"since": since}), now)
            .unwrap();
        assert_eq!(ids_of(&answer), ["recent"], "{since}");
    }
    let error = query
        .call_at("list_sessions", &json!({"since": iso(start - 1)}), now)
        .unwrap_err();
    assert_eq!(
        (error.code, error.window_start),
        ("outside_window", Some(start))
    );
}

#[test]
fn a_session_that_crosses_the_window_start_keeps_every_turn() {
    let home = Home::new("testbox");
    let now = now_ms();
    let early = now - 40 * DAY_MS;
    let late = now - 3_600_000;
    // One file, written an hour ago, with a turn from 40 days back.
    home.top(
        "long",
        &[
            human("long", iso(early), "Start the migration"),
            assistant("long", iso(early + 60_000), vec![text("Started.")]),
            human("long", iso(late), "Finish the migration"),
            assistant("long", iso(late + 60_000), vec![text("Finished.")]),
        ],
    );
    // A session split over two files: the first written 60 days ago, the
    // second naming it by `session_id`.
    let first = now - 60 * DAY_MS;
    home.top(
        "first",
        &[
            human("first", iso(first), "Plan the release"),
            assistant("first", iso(first + 60_000), vec![text("Planned.")]),
        ],
    );
    age(
        &home.root.join("claude/projects/-work-proj/first.jsonl"),
        60 * DAY_MS,
    );
    let resumed_at = now - 7_200_000;
    let mut resumed = human("second", iso(resumed_at), "Ship the release");
    resumed["session_id"] = json!("first");
    let mut shipped = assistant("second", iso(resumed_at + 60_000), vec![text("Shipped.")]);
    shipped["session_id"] = json!("first");
    home.top("second", &[resumed, shipped]);

    let mut query = windowed(&home, false);
    let long = query
        .call_at("get_session", &json!({"id": "long"}), now)
        .unwrap();
    let session = &long["session"];
    let turns: Vec<i64> = session["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|turn| turn["at"].as_i64().unwrap())
        .collect();
    assert_eq!(
        turns,
        [early, late],
        "no turn is trimmed at the window's start"
    );
    assert_eq!(session["handoffs"].as_array().unwrap().len(), 2);
    assert_eq!(
        session["busy"],
        json!([[early, early + 60_000], [late, late + 60_000]])
    );
    assert_eq!(session["start"], early);
    assert_eq!(session["turns_truncated"], false);
    // The resumed session's earlier file wasn't read: it says so.
    let listed = query.call_at("list_sessions", &json!({}), now).unwrap();
    let rows: Vec<(&str, bool)> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["turns_truncated"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("long", false), ("second", true)]);
    // Everything read: one session under its first file's id, whole.
    let mut all = windowed(&home, true);
    let listed = all.call_at("list_sessions", &json!({}), now).unwrap();
    let rows: Vec<(&str, bool)> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["turns_truncated"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("long", false), ("first", false)]);
    let whole = all
        .call_at("get_session", &json!({"id": "first"}), now)
        .unwrap();
    assert_eq!(whole["session"]["turns"].as_array().unwrap().len(), 2);
}

#[test]
fn each_machine_keeps_its_own_window_and_the_latest_start_answers() {
    let now = now_ms();
    let wide = Home::new("wide");
    let narrow = Home::new("narrow");
    for (home, id) in [(&wide, "wide-ten"), (&narrow, "narrow-ten")] {
        let at = now - 10 * DAY_MS;
        home.top(
            id,
            &[
                human(id, iso(at), "Ten days ago"),
                assistant(id, iso(at + 60_000), vec![text("Done.")]),
            ],
        );
        age(
            &home
                .root
                .join(format!("claude/projects/-work-proj/{id}.jsonl")),
            10 * DAY_MS,
        );
    }
    let options = |home: &Home, days: u64| Options {
        all: false,
        since: Duration::from_secs(days * 86_400),
        ..home.options.clone()
    };
    let mut query = Query::with_machines(vec![
        ("wide".into(), options(&wide, 30)),
        ("narrow".into(), options(&narrow, 7)),
    ]);
    let listed = query.call_at("list_sessions", &json!({}), now).unwrap();
    // The narrow machine read only its last 7 days; the wide one, 30.
    assert_eq!(ids_of(&listed), ["wide-ten"]);
    assert_eq!(listed["window_start"], now - 7 * DAY_MS);
    let error = query
        .call_at("list_sessions", &json!({"since": "10d"}), now)
        .unwrap_err();
    assert_eq!(error.code, "outside_window");
}

/// Names and values an environment holds that must never reach an answer,
/// a facts file or a log.
const SECRETS: [&str; 6] = [
    "AWS_SECRET_ACCESS_KEY",
    "aws-secret-value",
    "GITHUB_TOKEN",
    "ghp_notarealtoken",
    "wrong-run",
    "run-huge",
];

/// `/proc/<pid>/environ` of the fixture's proc root.
fn environ(home: &Home, pid: u32, bytes: &[u8]) {
    let path = home.root.join(format!("proc/{pid}/environ"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[cfg(unix)]
fn hold_lock(home: &Home, id: &str, pid: u32) {
    use std::os::unix::fs::MetadataExt;
    let path = home.write(&format!("codex/thread-writer-locks/{id}.lock"), "");
    let meta = fs::metadata(path).unwrap();
    let dev = meta.dev();
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    home.write(
        "proc/locks",
        &format!(
            "12: FLOCK ADVISORY WRITE {pid} {major:x}:{minor:x}:{} 0 EOF\n",
            meta.ino()
        ),
    );
}

/// Every live process's environment, each a case: the run ids among other
/// secrets, none, an oversized one, a gone process, an unreadable one, a
/// value that isn't UTF-8, and a Codex writer lock's holder.
#[cfg(unix)]
fn with_runs() -> Home {
    let home = Home::new("testbox");
    for (pid, id) in [
        (40, "ostrom"),
        (41, "plain"),
        (42, "huge"),
        (43, "gone"),
        (44, "unreadable"),
        (45, "binary"),
    ] {
        home.top(
            id,
            &[
                human(id, ts(9, 0), "Go"),
                assistant(id, ts(9, 1), vec![text("Going.")]),
            ],
        );
        home.pid_file(pid, id, "busy", 777);
        if id != "gone" {
            home.process(pid, 777);
        }
    }
    home.agent(
        "ostrom",
        "helper",
        "t9",
        &[
            user("helper", ts(9, 1), "Help"),
            assistant("helper", ts(9, 2), vec![text("Helped.")]),
        ],
    );
    environ(
        &home,
        40,
        b"PATH=/usr/bin\0AWS_SECRET_ACCESS_KEY=aws-secret-value\0OSTROM_RUN_ID=run-1\0\
GITHUB_TOKEN=ghp_notarealtoken\0OSTROM_RUN_IDX=wrong-run\0XOSTROM_RUN_ID=wrong-run\0\
OSTROM_WORK_ORDER_ID=order-7\0OSTROM_RUN_ID=wrong-run\0NOEQUALS\0",
    );
    environ(&home, 41, b"PATH=/usr/bin\0HOME=/home/fake-user\0");
    let mut huge = b"OSTROM_RUN_ID=run-huge\0PAD=".to_vec();
    huge.extend(std::iter::repeat_n(b'x', 300_000));
    huge.push(0);
    environ(&home, 42, &huge);
    fs::create_dir_all(home.root.join("proc/44/environ")).unwrap();
    environ(&home, 45, b"OSTROM_RUN_ID=\xff\xfe\0");
    home.lines(
        "codex/sessions/2026/09/24/rollout-cx.jsonl",
        &[
            codex_line(
                ts(9, 0),
                "session_meta",
                json!({"id": "cx", "cwd": "/work/proj", "originator": "codex_exec", "thread_source": "user"}),
            ),
            codex_line(
                ts(9, 1),
                "response_item",
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Go"}]}),
            ),
        ],
    );
    hold_lock(&home, "cx", 4242);
    environ(
        &home,
        4242,
        b"OSTROM_RUN_ID=run-cx\0GITHUB_TOKEN=ghp_notarealtoken\0OSTROM_WORK_ORDER_ID=order-cx\0",
    );
    home
}

fn runs_of(answer: &Value) -> BTreeMap<String, Value> {
    answer["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["id"].as_str().unwrap().to_owned(), row["run"].clone()))
        .collect()
}

fn expected_runs() -> BTreeMap<String, Value> {
    [
        (
            "ostrom",
            json!({"OSTROM_RUN_ID": "run-1", "OSTROM_WORK_ORDER_ID": "order-7"}),
        ),
        ("helper", Value::Null),
        ("plain", json!({})),
        ("huge", Value::Null),
        ("gone", Value::Null),
        ("unreadable", Value::Null),
        ("binary", Value::Null),
        (
            "cx",
            json!({"OSTROM_RUN_ID": "run-cx", "OSTROM_WORK_ORDER_ID": "order-cx"}),
        ),
    ]
    .into_iter()
    .map(|(id, run)| (id.to_owned(), run))
    .collect()
}

fn assert_no_secrets(what: &str, text: &str) {
    for secret in SECRETS {
        assert!(!text.contains(secret), "{what} holds {secret}");
    }
}

#[cfg(unix)]
#[test]
fn run_ids_come_from_the_allowlist_and_nothing_else() {
    let home = with_runs();
    let mut query = home.query();
    let listed = query.call_at("list_sessions", &json!({}), NOW).unwrap();
    assert_eq!(runs_of(&listed), expected_runs());
    let rows = listed["sessions"].as_array().unwrap();
    let row = |id: &str| rows.iter().find(|row| row["id"] == id).unwrap();
    assert_eq!(
        (&row("gone")["alive"], &row("cx")["pid"]),
        (&json!(false), &json!(4242))
    );
    // No exit status is in these logs: never inferred.
    assert!(rows.iter().all(|row| row["exit"].is_null()));
    // Nothing else of any environment reaches any answer.
    for (tool, args) in [
        ("list_sessions", json!({})),
        ("get_session", json!({"id": "ostrom"})),
        ("get_session", json!({"id": "cx"})),
        ("read_transcript", json!({"id": "ostrom"})),
        ("find", json!({"text": "o"})),
        ("stalls", json!({"idle_minutes": 1})),
    ] {
        let answer = query.call_at(tool, &args, NOW).unwrap();
        assert_no_secrets(tool, &answer.to_string());
    }
    // Over MCP, the same run ids and nothing else.
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_session","arguments":{"id":"ostrom"}}}"#,
        "\n",
    );
    let mut output = Vec::new();
    crate::serve_mcp(&mut query, input.as_bytes(), &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_no_secrets("MCP output", &output);
    let replies: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let text = |reply: &Value| -> Value {
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    assert_eq!(runs_of(&text(&replies[1])), expected_runs());
    assert_eq!(
        text(&replies[2])["session"]["run"],
        json!({"OSTROM_RUN_ID": "run-1", "OSTROM_WORK_ORDER_ID": "order-7"})
    );
}

#[cfg(unix)]
#[test]
fn run_ids_travel_in_the_facts_and_a_mirror_reads_no_proc() {
    let home = with_runs();
    let facts = crate::local_facts(&home.options).unwrap();
    let run = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    };
    assert_eq!(
        facts.runs,
        BTreeMap::from([
            (
                40,
                run(&[
                    ("OSTROM_RUN_ID", "run-1"),
                    ("OSTROM_WORK_ORDER_ID", "order-7")
                ])
            ),
            (41, run(&[])),
            (
                4242,
                run(&[
                    ("OSTROM_RUN_ID", "run-cx"),
                    ("OSTROM_WORK_ORDER_ID", "order-cx")
                ])
            ),
        ])
    );
    let path = home.root.join("facts.json");
    crate::write_facts(&path, &facts).unwrap();
    assert_no_secrets("the facts file", &fs::read_to_string(&path).unwrap());
    assert_eq!(crate::read_facts(&path).unwrap(), facts);
    // A mirror: the same logs, the recorded facts, and no /proc at all.
    let mirror = Options {
        proc_root: home.root.join("no-proc"),
        cache: home.root.join("mirror/index.json"),
        facts: Some(path),
        ..home.options.clone()
    };
    let mut query = Query::new(mirror);
    let listed = query.call_at("list_sessions", &json!({}), NOW).unwrap();
    assert_eq!(runs_of(&listed), expected_runs());
    // The offline machine has no live process, so no run.
    let offline = facts.offline(NOW);
    assert!(offline.runs.is_empty());
}

#[test]
fn an_empty_union_answers_without_creating_a_home() {
    let mut query = Query::with_machines(Vec::new());
    let listed = query.call_at("list_sessions", &json!({}), NOW).unwrap();
    assert_eq!(listed["sessions"], json!([]));
    assert_eq!(listed["window_start"], Value::Null);
    assert_eq!(
        query
            .call_at("find", &json!({"text":"hello"}), NOW)
            .unwrap()["matches"],
        json!([])
    );
    assert_eq!(
        query
            .call_at("get_session", &json!({"id":"missing"}), NOW)
            .unwrap_err()
            .code,
        "unknown_session"
    );
}

#[test]
fn native_owner_reuses_one_complete_index_across_query_tree_and_model_reads() {
    use crate::viewer::{MachineView, Reading};
    use std::io::Write;

    let home = Home::new("shared-native-owner");
    home.top("lead", &[human("lead", ts(1, 0), "Original evidence")]);
    let path = home.root.join("claude/projects/-work-proj/lead.jsonl");
    let original = fs::read(&path).unwrap();
    let mut local = Query::new(home.options.clone());
    assert!(
        local.sources.machines[0]
            .1
            .work
            .lock()
            .unwrap()
            .events
            .is_none()
    );
    assert!(local.call_at("unknown", &json!({}), NOW).is_err());
    assert!(!home.options.cache.exists());
    assert!(!home.options.cache.with_extension("sqlite3").exists());

    let view = MachineView::new(home.options.clone());
    let parsed = || crate::events::PARSED.with(std::cell::Cell::get);
    let first = view.evidence.query_at(NOW).unwrap();
    let count = parsed();
    assert!(count > 0);
    assert!(Arc::ptr_eq(&first, &view.evidence.query_at(NOW).unwrap()));
    let built = view.built(Reading::At(NOW)).unwrap();
    assert!(built.sessions.contains_key("lead"));
    assert_eq!(view.respond("GET", "/api/tree", "", None).status, 200);
    assert_eq!(
        parsed(),
        count,
        "all readers reuse the complete native index"
    );
    assert_eq!(fs::read(&path).unwrap(), original);

    let added = assistant("lead", ts(1, 1), vec![text("New evidence")]);
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{added}"
    )
    .unwrap();
    let next = view.evidence.query_at(NOW).unwrap();
    assert!(!Arc::ptr_eq(&first, &next));
    assert_eq!(
        parsed(),
        count + 1,
        "only the appended native record is parsed"
    );
    view.built(Reading::At(NOW)).unwrap();
    assert_eq!(view.respond("GET", "/api/tree", "", None).status, 200);
    assert_eq!(parsed(), count + 1);
    assert!(Arc::ptr_eq(&next, &view.evidence.query_at(NOW).unwrap()));
    view.close();
}

#[test]
fn native_owner_retirement_refuses_a_waiting_cold_read_and_keeps_a_warm_cohort() {
    use std::sync::mpsc;
    let home = Home::new("retired-native-owner");
    home.top("lead", &[human("lead", ts(1, 0), "Verified evidence")]);
    let evidence = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let work = evidence.work.lock().unwrap();
    let (started, waiting) = mpsc::channel();
    let worker = {
        let evidence = evidence.clone();
        std::thread::spawn(move || {
            started.send(()).unwrap();
            evidence.query_at(NOW)
        })
    };
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    evidence.retire();
    drop(work);
    assert_eq!(
        worker.join().unwrap().err().unwrap().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert!(!home.options.cache.exists());
    assert!(!home.options.cache.with_extension("sqlite3").exists());

    let warm = crate::native_evidence::NativeEvidence::new(home.options.clone());
    let retained = warm.query_at(NOW).unwrap();
    warm.retire();
    fs::remove_file(home.root.join("claude/projects/-work-proj/lead.jsonl")).unwrap();
    assert!(Arc::ptr_eq(&retained, &warm.query_at(NOW).unwrap()));
    // Retaining metadata never permits serving removed source evidence.
    let records = retained.sources.reader();
    assert!(
        records
            .read(&home.root.join("claude/projects/-work-proj/lead.jsonl"), 0)
            .is_err()
    );
    assert!(records.finish().is_err());
}

#[test]
fn shared_native_queries_reuse_cached_cohorts_and_expose_the_output_window() {
    let home = Home::new("shared");
    let mut options = home.options.clone();
    options.all = false;
    options.since = Duration::from_secs(86400);
    let sources = admitted(options);
    let mut first = Query::from_sources(sources.clone());
    let answer = first.call_at("list_sessions", &json!({}), NOW).unwrap();
    let start = answer["window_start"]
        .as_i64()
        .expect("output-trimmed native cohorts have a window too");
    assert_eq!(start, NOW - 86_400_000);
    let second = Query::from_sources(sources.clone())
        .call_at("list_sessions", &json!({}), NOW)
        .unwrap();
    assert_eq!(answer, second);
    assert_eq!(
        first
            .call_at(
                "list_sessions",
                &json!({"since":"1970-01-01T00:00:00Z"}),
                NOW
            )
            .unwrap_err()
            .window_start,
        Some(start)
    );
    // Closing keeps the last snapshot served without rebuilding sources
    // behind the embedding server.
    sources.close();
    assert_eq!(
        first.call_at("list_sessions", &json!({}), NOW).unwrap(),
        answer
    );
}

#[test]
fn admitted_sources_share_the_index_with_a_later_viewer_and_drain_on_close() {
    let home = fixture();
    let sources = Arc::new(SessionSources::with_machines(vec![(
        "a".into(),
        Options {
            all: false,
            since: Duration::from_secs(86400),
            ..home.options.clone()
        },
    )]));
    let mut first = Query::from_sources(sources.clone());
    assert!(sources.machines[0].1.work.lock().unwrap().events.is_none());
    assert_eq!(
        first.call_at("unknown", &json!({}), NOW).unwrap_err().code,
        "unknown_tool"
    );
    assert!(!home.options.cache.with_extension("sqlite3").exists());
    let builds = crate::model::COMPAT_BUILDS.with(std::cell::Cell::get);
    let answer = first.call_at("list_sessions", &json!({}), NOW).unwrap();
    assert_eq!(answer["window_start"], NOW - 86_400_000);
    assert_eq!(
        crate::model::COMPAT_BUILDS.with(std::cell::Cell::get),
        builds
    );
    let snapshot = first.native.as_ref().unwrap().parts[0].data.clone();
    let parsed = crate::events::PARSED.with(std::cell::Cell::get);
    let core = ViewerCore::from_sources(sources.clone());
    assert!(Arc::ptr_eq(
        &core.machine_views()[0].evidence,
        &sources.machines[0].1
    ));
    assert_eq!(core.respond("GET", "/api/model", "", None).status, 200);
    assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
    let mut second = Query::from_sources(sources.clone());
    assert_eq!(
        second.call_at("list_sessions", &json!({}), NOW).unwrap(),
        answer
    );
    assert!(Arc::ptr_eq(
        &snapshot,
        &second.native.as_ref().unwrap().parts[0].data
    ));
    core.close();
    sources.close();
    home.top(
        "after-close",
        &[human("after-close", ts(6, 0), "Closed source")],
    );
    assert_eq!(
        first.call_at("list_sessions", &json!({}), NOW).unwrap(),
        answer
    );
}

#[test]
fn retired_admitted_sources_refuse_cold_reads_and_new_catalog_demand() {
    let home = fixture();
    let sources = Arc::new(SessionSources::with_machines(vec![(
        "a".into(),
        home.options.clone(),
    )]));
    assert!(sources.invalidate("a"));
    assert!(!sources.invalidate("missing"));
    assert!(sources.machines[0].1.work.lock().unwrap().events.is_none());
    sources.close();
    assert!(!sources.invalidate("a"));
    assert!(
        !sources
            .demand_catalog_source("a", crate::RefreshPool::shared())
            .unwrap()
    );
    assert_eq!(
        Query::from_sources(sources.clone())
            .call_at("list_sessions", &json!({}), NOW)
            .unwrap_err()
            .code,
        "io"
    );
    let viewer = ViewerCore::from_sources(sources);
    assert_ne!(viewer.respond("GET", "/api/model", "", None).status, 200);
    assert!(!home.options.cache.with_extension("sqlite3").exists());
}

#[test]
fn embedded_native_queries_preserve_output_windows_and_legacy_tool_answers() {
    let home = fixture();
    let peer = Home::new("testbox");
    peer.top(
        "other",
        &[human("other", ts(6, 0), "Review the lexer change")],
    );
    for seconds in [86_400, 21_600] {
        for multiple in [false, true] {
            let options = |home: &Home| Options {
                all: false,
                since: Duration::from_secs(seconds),
                scan_window: false,
                ..home.options.clone()
            };
            let mut machines = vec![("a".into(), options(&home))];
            if multiple {
                machines.push(("b".into(), options(&peer)));
            }
            let sources = Arc::new(SessionSources::with_machines(machines));
            let calls = [
                ("list_sessions", json!({})),
                ("list_sessions", json!({"parent":"lead", "limit":1})),
                ("list_sessions", json!({"since":"1970-01-01T00:00:00Z"})),
                ("get_session", json!({"id":"lead"})),
                ("get_session", json!({"id":"scan"})),
                ("get_session", json!({"id":"asker"})),
                ("get_session", json!({"id":"cx"})),
                ("get_session", json!({"id":"absent"})),
                ("read_transcript", json!({"id":"done", "limit":1})),
                (
                    "read_transcript",
                    json!({"id":"scan", "after":0, "limit":2}),
                ),
                ("read_transcript", json!({"id":"done", "turn":"absent"})),
                ("find", json!({"text":"lexer", "limit":2})),
                ("find", json!({"text":"cargo", "max_bytes":1})),
                ("stalls", json!({"idle_minutes":1})),
            ];
            let mut native = Query::from_sources(sources.clone());
            let before = crate::model::COMPAT_BUILDS.with(|count| count.get());
            let actual: Vec<_> = calls
                .iter()
                .map(|(tool, args)| native.call_at(tool, args, NOW))
                .collect();
            assert_eq!(
                crate::model::COMPAT_BUILDS.with(|count| count.get()),
                before
            );
            let snapshot = native.native.as_ref().unwrap().parts[0].data.clone();
            assert_eq!(snapshot.window_start, Some(NOW - seconds as i64 * 1000));
            let mut second = Query::from_sources(sources.clone());
            second.call_at("list_sessions", &json!({}), NOW).unwrap();
            assert!(Arc::ptr_eq(
                &snapshot,
                &second.native.as_ref().unwrap().parts[0].data
            ));

            for ((tool, args), answer) in calls.iter().zip(actual) {
                assert_contract_reference("output", seconds, multiple, tool, args, answer);
            }
            let mut configured = vec![("a".into(), options(&home))];
            if multiple {
                configured.push(("b".into(), options(&peer)));
            }
            let independent_sources = Arc::new(SessionSources::with_machines(configured));
            let mut admitted = Query::from_sources(independent_sources.clone());
            for (tool, args) in &calls {
                assert_contract_reference(
                    "output",
                    seconds,
                    multiple,
                    tool,
                    args,
                    admitted.call_at(tool, args, NOW),
                );
            }
            assert_eq!(
                crate::model::COMPAT_BUILDS.with(|count| count.get()),
                before
            );
            sources.close();
            independent_sources.close();
        }
    }
}

#[test]
fn embedded_native_query_retirement_serves_only_retained_snapshots() {
    let home = fixture();
    let sources = admitted(home.options.clone());
    let mut native = Query::from_sources(sources.clone());
    let before = native.call_at("list_sessions", &json!({}), NOW).unwrap();
    sources.close();
    home.top(
        "after-close",
        &[human("after-close", ts(6, 0), "Closed source")],
    );
    assert_eq!(
        native.call_at("list_sessions", &json!({}), NOW).unwrap(),
        before
    );
    let unopened = admitted(home.options.clone());
    unopened.close();
    assert_eq!(
        Query::from_sources(unopened)
            .call_at("list_sessions", &json!({}), NOW)
            .unwrap_err()
            .code,
        "io"
    );
}

#[test]
fn local_tools_match_legacy_evidence_without_compatibility_preparation() {
    let home = fixture();
    // Same hostname exercises presentation disambiguation independently of the
    // authoritative source keys. The second source has its own native evidence.
    let peer = Home::new("testbox");
    peer.top(
        "other",
        &[
            human("other", ts(6, 0), "Review the lexer change"),
            assistant("other", ts(6, 1), vec![text("Looks right.")]),
        ],
    );
    for multiple in [false, true] {
        let mut machines = vec![("a".to_owned(), home.options.clone())];
        if multiple {
            machines.push(("b".to_owned(), peer.options.clone()));
        }
        let mut native = Query::with_machines(machines.clone());
        for revision in 0..3 {
            if revision == 1 {
                home.top(
                    "done",
                    &[
                        human("done", ts(4, 0), "Summarize the design doc"),
                        assistant("done", ts(4, 1), vec![text("Updated lexer evidence.")]),
                        assistant("done", ts(7, 0), vec![text("A later response.")]),
                    ],
                );
            }
            if revision == 2 {
                let path = home.root.join("proc/30/stat");
                if path.exists() {
                    fs::remove_file(path).unwrap();
                }
            }
            let calls = [
                ("list_sessions", json!({})),
                ("list_sessions", json!({"parent":"lead", "limit":1})),
                ("get_session", json!({"id":"lead"})),
                ("get_session", json!({"id":"scan"})),
                ("get_session", json!({"id":"asker"})),
                ("get_session", json!({"id":"cx"})),
                ("read_transcript", json!({"id":"done", "limit":1})),
                (
                    "read_transcript",
                    json!({"id":"scan", "after":0, "limit":2}),
                ),
                ("read_transcript", json!({"id":"done", "turn":"absent"})),
                ("get_session", json!({"id":"absent"})),
                ("find", json!({"text":"lexer", "limit":2})),
                ("find", json!({"text":"cargo", "max_bytes":1})),
                ("stalls", json!({"idle_minutes":1})),
            ];
            let before = crate::model::COMPAT_BUILDS.with(|count| count.get());
            let actual: Vec<_> = calls
                .iter()
                .map(|(tool, args)| native.call_at(tool, args, NOW))
                .collect();
            assert_eq!(
                crate::model::COMPAT_BUILDS.with(|count| count.get()),
                before
            );
            let snapshot = native.native.as_ref().unwrap().parts[0].data.clone();
            native.call_at("list_sessions", &json!({}), NOW).unwrap();
            assert!(Arc::ptr_eq(
                &snapshot,
                &native.native.as_ref().unwrap().parts[0].data
            ));

            for ((tool, args), answer) in calls.iter().zip(actual) {
                assert_contract_reference("scan", revision, multiple, tool, args, answer);
            }
        }
    }
}

#[test]
fn local_query_conflicts_and_source_removal_use_native_snapshots() {
    let first = Home::new("same");
    let second = Home::new("same");
    for home in [&first, &second] {
        home.top(
            "duplicate",
            &[
                human("duplicate", ts(1, 0), "Same native session ID"),
                assistant("duplicate", ts(1, 1), vec![text("Evidence")]),
            ],
        );
    }
    let mut native = Query::with_machines(vec![
        ("a".into(), first.options.clone()),
        ("b".into(), second.options.clone()),
    ]);
    let before = crate::model::COMPAT_BUILDS.with(|count| count.get());
    for tool in ["list_sessions", "get_session", "read_transcript"] {
        let args = if tool == "list_sessions" {
            json!({})
        } else {
            json!({"id":"duplicate"})
        };
        assert_eq!(
            native.call_at(tool, &args, NOW).unwrap_err().code,
            "id_conflict"
        );
    }
    fs::remove_file(
        second
            .root
            .join("claude/projects/-work-proj/duplicate.jsonl"),
    )
    .unwrap();
    assert_eq!(
        native.call_at("list_sessions", &json!({}), NOW).unwrap()["total"],
        1
    );
    assert!(
        native
            .call_at("read_transcript", &json!({"id":"duplicate"}), NOW)
            .is_ok()
    );
    assert_eq!(
        crate::model::COMPAT_BUILDS.with(|count| count.get()),
        before
    );
}

/// The fixture's independently captured turns and handoffs, without invoking
/// full-workspace compatibility preparation to construct expected values.
fn fixture_relation_reference() -> Value {
    let rows = contract_reference()["responses"].as_array().unwrap();
    let captured = |tool: &str, args: Value| -> &Value {
        let matches: Vec<_> = rows
            .iter()
            .filter(|row| {
                row["scope"] == "scan"
                    && row["scenario"] == 0
                    && row["multiple"] == false
                    && row["tool"] == tool
                    && row["args"] == args
            })
            .collect();
        assert_eq!(matches.len(), 1, "unique fixture reference: {tool} {args}");
        &matches[0]["response"]["ok"]
    };
    let asker = &captured("get_session", json!({"id": "asker"}))["session"];
    let mut turns = asker["turns"].as_array().unwrap().clone();
    for sid in ["cx", "scan"] {
        turns.extend(
            captured("get_session", json!({"id": sid}))["session"]["turns"]
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
    }
    let done = captured("read_transcript", json!({"id": "done", "limit": 1}));
    turns.push(json!({"sid": "done", "id": done["entries"][0]["turn"]}));
    json!({"turns": turns, "handoffs": asker["handoffs"]})
}

fn contract_reference() -> &'static Value {
    static REFERENCE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    REFERENCE.get_or_init(|| {
        let reference: Value =
            serde_json::from_str(include_str!("contract-reference.json")).unwrap();
        assert_eq!(reference["format"], "semon.query-contract-reference.v1");
        assert_eq!(
            reference["source_revision"],
            "7ce1ac4d4f34af9143aa292b631a07fd797890b1"
        );
        assert_eq!(reference["clock"], NOW);
        assert_eq!(reference["responses"].as_array().unwrap().len(), 134);
        reference
    })
}

/// Frozen responses from the retired production engine, captured before its
/// removal. These preserve an independent oracle without retaining that engine.
fn assert_contract_reference(
    scope: &str,
    scenario: u64,
    multiple: bool,
    tool: &str,
    args: &Value,
    actual: Result<Value, QueryError>,
) {
    let reference = contract_reference();
    let rows: Vec<_> = reference["responses"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["scope"] == scope
                && row["scenario"] == scenario
                && row["multiple"] == multiple
                && row["tool"] == tool
                && &row["args"] == args
        })
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "unique {scope} reference: {scenario}, multiple={multiple}, {tool} {args}"
    );
    let actual = match actual {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"err":error.to_json()}),
    };
    assert_eq!(
        actual, rows[0]["response"],
        "{scope}: {scenario}, multiple={multiple}, {tool} {args}"
    );
}

struct SelectedArchive {
    source: crate::SessionSourceRef,
    bytes: Arc<[u8]>,
    calls: std::sync::atomic::AtomicUsize,
    read_bytes: AtomicU64,
    fault: u8,
}
impl crate::SessionSourceReader for SelectedArchive {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        assert_eq!(source, &self.source, "unrelated source was requested");
        assert!(max <= crate::SESSION_SOURCE_CHUNK_MAX);
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fault == 3 && call > 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "revoked"));
        }
        let generation = if self.fault == 2 && call > 0 {
            "replacement"
        } else {
            "archive-generation"
        };
        if expected.is_some_and(|expected| expected != generation) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "generation changed",
            ));
        }
        let start = usize::try_from(offset).unwrap();
        let end = self.bytes.len().min(start + max);
        let mut bytes = self.bytes[start..end].to_vec();
        if self.fault == 1
            && let Some(at) = bytes.windows(6).position(|part| part == b"needle")
        {
            bytes[at] = b'N';
        }
        self.read_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(crate::SessionSourceRange {
            generation: generation.into(),
            length: self.bytes.len() as u64,
            offset,
            bytes: Arc::from(bytes),
            cached: call > 0,
        })
    }
}

fn selected_archive(home: &Home, fault: u8) -> SelectedArchive {
    let db =
        rusqlite::Connection::open(crate::events::EventCache::path(&home.options.cache)).unwrap();
    let text: String = db
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key='lead'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let row: crate::model::summary::CatalogRow = serde_json::from_str(&text).unwrap();
    assert_eq!(row.sources.len(), 1);
    let source = &row.sources[0];
    SelectedArchive {
        source: crate::SessionSourceRef {
            root: "claude".into(),
            path: "projects/-work-proj/lead.jsonl".into(),
            native_id: source.native_id.clone(),
            offset: source.offset,
            prefix_sha256: source.prefix_sha256,
            tail_sha256: source.tail_sha256,
        },
        bytes: Arc::from(fs::read(&source.path).unwrap()),
        calls: std::sync::atomic::AtomicUsize::new(0),
        read_bytes: AtomicU64::new(0),
        fault,
    }
}

#[test]
fn selected_query_uses_original_generations_without_workspace_preparation_or_restoration() {
    let mut measurements = Vec::new();
    for unrelated in [0, 256] {
        let home = Home::new("selected-native-query");
        home.top(
            "lead",
            &[
                human("lead", ts(1, 0), "selected needle"),
                assistant("lead", ts(1, 1), vec![text("selected answer")]),
            ],
        );
        for n in 0..unrelated {
            let id = format!("unrelated-{n}");
            home.top(&id, &[human(&id, ts(1, 0), &"unrelated".repeat(8192))]);
        }
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
        let tools = [
            ("get_session", json!({"id":"lead"})),
            ("read_transcript", json!({"id":"lead","limit":10})),
        ];
        let expected: Vec<_> = tools
            .iter()
            .map(|(tool, args)| home.query().call_at(tool, args, NOW).unwrap())
            .collect();
        let archive = selected_archive(&home, 0);
        fs::remove_file(home.options.claude_home.join(&archive.source.path)).unwrap();
        let sources = admitted(home.options.clone());
        let mut query = Query::from_sources(sources.clone());
        let started = std::time::Instant::now();
        for ((tool, args), expected) in tools.iter().zip(expected) {
            assert_eq!(
                query
                    .call_selected_at(
                        "local",
                        "lead",
                        tool,
                        args,
                        Some(&archive),
                        crate::SessionSourceReadMode::ProviderOnly,
                        NOW
                    )
                    .unwrap(),
                expected
            );
        }
        assert!(
            sources.machines[0].1.work.lock().unwrap().events.is_none(),
            "selected Query constructed a workspace producer"
        );
        assert!(!home.options.claude_home.join(&archive.source.path).exists());
        let work = (
            archive.calls.load(Ordering::Relaxed),
            archive.read_bytes.load(Ordering::Relaxed),
        );
        println!(
            "selected Query get/transcript: unrelated={unrelated}, {}us, calls={}, bytes={}",
            started.elapsed().as_micros(),
            work.0,
            work.1
        );
        measurements.push(work);
    }
    assert_eq!(
        measurements[0], measurements[1],
        "unrelated history changed focused I/O"
    );
}

#[test]
fn selected_query_refuses_changed_bytes_generations_and_revocation() {
    for fault in [1, 2, 3] {
        let home = Home::new("selected-query-fences");
        home.top("lead", &[human("lead", ts(1, 0), "selected needle")]);
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
        let archive = selected_archive(&home, fault);
        fs::remove_file(home.options.claude_home.join(&archive.source.path)).unwrap();
        let generation = catalog_generation(&home);
        let mut query = Query::from_sources(admitted(home.options.clone()));
        assert_eq!(
            query
                .call_selected_at(
                    "local",
                    "lead",
                    "read_transcript",
                    &json!({"id":"lead"}),
                    Some(&archive),
                    crate::SessionSourceReadMode::ProviderOnly,
                    NOW
                )
                .unwrap_err()
                .code,
            "io"
        );
        assert_eq!(
            catalog_generation(&home),
            generation,
            "a selected read published metadata"
        );
        assert!(!home.options.claude_home.join(&archive.source.path).exists());
    }
}

#[test]
fn selected_query_preserves_the_independent_tool_reference_and_union_labels() {
    let home = fixture();
    let peer = Home::new("testbox");
    peer.top(
        "other",
        &[human("other", ts(6, 0), "Review the lexer change")],
    );
    for seconds in [86_400, 21_600] {
        for multiple in [false, true] {
            let options = |home: &Home| Options {
                all: false,
                since: Duration::from_secs(seconds),
                scan_window: false,
                ..home.options.clone()
            };
            let mut machines = vec![("a".into(), options(&home))];
            if multiple {
                machines.push(("b".into(), options(&peer)));
            }
            // Publish through the actual native producer. Focused Query must
            // consume its source identity and recipes without preparing a union.
            let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
            crate::model::observe_catalog(
                &home.options,
                &mut cache,
                &mut false,
                &mut crate::model::Texts::default(),
                NOW,
            )
            .unwrap();
            let sources = Arc::new(SessionSources::with_machines(machines));
            let mut query = Query::from_sources(sources.clone());
            for (tool, args) in [
                ("get_session", json!({"id":"lead"})),
                ("get_session", json!({"id":"scan"})),
                ("get_session", json!({"id":"asker"})),
                ("get_session", json!({"id":"cx"})),
                ("read_transcript", json!({"id":"done","limit":1})),
                ("read_transcript", json!({"id":"scan","after":0,"limit":2})),
                ("read_transcript", json!({"id":"done","turn":"absent"})),
            ] {
                let key = args["id"].as_str().unwrap();
                let result = query.call_selected_at(
                    "a",
                    key,
                    tool,
                    &args,
                    None,
                    crate::SessionSourceReadMode::LocalThenProvider,
                    NOW,
                );
                assert_contract_reference("output", seconds, multiple, tool, &args, result);
            }
            for (_, source) in &sources.machines {
                assert!(
                    source.work.lock().unwrap().events.is_none(),
                    "focused read prepared an unrelated source owner"
                );
            }
        }
    }
}

struct PublishingArchive<'a> {
    archive: SelectedArchive,
    home: &'a Home,
    selected_changes: bool,
    published: std::sync::atomic::AtomicBool,
}
impl crate::SessionSourceReader for PublishingArchive<'_> {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        let result = self.archive.read_range(source, expected, offset, max)?;
        if !self.published.swap(true, Ordering::SeqCst) {
            if self.selected_changes {
                use std::io::Write;
                writeln!(
                    fs::OpenOptions::new()
                        .append(true)
                        .open(
                            self.home
                                .options
                                .claude_home
                                .join(&self.archive.source.path)
                        )
                        .unwrap(),
                    "{}",
                    assistant("lead", ts(1, 1), vec![text("new selected generation")])
                )
                .unwrap();
            } else {
                self.home.top(
                    "unrelated-arrival",
                    &[human("unrelated-arrival", ts(2, 0), "unrelated")],
                );
            }
            let mut cache =
                crate::events::EventCache::open_scoped(&self.home.options.cache).unwrap();
            crate::model::observe_catalog(
                &self.home.options,
                &mut cache,
                &mut false,
                &mut crate::model::Texts::default(),
                NOW,
            )
            .unwrap();
        }
        Ok(result)
    }
}

#[test]
fn selected_query_completion_fences_its_sources_without_coupling_unrelated_publications() {
    for selected_changes in [false, true] {
        let home = Home::new("selected-publication-fence");
        home.top("lead", &[human("lead", ts(1, 0), "selected needle")]);
        let mut baseline = Query::from_sources(admitted(home.options.clone()));
        let args = json!({"id":"lead"});
        let before = baseline.call_at("get_session", &args, NOW).unwrap();
        let generation = catalog_generation(&home);
        let archive = PublishingArchive {
            archive: selected_archive(&home, 0),
            home: &home,
            selected_changes,
            published: std::sync::atomic::AtomicBool::new(false),
        };
        let mut query = Query::from_sources(admitted(home.options.clone()));
        let result = query.call_selected_at(
            "local",
            "lead",
            "get_session",
            &args,
            Some(&archive),
            crate::SessionSourceReadMode::ProviderOnly,
            NOW,
        );
        assert!(archive.published.load(Ordering::SeqCst));
        assert_ne!(
            catalog_generation(&home),
            generation,
            "actual native publication must run during the read"
        );
        if selected_changes {
            assert_eq!(result.unwrap_err().code, "io");
        } else {
            assert_eq!(result.unwrap(), before);
        }
    }
}

#[test]
fn selected_query_uses_verified_local_bytes_and_never_hides_a_rewrite_with_archive_fallback() {
    let home = Home::new("selected-live-fallback");
    home.top("lead", &[human("lead", ts(1, 0), "selected needle")]);
    let args = json!({"id":"lead"});
    let expected = Query::from_sources(admitted(home.options.clone()))
        .call_at("get_session", &args, NOW)
        .unwrap();
    let archive = selected_archive(&home, 3);
    let mut query = Query::from_sources(admitted(home.options.clone()));
    assert_eq!(
        query
            .call_selected_at(
                "local",
                "lead",
                "get_session",
                &args,
                Some(&archive),
                crate::SessionSourceReadMode::LocalThenProvider,
                NOW
            )
            .unwrap(),
        expected
    );
    assert_eq!(
        archive.calls.load(Ordering::Relaxed),
        0,
        "live source required an archive manifest"
    );
    let path = home.options.claude_home.join(&archive.source.path);
    let bytes = fs::read_to_string(&path).unwrap();
    fs::write(path, bytes.replace("needle", "NEEDLE")).unwrap();
    assert_eq!(
        query
            .call_selected_at(
                "local",
                "lead",
                "get_session",
                &args,
                Some(&archive),
                crate::SessionSourceReadMode::LocalThenProvider,
                NOW
            )
            .unwrap_err()
            .code,
        "io"
    );
    assert_eq!(
        archive.calls.load(Ordering::Relaxed),
        0,
        "a changed current source fell back to old archive bytes"
    );
}

#[test]
fn focused_far_end_and_unsent_handoffs_use_native_owner_evidence_and_union_aliases() {
    let home = Home::new("same-host");
    home.top("lead", &[
        human("lead", ts(1, 0), "Delegate the work"),
        assistant("lead", ts(1, 1), vec![
            tool("far", "SendMessage", json!({"to":"remote","message":"Far-end context"})),
            tool("lost", "SendMessage", json!({"to":"missing","message":"Failed delivery"})),
        ]),
        json!({"type":"user","sessionId":"lead","timestamp":ts(1,2),
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"far","content":"sent"}]},
            "toolUseResult":{"success":true,"message":"sent"}}),
        json!({"type":"user","sessionId":"lead","timestamp":ts(1,2),
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"lost","content":"missing peer"}]},
            "toolUseResult":{"success":false,"message":"missing peer"}}),
    ]);
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let peer = Home::new("same-host");
    peer.top("other", &[human("other", ts(1, 0), "Other machine")]);
    for multiple in [false, true] {
        let mut machines = vec![("a".into(), home.options.clone())];
        if multiple {
            machines.push(("b".into(), peer.options.clone()));
        }
        let mut legacy = Query::with_machines(machines.clone());
        let listing = legacy.call_at("list_sessions", &json!({}), NOW).unwrap();
        let id = listing["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == "stub")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let raw = if multiple {
            id.rsplit_once('@').unwrap().0
        } else {
            id
        };
        let refs = crate::session_query_sources(&home.options, raw).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].native_id, "lead");
        let sources = Arc::new(SessionSources::with_machines(machines));
        let mut focused = Query::from_sources(sources.clone());
        let labels = focused.source_labels();
        assert_eq!(labels[0], ("a".into(), "same-host".into()));
        if multiple {
            assert_eq!(labels[1], ("b".into(), "same-host~b".into()));
        }
        for tool in ["get_session", "read_transcript"] {
            let args = json!({"id":id});
            assert_eq!(
                focused.call_selected_at(
                    "a",
                    raw,
                    tool,
                    &args,
                    None,
                    crate::SessionSourceReadMode::LocalThenProvider,
                    NOW
                ),
                legacy.call_at(tool, &args, NOW),
                "{multiple}: {tool} {args}"
            );
        }
        for (_, source) in &sources.machines {
            assert!(source.work.lock().unwrap().events.is_none());
        }
    }
}

#[test]
fn unsent_query_uses_indexed_bookmark_evidence() {
    // A failed peer send is a transcript address, not a session Catalog row.
    let home = Home::new("unsent-host");
    home.top("lead", &[
        human("lead", ts(1,0), "Send the result"),
        assistant("lead", ts(1,1), vec![tool("lost", "SendMessage",
            json!({"to":"missing","message":"Failed delivery"}))]),
        json!({"type":"user","sessionId":"lead","timestamp":ts(1,2),
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"lost","content":"missing peer"}]},
            "toolUseResult":{"success":false,"message":"missing peer"}}),
    ]);
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let mut normal = home.query();
    let args = json!({"id":"unsent:missing"});
    let expected = normal.call_at("read_transcript", &args, NOW).unwrap();
    assert_eq!(expected["total"], 2);
    let refs = crate::session_query_sources(&home.options, "unsent:missing").unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].native_id, "lead");
    let sources = admitted(home.options.clone());
    let mut focused = Query::from_sources(sources.clone());
    assert_eq!(
        focused
            .call_selected_at(
                "local",
                "unsent:missing",
                "read_transcript",
                &args,
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
                NOW
            )
            .unwrap(),
        expected
    );
    assert!(sources.machines[0].1.work.lock().unwrap().events.is_none());
}

#[test]
fn handoff_endpoint_indexes_upgrade_without_losing_native_evidence() {
    let home = fixture();
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    drop(cache);
    let db =
        rusqlite::Connection::open(crate::events::EventCache::path(&home.options.cache)).unwrap();
    let snapshot = || {
        let mut statement = db
            .prepare("SELECT metadata FROM session_catalog ORDER BY session_key")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let original = snapshot();
    let generation: String = db
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let events: i64 = db
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    db.execute_batch(
        "DROP INDEX session_trace_from; DROP INDEX session_trace_to; PRAGMA user_version=21;",
    )
    .unwrap();
    let reopened = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    assert_eq!(reopened.session_catalog_generation().unwrap(), generation);
    assert_eq!(snapshot(), original);
    assert_eq!(
        db.query_row("SELECT count(*) FROM events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        events
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        22
    );
    assert_eq!(db.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('session_trace_from','session_trace_to')", [], |row|row.get::<_,i64>(0)).unwrap(), 2);
    assert!(
        !crate::session_query_sources(&home.options, "scan")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn selected_native_query_closes_nested_parent_dependencies_without_unrelated_sources() {
    let home = Home::new("nested-query");
    for (index, (id, parent)) in [
        ("root", None),
        ("middle", Some("root")),
        ("child", Some("middle")),
        ("leaf", Some("child")),
    ]
    .into_iter()
    .enumerate()
    {
        let child = ["middle", "child", "leaf"].get(index).copied();
        let mut records = vec![if parent.is_none() {
            human(id, ts(1, 0), "Delegate nested work")
        } else {
            user(id, ts(1, 0), "Delegate nested work")
        }];
        if let Some(child) = child {
            records.push(assistant(
                id,
                ts(1, 1),
                vec![tool(
                    &format!("spawn-{child}"),
                    "Agent",
                    json!({"description":child,"prompt":"Delegate nested work"}),
                )],
            ));
        } else {
            records.push(assistant(id, ts(1, 2), vec![text("Nested result")]));
        }
        if let Some(parent) = parent {
            home.agent(parent, id, &format!("spawn-{id}"), &records);
        } else {
            home.top(id, &records);
        }
    }
    for n in 0..64 {
        let id = format!("unrelated-{n}");
        home.top(&id, &[human(&id, ts(1, 0), "Unrelated history")]);
    }
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let refs = crate::session_query_sources(&home.options, "leaf").unwrap();
    assert_eq!(
        refs.iter()
            .map(|source| source.native_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["root", "middle", "child", "leaf"])
    );
    assert_eq!(
        refs,
        crate::session_query_sources(&home.options, "leaf").unwrap(),
        "closure ordering changed"
    );
    let sources = admitted(home.options.clone());
    let args = json!({"id":"leaf"});
    let expected = home.query().call_at("get_session", &args, NOW).unwrap();
    assert_eq!(expected["session"]["parent"], "child");
    assert_eq!(
        Query::from_sources(sources.clone())
            .call_selected_at(
                "local",
                "leaf",
                "get_session",
                &args,
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
                NOW
            )
            .unwrap(),
        expected
    );
    assert!(sources.machines[0].1.work.lock().unwrap().events.is_none());
}

#[test]
fn focused_query_keeps_cross_harness_and_literal_native_identity_collision_context() {
    let home = Home::new("focused-aliases");
    home.top(
        "same-id",
        &[human("same-id", ts(1, 0), "Claude collision owner")],
    );
    home.top(
        "claude:same-id",
        &[human(
            "claude:same-id",
            ts(1, 0),
            "literal qualified-looking id",
        )],
    );
    home.lines(
        "codex/sessions/2026/09/24/rollout-same-id.jsonl",
        &[
            codex_line(
                ts(1, 0),
                "session_meta",
                json!({"id":"same-id","cwd":"/work/proj"}),
            ),
            codex_line(
                ts(1, 1),
                "event_msg",
                json!({"type":"user_message","message":"Codex collision owner"}),
            ),
        ],
    );
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let mut baseline = Query::from_sources(admitted(home.options.clone()));
    let mut focused = Query::from_sources(admitted(home.options.clone()));
    for key in [":claude:same-id", "claude:same-id", "codex:same-id"] {
        for tool in ["get_session", "read_transcript"] {
            let args = json!({"id":key});
            let expected = baseline.call_at(tool, &args, NOW).unwrap();
            let actual = focused
                .call_selected_at(
                    "local",
                    key,
                    tool,
                    &args,
                    None,
                    crate::SessionSourceReadMode::LocalThenProvider,
                    NOW,
                )
                .unwrap();
            assert_eq!(
                actual, expected,
                "{key} {tool}: focused derivation changed canonical identity"
            );
        }
    }
}

#[test]
fn focused_native_identity_collision_context_refuses_an_overbound_closure() {
    let home = Home::new("focused-alias-bound");
    home.top("same-id", &[human("same-id", ts(1, 0), "Claude owner")]);
    for depth in 0..64 {
        let id = format!("{}claude:same-id", ":".repeat(depth));
        home.top(&id, &[human(&id, ts(1, 0), "literal identity occupant")]);
    }
    home.lines(
        "codex/sessions/2026/09/24/rollout-same-id.jsonl",
        &[codex_line(
            ts(1, 0),
            "session_meta",
            json!({"id":"same-id"}),
        )],
    );
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let key = format!("{}claude:same-id", ":".repeat(64));
    let error = crate::session_query_sources(&home.options, &key).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert!(
        error
            .to_string()
            .contains("closure exceeds its source bound")
    );
}

#[test]
fn ordinary_focused_query_reuses_qualified_catalog_without_unrelated_index_loads() {
    for all in [false, true] {
        for unrelated in [0, 256] {
            let mut home = Home::new("ordinary-focused");
            home.options.all = all;
            home.top("lead", &[human("lead", ts(1, 0), "selected prompt")]);
            for n in 0..unrelated {
                let id = format!("unrelated-{n}");
                home.top(&id, &[human(&id, ts(1, 0), "unrelated prompt")]);
            }
            let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
            crate::model::observe_catalog(
                &home.options,
                &mut cache,
                &mut false,
                &mut crate::model::Texts::default(),
                NOW,
            )
            .unwrap();
            let mut baseline = Query::from_sources(admitted(Options {
                scan_window: true,
                ..home.options.clone()
            }));
            let tools = [
                ("get_session", json!({"id":"lead"})),
                ("read_transcript", json!({"id":"lead","limit":10})),
            ];
            let expected: Vec<_> = tools
                .iter()
                .map(|(tool, args)| baseline.call_at(tool, args, NOW).unwrap())
                .collect();
            let mut query = home.query();
            let before = crate::events::FILE_READS.with(std::cell::Cell::get);
            for ((tool, args), expected) in tools.iter().zip(expected) {
                assert_eq!(query.call_at(tool, args, NOW).unwrap(), expected);
            }
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - before;
            eprintln!("ordinary focused Query all={all} unrelated={unrelated} index_loads={loads}");
            assert!(
                loads <= 4,
                "ordinary focused Query loaded unrelated indexes: {loads}"
            );
        }
    }
}

#[test]
fn ordinary_focused_query_refreshes_selected_log_without_unrelated_index_loads() {
    use std::io::Write;

    for unrelated in [0, 256] {
        let home = Home::new("ordinary-focused-refresh");
        home.top("lead", &[human("lead", ts(1, 0), "selected prompt")]);
        for n in 0..unrelated {
            let id = format!("unrelated-{n}");
            home.top(&id, &[human(&id, ts(1, 0), "unrelated prompt")]);
        }
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
        let mut query = home.query();
        for phase in ["append", "rewrite"] {
            if phase == "append" {
                let mut file = fs::OpenOptions::new()
                    .append(true)
                    .open(
                        home.options
                            .claude_home
                            .join("projects/-work-proj/lead.jsonl"),
                    )
                    .unwrap();
                writeln!(file, "{}", human("lead", ts(1, 1), "appended prompt")).unwrap();
            } else {
                home.top("lead", &[human("lead", ts(1, 0), "rewritten prompt")]);
            }
            // The complete native reference has separate index custody. It
            // cannot accidentally refresh the Catalog being exercised here.
            let mut baseline = Query::from_sources(admitted(Options {
                cache: home.root.join(format!("reference-{phase}")),
                scan_window: true,
                ..home.options.clone()
            }));
            let tools = [
                ("get_session", json!({"id":"lead"})),
                ("read_transcript", json!({"id":"lead","limit":10})),
            ];
            let expected: Vec<_> = tools
                .iter()
                .map(|(tool, args)| baseline.call_at(tool, args, NOW).unwrap())
                .collect();
            let before = crate::events::FILE_READS.with(std::cell::Cell::get);
            for ((tool, args), expected) in tools.iter().zip(expected) {
                assert_eq!(query.call_at(tool, args, NOW).unwrap(), expected);
            }
            let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - before;
            eprintln!(
                "ordinary focused refresh phase={phase} unrelated={unrelated} index_loads={loads}"
            );
            assert!(
                loads <= 5,
                "focused refresh loaded unrelated indexes: {loads}"
            );
            assert_eq!(
                crate::session_catalog_identity(&home.options, "local", "lead")
                    .unwrap()
                    .unwrap()
                    .owner_qualification,
                crate::CatalogOwnerQualification::Qualified
            );
        }
    }
}

#[test]
fn ordinary_focused_query_reconciles_changed_log_sidecar_and_scan_window_context() {
    let mut home = Home::new("ordinary-focused-freshness");
    home.top("parent", &[human("parent", ts(1, 0), "parent prompt")]);
    home.agent(
        "parent",
        "child",
        "spawn",
        &[human("parent", ts(1, 1), "child prompt")],
    );
    for n in 0..256 {
        let id = format!("unrelated-{n}");
        home.top(&id, &[human(&id, ts(1, 0), "unrelated prompt")]);
    }
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let compare = |options: &Options, id: &str| {
        let args = json!({"id":id});
        let expected = Query::from_sources(admitted(Options {
            cache: home.root.join("reference-freshness"),
            scan_window: true,
            ..options.clone()
        }))
        .call_at("get_session", &args, NOW)
        .unwrap();
        let before = crate::events::FILE_READS.with(std::cell::Cell::get);
        assert_eq!(
            Query::new(options.clone())
                .call_at("get_session", &args, NOW)
                .unwrap(),
            expected
        );
        let loads = crate::events::FILE_READS.with(std::cell::Cell::get) - before;
        assert!(
            loads <= 8,
            "selected freshness read loaded unrelated indexes: {loads}"
        );
    };
    compare(&home.options, "child");
    home.write(
        "claude/projects/-work-proj/parent/subagents/agent-child.meta.json",
        &json!({"description":"updated native role","toolUseId":"spawn","model":"updated-model"})
            .to_string(),
    );
    compare(&home.options, "child");
    home.top(
        "parent",
        &[human("parent", ts(1, 0), "rewritten parent prompt")],
    );
    compare(&home.options, "parent");
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    home.options.all = false;
    home.options.since = Duration::from_secs(6 * 3600);
    fs::File::open(
        home.options
            .claude_home
            .join("projects/-work-proj/parent.jsonl"),
    )
    .unwrap()
    .set_times(
        fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_millis((NOW - 86400000) as u64)),
    )
    .unwrap();
    compare(&home.options, "child");
}

#[test]
fn focused_query_preserves_whole_file_scan_window_and_skipped_parent_context() {
    let mut home = Home::new("focused-cross-window");
    home.options.all = false;
    home.options.since = Duration::from_secs(6 * 3600);
    home.top(
        "parent",
        &[human("parent", ts(1, 0), "outside-window parent")],
    );
    home.top(
        "outside",
        &[human("outside", ts(1, 0), "excluded isolated source")],
    );
    home.agent(
        "parent",
        "child",
        "spawn",
        &[human("parent", ts(1, 1), "inside-window child")],
    );
    for (path, age) in [
        ("projects/-work-proj/parent.jsonl", 86400000),
        ("projects/-work-proj/outside.jsonl", 86400000),
        (
            "projects/-work-proj/parent/subagents/agent-child.jsonl",
            3600000,
        ),
    ] {
        fs::File::open(home.options.claude_home.join(path))
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + Duration::from_millis((NOW - age) as u64),
                ),
            )
            .unwrap();
    }
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    let options = Options {
        scan_window: true,
        ..home.options.clone()
    };
    for id in ["child", "parent", "outside"] {
        for tool in ["get_session", "read_transcript"] {
            let args = json!({"id":id});
            let expected = Query::from_sources(admitted(options.clone())).call_at(tool, &args, NOW);
            let focused = Query::from_sources(admitted(options.clone())).call_selected_at(
                "local",
                id,
                tool,
                &args,
                None,
                crate::SessionSourceReadMode::LocalThenProvider,
                NOW,
            );
            assert_eq!(
                focused, expected,
                "{id} {tool}: selected scan changed whole-file semantics"
            );
            let mut ordinary = Query::new(home.options.clone());
            assert_eq!(ordinary.call_at(tool, &args, NOW), expected);
            assert!(
                ordinary.sources.machines[0]
                    .1
                    .work
                    .lock()
                    .unwrap()
                    .events
                    .is_none()
            );
        }
    }
    let reference = crate::session_query_sources(&home.options, "child")
        .unwrap()
        .into_iter()
        .find(|source| source.native_id == "child")
        .unwrap();
    let archive = SelectedArchive {
        bytes: Arc::from(fs::read(home.options.claude_home.join(&reference.path)).unwrap()),
        source: reference,
        calls: std::sync::atomic::AtomicUsize::new(0),
        read_bytes: AtomicU64::new(0),
        fault: 0,
    };
    // Missing excluded ledgers cannot force archive-body fallback. This reader
    // accepts only the child and panics if any excluded source is requested.
    let db =
        rusqlite::Connection::open(crate::events::EventCache::path(&home.options.cache)).unwrap();
    db.execute(
        "DELETE FROM files WHERE path=?1",
        [home
            .options
            .claude_home
            .join("projects/-work-proj/parent.jsonl")
            .to_str()
            .unwrap()],
    )
    .unwrap();
    for tool in ["get_session", "read_transcript"] {
        let args = json!({"id":"child"});
        let expected = Query::from_sources(admitted(options.clone())).call_at(tool, &args, NOW);
        assert_eq!(
            Query::from_sources(admitted(options.clone())).call_selected_at(
                "local",
                "child",
                tool,
                &args,
                Some(&archive),
                crate::SessionSourceReadMode::ProviderOnly,
                NOW
            ),
            expected
        );
    }
    let before = archive.calls.load(Ordering::Relaxed);
    let error = Query::from_sources(admitted(options))
        .call_selected_at(
            "local",
            "outside",
            "get_session",
            &json!({"id":"outside"}),
            Some(&archive),
            crate::SessionSourceReadMode::ProviderOnly,
            NOW,
        )
        .unwrap_err();
    assert_eq!(error.code, "unknown_session");
    assert_eq!(
        archive.calls.load(Ordering::Relaxed),
        before,
        "excluded source caused archive I/O"
    );
}

#[test]
fn ordinary_focused_query_resolves_native_ids_after_scan_window_namespace_changes() {
    let mut home = Home::new("ordinary-native-alias");
    home.options.all = false;
    home.options.since = Duration::from_secs(6 * 3600);
    home.top("same-id", &[human("same-id", ts(1, 0), "old Claude owner")]);
    home.top(
        "claude:same-id",
        &[human("claude:same-id", ts(1, 0), "old literal qualifier")],
    );
    for id in ["same-id", "claude:same-id"] {
        fs::File::open(
            home.options
                .claude_home
                .join(format!("projects/-work-proj/{id}.jsonl")),
        )
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + Duration::from_millis((NOW - 86400000) as u64),
            ),
        )
        .unwrap();
    }
    home.lines(
        "codex/sessions/2026/09/24/rollout-same-id.jsonl",
        &[
            codex_line(ts(1, 0), "session_meta", json!({"id":"same-id"})),
            codex_line(
                ts(1, 1),
                "event_msg",
                json!({"type":"user_message","message":"current Codex owner"}),
            ),
        ],
    );
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    for tool in ["get_session", "read_transcript"] {
        let args = json!({"id":"same-id"});
        let expected = Query::from_sources(admitted(Options {
            scan_window: true,
            ..home.options.clone()
        }))
        .call_at(tool, &args, NOW)
        .unwrap();
        let mut ordinary = home.query();
        assert_eq!(ordinary.call_at(tool, &args, NOW).unwrap(), expected);
        assert!(
            ordinary.sources.machines[0]
                .1
                .work
                .lock()
                .unwrap()
                .events
                .is_none(),
            "raw native ID caused workspace Query preparation despite a qualified Catalog"
        );
    }
    let error = Query::from_sources(admitted(home.options.clone()))
        .call_selected_at(
            "local",
            "codex:same-id",
            "get_session",
            &json!({"id":"same-id"}),
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            NOW,
        )
        .unwrap_err();
    assert_eq!(
        error.code, "invalid_arguments",
        "host-selected Catalog ID validation was weakened"
    );
}

#[test]
fn ordinary_focused_query_keeps_omitted_resume_file_turns_truncated() {
    let mut home = Home::new("ordinary-resume-window");
    home.options.all = false;
    home.options.since = DEFAULT_WINDOW;
    let old = NOW - 60 * DAY_MS;
    home.top("first", &[human("first", iso(old), "Plan the release")]);
    fs::File::open(
        home.options
            .claude_home
            .join("projects/-work-proj/first.jsonl"),
    )
    .unwrap()
    .set_times(
        fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_millis(old as u64)),
    )
    .unwrap();
    let mut resumed = human("second", ts(1, 0), "Ship the release");
    resumed["session_id"] = json!("first");
    let mut shipped = assistant("second", ts(1, 1), vec![text("Shipped.")]);
    shipped["session_id"] = json!("first");
    home.top("second", &[resumed, shipped]);
    let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
    crate::model::observe_catalog(
        &home.options,
        &mut cache,
        &mut false,
        &mut crate::model::Texts::default(),
        NOW,
    )
    .unwrap();
    for tool in ["get_session", "read_transcript"] {
        let args = json!({"id":"second"});
        let expected = Query::from_sources(admitted(Options {
            scan_window: true,
            ..home.options.clone()
        }))
        .call_at(tool, &args, NOW)
        .unwrap();
        if tool == "get_session" {
            assert_eq!(expected["session"]["turns_truncated"], true);
        }
        let mut ordinary = home.query();
        assert_eq!(ordinary.call_at(tool, &args, NOW).unwrap(), expected);
        assert!(
            ordinary.sources.machines[0]
                .1
                .work
                .lock()
                .unwrap()
                .events
                .is_none()
        );
    }
}

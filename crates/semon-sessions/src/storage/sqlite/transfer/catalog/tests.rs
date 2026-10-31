use super::*;
use crate::storage::transfer::reported::tests::{root, seed_catalog, sqlite, stage};

#[test]
fn current_transfer225_unsupported_catalog_custody_preserves_capture_cursor_and_row() {
    let root = root();
    let source = sqlite(&root, "unsupported-custody-source");
    let _ = seed_catalog(&source, &root);
    let length = catalog_rows::MAX_CATALOG_ROW_BYTES as i64 + 1;
    super::super::fixture_transaction(&source, |db| {
        db.execute(
            "INSERT INTO meta(namespace,key,value) VALUES(?1,'history_original_header:unsupported',CAST(zeroblob(?2) AS TEXT))",
            params![source.namespace.as_str(), length],
        ).unwrap();
        Ok(())
    }).unwrap();
    let db = rusqlite::Connection::open(source.backend.sqlite_database().unwrap()).unwrap();
    let operation = [182; 32];
    let cursor_key = transfer::key(&operation, "capture");
    let mut previous: Option<String> = None;
    loop {
        match source.transfer_inbox().capture(&operation) {
            Ok(transfer::CaptureProgress::Capturing { .. }) => {
                previous = Some(
                    db.query_row(
                        "SELECT value FROM meta WHERE namespace=?1 AND key=?2",
                        params![source.namespace.as_str(), cursor_key],
                        |row| row.get(0),
                    )
                    .unwrap(),
                );
            }
            Err(StorageError::HistoryRetentionBounds) => break,
            other => panic!("unsupported custody must stop explicitly: {other:?}"),
        }
    }
    let unchanged: String = db
        .query_row(
            "SELECT value FROM meta WHERE namespace=?1 AND key=?2",
            params![source.namespace.as_str(), cursor_key],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(Some(unchanged), previous);
    let retained: i64 = db.query_row(
        "SELECT octet_length(value) FROM meta WHERE namespace=?1 AND key='history_original_header:unsupported'",
        [source.namespace.as_str()], |row| row.get(0),
    ).unwrap();
    assert_eq!(retained, length);
    assert_eq!(
        source.transfer_inbox().capture(&operation),
        Err(StorageError::HistoryRetentionBounds)
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_sqlite_real_large_presentation_reverse_without_raw() {
    use crate::storage::transfer::catalog_rows::tests::{
        assert_catalog_categories_equal, assert_provider_transcript,
    };
    use crate::storage::transfer::reported::tests::apply;
    let root = root();
    let source = sqlite(&root, "real-large-source");
    let target = sqlite(&root, "real-large-target");
    let reverse = sqlite(&root, "real-large-reverse");
    let (options, key, id, host) = seed_large_presentation(&source, &root);
    assert_provider_transcript(&source, &options, &host, &key);
    let manifest = stage(&source, &target, &[178; 32]);
    let mut presentation_parts = 0;
    for sequence in 1..=source
        .transfer_inbox()
        .checkpoint(&manifest)
        .unwrap()
        .sequence
    {
        let page = source
            .transfer_inbox()
            .staged_page(&manifest, sequence)
            .unwrap();
        assert!(page.encoded().unwrap().len() <= transfer::MAX_PAGE_BYTES);
        for record in page.records {
            if let TransferRecord::CatalogRow {
                kind: CatalogRowKind::Presentation,
                key,
                parts,
                ..
            } = record
                && key[0].as_str() == Some(id.as_str())
            {
                assert!(parts > 1);
                presentation_parts += 1;
            }
        }
    }
    assert!(presentation_parts > 1);
    std::fs::remove_dir_all(&options.claude_home).unwrap();
    apply(&target, &manifest);
    target
        .transfer_inbox()
        .admit_catalog_from(&source.transfer_inbox(), &manifest, &options)
        .unwrap();
    assert_provider_transcript(&target, &options, &host, &key);
    let reverse_manifest = stage(&target, &reverse, &[179; 32]);
    assert_catalog_categories_equal(&manifest, &reverse_manifest);
    apply(&reverse, &reverse_manifest);
    reverse
        .transfer_inbox()
        .admit_catalog_from(&target.transfer_inbox(), &reverse_manifest, &options)
        .unwrap();
    assert_provider_transcript(&reverse, &options, &host, &key);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_transfer225_sqlite_large_catalog_restart_replay_and_final_rollback() {
    let root = root();
    let source = sqlite(&root, "large-catalog-source");
    let target = sqlite(&root, "large-catalog-target");
    let _ = seed_catalog(&source, &root);
    let value = serde_json::to_string(&"\0\\\"界".repeat(160_000)).unwrap();
    assert!(value.len() > transfer::MAX_PAGE_BYTES);
    // Payload-only custody fixtures still use the adapter's restored owner and
    // ordinary guarded transaction; no publication or proof authority is minted.
    let escaped = serde_json::to_string(&"\\".repeat(250_000)).unwrap();
    assert!(escaped.len() < transfer::MAX_PAGE_BYTES);
    super::super::fixture_transaction(&source, |db| {
        db.execute(
            "INSERT INTO meta(namespace,key,value) VALUES(?1,'history_transfer:large-row',?2)",
            params![source.namespace.as_str(), value],
        )
        .unwrap();
        // Its raw cells fit one page, but escaped JSON and wire byte arrays span
        // several pages. The following row must wait for this row's final part.
        for (key, metadata) in [("00-escaped", escaped.as_str()), ("01-following", "null")] {
            db.execute(
                "INSERT INTO meta(namespace,key,value) VALUES(?1,?2,?3)",
                params![
                    source.namespace.as_str(),
                    format!("history_transfer:{key}"),
                    metadata
                ],
            )
            .unwrap();
        }
        Ok(())
    })
    .unwrap();
    let manifest = stage(&source, &target, &[177; 32]);
    let source_inbox = source.transfer_inbox();
    let count = source_inbox.checkpoint(&manifest).unwrap().sequence;
    let target_path = match &target.backend {
        crate::storage::StoreBackend::Sqlite { database } => database,
        #[cfg(feature = "postgres")]
        _ => unreachable!(),
    };
    let db = rusqlite::Connection::open(target_path).unwrap();
    let stored = |db: &rusqlite::Connection| {
        db.query_row(
            "SELECT value FROM meta WHERE namespace=?1 AND key='history_transfer:large-row'",
            [target.namespace.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .unwrap()
    };
    let mut saw_partial = false;
    let mut saw_final = false;
    let mut escaped_partial_pages = 0;
    for sequence in 1..=count {
        let page = source_inbox.staged_page(&manifest, sequence).unwrap();
        assert!(page.encoded().unwrap().len() <= transfer::MAX_PAGE_BYTES);
        let fragments = page
            .records
            .iter()
            .filter_map(|record| match record {
                TransferRecord::CatalogRow {
                    kind: CatalogRowKind::HistoryProgress,
                    part,
                    parts,
                    ..
                } => Some((*part, *parts)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let final_row = fragments.iter().any(|(part, parts)| part + 1 == *parts);
        let final_large_row = page.records.iter().any(|record| matches!(record,
            TransferRecord::CatalogRow { kind: CatalogRowKind::HistoryProgress, key, part, parts, .. }
                if key[0].as_str() == Some("large-row") && part + 1 == *parts
        ));
        let escaped_partial = page.records.iter().any(|record| matches!(record,
            TransferRecord::CatalogRow { kind: CatalogRowKind::HistoryProgress, key, part, parts, .. }
                if key[0].as_str() == Some("00-escaped") && part + 1 < *parts
        ));
        if final_large_row {
            let receipt = transfer::key(&manifest.identity(), &format!("applied:{sequence}"));
            db.execute_batch(&format!(
                "CREATE TRIGGER fail_catalog_final_receipt BEFORE INSERT ON meta WHEN NEW.key='{receipt}' BEGIN SELECT RAISE(ABORT,'injected final fragment receipt failure'); END"
            )).unwrap();
            assert!(
                target
                    .transfer_inbox()
                    .apply_page(&manifest, sequence)
                    .is_err()
            );
            assert!(stored(&db).is_none());
            let receipt_count: i64 = db
                .query_row(
                    "SELECT count(*) FROM meta WHERE namespace=?1 AND key=?2",
                    params![target.namespace.as_str(), receipt],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(receipt_count, 0);
            db.execute_batch("DROP TRIGGER fail_catalog_final_receipt")
                .unwrap();
            saw_final = true;
        }
        let checkpoint = target
            .transfer_inbox()
            .apply_page(&manifest, sequence)
            .unwrap();
        // New handles replay exact page receipts, including after restart and
        // after the final logical row was already inserted.
        assert_eq!(
            target
                .transfer_inbox()
                .apply_page(&manifest, sequence)
                .unwrap(),
            checkpoint
        );
        if !fragments.is_empty() && !final_row {
            assert!(stored(&db).is_none());
            saw_partial = true;
        }
        if final_large_row {
            assert_eq!(stored(&db).as_deref(), Some(value.as_str()));
        }
        escaped_partial_pages += usize::from(escaped_partial);
    }
    assert!(saw_partial && saw_final);
    assert!(escaped_partial_pages > 1);
    let stored_escaped: String = db
        .query_row(
            "SELECT value FROM meta WHERE namespace=?1 AND key='history_transfer:00-escaped'",
            [target.namespace.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_escaped, escaped);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn seed_large_presentation(
    source: &StoreConfig,
    root: &std::path::Path,
) -> (
    crate::Options,
    String,
    String,
    crate::storage::transfer::reported::tests::selected_receipts::AuthorizedOriginals,
) {
    use crate::storage::native_events::NativeEventStore;
    use crate::storage::transfer::reported::tests::{
        begin_active_catalog, cache, finish_catalog, seed_catalog,
        selected_receipts::{AuthorizedOriginals, originals},
    };
    use serde_json::json;
    const CHILDREN: usize = 16;
    const CHILD_TURNS: usize = 200;
    let (options, initial) = seed_catalog(source, root);
    let parent_key = initial[0].key.clone();
    let mut host = AuthorizedOriginals::default();
    originals(source, &options, &mut host);
    let db = rusqlite::Connection::open(source.backend.sqlite_database().unwrap()).unwrap();
    let original = |db: &rusqlite::Connection| -> (String, String, String) {
        db.query_row(
            "SELECT g.id,json_array(g.id,g.session_key,g.publication,g.proof,g.admission,g.stream,g.metadata,g.query_recipe,g.slots,g.prefix),p.metadata FROM storage_proof_refs f JOIN storage_proof_generations g USING(namespace,id,session_key) JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current'",
            params![source.namespace.as_str(), parent_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap()
    };
    let (first_id, first_proof, first_presentation) = original(&db);
    let proof_sources = |id: &str| -> String {
        db.query_row(
            "SELECT json_group_array(json_array(path,ledger)) FROM (SELECT path,ledger FROM storage_proof_sources WHERE namespace=?1 AND id=?2 AND session_key=?3 ORDER BY path)",
            params![source.namespace.as_str(), id, parent_key], |row| row.get(0),
        ).unwrap()
    };
    let first_sources = proof_sources(&first_id);
    assert_ne!(first_sources, "[]");
    let old_input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/sanitized/catalog.jsonl".into(),
    };
    let old_path = old_input.full_path(&options);
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/resumed/catalog.jsonl".into(),
    };
    let parent_path = input.full_path(&options);
    // Use the ordinary withdrawal/successor lifecycle: a same-path append
    // legitimately clears history. Preserve the first native prefix under its
    // original host authority before the same native identity resumes.
    let mut parent = std::fs::read_to_string(&old_path).unwrap();
    std::fs::remove_file(&old_path).unwrap();
    let mut cache = cache(source);
    let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    cache
        .retire_scoped_sources(
            &options,
            std::slice::from_ref(&old_input),
            &std::collections::BTreeSet::new(),
            &crate::facts::MachineFacts::Local,
        )
        .unwrap();
    begin_active_catalog(
        &mut cache,
        &[old_path.to_str().unwrap().to_owned()],
        &runtime,
        true,
    );
    assert!(
        finish_catalog(&mut cache, &options, std::slice::from_ref(&old_input)).is_empty()
    );
    let mut inputs = vec![input];
    let children = (0..CHILDREN)
        .map(|index| format!("large-child-{index:02}"))
        .collect::<Vec<_>>();
    let timestamp = |second: usize| {
        format!("2026-10-01T00:{:02}:{:02}Z", second / 60, second % 60)
    };
    let calls = children.iter().enumerate().map(|(index, _)| {
        json!({"type":"tool_use","id":format!("spawn-{index:02}"),"name":"Agent","input":{"description":format!("sanitized task {index}"),"prompt":"sanitized 0"}})
    }).collect::<Vec<_>>();
    let call_record = json!({"type":"assistant","sessionId":"catalog","uuid":"parent-calls","parentUuid":"u129","timestamp":"2026-10-01T00:00:01Z","cwd":"/sanitized","message":{"id":"parent-calls","role":"assistant","model":"claude-test","content":calls}});
    parent.push_str(&format!("{call_record}\n"));
    let mut previous = "parent-calls".to_owned();
    for (index, child) in children.iter().enumerate() {
        let call = format!("spawn-{index:02}");
        let result_id = format!("parent-result-{index:02}");
        let result = json!({"type":"user","sessionId":"catalog","uuid":result_id,"parentUuid":previous,"timestamp":"2026-10-01T00:00:02Z","toolUseResult":{"status":"async_launched","agentId":child},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":call,"content":"launched","is_error":false}]}});
        parent.push_str(&format!("{result}\n"));
        previous = result_id;
        let child_input = crate::Input {
            root: crate::InputRoot::Claude,
            path: format!("projects/resumed/catalog/subagents/agent-{child}.jsonl"),
        };
        let child_path = child_input.full_path(&options);
        std::fs::create_dir_all(child_path.parent().unwrap()).unwrap();
        let mut body = String::new();
        let mut previous: Option<String> = None;
        for turn in 0..CHILD_TURNS {
            let user_id = format!("{child}-u{turn}");
            let assistant_id = format!("{child}-a{turn}");
            for record in [
                json!({"type":"user","sessionId":"catalog","uuid":user_id,"parentUuid":previous,"timestamp":timestamp(3 + 2 * turn),"cwd":"/sanitized","message":{"role":"user","content":format!("sanitized {turn}")}}),
                json!({"type":"assistant","sessionId":"catalog","uuid":assistant_id,"parentUuid":user_id,"timestamp":timestamp(4 + 2 * turn),"cwd":"/sanitized","message":{"id":assistant_id,"role":"assistant","model":"claude-test","content":[{"type":"text","text":format!("sanitized answer {turn}")}]}}),
            ] {
                body.push_str(&format!("{record}\n"));
            }
            previous = Some(assistant_id);
        }
        // A child-owned native hand-back gives each canonical spawn its own
        // result preview owner instead of putting all results in the parent.
        let handback = json!({"type":"assistant","sessionId":"catalog","uuid":format!("{child}-handback"),"parentUuid":previous,"timestamp":timestamp(3 + 2 * CHILD_TURNS),"cwd":"/sanitized","message":{"id":format!("{child}-handback"),"role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":format!("{child}-return"),"name":"SubagentHandback","input":{"message":format!("sanitized completed child {index}")}}]}});
        body.push_str(&format!("{handback}\n"));
        std::fs::write(&child_path, body).unwrap();
        std::fs::write(
            child_path.with_extension("meta.json"),
            json!({"agentType":"general-purpose","description":format!("sanitized task {index}"),"toolUseId":call}).to_string(),
        ).unwrap();
        inputs.push(child_input);
    }
    std::fs::write(&parent_path, parent).unwrap();
    let paths = inputs.iter().map(|input| input.full_path(&options)).collect::<Vec<_>>();
    for (index, path) in paths.iter().enumerate() {
        cache.ingest(&format!("{:064x}", 1000 + index), path, "claude").unwrap();
    }
    cache.flush_cold_writes().unwrap();
    let paths = paths.iter().map(|path| path.to_str().unwrap().to_owned()).collect::<Vec<_>>();
    let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    begin_active_catalog(&mut cache, &paths, &runtime, true);
    let rows = finish_catalog(&mut cache, &options, &inputs);
    let parent = rows.iter().find(|row| row.key == parent_key).unwrap();
    let (id, metadata, slots, presentation): (String, String, String, String) = db.query_row(
        "SELECT g.id,g.metadata,g.slots,p.metadata FROM storage_proof_refs f JOIN storage_proof_generations g USING(namespace,id,session_key) JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current'",
        params![source.namespace.as_str(), parent_key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    let query_recipe: Option<String> = db.query_row(
        "SELECT query_recipe FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3",
        params![source.namespace.as_str(), id, parent_key], |row| row.get(0),
    ).unwrap();
    let metadata_recipe_bytes = metadata.len() + query_recipe.as_ref().map_or(0, String::len);
    let witness: crate::storage::selected::PresentationWitness = serde_json::from_str(&presentation).unwrap();
    let context = serde_json::to_value(&witness.context).unwrap();
    let relative = context["relative"].as_object().unwrap();
    let previews = context["preview_catalog"].as_object().unwrap();
    let edges = context["edge"].as_object().unwrap().values().map(|value| {
        serde_json::from_str::<serde_json::Value>(value.as_str().unwrap()).unwrap()
    }).collect::<Vec<_>>();
    let family_bytes = relative.values().map(|value| value.as_str().unwrap().len()).sum::<usize>();
    let preview_bytes = previews.values().map(|value| value.as_str().unwrap().len()).sum::<usize>();
    let context_bytes = serde_json::to_vec(&witness.context).unwrap().len();
    let family = crate::storage::selected::selected_family(
        &witness.context,
        &parent_key,
        &mut |relative, _| witness.relative_statuses[&relative.key].clone(),
    ).unwrap();
    let aggregate_bytes: i64 = db.query_row(
        "SELECT sum(octet_length(metadata)) FROM storage_proof_presentations WHERE namespace=?1 AND id=?2",
        params![source.namespace.as_str(), id], |row| row.get(0),
    ).unwrap();
    let source_paths = rows.iter().flat_map(|row| row.sources.iter().map(|source| source.path.clone())).collect::<std::collections::BTreeSet<_>>();
    let child_sizes = children.iter().map(|child| {
        db.query_row(
            "SELECT octet_length(g.metadata),octet_length(g.slots) FROM storage_proof_refs f JOIN storage_proof_generations g USING(namespace,id,session_key) WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current'",
            params![source.namespace.as_str(), child], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)),
        ).unwrap()
    }).collect::<Vec<_>>();
    eprintln!(
        "large_catalog_real children={} sources={} metadata={} metadata_recipe={} slots={} presentation={} context={} family_owners={} family_bytes={} family_state={} preview_owners={} preview_bytes={} handoffs={} publication_presentations={} child_cells={child_sizes:?}",
        children.len(), source_paths.len(), metadata.len(), metadata_recipe_bytes, slots.len(), presentation.len(), context_bytes,
        relative.len(), family_bytes, family["state"], previews.len(), preview_bytes, edges.len(), aggregate_bytes,
    );
    assert_eq!(rows.len(), CHILDREN + 1);
    assert_eq!(source_paths.len(), CHILDREN + 1);
    assert_eq!(family["state"], "complete");
    assert_eq!(family["runs"].as_array().unwrap().len(), CHILDREN);
    assert_eq!(relative.len(), CHILDREN + 1);
    assert!(relative.values().all(|value| value.as_str().unwrap().len() <= 65536));
    assert!(family_bytes <= transfer::MAX_PAGE_BYTES);
    assert_eq!(previews.len(), CHILDREN + 1);
    assert_eq!(edges.len(), CHILDREN);
    for (index, child) in children.iter().enumerate() {
        let row = rows.iter().find(|row| &row.key == child).unwrap();
        assert_eq!(row.parent.as_deref(), Some(parent_key.as_str()));
        assert_eq!(row.sources.len(), 1);
        assert_eq!(row.sources[0].native_id, *child);
        let role = row.sources[0].native_role.as_ref().unwrap();
        assert_eq!(role["Agent"]["parent"], parent_key);
        assert_eq!(role["Agent"]["tool_use_id"], format!("spawn-{index:02}"));
        assert!(row.sources[0].path.ends_with(format!("catalog/subagents/agent-{child}.jsonl")));
        let key = serde_json::to_string(&(child,)).unwrap();
        assert!(previews[&key].is_string(), "child must own a canonical result preview");
        let edge = edges.iter().find(|edge| edge["to"].as_str() == Some(child.as_str())).unwrap();
        assert_eq!(edge["kind"], "spawn");
        assert_eq!(edge["from"], parent_key);
        assert_eq!(edge["brief_session"], parent_key);
        assert_eq!(edge["result_session"], *child);
    }
    assert_eq!(parent.sources.len(), 1);
    assert_eq!(parent.sources[0].native_id, "catalog");
    assert_eq!(parent.sources[0].path, parent_path);
    assert!(child_sizes.iter().all(|(metadata, slots)| *metadata <= 65536 && *slots <= transfer::MAX_PAGE_BYTES as i64));
    assert!(metadata_recipe_bytes <= transfer::MAX_PAGE_BYTES);
    assert!(query_recipe.is_none(), "non-null immutable recipe activation is not qualified here");
    assert!(slots.len() <= transfer::MAX_PAGE_BYTES);
    assert!(context_bytes > transfer::MAX_PAGE_BYTES);
    assert!(context_bytes <= 8 * transfer::MAX_PAGE_BYTES);
    assert!(presentation.len() > transfer::MAX_PAGE_BYTES);
    assert!(presentation.len() <= 8 * transfer::MAX_PAGE_BYTES);
    assert!(aggregate_bytes <= 8 * transfer::MAX_PAGE_BYTES as i64);
    assert_ne!(id, first_id);
    let retained: (String, String) = db.query_row(
        "SELECT json_array(g.id,g.session_key,g.publication,g.proof,g.admission,g.stream,g.metadata,g.query_recipe,g.slots,g.prefix),p.metadata FROM storage_proof_refs f JOIN storage_proof_generations g USING(namespace,id,session_key) JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='history' AND f.id=?3",
        params![source.namespace.as_str(), parent_key, first_id], |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert_eq!(retained, (first_proof, first_presentation));
    assert_eq!(proof_sources(&first_id), first_sources);
    for row in &rows {
        for original in &row.sources {
            let ledger: String = db.query_row(
                "SELECT ledger FROM storage_proof_sources WHERE namespace=?1 AND id=?2 AND session_key=?3 AND path=?4",
                params![source.namespace.as_str(), id, row.key, original.path.to_string_lossy()],
                |row| row.get(0),
            ).unwrap();
            assert_eq!(ledger, crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(original)));
        }
    }
    let (current, history, nonnull): (i64, i64, i64) = db.query_row(
        "SELECT (SELECT count(*) FROM storage_proof_refs WHERE namespace=?1 AND kind='current'),(SELECT count(*) FROM storage_proof_refs WHERE namespace=?1 AND kind='history'),(SELECT count(*) FROM storage_proof_generations WHERE namespace=?1 AND query_recipe IS NOT NULL)",
        [source.namespace.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).unwrap();
    assert_eq!((current, history, nonnull), ((CHILDREN + 1) as i64, 1, 0));
    drop(db);
    originals(source, &options, &mut host);
    (options, parent_key, id, host)
}

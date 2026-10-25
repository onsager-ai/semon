use super::*;

fn bytes(
    lease: &crate::storage::sqlite::proof_generations::Lease,
    options: &crate::Options,
    host: &HostSourceBinding,
    query: &str,
) -> Vec<u8> {
    let (snapshot, reply) = read_bound(
        lease,
        options,
        host,
        query,
        None,
        SessionSourceReadMode::LocalOrPinnedProvider,
        &crate::SelectedReadControl::default(),
    )
    .unwrap();
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
    assert_eq!(snapshot.namespace, lease.snapshot().namespace);
    assert_eq!(snapshot.admission, lease.snapshot().admission);
    assert_eq!(snapshot.stream_epoch, lease.snapshot().stream_epoch);
    assert_eq!(snapshot.proof_generation, lease.snapshot().proof_generation);
    assert_eq!(snapshot.publication_generation, lease.snapshot().publication_generation);
    assert_eq!(snapshot.consumed_prefix, lease.snapshot().consumed_prefix);
    reply.body
}

#[test]
fn configured_semantic_selected60_facts_change_preserves_leased_bytes_and_fresh_publication() {
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let path = input.full_path(options);
    let facts_path = options.facts.as_ref().unwrap();
    let mut facts = crate::facts::read_facts(facts_path).unwrap();
    facts.codex_rollouts = Some([input.path.clone()].into_iter().collect());
    crate::facts::write_facts(facts_path, &facts).unwrap();
    cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected60");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    let lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding { source_key: "source-a".into(), snapshot: lease.snapshot().clone() };
    let query = format!("sid={key}&after=0&limit=1");
    let original = bytes(&lease, options, &host, &query);
    let original_value: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(original_value["identity"]["machine_label"], facts.hostname);
    cache.finish_semantic_stage(batch.epoch).unwrap();

    // An independent ordinary producer/public read is the live-facts oracle.
    // Its explicitly separate store cannot become an owned-reader fallback.
    let mut oracle_options = options.clone();
    oracle_options.cache = f.root.join("selected60-ordinary-oracle");
    let mut ordinary = EventCache::open_scoped(&oracle_options.cache).unwrap();
    let prepared = crate::model::prepare_sources(&oracle_options, &mut ordinary, std::slice::from_ref(input), 1_790_000_000_000).unwrap();
    ordinary.publish_catalog_projection(&crate::slot_projection::Publication { rows: &prepared.rows, transcripts: Some(&prepared.transcripts) }, prepared.base_generation.as_deref()).unwrap();
    let before = body(crate::session_transcript_range_with_mode(&oracle_options, "source-a", &query, None, SessionSourceReadMode::LocalOrPinnedProvider));
    assert_eq!(before["identity"]["machine_label"], facts.hostname);

    // No Raw mutation: change independent reported runtime facts and the facts
    // availability observation. The old witness must never be reconstructed.
    facts.hostname = "independent-facts-change".into();
    facts.offline_since = Some(1_790_000_100_000);
    crate::facts::write_facts(facts_path, &facts).unwrap();
    assert_eq!(bytes(&lease, options, &host, &query), original);
    let current = body(crate::session_transcript_range_with_mode(&oracle_options, "source-a", &query, None, SessionSourceReadMode::LocalOrPinnedProvider));
    assert_eq!(current["identity"]["machine_label"], "independent-facts-change");
    assert_ne!(before["identity"]["facts_observation"], current["identity"]["facts_observation"]);

    let (next, next_model) = model_ready(&f, &mut cache, options, input, "selected60-next-runtime");
    assert_eq!(cache.publish_semantic_catalog(&next, next_model.prepared()).unwrap().0, Outcome::Written);
    let fresh_id = runtime.current_generation(key).unwrap();
    assert_ne!(fresh_id, id);
    let fresh = runtime.selected_generation(key, &fresh_id, &"8".repeat(64), 60_000).unwrap();
    let fresh_host = HostSourceBinding { source_key: "source-a".into(), snapshot: fresh.snapshot().clone() };
    let fresh_bytes = bytes(&fresh, options, &fresh_host, &query);
    let fresh_value: Value = serde_json::from_slice(&fresh_bytes).unwrap();
    assert_eq!(fresh_value["identity"]["machine_label"], "independent-facts-change");
    assert_ne!(fresh_bytes, original);
    assert_eq!(bytes(&lease, options, &host, &query), original);

    // Even unavailable originals for the non-rebuildable Facts observation do
    // not cause a leased read to regenerate it from today's live state.
    fs::rename(facts_path, facts_path.with_extension("unavailable")).unwrap();
    assert_eq!(bytes(&lease, options, &host, &query), original);
    assert_eq!(bytes(&fresh, options, &fresh_host, &query), fresh_bytes);
    assert!(!EventCache::path(&options.cache).exists());
}

#[test]
fn configured_semantic_selected70_real_native_multi_window_range_oracle() {
    use std::io::Write;

    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let path = input.full_path(options);
    let mut raw = fs::OpenOptions::new().append(true).open(&path).unwrap();
    for (index, text) in ["window one", "window two", "window three"].into_iter().enumerate() {
        writeln!(
            raw,
            "{}",
            json!({
                "timestamp": format!("2026-10-01T00:0{}:00Z", index + 2),
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                },
            })
        )
        .unwrap();
    }
    drop(raw);
    cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected70-windows");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    let lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding { source_key: "source-a".into(), snapshot: lease.snapshot().clone() };

    let mut ordinary = EventCache::open_scoped(&options.cache).unwrap();
    let prepared = crate::model::prepare_sources(
        options,
        &mut ordinary,
        std::slice::from_ref(input),
        1_790_000_000_000,
    )
    .unwrap();
    ordinary
        .publish_catalog_projection(
            &crate::slot_projection::Publication { rows: &prepared.rows, transcripts: Some(&prepared.transcripts) },
            prepared.base_generation.as_deref(),
        )
        .unwrap();
    let db = SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();

    for after in 0..3 {
        let query = format!("sid={key}&after={after}&limit=1");
        let actual = read(&lease, options, &host, &query);
        let mut expected = body(crate::session_transcript_range_with_mode(
            options,
            &host.source_key,
            &query,
            None,
            SessionSourceReadMode::LocalOrPinnedProvider,
        ));
        publication_oracle(&mut expected, &db, "source-a");
        assert_eq!(actual, expected, "complete native range reply at window {after}");
        assert_eq!(actual["identity"]["generation"], lease.snapshot().publication_generation);
        assert_eq!(actual["range"]["first"], after);
        assert_eq!(actual["range"]["end"], after + 1);
    }
    cache.finish_semantic_stage(batch.epoch).unwrap();
}

#[test]
fn configured_semantic_selected65_private_witness_is_bound_independently_of_rows() {
    use crate::storage::selected::SelectedSnapshotStore;
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    cache.ingest(&"a".repeat(64), &input.full_path(options), "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected65-private");
    for row in &model.prepared().rows {
        assert!(row.query.as_ref().unwrap().get("selected_presentation_observations").is_none());
    }
    let db = SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();
    // This admitted fixture connection deliberately corrupts the staged
    // witness using the same writer role as Stage, with namespace guards kept.
    db.create_scalar_function(
        "semon_semantic_writer",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    ).unwrap();
    let saved: String = db.query_row(
        "SELECT projections FROM storage_semantic_model_batches WHERE namespace='source-a'",
        [], |row| row.get(0),
    ).unwrap();
    let mut altered: Value = serde_json::from_str(&saved).unwrap();
    assert!(altered[1]["sources"].as_object().unwrap().len() <= 128 * 64);
    assert!(altered[1].get("roots").is_none());
    assert!(altered[1].get("current_codex").is_none());
    altered[1]["machine_label"] = json!("tampered-private-input");
    db.execute(
        "UPDATE storage_semantic_model_batches SET projections=?1 WHERE namespace='source-a'",
        [serde_json::to_string(&altered).unwrap()],
    ).unwrap();
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Conflict);
    db.execute(
        "UPDATE storage_semantic_model_batches SET projections=?1 WHERE namespace='source-a'",
        [saved],
    ).unwrap();
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    let lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding { source_key: "source-a".into(), snapshot: lease.snapshot().clone() };
    let selected = lease.open_selected(options, &host, &crate::SelectedReadControl::default()).unwrap();
    assert_eq!(selected.witness.observations.runtime.as_deref(), model.prepared().rows[0].query.as_ref().unwrap()["runtime"].as_str());
    let stored: String = db.query_row("SELECT metadata FROM storage_proof_presentations WHERE namespace='source-a' AND id=?1 AND session_key=?2", params![id.0, key], |row| row.get(0)).unwrap();
    assert!(serde_json::from_str::<Value>(&stored).unwrap().get("observations").is_none(), "shared observations must be stored once per publication");
    for name in ["catalog_generation", "catalog_completeness", "catalog_observed_at"] {
        let committed: String = db.query_row(
            "SELECT value FROM main.meta WHERE namespace='source-a' AND key=?1",
            [name], |row| row.get(0),
        ).unwrap();
        assert_eq!(selected.witness.catalog.get(name), Some(&committed));
        if name == "catalog_generation" {
            assert_eq!(committed, lease.snapshot().publication_generation);
        }
    }
    let reply = read(&lease, options, &host, &format!("sid={key}&after=0&limit=1"));
    assert_eq!(reply["identity"]["generation"], lease.snapshot().publication_generation);
}

#[test]
fn configured_semantic_selected65_witness_and_manifest_tamper_refuse_before_raw() {
    struct Count(std::sync::atomic::AtomicUsize);
    impl crate::SessionSourceReader for Count {
        fn read_range(&self, _: &crate::SessionSourceRef, _: Option<&str>, _: u64, _: usize) -> std::io::Result<crate::SessionSourceRange> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(std::io::Error::other("unexpected Raw access"))
        }
    }
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    cache.ingest(&"a".repeat(64), &input.full_path(options), "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected65-tamper");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    let lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding { source_key: "source-a".into(), snapshot: lease.snapshot().clone() };
    let query = format!("sid={key}&after=0&limit=1");
    assert!(!read(&lease, options, &host, &query)["entries"].as_array().unwrap().is_empty());
    let db = SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();
    let saved: String = db.query_row(
        "SELECT metadata FROM storage_proof_presentations WHERE namespace='source-a' AND session_key=?1",
        [key], |row| row.get(0),
    ).unwrap();
    let mut altered: Value = serde_json::from_str(&saved).unwrap();
    altered["catalog"]["catalog_observed_at"] = json!("1");
    db.execute_batch("DROP TRIGGER storage_proof_presentation_immutable").unwrap();
    db.execute("UPDATE storage_proof_presentations SET metadata=?1 WHERE namespace='source-a' AND session_key=?2", params![serde_json::to_string(&altered).unwrap(), key]).unwrap();
    db.execute_batch("CREATE TRIGGER storage_proof_presentation_immutable BEFORE UPDATE ON storage_proof_presentations BEGIN SELECT RAISE(ABORT,'immutable presentation'); END;").unwrap();
    let provider = Count(std::sync::atomic::AtomicUsize::new(0));
    let refuse = || {
        assert!(read_bound(&lease, options, &host, &query, Some(&provider), SessionSourceReadMode::ProviderOnly, &crate::SelectedReadControl::default()).is_err());
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    };
    refuse();
    db.execute_batch("DROP TRIGGER storage_proof_presentation_immutable").unwrap();
    db.execute("UPDATE storage_proof_presentations SET metadata=?1 WHERE namespace='source-a' AND session_key=?2", params![saved, key]).unwrap();
    db.execute_batch("CREATE TRIGGER storage_proof_presentation_immutable BEFORE UPDATE ON storage_proof_presentations BEGIN SELECT RAISE(ABORT,'immutable presentation'); END;").unwrap();
    let manifest: String = db.query_row("SELECT manifest FROM storage_proof_presentation_roots WHERE namespace='source-a' AND publication=?1", [&lease.snapshot().publication_generation], |row| row.get(0)).unwrap();
    db.execute_batch("DROP TRIGGER storage_proof_presentation_root_immutable").unwrap();
    let mut altered_manifest: Value = serde_json::from_str(&manifest).unwrap();
    altered_manifest["observations"]["machine_label"] = json!("tampered-shared-observation");
    db.execute("UPDATE storage_proof_presentation_roots SET manifest=?1 WHERE namespace='source-a' AND publication=?2", params![serde_json::to_string(&altered_manifest).unwrap(), lease.snapshot().publication_generation]).unwrap();
    db.execute_batch("CREATE TRIGGER storage_proof_presentation_root_immutable BEFORE UPDATE ON storage_proof_presentation_roots BEGIN SELECT RAISE(ABORT,'immutable presentation root'); END;").unwrap();
    refuse();
    db.execute_batch("DROP TRIGGER storage_proof_presentation_root_immutable").unwrap();
    db.execute("UPDATE storage_proof_presentation_roots SET manifest=?1 WHERE namespace='source-a' AND publication=?2", params![manifest, lease.snapshot().publication_generation]).unwrap();
    db.execute_batch("CREATE TRIGGER storage_proof_presentation_root_immutable BEFORE UPDATE ON storage_proof_presentation_roots BEGIN SELECT RAISE(ABORT,'immutable presentation root'); END;").unwrap();
    assert!(!read(&lease, options, &host, &query)["entries"].as_array().unwrap().is_empty());
}

#[test]
fn configured_semantic_selected65_gc_requires_both_history_and_lease_to_be_gone() {
    use std::io::Write;
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let path = input.full_path(options);
    cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected65-gc");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    let mut lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    cache.finish_semantic_stage(batch.epoch).unwrap();
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(concat!(
        "{\"timestamp\":\"2026-10-01T00:03:00Z\",\"type\":\"response_item\",",
        "\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"generation B\"}]}}\n"
    ).as_bytes()).unwrap();
    cache.ingest(&"b".repeat(64), &path, "codex").unwrap();
    let (next, next_model) = model_ready(&f, &mut cache, options, input, "selected65-gc");
    assert_eq!(cache.publish_semantic_catalog(&next, next_model.prepared()).unwrap().0, Outcome::Written);
    assert_ne!(runtime.current_generation(key).unwrap(), id);
    let mut db = SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();
    let history: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM storage_proof_refs WHERE namespace='source-a' AND session_key=?1 AND id=?2 AND kind='history')", params![key, id.0], |row| row.get(0)).unwrap();
    assert!(history);
    // Test-only retirement of the History reference, independently of lease
    // release. This does not alter production retention policy or Raw custody.
    db.execute("DELETE FROM storage_proof_refs WHERE namespace='source-a' AND session_key=?1 AND id=?2 AND kind='history'", params![key, id.0]).unwrap();
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    crate::storage::sqlite::proof_generations::collect(&tx, "source-a", 64).unwrap();
    tx.commit().unwrap();
    let count = |db: &Connection| -> i64 {
        db.query_row("SELECT count(*) FROM storage_proof_generations WHERE namespace='source-a' AND session_key=?1 AND id=?2", params![key, id.0], |row| row.get(0)).unwrap()
    };
    assert_eq!(count(&db), 1, "active lease alone preserves A");
    lease.release().unwrap();
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    crate::storage::sqlite::proof_generations::collect(&tx, "source-a", 64).unwrap();
    tx.commit().unwrap();
    assert_eq!(count(&db), 0, "unreferenced A is collected");
    assert!(runtime.selected_generation(key, &id, &"8".repeat(64), 60_000).is_err());
    assert!(path.exists());
}

#[test]
fn configured_semantic_selected65_capture_unavailable_facts_is_per_row_unavailable() {
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    cache.ingest(&"a".repeat(64), &input.full_path(options), "codex").unwrap();
    let (_, model) = model_ready(&f, &mut cache, options, input, "selected65-unavailable");
    let path = options.facts.as_ref().unwrap();
    fs::rename(path, path.with_extension("unavailable")).unwrap();
    let mut observations = crate::storage::selected::PresentationObservations::capture(options, None, model.prepared().rows.iter());
    assert!(!observations.facts_known);
    assert!(!observations.selection_known);
    assert!(observations.runtime.is_none());
    assert!(observations.machine_label.is_none());
    assert_eq!(observations.capture_relative_status(&model.prepared().rows[0]), json!({"state":null,"observation":"unavailable"}));
}

#[test]
fn configured_semantic_selected60_history_witness_survives_gc_release_and_runtime_reopen() {
    use std::io::Write;
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let path = input.full_path(options);
    cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected60-history");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    cache.finish_semantic_stage(batch.epoch).unwrap();
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(concat!(
        "{\"timestamp\":\"2026-10-01T00:03:00Z\",\"type\":\"response_item\",",
        "\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"generation B\"}]}}\n"
    ).as_bytes()).unwrap();
    cache.ingest(&"b".repeat(64), &path, "codex").unwrap();
    let (next, next_model) = model_ready(&f, &mut cache, options, input, "selected60-history");
    assert_eq!(cache.publish_semantic_catalog(&next, next_model.prepared()).unwrap().0, Outcome::Written);
    assert_ne!(runtime.current_generation(key).unwrap(), id);
    let mut db = SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();
    let retained: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_proof_refs WHERE namespace='source-a' AND session_key=?1 AND id=?2 AND kind='history')",
        params![key,id.0], |r| r.get(0),
    ).unwrap();
    assert!(retained, "actual producer must retain A through History, independently of leases");
    let mut lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding { source_key: "source-a".into(), snapshot: lease.snapshot().clone() };
    let query = format!("sid={key}&scope=retained_history&after=0&limit=1");
    let original = bytes(&lease, options, &host, &query);
    let value: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(value["identity"]["read_scope"], "retained_history");
    assert_eq!(value["identity"]["generation"], lease.snapshot().publication_generation);
    lease.release().unwrap();
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    crate::storage::sqlite::proof_generations::collect(&tx, "source-a", 64).unwrap();
    tx.commit().unwrap();
    drop(db);
    drop(runtime);
    // Reopening the actual host runtime fences all earlier owner epochs.
    let reopened = binding.reader_runtime().unwrap();
    let again = reopened.selected_generation(key, &id, &"8".repeat(64), 60_000).unwrap();
    assert_eq!(bytes(&again, options, &host, &query), original);
    assert!(path.exists());
    assert!(!EventCache::path(&options.cache).exists());
}

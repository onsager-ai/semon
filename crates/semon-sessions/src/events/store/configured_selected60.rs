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
    facts.codex_rollouts = Some([input.path.to_string_lossy().into_owned()].into_iter().collect());
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

    // No Raw mutation: change independent reported runtime facts and the facts
    // availability observation. The old witness must never be reconstructed.
    facts.hostname = "independent-facts-change".into();
    facts.offline_since = Some(1_790_000_100_000);
    crate::facts::write_facts(facts_path, &facts).unwrap();
    assert_eq!(bytes(&lease, options, &host, &query), original);

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

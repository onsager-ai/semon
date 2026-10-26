use super::*;

// Shared existing negative-read hook: a refused proof must never call Raw.
struct ProviderCounter(std::sync::atomic::AtomicUsize);
impl crate::SessionSourceReader for ProviderCounter {
    fn read_range(&self, _: &crate::SessionSourceRef, _: Option<&str>, _: u64, _: usize) -> std::io::Result<crate::SessionSourceRange> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(std::io::Error::other("unexpected Raw access"))
    }
}

#[test]
fn configured_semantic_selected89_real_codec_roundtrip_and_one_byte_tamper_before_raw() {
    use crate::storage::proof_codec::{self, ExpectedGeneration, GenerationRecord};
    use crate::storage::selected::SelectedSnapshotStore;
    selected75_native(&[], |lease, options, host, key, db, _, _, _| {
        let snapshot = lease.snapshot();
        let record = db.query_row(
            "SELECT namespace,id,session_key,publication,proof,admission,stream,metadata,slots,prefix FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3",
            params![snapshot.namespace.as_str(), snapshot.proof_generation.0, key],
            |r| Ok(GenerationRecord {
                namespace: r.get(0)?, id: crate::storage::proof_lifetime::ProofGenerationId(r.get(1)?),
                session_key: r.get(2)?, publication: r.get(3)?, proof: r.get(4)?,
                admission: u64::try_from(r.get::<_, i64>(5)?).unwrap(), stream: r.get(6)?, metadata: r.get(7)?,
                slots: r.get(8)?, prefix: r.get(9)?,
            }),
        ).unwrap();
        let (publication, base, manifest, digest): (String, String, String, String) = db.query_row(
            "SELECT publication,base,manifest,digest FROM storage_proof_presentation_roots WHERE namespace=?1 AND digest=?2",
            params![snapshot.namespace.as_str(), record.proof],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(publication, snapshot.publication_generation);
        assert_eq!(digest, record.proof);
        proof_codec::verify_presentation_digest(&base, &manifest, &digest).unwrap();
        let (decoded, row) = proof_codec::decode_generation(record.clone(), ExpectedGeneration {
            namespace: snapshot.namespace.as_str(), id: &snapshot.proof_generation,
            admission: snapshot.admission, stream: &snapshot.stream_epoch,
            proof: &digest, publication: &snapshot.publication_generation,
            prefix: &snapshot.consumed_prefix,
        }).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(serde_json::to_string(&row).unwrap(), serde_json::to_string(lease.row()).unwrap());
        let control = crate::SelectedReadControl::default();
        let opened = lease.open_selected(options, host, &control).unwrap();
        assert_eq!(opened.snapshot.proof_generation, decoded.id);
        assert_eq!(opened.snapshot.consumed_prefix, decoded.prefix);
        assert_eq!(serde_json::to_string(&opened.row).unwrap(), record.metadata);
        drop(opened);
        let query = format!("sid={key}&after=0&limit=1");
        let (pinned, reply) = read_bound(lease, options, host, &query, None, SessionSourceReadMode::LocalOrPinnedProvider, &control).unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(pinned.namespace, snapshot.namespace);
        assert_eq!(pinned.admission, snapshot.admission);
        assert_eq!(pinned.stream_epoch, snapshot.stream_epoch);
        assert_eq!(pinned.publication_generation, decoded.publication);
        assert_eq!(pinned.proof_generation, decoded.id);
        assert_eq!(pinned.consumed_prefix, decoded.prefix);

        // Change exactly one byte while preserving valid JSON and byte length.
        let saved = record.metadata;
        let mut tampered = saved.as_bytes().to_vec();
        let marker = b"\"harness\":\"";
        let index = tampered.windows(marker.len()).position(|w| w == marker).unwrap() + marker.len();
        tampered[index] = b'X';
        assert_eq!(saved.len(), tampered.len());
        assert_eq!(saved.bytes().zip(tampered.iter()).filter(|(a, b)| a != *b).count(), 1);
        let tampered = String::from_utf8(tampered).unwrap();
        let _: crate::model::summary::CatalogRow = serde_json::from_str(&tampered).unwrap();
        db.execute_batch("DROP TRIGGER storage_proof_generation_immutable").unwrap();
        db.execute("UPDATE storage_proof_generations SET metadata=?1 WHERE namespace=?2 AND id=?3 AND session_key=?4",
            params![tampered, snapshot.namespace.as_str(), snapshot.proof_generation.0, key]).unwrap();
        db.execute_batch("CREATE TRIGGER storage_proof_generation_immutable BEFORE UPDATE ON storage_proof_generations BEGIN SELECT RAISE(ABORT,'immutable proof generation'); END;").unwrap();
        let provider = ProviderCounter(std::sync::atomic::AtomicUsize::new(0));
        assert!(read_bound(lease, options, host, &query, Some(&provider), SessionSourceReadMode::ProviderOnly, &control).is_err());
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(lease.open_selected(options, host, &control).is_err());
        assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    });
}

// Both stores run the native producer independently. Only the independently
// committed publication coordinates enter the existing publication oracle.
fn selected75_native(
    records: &[Value],
    check: impl FnOnce(
        &crate::storage::sqlite::proof_generations::Lease,
        &crate::Options,
        &HostSourceBinding,
        &str,
        &Connection,
        &Fixture,
        &mut EventCache,
        &crate::Input,
    ),
) {
    use std::io::Write;

    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let path = input.full_path(options);
    let mut raw = fs::OpenOptions::new().append(true).open(&path).unwrap();
    for record in records {
        writeln!(raw, "{record}").unwrap();
    }
    drop(raw);
    cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected75-native");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    let lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding { source_key: "source-a".into(), snapshot: lease.snapshot().clone() };
    let mut ordinary = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let prepared = crate::model::prepare_sources(options, &mut ordinary, std::slice::from_ref(input), 1_790_000_000_000).unwrap();
    ordinary.publish_catalog_projection(&crate::slot_projection::Publication {
        rows: &prepared.rows,
        transcripts: Some(&prepared.transcripts),
    }, prepared.base_generation.as_deref()).unwrap();
    let db = SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();
    cache.finish_semantic_stage(batch.epoch).unwrap();
    check(&lease, options, &host, key, &db, &f, &mut cache, input);
}

fn selected75_json_oracle(
    lease: &crate::storage::sqlite::proof_generations::Lease,
    options: &crate::Options,
    host: &HostSourceBinding,
    db: &Connection,
    query: &str,
    status: u16,
) -> Value {
    let (snapshot, actual) = read_bound(lease, options, host, query, None, SessionSourceReadMode::LocalOrPinnedProvider, &crate::SelectedReadControl::default()).unwrap();
    assert_eq!(snapshot.namespace, lease.snapshot().namespace);
    assert_eq!(snapshot.admission, lease.snapshot().admission);
    assert_eq!(snapshot.stream_epoch, lease.snapshot().stream_epoch);
    assert_eq!(snapshot.publication_generation, lease.snapshot().publication_generation);
    assert_eq!(snapshot.proof_generation, lease.snapshot().proof_generation);
    assert_eq!(snapshot.consumed_prefix, lease.snapshot().consumed_prefix);
    let expected = if query.contains("&field_chunk=") || query.contains("&part=") {
        crate::session_entry_field_with_mode(options, &host.source_key, query, None, SessionSourceReadMode::LocalOrPinnedProvider)
    } else {
        crate::session_transcript_range_with_mode(options, &host.source_key, query, None, SessionSourceReadMode::LocalOrPinnedProvider)
    };
    assert_eq!(actual.status, status, "{}", String::from_utf8_lossy(&actual.body));
    assert_eq!(actual.status, expected.status);
    assert_eq!(actual.content_type, expected.content_type);
    assert_eq!(actual.etag, expected.etag);
    let actual: Value = serde_json::from_slice(&actual.body).unwrap();
    let mut expected: Value = serde_json::from_slice(&expected.body).unwrap();
    if expected.get("identity").is_some() {
        publication_oracle(&mut expected, db, &host.source_key);
        assert_eq!(actual["identity"]["generation"], lease.snapshot().publication_generation);
    }
    assert_eq!(actual, expected, "full native reply for {query}");
    actual
}

#[test]
fn configured_semantic_selected75_native_long_output_walks_every_scalar_chunk() {
    let text = "scalar 中😀 quote\" slash\\ newline\n".repeat(9_000);
    assert!(text.len() > 3 * crate::json_string::JSON_STRING_CHECKPOINT_BYTES as usize);
    let record = |time, payload| json!({"timestamp":time,"type":"response_item","payload":payload});
    selected75_native(&[
        record("2026-10-01T00:02:00Z", json!({"type":"function_call","call_id":"selected75-long","name":"selected75_long_output","arguments":"{}"})),
        record("2026-10-01T00:02:01Z", json!({"type":"function_call_output","call_id":"selected75-long","output":text})),
    ], |lease, options, host, key, db, _, _, _| {
        let page = selected75_json_oracle(lease, options, host, db, &format!("sid={key}&after=0&limit=32"), 200);
        let entry = page["entries"].as_array().unwrap().iter().find(|entry| entry["name"] == "selected75_long_output").unwrap();
        let slot = entry["slot"].as_u64().unwrap();
        let generation = page["projection"]["generation"].as_str().unwrap();
        let mut joined = String::new();
        let mut chunks = 0;
        for chunk in 0..32 {
            let query = format!("sid={key}&generation={generation}&after={slot}&limit=1&field_chunk={chunk}");
            let reply = selected75_json_oracle(lease, options, host, db, &query, 200);
            assert_eq!(reply["projection"]["generation"], generation);
            assert_eq!(reply["slot"], slot);
            assert_eq!(reply["field"]["chunk"], chunk);
            assert_eq!(reply["field"]["name"], "out");
            assert!(reply["observation"]["source_bytes"].as_u64().unwrap() <= crate::json_string::JSON_STRING_CHECKPOINT_BYTES + 11);
            joined.push_str(reply["text"].as_str().unwrap());
            chunks += 1;
            if reply["field"]["complete"] == true {
                assert!(reply["field"]["next"].is_null());
                break;
            }
            assert_eq!(reply["field"]["next"], chunk + 1);
        }
        assert!(chunks >= 3);
        assert_eq!(joined.as_bytes(), text.as_bytes());
        let invalid = selected75_json_oracle(lease, options, host, db, &format!("sid={key}&generation={generation}&after={slot}&limit=1&field_chunk={chunks}"), 400);
        assert_eq!(invalid["error"]["code"], "invalid_arguments");
    });
}

#[test]
fn configured_semantic_selected75_native_script_output_and_image_parts() {
    let script = "text(await tools.exec_command({cmd:'printf selected75'}));";
    let output = "selected75 command output\n中😀\n";
    // The existing attachment fixture accepts this recorded PNG header. It is
    // decoded only by the native attachment reader, without image processing.
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    let record = |time, kind, payload| json!({"timestamp":time,"type":kind,"payload":payload});
    selected75_native(&[
        record("2026-10-01T00:02:00Z", "response_item", json!({"type":"custom_tool_call","call_id":"selected75-script","name":"exec","status":"completed","input":script})),
        record("2026-10-01T00:02:01Z", "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution","id":"selected75-item","command":["/bin/zsh","-lc","printf selected75"],"cwd":"file:///sanitized","exit_code":0,"duration":{"secs":1,"nanos":0},"aggregated_output":output}})),
        record("2026-10-01T00:02:02Z", "response_item", json!({"type":"custom_tool_call_output","call_id":"selected75-script","output":[{"type":"input_text","text":"Script completed"}]})),
        record("2026-10-01T00:03:00Z", "response_item", json!({"type":"message","role":"user","content":[{"type":"input_text","text":"recorded image"},{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUg=="}]})),
    ], |lease, options, host, key, db, _, _, _| {
        let page = selected75_json_oracle(lease, options, host, db, &format!("sid={key}&after=0&limit=32"), 200);
        let entries = page["entries"].as_array().unwrap();
        let command = entries.iter().find(|entry| entry["arg"] == "printf selected75").unwrap();
        let generation = page["projection"]["generation"].as_str().unwrap();
        let slot = command["slot"].as_u64().unwrap();
        let query = format!("sid={key}&generation={generation}&after={slot}&limit=1");
        let expanded = selected75_json_oracle(lease, options, host, db, &format!("{query}&part=script"), 200);
        assert_eq!(expanded["entries"][0]["scriptText"], script);
        assert_eq!(expanded["entries"][0]["scriptTruncated"], false);
        let expanded = selected75_json_oracle(lease, options, host, db, &format!("{query}&part=output"), 200);
        assert_eq!(expanded["entries"][0]["out"], output);
        assert_eq!(expanded["entries"][0]["outputTruncated"], false);
        let unsupported = selected75_json_oracle(lease, options, host, db, &format!("{query}&field_chunk=0"), 422);
        assert_eq!(unsupported["error"]["code"], "field_unsupported");

        let image = entries.iter().find(|entry| entry["img"].as_array().is_some_and(|images| !images.is_empty())).unwrap();
        let slot = image["slot"].as_u64().unwrap();
        let images = image["img"].as_array().unwrap();
        for ordinal in 0..images.len() {
            let query = format!("sid={key}&generation={generation}&after={slot}&limit=1&part=attachment&image={ordinal}");
            let (snapshot, actual) = read_bound(lease, options, host, &query, None, SessionSourceReadMode::LocalOrPinnedProvider, &crate::SelectedReadControl::default()).unwrap();
            assert_eq!(snapshot.proof_generation, lease.snapshot().proof_generation);
            assert_eq!(snapshot.publication_generation, lease.snapshot().publication_generation);
            let expected = crate::session_entry_field_with_mode(options, &host.source_key, &query, None, SessionSourceReadMode::LocalOrPinnedProvider);
            assert_eq!(actual.status, 200);
            assert_eq!(actual.status, expected.status);
            assert_eq!(actual.content_type, "image/png");
            assert_eq!(actual.content_type, expected.content_type);
            assert_eq!(actual.etag, expected.etag);
            assert_eq!(actual.body, expected.body);
            assert_eq!(actual.body, png.as_slice());
        }
    });
}

#[test]
fn configured_semantic_selected75_replaced_prefix_refuses_old_lease_bytes() {
    selected75_native(&[], |lease, options, host, key, db, f, cache, input| {
        let page = selected75_json_oracle(lease, options, host, db, &format!("sid={key}&after=0&limit=32"), 200);
        let slot = page["entries"][0]["slot"].as_u64().unwrap();
        let generation = page["projection"]["generation"].as_str().unwrap();
        let query = format!("sid={key}&generation={generation}&after={slot}&limit=1&field_chunk=0");
        let before = selected75_json_oracle(lease, options, host, db, &query, 200);
        assert_eq!(before["text"], "sanitized model fixture");
        let path = input.full_path(options);
        let original = fs::read_to_string(&path).unwrap();
        let replacement = original.replacen("sanitized model fixture", "rewritten model fixture", 1);
        assert_ne!(replacement, original);
        assert_eq!(replacement.len(), original.len());
        fs::write(&path, replacement).unwrap();
        cache.ingest(&"b".repeat(64), &path, "codex").unwrap();
        let (batch, model) = model_ready(f, cache, options, input, "selected75-replacement");
        assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
        let current: String = db.query_row("SELECT value FROM main.meta WHERE namespace='source-a' AND key='catalog_generation'", [], |row| row.get(0)).unwrap();
        assert_ne!(current, lease.snapshot().publication_generation);
        cache.finish_semantic_stage(batch.epoch).unwrap();
        // read_inner always calls Records::finish after presentation. The
        // incompatible consumed-prefix proof must refuse the whole read,
        // rather than return replacement bytes under A's authenticated identity.
        assert!(read_bound(lease, options, host, &query, None, SessionSourceReadMode::LocalOrPinnedProvider, &crate::SelectedReadControl::default()).is_err());
    });
}

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
    crate::facts::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
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
    let mut ordinary = EventCache::open_scoped(&oracle_options.cache, &oracle_options.source).unwrap();
    let prepared = crate::model::prepare_sources(&oracle_options, &mut ordinary, std::slice::from_ref(input), 1_790_000_000_000).unwrap();
    ordinary.publish_catalog_projection(&crate::slot_projection::Publication { rows: &prepared.rows, transcripts: Some(&prepared.transcripts) }, prepared.base_generation.as_deref()).unwrap();
    let before = body(crate::session_transcript_range_with_mode(&oracle_options, "source-a", &query, None, SessionSourceReadMode::LocalOrPinnedProvider));
    assert_eq!(before["identity"]["machine_label"], facts.hostname);

    // No Raw mutation: change independent reported runtime facts and the facts
    // availability observation. The old witness must never be reconstructed.
    facts.hostname = "independent-facts-change".into();
    facts.offline_since = Some(1_790_000_100_000);
    crate::facts::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
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

    let mut ordinary = EventCache::open_scoped(&options.cache, &options.source).unwrap();
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

// The frozen CI clock is intentionally supplied by the workflow command, not
// mutated here: configured tests run concurrently in the normal contract job.
#[cfg(all(feature = "test-clock", debug_assertions))]
#[test]
fn configured_semantic_selected71_active_call_uses_ordinary_clock_and_refuses_before_raw() {
    use std::{
        io::Write,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    struct Count(AtomicUsize);
    impl crate::SessionSourceReader for Count {
        fn read_range(
            &self,
            _: &crate::SessionSourceRef,
            _: Option<&str>,
            _: u64,
            _: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("unexpected Raw access"))
        }
    }

    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let path = input.full_path(options);
    let mut raw = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        raw,
        "{}",
        json!({
            "timestamp": "2026-10-01T00:02:00Z",
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "call_id": "selected71-active-call",
                "name": "long_native_tool",
                "arguments": "{\\\"payload\\\":\\\"active native call\\\"}",
            },
        })
    )
    .unwrap();
    drop(raw);
    cache.ingest(&"a".repeat(64), &path, "codex").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected71-active");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    // The CI constant is later than this fixture timestamp, but this lease is
    // created under that same constant and so its 60 s TTL remains valid.
    let lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding { source_key: "source-a".into(), snapshot: lease.snapshot().clone() };
    let query = format!("sid={key}&after=0&limit=8");

    let mut ordinary = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let prepared = crate::model::prepare_sources(
        options,
        &mut ordinary,
        std::slice::from_ref(input),
        1_790_000_000_000,
    )
    .unwrap();
    ordinary
        .publish_catalog_projection(
            &crate::slot_projection::Publication {
                rows: &prepared.rows,
                transcripts: Some(&prepared.transcripts),
            },
            prepared.base_generation.as_deref(),
        )
        .unwrap();
    let db = SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();
    let actual = read(&lease, options, &host, &query);
    let mut expected = body(crate::session_transcript_range_with_mode(
        options,
        &host.source_key,
        &query,
        None,
        SessionSourceReadMode::LocalOrPinnedProvider,
    ));
    publication_oracle(&mut expected, &db, "source-a");
    assert_eq!(actual, expected, "active native calls share the ordinary render clock");
    assert!(actual.to_string().contains("long_native_tool"));

    let provider = Count(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicBool::new(true));
    let control = crate::SelectedReadControl::new(Instant::now() + Duration::from_secs(1), cancelled);
    assert!(read_bound(
        &lease,
        options,
        &host,
        &query,
        Some(&provider),
        SessionSourceReadMode::ProviderOnly,
        &control,
    )
    .is_err());
    assert_eq!(provider.0.load(Ordering::SeqCst), 0, "cancelled reads do not admit Raw");

    db.execute("UPDATE storage_proof_leases SET expires=0 WHERE namespace='source-a'", []).unwrap();
    assert!(read_bound(
        &lease,
        options,
        &host,
        &query,
        Some(&provider),
        SessionSourceReadMode::ProviderOnly,
        &crate::SelectedReadControl::default(),
    )
    .is_err());
    assert_eq!(provider.0.load(Ordering::SeqCst), 0, "expired leases do not admit Raw");
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
    let provider = ProviderCounter(std::sync::atomic::AtomicUsize::new(0));
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
fn configured_semantic_selected80_facts_replacement_after_capture_never_substitutes_live_b() {
    // PRIVATE UNIT: the real producer has no hook between initial capture and
    // lazy family qualification. These are real prepared rows and Facts files,
    // but this specifically exercises that private capture boundary.
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let facts_path = options.facts.as_ref().unwrap();
    let mut facts_a = crate::facts::read_facts(facts_path).unwrap();
    facts_a.codex_rollouts = Some([input.path.clone()].into_iter().collect());
    crate::facts::write_facts(facts_path, &facts_a, &crate::Options::default().source).unwrap();
    cache.ingest(&"a".repeat(64), &input.full_path(options), "codex").unwrap();
    let (_, model) = model_ready(&f, &mut cache, options, input, "selected80-facts-race");
    let row = &model.prepared().rows[0];
    let runtime_binding = cache.query_runtime_binding().unwrap();
    assert!(runtime_binding.is_some());

    // A status already saved under A remains saved-only after B replaces Facts.
    let mut saved = crate::storage::selected::PresentationObservations::capture(
        options,
        runtime_binding.as_ref(),
        model.prepared().rows.iter(),
    );
    assert_eq!(saved.captured_source_count(), row.sources.len());
    let saved_a = saved.capture_relative_status(row);
    assert_ne!(saved_a, json!({"state":null,"observation":"unavailable"}));
    let mut facts_b = facts_a.clone();
    facts_b.codex_rollouts = Some(Default::default());
    crate::facts::write_facts(facts_path, &facts_b, &crate::Options::default().source).unwrap();
    assert_eq!(saved.capture_relative_status(row), saved_a);

    // A row requiring a lazy lookup through A's now-invalid identity fails
    // closed instead of taking B's current_codex answer.
    crate::facts::write_facts(facts_path, &facts_a, &crate::Options::default().source).unwrap();
    let mut lazy = crate::storage::selected::PresentationObservations::capture(
        options,
        runtime_binding.as_ref(),
        model.prepared().rows.iter().skip(1),
    );
    assert_eq!(lazy.captured_source_count(), 0);
    crate::facts::write_facts(facts_path, &facts_b, &crate::Options::default().source).unwrap();
    assert_eq!(
        lazy.capture_relative_status(row),
        json!({"state":null,"observation":"unavailable"})
    );
    let mut durable: crate::storage::selected::PresentationObservations =
        serde_json::from_str(&serde_json::to_string(&lazy).unwrap()).unwrap();
    assert_eq!(durable.captured_source_count(), 0);
    let mut facts_c = facts_a.clone();
    facts_c.hostname = "after-deserialization".into();
    crate::facts::write_facts(facts_path, &facts_c, &crate::Options::default().source).unwrap();
    assert_eq!(
        durable.capture_relative_status(row),
        json!({"state":null,"observation":"unavailable"})
    );
}

#[test]
fn configured_semantic_selected80_capture_ignores_unrelated_facts_history() {
    // PRIVATE UNIT: capture receives real prepared rows, while unrelated
    // current_codex metadata deliberately exceeds the selected cohort.
    let (f, _, options, inputs) = model_sources();
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let options = &options[0];
    let input = &inputs[0];
    let facts_path = options.facts.as_ref().unwrap();
    let mut facts = crate::facts::read_facts(facts_path).unwrap();
    let mut rollouts = std::collections::BTreeSet::new();
    rollouts.insert(input.path.clone());
    rollouts.extend((0..512).map(|number| format!("unrelated/history-{number}.jsonl")));
    facts.codex_rollouts = Some(rollouts);
    crate::facts::write_facts(facts_path, &facts, &crate::Options::default().source).unwrap();
    cache.ingest(&"a".repeat(64), &input.full_path(options), "codex").unwrap();
    let (_, model) = model_ready(&f, &mut cache, options, input, "selected80-bounds");
    let runtime_binding = cache.query_runtime_binding().unwrap();
    assert!(runtime_binding.is_some());
    let mut observations = crate::storage::selected::PresentationObservations::capture(
        options,
        runtime_binding.as_ref(),
        model.prepared().rows.iter(),
    );
    assert_eq!(observations.captured_source_count(), 1);
    assert_ne!(
        observations.capture_relative_status(&model.prepared().rows[0]),
        json!({"state":null,"observation":"unavailable"})
    );
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

use super::*;
use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};
use crate::storage::selected::HostSourceBinding;
use crate::transcript_range::{read_bound, SessionSourceReadMode};
use serde_json::{Value, json};

mod selected60 {
    include!("configured_selected60.rs");
}

fn body(reply: crate::ViewerReply) -> Value {
    assert_eq!(reply.status,200,"{}",String::from_utf8_lossy(&reply.body));
    serde_json::from_slice(&reply.body).unwrap()
}
fn read(lease: &crate::storage::sqlite::proof_generations::Lease, options: &crate::Options, host: &HostSourceBinding, query: &str) -> Value {
    let (snapshot,reply)=read_bound(lease,options,host,query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).unwrap();
    assert_eq!(snapshot.proof_generation,lease.snapshot().proof_generation);
    body(reply)
}

// The two producers have different physical publication coordinates. Derive
// the expected identity from the actual publication, including qualification;
// no product field is discarded from the comparison.
fn publication_oracle(expected: &mut Value, db: &Connection, namespace: &str) {
    let meta = |key: &str| -> String {
        db.query_row("SELECT value FROM main.meta WHERE namespace=?1 AND key=?2", params![namespace,key], |r| r.get(0)).unwrap()
    };
    expected["identity"]["generation"] = json!(meta("catalog_generation"));
    expected["identity"]["observed_at"] = json!(meta("catalog_observed_at").parse::<i64>().unwrap());
    let provisional = meta("catalog_completeness") == "partial";
    expected["identity"]["owner_qualification"] = json!(if provisional {"provisional"} else {"qualified"});
    expected["identity"]["native_id"] = if !provisional && expected["identity"]["native_ids"].as_array().unwrap().len() == 1 {
        expected["identity"]["native_ids"][0].clone()
    } else {Value::Null};
}

#[test]
fn configured_semantic_selected57_real_two_source_native_range_and_field_oracle() {
    let (f,b,options,inputs)=model_sources();
    for (i,config) in [&f.config,&b].into_iter().enumerate() {
        let capability=crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
        let binding=capability.reader_binding().unwrap();
        let mut cache=EventCache::open_semantic(capability).unwrap();
        cache.ingest(&"a".repeat(64),&inputs[i].full_path(&options[i]),inputs[i].root.as_str()).unwrap();
        let (batch,model)=model_ready(&f,&mut cache,&options[i],&inputs[i],"selected57");
        cache.publish_semantic_catalog(&batch,model.prepared()).unwrap();
        let runtime=binding.reader_runtime().unwrap();
        let key=&model.prepared().rows[0].key;
        let id=runtime.current_generation(key).unwrap();
        let lease=runtime.selected_generation(key,&id,&"7".repeat(64),60000).unwrap();
        let host=HostSourceBinding {source_key:config.namespace.as_str().into(),snapshot:lease.snapshot().clone()};
        // Independently run the normal native producer and public selected API.
        let mut ordinary=EventCache::open_scoped(&options[i].cache, &options[i].source).unwrap();
        let original=crate::model::prepare_sources(&options[i],&mut ordinary,std::slice::from_ref(&inputs[i]),1_790_000_000_000).unwrap();
        ordinary.publish_catalog_projection(&crate::slot_projection::Publication{rows:&original.rows,transcripts:Some(&original.transcripts),observations:None},original.base_generation.as_deref()).unwrap();
        let query=format!("sid={key}&after=0&limit=1");
        let actual=read(&lease,&options[i],&host,&query);
        let mut expected=body(crate::session_transcript_range_with_mode(&options[i],&host.source_key,&query,None,SessionSourceReadMode::LocalOrPinnedProvider));
        // Product data, header, complete context and bounded accounting match.
        for field in ["api","projection","range","entries","freshness","content_observation","relationship_context","limits","observation"] {
            assert_eq!(actual[field],expected[field],"{field}, source {i}");
        }
        let db=SourceJournal::owned(NativeOwnership::admit(config).unwrap()).connection().unwrap();
        publication_oracle(&mut expected,&db,config.namespace.as_str());
        assert_eq!(actual,expected,"complete range reply, source {i}");
        let generation=actual["projection"]["generation"].as_str().unwrap();
        let field_query=format!("{query}&generation={generation}&field_chunk=0");
        let (_,field)=read_bound(&lease,&options[i],&host,&field_query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).unwrap();
        let ordinary_field=crate::session_entry_field_with_mode(&options[i],&host.source_key,&field_query,None,SessionSourceReadMode::LocalOrPinnedProvider);
        assert_eq!(field.status,ordinary_field.status);
        let field:Value=serde_json::from_slice(&field.body).unwrap();
        let mut oracle:Value=serde_json::from_slice(&ordinary_field.body).unwrap();
        if oracle.get("identity").is_some() {
            publication_oracle(&mut oracle,&db,config.namespace.as_str());
        }
        assert_eq!(field,oracle);
        let mut foreign=host.clone();
        foreign.snapshot.namespace=LogicalSourceNamespace::new("foreign").unwrap();
        assert!(read_bound(&lease,&options[i],&foreign,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).is_err());
        let mut missing=options[i].clone(); missing.codex_home=f.root.join("absent-codex"); missing.claude_home=f.root.join("absent-claude");
        assert!(read_bound(&lease,&missing,&host,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).is_err());
        cache.finish_semantic_stage(batch.epoch).unwrap();
    }
}

#[test]
fn configured_semantic_selected57_claude_original_provenance_survives_current_reference_move() {
    use std::io::Write;
    let (f, config, options, inputs) = model_sources();
    let options = &options[1];
    let input = &inputs[1];
    let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap();
    let binding = capability.reader_binding().unwrap();
    let mut cache = EventCache::open_semantic(capability).unwrap();
    let path = input.full_path(options);
    cache.ingest(&"a".repeat(64), &path, "claude").unwrap();
    let (batch, model) = model_ready(&f, &mut cache, options, input, "selected57-provenance");
    assert_eq!(cache.publish_semantic_catalog(&batch, model.prepared()).unwrap().0, Outcome::Written);
    let runtime = binding.reader_runtime().unwrap();
    let key = &model.prepared().rows[0].key;
    let id = runtime.current_generation(key).unwrap();
    let lease = runtime.selected_generation(key, &id, &"7".repeat(64), 60_000).unwrap();
    let host = HostSourceBinding {
        source_key: config.namespace.as_str().into(),
        snapshot: lease.snapshot().clone(),
    };
    let query = format!("sid={key}&after=0&limit=1");
    let history_query = format!("{query}&scope=retained_history");
    let original = read(&lease, options, &host, &query);
    let original_history = read(&lease, options, &host, &history_query);
    assert_eq!(original["session"]["details"]["live_observation"], true);
    assert_eq!(original_history["session"], original["session"]);
    assert_eq!(original_history["identity"]["read_scope"], "retained_history");

    // Independently qualify the fresh selected presentation against the normal
    // producer, retaining every product field in the existing publication oracle.
    let mut ordinary = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let prepared = crate::model::prepare_sources(options, &mut ordinary, std::slice::from_ref(input), 1_790_000_000_000).unwrap();
    ordinary.publish_catalog_projection(&crate::slot_projection::Publication {
        rows: &prepared.rows,
        transcripts: Some(&prepared.transcripts),
        observations: None,
    }, prepared.base_generation.as_deref()).unwrap();
    let mut expected = body(crate::session_transcript_range_with_mode(options, &host.source_key, &query, None, SessionSourceReadMode::LocalOrPinnedProvider));
    let mut db = SourceJournal::owned(NativeOwnership::admit(&config).unwrap()).connection().unwrap();
    publication_oracle(&mut expected, &db, config.namespace.as_str());
    assert_eq!(original, expected);
    cache.finish_semantic_stage(batch.epoch).unwrap();

    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(concat!(
        "{\"type\":\"user\",\"sessionId\":\"model-1\",\"uuid\":\"later\",\"parentUuid\":\"answer\",",
        "\"timestamp\":\"2026-10-01T00:03:00Z\",\"message\":{\"role\":\"user\",\"content\":\"later Claude publication\"}}\n"
    ).as_bytes()).unwrap();
    cache.ingest(&"b".repeat(64), &path, "claude").unwrap();
    let (next, next_model) = model_ready(&f, &mut cache, options, input, "selected57-provenance");
    assert_eq!(cache.publish_semantic_catalog(&next, next_model.prepared()).unwrap().0, Outcome::Written);
    let next_id = runtime.current_generation(key).unwrap();
    assert_ne!(next_id, id);
    let next_lease = runtime.selected_generation(key, &next_id, &"8".repeat(64), 60_000).unwrap();
    let next_host = HostSourceBinding {
        source_key: host.source_key.clone(),
        snapshot: next_lease.snapshot().clone(),
    };
    let fresh = read(&next_lease, options, &next_host, &query);
    assert_ne!(fresh["projection"]["generation"], original["projection"]["generation"]);
    assert_ne!(fresh["session"], original["session"]);
    let references: (bool, bool) = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND id=?3 AND kind='current'),EXISTS(SELECT 1 FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND id=?3 AND kind='history')",
        params![config.namespace.as_str(), key, id.0],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(references, (false, true), "the real successor moves A out of Current");

    // Retire only the test's History reference, leaving the active lease as A's
    // sole retention authority. Catalog History metadata and Raw stay intact.
    assert_eq!(db.execute(
        "DELETE FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND id=?3 AND kind='history'",
        params![config.namespace.as_str(), key, id.0],
    ).unwrap(), 1);
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    crate::storage::sqlite::proof_generations::collect(&tx, config.namespace.as_str(), 64).unwrap();
    tx.commit().unwrap();
    for (query, expected) in [(&query, &original), (&history_query, &original_history)] {
        let (snapshot, reply) = read_bound(&lease, options, &host, query, None, SessionSourceReadMode::LocalOrPinnedProvider, &crate::SelectedReadControl::default()).unwrap();
        assert_eq!(body(reply), *expected, "both scopes retain the original presentation");
        assert_eq!(snapshot.namespace, host.snapshot.namespace);
        assert_eq!(snapshot.admission, host.snapshot.admission);
        assert_eq!(snapshot.stream_epoch, host.snapshot.stream_epoch);
        assert_eq!(snapshot.publication_generation, host.snapshot.publication_generation);
        assert_eq!(snapshot.proof_generation, host.snapshot.proof_generation);
        assert_eq!(snapshot.consumed_prefix, host.snapshot.consumed_prefix);
    }
    assert!(path.exists());
}

#[test]
fn configured_semantic_selected57_l1_append_gc_reopen_and_unknown_ack() {
    use std::io::Write;
    let (f,_,options,inputs)=model_sources();
    let capability=crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding=capability.reader_binding().unwrap();
    let mut cache=EventCache::open_semantic(capability).unwrap();
    let path=inputs[0].full_path(&options[0]);
    cache.ingest(&"a".repeat(64),&path,"codex").unwrap();
    let (batch,model)=model_ready(&f,&mut cache,&options[0],&inputs[0],"selected57");
    let publication=cache.publish_semantic_catalog(&batch,model.prepared()).unwrap();
    let state=shared_state(&f);
    assert_eq!(cache.publish_semantic_catalog(&batch,model.prepared()).unwrap(),publication,"unknown ACK reconciles the durable operation identity");
    assert_eq!(shared_state(&f),state);
    let runtime=binding.reader_runtime().unwrap();
    let key=model.prepared().rows[0].key.clone();
    let id=runtime.current_generation(&key).unwrap();
    let token="7".repeat(64);
    let mut lease=runtime.selected_generation(&key,&id,&token,60000).unwrap();
    let host=HostSourceBinding{source_key:"source-a".into(),snapshot:lease.snapshot().clone()};
    let query=format!("sid={key}&after=0&limit=1");
    let first=read(&lease,&options[0],&host,&query);
    cache.finish_semantic_stage(batch.epoch).unwrap();
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(format!("{}\n",json!({"type":"response_item","timestamp":"2026-10-01T00:03:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"later publication"}]}})).as_bytes()).unwrap();
    cache.ingest(&"b".repeat(64),&path,"codex").unwrap();
    let (next,next_model)=model_ready(&f,&mut cache,&options[0],&inputs[0],"selected57");
    cache.publish_semantic_catalog(&next,next_model.prepared()).unwrap();
    assert_ne!(runtime.current_generation(&key).unwrap(),id);
    let owner=NativeOwnership::admit(&f.config).unwrap();
    let mut db=SourceJournal::owned(owner).connection().unwrap();
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    crate::storage::sqlite::proof_generations::collect(&tx,"source-a",64).unwrap();
    tx.commit().unwrap();
    assert_eq!(read(&lease,&options[0],&host,&query),first);
    let reopened=runtime.selected_generation(&key,&id,&token,60000).unwrap();
    assert_eq!(read(&reopened,&options[0],&host,&query),first);
    lease.release().unwrap();
    assert!(read_bound(&lease,&options[0],&host,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).is_err());
}

#[test]
fn configured_semantic_selected57_tamper_cancel_deadline_and_frozen_admission() {
    let (f,_,options,inputs)=model_sources();
    let capability=crate::storage::sqlite::semantic::SemanticStaging::admit(&f.config).unwrap();
    let binding=capability.reader_binding().unwrap();
    let mut cache=EventCache::open_semantic(capability).unwrap();
    let path=inputs[0].full_path(&options[0]);
    cache.ingest(&"a".repeat(64),&path,"codex").unwrap();
    let (batch,model)=model_ready(&f,&mut cache,&options[0],&inputs[0],"selected57");
    cache.publish_semantic_catalog(&batch,model.prepared()).unwrap();
    let runtime=binding.reader_runtime().unwrap();
    let key=&model.prepared().rows[0].key;
    let id=runtime.current_generation(key).unwrap();
    let lease=runtime.selected_generation(key,&id,&"7".repeat(64),60000).unwrap();
    let host=HostSourceBinding{source_key:"source-a".into(),snapshot:lease.snapshot().clone()};
    let query=format!("sid={key}&after=0&limit=1");
    for control in [crate::SelectedReadControl::new(std::time::Instant::now()+Duration::from_secs(10),std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true))),crate::SelectedReadControl::new(std::time::Instant::now(),std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)))] {
        assert!(read_bound(&lease,&options[0],&host,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&control).is_err());
    }
    let db=SourceJournal::owned(NativeOwnership::admit(&f.config).unwrap()).connection().unwrap();
    let original:String=db.query_row("SELECT metadata FROM storage_proof_presentations WHERE namespace='source-a' AND session_key=?1",[key],|r|r.get(0)).unwrap();
    db.execute_batch("DROP TRIGGER storage_proof_presentation_immutable; UPDATE storage_proof_presentations SET metadata='null' WHERE namespace='source-a'").unwrap();
    assert!(read_bound(&lease,&options[0],&host,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).is_err());
    db.execute("UPDATE storage_proof_presentations SET metadata=?1 WHERE namespace='source-a' AND session_key=?2",params![original,key]).unwrap();
    db.execute_batch("CREATE TRIGGER storage_proof_presentation_immutable BEFORE UPDATE ON storage_proof_presentations BEGIN SELECT RAISE(ABORT,'immutable presentation'); END;").unwrap();
    let first=read(&lease,&options[0],&host,&query);
    assert!(!first["entries"].as_array().unwrap().is_empty());
    let original_raw=fs::read(&path).unwrap();
    let damaged=String::from_utf8(original_raw.clone()).unwrap().replace("sanitized model fixture","corrupted model fixture");
    assert_ne!(damaged.as_bytes(),original_raw.as_slice());
    fs::write(&path,damaged).unwrap();
    assert!(read_bound(&lease,&options[0],&host,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).is_err());
    fs::write(&path,&original_raw).unwrap();
    assert_eq!(read(&lease,&options[0],&host,&query),first);
    // Revoke the request after a real authorized Raw I/O has completed. The
    // shared renderer may have buffered bytes, but the port must return none.
    use crate::storage::selected::SelectedSnapshotStore;
    struct CancelAfter<'a> { reader:&'a dyn crate::SessionSourceReader, flag:std::sync::Arc<std::sync::atomic::AtomicBool> }
    impl crate::SessionSourceReader for CancelAfter<'_> {
        fn read_range(&self,source:&crate::SessionSourceRef,expected:Option<&str>,offset:u64,max:usize)->std::io::Result<crate::SessionSourceRange> {
            let bytes=self.reader.read_range(source,expected,offset,max)?;
            self.flag.store(true,std::sync::atomic::Ordering::Release);
            Ok(bytes)
        }
    }
    let opened=lease.open_selected(&options[0],&host,&crate::SelectedReadControl::default()).unwrap();
    let native=opened.sources.reader_with_mode(None,SessionSourceReadMode::LocalOrPinnedProvider);
    let flag=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider=CancelAfter{reader:&native,flag:flag.clone()};
    let control=crate::SelectedReadControl::new(std::time::Instant::now()+Duration::from_secs(10),flag.clone());
    assert!(read_bound(&lease,&options[0],&host,&query,Some(&provider),SessionSourceReadMode::ProviderOnly,&control).is_err());
    assert!(flag.load(std::sync::atomic::Ordering::Acquire));
    f.db().execute("UPDATE storage_namespaces SET admission_generation=admission_generation+1 WHERE namespace='source-a'",[]).unwrap();
    assert!(read_bound(&lease,&options[0],&host,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).is_err());
}

#[test]
fn configured_semantic_selected57_equal_native_ids_keep_authorized_host_identity() {
    let (f,b,options,mut inputs)=model_sources();
    inputs[1]=inputs[0].clone();
    fs::copy(inputs[0].full_path(&options[0]),inputs[1].full_path(&options[1])).unwrap();
    let mut replies=Vec::new();
    for (i,config) in [&f.config,&b].into_iter().enumerate() {
        let capability=crate::storage::sqlite::semantic::SemanticStaging::admit(config).unwrap();
        let binding=capability.reader_binding().unwrap();
        let mut cache=EventCache::open_semantic(capability).unwrap();
        cache.ingest(&"a".repeat(64),&inputs[i].full_path(&options[i]),"codex").unwrap();
        let (batch,model)=model_ready(&f,&mut cache,&options[i],&inputs[i],"selected57");
        cache.publish_semantic_catalog(&batch,model.prepared()).unwrap();
        let runtime=binding.reader_runtime().unwrap();
        let key=&model.prepared().rows[0].key;
        assert_eq!(key,"model-0");
        let id=runtime.current_generation(key).unwrap();
        let lease=runtime.selected_generation(key,&id,&"7".repeat(64),60000).unwrap();
        let host=HostSourceBinding{source_key:config.namespace.as_str().into(),snapshot:lease.snapshot().clone()};
        let reply=read(&lease,&options[i],&host,&format!("sid={key}&after=0&limit=1"));
        assert_eq!(reply["identity"]["source_key"],config.namespace.as_str());
        assert_eq!(reply["identity"]["native_ids"],json!(["model-0"]));
        replies.push((id,reply));
    }
    assert_ne!(replies[0].0,replies[1].0);
    assert_ne!(replies[0].1["entries"][0]["entry_id"],replies[1].1["entries"][0]["entry_id"]);
}

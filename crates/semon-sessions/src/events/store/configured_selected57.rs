use super::*;
use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};
use crate::storage::selected::HostSourceBinding;
use crate::transcript_range::{read_bound, SessionSourceReadMode};
use serde_json::{Value, json};

fn body(reply: crate::ViewerReply) -> Value {
    assert_eq!(reply.status,200,"{}",String::from_utf8_lossy(&reply.body));
    serde_json::from_slice(&reply.body).unwrap()
}
fn read(lease: &crate::storage::sqlite::proof_generations::Lease, options: &crate::Options, host: &HostSourceBinding, query: &str) -> Value {
    let (snapshot,reply)=read_bound(lease,options,host,query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).unwrap();
    assert_eq!(snapshot.proof_generation,lease.snapshot().proof_generation);
    body(reply)
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
        let mut ordinary=EventCache::open_scoped(&options[i].cache).unwrap();
        let original=crate::model::prepare_sources(&options[i],&mut ordinary,std::slice::from_ref(&inputs[i]),1_790_000_000_000).unwrap();
        ordinary.publish_catalog_projection(&crate::slot_projection::Publication{rows:&original.rows,transcripts:Some(&original.transcripts)},original.base_generation.as_deref()).unwrap();
        let query=format!("sid={key}&after=0&limit=1");
        let actual=read(&lease,&options[i],&host,&query);
        let expected=body(crate::session_transcript_range_with_mode(&options[i],&host.source_key,&query,None,SessionSourceReadMode::LocalOrPinnedProvider));
        // Product data, header, complete context and bounded accounting match.
        for field in ["api","projection","range","entries","freshness","content_observation","relationship_context","limits","observation"] {
            assert_eq!(actual[field],expected[field],"{field}, source {i}");
        }
        let mut actual_identity=actual["identity"].clone();
        let mut expected_identity=expected["identity"].clone();
        // Physical publication roots and observation timestamps belong to
        // independently published stores. Check them, then compare every other
        // identity field without deriving an oracle from Selection.
        assert_eq!(actual_identity["generation"],lease.snapshot().publication_generation);
        assert!(actual_identity["observed_at"].as_i64().is_some());
        assert!(expected_identity["generation"].as_str().is_some_and(|s|s.len()==64));
        assert!(expected_identity["observed_at"].as_i64().is_some());
        for identity in [&mut actual_identity,&mut expected_identity] {
            identity.as_object_mut().unwrap().remove("generation");
            identity.as_object_mut().unwrap().remove("observed_at");
            // The configured cohort deliberately advertises partial membership.
            identity.as_object_mut().unwrap().remove("owner_qualification");
            identity.as_object_mut().unwrap().remove("native_id");
        }
        assert_eq!(actual_identity,expected_identity);
        assert_eq!(actual["session"],expected["session"]);
        let generation=actual["projection"]["generation"].as_str().unwrap();
        let field_query=format!("{query}&generation={generation}&field_chunk=0");
        let (_,field)=read_bound(&lease,&options[i],&host,&field_query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).unwrap();
        let ordinary_field=crate::session_entry_field_with_mode(&options[i],&host.source_key,&field_query,None,SessionSourceReadMode::LocalOrPinnedProvider);
        assert_eq!(field.status,ordinary_field.status);
        let mut field:Value=serde_json::from_slice(&field.body).unwrap();
        let mut oracle:Value=serde_json::from_slice(&ordinary_field.body).unwrap();
        // Same independent identity-coordinate assertions as the range above.
        field.as_object_mut().unwrap().remove("identity");
        oracle.as_object_mut().unwrap().remove("identity");
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
    f.db().execute("UPDATE storage_namespaces SET admission_generation=admission_generation+1 WHERE namespace='source-a'",[]).unwrap();
    assert!(read_bound(&lease,&options[0],&host,&query,None,SessionSourceReadMode::LocalOrPinnedProvider,&crate::SelectedReadControl::default()).is_err());
}

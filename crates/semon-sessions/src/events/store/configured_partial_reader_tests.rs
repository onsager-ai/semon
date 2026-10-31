mod partial_readers {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Provider {
        source: crate::SessionSourceRef,
        bytes: Vec<u8>,
        calls: AtomicUsize,
    }

    impl crate::SessionSourceReader for Provider {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            assert_eq!(source, &self.source);
            let generation: String = source
                .prefix_sha256
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert!(expected.is_none_or(|expected| expected == generation));
            let start = usize::try_from(offset).unwrap();
            let end = start.saturating_add(max).min(self.bytes.len());
            Ok(crate::SessionSourceRange {
                generation,
                length: self.bytes.len() as u64,
                offset,
                bytes: self.bytes[start..end].to_vec().into(),
                cached: false,
            })
        }
    }

    fn selected(options: &crate::Options, key: &str, provider: &Provider) -> crate::ViewerReply {
        crate::session_transcript_range_with_mode(
            options,
            "source-a",
            &format!("sid={key}&after=0&limit=1"),
            Some(provider),
            crate::SessionSourceReadMode::ProviderOnly,
        )
    }

    fn selected_session(
        options: &crate::Options,
        key: &str,
        provider: &Provider,
        now: i64,
    ) -> serde_json::Value {
        let sources = std::sync::Arc::new(crate::SessionSources::with_machines(vec![(
            String::new(),
            options.clone(),
        )]));
        let mut query = crate::query::Query::from_sources(sources);
        query
            .call_selected_at(
                "local",
                key,
                "get_session",
                &serde_json::json!({"id":key}),
                Some(provider),
                crate::SessionSourceReadMode::ProviderOnly,
                now,
            )
            .unwrap()
    }

    #[test]
    fn configured_partial_selected_requires_exact_row_authority_before_provider_bytes() {
        let (_fixture, _, options, inputs) = model_sources();
        let mut options = options[0].clone();
        let facts_path = options.cache.with_file_name("direct-capture-facts.json");
        let facts = crate::Facts {
            version: crate::facts::FACTS_VERSION,
            hostname: "direct-capture-facts".into(),
            ..crate::Facts::default()
        };
        fs::write(&facts_path, serde_json::to_vec(&facts).unwrap()).unwrap();
        options.facts = Some(facts_path.clone());
        let options = &options;
        let input = &inputs[0];
        // Use the ordinary default-local producer, rather than marking a
        // complete publication partial or borrowing the private listing port.
        let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let prepared = crate::model::prepare_sources(
            options,
            &mut cache,
            std::slice::from_ref(input),
            1_790_000_000_000,
        )
        .unwrap();
        assert_eq!(
            cache.publish_partial_catalog_fenced(&prepared).unwrap().0,
            Outcome::Written
        );
        let row = &prepared.rows[0];
        let source = &row.sources[0];
        let provider = Provider {
            source: crate::SessionSourceRef {
                root: input.root.as_str().into(),
                path: input.path.clone(),
                native_id: source.native_id.clone(),
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
            },
            bytes: fs::read(input.full_path(options)).unwrap(),
            calls: AtomicUsize::new(0),
        };
        let config = options.source.config(&options.cache).unwrap();
        let mut db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let completeness: String = db
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_completeness'",
                [config.namespace.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(completeness, "partial");
        let reply = selected(options, &row.key, &provider);
        assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["identity"]["owner_qualification"], "provisional");
        assert!(!body["entries"].as_array().unwrap().is_empty());
        assert!(provider.calls.load(Ordering::Relaxed) > 0);
        let session_before_index =
            selected_session(options, &row.key, &provider, 1_790_000_000_000);

        let direct_capture: String = db
            .query_row(
                "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                params![config.namespace.as_str(), row.key],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap()
            .expect("the direct owner grant persists its typed row runtime capture");
        let metadata_before_rollback: String = db
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![config.namespace.as_str(), row.key],
                |r| r.get(0),
            )
            .unwrap();
        let marker_before_rollback: String = db
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                [config.namespace.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        db.execute_batch(
            "CREATE TRIGGER direct_capture_rollback BEFORE UPDATE ON storage_catalog_native_observation_grants BEGIN SELECT RAISE(ABORT,'injected direct capture update failure'); END;",
        )
        .unwrap();
        let mut retry_cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let retry_prepared = crate::model::prepare_sources(
            options,
            &mut retry_cache,
            std::slice::from_ref(input),
            1_790_000_000_001,
        )
        .unwrap();
        assert!(retry_cache
            .publish_partial_catalog_fenced(&retry_prepared)
            .is_err());
        drop(retry_cache);
        db.execute_batch("DROP TRIGGER direct_capture_rollback")
            .unwrap();
        let metadata_after_rollback: String = db
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![config.namespace.as_str(), row.key],
                |r| r.get(0),
            )
            .unwrap();
        let capture_after_rollback: String = db
            .query_row(
                "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                params![config.namespace.as_str(), row.key],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap()
            .unwrap();
        let marker_after_rollback: String = db
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_native_publication'",
                [config.namespace.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(metadata_after_rollback, metadata_before_rollback);
        assert_eq!(capture_after_rollback, direct_capture);
        assert_eq!(marker_after_rollback, marker_before_rollback);

        let source_path = source.path.to_string_lossy().into_owned();
        assert_eq!(
            db.execute(
                "UPDATE main.files SET size=size+1,resume_at=resume_at+1 WHERE namespace=semon_namespace() AND path=?1",
                [&source_path],
            )
            .unwrap(),
            1
        );
        assert!(crate::storage::sqlite::catalog::runtime_source_cwds_if_current(
            &db,
            &row.sources
        )
        .unwrap()
        .is_none());
        provider.calls.store(0, Ordering::Relaxed);
        let ahead = selected(options, &row.key, &provider);
        assert_eq!(ahead.status, 200, "{}", String::from_utf8_lossy(&ahead.body));
        assert!(provider.calls.load(Ordering::Relaxed) > 0);
        let session_ahead_index =
            selected_session(options, &row.key, &provider, 1_790_000_000_000);
        assert_eq!(session_ahead_index, session_before_index);

        let original_facts = fs::read(&facts_path).unwrap();
        let mut changed_facts: crate::Facts = serde_json::from_slice(&original_facts).unwrap();
        changed_facts.hostname.push_str("-runtime-drift");
        fs::write(&facts_path, serde_json::to_vec(&changed_facts).unwrap()).unwrap();
        provider.calls.store(0, Ordering::Relaxed);
        assert_eq!(selected(options, &row.key, &provider).status, 503);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
        fs::write(&facts_path, original_facts).unwrap();

        let metadata: String = db
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![config.namespace.as_str(), row.key],
                |r| r.get(0),
            )
            .unwrap();
        let (digest, admission, stream): (String, i64, String) = db
            .query_row(
                "SELECT metadata_digest,admission,stream FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                params![config.namespace.as_str(), row.key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        for corruption in [
            "missing-grant",
            "changed-grant",
            "missing-query",
            "changed-runtime",
            "mismatched-source",
            "mismatched-dependency",
            "changed-captured-global",
        ] {
            match corruption {
                "missing-grant" => {
                    db.execute(
                        "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2",
                        params![config.namespace.as_str(), row.key],
                    )
                    .unwrap();
                }
                "changed-grant" => {
                    db.execute(
                        "UPDATE storage_catalog_native_observation_grants SET metadata_digest=?3 WHERE namespace=?1 AND session_key=?2",
                        params![config.namespace.as_str(), row.key, "0".repeat(64)],
                    )
                    .unwrap();
                }
                "missing-query" | "changed-runtime" => {
                    let mut changed = row.clone();
                    if corruption == "missing-query" {
                        changed.query = None;
                    } else {
                        changed.query.as_mut().unwrap()["runtime"] =
                            serde_json::json!("unbound-runtime");
                    }
                    db.execute(
                        "UPDATE main.session_catalog SET metadata=?3 WHERE namespace=?1 AND session_key=?2",
                        params![config.namespace.as_str(), row.key, serde_json::to_string(&changed).unwrap()],
                    )
                    .unwrap();
                }
                _ => {
                    let mut changed: serde_json::Value =
                        serde_json::from_str(&direct_capture).unwrap();
                    if corruption == "mismatched-source" {
                        let cwd_fingerprints = changed["runtime_cwd_fingerprints"]
                            .as_object_mut()
                            .unwrap();
                        let value = cwd_fingerprints.values().next().unwrap().clone();
                        cwd_fingerprints.clear();
                        cwd_fingerprints.insert("0".repeat(64), value);
                    } else if corruption == "mismatched-dependency" {
                        let source_generation =
                            crate::retention::source_generation(&row.sources).unwrap();
                        let runtime_rows = changed["runtime_rows"][&row.key][&source_generation]
                            .as_object_mut()
                            .unwrap();
                        let value = runtime_rows.values().next().unwrap().clone();
                        runtime_rows.clear();
                        runtime_rows.insert("0".repeat(64), value);
                    } else {
                        changed["runtime"] = serde_json::json!(format!(
                            "source-runtime-v1:{}",
                            "0".repeat(64)
                        ));
                    }
                    db.execute(
                        "UPDATE storage_catalog_native_observation_grants SET direct_capture=?3 WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                        params![config.namespace.as_str(), row.key, serde_json::to_string(&changed).unwrap()],
                    )
                    .unwrap();
                }
            }
            provider.calls.store(0, Ordering::Relaxed);
            assert_eq!(selected(options, &row.key, &provider).status, 503, "{corruption}");
            assert_eq!(provider.calls.load(Ordering::Relaxed), 0, "{corruption}");
            db.execute(
                "UPDATE main.session_catalog SET metadata=?3 WHERE namespace=?1 AND session_key=?2",
                params![config.namespace.as_str(), row.key, metadata],
            )
            .unwrap();
            db.execute(
                "INSERT OR REPLACE INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream,direct_capture) VALUES(?1,?2,'catalog',?3,?4,?5,?6)",
                params![config.namespace.as_str(), row.key, digest, admission, stream, direct_capture],
            )
            .unwrap();
        }

        assert_eq!(selected(options, &row.key, &provider).status, 200);

        let current: crate::model::summary::CatalogRow =
            serde_json::from_str(&metadata).unwrap();
        let binding = crate::storage::local_navigation_binding(options, "").unwrap();
        let tx = db.transaction().unwrap();
        let current_binding = binding.runtime_binding_at(&tx).unwrap();
        let mut repriced_cost = current.cost.clone();
        repriced_cost.usd = Some(repriced_cost.usd.unwrap_or_default() + 1.0);
        crate::storage::sqlite::proof_generations::prepare_repriced_observation(
            &tx,
            &current_binding,
            crate::storage::catalog::CatalogObservationOwner::Catalog,
            &current,
            &repriced_cost,
            current.query.as_ref(),
        )
        .unwrap()
        .publish(&tx)
        .unwrap();
        tx.commit().unwrap();
        let repriced_metadata: String = db
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![config.namespace.as_str(), row.key],
                |r| r.get(0),
            )
            .unwrap();
        let repriced: crate::model::summary::CatalogRow =
            serde_json::from_str(&repriced_metadata).unwrap();
        let repriced_capture: String = db
            .query_row(
                "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                params![config.namespace.as_str(), row.key],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap()
            .unwrap();
        let repriced_capture: crate::storage::selected::PresentationObservations =
            serde_json::from_str(&repriced_capture).unwrap();
        assert!(repriced_capture.is_runtime_capture_for(&repriced));
        assert!(repriced_capture.runtime_for(&repriced).is_some());

        let mut repaired_query = repriced.query.clone().unwrap();
        let query_rows = repaired_query["rows"].as_object_mut().unwrap();
        query_rows.insert("new-runtime-dependency".into(), serde_json::json!({}));
        let tx = db.transaction().unwrap();
        let current_binding = binding.runtime_binding_at(&tx).unwrap();
        crate::storage::sqlite::proof_generations::publish_repaired_query_observation(
            &tx,
            &current_binding,
            crate::storage::catalog::CatalogObservationOwner::Catalog,
            &repriced,
            &repaired_query,
        )
        .unwrap();
        tx.commit().unwrap();
        let capture_after_dependency_change: Option<String> = db
            .query_row(
                "SELECT direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
                params![config.namespace.as_str(), row.key],
                |r| r.get(0),
            )
            .unwrap();
        assert!(capture_after_dependency_change.is_none());
    }

    #[test]
    fn configured_partial_selected_lease_requires_native_proof_before_provider_bytes() {
        use crate::storage::selected::HostSourceBinding;
        use crate::storage::{native_events::NativeEventStore, proof_lifetime::ProofReaderLease};

        let (fixture, _, options, inputs) = model_sources();
        let options = &options[0];
        let input = &inputs[0];
        let capability =
            crate::storage::sqlite::semantic::SemanticStaging::admit(&fixture.config).unwrap();
        let binding = capability.reader_binding().unwrap();
        let mut cache = EventCache::open_semantic(capability).unwrap();
        cache
            .ingest(&"a".repeat(64), &input.full_path(options), "codex")
            .unwrap();
        let (batch, model) = model_ready(&fixture, &mut cache, options, input, "partial-proof");
        cache
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        let row = &model.prepared().rows[0];
        let source = &row.sources[0];
        let provider = Provider {
            source: crate::SessionSourceRef {
                root: input.root.as_str().into(),
                path: input.path.clone(),
                native_id: source.native_id.clone(),
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
            },
            bytes: fs::read(input.full_path(options)).unwrap(),
            calls: AtomicUsize::new(0),
        };
        let runtime = binding.reader_runtime().unwrap();
        let id = runtime.current_generation(&row.key).unwrap();
        let lease = runtime
            .selected_generation(&row.key, &id, &"8".repeat(64), 60_000)
            .unwrap();
        let host = HostSourceBinding {
            source_key: fixture.config.namespace.as_str().into(),
            snapshot: lease.snapshot().clone(),
        };
        let read = || {
            crate::transcript_range::read_bound(
                &lease,
                options,
                &host,
                &format!("sid={}&after=0&limit=1", row.key),
                Some(&provider),
                crate::SessionSourceReadMode::ProviderOnly,
                &crate::SelectedReadControl::default(),
            )
        };
        let (_, reply) = read().unwrap();
        assert_eq!(reply.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["identity"]["owner_qualification"], "provisional");
        assert!(provider.calls.load(Ordering::Relaxed) > 0);

        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&fixture.config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let mut statement = db
            .prepare("SELECT key,value FROM main.meta WHERE namespace=?1 AND (key GLOB 'native_source_chunks:*:header' OR key GLOB 'native_published_chunks:*:header') ORDER BY key")
            .unwrap();
        let proofs: Vec<(String, String)> = statement
            .query_map([fixture.config.namespace.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(statement);
        assert!(!proofs.is_empty());
        // Both live and independently retained copies may cover this exact
        // ledger. Neither may supply an unsupported header to the body pager.
        for (key, _) in &proofs {
            db.execute(
                "UPDATE main.meta SET value='{}' WHERE namespace=?1 AND key=?2",
                params![fixture.config.namespace.as_str(), key],
            )
            .unwrap();
        }
        provider.calls.store(0, Ordering::Relaxed);
        assert!(read().is_err());
        assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
        for (key, proof) in &proofs {
            db.execute(
                "UPDATE main.meta SET value=?3 WHERE namespace=?1 AND key=?2",
                params![fixture.config.namespace.as_str(), key, proof],
            )
            .unwrap();
        }
        assert_eq!(read().unwrap().1.status, 200);
    }
}

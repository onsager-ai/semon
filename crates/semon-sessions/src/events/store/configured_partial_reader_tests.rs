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

    #[test]
    fn configured_partial_selected_requires_exact_row_authority_before_provider_bytes() {
        let (_fixture, _, options, inputs) = model_sources();
        let options = &options[0];
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
        let db = crate::storage::sqlite::semantic::Stage::open(
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
                _ => {
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
                "INSERT OR REPLACE INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream) VALUES(?1,?2,'catalog',?3,?4,?5)",
                params![config.namespace.as_str(), row.key, digest, admission, stream],
            )
            .unwrap();
        }

        assert_eq!(selected(options, &row.key, &provider).status, 200);
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

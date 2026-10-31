//! The ordinary native model must run after PG ingest, not be copied from the
//! SQLite oracle, admitted publication and lease-bound Query bodies/fields.
use super::*;
use crate::events::{EventCache, Outcome};
use crate::storage::native_events::NativeEventStore;
use crate::storage::postgres::{catalog_effects::withdrawal, native_keys};
use crate::storage::{LogicalSourceNamespace, catalog::CatalogSnapshot, postgres::PgPool};

fn stage_inputs(
    options: &Options,
    batch: &crate::model::relation_stage::Batch,
) -> Vec<crate::Input> {
    batch
        .paths
        .iter()
        .map(|path| {
            crate::native_evidence::catalog_input(std::path::Path::new(path), options)
                .expect("every enrolled source belongs to the configured native roots")
        })
        .collect()
}

fn producer_rows(rows: &[CatalogRow]) -> serde_json::Value {
    let mut value = serde_json::to_value(rows).unwrap();
    for row in value.as_array_mut().unwrap() {
        if let Some(query) = row
            .get_mut("query")
            .and_then(serde_json::Value::as_object_mut)
        {
            query.remove("runtime");
        }
    }
    value
}

fn native_proof_tuple(
    tx: &mut PgTxn<'_>,
    generation: &str,
    source: &CatalogSource,
) -> Result<(Vec<u8>, i64, bool), StorageError> {
    let proof = super::super::selected_reader::native_proof(tx, generation, source)?;
    let row = tx
        .driver
        .query_one(
            "SELECT page_generation,usage_generation,usage_ready FROM semon_pg_native_catalog_proofs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND path_digest=sha256($3::bytea) AND source_path=$3 AND page_generation=$4",
            &[
                &tx.namespace.as_str().as_bytes(),
                &generation.as_bytes(),
                &super::super::path(source)?.as_bytes(),
                &proof.id.0.as_bytes(),
            ],
        )
        .map_err(sqlstate::error)?;
    Ok((row.get(0), row.get(1), row.get(2)))
}

fn copy_prepared(saved: &crate::model::PreparedSources) -> crate::model::PreparedSources {
    crate::model::PreparedSources {
        base_generation: saved.base_generation.clone(),
        claims: saved.claims.clone(),
        rows: saved.rows.clone(),
        transcripts: saved
            .transcripts
            .iter()
            .map(|projection| crate::slot_projection::Projection {
                key: projection.key.clone(),
                slots: projection.slots.clone(),
                repair: projection.repair,
            })
            .collect(),
        pids: saved.pids.clone(),
        lineage_dependencies: saved.lineage_dependencies.clone(),
        message_dependencies: saved.message_dependencies.clone(),
        relationships: saved.relationships.clone(),
        indexed_sources: saved.indexed_sources.clone(),
        removed_sources: saved.removed_sources.clone(),
        indexed_native_parents: saved.indexed_native_parents.clone(),
        withdrawals: saved.withdrawals.clone(),
        withdrawal_fence: saved.withdrawal_fence.clone(),
    }
}

type BodySources = Vec<(crate::SessionSourceRef, std::sync::Arc<[u8]>)>;
#[derive(Clone)]
struct BodyHost(BodySources);
impl crate::SessionSourceReader for BodyHost {
    fn read_range(
        &self,
        reference: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> std::io::Result<crate::SessionSourceRange> {
        let (saved, bytes) = self
            .0
            .iter()
            .find(|(saved, _)| saved == reference)
            .ok_or_else(|| std::io::Error::other("host denied"))?;
        if saved != reference
            || expected.is_some_and(|pin| pin != "body-fixture")
            || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(std::io::Error::other("host denied"));
        }
        let start = usize::try_from(offset).map_err(std::io::Error::other)?;
        let end = bytes.len().min(start.saturating_add(max));
        Ok(crate::SessionSourceRange {
            generation: "body-fixture".into(),
            length: bytes.len() as u64,
            offset,
            bytes: std::sync::Arc::from(
                bytes
                    .get(start..end)
                    .ok_or_else(|| std::io::Error::other("host range"))?,
            ),
            cached: false,
        })
    }
}
fn body_host(options: &Options, rows: &[CatalogRow]) -> BodyHost {
    BodyHost(
        rows.iter()
            .flat_map(|row| {
                row.sources.iter().map(|source| {
                    let path =
                        crate::catalog::source_relative(options, &row.harness, source).unwrap();
                    let reference = crate::SessionSourceRef {
                        root: row.harness.clone(),
                        path: path.clone(),
                        native_id: source.native_id.clone(),
                        offset: source.offset,
                        prefix_sha256: source.prefix_sha256,
                        tail_sha256: source.tail_sha256,
                    };
                    (
                        reference,
                        std::sync::Arc::from(fs::read(&source.path).unwrap()),
                    )
                })
            })
            .collect(),
    )
}

// Exercise the generic physical publisher with the real PG model capture. Each
// successful case is rolled back so the staged businessflow below keeps its
// original initial head and source claims.
fn generic_runtime_publication_preflight(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    prepared: &crate::model::PreparedSources,
) {
    use super::super::publication;
    let captured = prepared.presentation_observations().unwrap();
    let scopes = prepared
        .rows
        .iter()
        .map(|row| row.query.as_ref().unwrap()["runtime"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        scopes.len() > 1,
        "ordinary rows have distinct runtime scopes"
    );
    for fault in [
        "missing",
        "raw_global",
        "bound_global",
        "row_scope",
        "source_generation",
        "dependency",
    ] {
        let mut rows = prepared.rows.clone();
        let mut observations = captured.clone();
        match fault {
            "missing" => {}
            "raw_global" => observations.source_runtime = Some("changed raw global".into()),
            "bound_global" => observations.runtime = Some("changed bound global".into()),
            "row_scope" => {
                let other = rows
                    .iter()
                    .find(|row| {
                        row.query.as_ref().unwrap()["runtime"]
                            != rows[0].query.as_ref().unwrap()["runtime"]
                    })
                    .unwrap()
                    .query
                    .as_ref()
                    .unwrap()["runtime"]
                    .clone();
                rows[0].query.as_mut().unwrap()["runtime"] = other;
            }
            "source_generation" => rows[0].sources[0].offset += 1,
            "dependency" => rows[0]
                .native_ids
                .push("unobserved native dependency".into()),
            _ => unreachable!(),
        }
        let publication = crate::slot_projection::Publication {
            rows: &rows,
            transcripts: Some(&prepared.transcripts),
            observations: (fault != "missing").then_some(&observations),
        };
        assert_eq!(
            pool.transaction(namespace, TxnKind::Write, control, |tx| {
                publication::publish(
                    tx,
                    &publication,
                    prepared.base_generation.as_deref(),
                    &prepared.claims,
                    false,
                    None,
                )
            }),
            Err(StorageError::ScopeChanged),
            "generic runtime qualification must refuse {fault}"
        );
    }
    let mut all_queryless_rows = prepared.rows.clone();
    for row in &mut all_queryless_rows {
        row.query = None;
    }
    assert_eq!(
        pool.transaction(namespace, TxnKind::Write, control, |tx| {
            publication::publish(
                tx,
                &crate::slot_projection::Publication {
                    rows: &all_queryless_rows,
                    transcripts: Some(&prepared.transcripts),
                    observations: Some(captured),
                },
                prepared.base_generation.as_deref(),
                &prepared.claims,
                false,
                None,
            )
        }),
        Err(StorageError::ScopeChanged),
        "nonempty all-queryless generic publication still requires a Query row"
    );
    pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
        assert!(
            super::super::super::model_cache::generation(tx)?.is_none(),
            "all-queryless refusal must leave the catalog head untouched"
        );
        let owner = tx
            .driver
            .query(
                "SELECT runtime FROM semon_pg_native_catalog_stage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton",
                &[&namespace.as_str().as_bytes()],
            )
            .map_err(sqlstate::error)?;
        assert!(
            owner.is_empty(),
            "all-queryless refusal must not set owner runtime"
        );
        Ok(())
    })
    .unwrap();
    let mut mixed_rows = prepared.rows.clone();
    mixed_rows
        .iter_mut()
        .find(|row| row.query.is_some())
        .expect("captured fixture has a Query row")
        .query = None;
    let mixed_result = pool.transaction(namespace, TxnKind::Write, control, |tx| {
        let result = publication::publish(
            tx,
            &crate::slot_projection::Publication {
                rows: &mixed_rows,
                transcripts: Some(&prepared.transcripts),
                observations: Some(captured),
            },
            prepared.base_generation.as_deref(),
            &prepared.claims,
            false,
            None,
        )?;
        assert_eq!(result.0, Outcome::Written);
        let binding = super::super::super::model_cache::binding(tx)?;
        let global = binding
            .witness(captured.source_runtime.as_deref().unwrap())
            .unwrap();
        assert_eq!(captured.runtime.as_deref(), Some(global.as_str()));
        Err::<(), _>(StorageError::Conflict)
    });
    assert_eq!(
        mixed_result,
        Err(StorageError::Conflict),
        "rollback mixed preflight writes"
    );
    assert_eq!(
        pool.transaction(namespace, TxnKind::Write, control, |tx| {
            publication::publish(
                tx,
                &crate::slot_projection::Publication {
                    rows: &[],
                    transcripts: None,
                    observations: None,
                },
                None,
                &[],
                false,
                None,
            )
        }),
        Err(StorageError::ScopeChanged),
        "initial empty generic publication has no owner runtime"
    );
    for closed in [false, true] {
        let result = pool.transaction(namespace, TxnKind::Write, control, |tx| {
            let publication = crate::slot_projection::Publication {
                rows: &prepared.rows,
                transcripts: Some(&prepared.transcripts),
                observations: Some(captured),
            };
            let written = publication::publish(
                tx,
                &publication,
                prepared.base_generation.as_deref(),
                &prepared.claims,
                closed,
                None,
            )?;
            assert_eq!(written.0, Outcome::Written);
            let generation = written.1.as_deref().unwrap();
            let binding = super::super::super::model_cache::binding(tx)?;
            let global = binding
                .witness(captured.source_runtime.as_deref().unwrap())
                .unwrap();
            assert_eq!(captured.runtime.as_deref(), Some(global.as_str()));
            let expected = crate::storage::semantic_owner::bound_runtime(
                namespace.as_str(),
                &binding.admission,
                &global,
            )?;
            let owner_runtime: Vec<u8> = tx.driver.query_one(
                "SELECT runtime FROM semon_pg_native_catalog_stage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton",
                &[&namespace.as_str().as_bytes()],
            ).map_err(sqlstate::error)?.get(0);
            assert_eq!(owner_runtime, expected.as_bytes());
            let empty = publication::publish(
                tx,
                &crate::slot_projection::Publication {
                    rows: &[],
                    transcripts: None,
                    observations: None,
                },
                Some(generation),
                &[],
                closed,
                None,
            )?;
            assert_eq!(empty.0, Outcome::Written);
            assert_eq!(
                publication::publish(
                    tx,
                    &publication,
                    prepared.base_generation.as_deref(),
                    &prepared.claims,
                    closed,
                    None,
                )?,
                written,
                "exact replay returns its acknowledgement before the now-stale head"
            );
            Err::<(), _>(StorageError::Conflict)
        });
        assert_eq!(
            result,
            Err(StorageError::Conflict),
            "rollback preflight writes"
        );
    }
}

fn generic_runtime_publication_facades(
    pool: &PgPool,
    control: &PgCancellation,
    options: &Options,
    inputs: &[crate::Input],
    expected_bodies: &std::collections::BTreeMap<String, serde_json::Value>,
) {
    for mode in ["full", "partial", "closed"] {
        let namespace = ns(&format!("generic-runtime-{mode}"));
        let source = crate::storage::HostSource::new(namespace.clone(), "a".repeat(64)).unwrap();
        let config = source
            .store_config(crate::storage::StoreBackend::Postgres {
                pool: pool.clone(),
                control: control.clone(),
            })
            .unwrap();
        config.create_native_store().unwrap();
        let mut options = options.clone();
        options.source = source.with_store(config).unwrap();
        if mode == "full" {
            // Generic PG runtime capture needs the canonical Facts file, but
            // does not depend on a separately prepared source-selection head.
            let facts_path = options
                .cache
                .with_file_name(format!("{}-facts.json", namespace.as_str()));
            crate::write_facts(
                &facts_path,
                &crate::Facts {
                    version: 2,
                    hostname: "generic-runtime-facts".into(),
                    ..Default::default()
                },
                &options.source,
            )
            .unwrap();
            options.facts = Some(facts_path);
        }
        let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
        for (at, input) in inputs.iter().enumerate() {
            cache
                .ingest(
                    &format!("{:064x}", at + 1),
                    &input.full_path(&options),
                    input.root.as_str(),
                )
                .unwrap();
        }
        let prepared =
            crate::model::prepare_sources(&options, &mut cache, inputs, 1790812802000).unwrap();
        let captured = prepared.presentation_observations().unwrap();
        assert!(
            prepared
                .rows
                .iter()
                .map(|row| captured.runtime_for(row).unwrap())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1
        );
        let publish = |cache: &mut EventCache| match mode {
            "full" => cache
                .publish_catalog_projection_fenced(
                    &crate::slot_projection::Publication {
                        rows: &prepared.rows,
                        transcripts: Some(&prepared.transcripts),
                        observations: Some(captured),
                    },
                    prepared.base_generation.as_deref(),
                )
                .unwrap(),
            "partial" => {
                let (outcome, generation) =
                    cache.publish_partial_catalog_fenced(&prepared).unwrap();
                assert_eq!(outcome, Outcome::Written);
                generation
            }
            "closed" => {
                let (outcome, generation) = cache
                    .publish_closed_catalog_fenced(&prepared)
                    .unwrap()
                    .unwrap();
                assert_eq!(outcome, Outcome::Written);
                generation
            }
            _ => unreachable!(),
        };
        let generation = publish(&mut cache).expect("ordinary generic facade publication");
        assert_eq!(publish(&mut cache).as_deref(), Some(generation.as_str()));
        if mode == "partial" {
            // This generic publication has operation authority and native
            // proofs, without a semantic selected-presentation manifest.
            let store = super::super::snapshot::query::QueryCatalog {
                captured: false,
                pool: pool.clone(),
                namespace: namespace.clone(),
                control: control.clone(),
                generation: generation.clone(),
                options: options.clone(),
            };
            let host = body_host(&options, &prepared.rows);
            let mut query = crate::Query::from_sources(std::sync::Arc::new(
                crate::SessionSources::with_machines(vec![(
                    namespace.as_str().into(),
                    options.clone(),
                )]),
            ));
            let row = prepared
                .rows
                .iter()
                .find(|row| row.native_ids.iter().any(|id| id == "one"))
                .unwrap();
            let body = query
                .call_bound_selected_at(
                    "read_transcript",
                    &serde_json::json!({"id":row.key,"limit":100}),
                    1790812802000,
                    &store,
                    &host,
                    None,
                )
                .expect("generic publication selected transcript through authorized native proofs");
            let fields = query
                .call_bound_selected_at(
                    "get_session",
                    &serde_json::json!({"id":row.key}),
                    1790812802000,
                    &store,
                    &host,
                    None,
                )
                .expect("generic publication selected fields without semantic manifest");
            assert_eq!(body, expected_bodies[&row.key]["body"]);
            assert_eq!(fields, expected_bodies[&row.key]["fields"]);

            // Advance only this private fixture's Index ledger. The generic
            // publication and its authenticated native pages stay unchanged.
            let path = row.sources[0].path.to_string_lossy();
            pool.transaction(&namespace, TxnKind::Write, control, |tx| {
                let ns = namespace.as_str().as_bytes();
                assert_eq!(tx.driver.execute(
                    "UPDATE semon_pg_native_files SET size=size+1,resume_at=resume_at+1 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND path=$2",
                    &[&ns, &path.as_bytes()],
                ).map_err(sqlstate::error)?, 1);
                assert!(super::super::super::native_reader::runtime_source_cwds_if_current(tx, &row.sources)?.is_none());
                Ok(())
            }).unwrap();
            let ahead = query
                .call_bound_selected_at(
                    "get_session",
                    &serde_json::json!({"id":row.key}),
                    1790812802000,
                    &store,
                    &host,
                    None,
                )
                .expect("generic typed capture admits an old row while Index is ahead");
            assert_eq!(ahead, fields);

            let hostname = options.proc_root.join("sys/kernel/hostname");
            let original_hostname = fs::read(&hostname).unwrap();
            let global_before = crate::native_evidence::query_runtime_generation(&options).unwrap();
            fs::write(&hostname, "generic-advanced-index-runtime-drift\n").unwrap();
            let global_after = crate::native_evidence::query_runtime_generation(&options).unwrap();
            let refused = query.call_bound_selected_at(
                "get_session",
                &serde_json::json!({"id":row.key}),
                1790812802000,
                &store,
                &host,
                None,
            );
            fs::write(&hostname, &original_hostname).unwrap();
            assert_ne!(global_before, global_after);
            assert!(
                refused.is_err(),
                "generic Index-ahead capture requires the same live global witness"
            );
        }
        pool.transaction(&namespace, TxnKind::ReadSnapshot, control, |tx| {
            let payload: Vec<u8> = tx.driver.query_one(
                "SELECT payload FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2",
                &[&namespace.as_str().as_bytes(), &generation.as_bytes()],
            ).map_err(sqlstate::error)?.get(0);
            let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            assert_eq!(payload.as_array().unwrap().len(), 9);
            assert_eq!(payload[4], serde_json::to_value(captured).unwrap());
            assert!(
                payload[8].is_null(),
                "generic operation has no semantic input"
            );
            let binding = super::super::super::model_cache::binding(tx)?;
            let global = binding
                .witness(captured.source_runtime.as_deref().unwrap())
                .unwrap();
            let expected = crate::storage::semantic_owner::bound_runtime(
                namespace.as_str(),
                &binding.admission,
                &global,
            )?;
            let runtime: Vec<u8> = tx.driver.query_one(
                "SELECT runtime FROM semon_pg_native_catalog_stage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton",
                &[&namespace.as_str().as_bytes()],
            ).map_err(sqlstate::error)?.get(0);
            assert_eq!(runtime, expected.as_bytes());
            Ok(())
        }).unwrap();
    }
}

fn withdrawal_preflight(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    options: &Options,
    batch: &crate::model::relation_stage::Batch,
    saved: &crate::model::PreparedSources,
) {
    let relative = "sessions/withdrawal-fence-check.jsonl";
    let path = options.codex_home.join(relative);
    fs::write(&path, b"").unwrap();
    let mut negative = saved.withdrawals[0].clone();
    negative.relative = relative.into();
    negative.path = path.to_string_lossy().into_owned();
    negative.stat = crate::events::Stat::of(&path).unwrap();
    let mut prepared = crate::model::PreparedSources {
        base_generation: saved.base_generation.clone(),
        claims: Vec::new(),
        rows: Vec::new(),
        transcripts: Vec::new(),
        pids: Default::default(),
        lineage_dependencies: Vec::new(),
        message_dependencies: None,
        relationships: Default::default(),
        indexed_sources: Vec::new(),
        removed_sources: Default::default(),
        indexed_native_parents: Vec::new(),
        withdrawals: vec![negative],
        withdrawal_fence: None,
    };
    prepared.retain_withdrawal_fence(options).unwrap();
    assert!(prepared.withdrawal_guard().unwrap().is_some());

    let cancelled = PgCancellation::default();
    let (mut refused, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        cancelled.clone(),
    )
    .unwrap();
    cancelled.cancel();
    assert!(matches!(
        refused.record_semantic_batch(batch, &prepared),
        Err(crate::events::StoreError::Configured(StorageError::Busy))
    ));
    fs::write(&path, b"changed").unwrap();
    assert!(
        matches!(
            refused.record_semantic_batch(batch, &prepared),
            Err(crate::events::StoreError::Configured(
                StorageError::Conflict
            ))
        ),
        "changed physical witness refuses before cancelled SQL admission"
    );
    fs::remove_file(&path).unwrap();
    assert!(
        matches!(
            refused.record_semantic_batch(batch, &prepared),
            Err(crate::events::StoreError::Configured(
                StorageError::Conflict
            ))
        ),
        "missing physical witness refuses before cancelled SQL admission"
    );
}

#[cfg(unix)]
fn withdrawal_commit_hook(
    options: &Options,
    prepared: &crate::model::PreparedSources,
) -> std::rc::Rc<std::cell::Cell<bool>> {
    let cache = options.cache.clone();
    let paths = prepared
        .withdrawals
        .iter()
        .map(|w| w.path.clone())
        .collect::<Vec<_>>();
    let checked = std::rc::Rc::new(std::cell::Cell::new(false));
    let hit = checked.clone();
    let callback = Box::new(move || {
        for path in std::iter::once("facts").chain(paths.iter().map(String::as_str)) {
            assert!(
                matches!(
                    crate::source_change::SourceLock::try_exclusive_cache(&cache, path),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                ),
                "withdrawal guard must survive the SQL callback through COMMIT"
            );
        }
        hit.set(true);
    });
    crate::storage::postgres::txn::BEFORE_WRITE_COMMIT.with(|hook| {
        assert!(hook.borrow_mut().replace(callback).is_none());
    });
    checked
}

#[cfg(unix)]
fn withdrawal_guard_released(options: &Options, prepared: &crate::model::PreparedSources) {
    for path in std::iter::once("facts").chain(prepared.withdrawals.iter().map(|w| w.path.as_str()))
    {
        assert!(
            crate::source_change::SourceLock::try_exclusive_cache(&options.cache, path).is_ok()
        );
    }
}

#[test]
fn pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite() {
    const NAME: &str = "storage::postgres::catalog_effects::tests::businessflow::pg_vertical140_ordinary_model_after_ingest_and_restart_without_sqlite";
    const CHILD: &str = "SEMON_VERTICAL140_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD, "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    // Producer parity precedes the process-wide rejection phase. No model
    // output from the oracle is supplied to the PG producer.
    let mut oracle = build_oracle(true, false);
    let namespace = ns("vertical140\0🦀");
    let source = crate::storage::HostSource::new(namespace.clone(), "a".repeat(64)).unwrap();
    let options = Options {
        source,
        claude_home: oracle.root.join("claude"),
        claude_json: oracle.root.join("claude.json"),
        codex_home: oracle.root.join("codex"),
        copilot_home: oracle.root.join("copilot"),
        proc_root: oracle.root.join("proc"),
        cache: oracle.root.join("never-open.sqlite3"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    // Use the ordinary harness contracts, including Copilot's UUID directory
    // and versioned session.start payload (model/tests.rs fixtures).
    let hostname = options.proc_root.join("sys/kernel/hostname");
    fs::create_dir_all(hostname.parent().unwrap()).unwrap();
    fs::write(&hostname, "synthetic-local-before207\n").unwrap();
    let local_authority =
        crate::model::relation_stage::LifecycleAuthority::capture(&options, &EventCache::default())
            .unwrap();
    assert!(local_authority.current().unwrap());
    fs::write(&hostname, "synthetic-local-after207\n").unwrap();
    assert!(!local_authority.current().unwrap());
    fs::write(&hostname, "synthetic-local-before207\n").unwrap();
    let codex_id = "01234567-89ab-4cde-8123-456789abcdef";
    let copilot_id = "11234567-89ab-4cde-8123-456789abcdef";
    let codex = options
        .codex_home
        .join(format!("sessions/2026/10/01/rollout-{codex_id}.jsonl"));
    let copilot = options
        .copilot_home
        .join(format!("session-state/{copilot_id}/events.jsonl"));
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    fs::create_dir_all(copilot.parent().unwrap()).unwrap();
    fs::write(&codex, [
        serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":codex_id,"cwd":"/synthetic/project","originator":"codex_exec","thread_source":"user","timestamp":"2026-10-01T00:00:00Z"}}),
        serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5","cwd":"/synthetic/project"}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized codex input"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(&copilot, [
        serde_json::json!({"id":"start","type":"session.start","timestamp":"2026-10-01T00:00:00Z","data":{"version":1,"copilotVersion":"1.0.91","sessionId":copilot_id,"context":{"cwd":"/synthetic/project"}}}),
        serde_json::json!({"id":"user","type":"user.message","timestamp":"2026-10-01T00:00:01Z","data":{"content":"sanitized copilot input"}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&codex).unwrap(), "{}",
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"related-spawn","arguments":"{\"task_name\":\"worker\",\"message\":\"go\"}"}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&codex).unwrap(), "{}",
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"related-spawn","output":"{\"task_name\":\"worker\"}"}})).unwrap();
    let child_id = "21234567-89ab-4cde-8123-456789abcdef";
    let child = options
        .codex_home
        .join(format!("sessions/2026/10/01/rollout-{child_id}.jsonl"));
    fs::write(&child, [
        serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":child_id,"cwd":"/synthetic/project","originator":"codex_exec","parent_thread_id":codex_id,"thread_source":"subagent","agent_path":"root/worker","agent_nickname":"worker","timestamp":"2026-10-01T00:00:00Z"}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"sanitized related codex input"}]}}),
        serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"sanitized related codex reply"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
    let claude_parent = options.claude_home.join("projects/project/one.jsonl");
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related-agent-call","parentUuid":"a1","timestamp":"2026-10-01T00:00:01Z","message":{"id":"related-agent-call","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"claude-related-spawn","name":"Agent","input":{"description":"sanitized agent task","prompt":"sanitized agent prompt"}}]}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related-agent-result","parentUuid":"related-agent-call","timestamp":"2026-10-01T00:00:02Z","toolUseResult":{"status":"async_launched","agentId":"related154"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"claude-related-spawn","content":"launched","is_error":false}]}})).unwrap();
    let claude_agent = options
        .claude_home
        .join("projects/project/one/subagents/agent-related154.jsonl");
    fs::create_dir_all(claude_agent.parent().unwrap()).unwrap();
    fs::write(&claude_agent,[
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related-user","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized agent prompt"}}),
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related-answer","parentUuid":"related-user","timestamp":"2026-10-01T00:00:02Z","message":{"id":"related-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized agent answer"}]}}),
    ].iter().map(|line|format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(claude_agent.with_extension("meta.json"),serde_json::json!({"agentType":"general-purpose","description":"sanitized agent task","toolUseId":"claude-related-spawn"}).to_string()).unwrap();
    // A second connected subagent of the same parent; the catch-up below
    // withdraws both, so their new custodies exceed one bounded advance.
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related191-agent-call","parentUuid":"related-agent-result","timestamp":"2026-10-01T00:00:01Z","message":{"id":"related191-agent-call","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"claude-related191-spawn","name":"Agent","input":{"description":"sanitized agent task","prompt":"sanitized agent prompt"}}]}})).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related191-agent-result","parentUuid":"related191-agent-call","timestamp":"2026-10-01T00:00:03Z","toolUseResult":{"status":"async_launched","agentId":"related191"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"claude-related191-spawn","content":"launched","is_error":false}]}})).unwrap();
    let claude_agent191 = options
        .claude_home
        .join("projects/project/one/subagents/agent-related191.jsonl");
    fs::create_dir_all(claude_agent191.parent().unwrap()).unwrap();
    fs::write(&claude_agent191,[
        serde_json::json!({"type":"user","sessionId":"one","uuid":"related191-user","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized agent prompt"}}),
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"related191-answer","parentUuid":"related191-user","timestamp":"2026-10-01T00:00:02Z","message":{"id":"related191-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized agent answer"}]}}),
    ].into_iter().chain((0..75).flat_map(|turn| {
        // A long subagent: its withdrawn retained recipe spans more than two
        // 64-slot custody pages, so the catch-up below resumes several times.
        let parent = if turn == 0 { "related191-answer".to_owned() } else { format!("related191-answer{}", turn - 1) };
        [
            serde_json::json!({"type":"user","sessionId":"one","uuid":format!("related191-user{turn}"),"parentUuid":parent,"timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":format!("sanitized agent follow-up {turn}")}}),
            serde_json::json!({"type":"assistant","sessionId":"one","uuid":format!("related191-answer{turn}"),"parentUuid":format!("related191-user{turn}"),"timestamp":"2026-10-01T00:00:02Z","message":{"id":format!("related191-answer{turn}"),"role":"assistant","model":"claude-test","content":[{"type":"text","text":format!("sanitized agent answer {turn}")}]}}),
        ]
    })).map(|line|format!("{line}\n")).collect::<String>()).unwrap();
    fs::write(claude_agent191.with_extension("meta.json"),serde_json::json!({"agentType":"general-purpose","description":"sanitized agent task","toolUseId":"claude-related191-spawn"}).to_string()).unwrap();
    // A real SendMessage result and peer receipt exercise the matched-sender
    // selector, rather than only proving that an absent receipt stays absent.
    for line in [
        serde_json::json!({"type":"assistant","sessionId":"one","uuid":"receipt203-send","parentUuid":"related191-agent-result","timestamp":"2026-10-01T00:00:04Z","message":{"id":"receipt203-send","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"receipt203-call","name":"SendMessage","input":{"to":"related154","message":"sanitized matched receipt203"}}]}}),
        serde_json::json!({"type":"user","sessionId":"one","uuid":"receipt203-result","parentUuid":"receipt203-send","timestamp":"2026-10-01T00:00:04Z","toolUseResult":{"success":true,"msg_id":"receipt203-message"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"receipt203-call","content":"sent","is_error":false}]}}),
    ] {
        writeln!(
            fs::OpenOptions::new()
                .append(true)
                .open(&claude_parent)
                .unwrap(),
            "{line}"
        )
        .unwrap();
    }
    writeln!(fs::OpenOptions::new().append(true).open(&claude_agent).unwrap(), "{}",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"receipt203-peer","parentUuid":"related-answer","timestamp":"2026-10-01T00:00:05Z","origin":{"kind":"peer","from":"uds:/run/user/1000/cc-socks/203.sock","name":"sanitized sender203","msg_id":"receipt203-message","body":"sanitized matched receipt203"},"message":{"role":"user","content":"<cross-session-message from=\"uds:/run/user/1000/cc-socks/203.sock\" from-name=\"sanitized sender203\">sanitized matched receipt203</cross-session-message>"}})).unwrap();
    // Positive ClaudeName: the native custom-title record names the session.
    writeln!(fs::OpenOptions::new().append(true).open(&claude_parent).unwrap(), "{}",
        serde_json::json!({"type":"custom-title","sessionId":"one","customTitle":"sanitized name175"})).unwrap();
    // Current upstream copy/resume semantics: the shared UUIDs retain one
    // native send, while the resumed file contributes its own new turn.
    let copied_records = [
        serde_json::json!({"type":"user","sessionId":"copy203-original","uuid":"copy203-user","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized copy203 input"}}),
        serde_json::json!({"type":"assistant","sessionId":"copy203-original","uuid":"copy203-send","parentUuid":"copy203-user","timestamp":"2026-10-01T00:00:01Z","message":{"id":"copy203-send","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":"copy203-call","name":"SendMessage","input":{"to":"sanitized external203","message":"sanitized external203 request"}}]}}),
        serde_json::json!({"type":"user","sessionId":"copy203-original","uuid":"copy203-result","parentUuid":"copy203-send","timestamp":"2026-10-01T00:00:01Z","toolUseResult":{"success":true},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"copy203-call","content":"sent","is_error":false}]}}),
    ];
    let original = options
        .claude_home
        .join("projects/project/copy203-original.jsonl");
    let resumed = options
        .claude_home
        .join("projects/project/copy203-resumed.jsonl");
    let mut old = copied_records.to_vec();
    old.push(serde_json::json!({"type":"continued-in","continuedInSessionId":"copy203-resumed","sessionId":"copy203-original","timestamp":"2026-10-01T00:00:02Z"}));
    let mut copy = copied_records.to_vec();
    copy.push(serde_json::json!({"type":"user","sessionId":"copy203-resumed","uuid":"copy203-new","parentUuid":"copy203-result","timestamp":"2026-10-01T00:00:03Z","message":{"role":"user","content":"sanitized resumed203 input"}}));
    for (path, records) in [(original, old), (resumed, copy)] {
        fs::write(
            path,
            records
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    // A fresh same-harness owner with two unchanged sources exercises missing
    // source composition without an earlier original History owner.
    let composite_key = "fresh-composite";
    let composite_a_path = options
        .claude_home
        .join("projects/project/fresh-composite-a.jsonl");
    let composite_b_path = options
        .claude_home
        .join("projects/project/fresh-composite-b.jsonl");
    for (path, records) in [
        (
            &composite_a_path,
            [
                serde_json::json!({"type":"user","sessionId":composite_key,"uuid":"fresh-a-user","timestamp":"2026-10-01T00:00:05Z","cwd":"/synthetic/project","message":{"role":"user","content":"sanitized fresh composite A"}}),
                serde_json::json!({"type":"assistant","sessionId":composite_key,"uuid":"fresh-a-answer","parentUuid":"fresh-a-user","timestamp":"2026-10-01T00:00:06Z","message":{"id":"fresh-a-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized fresh composite A reply"}]}}),
            ],
        ),
        (
            &composite_b_path,
            [
                serde_json::json!({"type":"user","sessionId":composite_key,"uuid":"fresh-b-user","timestamp":"2026-10-01T00:00:07Z","cwd":"/synthetic/project","message":{"role":"user","content":"sanitized fresh composite B"}}),
                serde_json::json!({"type":"assistant","sessionId":composite_key,"uuid":"fresh-b-answer","parentUuid":"fresh-b-user","timestamp":"2026-10-01T00:00:08Z","message":{"id":"fresh-b-answer","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized fresh composite B reply"}]}}),
            ],
        ),
    ] {
        fs::write(
            path,
            records
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    // Larger indexed history with a fixed small receiver append below.
    let padding = format!(
        "{}\n",
        serde_json::json!({"type":"system","subtype":"turn_duration","sessionId":"one","padding":"x".repeat(8192)})
    );
    let mut history = fs::OpenOptions::new()
        .append(true)
        .open(&claude_parent)
        .unwrap();
    for _ in 0..256 {
        history.write_all(padding.as_bytes()).unwrap();
    }
    history.sync_all().unwrap();
    drop(history);
    let mut inputs = crate::inputs::inputs(&options)
        .unwrap()
        .into_iter()
        .filter(|input| {
            crate::native_evidence::catalog_input(&input.full_path(&options), &options).is_some()
        })
        .collect::<Vec<_>>();
    // The trusted append below targets this exact padded parent, independently
    // of the discovery order of copies, subagents and other harnesses.
    let append_input = inputs
        .iter()
        .position(|input| input.full_path(&options) == claude_parent)
        .expect("padded Claude parent is a discovered native input");
    inputs.swap(0, append_input);
    let mut sqlite_options = options.clone();
    sqlite_options.cache = oracle.root.join("oracle-all140");
    let mut sqlite =
        EventCache::open_scoped(&sqlite_options.cache, &sqlite_options.source).unwrap();
    let expected =
        crate::model::prepare_sources(&sqlite_options, &mut sqlite, &inputs, 1790812802000)
            .unwrap();
    assert_eq!(
        expected
            .rows
            .iter()
            .map(|row| row.harness.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["claude", "codex", "copilot"])
    );
    sqlite
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &expected.rows,
                transcripts: Some(&expected.transcripts),
                observations: None,
            },
            expected.base_generation.as_deref(),
        )
        .expect("SQLite complete ordinary source publication");
    // Selected body/field reads consume the fully published ordinary Query
    // profile, including its completion and reader binding, not a partial head.
    let mut oracle_query =
        crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(
            vec![("vertical140-oracle".into(), sqlite_options.clone())],
        )));
    oracle_query
        .call_at("list_sessions", &serde_json::json!({}), 1790812802000)
        .expect("SQLite fully published body/field oracle");
    let expected_bodies = expected
        .rows
        .iter()
        .map(|row| {
            let body = oracle_query
                .call_at(
                    "read_transcript",
                    &serde_json::json!({"id":row.key,"limit":100}),
                    1790812802000,
                )
                .unwrap();
            assert!(
                body.to_string().contains("sanitized"),
                "ordinary body missing for {} {} {:?}",
                row.harness,
                row.kind,
                row.native_ids
            );
            let session = oracle_query
                .call_at(
                    "get_session",
                    &serde_json::json!({"id":row.key}),
                    1790812802000,
                )
                .expect("SQLite ordinary session fields");
            (
                row.key.clone(),
                serde_json::json!({"body":body,"fields":session}),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    // Recent windows (options.all=false): the ordinary SQLite Query refresh
    // fully publishes a fresh oracle cache; its exact lists are saved before
    // the PG no-SQLite phase.
    let recent_windows = [
        Duration::from_secs(86400),
        Duration::from_secs(86401),
        Duration::from_secs(86400 * 365 * 100),
    ];
    // One day after the fixtures: the cutoffs fall between fixture times.
    let recent_now = 1790812802000 + 86_400_000;
    let recent_machine = "vertical140\0🦀";
    let mut closed_options = options.clone();
    closed_options.cache = oracle.root.join("oracle-query177");
    let oracle_recent = recent_windows
        .iter()
        .map(|since| {
            let mut recent_options = closed_options.clone();
            recent_options.all = false;
            recent_options.since = *since;
            let list = crate::Query::from_sources(std::sync::Arc::new(
                crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
            ))
            .call_at("list_sessions", &serde_json::json!({}), recent_now)
            .expect("SQLite recent-window list");
            (*since, list)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let oracle_recent_fields = recent_windows
        .iter()
        .map(|since| {
            let mut recent_options = closed_options.clone();
            recent_options.all = false;
            recent_options.since = *since;
            let mut query = crate::Query::from_sources(std::sync::Arc::new(
                crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
            ));
            let fields = expected
                .rows
                .iter()
                .map(|row| {
                    let fields = query.call_at(
                        "get_session",
                        &serde_json::json!({"id":row.key}),
                        recent_now,
                    );
                    (row.key.clone(), fields)
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            (*since, fields)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let included_key = &expected.rows.first().expect("ordinary owner fixture").key;
    let included_fields = oracle_recent_fields[&recent_windows[2]][included_key]
        .as_ref()
        .expect("all-history recent window includes the ordinary owner");
    assert!(included_fields["session"].is_object());
    assert_eq!(included_fields["session"]["id"], *included_key);
    let all_total = expected
        .rows
        .iter()
        .flat_map(|row| {
            row.query.as_ref().unwrap()["rows"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u64;
    assert_eq!(
        oracle_recent[&recent_windows[2]]["total"].as_u64(),
        Some(all_total)
    );
    // At least one window has Sessions on both sides of its cutoff.
    assert!(
        oracle_recent.values().any(|list| list["total"]
            .as_u64()
            .is_some_and(|total| total > 0 && total < all_total)),
        "recent-window fixture lacks a boundary: {:?}",
        oracle_recent
            .values()
            .map(|list| &list["total"])
            .collect::<Vec<_>>()
    );
    // Retained-history scope through the ordinary SQLite viewer page over the
    // fully published Query cache; exact ordered keys are saved before the PG
    // no-SQLite phase. Nothing is withdrawn yet, so it equals the current page.
    let mut history_options = closed_options.clone();
    history_options.all = false;
    history_options.since = recent_windows[2];
    let page_keys = |reply: crate::ViewerReply| -> Vec<String> {
        assert_eq!(reply.status, 200);
        let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let oracle_history = page_keys(crate::catalog::session_catalog_page(
        &history_options,
        recent_machine,
        "scope=retained_history&limit=50",
    ));
    let mut ordered = expected.rows.clone();
    ordered.sort_by(|a, b| {
        b.last
            .unwrap_or(0)
            .cmp(&a.last.unwrap_or(0))
            .then(a.key.cmp(&b.key))
    });
    assert_eq!(
        oracle_history,
        ordered
            .iter()
            .map(|row| row.key.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        oracle_history,
        page_keys(crate::catalog::session_catalog_page(
            &history_options,
            recent_machine,
            "limit=50",
        ))
    );
    // P3 oracle: the same connected withdrawal (both subagent Sources
    // disappear) through the ordinary SQLite Query refresh. Retained-only
    // rows, keyset pages and the harness filter are saved before the
    // no-SQLite phase; the Sources are restored byte-identical for PG ingest.
    let stable = |item: &serde_json::Value| {
        let mut picked = serde_json::Map::new();
        for field in [
            "key",
            "harness",
            "kind",
            "lifecycle",
            "name",
            "native_ids",
            "parent",
            "root",
            "repo",
            "branch",
            "model",
            "first",
            "last",
        ] {
            picked.insert(field.into(), item[field].clone());
        }
        serde_json::Value::Object(picked)
    };
    let mut withdraw_options = history_options.clone();
    withdraw_options.cache = oracle.root.join("oracle-withdraw201");
    // Replay the actual parent continuation and recorded PID/Facts context
    // consumed before PG withdrawal. Retained fields use that live context;
    // comparing them with the earlier local-machine oracle is not parity.
    let source_append = format!(
        "{}\n",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"u2","parentUuid":"a1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":"sanitized continuation"}})
    );
    let facts_append = format!(
        "{}\n",
        serde_json::json!({"type":"user","sessionId":"one","uuid":"positive-facts158","parentUuid":"u2","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:04Z","message":{"role":"user","content":"positive Facts source continuation"}})
    );
    let pid_bytes = serde_json::to_vec(&serde_json::json!({"sessionId":"one","procStart":1234,"startedAt":1790812800000i64,"status":"waiting","waitingFor":"input needed"})).unwrap();
    let parent_before_withdrawal = fs::read(&claude_parent).unwrap();
    let mut parent = fs::OpenOptions::new()
        .append(true)
        .open(&claude_parent)
        .unwrap();
    parent.write_all(source_append.as_bytes()).unwrap();
    parent.write_all(facts_append.as_bytes()).unwrap();
    drop(parent);
    let oracle_pid = options.claude_home.join("sessions/4242.json");
    fs::create_dir_all(oracle_pid.parent().unwrap()).unwrap();
    fs::write(&oracle_pid, &pid_bytes).unwrap();
    let oracle_facts = oracle.root.join("withdrawal-facts.json");
    let mut withdrawal_facts = crate::Facts {
        version: 2,
        hostname: "source-write-after158".into(),
        ..Default::default()
    };
    withdrawal_facts.proc_starts.insert(4242, 1234);
    crate::write_facts(&oracle_facts, &withdrawal_facts, &options.source).unwrap();
    withdraw_options.facts = Some(oracle_facts);
    let refresh = |options: &Options| {
        // Query's private listing completion does not finish Catalog/search
        // publication. Run the ordinary observer to completion before saving
        // the exact retained-search oracle, rather than forging its markers.
        let listing = crate::Query::from_sources(std::sync::Arc::new(
            crate::SessionSources::with_machines(vec![(recent_machine.into(), options.clone())]),
        ))
        .call_at("list_sessions", &serde_json::json!({}), recent_now)
        .expect("SQLite withdrawal oracle list");
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        evidence.request_metadata_audit();
        let mut work = evidence.work.lock().unwrap();
        let mut complete = false;
        // Each advance retains its normal bounded work. Qualify completion
        // against the existing background budget, not a source-count-sized
        // number of calls through the producer's multiple durable phases.
        let started = std::time::Instant::now();
        let mut advances = 0;
        while started.elapsed() < Duration::from_secs(60) {
            advances += 1;
            if evidence.refresh_catalog(&mut work, recent_now).unwrap() {
                complete = true;
                break;
            }
            if advances % 32 == 0 {
                let cache = work.events.as_ref().unwrap();
                eprintln!(
                    "ordinary_observer advances={advances} qualified={} pending={} backfill={:?}",
                    cache.qualified_catalog().unwrap(),
                    cache.pending_catalog_sources(1).unwrap().len(),
                    cache.backfill_state().unwrap()
                );
            }
        }
        assert!(complete, "SQLite ordinary Catalog oracle did not finish");
        drop(work);
        listing
    };
    let before_withdrawal = refresh(&withdraw_options)["total"].as_u64().unwrap();
    let sqlite_before_current = page_keys(crate::catalog::session_catalog_page(
        &withdraw_options,
        recent_machine,
        "limit=50",
    ))
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    let mut withdrawal_cache =
        EventCache::open_scoped(&withdraw_options.cache, &withdraw_options.source).unwrap();
    let withdrawal_prepared = crate::model::prepare_sources(
        &withdraw_options,
        &mut withdrawal_cache,
        &inputs,
        1790812805000,
    )
    .unwrap();
    let mut sqlite_host = body_host(&withdraw_options, &withdrawal_prepared.rows);
    sqlite_host.0.extend(body_host(&options, &expected.rows).0);
    drop(withdrawal_cache);
    let parked = [
        (
            claude_agent.clone(),
            oracle.root.join("parked-agent180.jsonl"),
        ),
        (
            claude_agent191.clone(),
            oracle.root.join("parked-agent191.jsonl"),
        ),
    ];
    for (from, to) in &parked {
        fs::rename(from, to).unwrap();
    }
    let after_withdrawal = refresh(&withdraw_options)["total"].as_u64().unwrap();
    assert!(
        after_withdrawal < before_withdrawal,
        "SQLite withdrawal {before_withdrawal} -> {after_withdrawal}"
    );
    let sqlite_scoped = |scope: &str, limit: usize| {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let query = match &cursor {
                Some(cursor) => format!(
                    "{scope}limit={limit}&cursor={}",
                    crate::viewer::percent_encode(cursor)
                ),
                None => format!("{scope}limit={limit}"),
            };
            let reply =
                crate::catalog::session_catalog_page(&withdraw_options, recent_machine, &query);
            assert_eq!(
                reply.status,
                200,
                "{query} code={}",
                serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap()["error"]["code"]
            );
            let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            items.extend(page["items"].as_array().unwrap().iter().cloned());
            match page["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break items,
            }
        }
    };
    let sqlite_retained = sqlite_scoped("scope=retained_history&", 50);
    let sqlite_parent_exact = crate::catalog::session_catalog_page(
        &withdraw_options,
        recent_machine,
        "sid=one&scope=retained_history&limit=50",
    );
    assert_eq!(sqlite_parent_exact.status, 200);
    let sqlite_parent_exact: serde_json::Value =
        serde_json::from_slice(&sqlite_parent_exact.body).unwrap();
    let sqlite_parent_transcript = crate::session_transcript_range_with_mode(
        &withdraw_options,
        recent_machine,
        "sid=one&scope=retained_history&limit=100",
        Some(&sqlite_host),
        crate::SessionSourceReadMode::ProviderOnly,
    );
    assert_eq!(sqlite_parent_transcript.status, 200);
    let sqlite_parent_transcript: serde_json::Value =
        serde_json::from_slice(&sqlite_parent_transcript.body).unwrap();
    // Keyset pages over equal timestamps continue without gap or overlap.
    assert_eq!(
        sqlite_scoped("scope=retained_history&", 3)
            .iter()
            .map(stable)
            .collect::<Vec<_>>(),
        sqlite_retained.iter().map(stable).collect::<Vec<_>>()
    );
    let sqlite_current = sqlite_scoped("", 50)
        .iter()
        .map(|item| item["key"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let oracle_withdrawn = sqlite_retained
        .iter()
        .filter(|item| !sqlite_current.contains(item["key"].as_str().unwrap()))
        .map(stable)
        .collect::<Vec<_>>();
    // Retained-history can select an original composite projection for a key
    // that also remains Current (the connected parent). Keep that full oracle
    // separately from the exact keys whose Current membership disappeared.
    let oracle_history_projection = sqlite_retained.iter().map(stable).collect::<Vec<_>>();
    assert!(
        oracle_withdrawn
            .iter()
            .all(|item| item["lifecycle"] == "retained")
    );
    // Query totals also count virtual relationship stubs, whose membership can
    // change when a sender disappears. Native Catalog custody is the exact
    // difference of the native Current key sets, not that Query total delta.
    assert_eq!(
        oracle_withdrawn
            .iter()
            .map(|row| row["key"].as_str().unwrap().to_owned())
            .collect::<std::collections::BTreeSet<_>>(),
        sqlite_before_current
            .difference(&sqlite_current)
            .cloned()
            .collect(),
        "SQLite retained-only rows"
    );
    assert!(
        oracle_withdrawn
            .iter()
            .all(|item| !sqlite_current.contains(item["key"].as_str().unwrap()))
    );
    let oracle_withdrawn_claude = sqlite_scoped("scope=retained_history&harness=claude&", 50)
        .iter()
        .filter(|item| !sqlite_current.contains(item["key"].as_str().unwrap()))
        .map(|item| item["key"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        oracle_withdrawn_claude,
        oracle_withdrawn
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    // Match the all-history request used by the PG retained-body comparison.
    // The withdrawal listing above deliberately uses a finite window; its
    // window_start envelope is not the envelope of this all-history read.
    // Keep the same published withdrawal cache and real Raw custody.
    let mut retained_options = withdraw_options.clone();
    retained_options.all = options.all;
    retained_options.since = options.since;
    let mut retained_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(recent_machine.into(), retained_options)]),
    ));
    let oracle_retained_reads = oracle_withdrawn
        .iter()
        .map(|row| {
            let key = row["key"].as_str().unwrap();
            let args = serde_json::json!({"id":key,"limit":100});
            let body = retained_query
                .call_selected_at(
                    recent_machine,
                    key,
                    "read_transcript",
                    &args,
                    Some(&sqlite_host),
                    crate::SessionSourceReadMode::LocalThenProvider,
                    1790812802000,
                )
                .expect("SQLite retained body through public Query and preserved Raw custody");
            let fields = retained_query
                .call_selected_at(
                    recent_machine,
                    key,
                    "get_session",
                    &serde_json::json!({"id":key}),
                    Some(&sqlite_host),
                    crate::SessionSourceReadMode::LocalThenProvider,
                    1790812802000,
                )
                .expect("SQLite retained fields through public Query and preserved Raw custody");
            assert_eq!(body["sid"], key);
            assert_eq!(fields["session"]["id"], key);
            (
                key.to_owned(),
                serde_json::json!({"body":body,"fields":fields}),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    // Both withdrawn Claude runs lack a native hand-back. Their historical
    // unfinished spawns keep them in an output window even after its cutoff
    // passes every timestamp. Capture full public responses for each context,
    // including the Query's window_start envelope, before the SQLite guard.
    let expired_window_at = 1790812802000 + 86_400_000;
    let retained_window_contexts = [
        (true, Duration::from_secs(86400 * 365 * 100), 1790812802000),
        (false, Duration::from_secs(86400 * 365 * 100), 1790812802000),
        (false, Duration::from_secs(1), expired_window_at),
    ];
    assert_eq!(
        oracle_withdrawn
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["related154", "related191"])
    );
    let mut oracle_retained_windows = std::collections::BTreeMap::new();
    for &(all, since, at) in &retained_window_contexts {
        let mut window_options = withdraw_options.clone();
        window_options.all = all;
        window_options.scan_window = false;
        window_options.since = since;
        let mut window_query = crate::Query::from_sources(std::sync::Arc::new(
            crate::SessionSources::with_machines(vec![(recent_machine.into(), window_options)]),
        ));
        for row in &oracle_withdrawn {
            let key = row["key"].as_str().unwrap();
            let body = window_query
                .call_selected_at(
                    recent_machine,
                    key,
                    "read_transcript",
                    &serde_json::json!({"id":key,"limit":100}),
                    Some(&sqlite_host),
                    crate::SessionSourceReadMode::LocalThenProvider,
                    at,
                )
                .expect("SQLite unfinished retained body in its output window");
            let fields = window_query
                .call_selected_at(
                    recent_machine,
                    key,
                    "get_session",
                    &serde_json::json!({"id":key}),
                    Some(&sqlite_host),
                    crate::SessionSourceReadMode::LocalThenProvider,
                    at,
                )
                .expect("SQLite unfinished retained fields in their output window");
            assert!(
                fields["session"]["handoffs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|handoff| handoff["kind"] == "spawn" && handoff["status"] == "work")
            );
            let window_start = (!all).then(|| at - i64::try_from(since.as_millis()).unwrap());
            assert_eq!(body["window_start"], serde_json::json!(window_start));
            assert_eq!(fields["window_start"], serde_json::json!(window_start));
            oracle_retained_windows.insert(
                (all, since.as_secs(), at, key.to_owned()),
                serde_json::json!({"body":body,"fields":fields}),
            );
        }
    }
    // Codex accepts this child's final assistant reply as its native result;
    // it has no live writer lock or unfinished handoff. An old last turn alone
    // must not retain its membership after the output cutoff.
    let mut expired_options = withdraw_options.clone();
    expired_options.all = false;
    expired_options.scan_window = false;
    expired_options.since = Duration::from_secs(1);
    let mut expired_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(recent_machine.into(), expired_options)]),
    ));
    let oracle_expired_reads = ["get_session", "read_transcript"]
        .into_iter()
        .map(|tool| {
            let error = expired_query
                .call_selected_at(
                    recent_machine,
                    child_id,
                    tool,
                    &serde_json::json!({"id":child_id}),
                    Some(&sqlite_host),
                    crate::SessionSourceReadMode::LocalThenProvider,
                    expired_window_at,
                )
                .unwrap_err();
            assert_eq!(error.code, "unknown_session");
            (tool, error)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let oracle_retained_search = oracle_withdrawn
        .iter()
        .map(|row| {
            let key = row["key"].as_str().unwrap();
            let query = format!(
                "scope=retained_history&q={}&",
                crate::viewer::percent_encode(&key.to_lowercase())
            );
            let keys = sqlite_scoped(&query, 50)
                .iter()
                .map(|item| item["key"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            assert!(keys.contains(&key.to_owned()));
            (key.to_owned(), keys)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for (from, to) in &parked {
        fs::rename(to, from).unwrap();
    }
    fs::write(&claude_parent, &parent_before_withdrawal).unwrap();
    fs::remove_file(&oracle_pid).unwrap();
    // Capture the ordinary Query's post-withdrawal membership independently,
    // before the process-wide Guard. Query membership includes virtual peers
    // as well as physical Catalog owners.
    let copilot_parked = oracle.root.join("oracle-copilot220.jsonl");
    fs::rename(&copilot, &copilot_parked).unwrap();
    let mut withdrawal_options = options.clone();
    withdrawal_options.cache = oracle.root.join("oracle-withdrawal220");
    let withdrawal_oracle = crate::Query::new(withdrawal_options)
        .call_at("list_sessions", &serde_json::json!({}), 1790812802000)
        .expect("SQLite ordinary withdrawal cohort");
    fs::rename(&copilot_parked, &copilot).unwrap();
    // Rename preserves bytes but changes the physical ctime witness. Capture
    // the SQLite baseline from the restored Sources before NoSQLiteGuard, so
    // PG must match the same observation, including all proof fields.
    let expected =
        crate::model::prepare_sources(&sqlite_options, &mut sqlite, &inputs, 1790812802000)
            .unwrap();
    // The retained-row oracle consumes the same restored physical witnesses.
    // Its order remains the independently observed SQLite page order above.
    ordered = oracle_history
        .iter()
        .map(|key| {
            expected
                .rows
                .iter()
                .find(|row| &row.key == key)
                .unwrap()
                .clone()
        })
        .collect();
    oracle.rows = expected.rows;
    oracle.projections = expected.transcripts;
    drop(sqlite);
    let raw = inputs
        .iter()
        .map(|i| fs::read(i.full_path(&options)).unwrap())
        .collect::<Vec<_>>();
    let appended_path = inputs[0].full_path(&options);
    let appended_key = oracle
        .rows
        .iter()
        .find(|row| {
            row.sources
                .iter()
                .any(|source| source.path == appended_path)
        })
        .expect("SQLite prefix oracle owns the appended Claude source")
        .key
        .clone();
    let related_key = oracle
        .rows
        .iter()
        .find(|row| row.key == "related154" || row.native_ids.iter().any(|id| id == "related154"))
        .expect("fixture publishes the related Claude subagent")
        .key
        .clone();
    let append_oracle_root = oracle.root.join("oracle-same-append");
    let mut append_oracle_options = options.clone();
    append_oracle_options.claude_home = append_oracle_root.join("claude");
    append_oracle_options.codex_home = append_oracle_root.join("codex");
    append_oracle_options.copilot_home = append_oracle_root.join("copilot");
    append_oracle_options.cache = append_oracle_root.join("index.sqlite3");
    let append_oracle_path = append_oracle_options
        .claude_home
        .join("projects/project/one.jsonl");
    // Native neighbors and metadata determine spawn and receipt rendering.
    // The Catalog-only inputs above omit the subagent metadata sidecars.
    for input in crate::inputs::inputs(&options).unwrap() {
        let destination = input.full_path(&append_oracle_options);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(input.full_path(&options), destination).unwrap();
    }
    // Query completion only derives live Query data. Publish the real model
    // first so the append replaces an admitted Catalog/recipe generation.
    let mut append_oracle_cache =
        EventCache::open_scoped(&append_oracle_options.cache, &append_oracle_options.source)
            .unwrap();
    let append_prefix_prepared = crate::model::prepare_sources(
        &append_oracle_options,
        &mut append_oracle_cache,
        &inputs,
        1790812802000,
    )
    .unwrap();
    let append_prefix_generation = append_oracle_cache
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &append_prefix_prepared.rows,
                transcripts: Some(&append_prefix_prepared.transcripts),
                observations: append_prefix_prepared.presentation_observations(),
            },
            append_prefix_prepared.base_generation.as_deref(),
        )
        .expect("SQLite same-append original Catalog publication")
        .expect("SQLite original Catalog generation");
    let mut append_oracle_query =
        crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(
            vec![(recent_machine.into(), append_oracle_options.clone())],
        )));
    let prefix_list = append_oracle_query
        .call_at("list_sessions", &serde_json::json!({}), 1790812802000)
        .expect("SQLite same-append prefix Query");
    let prefix_ids = prefix_list["sessions"].as_array().unwrap();
    let append_oracle_key = prefix_ids
        .iter()
        .find(|session| session["id"].as_str() == Some(appended_key.as_str()))
        .and_then(|session| session["id"].as_str())
        .expect("append oracle includes the original parent in the full native cohort");
    let append_prefix_body = append_oracle_query
        .call_at(
            "read_transcript",
            &serde_json::json!({"id":append_oracle_key,"limit":100}),
            1790812802000,
        )
        .expect("SQLite same-append original prefix body");
    assert_eq!(
        append_prefix_body, expected_bodies[&appended_key]["body"],
        "isolated SQLite prefix agrees with the full-source oracle"
    );
    // Keep the full response for whole-JSON parity below. The retained-history
    // check itself uses the explicitly scoped range API and a saved source prefix.
    let append_history_body = append_prefix_body.clone();
    struct SavedPrefix(Vec<(crate::SessionSourceRef, std::sync::Arc<[u8]>)>);
    impl crate::SessionSourceReader for SavedPrefix {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            let (saved, bytes) = self
                .0
                .iter()
                .find(|(saved, _)| saved == source)
                .ok_or_else(|| std::io::Error::other("saved prefix denied"))?;
            let generation: String = saved
                .prefix_sha256
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let end = offset
                .checked_add(max as u64)
                .filter(|end| *end <= saved.offset)
                .and_then(|end| usize::try_from(end).ok())
                .ok_or_else(|| std::io::Error::other("saved prefix range"))?;
            let start = usize::try_from(offset).map_err(std::io::Error::other)?;
            if expected.is_some_and(|expected| expected != generation)
                || saved.offset > bytes.len() as u64
            {
                return Err(std::io::Error::other("saved prefix changed"));
            }
            Ok(crate::SessionSourceRange {
                generation,
                length: saved.offset,
                offset,
                bytes: bytes
                    .get(start..end)
                    .ok_or_else(|| std::io::Error::other("saved prefix range"))?
                    .to_vec()
                    .into(),
                cached: true,
            })
        }
    }
    let append_history_identity = crate::session_catalog_history_identity(
        &append_oracle_options,
        recent_machine,
        &appended_key,
    )
    .unwrap()
    .expect("SQLite append oracle has a retained-history identity");
    assert_eq!(
        append_history_identity.read_scope,
        crate::CatalogReadScope::RetainedHistory
    );
    assert_eq!(append_history_identity.generation, append_prefix_generation);
    let saved_prefixes = append_history_identity
        .source_refs
        .iter()
        .map(|observation| {
            let source = observation.source.clone();
            let root = match source.root.as_str() {
                "claude" => &append_oracle_options.claude_home,
                "codex" => &append_oracle_options.codex_home,
                "copilot" => &append_oracle_options.copilot_home,
                other => panic!("unexpected native source root: {other}"),
            };
            let bytes = fs::read(root.join(&source.path)).unwrap();
            let prefix_len = usize::try_from(source.offset).unwrap();
            assert!(prefix_len <= bytes.len());
            use sha2::Digest;
            let digest: [u8; 32] = sha2::Sha256::digest(&bytes[..prefix_len]).into();
            assert_eq!(digest, source.prefix_sha256);
            let bytes: std::sync::Arc<[u8]> = bytes[..prefix_len].to_vec().into();
            (source, bytes)
        })
        .collect::<Vec<_>>();
    let append_relative_path = append_oracle_path
        .strip_prefix(&append_oracle_options.claude_home)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        saved_prefixes
            .iter()
            .any(|(source, _)| { source.root == "claude" && source.path == append_relative_path })
    );
    let append_history_reader = SavedPrefix(saved_prefixes);
    let append_history_query = format!(
        "sid={}&scope=retained_history&after=0&limit=100",
        crate::viewer::percent_encode(append_oracle_key)
    );
    let append_history_prefix = crate::session_transcript_range_with_mode(
        &append_oracle_options,
        recent_machine,
        &append_history_query,
        Some(&append_history_reader),
        crate::SessionSourceReadMode::ProviderOnly,
    );
    assert_eq!(
        append_history_prefix.status,
        200,
        "SQLite retained-history prefix: {}",
        String::from_utf8_lossy(&append_history_prefix.body)
    );
    let append_history_prefix: serde_json::Value =
        serde_json::from_slice(&append_history_prefix.body).unwrap();
    assert_eq!(
        append_history_prefix["identity"]["read_scope"],
        "retained_history"
    );
    assert_eq!(
        append_history_prefix["content_observation"]["state"],
        "cached"
    );
    assert_eq!(
        append_history_prefix["projection"]["total"].as_u64(),
        Some(append_history_prefix["entries"].as_array().unwrap().len() as u64),
        "SQLite original prefix range includes every recipe"
    );
    let append_writer = crate::SessionSourceWrite::begin(
        &append_oracle_options,
        &inputs[0],
        None,
        raw[0].len() as u64,
    )
    .unwrap();
    let mut append_oracle_file = fs::OpenOptions::new()
        .append(true)
        .open(&append_oracle_path)
        .unwrap();
    append_oracle_file
        .write_all(source_append.as_bytes())
        .unwrap();
    append_oracle_file.sync_all().unwrap();
    drop(append_oracle_file);
    append_writer
        .complete(raw[0].len() as u64 + source_append.len() as u64)
        .unwrap();
    let append_successor_prepared = crate::model::prepare_sources(
        &append_oracle_options,
        &mut append_oracle_cache,
        &inputs,
        1790812803000,
    )
    .unwrap();
    assert_eq!(
        append_successor_prepared.base_generation.as_deref(),
        Some(append_prefix_generation.as_str())
    );
    let append_successor_generation = append_oracle_cache
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &append_successor_prepared.rows,
                transcripts: Some(&append_successor_prepared.transcripts),
                observations: append_successor_prepared.presentation_observations(),
            },
            append_successor_prepared.base_generation.as_deref(),
        )
        .expect("SQLite same-append successor Catalog publication")
        .expect("SQLite successor Catalog generation");
    assert_ne!(append_successor_generation, append_prefix_generation);
    let mut append_successor_query =
        crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(
            vec![(recent_machine.into(), append_oracle_options.clone())],
        )));
    append_successor_query
        .call_at("list_sessions", &serde_json::json!({}), 1790812803000)
        .expect("SQLite same-append successor Query");
    let append_successor_related_body = append_successor_query
        .call_at(
            "read_transcript",
            &serde_json::json!({"id":related_key,"limit":100}),
            1790812803000,
        )
        .expect("SQLite current related-session body after append");
    let append_successor_related_fields = append_successor_query
        .call_at(
            "get_session",
            &serde_json::json!({"id":related_key}),
            1790812803000,
        )
        .expect("SQLite current related-session fields after append");
    let append_current_identity =
        crate::session_catalog_identity(&append_oracle_options, recent_machine, &appended_key)
            .unwrap()
            .expect("SQLite append successor has a Current identity");
    assert_eq!(
        append_current_identity.generation,
        append_successor_generation
    );
    let appended_offset = |identity: &crate::CatalogSessionIdentity| {
        identity
            .source_refs
            .iter()
            .find(|reference| {
                reference.source.root == "claude" && reference.source.path == append_relative_path
            })
            .unwrap()
            .source
            .offset
    };
    assert_eq!(
        appended_offset(&append_current_identity),
        appended_offset(&append_history_identity) + source_append.len() as u64,
        "SQLite Current consumes the actual appended native record"
    );
    let append_history_after = crate::session_transcript_range_with_mode(
        &append_oracle_options,
        recent_machine,
        &append_history_query,
        Some(&append_history_reader),
        crate::SessionSourceReadMode::ProviderOnly,
    );
    assert_eq!(
        append_history_after.status,
        200,
        "SQLite retained-history successor: {}",
        String::from_utf8_lossy(&append_history_after.body)
    );
    let append_history_after: serde_json::Value =
        serde_json::from_slice(&append_history_after.body).unwrap();
    assert_eq!(
        append_history_after["identity"]["read_scope"],
        "retained_history"
    );
    assert_eq!(
        append_history_after["identity"]["generation"],
        append_successor_generation
    );
    assert_eq!(
        append_history_after["identity"]["source_refs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|reference| reference["source"].clone())
            .collect::<Vec<_>>(),
        append_history_prefix["identity"]["source_refs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|reference| reference["source"].clone())
            .collect::<Vec<_>>(),
        "SQLite History keeps the original source prefix after append"
    );
    assert_eq!(
        append_history_after["projection"], append_history_prefix["projection"],
        "SQLite History keeps the original projection after append"
    );
    assert_eq!(
        append_history_after["entries"], append_history_prefix["entries"],
        "SQLite History keeps the original exact source prefix after append"
    );
    drop(append_successor_query);
    drop(append_oracle_query);
    drop(append_oracle_cache);
    let _guard = crate::storage::postgres::native_ingest_tests::Guard::new();
    let (pool, factory) = fixture(|limits| limits.max_connections = 1);
    let control = PgCancellation::default();
    let config = options
        .source
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    config.create_native_store().unwrap();
    let mut wrong = config.clone();
    wrong.namespace = ns("different-host224");
    assert_eq!(
        options.source.clone().with_store(wrong).unwrap_err(),
        StorageError::NamespaceMismatch
    );
    let mut wrong = config.clone();
    wrong.expected_stream_epoch = Some("b".repeat(64));
    assert_eq!(
        options.source.clone().with_store(wrong).unwrap_err(),
        StorageError::ScopeChanged
    );
    let mut options = options;
    options.source = options.source.with_store(config.clone()).unwrap();
    assert_eq!(
        config.create_native_store().err(),
        Some(StorageError::Conflict)
    );
    let mut pg = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    for (at, input) in inputs.iter().enumerate() {
        pg.ingest(
            &format!("{:064x}", at + 1),
            &input.full_path(&options),
            input.root.as_str(),
        )
        .unwrap();
    }
    let prepared =
        crate::model::prepare_sources(&options, &mut pg, &inputs, 1790812802000).unwrap();
    let parent_row = prepared
        .rows
        .iter()
        .find(|row| row.native_ids.iter().any(|id| id == "one"))
        .unwrap();
    let parent_path = claude_parent.to_str().unwrap();
    let origin = parent_row.query.as_ref().unwrap()["selected"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| {
            edge["query"]["from"] == "one"
                && edge["query"]["to"] == "related154"
                && edge["query_content"]["origin"]["kind"] == "prompt"
                && edge["query_content"]["origin"]["path"] == parent_path
        })
        .map(|edge| &edge["query_content"]["origin"])
        .expect("initial user-turn origin keeps its exact native recipe");
    assert_eq!(origin["offset"], 0);
    assert_eq!(origin["block"], 0);
    let origin_source: crate::model::summary::CatalogSource =
        serde_json::from_value(origin["source"].clone()).unwrap();
    assert_eq!(&origin_source.path, &claude_parent);
    assert!(parent_row.sources.contains(&origin_source));
    assert_eq!(prepared.rows.len(), oracle.rows.len());
    assert_eq!(producer_rows(&prepared.rows), producer_rows(&oracle.rows));
    let copied = prepared
        .rows
        .iter()
        .find(|row| row.native_ids.iter().any(|id| id == "copy203-resumed"))
        .unwrap();
    assert!(copied.native_ids.iter().any(|id| id == "copy203-original"));
    assert_eq!(copied.sources.len(), 2);
    assert!(
        copied.query.as_ref().unwrap()["rows"]
            .as_object()
            .unwrap()
            .values()
            .any(|row| row["summary"]["kind"] == "stub"),
        "missing external recipient description"
    );
    assert!(
        serde_json::to_string(&prepared.rows)
            .unwrap()
            .contains("sanitized name175")
    );
    assert_eq!(prepared.transcripts.len(), oracle.projections.len());
    for (actual, expected) in prepared.transcripts.iter().zip(&oracle.projections) {
        assert_eq!(
            serde_json::to_value(&actual.slots).unwrap(),
            serde_json::to_value(&expected.slots).unwrap()
        );
    }
    let binding = pg.query_runtime_binding().unwrap().unwrap();
    let observations = crate::native_evidence::RuntimeObservations::new(&options);
    for row in &prepared.rows {
        if let Some(query) = &row.query {
            let cwds = pg.runtime_source_cwds(&row.sources).unwrap();
            let runtime = binding
                .witness(&observations.scope(row, &cwds).unwrap())
                .unwrap();
            assert_eq!(query["runtime"].as_str(), Some(runtime.as_str()));
        }
    }
    generic_runtime_publication_preflight(&pool, &namespace, &control, &prepared);
    generic_runtime_publication_facades(&pool, &control, &options, &inputs, &expected_bodies);
    // Drive the SAME ordinary queued producer against durable PG staging.
    let paths = inputs
        .iter()
        .map(|input| input.full_path(&options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let stage_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    assert!(
        pg.begin_semantic_stage(
            &(0..65).map(|i| format!("oversize-{i}")).collect::<Vec<_>>(),
            &stage_runtime
        )
        .is_err()
    );
    assert_eq!(
        pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(),
        crate::events::Outcome::Written
    );
    let mut stages = Vec::new();
    let (published, semantic_batch, semantic_model) = loop {
        let batch = pg.semantic_batch(64).unwrap().unwrap();
        stages.push(batch.phase);
        // Reopen mid-stage: resume must preserve the exact phase and epoch.
        if batch.phase == crate::model::relation_stage::Phase::Identity {
            drop(pg);
            pg = EventCache::open_scoped(&options.cache, &options.source).unwrap();
            assert_eq!(
                pg.begin_semantic_stage(&paths, &stage_runtime).unwrap(),
                crate::events::Outcome::Written
            );
            let resumed = pg.semantic_batch(64).unwrap().unwrap();
            assert_eq!(resumed.epoch, batch.epoch);
            assert_eq!(resumed.phase, batch.phase);
        }
        let model = pg
            .prepare_semantic_model_batch(
                &options,
                &stage_inputs(&options, &batch),
                &batch,
                1790812802000,
            )
            .unwrap();
        let current_binding = pg.query_runtime_binding().unwrap().unwrap();
        assert_eq!(current_binding.namespace, binding.namespace);
        assert_eq!(current_binding.admission, binding.admission);
        assert_eq!(current_binding.stream_epoch, binding.stream_epoch);
        let current_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
        assert_eq!(current_runtime, stage_runtime, "staged runtime changed");
        let observations = crate::native_evidence::RuntimeObservations::new(&options);
        for row in &model.prepared().rows {
            let cwds = pg.runtime_source_cwds(&row.sources).unwrap();
            let expected_runtime = current_binding
                .witness(&observations.scope(row, &cwds).unwrap())
                .unwrap();
            assert!(
                row.query.as_ref().is_some_and(
                    |query| query["runtime"].as_str() == Some(expected_runtime.as_str())
                ),
                "staged row runtime differs: phase={:?}",
                batch.phase
            );
        }
        if batch.phase == crate::model::relation_stage::Phase::Relations {
            use crate::model::relation_stage::Selector;
            let selected = pg
                .semantic_select(
                    batch.epoch,
                    &Selector::ClaudeCall {
                        id: "claude-related-spawn".into(),
                    },
                )
                .unwrap();
            assert_eq!(selected.count, 1);
            assert!(
                selected
                    .endpoint
                    .unwrap()
                    .row
                    .native_ids
                    .iter()
                    .any(|id| id == "one")
            );
            assert!(selected.location.is_some());
            let received = pg
                .semantic_select(
                    batch.epoch,
                    &Selector::ClaudeReceipt {
                        id: "receipt203-message".into(),
                    },
                )
                .unwrap();
            assert_eq!(received.count, 1);
            // Native locations retain the physical SourceFile path, just as
            // Location::local requires when checking the consumed witness.
            assert_eq!(received.location.unwrap().source.path, claude_agent);
            assert_eq!(
                pg.semantic_select(
                    batch.epoch,
                    &Selector::ClaudeReceipt {
                        id: "absent-receipt".into()
                    }
                )
                .unwrap()
                .count,
                0
            );
            assert_eq!(
                pg.semantic_select(
                    batch.epoch,
                    &Selector::ClaudeName {
                        name: "absent-native-name".into()
                    }
                )
                .unwrap()
                .count,
                0
            );
        }
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let saved = model.prepared();
            let captured = saved
                .presentation_observations()
                .expect("authentic publication must carry presentation observations");
            let expected_global = current_binding.witness(&batch.runtime).unwrap();
            assert_eq!(
                captured.runtime.as_deref(),
                Some(expected_global.as_str()),
                "authentic publication capture must bind the stage's global runtime"
            );
            for row in &saved.rows {
                let cwds = pg
                    .runtime_source_cwds(&row.sources)
                    .unwrap_or_else(|error| {
                        panic!(
                            "authentic capture CWD lookup failed: key={} error={error}",
                            row.key
                        )
                    });
                let observed = captured
                    .runtime_observation(row, &cwds)
                    .unwrap_or_else(|error| {
                        panic!(
                            "authentic capture authority failed: key={} error={error}",
                            row.key
                        )
                    });
                assert_eq!(
                    row.query
                        .as_ref()
                        .and_then(|query| query["runtime"].as_str()),
                    Some(observed.as_str()),
                    "authentic Query must match capture authority: key={}",
                    row.key
                );
                let saved_runtime = captured.runtime_for(row);
                if saved_runtime.as_deref() != Some(observed.as_str()) {
                    let source = crate::retention::source_generation(&row.sources).unwrap();
                    for endpoint in batch.context.endpoints.values().filter(|endpoint| {
                        endpoint.row.key == row.key
                            && crate::retention::source_generation(&endpoint.row.sources)
                                .ok()
                                .as_ref()
                                == Some(&source)
                    }) {
                        let endpoint_runtime = pg
                            .runtime_source_cwds(&endpoint.row.sources)
                            .and_then(|endpoint_cwds| {
                                let dependencies_differ =
                                    crate::native_evidence::runtime::dependencies(row, &cwds)
                                        != crate::native_evidence::runtime::dependencies(
                                            &endpoint.row,
                                            &endpoint_cwds,
                                        );
                                let runtime =
                                    captured.runtime_observation(&endpoint.row, &endpoint_cwds)?;
                                Ok((runtime, dependencies_differ))
                            });
                        eprintln!(
                            "authentic capture overlap: key={} source_generation={source} saved={saved_runtime:?} prepared={observed} endpoint_scope_and_dependencies_differ={endpoint_runtime:?}",
                            row.key
                        );
                    }
                }
                assert_eq!(
                    saved_runtime.as_deref(),
                    Some(observed.as_str()),
                    "authentic capture must save the exact row/source scope: key={} sources={}",
                    row.key,
                    row.sources.len()
                );
            }
            let (row, endpoint) = saved
                .rows
                .iter()
                .find_map(|row| {
                    batch.context.endpoints.values().find_map(|endpoint| {
                        (endpoint.row.key == row.key
                            && endpoint.row.query.is_none()
                            && crate::retention::source_generation(&endpoint.row.sources).unwrap()
                                == crate::retention::source_generation(&row.sources).unwrap()
                            && crate::native_evidence::runtime::dependencies(
                                row,
                                &Default::default(),
                            ) != crate::native_evidence::runtime::dependencies(
                                &endpoint.row,
                                &Default::default(),
                            ))
                        .then_some((row, &endpoint.row))
                    })
                })
                .expect(
                    "native fixture must contain a queryless endpoint with different dependencies",
                );
            let cwds = pg.runtime_source_cwds(&row.sources).unwrap();
            let row_runtime = captured.runtime_observation(row, &cwds).unwrap();
            let endpoint_runtime = captured.runtime_observation(endpoint, &cwds).unwrap();
            assert_ne!(row_runtime, endpoint_runtime);
            let replayed: crate::storage::selected::PresentationObservations =
                serde_json::from_value(serde_json::to_value(captured).unwrap()).unwrap();
            for observations in [captured, &replayed] {
                assert_eq!(observations.runtime_for(row), Some(row_runtime.clone()));
                assert_eq!(
                    observations.runtime_for(endpoint),
                    Some(endpoint_runtime.clone())
                );
            }
            let mut wrong_native = row.clone();
            wrong_native
                .native_ids
                .push("uncaptured native dependency".into());
            assert_eq!(replayed.runtime_for(&wrong_native), None);
            let mut wrong_owner = row.clone();
            wrong_owner.key = "uncaptured owner with identical dependencies and sources".into();
            assert_eq!(replayed.runtime_for(&wrong_owner), None);
            let mut wrong_query = row.clone();
            wrong_query.query.as_mut().unwrap()["rows"]["uncaptured query dependency"] =
                serde_json::json!({});
            assert_eq!(replayed.runtime_for(&wrong_query), None);
            let mut wrong_generation = row.clone();
            wrong_generation.sources[0].prefix_sha256[0] ^= 1;
            assert_eq!(replayed.runtime_for(&wrong_generation), None);
            let mut different_saved_runtime = row.clone();
            different_saved_runtime.query.as_mut().unwrap()["runtime"] =
                serde_json::json!("not dependency identity");
            assert_eq!(
                replayed.runtime_for(&different_saved_runtime),
                Some(row_runtime.clone())
            );
            let mut conflicting = captured.clone();
            assert!(
                conflicting
                    .capture_runtime(row, &cwds, endpoint_runtime.clone())
                    .is_err()
            );
            let mut different_cwds = cwds.clone();
            different_cwds.insert("different source repository".into());
            assert!(
                conflicting
                    .capture_runtime(row, &different_cwds, row_runtime.clone())
                    .is_err()
            );
            let long_cwds = std::collections::BTreeSet::from(["x".repeat(1024 * 1024 + 1)]);
            let long_runtime = captured.runtime_observation(row, &long_cwds).unwrap();
            let mut long_capture = crate::storage::selected::PresentationObservations::default();
            long_capture.runtime.clone_from(&captured.runtime);
            long_capture
                .capture_runtime(row, &long_cwds, long_runtime.clone())
                .unwrap();
            assert_eq!(long_capture.runtime_for(row), Some(long_runtime.clone()));
            let encoded = serde_json::to_vec(&long_capture).unwrap();
            assert!(encoded.len() < 4096, "long CWDs must not enter the capture");
            let long_replayed: crate::storage::selected::PresentationObservations =
                serde_json::from_slice(&encoded).unwrap();
            assert_eq!(long_replayed.runtime_for(row), Some(long_runtime));
            let mut long_prepared = copy_prepared(saved);
            long_prepared.relationships.presentation = Some(long_capture);
            assert!(long_prepared.staged_model_witness().unwrap().len() < 1024 * 1024);
            assert_eq!(conflicting.runtime_for(row), Some(row_runtime));
            assert_eq!(conflicting.runtime_for(endpoint), Some(endpoint_runtime));
            let scopes = saved
                .rows
                .iter()
                .map(|row| row.query.as_ref().unwrap()["runtime"].as_str().unwrap())
                .collect::<std::collections::BTreeSet<_>>();
            assert!(
                scopes.len() > 1,
                "publication must exercise differing row scopes"
            );
            for case in [
                "tampered_query",
                "missing_query",
                "missing_capture",
                "tampered_capture",
                "wrong_dependencies",
                "wrong_generation",
                "cross_owner_capture",
                "global_drift",
            ] {
                let mut altered = copy_prepared(saved);
                let key = altered.rows[0].key.clone();
                match case {
                    "tampered_query" => {
                        altered.rows[0].query.as_mut().unwrap()["runtime"] =
                            serde_json::json!("tampered scoped observation");
                    }
                    "missing_query" => {
                        altered.rows[0]
                            .query
                            .as_mut()
                            .unwrap()
                            .as_object_mut()
                            .unwrap()
                            .remove("runtime");
                    }
                    "missing_capture" => {
                        let mut capture = serde_json::to_value(
                            altered.relationships.presentation.as_ref().unwrap(),
                        )
                        .unwrap();
                        capture["runtime_rows"]
                            .as_object_mut()
                            .unwrap()
                            .remove(&key);
                        altered.relationships.presentation =
                            Some(serde_json::from_value(capture).unwrap());
                    }
                    "cross_owner_capture" => {
                        let other_key = altered
                            .rows
                            .iter()
                            .find(|row| row.key != key)
                            .unwrap()
                            .key
                            .clone();
                        let mut capture = serde_json::to_value(
                            altered.relationships.presentation.as_ref().unwrap(),
                        )
                        .unwrap();
                        let rows = capture["runtime_rows"].as_object_mut().unwrap();
                        let first = rows.remove(&key).unwrap();
                        let other = rows.remove(&other_key).unwrap();
                        rows.insert(key.clone(), other);
                        rows.insert(other_key, first);
                        altered.relationships.presentation =
                            Some(serde_json::from_value(capture).unwrap());
                    }
                    "tampered_capture" | "wrong_dependencies" | "wrong_generation" => {
                        let mut capture = serde_json::to_value(
                            altered.relationships.presentation.as_ref().unwrap(),
                        )
                        .unwrap();
                        let cohorts = capture["runtime_rows"][&key].as_object_mut().unwrap();
                        let source =
                            crate::retention::source_generation(&altered.rows[0].sources).unwrap();
                        if case == "wrong_generation" {
                            let scopes = cohorts.remove(&source).unwrap();
                            cohorts.insert("different source generation".into(), scopes);
                        } else {
                            let scopes = cohorts.get_mut(&source).unwrap().as_object_mut().unwrap();
                            let cwds = pg.runtime_source_cwds(&altered.rows[0].sources).unwrap();
                            let cwd_fingerprint =
                                crate::native_evidence::runtime::cwd_fingerprint(&cwds);
                            let dependency = crate::native_evidence::runtime::dependency_identity(
                                &altered.rows[0],
                                &cwd_fingerprint,
                            )
                            .unwrap();
                            if case == "wrong_dependencies" {
                                let runtime = scopes.remove(&dependency).unwrap();
                                scopes.insert("different dependency identity".into(), runtime);
                            } else {
                                scopes.insert(
                                    dependency,
                                    serde_json::json!("tampered captured scope"),
                                );
                            }
                        }
                        altered.relationships.presentation =
                            Some(serde_json::from_value(capture).unwrap());
                    }
                    "global_drift" => {
                        altered.relationships.presentation.as_mut().unwrap().runtime =
                            Some("different global observation".into());
                    }
                    _ => unreachable!(),
                }
                // Record this exact altered witness first, so rejection cannot
                // be explained by the independent staged-model equality gate.
                assert_eq!(
                    pg.record_semantic_batch(&batch, &altered).unwrap(),
                    Outcome::Written,
                    "{case} witness records under the same stage and claims"
                );
                assert_eq!(
                    pg.publish_semantic_catalog(&batch, &altered)
                        .unwrap_err()
                        .to_string(),
                    StorageError::ScopeChanged.to_string(),
                    "{case} must fail runtime qualification"
                );
                assert_eq!(pg.session_catalog_generation(), batch.base_generation);
            }
            assert_eq!(
                pg.record_semantic_batch(&batch, saved).unwrap(),
                Outcome::Written
            );
            let mut mismatched = batch.clone();
            mismatched
                .context
                .companions
                .push("altered full context".into());
            assert!(
                pg.publish_semantic_catalog(&mismatched, model.prepared())
                    .is_err()
            );
            let ack = pg
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            assert_eq!(
                pg.publish_semantic_catalog(&batch, model.prepared())
                    .unwrap(),
                ack
            );
            assert!(
                pg.publish_semantic_catalog(&mismatched, model.prepared())
                    .is_err(),
                "completed replay still binds the full batch context"
            );
            break (ack, batch, model);
        }
    };
    assert_eq!(stages.len(), 4);
    assert_eq!(
        producer_rows(&semantic_model.prepared().rows),
        producer_rows(&oracle.rows)
    );
    assert!(
        pg.note_catalog_audit(published.1.as_deref(), 1790812802000)
            .unwrap(),
        "committed publication closed its active stage before the finish ACK"
    );
    assert_eq!(
        pg.finish_semantic_stage(semantic_batch.epoch).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(published.0, crate::events::Outcome::Written);
    let generation = published.1.unwrap();
    let pid_owner_key = prepared
        .rows
        .iter()
        .find(|row| row.harness == "claude" && row.native_ids.iter().any(|id| id == "one"))
        .unwrap()
        .key
        .clone();
    let (pid_original_source, pid_original_proof) = super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            let original = snapshot
                .catalog(&pid_owner_key, false)?
                .ok_or(StorageError::ScopeChanged)?;
            let source = original
                .sources
                .iter()
                .find(|source| source.native_id == "one")
                .ok_or(StorageError::ScopeChanged)?
                .clone();
            let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
            let proof = native_proof_tuple(&mut tx, &generation, &source)?;
            Ok((source, proof))
        },
    )
    .unwrap();
    assert!(!pg.note_catalog_audit(Some("stale"), 1790812802000).unwrap());
    assert!(
        pg.note_catalog_audit(Some(&generation), 1790812802000)
            .unwrap()
    );
    let component = pg
        .catalog_component_paths(
            &[inputs
                .iter()
                .find(|input| input.full_path(&options) == codex)
                .unwrap()
                .clone()],
            &[],
            &[("codex".into(), codex_id.into())],
            &options,
        )
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|input| input.full_path(&options))
        .collect::<Vec<_>>();
    assert!(component.contains(&codex));
    assert!(component.contains(&child));
    // A lost response retries the identical full request, even though its
    // expected head and acknowledged claims have advanced.
    assert_eq!(
        pg.publish_semantic_catalog(&semantic_batch, semantic_model.prepared())
            .unwrap()
            .1
            .as_deref(),
        Some(generation.as_str())
    );
    let saved = semantic_model.prepared();
    let mut altered = copy_prepared(saved);
    let claim = &mut altered.claims.first_mut().unwrap().1;
    *claim = Some(claim.unwrap_or(0) + 1);
    assert_eq!(
        altered.staged_model_witness().unwrap(),
        saved.staged_model_witness().unwrap(),
        "claims are independent of the recorded model witness"
    );
    assert!(
        pg.publish_semantic_catalog(&semantic_batch, &altered)
            .is_err(),
        "completed replay rejects a changed claim revision"
    );
    altered.claims.clone_from(&saved.claims);
    altered.base_generation = Some("altered replay base".into());
    assert!(
        pg.publish_semantic_catalog(&semantic_batch, &altered)
            .is_err(),
        "completed replay binds the prepared base generation"
    );
    let mut altered_batch = semantic_batch.clone();
    altered_batch.base_generation = Some("altered replay base".into());
    assert!(
        pg.publish_semantic_catalog(&altered_batch, saved).is_err(),
        "completed replay binds the batch base generation"
    );
    assert_eq!(
        pg.publish_semantic_catalog(&semantic_batch, saved)
            .unwrap()
            .1
            .as_deref(),
        Some(generation.as_str()),
        "refused replays preserve the original acknowledgement"
    );
    let mut competing = prepared.rows.clone();
    competing[0].name.push_str(" competing publisher");
    assert!(
        pg.publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &competing,
                transcripts: Some(&prepared.transcripts),
                observations: None,
            },
            prepared.base_generation.as_deref()
        )
        .is_err()
    );
    let page = config.catalog_page(&options, "limit=20").unwrap();
    assert_eq!(page.status, 200);
    let page: serde_json::Value = serde_json::from_slice(&page.body).unwrap();
    assert!(page.to_string().contains(&prepared.rows[0].key));
    let exact_catalog_item = |query: &str| {
        let request = crate::catalog::Request::parse(query).unwrap();
        let reply = super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            &generation,
            |snapshot| {
                crate::catalog::read_page_snapshot(&options, recent_machine, &request, snapshot)
                    .map_err(|_| StorageError::ScopeChanged)
            },
        )
        .unwrap();
        assert_eq!(reply.status, 200, "{query}");
        let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        page["items"][0].clone()
    };
    let current_parent_item = exact_catalog_item("sid=one&limit=50");
    assert_eq!(
        current_parent_item["details"]["live_observation"], true,
        "same-source Current details preserve the PG live observation"
    );
    let history_parent_item = exact_catalog_item("sid=one&scope=retained_history&limit=50");
    assert_eq!(
        history_parent_item["details"]["live_observation"], false,
        "retained reads cannot claim a live observation for a Current PG row"
    );
    // P3: retained history over the admitted PG generation equals the SQLite
    // oracle page: exact ordered keys and full producer rows, then a keyset
    // second page continues without overlap.
    let (history_rows, history_page, history_next) =
        super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            &generation,
            |snapshot| {
                let request =
                    crate::catalog::Request::parse("scope=retained_history&limit=50").unwrap();
                let rows = snapshot
                    .rows(&request)
                    .map_err(|_| StorageError::ScopeChanged)?;
                let page = crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &request,
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)?;
                let first = crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &crate::catalog::Request::parse("scope=retained_history&limit=1").unwrap(),
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)?;
                let rows = rows
                    .into_iter()
                    .map(|selected| selected.row)
                    .collect::<Vec<_>>();
                Ok((rows, page, first))
            },
        )
        .unwrap();
    assert_eq!(page_keys(history_page), oracle_history);
    assert_eq!(producer_rows(&history_rows), producer_rows(&ordered));
    assert!(
        history_rows
            .iter()
            .all(|row| row.lifecycle == crate::model::summary::CatalogLifecycle::Current)
    );
    let first: serde_json::Value = serde_json::from_slice(&history_next.body).unwrap();
    assert_eq!(
        first["items"][0]["key"].as_str(),
        oracle_history.first().map(String::as_str)
    );
    if oracle_history.len() > 1 {
        let cursor = first["next_cursor"]
            .as_str()
            .expect("retained history keyset cursor");
        let second = super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            &generation,
            |snapshot| {
                crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &crate::catalog::Request::parse(&format!(
                        "scope=retained_history&limit=50&cursor={}",
                        crate::viewer::percent_encode(cursor)
                    ))
                    .unwrap(),
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)
            },
        )
        .unwrap();
        assert_eq!(page_keys(second), oracle_history[1..].to_vec());
    }
    let query_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
    };
    let long_recipe = prepared
        .transcripts
        .iter()
        .find(|row| row.key == "related191")
        .unwrap();
    assert_eq!(long_recipe.slots.len(), 152);
    assert!(
        crate::storage::query::QueryCatalogStore::has_search_slots(
            &query_store,
            &namespace,
            &long_recipe.key,
            None,
        )
        .unwrap()
    );
    assert!(
        !crate::storage::query::QueryCatalogStore::has_search_slots(
            &query_store,
            &namespace,
            &long_recipe.key,
            Some(i64::MAX),
        )
        .unwrap()
    );
    let mut query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(namespace.as_str().into(), options.clone())]),
    ));
    let query_list = query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812802000,
            &query_store,
        )
        .unwrap();
    assert_eq!(query_list["total"].as_u64(), Some(all_total));
    let proof_owner = &prepared.rows[0];
    let proof_source = &proof_owner.sources[0];
    let proof_relative =
        crate::catalog::source_relative(&options, &proof_owner.harness, proof_source).unwrap();
    let proof = crate::session_source_proof(&options, &proof_owner.harness, &proof_relative)
        .unwrap()
        .unwrap();
    assert_eq!(proof.source.offset, proof_source.offset);
    assert_eq!(proof.source.prefix_sha256, proof_source.prefix_sha256);
    assert_eq!(proof.source.tail_sha256, proof_source.tail_sha256);
    assert_eq!(
        query
            .call_at(
                "read_transcript",
                &serde_json::json!({"id":prepared.rows[0].key}),
                1790812802000
            )
            .unwrap_err()
            .code,
        "unavailable",
        "ordinary PG Query without a host reader must refuse body authority"
    );
    // Recent window (options.all=false): PG cohort equals the SQLite oracle.
    for since in recent_windows {
        let mut recent_options = options.clone();
        recent_options.all = false;
        recent_options.since = since;
        let recent_store = super::super::snapshot::query::QueryCatalog {
            captured: false,
            pool: pool.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: generation.clone(),
            options: recent_options.clone(),
        };
        let recent = crate::Query::from_sources(std::sync::Arc::new(
            crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
        ))
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            recent_now,
            &recent_store,
        )
        .unwrap();
        let expected = &oracle_recent[&since];
        let ids = |list: &serde_json::Value| {
            list["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            recent["total"], expected["total"],
            "recent window {since:?}"
        );
        assert_eq!(ids(&recent), ids(expected), "recent window ids {since:?}");
        assert_eq!(
            recent["sessions"], expected["sessions"],
            "recent window rows {since:?}"
        );
    }
    super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            for row in &prepared.rows {
                let observation = snapshot
                    .transcript_observation(&row.key, &generation, false)
                    .map_err(|_| StorageError::Unavailable)?
                    .unwrap();
                assert!(observation["total"].as_u64().is_some());
                let details = snapshot
                    .selected_details(
                        &row.key,
                        &row.sources,
                        &mut |_, _owner| serde_json::json!({"observation":"unavailable"}),
                    )
                    .map_err(|_| StorageError::Unavailable)?;
                assert!(details.is_some());
            }
            Ok(())
        },
    )
    .unwrap();
    let workspace = crate::session_catalog_union(
        &[crate::OverviewSource {
            key: "machine-independent-of-source-uuid".into(),
            label: "PG workspace".into(),
            options: options.clone(),
            up: true,
        }],
        "pg-businessflow",
        "limit=20",
    );
    assert_eq!(
        workspace.status, 200,
        "ordinary production workspace Catalog"
    );
    let workspace: serde_json::Value = serde_json::from_slice(&workspace.body).unwrap();
    assert!(
        workspace["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["machine"]["key"] == "machine-independent-of-source-uuid")
    );
    assert_eq!(
        workspace["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect::<Vec<_>>()
    );
    let retained_host = body_host(&options, &prepared.rows);
    // The semantic publisher receipt is the real producer output. Corrupt
    // each persisted receipt boundary, call the admitted selected reader, and
    // restore the receipt before testing the next mutation.
    let (operation_id, payload) = pool
        .transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let rows = tx.driver.query(
                "SELECT operation_id,payload FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 2",
                &[&namespace.as_str().as_bytes(), &generation.as_bytes()],
            ).map_err(sqlstate::error)?;
            assert_eq!(rows.len(), 1, "semantic publisher receipt must be unique");
            Ok((rows[0].get::<_, Vec<u8>>(0), rows[0].get::<_, Vec<u8>>(1)))
        })
        .unwrap();
    let read_admitted_session = || {
        let mut reader =
            crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(
                vec![(namespace.as_str().into(), options.clone())],
            )));
        reader.call_bound_selected_at(
            "get_session",
            &serde_json::json!({"id":prepared.rows[0].key}),
            1790812802000,
            &query_store,
            &retained_host,
            None,
        )
    };
    assert!(
        read_admitted_session().is_ok(),
        "the real semantic receipt admits its published selected row"
    );
    let receipt_mutation = |column: &str,
                            lookup_generation: &[u8],
                            changed: &[u8],
                            restore_generation: &[u8],
                            original: &[u8],
                            label: &str| {
        let sql = match column {
            "operation_id" => {
                "UPDATE semon_pg_native_catalog_operations SET operation_id=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2"
            }
            "payload" => {
                "UPDATE semon_pg_native_catalog_operations SET payload=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2"
            }
            "generation" => {
                "UPDATE semon_pg_native_catalog_operations SET generation=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2"
            }
            _ => unreachable!(),
        };
        assert_eq!(
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                tx.driver
                    .execute(
                        sql,
                        &[&namespace.as_str().as_bytes(), &lookup_generation, &changed],
                    )
                    .map_err(sqlstate::error)
            })
            .unwrap(),
            1
        );
        let refused = read_admitted_session();
        assert_eq!(
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                tx.driver
                    .execute(
                        sql,
                        &[
                            &namespace.as_str().as_bytes(),
                            &restore_generation,
                            &original,
                        ],
                    )
                    .map_err(sqlstate::error)
            })
            .unwrap(),
            1,
            "restore the real semantic receipt after {label}"
        );
        assert!(
            refused.is_err(),
            "admitted selected reader must refuse {label}"
        );
    };
    let mut changed_operation_id = operation_id.clone();
    changed_operation_id[0] ^= 1;
    receipt_mutation(
        "operation_id",
        generation.as_bytes(),
        &changed_operation_id,
        generation.as_bytes(),
        &operation_id,
        "a changed operation id",
    );
    let mut changed_payload = payload.clone();
    changed_payload[0] ^= 1;
    receipt_mutation(
        "payload",
        generation.as_bytes(),
        &changed_payload,
        generation.as_bytes(),
        &payload,
        "a changed receipt payload",
    );
    let mut changed_semantic_payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(changed_semantic_payload.as_array().unwrap().len(), 9);
    assert!(changed_semantic_payload[8].is_object());
    changed_semantic_payload[4]["runtime"] =
        serde_json::json!("changed but well-formed captured global witness");
    let changed_semantic_payload = serde_json::to_vec(&changed_semantic_payload).unwrap();
    let changed_semantic_operation = {
        use sha2::Digest;
        sha2::Sha256::digest(&changed_semantic_payload).to_vec()
    };
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute(
                "UPDATE semon_pg_native_catalog_operations SET operation_id=$3,payload=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2",
                &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &changed_semantic_operation, &changed_semantic_payload],
            ).map_err(sqlstate::error)?, 1);
        Ok(())
    })
    .unwrap();
    let refused_derived_generation = read_admitted_session();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(tx.driver.execute(
            "UPDATE semon_pg_native_catalog_operations SET operation_id=$3,payload=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2",
            &[&namespace.as_str().as_bytes(), &generation.as_bytes(), &operation_id, &payload],
        ).map_err(sqlstate::error)?, 1);
        Ok(())
    })
    .unwrap();
    assert!(
        refused_derived_generation.is_err(),
        "a well-formed rehashed payload still needs its derived generation"
    );
    let mut changed_generation = generation.as_bytes().to_vec();
    changed_generation[0] = if changed_generation[0] == b'0' {
        b'1'
    } else {
        b'0'
    };
    receipt_mutation(
        "generation",
        generation.as_bytes(),
        &changed_generation,
        &changed_generation,
        generation.as_bytes(),
        "a changed receipt generation",
    );
    for since in recent_windows {
        let mut recent_options = options.clone();
        recent_options.all = false;
        recent_options.since = since;
        let store = super::super::snapshot::query::QueryCatalog {
            captured: false,
            pool: pool.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: generation.clone(),
            options: recent_options.clone(),
        };
        let mut query = crate::Query::from_sources(std::sync::Arc::new(
            crate::SessionSources::with_machines(vec![(recent_machine.into(), recent_options)]),
        ));
        for (key, expected) in &oracle_recent_fields[&since] {
            let fields = query.call_bound_selected_at(
                "get_session",
                &serde_json::json!({"id":key}),
                recent_now,
                &store,
                &retained_host,
                None,
            );
            assert_eq!(
                &fields, expected,
                "recent-window selected fields {since:?} {key}"
            );
        }
    }
    assert_eq!(
        crate::native_evidence::query_runtime_generation(&options).unwrap(),
        stage_runtime,
        "the selected fixture's runtime changed after its publication"
    );
    let body_read_pass = std::cell::Cell::new(0usize);
    let read_bodies = |query: &mut crate::Query,
                       store: &super::super::snapshot::query::QueryCatalog,
                       host: &BodyHost| {
        let pass = body_read_pass.get() + 1;
        body_read_pass.set(pass);
        let read_binding = store
            .pool
            .transaction(
                &store.namespace,
                TxnKind::ReadSnapshot,
                &store.control,
                super::super::super::model_cache::binding,
            )
            .unwrap();
        let read_runtime =
            crate::native_evidence::query_runtime_generation(&store.options).unwrap();
        eprintln!(
            "pg_body_read_pass={pass} namespace_same={} admission_same={} epoch_same={} raw_runtime_same={}",
            read_binding.namespace == binding.namespace,
            read_binding.admission == binding.admission,
            read_binding.stream_epoch == binding.stream_epoch,
            read_runtime == stage_runtime
        );
        prepared
            .rows
            .iter()
            .map(|row| {
                let body = query
                    .call_bound_selected_at(
                        "read_transcript",
                        &serde_json::json!({"id":row.key,"limit":100}),
                        1790812802000,
                        store,
                        host,
                        None,
                    )
                    .unwrap_or_else(|error| panic!("PG selected body pass={pass}: {error:?}"));
                let fields = query
                    .call_bound_selected_at(
                        "get_session",
                        &serde_json::json!({"id":row.key}),
                        1790812802000,
                        store,
                        host,
                        None,
                    )
                    .unwrap();
                (
                    row.key.clone(),
                    serde_json::json!({"body":body,"fields":fields}),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(
        read_bodies(&mut query, &query_store, &retained_host),
        expected_bodies
    );
    let exact_key = prepared.rows[0].key.clone();
    let unrelated_runtime = options
        .claude_home
        .join("sessions/unrelated-runtime-observation.json");
    assert!(!unrelated_runtime.exists());
    let global_before = crate::native_evidence::query_runtime_generation(&options).unwrap();
    fs::write(&unrelated_runtime, "{}\n").unwrap();
    let global_after = crate::native_evidence::query_runtime_generation(&options).unwrap();
    let exact_ledger_read = query.call_bound_selected_at(
        "read_transcript",
        &serde_json::json!({"id":exact_key.as_str(),"limit":100}),
        1790812802000,
        &query_store,
        &retained_host,
        None,
    );
    fs::remove_file(&unrelated_runtime).unwrap();
    assert_ne!(global_before, global_after);
    assert_eq!(
        exact_ledger_read.unwrap(),
        expected_bodies[&exact_key]["body"],
        "fresh row scope survives unrelated global runtime drift"
    );
    // Captured content uses its admitted immutable generation and independent
    // host/proof authority. It is not an old live Query runtime context.
    let captured_bodies = |pool: &crate::storage::postgres::PgPool| {
        let mut captured_config = config.clone();
        captured_config.backend = crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        };
        let authorized = retained_host.clone();
        let expected_namespace = namespace.as_str().to_owned();
        let allowed = prepared
            .rows
            .iter()
            .map(|row| row.key.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let host = move |source: &str,
                         key: &str|
              -> std::io::Result<Box<dyn crate::SessionSourceReader>> {
            assert_eq!(source, expected_namespace);
            assert!(allowed.contains(key));
            Ok(Box::new(authorized.clone()))
        };
        prepared
            .rows
            .iter()
            .map(|row| {
                let request = crate::SelectedReadControl::default();
                for source in &row.sources {
                    let reference = crate::SessionSourceRef {
                        root: row.harness.clone(),
                        path: crate::catalog::source_relative(&options, &row.harness, source)
                            .unwrap(),
                        native_id: source.native_id.clone(),
                        offset: source.offset,
                        prefix_sha256: source.prefix_sha256,
                        tail_sha256: source.tail_sha256,
                    };
                    let proof = captured_config.captured_source_proof_at(
                        &options,
                        &generation,
                        &row.key,
                        &reference,
                    );
                    let parked_agent = row.harness == "claude"
                        && (source.path == claude_agent || source.path == claude_agent191);
                    if !source.path.exists() && (row.harness == "copilot" || parked_agent) {
                        // The fixture's host independently retained these
                        // bytes outside the input roots. Source cannot certify
                        // a vanished local carrier.
                        assert_eq!(proof.unwrap_err(), StorageError::ScopeChanged);
                        continue;
                    }
                    let proof = proof.unwrap_or_else(|error| {
                        panic!("captured source proof harness={}: {error:?}", row.harness)
                    });
                    assert_eq!(proof.source, reference);
                    let physical_path = if source.path.exists() {
                        source.path.clone()
                    } else {
                        assert_eq!(row.harness, "codex");
                        options
                            .codex_home
                            .join("archived_sessions")
                            .join(source.path.file_name().unwrap())
                    };
                    let physical = crate::events::Stat::of(&physical_path).unwrap();
                    assert_eq!(
                        (
                            proof.dev,
                            proof.ino,
                            proof.size,
                            proof.modified_ns,
                            proof.changed_ns
                        ),
                        (
                            physical.dev,
                            physical.ino,
                            physical.size,
                            physical.modified_ns,
                            physical.changed_ns
                        )
                    );
                    let mut changed = reference;
                    changed.prefix_sha256[0] ^= 1;
                    assert!(
                        captured_config
                            .captured_source_proof_at(&options, &generation, &row.key, &changed)
                            .is_err()
                    );
                }
                let args = serde_json::json!({"id":row.key,"limit":100});
                let body = captured_config
                    .call_captured_query_at(
                        &options,
                        "external-machine-z",
                        &generation,
                        "read_transcript",
                        &args,
                        1790812802000,
                        &request,
                        &host,
                    )
                    .unwrap();
                let mut fields = captured_config
                    .call_captured_query_at(
                        &options,
                        "external-machine-z",
                        &generation,
                        "get_session",
                        &serde_json::json!({"id":row.key}),
                        1790812802000,
                        &request,
                        &host,
                    )
                    .unwrap();
                assert_eq!(fields["session"]["machine"], "external-machine-z");
                fields["session"]["machine"] =
                    expected_bodies[&row.key]["fields"]["session"]["machine"].clone();
                (
                    row.key.clone(),
                    serde_json::json!({"body":body,"fields":fields}),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(captured_bodies(&pool), expected_bodies);
    for phase in [
        "captured_factory",
        "factory_call",
        "captured_proof",
        "captured_body",
        "captured_final",
    ] {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = cancelled.clone();
        if phase != "factory_call" {
            crate::selected_read::set_hook(phase, 0, move || {
                signal.store(true, std::sync::atomic::Ordering::Release);
            });
        }
        let request = crate::SelectedReadControl::new(
            std::time::Instant::now() + Duration::from_secs(20),
            cancelled.clone(),
        );
        let expected_namespace = namespace.as_str().to_owned();
        let expected_key = prepared.rows[0].key.clone();
        let authorized = retained_host.clone();
        let signal = cancelled.clone();
        let host = move |source: &str,
                         key: &str|
              -> std::io::Result<Box<dyn crate::SessionSourceReader>> {
            assert_eq!(source, expected_namespace);
            assert_eq!(key, expected_key);
            if phase == "factory_call" {
                signal.store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(Box::new(authorized.clone()))
        };
        let error = config
            .call_captured_query_at(
                &options,
                "external-machine-z",
                &generation,
                "read_transcript",
                &serde_json::json!({"id":prepared.rows[0].key,"limit":100}),
                1790812802000,
                &request,
                &host,
            )
            .unwrap_err();
        assert!(
            cancelled.load(std::sync::atomic::Ordering::Acquire),
            "phase not reached: {phase}"
        );
        assert_eq!(error.code, "unavailable", "{phase}");
        assert!(error.message.contains("cancelled"), "{phase}: {error:?}");
    }
    let denied_capture =
        |_: &str, _: &str| -> std::io::Result<Box<dyn crate::SessionSourceReader>> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "capture denied",
            ))
        };
    let request = crate::SelectedReadControl::default();
    assert!(
        config
            .call_captured_query_at(
                &options,
                "external-machine-z",
                &generation,
                "read_transcript",
                &serde_json::json!({"id":prepared.rows[0].key,"limit":100}),
                1790812802000,
                &request,
                &denied_capture
            )
            .is_err()
    );
    let uncalled = |_: &str, _: &str| -> std::io::Result<Box<dyn crate::SessionSourceReader>> {
        panic!("invalid capture tool must refuse before host admission")
    };
    assert!(
        config
            .call_captured_query_at(
                &options,
                "external-machine-z",
                &generation,
                "list_sessions",
                &serde_json::json!({}),
                1790812802000,
                &request,
                &uncalled
            )
            .is_err()
    );
    struct DeniedHost;
    impl crate::SessionSourceReader for DeniedHost {
        fn read_range(
            &self,
            _: &crate::SessionSourceRef,
            _: Option<&str>,
            _: u64,
            _: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            Err(std::io::Error::other("host denied"))
        }
    }
    assert!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":prepared.rows[0].key}),
                1790812802000,
                &query_store,
                &DeniedHost,
                None,
            )
            .is_err(),
        "Catalog metadata must not grant Raw or return blank success"
    );

    // Only the request-owned host can grant fixture bytes. The PG reader adds
    // persisted native proof verification to the ordinary scalar-field parser.
    struct Host {
        source: crate::SessionSourceRef,
        bytes: std::sync::Arc<[u8]>,
    }
    impl crate::SessionSourceReader for Host {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> std::io::Result<crate::SessionSourceRange> {
            if source != &self.source
                || expected.is_some_and(|g| g != "authorized-fixture")
                || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX
            {
                return Err(std::io::Error::other("fixture authority mismatch"));
            }
            let at = usize::try_from(offset).map_err(std::io::Error::other)?;
            let end = self.bytes.len().min(at.saturating_add(max));
            let bytes = self
                .bytes
                .get(at..end)
                .ok_or_else(|| std::io::Error::other("fixture range"))?;
            Ok(crate::SessionSourceRange {
                generation: "authorized-fixture".into(),
                length: self.bytes.len() as u64,
                offset,
                bytes: std::sync::Arc::from(bytes),
                cached: false,
            })
        }
    }
    let (projection, slot) = prepared
        .transcripts
        .iter()
        .find_map(|projection| {
            projection
                .slots
                .iter()
                .find(|slot| {
                    matches!(slot.recipe, crate::slot_projection::Recipe::U)
                        && slot.field.is_some()
                        && slot.file.is_some()
                })
                .map(|slot| (projection, slot))
        })
        .expect("ordinary producer must capture a selected native user field");
    let selected = prepared
        .rows
        .iter()
        .find(|row| row.key == projection.key)
        .unwrap();
    let source = selected
        .sources
        .iter()
        .find(|source| source.path == slot.file.as_ref().unwrap().path)
        .unwrap();
    let input = inputs
        .iter()
        .find(|input| input.full_path(&options) == source.path)
        .unwrap();
    let reference = crate::SessionSourceRef {
        root: input.root.as_str().to_owned(),
        path: input.path.clone(),
        native_id: source.native_id.clone(),
        offset: source.offset,
        prefix_sha256: source.prefix_sha256,
        tail_sha256: source.tail_sha256,
    };
    let host = Host {
        source: reference.clone(),
        bytes: std::sync::Arc::from(fs::read(&source.path).unwrap()),
    };
    let field = slot.field.as_ref().unwrap();
    let expected_field =
        crate::native_field::preview(&host, &reference, field, "prompt", &mut (1024 * 1024))
            .unwrap();
    let read_selected = || {
        super::super::selected_reader::with_reader(
            &pool,
            &namespace,
            &control,
            &generation,
            source,
            &reference,
            &host,
            |reader| {
                crate::native_field::preview(
                    reader,
                    &reference,
                    field,
                    "prompt",
                    &mut (1024 * 1024),
                )
            },
        )
        .unwrap()
    };
    assert_eq!(read_selected(), expected_field);
    drop(pg);
    let mut resumed = EventCache::open_scoped(&options.cache, &options.source).unwrap();
    let after = resumed.query_runtime_binding().unwrap().unwrap();
    assert_eq!(binding.admission, after.admission);
    assert_eq!(binding.stream_epoch, after.stream_epoch);
    assert_eq!(
        resumed.session_catalog_generation().as_deref(),
        Some(generation.as_str())
    );
    let repeated =
        crate::model::prepare_sources(&options, &mut resumed, &inputs, 1790812802000).unwrap();
    assert_eq!(
        serde_json::to_value(&repeated.rows).unwrap(),
        serde_json::to_value(&prepared.rows).unwrap()
    );
    for (input, original) in inputs.iter().zip(&raw) {
        assert_eq!(&fs::read(input.full_path(&options)).unwrap(), original);
    }
    let restarted_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: options.clone(),
    };
    let mut restarted_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_machines(vec![(namespace.as_str().into(), options.clone())]),
    ));
    assert_eq!(
        read_bodies(&mut restarted_query, &restarted_store, &retained_host),
        expected_bodies
    );
    use std::io::Write;
    let path = inputs[0].full_path(&options);
    let append = source_append;
    let raw_store = || {
        crate::storage::postgres::index_store::open(
            pool.clone(),
            namespace.clone(),
            control.clone(),
        )
        .unwrap()
        .0
    };
    assert!(
        raw[0].len() > 2 * 1024 * 1024,
        "trusted append fixture must have a large old prefix"
    );
    let io_before = crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get);
    let hash_before = crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get);
    let writer = crate::SessionSourceWrite::begin(
        &options,
        &inputs[0],
        Some("0123456789abcdef0123456789abcdef"),
        raw[0].len() as u64,
    )
    .unwrap();
    let raw_intent = writer.intent();
    assert!(
        raw_intent.continuation.is_some(),
        "native durable continuation must qualify trusted append"
    );
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        assert!(
            native_keys::read_meta(
                tx,
                format!("raw-write-intent:{}", raw_intent.operation).as_bytes(),
                131072
            )?
            .is_some()
        );
        Ok(())
    })
    .unwrap();
    drop(writer); // Fault: durable SQL intent, no Raw bytes yet.
    assert_eq!(fs::read(&path).unwrap(), raw[0]);
    let mut mismatch = raw_intent.clone();
    mismatch.from += 1;
    assert!(
        crate::SessionSourceWrite::begin_with_store(
            &options,
            &inputs[0],
            None,
            0,
            raw_store(),
            Some(mismatch)
        )
        .is_err()
    );
    let writer = crate::SessionSourceWrite::begin_with_store(
        &options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(raw_intent.clone()),
    )
    .unwrap();
    let mut raw_file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    raw_file.write_all(append.as_bytes()).unwrap();
    raw_file.sync_all().unwrap();
    drop(raw_file);
    drop(writer); // Fault: durable bytes, completion transaction not attempted.
    let written = raw[0].len() as u64 + append.len() as u64;
    assert_eq!(
        resumed
            .ingest(&format!("{:064x}", 158), &path, "claude")
            .err(),
        Some(StorageError::Conflict),
        "unacknowledged Raw bytes cannot enter native SQL"
    );
    let writer = crate::SessionSourceWrite::begin_with_store(
        &options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(raw_intent.clone()),
    )
    .unwrap();
    writer.complete(written).unwrap();
    // Fault: SQL committed but the receiver lost the ACK. The exact request
    // reconciles before head/ledger CAS and reproduces the same completion.
    let writer = crate::SessionSourceWrite::begin_with_store(
        &options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(raw_intent.clone()),
    )
    .unwrap();
    writer.complete(written).unwrap();
    let read_bytes = crate::events::SOURCE_READ_BYTES.with(std::cell::Cell::get) - io_before;
    let hash_bytes = crate::events::SOURCE_HASHED_BYTES.with(std::cell::Cell::get) - hash_before;
    assert!(
        read_bytes < 512 * 1024,
        "trusted SourceWrite read {read_bytes} bytes"
    );
    assert!(
        hash_bytes < 512 * 1024,
        "trusted SourceWrite hashed {hash_bytes} bytes"
    );
    let c = raw_intent.continuation.as_ref().unwrap();
    let mut source = crate::sealed::LogFile::open(&path).unwrap();
    assert_eq!(
        crate::source_chunks::continue_prefix(
            &mut source,
            &c.state,
            c.offset,
            c.prefix,
            written,
            || Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancel"
            ))
        )
        .unwrap_err()
        .kind(),
        std::io::ErrorKind::Interrupted
    );
    eprintln!(
        "sourcewrite160 old_bytes={} delta={} read_bytes={read_bytes} hash_bytes={hash_bytes} history_io_under_sql_lock=0",
        raw[0].len(),
        append.len()
    );
    let mut stale = raw_intent;
    stale.operation.push_str("-stale");
    stale.head = Some("stale-head".into());
    assert!(
        crate::SessionSourceWrite::begin_with_store(
            &options,
            &inputs[0],
            None,
            0,
            raw_store(),
            Some(stale)
        )
        .is_err()
    );
    let receipt = resumed
        .ingest(&format!("{:064x}", 128), &path, "claude")
        .unwrap();
    let continued =
        crate::model::prepare_sources(&options, &mut resumed, &inputs, 1790812803000).unwrap();
    assert_eq!(continued.rows.len(), prepared.rows.len());
    assert_eq!(continued.rows[0].key, prepared.rows[0].key);
    assert_eq!(
        receipt.consumed_bytes,
        raw[0].len() as u64 + append.len() as u64
    );
    assert_eq!(
        continued
            .rows
            .iter()
            .flat_map(|row| &row.sources)
            .find(|source| source.path == path)
            .unwrap()
            .offset,
        receipt.consumed_bytes
    );
    let index_ahead_key = selected.key.clone();
    let original_hostname = fs::read(&hostname).unwrap();
    let global_before = crate::native_evidence::query_runtime_generation(&options).unwrap();
    fs::write(&hostname, "synthetic-advanced-index-runtime-drift\n").unwrap();
    let global_after = crate::native_evidence::query_runtime_generation(&options).unwrap();
    let advanced_with_drift = query.call_bound_selected_at(
        "read_transcript",
        &serde_json::json!({"id":index_ahead_key.as_str(),"limit":100}),
        1790812802000,
        &query_store,
        &retained_host,
        None,
    );
    fs::write(&hostname, &original_hostname).unwrap();
    assert_ne!(global_before, global_after);
    assert!(
        advanced_with_drift.is_err(),
        "an advanced Index cohort needs its captured global witness"
    );
    let index_ahead_read = query
        .call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":index_ahead_key.as_str(),"limit":100}),
            1790812802000,
            &query_store,
            &retained_host,
            None,
        )
        .unwrap();
    assert_eq!(
        index_ahead_read, expected_bodies[&index_ahead_key]["body"],
        "authenticated old Catalog row stays readable while Index is ahead"
    );
    let append_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    resumed
        .begin_semantic_stage(&paths, &append_runtime)
        .unwrap();
    let continued_generation = loop {
        let batch = resumed.semantic_batch(64).unwrap().unwrap();
        let model = resumed
            .prepare_semantic_model_batch(
                &options,
                &stage_inputs(&options, &batch),
                &batch,
                1790812803000,
            )
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let ack = resumed
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            resumed.finish_semantic_stage(batch.epoch).unwrap();
            break ack.1.unwrap();
        }
    };
    let next_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: continued_generation.clone(),
        options: options.clone(),
    };
    let mut next_host = body_host(&options, &continued.rows);
    // A retained composite can select both exact original and successor
    // references for one physical path. Authorize those captured references
    // explicitly; a current-path entry alone cannot authorize an old prefix.
    next_host.0.extend(retained_host.0.iter().cloned());
    let next_bodies = read_bodies(&mut restarted_query, &next_store, &next_host);
    assert!(
        next_bodies[&appended_key]["body"] == append_history_body,
        "PG History-selected body must match SQLite after the same append"
    );
    assert_eq!(
        next_bodies[&appended_key]["body"], expected_bodies[&appended_key]["body"],
        "PG History-selected body must preserve the original exact prefix"
    );
    assert_eq!(
        next_bodies[&appended_key]["fields"], expected_bodies[&appended_key]["fields"],
        "PG History-selected fields must preserve the original exact oracle"
    );
    let successor_related_body = restarted_query
        .call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":related_key.as_str(),"limit":100}),
            1790812803000,
            &next_store,
            &next_host,
            None,
        )
        .expect("PG Current related-session body after append");
    assert_eq!(
        successor_related_body, append_successor_related_body,
        "PG Current related-session body must match the SQLite successor"
    );
    let successor_related_fields = restarted_query
        .call_bound_selected_at(
            "get_session",
            &serde_json::json!({"id":related_key.as_str()}),
            1790812803000,
            &next_store,
            &next_host,
            None,
        )
        .expect("PG Current related-session fields after append");
    assert_eq!(
        successor_related_fields, append_successor_related_fields,
        "PG Current related-session fields must match the SQLite successor"
    );
    let (related_harness, selected_parent_sources) =
        super::super::snapshot::with_admitted_snapshot(
            &pool,
            &namespace,
            &control,
            &continued_generation,
            |snapshot| {
                let row = snapshot
                    .catalog(&related_key, false)?
                    .ok_or(StorageError::ScopeChanged)?;
                let edges =
                    row.query.as_ref().ok_or(StorageError::ScopeChanged)?["selected"]["edges"]
                        .as_array()
                        .ok_or(StorageError::ScopeChanged)?;
                let sources = edges
                    .iter()
                    .flat_map(|edge| {
                        edge["query_content"]
                            .as_object()
                            .into_iter()
                            .flat_map(|recipes| recipes.values())
                    })
                    .filter(|recipe| {
                        recipe["path"].as_str().is_some_and(|path| {
                            std::path::Path::new(path) == appended_path.as_path()
                        })
                    })
                    .map(|recipe| recipe["source"].clone())
                    .collect::<Vec<_>>();
                Ok((row.harness, sources))
            },
        )
        .expect("successor related-session Catalog retains its exact parent edge recipe");
    assert!(
        !selected_parent_sources.is_empty(),
        "selected related-session edge must reference the appended parent source"
    );
    let current_parent_ref = append_current_identity
        .source_refs
        .iter()
        .find(|reference| {
            reference.source.root == "claude" && reference.source.path == append_relative_path
        })
        .expect("SQLite successor identity references the appended parent")
        .source
        .clone();
    let retained_parent_ref = append_history_identity
        .source_refs
        .iter()
        .find(|reference| {
            reference.source.root == "claude" && reference.source.path == append_relative_path
        })
        .expect("SQLite retained identity references the original parent prefix")
        .source
        .clone();
    for selected_source in selected_parent_sources {
        let selected_source: crate::model::summary::CatalogSource =
            serde_json::from_value(selected_source).expect("Catalog edge source is valid");
        let selected_parent_ref = crate::SessionSourceRef {
            root: related_harness.clone(),
            path: crate::catalog::source_relative(&options, &related_harness, &selected_source)
                .expect("selected edge source remains under its root"),
            native_id: selected_source.native_id.clone(),
            offset: selected_source.offset,
            prefix_sha256: selected_source.prefix_sha256,
            tail_sha256: selected_source.tail_sha256,
        };
        assert_eq!(
            selected_parent_ref, current_parent_ref,
            "selected edge recipe must use the parent's Current source ref"
        );
        assert_ne!(
            selected_parent_ref, retained_parent_ref,
            "selected edge recipe must not use the parent's retained History ref"
        );
    }
    let (history_harness, current_harness) = super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &continued_generation,
        |snapshot| {
            let history = snapshot
                .catalog(&appended_key, true)?
                .ok_or(StorageError::ScopeChanged)?;
            let current = snapshot
                .catalog(&appended_key, false)?
                .ok_or(StorageError::ScopeChanged)?;
            Ok((history.harness, current.harness))
        },
    )
    .expect("successor publication keeps History beside the Current owner");
    assert_eq!(
        history_harness, current_harness,
        "History runtime qualification requires its same-harness Current owner"
    );
    // This live PID file adds a native runtime dependency to this Claude owner.
    let pid = 314;
    let pid_stat = options.proc_root.join(pid.to_string()).join("stat");
    let pid_session = options
        .claude_home
        .join("sessions")
        .join(format!("{pid}.json"));
    assert!(!pid_stat.exists());
    assert!(!pid_session.exists());
    fs::create_dir_all(pid_stat.parent().unwrap()).unwrap();
    fs::create_dir_all(pid_session.parent().unwrap()).unwrap();
    let mut proc_fields = vec!["0"; 20];
    proc_fields[19] = "1234";
    fs::write(
        &pid_stat,
        format!("{pid} (claude) {}\n", proc_fields.join(" ")),
    )
    .unwrap();
    fs::write(
        &pid_session,
        serde_json::json!({"pid":pid,"sessionId":appended_key.as_str(),"procStart":1234,"status":"busy"})
            .to_string(),
    )
    .unwrap();
    let history_runtime_drift = restarted_query.call_bound_selected_at(
        "read_transcript",
        &serde_json::json!({"id":appended_key.as_str(),"limit":100}),
        1790812803000,
        &next_store,
        &next_host,
        None,
    );
    fs::remove_file(&pid_session).unwrap();
    fs::remove_file(&pid_stat).unwrap();
    let error = history_runtime_drift
        .expect_err("History-selected read must refuse when its same-harness PID runtime drifts");
    assert!(
        error.code == "unavailable" && error.message == StorageError::ScopeChanged.to_string(),
        "History runtime drift must refuse with ScopeChanged: {error:?}"
    );
    let recovered_history_body = restarted_query
        .call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":appended_key.as_str(),"limit":100}),
            1790812803000,
            &next_store,
            &next_host,
            None,
        )
        .expect("restoring same-harness runtime recovers the retained body");
    assert_eq!(
        recovered_history_body, expected_bodies[&appended_key]["body"],
        "restored runtime recovers the original retained source body"
    );
    let successor_current_body = super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &continued_generation,
        |snapshot| {
            let (row, header, _) = snapshot
                .selected(&appended_key, false, 0, 64)?
                .ok_or(StorageError::ScopeChanged)?;
            if row.lifecycle != crate::model::summary::CatalogLifecycle::Current
                || !row.sources.iter().any(|source| {
                    source.path.as_path() == path.as_path()
                        && source.offset == receipt.consumed_bytes
                })
            {
                return Err(StorageError::ScopeChanged);
            }
            snapshot
                .row_runtime_binding(&row)
                .map_err(|_| StorageError::ScopeChanged)?
                .ok_or(StorageError::ScopeChanged)?;
            let saved = row.query.as_ref().ok_or(StorageError::ScopeChanged)?;
            let runtime = crate::native_evidence::query_runtime_generation(&options)
                .map_err(|_| StorageError::ScopeChanged)?;
            let expected = snapshot
                .runtime_observation(
                    &options,
                    &row,
                    crate::storage::catalog::CatalogObservationOwner::Catalog,
                    &runtime,
                )
                .map_err(|_| StorageError::ScopeChanged)?
                .ok_or(StorageError::ScopeChanged)?;
            let mut retained_display = row.clone();
            retained_display.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
            assert_eq!(
                snapshot
                    .runtime_observation(
                        &options,
                        &retained_display,
                        crate::storage::catalog::CatalogObservationOwner::Catalog,
                        &runtime
                    )
                    .unwrap()
                    .as_deref(),
                Some(expected.as_str())
            );
            assert!(
                snapshot
                    .runtime_observation(
                        &options,
                        &retained_display,
                        crate::storage::catalog::CatalogObservationOwner::History,
                        &runtime
                    )
                    .is_err()
            );

            if !crate::model::summary::query_observation_matches(
                saved,
                &row.sources,
                true,
                Some(expected.as_str()),
            )
            .map_err(|_| StorageError::ScopeChanged)?
            {
                return Err(StorageError::ScopeChanged);
            }
            let total = usize::try_from(header.3).map_err(|_| StorageError::ScopeChanged)?;
            let observation: crate::slot_projection::QueryObservation =
                serde_json::from_value(saved["transcript_observation"].clone())
                    .map_err(|_| StorageError::ScopeChanged)?;
            if !observation.valid(total) {
                return Err(StorageError::ScopeChanged);
            }
            let sources = row
                .sources
                .iter()
                .map(|source| {
                    let relative = crate::catalog::source_relative(&options, &row.harness, source)
                        .ok_or(StorageError::ScopeChanged)?;
                    Ok((
                        source,
                        crate::SessionSourceRef {
                            root: row.harness.clone(),
                            path: relative,
                            native_id: source.native_id.clone(),
                            offset: source.offset,
                            prefix_sha256: source.prefix_sha256,
                            tail_sha256: source.tail_sha256,
                        },
                    ))
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            let records = super::super::selected_reader::SelectedRecords {
                tx: &snapshot.tx,
                generation: &continued_generation,
                host: &next_host,
                sources,
                pins: std::sync::Mutex::new(std::collections::BTreeMap::new()),
                bytes_left: std::sync::Mutex::new(64 * 1024 * 1024),
            };
            let page = super::super::snapshot::query::PgPage {
                snapshot,
                key: &appended_key,
                history: false,
                records: &records,
            };
            let body = crate::query::transcript::render_page(
                &options,
                &appended_key,
                &row,
                &observation,
                total,
                false,
                total,
                100,
                1790812803000,
                &page,
            )
            .map_err(|_| StorageError::ScopeChanged)?;
            records.finish().map_err(|_| StorageError::ScopeChanged)?;
            if crate::native_evidence::query_runtime_generation(&options)
                .map_err(|_| StorageError::ScopeChanged)?
                != runtime
            {
                return Err(StorageError::ScopeChanged);
            }
            Ok(body)
        },
    )
    .expect("successor Current recipe and proof authorize the appended Raw body");
    assert!(
        successor_current_body
            .to_string()
            .contains("sanitized continuation"),
        "successor Current body reads the appended continuation from Raw"
    );
    // The old generation keeps its original Current row receipts. Only rows
    // whose exact source cohort includes the appended file lose current
    // runtime qualification; unrelated source cohorts retain their witnesses.
    let mut changed_source_rows = 0;
    let mut unchanged_source_rows = 0;
    for row in &prepared.rows {
        let args = serde_json::json!({"id":row.key,"limit":100});
        if row
            .sources
            .iter()
            .any(|source| source.path.as_path() == path.as_path())
        {
            changed_source_rows += 1;
            let result = query.call_bound_selected_at(
                "read_transcript",
                &args,
                1790812802000,
                &query_store,
                &retained_host,
                None,
            );
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!(
                    "changed-source Current body unexpectedly succeeded key={}",
                    row.key
                ),
            };
            assert!(
                error.code == "unavailable"
                    && error.message == StorageError::ScopeChanged.to_string(),
                "changed-source Current row must refuse with ScopeChanged key={} code={}",
                row.key,
                error.code
            );
        } else {
            unchanged_source_rows += 1;
            let body = query
                .call_bound_selected_at(
                    "read_transcript",
                    &args,
                    1790812802000,
                    &query_store,
                    &retained_host,
                    None,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "unchanged-source Current body failed key={} code={}",
                        row.key, error.code
                    )
                });
            let fields = query
                .call_bound_selected_at(
                    "get_session",
                    &serde_json::json!({"id":row.key}),
                    1790812802000,
                    &query_store,
                    &retained_host,
                    None,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "unchanged-source Current fields failed key={} code={}",
                        row.key, error.code
                    )
                });
            let actual = serde_json::json!({"body":body,"fields":fields});
            assert!(
                actual == expected_bodies[&row.key],
                "unchanged-source Current body/fields differ from oracle key={}",
                row.key
            );
        }
    }
    assert!(
        changed_source_rows >= 1,
        "append path must match an original Current source cohort"
    );
    assert!(
        unchanged_source_rows >= 1,
        "fixture must retain an unaffected Current source cohort"
    );
    assert_eq!(
        captured_bodies(&pool),
        expected_bodies,
        "the exact captured prefix has a current physical covering witness after append"
    );
    assert_ne!(continued_generation, generation);
    assert_eq!(
        read_selected(),
        expected_field,
        "retained selected proof must survive a newer publication"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        [raw[0].as_slice(), append.as_bytes()].concat()
    );
    assert!(!options.cache.exists());
    let (mut cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let input_path = options
        .claude_home
        .join("projects/p/source.meta.json")
        .to_string_lossy()
        .into_owned();
    let mut facts_options = options.clone();
    facts_options.facts = Some(oracle.root.join("facts-dispatch-only"));
    cache
        .resume_runtime_input(&facts_options, &input_path)
        .unwrap();
    assert!(!options.cache.exists());
    let input_revision = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        super::super::super::source_journal::invalidate(tx,&input_path)?;
        Ok(tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&namespace.as_str().as_bytes(),&input_path.as_bytes()]).map_err(sqlstate::error)?.get::<_,i64>(0))
    }).unwrap();
    assert_eq!(
        cache
            .resolve_catalog_input(&input_path, input_revision + 1, None, None)
            .unwrap(),
        crate::events::Outcome::Conflict
    );
    assert_eq!(
        cache
            .resolve_catalog_input(
                &input_path,
                input_revision,
                Some(("codex", codex_id)),
                Some(codex.to_str().unwrap())
            )
            .unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        cache
            .resolve_catalog_input(
                &input_path,
                input_revision,
                Some(("codex", codex_id)),
                Some(codex.to_str().unwrap())
            )
            .unwrap(),
        crate::events::Outcome::Written,
        "exact lost-ACK replay"
    );
    assert_eq!(
        cache
            .resolve_catalog_input(&input_path, input_revision, Some(("codex", child_id)), None)
            .unwrap(),
        crate::events::Outcome::Conflict,
        "a changed full request cannot replay the ACK"
    );
    let key = "opaque\0🦀".repeat(512);
    cache
        .save_session_descriptions(&[(key.clone(), 1, "inputs".into(), "metadata".into())])
        .unwrap();
    assert_eq!(
        cache
            .session_descriptions(&[crate::events::SessionDescriptionRequest {
                key: key.clone(),
                version: 1,
                inputs: "inputs".into(),
            }])
            .unwrap(),
        vec![Some("metadata".into())]
    );
    assert_eq!(
        cache
            .session_descriptions(&[crate::events::SessionDescriptionRequest {
                key: key.clone(),
                version: 2,
                inputs: "inputs".into(),
            }])
            .unwrap(),
        vec![None]
    );
    assert_eq!(
        cache
            .session_descriptions(&[crate::events::SessionDescriptionRequest {
                key: key.clone(),
                version: 1,
                inputs: "other".into(),
            }])
            .unwrap(),
        vec![None]
    );
    assert!(
        cache
            .save_session_descriptions(&[(
                key.clone(),
                1,
                "inputs".into(),
                "x".repeat(1024 * 1024)
            )])
            .is_err()
    );
    assert_eq!(
        cache
            .session_descriptions(&[crate::events::SessionDescriptionRequest {
                key: key.clone(),
                version: 1,
                inputs: "inputs".into(),
            }])
            .unwrap(),
        vec![Some("metadata".into())]
    );
    // Disappearance withdraws Current membership, without consuming the host's
    // independently retained bytes or any older Catalog/native proof.
    let missing = copilot.to_str().unwrap().to_owned();
    let missing_ledger = cache.ledger(&missing).unwrap().unwrap();
    let retained_copy = oracle.root.join("retained-copilot156.jsonl");
    fs::rename(&copilot, &retained_copy).unwrap();
    let removal = vec![(missing.clone(), Some(missing_ledger.clone()))];
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written
    );
    let withdrawn_generation = cache.session_catalog_generation().unwrap().unwrap();
    assert_ne!(withdrawn_generation, continued_generation);
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written,
        "lost removal ACK"
    );
    assert_eq!(
        cache.session_catalog_generation().unwrap().as_deref(),
        Some(withdrawn_generation.as_str())
    );
    let mut stale = missing_ledger;
    stale.offset += 1;
    assert_eq!(
        cache
            .remove_files(&[(missing.clone(), Some(stale))])
            .unwrap(),
        crate::events::Outcome::Conflict
    );
    let withdrawn_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: withdrawn_generation.clone(),
        options: options.clone(),
    };
    let current = query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812802000,
            &withdrawn_store,
        )
        .unwrap();
    // This is a Query cohort, including virtual peer endpoints, rather than
    // the physical owner count captured before subsequent publication. The
    // independent SQLite withdrawal oracle fixes both its total and exact IDs.
    assert_eq!(
        current["total"].as_u64(),
        withdrawal_oracle["total"].as_u64(),
        "PG ids={:?}; SQLite withdrawal total={} ids={:?}; owner keys={:?}",
        current["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].clone())
            .collect::<Vec<_>>(),
        withdrawal_oracle["total"],
        withdrawal_oracle["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].clone())
            .collect::<Vec<_>>(),
        continued
            .rows
            .iter()
            .map(|row| &row.key)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        current["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| &row["id"])
            .collect::<Vec<_>>(),
        withdrawal_oracle["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| &row["id"])
            .collect::<Vec<_>>(),
        "withdrawal preserves the exact SQLite Query order, including virtual peers"
    );
    let removed_key = continued
        .rows
        .iter()
        .find(|r| r.harness == "copilot")
        .unwrap()
        .key
        .clone();
    assert!(!current.to_string().contains(&removed_key));
    assert!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":removed_key,"limit":100}),
                1790812802000,
                &withdrawn_store,
                &retained_host,
                None,
            )
            .is_err()
    );
    // Later old-generation checks use the immutable captured proof route; the
    // separate live Current refusal is asserted above.
    assert_eq!(captured_bodies(&pool), expected_bodies);
    drop(cache);
    let (mut cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    assert_eq!(
        cache.remove_files(&removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        captured_bodies(&pool),
        expected_bodies,
        "withdrawal preserves authorized old body after reopen"
    );
    assert_eq!(
        fs::read(&retained_copy).unwrap(),
        retained_host
            .0
            .iter()
            .find(|(r, _)| r.root == "copilot")
            .unwrap()
            .1
            .as_ref()
    );
    let archived = options
        .codex_home
        .join(format!("archived_sessions/rollout-{codex_id}.jsonl"));
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    let codex_bytes = fs::read(&codex).unwrap();
    let from_input = crate::Input {
        root: crate::InputRoot::Codex,
        path: codex
            .strip_prefix(&options.codex_home)
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
    };
    let to_input = crate::Input {
        root: crate::InputRoot::Codex,
        path: archived
            .strip_prefix(&options.codex_home)
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
    };
    let old_ledger = cache.ledger(codex.to_str().unwrap()).unwrap().unwrap();
    let relocation = crate::SessionSourceRelocation::begin(
        &options,
        &from_input,
        &to_input,
        codex_id,
        "0123456789abcdef0123456789abcdef",
        old_ledger.offset,
        &format!(
            "{:x}",
            sha2::digest::generic_array::GenericArray::from(old_ledger.prefix)
        ),
    )
    .unwrap();
    fs::copy(&codex, &archived).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&archived)
        .unwrap()
        .sync_all()
        .unwrap();
    relocation.complete().unwrap();
    fs::remove_file(&codex).unwrap();
    let moved_stat = crate::events::Stat::of(&archived).unwrap();
    let mut moved_file = crate::sealed::LogFile::open(&archived).unwrap();
    let mut timing = crate::native_timing::Measurements::new(false);
    assert_eq!(
        cache
            .relocate_codex(
                archived.to_str().unwrap(),
                &moved_stat,
                codex_id,
                &mut moved_file,
                &mut timing
            )
            .unwrap()
            .as_deref(),
        codex.to_str()
    );
    assert_eq!(
        cache
            .relocate_codex(
                archived.to_str().unwrap(),
                &moved_stat,
                codex_id,
                &mut moved_file,
                &mut timing
            )
            .unwrap()
            .as_deref(),
        codex.to_str(),
        "exact relocation ACK replay"
    );
    assert_eq!(fs::read(&archived).unwrap(), codex_bytes);
    assert_eq!(
        captured_bodies(&pool),
        expected_bodies,
        "old authorized body survives native relocation"
    );
    drop(cache);
    let moved_inputs = crate::inputs::inputs(&options)
        .unwrap()
        .into_iter()
        .filter(|i| {
            crate::native_evidence::catalog_input(&i.full_path(&options), &options).is_some()
        })
        .collect::<Vec<_>>();
    let moved_paths = moved_inputs
        .iter()
        .map(|i| i.full_path(&options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut moved_cache =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let moved_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    moved_cache
        .begin_semantic_stage(&moved_paths, &moved_runtime)
        .unwrap();
    let removed = vec![(codex.to_str().unwrap().to_owned(), Some(old_ledger.clone()))];
    let receipt_key = format!("native-relocation-receipt:{}", codex.display());
    let (relocation_request, removal_revision) = pool
        .transaction(&namespace, TxnKind::Write, &control, |tx| {
            let request = native_keys::read_meta(tx, receipt_key.as_bytes(), 16384)?.unwrap();
            let revision: i64 = tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed'", &[&namespace.as_str().as_bytes(), &codex.to_str().unwrap().as_bytes()]).map_err(sqlstate::error)?.get(0);
            assert_eq!(withdrawal::remove(tx, &removed)?, Outcome::Written);
            let after: i64 = tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed'", &[&namespace.as_str().as_bytes(), &codex.to_str().unwrap().as_bytes()]).map_err(sqlstate::error)?.get(0);
            assert_eq!(after, revision, "relocation replay preserves the stage claim");
            Ok((request, revision))
        })
        .unwrap();
    let ack_key = {
        use sha2::Digest;
        format!(
            "native-relocation-ack:{:x}",
            sha2::Sha256::digest(&relocation_request)
        )
    };
    // Each missing binding refuses under the real active semantic stage. The
    // deliberate error rolls the fixture mutation back before the next case.
    for key in [
        receipt_key.clone(),
        ack_key,
        format!("native_catalog_relocation:{}", codex.display()),
        format!("source-retired:{}", codex.display()),
        format!("native_codex_header:{}", codex.display()),
        format!("native_codex_header:{}", archived.display()),
    ] {
        let refused = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            native_keys::delete_meta(tx, key.as_bytes())?;
            assert_eq!(
                withdrawal::remove(tx, &removed)?,
                Outcome::Conflict,
                "missing relocation binding: {key}"
            );
            Err::<(), _>(StorageError::Conflict)
        });
        assert!(matches!(refused, Err(StorageError::Conflict)));
    }
    let refused = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let mut request: serde_json::Value = serde_json::from_slice(&relocation_request).unwrap();
        request[4]["size"] = serde_json::json!(moved_stat.size + 1);
        native_keys::put_meta(
            tx,
            receipt_key.as_bytes(),
            &serde_json::to_vec(&request).unwrap(),
        )?;
        assert_eq!(
            withdrawal::remove(tx, &removed)?,
            Outcome::Conflict,
            "a reconstructed physical Stat has no exact original ACK"
        );
        Err::<(), _>(StorageError::Conflict)
    });
    assert!(matches!(refused, Err(StorageError::Conflict)));
    for path in [&codex, &archived] {
        let refused = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3)", &[&namespace.as_str().as_bytes(), &path.to_str().unwrap().as_bytes(), &b"pending".as_slice()]).map_err(sqlstate::error)?;
            assert_eq!(withdrawal::remove(tx, &removed)?, Outcome::Conflict);
            Err::<(), _>(StorageError::Conflict)
        });
        assert!(matches!(refused, Err(StorageError::Conflict)));
        let refused = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            native_keys::put_meta(
                tx,
                format!("raw-write-inflight:{}", path.display()).as_bytes(),
                b"pending",
            )?;
            assert_eq!(withdrawal::remove(tx, &removed)?, Outcome::Conflict);
            Err::<(), _>(StorageError::Conflict)
        });
        assert!(matches!(refused, Err(StorageError::Conflict)));
        let refused = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            tx.driver.execute("UPDATE semon_pg_native_page_generations SET chunks_header=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&namespace.as_str().as_bytes(), &path.to_str().unwrap().as_bytes(), &b"{}".as_slice()]).map_err(sqlstate::error)?;
            assert!(
                matches!(
                    withdrawal::remove(tx, &removed),
                    Err(StorageError::ScopeChanged)
                ),
                "immutable proof header must qualify the exact ledger"
            );
            Err::<(), _>(StorageError::Conflict)
        });
        assert!(matches!(refused, Err(StorageError::Conflict)));
    }
    let refused = pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let changed = vec![(
            removed[0].0.clone(),
            Some(crate::events::Ledger {
                offset: old_ledger.offset + 1,
                ..old_ledger.clone()
            }),
        )];
        assert_eq!(
            withdrawal::remove(tx, &changed)?,
            Outcome::Conflict,
            "an arbitrary removal ledger cannot bypass the active stage"
        );
        Err::<(), _>(StorageError::Conflict)
    });
    assert!(matches!(refused, Err(StorageError::Conflict)));
    let (moved_generation, moved_rows) = loop {
        let batch = moved_cache.semantic_batch(64).unwrap().unwrap();
        let model = moved_cache
            .prepare_semantic_model_batch(
                &options,
                &stage_inputs(&options, &batch),
                &batch,
                1790812803000,
            )
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            let rows = model.prepared().rows.clone();
            let ack = moved_cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            moved_cache.finish_semantic_stage(batch.epoch).unwrap();
            break (ack.1.unwrap(), rows);
        }
    };
    pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let pending: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND revision=$3)", &[&namespace.as_str().as_bytes(), &codex.to_str().unwrap().as_bytes(), &removal_revision]).map_err(sqlstate::error)?.get(0);
        assert!(
            !pending,
            "qualified successor publication acknowledges the old claim"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(
        moved_rows
            .iter()
            .filter(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
            .count(),
        1
    );
    let moved_owner = moved_rows
        .iter()
        .find(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
        .unwrap();
    assert_eq!(
        moved_owner.key,
        continued
            .rows
            .iter()
            .find(|r| r.harness == "codex" && r.native_ids.iter().any(|id| id == codex_id))
            .unwrap()
            .key
    );
    let moved_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: moved_generation,
        options: options.clone(),
    };
    let mut moved_host = body_host(&options, &moved_rows);
    // Retained History can name both exact continued and original captures.
    moved_host.0.extend(next_host.0.iter().cloned());
    assert_eq!(
        query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":moved_owner.key,"limit":100}),
                1790812802000,
                &moved_store,
                &moved_host,
                None,
            )
            .unwrap(),
        expected_bodies[&moved_owner.key]["body"]
    );
    drop(moved_cache);
    let mut moved_cache =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(&archived).unwrap(),"{}",serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:03Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"sanitized relocation continuation"}]}})).unwrap();
    moved_cache
        .ingest(&format!("{:064x}", 156), &archived, "codex")
        .unwrap();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        assert_eq!(
            native_keys::read_meta(tx, receipt_key.as_bytes(), 16384)?.as_ref(),
            Some(&relocation_request),
            "append preserves the exact original relocation receipt"
        );
        assert_eq!(
            withdrawal::remove(tx, &removed)?,
            Outcome::Written,
            "completed relocation replay follows the qualified appended proof"
        );
        Ok(())
    })
    .unwrap();
    let appended =
        crate::model::prepare_sources(&options, &mut moved_cache, &moved_inputs, 1790812804000)
            .unwrap();
    assert!(appended.rows.iter().any(|r| r.key == moved_owner.key));
    assert_eq!(
        captured_bodies(&pool),
        expected_bodies,
        "old captured bodies survive relocation append"
    );
    let (_cache, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let collision = ns("vertical140\0🦀other");
    let (other, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        collision.clone(),
        control.clone(),
    )
    .unwrap();
    assert_eq!(
        other
            .session_descriptions(&[crate::events::SessionDescriptionRequest {
                key: key.clone(),
                version: 1,
                inputs: "inputs".into(),
            }])
            .unwrap(),
        vec![None]
    );
    let other_binding = other.query_runtime_binding().unwrap().unwrap();
    assert_ne!(binding.stream_epoch, other_binding.stream_epoch);
    let cancelled = PgCancellation::default();
    cancelled.cancel();
    assert!(matches!(
        crate::storage::postgres::index_store::open(pool.clone(), collision, cancelled),
        Err(StorageError::Busy)
    ));
    let facts_path = oracle.root.join("positive-facts158.json");
    let mut recorded = crate::Facts {
        version: 2,
        hostname: "source-write-before158".into(),
        ..Default::default()
    };
    crate::write_facts(&facts_path, &recorded, &options.source).unwrap();
    let mut facts_options = options.clone();
    facts_options.facts = Some(facts_path.clone());
    let before_runtime = crate::native_evidence::query_runtime_generation(&facts_options).unwrap();
    // Construction is lazy, but both observer and ordinary machine-set owners
    // must retain the same physical binding as the public receiver writers.
    let observer = crate::SessionCatalogObserver::new(facts_options.clone());
    let observed = observer.shared_view();
    assert!(matches!(
        observed.evidence.store_config.as_ref().unwrap().backend,
        crate::storage::StoreBackend::Postgres { .. }
    ));
    // The local publication cannot qualify this different, unprepared Facts
    // context, even when its immutable Raw recipes and host custody still match.
    let changed_runtime_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.clone(),
        options: facts_options.clone(),
    };
    for row in &prepared.rows {
        for tool in ["read_transcript", "get_session"] {
            let error = query
                .call_bound_selected_at(
                    tool,
                    &serde_json::json!({"id":row.key}),
                    1790812802000,
                    &changed_runtime_store,
                    &retained_host,
                    None,
                )
                .unwrap_err();
            assert_eq!(
                error.code, "unavailable",
                "unqualified Facts context {tool} {}",
                row.key
            );
        }
    }
    let from = fs::metadata(&path).unwrap().len();
    let writer = crate::SessionSourceWrite::begin_with_store(
        &facts_options,
        &inputs[0],
        None,
        from,
        raw_store(),
        None,
    )
    .unwrap();
    let facts_intent = writer.intent();
    let extra = facts_append;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(extra.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    writer.complete(from + extra.len() as u64).unwrap();
    let replay = crate::SessionSourceWrite::begin_with_store(
        &facts_options,
        &inputs[0],
        None,
        0,
        raw_store(),
        Some(facts_intent),
    )
    .unwrap();
    replay.complete(from + extra.len() as u64).unwrap();
    let pid_input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "sessions/4242.json".into(),
    };
    assert!(!pid_input.full_path(&facts_options).exists());
    fs::create_dir_all(options.claude_home.join("sessions")).unwrap();
    let pid_write = crate::SessionSourceWrite::begin_with_store(
        &facts_options,
        &pid_input,
        None,
        0,
        raw_store(),
        None,
    )
    .unwrap();
    // The moved publication already owns the continued parent prefix. Its
    // exact Index cohort is still qualified before this independent PID write.
    let pid_owner = moved_rows
        .iter()
        .find(|row| row.harness == "claude" && row.native_ids.iter().any(|id| id == "one"))
        .unwrap();
    let pid_host = moved_host.clone();
    let assert_pid_ledgers = || {
        super::super::snapshot::with_admitted_snapshot(
            &pool,
            &namespace,
            &control,
            &moved_store.generation,
            |snapshot| {
                let current = snapshot.catalog(&pid_owner.key, false)?.unwrap();
                assert_eq!(
                    current.lifecycle,
                    crate::model::summary::CatalogLifecycle::Current
                );
                let history = snapshot.owner(&pid_owner.key)?.is_some();
                assert!(history, "the PID owner keeps its original History source");
                let (row, _, _) = snapshot.selected(&pid_owner.key, history, 0, 1)?.unwrap();
                assert_eq!(row.key, current.key);
                assert_eq!(row.harness, current.harness);
                let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
                let history_source = row
                    .sources
                    .iter()
                    .find(|source| source.native_id == "one")
                    .ok_or(StorageError::ScopeChanged)?;
                assert!(
                    history_source == &pid_original_source,
                    "History keeps the exact original source ledger"
                );
                let current_source = current
                    .sources
                    .iter()
                    .find(|source| source.native_id == "one")
                    .ok_or(StorageError::ScopeChanged)?;
                let history_proof =
                    native_proof_tuple(&mut tx, &moved_store.generation, history_source)?;
                let current_proof =
                    native_proof_tuple(&mut tx, &moved_store.generation, current_source)?;
                assert!(
                    history_proof == pid_original_proof,
                    "History keeps the complete original Native proof tuple"
                );
                assert!(
                    history_proof != current_proof,
                    "a different Current proof cannot substitute for History"
                );
                assert!(
                    super::super::super::native_reader::runtime_source_cwds_if_current(
                        &mut tx,
                        &current.sources,
                    )?
                    .is_some(),
                    "the PID owner must qualify through its exact Current Index ledgers"
                );
                Ok(())
            },
        )
        .unwrap();
    };
    assert_pid_ledgers();
    let pid_body = query
        .call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":pid_owner.key,"limit":100}),
            1790812802000,
            &moved_store,
            &pid_host,
            None,
        )
        .unwrap();
    let pid_fields = query
        .call_bound_selected_at(
            "get_session",
            &serde_json::json!({"id":pid_owner.key}),
            1790812802000,
            &moved_store,
            &pid_host,
            None,
        )
        .unwrap();
    assert_eq!(
        serde_json::json!({"body":pid_body,"fields":pid_fields}),
        next_bodies[&pid_owner.key],
        "the qualified parent keeps its exact continued body and fields before PID creation"
    );
    // Earlier fixture mutations restored bytes, not their watched file clocks.
    // Isolate this PID write from those intentional runtime witness changes.
    let before_pid_runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    fs::write(pid_input.full_path(&facts_options), &pid_bytes).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(pid_input.full_path(&facts_options))
        .unwrap()
        .sync_all()
        .unwrap();
    pid_write.complete(pid_bytes.len() as u64).unwrap();
    assert!(
        crate::native_evidence::query_runtime_generation(&options).unwrap() != before_pid_runtime,
        "the shared PID mutation must invalidate the old local runtime witness"
    );
    assert_eq!(
        read_selected(),
        expected_field,
        "the retained native field proof survives an independent runtime mutation"
    );
    assert_pid_ledgers();
    for tool in ["read_transcript", "get_session"] {
        let error = query
            .call_bound_selected_at(
                tool,
                &serde_json::json!({"id":pid_owner.key}),
                1790812802000,
                &moved_store,
                &pid_host,
                None,
            )
            .unwrap_err();
        assert_eq!(
            (error.code, error.message.as_str()),
            (
                "unavailable",
                StorageError::ScopeChanged.to_string().as_str()
            ),
            "the previously qualified PID owner must refuse {tool}"
        );
    }
    // The original parent has an advanced Index and a changed Native("one")
    // observation. Every other original owner retains its exact Index and
    // scoped dependencies, including relocated/withdrawn sources whose host
    // bytes and native proofs remain authorized in this original generation.
    let mut affected_owners = 0;
    let mut unaffected_owners = 0;
    for row in &prepared.rows {
        let affected = row.harness == "claude" && row.native_ids.iter().any(|id| id == "one");
        assert_eq!(
            affected,
            row.sources.iter().any(|source| source.path == path),
            "the fixture's changed native identity and parent source agree"
        );
        let body = query.call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":row.key,"limit":100}),
            1790812802000,
            &query_store,
            &retained_host,
            None,
        );
        let fields = query.call_bound_selected_at(
            "get_session",
            &serde_json::json!({"id":row.key}),
            1790812802000,
            &query_store,
            &retained_host,
            None,
        );
        if affected {
            affected_owners += 1;
            for (tool, result) in [("read_transcript", body), ("get_session", fields)] {
                let error = result.unwrap_err();
                assert_eq!(
                    (error.code, error.message.as_str()),
                    (
                        "unavailable",
                        StorageError::ScopeChanged.to_string().as_str()
                    ),
                    "affected original Current {tool} {}",
                    row.key
                );
            }
        } else {
            unaffected_owners += 1;
            assert_eq!(
                serde_json::json!({"body":body.unwrap(),"fields":fields.unwrap()}),
                expected_bodies[&row.key],
                "unchanged original Current body and fields {}",
                row.key
            );
        }
    }
    assert!(affected_owners > 0, "fixture has a PID-dependent owner");
    assert!(
        unaffected_owners > 0,
        "fixture has unchanged qualified owners"
    );
    assert_eq!(captured_bodies(&pool), expected_bodies);
    recorded.hostname = "source-write-after158".into();
    recorded.proc_starts.insert(4242, 1234);
    let facts_writer = crate::SessionFactsWrite::begin(&facts_options, &recorded).unwrap();
    let facts_request = facts_writer.intent();
    drop(facts_writer); // Intent/no Facts bytes.
    let facts_writer = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &recorded,
        true,
        raw_store(),
        Some(facts_request.clone()),
    )
    .unwrap();
    crate::write_facts(&facts_path, &recorded, &facts_options.source).unwrap();
    assert!(
        !facts_path.with_extension("json.sources.sqlite").exists(),
        "the public PG Facts writer must not create a SQLite projection"
    );
    drop(facts_writer); // Bytes/no SQL completion.
    let facts_writer = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &recorded,
        true,
        raw_store(),
        Some(facts_request.clone()),
    )
    .unwrap();
    facts_writer.complete().unwrap();
    let facts_writer = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &recorded,
        true,
        raw_store(),
        Some(facts_request.clone()),
    )
    .unwrap();
    facts_writer.complete().unwrap(); // Lost ACK.
    let mut stale_facts = facts_request.clone();
    stale_facts.operation.push_str("-stale");
    stale_facts.head = Some("stale".into());
    assert!(
        crate::SessionFactsWrite::begin_with_store(
            &facts_options,
            &recorded,
            true,
            raw_store(),
            Some(stale_facts)
        )
        .is_err()
    );
    let mut wrong = facts_request;
    wrong.before.hostname.push_str("-wrong");
    assert!(
        crate::SessionFactsWrite::begin_with_store(
            &facts_options,
            &recorded,
            true,
            raw_store(),
            Some(wrong)
        )
        .is_err()
    );
    assert!(
        crate::model::pid_files(
            &facts_options,
            &crate::facts::MachineFacts::Recorded(Box::new(recorded.clone()))
        )
        .iter()
        .any(|p| p.pid == 4242 && p.session == "one" && p.alive)
    );
    let runtime = crate::native_evidence::query_runtime_generation(&facts_options).unwrap();
    assert_ne!(before_runtime, runtime);
    moved_cache
        .ingest(&format!("{:064x}", 159), &path, "claude")
        .unwrap();
    moved_cache
        .begin_semantic_stage(
            &moved_inputs
                .iter()
                .map(|i| i.full_path(&facts_options).to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            &runtime,
        )
        .unwrap();
    let (facts_generation, facts_rows) = loop {
        let batch = moved_cache.semantic_batch(64).unwrap().unwrap();
        let model = moved_cache
            .prepare_semantic_model_batch(
                &facts_options,
                &stage_inputs(&facts_options, &batch),
                &batch,
                1790812805000,
            )
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            assert!(
                serde_json::to_string(&model.prepared().rows)
                    .unwrap()
                    .contains("source-write-after158")
            );
            let ack = moved_cache
                .publish_semantic_catalog(&batch, model.prepared())
                .unwrap();
            moved_cache.finish_semantic_stage(batch.epoch).unwrap();
            break (ack.1.unwrap(), model.prepared().rows.clone());
        }
    };
    assert_eq!(facts_generation.len(), 64);
    // The existing Facts-write scene also exercises the bounded authority
    // under the process-wide SQLite guard, including actual header values.
    let facts_authority = {
        let limits =
            crate::selected_read::SelectedReadControl::default().with_limits(256, 16 * 1024);
        let _scope = crate::selected_read::Scope::enter(&limits);
        let authority = moved_cache.facts_authority(&facts_path).unwrap();
        assert!(authority.facts_known());
        assert_eq!(
            authority.hostname().as_deref(),
            Some("source-write-after158")
        );
        assert_eq!(
            authority
                .codex_current("sessions/unselected.jsonl")
                .unwrap(),
            None
        );
        authority.validate().unwrap();
        authority
    };
    assert!(
        other.facts_authority(&facts_path).is_err(),
        "namespace cannot borrow another Facts head"
    );
    let binding = crate::storage::NativeReaderBinding::Admitted(config.clone());
    let navigation = crate::storage::navigation_store(binding.clone());
    assert_eq!(navigation.catalog_generation().unwrap(), facts_generation);
    let bookmark = binding
        .bookmark_identity(&facts_options, namespace.as_str(), &facts_rows[0].key)
        .unwrap()
        .unwrap();
    assert_eq!(bookmark.catalog_key, facts_rows[0].key);
    assert_eq!(bookmark.generation, facts_generation);
    let page = crate::catalog::page(
        &facts_options,
        namespace.as_str(),
        crate::catalog::Request::parse("limit=50").unwrap(),
    );
    assert_eq!(page.status, 200, "ordinary bound Catalog renderer");
    assert!(String::from_utf8_lossy(&page.body).contains("source-write-after158"));
    let workspace = crate::session_catalog_union(
        &[crate::OverviewSource {
            key: "recorded-machine-independent-of-source".into(),
            label: "Recorded PG source".into(),
            options: facts_options.clone(),
            up: false,
        }],
        "authorized-workspace224",
        "limit=50",
    );
    assert_eq!(workspace.status, 200);
    let workspace: serde_json::Value = serde_json::from_slice(&workspace.body).unwrap();
    assert_eq!(
        workspace["items"].as_array().unwrap().len(),
        facts_rows.len()
    );

    let facts_store =
        crate::storage::configured_query_catalog(&facts_options, config.clone()).unwrap();
    let mut facts_query = crate::Query::from_sources(std::sync::Arc::new(
        crate::SessionSources::with_configured_machines(vec![(
            namespace.as_str().into(),
            facts_options.clone(),
            config.clone(),
        )])
        .unwrap(),
    ));
    let list = facts_query
        .call_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            None,
            crate::SessionSourceReadMode::LocalThenProvider,
            1790812805000,
        )
        .unwrap();
    let bound_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812805000,
            facts_store.as_ref(),
        )
        .unwrap();
    assert_eq!(list, bound_list);
    assert!(list.to_string().contains("source-write-after158"));
    let metadata = config.clone().open().unwrap();
    let mut hints = metadata.read_snapshot(&namespace).unwrap();
    for scope in [
        crate::CatalogReadScope::Current,
        crate::CatalogReadScope::RetainedHistory,
    ] {
        let page = hints
            .catalog_changes(
                &namespace,
                crate::storage::CatalogChangesRequest {
                    scope,
                    stream_epoch: config.expected_stream_epoch.as_deref(),
                    after: 0,
                    limit: 256,
                },
            )
            .unwrap();
        assert_eq!(page.source_generation, facts_generation);
        assert!(page.caught_up);
        assert!(!page.changes.is_empty());
        assert!(
            facts_rows
                .iter()
                .all(|row| page.changes.iter().any(|hint| hint.catalog_key == row.key))
        );
        assert!(
            page.changes
                .windows(2)
                .all(|pair| pair[0].revision < pair[1].revision)
        );
        let compacted = metadata
            .compact_catalog_changes(&namespace, scope, &page.stream_epoch, page.head, 256)
            .unwrap();
        assert!(compacted.removed > 0);
        assert!(
            hints
                .catalog_changes(
                    &namespace,
                    crate::storage::CatalogChangesRequest {
                        scope,
                        stream_epoch: Some(&page.stream_epoch),
                        after: 0,
                        limit: 256,
                    }
                )
                .is_err(),
            "compaction invalidates an old cursor"
        );
        let repeated = metadata
            .compact_catalog_changes(&namespace, scope, &page.stream_epoch, page.head, 256)
            .unwrap();
        assert_eq!(repeated.removed, 0);
        assert_eq!(repeated.compacted_through, compacted.compacted_through);
    }
    let mut facts_host = body_host(&facts_options, &facts_rows);
    facts_host.0.extend(retained_host.0.iter().cloned());
    let factory_host = facts_host.clone();
    let factory_rows = facts_rows.clone();
    let factory_namespace = namespace.clone();
    let facts_readers =
        move |source: &str, key: &str| -> std::io::Result<Box<dyn crate::SessionSourceReader>> {
            assert_eq!(source, factory_namespace.as_str());
            assert!(factory_rows.iter().any(|row| row.key == key));
            Ok(Box::new(factory_host.clone()))
        };
    for row in &facts_rows {
        for tool in ["read_transcript", "get_session"] {
            let args = serde_json::json!({"id":row.key});
            let expected = facts_query
                .call_bound_selected_at(
                    tool,
                    &args,
                    1790812805000,
                    facts_store.as_ref(),
                    &facts_host,
                    None,
                )
                .unwrap();
            let actual = facts_query
                .call_selected_at(
                    namespace.as_str(),
                    &row.key,
                    tool,
                    &args,
                    Some(&facts_host),
                    crate::SessionSourceReadMode::LocalThenProvider,
                    1790812805000,
                )
                .unwrap();
            assert_eq!(
                actual, expected,
                "configured ordinary Facts selected {tool} {}",
                row.key
            );
            assert_eq!(
                facts_query
                    .call_catalog_at(
                        tool,
                        &args,
                        Some(&facts_readers),
                        crate::SessionSourceReadMode::LocalThenProvider,
                        1790812805000,
                    )
                    .unwrap(),
                expected,
                "ordinary host-routed {tool} {}",
                row.key
            );
        }
    }

    assert_eq!(
        fs::read(&path).unwrap(),
        [raw[0].as_slice(), append.as_bytes(), extra.as_bytes()].concat()
    );
    assert!(!options.cache.exists());
    // (a) Ordinary incremental stages grow Current beyond 64 owners. Each
    // stage names only its changed paths; unmentioned owners carry forward.
    let withdrawal_fence_checked = std::cell::Cell::new(false);
    let stage_publish = |cache: &mut EventCache, inputs: &[crate::Input], at: i64| {
        let paths = inputs
            .iter()
            .map(|i| i.full_path(&facts_options).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let runtime = crate::native_evidence::query_runtime_generation(&facts_options).unwrap();
        cache.begin_semantic_stage(&paths, &runtime).unwrap();
        loop {
            let batch = cache.semantic_batch(64).unwrap().unwrap();
            let model = cache
                .prepare_semantic_model_batch(
                    &facts_options,
                    &stage_inputs(&facts_options, &batch),
                    &batch,
                    at,
                )
                .unwrap();
            if batch.phase == crate::model::relation_stage::Phase::Publication {
                // Each bounded custody advance commits as a Deferred checkpoint;
                // the qualified head is unchanged until the final CAS publishes.
                let head = cache.session_catalog_generation().unwrap();
                if !model.prepared().withdrawals.is_empty() {
                    if !withdrawal_fence_checked.replace(true) {
                        withdrawal_preflight(
                            &pool,
                            &namespace,
                            &facts_options,
                            &batch,
                            model.prepared(),
                        );
                        #[cfg(unix)]
                        {
                            let checked = withdrawal_commit_hook(&facts_options, model.prepared());
                            assert_eq!(
                                cache
                                    .record_semantic_batch(&batch, model.prepared())
                                    .unwrap(),
                                Outcome::Written
                            );
                            assert!(checked.get());
                            withdrawal_guard_released(&facts_options, model.prepared());
                        }
                    }
                    let stale_runtime = format!("{}:stale", batch.runtime);
                    let mut stale_withdrawals = model.prepared().withdrawals.clone();
                    for withdrawal in &mut stale_withdrawals {
                        withdrawal.runtime.clone_from(&stale_runtime);
                    }
                    let lifecycle = super::super::publication::preflight(
                        model.prepared().relationships.lifecycle_authority.as_ref(),
                    )
                    .unwrap();
                    assert_eq!(
                        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                            super::super::publication::publish(
                                tx,
                                &crate::slot_projection::Publication {
                                    rows: &model.prepared().rows,
                                    transcripts: Some(&model.prepared().transcripts),
                                    observations: None,
                                },
                                batch.base_generation.as_deref(),
                                &model.prepared().claims,
                                false,
                                Some(super::super::publication::Semantic {
                                    runtime: &stale_runtime,
                                    withdrawals: &stale_withdrawals,
                                    prior_owners: &batch.context.prior_owners,
                                    retired_owners: &model.prepared().relationships.retired_owners,
                                    lifecycle_authority: lifecycle,
                                }),
                            )
                        }),
                        Err(StorageError::Conflict),
                        "coherent stale lifecycle runtime cannot replace persisted Facts authority"
                    );
                    assert_eq!(cache.session_catalog_generation().unwrap(), head);
                }
                let mut deferred = 0;
                let ack = loop {
                    #[cfg(unix)]
                    let checked = (!model.prepared().withdrawals.is_empty())
                        .then(|| withdrawal_commit_hook(&facts_options, model.prepared()));
                    let ack = cache
                        .publish_semantic_catalog(&batch, model.prepared())
                        .unwrap();
                    #[cfg(unix)]
                    if let Some(checked) = checked {
                        assert!(checked.get());
                        withdrawal_guard_released(&facts_options, model.prepared());
                    }
                    if ack.0 != crate::events::Outcome::Deferred {
                        break ack;
                    }
                    assert_eq!(ack.1, None);
                    assert_eq!(cache.session_catalog_generation().unwrap(), head);
                    deferred += 1;
                    assert!(deferred < 64, "custody makes bounded progress");
                };
                assert_eq!(ack.0, crate::events::Outcome::Written);
                if let Some(negative) = model.prepared().withdrawals.first() {
                    let path = std::path::Path::new(&negative.path);
                    let moved = path.with_extension("withdrawal-replay");
                    fs::rename(path, &moved).unwrap();
                    let replay = cache.publish_semantic_catalog(&batch, model.prepared());
                    fs::rename(&moved, path).unwrap();
                    assert_eq!(
                        replay.unwrap(),
                        ack,
                        "completed replay uses its original ACK"
                    );
                }
                cache.finish_semantic_stage(batch.epoch).unwrap();
                break (ack.1.unwrap(), batch, model, deferred);
            }
        }
    };
    let bulk = (0..70)
        .map(|i| {
            let input = crate::Input {
                root: crate::InputRoot::Claude,
                path: format!("projects/project/bulk169-{i:03}.jsonl"),
            };
            let body = serde_json::json!({"type":"user","sessionId":format!("bulk169-{i:03}"),"uuid":format!("bulk-{i}"),"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized bulk input"}});
            fs::write(input.full_path(&facts_options), format!("{body}\n")).unwrap();
            input
        })
        .collect::<Vec<_>>();
    for (at, input) in bulk.iter().enumerate() {
        moved_cache
            .ingest(
                &format!("{:064x}", 1000 + at),
                &input.full_path(&facts_options),
                "claude",
            )
            .unwrap();
    }
    let mut bulk_generation = String::new();
    for chunk in bulk.chunks(30) {
        bulk_generation = stage_publish(&mut moved_cache, chunk, 1790812806000).0;
    }
    let store_at = |generation: &str| super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: generation.to_owned(),
        options: facts_options.clone(),
    };
    let total = list["total"].as_u64().unwrap() + 70;
    assert!(total > 64);
    let bulk_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812806000,
            &store_at(&bulk_generation),
        )
        .unwrap();
    assert_eq!(bulk_list["total"].as_u64(), Some(total));
    assert!(bulk_list.to_string().contains("source-write-after158"));
    let bulk_page = super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &bulk_generation,
        |snapshot| {
            crate::catalog::read_page_snapshot(
                &facts_options,
                "",
                &crate::catalog::Request::parse("limit=20").unwrap(),
                snapshot,
            )
            .map_err(|_| StorageError::ScopeChanged)
        },
    )
    .unwrap();
    assert_eq!(bulk_page.status, 200);
    // (b) A small partial update republishes one owner; every unrelated
    // Current row, Source and proof stays byte-identical, as do Facts bytes.
    let facts_bytes = fs::read(&facts_path).unwrap();
    writeln!(fs::OpenOptions::new().append(true).open(bulk[5].full_path(&facts_options)).unwrap(),"{}",serde_json::json!({"type":"user","sessionId":"bulk169-005","uuid":"bulk-5b","parentUuid":"bulk-5","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:01Z","message":{"role":"user","content":"sanitized bulk continuation"}})).unwrap();
    moved_cache
        .ingest(
            &format!("{:064x}", 1100),
            &bulk[5].full_path(&facts_options),
            "claude",
        )
        .unwrap();
    let (partial_generation, partial_batch, partial_model, _) =
        stage_publish(&mut moved_cache, &bulk[5..6], 1790812807000);
    assert_ne!(partial_generation, bulk_generation);
    // Unrelated means every owner and proof not reading `path`.
    let unrelated = |a: &str, b: &str, path: &str| {
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let ns = namespace.as_str().as_bytes();
            let mut differences = Vec::new();
            let owner = "session_key NOT IN (SELECT session_key FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation IN ($2,$3) AND source_path=$4)";
            for (table, columns, owner) in [
                ("catalog_rows", "session_key,metadata", owner),
                (
                    "catalog_sources",
                    "session_key,source_path,metadata,native_digest",
                    owner,
                ),
                (
                    "catalog_proofs",
                    "source_path,page_generation,usage_generation,usage_ready",
                    "source_path<>$4",
                ),
            ] {
                let count: i64 = tx.driver.query_one(&format!("SELECT count(*) FROM (((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND {owner}) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND {owner})) UNION ALL ((SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND {owner}) EXCEPT ALL (SELECT {columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND {owner}))) d"), &[&ns, &a.as_bytes(), &b.as_bytes(), &path.as_bytes()]).map_err(|_| StorageError::Unavailable)?.get(0);
                differences.push(count);
            }
            Ok(differences)
        })
        .unwrap()
    };
    let changed = bulk[5].full_path(&facts_options);
    assert_eq!(
        unrelated(
            &bulk_generation,
            &partial_generation,
            changed.to_str().unwrap()
        ),
        vec![0, 0, 0]
    );
    assert_eq!(fs::read(&facts_path).unwrap(), facts_bytes);
    let partial_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812807000,
            &store_at(&partial_generation),
        )
        .unwrap();
    assert_eq!(partial_list["total"].as_u64(), Some(total));
    assert!(partial_list.to_string().contains("source-write-after158"));
    // (c) An isolated Source withdraws by bounded pages. A mixed or connected
    // owner's Source is withdrawn last, below, through the semantic catch-up.
    let (mut store, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let agent = claude_agent.to_str().unwrap().to_owned();
    let agent_removal = vec![(agent.clone(), store.ledger(&agent).unwrap())];
    let composite_b = composite_b_path.to_str().unwrap().to_owned();
    let composite_b_removal = vec![(composite_b.clone(), store.ledger(&composite_b).unwrap())];
    let gone = bulk[1].full_path(&facts_options);
    let gone_path = gone.to_str().unwrap().to_owned();
    let gone_removal = vec![(gone_path.clone(), store.ledger(&gone_path).unwrap())];
    fs::rename(&gone, oracle.root.join("retained-bulk169.jsonl")).unwrap();
    assert_eq!(
        store.remove_files(&gone_removal).unwrap(),
        crate::events::Outcome::Written
    );
    let withdrawn = store.session_catalog_generation().unwrap().unwrap();
    assert_ne!(withdrawn, partial_generation);
    assert_eq!(
        unrelated(&partial_generation, &withdrawn, &gone_path),
        vec![0, 0, 0],
        "withdrawal of an isolated owner keeps unrelated Current"
    );
    // A successor publication, then exact old requests replay their own
    // recorded generations without republishing or moving the head.
    writeln!(fs::OpenOptions::new().append(true).open(bulk[2].full_path(&facts_options)).unwrap(),"{}",serde_json::json!({"type":"user","sessionId":"bulk169-002","uuid":"bulk-2b","parentUuid":"bulk-2","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":"sanitized successor"}})).unwrap();
    moved_cache
        .ingest(
            &format!("{:064x}", 1101),
            &bulk[2].full_path(&facts_options),
            "claude",
        )
        .unwrap();
    let successor = stage_publish(&mut moved_cache, &bulk[2..3], 1790812808000).0;
    assert_ne!(successor, withdrawn);
    assert_eq!(
        moved_cache
            .publish_semantic_catalog(&partial_batch, partial_model.prepared())
            .unwrap()
            .1
            .as_deref(),
        Some(partial_generation.as_str())
    );
    assert_eq!(
        store.remove_files(&gone_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        store.session_catalog_generation().unwrap().as_deref(),
        Some(successor.as_str())
    );
    let successor_list = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812808000,
            &store_at(&successor),
        )
        .unwrap();
    assert_eq!(successor_list["total"].as_u64(), Some(total - 1));
    // (d) Restart: reopened stores read the newest Current and the old
    // selected bodies exactly as before.
    drop(store);
    drop(moved_cache);
    let reopened =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        reopened.session_catalog_generation().as_deref(),
        Some(successor.as_str())
    );
    assert_eq!(
        captured_bodies(&pool),
        expected_bodies,
        "old selected bodies after incremental publication and restart"
    );
    // Fresh Pool: new connections run the cold binding audit, then read the
    // committed head, every older committed generation's cohort (retained
    // history merged as in SQLite) and the old selected bodies.
    drop(reopened);
    let started = std::time::Instant::now();
    let cold = crate::storage::postgres::PgPool::new(
        factory.clone(),
        crate::storage::postgres::tests::limits(),
    )
    .unwrap();
    let cold_cache =
        EventCache::open_pg_native(cold.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        cold_cache.session_catalog_generation().as_deref(),
        Some(successor.as_str())
    );
    let cold_store =
        |generation: &str, options: &Options| super::super::snapshot::query::QueryCatalog {
            captured: false,
            pool: cold.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: generation.to_owned(),
            options: options.clone(),
        };
    let first = facts_query
        .call_bound_catalog_at(
            "list_sessions",
            &serde_json::json!({}),
            1790812809000,
            &cold_store(&successor, &facts_options),
        )
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "cold first page"
    );
    assert_eq!(first["total"].as_u64(), Some(total - 1));
    for (at, expected) in [
        (&bulk_generation, total),
        (&partial_generation, total),
        (&withdrawn, total - 1),
    ] {
        let listed = facts_query
            .call_bound_catalog_at(
                "list_sessions",
                &serde_json::json!({}),
                1790812809000,
                &cold_store(at, &facts_options),
            )
            .unwrap();
        assert_eq!(listed["total"].as_u64(), Some(expected), "older cohort");
    }
    assert_eq!(
        captured_bodies(&cold),
        expected_bodies,
        "old selected bodies through a fresh Pool"
    );
    // Codex active/archive membership: a Facts write selecting the archived
    // rollout completes, replays its lost ACK and keeps the exact Facts bytes.
    let mut archived_facts = recorded.clone();
    archived_facts.codex_rollouts = Some(std::collections::BTreeSet::from([format!(
        "archived_sessions/rollout-{codex_id}.jsonl"
    )]));
    let membership = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &archived_facts,
        true,
        raw_store(),
        None,
    )
    .unwrap();
    let membership_request = membership.intent();
    crate::save_json_file(&facts_path, &archived_facts).unwrap();
    membership.complete().unwrap();
    crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &archived_facts,
        true,
        raw_store(),
        Some(membership_request),
    )
    .unwrap()
    .complete()
    .unwrap();
    assert_eq!(crate::read_facts(&facts_path).unwrap(), archived_facts);
    assert!(
        facts_authority.validate().is_err(),
        "exact canonical replacement invalidates the old authority"
    );
    cold_cache.prepare_facts_authority(&facts_options).unwrap();
    let selected = cold_cache.facts_authority(&facts_path).unwrap();
    assert_eq!(
        selected
            .codex_current(&format!("archived_sessions/rollout-{codex_id}.jsonl"))
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        selected.codex_current("sessions/unselected.jsonl").unwrap(),
        Some(false)
    );

    // A global Facts change invalidates every Current Source (> 64 here) by
    // bounded pages instead of refusing, and replays its lost ACK.
    let mut renamed = archived_facts.clone();
    renamed.hostname = "source-write-after174".into();
    let global = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &renamed,
        true,
        raw_store(),
        None,
    )
    .unwrap();
    let global_request = global.intent();
    crate::save_json_file(&facts_path, &renamed).unwrap();
    global.complete().unwrap();
    crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &renamed,
        true,
        raw_store(),
        Some(global_request),
    )
    .unwrap()
    .complete()
    .unwrap();
    assert_eq!(crate::read_facts(&facts_path).unwrap(), renamed);
    // (e) A connected Source withdrawal commits only its Index intent and the
    // `removed` invalidation. The qualified Current generation stays until the
    // ordinary semantic catch-up publishes; an exact lost-ACK replay is applied.
    let (mut store, _) = crate::storage::postgres::index_store::open(
        pool.clone(),
        namespace.clone(),
        control.clone(),
    )
    .unwrap();
    let qualified = store.session_catalog_generation().unwrap();
    let (composite_current_source, composite_withdrawn_source, composite_old_proof) =
        super::super::snapshot::with_admitted_snapshot(
            &pool,
            &namespace,
            &control,
            qualified.as_deref().unwrap(),
            |snapshot| {
                assert!(
                    snapshot.owner(composite_key)?.is_none(),
                    "the fresh owner has no History before Source B is withdrawn"
                );
                let current = snapshot
                    .catalog(composite_key, false)?
                    .ok_or(StorageError::ScopeChanged)?;
                assert_eq!(current.sources.len(), 2);
                let current_source = current
                    .sources
                    .iter()
                    .find(|source| source.path.to_str() == composite_a_path.to_str())
                    .ok_or(StorageError::ScopeChanged)?
                    .clone();
                let withdrawn_source = current
                    .sources
                    .iter()
                    .find(|source| source.path.to_str() == composite_b_path.to_str())
                    .ok_or(StorageError::ScopeChanged)?
                    .clone();
                let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
                let proof =
                    native_proof_tuple(&mut tx, qualified.as_deref().unwrap(), &withdrawn_source)?;
                Ok((current_source, withdrawn_source, proof))
            },
        )
        .unwrap();
    fs::rename(&claude_agent, oracle.root.join("retained-agent180.jsonl")).unwrap();
    assert_eq!(
        store.remove_files(&agent_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(store.session_catalog_generation().unwrap(), qualified);
    let reason: Vec<u8> = pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let row = tx.driver.query_one("SELECT reason FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&namespace.as_str().as_bytes(), &agent.as_bytes()]).map_err(sqlstate::error)?;
        Ok(row.get::<_, String>(0).into_bytes())
    }).unwrap();
    assert_eq!(reason, b"removed");
    assert_eq!(
        store.remove_files(&agent_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(store.session_catalog_generation().unwrap(), qualified);
    // The second connected subagent is withdrawn the same way; both removed
    // sessions need new retained custody in one catch-up publication.
    let agent191 = claude_agent191.to_str().unwrap().to_owned();
    let agent191_removal = vec![(agent191.clone(), store.ledger(&agent191).unwrap())];
    fs::rename(
        &claude_agent191,
        oracle.root.join("retained-agent191.jsonl"),
    )
    .unwrap();
    assert_eq!(
        store.remove_files(&agent191_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(store.session_catalog_generation().unwrap(), qualified);
    fs::rename(
        &composite_b_path,
        oracle.root.join("retained-fresh-composite-b.jsonl"),
    )
    .unwrap();
    assert_eq!(
        store.remove_files(&composite_b_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(store.session_catalog_generation().unwrap(), qualified);
    // The ordinary semantic catch-up claims the `removed` invalidations and
    // republishes connected owners without the withdrawn Sources.
    let mut catchup =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let connected = [
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/one.jsonl".into(),
        },
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/one/subagents/agent-related154.jsonl".into(),
        },
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/one/subagents/agent-related191.jsonl".into(),
        },
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/fresh-composite-a.jsonl".into(),
        },
        crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/fresh-composite-b.jsonl".into(),
        },
    ];
    let paths = connected
        .iter()
        .map(|i| i.full_path(&facts_options).to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let catchup_runtime = crate::native_evidence::query_runtime_generation(&facts_options).unwrap();
    catchup
        .begin_semantic_stage(&paths, &catchup_runtime)
        .unwrap();
    let (batch, model) = loop {
        let batch = catchup.semantic_batch(64).unwrap().unwrap();
        let model = catchup
            .prepare_semantic_model_batch(
                &facts_options,
                &stage_inputs(&facts_options, &batch),
                &batch,
                1790812809000,
            )
            .unwrap();
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            break (batch, model);
        }
    };
    // Two new custodies need more than one bounded advance. The first
    // checkpoint's ACK is lost with the process; a restarted producer resumes
    // the same operation from the durable checkpoint, not from scratch.
    assert_eq!(
        catchup
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap(),
        (crate::events::Outcome::Deferred, None)
    );
    assert_eq!(
        catchup.session_catalog_generation().unwrap(),
        qualified.as_deref().unwrap()
    );
    drop(catchup);
    let mut catchup =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    let mut deferred = 1;
    let ack = loop {
        let ack = catchup
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap();
        if ack.0 != crate::events::Outcome::Deferred {
            break ack;
        }
        assert_eq!(ack.1, None);
        assert_eq!(
            catchup.session_catalog_generation().unwrap(),
            qualified.as_deref().unwrap()
        );
        deferred += 1;
        assert!(deferred < 64, "custody makes bounded progress");
    };
    assert_eq!(ack.0, crate::events::Outcome::Written);
    // The long withdrawn recipe (>150 slots) cannot fit one 64-slot page.
    assert!(deferred >= 2, "long recipe resumed by pages: {deferred}");
    // A lost Written ACK replays the exact operation and its generation.
    assert_eq!(
        catchup
            .publish_semantic_catalog(&batch, model.prepared())
            .unwrap(),
        ack
    );
    catchup.finish_semantic_stage(batch.epoch).unwrap();
    let caught = ack.1.unwrap();
    assert_eq!(catchup.session_catalog_generation().unwrap(), caught);
    assert_ne!(Some(caught.clone()), qualified);
    let (composite_history_source, history_b_proof, current_a_proof) =
        super::super::snapshot::with_admitted_snapshot(
            &pool,
            &namespace,
            &control,
            &caught,
            |snapshot| {
                assert!(snapshot.owner(composite_key)?.is_some());
                let current = snapshot
                    .catalog(composite_key, false)?
                    .ok_or(StorageError::ScopeChanged)?;
                let (history, _, _) = snapshot
                    .selected(composite_key, true, 0, 1)?
                    .ok_or(StorageError::ScopeChanged)?;
                let history_a = history
                    .sources
                    .iter()
                    .find(|source| source.path == composite_current_source.path)
                    .ok_or(StorageError::ScopeChanged)?;
                assert_eq!(history_a, &composite_current_source);
                let history_b = history
                    .sources
                    .iter()
                    .find(|source| source.path == composite_withdrawn_source.path)
                    .ok_or(StorageError::ScopeChanged)?
                    .clone();
                assert_eq!(history_b, composite_withdrawn_source);
                assert!(
                    current.sources.contains(&composite_current_source),
                    "the unchanged same-harness Source A stays Current"
                );
                assert!(
                    !current.sources.contains(&composite_withdrawn_source),
                    "the same-harness Source B is withdrawn from Current"
                );
                assert_eq!(
                    current.sources,
                    vec![composite_current_source.clone()],
                    "Current is exactly Source A after the withdrawal"
                );
                let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
                let original: bool = tx
                    .driver
                    .query_one(
                        "SELECT original FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3",
                        &[
                            &namespace.as_str().as_bytes(),
                            &caught.as_bytes(),
                            &composite_key.as_bytes(),
                        ],
                    )
                    .map_err(sqlstate::error)?
                    .get(0);
                assert!(!original, "this is a new composite History owner");
                let stored_history_sources: Vec<Vec<u8>> = tx
                    .driver
                    .query(
                        "SELECT s.source_path FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_sources s ON s.namespace_digest=o.namespace_digest AND s.namespace=o.namespace AND s.generation=o.owner_generation AND s.key_digest=o.key_digest AND s.session_key=o.session_key WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.key_digest=sha256($3::bytea) AND o.session_key=$3 ORDER BY s.source_path",
                        &[
                            &namespace.as_str().as_bytes(),
                            &caught.as_bytes(),
                            &composite_key.as_bytes(),
                        ],
                    )
                    .map_err(sqlstate::error)?
                    .iter()
                    .map(|row| row.get(0))
                    .collect();
                assert_eq!(
                    stored_history_sources,
                    vec![
                        super::super::path(&composite_withdrawn_source)?
                            .as_bytes()
                            .to_vec()
                    ],
                    "History stores only the missing Source B; Current owns A's staged proof"
                );
                let history_b_proof = native_proof_tuple(&mut tx, &caught, &history_b)?;
                let current_a_proof =
                    native_proof_tuple(&mut tx, &caught, &composite_current_source)?;
                Ok((history_b, history_b_proof, current_a_proof))
            },
        )
        .unwrap();
    assert_eq!(
        composite_history_source, composite_withdrawn_source,
        "composite History retains the exact withdrawn Source B"
    );
    assert_eq!(
        history_b_proof, composite_old_proof,
        "composite History carries Source B's complete old Native proof tuple"
    );
    assert_ne!(
        history_b_proof, current_a_proof,
        "Current Source A's proof cannot substitute for retained Source B"
    );
    let parent = claude_parent.to_str().unwrap().to_owned();
    let counts: Vec<i64> = pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
        let ns = namespace.as_str().as_bytes();
        let mut counts = Vec::new();
        for (generation, path) in [(caught.as_str(), &agent), (caught.as_str(), &parent), (qualified.as_deref().unwrap(), &agent)] {
            counts.push(tx.driver.query_one("SELECT count(*) FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND source_path=$3", &[&ns, &generation.as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?.get::<_, i64>(0));
        }
        Ok(counts)
    }).unwrap();
    assert_eq!(counts[0], 0, "withdrawn Source left the successor Current");
    assert!(
        counts[1] > 0 && counts[2] > 0,
        "parent republished; qualified generation retained"
    );
    // P3: the successor's retained history is its Current rows plus every
    // session the withdrawal removed, served from the owner headers as
    // Retained. Whole scopes follow the viewer keyset cursor (bulk169 made
    // the generation larger than one page).
    let scoped_items = |generation: &str, scope: &str| {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let query = match &cursor {
                Some(cursor) => format!(
                    "{scope}limit=50&cursor={}",
                    crate::viewer::percent_encode(cursor)
                ),
                None => format!("{scope}limit=50"),
            };
            let reply = super::super::snapshot::with_admitted_generation(
                &pool,
                &namespace,
                &control,
                generation,
                |snapshot| {
                    crate::catalog::read_page_snapshot(
                        &options,
                        recent_machine,
                        &crate::catalog::Request::parse(&query).unwrap(),
                        snapshot,
                    )
                    .map_err(|_| StorageError::ScopeChanged)
                },
            )
            .unwrap();
            assert_eq!(reply.status, 200);
            let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            items.extend(page["items"].as_array().unwrap().iter().cloned());
            match page["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break items,
            }
        }
    };
    let scoped = |generation: &str, scope: &str| {
        let mut items = Vec::new();
        for item in scoped_items(generation, scope) {
            items.push((
                item["key"].as_str().unwrap().to_owned(),
                item["lifecycle"] == "retained",
            ));
        }
        items
    };
    let before = scoped(qualified.as_deref().unwrap(), "");
    let before_history = scoped(qualified.as_deref().unwrap(), "scope=retained_history&");
    let current = scoped(&caught, "");
    let mut retained = scoped(&caught, "scope=retained_history&");
    let current_keys = current
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let removed = before
        .iter()
        .filter(|(key, _)| !current_keys.contains(key))
        .map(|(key, _)| key.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        !removed.is_empty(),
        "connected withdrawal removed no Current session"
    );
    assert!(current.iter().chain(&before).all(|(_, retained)| !retained));
    // Membership and lifecycle are exact; removed rows come from custody.
    let mut expected_retained = before_history
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    for (key, _) in &current {
        expected_retained.entry(key.clone()).or_insert(false);
    }
    for key in &removed {
        expected_retained.insert(key.clone(), true);
    }
    // The parent keeps Current membership while its composite history wins in
    // retained scope. Check the exact SQLite projection, including that case.
    for row in &oracle_history_projection {
        if row["harness"] == "claude" {
            expected_retained.insert(
                row["key"].as_str().unwrap().to_owned(),
                row["lifecycle"] == "retained",
            );
        }
    }
    let mut expected_retained = expected_retained.into_iter().collect::<Vec<_>>();
    expected_retained.sort();
    retained.sort();
    assert_eq!(retained, expected_retained);
    // The same metadata-only resolver serves retained native aliases after
    // their Current row disappears. Expected generation fencing stays active.
    super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &caught,
        |snapshot| {
            use crate::storage::navigation::NativeNavigationStore;
            assert_eq!(snapshot.catalog_generation()?, caught);
            for row in prepared
                .rows
                .iter()
                .filter(|row| removed.contains(&row.key))
            {
                for native in &row.native_ids {
                    let current = snapshot.native_resolution(
                        &row.harness,
                        native,
                        crate::CatalogReadScope::Current,
                        Some(&caught),
                    )?;
                    assert!(current.keys.is_empty());
                    let retained = snapshot.native_resolution(
                        &row.harness,
                        native,
                        crate::CatalogReadScope::RetainedHistory,
                        Some(&caught),
                    )?;
                    assert_eq!(retained.keys, vec![row.key.clone()]);
                    let resolved: CatalogRow =
                        serde_json::from_str(retained.metadata.as_deref().unwrap()).unwrap();
                    assert_eq!(resolved.key, row.key);
                }
            }
            assert!(
                snapshot
                    .native_resolution(
                        "claude",
                        "one",
                        crate::CatalogReadScope::Current,
                        Some(&generation)
                    )
                    .is_err()
            );
            Ok(())
        },
    )
    .unwrap();
    // Owner-side filters: the withdrawn Claude sessions stay Retained under
    // the harness filter and are absent under another harness.
    let claude = scoped(&caught, "scope=retained_history&harness=claude&");
    let codex = scoped(&caught, "scope=retained_history&harness=codex&");
    for key in &removed {
        assert!(
            claude.contains(&(key.clone(), true)),
            "{key} under harness=claude"
        );
        assert!(codex.iter().all(|(other, _)| other != key));
    }
    assert!(claude.len() + codex.len() <= retained.len());
    // The SQLite pre-Guard replay of the same withdrawal: retained-only rows
    // in keyset order and under the harness filter are exactly the oracle's.
    let withdrawn_rows = |generation: &str| {
        scoped_items(generation, "scope=retained_history&")
            .iter()
            .filter(|item| removed.contains(item["key"].as_str().unwrap()))
            .map(stable)
            .collect::<Vec<_>>()
    };
    assert_eq!(withdrawn_rows(&caught), oracle_withdrawn);
    assert_eq!(
        claude
            .iter()
            .filter(|(key, _)| removed.contains(key))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
        oracle_withdrawn_claude
    );
    assert_eq!(
        scoped_items(&caught, "scope=retained_history&harness=claude&")
            .iter()
            .filter(|item| sqlite_before_current.contains(item["key"].as_str().unwrap()))
            .map(stable)
            .collect::<Vec<_>>(),
        oracle_history_projection
            .iter()
            .filter(|item| item["harness"] == "claude")
            .cloned()
            .collect::<Vec<_>>()
    );
    // The parent was appended after its retained owner was captured. Retained
    // list and search pages must use the latest Catalog index fields on both
    // engines, while exact History selection keeps the older stored row.
    let sqlite_parent = sqlite_retained
        .iter()
        .find(|item| item["key"] == "one")
        .expect("appended Claude parent in retained list");
    let exact_history_request =
        crate::catalog::Request::parse("sid=one&scope=retained_history&limit=50").unwrap();
    let pg_parent_exact = super::super::snapshot::with_admitted_generation(
        &pool,
        &namespace,
        &control,
        &caught,
        |snapshot| {
            crate::catalog::read_page_snapshot(
                &options,
                recent_machine,
                &exact_history_request,
                snapshot,
            )
            .map_err(|error| {
                eprintln!("pg_exact_history_page_failed: {error}");
                StorageError::ScopeChanged
            })
        },
    )
    .unwrap();
    assert_eq!(pg_parent_exact.status, 200);
    let pg_parent_exact: serde_json::Value = serde_json::from_slice(&pg_parent_exact.body).unwrap();
    assert_eq!(
        pg_parent_exact["items"], sqlite_parent_exact["items"],
        "exact retained History selection preserves the SQLite item"
    );
    assert_ne!(
        sqlite_parent["last"], sqlite_parent_exact["items"][0]["last"],
        "parent append must move the retained list position"
    );
    let current_only_key = super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &caught,
        |snapshot| {
            for row in prepared.rows.iter().filter(|row| row.harness == "codex") {
                if snapshot.catalog(&row.key, false)?.is_some()
                    && snapshot.owner(&row.key)?.is_none()
                {
                    return Ok(Some(row.key.clone()));
                }
            }
            Ok(None)
        },
    )
    .unwrap()
    .expect("a Current Codex row without a History owner");
    let selected_item = |request: &str| {
        let request = crate::catalog::Request::parse(request).unwrap();
        let reply = super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            &caught,
            |snapshot| {
                crate::catalog::read_page_snapshot(&options, recent_machine, &request, snapshot)
                    .map_err(|_| StorageError::ScopeChanged)
            },
        )
        .unwrap();
        assert_eq!(reply.status, 200);
        let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        page["items"][0].clone()
    };
    let current_only = crate::viewer::percent_encode(&current_only_key);
    let current_item = selected_item(&format!("sid={current_only}&limit=50"));
    let retained_item = selected_item(&format!(
        "sid={current_only}&scope=retained_history&limit=50"
    ));
    assert!(current_item["transcript"]["generation"].is_string());
    assert!(current_item["transcript"]["total"].is_number());
    assert_eq!(
        retained_item["transcript"]["generation"], current_item["transcript"]["generation"],
        "retained scope uses the Current transcript when this key has no History owner"
    );
    assert_eq!(
        retained_item["transcript"]["total"], current_item["transcript"]["total"],
        "retained scope falls back to the Current transcript when this key has no History owner"
    );
    let pg_parent = scoped_items(&caught, "scope=retained_history&")
        .into_iter()
        .find(|item| item["key"] == "one")
        .expect("appended Claude parent in PostgreSQL retained list");
    assert_eq!(stable(&pg_parent), stable(sqlite_parent));
    let caught_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: caught.clone(),
        options: options.clone(),
    };
    let pg_parent_transcript = query
        .call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":"one","limit":100}),
            1790812802000,
            &caught_store,
            &sqlite_host,
            None,
        )
        .expect("PG retained parent transcript through its original source generation");
    let transcript_entries = |value: &serde_json::Value| {
        value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| serde_json::json!({"k":entry["k"],"text":entry["text"]}))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pg_parent_transcript["total"], sqlite_parent_transcript["total"],
        "retained parent transcript totals match SQLite"
    );
    assert_eq!(
        transcript_entries(&pg_parent_transcript),
        transcript_entries(&sqlite_parent_transcript),
        "retained parent transcript content matches SQLite"
    );
    assert_eq!(
        scoped_items(&caught, "scope=retained_history&q=one&")
            .iter()
            .map(stable)
            .collect::<Vec<_>>(),
        sqlite_scoped("scope=retained_history&q=one&", 50)
            .iter()
            .map(stable)
            .collect::<Vec<_>>(),
        "retained search uses the latest Catalog index fields"
    );
    // P3 search: metadata search over the published PG projection. A key
    // query finds its Current session; a removed key is absent from the
    // successor's Current search but found in the qualified generation.
    let search = |generation: &str, scope: &str, key: &str| {
        let query = format!(
            "{scope}limit=50&q={}",
            crate::viewer::percent_encode(&key.to_lowercase())
        );
        let reply = super::super::snapshot::with_admitted_generation(
            &pool,
            &namespace,
            &control,
            generation,
            |snapshot| {
                crate::catalog::read_page_snapshot(
                    &options,
                    recent_machine,
                    &crate::catalog::Request::parse(&query).unwrap(),
                    snapshot,
                )
                .map_err(|_| StorageError::ScopeChanged)
            },
        )
        .unwrap();
        assert_eq!(reply.status, 200, "{query}");
        let page: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let kept = &current[0].0;
    assert!(
        search(&caught, "", kept).contains(kept),
        "Current search {kept}"
    );
    let gone = removed.iter().next().unwrap();
    assert!(
        !search(&caught, "", gone).contains(gone),
        "{gone} withdrawn"
    );
    let old = qualified.as_deref().unwrap();
    assert!(
        search(old, "", gone).contains(gone),
        "{gone} before withdrawal"
    );
    // Retained scope finds the withdrawn key through its carried document.
    assert!(
        search(&caught, "scope=retained_history&", gone).contains(gone),
        "{gone} retained search"
    );
    assert!(search(&caught, "scope=retained_history&", kept).contains(kept));
    for key in &removed {
        assert_eq!(
            search(&caught, "scope=retained_history&", key),
            oracle_retained_search[key]
        );
    }
    // Retained-only selected body: on the successor a withdrawn key has no
    // Current row, so its body is read through the proof-owned retained
    // recipe from Raw bytes and equals the SQLite ordinary body exactly.
    // Session fields also read the qualified connected parent, whose Raw
    // prefix advanced during the ordinary Source/Facts writes above. Retain
    // both actual captures explicitly; the original-only host still serves
    // the retained transcript and never gains authority by path alone.
    let mut withdrawal_host = facts_host.clone();
    withdrawal_host.0.extend(retained_host.0.iter().cloned());
    for key in &removed {
        let body = query
            .call_bound_selected_at(
                "read_transcript",
                &serde_json::json!({"id":key,"limit":100}),
                1790812802000,
                &caught_store,
                &retained_host,
                None,
            )
            .unwrap();
        assert_eq!(body, expected_bodies[key]["body"], "retained body {key}");
        assert_eq!(
            body, oracle_retained_reads[key]["body"],
            "SQLite withdrawal body parity {key}"
        );
        assert!(body.to_string().contains("sanitized agent answer"));
        let fields = query
            .call_bound_selected_at(
                "get_session",
                &serde_json::json!({"id":key}),
                1790812802000,
                &caught_store,
                &withdrawal_host,
                None,
            )
            .unwrap();
        assert_eq!(
            fields, oracle_retained_reads[key]["fields"],
            "SQLite withdrawal fields parity {key}"
        );
    }
    // Exact old OpID after its successor replays without moving the head.
    assert_eq!(
        store.remove_files(&agent_removal).unwrap(),
        crate::events::Outcome::Written
    );
    assert_eq!(
        store.session_catalog_generation().unwrap().as_deref(),
        Some(caught.as_str())
    );
    // Restart continues from the successor; the readers' retained proofs
    // still serve the old selected bodies exactly.
    drop(catchup);
    let mut reopened =
        EventCache::open_pg_native(pool.clone(), namespace.clone(), control.clone()).unwrap();
    assert_eq!(
        reopened.session_catalog_generation().as_deref(),
        Some(caught.as_str())
    );
    assert_eq!(
        captured_bodies(&pool),
        expected_bodies,
        "old selected bodies after a connected withdrawal and restart"
    );
    assert_eq!(
        withdrawn_rows(&caught),
        oracle_withdrawn,
        "retained-only rows after restart"
    );
    // Zero-row publication: Facts no longer list the archived rollout, so
    // its catch-up batch prepares no row. The recorded batch runtime is the
    // lifecycle authority that retires the owner into retained history.
    let mut emptied = renamed.clone();
    emptied.codex_rollouts = Some(std::collections::BTreeSet::new());
    let emptied_write = crate::SessionFactsWrite::begin_with_store(
        &facts_options,
        &emptied,
        true,
        raw_store(),
        None,
    )
    .unwrap();
    crate::save_json_file(&facts_path, &emptied).unwrap();
    emptied_write.complete().unwrap();
    let rollout = [crate::Input {
        root: crate::InputRoot::Codex,
        path: format!("archived_sessions/rollout-{codex_id}.jsonl"),
    }];
    let (retired, retired_batch, retired_model, _) =
        stage_publish(&mut reopened, &rollout, 1790812810000);
    assert!(
        withdrawal_fence_checked.get(),
        "withdrawal fence regression ran"
    );
    assert!(retired_model.prepared().rows.is_empty(), "zero-row batch");
    assert_eq!(
        retired_batch.runtime,
        crate::native_evidence::query_runtime_generation(&facts_options).unwrap()
    );
    assert_ne!(retired, caught);
    let codex_keys = prepared
        .rows
        .iter()
        .filter(|row| row.harness == "codex")
        .map(|row| row.key.clone())
        .collect::<Vec<_>>();
    assert!(!codex_keys.is_empty());
    let after = scoped(&retired, "");
    let kept_after = scoped(&retired, "scope=retained_history&");
    for key in &codex_keys {
        assert!(after.iter().all(|(other, _)| other != key), "{key} retired");
        assert!(
            kept_after.contains(&(key.clone(), true)),
            "{key} retained after zero-row publication"
        );
    }
    // Every other Current session is unchanged by the zero-row publication.
    assert_eq!(
        after.len() + codex_keys.len(),
        current.len(),
        "zero-row publication retires only the rollout owner"
    );
    // The withdrawn subagents stay Retained, unchanged, in the new generation.
    let still = withdrawn_rows(&retired);
    assert!(
        oracle_withdrawn.iter().all(|row| still.contains(row)),
        "withdrawn rows after zero-row publication"
    );

    // The public selected-upgrade entry must reach PG without opening SQLite.
    // These unchanged bulk owners have no retained composite to mask their
    // current recipe version. Ordinary producer completion follows the same
    // durable claims, while unrelated earlier work remains pending.
    let selected_key = "bulk169-000";
    super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &retired,
        |snapshot| {
            assert!(snapshot.owner(selected_key)?.is_none());
            assert!(snapshot.catalog(selected_key, false)?.is_some());
            Ok(())
        },
    )
    .unwrap();
    let selected_path = bulk[0]
        .full_path(&facts_options)
        .to_string_lossy()
        .into_owned();
    let unrelated_path = bulk[3]
        .full_path(&facts_options)
        .to_string_lossy()
        .into_owned();
    let mut journal = config.source_observations().unwrap();
    let empty: Vec<String> = Vec::new();
    let discover = crate::storage::source_observations::SourceOperation::capture(
        namespace.as_str(),
        "discover",
        &serde_json::to_vec(&(&empty, true)).unwrap(),
    );
    journal.discover(&discover, &empty, true).unwrap();
    let paths = vec![unrelated_path.clone(), selected_path.clone()];
    let expected_claims = journal.claims(&paths).unwrap();
    let invalidate = crate::storage::source_observations::SourceOperation::capture(
        namespace.as_str(),
        "invalidate",
        &serde_json::to_vec(&expected_claims).unwrap(),
    );
    journal.invalidate(&invalidate, &expected_claims).unwrap();
    let revisions = journal.claims(&paths).unwrap();
    // SelectedUpgrade intentionally hides FIFO while its marked claim exists.
    // Capture the complete fixture queue before installing that marker.
    let pending_before = journal.pending_catalog_sources(256, None).unwrap();
    assert_eq!(
        pending_before.tier,
        crate::storage::source_observations::CatalogPendingTier::Fifo
    );
    assert!(
        pending_before.paths.len() < 256,
        "fixture FIFO snapshot must be complete"
    );
    assert!(pending_before.paths.contains(&selected_path));
    assert!(pending_before.paths.contains(&unrelated_path));
    let expected_fifo_after = pending_before
        .paths
        .iter()
        .filter(|path| *path != &selected_path)
        .cloned()
        .collect::<Vec<_>>();
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        tx.driver.execute(
            "UPDATE semon_pg_native_catalog_recipe_headers SET version=5 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3",
            &[&namespace.as_str().as_bytes(), &retired.as_bytes(), &selected_key.as_bytes()],
        ).map_err(sqlstate::error)?;
        Ok(())
    }).unwrap();
    assert!(crate::request_session_projection_upgrade(&facts_options, selected_key).unwrap());
    let selected_pending = journal.pending_catalog_sources(64, None).unwrap();
    assert_eq!(
        selected_pending.tier,
        crate::storage::source_observations::CatalogPendingTier::SelectedUpgrade
    );
    assert_eq!(selected_pending.paths, vec![selected_path.clone()]);
    assert_eq!(
        journal.claims(&paths).unwrap(),
        revisions,
        "selection preserves producer CAS revisions"
    );
    // The existing ordinary semantic producer consumes the selected claim.
    let (upgraded, _, _, _) = stage_publish(&mut reopened, &bulk[..1], 1790812811000);
    assert_ne!(upgraded, retired);
    assert!(!crate::request_session_projection_upgrade(&facts_options, selected_key).unwrap());
    let pending_after = journal.pending_catalog_sources(256, None).unwrap();
    assert_eq!(
        pending_after.tier,
        crate::storage::source_observations::CatalogPendingTier::Fifo
    );
    assert_eq!(pending_after.paths, expected_fifo_after);
    let first_fifo_page = journal.pending_catalog_sources(64, None).unwrap();
    assert_eq!(
        first_fifo_page.tier,
        crate::storage::source_observations::CatalogPendingTier::Fifo
    );
    assert_eq!(
        first_fifo_page.paths,
        expected_fifo_after
            .iter()
            .take(64)
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(journal.claims(&[selected_path]).unwrap()[0].1, None);
    assert_eq!(journal.claims(&paths[..1]).unwrap(), revisions[..1]);
    // A retained-only owner remains readable from its publication proof when
    // the configured current Facts file has moved on. The same change refuses
    // Current selected rows above, where live runtime qualification applies.
    fs::write(
        &facts_path,
        b"changed current Facts after retained publication",
    )
    .unwrap();
    for &(all, since, at) in &retained_window_contexts {
        let mut retained_options = facts_options.clone();
        retained_options.all = all;
        retained_options.scan_window = false;
        retained_options.since = since;
        let mut retained_query =
            crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(
                vec![(namespace.as_str().into(), retained_options.clone())],
            )));
        let retained_runtime_store = super::super::snapshot::query::QueryCatalog {
            captured: false,
            pool: pool.clone(),
            namespace: namespace.clone(),
            control: control.clone(),
            generation: caught.clone(),
            options: retained_options,
        };
        for retained_key in &removed {
            let retained_fields = retained_query
                .call_bound_selected_at(
                    "get_session",
                    &serde_json::json!({"id":retained_key}),
                    at,
                    &retained_runtime_store,
                    &withdrawal_host,
                    None,
                )
                .unwrap();
            assert!(retained_fields["session"].is_object());
            assert_eq!(retained_fields["session"]["alive"], serde_json::Value::Null);
            assert_eq!(retained_fields["session"]["run"], serde_json::Value::Null);
            assert_eq!(
                retained_fields["session"]["open_question"],
                serde_json::Value::Null
            );
            let body = retained_query
                .call_bound_selected_at(
                    "read_transcript",
                    &serde_json::json!({"id":retained_key,"limit":100}),
                    at,
                    &retained_runtime_store,
                    &retained_host,
                    None,
                )
                .unwrap();
            assert_eq!(body["sid"], *retained_key);
            assert!(body.to_string().contains("sanitized agent answer"));
            assert_eq!(
                serde_json::json!({"body":body,"fields":retained_fields}),
                oracle_retained_windows[&(all, since.as_secs(), at, retained_key.clone())],
                "SQLite retained output-window parity all={all} since={since:?} at={at} {retained_key}"
            );
        }
    }
    // The zero-row publication retained the completed Codex child. Its old
    // last turn must not revive it after the output cutoff, unlike the
    // unfinished Claude spawns above.
    super::super::snapshot::with_admitted_snapshot(
        &pool,
        &namespace,
        &control,
        &retired,
        |snapshot| {
            assert!(snapshot.catalog(child_id, false)?.is_none());
            assert!(snapshot.owner(child_id)?.is_some());
            Ok(())
        },
    )
    .unwrap();
    let mut expired_options = facts_options.clone();
    expired_options.all = false;
    expired_options.scan_window = false;
    expired_options.since = Duration::from_secs(1);
    let mut expired_query =
        crate::Query::from_sources(std::sync::Arc::new(crate::SessionSources::with_machines(
            vec![(namespace.as_str().into(), expired_options.clone())],
        )));
    let expired_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: retired.clone(),
        options: expired_options,
    };
    for tool in ["get_session", "read_transcript"] {
        let error = expired_query
            .call_bound_selected_at(
                tool,
                &serde_json::json!({"id":child_id}),
                expired_window_at,
                &expired_store,
                &withdrawal_host,
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "unknown_session");
        assert_eq!(
            error, oracle_expired_reads[tool],
            "SQLite expired {tool} parity"
        );
    }
    // Live Current reads refuse after the canonical Facts bytes change.
    // Explicit captured reads of that same immutable generation retain their
    // independent proof and host custody.
    let captured_key = &facts_rows
        .iter()
        .find(|row| current.iter().any(|(key, _)| key == &row.key))
        .unwrap()
        .key;
    let live_current_store = super::super::snapshot::query::QueryCatalog {
        captured: false,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: caught.clone(),
        options: facts_options.clone(),
    };
    for tool in ["read_transcript", "get_session"] {
        let error = query
            .call_bound_selected_at(
                tool,
                &serde_json::json!({"id":captured_key}),
                1790812802000,
                &live_current_store,
                &withdrawal_host,
                None,
            )
            .unwrap_err();
        assert_eq!(
            (error.code, error.message.as_str()),
            (
                "unavailable",
                StorageError::ScopeChanged.to_string().as_str()
            ),
            "changed canonical Facts must refuse live Current {tool}"
        );
    }
    let captured_current_store = super::super::snapshot::query::QueryCatalog {
        captured: true,
        pool: pool.clone(),
        namespace: namespace.clone(),
        control: control.clone(),
        generation: caught.clone(),
        options: facts_options.clone(),
    };
    let captured_body = query
        .call_bound_selected_at(
            "read_transcript",
            &serde_json::json!({"id":captured_key}),
            1790812802000,
            &captured_current_store,
            &withdrawal_host,
            None,
        )
        .unwrap();
    assert_eq!(captured_body["sid"], *captured_key);
    assert!(
        !options.cache.exists(),
        "PG public request cannot create a SQLite cache"
    );
}

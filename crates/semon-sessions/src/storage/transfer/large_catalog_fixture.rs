//! Unchanged native 17-source fixture from 21f0186b; measurements precede record.
use super::{begin_active_catalog, cache, root, seed_catalog_input, sqlite};
use crate::storage::{StoreConfig, native_events::NativeEventStore};
use serde_json::json;

pub(crate) fn measure(
    config: &StoreConfig,
    root: &std::path::Path,
    verify_completed_withdrawal: impl FnOnce(&str, &str),
) {
    measure_mode(config, root, verify_completed_withdrawal, false);
}

pub(crate) fn measure_typed(
    config: &StoreConfig,
    root: &std::path::Path,
    verify_completed_withdrawal: impl FnOnce(&str, &str),
) {
    measure_mode(config, root, verify_completed_withdrawal, true);
}

pub(crate) fn measure_mode(
    config: &StoreConfig,
    root: &std::path::Path,
    verify_completed_withdrawal: impl FnOnce(&str, &str),
    typed: bool,
) {
    const CHILDREN: usize = 16;
    const USAGE_REPORTS: usize = 1000;
    let typed_refusal = typed.then(|| match &config.backend {
        crate::storage::StoreBackend::Sqlite { .. } => "semantic_context_budget".to_owned(),
        #[cfg(feature = "postgres")]
        crate::storage::StoreBackend::Postgres { .. } => {
            crate::storage::StorageError::Busy.to_string()
        }
    });
    let (options, initial_input) = seed_catalog_input(config, root);
    let initial_path = initial_input.full_path(&options);
    let mut initial_cache = cache(config);
    initial_cache
        .ingest(&"7".repeat(64), &initial_path, "claude")
        .unwrap();
    initial_cache.flush_cold_writes().unwrap();
    let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    begin_active_catalog(
        &mut initial_cache,
        &[initial_path.to_str().unwrap().to_owned()],
        &runtime,
        true,
    );
    let initial = finish_measured(
        config,
        &mut initial_cache,
        &options,
        std::slice::from_ref(&initial_input),
        typed_refusal.as_deref(),
    );
    assert_eq!(initial[0].repo.as_deref(), Some("facts-transfer-repo"));
    drop(initial_cache);
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
    // Withdraw the old path before the same native identity resumes under a
    // new path; same-path append legitimately clears history. Preserve the
    // first native prefix under its original host authority.
    let mut parent = std::fs::read_to_string(&old_path).unwrap();
    std::fs::remove_file(&old_path).unwrap();
    let mut cache = cache(config);
    let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    cache
        .retire_scoped_sources(
            &options,
            std::slice::from_ref(&old_input),
            &std::collections::BTreeSet::new(),
            &crate::facts::MachineFacts::Local,
        )
        .unwrap();
    verify_completed_withdrawal(&initial[0].key, old_path.to_str().unwrap());
    if matches!(&config.backend, crate::storage::StoreBackend::Sqlite { .. }) {
        begin_active_catalog(
            &mut cache,
            &[old_path.to_str().unwrap().to_owned()],
            &runtime,
            true,
        );
        assert!(
            finish_measured(
                config,
                &mut cache,
                &options,
                std::slice::from_ref(&old_input),
                typed_refusal.as_deref()
            )
            .is_empty()
        );
    }
    let mut inputs = vec![input];
    let children = (0..CHILDREN)
        .map(|index| format!("large-child-{index:02}"))
        .collect::<Vec<_>>();
    let timestamp = |second: usize| format!("2026-10-01T00:{:02}:{:02}Z", second / 60, second % 60);
    let usage_timestamp = |offset: usize| {
        let mut year = 2026;
        let mut month = 10;
        let mut day = 2 + offset;
        loop {
            let days_in_month = match month {
                2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            if day <= days_in_month {
                break;
            }
            day -= days_in_month;
            month += 1;
            if month == 13 {
                month = 1;
                year += 1;
            }
        }
        format!("{year:04}-{month:02}-{day:02}T00:00:00Z")
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
        let user_id = format!("{child}-u0");
        let assistant_id = format!("{child}-a0");
        for record in [
            json!({"type":"user","sessionId":"catalog","uuid":user_id,"parentUuid":null,"timestamp":timestamp(3),"cwd":"/sanitized","message":{"role":"user","content":"sanitized child task"}}),
            json!({"type":"assistant","sessionId":"catalog","uuid":assistant_id,"parentUuid":user_id,"timestamp":timestamp(4),"cwd":"/sanitized","message":{"id":assistant_id,"role":"assistant","model":"claude-test","content":[{"type":"text","text":format!("sanitized answer {index}")}]}}),
        ] {
            body.push_str(&format!("{record}\n"));
        }
        let mut previous = assistant_id;
        for day in 0..USAGE_REPORTS {
            let usage_uuid = format!("{child}-usage-record-{day:04}");
            let usage_id = format!("{child}-usage-message-{day:04}");
            let usage = json!({"type":"assistant","sessionId":"catalog","uuid":usage_uuid,"parentUuid":previous,"timestamp":usage_timestamp(day),"cwd":"/sanitized","message":{"id":usage_id,"role":"assistant","model":"claude-sonnet-4-5-20250929","content":[],"usage":{"input_tokens":410,"output_tokens":0}}});
            body.push_str(&format!("{usage}\n"));
            previous = usage_uuid;
        }
        // A child-owned native hand-back gives each canonical spawn its own
        // result preview owner instead of putting all results in the parent.
        let handback = json!({"type":"assistant","sessionId":"catalog","uuid":format!("{child}-handback"),"parentUuid":previous,"timestamp":usage_timestamp(USAGE_REPORTS),"cwd":"/sanitized","message":{"id":format!("{child}-handback"),"role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":format!("{child}-return"),"name":"SubagentHandback","input":{"message":format!("sanitized completed child {index}")}}]}});
        body.push_str(&format!("{handback}\n"));
        std::fs::write(&child_path, body).unwrap();
        std::fs::write(
            child_path.with_extension("meta.json"),
            json!({"agentType":"general-purpose","description":format!("sanitized task {index}"),"toolUseId":call}).to_string(),
        ).unwrap();
        inputs.push(child_input);
    }
    std::fs::write(&parent_path, parent).unwrap();
    let paths = inputs
        .iter()
        .map(|input| input.full_path(&options))
        .collect::<Vec<_>>();
    for (index, path) in paths.iter().enumerate() {
        cache
            .ingest(&format!("{:064x}", 1000 + index), path, "claude")
            .unwrap();
    }
    cache.flush_cold_writes().unwrap();
    let paths = paths
        .iter()
        .map(|path| path.to_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let runtime = crate::native_evidence::query_runtime_generation(&options).unwrap();
    begin_active_catalog(&mut cache, &paths, &runtime, true);
    let rows = finish_measured(
        config,
        &mut cache,
        &options,
        &inputs,
        typed_refusal.as_deref(),
    );
    assert_eq!(rows.len(), CHILDREN + 1);
    assert_eq!(
        rows.iter()
            .flat_map(|row| &row.sources)
            .map(|source| &source.path)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        CHILDREN + 1
    );
    assert_eq!(
        rows.iter().map(|row| row.cost.by_day.len()).sum::<usize>(),
        CHILDREN * USAGE_REPORTS
    );
}

fn finish_measured(
    config: &StoreConfig,
    cache: &mut crate::events::EventCache,
    options: &crate::Options,
    inputs: &[crate::Input],
    typed_refusal: Option<&str>,
) -> Vec<crate::model::summary::CatalogRow> {
    for _ in 0..256 {
        let batch = cache.semantic_batch(64).unwrap().unwrap();
        if batch.paths.is_empty() {
            continue;
        }
        let batch_inputs = batch
            .paths
            .iter()
            .map(|path| {
                inputs
                    .iter()
                    .find(|input| {
                        input.full_path(options).to_string_lossy().as_ref() == path.as_str()
                    })
                    .cloned()
                    .expect("each staged path is a declared native fixture input")
            })
            .collect::<Vec<_>>();
        if batch.paths.len() == 17 {
            eprintln!(
                "selected_large_fixture prepare_phase={:?} paths={}",
                batch.phase,
                batch.paths.len()
            );
        }
        let model = crate::model::prepare_semantic_batch(
            options,
            cache,
            &batch_inputs,
            &batch,
            1_790_000_000_000,
        )
        .unwrap_or_else(|error| {
            panic!(
                "native large fixture prepare phase={:?}: {error}",
                batch.phase
            )
        });
        if batch.paths.len() == 17 {
            let prepared = model.prepared();
            let mut cells = Vec::new();
            for row in &prepared.rows {
                let mut compact = row.clone();
                let payload =
                    crate::query_projection::SelectedRecipe::detach(&mut compact).unwrap();
                cells.push((
                    row.key.clone(),
                    serde_json::to_string(&compact).unwrap(),
                    payload,
                ));
            }
            let metadata_bytes = cells
                .iter()
                .map(|(_, metadata, _)| metadata.len())
                .sum::<usize>();
            let payload_bytes = cells
                .iter()
                .map(|(_, _, payload)| payload.as_ref().map_or(0, String::len))
                .sum::<usize>();
            let maximum_pair = cells
                .iter()
                .map(|(_, metadata, payload)| {
                    metadata.len() + payload.as_ref().map_or(0, String::len)
                })
                .max()
                .unwrap_or(0);
            let coordinator_bytes = serde_json::to_vec(&cells).unwrap().len();
            let witness = prepared.staged_model_witness().unwrap();
            let witness_bytes = witness.len();
            let witness: serde_json::Value = serde_json::from_str(&witness).unwrap();
            let witness = witness.as_array().unwrap();
            assert_eq!(witness.len(), 2);
            for (label, value) in ["projection", "presentation"].into_iter().zip(witness) {
                eprintln!(
                    "selected_large_fixture phase={:?} model_component={label} bytes={}",
                    batch.phase,
                    serde_json::to_vec(value).unwrap().len()
                );
            }
            let projection = witness[0].as_array().unwrap();
            let labels = [
                "slots",
                "trace_rows",
                "pids",
                "lineage_dependencies",
                "indexed_parents",
                "withdrawals",
                "endpoints",
                "relations",
                "message_dependencies",
                "retired_owners",
                "lifecycle_authority",
            ];
            assert_eq!(projection.len(), labels.len());
            for (label, value) in labels.into_iter().zip(projection) {
                eprintln!(
                    "selected_large_fixture phase={:?} projection_component={label} bytes={}",
                    batch.phase,
                    serde_json::to_vec(value).unwrap().len()
                );
            }
            eprintln!(
                "selected_large_fixture phase={:?} paths={} rows={} children=16 usage_reports_per_child=1000 cost_bytes={} cost_quote_count={} transcripts={} transcript_slots={} producer_metadata_bytes={} compact_metadata_bytes={} payload_bytes={} maximum_pair_bytes={} coordinator_bytes={} witness_bytes={} sqlite_context_bytes={}",
                batch.phase,
                batch.paths.len(),
                prepared.rows.len(),
                prepared
                    .rows
                    .iter()
                    .map(|row| serde_json::to_vec(&row.cost).unwrap().len())
                    .sum::<usize>(),
                prepared
                    .rows
                    .iter()
                    .map(|row| row.cost.by_day.len())
                    .sum::<usize>(),
                prepared.transcripts.len(),
                prepared
                    .transcripts
                    .iter()
                    .map(|projection| projection.slots.len())
                    .sum::<usize>(),
                prepared
                    .rows
                    .iter()
                    .map(|row| serde_json::to_vec(row).unwrap().len())
                    .sum::<usize>(),
                metadata_bytes,
                payload_bytes,
                maximum_pair,
                coordinator_bytes,
                witness_bytes,
                coordinator_bytes + witness_bytes
            );
            #[cfg(feature = "postgres")]
            eprintln!(
                "selected_large_fixture phase={:?} pg_model_witness_bytes={:?}",
                batch.phase,
                prepared
                    .staged_publication_witness()
                    .map(|witness| witness.len())
            );
            if batch.phase == crate::model::relation_stage::Phase::Publication {
                assert_eq!(prepared.rows.len(), 17);
                assert_eq!(
                    prepared
                        .rows
                        .iter()
                        .map(|row| row.cost.by_day.len())
                        .sum::<usize>(),
                    16000
                );
            }
        }
        let typed_publication = typed_refusal.is_some()
            && batch.paths.len() == 17
            && batch.phase == crate::model::relation_stage::Phase::Publication;
        if typed_publication {
            let semantic_bytes = crate::model::typed_witness::qualify(model.prepared()).unwrap();
            super::presentation_measurement::qualify(config, model.prepared(), semantic_bytes, &batch)
                .unwrap();
        }
        let before = cache.session_catalog_generation();
        let pending = if typed_publication {
            Some(cache.pending_catalog_sources(64).unwrap())
        } else {
            None
        };
        let recorded = cache.record_semantic_batch(&batch, model.prepared());
        if batch.paths.len() == 17 {
            eprintln!(
                "selected_large_fixture record_phase={:?} result={recorded:?} rejection={:?}",
                batch.phase,
                cache.semantic_rejection()
            );
        }
        if typed_publication {
            assert_eq!(recorded.unwrap_err().to_string(), typed_refusal.unwrap());
            assert_eq!(cache.session_catalog_generation(), before);
            assert_eq!(before, batch.base_generation);
            assert_eq!(cache.pending_catalog_sources(64).unwrap(), pending.unwrap());
            eprintln!(
                "typed_witness_production_refusal unchanged_head=true pending_claim_paths_unchanged=true publication_not_activated=true"
            );
            return model.prepared().rows.clone();
        }
        assert_eq!(recorded.unwrap(), crate::events::Outcome::Written);
        if batch.phase == crate::model::relation_stage::Phase::Publication {
            for _ in 0..256 {
                let result = cache.publish_semantic_catalog(&batch, model.prepared());
                if batch.paths.len() == 17 {
                    eprintln!("selected_large_fixture publish_result={result:?}");
                }
                match result.unwrap() {
                    (crate::events::Outcome::Deferred, None) => continue,
                    (crate::events::Outcome::Written, Some(_)) => {
                        cache.finish_semantic_stage(batch.epoch).unwrap();
                        return model.prepared().rows.clone();
                    }
                    other => panic!("native large fixture publication refused: {other:?}"),
                }
            }
            panic!("native large fixture custody did not finish");
        }
    }
    panic!("native large fixture did not reach Publication");
}

#[test]
fn selected_recipe_large_fixture_sqlite_measurement() {
    let root = root();
    let source = sqlite(&root, "selected-large-fixture");
    measure(&source, &root, |_, _| {});
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_witness_genuine_fixture_sqlite_reconstruction_and_refusal() {
    let root = root();
    let source = sqlite(&root, "typed-witness-fixture");
    measure_typed(&source, &root, |_, _| {});
    std::fs::remove_dir_all(root).unwrap();
}

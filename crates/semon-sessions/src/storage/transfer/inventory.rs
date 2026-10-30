//! Explicit current physical inventory. A new table must be classified, rather
//! than silently disappearing from a portable checkpoint.
use super::Category;

#[derive(Debug)]
pub(crate) enum Disposition {
    Authoritative(&'static [Category]),
    LocalOnly(&'static str),
    DrainReaders(&'static str),
}

pub(crate) fn sqlite(name: &str) -> Option<Disposition> {
    use Category::*;
    let categories: &'static [Category] = match name {
        "files" => &[Ledgers],
        "events" | "signals" => &[Events],
        "usage"
        | "usage_identity"
        | "codex_usage"
        | "native_usage_sources"
        | "native_usage_current"
        | "native_usage_facts"
        | "native_usage_identities"
        | "native_usage_dimensions" => &[Usage],
        "reported_runs" => &[ReportedFacts],
        "storage_proof_generations" => &[Proofs, Recipes],
        "storage_catalog_native_observation_grants" => {
            return Some(Disposition::LocalOnly(
                "Direct SQLite Catalog observation grants bind a destination-local admission and stream; they are minted only by its admitted publisher, never copied or inferred from transferred metadata.",
            ));
        }
        "storage_proof_presentations" => &[Metadata],
        "storage_proof_presentation_roots" | "storage_proof_refs" => &[ProofReferences],
        "storage_proof_sources" => &[Proofs],
        "storage_proof_leases" | "storage_proof_reader_owners" => {
            return Some(Disposition::DrainReaders(
                "Drain process-local readers and pin reachable proofs/floors; do not copy live leases or reader ownership as foreign authority.",
            ));
        }
        "meta" => &[
            Metadata,
            NativeHeaders,
            NativeSidecars,
            Custody,
            Generations,
            Proofs,
            ProofReferences,
            PageBlobs,
            PageGenerations,
            PageReferences,
            ChangeHeads,
            ChangeFloors,
            Checkpoints,
            ReportedFacts,
        ],
        "session_catalog" | "session_catalog_sources" => &[CatalogCurrent],
        "session_descriptions" | "session_overview" => &[Metadata],
        "session_slot_projections" => &[Recipes],
        "session_slots" => &[Slots],
        "session_history_catalog"
        | "session_history_projections"
        | "session_history_slots"
        | "session_history_slot_parts" => &[CatalogHistory, Recipes, Slots],
        "session_trace_headers"
        | "session_trace_turns"
        | "session_trace_edges"
        | "session_trace_bookmarks" => &[Trace],
        "session_search_coverage"
        | "session_search_documents"
        | "session_search_memberships"
        | "session_search_counts" => &[Search],
        "session_catalog_changes" => &[ChangeHeads, ChangeFloors],
        "session_catalog_invalidations" => &[Invalidations, Claims],
        "catalog_backfill_sources" => &[SourceMembers],
        "session_source_mutations" => &[Mutations],
        "catalog_relocation_candidates" => &[Relocations],
        "catalog_native_dependencies" => &[SourceState],
        "source_observation_intents" => &[SourceObservations, RawIntents],
        "source_observation_receipts" => &[Receipts, RawCompletions],
        "storage_event_operations" => &[Operations, RawIntents, RawCompletions],
        "storage_source_journal" => &[SourceState, Checkpoints],
        "storage_semantic_publications" => &[Operations, Generations],
        "storage_semantic_profiles" | "storage_semantic_model_batches" => &[StagedWork],
        "storage_namespaces" | "storage_namespace_sequence" => &[Generations],
        "catalog_semantic_stage"
        | "catalog_semantic_frontier"
        | "catalog_semantic_sources"
        | "catalog_semantic_inputs"
        | "catalog_semantic_keys"
        | "catalog_semantic_endpoints"
        | "catalog_semantic_native"
        | "catalog_semantic_owner_sources"
        | "catalog_semantic_paths"
        | "catalog_semantic_names"
        | "catalog_semantic_events"
        | "catalog_semantic_relations" => &[StagedWork, Claims],
        "native_tree_repairs"
        | "native_tree_generations"
        | "native_tree_usage_work"
        | "native_tree_codex_work"
        | "native_tree_headers"
        | "native_tree_models"
        | "native_tree_labels"
        | "native_tree_tools"
        | "native_tree_usage_models"
        | "native_tree_copied_identities" => &[Tree],
        "pricing_recompute_pending" => &[StagedWork],
        "storage_event_tables" | "storage_writer_inventory" => {
            return Some(Disposition::LocalOnly(
                "Physical writer qualification is re-established by the destination adapter, never copied as authority.",
            ));
        }
        _ => return None,
    };
    Some(Disposition::Authoritative(categories))
}

#[cfg(feature = "postgres")]
pub(crate) fn postgres(name: &str) -> Option<Disposition> {
    use Category::*;
    let categories: &'static [Category] = match name {
        "semon_pg_schema" | "semon_pg_store_owner" => {
            return Some(Disposition::LocalOnly(
                "Physical schema and database role ownership are provisioned by the authorized host, never imported.",
            ));
        }
        "semon_pg_store_state" | "semon_pg_store_operations" => &[Metadata, Operations],
        "semon_pg_operations" => &[Operations],
        "semon_pg_source_state" => &[SourceState],
        "semon_pg_source_members" => &[SourceMembers],
        "semon_pg_source_invalidations" => &[Invalidations, Claims],
        "semon_pg_source_receipts" => &[Receipts],
        "semon_pg_source_intents" => &[SourceObservations, RawIntents],
        _ => return native(name.strip_prefix("semon_pg_native_")?),
    };
    Some(Disposition::Authoritative(categories))
}

#[cfg(feature = "postgres")]
fn native(name: &str) -> Option<Disposition> {
    use Category::*;
    let categories: &'static [Category] = match name {
        "files" => &[Ledgers],
        "events" | "signals" => &[Events],
        "usage" | "usage_identity" | "codex_usage" | "usage_sources" | "usage_current"
        | "usage_facts" | "usage_identities" | "usage_dimensions" => &[Usage],
        "reported_runs" | "reported_stamp" => &[ReportedFacts],
        "meta" => &[
            Metadata,
            NativeHeaders,
            NativeSidecars,
            Custody,
            Generations,
            Proofs,
            ProofReferences,
            RawIntents,
            RawCompletions,
            Claims,
            StagedWork,
            Checkpoints,
        ],
        "descriptions" => &[Metadata],
        "semantic_publications" => &[Operations, Generations],
        "selected_roots" | "selected_refs" => &[ProofReferences],
        "selected_generations" | "selected_sources" => &[Proofs],
        "selected_presentations" => &[Metadata],
        "semantic_runtime" => &[Generations],
        "catalog_rows" | "catalog_sources" | "catalog_links" | "catalog_dependencies" => {
            &[CatalogCurrent]
        }
        "catalog_history_owners"
        | "catalog_history_headers"
        | "catalog_history_slots"
        | "catalog_history_sources"
        | "catalog_history_progress" => &[CatalogHistory],
        "catalog_recipe_headers" | "catalog_recipe_rows" => &[Recipes, Slots],
        "catalog_search_documents" | "catalog_search_memberships" | "catalog_search_counts" => {
            &[Search]
        }
        "catalog_search_headers" => {
            return Some(Disposition::LocalOnly(
                "Derived header search keys are rebuilt in the same destination transaction as the transferred native_codex_header metadata; authoritative header bytes travel as NativeMetadata in NativeHeaders.",
            ));
        }
        "catalog_trace_headers"
        | "catalog_trace_turns"
        | "catalog_trace_edges"
        | "catalog_trace_bookmarks" => &[Trace],
        "catalog_stage" | "catalog_stage_items" => &[StagedWork],
        "catalog_operations" => &[Operations],
        "catalog_proofs" => &[ProofReferences],
        "catalog_changes" => &[Generations],
        "catalog_leases" => {
            return Some(Disposition::DrainReaders(
                "Process reader leases cannot become live foreign readers. Drain and pin every reachable proof before final qualification.",
            ));
        }
        "page_blobs" => &[PageBlobs],
        "page_generations" => &[PageGenerations],
        "page_refs" | "page_current" => &[PageReferences],
        "change_feed" => &[ChangeHeads],
        "change_feed_state" => &[ChangeHeads, ChangeFloors],
        "source_mutations" => &[Mutations],
        "relocation_candidates" => &[Relocations],
        "native_dependencies" => &[SourceState],
        "recovery" => &[Checkpoints],
        "tree_generations"
        | "tree_usage_work"
        | "tree_codex_work"
        | "tree_headers"
        | "tree_models"
        | "tree_labels"
        | "tree_tools"
        | "tree_usage_models"
        | "tree_copied_identities"
        | "tree_repairs" => &[Tree],
        _ => return None,
    };
    Some(Disposition::Authoritative(categories))
}

pub(crate) fn require(disposition: Option<Disposition>, name: &str) {
    match disposition.unwrap_or_else(|| panic!("unclassified current transfer table: {name}")) {
        Disposition::Authoritative(categories) => assert!(!categories.is_empty()),
        Disposition::LocalOnly(reason) => assert!(!reason.is_empty()),
        Disposition::DrainReaders(reason) => assert!(!reason.is_empty()),
    }
}

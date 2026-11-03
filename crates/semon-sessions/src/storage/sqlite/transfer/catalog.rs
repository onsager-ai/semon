//! Current logical Catalog rows; all physical names remain private.
use super::*;
use crate::storage::transfer::{
    CatalogRowKind, TransferRecord, catalog_rows, reported::CaptureState,
};
use rusqlite::types::Value;

fn spec(kind: CatalogRowKind) -> (&'static str, &'static str, &'static [usize]) {
    use CatalogRowKind::*;
    match kind {
        PublicationReceipt => (
            "storage_semantic_publications",
            "generation,epoch,admission,base_generation,source_revision,proof_digest,claims,metadata",
            &[],
        ),
        PresentationRoot => (
            "storage_proof_presentation_roots",
            "publication,base,manifest,digest",
            &[],
        ),
        PresentationContext => (
            "storage_proof_presentation_contexts",
            "publication,digest,entry",
            &[],
        ),
        ProofGeneration => (
            "storage_proof_generations",
            "id,session_key,publication,proof,admission,stream,metadata,query_recipe,slots,prefix",
            &[],
        ),
        Presentation => (
            "storage_proof_presentations",
            "id,session_key,metadata",
            &[],
        ),
        ProofSource => ("storage_proof_sources", "id,session_key,path,ledger", &[]),
        ProofRef => ("storage_proof_refs", "session_key,kind,id", &[]),
        Current => ("session_catalog", "session_key,metadata,query_recipe", &[]),
        Recipe => (
            "session_slot_projections",
            "session_key,generation,source_generation,version,total",
            &[],
        ),
        Slot => ("session_slots", "session_key,slot,metadata", &[]),
        History => (
            "session_history_catalog",
            "session_key,metadata,query_recipe",
            &[],
        ),
        HistoryRecipe => (
            "session_history_projections",
            "session_key,generation,source_generation,version,total,incomplete",
            &[5],
        ),
        HistorySlot => ("session_history_slots", "session_key,slot,metadata", &[]),
        HistoryOriginal => (
            "(SELECT namespace,substr(key,25) AS session_key,value AS metadata FROM main.meta WHERE key>='history_original_header:' AND key<'history_original_header;')",
            "session_key,metadata",
            &[],
        ),
        TraceHeader => (
            "session_trace_headers",
            "session_key,generation,source_generation,metadata",
            &[],
        ),
        TraceTurn => (
            "session_trace_turns",
            "session_key,turn_id,start_id,metadata",
            &[],
        ),
        TraceEdge => ("session_trace_edges", "session_key,edge_id,metadata", &[]),
        TraceBookmark => ("session_trace_bookmarks", "session_key,target,edge_id", &[]),
        SearchMembership => (
            "session_search_memberships",
            "session_key,gram,current_last_ms,history_last_ms",
            &[],
        ),
        SearchCoverage => ("session_search_coverage", "read_scope,incomplete", &[]),
        SearchPublication => (
            SEARCH_PUBLICATION,
            "read_scope,version,search_generation,backfill_complete,incomplete",
            &[3],
        ),
        SearchDocument => (
            "session_search_documents",
            "read_scope,session_key,fields,complete",
            &[3],
        ),
        SearchCount => ("session_search_counts", "read_scope,gram,members", &[]),
        Dependency => (
            "catalog_native_dependencies",
            "source_path,harness,native_id",
            &[],
        ),
        Description => (
            "session_descriptions",
            "session_key,version,inputs,description",
            &[],
        ),
        HistoryProgress => (
            "(SELECT namespace,substr(key,18) AS session_key,value AS metadata FROM main.meta WHERE key>='history_transfer:' AND key<'history_transfer;')",
            "session_key,metadata",
            &[],
        ),
    }
}

// These are the actual published values, not values inferred from a Catalog head.
const SEARCH_PUBLICATION: &str = "(SELECT v.namespace,c.read_scope,v.value AS version,g.value AS search_generation,CAST(b.value AS INTEGER) AS backfill_complete,CAST(i.value AS INTEGER) AS incomplete FROM main.session_search_coverage c JOIN main.meta v ON v.namespace=c.namespace AND v.key='search_version' JOIN main.meta g ON g.namespace=c.namespace AND g.key='search_generation_'||c.read_scope JOIN main.meta b ON b.namespace=c.namespace AND b.key='search_backfill_complete' JOIN main.meta i ON i.namespace=c.namespace AND i.key='search_incomplete_'||c.read_scope)";
const STAGED_SEARCH_PUBLICATION: &str = "(SELECT namespace,json_extract(value,'$[0]') AS read_scope,json_extract(value,'$[1]') AS version,json_extract(value,'$[2]') AS search_generation,json_extract(value,'$[3]') AS backfill_complete,json_extract(value,'$[4]') AS incomplete FROM main.meta WHERE key IN ('native_transfer_v1:search-publication:current','native_transfer_v1:search-publication:retained_history'))";

#[derive(Debug)]
struct SearchPublication {
    scope: String,
    version: String,
    generation: String,
    backfill_complete: bool,
    incomplete: i64,
}

fn verify_search_publications(
    tx: &Inbox<'_>,
    generation: &str,
    publications: &[SearchPublication],
) -> Result<(), StorageError> {
    if publications.len() != 2
        || publications[0].scope != "current"
        || publications[1].scope != "retained_history"
    {
        return Err(StorageError::ScopeChanged);
    }
    let coverage_count: i64 =
        tx.0.query_row(
            "SELECT count(*) FROM main.session_search_coverage WHERE namespace=?1",
            [tx.1],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if coverage_count != 2 {
        return Err(StorageError::ScopeChanged);
    }
    for publication in publications {
        if publication.version != crate::catalog_search::VERSION
            || publication.generation != generation
            || !publication.backfill_complete
            || publication.incomplete < 0
        {
            return Err(StorageError::ScopeChanged);
        }
        let coverage: Option<(i64, i64)> = tx
            .0
            .query_row(
                "SELECT incomplete,(SELECT count(*) FROM main.session_search_documents d WHERE d.namespace=c.namespace AND d.read_scope=c.read_scope AND NOT d.complete) FROM main.session_search_coverage c WHERE c.namespace=?1 AND c.read_scope=?2",
                params![tx.1, publication.scope],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(failed)?;
        if coverage != Some((publication.incomplete, publication.incomplete)) {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(())
}

fn verify_search_content(tx: &Inbox<'_>) -> Result<(), StorageError> {
    let valid: bool = tx
        .0
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM main.session_catalog c WHERE c.namespace=?1 AND (c.lifecycle='current' OR EXISTS(SELECT 1 FROM main.session_history_catalog h WHERE h.namespace=c.namespace AND h.session_key=c.session_key)) AND (NOT EXISTS(SELECT 1 FROM main.session_search_documents d WHERE d.namespace=c.namespace AND d.session_key=c.session_key AND d.read_scope='retained_history') OR (c.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM main.session_search_documents d WHERE d.namespace=c.namespace AND d.session_key=c.session_key AND d.read_scope='current')))) AND NOT EXISTS(SELECT 1 FROM main.session_search_documents d WHERE d.namespace=?1 AND NOT EXISTS(SELECT 1 FROM main.session_catalog c WHERE c.namespace=d.namespace AND c.session_key=d.session_key AND (c.lifecycle='current' OR (d.read_scope='retained_history' AND EXISTS(SELECT 1 FROM main.session_history_catalog h WHERE h.namespace=c.namespace AND h.session_key=c.session_key))))) AND NOT EXISTS(SELECT 1 FROM main.session_search_memberships m WHERE m.namespace=?1 AND (m.history_last_ms IS NULL OR (m.current_last_ms IS NOT NULL AND m.current_last_ms<>m.history_last_ms) OR NOT EXISTS(SELECT 1 FROM main.session_search_documents d WHERE d.namespace=m.namespace AND d.session_key=m.session_key AND d.read_scope='retained_history' AND d.complete) OR ((m.current_last_ms IS NOT NULL) <> EXISTS(SELECT 1 FROM main.session_search_documents d WHERE d.namespace=m.namespace AND d.session_key=m.session_key AND d.read_scope='current'))))",
            [tx.1],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if !valid {
        return Err(StorageError::ScopeChanged);
    }
    let mismatch: bool = tx
        .0
        .query_row(
            "WITH physical AS (SELECT 'current' AS read_scope,gram,count(*) AS members FROM main.session_search_memberships WHERE namespace=?1 AND current_last_ms IS NOT NULL GROUP BY gram UNION ALL SELECT 'retained_history',gram,count(*) FROM main.session_search_memberships WHERE namespace=?1 AND history_last_ms IS NOT NULL GROUP BY gram), stored AS (SELECT read_scope,gram,members FROM main.session_search_counts WHERE namespace=?1) SELECT EXISTS(SELECT * FROM physical EXCEPT SELECT * FROM stored) OR EXISTS(SELECT * FROM stored EXCEPT SELECT * FROM physical)",
            [tx.1],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if mismatch {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

pub(super) fn verify_search_state(tx: &Inbox<'_>, generation: &str) -> Result<(), StorageError> {
    let valid: bool = tx.0.query_row("SELECT (SELECT count(*) FROM main.session_search_coverage WHERE namespace=?1)=2 AND (SELECT count(*) FROM main.meta WHERE namespace=?1 AND key IN ('search_version','search_backfill_complete','search_generation_current','search_generation_retained_history','search_incomplete_current','search_incomplete_retained_history'))=6 AND EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key='search_version' AND value=?3) AND EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key='search_backfill_complete' AND value='1') AND NOT EXISTS(SELECT 1 FROM main.session_search_coverage c WHERE c.namespace=?1 AND (c.incomplete<>(SELECT count(*) FROM main.session_search_documents d WHERE d.namespace=c.namespace AND d.read_scope=c.read_scope AND NOT d.complete) OR NOT EXISTS(SELECT 1 FROM main.meta m WHERE m.namespace=c.namespace AND m.key='search_incomplete_'||c.read_scope AND m.value=CAST(c.incomplete AS TEXT)) OR NOT EXISTS(SELECT 1 FROM main.meta m WHERE m.namespace=c.namespace AND m.key='search_generation_'||c.read_scope AND m.value=?2)))", params![tx.1,generation,crate::catalog_search::VERSION], |row| row.get(0)).map_err(failed)?;
    if !valid {
        return Err(StorageError::ScopeChanged);
    }
    let publications = tx
        .0
        .prepare(&format!(
            "SELECT read_scope,version,search_generation,backfill_complete,incomplete FROM {SEARCH_PUBLICATION} WHERE namespace=?1 ORDER BY read_scope"
        ))
        .map_err(failed)?
        .query_map([tx.1], |row| {
            Ok(SearchPublication {
                scope: row.get(0)?,
                version: row.get(1)?,
                generation: row.get(2)?,
                backfill_complete: row.get(3)?,
                incomplete: row.get(4)?,
            })
        })
        .map_err(failed)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(failed)?;
    verify_search_publications(tx, generation, &publications)?;
    verify_search_content(tx)
}

fn verify_imported_search_state(tx: &mut Inbox<'_>, generation: &str) -> Result<(), StorageError> {
    let staged_count: i64 = tx
        .0
        .query_row(
            "SELECT count(*) FROM main.meta WHERE namespace=?1 AND key GLOB 'native_transfer_v1:search-publication:*'",
            [tx.1],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if staged_count != 2 {
        return Err(StorageError::ScopeChanged);
    }
    let mut publications = Vec::with_capacity(2);
    for scope in ["current", "retained_history"] {
        let bytes = tx
            .get(
                &format!("native_transfer_v1:search-publication:{scope}"),
                4096,
            )?
            .ok_or(StorageError::ScopeChanged)?;
        let values: Vec<serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        catalog_rows::validate(&TransferRecord::CatalogRow {
            generation: generation.to_owned(),
            kind: CatalogRowKind::SearchPublication,
            values: values.clone(),
        })?;
        if values.first().and_then(serde_json::Value::as_str) != Some(scope) {
            return Err(StorageError::ScopeChanged);
        }
        publications.push(SearchPublication {
            scope: scope.to_owned(),
            version: values[1]
                .as_str()
                .ok_or(StorageError::ScopeChanged)?
                .to_owned(),
            generation: values[2]
                .as_str()
                .ok_or(StorageError::ScopeChanged)?
                .to_owned(),
            backfill_complete: values[3].as_bool().ok_or(StorageError::ScopeChanged)?,
            incomplete: values[4].as_i64().ok_or(StorageError::ScopeChanged)?,
        });
    }
    verify_search_publications(tx, generation, &publications)?;
    verify_search_content(tx)
}

/// Keep incoming readiness private until the same transaction publishes its head.
pub(super) fn publish_search_state(
    tx: &mut Inbox<'_>,
    generation: &str,
) -> Result<(), StorageError> {
    for scope in ["current", "retained_history"] {
        let bytes = tx
            .get(
                &format!("native_transfer_v1:search-publication:{scope}"),
                4096,
            )?
            .ok_or(StorageError::ScopeChanged)?;
        let values: Vec<serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        catalog_rows::validate(&TransferRecord::CatalogRow {
            generation: generation.to_owned(),
            kind: CatalogRowKind::SearchPublication,
            values: values.clone(),
        })?;
        if values[0].as_str() != Some(scope) {
            return Err(StorageError::ScopeChanged);
        }
        for (key, value) in [
            (
                "search_version".to_owned(),
                values[1]
                    .as_str()
                    .ok_or(StorageError::ScopeChanged)?
                    .to_owned(),
            ),
            ("search_backfill_complete".to_owned(), "1".to_owned()),
            (
                format!("search_generation_{scope}"),
                values[2]
                    .as_str()
                    .ok_or(StorageError::ScopeChanged)?
                    .to_owned(),
            ),
            (
                format!("search_incomplete_{scope}"),
                values[4]
                    .as_i64()
                    .ok_or(StorageError::ScopeChanged)?
                    .to_string(),
            ),
        ] {
            if tx.get(&key, 64)?.is_some_and(|old| old != value.as_bytes()) {
                return Err(StorageError::Conflict);
            }
            tx.put(&key, value.as_bytes())?;
        }
    }
    verify_search_state(tx, generation)
}

/// Refuse all already-owned Catalog content before applying the first page.
/// A later unique-key failure is too late: earlier pages would already commit.
pub(super) fn occupied(tx: &Inbox<'_>) -> Result<bool, StorageError> {
    let mut terms = CatalogRowKind::ALL
        .iter()
        .map(|kind| {
            let (table, _, _) = spec(*kind);
            let table = if table.starts_with('(') {
                table.to_owned()
            } else {
                format!("main.{table}")
            };
            format!("EXISTS(SELECT 1 FROM {table} WHERE namespace=?1)")
        })
        .collect::<Vec<_>>();
    for table in [
        "session_catalog_sources",
        "session_history_slot_parts",
        "session_overview",
    ] {
        terms.push(format!(
            "EXISTS(SELECT 1 FROM main.{table} WHERE namespace=?1)"
        ));
    }
    terms.push("EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key IN ('search_version','search_backfill_complete','search_generation_current','search_generation_retained_history','search_incomplete_current','search_incomplete_retained_history'))".to_owned());
    tx.0.query_row(&format!("SELECT {}", terms.join(" OR ")), [tx.1], |row| {
        row.get(0)
    })
    .map_err(failed)
}

fn sql_value(value: &serde_json::Value) -> Result<Value, StorageError> {
    Ok(match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(v) => Value::Integer(i64::from(*v)),
        serde_json::Value::Number(v) => {
            Value::Integer(v.as_i64().ok_or(StorageError::ScopeChanged)?)
        }
        serde_json::Value::String(v) => Value::Text(v.clone()),
        _ => return Err(StorageError::ScopeChanged),
    })
}

fn verify_authority(
    tx: &Inbox<'_>,
    kind: CatalogRowKind,
    values: &[serde_json::Value],
    verification_bytes: &mut usize,
) -> Result<(), StorageError> {
    use CatalogRowKind::*;
    let text = |i: usize| values[i].as_str().ok_or(StorageError::ScopeChanged);
    match kind {
        PresentationRoot => {
            let exists: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM storage_semantic_publications WHERE namespace=?1 AND generation=?2 AND proof_digest=?3)", params![tx.1,text(0)?,text(3)?], |r| r.get(0)).map_err(failed)?;
            if !exists {
                return Err(StorageError::ScopeChanged);
            }
        }
        // Canonical bytes and digest are checked by catalog_rows::validate.
        // Exact digest-set membership is authenticated once per publication by
        // verify_context_stores before the imported catalog can be admitted.
        PresentationContext => {}
        ProofGeneration => {
            let record = catalog_rows::proof_generation(tx.1, values)?;
            let exists: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM storage_semantic_publications p JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest WHERE p.namespace=?1 AND p.generation=?2 AND p.proof_digest=?3 AND p.admission=?4 AND EXISTS(SELECT 1 FROM json_each(p.metadata) WHERE value=?5))", params![tx.1,record.publication,record.proof,record.admission as i64,record.metadata], |r| r.get(0)).map_err(failed)?;
            if !exists {
                return Err(StorageError::ScopeChanged);
            }
        }
        Presentation => {
            let (metadata,slots,manifest): (Option<String>,Option<String>,Option<String>) = tx.0.query_row("SELECT CASE WHEN octet_length(g.metadata)<=1048576 AND octet_length(g.metadata)+octet_length(g.slots)+octet_length(r.manifest)<=?4 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END FROM storage_proof_generations g JOIN storage_proof_presentation_roots r ON r.namespace=g.namespace AND r.publication=g.publication AND r.digest=g.proof WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3", params![tx.1,text(0)?,text(1)?,*verification_bytes as i64], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
            let metadata = metadata.ok_or(StorageError::Busy)?;
            let slots = slots.ok_or(StorageError::Busy)?;
            let manifest = manifest.ok_or(StorageError::Busy)?;
            *verification_bytes = verification_bytes
                .checked_sub(metadata.len() + slots.len() + manifest.len())
                .ok_or(StorageError::Busy)?;
            crate::storage::proof_codec::verify_presentation_references(
                text(1)?,
                &metadata,
                &slots,
                text(2)?,
                &manifest,
            )?;
        }
        ProofSource => {
            let metadata: String = tx.0.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3", params![tx.1,text(0)?,text(1)?], |r| r.get(0)).map_err(failed)?;
            catalog_rows::verify_source_edge(&metadata, text(2)?, text(3)?)?;
            super::pages::verify_backing(tx, &metadata, text(2)?, text(3)?)?;
        }
        ProofRef => {
            let (metadata,count): (String,i64) = tx.0.query_row("SELECT CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,(SELECT count(*) FROM storage_proof_sources s WHERE s.namespace=g.namespace AND s.id=g.id AND s.session_key=g.session_key) FROM storage_proof_generations g JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3", params![tx.1,text(2)?,text(0)?], |r| Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
            let row: crate::model::summary::CatalogRow =
                serde_json::from_str(&metadata).map_err(|_| StorageError::ScopeChanged)?;
            if usize::try_from(count).ok() != Some(row.sources.len()) {
                return Err(StorageError::ScopeChanged);
            }
        }
        SearchDocument => {
            let scope = text(0)?;
            let metadata: String = tx.0.query_row("SELECT CASE WHEN octet_length(metadata)<=?3 THEN metadata END FROM main.session_catalog WHERE namespace=?1 AND session_key=?2", params![tx.1,text(1)?,*verification_bytes as i64], |r| r.get::<_,Option<String>>(0)).map_err(failed)?.ok_or(StorageError::Busy)?;
            *verification_bytes = verification_bytes
                .checked_sub(metadata.len())
                .ok_or(StorageError::Busy)?;
            let column = if scope == "current" {
                "current_last_ms"
            } else {
                "history_last_ms"
            };
            let posting_bytes: i64 = tx.0.query_row(&format!("SELECT coalesce(sum(octet_length(gram)+8),0) FROM (SELECT gram FROM main.session_search_memberships WHERE namespace=?1 AND session_key=?2 AND {column} IS NOT NULL ORDER BY gram LIMIT 1025)"), params![tx.1,text(1)?], |r| r.get(0)).map_err(failed)?;
            *verification_bytes = verification_bytes
                .checked_sub(
                    usize::try_from(posting_bytes).map_err(|_| StorageError::ScopeChanged)?,
                )
                .ok_or(StorageError::Busy)?;
            let postings: Vec<(String,i64)> = tx.0.prepare(&format!("SELECT gram,{column} FROM main.session_search_memberships WHERE namespace=?1 AND session_key=?2 AND {column} IS NOT NULL ORDER BY gram LIMIT 1025")).map_err(failed)?.query_map(params![tx.1,text(1)?], |r| Ok((r.get(0)?,r.get(1)?))).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
            catalog_rows::verify_search_document(
                &metadata,
                text(1)?,
                text(2)?,
                values[3].as_bool().ok_or(StorageError::ScopeChanged)?,
                postings,
            )?;
        }
        _ => {}
    }
    Ok(())
}

/// Final admission authenticates complete stores, including unreferenced entries.
pub(super) fn verify_context_stores(tx: &Inbox<'_>) -> Result<(), StorageError> {
    let orphan: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM storage_proof_presentation_contexts c WHERE c.namespace=?1 AND NOT EXISTS(SELECT 1 FROM storage_proof_presentation_roots r WHERE r.namespace=c.namespace AND r.publication=c.publication))", [tx.1], |r| r.get(0)).map_err(failed)?;
    if orphan {
        return Err(StorageError::ScopeChanged);
    }
    let mut after = String::new();
    loop {
        let root: Option<(String,Option<String>)> = tx.0.query_row("SELECT digest,CASE WHEN octet_length(manifest)<=1048576 THEN manifest END FROM storage_proof_presentation_roots WHERE namespace=?1 AND digest>?2 ORDER BY digest LIMIT 1", params![tx.1,after], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(failed)?;
        let Some((digest, manifest)) = root else {
            break;
        };
        let manifest = manifest.ok_or(StorageError::HistoryRetentionBounds)?;
        let publication: String = tx.0.query_row("SELECT publication FROM storage_proof_presentation_roots WHERE namespace=?1 AND digest=?2", params![tx.1,digest], |r| r.get(0)).map_err(failed)?;
        let decoded = crate::storage::proof_codec::decode_presentation_manifest(&manifest)?;
        let (row_bytes, largest): (i64,i64) = tx.0.query_row("SELECT COALESCE(sum(octet_length(p.metadata)),0),COALESCE(max(max(octet_length(g.metadata),octet_length(g.slots))),0) FROM storage_proof_presentations p JOIN storage_proof_generations g USING(namespace,id,session_key) WHERE g.namespace=?1 AND g.publication=?2", params![tx.1,publication], |r| Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
        if largest > 1048576 {
            return Err(StorageError::ScopeChanged);
        }
        let contexts = super::super::proof_generations::presentation_contexts(
            tx.0,
            tx.1,
            &publication,
            &decoded,
            |bytes| {
                if usize::try_from(row_bytes)
                    .ok()
                    .and_then(|rows| rows.checked_add(bytes))
                    .is_none_or(|total| total > 8 * 1048576)
                {
                    return Err(StorageError::HistoryRetentionBounds);
                }
                Ok(())
            },
        )?;
        let mut rows = tx.0.prepare("SELECT g.session_key,CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,CASE WHEN octet_length(p.metadata)<=8388608 THEN p.metadata END FROM storage_proof_generations g JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE g.namespace=?1 AND g.publication=?2").map_err(failed)?;
        let mut rows = rows.query(params![tx.1, publication]).map_err(failed)?;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(row) = rows.next().map_err(failed)? {
            if !seen.insert(row.get::<_, String>(0).map_err(failed)?) {
                return Err(StorageError::ScopeChanged);
            }
            let metadata = row
                .get::<_, Option<String>>(1)
                .map_err(failed)?
                .ok_or(StorageError::HistoryRetentionBounds)?;
            let slots = row
                .get::<_, Option<String>>(2)
                .map_err(failed)?
                .ok_or(StorageError::HistoryRetentionBounds)?;
            let presentation = row
                .get::<_, Option<String>>(3)
                .map_err(failed)?
                .ok_or(StorageError::HistoryRetentionBounds)?;
            crate::storage::proof_codec::verify_presentation_references(
                &row.get::<_, String>(0).map_err(failed)?,
                &metadata,
                &slots,
                &presentation,
                &manifest,
            )?;
            crate::storage::proof_codec::resolve_presentation(&presentation, &contexts)?;
        }
        after = digest;
    }
    Ok(())
}

pub(super) fn capture(
    tx: &mut Inbox<'_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    let index = usize::from(state.native_family - 32);
    let Some(kind) = CatalogRowKind::ALL.get(index).copied() else {
        if let Some(generation) = tx.get("catalog_generation", 64)? {
            verify_search_state(
                tx,
                std::str::from_utf8(&generation).map_err(|_| StorageError::ScopeChanged)?,
            )?;
        } else if let Some(generation) = tx.get(catalog_rows::IMPORTED_GENERATION, 64)? {
            verify_imported_search_state(
                tx,
                std::str::from_utf8(&generation).map_err(|_| StorageError::ScopeChanged)?,
            )?;
        }
        transfer::reported::finish_capture(tx, state)?;
        return Ok(state.progress());
    };
    let published_generation: Option<String> =
        tx.0.query_row(
            "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_generation'",
            [tx.1],
            |r| r.get(0),
        )
        .optional()
        .map_err(failed)?;
    let imported_source = published_generation.is_none();
    let generation = match published_generation {
        Some(generation) => Some(generation),
        None => tx
            .get(catalog_rows::IMPORTED_GENERATION, 64)?
            .map(|bytes| String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged))
            .transpose()?,
    };
    let Some(generation) = generation else {
        // Absence of a publication is not an empty replacement publication.
        let occupied: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM main.session_catalog WHERE namespace=?1) OR EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=?1)", [tx.1], |r| r.get(0)).map_err(failed)?;
        if occupied {
            return Err(StorageError::ScopeChanged);
        }
        transfer::reported::finish_capture(tx, state)?;
        return Ok(state.progress());
    };
    let imported_search_publication = kind == CatalogRowKind::SearchPublication && imported_source;
    if imported_search_publication {
        verify_imported_search_state(tx, &generation)?;
    }
    let (table, columns, bools) = spec(kind);
    let names = columns.split(',').collect::<Vec<_>>();
    let keys = kind
        .keys()
        .iter()
        .map(|i| names[*i])
        .collect::<Vec<_>>()
        .join(",");
    let mut binds = vec![Value::Text(tx.1.into())];
    let after = if let Some(cursor) = &state.native_key {
        let cursor: Vec<serde_json::Value> =
            serde_json::from_str(cursor).map_err(|_| StorageError::ScopeChanged)?;
        if cursor.len() != kind.keys().len() {
            return Err(StorageError::ScopeChanged);
        }
        for value in cursor {
            binds.push(sql_value(&value)?);
        }
        format!(
            " AND ({keys})>({})",
            (2..binds.len() + 1)
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        String::new()
    };
    let current = if kind == CatalogRowKind::Current {
        " AND lifecycle='current'"
    } else {
        ""
    };
    let table = if imported_search_publication {
        STAGED_SEARCH_PUBLICATION.to_owned()
    } else if table.starts_with('(') {
        table.to_owned()
    } else {
        format!("main.{table}")
    };
    let suffix = format!("FROM {table} WHERE namespace=?1{current}{after} ORDER BY {keys}");
    let size = names
        .iter()
        .map(|n| format!("coalesce(octet_length({n}),0)"))
        .collect::<Vec<_>>()
        .join("+");
    let size = if kind == CatalogRowKind::SearchDocument {
        format!(
            "{size}+coalesce((SELECT octet_length(c.metadata) FROM main.session_catalog c WHERE c.namespace=?1 AND c.session_key=session_search_documents.session_key),0)+coalesce((SELECT sum(octet_length(m.gram)+8) FROM main.session_search_memberships m WHERE m.namespace=?1 AND m.session_key=session_search_documents.session_key AND CASE WHEN session_search_documents.read_scope='current' THEN m.current_last_ms IS NOT NULL ELSE m.history_last_ms IS NOT NULL END),0)"
        )
    } else {
        size
    };
    let sizes =
        tx.0.prepare(&format!("SELECT {size} {suffix} LIMIT 64"))
            .map_err(failed)?
            .query_map(rusqlite::params_from_iter(&binds), |r| r.get::<_, i64>(0))
            .map_err(failed)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed)?;
    let mut count = 0;
    let mut bytes = 0usize;
    for size in &sizes {
        let size = usize::try_from(*size).map_err(|_| StorageError::ScopeChanged)?;
        if bytes
            .checked_add(size)
            .is_none_or(|n| n > transfer::MAX_PAGE_BYTES)
        {
            break;
        }
        bytes += size;
        count += 1;
    }
    if count == 0 && !sizes.is_empty() {
        return Err(StorageError::Busy);
    }
    let rows =
        tx.0.prepare(&format!("SELECT {columns} {suffix} LIMIT {count}"))
            .map_err(failed)?
            .query_map(rusqlite::params_from_iter(&binds), |r| {
                crate::events::store::native_decode::values(r, bools)
            })
            .map_err(failed)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed)?;
    let mut verification_bytes = transfer::MAX_PAGE_BYTES - 4096;
    if rows.is_empty() {
        state.native_family += 1;
        state.native_key = None;
    } else {
        let mut records = Vec::new();
        for row in rows {
            let record = TransferRecord::CatalogRow {
                generation: generation.clone(),
                kind,
                values: row.0,
            };
            let size = serde_json::to_vec(&record)
                .map_err(|_| StorageError::ScopeChanged)?
                .len()
                + 1;
            if verification_bytes < size {
                break;
            }
            verification_bytes -= size;
            let TransferRecord::CatalogRow { values, .. } = &record else {
                unreachable!()
            };
            match verify_authority(tx, kind, values, &mut verification_bytes) {
                Err(StorageError::Busy) if !records.is_empty() => break,
                result => result?,
            }
            records.push(record);
        }
        catalog_rows::append(tx, state, records)?;
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply(
    tx: &mut Inbox<'_>,
    record: &TransferRecord,
    verification_bytes: &mut usize,
) -> Result<(), StorageError> {
    catalog_rows::validate(record)?;
    let TransferRecord::CatalogRow {
        generation,
        kind,
        values,
    } = record
    else {
        return Err(StorageError::InvalidArguments);
    };
    catalog_rows::bind_imported_generation(tx, generation)?;
    verify_authority(tx, *kind, values, verification_bytes)?;
    catalog_rows::bind_imported_publication(tx, generation, *kind, values)?;
    if *kind == CatalogRowKind::SearchPublication {
        let scope = values[0].as_str().ok_or(StorageError::ScopeChanged)?;
        let key = format!("native_transfer_v1:search-publication:{scope}");
        let bytes = serde_json::to_vec(values).map_err(|_| StorageError::ScopeChanged)?;
        if tx.get(&key, 4096)?.is_some() {
            return Err(StorageError::Conflict);
        }
        return tx.put(&key, &bytes);
    }
    if matches!(
        kind,
        CatalogRowKind::HistoryOriginal | CatalogRowKind::HistoryProgress
    ) {
        return tx.put(
            &format!(
                "{}{}",
                if *kind == CatalogRowKind::HistoryOriginal {
                    "history_original_header:"
                } else {
                    "history_transfer:"
                },
                values[0].as_str().ok_or(StorageError::ScopeChanged)?
            ),
            values[1]
                .as_str()
                .ok_or(StorageError::ScopeChanged)?
                .as_bytes(),
        );
    }
    let (table, columns, _) = spec(*kind);
    let (columns, values) = if *kind == CatalogRowKind::Current {
        let metadata = values[1].as_str().ok_or(StorageError::ScopeChanged)?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
        (
            "session_key,metadata,query_recipe,lifecycle,last_ms,harness,repo,parent_key",
            vec![
                values[0].clone(),
                values[1].clone(),
                values[2].clone(),
                row.lifecycle.as_str().into(),
                row.last.unwrap_or(0).into(),
                row.harness.into(),
                row.repo.into(),
                row.parent.into(),
            ],
        )
    } else if *kind == CatalogRowKind::History {
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(values[1].as_str().ok_or(StorageError::ScopeChanged)?)
                .map_err(|_| StorageError::ScopeChanged)?;
        let (activity, active) = crate::storage::query::history_activity(&row);
        (
            "session_key,metadata,query_recipe,query_activity,query_active",
            vec![
                values[0].clone(),
                values[1].clone(),
                values[2].clone(),
                activity.into(),
                active.into(),
            ],
        )
    } else {
        (columns, values.clone())
    };
    let mut binds = vec![Value::Text(tx.1.into())];
    for value in &values {
        binds.push(sql_value(value)?);
    }
    let slots = (1..=binds.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    tx.0.execute(
        &format!("INSERT INTO main.{table}(namespace,{columns}) VALUES({slots})"),
        rusqlite::params_from_iter(binds),
    )
    .map_err(failed)?;
    if *kind == CatalogRowKind::History {
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(values[1].as_str().ok_or(StorageError::ScopeChanged)?)
                .map_err(|_| StorageError::ScopeChanged)?;
        // Retained-only sessions still need the normal SQLite listing owner;
        // an existing Current row for a composite keeps its own observation.
        tx.0.execute("INSERT INTO main.session_catalog(namespace,session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) VALUES(?1,?2,'retained',?3,?4,?5,?6,?7) ON CONFLICT(namespace,session_key) DO NOTHING", params![tx.1,row.key,row.last.unwrap_or(0),row.harness,row.repo,row.parent,values[1].as_str()]).map_err(failed)?;
    }
    if *kind == CatalogRowKind::Current {
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(values[1].as_str().ok_or(StorageError::ScopeChanged)?)
                .map_err(|_| StorageError::ScopeChanged)?;
        for source in &row.sources {
            tx.0.execute("INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES(?1,?2,?3,?4,?5,?6)", params![tx.1,row.key,row.lifecycle.as_str(),row.harness,source.native_id,source.path.to_str().ok_or(StorageError::ScopeChanged)?]).map_err(failed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_transfer225_search_bounded_document_and_state_refusals() {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(super::super::super::search::SCHEMA)
            .unwrap();
        db.execute_batch("CREATE TABLE session_catalog(namespace TEXT,session_key TEXT,lifecycle TEXT,metadata TEXT); CREATE TABLE session_history_catalog(namespace TEXT,session_key TEXT); CREATE TABLE meta(namespace TEXT,key TEXT,value TEXT,PRIMARY KEY(namespace,key));").unwrap();
        let row: crate::model::summary::CatalogRow = serde_json::from_value(serde_json::json!({"key":"key","name":"search 漢字","harness":"claude","kind":"session","native_ids":[],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":"model","effort":null,"start":null,"last":7,"tokens":[0.0,0.0,0.0],"cost":crate::pricing::Cost::default(),"sources":[]})).unwrap();
        let metadata = serde_json::to_string(&row).unwrap();
        let (fields, grams, complete) = crate::catalog_search::prepared_fields_port(&row).unwrap();
        let tx = db.transaction().unwrap();
        tx.execute(
            "INSERT INTO session_catalog VALUES('ns','key','current',?1)",
            [&metadata],
        )
        .unwrap();
        for gram in &grams {
            tx.execute(
                "INSERT INTO session_search_memberships VALUES('ns','key',?1,7,7)",
                [gram],
            )
            .unwrap();
        }
        let values = vec![
            "current".into(),
            "key".into(),
            fields.clone().into(),
            complete.into(),
        ];
        let mut inbox = Inbox(&tx, "ns");
        let exact = metadata.len() + grams.iter().map(|g| g.len() + 8).sum::<usize>();
        let mut budget = exact;
        verify_authority(&inbox, CatalogRowKind::SearchDocument, &values, &mut budget).unwrap();
        assert_eq!(budget, 0);
        let mut short = exact - 1;
        assert!(matches!(
            verify_authority(&inbox, CatalogRowKind::SearchDocument, &values, &mut short),
            Err(StorageError::Busy)
        ));
        let mut no_metadata = metadata.len() - 1;
        assert!(matches!(
            verify_authority(
                &inbox,
                CatalogRowKind::SearchDocument,
                &values,
                &mut no_metadata
            ),
            Err(StorageError::Busy)
        ));
        let generation = "a".repeat(64);
        for scope in ["current", "retained_history"] {
            tx.execute(
                "INSERT INTO session_search_documents VALUES('ns',?1,'key',?2,1)",
                params![scope, fields],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO session_search_coverage VALUES('ns',?1,0)",
                [scope],
            )
            .unwrap();
            for gram in &grams {
                tx.execute(
                    "INSERT INTO session_search_counts VALUES('ns',?1,?2,1)",
                    params![scope, gram],
                )
                .unwrap();
            }
            let record = TransferRecord::CatalogRow {
                generation: generation.clone(),
                kind: CatalogRowKind::SearchPublication,
                values: vec![
                    scope.into(),
                    crate::catalog_search::VERSION.into(),
                    generation.clone().into(),
                    true.into(),
                    0.into(),
                ],
            };
            let mut budget = transfer::MAX_PAGE_BYTES;
            apply(&mut inbox, &record, &mut budget).unwrap();
        }
        let current_publication_key = "native_transfer_v1:search-publication:current";
        let current_publication = inbox.get(current_publication_key, 4096).unwrap().unwrap();
        verify_imported_search_state(&mut inbox, &generation).unwrap();
        let assert_no_authority = || {
            let clean: bool = tx
                .query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key IN ('catalog_generation','search_version','search_backfill_complete','search_generation_current','search_generation_retained_history','search_incomplete_current','search_incomplete_retained_history')) AND NOT EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key GLOB 'native_transfer_v1:catalog-owner:*')",
                    ["ns"],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(clean, "search validation changed publication authority");
        };
        assert_no_authority();
        tx.execute(
            "DELETE FROM main.meta WHERE namespace='ns' AND key=?1",
            [current_publication_key],
        )
        .unwrap();
        assert!(verify_imported_search_state(&mut inbox, &generation).is_err());
        assert_no_authority();
        inbox
            .put(current_publication_key, &current_publication)
            .unwrap();
        tx.execute(
            "UPDATE main.meta SET value='not-json' WHERE namespace='ns' AND key=?1",
            [current_publication_key],
        )
        .unwrap();
        assert!(verify_imported_search_state(&mut inbox, &generation).is_err());
        assert_no_authority();
        inbox
            .put(current_publication_key, &current_publication)
            .unwrap();
        let mut mismatched: Vec<serde_json::Value> =
            serde_json::from_slice(&current_publication).unwrap();
        mismatched[2] = "b".repeat(64).into();
        inbox
            .put(
                current_publication_key,
                &serde_json::to_vec(&mismatched).unwrap(),
            )
            .unwrap();
        assert!(verify_imported_search_state(&mut inbox, &generation).is_err());
        assert_no_authority();
        inbox
            .put(current_publication_key, &current_publication)
            .unwrap();
        tx.execute(
            "UPDATE main.session_search_coverage SET incomplete=1 WHERE namespace='ns' AND read_scope='current'",
            [],
        )
        .unwrap();
        assert!(verify_imported_search_state(&mut inbox, &generation).is_err());
        assert_no_authority();
        tx.execute(
            "UPDATE main.session_search_coverage SET incomplete=0 WHERE namespace='ns' AND read_scope='current'",
            [],
        )
        .unwrap();
        verify_imported_search_state(&mut inbox, &generation).unwrap();
        assert_no_authority();
        assert!(
            inbox.get("search_version", 64).unwrap().is_none(),
            "incoming records do not expose readiness"
        );
        publish_search_state(&mut inbox, &generation).unwrap();
        for corruption in [
            "DELETE FROM meta WHERE key='native_transfer_v1:search-publication:current'",
            "UPDATE meta SET value='unsupported' WHERE key='search_version'",
            "UPDATE session_search_coverage SET incomplete=1 WHERE read_scope='current'",
            "DELETE FROM session_search_counts WHERE read_scope='current'",
            "DELETE FROM session_search_documents WHERE read_scope='retained_history'",
            "UPDATE session_search_memberships SET current_last_ms=8",
        ] {
            tx.execute_batch("SAVEPOINT corrupt").unwrap();
            tx.execute_batch(corruption).unwrap();
            assert!(
                publish_search_state(&mut inbox, &generation).is_err(),
                "accepted {corruption}"
            );
            tx.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
                .unwrap();
        }
        tx.execute(
            "DELETE FROM session_search_memberships WHERE gram=?1",
            [grams.first().unwrap()],
        )
        .unwrap();
        let mut budget = transfer::MAX_PAGE_BYTES;
        assert!(
            verify_authority(&inbox, CatalogRowKind::SearchDocument, &values, &mut budget).is_err()
        );
    }
}

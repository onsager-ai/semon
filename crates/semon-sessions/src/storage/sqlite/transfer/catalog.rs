//! Current logical Catalog rows; all physical names remain private.
use super::*;
use crate::storage::transfer::{
    CatalogRowKind, TransferRecord, catalog_rows, reported::CaptureState,
};
use rusqlite::types::Value;

fn spec(kind: CatalogRowKind) -> (&'static str, &'static str, &'static [usize]) {
    use CatalogRowKind::*;
    match kind {
        PublicationReceipt => ("storage_semantic_publications", "generation,epoch,admission,base_generation,source_revision,proof_digest,claims,metadata", &[]),
        PresentationRoot => ("storage_proof_presentation_roots", "publication,base,manifest,digest", &[]),
        ProofGeneration => ("storage_proof_generations", "id,session_key,publication,proof,admission,stream,metadata,slots,prefix", &[]),
        Presentation => ("storage_proof_presentations", "id,session_key,metadata", &[]),
        ProofSource => ("storage_proof_sources", "id,session_key,path,ledger", &[]),
        ProofRef => ("storage_proof_refs", "session_key,kind,id", &[]),
        Current => ("session_catalog", "session_key,metadata", &[]),
        Recipe => (
            "session_slot_projections",
            "session_key,generation,source_generation,version,total",
            &[],
        ),
        Slot => ("session_slots", "session_key,slot,metadata", &[]),
        History => ("session_history_catalog", "session_key,metadata", &[]),
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

fn verify_authority(tx: &Inbox<'_>, kind: CatalogRowKind, values: &[serde_json::Value]) -> Result<(), StorageError> {
    use CatalogRowKind::*;
    let text = |i: usize| values[i].as_str().ok_or(StorageError::ScopeChanged);
    match kind {
        PresentationRoot => {
            let exists: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM storage_semantic_publications WHERE namespace=?1 AND generation=?2 AND proof_digest=?3)", params![tx.1,text(0)?,text(3)?], |r| r.get(0)).map_err(failed)?;
            if !exists { return Err(StorageError::ScopeChanged); }
        }
        ProofGeneration => {
            let record = catalog_rows::proof_generation(tx.1, values)?;
            let metadata: String = tx.0.query_row("SELECT CASE WHEN octet_length(p.metadata)<=1048576 THEN p.metadata END FROM storage_semantic_publications p JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest WHERE p.namespace=?1 AND p.generation=?2 AND p.proof_digest=?3 AND p.admission=?4", params![tx.1,record.publication,record.proof,record.admission as i64], |r| r.get(0)).map_err(failed)?;
            let rows: Vec<String> = serde_json::from_str(&metadata).map_err(|_| StorageError::ScopeChanged)?;
            if !rows.contains(&record.metadata) { return Err(StorageError::ScopeChanged); }
        }
        Presentation => {
            let (metadata,slots,manifest): (String,String,String) = tx.0.query_row("SELECT CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END FROM storage_proof_generations g JOIN storage_proof_presentation_roots r ON r.namespace=g.namespace AND r.publication=g.publication AND r.digest=g.proof WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3", params![tx.1,text(0)?,text(1)?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
            crate::storage::proof_codec::verify_presentation_entry(text(1)?, &metadata, &slots, text(2)?, &manifest)?;
        }
        ProofSource => {
            let metadata: String = tx.0.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3", params![tx.1,text(0)?,text(1)?], |r| r.get(0)).map_err(failed)?;
            catalog_rows::verify_source_edge(&metadata, text(2)?, text(3)?)?;
        }
        ProofRef => {
            let exists: bool = tx.0.query_row("SELECT EXISTS(SELECT 1 FROM storage_proof_generations g JOIN storage_proof_presentations p USING(namespace,id,session_key) WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3)", params![tx.1,text(2)?,text(0)?], |r| r.get(0)).map_err(failed)?;
            if !exists { return Err(StorageError::ScopeChanged); }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn capture(
    tx: &mut Inbox<'_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    let index = usize::from(state.native_family - 32);
    let Some(kind) = CatalogRowKind::ALL.get(index).copied() else {
        transfer::reported::finish_capture(tx, state)?;
        return Ok(state.progress());
    };
    let generation: Option<String> =
        tx.0.query_row(
            "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_generation'",
            [tx.1],
            |r| r.get(0),
        )
        .optional()
        .map_err(failed)?;
    let generation = match generation {
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
    let table = if table.starts_with('(') {
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
    if rows.is_empty() {
        state.native_family += 1;
        state.native_key = None;
    } else {
        for row in &rows {
            verify_authority(tx, kind, &row.0)?;
        }
        catalog_rows::append(
            tx,
            state,
            rows.into_iter()
                .map(|row| TransferRecord::CatalogRow {
                    generation: generation.clone(),
                    kind,
                    values: row.0,
                })
                .collect(),
        )?;
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply(tx: &mut Inbox<'_>, record: &TransferRecord) -> Result<(), StorageError> {
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
    verify_authority(tx, *kind, values)?;
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
            "session_key,metadata,lifecycle,last_ms,harness,repo,parent_key",
            vec![
                values[0].clone(),
                values[1].clone(),
                row.lifecycle.as_str().into(),
                row.last.unwrap_or(0).into(),
                row.harness.into(),
                row.repo.into(),
                row.parent.into(),
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

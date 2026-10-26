//! Current logical Catalog rows; all physical names remain private.
use super::*;
use crate::storage::transfer::{CatalogRowKind, TransferRecord, catalog_rows, reported::CaptureState};
use rusqlite::types::Value;

fn spec(kind: CatalogRowKind) -> (&'static str, &'static str, &'static [usize]) {
    use CatalogRowKind::*;
    match kind {
        Current => ("session_catalog", "session_key,metadata", &[]),
        Recipe => ("session_slot_projections", "session_key,generation,source_generation,version,total", &[]),
        Slot => ("session_slots", "session_key,slot,metadata", &[]),
        History => ("session_history_catalog", "session_key,metadata", &[]),
        HistoryRecipe => ("session_history_projections", "session_key,generation,source_generation,version,total,incomplete", &[5]),
        HistorySlot => ("session_history_slots", "session_key,slot,metadata", &[]),
        HistoryOriginal => ("(SELECT namespace,substr(key,25) AS session_key,value AS metadata FROM main.meta WHERE key>='history_original_header:' AND key<'history_original_header;')", "session_key,metadata", &[]),
        TraceHeader => ("session_trace_headers", "session_key,generation,source_generation,metadata", &[]),
        TraceTurn => ("session_trace_turns", "session_key,turn_id,start_id,metadata", &[]),
        TraceEdge => ("session_trace_edges", "session_key,edge_id,metadata", &[]),
        TraceBookmark => ("session_trace_bookmarks", "session_key,target,edge_id", &[]),
        SearchDocument => ("session_search_documents", "read_scope,session_key,fields,complete", &[3]),
        SearchCount => ("session_search_counts", "read_scope,gram,members", &[]),
    }
}

fn sql_value(value: &serde_json::Value) -> Result<Value, StorageError> {
    Ok(match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(v) => Value::Integer(i64::from(*v)),
        serde_json::Value::Number(v) => Value::Integer(v.as_i64().ok_or(StorageError::ScopeChanged)?),
        serde_json::Value::String(v) => Value::Text(v.clone()),
        _ => return Err(StorageError::ScopeChanged),
    })
}

pub(super) fn capture(tx: &mut Inbox<'_>, state: &mut CaptureState) -> Result<transfer::CaptureProgress, StorageError> {
    let index = usize::from(state.native_family - 32);
    let Some(kind) = CatalogRowKind::ALL.get(index).copied() else {
        transfer::reported::finish_capture(tx, state)?;
        return Ok(state.progress());
    };
    let generation: Option<String> = tx.0.query_row("SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_generation'", [tx.1], |r| r.get(0)).optional().map_err(failed)?;
    let generation = match generation {
        Some(generation) => Some(generation),
        None => tx.get(catalog_rows::IMPORTED_GENERATION, 64)?.map(|bytes| String::from_utf8(bytes).map_err(|_|StorageError::ScopeChanged)).transpose()?,
    };
    let Some(generation) = generation else {
        // Absence of a publication is not an empty replacement publication.
        transfer::reported::finish_capture(tx, state)?;
        return Ok(state.progress());
    };
    let (table, columns, bools) = spec(kind);
    let names = columns.split(',').collect::<Vec<_>>();
    let keys = kind.keys().iter().map(|i| names[*i]).collect::<Vec<_>>().join(",");
    let mut binds = vec![Value::Text(tx.1.into())];
    let after = if let Some(cursor) = &state.native_key {
        let cursor: Vec<serde_json::Value> = serde_json::from_str(cursor).map_err(|_| StorageError::ScopeChanged)?;
        if cursor.len() != kind.keys().len() { return Err(StorageError::ScopeChanged); }
        for value in cursor { binds.push(sql_value(&value)?); }
        format!(" AND ({keys})>({})", (2..binds.len()+1).map(|i| format!("?{i}")).collect::<Vec<_>>().join(","))
    } else { String::new() };
    let current = if kind == CatalogRowKind::Current { " AND lifecycle='current'" } else { "" };
    let table = if table.starts_with('(') { table.to_owned() } else { format!("main.{table}") };
    let suffix = format!("FROM {table} WHERE namespace=?1{current}{after} ORDER BY {keys}");
    let size = names.iter().map(|n| format!("coalesce(octet_length({n}),0)")).collect::<Vec<_>>().join("+");
    let sizes = tx.0.prepare(&format!("SELECT {size} {suffix} LIMIT 64")).map_err(failed)?.query_map(rusqlite::params_from_iter(&binds), |r| r.get::<_,i64>(0)).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
    let mut count = 0;
    let mut bytes = 0usize;
    for size in &sizes {
        let size = usize::try_from(*size).map_err(|_| StorageError::ScopeChanged)?;
        if bytes.checked_add(size).is_none_or(|n| n > transfer::MAX_PAGE_BYTES) { break; }
        bytes += size; count += 1;
    }
    if count == 0 && !sizes.is_empty() { return Err(StorageError::Busy); }
    let rows = tx.0.prepare(&format!("SELECT {columns} {suffix} LIMIT {count}")).map_err(failed)?.query_map(rusqlite::params_from_iter(&binds), |r| crate::events::store::native_decode::values(r, bools)).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
    if rows.is_empty() {
        state.native_family += 1;
        state.native_key = None;
    } else {
        catalog_rows::append(tx, state, rows.into_iter().map(|row| TransferRecord::CatalogRow { generation: generation.clone(), kind, values: row.0 }).collect())?;
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply(tx: &mut Inbox<'_>, record: &TransferRecord) -> Result<(), StorageError> {
    catalog_rows::validate(record)?;
    let TransferRecord::CatalogRow { generation, kind, values } = record else { return Err(StorageError::InvalidArguments); };
    catalog_rows::bind_imported_generation(tx, generation)?;
    if *kind == CatalogRowKind::HistoryOriginal {
        return tx.put(&format!("history_original_header:{}", values[0].as_str().ok_or(StorageError::ScopeChanged)?), values[1].as_str().ok_or(StorageError::ScopeChanged)?.as_bytes());
    }
    let (table, columns, _) = spec(*kind);
    let (columns, values) = if *kind == CatalogRowKind::Current {
        let metadata = values[1].as_str().ok_or(StorageError::ScopeChanged)?;
        let row: crate::model::summary::CatalogRow = serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
        ("session_key,metadata,lifecycle,last_ms,harness,repo,parent_key", vec![values[0].clone(),values[1].clone(),row.lifecycle.as_str().into(),row.last.unwrap_or(0).into(),row.harness.into(),row.repo.into(),row.parent.into()])
    } else { (columns, values.clone()) };
    let mut binds = vec![Value::Text(tx.1.into())];
    for value in &values { binds.push(sql_value(value)?); }
    let slots = (1..=binds.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    tx.0.execute(&format!("INSERT INTO main.{table}(namespace,{columns}) VALUES({slots})"), rusqlite::params_from_iter(binds)).map_err(failed)?;
    if *kind == CatalogRowKind::Current {
        let row: crate::model::summary::CatalogRow = serde_json::from_str(values[1].as_str().ok_or(StorageError::ScopeChanged)?).map_err(|_|StorageError::ScopeChanged)?;
        for source in &row.sources {
            tx.0.execute("INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES(?1,?2,?3,?4,?5,?6)", params![tx.1,row.key,row.lifecycle.as_str(),row.harness,source.native_id,source.path.to_str().ok_or(StorageError::ScopeChanged)?]).map_err(failed)?;
        }
    }
    Ok(())
}

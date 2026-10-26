//! Fixed native child rows, with adapter-local parent IDs and keyset cursors.
use super::*;
use crate::events::store::{CODEX_USAGE_COLUMNS, SIGNAL_COLUMNS, USAGE_COLUMNS};
use crate::storage::transfer::{
    NativeRowKind, TransferRecord, native_rows, reported::CaptureState,
};
use rusqlite::types::Value;
fn spec(
    family: u8,
) -> Result<
    (
        NativeRowKind,
        &'static str,
        &'static str,
        &'static [usize],
        bool,
    ),
    StorageError,
> {
    use NativeRowKind::*;
    Ok(match family {
        8 => (Signal, "signals", SIGNAL_COLUMNS, &[0], false),
        9 => (MessageUsage, "usage", USAGE_COLUMNS, &[0], false),
        10 => (
            CopiedUsage,
            "usage_identity",
            "message_id,record_uuid",
            &[0, 1],
            false,
        ),
        11 => (CodexUsage, "codex_usage", CODEX_USAGE_COLUMNS, &[0], false),
        12 => (
            UsageFact,
            "native_usage_facts",
            "native_offset,identity,payload,record,outcome,model_outcome",
            &[0],
            true,
        ),
        13 => (
            UsageIdentity,
            "native_usage_identities",
            "scope,identity,native_offset",
            &[0, 1],
            true,
        ),
        14 => (
            UsageDimension,
            "native_usage_dimensions",
            "kind,dimension,value",
            &[0, 1],
            true,
        ),
        _ => return Err(StorageError::ScopeChanged),
    })
}
fn value(v: &serde_json::Value) -> Result<Value, StorageError> {
    Ok(match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => {
            Value::Integer(n.as_i64().ok_or(StorageError::ScopeChanged)?)
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        _ => return Err(StorageError::ScopeChanged),
    })
}
fn parent(tx: &Inbox<'_>, path: &str, source: bool) -> Result<i64, StorageError> {
    tx.0.query_row(
        if source {
            "SELECT source_id FROM native_usage_current WHERE namespace=?1 AND source_path=?2"
        } else {
            "SELECT file_id FROM files WHERE namespace=?1 AND path=?2"
        },
        params![tx.1, path],
        |r| r.get(0),
    )
    .map_err(failed)
}
pub(super) fn capture(
    tx: &mut Inbox<'_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    if state.native_family == 15 {
        state.native_family = 0;
        state.native_key = None;
        transfer::reported::save_capture(tx, state)?;
        return Ok(state.progress());
    }
    let (kind, table, columns, keys, source) = spec(state.native_family)?;
    let path = state
        .native_path
        .clone()
        .ok_or(StorageError::ScopeChanged)?;
    let id = parent(tx, &path, source)?;
    let names = columns.split(',').map(str::trim).collect::<Vec<_>>();
    let key_names = keys.iter().map(|i| names[*i]).collect::<Vec<_>>().join(",");
    let mut binds = vec![Value::Text(tx.1.into()), Value::Integer(id)];
    let after = if let Some(cursor) = &state.native_key {
        let cursor: Vec<serde_json::Value> =
            serde_json::from_str(cursor).map_err(|_| StorageError::ScopeChanged)?;
        if cursor.len() != keys.len() {
            return Err(StorageError::ScopeChanged);
        }
        for v in cursor {
            binds.push(value(&v)?);
        }
        format!(
            " AND ({key_names})>({})",
            (3..3 + keys.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        String::new()
    };
    let predicate = format!(
        "namespace=?1 AND {}=?2{after} ORDER BY {key_names}",
        if source { "source_id" } else { "file_id" }
    );
    let sizes = names
        .iter()
        .map(|n| format!("coalesce(octet_length({n}),0)"))
        .collect::<Vec<_>>()
        .join("+");
    let sizes =
        tx.0.prepare(&format!(
            "SELECT {sizes} FROM {table} WHERE {predicate} LIMIT 64"
        ))
        .map_err(failed)?
        .query_map(rusqlite::params_from_iter(&binds), |r| r.get::<_, i64>(0))
        .map_err(failed)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(failed)?;
    let mut bytes = 0usize;
    let mut count = 0;
    for size in sizes {
        let size = usize::try_from(size).map_err(|_| StorageError::ScopeChanged)?;
        if bytes
            .checked_add(size)
            .is_none_or(|n| n > transfer::MAX_PAGE_BYTES)
        {
            break;
        }
        bytes += size;
        count += 1;
    }
    if count == 0 {
        let exists: bool =
            tx.0.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {predicate})"),
                rusqlite::params_from_iter(&binds),
                |r| r.get(0),
            )
            .map_err(failed)?;
        if exists {
            return Err(StorageError::Busy);
        }
        state.native_family += 1;
        state.native_key = None;
    } else {
        let bools: &[usize] = if kind == NativeRowKind::MessageUsage {
            &[21]
        } else {
            &[]
        };
        let rows =
            tx.0.prepare(&format!(
                "SELECT {columns} FROM {table} WHERE {predicate} LIMIT {count}"
            ))
            .map_err(failed)?
            .query_map(rusqlite::params_from_iter(&binds), |r| {
                crate::events::store::native_decode::values(r, bools)
            })
            .map_err(failed)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed)?;
        let records = rows
            .into_iter()
            .map(|v| TransferRecord::NativeRow {
                path: path.clone(),
                kind,
                values: v.0,
            })
            .collect();
        native_rows::append(tx, state, records, keys)?;
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}
pub(super) fn apply(tx: &mut Inbox<'_>, record: &TransferRecord) -> Result<(), StorageError> {
    native_rows::validate(record)?;
    let TransferRecord::NativeRow { path, kind, values } = record else {
        return Err(StorageError::InvalidArguments);
    };
    let (_, table, columns, _, source) = (8..=14)
        .map(spec)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|s| s.0 == *kind)
        .ok_or(StorageError::InvalidArguments)?;
    let id = parent(tx, path, source)?;
    let mut binds = vec![Value::Text(tx.1.into()), Value::Integer(id)];
    for v in values {
        binds.push(value(v)?);
    }
    let slots = (1..=binds.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    tx.0.execute(
        &format!(
            "INSERT INTO {table}(namespace,{},{columns}) VALUES({slots})",
            if source { "source_id" } else { "file_id" }
        ),
        rusqlite::params_from_iter(binds),
    )
    .map_err(failed)?;
    Ok(())
}

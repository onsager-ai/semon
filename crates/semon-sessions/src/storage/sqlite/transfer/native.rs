//! Fixed native FILE/EVENT data plane; physical IDs remain adapter-local.
use super::*;
use crate::events::store::{EVENT_COLUMNS, FILE_COLUMNS};
use crate::storage::transfer::{CaptureProgress, Category, TransferRecord, reported::CaptureState};

pub(super) fn capture_native(
    tx: &mut Inbox<'_>,
    state: &mut CaptureState,
) -> Result<CaptureProgress, StorageError> {
    if state.native_family >= 32 {
        return super::catalog::capture(tx, state);
    }
    if state.native_family == 16 {
        // Withdrawn files have no live file row. Their reachable immutable
        // prefixes are still authoritative Native backing, not orphan inventory.
        let next: Option<(Option<String>, Option<String>)> = tx.0.query_row("SELECT CASE WHEN octet_length(s.path)<=1048576 THEN s.path END,CASE WHEN octet_length(s.ledger)=64 THEN s.ledger END FROM storage_proof_sources s WHERE s.namespace=?1 AND (?2 IS NULL OR (s.path,s.ledger)>(?2,?3)) AND NOT EXISTS(SELECT 1 FROM files f WHERE f.namespace=s.namespace AND f.path=s.path) AND EXISTS(SELECT 1 FROM storage_proof_refs r WHERE r.namespace=s.namespace AND r.id=s.id AND r.session_key=s.session_key) GROUP BY s.path,s.ledger ORDER BY s.path,s.ledger LIMIT 1", params![tx.1,state.native_path,state.native_backing], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(failed)?;
        if let Some((path, ledger)) = next {
            state.native_path = Some(path.ok_or(StorageError::ScopeChanged)?);
            state.native_backing = Some(ledger.ok_or(StorageError::ScopeChanged)?);
            state.native_family = 3;
        } else {
            state.native_family = 32;
            state.native_path = None;
            state.native_backing = None;
        }
        state.native_key = None;
        transfer::reported::save_capture(tx, state)?;
        return Ok(state.progress());
    }
    let id = state
        .cursor
        .as_ref()
        .map(|(key, _)| {
            key.as_slice()
                .try_into()
                .map(i64::from_be_bytes)
                .map_err(|_| StorageError::ScopeChanged)
        })
        .transpose()?
        .unwrap_or(0);
    if state.native_family >= 3 {
        return super::pages::capture(tx, state);
    }
    if state.native_family == 0 {
        let next: Option<i64> =
            tx.0.query_row(
                "SELECT min(file_id) FROM files WHERE namespace=?1 AND file_id>?2",
                params![tx.1, id],
                |r| r.get(0),
            )
            .map_err(failed)?;
        let Some(next) = next else {
            state.native_family = 16;
            state.native_key = None;
            state.native_path = None;
            state.native_backing = None;
            state.cursor = None;
            transfer::reported::save_capture(tx, state)?;
            return Ok(state.progress());
        };
        let sizes = FILE_COLUMNS
            .split(',')
            .map(|name| format!("coalesce(octet_length({}),0)", name.trim()))
            .collect::<Vec<_>>()
            .join("+");
        let size: i64 =
            tx.0.query_row(
                &format!("SELECT {sizes} FROM files WHERE namespace=?1 AND file_id=?2"),
                params![tx.1, next],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if size < 0 || size > (transfer::MAX_PAGE_BYTES - 4096) as i64 {
            return Err(StorageError::Busy);
        }
        let mut values =
            tx.0.query_row(
                &format!("SELECT {FILE_COLUMNS} FROM files WHERE namespace=?1 AND file_id=?2"),
                params![tx.1, next],
                |r| crate::events::store::native_decode::values(r, &[13]),
            )
            .map_err(failed)?
            .0;
        let path = values[1]
            .as_str()
            .ok_or(StorageError::ScopeChanged)?
            .to_owned();
        let record = TransferRecord::NativeFile {
            path: path.clone(),
            values: values.drain(2..).collect(),
        };
        record.validate_native()?;
        transfer::reported::append_capture(tx, state, Category::Ledgers, vec![record])?;
        state.native_path = Some(path);
        state.cursor = Some((next.to_be_bytes().to_vec(), -1));
        state.native_family = 1;
    } else {
        let extra = state.native_family == 2;
        let ordinal = state.cursor.as_ref().ok_or(StorageError::ScopeChanged)?.1;
        let sizes = EVENT_COLUMNS
            .split(',')
            .map(|name| format!("coalesce(octet_length({}),0)", name.trim()))
            .collect::<Vec<_>>()
            .join("+");
        let probe = format!(
            "SELECT {sizes} FROM events WHERE namespace=?1 AND file_id=?2 AND extra=?3 AND seq>?4 ORDER BY seq LIMIT 64"
        );
        let sizes =
            tx.0.prepare(&probe)
                .map_err(failed)?
                .query_map(params![tx.1, id, extra, ordinal], |r| r.get::<_, i64>(0))
                .map_err(failed)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(failed)?;
        let mut count = 0;
        let mut bytes = 4096usize;
        let path = state
            .native_path
            .clone()
            .ok_or(StorageError::ScopeChanged)?;
        for size in &sizes {
            let size = usize::try_from(*size)
                .map_err(|_| StorageError::ScopeChanged)?
                .checked_add(path.len())
                .and_then(|n| n.checked_add(1024))
                .ok_or(StorageError::Busy)?;
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
            if !sizes.is_empty() {
                return Err(StorageError::Busy);
            }
            state.native_family = if extra { 3 } else { 2 };
            state.cursor = Some((id.to_be_bytes().to_vec(), -1));
        } else {
            let sql = format!(
                "SELECT {EVENT_COLUMNS} FROM events WHERE namespace=?1 AND file_id=?2 AND extra=?3 AND seq>?4 ORDER BY seq LIMIT ?5"
            );
            let rows =
                tx.0.prepare(&sql)
                    .map_err(failed)?
                    .query_map(params![tx.1, id, extra, ordinal, count], |r| {
                        crate::events::store::native_decode::values(r, &[0, 8, 9, 14, 18, 22])
                    })
                    .map_err(failed)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(failed)?;
            let mut records = Vec::new();
            let mut encoded_bytes = 4096usize;
            for values in rows {
                let record = TransferRecord::NativeEvent {
                    path: path.clone(),
                    values: values.0,
                };
                record.validate_native()?;
                let size = transfer::encode_bounded(&record, transfer::MAX_PAGE_BYTES)?.len() + 1;
                if encoded_bytes
                    .checked_add(size)
                    .is_none_or(|n| n > transfer::MAX_PAGE_BYTES)
                {
                    break;
                }
                encoded_bytes += size;
                records.push(record);
            }
            if records.is_empty() {
                return Err(StorageError::Busy);
            }
            let last = match records.last().ok_or(StorageError::ScopeChanged)? {
                TransferRecord::NativeEvent { values, .. } => {
                    values[1].as_i64().ok_or(StorageError::ScopeChanged)?
                }
                _ => return Err(StorageError::ScopeChanged),
            };
            transfer::reported::append_capture(tx, state, Category::Events, records)?;
            state.cursor = Some((id.to_be_bytes().to_vec(), last));
        }
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply_native(
    tx: &mut Inbox<'_>,
    record: &TransferRecord,
) -> Result<(), StorageError> {
    record.validate_native()?;
    let (table, columns, values) = match record {
        TransferRecord::NativeFile { path, values } => {
            let id: i64 =
                tx.0.query_row(
                    "SELECT coalesce(max(file_id),0)+1 FROM files WHERE namespace=?1",
                    [tx.1],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            let mut row = vec![
                serde_json::Value::from(id),
                serde_json::Value::String(path.clone()),
            ];
            row.extend(values.clone());
            ("files", FILE_COLUMNS.to_owned(), row)
        }
        TransferRecord::NativeEvent { path, values } => {
            let id: i64 =
                tx.0.query_row(
                    "SELECT file_id FROM files WHERE namespace=?1 AND path=?2",
                    params![tx.1, path],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            let extra = values[0].as_bool().ok_or(StorageError::ScopeChanged)?;
            let sequence = values[1].as_i64().ok_or(StorageError::ScopeChanged)?;
            let expected: i64 = tx.0.query_row("SELECT coalesce(max(seq),-1)+1 FROM events WHERE namespace=?1 AND file_id=?2 AND extra=?3", params![tx.1,id,extra], |r|r.get(0)).map_err(failed)?;
            if sequence != expected {
                return Err(StorageError::Conflict);
            }
            let mut row = vec![serde_json::Value::from(id)];
            row.extend(values.clone());
            ("events", format!("file_id,{EVENT_COLUMNS}"), row)
        }
        _ => return Err(StorageError::InvalidArguments),
    };
    let mut binds = vec![rusqlite::types::Value::Text(tx.1.to_owned())];
    for value in values {
        use serde_json::Value;
        binds.push(match value {
            Value::Null => rusqlite::types::Value::Null,
            Value::Bool(v) => rusqlite::types::Value::Integer(i64::from(v)),
            Value::Number(v) => {
                rusqlite::types::Value::Integer(v.as_i64().ok_or(StorageError::ScopeChanged)?)
            }
            Value::String(v) => rusqlite::types::Value::Text(v),
            value @ Value::Array(_) => rusqlite::types::Value::Blob(
                serde_json::from_value(value).map_err(|_| StorageError::ScopeChanged)?,
            ),
            _ => return Err(StorageError::ScopeChanged),
        });
    }
    let slots = (1..=binds.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    tx.0.execute(
        &format!("INSERT INTO {table}(namespace,{columns}) VALUES({slots})"),
        rusqlite::params_from_iter(binds),
    )
    .map_err(|error| {
        #[cfg(test)]
        eprintln!("transfer SQLite native apply failed: table={table}");
        failed(error)
    })?;
    Ok(())
}

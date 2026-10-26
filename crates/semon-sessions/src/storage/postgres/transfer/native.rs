//! Fixed current native rows, normalized by the existing native reader.
use super::super::native_reader::{self, EVENT, FILE};
use super::*;
use crate::storage::transfer::{CaptureProgress, Category, TransferRecord, reported::CaptureState};
use tokio_postgres::types::ToSql;

pub(super) fn capture_native(
    tx: &mut Inbox<'_, '_>,
    state: &mut CaptureState,
) -> Result<CaptureProgress, StorageError> {
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
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
    if state.native_family == 0 {
        let rows = native_reader::transfer_rows(
            tx.0,
            "files",
            FILE,
            "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=(SELECT min(file_id) FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id>$2)",
            &[&ns, &id],
        )?;
        let Some(mut values) = rows.into_iter().next().map(|v| v.0) else {
            transfer::reported::finish_capture(tx, state)?;
            return Ok(state.progress());
        };
        let next = values[0].as_i64().ok_or(StorageError::ScopeChanged)?;
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
        state.native_family = 1;
        state.cursor = Some((next.to_be_bytes().to_vec(), -1));
    } else {
        let extra = state.native_family == 2;
        let ordinal = state.cursor.as_ref().ok_or(StorageError::ScopeChanged)?.1;
        let rows = native_reader::transfer_rows(
            tx.0,
            "events",
            EVENT,
            "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 AND extra=$3 AND seq>$4",
            &[&ns, &id, &extra, &ordinal],
        )?;
        if rows.is_empty() {
            state.native_family = if extra { 0 } else { 2 };
            state.cursor = Some((id.to_be_bytes().to_vec(), -1));
        } else {
            let path = state
                .native_path
                .clone()
                .ok_or(StorageError::ScopeChanged)?;
            let mut records = Vec::new();
            let mut bytes = 4096usize;
            let mut last = ordinal;
            for row in rows {
                let seq = row.0[1].as_i64().ok_or(StorageError::ScopeChanged)?;
                let record = TransferRecord::NativeEvent {
                    path: path.clone(),
                    values: row.0,
                };
                record.validate_native()?;
                let size = transfer::encode_bounded(&record, transfer::MAX_PAGE_BYTES)?.len() + 1;
                if bytes
                    .checked_add(size)
                    .is_none_or(|n| n > transfer::MAX_PAGE_BYTES)
                {
                    break;
                }
                records.push(record);
                bytes += size;
                last = seq;
            }
            if records.is_empty() {
                return Err(StorageError::Busy);
            }
            transfer::reported::append_capture(tx, state, Category::Events, records)?;
            state.cursor = Some((id.to_be_bytes().to_vec(), last));
        }
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply_native(
    tx: &mut Inbox<'_, '_>,
    record: &TransferRecord,
) -> Result<(), StorageError> {
    record.validate_native()?;
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    let (table, columns, values) = match record {
        TransferRecord::NativeFile { path, values } => {
            let id:i64=tx.0.driver.query_one("SELECT coalesce(max(file_id),0)+1 FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",&[&ns]).map_err(sqlstate::error)?.get(0);
            let mut row = vec![
                serde_json::Value::from(id),
                serde_json::Value::String(path.clone()),
            ];
            row.extend(values.clone());
            ("files", FILE.to_owned(), row)
        }
        TransferRecord::NativeEvent { path, values } => {
            let id:i64=tx.0.driver.query_one("SELECT file_id FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND path=$2",&[&ns,&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
            let extra = values[0].as_bool().ok_or(StorageError::ScopeChanged)?;
            let sequence = values[1].as_i64().ok_or(StorageError::ScopeChanged)?;
            let expected: i64 = tx.0.driver.query_one("SELECT coalesce(max(seq),-1)+1 FROM semon_pg_native_events WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 AND extra=$3", &[&ns,&id,&extra]).map_err(sqlstate::error)?.get(0);
            if sequence != expected { return Err(StorageError::Conflict); }
            let mut row = vec![serde_json::Value::from(id)];
            row.extend(values.clone());
            ("events", format!("file_id,{EVENT}"), row)
        }
        _ => return Err(StorageError::InvalidArguments),
    };
    let mut binds: Vec<Box<dyn ToSql + Sync>> = vec![Box::new(ns)];
    let mut names = vec!["namespace".to_owned()];
    let mut slots = vec!["$1".to_owned()];
    for (column, value) in columns.split(',').zip(values) {
        let name = column.split(':').next().ok_or(StorageError::ScopeChanged)?;
        names.push(name.to_owned());
        let mut slot = format!("${}", binds.len() + 1);
        let bound: Box<dyn ToSql + Sync> = if column.ends_with(":u") {
            slot.push_str("::text::numeric");
            Box::new(if value.is_null() {
                None
            } else {
                Some((value.as_i64().ok_or(StorageError::ScopeChanged)? as u64).to_string())
            })
        } else if [
            "failed",
            "extra",
            "code_mode",
            "code_mode_open",
            "reply",
            "reply_e",
            "peer",
        ]
        .contains(&name)
        {
            Box::new(if value.is_null() {
                None
            } else {
                Some(value.as_bool().ok_or(StorageError::ScopeChanged)?)
            })
        } else if ["script", "reply_f"].contains(&name) {
            Box::new(if value.is_null() {
                None
            } else {
                Some(
                    i16::try_from(value.as_i64().ok_or(StorageError::ScopeChanged)?)
                        .map_err(|_| StorageError::ScopeChanged)?,
                )
            })
        } else if [
            "file_id", "first_ms", "last_ms", "seq", "t", "parent", "poll", "reply_t", "item",
            "peer_pid",
        ]
        .contains(&name)
        {
            Box::new(if value.is_null() {
                None
            } else {
                Some(value.as_i64().ok_or(StorageError::ScopeChanged)?)
            })
        } else {
            let bytes: Option<Vec<u8>> = match value {
                serde_json::Value::Null => None,
                serde_json::Value::String(text) => Some(text.into_bytes()),
                value @ serde_json::Value::Array(_) => {
                    Some(serde_json::from_value(value).map_err(|_| StorageError::ScopeChanged)?)
                }
                _ => return Err(StorageError::ScopeChanged),
            };
            Box::new(bytes)
        };
        binds.push(bound);
        slots.push(slot);
    }
    let refs = binds
        .iter()
        .map(|v| &**v as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    tx.0.driver
        .execute(
            &format!(
                "INSERT INTO semon_pg_native_{table}({}) VALUES({})",
                names.join(","),
                slots.join(",")
            ),
            &refs,
        )
        .map_err(sqlstate::error)?;
    Ok(())
}

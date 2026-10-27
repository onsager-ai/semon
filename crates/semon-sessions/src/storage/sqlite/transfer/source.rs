//! Source journal pages use the same durable transfer cursor as native rows.
use super::*;
use transfer::{SourceJournalRecord as Row, TransferRecord, reported::CaptureState};

pub(super) fn capture(
    tx: &mut Inbox<'_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    let row = match state.source_phase {
        0 => Some(tx.0.query_row("SELECT revision,backfill_state FROM storage_source_journal WHERE namespace=?1", [tx.1], |r| Ok(Row::State { revision: r.get(0)?, backfill: r.get(1)? })).map_err(failed)?),
        1 => tx.get(crate::storage::source_observations::SELECTED_PROJECTION_KEY, 32768)?.map(|v| String::from_utf8(v).map(|value| Row::Selected { value }).map_err(|_| StorageError::ScopeChanged)).transpose()?,
        2 => tx.0.query_row("SELECT CASE WHEN octet_length(source_path)+octet_length(reason)<1044480 THEN source_path END,revision,reason,observed_at FROM session_catalog_invalidations WHERE namespace=?1 AND source_path>?2 ORDER BY source_path LIMIT 1", params![tx.1,state.source_after], |r| Ok(Row::Claim { path: r.get(0)?, revision: r.get(1)?, reason: r.get(2)?, observed_at: r.get(3)? })).optional().map_err(failed)?,
        3 => tx.0.query_row("SELECT CASE WHEN octet_length(source_path)<1044480 THEN source_path END FROM catalog_backfill_sources WHERE namespace=?1 AND source_path>?2 ORDER BY source_path LIMIT 1", params![tx.1,state.source_after], |r| Ok(Row::Member { path: r.get(0)? })).optional().map_err(failed)?,
        4 => tx.0.query_row("SELECT operation_id,payload_digest,outcome,recovery_done FROM source_observation_receipts WHERE namespace=?1 AND operation_id>?2 ORDER BY operation_id LIMIT 1", params![tx.1,state.source_after], |r| Ok(Row::Receipt {
            operation: crate::storage::source_observations::SourceOperation { namespace: tx.1.into(), id: r.get(0)?, payload_digest: r.get(1)? }, outcome: r.get(2)?, recovery_done: r.get(3)?,
        })).optional().map_err(failed)?,
        5 => {
            let after = if state.source_after.is_empty() { 0 } else { state.source_after.parse::<i64>().map_err(|_| StorageError::ScopeChanged)? };
            let row = tx.0.query_row("SELECT sequence,operation_id,payload_digest,CASE WHEN octet_length(payload)<1044480 THEN payload END FROM source_observation_intents WHERE namespace=?1 AND sequence>?2 ORDER BY sequence LIMIT 1", params![tx.1,after], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?))).optional().map_err(failed)?;
            if let Some((sequence,id,digest,payload)) = row {
                let payload = payload.ok_or(StorageError::Busy)?;
                let intent: crate::storage::source_observations::SourceIntent = serde_json::from_str(&payload).map_err(|_| StorageError::ScopeChanged)?;
                if intent.operation().id != id || intent.operation().payload_digest != digest { return Err(StorageError::ScopeChanged); }
                state.source_after = sequence.to_string();
                Some(Row::Intent { payload })
            } else { None }
        }
        _ => return Err(StorageError::InvalidArguments),
    };
    if let Some(row) = row {
        row.validate_namespace(tx.1)?;
        if matches!(
            row,
            Row::State {
                revision: 0,
                backfill: None
            }
        ) {
            state.source_phase += 1;
            transfer::reported::save_capture(tx, state)?;
            return Ok(state.progress());
        }
        match &row {
            Row::Claim { path, .. } | Row::Member { path } => state.source_after = path.clone(),
            Row::Receipt { operation, .. } => state.source_after = operation.id.clone(),
            Row::Intent { .. } => {},
            _ => state.source_phase += 1,
        }
        transfer::reported::append_capture(
            tx,
            state,
            row.category(),
            vec![TransferRecord::SourceJournal(row)],
        )?;
    } else {
        state.source_phase += 1;
        state.source_after.clear();
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply(tx: &mut Inbox<'_>, row: &Row) -> Result<(), StorageError> {
    row.validate_namespace(tx.1)?;
    match row {
        Row::Intent { payload } => {
            let intent: crate::storage::source_observations::SourceIntent = serde_json::from_str(payload).map_err(|_| StorageError::ScopeChanged)?;
            let op = intent.operation();
            tx.0.execute("INSERT INTO source_observation_intents(namespace,operation_id,payload_digest,payload) VALUES(?1,?2,?3,?4)", params![tx.1,op.id,op.payload_digest,payload]).map_err(failed)?;
        }
        Row::Receipt { operation, outcome, recovery_done } => {
            tx.0.execute("INSERT INTO source_observation_receipts(namespace,operation_id,payload_digest,outcome,recovery_done) VALUES(?1,?2,?3,?4,?5)", params![tx.1,operation.id,operation.payload_digest,outcome,recovery_done]).map_err(failed)?;
        }
        Row::State { revision, backfill } => {
            let changed = tx.0.execute("UPDATE storage_source_journal SET revision=?2,backfill_state=?3 WHERE namespace=?1 AND revision=0 AND backfill_state IS NULL", params![tx.1,revision,backfill]).map_err(failed)?;
            if changed != 1 {
                return Err(StorageError::Conflict);
            }
        }
        Row::Selected { value } => tx.put(
            crate::storage::source_observations::SELECTED_PROJECTION_KEY,
            value.as_bytes(),
        )?,
        Row::Claim {
            path,
            revision,
            reason,
            observed_at,
        } => {
            let valid: bool =
                tx.0.query_row(
                    "SELECT revision>=?2 FROM storage_source_journal WHERE namespace=?1",
                    params![tx.1, revision],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            if !valid {
                return Err(StorageError::ScopeChanged);
            }
            tx.0.execute("INSERT INTO session_catalog_invalidations(namespace,source_path,revision,reason,observed_at) VALUES(?1,?2,?3,?4,?5)", params![tx.1,path,revision,reason,observed_at]).map_err(failed)?;
        }
        Row::Member { path } => {
            tx.0.execute(
                "INSERT INTO catalog_backfill_sources(namespace,source_path) VALUES(?1,?2)",
                params![tx.1, path],
            )
            .map_err(failed)?;
        }
    }
    Ok(())
}

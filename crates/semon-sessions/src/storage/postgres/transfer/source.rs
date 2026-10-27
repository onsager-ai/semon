//! RLS-bound Source journal transfer, independent of Catalog head admission.
use super::*;
use transfer::{SourceJournalRecord as Row, TransferRecord, reported::CaptureState};
use sha2::Digest;

pub(super) fn capture(
    tx: &mut Inbox<'_, '_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    let row = match state.source_phase {
        0 => {
            let r = tx.0.driver.query_one("SELECT revision,backfill_state FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?;
            Some(Row::State {
                revision: r.get(0),
                backfill: r.get(1),
            })
        }
        1 => tx
            .get(
                crate::storage::source_observations::SELECTED_PROJECTION_KEY,
                32768,
            )?
            .map(|v| {
                String::from_utf8(v)
                    .map(|value| Row::Selected { value })
                    .map_err(|_| StorageError::ScopeChanged)
            })
            .transpose()?,
        2 => {
            let rows = tx.0.driver.query("SELECT CASE WHEN octet_length(source_path)+octet_length(reason)<1044480 THEN source_path END,revision,reason,observed_at FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path>$2 ORDER BY source_path LIMIT 1", &[&ns,&state.source_after.as_bytes()]).map_err(sqlstate::error)?;
            rows.first()
                .map(|r| -> Result<Row, StorageError> {
                    Ok(Row::Claim {
                        path: String::from_utf8(
                            r.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?,
                        )
                        .map_err(|_| StorageError::ScopeChanged)?,
                        revision: r.get(1),
                        reason: r.get(2),
                        observed_at: r.get(3),
                    })
                })
                .transpose()?
        }
        3 => {
            let rows = tx.0.driver.query("SELECT CASE WHEN octet_length(source_path)<1044480 THEN source_path END FROM semon_pg_source_members WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path>$2 ORDER BY source_path LIMIT 1", &[&ns,&state.source_after.as_bytes()]).map_err(sqlstate::error)?;
            rows.first()
                .map(|r| -> Result<Row, StorageError> {
                    Ok(Row::Member {
                        path: String::from_utf8(
                            r.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?,
                        )
                        .map_err(|_| StorageError::ScopeChanged)?,
                    })
                })
                .transpose()?
        }
        4 => {
            let rows = tx.0.driver.query("SELECT operation_id,payload_digest,outcome FROM semon_pg_source_receipts WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id>$2 ORDER BY operation_id LIMIT 1", &[&ns,&state.source_after]).map_err(sqlstate::error)?;
            if let Some(r) = rows.first() {
                let operation = crate::storage::source_observations::SourceOperation { namespace: tx.0.namespace.as_str().into(), id: r.get(0), payload_digest: r.get(1) };
                let outcome: String = r.get(2);
                let id = format!("{:x}", sha2::Sha256::digest(operation.id.as_bytes()));
                let request: [u8;32] = sha2::Sha256::digest(operation.payload_digest.as_bytes()).into();
                let prior = tx.0.driver.query("SELECT kind,request_digest,outcome FROM semon_pg_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id=$2", &[&ns,&id]).map_err(sqlstate::error)?;
                let recovery_done = if let Some(prior) = prior.first() {
                    let value: serde_json::Value = prior.get(2);
                    if prior.get::<_,String>(0) != "native-source-recovery" || prior.get::<_,Vec<u8>>(1) != request || value.get("applied").and_then(serde_json::Value::as_bool) != Some(outcome == "applied") { return Err(StorageError::ScopeChanged); }
                    Some(value.get("done").and_then(serde_json::Value::as_bool).ok_or(StorageError::ScopeChanged)?)
                } else { None };
                Some(Row::Receipt { operation, outcome, recovery_done })
            } else { None }
        }
        5 => {
            let after = if state.source_after.is_empty() { 0 } else { state.source_after.parse::<i64>().map_err(|_| StorageError::ScopeChanged)? };
            let rows = tx.0.driver.query("SELECT sequence,operation_id,payload_digest,CASE WHEN octet_length(payload)<1044480 THEN payload END FROM semon_pg_source_intents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND sequence>$2 ORDER BY sequence LIMIT 1", &[&ns,&after]).map_err(sqlstate::error)?;
            if let Some(r) = rows.first() {
                let payload = r.get::<_,Option<String>>(3).ok_or(StorageError::Busy)?;
                let intent: crate::storage::source_observations::SourceIntent = serde_json::from_str(&payload).map_err(|_| StorageError::ScopeChanged)?;
                if intent.operation().id != r.get::<_,String>(1) || intent.operation().payload_digest != r.get::<_,String>(2) { return Err(StorageError::ScopeChanged); }
                state.source_after = r.get::<_,i64>(0).to_string();
                Some(Row::Intent { payload })
            } else { None }
        }
        _ => return Err(StorageError::InvalidArguments),
    };
    if let Some(row) = row {
        row.validate_namespace(tx.0.namespace.as_str())?;
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

pub(super) fn apply(tx: &mut Inbox<'_, '_>, row: &Row) -> Result<(), StorageError> {
    row.validate_namespace(tx.0.namespace.as_str())?;
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    match row {
        Row::Intent { payload } => {
            let intent: crate::storage::source_observations::SourceIntent = serde_json::from_str(payload).map_err(|_| StorageError::ScopeChanged)?;
            let op = intent.operation();
            tx.0.driver.execute("INSERT INTO semon_pg_source_intents(namespace,operation_id,payload_digest,payload) VALUES($1,$2,$3,$4)", &[&ns,&op.id,&op.payload_digest,payload]).map_err(sqlstate::error)?;
        }
        Row::Receipt { operation, outcome, recovery_done } => {
            tx.0.driver.execute("INSERT INTO semon_pg_source_receipts(namespace,operation_id,payload_digest,outcome) VALUES($1,$2,$3,$4)", &[&ns,&operation.id,&operation.payload_digest,outcome]).map_err(sqlstate::error)?;
            if let Some(done) = recovery_done {
                let id = format!("{:x}", sha2::Sha256::digest(operation.id.as_bytes()));
                let request: [u8;32] = sha2::Sha256::digest(operation.payload_digest.as_bytes()).into();
                let value = serde_json::json!({"applied":outcome == "applied","done":done});
                let applied = tx.0.apply_operation(&super::super::OperationIdentity { operation_id: &id, kind: "native-source-recovery", request_digest: &request }, |_| Ok(value.clone()))?;
                if applied != value { return Err(StorageError::Conflict); }
            }
        }
        Row::State { revision, backfill } => {
            let changed = tx.0.driver.execute("UPDATE semon_pg_source_state SET revision=$2,backfill_state=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND revision=0 AND backfill_state IS NULL", &[&ns,revision,backfill]).map_err(sqlstate::error)?;
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
            let valid: bool = tx.0.driver.query_one("SELECT revision>=$2 FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns,revision]).map_err(sqlstate::error)?.get(0);
            if !valid {
                return Err(StorageError::ScopeChanged);
            }
            tx.0.driver.execute("INSERT INTO semon_pg_source_invalidations(namespace,source_path,revision,reason,observed_at) VALUES($1,$2,$3,$4,$5)", &[&ns,&path.as_bytes(),revision,reason,observed_at]).map_err(sqlstate::error)?;
        }
        Row::Member { path } => {
            tx.0.driver
                .execute(
                    "INSERT INTO semon_pg_source_members(namespace,source_path) VALUES($1,$2)",
                    &[&ns, &path.as_bytes()],
                )
                .map_err(sqlstate::error)?;
        }
    }
    Ok(())
}

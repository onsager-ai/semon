//! RLS-bound Source journal transfer, independent of Catalog head admission.
use super::*;
use transfer::{SourceJournalRecord as Row, TransferRecord, reported::CaptureState};

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
        _ => return Err(StorageError::InvalidArguments),
    };
    if let Some(row) = row {
        row.validate()?;
        match &row {
            Row::Claim { path, .. } | Row::Member { path } => state.source_after = path.clone(),
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
    row.validate()?;
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    match row {
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

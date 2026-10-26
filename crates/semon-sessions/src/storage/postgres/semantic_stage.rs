//! Bounded durable ordinary semantic producer. The final write reuses the
//! admitted publisher; stage metadata never authorizes native bytes.
use super::{catalog_effects::publication, model_cache, sqlstate, txn::PgTxn};
use crate::{
    events::Outcome,
    model::{PreparedSources, relation_stage::{Batch, Context, Phase}},
    storage::StorageError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stage {
    epoch: i64,
    paths: Vec<String>,
    runtime: String,
    generation: Option<String>,
    phase: Phase,
    context: Context,
    revision: i64,
    active: bool,
}

fn lock(tx: &mut PgTxn<'_>) -> Result<i64, StorageError> {
    tx.check()?;
    Ok(tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0))
}

fn load(tx: &mut PgTxn<'_>, active: bool) -> Result<Option<Stage>, StorageError> {
    let rows = tx.driver.query("SELECT stage_epoch,payload,payload_hash,active FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND seq=0 AND (NOT $2 OR active) ORDER BY stage_epoch DESC LIMIT 2", &[&tx.namespace.as_str().as_bytes(), &active]).map_err(sqlstate::error)?;
    let Some(row) = rows.first() else { return Ok(None); };
    if active && rows.len() > 1 { return Err(StorageError::ScopeChanged); }
    let bytes: Vec<u8> = row.get(1);
    if bytes.len() > MAX { return Err(StorageError::Busy); }
    let hash: Vec<u8> = row.get(2);
    if Sha256::digest(&bytes).as_slice() != hash { return Err(StorageError::ScopeChanged); }
    let stage: Stage = serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    if stage.epoch != row.get::<_, i64>(0) || stage.active != row.get::<_, bool>(3) { return Err(StorageError::ScopeChanged); }
    Ok(Some(stage))
}

fn save(tx: &mut PgTxn<'_>, stage: &Stage, insert: bool) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec(stage).map_err(|_| StorageError::Unavailable)?;
    if bytes.len() > MAX || stage.paths.len() > 64 || stage.context.endpoints.len() > 64 { return Err(StorageError::Busy); }
    let hash = Sha256::digest(&bytes).to_vec();
    let ns = tx.namespace.as_str().as_bytes();
    let changed = if insert {
        tx.driver.execute("INSERT INTO semon_pg_native_catalog_stage_items(namespace,stage_epoch,seq,payload,payload_hash,active) VALUES($1,$2,0,$3,$4,$5)", &[&ns, &stage.epoch, &bytes, &hash, &stage.active])
    } else {
        tx.driver.execute("UPDATE semon_pg_native_catalog_stage_items SET payload=$3,payload_hash=$4,active=$5 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND stage_epoch=$2 AND seq=0 AND active", &[&ns, &stage.epoch, &bytes, &hash, &stage.active])
    }.map_err(sqlstate::error)?;
    if changed != 1 { return Err(StorageError::Conflict); }
    tx.check()
}

pub(super) fn begin(tx: &mut PgTxn<'_>, paths: &[String], runtime: &str, held: Option<i64>) -> Result<(Outcome, i64), StorageError> {
    if paths.is_empty() || paths.len() > 64 || runtime.len() > 65536 || paths.iter().collect::<std::collections::BTreeSet<_>>().len() != paths.len() { return Err(StorageError::InvalidArguments); }
    let revision = lock(tx)?;
    let generation = model_cache::generation(tx)?;
    if let Some(stage) = load(tx, true)? {
        if held.is_some_and(|epoch| epoch != stage.epoch) || stage.runtime != runtime || stage.paths != paths || stage.generation != generation { return Err(StorageError::Conflict); }
        return Ok((Outcome::Written, stage.epoch));
    }
    if held.is_some() { return Err(StorageError::Conflict); }
    let epoch = load(tx, false)?.map_or(Ok(1), |stage| stage.epoch.checked_add(1).ok_or(StorageError::ScopeChanged))?;
    save(tx, &Stage { epoch, paths: paths.to_vec(), runtime: runtime.into(), generation, phase: Phase::Indexing, context: Context::default(), revision, active: true }, true)?;
    Ok((Outcome::Written, epoch))
}

pub(super) fn next(tx: &mut PgTxn<'_>, limit: usize) -> Result<Option<Batch>, StorageError> {
    tx.check()?;
    let Some(stage) = load(tx, true)? else { return Ok(None); };
    if limit == 0 || limit > 64 || stage.paths.len() > limit || (stage.phase == Phase::Relations && stage.paths.len() > 32) { return Err(StorageError::Busy); }
    if model_cache::generation(tx)? != stage.generation { return Err(StorageError::Conflict); }
    Ok(Some(Batch { epoch: stage.epoch, phase: stage.phase, paths: stage.paths, base_generation: stage.generation, source_revision: stage.revision, runtime: stage.runtime, context: stage.context, relation_cursor: None }))
}

fn checked(tx: &mut PgTxn<'_>, batch: &Batch) -> Result<Stage, StorageError> {
    let stage = load(tx, true)?.ok_or(StorageError::Conflict)?;
    if stage.epoch != batch.epoch || stage.phase != batch.phase || stage.paths != batch.paths || stage.runtime != batch.runtime || stage.generation != batch.base_generation || stage.revision != batch.source_revision || model_cache::generation(tx)? != stage.generation { return Err(StorageError::Conflict); }
    Ok(stage)
}

fn claims(tx: &mut PgTxn<'_>, batch: &Batch, prepared: &PreparedSources) -> Result<(), StorageError> {
    if prepared.base_generation != batch.base_generation || prepared.claims.len() != batch.paths.len() || prepared.claims.iter().map(|(path, _)| path).collect::<std::collections::BTreeSet<_>>() != batch.paths.iter().collect() || !prepared.withdrawals.is_empty() { return Err(StorageError::Conflict); }
    for (path, expected) in &prepared.claims {
        let rows = tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?;
        if rows.first().map(|row| row.get::<_, i64>(0)) != *expected { return Err(StorageError::Conflict); }
    }
    Ok(())
}

pub(super) fn record(tx: &mut PgTxn<'_>, batch: &Batch, prepared: &PreparedSources) -> Result<Outcome, StorageError> {
    let revision = lock(tx)?;
    let mut stage = checked(tx, batch)?;
    if batch.phase == Phase::Publication { return Err(StorageError::InvalidArguments); }
    claims(tx, batch, prepared)?;
    if batch.phase != Phase::Indexing && revision != stage.revision { return Err(StorageError::Conflict); }
    stage.context = prepared.relationships.clone();
    stage.context.epoch = Some(stage.epoch);
    stage.phase = match stage.phase { Phase::Indexing => Phase::Identity, Phase::Identity => Phase::Relations, Phase::Relations => Phase::Publication, Phase::Publication => unreachable!() };
    stage.revision = revision;
    save(tx, &stage, false)?;
    Ok(Outcome::Written)
}

pub(super) fn publish(tx: &mut PgTxn<'_>, batch: &Batch, prepared: &PreparedSources) -> Result<(Outcome, Option<String>), StorageError> {
    let revision = lock(tx)?;
    let mut stage = checked(tx, batch)?;
    if batch.phase != Phase::Publication || stage.revision != revision { return Err(StorageError::Conflict); }
    claims(tx, batch, prepared)?;
    let result = publication::publish(tx, &crate::slot_projection::Publication { rows: &prepared.rows, transcripts: Some(&prepared.transcripts) }, batch.base_generation.as_deref(), &prepared.claims, false)?;
    if result.0 == Outcome::Written { stage.active = false; save(tx, &stage, false)?; }
    Ok(result)
}

pub(super) fn finish(tx: &mut PgTxn<'_>, epoch: i64) -> Result<Outcome, StorageError> {
    lock(tx)?;
    let Some(mut stage) = load(tx, false)? else { return Err(StorageError::Conflict); };
    if stage.epoch != epoch { return Err(StorageError::Conflict); }
    if stage.active { stage.active = false; save(tx, &stage, false)?; }
    Ok(Outcome::Written)
}

pub(super) fn context(tx: &mut PgTxn<'_>, epoch: i64) -> Result<Context, StorageError> {
    tx.check()?;
    let stage = load(tx, true)?.ok_or(StorageError::Conflict)?;
    if stage.epoch != epoch || stage.generation != model_cache::generation(tx)? { return Err(StorageError::Conflict); }
    Ok(stage.context)
}

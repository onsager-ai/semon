//! Bounded durable ordinary semantic producer. The final write reuses the
//! admitted publisher; stage metadata never authorizes native bytes.
use super::{catalog_effects::publication, model_cache, sqlstate, txn::PgTxn};
use crate::{
    events::Outcome,
    model::{
        PreparedSources,
        relation_stage::{Batch, Context, Phase},
    },
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
    binding: String,
    generation: Option<String>,
    phase: Phase,
    context: Context,
    revision: i64,
    active: bool,
    model: Option<String>,
    acknowledgement: Option<String>,
    identities: Vec<(String, String, String)>,
}

fn lock(tx: &mut PgTxn<'_>) -> Result<i64, StorageError> {
    tx.check()?;
    Ok(tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0))
}

fn load(tx: &mut PgTxn<'_>, active: bool) -> Result<Option<Stage>, StorageError> {
    let rows = tx.driver.query("SELECT stage_epoch,CASE WHEN octet_length(payload)<=1048576 THEN payload END,payload_hash,active FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND seq=0 AND (NOT $2 OR active) ORDER BY stage_epoch DESC LIMIT 2", &[&tx.namespace.as_str().as_bytes(), &active]).map_err(sqlstate::error)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    if active && rows.len() > 1 {
        return Err(StorageError::ScopeChanged);
    }
    let bytes: Vec<u8> = row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?;
    if bytes.len() > MAX {
        return Err(StorageError::Busy);
    }
    let hash: Vec<u8> = row.get(2);
    if Sha256::digest(&bytes).to_vec() != hash {
        return Err(StorageError::ScopeChanged);
    }
    let stage: Stage = serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    if stage.epoch != row.get::<_, i64>(0) || stage.active != row.get::<_, bool>(3) {
        return Err(StorageError::ScopeChanged);
    }
    if model_cache::binding(tx)?
        .witness(&stage.runtime)
        .map_err(|_| StorageError::ScopeChanged)?
        != stage.binding
    {
        return Err(StorageError::Conflict);
    }
    Ok(Some(stage))
}

fn save(tx: &mut PgTxn<'_>, stage: &Stage, insert: bool) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec(stage).map_err(|_| StorageError::Unavailable)?;
    if bytes.len() > MAX || stage.paths.len() > 64 || stage.context.endpoints.len() > 64 {
        return Err(StorageError::Busy);
    }
    let hash = Sha256::digest(&bytes).to_vec();
    let ns = tx.namespace.as_str().as_bytes();
    let changed = if insert {
        tx.driver.execute("INSERT INTO semon_pg_native_catalog_stage_items(namespace,stage_epoch,seq,payload,payload_hash,active) VALUES($1,$2,0,$3,$4,$5)", &[&ns, &stage.epoch, &bytes, &hash, &stage.active])
    } else {
        tx.driver.execute("UPDATE semon_pg_native_catalog_stage_items SET payload=$3,payload_hash=$4,active=$5 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND stage_epoch=$2 AND seq=0 AND active", &[&ns, &stage.epoch, &bytes, &hash, &stage.active])
    }.map_err(sqlstate::error)?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    tx.check()
}

pub(super) fn begin(
    tx: &mut PgTxn<'_>,
    paths: &[String],
    runtime: &str,
    held: Option<i64>,
) -> Result<(Outcome, i64), StorageError> {
    if paths.is_empty()
        || paths.len() > 64
        || runtime.len() > 65536
        || paths
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != paths.len()
    {
        return Err(StorageError::InvalidArguments);
    }
    let revision = lock(tx)?;
    let files = super::native_reader::file_ledgers(tx)?;
    if files.len() > 64 || files.iter().any(|(path, _)| !paths.contains(path)) {
        return Err(StorageError::NamespaceUpgradePending);
    }
    let generation = model_cache::generation(tx)?;
    if let Some(stage) = load(tx, true)? {
        if held.is_some_and(|epoch| epoch != stage.epoch)
            || stage.runtime != runtime
            || stage.paths != paths
            || stage.generation != generation
        {
            return Err(StorageError::Conflict);
        }
        return Ok((Outcome::Written, stage.epoch));
    }
    if held.is_some() {
        return Err(StorageError::Conflict);
    }
    let epoch = load(tx, false)?.map_or(Ok(1), |stage| {
        stage.epoch.checked_add(1).ok_or(StorageError::ScopeChanged)
    })?;
    let binding = model_cache::binding(tx)?
        .witness(runtime)
        .map_err(|_| StorageError::ScopeChanged)?;
    save(
        tx,
        &Stage {
            epoch,
            paths: paths.to_vec(),
            runtime: runtime.into(),
            binding,
            generation,
            phase: Phase::Indexing,
            context: Context::default(),
            revision,
            active: true,
            model: None,
            acknowledgement: None,
            identities: Vec::new(),
        },
        true,
    )?;
    Ok((Outcome::Written, epoch))
}

pub(super) fn next(tx: &mut PgTxn<'_>, limit: usize) -> Result<Option<Batch>, StorageError> {
    tx.check()?;
    let Some(stage) = load(tx, true)? else {
        return Ok(None);
    };
    if limit == 0
        || limit > 64
        || stage.paths.len() > limit
        || (stage.phase == Phase::Relations && stage.paths.len() > 32)
    {
        return Err(StorageError::Busy);
    }
    if model_cache::generation(tx)? != stage.generation {
        return Err(StorageError::Conflict);
    }
    Ok(Some(Batch {
        epoch: stage.epoch,
        phase: stage.phase,
        paths: stage.paths,
        base_generation: stage.generation,
        source_revision: stage.revision,
        runtime: stage.runtime,
        context: stage.context,
        relation_cursor: None,
    }))
}

fn checked(tx: &mut PgTxn<'_>, batch: &Batch) -> Result<Stage, StorageError> {
    let stage = load(tx, true)?.ok_or(StorageError::Conflict)?;
    if stage.epoch != batch.epoch
        || stage.phase != batch.phase
        || stage.paths != batch.paths
        || stage.runtime != batch.runtime
        || stage.generation != batch.base_generation
        || stage.revision != batch.source_revision
        || serde_json::to_vec(&stage.context).map_err(|_| StorageError::ScopeChanged)?
            != serde_json::to_vec(&batch.context).map_err(|_| StorageError::ScopeChanged)?
        || model_cache::generation(tx)? != stage.generation
    {
        return Err(StorageError::Conflict);
    }
    Ok(stage)
}

fn claims(
    tx: &mut PgTxn<'_>,
    batch: &Batch,
    prepared: &PreparedSources,
) -> Result<(), StorageError> {
    if prepared.base_generation != batch.base_generation
        || prepared.claims.len() != batch.paths.len()
        || prepared
            .claims
            .iter()
            .map(|(path, _)| path)
            .collect::<std::collections::BTreeSet<_>>()
            != batch.paths.iter().collect()
        || !prepared.withdrawals.is_empty()
    {
        return Err(StorageError::Conflict);
    }
    if prepared.rows.iter().any(|row| {
        row.sources.len() > 64
            || row.sources.iter().any(|source| {
                !batch
                    .paths
                    .iter()
                    .any(|path| std::path::Path::new(path) == source.path)
            })
    }) {
        return Err(StorageError::ScopeChanged);
    }
    for (path, expected) in &prepared.claims {
        let rows = tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?;
        if rows.first().map(|row| row.get::<_, i64>(0)) != *expected {
            return Err(StorageError::Conflict);
        }
    }
    Ok(())
}

pub(super) fn record(
    tx: &mut PgTxn<'_>,
    batch: &Batch,
    prepared: &PreparedSources,
) -> Result<Outcome, StorageError> {
    let revision = lock(tx)?;
    let mut stage = checked(tx, batch)?;
    claims(tx, batch, prepared)?;
    if batch.phase != Phase::Indexing && revision != stage.revision {
        return Err(StorageError::Conflict);
    }
    if batch.phase == Phase::Publication {
        stage.model = Some(
            prepared
                .staged_model_witness()
                .map_err(|_| StorageError::ScopeChanged)?,
        );
        save(tx, &stage, false)?;
        return Ok(Outcome::Written);
    }
    if batch.phase == Phase::Indexing {
        let observed = prepared
            .indexed_sources
            .iter()
            .map(|(harness, source)| (harness.clone(), source.native_id.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        if prepared.indexed_sources.len() != batch.paths.len()
            || prepared
                .indexed_sources
                .iter()
                .map(|(_, source)| source.path.to_string_lossy().into_owned())
                .collect::<std::collections::BTreeSet<_>>()
                != batch.paths.iter().cloned().collect()
        {
            return Err(StorageError::Conflict);
        }
        let mut keys = std::collections::BTreeSet::new();
        for (harness, native) in &observed {
            let collision = observed
                .iter()
                .any(|(other, id)| id == native && other != harness);
            let mut key = if collision {
                format!("{harness}:{native}")
            } else {
                native.clone()
            };
            let mut probes = 0;
            while collision && (observed.iter().any(|(_, id)| id == &key) || keys.contains(&key)) {
                probes += 1;
                if probes > 64 || key.len() > 65536 {
                    return Err(StorageError::Busy);
                }
                key.insert(0, ':');
            }
            keys.insert(key.clone());
            stage
                .identities
                .push((harness.clone(), native.clone(), key));
        }
    }
    stage.context = prepared.relationships.clone();
    stage.context.epoch = Some(stage.epoch);
    stage.phase = match stage.phase {
        Phase::Indexing => Phase::Identity,
        Phase::Identity => Phase::Relations,
        Phase::Relations => Phase::Publication,
        Phase::Publication => unreachable!(),
    };
    stage.revision = revision;
    save(tx, &stage, false)?;
    Ok(Outcome::Written)
}

pub(super) fn publish(
    tx: &mut PgTxn<'_>,
    batch: &Batch,
    prepared: &PreparedSources,
) -> Result<(Outcome, Option<String>), StorageError> {
    let revision = lock(tx)?;
    let witness = prepared
        .staged_model_witness()
        .map_err(|_| StorageError::ScopeChanged)?;
    if let Some(stage) = load(tx, false)?
        && !stage.active
        && stage.epoch == batch.epoch
    {
        if stage.model.as_deref() != Some(&witness) {
            return Err(StorageError::Conflict);
        }
        return Ok((Outcome::Written, stage.acknowledgement));
    }
    let mut stage = checked(tx, batch)?;
    if stage.model.as_deref() != Some(&witness) {
        return Err(StorageError::Conflict);
    }
    if batch.phase != Phase::Publication || stage.revision != revision {
        return Err(StorageError::Conflict);
    }
    claims(tx, batch, prepared)?;
    let result = publication::publish(
        tx,
        &crate::slot_projection::Publication {
            rows: &prepared.rows,
            transcripts: Some(&prepared.transcripts),
        },
        batch.base_generation.as_deref(),
        &prepared.claims,
        false,
    )?;
    if result.0 == Outcome::Written {
        stage.active = false;
        stage.acknowledgement = result.1.clone();
        save(tx, &stage, false)?;
    }
    Ok(result)
}

pub(super) fn finish(tx: &mut PgTxn<'_>, epoch: i64) -> Result<Outcome, StorageError> {
    lock(tx)?;
    let Some(mut stage) = load(tx, false)? else {
        return Err(StorageError::Conflict);
    };
    if stage.epoch != epoch {
        return Err(StorageError::Conflict);
    }
    if stage.active {
        stage.active = false;
        save(tx, &stage, false)?;
    }
    Ok(Outcome::Written)
}

pub(super) fn context(tx: &mut PgTxn<'_>, epoch: i64) -> Result<Context, StorageError> {
    tx.check()?;
    let stage = load(tx, true)?.ok_or(StorageError::Conflict)?;
    if stage.epoch != epoch || stage.generation != model_cache::generation(tx)? {
        return Err(StorageError::Conflict);
    }
    Ok(stage.context)
}

pub(super) fn native_key(
    tx: &mut PgTxn<'_>,
    epoch: i64,
    harness: &str,
    native: &str,
) -> Result<String, StorageError> {
    let stage = load(tx, true)?.ok_or(StorageError::Conflict)?;
    if stage.epoch != epoch || stage.generation != model_cache::generation(tx)? {
        return Err(StorageError::Conflict);
    }
    stage
        .identities
        .iter()
        .find(|(h, n, _)| h == harness && n == native)
        .map(|(_, _, key)| key.clone())
        .ok_or(StorageError::ScopeChanged)
}

// The ordinary relationship recipe owns candidate cardinality and endpoint
// validation. This adapter supplies the epoch-fenced, native-derived cohort.
pub(super) fn select(
    tx: &mut PgTxn<'_>,
    epoch: i64,
    selector: &crate::model::relation_stage::Selector,
) -> Result<crate::model::relation_stage::Selection, StorageError> {
    use crate::model::relation_stage::{self, Selection, SelectorEndpointRow, SelectorLookup};
    struct Lookup {
        context: Context,
    }
    impl Lookup {
        fn keys(&self, predicate: impl Fn(&relation_stage::Endpoint) -> bool) -> Vec<String> {
            self.context
                .endpoints
                .values()
                .filter(|e| predicate(e))
                .take(2)
                .map(|e| e.row.key.clone())
                .collect()
        }
    }
    impl SelectorLookup for Lookup {
        type Error = StorageError;
        fn invalid(&self, _: &'static str) -> StorageError {
            StorageError::ScopeChanged
        }
        fn native_key(&self, _: &str, _: &str) -> Result<Selection, StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn receipt_first(&self, _: &str) -> Result<Option<Option<String>>, StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn call_owners(&self, _: &str) -> Result<Vec<(String, String)>, StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn event_metadata(&self, _: &str, _: &str) -> Result<Option<String>, StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn event_owner(&self, _: &str) -> Result<String, StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn endpoint_row(&self, key: &str) -> Result<SelectorEndpointRow, StorageError> {
            let e = self
                .context
                .endpoints
                .get(key)
                .ok_or(StorageError::ScopeChanged)?;
            let encoded = serde_json::to_string(e).map_err(|_| StorageError::Unavailable)?;
            if encoded.len() > 1048576 {
                return Err(StorageError::Busy);
            }
            Ok((Some(encoded), e.parent.clone(), Some(e.root.clone())))
        }
        fn root_alive(&self, key: &str) -> Result<bool, StorageError> {
            Ok(self.context.endpoints.get(key).is_some_and(|e| e.alive))
        }
        // Claude names can also be established by qualified send/receipt
        // pairs. Refuse until that native-event projection is connected;
        // endpoint names alone cannot prove a negative selection.
        fn name_candidates(&self, _: &str) -> Result<Vec<String>, StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn name_hints(&self, _: &str) -> Result<Vec<String>, StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn request_members_replay(&mut self, _: Vec<String>) -> Result<(), StorageError> {
            Err(StorageError::NamespaceUpgradePending)
        }
        fn codex_sibling(&self, parent: &str, task: &str) -> Result<Vec<String>, StorageError> {
            for e in self.context.endpoints.values() {
                if e.native_parents
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    > 1
                {
                    return Err(StorageError::ScopeChanged);
                }
            }
            Ok(self.keys(|e| {
                let name = e
                    .paths
                    .first()
                    .and_then(|p| p.rsplit('/').next())
                    .filter(|p| !p.is_empty())
                    .or(e.nickname.as_deref());
                e.row.harness == "codex"
                    && e.native_parents.first().is_some_and(|p| p.1 == parent)
                    && name == Some(task)
            }))
        }
        fn codex_relay(&self, root: &str, target: &str) -> Result<Vec<String>, StorageError> {
            Ok(self.keys(|e| {
                e.row.harness == "codex"
                    && e.root == root
                    && (e.nickname.as_deref() == Some(target)
                        || e.paths.iter().any(|p| p == target))
            }))
        }
        fn codex_wait(
            &self,
            root: &str,
            target: &str,
            exclude: &str,
        ) -> Result<Vec<String>, StorageError> {
            Ok(self.keys(|e| {
                e.row.harness == "codex"
                    && e.root == root
                    && e.row.key != exclude
                    && (e.row.key == target || e.paths.iter().any(|p| p == target))
            }))
        }
    }
    let mut lookup = Lookup {
        context: context(tx, epoch)?,
    };
    relation_stage::select_with(&mut lookup, selector)
}

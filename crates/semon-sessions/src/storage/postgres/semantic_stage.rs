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

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

const MAX: usize = 1024 * 1024;
const PUBLICATION_REFUSAL_KEY: &[u8] = b"semon:semantic_publication_refusal:v1";
const PUBLICATION_REFUSAL_MAX_BYTES: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationRefusal {
    version: u8,
    stage_epoch: i64,
    active_phase: Phase,
    attempted_phase: Phase,
    active_stage_sha256: String,
    active_claims_sha256: String,
    claims_sha256: String,
    runtime_sha256: String,
    head_sha256: Option<String>,
    source_revision: i64,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StageSnapshot {
    pub(crate) generation: Option<String>,
    pub(crate) epoch: Option<i64>,
    pub(crate) phase: Option<String>,
    pub(crate) paths: usize,
    pub(crate) claims: usize,
    pub(crate) stage_hash: Vec<u8>,
    pub(crate) path_hash: Vec<u8>,
    pub(crate) claims_hash: Vec<u8>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct SaveDiagnostic {
    pub(crate) phase: String,
    pub(crate) candidate_stage_bytes: usize,
    pub(crate) paths: usize,
    pub(crate) endpoints: usize,
    pub(crate) relations: usize,
    pub(crate) largest_owner_row_bytes: usize,
    pub(crate) largest_owner_witness_bytes: usize,
    pub(crate) model_witness_bytes: usize,
    pub(crate) stage_bytes_exceeded: bool,
    pub(crate) path_limit_exceeded: bool,
    pub(crate) endpoint_limit_exceeded: bool,
    pub(crate) before: Option<StageSnapshot>,
    pub(crate) after: Option<StageSnapshot>,
}

#[cfg(test)]
static SAVE_DIAGNOSTICS: OnceLock<Mutex<std::collections::HashMap<String, SaveDiagnostic>>> =
    OnceLock::new();

#[cfg(test)]
pub(crate) fn take_save_diagnostic(namespace: &str) -> Option<SaveDiagnostic> {
    SAVE_DIAGNOSTICS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(namespace)
}

#[cfg(test)]
fn save_diagnostic(
    namespace: &str,
    stage: &Stage,
    candidate_stage_bytes: usize,
    stage_bytes_exceeded: bool,
    path_limit_exceeded: bool,
    endpoint_limit_exceeded: bool,
) {
    let largest_owner_row_bytes = stage
        .context
        .endpoints
        .values()
        .map(|endpoint| serde_json::to_vec(&endpoint.row).map_or(0, |bytes| bytes.len()))
        .chain(
            stage
                .context
                .prior_owners
                .values()
                .chain(stage.context.retired_owners.iter())
                .map(|row| serde_json::to_vec(row).map_or(0, |bytes| bytes.len())),
        )
        .max()
        .unwrap_or_default();
    let largest_owner_witness_bytes = stage
        .context
        .endpoints
        .values()
        .map(|endpoint| serde_json::to_vec(endpoint).map_or(0, |bytes| bytes.len()))
        .chain(
            stage
                .context
                .prior_owners
                .values()
                .chain(stage.context.retired_owners.iter())
                .map(|row| serde_json::to_vec(row).map_or(0, |bytes| bytes.len())),
        )
        .max()
        .unwrap_or_default();
    let diagnostic = SaveDiagnostic {
        phase: format!("{:?}", stage.phase),
        candidate_stage_bytes,
        paths: stage.paths.len(),
        endpoints: stage.context.endpoints.len(),
        relations: stage.context.relations.len(),
        largest_owner_row_bytes,
        largest_owner_witness_bytes,
        model_witness_bytes: stage.model.as_ref().map_or(0, String::len),
        stage_bytes_exceeded,
        path_limit_exceeded,
        endpoint_limit_exceeded,
        before: None,
        after: None,
    };
    SAVE_DIAGNOSTICS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(namespace.to_owned(), diagnostic);
}

#[cfg(test)]
fn stage_snapshot(generation: Option<String>, stage: Option<&Stage>) -> StageSnapshot {
    let mut path_hash = Sha256::new();
    let mut claims_hash = Sha256::new();
    if let Some(stage) = stage {
        let stage_hash = serde_json::to_vec(stage)
            .map_or_else(|_| Vec::new(), |bytes| Sha256::digest(bytes).to_vec());
        for path in &stage.paths {
            path_hash.update((path.len() as u64).to_be_bytes());
            path_hash.update(path.as_bytes());
        }
        for (path, claim) in &stage.claims {
            claims_hash.update((path.len() as u64).to_be_bytes());
            claims_hash.update(path.as_bytes());
            match claim {
                Some(claim) => {
                    claims_hash.update([1]);
                    claims_hash.update(claim.to_be_bytes());
                }
                None => claims_hash.update([0]),
            }
        }
        StageSnapshot {
            generation,
            epoch: Some(stage.epoch),
            phase: Some(format!("{:?}", stage.phase)),
            paths: stage.paths.len(),
            claims: stage.claims.len(),
            stage_hash,
            path_hash: path_hash.finalize().to_vec(),
            claims_hash: claims_hash.finalize().to_vec(),
        }
    } else {
        StageSnapshot {
            generation,
            epoch: None,
            phase: None,
            paths: 0,
            claims: 0,
            stage_hash: Vec::new(),
            path_hash: path_hash.finalize().to_vec(),
            claims_hash: claims_hash.finalize().to_vec(),
        }
    }
}

#[cfg(test)]
pub(super) fn test_snapshot(tx: &mut PgTxn<'_>) -> Result<StageSnapshot, StorageError> {
    let generation = model_cache::generation(tx)?;
    let stage = load(tx, true)?;
    Ok(stage_snapshot(generation, stage.as_ref()))
}

#[cfg(test)]
pub(super) fn test_refusal_bytes(tx: &mut PgTxn<'_>) -> Result<Option<Vec<u8>>, StorageError> {
    super::native_keys::read_meta(tx, PUBLICATION_REFUSAL_KEY, PUBLICATION_REFUSAL_MAX_BYTES)
}

#[cfg(test)]
pub(super) fn test_write_refusal_bytes(
    tx: &mut PgTxn<'_>,
    bytes: &[u8],
) -> Result<(), StorageError> {
    if bytes.len() > PUBLICATION_REFUSAL_MAX_BYTES {
        return Err(StorageError::InvalidArguments);
    }
    super::native_keys::put_meta(tx, PUBLICATION_REFUSAL_KEY, bytes)
}

#[cfg(test)]
fn record_owner_sizes(namespace: &str, prepared: &PreparedSources, before: StageSnapshot) {
    let largest_owner_row_bytes = prepared
        .rows
        .iter()
        .map(|row| serde_json::to_vec(row).map_or(0, |bytes| bytes.len()))
        .max()
        .unwrap_or_default();
    let model_witness_bytes = prepared
        .staged_model_witness()
        .map_or(0, |witness| witness.len());
    let mut diagnostics = SAVE_DIAGNOSTICS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(diagnostic) = diagnostics.get_mut(namespace) {
        diagnostic.largest_owner_row_bytes = diagnostic
            .largest_owner_row_bytes
            .max(largest_owner_row_bytes);
        diagnostic.model_witness_bytes = model_witness_bytes;
        diagnostic.before = Some(before);
    }
}

#[cfg(test)]
pub(super) fn capture_after_refusal(
    pool: &super::PgPool,
    namespace: &crate::storage::LogicalSourceNamespace,
    control: &super::PgCancellation,
) {
    let has_diagnostic = SAVE_DIAGNOSTICS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(namespace.as_str());
    if !has_diagnostic {
        return;
    }
    let after = pool.transaction(
        namespace,
        super::txn::TxnKind::ReadSnapshot,
        control,
        |tx| {
            let generation = model_cache::generation(tx)?;
            let stage = load(tx, true)?;
            Ok(stage_snapshot(generation, stage.as_ref()))
        },
    );
    if let Ok(after) = after {
        let mut diagnostics = SAVE_DIAGNOSTICS
            .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(diagnostic) = diagnostics.get_mut(namespace.as_str()) {
            diagnostic.after = Some(after);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stage {
    epoch: i64,
    seeds: Vec<String>,
    paths: Vec<String>,
    claims: Vec<(String, Option<i64>)>,
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

fn sha256_hex(bytes: &[u8]) -> String {
    digest_hex(&Sha256::digest(bytes))
}

fn digest_hex(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let byte = *byte;
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn claims_sha256(claims: &[(String, Option<i64>)]) -> Result<String, StorageError> {
    if claims.len() > 64 {
        return Err(StorageError::ScopeChanged);
    }
    let mut ordered = claims.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(StorageError::ScopeChanged);
    }
    let mut digest = Sha256::new();
    digest.update(b"semon-pg-semantic-stage-claims-v1\0");
    for (path, claim) in ordered {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        match claim {
            Some(claim) => {
                digest.update([1]);
                digest.update(claim.to_be_bytes());
            }
            None => digest.update([0]),
        }
    }
    Ok(digest_hex(&digest.finalize()))
}

fn digest_is_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn attempted_phase(active: Phase) -> Phase {
    match active {
        Phase::Indexing => Phase::Identity,
        Phase::Identity => Phase::Relations,
        Phase::Relations => Phase::Publication,
        Phase::Publication => Phase::Publication,
    }
}

fn write_publication_refusal(
    tx: &mut PgTxn<'_>,
    active_stage_sha256: String,
    active_claims_sha256: String,
    active_phase: Phase,
    candidate: &Stage,
    claims: &[(String, Option<i64>)],
) -> Result<(), StorageError> {
    let head = model_cache::generation(tx)?;
    if head != candidate.generation {
        return Err(StorageError::Conflict);
    }
    let refusal = PublicationRefusal {
        version: 1,
        stage_epoch: candidate.epoch,
        active_phase,
        attempted_phase: candidate.phase,
        active_stage_sha256,
        active_claims_sha256,
        claims_sha256: claims_sha256(claims)?,
        runtime_sha256: sha256_hex(candidate.runtime.as_bytes()),
        head_sha256: head
            .as_deref()
            .map(|generation| sha256_hex(generation.as_bytes())),
        source_revision: candidate.revision,
    };
    let bytes = serde_json::to_vec(&refusal).map_err(|_| StorageError::Unavailable)?;
    if bytes.len() > PUBLICATION_REFUSAL_MAX_BYTES {
        return Err(StorageError::ScopeChanged);
    }
    super::native_keys::put_meta(tx, PUBLICATION_REFUSAL_KEY, &bytes)
}

fn current_claims(
    tx: &mut PgTxn<'_>,
    paths: &[String],
) -> Result<Vec<(String, Option<i64>)>, StorageError> {
    if paths.len() > 64
        || paths
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != paths.len()
    {
        return Err(StorageError::ScopeChanged);
    }
    let namespace = tx.namespace.as_str().as_bytes();
    let encoded = paths
        .iter()
        .map(|path| path.as_bytes().to_vec())
        .collect::<Vec<_>>();
    let rows = tx
        .driver
        .query(
            "SELECT source_path,revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=ANY($2::bytea[]) ORDER BY source_path",
            &[&namespace, &encoded],
        )
        .map_err(sqlstate::error)?;
    if rows.len() > paths.len() {
        return Err(StorageError::ScopeChanged);
    }
    let mut revisions = std::collections::BTreeMap::new();
    for row in rows {
        let path: Vec<u8> = row.get(0);
        let revision: i64 = row.get(1);
        if revisions.insert(path, revision).is_some() {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(paths
        .iter()
        .map(|path| (path.clone(), revisions.get(path.as_bytes()).copied()))
        .collect())
}

fn source_revision(tx: &mut PgTxn<'_>) -> Result<i64, StorageError> {
    let rows = tx
        .driver
        .query(
            "SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
            &[&tx.namespace.as_str().as_bytes()],
        )
        .map_err(sqlstate::error)?;
    if rows.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    Ok(rows[0].get(0))
}

pub(super) fn publication_blocker(
    tx: &mut PgTxn<'_>,
    generation: &str,
) -> Result<Option<crate::storage::catalog::CatalogPublicationBlocker>, StorageError> {
    let Some(bytes) =
        super::native_keys::read_meta(tx, PUBLICATION_REFUSAL_KEY, PUBLICATION_REFUSAL_MAX_BYTES)?
    else {
        return Ok(None);
    };
    let refusal: PublicationRefusal =
        serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    if refusal.version != 1
        || !digest_is_valid(&refusal.active_stage_sha256)
        || !digest_is_valid(&refusal.active_claims_sha256)
        || !digest_is_valid(&refusal.claims_sha256)
        || !digest_is_valid(&refusal.runtime_sha256)
        || refusal
            .head_sha256
            .as_deref()
            .is_some_and(|digest| !digest_is_valid(digest))
        || refusal.attempted_phase != attempted_phase(refusal.active_phase)
    {
        return Err(StorageError::ScopeChanged);
    }
    let (stage, payload_hash) = match load_epoch_with_hash(tx, true, None) {
        Ok(Some(stage)) => stage,
        Ok(None) | Err(StorageError::Conflict) => return Ok(None),
        Err(error) => return Err(error),
    };
    let current_generation = model_cache::generation(tx)?;
    if stage.epoch != refusal.stage_epoch
        || stage.phase != refusal.active_phase
        || digest_hex(&payload_hash) != refusal.active_stage_sha256
        || claims_sha256(&stage.claims)? != refusal.active_claims_sha256
        || claims_sha256(&current_claims(tx, &stage.paths)?)? != refusal.claims_sha256
        || sha256_hex(stage.runtime.as_bytes()) != refusal.runtime_sha256
        || current_generation.as_deref() != Some(generation)
        || stage.generation.as_deref() != Some(generation)
        || refusal.head_sha256
            != current_generation
                .as_deref()
                .map(|head| sha256_hex(head.as_bytes()))
        || source_revision(tx)? != refusal.source_revision
    {
        return Ok(None);
    }
    Ok(Some(
        crate::storage::catalog::CatalogPublicationBlocker::PublicationTooLarge,
    ))
}

fn lock(tx: &mut PgTxn<'_>) -> Result<i64, StorageError> {
    tx.check()?;
    Ok(tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&tx.namespace.as_str().as_bytes()]).map_err(sqlstate::error)?.get(0))
}

fn load(tx: &mut PgTxn<'_>, active: bool) -> Result<Option<Stage>, StorageError> {
    load_epoch(tx, active, None)
}

// Finished stages are retained per epoch, so an exact old request still finds
// its own recorded acknowledgement after successor publications.
fn load_epoch(
    tx: &mut PgTxn<'_>,
    active: bool,
    epoch: Option<i64>,
) -> Result<Option<Stage>, StorageError> {
    Ok(load_epoch_with_hash(tx, active, epoch)?.map(|(stage, _)| stage))
}

fn load_epoch_with_hash(
    tx: &mut PgTxn<'_>,
    active: bool,
    epoch: Option<i64>,
) -> Result<Option<(Stage, Vec<u8>)>, StorageError> {
    let rows = tx.driver.query("SELECT stage_epoch,CASE WHEN octet_length(payload)<=1048576 THEN payload END,payload_hash,active FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND seq=0 AND (NOT $2 OR active) AND ($3::bigint IS NULL OR stage_epoch=$3) ORDER BY stage_epoch DESC LIMIT 2", &[&tx.namespace.as_str().as_bytes(), &active, &epoch]).map_err(sqlstate::error)?;
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
    Ok(Some((stage, hash)))
}

fn save(tx: &mut PgTxn<'_>, stage: &Stage, insert: bool) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec(stage).map_err(|_| StorageError::Unavailable)?;
    save_serialized(tx, stage, insert, bytes)
}

fn save_serialized(
    tx: &mut PgTxn<'_>,
    stage: &Stage,
    insert: bool,
    bytes: Vec<u8>,
) -> Result<(), StorageError> {
    let stage_bytes_exceeded = bytes.len() > MAX;
    let path_limit_exceeded = stage.paths.len() > 64;
    let endpoint_limit_exceeded = stage.context.endpoints.len() > 64;
    if stage_bytes_exceeded || path_limit_exceeded || endpoint_limit_exceeded {
        #[cfg(test)]
        save_diagnostic(
            tx.namespace.as_str(),
            stage,
            bytes.len(),
            stage_bytes_exceeded,
            path_limit_exceeded,
            endpoint_limit_exceeded,
        );
        return Err(StorageError::Busy);
    }
    let hash = Sha256::digest(&bytes).to_vec();
    let ns = tx.namespace.as_str().as_bytes();
    let previous = if insert {
        None
    } else {
        let rows = tx.driver.query(
            "SELECT payload_hash FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND stage_epoch=$2 AND seq=0 AND active",
            &[&ns, &stage.epoch],
        ).map_err(sqlstate::error)?;
        if rows.len() > 1 {
            return Err(StorageError::ScopeChanged);
        }
        rows.first().map(|row| row.get::<_, Vec<u8>>(0))
    };
    if insert || previous.as_ref().is_some_and(|previous| previous != &hash) {
        super::native_keys::delete_meta(tx, PUBLICATION_REFUSAL_KEY)?;
    }
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

struct BoundedJsonCounter {
    bytes: usize,
    exceeded: bool,
}

impl std::io::Write for BoundedJsonCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let limit = MAX + 1;
        let remaining = limit.saturating_sub(self.bytes);
        if buffer.len() > remaining {
            self.bytes = limit;
            self.exceeded = true;
            return Err(std::io::Error::other(
                "owner row serialization exceeded bound",
            ));
        }
        self.bytes += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn has_oversized_owner_row(prepared: &PreparedSources) -> Result<bool, StorageError> {
    for row in &prepared.rows {
        let mut counter = BoundedJsonCounter {
            bytes: 0,
            exceeded: false,
        };
        let serialized = serde_json::to_writer(&mut counter, row);
        if counter.exceeded || counter.bytes > MAX {
            return Ok(true);
        }
        serialized.map_err(|_| StorageError::Unavailable)?;
    }
    Ok(false)
}

fn qualifies_owner_row_refusal(candidate_stage_bytes: usize, oversized_owner_row: bool) -> bool {
    candidate_stage_bytes > MAX && oversized_owner_row
}

fn oversized_owner_refusal(
    tx: &mut PgTxn<'_>,
    active_stage_sha256: &str,
    active_claims_sha256: &str,
    active_phase: Phase,
    candidate: &Stage,
    candidate_stage_bytes: usize,
    prepared: &PreparedSources,
) -> Result<bool, StorageError> {
    let oversized_owner_row = has_oversized_owner_row(prepared)?;
    if !qualifies_owner_row_refusal(candidate_stage_bytes, oversized_owner_row) {
        return Ok(false);
    }
    write_publication_refusal(
        tx,
        active_stage_sha256.to_owned(),
        active_claims_sha256.to_owned(),
        active_phase,
        candidate,
        &prepared.claims,
    )?;
    Ok(true)
}

pub(super) fn begin(
    tx: &mut PgTxn<'_>,
    paths: &[String],
    runtime: &str,
    held: Option<i64>,
) -> Result<(Outcome, Option<i64>), StorageError> {
    if paths.len() > 64
        || runtime.len() > 65536
        || paths
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != paths.len()
    {
        return Err(StorageError::InvalidArguments);
    }
    // A stage names changed paths and their component, not every ingested
    // file: publication carries unmentioned Current owners by bounded pages
    // and refuses an unmentioned owner connected to a claimed Source.
    let revision = lock(tx)?;
    let generation = model_cache::generation(tx)?;
    if let Some(stage) = load(tx, true)? {
        if held.is_some_and(|epoch| epoch != stage.epoch)
            || stage.runtime != runtime
            || (!paths.is_empty() && stage.seeds != paths)
            || stage.generation != generation
        {
            return Err(StorageError::Conflict);
        }
        return Ok((Outcome::Written, Some(stage.epoch)));
    }
    if paths.is_empty() {
        return Ok((Outcome::Conflict, None));
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
            seeds: paths.to_vec(),
            paths: paths.to_vec(),
            claims: Vec::new(),
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
    Ok((Outcome::Written, Some(epoch)))
}

pub(super) fn next(tx: &mut PgTxn<'_>, limit: usize) -> Result<Option<Batch>, StorageError> {
    let revision = lock(tx)?;
    let Some(mut stage) = load(tx, true)? else {
        return Ok(None);
    };
    if model_cache::generation(tx)? != stage.generation {
        return Err(StorageError::Conflict);
    }
    // Revisit the physical incident indexes after native indexing, too: a new
    // dependency can enlarge the old-owner closure without moving the head.
    let prior = super::semantic_owners::affected(tx, stage.generation.as_deref(), &stage.paths)?;
    let mut paths = stage
        .paths
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    paths.extend(prior.values().flat_map(|row| {
        row.sources
            .iter()
            .map(|source| source.path.to_string_lossy().into_owned())
    }));
    if paths.len() > 64 {
        return Err(StorageError::Busy);
    }
    let paths = paths.into_iter().collect::<Vec<_>>();
    let added = paths.iter().any(|path| !stage.paths.contains(path));
    let mut claims = Vec::new();
    for path in &paths {
        let rows = tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?;
        claims.push((path.clone(), rows.first().map(|row| row.get(0))));
    }
    if stage.phase != Phase::Indexing && (added || revision != stage.revision) {
        // Invalidate every in-memory batch and reduction witness atomically;
        // published rows, source claims and native custody remain untouched.
        stage.active = false;
        stage.model = None;
        stage.acknowledgement = None;
        save(tx, &stage, false)?;
        stage.epoch = stage
            .epoch
            .checked_add(1)
            .ok_or(StorageError::ScopeChanged)?;
        stage.active = true;
        stage.phase = Phase::Indexing;
        stage.context = Context::default();
        stage.identities.clear();
        stage.paths = paths;
        stage.claims = claims;
        stage.context.prior_owners = prior;
        stage.revision = revision;
        save(tx, &stage, true)?;
    } else {
        stage.paths = paths;
        stage.claims = claims;
        stage.context.prior_owners = prior;
        stage.revision = revision;
        save(tx, &stage, false)?;
    }
    #[cfg(test)]
    crate::events::store::configured::effect("semantic_lifecycle_enroll")
        .map_err(|_| StorageError::Unavailable)?;
    if limit == 0
        || limit > 64
        || stage.paths.len() > limit
        || (stage.phase == Phase::Relations && stage.paths.len() > 32)
    {
        return Err(StorageError::Busy);
    }
    Ok(Some(Batch {
        epoch: stage.epoch,
        phase: stage.phase,
        paths: stage.paths,
        source_claims: stage.claims.clone(),
        base_generation: stage.generation,
        source_revision: stage.revision,
        runtime: stage.runtime,
        context: stage.context,
        relation_cursors: Vec::new(),
    }))
}

fn checked(tx: &mut PgTxn<'_>, batch: &Batch) -> Result<Stage, StorageError> {
    checked_with_hash(tx, batch).map(|(stage, _)| stage)
}

fn checked_with_hash(tx: &mut PgTxn<'_>, batch: &Batch) -> Result<(Stage, Vec<u8>), StorageError> {
    let (stage, payload_hash) =
        load_epoch_with_hash(tx, true, None)?.ok_or(StorageError::Conflict)?;
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
    Ok((stage, payload_hash))
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
    {
        return Err(StorageError::Conflict);
    }
    withdrawals(tx, batch, prepared)?;
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

/// The shared producer's Facts-authority withdrawals (a non-current Codex
/// rollout). The staged model witness binds them from record to publication;
/// publication claims the path, so its owners leave Current under the same
/// generation CAS while native rows, history and older proofs stay retained.
fn withdrawals(
    tx: &mut PgTxn<'_>,
    batch: &Batch,
    prepared: &PreparedSources,
) -> Result<(), StorageError> {
    if prepared.withdrawals.len() > 64 {
        return Err(StorageError::Busy);
    }
    let mut paths = std::collections::BTreeSet::new();
    for negative in &prepared.withdrawals {
        let authority = &prepared.withdrawals[0];
        if !paths.insert(&negative.path)
            || !batch.paths.contains(&negative.path)
            || negative.runtime != batch.runtime
            || negative.facts_path != authority.facts_path
            || negative.facts_identity != authority.facts_identity
            || prepared
                .indexed_sources
                .iter()
                .any(|(_, source)| source.path == std::path::Path::new(&negative.path))
            || prepared
                .rows
                .iter()
                .flat_map(|row| &row.sources)
                .any(|source| source.path == std::path::Path::new(&negative.path))
            || !super::current_sources::withdrawal_recorded(tx, negative)?
        {
            return Err(StorageError::Conflict);
        }
    }
    Ok(())
}

fn absent(
    tx: &mut PgTxn<'_>,
    prepared: &PreparedSources,
    path: &str,
) -> Result<bool, StorageError> {
    if prepared.withdrawals.iter().any(|w| w.path == path) {
        return Ok(true);
    }
    if !prepared.removed_sources.contains(path) {
        return Ok(false);
    }
    let removed: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed')", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    Ok(removed
        && (super::native_keys::read_meta(tx, format!("source-retired:{path}").as_bytes(), 16384)?
            .is_some()
            || super::native_reader::ledger(tx, path)?.is_none()))
}

pub(super) fn record(
    tx: &mut PgTxn<'_>,
    batch: &Batch,
    prepared: &PreparedSources,
) -> Result<Outcome, StorageError> {
    let revision = lock(tx)?;
    let (mut stage, active_stage_payload_hash) = checked_with_hash(tx, batch)?;
    #[cfg(test)]
    let before_state = stage_snapshot(stage.generation.clone(), Some(&stage));
    claims(tx, batch, prepared)?;
    if batch.phase != Phase::Indexing && prepared.claims != stage.claims {
        return Err(StorageError::Conflict);
    }
    let active_stage_sha256 = digest_hex(&active_stage_payload_hash);
    let active_claims_sha256 = claims_sha256(&stage.claims)?;
    let active_phase = stage.phase;
    stage.claims.clone_from(&prepared.claims);
    if batch.phase != Phase::Indexing && revision != stage.revision {
        return Err(StorageError::Conflict);
    }
    if batch.phase == Phase::Publication {
        stage.model = Some(
            prepared
                .staged_model_witness()
                .map_err(|_| StorageError::ScopeChanged)?,
        );
        let candidate_bytes = serde_json::to_vec(&stage).map_err(|_| StorageError::Unavailable)?;
        let candidate_stage_bytes = candidate_bytes.len();
        let saved = save_serialized(tx, &stage, false, candidate_bytes);
        #[cfg(test)]
        if matches!(&saved, Err(StorageError::Busy)) {
            record_owner_sizes(tx.namespace.as_str(), prepared, before_state);
        }
        if matches!(&saved, Err(StorageError::Busy))
            && candidate_stage_bytes > MAX
            && oversized_owner_refusal(
                tx,
                &active_stage_sha256,
                &active_claims_sha256,
                active_phase,
                &stage,
                candidate_stage_bytes,
                prepared,
            )?
        {
            return Ok(Outcome::Deferred);
        }
        saved?;
        return Ok(Outcome::Written);
    }
    if batch.phase == Phase::Indexing {
        let observed = prepared
            .indexed_sources
            .iter()
            .map(|(harness, source)| (harness.clone(), source.native_id.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        let indexed = prepared
            .indexed_sources
            .iter()
            .map(|(_, source)| source.path.to_string_lossy().into_owned())
            .collect::<std::collections::BTreeSet<_>>();
        if indexed.len() != prepared.indexed_sources.len()
            || !indexed.iter().all(|path| batch.paths.contains(path))
        {
            return Err(StorageError::Conflict);
        }
        // The shared scan skips an absent Source, as the SQLite stage does.
        // Only a producer withdrawal or durable removal may be absent. Physical
        // absence was observed during preparation under Source locks; this SQL
        // transaction checks persisted evidence and the exact claim/ledger CAS.
        for path in batch.paths.iter().filter(|path| !indexed.contains(*path)) {
            if !absent(tx, prepared, path)? {
                return Err(StorageError::Conflict);
            }
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
    let prior = std::mem::take(&mut stage.context.prior_owners);
    stage.context = prepared.relationships.clone();
    stage.context.prior_owners = prior;
    stage.context.epoch = Some(stage.epoch);
    stage.phase = match stage.phase {
        Phase::Indexing => Phase::Identity,
        Phase::Identity => Phase::Relations,
        Phase::Relations => Phase::Publication,
        Phase::Publication => unreachable!(),
    };
    stage.revision = revision;
    let candidate_bytes = serde_json::to_vec(&stage).map_err(|_| StorageError::Unavailable)?;
    let candidate_stage_bytes = candidate_bytes.len();
    let saved = save_serialized(tx, &stage, false, candidate_bytes);
    #[cfg(test)]
    if matches!(&saved, Err(StorageError::Busy)) {
        record_owner_sizes(tx.namespace.as_str(), prepared, before_state);
    }
    if matches!(&saved, Err(StorageError::Busy))
        && candidate_stage_bytes > MAX
        && oversized_owner_refusal(
            tx,
            &active_stage_sha256,
            &active_claims_sha256,
            active_phase,
            &stage,
            candidate_stage_bytes,
            prepared,
        )?
    {
        return Ok(Outcome::Deferred);
    }
    saved?;
    Ok(Outcome::Written)
}

pub(super) fn publish(
    tx: &mut PgTxn<'_>,
    batch: &Batch,
    prepared: &PreparedSources,
    lifecycle: Result<Option<publication::ValidatedLifecycle<'_>>, StorageError>,
    withdrawals: Result<(), StorageError>,
) -> Result<(Outcome, Option<String>), StorageError> {
    let revision = lock(tx)?;
    let witness = prepared
        .staged_model_witness()
        .map_err(|_| StorageError::ScopeChanged)?;
    if let Some(stage) = load_epoch(tx, false, Some(batch.epoch))?
        && !stage.active
        && stage.epoch == batch.epoch
    {
        // Replay qualifies the captured request, not today's acknowledged
        // journal/head. The model witness excludes claims and base generation.
        if stage.model.as_deref() != Some(&witness)
            || stage.acknowledgement.is_none()
            || stage.claims != prepared.claims
            || stage.generation != prepared.base_generation
            || stage.phase != batch.phase
            || stage.paths != batch.paths
            || stage.runtime != batch.runtime
            || stage.generation != batch.base_generation
            || stage.revision != batch.source_revision
            || serde_json::to_vec(&stage.context).map_err(|_| StorageError::ScopeChanged)?
                != serde_json::to_vec(&batch.context).map_err(|_| StorageError::ScopeChanged)?
        {
            return Err(StorageError::Conflict);
        }
        return Ok((Outcome::Written, stage.acknowledgement));
    }
    withdrawals?;
    let mut stage = checked(tx, batch)?;
    let lifecycle = lifecycle?;
    if serde_json::to_vec(&lifecycle).map_err(|_| StorageError::ScopeChanged)?
        != serde_json::to_vec(&prepared.relationships.lifecycle_authority)
            .map_err(|_| StorageError::ScopeChanged)?
        || stage.claims != prepared.claims
    {
        return Err(StorageError::Conflict);
    }
    if stage.model.as_deref() != Some(&witness) {
        return Err(StorageError::Conflict);
    }
    if batch.phase != Phase::Publication || stage.revision != revision {
        return Err(StorageError::Conflict);
    }
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE semantic_claims");
    claims(tx, batch, prepared)?;
    if !prepared.rows.is_empty() {
        // The stage owns one global lifecycle runtime; each Query row owns
        // its independently captured scope for this exact source generation.
        let observations = prepared
            .presentation_observations()
            .ok_or(StorageError::ScopeChanged)?;
        let runtime = model_cache::binding(tx)?
            .witness(&batch.runtime)
            .map_err(|_| StorageError::ScopeChanged)?;
        if observations.source_runtime.as_deref() != Some(batch.runtime.as_str())
            || observations.runtime.as_deref() != Some(runtime.as_str())
        {
            return Err(StorageError::ScopeChanged);
        }
        for row in &prepared.rows {
            let expected = observations
                .runtime_for(row)
                .ok_or(StorageError::ScopeChanged)?;
            if row
                .query
                .as_ref()
                .and_then(|query| query["runtime"].as_str())
                != Some(expected.as_str())
            {
                return Err(StorageError::ScopeChanged);
            }
        }
    }
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE publication_call");
    let result = publication::publish(
        tx,
        &crate::slot_projection::Publication {
            rows: &prepared.rows,
            transcripts: Some(&prepared.transcripts),
            observations: prepared.presentation_observations(),
        },
        batch.base_generation.as_deref(),
        &prepared.claims,
        false,
        Some(publication::Semantic {
            runtime: &batch.runtime,
            withdrawals: &prepared.withdrawals,
            prior_owners: &batch.context.prior_owners,
            retired_owners: &prepared.relationships.retired_owners,
            lifecycle_authority: lifecycle,
        }),
    )?;
    if result.0 == Outcome::Written {
        stage.active = false;
        stage.acknowledgement = result.1.clone();
        #[cfg(test)]
        eprintln!("PG_PUBLISH_STAGE stage_ack_save");
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

// The ordinary relationship recipe owns candidate cardinality and endpoint
// validation. This adapter supplies the epoch-fenced, native-derived cohort.
pub(super) fn select(
    tx: &mut PgTxn<'_>,
    epoch: i64,
    selector: &crate::model::relation_stage::Selector,
) -> Result<Option<crate::model::relation_stage::Selection>, StorageError> {
    use crate::model::relation_stage::{self, Selection, SelectorEndpointRow, SelectorLookup};
    struct Lookup {
        context: Context,
        identities: Vec<(String, String, String)>,
        events: Vec<crate::events::store::semantic_stage::ProjectedEvent>,
        hints: Vec<(String, String)>,
        replay: Vec<String>,
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
        fn metadata_budget(&self) -> StorageError {
            StorageError::Busy
        }
        fn event_read_budget(&self) -> StorageError {
            StorageError::Busy
        }
        fn native_key(&self, native: &str, harness: &str) -> Result<Selection, StorageError> {
            let key = self
                .identities
                .iter()
                .find(|(h, n, _)| h == harness && n == native)
                .map(|(_, _, key)| key.clone())
                .ok_or(StorageError::ScopeChanged)?;
            let endpoints = self
                .context
                .endpoints
                .values()
                .filter(|e| {
                    e.row.harness == harness && e.row.native_ids.iter().any(|n| n == native)
                })
                .collect::<Vec<_>>();
            Ok(Selection {
                count: 1,
                key: Some(key),
                endpoint: (endpoints.len() == 1).then(|| endpoints[0].clone()),
                ..Default::default()
            })
        }
        fn receipt_first(&self, id: &str) -> Result<Option<Option<String>>, StorageError> {
            Ok(self
                .events
                .iter()
                .find(|r| r.kind == "xsm" && r.msg.as_deref() == Some(id))
                .map(|r| Some(r.encoded.clone())))
        }
        fn call_owners(&self, id: &str) -> Result<Vec<(String, String)>, StorageError> {
            let mut owners = self
                .events
                .iter()
                .filter(|r| r.kind == "call" && r.native.event.id.as_deref() == Some(id))
                .map(|r| (r.native.owner.clone(), r.key.clone()))
                .collect::<Vec<_>>();
            owners.sort();
            owners.truncate(2);
            Ok(owners)
        }
        fn event_metadata(&self, path: &str, key: &str) -> Result<Option<String>, StorageError> {
            self.events
                .iter()
                .find(|r| r.native.owner == path && r.key == key)
                .map(|r| Some(r.encoded.clone()))
                .ok_or(StorageError::ScopeChanged)
        }
        fn event_owner(&self, path: &str) -> Result<String, StorageError> {
            self.context
                .endpoints
                .values()
                .find(|e| e.row.sources.iter().any(|s| s.path.to_str() == Some(path)))
                .map(|e| e.row.key.clone())
                .ok_or(StorageError::ScopeChanged)
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
        fn name_candidates(&self, name: &str) -> Result<Vec<String>, StorageError> {
            let mut keys = self
                .context
                .endpoints
                .values()
                .filter(|e| {
                    e.row.harness == "claude"
                        && e.row.kind == "session"
                        && e.names.iter().any(|n| n == name)
                })
                .map(|e| e.row.key.clone())
                .collect::<std::collections::BTreeSet<_>>();
            for sent in self.events.iter().filter(|r| r.kind == "send") {
                let Some(msg) = &sent.msg else {
                    continue;
                };
                let Some(receipt) = self
                    .events
                    .iter()
                    .find(|r| r.kind == "xsm" && r.msg.as_ref() == Some(msg))
                else {
                    continue;
                };
                if receipt.name.as_deref() != Some(name) {
                    continue;
                }
                let owner = self.event_owner(&sent.native.owner)?;
                let endpoint = self
                    .context
                    .endpoints
                    .get(&owner)
                    .ok_or(StorageError::ScopeChanged)?;
                if self
                    .context
                    .endpoints
                    .get(&endpoint.root)
                    .is_some_and(|e| e.row.kind == "session")
                {
                    keys.insert(endpoint.root.clone());
                }
            }
            Ok(keys.into_iter().take(2).collect())
        }
        fn name_hints(&self, name: &str) -> Result<Vec<String>, StorageError> {
            Ok(self
                .hints
                .iter()
                .filter(|(_, n)| n == name)
                .map(|(k, _)| k.clone())
                .collect())
        }
        fn request_members_replay(&mut self, keys: Vec<String>) -> Result<(), StorageError> {
            relation_stage::replay_members_with(&keys, || StorageError::Busy, |_| Ok(()))?;
            self.replay = keys;
            Ok(())
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
    lock(tx)?;
    tx.check()?;
    let stage = load(tx, true)?.ok_or(StorageError::Conflict)?;
    if stage.epoch != epoch || stage.generation != model_cache::generation(tx)? {
        return Err(StorageError::Conflict);
    }
    let (context, identities) = (stage.context, stage.identities);
    if let relation_stage::Selector::NativeKey { .. } = selector {
        // Stage identities answer this selector; no event projection needed.
        let mut lookup = Lookup {
            context,
            identities,
            events: Vec::new(),
            hints: Vec::new(),
            replay: Vec::new(),
        };
        let mut used = 0;
        let mut event_reads = 0;
        return relation_stage::select_with(&mut lookup, selector, &mut used, &mut event_reads)
            .map(Some);
    }
    let events = super::semantic_events::project(tx, &context)?;
    let generation = model_cache::generation(tx)?;
    let ns = tx.namespace.as_str().as_bytes();
    let mut hints = Vec::new();
    if let Some(generation) = generation {
        // Claude Current owners with saved names, by bounded key pages. Each
        // page has its own 1 MiB budget and checks the deadline.
        let mut after: Option<Vec<u8>> = None;
        loop {
            tx.check()?;
            let values=tx.driver.query("WITH page AS MATERIALIZED (SELECT d.key_digest,d.session_key,d.metadata AS description,r.metadata AS owner FROM semon_pg_native_descriptions d JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=d.namespace_digest AND r.namespace=d.namespace AND r.key_digest=d.key_digest AND r.session_key=d.session_key WHERE d.namespace_digest=sha256($1::bytea) AND d.namespace=$1 AND r.generation=$2 AND r.harness=$3 AND ($4::bytea IS NULL OR d.key_digest>$4) ORDER BY d.key_digest LIMIT 64) SELECT key_digest,session_key,CASE WHEN sum(octet_length(description)+octet_length(owner)) OVER ()<=1048576 THEN description END,owner FROM page ORDER BY key_digest", &[&ns,&generation.as_bytes(),&b"claude".as_slice(),&after]).map_err(sqlstate::error)?;
            let Some(last) = values.last() else {
                break;
            };
            after = Some(last.get(0));
            for value in &values {
                let key =
                    String::from_utf8(value.get(1)).map_err(|_| StorageError::ScopeChanged)?;
                let raw: Vec<u8> = value
                    .get::<_, Option<Vec<u8>>>(2)
                    .ok_or(StorageError::Busy)?;
                if context.endpoints.contains_key(&key) {
                    continue;
                }
                let owner: crate::model::summary::CatalogRow =
                    serde_json::from_slice(&value.get::<_, Vec<u8>>(3))
                        .map_err(|_| StorageError::ScopeChanged)?;
                if owner.harness != "claude" || owner.key != key {
                    return Err(StorageError::ScopeChanged);
                }
                let description: serde_json::Value =
                    serde_json::from_slice(&raw).map_err(|_| StorageError::ScopeChanged)?;
                if let Some(names) = description["names"].as_array() {
                    for name in names.iter().filter_map(|n| n.as_str()) {
                        hints.push((key.clone(), name.to_owned()));
                    }
                }
            }
        }
    }
    let mut lookup = Lookup {
        context,
        identities,
        events,
        hints,
        replay: Vec::new(),
    };
    let mut used = 0;
    let mut event_reads = 0;
    let result = relation_stage::select_with(&mut lookup, selector, &mut used, &mut event_reads);
    if !lookup.replay.is_empty() {
        let mut stage = load(tx, true)?.ok_or(StorageError::Conflict)?;
        if stage.epoch != epoch {
            return Err(StorageError::Conflict);
        }
        stage.context.replay_owners.extend(lookup.replay);
        stage.context.replay_owners.sort();
        stage.context.replay_owners.dedup();
        if stage.context.replay_owners.len() > 64 {
            return Err(StorageError::Busy);
        }
        stage.phase = Phase::Indexing;
        stage.model = None;
        save(tx, &stage, false)?;
        return Ok(None);
    }
    result.map(Some)
}

#[cfg(test)]
mod publication_refusal_tests {
    use super::{MAX, qualifies_owner_row_refusal};

    #[test]
    fn aggregate_and_count_only_refusals_remain_busy() {
        assert!(!qualifies_owner_row_refusal(MAX + 1, false));
        assert!(!qualifies_owner_row_refusal(MAX, true));
        assert!(qualifies_owner_row_refusal(MAX + 1, true));
    }
}

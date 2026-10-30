//! Namespace/epoch-bound current Facts projection. Only producer preparation
//! reads canonical Raw Facts; selected reads fetch one header and one member.
use super::{
    PgCancellation, PgPool, model_cache, native_keys,
    txn::{PgTxn, TxnKind},
};
use crate::{
    facts::{FACTS_VERSION, source_authority::Identity},
    storage::{LogicalSourceNamespace, QueryRuntimeBinding, StorageError},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const HEAD: &[u8] = b"facts-current-head";
const MAX: usize = 8192;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    path: PathBuf,
    source: Identity,
    facts_version: u32,
    hostname: Option<String>,
    selection_known: bool,
    admission: u64,
    stream_epoch: String,
    runtime: String,
    activity: String,
}
fn identity(path: &Path) -> Result<Identity, StorageError> {
    Identity::of(&fs::symlink_metadata(path).map_err(|_| StorageError::ScopeChanged)?)
        .map_err(|_| StorageError::ScopeChanged)
}
fn load(tx: &mut PgTxn<'_>) -> Result<Option<Head>, StorageError> {
    native_keys::read_meta(tx, HEAD, MAX)?
        .map(|bytes| {
            let head: Head =
                serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
            let binding = model_cache::binding(tx)?;
            if head.admission != binding.admission || head.stream_epoch != binding.stream_epoch {
                #[cfg(test)]
                eprintln!("transfer_query_stage facts_head_binding_mismatch");
                return Err(StorageError::ScopeChanged);
            }
            Ok(head)
        })
        .transpose()
}
fn key(stamp: &[u8], relative: &str) -> Vec<u8> {
    [
        b"facts-current-member:".as_slice(),
        stamp,
        b"\0",
        relative.as_bytes(),
    ]
    .concat()
}
fn runtime_key(
    stamp: &[u8],
    dependency: &crate::native_evidence::runtime::RuntimeDependency,
) -> Result<Vec<u8>, StorageError> {
    let key = serde_json::to_vec(dependency).map_err(|_| StorageError::Unavailable)?;
    Ok([b"facts-current-runtime:".as_slice(), stamp, b"\0", &key].concat())
}
fn selected(tx: &mut PgTxn<'_>, head: &Head, relative: &str) -> Result<Option<bool>, StorageError> {
    if head.facts_version != FACTS_VERSION || !head.selection_known {
        return Ok(None);
    }
    let stamp = serde_json::to_vec(&head.source).map_err(|_| StorageError::Unavailable)?;
    Ok(Some(
        native_keys::read_meta(tx, &key(&stamp, relative), MAX)?.as_ref() == Some(&stamp),
    ))
}

/// Cold/replacement ingestion only. Both Raw reads and key construction finish
/// before the SQL writer reserves SourceState; the old head is a CAS witness.
pub(super) fn prepare(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    options: &crate::Options,
) -> Result<(), StorageError> {
    let path = options
        .facts
        .as_ref()
        .ok_or(StorageError::InvalidArguments)?;
    let activity = crate::native_evidence::recorded_activity_generation(options)
        .map_err(|_| StorageError::Unavailable)?;
    let source = identity(path)?;
    let (prior, binding) = pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
        Ok((load(tx)?, model_cache::binding(tx)?))
    })?;
    if prior.as_ref().is_some_and(|head| {
        head.path == *path && head.source == source && head.activity == activity
    }) {
        return Ok(());
    }
    let bytes = crate::read_regular_at_most(path, 64 * 1024 * 1024)
        .map_err(|_| StorageError::Unavailable)?;
    let facts: crate::Facts =
        serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    if identity(path)? != source {
        return Err(StorageError::Conflict);
    }
    let stamp = serde_json::to_vec(&source).map_err(|_| StorageError::Unavailable)?;
    let runtime = crate::native_evidence::recorded_runtime_fingerprint(options, &facts)
        .map_err(|_| StorageError::Unavailable)?;
    let runtime_parts =
        crate::native_evidence::RuntimeObservations::recorded(options, facts.clone())
            .projections()
            .map_err(|_| StorageError::Unavailable)?;
    if crate::native_evidence::recorded_activity_generation(options)
        .map_err(|_| StorageError::Unavailable)?
        != activity
    {
        return Err(StorageError::Conflict);
    }
    let members = facts
        .codex_rollouts
        .iter()
        .chain(facts.codex_provisional_rollouts.iter())
        .flat_map(|paths| paths.iter())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|path| key(&stamp, path))
        .collect::<Vec<_>>();
    // Immutable identity-addressed pages cannot change membership of the old
    // published header. Each SQL operation is at most 64 members / 1 MiB.
    for page in members.chunks(64) {
        let page_bytes = serde_json::to_vec(page).map_err(|_| StorageError::Unavailable)?;
        let completed = [
            b"facts-current-page:".as_slice(),
            &Sha256::digest(&page_bytes),
        ]
        .concat();
        if page
            .iter()
            .map(|key| key.len() + stamp.len())
            .sum::<usize>()
            + completed.len()
            + stamp.len()
            > 1024 * 1024
        {
            return Err(StorageError::Busy);
        }
        pool.transaction(namespace, TxnKind::Write, control, |tx| {
            let current = model_cache::binding(tx)?;
            if current.admission != binding.admission
                || current.stream_epoch != binding.stream_epoch
                || identity(path)? != source
            {
                return Err(StorageError::Conflict);
            }
            // The marker commits with its immutable members. A restarted
            // producer skips completed pages; an interrupted page rolls back.
            if native_keys::read_meta(tx, &completed, MAX)?.as_ref() == Some(&stamp) {
                return Ok(());
            }
            for member in page {
                tx.check()?;
                native_keys::put_meta(tx, member, &stamp)?;
            }
            if identity(path)? != source {
                return Err(StorageError::Conflict);
            }
            native_keys::put_meta(tx, &completed, &stamp)?;
            Ok(())
        })?;
    }
    // Runtime dependencies share this Facts head and its independent PID-file
    // activity stamp. Readers fetch only the requested native/name/repo keys.
    let runtime_stamp =
        serde_json::to_vec(&(&source, &activity)).map_err(|_| StorageError::Unavailable)?;
    let runtime_parts = runtime_parts
        .into_iter()
        .map(|(dependency, value)| Ok((runtime_key(&runtime_stamp, &dependency)?, value)))
        .collect::<Result<Vec<_>, StorageError>>()?;
    for page in runtime_parts.chunks(64) {
        let encoded = serde_json::to_vec(page).map_err(|_| StorageError::Unavailable)?;
        if encoded.len() > 1024 * 1024 {
            return Err(StorageError::Busy);
        }
        let completed = [
            b"facts-current-runtime-page:".as_slice(),
            &Sha256::digest(&encoded),
        ]
        .concat();
        pool.transaction(namespace, TxnKind::Write, control, |tx| {
            let current = model_cache::binding(tx)?;
            if current.admission != binding.admission
                || current.stream_epoch != binding.stream_epoch
                || identity(path)? != source
            {
                return Err(StorageError::Conflict);
            }
            if native_keys::read_meta(tx, &completed, MAX)?.as_ref() == Some(&runtime_stamp) {
                return Ok(());
            }
            for (key, value) in page {
                native_keys::put_meta(tx, key, value.as_bytes())?;
            }
            native_keys::put_meta(tx, &completed, &runtime_stamp)
        })?;
    }
    pool.transaction(namespace, TxnKind::Write, control, |tx| {
        tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&namespace.as_str().as_bytes()]).map_err(super::sqlstate::error)?;
        if load(tx)? != prior || identity(path)? != source { return Err(StorageError::Conflict); }
        let current = model_cache::binding(tx)?;
        if current.admission != binding.admission || current.stream_epoch != binding.stream_epoch { return Err(StorageError::Conflict); }
        let head = Head { path: path.to_owned(), source: source.clone(), facts_version: facts.version,
            hostname: (facts.hostname.len() <= 4096).then(|| facts.hostname.clone()),
            selection_known: facts.codex_rollouts.is_some(), admission: binding.admission, stream_epoch: binding.stream_epoch, runtime, activity };
        let encoded = serde_json::to_vec(&head).map_err(|_| StorageError::Unavailable)?;
        if encoded.len() > MAX { return Err(StorageError::Busy); }
        native_keys::put_meta(tx, HEAD, &encoded)?;
        if identity(path)? != source { return Err(StorageError::Conflict); }
        Ok(())
    })
}

pub(crate) struct CurrentSources {
    pool: PgPool,
    namespace: LogicalSourceNamespace,
    control: PgCancellation,
    head: Head,
    binding: QueryRuntimeBinding,
}

struct SnapshotSources<'a, 'b, 'tx> {
    snapshot: &'a super::catalog_effects::snapshot::Snapshot<'b, 'tx>,
    head: Head,
}
pub(in crate::storage::postgres) fn runtime_in_snapshot(
    snapshot: &super::catalog_effects::snapshot::Snapshot<'_, '_>,
    options: &crate::Options,
) -> Result<String, StorageError> {
    let Some(path) = &options.facts else {
        return crate::native_evidence::query_runtime_generation(options)
            .map_err(|_| StorageError::Unavailable);
    };
    let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
    let head = load(&mut tx)?.ok_or(StorageError::ScopeChanged)?;
    if head.path != *path
        || identity(path)? != head.source
        || crate::native_evidence::recorded_activity_generation(options)
            .map_err(|_| StorageError::Unavailable)?
            != head.activity
        || identity(path)? != head.source
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(head.runtime)
}

pub(super) fn runtime_scope(
    tx: &mut PgTxn<'_>,
    options: &crate::Options,
    row: &crate::model::summary::CatalogRow,
    runtime: &str,
    cwds: &std::collections::BTreeSet<String>,
) -> Result<String, StorageError> {
    let head = load(tx)?.ok_or(StorageError::ScopeChanged)?;
    if options.facts.as_ref() != Some(&head.path) || head.runtime != runtime {
        return Err(StorageError::ScopeChanged);
    }
    let stamp = serde_json::to_vec(&(&head.source, &head.activity))
        .map_err(|_| StorageError::Unavailable)?;
    let mut parts = Vec::new();
    for dependency in crate::native_evidence::runtime::dependencies(row, cwds) {
        let value = native_keys::read_meta(tx, &runtime_key(&stamp, &dependency)?, 64)?
            .map(|bytes| {
                let value = String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged)?;
                if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(StorageError::ScopeChanged);
                }
                Ok(value)
            })
            .transpose()?;
        parts.push((dependency, value));
    }
    crate::native_evidence::runtime::scope_parts(&parts).map_err(|_| StorageError::ScopeChanged)
}
pub(in crate::storage::postgres) fn in_snapshot<'a, 'b, 'tx>(
    snapshot: &'a super::catalog_effects::snapshot::Snapshot<'b, 'tx>,
    path: &Path,
) -> Result<Box<dyn crate::facts::source_authority::SourceAuthority + 'a>, StorageError> {
    let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
    let head = load(&mut tx)?.ok_or(StorageError::ScopeChanged)?;
    if head.path != path || identity(path)? != head.source {
        return Err(StorageError::ScopeChanged);
    }
    drop(tx);
    Ok(Box::new(SnapshotSources { snapshot, head }))
}
impl crate::facts::source_authority::SourceAuthority for SnapshotSources<'_, '_, '_> {
    fn validate(&self) -> io::Result<()> {
        let mut tx = self
            .snapshot
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        tx.check().map_err(io::Error::other)?;
        if load(&mut tx).map_err(io::Error::other)?.as_ref() != Some(&self.head)
            || identity(&self.head.path).map_err(io::Error::other)? != self.head.source
        {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        Ok(())
    }
    fn facts_known(&self) -> bool {
        self.head.facts_version == FACTS_VERSION
    }
    fn selection_known(&self) -> bool {
        self.facts_known() && self.head.selection_known
    }
    fn hostname(&self) -> Option<String> {
        self.facts_known()
            .then(|| self.head.hostname.clone())
            .flatten()
    }
    fn codex_current(&self, relative: &str) -> io::Result<Option<bool>> {
        self.validate()?;
        let mut tx = self
            .snapshot
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        selected(&mut tx, &self.head, relative).map_err(io::Error::other)
    }
}
impl CurrentSources {
    pub(crate) fn open(
        pool: PgPool,
        namespace: LogicalSourceNamespace,
        control: PgCancellation,
        path: &Path,
    ) -> Result<Self, StorageError> {
        let source = identity(path).inspect_err(|_| {
            #[cfg(test)]
            eprintln!("transfer_query_stage facts_identity_refused");
        })?;
        let (head, binding) =
            pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
                Ok((
                    load(tx)
                        .inspect_err(|_| {
                            #[cfg(test)]
                            eprintln!("transfer_query_stage facts_head_load_refused");
                        })?
                        .ok_or(StorageError::ScopeChanged)?,
                    model_cache::binding(tx)?,
                ))
            })?;
        if head.path != path || head.source != source || identity(path)? != source {
            #[cfg(test)]
            eprintln!("transfer_query_stage facts_identity_mismatch");
            return Err(StorageError::ScopeChanged);
        }
        Ok(Self {
            pool,
            namespace,
            control,
            head,
            binding,
        })
    }
    fn check(&self, tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
        let binding = model_cache::binding(tx)?;
        if binding.admission != self.binding.admission
            || binding.stream_epoch != self.binding.stream_epoch
            || load(tx)?.as_ref() != Some(&self.head)
            || identity(&self.head.path)? != self.head.source
        {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
    pub(crate) fn validate(&self) -> io::Result<()> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| self.check(tx),
            )
            .map_err(io::Error::other)
    }
    pub(crate) fn facts_known(&self) -> bool {
        self.head.facts_version == FACTS_VERSION
    }
    pub(crate) fn selection_known(&self) -> bool {
        self.facts_known() && self.head.selection_known
    }
    pub(crate) fn hostname(&self) -> Option<String> {
        self.facts_known()
            .then(|| self.head.hostname.clone())
            .flatten()
    }
    pub(crate) fn codex_current(&self, relative: &str) -> io::Result<Option<bool>> {
        self.pool
            .transaction(
                &self.namespace,
                TxnKind::ReadSnapshot,
                &self.control,
                |tx| {
                    self.check(tx)?;
                    let found = selected(tx, &self.head, relative)?;
                    if identity(&self.head.path)? != self.head.source {
                        return Err(StorageError::ScopeChanged);
                    }
                    Ok(found)
                },
            )
            .map_err(io::Error::other)
    }
}

/// SQL-only persisted Facts authority check. Physical Source qualification
/// remains separate; this predicate alone is not a publication grant.
pub(super) fn lifecycle_recorded(
    tx: &mut PgTxn<'_>,
    authority: &crate::model::relation_stage::LifecycleAuthority,
) -> Result<bool, StorageError> {
    // Facts begin reserves this namespace before allowing the host write;
    // completion replaces the head and clears the marker under the same lock.
    if native_keys::read_meta(tx, b"facts-write-inflight", 1048576)?.is_some() {
        return Ok(false);
    }
    let binding = model_cache::binding(tx)?;
    let source = crate::storage::HostSource::new(tx.namespace.clone(), binding.stream_epoch)?;
    if serde_json::to_vec(&source).map_err(|_| StorageError::ScopeChanged)?
        != serde_json::to_vec(&authority.source).map_err(|_| StorageError::ScopeChanged)?
    {
        return Ok(false);
    }
    let Some((path, identity)) = &authority.facts else {
        return Ok(true);
    };
    let Some(head) = load(tx)? else {
        return Ok(false);
    };
    Ok(head.path == *path
        && head.source == *identity
        && head.runtime == authority.runtime
        && Some(&head.activity) == authority.facts_activity.as_ref())
}

pub(super) fn withdrawal_recorded(
    tx: &mut PgTxn<'_>,
    withdrawal: &crate::model::relation_stage::Withdrawal,
) -> Result<bool, StorageError> {
    let Some(head) = load(tx)? else {
        return Ok(false);
    };
    Ok(head.path == withdrawal.facts_path
        && head.source == withdrawal.facts_identity
        && head.runtime == withdrawal.runtime
        && selected(tx, &head, &withdrawal.relative)? == Some(false))
}

/// One request's recorded runtime witness. Rechecks inside a leased read use
/// only the exact canonical identity and independent runtime-file stamps.
pub(crate) struct Runtime(Option<CurrentSources>);
impl Runtime {
    pub(crate) fn open(
        pool: &PgPool,
        namespace: &LogicalSourceNamespace,
        control: &PgCancellation,
        options: &crate::Options,
    ) -> Result<Self, StorageError> {
        options
            .facts
            .as_ref()
            .map(|path| {
                CurrentSources::open(pool.clone(), namespace.clone(), control.clone(), path)
            })
            .transpose()
            .map(Self)
    }
    pub(crate) fn generation(&self, options: &crate::Options) -> io::Result<String> {
        match &self.0 {
            None => crate::native_evidence::query_runtime_generation(options),
            Some(source) => {
                if identity(&source.head.path).map_err(io::Error::other)? != source.head.source
                    || crate::native_evidence::recorded_activity_generation(options)?
                        != source.head.activity
                {
                    return Err(io::Error::other("Recorded runtime authority changed"));
                }
                if identity(&source.head.path).map_err(io::Error::other)? != source.head.source {
                    return Err(io::Error::other("Recorded runtime authority changed"));
                }
                Ok(source.head.runtime.clone())
            }
        }
    }
}

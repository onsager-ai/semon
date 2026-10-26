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
        if page
            .iter()
            .map(|key| key.len() + stamp.len())
            .sum::<usize>()
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
            for member in page {
                tx.check()?;
                native_keys::put_meta(tx, member, &stamp)?;
            }
            if identity(path)? != source {
                return Err(StorageError::Conflict);
            }
            Ok(())
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
impl CurrentSources {
    pub(crate) fn open(
        pool: PgPool,
        namespace: LogicalSourceNamespace,
        control: PgCancellation,
        path: &Path,
    ) -> Result<Self, StorageError> {
        let source = identity(path)?;
        let (head, binding) =
            pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
                Ok((
                    load(tx)?.ok_or(StorageError::ScopeChanged)?,
                    model_cache::binding(tx)?,
                ))
            })?;
        if head.path != path || head.source != source || identity(path)? != source {
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

/// Durable withdrawal replay uses the caller's existing namespace transaction,
/// never another connection or a Raw read under the publication row locks.
pub(super) fn withdrawal_current(
    tx: &mut PgTxn<'_>,
    withdrawal: &crate::model::relation_stage::Withdrawal,
) -> Result<bool, StorageError> {
    let Some(head) = load(tx)? else {
        return Ok(false);
    };
    if head.path != withdrawal.facts_path
        || head.source != withdrawal.facts_identity
        || identity(&head.path)? != head.source
        || selected(tx, &head, &withdrawal.relative)? != Some(false)
    {
        return Ok(false);
    }
    let source =
        crate::open_input(Path::new(&withdrawal.path)).map_err(|_| StorageError::ScopeChanged)?;
    let before = crate::events::Stat::from_metadata(
        &source.metadata().map_err(|_| StorageError::ScopeChanged)?,
    )
    .map_err(|_| StorageError::ScopeChanged)?;
    let after = crate::events::Stat::of(Path::new(&withdrawal.path))
        .map_err(|_| StorageError::ScopeChanged)?;
    Ok(crate::is_input_path("codex", &withdrawal.relative)
        && withdrawal.relative.ends_with(".jsonl")
        && before == withdrawal.stat
        && after == before
        && identity(&head.path)? == head.source)
}

/// One request's recorded runtime witness. Rechecks inside a leased read use
/// only the exact canonical identity and independent runtime-file stamps.
pub(super) struct Runtime(Option<CurrentSources>);
impl Runtime {
    pub(super) fn open(
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
    pub(super) fn generation(&self, options: &crate::Options) -> io::Result<String> {
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

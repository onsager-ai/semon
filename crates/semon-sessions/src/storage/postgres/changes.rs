//! Durable hydration hints commit with the acknowledged Catalog publication.
use super::{
    model_cache, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{
    CatalogChangesRequest, LogicalSourceNamespace, MetadataReadSnapshot, SessionMetadataStore,
    StorageError, StoreBackend, StoreConfig,
};
use crate::{CatalogChange, CatalogChangeCompaction, CatalogChangePage, CatalogReadScope};

fn scope_key(scope: CatalogReadScope) -> &'static [u8] {
    match scope {
        CatalogReadScope::Current => b"current",
        CatalogReadScope::RetainedHistory => b"retained_history",
    }
}

/// The publisher holds SourceState through commit. Replayed publication ACKs
/// return before this call, so an uncertain commit cannot duplicate revisions.
pub(super) fn publish<'a>(
    tx: &mut PgTxn<'_>,
    keys: impl Iterator<Item = &'a str>,
) -> Result<(), StorageError> {
    let keys = keys.collect::<std::collections::BTreeSet<_>>();
    if keys.iter().any(|key| key.is_empty() || key.len() > 4096)
        || keys.iter().map(|key| key.len() + 32).sum::<usize>() * 2 > 1024 * 1024
    {
        return Err(StorageError::Busy);
    }
    let ns = tx.namespace.as_str().as_bytes();
    for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
        let scope = scope_key(scope);
        tx.driver.execute("INSERT INTO semon_pg_native_change_feed_state(namespace,read_scope,head,floor) VALUES($1,$2,0,0) ON CONFLICT DO NOTHING", &[&ns, &scope]).map_err(sqlstate::error)?;
        let mut head: i64 = tx.driver.query_one("SELECT head FROM semon_pg_native_change_feed_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND read_scope=$2 FOR UPDATE", &[&ns, &scope]).map_err(sqlstate::error)?.get(0);
        for key in &keys {
            tx.check()?;
            head = head.checked_add(1).ok_or(StorageError::Unavailable)?;
            tx.driver.execute("INSERT INTO semon_pg_native_change_feed(namespace,read_scope,revision,session_key) VALUES($1,$2,$3,$4)", &[&ns, &scope, &head, &key.as_bytes()]).map_err(sqlstate::error)?;
        }
        tx.driver.execute("UPDATE semon_pg_native_change_feed_state SET head=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND read_scope=$2", &[&ns, &scope, &head]).map_err(sqlstate::error)?;
    }
    Ok(())
}

pub(in crate::storage) struct MetadataStore(pub(in crate::storage) StoreConfig);
struct Snapshot {
    config: StoreConfig,
    epoch: String,
    generation: String,
    cursors: [(u64, u64); 2],
}
fn index(scope: CatalogReadScope) -> usize {
    match scope {
        CatalogReadScope::Current => 0,
        CatalogReadScope::RetainedHistory => 1,
    }
}
fn cursor(tx: &mut PgTxn<'_>, scope: CatalogReadScope) -> Result<(u64, u64), StorageError> {
    let rows = tx.driver.query("SELECT head,floor FROM semon_pg_native_change_feed_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND read_scope=$2", &[&tx.namespace.as_str().as_bytes(), &scope_key(scope)]).map_err(sqlstate::error)?;
    let row = rows.first().ok_or(StorageError::ScopeChanged)?;
    let head = u64::try_from(row.get::<_, i64>(0)).map_err(|_| StorageError::ScopeChanged)?;
    let floor = u64::try_from(row.get::<_, i64>(1)).map_err(|_| StorageError::ScopeChanged)?;
    if floor > head {
        return Err(StorageError::ScopeChanged);
    }
    Ok((head, floor))
}
fn check(tx: &mut PgTxn<'_>, config: &StoreConfig) -> Result<String, StorageError> {
    let binding = model_cache::binding(tx)?;
    if config.expected_stream_epoch.as_deref() != Some(binding.stream_epoch.as_str()) {
        return Err(StorageError::ScopeChanged);
    }
    Ok(binding.stream_epoch)
}
impl SessionMetadataStore for MetadataStore {
    fn read_snapshot(
        &self,
        namespace: &LogicalSourceNamespace,
    ) -> Result<Box<dyn MetadataReadSnapshot>, StorageError> {
        if namespace != &self.0.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        let StoreBackend::Postgres { pool, control } = &self.0.backend else {
            return Err(StorageError::InvalidArguments);
        };
        pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
            let epoch = check(tx, &self.0)?;
            let generation = model_cache::generation(tx)?.ok_or(StorageError::ScopeChanged)?;
            let cursors = [
                cursor(tx, CatalogReadScope::Current)?,
                cursor(tx, CatalogReadScope::RetainedHistory)?,
            ];
            Ok(Box::new(Snapshot {
                config: self.0.clone(),
                epoch,
                generation,
                cursors,
            }) as Box<dyn MetadataReadSnapshot>)
        })
    }
    fn compact_catalog_changes(
        &self,
        namespace: &LogicalSourceNamespace,
        scope: CatalogReadScope,
        epoch: &str,
        acknowledged: u64,
        limit: usize,
    ) -> Result<CatalogChangeCompaction, StorageError> {
        if namespace != &self.0.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        if limit == 0 || limit > 256 || acknowledged > i64::MAX as u64 {
            return Err(StorageError::InvalidArguments);
        }
        let StoreBackend::Postgres { pool, control } = &self.0.backend else {
            return Err(StorageError::InvalidArguments);
        };
        pool.transaction(namespace, TxnKind::Write, control, |tx| {
            tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
            if check(tx, &self.0)? != epoch { return Err(StorageError::ScopeChanged); }
            let (head, floor) = cursor(tx, scope)?;
            if acknowledged > head { return Err(StorageError::ScopeChanged); }
            let ns = namespace.as_str().as_bytes();
            let scope = scope_key(scope);
            let revisions = tx.driver.query("SELECT revision FROM semon_pg_native_change_feed WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND read_scope=$2 AND revision<=$3 AND revision<$4 ORDER BY revision LIMIT $5", &[&ns, &scope, &(acknowledged as i64), &(head as i64), &(limit as i64 + 1)]).map_err(sqlstate::error)?;
            let removed = revisions.len().min(limit);
            let through = revisions.iter().take(limit).next_back().map_or(floor, |row| row.get::<_, i64>(0) as u64);
            if removed > 0 {
                tx.driver.execute("DELETE FROM semon_pg_native_change_feed WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND read_scope=$2 AND revision<=$3", &[&ns, &scope, &(through as i64)]).map_err(sqlstate::error)?;
                tx.driver.execute("UPDATE semon_pg_native_change_feed_state SET floor=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND read_scope=$2", &[&ns, &scope, &(through as i64)]).map_err(sqlstate::error)?;
            }
            Ok(CatalogChangeCompaction { removed, compacted_through: through, head, more: revisions.len() > limit })
        })
    }
}
impl MetadataReadSnapshot for Snapshot {
    fn catalog_changes(
        &mut self,
        namespace: &LogicalSourceNamespace,
        request: CatalogChangesRequest<'_>,
    ) -> Result<CatalogChangePage, StorageError> {
        request.validate()?;
        if namespace != &self.config.namespace {
            return Err(StorageError::NamespaceMismatch);
        }
        let (head, floor) = self.cursors[index(request.scope)];
        if request
            .stream_epoch
            .is_some_and(|epoch| epoch != self.epoch)
            || request.after < floor
            || request.after > head
        {
            return Err(StorageError::ScopeChanged);
        }
        let StoreBackend::Postgres { pool, control } = &self.config.backend else {
            return Err(StorageError::InvalidArguments);
        };
        pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
            if check(tx, &self.config)? != self.epoch || cursor(tx, request.scope)?.1 > request.after {
                return Err(StorageError::ScopeChanged);
            }
            // Revisions are immutable. Pinning head/generation gives coherent
            // pages without holding a connection between caller requests;
            // compaction invalidates the snapshot rather than losing hints.
            let rows = tx.driver.query("SELECT revision,CASE WHEN octet_length(session_key)<=4096 THEN session_key END FROM semon_pg_native_change_feed WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND read_scope=$2 AND revision>$3 AND revision<=$4 ORDER BY revision LIMIT $5", &[&namespace.as_str().as_bytes(), &scope_key(request.scope), &(request.after as i64), &(head as i64), &(request.limit as i64)]).map_err(sqlstate::error)?;
            let changes = rows.iter().map(|row| Ok(CatalogChange {
                revision: row.get::<_, i64>(0) as u64,
                catalog_key: String::from_utf8(row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::ScopeChanged)?).map_err(|_| StorageError::ScopeChanged)?,
                read_scope: request.scope,
            })).collect::<Result<Vec<_>, StorageError>>()?;
            let next = changes.last().map_or(request.after, |change| change.revision);
            Ok(CatalogChangePage { api: 1, source_key: namespace.as_str().into(), stream_epoch: self.epoch.clone(), source_generation: self.generation.clone(), read_scope: request.scope, head, compacted_through: floor, next_revision: next, caught_up: next == head, changes })
        })
    }
}

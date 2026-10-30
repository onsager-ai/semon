//! Backend-neutral public change delivery helpers for the local source binding.
use crate::{
    CatalogIdentityError, CatalogReadScope, Options,
    storage::{CatalogChangesRequest, LogicalSourceNamespace},
};
use serde::{Deserialize, Serialize};

/// A changed key is a hydration hint, not a native identity or an authorization.
/// Consumers hydrate against this page's source generation and use absence or
/// retained lifecycle as a current-list tombstone. Repeated keys are idempotent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogChange {
    pub revision: u64,
    pub catalog_key: String,
    pub read_scope: CatalogReadScope,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogChangePage {
    pub api: u32,
    pub source_key: String,
    pub stream_epoch: String,
    pub source_generation: String,
    pub read_scope: CatalogReadScope,
    pub head: u64,
    pub compacted_through: u64,
    pub next_revision: u64,
    pub caught_up: bool,
    pub changes: Vec<CatalogChange>,
}

/// Result of one explicitly requested, bounded acknowledgement batch. Hosts
/// supply the least durable acknowledgement of every consumer of this scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogChangeCompaction {
    pub removed: usize,
    pub compacted_through: u64,
    pub head: u64,
    pub more: bool,
}

/// Compact at most 256 acknowledged hints. The existing local API binds one
/// configured source; shared-database namespace admission is not implemented yet.
pub fn compact_session_catalog_changes(
    options: &Options,
    scope: CatalogReadScope,
    stream_epoch: &str,
    acknowledged_through: u64,
    limit: usize,
) -> Result<CatalogChangeCompaction, CatalogIdentityError> {
    if limit == 0 || limit > 256 || acknowledged_through > i64::MAX as u64 {
        return Err(CatalogIdentityError::InvalidArguments);
    }
    let config = options.source.config(&options.cache)?;
    let namespace = config.namespace.clone();
    let store = config.open()?;
    store
        .compact_catalog_changes(&namespace, scope, stream_epoch, acknowledged_through, limit)
        .map_err(Into::into)
}

/// Metadata-only source outbox read. The caller authorizes this exact local
/// binding; namespace labels do not themselves grant source access.
pub fn session_catalog_changes(
    options: &Options,
    source_key: &str,
    scope: CatalogReadScope,
    stream_epoch: Option<&str>,
    after: u64,
    limit: usize,
) -> Result<CatalogChangePage, CatalogIdentityError> {
    let namespace = LogicalSourceNamespace::new(source_key)?;
    let request = CatalogChangesRequest {
        scope,
        stream_epoch,
        after,
        limit,
    };
    request.validate()?;
    let store = options.source.config(&options.cache)?.open()?;
    store
        .read_snapshot(&namespace)?
        .catalog_changes(&namespace, request)
        .map_err(Into::into)
}

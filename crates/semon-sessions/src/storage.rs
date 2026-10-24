//! Explicit metadata storage boundary. This first increment covers change
//! delivery only; Index, Catalog, Query and byte-proof ports remain in #478.
//! A namespace is an authorized logical binding, never a path or a credential.
#[cfg(test)]
mod conformance;
pub(crate) mod sqlite;
pub(crate) mod query;
pub(crate) mod catalog;

pub(crate) fn read_local_catalog_page(options: &crate::Options, source_key: &str, request: &crate::catalog::Request) -> Result<crate::ViewerReply, Box<dyn std::error::Error>> {
    sqlite::catalog::read_page(options, source_key, request)
}

pub(crate) fn source_proof_observation(options: &crate::Options, root: &str, absolute: &str) -> Result<Option<(String, crate::model::summary::CatalogSource, String)>, Box<dyn std::error::Error>> {
    sqlite::catalog::source_proof_observation(options, root, absolute)
}

pub(crate) fn local_query_catalog(options: &crate::Options, namespace: LogicalSourceNamespace) -> Box<dyn query::QueryCatalogStore> {
    Box::new(sqlite::query::SqliteQueryCatalog { options: options.clone(), namespace })
}

use crate::{CatalogChangeCompaction, CatalogChangePage, CatalogIdentityError, CatalogReadScope};
use std::path::PathBuf;

/// Opaque host-assigned source/access namespace, independent of Raw roots and
/// database coordinates. Hosts must authorize its binding before opening a store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogicalSourceNamespace(String);

impl LogicalSourceNamespace {
    pub fn new(value: impl Into<String>) -> Result<Self, StorageError> {
        let value = value.into();
        if value.is_empty() || value.len() > 4096 {
            return Err(StorageError::InvalidArguments);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Only implemented backends are selectable. No PostgreSQL driver is required
/// by SQLite. #479 adds PostgreSQL after the full native boundary is ported.
#[derive(Clone, Debug)]
pub enum StoreBackend {
    Sqlite { database: PathBuf },
}

/// Explicit physical coordinates and logical binding. A discovered epoch must
/// be persisted by the host and supplied on subsequent admissions; changing
/// coordinates must not silently adopt a different stream.
#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub backend: StoreBackend,
    pub namespace: LogicalSourceNamespace,
    pub expected_stream_epoch: Option<String>,
}

impl StoreConfig {
    /// Resolve configuration without creating, resetting or repairing storage.
    /// Every operation opens the configured backend or returns an explicit error.
    pub fn open(self) -> Result<Box<dyn SessionMetadataStore>, StorageError> {
        if self.expected_stream_epoch.as_ref().is_some_and(|epoch| {
            epoch.len() != 64 || !epoch.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err(StorageError::InvalidArguments);
        }
        match &self.backend {
            StoreBackend::Sqlite { database } => {
                if database.as_os_str().is_empty() || database == std::path::Path::new(":memory:") {
                    return Err(StorageError::InvalidArguments);
                }
            }
        }
        Ok(Box::new(sqlite::SqliteMetadataStore::new(self)))
    }

    /// Explicitly bind a supported current single-source store. This stages
    /// admission and fences native writes until their namespace port completes;
    /// it is not a finished row migration or permission to admit another source.
    pub fn admit_namespace(&self) -> Result<NamespaceAdmission, StorageError> {
        sqlite::namespace::admit(self)
    }

    pub(crate) fn open_local(self) -> Result<Box<dyn SessionMetadataStore>, StorageError> {
        Ok(Box::new(sqlite::SqliteMetadataStore::legacy_local(self)))
    }

    /// Transitional one-source local binding. Only this existing API bridge
    /// derives coordinates from Options.cache; the contract never receives it.
    pub(crate) fn local_metadata(
        options: &crate::Options,
        namespace: LogicalSourceNamespace,
    ) -> Self {
        Self {
            backend: StoreBackend::Sqlite {
                database: crate::events::EventCache::path(&options.cache),
            },
            namespace,
            expected_stream_epoch: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError {
    InvalidArguments,
    Unavailable,
    Busy,
    ScopeChanged,
    NamespaceMismatch,
    NamespaceUnadmitted,
    NamespaceUpgradePending,
    MultiSourceUnqualified,
    /// The client must reconcile epoch/floor and retry the idempotent scoped
    /// acknowledgement. It must not infer rollback from a missing commit reply.
    CommitUncertain,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidArguments => "Invalid storage arguments.",
            Self::Unavailable => "Configured storage is unavailable.",
            Self::Busy => "Configured storage is busy.",
            Self::ScopeChanged => "Storage epoch or scope changed.",
            Self::NamespaceMismatch => "Logical storage namespace changed.",
            Self::NamespaceUnadmitted => "Logical storage namespace is not admitted.",
            Self::NamespaceUpgradePending => "Native namespace writer upgrade is pending.",
            Self::MultiSourceUnqualified => "Multi-source writers are not qualified.",
            Self::CommitUncertain => "Storage commit requires reconciliation.",
        })
    }
}

/// Durable admission witness, independent of a database row ID or pathname.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceAdmission {
    pub namespace: LogicalSourceNamespace,
    pub generation: u64,
    pub admitted_at: i64,
}
impl std::error::Error for StorageError {}

impl From<StorageError> for CatalogIdentityError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::InvalidArguments => Self::InvalidArguments,
            StorageError::ScopeChanged | StorageError::NamespaceMismatch => Self::ScopeChanged,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CatalogChangesRequest<'a> {
    pub scope: CatalogReadScope,
    pub stream_epoch: Option<&'a str>,
    pub after: u64,
    pub limit: usize,
}

impl CatalogChangesRequest<'_> {
    pub(crate) fn validate(self) -> Result<(), StorageError> {
        if self.limit == 0
            || self.limit > 256
            || self.after > i64::MAX as u64
            || (self.after > 0 && self.stream_epoch.is_none())
        {
            return Err(StorageError::InvalidArguments);
        }
        Ok(())
    }
}

/// A request-owned coherent snapshot. Drop releases it on success, refusal,
/// cancellation or unwind. No driver connection escapes the adapter.
pub trait MetadataReadSnapshot {
    fn catalog_changes(
        &mut self,
        namespace: &LogicalSourceNamespace,
        request: CatalogChangesRequest<'_>,
    ) -> Result<CatalogChangePage, StorageError>;
}

/// Semantic operations, not an arbitrary SQL facade. SQLite currently binds one
/// authorized namespace per database; shared-database namespace isolation and
/// native publication transactions remain explicit undelivered #478 work.
pub trait SessionMetadataStore {
    fn read_snapshot(
        &self,
        namespace: &LogicalSourceNamespace,
    ) -> Result<Box<dyn MetadataReadSnapshot>, StorageError>;

    /// One explicit write transaction: check epoch, delete a bounded acknowledged
    /// prefix, advance its scoped floor, commit. Retry is idempotent. This is not
    /// a transaction spanning Raw, Index or Catalog publication.
    fn compact_catalog_changes(
        &self,
        namespace: &LogicalSourceNamespace,
        scope: CatalogReadScope,
        stream_epoch: &str,
        acknowledged_through: u64,
        limit: usize,
    ) -> Result<CatalogChangeCompaction, StorageError>;
}

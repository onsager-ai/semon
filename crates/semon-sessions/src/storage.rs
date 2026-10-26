//! Explicit metadata storage boundary. This first increment covers change
//! delivery only; Index, Catalog, Query and byte-proof ports remain in #478.
//! A namespace is an authorized logical binding, never a path or a credential.
pub(crate) mod catalog;
#[cfg(test)]
mod conformance;
mod host_source;
pub mod native_events;
pub(crate) mod navigation;
#[cfg(feature = "postgres")]
pub mod postgres;
pub(crate) mod proof_codec;
pub(crate) mod proof_lifetime;
pub(crate) mod query;
pub(crate) mod selected;
pub(crate) mod semantic_owner;
pub mod source_observations;
pub use host_source::HostSource;

fn local_database(cache: &std::path::Path) -> std::path::PathBuf {
    crate::events::EventCache::path(cache)
}
pub(crate) mod sqlite;

pub(crate) fn read_local_catalog_page(
    options: &crate::Options,
    source_key: &str,
    request: &crate::catalog::Request,
) -> Result<crate::ViewerReply, Box<dyn std::error::Error>> {
    let binding = local_reader_binding(options)?;
    sqlite::catalog::read_page(&binding, options, source_key, request)
}

pub(crate) fn source_proof_observation(
    options: &crate::Options,
    root: &str,
    absolute: &str,
) -> Result<Option<catalog::SourceProofObservation>, Box<dyn std::error::Error>> {
    let binding = local_reader_binding(options)?;
    sqlite::catalog::source_proof_observation(&binding, options, root, absolute)
}

pub(crate) fn local_query_catalog(
    options: &crate::Options,
    _namespace: LogicalSourceNamespace,
) -> Result<Box<dyn query::QueryCatalogStore>, StorageError> {
    Ok(Box::new(sqlite::query::SqliteQueryCatalog {
        options: options.clone(),
        binding: local_reader_binding(options)?,
    }))
}

/// The only Options bridge is explicit and cannot enter an enrolled store.
#[derive(Clone, Debug)]
pub(crate) enum NativeReaderBinding {
    Admitted(StoreConfig),
    /// Private producer custody; never constructed by public configuration.
    Semantic(crate::events::store::configured::NativeOwnership),
}

impl NativeReaderBinding {
    pub(crate) fn runtime_binding(&self) -> Result<Option<QueryRuntimeBinding>, StorageError> {
        match self {
            Self::Semantic(owner) => Ok(Some(QueryRuntimeBinding {
                namespace: owner.config().namespace.clone(),
                admission: owner.admission(),
                stream_epoch: owner
                    .config()
                    .expected_stream_epoch
                    .clone()
                    .ok_or(StorageError::ScopeChanged)?,
            })),
            Self::Admitted(_) => Ok(None),
        }
    }

    pub(crate) fn config(&self) -> &StoreConfig {
        match self {
            Self::Admitted(config) => config,
            Self::Semantic(owner) => owner.config(),
        }
    }
}

/// Logical runtime observation fence. Physical database coordinates are absent.
#[derive(Clone, Debug)]
pub(crate) struct QueryRuntimeBinding {
    pub(crate) namespace: LogicalSourceNamespace,
    pub(crate) admission: u64,
    pub(crate) stream_epoch: String,
}

impl QueryRuntimeBinding {
    pub(crate) fn witness(&self, runtime: &str) -> std::io::Result<String> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            "semon-source-runtime-v1",
            self.namespace.as_str(),
            self.admission,
            &self.stream_epoch,
            runtime,
        ))
        .map_err(std::io::Error::other)?;
        Ok(format!("source-runtime-v1:{:x}", Sha256::digest(bytes)))
    }
}

pub(crate) fn local_navigation_binding(
    options: &crate::Options,
    _source: &str,
) -> Result<NativeReaderBinding, StorageError> {
    local_reader_binding(options)
}

fn local_reader_binding(options: &crate::Options) -> Result<NativeReaderBinding, StorageError> {
    let config = options.source.config(&options.cache)?;
    Ok(NativeReaderBinding::Semantic(
        crate::events::store::configured::NativeOwnership::restore(&config)?,
    ))
}

pub(crate) fn local_catalog_generation(
    options: &crate::Options,
    source: &str,
) -> Result<String, StorageError> {
    navigation_store(local_navigation_binding(options, source)?).catalog_generation()
}

pub(crate) fn navigation_store(
    binding: NativeReaderBinding,
) -> Box<dyn navigation::NativeNavigationStore> {
    Box::new(sqlite::navigation::SqliteNavigation(binding))
}

impl NativeReaderBinding {
    pub(crate) fn bookmark_page(
        &self,
        options: &crate::Options,
        source: &str,
        query: &str,
    ) -> crate::ViewerReply {
        let request = match crate::catalog::Request::parse(query) {
            Ok(request) => request,
            Err(reply) => return reply,
        };
        sqlite::catalog::read_page(self, options, source, &request).unwrap_or_else(|_| crate::catalog::error(503, "catalog_unavailable", "The session catalog is unavailable. Retry after the background observation completes.", true))
    }
    pub(crate) fn bookmark_identity(
        &self,
        options: &crate::Options,
        source: &str,
        key: &str,
    ) -> Result<Option<crate::CatalogSessionIdentity>, CatalogIdentityError> {
        sqlite::catalog::history_identity(self, options, source, key)
    }
}

pub(crate) fn configured_query_catalog(
    options: &crate::Options,
    config: StoreConfig,
) -> Result<Box<dyn query::QueryCatalogStore>, StorageError> {
    #[cfg(feature = "postgres")]
    if let StoreBackend::Postgres { pool, control } = &config.backend {
        return postgres::configured_query_catalog(options, &config, pool.clone(), control.clone());
    }
    Ok(Box::new(sqlite::query::SqliteQueryCatalog {
        options: options.clone(),
        binding: NativeReaderBinding::Admitted(config),
    }))
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

/// Explicit host-owned storage resources. PostgreSQL never derives credentials
/// or database placement from the logical Source namespace or Raw root.
#[derive(Clone)]
pub enum StoreBackend {
    Sqlite { database: PathBuf },
    #[cfg(feature = "postgres")]
    Postgres {
        pool: postgres::PgPool,
        control: postgres::PgCancellation,
    },
}

impl std::fmt::Debug for StoreBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite { database } => f.debug_struct("Sqlite").field("database", database).finish(),
            #[cfg(feature = "postgres")]
            Self::Postgres { .. } => f.write_str("Postgres"),
        }
    }
}

impl StoreBackend {
    pub(crate) fn sqlite_database(&self) -> Result<&PathBuf, StorageError> {
        match self {
            Self::Sqlite { database } => Ok(database),
            #[cfg(feature = "postgres")]
            Self::Postgres { .. } => Err(StorageError::InvalidArguments),
        }
    }
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
    /// Explicitly create a fresh native store at a previously absent path.
    /// Requires a host-chosen epoch. Existing stores are never reset or copied;
    /// admission and Catalog namespace assignment are separate operations.
    pub fn create_native_store(&self) -> Result<(), StorageError> {
        crate::events::store::configured::create(self)
    }

    /// Explicit single-source Event/ledger ingestion. Hosts authorize the Raw
    /// path independently; this does not publish Catalog or enable multi-source.
    pub fn native_events(&self) -> Result<Box<dyn native_events::NativeEventStore>, StorageError> {
        match &self.backend {
            StoreBackend::Sqlite { .. } => crate::events::store::configured::prepare(self)?,
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { .. } => {},
        }
        Ok(Box::new(crate::events::EventCache::open_configured(self)?))
    }

    /// Open the configured source-observation writer. This does not enable
    /// Event commits or Catalog publication; those families remain fenced.
    pub fn source_observations(
        &self,
    ) -> Result<Box<dyn source_observations::SourceObservationStore>, StorageError> {
        match &self.backend {
            StoreBackend::Sqlite { .. } => sqlite::source_journal::open(self),
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => Ok(Box::new(postgres::PgSourceJournal::configured(
                pool.clone(), self.namespace.clone(), control.clone(),
            ))),
        }
    }

    /// Resolve configuration without creating, resetting or repairing storage.
    /// Every operation opens the configured backend or returns an explicit error.
    pub fn open(self) -> Result<Box<dyn SessionMetadataStore>, StorageError> {
        if self.expected_stream_epoch.is_none() {
            return Err(StorageError::InvalidArguments);
        }
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
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { .. } => return Err(StorageError::NamespaceUpgradePending),
        }
        Ok(Box::new(sqlite::SqliteMetadataStore::new(self)))
    }

    /// Explicitly bind a supported current single-source store. This stages
    /// admission and fences native writes until their namespace port completes;
    /// it is not a finished row migration or permission to admit another source.
    pub fn admit_namespace(&self) -> Result<NamespaceAdmission, StorageError> {
        sqlite::namespace::admit(self)
    }

    /// Undo the read-only admission stage for its exact first-source witness.
    /// Refuses once a qualified row migration has started; this is not general
    /// migration rollback and never discards native rows or accepted writes.
    pub fn unenroll_namespace(&self, admission: &NamespaceAdmission) -> Result<(), StorageError> {
        sqlite::namespace::unenroll(self, admission)
    }

    /// Read a native Catalog through durable namespace and epoch admission.
    /// Options supplies Raw/runtime authority only, never database coordinates.
    pub fn catalog_page(
        &self,
        options: &crate::Options,
        query: &str,
    ) -> Result<crate::ViewerReply, StorageError> {
        let request =
            crate::catalog::Request::parse(query).map_err(|_| StorageError::InvalidArguments)?;
        let binding = NativeReaderBinding::Admitted(self.clone());
        sqlite::catalog::read_page(&binding, options, self.namespace.as_str(), &request).map_err(
            |error| {
                error
                    .downcast::<StorageError>()
                    .map_or(StorageError::Unavailable, |error| *error)
            },
        )
    }

    /// Resolve metadata-only native navigation under this admitted namespace.
    pub fn resolve_native(
        &self,
        options: &crate::Options,
        harness: &str,
        native_id: &str,
        scope: CatalogReadScope,
        expected_generation: Option<&str>,
    ) -> Result<crate::CatalogNativeResolution, StorageError> {
        let store = navigation_store(NativeReaderBinding::Admitted(self.clone()));
        crate::native_resolution::resolve_with_store(
            options,
            self.namespace.as_str(),
            harness,
            native_id,
            scope,
            expected_generation,
            store.as_ref(),
        )
    }

    /// Read the published Catalog generation under this admitted namespace and
    /// stream epoch snapshot.
    pub fn catalog_generation(&self) -> Result<String, StorageError> {
        navigation_store(NativeReaderBinding::Admitted(self.clone())).catalog_generation()
    }

    /// Bounded metadata-only permalink target lookup in this admitted source.
    pub fn bookmarked_target_keys(&self, target: &str) -> Result<Vec<String>, StorageError> {
        navigation_store(NativeReaderBinding::Admitted(self.clone())).bookmarked_target_keys(target)
    }

    pub fn session_bookmark(&self, options: &crate::Options, query: &str) -> crate::ViewerReply {
        crate::session_bookmark::with_binding(
            options,
            self.namespace.as_str(),
            query,
            &NativeReaderBinding::Admitted(self.clone()),
        )
    }

    /// Bounded native Query publication metadata from this configured source.
    /// This does not fetch source-backed question text or replace Query tools.
    pub fn query_metadata(
        &self,
        options: &crate::Options,
        now: i64,
        max_bytes: usize,
    ) -> Result<serde_json::Value, crate::QueryError> {
        if max_bytes == 0 || max_bytes > 8 * 1024 * 1024 {
            return Err(crate::QueryError::new(
                "invalid_arguments",
                "Invalid metadata byte bound",
            ));
        }
        let store = configured_query_catalog(options, self.clone())
            .map_err(|error| crate::QueryError::new("unavailable", error.to_string()))?;
        let mut left = max_bytes;
        let part = store.read_cohort(&self.namespace, now, &mut left)?;
        store.validate(&self.namespace, &part)?;
        Ok(
            serde_json::json!({"source_key":self.namespace.as_str(),"generation":part.generation,"runtime":part.runtime,"rows":part.rows}),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError {
    InvalidArguments,
    Conflict,
    Unavailable,
    Busy,
    ScopeChanged,
    NamespaceMismatch,
    NamespaceUnadmitted,
    NamespaceUpgradePending,
    /// Retained History exceeds the bounded immutable-generation contract.
    HistoryRetentionBounds,
    MultiSourceUnqualified,
    /// Reconcile the epoch and operation's durable observations before retrying.
    /// A missing commit reply does not establish rollback.
    CommitUncertain,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidArguments => "Invalid storage arguments.",
            Self::Conflict => "Source ledger or observed revision changed.",
            Self::Unavailable => "Configured storage is unavailable.",
            Self::Busy => "Configured storage is busy.",
            Self::ScopeChanged => "Storage epoch or scope changed.",
            Self::NamespaceMismatch => "Logical storage namespace changed.",
            Self::NamespaceUnadmitted => "Logical storage namespace is not admitted.",
            Self::NamespaceUpgradePending => "Native namespace writer upgrade is pending.",
            Self::HistoryRetentionBounds => "Retained History exceeds storage bounds.",
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

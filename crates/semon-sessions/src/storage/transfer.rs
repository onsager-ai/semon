//! Current-format transfer inbox. Staging is durable, but is deliberately not
//! an ownership flip or an acknowledgement that native state is qualified.
//! Adapters keep the inbox in the namespace-owned metadata region; they never
//! interpret a page as SQL or replay Raw bodies.
use super::{StorageError, StoreBackend, StoreConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FORMAT_VERSION: u32 = 1;
pub const MAX_PAGE_ROWS: usize = 64;
pub const MAX_PAGE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_MANIFEST_BYTES: usize = 32768;

/// Logical inventory, independent of either adapter's physical tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    Events,
    Ledgers,
    NativeHeaders,
    NativeSidecars,
    Usage,
    ReportedFacts,
    SourceObservations,
    Custody,
    Generations,
    RawIntents,
    RawCompletions,
    Receipts,
    Operations,
    Claims,
    StagedWork,
    Checkpoints,
    CatalogCurrent,
    CatalogHistory,
    Slots,
    Recipes,
    PageBlobs,
    PageGenerations,
    PageReferences,
    Proofs,
    ProofReferences,
    Search,
    Trace,
    SourceState,
    SourceMembers,
    Invalidations,
    Mutations,
    Relocations,
    Tree,
    ChangeHeads,
    ChangeFloors,
    Metadata,
}

impl Category {
    pub const ALL: &'static [Self] = &[
        Self::Events, Self::Ledgers, Self::NativeHeaders, Self::NativeSidecars,
        Self::Usage, Self::ReportedFacts, Self::SourceObservations, Self::Custody,
        Self::Generations, Self::RawIntents, Self::RawCompletions, Self::Receipts,
        Self::Operations, Self::Claims, Self::StagedWork, Self::Checkpoints,
        Self::CatalogCurrent, Self::CatalogHistory, Self::Slots, Self::Recipes,
        Self::PageBlobs, Self::PageGenerations, Self::PageReferences, Self::Proofs,
        Self::ProofReferences, Self::Search, Self::Trace, Self::SourceState,
        Self::SourceMembers, Self::Invalidations, Self::Mutations, Self::Relocations,
        Self::Tree, Self::ChangeHeads, Self::ChangeFloors, Self::Metadata,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryManifest {
    pub category: Category,
    pub rows: u64,
    pub digest: [u8; 32],
}

/// Source identity must come from a quiesced authoritative checkpoint. An inbox
/// does not manufacture that checkpoint or authorize source/target ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotManifest {
    pub version: u32,
    pub namespace: String,
    pub stream_epoch: String,
    pub binding_epoch: u64,
    pub revision: u64,
    pub categories: Vec<CategoryManifest>,
    pub digest: [u8; 32],
}

impl SnapshotManifest {
    pub fn seal(&mut self) -> Result<(), StorageError> {
        self.digest = self.content_digest()?;
        self.validate()
    }

    fn content_digest(&self) -> Result<[u8; 32], StorageError> {
        hash(&(
            "semon-current-transfer-manifest-v1", self.version, &self.namespace,
            &self.stream_epoch, self.binding_epoch, self.revision, &self.categories,
        ))
    }

    pub fn validate(&self) -> Result<(), StorageError> {
        if self.version != FORMAT_VERSION || self.namespace.is_empty()
            || self.namespace.len() > 4096 || self.binding_epoch == 0
            || self.stream_epoch.len() != 64
            || !self.stream_epoch.bytes().all(|b| b.is_ascii_hexdigit())
            || self.categories.len() != Category::ALL.len()
            || !self.categories.iter().map(|c| c.category).eq(Category::ALL.iter().copied())
            || self.digest != self.content_digest()?
        {
            return Err(StorageError::InvalidArguments);
        }
        Ok(())
    }

    pub(crate) fn bind(&self, config: &StoreConfig) -> Result<(), StorageError> {
        self.validate()?;
        if self.namespace != config.namespace.as_str() {
            return Err(StorageError::NamespaceMismatch);
        }
        if config.expected_stream_epoch.as_deref() != Some(self.stream_epoch.as_str()) {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
}

/// First typed inbox records. Other inventory categories are not yet encoded;
/// their presence in a manifest is not evidence of transfer implementation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TransferRecord {
    Metadata { key: String, value: String },
    ReportedFact(crate::facts::ReportedRunSnapshot),
    SourceIntent(super::source_observations::SourceIntent),
}

impl TransferRecord {
    fn category(&self) -> Category {
        match self {
            Self::Metadata { .. } => Category::Metadata,
            Self::ReportedFact(_) => Category::ReportedFacts,
            Self::SourceIntent(_) => Category::SourceObservations,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferPage {
    pub snapshot: [u8; 32],
    pub sequence: u64,
    pub operation: [u8; 32],
    pub previous: [u8; 32],
    pub category: Category,
    pub records: Vec<TransferRecord>,
    pub digest: [u8; 32],
}

impl TransferPage {
    pub fn seal(&mut self) -> Result<(), StorageError> {
        self.digest = self.content_digest()?;
        self.encoded().map(|_| ())
    }

    fn content_digest(&self) -> Result<[u8; 32], StorageError> {
        hash(&(
            "semon-current-transfer-page-v1", self.snapshot, self.sequence,
            self.operation, self.previous, self.category, &self.records,
        ))
    }

    pub(crate) fn encoded(&self) -> Result<Vec<u8>, StorageError> {
        if self.records.is_empty() || self.records.len() > MAX_PAGE_ROWS
            || self.records.iter().any(|r| r.category() != self.category)
            || self.sequence == 0 || self.sequence > i64::MAX as u64
            || self.digest != self.content_digest()?
        {
            return Err(StorageError::InvalidArguments);
        }
        encode_bounded(self, MAX_PAGE_BYTES)
    }
}

// A limiting writer prevents a large record from allocating an unbounded JSON
// buffer before the page limit is checked.
struct Limited(Vec<u8>, usize);
impl std::io::Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.1.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("transfer byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

pub(crate) fn encode_bounded(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, StorageError> {
    let mut out = Limited(Vec::new(), limit);
    serde_json::to_writer(&mut out, value).map_err(|_| StorageError::InvalidArguments)?;
    Ok(out.0)
}

fn hash(value: &impl Serialize) -> Result<[u8; 32], StorageError> {
    let bytes = encode_bounded(value, MAX_PAGE_BYTES)?;
    Ok(Sha256::digest(bytes).into())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferCheckpoint {
    pub snapshot: [u8; 32],
    pub sequence: u64,
    pub digest: [u8; 32],
    pub rows: Vec<u64>,
}

impl TransferCheckpoint {
    pub(crate) fn initial(manifest: &SnapshotManifest) -> Self {
        Self { snapshot: manifest.digest, sequence: 0, digest: [0; 32], rows: vec![0; Category::ALL.len()] }
    }

    pub(crate) fn advance(&self, manifest: &SnapshotManifest, page: &TransferPage) -> Result<Self, StorageError> {
        if self.snapshot != manifest.digest || page.snapshot != manifest.digest
            || self.sequence.checked_add(1) != Some(page.sequence)
            || self.digest != page.previous || self.rows.len() != Category::ALL.len()
        {
            return Err(StorageError::Conflict);
        }
        let index = Category::ALL.iter().position(|c| *c == page.category).ok_or(StorageError::InvalidArguments)?;
        let mut next = self.clone();
        next.rows[index] = next.rows[index].checked_add(page.records.len() as u64).ok_or(StorageError::InvalidArguments)?;
        if next.rows[index] > manifest.categories[index].rows { return Err(StorageError::Conflict); }
        next.sequence = page.sequence;
        next.digest = page.digest;
        Ok(next)
    }
}

/// Durable transport inbox only. No `finish_import` exists until exhaustive
/// native materialization, proof qualification and ownership fencing exist.
/// Staged pages never become Catalog or native read state through this API.
pub struct TransferInbox(StoreConfig);

impl StoreConfig {
    pub fn transfer_inbox(&self) -> TransferInbox { TransferInbox(self.clone()) }
}

impl TransferInbox {
    pub fn begin(&self, manifest: &SnapshotManifest) -> Result<TransferCheckpoint, StorageError> {
        manifest.bind(&self.0)?;
        encode_bounded(manifest, MAX_MANIFEST_BYTES)?;
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => super::sqlite::transfer::begin(&self.0, manifest),
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => super::postgres::transfer::begin(&self.0, pool, control, manifest),
        }
    }

    pub fn stage_page(&self, manifest: &SnapshotManifest, page: &TransferPage) -> Result<TransferCheckpoint, StorageError> {
        manifest.bind(&self.0)?;
        let bytes = page.encoded()?;
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => super::sqlite::transfer::stage(&self.0, manifest, page, &bytes),
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => super::postgres::transfer::stage(&self.0, pool, control, manifest, page, &bytes),
        }
    }

    pub fn checkpoint(&self, manifest: &SnapshotManifest) -> Result<TransferCheckpoint, StorageError> {
        self.begin(manifest)
    }
}

pub(crate) fn key(snapshot: &[u8; 32], suffix: &str) -> String {
    let id = snapshot.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    format!("native_transfer_v1:{id}:{suffix}")
}

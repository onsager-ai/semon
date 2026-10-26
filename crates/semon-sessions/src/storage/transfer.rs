//! Current-format transfer inbox and resumable authoritative page application.
//! Neither is an ownership flip or a claim that all native state is qualified.
//! Adapters keep the inbox in the namespace-owned metadata region; they never
//! interpret a page as SQL or replay Raw bodies.
use super::{StorageError, StoreBackend, StoreConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(test)]
pub(crate) mod inventory;
pub(crate) mod reported;
pub use reported::CaptureProgress;

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
        Self::Events,
        Self::Ledgers,
        Self::NativeHeaders,
        Self::NativeSidecars,
        Self::Usage,
        Self::ReportedFacts,
        Self::SourceObservations,
        Self::Custody,
        Self::Generations,
        Self::RawIntents,
        Self::RawCompletions,
        Self::Receipts,
        Self::Operations,
        Self::Claims,
        Self::StagedWork,
        Self::Checkpoints,
        Self::CatalogCurrent,
        Self::CatalogHistory,
        Self::Slots,
        Self::Recipes,
        Self::PageBlobs,
        Self::PageGenerations,
        Self::PageReferences,
        Self::Proofs,
        Self::ProofReferences,
        Self::Search,
        Self::Trace,
        Self::SourceState,
        Self::SourceMembers,
        Self::Invalidations,
        Self::Mutations,
        Self::Relocations,
        Self::Tree,
        Self::ChangeHeads,
        Self::ChangeFloors,
        Self::Metadata,
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
    /// Host-retained identity of this capture, distinct from a content digest.
    pub snapshot_id: [u8; 32],
    pub namespace: String,
    pub stream_epoch: String,
    pub binding_epoch: u64,
    pub revision: u64,
    pub categories: Vec<CategoryManifest>,
    pub digest: [u8; 32],
}

impl SnapshotManifest {
    pub(crate) fn identity(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"semon-current-snapshot-identity-v1");
        digest.update(self.snapshot_id);
        digest.update((self.namespace.len() as u64).to_be_bytes());
        digest.update(self.namespace.as_bytes());
        digest.update(self.stream_epoch.as_bytes());
        digest.update(self.binding_epoch.to_be_bytes());
        digest.update(self.revision.to_be_bytes());
        digest.finalize().into()
    }

    pub fn seal(&mut self) -> Result<(), StorageError> {
        self.digest = self.content_digest()?;
        self.validate()
    }

    fn content_digest(&self) -> Result<[u8; 32], StorageError> {
        hash(&(
            "semon-current-transfer-manifest-v1",
            self.version,
            self.snapshot_id,
            &self.namespace,
            &self.stream_epoch,
            self.binding_epoch,
            self.revision,
            &self.categories,
        ))
    }

    pub fn validate(&self) -> Result<(), StorageError> {
        if self.version != FORMAT_VERSION
            || self.namespace.is_empty()
            || self.namespace.len() > 4096
            || self.binding_epoch == 0
            || self.stream_epoch.len() != 64
            || !self.stream_epoch.bytes().all(|b| b.is_ascii_hexdigit())
            || self.categories.len() != Category::ALL.len()
            || !self
                .categories
                .iter()
                .map(|c| c.category)
                .eq(Category::ALL.iter().copied())
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
    /// Fixed current FILE columns after file_id/path; physical IDs are not portable.
    NativeFile { path: String, values: Vec<serde_json::Value> },
    /// Fixed current EVENT columns; the owning file is a logical path.
    NativeEvent { path: String, values: Vec<serde_json::Value> },
}

impl TransferRecord {
    pub(crate) fn validate_native(&self) -> Result<(), StorageError> {
        use crate::events::native_decode::{self, Values};
        match self {
            Self::NativeFile { path, values } => {
                if path.is_empty() || values.len() != 27 { return Err(StorageError::ScopeChanged); }
                let ledger = Values(vec![0.into(), values[0].clone(), values[1].clone(), values[2].clone(), values[3].clone(), values[4].clone(), values[5].clone(), values[6].clone(), values[26].clone()]);
                native_decode::ledger(&ledger).map_err(|_| StorageError::ScopeChanged)?;
            }
            Self::NativeEvent { path, values } => {
                if path.is_empty() || values.len() != 27 { return Err(StorageError::ScopeChanged); }
                native_decode::event(&Values(values.clone())).map_err(|_| StorageError::ScopeChanged)?;
            }
            _ => return Err(StorageError::InvalidArguments),
        }
        Ok(())
    }

    fn category(&self) -> Category {
        match self {
            Self::Metadata { .. } => Category::Metadata,
            Self::ReportedFact(_) => Category::ReportedFacts,
            Self::SourceIntent(_) => Category::SourceObservations,
            Self::NativeFile { .. } => Category::Ledgers,
            Self::NativeEvent { .. } => Category::Events,
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
            "semon-current-transfer-page-v1",
            self.snapshot,
            self.sequence,
            self.operation,
            self.previous,
            self.category,
            &self.records,
        ))
    }

    pub(crate) fn encoded(&self) -> Result<Vec<u8>, StorageError> {
        if self.records.is_empty()
            || self.records.len() > MAX_PAGE_ROWS
            || self.records.iter().any(|r| r.category() != self.category)
            || self.sequence == 0
            || self.sequence > i64::MAX as u64
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
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn encode_bounded(
    value: &impl Serialize,
    limit: usize,
) -> Result<Vec<u8>, StorageError> {
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
    pub digests: Vec<[u8; 32]>,
}

impl TransferCheckpoint {
    pub(crate) fn initial(manifest: &SnapshotManifest) -> Self {
        Self {
            snapshot: manifest.identity(),
            sequence: 0,
            digest: [0; 32],
            rows: vec![0; Category::ALL.len()],
            digests: vec![[0; 32]; Category::ALL.len()],
        }
    }

    pub(crate) fn advance(
        &self,
        manifest: &SnapshotManifest,
        page: &TransferPage,
    ) -> Result<Self, StorageError> {
        if self.snapshot != manifest.identity()
            || page.snapshot != manifest.identity()
            || self.sequence.checked_add(1) != Some(page.sequence)
            || self.digest != page.previous
            || self.rows.len() != Category::ALL.len()
            || self.digests.len() != Category::ALL.len()
        {
            return Err(StorageError::Conflict);
        }
        let index = Category::ALL
            .iter()
            .position(|c| *c == page.category)
            .ok_or(StorageError::InvalidArguments)?;
        let mut next = self.clone();
        next.rows[index] = next.rows[index]
            .checked_add(page.records.len() as u64)
            .ok_or(StorageError::InvalidArguments)?;
        if next.rows[index] > manifest.categories[index].rows {
            return Err(StorageError::Conflict);
        }
        for record in &page.records {
            next.digests[index] = hash(&(next.digests[index], record))?;
        }
        next.sequence = page.sequence;
        next.digest = page.digest;
        Ok(next)
    }
}

/// Durable transport inbox. No `finish_import` exists until exhaustive
/// native materialization, proof qualification and ownership fencing exist.
/// Staging alone never changes Catalog or native read state. The explicit
/// capture/apply operations materialize reported Facts through bounded pages.
pub struct TransferInbox(StoreConfig);

impl StoreConfig {
    pub fn transfer_inbox(&self) -> TransferInbox {
        TransferInbox(self.clone())
    }
}

impl TransferInbox {
    pub fn begin(&self, manifest: &SnapshotManifest) -> Result<TransferCheckpoint, StorageError> {
        manifest.bind(&self.0)?;
        encode_bounded(manifest, MAX_MANIFEST_BYTES)?;
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => super::sqlite::transfer::begin(&self.0, manifest),
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::postgres::transfer::begin(&self.0, pool, control, manifest)
            }
        }
    }

    pub fn stage_page(
        &self,
        manifest: &SnapshotManifest,
        page: &TransferPage,
    ) -> Result<TransferCheckpoint, StorageError> {
        manifest.bind(&self.0)?;
        let bytes = page.encoded()?;
        if page.snapshot != manifest.identity() {
            return Err(StorageError::ScopeChanged);
        }
        for record in &page.records {
            match record {
                TransferRecord::SourceIntent(intent) => {
                    super::source_observations::validate_intent(Some(&manifest.namespace), intent)?;
                }
                TransferRecord::ReportedFact(run) => {
                    crate::events::reported_rows::ReportedRow::new(run)
                        .map_err(|_| StorageError::InvalidArguments)?;
                }
                TransferRecord::NativeFile { .. } | TransferRecord::NativeEvent { .. } => {
                    record.validate_native()?;
                }
                TransferRecord::Metadata { .. } => {}
            }
        }
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => {
                super::sqlite::transfer::stage(&self.0, manifest, page, &bytes)
            }
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::postgres::transfer::stage(&self.0, pool, control, manifest, page, &bytes)
            }
        }
    }

    pub fn checkpoint(
        &self,
        manifest: &SnapshotManifest,
    ) -> Result<TransferCheckpoint, StorageError> {
        manifest.bind(&self.0)?;
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => super::sqlite::transfer::checkpoint(&self.0, manifest),
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::postgres::transfer::checkpoint(&self.0, pool, control, manifest)
            }
        }
    }

    pub fn staged_page(
        &self,
        manifest: &SnapshotManifest,
        sequence: u64,
    ) -> Result<TransferPage, StorageError> {
        manifest.bind(&self.0)?;
        if sequence == 0 || sequence > i64::MAX as u64 {
            return Err(StorageError::InvalidArguments);
        }
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => {
                super::sqlite::transfer::page(&self.0, manifest, sequence)
            }
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::postgres::transfer::page(&self.0, pool, control, manifest, sequence)
            }
        }
    }
}

pub(crate) fn key(snapshot: &[u8; 32], suffix: &str) -> String {
    let id = snapshot
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("native_transfer_v1:{id}:{suffix}")
}

pub(crate) trait InboxTransaction {
    fn get(&mut self, key: &str, limit: usize) -> Result<Option<Vec<u8>>, StorageError>;
    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), StorageError>;
}

pub(crate) fn begin_tx(
    tx: &mut impl InboxTransaction,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    let manifest_key = key(&manifest.identity(), "manifest");
    let checkpoint_key = key(&manifest.identity(), "checkpoint");
    let bytes = encode_bounded(manifest, MAX_MANIFEST_BYTES)?;
    if let Some(stored) = tx.get(&manifest_key, MAX_MANIFEST_BYTES)? {
        if stored != bytes {
            return Err(StorageError::Conflict);
        }
        let checkpoint = tx
            .get(&checkpoint_key, MAX_MANIFEST_BYTES)?
            .ok_or(StorageError::ScopeChanged)?;
        return decode_checkpoint(&checkpoint, manifest);
    }
    let checkpoint = TransferCheckpoint::initial(manifest);
    tx.put(&manifest_key, &bytes)?;
    tx.put(
        &checkpoint_key,
        &encode_bounded(&checkpoint, MAX_MANIFEST_BYTES)?,
    )?;
    Ok(checkpoint)
}

fn decode_checkpoint(
    bytes: &[u8],
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    let value: TransferCheckpoint =
        serde_json::from_slice(bytes).map_err(|_| StorageError::ScopeChanged)?;
    if value.snapshot != manifest.identity()
        || value.rows.len() != Category::ALL.len()
        || value.digests.len() != Category::ALL.len()
        || value
            .rows
            .iter()
            .zip(&manifest.categories)
            .any(|(rows, category)| *rows > category.rows)
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(value)
}

pub(crate) fn checkpoint_tx(
    tx: &mut impl InboxTransaction,
    manifest: &SnapshotManifest,
) -> Result<TransferCheckpoint, StorageError> {
    let stored = tx
        .get(&key(&manifest.identity(), "manifest"), MAX_MANIFEST_BYTES)?
        .ok_or(StorageError::ScopeChanged)?;
    if stored != encode_bounded(manifest, MAX_MANIFEST_BYTES)? {
        return Err(StorageError::Conflict);
    }
    let bytes = tx
        .get(&key(&manifest.identity(), "checkpoint"), MAX_MANIFEST_BYTES)?
        .ok_or(StorageError::ScopeChanged)?;
    decode_checkpoint(&bytes, manifest)
}

pub(crate) fn stage_tx(
    tx: &mut impl InboxTransaction,
    manifest: &SnapshotManifest,
    page: &TransferPage,
    bytes: &[u8],
) -> Result<TransferCheckpoint, StorageError> {
    let checkpoint = begin_tx(tx, manifest)?;
    let operation_key = key(&manifest.identity(), &key(&page.operation, "operation"));
    let page_key = key(&manifest.identity(), &format!("page:{}", page.sequence));
    if let Some(receipt) = tx.get(&operation_key, MAX_MANIFEST_BYTES)? {
        let committed = decode_checkpoint(&receipt, manifest)?;
        if committed.sequence != page.sequence
            || committed.digest != page.digest
            || tx.get(&page_key, MAX_PAGE_BYTES)?.as_deref() != Some(bytes)
        {
            return Err(StorageError::Conflict);
        }
        return Ok(committed);
    }
    let next = checkpoint.advance(manifest, page)?;
    if tx.get(&page_key, MAX_PAGE_BYTES)?.is_some() {
        return Err(StorageError::Conflict);
    }
    let encoded = encode_bounded(&next, MAX_MANIFEST_BYTES)?;
    tx.put(&page_key, bytes)?;
    tx.put(&operation_key, &encoded)?;
    tx.put(&key(&manifest.identity(), "checkpoint"), &encoded)?;
    Ok(next)
}

pub(crate) fn page_tx(
    tx: &mut impl InboxTransaction,
    manifest: &SnapshotManifest,
    sequence: u64,
) -> Result<TransferPage, StorageError> {
    let manifest_bytes = tx
        .get(&key(&manifest.identity(), "manifest"), MAX_MANIFEST_BYTES)?
        .ok_or(StorageError::ScopeChanged)?;
    if manifest_bytes != encode_bounded(manifest, MAX_MANIFEST_BYTES)? {
        return Err(StorageError::Conflict);
    }
    let bytes = tx
        .get(
            &key(&manifest.identity(), &format!("page:{sequence}")),
            MAX_PAGE_BYTES,
        )?
        .ok_or(StorageError::ScopeChanged)?;
    let page: TransferPage =
        serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    if page.sequence != sequence || page.snapshot != manifest.identity() || page.encoded()? != bytes
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(page)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn manifest() -> SnapshotManifest {
        let mut manifest = SnapshotManifest {
            version: FORMAT_VERSION,
            snapshot_id: [9; 32],
            namespace: "transfer225".into(),
            stream_epoch: "a".repeat(64),
            binding_epoch: 1,
            revision: 7,
            categories: Category::ALL
                .iter()
                .map(|category| CategoryManifest {
                    category: *category,
                    rows: if *category == Category::Metadata {
                        2
                    } else {
                        0
                    },
                    digest: [0; 32],
                })
                .collect(),
            digest: [0; 32],
        };
        manifest.seal().unwrap();
        manifest
    }

    pub(crate) fn page(
        manifest: &SnapshotManifest,
        sequence: u64,
        previous: [u8; 32],
    ) -> TransferPage {
        let mut page = TransferPage {
            snapshot: manifest.identity(),
            sequence,
            previous,
            operation: [sequence as u8; 32],
            category: Category::Metadata,
            records: vec![TransferRecord::Metadata {
                key: format!("native_header:{sequence}"),
                value: "retained-proof".into(),
            }],
            digest: [0; 32],
        };
        page.seal().unwrap();
        page
    }

    pub(crate) fn exercise(config: &StoreConfig) {
        let manifest = manifest();
        let inbox = config.transfer_inbox();
        let initial = inbox.begin(&manifest).unwrap();
        assert_eq!(initial.sequence, 0);
        let first = page(&manifest, 1, [0; 32]);
        let after_first = inbox.stage_page(&manifest, &first).unwrap();
        assert_eq!(after_first.sequence, 1);
        assert_eq!(inbox.stage_page(&manifest, &first).unwrap(), after_first);
        // A restarted handle resumes the durable cursor, and the original page
        // receipt remains exact even after a later page has been committed.
        let restarted = config.transfer_inbox();
        assert_eq!(restarted.checkpoint(&manifest).unwrap(), after_first);
        assert_eq!(
            restarted
                .staged_page(&manifest, 1)
                .unwrap()
                .encoded()
                .unwrap(),
            first.encoded().unwrap()
        );
        let second = page(&manifest, 2, first.digest);
        let after_second = restarted.stage_page(&manifest, &second).unwrap();
        assert_eq!(after_second.sequence, 2);
        assert_eq!(
            restarted.stage_page(&manifest, &first).unwrap(),
            after_first
        );
        let mut altered = first.clone();
        altered.records = vec![TransferRecord::Metadata {
            key: "native_header:changed".into(),
            value: "changed".into(),
        }];
        altered.seal().unwrap();
        assert!(matches!(
            restarted.stage_page(&manifest, &altered),
            Err(StorageError::Conflict)
        ));
        let third = page(&manifest, 3, second.digest);
        assert!(matches!(
            restarted.stage_page(&manifest, &third),
            Err(StorageError::Conflict)
        ));
        let mut corrupt = manifest.clone();
        corrupt.revision += 1;
        assert!(restarted.begin(&corrupt).is_err());
        assert_eq!(restarted.checkpoint(&manifest).unwrap(), after_second);
        let mut foreign = manifest.clone();
        foreign.namespace = "foreign".into();
        foreign.seal().unwrap();
        assert!(matches!(
            restarted.begin(&foreign),
            Err(StorageError::NamespaceMismatch)
        ));
        let mut old = manifest.clone();
        old.version = 0;
        assert!(old.seal().is_err());
        let mut oversized = first.clone();
        oversized.records = vec![TransferRecord::Metadata {
            key: "native_header:large".into(),
            value: "x".repeat(MAX_PAGE_BYTES),
        }];
        assert!(oversized.seal().is_err());
        assert_eq!(restarted.checkpoint(&manifest).unwrap(), after_second);
        let mut intent_manifest = manifest.clone();
        let index = Category::ALL
            .iter()
            .position(|c| *c == Category::SourceObservations)
            .unwrap();
        intent_manifest.categories[index].rows = 1;
        intent_manifest.seal().unwrap();
        let operation = super::super::source_observations::SourceOperation::capture(
            "foreign", "recover", b"null",
        );
        let mut foreign_intent = page(&intent_manifest, 1, [0; 32]);
        foreign_intent.category = Category::SourceObservations;
        foreign_intent.records = vec![TransferRecord::SourceIntent(
            super::super::source_observations::SourceIntent::Recover {
                operation,
                cursor: None,
            },
        )];
        foreign_intent.seal().unwrap();
        assert!(
            restarted
                .stage_page(&intent_manifest, &foreign_intent)
                .is_err()
        );
        assert!(restarted.checkpoint(&intent_manifest).is_err());
    }

    #[test]
    fn current_transfer225_sqlite_inbox_restart_exact_replay_and_bounds() {
        let root = std::env::temp_dir().join(format!("semon-transfer225-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let config = StoreConfig {
            backend: StoreBackend::Sqlite {
                database: root.join("current.sqlite3"),
            },
            namespace: super::super::LogicalSourceNamespace::new("transfer225").unwrap(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        config.create_native_store().unwrap();
        crate::events::store::configured::NativeOwnership::admit(&config).unwrap();
        exercise(&config);
        std::fs::remove_dir_all(root).unwrap();
    }
}

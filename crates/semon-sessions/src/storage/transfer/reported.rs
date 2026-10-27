//! Resumable current snapshot capture and authoritative page application.
//! Capture currently exports reported Facts/stamp and fixed core files/events.
//! Other native families remain unqualified; neither operation grants ownership.
use super::*;
use crate::events::ReportedFileStamp;

pub const FREEZE: &str = "native_transfer_v1:source-freeze";
pub const INCOMING: &str = "native_transfer_v1:incoming";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureProgress {
    Capturing { pages: u64 },
    Ready(SnapshotManifest),
}

impl TransferInbox {
    /// Admit the imported Catalog from the actual host-authorized frozen source.
    /// This publishes a Catalog operation, not a full-source ownership flip.
    /// A caller-supplied manifest or an inbox alone cannot mint this authority.
    pub fn admit_catalog_from(
        &self,
        source: &TransferInbox,
        manifest: &SnapshotManifest,
        options: &crate::Options,
    ) -> Result<String, StorageError> {
        manifest.bind(&self.0)?;
        // Facts are host-authorized Raw, independent of the SQL placement.
        // Validate the host's logical identity before any destination effect.
        options.source.clone().with_store(self.0.clone())?;
        if source.capture(&manifest.snapshot_id)? != CaptureProgress::Ready(manifest.clone()) {
            return Err(StorageError::Conflict);
        }
        let import = CatalogImport {
            manifest: manifest.clone(),
            generation: source.0.catalog_generation()?,
        };
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => {
                super::super::sqlite::transfer::admit_catalog(&self.0, &import)
            }
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::super::postgres::transfer::admit_catalog(&self.0, pool, control, &import, options)
            }
        }
    }

    /// Freeze ordinary source writers and advance one bounded capture page.
    /// Retry the same identity after restart or an uncertain commit. Every page
    /// retains the 64-row/1-MiB limit; total history has no row/byte cap.
    /// This partial inventory is not a full-source completeness witness.
    pub fn capture(&self, operation: &[u8; 32]) -> Result<CaptureProgress, StorageError> {
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => {
                super::super::sqlite::transfer::capture(&self.0, operation)
            }
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::super::postgres::transfer::capture(&self.0, pool, control, operation)
            }
        }
    }

    /// Apply an already staged page to authoritative native rows. Each page's
    /// rows, exact receipt and applied cursor commit together. Ordinary readers
    /// and writers refuse an unfinished destination; final application opens it.
    /// Sequence zero finalizes an entirely empty manifest; it never skips rows.
    pub fn apply_page(
        &self,
        manifest: &SnapshotManifest,
        sequence: u64,
    ) -> Result<TransferCheckpoint, StorageError> {
        manifest.bind(&self.0)?;
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => {
                super::super::sqlite::transfer::apply(&self.0, manifest, sequence)
            }
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::super::postgres::transfer::apply(&self.0, pool, control, manifest, sequence)
            }
        }
    }
}

/// Constructed only after observing the exact completed capture on the source.
/// It is neither deserializable nor constructible from transported page bytes.
pub(crate) struct CatalogImport {
    manifest: SnapshotManifest,
    generation: String,
}

impl CatalogImport {
    pub(crate) fn manifest(&self) -> &SnapshotManifest {
        &self.manifest
    }

    pub(crate) fn generation(&self) -> &str {
        &self.generation
    }

    pub(crate) fn payload(&self) -> Result<Vec<u8>, StorageError> {
        encode_bounded(
            &("catalog-import-v1", &self.manifest, &self.generation),
            MAX_PAGE_BYTES,
        )
    }

    pub(crate) fn require_applied(
        &self,
        tx: &mut impl InboxTransaction,
    ) -> Result<(), StorageError> {
        let staged = checkpoint_tx(tx, &self.manifest)?;
        if staged
            .rows
            .iter()
            .zip(&self.manifest.categories)
            .any(|(rows, category)| *rows != category.rows)
            || staged
                .digests
                .iter()
                .zip(&self.manifest.categories)
                .any(|(digest, category)| *digest != category.digest)
            || tx
                .get(
                    &key(&self.manifest.identity(), "applied"),
                    MAX_MANIFEST_BYTES,
                )?
                .map(|bytes| decode_checkpoint(&bytes, &self.manifest))
                .transpose()?
                != Some(staged)
            || tx
                .get(INCOMING, MAX_MANIFEST_BYTES)?
                .is_some_and(|bytes| !bytes.is_empty())
            || tx
                .get(super::catalog_rows::IMPORTED_GENERATION, 64)?
                .as_deref()
                != Some(self.generation.as_bytes())
        {
            return Err(StorageError::Conflict);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CaptureState {
    pub(crate) manifest: SnapshotManifest,
    pub(crate) checkpoint: TransferCheckpoint,
    /// Private adapter keyset: SQLite UTF-8 ID or PG digest, plus exact start.
    pub(crate) cursor: Option<(Vec<u8>, i64)>,
    pub(crate) done: bool,
    pub(crate) native: bool,
    pub(crate) native_path: Option<String>,
    pub(crate) native_family: u8,
    pub(crate) native_key: Option<String>,
    pub(crate) native_backing: Option<String>,
    pub(crate) native_withdrawn_after: Option<[Vec<u8>; 3]>,
}
impl CaptureState {
    pub(crate) fn progress(&self) -> CaptureProgress {
        if self.done {
            CaptureProgress::Ready(self.manifest.clone())
        } else {
            CaptureProgress::Capturing {
                pages: self.checkpoint.sequence,
            }
        }
    }
}

pub(crate) fn capture_state(
    tx: &mut impl InboxTransaction,
    config: &StoreConfig,
    operation: &[u8; 32],
) -> Result<Option<CaptureState>, StorageError> {
    let expected = encode_bounded(operation, MAX_MANIFEST_BYTES)?;
    if let Some(frozen) = tx.get(FREEZE, MAX_MANIFEST_BYTES)?
        && frozen != expected
    {
        return Err(StorageError::Conflict);
    }
    tx.get(&key(operation, "capture"), MAX_MANIFEST_BYTES)?
        .map(|bytes| {
            let state: CaptureState =
                serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
            state.manifest.bind(config)?;
            if state.manifest.snapshot_id != *operation
                || state.checkpoint.snapshot != state.manifest.identity()
                || state.checkpoint.rows.len() != Category::ALL.len()
                || state.checkpoint.digests.len() != Category::ALL.len()
                || state
                    .checkpoint
                    .rows
                    .iter()
                    .zip(&state.checkpoint.digests)
                    .zip(&state.manifest.categories)
                    .any(|((rows, digest), category)| {
                        *rows != category.rows || *digest != category.digest
                    })
            {
                return Err(StorageError::ScopeChanged);
            }
            Ok(state)
        })
        .transpose()
}

pub(crate) fn start_capture(
    tx: &mut impl InboxTransaction,
    config: &StoreConfig,
    operation: &[u8; 32],
    binding: u64,
    revision: u64,
    stamp: Option<ReportedFileStamp>,
) -> Result<CaptureState, StorageError> {
    if tx
        .get(INCOMING, MAX_MANIFEST_BYTES)?
        .is_some_and(|bytes| !bytes.is_empty())
    {
        return Err(StorageError::Conflict);
    }
    let mut manifest = SnapshotManifest {
        version: FORMAT_VERSION,
        snapshot_id: *operation,
        namespace: config.namespace.as_str().into(),
        stream_epoch: config
            .expected_stream_epoch
            .clone()
            .ok_or(StorageError::InvalidArguments)?,
        binding_epoch: binding,
        revision,
        categories: Category::ALL
            .iter()
            .map(|category| CategoryManifest {
                category: *category,
                rows: 0,
                digest: [0; 32],
            })
            .collect(),
        digest: [0; 32],
    };
    manifest.seal()?;
    let checkpoint = TransferCheckpoint::initial(&manifest);
    let mut state = CaptureState {
        manifest,
        checkpoint,
        cursor: None,
        done: false,
        native: false,
        native_path: None,
        native_family: 0,
        native_key: None,
        native_backing: None,
        native_withdrawn_after: None,
    };
    tx.put(FREEZE, &encode_bounded(operation, MAX_MANIFEST_BYTES)?)?;
    if let Some(stamp) = stamp {
        let value = String::from_utf8(
            crate::events::reported_rows::encode_stamp(&stamp)
                .map_err(|_| StorageError::ScopeChanged)?,
        )
        .map_err(|_| StorageError::ScopeChanged)?;
        append_capture(
            tx,
            &mut state,
            Category::Metadata,
            vec![TransferRecord::Metadata {
                key: "claude_json_stamp".into(),
                value,
            }],
        )?;
    }
    save_capture(tx, &state)?;
    Ok(state)
}

pub(crate) fn append_capture(
    tx: &mut impl InboxTransaction,
    state: &mut CaptureState,
    category: Category,
    records: Vec<TransferRecord>,
) -> Result<(), StorageError> {
    let index = Category::ALL
        .iter()
        .position(|c| *c == category)
        .ok_or(StorageError::InvalidArguments)?;
    let mut page = TransferPage {
        snapshot: state.manifest.identity(),
        sequence: state.checkpoint.sequence + 1,
        operation: hash(&(state.manifest.snapshot_id, state.checkpoint.sequence + 1))?,
        previous: state.checkpoint.digest,
        category,
        records,
        digest: [0; 32],
    };
    page.seal()?;
    state.manifest.categories[index].rows = state.manifest.categories[index]
        .rows
        .checked_add(page.records.len() as u64)
        .ok_or(StorageError::InvalidArguments)?;
    state.checkpoint = state.checkpoint.advance(&state.manifest, &page)?;
    state.manifest.categories[index].digest = state.checkpoint.digests[index];
    state.manifest.seal()?;
    tx.put(
        &key(&page.snapshot, &format!("page:{}", page.sequence)),
        &page.encoded()?,
    )?;
    Ok(())
}

pub(crate) fn save_capture(
    tx: &mut impl InboxTransaction,
    state: &CaptureState,
) -> Result<(), StorageError> {
    tx.put(
        &key(&state.manifest.snapshot_id, "capture"),
        &encode_bounded(state, MAX_MANIFEST_BYTES)?,
    )
}

pub(crate) fn finish_capture(
    tx: &mut impl InboxTransaction,
    state: &mut CaptureState,
) -> Result<(), StorageError> {
    state.done = true;
    state.manifest.seal()?;
    let id = state.manifest.identity();
    tx.put(
        &key(&id, "manifest"),
        &encode_bounded(&state.manifest, MAX_MANIFEST_BYTES)?,
    )?;
    tx.put(
        &key(&id, "checkpoint"),
        &encode_bounded(&state.checkpoint, MAX_MANIFEST_BYTES)?,
    )?;
    save_capture(tx, state)
}

pub(crate) struct ApplyPage {
    pub(crate) checkpoint: TransferCheckpoint,
    pub(crate) records: Vec<TransferRecord>,
    pub(crate) replayed: bool,
    pub(crate) first: bool,
    pub(crate) complete: bool,
}

pub(crate) fn prepare_apply(
    tx: &mut impl InboxTransaction,
    manifest: &SnapshotManifest,
    sequence: u64,
) -> Result<ApplyPage, StorageError> {
    let staged = checkpoint_tx(tx, manifest)?;
    if staged
        .rows
        .iter()
        .zip(&staged.digests)
        .zip(&manifest.categories)
        .any(|((rows, digest), category)| {
            *rows != category.rows
                || *digest != category.digest
                || (category.rows != 0
                    && !matches!(
                        category.category,
                        Category::ReportedFacts
                            | Category::Metadata
                            | Category::Ledgers
                            | Category::Events
                            | Category::PageGenerations
                            | Category::PageBlobs
                            | Category::NativeHeaders
                            | Category::Usage
                            | Category::CatalogCurrent
                            | Category::CatalogHistory
                            | Category::Recipes
                            | Category::Slots
                            | Category::Trace
                            | Category::Search
                            | Category::Operations
                            | Category::Proofs
                            | Category::ProofReferences
                    ))
        })
    {
        return Err(StorageError::Conflict);
    }
    let id = manifest.identity();
    if tx
        .get(FREEZE, MAX_MANIFEST_BYTES)?
        .is_some_and(|bytes| !bytes.is_empty())
    {
        return Err(StorageError::Conflict);
    }
    if sequence == 0 {
        if staged != TransferCheckpoint::initial(manifest) {
            return Err(StorageError::Conflict);
        }
        let expected = encode_bounded(&id, MAX_MANIFEST_BYTES)?;
        if tx
            .get(INCOMING, MAX_MANIFEST_BYTES)?
            .is_some_and(|bytes| !bytes.is_empty() && bytes != expected)
        {
            return Err(StorageError::Conflict);
        }
        let receipt = tx.get(&key(&id, "applied:0"), MAX_MANIFEST_BYTES)?;
        if let Some(bytes) = &receipt
            && decode_checkpoint(bytes, manifest)? != staged
        {
            return Err(StorageError::Conflict);
        }
        return Ok(ApplyPage {
            checkpoint: staged,
            records: Vec::new(),
            replayed: receipt.is_some(),
            first: true,
            complete: true,
        });
    }
    let page = page_tx(tx, manifest, sequence)?;
    let receipt = key(&id, &format!("applied:{sequence}"));
    if let Some(bytes) = tx.get(&receipt, MAX_MANIFEST_BYTES)? {
        let checkpoint = decode_checkpoint(&bytes, manifest)?;
        if checkpoint.sequence != sequence || checkpoint.digest != page.digest {
            return Err(StorageError::Conflict);
        }
        return Ok(ApplyPage {
            checkpoint,
            records: Vec::new(),
            replayed: true,
            first: false,
            complete: false,
        });
    }
    let previous = tx
        .get(&key(&id, "applied"), MAX_MANIFEST_BYTES)?
        .map(|bytes| decode_checkpoint(&bytes, manifest))
        .transpose()?
        .unwrap_or_else(|| TransferCheckpoint::initial(manifest));
    if let Some(incoming) = tx.get(INCOMING, MAX_MANIFEST_BYTES)?
        && !incoming.is_empty()
        && incoming != encode_bounded(&id, MAX_MANIFEST_BYTES)?
    {
        return Err(StorageError::Conflict);
    }
    let checkpoint = previous.advance(manifest, &page)?;
    let complete = checkpoint
        .rows
        .iter()
        .zip(&manifest.categories)
        .all(|(rows, category)| *rows == category.rows);
    if complete
        && checkpoint
            .digests
            .iter()
            .zip(&manifest.categories)
            .any(|(digest, category)| *digest != category.digest)
    {
        return Err(StorageError::Conflict);
    }
    for record in &page.records {
        match record {
            TransferRecord::CatalogRow { .. } => super::catalog_rows::validate(record)?,
            TransferRecord::ReportedFact(run) => {
                crate::events::reported_rows::ReportedRow::new(run)
                    .map_err(|_| StorageError::ScopeChanged)?;
            }
            TransferRecord::Metadata { key, value } if key == "claude_json_stamp" => {
                crate::events::reported_rows::decode_stamp(value.as_bytes())
                    .map_err(|_| StorageError::ScopeChanged)?;
            }
            TransferRecord::NativeRow { .. } => native_rows::validate(record)?,
            TransferRecord::NativeSource(_)
            | TransferRecord::NativePage { .. }
            | TransferRecord::NativeMetadata { .. } => native_pages::validate(record)?,
            TransferRecord::NativeFile { .. } | TransferRecord::NativeEvent { .. } => {
                record.validate_native()?
            }
            _ => return Err(StorageError::Conflict),
        }
    }
    Ok(ApplyPage {
        checkpoint,
        records: page.records,
        replayed: false,
        first: previous.sequence == 0,
        complete,
    })
}

pub(crate) fn acknowledge_apply(
    tx: &mut impl InboxTransaction,
    page: &ApplyPage,
) -> Result<(), StorageError> {
    let id = page.checkpoint.snapshot;
    let bytes = encode_bounded(&page.checkpoint, MAX_MANIFEST_BYTES)?;
    tx.put(
        &key(&id, &format!("applied:{}", page.checkpoint.sequence)),
        &bytes,
    )?;
    tx.put(&key(&id, "applied"), &bytes)?;
    tx.put(
        INCOMING,
        if page.complete {
            Vec::new()
        } else {
            encode_bounded(&id, MAX_MANIFEST_BYTES)?
        }
        .as_slice(),
    )
}

type FactsCursor = Option<(Vec<u8>, i64)>;
type CapturedFacts = (Vec<TransferRecord>, FactsCursor);

pub(crate) fn fit_facts(
    rows: Vec<(Vec<u8>, crate::facts::ReportedRunSnapshot)>,
) -> Result<CapturedFacts, StorageError> {
    let mut records = Vec::new();
    let mut cursor = None;
    let mut bytes = 4096usize;
    for (key, run) in rows {
        crate::events::reported_rows::ReportedRow::new(&run)
            .map_err(|_| StorageError::ScopeChanged)?;
        let start = run.last_start_time;
        let record = TransferRecord::ReportedFact(run);
        let size = encode_bounded(&record, MAX_PAGE_BYTES)?
            .len()
            .checked_add(1)
            .ok_or(StorageError::Busy)?;
        if bytes
            .checked_add(size)
            .is_none_or(|next| next > MAX_PAGE_BYTES)
        {
            if records.is_empty() {
                return Err(StorageError::Busy);
            }
            break;
        }
        bytes += size;
        records.push(record);
        cursor = Some((key, start));
    }
    Ok((records, cursor))
}

#[cfg(test)]
#[path = "reported_tests.rs"]
pub(crate) mod tests;

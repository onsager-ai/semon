//! Authoritative reported-Facts transfer, deliberately bounded to one family.
//! This is not complete Source qualification, quiescence or an ownership flip.
use super::*;
use crate::{events::ReportedFileStamp, facts::ReportedRunSnapshot};

impl TransferInbox {
    /// Capture at most 64 reported observations and their stamp in one source
    /// transaction. The durable snapshot survives source writes and restart.
    /// Larger histories refuse rather than exporting an incomplete history.
    pub fn snapshot_reported_facts(
        &self,
        operation: &[u8; 32],
    ) -> Result<SnapshotManifest, StorageError> {
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => {
                super::super::sqlite::transfer::snapshot_reported(&self.0, operation)
            }
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::super::postgres::transfer::snapshot_reported(
                    &self.0, pool, control, operation,
                )
            }
        }
    }

    /// Materialize this complete bounded family into an empty destination Facts
    /// region. Rows, stamp and exact snapshot receipt commit together. Other
    /// Source families remain unqualified and grant no ownership authority.
    pub fn materialize_reported_facts(
        &self,
        manifest: &SnapshotManifest,
    ) -> Result<TransferCheckpoint, StorageError> {
        manifest.bind(&self.0)?;
        match &self.0.backend {
            StoreBackend::Sqlite { .. } => {
                super::super::sqlite::transfer::materialize_reported(&self.0, manifest)
            }
            #[cfg(feature = "postgres")]
            StoreBackend::Postgres { pool, control } => {
                super::super::postgres::transfer::materialize_reported(
                    &self.0, pool, control, manifest,
                )
            }
        }
    }
}

pub(crate) fn saved_reported(
    tx: &mut impl InboxTransaction,
    config: &StoreConfig,
    operation: &[u8; 32],
) -> Result<Option<SnapshotManifest>, StorageError> {
    tx.get(&key(operation, "reported-source"), MAX_MANIFEST_BYTES)?
        .map(|bytes| {
            let manifest: SnapshotManifest =
                serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
            manifest.bind(config)?;
            if manifest.snapshot_id != *operation {
                return Err(StorageError::Conflict);
            }
            checkpoint_tx(tx, &manifest)?;
            Ok(manifest)
        })
        .transpose()
}

pub(crate) fn capture_reported(
    tx: &mut impl InboxTransaction,
    config: &StoreConfig,
    operation: &[u8; 32],
    binding_epoch: u64,
    revision: u64,
    runs: Vec<ReportedRunSnapshot>,
    stamp: Option<ReportedFileStamp>,
) -> Result<SnapshotManifest, StorageError> {
    if runs.len() > MAX_PAGE_ROWS || (stamp.is_none() && !runs.is_empty()) {
        return Err(StorageError::Busy);
    }
    let facts = runs
        .into_iter()
        .map(TransferRecord::ReportedFact)
        .collect::<Vec<_>>();
    for fact in &facts {
        if let TransferRecord::ReportedFact(run) = fact {
            crate::events::reported_rows::ReportedRow::new(run)
                .map_err(|_| StorageError::ScopeChanged)?;
        }
    }
    let metadata = stamp
        .map(|stamp| {
            crate::events::reported_rows::encode_stamp(&stamp)
                .map_err(|_| StorageError::ScopeChanged)
                .and_then(|bytes| String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged))
                .map(|value| {
                    vec![TransferRecord::Metadata {
                        key: "claude_json_stamp".into(),
                        value,
                    }]
                })
        })
        .transpose()?
        .unwrap_or_default();
    // Check the bounded family before any durable snapshot effect.
    encode_bounded(&(&facts, &metadata), MAX_PAGE_BYTES - 4096)?;
    let empty = Vec::new();
    let mut manifest = SnapshotManifest {
        version: FORMAT_VERSION,
        snapshot_id: *operation,
        namespace: config.namespace.as_str().into(),
        stream_epoch: config
            .expected_stream_epoch
            .clone()
            .ok_or(StorageError::InvalidArguments)?,
        binding_epoch,
        revision,
        categories: Category::ALL
            .iter()
            .map(|category| {
                let rows = match category {
                    Category::ReportedFacts => &facts,
                    Category::Metadata => &metadata,
                    _ => &empty,
                };
                Ok(CategoryManifest {
                    category: *category,
                    rows: rows.len() as u64,
                    digest: hash(rows)?,
                })
            })
            .collect::<Result<_, StorageError>>()?,
        digest: [0; 32],
    };
    manifest.seal()?;
    let mut checkpoint = begin_tx(tx, &manifest)?;
    for (category, records) in [
        (Category::ReportedFacts, facts),
        (Category::Metadata, metadata),
    ] {
        if records.is_empty() {
            continue;
        }
        let mut page = TransferPage {
            snapshot: manifest.digest,
            sequence: checkpoint.sequence + 1,
            operation: hash(&(operation, category))?,
            previous: checkpoint.digest,
            category,
            records,
            digest: [0; 32],
        };
        page.seal()?;
        checkpoint = stage_tx(tx, &manifest, &page, &page.encoded()?)?;
    }
    tx.put(
        &key(operation, "reported-source"),
        &encode_bounded(&manifest, MAX_MANIFEST_BYTES)?,
    )?;
    Ok(manifest)
}

pub(crate) struct ReportedMaterialization {
    pub(crate) checkpoint: TransferCheckpoint,
    pub(crate) runs: Vec<ReportedRunSnapshot>,
    pub(crate) stamp: Option<ReportedFileStamp>,
    pub(crate) replayed: bool,
}

pub(crate) fn reported_materialization(
    tx: &mut impl InboxTransaction,
    manifest: &SnapshotManifest,
) -> Result<ReportedMaterialization, StorageError> {
    let checkpoint = checkpoint_tx(tx, manifest)?;
    if checkpoint.sequence > 2
        || checkpoint
            .rows
            .iter()
            .zip(&manifest.categories)
            .any(|(rows, category)| {
                *rows != category.rows
                    || (category.rows != 0
                        && !matches!(
                            category.category,
                            Category::ReportedFacts | Category::Metadata
                        ))
            })
    {
        return Err(StorageError::Conflict);
    }
    let receipt = key(&manifest.digest, "reported-materialized");
    if let Some(bytes) = tx.get(&receipt, MAX_MANIFEST_BYTES)? {
        if bytes != encode_bounded(&checkpoint, MAX_MANIFEST_BYTES)? {
            return Err(StorageError::Conflict);
        }
        return Ok(ReportedMaterialization {
            checkpoint,
            runs: Vec::new(),
            stamp: None,
            replayed: true,
        });
    }
    let mut facts = Vec::new();
    let mut metadata = Vec::new();
    let mut previous = [0; 32];
    for sequence in 1..=checkpoint.sequence {
        let page = page_tx(tx, manifest, sequence)?;
        if page.previous != previous {
            return Err(StorageError::Conflict);
        }
        previous = page.digest;
        match page.category {
            Category::ReportedFacts => facts.extend(page.records),
            Category::Metadata => metadata.extend(page.records),
            _ => return Err(StorageError::Conflict),
        }
    }
    if previous != checkpoint.digest || facts.len() > MAX_PAGE_ROWS || metadata.len() > 1 {
        return Err(StorageError::Conflict);
    }
    let empty = Vec::new();
    for category in &manifest.categories {
        let records = match category.category {
            Category::ReportedFacts => &facts,
            Category::Metadata => &metadata,
            _ => &empty,
        };
        if category.rows != records.len() as u64 || category.digest != hash(records)? {
            return Err(StorageError::Conflict);
        }
    }
    let stamp = metadata
        .first()
        .map(|record| match record {
            TransferRecord::Metadata { key, value } if key == "claude_json_stamp" => {
                crate::events::reported_rows::decode_stamp(value.as_bytes())
                    .map_err(|_| StorageError::ScopeChanged)
            }
            _ => Err(StorageError::Conflict),
        })
        .transpose()?;
    let mut runs = Vec::new();
    let mut keys = std::collections::BTreeSet::new();
    for record in facts {
        let TransferRecord::ReportedFact(run) = record else {
            return Err(StorageError::Conflict);
        };
        crate::events::reported_rows::ReportedRow::new(&run)
            .map_err(|_| StorageError::ScopeChanged)?;
        if !keys.insert((run.last_session_id.clone(), run.last_start_time)) {
            return Err(StorageError::Conflict);
        }
        runs.push(run);
    }
    if stamp.is_none() && !runs.is_empty() {
        return Err(StorageError::ScopeChanged);
    }
    Ok(ReportedMaterialization {
        checkpoint,
        runs,
        stamp,
        replayed: false,
    })
}

pub(crate) fn acknowledge_reported(
    tx: &mut impl InboxTransaction,
    checkpoint: &TransferCheckpoint,
) -> Result<(), StorageError> {
    tx.put(
        &key(&checkpoint.snapshot, "reported-materialized"),
        &encode_bounded(checkpoint, MAX_MANIFEST_BYTES)?,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        collections::BTreeSet,
        path::{Path, PathBuf},
    };

    pub(crate) fn sqlite(root: &Path, name: &str) -> StoreConfig {
        let config = StoreConfig {
            backend: StoreBackend::Sqlite {
                database: root.join(format!("{name}.sqlite3")),
            },
            namespace: super::super::super::LogicalSourceNamespace::new("transfer225").unwrap(),
            expected_stream_epoch: Some("a".repeat(64)),
        };
        config.create_native_store().unwrap();
        crate::events::store::configured::NativeOwnership::admit(&config).unwrap();
        config
    }

    pub(crate) fn observe(config: &StoreConfig, input: &Path, start: i64, capture: i64) {
        std::fs::write(
            input,
            serde_json::json!({"projects":{"/sanitized":{
                "lastSessionId":"原生\0id", "lastStartTime":start,
                "lastDuration":u64::MAX, "lastCost":if start == 200 { None } else { Some(0.25) },
                "lastModelUsage":{"模型\0":{"inputTokens":u64::MAX,"outputTokens":31}}
            }}})
            .to_string(),
        )
        .unwrap();
        let mut cache = crate::events::EventCache::open_configured(config).unwrap();
        cache
            .refresh_reported_runs(input, capture, &mut false)
            .unwrap();
    }

    pub(crate) fn read(config: &StoreConfig) -> Vec<ReportedRunSnapshot> {
        let mut cache = crate::events::EventCache::open_configured(config).unwrap();
        cache
            .select_reported_runs(&BTreeSet::from(["原生\0id".into()]))
            .unwrap();
        cache.reported_runs().cloned().collect()
    }

    pub(crate) fn stage(
        source: &StoreConfig,
        target: &StoreConfig,
        operation: &[u8; 32],
    ) -> SnapshotManifest {
        let source = source.transfer_inbox();
        let manifest = source.snapshot_reported_facts(operation).unwrap();
        let checkpoint = source.checkpoint(&manifest).unwrap();
        let target = target.transfer_inbox();
        target.begin(&manifest).unwrap();
        for sequence in 1..=checkpoint.sequence {
            let page = source.staged_page(&manifest, sequence).unwrap();
            assert_eq!(
                target.stage_page(&manifest, &page).unwrap(),
                target.stage_page(&manifest, &page).unwrap()
            );
        }
        manifest
    }

    pub(crate) fn root() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "transfer228-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn current_transfer225_reported_authoritative_sqlite_snapshot_restart_and_nonmutation() {
        let root = root();
        let source = sqlite(&root, "source");
        let target = sqlite(&root, "target");
        let input = root.join(".claude.json");
        observe(&source, &input, 100, 101);
        observe(&source, &input, 200, 202);
        let expected = read(&source);
        assert_eq!(expected.len(), 2);
        let manifest = stage(&source, &target, &[1; 32]);
        let separate_capture = source.transfer_inbox().snapshot_reported_facts(&[5; 32]).unwrap();
        assert_ne!(separate_capture.digest, manifest.digest, "equal source content must still retain distinct capture identities");
        assert!(
            read(&target).is_empty(),
            "inbox alone must not materialize Facts"
        );
        observe(&source, &input, 300, 303);
        assert_eq!(
            source
                .transfer_inbox()
                .snapshot_reported_facts(&[1; 32])
                .unwrap(),
            manifest
        );
        let db = rusqlite::Connection::open(target.backend.sqlite_database().unwrap()).unwrap();
        db.execute_batch("CREATE TRIGGER transfer228_authority_fault BEFORE INSERT ON meta WHEN NEW.key GLOB 'native_transfer_v1:*:reported-materialized' BEGIN SELECT RAISE(ABORT,'injected authoritative commit fault'); END;").unwrap();
        assert!(target.transfer_inbox().materialize_reported_facts(&manifest).is_err());
        assert!(read(&target).is_empty(), "Facts rows and stamp must roll back with receipt failure");
        db.execute_batch("DROP TRIGGER transfer228_authority_fault").unwrap();
        drop(db);
        let checkpoint = target
            .transfer_inbox()
            .materialize_reported_facts(&manifest)
            .unwrap();
        assert_eq!(read(&target), expected);
        assert_eq!(
            target
                .transfer_inbox()
                .materialize_reported_facts(&manifest)
                .unwrap(),
            checkpoint
        );
        let later = stage(&source, &target, &[2; 32]);
        assert!(matches!(
            target.transfer_inbox().materialize_reported_facts(&later),
            Err(StorageError::Conflict)
        ));
        assert_eq!(
            read(&target),
            expected,
            "occupied target must stay untouched"
        );
        let mut foreign = manifest.clone();
        foreign.namespace = "foreign".into();
        foreign.seal().unwrap();
        assert!(matches!(
            target.transfer_inbox().materialize_reported_facts(&foreign),
            Err(StorageError::NamespaceMismatch)
        ));
        assert_eq!(read(&target), expected);
        let corrupt = sqlite(&root, "corrupt");
        let mut broken = manifest.clone();
        broken
            .categories
            .iter_mut()
            .find(|c| c.category == Category::ReportedFacts)
            .unwrap()
            .digest = [7; 32];
        broken.seal().unwrap();
        let inbox = corrupt.transfer_inbox();
        inbox.begin(&broken).unwrap();
        let mut previous = [0; 32];
        for sequence in 1..=checkpoint.sequence {
            let mut page = source
                .transfer_inbox()
                .staged_page(&manifest, sequence)
                .unwrap();
            page.snapshot = broken.digest;
            page.previous = previous;
            page.seal().unwrap();
            previous = page.digest;
            inbox.stage_page(&broken, &page).unwrap();
        }
        assert!(inbox.materialize_reported_facts(&broken).is_err());
        assert!(read(&corrupt).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}

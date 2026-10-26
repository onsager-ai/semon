//! Existing native immutable page contracts, carried by the common transfer.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSource {
    pub path: String,
    pub generation: u64,
    pub receipt: String,
    pub checkpoint: u64,
    pub parser_version: u32,
    pub usage_ready: bool,
    pub chunks_header: String,
    pub launch_header: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativePageKind {
    Chunks,
    Launch,
    Tree,
}

impl NativeSource {
    pub(crate) fn id(
        &self,
        namespace: &str,
    ) -> Result<super::super::proof_lifetime::ProofGenerationId, StorageError> {
        let binding = serde_json::to_string(&(
            &self.path,
            &self.receipt,
            &self.chunks_header,
            self.launch_header.as_ref(),
        ))
        .map_err(|_| StorageError::ScopeChanged)?;
        Ok(super::super::proof_lifetime::ProofGenerationId::derive(
            namespace,
            self.generation,
            "native-file",
            &binding,
        ))
    }
}

pub(crate) fn metadata_key(path: &str, key: &str) -> bool {
    [
        "native_parent_marker:",
        "native_codex_header:",
        "native_claude_sidecar:",
    ]
    .iter()
    .any(|prefix| key.strip_prefix(prefix) == Some(path))
}

pub(crate) fn validate(record: &TransferRecord) -> Result<(), StorageError> {
    let bad = || StorageError::ScopeChanged;
    match record {
        TransferRecord::NativeSource(source) => {
            if source.path.is_empty()
                || source.generation == 0
                || source.generation > i64::MAX as u64
                || source.parser_version != crate::events::CACHE_VERSION
            {
                return Err(bad());
            }
            let header: crate::events::native_pages::chunks::Header =
                serde_json::from_str(&source.chunks_header).map_err(|_| bad())?;
            if header.version != 1
                || header.parser_version != source.parser_version
                || header.ledger != source.receipt
                || header.consumed_offset != Some(source.checkpoint)
            {
                return Err(bad());
            }
            if let Some(encoded) = &source.launch_header {
                let header: crate::events::native_pages::launch::Header =
                    serde_json::from_str(encoded).map_err(|_| bad())?;
                if header.version != 1
                    || header.parser_version != source.parser_version
                    || header.ledger != source.receipt
                {
                    return Err(bad());
                }
            }
        }
        TransferRecord::NativePage {
            path,
            kind,
            key,
            payload,
        } => {
            if path.is_empty() {
                return Err(bad());
            }
            match kind {
                NativePageKind::Chunks => {
                    let page: crate::events::native_pages::chunks::Page =
                        serde_json::from_str(payload).map_err(|_| bad())?;
                    if page.version != 1
                        || *key != format!("{:010}", page.index)
                        || page.sha256
                            != format!(
                                "{:x}",
                                Sha256::digest(page.canonical().map_err(|_| bad())?.as_bytes())
                            )
                    {
                        return Err(bad());
                    }
                }
                NativePageKind::Launch => {
                    let page: crate::events::native_pages::launch::Page =
                        serde_json::from_str(payload).map_err(|_| bad())?;
                    if page.version != 1 || *key != format!("{:010}", page.index) {
                        return Err(bad());
                    }
                }
                NativePageKind::Tree => {
                    let _: [u8; 32] = serde_json::from_str(payload).map_err(|_| bad())?;
                    if key.len() != 14
                        || key.as_bytes()[3] != b':'
                        || !key
                            .bytes()
                            .enumerate()
                            .all(|(i, b)| i == 3 || b.is_ascii_digit())
                    {
                        return Err(bad());
                    }
                }
            }
        }
        TransferRecord::NativeMetadata { path, key, value } => {
            if !metadata_key(path, key) {
                return Err(bad());
            }
            let _: serde_json::Value = serde_json::from_str(value).map_err(|_| bad())?;
        }
        _ => return Err(StorageError::InvalidArguments),
    }
    Ok(())
}

pub(crate) fn append(
    tx: &mut impl InboxTransaction,
    state: &mut reported::CaptureState,
    category: Category,
    records: Vec<TransferRecord>,
) -> Result<(), StorageError> {
    let mut fitted = Vec::new();
    let mut bytes = 4096usize;
    for record in records {
        validate(&record)?;
        let size = encode_bounded(&record, MAX_PAGE_BYTES)?.len() + 1;
        if bytes.checked_add(size).is_none_or(|n| n > MAX_PAGE_BYTES) {
            break;
        }
        bytes += size;
        fitted.push(record);
    }
    if fitted.is_empty() {
        return Err(StorageError::Busy);
    }
    state.native_key = Some(match fitted.last().ok_or(StorageError::ScopeChanged)? {
        TransferRecord::NativePage { key, .. } | TransferRecord::NativeMetadata { key, .. } => {
            key.clone()
        }
        _ => return Err(StorageError::ScopeChanged),
    });
    reported::append_capture(tx, state, category, fitted)
}

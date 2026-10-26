//! Existing native immutable page contracts, carried by the common transfer.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSource {
    pub path: String,
    pub backing: Option<String>,
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
    #[cfg(feature = "postgres")]
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
            if let Some(backing) = &source.backing
                && *backing != crate::source_carrier::ledger_digest_value(&source.receipt)
            {
                return Err(bad());
            }
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
                || header.consumed_offset.is_none_or(|offset| {
                    source.checkpoint > offset
                        || (source.usage_ready && source.checkpoint != offset)
                })
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
            backing,
            kind,
            key,
            payload,
        } => {
            if path.is_empty()
                || backing
                    .as_ref()
                    .is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
            {
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

pub(crate) fn validate_source_ledger(
    source: &NativeSource,
    ledger: &crate::events::Ledger,
) -> Result<(), StorageError> {
    let header: crate::events::native_pages::chunks::Header =
        serde_json::from_str(&source.chunks_header).map_err(|_| StorageError::ScopeChanged)?;
    if source.receipt != crate::events::store::header_ledger(ledger)
        || header.consumed_offset != Some(ledger.offset)
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

/// Validate one existing tree node against its already transferred children.
/// Key order puts children before parents, so work stays bounded per page.
pub(crate) fn verify_tree_node(
    header: &str,
    key: &str,
    payload: &str,
    mut get: impl FnMut(NativePageKind, &str) -> Result<String, StorageError>,
) -> Result<(), StorageError> {
    let bad = || StorageError::ScopeChanged;
    let header: crate::events::native_pages::chunks::Header = serde_json::from_str(header).map_err(|_|bad())?;
    let (level,index) = key.split_once(':').ok_or_else(bad)?;
    let level: u32 = level.parse().map_err(|_|bad())?;
    let index: usize = index.parse().map_err(|_|bad())?;
    if level >= usize::BITS { return Err(bad()); }
    let mut width = header.count.div_ceil(1024);
    for _ in 0..level {
        if width <= 1 { return Err(bad()); }
        width = width.div_ceil(2);
    }
    if index >= width { return Err(bad()); }
    let expected: [u8;32] = if level == 0 {
        let page: crate::events::native_pages::chunks::Page = serde_json::from_str(&get(NativePageKind::Chunks,&format!("{index:010}"))?).map_err(|_|bad())?;
        if page.index != index || page.hashes.len() != (header.count-index*1024).min(1024) {return Err(bad());}
        Sha256::digest(page.canonical().map_err(|_|bad())?.as_bytes()).into()
    } else {
        let previous_width = header.count.div_ceil(1024).div_ceil(1usize << (level-1));
        let left: [u8;32] = serde_json::from_str(&get(NativePageKind::Tree,&format!("{:03}:{:010}",level-1,index*2))?).map_err(|_|bad())?;
        let right: [u8;32] = if index*2+1 < previous_width {
            serde_json::from_str(&get(NativePageKind::Tree,&format!("{:03}:{:010}",level-1,index*2+1))?).map_err(|_|bad())?
        } else {left};
        let mut hash = Sha256::new();
        hash.update(left);
        hash.update(right);
        hash.finalize().into()
    };
    let actual: [u8;32] = serde_json::from_str(payload).map_err(|_|bad())?;
    if actual != expected || (width == 1 && actual != header.pages_root) {return Err(bad());}
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

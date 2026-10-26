//! Physical mapping of existing native page metadata, not Raw replay.
use super::*;
use crate::storage::transfer::{
    Category, NativePageKind, NativeSource, TransferRecord, native_pages, reported::CaptureState,
};
use sha2::{Digest, Sha256};

fn prefix(path: &str, kind: NativePageKind) -> String {
    let hash = format!("{:x}", Sha256::digest(path.as_bytes()));
    match kind {
        NativePageKind::Chunks => format!("native_source_chunks:{hash}:"),
        NativePageKind::Launch => format!("native_launch_calls:{hash}:"),
        NativePageKind::Tree => format!("native_proof_tree:native_source_chunks:{hash}::"),
    }
}
fn backing_prefix(path: &str, kind: NativePageKind, backing: Option<&str>) -> String {
    match backing {
        None => prefix(path, kind),
        Some(digest) => {
            let chunks = format!(
                "native_published_chunks:{:x}:{digest}:",
                Sha256::digest(path.as_bytes())
            );
            match kind {
                NativePageKind::Chunks => chunks,
                NativePageKind::Tree => format!("native_proof_tree:{chunks}:"),
                NativePageKind::Launch => format!("{chunks}launch:"),
            }
        }
    }
}
fn metadata(tx: &Inbox<'_>, key: &str) -> Result<Option<String>, StorageError> {
    let value:Option<Option<String>>=tx.0.query_row("SELECT CASE WHEN octet_length(value)<=1048576 THEN value END FROM meta WHERE namespace=?1 AND key=?2",params![tx.1,key],|r|r.get(0)).optional().map_err(failed)?;
    value.map(|v| v.ok_or(StorageError::Busy)).transpose()
}

pub(super) fn verify_backing(
    tx: &Inbox<'_>,
    row: &str,
    path: &str,
    digest: &str,
) -> Result<(), StorageError> {
    let row: crate::model::summary::CatalogRow =
        serde_json::from_str(row).map_err(|_| StorageError::ScopeChanged)?;
    let source = row
        .sources
        .iter()
        .find(|source| {
            source.path.to_str() == Some(path)
                && crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(source))
                    == digest
        })
        .ok_or(StorageError::ScopeChanged)?;
    let receipt = crate::events::store::header_ledger(&crate::source_carrier::ledger(source));
    let current = metadata(
        tx,
        &format!("{}header", prefix(path, NativePageKind::Chunks)),
    )?
    .ok_or(StorageError::ScopeChanged)?;
    let current_header: crate::events::native_pages::chunks::Header =
        serde_json::from_str(&current).map_err(|_| StorageError::ScopeChanged)?;
    let backing = (current_header.ledger != receipt).then_some(digest);
    let encoded = metadata(
        tx,
        &format!(
            "{}header",
            backing_prefix(path, NativePageKind::Chunks, backing)
        ),
    )?
    .ok_or(StorageError::ScopeChanged)?;
    let header: crate::events::native_pages::chunks::Header =
        serde_json::from_str(&encoded).map_err(|_| StorageError::ScopeChanged)?;
    if header.ledger != receipt
        || header.consumed_offset != Some(source.offset)
        || header.version != 1
        || header.parser_version != crate::events::CACHE_VERSION
    {
        return Err(StorageError::ScopeChanged);
    }
    let pages = header.count.div_ceil(1024);
    let mut nodes = 0usize;
    let mut width = pages;
    while width > 0 {
        nodes = nodes.checked_add(width).ok_or(StorageError::ScopeChanged)?;
        if width == 1 {
            break;
        }
        width = width.div_ceil(2);
    }
    for (kind, length, expected) in [
        (NativePageKind::Chunks, 10, pages),
        (NativePageKind::Tree, 14, nodes),
    ] {
        let prefix = backing_prefix(path, kind, backing);
        let count:i64=tx.0.query_row("SELECT count(*) FROM meta WHERE namespace=?1 AND key>=?2 AND key<?3 AND length(key)=length(?2)+?4",params![tx.1,prefix,format!("{prefix}~"),length],|r|r.get(0)).map_err(failed)?;
        if usize::try_from(count).ok() != Some(expected) {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(())
}

pub(super) fn capture(
    tx: &mut Inbox<'_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    if state.native_family >= 8 {
        return super::rows::capture(tx, state);
    }
    let path = state
        .native_path
        .clone()
        .ok_or(StorageError::ScopeChanged)?;
    if state.native_family == 3 {
        let chunks_header = metadata(
            tx,
            &format!(
                "{}header",
                backing_prefix(
                    &path,
                    NativePageKind::Chunks,
                    state.native_backing.as_deref()
                )
            ),
        )?
        .ok_or(StorageError::ScopeChanged)?;
        let header: crate::events::native_pages::chunks::Header =
            serde_json::from_str(&chunks_header).map_err(|_| StorageError::ScopeChanged)?;
        let (generation, receipt, checkpoint, parser_version, usage_ready): (
            i64,
            String,
            i64,
            u32,
            bool,
        ) = if state.native_backing.is_some() {
            let original = metadata(
                tx,
                &format!(
                    "{}source",
                    backing_prefix(
                        &path,
                        NativePageKind::Chunks,
                        state.native_backing.as_deref()
                    )
                ),
            )?;
            let generation = match original {
                Some(original) => original
                    .parse::<i64>()
                    .map_err(|_| StorageError::ScopeChanged)?,
                None => {
                    // Existing qualified SQLite prefixes bind their ledger and
                    // chunk root, not a historical adapter-local Usage row ID.
                    // Translate that physical address without inventing usage
                    // authority: retained usage_ready stays false. The target
                    // page ID additionally binds the path and original header.
                    let digest = Sha256::digest(chunks_header.as_bytes());
                    let mut address = [0; 8];
                    address.copy_from_slice(&digest[..8]);
                    (u64::from_be_bytes(address) & i64::MAX as u64).max(1) as i64
                }
            };
            (
                generation,
                header.ledger.clone(),
                i64::try_from(header.consumed_offset.ok_or(StorageError::ScopeChanged)?)
                    .map_err(|_| StorageError::ScopeChanged)?,
                header.parser_version,
                false,
            )
        } else {
            tx.0.query_row("SELECT s.source_id,s.receipt,s.checkpoint,s.parser_version,s.usage_ready FROM native_usage_current c JOIN native_usage_sources s USING(namespace,source_id) WHERE c.namespace=?1 AND c.source_path=?2",params![tx.1,path],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(failed)?
        };
        let source = NativeSource {
            path: path.clone(),
            backing: state.native_backing.clone(),
            generation: u64::try_from(generation).map_err(|_| StorageError::ScopeChanged)?,
            receipt,
            checkpoint: checkpoint as u64,
            parser_version,
            usage_ready,
            chunks_header,
            launch_header: metadata(
                tx,
                &format!(
                    "{}header",
                    backing_prefix(
                        &path,
                        NativePageKind::Launch,
                        state.native_backing.as_deref()
                    )
                ),
            )?,
        };
        let record = TransferRecord::NativeSource(source);
        native_pages::validate(&record)?;
        transfer::reported::append_capture(tx, state, Category::PageGenerations, vec![record])?;
        state.native_family = 4;
        state.native_key = None;
    } else if state.native_family <= 6 {
        let kind = match state.native_family {
            4 => NativePageKind::Chunks,
            5 => NativePageKind::Launch,
            _ => NativePageKind::Tree,
        };
        let prefix = backing_prefix(&path, kind, state.native_backing.as_deref());
        let after = state.native_key.as_ref().map(|k| format!("{prefix}{k}"));
        let suffix = "FROM meta WHERE namespace=?1 AND key>=?2 AND key<?3 AND key<>?4 AND key<>?2||'source' AND (?5 IS NULL OR key>?5) ORDER BY key";
        let sizes =
            tx.0.prepare(&format!("SELECT octet_length(value) {suffix} LIMIT 64"))
                .map_err(failed)?
                .query_map(
                    params![
                        tx.1,
                        prefix,
                        format!("{prefix}~"),
                        format!("{prefix}header"),
                        after
                    ],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(failed)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(failed)?;
        let mut count = 0;
        let mut bytes = 0usize;
        for size in &sizes {
            let size = usize::try_from(*size).map_err(|_| StorageError::ScopeChanged)?;
            if bytes
                .checked_add(size)
                .is_none_or(|n| n > transfer::MAX_PAGE_BYTES)
            {
                break;
            }
            bytes += size;
            count += 1;
        }
        if count == 0 && !sizes.is_empty() {
            return Err(StorageError::Busy);
        }
        let rows =
            tx.0.prepare(&format!("SELECT key,value {suffix} LIMIT {count}"))
                .map_err(failed)?
                .query_map(
                    params![
                        tx.1,
                        prefix,
                        format!("{prefix}~"),
                        format!("{prefix}header"),
                        after
                    ],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .map_err(failed)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(failed)?;
        if rows.is_empty() {
            state.native_family += 1;
            state.native_key = None;
        } else {
            let records = rows
                .into_iter()
                .map(|(key, value)| {
                    Ok(TransferRecord::NativePage {
                        path: path.clone(),
                        backing: state.native_backing.clone(),
                        kind,
                        key: key
                            .strip_prefix(&prefix)
                            .ok_or(StorageError::ScopeChanged)?
                            .into(),
                        payload: value.ok_or(StorageError::Busy)?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            native_pages::append(tx, state, Category::PageBlobs, records)?;
        }
    } else {
        let manifest_key = format!(
            "native_published_chunks_manifest:{:x}",
            Sha256::digest(path.as_bytes())
        );
        let next: Option<String> = tx.0.query_row("SELECT j.value FROM meta m,json_each(m.value) j WHERE m.namespace=?1 AND m.key=?2 AND (?3 IS NULL OR j.value>?3) ORDER BY j.value LIMIT 1", params![tx.1,manifest_key,state.native_backing], |r|r.get(0)).optional().map_err(failed)?;
        if let Some(next) = next {
            let current_header = metadata(
                tx,
                &format!("{}header", prefix(&path, NativePageKind::Chunks)),
            )?
            .ok_or(StorageError::ScopeChanged)?;
            let header: crate::events::native_pages::chunks::Header =
                serde_json::from_str(&current_header).map_err(|_| StorageError::ScopeChanged)?;
            if crate::source_carrier::ledger_digest_value(&header.ledger) == next {
                let retained = metadata(
                    tx,
                    &format!(
                        "{}header",
                        backing_prefix(&path, NativePageKind::Chunks, Some(&next))
                    ),
                )?
                .ok_or(StorageError::ScopeChanged)?;
                if retained != current_header {
                    return Err(StorageError::ScopeChanged);
                }
                state.native_backing = Some(next);
                state.native_key = None;
                transfer::reported::save_capture(tx, state)?;
                return Ok(state.progress());
            }
            state.native_backing = Some(next);
            state.native_family = 3;
            state.native_key = None;
            transfer::reported::save_capture(tx, state)?;
            return Ok(state.progress());
        }
        let mut records = Vec::new();
        for prefix in [
            "native_claude_sidecar:",
            "native_codex_header:",
            "native_parent_marker:",
        ] {
            let key = format!("{prefix}{path}");
            if state.native_key.as_ref().is_some_and(|old| key <= *old) {
                continue;
            }
            if let Some(value) = metadata(tx, &key)? {
                records.push(TransferRecord::NativeMetadata {
                    path: path.clone(),
                    key,
                    value,
                });
            }
        }
        if records.is_empty() {
            state.native_family = 8;
            state.native_key = None;
            state.native_backing = None;
        } else {
            native_pages::append(tx, state, Category::NativeHeaders, records)?;
        }
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply(tx: &mut Inbox<'_>, record: &TransferRecord) -> Result<(), StorageError> {
    native_pages::validate(record)?;
    match record {
        TransferRecord::NativeSource(source) => {
            if source.backing.is_some() && source.launch_header.is_some() {
                return Err(StorageError::NamespaceUpgradePending);
            }
            if source.backing.is_none() {
                let (_, ledger) = crate::events::store::ledger_row(tx.0, &source.path)
                    .map_err(failed)?
                    .ok_or(StorageError::ScopeChanged)?;
                native_pages::validate_source_ledger(source, &ledger)?;
                tx.0.execute("INSERT INTO native_usage_sources(namespace,source_id,source_path,receipt,checkpoint,parser_version,usage_ready) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![tx.1,source.generation as i64,source.path,source.receipt,source.checkpoint as i64,source.parser_version,source.usage_ready]).map_err(|error| {
                #[cfg(test)]
                eprintln!("transfer SQLite NativeSource apply failed: table=native_usage_sources");
                failed(error)
            })?;
                tx.0.execute("INSERT INTO native_usage_current(namespace,source_path,source_id) VALUES(?1,?2,?3)",params![tx.1,source.path,source.generation as i64]).map_err(|error| {
                #[cfg(test)]
                eprintln!("transfer SQLite NativeSource apply failed: table=native_usage_current");
                failed(error)
            })?;
            }
            tx.put(
                &format!(
                    "{}header",
                    backing_prefix(
                        &source.path,
                        NativePageKind::Chunks,
                        source.backing.as_deref()
                    )
                ),
                source.chunks_header.as_bytes(),
            )?;
            if let Some(header) = &source.launch_header {
                tx.put(
                    &format!(
                        "{}header",
                        backing_prefix(
                            &source.path,
                            NativePageKind::Launch,
                            source.backing.as_deref()
                        )
                    ),
                    header.as_bytes(),
                )?;
            }
            if let Some(digest) = &source.backing {
                tx.put(
                    &format!(
                        "{}source",
                        backing_prefix(&source.path, NativePageKind::Chunks, Some(digest))
                    ),
                    source.generation.to_string().as_bytes(),
                )?;
                let key = format!(
                    "native_published_chunks_manifest:{:x}",
                    Sha256::digest(source.path.as_bytes())
                );
                let mut saved: std::collections::BTreeSet<String> = metadata(tx, &key)?
                    .map(|s| serde_json::from_str(&s).map_err(|_| StorageError::ScopeChanged))
                    .transpose()?
                    .unwrap_or_default();
                saved.insert(digest.clone());
                tx.put(
                    &key,
                    serde_json::to_string(&saved)
                        .map_err(|_| StorageError::ScopeChanged)?
                        .as_bytes(),
                )?;
            }
        }
        TransferRecord::NativePage {
            path,
            backing,
            kind,
            key,
            payload,
        } => {
            if *kind == NativePageKind::Tree {
                let header = metadata(
                    tx,
                    &format!(
                        "{}header",
                        backing_prefix(path, NativePageKind::Chunks, backing.as_deref())
                    ),
                )?
                .ok_or(StorageError::ScopeChanged)?;
                native_pages::verify_tree_node(&header, key, payload, |kind, key| {
                    metadata(
                        tx,
                        &format!("{}{key}", backing_prefix(path, kind, backing.as_deref())),
                    )?
                    .ok_or(StorageError::ScopeChanged)
                })?;
            }
            tx.put(
                &format!("{}{key}", backing_prefix(path, *kind, backing.as_deref())),
                payload.as_bytes(),
            )?;
        }
        TransferRecord::NativeMetadata { key, value, .. } => tx.put(key, value.as_bytes())?,
        _ => return Err(StorageError::InvalidArguments),
    }
    Ok(())
}

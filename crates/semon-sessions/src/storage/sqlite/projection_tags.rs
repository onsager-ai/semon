//! Current projection cache tags. Native bytes and retained custody stay authoritative.
use super::failed;
use crate::model::summary::CatalogRow;
use crate::storage::{QueryRuntimeBinding, StorageError, catalog::CatalogObservationOwner};
use rusqlite::{Connection, OptionalExtension, params};

const PROJECTOR: &str = "native-projection-v1";
pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS storage_projection_tags(namespace TEXT NOT NULL,session_key TEXT NOT NULL,owner_kind TEXT NOT NULL CHECK(owner_kind IN('catalog','history')),raw_identity TEXT NOT NULL,prefix_digest TEXT NOT NULL,projector TEXT NOT NULL,generation TEXT NOT NULL,capture TEXT CHECK(capture IS NULL OR octet_length(capture)<=8388608),PRIMARY KEY(namespace,session_key,owner_kind)) STRICT, WITHOUT ROWID;
";

fn kind(owner: CatalogObservationOwner) -> &'static str {
    match owner {
        CatalogObservationOwner::Catalog => "catalog",
        CatalogObservationOwner::History => "history",
    }
}

fn identity(row: &CatalogRow) -> Result<String, StorageError> {
    use sha2::{Digest, Sha256};
    let sources = row
        .sources
        .iter()
        .map(|source| (&source.path, source.dev, source.ino, &source.native_id))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&(&row.key, &row.harness, sources))
        .map_err(|_| StorageError::Unavailable)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn publish(
    db: &Connection,
    namespace: &str,
    row: &CatalogRow,
    owner: CatalogObservationOwner,
    generation: &str,
    observations: Option<&crate::storage::selected::PresentationObservations>,
) -> Result<(), StorageError> {
    let raw_identity = identity(row)?;
    let prefix = crate::retention::source_generation(&row.sources)
        .map_err(|_| StorageError::ScopeChanged)?;
    let capture = observations
        .and_then(|observations| observations.runtime_capture_for(row))
        .map(|capture| serde_json::to_string(&capture).map_err(|_| StorageError::Unavailable))
        .transpose()?;
    if capture
        .as_ref()
        .is_some_and(|capture| capture.len() > 8 * 1024 * 1024)
    {
        return Err(StorageError::HistoryRetentionBounds);
    }
    db.prepare_cached("INSERT INTO storage_projection_tags(namespace,session_key,owner_kind,raw_identity,prefix_digest,projector,generation,capture) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(namespace,session_key,owner_kind) DO UPDATE SET raw_identity=excluded.raw_identity,prefix_digest=excluded.prefix_digest,projector=excluded.projector,generation=excluded.generation,capture=excluded.capture")
        .map_err(failed)?.execute(params![namespace,row.key,kind(owner),raw_identity,prefix,PROJECTOR,generation,capture]).map_err(failed)?;
    Ok(())
}

pub(crate) fn check(
    db: &Connection,
    current: &QueryRuntimeBinding,
    row: &CatalogRow,
    owner: CatalogObservationOwner,
) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
    check_tag(db, current, row, owner, true)
}

/// Custody keeps source identity even when a recipe version cannot be decoded.
/// Such a predecessor contributes metadata and CWDs, never unsupported slots.
pub(crate) fn check_custody(
    db: &Connection,
    current: &QueryRuntimeBinding,
    row: &CatalogRow,
    owner: CatalogObservationOwner,
) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
    check_tag(db, current, row, owner, false)
}

fn check_tag(
    db: &Connection,
    current: &QueryRuntimeBinding,
    row: &CatalogRow,
    owner: CatalogObservationOwner,
    require_recipe: bool,
) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
    let tag: Option<(String,String,String,String,Option<String>)> = db.query_row(
        "SELECT raw_identity,prefix_digest,projector,generation,CASE WHEN octet_length(capture)<=8388608 THEN capture END FROM storage_projection_tags WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3",
        params![current.namespace.as_str(),row.key,kind(owner)],
        |saved| Ok((saved.get(0)?,saved.get(1)?,saved.get(2)?,saved.get(3)?,saved.get(4)?)),
    ).optional().map_err(failed)?;
    let (raw_identity, prefix, projector, generation, capture) =
        tag.ok_or(StorageError::ScopeChanged)?;
    if projector != PROJECTOR
        || raw_identity != identity(row)?
        || prefix
            != crate::retention::source_generation(&row.sources)
                .map_err(|_| StorageError::ScopeChanged)?
    {
        return Err(StorageError::ScopeChanged);
    }
    for source in &row.sources {
        if !crate::events::store::chunk_proofs::projection_header_ready(
            db,
            &source.path.to_string_lossy(),
            &crate::source_carrier::ledger(source),
        )
        .map_err(failed)?
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    let (table, projection) = match owner {
        CatalogObservationOwner::Catalog => ("session_catalog", "session_slot_projections"),
        CatalogObservationOwner::History => {
            ("session_history_catalog", "session_history_projections")
        }
    };
    let metadata: String = db.query_row(&format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.{table} WHERE namespace=?1 AND session_key=?2"), params![current.namespace.as_str(),row.key], |saved| saved.get(0)).map_err(failed)?;
    let mut stored: CatalogRow =
        serde_json::from_str(&metadata).map_err(|_| StorageError::Unavailable)?;
    let mut selected = row.clone();
    stored.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    selected.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    if serde_json::to_value(stored).map_err(|_| StorageError::Unavailable)?
        != serde_json::to_value(selected).map_err(|_| StorageError::Unavailable)?
    {
        return Err(StorageError::ScopeChanged);
    }
    let header: Option<(String,Option<String>,u32)> = db.query_row(&format!("SELECT generation,source_generation,version FROM main.{projection} WHERE namespace=?1 AND session_key=?2"), params![current.namespace.as_str(),row.key], |saved| Ok((saved.get(0)?,saved.get(1)?,saved.get(2)?))).optional().map_err(failed)?;
    if let Some((actual, source, version)) = header {
        if actual != generation
            || source.as_deref() != Some(prefix.as_str())
            || (require_recipe && version != crate::slot_projection::VERSION)
        {
            return Err(StorageError::ScopeChanged);
        }
    } else if !generation.is_empty() {
        return Err(StorageError::ScopeChanged);
    }
    capture
        .map(|capture| {
            let observations: crate::storage::selected::PresentationObservations =
                serde_json::from_str(&capture).map_err(|_| StorageError::ScopeChanged)?;
            if !observations.is_runtime_capture_for(row) {
                return Err(StorageError::ScopeChanged);
            }
            Ok(observations)
        })
        .transpose()
}

/// Withdrawal/replacement hands the exact old cache owner to durable History in
/// the same transaction, before the Current row or its tag can be overwritten.
pub(crate) fn retain(
    db: &Connection,
    namespace: &str,
    row: &CatalogRow,
) -> Result<(), StorageError> {
    let prefix = crate::retention::source_generation(&row.sources)
        .map_err(|_| StorageError::ScopeChanged)?;
    db.execute("INSERT INTO storage_projection_tags(namespace,session_key,owner_kind,raw_identity,prefix_digest,projector,generation,capture) SELECT namespace,session_key,'history',raw_identity,prefix_digest,projector,generation,capture FROM storage_projection_tags WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog' AND raw_identity=?3 AND prefix_digest=?4 AND projector=?5 ON CONFLICT(namespace,session_key,owner_kind) DO NOTHING", params![namespace,row.key,identity(row)?,prefix,PROJECTOR]).map_err(failed)?;
    Ok(())
}

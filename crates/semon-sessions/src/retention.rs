//! Readiness and explicit archive retention of native source metadata. No body reads,
//! provider credentials, archive locations, native selection or control grant.
use crate::{
    Options, SessionSourceProof, SessionSourceRef,
    catalog::CatalogIdentityError,
    events::EventCache,
    model::summary::{CatalogRow, CatalogSource},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProjectionReady {
    pub source: SessionSourceRef,
    pub projection_version: u32,
    pub projection_generation: String,
    /// Observation lifecycle only; independent of native/runtime authority.
    pub lifecycle: String,
}

pub(crate) fn source_generation(sources: &[CatalogSource]) -> Result<String, serde_json::Error> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(sources)?)
    ))
}

/// Looks up an exact allowlisted source path without initializing any index.
/// The host supplies authorized options, verifies the registered archive and
/// actual physical bytes against this consumed-prefix reference, and rechecks
/// before removal under its lifecycle gates. Readiness is not permission to
/// delete bytes or to control the native session. A retained result is valid
/// historical provenance even when current native selection retired it.
///
/// Current observation mappings take precedence over retained mappings. Within
/// the selected lifecycle, every candidate must have a supported, source-bound
/// recipe header and the same source reference; ambiguity returns not-ready.
/// No prefix/record or complete transcript is loaded. The containing session's
/// metadata is necessary relationship context, bounded to at most eight owners.
pub fn source_projection_ready(
    options: &Options,
    root: &str,
    path: &str,
) -> Result<Option<SourceProjectionReady>, CatalogIdentityError> {
    use CatalogIdentityError::{InvalidArguments, Unavailable};
    let root_kind = crate::inputs::InputRoot::parse(root).ok_or(InvalidArguments)?;
    if !path.ends_with(".jsonl") || !crate::is_input_path(root, path) {
        return Err(InvalidArguments);
    }
    let input = crate::inputs::Input {
        root: root_kind,
        path: path.into(),
    };
    let full = input.full_path(options);
    let Some(full) = full.to_str() else {
        return Err(InvalidArguments);
    };
    let index = EventCache::path(&options.cache);
    if !index.exists() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(
        index,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| Unavailable)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|_| Unavailable)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| Unavailable)?;
    projection_ready(&transaction, root, path, full)
}

fn projection_ready(
    transaction: &Connection,
    root: &str,
    path: &str,
    full: &str,
) -> Result<Option<SourceProjectionReady>, CatalogIdentityError> {
    use CatalogIdentityError::{InvalidArguments, Unavailable};
    let version: Option<String> = transaction
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| Unavailable)?;
    if version.as_deref() != Some(crate::model::summary::CATALOG_VERSION.to_string().as_str()) {
        return Ok(None);
    }
    let partial: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='catalog_completeness' AND value='partial')",[],|row|row.get(0)).map_err(|_|Unavailable)?;
    if partial {
        return Ok(None);
    }
    let coherent: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM meta a JOIN meta b ON a.value=b.value WHERE a.key='catalog_generation' AND b.key='slot_projection_catalog_generation')",[],|row|row.get(0)).map_err(|_|Unavailable)?;
    if !coherent {
        return Ok(None);
    }
    for lifecycle in ["current", "retained"] {
        let history = lifecycle == "retained" && transaction.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='session_history_catalog')",[],|row|row.get::<_,bool>(0)).map_err(|_|Unavailable)?;
        let tables = if history {
            "LEFT JOIN session_history_catalog USING(session_key) LEFT JOIN session_history_projections USING(session_key)"
        } else {
            "LEFT JOIN session_slot_projections USING(session_key)"
        };
        let metadata = if history {
            "coalesce(session_history_catalog.metadata,session_catalog.metadata)"
        } else {
            "session_catalog.metadata"
        };
        let header = if history {
            "CASE WHEN session_history_projections.incomplete=1 THEN NULL ELSE coalesce(session_history_projections.version,session_slot_projections.version) END,coalesce(session_history_projections.generation,session_slot_projections.generation),coalesce(session_history_projections.source_generation,session_slot_projections.source_generation)"
        } else {
            "session_slot_projections.version,session_slot_projections.generation,session_slot_projections.source_generation"
        };
        let current_header = if history {
            "LEFT JOIN session_slot_projections USING(session_key)"
        } else {
            ""
        };
        let mut statement=transaction.prepare(&format!("SELECT CASE WHEN octet_length({metadata})<=1048576 THEN {metadata} ELSE NULL END,{header} FROM session_catalog_sources INDEXED BY session_catalog_current_source_path JOIN session_catalog USING(session_key) {tables} {current_header} WHERE session_catalog_sources.lifecycle=?1 AND source_path=?2 ORDER BY session_catalog_sources.session_key LIMIT 9")).map_err(|_|Unavailable)?;
        let candidates = statement
            .query_map([lifecycle, full], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<u32>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|_| Unavailable)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Unavailable)?;
        if candidates.is_empty() {
            continue;
        }
        if candidates.len() > 8 {
            return Ok(None);
        }
        let mut result: Option<SourceProjectionReady> = None;
        for (metadata, version, generation, source_hash) in candidates {
            let Some(metadata) = metadata else {
                return Ok(None);
            };
            let Ok(row) = serde_json::from_str::<CatalogRow>(&metadata) else {
                return Ok(None);
            };
            if row.sources.len() > 64
                || row.harness != root
                || (lifecycle == "current" && row.lifecycle.as_str() != lifecycle)
            {
                return Ok(None);
            }
            let Some(source) = row
                .sources
                .iter()
                .find(|source| source.path.to_str() == Some(full))
            else {
                return Ok(None);
            };
            // A recipe can still describe a coherent old prefix while a newer
            // source observation awaits publication. Preserve it for history,
            // but do not qualify another local eviction during that interval.
            for dependency in &row.sources {
                let pending: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                    [dependency.path.to_str().ok_or(InvalidArguments)?],
                    |row| row.get(0),
                ).map_err(|_| Unavailable)?;
                if pending {
                    return Ok(None);
                }
            }
            let expected = source_generation(&row.sources).map_err(|_| Unavailable)?;
            if version != Some(crate::slot_projection::VERSION)
                || source_hash.as_deref() != Some(expected.as_str())
            {
                return Ok(None);
            }
            let Some(generation) = generation.filter(|value| {
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) else {
                return Ok(None);
            };
            let reference = SessionSourceRef {
                root: root.into(),
                path: path.into(),
                native_id: source.native_id.clone(),
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
            };
            if reference.native_id.is_empty()
                || result
                    .as_ref()
                    .is_some_and(|ready| ready.source != reference)
            {
                return Ok(None);
            }
            if result.is_none() {
                result = Some(SourceProjectionReady {
                    source: reference,
                    projection_version: crate::slot_projection::VERSION,
                    projection_generation: generation,
                    lifecycle: lifecycle.into(),
                });
            }
        }
        return Ok(result);
    }
    Ok(None)
}

fn same_projection(left: &SourceProjectionReady, right: &SourceProjectionReady) -> bool {
    left.source == right.source
        && left.projection_version == right.projection_version
        && left.projection_generation == right.projection_generation
}

fn archive_index_path(
    options: &Options,
    proof: &SessionSourceProof,
    projection: &SourceProjectionReady,
) -> Result<std::path::PathBuf, CatalogIdentityError> {
    use CatalogIdentityError::InvalidArguments;
    let source = &proof.source;
    let root = crate::InputRoot::parse(&source.root).ok_or(InvalidArguments)?;
    if source != &projection.source
        || !source.path.ends_with(".jsonl")
        || source.path.len() > 4096
        || !crate::is_input_path(&source.root, &source.path)
        || source.native_id.is_empty()
        || source.native_id.len() > 256
    {
        return Err(InvalidArguments);
    }
    Ok(crate::Input {
        root,
        path: source.path.clone(),
    }
    .full_path(options))
}

/// Retain one existing, decodable Session Event Index generation for a verified
/// archive. The host owns archive authorization, verifies the original bytes,
/// drains producers and rechecks physical identity before removing local bytes.
/// This metadata-only operation neither verifies an archive nor permits deletion.
/// It cannot initialize or migrate an index, and refuses changed catalog, slot or
/// source generations. No source bodies, provider keys or runtime facts are copied.
/// Ordinary missing sources remain subject to normal index removal.
pub fn retain_source_event_index(
    options: &Options,
    proof: &SessionSourceProof,
    projection: &SourceProjectionReady,
) -> Result<(), CatalogIdentityError> {
    use CatalogIdentityError::{InvalidArguments, ScopeChanged, Unavailable};
    let path = archive_index_path(options, proof, projection)?;
    let full = path.to_str().ok_or(InvalidArguments)?;
    let mut connection = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| Unavailable)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|_| Unavailable)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| Unavailable)?;
    if !projection_ready(&transaction, &proof.source.root, &proof.source.path, full)?
        .is_some_and(|ready| same_projection(&ready, projection))
        || !crate::events::retain_archive_index(&transaction, full, proof, projection)
            .map_err(|_| Unavailable)?
    {
        return Err(ScopeChanged);
    }
    transaction.commit().map_err(|_| Unavailable)
}

/// Recheck the explicit retention marker and its exact committed ledger under
/// one read transaction. The host uses this after its durable recovery marker
/// and before local removal. A native index write clears the marker; unsupported
/// versions and changed projections cannot qualify another eviction. Historical
/// lifecycle alone does not change the source-bound projection generation.
pub fn source_event_index_retained(
    options: &Options,
    proof: &SessionSourceProof,
    projection: &SourceProjectionReady,
) -> Result<bool, CatalogIdentityError> {
    use CatalogIdentityError::{InvalidArguments, Unavailable};
    let path = archive_index_path(options, proof, projection)?;
    let full = path.to_str().ok_or(InvalidArguments)?;
    let connection = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| Unavailable)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|_| Unavailable)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|_| Unavailable)?;
    Ok(
        projection_ready(&transaction, &proof.source.root, &proof.source.path, full)?
            .is_some_and(|ready| same_projection(&ready, projection))
            && crate::events::archive_index_retained(&transaction, full, proof, projection)
                .map_err(|_| Unavailable)?,
    )
}

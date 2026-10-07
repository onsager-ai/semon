//! Read-only readiness of retained native source projections. No body reads,
//! provider credentials, archive locations, native selection or control grant.
use crate::{
    Options, SessionSourceRef,
    catalog::CatalogIdentityError,
    events::EventCache,
    model::summary::{CatalogRow, CatalogSource},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
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
    let coherent: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM meta a JOIN meta b ON a.value=b.value WHERE a.key='catalog_generation' AND b.key='slot_projection_catalog_generation')",[],|row|row.get(0)).map_err(|_|Unavailable)?;
    if !coherent {
        return Ok(None);
    }
    for lifecycle in ["current", "retained"] {
        let mut statement=transaction.prepare("SELECT session_catalog.metadata,session_slot_projections.version,session_slot_projections.generation,session_slot_projections.source_generation FROM session_catalog_sources INDEXED BY session_catalog_current_source_path JOIN session_catalog USING(session_key) LEFT JOIN session_slot_projections USING(session_key) WHERE session_catalog_sources.lifecycle=?1 AND source_path=?2 ORDER BY session_catalog_sources.session_key LIMIT 9").map_err(|_|Unavailable)?;
        let candidates = statement
            .query_map([lifecycle, full], |row| {
                Ok((
                    row.get::<_, String>(0)?,
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
            let Ok(row) = serde_json::from_str::<CatalogRow>(&metadata) else {
                return Ok(None);
            };
            if row.harness != root || row.lifecycle.as_str() != lifecycle {
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

//! Producer-materialized history context. Current catalog/native authority stays
//! independent. Missing source paths retain their original generation recipes;
//! rewrites at the same path are not silently treated as that original source.
use crate::{model::summary::CatalogRow, slot_projection::VERSION};
use rusqlite::{Connection, OptionalExtension};

#[cfg(test)]
pub(crate) const SCHEMA: &str = crate::storage::sqlite::history::SCHEMA;

/// Selected history metadata of the bound namespace for admitted readers.
pub(crate) fn catalog(connection: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    match connection.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM main.session_history_catalog WHERE namespace=semon_namespace() AND session_key=?1", [key], |row| row.get::<_, Option<String>>(0)).optional()? {
        Some(None) => Err(rusqlite::Error::InvalidQuery),
        Some(Some(metadata)) => Ok(Some(metadata)),
        None => Ok(None),
    }
}

/// An explicit older harness observation must carry its own complete recipe
/// header and consumed-source cohort, independently of the Current owner.
pub(crate) fn qualified(connection: &Connection, row: &CatalogRow) -> rusqlite::Result<bool> {
    if row.sources.is_empty() || row.sources.len() > 64 {
        return Ok(false);
    }
    // An admitted reader qualifies only its own namespace's recipe header.
    let header: Option<(u32, String, bool)> = connection
        .query_row(
            "SELECT version,source_generation,incomplete FROM main.session_history_projections WHERE namespace=semon_namespace() AND session_key=?1",
            [&row.key],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .optional()?;
    let source_generation = crate::retention::source_generation(&row.sources).map_err(data)?;
    Ok(header.is_some_and(|(version, recorded, incomplete)| {
        qualified_header(&source_generation, version, &recorded, incomplete)
    }))
}

/// Shared History header check for SQLite and PostgreSQL snapshots.
pub(crate) fn qualified_header(
    expected_source_generation: &str,
    version: u32,
    recorded_source_generation: &str,
    incomplete: bool,
) -> bool {
    version == VERSION
        && recorded_source_generation == expected_source_generation
        && !incomplete
}

fn data(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> crate::model::summary::CatalogSource {
        crate::model::summary::CatalogSource {
            path: "projects/session.jsonl".into(),
            native_id: "session".into(),
            dev: 1,
            ino: 2,
            size: 3,
            modified_ns: 4,
            changed_ns: Some(5),
            offset: 3,
            prefix_sha256: [6; 32],
            tail_sha256: [7; 32],
            native_role: None,
        }
    }

    #[test]
    fn retained_header_requires_exact_version_generation_and_completeness() {
        let sources = [source()];
        let generation = crate::retention::source_generation(&sources).unwrap();
        assert!(qualified_header(&generation, VERSION, &generation, false));
        assert!(!qualified_header(&generation, VERSION + 1, &generation, false));
        assert!(!qualified_header(&generation, VERSION, "mismatched", false));
        assert!(!qualified_header(&generation, VERSION, &generation, true));
    }
}

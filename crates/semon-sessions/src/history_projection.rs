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
    let sources = crate::retention::source_generation(&row.sources).map_err(data)?;
    Ok(header.is_some_and(|(version, recorded, incomplete)| {
        version == VERSION && recorded == sources && !incomplete
    }))
}

fn data(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

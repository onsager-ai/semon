//! Producer-materialized history context. Current catalog/native authority stays
//! independent. Missing source paths retain their original generation recipes;
//! rewrites at the same path are not silently treated as that original source.
use crate::{
    model::summary::CatalogRow,
    slot_projection::{Publication, SavedSlot, VERSION},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_history_catalog (
 session_key TEXT PRIMARY KEY, metadata TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS session_history_projections (
 session_key TEXT PRIMARY KEY, generation TEXT NOT NULL, source_generation TEXT NOT NULL,
 version INTEGER NOT NULL, total INTEGER NOT NULL, incomplete INTEGER NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS session_history_slots (
 session_key TEXT NOT NULL, slot INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(session_key,slot)
) STRICT, WITHOUT ROWID;
";

/// Optional history metadata is a primary-key read; old stores continue to
/// expose their existing qualified projection until a producer migrates them.
pub(crate) fn catalog(connection: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    if !connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='session_history_catalog')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Ok(None);
    }
    match connection.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_history_catalog WHERE session_key=?1", [key], |row| row.get::<_, Option<String>>(0)).optional()? {
        Some(None) => Err(rusqlite::Error::InvalidQuery),
        Some(Some(metadata)) => Ok(Some(metadata)),
        None => Ok(None),
    }
}

/// Must run inside the same source/membership/revision-CAS transaction, before
/// current rows or recipes are overwritten. No source body is copied or read.
pub(crate) fn publish(tx: &Transaction<'_>, publication: &Publication<'_>) -> rusqlite::Result<()> {
    let Some(projections) = publication.transcripts else {
        return Ok(());
    };
    let rows: BTreeMap<_, _> = publication
        .rows
        .iter()
        .map(|row| (row.key.as_str(), row))
        .collect();
    for projection in projections {
        let Some(current) = rows.get(projection.key.as_str()).copied() else {
            continue;
        };
        let old_text = catalog(tx, &current.key)?.or(tx
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1",
                [&current.key],
                |row| row.get::<_, String>(0),
            )
            .optional()?);
        let old = old_text
            .as_deref()
            .and_then(|text| serde_json::from_str::<CatalogRow>(text).ok());
        let mut history = current.clone();
        let paths: BTreeSet<_> = current.sources.iter().map(|source| &source.path).collect();
        let missing: BTreeMap<_, _> = old
            .as_ref()
            .into_iter()
            .flat_map(|row| &row.sources)
            .filter(|source| !paths.contains(&source.path))
            .map(|source| (source.path.clone(), source.clone()))
            .collect();
        if missing.is_empty() {
            // The normal case reuses the existing current projection rather
            // than duplicating every transcript in a second persistent table.
            tx.execute(
                "DELETE FROM session_history_slots WHERE session_key=?1",
                [&current.key],
            )?;
            tx.execute(
                "DELETE FROM session_history_projections WHERE session_key=?1",
                [&current.key],
            )?;
            tx.execute(
                "DELETE FROM session_history_catalog WHERE session_key=?1",
                [&current.key],
            )?;
            continue;
        }
        history.sources.extend(missing.values().cloned());
        history.native_ids = history
            .sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for source in missing.values() {
            tx.execute("INSERT INTO session_catalog_sources(session_key,lifecycle,harness,native_id,source_path) VALUES(?1,'retained',?2,?3,?4) ON CONFLICT(session_key,source_path) DO UPDATE SET lifecycle='retained'",params![current.key,current.harness,source.native_id,source.path.to_string_lossy()])?;
        }

        let mut slots = projection.slots.clone();
        let mut incomplete = false;
        if !missing.is_empty() {
            let history_header: Option<(u32, Option<String>, bool)> = tx.query_row("SELECT version,source_generation,incomplete FROM session_history_projections WHERE session_key=?1", [&current.key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let (table, header) = if history_header.is_some() {
                ("session_history_slots", history_header)
            } else {
                ("session_slots", tx.query_row("SELECT version,source_generation,0 FROM session_slot_projections WHERE session_key=?1", [&current.key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?)
            };
            let qualified =
                old.as_ref()
                    .zip(header.as_ref())
                    .is_some_and(|(old, (version, hash, _))| {
                        *version == VERSION
                            && hash.is_some()
                            && crate::retention::source_generation(&old.sources)
                                .ok()
                                .as_ref()
                                == hash.as_ref()
                    });
            if qualified {
                incomplete = header.is_some_and(|(_, _, incomplete)| incomplete);
                let old_sources: BTreeMap<_, _> = old
                    .as_ref()
                    .unwrap()
                    .sources
                    .iter()
                    .map(|source| (&source.path, source))
                    .collect();
                let current_sources: BTreeMap<_, _> = current
                    .sources
                    .iter()
                    .map(|source| (&source.path, source))
                    .collect();
                let mut statement = tx.prepare(&format!(
                    "SELECT metadata FROM {table} WHERE session_key=?1 ORDER BY slot"
                ))?;
                for text in statement.query_map([&current.key], |row| row.get::<_, String>(0))? {
                    let slot: SavedSlot = match decode(&text?) {
                        Ok(slot) => slot,
                        Err(_) => {
                            incomplete = true;
                            continue;
                        }
                    };
                    let Some(file) = &slot.file else {
                        continue;
                    };
                    if !missing.contains_key(&file.path) {
                        if slot
                            .source_paths()
                            .iter()
                            .any(|path| missing.contains_key(*path))
                        {
                            incomplete = true;
                        }
                        continue;
                    }
                    // Every foreign-file dependency must retain the same original
                    // consumed generation. Changed foreign paths need a future
                    // multi-generation recipe, never an implicit current rebind.
                    if slot.source_paths().iter().all(|path| {
                        missing.contains_key(*path)
                            || old_sources.get(*path) == current_sources.get(*path)
                    }) {
                        let at = slot
                            .t
                            .and_then(|time| {
                                slots
                                    .iter()
                                    .position(|current| current.t.is_some_and(|t| t > time))
                            })
                            .unwrap_or(slots.len());
                        slots.insert(at, slot);
                    } else {
                        incomplete = true;
                    }
                }
            } else {
                incomplete = true;
            }
        }
        // Native source/event/block/kind identity, not text equality. Existing
        // current slots win. Stable timestamp ordering preserves old records
        // while allowing current continuation to advance after a disappearance.
        let mut seen = BTreeSet::new();
        slots.retain(|slot| seen.insert(slot.identity()));
        let texts: Vec<String> = slots.iter().map(encode).collect::<Result<_, _>>()?;
        let sources = crate::retention::source_generation(&history.sources).map_err(data)?;
        let metadata = encode(&history)?;
        let generation = format!(
            "{:x}",
            Sha256::digest(encode(&(VERSION, &sources, &texts, incomplete))?.as_bytes())
        );
        let changed=tx.execute("INSERT INTO session_history_projections(session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,version=excluded.version,total=excluded.total,incomplete=excluded.incomplete WHERE session_history_projections.generation != excluded.generation",params![current.key,generation,sources,VERSION,texts.len() as i64,incomplete])?;
        tx.execute("INSERT INTO session_history_catalog(session_key,metadata) VALUES(?1,?2) ON CONFLICT(session_key) DO UPDATE SET metadata=excluded.metadata WHERE session_history_catalog.metadata != excluded.metadata",params![current.key,metadata])?;
        if changed > 0 {
            tx.execute(
                "DELETE FROM session_history_slots WHERE session_key=?1",
                [&current.key],
            )?;
            let mut put = tx.prepare(
                "INSERT INTO session_history_slots(session_key,slot,metadata) VALUES(?1,?2,?3)",
            )?;
            for (index, text) in texts.iter().enumerate() {
                put.execute(params![current.key, index as i64, text])?;
            }
        }
    }
    Ok(())
}
fn data(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}
fn encode(value: &impl serde::Serialize) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(data)
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(data)
}

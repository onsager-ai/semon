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
CREATE INDEX IF NOT EXISTS session_history_query_last ON session_history_catalog(json_extract(metadata,'$.query.last'),session_key);
CREATE TABLE IF NOT EXISTS session_history_projections (
 session_key TEXT PRIMARY KEY, generation TEXT NOT NULL, source_generation TEXT NOT NULL,
 version INTEGER NOT NULL, total INTEGER NOT NULL, incomplete INTEGER NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS session_history_slots (
 session_key TEXT NOT NULL, slot INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(session_key,slot)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_history_slots_turn ON session_history_slots(session_key,json_extract(metadata,'$.turn'),slot);
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

/// An explicit older harness observation must carry its own complete recipe
/// header and consumed-source cohort, independently of the Current owner.
pub(crate) fn qualified(connection: &Connection, row: &CatalogRow) -> rusqlite::Result<bool> {
    if row.sources.is_empty() || row.sources.len() > 64 {
        return Ok(false);
    }
    let header: Option<(u32, String, bool)> = connection
        .query_row(
            "SELECT version,source_generation,incomplete FROM session_history_projections WHERE session_key=?1",
            [&row.key],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .optional()?;
    let sources = crate::retention::source_generation(&row.sources).map_err(data)?;
    Ok(header.is_some_and(|(version, recorded, incomplete)| {
        version == VERSION && recorded == sources && !incomplete
    }))
}

fn preserve_original_harness(tx: &Transaction<'_>, old: &CatalogRow) -> rusqlite::Result<()> {
    // Existing History is original custody, including unsupported versions or
    // incomplete recipes. A Current replacement cannot overwrite that state.
    if catalog(tx, &old.key)?.is_some() {
        return Ok(());
    }
    let mut history = old.clone();
    history.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
    let header: Option<(String, u32, Option<String>, i64)> = tx.query_row(
        "SELECT generation,version,source_generation,total FROM session_slot_projections WHERE session_key=?1",
        [&old.key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?;
    let metadata = encode(&history)?;
    let original_header = encode(&header)?;
    let mut bytes = metadata.len().saturating_add(original_header.len());
    // Reuse the semantic metadata page's 64-row/1MiB budget for this
    // selected-owner custody transfer. Refuse oversized original state before
    // any Current overwrite; never truncate it or certify a partial copy.
    if bytes > 1048576 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut saved = Vec::new();
    let mut recipes = tx.prepare("SELECT slot,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM session_slots WHERE session_key=?1 ORDER BY slot LIMIT 65")?;
    for recipe in recipes.query_map([&old.key], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
    })? {
        let (slot, recipe) = recipe?;
        let recipe = recipe.ok_or(rusqlite::Error::InvalidQuery)?;
        bytes = bytes.saturating_add(recipe.len());
        if saved.len() == 64 || bytes > 1048576 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        saved.push((slot, recipe));
    }
    drop(recipes);
    let mut complete = header.as_ref().is_some_and(|(_, version, recorded, _)| {
        *version == VERSION
            && !old.sources.is_empty()
            && old.sources.len() <= 64
            && recorded.is_some()
            && crate::retention::source_generation(&old.sources)
                .ok()
                .as_ref()
                == recorded.as_ref()
    });
    complete &= header.as_ref().is_some_and(|(_, _, _, total)| {
        *total >= 0 && usize::try_from(*total).ok() == Some(saved.len())
    });
    if complete {
        let paths: BTreeSet<_> = old.sources.iter().map(|source| &source.path).collect();
        for (_, recipe) in &saved {
            let qualified = decode::<SavedSlot>(recipe)
                .ok()
                .is_some_and(|slot| slot.source_paths().iter().all(|path| paths.contains(*path)));
            if !qualified {
                complete = false;
                break;
            }
        }
    }
    tx.execute(
        "INSERT INTO session_history_catalog(session_key,metadata) VALUES(?1,?2)",
        params![old.key, metadata],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
        params![
            format!("history_original_header:{}", old.key),
            original_header
        ],
    )?;
    if let Some((generation, version, sources, total)) = header {
        // History's hash column is non-nullable. Preserve the exact original
        // nullable header separately; an absent hash remains explicitly
        // incomplete and can never qualify as a fresh V5 observation.
        tx.execute("INSERT INTO session_history_projections(session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6)",params![old.key,generation,sources.unwrap_or_default(),version,total,!complete])?;
    }
    let mut put = tx
        .prepare("INSERT INTO session_history_slots(session_key,slot,metadata) VALUES(?1,?2,?3)")?;
    for (slot, recipe) in saved {
        put.execute(params![old.key, slot, recipe])?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,'1')",
        [format!("overview_needed:{}", old.key)],
    )?;
    Ok(())
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
        let separate_harness = old
            .as_ref()
            .is_some_and(|old| old.harness != current.harness);
        if separate_harness {
            let old = old.as_ref().unwrap();
            for source in &old.sources {
                tx.execute("INSERT INTO session_catalog_sources(session_key,lifecycle,harness,native_id,source_path) VALUES(?1,'retained',?2,?3,?4) ON CONFLICT(session_key,source_path) DO UPDATE SET lifecycle='retained'",params![old.key,old.harness,source.native_id,source.path.to_string_lossy()])?;
            }
            preserve_original_harness(tx, old)?;
            continue;
        }
        // A reused viewer key does not join two native harness identities.
        // Retain the original owner and recipes independently of Current.
        let mut history = current.clone();
        let paths: BTreeSet<_> = current.sources.iter().map(|source| &source.path).collect();
        let mut missing = BTreeMap::new();
        for source in old.as_ref().into_iter().flat_map(|row| &row.sources) {
            if !paths.contains(&source.path)
                && !crate::events::store::relocated_catalog_source(tx, source, &current.sources)?
            {
                missing.insert(source.path.clone(), source.clone());
            }
        }
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
        // A composite recipe is custody, not an exact aggregate of its native
        // records. The existing selected producer must qualify full Query facts.
        history.query = None;
        history.sources.extend(missing.values().cloned());
        history.native_ids = history
            .sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for source in missing.values() {
            tx.execute("INSERT INTO session_catalog_sources(session_key,lifecycle,harness,native_id,source_path) VALUES(?1,'retained',?2,?3,?4) ON CONFLICT(session_key,source_path) DO UPDATE SET lifecycle='retained'",params![current.key,old.as_ref().unwrap().harness,source.native_id,source.path.to_string_lossy()])?;
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
                "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,'1')",
                [format!("overview_needed:{}", current.key)],
            )?;
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

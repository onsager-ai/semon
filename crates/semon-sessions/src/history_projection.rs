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
CREATE TABLE IF NOT EXISTS session_history_slot_parts (
 session_key TEXT NOT NULL, slot INTEGER NOT NULL, part INTEGER NOT NULL, bytes BLOB NOT NULL,
 PRIMARY KEY(session_key,slot,part)
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

// Transfer checkpoints are private producer work. Neither a History catalog row
// nor a projection header is visible until every original byte has been copied.
// Current remains the custody owner throughout an interrupted transfer.
#[derive(serde::Serialize, serde::Deserialize)]
struct Transfer {
    witness: String,
    last: Option<i64>,
    copied: usize,
    partial: i64,
    complete: bool,
}

fn preserve_original_harness(tx: &Transaction<'_>, old: &CatalogRow) -> rusqlite::Result<bool> {
    if catalog(tx, &old.key)?.is_some() {
        return Ok(true);
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
    let witness = format!(
        "{:x}",
        Sha256::digest(encode(&(&metadata, &original_header))?.as_bytes())
    );
    let marker = format!("history_transfer:{}", old.key);
    let saved: Option<String> = tx
        .query_row("SELECT value FROM meta WHERE key=?1", [&marker], |row| {
            row.get(0)
        })
        .optional()?;
    let prior = saved.as_deref().map(decode::<Transfer>).transpose()?;
    if prior.as_ref().is_some_and(|prior| prior.witness != witness) {
        // Another fenced publication changed the original. Discard only private
        // staged rows, in bounded batches; never combine generations.
        let removed = tx.execute("DELETE FROM session_history_slots WHERE session_key=?1 AND slot IN (SELECT slot FROM session_history_slots WHERE session_key=?1 ORDER BY slot LIMIT 64)", [&old.key])?;
        let parts = tx.execute("DELETE FROM session_history_slot_parts WHERE session_key=?1 AND (slot,part) IN (SELECT slot,part FROM session_history_slot_parts WHERE session_key=?1 ORDER BY slot,part LIMIT 64)", [&old.key])?;
        if removed == 0 && parts == 0 {
            tx.execute("DELETE FROM meta WHERE key=?1", [&marker])?;
        }
        return Ok(false);
    }
    let mut transfer = prior.unwrap_or_else(|| Transfer {
        witness,
        last: None,
        copied: 0,
        partial: 0,
        complete: header
            .as_ref()
            .is_some_and(|(_, version, recorded, total)| {
                *version == VERSION
                    && *total >= 0
                    && !old.sources.is_empty()
                    && old.sources.len() <= 64
                    && recorded.is_some()
                    && crate::retention::source_generation(&old.sources)
                        .ok()
                        .as_ref()
                        == recorded.as_ref()
            }),
    });
    let paths: BTreeSet<_> = old.sources.iter().map(|source| &source.path).collect();
    let mut bytes = 0usize;
    let mut processed = 0;
    while processed < 64 && bytes < 1048576 {
        let comparison = if transfer.last.is_some() { ">" } else { ">=" };
        let next: Option<(i64, i64)> = tx.query_row(
            &format!("SELECT slot,octet_length(metadata) FROM session_slots WHERE session_key=?1 AND slot{comparison}?2 ORDER BY slot LIMIT 1"),
            params![old.key, transfer.last.unwrap_or(i64::MIN)], |row|Ok((row.get(0)?,row.get(1)?)),
        ).optional()?;
        let Some((slot, length)) = next else { break };
        let offset = transfer.partial;
        let take = length.saturating_sub(offset).min((1048576 - bytes) as i64);
        if length > 1048576 {
            // Unsupported oversized values stay in ordered byte parts. No
            // concatenation or whole-value decode can grow with their length.
            // Their original header stays incomplete; ordinary readers refuse.
            tx.execute("INSERT INTO session_history_slot_parts(session_key,slot,part,bytes) SELECT session_key,slot,?3,substr(CAST(metadata AS BLOB),?3+1,?4) FROM session_slots WHERE session_key=?1 AND slot=?2", params![old.key,slot,offset,take])?;
            transfer.complete = false;
        } else {
            if length as usize > 1048576 - bytes {
                break;
            }
            tx.execute("INSERT INTO session_history_slots(session_key,slot,metadata) SELECT session_key,slot,metadata FROM session_slots WHERE session_key=?1 AND slot=?2", params![old.key,slot])?;
        }
        bytes += take as usize;
        processed += 1;
        if offset + take != length {
            transfer.partial += take;
            break;
        }
        transfer.partial = 0;
        transfer.complete &= usize::try_from(slot).ok() == Some(transfer.copied);
        if transfer.complete {
            if length > 1048576 {
                transfer.complete = false;
            } else {
                let text: String = tx.query_row(
                    "SELECT metadata FROM session_history_slots WHERE session_key=?1 AND slot=?2",
                    params![old.key, slot],
                    |row| row.get(0),
                )?;
                transfer.complete &= decode::<SavedSlot>(&text).ok().is_some_and(|slot| {
                    slot.source_paths().iter().all(|path| paths.contains(*path))
                });
            }
        }
        transfer.last = Some(slot);
        transfer.copied += 1;
    }
    let comparison = if transfer.last.is_some() { ">" } else { ">=" };
    let more: bool = tx.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM session_slots WHERE session_key=?1 AND slot{comparison}?2)"
        ),
        params![old.key, transfer.last.unwrap_or(i64::MIN)],
        |row| row.get(0),
    )?;
    if more {
        tx.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
            params![marker, encode(&transfer)?],
        )?;
        return Ok(false);
    }
    transfer.complete &= header
        .as_ref()
        .is_some_and(|(_, _, _, total)| usize::try_from(*total).ok() == Some(transfer.copied));
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
        tx.execute("INSERT INTO session_history_projections(session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6)",params![old.key,generation,sources.unwrap_or_default(),version,total,!transfer.complete])?;
    }
    tx.execute("DELETE FROM meta WHERE key=?1", [marker])?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,'1')",
        [format!("overview_needed:{}", old.key)],
    )?;
    Ok(true)
}

/// One bounded custody step per replacement owner, after publication/source CAS
/// checks and before any Current mutation. False must commit only this private
/// progress and leave the caller's durable publication intent pending.
pub(crate) fn preserve_replacements(
    tx: &Transaction<'_>,
    publication: &Publication<'_>,
) -> rusqlite::Result<bool> {
    let mut advanced = false;
    for row in publication.rows {
        let retained: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_history_catalog WHERE session_key=?1)",
            [&row.key],
            |record| record.get(0),
        )?;
        if retained {
            continue;
        }
        let old: Option<String> = tx
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key=?1 AND harness!=?2",
                params![row.key, row.harness],
                |record| record.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            if advanced {
                return Ok(false);
            }
            let old: CatalogRow = decode(&old)?;
            if !preserve_original_harness(tx, &old)? {
                return Ok(false);
            }
            advanced = true;
        }
    }
    Ok(true)
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
        // A completed cross-harness original is immutable custody even if a
        // later Current publication reuses its original harness again.
        let original: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_history_catalog h JOIN meta m ON m.key=?2 WHERE h.session_key=?1)",params![current.key,format!("history_original_header:{}",current.key)],|row|row.get(0))?;
        if original {
            continue;
        }
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
            // The owning publication transaction completed custody preflight.
            debug_assert!(catalog(tx, &old.key)?.is_some());
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
            tx.execute(
                "DELETE FROM meta WHERE key=?1",
                [format!("history_transfer:{}", current.key)],
            )?;
            tx.execute(
                "DELETE FROM session_history_slot_parts WHERE session_key=?1",
                [&current.key],
            )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(recipes: usize, padding: usize) -> (Connection, CatalogRow, Vec<(i64, String)>) {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE session_slot_projections(session_key TEXT PRIMARY KEY,generation TEXT,version INTEGER,source_generation TEXT,total INTEGER); CREATE TABLE session_slots(session_key TEXT,slot INTEGER,metadata TEXT,PRIMARY KEY(session_key,slot)) WITHOUT ROWID;").unwrap();
        let old: CatalogRow = serde_json::from_value(serde_json::json!({"key":"old","name":"original","harness":"codex","kind":"session","native_ids":["old"],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":"","effort":null,"start":null,"last":null,"tokens":[0,0,0],"cost":serde_json::to_value(crate::pricing::Cost::default()).unwrap(),"sources":[]})).unwrap();
        db.execute(
            "INSERT INTO session_slot_projections VALUES('old','original-generation',4,NULL,?1)",
            [recipes as i64],
        )
        .unwrap();
        let mut saved = Vec::new();
        for n in 0..recipes {
            // Unsupported originals are still custody, including arbitrary UTF-8.
            let text = serde_json::json!({"original":n,"padding":"水".repeat(padding)}).to_string();
            db.execute(
                "INSERT INTO session_slots VALUES('old',?1,?2)",
                params![n as i64, text],
            )
            .unwrap();
            saved.push((n as i64, text));
        }
        (db, old, saved)
    }
    fn step(db: &mut Connection, old: &CatalogRow) -> bool {
        let tx = db.transaction().unwrap();
        let done = preserve_original_harness(&tx, old).unwrap();
        tx.commit().unwrap();
        done
    }
    fn slots(db: &Connection) -> Vec<(i64, String)> {
        db.prepare("SELECT slot,metadata FROM session_history_slots UNION ALL SELECT slot,CAST(group_concat(bytes,'') AS TEXT) FROM (SELECT slot,bytes FROM session_history_slot_parts ORDER BY slot,part) GROUP BY slot ORDER BY slot").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }
    #[test]
    fn custody_pages_resume_without_exposing_partial_history() {
        for count in [63, 64, 65, 1025] {
            let (mut db, old, original) = fixture(count, 0);
            let mut steps = 0;
            loop {
                let before = slots(&db).len();
                let done = step(&mut db, &old);
                steps += 1;
                assert!(slots(&db).len() - before <= 64);
                if done {
                    break;
                }
                assert!(catalog(&db, &old.key).unwrap().is_none());
                assert!(!qualified(&db, &old).unwrap());
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM session_history_projections",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    0
                );
                // A new connection/process needs only this durable checkpoint.
                assert!(db.is_autocommit());
                assert!(steps <= count.div_ceil(64));
            }
            assert_eq!(steps, count.div_ceil(64));
            assert_eq!(slots(&db), original);
            assert!(step(&mut db, &old));
            assert_eq!(slots(&db), original);
            let incomplete: bool = db
                .query_row(
                    "SELECT incomplete FROM session_history_projections",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(incomplete, "unsupported originals gained authority");
            assert_eq!(
                db.query_row(
                    "SELECT value FROM meta WHERE key='history_original_header:old'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                encode(&Some((
                    "original-generation",
                    4,
                    Option::<String>::None,
                    count
                )))
                .unwrap()
            );
        }
    }
    #[test]
    fn custody_byte_pages_preserve_utf8_and_oversized_unsupported_recipes() {
        for padding in [349510, 349520, 800000] {
            let (mut db, old, original) = fixture(3, padding);
            let mut done = false;
            for _ in 0..16 {
                let before:i64=db.query_row("SELECT coalesce((SELECT sum(octet_length(metadata)) FROM session_history_slots),0)+coalesce((SELECT sum(length(bytes)) FROM session_history_slot_parts),0)",[],|r|r.get(0)).unwrap();
                done = step(&mut db, &old);
                let after:i64=db.query_row("SELECT coalesce((SELECT sum(octet_length(metadata)) FROM session_history_slots),0)+coalesce((SELECT sum(length(bytes)) FROM session_history_slot_parts),0)",[],|r|r.get(0)).unwrap();
                assert!(after - before <= 1048576);
                if done {
                    break;
                }
                assert!(catalog(&db, &old.key).unwrap().is_none());
            }
            assert!(done);
            assert_eq!(slots(&db), original);
        }
    }
    #[test]
    fn completed_original_is_immutable_when_current_reuses_its_harness() {
        let (mut db, mut current, original) = fixture(65, 0);
        while !step(&mut db, &current) {}
        let metadata = catalog(&db, &current.key).unwrap();
        let header: String = db
            .query_row(
                "SELECT value FROM meta WHERE key='history_original_header:old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        current.name = "New Current with original harness".into();
        let tx = db.transaction().unwrap();
        publish(
            &tx,
            &Publication {
                rows: &[current.clone()],
                transcripts: Some(&[crate::slot_projection::Projection {
                    key: current.key.clone(),
                    slots: Vec::new(),
                    repair: false,
                }]),
            },
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(catalog(&db, &current.key).unwrap(), metadata);
        assert_eq!(slots(&db), original);
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='history_original_header:old'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            header
        );
        assert!(!qualified(&db, &current).unwrap());
    }
    #[test]
    fn custody_preserves_unsupported_original_slot_addresses() {
        let (mut db, old, mut original) = fixture(65, 0);
        db.execute("UPDATE session_slots SET slot=?1 WHERE slot=0", [i64::MIN])
            .unwrap();
        original[0].0 = i64::MIN;
        assert!(!step(&mut db, &old));
        assert!(step(&mut db, &old));
        assert_eq!(slots(&db), original);
    }
    #[test]
    fn custody_rollback_and_changed_original_restart_cleanly() {
        let (mut db, mut old, original) = fixture(130, 0);
        {
            let tx = db.transaction().unwrap();
            assert!(!preserve_original_harness(&tx, &old).unwrap());
            // Cancellation/failure before commit rolls back rows and checkpoint.
        }
        assert!(slots(&db).is_empty());
        assert!(!step(&mut db, &old));
        old.name = "changed original generation".into();
        db.execute(
            "UPDATE session_slot_projections SET generation='replacement-generation'",
            [],
        )
        .unwrap();
        assert!(!step(&mut db, &old));
        assert!(slots(&db).is_empty());
        assert!(!step(&mut db, &old)); // retire stale checkpoint
        assert!(!step(&mut db, &old));
        assert!(!step(&mut db, &old));
        assert!(step(&mut db, &old));
        assert_eq!(slots(&db), original);
        let saved: CatalogRow = decode(&catalog(&db, &old.key).unwrap().unwrap()).unwrap();
        assert_eq!(saved.name, old.name);
        assert_eq!(
            db.query_row(
                "SELECT generation FROM session_history_projections",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "replacement-generation"
        );
    }
}

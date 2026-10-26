//! Producer-materialized history context. Current catalog/native authority stays
//! independent. Missing source paths retain their original generation recipes;
//! rewrites at the same path are not silently treated as that original source.
use crate::{
    model::summary::CatalogRow,
    slot_projection::{Publication, VERSION},
};
#[cfg(test)]
use rusqlite::params;
use rusqlite::{Connection, OptionalExtension, Transaction};

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

/// Optional selected history metadata in the current storage contract.
pub(crate) fn catalog(connection: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    match connection.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_history_catalog WHERE session_key=?1", [key], |row| row.get::<_, Option<String>>(0)).optional()? {
        Some(None) => Err(rusqlite::Error::InvalidQuery),
        Some(Some(metadata)) => Ok(Some(metadata)),
        None => Ok(None),
    }
}

/// Selected history metadata of the bound namespace for admitted readers.
pub(crate) fn admitted_catalog(
    connection: &Connection,
    key: &str,
) -> rusqlite::Result<Option<String>> {
    match connection.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM main.session_history_catalog WHERE namespace=semon_namespace() AND session_key=?1", [key], |row| row.get::<_, Option<String>>(0)).optional()? {
        Some(None) => Err(rusqlite::Error::InvalidQuery),
        Some(Some(metadata)) => Ok(Some(metadata)),
        None => Ok(None),
    }
}

/// An explicit older harness observation must carry its own complete recipe
/// header and consumed-source cohort, independently of the Current owner.
pub(crate) fn qualified(
    connection: &Connection,
    row: &CatalogRow,
    admitted: bool,
) -> rusqlite::Result<bool> {
    if row.sources.is_empty() || row.sources.len() > 64 {
        return Ok(false);
    }
    // An admitted reader qualifies only its own namespace's recipe header.
    let scope = if admitted {
        "namespace=semon_namespace() AND "
    } else {
        ""
    };
    let header: Option<(u32, String, bool)> = connection
        .query_row(
            &format!("SELECT version,source_generation,incomplete FROM main.session_history_projections WHERE {scope}session_key=?1"),
            [&row.key],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .optional()?;
    let sources = crate::retention::source_generation(&row.sources).map_err(data)?;
    Ok(header.is_some_and(|(version, recorded, incomplete)| {
        version == VERSION && recorded == sources && !incomplete
    }))
}

// Original custody uses the same typed bounded recipe as the admitted publisher.
#[cfg(test)]
fn preserve_original_harness(tx: &Transaction<'_>, old: &CatalogRow) -> rusqlite::Result<bool> {
    crate::storage::catalog::preserve_original(
        &crate::storage::sqlite::history::OriginalHistory::unbound(tx),
        old,
    )
}

/// Must run inside the same source/membership/revision-CAS transaction, before
/// current rows or recipes are overwritten. No source body is copied or read.
pub(crate) fn publish(tx: &Transaction<'_>, publication: &Publication<'_>) -> rusqlite::Result<()> {
    crate::storage::catalog::publish_history_rows(
        &crate::storage::sqlite::history::OriginalHistory::unbound(tx),
        publication.rows,
        publication.transcripts,
    )
}

fn data(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}
#[cfg(test)]
fn encode(value: &impl serde::Serialize) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(data)
}
#[cfg(test)]
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
                assert!(!qualified(&db, &old, false).unwrap());
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
        assert!(!qualified(&db, &current, false).unwrap());
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

// Frozen pre-extraction custody implementation, CI oracle only.

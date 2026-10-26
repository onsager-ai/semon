//! Authored physical operations for the shared bounded original-custody recipe.
use crate::model::summary::CatalogRow;
use crate::storage::catalog::{OriginalHeader, OriginalHistoryStore};
use rusqlite::{Connection, OptionalExtension, params};

/// Authored fresh private semantic profile; never converts singleton custody.
pub(crate) const SCHEMA: &str = "
CREATE TABLE session_history_catalog (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(namespace,session_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX session_history_query_last ON session_history_catalog(namespace,json_extract(metadata,'$.query.last'),session_key);
CREATE TABLE session_history_projections (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, generation TEXT NOT NULL,
 source_generation TEXT NOT NULL, version INTEGER NOT NULL, total INTEGER NOT NULL,
 incomplete INTEGER NOT NULL, PRIMARY KEY(namespace,session_key)
) STRICT, WITHOUT ROWID;
CREATE TABLE session_history_slots (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, slot INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(namespace,session_key,slot)
) STRICT, WITHOUT ROWID;
CREATE TABLE session_history_slot_parts (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, slot INTEGER NOT NULL,
 part INTEGER NOT NULL, bytes BLOB NOT NULL,
 PRIMARY KEY(namespace,session_key,slot,part)
) STRICT, WITHOUT ROWID;
CREATE INDEX session_history_slots_turn ON session_history_slots(namespace,session_key,json_extract(metadata,'$.turn'),slot);
";

pub(crate) struct OriginalHistory<'a> {
    db: &'a Connection,
    namespace: &'a str,
}

impl<'a> OriginalHistory<'a> {
    pub(crate) fn admitted(
        db: &'a Connection,
        source: &'a super::semantic::AdmittedSource,
    ) -> Result<Self, crate::storage::StorageError> {
        source.validate_publication(db)?;
        Ok(Self {
            db,
            namespace: source.namespace(),
        })
    }
}

impl OriginalHistoryStore for OriginalHistory<'_> {
    type Error = rusqlite::Error;
    fn data(error: serde_json::Error) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }
    fn retained(&self, key: &str) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2)", params![self.namespace,key], |r|r.get(0))
    }
    fn replacement(&self, row: &CatalogRow) -> rusqlite::Result<Option<CatalogRow>> {
        let text: Option<String> = self.db.query_row("SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2 AND metadata<>?3",params![self.namespace,row.key,serde_json::to_string(row).map_err(<Self as OriginalHistoryStore>::data)?],|r|r.get(0)).optional()?;
        text.as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(<Self as OriginalHistoryStore>::data)
    }
    fn header(&self, key: &str) -> rusqlite::Result<Option<OriginalHeader>> {
        self.db.query_row("SELECT generation,version,source_generation,total FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2", params![self.namespace,key], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()
    }
    fn transfer(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key=?2",
                params![self.namespace, format!("history_transfer:{key}")],
                |r| r.get(0),
            )
            .optional()
    }
    fn reset_transfer(&self, key: &str) -> rusqlite::Result<()> {
        let removed = self.db.execute("DELETE FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 AND slot IN (SELECT slot FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 ORDER BY slot LIMIT 64)",params![self.namespace,key])?;
        let parts = self.db.execute("DELETE FROM main.session_history_slot_parts WHERE namespace=?1 AND session_key=?2 AND (slot,part) IN (SELECT slot,part FROM main.session_history_slot_parts WHERE namespace=?1 AND session_key=?2 ORDER BY slot,part LIMIT 64)",params![self.namespace,key])?;
        if removed == 0 && parts == 0 {
            self.db.execute(
                "DELETE FROM main.meta WHERE namespace=?1 AND key=?2",
                params![self.namespace, format!("history_transfer:{key}")],
            )?;
        }
        Ok(())
    }
    fn next_slot(&self, key: &str, last: Option<i64>) -> rusqlite::Result<Option<(i64, i64)>> {
        let sql = if last.is_some() {
            "SELECT slot,octet_length(metadata) FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>?3 ORDER BY slot LIMIT 1"
        } else {
            "SELECT slot,octet_length(metadata) FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>=?3 ORDER BY slot LIMIT 1"
        };
        self.db
            .query_row(
                sql,
                params![self.namespace, key, last.unwrap_or(i64::MIN)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }
    fn copy_slot(
        &self,
        key: &str,
        slot: i64,
        offset: i64,
        take: i64,
        split: bool,
        inspect: bool,
    ) -> rusqlite::Result<Option<String>> {
        if split {
            self.db.execute("INSERT INTO main.session_history_slot_parts(namespace,session_key,slot,part,bytes) SELECT namespace,session_key,slot,?4,substr(CAST(metadata AS BLOB),?4+1,?5) FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot=?3",params![self.namespace,key,slot,offset,take])?;
            return Ok(None);
        }
        self.db.execute("INSERT INTO main.session_history_slots(namespace,session_key,slot,metadata) SELECT namespace,session_key,slot,metadata FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot=?3",params![self.namespace,key,slot])?;
        if !inspect {
            return Ok(None);
        }
        self.db.query_row("SELECT metadata FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 AND slot=?3",params![self.namespace,key,slot], |r|r.get(0)).map(Some)
    }
    fn more_slots(&self, key: &str, last: Option<i64>) -> rusqlite::Result<bool> {
        let sql = if last.is_some() {
            "SELECT EXISTS(SELECT 1 FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>?3)"
        } else {
            "SELECT EXISTS(SELECT 1 FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>=?3)"
        };
        self.db.query_row(
            sql,
            params![self.namespace, key, last.unwrap_or(i64::MIN)],
            |r| r.get(0),
        )
    }
    fn checkpoint(&self, key: &str, transfer: &str) -> rusqlite::Result<()> {
        self.db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![self.namespace,format!("history_transfer:{key}"),transfer])?;
        Ok(())
    }
    fn complete(
        &self,
        row: &CatalogRow,
        metadata: &str,
        original_header: &str,
        header: Option<&OriginalHeader>,
        complete: bool,
    ) -> rusqlite::Result<()> {
        let key = &row.key;
        self.db.execute("INSERT INTO main.session_history_catalog(namespace,session_key,metadata) VALUES(?1,?2,?3)",params![self.namespace,key,metadata])?;
        self.db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![self.namespace,format!("history_original_header:{key}"),original_header])?;
        if let Some((generation, version, sources, total)) = header {
            self.db.execute("INSERT INTO main.session_history_projections(namespace,session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![self.namespace,key,generation,sources.as_deref().unwrap_or_default(),version,total,!complete])?;
        }
        self.db.execute(
            "DELETE FROM main.meta WHERE namespace=?1 AND key=?2",
            params![self.namespace, format!("history_transfer:{key}")],
        )?;
        self.db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,'1') ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![self.namespace,format!("overview_needed:{key}")])?;
        Ok(())
    }
}

impl crate::storage::catalog::CompositeHistoryStore for OriginalHistory<'_> {
    type Error = rusqlite::Error;
    fn data(error: serde_json::Error) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }
    fn original(&self, key: &str) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_history_catalog h JOIN main.meta m ON m.namespace=h.namespace AND m.key=?3 WHERE h.namespace=?1 AND h.session_key=?2)",params![self.namespace,key,format!("history_original_header:{key}")],|r|r.get(0))
    }
    fn previous(&self, key: &str) -> rusqlite::Result<Option<String>> {
        let history: Option<Option<String>> = self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",params![self.namespace,key],|r|r.get(0)).optional()?;
        let history = match history {
            Some(None) => return Err(rusqlite::Error::InvalidQuery),
            Some(value) => value,
            None => None,
        };
        // Preserve the original eager Current read and its corruption errors.
        let current = self
            .db
            .query_row(
                "SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
                params![self.namespace, key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(history.or(current))
    }
    fn history_exists(&self, key: &str) -> rusqlite::Result<bool> {
        <Self as OriginalHistoryStore>::retained(self, key)
    }
    fn retain_source(
        &self,
        row: &CatalogRow,
        source: &crate::model::summary::CatalogSource,
    ) -> rusqlite::Result<()> {
        self.db.execute("INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES(?1,?2,'retained',?3,?4,?5) ON CONFLICT(namespace,session_key,source_path) DO UPDATE SET lifecycle='retained'",params![self.namespace,row.key,row.harness,source.native_id,source.path.to_string_lossy()])?;
        Ok(())
    }
    fn relocated(
        &self,
        old: &crate::model::summary::CatalogSource,
        current: &[crate::model::summary::CatalogSource],
    ) -> rusqlite::Result<bool> {
        // The existing relocation witness reader already scopes metadata and
        // ledger reads through the admitted connection's native binding.
        crate::events::store::relocated_catalog_source(self.db, old, current)
    }
    fn clear(&self, key: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM main.meta WHERE namespace=?1 AND key=?2",
            params![self.namespace, format!("history_transfer:{key}")],
        )?;
        for sql in [
            "DELETE FROM main.session_history_slot_parts WHERE namespace=?1 AND session_key=?2",
            "DELETE FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2",
            "DELETE FROM main.session_history_projections WHERE namespace=?1 AND session_key=?2",
            "DELETE FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
        ] {
            self.db.execute(sql, params![self.namespace, key])?;
        }
        Ok(())
    }
    fn previous_recipe(
        &self,
        key: &str,
    ) -> rusqlite::Result<(bool, Option<crate::storage::catalog::HistoryRecipeHeader>)> {
        let header=self.db.query_row("SELECT version,source_generation,incomplete FROM main.session_history_projections WHERE namespace=?1 AND session_key=?2",params![self.namespace,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if header.is_some() {
            return Ok((true, header));
        }
        let header=self.db.query_row("SELECT version,source_generation,0 FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2",params![self.namespace,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        Ok((false, header))
    }
    fn recipe_slots(&self, key: &str, history: bool) -> rusqlite::Result<Vec<String>> {
        let sql = if history {
            "SELECT metadata FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 ORDER BY slot LIMIT 65"
        } else {
            "SELECT metadata FROM main.session_slots WHERE namespace=?1 AND session_key=?2 ORDER BY slot LIMIT 65"
        };
        let texts = self
            .db
            .prepare(sql)?
            .query_map(params![self.namespace, key], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        if texts.len() > 64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(texts)
    }
    fn composed(
        &self,
        key: &str,
        generation: &str,
        sources: &str,
        metadata: &str,
        texts: &[String],
        incomplete: bool,
    ) -> rusqlite::Result<()> {
        let total = i64::try_from(texts.len()).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let changed=self.db.execute("INSERT INTO main.session_history_projections(namespace,session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(namespace,session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,version=excluded.version,total=excluded.total,incomplete=excluded.incomplete WHERE session_history_projections.generation != excluded.generation",params![self.namespace,key,generation,sources,crate::slot_projection::VERSION,total,incomplete])?;
        self.db.execute("INSERT INTO main.session_history_catalog(namespace,session_key,metadata) VALUES(?1,?2,?3) ON CONFLICT(namespace,session_key) DO UPDATE SET metadata=excluded.metadata WHERE session_history_catalog.metadata != excluded.metadata",params![self.namespace,key,metadata])?;
        if changed > 0 {
            self.db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,'1') ON CONFLICT(namespace,key) DO UPDATE SET value='1'",params![self.namespace,format!("overview_needed:{key}")])?;
            self.db.execute(
                "DELETE FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2",
                params![self.namespace, key],
            )?;
            let mut put=self.db.prepare("INSERT INTO main.session_history_slots(namespace,session_key,slot,metadata) VALUES(?1,?2,?3,?4)")?;
            for (slot, text) in texts.iter().enumerate() {
                put.execute(params![
                    self.namespace,
                    key,
                    i64::try_from(slot).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    text
                ])?;
            }
        }
        Ok(())
    }
}

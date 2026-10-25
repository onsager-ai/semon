//! Authored physical operations for the shared bounded original-custody recipe.
use crate::model::summary::CatalogRow;
use crate::storage::catalog::{OriginalHeader, OriginalHistoryStore};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) struct OriginalHistory<'a> {
    db: &'a Connection,
    namespace: Option<&'a str>,
}

impl<'a> OriginalHistory<'a> {
    pub(crate) fn unbound(db: &'a Connection) -> Self {
        Self { db, namespace: None }
    }
    pub(super) fn admitted(db: &'a Connection, source: &'a super::semantic::AdmittedSource) -> Result<Self, crate::storage::StorageError> {
        source.validate_publication(db)?;
        Ok(Self { db, namespace: Some(source.namespace()) })
    }
    fn sql<'s>(&self, unbound: &'s str, bound: &'s str) -> &'s str {
        if self.namespace.is_some() { bound } else { unbound }
    }
}

impl OriginalHistoryStore for OriginalHistory<'_> {
    type Error = rusqlite::Error;
    fn data(error: serde_json::Error) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }
    fn retained(&self, key: &str) -> rusqlite::Result<bool> {
        self.db.query_row(self.sql("SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE session_key=?2)", "SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2)"), params![self.namespace,key], |r|r.get(0))
    }
    fn replacement(&self, row: &CatalogRow) -> rusqlite::Result<Option<CatalogRow>> {
        // The existing unbound policy preserves cross-harness originals. The
        // private scoped primitive policy also preserves differing metadata.
        let text: Option<String> = match self.namespace {
            Some(ns) => self.db.query_row("SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2 AND metadata<>?3",params![ns,row.key,serde_json::to_string(row).map_err(Self::data)?],|r|r.get(0)).optional()?,
            None => self.db.query_row("SELECT metadata FROM main.session_catalog WHERE session_key=?1 AND harness<>?2",params![row.key,row.harness],|r|r.get(0)).optional()?,
        };
        text.as_deref().map(serde_json::from_str).transpose().map_err(Self::data)
    }
    fn header(&self, key: &str) -> rusqlite::Result<Option<OriginalHeader>> {
        self.db.query_row(self.sql("SELECT generation,version,source_generation,total FROM main.session_slot_projections WHERE session_key=?2", "SELECT generation,version,source_generation,total FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2"), params![self.namespace,key], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()
    }
    fn transfer(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.db.query_row(self.sql("SELECT value FROM main.meta WHERE key=?2", "SELECT value FROM main.meta WHERE namespace=?1 AND key=?2"), params![self.namespace,format!("history_transfer:{key}")], |r|r.get(0)).optional()
    }
    fn reset_transfer(&self, key: &str) -> rusqlite::Result<()> {
        let removed = self.db.execute(self.sql("DELETE FROM main.session_history_slots WHERE session_key=?2 AND slot IN (SELECT slot FROM main.session_history_slots WHERE session_key=?2 ORDER BY slot LIMIT 64)", "DELETE FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 AND slot IN (SELECT slot FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 ORDER BY slot LIMIT 64)"),params![self.namespace,key])?;
        let parts = self.db.execute(self.sql("DELETE FROM main.session_history_slot_parts WHERE session_key=?2 AND (slot,part) IN (SELECT slot,part FROM main.session_history_slot_parts WHERE session_key=?2 ORDER BY slot,part LIMIT 64)", "DELETE FROM main.session_history_slot_parts WHERE namespace=?1 AND session_key=?2 AND (slot,part) IN (SELECT slot,part FROM main.session_history_slot_parts WHERE namespace=?1 AND session_key=?2 ORDER BY slot,part LIMIT 64)"),params![self.namespace,key])?;
        if removed == 0 && parts == 0 {
            self.db.execute(self.sql("DELETE FROM main.meta WHERE key=?2", "DELETE FROM main.meta WHERE namespace=?1 AND key=?2"), params![self.namespace,format!("history_transfer:{key}")])?;
        }
        Ok(())
    }
    fn next_slot(&self, key: &str, last: Option<i64>) -> rusqlite::Result<Option<(i64,i64)>> {
        let sql = if last.is_some() {
            self.sql("SELECT slot,octet_length(metadata) FROM main.session_slots WHERE session_key=?2 AND slot>?3 ORDER BY slot LIMIT 1", "SELECT slot,octet_length(metadata) FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>?3 ORDER BY slot LIMIT 1")
        } else {
            self.sql("SELECT slot,octet_length(metadata) FROM main.session_slots WHERE session_key=?2 AND slot>=?3 ORDER BY slot LIMIT 1", "SELECT slot,octet_length(metadata) FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>=?3 ORDER BY slot LIMIT 1")
        };
        self.db.query_row(sql,params![self.namespace,key,last.unwrap_or(i64::MIN)],|r|Ok((r.get(0)?,r.get(1)?))).optional()
    }
    fn copy_slot(&self, key: &str, slot: i64, offset: i64, take: i64, split: bool, inspect: bool) -> rusqlite::Result<Option<String>> {
        if split {
            self.db.execute(self.sql("INSERT INTO main.session_history_slot_parts(session_key,slot,part,bytes) SELECT session_key,slot,?4,substr(CAST(metadata AS BLOB),?4+1,?5) FROM main.session_slots WHERE session_key=?2 AND slot=?3", "INSERT INTO main.session_history_slot_parts(namespace,session_key,slot,part,bytes) SELECT namespace,session_key,slot,?4,substr(CAST(metadata AS BLOB),?4+1,?5) FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot=?3"),params![self.namespace,key,slot,offset,take])?;
            return Ok(None);
        }
        self.db.execute(self.sql("INSERT INTO main.session_history_slots(session_key,slot,metadata) SELECT session_key,slot,metadata FROM main.session_slots WHERE session_key=?2 AND slot=?3", "INSERT INTO main.session_history_slots(namespace,session_key,slot,metadata) SELECT namespace,session_key,slot,metadata FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot=?3"),params![self.namespace,key,slot])?;
        if !inspect { return Ok(None); }
        self.db.query_row(self.sql("SELECT metadata FROM main.session_history_slots WHERE session_key=?2 AND slot=?3", "SELECT metadata FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 AND slot=?3"),params![self.namespace,key,slot], |r|r.get(0)).map(Some)
    }
    fn more_slots(&self, key: &str, last: Option<i64>) -> rusqlite::Result<bool> {
        let sql = if last.is_some() {
            self.sql("SELECT EXISTS(SELECT 1 FROM main.session_slots WHERE session_key=?2 AND slot>?3)", "SELECT EXISTS(SELECT 1 FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>?3)")
        } else {
            self.sql("SELECT EXISTS(SELECT 1 FROM main.session_slots WHERE session_key=?2 AND slot>=?3)", "SELECT EXISTS(SELECT 1 FROM main.session_slots WHERE namespace=?1 AND session_key=?2 AND slot>=?3)")
        };
        self.db.query_row(sql,params![self.namespace,key,last.unwrap_or(i64::MIN)],|r|r.get(0))
    }
    fn checkpoint(&self, key: &str, transfer: &str) -> rusqlite::Result<()> {
        self.db.execute(self.sql("INSERT OR REPLACE INTO main.meta(key,value) VALUES(?2,?3)", "INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value"),params![self.namespace,format!("history_transfer:{key}"),transfer])?;
        Ok(())
    }
    fn complete(&self, row: &CatalogRow, metadata: &str, original_header: &str, header: Option<&OriginalHeader>, complete: bool) -> rusqlite::Result<()> {
        let key = &row.key;
        self.db.execute(self.sql("INSERT INTO main.session_history_catalog(session_key,metadata) VALUES(?2,?3)", "INSERT INTO main.session_history_catalog(namespace,session_key,metadata) VALUES(?1,?2,?3)"),params![self.namespace,key,metadata])?;
        self.db.execute(self.sql("INSERT OR REPLACE INTO main.meta(key,value) VALUES(?2,?3)", "INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value"),params![self.namespace,format!("history_original_header:{key}"),original_header])?;
        if let Some((generation, version, sources, total)) = header {
            self.db.execute(self.sql("INSERT INTO main.session_history_projections(session_key,generation,source_generation,version,total,incomplete) VALUES(?2,?3,?4,?5,?6,?7)", "INSERT INTO main.session_history_projections(namespace,session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6,?7)"),params![self.namespace,key,generation,sources.as_deref().unwrap_or_default(),version,total,!complete])?;
        }
        self.db.execute(self.sql("DELETE FROM main.meta WHERE key=?2", "DELETE FROM main.meta WHERE namespace=?1 AND key=?2"),params![self.namespace,format!("history_transfer:{key}")])?;
        self.db.execute(self.sql("INSERT OR REPLACE INTO main.meta(key,value) VALUES(?2,'1')", "INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,'1') ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value"),params![self.namespace,format!("overview_needed:{key}")])?;
        Ok(())
    }
}

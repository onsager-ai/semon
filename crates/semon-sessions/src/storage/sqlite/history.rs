//! Authored physical operations for the shared bounded original-custody recipe.
use crate::model::summary::CatalogRow;
use crate::storage::catalog::{
    CompositeHistoryOwner, CompositeHistoryPublication, OriginalHeader, OriginalHistoryStore,
};
use rusqlite::{Connection, OptionalExtension, params};

/// Authored fresh private semantic profile; never converts singleton custody.
pub(crate) const SCHEMA: &str = "
CREATE TABLE session_history_catalog (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, metadata TEXT NOT NULL,
 query_recipe TEXT CHECK(query_recipe IS NULL OR octet_length(metadata)+octet_length(query_recipe)<=1048576),
 query_activity INTEGER, query_active INTEGER NOT NULL DEFAULT 0 CHECK(query_active IN (0,1)),
 PRIMARY KEY(namespace,session_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX session_history_query_activity ON session_history_catalog(namespace,query_activity,session_key);
CREATE INDEX session_history_query_active ON session_history_catalog(namespace,query_active,session_key);
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
    source: &'a super::semantic::AdmittedSource,
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
            source,
        })
    }
}

fn valid_history_slot_layout(
    slots: &[(i64, String)],
    parts: &[(i64, i64, i64)],
    expected_total: Option<i64>,
) -> bool {
    use std::collections::{BTreeMap, BTreeSet};
    let mut slot_numbers = BTreeSet::new();
    let direct_slots = slots.iter().map(|(slot, _)| *slot).collect::<BTreeSet<_>>();
    for (slot, _) in slots {
        if *slot < 0 || !slot_numbers.insert(*slot) {
            return false;
        }
    }
    let mut next_part = BTreeMap::<i64, i64>::new();
    for (slot, part, length) in parts {
        if *slot < 0 || *part < 0 || *length <= 0 || direct_slots.contains(slot) {
            return false;
        }
        let next = next_part.entry(*slot).or_insert(0);
        if *part != *next {
            return false;
        }
        *next += *length;
        slot_numbers.insert(*slot);
    }
    if slot_numbers
        .iter()
        .enumerate()
        .any(|(expected, actual)| i64::try_from(expected).ok() != Some(*actual))
    {
        return false;
    }
    expected_total.is_none_or(|total| total >= 0 && total == slot_numbers.len() as i64)
}

impl OriginalHistoryStore for OriginalHistory<'_> {
    type Error = rusqlite::Error;
    fn data(error: serde_json::Error) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }
    fn retained(&self, key: &str) -> rusqlite::Result<bool> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_retained");
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2)", params![self.namespace,key], |r|r.get(0))
    }
    fn previous(&self, row: &CatalogRow) -> rusqlite::Result<Option<CatalogRow>> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_previous");
        let text: Option<String> = self.db.query_row("SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2 AND metadata<>?3",params![self.namespace,row.key,serde_json::to_string(row).map_err(<Self as OriginalHistoryStore>::data)?],|r|r.get(0)).optional()?;
        let old: Option<CatalogRow> = text
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(<Self as OriginalHistoryStore>::data)?;
        Ok(old)
    }
    fn header(&self, key: &str) -> rusqlite::Result<Option<OriginalHeader>> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_header");
        self.db.query_row("SELECT generation,version,source_generation,total FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2", params![self.namespace,key], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()
    }
    fn transfer(&self, key: &str) -> rusqlite::Result<Option<String>> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_transfer");
        self.db
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key=?2",
                params![self.namespace, format!("history_transfer:{key}")],
                |r| r.get(0),
            )
            .optional()
    }
    fn reset_transfer(&self, key: &str) -> rusqlite::Result<()> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_reset_transfer");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_next_slot");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_copy_slot");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_more_slots");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_checkpoint");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=original_complete");
        let key = &row.key;
        let (activity, active) = crate::storage::query::history_activity(row);
        self.db.execute("INSERT INTO main.session_history_catalog(namespace,session_key,metadata,query_activity,query_active) VALUES(?1,?2,?3,?4,?5)",params![self.namespace,key,metadata,activity,active])?;
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
    fn refused() -> Self::Error {
        rusqlite::Error::InvalidQuery
    }
    fn validate_publication(
        &self,
        publication: &CompositeHistoryPublication,
    ) -> rusqlite::Result<()> {
        self.source
            .validate_publication(self.db)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let stream = self
            .source
            .stream_epoch()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        if !publication.matches_scope(self.namespace, self.source.admission(), stream) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }
    fn authenticate_owner(&self, key: &str) -> rusqlite::Result<CompositeHistoryOwner> {
        let metadata: Option<Option<String>> = self.db.query_row(
            "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
            rusqlite::params![self.namespace, key],
            |row| row.get(0),
        ).optional()?;
        let metadata = match metadata {
            Some(Some(metadata)) => Some(metadata),
            Some(None) => return Err(rusqlite::Error::InvalidQuery),
            None => None,
        };
        let projection: Option<(String, String, i64, i64, bool)> = self.db.query_row(
            "SELECT generation,source_generation,version,total,incomplete FROM main.session_history_projections WHERE namespace=?1 AND session_key=?2",
            rusqlite::params![self.namespace, key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).optional()?;
        let original_header: Option<String> = self.db.query_row(
            "SELECT value FROM main.meta WHERE namespace=?1 AND key=?2",
            rusqlite::params![self.namespace, format!("history_original_header:{key}")],
            |row| row.get(0),
        ).optional()?;
        let transfer: Option<String> = self.db.query_row(
            "SELECT value FROM main.meta WHERE namespace=?1 AND key=?2",
            rusqlite::params![self.namespace, format!("history_transfer:{key}")],
            |row| row.get(0),
        ).optional()?;
        let slots = self.db.prepare(
            "SELECT slot,metadata FROM main.session_history_slots WHERE namespace=?1 AND session_key=?2 ORDER BY slot LIMIT 65",
        )?.query_map(rusqlite::params![self.namespace, key], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(i64, String)>>>()?;
        let parts = self.db.prepare(
            "SELECT slot,part,length(bytes) FROM main.session_history_slot_parts WHERE namespace=?1 AND session_key=?2 ORDER BY slot,part LIMIT 513",
        )?.query_map(rusqlite::params![self.namespace, key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(i64, i64, i64)>>>()?;
        if slots.len() > 64 || parts.len() > 512 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let Some(metadata) = metadata else {
            if projection.is_some()
                || original_header.is_some()
                || transfer.is_some()
                || !slots.is_empty()
                || !parts.is_empty()
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            return Ok(CompositeHistoryOwner::Absent);
        };
        if transfer.is_some() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let Some(original_header) = original_header else {
            // A source/version/content header alone cannot authorize a prior
            // Composite owner. The terminal output receipt is added separately.
            return Err(rusqlite::Error::InvalidQuery);
        };
        let row: CatalogRow = serde_json::from_str(&metadata).map_err(Self::data)?;
        if row.key != key
            || serde_json::to_string(&row).map_err(Self::data)? != metadata
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if row.lifecycle != crate::model::summary::CatalogLifecycle::Retained {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let original: Option<OriginalHeader> =
            serde_json::from_str(&original_header).map_err(Self::data)?;
        match (original, projection.as_ref()) {
            (None, None) => {
                if !valid_history_slot_layout(&slots, &parts, None) {
                    return Err(rusqlite::Error::InvalidQuery);
                }
            }
            (Some((generation, version, source_generation, total)), Some(physical)) => {
                if physical.0 != generation
                    || physical.1 != source_generation.as_deref().unwrap_or_default()
                    || i64::from(version) != physical.2
                    || total != physical.3
                    || !valid_history_slot_layout(&slots, &parts, Some(total))
                {
                    return Err(rusqlite::Error::InvalidQuery);
                }
            }
            _ => return Err(rusqlite::Error::InvalidQuery),
        }
        Ok(CompositeHistoryOwner::Original)
    }
    fn original(&self, key: &str) -> rusqlite::Result<bool> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_original");
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_history_catalog h JOIN main.meta m ON m.namespace=h.namespace AND m.key=?3 WHERE h.namespace=?1 AND h.session_key=?2)",params![self.namespace,key,format!("history_original_header:{key}")],|r|r.get(0))
    }
    fn previous(&self, key: &str) -> rusqlite::Result<Option<String>> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_previous");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_history_exists");
        <Self as OriginalHistoryStore>::retained(self, key)
    }
    fn retain_source(
        &self,
        row: &CatalogRow,
        source: &crate::model::summary::CatalogSource,
    ) -> rusqlite::Result<()> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_retain_source");
        self.db.execute("INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES(?1,?2,'retained',?3,?4,?5) ON CONFLICT(namespace,session_key,source_path) DO UPDATE SET lifecycle='retained'",params![self.namespace,row.key,row.harness,source.native_id,source.path.to_string_lossy()])?;
        Ok(())
    }
    fn relocated(
        &self,
        old: &crate::model::summary::CatalogSource,
        current: &[crate::model::summary::CatalogSource],
    ) -> rusqlite::Result<bool> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_relocated");
        // The existing relocation witness reader already scopes metadata and
        // ledger reads through the admitted connection's native binding.
        crate::events::store::relocated_catalog_source(self.db, old, current)
    }
    fn clear(&self, key: &str) -> rusqlite::Result<()> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_clear");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_previous_recipe");
        let header=self.db.query_row("SELECT version,source_generation,incomplete FROM main.session_history_projections WHERE namespace=?1 AND session_key=?2",params![self.namespace,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if header.is_some() {
            return Ok((true, header));
        }
        let header=self.db.query_row("SELECT version,source_generation,0 FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2",params![self.namespace,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        Ok((false, header))
    }
    fn recipe_slots(&self, key: &str, history: bool) -> rusqlite::Result<Vec<String>> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_recipe_slots");
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
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_composed");
        let total = i64::try_from(texts.len()).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let changed=self.db.execute("INSERT INTO main.session_history_projections(namespace,session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(namespace,session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,version=excluded.version,total=excluded.total,incomplete=excluded.incomplete WHERE session_history_projections.generation != excluded.generation",params![self.namespace,key,generation,sources,crate::slot_projection::VERSION,total,incomplete])?;
        let row: CatalogRow = serde_json::from_str(metadata)
            .map_err(<Self as crate::storage::catalog::CompositeHistoryStore>::data)?;
        let (activity, active) = crate::storage::query::history_activity(&row);
        self.db.execute("INSERT INTO main.session_history_catalog(namespace,session_key,metadata,query_activity,query_active) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,session_key) DO UPDATE SET metadata=excluded.metadata,query_activity=excluded.query_activity,query_active=excluded.query_active WHERE session_history_catalog.metadata != excluded.metadata",params![self.namespace,key,metadata,activity,active])?;
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

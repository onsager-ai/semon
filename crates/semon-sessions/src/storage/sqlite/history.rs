//! Authored physical operations for the shared bounded original-custody recipe.
use crate::model::summary::CatalogRow;
use crate::storage::catalog::{OriginalHeader, OriginalHistoryStore};
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
    admission: u64,
    stream: Option<&'a str>,
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
            admission: source.admission(),
            stream: source.stream_epoch(),
        })
    }
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
        _key: &str,
        _generation: &str,
        _sources: &str,
        _metadata: &str,
        _texts: &[String],
        _incomplete: bool,
    ) -> rusqlite::Result<()> {
        Err(rusqlite::Error::InvalidQuery)
    }
}

impl crate::storage::catalog::VerifiedHistoryCompositionStore for OriginalHistory<'_> {
    fn composition_snapshot(
        &self,
        current: &CatalogRow,
        budget: &mut crate::overview::SummaryPublicationBudget,
    ) -> rusqlite::Result<crate::storage::catalog::HistoryCompositionSnapshot> {
        use crate::storage::catalog::HistoryCompositionSnapshot;

        let history_exists = <Self as crate::storage::catalog::CompositeHistoryStore>::history_exists(
            self,
            &current.key,
        )?;
        let (metadata_table, slots_table, projection_table) = if history_exists {
            (
                "main.session_history_catalog",
                "main.session_history_slots",
                "main.session_history_projections",
            )
        } else {
            (
                "main.session_catalog",
                "main.session_slots",
                "main.session_slot_projections",
            )
        };
        let previous_bytes: Option<i64> = self.db.query_row(
            &format!("SELECT octet_length(metadata) FROM {metadata_table} WHERE namespace=?1 AND session_key=?2"),
            params![self.namespace, current.key],
            |row| row.get(0),
        ).optional()?;
        let recipe_header_exists: bool = if history_exists {
            self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session_history_projections WHERE namespace=?1 AND session_key=?2)",
                params![self.namespace, current.key],
                |row| row.get(0),
            )?
        } else {
            self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2)",
                params![self.namespace, current.key],
                |row| row.get(0),
            )?
        };
        let header_bytes: i64 = self.db.query_row(
            &format!("SELECT COALESCE(sum(octet_length(generation)+COALESCE(octet_length(source_generation),0)),0) FROM {projection_table} WHERE namespace=?1 AND session_key=?2"),
            params![self.namespace, current.key],
            |row| row.get(0),
        )?;
        let (slot_count, slot_bytes): (i64, i64) = self.db.query_row(
            &format!("SELECT count(*),COALESCE(sum(octet_length(metadata)),0) FROM {slots_table} WHERE namespace=?1 AND session_key=?2"),
            params![self.namespace, current.key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let cwd_bytes = super::catalog::runtime_source_cwd_bytes(self.db, &current.sources)
            .ok()
            .and_then(|bytes| i64::try_from(bytes).ok())
            .unwrap_or(i64::MAX);
        let ledger_bytes = current
            .sources
            .len()
            .checked_mul(std::mem::size_of::<crate::events::Ledger>())
            .and_then(|bytes| i64::try_from(bytes).ok())
            .unwrap_or(i64::MAX);
        let sizes = [
            previous_bytes.unwrap_or(0),
            header_bytes,
            if recipe_header_exists { slot_bytes } else { 0 },
            cwd_bytes,
            ledger_bytes,
        ];
        let total_bytes = sizes.into_iter().try_fold(0usize, |total, bytes| {
            usize::try_from(bytes)
                .ok()
                .and_then(|bytes| total.checked_add(bytes))
        });
        let mut snapshot = HistoryCompositionSnapshot {
            key: current.key.clone(),
            original: false,
            previous: None,
            history_exists,
            recipe_history: history_exists,
            recipe_header: None,
            recipe_slots: Vec::new(),
            relocated: std::collections::BTreeSet::new(),
            receipts: Vec::new(),
            current_cwds: None,
            unavailable: false,
        };
        if slot_count > 64
            || total_bytes.is_none_or(|bytes| budget.charge(bytes).is_err())
        {
            snapshot.unavailable = true;
            return Ok(snapshot);
        }
        snapshot.recipe_header = if recipe_header_exists && history_exists {
            self.db.query_row(
                "SELECT version,source_generation,incomplete FROM main.session_history_projections WHERE namespace=?1 AND session_key=?2",
                params![self.namespace, current.key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?
        } else if recipe_header_exists {
            self.db.query_row(
                "SELECT version,source_generation,0 FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2",
                params![self.namespace, current.key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?
        } else {
            None
        };

        if previous_bytes.is_some() {
            snapshot.previous = self.db.query_row(
                &format!("SELECT metadata FROM {metadata_table} WHERE namespace=?1 AND session_key=?2"),
                params![self.namespace, current.key],
                |row| row.get(0),
            ).optional()?;
        }
        let previous_row = snapshot
            .previous
            .as_deref()
            .and_then(|text| serde_json::from_str::<CatalogRow>(text).ok());
        if snapshot.previous.is_some() && previous_row.is_none() {
            snapshot.unavailable = true;
        }
        if snapshot.recipe_header.is_some() {
            snapshot.recipe_slots = <Self as crate::storage::catalog::CompositeHistoryStore>::recipe_slots(
                self,
                &current.key,
                history_exists,
            )?;
        }
        if let Some(previous_row) = previous_row.as_ref() {
            if previous_row.sources.len() > 64 {
                snapshot.unavailable = true;
            } else {
                for source in &previous_row.sources {
                    if !current.sources.iter().any(|now| now.path == source.path)
                        && self.relocated(source, &current.sources)?
                    {
                        snapshot
                            .relocated
                            .insert(source.path.to_string_lossy().into_owned());
                    }
                }
            }
            if !snapshot.unavailable
                && snapshot.recipe_header.is_some()
                && let Some(stream) = self.stream
            {
                let exact_current_metadata = (!history_exists)
                    .then_some(snapshot.previous.as_deref())
                    .flatten();
                match super::proof_generations::verified_publication_receipts(
                    self.db,
                    self.namespace,
                    &current.key,
                    stream,
                    previous_row,
                    exact_current_metadata,
                    if history_exists {
                        super::proof_generations::PublicationReceiptRef::History
                    } else {
                        super::proof_generations::PublicationReceiptRef::Current
                    },
                    budget,
                )
                .map_err(|error| {
                    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                })? {
                    Some(receipts) => snapshot.receipts = receipts,
                    None => snapshot.unavailable = true,
                }
            }
        }
        snapshot.current_cwds = super::catalog::runtime_source_cwds_by_path_if_current(
            self.db,
            &current.sources,
        )
        .ok()
        .flatten()
        .map(|by_path| by_path.into_values().flatten().collect());
        Ok(snapshot)
    }

    fn composed_verified(
        &self,
        key: &str,
        generation: &str,
        sources: &str,
        metadata: &str,
        texts: &[String],
        incomplete: bool,
        _receipt_ids: &[String],
        capture: &str,
    ) -> rusqlite::Result<()> {
        #[cfg(test)]
        eprintln!("catalog_history_probe method=composite_composed");
        let total = i64::try_from(texts.len()).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let projection_changed = self.db.execute("INSERT INTO main.session_history_projections(namespace,session_key,generation,source_generation,version,total,incomplete) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(namespace,session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,version=excluded.version,total=excluded.total,incomplete=excluded.incomplete WHERE session_history_projections.generation != excluded.generation",params![self.namespace,key,generation,sources,crate::slot_projection::VERSION,total,incomplete])?;
        let row: CatalogRow = serde_json::from_str(metadata)
            .map_err(<Self as crate::storage::catalog::CompositeHistoryStore>::data)?;
        let (activity, active) = crate::storage::query::history_activity(&row);
        let metadata_changed = self.db.execute("INSERT INTO main.session_history_catalog(namespace,session_key,metadata,query_activity,query_active) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,session_key) DO UPDATE SET metadata=excluded.metadata,query_activity=excluded.query_activity,query_active=excluded.query_active WHERE session_history_catalog.metadata != excluded.metadata",params![self.namespace,key,metadata,activity,active])?;
        if projection_changed > 0 {
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
        if projection_changed > 0 || metadata_changed > 0 {
            self.db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,'1') ON CONFLICT(namespace,key) DO UPDATE SET value='1'",params![self.namespace,format!("overview_needed:{key}")])?;
        }
        super::proof_generations::native_history_observation_grant(
            self.db,
            self.namespace,
            key,
            self.admission,
            self.stream.ok_or(rusqlite::Error::InvalidQuery)?,
            metadata,
            capture,
        )
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        Ok(())
    }
}

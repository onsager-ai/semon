//! Shared original custody and missing-source composition over immutable PG
//! generations. This is staging, never an admission, head CAS or claim ACK.
use super::*;
use crate::storage::catalog::{CompositeHistoryStore, OriginalHeader, OriginalHistoryStore};

pub(super) struct History<'a, 'tx> {
    pub(super) effects: Effects<'a, 'tx>,
    pub(super) previous: &'a str,
}

fn text(bytes: Vec<u8>) -> Result<String, StorageError> {
    String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged)
}

impl History<'_, '_> {
    fn owner(&self, key: &str) -> Result<Option<(String, bool)>, StorageError> {
        self.owner_at(self.previous, key)
    }
    fn owner_at(
        &self,
        generation: &str,
        key: &str,
    ) -> Result<Option<(String, bool)>, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,owner_generation,original FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3", &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        rows.first()
            .map(|row| {
                if !row.get::<_, bool>(0) {
                    return Err(StorageError::ScopeChanged);
                }
                Ok((text(row.get(1))?, row.get(2)))
            })
            .transpose()
    }
    /// The owner row carries its header's keyset columns, so retained history
    /// filters and pages on the owner index without joining every header.
    fn link(&self, key: &str, owner: &str, original: bool) -> Result<(), StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let changed = tx.driver.execute("INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model,query_activity,query_active) SELECT namespace,$2,key_digest,session_key,$5,$6,last_ms,harness,repo,model,query_activity,query_active FROM semon_pg_native_catalog_history_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$5 AND key_digest=$3 AND session_key=$4 ON CONFLICT(namespace_digest,generation,key_digest) DO UPDATE SET generation=excluded.generation WHERE semon_pg_native_catalog_history_owners.namespace=excluded.namespace AND semon_pg_native_catalog_history_owners.session_key=excluded.session_key AND semon_pg_native_catalog_history_owners.owner_generation=excluded.owner_generation AND semon_pg_native_catalog_history_owners.original=excluded.original AND semon_pg_native_catalog_history_owners.last_ms=excluded.last_ms AND semon_pg_native_catalog_history_owners.harness=excluded.harness AND semon_pg_native_catalog_history_owners.repo=excluded.repo AND semon_pg_native_catalog_history_owners.model=excluded.model AND semon_pg_native_catalog_history_owners.query_activity IS NOT DISTINCT FROM excluded.query_activity AND semon_pg_native_catalog_history_owners.query_active=excluded.query_active", &[&ns, &self.effects.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes(), &owner.as_bytes(), &original]).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
        boundary(&tx)
    }
    fn metadata(
        &self,
        table: &'static str,
        generation: &str,
        key: &str,
    ) -> Result<Option<String>, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query(&format!("SELECT session_key=$4,CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3"), &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        rows.first()
            .map(|row| {
                if !row.get::<_, bool>(0) {
                    return Err(StorageError::ScopeChanged);
                }
                text(row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?)
            })
            .transpose()
    }
    fn recipe(
        &self,
        generation: &str,
        key: &str,
        history: bool,
    ) -> Result<Option<(OriginalHeader, bool)>, StorageError> {
        let table = if history {
            "semon_pg_native_catalog_history_headers"
        } else {
            "semon_pg_native_catalog_recipe_headers"
        };
        let incomplete = if history { "incomplete" } else { "false" };
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query(&format!("SELECT session_key=$4,recipe_generation,version,source_generation,total,{incomplete} FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3"), &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        rows.first()
            .map(|row| {
                if !row.get::<_, bool>(0) {
                    return Err(StorageError::ScopeChanged);
                }
                let version =
                    u32::try_from(row.get::<_, i64>(2)).map_err(|_| StorageError::ScopeChanged)?;
                Ok((
                    (
                        text(row.get(1))?,
                        version,
                        row.get::<_, Option<Vec<u8>>>(3).map(text).transpose()?,
                        row.get(4),
                    ),
                    row.get(5),
                ))
            })
            .transpose()
    }
    fn write_header(
        &self,
        key: &str,
        metadata: &str,
        header: Option<&OriginalHeader>,
        incomplete: bool,
        original: Option<&str>,
    ) -> Result<(), StorageError> {
        if metadata.len() > MAX || original.is_some_and(|value| value.len() > MAX) {
            return Err(StorageError::Busy);
        }
        let empty = (String::new(), 0, None, 0);
        let (recipe, version, source, total) = header.unwrap_or(&empty);
        let source = source.as_ref().map(|value| value.as_bytes().to_vec());
        let original = original.map(|value| value.as_bytes().to_vec());
        // Header columns mirror catalog_rows so retained history filters and
        // keysets in SQL; the reader re-checks them against the metadata.
        let row: CatalogRow =
            serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
        if row.key != key {
            return Err(StorageError::ScopeChanged);
        }
        let (activity, active) = crate::storage::query::history_activity(&row);
        self.effects.put(
            "semon_pg_native_catalog_history_headers",
            &[
                "key_digest",
                "session_key",
                "recipe_generation",
                "source_generation",
                "version",
                "total",
                "incomplete",
                "metadata",
                "original_header",
                "last_ms",
                "harness",
                "repo",
                "model",
                "query_activity",
                "query_active",
            ],
            "key_digest",
            &[
                &digest(key.as_bytes()),
                &key.as_bytes(),
                &recipe.as_bytes(),
                &source,
                &i64::from(*version),
                total,
                &incomplete,
                &metadata.as_bytes(),
                &original,
                &row.last.unwrap_or(0),
                &row.harness.as_bytes(),
                &row.repo.as_deref().unwrap_or("").as_bytes(),
                &row.model.as_bytes(),
                &activity,
                &active,
            ],
        )?;
        self.link(key, self.effects.generation, original.is_some())
    }
    fn put_slot(&self, key: &str, slot: i64, metadata: &str) -> Result<(), StorageError> {
        if metadata.len() > MAX {
            return Err(StorageError::Busy);
        }
        self.effects.put(
            "semon_pg_native_catalog_history_slots",
            &["key_digest", "session_key", "slot", "metadata"],
            "key_digest,slot",
            &[
                &digest(key.as_bytes()),
                &key.as_bytes(),
                &slot,
                &metadata.as_bytes(),
            ],
        )
    }
}

impl OriginalHistoryStore for History<'_, '_> {
    type Error = StorageError;
    fn data(_: serde_json::Error) -> StorageError {
        StorageError::ScopeChanged
    }
    fn retained(&self, key: &str) -> Result<bool, StorageError> {
        let current = self.owner_at(self.effects.generation, key)?;
        if let Some((owner, true)) = current.or(self.owner(key)?) {
            // Preserve custody by reference; no recipe copying or latest-path
            // overwrite can invalidate the previous immutable owner.
            if self.recipe(&owner, key, true)?.is_none() {
                return Err(StorageError::ScopeChanged);
            }
            self.link(key, &owner, true)?;
            return Ok(true);
        }
        Ok(false)
    }
    fn replacement(&self, row: &CatalogRow) -> Result<Option<CatalogRow>, StorageError> {
        let previous = self.metadata("semon_pg_native_catalog_rows", self.previous, &row.key)?;
        // Same-harness source withdrawal composes retained sources; only a
        // different native harness replaces the original owner.
        if previous.as_deref()
            == Some(
                serde_json::to_string(row)
                    .map_err(|_| StorageError::ScopeChanged)?
                    .as_str(),
            )
        {
            return Ok(None);
        }
        let old: Option<CatalogRow> = previous
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| StorageError::ScopeChanged)?;
        Ok(old.filter(|old| old.harness != row.harness))
    }
    fn header(&self, key: &str) -> Result<Option<OriginalHeader>, StorageError> {
        Ok(self
            .recipe(self.previous, key, false)?
            .map(|(header, _)| header))
    }
    fn transfer(&self, key: &str) -> Result<Option<String>, StorageError> {
        self.metadata(
            "semon_pg_native_catalog_history_progress",
            self.effects.generation,
            key,
        )
    }
    fn reset_transfer(&self, _: &str) -> Result<(), StorageError> {
        // A changed same-generation custody request is not a reset authority.
        Err(StorageError::ScopeChanged)
    }
    fn next_slot(&self, key: &str, last: Option<i64>) -> Result<Option<(i64, i64)>, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,slot,octet_length(metadata)::bigint FROM semon_pg_native_catalog_recipe_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND slot>$5 ORDER BY slot LIMIT 1", &[&ns, &self.previous.as_bytes(), &digest(key.as_bytes()), &key.as_bytes(), &last.unwrap_or(-1)]).map_err(sqlstate::error)?;
        rows.first()
            .map(|row| {
                if !row.get::<_, bool>(0) {
                    return Err(StorageError::ScopeChanged);
                }
                Ok((row.get(1), row.get(2)))
            })
            .transpose()
    }
    fn copy_slot(
        &self,
        key: &str,
        slot: i64,
        offset: i64,
        take: i64,
        split: bool,
        inspect: bool,
    ) -> Result<Option<String>, StorageError> {
        // Current PG recipe rows are <=1 MiB. An oversized/corrupt row must
        // refuse before decode, never be truncated into a successful recipe.
        if split || offset != 0 || !(0..=MAX as i64).contains(&take) {
            return Err(StorageError::Busy);
        }
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,CASE WHEN octet_length(metadata)::bigint=$6 AND octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM semon_pg_native_catalog_recipe_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND slot=$5", &[&ns, &self.previous.as_bytes(), &digest(key.as_bytes()), &key.as_bytes(), &slot, &take]).map_err(sqlstate::error)?;
        let row = rows.first().ok_or(StorageError::ScopeChanged)?;
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        let metadata = text(row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?)?;
        drop(tx);
        self.put_slot(key, slot, &metadata)?;
        Ok(inspect.then_some(metadata))
    }
    fn more_slots(&self, key: &str, last: Option<i64>) -> Result<bool, StorageError> {
        Ok(self.next_slot(key, last)?.is_some())
    }
    fn checkpoint(&self, key: &str, transfer: &str) -> Result<(), StorageError> {
        if transfer.len() > MAX {
            return Err(StorageError::Busy);
        }
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let count = tx.driver.execute("INSERT INTO semon_pg_native_catalog_history_progress(namespace,generation,key_digest,session_key,metadata) VALUES($1,$2,$3,$4,$5) ON CONFLICT(namespace_digest,generation,key_digest) DO UPDATE SET metadata=excluded.metadata WHERE semon_pg_native_catalog_history_progress.namespace=excluded.namespace AND semon_pg_native_catalog_history_progress.session_key=excluded.session_key", &[&ns, &self.effects.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes(), &transfer.as_bytes()]).map_err(sqlstate::error)?;
        if count != 1 {
            return Err(StorageError::ScopeChanged);
        }
        boundary(&tx)
    }
    fn complete(
        &self,
        row: &CatalogRow,
        metadata: &str,
        original_header: &str,
        header: Option<&OriginalHeader>,
        complete: bool,
    ) -> Result<(), StorageError> {
        for source in &row.sources {
            self.retain_source(row, source)?;
        }
        self.write_header(&row.key, metadata, header, !complete, Some(original_header))
    }
}

impl CompositeHistoryStore for History<'_, '_> {
    type Error = StorageError;
    fn data(_: serde_json::Error) -> StorageError {
        StorageError::ScopeChanged
    }
    fn original(&self, key: &str) -> Result<bool, StorageError> {
        self.retained(key)
    }
    fn previous(&self, key: &str) -> Result<Option<String>, StorageError> {
        if let Some((owner, _)) = self.owner(key)? {
            return self
                .metadata("semon_pg_native_catalog_history_headers", &owner, key)?
                .map(Some)
                .ok_or(StorageError::ScopeChanged);
        }
        self.metadata("semon_pg_native_catalog_rows", self.previous, key)
    }
    fn history_exists(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.owner_at(self.effects.generation, key)?.is_some() || self.owner(key)?.is_some())
    }
    fn retain_source(&self, row: &CatalogRow, source: &CatalogSource) -> Result<(), StorageError> {
        let path = path(source)?.as_bytes();
        // Retain the exact Native prefix alongside the historical source.
        // A successor can also publish a newer prefix for this same path.
        let receipt = crate::events::store::header_ledger(&crate::source_carrier::ledger(source));
        let tx = self.effects.tx.borrow_mut();
        let ns = tx.namespace.as_str().as_bytes();
        let changed = tx.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) SELECT p.namespace,$2,p.path_digest,p.source_path,p.page_generation,p.usage_generation,p.usage_ready FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_page_generations g ON g.namespace_digest=p.namespace_digest AND g.namespace=p.namespace AND g.generation=p.page_generation WHERE p.namespace_digest=sha256($1::bytea) AND p.namespace=$1 AND p.generation=$3 AND p.path_digest=sha256($4::bytea) AND p.source_path=$4 AND g.source_path=$4 AND g.ledger=$5 ON CONFLICT DO NOTHING", &[&ns, &self.effects.generation.as_bytes(), &self.previous.as_bytes(), &path, &receipt.as_bytes()]).map_err(sqlstate::error)?;
        if changed == 0 {
            let retained: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_page_generations g ON g.namespace_digest=p.namespace_digest AND g.namespace=p.namespace AND g.generation=p.page_generation WHERE p.namespace_digest=sha256($1::bytea) AND p.namespace=$1 AND p.generation=$2 AND p.path_digest=sha256($3::bytea) AND p.source_path=$3 AND g.source_path=$3 AND g.ledger=$4)", &[&ns, &self.effects.generation.as_bytes(), &path, &receipt.as_bytes()]).map_err(sqlstate::error)?.get(0);
            if !retained {
                return Err(StorageError::ScopeChanged);
            }
        }
        drop(tx);
        self.effects.put(
            "semon_pg_native_catalog_history_sources",
            &[
                "key_digest",
                "session_key",
                "path_digest",
                "source_path",
                "metadata",
                "native_digest",
            ],
            "key_digest,path_digest",
            &[
                &digest(row.key.as_bytes()),
                &row.key.as_bytes(),
                &digest(path),
                &path,
                &encoded(source)?,
                &digest(source.native_id.as_bytes()),
            ],
        )
    }
    fn relocated(
        &self,
        old: &CatalogSource,
        current: &[CatalogSource],
    ) -> Result<bool, StorageError> {
        let mut tx = self.effects.tx.borrow_mut();
        let key = format!("native_catalog_relocation:{}", path(old)?);
        let Some(bytes) = super::super::native_keys::read_meta(&mut tx, key.as_bytes(), 16384)?
        else {
            return Ok(false);
        };
        let proof: crate::events::native_commit::CatalogRelocation =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if proof.version != 1
            || proof.native_id != old.native_id
            || (!catalog::source_matches_ledger(old, &proof.prior)
                && !proof
                    .catalog_before
                    .as_ref()
                    .is_some_and(|l| catalog::source_matches_ledger(old, l)))
        {
            return Ok(false);
        }
        for source in current {
            if source.path.to_str() == Some(proof.new_path.as_str())
                && source.native_id == proof.native_id
                && catalog::source_matches_ledger(source, &proof.after)
                && super::super::native_reader::ledger(&mut tx, &proof.new_path)?.map(|(_, l)| l)
                    == Some(proof.after.clone())
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn clear(&self, _: &str) -> Result<(), StorageError> {
        self.effects.tx.borrow().check()
    }
    fn previous_recipe(
        &self,
        key: &str,
    ) -> Result<(bool, Option<catalog::HistoryRecipeHeader>), StorageError> {
        let owner = self.owner(key)?;
        let history = owner.is_some();
        let generation = owner
            .as_ref()
            .map(|(generation, _)| generation.as_str())
            .unwrap_or(self.previous);
        Ok((
            history,
            self.recipe(generation, key, history)?
                .map(|((_, version, source, _), incomplete)| (version, source, incomplete)),
        ))
    }
    fn recipe_slots(&self, key: &str, history: bool) -> Result<Vec<String>, StorageError> {
        let owner = self.owner(key)?;
        let generation = if history {
            owner
                .as_ref()
                .map(|(generation, _)| generation.as_str())
                .ok_or(StorageError::ScopeChanged)?
        } else {
            self.previous
        };
        let table = if history {
            "semon_pg_native_catalog_history_slots"
        } else {
            "semon_pg_native_catalog_recipe_rows"
        };
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query(&format!("SELECT session_key=$4,CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata ELSE NULL END FROM (SELECT slot,session_key,metadata FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 ORDER BY slot LIMIT 65) p ORDER BY slot"), &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        if rows.len() > 64 {
            return Err(StorageError::Busy);
        }
        rows.iter()
            .map(|row| {
                if !row.get::<_, bool>(0) {
                    return Err(StorageError::ScopeChanged);
                }
                text(row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?)
            })
            .collect()
    }
    fn composed(
        &self,
        key: &str,
        generation: &str,
        sources: &str,
        metadata: &str,
        texts: &[String],
        incomplete: bool,
    ) -> Result<(), StorageError> {
        if texts.len() > 64 || texts.iter().map(String::len).sum::<usize>() > MAX {
            return Err(StorageError::Busy);
        }
        let header = (
            generation.to_owned(),
            crate::slot_projection::VERSION,
            Some(sources.to_owned()),
            texts.len() as i64,
        );
        self.write_header(key, metadata, Some(&header), incomplete, None)?;
        for (slot, metadata) in texts.iter().enumerate() {
            self.put_slot(key, slot as i64, metadata)?;
        }
        Ok(())
    }
}

/// Call repeatedly with the same immutable generations until custody is done;
/// false commits only bounded custody staging, never a Catalog receipt.
pub(super) fn preserve(
    tx: &mut PgTxn<'_>,
    generation: &str,
    previous: &str,
    publication: &crate::slot_projection::Publication<'_>,
    withdrawn: &[Vec<u8>],
) -> Result<bool, StorageError> {
    for generation in [generation, previous] {
        if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidArguments);
        }
    }
    if generation == previous || publication.rows.len() > 64 || withdrawn.len() > 64 {
        return Err(StorageError::InvalidArguments);
    }
    // Rows withdrawn from Current are read from the previous immutable
    // generation; each digest must name exactly one bounded row.
    let mut old = Vec::new();
    for key_digest in withdrawn {
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3", &[&ns, &previous.as_bytes(), key_digest]).map_err(sqlstate::error)?;
        let row = rows.first().ok_or(StorageError::ScopeChanged)?;
        let key: Vec<u8> = row.get(0);
        let bytes = row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?;
        let value: CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if digest(&key) != *key_digest || value.key.as_bytes() != key {
            return Err(StorageError::ScopeChanged);
        }
        old.push(value);
    }
    let store = History {
        effects: Effects {
            tx: RefCell::new(tx),
            generation,
        },
        previous,
    };
    if !catalog::preserve_rows(&store, publication.rows, &old)? {
        return Ok(false);
    }
    catalog::publish_history_rows(&store, publication.rows, publication.transcripts)?;
    Ok(true)
}

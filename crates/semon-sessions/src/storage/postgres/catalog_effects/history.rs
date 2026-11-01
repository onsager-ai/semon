//! Shared original custody and missing-source composition over immutable PG
//! generations. This is staging, never an admission, head CAS or claim ACK.
use super::*;
use crate::storage::catalog::{CompositeHistoryStore, OriginalHeader, OriginalHistoryStore};

type ProofTuple = (Vec<u8>, i64, bool);
type TraceFamilyHeader = (String, String, Vec<u8>);

struct RecipeContents {
    header: OriginalHeader,
    incomplete: bool,
    slots: Vec<String>,
}

enum WithdrawnComposite {
    NotApplicable,
    Complete,
    Staged,
}

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
    /// The owner row carries its header's keyset and parent columns, so
    /// retained history filters, pages and family traversal use indexes.
    fn link(&self, key: &str, owner: &str, original: bool) -> Result<(), StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let changed = tx.driver.execute("INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model,query_activity,query_active,parent_key) SELECT namespace,$2,key_digest,session_key,$5,$6,last_ms,harness,repo,model,query_activity,query_active,parent_key FROM semon_pg_native_catalog_history_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$5 AND key_digest=$3 AND session_key=$4 ON CONFLICT(namespace_digest,generation,key_digest) DO UPDATE SET generation=excluded.generation WHERE semon_pg_native_catalog_history_owners.namespace=excluded.namespace AND semon_pg_native_catalog_history_owners.session_key=excluded.session_key AND semon_pg_native_catalog_history_owners.owner_generation=excluded.owner_generation AND semon_pg_native_catalog_history_owners.original=excluded.original AND semon_pg_native_catalog_history_owners.last_ms=excluded.last_ms AND semon_pg_native_catalog_history_owners.harness=excluded.harness AND semon_pg_native_catalog_history_owners.repo=excluded.repo AND semon_pg_native_catalog_history_owners.model=excluded.model AND semon_pg_native_catalog_history_owners.query_activity IS NOT DISTINCT FROM excluded.query_activity AND semon_pg_native_catalog_history_owners.query_active=excluded.query_active AND semon_pg_native_catalog_history_owners.parent_key IS NOT DISTINCT FROM excluded.parent_key", &[&ns, &self.effects.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes(), &owner.as_bytes(), &original]).map_err(sqlstate::error)?;
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

    fn recipe_slots_at(
        &self,
        generation: &str,
        key: &str,
        history: bool,
    ) -> Result<Vec<String>, StorageError> {
        let table = if history {
            "semon_pg_native_catalog_history_slots"
        } else {
            "semon_pg_native_catalog_recipe_rows"
        };
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query(&format!("SELECT session_key=$4,slot,CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata ELSE NULL END FROM (SELECT slot,session_key,metadata FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 ORDER BY slot LIMIT 65) p ORDER BY slot"), &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        if rows.len() > 64 {
            return Err(StorageError::Busy);
        }
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                if !row.get::<_, bool>(0) || row.get::<_, i64>(1) != index as i64 {
                    return Err(StorageError::ScopeChanged);
                }
                text(row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?)
            })
            .collect()
    }

    fn recipe_contents(
        &self,
        generation: &str,
        key: &str,
        history: bool,
    ) -> Result<RecipeContents, StorageError> {
        let (header, incomplete) = self
            .recipe(generation, key, history)?
            .ok_or(StorageError::ScopeChanged)?;
        let slots = self.recipe_slots_at(generation, key, history)?;
        if header.3 < 0 || usize::try_from(header.3).ok() != Some(slots.len()) {
            return Err(StorageError::ScopeChanged);
        }
        Ok(RecipeContents {
            header,
            incomplete,
            slots,
        })
    }

    fn history_sources_at(
        &self,
        generation: &str,
        key: &str,
    ) -> Result<std::collections::BTreeMap<String, CatalogSource>, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,source_path,CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata ELSE NULL END,native_digest FROM semon_pg_native_catalog_history_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 ORDER BY source_path LIMIT 65", &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        if rows.len() > 64 {
            return Err(StorageError::Busy);
        }
        let mut sources = std::collections::BTreeMap::new();
        for row in rows {
            if !row.get::<_, bool>(0) {
                return Err(StorageError::ScopeChanged);
            }
            let path = text(row.get(1))?;
            let bytes = row
                .get::<_, Option<Vec<u8>>>(2)
                .ok_or(StorageError::Busy)?;
            let source: CatalogSource =
                serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
            if source.path.to_str() != Some(path.as_str())
                || row.get::<_, Vec<u8>>(3) != digest(source.native_id.as_bytes())
                || sources.insert(path, source).is_some()
            {
                return Err(StorageError::ScopeChanged);
            }
        }
        Ok(sources)
    }

    fn proof_tuples_at(
        &self,
        generations: &[&str],
        sources: &[CatalogSource],
    ) -> Result<std::collections::BTreeMap<(Vec<u8>, Vec<u8>), ProofTuple>, StorageError> {
        if generations.len() > 2 || sources.len() > 64 {
            return Err(StorageError::InvalidArguments);
        }
        if generations.is_empty() || sources.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }
        let generation_bytes = generations
            .iter()
            .map(|generation| generation.as_bytes().to_vec())
            .collect::<Vec<_>>();
        let source_paths = sources
            .iter()
            .map(|source| path(source).map(|path| path.as_bytes().to_vec()))
            .collect::<Result<Vec<_>, _>>()?;
        let expected_receipts = sources
            .iter()
            .map(|source| {
                let path = path(source)?.as_bytes().to_vec();
                let receipt = crate::events::store::header_ledger(
                    &crate::source_carrier::ledger(source),
                );
                Ok((path, receipt.into_bytes()))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>, StorageError>>()?;
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT p.generation,p.source_path,p.path_digest=sha256(p.source_path),p.page_generation,p.usage_generation,p.usage_ready,g.source_path=p.source_path,g.ledger FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_page_generations g ON g.namespace_digest=p.namespace_digest AND g.namespace=p.namespace AND g.generation=p.page_generation WHERE p.namespace_digest=sha256($1::bytea) AND p.namespace=$1 AND p.generation=ANY($2::bytea[]) AND p.source_path=ANY($3::bytea[]) ORDER BY p.generation,p.source_path,p.page_generation LIMIT 129", &[&ns, &generation_bytes, &source_paths]).map_err(sqlstate::error)?;
        if rows.len() > generations.len() * sources.len() {
            return Err(StorageError::ScopeChanged);
        }
        let mut proofs = std::collections::BTreeMap::new();
        for row in rows {
            let generation = row.get::<_, Vec<u8>>(0);
            let path = row.get::<_, Vec<u8>>(1);
            let receipt = row.get::<_, Vec<u8>>(7);
            if !row.get::<_, bool>(2)
                || !row.get::<_, bool>(6)
                || expected_receipts.get(&path) != Some(&receipt)
            {
                return Err(StorageError::ScopeChanged);
            }
            let tuple = (row.get(3), row.get(4), row.get(5));
            if proofs.insert((generation, path), tuple).is_some() {
                return Err(StorageError::ScopeChanged);
            }
        }
        if proofs.keys().any(|(generation, path)| {
            !generation_bytes.contains(generation) || !source_paths.contains(path)
        }) {
            return Err(StorageError::ScopeChanged);
        }
        Ok(proofs)
    }

    fn has_original_header(&self, generation: &str, key: &str) -> Result<bool, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,original_header IS NOT NULL FROM semon_pg_native_catalog_history_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3", &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        if rows.len() != 1 || !rows[0].get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        Ok(rows[0].get(1))
    }

    fn denormalized_history_matches(
        &self,
        header_generation: &str,
        owner_generation: &str,
        key: &str,
        row: &CatalogRow,
    ) -> Result<bool, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT h.session_key=$4,h.last_ms,h.harness,h.repo,h.model,h.query_activity,h.query_active,h.parent_key,o.session_key=$4,o.owner_generation,o.original,o.last_ms,o.harness,o.repo,o.model,o.query_activity,o.query_active,o.parent_key FROM semon_pg_native_catalog_history_headers h JOIN semon_pg_native_catalog_history_owners o ON o.namespace_digest=h.namespace_digest AND o.namespace=h.namespace AND o.key_digest=h.key_digest AND o.session_key=h.session_key AND o.owner_generation=h.generation WHERE h.namespace_digest=sha256($1::bytea) AND h.namespace=$1 AND h.generation=$2 AND h.key_digest=$3 AND h.session_key=$4 AND o.generation=$5", &[&ns, &header_generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes(), &owner_generation.as_bytes()]).map_err(sqlstate::error)?;
        if rows.len() != 1 || !rows[0].get::<_, bool>(0) || !rows[0].get::<_, bool>(8) {
            return Ok(false);
        }
        let last = row.last.unwrap_or(0);
        let harness = row.harness.as_bytes();
        let repo = row.repo.as_deref().unwrap_or("").as_bytes();
        let model = row.model.as_bytes();
        let (activity, active) = crate::storage::query::history_activity(row);
        let parent = row.parent.as_deref().map(str::as_bytes);
        Ok(rows[0].get::<_, Vec<u8>>(9).as_slice() == header_generation.as_bytes()
            && !rows[0].get::<_, bool>(10)
            && rows[0].get::<_, i64>(1) == last
            && rows[0].get::<_, Option<Vec<u8>>>(2).as_deref() == Some(harness)
            && rows[0].get::<_, Option<Vec<u8>>>(3).as_deref() == Some(repo)
            && rows[0].get::<_, Option<Vec<u8>>>(4).as_deref() == Some(model)
            && rows[0].get::<_, Option<i64>>(5) == activity
            && rows[0].get::<_, bool>(6) == active
            && rows[0].get::<_, Option<Vec<u8>>>(7).as_deref() == parent
            && rows[0].get::<_, i64>(11) == last
            && rows[0].get::<_, Option<Vec<u8>>>(12).as_deref() == Some(harness)
            && rows[0].get::<_, Option<Vec<u8>>>(13).as_deref() == Some(repo)
            && rows[0].get::<_, Option<Vec<u8>>>(14).as_deref() == Some(model)
            && rows[0].get::<_, Option<i64>>(15) == activity
            && rows[0].get::<_, bool>(16) == active
            && rows[0].get::<_, Option<Vec<u8>>>(17).as_deref() == parent)
    }

    fn trace_header_at(
        &self,
        generation: &str,
        key: &str,
    ) -> Result<Option<TraceFamilyHeader>, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,trace_generation,source_generation,CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 LIMIT 2", &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        if rows.len() > 1 {
            return Err(StorageError::ScopeChanged);
        }
        rows.first()
            .map(|row| {
                if !row.get::<_, bool>(0) {
                    return Err(StorageError::ScopeChanged);
                }
                Ok((
                    text(row.get(1))?,
                    text(row.get(2))?,
                    row.get::<_, Option<Vec<u8>>>(3).ok_or(StorageError::Busy)?,
                ))
            })
            .transpose()
    }

    fn trace_members_without_header(&self, generation: &str, key: &str) -> Result<bool, StorageError> {
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        Ok(tx
            .driver
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_turns c WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.generation=$2 AND c.key_digest=$3 AND c.session_key=$4 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_headers h WHERE h.namespace_digest=c.namespace_digest AND h.namespace=c.namespace AND h.generation=c.generation AND h.key_digest=c.key_digest AND h.session_key=$4)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_edges c WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.generation=$2 AND c.key_digest=$3 AND c.session_key=$4 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_headers h WHERE h.namespace_digest=c.namespace_digest AND h.namespace=c.namespace AND h.generation=c.generation AND h.key_digest=c.key_digest AND h.session_key=$4)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_bookmarks c WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.generation=$2 AND c.key_digest=$3 AND c.session_key=$4 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_headers h WHERE h.namespace_digest=c.namespace_digest AND h.namespace=c.namespace AND h.generation=c.generation AND h.key_digest=c.key_digest AND h.session_key=$4))",
                &[&ns, &generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()],
            )
            .map_err(sqlstate::error)?
            .get(0))
    }

    fn trace_family_matches(
        &self,
        key: &str,
        source_generation: &str,
    ) -> Result<bool, StorageError> {
        let previous = self.trace_header_at(self.previous, key)?;
        let target = self.trace_header_at(self.effects.generation, key)?;
        if self.trace_members_without_header(self.previous, key)?
            || self.trace_members_without_header(self.effects.generation, key)?
        {
            return Ok(false);
        }
        let Some(previous) = previous.filter(|header| header.1 == source_generation) else {
            return Ok(target.is_none());
        };
        if target.as_ref() != Some(&previous) {
            return Ok(false);
        }
        let tx = self.effects.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let changed: bool = tx
            .driver
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_turns s WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.key_digest=$4 AND s.session_key=$5 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_turns t WHERE t.namespace_digest=s.namespace_digest AND t.namespace=s.namespace AND t.generation=$3 AND t.key_digest=s.key_digest AND t.session_key=$5 AND t.turn_digest=s.turn_digest AND t.turn_id=s.turn_id AND t.start_id IS NOT DISTINCT FROM s.start_id AND t.metadata=s.metadata)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_turns t WHERE t.namespace_digest=sha256($1::bytea) AND t.namespace=$1 AND t.generation=$3 AND t.key_digest=$4 AND t.session_key=$5 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_turns s WHERE s.namespace_digest=t.namespace_digest AND s.namespace=t.namespace AND s.generation=$2 AND s.key_digest=t.key_digest AND s.session_key=$5 AND s.turn_digest=t.turn_digest AND s.turn_id=t.turn_id AND s.start_id IS NOT DISTINCT FROM t.start_id AND s.metadata=t.metadata)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_edges s WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.key_digest=$4 AND s.session_key=$5 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_edges t WHERE t.namespace_digest=s.namespace_digest AND t.namespace=s.namespace AND t.generation=$3 AND t.key_digest=s.key_digest AND t.session_key=$5 AND t.edge_digest=s.edge_digest AND t.edge_id=s.edge_id AND t.metadata=s.metadata)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_edges t WHERE t.namespace_digest=sha256($1::bytea) AND t.namespace=$1 AND t.generation=$3 AND t.key_digest=$4 AND t.session_key=$5 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_edges s WHERE s.namespace_digest=t.namespace_digest AND s.namespace=t.namespace AND s.generation=$2 AND s.key_digest=t.key_digest AND s.session_key=$5 AND s.edge_digest=t.edge_digest AND s.edge_id=t.edge_id AND s.metadata=t.metadata)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_bookmarks s WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.key_digest=$4 AND s.session_key=$5 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_bookmarks t WHERE t.namespace_digest=s.namespace_digest AND t.namespace=s.namespace AND t.generation=$3 AND t.key_digest=s.key_digest AND t.session_key=$5 AND t.target_digest=s.target_digest AND t.target=s.target AND t.edge_digest=s.edge_digest AND t.edge_id=s.edge_id)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_bookmarks t WHERE t.namespace_digest=sha256($1::bytea) AND t.namespace=$1 AND t.generation=$3 AND t.key_digest=$4 AND t.session_key=$5 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_trace_bookmarks s WHERE s.namespace_digest=t.namespace_digest AND s.namespace=t.namespace AND s.generation=$2 AND s.key_digest=t.key_digest AND s.session_key=$5 AND s.target_digest=t.target_digest AND s.target=t.target AND s.edge_digest=t.edge_digest AND s.edge_id=t.edge_id))",
                &[
                    &ns,
                    &self.previous.as_bytes(),
                    &self.effects.generation.as_bytes(),
                    &digest(key.as_bytes()),
                    &key.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)?
            .get(0);
        Ok(!changed)
    }

    fn validate_saved_slots(
        slots: &[String],
        sources: &[CatalogSource],
    ) -> Result<std::collections::BTreeMap<String, String>, StorageError> {
        use std::collections::{BTreeMap, BTreeSet};
        let paths = sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<BTreeSet<_>>();
        if paths.len() != sources.len() || sources.len() > 64 {
            return Err(StorageError::ScopeChanged);
        }
        let mut by_identity = BTreeMap::new();
        for text in slots {
            let slot: crate::slot_projection::SavedSlot =
                serde_json::from_str(text).map_err(|_| StorageError::ScopeChanged)?;
            if slot.source_paths().iter().any(|path| !paths.contains(*path))
                || by_identity.insert(slot.identity(), text.clone()).is_some()
            {
                return Err(StorageError::ScopeChanged);
            }
        }
        Ok(by_identity)
    }

    fn validate_withdrawn_composite(
        &self,
        current: &CatalogRow,
        owner_generation: &str,
        metadata: &str,
    ) -> Result<(CatalogRow, RecipeContents), StorageError> {
        use std::collections::{BTreeMap, BTreeSet};
        use sha2::{Digest, Sha256};

        let history: CatalogRow =
            serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
        if self.has_original_header(owner_generation, &current.key)?
            || history.key != current.key
            || history.query.is_some()
            || history.sources.len() > 64
        {
            return Err(StorageError::ScopeChanged);
        }
        if !self.denormalized_history_matches(
            owner_generation,
            self.previous,
            &current.key,
            &history,
        )? {
            return Err(StorageError::ScopeChanged);
        }
        let paths = history
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<BTreeSet<_>>();
        if paths.len() != history.sources.len()
            || current
                .sources
                .iter()
                .any(|source| !history.sources.contains(source))
        {
            return Err(StorageError::ScopeChanged);
        }
        let native_ids = history
            .sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if native_ids != history.native_ids {
            return Err(StorageError::ScopeChanged);
        }
        let mut projected = current.clone();
        projected.query = None;
        projected.sources = history.sources.clone();
        projected.native_ids = history.native_ids.clone();
        if serde_json::to_value(&projected).map_err(|_| StorageError::ScopeChanged)?
            != serde_json::to_value(&history).map_err(|_| StorageError::ScopeChanged)?
        {
            return Err(StorageError::ScopeChanged);
        }

        let current_recipe = self.recipe_contents(self.previous, &current.key, false)?;
        let current_source_generation =
            crate::retention::source_generation(&current.sources)
                .map_err(|_| StorageError::ScopeChanged)?;
        if current_recipe.incomplete
            || current_recipe.header.1 != crate::slot_projection::VERSION
            || current_recipe.header.2.as_deref() != Some(current_source_generation.as_str())
            || crate::slot_projection::generation(&current_recipe.slots, &current.sources)
                .map_err(|_| StorageError::ScopeChanged)?
                != current_recipe.header.0
        {
            return Err(StorageError::ScopeChanged);
        }
        let current_slots = Self::validate_saved_slots(&current_recipe.slots, &current.sources)?;

        let history_recipe = self.recipe_contents(owner_generation, &current.key, true)?;
        let history_source_generation =
            crate::retention::source_generation(&history.sources)
                .map_err(|_| StorageError::ScopeChanged)?;
        let expected_history_generation = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_string(&(
                    crate::slot_projection::VERSION,
                    &history_source_generation,
                    &history_recipe.slots,
                    history_recipe.incomplete,
                ))
                .map_err(|_| StorageError::ScopeChanged)?
                .as_bytes()
            )
        );
        if history_recipe.header.1 != crate::slot_projection::VERSION
            || history_recipe.header.2.as_deref() != Some(history_source_generation.as_str())
            || history_recipe.header.0 != expected_history_generation
        {
            return Err(StorageError::ScopeChanged);
        }
        let history_slots = Self::validate_saved_slots(&history_recipe.slots, &history.sources)?;
        if current_slots
            .iter()
            .any(|(identity, text)| history_slots.get(identity) != Some(text))
        {
            return Err(StorageError::ScopeChanged);
        }

        let previous_sources = self.history_sources_at(owner_generation, &current.key)?;
        let history_by_path = history
            .sources
            .iter()
            .map(|source| (source.path.to_string_lossy().into_owned(), source))
            .collect::<BTreeMap<_, _>>();
        if previous_sources.iter().any(|(path, source)| {
            history_by_path.get(path).is_none_or(|expected| *expected != source)
        }) {
            return Err(StorageError::ScopeChanged);
        }
        let previous_proofs = self.proof_tuples_at(&[self.previous], &history.sources)?;
        let previous_generation = self.previous.as_bytes().to_vec();
        for source in &history.sources {
            if !current.sources.contains(source)
                && previous_sources.get(path(source)?) != Some(source)
            {
                return Err(StorageError::ScopeChanged);
            }
            if !previous_proofs.contains_key(&(
                previous_generation.clone(),
                path(source)?.as_bytes().to_vec(),
            )) {
                return Err(StorageError::ScopeChanged);
            }
        }
        Ok((history, history_recipe))
    }

    fn withdrawn_composite_complete(
        &self,
        key: &str,
        metadata: &str,
        history: &CatalogRow,
        recipe: &RecipeContents,
    ) -> Result<bool, StorageError> {
        let generation = self.effects.generation;
        if self.owner_at(generation, key)? != Some((generation.to_owned(), false))
            || self.metadata("semon_pg_native_catalog_history_headers", generation, key)?
                .as_deref()
                != Some(metadata)
            || !self.denormalized_history_matches(generation, generation, key, history)?
        {
            return Ok(false);
        }
        let Some((header, incomplete)) = self.recipe(generation, key, true)? else {
            return Ok(false);
        };
        let expected_header = (
            recipe.header.0.clone(),
            recipe.header.1,
            recipe.header.2.clone(),
            recipe.header.3,
        );
        if header != expected_header
            || incomplete != recipe.incomplete
            || self.recipe_slots_at(generation, key, true)? != recipe.slots
        {
            return Ok(false);
        }
        if self.has_original_header(generation, key)? {
            return Ok(false);
        }
        let expected_sources = history
            .sources
            .iter()
            .map(|source| (source.path.to_string_lossy().into_owned(), source))
            .collect::<std::collections::BTreeMap<_, _>>();
        let actual_sources = self.history_sources_at(generation, key)?;
        if actual_sources.len() != expected_sources.len()
            || expected_sources
                .iter()
                .any(|(path, source)| actual_sources.get(path).is_none_or(|actual| actual != *source))
        {
            return Ok(false);
        }
        let proofs = self.proof_tuples_at(&[self.previous, generation], &history.sources)?;
        let previous_generation = self.previous.as_bytes().to_vec();
        let successor_generation = generation.as_bytes().to_vec();
        for source in &history.sources {
            let path = path(source)?.as_bytes().to_vec();
            let previous = proofs.get(&(previous_generation.clone(), path.clone()));
            let successor = proofs.get(&(successor_generation.clone(), path));
            if previous.is_none() || previous != successor {
                return Ok(false);
            }
        }
        let source_generation = crate::retention::source_generation(&history.sources)
            .map_err(|_| StorageError::ScopeChanged)?;
        if !self.trace_family_matches(key, &source_generation)? {
            return Ok(false);
        }
        Ok(true)
    }

    fn preserve_withdrawn_composite(
        &self,
        current: &CatalogRow,
    ) -> Result<WithdrawnComposite, StorageError> {
        // `stage` writes the Current projection before `preserve`, and the
        // shared composer builds each non-original owner from that same row
        // and projection. Reuse is safe only after the checks below prove the
        // predecessor still contains that exact Current source/recipe subset.
        let Some((owner_generation, false)) = self.owner(&current.key)? else {
            return Ok(WithdrawnComposite::NotApplicable);
        };
        let metadata = self
            .metadata(
                "semon_pg_native_catalog_history_headers",
                &owner_generation,
                &current.key,
            )?
            .ok_or(StorageError::ScopeChanged)?;
        let (history, recipe) =
            self.validate_withdrawn_composite(current, &owner_generation, &metadata)?;
        if self.withdrawn_composite_complete(&current.key, &metadata, &history, &recipe)? {
            return Ok(WithdrawnComposite::Complete);
        }
        for source in &history.sources {
            self.retain_source(&history, source)?;
        }
        let source_generation = crate::retention::source_generation(&history.sources)
            .map_err(|_| StorageError::ScopeChanged)?;
        self.composed(
            &current.key,
            &recipe.header.0,
            &source_generation,
            &metadata,
            &recipe.slots,
            recipe.incomplete,
        )?;
        {
            let mut tx = self.effects.tx.borrow_mut();
            let owner = vec![(digest(current.key.as_bytes()), current.key.as_bytes().to_vec())];
            super::carry::trace_families(
                &mut tx,
                self.effects.generation,
                self.previous,
                &owner,
                Some(&source_generation),
            )?;
        }
        if !self.withdrawn_composite_complete(&current.key, &metadata, &history, &recipe)? {
            return Err(StorageError::ScopeChanged);
        }
        Ok(WithdrawnComposite::Staged)
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
        // Header columns mirror catalog_rows so retained history filters,
        // family traversal and keysets in SQL; the reader re-checks them
        // against the metadata.
        let row: CatalogRow =
            serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
        if row.key != key {
            return Err(StorageError::ScopeChanged);
        }
        let parent_key = row
            .parent
            .as_deref()
            .map(|parent| parent.as_bytes().to_vec());
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
                "parent_key",
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
                &parent_key,
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
    fn previous(&self, row: &CatalogRow) -> Result<Option<CatalogRow>, StorageError> {
        let previous = self.metadata("semon_pg_native_catalog_rows", self.previous, &row.key)?;
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
        Ok(old)
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
        self.write_header(&row.key, metadata, header, !complete, Some(original_header))?;
        let source_generation = crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::ScopeChanged)?;
        {
            // Current and trace effects are staged before publication calls
            // History preservation. An existing target header is the genuine
            // successor trace and must keep its own children.
            let mut tx = self.effects.tx.borrow_mut();
            let owner = vec![(digest(row.key.as_bytes()), row.key.as_bytes().to_vec())];
            super::carry::trace_families(
                &mut tx,
                self.effects.generation,
                self.previous,
                &owner,
                Some(&source_generation),
            )?;
        }
        if !complete {
            return Ok(());
        }
        let Some(query) = &row.query else {
            return Ok(());
        };
        if !crate::model::summary::query_observation_matches(query, &row.sources, false, None)
            .map_err(|_| StorageError::ScopeChanged)?
        {
            return Err(StorageError::ScopeChanged);
        }
        let mut tx = self.effects.tx.borrow_mut();
        let origin = snapshot::in_transaction(&mut tx, self.previous, |snapshot| {
            let (_, observations, origin) = snapshot
                .row_observation(row, Some(catalog::CatalogObservationOwner::Catalog))
                .map_err(|error| {
                    error
                        .get_ref()
                        .and_then(|error| error.downcast_ref::<StorageError>())
                        .copied()
                        .unwrap_or(StorageError::Unavailable)
                })?
                .ok_or(StorageError::ScopeChanged)?;
            let captured = observations
                .as_ref()
                .and_then(|observations| observations.runtime_for(row))
                .ok_or(StorageError::ScopeChanged)?;
            if query["runtime"].as_str() != Some(captured.as_str()) {
                return Err(StorageError::ScopeChanged);
            }
            Ok(origin)
        })?;
        match origin {
            observations::Origin::Native(receipt) => observations::issue_history(
                &mut tx,
                self.effects.generation,
                metadata.as_bytes(),
                row.lifecycle,
                receipt,
            ),
            observations::Origin::Imported(id) => {
                // Add custody without changing the earlier Current association.
                // This ID, root/presentation and destination import grant were
                // authenticated by the same reader above, inside this transaction.
                let changed = tx.driver.execute("INSERT INTO semon_pg_native_selected_refs(namespace,key_digest,session_key,kind,id) VALUES($1,sha256($2::bytea),$2,'history'::bytea,$3) ON CONFLICT(namespace_digest,key_digest,kind,id) DO UPDATE SET kind=excluded.kind WHERE semon_pg_native_selected_refs.namespace=excluded.namespace AND semon_pg_native_selected_refs.session_key=excluded.session_key", &[&tx.namespace.as_str().as_bytes(),&row.key.as_bytes(),&id.0.as_bytes()]).map_err(sqlstate::error)?;
                if changed != 1 {
                    return Err(StorageError::ScopeChanged);
                }
                tx.check()
            }
        }
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
    let continuations = publication
        .observations
        .and_then(crate::storage::selected::PresentationObservations::verified_continuations)
        .unwrap_or(&[]);
    let mut completed_composites = std::collections::BTreeSet::new();
    for row in &old {
        match store.preserve_withdrawn_composite(row)? {
            WithdrawnComposite::NotApplicable => {}
            WithdrawnComposite::Complete => {
                completed_composites.insert(row.key.as_str());
            }
            WithdrawnComposite::Staged => {
                // One composite is staged per attempt. Its complete receipt
                // is the private progress witness for the deterministic
                // successor generation; Current and the withdrawal CAS wait.
                return Ok(false);
            }
        }
    }
    let original_withdrawn = old
        .iter()
        .filter(|row| !completed_composites.contains(row.key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !catalog::preserve_rows(
        &store,
        publication.rows,
        &original_withdrawn,
        continuations,
    )? {
        return Ok(false);
    }
    catalog::publish_history_rows(&store, publication.rows, publication.transcripts)?;
    Ok(true)
}

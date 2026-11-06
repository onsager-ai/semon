//! Catalog and recipe snapshots. These APIs do not construct a served model or
//! authorize native bytes; selected renderers must keep this transaction alive.
use super::{
    Namespace, PgProjectionTransaction, ProjectionTag, StorageError, TagCheck, backend, ensure,
    number,
};
use crate::{
    CatalogReadScope,
    model::summary::{CatalogLifecycle, CatalogRow},
    slot_projection::{Publication, SavedSlot},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_METADATA: usize = 1024 * 1024;

pub(crate) struct CatalogPage {
    pub(crate) generation: String,
    pub(crate) rows: Vec<CatalogRow>,
    pub(crate) next: Option<(i64, String)>,
}

impl PgProjectionTransaction<'_> {
    pub(crate) async fn catalog_generation(
        &self,
        namespace: &Namespace,
    ) -> Result<Option<String>, StorageError> {
        ensure(&self.namespace, namespace)?;
        Ok(self
            .tx
            .query_opt(
                "SELECT generation FROM semon_projection.catalog_heads WHERE namespace=$1",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?
            .map(|row| row.get(0)))
    }

    /// A complete bounded component is replaced under Catalog and source CAS.
    /// Unrelated components remain intact. Serialize and qualify the whole batch
    /// before writing, so a refused operation has no committable partial effects.
    pub(crate) async fn publish_catalog(
        &self,
        namespace: &Namespace,
        publication: &Publication<'_>,
        expected_generation: Option<&str>,
    ) -> Result<Option<String>, StorageError> {
        ensure(&self.namespace, namespace)?;
        if publication.rows.is_empty() || publication.rows.len() > 64 {
            return Err(StorageError::Backend(
                "catalog component budget exceeded".into(),
            ));
        }
        self.tx
            .query_one(
                "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
        if self.catalog_generation(namespace).await?.as_deref() != expected_generation {
            return Ok(None);
        }
        let mut keys = BTreeSet::new();
        let mut encoded = Vec::new();
        let mut tags = BTreeMap::new();
        let mut bytes = 0usize;
        for row in publication.rows {
            if !keys.insert(row.key.as_str())
                || row.lifecycle != CatalogLifecycle::Current
                || row.sources.is_empty()
                || row.sources.len() > 64
            {
                return Err(StorageError::Backend("invalid catalog component".into()));
            }
            let metadata = serde_json::to_string(row).map_err(backend)?;
            bytes += metadata.len();
            if bytes > MAX_METADATA {
                return Err(StorageError::Backend(
                    "catalog component byte budget exceeded".into(),
                ));
            }
            for source in &row.sources {
                let path = source
                    .path
                    .to_str()
                    .ok_or_else(|| backend("invalid native path"))?;
                let Some(tag) = self.ledger(namespace, path).await? else {
                    return Ok(None);
                };
                if tag
                    != ProjectionTag::of(source, tag.epoch, crate::model::summary::CATALOG_VERSION)
                {
                    return Ok(None);
                }
                tags.insert((row.key.as_str(), path), tag);
            }
            encoded.push(metadata);
        }
        let mut recipes = BTreeMap::new();
        for projection in publication.transcripts.unwrap_or_default() {
            let Some(row) = publication
                .rows
                .iter()
                .find(|row| row.key == projection.key)
            else {
                return Err(backend("recipe owner missing from publication"));
            };
            let paths: BTreeSet<_> = row.sources.iter().map(|source| &source.path).collect();
            let mut slots = Vec::new();
            for slot in &projection.slots {
                if slot.source_paths().iter().any(|path| !paths.contains(path)) {
                    return Err(backend("recipe source missing from publication"));
                }
                let text = serde_json::to_string(slot).map_err(backend)?;
                bytes += text.len();
                if bytes > MAX_METADATA {
                    return Err(backend("catalog component byte budget exceeded"));
                }
                slots.push(text);
            }
            if recipes.insert(projection.key.as_str(), slots).is_some() {
                return Err(backend("duplicate recipe owner"));
            }
        }
        // A failed source check above leaves the old Catalog and recipes intact.
        // Append publication advances History with Current. A replaced Raw prefix
        // first preserves the outgoing observation independently of the new one.
        for row in publication.rows {
            let previous = self.tx.query_opt(
                "SELECT metadata FROM semon_projection.catalog WHERE namespace=$1 AND session_key=$2",
                &[&namespace.binding(), &row.key],
            ).await.map_err(backend)?;
            if let Some(previous) = previous {
                let previous: CatalogRow =
                    serde_json::from_str(previous.get(0)).map_err(backend)?;
                let mut replaced = false;
                for source in &previous.sources {
                    let path = source
                        .path
                        .to_str()
                        .ok_or_else(|| backend("invalid native path"))?;
                    replaced |= matches!(
                        self.check_tag(namespace, &row.key, path).await?,
                        None | Some(TagCheck::Mismatch(_))
                    );
                }
                if replaced {
                    self.preserve_history(namespace, &row.key).await?;
                }
            }
        }
        let mut hash = Sha256::new();
        hash.update(expected_generation.unwrap_or_default());
        hash.update(serde_json::to_vec(&encoded).map_err(backend)?);
        hash.update(serde_json::to_vec(&recipes).map_err(backend)?);
        let generation = format!("{:x}", hash.finalize());
        for (row, metadata) in publication.rows.iter().zip(&encoded) {
            self.tx.execute(
                "INSERT INTO semon_projection.catalog(namespace,session_key,lifecycle,last_ms,harness,repo,model,metadata) VALUES($1,$2,'current',$3,$4,$5,$6,$7) ON CONFLICT(namespace,session_key) DO UPDATE SET lifecycle='current',last_ms=excluded.last_ms,harness=excluded.harness,repo=excluded.repo,model=excluded.model,metadata=excluded.metadata",
                &[&namespace.binding(), &row.key, &row.last.unwrap_or(0), &row.harness, &row.repo.as_deref().unwrap_or(""), &row.model, metadata],
            ).await.map_err(backend)?;
            self.tx
                .execute(
                    "DELETE FROM semon_projection.units WHERE namespace=$1 AND unit_key=$2",
                    &[&namespace.binding(), &row.key],
                )
                .await
                .map_err(backend)?;
            for source in &row.sources {
                let path = source
                    .path
                    .to_str()
                    .ok_or_else(|| backend("invalid native path"))?;
                self.publish_tag(namespace, &row.key, path, tags[&(row.key.as_str(), path)])
                    .await?;
            }
            self.tx
                .execute(
                    "DELETE FROM semon_projection.recipes WHERE namespace=$1 AND session_key=$2",
                    &[&namespace.binding(), &row.key],
                )
                .await
                .map_err(backend)?;
            self.tx.execute("DELETE FROM semon_projection.recipe_heads WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &row.key]).await.map_err(backend)?;
            if let Some(slots) = recipes.get(row.key.as_str()) {
                let recipe_generation =
                    crate::slot_projection::generation(slots, &row.sources).map_err(backend)?;
                self.tx.execute(
                    "INSERT INTO semon_projection.recipe_heads(namespace,session_key,generation,source_generation,version,total) VALUES($1,$2,$3,$4,$5,$6)",
                    &[&namespace.binding(), &row.key, &recipe_generation, &crate::retention::source_generation(&row.sources).map_err(backend)?, &(crate::slot_projection::VERSION as i32), &number(slots.len() as u64)?],
                ).await.map_err(backend)?;
                let ordinals = (0..slots.len())
                    .map(|at| number(at as u64))
                    .collect::<Result<Vec<_>, _>>()?;
                self.tx.execute(
                    "INSERT INTO semon_projection.recipes(namespace,session_key,slot,metadata) SELECT $1,$2,r.* FROM unnest($3::bigint[],$4::text[]) r",
                    &[&namespace.binding(), &row.key, &ordinals, slots],
                ).await.map_err(backend)?;
            }
        }
        for row in publication.rows {
            self.publish_search(namespace, row).await?;
            self.catalog_changed(namespace, &row.key).await?;
        }
        self.tx.execute(
            "INSERT INTO semon_projection.catalog_heads(namespace,generation) VALUES($1,$2) ON CONFLICT(namespace) DO UPDATE SET generation=excluded.generation",
            &[&namespace.binding(), &generation],
        ).await.map_err(backend)?;
        Ok(Some(generation))
    }

    /// Capture outgoing History and its recipes in the same transaction as
    /// replacement or withdrawal. History never checks a newer Raw ledger.
    async fn preserve_history(&self, namespace: &Namespace, key: &str) -> Result<(), StorageError> {
        self.tx.execute(
            "INSERT INTO semon_projection.history_catalog SELECT * FROM semon_projection.catalog WHERE namespace=$1 AND session_key=$2 ON CONFLICT(namespace,session_key) DO NOTHING",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        self.tx.execute(
            "INSERT INTO semon_projection.history_recipe_heads SELECT * FROM semon_projection.recipe_heads WHERE namespace=$1 AND session_key=$2 ON CONFLICT(namespace,session_key) DO NOTHING",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        self.tx.execute(
            "INSERT INTO semon_projection.history_recipes SELECT r.* FROM semon_projection.recipes r JOIN semon_projection.recipe_heads c USING(namespace,session_key) JOIN semon_projection.history_recipe_heads h USING(namespace,session_key) WHERE r.namespace=$1 AND r.session_key=$2 AND c.generation=h.generation ON CONFLICT(namespace,session_key,slot) DO NOTHING",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        Ok(())
    }

    pub(crate) async fn catalog_row(
        &self,
        namespace: &Namespace,
        key: &str,
        scope: CatalogReadScope,
    ) -> Result<Option<CatalogRow>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let table = if scope == CatalogReadScope::RetainedHistory {
            "history_catalog"
        } else {
            "catalog"
        };
        let row = self.tx.query_opt(
            &format!("SELECT metadata,lifecycle FROM semon_projection.{table} WHERE namespace=$1 AND session_key=$2 AND octet_length(metadata)<=$3"),
            &[&namespace.binding(), &key, &(MAX_METADATA as i32)],
        ).await.map_err(backend)?;
        // Until an outgoing observation exists, History is the current prefix
        // and must pass the same tag check as Current.
        let captured_history = scope == CatalogReadScope::RetainedHistory && row.is_some();
        let row = match row {
            Some(row) => Some(row),
            None if scope == CatalogReadScope::RetainedHistory => self.tx.query_opt(
                "SELECT metadata,lifecycle FROM semon_projection.catalog WHERE namespace=$1 AND session_key=$2 AND octet_length(metadata)<=$3",
                &[&namespace.binding(), &key, &(MAX_METADATA as i32)],
            ).await.map_err(backend)?,
            None => None,
        };
        let Some(row) = row else { return Ok(None) };
        let lifecycle: &str = row.get(1);
        if scope == CatalogReadScope::Current && lifecycle != "current" {
            return Ok(None);
        }
        let mut row: CatalogRow = serde_json::from_str(row.get(0)).map_err(backend)?;
        if scope == CatalogReadScope::Current {
            if row.lifecycle != CatalogLifecycle::Current {
                return Err(backend("catalog lifecycle mismatch"));
            }
            self.qualify_catalog_row(namespace, &row).await?;
        } else {
            if !captured_history && row.lifecycle == CatalogLifecycle::Current {
                self.qualify_catalog_row(namespace, &row).await?;
            }
            row.lifecycle = CatalogLifecycle::Retained;
        }
        Ok(Some(row))
    }

    async fn qualify_catalog_row(
        &self,
        namespace: &Namespace,
        row: &CatalogRow,
    ) -> Result<(), StorageError> {
        for source in &row.sources {
            let path = source
                .path
                .to_str()
                .ok_or_else(|| backend("invalid native path"))?;
            let published = self.tx.query_opt(
                "SELECT dev,ino,consumed,prefix,tail,epoch,projector FROM semon_projection.units WHERE namespace=$1 AND unit_key=$2 AND path=$3",
                &[&namespace.binding(), &row.key, &path],
            ).await.map_err(backend)?.ok_or_else(|| backend("projection_stale"))?;
            let published = super::decode(&published)?;
            if published
                != ProjectionTag::of(
                    source,
                    published.epoch,
                    crate::model::summary::CATALOG_VERSION,
                )
            {
                return Err(backend("projection_stale"));
            }
            match self.check_tag(namespace, &row.key, path).await? {
                Some(TagCheck::Current | TagCheck::Advanced | TagCheck::Relocated) => {}
                _ => return Err(backend("projection_stale")),
            }
        }
        Ok(())
    }

    /// Bounded keyset metadata read. Filter and search adoption by the existing
    /// viewer port is separate; this returns the shared typed publication rows.
    pub(crate) async fn catalog_page(
        &self,
        namespace: &Namespace,
        scope: CatalogReadScope,
        after: Option<(i64, &str)>,
        limit: usize,
    ) -> Result<CatalogPage, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !(1..=100).contains(&limit) {
            return Err(backend("invalid catalog page limit"));
        }
        let generation = self
            .catalog_generation(namespace)
            .await?
            .ok_or_else(|| backend("catalog not published"))?;
        let (last, key) = after.unwrap_or((i64::MAX, ""));
        let selected = self.tx.query(
            "SELECT session_key,last_ms FROM semon_projection.catalog WHERE namespace=$1 AND ($2 OR lifecycle='current') AND (last_ms<$3 OR (last_ms=$3 AND session_key>$4)) ORDER BY last_ms DESC,session_key LIMIT $5",
            &[&namespace.binding(), &(scope == CatalogReadScope::RetainedHistory), &last, &key, &number(limit as u64 + 1)?],
        ).await.map_err(backend)?;
        let more = selected.len() > limit;
        let mut rows = Vec::new();
        for item in selected.iter().take(limit) {
            let key: &str = item.get(0);
            rows.push(
                self.catalog_row(namespace, key, scope)
                    .await?
                    .ok_or_else(|| backend("catalog row unavailable"))?,
            );
        }
        let next = more.then(|| {
            let row = &selected[limit - 1];
            (row.get(1), row.get(0))
        });
        Ok(CatalogPage {
            generation,
            rows,
            next,
        })
    }

    /// Selected recipe window from this same metadata/proof snapshot. The
    /// generation is independent of unrelated Catalog publications.
    pub(crate) async fn recipe_page(
        &self,
        namespace: &Namespace,
        key: &str,
        scope: CatalogReadScope,
        expected: Option<&str>,
        after: usize,
        limit: usize,
    ) -> Result<(String, usize, Vec<SavedSlot>), StorageError> {
        ensure(&self.namespace, namespace)?;
        if !(1..=100).contains(&limit) {
            return Err(backend("invalid recipe page limit"));
        }
        let row = self
            .catalog_row(namespace, key, scope)
            .await?
            .ok_or_else(|| backend("selected identity unavailable"))?;
        let history = scope == CatalogReadScope::RetainedHistory && self.tx.query_opt("SELECT 1 FROM semon_projection.history_catalog WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?.is_some();
        let (heads, slots) = if history {
            ("history_recipe_heads", "history_recipes")
        } else {
            ("recipe_heads", "recipes")
        };
        let header = self.tx.query_opt(
            &format!("SELECT generation,source_generation,version,total FROM semon_projection.{heads} WHERE namespace=$1 AND session_key=$2"),
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?.ok_or_else(|| backend("selected recipes unavailable"))?;
        let generation: String = header.get(0);
        let total = usize::try_from(header.get::<_, i64>(3)).map_err(backend)?;
        if header.get::<_, i32>(2) != crate::slot_projection::VERSION as i32
            || header.get::<_, String>(1)
                != crate::retention::source_generation(&row.sources).map_err(backend)?
            || expected.is_some_and(|expected| expected != generation)
        {
            return Err(backend("selected recipe generation mismatch"));
        }
        let records = self.tx.query(
            &format!("SELECT slot,metadata FROM semon_projection.{slots} WHERE namespace=$1 AND session_key=$2 AND slot>=$3 AND slot<$4 AND octet_length(metadata)<=$5 ORDER BY slot"),
            &[&namespace.binding(), &key, &number(after as u64)?, &number(after.saturating_add(limit) as u64)?, &(MAX_METADATA as i32)],
        ).await.map_err(backend)?;
        if records.len() != total.saturating_sub(after).min(limit) {
            return Err(backend("selected recipe sequence gap"));
        }
        let mut decoded = Vec::new();
        let mut bytes = 0;
        for (index, record) in records.iter().enumerate() {
            let text: &str = record.get(1);
            bytes += text.len();
            if bytes > MAX_METADATA || record.get::<_, i64>(0) != number((after + index) as u64)? {
                return Err(backend("selected recipe budget or sequence mismatch"));
            }
            let slot: SavedSlot = serde_json::from_str(text).map_err(backend)?;
            if slot
                .source_paths()
                .iter()
                .any(|path| !row.sources.iter().any(|source| &source.path == *path))
            {
                return Err(backend("selected recipe source mismatch"));
            }
            decoded.push(slot);
        }
        Ok((generation, total, decoded))
    }

    /// A host must commit this handoff before withdrawing Raw. It does not
    /// delete bytes or grant permission to do so. An incomplete/unknown owner
    /// refuses rather than turning unavailable History into an empty success.
    pub(crate) async fn withdraw_source(
        &self,
        namespace: &Namespace,
        path: &str,
        expected: ProjectionTag,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        self.tx
            .query_one(
                "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
        if self.ledger(namespace, path).await? != Some(expected) {
            return Ok(false);
        }
        let owners = self.tx.query("SELECT DISTINCT unit_key FROM semon_projection.units WHERE namespace=$1 AND path=$2 ORDER BY unit_key LIMIT 65", &[&namespace.binding(), &path]).await.map_err(backend)?;
        if owners.is_empty() || owners.len() > 64 {
            return Ok(false);
        }
        for owner in &owners {
            let key: &str = owner.get(0);
            let (_, total, _) = self
                .recipe_page(namespace, key, CatalogReadScope::Current, None, 0, 1)
                .await?;
            let count: i64 = self.tx.query_one(
                "SELECT count(*) FROM semon_projection.recipes WHERE namespace=$1 AND session_key=$2 AND slot<$3",
                &[&namespace.binding(), &key, &number(total as u64)?],
            ).await.map_err(backend)?.get(0);
            if count != number(total as u64)? {
                return Ok(false);
            }
            // Composite History needs a later merge implementation. Never
            // overwrite an older custody owner or withdraw a newer uncaptured one.
            let conflicting: bool = self.tx.query_one(
                "SELECT EXISTS(SELECT 1 FROM semon_projection.history_recipe_heads h JOIN semon_projection.recipe_heads c USING(namespace,session_key) WHERE h.namespace=$1 AND h.session_key=$2 AND h.source_generation<>c.source_generation)",
                &[&namespace.binding(), &key],
            ).await.map_err(backend)?.get(0);
            if conflicting {
                return Ok(false);
            }
        }
        for owner in &owners {
            let key: &str = owner.get(0);
            self.preserve_history(namespace, key).await?;
            self.catalog_changed(namespace, key).await?;
            self.tx.execute("UPDATE semon_projection.catalog SET lifecycle='retained' WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?;
        }
        self.tx
            .execute(
                "DELETE FROM semon_projection.files WHERE namespace=$1 AND path=$2",
                &[&namespace.binding(), &path],
            )
            .await
            .map_err(backend)?;
        self.tx
            .execute(
                "DELETE FROM semon_projection.ledgers WHERE namespace=$1 AND path=$2",
                &[&namespace.binding(), &path],
            )
            .await
            .map_err(backend)?;
        Ok(true)
    }
}

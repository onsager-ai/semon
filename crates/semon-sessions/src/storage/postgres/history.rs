//! Transactional retained observations. Every outgoing generation keeps its own
//! source cohort; a reused path never silently rebinds an older recipe.
use super::{Namespace, PgProjectionTransaction, StorageError, backend, number};
use crate::{
    model::summary::{CatalogLifecycle, CatalogRow},
    slot_projection::SavedSlot,
};
use std::collections::BTreeSet;

impl PgProjectionTransaction<'_> {
    pub(super) async fn preserve_history(
        &self,
        namespace: &Namespace,
        key: &str,
    ) -> Result<(), StorageError> {
        let current = self.tx.query_opt(
            "SELECT metadata FROM semon_projection.catalog WHERE namespace=$1 AND session_key=$2",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        let Some(current) = current else {
            return Ok(());
        };
        let mut row: CatalogRow = serde_json::from_str(current.get(0)).map_err(backend)?;
        let current_sources = row.sources.clone();
        let header = self.tx.query_opt(
            "SELECT generation,source_generation,version,total FROM semon_projection.recipe_heads WHERE namespace=$1 AND session_key=$2",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?.ok_or_else(|| backend("outgoing recipes unavailable"))?;
        if header.get::<_, i32>(2) != crate::slot_projection::VERSION as i32
            || header.get::<_, String>(1)
                != crate::retention::source_generation(&row.sources).map_err(backend)?
        {
            return Err(backend("outgoing recipe generation mismatch"));
        }
        let current_observation = self
            .current_observation(namespace, key, row.sources.clone())
            .await?;
        let observation = serde_json::to_string(&current_observation).map_err(backend)?;
        let mut slots = self
            .history_slots(namespace, key, false, header.get(3))
            .await?
            .into_iter()
            .map(|slot| (slot, observation.clone()))
            .collect::<Vec<_>>();
        let prior = self.tx.query_opt(
            "SELECT metadata FROM semon_projection.history_catalog WHERE namespace=$1 AND session_key=$2",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        if let Some(prior) = prior {
            let prior: CatalogRow = serde_json::from_str(prior.get(0)).map_err(backend)?;
            if prior.harness != row.harness {
                // As on SQLite, a reused viewer key cannot join harness identities.
                return Ok(());
            }
            let prior_header = self.tx.query_one(
                "SELECT source_generation,version,total FROM semon_projection.history_recipe_heads WHERE namespace=$1 AND session_key=$2",
                &[&namespace.binding(), &key],
            ).await.map_err(backend)?;
            if prior_header.get::<_, i32>(1) != crate::slot_projection::VERSION as i32
                || prior_header.get::<_, String>(0)
                    != crate::retention::source_generation(&prior.sources).map_err(backend)?
            {
                return Err(backend("retained recipe generation mismatch"));
            }
            let previous = self
                .history_slots(namespace, key, true, prior_header.get(2))
                .await?;
            let observations = self.tx.query("SELECT observation FROM semon_projection.history_recipes WHERE namespace=$1 AND session_key=$2 ORDER BY slot", &[&namespace.binding(), &key]).await.map_err(backend)?;
            let observations = observations
                .iter()
                .map(|record| {
                    serde_json::from_str::<super::observation::Observation>(record.get(0))
                        .map_err(backend)
                })
                .collect::<Result<Vec<_>, _>>()?;
            for retained in &observations {
                if current_observation.covers(retained) {
                    continue;
                }
                for source in &retained.sources {
                    if !row.sources.contains(source) {
                        row.sources.push(source.clone());
                    }
                }
            }
            // Deduplicate only within the same consumed generation. Offsets and
            // native IDs can be reused after replacement at the very same path.
            let same_sources = row.sources == current_sources;
            let mut identities: BTreeSet<_> = slots
                .iter()
                .map(|(slot, observation)| (slot.identity(), observation.clone()))
                .collect();
            for (slot, retained) in previous.into_iter().zip(observations) {
                if current_observation.covers(&retained) {
                    continue;
                }
                let observation = serde_json::to_string(&retained).map_err(backend)?;
                if identities.insert((slot.identity(), observation.clone())) {
                    let at = slot
                        .t
                        .and_then(|time| {
                            slots
                                .iter()
                                .position(|(next, _)| next.t.is_some_and(|t| t > time))
                        })
                        .unwrap_or(slots.len());
                    slots.insert(at, (slot, observation));
                }
            }
            if !same_sources {
                // Composite usage and Query facts require derivation from the
                // complete native cohort, not a sum of overlapping summaries.
                row.query = None;
                row.activity_analytics = None;
            }
        }
        if row.sources.len() > 64 || slots.len() > 4096 {
            return Err(backend("retained source or recipe budget exceeded"));
        }
        row.lifecycle = CatalogLifecycle::Retained;
        row.native_ids = row
            .sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let texts = slots
            .iter()
            .map(|(slot, _)| serde_json::to_string(slot))
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        let observations: Vec<_> = slots
            .iter()
            .map(|(_, observation)| observation.clone())
            .collect();
        let metadata = serde_json::to_string(&row).map_err(backend)?;
        if metadata.len()
            + texts.iter().map(String::len).sum::<usize>()
            + observations.iter().map(String::len).sum::<usize>()
            > 1024 * 1024
        {
            return Err(backend("retained component byte budget exceeded"));
        }
        let generation =
            crate::slot_projection::generation(&texts, &row.sources).map_err(backend)?;
        self.tx.execute(
            "INSERT INTO semon_projection.history_catalog(namespace,session_key,lifecycle,last_ms,harness,repo,model,metadata) VALUES($1,$2,'retained',$3,$4,$5,$6,$7) ON CONFLICT(namespace,session_key) DO UPDATE SET lifecycle='retained',last_ms=excluded.last_ms,harness=excluded.harness,repo=excluded.repo,model=excluded.model,metadata=excluded.metadata",
            &[&namespace.binding(), &key, &row.last.unwrap_or(0), &row.harness, &row.repo.as_deref().unwrap_or(""), &row.model, &metadata],
        ).await.map_err(backend)?;
        self.tx.execute(
            "INSERT INTO semon_projection.history_recipe_heads(namespace,session_key,generation,source_generation,version,total) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(namespace,session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,version=excluded.version,total=excluded.total",
            &[&namespace.binding(), &key, &generation, &crate::retention::source_generation(&row.sources).map_err(backend)?, &(crate::slot_projection::VERSION as i32), &number(texts.len() as u64)?],
        ).await.map_err(backend)?;
        self.tx.execute("DELETE FROM semon_projection.history_recipes WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?;
        let ordinals = (0..texts.len())
            .map(|at| number(at as u64))
            .collect::<Result<Vec<_>, _>>()?;
        self.tx.execute(
            "INSERT INTO semon_projection.history_recipes(namespace,session_key,slot,metadata,observation) SELECT $1,$2,r.* FROM unnest($3::bigint[],$4::text[],$5::text[]) r",
            &[&namespace.binding(), &key, &ordinals, &texts, &observations],
        ).await.map_err(backend)?;
        Ok(())
    }

    pub(super) async fn recipe_observations(
        &self,
        namespace: &Namespace,
        key: &str,
        scope: crate::CatalogReadScope,
        after: usize,
        limit: usize,
    ) -> Result<Vec<Vec<crate::model::summary::CatalogSource>>, StorageError> {
        let history = scope == crate::CatalogReadScope::RetainedHistory && self.tx.query_opt(
            "SELECT 1 FROM semon_projection.history_catalog WHERE namespace=$1 AND session_key=$2",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?.is_some();
        if !history {
            let row = self
                .catalog_row(namespace, key, scope)
                .await?
                .ok_or_else(|| backend("selected owner unavailable"))?;
            return Ok(vec![row.sources; limit]);
        }
        let records = self.tx.query(
            "SELECT CASE WHEN sum(octet_length(observation)) OVER (ORDER BY slot)<=1048576 THEN observation END FROM semon_projection.history_recipes WHERE namespace=$1 AND session_key=$2 AND slot>=$3 AND slot<$4 ORDER BY slot",
            &[&namespace.binding(), &key, &number(after as u64)?, &number(after.saturating_add(limit) as u64)?],
        ).await.map_err(backend)?;
        records
            .iter()
            .map(|record| {
                let text: Option<&str> = record.get(0);
                serde_json::from_str::<super::observation::Observation>(
                    text.ok_or_else(|| backend("selected observation budget exceeded"))?,
                )
                .map(|observation| observation.sources)
                .map_err(backend)
            })
            .collect()
    }

    async fn history_slots(
        &self,
        namespace: &Namespace,
        key: &str,
        history: bool,
        total: i64,
    ) -> Result<Vec<SavedSlot>, StorageError> {
        let table = if history {
            "history_recipes"
        } else {
            "recipes"
        };
        let records = self.tx.query(
            &format!("SELECT slot,CASE WHEN sum(octet_length(metadata)) OVER (ORDER BY slot)<=1048576 THEN metadata END FROM semon_projection.{table} WHERE namespace=$1 AND session_key=$2 ORDER BY slot LIMIT 4097"),
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        if total < 0 || records.len() > 4096 || records.len() as i64 != total {
            return Err(backend("retained recipe sequence or budget mismatch"));
        }
        records
            .iter()
            .enumerate()
            .map(|(at, record)| {
                if record.get::<_, i64>(0) != at as i64 {
                    return Err(backend("retained recipe sequence gap"));
                }
                let text: Option<&str> = record.get(1);
                serde_json::from_str(
                    text.ok_or_else(|| backend("retained recipe byte budget exceeded"))?,
                )
                .map_err(backend)
            })
            .collect()
    }
}

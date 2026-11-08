//! Source-bound activity publication and metadata-only selected pages.
use super::{Namespace, PgProjectionStore, PgProjectionTransaction, StorageError, backend};
use crate::{CatalogReadScope, Options, SessionSourceReader};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl PgProjectionTransaction<'_> {
    pub(super) async fn publish_activity(
        &self,
        namespace: &Namespace,
        row: &crate::model::summary::CatalogRow,
    ) -> Result<(), StorageError> {
        self.tx.execute("DELETE FROM semon_projection.session_activity WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &row.key]).await.map_err(backend)?;
        let Some(projection) = &row.session_activity else {
            return Ok(());
        };
        let activity = &projection.activity;
        if activity.session_key != row.key
            || activity.rules_version != crate::events::session_activity::ACTIVITY_RULES_VERSION
        {
            return Err(backend("activity owner or rules mismatch"));
        }
        let sources = crate::retention::source_generation(&row.sources).map_err(backend)?;
        let mut header = activity.clone();
        header.steps.clear();
        header.facts.clear();
        header.compactions.clear();
        let metadata = serde_json::to_string(&header).map_err(backend)?;
        if metadata.len() > 1048576 {
            return Err(backend("activity publication byte budget exceeded"));
        }
        let generation = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&sources, activity)).map_err(backend)?)
        );
        self.tx.execute("INSERT INTO semon_projection.session_activity(namespace,session_key,source_generation,generation,metadata) VALUES($1,$2,$3,$4,$5)", &[&namespace.binding(), &row.key, &sources, &generation, &metadata]).await.map_err(backend)?;
        let steps = activity
            .steps
            .iter()
            .map(|step| {
                let mut item = serde_json::to_value(step).map_err(backend)?;
                item.as_object_mut()
                    .expect("typed activity step")
                    .remove("position");
                Ok(item)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let mut facts = activity.facts.iter().collect::<Vec<_>>();
        facts.sort_by(|a, b| a.identity.cmp(&b.identity));
        let facts = facts.iter().map(|fact| json!({"identity":fact.identity,"timestamp":fact.timestamp,"step_id":fact.step_id,"change":fact.change})).collect::<Vec<_>>();
        for (kind, items) in [("steps", steps), ("facts", facts)] {
            let mut positions = Vec::new();
            let mut identities = Vec::new();
            let mut texts = Vec::new();
            for (position, item) in items.iter().enumerate() {
                positions.push(super::number(position as u64)?);
                identities.push(if kind == "facts" {
                    item["identity"]
                        .as_str()
                        .ok_or_else(|| backend("activity fact identity unavailable"))?
                        .to_owned()
                } else {
                    position.to_string()
                });
                let text = item.to_string();
                if text.len() > 1048576 {
                    return Err(backend("activity item budget exceeded"));
                }
                texts.push(text);
            }
            self.tx.execute("INSERT INTO semon_projection.activity_items(namespace,session_key,kind,position,identity,metadata) SELECT $1,$2,$3,r.* FROM unnest($4::bigint[],$5::text[],$6::text[]) r", &[&namespace.binding(), &row.key, &kind, &positions, &identities, &texts]).await.map_err(backend)?;
        }
        Ok(())
    }
}

impl PgProjectionStore {
    pub(crate) async fn activity_target(
        &mut self,
        namespace: &Namespace,
        requested: &str,
    ) -> Result<Option<String>, StorageError> {
        let tx = self.begin(namespace).await?;
        let keys = tx.tx.query("SELECT session_key FROM semon_projection.catalog WHERE namespace=$1 AND lifecycle='current' AND (session_key=$2 OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(metadata::jsonb->'native_ids') id WHERE id=$2)) ORDER BY session_key LIMIT 2", &[&namespace.binding(), &requested]).await.map_err(backend)?;
        if keys.len() > 1 {
            return Err(backend("ambiguous activity identity"));
        }
        let key = keys.first().map(|row| row.get(0));
        tx.commit(namespace).await?;
        Ok(key)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn activity_query(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        key: &str,
        args: &Value,
        provider: Option<&dyn SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
        now: i64,
    ) -> Result<Value, StorageError> {
        if args["id"].as_str() != Some(key) {
            return Err(backend("activity identity mismatch"));
        }
        if args["cursor"].is_string() && !args["generation"].is_string() {
            return Err(backend("activity continuation requires generation"));
        }
        let _facts_lock =
            crate::source_change::SourceLock::try_shared(options, "facts").map_err(backend)?;
        let tx = self.begin(namespace).await?;
        let row = tx
            .catalog_row(namespace, key, CatalogReadScope::Current)
            .await?
            .ok_or_else(|| backend("activity Current owner unavailable"))?;
        if row.sources.iter().any(|source| {
            crate::catalog::source_relative(options, &row.harness, source).is_none()
                || !crate::model::catalog_source_in_window(
                    source,
                    crate::model::scan_cutoff(options, now),
                )
        }) {
            return Err(backend("activity source authority or window changed"));
        }
        let _source_locks = row
            .sources
            .iter()
            .map(|source| {
                crate::source_change::SourceLock::try_shared(
                    options,
                    &source.path.to_string_lossy(),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        let record = tx.tx.query_opt("SELECT source_generation,generation,metadata FROM semon_projection.session_activity WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?.ok_or_else(|| backend("activity projection unavailable"))?;
        let source_generation: String = record.get(0);
        let generation: String = record.get(1);
        let activity: crate::events::session_activity::SessionActivity =
            serde_json::from_str(record.get(2)).map_err(backend)?;
        if source_generation
            != crate::retention::source_generation(&row.sources).map_err(backend)?
            || activity.session_key != key
            || activity.rules_version != crate::events::session_activity::ACTIVITY_RULES_VERSION
            || activity.pricing_fingerprint != crate::pricing::snapshot().fingerprint
            || args["generation"]
                .as_str()
                .is_some_and(|expected| expected != generation)
        {
            return Err(backend("activity generation changed"));
        }
        if !options.all && !options.scan_window {
            let cutoff =
                now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX));
            if row.last.is_none_or(|last| last < cutoff)
                && !matches!(
                    row.query.as_ref().and_then(|query| query["state"].as_str()),
                    Some("work" | "wait")
                )
            {
                return Err(backend("activity outside query window"));
            }
        }
        let mut proofs = Vec::new();
        for source in &row.sources {
            let (ledger, index) = tx
                .load_index(
                    namespace,
                    source
                        .path
                        .to_str()
                        .ok_or_else(|| backend("invalid activity path"))?,
                )
                .await?
                .ok_or_else(|| backend("activity index unavailable"))?;
            if ledger != crate::source_carrier::ledger(source) {
                return Err(backend("activity index generation changed"));
            }
            proofs.push((source.clone(), index.source_chunks));
        }
        let proofs =
            crate::query_source::Sources::capture(options, proofs.into_iter()).map_err(backend)?;
        let reader = proofs.reader_with_mode(provider, mode);
        reader.verify_all().map_err(backend)?;
        let limit = args["limit"].as_u64().unwrap_or(100).min(256) as usize;
        let cursor = args["cursor"].as_str();
        let facts = args["kind"] == "facts";
        if limit == 0 {
            return Err(backend("invalid activity limit"));
        }
        let records = if facts {
            tx.tx.query("SELECT identity,metadata FROM semon_projection.activity_items WHERE namespace=$1 AND session_key=$2 AND kind='facts' AND identity>$3 ORDER BY identity LIMIT $4", &[&namespace.binding(), &key, &cursor.unwrap_or(""), &super::number(limit as u64 + 1)?]).await.map_err(backend)?
        } else {
            let start = cursor
                .map(str::parse::<u64>)
                .transpose()
                .map_err(backend)?
                .unwrap_or(0);
            let total: i64 = tx.tx.query_one("SELECT count(*) FROM semon_projection.activity_items WHERE namespace=$1 AND session_key=$2 AND kind='steps'", &[&namespace.binding(), &key]).await.map_err(backend)?.get(0);
            if start > total as u64 {
                return Err(backend("activity cursor outside projection"));
            }
            tx.tx.query("SELECT identity,metadata FROM semon_projection.activity_items WHERE namespace=$1 AND session_key=$2 AND kind='steps' AND position>=$3 ORDER BY position LIMIT $4", &[&namespace.binding(), &key, &super::number(start)?, &super::number(limit as u64 + 1)?]).await.map_err(backend)?
        };
        let items = records
            .iter()
            .take(limit)
            .map(|record| serde_json::from_str::<Value>(record.get(1)).map_err(backend))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if records.len() > limit {
            let last: String = records[limit - 1].get(0);
            Some(if facts {
                last
            } else {
                (last.parse::<u64>().map_err(backend)? + 1).to_string()
            })
        } else {
            None
        };
        reader.finish().map_err(backend)?;
        drop(reader);
        tx.commit(namespace).await?;
        let check = self.begin(namespace).await?;
        let current = check
            .catalog_row(namespace, key, CatalogReadScope::Current)
            .await?
            .ok_or_else(|| backend("activity owner changed"))?;
        let actual = check.tx.query_opt("SELECT generation FROM semon_projection.session_activity WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?;
        if current.sources != row.sources
            || actual
                .as_ref()
                .map(|record| record.get::<_, String>(0))
                .as_deref()
                != Some(generation.as_str())
        {
            return Err(backend("activity publication changed"));
        }
        check.commit(namespace).await?;
        Ok(
            json!({"now":now,"session":{"id":key,"harness":row.harness,"native_ids":row.native_ids},"generation":generation,"kind":if facts {"facts"} else {"steps"},"items":items,"next_cursor":next_cursor}),
        )
    }
}

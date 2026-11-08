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
        let metadata = serde_json::to_string(activity).map_err(backend)?;
        if metadata.len() > 1048576 {
            return Err(backend("activity publication byte budget exceeded"));
        }
        let generation = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&sources, activity)).map_err(backend)?)
        );
        self.tx.execute("INSERT INTO semon_projection.session_activity(namespace,session_key,source_generation,generation,metadata) VALUES($1,$2,$3,$4,$5)", &[&namespace.binding(), &row.key, &sources, &generation, &metadata]).await.map_err(backend)?;
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
        let (items, next_cursor) = if facts {
            let mut facts = activity
                .facts
                .iter()
                .filter(|fact| fact.identity.as_str() > cursor.unwrap_or(""))
                .collect::<Vec<_>>();
            facts.sort_by(|a, b| a.identity.cmp(&b.identity));
            let more = facts.len() > limit;
            let items = facts.iter().take(limit).map(|fact| json!({"identity":fact.identity,"timestamp":fact.timestamp,"step_id":fact.step_id,"change":fact.change})).collect::<Vec<_>>();
            let next = more.then(|| facts[limit - 1].identity.clone());
            (items, next)
        } else {
            let start = cursor
                .map(str::parse::<usize>)
                .transpose()
                .map_err(backend)?
                .unwrap_or(0);
            if start > activity.steps.len() {
                return Err(backend("activity cursor outside projection"));
            }
            let items = activity
                .steps
                .iter()
                .skip(start)
                .take(limit)
                .map(|step| {
                    let mut item = serde_json::to_value(step).map_err(backend)?;
                    item.as_object_mut()
                        .expect("typed activity step")
                        .remove("position");
                    Ok(item)
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            let next = start + items.len();
            let cursor = (next < activity.steps.len()).then(|| next.to_string());
            (items, cursor)
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

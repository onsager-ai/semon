//! Source/rules/price-qualified activity through the restricted PostgreSQL role.
use super::{Namespace, PgProjectionStore, PgProjectionTransaction, StorageError, backend};
use crate::{CatalogReadScope, Options, model::summary::CatalogRow};
use serde_json::{Value, json};

impl PgProjectionTransaction<'_> {
    pub(super) async fn publish_activity(
        &self,
        namespace: &Namespace,
        row: &CatalogRow,
    ) -> Result<(), StorageError> {
        self.tx
            .execute(
                "DELETE FROM semon_projection.activity WHERE namespace=$1 AND session_key=$2",
                &[&namespace.binding(), &row.key],
            )
            .await
            .map_err(backend)?;
        let Some(projection) = &row.session_activity else {
            return Ok(());
        };
        let activity = &projection.activity;
        if activity.session_key != row.key
            || activity.rules_version != crate::events::session_activity::ACTIVITY_RULES_VERSION
        {
            return Err(backend("invalid activity publication"));
        }
        let sources = crate::retention::source_generation(&row.sources).map_err(backend)?;
        let metadata = serde_json::to_string(activity).map_err(backend)?;
        if metadata.len() > 1024 * 1024 {
            return Err(backend("activity publication budget exceeded"));
        }
        use sha2::{Digest, Sha256};
        let generation = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_string(&(&sources, activity))
                    .map_err(backend)?
                    .as_bytes()
            )
        );
        self.tx.execute("INSERT INTO semon_projection.activity(namespace,session_key,source_generation,generation,rules,pricing,metadata) VALUES($1,$2,$3,$4,$5,$6,$7)", &[&namespace.binding(), &row.key, &sources, &generation, &(activity.rules_version as i32), &activity.pricing_fingerprint, &metadata]).await.map_err(backend)?;
        Ok(())
    }
}

impl PgProjectionStore {
    pub(crate) async fn activity(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        key: &str,
        args: &Value,
        now: i64,
    ) -> Result<Value, StorageError> {
        if args["id"].as_str() != Some(key) {
            return Err(backend("activity identity changed"));
        }
        let tx = self.begin(namespace).await?;
        let row = tx
            .catalog_row(namespace, key, CatalogReadScope::Current)
            .await?
            .ok_or_else(|| backend("selected activity unavailable"))?;
        if row.sources.iter().any(|source| {
            crate::catalog::source_relative(options, &row.harness, source).is_none()
                || !crate::model::catalog_source_in_window(
                    source,
                    crate::model::scan_cutoff(options, now),
                )
        }) {
            return Err(backend("selected activity source mismatch"));
        }
        let record = tx.tx.query_opt("SELECT generation,source_generation,rules,pricing,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_projection.activity WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?.ok_or_else(|| backend("selected activity unavailable"))?;
        let generation: String = record.get(0);
        let pricing = crate::pricing::snapshot();
        if record.get::<_, String>(1)
            != crate::retention::source_generation(&row.sources).map_err(backend)?
            || record.get::<_, i32>(2)
                != crate::events::session_activity::ACTIVITY_RULES_VERSION as i32
            || record.get::<_, String>(3) != pricing.fingerprint
            || args["generation"]
                .as_str()
                .is_some_and(|expected| expected != generation)
            || (args["cursor"].is_string() && !args["generation"].is_string())
        {
            return Err(backend("selected activity generation mismatch"));
        }
        let text: Option<&str> = record.get(4);
        let activity: crate::events::session_activity::SessionActivity =
            serde_json::from_str(text.ok_or_else(|| backend("activity budget exceeded"))?)
                .map_err(backend)?;
        let facts = args["kind"] == "facts";
        let cursor = args["cursor"].as_str();
        let limit = args["limit"].as_u64().unwrap_or(100).min(256) as usize;
        let mut items = Vec::new();
        let next = if facts {
            let mut facts = activity.facts;
            facts.sort_by(|a, b| a.identity.cmp(&b.identity));
            let eligible: Vec<_> = facts
                .iter()
                .filter(|fact| cursor.is_none_or(|cursor| fact.identity.as_str() > cursor))
                .take(limit + 1)
                .collect();
            for fact in eligible.iter().take(limit) {
                items.push(json!({"identity":fact.identity,"timestamp":fact.timestamp,"step_id":fact.step_id,"change":fact.change}));
            }
            (eligible.len() > limit).then(|| eligible[limit - 1].identity.clone())
        } else {
            let start = cursor
                .map(str::parse::<usize>)
                .transpose()
                .map_err(backend)?
                .unwrap_or(0);
            if start > activity.steps.len() {
                return Err(backend("activity cursor outside projection"));
            }
            for step in activity.steps.iter().skip(start).take(limit) {
                let mut item = serde_json::to_value(step).map_err(backend)?;
                item.as_object_mut()
                    .expect("typed activity")
                    .remove("position");
                items.push(item);
            }
            let end = start + items.len();
            (end < activity.steps.len()).then(|| end.to_string())
        };
        tx.commit(namespace).await?;
        Ok(
            json!({"now":now,"session":{"id":key,"harness":row.harness,"native_ids":row.native_ids},"generation":generation,"kind":if facts {"facts"} else {"steps"},"items":items,"next_cursor":next}),
        )
    }
}

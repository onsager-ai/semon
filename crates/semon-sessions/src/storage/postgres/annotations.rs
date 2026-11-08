//! Metadata-only annotation addresses in the selected repeatable-read snapshot.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure, number};
use crate::{CatalogReadScope, Options, ViewerReply};
use serde_json::json;

impl PgProjectionStore {
    pub(crate) async fn annotations(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        query: &str,
    ) -> Result<ViewerReply, StorageError> {
        ensure(&self.namespace, namespace)?;
        let invalid = || crate::catalog::error(400, "invalid_arguments", "Use one selected session, retained scope, errors or signals, and limit=1..100.", false);
        for name in ["sid", "machine", "scope", "kind", "after", "limit", "generation"] {
            if query.split('&').filter(|part| part.split('=').next() == Some(name)).count() > 1
                || crate::viewer::query_value(query, name).is_some_and(|value| crate::viewer::decoded(value).is_none())
            { return Ok(invalid()); }
        }
        let argument = |name| crate::viewer::query_value(query, name).and_then(crate::viewer::decoded);
        let Some(key) = argument("sid").filter(|key| !key.is_empty() && key.len() <= 4096) else { return Ok(invalid()) };
        let kind = argument("kind").unwrap_or_default();
        if argument("scope").as_deref() != Some("retained_history") || !matches!(kind.as_str(), "errors" | "signals") { return Ok(invalid()); }
        let (Ok(after), Ok(limit)) = (argument("after").map(|value| value.parse::<usize>()).transpose(), argument("limit").map(|value| value.parse::<usize>()).transpose()) else { return Ok(invalid()) };
        let (after, limit) = (after.unwrap_or(0), limit.unwrap_or(100));
        if !(1..=100).contains(&limit) || (after > 0 && argument("generation").is_none()) { return Ok(invalid()); }
        let tx = self.begin(namespace).await?;
        let request = crate::catalog::Request::parse(&format!("sid={}&scope=retained_history", crate::viewer::percent_encode(&key))).map_err(|_| backend("invalid selected identity"))?;
        let reply = crate::catalog::postgres_page(&tx, namespace, options, &request).await.map_err(backend)?;
        let Some((_, session)) = crate::catalog::selection_from_reply(reply, namespace.as_str(), &key, CatalogReadScope::RetainedHistory, false).map_err(|_| backend("selected identity unavailable"))? else { return Ok(crate::catalog::error(404, "session_not_found", "Recorded session unavailable.", false)) };
        let (stored, _, _) = tx.recipe_page(namespace, &key, CatalogReadScope::RetainedHistory, None, 0, 1).await?;
        let history = tx.tx.query_opt("SELECT 1 FROM semon_projection.history_catalog WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?.is_some();
        let table = if history { "history_recipes" } else { "recipes" };
        let predicate = if kind == "signals" {
            "metadata::jsonb->'recipe'->>'recipe'='Signal'"
        } else if crate::transcript_range::selected_working(&session) {
            "((metadata::jsonb->'recipe'->>'recipe' IN ('Tool','Yielded','Sent') AND metadata::jsonb->'recipe'->>'outcome'='Err') OR (metadata::jsonb->'recipe'->>'recipe'='Operation' AND metadata::jsonb->'recipe'->>'ok'='false'))"
        } else {
            "((metadata::jsonb->'recipe'->>'recipe' IN ('Tool','Yielded','Sent') AND metadata::jsonb->'recipe'->>'outcome' IN ('Err','Pending')) OR (metadata::jsonb->'recipe'->>'recipe'='Operation' AND metadata::jsonb->'recipe'->>'ok'='false'))"
        };
        let entries_generation = crate::transcript_range::entry_generation(&stored);
        let generation = if kind == "errors" { crate::transcript_range::error_generation(&entries_generation, &session) } else { entries_generation };
        if argument("generation").is_some_and(|expected| expected != generation) { return Ok(crate::catalog::error(409, "stale_projection", "The selected annotation projection changed.", true)); }
        let total: i64 = tx.tx.query_one(&format!("SELECT count(*) FROM semon_projection.{table} WHERE namespace=$1 AND session_key=$2 AND {predicate}"), &[&namespace.binding(), &key]).await.map_err(backend)?.get(0);
        if after as u64 > total as u64 { return Ok(invalid()); }
        let records = tx.tx.query(&format!("SELECT slot FROM semon_projection.{table} WHERE namespace=$1 AND session_key=$2 AND {predicate} ORDER BY slot LIMIT $3 OFFSET $4"), &[&namespace.binding(), &key, &number(limit as u64)?, &number(after as u64)?]).await.map_err(backend)?;
        let entries: Vec<_> = records.iter().map(|record| json!({"slot":record.get::<_, i64>(0)})).collect();
        let end = after + entries.len();
        tx.commit(namespace).await?;
        Ok(ViewerReply { status: 200, content_type: "application/json; charset=utf-8", body: json!({"api":1,"source_key":namespace.as_str(),"catalog_key":key,"scope":"retained_history","kind":kind,"generation":generation,"total":total,"first":after,"entries":entries,"next_cursor":if (end as u64)<total as u64 {Some(end)} else {None}}).to_string().into_bytes(), etag: None })
    }
}

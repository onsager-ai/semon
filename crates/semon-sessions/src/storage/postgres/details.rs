//! Selected details from the producer's source-bound causal publication.
use super::{Namespace, PgProjectionTransaction, StorageError, backend};
use crate::{CatalogReadScope, Options};
use serde_json::{Value, json};

impl PgProjectionTransaction<'_> {
    pub(crate) async fn selected_details(
        &self,
        namespace: &Namespace,
        options: &Options,
        key: &str,
        scope: CatalogReadScope,
    ) -> Result<Option<Value>, StorageError> {
        let Some(row) = self.catalog_row(namespace, key, scope).await? else { return Ok(None) };
        let context = self.tx.query_opt(
            "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,source_generation FROM semon_projection.query_context WHERE namespace=$1 AND session_key=$2",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        let Some(context) = context else { return Ok(None) };
        if context.get::<_, String>(1) != crate::retention::source_generation(&row.sources).map_err(backend)? {
            return Ok(None);
        }
        let text: Option<&str> = context.get(0);
        let context: Value = serde_json::from_str(text.ok_or_else(|| backend("selected details budget exceeded"))?).map_err(backend)?;
        let mut details = context["session"]["details"].clone();
        if !details.is_object() { return Err(backend("selected details unavailable")); }
        details["busy"] = context["session"]["busy"].clone();
        details["content_generation"] = context["session"]["content_generation"].clone();
        details["live_observation"] = json!(scope == CatalogReadScope::Current);
        let relatives = self.tx.query(
            "WITH RECURSIVE family(session_key,depth) AS (SELECT session_key,0 FROM semon_projection.catalog WHERE namespace=$1 AND session_key=$2 UNION ALL SELECT c.session_key,f.depth+1 FROM semon_projection.catalog c JOIN family f ON c.metadata::jsonb->>'parent'=f.session_key WHERE c.namespace=$1 AND f.depth<32) SELECT session_key FROM family WHERE depth>0 ORDER BY depth,session_key LIMIT 100",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        let mut runs = Vec::new();
        let mut ancestors = Vec::new();
        let mut seen = std::collections::BTreeSet::from([key.to_owned()]);
        let mut incomplete = relatives.len() == 100;
        for record in relatives.iter().take(99) {
            let sid: &str = record.get(0);
            if !seen.insert(sid.to_owned()) { incomplete = true; continue; }
            let selected = self.catalog_row(namespace, sid, CatalogReadScope::RetainedHistory).await?;
            if let Some(selected) = selected {
                runs.push(relative(options, selected)?);
            } else { incomplete = true; }
        }
        let mut parent = row.parent;
        for _ in 0..32 {
            let Some(sid) = parent.take() else { break };
            if !seen.insert(sid.clone()) { incomplete = true; break; }
            let Some(selected) = self.catalog_row(namespace, &sid, CatalogReadScope::RetainedHistory).await? else { incomplete = true; break };
            parent = selected.parent.clone();
            ancestors.push(relative(options, selected)?);
        }
        incomplete |= parent.is_some();
        ancestors.reverse();
        details["family"] = json!({"state":if incomplete {"incomplete"} else {"complete"},"ancestors":ancestors,"runs":runs});
        Ok(Some(details))
    }
}

fn relative(options: &Options, row: crate::model::summary::CatalogRow) -> Result<Value, StorageError> {
    if row.sources.iter().any(|source| crate::catalog::source_relative(options, &row.harness, source).is_none()) {
        return Err(backend("relative source outside configured roots"));
    }
    Ok(json!({"key":row.key,"name":row.name,"harness":row.harness,"kind":row.presentation_kind.as_deref().unwrap_or(&row.kind),"parent":row.parent,"last":row.last,"cost":row.cost,"status":{"state":null,"observation":"unavailable"}}))
}

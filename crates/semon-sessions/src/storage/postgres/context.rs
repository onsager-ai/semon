//! Bounded causal range context from the same snapshot as selected recipes.
use super::{Namespace, PgProjectionTransaction, StorageError, backend};
use crate::{
    CatalogReadScope, Options,
    model::summary::CatalogSource,
    slot_projection::{Recipe, SavedSlot},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

impl PgProjectionTransaction<'_> {
    async fn causal_context(
        &self,
        namespace: &Namespace,
        options: &Options,
        key: &str,
        sources: Option<&[CatalogSource]>,
    ) -> Result<Option<Value>, StorageError> {
        let record = self.tx.query_opt("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,source_generation FROM semon_projection.query_context WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?;
        let Some(record) = record else {
            return Ok(None);
        };
        let owned;
        let sources = if let Some(sources) = sources {
            sources
        } else {
            let Some(row) = self
                .catalog_row(namespace, key, CatalogReadScope::RetainedHistory)
                .await?
            else {
                return Ok(None);
            };
            owned = row.sources;
            &owned
        };
        if record.get::<_, String>(1)
            != crate::retention::source_generation(sources).map_err(backend)?
        {
            return Ok(None);
        }
        if sources
            .iter()
            .any(|source| crate::native_evidence::catalog_input(&source.path, options).is_none())
        {
            return Err(backend("causal context outside configured roots"));
        }
        let text: Option<&str> = record.get(0);
        let context =
            serde_json::from_str(text.ok_or_else(|| backend("causal context budget exceeded"))?)
                .map_err(backend)?;
        Ok(Some(context))
    }

    pub(crate) async fn range_context(
        &self,
        namespace: &Namespace,
        options: &Options,
        key: &str,
        slots: &[SavedSlot],
        sources: &[CatalogSource],
    ) -> Result<Value, StorageError> {
        let unavailable =
            || json!({"state":"incomplete","turns":[],"handoffs":[],"sessions":{},"previews":{}});
        let Some(context) = self
            .causal_context(namespace, options, key, Some(sources))
            .await?
        else {
            return Ok(unavailable());
        };
        let mut missing = context["session"]["relationships_incomplete"] == true;
        let mut ids = BTreeSet::new();
        let turn_ids: BTreeSet<_> = slots
            .iter()
            .filter_map(|slot| slot.turn.as_deref())
            .collect();
        for slot in slots {
            if let Recipe::H { id } = &slot.recipe {
                ids.insert(id.clone());
            }
        }
        let mut turns = Vec::new();
        for id in turn_ids {
            if let Some(turn) = context["turns"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|turn| turn["id"] == id)
            {
                if turn.to_string().len() > 65536
                    || turn["sent"].as_array().is_none_or(|sent| sent.len() > 1000)
                {
                    missing = true;
                    continue;
                }
                if let Some(start) = turn["start"].as_str() {
                    ids.insert(start.to_owned());
                }
                turns.push(turn.clone());
            } else {
                missing = true;
            }
        }
        let mut edges = Vec::new();
        let mut participants = BTreeSet::new();
        let mut owners = BTreeSet::new();
        for id in ids {
            let Some(edge) = context["edges"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|edge| edge["id"] == id)
            else {
                missing = true;
                continue;
            };
            if edge.to_string().len() > 65536 {
                missing = true;
                continue;
            }
            let mut edge = edge.clone();
            for field in ["brief_session", "result_session"] {
                if let Some(owner) = edge[field].as_str() {
                    owners.insert(owner.to_owned());
                }
            }
            for field in ["from", "to"] {
                if let Some(sid) = edge[field].as_str().filter(|sid| *sid != "you") {
                    participants.insert(sid.to_owned());
                }
            }
            if let Some(target) = edge["to"].as_str() {
                if let Some(target_context) = self
                    .causal_context(namespace, options, target, None)
                    .await?
                {
                    let found: Vec<_> = target_context["turns"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|turn| turn["start"] == id)
                        .take(2)
                        .collect();
                    if found.len() == 1 {
                        edge["target_turn"] = found[0]["id"].clone();
                    }
                }
            }
            edges.push(edge);
        }
        missing |= participants.len() > 201 || owners.len() > 128;
        let mut sessions = BTreeMap::new();
        let mut previews = BTreeMap::new();
        let mut bytes = 0usize;
        for sid in participants.into_iter().take(201) {
            let Some(context) = self.causal_context(namespace, options, &sid, None).await? else {
                missing = true;
                continue;
            };
            let session = &context["session"];
            let compact = json!({"name":session["name"],"harness":session["harness"],"model":session["model"],"kind":session["kind"],"state":session["state"],"start":session["start"],"last":session["last"],"calls":session["details"]["calls"],"usd":session["family_usd"],"activity":session["details"]["activity"],"live_observation":false});
            bytes += compact.to_string().len();
            if bytes > 4 * 1024 * 1024 {
                return Ok(unavailable());
            }
            sessions.insert(sid, compact);
        }
        for sid in owners.into_iter().take(128) {
            let Some(context) = self.causal_context(namespace, options, &sid, None).await? else {
                missing = true;
                continue;
            };
            if let Some(content) = context["session"]["content_generation"]
                .as_str()
                .filter(|content| content.len() == 64)
            {
                previews.insert(sid, content.to_owned());
            } else {
                missing = true;
            }
        }
        Ok(
            json!({"state":if missing {"incomplete"} else {"complete"},"turns":turns,"handoffs":edges,"sessions":sessions,"previews":previews}),
        )
    }
}

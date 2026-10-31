//! Private, generation-bound Query recipes in the Event Index metadata region.
//! Public Trace rows and digests retain their existing contract. No bodies live here.
use crate::{
    model::{Handoff, Session, Turn, summary::CatalogRow},
    overview::Summary,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The selected result is distinct from the compact Catalog observation. Its
/// complete native metadata remains available to explicit session/field reads.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectedRecipe {
    pub(crate) session: Value,
    pub(crate) turns: Vec<Value>,
    pub(crate) edges: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectedRecipeRef {
    version: u32,
    digest: String,
}

impl SelectedRecipe {
    /// Replace selected result metadata with its exact publication reference.
    /// The caller publishes the returned bytes atomically with this row.
    pub(crate) fn detach(row: &mut CatalogRow) -> std::io::Result<Option<String>> {
        let Some(recipe) = &row.query_recipe else {
            return Ok(None);
        };
        let reference = recipe.reference(row).map_err(std::io::Error::other)?;
        let encoded = serde_json::to_string(recipe).map_err(std::io::Error::other)?;
        let query = row.query.as_mut().ok_or_else(|| {
            std::io::Error::other("Selected Query observation unavailable")
        })?;
        query["selected"] = serde_json::to_value(reference).map_err(std::io::Error::other)?;
        Ok(Some(encoded))
    }

    pub(crate) fn reference(&self, row: &CatalogRow) -> serde_json::Result<SelectedRecipeRef> {
        let source = crate::retention::source_generation(&row.sources)?;
        let bytes = serde_json::to_vec(&(
            "selected-query-recipe-v1",
            &row.key,
            &row.harness,
            source,
            self,
        ))?;
        Ok(SelectedRecipeRef {
            version: 1,
            digest: format!("{:x}", Sha256::digest(bytes)),
        })
    }

    /// The caller has already qualified the row's actual publication residence.
    /// A digest is integrity evidence, never namespace or source authority.
    pub(crate) fn decode(row: &CatalogRow, text: &str) -> std::io::Result<Self> {
        let metadata = serde_json::to_vec(row).map_err(std::io::Error::other)?;
        if metadata.len().saturating_add(text.len()) > crate::storage::proof_codec::METADATA_MAX {
            return Err(std::io::Error::other("Selected Query metadata budget exceeded"));
        }
        let selected = row.query.as_ref().and_then(|query| query.get("selected"))
            .ok_or_else(|| std::io::Error::other("Selected Query reference unavailable"))?;
        let expected: SelectedRecipeRef = serde_json::from_value(selected.clone())
            .map_err(std::io::Error::other)?;
        let recipe: Self = serde_json::from_str(text).map_err(std::io::Error::other)?;
        if expected != recipe.reference(row).map_err(std::io::Error::other)? {
            return Err(std::io::Error::other("Selected Query recipe changed"));
        }
        Ok(recipe)
    }
}

#[cfg(test)]
mod selected_recipe_tests {
    use super::*;

    fn row() -> CatalogRow {
        serde_json::from_value(json!({
            "key":"owner","name":"owner","harness":"claude","kind":"session",
            "native_ids":[],"parent":null,"parent_source":null,"repo":null,
            "branch":null,"model":"","effort":null,"start":100,"last":100,
            "tokens":[0,0,0],"cost":{"usd":null,"unpriced_models":[],
                "split_unknown_messages":0,"by_model":{},"by_day":{}},
            "sources":[],"query":{"version":1}
        })).unwrap()
    }

    fn recipe() -> SelectedRecipe {
        SelectedRecipe {
            session: json!({"busy":[[100,200]]}),
            turns: vec![json!({"id":"turn","query":{"at":100}})],
            edges: vec![json!({"id":"edge","query":{"from":"owner","to":"peer"}})],
        }
    }

    #[test]
    fn detached_recipe_preserves_complete_payload_and_compact_observation() {
        let mut row = row();
        let mut recipe = recipe();
        recipe.session["busy"] = json!(vec![[100,200]; 2048]);
        row.query_recipe = Some(recipe.clone());
        let encoded = SelectedRecipe::detach(&mut row).unwrap().unwrap();
        let metadata = serde_json::to_string(&row).unwrap();
        assert!(metadata.len() < 8192);
        assert!(row.query.as_ref().unwrap()["selected"].get("session").is_none());
        assert_eq!(SelectedRecipe::decode(&row, &encoded).unwrap(), recipe);
        let restored: CatalogRow = serde_json::from_str(&metadata).unwrap();
        assert!(restored.query_recipe.is_none());
        assert_eq!(SelectedRecipe::decode(&restored, &encoded).unwrap(), recipe);
    }

    #[test]
    fn recipe_reference_binds_owner_harness_and_complete_source_vector() {
        let mut row = row();
        row.query_recipe = Some(recipe());
        let encoded = SelectedRecipe::detach(&mut row).unwrap().unwrap();
        let mut foreign = row.clone();
        foreign.key = "other".into();
        assert!(SelectedRecipe::decode(&foreign, &encoded).is_err());
        foreign = row.clone();
        foreign.harness = "codex".into();
        assert!(SelectedRecipe::decode(&foreign, &encoded).is_err());
        foreign = row.clone();
        foreign.sources.push(crate::model::summary::CatalogSource {
            path: "source.jsonl".into(), native_id: "native".into(),
            dev: 1, ino: 1, size: 1, modified_ns: 1, changed_ns: Some(1),
            offset: 1, prefix_sha256: [1;32], tail_sha256: [1;32], native_role: None,
        });
        assert!(SelectedRecipe::decode(&foreign, &encoded).is_err());
        row.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        assert!(SelectedRecipe::decode(&row, &encoded).is_ok());
    }

    #[test]
    fn consuming_recipe_refuses_missing_corrupt_or_changed_payload() {
        let mut row = row();
        row.query_recipe = Some(recipe());
        let encoded = SelectedRecipe::detach(&mut row).unwrap().unwrap();
        assert!(SelectedRecipe::decode(&row, "").is_err());
        assert!(SelectedRecipe::decode(&row, "{}").is_err());
        let mut changed = recipe();
        changed.session["busy"] = json!([]);
        assert!(SelectedRecipe::decode(&row, &serde_json::to_string(&changed).unwrap()).is_err());
        row.query.as_mut().unwrap().as_object_mut().unwrap().remove("selected");
        assert!(SelectedRecipe::decode(&row, &encoded).is_err());
    }

    #[test]
    fn recipe_load_preserves_combined_catalog_record_budget() {
        let mut row = row();
        row.query_recipe = Some(recipe());
        let mut encoded = SelectedRecipe::detach(&mut row).unwrap().unwrap();
        let metadata = serde_json::to_vec(&row).unwrap().len();
        encoded.push_str(&" ".repeat(crate::storage::proof_codec::METADATA_MAX - metadata - encoded.len()));
        assert!(SelectedRecipe::decode(&row, &encoded).is_ok());
        encoded.push(' ');
        assert!(SelectedRecipe::decode(&row, &encoded).is_err());
    }
}

/// Retained endpoints must own their field recipes before a later Current
/// publication can withdraw the other endpoint's relationship observation.
pub(crate) fn complete_recipes(rows: &mut [CatalogRow]) -> std::io::Result<()> {
    let mut fields = std::collections::BTreeMap::<String, serde_json::Map<String, Value>>::new();
    for row in rows.iter() {
        for edge in row
            .query_recipe
            .as_ref()
            .into_iter()
            .flat_map(|recipe| &recipe.edges)
        {
            let saved = fields
                .entry(edge["id"].as_str().expect("edge identity").into())
                .or_default();
            for (field, recipe) in edge["query_content"].as_object().into_iter().flatten() {
                if recipe.is_null() {
                    continue;
                }
                if let Some(prior) = saved.insert(field.clone(), recipe.clone())
                    && prior != *recipe
                {
                    return Err(std::io::Error::other(
                        "Native handoff source recipes disagree",
                    ));
                }
            }
        }
    }
    for row in rows {
        if let Some(recipe) = &mut row.query_recipe {
            for edge in &mut recipe.edges {
                edge["query_content"] = json!(fields[edge["id"].as_str().expect("edge identity")]);
            }
        }
        if let Some(recipe) = &row.query_recipe {
            row.query.as_mut().ok_or_else(|| std::io::Error::other("Selected Query observation missing"))?["selected"] =
                serde_json::to_value(recipe).map_err(std::io::Error::other)?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct Projection {
    session: Value,
    turns: Vec<Value>,
    edges: Vec<Value>,
    virtuals: std::collections::BTreeMap<String, Vec<Value>>,
}
fn data(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}
pub(crate) fn identity(row: &CatalogRow) -> String {
    // One latest projection per native harness owner. A reused cross-harness
    // key keeps original and Current custody separate without accumulating a
    // new copy of every recipe on each append.
    format!("{}:{}:{}", row.harness.len(), row.harness, row.key)
}
pub(crate) fn prefix(kind: &str, key: &str) -> String {
    format!("native_query_{kind}:{}:{key}:", key.len())
}
pub(crate) fn header(db: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT CASE WHEN octet_length(value)<=65536 THEN value ELSE NULL END FROM main.meta WHERE namespace=semon_namespace() AND key=?1", [prefix("header",key)], |r|r.get(0)).optional().map(Option::flatten)
}
impl Projection {
    pub(crate) fn capture(session: &Session, turns: &[&Turn], edges: &[&Handoff]) -> Self {
        let mut virtuals = std::collections::BTreeMap::new();
        for edge in edges {
            if edge.to.is_none()
                && let Some(target) = &edge.target
            {
                let name = format!("unsent:{target}");
                virtuals
                    .entry(name)
                    .or_insert_with(Vec::new)
                    .push(json!({"id":edge.id,"at":edge.at}));
            }
        }
        for slots in virtuals.values_mut() {
            slots.sort_by_key(|slot| {
                (
                    slot["at"].as_i64(),
                    slot["id"].as_str().unwrap_or("").to_owned(),
                )
            });
        }
        Self {
            virtuals,
            session: json!({"busy":session.busy}),
            turns: turns.iter().enumerate().map(|(order,turn)| {
                let mut query=json!(turn);
                query.as_object_mut().expect("turn object").remove("text");
                json!({"query":query,"query_text":turn.text.is_some(),"query_order":order,"id":turn.id})
            }).collect(),
            edges: edges.iter().map(|edge| {
                let mut query=json!(edge);
                let mut presence=serde_json::Map::new();
                for field in ["brief","result","answer","answers"] {
                    presence.insert(field.into(),json!(query.as_object_mut().expect("edge object").remove(field).is_some()));
                }
                json!({"query":query,"query_presence":presence,"query_brief_constant":match edge.brief.as_str() { "(brief not in these logs)"=>Some(1),"(observed child run)"=>Some(2),"(question)"=>Some(3),_=>None },"id":edge.id})
            }).collect(),
        }
    }
    /// Content-free selected recipes, persisted with the same Catalog witness.
    pub(crate) fn selected_recipe(&self, summary: &Summary) -> SelectedRecipe {
        let edges: Vec<_> = self
            .edges
            .iter()
            .map(|edge| {
                let mut edge = edge.clone();
                edge["query_content"] =
                    summary.query_recipe(edge["id"].as_str().expect("edge identity"));
                edge
            })
            .collect();
        SelectedRecipe {
            session: self.session.clone(),
            turns: self.turns.clone(),
            edges,
        }
    }

    pub(crate) fn virtual_transcript(&mut self, key: &str, transcript: &crate::model::Transcript) {
        self.virtuals.insert(
            key.into(),
            transcript
                .slots
                .iter()
                .filter_map(|slot| {
                    if let crate::model::SlotKind::H(id) = &slot.kind {
                        Some(json!({"id":id,"at":slot.t}))
                    } else {
                        None
                    }
                })
                .collect(),
        );
    }

    pub(crate) fn publish(
        &self,
        tx: &Transaction<'_>,
        row: &CatalogRow,
        trace: &str,
    ) -> rusqlite::Result<()> {
        let session = serde_json::to_string(&self.session).map_err(data)?;
        let turns: Vec<_> = self
            .turns
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()
            .map_err(data)?;
        let edges: Vec<_> = row
            .query_recipe
            .as_ref()
            .map(|recipe| &recipe.edges)
            .ok_or(rusqlite::Error::InvalidQuery)?
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()
            .map_err(data)?;
        let source = crate::retention::source_generation(&row.sources).map_err(data)?;
        let identity = identity(row);
        let query_generation = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&row.query).map_err(data)?)
        );
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(&(1, &source, trace)).map_err(data)?);
        for text in std::iter::once(&session).chain(turns.iter()).chain(&edges) {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
        hash.update(serde_json::to_vec(&self.virtuals).map_err(data)?);
        let generation = format!("{:x}", hash.finalize());
        let complete =
            session.len() <= 1048576 && turns.iter().chain(&edges).all(|text| text.len() <= 65536);
        let header=json!({"version":1,"generation":generation,"source_generation":source,"trace":trace,"query_generation":query_generation,"complete":complete,"turns":turns.len(),"edges":edges.len()}).to_string();
        if super::query_projection::header(tx, &identity)?.as_deref() == Some(&header) {
            return Ok(());
        }
        for kind in ["turn", "edge", "virtual", "virtual-slot"] {
            let prefix = prefix(kind, &identity);
            tx.execute("DELETE FROM main.meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?1||'~'", [prefix])?;
        }
        let mut put = tx.prepare_cached(
            "INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_namespace(),?1,?2)",
        )?;
        if complete {
            put.execute(params![prefix("session", &identity), session])?;
            for (order, text) in turns.iter().enumerate() {
                put.execute(params![
                    format!("{}{:020}", prefix("turn", &identity), order),
                    text
                ])?;
            }
            for (key, slots) in &self.virtuals {
                put.execute(params![
                    format!("{}{}", prefix("virtual", &identity), key),
                    json!({"total":slots.len()+1}).to_string()
                ])?;
                for (position, slot) in slots.iter().enumerate() {
                    put.execute(params![
                        format!(
                            "{}{}:{key}:{:020}",
                            prefix("virtual-slot", &identity),
                            key.len(),
                            position + 1
                        ),
                        slot.to_string()
                    ])?;
                }
            }
            for (edge, text) in self.edges.iter().zip(&edges) {
                put.execute(params![
                    format!(
                        "{}{}",
                        prefix("edge", &identity),
                        edge["id"].as_str().expect("edge identity")
                    ),
                    text
                ])?;
            }
        }
        put.execute(params![prefix("header", &identity), header])?;
        Ok(())
    }
}

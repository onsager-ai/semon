//! Private, generation-bound Query recipes in the Event Index metadata region.
//! Public Trace rows and digests retain their existing contract. No bodies live here.
use crate::{
    model::{Handoff, Session, Turn, summary::CatalogRow},
    overview::Summary,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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
    db.query_row("SELECT CASE WHEN octet_length(value)<=65536 THEN value ELSE NULL END FROM meta WHERE key=?1", [prefix("header",key)], |r|r.get(0)).optional().map(Option::flatten)
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
        summary: &Summary,
        trace: &str,
    ) -> rusqlite::Result<()> {
        let session = serde_json::to_string(&self.session).map_err(data)?;
        let turns: Vec<_> = self
            .turns
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()
            .map_err(data)?;
        let edges: Vec<_> = self
            .edges
            .iter()
            .map(|edge| {
                let mut edge = edge.clone();
                edge["query_content"] =
                    summary.query_recipe(edge["id"].as_str().expect("edge identity"));
                serde_json::to_string(&edge)
            })
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
            tx.execute("DELETE FROM meta WHERE key>=?1 AND key<?1||'~'", [prefix])?;
        }
        let mut put = tx.prepare_cached("INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)")?;
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

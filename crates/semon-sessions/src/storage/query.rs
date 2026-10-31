//! Backend-neutral Query metadata operations; derivation and native parsing stay shared.
use crate::{QueryError, storage::LogicalSourceNamespace};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Exact residence selected by an admitted store snapshot. This is separate
/// from Catalog lifecycle: a retained display row can still reside in Current,
/// and imported originals retain their immutable producer generation.
#[derive(Clone, Debug)]
pub(crate) enum QueryRecipeResidence {
    NativeCatalog {
        metadata_digest: String,
    },
    NativeHistory {
        metadata_digest: String,
    },
    Immutable {
        owner: QueryRecipeOwner,
        generation: super::proof_lifetime::ProofGenerationId,
        publication: String,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum QueryRecipeOwner {
    Catalog,
    History,
}

/// Store-issued recipe lookup authority. Callers cannot manufacture a History
/// selection from a lifecycle tag or pass a guessed history boolean to lookup.
#[derive(Clone, Debug)]
pub(crate) struct QueryRecipeObservation {
    pub(in crate::storage) residence: QueryRecipeResidence,
    pub(in crate::storage) metadata: String,
    pub(in crate::storage) original: super::QueryRuntimeBinding,
    pub(in crate::storage) destination: super::QueryRuntimeBinding,
    row: crate::model::summary::CatalogRow,
}

impl QueryRecipeObservation {
    /// The owning adapter must first authenticate this exact metadata and its
    /// original binding using the selected residence's existing grant/receipt.
    pub(in crate::storage) fn qualified(
        residence: QueryRecipeResidence,
        metadata: String,
        original: super::QueryRuntimeBinding,
        destination: super::QueryRuntimeBinding,
    ) -> Result<Self, super::StorageError> {
        if original.namespace != destination.namespace
            || original.stream_epoch != destination.stream_epoch
            || original.admission == 0
            || destination.admission == 0
            || metadata.len() > super::proof_codec::METADATA_MAX
        {
            return Err(super::StorageError::ScopeChanged);
        }
        let row = serde_json::from_str(&metadata).map_err(|_| super::StorageError::Unavailable)?;
        Ok(Self {
            residence,
            metadata,
            original,
            destination,
            row,
        })
    }

    pub(crate) fn row(&self) -> &crate::model::summary::CatalogRow {
        &self.row
    }

    /// Decoding cannot substitute a payload from a different observation. The
    /// adapter additionally checks the stored residence and original binding.
    pub(in crate::storage) fn decode(
        &self,
        payload: &str,
    ) -> std::io::Result<crate::query_projection::SelectedRecipe> {
        if self.metadata.len().saturating_add(payload.len()) > super::proof_codec::METADATA_MAX {
            return Err(std::io::Error::other(
                "Selected Query metadata budget exceeded",
            ));
        }
        crate::query_projection::SelectedRecipe::decode(&self.row, payload)
    }
}

#[derive(Debug)]
pub(crate) enum QueryRecipeLoadError {
    Storage(super::StorageError),
    ObservationChanged,
    MissingPayload,
    CorruptPayload,
}

impl From<super::StorageError> for QueryRecipeLoadError {
    fn from(error: super::StorageError) -> Self {
        Self::Storage(error)
    }
}

impl std::fmt::Display for QueryRecipeLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => std::fmt::Display::fmt(error, formatter),
            Self::ObservationChanged => {
                formatter.write_str("Selected Query observation residence or authority changed.")
            }
            Self::MissingPayload => {
                formatter.write_str("Selected Query recipe payload is unavailable.")
            }
            Self::CorruptPayload => {
                formatter.write_str("Selected Query recipe payload is corrupt.")
            }
        }
    }
}

impl std::error::Error for QueryRecipeLoadError {}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Work {
    pub metadata: usize,
    pub edges: usize,
    pub turns: usize,
    pub vm_steps: i32,
}
#[cfg(test)]
thread_local! { pub(crate) static WORK: std::cell::Cell<Work> = const {std::cell::Cell::new(Work {metadata:0,edges:0,turns:0,vm_steps:0})}; }
#[cfg(test)]
pub(crate) fn measured(metadata: usize, edges: usize, turns: usize, vm_steps: i32) {
    WORK.with(|work| {
        let prior = work.get();
        work.set(Work {
            metadata: prior.metadata + metadata,
            edges: prior.edges + edges,
            turns: prior.turns + turns,
            vm_steps: prior.vm_steps + vm_steps,
        });
    });
}

pub(crate) struct Part {
    pub(crate) generation: String,
    pub(crate) runtime: String,
    pub(crate) rows: BTreeMap<String, Value>,
    pub(crate) owners: BTreeMap<String, String>,
    pub(crate) content: BTreeMap<String, String>,
    /// Owner, exact Query facts and whether they came from History.
    pub(crate) witnesses: Vec<(String, String, bool)>,
    pub(crate) edges: BTreeSet<String>,
    pub(crate) turns: BTreeSet<String>,
}

/// Exact retained Query context selected from a row's immutable publication.
/// Current trace tables can change on append, so History must use this saved
/// graph whenever its metadata row is selected.
pub(crate) struct HistoryWindow {
    pub(crate) member: bool,
    pub(crate) rows: BTreeMap<String, Value>,
    pub(crate) context: BTreeSet<String>,
    pub(crate) edges: BTreeSet<String>,
    pub(crate) turns: BTreeSet<String>,
}

pub(crate) fn edge_timed(edge: &Value, cutoff: i64) -> bool {
    edge["at"].as_i64().is_some_and(|at| at >= cutoff)
        || edge["done"].as_i64().is_some_and(|done| done >= cutoff)
        || edge["status"]
            .as_str()
            .is_some_and(|status| matches!(status, "work" | "wait" | "new"))
}

/// Indexed finite-window eligibility, authored with the immutable History row.
/// Last-turn links do not admit an expired owner; `history_window` remains the
/// authority for the exact graph after this projection selects candidates.
pub(crate) fn history_activity(row: &crate::model::summary::CatalogRow) -> (Option<i64>, bool) {
    let Some(query) = &row.query else {
        return (None, false);
    };
    let mut last = query["last"].as_i64();
    let mut active = matches!(query["state"].as_str(), Some("work" | "wait"));
    for edge in query["selected"]["edges"].as_array().into_iter().flatten() {
        let edge = &edge["query"];
        for at in [edge["at"].as_i64(), edge["done"].as_i64()]
            .into_iter()
            .flatten()
        {
            last = Some(last.map_or(at, |last| last.max(at)));
        }
        active |= matches!(edge["status"].as_str(), Some("work" | "wait" | "new"));
    }
    (last, active)
}

pub(crate) fn turn_kept(turn: &Value, cutoff: i64, timed_edges: &BTreeSet<String>) -> bool {
    turn["last"] == serde_json::json!(true)
        || turn["last"] == serde_json::json!(1)
        || turn["at"].as_i64().is_some_and(|at| at >= cutoff)
        || turn["start"]
            .as_str()
            .is_some_and(|id| timed_edges.contains(id))
}

/// All-time Queries retain History preference; finite windows use History only
/// when it qualifies and no eligible Current row is available.
pub(crate) fn select_history(
    cutoff: Option<i64>,
    current_eligible: bool,
    history_available: bool,
    history_member: bool,
) -> bool {
    history_available && (cutoff.is_none() || (!current_eligible && history_member))
}

/// Derive retained membership and required peer context from the exact saved
/// Query graph. `None` preserves the all time retained cohort.
pub(crate) fn history_window(
    row: &crate::model::summary::CatalogRow,
    cutoff: Option<i64>,
) -> Result<HistoryWindow, &'static str> {
    let saved = row.query.as_ref().ok_or("retained_query_missing")?;
    let selected = &saved["selected"];
    let edges = selected["edges"]
        .as_array()
        .ok_or("retained_query_edges_missing")?;
    let turns = selected["turns"]
        .as_array()
        .ok_or("retained_query_turns_missing")?;
    let mut edge_ids = BTreeMap::new();
    let mut timed_edges = BTreeSet::new();
    for recipe in edges {
        let id = recipe["id"]
            .as_str()
            .ok_or("retained_query_edge_id_missing")?;
        let query = recipe
            .get("query")
            .and_then(Value::as_object)
            .ok_or("retained_query_edge_missing")?;
        if edge_ids.insert(id, recipe).is_some() {
            return Err("retained_query_edge_duplicate");
        }
        if cutoff.is_none_or(|cutoff| edge_timed(&Value::Object(query.clone()), cutoff)) {
            timed_edges.insert(id.to_owned());
        }
    }

    let member = cutoff.is_none_or(|cutoff| {
        saved["last"].as_i64().is_some_and(|last| last >= cutoff)
            || matches!(saved["state"].as_str(), Some("work" | "wait"))
            || !timed_edges.is_empty()
    });
    if !member {
        return Ok(HistoryWindow {
            member,
            rows: BTreeMap::new(),
            context: BTreeSet::new(),
            edges: BTreeSet::new(),
            turns: BTreeSet::new(),
        });
    }

    let summaries = saved["rows"]
        .as_object()
        .ok_or("retained_query_rows_missing")?;
    let owner = summaries
        .get(&row.key)
        .cloned()
        .ok_or("retained_query_owner_missing")?;

    let mut kept_edges = timed_edges.clone();
    let mut kept_turns = BTreeSet::new();
    for recipe in turns {
        let id = recipe["id"]
            .as_str()
            .ok_or("retained_query_turn_id_missing")?;
        let turn = recipe
            .get("query")
            .and_then(Value::as_object)
            .ok_or("retained_query_turn_missing")?;
        let turn = Value::Object(turn.clone());
        if cutoff.is_some_and(|cutoff| !turn_kept(&turn, cutoff, &timed_edges)) {
            continue;
        }
        if !kept_turns.insert(id.to_owned()) {
            return Err("retained_query_turn_duplicate");
        }
        if let Some(id) = turn["start"].as_str() {
            kept_edges.insert(id.to_owned());
        }
        if let Some(sent) = turn["sent"].as_array() {
            kept_edges.extend(sent.iter().filter_map(|id| id.as_str().map(str::to_owned)));
        }
    }

    let mut rows = BTreeMap::from([(row.key.clone(), owner)]);
    let mut context = BTreeSet::new();
    for id in &kept_edges {
        let recipe = edge_ids
            .get(id.as_str())
            .ok_or("retained_query_link_missing")?;
        let query = recipe
            .get("query")
            .and_then(Value::as_object)
            .ok_or("retained_query_edge_missing")?;
        for endpoint in ["from", "to"]
            .into_iter()
            .filter_map(|field| query.get(field).and_then(Value::as_str))
            .filter(|endpoint| *endpoint != "you")
        {
            if let Some(summary) = summaries.get(endpoint) {
                rows.entry(endpoint.to_owned())
                    .or_insert_with(|| summary.clone());
            } else {
                context.insert(endpoint.to_owned());
            }
        }
    }

    Ok(HistoryWindow {
        member,
        rows,
        context,
        edges: kept_edges,
        turns: kept_turns,
    })
}

pub(crate) trait QueryCatalogStore {
    /// Load only the exact store-issued observation; this grants no source access.
    fn selected_recipe(
        &self,
        _observation: &QueryRecipeObservation,
    ) -> Result<Option<crate::query_projection::SelectedRecipe>, QueryRecipeLoadError> {
        Err(super::StorageError::Unavailable.into())
    }

    fn selected_body(
        &self,
        _namespace: &LogicalSourceNamespace,
        _key: &str,
        _args: &Value,
        _now: i64,
        _host: &dyn crate::SessionSourceReader,
    ) -> Result<Value, QueryError> {
        Err(QueryError::new(
            "unavailable",
            "Selected body requires a qualified reader",
        ))
    }

    fn read_cohort(
        &self,
        namespace: &LogicalSourceNamespace,
        now: i64,
        bytes_left: &mut usize,
    ) -> Result<Part, QueryError>;
    fn has_search_slots(
        &self,
        namespace: &LogicalSourceNamespace,
        key: &str,
        since: Option<i64>,
    ) -> Result<bool, QueryError>;
    fn validate(&self, namespace: &LogicalSourceNamespace, part: &Part) -> Result<(), QueryError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(id: &str) -> Value {
        json!({"summary":{"id":id}})
    }

    fn retained_row(query: Value) -> crate::model::summary::CatalogRow {
        serde_json::from_value(json!({
            "lifecycle":"retained",
            "key":"owner",
            "name":"owner",
            "harness":"claude",
            "kind":"session",
            "native_ids":[],
            "parent":null,
            "parent_source":null,
            "repo":null,
            "branch":null,
            "model":"",
            "effort":null,
            "start":100,
            "last":100,
            "tokens":[0,0,0],
            "cost":{"usd":null,"unpriced_models":[],"split_unknown_messages":0,"by_model":{},"by_day":{}},
            "sources":[],
            "query":query
        }))
        .unwrap()
    }

    #[test]
    fn prepared_recipe_charges_compact_metadata_and_payload_without_changing_active_row() {
        let recipe = crate::query_projection::SelectedRecipe {
            session: json!({"busy":"x".repeat(super::super::proof_codec::METADATA_MAX * 3 / 4)}),
            turns: Vec::new(),
            edges: Vec::new(),
        };
        let mut row = retained_row(json!({"version":1,"selected":recipe}));
        row.query_recipe = Some(recipe.clone());
        let active = serde_json::to_string(&row).unwrap();
        let prepared = crate::query_projection::SelectedRecipe::prepare(&row).unwrap();
        let payload = prepared.query_recipe.unwrap();
        assert!(active.len() + payload.len() > super::super::proof_codec::METADATA_MAX);
        assert!(prepared.metadata.len() + payload.len() < super::super::proof_codec::METADATA_MAX);
        assert_eq!(serde_json::to_string(&row).unwrap(), active);
        let compact = serde_json::from_str(&prepared.metadata).unwrap();
        assert_eq!(
            crate::query_projection::SelectedRecipe::decode(&compact, &payload).unwrap(),
            recipe
        );
        row.query_recipe = None;
        assert!(crate::query_projection::SelectedRecipe::prepare(&row).is_err());
    }

    #[test]
    fn history_activity_matches_window_eligibility() {
        let mut row = retained_row(json!({
            "last":null,"state":"idle",
            "selected":{"edges":[
                {"query":{"at":100,"done":950,"status":"done"}},
                {"query":{"at":null,"status":"new"}}
            ],"turns":[{"query":{"at":2000,"last":true}}]}
        }));
        assert_eq!(history_activity(&row), (Some(950), true));
        row.query.as_mut().unwrap()["selected"]["edges"][1]["query"]["status"] = json!("done");
        assert_eq!(
            history_activity(&row),
            (Some(950), false),
            "a recent or last turn does not admit the owner"
        );
        row.query.as_mut().unwrap()["state"] = json!("wait");
        assert_eq!(history_activity(&row), (Some(950), true));
        row.query.as_mut().unwrap()["last"] = json!(1000);
        assert_eq!(history_activity(&row), (Some(1000), true));
        row.query = None;
        assert_eq!(history_activity(&row), (None, false));
    }

    #[test]
    fn finite_history_window_uses_only_saved_edge_and_turn_ids() {
        let row = retained_row(json!({
            "last":100,
            "state":"idle",
            "rows":{
                "owner":summary("owner"),
                "recent-peer":summary("recent-peer"),
                "start-peer":summary("start-peer"),
                "sent-peer":summary("sent-peer"),
                "expired-peer":summary("expired-peer")
            },
            "selected":{
                "edges":[
                    {"id":"expired-edge","query":{"at":100,"status":"done","from":"owner","to":"expired-peer"}},
                    {"id":"recent-edge","query":{"at":950,"status":"done","from":"owner","to":"recent-peer"}},
                    {"id":"turn-start","query":{"at":100,"status":"done","from":"owner","to":"start-peer"}},
                    {"id":"turn-sent","query":{"at":110,"status":"done","from":"owner","to":"sent-peer"}}
                ],
                "turns":[
                    {"id":"expired-turn","query":{"at":100,"last":false,"start":"expired-edge","sent":[]}},
                    {"id":"last-turn","query":{"at":100,"last":true,"start":"turn-start","sent":["turn-sent"]}},
                    {"id":"recent-turn","query":{"at":950,"last":false,"start":"recent-edge","sent":[]}}
                ]
            }
        }));

        let window = history_window(&row, Some(900)).unwrap();
        assert!(
            window.member,
            "a recent saved edge admits an old History owner"
        );
        assert_eq!(
            window.edges,
            BTreeSet::from([
                "recent-edge".to_owned(),
                "turn-sent".to_owned(),
                "turn-start".to_owned(),
            ])
        );
        assert_eq!(
            window.turns,
            BTreeSet::from(["last-turn".to_owned(), "recent-turn".to_owned()])
        );
        assert!(!window.rows.contains_key("expired-peer"));
        assert!(window.context.is_empty());
        assert!(select_history(Some(900), false, true, window.member));
        assert!(!select_history(Some(900), true, true, window.member));
        assert!(!select_history(Some(900), false, false, window.member));
        assert!(select_history(None, false, true, window.member));
    }
}

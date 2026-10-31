//! Backend-neutral Query metadata operations; derivation and native parsing stay shared.
use crate::{
    QueryError,
    model::summary::CatalogRow,
    storage::{LogicalSourceNamespace, StorageError},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

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

struct EdgeSourceNeed {
    source: crate::model::summary::CatalogSource,
    owners: BTreeSet<String>,
}

fn edge_source_needs(
    row: &CatalogRow,
) -> Result<(Vec<EdgeSourceNeed>, BTreeSet<String>), StorageError> {
    let edges = row
        .query
        .as_ref()
        .and_then(|query| query["selected"]["edges"].as_array())
        .ok_or(StorageError::ScopeChanged)?;
    let mut needs = Vec::<EdgeSourceNeed>::new();
    let mut endpoint_owners = BTreeSet::new();
    for edge in edges {
        let mut owners = BTreeSet::from([row.key.clone()]);
        for field in ["from", "to"] {
            if let Some(owner) = edge["query"][field].as_str() {
                endpoint_owners.insert(owner.to_owned());
                if owner != "you" {
                    owners.insert(owner.to_owned());
                }
            }
        }
        for recipe in edge["query_content"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, recipe)| recipe)
            .filter(|recipe| !recipe.is_null())
        {
            let path = recipe["path"].as_str().ok_or(StorageError::ScopeChanged)?;
            let source: crate::model::summary::CatalogSource =
                serde_json::from_value(recipe["source"].clone())
                    .map_err(|_| StorageError::ScopeChanged)?;
            if source.path != std::path::Path::new(path) {
                return Err(StorageError::ScopeChanged);
            }
            if let Some(need) = needs.iter_mut().find(|need| need.source == source) {
                need.owners.extend(owners.iter().cloned());
                if need.owners.len() > 64 {
                    return Err(StorageError::Busy);
                }
            } else {
                if needs.len() >= 64 {
                    return Err(StorageError::Busy);
                }
                needs.push(EdgeSourceNeed {
                    source,
                    owners: owners.clone(),
                });
            }
        }
    }
    if endpoint_owners.len() > 64 {
        return Err(StorageError::Busy);
    }
    if let Some(summary) = row
        .query
        .as_ref()
        .and_then(|query| query["rows"].get(&row.key))
        .and_then(|owner| owner["summary"].as_object())
    {
        if let Some(parent) = summary.get("parent").and_then(Value::as_str) {
            endpoint_owners.insert(parent.to_owned());
        }
        if let Some(children) = summary.get("children").and_then(Value::as_array) {
            for child in children {
                endpoint_owners
                    .insert(child.as_str().ok_or(StorageError::ScopeChanged)?.to_owned());
            }
        }
    }
    if endpoint_owners.len() > 64 {
        return Err(StorageError::Busy);
    }
    Ok((needs, endpoint_owners))
}

/// Resolve saved field dependencies by their exact publication source, rather
/// than applying the selected owner's History preference to every endpoint.
/// The boolean preserves each candidate's physical Catalog/History ownership.
/// Adapters load bounded metadata and qualify their own namespace/proof grants.
pub(crate) fn selected_edge_context(
    row: &CatalogRow,
    history: bool,
    mut candidate: impl FnMut(&str, bool) -> Result<Option<CatalogRow>, StorageError>,
    mut qualify: impl FnMut(&CatalogRow, bool) -> Result<(), StorageError>,
) -> Result<Vec<(CatalogRow, bool)>, StorageError> {
    let (needs, endpoint_owners) = edge_source_needs(row)?;
    let mut context = vec![(row.clone(), history)];
    let mut sources = BTreeMap::new();
    for source in &row.sources {
        if let Some(prior) = sources.insert(source.path.clone(), source.clone())
            && prior != *source
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    if sources.is_empty() || sources.len() > 64 {
        return Err(StorageError::Busy);
    }
    let mut candidates = BTreeMap::<(String, bool), Option<CatalogRow>>::new();
    let mut qualified = BTreeSet::new();
    for need in needs {
        match sources.get(&need.source.path) {
            Some(source) if source == &need.source => continue,
            Some(_) => return Err(StorageError::ScopeChanged),
            None => {}
        }
        let mut matched = false;
        for owner in &need.owners {
            for candidate_history in [history, !history] {
                let key = (owner.clone(), candidate_history);
                if !candidates.contains_key(&key) {
                    candidates.insert(key.clone(), candidate(owner, candidate_history)?);
                }
                let Some(row) = candidates.get(&key).cloned().flatten() else {
                    continue;
                };
                if row.key != *owner || !row.sources.contains(&need.source) {
                    continue;
                }
                if qualified.insert(key) {
                    qualify(&row, candidate_history)?;
                }
                let mut additions = Vec::new();
                let mut compatible = true;
                for source in &row.sources {
                    if let Some(prior) = sources.get(&source.path) {
                        if prior != source {
                            compatible = false;
                            break;
                        }
                    } else {
                        additions.push(source.clone());
                    }
                }
                if !compatible || sources.len() + additions.len() > 64 || context.len() >= 64 {
                    continue;
                }
                context.push((row, candidate_history));
                for source in additions {
                    sources.insert(source.path.clone(), source);
                }
                matched = true;
                break;
            }
            if matched {
                break;
            }
        }
        if !matched {
            return Err(StorageError::ScopeChanged);
        }
    }

    // Recipe sources above establish exact body ownership first. Endpoint
    // metadata is then added for saved parent/child fields and edge labels that
    // are not already represented by one of those owners.
    for owner in endpoint_owners {
        if owner == "you" || context.iter().any(|(row, _)| row.key == owner) {
            continue;
        }

        let attempts = if history { 2 } else { 1 };
        let mut selected = None;
        for candidate_history in [history, false].into_iter().take(attempts) {
            let key = (owner.clone(), candidate_history);
            if !candidates.contains_key(&key) {
                candidates.insert(key.clone(), candidate(&owner, candidate_history)?);
            }
            if let Some(row) = candidates.get(&key).cloned().flatten() {
                selected = Some((row, candidate_history));
                break;
            }
        }
        let Some((row, candidate_history)) = selected else {
            continue;
        };
        if row.key != owner {
            return Err(StorageError::ScopeChanged);
        }
        // A source-less native stub has no independent Query observation for
        // finite-window child membership. The saved parent result will filter
        // it as before.
        if row.query.is_none() {
            continue;
        }

        let mut additions = Vec::new();
        for source in &row.sources {
            if let Some(prior) = sources.get(&source.path) {
                if prior != source {
                    return Err(StorageError::ScopeChanged);
                }
            } else {
                additions.push(source.clone());
            }
        }
        if sources.len() + additions.len() > 64 || context.len() >= 64 {
            return Err(StorageError::Busy);
        }

        let key = (owner, candidate_history);
        if qualified.insert(key) {
            qualify(&row, candidate_history)?;
        }
        context.push((row, candidate_history));
        for source in additions {
            sources.insert(source.path.clone(), source);
        }
    }
    Ok(context)
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

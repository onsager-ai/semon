//! Backend-neutral Query metadata operations; derivation and native parsing stay shared.
use crate::{QueryError, storage::LogicalSourceNamespace};
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
    pub(crate) witnesses: Vec<(String, String)>,
    pub(crate) edges: BTreeSet<String>,
    pub(crate) turns: BTreeSet<String>,
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

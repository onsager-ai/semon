//! Immutable selected-presentation metadata. This port contains no SQL or Raw bytes.
#![allow(dead_code)] // Private qualification only; public factories stay closed.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(crate) trait ContextLookup {
    fn details(
        &self,
        key: &str,
        source_generation: &str,
    ) -> io::Result<Option<(Option<String>, bool)>>;
    fn relative(&self, key: &str) -> io::Result<Option<String>>;
    fn descendants(&self, key: &str) -> io::Result<Vec<String>>;
    fn header(&self, key: &str) -> io::Result<Option<(String, bool)>>;
    fn turn(&self, key: &str, id: &str) -> io::Result<Option<String>>;
    fn edge(&self, key: &str, id: &str) -> io::Result<Option<String>>;
    fn target_turns(&self, key: &str, id: &str) -> io::Result<Vec<String>>;
    fn participant(&self, key: &str) -> io::Result<Option<String>>;
    fn preview_header(&self, key: &str) -> io::Result<Option<(String, Option<String>)>>;
    fn preview_catalog(&self, key: &str) -> io::Result<Option<String>>;
}
pub(crate) fn selected_details(
    lookup: &dyn ContextLookup,
    key: &str,
    sources: &[crate::model::summary::CatalogSource],
    relative_status: &mut impl FnMut(&crate::model::summary::CatalogRow) -> Value,
) -> io::Result<Option<Value>> {
    let expected = crate::retention::source_generation(sources).map_err(io::Error::other)?;
    let encoded = lookup.details(key, &expected)?;
    encoded
        .and_then(|(text, current)| text.map(|text| (text, current)))
        .map(|(text, current)| {
            let mut value: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
            value["live_observation"] = json!(current);
            value["family"] = selected_family(lookup, key, relative_status)?;
            Ok(value)
        })
        .transpose()
}

/// Indexed, body-free relatives of the selected owner. This never walks an
/// unrelated workspace. A truncated family retains an explicit unknown total.
pub(crate) fn selected_family(
    lookup: &dyn ContextLookup,
    key: &str,
    relative_status: &mut impl FnMut(&crate::model::summary::CatalogRow) -> Value,
) -> io::Result<Value> {
    let mut bytes = 0;
    let incomplete = std::cell::Cell::new(false);
    let mut read = |sid: &str| -> io::Result<Option<Value>> {
        let text = lookup.relative(sid)?;
        let Some(text) = text else {
            incomplete.set(true);
            return Ok(None);
        };
        bytes += text.len();
        if text.len() > 65536 || bytes > 1048576 {
            incomplete.set(true);
            return Ok(None);
        }
        let mut row: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
        // Qualification uses the same bounded published scope as the relative.
        // Private native paths and Query recipes never enter the UI response.
        let scope = row
            .as_object_mut()
            .and_then(|row| row.remove("_status_catalog"))
            .and_then(|value| {
                serde_json::from_value::<crate::model::summary::CatalogRow>(value).ok()
            });
        row["status"] = scope
            .filter(|scope| scope.key == sid && row["harness"] == scope.harness)
            .map(|scope| relative_status(&scope))
            .unwrap_or_else(|| json!({"state":null,"observation":"unavailable"}));
        Ok(Some(row))
    };
    let mut ancestors = Vec::new();
    let mut seen = BTreeSet::from([key.to_owned()]);
    let mut parent = read(key)?.and_then(|row| row["parent"].as_str().map(str::to_owned));
    while let Some(sid) = parent.take() {
        if ancestors.len() >= 32 || !seen.insert(sid.clone()) {
            incomplete.set(true);
            break;
        }
        let Some(row) = read(&sid)? else {
            break;
        };
        parent = row["parent"].as_str().map(str::to_owned);
        ancestors.push(row);
    }
    ancestors.reverse();
    let mut runs = Vec::new();
    for sid in lookup.descendants(key)? {
        if seen.contains(&sid) {
            incomplete.set(true);
            continue;
        }
        if runs.len() >= 99 {
            incomplete.set(true);
            break;
        }
        if let Some(row) = read(&sid)? {
            runs.push(row);
        }
    }
    Ok(
        json!({"state":if incomplete.get() {"incomplete"} else {"complete"},"ancestors":ancestors,"runs":runs}),
    )
}

/// Relationships for the issued native range only. This shares the producer's
/// causal projection; it never constructs a graph or reads native bodies.
pub(crate) fn range_context(
    lookup: &dyn ContextLookup,
    key: &str,
    slots: &[crate::slot_projection::SavedSlot],
    sources: &[crate::model::summary::CatalogSource],
) -> io::Result<Value> {
    let mut ids = BTreeSet::new();
    let turn_ids: BTreeSet<_> = slots
        .iter()
        .filter_map(|slot| slot.turn.as_deref())
        .collect();
    for slot in slots {
        if let crate::slot_projection::Recipe::H { id } = &slot.recipe {
            ids.insert(id.clone());
        }
    }
    if !ids.is_empty() || !turn_ids.is_empty() {
        let expected = crate::retention::source_generation(sources).map_err(io::Error::other)?;
        let published = lookup.header(key)?.map(|header| header.0);
        if published.as_deref() != Some(&expected) {
            return Ok(
                json!({"state":"incomplete","turns":[],"handoffs":[],"sessions":{},"previews":{}}),
            );
        }
    }
    let mut missing = lookup.header(key)?.is_some_and(|header| header.1);
    let mut turns = Vec::new();
    let mut bytes = 0;
    for id in turn_ids {
        let encoded = lookup.turn(key, id)?;
        let Some(encoded) = encoded else {
            missing = true;
            continue;
        };
        bytes += encoded.len();
        if bytes > 4 * 1024 * 1024 {
            missing = true;
            break;
        }
        let turn: Value = serde_json::from_str(&encoded).map_err(io::Error::other)?;
        if turn["sent"].as_array().is_none_or(|sent| sent.len() > 1000) {
            missing = true;
            continue;
        }
        if let Some(start) = turn["start"].as_str() {
            ids.insert(start.to_owned());
        }
        turns.push(turn);
    }
    let mut edges = Vec::new();
    let mut owners = BTreeSet::new();
    let mut participants = BTreeSet::new();
    for id in ids {
        let encoded = lookup.edge(key, &id)?;
        let Some(encoded) = encoded else {
            missing = true;
            continue;
        };
        bytes += encoded.len();
        if bytes > 4 * 1024 * 1024 {
            missing = true;
            break;
        }
        let mut edge: Value = serde_json::from_str(&encoded).map_err(io::Error::other)?;
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
            let found = lookup.target_turns(target, &id)?;
            if found.len() == 1 {
                edge["target_turn"] = json!(found[0]);
            }
        }
        edges.push(edge);
    }
    let mut sessions = BTreeMap::new();
    if participants.len() > 201 {
        missing = true;
    }
    for sid in participants.into_iter().take(201) {
        let metadata = lookup.participant(&sid)?;
        if let Some(metadata) = metadata {
            bytes += metadata.len();
            if metadata.len() > 65536 || bytes > 4 * 1024 * 1024 {
                missing = true;
                continue;
            }
            sessions.insert(
                sid,
                serde_json::from_str::<Value>(&metadata).map_err(io::Error::other)?,
            );
        } else {
            missing = true;
        }
    }
    for edge in &edges {
        if edge["kind"] == "spawn"
            && let Some(target) = edge["to"].as_str()
            && let Some(session) = sessions.get(target)
            && session["state"] == "work"
            && session["live_observation"] == 1
            && !session["activity"].is_null()
        {
            owners.insert(target.to_owned());
        }
    }
    let mut previews = BTreeMap::new();
    if owners.len() > 128 {
        missing = true;
    }
    for sid in owners.into_iter().take(128) {
        let header = lookup.preview_header(&sid)?;
        let Some((generation, Some(content))) = header else {
            missing = true;
            continue;
        };
        let metadata = lookup.preview_catalog(&sid)?;
        let Some(metadata) = metadata else {
            missing = true;
            continue;
        };
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(&metadata).map_err(io::Error::other)?;
        if row.key != sid
            || crate::retention::source_generation(&row.sources).map_err(io::Error::other)?
                != generation
            || content.len() != 64
        {
            missing = true;
            continue;
        }
        previews.insert(sid, content);
    }
    Ok(
        json!({"state":if missing {"incomplete"} else {"complete"},"turns":turns,"handoffs":edges,"sessions":sessions,"previews":previews}),
    )
}

// Each method has its own typed, bounded metadata family. This is not a SQL
// recording/replay facade: backend coordinates and statements never cross it.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct CapturedContext {
    details: BTreeMap<String, Option<(Option<String>, bool)>>,
    relative: BTreeMap<String, Option<String>>,
    descendants: BTreeMap<String, Vec<String>>,
    header: BTreeMap<String, Option<(String, bool)>>,
    turn: BTreeMap<String, Option<String>>,
    edge: BTreeMap<String, Option<String>>,
    target_turns: BTreeMap<String, Vec<String>>,
    participant: BTreeMap<String, Option<String>>,
    preview_header: BTreeMap<String, Option<(String, Option<String>)>>,
    preview_catalog: BTreeMap<String, Option<String>>,
}
struct Capture<'a> {
    source: &'a dyn ContextLookup,
    data: std::cell::RefCell<CapturedContext>,
    bytes: std::cell::Cell<usize>,
}
macro_rules! captured_methods {
    ($($name:ident($($arg:ident),*) -> $result:ty;)+) => {
        impl ContextLookup for CapturedContext {
            $(fn $name(&self, $($arg: &str),*) -> io::Result<$result> {
                let key = serde_json::to_string(&($($arg,)* )).map_err(io::Error::other)?;
                self.$name.get(&key).cloned().ok_or_else(|| io::Error::other("missing immutable context witness"))
            })+
        }
        impl ContextLookup for Capture<'_> {
            $(fn $name(&self, $($arg: &str),*) -> io::Result<$result> {
                let key = serde_json::to_string(&($($arg,)* )).map_err(io::Error::other)?;
                if let Some(value) = self.data.borrow().$name.get(&key) { return Ok(value.clone()); }
                let value = self.source.$name($($arg),*)?;
                let bytes = self.bytes.get().saturating_add(key.len()).saturating_add(serde_json::to_vec(&value).map_err(io::Error::other)?.len());
                if bytes > 8 * 1048576 { return Err(io::Error::other("selected witness capacity")); }
                self.bytes.set(bytes);
                self.data.borrow_mut().$name.insert(key, value.clone());
                Ok(value)
            })+
        }
    }
}
captured_methods! {
    details(key, source) -> Option<(Option<String>, bool)>;
    relative(key) -> Option<String>;
    descendants(key) -> Vec<String>;
    header(key) -> Option<(String, bool)>;
    turn(key, id) -> Option<String>;
    edge(key, id) -> Option<String>;
    target_turns(key, id) -> Vec<String>;
    participant(key) -> Option<String>;
    preview_header(key) -> Option<(String, Option<String>)>;
    preview_catalog(key) -> Option<String>;
}
impl CapturedContext {
    pub(crate) fn capture(
        lookup: &dyn ContextLookup,
        row: &crate::model::summary::CatalogRow,
        slots: &[crate::slot_projection::SavedSlot],
        status: &mut impl FnMut(&crate::model::summary::CatalogRow) -> Value,
    ) -> io::Result<Self> {
        let capture = Capture {
            source: lookup,
            data: Default::default(),
            bytes: std::cell::Cell::new(0),
        };
        selected_details(&capture, &row.key, &row.sources, status)?;
        // Capture dependencies of every possible bounded window, including the
        // empty window. Lookup caching avoids repeating SQL for shared turns.
        range_context(&capture, &row.key, &[], &row.sources)?;
        for slot in slots {
            range_context(&capture, &row.key, std::slice::from_ref(slot), &row.sources)?;
        }
        Ok(capture.data.into_inner())
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresentationWitness {
    pub(crate) header_generation: String,
    pub(crate) version: u32,
    pub(crate) total: usize,
    pub(crate) has_history: bool,
    pub(crate) catalog: BTreeMap<String, String>,
    pub(crate) context: CapturedContext,
    pub(crate) relative_statuses: BTreeMap<String, Value>,
    // Loaded from the authenticated once-per-publication manifest, never
    // duplicated in each selected row's persisted presentation.
    #[serde(skip)]
    pub(crate) observations: PresentationObservations,
}

/// Body-free runtime and source-selection inputs captured by the real model.
/// They enter its private staged model witness and publication manifest together.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresentationObservations {
    pub(crate) runtime: Option<String>,
    pub(crate) facts_known: bool,
    pub(crate) selection_known: bool,
    pub(crate) machine_label: Option<String>,
    sources: BTreeMap<String, SourceObservation>,
    pub(crate) relative_statuses: BTreeMap<String, Value>,
    /// Request-local capture authority. Neither physical roots nor an open
    /// Facts connection can enter the durable witness or a leased read.
    #[serde(skip)]
    authority: Option<std::sync::Arc<PresentationAuthority>>,
}
struct PresentationAuthority {
    options: crate::Options,
    recorded: Option<std::sync::Mutex<crate::facts::source_authority::Sources>>,
}
impl std::fmt::Debug for PresentationAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PresentationAuthority")
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceObservation {
    relative: String,
    current_codex: Option<bool>,
    cached: bool,
}
impl PresentationObservations {
    #[cfg(test)]
    pub(crate) fn captured_source_count(&self) -> usize {
        self.sources.len()
    }

    pub(crate) fn capture<'a>(
        options: &crate::Options,
        binding: Option<&crate::storage::QueryRuntimeBinding>,
        rows: impl Iterator<Item = &'a crate::model::summary::CatalogRow>,
    ) -> Self {
        let recorded = options
            .facts
            .as_ref()
            .and_then(|path| crate::facts::source_authority::Sources::sqlite(path).ok());
        Self::capture_with_authority(options, binding, rows, recorded)
    }
    pub(crate) fn capture_with_authority<'a>(
        options: &crate::Options,
        binding: Option<&crate::storage::QueryRuntimeBinding>,
        rows: impl Iterator<Item = &'a crate::model::summary::CatalogRow>,
        recorded: Option<crate::facts::source_authority::Sources>,
    ) -> Self {
        let runtime = crate::native_evidence::query_runtime_generation(options).ok();
        let mut facts_known =
            options.facts.is_none() || recorded.as_ref().is_some_and(|source| source.facts_known());
        let mut selection_known = options.facts.is_none()
            || recorded
                .as_ref()
                .is_some_and(|source| source.selection_known());
        let mut machine_label = match &options.facts {
            None => Some(crate::facts::MachineFacts::Local.hostname(options)),
            Some(_) => recorded.as_ref().and_then(|source| source.hostname()),
        };
        let mut sources = BTreeMap::new();
        // Only this bounded cohort and its typed family endpoints participate.
        // No full-host manifest scan, absolute home root, or second Facts read.
        for row in rows.take(128) {
            for source in row.sources.iter().take(64) {
                let Some(relative) = crate::catalog::source_relative(options, &row.harness, source)
                else {
                    continue;
                };
                let current_codex = if row.harness == "codex" {
                    match recorded
                        .as_ref()
                        .map(|head| head.codex_current(&relative))
                        .transpose()
                    {
                        Ok(value) => value.flatten(),
                        Err(_) => {
                            selection_known = false;
                            None
                        }
                    }
                } else {
                    None
                };
                if let Ok(key) = Self::source_key(source) {
                    sources.insert(
                        key,
                        SourceObservation {
                            relative,
                            current_codex,
                            cached: crate::catalog::source_state(source) == "cached",
                        },
                    );
                }
            }
        }
        if recorded
            .as_ref()
            .is_some_and(|head| head.validate().is_err())
        {
            facts_known = false;
            selection_known = false;
            machine_label = None;
            sources.clear();
        }
        let runtime = runtime
            .filter(|initial| {
                crate::native_evidence::query_runtime_generation(options)
                    .ok()
                    .as_ref()
                    == Some(initial)
            })
            .and_then(|runtime| binding.and_then(|binding| binding.witness(&runtime).ok()));
        Self {
            runtime,
            facts_known,
            selection_known,
            machine_label,
            sources,
            relative_statuses: BTreeMap::new(),
            authority: Some(std::sync::Arc::new(PresentationAuthority {
                options: options.clone(),
                recorded: recorded.map(std::sync::Mutex::new),
            })),
        }
    }
    fn source_key(source: &crate::model::summary::CatalogSource) -> io::Result<String> {
        crate::retention::source_generation(std::slice::from_ref(source)).map_err(io::Error::other)
    }
    pub(crate) fn retired(&self, source: &crate::model::summary::CatalogSource) -> bool {
        Self::source_key(source)
            .ok()
            .and_then(|key| self.sources.get(&key))
            .is_some_and(|saved| saved.current_codex == Some(false))
    }
    pub(crate) fn capture_relative_status(
        &mut self,
        row: &crate::model::summary::CatalogRow,
    ) -> Value {
        // Publication's indexed family lookup may include a relative outside
        // the model's incident endpoint page. Qualify only those actual paths
        // against the SAME still-open authority, never a new Facts snapshot.
        // Leased readers deserialize authority=None and only use saved status.
        let captured = (|| -> io::Result<()> {
            let keys = row
                .sources
                .iter()
                .take(64)
                .map(Self::source_key)
                .collect::<io::Result<Vec<_>>>()?;
            // A completed observation remains self-contained even after the
            // live Facts file changes. Reopening or validating authority here
            // would turn authenticated A data into an unnecessary unavailable
            // result despite no lazy lookup being requested.
            if keys.iter().all(|key| self.sources.contains_key(key)) {
                return Ok(());
            }
            let Some(authority) = &self.authority else {
                return Ok(());
            };
            let recorded = authority
                .recorded
                .as_ref()
                .map(|head| {
                    head.lock()
                        .map_err(|_| io::Error::other("selected source authority unavailable"))
                })
                .transpose()?;
            if let Some(recorded) = &recorded {
                recorded.validate()?;
            }
            for source in row.sources.iter().take(64) {
                let key = Self::source_key(source)?;
                if self.sources.contains_key(&key) {
                    continue;
                }
                if self.sources.len() >= 8192 {
                    return Err(io::Error::other("selected source observation bound"));
                }
                let relative =
                    crate::catalog::source_relative(&authority.options, &row.harness, source)
                        .ok_or_else(|| io::Error::other("selected relative source unavailable"))?;
                let current_codex = if row.harness == "codex" {
                    recorded
                        .as_ref()
                        .map(|head| head.codex_current(&relative))
                        .transpose()?
                        .flatten()
                } else {
                    None
                };
                self.sources.insert(
                    key,
                    SourceObservation {
                        relative,
                        current_codex,
                        cached: crate::catalog::source_state(source) == "cached",
                    },
                );
            }
            if let Some(recorded) = &recorded {
                recorded.validate()?;
            }
            Ok(())
        })();
        if captured.is_err() {
            let unavailable = json!({"state":null,"observation":"unavailable"});
            self.relative_statuses
                .insert(row.key.clone(), unavailable.clone());
            return unavailable;
        }
        let status = self.observed_relative_status(row);
        self.relative_statuses
            .insert(row.key.clone(), status.clone());
        status
    }
    fn observed_relative_status(&self, row: &crate::model::summary::CatalogRow) -> Value {
        if row.sources.is_empty()
            || row.sources.len() > 64
            || (row.lifecycle == crate::model::summary::CatalogLifecycle::Current
                && (!self.facts_known || (row.harness == "codex" && !self.selection_known)))
            || row.sources.iter().any(|source| {
                Self::source_key(source)
                    .ok()
                    .and_then(|key| self.sources.get(&key))
                    .is_none()
            })
        {
            return json!({"state":null,"observation":"unavailable"});
        }
        crate::catalog::recorded_status(
            row,
            self.runtime.as_deref(),
            row.sources.iter().all(|source| {
                Self::source_key(source)
                    .ok()
                    .and_then(|key| self.sources.get(&key))
                    .is_some_and(|saved| saved.cached)
            }),
            self.facts_known,
            row.harness == "codex" && row.sources.iter().any(|source| self.retired(source)),
        )
        .unwrap_or_else(|_| json!({"state":null,"observation":"unavailable"}))
    }
    fn relative_status(&self, row: &crate::model::summary::CatalogRow) -> Value {
        self.relative_statuses
            .get(&row.key)
            .cloned()
            .unwrap_or_else(|| json!({"state":null,"observation":"unavailable"}))
    }
}

/// Constructed explicitly by the host alongside its authorized source mapping.
/// SQL cannot manufacture or change the source_key or Raw roots in this binding.
#[derive(Clone)]
pub(crate) struct HostSourceBinding {
    pub(crate) source_key: String,
    pub(crate) snapshot: crate::storage::proof_lifetime::ReaderSnapshot,
}
impl HostSourceBinding {
    pub(crate) fn validate(
        &self,
        snapshot: &crate::storage::proof_lifetime::ReaderSnapshot,
    ) -> io::Result<()> {
        if self.source_key.is_empty()
            || self.source_key.len() > 4096
            || self.snapshot.namespace != snapshot.namespace
            || self.snapshot.admission != snapshot.admission
            || self.snapshot.stream_epoch != snapshot.stream_epoch
            || self.snapshot.publication_generation != snapshot.publication_generation
            || self.snapshot.proof_generation != snapshot.proof_generation
        {
            return Err(io::Error::other("selected host binding changed"));
        }
        Ok(())
    }
}

pub(crate) struct ImmutableCatalog<'a> {
    pub(crate) row: &'a crate::model::summary::CatalogRow,
    pub(crate) witness: &'a PresentationWitness,
    pub(crate) snapshot: &'a crate::storage::proof_lifetime::ReaderSnapshot,
}
impl crate::storage::catalog::CatalogSnapshot for ImmutableCatalog<'_> {
    fn facts_authority<'a>(
        &'a self,
        _: &std::path::Path,
    ) -> io::Result<Box<dyn crate::facts::source_authority::SourceAuthority + 'a>> {
        Err(io::Error::other("immutable presentation has no live Facts authority"))
    }
    fn presentation_observations(&self) -> Option<&PresentationObservations> {
        Some(&self.witness.observations)
    }
    // The publication validated these consumed ledgers. Physical liveness is
    // requalified by Records, never used to rewrite the pinned presentation.
    fn source_state(&self, source: &crate::model::summary::CatalogSource) -> Option<&'static str> {
        self.row
            .sources
            .iter()
            .any(|saved| saved == source)
            .then_some("cached")
    }
    fn runtime_binding(&self) -> Option<crate::storage::QueryRuntimeBinding> {
        Some(crate::storage::QueryRuntimeBinding {
            namespace: self.snapshot.namespace.clone(),
            admission: self.snapshot.admission,
            stream_epoch: self.snapshot.stream_epoch.clone(),
        })
    }
    fn require_current(&self) -> io::Result<()> {
        Ok(())
    }
    fn publication_value(&self, key: &str) -> io::Result<Option<String>> {
        Ok(self.witness.catalog.get(key).cloned())
    }
    fn rows(
        &self,
        request: &crate::catalog::Request,
    ) -> io::Result<Vec<crate::model::summary::CatalogRow>> {
        if request.sid.as_deref() != Some(self.row.key.as_str()) {
            return Err(io::Error::other("immutable selected scope"));
        }
        // Exhaustive scope matching keeps any future scope closed until its
        // immutable selection semantics are deliberately implemented here.
        match request.scope {
            crate::CatalogReadScope::Current | crate::CatalogReadScope::RetainedHistory => {
                Ok(vec![self.row.clone()])
            }
        }
    }
    fn search(
        &self,
        _: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection> {
        Err(io::Error::other(
            "selected snapshot has no search authority",
        ))
    }
    fn has_history(&self, key: &str) -> io::Result<bool> {
        Ok(key == self.row.key && self.witness.has_history)
    }
    // The leased generation already owns its original row and recipes. Never
    // replace it with today's composite History Catalog or live facts.
    fn history_catalog(&self, _: &str) -> io::Result<Option<String>> {
        Ok(None)
    }
    fn history_qualified(&self, row: &crate::model::summary::CatalogRow) -> io::Result<bool> {
        Ok(row.key == self.row.key)
    }
    fn selected_details(
        &self,
        key: &str,
        sources: &[crate::model::summary::CatalogSource],
        _status: &mut dyn FnMut(&crate::model::summary::CatalogRow) -> Value,
    ) -> io::Result<Option<Value>> {
        selected_details(&self.witness.context, key, sources, &mut |row| {
            self.witness.observations.relative_status(row)
        })
    }
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        _history: bool,
    ) -> io::Result<Option<Value>> {
        if key != self.row.key
            || self
                .witness
                .catalog
                .get("slot_projection_catalog_generation")
                .map(String::as_str)
                != Some(generation)
        {
            return Ok(None);
        }
        Ok(Some(
            json!({"generation":crate::transcript_range::entry_generation(&self.witness.header_generation),"total":self.witness.total}),
        ))
    }
}

pub(crate) struct SelectedRead {
    pub(crate) snapshot: crate::storage::proof_lifetime::ReaderSnapshot,
    pub(crate) row: crate::model::summary::CatalogRow,
    pub(crate) witness: PresentationWitness,
    pub(crate) slots: Vec<crate::slot_projection::SavedSlot>,
    pub(crate) sources: crate::query_source::Sources,
}
pub(crate) trait SelectedSnapshotStore:
    crate::storage::proof_lifetime::ProofReaderLease
{
    fn open_selected(
        &self,
        options: &crate::Options,
        host: &HostSourceBinding,
        control: &crate::SelectedReadControl,
    ) -> io::Result<SelectedRead>;
    fn finish_selected(&self, control: &crate::SelectedReadControl) -> io::Result<()>;
}

pub(crate) struct Controlled<'a> {
    pub(crate) reader: &'a dyn crate::SessionSourceReader,
    pub(crate) control: &'a crate::SelectedReadControl,
}
impl crate::SessionSourceReader for Controlled<'_> {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        self.control.check()?;
        let value = self.reader.read_range(source, expected, offset, max)?;
        self.control.check()?;
        Ok(value)
    }
}

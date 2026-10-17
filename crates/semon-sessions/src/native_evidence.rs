//! Native evidence ownership, independent of Viewer transport and refresh workers.
//! Viewer, Catalog and Query share one complete source index per machine.
use crate::{
    Node, Options,
    events::EventCache,
    model::{self, Texts},
    proc_start,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

mod runtime;
pub(crate) use runtime::RuntimeObservations;

pub(crate) struct NativeEvidence {
    pub(crate) options: Options,
    pub(crate) work: Mutex<Work>,
    pub(crate) catalog_demand: AtomicBool,
    query: RwLock<Option<Arc<QueryCache>>>,
    closed: AtomicBool,
    audit_request: AtomicU64,
    audit_completed: AtomicU64,
}

pub(crate) enum CatalogPreparation {
    Pending,
    PublishedPending,
    Reconcile,
    Ready(String),
}

/// Native source ownership for an admitted machine set. This constructs no
/// Viewer state, refresh pool or worker and opens no files. Embedding hosts keep
/// their authorization, admission and source-custody guards around actual reads.
/// Options are preserved, including output windows and received machine facts.
pub struct SessionSources {
    pub(crate) machines: Vec<(String, Arc<NativeEvidence>)>,
    views: Vec<OnceLock<Arc<crate::viewer::MachineView>>>,
    pub(crate) inventory: Arc<crate::SessionSourceInventory>,
    retired: AtomicBool,
}

impl SessionSources {
    pub fn with_machines(machines: Vec<(String, Options)>) -> Self {
        Self::from_evidence(
            machines
                .into_iter()
                .map(|(key, options)| (key, NativeEvidence::new(options)))
                .collect(),
        )
    }

    /// Adopt existing Catalog evidence owners without closing their producers,
    /// reopening indexes or creating a second writer. The host transfers their
    /// lifecycle ownership into this machine set: retiring or closing this set
    /// also retires the original observers. This grants no source authority.
    pub fn from_catalog_observers(machines: Vec<(String, &crate::SessionCatalogObserver)>) -> Self {
        let views: Vec<_> = machines
            .into_iter()
            .map(|(key, observer)| (key, observer.shared_view()))
            .collect();
        let sources = Self::from_evidence(
            views
                .iter()
                .map(|(key, view)| (key.clone(), view.evidence.clone()))
                .collect(),
        );
        for (slot, (_, view)) in sources.views.iter().zip(views) {
            let _ = slot.set(view);
        }
        sources
    }

    fn from_evidence(machines: Vec<(String, Arc<NativeEvidence>)>) -> Self {
        let local = machines.len() == 1 && machines[0].0.is_empty();
        let inventory = crate::SessionSourceInventory::new(machines.iter().map(|(key, _)| {
            let key = if local {
                "local".to_owned()
            } else {
                key.clone()
            };
            (key.clone(), key)
        }));
        let views = machines.iter().map(|_| OnceLock::new()).collect();
        Self {
            machines,
            views,
            inventory: Arc::new(inventory),
            retired: AtomicBool::new(false),
        }
    }

    /// Stop native refreshes without blocking. A warm query retains its last
    /// cohort; its source-backed reads still verify evidence independently.
    pub fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        for (_, evidence) in &self.machines {
            evidence.retire();
        }
        for view in self.views.iter().filter_map(OnceLock::get) {
            view.retire();
        }
    }

    /// Drain native preparation before custody changes. The host first prevents
    /// new requests and closes attached Viewer transports, if any. This drains
    /// the shared native/catalog producers too. Async hosts call it off-reactor.
    pub fn close(&self) {
        self.retire();
        for view in self.views.iter().filter_map(OnceLock::get) {
            view.close();
        }
        for (_, evidence) in &self.machines {
            drop(lock(&evidence.work));
        }
    }

    /// Queue the existing source-scoped Catalog producer without a ViewerCore
    /// or native body reads. The host authorizes custody and supplies its bounded
    /// refresh pool. Query and any later Viewer borrow this same producer/index.
    pub fn demand_catalog_source(
        &self,
        source_key: &str,
        pool: Arc<crate::RefreshPool>,
    ) -> io::Result<bool> {
        self.catalog_source(source_key, pool, true)
    }

    /// Register interest after a host has read qualified persisted evidence.
    /// Existing admission, source identity and retirement checks still apply.
    pub fn watch_catalog_source(
        &self,
        source_key: &str,
        pool: Arc<crate::RefreshPool>,
    ) -> io::Result<bool> {
        self.catalog_source(source_key, pool, false)
    }

    fn catalog_source(
        &self,
        source_key: &str,
        pool: Arc<crate::RefreshPool>,
        prepare: bool,
    ) -> io::Result<bool> {
        if self.is_retired() || !self.inventory.valid() {
            return Ok(false);
        }
        let Some(index) = self.inventory.index(source_key) else {
            return Ok(false);
        };
        let view = self.view(index);
        view.set_pool(pool);
        view.set_refresh(crate::Refresh::OnInvalidate);
        if self.is_retired() {
            view.retire();
            return Ok(false);
        }
        if prepare {
            view.note_catalog_read()?;
        } else {
            view.note_catalog_interest()?;
        }
        Ok(true)
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    /// Notify an already demanded source producer after an authorized push.
    /// This never constructs a producer or starts compatibility warming.
    /// True means the source is configured and open, even before demand.
    pub fn invalidate(&self, machine: &str) -> bool {
        self.invalidate_inner(machine, false)
    }

    /// Notify after exact changes were durably recorded by the source writer.
    pub fn invalidate_journaled(&self, machine: &str) -> bool {
        self.invalidate_inner(machine, true)
    }

    fn invalidate_inner(&self, machine: &str, journaled: bool) -> bool {
        if self.is_retired() {
            return false;
        }
        let Some(index) = self.machines.iter().position(|(key, _)| key == machine) else {
            return false;
        };
        if self.machines[index].1.closed.load(Ordering::Acquire) {
            return false;
        }
        let Some(view) = self.views[index].get() else {
            if !journaled {
                self.machines[index].1.request_metadata_audit();
            }
            return true;
        };
        if journaled {
            view.invalidate_journaled();
        } else {
            view.invalidate();
        }
        true
    }

    fn view(&self, index: usize) -> Arc<crate::viewer::MachineView> {
        self.views[index]
            .get_or_init(|| {
                crate::viewer::MachineView::from_evidence(self.machines[index].1.clone())
            })
            .clone()
    }

    pub(crate) fn machine_views(&self) -> Vec<(String, Arc<crate::viewer::MachineView>)> {
        self.machines
            .iter()
            .enumerate()
            .map(|(index, (key, _))| (key.clone(), self.view(index)))
            .collect()
    }

    pub(crate) fn query_at(&self, now: i64) -> io::Result<crate::union::QueryRead> {
        let parts = self
            .machines
            .iter()
            .map(|(key, evidence)| Ok((key.clone(), evidence.query_at(now)?)))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(crate::union::QueryRead::new(parts, usize::MAX))
    }
}

impl NativeEvidence {
    /// Construction is lazy: no source, cache or refresh worker is opened.
    pub(crate) fn new(options: Options) -> Arc<Self> {
        Arc::new(Self {
            options,
            work: Mutex::default(),
            catalog_demand: AtomicBool::new(false),
            query: RwLock::default(),
            closed: AtomicBool::new(false),
            audit_request: AtomicU64::new(0),
            audit_completed: AtomicU64::new(0),
        })
    }

    pub(crate) fn request_metadata_audit_if_idle(&self) {
        let completed = self.audit_completed.load(Ordering::Acquire);
        let _ = self.audit_request.compare_exchange(
            completed,
            completed.wrapping_add(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn finish_audit(
        &self,
        work: &mut Work,
        now: i64,
        before_build: &mut impl FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        let generation = work
            .events
            .as_ref()
            .and_then(|cache| cache.session_catalog_generation());
        if work.audit.is_some()
            && !crate::events::store::note_catalog_audit(&self.options, generation.as_deref(), now)?
        {
            return Ok(true);
        }
        // A peer can publish and consume the journal before this owner audits
        // its source stamps. That is a new observation for this owner without
        // another derivation or publication. Successful own publications already
        // acknowledge their generation; a later peer advance remains observable.
        if let Some(generation) = generation {
            if work
                .observed_catalog_generation
                .as_ref()
                .is_some_and(|observed| observed != &generation)
            {
                before_build()?;
                work.built_at = Some(Instant::now());
            }
            if work.observed_catalog_generation.as_ref() != Some(&generation) {
                work.observed_catalog_generation = Some(generation);
            }
        }
        let Some(audit) = work.audit.take() else {
            return Ok(false);
        };
        work.audit_revision = audit.revision;
        self.audit_completed
            .store(audit.revision, Ordering::Release);
        work.catalog_snapshot = audit.snapshot;
        Ok(self.audit_request.load(Ordering::Acquire) != work.audit_revision)
    }

    pub(crate) fn request_metadata_audit(&self) {
        self.audit_request.fetch_add(1, Ordering::AcqRel);
    }

    /// Retired sources can only answer from their last verified cohort.
    /// The embedding host still waits out readers before removing source custody.
    pub(crate) fn retire(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Initialize the source index and publish a bounded cold catalog page.
    pub(crate) fn prepare_events(&self, work: &mut Work, now: i64) -> io::Result<()> {
        if work
            .events
            .as_ref()
            .is_some_and(|cache| cache.scoped_store_lost())
        {
            work.events = None;
        }
        if work.events.is_none() {
            let _timing = crate::native_timing::Phase::new(&self.options.cache, "index_open");
            work.events = Some(match EventCache::open_scoped(&self.options.cache) {
                Ok(cache) => cache,
                Err(error) if self.catalog_demand.load(Ordering::Relaxed) => {
                    return Err(io::Error::other(error));
                }
                Err(_) => EventCache::open(&self.options.cache),
            });
        }
        let cache = work.events.as_mut().expect("event cache initialized");
        // The index commits each file's change as it reads it: nothing is
        // left to save.
        let mut dirty = false;
        if self.options.facts.is_none() {
            cache.refresh_reported_runs(&self.options.claude_json, now, &mut dirty);
        }
        Ok(())
    }

    /// Advance cold candidate discovery without reading native bodies. The
    /// latest page is published after discovery finishes, so a flat directory's
    /// enumeration order cannot displace recent sessions with old history.
    /// Workers yield after 512 entries or 64 accepted candidates; only the 64
    /// newest candidates remain resident between steps.
    pub(crate) fn prepare_catalog(
        &self,
        work: &mut Work,
        now: i64,
    ) -> io::Result<CatalogPreparation> {
        let _timing = crate::native_timing::Phase::new(&self.options.cache, "cold_total");
        self.prepare_events(work, now)?;
        let cache = work.events.as_mut().expect("event cache initialized");
        let state = cache.backfill_state()?;
        if !cache.is_scoped() {
            work.cold_catalog = None;
            return Ok(CatalogPreparation::Reconcile);
        }
        if (cache.session_catalog_generation().is_some() && state.is_none())
            || state.as_deref() == Some("complete")
        {
            crate::events::store::begin_catalog_recovery(
                &self.options,
                "unsupported-publication-or-dependency-gap",
            )?;
            work.cold_catalog = None;
            return Ok(CatalogPreparation::Pending);
        }
        let discovery = work.cold_catalog.get_or_insert_with(|| ColdCatalog {
            revision: self.audit_request.load(Ordering::Acquire),
            ..Default::default()
        });
        if state.as_deref() != Some("draining") {
            let recovery_complete = {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "recovery_discovery");
                crate::events::store::discover_recovery_sources(&self.options)?
            };
            let page = {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "path_discovery");
                discovery.cursor.next(&self.options, 64, 512)?
            };
            debug_assert!(page.examined <= 512);
            {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "discovery_publication");
                cache.discover_catalog_sources(
                    &self.options,
                    &page.inputs,
                    page.complete && recovery_complete,
                )?;
            }
            for input in page.inputs {
                if !input.path.ends_with(".jsonl") {
                    continue;
                }
                let modified = fs::metadata(input.full_path(&self.options))
                    .and_then(|metadata| metadata.modified())
                    .ok();
                discovery
                    .newest
                    .insert((std::cmp::Reverse(modified), input));
                if discovery.newest.len() > 64 {
                    discovery.newest.pop_last();
                }
            }
            if !page.complete || !recovery_complete {
                return Ok(CatalogPreparation::Pending);
            }
        }
        let pending_inputs = cache.pending_catalog_sources(64)?;
        if let Some(path) = pending_inputs.iter().find(|path| path.ends_with(".json")) {
            cache.resolve_catalog_input(&self.options, path)?;
            return Ok(CatalogPreparation::Pending);
        }
        let inputs = if cache.session_catalog_generation().is_none() && !discovery.newest.is_empty()
        {
            std::mem::take(&mut discovery.newest)
                .into_iter()
                .map(|(_, input)| input)
                .collect::<Vec<_>>()
        } else {
            cache
                .pending_catalog_sources(64)?
                .into_iter()
                .map(|path| {
                    catalog_input(Path::new(&path), &self.options)
                        .ok_or_else(|| io::Error::other("backfill source outside allowlist"))
                })
                .collect::<io::Result<Vec<_>>>()?
        };
        if !inputs.is_empty() || cache.session_catalog_generation().is_none() {
            let snapshot = discovery
                .snapshot
                .get_or_insert_with(|| Snapshot::sources(&self.options, &[]));
            snapshot.extend_sources(&self.options, &inputs);
            let seeds = inputs;
            let prepared = {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "cold_component");
                cache.scan_byte_budget = Some(4 * 1024 * 1024);
                let prepared = self.prepare_catalog_component_mode(cache, seeds.clone(), true, now);
                cache.scan_byte_budget = None;
                if let Some(prepared) = prepared? {
                    prepared
                } else {
                    let paths = seeds
                        .iter()
                        .map(|input| {
                            input
                                .full_path(&self.options)
                                .to_string_lossy()
                                .into_owned()
                        })
                        .collect::<Vec<_>>();
                    if cache
                        .begin_semantic_stage(&paths, &query_runtime_generation(&self.options)?)?
                        != crate::events::Outcome::Written
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "semantic stage changed",
                        ));
                    }
                    cache.end_scan();
                    cache.release_published_indexes();
                    work.texts.release_published_derivations();
                    return Ok(CatalogPreparation::Pending);
                }
            };
            discovery.pids.extend(prepared.pids.iter().copied());
            let (outcome, _) = cache.publish_partial_catalog_fenced(&prepared)?;
            if outcome == crate::events::Outcome::Deferred {
                cache.end_scan();
                cache.release_published_indexes();
                work.texts.release_published_derivations();
                return Ok(CatalogPreparation::Pending);
            }
            if outcome != crate::events::Outcome::Written {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "backfill publication changed",
                ));
            }
            let _timing = crate::native_timing::Phase::new(&self.options.cache, "cold_cleanup");
            crate::events::store::requeue_concurrent_sources(&self.options, &prepared)?;
            cache.publish_session_descriptions();
            cache.end_scan();
            cache.release_published_indexes();
            work.texts.release_published_derivations();
        }
        if !cache.pending_catalog_sources(1)?.is_empty() {
            if !discovery.started {
                discovery.started = true;
                return Ok(CatalogPreparation::PublishedPending);
            }
            return Ok(CatalogPreparation::Pending);
        }
        Ok(CatalogPreparation::Ready(
            cache
                .session_catalog_generation()
                .ok_or_else(|| io::Error::other("backfill generation missing"))?,
        ))
    }

    /// Resume semantic work before the ordinary journal drain. Source claims
    /// remain pending until the epoch's relationships and owner projections are
    /// qualified; losing this Work does not lose any progress or grant readiness.
    fn advance_semantic_stage(
        &self,
        work: &mut Work,
        now: i64,
        before_build: &mut impl FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        use model::relation_stage::Phase;
        let cache = work.events.as_mut().expect("event cache initialized");
        if !cache.is_scoped() {
            return Ok(false);
        }
        let runtime = query_runtime_generation(&self.options)?;
        match cache.begin_semantic_stage(&[], &runtime)? {
            crate::events::Outcome::Conflict => return Ok(false),
            crate::events::Outcome::Written => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "semantic index changed",
                ));
            }
        }
        let Some(batch) = cache.semantic_batch(64)? else {
            return Ok(false);
        };
        work.pending_sources = true;
        // Runtime/facts qualification also covers scalar reductions and the
        // durable final acknowledgement, which may have no selected sources.
        before_build()?;
        work.built_at = Some(Instant::now());
        if batch.paths.is_empty() {
            if batch.phase == Phase::Publication
                && cache.finish_semantic_stage(batch.epoch)? != crate::events::Outcome::Written
            {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "semantic completion changed",
                ));
            }
            return Ok(true);
        }
        let mut paths: BTreeSet<_> = batch.paths.iter().cloned().collect();
        if batch.phase == Phase::Relations {
            paths.extend(batch.context.parent_paths(&batch.paths));
            paths.extend(batch.context.companions.iter().cloned());
        }
        if paths.len() > 64 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "semantic matching source budget exceeded",
            ));
        }
        let inputs = paths
            .iter()
            .map(|path| {
                catalog_input(Path::new(path), &self.options)
                    .ok_or_else(|| io::Error::other("semantic source outside native allowlist"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        cache.scan_byte_budget =
            matches!(batch.phase, Phase::Indexing | Phase::Identity).then_some(4 * 1024 * 1024);
        let catalog_options = Options {
            scan_window: false,
            ..self.options.clone()
        };
        if let Some(discovery) = &mut work.cold_catalog {
            discovery
                .snapshot
                .get_or_insert_with(|| Snapshot::sources(&self.options, &[]))
                .extend_sources(&self.options, &inputs);
        }
        let prepared = if batch.phase == Phase::Indexing {
            model::prepare_semantic_index_sources(&catalog_options, cache, &inputs, now)
        } else if batch.phase == Phase::Identity {
            model::prepare_semantic_identity_sources(
                &catalog_options,
                cache,
                &inputs,
                &batch.context,
                now,
            )
        } else {
            model::prepare_staged_sources(
                &catalog_options,
                cache,
                &inputs,
                &batch.context,
                batch.phase == Phase::Relations,
                now,
            )
        };
        cache.scan_byte_budget = None;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) if model::relation_stage::source_generation_changed(&error) => {
                // Scanning committed the newer Ledger and exact dirty claim.
                // The next bounded batch resets the old epoch before replay.
                cache.end_scan();
                cache.release_published_indexes();
                work.texts.release_published_derivations();
                return Ok(true);
            }
            Err(error) => return Err(error),
        };
        if let Some(discovery) = &mut work.cold_catalog {
            discovery.pids.extend(prepared.pids.iter().copied());
        }
        let (outcome, generation) = if batch.phase == Phase::Publication {
            cache.publish_semantic_catalog(&batch, &prepared)?
        } else {
            (cache.record_semantic_batch(&batch, &prepared)?, None)
        };
        if outcome == crate::events::Outcome::Deferred {
            cache.end_scan();
            cache.release_published_indexes();
            work.texts.release_published_derivations();
            return Ok(true);
        }
        if outcome != crate::events::Outcome::Written {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "semantic publication changed",
            ));
        }
        if batch.phase == Phase::Publication {
            work.observed_catalog_generation = generation;
            crate::events::store::requeue_concurrent_sources(&self.options, &prepared)?;
            cache.publish_session_descriptions();
        }
        cache.end_scan();
        cache.release_published_indexes();
        work.texts.release_published_derivations();
        Ok(true)
    }

    /// Expand and derive the same bounded component for Catalog refresh and
    /// ordinary selected local reads. Publication retains its existing fences.
    fn prepare_catalog_component(
        &self,
        cache: &mut EventCache,
        inputs: Vec<crate::inputs::Input>,
        now: i64,
    ) -> io::Result<Option<model::PreparedSources>> {
        self.prepare_catalog_component_mode(cache, inputs, false, now)
    }
    fn prepare_catalog_component_mode(
        &self,
        cache: &mut EventCache,
        inputs: Vec<crate::Input>,
        recovering: bool,
        now: i64,
    ) -> io::Result<Option<model::PreparedSources>> {
        let timing = crate::native_timing::enabled();
        let mut measurements = crate::native_timing::Measurements::new(timing);
        let total_started = measurements.start();
        if inputs.is_empty() && recovering {
            let started = measurements.start();
            let prepared = model::prepare_sources(&self.options, cache, &[], now)?;
            measurements.record("model_prepare_inclusive", started, 0);
            measurements.record("component_total_inclusive", total_started, 1);
            if timing {
                measurements.emit(
                    &crate::native_timing::id(&EventCache::path(&self.options.cache)),
                    "component_resolution",
                );
            }
            return Ok(Some(prepared));
        }
        let mut inputs: BTreeSet<_> = inputs.into_iter().collect();
        let options = Options {
            scan_window: false,
            ..self.options.clone()
        };
        // Expand through both persisted relationships and newly derived
        // endpoints. Reprepare only when another current source joins;
        // the event cache retains the already loaded component indexes.
        let mut keys = Vec::new();
        let mut native_parents = Vec::new();
        let prepared = loop {
            let selected: Vec<_> = inputs.iter().cloned().collect();
            let started = measurements.start();
            let component =
                cache.catalog_component_paths(&selected, &keys, &native_parents, &self.options)?;
            measurements.record("lookup_before", started, selected.len());
            let Some(component) = component else {
                break None;
            };
            inputs.extend(component);
            if inputs.len() > 64 {
                break None;
            }
            let selected: Vec<_> = inputs.iter().cloned().collect();
            let started = measurements.start();
            let prepared = model::prepare_sources(&options, cache, &selected, now)?;
            measurements.record("model_prepare_inclusive", started, selected.len());
            let dependencies: BTreeSet<_> = prepared
                .rows
                .iter()
                .flat_map(|row| {
                    std::iter::once(row.key.as_str())
                        .chain(row.parent.as_deref())
                        .chain(row.trace.iter().flat_map(|trace| trace.endpoints()))
                })
                .map(str::to_owned)
                .collect();
            if dependencies.len() > 64 {
                break None;
            }
            let Some(messages) = &prepared.message_dependencies else {
                break None;
            };
            let selectors = prepared
                .rows
                .iter()
                .flat_map(|row| {
                    row.native_ids
                        .iter()
                        .map(|id| (row.harness.clone(), id.clone()))
                })
                .chain(prepared.lineage_dependencies.iter().cloned())
                .chain(
                    messages
                        .iter()
                        .map(|id| ("message:claude".to_owned(), id.clone())),
                )
                .collect::<Vec<_>>();
            let started = measurements.start();
            let dependents =
                crate::events::store::catalog_dependents_bounded(&self.options, &selectors)?;
            measurements.record("dependents", started, selectors.len());
            let Some(dependents) = dependents else {
                break None;
            };
            inputs.extend(dependents);
            if inputs.len() > 64 {
                break None;
            }
            keys = dependencies.into_iter().collect();
            // Backfill must resolve parents that were published by an earlier
            // source batch too. The lookup uses indexed current owners only;
            // an undiscovered parent remains provisional until its own batch.
            // Omitting this context can incorrectly qualify orphaned children.
            native_parents = prepared.native_parents();
            native_parents.extend(
                prepared
                    .lineage_dependencies
                    .iter()
                    .filter(|(harness, _)| !harness.starts_with("bridge:"))
                    .cloned(),
            );
            let started = measurements.start();
            let component =
                cache.catalog_component_paths(&selected, &keys, &native_parents, &self.options)?;
            measurements.record("lookup_after", started, selected.len());
            let Some(component) = component else {
                break None;
            };
            if inputs.len() == selected.len()
                && component.iter().all(|input| inputs.contains(input))
            {
                break Some(prepared);
            }
            inputs.extend(component);
            if inputs.len() > 64 {
                break None;
            }
        };
        measurements.record("component_total_inclusive", total_started, 1);
        if timing {
            measurements.emit(
                &crate::native_timing::id(&EventCache::path(&self.options.cache)),
                "component_resolution",
            );
        }
        Ok(prepared)
    }

    /// Refresh an existing current owner through the shared Catalog producer.
    /// Cold discovery and unsupported role/identity changes still reconcile.
    pub(crate) fn local_query_target(&self, key: &str, now: i64) -> io::Result<Option<String>> {
        if let Some(target) = crate::overview_repair::local_query_target(&self.options, key, now)? {
            return Ok(Some(target));
        }
        let Some(inputs) = crate::overview_repair::local_query_inputs(&self.options, key)? else {
            return Ok(None);
        };
        let mut work = lock(&self.work);
        if self.closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "native query source is closed",
            ));
        }
        self.prepare_events(&mut work, now)?;
        let cache = work.events.as_mut().expect("event cache initialized");
        let Some(prepared) = self.prepare_catalog_component(cache, inputs, now)? else {
            return Ok(None);
        };
        match cache.publish_closed_catalog(&prepared)? {
            Some(crate::events::Outcome::Written) => {
                cache.publish_session_descriptions();
                cache.end_scan();
                *write_lock(&self.query) = None;
            }
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "native Catalog component generation changed",
                ));
            }
            None => return Ok(None),
        }
        crate::overview_repair::local_query_target(&self.options, key, now)?
            .map(Some)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Selected native source changed during refresh",
                )
            })
    }

    /// Prepare/publish Catalog evidence directly, independent of Viewer state.
    /// The caller holds native work; false yields a bounded discovery slice.
    pub(crate) fn refresh_catalog(&self, work: &mut Work, now: i64) -> io::Result<bool> {
        self.refresh_catalog_with(work, now, || Ok(()))
    }

    pub(crate) fn refresh_catalog_with(
        &self,
        work: &mut Work,
        now: i64,
        mut before_build: impl FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        // Keep the existing receiver facts fence through derivation and the
        // Catalog commit. A newly indexed source has no published owner yet;
        // otherwise a facts writer could miss that dependency in the gap after
        // preparation releases its own read fence and before publication.
        let _facts_publication = self
            .options
            .facts
            .as_ref()
            .map(|_| crate::source_change::SourceLock::acquire(&self.options, "facts", false))
            .transpose()?;
        // Hosted Raw writers journal exact intents durably. The persisted
        // qualification is independent of this process's in-memory snapshot.
        self.prepare_events(work, now)?;
        if work.events.as_ref().is_some_and(EventCache::is_scoped)
            && crate::events::store::recover_unsupported_catalog(&self.options)?
        {
            work.cold_catalog = None;
            work.catalog_snapshot = None;
            work.pending_sources = true;
        }
        if self.advance_semantic_stage(work, now, &mut before_build)? {
            let cache = work.events.as_ref().expect("event cache initialized");
            if cache.qualified_catalog()? && cache.pending_catalog_sources(1)?.is_empty() {
                if let Some(discovery) = work.cold_catalog.take() {
                    work.audit_revision = discovery.revision;
                    self.audit_completed
                        .store(discovery.revision, Ordering::Release);
                    let mut snapshot = discovery
                        .snapshot
                        .unwrap_or_else(|| Snapshot::sources(&self.options, &[]));
                    if self.options.facts.is_none() {
                        snapshot.pids.extend(
                            discovery
                                .pids
                                .into_iter()
                                .map(|pid| (pid, proc_start(&self.options.proc_root, pid))),
                        );
                    }
                    work.catalog_snapshot = Some(snapshot);
                }
                work.pending_sources = if let Some(audit) = &work.audit {
                    if audit.discovered && audit.checked {
                        self.finish_audit(work, now, &mut before_build)?
                    } else {
                        true
                    }
                } else {
                    self.audit_request.load(Ordering::Acquire) != work.audit_revision
                };
                return Ok(!work.pending_sources);
            }
            return Ok(false);
        }
        let requested = self.audit_request.load(Ordering::Acquire);
        if requested != work.audit_revision
            && work.audit.is_none()
            && work
                .events
                .as_ref()
                .expect("event cache initialized")
                .qualified_catalog()?
        {
            work.audit = Some(CatalogAudit {
                revision: requested,
                ..Default::default()
            });
            work.pending_sources = true;
        }
        let cache = work.events.as_mut().expect("event cache initialized");
        let controlled = work.audit.is_none()
            && self.options.facts.is_some()
            && cache.is_scoped()
            && crate::events::store::push_driven(&self.options)?;
        if cache.is_scoped()
            && matches!(cache.backfill_state()?.as_deref(), None | Some("complete"))
            && cache.qualified_catalog()?
            && (controlled
                || work.catalog_snapshot.is_none()
                || work.audit.is_some()
                || work.pending_sources)
        {
            let mut audit_pending = false;
            let mut notified = false;
            if !controlled && (work.audit.is_some() || !work.pending_sources) {
                let audit = work.audit.get_or_insert_with(CatalogAudit::default);
                let page = if audit.discovered {
                    None
                } else {
                    Some(audit.cursor.next(&self.options, 64, 512)?)
                };
                if let Some(page) = &page {
                    audit.discovered = page.complete;
                    audit
                        .snapshot
                        .get_or_insert_with(|| Snapshot::sources(&self.options, &[]))
                        .extend_sources(&self.options, &page.inputs);
                }
                let last = crate::events::store::audit_catalog_sources(
                    &self.options,
                    page.as_ref().map_or(&[], |page| page.inputs.as_slice()),
                    (!audit.checked).then_some(audit.after.as_str()),
                    &query_runtime_generation(&self.options)?,
                )?;
                if let Some(last) = last {
                    audit.after = last;
                } else {
                    audit.checked = true;
                }
                audit_pending = !audit.discovered || !audit.checked;
                if !audit_pending
                    && !audit.started
                    && let (Some(previous), Some(observed)) =
                        (&work.catalog_snapshot, &audit.snapshot)
                    && previous.runtime_inputs_changed(observed, &self.options)
                {
                    before_build()?;
                    work.built_at = Some(Instant::now());
                    audit.started = true;
                    notified = true;
                    if cache.pending_catalog_sources(1)?.is_empty() {
                        // Observe changed runtime inputs coherently even when
                        // no session depends on them. This native preparation
                        // has no sources, history loads or Catalog row writes.
                        model::prepare_sources(&self.options, cache, &[], now)?;
                        cache.end_scan();
                    }
                }
            }
            let pending = cache.pending_catalog_sources(1)?;
            if pending.is_empty() {
                if !audit_pending {
                    audit_pending = self.finish_audit(work, now, &mut before_build)?;
                }
                work.pending_sources = audit_pending;
                return Ok(!audit_pending);
            }
            if pending[0].ends_with(".json") {
                cache.resolve_catalog_input(&self.options, &pending[0])?;
                work.pending_sources =
                    audit_pending || !cache.pending_catalog_sources(1)?.is_empty();
                return Ok(!work.pending_sources);
            }
            let inputs = pending
                .into_iter()
                .map(|path| {
                    catalog_input(Path::new(&path), &self.options)
                        .ok_or_else(|| io::Error::other("journal source outside native allowlist"))
                })
                .collect::<io::Result<Vec<_>>>()?;
            let snapshot = Snapshot::sources(&self.options, &inputs);
            if !notified && !work.audit.as_ref().is_some_and(|audit| audit.started) {
                before_build()?;
                work.built_at = Some(Instant::now());
                if let Some(audit) = &mut work.audit {
                    audit.started = true;
                }
            }
            let staged_paths = inputs
                .iter()
                .map(|input| {
                    input
                        .full_path(&self.options)
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>();
            if let Some(prepared) = self.prepare_catalog_component(cache, inputs, now)? {
                match cache.publish_closed_catalog_fenced(&prepared)? {
                    Some((crate::events::Outcome::Written, generation)) => {
                        crate::events::store::requeue_concurrent_sources(&self.options, &prepared)?;
                        cache.publish_session_descriptions();
                        cache.end_scan();
                        if controlled {
                            work.catalog_snapshot = Some(snapshot);
                        }
                        work.observed_catalog_generation = generation;
                        cache.release_published_indexes();
                        let sources_pending = !cache.pending_catalog_sources(1)?.is_empty();
                        if controlled && !sources_pending {
                            let generation = cache.session_catalog_generation();
                            if !crate::events::store::note_catalog_audit(
                                &self.options,
                                generation.as_deref(),
                                now,
                            )? {
                                audit_pending = true;
                            }
                        }
                        work.texts.release_published_derivations();
                        if !controlled && !audit_pending && !sources_pending {
                            audit_pending = self.finish_audit(work, now, &mut before_build)?;
                        }
                        work.pending_sources = audit_pending || sources_pending;
                        return Ok(!work.pending_sources);
                    }
                    Some((crate::events::Outcome::Deferred, _)) => {
                        cache.end_scan();
                        cache.release_published_indexes();
                        work.texts.release_published_derivations();
                        work.pending_sources = true;
                        return Ok(false);
                    }
                    Some(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "Catalog source publication changed",
                        ));
                    }
                    None => {}
                }
            }
            if cache
                .begin_semantic_stage(&staged_paths, &query_runtime_generation(&self.options)?)?
                != crate::events::Outcome::Written
            {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "semantic stage changed",
                ));
            }
            cache.end_scan();
            cache.release_published_indexes();
            work.texts.release_published_derivations();
            work.catalog_snapshot = None;
            work.pending_sources = true;
            return Ok(false);
        }
        if work.cold_catalog.is_none()
            && let Some(snapshot) = &work.catalog_snapshot
            && !snapshot.changed(&self.options)
        {
            return Ok(true);
        }
        work.built_at = Some(Instant::now());
        let started = false;
        if work.cold_catalog.is_none() && work.catalog_snapshot.is_some() {
            self.prepare_events(work, now)?;
            let cache = work.events.as_mut().expect("event cache initialized");
            // The old snapshot proves discovery/runtime inputs did not change;
            // stamp before reads so concurrent writes remain retryable.
            let snapshot = Snapshot::capture_pids(
                &self.options,
                work.catalog_snapshot
                    .as_ref()
                    .expect("snapshot exists")
                    .pids
                    .keys()
                    .copied()
                    .collect(),
                cache.paths(),
            );
            if let Some(inputs) = work
                .catalog_snapshot
                .as_ref()
                .expect("snapshot exists")
                .changed_catalog_inputs(&snapshot, &self.options)
            {
                // Incremental semantic work shares the same lifecycle boundary
                // as reconciliation, including worker failure instrumentation.
                before_build()?;
                let staged_paths = inputs
                    .iter()
                    .map(|input| {
                        input
                            .full_path(&self.options)
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect::<Vec<_>>();
                let prepared = self.prepare_catalog_component(cache, inputs, now)?;
                if let Some(prepared) = prepared {
                    match cache.publish_closed_catalog_fenced(&prepared)? {
                        Some((crate::events::Outcome::Written, generation)) => {
                            crate::events::store::requeue_concurrent_sources(
                                &self.options,
                                &prepared,
                            )?;
                            cache.publish_session_descriptions();
                            cache.end_scan();
                            work.catalog_snapshot = Some(snapshot);
                            work.observed_catalog_generation = generation;
                            cache.release_published_indexes();
                            work.texts.release_published_derivations();
                            work.pending_sources = !cache.pending_catalog_sources(1)?.is_empty();
                            return Ok(!work.pending_sources);
                        }
                        Some((crate::events::Outcome::Deferred, _)) => {
                            cache.end_scan();
                            cache.release_published_indexes();
                            work.texts.release_published_derivations();
                            work.pending_sources = true;
                            return Ok(false);
                        }
                        Some(_) => {
                            return Err(io::Error::new(
                                io::ErrorKind::WouldBlock,
                                "native Catalog component generation changed",
                            ));
                        }
                        None => {}
                    }
                }
                if cache.begin_semantic_stage(
                    &staged_paths,
                    &query_runtime_generation(&self.options)?,
                )? != crate::events::Outcome::Written
                {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "semantic stage changed",
                    ));
                }
                cache.end_scan();
                cache.release_published_indexes();
                work.texts.release_published_derivations();
                work.pending_sources = true;
                return Ok(false);
            }
        }
        // Untrusted runtime or inventory changes use the bounded metadata audit;
        // their persisted dirty sources still take the same publication path.
        if work.cold_catalog.is_none()
            && work.catalog_snapshot.is_some()
            && work.events.as_ref().is_some_and(|cache| cache.is_scoped())
        {
            work.audit = Some(CatalogAudit {
                started,
                revision: requested,
                ..Default::default()
            });
            work.pending_sources = true;
            return Ok(false);
        }
        match self.prepare_catalog(work, now)? {
            CatalogPreparation::Pending => return Ok(false),
            CatalogPreparation::PublishedPending => {
                before_build()?;
                return Ok(false);
            }
            CatalogPreparation::Ready(generation) => {
                if !work
                    .cold_catalog
                    .as_ref()
                    .is_some_and(|discovery| discovery.started)
                {
                    before_build()?;
                }
                let cache = work.events.as_mut().expect("event cache initialized");
                if cache.finish_catalog_backfill(&generation)? != crate::events::Outcome::Written {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "backfill completion changed",
                    ));
                }
                let discovery = work.cold_catalog.take().expect("backfill state exists");
                work.audit_revision = discovery.revision;
                self.audit_completed
                    .store(discovery.revision, Ordering::Release);
                let mut snapshot = discovery
                    .snapshot
                    .unwrap_or_else(|| Snapshot::sources(&self.options, &[]));
                if self.options.facts.is_none() {
                    snapshot.pids.extend(
                        discovery
                            .pids
                            .into_iter()
                            .map(|pid| (pid, proc_start(&self.options.proc_root, pid))),
                    );
                }
                work.catalog_snapshot = Some(snapshot);
                work.observed_catalog_generation = Some(generation);
                return Ok(true);
            }
            CatalogPreparation::Reconcile => {}
        }
        let cache = work.events.as_mut().expect("event cache initialized");
        // Stamp before native reads so concurrent appends remain observable.
        let mut snapshot = Snapshot::capture_pids(&self.options, BTreeSet::new(), cache.paths());
        before_build()?;
        let catalog_options = Options {
            scan_window: false,
            ..self.options.clone()
        };
        let (pids, generation) = model::observe_catalog_fenced(
            &catalog_options,
            cache,
            &mut false,
            &mut work.texts,
            now,
        )?;
        if self.options.facts.is_none() {
            snapshot.pids = pids
                .iter()
                .map(|pid| (*pid, proc_start(&self.options.proc_root, *pid)))
                .collect();
        }
        work.catalog_snapshot = Some(snapshot);
        work.observed_catalog_generation = generation;
        cache.release_published_indexes();
        work.texts.release_published_derivations();
        Ok(true)
    }

    /// Native queries refresh this owner's source cache and coherent cohort.
    /// This owner has no compatibility transport or background refresh workers.
    pub(crate) fn catalog_continuation(&self, work: &Work) -> bool {
        work.cold_catalog.is_some()
            || work.audit.is_some()
            || (work.pending_sources
                && self.audit_request.load(Ordering::Acquire) == work.audit_revision)
    }

    pub(crate) fn catalog_pending(&self, work: &Work) -> bool {
        work.cold_catalog.is_some()
            || work.pending_sources
            || work.audit.is_some()
            || self.audit_request.load(Ordering::Acquire) != work.audit_revision
    }

    pub(crate) fn query_at(&self, now: i64) -> io::Result<Arc<model::QueryData>> {
        let retained = || {
            self.closed.load(Ordering::Acquire).then(|| {
                read_lock(&self.query)
                    .as_ref()
                    .map(|query| query.data.clone())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "native query source is closed")
                    })
            })
        };
        if let Some(data) = retained() {
            return data;
        }
        let unchanged = || {
            let query = read_lock(&self.query).clone();
            query.filter(|query| !query.snapshot.changed(&self.options))
        };
        if let Some(query) = unchanged() {
            return Ok(query.data.clone());
        }
        let mut work = lock(&self.work);
        if let Some(data) = retained() {
            return data;
        }
        if let Some(query) = unchanged() {
            return Ok(query.data.clone());
        }
        let work = &mut *work;
        self.prepare_events(work, now)?;
        let cache = work.events.as_mut().expect("event cache initialized");
        let mut snapshot = Snapshot::capture_pids(&self.options, BTreeSet::new(), cache.paths());
        let data = model::build_query(&self.options, cache, &mut false, &mut work.texts, now)?;
        if self.options.facts.is_none() {
            snapshot.pids = data
                .pids
                .iter()
                .map(|pid| (*pid, proc_start(&self.options.proc_root, *pid)))
                .collect();
        }
        let data = Arc::new(data);
        *write_lock(&self.query) = Some(Arc::new(QueryCache {
            data: data.clone(),
            snapshot,
        }));
        Ok(data)
    }
}

struct QueryCache {
    data: Arc<model::QueryData>,
    snapshot: Snapshot,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}
fn read_lock<T>(value: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    value.read().unwrap_or_else(PoisonError::into_inner)
}
fn write_lock<T>(value: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    value.write().unwrap_or_else(PoisonError::into_inner)
}

/// The single native source index and producer state shared by supported readers.
#[derive(Default)]
pub(crate) struct Work {
    pub(crate) events: Option<EventCache>,
    pub(crate) texts: Texts,
    pub(crate) catalog_snapshot: Option<Snapshot>,
    /// This owner's acknowledged shared publication, never a completeness proof.
    observed_catalog_generation: Option<String>,
    cold_catalog: Option<ColdCatalog>,
    pending_sources: bool,
    audit: Option<CatalogAudit>,
    audit_revision: u64,
    /// When the last native preparation started.
    pub(crate) built_at: Option<Instant>,
}

#[derive(Default)]
struct CatalogAudit {
    revision: u64,
    cursor: crate::inputs::Discovery,
    discovered: bool,
    after: String,
    checked: bool,
    started: bool,
    snapshot: Option<Snapshot>,
}

#[derive(Default)]
struct ColdCatalog {
    revision: u64,
    cursor: crate::inputs::Discovery,
    pids: BTreeSet<u32>,
    snapshot: Option<Snapshot>,
    started: bool,
    newest: BTreeSet<(
        std::cmp::Reverse<Option<std::time::SystemTime>>,
        crate::inputs::Input,
    )>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    kind: fs::FileType,
    generation: crate::events::Stat,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = fs::symlink_metadata(path).ok()?;
    Some(Stamp {
        kind: metadata.file_type(),
        generation: crate::events::Stat::from_metadata(&metadata).ok()?,
    })
}

/// Runtime observations fence published Query summaries independently of native
/// history. This visits process records and naming inputs, never transcript trees.
pub(crate) fn query_runtime_generation(options: &Options) -> io::Result<String> {
    if let Some(path) = &options.facts {
        let facts = crate::read_regular_at_most(path, 64 * 1024 * 1024)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<crate::Facts>(&bytes).ok())
            .unwrap_or_default();
        return recorded_runtime_fingerprint(options, &facts);
    }
    legacy_runtime_generation(options)
}

/// Semantic hosted runtime fence. Source selection has its independent authority
/// and revision fences; transfer sequence alone cannot stale runtime summaries.
pub(crate) fn recorded_runtime_fingerprint(
    options: &Options,
    facts: &crate::Facts,
) -> io::Result<String> {
    recorded_runtime_fingerprint_excluding(options, facts, None)
}

pub(crate) fn recorded_runtime_fingerprint_excluding(
    options: &Options,
    facts: &crate::Facts,
    excluded: Option<&Path>,
) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut facts = facts.clone();
    facts.mirror_observation_id = None;
    facts.mirror_sequence = None;
    facts.codex_rollouts = None;
    facts.codex_provisional_rollouts = None;
    for name in &mut facts.native_names {
        name.observed_at = 0;
    }
    let mut active = BTreeMap::new();
    watch_tree(
        &options.claude_home.join("sessions"),
        &[".json"],
        &mut active,
    );
    let active = active
        .into_iter()
        // Directory clocks describe enumeration, not PID runtime semantics.
        .filter(|(path, _)| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter(|(path, _)| excluded != Some(path.as_path()))
        .map(|(path, stamp)| (path, stamp.map(|stamp| stamp.generation)))
        .collect::<Vec<_>>();
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(facts, active)).map_err(io::Error::other)?)
    ))
}

/// Qualification of pre-scoped Catalog recipes during an additive upgrade.
pub(crate) fn legacy_runtime_generation(options: &Options) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut watched = BTreeMap::new();
    watch_tree(
        &options.claude_home.join("sessions"),
        &[".json"],
        &mut watched,
    );
    for path in [&options.claude_json]
        .into_iter()
        .chain(options.facts.iter())
    {
        watched.insert(path.clone(), stamp(path));
    }
    let mut processes = BTreeMap::new();
    let mut locks = None;
    if options.facts.is_none() {
        let path = options.proc_root.join("sys/kernel/hostname");
        watched.insert(path.clone(), stamp(&path));
        for name in [
            "session_index.jsonl",
            "state_5.sqlite",
            "state_5.sqlite-wal",
        ] {
            let path = options.codex_home.join(name);
            watched.insert(path.clone(), stamp(&path));
        }
        watch_tree(
            &options.codex_home.join("thread-writer-locks"),
            &[".lock"],
            &mut watched,
        );
        let ids = lock_ids(options);
        locks = writer_locks(options, &ids);
        let machine = crate::facts::MachineFacts::Local;
        let pids = model::pid_files(options, &machine)
            .into_iter()
            .map(|pid| pid.pid)
            .chain(
                machine
                    .codex_lock_pids(options)
                    .into_iter()
                    .flat_map(|pids| pids.into_values()),
            );
        for pid in pids {
            processes.insert(
                pid,
                (
                    proc_start(&options.proc_root, pid),
                    machine.run(options, pid, None),
                ),
            );
        }
    }
    let paths: Vec<_> = watched
        .into_iter()
        .map(|(path, observed)| {
            let path = path
                .to_str()
                .ok_or_else(|| io::Error::other("Nonportable runtime observation"))?;
            Ok((path.to_owned(), observed.map(|stamp| stamp.generation)))
        })
        .collect::<io::Result<_>>()?;
    let bytes = serde_json::to_vec(&(paths, locks, processes)).map_err(io::Error::other)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) struct Snapshot {
    pub(crate) watched: BTreeMap<PathBuf, Option<Stamp>>,
    /// The Codex writer-lock files' identities, and the `/proc/locks` lines
    /// that name them. Other locks on the machine (a browser's, SQLite's)
    /// come and go all the time and change nothing here.
    lock_ids: BTreeSet<(u64, u64, u64)>,
    locks: Option<String>,
    pub(crate) pids: BTreeMap<u32, Option<u64>>,
}

/// A `/proc/locks` line's `major:minor:inode`, as `lock_pid` reads it.
fn lock_key(line: &str) -> Option<(u64, u64, u64)> {
    let words: Vec<_> = line.split_whitespace().collect();
    let parts: Vec<_> = words.get(5)?.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        u64::from_str_radix(parts[0], 16).ok()?,
        u64::from_str_radix(parts[1], 16).ok()?,
        parts[2].parse().ok()?,
    ))
}

/// The lines of `/proc/locks` on the writer-lock files, and nothing else.
fn writer_locks(options: &Options, ids: &BTreeSet<(u64, u64, u64)>) -> Option<String> {
    let locks = fs::read_to_string(options.proc_root.join("locks")).ok()?;
    Some(
        locks
            .lines()
            .filter(|line| lock_key(line).is_some_and(|key| ids.contains(&key)))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn lock_ids(options: &Options) -> BTreeSet<(u64, u64, u64)> {
    let mut ids = BTreeSet::new();
    #[cfg(unix)]
    if let Ok(entries) = fs::read_dir(options.codex_home.join("thread-writer-locks")) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "lock")
                && let Ok(id) = crate::lock_identity(&entry.path())
            {
                ids.insert(id);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = options;
    ids
}

fn watch_tree(path: &Path, suffixes: &[&str], watched: &mut BTreeMap<PathBuf, Option<Stamp>>) {
    watched.insert(path.to_owned(), stamp(path));
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        if let Some(parent) = path.parent() {
            watched
                .entry(parent.to_owned())
                .or_insert_with(|| stamp(parent));
        }
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // A symbolic link is neither: it is never followed.
        if kind.is_dir() {
            watch_tree(&path, suffixes, watched);
        } else if kind.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| suffixes.iter().any(|suffix| name.ends_with(suffix)))
        {
            watched.insert(path.clone(), stamp(&path));
        }
    }
}

fn node_pids(nodes: &[Node], output: &mut BTreeSet<u32>) {
    for node in nodes {
        if let Some(pid) = node.pid {
            output.insert(pid);
        }
        node_pids(&node.children, output);
    }
}

pub(crate) fn source_input(path: &Path, options: &Options) -> Option<crate::Input> {
    [
        crate::InputRoot::Claude,
        crate::InputRoot::Codex,
        crate::InputRoot::Copilot,
    ]
    .into_iter()
    .find_map(|root| {
        let relative = path
            .strip_prefix(root.home(options))
            .ok()?
            .to_str()?
            .replace(std::path::MAIN_SEPARATOR, "/");
        crate::is_input_path(root.as_str(), &relative).then_some(crate::Input {
            root,
            path: relative,
        })
    })
}

pub(crate) fn catalog_input(path: &Path, options: &Options) -> Option<crate::inputs::Input> {
    if path
        .extension()
        .is_none_or(|extension| extension != "jsonl")
    {
        return None;
    }
    [
        crate::inputs::InputRoot::Claude,
        crate::inputs::InputRoot::Codex,
        crate::inputs::InputRoot::Copilot,
    ]
    .into_iter()
    .find_map(|root| {
        let relative = path
            .strip_prefix(root.home(options))
            .ok()?
            .to_str()?
            .replace(std::path::MAIN_SEPARATOR, "/");
        crate::inputs::is_input_path(root.as_str(), &relative).then_some(crate::inputs::Input {
            root,
            path: relative,
        })
    })
}

impl Snapshot {
    /// An audit may have sampled a transcript before its pending revision was
    /// published. Source stamps are checked against the durable ledger by the
    /// audit, so that older stamp must not also request an empty runtime build.
    /// Missing sources, type changes, directories and sidecars still differ;
    /// naming files outside the native log allowlist remain runtime inputs.
    fn runtime_inputs_changed(&self, observed: &Self, options: &Options) -> bool {
        self.locks != observed.locks
            || self.pids != observed.pids
            || self.watched.iter().any(|(path, original)| {
                match (original, observed.watched.get(path)) {
                    (Some(original), Some(Some(current)))
                        if original.kind.is_file()
                            && current.kind.is_file()
                            && catalog_input(path, options).is_some() =>
                    {
                        false
                    }
                    (_, Some(current)) => current != original,
                    (_, None) => true,
                }
            })
            || observed
                .watched
                .keys()
                .any(|path| !self.watched.contains_key(path))
    }

    /// Assemble observation stamps along bounded source batches. Never enumerate
    /// native transcript trees again after progressive discovery.
    fn sources(options: &Options, inputs: &[crate::Input]) -> Self {
        let mut watched = BTreeMap::new();
        watch_tree(
            &options.claude_home.join("sessions"),
            &[".json"],
            &mut watched,
        );
        for path in [&options.claude_json]
            .into_iter()
            .chain(options.facts.iter())
        {
            watched.insert(path.clone(), stamp(path));
        }
        let mut pids = BTreeMap::new();
        let mut ids = BTreeSet::new();
        let locks = if options.facts.is_none() {
            watch_tree(
                &options.codex_home.join("thread-writer-locks"),
                &[".lock"],
                &mut watched,
            );
            for name in [
                "session_index.jsonl",
                "state_5.sqlite",
                "state_5.sqlite-wal",
            ] {
                let path = options.codex_home.join(name);
                watched.insert(path.clone(), stamp(&path));
            }
            let path = options.proc_root.join("sys/kernel/hostname");
            watched.insert(path.clone(), stamp(&path));
            ids = lock_ids(options);
            for pid in model::pid_files(options, &crate::facts::MachineFacts::Local) {
                pids.insert(pid.pid, proc_start(&options.proc_root, pid.pid));
            }
            writer_locks(options, &ids)
        } else {
            None
        };
        let mut snapshot = Self {
            watched,
            pids,
            lock_ids: ids,
            locks,
        };
        for path in [
            options.claude_home.join("projects"),
            options.codex_home.join("sessions"),
            options.codex_home.join("archived_sessions"),
            options.copilot_home.join("session-state"),
        ] {
            snapshot.watched.insert(path.clone(), stamp(&path));
        }
        snapshot.extend_sources(options, inputs);
        snapshot
    }
    fn extend_sources(&mut self, options: &Options, inputs: &[crate::Input]) {
        for input in inputs {
            let path = input.full_path(options);
            self.watched.insert(path.clone(), stamp(&path));
            if input.path.ends_with(".jsonl")
                && input.root == crate::InputRoot::Claude
                && crate::native_sidecar::agent_path(&path)
            {
                let sidecar = path.with_extension("meta.json");
                self.watched.insert(sidecar.clone(), stamp(&sidecar));
            }
            let mut directory = path.parent();
            while let Some(path) =
                directory.filter(|path| path.starts_with(input.root.home(options)))
            {
                self.watched
                    .entry(path.to_owned())
                    .or_insert_with(|| stamp(path));
                directory = path.parent();
            }
        }
    }

    /// A bounded rewrite/append/addition set with unchanged process inputs.
    /// Directory changes qualify only when an admitted new source explains them;
    /// disappearance, sidecars and runtime changes still need reconciliation.
    pub(crate) fn changed_catalog_inputs(
        &self,
        observed: &Self,
        options: &Options,
    ) -> Option<Vec<crate::inputs::Input>> {
        if self
            .watched
            .keys()
            .any(|path| !observed.watched.contains_key(path))
            || self.lock_ids != observed.lock_ids
            || self.locks != observed.locks
            || self.pids != observed.pids
        {
            return None;
        }
        let mut inputs = Vec::new();
        let mut additions = Vec::new();
        let mut directories = Vec::new();
        for (path, current) in &observed.watched {
            let previous = self.watched.get(path).copied().flatten();
            if *current == previous {
                continue;
            }
            let current = current.as_ref()?;
            if current.kind.is_dir() {
                if previous.is_some_and(|previous| {
                    !previous.kind.is_dir()
                        || (previous.generation.dev, previous.generation.ino)
                            != (current.generation.dev, current.generation.ino)
                }) {
                    return None;
                }
                directories.push(path);
                continue;
            }
            if !current.kind.is_file() || previous.is_some_and(|previous| !previous.kind.is_file())
            {
                return None;
            }
            let input = catalog_input(path, options).or_else(|| {
                let stem = path.file_name()?.to_str()?.strip_suffix(".meta.json")?;
                let log = path.with_file_name(format!("{stem}.jsonl"));
                if !crate::native_sidecar::agent_path(&log)
                    || !fs::symlink_metadata(&log).is_ok_and(|metadata| metadata.is_file())
                {
                    return None;
                }
                let input = catalog_input(&log, options)?;
                (input.root == crate::inputs::InputRoot::Claude).then_some(input)
            })?;
            if previous.is_none() {
                additions.push(path);
            }
            inputs.push(input);
            if inputs.len() > 64 {
                return None;
            }
        }
        if directories
            .iter()
            .any(|directory| !additions.iter().any(|path| path.starts_with(directory)))
        {
            return None;
        }
        (!inputs.is_empty()).then_some(inputs)
    }

    pub(crate) fn capture(options: &Options, roots: &[Node], index: &EventCache) -> Self {
        let mut ids = BTreeSet::new();
        node_pids(roots, &mut ids);
        Self::capture_pids(options, ids, index.paths())
    }

    pub(crate) fn capture_pids<'a>(
        options: &Options,
        ids: BTreeSet<u32>,
        paths: impl Iterator<Item = &'a str>,
    ) -> Self {
        let mut watched = BTreeMap::new();
        watch_tree(
            &options.claude_home.join("projects"),
            &[".jsonl", ".meta.json"],
            &mut watched,
        );
        watch_tree(
            &options.claude_home.join("sessions"),
            &[".json"],
            &mut watched,
        );
        for root in crate::inputs::codex_rollout_dirs(options) {
            watch_tree(&root, &[".jsonl"], &mut watched);
        }
        watch_tree(
            &options.codex_home.join("thread-writer-locks"),
            &[".lock"],
            &mut watched,
        );
        let copilot_root = options.copilot_home.join("session-state");
        watched.insert(copilot_root.clone(), stamp(&copilot_root));
        if let Ok(entries) = fs::read_dir(&copilot_root) {
            for entry in entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            {
                let dir = entry.path();
                watched.insert(dir.clone(), stamp(&dir));
                let file = dir.join("events.jsonl");
                watched.insert(file.clone(), stamp(&file));
            }
        }
        watched.insert(options.claude_json.clone(), stamp(&options.claude_json));
        for path in paths {
            let path = PathBuf::from(path);
            watched.entry(path.clone()).or_insert_with(|| stamp(&path));
        }
        // With a facts file, it is the machine: nothing here reads `/proc`.
        if let Some(facts) = &options.facts {
            watched.insert(facts.clone(), stamp(facts));
            return Self {
                watched,
                lock_ids: BTreeSet::new(),
                locks: None,
                pids: BTreeMap::new(),
            };
        }
        let hostname = options.proc_root.join("sys/kernel/hostname");
        watched.insert(hostname.clone(), stamp(&hostname));
        for name in [
            "session_index.jsonl",
            "state_5.sqlite",
            "state_5.sqlite-wal",
        ] {
            let path = options.codex_home.join(name);
            watched.insert(path.clone(), stamp(&path));
        }
        let pids = ids
            .into_iter()
            .map(|pid| (pid, proc_start(&options.proc_root, pid)))
            .collect();
        let lock_ids = lock_ids(options);
        Self {
            watched,
            locks: writer_locks(options, &lock_ids),
            lock_ids,
            pids,
        }
    }

    pub(crate) fn changed(&self, options: &Options) -> bool {
        self.changed_with(options, true)
    }

    /// Cached tree metadata remains readable after permission-only changes.
    /// It cannot qualify current native source bytes or control authority.
    pub(crate) fn metadata_changed(&self, options: &Options) -> bool {
        self.changed_with(options, false)
    }

    fn changed_with(&self, options: &Options, native_generation: bool) -> bool {
        if self.watched.iter().any(|(path, original)| {
            let observed = stamp(path);
            match (observed, original) {
                (Some(observed), Some(original)) if !native_generation => {
                    observed.kind != original.kind
                        || (
                            observed.generation.dev,
                            observed.generation.ino,
                            observed.generation.size,
                            observed.generation.modified_ns,
                        ) != (
                            original.generation.dev,
                            original.generation.ino,
                            original.generation.size,
                            original.generation.modified_ns,
                        )
                }
                _ => observed != *original,
            }
        }) {
            return true;
        }
        if options.facts.is_some() {
            return false;
        }
        writer_locks(options, &self.lock_ids) != self.locks
            || self
                .pids
                .iter()
                .any(|(pid, original)| proc_start(&options.proc_root, *pid) != *original)
    }
}

#[cfg(test)]
mod catalog_owner_tests {
    use super::*;

    fn audit_fixture() -> (PathBuf, Options, crate::Input) {
        let root = std::env::temp_dir().join(format!(
            "semon-catalog-audit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = Options {
            claude_home: root.join("claude"),
            claude_json: root.join("claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            ..Options::default()
        };
        let input = crate::Input {
            root: crate::InputRoot::Codex,
            path: "sessions/2026/10/01/rollout-audit.jsonl".into(),
        };
        let log = input.full_path(&options);
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(log, "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"payload\":{\"id\":\"audit\"}}\n").unwrap();
        let hostname = options.proc_root.join("sys/kernel/hostname");
        fs::create_dir_all(hostname.parent().unwrap()).unwrap();
        fs::write(hostname, "initial-host\n").unwrap();
        (root, options, input)
    }

    fn drain_catalog(evidence: &NativeEvidence, builds: &std::cell::Cell<u64>) {
        let mut work = lock(&evidence.work);
        for _ in 0..16 {
            if evidence
                .refresh_catalog_with(&mut work, model::now_ms(), || {
                    builds.set(builds.get() + 1);
                    Ok(())
                })
                .unwrap()
            {
                return;
            }
        }
        panic!("bounded fixture observation did not finish");
    }

    fn append_audit_record(options: &Options, input: &crate::Input) {
        use std::io::Write;

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(input.full_path(options))
            .unwrap();
        writeln!(file, "{{\"type\":\"event_msg\",\"timestamp\":\"2026-10-01T00:01:00Z\",\"payload\":{{\"type\":\"user_message\",\"message\":\"shared publication\"}}}}").unwrap();
    }

    #[test]
    fn old_publication_with_active_subset_rebuilds_unrelated_metadata_before_qualification() {
        let (root, options, input) = audit_fixture();
        let unrelated = crate::Input {
            root: crate::InputRoot::Codex,
            path: "sessions/2026/10/01/rollout-unrelated.jsonl".into(),
        };
        fs::write(unrelated.full_path(&options), "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"payload\":{\"id\":\"unrelated\"}}\n").unwrap();
        let evidence = NativeEvidence::new(options.clone());
        drain_catalog(&evidence, &std::cell::Cell::new(0));
        {
            let mut work = lock(&evidence.work);
            assert_eq!(
                work.events
                    .as_mut()
                    .unwrap()
                    .begin_semantic_stage(
                        &[input.full_path(&options).to_string_lossy().into_owned()],
                        &query_runtime_generation(&options).unwrap(),
                    )
                    .unwrap(),
                crate::events::Outcome::Written
            );
        }
        let db = rusqlite::Connection::open(EventCache::path(&options.cache)).unwrap();
        let unrelated_key: String = db.query_row("SELECT session_key FROM session_catalog_sources WHERE native_id='unrelated' LIMIT 1", [], |row| row.get(0)).unwrap();
        db.execute(
            "UPDATE meta SET value=?1 WHERE key='catalog_version'",
            [(model::summary::CATALOG_VERSION - 1).to_string()],
        )
        .unwrap();
        assert_eq!(db.execute("UPDATE session_catalog SET metadata=json_set(metadata,'$.name','old derived metadata') WHERE session_key=?1", [&unrelated_key]).unwrap(), 1);
        db.execute("DELETE FROM meta WHERE key='catalog_semantic_recipe'", [])
            .unwrap();
        let published: i64 = db
            .query_row("SELECT COUNT(*) FROM session_catalog", [], |row| row.get(0))
            .unwrap();
        {
            let mut work = lock(&evidence.work);
            assert!(
                !evidence
                    .refresh_catalog(&mut work, model::now_ms())
                    .unwrap()
            );
            assert!(!work.events.as_ref().unwrap().qualified_catalog().unwrap());
        }
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM session_catalog", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            published,
            "upgrade must retain published history"
        );
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='catalog_backfill_state'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "discovering"
        );
        let mut complete = false;
        for _ in 0..128 {
            let mut work = lock(&evidence.work);
            if evidence
                .refresh_catalog(&mut work, model::now_ms())
                .unwrap()
            {
                assert!(work.events.as_ref().unwrap().qualified_catalog().unwrap());
                complete = true;
                break;
            }
        }
        assert!(complete, "bounded upgrade did not finish");
        let name: String = db
            .query_row(
                "SELECT json_extract(metadata,'$.name') FROM session_catalog WHERE session_key=?1",
                [&unrelated_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(
            name, "old derived metadata",
            "subset publication qualified untouched old metadata"
        );
        drop(db);
        drop(evidence);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_shared_publication_notifies_a_peer_without_rebuilding_history() {
        let (root, options, input) = audit_fixture();
        let first = NativeEvidence::new(options.clone());
        let second = NativeEvidence::new(options.clone());
        let builds = std::cell::Cell::new(0);
        drain_catalog(&first, &builds);
        drain_catalog(&second, &builds);
        let initial = lock(&second.work).observed_catalog_generation.clone();

        append_audit_record(&options, &input);
        first.request_metadata_audit();
        second.request_metadata_audit();
        drain_catalog(&first, &builds);
        let published = lock(&first.work).observed_catalog_generation.clone();
        assert!(initial != published, "append did not advance publication");
        builds.set(0);
        let counts = || {
            (
                crate::events::FILE_READS.with(std::cell::Cell::get),
                crate::events::PARSED.with(std::cell::Cell::get),
                crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get),
                crate::events::CATALOG_PUBLISHED_ROWS.with(std::cell::Cell::get),
            )
        };
        let before = counts();
        {
            let mut work = lock(&second.work);
            let mut failed = false;
            for _ in 0..16 {
                match second.refresh_catalog_with(&mut work, model::now_ms(), || {
                    Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "notification retry",
                    ))
                }) {
                    Err(error) => {
                        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
                        failed = true;
                        break;
                    }
                    Ok(complete) => assert!(!complete, "peer notification was skipped"),
                }
            }
            assert!(failed, "peer notification did not reach its callback");
            assert!(work.audit.is_some(), "failed callback consumed the audit");
            assert_eq!(work.observed_catalog_generation, initial);
        }
        drain_catalog(&second, &builds);
        assert_eq!(builds.get(), 1, "peer publication lost its observation");
        assert_eq!(counts(), before, "peer observation rebuilt shared history");
        let log = input.full_path(&options);
        {
            let work = lock(&second.work);
            assert_eq!(work.observed_catalog_generation, published);
            assert!(!second.catalog_pending(&work));
            assert!(
                work.catalog_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.watched.get(&log) == Some(&stamp(&log)))
            );
        }
        second.request_metadata_audit();
        drain_catalog(&second, &builds);
        assert_eq!(builds.get(), 1, "idle audit repeated peer notification");
        assert_eq!(counts(), before, "idle peer observation rebuilt history");
        drop(first);
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn peer_advance_after_own_publication_is_observed_in_the_same_audit() {
        let (root, options, input) = audit_fixture();
        let first = NativeEvidence::new(options.clone());
        let second = NativeEvidence::new(options.clone());
        let builds = std::cell::Cell::new(0);
        drain_catalog(&first, &builds);
        drain_catalog(&second, &builds);
        builds.set(0);
        second.request_metadata_audit();
        {
            let mut work = lock(&second.work);
            assert!(
                !second
                    .refresh_catalog_with(&mut work, model::now_ms(), || {
                        builds.set(builds.get() + 1);
                        Ok(())
                    })
                    .unwrap()
            );
            work.audit.as_mut().unwrap().after.clear();
        }
        append_audit_record(&options, &input);
        {
            let mut work = lock(&second.work);
            assert!(
                !second
                    .refresh_catalog_with(&mut work, model::now_ms(), || {
                        builds.set(builds.get() + 1);
                        Ok(())
                    })
                    .unwrap()
            );
            assert!(work.audit.as_ref().is_some_and(|audit| audit.started));
        }
        assert_eq!(builds.get(), 1);
        append_audit_record(&options, &input);
        first.request_metadata_audit();
        drain_catalog(&first, &std::cell::Cell::new(0));
        let parsed = crate::events::PARSED.with(std::cell::Cell::get);
        drain_catalog(&second, &builds);
        assert_eq!(
            builds.get(),
            2,
            "own work suppressed a later peer generation"
        );
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
        second.request_metadata_audit();
        drain_catalog(&second, &builds);
        assert_eq!(builds.get(), 2, "peer generation notified twice");
        drop(first);
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn peer_commit_before_own_acknowledgement_does_not_get_acknowledged_silently() {
        let (root, options, input) = audit_fixture();
        let peer_input = crate::Input {
            root: crate::InputRoot::Codex,
            path: "sessions/2026/10/01/rollout-peer.jsonl".into(),
        };
        fs::write(peer_input.full_path(&options), "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"payload\":{\"id\":\"peer\"}}\n").unwrap();
        let first = NativeEvidence::new(options.clone());
        let second = NativeEvidence::new(options.clone());
        let builds = std::cell::Cell::new(0);
        drain_catalog(&first, &builds);
        drain_catalog(&second, &builds);
        builds.set(0);
        second.request_metadata_audit();
        {
            let mut work = lock(&second.work);
            assert!(
                !second
                    .refresh_catalog_with(&mut work, model::now_ms(), || {
                        builds.set(builds.get() + 1);
                        Ok(())
                    })
                    .unwrap()
            );
            work.audit.as_mut().unwrap().after.clear();
        }
        append_audit_record(&options, &input);
        let peer_generation = std::rc::Rc::new(std::cell::RefCell::new(None));
        let published = std::rc::Rc::clone(&peer_generation);
        let peer = Arc::clone(&first);
        let peer_options = options.clone();
        crate::events::AFTER_CATALOG_PUBLICATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                // The first transaction has committed, but its caller has not
                // yet acknowledged that generation. Publish through a peer in
                // exactly the gap where a free-standing getter loses notice.
                append_audit_record(&peer_options, &peer_input);
                peer.request_metadata_audit();
                drain_catalog(&peer, &std::cell::Cell::new(0));
                *published.borrow_mut() = lock(&peer.work).observed_catalog_generation.clone();
            }));
        });
        {
            let mut work = lock(&second.work);
            assert!(
                !second
                    .refresh_catalog_with(&mut work, model::now_ms(), || {
                        builds.set(builds.get() + 1);
                        Ok(())
                    })
                    .unwrap()
            );
            let foreign = peer_generation.borrow();
            assert!(foreign.is_some(), "commit-gap hook did not publish");
            assert_ne!(
                &work.observed_catalog_generation, &*foreign,
                "own transaction acknowledged the intervening peer generation"
            );
            assert_eq!(
                work.events.as_ref().unwrap().session_catalog_generation(),
                *foreign,
                "fixture did not advance the shared Catalog inside the gap"
            );
        }
        assert_eq!(builds.get(), 1);
        let parsed = crate::events::PARSED.with(std::cell::Cell::get);
        drain_catalog(&second, &builds);
        assert_eq!(builds.get(), 2, "intervening peer commit lost its notice");
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
        assert_eq!(
            lock(&second.work).observed_catalog_generation,
            *peer_generation.borrow()
        );
        second.request_metadata_audit();
        drain_catalog(&second, &builds);
        assert_eq!(builds.get(), 2, "intervening peer commit notified twice");
        drop(first);
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staged_commit_acknowledges_own_generation_before_intervening_peer_metadata() {
        let (root, options, input) = audit_fixture();
        let first = NativeEvidence::new(options.clone());
        let peer = NativeEvidence::new(options.clone());
        let builds = std::cell::Cell::new(0);
        drain_catalog(&first, &builds);
        drain_catalog(&peer, &std::cell::Cell::new(0));
        append_audit_record(&options, &input);
        {
            let mut work = lock(&first.work);
            assert_eq!(
                work.events
                    .as_mut()
                    .unwrap()
                    .begin_semantic_stage(
                        &[input.full_path(&options).to_string_lossy().into_owned()],
                        &query_runtime_generation(&options).unwrap()
                    )
                    .unwrap(),
                crate::events::Outcome::Written
            );
        }
        let committed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let foreign = std::rc::Rc::new(std::cell::RefCell::new(None));
        let committed_in_hook = std::rc::Rc::clone(&committed);
        let foreign_in_hook = std::rc::Rc::clone(&foreign);
        let peer_in_hook = Arc::clone(&peer);
        let peer_options = options.clone();
        crate::events::AFTER_CATALOG_PUBLICATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let db = rusqlite::Connection::open(EventCache::path(&peer_options.cache)).unwrap();
                *committed_in_hook.borrow_mut() = Some(
                    db.query_row(
                        "SELECT value FROM meta WHERE key='catalog_generation'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                );
                drop(db);
                // An actual peer observes a legitimate runtime change. Its
                // stage replay can commit before the first caller acknowledges
                // the generation already returned by its own transaction.
                fs::write(
                    peer_options.proc_root.join("sys/kernel/hostname"),
                    "peer-host\n",
                )
                .unwrap();
                peer_in_hook.request_metadata_audit();
                let mut complete = false;
                for _ in 0..128 {
                    if peer_in_hook
                        .refresh_catalog(&mut lock(&peer_in_hook.work), model::now_ms())
                        .unwrap()
                    {
                        complete = true;
                        break;
                    }
                }
                assert!(complete, "peer metadata replay did not complete");
                *foreign_in_hook.borrow_mut() =
                    lock(&peer_in_hook.work).observed_catalog_generation.clone();
                assert_ne!(
                    *committed_in_hook.borrow(),
                    *foreign_in_hook.borrow(),
                    "peer metadata did not advance the shared generation"
                );
            }));
        });
        for _ in 0..128 {
            first
                .refresh_catalog_with(&mut lock(&first.work), model::now_ms(), || {
                    builds.set(builds.get() + 1);
                    Ok(())
                })
                .unwrap();
            if foreign.borrow().is_some() {
                break;
            }
        }
        assert!(
            foreign.borrow().is_some(),
            "staged publication did not invoke commit-gap hook"
        );
        {
            let work = lock(&first.work);
            assert_eq!(
                &work.observed_catalog_generation,
                &*committed.borrow(),
                "staged own acknowledgement did not preserve its transaction result"
            );
            assert_ne!(
                &work.observed_catalog_generation,
                &*foreign.borrow(),
                "staged own acknowledgement sampled the intervening peer generation"
            );
            assert_eq!(
                work.events.as_ref().unwrap().session_catalog_generation(),
                *foreign.borrow()
            );
        }
        let parsed = crate::events::PARSED.with(std::cell::Cell::get);
        let published = crate::events::CATALOG_PUBLISHED_ROWS.with(std::cell::Cell::get);
        let notified = builds.get();
        first.request_metadata_audit();
        drain_catalog(&first, &builds);
        assert!(
            builds.get() > notified,
            "intervening staged peer generation lost its notification"
        );
        assert_eq!(
            crate::events::PARSED.with(std::cell::Cell::get),
            parsed,
            "peer notice reparsed source history"
        );
        assert_eq!(
            crate::events::CATALOG_PUBLISHED_ROWS.with(std::cell::Cell::get),
            published,
            "peer notice republished source history"
        );
        assert_eq!(
            lock(&first.work).observed_catalog_generation,
            *foreign.borrow()
        );
        let notified = builds.get();
        first.request_metadata_audit();
        drain_catalog(&first, &builds);
        assert_eq!(
            builds.get(),
            notified,
            "staged peer generation notified twice"
        );
        drop(first);
        drop(peer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn append_after_audit_snapshot_does_not_request_an_empty_runtime_rebuild() {
        use std::io::Write;

        let (root, options, input) = audit_fixture();
        let evidence = NativeEvidence::new(options.clone());
        let mut work = lock(&evidence.work);
        let mut complete = false;
        for _ in 0..16 {
            if evidence
                .refresh_catalog(&mut work, model::now_ms())
                .unwrap()
            {
                complete = true;
                break;
            }
        }
        assert!(complete, "cold fixture did not finish");
        let builds = std::cell::Cell::new(0);
        let before_build = || {
            builds.set(builds.get() + 1);
            Ok(())
        };
        evidence.request_metadata_audit();
        assert!(
            !evidence
                .refresh_catalog_with(&mut work, model::now_ms(), before_build)
                .unwrap(),
            "the keyset audit must finish after its discovery snapshot"
        );
        assert_eq!(builds.get(), 0);
        let log = input.full_path(&options);
        assert!(
            work.audit
                .as_ref()
                .and_then(|audit| audit.snapshot.as_ref())
                .is_some_and(|snapshot| snapshot.watched.get(&log) == Some(&stamp(&log)))
        );
        // Replay the selected keyset row after discovery, placing the append
        // between snapshot sampling and its ledger audit without timing sleeps.
        work.audit.as_mut().unwrap().after.clear();
        let parsed = crate::events::PARSED.with(std::cell::Cell::get);
        let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(file, "{{\"type\":\"event_msg\",\"timestamp\":\"2026-10-01T00:01:00Z\",\"payload\":{{\"type\":\"user_message\",\"message\":\"published during audit\"}}}}").unwrap();
        drop(file);
        evidence.request_metadata_audit();
        for _ in 0..16 {
            complete = evidence
                .refresh_catalog_with(&mut work, model::now_ms(), before_build)
                .unwrap();
            if complete {
                break;
            }
        }
        assert!(complete, "append audits did not finish");
        assert_eq!(
            builds.get(),
            1,
            "append gained a second runtime notification"
        );
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get) - parsed, 1);
        assert!(!evidence.catalog_pending(&work));
        assert!(
            work.catalog_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.watched.get(&log) == Some(&stamp(&log)))
        );

        fs::write(
            options.proc_root.join("sys/kernel/hostname"),
            "changed-hostname\n",
        )
        .unwrap();
        evidence.request_metadata_audit();
        for _ in 0..16 {
            complete = evidence
                .refresh_catalog_with(&mut work, model::now_ms(), before_build)
                .unwrap();
            if complete {
                break;
            }
        }
        assert!(complete, "runtime audit did not finish");
        assert_eq!(builds.get(), 2, "hostname change lost its observation");
        drop(work);
        drop(evidence);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_comparison_preserves_missing_sources_and_type_changes() {
        let (root, options, input) = audit_fixture();
        let before = Snapshot::sources(&options, std::slice::from_ref(&input));
        let log = input.full_path(&options);
        fs::remove_file(&log).unwrap();
        let mut missing = Snapshot::sources(&options, std::slice::from_ref(&input));
        missing.watched = before.watched.clone();
        missing.watched.insert(log.clone(), None);
        assert!(before.runtime_inputs_changed(&missing, &options));
        fs::create_dir(&log).unwrap();
        let mut directory = Snapshot::sources(&options, std::slice::from_ref(&input));
        directory.watched = before.watched.clone();
        directory.watched.insert(log.clone(), stamp(&log));
        assert!(before.runtime_inputs_changed(&directory, &options));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_catalog_production_has_no_viewer_or_query_cohort_owner() {
        let root = std::env::temp_dir().join(format!(
            "semon-native-catalog-owner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = Options {
            claude_home: root.join("claude"),
            claude_json: root.join("claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("index.json"),
            scan_window: true,
            ..Options::default()
        };
        let log = options
            .codex_home
            .join("sessions/2026/10/01/rollout-owner.jsonl");
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(log, "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"payload\":{\"id\":\"owner\"}}\n").unwrap();
        let sources = SessionSources::with_machines(vec![(String::new(), options.clone())]);
        let evidence = &sources.machines[0].1;
        let before = crate::model::COMPAT_BUILDS.with(std::cell::Cell::get);
        let mut work = lock(&evidence.work);
        let mut complete = false;
        for _ in 0..16 {
            if evidence
                .refresh_catalog(&mut work, crate::model::now_ms())
                .unwrap()
            {
                complete = true;
                break;
            }
        }
        assert!(complete, "single-source discovery did not complete");
        let reply = crate::session_catalog_page(&options, "local", "limit=60");
        assert_eq!(reply.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["completeness"]["state"], "complete");
        assert_eq!(body["items"][0]["key"], "owner");
        assert!(sources.views.iter().all(|view| view.get().is_none()));
        assert!(read_lock(&evidence.query).is_none());
        assert_eq!(
            crate::model::COMPAT_BUILDS.with(std::cell::Cell::get),
            before
        );
        drop(work);
        drop(sources);
        fs::remove_dir_all(root).unwrap();
    }
}

//! Native evidence ownership, independent of Viewer transport and refresh workers.
//! Viewer, Catalog and Query share one complete source index per machine.
use crate::{
    Node, Options,
    events::EventCache,
    model::{self, Texts},
    proc_start,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

pub(crate) mod runtime;
pub(crate) use runtime::RuntimeObservations;

pub(crate) struct NativeEvidence {
    pub(crate) options: Options,
    pub(crate) store_config: Option<crate::storage::StoreConfig>,
    pub(crate) work: Mutex<Work>,
    pub(crate) catalog_demand: AtomicBool,
    catalog_search: Mutex<CatalogSearchQueue>,
    query: RwLock<Option<Arc<QueryCache>>>,
    closed: AtomicBool,
    audit_request: AtomicU64,
    audit_completed: AtomicU64,
}

// Reuse the existing bounded cohort size as scheduling capacity, not a
// performance budget. Only qualified pending paths and failed probes are
// admitted; ordinary metadata queries never occupy a scheduling ticket.
const CATALOG_SEARCH_CAPACITY: usize = crate::events::CATALOG_SEARCH_LIMIT;

#[derive(Clone)]
struct CatalogSearchTicket {
    id: u64,
    epoch: u64,
    query: String,
    harness: Option<String>,
    waiting: BTreeSet<String>,
}

struct CatalogSearchQueue {
    active: bool,
    epoch: u64,
    next_id: u64,
    tickets: VecDeque<CatalogSearchTicket>,
    ordinary_due: bool,
}

impl Default for CatalogSearchQueue {
    fn default() -> Self {
        Self {
            active: true,
            epoch: 0,
            next_id: 0,
            tickets: VecDeque::new(),
            ordinary_due: false,
        }
    }
}

impl CatalogSearchQueue {
    fn note(
        &mut self,
        interest: Option<(&str, Option<&str>)>,
        paths: Result<Vec<String>, ()>,
    ) -> bool {
        let Some((query, harness)) = interest else {
            return true;
        };
        if !self.active {
            return true;
        }
        if paths.as_ref().is_ok_and(Vec::is_empty) {
            return true;
        }
        let renewed = paths.is_ok();
        let waiting: BTreeSet<_> = paths.unwrap_or_default().into_iter().collect();
        if let Some(ticket) = self
            .tickets
            .iter_mut()
            .find(|ticket| ticket.query == query && ticket.harness.as_deref() == harness)
        {
            if renewed {
                self.next_id += 1;
                ticket.id = self.next_id;
            }
            for path in waiting {
                if ticket.waiting.len() == CATALOG_SEARCH_CAPACITY {
                    break;
                }
                ticket.waiting.insert(path);
            }
            return true;
        }
        if self.tickets.len() == CATALOG_SEARCH_CAPACITY {
            return false;
        }
        self.next_id += 1;
        self.tickets.push_back(CatalogSearchTicket {
            id: self.next_id,
            epoch: self.epoch,
            query: query.to_owned(),
            harness: harness.map(str::to_owned),
            waiting,
        });
        true
    }

    fn next(&mut self) -> Option<CatalogSearchTicket> {
        let ticket = self.tickets.pop_front()?;
        self.tickets.push_back(ticket.clone());
        Some(ticket)
    }

    fn remember(&mut self, ticket: &CatalogSearchTicket, paths: impl Iterator<Item = String>) {
        if let Some(current) = self
            .tickets
            .iter_mut()
            .find(|current| current.id == ticket.id && current.epoch == ticket.epoch)
        {
            for path in paths {
                if current.waiting.len() == CATALOG_SEARCH_CAPACITY {
                    break;
                }
                current.waiting.insert(path);
            }
        }
    }

    fn remember_component(
        &mut self,
        ticket: &CatalogSearchTicket,
        seeds: &[String],
        paths: impl Iterator<Item = String>,
    ) {
        let Some(current) = self.tickets.iter_mut().find(|current| {
            current.epoch == ticket.epoch
                && current.query == ticket.query
                && current.harness == ticket.harness
                && (current.id == ticket.id
                    || seeds.iter().any(|path| current.waiting.contains(path)))
        }) else {
            return;
        };
        let paths: BTreeSet<_> = paths.collect();
        let waiting: BTreeSet<_> = current.waiting.union(&paths).cloned().collect();
        // Retain the entire bounded attempted component for ordinary exclusion.
        // Extra renewed advisory interests can wait for FIFO or another read;
        // scheduling capacity never changes semantic component qualification.
        current.waiting = if waiting.len() > CATALOG_SEARCH_CAPACITY {
            paths
        } else {
            waiting
        };
    }

    fn acknowledge(&mut self, epoch: u64, tickets: &BTreeSet<u64>, published: &[String]) {
        self.tickets.retain_mut(|ticket| {
            if ticket.epoch != epoch || !tickets.contains(&ticket.id) {
                return true;
            }
            if ticket.waiting.is_empty() {
                return true;
            }
            for path in published {
                ticket.waiting.remove(path);
            }
            !ticket.waiting.is_empty()
        });
    }

    fn resolve(&mut self, ticket: &CatalogSearchTicket, pending: &[String]) {
        if let Some(current) = self
            .tickets
            .iter_mut()
            .find(|current| current.id == ticket.id && current.epoch == ticket.epoch)
        {
            current
                .waiting
                .retain(|path| !ticket.waiting.contains(path) || pending.contains(path));
        }
        self.remember(ticket, pending.iter().cloned());
        self.tickets.retain(|current| {
            current.id != ticket.id || current.epoch != ticket.epoch || !current.waiting.is_empty()
        });
    }

    fn close(&mut self) {
        self.active = false;
        self.tickets.clear();
        self.ordinary_due = false;
    }

    fn open(&mut self) {
        if !self.active {
            self.epoch += 1;
            self.tickets.clear();
            self.active = true;
        }
    }
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

    /// Host-authorized physical placement for each independent Source. A
    /// configured backend error never falls back to the machine's cache path.
    pub fn with_configured_machines(
        machines: Vec<(String, Options, crate::storage::StoreConfig)>,
    ) -> Result<Self, crate::storage::StorageError> {
        let machines = machines
            .into_iter()
            .map(|(key, options, config)| {
                if options.source.namespace()? != config.namespace {
                    return Err(crate::storage::StorageError::NamespaceMismatch);
                }
                let mut options = options;
                options.source = options.source.with_store(config)?;
                Ok((key, NativeEvidence::new(options)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_evidence(machines))
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

    /// Wait for the first readable Catalog page through this machine set's
    /// existing source owner; call off-reactor. Unknown, invalid or retired
    /// sources return `Ok(false)`. Deadline, closure and producer/storage
    /// failures propagate from the existing owner.
    pub fn wait_until_initial_catalog(
        &self,
        source_key: &str,
        pool: Arc<crate::RefreshPool>,
        deadline: Instant,
    ) -> io::Result<bool> {
        if self.is_retired() || !self.inventory.valid() {
            return Ok(false);
        }
        let Some(index) = self.inventory.index(source_key) else {
            return Ok(false);
        };
        let view = self.view(index);
        view.set_refresh(crate::Refresh::OnInvalidate);
        if self.is_retired() {
            view.retire();
            return Ok(false);
        }
        view.wait_until_initial_catalog_in_pool(pool, deadline)?;
        Ok(true)
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
        let config = options.source.selected_store().cloned();
        Self::with_store(options, config)
    }

    fn with_store(
        options: Options,
        store_config: Option<crate::storage::StoreConfig>,
    ) -> Arc<Self> {
        Arc::new(Self {
            options,
            store_config,
            work: Mutex::default(),
            catalog_demand: AtomicBool::new(false),
            catalog_search: Mutex::default(),
            query: RwLock::default(),
            closed: AtomicBool::new(false),
            audit_request: AtomicU64::new(0),
            audit_completed: AtomicU64::new(0),
        })
    }

    pub(crate) fn query_catalog(
        &self,
    ) -> Result<Box<dyn crate::storage::query::QueryCatalogStore>, crate::storage::StorageError>
    {
        match &self.store_config {
            Some(config) => crate::storage::configured_query_catalog(&self.options, config.clone()),
            None => {
                crate::storage::local_query_catalog(&self.options, self.options.source.namespace()?)
            }
        }
    }

    pub(crate) fn query_runtime(&self) -> io::Result<String> {
        #[cfg(feature = "postgres")]
        if let Some(crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Postgres { pool, control },
            namespace,
            ..
        }) = &self.store_config
        {
            return crate::storage::postgres::current_sources::Runtime::open(
                pool,
                namespace,
                control,
                &self.options,
            )
            .map_err(|error| {
                #[cfg(test)]
                eprintln!("transfer_query_stage pg_runtime_open_refused");
                io::Error::other(error)
            })?
            .generation(&self.options)
            .inspect_err(|_| {
                #[cfg(test)]
                eprintln!("transfer_query_stage pg_runtime_generation_refused");
            });
        }
        query_runtime_generation(&self.options)
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

    /// Search interest only changes producer scheduling. HTTP readers neither
    /// open native files nor acquire the producer's Work/publication fence.
    pub(crate) fn catalog_search_epoch(&self) -> u64 {
        lock(&self.catalog_search).epoch
    }

    pub(crate) fn note_catalog_search_result(
        &self,
        epoch: u64,
        interest: Option<(&str, Option<&str>)>,
        paths: Result<Vec<String>, ()>,
    ) -> bool {
        let mut queue = lock(&self.catalog_search);
        if queue.epoch != epoch || paths.as_ref().is_ok_and(Vec::is_empty) {
            return true;
        }
        queue.note(interest, paths)
    }

    #[cfg(test)]
    pub(crate) fn note_catalog_search(&self, interest: Option<(&str, Option<&str>)>) {
        let Some((query, harness)) = interest else {
            return;
        };
        let epoch = self.catalog_search_epoch();
        let mut request = crate::catalog::Request::parse("").unwrap();
        request.q = Some(query.to_owned());
        request.harness = harness.map(str::to_owned);
        let mut accepted = true;
        let mut observed = false;
        let mut on_search = |admission: crate::catalog::CatalogSearchAdmission| {
            observed = true;
            accepted = self.note_catalog_search_result(epoch, interest, admission.paths);
            accepted
        };
        let reply = crate::storage::read_local_catalog_page_with_search(
            &self.options,
            "test",
            &request,
            Some(&mut on_search),
        );
        if !observed && !matches!(reply, Ok(ref reply) if reply.status == 200) {
            accepted = self.note_catalog_search_result(epoch, interest, Err(()));
        }
        assert!(
            accepted,
            "test search admission exceeds the pending-path capacity"
        );
    }

    fn acknowledge_catalog_search(&self, cache: &EventCache, prepared: &model::PreparedSources) {
        let (epoch, tickets, waiting) = {
            let queue = lock(&self.catalog_search);
            (
                queue.epoch,
                queue.tickets.iter().map(|ticket| ticket.id).collect(),
                queue
                    .tickets
                    .iter()
                    .flat_map(|ticket| ticket.waiting.iter().cloned())
                    .collect::<BTreeSet<_>>(),
            )
        };
        if waiting.is_empty() {
            return;
        }
        let paths: Vec<_> = prepared
            .claims
            .iter()
            .map(|(path, _)| path.clone())
            .chain(
                prepared
                    .indexed_sources
                    .iter()
                    .map(|(path, _)| path.clone()),
            )
            .chain(
                prepared
                    .withdrawals
                    .iter()
                    .map(|source| source.path.clone()),
            )
            .filter(|path| waiting.contains(path))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let Ok(claims) = cache.catalog_claims(&paths) else {
            // Publication already committed. Retain advisory demand for a
            // later pending-path probe without losing its progress signal.
            return;
        };
        let published: Vec<_> = claims
            .into_iter()
            .filter_map(|(path, pending)| pending.is_none().then_some(path))
            .collect();
        lock(&self.catalog_search).acknowledge(epoch, &tickets, &published);
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
            && !work
                .events
                .as_mut()
                .expect("event cache initialized")
                .note_catalog_audit(generation.as_deref(), now)?
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
            work.events = Some(match &self.store_config {
                Some(config) => EventCache::open_configured(config).map_err(io::Error::other),
                None => EventCache::open_scoped(&self.options.cache, &self.options.source)
                    .map_err(io::Error::other),
            }?);
        }
        let cache = work.events.as_mut().expect("event cache initialized");
        #[cfg(feature = "postgres")]
        if self.store_config.as_ref().is_some_and(|config| {
            matches!(
                config.backend,
                crate::storage::StoreBackend::Postgres { .. }
            )
        }) {
            // Facts consumption belongs to this producer, before any semantic
            // transaction. Later runtime checks reuse its bounded PG header.
            cache.prepare_facts_authority(&self.options)?;
        }
        // The index commits each file's change as it reads it: nothing is
        // left to save.
        let mut dirty = false;
        if self.options.facts.is_none() {
            cache.refresh_reported_runs(&self.options.claude_json, now, &mut dirty)?;
        }
        Ok(())
    }

    /// Advance cold candidate discovery without reading native bodies. Workers
    /// yield after 512 entries or 64 accepted candidates; only the 64 newest
    /// candidates remain resident between steps.
    pub(crate) fn prepare_catalog(
        &self,
        work: &mut Work,
        now: i64,
    ) -> io::Result<CatalogPreparation> {
        let _timing = crate::native_timing::Phase::new(&self.options.cache, "cold_total");
        self.prepare_events(work, now)?;
        let cache = work.events.as_mut().expect("event cache initialized");
        let state = cache.backfill_state().inspect_err(|_| {
            #[cfg(test)]
            eprintln!("native catalog phase=backfill_state");
        })?;
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
        lock(&self.catalog_search).open();
        let discovery = work.cold_catalog.get_or_insert_with(|| ColdCatalog {
            revision: self.audit_request.load(Ordering::Acquire),
            ..Default::default()
        });
        if state.as_deref() != Some("draining") {
            let discovery_started =
                crate::native_timing::ColdDiscoveryProgress::start(crate::native_timing::enabled());
            let recovery_complete = {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "recovery_discovery");
                cache.discover_recovery_sources().inspect_err(|_| {
                    #[cfg(test)]
                    eprintln!("native catalog phase=recovery_page");
                })?
            };
            let page = {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "path_discovery");
                discovery.cursor.next(&self.options, 64, 512)?
            };
            debug_assert!(page.examined <= 512);
            let candidates = page.inputs.len();
            {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "discovery_publication");
                cache
                    .discover_catalog_sources(
                        &self.options,
                        &page.inputs,
                        page.complete && recovery_complete,
                    )
                    .inspect_err(|_| {
                        #[cfg(test)]
                        eprintln!("native catalog phase=discovery_publication");
                    })?;
            }
            {
                let _timing =
                    crate::native_timing::Phase::new(&self.options.cache, "candidate_metadata");
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
            }
            discovery.timing.page(
                &self.options.cache,
                discovery_started,
                page.examined,
                candidates,
                (page.complete, recovery_complete),
            );
            if !page.complete || !recovery_complete {
                return Ok(CatalogPreparation::Pending);
            }
        }
        if cache.semantic_origin_pending()? {
            let runtime = self.query_runtime()?;
            if self.begin_catalog_stage(cache, &[], &runtime)? != crate::events::Outcome::Written {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "cold origin activation changed",
                ));
            }
        }
        let use_newest =
            cache.session_catalog_generation().is_none() && !discovery.newest.is_empty();
        let mut attempted_ticket = None;
        let inputs = if use_newest {
            // Whole JSON intents replay their dependencies before the first
            // native publication. Inspect raw pending paths without mapping
            // unrelated FIFO sources into this newest native cohort.
            if let Some(path) = cache
                .pending_catalog_sources(crate::events::CATALOG_SEARCH_LIMIT)?
                .iter()
                .find(|path| path.ends_with(".json"))
            {
                cache.resolve_catalog_input(&self.options, path)?;
                return Ok(CatalogPreparation::Pending);
            }
            // Retain the selection until its first publication commits. If the
            // identity gate refuses it, bounded stage progress can complete
            // owner indexing before the one-shot handoff retries this group.
            discovery
                .newest
                .iter()
                .map(|(_, input)| input.clone())
                .collect::<Vec<_>>()
        } else {
            let (ticket, ordinary_due, excluded) = {
                let queue = lock(&self.catalog_search);
                let ticket = queue.tickets.front().cloned();
                let excluded: BTreeSet<_> = queue
                    .tickets
                    .iter()
                    .flat_map(|ticket| ticket.waiting.iter().cloned())
                    .collect();
                (
                    ticket,
                    queue.ordinary_due,
                    excluded.into_iter().collect::<Vec<_>>(),
                )
            };
            let interest = ticket
                .as_ref()
                .map(|ticket| crate::events::CatalogSearchInterest {
                    query: ticket.query.as_str(),
                    harness: ticket.harness.as_deref(),
                });
            let paths: Vec<_> = ticket
                .as_ref()
                .map(|ticket| ticket.waiting.iter().cloned().collect())
                .unwrap_or_default();
            let ranges = EventCache::catalog_search_ranges(
                &self.options,
                interest.as_ref().and_then(|interest| interest.harness),
            );
            let retry = interest
                .as_ref()
                .filter(|_| paths.is_empty())
                .map(|interest| crate::events::CatalogSearchLookup {
                    query: interest.query,
                    ranges: &ranges,
                });
            let preferred =
                (ticket.is_some() || ordinary_due).then_some(crate::events::CatalogPendingSearch {
                    paths: &paths,
                    retry,
                    yield_to_fifo: ordinary_due,
                    excluded: &excluded,
                });
            let batch = match cache.prioritized_catalog_sources(
                crate::events::CATALOG_SEARCH_LIMIT,
                preferred.as_ref(),
            ) {
                Ok(batch) => batch,
                Err(error) => {
                    if ticket.is_some() {
                        let mut queue = lock(&self.catalog_search);
                        queue.ordinary_due = true;
                        queue.next();
                    }
                    return Err(error);
                }
            };
            {
                let mut queue = lock(&self.catalog_search);
                if batch.search_checked {
                    // Reserve ordinary progress even when this attempt fails,
                    // is requeued, or completes the last search ticket.
                    queue.ordinary_due = true;
                    if ticket.is_some() {
                        queue.next();
                    }
                } else if ordinary_due
                    && batch.tier == crate::events::CatalogPendingTier::Fifo
                    && !batch.paths.is_empty()
                {
                    queue.ordinary_due = false;
                }
            }
            let resolved_empty =
                batch.search_checked && batch.tier == crate::events::CatalogPendingTier::Fifo;
            if resolved_empty && let Some(ticket) = &ticket {
                // This probe is already conclusive even if unrelated FIFO
                // sidecar resolution or source mapping subsequently fails.
                lock(&self.catalog_search).resolve(ticket, &[]);
            }
            // Remembered closures can cross harness boundaries. Their pending
            // paths were already admitted through the native source facade.
            let interest = if paths.is_empty() { interest } else { None };
            let (tier, pending, checked) =
                EventCache::catalog_pending_inputs(&self.options, batch, interest)?;
            if tier == crate::events::CatalogPendingTier::SearchHint {
                attempted_ticket = ticket.clone();
            }
            if !resolved_empty && let (Some(ticket), Some(checked)) = (&ticket, checked) {
                lock(&self.catalog_search).resolve(ticket, &checked);
            }
            if let Some(input) = pending.iter().find(|input| input.path.ends_with(".json")) {
                cache.resolve_catalog_input(
                    &self.options,
                    input.full_path(&self.options).to_string_lossy().as_ref(),
                )?;
                return Ok(CatalogPreparation::Pending);
            }
            pending
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
                let prepared = self.prepare_catalog_component_mode(
                    cache,
                    seeds.clone(),
                    true,
                    now,
                    attempted_ticket.as_ref(),
                );
                cache.scan_byte_budget = None;
                if let Some(prepared) = prepared.inspect_err(|_| {
                    #[cfg(test)]
                    eprintln!("native catalog phase=cold_component");
                })? {
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
                    if self.begin_catalog_stage(cache, &paths, &self.query_runtime()?)?
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
            let (outcome, _) = cache
                .publish_partial_catalog_fenced(&prepared)
                .inspect_err(|_| {
                    #[cfg(test)]
                    eprintln!("native catalog phase=cold_partial_publication");
                })?;
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
            if use_newest {
                discovery.newest.clear();
            }
            let _timing = crate::native_timing::Phase::new(&self.options.cache, "cold_cleanup");
            crate::events::store::requeue_concurrent_sources(&self.options, &prepared)?;
            self.acknowledge_catalog_search(cache, &prepared);
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

    fn begin_catalog_stage(
        &self,
        cache: &mut EventCache,
        paths: &[String],
        runtime: &str,
    ) -> io::Result<crate::events::Outcome> {
        // The store checks journal authority in the stage transaction.
        if self.options.facts.is_some() {
            cache.begin_semantic_stage_with_journal(paths, runtime, true)
        } else {
            cache.begin_semantic_stage(paths, runtime)
        }
    }

    fn semantic_write_conflict(
        &self,
        cache: &EventCache,
        batch: &model::relation_stage::Batch,
        operation: &'static str,
        outcome: &crate::events::Outcome,
    ) -> io::Error {
        // Fixed labels, bounded counters and an existing opaque cache ID only.
        // Never include paths, runtime/facts values, claims or native payloads.
        let producer = crate::native_timing::id(&EventCache::path(&self.options.cache));
        let kind = if operation == "finish" {
            "completion"
        } else {
            "publication"
        };
        let reason =
            cache
                .semantic_rejection()
                .unwrap_or(if *outcome == crate::events::Outcome::Stale {
                    "store_version"
                } else {
                    "unspecified"
                });
        io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "semantic {kind} changed: producer={producer} operation={operation} phase={:?} epoch={} revision={} sources={} outcome={outcome:?} reason={reason}",
                batch.phase,
                batch.epoch,
                batch.source_revision,
                batch.paths.len(),
            ),
        )
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
        let runtime = self.query_runtime()?;
        match self.begin_catalog_stage(cache, &[], &runtime)? {
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
        // Identity batches have now filled catalog_semantic_owner_sources.
        // Give the retained cold selection one ordinary facade attempt before
        // this stage proceeds to relationship/publication work. A reset gets
        // a new handoff because its key epoch changes.
        if matches!(batch.phase, Phase::Relations | Phase::Publication)
            && cache.semantic_origin_pending()?
            && let Some(origin) = cache.semantic_origin_publication()?
            && origin.held_epoch == batch.epoch
            && let Some(discovery) = &mut work.cold_catalog
        {
            let binding = (origin.origin.0, origin.held_epoch, origin.key_epoch);
            if discovery.origin_identity_handoff != Some(binding) {
                discovery.origin_identity_handoff = Some(binding);
                work.pending_sources = true;
                return Ok(false);
            }
        }
        #[cfg(test)]
        eprintln!(
            "native_stage phase={:?} epoch={} revision={} sources={}",
            batch.phase,
            batch.epoch,
            batch.source_revision,
            batch.paths.len()
        );
        work.pending_sources = true;
        // Runtime/facts qualification also covers scalar reductions and the
        // durable final acknowledgement, which may have no selected sources.
        before_build()?;
        work.built_at = Some(Instant::now());
        if batch.paths.is_empty() {
            if batch.phase == Phase::Publication {
                let outcome = cache.finish_semantic_stage(batch.epoch)?;
                if outcome != crate::events::Outcome::Written {
                    return Err(self.semantic_write_conflict(cache, &batch, "finish", &outcome));
                }
            }
            return Ok(true);
        }
        let paths: BTreeSet<_> = batch.paths.iter().cloned().collect();
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
        let prepared = if batch.phase == Phase::Publication {
            cache.prepare_semantic_model_batch(&catalog_options, &inputs, &batch, now)
        } else {
            model::prepare_semantic_batch(&catalog_options, cache, &inputs, &batch, now)
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
            Err(error) => {
                if let Some(repair) = error
                    .get_ref()
                    .and_then(|cause| cause.downcast_ref::<crate::events::TreeProjectionRepair>())
                {
                    repair.queue(cache)?;
                }
                return Err(error);
            }
        };
        let prepared = prepared.prepared();
        if let Some(discovery) = &mut work.cold_catalog {
            discovery.pids.extend(prepared.pids.iter().copied());
        }
        let (outcome, generation) = if batch.phase == Phase::Publication {
            cache.publish_semantic_catalog(&batch, prepared)?
        } else {
            (cache.record_semantic_batch(&batch, prepared)?, None)
        };
        #[cfg(test)]
        eprintln!(
            "native_stage outcome={outcome:?} generation={}",
            generation.is_some()
        );
        if outcome == crate::events::Outcome::Deferred {
            cache.end_scan();
            cache.release_published_indexes();
            work.texts.release_published_derivations();
            return Ok(true);
        }
        if outcome != crate::events::Outcome::Written {
            return Err(self.semantic_write_conflict(
                cache,
                &batch,
                if batch.phase == Phase::Publication {
                    "publish"
                } else {
                    "record"
                },
                &outcome,
            ));
        }
        if batch.phase == Phase::Publication {
            work.observed_catalog_generation = generation;
            crate::events::store::requeue_concurrent_sources(&self.options, prepared)?;
            self.acknowledge_catalog_search(cache, prepared);
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
        self.prepare_catalog_component_mode(cache, inputs, false, now, None)
    }
    fn prepare_catalog_component_mode(
        &self,
        cache: &mut EventCache,
        inputs: Vec<crate::Input>,
        recovering: bool,
        now: i64,
        ticket: Option<&CatalogSearchTicket>,
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
        let seeds: Vec<_> = inputs
            .iter()
            .map(|input| {
                input
                    .full_path(&self.options)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
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
            let component = cache
                .catalog_component_paths(&selected, &keys, &native_parents, &self.options)
                .inspect_err(|_| {
                    #[cfg(test)]
                    eprintln!("native catalog phase=component_lookup_before");
                })?;
            measurements.record("lookup_before", started, selected.len());
            let Some(component) = component else {
                break None;
            };
            inputs.extend(component);
            if inputs.len() > 64 {
                break None;
            }
            if let Some(ticket) = ticket {
                lock(&self.catalog_search).remember_component(
                    ticket,
                    &seeds,
                    inputs.iter().map(|input| {
                        input
                            .full_path(&self.options)
                            .to_string_lossy()
                            .into_owned()
                    }),
                );
            }
            let selected: Vec<_> = inputs.iter().cloned().collect();
            let started = measurements.start();
            let prepared =
                model::prepare_sources(&options, cache, &selected, now).inspect_err(|_| {
                    #[cfg(test)]
                    eprintln!("native catalog phase=component_prepare");
                })?;
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
        #[cfg(feature = "postgres")]
        if let Some(crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Postgres { pool, control },
            ..
        }) = &self.store_config
        {
            return crate::storage::postgres::navigation::query_target(
                self.store_config.as_ref().expect("configured source"),
                pool,
                control,
                key,
            )
            .map_err(io::Error::other);
        }
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
        before_build: impl FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        let complete = self.refresh_catalog_step(work, now, before_build)?;
        if complete {
            lock(&self.catalog_search).close();
        }
        Ok(complete && !work.tree_repair_pending)
    }

    fn refresh_catalog_step(
        &self,
        work: &mut Work,
        now: i64,
        mut before_build: impl FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        #[cfg(test)]
        let cold_diagnostic = work.cold_catalog.is_some() || work.catalog_snapshot.is_none();
        #[cfg(test)]
        let timing = crate::native_timing::enabled();
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
        #[cfg(test)]
        let prepare_events_started = timing.then(std::time::Instant::now);
        self.prepare_events(work, now).inspect_err(|_| {
            #[cfg(test)]
            eprintln!("native catalog phase=prepare_events");
        })?;
        #[cfg(test)]
        let prepare_events_us =
            prepare_events_started.map_or(0, |started| started.elapsed().as_micros());
        #[cfg(test)]
        let tree_repair_started = timing.then(std::time::Instant::now);
        work.tree_repair_pending = work
            .events
            .as_mut()
            .expect("event cache initialized")
            .advance_tree_repair(&self.options)
            .inspect_err(|_| {
                #[cfg(test)]
                eprintln!("native catalog phase=tree_repair");
            })?;
        #[cfg(test)]
        let tree_repair_us = tree_repair_started.map_or(0, |started| started.elapsed().as_micros());
        #[cfg(test)]
        let semantic_stage_started = timing.then(std::time::Instant::now);
        let semantic_stage_pending = self
            .advance_semantic_stage(work, now, &mut before_build)
            .inspect_err(|_| {
                #[cfg(test)]
                eprintln!("native catalog phase=stage_resume");
            })?;
        #[cfg(test)]
        let semantic_stage_us =
            semantic_stage_started.map_or(0, |started| started.elapsed().as_micros());
        #[cfg(test)]
        if timing && cold_diagnostic {
            let producer =
                crate::native_timing::id(&crate::events::EventCache::path(&self.options.cache));
            eprintln!(
                "semon sessions native: producer={producer} phase=cold_refresh_wrapper prepare_events_us={prepare_events_us} tree_repair_us={tree_repair_us} advance_semantic_stage_us={semantic_stage_us}"
            );
        }
        if semantic_stage_pending {
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
                    &self.query_runtime()?,
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
                            if !cache.note_catalog_audit(generation.as_deref(), now)? {
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
            if self.begin_catalog_stage(cache, &staged_paths, &self.query_runtime()?)?
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
                if self.begin_catalog_stage(cache, &staged_paths, &self.query_runtime()?)?
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
        match self.prepare_catalog(work, now).inspect_err(|_| {
            #[cfg(test)]
            eprintln!("native catalog phase=cold_prepare");
        })? {
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
                if cache
                    .finish_catalog_backfill(&generation)
                    .inspect_err(|_| {
                        #[cfg(test)]
                        eprintln!("native catalog phase=finish_backfill table=meta");
                    })?
                    != crate::events::Outcome::Written
                {
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
        work.tree_repair_pending
            || work.cold_catalog.is_some()
            || work.audit.is_some()
            || (work.pending_sources
                && self.audit_request.load(Ordering::Acquire) == work.audit_revision)
    }

    pub(crate) fn catalog_pending(&self, work: &Work) -> bool {
        #[cfg(test)]
        let cold_diagnostic = work.cold_catalog.is_some();
        #[cfg(test)]
        let timing = crate::native_timing::enabled();
        #[cfg(test)]
        let started = timing.then(std::time::Instant::now);
        let pending = work.tree_repair_pending
            || work
                .events
                .as_ref()
                .is_some_and(EventCache::tree_repair_pending)
            || work.cold_catalog.is_some()
            || work.pending_sources
            || work.audit.is_some()
            || self.audit_request.load(Ordering::Acquire) != work.audit_revision;
        #[cfg(test)]
        if timing && cold_diagnostic {
            let elapsed_us = started.map_or(0, |started| started.elapsed().as_micros());
            let producer =
                crate::native_timing::id(&crate::events::EventCache::path(&self.options.cache));
            eprintln!(
                "semon sessions native: producer={producer} phase=cold_catalog_pending elapsed_us={elapsed_us}"
            );
        }
        pending
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
    tree_repair_pending: bool,
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
    timing: crate::native_timing::ColdDiscoveryProgress,
    origin_identity_handoff: Option<(i64, i64, i64)>,
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
    local_runtime_generation(options)
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

/// Files that contribute to recorded runtime independently of immutable Facts.
/// Lifecycle validation observes these stamps without parsing the manifest.
pub(crate) fn recorded_activity_generation(options: &Options) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut active = BTreeMap::new();
    watch_tree(
        &options.claude_home.join("sessions"),
        &[".json"],
        &mut active,
    );
    let active = active
        .into_iter()
        .filter(|(path, _)| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|(path, stamp)| (path, stamp.map(|stamp| stamp.generation)))
        .collect::<Vec<_>>();
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&active).map_err(io::Error::other)?)
    ))
}

/// Aggregate generation for current local process and naming observations.
fn local_runtime_generation(options: &Options) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut watched = BTreeMap::new();
    watch_tree(
        &options.claude_home.join("sessions"),
        &[".json"],
        &mut watched,
    );
    watched.insert(options.claude_json.clone(), stamp(&options.claude_json));
    let mut processes = BTreeMap::new();
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
    let locks = writer_locks(options, &ids);
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

    fn cold_search_connection(options: &Options) -> rusqlite::Connection {
        let config = options.source.config(&options.cache).unwrap();
        crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection()
    }

    fn cold_search_claim(db: &rusqlite::Connection, path: &str, revision: i64) {
        let tx = db.unchecked_transaction().unwrap();
        // Preserve the test's explicit FIFO order while keeping the namespace
        // journal at least as new as every claim, as real invalidation does.
        tx.execute("UPDATE storage_source_journal SET revision=max(revision,?1) WHERE namespace=semon_namespace()", [revision]).unwrap();
        tx.execute("INSERT INTO session_catalog_invalidations(namespace,source_path,revision,reason,observed_at) VALUES(semon_namespace(),?1,?2,'changed',0) ON CONFLICT(namespace,source_path) DO UPDATE SET revision=excluded.revision", rusqlite::params![path, revision]).unwrap();
        tx.commit().unwrap();
    }

    fn cold_search_reader() -> (
        PathBuf,
        Arc<NativeEvidence>,
        crate::ViewerCore,
        rusqlite::Connection,
    ) {
        cold_search_reader_with_shared_roots(false)
    }

    fn cold_search_reader_with_shared_roots(
        shared_roots: bool,
    ) -> (
        PathBuf,
        Arc<NativeEvidence>,
        crate::ViewerCore,
        rusqlite::Connection,
    ) {
        let (root, mut options, _) = audit_fixture();
        if shared_roots {
            options.claude_home = options.codex_home.clone();
        }
        let initial = NativeEvidence::new(options.clone());
        drain_catalog(&initial, &std::cell::Cell::new(0));
        drop(initial);
        let db = cold_search_connection(&options);
        db.execute_batch("UPDATE meta SET value='partial' WHERE namespace=semon_namespace() AND key='catalog_completeness'; UPDATE storage_source_journal SET backfill_state='draining' WHERE namespace=semon_namespace();").unwrap();
        let evidence = NativeEvidence::new(options);
        let core =
            crate::ViewerCore::from_sources(Arc::new(SessionSources::from_evidence(vec![(
                String::new(),
                evidence.clone(),
            )])));
        (root, evidence, core, db)
    }

    fn pending_search_source(
        evidence: &NativeEvidence,
        db: &rusqlite::Connection,
        id: &str,
    ) -> String {
        let path = evidence
            .options
            .codex_home
            .join(format!("sessions/2026/10/01/{id}.jsonl"));
        fs::write(&path, format!("{{\"type\":\"session_meta\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"payload\":{{\"id\":\"{id}\"}}}}\n")).unwrap();
        let path = path.to_string_lossy().into_owned();
        cold_search_claim(db, &path, 100);
        path
    }

    #[test]
    fn cold_catalog_search_both_typed_burst_orders_keep_live_exact_interest() {
        for exact_first in [false, true] {
            let (root, evidence, core, db) = cold_search_reader();
            let selected = pending_search_source(&evidence, &db, "selected");
            cold_search_claim(&db, "/unrelated/first.jsonl", 1);
            let mut work = lock(&evidence.work);
            if exact_first {
                assert_eq!(
                    core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                        .status,
                    200
                );
            }
            for index in 0..80 {
                for query in [
                    format!("q=typed-miss-{index}"),
                    "q=selected&harness=claude".into(),
                    "q=audit&harness=codex".into(),
                ] {
                    let reply = core.respond("GET", "/api/sessions", &query, None);
                    assert_eq!(
                        reply.status,
                        200,
                        "{}",
                        String::from_utf8_lossy(&reply.body)
                    );
                }
            }
            if !exact_first {
                assert_eq!(
                    core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                        .status,
                    200
                );
            }
            assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
            assert_eq!(
                lock(&evidence.catalog_search).tickets[0].waiting,
                BTreeSet::from([selected.clone()])
            );
            let before = crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get);
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap();
            assert_eq!(
                crate::events::CATALOG_DERIVED_SOURCES.with(std::cell::Cell::get) - before,
                1
            );
            assert!(!db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [&selected], |row| row.get::<_, bool>(0)).unwrap());
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE source_path=?1 AND native_id='selected')", [&selected], |row| row.get::<_, bool>(0)).unwrap());
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path='/unrelated/first.jsonl')", [], |row| row.get::<_, bool>(0)).unwrap());
            assert!(lock(&evidence.catalog_search).tickets.is_empty());
            drop(work);
            core.close();
            drop(db);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cold_catalog_search_real_capacity_preserves_readable_http_page() {
        let (root, evidence, core, db) = cold_search_reader();
        let work = lock(&evidence.work);
        for index in 0..=CATALOG_SEARCH_CAPACITY {
            let id = format!("pending-{index}");
            pending_search_source(&evidence, &db, &id);
            let reply = core.respond(
                "GET",
                "/api/sessions",
                &format!("q={id}&harness=codex"),
                None,
            );
            assert_eq!(reply.status, 200);
            if index == CATALOG_SEARCH_CAPACITY {
                let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
                assert!(body["items"].as_array().unwrap().is_empty());
                assert_eq!(body["search"]["index_complete"], false);
            }
        }
        pending_search_source(&evidence, &db, "audit");
        let reply = core.respond("GET", "/api/sessions", "q=audit&harness=codex", None);
        assert_eq!(reply.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["items"][0]["key"], "audit");
        assert_eq!(
            lock(&evidence.catalog_search).tickets.len(),
            CATALOG_SEARCH_CAPACITY
        );
        assert!(
            !lock(&evidence.catalog_search)
                .tickets
                .iter()
                .any(|ticket| ticket.query == "audit")
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM session_catalog_invalidations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            CATALOG_SEARCH_CAPACITY as i64 + 2,
            "skipping advisory hints must preserve every durable source claim"
        );
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_filters_other_harness_roots_before_candidate_limit() {
        let (root, evidence, core, db) = cold_search_reader();
        let selected = pending_search_source(&evidence, &db, "selected");
        for index in 0..80 {
            for path in [
                format!("/outside/{index}/selected.jsonl"),
                evidence
                    .options
                    .claude_home
                    .join(format!("projects/{index}/selected.jsonl"))
                    .to_string_lossy()
                    .into_owned(),
            ] {
                cold_search_claim(&db, &path, 1);
            }
        }
        let mut work = lock(&evidence.work);
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                .status,
            200
        );
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([selected.clone()])
        );
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&selected],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_rejected_hint_resolves_fifo_json_before_native_publication() {
        let (root, evidence, core, db) = cold_search_reader_with_shared_roots(true);
        let selected = evidence
            .options
            .claude_home
            .join("projects/project/selected.jsonl");
        fs::create_dir_all(selected.parent().unwrap()).unwrap();
        fs::write(&selected, serde_json::json!({"type":"user","uuid":"selected-message","sessionId":"selected","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"selected"}}).to_string()+"\n").unwrap();
        cold_search_claim(&db, selected.to_string_lossy().as_ref(), 100);
        let ordinary = pending_search_source(&evidence, &db, "ordinary");
        let runtime = crate::Input {
            root: crate::InputRoot::Claude,
            path: "sessions/99.json".into(),
        };
        let runtime_path = runtime.full_path(&evidence.options);
        let intent =
            crate::SessionSourceWrite::begin(&evidence.options, &runtime, None, 0).unwrap();
        fs::write(
            &runtime_path,
            r#"{"sessionId":"selected","name":"pending runtime"}"#,
        )
        .unwrap();
        intent
            .complete(fs::metadata(&runtime_path).unwrap().len())
            .unwrap();
        let lookup_fault = crate::storage::sqlite::source_journal::CatalogSearchFault::new();
        let mut work = lock(&evidence.work);
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                .status,
            200
        );
        assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
        assert!(lock(&evidence.catalog_search).tickets[0].waiting.is_empty());
        drop(lookup_fault);
        evidence.prepare_events(&mut work, model::now_ms()).unwrap();
        let generation = work.events.as_ref().unwrap().session_catalog_generation();
        assert!(matches!(
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap(),
            CatalogPreparation::Pending
        ));
        assert_eq!(
            work.events.as_ref().unwrap().session_catalog_generation(),
            generation
        );
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [runtime_path.to_string_lossy().as_ref()],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        for path in [selected.to_string_lossy().as_ref(), ordinary.as_str()] {
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [path], |row| row.get::<_, bool>(0)).unwrap());
        }
        assert!(lock(&evidence.catalog_search).tickets.is_empty());
        assert!((0..8).any(|_| {
            evidence
                .refresh_catalog(&mut work, model::now_ms())
                .unwrap()
        }));
        assert_ne!(
            work.events.as_ref().unwrap().session_catalog_generation(),
            generation
        );
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE native_id='selected' AND harness='claude')", [], |row| row.get::<_, bool>(0)).unwrap());
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE native_id='ordinary' AND harness='codex')", [], |row| row.get::<_, bool>(0)).unwrap());
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM session_catalog_invalidations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_full_mixed_hint_page_yields_to_real_fifo_completion() {
        let (root, evidence, core, db) = cold_search_reader_with_shared_roots(true);
        let claude = evidence
            .options
            .claude_home
            .join("projects/project/selected.jsonl");
        fs::create_dir_all(claude.parent().unwrap()).unwrap();
        fs::write(&claude, serde_json::json!({"type":"user","uuid":"selected-message","sessionId":"selected","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"selected"}}).to_string()+"\n").unwrap();
        cold_search_claim(&db, claude.to_string_lossy().as_ref(), 100);
        for index in 0..63 {
            let path = evidence
                .options
                .codex_home
                .join(format!("sessions/native-{index:03}/selected.jsonl"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":format!("selected-native-{index:03}")}}).to_string()+"\n").unwrap();
            cold_search_claim(&db, path.to_string_lossy().as_ref(), 100);
        }
        for index in 0..2 {
            pending_search_source(&evidence, &db, &format!("ordinary-{index}"));
        }
        let mut work = lock(&evidence.work);
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                .status,
            200
        );
        assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
        assert!(lock(&evidence.catalog_search).tickets[0].waiting.is_empty());
        evidence.prepare_events(&mut work, model::now_ms()).unwrap();
        let generation = work.events.as_ref().unwrap().session_catalog_generation();
        assert!(matches!(
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap(),
            CatalogPreparation::PublishedPending
        ));
        assert_ne!(
            work.events.as_ref().unwrap().session_catalog_generation(),
            generation
        );
        assert!(lock(&evidence.catalog_search).tickets.is_empty());
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM session_catalog_invalidations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert!(!work.events.as_ref().unwrap().qualified_catalog().unwrap());
        assert!((0..8).any(|_| {
            evidence
                .refresh_catalog(&mut work, model::now_ms())
                .unwrap()
        }));
        assert_eq!(db.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE native_id LIKE 'selected-native-%'", [], |row| row.get::<_, i64>(0)).unwrap(), 63);
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE native_id='selected' AND harness='claude')", [], |row| row.get::<_, bool>(0)).unwrap());
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM session_catalog_invalidations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert!(work.events.as_ref().unwrap().qualified_catalog().unwrap());
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_unqualified_fifo_claims_keep_native_authority_refusal() {
        let (root, evidence, core, db) = cold_search_reader();
        pending_search_source(&evidence, &db, "selected");
        for index in 0..64 {
            let path = evidence
                .options
                .codex_home
                .join(format!("aaa/{index}/selected.jsonl"));
            cold_search_claim(&db, path.to_string_lossy().as_ref(), 1);
        }
        let mut work = lock(&evidence.work);
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                .status,
            200
        );
        assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
        assert!(
            lock(&evidence.catalog_search).tickets[0].waiting.is_empty(),
            "an unqualified full page is unknown, never a successful empty lookup"
        );
        let error = match evidence.prepare_catalog(&mut work, model::now_ms()) {
            Err(error) => error,
            Ok(_) => {
                panic!("invalid journal source layouts require explicit qualification refusal")
            }
        };
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            error.to_string(),
            "Catalog source is outside the native allowlist"
        );
        assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
        assert!(!work.events.as_ref().unwrap().qualified_catalog().unwrap());
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM session_catalog_invalidations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            65
        );
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_unicode_query_uses_fifo_and_keeps_unicode_metadata_semantics() {
        let (root, evidence, core, db) = cold_search_reader();
        pending_search_source(&evidence, &db, "ÄID");
        let mut work = lock(&evidence.work);
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=%C3%A4id", None)
                .status,
            200
        );
        assert!(lock(&evidence.catalog_search).tickets.is_empty());
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        let reply = core.respond("GET", "/api/sessions", "q=%C3%A4id", None);
        assert_eq!(reply.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["items"][0]["key"], "ÄID");
        assert!(lock(&evidence.catalog_search).tickets.is_empty());
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_fresh_successful_hints_preserve_reserved_fifo_turns() {
        let (root, evidence, core, db) = cold_search_reader();
        for index in 0..130 {
            pending_search_source(&evidence, &db, &format!("ordinary-{index:03}"));
        }
        let mut work = lock(&evidence.work);
        for index in 0..4 {
            let id = format!("selected-{index}");
            let path = pending_search_source(&evidence, &db, &id);
            assert_eq!(
                core.respond(
                    "GET",
                    "/api/sessions",
                    &format!("q={id}&harness=codex"),
                    None
                )
                .status,
                200
            );
            if index > 0 {
                evidence
                    .prepare_catalog(&mut work, model::now_ms())
                    .unwrap();
                assert_eq!(db.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE native_id LIKE 'ordinary-%'", [], |row| row.get::<_, i64>(0)).unwrap(), (index * 64).min(130));
                assert_eq!(
                    lock(&evidence.catalog_search).tickets[0].waiting,
                    BTreeSet::from([path.clone()])
                );
            }
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap();
            assert!(lock(&evidence.catalog_search).tickets.is_empty());
            assert!(
                lock(&evidence.catalog_search).ordinary_due,
                "completed demand reserves the next ordinary turn"
            );
            assert!(!db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [&path], |row| row.get::<_, bool>(0)).unwrap());
        }
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_discarded_fifo_json_does_not_displace_hint() {
        let (root, evidence, core, db) = cold_search_reader();
        let selected = pending_search_source(&evidence, &db, "selected");
        cold_search_claim(&db, "/unrelated/first.json", 1);
        let mut work = lock(&evidence.work);
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                .status,
            200
        );
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&selected],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path='/unrelated/first.json')", [], |row| row.get::<_, bool>(0)).unwrap());
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_failed_oldest_hint_yields_to_unrelated_fifo() {
        for bytes in ["", "{invalid}\n"] {
            let (root, evidence, core, db) = cold_search_reader();
            let path = pending_search_source(&evidence, &db, "selected");
            db.execute(
                "UPDATE session_catalog_invalidations SET revision=1 WHERE source_path=?1",
                [&path],
            )
            .unwrap();
            for index in 0..130 {
                pending_search_source(&evidence, &db, &format!("ordinary-{index:03}"));
            }
            fs::write(&path, bytes).unwrap();
            let mut work = lock(&evidence.work);
            assert_eq!(
                core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                    .status,
                200
            );
            for expected in [64, 128, 130] {
                let error = match evidence.prepare_catalog(&mut work, model::now_ms()) {
                    Err(error) => error,
                    Ok(_) => panic!("unqualified hint must retain its refusal"),
                };
                assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
                assert_eq!(error.to_string(), "selected source cannot be qualified");
                evidence
                    .prepare_catalog(&mut work, model::now_ms())
                    .unwrap();
                assert_eq!(db.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE native_id LIKE 'ordinary-%'", [], |row| row.get::<_, i64>(0)).unwrap(), expected);
                assert_eq!(
                    lock(&evidence.catalog_search).tickets[0].waiting,
                    BTreeSet::from([path.clone()])
                );
                assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [&path], |row| row.get::<_, bool>(0)).unwrap());
                assert!(!work.events.as_ref().unwrap().qualified_catalog().unwrap());
            }
            drop(work);
            core.close();
            drop(db);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cold_catalog_search_changing_hint_yields_while_demand_remains() {
        use std::io::Write;
        let (root, evidence, core, db) = cold_search_reader();
        let path = pending_search_source(&evidence, &db, "selected");
        db.execute(
            "UPDATE session_catalog_invalidations SET revision=1 WHERE source_path=?1",
            [&path],
        )
        .unwrap();
        for index in 0..130 {
            pending_search_source(&evidence, &db, &format!("ordinary-{index:03}"));
        }
        let mut work = lock(&evidence.work);
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=selected&harness=codex", None)
                .status,
            200
        );
        for expected in [64, 128, 130] {
            let changed = path.clone();
            crate::events::BEFORE_CATALOG_PUBLICATION.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    writeln!(fs::OpenOptions::new().append(true).open(changed).unwrap(), "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"appended\"}}}}").unwrap();
                }));
            });
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap();
            assert_eq!(
                lock(&evidence.catalog_search).tickets[0].waiting,
                BTreeSet::from([path.clone()])
            );
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap();
            assert_eq!(db.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE native_id LIKE 'ordinary-%'", [], |row| row.get::<_, i64>(0)).unwrap(), expected);
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [&path], |row| row.get::<_, bool>(0)).unwrap());
            assert!(!work.events.as_ref().unwrap().qualified_catalog().unwrap());
        }
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_failed_lineage_excludes_whole_bounded_component() {
        let (root, evidence, core, db) = cold_search_reader();
        let parent = pending_search_source(&evidence, &db, "selected-parent");
        let child = pending_search_source(&evidence, &db, "related-child");
        fs::write(&child, serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"related-child","parent_thread_id":"selected-parent","thread_source":"subagent","agent_path":"selected-parent/child"}}).to_string()+"\n").unwrap();
        let mut work = lock(&evidence.work);
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        assert_eq!(db.query_row("SELECT COUNT(*) FROM session_catalog_sources WHERE native_id IN ('selected-parent','related-child')", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
        for path in [&parent, &child] {
            cold_search_claim(&db, path, 1);
        }
        let ordinary = pending_search_source(&evidence, &db, "ordinary-independent");
        fs::write(&parent, "{invalid}\n").unwrap();
        assert_eq!(
            core.respond(
                "GET",
                "/api/sessions",
                "q=selected-parent&harness=codex",
                None
            )
            .status,
            200
        );
        let error = match evidence.prepare_catalog(&mut work, model::now_ms()) {
            Err(error) => error,
            Ok(_) => panic!("the persisted parent must refuse qualification"),
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([parent.clone(), child.clone()])
        );
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        for path in [&parent, &child] {
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [path], |row| row.get::<_, bool>(0)).unwrap());
        }
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&ordinary],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE native_id='ordinary-independent')", [], |row| row.get::<_, bool>(0)).unwrap());
        assert_eq!(lock(&evidence.catalog_search).tickets[0].waiting.len(), 2);
        assert!(!work.events.as_ref().unwrap().qualified_catalog().unwrap());
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_ticket_capacity_keeps_small_component_out_of_semantic_stage() {
        let (root, evidence, core, db) = cold_search_reader();
        let parent = pending_search_source(&evidence, &db, "selected-parent");
        let child = pending_search_source(&evidence, &db, "related-child");
        fs::write(&child, serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"related-child","parent_thread_id":"selected-parent","thread_source":"subagent","agent_path":"selected-parent/child"}}).to_string()+"\n").unwrap();
        let mut work = lock(&evidence.work);
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        for path in [&parent, &child] {
            cold_search_claim(&db, path, 1);
        }
        let ordinary = pending_search_source(&evidence, &db, "ordinary-independent");
        let mut renewed_paths = vec![parent.clone()];
        for index in 0..63 {
            renewed_paths.push(pending_search_source(
                &evidence,
                &db,
                &format!("renewed-{index:03}"),
            ));
        }
        assert_eq!(
            core.respond(
                "GET",
                "/api/sessions",
                "q=selected-parent&harness=codex",
                None
            )
            .status,
            200
        );
        let attempted = lock(&evidence.catalog_search).tickets[0].clone();
        assert_eq!(attempted.waiting, BTreeSet::from([parent.clone()]));
        // Reproduce positive readmission between producer selection and closure
        // registration without changing the publication or source-read seams.
        let renewed_id = {
            let mut queue = lock(&evidence.catalog_search);
            assert!(queue.note(Some(("selected-parent", Some("codex"))), Ok(renewed_paths)));
            assert_eq!(queue.tickets[0].waiting.len(), 64);
            queue.tickets[0].id
        };
        assert_ne!(renewed_id, attempted.id);
        let input = catalog_input(Path::new(&parent), &evidence.options).unwrap();
        let prepared = evidence
            .prepare_catalog_component_mode(
                work.events.as_mut().unwrap(),
                vec![input],
                true,
                model::now_ms(),
                Some(&attempted),
            )
            .unwrap();
        assert!(
            prepared.is_some(),
            "advisory queue pressure cannot signal semantic component overflow"
        );
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage)",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        assert_eq!(lock(&evidence.catalog_search).tickets[0].id, renewed_id);
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([parent.clone(), child.clone()])
        );
        fs::write(&parent, "{invalid}\n").unwrap();
        let error = match evidence.prepare_catalog(&mut work, model::now_ms()) {
            Err(error) => error,
            Ok(_) => panic!("the bounded component must retain its qualification refusal"),
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&ordinary],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE native_id='ordinary-independent')", [], |row| row.get::<_, bool>(0)).unwrap());
        for path in [&parent, &child] {
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [path], |row| row.get::<_, bool>(0)).unwrap());
        }
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([parent, child])
        );
        assert!(!work.events.as_ref().unwrap().qualified_catalog().unwrap());
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage)",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_renewed_ticket_remembers_attempted_closure_without_stale_ack() {
        let mut queue = CatalogSearchQueue::default();
        queue.note(Some(("selected", None)), Ok(vec!["/selected".into()]));
        let old = queue.next().unwrap();
        queue.note(Some(("selected", None)), Ok(vec!["/selected".into()]));
        queue.remember_component(
            &old,
            &["/selected".into()],
            ["/selected".into(), "/related".into()].into_iter(),
        );
        queue.resolve(&old, &[]);
        queue.acknowledge(
            old.epoch,
            &BTreeSet::from([old.id]),
            &["/selected".into(), "/related".into()],
        );
        assert_eq!(
            queue.tickets[0].waiting,
            BTreeSet::from(["/selected".into(), "/related".into()])
        );
        let renewed = queue.next().unwrap();
        queue.resolve(&renewed, &["/selected".into(), "/related".into()]);
        assert_eq!(queue.tickets[0].waiting.len(), 2);
        queue.close();
        queue.open();
        queue.note(Some(("selected", None)), Ok(vec!["/selected".into()]));
        queue.remember_component(&old, &["/selected".into()], ["/related".into()].into_iter());
        assert_eq!(
            queue.tickets[0].waiting,
            BTreeSet::from(["/selected".into()])
        );
    }

    #[test]
    fn cold_catalog_search_renewed_same_path_survives_old_ack_and_resolution() {
        let mut queue = CatalogSearchQueue::default();
        queue.note(Some(("selected", None)), Ok(vec!["/selected".into()]));
        let old = queue.next().unwrap();
        queue.note(Some(("selected", None)), Ok(vec!["/selected".into()]));
        let renewed = queue.next().unwrap();
        assert_ne!(old.id, renewed.id);
        queue.acknowledge(old.epoch, &BTreeSet::from([old.id]), &["/selected".into()]);
        queue.resolve(&old, &[]);
        assert_eq!(queue.tickets.len(), 1);
        assert_eq!(
            queue.tickets[0].waiting,
            BTreeSet::from(["/selected".into()])
        );
        queue.acknowledge(
            renewed.epoch,
            &BTreeSet::from([renewed.id]),
            &["/selected".into()],
        );
        assert!(queue.tickets.is_empty());
    }

    #[test]
    fn cold_catalog_search_deleted_source_rowless_publication_acknowledges_claim() {
        let (root, evidence, core, db) = cold_search_reader();
        cold_search_claim(&db, "/unrelated/first.jsonl", 1);
        let mut work = lock(&evidence.work);
        let path = pending_search_source(&evidence, &db, "deleted");
        assert_eq!(
            core.respond("GET", "/api/sessions", "q=deleted&harness=codex", None)
                .status,
            200
        );
        fs::remove_file(&path).unwrap();
        evidence.prepare_events(&mut work, model::now_ms()).unwrap();
        let cache = work.events.as_mut().unwrap();
        let generation = cache.session_catalog_generation().unwrap();
        let input = catalog_input(Path::new(&path), &evidence.options).unwrap();
        let prepared = evidence
            .prepare_catalog_component_mode(cache, vec![input], true, model::now_ms(), None)
            .unwrap()
            .unwrap();
        assert!(
            prepared.rows.is_empty(),
            "an observed absence emits no Catalog source row"
        );
        assert_eq!(prepared.claims, vec![(path.clone(), Some(100))]);
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([path.clone()])
        );
        assert!(matches!(
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap(),
            CatalogPreparation::PublishedPending
        ));
        assert_ne!(
            work.events
                .as_ref()
                .unwrap()
                .session_catalog_generation()
                .unwrap(),
            generation,
            "the ordinary producer must commit the rowless publication"
        );
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&path],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        assert!(lock(&evidence.catalog_search).tickets.is_empty());
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path='/unrelated/first.jsonl')", [], |row| row.get::<_, bool>(0)).unwrap());
        drop(work);
        core.close();
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_unqualified_empty_and_invalid_sources_keep_pending_demand() {
        for (id, bytes) in [("empty", ""), ("invalid", "{invalid}\n")] {
            let (root, evidence, core, db) = cold_search_reader();
            cold_search_claim(&db, "/unrelated/first.jsonl", 1);
            let mut work = lock(&evidence.work);
            let path = pending_search_source(&evidence, &db, id);
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                core.respond(
                    "GET",
                    "/api/sessions",
                    &format!("q={id}&harness=codex"),
                    None
                )
                .status,
                200
            );
            let error = match evidence.prepare_catalog(&mut work, model::now_ms()) {
                Err(error) => error,
                Ok(_) => {
                    panic!("a present source without a native identity must refuse qualification")
                }
            };
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            assert_eq!(error.to_string(), "selected source cannot be qualified");
            assert!(
                work.events
                    .as_ref()
                    .unwrap()
                    .catalog_claims(std::slice::from_ref(&path))
                    .unwrap()[0]
                    .1
                    .is_some()
            );
            assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
            assert_eq!(
                lock(&evidence.catalog_search).tickets[0].waiting,
                BTreeSet::from([path])
            );
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path='/unrelated/first.jsonl')", [], |row| row.get::<_, bool>(0)).unwrap());
            drop(work);
            core.close();
            drop(db);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cold_catalog_search_first_generation_resolves_pending_sidecar_before_newest() {
        let (root, options, _) = audit_fixture();
        let child = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/parent/subagents/agent-child.jsonl".into(),
        };
        let child_path = child.full_path(&options);
        fs::create_dir_all(child_path.parent().unwrap()).unwrap();
        fs::write(&child_path, serde_json::json!({"type":"user","uuid":"child-message","sessionId":"parent","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"child"}}).to_string()+"\n").unwrap();
        let sidecar = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/parent/subagents/agent-child.meta.json".into(),
        };
        let sidecar_path = sidecar.full_path(&options);
        let intent = crate::SessionSourceWrite::begin(&options, &sidecar, None, 0).unwrap();
        fs::write(
            &sidecar_path,
            r#"{"description":"first-sidecar-label","toolUseId":"first-launch"}"#,
        )
        .unwrap();
        intent
            .complete(fs::metadata(&sidecar_path).unwrap().len())
            .unwrap();
        let evidence = NativeEvidence::new(options.clone());
        let mut work = lock(&evidence.work);
        let db = cold_search_connection(&options);
        let sidecar_pending = || {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [sidecar_path.to_string_lossy().as_ref()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
        };
        assert!(sidecar_pending());
        let mut resolved_before_publication = false;
        for _ in 0..16 {
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap();
            let generation = work.events.as_ref().unwrap().session_catalog_generation();
            if generation.is_some() {
                assert!(resolved_before_publication);
                assert!(!sidecar_pending());
                break;
            }
            if !sidecar_pending() {
                resolved_before_publication = true;
                assert!(
                    work.cold_catalog
                        .as_ref()
                        .unwrap()
                        .newest
                        .iter()
                        .any(|(_, input)| input == &child)
                );
                assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [child_path.to_string_lossy().as_ref()], |row| row.get::<_, bool>(0)).unwrap(), "sidecar replay must preserve its native dependency until publication");
            }
        }
        assert!(crate::session_catalog_page_ready(&options).unwrap());
        let metadata: String = db.query_row("SELECT c.metadata FROM session_catalog c JOIN session_catalog_sources s ON s.session_key=c.session_key WHERE s.source_path=?1", [child_path.to_string_lossy().as_ref()], |row| row.get(0)).unwrap();
        let row: model::summary::CatalogRow = serde_json::from_str(&metadata).unwrap();
        let source = row
            .sources
            .iter()
            .find(|source| source.path == child_path)
            .unwrap();
        assert_eq!(
            source.native_role.as_ref().unwrap()["Agent"]["label"],
            "first-sidecar-label"
        );
        assert_eq!(
            source.native_role.as_ref().unwrap()["Agent"]["tool_use_id"],
            "first-launch"
        );
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [child_path.to_string_lossy().as_ref()],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        drop(work);
        drop(evidence);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_newest_first_page_bypasses_unrelated_pending_paths() {
        let (root, options, input) = audit_fixture();
        let evidence = NativeEvidence::new(options.clone());
        let mut work = lock(&evidence.work);
        evidence.prepare_events(&mut work, model::now_ms()).unwrap();
        let db = cold_search_connection(&options);
        db.execute_batch("UPDATE storage_source_journal SET backfill_state='draining' WHERE namespace=semon_namespace();").unwrap();
        cold_search_claim(&db, "/outside/allowlist.jsonl", 1);
        work.cold_catalog = Some(ColdCatalog {
            newest: BTreeSet::from([(std::cmp::Reverse(None), input.clone())]),
            ..Default::default()
        });
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        assert!(
            work.events
                .as_ref()
                .unwrap()
                .session_catalog_generation()
                .is_some()
        );
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_sources WHERE native_id='audit')",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        drop(work);
        drop(evidence);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_queue_is_fair_bounded_and_rejects_stale_acknowledgements() {
        let mut queue = CatalogSearchQueue::default();
        assert!(queue.note(Some(("first", Some("codex"))), Ok(vec!["/first".into()])));
        assert!(queue.note(Some(("second", None)), Ok(vec!["/second".into()])));
        assert!(queue.note(None, Ok(Vec::new())));
        let first = queue.next().unwrap();
        assert_eq!(first.query, "first");
        assert_eq!(queue.next().unwrap().query, "second");
        queue.remember(&first, ["/first".to_owned()].into_iter());
        assert!(queue.note(Some(("first", Some("codex"))), Ok(vec!["/first".into()])));
        for index in 2..CATALOG_SEARCH_CAPACITY {
            assert!(queue.note(
                Some((&format!("tab-{index}"), None)),
                Ok(vec![format!("/tab-{index}")])
            ));
        }
        assert!(!queue.note(Some(("overflow", None)), Err(())));
        assert_eq!(queue.tickets.len(), CATALOG_SEARCH_CAPACITY);
        queue.close();
        assert!(queue.note(Some(("warm", None)), Ok(vec!["/warm".into()])));
        assert!(queue.tickets.is_empty());
        queue.open();
        queue.note(Some(("first", Some("codex"))), Ok(vec!["/first".into()]));
        let reposted = queue.next().unwrap();
        queue.remember(&reposted, ["/first".to_owned()].into_iter());
        queue.acknowledge(first.epoch, &BTreeSet::from([first.id]), &["/first".into()]);
        assert_eq!(queue.tickets.len(), 1);
        queue.acknowledge(
            reposted.epoch,
            &BTreeSet::from([first.id]),
            &["/first".into()],
        );
        assert_eq!(queue.tickets.len(), 1);
        queue.acknowledge(
            reposted.epoch,
            &BTreeSet::from([reposted.id]),
            &["/first".into()],
        );
        assert!(queue.tickets.is_empty());
    }

    #[test]
    fn cold_catalog_search_survives_lookup_error_and_scan_budget_until_qualified_publication() {
        use std::io::Write;
        let (root, options, _) = audit_fixture();
        let initial = NativeEvidence::new(options.clone());
        drain_catalog(&initial, &std::cell::Cell::new(0));
        initial.note_catalog_search(Some(("stale-warm", None)));
        assert!(lock(&initial.catalog_search).tickets.is_empty());
        drop(initial);
        let selected = crate::Input {
            root: crate::InputRoot::Codex,
            path: "sessions/2026/10/01/selected.jsonl".into(),
        };
        let ordinary = crate::Input {
            root: crate::InputRoot::Codex,
            path: "sessions/2026/10/01/ordinary.jsonl".into(),
        };
        for (input, id) in [(&selected, "selected"), (&ordinary, "ordinary")] {
            fs::write(input.full_path(&options), format!("{{\"type\":\"session_meta\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"payload\":{{\"id\":\"{id}\"}}}}\n")).unwrap();
        }
        let mut log = fs::OpenOptions::new()
            .append(true)
            .open(selected.full_path(&options))
            .unwrap();
        let record = format!(
            "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{}\"}}}}\n",
            "x".repeat(8192)
        );
        for _ in 0..640 {
            log.write_all(record.as_bytes()).unwrap();
        }
        drop(log);
        let selected_path = selected.full_path(&options).to_string_lossy().into_owned();
        let ordinary_path = ordinary.full_path(&options).to_string_lossy().into_owned();
        let db = cold_search_connection(&options);
        db.execute_batch("UPDATE meta SET value='partial' WHERE namespace=semon_namespace() AND key='catalog_completeness'; UPDATE storage_source_journal SET backfill_state='draining' WHERE namespace=semon_namespace();").unwrap();
        for (path, revision) in [(&ordinary_path, 100), (&selected_path, 101)] {
            cold_search_claim(&db, path, revision);
        }
        for index in 0..63 {
            let path = options
                .codex_home
                .join(format!("sessions/2026/10/01/ordinary-{index:03}.jsonl"));
            fs::write(&path, serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":format!("ordinary-{index:03}")}}).to_string()+"\n").unwrap();
            cold_search_claim(&db, path.to_string_lossy().as_ref(), 100);
        }
        let lookup_fault = crate::storage::sqlite::source_journal::CatalogSearchFault::new();
        let evidence = NativeEvidence::new(options.clone());
        evidence.note_catalog_search(Some(("selected", Some("codex"))));
        let mut work = lock(&evidence.work);
        assert!(
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .is_err()
        );
        assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
        assert!(matches!(
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap(),
            CatalogPreparation::PublishedPending
        ));
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM session_catalog_sources WHERE native_id LIKE 'ordinary%'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            64,
            "the reserved FIFO turn runs even while the hint lookup fails"
        );
        assert_eq!(lock(&evidence.catalog_search).tickets.len(), 1);
        drop(lookup_fault);
        assert!(matches!(
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap(),
            CatalogPreparation::Pending
        ));
        let indexed = work
            .events
            .as_ref()
            .unwrap()
            .indexed_stat(&selected.full_path(&options))
            .unwrap();
        assert!(indexed.size >= 4 * 1024 * 1024);
        assert!(
            indexed.size < fs::metadata(selected.full_path(&options)).unwrap().len(),
            "the first publication must contain a real bounded prefix"
        );
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage)",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap(),
            "a byte checkpoint alone uses ordinary publication and requeue"
        );
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([selected_path.clone()])
        );
        let mut published = false;
        for _ in 0..4 {
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap();
            let pending: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [&selected_path], |row| row.get(0)).unwrap();
            if !pending {
                published = true;
                assert!(lock(&evidence.catalog_search).tickets.is_empty());
                break;
            }
            assert_eq!(
                lock(&evidence.catalog_search).tickets.len(),
                1,
                "a requeued byte checkpoint does not acknowledge demand"
            );
        }
        assert!(published);
        assert!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [&ordinary_path],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
        drop(work);
        drop(evidence);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_survives_component_budget_stage_begin_until_publication() {
        use std::io::Write;
        let (root, options, _) = audit_fixture();
        let initial = NativeEvidence::new(options.clone());
        drain_catalog(&initial, &std::cell::Cell::new(0));
        drop(initial);
        let input = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/selected.jsonl".into(),
        };
        let path = input.full_path(&options);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut log = fs::File::create(&path).unwrap();
        for index in 0..65 {
            writeln!(log, "{}", serde_json::json!({"type":"user","sessionId":"selected","uuid":format!("peer-{index}"),"timestamp":"2026-10-01T00:00:00Z","origin":{"kind":"peer","from":"uds:/synthetic/peer.sock","name":"peer","msg_id":format!("message-{index}")},"message":{"role":"user","content":"peer message"}})).unwrap();
        }
        drop(log);
        let path = path.to_string_lossy().into_owned();
        let db = cold_search_connection(&options);
        db.execute_batch("UPDATE meta SET value='partial' WHERE namespace=semon_namespace() AND key='catalog_completeness'; UPDATE storage_source_journal SET backfill_state='draining' WHERE namespace=semon_namespace();").unwrap();
        for (pending, revision) in [("/unrelated/first.jsonl", 100), (path.as_str(), 101)] {
            cold_search_claim(&db, pending, revision);
        }
        let evidence = NativeEvidence::new(options.clone());
        evidence.note_catalog_search(Some(("selected", Some("claude"))));
        let mut work = lock(&evidence.work);
        assert!(matches!(
            evidence
                .prepare_catalog(&mut work, model::now_ms())
                .unwrap(),
            CatalogPreparation::Pending
        ));
        assert!(
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage)",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap(),
            "65 distinct peer-message dependencies must start a genuine semantic stage"
        );
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([path.clone()])
        );
        let mut published = false;
        for _ in 0..128 {
            evidence
                .advance_semantic_stage(&mut work, model::now_ms(), &mut || Ok(()))
                .unwrap();
            if !db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)", [&path], |row| row.get::<_, bool>(0)).unwrap() {
                published = true;
                assert!(lock(&evidence.catalog_search).tickets.is_empty());
                break;
            }
            assert_eq!(
                lock(&evidence.catalog_search).tickets.len(),
                1,
                "stage begin and indexing retain the original demand"
            );
        }
        assert!(published);
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path='/unrelated/first.jsonl')", [], |row| row.get::<_, bool>(0)).unwrap());
        drop(work);
        drop(evidence);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_survives_publication_fence_would_block() {
        let (root, options, input) = audit_fixture();
        let initial = NativeEvidence::new(options.clone());
        drain_catalog(&initial, &std::cell::Cell::new(0));
        drop(initial);
        append_audit_record(&options, &input);
        let path = input.full_path(&options).to_string_lossy().into_owned();
        let db = cold_search_connection(&options);
        db.execute_batch("UPDATE meta SET value='partial' WHERE namespace=semon_namespace() AND key='catalog_completeness'; UPDATE storage_source_journal SET backfill_state='draining' WHERE namespace=semon_namespace();").unwrap();
        cold_search_claim(&db, &path, 100);
        let generation: String = db
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let captured_options = options.clone();
        let changed = path.clone();
        let hook_revisions = std::rc::Rc::new(std::cell::Cell::new(None));
        let captured_revisions = hook_revisions.clone();
        crate::events::BEFORE_CATALOG_PUBLICATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let config = captured_options
                    .source
                    .config(&captured_options.cache)
                    .unwrap();
                let mut journal = config.source_observations().unwrap();
                let claims = journal.claims(std::slice::from_ref(&changed)).unwrap();
                let before = claims[0]
                    .1
                    .expect("the prepared source has a pending claim");
                let operation = crate::storage::source_observations::SourceOperation::capture(
                    config.namespace.as_str(),
                    "invalidate",
                    &serde_json::to_vec(&claims).unwrap(),
                );
                assert_eq!(
                    journal.invalidate(&operation, &claims).unwrap(),
                    crate::storage::source_observations::SourceMutationReceipt::Applied
                );
                let after = journal.claims(std::slice::from_ref(&changed)).unwrap()[0]
                    .1
                    .unwrap();
                assert!(
                    after > before,
                    "the hook must change the actual prepared claim"
                );
                captured_revisions.set(Some((before, after)));
            }));
        });
        let evidence = NativeEvidence::new(options.clone());
        evidence.note_catalog_search(Some(("audit", Some("codex"))));
        let mut work = lock(&evidence.work);
        let error = match evidence.prepare_catalog(&mut work, model::now_ms()) {
            Err(error) => error,
            Ok(_) => panic!("the changed claim must refuse publication"),
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        let (_, changed_revision) = hook_revisions.get().expect("publication hook must run");
        assert_eq!(
            db.query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [&path],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            changed_revision,
            "refused publication must leave the new claim pending"
        );
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            generation,
            "the changed claim must fence the Catalog generation"
        );
        assert_eq!(
            lock(&evidence.catalog_search).tickets[0].waiting,
            BTreeSet::from([path])
        );
        evidence
            .prepare_catalog(&mut work, model::now_ms())
            .unwrap();
        assert!(lock(&evidence.catalog_search).tickets.is_empty());
        drop(work);
        drop(evidence);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cold_catalog_search_interest_is_scoped_after_machine_authorization() {
        let (first_root, first_options, _) = audit_fixture();
        let (second_root, second_options, _) = audit_fixture();
        let first = NativeEvidence::new(first_options);
        let second = NativeEvidence::new(second_options);
        drain_catalog(&first, &std::cell::Cell::new(0));
        drain_catalog(&second, &std::cell::Cell::new(0));
        assert!(!lock(&first.catalog_search).active);
        assert!(!lock(&second.catalog_search).active);
        lock(&first.catalog_search).open();
        lock(&second.catalog_search).open();
        for evidence in [&first, &second] {
            let db = cold_search_connection(&evidence.options);
            db.execute_batch("UPDATE meta SET value='partial' WHERE namespace=semon_namespace() AND key='catalog_completeness'; UPDATE storage_source_journal SET backfill_state='draining' WHERE namespace=semon_namespace();").unwrap();
            let path = evidence
                .options
                .codex_home
                .join("sessions/2026/10/01/rollout-audit.jsonl");
            cold_search_claim(&db, path.to_string_lossy().as_ref(), 100);
        }
        // HTTP demand starts the real producer. Pin both Work fences while
        // inspecting admission so it cannot finish and acknowledge the claims.
        let first_work = lock(&first.work);
        let second_work = lock(&second.work);
        let sources = Arc::new(SessionSources::from_evidence(vec![
            ("first".into(), first.clone()),
            ("second".into(), second.clone()),
        ]));
        let core = crate::ViewerCore::from_sources(sources);
        assert_eq!(
            core.respond("GET", "/api/sessions", "machine=missing&q=audit", None)
                .status,
            404
        );
        assert!(lock(&first.catalog_search).tickets.is_empty());
        assert!(lock(&second.catalog_search).tickets.is_empty());
        assert_eq!(
            core.respond(
                "GET",
                "/api/sessions",
                "machine=second&q=audit&harness=codex",
                None
            )
            .status,
            200
        );
        assert!(lock(&first.catalog_search).tickets.is_empty());
        assert_eq!(
            lock(&second.catalog_search)
                .tickets
                .iter()
                .map(|ticket| (ticket.query.clone(), ticket.harness.clone()))
                .collect::<Vec<_>>(),
            vec![("audit".into(), Some("codex".into()))]
        );
        assert_eq!(
            core.respond("GET", "/api/sessions", "machine=second", None)
                .status,
            200
        );
        assert_eq!(
            lock(&second.catalog_search)
                .tickets
                .iter()
                .map(|ticket| (ticket.query.clone(), ticket.harness.clone()))
                .collect::<Vec<_>>(),
            vec![("audit".into(), Some("codex".into()))]
        );
        for evidence in [&first, &second] {
            let db = cold_search_connection(&evidence.options);
            let path = evidence
                .options
                .codex_home
                .join("sessions/2026/10/01/rollout-audit.jsonl");
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1)", [path.to_string_lossy().as_ref()], |row| row.get::<_, bool>(0)).unwrap());
        }
        drop(first_work);
        drop(second_work);
        core.close();
        fs::remove_dir_all(first_root).unwrap();
        fs::remove_dir_all(second_root).unwrap();
    }

    #[test]
    fn semantic_conflict_diagnostics_are_bounded_and_do_not_include_native_inputs() {
        let (root, options, input) = audit_fixture();
        let evidence = NativeEvidence::new(options.clone());
        let cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let mut batch = model::relation_stage::Batch {
            epoch: 7,
            phase: model::relation_stage::Phase::Indexing,
            paths: vec![input.full_path(&options).to_string_lossy().into_owned()],
            source_revision: 9,
            base_generation: Some("private-generation-value".into()),
            runtime: "private-runtime-value".into(),
            context: Default::default(),
            relation_cursor: Some("private-relation-cursor".into()),
        };
        for operation in ["record", "publish", "finish"] {
            if operation != "record" {
                batch.phase = model::relation_stage::Phase::Publication;
            }
            let error = evidence.semantic_write_conflict(
                &cache,
                &batch,
                operation,
                &crate::events::Outcome::Conflict,
            );
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            let message = error.to_string();
            assert!(message.len() < 384);
            assert!(message.contains(&format!("operation={operation}")));
            assert!(
                message
                    .contains("epoch=7 revision=9 sources=1 outcome=Conflict reason=unspecified")
            );
            assert!(!message.contains(root.to_string_lossy().as_ref()));
            assert!(!message.contains(&input.path));
            assert!(!message.contains("private-"));
        }
        evidence.retire();
        drop(cache);
        drop(evidence);
        fs::remove_dir_all(root).unwrap();
    }

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
    fn unsupported_semantic_recipe_with_active_subset_refuses_without_rebinding_history() {
        let (root, options, input) = audit_fixture();
        let unrelated = crate::Input {
            root: crate::InputRoot::Codex,
            path: "sessions/2026/10/01/rollout-unrelated.jsonl".into(),
        };
        fs::write(unrelated.full_path(&options), "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"payload\":{\"id\":\"unrelated\"}}\n").unwrap();
        let evidence = NativeEvidence::new(options.clone());
        drain_catalog(&evidence, &std::cell::Cell::new(0));
        // A completed publication advances the head. Start this separate
        // producer operation with a fresh admitted head, rather than asking
        // the old stage handle to seize a successor generation.
        evidence.retire();
        let evidence = NativeEvidence::new(options.clone());
        {
            let mut work = lock(&evidence.work);
            work.events = Some(EventCache::open_scoped(&options.cache, &options.source).unwrap());
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
        let config = options.source.config(&options.cache).unwrap();
        let db = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        let unrelated_key: String = db.query_row("SELECT session_key FROM session_catalog_sources WHERE native_id='unrelated' LIMIT 1", [], |row| row.get(0)).unwrap();
        assert_eq!(db.execute("UPDATE session_catalog SET metadata=json_set(metadata,'$.name','old derived metadata') WHERE session_key=?1", [&unrelated_key]).unwrap(), 1);
        db.execute(
            "UPDATE main.storage_semantic_profiles SET recipe=?1 WHERE namespace=semon_namespace()",
            ["unsupported-semantic-recipe"],
        )
        .unwrap();
        let before = crate::native_timing::cleanup_tests::snapshot(&db);
        let path = EventCache::path(&options.cache);
        let wal = path.with_extension("sqlite3-wal");
        let original = fs::read(&path).unwrap();
        let original_wal = fs::read(&wal).unwrap();
        for _ in 0..2 {
            let mut work = lock(&evidence.work);
            let error = evidence
                .refresh_catalog(&mut work, model::now_ms())
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&crate::storage::StorageError::NamespaceUpgradePending.to_string())
            );
            assert_eq!(
                work.events
                    .as_ref()
                    .unwrap()
                    .qualified_catalog()
                    .unwrap_err()
                    .to_string(),
                crate::storage::StorageError::NamespaceUpgradePending.to_string()
            );
            assert!(EventCache::open_scoped(&options.cache, &options.source).is_err());
            assert!(EventCache::open(&options.cache, &options.source).is_err());
            assert_eq!(
                crate::native_timing::cleanup_tests::snapshot(&db),
                before,
                "refusal preserves the active subset and unrelated published history"
            );
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::read(&wal).unwrap(), original_wal);
        }
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            crate::events::store::SCHEMA_VERSION
        );
        let name: String = db
            .query_row(
                "SELECT json_extract(metadata,'$.name') FROM session_catalog WHERE session_key=?1",
                [&unrelated_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            name, "old derived metadata",
            "unsupported publication must not rewrite unrelated metadata"
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
        drop(work);
        drop(sources);
        fs::remove_dir_all(root).unwrap();
    }
}

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
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

pub(crate) struct NativeEvidence {
    pub(crate) options: Options,
    pub(crate) work: Mutex<Work>,
    pub(crate) catalog_demand: AtomicBool,
    query: RwLock<Option<Arc<QueryCache>>>,
    closed: AtomicBool,
}

pub(crate) enum CatalogPreparation {
    Pending,
    Reconcile,
    Complete(Box<CompleteCatalog>),
}

pub(crate) struct CompleteCatalog {
    pub(crate) prepared: model::PreparedSources,
    pub(crate) snapshot: Snapshot,
    pub(crate) generation: String,
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
            machines: machines
                .into_iter()
                .map(|(key, options)| (key, NativeEvidence::new(options)))
                .collect(),
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
        view.note_catalog_read()?;
        Ok(true)
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    /// Notify an already demanded source producer after an authorized push.
    /// This never constructs a producer or starts compatibility warming.
    /// True means the source is configured and open, even before demand.
    pub fn invalidate(&self, machine: &str) -> bool {
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
            return true;
        };
        view.invalidate();
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
        })
    }

    /// Retired sources can only answer from their last verified cohort.
    /// The embedding host still waits out readers before removing source custody.
    pub(crate) fn retire(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Initialize the source index and publish a bounded cold catalog page.
    pub(crate) fn prepare_events(&self, work: &mut Work, now: i64) -> io::Result<()> {
        if work.events.is_none() {
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
        self.prepare_events(work, now)?;
        let cache = work.events.as_mut().expect("event cache initialized");
        if !cache.is_scoped() || cache.session_catalog_generation().is_some() {
            work.cold_catalog = None;
            return Ok(CatalogPreparation::Reconcile);
        }
        let discovery = work.cold_catalog.get_or_insert_with(ColdCatalog::default);
        let page = discovery.cursor.next(&self.options, 64, 512)?;
        debug_assert!(page.examined <= 512);
        for input in page.inputs {
            discovery.count += 1;
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
        if !page.complete {
            return Ok(CatalogPreparation::Pending);
        }
        let discovery = work.cold_catalog.take().expect("discovery initialized");
        let inputs: Vec<_> = discovery
            .newest
            .into_iter()
            .map(|(_, input)| input)
            .collect();
        let snapshot = (discovery.count <= 64)
            .then(|| Snapshot::capture_pids(&self.options, BTreeSet::new(), cache.paths()))
            .filter(|snapshot| {
                snapshot.catalog_inputs(&self.options) == inputs.iter().cloned().collect()
            });
        if !inputs.is_empty() {
            // Catalog owns physical membership independently of a Query scan
            // window; each consumer still applies its configured output window.
            let options = Options {
                scan_window: false,
                ..self.options.clone()
            };
            let prepared = model::prepare_sources(&options, cache, &inputs, now)?;
            let (outcome, generation) = cache.publish_partial_catalog_fenced(&prepared)?;
            if outcome != crate::events::Outcome::Written {
                return Err(io::Error::other(
                    "Initial native catalog publication changed; retry observation",
                ));
            }
            cache.end_scan();
            if let (Some(snapshot), Some(generation)) = (snapshot, generation) {
                return Ok(CatalogPreparation::Complete(Box::new(CompleteCatalog {
                    prepared,
                    snapshot,
                    generation,
                })));
            }
        }
        Ok(CatalogPreparation::Reconcile)
    }

    /// Expand and derive the same bounded component for Catalog refresh and
    /// ordinary selected local reads. Publication retains its existing fences.
    fn prepare_catalog_component(
        &self,
        cache: &mut EventCache,
        inputs: Vec<crate::inputs::Input>,
        now: i64,
    ) -> io::Result<Option<model::PreparedSources>> {
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
            let Some(component) =
                cache.catalog_component_paths(&selected, &keys, &native_parents, &self.options)?
            else {
                break None;
            };
            inputs.extend(component);
            if inputs.len() > 64 {
                break None;
            }
            let selected: Vec<_> = inputs.iter().cloned().collect();
            let prepared = model::prepare_sources(&options, cache, &selected, now)?;
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
            keys = dependencies.into_iter().collect();
            native_parents = prepared.native_parents();
            let Some(component) =
                cache.catalog_component_paths(&selected, &keys, &native_parents, &self.options)?
            else {
                break None;
            };
            if component.iter().all(|input| inputs.contains(input)) {
                break Some(prepared);
            }
            inputs.extend(component);
            if inputs.len() > 64 {
                break None;
            }
        };
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
        if let Some(snapshot) = &work.catalog_snapshot
            && !snapshot.changed(&self.options)
        {
            return Ok(true);
        }
        work.built_at = Some(Instant::now());
        if work.catalog_snapshot.is_some() {
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
                let prepared = self.prepare_catalog_component(cache, inputs, now)?;
                if let Some(prepared) = prepared {
                    match cache.publish_closed_catalog(&prepared)? {
                        Some(crate::events::Outcome::Written) => {
                            cache.publish_session_descriptions();
                            cache.end_scan();
                            work.catalog_snapshot = Some(snapshot);
                            return Ok(true);
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
            }
        }
        match self.prepare_catalog(work, now)? {
            CatalogPreparation::Pending => return Ok(false),
            CatalogPreparation::Complete(complete) => {
                before_build()?;
                let cache = work.events.as_mut().expect("event cache initialized");
                cache.publish_catalog_projection(
                    &crate::slot_projection::Publication {
                        rows: &complete.prepared.rows,
                        transcripts: Some(&complete.prepared.transcripts),
                    },
                    Some(&complete.generation),
                )?;
                cache.publish_session_descriptions();
                let mut snapshot = complete.snapshot;
                if self.options.facts.is_none() {
                    snapshot.pids = complete
                        .prepared
                        .pids
                        .iter()
                        .map(|pid| (*pid, proc_start(&self.options.proc_root, *pid)))
                        .collect();
                }
                work.catalog_snapshot = Some(snapshot);
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
        let pids =
            model::observe_catalog(&catalog_options, cache, &mut false, &mut work.texts, now)?;
        if self.options.facts.is_none() {
            snapshot.pids = pids
                .iter()
                .map(|pid| (*pid, proc_start(&self.options.proc_root, *pid)))
                .collect();
        }
        work.catalog_snapshot = Some(snapshot);
        Ok(true)
    }

    /// Native queries refresh this owner's source cache and coherent cohort.
    /// This owner has no compatibility transport or background refresh workers.
    pub(crate) fn catalog_pending(&self, work: &Work) -> bool {
        work.cold_catalog.is_some()
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
    cold_catalog: Option<ColdCatalog>,
    /// When the last native preparation or legacy model rebuild started.
    pub(crate) built_at: Option<Instant>,
}

#[derive(Default)]
struct ColdCatalog {
    cursor: crate::inputs::Discovery,
    count: usize,
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
    fn catalog_inputs(&self, options: &Options) -> BTreeSet<crate::inputs::Input> {
        self.watched
            .iter()
            .filter_map(|(path, stamp)| {
                stamp.filter(|stamp| stamp.kind.is_file())?;
                catalog_input(path, options)
            })
            .collect()
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

//! [`ViewerCore`]: the session viewer over one machine's homes or several.
//!
//! Configured machine keys define source authority. Focused Catalog, Transcript,
//! Entry, identity and relationship reads use the shared native Event Index.
//! Cross-machine handoffs stay scoped to their source; duplicate native session
//! IDs are refused for writable control ownership instead of choosing a machine.
//! The core owns configuration and refresh scheduling, never a full workspace
//! model. Native Query reads reuse the same evidence owner.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::{Arc, Condvar, Mutex, PoisonError, RwLock, Weak},
};

use serde_json::{Value, json};

use crate::{
    Options,
    model::{self, QueryData, Session, Transcript},
    received::{Listing, ReceivedMachines},
    refresh::RefreshPool,
    viewer::{
        MachineView, ViewerReply, decoded, is_named_page, lock, page_parts, percent_encode,
        query_value, read_lock, write_lock,
    },
};

/// A link to the embedding server's own machine-management page, shown on
/// the Machines page. The local viewer never sets one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminLink {
    label: String,
    href: String,
}

impl AdminLink {
    /// `href` must be a same-origin path (`/…`, not `//…`), at most 512
    /// bytes, with no backslash or control characters. `label` must be 1 to
    /// 80 characters with no control characters.
    pub fn new(label: &str, href: &str) -> Option<Self> {
        (valid_label(label) && valid_href(href)).then(|| Self {
            label: label.trim().to_owned(),
            href: href.to_owned(),
        })
    }
}

fn valid_text(text: &str, min: usize, max: usize) -> bool {
    let count = text.chars().count();
    count >= min && count <= max && !text.chars().any(char::is_control)
}

fn valid_label(label: &str) -> bool {
    !label.trim().is_empty() && valid_text(label, 1, 80)
}

fn valid_href(href: &str) -> bool {
    href.starts_with('/')
        && !href.starts_with("//")
        && !href.contains('\\')
        && href.len() <= 512
        && !href.chars().any(char::is_control)
}

/// One named workspace listed by an embedding server in an account menu.
/// Selecting it submits a POST to `switch_href`; the embedding server should
/// apply its same-origin check before changing the active workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountWorkspace {
    /// The name shown in the list.
    pub name: String,
    /// The role shown below the name.
    pub role: String,
    /// Whether this is the account's current workspace.
    pub current: bool,
    /// Same-origin form action submitted with POST when this item is selected.
    pub switch_href: String,
}

/// The HTTP method used by an account link.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkMethod {
    /// Navigate with a link. This is the default.
    #[default]
    Get,
    /// Submit a same-origin form to the link's action.
    Post,
}

impl LinkMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
        }
    }
}

/// A link listed at the end of an account menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountLink {
    /// The text shown for the link.
    pub label: String,
    /// Same-origin path used as the link destination or form action.
    pub href: String,
    /// Method used to select this link. Defaults to [`LinkMethod::Get`].
    pub method: LinkMethod,
    /// Whether the link is shown with the error color.
    pub danger: bool,
}

/// Account details an embedding server can add to the viewer's navigation.
/// Every path is same-origin and every list is validated as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMenu {
    name: String,
    login: String,
    initials: String,
    avatar_href: Option<String>,
    workspaces: Vec<AccountWorkspace>,
    links: Vec<AccountLink>,
}

impl AccountMenu {
    /// Creates a menu, or returns `None` if any value, path, or list count is
    /// invalid. The account name, workspace names, and link labels use the
    /// [`AdminLink`] label checks (1–80 characters). Login and workspace roles
    /// allow 0–80 characters; initials allow 1–3. Display text rejects
    /// controls. `avatar_href`, `switch_href`, and link `href` values use the
    /// [`AdminLink`] same-origin path checks. Workspaces always submit their
    /// `switch_href` with POST; account links default to GET and can use POST
    /// with [`LinkMethod::Post`]. The embedding server must enforce its
    /// same-origin check on POST actions. The menu accepts up to 50 workspaces
    /// and 12 links, and does not keep a partial list.
    pub fn new(
        name: &str,
        login: &str,
        initials: &str,
        avatar_href: Option<&str>,
        mut workspaces: Vec<AccountWorkspace>,
        mut links: Vec<AccountLink>,
    ) -> Option<Self> {
        let valid_workspace = |workspace: &AccountWorkspace| {
            valid_label(&workspace.name)
                && valid_text(&workspace.role, 0, 80)
                && valid_href(&workspace.switch_href)
        };
        let valid_link = |link: &AccountLink| valid_label(&link.label) && valid_href(&link.href);
        if !valid_label(name)
            || !valid_text(login, 0, 80)
            || !valid_text(initials, 1, 3)
            || initials.trim().is_empty()
            || avatar_href.is_some_and(|href| !valid_href(href))
            || workspaces.len() > 50
            || links.len() > 12
            || workspaces
                .iter()
                .any(|workspace| !valid_workspace(workspace))
            || links.iter().any(|link| !valid_link(link))
        {
            return None;
        }
        for workspace in &mut workspaces {
            workspace.name = workspace.name.trim().to_owned();
        }
        for link in &mut links {
            link.label = link.label.trim().to_owned();
        }
        Some(Self {
            name: name.trim().to_owned(),
            login: login.to_owned(),
            initials: initials.trim().to_owned(),
            avatar_href: avatar_href.map(str::to_owned),
            workspaces,
            links,
        })
    }
}

/// Refresh scheduling for a [`ViewerCore`]'s native evidence and tree reads.
///
/// Catalog demand always runs through the shared bounded producer, including
/// [`Refresh::OnRead`]. Catalog and selected reads use published generations;
/// they do not wait for an unrelated workspace preparation. Refresh failures
/// are reported as retryable refresh metadata while qualified data remains
/// available. Missing or invalid selected evidence has its own explicit error.
///
/// Background work shares a [`RefreshPool`](crate::RefreshPool), with at most
/// one queued check per machine. Changed observations are spaced by one second;
/// cold discovery continuations yield behind other due work. Reads keep checks
/// active for 30 seconds. A normal backlog cannot grow the pool; overflow is
/// bounded to twice its configured size when every worker has been stuck for
/// five seconds. Closing a core drops queued checks and waits for running work.
///
/// Explicit `/api/tree` reads retain an independent native tree snapshot. Their
/// first read may wait for collection; subsequent background reads serve the
/// last tree, then report sustained failures or stalls under the existing
/// refresh policy. Tree preparation is never required by focused session reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refresh {
    /// Refresh explicit tree reads synchronously. Native Catalog demand still
    /// uses the bounded asynchronous producer. The default for library callers.
    #[default]
    OnRead,
    /// Check demanded native sources every 250 ms while reads keep them active.
    /// Used by the local server when files can change without an invalidation.
    Background,
    /// Refresh after [`ViewerCore::invalidate`], with a 30-second safety check
    /// for active sources when a writer did not announce its change.
    OnInvalidate,
}

impl Refresh {
    /// The mode as a view stores it.
    pub(crate) fn code(self) -> u8 {
        match self {
            Refresh::OnRead => 0,
            Refresh::Background => 1,
            Refresh::OnInvalidate => 2,
        }
    }

    pub(crate) fn from_code(code: u8) -> Self {
        match code {
            1 => Refresh::Background,
            2 => Refresh::OnInvalidate,
            _ => Refresh::OnRead,
        }
    }
}

/// The session viewer without a transport: the pages, assets and API routes
/// `semon sessions --serve` answers, with shared native evidence and refresh
/// coordination, and no listener, token or Host check of its own.
/// It lets another server embed the viewer: that server owns the socket and
/// decides who may ask, then hands each request to [`ViewerCore::respond`]
/// and sends back the [`ViewerReply`] with its headers
/// ([`ViewerReply::headers`]: [`SECURITY_HEADERS`](crate::SECURITY_HEADERS),
/// or an attachment's image's own).
///
/// A core reads one machine's homes ([`ViewerCore::new`]) or several
/// ([`ViewerCore::with_machines`]). Calls can perform blocking selected-source
/// I/O, so async hosts invoke them off their runtime. Configure the core before
/// sharing it; `respond` takes `&self` and supports concurrent requests.
///
/// Catalog demand schedules native observation. Pages use indexed identity and
/// focused reads use source and projection generations. An embedding server
/// calls [`ViewerCore::invalidate`] after writing a machine's new source data.
/// Per-request account, navigation and management values are passed as [`Extras`]
/// to [`ViewerCore::respond_with`], independently of native evidence.
/// [`ViewerCore::close`] drains work before the host removes readable source data.
pub struct ViewerCore {
    views: RwLock<Arc<Views>>,
    source_inventory: Mutex<Option<(Weak<Views>, Arc<crate::SessionSourceInventory>)>>,
    received: Option<Received>,
    /// Chrome metadata [`ViewerCore::respond`] serves when a call
    /// doesn't pass its own ([`ViewerCore::respond_with`]).
    extras: Extras,
    refresh: Refresh,
    /// The pool that checks this core's machines in the background.
    pool: Arc<RefreshPool>,
    open: Gate,
    /// Views [`ViewerCore::follow`] stopped serving, which
    /// [`ViewerCore::close`] still waits out: a rebuild of one may run on.
    retired: Mutex<Vec<Weak<MachineView>>>,
}

/// Per-request embedding metadata served by `/api/viewer/chrome`: the Machines
/// management link, account menu and Machines navigation destination. Reading
/// or changing it never observes session sources or alters their generations.
/// Pass it to [`ViewerCore::respond_with`] on a shared core. Setters configure
/// the defaults used by [`ViewerCore::respond`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    admin: Option<AdminLink>,
    account: Option<AccountMenu>,
    nav_machines: Option<String>,
}

impl Extras {
    /// Sets, or clears, the link the Machines page shows to the embedding
    /// server's machine-management page, served as `admin`.
    pub fn set_admin_link(&mut self, link: Option<AdminLink>) {
        self.admin = link;
    }

    /// Sets, or clears, the account menu, served as `account`.
    pub fn set_account(&mut self, account: Option<AccountMenu>) {
        self.account = account;
    }

    /// Sets the destination for the Machines navigation entry, served as
    /// `nav`. `item` must be `"machines"`, and `href` must be a same-origin
    /// path. Returns false and leaves the current value unchanged for an
    /// unknown item or path.
    pub fn set_nav_override(&mut self, item: &str, href: &str) -> bool {
        if item != "machines" || !valid_href(href) {
            return false;
        }
        self.nav_machines = Some(href.to_owned());
        true
    }
}

/// The machines a core serves, each with its key, in order.
type Views = Vec<(String, Arc<MachineView>)>;

/// The received machines a core follows, served after its fixed ones.
struct Received {
    /// How many of the core's views, from the front, are its fixed machines.
    fixed: usize,
    following: Mutex<Following>,
}

/// What following the received machines keeps between requests.
struct Following {
    machines: ReceivedMachines,
    /// The hostnames of the fixed machines read from this one (no recorded
    /// facts), read at the first pass.
    hosts: Option<Vec<String>>,
    /// What the last pass saw.
    seen: Option<Listing>,
    /// Entries of `DIR/machines/` already warned about.
    warned: BTreeSet<String>,
}

/// Counts the calls in progress, so [`ViewerCore::close`] can wait them
/// out; its lock is held only to count.
#[derive(Default)]
struct Gate {
    /// Closed, and how many calls are in progress.
    state: Mutex<(bool, usize)>,
    idle: Condvar,
}

/// A call in progress, until dropped.
struct Entered<'a>(&'a Gate);

impl Gate {
    /// A call starts, unless the core is closed.
    fn enter(&self) -> Option<Entered<'_>> {
        let mut state = lock(&self.state);
        if state.0 {
            return None;
        }
        state.1 += 1;
        Some(Entered(self))
    }

    /// Whether [`Gate::close`] was called.
    fn is_closed(&self) -> bool {
        lock(&self.state).0
    }

    /// No call starts again; waits for those in progress.
    fn close(&self) {
        let mut state = lock(&self.state);
        state.0 = true;
        while state.1 > 0 {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.1 -= 1;
        if state.1 == 0 {
            self.0.idle.notify_all();
        }
    }
}

/// Which machine answers for each session id the union serves.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// Each machine's id in the union, in order: its hostname, or
    /// `<hostname>~<key>` when an earlier machine has that hostname.
    machine_ids: Vec<String>,
    /// Served session id → (machine, its id there).
    owners: BTreeMap<String, (usize, String)>,
    /// Ids two machines both claim: refused.
    conflicts: BTreeSet<String>,
    /// For each machine, its own session ids an earlier machine already
    /// serves: left out. Only a received machine's are (see
    /// [`Plan::droppable`]); between fixed machines a shared id is refused.
    dropped: Vec<BTreeSet<String>>,
    /// The first machine whose ids an earlier machine's win over: the first
    /// received machine. `usize::MAX` without received machines.
    droppable: usize,
}

/// Which machine answers for a session id.
pub(crate) enum Owner {
    /// This machine (by position), under this id there.
    At(usize, String),
    Missing,
    /// Two machines both claim it: refused, never picked.
    Conflict,
}
fn session_local(sessions: &BTreeMap<String, Session>, id: &str) -> bool {
    id.starts_with("unsent:") || sessions.get(id).is_some_and(|session| session.stub)
}

/// A machine's id in the union: its hostname, or, when an earlier machine
/// has that id, `<hostname>~<key>`, then `<hostname>~<index>`, then
/// `<hostname>~<index>~<n>` until no earlier machine has it.
fn machine_id(hostname: &str, key: &str, index: usize, taken: &[String]) -> String {
    let mut candidates = vec![hostname.to_owned()];
    if !key.is_empty() {
        candidates.push(format!("{hostname}~{key}"));
    }
    candidates.push(format!("{hostname}~{index}"));
    candidates
        .into_iter()
        .chain((2..).map(|n| format!("{hostname}~{index}~{n}")))
        .find(|id| !taken.contains(id))
        .expect("an unbounded list of distinct ids")
}

/// Presentation aliases depend only on configured native machine identity.
fn native_machine_ids(views: &Views) -> Vec<String> {
    let mut labels = Vec::with_capacity(views.len());
    for (index, (key, view)) in views.iter().enumerate() {
        let hostname = crate::facts::MachineFacts::of(view.options()).hostname(view.options());
        labels.push(machine_id(&hostname, key, index, &labels));
    }
    labels
}
/// Ownership and presentation identity are native semantics, shared by query
/// evidence and compatibility transport. A version hash belongs only to the latter.
struct IdentityPart<'a> {
    key: &'a str,
    machine: &'a str,
    sessions: &'a BTreeMap<String, Session>,
    transcripts: &'a BTreeMap<String, Arc<Transcript>>,
}

fn identity_plan(parts: &[IdentityPart<'_>], droppable: usize) -> Plan {
    let mut machine_ids: Vec<String> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let id = machine_id(part.machine, part.key, index, &machine_ids);
        machine_ids.push(id);
    }
    let mut owners: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut conflicts = BTreeSet::new();
    let mut dropped = vec![BTreeSet::new(); parts.len()];
    for (index, part) in parts.iter().enumerate() {
        let ids: BTreeSet<&String> = part
            .sessions
            .keys()
            .chain(part.transcripts.keys())
            .collect();
        for id in ids {
            let served = if session_local(part.sessions, id) {
                format!("{id}@{}", machine_ids[index])
            } else {
                id.clone()
            };
            if owners.contains_key(&served) && index >= droppable {
                dropped[index].insert(id.clone());
            } else if owners.insert(served.clone(), (index, id.clone())).is_some() {
                conflicts.insert(served);
            }
        }
    }
    Plan {
        machine_ids,
        owners,
        conflicts,
        dropped,
        droppable,
    }
}

/// One call's native query snapshots. Window, identity, summaries and transcript
/// offsets all belong to these exact source generations, rather than separate reads.
pub(crate) struct QueryRead {
    pub(crate) parts: Vec<QueryPart>,
    plan: Plan,
}

pub(crate) struct QueryPart {
    pub(crate) data: Arc<QueryData>,
    pub(crate) machine: Option<String>,
    pub(crate) dropped: BTreeSet<String>,
    pub(crate) droppable: bool,
}

impl QueryPart {
    pub(crate) fn served(&self, id: &str) -> String {
        match &self.machine {
            Some(machine) if session_local(&self.data.sessions, id) => format!("{id}@{machine}"),
            _ => id.to_owned(),
        }
    }
}

impl QueryRead {
    pub(crate) fn new(machines: Vec<(String, Arc<QueryData>)>, droppable: usize) -> Self {
        let identities: Vec<_> = machines
            .iter()
            .map(|(key, data)| IdentityPart {
                key,
                machine: &data.machine_id,
                sessions: &data.sessions,
                transcripts: &data.tx,
            })
            .collect();
        let plan = identity_plan(&identities, droppable);
        let multiple = machines.len() > 1;
        let parts = machines
            .into_iter()
            .enumerate()
            .map(|(index, (_, data))| QueryPart {
                data,
                machine: multiple.then(|| plan.machine_ids[index].clone()),
                dropped: plan.dropped[index].clone(),
                droppable: multiple && index >= plan.droppable,
            })
            .collect();
        Self { parts, plan }
    }

    pub(crate) fn owner(&self, id: &str) -> Owner {
        match self.parts.len() {
            0 => Owner::Missing,
            1 => Owner::At(0, id.to_owned()),
            _ if self.plan.conflicts.contains(id) => Owner::Conflict,
            _ => match self.plan.owners.get(id) {
                Some((index, own)) => Owner::At(*index, own.clone()),
                None => Owner::Missing,
            },
        }
    }

    pub(crate) fn conflicts(&self) -> BTreeSet<String> {
        if self.parts.len() > 1 {
            self.plan.conflicts.clone()
        } else {
            BTreeSet::new()
        }
    }
}
/// Sets `machine` on a V1 tree node and every node under it.
fn tag(node: &mut Value, machine: &str) {
    if let Some(fields) = node.as_object_mut() {
        fields.insert("machine".into(), Value::from(machine));
        if let Some(Value::Array(children)) = fields.get_mut("children") {
            for child in children {
                tag(child, machine);
            }
        }
    }
}

fn account_json(account: &AccountMenu) -> Value {
    json!({
        "name": account.name,
        "login": account.login,
        "initials": account.initials,
        "avatar_href": account.avatar_href,
        "workspaces": account.workspaces.iter().map(|workspace| json!({
            "name": workspace.name,
            "role": workspace.role,
            "current": workspace.current,
            "switch_href": workspace.switch_href,
        })).collect::<Vec<_>>(),
        "links": account.links.iter().map(|link| json!({
            "label": link.label,
            "href": link.href,
            "method": link.method.as_str(),
            "danger": link.danger,
        })).collect::<Vec<_>>(),
    })
}
fn text(status: u16, body: &str) -> ViewerReply {
    ViewerReply {
        status,
        content_type: "text/plain; charset=utf-8",
        body: body.as_bytes().to_vec(),
        etag: None,
    }
}

fn failed(error: &io::Error) -> ViewerReply {
    match error.kind() {
        io::ErrorKind::NotFound => text(404, "Not found"),
        io::ErrorKind::InvalidInput => text(400, "Invalid request"),
        _ => {
            eprintln!("semon sessions viewer: {error}");
            text(500, "Internal error")
        }
    }
}
impl ViewerCore {
    /// A core over one machine's agent homes, `/proc` and cache that
    /// `options` name. Nothing is read until the first request.
    pub fn new(options: Options) -> Self {
        Self::with_machines(vec![(String::new(), options)])
    }

    /// A core over several machines' homes, serving the union of their
    /// models. Each machine's `key` is the embedder's stable name for it,
    /// used only to tell apart two machines with the same hostname. The
    /// order is kept: the first machine is the model's `machine`. With one
    /// machine this is [`ViewerCore::new`], byte for byte.
    pub fn with_machines(machines: Vec<(String, Options)>) -> Self {
        Self::from_sources(Arc::new(crate::SessionSources::with_machines(machines)))
    }

    /// Attach the remaining Viewer consumers to admitted native sources. Their
    /// complete source index and query cohort stay with the existing owner.
    /// The host closes this Viewer before draining sources or removing custody.
    pub fn from_sources(sources: Arc<crate::SessionSources>) -> Self {
        let views = Arc::new(sources.machine_views());
        let core = Self {
            views: RwLock::new(views.clone()),
            source_inventory: Mutex::new(None),
            received: None,
            extras: Extras::default(),
            refresh: Refresh::OnRead,
            pool: RefreshPool::shared(),
            open: Gate::default(),
            retired: Mutex::default(),
        };
        if sources.is_retired() {
            core.close();
        }
        core
    }

    /// A core over `machines` (as [`ViewerCore::with_machines`], possibly
    /// none) and every machine under `received`'s directory, after them in
    /// name order. The received machines are followed: every request first
    /// takes one pass over `DIR/machines/` (file types and the facts file's
    /// stamp only), and a machine added or removed since is served, or no
    /// longer, from that request on. A received machine's key is its
    /// directory's name, so one with the same hostname as a fixed machine
    /// is served as `<hostname>~<name>`.
    pub fn with_received(machines: Vec<(String, Options)>, received: ReceivedMachines) -> Self {
        let fixed = machines.len();
        let mut core = Self::with_machines(machines);
        core.received = Some(Received {
            fixed,
            following: Mutex::new(Following {
                machines: received,
                hosts: None,
                seen: None,
                warned: BTreeSet::new(),
            }),
        });
        core
    }

    /// Route tests through the production native adapter while retaining the
    /// exact instrumented refresh worker and evidence owner under test.
    #[cfg(test)]
    pub(crate) fn with_test_view(view: Arc<MachineView>) -> Self {
        let core = Self::with_machines(Vec::new());
        *write_lock(&core.views) = Arc::new(vec![(String::new(), view)]);
        core
    }

    /// The machines served now.
    fn views(&self) -> Arc<Views> {
        read_lock(&self.views).clone()
    }

    /// Build configured source metadata once per views snapshot. No native
    /// model, facts, source bodies or health checks participate in this index.
    fn source_inventory(&self, views: &Arc<Views>) -> Arc<crate::SessionSourceInventory> {
        let mut cached = lock(&self.source_inventory);
        if let Some((snapshot, inventory)) = cached.as_ref()
            && snapshot
                .upgrade()
                .is_some_and(|old| Arc::ptr_eq(&old, views))
        {
            return inventory.clone();
        }
        let inventory = Self::configured_source_inventory(views);
        *cached = Some((Arc::downgrade(views), inventory.clone()));
        inventory
    }

    fn configured_source_inventory(views: &Views) -> Arc<crate::SessionSourceInventory> {
        let labels = native_machine_ids(views);
        Arc::new(crate::SessionSourceInventory::new(
            views.iter().enumerate().map(|(index, (key, _))| {
                let key = if key.is_empty() {
                    "local".to_owned()
                } else {
                    key.clone()
                };
                let label = labels[index].clone();
                (key, label)
            }),
        ))
    }

    /// Each machine's view, in order: for tests that read its hooks.
    #[cfg(test)]
    pub(crate) fn machine_views(&self) -> Vec<Arc<MachineView>> {
        self.views().iter().map(|(_, view)| view.clone()).collect()
    }

    /// A view of a machine, in this core's refresh mode.
    fn view(&self, options: Options) -> Arc<MachineView> {
        let view = MachineView::new(options);
        view.set_pool(self.pool.clone());
        view.set_refresh(self.refresh);
        view
    }

    /// Brings the served machines in line with the received ones, when
    /// this core follows any: one pass over `DIR/machines/`, and changes
    /// only where it differs from the last. One request follows at a time.
    fn follow(&self) {
        let Some(received) = &self.received else {
            return;
        };
        let mut following = lock(&received.following);
        let listing = following.machines.scan();
        if following.seen.as_ref() == Some(&listing) {
            return;
        }
        let previous = following.seen.take().unwrap_or_default();
        let fixed = received.fixed;
        let views = self.views();
        let Following {
            machines,
            hosts,
            warned,
            ..
        } = &mut *following;
        let hosts = hosts.get_or_insert_with(|| {
            views[..fixed]
                .iter()
                .filter(|(_, view)| view.options().facts.is_none())
                .map(|(_, view)| model::local_hostname(view.options()))
                .collect()
        });
        for (name, seen) in &listing {
            let before = previous.get(name);
            if before == Some(seen) {
                continue;
            }
            let Some(seen) = seen else {
                if warned.insert(name.clone()) {
                    eprintln!(
                        "semon: {name} in {}: not a machine directory (a directory named with \
                         a-z, 0-9 and -, 1 to 63 of them); ignored",
                        machines.dir().join("machines").display()
                    );
                }
                continue;
            };
            let before = before.cloned().flatten();
            for (home, readable, was) in [
                (
                    "claude",
                    seen.claude,
                    before.as_ref().map(|seen| seen.claude),
                ),
                ("codex", seen.codex, before.as_ref().map(|seen| seen.codex)),
                (
                    "copilot",
                    seen.copilot,
                    before.as_ref().map(|seen| seen.copilot),
                ),
            ] {
                if !readable && was != Some(false) {
                    eprintln!(
                        "semon: received machine {name}: its {home} home is behind a \
                         symbolic link; ignored"
                    );
                }
            }
            if before.is_none_or(|before| before.facts != seen.facts || before.stale != seen.stale)
            {
                machines.copy_facts(name, seen, hosts, warned);
            }
        }
        let mut old: BTreeMap<String, Arc<MachineView>> = views[fixed..].iter().cloned().collect();
        let mut next: Views = views[..fixed].to_vec();
        let mut gone = Vec::new();
        for (name, seen) in &listing {
            let Some(seen) = seen else {
                continue;
            };
            let options = machines.options(name, seen);
            let view = match old.remove(name) {
                Some(view)
                    if view.options().claude_home == options.claude_home
                        && view.options().codex_home == options.codex_home
                        && view.options().copilot_home == options.copilot_home =>
                {
                    view
                }
                replaced => {
                    gone.extend(replaced);
                    self.view(options)
                }
            };
            next.push((name.clone(), view));
        }
        gone.extend(old.into_values());
        *write_lock(&self.views) = Arc::new(next);
        following.seen = Some(listing);
        // A machine no longer served stops refreshing; a rebuild of it may
        // still run, and `close` waits for it.
        let mut retired = lock(&self.retired);
        retired.retain(|view| view.strong_count() > 0);
        for view in gone {
            view.retire();
            retired.push(Arc::downgrade(&view));
        }
    }

    /// How many machines this core serves.
    pub fn machines(&self) -> usize {
        self.views().len()
    }

    /// Sets when the core refreshes its native sources and tree: see
    /// [`Refresh`]. [`Refresh::OnRead`] unless set.
    pub fn set_refresh(&mut self, refresh: Refresh) {
        self.refresh = refresh;
        for (_, view) in self.views().iter() {
            view.set_refresh(refresh);
        }
    }

    /// Sets the pool whose threads check this core's machines in the
    /// background: [`RefreshPool::shared`] unless set. Give every core of
    /// a process the same pool to bound its refresh threads to that pool's
    /// size; the shared one does that already.
    pub fn set_refresh_pool(&mut self, pool: Arc<RefreshPool>) {
        for (_, view) in self.views().iter() {
            view.set_pool(pool.clone());
        }
        self.pool = pool;
    }

    /// Tells the core that the logs of the machine served under `machine`
    /// changed: the key given to [`ViewerCore::with_machines`], a received
    /// machine's directory name, or `""` for [`ViewerCore::new`]'s one.
    /// Call it once the new data is written where the machine's
    /// [`Options`] read it. In [`Refresh::OnInvalidate`] that machine is
    /// then rebuilt in the background, as soon as its one-second spacing
    /// allows, however many calls come in that second; in
    /// [`Refresh::Background`] it is checked sooner than its next 250 ms
    /// check; refreshed on read, nothing needs it. It never blocks on a
    /// build or touches a file, so it can be called from an async task.
    /// Returns whether a machine is served under that key: false once the
    /// core is closed, and for a received machine that is new since the
    /// last request, which is read in full when it is.
    pub fn invalidate(&self, machine: &str) -> bool {
        if self.open.is_closed() {
            return false;
        }
        let views = self.views();
        let Some((_, view)) = views.iter().find(|(key, _)| key == machine) else {
            return false;
        };
        view.invalidate();
        true
    }

    /// Reuse this core's asynchronous producer for an exact configured source.
    /// This uses the published configuration index; it performs no received
    /// directory discovery, model construction or native body reads. Hosts
    /// authorize source custody first and call invalidate() after a push.
    /// False means absent/ambiguous or closed. Every refresh mode uses the
    /// shared native producer; no compatibility warm call is required.
    pub fn demand_catalog_source(&self, source_key: &str) -> io::Result<bool> {
        let Some(_entered) = self.open.enter() else {
            return Ok(false);
        };
        let views = self.views();
        let inventory = self.source_inventory(&views);
        if !inventory.valid() {
            return Ok(false);
        }
        let Some(index) = inventory.index(source_key) else {
            return Ok(false);
        };
        views[index].1.note_catalog_read()?;
        Ok(true)
    }

    /// Stops the core for good, and returns once it no longer reads or
    /// writes any file: calls in progress and each machine's background
    /// rebuild have finished, and every later call answers 404 without
    /// touching a file. An embedding server calls it before it removes
    /// the files the core reads (its caches included), since a background
    /// rebuild outlives the request that started it. It blocks for as long
    /// as the slowest of those takes: call it off an async runtime.
    pub fn close(&self) {
        self.open.close();
        for (_, view) in self.views().iter() {
            view.close();
        }
        // A view still alive after it stopped being served is alive
        // because its last rebuild runs: wait for that too.
        let retired: Vec<_> = lock(&self.retired)
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for view in retired {
            view.close();
        }
    }

    /// Sets, or clears, the link the Machines page shows to the embedding
    /// server's machine-management page, served in native chrome as `admin` by
    /// [`ViewerCore::respond`]. A value that differs per request goes in
    /// the [`Extras`] passed to [`ViewerCore::respond_with`] instead.
    pub fn set_admin_link(&mut self, link: Option<AdminLink>) {
        self.extras.set_admin_link(link);
    }

    /// Sets, or clears, the account menu served in `/api/viewer/chrome` as `account`
    /// by [`ViewerCore::respond`]. It is not included in the model version
    /// or its `ETag`. A menu that differs per request goes in the
    /// [`Extras`] passed to [`ViewerCore::respond_with`] instead.
    pub fn set_account(&mut self, account: Option<AccountMenu>) {
        self.extras.set_account(account);
    }

    /// Sets the destination for the Machines navigation entry that
    /// [`ViewerCore::respond`] serves; see [`Extras::set_nav_override`].
    pub fn set_nav_override(&mut self, item: &str, href: &str) -> bool {
        self.extras.set_nav_override(item, href)
    }

    /// The [`Extras`] [`ViewerCore::respond`] serves: those its setters set.
    pub fn extras(&self) -> &Extras {
        &self.extras
    }

    /// Answers one request: `path` and `query` split at the `?`, still
    /// percent-encoded, and the request's `If-None-Match`. Only GET is
    /// answered (405 otherwise); every URL the viewer uses is a GET. The
    /// caller authenticates first: the core serves whoever it is handed.
    /// Any number of threads may call it at once. Embedding metadata uses the
    /// defaults configured by the setters; native session APIs authorize and
    /// fence their selected source and generation independently.
    pub fn respond(
        &self,
        method: &str,
        path: &str,
        query: &str,
        if_none_match: Option<&str>,
    ) -> ViewerReply {
        self.respond_with(&self.extras, method, path, query, if_none_match)
    }

    /// [`ViewerCore::respond`], with this request's own [`Extras`] in the
    /// native chrome instead of the core's.
    pub fn respond_with(
        &self,
        extras: &Extras,
        method: &str,
        path: &str,
        query: &str,
        if_none_match: Option<&str>,
    ) -> ViewerReply {
        if method != "GET" {
            return text(405, "Method not allowed");
        }
        let Some(_entered) = self.open.enter() else {
            return text(404, "Not found");
        };
        if path == "/api/viewer/chrome" {
            // Standalone embedding metadata is independent of source/model
            // observation. Hosted consumers provide their authenticated host.
            return ViewerReply {
                status: 200,
                content_type: "application/json; charset=utf-8",
                body: json!({
                    "api": 1,
                    "account": extras.account.as_ref().map(account_json),
                    "admin": extras.admin.as_ref().map(|link| json!({"label": link.label, "href": link.href})),
                    "navigation": {"paths": {"machines": extras.nav_machines.as_deref().unwrap_or("/machines")}}
                }).to_string().into_bytes(),
                etag: None,
            };
        }
        if crate::shell::is_page_path(path) && query_value(query, "native") == Some("1") {
            return crate::native_page::reply(path, query, |endpoint, args| {
                self.respond_with(extras, "GET", endpoint, args, None)
            });
        }
        self.follow();
        let views = self.views();
        if path == "/api/session-sources" {
            return self.source_inventory(&views).page(query);
        }
        if matches!(path, "/api/viewer/home" | "/api/viewer/analytics") {
            let selected_source = match crate::overview_source_key(query) {
                Ok(source) => source,
                Err(error) => return failed(&error),
            };
            if selected_source.as_ref().is_some_and(|selected| {
                !views
                    .iter()
                    .any(|(key, _)| (if key.is_empty() { "local" } else { key }) == selected)
            }) {
                return text(404, "Not found");
            }
            let labels = native_machine_ids(&views);
            let sources: Vec<_> = views
                .iter()
                .enumerate()
                .filter(|(_, (key, _))| {
                    selected_source.as_ref().is_none_or(|selected| {
                        (if key.is_empty() { "local" } else { key }) == selected
                    })
                })
                .map(|(index, (key, view))| {
                    let _ = view.note_catalog_read();
                    let key = if key.is_empty() { "local" } else { key };
                    {
                        let facts = view
                            .options()
                            .facts
                            .as_ref()
                            .and_then(|path| crate::read_facts(path).ok());
                        crate::OverviewSource {
                            key: key.into(),
                            label: labels[index].clone(),
                            options: view.options().clone(),
                            up: facts
                                .as_ref()
                                .is_none_or(|facts| facts.offline_since.is_none()),
                        }
                    }
                })
                .collect();
            return crate::session_overview_reply(
                &sources,
                if path.ends_with("analytics") {
                    "analytics"
                } else {
                    "home"
                },
                query,
                crate::model::now_ms(),
            )
            .unwrap_or_else(|error| failed(&error));
        }
        if matches!(
            path,
            "/api/sessions"
                | "/api/session-locate"
                | "/api/session-transcript"
                | "/api/session-entry"
                | "/api/session-capabilities"
                | "/api/session-identity"
                | "/api/session-anchor"
                | "/api/session-annotations"
                | "/api/session-trace"
                | "/api/session-home"
                | "/api/session-observe"
                | "/api/session-resolve"
        ) {
            return self.catalog(&views, path, query);
        }
        if views.is_empty() {
            return text(404, "Not found");
        }
        if let Some(reply) = self.page_at_once(&views, path) {
            return reply;
        }
        if path == "/api/tree" {
            return self.tree(&views).unwrap_or_else(|error| failed(&error));
        }
        views[0].1.respond(method, path, query, if_none_match)
    }

    /// Focused lists are scoped by the embedder's stable machine key. A
    /// hostname is a label, and never selects a source access boundary.
    fn catalog(&self, views: &Arc<Views>, path: &str, query: &str) -> ViewerReply {
        let request = if path == "/api/sessions" {
            match crate::catalog::Request::parse(query) {
                Ok(request) => Some(request),
                Err(reply) => return reply,
            }
        } else {
            None
        };
        // The original single-machine Viewer uses an empty internal key.
        // Its additive catalog contract has a stable explicit local alias.
        // This never derives source authority from hostnames or session ids.
        let local_alias = views.len() == 1 && views[0].0.is_empty();
        let machine = match query_value(query, "machine") {
            Some(value) => match decoded(value) {
                Some(machine) => machine,
                None => {
                    return crate::catalog::error(
                        400,
                        "invalid_arguments",
                        "The machine key must be valid UTF-8.",
                        false,
                    );
                }
            },
            None if local_alias => "local".to_owned(),
            None if views.len() == 1 => views[0].0.clone(),
            None => {
                return crate::catalog::error(
                    400,
                    "machine_scope_required",
                    "Select a stable machine key for this focused session list.",
                    false,
                );
            }
        };
        let inventory = self.source_inventory(views);
        let matches: Vec<_> = if inventory.valid() {
            inventory
                .index(&machine)
                .map(|index| &views[index])
                .into_iter()
                .collect()
        } else {
            // Invalid configurations retain the legacy explicit ambiguity
            // refusal. Valid configured inventories use indexed lookup.
            views.iter().filter(|(key, _)| *key == machine).collect()
        };
        let [(key, view)] = matches.as_slice() else {
            return crate::catalog::error(
                if matches.is_empty() { 404 } else { 409 },
                "machine_scope_unavailable",
                "The selected machine key is missing or ambiguous.",
                false,
            );
        };
        let key = if key.is_empty() {
            "local"
        } else {
            key.as_str()
        };
        // Record demand without waiting for an initial model build. The
        // shared native coordinator performs coherent observation in every
        // refresh mode, including default library callers.
        let refresh_error =
            (path != "/api/session-capabilities").then(|| view.note_catalog_read().err());
        let mut reply = match path {
            "/api/session-locate" => crate::session_bookmark(view.options(), key, query),
            "/api/session-observe" => crate::session_overview_observe(view.options(), key, query),
            "/api/session-home" => crate::session_home_details_with_mode(
                view.options(),
                key,
                query,
                None,
                crate::SessionSourceReadMode::ProviderOnly,
            ),
            "/api/session-anchor" => crate::session_turn_anchor(view.options(), key, query),
            "/api/session-annotations" => crate::session_annotations(view.options(), key, query),
            "/api/session-trace" => crate::session_causal_trace(view.options(), key, query),
            "/api/session-resolve" => crate::native_resolution::page(view.options(), key, query),
            "/api/session-identity" => {
                let sid = query_value(query, "sid").and_then(decoded);
                match sid
                    .as_deref()
                    .map(|sid| crate::session_catalog_identity(view.options(), key, sid))
                {
                    Some(Ok(Some(identity))) => ViewerReply {
                        status: 200,
                        content_type: "application/json; charset=utf-8",
                        body: json!({"api":1,"identity":identity})
                            .to_string()
                            .into_bytes(),
                        etag: None,
                    },
                    Some(Ok(None)) => crate::catalog::error(
                        404,
                        "session_not_found",
                        "The selected session is absent from this catalog.",
                        false,
                    ),
                    Some(Err(crate::CatalogIdentityError::ScopeChanged)) => crate::catalog::error(
                        503,
                        "catalog_scope_changed",
                        "The configured native source authority changed.",
                        true,
                    ),
                    Some(Err(crate::CatalogIdentityError::Unavailable)) => crate::catalog::error(
                        503,
                        "catalog_unavailable",
                        "The selected identity observation is unavailable.",
                        true,
                    ),
                    _ => crate::catalog::error(
                        400,
                        "invalid_arguments",
                        "A canonical sid is required.",
                        false,
                    ),
                }
            }
            "/api/session-transcript" => {
                crate::session_transcript_range(view.options(), key, query, None)
            }
            "/api/session-entry" => crate::session_entry_field(view.options(), key, query, None),
            "/api/session-capabilities" => crate::session_catalog_capabilities(
                key,
                crate::SessionReadEndpoints {
                    selected_identity: true,
                    native_resolution: true,
                    selected_transcript: true,
                    selected_entry: true,
                },
            ),
            _ => crate::catalog::page(
                view.options(),
                key,
                request.expect("catalog route parsed request"),
            ),
        };
        if let Ok(mut body) = serde_json::from_slice::<Value>(&reply.body) {
            if path == "/api/session-capabilities" {
                body["source_candidates"] = json!(views.len() > 1);
            }
            if let Some(refresh_error) = refresh_error {
                body["refresh"] = match refresh_error {
                    Some(_) => json!({"state":"unavailable","retryable":true}),
                    None => json!({"state":"updating"}),
                };
            }
            reply.body = body.to_string().into_bytes();
        }
        reply
    }
    /// The first view whose ids an earlier view's win over: the first
    /// received machine's.
    fn droppable(&self) -> usize {
        self.received
            .as_ref()
            .map_or(usize::MAX, |received| received.fixed)
    }
    /// The caller-supplied machine key that owns a served session. Hostnames
    /// are presentation labels; embedders use this key to bind native controls.
    /// Missing or ambiguous native IDs never select a writable destination.
    pub fn session_machine_key(&self, id: &str) -> io::Result<Option<String>> {
        Ok(self.session_view_identity(id)?.map(|(source, _)| source))
    }

    /// Authoritative source key and projected presentation machine for a served session.
    /// Ambiguous native identities never select a source.
    pub fn session_view_identity(&self, id: &str) -> io::Result<Option<(String, String)>> {
        let Some(_entered) = self.open.enter() else {
            return Ok(None);
        };
        self.follow();
        let views = self.views();
        let labels = native_machine_ids(&views);
        let lookup = |view: &MachineView,
                      source: &str,
                      id: &str|
         -> Result<
            Option<crate::CatalogSessionIdentity>,
            crate::CatalogIdentityError,
        > {
            let mut selected =
                crate::catalog::qualified_current_identity(view.options(), source, id)?;
            if selected.is_none() {
                for harness in ["claude", "codex", "copilot"] {
                    let resolved = crate::session_catalog_resolve_native(
                        view.options(),
                        source,
                        harness,
                        id,
                        crate::CatalogReadScope::Current,
                    )?;
                    match resolved.state {
                        crate::CatalogNativeResolutionState::Pending => continue,
                        crate::CatalogNativeResolutionState::Resolved => {}
                        _ => return Err(crate::CatalogIdentityError::Unavailable),
                    }
                    let key = resolved
                        .catalog_key
                        .as_deref()
                        .ok_or(crate::CatalogIdentityError::Unavailable)?;
                    let identity =
                        crate::catalog::qualified_current_identity(view.options(), source, key)?
                            .ok_or(crate::CatalogIdentityError::Unavailable)?;
                    if identity.generation != resolved.generation.as_deref().unwrap_or("")
                        || identity.native_id.as_deref() != Some(id)
                        || selected
                            .as_ref()
                            .is_some_and(|prior| prior.catalog_key != identity.catalog_key)
                    {
                        return Err(crate::CatalogIdentityError::Unavailable);
                    }
                    selected = Some(identity);
                }
            }
            if let Some(identity) = &selected {
                if identity.native_id.is_none()
                    || view
                        .evidence
                        .local_query_target(&identity.catalog_key, model::now_ms())
                        .ok()
                        .flatten()
                        .is_none()
                {
                    return Err(crate::CatalogIdentityError::Unavailable);
                }
            }
            Ok(selected)
        };
        let mut selected = None;
        for (index, (key, view)) in views.iter().enumerate() {
            if view.catalog_observation_pending() {
                return Ok(None);
            }
            let source = if key.is_empty() { "local" } else { key };
            match lookup(view, source, id) {
                Ok(Some(_)) => {
                    // Received copies give way to an earlier qualified owner.
                    if selected.is_some() {
                        if index < self.droppable() {
                            return Ok(None);
                        }
                    } else {
                        selected = Some((key.clone(), labels[index].clone()));
                    }
                }
                Ok(None) => {}
                Err(_) => return Ok(None),
            }
        }
        // Literal native IDs win before interpreting a presentation alias.
        if selected.is_none()
            && let Some((own, label)) = id.rsplit_once('@')
        {
            for (index, (key, view)) in views.iter().enumerate() {
                if labels[index] != label {
                    continue;
                }
                let source = if key.is_empty() { "local" } else { key };
                match lookup(view, source, own) {
                    Ok(Some(_)) if selected.is_none() => {
                        selected = Some((key.clone(), label.to_owned()))
                    }
                    Ok(Some(_)) | Err(_) => return Ok(None),
                    Ok(None) => {}
                }
            }
        }
        // A publication/audit that starts during lookup cannot lend ownership.
        if views
            .iter()
            .any(|(_, view)| view.catalog_observation_pending())
        {
            return Ok(None);
        }
        Ok(selected)
    }
    /// The V1 tree: every machine's roots, each node with its machine's
    /// id as `machine`.
    fn tree(&self, views: &Views) -> io::Result<ViewerReply> {
        // Tree identity depends on configured machine names and keys, not a
        // full Viewer model or its transcript/analytics ownership projection.
        let machine_ids = native_machine_ids(views);
        let droppable = self.droppable();
        let mut roots = Vec::new();
        let mut seen = BTreeSet::new();
        for (index, (_, view)) in views.iter().enumerate() {
            let machine = machine_ids.get(index).cloned().unwrap_or_default();
            for root in view.tree_roots()? {
                // A received machine's copy of a root an earlier machine
                // has is left out using the existing received-copy policy.
                if !seen.insert((root.harness.clone(), root.id.clone())) && index >= droppable {
                    continue;
                }
                let mut root = serde_json::to_value(&root).map_err(io::Error::other)?;
                tag(&mut root, &machine);
                roots.push(root);
            }
        }
        let body = serde_json::to_string_pretty(&json!({"schema_version": 1, "roots": roots}))
            .map_err(io::Error::other)?;
        Ok(ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: body.into_bytes(),
            etag: None,
        })
    }

    /// Native page shells use indexed identity and turn addresses only. A
    /// missing/partial publication is resolved by the focused client APIs;
    /// opening the shell never waits for native bodies or a workspace model.
    fn page_at_once(&self, views: &Views, path: &str) -> Option<ViewerReply> {
        if !is_named_page(path) {
            return None;
        }
        let Some(parts) = page_parts(path) else {
            return Some(text(400, "Invalid request"));
        };
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let shell = || views[0].1.respond("GET", "/", "", None);
        let labels = native_machine_ids(views);
        let (harness, id, turn) = match parts.as_slice() {
            ["machines", id] => {
                return Some(
                    if labels.iter().any(|label| label == id)
                        || views
                            .iter()
                            .any(|(key, _)| key == id || (key.is_empty() && *id == "local"))
                    {
                        shell()
                    } else {
                        text(404, "Not found")
                    },
                );
            }
            ["s", harness, id] => (*harness, *id, None),
            ["trace", harness, id, turn] => (*harness, *id, Some(*turn)),
            _ => return Some(text(404, "Not found")),
        };
        if !matches!(harness, "claude" | "codex" | "copilot")
            || id.is_empty()
            || id.len() > 4096
            || id.chars().any(char::is_control)
            || turn.is_some_and(|turn| {
                turn.is_empty() || turn.len() > 4096 || turn.chars().any(char::is_control)
            })
        {
            return Some(text(404, "Not found"));
        }
        // A literal native id wins before interpreting a multi-machine alias.
        let aliases = id.rsplit_once('@').filter(|(_, label)| {
            views.len() > 1 && labels.iter().any(|candidate| candidate == label)
        });
        for (own, selected_label) in
            std::iter::once((id, None)).chain(aliases.map(|(own, label)| (own, Some(label))))
        {
            let mut found = Vec::new();
            let mut unavailable = false;
            for (index, (key, view)) in views.iter().enumerate() {
                let source_key = if key.is_empty() && views.len() == 1 {
                    "local"
                } else {
                    key
                };
                let query = format!("sid={}", percent_encode(own));
                // A publication in progress can contain a newly discovered id.
                // Serve the shell while its focused APIs wait for that index.
                unavailable |= view.evidence.work.try_lock().is_err();
                let reply = crate::session_bookmark(view.options(), source_key, &query);
                let body = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                unavailable |= reply.status >= 500
                    || body["unavailable"].as_u64().is_some_and(|count| count > 0);
                let matches = body["matches"].as_array().into_iter().flatten();
                let mut matched = false;
                for item in matches {
                    if item["harness"] != harness {
                        continue;
                    }
                    if let Some(turn) = turn {
                        let Some(catalog_key) = item["catalog_key"].as_str() else {
                            continue;
                        };
                        let anchor = crate::session_turn_anchor(
                            view.options(),
                            source_key,
                            &format!(
                                "sid={}&turn={}&scope=retained_history",
                                percent_encode(catalog_key),
                                percent_encode(turn)
                            ),
                        );
                        let value =
                            serde_json::from_slice::<Value>(&anchor.body).unwrap_or(Value::Null);
                        unavailable |= anchor.status >= 500 || anchor.status == 409;
                        matched |= anchor.status == 200 && value["slot"].is_number();
                    } else {
                        matched = true;
                    }
                }
                // Far-end handoff stubs have indexed owners but no native file.
                if !matched
                    && turn.is_none()
                    && harness == "claude"
                    && body["matches"].as_array().is_some_and(Vec::is_empty)
                {
                    matched = crate::session_query_sources(view.options(), own).is_ok();
                }
                if matched && (index < self.droppable() || found.is_empty()) {
                    found.push(index);
                }
            }
            let candidates: Vec<_> = found
                .into_iter()
                .filter(|index| selected_label.is_none_or(|label| labels[*index] == label))
                .collect();
            if candidates.len() == 1 {
                return Some(shell());
            }
            if candidates.len() > 1 {
                return Some(text(404, "Not found"));
            }
            if selected_label.is_some() || aliases.is_none() {
                return Some(if unavailable {
                    shell()
                } else {
                    text(404, "Not found")
                });
            }
        }
        Some(text(404, "Not found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ownership_tracks_configuration_rewrites_and_removal() {
        let root = std::env::temp_dir().join(format!(
            "semon-ownership-{}-{}",
            std::process::id(),
            model::now_ms()
        ));
        let options = |key: &str| {
            let home = root.join(key);
            let proc_root = home.join("proc");
            std::fs::create_dir_all(&proc_root).unwrap();
            std::fs::write(proc_root.join("locks"), "").unwrap();
            Options {
                claude_home: home.join("claude"),
                claude_json: home.join(".claude.json"),
                codex_home: home.join("codex"),
                copilot_home: home.join("copilot"),
                proc_root,
                cache: home.join("cache"),
                all: true,
                since: std::time::Duration::from_secs(86400),
                session: None,
                facts: None,
                scan_window: false,
            }
        };
        let first = options("first");
        let second = options("second");
        let write = |options: &Options, name: &str| {
            let projects = options.claude_home.join("projects/project");
            std::fs::create_dir_all(&projects).unwrap();
            std::fs::write(
                projects.join(format!("{name}.jsonl")),
                format!(
                    "{}\n",
                    json!({
                        "type":"user","sessionId":name,"uuid":name,"parentUuid":null,
                        "cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z",
                        "message":{"role":"user","content":"synthetic prompt"}
                    })
                ),
            )
            .unwrap();
        };
        write(&first, "shared");
        write(&second, "shared");
        let mut core = ViewerCore::with_machines(vec![
            ("first".into(), first),
            ("second".into(), second.clone()),
        ]);
        core.set_refresh(Refresh::OnInvalidate);
        let builds = model::COMPAT_BUILDS.with(std::cell::Cell::get);
        let native_tree = core.respond("GET", "/api/tree", "", None);
        assert_eq!(native_tree.status, 200);
        let native_tree: Value = serde_json::from_slice(&native_tree.body).unwrap();
        let roots = native_tree["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|root| root["id"] == "shared"));
        assert_ne!(roots[0]["machine"], roots[1]["machine"]);
        assert_eq!(core.session_machine_key("shared").unwrap(), None);
        // Reordering or renaming configured sources never chooses a duplicate.
        let mut configured: Vec<_> = core
            .views()
            .iter()
            .map(|(key, view)| (key.clone(), view.options().clone()))
            .collect();
        configured[1].0 = "renamed".into();
        configured.reverse();
        let renamed = ViewerCore::with_machines(configured);
        assert_eq!(renamed.session_machine_key("shared").unwrap(), None);
        write(&second, "shared");
        assert!(core.invalidate("second"));
        assert_eq!(core.session_machine_key("shared").unwrap(), None);
        std::fs::remove_file(second.claude_home.join("projects/project/shared.jsonl")).unwrap();
        write(&second, "replacement");
        assert!(core.invalidate("second"));
        assert_eq!(
            core.session_machine_key("replacement").unwrap(),
            None,
            "invalidated ownership must await native publication"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let page = core.respond("GET", "/api/sessions", "machine=second", None);
            assert!(
                matches!(page.status, 200 | 503),
                "unexpected native publication response: {}",
                page.status
            );
            if core.session_machine_key("replacement").unwrap().as_deref() == Some("second") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native ownership publication did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            core.session_machine_key("replacement").unwrap().as_deref(),
            Some("second")
        );
        assert_eq!(
            core.session_machine_key("shared").unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(model::COMPAT_BUILDS.with(std::cell::Cell::get), builds);
        renamed.close();
        core.close();
        assert_eq!(core.session_machine_key("replacement").unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_admin_link_is_a_same_origin_path_with_a_label() {
        assert!(AdminLink::new("Manage machines", "/admin/machines").is_some());
        for (label, href) in [
            ("", "/x"),
            ("Manage", "https://elsewhere.test/x"),
            ("Manage", "//elsewhere.test/x"),
            ("Manage", "javascript:alert(1)"),
            ("Manage", "/x\\y"),
            ("Man\nage", "/x"),
            ("Manage", "relative"),
        ] {
            assert!(AdminLink::new(label, href).is_none(), "{label:?} {href:?}");
        }
        let link = AdminLink::new("Manage machines", "/admin/machines").unwrap();
        assert_eq!(link.label, "Manage machines");
        assert_eq!(link.href, "/admin/machines");
    }

    #[test]
    fn two_machine_native_inventory_advertises_the_api_version() {
        let root = std::env::temp_dir().join(format!(
            "semon-union-api-{}-{}",
            std::process::id(),
            model::now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let options = |name: &str| {
            let home = root.join(name);
            let claude_home = home.join("claude");
            let codex_home = home.join("codex");
            let proc_root = home.join("proc");
            std::fs::create_dir_all(&claude_home).unwrap();
            std::fs::create_dir_all(&codex_home).unwrap();
            std::fs::create_dir_all(&proc_root).unwrap();
            std::fs::write(proc_root.join("locks"), "").unwrap();
            Options {
                claude_home,
                claude_json: home.join(".claude.json"),
                codex_home,
                copilot_home: home.join("copilot"),
                proc_root,
                cache: home.join("cache"),
                all: false,
                since: std::time::Duration::from_secs(86400),
                session: None,
                facts: None,
                scan_window: false,
            }
        };
        let core = ViewerCore::with_machines(vec![
            ("first".into(), options("first")),
            ("second".into(), options("second")),
        ]);
        let reply = core.respond("GET", "/api/session-sources", "", None);
        assert_eq!(reply.status, 200);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["api"], 1);
        assert_eq!(body["items"].as_array().unwrap().len(), 2);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn account_menu_validates_all_values_and_list_counts() {
        let workspace = || AccountWorkspace {
            name: "Research".into(),
            role: "Owner".into(),
            current: true,
            switch_href: "/workspaces/research".into(),
        };
        let link = || AccountLink {
            label: "Profile".into(),
            href: "/account/profile".into(),
            method: LinkMethod::default(),
            danger: false,
        };
        let menu = || {
            AccountMenu::new(
                "Morgan Lee",
                "morgan@example.invalid",
                "ML",
                Some("/avatars/morgan.png"),
                vec![workspace()],
                vec![link()],
            )
        };
        assert!(menu().is_some());

        let overlong = format!("/{}", "x".repeat(512));
        for href in ["//x", "http://x", "/x\\y", "/x\n", overlong.as_str()] {
            assert!(
                AccountMenu::new("Morgan", "", "M", Some(href), vec![], vec![]).is_none(),
                "avatar href {href:?}"
            );
            let mut bad_workspace = workspace();
            bad_workspace.switch_href = href.to_owned();
            assert!(
                AccountMenu::new("Morgan", "", "M", None, vec![bad_workspace], vec![]).is_none(),
                "workspace href {href:?}"
            );
            let mut bad_link = link();
            bad_link.method = LinkMethod::Post;
            bad_link.href = href.to_owned();
            assert!(
                AccountMenu::new("Morgan", "", "M", None, vec![], vec![bad_link]).is_none(),
                "link href {href:?}"
            );
        }

        let mut bad_workspace = workspace();
        bad_workspace.switch_href = "//x".into();
        assert!(
            AccountMenu::new(
                "Morgan",
                "",
                "M",
                None,
                vec![workspace(), bad_workspace],
                vec![]
            )
            .is_none()
        );

        let long_name = "x".repeat(81);
        let long_login = "x".repeat(81);
        for (name, login, initials) in [
            ("", "", "M"),
            (" ", "", "M"),
            (long_name.as_str(), "", "M"),
            ("Morgan", long_login.as_str(), "M"),
            ("Morgan", "", ""),
            ("Morgan", "", "MORE"),
            ("Morgan", "\n", "M"),
        ] {
            assert!(
                AccountMenu::new(name, login, initials, None, vec![], vec![]).is_none(),
                "{name:?} {login:?} {initials:?}"
            );
        }
        assert!(AccountMenu::new("Morgan", "", "M", None, vec![workspace(); 51], vec![]).is_none());
        assert!(AccountMenu::new("Morgan", "", "M", None, vec![], vec![link(); 13]).is_none());
        assert!(
            AccountMenu::new(
                "Morgan",
                "",
                "M",
                None,
                vec![],
                vec![AccountLink {
                    label: "\n".into(),
                    ..link()
                }]
            )
            .is_none()
        );
        let long_label = "x".repeat(81);
        assert!(
            AccountMenu::new(
                "Morgan",
                "",
                "M",
                None,
                vec![],
                vec![AccountLink {
                    label: long_label,
                    ..link()
                }]
            )
            .is_none()
        );
    }

    #[test]
    fn nav_override_only_accepts_a_known_item_and_same_origin_path() {
        let root = std::env::temp_dir().join("semon-account-test");
        let options = crate::Options {
            claude_home: root.join("claude"),
            claude_json: root.join(".claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            cache: root.join("cache"),
            all: false,
            since: std::time::Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
        };
        let mut core = ViewerCore::new(options);
        assert!(!core.set_nav_override("unknown", "/x"));
        let overlong = format!("/{}", "x".repeat(512));
        for href in ["//x", "http://x", "/x\\y", "/x\n", overlong.as_str()] {
            assert!(!core.set_nav_override("machines", href), "{href:?}");
        }
        assert!(core.set_nav_override("machines", "/account/workspaces"));
        assert_eq!(
            core.extras().nav_machines.as_deref(),
            Some("/account/workspaces")
        );
    }
}

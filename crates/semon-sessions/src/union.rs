//! [`ViewerCore`]: the session viewer over one machine's homes or several.
//!
//! With one machine it is exactly the single-machine viewer. With several it
//! serves one model, the union of each machine's own:
//! - every session keeps the machine it ran on;
//! - handoffs are never linked across machines (a link is never guessed);
//!   a stub or an unsent send's stand-in belongs to its machine and is
//!   keyed `<id>@<machine>`;
//! - transcripts and entries are answered by the machine that owns the
//!   session. Session ids are UUIDs, so they don't collide; if two machines
//!   ever share one, the viewer refuses (409) rather than pick one.
//!
//! The same core serves any view across machines: an embedding server's, or
//! semon's own cross-machine view.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::{Arc, Condvar, Mutex, PoisonError, RwLock, Weak},
};

use serde_json::{Map, Value, json};

use crate::{
    Options, analytics,
    model::{self, Built, MODEL_API, fnv},
    received::{Listing, ReceivedMachines},
    refresh::RefreshPool,
    viewer::{
        MachineView, Reading, ViewerReply, decoded, has_page, has_session_page, has_trace_page,
        is_named_page, lock, page_parts, percent_encode, query_value, read_lock, write_lock,
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

/// When a [`ViewerCore`] brings each machine's model up to date with its
/// logs.
///
/// Outside [`Refresh::OnRead`], a read answers from the last model built,
/// at once, and machines are checked (and rebuilt when their logs changed)
/// off the request path by a [`RefreshPool`]'s few threads, shared by every
/// machine and core that uses the pool: the queue holds each machine once
/// at most. A machine is rebuilt at most once a second, so the changes of
/// one second are one rebuild, and it is checked only while it is read: 30 s
/// after its last read it goes idle, and its next read queues a check at
/// once. A read waits for a build only when its machine has no model yet:
/// once one is built, every read answers at once, never waiting for a
/// rebuild in progress, a late check or failing rebuilds. While the pool
/// keeps up, an answer is at most about 1 s plus one build behind the logs,
/// and the first read after an idle spell answers from the model before it
/// (a poll sees the change once the check it queued has rebuilt). When the
/// pool falls behind (every worker busy), a machine's check waits its turn,
/// and its answers are as far behind the logs as the pool's queue is.
///
/// A background rebuild that fails leaves the last model served, with the
/// error printed once, and each check tries again; 3 s after rebuilds
/// started failing, each read answers the error (500) instead, at once,
/// without waiting for a build, until a build works again. The model and
/// the V1 tree fail on their own: a tree that won't build never turns
/// `/api/model` into an error.
///
/// [`RefreshPool`]: crate::RefreshPool
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refresh {
    /// Before each read: every answer reflects the logs as they are. A read
    /// that finds a log changed rebuilds before it answers (185–303 ms for
    /// 519 files); reads that find nothing changed answer at once, side by
    /// side. No thread is used. For one-shot and tool callers. The default.
    #[default]
    OnRead,
    /// In the background, by watching the files: while a machine is read,
    /// the pool checks it every 250 ms (a stat pass over its logs) and
    /// rebuilds when they changed. For a server whose logs change on disk
    /// with no one to say so; `semon sessions --serve` uses it. The first
    /// read after the machine went idle answers from its last model and
    /// queues a check at once, since its logs may have changed unseen.
    Background,
    /// In the background, when the embedding server says a machine's logs
    /// changed ([`ViewerCore::invalidate`]): no stat pass every 250 ms, only
    /// a rebuild once invalidated, as soon as the one-second spacing
    /// allows. However many invalidations come in that second, they are one
    /// rebuild, and one that comes while a rebuild runs is one more after
    /// it. As a safety net for a writer that doesn't invalidate, a machine
    /// that is read is still checked every 30 s. The first read after the
    /// machine went idle answers at once from its last model, invalidated
    /// meanwhile or not, and the machine is checked at its next safety
    /// check (at once if none is queued), so a writer that doesn't
    /// invalidate is seen within 30 s. For servers that receive the logs
    /// themselves.
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
/// `semon sessions --serve` answers, with the in-memory model, caches and
/// live refresh behind them, and no listener, token or Host check of its own.
/// It lets another server embed the viewer: that server owns the socket and
/// decides who may ask, then hands each request to [`ViewerCore::respond`]
/// and sends back the [`ViewerReply`] with its headers
/// ([`ViewerReply::headers`]: [`SECURITY_HEADERS`](crate::SECURITY_HEADERS),
/// or an attachment's image's own).
///
/// A core reads one machine's homes ([`ViewerCore::new`]) or several
/// ([`ViewerCore::with_machines`]). A call does blocking file I/O, and the
/// first one builds the model, so an async server should call it off its
/// runtime (for example on a blocking thread).
///
/// [`ViewerCore::respond`] takes `&self`: share one core (in an `Arc`)
/// and call it from as many threads at once as there are requests. Each
/// answer comes from one snapshot of each machine's model, cloned (an
/// `Arc`) under a lock held for nothing else, and its `ETag` is that
/// snapshot's version. With [`Refresh::Background`] or
/// [`Refresh::OnInvalidate`] ([`ViewerCore::set_refresh`]) no read waits
/// for a rebuild while the refresh pool keeps up; see [`Refresh`] for how
/// far behind the logs an answer can be. An embedding server that receives
/// logs uses [`Refresh::OnInvalidate`] and calls [`ViewerCore::invalidate`]
/// once a machine's new logs are written. The setters
/// take `&mut self`: configure the core before sharing it. What differs per
/// request (an account menu, say) is passed with the call, as [`Extras`] to
/// [`ViewerCore::respond_with`], so the shared core holds no per-request
/// state and needs no lock.
/// [`ViewerCore::close`] stops it before the files it reads are removed.
pub struct ViewerCore {
    views: RwLock<Arc<Views>>,
    received: Option<Received>,
    /// What [`ViewerCore::respond`] serves in the model when a call
    /// doesn't pass its own ([`ViewerCore::respond_with`]).
    extras: Extras,
    refresh: Refresh,
    /// The pool that checks this core's machines in the background.
    pool: Arc<RefreshPool>,
    open: Gate,
    /// Views [`ViewerCore::follow`] stopped serving, which
    /// [`ViewerCore::close`] still waits out: a rebuild of one may run on.
    retired: Mutex<Vec<Weak<MachineView>>>,
    /// `/api/analytics`' answers, kept until the model changes.
    analytics: Mutex<analytics::Cache>,
}

/// The embedding server's own values in `/api/model`, which can differ per
/// request: the Machines page's management link (`admin`), the account menu
/// (`account`) and the Machines navigation destination (`nav`). None of them
/// changes the model version or its `ETag`. Pass them with each call to
/// [`ViewerCore::respond_with`], so a core shared between requests holds no
/// per-request state; [`ViewerCore::set_admin_link`] and its siblings set
/// the ones [`ViewerCore::respond`] uses.
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

    /// `body`, a model's JSON object, with these values at its front.
    fn apply(&self, body: &[u8]) -> Vec<u8> {
        with_model_extras(
            body,
            self.admin.as_ref(),
            self.account.as_ref(),
            self.nav_machines.as_deref(),
        )
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
    /// How many sessions each received machine last had left out, as
    /// another machine's: warned about when it changes.
    dropped: BTreeMap<String, usize>,
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
    version: String,
}

/// Which machine answers for a session id.
pub(crate) enum Owner {
    /// This machine (by position), under this id there.
    At(usize, String),
    Missing,
    /// Two machines both claim it: refused, never picked.
    Conflict,
}

/// One machine's part of what a core serves: its model, and the machine id
/// its machine-local ids are served under (`None` with one machine, where
/// no id is renamed).
pub(crate) struct Served {
    pub(crate) built: Arc<Built>,
    pub(crate) machine: Option<String>,
}

impl Served {
    /// The id the core serves this machine's session `id` under.
    pub(crate) fn served(&self, id: &str) -> String {
        match &self.machine {
            Some(machine) if machine_local(&self.built, id) => format!("{id}@{machine}"),
            _ => id.to_owned(),
        }
    }
}

/// Whether `id` is a stand-in only its own machine knows: a stub for the
/// far end of a handoff, or an unsent send's.
fn machine_local(built: &Built, id: &str) -> bool {
    id.starts_with("unsent:") || built.sessions.get(id).is_some_and(|session| session.stub)
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

/// The union's plan. Machines from `droppable` on (the received ones) lose
/// an id an earlier machine has: it is left out of theirs, not refused.
fn plan(parts: &[(&str, &Built)], droppable: usize) -> Plan {
    let mut machine_ids: Vec<String> = Vec::new();
    for (index, (key, built)) in parts.iter().enumerate() {
        let id = machine_id(&built.machine_id, key, index, &machine_ids);
        machine_ids.push(id);
    }
    let mut owners: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut conflicts = BTreeSet::new();
    let mut dropped = vec![BTreeSet::new(); parts.len()];
    for (index, (_, built)) in parts.iter().enumerate() {
        let ids: BTreeSet<&String> = built.sessions.keys().chain(built.tx.keys()).collect();
        for id in ids {
            let served = if machine_local(built, id) {
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
    let joined: String = parts
        .iter()
        .zip(&machine_ids)
        .map(|((_, built), id)| format!("{id}={};", built.version))
        .collect();
    Plan {
        machine_ids,
        owners,
        conflicts,
        dropped,
        droppable,
        version: format!("u{:016x}", fnv(&joined)),
    }
}

/// The union model's JSON, or the ids two machines both claim.
fn union_json(parts: &[(&str, &Built)], plan: &Plan, now: i64) -> Result<String, Vec<String>> {
    let mut conflicts: BTreeSet<String> = plan.conflicts.clone();
    let mut machines = Vec::new();
    let mut sessions = Map::new();
    let mut handoffs = Vec::new();
    let mut handoff_ids = BTreeSet::new();
    let mut turns = Vec::new();
    let mut turn_ids = BTreeSet::new();
    let mut busy = Map::new();
    let mut tx = Map::new();
    let mut served_now = Value::from(now);
    for (index, (_, built)) in parts.iter().enumerate() {
        let machine_id = &plan.machine_ids[index];
        // A received machine's part an earlier machine already serves is
        // left out, never refused.
        let droppable = index >= plan.droppable;
        let dropped = |id: &str| plan.dropped[index].contains(id);
        let rename = |id: &str| -> String {
            if machine_local(built, id) {
                format!("{id}@{machine_id}")
            } else {
                id.to_owned()
            }
        };
        let Ok(Value::Object(mut model)) = serde_json::from_str::<Value>(&built.json(now)) else {
            continue;
        };
        if let Some(value) = model.remove("now") {
            served_now = value;
        }
        if let Some(Value::Object(mut machine)) = model.remove("machine") {
            machine.insert("id".into(), Value::from(machine_id.as_str()));
            machines.push(Value::Object(machine));
        }
        if let Some(Value::Object(own)) = model.remove("sessions") {
            for (id, mut session) in own {
                if dropped(id.as_str()) {
                    continue;
                }
                if let Some(fields) = session.as_object_mut() {
                    fields.insert("machine".into(), Value::from(machine_id.as_str()));
                }
                let served = rename(&id);
                if droppable && sessions.contains_key(&served) {
                    continue;
                }
                if sessions.insert(served.clone(), session).is_some() {
                    conflicts.insert(served);
                }
            }
        }
        if let Some(Value::Array(own)) = model.remove("handoffs") {
            for mut handoff in own {
                let ends_dropped = ["from", "to"].into_iter().any(|end| {
                    handoff
                        .get(end)
                        .and_then(Value::as_str)
                        .is_some_and(dropped)
                });
                let id = handoff.get("id").and_then(Value::as_str);
                if ends_dropped || (droppable && id.is_some_and(|id| handoff_ids.contains(id))) {
                    continue;
                }
                for end in ["from", "to"] {
                    if let Some(served) = handoff.get(end).and_then(Value::as_str).map(rename) {
                        handoff[end] = Value::from(served);
                    }
                }
                if let Some(id) = handoff.get("id").and_then(Value::as_str)
                    && !handoff_ids.insert(id.to_owned())
                {
                    conflicts.insert(id.to_owned());
                }
                handoffs.push(handoff);
            }
        }
        if let Some(Value::Array(own)) = model.remove("turns") {
            for mut turn in own {
                let sid_dropped = turn.get("sid").and_then(Value::as_str).is_some_and(dropped);
                let id = turn.get("id").and_then(Value::as_str);
                if sid_dropped || (droppable && id.is_some_and(|id| turn_ids.contains(id))) {
                    continue;
                }
                if let Some(served) = turn.get("sid").and_then(Value::as_str).map(rename) {
                    turn["sid"] = Value::from(served);
                }
                if let Some(id) = turn.get("id").and_then(Value::as_str)
                    && !turn_ids.insert(id.to_owned())
                {
                    conflicts.insert(id.to_owned());
                }
                turns.push(turn);
            }
        }
        if let Some(Value::Object(own)) = model.remove("busy") {
            for (_, intervals) in own {
                busy.insert(machine_id.clone(), intervals);
            }
        }
        if let Some(Value::Object(own)) = model.remove("tx") {
            for (id, mark) in own {
                if !dropped(id.as_str()) {
                    tx.insert(rename(&id), mark);
                }
            }
        }
    }
    if !conflicts.is_empty() {
        return Err(conflicts.into_iter().collect());
    }
    let first = machines.first().cloned().unwrap_or(Value::Null);
    let union = json!({
        "api": MODEL_API,
        "version": plan.version,
        "now": served_now,
        "machine": first,
        "machines": machines,
        "sessions": sessions,
        "handoffs": handoffs,
        "turns": turns,
        "busy": busy,
        "tx": tx,
    });
    Ok(union.to_string())
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

/// Adds per-request embedding values at the front of a model's JSON object.
fn with_model_extras(
    body: &[u8],
    admin: Option<&AdminLink>,
    account: Option<&AccountMenu>,
    nav_machines: Option<&str>,
) -> Vec<u8> {
    if admin.is_none() && account.is_none() && nav_machines.is_none() {
        return body.to_vec();
    }
    let mut out = b"{".to_vec();
    let mut first = true;
    let mut add = |name: &str, value: Value| {
        if !first {
            out.push(b',');
        }
        first = false;
        out.extend_from_slice(format!("\"{name}\":{}", value).as_bytes());
    };
    if let Some(admin) = admin {
        add("admin", json!({ "label": admin.label, "href": admin.href }));
    }
    if let Some(account) = account {
        add(
            "account",
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
            }),
        );
    }
    if let Some(href) = nav_machines {
        add("nav", json!({ "machines": href }));
    }
    if body.get(1).is_some_and(|byte| *byte != b'}') {
        out.push(b',');
    }
    out.extend_from_slice(body.get(1..).unwrap_or_default());
    out
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

fn conflict(ids: &[String]) -> ViewerReply {
    ViewerReply {
        status: 409,
        content_type: "application/json; charset=utf-8",
        body: json!({ "error": "id_conflict", "ids": ids })
            .to_string()
            .into_bytes(),
        etag: None,
    }
}

/// `query` with its `key` parameter set to `value`.
fn with_param(query: &str, key: &str, value: &str) -> String {
    query
        .split('&')
        .map(|part| match part.split_once('=') {
            Some((name, _)) if name == key => format!("{key}={}", percent_encode(value)),
            _ => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Each machine's key and model, in the order they are served.
fn parts<'a>(views: &'a Views, models: &'a [Arc<Built>]) -> Vec<(&'a str, &'a Built)> {
    views
        .iter()
        .zip(models)
        .map(|((key, _), built)| (key.as_str(), &**built))
        .collect()
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
        Self {
            views: RwLock::new(Arc::new(
                machines
                    .into_iter()
                    .map(|(key, options)| (key, MachineView::new(options)))
                    .collect(),
            )),
            received: None,
            extras: Extras::default(),
            refresh: Refresh::OnRead,
            pool: RefreshPool::shared(),
            open: Gate::default(),
            retired: Mutex::default(),
            analytics: Mutex::default(),
        }
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
                dropped: BTreeMap::new(),
            }),
        });
        core
    }

    /// The machines served now.
    fn views(&self) -> Arc<Views> {
        read_lock(&self.views).clone()
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
                        && view.options().codex_home == options.codex_home =>
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

    /// Sets when the core brings its models up to date with the logs: see
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
    /// server's machine-management page, served in the model as `admin` by
    /// [`ViewerCore::respond`]. A value that differs per request goes in
    /// the [`Extras`] passed to [`ViewerCore::respond_with`] instead.
    pub fn set_admin_link(&mut self, link: Option<AdminLink>) {
        self.extras.set_admin_link(link);
    }

    /// Sets, or clears, the account menu served in `/api/model` as `account`
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
    /// Any number of threads may call it at once. The model carries the
    /// [`Extras`] the core's setters set. Outside [`Refresh::OnRead`] only
    /// the API routes refresh a model: a page, including one that names a
    /// machine, session or trace, is answered from the models already built
    /// and never waits for a build.
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
    /// model instead of the core's.
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
        self.follow();
        let views = self.views();
        if views.is_empty() {
            return text(404, "Not found");
        }
        // One machine or several, Analytics is the core's own answer.
        if path == "/api/analytics" {
            return self
                .analytics(&views, query, if_none_match)
                .unwrap_or_else(|error| failed(&error));
        }
        // A page never waits for a build: only the API routes refresh.
        if let Some(reply) = self.page_at_once(&views, path) {
            return reply;
        }
        // With received machines the tree always names each node's machine.
        if views.len() == 1 && !(self.received.is_some() && path == "/api/tree") {
            let mut reply = views[0].1.respond(method, path, query, if_none_match);
            if path == "/api/model" && reply.status == 200 {
                reply.body = extras.apply(&reply.body);
            }
            return reply;
        }
        let answer = match path {
            "/api/model" => self.model(&views, extras, query, if_none_match),
            "/api/tx" | "/api/attachment" => self.by_session(&views, path, query, if_none_match),
            "/api/entry" if query_value(query, "as").is_some() => {
                self.by_session(&views, path, query, if_none_match)
            }
            "/api/entry" | "/api/transcript" => {
                self.by_transcript(&views, path, query, if_none_match)
            }
            "/api/tree" => self.tree(&views),
            _ if is_named_page(path) => self.page(&views, path),
            _ => Ok(views[0].1.respond(method, path, query, if_none_match)),
        };
        answer.unwrap_or_else(|error| failed(&error))
    }

    /// Every machine's model, read as `read` says, and the union's plan.
    /// Each model is read once, so an answer comes from one snapshot of
    /// each.
    fn refresh_at(&self, views: &Views, read: Reading) -> io::Result<(Vec<Arc<Built>>, Plan)> {
        let models = views
            .iter()
            .map(|(_, view)| view.built(read))
            .collect::<io::Result<Vec<_>>>()?;
        let plan = plan(&parts(views, &models), self.droppable());
        if let Some(received) = &self.received {
            let mut following = lock(&received.following);
            for ((name, _), dropped) in views.iter().zip(&plan.dropped).skip(received.fixed) {
                let count = dropped.len();
                if following.dropped.insert(name.clone(), count).unwrap_or(0) != count && count > 0
                {
                    eprintln!(
                        "semon: received machine {name}: {count} session(s) an earlier machine \
                         already has are left out of it (if it is this machine's own push, use \
                         --no-local)"
                    );
                }
            }
        }
        Ok((models, plan))
    }

    /// The first view whose ids an earlier view's win over: the first
    /// received machine's.
    fn droppable(&self) -> usize {
        self.received
            .as_ref()
            .map_or(usize::MAX, |received| received.fixed)
    }

    fn model(
        &self,
        views: &Views,
        extras: &Extras,
        query: &str,
        if_none_match: Option<&str>,
    ) -> io::Result<ViewerReply> {
        let json = "application/json; charset=utf-8";
        let (models, plan) = self.refresh_at(views, Reading::Served)?;
        let etag = format!("\"{}\"", plan.version);
        let since = query_value(query, "since").and_then(decoded);
        if if_none_match == Some(etag.as_str()) || since.as_deref() == Some(plan.version.as_str()) {
            return Ok(ViewerReply {
                status: 304,
                content_type: json,
                body: Vec::new(),
                etag: Some(etag),
            });
        }
        let now = crate::model::now_ms();
        match union_json(&parts(views, &models), &plan, now) {
            Ok(body) => {
                let body = body.into_bytes();
                Ok(ViewerReply {
                    status: 200,
                    content_type: json,
                    body: extras.apply(&body),
                    etag: Some(etag),
                })
            }
            Err(ids) => Ok(conflict(&ids)),
        }
    }

    /// `/api/analytics?range=24h|7d|30d` (and the page's filters, see
    /// [`analytics::Request`]): the figures, charts and lists Analytics draws
    /// for the range, over every machine served, from the rows each build
    /// keeps ([`analytics::Activity`]); nothing is built for it. The answer
    /// is kept per range and filters until the model changes (and at least
    /// a second), or for 30 s while nothing does, as time moves the range.
    /// Its `ETag` hashes the answer but for the time it was computed at: a
    /// poll whose `If-None-Match` still matches is a 304. A filter value no
    /// session has is answered (empty) but not kept. `version` in the body
    /// is the model version it was computed from, as `/api/model` names it.
    fn analytics(
        &self,
        views: &Views,
        query: &str,
        if_none_match: Option<&str>,
    ) -> io::Result<ViewerReply> {
        let json = "application/json; charset=utf-8";
        let Some(request) = analytics::Request::parse(query) else {
            return Ok(text(400, "Invalid request"));
        };
        let (models, plan) = self.refresh_at(views, Reading::Served)?;
        if !plan.conflicts.is_empty() {
            return Ok(conflict(
                &plan.conflicts.iter().cloned().collect::<Vec<_>>(),
            ));
        }
        let version = match models.as_slice() {
            [one] => one.version.clone(),
            _ => plan.version.clone(),
        };
        let key = request.key();
        // The lock is held to look up or to keep an answer, never to
        // compute one: two reads that miss at once may both compute it.
        let kept = lock(&self.analytics).kept(&key, &version);
        let kept = match kept {
            Some(kept) => kept,
            None => {
                let rows = analytics::rows(
                    plan.machine_ids
                        .iter()
                        .zip(&models)
                        .map(|(machine, built)| (machine.as_str(), &built.activity)),
                );
                let answer = analytics::answer(&rows, &request, crate::model::now_ms(), &version);
                let kept = analytics::Kept {
                    etag: analytics::etag(&answer),
                    body: Arc::new(serde_json::to_vec(&answer).map_err(io::Error::other)?),
                };
                // A filter value no session has gets its (empty) answer,
                // not a place among the kept ones.
                if request.known(&rows) {
                    lock(&self.analytics).keep(&key, &version, kept.clone());
                }
                kept
            }
        };
        let etag = Some(kept.etag.clone());
        if if_none_match == Some(kept.etag.as_str()) {
            return Ok(ViewerReply {
                status: 304,
                content_type: json,
                body: Vec::new(),
                etag,
            });
        }
        Ok(ViewerReply {
            status: 200,
            content_type: json,
            body: kept.body.to_vec(),
            etag,
        })
    }

    /// `/api/tx`, `/api/attachment` and `/api/entry?sid=…`: the machine that
    /// owns the session.
    fn by_session(
        &self,
        views: &Views,
        path: &str,
        query: &str,
        if_none_match: Option<&str>,
    ) -> io::Result<ViewerReply> {
        let Some(sid) = query_value(query, "sid").and_then(decoded) else {
            return Ok(text(400, "Invalid request"));
        };
        Ok(match self.owner_in(views, &sid, Reading::Served)? {
            Owner::Conflict => conflict(&[sid]),
            Owner::Missing => text(404, "Not found"),
            Owner::At(index, own) => {
                let query = if own == sid {
                    query.to_owned()
                } else {
                    with_param(query, "sid", &own)
                };
                views[index].1.respond("GET", path, &query, if_none_match)
            }
        })
    }

    /// The machine among `views` that answers for session `sid`, and the
    /// session's id there. With one machine that is always the one
    /// machine, under `sid` itself: whether it has the session is its own
    /// answer.
    fn owner_in(&self, views: &Views, sid: &str, read: Reading) -> io::Result<Owner> {
        match views.len() {
            0 => Ok(Owner::Missing),
            1 => Ok(Owner::At(0, sid.to_owned())),
            _ => {
                let (_, plan) = self.refresh_at(views, read)?;
                if plan.conflicts.contains(sid) {
                    return Ok(Owner::Conflict);
                }
                Ok(match plan.owners.get(sid) {
                    Some((index, own)) => Owner::At(*index, own.clone()),
                    None => Owner::Missing,
                })
            }
        }
    }

    /// The machine that answers for session `sid`, every model brought up
    /// to date first.
    pub(crate) fn owner(&self, sid: &str) -> io::Result<Owner> {
        self.follow();
        self.owner_in(&self.views(), sid, Reading::At(crate::model::now_ms()))
    }

    /// Machine `index`'s model, rebuilt first if its logs changed.
    pub(crate) fn built_at(&self, index: usize) -> io::Result<Arc<Built>> {
        self.follow();
        self.views()
            .get(index)
            .ok_or(io::ErrorKind::NotFound)?
            .1
            .built(Reading::At(crate::model::now_ms()))
    }

    /// The model `/api/model` serves (without an admin link) at `now`, or
    /// the ids two machines both claim.
    pub(crate) fn model_at(&self, now: i64) -> io::Result<Result<String, Vec<String>>> {
        self.follow();
        let views = self.views();
        match views.len() {
            0 => Err(io::ErrorKind::NotFound.into()),
            1 => Ok(Ok(views[0].1.built(Reading::At(now))?.json(now))),
            _ => {
                let (models, plan) = self.refresh_at(&views, Reading::At(now))?;
                Ok(union_json(&parts(&views, &models), &plan, now))
            }
        }
    }

    /// Every machine's model, brought up to date, with how the core serves
    /// its session ids.
    pub(crate) fn served(&self, now: i64) -> io::Result<Vec<Served>> {
        self.follow();
        let views = self.views();
        let (models, machine_ids) = if views.len() > 1 {
            let (models, plan) = self.refresh_at(&views, Reading::At(now))?;
            (models, plan.machine_ids)
        } else {
            let models = views
                .iter()
                .map(|(_, view)| view.built(Reading::At(now)))
                .collect::<io::Result<Vec<_>>>()?;
            (models, Vec::new())
        };
        Ok(models
            .into_iter()
            .enumerate()
            .map(|(index, built)| Served {
                built,
                machine: machine_ids.get(index).cloned(),
            })
            .collect())
    }

    /// The V1 routes that name a transcript by harness and id: the machine
    /// that has that file.
    fn by_transcript(
        &self,
        views: &Views,
        path: &str,
        query: &str,
        if_none_match: Option<&str>,
    ) -> io::Result<ViewerReply> {
        let (Some(harness), Some(id)) = (
            query_value(query, "harness").and_then(decoded),
            query_value(query, "id").and_then(decoded),
        ) else {
            return Ok(text(400, "Invalid request"));
        };
        let mut found = Vec::new();
        for (index, (_, view)) in views.iter().enumerate() {
            if view.has_transcript(&harness, &id)? {
                found.push(index);
            }
        }
        let droppable = self.droppable();
        Ok(match found.as_slice() {
            [index] => views[*index].1.respond("GET", path, query, if_none_match),
            [] => text(404, "Not found"),
            // Received machines' copies give way to the first.
            [index, rest @ ..] if rest.iter().all(|later| *later >= droppable) => {
                views[*index].1.respond("GET", path, query, if_none_match)
            }
            _ => conflict(&[id]),
        })
    }

    /// The V1 tree: every machine's roots, each node with its machine's
    /// id as `machine`.
    fn tree(&self, views: &Views) -> io::Result<ViewerReply> {
        let (_, plan) = self.refresh_at(views, Reading::Served)?;
        let droppable = self.droppable();
        let mut roots = Vec::new();
        let mut seen = BTreeSet::new();
        for (index, (_, view)) in views.iter().enumerate() {
            let machine = plan.machine_ids.get(index).cloned().unwrap_or_default();
            for root in view.tree_roots()? {
                // A received machine's copy of a root an earlier machine
                // has is left out, as in the model.
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

    /// A page URL that names something in the union: a machine, a session
    /// or a trace. Checked against every model brought up to date first,
    /// whatever the refresh mode: outside OnRead only a URL the built
    /// models lack comes here, and it may name a session newer than them
    /// (see [`ViewerCore::page_at_once`]).
    fn page(&self, views: &Views, path: &str) -> io::Result<ViewerReply> {
        let Some(parts) = page_parts(path) else {
            return Ok(text(400, "Invalid request"));
        };
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let (models, plan) = self.refresh_at(views, Reading::At(crate::model::now_ms()))?;
        Ok(if union_has_page(&plan, &models, &parts) {
            views[0].1.respond("GET", "/", "", None)
        } else {
            text(404, "Not found")
        })
    }

    /// A page URL that names something (`/machines/…`, `/s/…`, `/trace/…`),
    /// answered from the models already built, without refreshing one,
    /// outside [`Refresh::OnRead`]. The page is the same shell whatever it
    /// names, and it asks for the model itself, so it never waits for a
    /// build: it is served when the models built so far have what it names,
    /// and when a machine has no model yet (the page's own `/api/model`
    /// builds it, and a URL that names nothing there shows the home screen).
    /// `None` leaves the answer to the usual route, which is exact:
    /// refreshed on read, a URL the built models lack (checked against
    /// models brought up to date first, since it may name a session newer
    /// than them, even one a warm in progress already took the invalidation
    /// for; 404 if it still names nothing), and one that doesn't decode
    /// (400).
    fn page_at_once(&self, views: &Views, path: &str) -> Option<ViewerReply> {
        if self.refresh == Refresh::OnRead || !is_named_page(path) {
            return None;
        }
        let segments = page_parts(path)?;
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        let models: Option<Vec<Arc<Built>>> =
            views.iter().map(|(_, view)| view.shown_built()).collect();
        let named = match models {
            None => true,
            // One machine is served as the machine itself, no id renamed.
            Some(models) if views.len() == 1 => has_page(&models[0], &segments),
            Some(models) => {
                let plan = plan(&parts(views, &models), self.droppable());
                union_has_page(&plan, &models, &segments)
            }
        };
        named.then(|| views[0].1.respond("GET", "/", "", None))
    }

    /// Brings every machine's model up to date now, on the calling thread,
    /// so that the reads that come later find it built: a machine with no
    /// model yet is built, and one whose logs changed (invalidated or not;
    /// a stat pass tells) is rebuilt. It is for an embedding server that
    /// wants models warm before anyone reads them, for example once at
    /// start and again, spaced out, after it writes a machine's logs.
    ///
    /// It is not a read: in [`Refresh::Background`] or
    /// [`Refresh::OnInvalidate`] it starts no background checks, and it
    /// clears the invalidations it covers, so the first read after an idle
    /// spell answers at once from what it built. It blocks for as long as
    /// the builds take (a machine's first build reads all its logs): call
    /// it off an async runtime, and bound how many run at once. A read of a
    /// machine that has a model never waits for it; the first read of one
    /// with none waits for the machine it is building, then finds it built. Every machine is tried; the first error is returned. Once the
    /// core is closed it does nothing.
    pub fn warm(&self) -> io::Result<()> {
        let Some(_entered) = self.open.enter() else {
            return Ok(());
        };
        self.follow();
        let mut first = Ok(());
        for (_, view) in self.views().iter() {
            if let Err(error) = view.warm()
                && first.is_ok()
            {
                first = Err(error);
            }
        }
        first
    }

    /// [`ViewerCore::warm`] for the one machine served under `machine`, as
    /// [`ViewerCore::invalidate`] names it: the key given to
    /// [`ViewerCore::with_machines`], a received machine's directory name,
    /// or `""` for [`ViewerCore::new`]'s one. No other machine is looked
    /// at, so an embedding server that warms the machine it just wrote
    /// rebuilds nothing else. Returns whether a machine is served under
    /// that key: false once the core is closed, and for a received machine
    /// that is new since the last request.
    pub fn warm_machine(&self, machine: &str) -> io::Result<bool> {
        let Some(_entered) = self.open.enter() else {
            return Ok(false);
        };
        let views = self.views();
        let Some((_, view)) = views.iter().find(|(key, _)| key == machine) else {
            return Ok(false);
        };
        view.warm()?;
        Ok(true)
    }
}

/// Whether the union these models and plan serve has the page the decoded
/// path segments name: a machine, a session or a trace.
fn union_has_page(plan: &Plan, models: &[Arc<Built>], parts: &[&str]) -> bool {
    let built = |id: &str| {
        plan.owners
            .get(id)
            .filter(|_| !plan.conflicts.contains(id))
            .map(|(index, own)| (&*models[*index], own.clone()))
    };
    match parts {
        ["machines", id] => plan.machine_ids.iter().any(|machine| machine == id),
        ["s", harness, id] => {
            built(id).is_some_and(|(built, own)| has_session_page(built, harness, &own))
        }
        ["trace", harness, id, turn] => {
            built(id).is_some_and(|(built, own)| has_trace_page(built, harness, &own, turn))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            with_model_extras(b"{\"api\":1,\"version\":\"v\"}", Some(&link), None, None),
            b"{\"admin\":{\"label\":\"Manage machines\",\"href\":\"/admin/machines\"},\"api\":1,\"version\":\"v\"}"
        );
        assert_eq!(
            with_param("sid=a%40b&before=3", "sid", "a"),
            "sid=a&before=3"
        );
    }

    #[test]
    fn two_machine_model_advertises_the_api_version() {
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
        let reply = core.respond("GET", "/api/model", "", None);
        assert_eq!(reply.status, 200);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["api"], MODEL_API);
        assert_eq!(body["machines"].as_array().unwrap().len(), 2);
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

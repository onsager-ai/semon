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
};

use serde_json::{Map, Value, json};

use crate::{
    Options,
    model::{self, Built, MODEL_API, fnv},
    received::{Listing, ReceivedMachines},
    viewer::{
        MachineView, ViewerReply, decoded, has_session_page, has_trace_page, percent_encode,
        query_value,
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

/// The session viewer without a transport: the pages, assets and API routes
/// `semon sessions --serve` answers, with the in-memory model, caches and
/// live refresh behind them, and no listener, token or Host check of its own.
/// It lets another server embed the viewer: that server owns the socket and
/// decides who may ask, then hands each request to [`ViewerCore::respond`]
/// and sends back the [`ViewerReply`] with
/// [`SECURITY_HEADERS`](crate::SECURITY_HEADERS).
///
/// A core reads one machine's homes ([`ViewerCore::new`]) or several
/// ([`ViewerCore::with_machines`]). A call does blocking file I/O, and the
/// first one builds the model, so an async server should call it off its
/// runtime (for example on a blocking thread) and keep the core behind a
/// lock: one core answers one request at a time.
pub struct ViewerCore {
    views: Vec<(String, MachineView)>,
    received: Option<Following>,
    admin: Option<AdminLink>,
    account: Option<AccountMenu>,
    nav_machines: Option<String>,
}

/// The received machines a core follows, served after its fixed ones.
struct Following {
    machines: ReceivedMachines,
    /// How many of the core's views, from the front, are its fixed machines.
    fixed: usize,
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
pub(crate) struct Served<'a> {
    pub(crate) built: &'a Built,
    pub(crate) machine: Option<String>,
}

impl Served<'_> {
    /// The id the core serves this machine's session `id` under.
    pub(crate) fn served(&self, id: &str) -> String {
        match &self.machine {
            Some(machine) if machine_local(self.built, id) => format!("{id}@{machine}"),
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
            views: machines
                .into_iter()
                .map(|(key, options)| (key, MachineView::new(options)))
                .collect(),
            received: None,
            admin: None,
            account: None,
            nav_machines: None,
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
        core.received = Some(Following {
            machines: received,
            fixed,
            hosts: None,
            seen: None,
            warned: BTreeSet::new(),
            dropped: BTreeMap::new(),
        });
        core
    }

    /// Brings the served machines in line with the received ones, when
    /// this core follows any: one pass over `DIR/machines/`, and changes
    /// only where it differs from the last.
    fn follow(&mut self) {
        let Self {
            views, received, ..
        } = self;
        let Some(following) = received else {
            return;
        };
        let listing = following.machines.scan();
        if following.seen.as_ref() == Some(&listing) {
            return;
        }
        let previous = following.seen.take().unwrap_or_default();
        let fixed = following.fixed;
        let hosts = following.hosts.get_or_insert_with(|| {
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
                if following.warned.insert(name.clone()) {
                    eprintln!(
                        "semon: {name} in {}: not a machine directory (a directory named with \
                         a-z, 0-9 and -, 1 to 63 of them); ignored",
                        following.machines.dir().join("machines").display()
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
                following
                    .machines
                    .copy_facts(name, seen, hosts, &mut following.warned);
            }
        }
        let mut old: BTreeMap<String, MachineView> = views.drain(fixed..).collect();
        for (name, seen) in &listing {
            let Some(seen) = seen else {
                continue;
            };
            let options = following.machines.options(name, seen);
            let view = match old.remove(name) {
                Some(view)
                    if view.options().claude_home == options.claude_home
                        && view.options().codex_home == options.codex_home =>
                {
                    view
                }
                _ => MachineView::new(options),
            };
            views.push((name.clone(), view));
        }
        following.seen = Some(listing);
    }

    /// How many machines this core serves.
    pub fn machines(&self) -> usize {
        self.views.len()
    }

    /// Sets, or clears, the link the Machines page shows to the embedding
    /// server's machine-management page. It is served in the model as
    /// `admin`, so it can differ per request.
    pub fn set_admin_link(&mut self, link: Option<AdminLink>) {
        self.admin = link;
    }

    /// Sets, or clears, the account menu served in `/api/model` as `account`.
    /// It can differ per request and is not included in the model version or
    /// its `ETag`.
    pub fn set_account(&mut self, account: Option<AccountMenu>) {
        self.account = account;
    }

    /// Sets the destination for the Machines navigation entry. `item` must
    /// be `"machines"`, and `href` must be a same-origin path. Returns false
    /// and leaves the current value unchanged for an unknown item or path.
    pub fn set_nav_override(&mut self, item: &str, href: &str) -> bool {
        if item != "machines" || !valid_href(href) {
            return false;
        }
        self.nav_machines = Some(href.to_owned());
        true
    }

    /// Answers one request: `path` and `query` split at the `?`, still
    /// percent-encoded, and the request's `If-None-Match`. Only GET is
    /// answered (405 otherwise); every URL the viewer uses is a GET. The
    /// caller authenticates first: the core serves whoever it is handed.
    pub fn respond(
        &mut self,
        method: &str,
        path: &str,
        query: &str,
        if_none_match: Option<&str>,
    ) -> ViewerReply {
        if method != "GET" {
            return text(405, "Method not allowed");
        }
        self.follow();
        if self.views.is_empty() {
            return text(404, "Not found");
        }
        // With received machines the tree always names each node's machine.
        if self.views.len() == 1 && !(self.received.is_some() && path == "/api/tree") {
            let mut reply = self.views[0].1.respond(method, path, query, if_none_match);
            if path == "/api/model" && reply.status == 200 {
                reply.body = with_model_extras(
                    &reply.body,
                    self.admin.as_ref(),
                    self.account.as_ref(),
                    self.nav_machines.as_deref(),
                );
            }
            return reply;
        }
        let answer = match path {
            "/api/model" => self.model(query, if_none_match),
            "/api/tx" => self.by_session(path, query, if_none_match),
            "/api/entry" if query_value(query, "as").is_some() => {
                self.by_session(path, query, if_none_match)
            }
            "/api/entry" | "/api/transcript" => self.by_transcript(path, query, if_none_match),
            "/api/tree" => self.tree(),
            _ if path.starts_with("/machines/")
                || path.starts_with("/s/")
                || path.starts_with("/trace/") =>
            {
                self.page(path)
            }
            _ => Ok(self.views[0].1.respond(method, path, query, if_none_match)),
        };
        answer.unwrap_or_else(|error| failed(&error))
    }

    /// Brings every machine's model up to date, and plans the union.
    fn refresh(&mut self) -> io::Result<Plan> {
        self.refresh_at(crate::model::now_ms())
    }

    /// [`ViewerCore::refresh`], a rebuild taking `now` as its clock.
    fn refresh_at(&mut self, now: i64) -> io::Result<Plan> {
        for (_, view) in &mut self.views {
            view.built_at(now)?;
        }
        let plan = plan(&self.parts(), self.droppable());
        if let Some(following) = &mut self.received {
            for ((name, _), dropped) in self.views.iter().zip(&plan.dropped).skip(following.fixed) {
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
        Ok(plan)
    }

    /// The first view whose ids an earlier view's win over: the first
    /// received machine's.
    fn droppable(&self) -> usize {
        self.received
            .as_ref()
            .map_or(usize::MAX, |following| following.fixed)
    }

    fn parts(&self) -> Vec<(&str, &Built)> {
        self.views
            .iter()
            .filter_map(|(key, view)| view.last_built().map(|built| (key.as_str(), built)))
            .collect()
    }

    fn model(&mut self, query: &str, if_none_match: Option<&str>) -> io::Result<ViewerReply> {
        let json = "application/json; charset=utf-8";
        let plan = self.refresh()?;
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
        match union_json(&self.parts(), &plan, now) {
            Ok(body) => {
                let body = body.into_bytes();
                Ok(ViewerReply {
                    status: 200,
                    content_type: json,
                    body: with_model_extras(
                        &body,
                        self.admin.as_ref(),
                        self.account.as_ref(),
                        self.nav_machines.as_deref(),
                    ),
                    etag: Some(etag),
                })
            }
            Err(ids) => Ok(conflict(&ids)),
        }
    }

    /// `/api/tx` and `/api/entry?sid=…`: the machine that owns the session.
    fn by_session(
        &mut self,
        path: &str,
        query: &str,
        if_none_match: Option<&str>,
    ) -> io::Result<ViewerReply> {
        let Some(sid) = query_value(query, "sid").and_then(decoded) else {
            return Ok(text(400, "Invalid request"));
        };
        Ok(match self.owner(&sid)? {
            Owner::Conflict => conflict(&[sid]),
            Owner::Missing => text(404, "Not found"),
            Owner::At(index, own) => {
                let query = if own == sid {
                    query.to_owned()
                } else {
                    with_param(query, "sid", &own)
                };
                self.views[index]
                    .1
                    .respond("GET", path, &query, if_none_match)
            }
        })
    }

    /// The machine that answers for session `sid`, and the session's id
    /// there. With one machine that is always the one machine, under `sid`
    /// itself: whether it has the session is its own answer.
    pub(crate) fn owner(&mut self, sid: &str) -> io::Result<Owner> {
        self.follow();
        match self.views.len() {
            0 => Ok(Owner::Missing),
            1 => Ok(Owner::At(0, sid.to_owned())),
            _ => {
                let plan = self.refresh()?;
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

    /// Machine `index`'s model, rebuilt first if its logs changed.
    pub(crate) fn built_at(&mut self, index: usize) -> io::Result<&Built> {
        self.follow();
        self.views
            .get_mut(index)
            .ok_or(io::ErrorKind::NotFound)?
            .1
            .built()
    }

    /// The model `/api/model` serves (without an admin link) at `now`, or
    /// the ids two machines both claim.
    pub(crate) fn model_at(&mut self, now: i64) -> io::Result<Result<String, Vec<String>>> {
        self.follow();
        match self.views.len() {
            0 => Err(io::ErrorKind::NotFound.into()),
            1 => Ok(Ok(self.views[0].1.built_at(now)?.json(now))),
            _ => {
                let plan = self.refresh_at(now)?;
                Ok(union_json(&self.parts(), &plan, now))
            }
        }
    }

    /// Every machine's model, brought up to date, with how the core serves
    /// its session ids.
    pub(crate) fn served(&mut self, now: i64) -> io::Result<Vec<Served<'_>>> {
        self.follow();
        let machine_ids = if self.views.len() > 1 {
            self.refresh_at(now)?.machine_ids
        } else {
            for (_, view) in &mut self.views {
                view.built_at(now)?;
            }
            Vec::new()
        };
        Ok(self
            .views
            .iter()
            .enumerate()
            .filter_map(|(index, (_, view))| {
                view.last_built().map(|built| Served {
                    built,
                    machine: machine_ids.get(index).cloned(),
                })
            })
            .collect())
    }

    /// The V1 routes that name a transcript by harness and id: the machine
    /// that has that file.
    fn by_transcript(
        &mut self,
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
        for (index, (_, view)) in self.views.iter_mut().enumerate() {
            if view.has_transcript(&harness, &id)? {
                found.push(index);
            }
        }
        let droppable = self.droppable();
        Ok(match found.as_slice() {
            [index] => self.views[*index]
                .1
                .respond("GET", path, query, if_none_match),
            [] => text(404, "Not found"),
            // Received machines' copies give way to the first.
            [index, rest @ ..] if rest.iter().all(|later| *later >= droppable) => self.views
                [*index]
                .1
                .respond("GET", path, query, if_none_match),
            _ => conflict(&[id]),
        })
    }

    /// The V1 tree: every machine's roots, each node with its machine's
    /// id as `machine`.
    fn tree(&mut self) -> io::Result<ViewerReply> {
        let plan = self.refresh()?;
        let droppable = self.droppable();
        let mut roots = Vec::new();
        let mut seen = BTreeSet::new();
        for (index, (_, view)) in self.views.iter_mut().enumerate() {
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
    /// or a trace.
    fn page(&mut self, path: &str) -> io::Result<ViewerReply> {
        let parts: Option<Vec<String>> = path
            .trim_start_matches('/')
            .split('/')
            .map(decoded)
            .collect();
        let Some(parts) = parts else {
            return Ok(text(400, "Invalid request"));
        };
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let plan = self.refresh()?;
        let built = |id: &str| {
            plan.owners
                .get(id)
                .filter(|_| !plan.conflicts.contains(id))
                .and_then(|(index, own)| {
                    self.views[*index]
                        .1
                        .last_built()
                        .map(|built| (built, own.clone()))
                })
        };
        let exists = match parts.as_slice() {
            ["machines", id] => plan.machine_ids.iter().any(|machine| machine == id),
            ["s", harness, id] => {
                built(id).is_some_and(|(built, own)| has_session_page(built, harness, &own))
            }
            ["trace", harness, id, turn] => {
                built(id).is_some_and(|(built, own)| has_trace_page(built, harness, &own, turn))
            }
            _ => false,
        };
        Ok(if exists {
            self.views[0].1.respond("GET", "/", "", None)
        } else {
            text(404, "Not found")
        })
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
        let mut core = ViewerCore::with_machines(vec![
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
        assert_eq!(core.nav_machines.as_deref(), Some("/account/workspaces"));
    }
}

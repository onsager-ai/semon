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
    Node, Options,
    model::{Built, fnv},
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
    /// `href` must be a same-origin path (`/…`, not `//…`), and `label` 1 to
    /// 80 characters; control characters are refused in both.
    pub fn new(label: &str, href: &str) -> Option<Self> {
        let clean = |text: &str| !text.chars().any(char::is_control);
        let label_ok = !label.trim().is_empty() && label.chars().count() <= 80 && clean(label);
        let href_ok = href.starts_with('/')
            && !href.starts_with("//")
            && !href.contains('\\')
            && href.len() <= 512
            && clean(href);
        (label_ok && href_ok).then(|| Self {
            label: label.trim().to_owned(),
            href: href.to_owned(),
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
    admin: Option<AdminLink>,
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
    version: String,
}

/// Whether `id` is a stand-in only its own machine knows: a stub for the
/// far end of a handoff, or an unsent send's.
fn machine_local(built: &Built, id: &str) -> bool {
    id.starts_with("unsent:") || built.sessions.get(id).is_some_and(|session| session.stub)
}

fn plan(parts: &[(&str, &Built)]) -> Plan {
    let mut machine_ids: Vec<String> = Vec::new();
    for (index, (key, built)) in parts.iter().enumerate() {
        let mut id = built.machine_id.clone();
        if machine_ids.contains(&id) {
            id = if key.is_empty() || machine_ids.contains(&format!("{id}~{key}")) {
                format!("{id}~{index}")
            } else {
                format!("{id}~{key}")
            };
        }
        machine_ids.push(id);
    }
    let mut owners = BTreeMap::new();
    let mut conflicts = BTreeSet::new();
    for (index, (_, built)) in parts.iter().enumerate() {
        let ids: BTreeSet<&String> = built.sessions.keys().chain(built.tx.keys()).collect();
        for id in ids {
            let served = if machine_local(built, id) {
                format!("{id}@{}", machine_ids[index])
            } else {
                id.clone()
            };
            if owners.insert(served.clone(), (index, id.clone())).is_some() {
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
                if let Some(fields) = session.as_object_mut() {
                    fields.insert("machine".into(), Value::from(machine_id.as_str()));
                }
                let served = rename(&id);
                if sessions.insert(served.clone(), session).is_some() {
                    conflicts.insert(served);
                }
            }
        }
        if let Some(Value::Array(own)) = model.remove("handoffs") {
            for mut handoff in own {
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
                tx.insert(rename(&id), mark);
            }
        }
    }
    if !conflicts.is_empty() {
        return Err(conflicts.into_iter().collect());
    }
    let first = machines.first().cloned().unwrap_or(Value::Null);
    let union = json!({
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

/// `body` (a model's JSON object) with the admin link as its first field.
fn with_admin(body: &[u8], admin: &AdminLink) -> Vec<u8> {
    let link = json!({ "label": admin.label, "href": admin.href }).to_string();
    let mut out = format!("{{\"admin\":{link},").into_bytes();
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
            admin: None,
        }
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
        if self.views.is_empty() {
            return text(404, "Not found");
        }
        if self.views.len() == 1 {
            let mut reply = self.views[0].1.respond(method, path, query, if_none_match);
            if path == "/api/model"
                && reply.status == 200
                && let Some(admin) = &self.admin
            {
                reply.body = with_admin(&reply.body, admin);
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
        for (_, view) in &mut self.views {
            view.built()?;
        }
        Ok(plan(&self.parts()))
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
                    body: match &self.admin {
                        Some(admin) => with_admin(&body, admin),
                        None => body,
                    },
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
        let plan = self.refresh()?;
        if plan.conflicts.contains(&sid) {
            return Ok(conflict(&[sid]));
        }
        let Some((index, own)) = plan.owners.get(&sid) else {
            return Ok(text(404, "Not found"));
        };
        let query = if *own == sid {
            query.to_owned()
        } else {
            with_param(query, "sid", own)
        };
        Ok(self.views[*index]
            .1
            .respond("GET", path, &query, if_none_match))
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
        Ok(match found.as_slice() {
            [index] => self.views[*index]
                .1
                .respond("GET", path, query, if_none_match),
            [] => text(404, "Not found"),
            _ => conflict(&[id]),
        })
    }

    /// The V1 tree: every machine's roots.
    fn tree(&mut self) -> io::Result<ViewerReply> {
        let mut roots: Vec<Node> = Vec::new();
        for (_, view) in &mut self.views {
            roots.extend(view.tree_roots()?);
        }
        Ok(ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: crate::render_json(&roots).into_bytes(),
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
        assert!(AdminLink::new("Manage machines", "/hub/machines").is_some());
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
        let link = AdminLink::new("Manage machines", "/hub/machines").unwrap();
        assert_eq!(
            with_admin(b"{\"version\":\"v\"}", &link),
            b"{\"admin\":{\"label\":\"Manage machines\",\"href\":\"/hub/machines\"},\"version\":\"v\"}"
        );
        assert_eq!(
            with_param("sid=a%40b&before=3", "sid", "a"),
            "sid=a&before=3"
        );
    }
}

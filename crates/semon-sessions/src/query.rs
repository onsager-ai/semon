//! The agent read surface (#14): `semon query <tool>` and `semon mcp` answer
//! the same tools, as JSON, from native session evidence. Hosted callers reuse
//! their admitted native source cache and configured output window.
//!
//! - **Facts, not verdicts.** Every field is something a log line, a process
//!   or a lock says. A field Semon can't know exactly is `null`, or the state
//!   `unknown`; it is never inferred. `stalls` is a convenience: it names its
//!   rule and gives closed reason codes, never prose.
//! - **One evidence generation.** Sessions, turns, handoffs, process facts and
//!   source-backed transcript offsets come from the same native build.
//! - **Read-only.** No listener and no network. Nothing is written but the
//!   metadata cache the viewer keeps too.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt, io,
    sync::Arc,
    time::Duration,
};

use serde_json::{Map, Value, json};

use crate::{
    Options,
    model::{SessionFacts, SlotKind, Transcript, now_ms, one_line},
    native_evidence::SessionSources,
    tx,
    union::{Owner, QueryRead},
};

/// A session's state: a closed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Running: a live Claude process whose status is `busy`, a subagent
    /// whose parent runs and whose spawning call has no result yet, or a
    /// Codex run that holds its writer lock.
    Working,
    /// An open `AskUserQuestion` or `request_user_input` waits on you.
    WaitingQuestion,
    /// Alive with nothing running, its last word said.
    Idle,
    Ended,
    /// The run failed, as its log records.
    Error,
    /// Not in these logs: the far end of a handoff.
    Unknown,
    // Q3 adds `waiting-permission` once an exact signal for a pending
    // permission prompt exists (#14's open question). Until then such a
    // session reads `working`.
}

impl State {
    const NAMES: [&'static str; 6] = [
        "working",
        "waiting-question",
        "idle",
        "ended",
        "error",
        "unknown",
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::WaitingQuestion => "waiting-question",
            Self::Idle => "idle",
            Self::Ended => "ended",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

/// Why `stalls` lists a session: a closed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StallReason {
    WaitingQuestion,
    NoOutput,
    ProcessGone,
    // Q3: `waiting-permission`, with the state.
}

impl StallReason {
    fn name(self) -> &'static str {
        match self {
            Self::WaitingQuestion => "waiting-question",
            Self::NoOutput => "no-output",
            Self::ProcessGone => "process-gone",
        }
    }
}

/// The rule `stalls` applies, as it states it in every answer.
const STALL_RULE: &str = "A session stalls when no log line has landed for at least idle_minutes and: \
     a Claude pid file recorded it busy but that process is gone (process-gone); or a question to you \
     is open (waiting-question); or it is working (no-output). Working is a live Claude process whose \
     status is busy, a subagent whose parent runs and whose spawning call has no result, or a Codex run \
     that holds its writer lock.";

const SINCE: &str = "A duration back from now (90m, 2h, 7d, 1w) or an RFC 3339 time; not before the \
     window's start (window_start), or the answer is the error outside_window.";

/// How far back `semon query` and `semon mcp` read by default.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Defaults and bounds.
const LIST_LIMIT: i64 = 100;
const LIST_MAX: i64 = 1000;
const FIND_LIMIT: i64 = 20;
const FIND_MAX: i64 = 200;
const FIND_BYTES: i64 = 64 * 1024 * 1024;
const FIND_BYTES_MAX: i64 = 1024 * 1024 * 1024;
/// A snippet shows this many bytes on each side of a match.
const CONTEXT: usize = 80;

/// A tool's failure: a closed `code` and a message for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError {
    /// `unknown_tool`, `invalid_arguments`, `unknown_session`,
    /// `unknown_turn`, `outside_window`, `id_conflict`, `capability_unavailable`,
    /// `stale_projection` or `io`.
    pub code: &'static str,
    pub message: String,
    /// With `outside_window`: where the window starts (epoch ms).
    pub window_start: Option<i64>,
    /// Closed physical-source diagnostic; contains no native path or content.
    pub local_revision_reason: Option<&'static str>,
    /// Closed proof/read failure phase, never an original provider error.
    pub stage: Option<&'static str>,
}

impl QueryError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            window_start: None,
            local_revision_reason: None,
            stage: None,
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_arguments", message)
    }

    fn unknown_session(id: &str) -> Self {
        Self::new(
            "unknown_session",
            format!("no session {id:?} in these logs"),
        )
    }

    pub(crate) fn io(error: &io::Error) -> Self {
        if let Some(stage) = crate::source_carrier::FailureStage::of(error) {
            return Self::source_failure(stage);
        }
        if let Some(revision) = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<crate::source_carrier::RevisionChanged>())
        {
            return Self::local_revision(revision.0);
        }
        Self::new("io", error.to_string())
    }

    pub(crate) fn source_failure(stage: crate::source_carrier::FailureStage) -> Self {
        let mut error = Self::new("io", "Original native transcript proofs are unavailable");
        error.stage = Some(stage.name());
        error
    }

    pub(crate) fn local_revision(reason: crate::source_carrier::RevisionReason) -> Self {
        let mut error = Self::new(
            "stale_projection",
            "The selected local source changed during this read; resynchronize.",
        );
        error.local_revision_reason = Some(reason.name());
        error
    }

    /// `{"error": {"code": …, "message": …}}`, and the `window_start` of
    /// an `outside_window`. A local-revision failure also carries the closed
    /// 409 source-fence diagnostics, without any partial tool result.
    pub fn to_json(&self) -> Value {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(start) = self.window_start {
            error["window_start"] = json!(start);
        }
        let mut value = json!({ "error": error });
        if let Some(stage) = self.stage {
            value["stage"] = json!(stage);
        }
        if let Some(reason) = self.local_revision_reason {
            value["status"] = json!(409);
            value["resynchronize"] = json!(true);
            value["fence_reason"] = json!("local_source_revision");
            value["local_revision_reason"] = json!(reason);
        }
        value
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for QueryError {}

/// One tool of the read surface, as `semon query` and MCP's `tools/list`
/// describe it.
#[derive(Clone, Debug)]
pub struct QueryTool {
    pub name: &'static str,
    pub description: &'static str,
    /// The argument `semon query <tool> VALUE` fills.
    pub positional: Option<&'static str>,
    /// The arguments, as JSON Schema.
    pub input_schema: Value,
}

fn since_property(what: &str) -> Value {
    json!({"type": "string", "description": format!("{what} {SINCE}")})
}

fn id_property(what: &str) -> Value {
    json!({"type": "string", "minLength": 1, "description": what})
}

/// Active collection tools, with their input schemas.
pub fn query_tools() -> Vec<QueryTool> {
    retained_query_tools()
        .into_iter()
        .filter(|tool| tool.name != "find")
        .collect()
}

// Preserve the optional matcher contract and independent semantic fixtures.
// Public dispatch must pass validate_query_call before preparing any evidence.
fn retained_query_tools() -> Vec<QueryTool> {
    vec![
        QueryTool {
            name: "list_sessions",
            description: "Session summaries, most recently active first: id, harness, kind, name, model, repo, \
                branch, machine, parent, children, state, start, last_activity (epoch ms), pid, alive, exit, \
                open_question, tokens, run and turns_truncated. A field Semon can't know exactly is null.",
            positional: None,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": {"type": "string", "enum": State::NAMES, "description": "Only sessions in this state."},
                    "repo": {"type": "string", "description": "Only sessions in this repository (its directory's name)."},
                    "since": since_property("Only sessions with a log line since then:"),
                    "parent": id_property("Only the sessions this session spawned."),
                    "harness": {"type": "string", "enum": ["claude", "codex"], "description": "Only this harness's sessions."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": LIST_MAX, "default": LIST_LIMIT, "description": "At most this many sessions."},
                },
                "additionalProperties": false,
            }),
        },
        QueryTool {
            name: "get_session",
            description: "One session: its summary, its busy intervals, its turns (each incoming message and how the \
                turn ended, with the handoffs it sent) and every handoff to or from it, as the session model has them.",
            positional: Some("id"),
            input_schema: json!({
                "type": "object",
                "properties": {"id": id_property("The session's id.")},
                "required": ["id"],
                "additionalProperties": false,
            }),
        },
        QueryTool {
            name: "read_transcript",
            description: "A page of one session's transcript, the same page the viewer's /api/tx serves: entries \
                with from and to (their positions) and total. With no anchor, the last page; before or after a \
                position; or the page starting at a turn. Tool input and output are previews.",
            positional: Some("id"),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": id_property("The session's id."),
                    "before": {"type": "integer", "minimum": 0, "description": "The page ending before this position (a page's from)."},
                    "after": {"type": "integer", "minimum": 0, "description": "The page starting at this position (a page's to)."},
                    "turn": {"type": "string", "minLength": 1, "description": "The page starting at this turn's first entry."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": tx::PAGE_ENTRIES, "default": tx::PAGE_ENTRIES, "description": "At most this many entries."},
                },
                "required": ["id"],
                "additionalProperties": false,
            }),
        },
        QueryTool {
            name: "find",
            description: "Entries whose text contains the given text (ASCII case-insensitive), newest first, with \
                the session, the entry's position (for read_transcript) and a snippet; and the sessions they are \
                in, or whose name, repo or branch contains it. Bounded by a result limit and a byte budget.",
            positional: Some("text"),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "minLength": 1, "maxLength": 1000, "description": "The text to find."},
                    "since": since_property("Only entries since then:"),
                    "limit": {"type": "integer", "minimum": 1, "maximum": FIND_MAX, "default": FIND_LIMIT, "description": "At most this many entries."},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": FIND_BYTES_MAX, "default": FIND_BYTES, "description": "Stop after reading this many bytes of logs."},
                },
                "required": ["text"],
                "additionalProperties": false,
            }),
        },
        QueryTool {
            name: "stalls",
            description: "Sessions with no log line for at least idle_minutes that are working, wait on an open \
                question, or whose process is gone while its record said busy, each with a closed stall_reason: \
                waiting-question, no-output or process-gone. The answer states its rule.",
            positional: None,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "idle_minutes": {"type": "integer", "minimum": 1, "maximum": 525_600, "description": "Minutes without a log line."},
                    "since": since_property("Only sessions with a log line since then:"),
                },
                "required": ["idle_minutes"],
                "additionalProperties": false,
            }),
        },
    ]
}

/// An integer argument: a JSON integer, or a number with no fraction.
fn as_integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|number| number.fract() == 0.0 && number.abs() < 9.0e15)
            .map(|number| number as i64)
    })
}

/// Checks `args` against a tool's schema: an object, no unknown or missing
/// argument, and each value of its type, enum and bounds. A `null` counts as
/// not given.
fn validate(schema: &Value, args: &Value) -> Result<(), QueryError> {
    let Some(args) = args.as_object() else {
        return Err(QueryError::invalid("the arguments must be a JSON object"));
    };
    let empty = Map::new();
    let properties = schema["properties"].as_object().unwrap_or(&empty);
    for required in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if args.get(required).is_none_or(Value::is_null) {
            return Err(QueryError::invalid(format!("{required} is required")));
        }
    }
    for (key, value) in args {
        let Some(property) = properties.get(key) else {
            return Err(QueryError::invalid(format!("unknown argument {key}")));
        };
        if value.is_null() {
            continue;
        }
        match property["type"].as_str() {
            Some("string") => {
                let Some(text) = value.as_str() else {
                    return Err(QueryError::invalid(format!("{key} must be a string")));
                };
                let length = text.chars().count() as u64;
                if property["minLength"]
                    .as_u64()
                    .is_some_and(|min| length < min)
                    || property["maxLength"]
                        .as_u64()
                        .is_some_and(|max| length > max)
                {
                    return Err(QueryError::invalid(format!("{key} has the wrong length")));
                }
                if let Some(allowed) = property["enum"].as_array()
                    && !allowed.iter().any(|option| option.as_str() == Some(text))
                {
                    let names: Vec<&str> = allowed.iter().filter_map(Value::as_str).collect();
                    return Err(QueryError::invalid(format!(
                        "{key} must be one of {}",
                        names.join(", ")
                    )));
                }
            }
            Some("integer") => {
                let Some(number) = as_integer(value) else {
                    return Err(QueryError::invalid(format!("{key} must be an integer")));
                };
                let min = property["minimum"].as_i64();
                let max = property["maximum"].as_i64();
                if min.is_some_and(|min| number < min) || max.is_some_and(|max| number > max) {
                    return Err(QueryError::invalid(format!(
                        "{key} must be between {} and {}",
                        min.map_or("-∞".into(), |min| min.to_string()),
                        max.map_or("∞".into(), |max| max.to_string())
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn string(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn integer(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(as_integer)
}

/// `since` as epoch ms: an RFC 3339 time, or a duration back from `now`.
fn since_ms(args: &Value, now: i64) -> Result<Option<i64>, QueryError> {
    let Some(value) = string(args, "since") else {
        return Ok(None);
    };
    if let Some(at) = crate::events::parse_ms(&value) {
        return Ok(Some(at));
    }
    crate::parse_duration(&value)
        .map(|duration| {
            Some(now.saturating_sub(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)))
        })
        .map_err(|_| {
            QueryError::invalid(format!(
                "since must be a duration such as 90m, 2h or 7d, or an RFC 3339 time; got {value:?}"
            ))
        })
}

/// The model as the tools read it: the served model's JSON, and each
/// session's facts, by the id the model serves it under.
struct View {
    now: i64,
    sessions: Vec<(String, Value)>,
    /// Each session's position in `sessions`, by id.
    index: HashMap<String, usize>,
    handoffs: Vec<Value>,
    turns: Vec<Value>,
    facts: BTreeMap<String, SessionFacts>,
    /// The latest open question each session asked.
    open: HashMap<String, Value>,
    children: BTreeMap<String, Vec<String>>,
}

enum ReadShape<'a> {
    Summaries,
    Session(&'a str),
    Search,
}

impl View {
    fn native(
        read: &QueryRead,
        now: i64,
        shape: ReadShape<'_>,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
    ) -> Result<Self, QueryError> {
        let mut conflicts = read.conflicts();
        let multiple = read.parts.len() > 1;
        let mut sessions = BTreeMap::new();
        let mut facts = BTreeMap::new();
        let mut handoffs = Vec::new();
        let mut turns = Vec::new();
        let mut handoff_ids = BTreeSet::new();
        let mut turn_ids = BTreeSet::new();
        for part in &read.parts {
            let records = part.data.sources.reader_with(provider);
            for (id, session) in &part.data.sessions {
                if part.dropped.contains(id) {
                    continue;
                }
                // The tools expose only these session fields. Pricing tables,
                // workspace transport and per-transcript growth marks stay out.
                let row = session_fields(session, part.machine.as_deref());
                sessions.insert(part.served(id), row);
            }
            for (id, own) in &part.data.facts {
                let mut own = own.clone();
                own.parent = own.parent.map(|parent| part.served(&parent));
                facts.insert(part.served(id), own);
            }
            for handoff in &part.data.handoffs {
                if part.dropped.contains(&handoff.from)
                    || handoff
                        .to
                        .as_ref()
                        .is_some_and(|to| part.dropped.contains(to))
                    || (part.droppable && handoff_ids.contains(&handoff.id))
                {
                    continue;
                }
                if !handoff_ids.insert(handoff.id.clone()) && multiple {
                    conflicts.insert(handoff.id.clone());
                }
                let question = handoff.kind == "toyou"
                    && handoff.ask == Some("question")
                    && handoff.status == "wait";
                let requested = match shape {
                    ReadShape::Session(id) => {
                        part.served(&handoff.from) == id
                            || handoff.to.as_ref().is_some_and(|to| part.served(to) == id)
                    }
                    ReadShape::Search => true,
                    ReadShape::Summaries => false,
                };
                if !requested && !question {
                    continue;
                }
                let mut handoff = handoff.clone();
                if matches!(shape, ReadShape::Session(_))
                    && let Some(brief) = part.data.ask_briefs.get(&handoff.id)
                {
                    handoff.brief = brief
                        .read(&records)
                        .map_err(|error| QueryError::io(&error))?;
                }
                handoff.from = part.served(&handoff.from);
                handoff.to = handoff.to.map(|to| part.served(&to));
                handoffs.push(json!(handoff));
            }
            for turn in &part.data.turns {
                if part.dropped.contains(&turn.sid)
                    || (part.droppable && turn_ids.contains(&turn.id))
                {
                    continue;
                }
                if !turn_ids.insert(turn.id.clone()) && multiple {
                    conflicts.insert(turn.id.clone());
                }
                if !matches!(shape, ReadShape::Session(id) if part.served(&turn.sid) == id) {
                    continue;
                }
                let mut turn = turn.clone();
                turn.sid = part.served(&turn.sid);
                turns.push(json!(turn));
            }
            records.finish().map_err(|error| QueryError::io(&error))?;
        }
        if !conflicts.is_empty() {
            return Err(QueryError::new(
                "id_conflict",
                format!(
                    "two machines both have {}",
                    conflicts.into_iter().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        Ok(Self::from_evidence(
            now,
            sessions.into_iter().collect(),
            handoffs,
            turns,
            facts,
        ))
    }

    fn from_evidence(
        now: i64,
        sessions: Vec<(String, Value)>,
        handoffs: Vec<Value>,
        turns: Vec<Value>,
        facts: BTreeMap<String, SessionFacts>,
    ) -> Self {
        let mut open = HashMap::new();
        for handoff in &handoffs {
            if handoff["kind"] == "toyou"
                && handoff["ask"] == "question"
                && handoff["status"] == "wait"
                && let Some(from) = handoff["from"].as_str()
            {
                open.insert(
                    from.to_owned(),
                    json!({"text": handoff["brief"], "asked_at": handoff["at"], "handoff": handoff["id"]}),
                );
            }
        }
        let index = sessions
            .iter()
            .enumerate()
            .map(|(position, (id, _))| (id.clone(), position))
            .collect();
        let mut children = BTreeMap::<String, Vec<String>>::new();
        for (id, _) in &sessions {
            if let Some(parent) = facts.get(id).and_then(|facts| facts.parent.clone()) {
                children.entry(parent).or_default().push(id.clone());
            }
        }
        Self {
            now,
            sessions,
            index,
            handoffs,
            turns,
            facts,
            open,
            children,
        }
    }

    fn session(&self, id: &str) -> Option<&Value> {
        self.index
            .get(id)
            .map(|position| &self.sessions[*position].1)
    }

    fn state(&self, id: &str, session: &Value) -> State {
        if self.open.contains_key(id) {
            return State::WaitingQuestion;
        }
        if self.facts.get(id).is_some_and(|facts| facts.kind == "stub") {
            return State::Unknown;
        }
        match session["state"].as_str() {
            Some("work") => State::Working,
            // A result waits on you to read it; nothing runs.
            Some("wait" | "idle") => State::Idle,
            Some("done") => State::Ended,
            Some("err") => State::Error,
            _ => State::Unknown,
        }
    }

    /// A session's summary: the fields #14 names, from the model and its
    /// facts.
    fn summary(&self, id: &str, session: &Value) -> Value {
        let facts = self.facts.get(id);
        let field = |key: &str| session.get(key).cloned().unwrap_or(Value::Null);
        let mut row = json!({
            "id": id,
            "harness": field("harness"),
            "kind": facts.map(|facts| facts.kind),
            "name": field("name"),
            "model": session["model"].as_str().filter(|model| *model != "—"),
            "repo": field("repo"),
            "branch": field("branch"),
            "machine": field("machine"),
            "parent": facts.and_then(|facts| facts.parent.clone()),
            "children": self.children.get(id).cloned().unwrap_or_default(),
            "state": self.state(id, session).name(),
            "start": facts.and_then(|facts| facts.first),
            "last_activity": facts.and_then(|facts| facts.last),
            "pid": facts.and_then(|facts| facts.pid),
            "alive": facts.and_then(|facts| facts.alive),
            // No log records a run's exit status: a Codex rollout's
            // `task_complete` error is a turn's outcome (the state `error`),
            // not a process's exit, and Claude's logs hold none. Never
            // inferred.
            "exit": Value::Null,
            "open_question": self.open.get(id).cloned().unwrap_or(Value::Null),
            "turns_truncated": facts.is_some_and(|facts| facts.turns_truncated),
            "tokens": facts.and_then(|facts| facts.tokens.as_ref()).map(|tokens| json!({
                "input": tokens.input.saturating_sub(tokens.cached_input),
                "cached": tokens.cached_input,
                "output": tokens.output,
            })),
            // The live process's run ids (RUN_VARIABLES only), or null.
            "run": facts.and_then(|facts| facts.run.clone()),
        });
        if let Some(evidence) = session.get("copilot") {
            row["copilot"] = evidence.clone();
        }
        if let Some(evidence) = session.get("claude_usage") {
            row["claude_usage"] = evidence.clone();
        }
        if let Some(signals) = session.get("signals") {
            row["signals"] = signals.clone();
        }
        row
    }

    fn last(&self, id: &str) -> Option<i64> {
        self.facts.get(id).and_then(|facts| facts.last)
    }
}

fn session_fields(session: &crate::model::Session, machine: Option<&str>) -> Value {
    let mut row = json!({"name":session.name,"harness":session.harness,
        "model":session.model,"repo":session.repo,"branch":session.branch,
        "state":session.state,"last":session.last,"machine":machine.unwrap_or(&session.machine),"busy":session.busy});
    if let Some(evidence) = &session.copilot {
        row["copilot"] = json!(evidence);
    }
    if let Some(evidence) = &session.claude_usage {
        row["claude_usage"] = json!(evidence);
    }
    if !session.signals.is_empty() {
        row["signals"] = json!(session.signals);
    }
    row
}

/// The native producer records exact five-tool facts alongside its existing
/// relationship header. Readers do not reconstruct those facts from rounded UI
/// totals or decode transcript event rows to list sessions.
pub(crate) fn catalog_summaries<'a>(
    sessions: impl Iterator<Item = (&'a str, &'a crate::model::Session)>,
    handoffs: &[crate::model::Handoff],
    facts: &BTreeMap<String, SessionFacts>,
    transcripts: &BTreeMap<String, Arc<Transcript>>,
    now: i64,
) -> BTreeMap<String, Value> {
    let sessions: Vec<_> = sessions
        .map(|(id, session)| {
            let mut fields = session_fields(session, None);
            if let Some(tx) = transcripts.get(id)
                && !tx.signals.is_empty()
            {
                fields["signals"] = json!(tx.signals);
            }
            (id.to_owned(), fields)
        })
        .collect();
    let view = View::from_evidence(
        now,
        sessions,
        handoffs.iter().map(|h| json!(h)).collect(),
        Vec::new(),
        facts.clone(),
    );
    view.sessions
        .iter()
        .map(|(id, session)| {
            let mut summary = view.summary(id, session);
            if !summary["open_question"].is_null() {
                summary["open_question"]["text"] = Value::Null;
            }
            (
                id.clone(),
                json!({"summary":summary,
                    "recorded_status":facts.get(id).and_then(|f| f.recorded_status.as_deref()),
                    "state":session["state"], "last":session["last"],
                }),
            )
        })
        .collect()
}

/// Orders summaries by their last log line, newest first, then by id.
fn newest_first(a: &Value, b: &Value) -> std::cmp::Ordering {
    b["last_activity"]
        .as_i64()
        .cmp(&a["last_activity"].as_i64())
        .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
}

/// Where `needle` (already ASCII-lowercase) first occurs in `text`, ASCII
/// case-insensitively. Byte offsets are the text's own.
fn find_in(text: &str, needle: &str) -> Option<usize> {
    text.to_ascii_lowercase().find(needle)
}

/// The text around a match, on one line.
fn snippet(text: &str, at: usize, length: usize) -> String {
    let mut start = at.saturating_sub(CONTEXT);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (at + length + CONTEXT).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&one_line(&text[start..end], 4 * CONTEXT));
    if end < text.len() {
        out.push('…');
    }
    out
}

/// Validate the active collection contract before admitting source/archive work.
/// This performs no I/O and grants no source authority. Window membership and
/// generation checks still belong to the actual Query call.
pub fn validate_query_call(tool: &str, args: &Value) -> Result<(), QueryError> {
    if tool == "find" {
        return Err(QueryError::new(
            "capability_unavailable",
            "Full-text find is paused in the collection configuration",
        ));
    }
    validate_tool_call(&query_tools(), tool, args)
}

fn validate_tool_call(tools: &[QueryTool], tool: &str, args: &Value) -> Result<(), QueryError> {
    let Some(spec) = tools.iter().find(|spec| spec.name == tool) else {
        let names: Vec<&str> = tools.iter().map(|spec| spec.name).collect();
        return Err(QueryError::new(
            "unknown_tool",
            format!("no tool {tool:?}; the tools are {}", names.join(", ")),
        ));
    };
    let empty = json!({});
    let args = if args.is_null() { &empty } else { args };
    validate(&spec.input_schema, args)?;
    since_ms(args, now_ms())?;
    Ok(())
}

/// Request-scoped native reader admission for a selected Catalog owner.
/// The embedding host retains authorization and cancellation policy.
pub type CatalogReaderFactory =
    dyn Fn(&str, &str) -> io::Result<Box<dyn crate::SessionSourceReader>>;

fn selected_error(error: &io::Error) -> QueryError {
    let terminal = crate::selected_read::check().err();
    let error = terminal.as_ref().unwrap_or(error);
    if matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        QueryError::new("unavailable", error.to_string())
    } else {
        QueryError::io(error)
    }
}

/// The agent read surface over one machine's homes or several: the tools
/// `semon query` and `semon mcp` answer. Local calls share the native source
/// index and snapshots between calls, rebuilding only what changed. Hosted
/// calls reuse admitted native sources and their configured output windows.
pub struct Query {
    sources: Arc<SessionSources>,
    /// Keeps embedding window-error text; source scope comes from the options.
    embedded: bool,
    /// The current call's window start (epoch ms), from its native cohort.
    start: Option<i64>,
    /// One native evidence cohort supplies the entire call.
    native: Option<QueryRead>,
}

mod catalog;
mod search;
mod transcript;

impl Query {
    /// The tools over the agent homes, `/proc` and cache `options` name.
    pub fn new(options: Options) -> Self {
        Self::with_machines(vec![(String::new(), options)])
    }

    /// The tools over several machines' native evidence, using the same
    /// identity and ownership rules as [`SessionSources::with_machines`].
    ///
    /// Each machine keeps its own window: its `since`, or every file with
    /// `all`. Only log files modified within it are read
    /// ([`Options::scan_window`]), and what is read is answered whole. Every
    /// answer names `window_start`, the latest of the machines' starts: a
    /// tool's `since` before it would be cut short on some machine, so it is
    /// refused.
    pub fn with_machines(machines: Vec<(String, Options)>) -> Self {
        let machines: Vec<_> = machines
            .into_iter()
            .map(|(key, options)| {
                (
                    key,
                    Options {
                        scan_window: true,
                        session: None,
                        ..options
                    },
                )
            })
            .collect();
        Self {
            sources: Arc::new(SessionSources::with_machines(machines)),
            embedded: false,
            start: None,
            native: None,
        }
    }

    /// Query admitted native sources with their configured windows unchanged.
    /// The host holds authorization and custody until the blocking call ends.
    /// No Viewer or background refresh producer is constructed here.
    pub fn from_sources(sources: Arc<SessionSources>) -> Self {
        Self {
            sources,
            embedded: true,
            start: None,
            native: None,
        }
    }

    /// The latest machine's output or scan window start, from this call's
    /// exact native source snapshots. `None` means every part is unbounded.
    fn window_start(&mut self, now: i64) -> Result<Option<i64>, QueryError> {
        let native = self
            .sources
            .query_at(now)
            .map_err(|error| QueryError::io(&error))?;
        let start = native
            .parts
            .iter()
            .filter_map(|part| part.data.window_start)
            .max();
        self.native = Some(native);
        Ok(start)
    }

    fn evidence(&self) -> &QueryRead {
        self.native
            .as_ref()
            .expect("call prepares one native evidence cohort")
    }

    /// A tool's `since`, which must not reach before the window's start.
    fn since(&self, args: &Value, now: i64) -> Result<Option<i64>, QueryError> {
        let parsed = since_ms(args, now)?;
        if let (Some(since), Some(start)) = (parsed, self.start)
            && since < start
        {
            return Err(QueryError {
                window_start: Some(start),
                ..QueryError::new(
                    "outside_window",
                    if self.embedded {
                        format!(
                            "since reaches before the cached model window, which starts at {start} (epoch ms)"
                        )
                    } else {
                        format!(
                            "since reaches before the window, which starts at {start} (epoch ms); widen it with --since or --all"
                        )
                    },
                )
            });
        }
        Ok(parsed)
    }

    /// Calls one tool with its arguments (a JSON object).
    pub fn call(&mut self, tool: &str, args: &Value) -> Result<Value, QueryError> {
        self.call_at(tool, args, now_ms())
    }

    /// [`Query::call`] at a fixed `now` (epoch ms), for fixtures.
    #[doc(hidden)]
    pub fn call_at(&mut self, tool: &str, args: &Value, now: i64) -> Result<Value, QueryError> {
        let empty = json!({});
        let args = if args.is_null() { &empty } else { args };
        validate_query_call(tool, args)?;
        self.execute_at(tool, args, now)
    }

    // Only unit fixtures can enter the retained optional search engine.
    #[cfg(test)]
    fn call_optional_at(
        &mut self,
        tool: &str,
        args: &Value,
        now: i64,
    ) -> Result<Value, QueryError> {
        let empty = json!({});
        let args = if args.is_null() { &empty } else { args };
        validate_tool_call(&retained_query_tools(), tool, args)?;
        self.execute_at(tool, args, now)
    }

    fn execute_at(&mut self, tool: &str, args: &Value, now: i64) -> Result<Value, QueryError> {
        if !self.embedded
            && self.sources.machines.len() == 1
            && !self.sources.is_retired()
            && matches!(tool, "get_session" | "read_transcript")
        {
            let id = args["id"].as_str().expect("validated focused id");
            let (source_key, evidence) = &self.sources.machines[0];
            if let Some(catalog_key) = evidence
                .local_query_target(id, now)
                .map_err(|error| QueryError::io(&error))?
            {
                let source_key = if source_key.is_empty() {
                    "local"
                } else {
                    source_key
                }
                .to_owned();
                return self.execute_selected_at(
                    &source_key,
                    &catalog_key,
                    tool,
                    args,
                    None,
                    crate::SessionSourceReadMode::LocalThenProvider,
                    now,
                );
            }
        }
        if !self.embedded
            && self.sources.machines.len() > 1
            && !self.sources.is_retired()
            && matches!(tool, "get_session" | "read_transcript")
        {
            let id = args["id"].as_str().expect("validated focused id");
            let mut selected = None;
            let mut ready = true;
            for (source_key, evidence) in &self.sources.machines {
                let key = if source_key.is_empty() {
                    "local"
                } else {
                    source_key
                };
                // Resolve ownership from every admitted source's published
                // Catalog. Do not decode another machine's event rows merely
                // to rule out a duplicate global session ID.
                match crate::catalog::qualified_history_identity(&evidence.options, key, id) {
                    Ok(Some(identity)) => {
                        let target = evidence
                            .local_query_target(&identity.catalog_key, now)
                            .map_err(|error| QueryError::io(&error))?;
                        if target.is_none() {
                            ready = false;
                            continue;
                        }
                        if selected.is_some() {
                            return Err(QueryError::new(
                                "id_conflict",
                                format!("two machines both have {id}"),
                            ));
                        }
                        selected = Some((key.to_owned(), identity.catalog_key));
                    }
                    Ok(None) => {}
                    Err(_) => ready = false,
                }
            }
            if ready && let Some((source_key, catalog_key)) = selected {
                return self.execute_selected_at(
                    &source_key,
                    &catalog_key,
                    tool,
                    args,
                    None,
                    crate::SessionSourceReadMode::LocalThenProvider,
                    now,
                );
            }
        }
        // Once per call: the answer's window and the check of `since` belong
        // to the same native evidence cohort.
        self.start = self.window_start(now)?;
        let mut answer = match tool {
            "list_sessions" => self.list_sessions(args, now, None),
            "get_session" => self.get_session(args, now, None),
            "read_transcript" => self.read_transcript(args, now, None),
            "find" => self.find(args, now, None),
            "stalls" => self.stalls(args, now, None),
            _ => unreachable!("every tool is handled"),
        }?;
        answer["window_start"] = json!(self.start);
        Ok(answer)
    }

    /// Execute a focused tool over one host-selected native Catalog identity.
    /// The host admits the source and lends a request-owned reader; Query never
    /// restores a workspace or retains its credentials. Listings and workspace
    /// search use their own native selection and cannot call this API.
    pub fn call_selected(
        &mut self,
        source_key: &str,
        catalog_key: &str,
        tool: &str,
        args: &Value,
        provider: Option<&dyn crate::SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
    ) -> Result<Value, QueryError> {
        self.call_selected_at(
            source_key,
            catalog_key,
            tool,
            args,
            provider,
            mode,
            now_ms(),
        )
    }

    /// Selected read with the host's whole-request deadline/cancellation.
    #[allow(clippy::too_many_arguments)]
    pub fn call_selected_with_control(
        &mut self,
        source_key: &str,
        catalog_key: &str,
        tool: &str,
        args: &Value,
        provider: Option<&dyn crate::SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
        control: &crate::SelectedReadControl,
    ) -> Result<Value, QueryError> {
        // Paused tools refuse before cancellation/admission, including stale callers.
        validate_query_call(tool, args)?;
        let _scope = crate::selected_read::Scope::enter(control);
        crate::selected_read::check().map_err(|error| selected_error(&error))?;
        let result = self.call_selected_at(
            source_key,
            catalog_key,
            tool,
            args,
            provider,
            mode,
            now_ms(),
        );
        crate::selected_read::check().map_err(|error| selected_error(&error))?;
        result
    }

    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn call_selected_at(
        &mut self,
        source_key: &str,
        catalog_key: &str,
        tool: &str,
        args: &Value,
        provider: Option<&dyn crate::SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
        now: i64,
    ) -> Result<Value, QueryError> {
        let empty = json!({});
        let args = if args.is_null() { &empty } else { args };
        validate_query_call(tool, args)?;
        if !matches!(tool, "get_session" | "read_transcript")
            || !args["id"].as_str().is_some_and(|id| {
                id == catalog_key
                    || id
                        .rsplit_once('@')
                        .is_some_and(|(own, _)| own == catalog_key)
            })
        {
            return Err(QueryError::invalid(
                "A focused tool requires the selected Catalog id",
            ));
        }
        self.execute_selected_at(source_key, catalog_key, tool, args, provider, mode, now)
    }

    // Public selected reads validate the exact host-selected Catalog ID above.
    // Ordinary local reads may resolve a raw native ID whose spelling changes
    // under the file window; their admitted native Catalogs resolve ownership
    // before entering here. Derivation still resolves args.id.
    #[allow(clippy::too_many_arguments)]
    fn execute_selected_at(
        &mut self,
        source_key: &str,
        catalog_key: &str,
        tool: &str,
        args: &Value,
        provider: Option<&dyn crate::SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
        now: i64,
    ) -> Result<Value, QueryError> {
        let _scope = crate::selected_read::Scope::ensure();
        crate::selected_read::check().map_err(|error| selected_error(&error))?;
        let evidence = self
            .sources
            .machines
            .iter()
            .find(|(key, _)| key == source_key || (key.is_empty() && source_key == "local"))
            .map(|(_, evidence)| evidence.clone())
            .ok_or_else(|| QueryError::unknown_session(catalog_key))?;
        if self.sources.is_retired() {
            return Err(QueryError::new("io", "native query source is closed"));
        }
        let mut focused_args = args.clone();
        if self.sources.machines.len() > 1 {
            let labels = self.source_labels();
            if let Some((_, label)) = labels
                .iter()
                .find(|(key, _)| key == source_key || (key.is_empty() && source_key == "local"))
            {
                focused_args["_machine_alias"] = json!(label);
            }
        }
        // One bounded request budget spans semantic derivation, transcript or
        // search bytes, and the final authorization/generation recheck.
        let bounded =
            provider.map(|reader| crate::source_reader::BudgetedReader::new(reader, 1024));
        let provider = bounded
            .as_ref()
            .map(|reader| reader as &dyn crate::SessionSourceReader);
        let rendering = provider.map(|reader| (reader, mode));
        if tool == "get_session"
            && let Some(mut answer) = transcript::session(
                &evidence.options,
                catalog_key,
                &focused_args,
                provider,
                mode,
                now,
            )?
        {
            if self.sources.is_retired() {
                return Err(QueryError::new("io", "native query source is closed"));
            }
            self.start = self
                .sources
                .machines
                .iter()
                .filter_map(|(_, source)| {
                    crate::model::QueryData::identity(&source.options, now).window_start
                })
                .max();
            if self.sources.machines.len() > 1 {
                let labels = self.source_labels();
                if let Some((_, label)) = labels
                    .iter()
                    .find(|(key, _)| key == source_key || (key.is_empty() && source_key == "local"))
                {
                    answer["session"]["machine"] = json!(label);
                }
            }
            answer["window_start"] = json!(self.start);
            return Ok(answer);
        }
        if tool == "read_transcript"
            && let Some(mut answer) = transcript::read(
                &evidence.options,
                catalog_key,
                &focused_args,
                provider,
                mode,
                now,
            )?
        {
            self.start = self
                .sources
                .machines
                .iter()
                .filter_map(|(_, source)| {
                    crate::model::QueryData::identity(&source.options, now).window_start
                })
                .max();
            if self.sources.is_retired() {
                return Err(QueryError::new("io", "native query source is closed"));
            }
            answer["window_start"] = json!(self.start);
            return Ok(answer);
        }
        let result = crate::overview_repair::with_query(
            &evidence.options,
            catalog_key,
            provider,
            mode,
            now,
            |data| {
                let data = Arc::new(data);
                let witness = data.sources.reader_with_mode(provider, mode);
                witness
                    .verify_all()
                    .map_err(|error| QueryError::io(&error))?;
                let parts = self
                    .sources
                    .machines
                    .iter()
                    .map(|(key, source)| {
                        let part = if Arc::ptr_eq(source, &evidence) {
                            data.clone()
                        } else {
                            Arc::new(crate::model::QueryData::identity(&source.options, now))
                        };
                        (key.clone(), part)
                    })
                    .collect();
                let native = QueryRead::new(parts, usize::MAX);
                self.start = native
                    .parts
                    .iter()
                    .filter_map(|part| part.data.window_start)
                    .max();
                self.native = Some(native);
                let answer = match tool {
                    "get_session" => self.get_session(args, now, rendering),
                    "read_transcript" => self.read_transcript(args, now, rendering),
                    _ => unreachable!("focused tools were checked"),
                }?;
                witness.finish().map_err(|error| QueryError::io(&error))?;
                Ok(answer)
            },
        )
        .map_err(|error| {
            let mut error = selected_error(&error);
            if error.stage.is_none() {
                error.stage = Some("observation");
            }
            error
        })?;
        let mut answer: Value = result?;
        crate::selected_read::check().map_err(|error| selected_error(&error))?;
        answer["window_start"] = json!(self.start);
        Ok(answer)
    }

    /// Configured presentation labels, including duplicate-hostname aliases.
    /// This reads machine identity metadata without preparing native sources.
    pub fn source_labels(&self) -> Vec<(String, String)> {
        let identities = self
            .sources
            .machines
            .iter()
            .map(|(key, source)| {
                (
                    key.clone(),
                    Arc::new(crate::model::QueryData::identity(&source.options, now_ms())),
                )
            })
            .collect();
        let read = QueryRead::new(identities, usize::MAX);
        self.sources
            .machines
            .iter()
            .zip(read.parts)
            .map(|((key, _), part)| {
                (
                    key.clone(),
                    part.machine.unwrap_or_else(|| part.data.machine_id.clone()),
                )
            })
            .collect()
    }

    fn view(
        &self,
        now: i64,
        shape: ReadShape<'_>,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
    ) -> Result<View, QueryError> {
        View::native(self.evidence(), now, shape, provider)
    }

    fn list_sessions(
        &mut self,
        args: &Value,
        now: i64,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
    ) -> Result<Value, QueryError> {
        let view = self.view(now, ReadShape::Summaries, provider)?;
        let since = self.since(args, now)?;
        let rows = view
            .sessions
            .iter()
            .map(|(id, session)| view.summary(id, session))
            .collect();
        catalog::list_response(rows, args, since, view.now)
    }

    fn get_session(
        &mut self,
        args: &Value,
        now: i64,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
    ) -> Result<Value, QueryError> {
        let id = string(args, "id").unwrap_or_default();
        let view = self.view(now, ReadShape::Session(&id), provider)?;
        let Some(session) = view.session(&id) else {
            return Err(QueryError::unknown_session(&id));
        };
        let mut out = view.summary(&id, session);
        out["busy"] = session.get("busy").cloned().unwrap_or_else(|| json!([]));
        out["turns"] = Value::Array(
            view.turns
                .iter()
                .filter(|turn| turn["sid"] == id.as_str())
                .cloned()
                .collect(),
        );
        out["handoffs"] = Value::Array(
            view.handoffs
                .iter()
                .filter(|handoff| handoff["from"] == id.as_str() || handoff["to"] == id.as_str())
                .cloned()
                .collect(),
        );
        Ok(json!({"now": view.now, "session": out}))
    }

    fn read_transcript(
        &mut self,
        args: &Value,
        now: i64,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
    ) -> Result<Value, QueryError> {
        let id = string(args, "id").unwrap_or_default();
        let position = |key: &str| integer(args, key).map(|value| value.max(0) as usize);
        let anchor = tx::Anchor::of(position("before"), position("after"), string(args, "turn"))
            .ok_or_else(|| QueryError::invalid("before, after and turn are exclusive"))?;
        let limit = integer(args, "limit").map_or(tx::PAGE_ENTRIES, |limit| limit as usize);
        let owner = self.evidence().owner(&id);
        let (index, own) = match owner {
            Owner::At(index, own) => (index, own),
            Owner::Missing => return Err(QueryError::unknown_session(&id)),
            Owner::Conflict => {
                return Err(QueryError::new(
                    "id_conflict",
                    format!("two machines both have {id:?}"),
                ));
            }
        };
        let data = &self.evidence().parts[index].data;
        let transcript = data
            .tx
            .get(&own)
            .ok_or_else(|| QueryError::unknown_session(&id))?;
        let records = data.sources.reader_with(provider);
        let mut read = |path: &std::path::Path, offset| records.read(path, offset);
        let mut prompts = |file: &crate::model::SlotFile, offset| records.prompt(file, offset);
        let page = tx::page_from_lines(
            &tx::RenderContext {
                home: data.home.as_deref(),
                harness: data.sessions.get(&own).map(|session| session.harness),
                bounded: false,
            },
            transcript,
            &own,
            &anchor,
            now,
            limit,
            &mut tx::Lines::provider_with_prompts(&mut read, &mut prompts),
        )
        .map_err(|error| match (&anchor, error.kind()) {
            (tx::Anchor::Turn(turn), io::ErrorKind::NotFound) => QueryError::new(
                "unknown_turn",
                format!("session {id:?} has no turn {turn:?}"),
            ),
            _ => QueryError::io(&error),
        })?;
        records.finish().map_err(|error| QueryError::io(&error))?;
        serde_json::from_str(&page).map_err(|error| QueryError::new("io", error.to_string()))
    }

    fn find(
        &mut self,
        args: &Value,
        now: i64,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
    ) -> Result<Value, QueryError> {
        let since = self.since(args, now)?;
        let view = self.view(now, ReadShape::Search, provider)?;
        let rows = view
            .sessions
            .iter()
            .map(|(id, row)| {
                let mut row = row.clone();
                row["id"] = json!(id);
                row["kind"] = json!(view.facts.get(id).map(|facts| &facts.kind));
                row["last_activity"] = json!(view.last(id));
                (id.clone(), row)
            })
            .collect();
        let mut search = search::Search::new(args, since, rows);
        let transcripts: BTreeMap<_, _> = self
            .evidence()
            .parts
            .iter()
            .enumerate()
            .flat_map(|(index, part)| {
                part.data
                    .tx
                    .iter()
                    .map(move |(own, tx)| (part.served(own), (index, tx)))
            })
            .collect();
        for served in search.order() {
            let Some((index, transcript)) = transcripts.get(&served) else {
                continue;
            };
            let data = &self.evidence().parts[*index].data;
            let records = data.sources.reader_with(provider);
            search.scan(&served, transcript, data, &view.handoffs, &records)?;
            records.finish().map_err(|error| QueryError::io(&error))?;
            if search.stopped() {
                break;
            }
        }
        Ok(search.finish(now))
    }

    fn stalls(
        &mut self,
        args: &Value,
        now: i64,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
    ) -> Result<Value, QueryError> {
        let view = self.view(now, ReadShape::Summaries, provider)?;
        let since = self.since(args, now)?;
        let rows = view
            .sessions
            .iter()
            .filter_map(|(id, session)| {
                let facts = view.facts.get(id)?;
                Some((view.summary(id, session), facts.recorded_status.clone()))
            })
            .collect();
        Ok(catalog::stall_response(rows, args, since, view.now))
    }
}

#[cfg(test)]
mod tests;

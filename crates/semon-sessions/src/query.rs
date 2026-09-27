//! The agent read surface (#14): `semon query <tool>` and `semon mcp` answer
//! the same tools, as JSON, from the session model `--model-json` and
//! `/api/model` serve, so a field means the same thing in all of them.
//!
//! - **Facts, not verdicts.** Every field is something a log line, a process
//!   or a lock says. A field Semon can't know exactly is `null`, or the state
//!   `unknown`; it is never inferred. `stalls` is a convenience: it names its
//!   rule and gives closed reason codes, never prose.
//! - **One model.** Sessions, turns and handoffs are the model's; the few
//!   facts the model holds but doesn't serve (a session's process, its parent
//!   and exact token counts) come from the same build.
//! - **Read-only.** No listener and no network. Nothing is written but the
//!   metadata cache the viewer keeps too.

use std::{
    collections::{BTreeMap, HashMap},
    fmt, io,
    time::Duration,
};

use serde_json::{Map, Value, json};

use crate::{
    Options,
    model::{SessionFacts, SlotKind, now_ms, one_line},
    tx,
    union::{Owner, ViewerCore},
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
    /// `unknown_turn`, `outside_window`, `id_conflict` or `io`.
    pub code: &'static str,
    pub message: String,
    /// With `outside_window`: where the window starts (epoch ms).
    pub window_start: Option<i64>,
}

impl QueryError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            window_start: None,
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

    fn io(error: &io::Error) -> Self {
        Self::new("io", error.to_string())
    }

    /// `{"error": {"code": …, "message": …}}`, and the `window_start` of
    /// an `outside_window`.
    pub fn to_json(&self) -> Value {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(start) = self.window_start {
            error["window_start"] = json!(start);
        }
        json!({ "error": error })
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

/// Every tool, with its input schema.
pub fn query_tools() -> Vec<QueryTool> {
    vec![
        QueryTool {
            name: "list_sessions",
            description: "Session summaries, most recently active first: id, harness, kind, name, model, repo, \
                branch, machine, parent, children, state, start, last_activity (epoch ms), pid, alive, exit, \
                open_question, tokens and run. A field Semon can't know exactly is null.",
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

impl View {
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
        json!({
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
            // Q3: a Codex rollout's recorded exit status, and Claude's if its logs hold one.
            "exit": Value::Null,
            "open_question": self.open.get(id).cloned().unwrap_or(Value::Null),
            "tokens": facts.and_then(|facts| facts.tokens.as_ref()).map(|tokens| json!({
                "input": tokens.input.saturating_sub(tokens.cached_input),
                "cached": tokens.cached_input,
                "output": tokens.output,
            })),
            // Q3: orchestrator ids from an allowlisted set of environment variables.
            "run": Value::Null,
        })
    }

    fn last(&self, id: &str) -> Option<i64> {
        self.facts.get(id).and_then(|facts| facts.last)
    }
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

/// The agent read surface over one machine's homes or several: the tools
/// `semon query` and `semon mcp` answer. It keeps the viewer's model and
/// caches between calls, rebuilding only what changed.
pub struct Query {
    core: ViewerCore,
    /// How far back the model reads; `None` for the whole history.
    window: Option<Duration>,
}

impl Query {
    /// The tools over the agent homes, `/proc` and cache `options` name.
    pub fn new(options: Options) -> Self {
        Self::with_machines(vec![(String::new(), options)])
    }

    /// The tools over several machines' homes, as one model: the union
    /// [`ViewerCore::with_machines`] serves.
    ///
    /// The window is the first machine's `since`, or the whole history with
    /// `all`: only log files modified within it are read
    /// ([`Options::scan_window`]), and every answer names its
    /// `window_start`. A tool's own `since` is a filter inside it.
    pub fn with_machines(machines: Vec<(String, Options)>) -> Self {
        let window = machines
            .first()
            .filter(|(_, options)| !options.all)
            .map(|(_, options)| options.since);
        let machines = machines
            .into_iter()
            .map(|(key, options)| {
                (
                    key,
                    Options {
                        all: window.is_none(),
                        since: window.unwrap_or(options.since),
                        scan_window: true,
                        session: None,
                        ..options
                    },
                )
            })
            .collect();
        Self {
            core: ViewerCore::with_machines(machines),
            window,
        }
    }

    /// Where the window starts at `now` (epoch ms); `None` without one.
    fn window_start(&self, now: i64) -> Option<i64> {
        self.window
            .map(|window| now.saturating_sub(i64::try_from(window.as_millis()).unwrap_or(i64::MAX)))
    }

    /// A tool's `since`, which must not reach before the window's start.
    fn since(&self, args: &Value, now: i64) -> Result<Option<i64>, QueryError> {
        let parsed = since_ms(args, now)?;
        if let (Some(since), Some(start)) = (parsed, self.window_start(now))
            && since < start
        {
            return Err(QueryError {
                window_start: Some(start),
                ..QueryError::new(
                    "outside_window",
                    format!(
                        "since reaches before the window, which starts at {start} (epoch ms); \
                         widen it with --since or --all"
                    ),
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
        let tools = query_tools();
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
        let mut answer = match tool {
            "list_sessions" => self.list_sessions(args, now),
            "get_session" => self.get_session(args, now),
            "read_transcript" => self.read_transcript(args, now),
            "find" => self.find(args, now),
            "stalls" => self.stalls(args, now),
            _ => unreachable!("every tool is handled"),
        }?;
        answer["window_start"] = json!(self.window_start(now));
        Ok(answer)
    }

    fn view(&mut self, now: i64) -> Result<View, QueryError> {
        let model = match self
            .core
            .model_at(now)
            .map_err(|error| QueryError::io(&error))?
        {
            Ok(model) => model,
            Err(ids) => {
                return Err(QueryError::new(
                    "id_conflict",
                    format!("two machines both have {}", ids.join(", ")),
                ));
            }
        };
        let mut model: Value = serde_json::from_str(&model)
            .map_err(|error| QueryError::new("io", error.to_string()))?;
        let mut facts = BTreeMap::new();
        for part in self.core.served().map_err(|error| QueryError::io(&error))? {
            for (id, own) in &part.built.facts {
                let mut own = own.clone();
                own.parent = own.parent.map(|parent| part.served(&parent));
                facts.insert(part.served(id), own);
            }
        }
        let sessions: Vec<(String, Value)> = match model["sessions"].take() {
            Value::Object(sessions) => sessions.into_iter().collect(),
            _ => Vec::new(),
        };
        let handoffs = match model["handoffs"].take() {
            Value::Array(handoffs) => handoffs,
            _ => Vec::new(),
        };
        let turns = match model["turns"].take() {
            Value::Array(turns) => turns,
            _ => Vec::new(),
        };
        let mut open = HashMap::new();
        for handoff in &handoffs {
            if handoff["kind"] == "toyou"
                && handoff["ask"] == "question"
                && handoff["status"] == "wait"
                && let Some(from) = handoff["from"].as_str()
            {
                // Handoffs are in time order: the latest open question wins.
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
        Ok(View {
            now,
            sessions,
            index,
            handoffs,
            turns,
            facts,
            open,
            children,
        })
    }

    fn list_sessions(&mut self, args: &Value, now: i64) -> Result<Value, QueryError> {
        let view = self.view(now)?;
        let since = self.since(args, now)?;
        let state = string(args, "state");
        let repo = string(args, "repo");
        let parent = string(args, "parent");
        let harness = string(args, "harness");
        let limit = integer(args, "limit").unwrap_or(LIST_LIMIT) as usize;
        if let Some(parent) = &parent
            && view.session(parent).is_none()
        {
            return Err(QueryError::unknown_session(parent));
        }
        let mut rows: Vec<Value> = view
            .sessions
            .iter()
            .map(|(id, session)| view.summary(id, session))
            .filter(|row| {
                state.as_deref().is_none_or(|state| row["state"] == state)
                    && repo.as_deref().is_none_or(|repo| row["repo"] == repo)
                    && parent
                        .as_deref()
                        .is_none_or(|parent| row["parent"] == parent)
                    && harness
                        .as_deref()
                        .is_none_or(|harness| row["harness"] == harness)
                    && since.is_none_or(|since| {
                        row["last_activity"]
                            .as_i64()
                            .is_some_and(|last| last >= since)
                    })
            })
            .collect();
        rows.sort_by(newest_first);
        let total = rows.len();
        rows.truncate(limit);
        Ok(json!({
            "now": view.now,
            "total": total,
            "truncated": total > rows.len(),
            "sessions": rows,
        }))
    }

    fn get_session(&mut self, args: &Value, now: i64) -> Result<Value, QueryError> {
        let view = self.view(now)?;
        let id = string(args, "id").unwrap_or_default();
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

    fn read_transcript(&mut self, args: &Value, now: i64) -> Result<Value, QueryError> {
        let id = string(args, "id").unwrap_or_default();
        let position = |key: &str| integer(args, key).map(|value| value.max(0) as usize);
        let anchor = tx::Anchor::of(position("before"), position("after"), string(args, "turn"))
            .ok_or_else(|| QueryError::invalid("before, after and turn are exclusive"))?;
        let limit = integer(args, "limit").map_or(tx::PAGE_ENTRIES, |limit| limit as usize);
        let own = match self
            .core
            .owner(&id)
            .map_err(|error| QueryError::io(&error))?
        {
            Owner::At(index, own) => (index, own),
            Owner::Missing => return Err(QueryError::unknown_session(&id)),
            Owner::Conflict => {
                return Err(QueryError::new(
                    "id_conflict",
                    format!("two machines both have {id:?}"),
                ));
            }
        };
        let built = self
            .core
            .built_at(own.0)
            .map_err(|error| QueryError::io(&error))?;
        if !built.tx.contains_key(&own.1) {
            return Err(QueryError::unknown_session(&id));
        }
        let page =
            tx::page_limited(built, &own.1, &anchor, now, limit).map_err(|error| {
                match (&anchor, error.kind()) {
                    (tx::Anchor::Turn(turn), io::ErrorKind::NotFound) => QueryError::new(
                        "unknown_turn",
                        format!("session {id:?} has no turn {turn:?}"),
                    ),
                    _ => QueryError::io(&error),
                }
            })?;
        serde_json::from_str(&page).map_err(|error| QueryError::new("io", error.to_string()))
    }

    fn find(&mut self, args: &Value, now: i64) -> Result<Value, QueryError> {
        let text = string(args, "text").unwrap_or_default();
        let needle = text.to_ascii_lowercase();
        let since = self.since(args, now)?;
        let limit = integer(args, "limit").unwrap_or(FIND_LIMIT) as usize;
        let max_bytes = integer(args, "max_bytes").unwrap_or(FIND_BYTES) as u64;
        let view = self.view(now)?;
        let handoffs: HashMap<&str, &Value> = view
            .handoffs
            .iter()
            .filter_map(|handoff| handoff["id"].as_str().map(|id| (id, handoff)))
            .collect();
        let recent =
            |id: &str| since.is_none_or(|since| view.last(id).is_some_and(|last| last >= since));
        // Sessions whose name, repo or branch holds the text.
        let mut sessions: Vec<(String, Vec<&str>, usize)> = Vec::new();
        for (id, session) in &view.sessions {
            if view.facts.get(id).is_some_and(|facts| facts.kind == "stub") || !recent(id) {
                continue;
            }
            let fields: Vec<&str> = ["name", "repo", "branch"]
                .into_iter()
                .filter(|field| {
                    session[*field]
                        .as_str()
                        .is_some_and(|value| find_in(value, &needle).is_some())
                })
                .collect();
            if !fields.is_empty() {
                sessions.push((id.clone(), fields, 0));
            }
        }
        // Every session's transcript, the most recently active first, each
        // from its newest entry back. Stubs have no activity of their own.
        let parts = self.core.served().map_err(|error| QueryError::io(&error))?;
        let mut order: Vec<(Option<i64>, String, usize, &str)> = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            for own in part.built.tx.keys() {
                let served = part.served(own);
                if view.session(&served).is_none()
                    || view
                        .facts
                        .get(&served)
                        .is_some_and(|facts| facts.kind == "stub")
                    || !recent(&served)
                {
                    continue;
                }
                order.push((view.last(&served), served, index, own.as_str()));
            }
        }
        order.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut lines = tx::Lines::default();
        let mut matches = Vec::new();
        let mut stopped = None;
        'sessions: for (_, served, index, own) in &order {
            let built = parts[*index].built;
            let Some(transcript) = built.tx.get(*own) else {
                continue;
            };
            for (position, slot) in transcript.slots.iter().enumerate().rev() {
                if since.is_some_and(|since| !slot.t.is_some_and(|t| t >= since)) {
                    continue;
                }
                if matches.len() >= limit {
                    stopped = Some("limit");
                    break 'sessions;
                }
                if lines.bytes >= max_bytes {
                    stopped = Some("bytes");
                    break 'sessions;
                }
                let (kind, texts) = match &slot.kind {
                    SlotKind::H(id) => {
                        let handoff: Option<&Value> = handoffs.get(id.as_str()).copied();
                        let mut texts = Vec::new();
                        for part in ["brief", "result"] {
                            if let Some(text) = handoff.and_then(|handoff| handoff[part].as_str()) {
                                texts.push((part, text.to_owned()));
                            }
                        }
                        if let Some(answers) =
                            handoff.and_then(|handoff| handoff["answer"].as_array())
                        {
                            let answer: Vec<&str> =
                                answers.iter().filter_map(Value::as_str).collect();
                            texts.push(("answer", answer.join("\n")));
                        }
                        ("h", texts)
                    }
                    SlotKind::U => ("u", tx::slot_texts(built, &mut lines, slot)),
                    SlotKind::A => ("a", tx::slot_texts(built, &mut lines, slot)),
                    SlotKind::Think => ("think", tx::slot_texts(built, &mut lines, slot)),
                    SlotKind::Tool { .. } => ("tool", tx::slot_texts(built, &mut lines, slot)),
                    _ => continue,
                };
                let Some((part, text, at)) = texts
                    .iter()
                    .find_map(|(part, text)| find_in(text, &needle).map(|at| (*part, text, at)))
                else {
                    continue;
                };
                let mut entry = json!({
                    "session": served,
                    "position": position,
                    "turn": slot.turn,
                    "kind": kind,
                    "part": part,
                    "at": slot.t,
                    "snippet": snippet(text, at, needle.len()),
                });
                match &slot.kind {
                    SlotKind::H(id) => entry["handoff"] = json!(id),
                    SlotKind::Tool { name, .. } => entry["name"] = json!(name),
                    _ => {}
                }
                matches.push(entry);
                match sessions.iter_mut().find(|(id, _, _)| id == served) {
                    Some((_, _, count)) => *count += 1,
                    None => sessions.push((served.clone(), Vec::new(), 1)),
                }
            }
        }
        let mut sessions: Vec<Value> = sessions
            .into_iter()
            .filter_map(|(id, fields, count)| {
                let session = view.session(&id)?;
                Some(json!({
                    "id": id,
                    "name": session["name"],
                    "repo": session.get("repo").cloned().unwrap_or(Value::Null),
                    "last_activity": view.last(&id),
                    "fields": fields,
                    "entries": count,
                }))
            })
            .collect();
        sessions.sort_by(newest_first);
        Ok(json!({
            "now": view.now,
            "text": text,
            "matches": matches,
            "sessions": sessions,
            "scanned_bytes": lines.bytes,
            "truncated": stopped.is_some(),
            "truncated_by": stopped,
        }))
    }

    fn stalls(&mut self, args: &Value, now: i64) -> Result<Value, QueryError> {
        let view = self.view(now)?;
        let idle_minutes = integer(args, "idle_minutes").unwrap_or(1);
        let since = self.since(args, now)?;
        let threshold = idle_minutes.saturating_mul(60_000);
        let mut rows = Vec::new();
        for (id, session) in &view.sessions {
            let Some(facts) = view.facts.get(id) else {
                continue;
            };
            // Without a log line, how long it has been quiet isn't known.
            let Some(last) = facts.last else {
                continue;
            };
            if since.is_some_and(|since| last < since) || now.saturating_sub(last) < threshold {
                continue;
            }
            let reason =
                if facts.alive == Some(false) && facts.recorded_status.as_deref() == Some("busy") {
                    StallReason::ProcessGone
                } else {
                    match view.state(id, session) {
                        State::WaitingQuestion => StallReason::WaitingQuestion,
                        State::Working => StallReason::NoOutput,
                        _ => continue,
                    }
                };
            let mut row = view.summary(id, session);
            row["stall_reason"] = json!(reason.name());
            row["idle_ms"] = json!(now.saturating_sub(last));
            rows.push(row);
        }
        // The longest quiet first.
        rows.sort_by(|a, b| {
            a["last_activity"]
                .as_i64()
                .cmp(&b["last_activity"].as_i64())
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        Ok(json!({
            "now": view.now,
            "idle_minutes": idle_minutes,
            "rule": STALL_RULE,
            "stalls": rows,
        }))
    }
}

#[cfg(test)]
mod tests;

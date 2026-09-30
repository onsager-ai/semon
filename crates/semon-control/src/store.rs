//! The request store: the pending requests, their transitions and caps, and
//! the driver's answer operation.
//!
//! All state sits behind one lock. An answer is checked and claimed under it
//! (409 before 412), delivered by the [`Adapter`] outside it, and ended under
//! it again, so the first claim wins and a slow delivery blocks nobody.

use std::{
    collections::HashSet,
    sync::{Mutex, MutexGuard, PoisonError},
};

use serde_json::{Value, json};

use crate::{
    canonical,
    journal::Journal,
    request::{
        Answer, Answerable, Kind, LeftReason, MatchKey, NewRequest, PendingRequest, RequestId,
        ResolvedReason, Source, State, single_choice_questions,
    },
};

/// At most this many requests are open or claimed at a time.
pub const MAX_OPEN: usize = 256;
/// At most this many Claude hook connections wait at a time.
pub const MAX_HOOK_WAITS: usize = 64;
/// At most this many Claude hook connections wait for one session.
pub const MAX_HOOK_WAITS_PER_SESSION: usize = 4;
/// A payload whose canonical encoding is larger is not registered.
pub const MAX_PAYLOAD_BYTES: usize = 1 << 20;
/// A session key, tool name or harness reference longer than this is not
/// registered.
pub const MAX_ID_BYTES: usize = 256;
/// At most this many final requests are kept; the oldest go first.
pub const MAX_RETAINED_FINAL: usize = 1024;
/// A final request is kept this long after it ended.
pub const RETAIN_FINAL_MS: u64 = 10 * 60 * 1000;
/// A claim that hasn't ended this long after it was made becomes `left`,
/// delivery unknown.
pub const DELIVERY_DEADLINE_MS: u64 = 10_000;
/// Refused registrations for one session are journalled at most once per
/// this window; the next line carries the count suppressed meanwhile.
pub const REFUSAL_WINDOW_MS: u64 = 60_000;
/// At most this many sessions' refusal windows are tracked; the oldest is
/// forgotten first.
const MAX_REFUSAL_WINDOWS: usize = 64;
/// A session key in a registration-refusal line is cut to this many bytes.
const JOURNAL_SESSION_BYTES: usize = 64;

/// How delivering an answer to the harness ended, as far as it lets Semon
/// see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Claude: the decision was written to the hook's connection.
    Written,
    /// The harness's side was already closed; the request ended elsewhere.
    /// Claude: [`ResolvedReason::DeniedOrInterruptedInTerminal`].
    Closed(ResolvedReason),
    /// Codex: sent; the claim ends when the harness confirms it
    /// ([`RequestStore::confirm_delivery`]) or at the delivery deadline.
    AwaitConfirmation,
    /// OpenCode: the reply route's HTTP status. 2xx is `answered`, anything
    /// else `rejected`.
    Status(u16),
}

/// Delivers an answer to one harness. Called outside the store's lock, with
/// the request already claimed.
pub trait Adapter {
    /// Delivers `answer` for `request` and says how that ended.
    fn deliver(&self, request: &PendingRequest, answer: &Answer) -> Delivery;
}

/// Why a request was not registered. The hook gets no decision at once, the
/// terminal asks, and the viewer says requests were dropped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    /// [`MAX_OPEN`] requests are already open or claimed.
    #[error("too many open requests")]
    TooManyOpen,
    /// [`MAX_HOOK_WAITS`] hook connections already wait.
    #[error("too many waiting hook connections")]
    TooManyHookWaits,
    /// [`MAX_HOOK_WAITS_PER_SESSION`] hook connections already wait for this
    /// session.
    #[error("too many waiting hook connections for this session")]
    TooManyHookWaitsForSession,
    /// The payload's canonical encoding is over [`MAX_PAYLOAD_BYTES`].
    #[error("the payload is too large")]
    PayloadTooLarge,
    /// The session key, tool name or harness reference is over
    /// [`MAX_ID_BYTES`].
    #[error("the session key, tool name or harness reference is too long")]
    IdTooLong,
    /// No random id could be made.
    #[error("no random id: {0}")]
    Randomness(String),
}

/// Why an answer was refused. Checked in this order.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AnswerError {
    /// No such request (404).
    #[error("no such request")]
    UnknownId,
    /// The request is not open (409), with its current state. This comes
    /// before a hash mismatch.
    #[error("the request is {}", .0.name())]
    NotOpen(State),
    /// The request is read-only (409), with the reason.
    #[error("the request can't be answered here: {0}")]
    ReadOnly(String),
    /// The answer's payload hash is not the request's (412), with the
    /// current payload.
    #[error("the request's payload has changed")]
    HashMismatch {
        /// The payload the request holds now.
        payload: Box<Value>,
    },
    /// The answer doesn't fit the request (400): an allow or deny for a
    /// question, or answers whose questions aren't exactly the request's or
    /// whose labels aren't among its options.
    #[error("the answer doesn't fit the request")]
    WrongShape,
}

impl AnswerError {
    /// The HTTP status the answer route gives this refusal.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::UnknownId => 404,
            Self::NotOpen(_) | Self::ReadOnly(_) => 409,
            Self::HashMismatch { .. } => 412,
            Self::WrongShape => 400,
        }
    }
}

/// What a tool-run report did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRun {
    /// It matched an open request, which is now `resolved`, allowed in the
    /// terminal. The adapter tells that request's hook to exit.
    ResolvedOpen(RequestId),
    /// It matched a request the viewer denied: the terminal allowed it first.
    AfterDeny(RequestId),
    /// It matched a request and only marked it.
    Marked(RequestId),
    /// No request matched.
    Unmatched,
}

/// The store's contents at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Open and claimed requests, and final ones still retained, oldest
    /// first.
    pub requests: Vec<PendingRequest>,
    /// Registrations refused by a cap since the store started.
    pub dropped: u64,
    /// Journal lines that couldn't be written since the store started.
    pub journal_failures: u64,
}

/// The pending requests of one controlling process.
#[derive(Debug)]
pub struct RequestStore {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// In registration order, so the first match is the oldest.
    requests: Vec<PendingRequest>,
    journal: Journal,
    dropped: u64,
    journal_failures: u64,
    refusal_windows: Vec<RefusalWindow>,
}

/// One session's current window of journalled registration refusals.
#[derive(Debug)]
struct RefusalWindow {
    session: String,
    started_ms: u64,
    suppressed: u64,
}

impl RequestStore {
    /// An empty store that journals to `journal`.
    pub fn new(journal: Journal) -> Self {
        Self {
            inner: Mutex::new(Inner {
                requests: Vec::new(),
                journal,
                dropped: 0,
                journal_failures: 0,
                refusal_windows: Vec::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a request, or refuses it for a cap. A payload with no
    /// canonical encoding is registered read-only.
    pub fn register(&self, new: NewRequest, now_ms: u64) -> Result<RequestId, RegisterError> {
        let hashed = hash_payload(&new);
        let id = RequestId::random().map_err(|error| RegisterError::Randomness(error.to_string()));
        let mut inner = self.lock();
        inner.expire(now_ms);
        let admitted = hashed.and_then(|hashed| {
            inner.check_caps(&new)?;
            Ok((id?, hashed))
        });
        let result = match admitted {
            Ok((id, (payload_sha256, problem))) => {
                if let Some(key) = &new.match_key {
                    inner.supersede(&new.session, key);
                }
                let question_problem = if new.kind == Kind::Question {
                    single_choice_questions(&new.payload).err()
                } else {
                    None
                };
                let answerable = match new.read_only.or(problem).or(question_problem) {
                    Some(reason) => Answerable::No(reason),
                    None => Answerable::Yes,
                };
                inner.requests.push(PendingRequest {
                    id,
                    session: new.session,
                    harness: new.harness,
                    harness_ref: new.harness_ref,
                    kind: new.kind,
                    payload: new.payload,
                    answerable,
                    payload_sha256,
                    match_key: new.match_key,
                    hook_wait: new.hook_wait,
                    created_ms: now_ms,
                    expires_ms: new.expires_ms,
                    state: State::Open,
                    ended_ms: None,
                    tool_run_matched: false,
                    superseded: false,
                    tool_ran_after_deny: false,
                });
                Ok(id)
            }
            Err(error) => {
                inner.refuse_registration(&new, &error, now_ms);
                Err(error)
            }
        };
        inner.prune(now_ms);
        result
    }

    /// The driver's answer operation: checks and claims the request, has
    /// `adapter` deliver the answer, and returns the state that ends in
    /// (`claimed` while a Codex confirmation is awaited). Every answer and
    /// every refusal is journalled. `clock` is read before the claim and
    /// again after delivery, so a delivery that outlasts the deadline ends as
    /// `left`, delivery unknown.
    pub fn answer(
        &self,
        adapter: &dyn Adapter,
        id: RequestId,
        answer: Answer,
        payload_sha256: &str,
        source: Source,
        clock: &dyn Fn() -> u64,
    ) -> Result<State, AnswerError> {
        let claimed = {
            let now_ms = clock();
            let mut inner = self.lock();
            inner.expire(now_ms);
            inner.claim(id, &answer, payload_sha256, &source, now_ms)?
        };
        let delivery = adapter.deliver(&claimed, &answer);
        let now_ms = clock();
        let mut inner = self.lock();
        inner.expire(now_ms);
        let Some(index) = inner.position(id) else {
            return Ok(State::Gone);
        };
        if !matches!(inner.requests[index].state, State::Claimed { .. }) {
            // The claim already ended: the deadline, the session's end or a
            // confirmation got there first, and journalled it.
            return Ok(inner.requests[index].state.clone());
        }
        let next = match delivery {
            Delivery::Written => State::Answered { answer, source },
            Delivery::Closed(reason) => State::Resolved(reason),
            Delivery::AwaitConfirmation => return Ok(inner.requests[index].state.clone()),
            Delivery::Status(status) if (200..300).contains(&status) => {
                State::Answered { answer, source }
            }
            Delivery::Status(status) => State::Rejected {
                answer,
                source,
                status,
            },
        };
        let state = inner.end_claim(index, next, now_ms);
        inner.prune(now_ms);
        Ok(state)
    }

    /// The harness confirmed Semon's delivered answer (Codex
    /// `serverRequest/resolved`): a claimed request becomes `answered`.
    /// Returns the request's state, or `None` for an unknown id.
    pub fn confirm_delivery(&self, id: RequestId, now_ms: u64) -> Option<State> {
        let mut inner = self.lock();
        inner.expire(now_ms);
        let index = inner.position(id)?;
        let claim = match &inner.requests[index].state {
            State::Claimed { answer, source, .. } => Some((answer.clone(), source.clone())),
            _ => None,
        };
        let state = match claim {
            Some((answer, source)) => {
                inner.end_claim(index, State::Answered { answer, source }, now_ms)
            }
            None => inner.requests[index].state.clone(),
        };
        inner.prune(now_ms);
        Some(state)
    }

    /// The request ended elsewhere, for `reason`: an open or claimed request
    /// becomes `resolved`; a final one is unchanged. Returns the request's
    /// state, or `None` for an unknown id.
    pub fn resolve(&self, id: RequestId, reason: ResolvedReason, now_ms: u64) -> Option<State> {
        let mut inner = self.lock();
        inner.expire(now_ms);
        let index = inner.position(id)?;
        let current = inner.requests[index].state.clone();
        let state = match current {
            State::Open => {
                set_state(&mut inner.requests[index], State::Resolved(reason), now_ms);
                inner.requests[index].state.clone()
            }
            State::Claimed { .. } => inner.end_claim(index, State::Resolved(reason), now_ms),
            other => other,
        };
        inner.prune(now_ms);
        Some(state)
    }

    /// Claude reported that a tool ran in `session` (a `PostToolUse` hook, or
    /// the transcript's tool result). It is matched to the oldest request in
    /// that session with the same key that no earlier report matched and that
    /// could have led to the run.
    pub fn tool_ran(&self, session: &str, key: &MatchKey, now_ms: u64) -> ToolRun {
        let mut inner = self.lock();
        inner.expire(now_ms);
        let outcome = inner.tool_ran(session, key, now_ms);
        inner.prune(now_ms);
        outcome
    }

    /// The session ended: its open and claimed requests become `gone`.
    pub fn session_ended(&self, session: &str, now_ms: u64) {
        let mut inner = self.lock();
        inner.expire(now_ms);
        for index in 0..inner.requests.len() {
            let (skip, claimed) = {
                let request = &inner.requests[index];
                (
                    request.session != session || request.state.is_final(),
                    matches!(request.state, State::Claimed { .. }),
                )
            };
            if skip {
                continue;
            }
            if claimed {
                inner.end_claim(index, State::Gone, now_ms);
            } else {
                set_state(&mut inner.requests[index], State::Gone, now_ms);
            }
        }
        inner.prune(now_ms);
    }

    /// Applies the deadlines due at `now_ms`: expired open requests become
    /// `left`, timed out; overdue claims become `left`, delivery unknown.
    pub fn tick(&self, now_ms: u64) {
        let mut inner = self.lock();
        inner.expire(now_ms);
        inner.prune(now_ms);
    }

    /// One request, if it is still held.
    pub fn get(&self, id: RequestId, now_ms: u64) -> Option<PendingRequest> {
        let mut inner = self.lock();
        inner.expire(now_ms);
        inner.prune(now_ms);
        let index = inner.position(id)?;
        Some(inner.requests[index].clone())
    }

    /// Everything the store holds at `now_ms`.
    pub fn snapshot(&self, now_ms: u64) -> Snapshot {
        let mut inner = self.lock();
        inner.expire(now_ms);
        inner.prune(now_ms);
        Snapshot {
            requests: inner.requests.clone(),
            dropped: inner.dropped,
            journal_failures: inner.journal_failures,
        }
    }
}

/// The payload's hash and, when it has no canonical encoding, why; or a
/// refusal for a size cap.
type Hashed = (Option<String>, Option<String>);

fn hash_payload(new: &NewRequest) -> Result<Hashed, RegisterError> {
    let tool_name_len = new.match_key.as_ref().map_or(0, |key| key.tool_name.len());
    if new.session.len() > MAX_ID_BYTES
        || tool_name_len > MAX_ID_BYTES
        || new.harness_ref.len() > MAX_ID_BYTES
    {
        return Err(RegisterError::IdTooLong);
    }
    // Refuse a payload that is too large before encoding it, from a cheap
    // lower bound on its size; the exact check follows the encoding.
    if encoded_len_floor(&new.payload, MAX_PAYLOAD_BYTES) > MAX_PAYLOAD_BYTES {
        return Err(RegisterError::PayloadTooLarge);
    }
    match canonical::canonical_bytes(&new.payload) {
        Ok(bytes) if bytes.len() > MAX_PAYLOAD_BYTES => Err(RegisterError::PayloadTooLarge),
        Ok(bytes) => Ok((Some(canonical::sha256_of(&bytes)), None)),
        Err(error) => Ok((
            None,
            Some(format!("Semon can't hash this request exactly ({error})")),
        )),
    }
}

/// A lower bound on the length of `value`'s JSON encoding, canonical or not.
/// The walk stops soon after the bound passes `limit`.
fn encoded_len_floor(value: &Value, limit: usize) -> usize {
    fn walk(value: &Value, total: &mut usize, limit: usize) {
        match value {
            Value::Null | Value::Bool(_) => *total += 4,
            Value::Number(_) => *total += 1,
            Value::String(text) => *total += text.len() + 2,
            Value::Array(items) => {
                *total += 2 + items.len().saturating_sub(1);
                for item in items {
                    if *total > limit {
                        return;
                    }
                    walk(item, total, limit);
                }
            }
            Value::Object(fields) => {
                *total += 2 + fields.len().saturating_sub(1);
                for (key, item) in fields {
                    if *total > limit {
                        return;
                    }
                    *total += key.len() + 3;
                    walk(item, total, limit);
                }
            }
        }
    }
    let mut total = 0;
    walk(value, &mut total, limit);
    total
}

/// Whether `answer` fits `request`: allow or deny for a permission; for a
/// question, exactly one label per question, each among its options.
fn answer_fits(request: &PendingRequest, answer: &Answer) -> bool {
    match (request.kind, answer) {
        (Kind::Permission, Answer::Allow | Answer::Deny { .. }) => true,
        (Kind::Question, Answer::Questions(chosen)) => single_choice_questions(&request.payload)
            .is_ok_and(|questions| {
                chosen.len() == questions.len()
                    && questions.iter().all(|question| {
                        chosen
                            .get(&question.text)
                            .is_some_and(|label| question.labels.contains(label))
                    })
            }),
        _ => false,
    }
}

/// `text` cut to at most `max` bytes on a character boundary, marked with
/// an ellipsis when cut.
fn truncated(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\u{2026}", &text[..end])
}

/// Moves `request` to `state`, stamping the end time when it is final.
fn set_state(request: &mut PendingRequest, state: State, now_ms: u64) {
    request.ended_ms = state.is_final().then_some(now_ms);
    request.state = state;
}

impl Inner {
    fn position(&self, id: RequestId) -> Option<usize> {
        self.requests.iter().position(|request| request.id == id)
    }

    fn write(&mut self, line: &Value) {
        if let Err(error) = self.journal.append(line) {
            self.journal_failures += 1;
            eprintln!("semon control: the journal write failed: {error}");
        }
    }

    fn check_caps(&self, new: &NewRequest) -> Result<(), RegisterError> {
        let live = || {
            self.requests
                .iter()
                .filter(|request| !request.state.is_final())
        };
        if live().count() >= MAX_OPEN {
            return Err(RegisterError::TooManyOpen);
        }
        if new.hook_wait {
            if live().filter(|request| request.hook_wait).count() >= MAX_HOOK_WAITS {
                return Err(RegisterError::TooManyHookWaits);
            }
            let for_session = live()
                .filter(|request| request.hook_wait && request.session == new.session)
                .count();
            if for_session >= MAX_HOOK_WAITS_PER_SESSION {
                return Err(RegisterError::TooManyHookWaitsForSession);
            }
        }
        Ok(())
    }

    /// A new request with `key` is registering in `session`, so the session
    /// has moved past every earlier request with that key that has ended and
    /// that no tool run matched: a later run belongs to the new one. This
    /// assumes Claude asks for one session's tool calls in order.
    fn supersede(&mut self, session: &str, key: &MatchKey) {
        for request in &mut self.requests {
            if request.session == session
                && request.match_key.as_ref() == Some(key)
                && request.state.is_final()
                && !request.tool_run_matched
            {
                request.superseded = true;
            }
        }
    }

    /// Counts a refused registration and journals it, at most once per
    /// session per [`REFUSAL_WINDOW_MS`]; the line carries the number of
    /// refusals suppressed in that session's previous window.
    fn refuse_registration(&mut self, new: &NewRequest, error: &RegisterError, now_ms: u64) {
        self.dropped += 1;
        let session = truncated(&new.session, JOURNAL_SESSION_BYTES);
        let existing = self
            .refusal_windows
            .iter()
            .position(|window| window.session == session);
        let suppressed_before = match existing {
            Some(index) => {
                let window = &mut self.refusal_windows[index];
                if now_ms < window.started_ms.saturating_add(REFUSAL_WINDOW_MS) {
                    window.suppressed += 1;
                    return;
                }
                let suppressed = window.suppressed;
                window.started_ms = now_ms;
                window.suppressed = 0;
                suppressed
            }
            None => {
                if self.refusal_windows.len() >= MAX_REFUSAL_WINDOWS {
                    let oldest = self
                        .refusal_windows
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, window)| window.started_ms)
                        .map_or(0, |(index, _)| index);
                    self.refusal_windows.swap_remove(oldest);
                }
                self.refusal_windows.push(RefusalWindow {
                    session: session.clone(),
                    started_ms: now_ms,
                    suppressed: 0,
                });
                0
            }
        };
        let line = json!({
            "time_ms": now_ms,
            "event": "refused_registration",
            "session": session,
            "harness": new.harness.as_str(),
            "kind": new.kind.as_str(),
            "reason": error.to_string(),
            "suppressed_before": suppressed_before,
        });
        self.write(&line);
    }

    /// Checks an answer in order (unknown, not open, read-only, hash, shape)
    /// and, if it passes, claims the request. A refusal is journalled.
    fn claim(
        &mut self,
        id: RequestId,
        answer: &Answer,
        payload_sha256: &str,
        source: &Source,
        now_ms: u64,
    ) -> Result<PendingRequest, AnswerError> {
        let index = self.position(id);
        let refusal = match index.map(|index| &self.requests[index]) {
            None => Some(AnswerError::UnknownId),
            Some(request) if request.state != State::Open => {
                Some(AnswerError::NotOpen(request.state.clone()))
            }
            Some(request) => match &request.answerable {
                Answerable::No(reason) => Some(AnswerError::ReadOnly(reason.clone())),
                Answerable::Yes if request.payload_sha256.as_deref() != Some(payload_sha256) => {
                    Some(AnswerError::HashMismatch {
                        payload: Box::new(request.payload.clone()),
                    })
                }
                Answerable::Yes if !answer_fits(request, answer) => Some(AnswerError::WrongShape),
                Answerable::Yes => None,
            },
        };
        match (index, refusal) {
            (Some(index), None) => {
                let request = &mut self.requests[index];
                request.state = State::Claimed {
                    answer: answer.clone(),
                    source: source.clone(),
                    at_ms: now_ms,
                };
                Ok(request.clone())
            }
            (index, refusal) => {
                let error = refusal.unwrap_or(AnswerError::UnknownId);
                let mut line = json!({
                    "time_ms": now_ms,
                    "event": "refused_answer",
                    "request": id.to_string(),
                    "source": source.to_json(),
                    "answer": answer.to_json(),
                    "payload_sha256": payload_sha256,
                    "status": error.http_status(),
                    "reason": error.to_string(),
                });
                if let Some(request) = index.map(|index| &self.requests[index]) {
                    line["session"] = json!(request.session);
                    line["harness"] = json!(request.harness.as_str());
                    line["kind"] = json!(request.kind.as_str());
                    line["outcome"] = request.state.to_json();
                }
                self.write(&line);
                Err(error)
            }
        }
    }

    /// Ends a claim in `next` and journals the answer with its outcome. A
    /// delivered deny on a request whose tool already ran is marked.
    fn end_claim(&mut self, index: usize, next: State, now_ms: u64) -> State {
        let request = &mut self.requests[index];
        let previous = std::mem::replace(&mut request.state, State::Open);
        set_state(request, next, now_ms);
        let State::Claimed { answer, source, .. } = previous else {
            return request.state.clone();
        };
        let mut lines = vec![answer_line(request, &answer, &source, now_ms)];
        if request.tool_run_matched
            && answer.is_deny()
            && matches!(request.state, State::Answered { .. })
        {
            request.tool_ran_after_deny = true;
            lines.push(tool_ran_after_deny_line(request, now_ms));
        }
        let state = request.state.clone();
        for line in &lines {
            self.write(line);
        }
        state
    }

    fn expire(&mut self, now_ms: u64) {
        for index in 0..self.requests.len() {
            let (timed_out, overdue) = {
                let request = &self.requests[index];
                (
                    request.state == State::Open && now_ms >= request.expires_ms,
                    matches!(
                        request.state,
                        State::Claimed { at_ms, .. }
                            if now_ms >= at_ms.saturating_add(DELIVERY_DEADLINE_MS)
                    ),
                )
            };
            if timed_out {
                set_state(
                    &mut self.requests[index],
                    State::Left(LeftReason::TimedOut),
                    now_ms,
                );
            } else if overdue {
                self.end_claim(index, State::Left(LeftReason::DeliveryUnknown), now_ms);
            }
        }
    }

    /// Drops final requests older than [`RETAIN_FINAL_MS`], then the
    /// oldest-ended ones beyond [`MAX_RETAINED_FINAL`].
    fn prune(&mut self, now_ms: u64) {
        self.requests.retain(|request| {
            request
                .ended_ms
                .is_none_or(|ended| now_ms < ended.saturating_add(RETAIN_FINAL_MS))
        });
        let mut finals: Vec<(u64, usize)> = self
            .requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| request.ended_ms.map(|ended| (ended, index)))
            .collect();
        if finals.len() <= MAX_RETAINED_FINAL {
            return;
        }
        finals.sort_unstable();
        let excess = finals.len() - MAX_RETAINED_FINAL;
        let dropped: HashSet<usize> = finals[..excess].iter().map(|&(_, index)| index).collect();
        let mut index = 0;
        self.requests.retain(|_| {
            let keep = !dropped.contains(&index);
            index += 1;
            keep
        });
    }

    fn tool_ran(&mut self, session: &str, key: &MatchKey, now_ms: u64) -> ToolRun {
        let Some(index) = self.requests.iter().position(|request| {
            request.session == session
                && request.match_key.as_ref() == Some(key)
                && !request.tool_run_matched
                && !request.superseded
                && matches!(
                    request.state,
                    State::Open | State::Claimed { .. } | State::Answered { .. } | State::Left(_)
                )
        }) else {
            return ToolRun::Unmatched;
        };
        let request = &mut self.requests[index];
        request.tool_run_matched = true;
        let id = request.id;
        let open = request.state == State::Open;
        let denied = matches!(&request.state, State::Answered { answer, .. } if answer.is_deny());
        if open {
            set_state(
                request,
                State::Resolved(ResolvedReason::AllowedInTerminal),
                now_ms,
            );
            ToolRun::ResolvedOpen(id)
        } else if denied {
            request.tool_ran_after_deny = true;
            let line = tool_ran_after_deny_line(request, now_ms);
            self.write(&line);
            ToolRun::AfterDeny(id)
        } else {
            ToolRun::Marked(id)
        }
    }
}

fn answer_line(request: &PendingRequest, answer: &Answer, source: &Source, now_ms: u64) -> Value {
    json!({
        "time_ms": now_ms,
        "event": "answer",
        "request": request.id.to_string(),
        "session": request.session,
        "harness": request.harness.as_str(),
        "kind": request.kind.as_str(),
        "source": source.to_json(),
        "answer": answer.to_json(),
        "payload_sha256": request.payload_sha256,
        "outcome": request.state.to_json(),
    })
}

fn tool_ran_after_deny_line(request: &PendingRequest, now_ms: u64) -> Value {
    json!({
        "time_ms": now_ms,
        "event": "tool_ran_after_deny",
        "request": request.id.to_string(),
        "session": request.session,
        "harness": request.harness.as_str(),
        "kind": request.kind.as_str(),
        "payload_sha256": request.payload_sha256,
        "outcome": request.state.to_json(),
    })
}

#[cfg(test)]
mod tests;

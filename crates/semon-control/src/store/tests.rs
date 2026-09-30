use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use super::{
    Adapter, AnswerError, DELIVERY_DEADLINE_MS, Delivery, MAX_HELD_PAYLOAD_BYTES, MAX_HOOK_WAITS,
    MAX_HOOK_WAITS_PER_SESSION, MAX_ID_BYTES, MAX_OPEN, MAX_PAYLOAD_BYTES, MAX_RETAINED_FINAL,
    REFUSAL_WINDOW_MS, RETAIN_FINAL_MS, RegisterError, RequestStore, ToolRun,
};
use crate::{
    journal::{
        Journal,
        tests::{lines, scratch},
    },
    request::{
        Answer, Answerable, Harness, Kind, LeftReason, MAX_DENY_MESSAGE_BYTES, MatchKey,
        NewRequest, PendingRequest, RequestId, ResolvedReason, Source, State,
    },
};

const T0: u64 = 1_000_000;
/// The wall-clock time the test stores stamp on journal lines.
const WALL: u64 = 1_700_000_000_000;
const EXPIRES: u64 = T0 + 120_000;

/// The fake adapter: every delivery ends the same way, after an optional
/// pause, and is counted.
struct Fake {
    delivery: Delivery,
    pause: Duration,
    calls: AtomicUsize,
}

impl Fake {
    fn new(delivery: Delivery) -> Self {
        Self {
            delivery,
            pause: Duration::ZERO,
            calls: AtomicUsize::new(0),
        }
    }

    fn slow(delivery: Delivery, pause: Duration) -> Self {
        Self {
            pause,
            ..Self::new(delivery)
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Adapter for Fake {
    fn deliver(&self, _request: &PendingRequest, _answer: &Answer) -> Delivery {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.pause.is_zero() {
            thread::sleep(self.pause);
        }
        self.delivery
    }
}

fn new_store(name: &str) -> (RequestStore, PathBuf) {
    let path = scratch(name).join("journal.jsonl");
    (
        RequestStore::with_wall_clock(Journal::open(&path).unwrap(), || WALL),
        path,
    )
}

fn bash_input(command: &str) -> Value {
    json!({"command": command, "description": "The agent's words"})
}

fn key(command: &str) -> MatchKey {
    MatchKey::claude("Bash", &bash_input(command)).unwrap()
}

/// A Claude Bash permission prompt with a waiting hook.
fn permission(session: &str, command: &str) -> NewRequest {
    NewRequest {
        session: session.to_owned(),
        harness: Harness::Claude,
        harness_ref: "hook".to_owned(),
        kind: Kind::Permission,
        payload: json!({"tool_name": "Bash", "tool_input": bash_input(command), "cwd": "/work"}),
        read_only: None,
        match_key: Some(key(command)),
        hook_wait: true,
        expires_ms: EXPIRES,
    }
}

/// A request with no hook waiting on it, so only the open cap applies.
fn unhooked(session: &str) -> NewRequest {
    NewRequest {
        hook_wait: false,
        match_key: None,
        harness: Harness::Codex,
        ..permission(session, "ls")
    }
}

fn question(session: &str) -> NewRequest {
    let input = json!({"questions": [{
        "question": "Which branch?",
        "header": "Branch",
        "options": [{"label": "main", "description": ""}, {"label": "dev", "description": ""}],
        "multiSelect": false,
    }]});
    NewRequest {
        kind: Kind::Question,
        match_key: Some(MatchKey::claude("AskUserQuestion", &input).unwrap()),
        payload: json!({"tool_name": "AskUserQuestion", "tool_input": input}),
        ..permission(session, "unused")
    }
}

fn source(window: &str) -> Source {
    Source {
        window: window.to_owned(),
        peer_pid: 4242,
        peer_uid: 1000,
    }
}

fn digest(store: &RequestStore, id: RequestId) -> String {
    store.get(id, T0).unwrap().payload_sha256.unwrap()
}

fn state(store: &RequestStore, id: RequestId, now_ms: u64) -> State {
    store.get(id, now_ms).unwrap().state
}

fn events(path: &Path) -> Vec<String> {
    lines(path)
        .iter()
        .map(|line| line["event"].as_str().unwrap().to_owned())
        .collect()
}

fn deny() -> Answer {
    Answer::Deny {
        message: Some("not now".to_owned()),
    }
}

#[test]
fn first_claim_wins_under_concurrent_answers() {
    let (store, path) = new_store("race");
    let id = store.register(permission("s1", "cargo test"), T0).unwrap();
    let hash = digest(&store, id);
    let adapter = Fake::slow(Delivery::Written, Duration::from_millis(50));
    let barrier = Barrier::new(8);

    let results: Vec<Result<State, AnswerError>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|window| {
                let (store, adapter, barrier, hash) = (&store, &adapter, &barrier, &hash);
                scope.spawn(move || {
                    barrier.wait();
                    store.answer(
                        adapter,
                        id,
                        Answer::Allow,
                        hash,
                        source(&format!("w{window}")),
                        &|| T0 + 1,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(adapter.calls(), 1);
    for result in &results {
        match result {
            Ok(state) => assert!(matches!(state, State::Answered { .. }), "{state:?}"),
            Err(error) => {
                assert!(
                    matches!(
                        error,
                        AnswerError::NotOpen(State::Claimed { .. } | State::Answered { .. })
                    ),
                    "{error:?}"
                );
                assert_eq!(error.http_status(), 409);
            }
        }
    }
    let events = events(&path);
    assert_eq!(events.iter().filter(|event| *event == "answer").count(), 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| *event == "refused_answer")
            .count(),
        7
    );
}

#[test]
fn not_open_comes_before_a_hash_mismatch() {
    let (store, _) = new_store("409-412");
    let id = store.register(permission("s1", "cargo test"), T0).unwrap();
    let hash = digest(&store, id);
    let adapter = Fake::new(Delivery::Written);
    store
        .answer(&adapter, id, Answer::Allow, &hash, source("w1"), &|| T0 + 1)
        .unwrap();

    let error = store
        .answer(&adapter, id, Answer::Allow, "0000", source("w2"), &|| {
            T0 + 2
        })
        .unwrap_err();
    assert!(
        matches!(error, AnswerError::NotOpen(State::Answered { .. })),
        "{error:?}"
    );
    assert_eq!(error.http_status(), 409);
    assert_eq!(adapter.calls(), 1);
}

#[test]
fn a_wrong_hash_on_an_open_request_is_412_with_the_current_payload() {
    let (store, path) = new_store("412");
    let request = permission("s1", "cargo test");
    let payload = request.payload.clone();
    let id = store.register(request, T0).unwrap();
    let adapter = Fake::new(Delivery::Written);
    let stale = crate::canonical::sha256_hex(&json!({"command": "cargo test --release"})).unwrap();

    let error = store
        .answer(&adapter, id, Answer::Allow, &stale, source("w1"), &|| {
            T0 + 1
        })
        .unwrap_err();
    assert_eq!(
        error,
        AnswerError::HashMismatch {
            payload: Box::new(payload)
        }
    );
    assert_eq!(error.http_status(), 412);
    assert_eq!(state(&store, id, T0 + 1), State::Open);
    assert_eq!(adapter.calls(), 0);
    assert_eq!(events(&path), ["refused_answer"]);
}

#[test]
fn an_answer_must_fit_the_request_kind() {
    let (store, _) = new_store("shape");
    let adapter = Fake::new(Delivery::Written);
    let asked = store.register(permission("s1", "ls"), T0).unwrap();
    let questioned = store.register(question("s2"), T0).unwrap();
    let choice = Answer::Questions(BTreeMap::from([(
        "Which branch?".to_owned(),
        "main".to_owned(),
    )]));

    for (id, answer) in [
        (asked, choice.clone()),
        (questioned, Answer::Allow),
        (questioned, Answer::Questions(BTreeMap::new())),
    ] {
        let hash = digest(&store, id);
        let error = store
            .answer(&adapter, id, answer, &hash, source("w1"), &|| T0 + 1)
            .unwrap_err();
        assert_eq!(error, AnswerError::WrongShape);
        assert_eq!(error.http_status(), 400);
    }
    let hash = digest(&store, questioned);
    let answered = store
        .answer(&adapter, questioned, choice, &hash, source("w1"), &|| {
            T0 + 1
        })
        .unwrap();
    assert!(matches!(answered, State::Answered { .. }));
    assert_eq!(adapter.calls(), 1);
}

#[test]
fn read_only_requests_refuse_answers() {
    let (store, _) = new_store("read-only");
    let adapter = Fake::new(Delivery::Written);
    let marked = store
        .register(
            NewRequest {
                read_only: Some("multi-select questions are answered in the terminal".to_owned()),
                ..question("s1")
            },
            T0,
        )
        .unwrap();
    let hash = digest(&store, marked);
    let error = store
        .answer(
            &adapter,
            marked,
            Answer::Allow,
            &hash,
            source("w1"),
            &|| T0 + 1,
        )
        .unwrap_err();
    assert!(matches!(error, AnswerError::ReadOnly(_)), "{error:?}");
    assert_eq!(error.http_status(), 409);

    // A payload with no canonical encoding has no hash to approve, so it is
    // registered read-only rather than hashed ambiguously.
    let unsafe_number = store
        .register(
            NewRequest {
                payload: json!({"tool_input": {"command": "sleep", "n": 9_007_199_254_740_993_u64}}),
                ..permission("s2", "sleep")
            },
            T0,
        )
        .unwrap();
    let request = store.get(unsafe_number, T0).unwrap();
    assert_eq!(request.payload_sha256, None);
    assert!(matches!(request.answerable, Answerable::No(_)));
    let error = store
        .answer(
            &adapter,
            unsafe_number,
            Answer::Allow,
            "",
            source("w1"),
            &|| T0 + 1,
        )
        .unwrap_err();
    assert!(matches!(error, AnswerError::ReadOnly(_)), "{error:?}");
    assert_eq!(adapter.calls(), 0);
}

#[test]
fn unknown_ids_are_404_and_journalled() {
    let (store, path) = new_store("404");
    let adapter = Fake::new(Delivery::Written);
    let error = store
        .answer(
            &adapter,
            RequestId::from_bytes([7; 16]),
            Answer::Allow,
            "",
            source("w1"),
            &|| T0,
        )
        .unwrap_err();
    assert_eq!(error, AnswerError::UnknownId);
    assert_eq!(error.http_status(), 404);
    assert_eq!(events(&path), ["refused_answer"]);
}

#[test]
fn each_delivery_mode_ends_the_claim_as_specified() {
    let (store, _) = new_store("modes");
    let cases = [
        (Delivery::Written, "answered"),
        (
            Delivery::Closed(ResolvedReason::DeniedOrInterruptedInTerminal),
            "resolved",
        ),
        (Delivery::Status(200), "answered"),
        (Delivery::Status(204), "answered"),
        (Delivery::Status(409), "rejected"),
        (Delivery::Status(500), "rejected"),
        (Delivery::AwaitConfirmation, "claimed"),
    ];
    for (number, (delivery, expected)) in cases.into_iter().enumerate() {
        let id = store
            .register(permission(&format!("s{number}"), "ls"), T0)
            .unwrap();
        let hash = digest(&store, id);
        let ended = store
            .answer(
                &Fake::new(delivery),
                id,
                deny(),
                &hash,
                source("w1"),
                &|| T0 + 1,
            )
            .unwrap();
        assert_eq!(ended.name(), expected, "{delivery:?}");
        assert_eq!(ended, state(&store, id, T0 + 1));
        // `rejected` is reachable only through a non-2xx status.
        assert_eq!(
            matches!(ended, State::Rejected { .. }),
            matches!(delivery, Delivery::Status(status) if !(200..300).contains(&status)),
        );
        match (delivery, &ended) {
            (Delivery::Closed(reason), State::Resolved(resolved)) => assert_eq!(reason, *resolved),
            (Delivery::Status(status), State::Rejected { status: got, .. }) => {
                assert_eq!(status, *got)
            }
            _ => {}
        }
        if delivery == Delivery::AwaitConfirmation {
            let confirmed = store.confirm_delivery(id, T0 + 2).unwrap();
            assert!(matches!(confirmed, State::Answered { .. }), "{confirmed:?}");
        }
    }
}

#[test]
fn a_claim_with_no_outcome_is_left_delivery_unknown_at_the_deadline() {
    let (store, path) = new_store("deadline");
    let id = store.register(permission("s1", "ls"), T0).unwrap();
    let hash = digest(&store, id);
    let claimed_at = T0 + 1;
    let claimed = store
        .answer(
            &Fake::new(Delivery::AwaitConfirmation),
            id,
            Answer::Allow,
            &hash,
            source("w1"),
            &|| claimed_at,
        )
        .unwrap();
    assert_eq!(claimed.name(), "claimed");
    assert!(
        events(&path).is_empty(),
        "an answer is journalled when its claim ends"
    );

    store.tick(claimed_at + DELIVERY_DEADLINE_MS - 1);
    assert_eq!(
        state(&store, id, claimed_at + DELIVERY_DEADLINE_MS - 1).name(),
        "claimed"
    );
    store.tick(claimed_at + DELIVERY_DEADLINE_MS);
    let left = State::Left(LeftReason::DeliveryUnknown);
    assert_eq!(state(&store, id, claimed_at + DELIVERY_DEADLINE_MS), left);
    // A confirmation after the deadline changes nothing.
    assert_eq!(
        store.confirm_delivery(id, claimed_at + DELIVERY_DEADLINE_MS + 1),
        Some(left)
    );

    let journal = lines(&path);
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0]["event"], "answer");
    assert_eq!(
        journal[0]["outcome"],
        json!({"state": "left", "reason": "delivery_unknown"})
    );
}

#[test]
fn answers_after_expiry_are_refused_and_the_request_is_left_timed_out() {
    let (store, _) = new_store("expiry");
    let adapter = Fake::new(Delivery::Written);
    let late = store.register(permission("s1", "ls"), T0).unwrap();
    let hash = digest(&store, late);
    let error = store
        .answer(&adapter, late, Answer::Allow, &hash, source("w1"), &|| {
            EXPIRES
        })
        .unwrap_err();
    assert_eq!(
        error,
        AnswerError::NotOpen(State::Left(LeftReason::TimedOut))
    );
    assert_eq!(adapter.calls(), 0);

    let in_time = store.register(permission("s2", "ls"), T0).unwrap();
    let hash = digest(&store, in_time);
    assert!(
        store
            .answer(
                &adapter,
                in_time,
                Answer::Allow,
                &hash,
                source("w1"),
                &|| EXPIRES - 1
            )
            .is_ok()
    );
}

#[test]
fn final_states_are_final() {
    let (store, _) = new_store("final");
    let adapter = Fake::new(Delivery::Written);
    let now = T0 + 1;
    let mut finals = Vec::new();

    let answered = store.register(permission("a", "ls"), T0).unwrap();
    store
        .answer(
            &adapter,
            answered,
            Answer::Allow,
            &digest(&store, answered),
            source("w"),
            &|| now,
        )
        .unwrap();
    finals.push(answered);

    let resolved = store.register(permission("b", "ls"), T0).unwrap();
    store.resolve(resolved, ResolvedReason::OtherClient, now);
    finals.push(resolved);

    let rejected = store.register(permission("c", "ls"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::Status(409)),
            rejected,
            Answer::Allow,
            &digest(&store, rejected),
            source("w"),
            &|| now,
        )
        .unwrap();
    finals.push(rejected);

    let timed_out = store
        .register(
            NewRequest {
                expires_ms: now,
                ..permission("d", "ls")
            },
            T0,
        )
        .unwrap();
    store.tick(now);
    finals.push(timed_out);

    let undelivered = store.register(permission("e", "ls"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::AwaitConfirmation),
            undelivered,
            Answer::Allow,
            &digest(&store, undelivered),
            source("w"),
            &|| now,
        )
        .unwrap();
    store.tick(now + DELIVERY_DEADLINE_MS);
    finals.push(undelivered);

    let gone = store.register(permission("f", "ls"), T0).unwrap();
    store.session_ended("f", now);
    finals.push(gone);

    let later = now + DELIVERY_DEADLINE_MS + 1;
    let expected: Vec<State> = finals.iter().map(|id| state(&store, *id, later)).collect();
    assert_eq!(
        expected.iter().map(State::name).collect::<Vec<_>>(),
        ["answered", "resolved", "rejected", "left", "left", "gone"]
    );
    for (id, before) in finals.iter().zip(&expected) {
        let request = store.get(*id, later).unwrap();
        assert!(request.state.is_final());
        let error = store
            .answer(
                &adapter,
                *id,
                Answer::Allow,
                &digest(&store, *id),
                source("w"),
                &|| later,
            )
            .unwrap_err();
        assert_eq!(error, AnswerError::NotOpen(before.clone()));
        assert_eq!(
            store
                .resolve(*id, ResolvedReason::AllowedInTerminal, later)
                .as_ref(),
            Some(before)
        );
        assert_eq!(store.confirm_delivery(*id, later).as_ref(), Some(before));
        store.session_ended(&request.session, later);
        store.tool_ran(&request.session, request.match_key.as_ref().unwrap(), later);
        store.tick(EXPIRES + 1);
        assert_eq!(&state(&store, *id, EXPIRES + 1), before);
    }
    assert_eq!(adapter.calls(), 1);
}

#[test]
fn a_session_ending_makes_its_open_and_claimed_requests_gone() {
    let (store, path) = new_store("gone");
    let open = store.register(permission("s1", "ls"), T0).unwrap();
    let claimed = store.register(permission("s1", "pwd"), T0).unwrap();
    let other = store.register(permission("s2", "ls"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::AwaitConfirmation),
            claimed,
            Answer::Allow,
            &digest(&store, claimed),
            source("w"),
            &|| T0 + 1,
        )
        .unwrap();

    store.session_ended("s1", T0 + 2);
    assert_eq!(state(&store, open, T0 + 2), State::Gone);
    assert_eq!(state(&store, claimed, T0 + 2), State::Gone);
    assert_eq!(state(&store, other, T0 + 2), State::Open);
    let journal = lines(&path);
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0]["outcome"], json!({"state": "gone"}));
}

#[test]
fn a_tool_run_resolves_the_open_request_and_a_later_answer_is_refused() {
    let (store, path) = new_store("terminal-yes");
    let adapter = Fake::new(Delivery::Written);
    let id = store.register(permission("s1", "cargo test"), T0).unwrap();
    let hash = digest(&store, id);

    assert_eq!(
        store.tool_ran("s2", &key("cargo test"), T0 + 1),
        ToolRun::Unmatched
    );
    assert_eq!(
        store.tool_ran("s1", &key("cargo test -q"), T0 + 1),
        ToolRun::Unmatched
    );
    assert_eq!(
        store.tool_ran("s1", &key("cargo test"), T0 + 1),
        ToolRun::ResolvedOpen(id)
    );
    let allowed = State::Resolved(ResolvedReason::AllowedInTerminal);
    assert_eq!(state(&store, id, T0 + 1), allowed);

    let error = store
        .answer(&adapter, id, deny(), &hash, source("w1"), &|| T0 + 2)
        .unwrap_err();
    assert_eq!(error, AnswerError::NotOpen(allowed));
    assert_eq!(adapter.calls(), 0);
    let journal = lines(&path);
    assert_eq!(events(&path), ["refused_answer"]);
    assert_eq!(
        journal[0]["outcome"],
        json!({"state": "resolved", "reason": "allowed_in_terminal"})
    );
}

#[test]
fn a_tool_run_after_a_viewer_deny_is_marked_and_journalled() {
    let (store, path) = new_store("deny-overtaken");
    let id = store
        .register(permission("s1", "rm -rf target"), T0)
        .unwrap();
    store
        .answer(
            &Fake::new(Delivery::Written),
            id,
            deny(),
            &digest(&store, id),
            source("w1"),
            &|| T0 + 1,
        )
        .unwrap();

    assert_eq!(
        store.tool_ran("s1", &key("rm -rf target"), T0 + 2),
        ToolRun::AfterDeny(id)
    );
    let request = store.get(id, T0 + 2).unwrap();
    assert!(request.tool_ran_after_deny);
    assert!(matches!(request.state, State::Answered { .. }));
    assert_eq!(events(&path), ["answer", "tool_ran_after_deny"]);
}

#[test]
fn a_tool_run_during_a_claimed_deny_marks_it_when_the_claim_ends() {
    let (store, path) = new_store("deny-claimed");
    let id = store.register(permission("s1", "make"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::AwaitConfirmation),
            id,
            deny(),
            &digest(&store, id),
            source("w1"),
            &|| T0 + 1,
        )
        .unwrap();
    assert_eq!(
        store.tool_ran("s1", &key("make"), T0 + 2),
        ToolRun::Marked(id)
    );
    assert!(!store.get(id, T0 + 2).unwrap().tool_ran_after_deny);

    store.confirm_delivery(id, T0 + 3);
    assert!(store.get(id, T0 + 3).unwrap().tool_ran_after_deny);
    assert_eq!(events(&path), ["answer", "tool_ran_after_deny"]);
}

#[test]
fn identical_requests_in_one_session_are_matched_in_order() {
    let (store, _) = new_store("order");
    let adapter = Fake::new(Delivery::Written);
    let first = store.register(permission("s1", "ls"), T0).unwrap();
    let second = store.register(permission("s1", "ls"), T0).unwrap();
    store
        .answer(
            &adapter,
            first,
            Answer::Allow,
            &digest(&store, first),
            source("w"),
            &|| T0 + 1,
        )
        .unwrap();

    // The first run is the viewer-allowed one, not the terminal's answer to
    // the second.
    assert_eq!(
        store.tool_ran("s1", &key("ls"), T0 + 2),
        ToolRun::Marked(first)
    );
    assert_eq!(state(&store, second, T0 + 2), State::Open);
    assert_eq!(
        store.tool_ran("s1", &key("ls"), T0 + 3),
        ToolRun::ResolvedOpen(second)
    );
    assert_eq!(store.tool_ran("s1", &key("ls"), T0 + 4), ToolRun::Unmatched);

    let third = store.register(permission("s2", "ls"), T0).unwrap();
    let fourth = store.register(permission("s2", "ls"), T0).unwrap();
    assert_eq!(
        store.tool_ran("s2", &key("ls"), T0 + 5),
        ToolRun::ResolvedOpen(third)
    );
    assert_eq!(state(&store, fourth, T0 + 5), State::Open);
    assert_eq!(
        store.tool_ran("s2", &key("ls"), T0 + 6),
        ToolRun::ResolvedOpen(fourth)
    );
}

#[test]
fn a_denied_or_interrupted_request_is_not_matched_by_a_tool_run() {
    let (store, _) = new_store("interrupted");
    let closed = store.register(permission("s1", "ls"), T0).unwrap();
    store.resolve(
        closed,
        ResolvedReason::DeniedOrInterruptedInTerminal,
        T0 + 1,
    );
    let next = store.register(permission("s1", "ls"), T0 + 2).unwrap();
    assert_eq!(
        store.tool_ran("s1", &key("ls"), T0 + 3),
        ToolRun::ResolvedOpen(next)
    );
}

#[test]
fn ask_user_question_matches_without_its_answers() {
    let questions =
        json!({"questions": [{"question": "Which branch?", "options": [{"label": "main"}]}]});
    let mut answered = questions.clone();
    answered["answers"] = json!({"Which branch?": "main"});
    assert_eq!(
        MatchKey::claude("AskUserQuestion", &questions).unwrap(),
        MatchKey::claude("AskUserQuestion", &answered).unwrap()
    );
    // Only for that tool: elsewhere `answers` is part of the input.
    assert_ne!(
        MatchKey::claude("Bash", &questions).unwrap(),
        MatchKey::claude("Bash", &answered).unwrap()
    );
}

#[test]
fn open_requests_are_capped() {
    let (store, path) = new_store("cap-open");
    let ids: Vec<RequestId> = (0..MAX_OPEN)
        .map(|number| store.register(unhooked(&format!("s{number}")), T0).unwrap())
        .collect();
    assert_eq!(
        store.register(unhooked("one-more"), T0),
        Err(RegisterError::TooManyOpen)
    );
    let snapshot = store.snapshot(T0);
    assert_eq!(snapshot.dropped, 1);
    assert_eq!(
        snapshot
            .requests
            .iter()
            .filter(|request| !request.state.is_final())
            .count(),
        MAX_OPEN
    );
    assert_eq!(events(&path), ["refused_registration"]);

    // The cap counts live requests only.
    store.resolve(ids[0], ResolvedReason::OtherClient, T0 + 1);
    assert!(store.register(unhooked("one-more"), T0 + 1).is_ok());
}

#[test]
fn hook_waits_are_capped_per_session_and_globally() {
    let (store, _) = new_store("cap-hooks");
    let first: Vec<RequestId> = (0..MAX_HOOK_WAITS_PER_SESSION)
        .map(|_| store.register(permission("s0", "ls"), T0).unwrap())
        .collect();
    assert_eq!(
        store.register(permission("s0", "ls"), T0),
        Err(RegisterError::TooManyHookWaitsForSession)
    );
    // A request no hook waits on isn't counted against hook waits.
    assert!(store.register(unhooked("s0"), T0).is_ok());

    let mut session = 0;
    while store
        .snapshot(T0)
        .requests
        .iter()
        .filter(|request| request.hook_wait && !request.state.is_final())
        .count()
        < MAX_HOOK_WAITS
    {
        let name = format!("t{}", session / MAX_HOOK_WAITS_PER_SESSION);
        store.register(permission(&name, "ls"), T0).unwrap();
        session += 1;
    }
    assert_eq!(
        store.register(permission("fresh", "ls"), T0),
        Err(RegisterError::TooManyHookWaits)
    );
    assert_eq!(store.snapshot(T0).dropped, 2);

    // A hook that stops waiting frees its slot.
    store.resolve(
        first[0],
        ResolvedReason::DeniedOrInterruptedInTerminal,
        T0 + 1,
    );
    assert!(store.register(permission("s0", "ls"), T0 + 1).is_ok());
}

#[test]
fn oversized_payloads_and_ids_are_not_registered() {
    let (store, path) = new_store("cap-size");
    // `{"s":"…"}` is the string plus 8 bytes.
    let sized = |length: usize| NewRequest {
        payload: json!({"s": "x".repeat(length - 8)}),
        ..unhooked("s1")
    };
    assert!(store.register(sized(MAX_PAYLOAD_BYTES), T0).is_ok());
    assert_eq!(
        store.register(sized(MAX_PAYLOAD_BYTES + 1), T0),
        Err(RegisterError::PayloadTooLarge)
    );

    assert!(
        store
            .register(unhooked(&"s".repeat(MAX_ID_BYTES)), T0)
            .is_ok()
    );
    assert_eq!(
        store.register(unhooked(&"s".repeat(MAX_ID_BYTES + 1)), T0),
        Err(RegisterError::IdTooLong)
    );
    let long_tool = NewRequest {
        match_key: Some(MatchKey {
            tool_name: "t".repeat(MAX_ID_BYTES + 1),
            input_sha256: String::new(),
        }),
        ..permission("s2", "ls")
    };
    assert_eq!(store.register(long_tool, T0), Err(RegisterError::IdTooLong));
    let long_ref = NewRequest {
        harness_ref: "r".repeat(MAX_ID_BYTES + 1),
        ..unhooked("s3")
    };
    assert_eq!(store.register(long_ref, T0), Err(RegisterError::IdTooLong));
    assert_eq!(store.snapshot(T0).dropped, 4);
    assert_eq!(
        events(&path),
        [
            "refused_registration",
            "refused_registration",
            "refused_registration",
            "refused_registration"
        ]
    );
    // The over-long session key is cut in the journal.
    let cut = lines(&path)[1]["session"].as_str().unwrap().to_owned();
    assert!(
        cut.len() <= 64 + '\u{2026}'.len_utf8() && cut.ends_with('\u{2026}'),
        "{cut}"
    );
}

#[test]
fn final_requests_are_kept_ten_minutes_and_at_most_1024() {
    let (store, _) = new_store("retention");
    let total = MAX_RETAINED_FINAL + 76;
    for number in 0..total as u64 {
        let id = store.register(unhooked("s1"), T0 + number).unwrap();
        store.resolve(id, ResolvedReason::OtherClient, T0 + number);
    }
    let last = T0 + total as u64 - 1;
    let kept = store.snapshot(last).requests;
    assert_eq!(kept.len(), MAX_RETAINED_FINAL);
    assert_eq!(kept[0].created_ms, T0 + 76, "the oldest-ended go first");

    let later = T0 + 1050 + RETAIN_FINAL_MS;
    let kept = store.snapshot(later).requests;
    assert_eq!(kept.len(), total - 1051);
    assert!(
        kept.iter()
            .all(|request| request.ended_ms.unwrap() > T0 + 1050)
    );
}

#[test]
fn the_journal_gets_one_line_per_answer_and_per_refusal() {
    let (store, path) = new_store("journal");
    let adapter = Fake::new(Delivery::Written);
    let id = store.register(permission("s1", "cargo test"), T0).unwrap();
    let hash = digest(&store, id);

    store
        .answer(&adapter, id, Answer::Allow, &hash, source("w1"), &|| T0 + 1)
        .unwrap();
    store
        .answer(&adapter, id, Answer::Allow, &hash, source("w2"), &|| T0 + 2)
        .unwrap_err();
    store
        .answer(
            &adapter,
            RequestId::from_bytes([1; 16]),
            Answer::Allow,
            &hash,
            source("w2"),
            &|| T0 + 3,
        )
        .unwrap_err();
    for _ in 0..MAX_HOOK_WAITS_PER_SESSION {
        store.register(permission("s2", "ls"), T0 + 4).unwrap();
    }
    store.register(permission("s2", "ls"), T0 + 4).unwrap_err();

    let journal = lines(&path);
    assert_eq!(
        events(&path),
        [
            "answer",
            "refused_answer",
            "refused_answer",
            "refused_registration"
        ]
    );
    assert_eq!(
        journal[0],
        json!({
            "time_ms": WALL,
            "event": "answer",
            "request": id.to_string(),
            "session": "s1",
            "harness": "claude",
            "kind": "permission",
            "source": {"window": "w1", "peer_pid": 4242, "peer_uid": 1000},
            "answer": {"decision": "allow"},
            "payload_sha256": hash,
            "outcome": {"state": "answered"},
        })
    );
    assert_eq!(journal[1]["status"], 409);
    assert_eq!(journal[1]["outcome"], json!({"state": "answered"}));
    assert_eq!(journal[2]["status"], 404);
    assert_eq!(journal[3]["session"], "s2");
    assert_eq!(store.snapshot(T0 + 4).journal_failures, 0);
}

#[test]
fn request_ids_are_32_lowercase_hex_digits() {
    let one = RequestId::random().unwrap();
    let two = RequestId::random().unwrap();
    assert_ne!(one, two);
    let text = one.to_string();
    assert_eq!(text.len(), 32);
    assert_eq!(RequestId::parse(&text), Some(one));
    let fixed = RequestId::from_bytes([0xab; 16]);
    assert_eq!(fixed.to_string(), "ab".repeat(16));
    assert_eq!(RequestId::parse(&"AB".repeat(16)), None);
    assert_eq!(RequestId::parse(&text[..31]), None);
    assert_eq!(RequestId::parse(&format!("{text}0")), None);
    assert_eq!(RequestId::parse("zz".repeat(16).as_str()), None);
}

#[test]
fn question_answers_must_match_the_payload() {
    let (store, _) = new_store("question-shape");
    let adapter = Fake::new(Delivery::Written);
    let id = store.register(question("s1"), T0).unwrap();
    let hash = digest(&store, id);
    let pick = |pairs: &[(&str, &str)]| {
        Answer::Questions(
            pairs
                .iter()
                .map(|(text, label)| ((*text).to_owned(), (*label).to_owned()))
                .collect(),
        )
    };
    for wrong in [
        pick(&[("Which branch?", "release")]),
        pick(&[("Which tag?", "main")]),
        pick(&[("Which branch?", "main"), ("Anything else?", "no")]),
    ] {
        let error = store
            .answer(&adapter, id, wrong, &hash, source("w1"), &|| T0 + 1)
            .unwrap_err();
        assert_eq!(error, AnswerError::WrongShape);
    }
    assert_eq!(state(&store, id, T0 + 1), State::Open);
    assert_eq!(adapter.calls(), 0);
    let answered = store
        .answer(
            &adapter,
            id,
            pick(&[("Which branch?", "dev")]),
            &hash,
            source("w1"),
            &|| T0 + 1,
        )
        .unwrap();
    assert!(matches!(answered, State::Answered { .. }), "{answered:?}");
}

#[test]
fn questions_step_1_cannot_answer_register_read_only() {
    let (store, _) = new_store("question-read-only");
    let payload = |questions: Value| json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": questions}});
    let cases = [
        payload(json!([{"question": "Which?", "options": [{"label": "a"}], "multiSelect": true}])),
        payload(json!([
            {"question": "Which?", "options": [{"label": "a"}]},
            {"question": "Which?", "options": [{"label": "b"}]}
        ])),
        payload(json!([{"question": "Why?", "options": []}])),
        payload(json!([])),
    ];
    for (number, payload) in cases.into_iter().enumerate() {
        let id = store
            .register(
                NewRequest {
                    payload,
                    ..question(&format!("s{number}"))
                },
                T0,
            )
            .unwrap();
        let request = store.get(id, T0).unwrap();
        assert!(matches!(request.answerable, Answerable::No(_)), "{number}");
    }
}

#[test]
fn a_retry_after_a_honoured_deny_takes_the_next_tool_run() {
    let (store, path) = new_store("retry-deny");
    let adapter = Fake::new(Delivery::Written);
    let first = store.register(permission("s1", "cargo test"), T0).unwrap();
    store
        .answer(
            &adapter,
            first,
            deny(),
            &digest(&store, first),
            source("w"),
            &|| T0 + 1,
        )
        .unwrap();
    let retry = store
        .register(permission("s1", "cargo test"), T0 + 2)
        .unwrap();

    assert_eq!(
        store.tool_ran("s1", &key("cargo test"), T0 + 3),
        ToolRun::ResolvedOpen(retry)
    );
    let denied = store.get(first, T0 + 3).unwrap();
    assert!(denied.superseded);
    assert!(!denied.tool_ran_after_deny);
    assert_eq!(events(&path), ["answer"]);
}

#[test]
fn a_retry_after_an_allowed_run_that_failed_takes_the_next_tool_run() {
    let (store, _) = new_store("retry-allow");
    let first = store.register(permission("s1", "cargo test"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::Written),
            first,
            Answer::Allow,
            &digest(&store, first),
            source("w"),
            &|| T0 + 1,
        )
        .unwrap();
    // The tool failed and no report of it arrived; the agent asks again.
    let retry = store
        .register(permission("s1", "cargo test"), T0 + 2)
        .unwrap();
    assert_eq!(
        store.tool_ran("s1", &key("cargo test"), T0 + 3),
        ToolRun::ResolvedOpen(retry)
    );
}

#[test]
fn a_retry_after_a_timed_out_request_takes_the_next_tool_run() {
    let (store, _) = new_store("retry-left");
    let first = store
        .register(
            NewRequest {
                expires_ms: T0 + 1,
                ..permission("s1", "cargo test")
            },
            T0,
        )
        .unwrap();
    store.tick(T0 + 1);
    assert_eq!(
        state(&store, first, T0 + 1),
        State::Left(LeftReason::TimedOut)
    );
    let retry = store
        .register(permission("s1", "cargo test"), T0 + 2)
        .unwrap();
    assert_eq!(
        store.tool_ran("s1", &key("cargo test"), T0 + 3),
        ToolRun::ResolvedOpen(retry)
    );
}

#[test]
fn registration_refusals_are_journalled_once_per_session_per_window() {
    let (store, path) = new_store("coalesce");
    for _ in 0..MAX_HOOK_WAITS_PER_SESSION {
        store.register(permission("s1", "ls"), T0).unwrap();
    }
    for offset in 0..5 {
        store
            .register(permission("s1", "ls"), T0 + offset)
            .unwrap_err();
    }
    assert_eq!(events(&path), ["refused_registration"]);
    store
        .register(unhooked(&"x".repeat(MAX_ID_BYTES + 1)), T0 + 5)
        .unwrap_err();
    store
        .register(permission("s1", "ls"), T0 + REFUSAL_WINDOW_MS)
        .unwrap_err();

    let journal = lines(&path);
    assert_eq!(journal.len(), 3);
    assert_eq!(journal[0]["suppressed_before"], 0);
    assert_eq!(journal[1]["reason"], RegisterError::IdTooLong.to_string());
    assert_eq!(journal[2]["session"], "s1");
    assert_eq!(journal[2]["suppressed_before"], 4);
    assert_eq!(store.snapshot(T0 + REFUSAL_WINDOW_MS).dropped, 7);
}

#[test]
fn a_delivery_that_outlasts_the_deadline_is_left_delivery_unknown() {
    let (store, path) = new_store("slow-delivery");
    let id = store.register(permission("s1", "ls"), T0).unwrap();
    let hash = digest(&store, id);
    let reads = std::cell::Cell::new(0_u64);
    let clock = || {
        let read = reads.get();
        reads.set(read + 1);
        if read == 0 {
            T0 + 1
        } else {
            T0 + 1 + DELIVERY_DEADLINE_MS
        }
    };
    let ended = store
        .answer(
            &Fake::new(Delivery::Written),
            id,
            Answer::Allow,
            &hash,
            source("w1"),
            &clock,
        )
        .unwrap();
    assert_eq!(ended, State::Left(LeftReason::DeliveryUnknown));
    assert_eq!(reads.get(), 2);
    let journal = lines(&path);
    assert_eq!(journal.len(), 1);
    assert_eq!(
        journal[0]["outcome"],
        json!({"state": "left", "reason": "delivery_unknown"})
    );
}

#[test]
fn the_size_floor_never_exceeds_the_encoding() {
    for value in [
        json!(null),
        json!(false),
        json!(12345.5),
        json!("a\u{1}b\"c\u{e9}"),
        json!([1, [2, {"k": "v"}], []]),
        json!({"a": {"b": [true, null, "x"]}, "": 0}),
        permission("s", "cargo test").payload,
    ] {
        let exact = crate::canonical::canonical_bytes(&value).unwrap().len();
        assert!(
            super::encoded_len_floor(&value, usize::MAX) <= exact,
            "{value}"
        );
    }
    let big = json!({"s": "x".repeat(MAX_PAYLOAD_BYTES)});
    assert!(super::encoded_len_floor(&big, MAX_PAYLOAD_BYTES) > MAX_PAYLOAD_BYTES);
}

#[test]
fn superseding_needs_the_same_session_and_key_and_an_ended_request() {
    let (store, _) = new_store("supersede-scope");
    let ended = store
        .register(
            NewRequest {
                expires_ms: T0 + 1,
                ..permission("s1", "ls")
            },
            T0,
        )
        .unwrap();
    store.tick(T0 + 1);
    store.register(permission("s1", "pwd"), T0 + 2).unwrap();
    store.register(permission("s2", "ls"), T0 + 2).unwrap();
    assert!(
        !store.get(ended, T0 + 2).unwrap().superseded,
        "another key or session"
    );

    let open = store.register(permission("s3", "ls"), T0 + 2).unwrap();
    let claimed = store.register(permission("s4", "ls"), T0 + 2).unwrap();
    store
        .answer(
            &Fake::new(Delivery::AwaitConfirmation),
            claimed,
            Answer::Allow,
            &digest(&store, claimed),
            source("w"),
            &|| T0 + 2,
        )
        .unwrap();
    store.register(permission("s3", "ls"), T0 + 3).unwrap();
    store.register(permission("s4", "ls"), T0 + 3).unwrap();
    assert!(
        !store.get(open, T0 + 3).unwrap().superseded,
        "an open duplicate"
    );
    assert!(
        !store.get(claimed, T0 + 3).unwrap().superseded,
        "a claimed duplicate"
    );
    assert!(store.get(ended, T0 + 3).unwrap().state.is_final());
}

#[test]
fn a_duplicate_registered_while_an_allowed_run_is_still_running_takes_that_run() {
    // Pinned, not wanted: the first request's tool is still running when an
    // identical second one registers (a parallel subagent in the same
    // session), so the first run's report resolves the second request and
    // the second's own run later matches nothing. Telling the two apart
    // needs an agent id in the hook input (docs/design/two-way.md, S10).
    let (store, _) = new_store("interleave");
    let first = store.register(permission("s1", "cargo test"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::Written),
            first,
            Answer::Allow,
            &digest(&store, first),
            source("w"),
            &|| T0 + 1,
        )
        .unwrap();
    let second = store
        .register(permission("s1", "cargo test"), T0 + 2)
        .unwrap();
    assert!(store.get(first, T0 + 2).unwrap().superseded);

    assert_eq!(
        store.tool_ran("s1", &key("cargo test"), T0 + 3),
        ToolRun::ResolvedOpen(second)
    );
    assert_eq!(
        store.tool_ran("s1", &key("cargo test"), T0 + 4),
        ToolRun::Unmatched
    );
}

#[test]
fn held_payloads_stay_within_their_byte_budget() {
    let (store, _) = new_store("budget");
    let big = |session: &str| NewRequest {
        payload: json!({"s": "x".repeat(MAX_PAYLOAD_BYTES - 8)}),
        ..unhooked(session)
    };
    let slots = MAX_HELD_PAYLOAD_BYTES / MAX_PAYLOAD_BYTES;
    let mut finals = Vec::new();
    for number in 0..slots {
        let at = T0 + number as u64;
        let id = store.register(big(&format!("f{number}")), at).unwrap();
        store.resolve(id, ResolvedReason::OtherClient, at);
        finals.push(id);
    }
    assert_eq!(store.held_payload_bytes(), MAX_HELD_PAYLOAD_BYTES);

    // Room is made by dropping the oldest final payloads first.
    store.register(big("o0"), T0 + 1000).unwrap();
    assert!(store.held_payload_bytes() <= MAX_HELD_PAYLOAD_BYTES);
    let oldest = store.get(finals[0], T0 + 1000).unwrap();
    assert!(oldest.payload_dropped);
    assert_eq!(oldest.payload, Value::Null);
    assert_eq!(oldest.payload_bytes, 0);
    assert!(!store.get(finals[1], T0 + 1000).unwrap().payload_dropped);

    for number in 1..slots {
        store
            .register(big(&format!("o{number}")), T0 + 1000)
            .unwrap();
        assert!(store.held_payload_bytes() <= MAX_HELD_PAYLOAD_BYTES);
    }
    assert!(
        finals
            .iter()
            .all(|id| store.get(*id, T0 + 1000).unwrap().payload_dropped)
    );
    // Open payloads are never dropped, so a registration that can't fit is
    // refused, however small.
    assert_eq!(
        store.register(big("over"), T0 + 1000),
        Err(RegisterError::OverBudget)
    );
    assert_eq!(
        store.register(unhooked("small"), T0 + 1000),
        Err(RegisterError::OverBudget)
    );
    assert_eq!(store.held_payload_bytes(), MAX_HELD_PAYLOAD_BYTES);
}

#[test]
fn a_lapsed_refusal_window_is_flushed_on_tick() {
    let (store, path) = new_store("flush-tick");
    for _ in 0..MAX_HOOK_WAITS_PER_SESSION {
        store.register(permission("s1", "ls"), T0).unwrap();
    }
    for offset in 0..3 {
        store
            .register(permission("s1", "ls"), T0 + offset)
            .unwrap_err();
    }
    store.tick(T0 + REFUSAL_WINDOW_MS - 1);
    assert_eq!(events(&path), ["refused_registration"]);
    store.tick(T0 + REFUSAL_WINDOW_MS);
    assert_eq!(
        events(&path),
        ["refused_registration", "refused_registrations_suppressed"]
    );
    let journal = lines(&path);
    assert_eq!(journal[1]["session"], "s1");
    assert_eq!(journal[1]["count"], 2);

    // The flushed window is gone: the next refusal starts a new one.
    store
        .register(permission("s1", "ls"), T0 + REFUSAL_WINDOW_MS + 1)
        .unwrap_err();
    assert_eq!(lines(&path)[2]["suppressed_before"], 0);
}

#[test]
fn an_evicted_refusal_window_is_flushed() {
    let (store, path) = new_store("flush-evict");
    let long = |number: usize| format!("{number:03}{}", "x".repeat(MAX_ID_BYTES));
    let windows = 64;
    for number in 0..windows {
        for _ in 0..2 {
            store
                .register(unhooked(&long(number)), T0 + number as u64)
                .unwrap_err();
        }
    }
    let flushed = |path: &Path| {
        lines(path)
            .into_iter()
            .filter(|line| line["event"] == "refused_registrations_suppressed")
            .collect::<Vec<_>>()
    };
    assert!(flushed(&path).is_empty());

    store
        .register(unhooked(&long(windows)), T0 + windows as u64)
        .unwrap_err();
    let flushed = flushed(&path);
    assert_eq!(flushed.len(), 1);
    assert!(flushed[0]["session"].as_str().unwrap().starts_with("000"));
    assert_eq!(flushed[0]["count"], 1);
    assert_eq!(store.snapshot(T0 + 100).dropped, 2 * windows as u64 + 1);
}

#[test]
fn a_deny_whose_delivery_was_left_unknown_still_flags_a_later_run() {
    let (store, path) = new_store("deny-left");
    let id = store.register(permission("s1", "make"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::AwaitConfirmation),
            id,
            deny(),
            &digest(&store, id),
            source("w"),
            &|| T0 + 1,
        )
        .unwrap();
    store.tick(T0 + 1 + DELIVERY_DEADLINE_MS);
    assert_eq!(
        state(&store, id, T0 + 1 + DELIVERY_DEADLINE_MS),
        State::Left(LeftReason::DeliveryUnknown)
    );
    assert_eq!(
        store.tool_ran("s1", &key("make"), T0 + 2 + DELIVERY_DEADLINE_MS),
        ToolRun::AfterDeny(id)
    );
    assert!(
        store
            .get(id, T0 + 2 + DELIVERY_DEADLINE_MS)
            .unwrap()
            .tool_ran_after_deny
    );
    assert_eq!(events(&path), ["answer", "tool_ran_after_deny"]);
}

#[test]
fn a_claude_request_kind_must_match_its_tool() {
    let (store, _) = new_store("kind");
    assert_eq!(
        store.register(
            NewRequest {
                kind: Kind::Question,
                ..permission("s1", "ls")
            },
            T0
        ),
        Err(RegisterError::KindMismatch)
    );
    assert_eq!(
        store.register(
            NewRequest {
                kind: Kind::Permission,
                ..question("s2")
            },
            T0
        ),
        Err(RegisterError::KindMismatch)
    );
}

#[test]
fn a_refused_answer_is_journalled_capped() {
    let (store, path) = new_store("capped");
    let id = store.register(permission("s1", "ls"), T0).unwrap();
    let hash = digest(&store, id);
    let adapter = Fake::new(Delivery::Written);
    store
        .answer(
            &adapter,
            id,
            Answer::Deny {
                message: Some("m".repeat(5000)),
            },
            "stale",
            source("w"),
            &|| T0 + 1,
        )
        .unwrap_err();
    let many: BTreeMap<String, String> = (0..40)
        .map(|number| (format!("{number:02}{}", "q".repeat(300)), "l".repeat(300)))
        .collect();
    store
        .answer(
            &adapter,
            id,
            Answer::Questions(many),
            &hash,
            source("w"),
            &|| T0 + 1,
        )
        .unwrap_err();

    let journal = lines(&path);
    let message = journal[0]["answer"]["message"].as_str().unwrap();
    assert!(
        message.len() <= 1024 + '\u{2026}'.len_utf8(),
        "{}",
        message.len()
    );
    let answers = journal[1]["answer"]["answers"].as_object().unwrap();
    assert_eq!(answers.len(), 16);
    assert!(answers.iter().all(|(text, label)| {
        text.len() <= 256 + '\u{2026}'.len_utf8()
            && label.as_str().unwrap().len() <= 256 + '\u{2026}'.len_utf8()
    }));
    assert_eq!(journal[1]["answer"]["answer_count"], 40);
    assert_eq!(adapter.calls(), 0);
}

#[test]
fn a_registration_is_refused_without_dropping_when_dropping_would_not_be_enough() {
    let (store, _) = new_store("budget-partial");
    let sized = |session: &str, bytes: usize| NewRequest {
        payload: json!({"s": "x".repeat(bytes - 8)}),
        ..unhooked(session)
    };
    let quarter = MAX_PAYLOAD_BYTES / 4;
    let final_one = store.register(sized("f", quarter), T0).unwrap();
    store.resolve(final_one, ResolvedReason::OtherClient, T0);
    let full_slots = MAX_HELD_PAYLOAD_BYTES / MAX_PAYLOAD_BYTES - 1;
    for number in 0..full_slots {
        store
            .register(sized(&format!("o{number}"), MAX_PAYLOAD_BYTES), T0 + 1)
            .unwrap();
    }
    store
        .register(sized("half", MAX_PAYLOAD_BYTES / 2), T0 + 1)
        .unwrap();
    let held = store.held_payload_bytes();
    assert_eq!(
        held,
        full_slots * MAX_PAYLOAD_BYTES + MAX_PAYLOAD_BYTES / 2 + quarter
    );

    // One more full payload needs three quarters of a slot freed; only a
    // quarter is droppable, so the registration is refused and nothing is
    // dropped.
    assert_eq!(
        store.register(sized("over", MAX_PAYLOAD_BYTES), T0 + 2),
        Err(RegisterError::OverBudget)
    );
    let kept = store.get(final_one, T0 + 2).unwrap();
    assert!(!kept.payload_dropped);
    assert_eq!(kept.payload_bytes, quarter);
    assert_eq!(store.held_payload_bytes(), held);
}

#[test]
fn a_deny_message_is_capped_before_the_claim() {
    let (store, path) = new_store("deny-cap");
    let id = store.register(permission("s1", "ls"), T0).unwrap();
    store
        .answer(
            &Fake::new(Delivery::Written),
            id,
            Answer::Deny {
                message: Some("m".repeat(5000)),
            },
            &digest(&store, id),
            source(&"w".repeat(500)),
            &|| T0 + 1,
        )
        .unwrap();
    let State::Answered {
        answer: Answer::Deny {
            message: Some(message),
        },
        ..
    } = state(&store, id, T0 + 1)
    else {
        panic!("expected a delivered deny");
    };
    assert!(message.len() <= MAX_DENY_MESSAGE_BYTES + '\u{2026}'.len_utf8());
    let journal = lines(&path);
    assert_eq!(journal[0]["answer"]["message"], json!(message));
    assert!(journal[0]["source"]["window"].as_str().unwrap().len() <= 128 + '\u{2026}'.len_utf8());
}

#[test]
fn a_malformed_hash_is_a_mismatch_and_is_journalled_cut() {
    let (store, path) = new_store("hash-format");
    let id = store.register(permission("s1", "ls"), T0).unwrap();
    let upper = digest(&store, id).to_uppercase();
    let adapter = Fake::new(Delivery::Written);
    for bad in [upper, "f".repeat(5000)] {
        let error = store
            .answer(&adapter, id, Answer::Allow, &bad, source("w"), &|| T0 + 1)
            .unwrap_err();
        assert_eq!(error.http_status(), 412);
    }
    assert!(
        lines(&path).iter().all(
            |line| line["payload_sha256"].as_str().unwrap().len() <= 64 + '\u{2026}'.len_utf8()
        )
    );
    assert_eq!(adapter.calls(), 0);
}

#[test]
fn a_claude_request_without_a_match_key_is_refused() {
    let (store, _) = new_store("no-key");
    assert_eq!(
        store.register(
            NewRequest {
                match_key: None,
                ..permission("s1", "ls")
            },
            T0
        ),
        Err(RegisterError::KindMismatch)
    );
    let id = store.register(permission("s2", "ls"), T0).unwrap();
    let request = store.get(id, T0).unwrap();
    assert_eq!(request.created_ms, T0);
    assert_eq!(request.created_wall_ms, WALL);
}

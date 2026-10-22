//! Durable turn evidence. The enclosing Event Index ledger owns its generation.
//! A writer lock is deliberately not input to this reducer.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write;

// Bound selected metadata, including exact replay rejection. Exhaustion refuses
// further attribution instead of evicting IDs and accepting old starts again.
const MAX_TURNS: usize = 4096;

fn id(value: Option<&str>) -> Option<&str> {
    value.filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnPhase {
    Active,
    Completed,
    Aborted,
    Errored,
}

/// Last exactly owned turn, observed at a complete native record boundary.
/// This is an observation, never process exit status or control authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnLifecycle {
    pub thread_id: String,
    pub turn_id: String,
    pub phase: TurnPhase,
    pub offset: u64,
    /// Consumed native prefix SHA-256; not a proof of unread trailing bytes.
    pub source_generation: String,
}

impl TurnLifecycle {
    pub(crate) fn with_generation(mut self, prefix: &[u8; 32]) -> Self {
        self.source_generation.clear();
        for byte in prefix {
            write!(&mut self.source_generation, "{byte:02x}")
                .expect("writing a String cannot fail");
        }
        self
    }
    pub(crate) fn state(&self, writer_live: bool) -> &'static str {
        match self.phase {
            TurnPhase::Active if writer_live => "work",
            TurnPhase::Active => "off",
            TurnPhase::Errored => "err",
            TurnPhase::Completed | TurnPhase::Aborted if writer_live => "idle",
            TurnPhase::Completed | TurnPhase::Aborted => "done",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct State {
    owner: Option<String>,
    // Copied history without a qualified local boundary cannot establish ownership.
    copied: bool,
    invalid: bool,
    paginated: bool,
    local_start: Option<u64>,
    ordinal: Option<u64>,
    seen: BTreeSet<String>,
    pub(crate) last: Option<TurnLifecycle>,
}

impl State {
    pub(crate) fn owned(&self, id: &str) -> Option<TurnLifecycle> {
        self.last
            .as_ref()
            .filter(|fact| fact.thread_id == id)
            .cloned()
    }

    pub(crate) fn observe(&mut self, record: &Value, offset: u64) {
        let payload = &record["payload"];
        let kind = crate::field(record, "type");
        if offset == 0 && kind == Some("session_meta") {
            self.owner = id(crate::field(payload, "id")).map(str::to_owned);
            self.copied = payload
                .get("forked_from_id")
                .is_some_and(|id| !id.is_null())
                || payload
                    .get("subagent_history_start_ordinal")
                    .is_some_and(|id| !id.is_null());
            self.paginated = crate::field(payload, "history_mode") == Some("paginated");
            self.invalid = crate::field(payload, "history_mode")
                .is_some_and(|mode| !matches!(mode, "legacy" | "paginated"));
            // These are explicit logical/local boundaries. history_base names a
            // physical rollout (possibly a revert), not the logical fork parent.
            self.local_start = payload
                .get("forked_from_ordinal_exclusive")
                .and_then(Value::as_u64)
                .into_iter()
                .chain(
                    payload
                        .get("subagent_history_start_ordinal")
                        .and_then(Value::as_u64),
                )
                .max();
            self.ordinal = record.get("ordinal").and_then(Value::as_u64);
            if self.paginated && self.ordinal.is_none() {
                self.invalid = true;
            }
            return;
        }
        if kind == Some("session_meta") {
            // A second source identity is not a thread ownership transition.
            self.invalid = true;
            self.last = None;
            return;
        }
        if self.invalid {
            return;
        }
        if self.paginated {
            let Some(ordinal) = record.get("ordinal").and_then(Value::as_u64) else {
                self.invalid = true;
                self.last = None;
                return;
            };
            if self.ordinal.is_some_and(|previous| ordinal <= previous) {
                self.invalid = true;
                self.last = None;
                return;
            }
            self.ordinal = Some(ordinal);
            if self.copied && self.local_start.is_none_or(|start| ordinal < start) {
                return;
            }
        } else if self.copied {
            return;
        }
        if kind != Some("event_msg") {
            return;
        }
        let Some(owner) = &self.owner else { return };
        // Native durable EventMsg payloads inherit their physical session owner.
        // If a carrier supplies an explicit owner, it must agree exactly.
        if crate::field(payload, "thread_id").is_some_and(|id| id != owner)
            || crate::field(record, "thread_id").is_some_and(|id| id != owner)
        {
            return;
        }
        let Some(turn) = id(crate::field(payload, "turn_id")) else {
            return;
        };
        match crate::field(payload, "type") {
            Some("task_started" | "turn_started") => {
                // A replayed start cannot reopen the terminal of that same turn.
                if self.seen.contains(turn) {
                    return;
                }
                if self.seen.len() >= MAX_TURNS {
                    self.invalid = true;
                    self.last = None;
                    return;
                }
                self.seen.insert(turn.into());
                self.last = Some(TurnLifecycle {
                    thread_id: owner.clone(),
                    turn_id: turn.into(),
                    phase: TurnPhase::Active,
                    offset,
                    source_generation: String::new(),
                });
            }
            Some("task_complete" | "turn_complete" | "turn_aborted") => {
                if payload.get("error").is_some_and(|error| {
                    !error.is_null()
                        && (!error.is_object() || crate::field(error, "message").is_none())
                }) {
                    return;
                }
                if crate::field(payload, "type") == Some("turn_aborted")
                    && !matches!(
                        crate::field(payload, "reason"),
                        Some("interrupted" | "replaced" | "review_ended" | "budget_limited")
                    )
                {
                    return;
                }
                let Some(last) = &mut self.last else { return };
                if last.turn_id != turn || last.phase != TurnPhase::Active {
                    return;
                }
                last.phase = if payload.get("error").is_some_and(Value::is_object) {
                    TurnPhase::Errored
                } else if crate::field(payload, "type") == Some("turn_aborted") {
                    TurnPhase::Aborted
                } else {
                    TurnPhase::Completed
                };
                last.offset = offset;
            }
            // Standalone Error is transient at the pinned upstream revision.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Query};
    use serde_json::json;
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const NOW: i64 = 1_790_208_000_000;

    fn event(kind: &str, turn: Option<&str>) -> Value {
        let mut payload = json!({"type":kind});
        if let Some(turn) = turn {
            payload["turn_id"] = json!(turn);
        }
        if kind == "turn_aborted" {
            payload["reason"] = json!("interrupted");
        }
        json!({"type":"event_msg","timestamp":"2026-09-24T00:00:00Z","payload":payload})
    }

    fn root() -> State {
        let mut state = State::default();
        state.observe(&json!({"type":"session_meta","payload":{"id":"owner"}}), 0);
        state
    }

    #[test]
    fn pinned_source_shaped_modes_preserve_local_turn_ownership() {
        for (source, phase, turn) in [
            (
                include_str!(
                    "../../../tests/fixtures/compatibility/codex-lifecycle-c22a06c/legacy.jsonl"
                ),
                TurnPhase::Completed,
                "turn-a",
            ),
            (
                include_str!(
                    "../../../tests/fixtures/compatibility/codex-lifecycle-c22a06c/paginated-copy.jsonl"
                ),
                TurnPhase::Aborted,
                "local-turn",
            ),
        ] {
            let mut state = State::default();
            let mut offset = 0;
            for line in source.split_inclusive('\n') {
                state.observe(&serde_json::from_str(line).unwrap(), offset);
                offset += line.len() as u64;
            }
            assert_eq!(state.last.as_ref().unwrap().phase, phase);
            assert_eq!(state.last.as_ref().unwrap().turn_id, turn);
        }
    }

    #[test]
    fn exact_turns_aliases_terminals_and_late_records() {
        for start in ["task_started", "turn_started"] {
            for complete in ["task_complete", "turn_complete", "turn_aborted"] {
                for error in [false, true] {
                    let mut state = root();
                    state.observe(&event(start, Some("a")), 100);
                    assert_eq!(state.last.as_ref().unwrap().phase, TurnPhase::Active);
                    for mut wrong in [
                        event(complete, None),
                        event(complete, Some("foreign")),
                        event(complete, Some("a")),
                    ] {
                        if wrong["payload"]["turn_id"] == "a" {
                            wrong["payload"]["thread_id"] = json!("foreign");
                        }
                        state.observe(&wrong, 150);
                        assert_eq!(state.last.as_ref().unwrap().phase, TurnPhase::Active);
                    }
                    let mut terminal = event(complete, Some("a"));
                    if error {
                        terminal["payload"]["error"] = json!({"message":"sanitized"});
                    }
                    state.observe(&terminal, 200);
                    let phase = if error {
                        TurnPhase::Errored
                    } else if complete == "turn_aborted" {
                        TurnPhase::Aborted
                    } else {
                        TurnPhase::Completed
                    };
                    assert_eq!(state.last.as_ref().unwrap().phase, phase);
                    assert_ne!(state.last.as_ref().unwrap().state(true), "work");
                    state.observe(&event(start, Some("b")), 300);
                    state.observe(&terminal, 400);
                    state.observe(&event(start, Some("a")), 500);
                    assert_eq!(state.last.as_ref().unwrap().turn_id, "b");
                    assert_eq!(state.last.as_ref().unwrap().state(true), "work");
                    assert_eq!(state.last.as_ref().unwrap().state(false), "off");
                    assert!(state.owned("different-owner").is_none());
                }
            }
        }
    }

    #[test]
    fn missing_start_malformed_terminal_and_transient_error_cannot_prove_completion() {
        let mut state = root();
        state.observe(&event("task_complete", Some("a")), 100);
        assert!(state.last.is_none());
        state.observe(&event("task_started", None), 110);
        assert!(state.last.is_none());
        state.observe(&event("task_started", Some("a")), 120);
        let mut malformed = event("task_complete", Some("a"));
        malformed["payload"]["error"] = json!("failure");
        let mut unknown_abort = event("turn_aborted", Some("a"));
        unknown_abort["payload"]["reason"] = json!("future_reason");
        for record in [malformed, unknown_abort, event("error", Some("a"))] {
            state.observe(&record, 200);
            assert_eq!(state.last.as_ref().unwrap().phase, TurnPhase::Active);
        }
        let mut no_header = State::default();
        no_header.observe(&event("task_started", Some("a")), 0);
        assert!(no_header.last.is_none());
    }

    #[test]
    fn replay_memory_is_bounded_and_exhaustion_cannot_accept_old_turns() {
        let mut state = root();
        for number in 0..MAX_TURNS {
            state.observe(
                &event("task_started", Some(&format!("turn-{number}"))),
                number as u64 + 1,
            );
        }
        assert_eq!(state.seen.len(), MAX_TURNS);
        state.observe(&event("task_started", Some("one-more")), 10000);
        state.observe(&event("task_started", Some("turn-0")), 10001);
        assert_eq!(state.seen.len(), MAX_TURNS);
        assert!(state.last.is_none());
    }

    #[test]
    fn malformed_paginated_carriers_and_conflicting_source_identity_fail_closed() {
        let mut state = State::default();
        state.observe(&json!({"ordinal":0,"type":"session_meta","payload":{"id":"owner","history_mode":"paginated"}}), 0);
        let mut start = event("turn_started", Some("a"));
        start["ordinal"] = json!(1);
        state.observe(&start, 100);
        state.observe(&event("turn_complete", Some("a")), 200);
        assert!(state.last.is_none(), "missing ordinal invalidates carrier");
        let mut state = root();
        state.observe(&event("turn_started", Some("a")), 100);
        state.observe(
            &json!({"type":"session_meta","payload":{"id":"another"}}),
            200,
        );
        state.observe(&event("turn_complete", Some("a")), 300);
        assert!(state.last.is_none());
    }

    #[test]
    fn paginated_copied_history_needs_explicit_local_boundary() {
        for boundary in [None, Some(10)] {
            let mut state = State::default();
            let mut header = json!({"ordinal":0,"type":"session_meta","payload":{"id":"owner","history_mode":"paginated","forked_from_id":"parent","history_base":{"thread_id":"different-physical-rollout","end_ordinal_exclusive":3,"end_byte_offset":100}}});
            if let Some(boundary) = boundary {
                header["payload"]["forked_from_ordinal_exclusive"] = json!(boundary);
            }
            state.observe(&header, 0);
            for (ordinal, kind, turn) in [
                (1, "task_started", "ancestor"),
                (2, "task_complete", "ancestor"),
                (10, "turn_started", "local"),
                (11, "turn_complete", "ancestor"),
            ] {
                let mut record = event(kind, Some(turn));
                record["ordinal"] = json!(ordinal);
                state.observe(&record, ordinal * 100);
            }
            if boundary.is_some() {
                assert_eq!(state.last.as_ref().unwrap().turn_id, "local");
                assert_eq!(state.last.as_ref().unwrap().phase, TurnPhase::Active);
                let mut complete = event("turn_complete", Some("local"));
                complete["ordinal"] = json!(12);
                state.observe(&complete, 1200);
                assert_eq!(state.last.as_ref().unwrap().phase, TurnPhase::Completed);
            } else {
                assert!(state.last.is_none());
            }
        }
        let mut legacy = State::default();
        legacy.observe(
            &json!({"type":"session_meta","payload":{"id":"owner","forked_from_id":"parent"}}),
            0,
        );
        legacy.observe(&event("task_started", Some("ancestor")), 100);
        assert!(legacy.last.is_none());
    }

    struct Home {
        root: PathBuf,
        path: PathBuf,
        facts: PathBuf,
        options: Options,
    }
    impl Home {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-turn-lifecycle-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let facts = root.join("facts.json");
            let options = Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                facts: Some(facts.clone()),
                all: true,
                ..Options::default()
            };
            let path = options
                .codex_home
                .join("sessions/2026/09/24/rollout-owner.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "{\"timestamp\":\"2026-09-24T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"owner\",\"cwd\":\"/synthetic\"}}\n").unwrap();
            let home = Self {
                root,
                path,
                facts,
                options,
            };
            home.live(true);
            home
        }
        fn live(&self, live: bool) {
            let mut facts = crate::facts::Facts {
                version: crate::facts::FACTS_VERSION,
                hostname: "synthetic".into(),
                codex_rollouts: Some(BTreeSet::from([
                    "sessions/2026/09/24/rollout-owner.jsonl".into()
                ])),
                ..Default::default()
            };
            if live {
                facts.codex_locks.insert("owner".into(), 42);
            } else {
                facts.offline_since = Some(NOW);
            }
            fs::write(&self.facts, serde_json::to_vec(&facts).unwrap()).unwrap();
        }
        fn append(&self, record: Value) {
            writeln!(
                fs::OpenOptions::new()
                    .append(true)
                    .open(&self.path)
                    .unwrap(),
                "{record}"
            )
            .unwrap();
        }
        fn query(&self) -> Value {
            // Query's complete fallback does not itself promise Catalog/trace
            // publication. Exercise the owning native producer before asserting
            // either its persisted rows or portable archive custody.
            let evidence = crate::native_evidence::NativeEvidence::new(self.options.clone());
            let mut work = crate::viewer::lock(&evidence.work);
            while !evidence.refresh_catalog(&mut work, NOW).unwrap() {}
            drop(work);
            evidence.retire();
            Query::new(self.options.clone())
                .call_at("list_sessions", &json!({}), NOW)
                .unwrap()["sessions"][0]
                .clone()
        }
        fn db(&self) -> rusqlite::Connection {
            rusqlite::Connection::open(crate::events::EventCache::path(&self.options.cache))
                .unwrap()
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn held_writer_index_catalog_query_and_cli_share_durable_outcome_after_restart() {
        for terminal in ["task_complete", "turn_aborted", "error-complete"] {
            let home = Home::new();
            home.append(event("task_started", Some("a")));
            assert_eq!(home.query()["state"], "working");
            let mut record = event(
                if terminal == "error-complete" {
                    "turn_complete"
                } else {
                    terminal
                },
                Some("a"),
            );
            if terminal == "error-complete" {
                record["payload"]["error"] = json!({"message":"sanitized"});
            }
            home.append(record);
            let expected = if terminal == "error-complete" {
                "error"
            } else {
                "idle"
            };
            for _ in 0..2 {
                let row = home.query();
                assert_eq!(row["state"], expected);
                assert_eq!(row["alive"], true, "writer remains held");
                assert_eq!(row["pid"], 42);
                assert!(row["exit"].is_null());
                assert_eq!(row["turn_lifecycle"]["thread_id"], "owner");
                assert_eq!(
                    row["turn_lifecycle"]["source_generation"]
                        .as_str()
                        .unwrap()
                        .len(),
                    64
                );
                let nodes = crate::collect(&home.options).unwrap();
                assert_eq!(nodes[0].state, expected);
                assert_eq!(nodes[0].pid, Some(42));
                let saved: String = home.db().query_row("SELECT json_extract(metadata,'$.query.rows.owner.summary.turn_lifecycle.phase') FROM session_catalog WHERE session_key='owner'", [], |row|row.get(0)).unwrap();
                assert_eq!(saved, row["turn_lifecycle"]["phase"].as_str().unwrap());
                let selected: String = home.db().query_row("SELECT json_extract(metadata,'$.details.turn_lifecycle.phase') FROM session_trace_headers WHERE session_key='owner'", [], |row|row.get(0)).unwrap();
                assert_eq!(selected, saved, "focused Viewer receives the same outcome");
            }
            home.append(event("turn_started", Some("b")));
            home.append(event("task_complete", Some("a")));
            home.append(event("task_started", Some("a")));
            assert_eq!(home.query()["state"], "working");
            home.live(false);
            let offline = home.query();
            assert_eq!(offline["state"], "unknown");
            assert_eq!(offline["turn_lifecycle"]["phase"], "active");
            assert!(offline["exit"].is_null());
        }
    }

    #[test]
    fn parser28_rebuild_and_source_replacement_cannot_retain_completed_turn() {
        let home = Home::new();
        home.append(event("task_started", Some("a")));
        home.append(event("task_complete", Some("a")));
        assert_eq!(home.query()["turn_lifecycle"]["phase"], "completed");
        home.db()
            .execute("UPDATE meta SET value='28' WHERE key='cache_version'", [])
            .unwrap();
        assert_eq!(home.query()["turn_lifecycle"]["phase"], "completed");
        let version: String = home
            .db()
            .query_row(
                "SELECT value FROM meta WHERE key='cache_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "29");
        let replacement = home.path.with_extension("replacement");
        fs::write(
            &replacement,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"owner\"}}\n",
        )
        .unwrap();
        fs::rename(replacement, &home.path).unwrap();
        assert_eq!(home.query()["state"], "unknown");
        assert!(home.query().get("turn_lifecycle").is_none());
    }

    #[test]
    fn retained_archive_preserves_outcome_without_inheriting_live_writer_authority() {
        let home = Home::new();
        home.append(event("task_started", Some("a")));
        home.append(event("turn_aborted", Some("a")));
        let observed = home.query();
        assert_eq!(observed["alive"], true);
        let relative = "sessions/2026/09/24/rollout-owner.jsonl";
        let proof = crate::session_source_proof(&home.options, "codex", relative)
            .unwrap()
            .unwrap();
        let ready = crate::source_projection_ready(&home.options, "codex", relative)
            .unwrap()
            .unwrap();
        crate::retain_source_event_index(&home.options, &proof, &ready).unwrap();
        fs::remove_file(&home.path).unwrap();
        let retained = home.query();
        assert_eq!(retained["turn_lifecycle"], observed["turn_lifecycle"]);
        assert!(retained["alive"].is_null());
        assert!(retained["run"].is_null());
        assert_ne!(retained["state"], "working");
        assert!(retained["exit"].is_null());
    }
}

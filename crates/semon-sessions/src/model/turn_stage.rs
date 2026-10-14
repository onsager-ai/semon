//! The native turn-end reducer, shared by complete and paged producers.
//! Only content-free observations survive a page boundary. Entries and sent
//! handoffs remain in the producer's indexed output, not in reducer state.
use super::{End, Event, ToolState, native_stage::EntryKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Content {
    Assistant,
    Tool { state: ToolState },
    Operation { state: ToolState },
    Handoff { id: String, failed: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToYou {
    pub(crate) id: String,
    pub(crate) waiting: bool,
}

pub(crate) struct HandoffEnd<'a> {
    pub(crate) id: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) status: &'a str,
}

pub(crate) fn incoming_handoff(kind: &str, received: bool, started: bool) -> bool {
    received && matches!(kind, "ask" | "relay" | "spawn") && !started
}

pub(crate) fn turn_id(owner: &str, file: &str, offset: u64, start: Option<&str>) -> String {
    start.map_or_else(|| format!("{owner}:{file}:{offset}"), str::to_owned)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EndFacts {
    has_handoff: bool,
    has_assistant: bool,
    last_content: Option<Content>,
    last_to_you: Option<ToYou>,
    completed_at: Option<i64>,
}

/// Group identity is independent of whether the group's eventual end emits a
/// turn. A gap closes the current group; incoming entries always start one.
/// Incoming-handoff deduplication belongs to the producer's indexed membership
/// set, so this cursor never retains all handoff IDs or entry positions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupCursor {
    current: Option<usize>,
    next: usize,
}

impl GroupCursor {
    pub(crate) fn push(&mut self, gap: bool, incoming: bool) -> Option<(usize, bool)> {
        if gap {
            self.current = None;
            return None;
        }
        let starts = incoming || self.current.is_none();
        if starts {
            self.current = Some(self.next);
            self.next += 1;
        }
        Some((self.current.expect("native group is open"), starts))
    }
}

impl EndFacts {
    /// Observe one native entry. Only outgoing handoffs are content; every
    /// handoff still keeps a group, and only native event references contribute
    /// an observed completion timestamp. Return whether this entry replaced
    /// the last content, so storage can checkpoint its lazy yield reference.
    pub(crate) fn observe(
        &mut self,
        kind: EntryKind,
        found: Option<&Event>,
        outgoing: Option<HandoffEnd<'_>>,
    ) -> bool {
        let content = match kind {
            EntryKind::A => Some(Content::Assistant),
            EntryKind::Tool(state) => Some(Content::Tool { state }),
            EntryKind::Operation(state) => Some(Content::Operation { state }),
            EntryKind::H(_) => outgoing.as_ref().map(|handoff| Content::Handoff {
                id: handoff.id.to_owned(),
                failed: handoff.status == "err",
            }),
            _ => None,
        };
        let replaced = content.is_some();
        let to_you = outgoing
            .filter(|handoff| handoff.kind == "toyou")
            .map(|handoff| ToYou {
                id: handoff.id.to_owned(),
                waiting: handoff.status == "wait",
            });
        let completed_at = found.and_then(|found| match kind {
            EntryKind::A | EntryKind::Operation(_) => found.t,
            EntryKind::Tool(_) => found.r.as_ref().and_then(|reply| reply.t),
            _ => None,
        });
        self.push(
            content,
            matches!(kind, EntryKind::H(_)),
            to_you,
            completed_at,
        );
        replaced
    }

    pub(crate) fn push(
        &mut self,
        content: Option<Content>,
        handoff: bool,
        to_you: Option<ToYou>,
        completed_at: Option<i64>,
    ) {
        self.append(Self {
            has_handoff: handoff,
            has_assistant: matches!(content, Some(Content::Assistant)),
            last_content: content,
            last_to_you: to_you,
            completed_at,
        });
    }

    /// Concatenate adjacent native entry ranges in transcript order. The
    /// result is identical to observing every entry, so an indexed range tree
    /// can answer a group end without decoding all of its member events.
    pub(crate) fn append(&mut self, later: Self) {
        self.has_handoff |= later.has_handoff;
        self.has_assistant |= later.has_assistant;
        if later.last_content.is_some() {
            self.last_content = later.last_content;
        }
        if later.last_to_you.is_some() {
            self.last_to_you = later.last_to_you;
        }
        self.completed_at = self.completed_at.max(later.completed_at);
    }

    pub(crate) fn finish(
        &self,
        start: Option<(bool, bool)>,
        user: bool,
        last: bool,
        waiting_for: Option<&str>,
        state: &str,
        live_yield: impl FnOnce() -> bool,
    ) -> Option<End> {
        if start.is_none() && !user && self.last_content.is_none() && !self.has_handoff {
            return None;
        }
        let make = |st, why, h| {
            Some(End {
                at: if !matches!(st, "work" | "wait")
                    && !matches!(why, "unfinished_step" | "no_reply")
                {
                    self.completed_at
                } else {
                    None
                },
                st,
                why,
                h,
            })
        };
        if last && let Some(waiting_for) = waiting_for {
            return make(
                "wait",
                if waiting_for == "permission prompt" {
                    "permission"
                } else {
                    "input"
                },
                None,
            );
        }
        if last && state == "work" {
            return make("work", "working", None);
        }
        if let Some(to_you) = &self.last_to_you {
            return make(
                if to_you.waiting { "wait" } else { "done" },
                "toyou",
                Some(to_you.id.clone()),
            );
        }
        if start.is_some_and(|(failed, _)| failed) {
            return make("err", "failed", None);
        }
        match &self.last_content {
            Some(
                Content::Tool {
                    state: ToolState::Err,
                    ..
                }
                | Content::Operation {
                    state: ToolState::Err,
                },
            ) => {
                return make("err", "failed_step", None);
            }
            Some(Content::Tool {
                state: ToolState::Pending,
            }) if !live_yield() => {
                return make("err", "unfinished_step", None);
            }
            Some(Content::Handoff { id, failed: true }) => {
                return make("err", "handoff_failed", Some(id.clone()));
            }
            _ => {}
        }
        if start.is_some_and(|(_, returned)| returned) {
            return make("done", "returned", None);
        }
        if self.has_assistant {
            return make("done", "replied", None);
        }
        make("idle", "no_reply", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_turn_ranges_combine_in_order_across_tree_shapes() {
        let mut whole = EndFacts::default();
        let mut ranges = Vec::new();
        for position in 0..1024 {
            let content = match position % 4 {
                0 => Some(Content::Assistant),
                1 => Some(Content::Tool {
                    state: ToolState::Unknown,
                }),
                2 => Some(Content::Operation {
                    state: ToolState::Err,
                }),
                _ => None,
            };
            let to_you = (position == 100).then(|| ToYou {
                id: "handoff".into(),
                waiting: false,
            });
            whole.push(
                content.clone(),
                position % 7 == 0,
                to_you.clone(),
                Some(position),
            );
            let mut range = EndFacts::default();
            range.push(content, position % 7 == 0, to_you, Some(position));
            ranges.push(range);
        }
        while ranges.len() > 1 {
            ranges = ranges
                .chunks(2)
                .map(|pair| {
                    let mut result = pair[0].clone();
                    if let Some(later) = pair.get(1) {
                        result.append(later.clone());
                    }
                    serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap()
                })
                .collect();
        }
        assert_eq!(
            serde_json::to_value(&whole).unwrap(),
            serde_json::to_value(&ranges[0]).unwrap()
        );
        for (waiting, state, last) in [
            (None, "idle", false),
            (None, "work", true),
            (Some("permission prompt"), "wait", true),
        ] {
            assert_eq!(
                serde_json::to_value(whole.finish(None, true, last, waiting, state, || false))
                    .unwrap(),
                serde_json::to_value(ranges[0].finish(None, true, last, waiting, state, || false))
                    .unwrap()
            );
        }
        assert!(serde_json::to_string(&whole).unwrap().len() < 512);
    }

    #[test]
    fn native_turn_end_survives_page_and_process_boundaries() {
        let mut facts = EndFacts::default();
        facts.push(Some(Content::Assistant), false, None, Some(20));
        let mut facts: EndFacts =
            serde_json::from_str(&serde_json::to_string(&facts).unwrap()).unwrap();
        // An incoming handoff is not outgoing content, but still keeps an
        // otherwise empty native group. An older reply time cannot undo the
        // observed completion of an earlier member.
        facts.push(None, true, None, None);
        facts.push(
            Some(Content::Tool {
                state: ToolState::Unknown,
            }),
            false,
            None,
            Some(10),
        );
        assert_eq!(
            facts.finish(None, true, false, None, "done", || false),
            Some(End {
                at: Some(20),
                st: "done",
                why: "replied",
                h: None,
            })
        );
        assert_eq!(
            facts.finish(None, true, true, Some("permission prompt"), "work", || {
                false
            }),
            Some(End {
                at: None,
                st: "wait",
                why: "permission",
                h: None,
            })
        );
    }

    #[test]
    fn native_turn_end_keeps_handoff_and_pending_priorities() {
        let mut facts = EndFacts::default();
        assert!(
            facts
                .finish(None, false, true, None, "work", || false)
                .is_none()
        );
        facts.push(None, true, None, None);
        assert_eq!(
            facts
                .finish(None, false, false, None, "done", || false)
                .unwrap()
                .why,
            "no_reply"
        );
        facts.push(
            Some(Content::Tool {
                state: ToolState::Pending,
            }),
            false,
            None,
            Some(8),
        );
        assert_eq!(
            facts.finish(Some((false, true)), false, false, None, "done", || false),
            Some(End {
                at: None,
                st: "err",
                why: "unfinished_step",
                h: None,
            })
        );
        facts.push(
            Some(Content::Tool {
                state: ToolState::Pending,
            }),
            false,
            None,
            None,
        );
        assert_eq!(
            facts
                .finish(Some((false, true)), false, false, None, "work", || true)
                .unwrap()
                .why,
            "returned"
        );
        facts.push(
            Some(Content::Handoff {
                id: "toyou".into(),
                failed: true,
            }),
            true,
            Some(ToYou {
                id: "toyou".into(),
                waiting: false,
            }),
            None,
        );
        assert_eq!(
            facts.finish(Some((true, false)), false, false, None, "done", || false),
            Some(End {
                at: Some(8),
                st: "done",
                why: "toyou",
                h: Some("toyou".into()),
            })
        );
    }

    #[test]
    fn native_turn_end_residency_stays_fixed_with_long_turns() {
        for count in [0, 128, 1024, 8192] {
            let mut facts = EndFacts::default();
            for _ in 0..count {
                facts.push(Some(Content::Assistant), false, None, Some(3));
            }
            facts.push(Some(Content::Assistant), false, None, Some(4));
            let encoded = serde_json::to_string(&facts).unwrap();
            assert!(encoded.len() < 256, "turn reducer retained its members");
            assert_eq!(
                facts.finish(None, true, false, None, "done", || false),
                Some(End {
                    at: Some(4),
                    st: "done",
                    why: "replied",
                    h: None,
                })
            );
        }
    }

    #[test]
    fn native_group_cursor_preserves_gaps_and_unemitted_group_order() {
        let mut cursor = GroupCursor::default();
        assert_eq!(cursor.push(false, false), Some((0, true)));
        for _ in 0..8192 {
            assert_eq!(cursor.push(false, false), Some((0, false)));
        }
        let mut cursor: GroupCursor =
            serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        assert_eq!(cursor.push(true, false), None);
        assert_eq!(cursor.push(true, false), None);
        assert_eq!(cursor.push(false, false), Some((1, true)));
        assert_eq!(cursor.push(false, true), Some((2, true)));
        assert_eq!(cursor.push(false, false), Some((2, false)));
        assert!(serde_json::to_string(&cursor).unwrap().len() < 64);
    }

    #[test]
    fn native_turn_end_resolves_only_the_needed_final_yield() {
        let mut facts = EndFacts::default();
        facts.push(
            Some(Content::Tool {
                state: ToolState::Pending,
            }),
            false,
            None,
            None,
        );
        let not_needed = || panic!("native yield observation crossed end precedence");
        assert_eq!(
            facts
                .finish(None, true, true, Some("input needed"), "work", not_needed)
                .unwrap()
                .why,
            "input"
        );
        assert_eq!(
            facts
                .finish(None, true, true, None, "work", not_needed)
                .unwrap()
                .why,
            "working"
        );
        assert_eq!(
            facts
                .finish(Some((true, false)), true, false, None, "done", not_needed)
                .unwrap()
                .why,
            "failed"
        );
        let calls = std::cell::Cell::new(0);
        assert_eq!(
            facts
                .finish(None, true, false, None, "done", || {
                    calls.set(calls.get() + 1);
                    false
                })
                .unwrap()
                .why,
            "unfinished_step"
        );
        assert_eq!(calls.get(), 1);
        facts.push(Some(Content::Assistant), false, None, Some(8));
        assert_eq!(
            facts
                .finish(None, true, false, None, "done", not_needed)
                .unwrap()
                .why,
            "replied"
        );
    }

    #[test]
    fn native_turn_observation_keeps_incoming_handoff_and_event_completion_distinct() {
        let mut facts = EndFacts::default();
        let found = Event {
            t: Some(100),
            ..Event::default()
        };
        assert!(!facts.observe(EntryKind::H(3), Some(&found), None));
        assert_eq!(
            facts
                .finish(None, false, false, None, "done", || false)
                .unwrap()
                .why,
            "no_reply"
        );
        let response = Event {
            t: Some(9),
            ..Event::default()
        };
        assert!(facts.observe(EntryKind::A, Some(&response), None));
        assert!(facts.observe(
            EntryKind::H(4),
            Some(&found),
            Some(HandoffEnd {
                id: "h4",
                kind: "toyou",
                status: "done"
            })
        ));
        assert_eq!(
            facts.finish(None, false, false, None, "done", || false),
            Some(End {
                at: Some(9),
                st: "done",
                why: "toyou",
                h: Some("h4".into())
            })
        );
        assert!(incoming_handoff("spawn", true, false));
        assert!(!incoming_handoff("spawn", true, true));
        assert!(!incoming_handoff("toyou", true, false));
        assert_eq!(
            turn_id(":claude:same", "file", 3, None),
            ":claude:same:file:3"
        );
        assert_eq!(turn_id("owner", "file", 3, Some("h4")), "h4");
    }
}

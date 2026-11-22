//! Read-time causal turn partitioning over selected native sources and placed
//! relationships. Runtime and parent completion are qualified by the caller;
//! this helper neither acquires evidence nor persists a chosen turn or state.

use super::Kind;
use super::{BTreeSet, End, Entry, EntryKind, Group, H, Ref, SourceFile, ToolState, Turn, event};

pub(crate) fn native_id(session: &str, file: &str, offset: u64) -> String {
    format!("{session}:{file}:{offset}")
}

/// Complete read context for a selected session. No Builder, caches, body reader,
/// or mutable derivation state is needed. `entries` passed to `split` must already
/// be merged and placed in native transcript order against these source/edge refs.
pub(super) struct TurnContext<'a> {
    pub(super) files: &'a [SourceFile],
    pub(super) handoffs: &'a [H],
    pub(super) session: usize,
    pub(super) key: &'a str,
    pub(super) state: &'a str,
    pub(super) waiting_for: Option<&'a str>,
}

impl TurnContext<'_> {
    /// The mockup's turn rule: a turn starts at each incoming entry (your
    /// message, a relay, a brief, or a user prompt) and a gap ends one.
    pub(super) fn groups(&self, entries: &[Entry]) -> Vec<Group> {
        let mut started = BTreeSet::new();
        let mut open: Vec<Group> = Vec::new();
        let mut current: Option<usize> = None;
        for (position, entry) in entries.iter().enumerate() {
            if matches!(entry.kind, EntryKind::Gap) {
                current = None;
                continue;
            }
            let incoming = match entry.kind {
                EntryKind::H(handoff) => {
                    let found = &self.handoffs[handoff];
                    found.to == Some(self.session)
                        && matches!(found.out.kind, "ask" | "relay" | "spawn")
                        && !found.inline
                        && !started.contains(&handoff)
                }
                EntryKind::U => true,
                _ => false,
            };
            if incoming || current.is_none() {
                let start = match entry.kind {
                    EntryKind::H(handoff) if incoming => {
                        started.insert(handoff);
                        Some(handoff)
                    }
                    _ => None,
                };
                open.push(Group {
                    start,
                    u: incoming && start.is_none(),
                    file: entry.file,
                    offset: entry.offset,
                    at: start
                        .map(|handoff| self.handoffs[handoff].out.at)
                        .or(entry.t),
                    entries: Vec::new(),
                });
                current = Some(open.len() - 1);
            }
            open[current.expect("open turn")].entries.push(position);
        }
        open
    }

    /// Attribute a reply to a human-started causal turn. The caller qualifies
    /// incoming native events against its selected relay/spawn relationships.
    pub(super) fn reply_started_by_you(
        &self,
        entries: &[Entry],
        refs: &[Ref],
        reply: Ref,
        root_codex: bool,
        continues_turn: impl Fn(Ref) -> bool,
    ) -> bool {
        let Some(reply_entry) = entries
            .iter()
            .position(|entry| entry.at == Some(reply) && entry.kind == EntryKind::A)
        else {
            return false;
        };
        let groups = self.groups(entries);
        let Some(group) = groups
            .iter()
            .find(|group| group.entries.contains(&reply_entry))
        else {
            return false;
        };
        let start = group
            .start
            .filter(|start| self.handoffs[*start].out.kind == "ask");
        let codex_prompt = root_codex
            && group
                .entries
                .first()
                .is_some_and(|position| entries[*position].kind == EntryKind::U);
        if start.is_none() && !codex_prompt {
            return false;
        }
        let Some(start_at) = group
            .entries
            .iter()
            .find_map(|position| entries[*position].at)
        else {
            return false;
        };
        let Some(start_position) = refs.iter().position(|at| *at == start_at) else {
            return false;
        };
        let Some(reply_position) = refs.iter().position(|at| *at == reply) else {
            return false;
        };
        if start_position > reply_position {
            return false;
        }
        !refs[start_position..=reply_position].iter().any(|at| {
            let event = event(self.files, *at);
            matches!(event.k, Kind::Xsm | Kind::Agm | Kind::Tn) && !continues_turn(*at)
                || event.k == Kind::Tool && event.n.as_deref() == Some("SubagentHandback")
        }) && start.is_none_or(|start| self.handoffs[start].out.from == "you")
    }

    /// Returns the turns, each entry's turn (by position in the returned
    /// list; `None` for entries of a turn with nothing in it), and the prompt
    /// event of each `u` turn.
    #[allow(clippy::type_complexity)]
    pub(super) fn split(
        &self,
        entries: &[Entry],
    ) -> (Vec<Turn>, Vec<Option<usize>>, Vec<(usize, Ref)>) {
        let sid = self.key;
        let open = self.groups(entries);
        let count = open.len();
        let mut turns = Vec::new();
        let mut owners = vec![None; entries.len()];
        let mut prompts = Vec::new();
        for (number, turn) in open.into_iter().enumerate() {
            let last = number + 1 == count;
            let sent: Vec<usize> = turn
                .entries
                .iter()
                .filter_map(|position| match entries[*position].kind {
                    EntryKind::H(handoff)
                        if Some(handoff) != turn.start
                            && self.handoffs[handoff].from == Some(self.session) =>
                    {
                        Some(handoff)
                    }
                    _ => None,
                })
                .collect();
            let Some(mut end) = self.end(&turn.entries, entries, turn.start, turn.u, &sent, last)
            else {
                continue;
            };
            if !matches!(end.st, "work" | "wait")
                && !matches!(end.why, "unfinished_step" | "no_reply")
            {
                end.at = turn
                    .entries
                    .iter()
                    .filter_map(|position| {
                        let entry = &entries[*position];
                        let at = entry.at?;
                        let found = event(self.files, at);
                        match entry.kind {
                            EntryKind::A => found.t,
                            EntryKind::Tool(_) => found.r.as_ref().and_then(|reply| reply.t),
                            EntryKind::Operation(_) => found.t,
                            _ => None,
                        }
                    })
                    .max();
            }
            for position in &turn.entries {
                owners[*position] = Some(turns.len());
            }
            if turn.u
                && let Some(at) = turn
                    .entries
                    .first()
                    .and_then(|position| entries[*position].at)
            {
                prompts.push((turns.len(), at));
            }
            turns.push(Turn {
                id: match turn.start {
                    Some(handoff) => self.handoffs[handoff].out.id.clone(),
                    None => native_id(sid, &self.files[turn.file].id, turn.offset),
                },
                sid: sid.to_owned(),
                start: turn
                    .start
                    .map(|handoff| self.handoffs[handoff].out.id.clone()),
                u: turn.u,
                text: None,
                at: turn.at,
                end,
                sent: sent
                    .iter()
                    .map(|handoff| self.handoffs[*handoff].out.id.clone())
                    .collect(),
                file: self.files[turn.file].id.clone(),
                offset: turn.offset,
                last,
            });
        }
        (turns, owners, prompts)
    }

    /// How a turn ended, as the mockup's `turnEnd`. `None` for a turn with
    /// nothing in it.
    #[allow(clippy::too_many_arguments)]
    fn end(
        &self,
        members: &[usize],
        entries: &[Entry],
        start: Option<usize>,
        u: bool,
        sent: &[usize],
        last: bool,
    ) -> Option<End> {
        let content: Vec<&Entry> = members
            .iter()
            .map(|position| &entries[*position])
            .filter(|entry| match entry.kind {
                EntryKind::A | EntryKind::Tool(_) | EntryKind::Operation(_) => true,
                EntryKind::H(handoff) => sent.contains(&handoff),
                _ => false,
            })
            .collect();
        let has_h = members
            .iter()
            .any(|position| matches!(entries[*position].kind, EntryKind::H(_)));
        if start.is_none() && !u && content.is_empty() && !has_h {
            return None;
        }
        if last && self.waiting_for.is_some() {
            return Some(End {
                at: None,
                st: "wait",
                why: if self.waiting_for == Some("permission prompt") {
                    "permission"
                } else {
                    "input"
                },
                h: None,
            });
        }
        if last && self.state == "work" {
            return Some(End {
                at: None,
                st: "work",
                why: "working",
                h: None,
            });
        }
        if let Some(handoff) = sent
            .iter()
            .rev()
            .find(|handoff| self.handoffs[**handoff].out.kind == "toyou")
        {
            let found = &self.handoffs[*handoff].out;
            return Some(End {
                at: None,
                st: if found.status == "wait" {
                    "wait"
                } else {
                    "done"
                },
                why: "toyou",
                h: Some(found.id.clone()),
            });
        }
        let start = start.map(|handoff| &self.handoffs[handoff].out);
        if start.is_some_and(|start| start.status == "err") {
            return Some(End {
                at: None,
                st: "err",
                why: "failed",
                h: None,
            });
        }
        let last_content = content.last();
        match last_content.map(|entry| entry.kind) {
            Some(EntryKind::Operation(ToolState::Err)) => {
                return Some(End {
                    at: None,
                    st: "err",
                    why: "failed_step",
                    h: None,
                });
            }
            Some(EntryKind::Tool(ToolState::Err)) => {
                return Some(End {
                    at: None,
                    st: "err",
                    why: "failed_step",
                    h: None,
                });
            }
            // Reached only outside the running last turn of a working
            // session, or for a yielded command still running there.
            Some(EntryKind::Tool(ToolState::Pending))
                if !last_content
                    .and_then(|entry| entry.at)
                    .is_some_and(|at| self.live_yield(at)) =>
            {
                return Some(End {
                    at: None,
                    st: "err",
                    why: "unfinished_step",
                    h: None,
                });
            }
            Some(EntryKind::H(handoff)) if self.handoffs[handoff].out.status == "err" => {
                return Some(End {
                    at: None,
                    st: "err",
                    why: "handoff_failed",
                    h: Some(self.handoffs[handoff].out.id.clone()),
                });
            }
            _ => {}
        }
        if start.is_some_and(|start| start.kind == "spawn" && start.status == "done") {
            return Some(End {
                at: None,
                st: "done",
                why: "returned",
                h: None,
            });
        }
        if content
            .iter()
            .any(|entry| matches!(entry.kind, EntryKind::A))
        {
            return Some(End {
                at: None,
                st: "done",
                why: "replied",
                h: None,
            });
        }
        Some(End {
            at: None,
            st: "idle",
            why: "no_reply",
            h: None,
        })
    }
    /// A yielded command still running: no completion yet, its session id
    /// still open in its file, and its session working.
    pub(super) fn live_yield(&self, at: Ref) -> bool {
        let found = event(self.files, at);
        self.state == "work"
            && found
                .y
                .as_ref()
                .is_some_and(|yielded| yielded.done.is_none())
            && self.files[at.0]
                .summary
                .yields
                .values()
                .any(|start| *start == at.1)
    }
}

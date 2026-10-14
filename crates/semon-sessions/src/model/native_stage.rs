//! Native entry kernels used by both the complete and paged producers.
//! Locations use generation-bound source ordinals and absolute event indices;
//! neither an entry nor its ordering key retains an Event Index payload.
use super::{
    Background, Event, Kind, Ref, Shown, SignalData, SlotKind, ToolState, events, tool_state,
};
use serde::{Deserialize, Serialize};

pub(crate) type EntryKey = (i64, usize, u64, u32, u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum EntryKind {
    H(usize),
    U,
    A,
    Tool(ToolState),
    Operation(ToolState),
    BgEnd,
    Gap,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) kind: EntryKind,
    pub(crate) file: usize,
    pub(crate) offset: u64,
    /// Transcript placement can differ from the source event's own location:
    /// a failed or pending code-mode call follows its final operation.
    pub(crate) pos: (u64, u32),
    pub(crate) t: Option<i64>,
    pub(crate) at: Option<Ref>,
}

/// A source's annotation queue is consumed in global entry order. Previously
/// reached positions stay consumed when the next entry has an earlier native
/// position; a range index can use the same scalar maximum for prefix ranks.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnnotationReach {
    position: Option<(u64, u32)>,
}
impl AnnotationReach {
    pub(crate) fn push(&mut self, position: (u64, u32)) {
        self.position = self.position.max(Some(position));
    }
    pub(crate) fn before(&self, position: (u64, u32)) -> bool {
        self.consumed(position, false)
    }
    /// Under the native parser's unique operation-offset qualification, direct
    /// operation consumption and offset-prefix consumption form this union.
    pub(crate) fn consumed(&self, position: (u64, u32), operation_reached: bool) -> bool {
        operation_reached || self.position.is_some_and(|reached| position < reached)
    }
}

/// Operation entries consume their indexed annotation instead of emitting an
/// ordinary slot. Completed code-mode calls are represented by their children.
pub(crate) fn ordinary_slot(kind: EntryKind, has_operations: bool) -> bool {
    match kind {
        EntryKind::Operation(_) => false,
        EntryKind::Tool(ToolState::Ok | ToolState::Unknown) => !has_operations,
        _ => true,
    }
}

/// Events before the first timestamp inherit that first timestamp; later
/// untimed events inherit the preceding timestamp. Producers first locate the
/// first timed event, then replay source order with this scalar cursor.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventTimeCursor {
    first: Option<i64>,
    cursor: SourceTimeCursor,
}
impl EventTimeCursor {
    pub(crate) fn new(first: Option<i64>) -> Self {
        Self {
            first,
            cursor: SourceTimeCursor::default(),
        }
    }
    pub(crate) fn push(&mut self, time: Option<i64>) -> i64 {
        self.cursor.push(time).resolve(self.first)
    }
}

/// Untimed prefix facts keep a symbolic seed. Appending the first timed event
/// changes the source scalar, while every previously stored fact stays valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SourceTime {
    BeforeFirst,
    Observed(i64),
}
impl SourceTime {
    pub(crate) fn resolve(self, first: Option<i64>) -> i64 {
        match self {
            Self::BeforeFirst => first.unwrap_or(i64::MIN),
            Self::Observed(time) => time,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceTimeCursor {
    latest: Option<i64>,
}
impl SourceTimeCursor {
    pub(crate) fn push(&mut self, time: Option<i64>) -> SourceTime {
        self.latest = time.or(self.latest);
        self.latest
            .map_or(SourceTime::BeforeFirst, SourceTime::Observed)
    }
}

pub(crate) fn event_entry(
    kind: EntryKind,
    file: usize,
    seq: usize,
    found: &Event,
    last_operation: Option<(i64, (u64, u32))>,
    effective_time: i64,
    reply_time: impl FnOnce(u64) -> i64,
) -> (i64, u8, Entry) {
    let candidate = super::native_producer::Candidate::regular(
        super::native_producer::EventInput {
            kind,
            seq,
            event: found,
            inherited: SourceTime::Observed(effective_time),
            operation: last_operation.map(|(time, pos)| (SourceTime::Observed(time), pos)),
            identity: [0; 32],
            ordinal: 0,
        },
        |offset| SourceTime::Observed(reply_time(offset)),
    );
    let (order, entry) = candidate.qualify(file, 0, true, None);
    (order.key.0, candidate.phase, entry)
}

pub(crate) fn after_entry(handoff: usize, file: usize, found: &Event) -> Entry {
    super::native_producer::Candidate::after(0, found, handoff, SourceTime::BeforeFirst, [0; 32], 0)
        .qualify(file, 0, true, None)
        .1
}

pub(crate) fn operation_entry(file: usize, found: &Event) -> Entry {
    let candidate =
        super::native_producer::Candidate::operation(0, found, SourceTime::BeforeFirst, [0; 32], 0);
    candidate.qualify(file, 0, true, None).1
}

pub(crate) fn operation_state(found: &Event) -> ToolState {
    match found.r.as_ref() {
        Some(reply) if reply.e => ToolState::Err,
        Some(reply) if reply.f & events::UNKNOWN != 0 => ToolState::Unknown,
        Some(_) => ToolState::Ok,
        None => ToolState::Pending,
    }
}

pub(crate) fn event_kind(
    found: &Event,
    background_end: bool,
    has_operations: bool,
) -> Option<EntryKind> {
    match found.k {
        Kind::U => Some(EntryKind::U),
        Kind::A => Some(EntryKind::A),
        Kind::Gap => Some(EntryKind::Gap),
        Kind::Tn if background_end => Some(EntryKind::BgEnd),
        Kind::Tool
            if found.poll.is_some() && found.script & events::SENDS == 0 && !has_operations =>
        {
            None
        }
        Kind::Tool if found.n.as_deref() != Some("SubagentHandback") => {
            Some(EntryKind::Tool(tool_state(found)))
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn position(
    kind: EntryKind,
    found: &Event,
    last_operation: Option<(i64, (u64, u32))>,
    effective_time: i64,
    reply_time: impl FnOnce(u64) -> i64,
) -> (i64, (u64, u32), u8) {
    position_time(
        kind,
        found,
        last_operation,
        effective_time,
        reply_time,
        |time| time,
    )
}

/// The position rule also accepts symbolic inherited times. Current and
/// original indexed producers therefore keep a before-first seed unresolved
/// without introducing another failed/pending operation placement rule.
pub(crate) fn position_time<T: Copy>(
    kind: EntryKind,
    found: &Event,
    last_operation: Option<(T, (u64, u32))>,
    effective_time: T,
    reply_time: impl FnOnce(u64) -> T,
    observed_time: impl FnOnce(i64) -> T,
) -> (T, (u64, u32), u8) {
    match (kind, &found.r, last_operation) {
        (EntryKind::Tool(ToolState::Err), Some(reply), Some(_)) => (
            reply
                .t
                .map(observed_time)
                .unwrap_or_else(|| reply_time(reply.o)),
            (reply.o, reply.b),
            1,
        ),
        (EntryKind::Tool(ToolState::Pending), None, Some((time, pos))) => (time, pos, 2),
        _ => (effective_time, (found.o, found.b), 1),
    }
}

pub(crate) fn key(
    several: bool,
    time: i64,
    file_order: usize,
    pos: (u64, u32),
    phase: u8,
) -> EntryKey {
    (
        if several { time } else { 0 },
        file_order,
        pos.0,
        pos.1,
        phase,
    )
}

/// A tool's native observation and bounded indexed parent context. Callers
/// resolve absolute references under their source witness; the kernel cannot
/// read a body or load another source index.
pub(crate) struct ToolInput<'a> {
    pub(crate) state: ToolState,
    pub(crate) running: bool,
    pub(crate) harness: &'a str,
    pub(crate) found: Option<&'a Event>,
    pub(crate) started: Option<&'a Event>,
    pub(crate) has_operations: bool,
}

pub(crate) fn tool_kind(
    input: ToolInput<'_>,
    live_yield: impl FnOnce() -> bool,
    background: impl FnOnce() -> Option<Background>,
) -> SlotKind {
    let shown = match input.state {
        ToolState::Ok => Shown::Ok,
        ToolState::Err => Shown::Err,
        ToolState::Unknown => Shown::Unknown,
        ToolState::Pending if input.harness == "copilot" => Shown::Unknown,
        ToolState::Pending if input.running => Shown::Live,
        ToolState::Pending => Shown::Unfinished,
    };
    let yielded = input.found.and_then(|found| found.y.as_deref());
    match (input.found, yielded, input.started) {
        (Some(found), Some(yielded), _) => SlotKind::Yielded {
            shown: match input.state {
                ToolState::Pending if live_yield() => Shown::Live,
                ToolState::Pending => Shown::Unfinished,
                _ => shown,
            },
            first: found.r.as_ref().map(|reply| reply.o),
            polls: yielded.polls.clone(),
            cut: yielded.cut,
            done: yielded.done.clone(),
        },
        (Some(found), None, Some(started)) if !input.has_operations => SlotKind::Sent {
            shown,
            reply: found.r.clone(),
            start: started.o,
            done: started
                .y
                .as_ref()
                .and_then(|yielded| yielded.done.as_ref())
                .map(|done| done.o),
        },
        _ => SlotKind::Tool {
            shown,
            name: input
                .found
                .and_then(|found| found.n.clone())
                .unwrap_or_else(|| "tool".into()),
            reply: input.found.and_then(|found| found.r.clone()),
            item: input.found.and_then(|found| found.item),
            bg: background(),
        },
    }
}

pub(crate) fn extra_kind(found: &Event, parent: Option<&Event>) -> Option<SlotKind> {
    Some(match found.k {
        Kind::Harness => SlotKind::Harness(found.n.clone().unwrap_or_default()),
        Kind::Operation => SlotKind::Operation {
            kind: found.n.clone().unwrap_or_default(),
            ok: found
                .r
                .as_ref()
                .and_then(|reply| (reply.f & events::UNKNOWN == 0).then_some(!reply.e)),
            script_offset: parent
                .filter(|parent| parent.k == Kind::Tool && parent.code_mode)
                .map(|parent| parent.o),
        },
        Kind::Think => SlotKind::Think,
        _ => return None,
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerCursor {
    owner: Option<usize>,
}

impl OwnerCursor {
    pub(crate) fn before(&self, owner: Option<usize>) -> Option<usize> {
        self.owner.or(owner)
    }
    pub(crate) fn set(&mut self, owner: Option<usize>) {
        self.owner = owner;
    }
    pub(crate) fn current(&self) -> Option<usize> {
        self.owner
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalCursor {
    previous: std::collections::BTreeMap<events::SignalKind, String>,
}

impl SignalCursor {
    pub(crate) fn apply(&mut self, signal: &mut SignalData) {
        if matches!(
            signal.kind,
            events::SignalKind::Model
                | events::SignalKind::Effort
                | events::SignalKind::Approval
                | events::SignalKind::Sandbox
                | events::SignalKind::Permission
        ) && let Some(tag) = &signal.tag
        {
            signal.previous = self.previous.insert(signal.kind, tag.clone());
        }
    }
}

pub(crate) fn signal_kind(signal: &events::Signal, tool: Option<String>) -> SignalData {
    SignalData {
        kind: signal.k,
        tag: signal.n.clone(),
        previous: None,
        value: (signal.k == events::SignalKind::Compact)
            .then_some(signal.v)
            .flatten(),
        tool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_operation_annotation_prefix_matches_queue_union_with_backwards_entries() {
        let annotations = [
            ((1, 0), false),
            ((4, 0), true),
            ((6, 0), false),
            ((7, 0), true),
            ((9, 0), true),
            ((10, 0), false),
        ];
        let mut queue = std::collections::VecDeque::from(annotations);
        let mut reach = AnnotationReach::default();
        let mut consumed = std::collections::BTreeSet::new();
        let mut operations = std::collections::BTreeSet::new();
        for (position, operation) in [
            ((7, 0), true),
            ((8, 0), false),
            ((4, 0), true),
            ((9, 0), true),
            ((2, 0), false),
            ((11, 0), false),
        ] {
            while queue.front().is_some_and(|row| row.0 < position) {
                consumed.insert(queue.pop_front().unwrap().0);
            }
            if operation {
                operations.insert(position);
                if let Some(at) = queue.iter().position(|row| row.1 && row.0.0 == position.0) {
                    consumed.insert(queue.remove(at).unwrap().0);
                }
            }
            reach.push(position);
            for (marker, is_operation) in annotations {
                assert_eq!(
                    reach.consumed(marker, is_operation && operations.contains(&marker)),
                    consumed.contains(&marker)
                );
            }
        }
    }

    #[test]
    fn native_parser_operation_offsets_are_unique_even_when_item_ids_repeat() {
        let mut index = crate::events::FileIndex::default();
        crate::events::codex(
            &mut index,
            &serde_json::json!({"type":"response_item",
            "payload":{"type":"custom_tool_call","call_id":"exec","name":"exec",
                "input":"await tools.exec_command({cmd:'ls'})"}}),
            0,
        );
        for offset in 1..=32 {
            crate::events::codex(
                &mut index,
                &serde_json::json!({"type":"event_msg",
                "payload":{"type":"item_completed","item":{"type":"CommandExecution",
                    "id":"same-item","exit_code":0}}}),
                offset,
            );
        }
        let operations: Vec<_> = index
            .extras
            .iter()
            .filter(|event| event.k == Kind::Operation)
            .collect();
        assert_eq!(operations.len(), 32);
        assert!(operations.iter().all(|event| event.b == 0));
        assert_eq!(
            operations
                .iter()
                .map(|event| event.o)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            operations.len()
        );
    }

    #[test]
    fn native_annotation_reach_matches_offset_queue_with_backwards_entries_and_operations() {
        let annotations = [
            ((1, 0), false),
            ((4, 0), true),
            ((4, 1), true),
            ((6, 0), false),
            ((9, 0), true),
            ((12, 0), false),
        ];
        let mut reference = std::collections::VecDeque::from(annotations);
        let mut indexed = reference.clone();
        let mut reach = AnnotationReach::default();
        let mut expected = Vec::new();
        let mut found = Vec::new();
        // These native placements are reached in timestamp order. Direct
        // operation removal selects the first remaining same-offset marker.
        for (step, (position, operation)) in [
            ((4, 0), true),
            ((9, 0), false),
            ((2, 0), false),
            ((4, 1), true),
            ((11, 0), false),
        ]
        .into_iter()
        .enumerate()
        {
            while reference.front().is_some_and(|row| row.0 < position) {
                expected.push(reference.pop_front().unwrap());
            }
            reach.push(position);
            while indexed.front().is_some_and(|row| reach.before(row.0)) {
                found.push(indexed.pop_front().unwrap());
            }
            if operation {
                for (queue, output) in [(&mut reference, &mut expected), (&mut indexed, &mut found)]
                {
                    if let Some(at) = queue.iter().position(|row| row.1 && row.0.0 == position.0) {
                        output.push(queue.remove(at).unwrap());
                    }
                }
            }
            assert_eq!(found, expected);
            assert_eq!(indexed, reference);
            if step == 1 {
                reach = serde_json::from_str(&serde_json::to_string(&reach).unwrap()).unwrap();
            }
        }
        assert_eq!(
            indexed.into_iter().collect::<Vec<_>>(),
            vec![((12, 0), false)]
        );
        for state in [ToolState::Ok, ToolState::Unknown] {
            assert!(!ordinary_slot(EntryKind::Tool(state), true));
            assert!(ordinary_slot(EntryKind::Tool(state), false));
        }
        assert!(ordinary_slot(EntryKind::Tool(ToolState::Err), true));
        assert!(ordinary_slot(EntryKind::Tool(ToolState::Pending), true));
        assert!(!ordinary_slot(EntryKind::Operation(ToolState::Ok), false));
    }

    #[test]
    fn native_symbolic_time_prefix_survives_first_timestamp_append() {
        let mut cursor = SourceTimeCursor::default();
        let prefix = [cursor.push(None), cursor.push(None)];
        let before = serde_json::to_value(prefix).unwrap();
        let mut cursor: SourceTimeCursor =
            serde_json::from_value(serde_json::to_value(cursor).unwrap()).unwrap();
        assert!(prefix.iter().all(|time| time.resolve(None) == i64::MIN));
        assert_eq!(cursor.push(Some(40)), SourceTime::Observed(40));
        assert_eq!(cursor.push(None), SourceTime::Observed(40));
        assert_eq!(cursor.push(Some(10)), SourceTime::Observed(10));
        assert!(prefix.iter().all(|time| time.resolve(Some(40)) == 40));
        assert_eq!(serde_json::to_value(prefix).unwrap(), before);
    }

    #[test]
    fn native_event_time_cursor_preserves_untimed_prefix_across_restart() {
        let times = [None, None, Some(40), None, Some(10), None];
        let mut cursor = EventTimeCursor::new(times.iter().copied().flatten().next());
        let mut actual = Vec::new();
        for time in times {
            actual.push(cursor.push(time));
            cursor = serde_json::from_value(serde_json::to_value(cursor).unwrap()).unwrap();
        }
        assert_eq!(actual, [40, 40, 40, 40, 10, 10]);
        assert_eq!(EventTimeCursor::new(None).push(None), i64::MIN);
    }

    #[test]
    fn native_entry_keys_preserve_single_file_order_and_merge_ties() {
        assert!(key(false, 30, 0, (8, 0), 1) < key(false, 10, 0, (9, 0), 1));
        assert!(key(true, 10, 1, (9, 0), 1) < key(true, 30, 0, (8, 0), 1));
        assert!(key(true, 10, 0, (9, 4), 1) < key(true, 10, 1, (1, 0), 1));
        assert!(key(true, 10, 0, (9, 4), 1) < key(true, 10, 0, (9, 4), 2));
    }

    #[test]
    fn native_entry_mapping_keeps_poll_and_operation_ownership() {
        let found = Event {
            k: Kind::Tool,
            poll: Some(2),
            ..Event::default()
        };
        assert_eq!(event_kind(&found, false, false), None);
        assert_eq!(
            event_kind(&found, false, true),
            Some(EntryKind::Tool(ToolState::Pending))
        );
        let sent = Event {
            script: events::SENDS,
            ..found.clone()
        };
        assert_eq!(
            event_kind(&sent, false, false),
            Some(EntryKind::Tool(ToolState::Pending))
        );
        assert_eq!(
            position(
                EntryKind::Tool(ToolState::Pending),
                &found,
                Some((20, (30, 4))),
                10,
                |_| panic!("unneeded reply lookup")
            ),
            (20, (30, 4), 2)
        );
        let returned = Event {
            n: Some("SubagentHandback".into()),
            ..found
        };
        assert_eq!(event_kind(&returned, false, true), None);
    }

    #[test]
    fn native_tool_kernel_preserves_pending_and_yielded_observations() {
        let found = Event {
            k: Kind::Tool,
            n: Some("Bash".into()),
            ..Event::default()
        };
        for (harness, running, expected) in [
            ("claude", false, Shown::Unfinished),
            ("claude", true, Shown::Live),
            ("copilot", true, Shown::Unknown),
        ] {
            let kind = tool_kind(
                ToolInput {
                    state: ToolState::Pending,
                    running,
                    harness,
                    found: Some(&found),
                    started: None,
                    has_operations: false,
                },
                || panic!("ordinary tool consulted yielded-command liveness"),
                || None,
            );
            let SlotKind::Tool { shown, .. } = kind else {
                panic!("ordinary tool shape changed")
            };
            assert_eq!(shown, expected);
        }
        let yielded = Event {
            y: Some(Box::default()),
            ..found
        };
        for (live, expected) in [(false, Shown::Unfinished), (true, Shown::Live)] {
            let kind = tool_kind(
                ToolInput {
                    state: ToolState::Pending,
                    running: true,
                    harness: "codex",
                    found: Some(&yielded),
                    started: None,
                    has_operations: false,
                },
                || live,
                || panic!("yielded command consulted Bash background state"),
            );
            let SlotKind::Yielded { shown, .. } = kind else {
                panic!("yielded tool shape changed")
            };
            assert_eq!(shown, expected);
        }
    }

    #[test]
    fn native_extra_kernel_keeps_code_mode_parent_attribution() {
        let extra = Event {
            k: Kind::Operation,
            n: Some("CommandExecution".into()),
            ..Event::default()
        };
        let parent = Event {
            k: Kind::Tool,
            code_mode: true,
            o: 42,
            ..Event::default()
        };
        let Some(SlotKind::Operation {
            ok, script_offset, ..
        }) = extra_kind(&extra, Some(&parent))
        else {
            panic!("operation shape changed")
        };
        assert_eq!(ok, None);
        assert_eq!(script_offset, Some(42));
        let wrong_parent = Event {
            code_mode: false,
            ..parent
        };
        let Some(SlotKind::Operation { script_offset, .. }) =
            extra_kind(&extra, Some(&wrong_parent))
        else {
            panic!("operation shape changed")
        };
        assert_eq!(script_offset, None);
    }

    #[test]
    fn native_annotation_owner_cursor_survives_empty_and_changed_groups() {
        let mut owners = OwnerCursor::default();
        assert_eq!(owners.before(Some(1)), Some(1));
        owners.set(Some(1));
        assert_eq!(owners.before(Some(2)), Some(1));
        owners.set(Some(2));
        let mut owners: OwnerCursor =
            serde_json::from_str(&serde_json::to_string(&owners).unwrap()).unwrap();
        assert_eq!(owners.current(), Some(2));
        assert_eq!(owners.before(None), Some(2));
        owners.set(None);
        assert_eq!(owners.current(), None);
        assert_eq!(owners.before(Some(3)), Some(3));
    }

    #[test]
    fn native_signal_cursor_keeps_only_native_setting_history() {
        let mut cursor = SignalCursor::default();
        for tag in ["a", "b"] {
            let signal = events::Signal {
                k: events::SignalKind::Model,
                o: 0,
                t: None,
                at: 0,
                n: Some(tag.into()),
                v: None,
            };
            let mut data = signal_kind(&signal, None);
            cursor.apply(&mut data);
            assert_eq!(data.previous.as_deref(), (tag == "b").then_some("a"));
        }
        let mut cursor: SignalCursor =
            serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        for _ in 0..8192 {
            let signal = events::Signal {
                k: events::SignalKind::Compact,
                o: 0,
                t: None,
                at: 0,
                n: Some("auto".into()),
                v: Some(128),
            };
            let mut data = signal_kind(&signal, None);
            cursor.apply(&mut data);
            assert_eq!(data.value, Some(128));
            assert_eq!(data.previous, None);
        }
        assert!(serde_json::to_string(&cursor).unwrap().len() < 128);
        let signal = events::Signal {
            k: events::SignalKind::Model,
            o: 0,
            t: None,
            at: 0,
            n: Some("c".into()),
            v: None,
        };
        let mut data = signal_kind(&signal, None);
        cursor.apply(&mut data);
        assert_eq!(data.previous.as_deref(), Some("b"));
    }
}

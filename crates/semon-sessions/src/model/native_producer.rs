//! Point-address native candidates consumed by current and original range trees.
//! This adapter retains one content-free entry, never an event/transcript Vec.
use super::{
    Event,
    native_stage::{self, Entry, EntryKind, SourceTime},
    range_stage::{CandidateRef, Facts, Leaf, Order},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Address {
    pub(crate) extra: bool,
    pub(crate) seq: usize,
}

/// Stable native emission order inside an equal placement key. Storage assigns
/// the tree's u64 tie labels against this descriptor; it must never use the
/// candidate digest or a packed/truncated event sequence as semantic order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmissionOrder {
    class: u8,
    seq: u64,
    member: u64,
}

impl EmissionOrder {
    pub(crate) fn regular(seq: usize, member: usize) -> Self {
        Self {
            class: 1,
            seq: seq as u64,
            member: member as u64,
        }
    }
    pub(crate) fn after(seq: usize) -> Self {
        Self {
            class: 1,
            seq: seq as u64,
            member: u64::MAX,
        }
    }
    pub(crate) fn operation(seq: usize) -> Self {
        Self {
            class: 2,
            seq: seq as u64,
            member: 0,
        }
    }
    pub(crate) fn head() -> Self {
        Self {
            class: 0,
            seq: 0,
            member: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Candidate {
    pub(crate) address: Address,
    pub(crate) identity: CandidateRef,
    pub(crate) ordinal: u64,
    pub(crate) time: SourceTime,
    pub(crate) phase: u8,
    pub(crate) entry: Entry,
    pub(crate) has_operations: bool,
    #[serde(default)]
    pub(crate) head: bool,
}

pub(crate) struct EventInput<'a> {
    pub(crate) seq: usize,
    pub(crate) event: &'a Event,
    pub(crate) kind: EntryKind,
    pub(crate) inherited: SourceTime,
    pub(crate) operation: Option<(SourceTime, (u64, u32))>,
    pub(crate) identity: CandidateRef,
    pub(crate) ordinal: u64,
}

impl Candidate {
    pub(crate) fn regular(
        input: EventInput<'_>,
        reply_time: impl FnOnce(u64) -> SourceTime,
    ) -> Self {
        let (time, pos, phase) = native_stage::position_time(
            input.kind,
            input.event,
            input.operation,
            input.inherited,
            reply_time,
            SourceTime::Observed,
        );
        Self {
            address: Address {
                extra: false,
                seq: input.seq,
            },
            identity: input.identity,
            ordinal: input.ordinal,
            time,
            phase,
            entry: Entry {
                kind: input.kind,
                file: 0,
                offset: input.event.o,
                pos,
                t: input.event.t,
                at: Some((0, input.seq)),
            },
            has_operations: input.operation.is_some(),
            head: false,
        }
    }
    pub(crate) fn operation(
        seq: usize,
        found: &Event,
        inherited: SourceTime,
        identity: CandidateRef,
        ordinal: u64,
    ) -> Self {
        Self {
            address: Address { extra: true, seq },
            identity,
            ordinal,
            time: found.t.map_or(inherited, SourceTime::Observed),
            phase: 1,
            entry: Entry {
                kind: EntryKind::Operation(native_stage::operation_state(found)),
                file: 0,
                offset: found.o,
                pos: (found.o, found.b),
                t: found.t,
                at: None,
            },
            has_operations: false,
            head: false,
        }
    }
    pub(crate) fn after(
        seq: usize,
        event: &Event,
        handoff: usize,
        inherited: SourceTime,
        identity: CandidateRef,
        ordinal: u64,
    ) -> Self {
        Self {
            address: Address { extra: false, seq },
            identity,
            ordinal,
            time: inherited,
            phase: 2,
            entry: Entry {
                kind: EntryKind::H(handoff),
                file: 0,
                offset: event.o,
                pos: (event.o, event.b),
                t: event.t,
                at: None,
            },
            has_operations: false,
            head: false,
        }
    }
    /// A native head points at the first eligible event's source offset (or
    /// first source offset0), but its ordering key is always the forced head.
    pub(crate) fn head(seq: usize, offset: u64, handoff: usize, identity: CandidateRef) -> Self {
        Self {
            address: Address { extra: false, seq },
            identity,
            ordinal: 0,
            time: SourceTime::Observed(i64::MIN),
            phase: 0,
            entry: Entry {
                kind: EntryKind::H(handoff),
                file: 0,
                offset,
                pos: (offset, 0),
                t: None,
                at: None,
            },
            has_operations: false,
            head: true,
        }
    }
    /// A file's global source reference ordinal and its session merge ordinal
    /// are different native concepts; qualify both explicitly.
    pub(crate) fn qualify(
        &self,
        source: usize,
        file_order: usize,
        several: bool,
        first_regular_time: Option<i64>,
    ) -> (Order, Entry) {
        let mut entry = self.entry.clone();
        entry.file = source;
        entry.at = entry.at.map(|(_, seq)| (source, seq));
        let order = Order {
            key: if self.head {
                (i64::MIN, 0, 0, 0, 0)
            } else {
                native_stage::key(
                    several,
                    self.time.resolve(first_regular_time),
                    file_order,
                    entry.pos,
                    self.phase,
                )
            },
            ordinal: self.ordinal,
            identity: self.identity,
        };
        (order, entry)
    }
    pub(crate) fn observe(facts: &mut Facts, leaf: Leaf<'_>) -> Result<(), &'static str> {
        // Only native event-backed members contribute event completion times.
        let found = leaf.entry.at.and(leaf.found);
        facts.observe(Leaf { found, ..leaf })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{Kind, Reply},
        model::ToolState,
    };

    #[test]
    fn native_head_after_and_equal_key_emission_keep_the_complete_builder_order() {
        let event = Event {
            o: u64::MAX,
            b: u32::MAX,
            t: Some(-40),
            ..Default::default()
        };
        for several in [false, true] {
            let (order, entry) =
                Candidate::head(91, event.o, 7, [3; 32]).qualify(9, 4, several, Some(400));
            assert_eq!(order.key, (i64::MIN, 0, 0, 0, 0));
            assert_eq!(
                (entry.file, entry.offset, entry.pos, entry.t, entry.at),
                (9, u64::MAX, (u64::MAX, 0), None, None)
            );
            let after = Candidate::after(91, &event, 7, SourceTime::BeforeFirst, [4; 32], 19);
            let (order, entry) = after.qualify(9, 4, several, Some(400));
            assert_eq!(
                order.key,
                (if several { 400 } else { 0 }, 4, u64::MAX, u32::MAX, 2)
            );
            assert_eq!(
                (entry.file, entry.offset, entry.t, entry.at),
                (9, u64::MAX, Some(-40), None)
            );
        }
        // This is the actual insertion sequence when placement keys coincide:
        // head, event members and after for each native regular ref, then extras.
        let expected = vec![
            EmissionOrder::head(),
            EmissionOrder::regular(0, 0),
            EmissionOrder::regular(0, 1),
            EmissionOrder::after(0),
            EmissionOrder::regular(1, 0),
            EmissionOrder::after(1),
            EmissionOrder::regular(usize::MAX - 1, usize::MAX - 1),
            EmissionOrder::operation(0),
            EmissionOrder::operation(1),
        ];
        let mut shuffled = expected.clone();
        shuffled.reverse();
        shuffled.sort();
        assert_eq!(shuffled, expected);
        for emission in expected {
            assert_eq!(
                serde_json::from_str::<EmissionOrder>(&serde_json::to_string(&emission).unwrap())
                    .unwrap(),
                emission
            );
        }
    }

    #[test]
    fn native_candidate_keeps_symbolic_prefix_and_qualifies_reference_and_merge_order() {
        let event = Event {
            k: Kind::U,
            o: 5,
            b: 2,
            ..Default::default()
        };
        let candidate = Candidate::regular(
            EventInput {
                seq: 7,
                event: &event,
                kind: EntryKind::U,
                inherited: SourceTime::BeforeFirst,
                operation: None,
                identity: [4; 32],
                ordinal: 9,
            },
            |_| panic!("ordinary candidate does not inspect a reply"),
        );
        let saved = serde_json::to_string(&candidate).unwrap();
        for (first, expected) in [(None, i64::MIN), (Some(40), 40)] {
            let restored: Candidate = serde_json::from_str(&saved).unwrap();
            let (order, entry) = restored.qualify(11, 2, true, first);
            assert_eq!(order.key, (expected, 2, 5, 2, 1));
            assert_eq!(order.ordinal, 9);
            assert_eq!(order.identity, [4; 32]);
            assert_eq!(entry.at, Some((11, 7)));
            assert_eq!(entry.file, 11);
            assert_eq!(serde_json::to_string(&restored).unwrap(), saved);
        }
        assert_eq!(
            candidate.qualify(11, 0, false, Some(40)).0.key,
            (0, 0, 5, 2, 1)
        );
    }

    #[test]
    fn native_candidate_uses_native_failed_and_pending_operation_placement() {
        let failed = Event {
            k: Kind::Tool,
            o: 3,
            r: Some(Reply {
                e: true,
                o: 8,
                b: 4,
                ..Default::default()
            }),
            ..Default::default()
        };
        let candidate = Candidate::regular(
            EventInput {
                seq: 2,
                event: &failed,
                kind: EntryKind::Tool(ToolState::Err),
                inherited: SourceTime::BeforeFirst,
                operation: Some((SourceTime::Observed(30), (6, 1))),
                identity: [0; 32],
                ordinal: 0,
            },
            |offset| {
                assert_eq!(offset, 8);
                SourceTime::BeforeFirst
            },
        );
        assert_eq!(
            candidate.qualify(0, 0, true, Some(40)).0.key,
            (40, 0, 8, 4, 1)
        );
        let pending = Event {
            k: Kind::Tool,
            o: 3,
            ..Default::default()
        };
        let candidate = Candidate::regular(
            EventInput {
                seq: 2,
                event: &pending,
                kind: EntryKind::Tool(ToolState::Pending),
                inherited: SourceTime::BeforeFirst,
                operation: Some((SourceTime::Observed(30), (6, 1))),
                identity: [0; 32],
                ordinal: 0,
            },
            |_| panic!("pending candidate has no reply lookup"),
        );
        assert_eq!(
            candidate.qualify(0, 0, true, Some(40)).0.key,
            (30, 0, 6, 1, 2)
        );
    }

    #[test]
    fn native_candidate_operation_annotation_is_not_an_ordinary_entry_or_event_completion() {
        let event = Event {
            k: Kind::Operation,
            o: 4,
            b: 3,
            t: Some(10),
            r: Some(Reply::default()),
            ..Default::default()
        };
        let candidate = Candidate::operation(8, &event, SourceTime::BeforeFirst, [0; 32], 0);
        let (order, entry) = candidate.qualify(5, 1, true, Some(40));
        assert_eq!(order.key, (10, 1, 4, 3, 1));
        assert!(entry.at.is_none());
        assert!(!native_stage::ordinary_slot(entry.kind, false));
        let mut facts = Facts::default();
        Candidate::observe(
            &mut facts,
            Leaf {
                order,
                entry: &entry,
                found: Some(&event),
                outgoing: None,
                boundary: false,
                count: 0,
            },
        )
        .unwrap();
        assert_eq!(
            facts
                .end
                .finish(None, true, false, None, "idle", || false)
                .unwrap()
                .at,
            None
        );
    }
}

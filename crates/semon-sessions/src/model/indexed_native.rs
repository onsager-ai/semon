//! Native payload for an ordered candidate forest. Stored source ranges may
//! only compose after native source/time/relationship qualification proves
//! their global order; a source-local extrema merge is not a group reduction.
use super::{
    native_producer::Candidate,
    range_stage::{Facts, Leaf},
    turn_stage::{GroupMark, GroupSummary},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrderedFacts {
    pub(crate) native: Facts,
    pub(crate) groups: GroupSummary,
}

impl OrderedFacts {
    /// Incoming handoffs have already passed the native first-membership
    /// predicate. No body, Index, source vector, or transcript is retained.
    pub(crate) fn observe(&mut self, leaf: Leaf<'_>, incoming: bool) -> Result<(), &'static str> {
        let groups = GroupSummary::single(GroupMark {
            order: leaf.order.clone(),
            entry: leaf.entry.clone(),
            incoming,
        });
        let mut native = Facts::default();
        Candidate::observe(&mut native, leaf)?;
        self.append(Self { native, groups })
    }

    /// Tree/forest parents use this model algebra only for adjacent globally
    /// ordered children. Rejecting overlapping streams prevents accidentally
    /// using unordered Facts.merge as a turn or group count.
    pub(crate) fn append(&mut self, later: Self) -> Result<(), &'static str> {
        if let (Some(last), Some(first)) = (&self.native.last, &later.native.first)
            && last.order >= first.order
        {
            return Err("native ordered ranges overlap");
        }
        let mut groups = self.groups.clone();
        groups.append(&later.groups)?;
        let mut native = self.native.clone();
        native.append(later.native)?;
        *self = Self { native, groups };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{Event, Kind},
        model::{
            ToolState,
            native_stage::{Entry, EntryKind},
            range_stage::Order,
            turn_stage::{EndFacts, HandoffEnd},
        },
    };

    #[test]
    fn native_ordered_payload_preserves_interleaved_end_and_group_boundaries() {
        for history in [0usize, 128, 1024, 8192] {
            let mut sequential = EndFacts::default();
            let mut whole = OrderedFacts::default();
            let mut page = OrderedFacts::default();
            let mut paged = OrderedFacts::default();
            let mut groups = 0;
            let mut open = false;
            for position in 0..history + 1 {
                let kind = match position % 7 {
                    0 => EntryKind::U,
                    1 => EntryKind::Tool(ToolState::Pending),
                    2 => EntryKind::H(position),
                    3 => EntryKind::Gap,
                    _ => EntryKind::A,
                };
                let incoming = matches!(kind, EntryKind::U);
                if matches!(kind, EntryKind::Gap) {
                    open = false;
                } else if incoming || !open {
                    groups += 1;
                    open = true;
                }
                let event = Event {
                    k: Kind::A,
                    t: Some(position as i64),
                    ..Default::default()
                };
                let entry = Entry {
                    kind,
                    file: position % 3,
                    offset: position as u64,
                    pos: ((history + 1 - position) as u64, 0),
                    t: event.t,
                    at: Some((position % 3, position)),
                };
                let outgoing = || {
                    matches!(kind, EntryKind::H(_)).then_some(HandoffEnd {
                        id: "handoff",
                        kind: "toyou",
                        status: "done",
                    })
                };
                sequential.observe(kind, Some(&event), outgoing());
                let leaf = || Leaf {
                    order: Order {
                        key: (position as i64, entry.file, entry.pos.0, 0, 1),
                        ordinal: position as u64,
                        identity: [0; 32],
                    },
                    entry: &entry,
                    found: Some(&event),
                    outgoing: outgoing(),
                    boundary: incoming,
                    count: 1,
                };
                whole.observe(leaf(), incoming).unwrap();
                page.observe(leaf(), incoming).unwrap();
                if position % 17 == 16 {
                    let restored: OrderedFacts =
                        serde_json::from_str(&serde_json::to_string(&page).unwrap()).unwrap();
                    paged.append(restored).unwrap();
                    page = OrderedFacts::default();
                }
            }
            paged.append(page).unwrap();
            assert_eq!(whole.native.count, (history + 1) as u64);
            assert_eq!(whole.groups.effect(false).groups, groups);
            assert_eq!(whole.groups.effect(false).open, open);
            assert_eq!(
                serde_json::to_value(&whole.native.end).unwrap(),
                serde_json::to_value(sequential).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&whole).unwrap(),
                serde_json::to_value(paged).unwrap()
            );
            assert!(serde_json::to_string(&whole).unwrap().len() < 4096);
        }
    }

    #[test]
    fn native_ordered_payload_refuses_interleaved_source_summary_and_keeps_previous_facts() {
        let mut payloads = [OrderedFacts::default(), OrderedFacts::default()];
        for position in 0..4 {
            let entry = Entry {
                kind: EntryKind::U,
                file: position % 2,
                offset: position as u64,
                pos: (position as u64, 0),
                t: Some(position as i64),
                at: None,
            };
            payloads[entry.file]
                .observe(
                    Leaf {
                        order: Order {
                            key: (position as i64, entry.file, position as u64, 0, 1),
                            ordinal: position as u64,
                            identity: [0; 32],
                        },
                        entry: &entry,
                        found: None,
                        outgoing: None,
                        boundary: true,
                        count: 1,
                    },
                    true,
                )
                .unwrap();
        }
        let [mut left, right] = payloads;
        let before = serde_json::to_value(&left).unwrap();
        assert_eq!(
            left.append(right).unwrap_err(),
            "native ordered ranges overlap"
        );
        assert_eq!(serde_json::to_value(left).unwrap(), before);
    }
}

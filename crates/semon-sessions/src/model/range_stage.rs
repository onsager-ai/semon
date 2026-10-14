//! Native scalar range facts. Storage treats this payload as opaque and uses
//! this ordered combine kernel; it never derives a second turn-end rule.
use super::{
    Event, Ref,
    native_stage::{Entry, EntryKey, EntryKind},
    turn_stage::{EndFacts, HandoffEnd},
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

pub(crate) type CandidateRef = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Order {
    pub(crate) key: EntryKey,
    pub(crate) ordinal: u64,
    /// Private candidate content address, never the raw native session ID.
    pub(crate) identity: CandidateRef,
}

impl Order {
    /// Fixed-width lexicographic encoding of the complete native placement
    /// key, including stable tie ordinal and point-address candidate identity.
    pub(crate) fn encoded(&self) -> [u8; 69] {
        let mut bytes = [0; 69];
        bytes[..8].copy_from_slice(&((self.key.0 as u64) ^ (1 << 63)).to_be_bytes());
        bytes[8..16].copy_from_slice(
            &u64::try_from(self.key.1)
                .expect("native file ordinal fits")
                .to_be_bytes(),
        );
        bytes[16..24].copy_from_slice(&self.key.2.to_be_bytes());
        bytes[24..28].copy_from_slice(&self.key.3.to_be_bytes());
        bytes[28] = self.key.4;
        bytes[29..37].copy_from_slice(&self.ordinal.to_be_bytes());
        bytes[37..].copy_from_slice(&self.identity);
        bytes
    }
}
impl Ord for Order {
    fn cmp(&self, other: &Self) -> Ordering {
        self.encoded().cmp(&other.encoded())
    }
}
impl PartialOrd for Order {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mark {
    pub(crate) order: Order,
    pub(crate) file: usize,
    pub(crate) offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentMark {
    order: Order,
    at: Option<Ref>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Facts {
    /// Native group members include zero-slot Operation entries. Keep this
    /// distinct from the caller's ordinary-slot weight in `count`.
    pub(crate) entries: u64,
    pub(crate) gaps: u64,
    pub(crate) boundaries: u64,
    pub(crate) count: u64,
    pub(crate) end: EndFacts,
    pub(crate) first: Option<Mark>,
    pub(crate) last: Option<Mark>,
    pub(crate) boundary: Option<Mark>,
    pub(crate) gap: Option<Mark>,
    /// A source-local prefix reaches every queued annotation strictly before
    /// this position, even when native timestamps move backwards.
    pub(crate) reached: Option<(u64, u32)>,
    content: Option<ContentMark>,
    to_you: Option<Order>,
}

pub(crate) struct Leaf<'a> {
    pub(crate) order: Order,
    pub(crate) entry: &'a Entry,
    pub(crate) found: Option<&'a Event>,
    pub(crate) outgoing: Option<HandoffEnd<'a>>,
    pub(crate) boundary: bool,
    pub(crate) count: u64,
}

impl Facts {
    pub(crate) fn observe(&mut self, leaf: Leaf<'_>) -> Result<(), &'static str> {
        let mut next = Self {
            entries: 1,
            gaps: u64::from(matches!(leaf.entry.kind, EntryKind::Gap)),
            boundaries: u64::from(leaf.boundary),
            count: leaf.count,
            ..Self::default()
        };
        let mark = Mark {
            order: leaf.order.clone(),
            file: leaf.entry.file,
            offset: leaf.entry.offset,
        };
        next.first = Some(mark.clone());
        next.last = Some(mark.clone());
        next.reached = Some(leaf.entry.pos);
        if matches!(leaf.entry.kind, EntryKind::Gap) {
            next.gap = Some(mark.clone());
        }
        if leaf.boundary {
            next.boundary = Some(mark);
        }
        if leaf
            .outgoing
            .as_ref()
            .is_some_and(|handoff| handoff.kind == "toyou")
        {
            next.to_you = Some(leaf.order.clone());
        }
        if next.end.observe(leaf.entry.kind, leaf.found, leaf.outgoing) {
            next.content = Some(ContentMark {
                order: leaf.order,
                at: leaf.entry.at,
            });
        }
        self.append(next)
    }

    /// Inputs are contiguous and already in native transcript order.
    pub(crate) fn append(&mut self, later: Self) -> Result<(), &'static str> {
        self.merge(later)
    }

    /// Merge source-local ranges after their symbolic times and file ordinals
    /// have been qualified. Ranges need not be contiguous in global order.
    /// `count` is the caller's native weight, not an inferred transcript size.
    pub(crate) fn merge(&mut self, other: Self) -> Result<(), &'static str> {
        let entries = self
            .entries
            .checked_add(other.entries)
            .ok_or("native entry count overflow")?;
        let gaps = self
            .gaps
            .checked_add(other.gaps)
            .ok_or("native gap count overflow")?;
        let boundaries = self
            .boundaries
            .checked_add(other.boundaries)
            .ok_or("native boundary count overflow")?;
        let count = self
            .count
            .checked_add(other.count)
            .ok_or("native range count overflow")?;
        let content_later = other.content.as_ref().is_some_and(|right| {
            self.content
                .as_ref()
                .is_none_or(|left| right.order > left.order)
        });
        let to_you_later = other
            .to_you
            .as_ref()
            .is_some_and(|right| self.to_you.as_ref().is_none_or(|left| right > left));
        self.end.merge(other.end, content_later, to_you_later);
        if other.first.as_ref().is_some_and(|right| {
            self.first
                .as_ref()
                .is_none_or(|left| right.order < left.order)
        }) {
            self.first = other.first;
        }
        if other.last.as_ref().is_some_and(|right| {
            self.last
                .as_ref()
                .is_none_or(|left| right.order > left.order)
        }) {
            self.last = other.last;
        }
        if other.boundary.as_ref().is_some_and(|right| {
            self.boundary
                .as_ref()
                .is_none_or(|left| right.order > left.order)
        }) {
            self.boundary = other.boundary;
        }
        if other.gap.as_ref().is_some_and(|right| {
            self.gap
                .as_ref()
                .is_none_or(|left| right.order > left.order)
        }) {
            self.gap = other.gap;
        }
        if content_later {
            self.content = other.content;
        }
        if to_you_later {
            self.to_you = other.to_you;
        }
        self.reached = self.reached.max(other.reached);
        self.count = count;
        self.entries = entries;
        self.gaps = gaps;
        self.boundaries = boundaries;
        Ok(())
    }

    /// Qualify one immutable source channel after physical source selection.
    /// `time` is supplied only for the symbolic BeforeFirst channel. Observed
    /// times remain in the local keys; Offset keys resolve to zero. No stored
    /// prefix is rewritten when the first regular timestamp or file order
    /// changes. Qualify before combining different source streams.
    pub(crate) fn qualify(
        &mut self,
        source: usize,
        file_order: usize,
        several: bool,
        time: Option<i64>,
    ) {
        let order = |order: &mut Order| {
            order.key.0 = if several {
                time.unwrap_or(order.key.0)
            } else {
                0
            };
            order.key.1 = if several { file_order } else { 0 };
        };
        for mark in [
            &mut self.first,
            &mut self.last,
            &mut self.boundary,
            &mut self.gap,
        ]
        .into_iter()
        .flatten()
        {
            order(&mut mark.order);
            mark.file = source;
        }
        if let Some(content) = &mut self.content {
            order(&mut content.order);
            if let Some((file, _)) = &mut content.at {
                *file = source;
            }
        }
        if let Some(to_you) = &mut self.to_you {
            order(to_you);
        }
    }

    pub(crate) fn last_content_at(&self) -> Option<Ref> {
        self.content.as_ref().and_then(|content| content.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ToolState, native_stage::EntryKind};

    #[test]
    fn native_range_order_encoding_preserves_signed_time_and_all_tie_fields() {
        let mut orders = Vec::new();
        for time in [i64::MIN, -1, 0, 1, i64::MAX] {
            for file in [0, 1] {
                for offset in [0, u64::MAX] {
                    for block in [0, u32::MAX] {
                        for phase in [0, 2] {
                            for ordinal in [0, u64::MAX] {
                                for identity in [[0; 32], [255; 32]] {
                                    orders.push(Order {
                                        key: (time, file, offset, block, phase),
                                        ordinal,
                                        identity,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        for pair in orders.windows(2) {
            assert_eq!(
                pair[0].encoded().cmp(&pair[1].encoded()),
                (&pair[0].key, pair[0].ordinal, &pair[0].identity).cmp(&(
                    &pair[1].key,
                    pair[1].ordinal,
                    &pair[1].identity
                ))
            );
            assert!(pair[0] < pair[1]);
        }
    }

    #[test]
    fn native_interleaved_source_ranges_match_global_order_and_lazy_content_reference() {
        let mut global = Facts::default();
        let mut sources = [Facts::default(), Facts::default(), Facts::default()];
        for position in 0..128usize {
            let file = position % sources.len();
            let kind = match position % 4 {
                0 => EntryKind::U,
                1 => EntryKind::A,
                2 => EntryKind::H(0),
                _ => EntryKind::Tool(ToolState::Pending),
            };
            let entry = Entry {
                kind,
                file,
                offset: position as u64,
                pos: ((128 - position) as u64, 0),
                t: Some(position as i64),
                at: Some((file, position)),
            };
            let event = Event {
                t: entry.t,
                ..Event::default()
            };
            let leaf = || Leaf {
                order: Order {
                    key: (position as i64, file, entry.pos.0, 0, 1),
                    ordinal: position as u64,
                    identity: [file as u8; 32],
                },
                entry: &entry,
                found: Some(&event),
                outgoing: matches!(kind, EntryKind::H(_)).then_some(HandoffEnd {
                    id: "native-handoff",
                    kind: "toyou",
                    status: "done",
                }),
                boundary: matches!(kind, EntryKind::U),
                count: 1,
            };
            global.observe(leaf()).unwrap();
            sources[file].observe(leaf()).unwrap();
        }
        let mut combined = Facts::default();
        for source in sources.into_iter().rev() {
            combined.merge(source).unwrap();
        }
        assert_eq!(
            serde_json::to_value(&global).unwrap(),
            serde_json::to_value(&combined).unwrap()
        );
        assert_eq!(combined.last_content_at(), Some((1, 127)));
        assert_eq!(combined.reached, Some((128, 0)));
        assert_eq!(combined.boundary.as_ref().unwrap().offset, 124);
        let overflow = Facts {
            count: u64::MAX,
            ..Default::default()
        };
        let before = serde_json::to_value(&combined).unwrap();
        assert!(combined.merge(overflow).is_err());
        assert_eq!(before, serde_json::to_value(combined).unwrap());
    }

    #[test]
    fn native_range_payload_matches_sequential_end_and_ordered_pages() {
        let mut sequential = EndFacts::default();
        let mut whole = Facts::default();
        let mut pages = Vec::new();
        let mut page = Facts::default();
        for position in 0..1024usize {
            let kind = match position % 4 {
                0 => EntryKind::U,
                1 => EntryKind::A,
                2 => EntryKind::Tool(ToolState::Pending),
                _ => EntryKind::H(0),
            };
            let entry = Entry {
                kind,
                file: 0,
                offset: position as u64,
                pos: (position as u64, 0),
                t: Some(position as i64),
                at: Some((0, position)),
            };
            let event = Event {
                t: entry.t,
                ..Event::default()
            };
            let outgoing = || {
                matches!(kind, EntryKind::H(_)).then_some(HandoffEnd {
                    id: "to-you",
                    kind: "toyou",
                    status: "done",
                })
            };
            sequential.observe(kind, Some(&event), outgoing());
            let leaf = || Leaf {
                order: Order {
                    key: (0, 0, position as u64, 0, 1),
                    ordinal: 0,
                    identity: [0; 32],
                },
                entry: &entry,
                found: Some(&event),
                outgoing: outgoing(),
                boundary: matches!(kind, EntryKind::U),
                count: 1,
            };
            whole.observe(leaf()).unwrap();
            page.observe(leaf()).unwrap();
            if position % 13 == 12 {
                pages.push(page);
                page = Facts::default();
            }
        }
        pages.push(page);
        let mut combined = Facts::default();
        for page in pages {
            let serialized = serde_json::to_string(&page).unwrap();
            combined
                .append(serde_json::from_str(&serialized).unwrap())
                .unwrap();
        }
        assert_eq!(
            serde_json::to_value(&whole).unwrap(),
            serde_json::to_value(&combined).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&sequential).unwrap(),
            serde_json::to_value(&combined.end).unwrap()
        );
        assert_eq!(combined.count, 1024);
        assert_eq!(combined.first.as_ref().unwrap().offset, 0);
        assert_eq!(combined.last.as_ref().unwrap().offset, 1023);
        assert_eq!(combined.boundary.as_ref().unwrap().offset, 1020);
        assert_eq!(combined.last_content_at(), Some((0, 1023)));
        assert!(serde_json::to_string(&combined).unwrap().len() < 4096);
    }

    #[test]
    fn native_range_symbolic_qualification_matches_interleaved_native_observations() {
        use crate::model::{
            native_producer::{Candidate, EventInput},
            native_stage::SourceTime,
        };
        for history in [0usize, 128, 1024, 8192] {
            for first_time in [None, Some(-40), Some(400)] {
                let mut qualified = Vec::new();
                let mut channels = [Facts::default(), Facts::default(), Facts::default()];
                for seq in 0..history + 3 {
                    let source = seq % 3;
                    let kind = match seq % 7 {
                        0 => EntryKind::U,
                        1 => EntryKind::Gap,
                        2 => EntryKind::Operation(crate::model::ToolState::Ok),
                        _ => EntryKind::A,
                    };
                    let event = Event {
                        k: crate::events::Kind::A,
                        o: seq as u64,
                        b: (seq % 2) as u32,
                        t: (source != 0).then_some((seq as i64 % 13) - 6),
                        ..Event::default()
                    };
                    let time = event
                        .t
                        .map_or(SourceTime::BeforeFirst, SourceTime::Observed);
                    let candidate = Candidate::regular(
                        EventInput {
                            seq,
                            event: &event,
                            kind,
                            inherited: time,
                            operation: None,
                            identity: [source as u8; 32],
                            ordinal: seq as u64,
                        },
                        |_| unreachable!(),
                    );
                    let (local, entry) = candidate.qualify(0, 0, true, Some(0));
                    channels[source]
                        .observe(Leaf {
                            order: local,
                            entry: &entry,
                            found: Some(&event),
                            outgoing: None,
                            boundary: matches!(kind, EntryKind::U),
                            count: u64::from(!matches!(kind, EntryKind::Operation(_))),
                        })
                        .unwrap();
                    let (global, entry) =
                        candidate.qualify(17 + source, 2 - source, true, first_time);
                    qualified.push((global, entry, event));
                }
                let mut combined = Facts::default();
                for (source, mut facts) in channels.into_iter().enumerate() {
                    facts.qualify(
                        17 + source,
                        2 - source,
                        true,
                        (source == 0).then_some(first_time.unwrap_or(i64::MIN)),
                    );
                    combined.merge(facts).unwrap();
                }
                qualified.sort_by(|a, b| a.0.cmp(&b.0));
                let mut sequential = Facts::default();
                for (order, entry, event) in &qualified {
                    sequential
                        .observe(Leaf {
                            order: order.clone(),
                            entry,
                            found: Some(event),
                            outgoing: None,
                            boundary: matches!(entry.kind, EntryKind::U),
                            count: u64::from(!matches!(entry.kind, EntryKind::Operation(_))),
                        })
                        .unwrap();
                }
                assert_eq!(
                    serde_json::to_value(&combined).unwrap(),
                    serde_json::to_value(&sequential).unwrap(),
                    "history={history}, first={first_time:?}"
                );
                assert_eq!(combined.entries, (history + 3) as u64);
                assert!(combined.gaps > 0);
                assert!(combined.boundaries > 0);
                assert!(combined.count < combined.entries);
            }
        }
    }

    #[test]
    fn native_range_new_counter_overflows_keep_all_previous_facts() {
        for field in ["entries", "gaps", "boundaries"] {
            let mut left = Facts::default();
            let mut right = Facts::default();
            match field {
                "entries" => {
                    left.entries = u64::MAX;
                    right.entries = 1;
                }
                "gaps" => {
                    left.gaps = u64::MAX;
                    right.gaps = 1;
                }
                _ => {
                    left.boundaries = u64::MAX;
                    right.boundaries = 1;
                }
            }
            let before = serde_json::to_value(&left).unwrap();
            assert!(left.merge(right).is_err());
            assert_eq!(serde_json::to_value(&left).unwrap(), before);
        }
    }

    #[test]
    fn native_range_count_overflow_keeps_original_facts() {
        let mut left = Facts {
            count: u64::MAX,
            ..Facts::default()
        };
        let before = serde_json::to_value(&left).unwrap();
        assert!(
            left.append(Facts {
                count: 1,
                ..Facts::default()
            })
            .is_err()
        );
        assert_eq!(serde_json::to_value(left).unwrap(), before);
    }

    #[test]
    fn native_range_marks_retain_stable_equal_key_placements() {
        let mut facts = Facts::default();
        let entry = Entry {
            kind: EntryKind::H(0),
            file: 0,
            offset: 0,
            pos: (0, 0),
            t: None,
            at: None,
        };
        for (ordinal, id) in [(0, "first"), (1, "second")] {
            facts
                .observe(Leaf {
                    order: Order {
                        key: (i64::MIN, 0, 0, 0, 0),
                        ordinal,
                        identity: [u8::try_from(ordinal + 1).unwrap(); 32],
                    },
                    entry: &entry,
                    found: None,
                    outgoing: Some(HandoffEnd {
                        id,
                        kind: "toyou",
                        status: "done",
                    }),
                    boundary: true,
                    count: 1,
                })
                .unwrap();
        }
        assert_eq!(facts.first.as_ref().unwrap().order.identity, [1; 32]);
        assert_eq!(facts.last.as_ref().unwrap().order.ordinal, 1);
        assert_eq!(facts.boundary.as_ref().unwrap().order.identity, [2; 32]);
        assert_eq!(
            facts
                .end
                .finish(None, true, false, None, "idle", || panic!("unneeded yield"))
                .unwrap()
                .h
                .as_deref(),
            Some("second")
        );
    }
}

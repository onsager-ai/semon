//! Native scalar range facts. Storage treats this payload as opaque and uses
//! this ordered combine kernel; it never derives a second turn-end rule.
use super::{
    Event, Ref,
    native_stage::{Entry, EntryKey},
    turn_stage::{EndFacts, HandoffEnd},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mark {
    pub(crate) key: EntryKey,
    pub(crate) file: usize,
    pub(crate) offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentMark {
    key: EntryKey,
    at: Option<Ref>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Facts {
    pub(crate) count: u64,
    pub(crate) end: EndFacts,
    pub(crate) first: Option<Mark>,
    pub(crate) last: Option<Mark>,
    pub(crate) boundary: Option<Mark>,
    content: Option<ContentMark>,
    to_you: Option<EntryKey>,
}

pub(crate) struct Leaf<'a> {
    pub(crate) key: EntryKey,
    pub(crate) entry: &'a Entry,
    pub(crate) found: Option<&'a Event>,
    pub(crate) outgoing: Option<HandoffEnd<'a>>,
    pub(crate) boundary: bool,
    pub(crate) count: u64,
}

impl Facts {
    pub(crate) fn observe(&mut self, leaf: Leaf<'_>) -> Result<(), &'static str> {
        let mut next = Self {
            count: leaf.count,
            ..Self::default()
        };
        let mark = Mark {
            key: leaf.key,
            file: leaf.entry.file,
            offset: leaf.entry.offset,
        };
        next.first = Some(mark.clone());
        next.last = Some(mark.clone());
        if leaf.boundary {
            next.boundary = Some(mark);
        }
        if leaf
            .outgoing
            .as_ref()
            .is_some_and(|handoff| handoff.kind == "toyou")
        {
            next.to_you = Some(leaf.key);
        }
        if next.end.observe(leaf.entry.kind, leaf.found, leaf.outgoing) {
            next.content = Some(ContentMark {
                key: leaf.key,
                at: leaf.entry.at,
            });
        }
        self.append(next)
    }

    /// Inputs are contiguous and already in native transcript order.
    pub(crate) fn append(&mut self, later: Self) -> Result<(), &'static str> {
        let count = self
            .count
            .checked_add(later.count)
            .ok_or("native range count overflow")?;
        self.end.append(later.end);
        if self.first.is_none() {
            self.first = later.first;
        }
        if later.last.is_some() {
            self.last = later.last;
        }
        if later.boundary.is_some() {
            self.boundary = later.boundary;
        }
        if later.content.is_some() {
            self.content = later.content;
        }
        if later.to_you.is_some() {
            self.to_you = later.to_you;
        }
        self.count = count;
        Ok(())
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
                key: (0, 0, position as u64, 0, 1),
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
        assert!(serde_json::to_string(&combined).unwrap().len() < 1024);
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
}

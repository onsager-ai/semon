//! Stateful native producer operations over indexed, absolute event addresses.
//!
//! The complete producer and derived-metadata repair share these decisions.
//! Implementations own transaction/cancellation fences: a failed operation
//! must roll back the whole record, including pending-call changes. No method
//! requires a complete source index. Point reads return one metadata event;
//! turn closure visits at most 64 pending calls per resumable step.

use super::{Event, FileIndex, Kind, Reply, UNKNOWN};
use std::convert::Infallible;

pub(crate) const CLOSE_PAGE: usize = 64;

pub(crate) trait ToolStore {
    type Error;
    fn event_count(&self) -> Result<usize, Self::Error>;
    fn event(&self, sequence: usize) -> Result<Option<Event>, Self::Error>;
    fn replace_event(&mut self, sequence: usize, event: Event) -> Result<(), Self::Error>;
    fn append_event(&mut self, event: Event) -> Result<usize, Self::Error>;
    fn pending(&self, id: &str) -> Result<Option<usize>, Self::Error>;
    fn take_pending(&mut self, id: &str) -> Result<Option<usize>, Self::Error>;
    fn set_pending(&mut self, id: &str, sequence: usize) -> Result<(), Self::Error>;
    fn add_tool_id(&mut self, id: &str, sequence: usize) -> Result<(), Self::Error>;
    /// Newest exact occurrence, optionally requiring a non-code-mode call.
    /// `floor` is an absolute event address, not a number of matching calls.
    fn latest_tool(
        &self,
        id: &str,
        plain: bool,
        floor: usize,
    ) -> Result<Option<usize>, Self::Error>;
    /// Native late-exit fallback scans the last 512 events, even when a
    /// persisted occurrence map is absent. The floor remains absolute.
    fn recent_tool(&self, id: &str, floor: usize) -> Result<Option<usize>, Self::Error>;
    /// Exactly one open code-mode pending call, otherwise None. Indexed
    /// implementations need at most two matching rows to establish ambiguity.
    fn unique_open_code_mode(&self) -> Result<Option<usize>, Self::Error>;
}

pub(crate) fn resolve<S: ToolStore>(
    store: &mut S,
    id: &str,
    reply: impl FnOnce(Option<&str>) -> Reply,
) -> Result<(), S::Error> {
    let Some(sequence) = store.take_pending(id)? else {
        return Ok(());
    };
    if let Some(mut event) = store.event(sequence)?
        && event.k == Kind::Tool
    {
        event.r = Some(reply(event.n.as_deref()));
        event.code_mode_open = false;
        store.replace_event(sequence, event)?;
    }
    Ok(())
}

pub(crate) fn tool_event<S: ToolStore>(store: &mut S, event: Event) -> Result<(), S::Error> {
    let id = event.id.clone();
    let pending = event.r.is_none();
    let sequence = store.append_event(event)?;
    if let Some(id) = id {
        store.add_tool_id(&id, sequence)?;
        if pending {
            store.set_pending(&id, sequence)?;
        }
    }
    Ok(())
}

pub(crate) fn link_item<S: ToolStore>(
    store: &mut S,
    id: &str,
    offset: u64,
) -> Result<(), S::Error> {
    if let Some(sequence) = store.latest_tool(id, true, 0)?
        && let Some(mut event) = store.event(sequence)?
    {
        event.item = Some(offset);
        store.replace_event(sequence, event)?;
    }
    Ok(())
}

pub(crate) fn exit_code<S: ToolStore>(
    store: &mut S,
    id: &str,
    code: i64,
    offset: u64,
    time: Option<i64>,
) -> Result<(), S::Error> {
    if store.pending(id)?.is_some() {
        return resolve(store, id, |_| Reply {
            o: offset,
            t: time,
            e: code != 0,
            ..Reply::default()
        });
    }
    // Preserve the native last-512-events limit, including non-tool events.
    let floor = store.event_count()?.saturating_sub(512);
    if let Some(sequence) = store.recent_tool(id, floor)?
        && let Some(mut event) = store.event(sequence)?
        && let Some(reply) = event.r.as_mut()
    {
        reply.e = code != 0;
        reply.f &= !UNKNOWN;
        store.replace_event(sequence, event)?;
    }
    Ok(())
}

/// Close one already bounded page. The durable producer obtains at most
/// CLOSE_PAGE pending addresses from its indexed keyset reader and checkpoints
/// that reader's cursor atomically with these updates. The record advances
/// only after every page is committed; pending calls remain pending.
pub(crate) fn close_code_mode_page<S: ToolStore>(
    store: &mut S,
    pending: &[usize],
) -> Result<(), S::Error> {
    assert!(pending.len() <= CLOSE_PAGE);
    for &sequence in pending {
        close_event(store, sequence)?;
    }
    Ok(())
}

fn close_event<S: ToolStore>(store: &mut S, sequence: usize) -> Result<(), S::Error> {
    if let Some(mut event) = store.event(sequence)?
        && event.k == Kind::Tool
        && event.code_mode
    {
        event.code_mode_open = false;
        store.replace_event(sequence, event)?;
    }
    Ok(())
}

pub(super) fn close_complete(source: &mut FileIndex) {
    // Preserve the complete producer's one traversal. Only the durable adapter
    // uses the resumable page driver; ordinary append must not rescan this map
    // once for every page of pending calls.
    let pending: Vec<_> = source.pending.values().copied().collect();
    for page in pending.chunks(CLOSE_PAGE) {
        complete(close_code_mode_page(source, page));
    }
}

pub(super) fn complete<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(impossible) => match impossible {},
    }
}

impl ToolStore for FileIndex {
    type Error = Infallible;
    fn event_count(&self) -> Result<usize, Infallible> {
        Ok(self.events.len())
    }
    fn event(&self, sequence: usize) -> Result<Option<Event>, Infallible> {
        Ok(self.events.get(sequence).cloned())
    }
    fn replace_event(&mut self, sequence: usize, event: Event) -> Result<(), Infallible> {
        if let Some(existing) = self.event_mut(sequence) {
            *existing = event;
        }
        Ok(())
    }
    fn append_event(&mut self, event: Event) -> Result<usize, Infallible> {
        let sequence = self.events.len();
        self.events.push(event);
        Ok(sequence)
    }
    fn pending(&self, id: &str) -> Result<Option<usize>, Infallible> {
        Ok(self.pending.get(id).copied())
    }
    fn take_pending(&mut self, id: &str) -> Result<Option<usize>, Infallible> {
        Ok(self.pending.remove(id))
    }
    fn set_pending(&mut self, id: &str, sequence: usize) -> Result<(), Infallible> {
        self.pending.insert(id.to_owned(), sequence);
        Ok(())
    }
    fn add_tool_id(&mut self, id: &str, sequence: usize) -> Result<(), Infallible> {
        self.tool_ids
            .entry(id.to_owned())
            .or_default()
            .push(sequence);
        Ok(())
    }
    fn latest_tool(
        &self,
        id: &str,
        plain: bool,
        floor: usize,
    ) -> Result<Option<usize>, Infallible> {
        Ok(self.tool_ids.get(id).and_then(|indices| {
            indices.iter().rev().copied().find(|&sequence| {
                sequence >= floor
                    && self
                        .events
                        .get(sequence)
                        .is_some_and(|event| event.k == Kind::Tool && (!plain || !event.code_mode))
            })
        }))
    }
    fn recent_tool(&self, id: &str, floor: usize) -> Result<Option<usize>, Infallible> {
        Ok(self
            .events
            .iter()
            .enumerate()
            .rev()
            .take(512)
            .find(|(sequence, event)| {
                *sequence >= floor && event.k == Kind::Tool && event.id.as_deref() == Some(id)
            })
            .map(|(sequence, _)| sequence))
    }
    fn unique_open_code_mode(&self) -> Result<Option<usize>, Infallible> {
        let mut open = self.pending.values().copied().filter(|&sequence| {
            self.events.get(sequence).is_some_and(|event| {
                event.k == Kind::Tool && event.code_mode && event.code_mode_open
            })
        });
        Ok(open.next().filter(|_| open.next().is_none()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_resume_preserves_pending_and_absolute_addresses() {
        let mut source = FileIndex::default();
        for sequence in 0..131 {
            complete(tool_event(
                &mut source,
                Event {
                    k: Kind::Tool,
                    id: Some(format!("call-{sequence}")),
                    code_mode: sequence % 2 == 0,
                    code_mode_open: sequence % 2 == 0,
                    ..Event::default()
                },
            ));
        }
        let first: Vec<_> = (0..64).collect();
        complete(close_code_mode_page(&mut source, &first));
        assert!(!source.events[62].code_mode_open);
        assert!(source.events[64].code_mode_open);
        // A restart restores the committed metadata and continuation, not a
        // different event numbering or a fresh whole-history fold.
        let mut restored = source.clone();
        let second: Vec<_> = (64..128).collect();
        complete(close_code_mode_page(&mut restored, &second));
        complete(close_code_mode_page(&mut restored, &[128, 129, 130]));
        assert_eq!(restored.pending.len(), 131);
        assert!(restored.events.iter().all(|event| !event.code_mode_open));
    }

    #[test]
    fn late_exit_uses_event_floor_and_does_not_require_occurrence_map() {
        for padding in [511, 512] {
            let mut source = FileIndex::default();
            source.events.push(Event {
                k: Kind::Tool,
                id: Some("tool".into()),
                r: Some(Reply {
                    f: UNKNOWN,
                    ..Reply::default()
                }),
                ..Event::default()
            });
            source.events.extend((0..padding).map(|_| Event {
                k: Kind::A,
                ..Event::default()
            }));
            complete(exit_code(&mut source, "tool", 1, 99, None));
            let reply = source.events[0].r.as_ref().unwrap();
            assert_eq!(reply.e, padding == 511);
            assert_eq!(reply.f & UNKNOWN == 0, padding == 511);
        }
    }

    #[test]
    fn exact_newest_pending_owner_and_code_mode_ambiguity() {
        let mut source = FileIndex::default();
        for offset in [10, 20] {
            complete(tool_event(
                &mut source,
                Event {
                    k: Kind::Tool,
                    o: offset,
                    id: Some("same".into()),
                    code_mode: true,
                    code_mode_open: true,
                    ..Event::default()
                },
            ));
        }
        assert_eq!(complete(source.unique_open_code_mode()), Some(1));
        complete(tool_event(
            &mut source,
            Event {
                k: Kind::Tool,
                id: Some("other".into()),
                code_mode: true,
                code_mode_open: true,
                ..Event::default()
            },
        ));
        assert_eq!(complete(source.unique_open_code_mode()), None);
        complete(resolve(&mut source, "same", |_| Reply {
            o: 30,
            ..Reply::default()
        }));
        assert!(source.events[0].r.is_none());
        assert_eq!(source.events[1].r.as_ref().unwrap().o, 30);
        assert_eq!(complete(source.unique_open_code_mode()), Some(2));
    }
}

//! Stateful native producer operations over indexed, absolute event addresses.
//!
//! The complete producer and derived-metadata repair share these decisions.
//! Implementations own transaction/cancellation fences: a failed operation
//! must roll back the whole record, including pending-call changes. No method
//! requires a complete source index. Point reads return one metadata event;
//! turn closure visits at most 64 pending calls per resumable step.

use super::{
    Event, FileIndex, Kind, POLLS_MAX, Reply, STDIN, Signal, SignalKind, UNKNOWN, YIELDS_MAX,
};
use std::collections::BTreeMap;
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

pub(crate) trait SignalStore: ToolStore {
    fn last_signal(&self, kind: Option<SignalKind>)
    -> Result<Option<(usize, Signal)>, Self::Error>;
    fn append_signal(&mut self, signal: Signal) -> Result<(), Self::Error>;
    fn replace_signal(&mut self, sequence: usize, signal: Signal) -> Result<(), Self::Error>;
}

pub(crate) fn note<S: SignalStore>(
    store: &mut S,
    k: SignalKind,
    o: u64,
    t: Option<i64>,
    n: String,
    v: Option<u64>,
) -> Result<(), S::Error> {
    let at = u32::try_from(store.event_count()?).unwrap_or(u32::MAX);
    store.append_signal(Signal {
        k,
        o,
        t,
        at,
        n: Some(n),
        v,
    })
}

pub(crate) fn setting<S: SignalStore>(
    store: &mut S,
    k: SignalKind,
    o: u64,
    t: Option<i64>,
    value: Option<&str>,
) -> Result<(), S::Error> {
    let Some(value) = value.and_then(super::bounded) else {
        return Ok(());
    };
    if store
        .last_signal(Some(k))?
        .and_then(|(_, signal)| signal.n)
        .as_deref()
        != Some(value.as_str())
    {
        note(store, k, o, t, value, None)?;
    }
    Ok(())
}

pub(crate) fn signal<S: SignalStore>(
    store: &mut S,
    k: SignalKind,
    o: u64,
    t: Option<i64>,
    n: Option<&str>,
    v: Option<u64>,
) -> Result<(), S::Error> {
    let n = n
        .filter(|tag| {
            tag.len() <= 32
                && tag
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b' '))
        })
        .map(str::to_owned);
    let at = u32::try_from(store.event_count()?).unwrap_or(u32::MAX);
    if k == SignalKind::Compact
        && let Some((sequence, mut last)) = store.last_signal(None)?
        && last.k == SignalKind::Compact
        && last.at == at
    {
        if last.n.is_none() {
            last.n = n;
        }
        if last.v.is_none() {
            last.v = v;
        }
        if last.t.is_none() {
            last.t = t;
        }
        store.replace_signal(sequence, last)?;
        return Ok(());
    }
    store.append_signal(Signal { k, o, t, at, n, v })
}

pub(crate) fn push<S: ToolStore>(store: &mut S, event: Event) -> Result<(), S::Error> {
    if event.k == Kind::Gap
        && let Some(last) = store.event_count()?.checked_sub(1)
        && store.event(last)?.is_some_and(|last| last.k == Kind::Gap)
    {
        return Ok(());
    }
    store.append_event(event)?;
    Ok(())
}

impl SignalStore for FileIndex {
    fn last_signal(&self, kind: Option<SignalKind>) -> Result<Option<(usize, Signal)>, Infallible> {
        Ok(self
            .signals
            .iter()
            .enumerate()
            .rev()
            .find(|(_, signal)| kind.map_or_else(|| !signal.k.annotates(), |kind| signal.k == kind))
            .map(|(sequence, signal)| (sequence, signal.clone())))
    }
    fn append_signal(&mut self, signal: Signal) -> Result<(), Infallible> {
        self.signals.push(signal);
        Ok(())
    }
    fn replace_signal(&mut self, sequence: usize, signal: Signal) -> Result<(), Infallible> {
        if let Some(existing) = self.signal_mut(sequence) {
            *existing = signal;
        }
        Ok(())
    }
}

/// Native yields already have a fixed 64-session bound, with session IDs at
/// most 64 bytes. Durable adapters may retain this scalar native state; event
/// payloads and completed operation history remain point/indexed reads.
pub(crate) trait YieldStore: ToolStore {
    fn yields(&self) -> &BTreeMap<String, usize>;
    fn yields_mut(&mut self) -> &mut BTreeMap<String, usize>;
    fn operation_after(&self, parent: usize, offset: u64) -> Result<bool, Self::Error>;
}

pub(crate) fn fold_yield<S: YieldStore>(
    store: &mut S,
    index: usize,
    session: Option<String>,
    offset: u64,
) -> Result<(), S::Error> {
    let Some(mut event) = store.event(index)? else {
        return Ok(());
    };
    let (call_offset, flags, linked) = (event.o, event.script, event.poll);
    let failed = event.r.as_ref().is_some_and(|reply| reply.e);
    let operations = store.operation_after(index, call_offset)?;
    let known = session
        .as_ref()
        .filter(|_| flags & STDIN != 0)
        .and_then(|session| store.yields().get(session).copied())
        .filter(|start| *start < index);
    if operations || failed {
        event.poll = None;
        store.replace_event(index, event)?;
        if let Some(start) = known {
            push_poll(store, start, offset)?;
        }
        return Ok(());
    }
    let ended_here = if let Some(start) = linked {
        store
            .event(start)?
            .and_then(|found| found.y)
            .and_then(|yielded| yielded.done)
            .filter(|done| done.o > call_offset)
            .map(|_| start)
    } else {
        None
    };
    let start = match session {
        None => ended_here,
        Some(_) if known.is_some() => known,
        Some(_) if ended_here.is_some() => ended_here,
        Some(session) => {
            if store.yields().len() >= YIELDS_MAX
                && !store.yields().contains_key(&session)
                && let Some(oldest) = store
                    .yields()
                    .iter()
                    .min_by_key(|(_, start)| **start)
                    .map(|(session, _)| session.clone())
            {
                store.yields_mut().remove(&oldest);
            }
            store.yields_mut().insert(session, index);
            event.poll = None;
            event.y = Some(Box::default());
            store.replace_event(index, event)?;
            return Ok(());
        }
    };
    let Some(start) = start.filter(|start| *start < index) else {
        event.poll = None;
        store.replace_event(index, event)?;
        return Ok(());
    };
    event.poll = Some(start);
    store.replace_event(index, event)?;
    push_poll(store, start, offset)
}

pub(crate) fn push_poll<S: ToolStore>(
    store: &mut S,
    start: usize,
    offset: u64,
) -> Result<(), S::Error> {
    if let Some(mut event) = store.event(start)?
        && let Some(yielded) = event.y.as_mut()
    {
        if yielded.polls.len() < POLLS_MAX {
            yielded.polls.push(offset);
        } else {
            yielded.cut = true;
        }
        store.replace_event(start, event)?;
    }
    Ok(())
}

impl YieldStore for FileIndex {
    fn yields(&self) -> &BTreeMap<String, usize> {
        &self.yields
    }
    fn yields_mut(&mut self) -> &mut BTreeMap<String, usize> {
        &mut self.yields
    }
    fn operation_after(&self, parent: usize, offset: u64) -> Result<bool, Infallible> {
        Ok(self
            .extras
            .iter()
            .rev()
            .take_while(|extra| extra.o > offset)
            .any(|extra| extra.k == Kind::Operation && extra.parent == Some(parent)))
    }
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
    fn compaction_and_settings_preserve_signal_placement_and_first_evidence() {
        let mut source = FileIndex::default();
        complete(signal(
            &mut source,
            SignalKind::Compact,
            10,
            None,
            None,
            None,
        ));
        complete(setting(
            &mut source,
            SignalKind::Effort,
            11,
            Some(11),
            Some("high"),
        ));
        complete(signal(
            &mut source,
            SignalKind::Compact,
            12,
            Some(12),
            Some("auto"),
            Some(9),
        ));
        assert_eq!(source.signals.len(), 2);
        assert_eq!(source.signals[0].o, 10);
        assert_eq!(source.signals[0].t, Some(12));
        assert_eq!(source.signals[0].n.as_deref(), Some("auto"));
        assert_eq!(source.signals[0].v, Some(9));
        complete(setting(
            &mut source,
            SignalKind::Effort,
            13,
            Some(13),
            Some("high"),
        ));
        assert_eq!(source.signals.len(), 2);
        complete(signal(
            &mut source,
            SignalKind::Interrupt,
            14,
            Some(14),
            Some("user"),
            None,
        ));
        complete(signal(
            &mut source,
            SignalKind::Compact,
            15,
            Some(15),
            None,
            None,
        ));
        assert_eq!(source.signals.len(), 4);
        complete(push(
            &mut source,
            Event {
                k: Kind::Gap,
                o: 16,
                ..Event::default()
            },
        ));
        complete(push(
            &mut source,
            Event {
                k: Kind::Gap,
                o: 17,
                ..Event::default()
            },
        ));
        assert_eq!(source.events.len(), 1);
        complete(signal(
            &mut source,
            SignalKind::Compact,
            18,
            Some(18),
            None,
            None,
        ));
        assert_eq!(source.signals.len(), 5);
        assert_eq!(source.signals[4].at, 1);
    }

    #[test]
    fn operation_and_failed_poll_keep_native_output_links() {
        for failed in [false, true] {
            let mut source = FileIndex::default();
            complete(tool_event(
                &mut source,
                Event {
                    k: Kind::Tool,
                    o: 10,
                    id: Some("start".into()),
                    code_mode: true,
                    code_mode_open: true,
                    r: Some(Reply::default()),
                    ..Event::default()
                },
            ));
            complete(fold_yield(&mut source, 0, Some("123".into()), 20));
            complete(tool_event(
                &mut source,
                Event {
                    k: Kind::Tool,
                    o: 30,
                    id: Some("poll".into()),
                    code_mode: true,
                    code_mode_open: true,
                    script: STDIN,
                    r: Some(Reply {
                        e: failed,
                        ..Reply::default()
                    }),
                    ..Event::default()
                },
            ));
            if !failed {
                source.extras.push(Event {
                    k: Kind::Operation,
                    o: 40,
                    parent: Some(1),
                    ..Event::default()
                });
            }
            complete(fold_yield(&mut source, 1, Some("123".into()), 50));
            assert_eq!(source.events[1].poll, None);
            assert_eq!(source.events[0].y.as_ref().unwrap().polls, vec![50]);
            assert_eq!(source.yields.get("123"), Some(&0));
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

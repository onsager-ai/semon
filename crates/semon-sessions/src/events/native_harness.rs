//! Native harness record transitions shared by the complete and durable producers.
//! Durable callers checkpoint a subrecord cursor in the same transaction as each
//! transition. A repeated Copilot call id invalidates every previous reply; the
//! reset therefore advances through exact indexed occurrences before appending
//! the new request, rather than admitting a prefix of that request as complete.

use super::native_event::NativeReply;
use super::native_producer::ToolStore;
use super::{DENIED, FileIndex, UNKNOWN};
use std::convert::Infallible;

pub(crate) trait CopilotStore: ToolStore {
    /// The first exact native occurrence after an absolute event address.
    /// None starts at the first occurrence. The returned address is never a
    /// rank in the matching subset, and identity is a qualified native class.
    fn tool_occurrence_after(
        &self,
        id: &Self::Identity,
        after: Option<usize>,
    ) -> Result<Option<usize>, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reset {
    Progress(usize),
    Complete,
}

pub(crate) fn copilot_reset_reply_step<S: CopilotStore>(
    store: &mut S,
    id: &S::Identity,
    after: Option<usize>,
) -> Result<Reset, S::Error> {
    let Some(sequence) = store.tool_occurrence_after(id, after)? else {
        return Ok(Reset::Complete);
    };
    if let Some(mut event) = store.event(sequence)? {
        event.r = None;
        store.replace_event(sequence, event)?;
    }
    Ok(Reset::Progress(sequence))
}

/// Native Copilot accepts completion only for exactly one historical request.
/// Orphan starts and ambiguous ids never acquire a request by proximity.
pub(crate) fn copilot_completion_target<S: CopilotStore>(
    store: &S,
    id: &S::Identity,
) -> Result<Option<usize>, S::Error> {
    let Some(first) = store.tool_occurrence_after(id, None)? else {
        return Ok(None);
    };
    if store.tool_occurrence_after(id, Some(first))?.is_some() {
        return Ok(None);
    }
    Ok(Some(first))
}

/// is_bash must come from the native name owner: String equality in the
/// complete producer, qualified descriptor comparison in Original repair.
pub(crate) fn copilot_completion_reply<T>(
    offset: u64,
    time: Option<i64>,
    success: Option<bool>,
    exit: Option<i64>,
    denied: bool,
    is_bash: bool,
) -> NativeReply<T> {
    let mut flags = if denied { DENIED } else { 0 };
    if success.is_none() || (is_bash && exit.is_none() && success == Some(true)) {
        flags |= UNKNOWN;
    }
    NativeReply {
        o: offset,
        t: time,
        e: success == Some(false) || exit.is_some_and(|code| code != 0),
        f: flags,
        ..Default::default()
    }
}

impl CopilotStore for FileIndex {
    fn tool_occurrence_after(
        &self,
        id: &str,
        after: Option<usize>,
    ) -> Result<Option<usize>, Infallible> {
        Ok(self.tool_ids.get(id).and_then(|indices| {
            let start = after.map_or(0, |after| indices.partition_point(|index| *index <= after));
            indices.get(start).copied()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::native_producer::{complete, tool_event};
    use crate::events::{Event, Kind, Reply};

    #[test]
    fn copilot_repeated_id_reset_advances_all_absolute_occurrences() {
        for count in [0usize, 1, 128, 1024, 8192] {
            let mut source = FileIndex::default();
            for index in 0..count {
                source.events.push(Event {
                    k: Kind::A,
                    ..Default::default()
                });
                complete(tool_event(
                    &mut source,
                    Event {
                        k: Kind::Tool,
                        id: Some("same".into()),
                        o: index as u64,
                        r: Some(Reply {
                            e: true,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ));
            }
            let mut after = None;
            let mut reset = 0;
            loop {
                match complete(copilot_reset_reply_step(&mut source, "same", after)) {
                    Reset::Complete => break,
                    Reset::Progress(sequence) => {
                        assert_eq!(sequence, reset * 2 + 1);
                        assert!(source.events[sequence].r.is_none());
                        after = Some(sequence);
                        reset += 1;
                    }
                }
            }
            assert_eq!(reset, count);
            assert_eq!(
                complete(copilot_completion_target(&source, "same")),
                (count == 1).then_some(1)
            );
        }
    }

    #[test]
    fn copilot_completion_preserves_native_unknown_denied_and_exit_rules() {
        for success in [None, Some(false), Some(true)] {
            for exit in [None, Some(0), Some(1)] {
                for denied in [false, true] {
                    for bash in [false, true] {
                        let reply = copilot_completion_reply::<String>(
                            7,
                            Some(11),
                            success,
                            exit,
                            denied,
                            bash,
                        );
                        assert_eq!(
                            reply.e,
                            success == Some(false) || exit.is_some_and(|code| code != 0)
                        );
                        assert_eq!(reply.f & DENIED != 0, denied);
                        assert_eq!(
                            reply.f & UNKNOWN != 0,
                            success.is_none() || (bash && exit.is_none() && success == Some(true))
                        );
                        assert_eq!((reply.o, reply.t, reply.m), (7, Some(11), None));
                    }
                }
            }
        }
    }
}

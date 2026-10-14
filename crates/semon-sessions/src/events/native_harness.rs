//! Native harness record transitions shared by the complete and durable producers.
//! Durable callers checkpoint a subrecord cursor in the same transaction as each
//! transition. A repeated Copilot call id invalidates every previous reply; the
//! reset therefore advances through exact indexed occurrences before appending
//! the new request, rather than admitting a prefix of that request as complete.

pub(crate) mod activity;
pub(crate) mod copilot;
pub(crate) mod copilot_admission;
pub(crate) mod header;
pub(crate) mod record;
pub(crate) mod tag;
pub(crate) mod time;

use super::native_event::NativeReply;
use super::native_producer::ToolStore;
use super::{DENIED, FileIndex, UNKNOWN};
use std::convert::Infallible;

pub(crate) trait ExtraStore: ToolStore {
    fn append_extra(
        &mut self,
        event: super::native_event::NativeEvent<Self::Text>,
    ) -> Result<(), Self::Error>;
}
impl ExtraStore for FileIndex {
    fn append_extra(
        &mut self,
        event: super::native_event::NativeEvent<String>,
    ) -> Result<(), Infallible> {
        self.extras.push(event.into());
        Ok(())
    }
}

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

/// Copilot admission deliberately precedes activity, timestamp observations and
/// every event action. A non-start record must not poison the seen-id set.
pub(crate) trait CopilotIdentityStore: ToolStore {
    fn copilot_started(&self) -> Result<bool, Self::Error>;
    fn copilot_record_seen(&mut self, identity: &Self::Text) -> Result<bool, Self::Error>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Admission {
    Gap,
    BeforeStart,
    Duplicate,
    Accepted,
}
/// The field owner filters the native empty id before calling this action.
/// initialize retains the native version/session metadata at its existing owner.
pub(crate) fn copilot_admit<S: CopilotIdentityStore>(
    store: &mut S,
    identity: Option<&S::Text>,
    is_start: bool,
    initialize: impl FnOnce(&mut S) -> Result<(), S::Error>,
) -> Result<Admission, S::Error> {
    let Some(identity) = identity else {
        return Ok(Admission::Gap);
    };
    if !store.copilot_started()? {
        if !is_start {
            return Ok(Admission::BeforeStart);
        }
        initialize(store)?;
    }
    Ok(if store.copilot_record_seen(identity)? {
        Admission::Duplicate
    } else {
        Admission::Accepted
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordIdentity {
    Progress(usize),
    Complete,
}

/// Claude applies the record UUID after every regular event action. The
/// complete producer's reverse suffix walk and Original's durable cursor use
/// the same offset stop rule. Extras retain their original UUID behavior.
pub(crate) fn claude_record_identity_step<S: ToolStore>(
    store: &mut S,
    offset: u64,
    identity: &S::Text,
    before: Option<usize>,
) -> Result<RecordIdentity, S::Error> {
    let before = match before {
        Some(before) => before,
        None => store.event_count()?,
    };
    let Some(sequence) = before.checked_sub(1) else {
        return Ok(RecordIdentity::Complete);
    };
    let Some(mut event) = store.event(sequence)? else {
        return Ok(RecordIdentity::Complete);
    };
    if event.o != offset {
        return Ok(RecordIdentity::Complete);
    }
    event.u = Some(identity.clone());
    store.replace_event(sequence, event)?;
    Ok(RecordIdentity::Progress(sequence))
}

/// A turn boundary's existing logical page still contains at most 64 pending
/// addresses. Original commits only a small subpage: event reads and two row
/// writes per call leave room for indexed address selection and cursor seals
/// inside the durable adapter's unchanged 64-row step budget.
pub(crate) const CLOSE_STEP: usize = 12;
pub(crate) fn codex_boundary_step<S: ToolStore>(
    store: &mut S,
    pending: &[usize],
    after: usize,
) -> Result<usize, S::Error> {
    assert!(pending.len() <= super::native_producer::CLOSE_PAGE);
    assert!(after <= pending.len());
    let end = after.saturating_add(CLOSE_STEP).min(pending.len());
    super::native_producer::close_code_mode_page(store, &pending[after..end])?;
    Ok(end)
}

impl CopilotIdentityStore for FileIndex {
    fn copilot_started(&self) -> Result<bool, Infallible> {
        Ok(self.copilot.is_some())
    }
    fn copilot_record_seen(&mut self, identity: &String) -> Result<bool, Infallible> {
        Ok(!self
            .copilot
            .as_mut()
            .expect("initialized")
            .seen
            .insert(identity.clone()))
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
    fn copilot_admission_checks_start_before_remembering_record_identity() {
        let mut source = FileIndex::default();
        let identity = "record".to_owned();
        assert_eq!(
            complete(copilot_admit(
                &mut source,
                Some(&identity),
                false,
                |_| unreachable!()
            )),
            Admission::BeforeStart
        );
        assert!(source.copilot.is_none());
        assert_eq!(
            complete(copilot_admit(&mut source, None, true, |_| unreachable!())),
            Admission::Gap
        );
        assert_eq!(
            complete(copilot_admit(
                &mut source,
                Some(&identity),
                true,
                |source| {
                    source.copilot = Some(Default::default());
                    Ok(())
                }
            )),
            Admission::Accepted
        );
        assert_eq!(
            complete(copilot_admit(
                &mut source,
                Some(&identity),
                true,
                |_| unreachable!()
            )),
            Admission::Duplicate
        );
        assert_eq!(source.copilot.as_ref().unwrap().seen.len(), 1);
    }

    #[test]
    fn claude_uuid_suffix_matches_independent_reverse_record_reference() {
        for count in [0usize, 1, 128, 1024, 8192] {
            let mut source = FileIndex::default();
            source.events.push(Event {
                k: Kind::U,
                o: 5,
                u: Some("old".into()),
                ..Default::default()
            });
            for _ in 0..count {
                source.events.push(Event {
                    k: Kind::A,
                    o: 7,
                    ..Default::default()
                });
            }
            let mut expected = source.events.clone();
            for event in expected.iter_mut().rev().take_while(|event| event.o == 7) {
                event.u = Some("record".into());
            }
            let mut before = None;
            let mut visits = 0;
            loop {
                match complete(claude_record_identity_step(
                    &mut source,
                    7,
                    &"record".into(),
                    before,
                )) {
                    RecordIdentity::Complete => break,
                    RecordIdentity::Progress(sequence) => {
                        before = Some(sequence);
                        visits += 1;
                    }
                }
            }
            assert_eq!(visits, count);
            assert_eq!(source.events, expected);
        }
    }

    #[test]
    fn codex_boundary_substeps_preserve_logical_page_and_native_pending_map() {
        let mut source = FileIndex::default();
        for sequence in 0..super::super::native_producer::CLOSE_PAGE {
            complete(tool_event(
                &mut source,
                Event {
                    k: Kind::Tool,
                    id: Some(sequence.to_string()),
                    code_mode: true,
                    code_mode_open: true,
                    ..Default::default()
                },
            ));
        }
        let pending: Vec<_> = source.pending.values().copied().collect();
        let original_pending = source.pending.clone();
        let mut after = 0;
        let mut steps = 0;
        while after < pending.len() {
            let next = complete(codex_boundary_step(&mut source, &pending, after));
            assert_eq!(next - after, (pending.len() - after).min(CLOSE_STEP));
            after = next;
            steps += 1;
        }
        assert!(steps > 1);
        assert_eq!(source.pending, original_pending);
        assert!(
            source
                .events
                .iter()
                .all(|event| !event.code_mode_open && event.r.is_none())
        );
    }

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

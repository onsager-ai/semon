//! Origin-local, read-time two-pass relay pairing. Callers qualify the child,
//! parent-addressed sends and non-final receipts, and read bodies from selected
//! native references. No Builder or persisted pairing state is involved.
use super::{Kind, Ref, SourceFile, event};

/// Returns one receipt index per send and the receipt claim bitmap. `refs` is
/// the child's native event order; `positions` selects its qualified sends.
pub(super) fn pair_sends(
    files: &[SourceFile],
    refs: &[Ref],
    positions: &[usize],
    messages: &[Option<String>],
    received: &[Ref],
    bodies: &[Option<String>],
) -> (Vec<Option<usize>>, Vec<bool>) {
    let written = move |at: Ref| event(files, at).t;
    let mut paired = vec![false; received.len()];
    let mut claims: Vec<Option<usize>> = vec![None; positions.len()];
    let mut next = 0;
    for (send, position) in positions.iter().enumerate() {
        let at = refs[*position];
        let Some(message) = messages[send].as_deref() else {
            continue;
        };
        let sent = written(at);
        let found = (next..received.len()).find(|index| {
            !paired[*index]
                && sent
                    .zip(written(received[*index]))
                    .is_none_or(|(sent, written)| written >= sent)
                && bodies[*index].as_deref().map(str::trim) == Some(message.trim())
        });
        if let Some(index) = found {
            paired[index] = true;
            claims[send] = Some(index);
            next = index + 1;
        }
    }
    for (send, &position) in positions.iter().enumerate() {
        if claims[send].is_some() {
            continue;
        }
        let Some(sent) = written(refs[position]) else {
            continue;
        };
        // The child's last event before its next send or prompt, with
        // the send's own result.
        let boundary = (position + 1..refs.len())
            .find(|later| positions.contains(later) || event(files, refs[*later]).k == Kind::U)
            .unwrap_or(refs.len());
        let last = refs[position..boundary]
            .iter()
            .flat_map(|at| {
                let found = event(files, *at);
                [found.t, found.r.as_ref().and_then(|reply| reply.t)]
            })
            .flatten()
            .max()
            .unwrap_or(sent);
        let low = claims[..send]
            .iter()
            .rev()
            .find_map(|claim| *claim)
            .map_or(0, |index| index + 1);
        let high = claims[send + 1..]
            .iter()
            .find_map(|claim| *claim)
            .unwrap_or(received.len());
        let found = (low..high).find(|index| {
            !paired[*index]
                && written(received[*index]).is_some_and(|written| (sent..=last).contains(&written))
        });
        if let Some(index) = found {
            paired[index] = true;
            claims[send] = Some(index);
        }
    }
    (claims, paired)
}

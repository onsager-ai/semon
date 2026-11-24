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
    let windows: Vec<_> = positions
        .iter()
        .enumerate()
        .map(|(send, &position)| {
            let next = positions.get(send + 1).copied().unwrap_or(refs.len());
            let boundary = (position + 1..next)
                .find(|&later| event(files, refs[later]).k == Kind::U)
                .unwrap_or(next);
            let sent = event(files, refs[position]).t;
            let last = refs[position..boundary]
                .iter()
                .flat_map(|at| {
                    let found = event(files, *at);
                    [found.t, found.r.as_ref().and_then(|reply| reply.t)]
                })
                .flatten()
                .max();
            (sent, last)
        })
        .collect();
    let written: Vec<_> = received.iter().map(|at| event(files, *at).t).collect();
    pair_up(&windows, messages, &written, bodies)
}

/// Pair parent-addressed sends using their native time windows and bodies.
pub(crate) fn pair_up(
    windows: &[(Option<i64>, Option<i64>)],
    messages: &[Option<String>],
    written: &[Option<i64>],
    bodies: &[Option<String>],
) -> (Vec<Option<usize>>, Vec<bool>) {
    let mut paired = vec![false; written.len()];
    let mut claims = vec![None; windows.len()];
    let mut next = 0;
    for (send, &(sent, _)) in windows.iter().enumerate() {
        let Some(message) = messages[send].as_deref() else {
            continue;
        };
        if let Some(index) = (next..written.len()).find(|&index| {
            !paired[index]
                && sent.zip(written[index]).is_none_or(|(sent, at)| at >= sent)
                && bodies[index].as_deref().map(str::trim) == Some(message.trim())
        }) {
            paired[index] = true;
            claims[send] = Some(index);
            next = index + 1;
        }
    }
    for (send, &(sent, last)) in windows.iter().enumerate() {
        if claims[send].is_some() {
            continue;
        }
        let Some(sent) = sent else { continue };
        let low = claims[..send]
            .iter()
            .rev()
            .find_map(|claim| *claim)
            .map_or(0, |index| index + 1);
        let high = claims[send + 1..]
            .iter()
            .find_map(|claim| *claim)
            .unwrap_or(written.len());
        if let Some(index) = (low..high).find(|&index| {
            !paired[index]
                && written[index].is_some_and(|at| (sent..=last.unwrap_or(sent)).contains(&at))
        }) {
            paired[index] = true;
            claims[send] = Some(index);
        }
    }
    (claims, paired)
}

/// Pairs parent sends with down receipts: body first, then order between the
/// neighbouring claims. This differs from up receipts' bounded time fallback.
pub(super) fn pair_downs(
    files: &[SourceFile],
    sends: &[Ref],
    messages: &[Option<String>],
    downs: &[Ref],
    bodies: &[Option<String>],
) -> Vec<Option<usize>> {
    // Sent no later than it arrived, when both times are known.
    let before = |send: Ref, down: Ref| {
        event(files, send)
            .t
            .zip(event(files, down).t)
            .is_none_or(|(sent, arrived)| sent <= arrived)
    };
    let mut claims: Vec<Option<usize>> = vec![None; downs.len()];
    let mut used = vec![false; sends.len()];
    let mut next = 0;
    for (index, down) in downs.iter().enumerate() {
        let Some(body) = bodies[index].as_deref().map(str::trim) else {
            continue;
        };
        let found = (next..sends.len()).find(|send| {
            !used[*send]
                && before(sends[*send], *down)
                && messages[*send].as_deref().map(str::trim) == Some(body)
        });
        if let Some(send) = found {
            used[send] = true;
            claims[index] = Some(send);
            next = send + 1;
        }
    }
    for (index, down) in downs.iter().enumerate() {
        if claims[index].is_some() {
            continue;
        }
        let low = claims[..index]
            .iter()
            .rev()
            .find_map(|claim| *claim)
            .map_or(0, |send| send + 1);
        let high = claims[index + 1..]
            .iter()
            .find_map(|claim| *claim)
            .unwrap_or(sends.len());
        let found = (low..high).find(|send| !used[*send] && before(sends[*send], *down));
        if let Some(send) = found {
            used[send] = true;
            claims[index] = Some(send);
        }
    }
    claims
}

/// Qualifies inline messages and completed/resumed native runs over the selected
/// child events and its parent's final receipts and task notifications.
pub(super) fn claude_runs(
    files: &[SourceFile],
    child_events: &[Ref],
    first_prompt: Option<Ref>,
    finals: &[Ref],
    ends: &[(Ref, usize)],
    resume_calls: &[Ref],
) -> (Vec<super::Run>, super::BTreeSet<Ref>) {
    use super::{AGM_DOWN, BTreeSet};
    use ClaudeRunStep as Step;
    let mut steps: Vec<(Ref, Step)> = finals
        .iter()
        .map(|at| (*at, Step::Final))
        .chain(ends.iter().map(|(at, run)| {
            (
                *at,
                Step::Notification {
                    run: *run,
                    failed: !matches!(
                        event(files, *at).n.as_deref(),
                        Some("completed" | "success" | "done")
                    ),
                },
            )
        }))
        .chain(
            resume_calls
                .iter()
                .enumerate()
                .map(|(index, at)| (*at, Step::Resume { run: index + 1 })),
        )
        .collect();
    for at in child_events {
        let found = event(files, *at);
        let step = match found.k {
            Kind::Tool if found.n.as_deref() == Some("SubagentHandback") => Step::Handback,
            Kind::Agm if found.script & AGM_DOWN != 0 => Step::Start { coordinator: true },
            Kind::U if Some(*at) != first_prompt => Step::Start { coordinator: false },
            _ => continue,
        };
        steps.push((*at, step));
    }
    let steps = steps
        .into_iter()
        .map(|(at, step)| (event(files, at).t, step, at));
    let mut inline = BTreeSet::new();
    let runs = group_claude_runs(steps.collect(), |at| {
        inline.insert(at);
    });
    (runs, inline)
}

#[derive(Clone, Copy)]
pub(crate) enum ClaudeRunStep {
    Final,
    Handback,
    Notification { run: usize, failed: bool },
    Resume { run: usize },
    Start { coordinator: bool },
}

pub(crate) fn group_claude_runs<A: Copy + Ord>(
    mut steps: Vec<(Option<i64>, ClaudeRunStep, A)>,
    mut inline: impl FnMut(A),
) -> Vec<super::Run<A>> {
    use super::Run;
    use ClaudeRunStep as Step;
    steps.sort_by_key(|(time, step, at)| {
        let order = match step {
            Step::Final => 0,
            Step::Handback => 1,
            Step::Notification { run: 0, .. } => 2,
            Step::Resume { .. } => 3,
            Step::Notification { .. } => 5,
            Step::Start { .. } => 4,
        };
        (time.is_none(), *time, order, *at)
    });
    let mut runs = vec![Run::default()];
    let mut matched = std::collections::BTreeMap::from([(0, 0)]);
    let mut awaiting_delivery = false;
    for (_, step, at) in steps {
        let last = runs.len() - 1;
        match step {
            Step::Resume { run } => {
                runs.push(Run::default());
                matched.insert(run, last + 1);
                awaiting_delivery = true;
            }
            Step::Start { coordinator: true } if std::mem::take(&mut awaiting_delivery) => (),
            Step::Start { .. } if runs[last].ended() => runs.push(Run::default()),
            Step::Start { coordinator: true } => inline(at),
            Step::Start { coordinator: false } => (),
            // The parent records a hand-back at its next turn, so it can
            // land after the message that resumed the child: it belongs
            // to the run the child handed back, when that one has none.
            Step::Final => {
                let target = if last > 0
                    && !runs[last].ended()
                    && runs[last - 1].handback.is_some()
                    && runs[last - 1].fin.is_none()
                {
                    last - 1
                } else {
                    last
                };
                runs[target].fin = Some(at);
            }
            Step::Handback => runs[last].handback = Some(at),
            // A completion belongs to the invocation that identifies its run.
            Step::Notification { run, .. } => {
                if let Some(&target) = matched.get(&run) {
                    runs[target].tn.get_or_insert(at);
                }
            }
        }
    }
    runs
}

import type { CatalogTranscriptEntry } from '../state/catalog-transcript-wire';
import type { TranscriptBlock, EntryView, ToolView, BackgroundView } from '../lib/transcript';
import { toolInfo, runningToolLabel } from '../domain/toolNames';
import { I } from './registry';
import { STATE, HARNESSES } from './registry';
import { clock, compactCount, shortModel, dur } from '../domain/format';
import type { Entry } from '../domain/types';
import { coalesceRecordedThoughts, recordedThoughtLabel } from '../domain/thinking';
import type { RecordedRelationship } from '../domain/relationships';
import type {
  CatalogTranscriptPage,
  RecordedSessionPresentation,
} from '../state/catalog-transcript-wire';
export interface CatalogFieldView {
  text?: string;
  note: string;
  action?: { label: string; busy: boolean; run(): void };
}
export function recordedSignalLabel(entry: Extract<Entry, { k: 'signal' }>): string {
  const s = entry.signal,
    tag = s.tag ?? '';
  if (s.kind === 'compact')
    return (
      'Context compacted' +
      (s.value == null ? '' : ' · ' + compactCount(s.value) + ' tokens before')
    );
  if (s.kind === 'interrupt') return 'Interrupted by you';
  if (s.kind === 'denial') return (s.tool ?? 'Tool call') + ' denied';
  if (s.kind === 'hook')
    return (
      'Hook ' + (tag.includes('block') ? 'blocked' : tag || 'ran') + (s.tool ? ' ' + s.tool : '')
    );
  const name =
    (
      {
        model: 'Model',
        effort: 'Effort',
        approval: 'Approval policy',
        sandbox: 'Sandbox',
        permission: 'Permission mode',
      } as Record<string, string>
    )[s.kind] ?? 'Signal';
  return name + (s.previous ? ' ' + s.previous + ' → ' : ' → ') + tag;
}
function recordedBackgroundStatus(bg: import('../domain/types').Background) {
  if (bg.state === 'running') return 'running ' + (bg.secs ?? '');
  if (bg.state === 'unknown') return 'no end recorded';
  const outcome =
    bg.state === 'failed'
      ? 'failed'
      : bg.state === 'killed'
        ? 'stopped'
        : bg.exit != null
          ? 'exit ' + bg.exit
          : 'done';
  return outcome + (bg.secs ? ' · ' + bg.secs : '');
}
function gatherTools(entries: readonly EntryView[]): EntryView[] {
  const output: EntryView[] = [];
  let run: (ToolView | BackgroundView)[] = [];
  const flush = () => {
    if (!run.length) return;
    const ends = run.filter((row): row is BackgroundView => row.kind === 'background');
    const counts = new Map<string, { n: number; one: string; many: string }>();
    for (const row of run) {
      if (row.kind !== 'tool' || ends.some((end) => end.call === row.step.tid)) continue;
      const [, , verb, one, many] = toolInfo(row.step.data.name),
        count = counts.get(verb) ?? { n: 0, one, many };
      count.n++;
      counts.set(verb, count);
    }
    const text = [...counts]
      .map(([verb, count]) => verb + ' ' + count.n + ' ' + (count.n === 1 ? count.one : count.many))
      .join(', ');
    const tools = run.filter((row): row is ToolView => row.kind === 'tool');
    const finished = ends.length
      ? 'Finished ' + ends.length + ' background command' + (ends.length === 1 ? '' : 's')
      : '';
    const failures = ends.filter((row) => row.failed).length,
      stopped = ends.filter((row) => row.state === 'killed').length;
    const outcome = [failures ? failures + ' failed' : '', stopped ? stopped + ' stopped' : '']
      .filter(Boolean)
      .join(', ');
    output.push({
      kind: 'group',
      key: 'g:' + run[0].key,
      entries: run,
      lone: run.length === 1,
      summary: ends.length
        ? [
            { text: finished },
            ...(outcome ? [{ text: ' (' + outcome + ')', className: 'tt-counts' }] : []),
            ...(text ? [{ text: ', ' + text }] : []),
          ]
        : run.length > 1
          ? [{ text: text[0].toUpperCase() + text.slice(1) }]
          : [],
      background: ends.length > 0,
      failed: tools.filter(
        (row) => row.step.data.ok === false && !ends.some((end) => end.call === row.step.tid),
      ).length,
      running:
        tools.find((row) => row.step.running)?.step.backgroundStatus?.replace(/^running /, '') ??
        tools.find((row) => row.step.running)?.step.status ??
        undefined,
      stack: I.stack,
      chevron: I.chev,
    });
    run = [];
  };
  for (const entry of entries) {
    if (entry.kind === 'tool' || entry.kind === 'background') run.push(entry);
    else if (entry.kind === 'label' && ['signal-marker', 'vnote'].includes(entry.className))
      output.push(entry);
    else {
      flush();
      output.push(entry);
    }
  }
  flush();
  return output;
}
/** Native source records can be readable before relationship context is complete. */
export function catalogTranscriptBlocks(
  entries: readonly CatalogTranscriptEntry[],
  fieldView?: (entry: CatalogTranscriptEntry) => CatalogFieldView | null,
  relationshipView?: (entry: CatalogTranscriptEntry) => EntryView[] | null,
  liveObservation = false,
  context?: CatalogTranscriptPage['relationship_context'],
  attachments?: (item: CatalogTranscriptEntry) => import('../lib/attachments').Attachment[],
  harness = '',
  sessionKey = '',
  activity?: { tool: string; since: number },
): TranscriptBlock[] {
  entries = coalesceRecordedThoughts(
    entries,
    harness,
    new Set(context?.turns.map((turn) => turn.id) ?? []),
  );
  const closingBodies = new Map<string, string>(),
    consumed = new Set<string>(),
    returnedFailures = new Set<string>();
  if (relationshipView)
    for (const item of entries) {
      if (item.entry.k !== 'h') continue;
      const recorded = item.entry;
      const edge = context?.handoffs.find((row) => row.edge.id === recorded.id)?.edge;
      if (
        edge?.kind !== 'toyou' ||
        edge.ask !== 'result' ||
        edge.from !== sessionKey ||
        !context?.turns.some((turn) => turn.id === item.entry.turn && turn.sent.includes(edge.id))
      )
        continue;
      const body = [...entries]
        .reverse()
        .find(
          (row) =>
            row.slot < item.slot && row.entry.k === 'a' && row.entry.turn === item.entry.turn,
        );
      if (body?.entry.k !== 'a') continue;
      closingBodies.set(item.entry_id, body.entry.text);
      consumed.add(body.entry_id);
    }
  entries = entries.filter((item) => !consumed.has(item.entry_id));
  const blocks = entries.map((item) => {
    const { entry_id, native_action_text, clipped, freshness } = item,
      field = fieldView?.(item),
      entry =
        field?.text === undefined
          ? item.entry
          : item.entry.k === 'tool'
            ? { ...item.entry, out: field.text }
            : { ...item.entry, text: field.text };
    const views: EntryView[] = [];
    let notice = 0;
    const label = (text: string, className = 'vnote') =>
      views.push({
        kind: 'label',
        key: entry_id + ':notice:' + notice++,
        className,
        text,
      });
    if (entry.k === 'u' || entry.k === 'a')
      views.push({
        kind: 'message',
        key: entry_id,
        entryKey: entry_id,
        flavor: entry.k === 'u' ? 'user' : 'assistant',
        text: entry.text,
        images: attachments?.(item),
      });
    else if (entry.k === 'think')
      views.push({
        kind: 'thought',
        key: entry_id,
        entryKey: entry_id,
        mode: entry.text ? 'readable' : entry.pending ? 'pending' : 'masked',
        label: entry.text ? recordedThoughtLabel(entry.secs) : undefined,
        text: entry.text,
      });
    else if (entry.k === 'tool') {
      // Whole result expansion uses qualified native fields; no workspace endpoint handles survive.
      const { more, ...preview } = entry;
      const observedRunning =
          !!activity &&
          activity.tool === entry.name &&
          activity.since === item.observed_at &&
          entry.ok === null &&
          !entry.bg &&
          !entry.unfinished,
        running =
          liveObservation &&
          (entry.live === true || entry.bg?.state === 'running' || observedRunning),
        since = observedRunning ? activity!.since : (entry.bg?.since ?? entry.since),
        verb = running ? runningToolLabel(entry.name) : toolInfo(entry.name)[1],
        status = running
          ? since == null
            ? entry.secs
            : Math.max(0, Math.floor((Date.now() - since) / 1000)) + 's'
          : entry.unfinished
            ? 'Result not recorded'
            : [
                entry.exit != null
                  ? 'exit ' + entry.exit
                  : entry.ok === false
                    ? 'failed'
                    : entry.ok === null
                      ? 'exit unknown'
                      : '',
                entry.secs,
              ]
                .filter(Boolean)
                .join(' · ') || null;
      views.push({
        kind: 'tool',
        key: entry_id,
        entryKey: entry_id,
        step: {
          tid: entry.tid,
          sid: running ? sessionKey : undefined,
          since: running ? since : undefined,
          className:
            'step' +
            (entry.bg ? ' background' : '') +
            (running ? ' live' : entry.ok === false || entry.bg?.state === 'failed' ? ' err' : ''),
          key: entry_id,
          entryKey: entry_id,
          running,
          background: !!entry.bg,
          backgroundStatus: entry.bg ? recordedBackgroundStatus(entry.bg) : undefined,
          waiting: false,
          label: entry.title ?? entry.arg,
          named: !!entry.title,
          prefix: entry.title
            ? (running ? 'Running' : entry.ok === false ? 'Failed' : verb) + ': '
            : undefined,
          tip: entry.title
            ? (entry.in ?? entry.arg)
                .split(/\r\n|\n|\r/)
                .find((line) => line.trim())
                ?.slice(0, 200)
            : undefined,
          verb,
          status: entry.bg ? null : (status ?? null),
          icon: I[toolInfo(entry.name)[0]],
          chevron: I.chev,
          data: {
            ...preview,
            expandable:
              !!field?.action || !!(entry.cut && (entry.out?.split('\n').length ?? 0) > 24),
          },
        },
      });
      if (more?.length)
        label('Full result unavailable for this source. The recorded preview is shown.');
    } else if (entry.k === 'h') {
      const related = relationshipView?.(item);
      if (related) {
        views.push(
          ...related.map((view) =>
            view.kind === 'message'
              ? { ...view, images: attachments?.(item) }
              : view.kind === 'event' && closingBodies.has(item.entry_id)
                ? { ...view, brief: closingBodies.get(item.entry_id)! }
                : view,
          ),
        );
      } else {
        label('Native action · related session context is incomplete');
        if (native_action_text === undefined) label('Original action prompt is unavailable.');
        else
          views.push({
            kind: 'message',
            key: entry_id,
            entryKey: entry_id,
            flavor: 'incoming',
            text: native_action_text ?? '',
          });
      }
    } else if (entry.k === 'end') {
      const parent = entry.ret && context?.sessions[entry.ret.to],
        returned =
          entry.ret &&
          context?.handoffs.some(
            ({ edge }) =>
              edge.kind === 'spawn' &&
              edge.to === sessionKey &&
              edge.from === entry.ret!.to &&
              edge.done === entry.ret!.at,
          );
      if (parent && returned && entry.ret!.failed && entry.turn) returnedFailures.add(entry.turn);
      label(
        parent && returned
          ? 'Returned to ' +
              parent.name +
              (entry.ret!.failed ? ' · failed' : '') +
              ' · ' +
              clock(entry.ret!.at!, Date.now())
          : item.provenance
            ? (entry.text ?? 'Native session end recorded')
            : 'Session boundary · native provenance unavailable',
        'divider',
      );
    } else if (entry.k === 'bgend')
      views.push({
        kind: 'background',
        state: entry.state,
        key: entry_id,
        entryKey: entry_id,
        failed: entry.state === 'failed',
        call: entry.call,
        label:
          'Background command ' +
          (entry.state === 'failed'
            ? 'failed'
            : entry.state === 'killed'
              ? 'stopped'
              : 'completed') +
          ' · ' +
          (entry.label ?? ''),
        loaded: entries.some((item) => item.entry.k === 'tool' && item.entry.tid === entry.call),
      });
    else if (entry.k === 'harness')
      label('Harness text added before the prompt (' + entry.label + ')', 'harness-note');
    else if (entry.k === 'signal')
      views.push({
        kind: 'label',
        key: entry_id,
        entryKey: entry_id,
        className: 'signal-marker',
        text: recordedSignalLabel(entry),
      });
    if (field) {
      views.push({
        kind: 'label',
        key: entry_id + ':field',
        className: 'vnote',
        text: field.note,
        action: field.action,
      });
    } else if (clipped) label('Recorded text preview. The complete source text is not loaded.');
    if (freshness && freshness.state !== 'cached')
      label(
        'Source observation is ' + freshness.state + '. Previously loaded content is retained.',
      );
    if (entry.img?.length) label('Attachments unavailable for this source.');
    return { kind: 'loose' as const, key: entry_id, entries: views };
  });
  const grouped: TranscriptBlock[] = [];
  for (let index = 0; index < entries.length; index++) {
    const item = entries[index],
      block = blocks[index],
      turn = item.entry.turn;
    if (!turn) {
      const previous = grouped.at(-1);
      if (previous?.kind === 'loose') (previous.entries as EntryView[]).push(...block.entries);
      else
        grouped.push({
          ...block,
          key: 'loose:' + (previous?.kind === 'turn' ? previous.turn.id : 'start'),
        });
      continue;
    }
    const previous = grouped.at(-1);
    if (previous?.kind === 'turn' && previous.turn.id === turn) {
      // This is a fresh presentation array; append without copying a growing turn each time.
      (previous.turn.entries as EntryView[]).push(...block.entries);
    } else
      grouped.push({
        kind: 'turn',
        turn: {
          id: turn,
          label: 'Recorded turn',
          entries: block.entries,
          traceIcon: I.trace,
          stateLabel: 'Recorded',
        },
      });
  }
  for (const block of grouped) {
    if (block.kind !== 'turn') continue;
    const descriptor = context?.turns.find((turn) => turn.id === block.turn.id);
    if (!descriptor) continue;
    const start = context?.handoffs.find((row) => row.edge.id === descriptor.start)?.edge;
    block.turn.label = descriptor.u || start?.kind === 'ask' ? 'Your message' : 'Recorded turn';
    block.turn.trace = descriptor.sent.length ? descriptor.id : undefined;
    if (
      descriptor.end.st === 'err' &&
      !(descriptor.end.why === 'failed' && returnedFailures.has(block.turn.id))
    )
      block.turn.error = descriptor.end.why === 'failed' ? 'Failed' : descriptor.end.why;
    if (start && start.kind !== 'ask') {
      const sender = context?.sessions[start.from],
        harness = sender && HARNESSES[sender.harness];
      block.turn.header = {
        mark: harness
          ? {
              id: sender!.harness,
              name: harness.name,
              light: harness.icon.light,
              dark: harness.icon.dark,
              darkTheme: matchMedia('(prefers-color-scheme: dark)').matches,
            }
          : undefined,
        parts: [
          {
            className: 'soft',
            text: start.kind === 'spawn' ? 'Brief from ' : 'Message from ',
          },
          {
            className: 'who who-link',
            text: sender?.name ?? 'Recorded session',
            action: sender ? { kind: 'session', id: start.from } : undefined,
          },
        ],
        time: clock(start.at, Date.now()),
      };
    }
  }
  return grouped.map((block) =>
    block.kind === 'turn'
      ? { ...block, turn: { ...block.turn, entries: gatherTools(block.turn.entries) } }
      : { ...block, entries: gatherTools(block.entries) },
  );
}

export function recordedRelationshipViews(
  item: CatalogTranscriptEntry,
  relationship: RecordedRelationship,
  selected: string,
  participants: Record<string, RecordedSessionPresentation>,
  native: { brief?: string; result?: string; answer?: string | string[] },
): EntryView[] {
  const edge = relationship.edge,
    target = participants[edge.to],
    source = participants[edge.from];
  const name = (id: string) =>
    id === 'you' ? 'You' : (participants[id]?.name ?? 'Recorded session');
  if (edge.kind === 'spawn' && edge.from === selected && target) {
    const mark = HARNESSES[target.harness],
      status =
        target.state === 'work' || target.state === 'wait'
          ? target.live_observation === false
            ? 'off'
            : target.state
          : (target.state ?? edge.status),
      kind =
        target.kind === 'subagent'
          ? 'Subagent'
          : target.kind === 'codex-run'
            ? 'Codex run'
            : target.kind,
      meta = [
        kind,
        shortModel(target.model),
        target.start == null
          ? undefined
          : dur(
              target.start,
              target.live_observation && status === 'work' ? undefined : target.last,
              Date.now(),
            ),
        target.calls == null ? undefined : target.calls + (target.calls === 1 ? ' step' : ' steps'),
        target.usd == null ? undefined : '$' + target.usd.toFixed(2),
      ]
        .filter(Boolean)
        .join(' · ');
    return [
      {
        kind: 'child',
        key: item.entry_id,
        entryKey: item.entry_id,
        handoff: edge.id,
        id: edge.to,
        turn: relationship.targetTurn,
        name: target.name,
        state: status,
        stateLabel: STATE[status] ?? 'Recorded history',
        meta,
        brief: native.brief ?? '',
        result: native.result,
        failed: status === 'err',
        mark: mark
          ? {
              id: target.harness,
              name: mark.name,
              light: mark.icon.light,
              dark: mark.icon.dark,
              darkTheme: matchMedia('(prefers-color-scheme: dark)').matches,
            }
          : undefined,
        chevron: I.chev,
      },
    ];
  }
  if (edge.to === selected && edge.from !== selected)
    return [
      {
        kind: 'message',
        key: item.entry_id,
        entryKey: item.entry_id,
        handoff: edge.id,
        flavor: edge.kind === 'ask' ? 'user' : 'incoming',
        text: native.brief ?? item.native_action_text ?? '',
      },
      ...(edge.kind === 'ask' && relationship.targetTurn
        ? [
            {
              kind: 'label' as const,
              key: item.entry_id + ':time',
              className: 'msg-tm',
              text: clock(edge.at, Date.now()),
            },
          ]
        : []),
    ];
  const parts = [
    {
      className: 'who who-link',
      text: name(edge.from),
      action: source ? { kind: 'session' as const, id: edge.from } : undefined,
    },
    {
      className: 'soft',
      text:
        edge.kind === 'toyou'
          ? edge.ask === 'result'
            ? ' sent '
            : ' asked '
          : edge.kind === 'relay'
            ? ' relayed to '
            : ' asked ',
    },
    {
      className: 'who who-link',
      text: name(edge.to),
      action: target
        ? { kind: 'session' as const, id: edge.to, turn: relationship.targetTurn }
        : undefined,
    },
  ];
  if (edge.kind === 'toyou' && edge.ask === 'result')
    parts.splice(1, 2, { className: 'soft', text: ' sent you a result' });
  return [
    {
      kind: 'event',
      key: item.entry_id,
      entryKey: item.entry_id,
      handoff: edge.id,
      className:
        'event' +
        (edge.kind === 'move' ? ' move' : '') +
        (edge.kind === 'toyou' && edge.status === 'wait' ? ' waiting' : ''),
      icon:
        edge.kind === 'toyou' && edge.ask !== 'result' && edge.status !== 'wait'
          ? I.done
          : I[
              edge.kind === 'toyou'
                ? edge.ask === 'result'
                  ? 'result'
                  : edge.ask === 'decision'
                    ? 'decide'
                    : 'qc'
                : edge.kind === 'relay'
                  ? 'relay'
                  : 'ask'
            ],
      parts,
      time: clock(edge.at, Date.now()),
      brief: native.brief ?? item.native_action_text ?? '',
      result: native.result,
      resultLabel: 'Result',
      answers:
        native.answer == null
          ? undefined
          : Array.isArray(native.answer)
            ? native.answer
            : [native.answer],
      waiting: edge.status === 'wait',
      stateLabel: STATE[edge.status],
    },
  ];
}

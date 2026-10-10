import { toolInfo } from '../domain/toolNames';
import { STATE } from './registry';
import { I } from './registry';
import type { ViewerModelStore } from '../state/model';
import type { TranscriptStore } from '../state/transcript';
import type { createDomain } from '../domain/calculations';
import type { createSentences } from './sentences';
import type { createScreenViews } from './screenViews';
import type { createTicker } from './ticker';
import type { createToolViews } from './toolViews';
import type { createPaging } from './paging';
import { compactCount, preview } from '../domain/format';
import type { Background, Entry, Turn } from '../domain/types';
import type {
  BackgroundView,
  EntryView,
  SessionSnapshot,
  ToolStepSnapshot,
  ToolView,
  TranscriptBlock,
  TurnView,
} from '../lib';
type ToolEntry = Extract<Entry, { k: 'tool' }> & { full?: boolean; scriptLoaded?: boolean };
type ThoughtEntry = Extract<Entry, { k: 'think' }>;
interface GroupEntry {
  view: ToolView | BackgroundView;
  end?: boolean;
  state?: string;
  key?: string;
  k?: string;
  v?: string;
  err?: boolean;
  bg?: boolean;
  tid?: string;
  live?: boolean;
  secs?: string;
}
interface TranscriptViewHost {
  find: string;
  show: { messages: boolean; tools: boolean; thinking: boolean };
  clock: (t: number) => string;
  now: number;
  isGap: (e: Entry) => boolean;

  pagingOwner: Pick<ReturnType<typeof createPaging>, 'pagerSnapshot'>;

  toolViewsOwner: Pick<
    ReturnType<typeof createToolViews>,
    'attachmentSnapshot' | 'childSnapshot' | 'footerSnapshot'
  >;

  tickerOwner: Pick<ReturnType<typeof createTicker>, 'running'>;

  screenViews: Pick<ReturnType<typeof createScreenViews>, 'harnessSnapshot' | 'showsFooter'>;

  sentencesOwner: Pick<ReturnType<typeof createSentences>, 'sentenceSnapshot'>;

  domain: Pick<ReturnType<typeof createDomain>, 'turnEnd' | 'answersOf' | 'originHandoff'>;

  transcripts: Pick<TranscriptStore, 'entries' | 'meta' | 'view' | 'staleBriefs'>;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'turns' | 'handoff' | 'machines'>;
}
/** Owns transcriptView behavior through explicit application ports. */
export function createTranscriptView(host: TranscriptViewHost) {
  const thoughtText = (e: ThoughtEntry) => String(e.text ?? '').trim();
  const isPendingThought = (
    e: ThoughtEntry,
    entries: Entry[] = [],
    i: number = 0,
    sid: string = '',
  ) =>
    !!(e.pending || e.status === 'thinking') ||
    (Array.isArray(entries) &&
      e.k === 'think' &&
      !thoughtText(e) &&
      i === entries.length - 1 &&
      host.modelStore.sessions[sid]?.state === 'work');
  const signalLabel = (e: Extract<Entry, { k: 'signal' }>) => {
    const s = e.signal ?? {},
      tag = s.tag ?? '',
      kind = s.kind;
    if (kind === 'compact')
      return (
        'Context compacted' +
        (s.value != null ? ' · ' + compactCount(s.value) + ' tokens before' : '')
      );
    if (kind === 'interrupt') return 'Interrupted by you';
    if (kind === 'denial') return (s.tool ?? 'Tool call') + ' denied';
    if (kind === 'hook')
      return (
        'Hook ' + (tag.includes('block') ? 'blocked' : tag || 'ran') + (s.tool ? ' ' + s.tool : '')
      );
    const name =
      {
        model: 'Model',
        effort: 'Effort',
        approval: 'Approval policy',
        sandbox: 'Sandbox',
        permission: 'Permission mode',
      }[kind] ?? 'Signal';
    return name + (s.previous ? ' ' + s.previous + ' → ' : ' → ') + tag;
  };
  const isMaskedThought = (e: Entry, entries: Entry[] = [], i: number = 0, sid: string = '') =>
    e.k === 'think' && !isPendingThought(e, entries, i, sid) && !thoughtText(e);
  function thoughtSeconds(e: ThoughtEntry) {
    if (typeof e.secs === 'number' && Number.isFinite(e.secs) && e.secs >= 0) return e.secs;
    if (typeof e?.secs === 'string') {
      const m = /^(\d+(?:\.\d+)?)s?$/.exec(e.secs.trim());
      if (m) return Number(m[1]);
    }
    return null;
  }
  // The label above a thought. The log holds no measured thinking time: the duration is the gap between the log timestamps of
  // this record and the one before it, which rounds to 0 s whenever the two were written together. So a duration is named only
  // once it reaches a second ("Thinking · 12s"); shorter, the label says just "Thinking", never "Thought for 0s".
  const thoughtLabel = (secs: number | null | undefined) => {
    const n = Number.isFinite(secs) ? Math.round(secs ?? 0) : 0;
    return n < 1
      ? 'Thinking'
      : 'Thinking · ' + (n >= 60 ? Math.floor(n / 60) + 'm ' + (n % 60) + 's' : n + 's');
  };
  // A masked thought (Claude redacts its thinking, Codex encrypts its reasoning) is kept in the list, so entry keys and turn
  // starts still line up with the index, and the page draws one quiet line for it.
  // Join adjacent Codex snapshots with the same resolved owner; unknown explicit turns block merging until ownership resumes.
  function transcriptEntries(entries: Entry[], sid: string) {
    const out: Entry[] = [],
      codex = host.modelStore.sessions[sid]?.harness === 'codex';
    const turns = codex ? (host.modelStore.turns[sid] ?? []) : [],
      owners = codex
        ? new Map(turns.flatMap((t) => t.entries.map((e) => [e.key, t.id])))
        : new Map<string | undefined, string>();
    const turnIds = codex ? new Set(turns.map((t) => t.id)) : new Set<string | undefined>();
    let chain: {
        turn: string | null | undefined;
        text: string;
        row: ThoughtEntry;
        index: number;
      } | null = null,
      sawOwnedEntry = false,
      unresolvedOwner = false;
    for (let i = 0; i < entries.length; i++) {
      const e = entries[i],
        indexedOwner = codex ? owners.get(e.key) : null;
      const turn = codex ? (indexedOwner ?? (turnIds.has(e.turn) ? e.turn : null)) : null;
      if (codex && indexedOwner == null && e.turn != null && !turnIds.has(e.turn)) {
        unresolvedOwner = true;
        chain = null;
      } else if (turn !== null) {
        sawOwnedEntry = true;
        unresolvedOwner = false;
      }
      if (e.k !== 'think') {
        out.push(e);
        chain = null;
        continue;
      }
      const pending = isPendingThought(e, entries, i, sid);
      const row = { ...e, ...(pending ? { pending: true } : {}), displaySecs: thoughtSeconds(e) };
      const text = thoughtText(row);
      if (!codex || pending || !text || unresolvedOwner || (turn === null && sawOwnedEntry)) {
        out.push(row);
        chain = null;
        continue;
      }
      if (
        chain &&
        chain.turn === turn &&
        (text === chain.text ||
          text.startsWith(chain.text + '\n') ||
          text.startsWith(chain.text + '\r\n'))
      ) {
        const merged: ThoughtEntry = { ...chain.row, text, displaySecs: row.displaySecs };
        out[chain.index] = merged;
        chain = { index: chain.index, turn: chain.turn, row: merged, text };
      } else {
        out.push(row);
        chain = { index: out.length - 1, row, text, turn };
      }
    }
    return out;
  }

  // What a tool call did: an icon and verb for its step row, and a phrase and nouns for a group summary
  // ("ran 2 commands, asked you 1 question"). An unknown tool keeps its own name ("TodoWrite 1 step").
  const verb = (name: string) => toolInfo(name).slice(0, 2);
  const NOW_VERB: Record<string, string> = {
    Ran: 'Running',
    Read: 'Reading',
    Edited: 'Editing',
    Patched: 'Patching',
    Wrote: 'Writing',
    'Searched for': 'Searching for',
  };
  const verbNow = (name: string) => NOW_VERB[verb(name)[1]] ?? verb(name)[1];
  const isCmd = (name: string) => /^(Bash|shell|exec_command|local_shell)$/.test(name);
  // A background step's outcome ("running 8m 18s", "exit 0 · 1m 0s", "failed · 2m 0s"). Its `.sd` puts "background · " before it,
  // in a span a phone hides in favour of a marker before the verb, so the outcome is never what gets cut.
  const backgroundText = (bg: Background) => {
    if (bg.state === 'running') return 'running ' + bg.secs;
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
  };
  const elapsedMs = (text: string | undefined) => {
    const m = /^(?:(\d+)m )?(\d+(?:\.\d+)?)s$/.exec(text ?? '');
    return m ? (Number(m[1] ?? 0) * 60 + Number(m[2])) * 1000 : null;
  };
  function transcriptSnapshot(
    sid: string,
    opts: { only?: ReadonlySet<string> } = {},
  ): SessionSnapshot {
    const projection = host.transcripts.view(sid);
    const entries = transcriptEntries(projection.entries ?? [], sid),
      blocks: TranscriptBlock[] = [];
    let tx: EntryView[] = [];
    const currentTurn: { value: { t: Turn; view: TurnView } | null } = { value: null };
    let loose = 0;
    const hit = (text: unknown) =>
      !host.find ||
      String(text ?? '')
        .toLowerCase()
        .includes(host.find);
    const filtering = !!host.find || !host.show.messages || !host.show.tools || !host.show.thinking;
    const entryKey = (e: Entry) => (e.slot != null ? sid + '#slot:' + e.slot : e.key);
    const keyed = <T extends EntryView>(view: T, e: Entry): T => ({
      ...view,
      key: e.key,
      entryKey: e.key ? entryKey(e) : undefined,
    });
    const endedCalls = new Set(entries.flatMap((e) => (e.k === 'bgend' ? [e.call] : [])));
    let run: GroupEntry[] = [],
      masked = false;
    const flush = () => {
      if (!run.length) return;
      const summary = [],
        ends = run.filter((r) => r.end),
        counts = new Map();
      if (run.length > 1 && !filtering) {
        for (const r of run.filter((r) => !r.end)) {
          const [, , p, one, many] = toolInfo(r.k ?? ''),
            c = counts.get(p) ?? { n: 0, one, many };
          c.n++;
          counts.set(p, c);
        }
        if (ends.length) {
          const outcomes = [
            [ends.filter((r) => r.state === 'failed').length, 'failed'],
            [ends.filter((r) => r.state === 'killed').length, 'stopped'],
          ]
            .filter(([n]) => n)
            .map(([n, word]) => n + ' ' + word);
          summary.push(
            { text: 'Finished ', className: 'long' },
            { text: ends.length + ' background' },
            { text: ' command' + (ends.length === 1 ? '' : 's'), className: 'long' },
          );
          if (outcomes.length)
            summary.push({ text: ' (' + outcomes.join(', ') + ')', className: 'tt-counts' });
          for (const [p, c] of counts)
            summary.push(
              { text: ', ' },
              { text: p + ' ', className: p === 'ran' ? 'long' : undefined },
              { text: c.n + ' ' + (c.n === 1 ? c.one : c.many) },
            );
        } else {
          const text = [...counts]
            .map(([p, c]) => p + ' ' + c.n + ' ' + (c.n === 1 ? c.one : c.many))
            .join(', ');
          summary.push({ text: text[0].toUpperCase() + text.slice(1) });
        }
      }
      const failed = run.filter(
        (r) => !r.end && r.err && (!r.bg || !endedCalls.has(r.tid ?? '')),
      ).length;
      const live = run.some((r) => r.bg)
        ? run
            .filter((r) => r.live)
            .sort((a, b) => (elapsedMs(b.secs) ?? 0) - (elapsedMs(a.secs) ?? 0))[0]
        : run.find((r) => r.live);
      tx.push({
        kind: 'group',
        key: run[0].key ? 'g:' + run[0].key : undefined,
        entryKey: run[0].view.entryKey ? 'g:' + run[0].view.entryKey : undefined,
        entries: run.map((r) => r.view),
        lone: run.length === 1 && !filtering,
        summary,
        background: !!ends.length,
        failed,
        running: live?.secs,
        stack: I.stack,
        chevron: I.chev,
      });
      run = [];
    };
    const firsts = new Map(
      (host.modelStore.turns[sid] ?? [])
        .filter((t) => t.entries[0]?.key)
        .map((t) => [t.entries[0].key, t]),
    );
    const owner = opts.only
      ? new Map(
          (host.modelStore.turns[sid] ?? []).flatMap((t) => t.entries.map((e) => [e.key, t.id])),
        )
      : null;
    const content = (values: readonly EntryView[]) =>
      values.some((v) => v.kind !== 'label' || v.className === 'harness-note');
    const closeTurn = () => {
      flush();
      if (!currentTurn.value) {
        if (tx.length) blocks.push({ kind: 'loose', key: 'loose:' + loose++, entries: tx });
        tx = [];
        return;
      }
      const { t, view } = currentTurn.value,
        end = host.domain.turnEnd(t),
        returned = t.entries.some((e) => e.k === 'end' && /^Returned to /.test(e.text ?? ''));
      if (!filtering) {
        if (end?.st === 'err' && !returned) view.error = end.text;
        if (t.out.length) view.trace = t.id;
      }
      view.entries = tx;
      if (!filtering || content(tx)) blocks.push({ kind: 'turn', turn: view });
      currentTurn.value = null;
      tx = [];
      masked = false;
    };
    const openTurn = (t: Turn) => {
      closeTurn();
      const h = t.start,
        view: TurnView = {
          id: t.id,
          entries: [],
          traceIcon: I.trace,
          stateLabel: STATE.err,
        };
      if (t.u || h?.kind === 'ask')
        view.label = 'Your message' + (h ? ' at ' + host.clock(h.at) : '');
      else if (h)
        view.header = {
          parts: host.sentencesOwner.sentenceSnapshot(h, sid, true).parts,
          mark: host.modelStore.sessions[h.from]
            ? host.screenViews.harnessSnapshot(host.modelStore.sessions[h.from].harness)
            : undefined,
          time: host.clock(h.at),
        };
      currentTurn.value = { t, view };
    };
    const toolStep = (e: ToolEntry, v: string, ic: string, live: boolean): ToolStepSnapshot => {
      const bg = e.bg,
        bgRunning = bg?.state === 'running',
        waiting = live && host.modelStore.sessions[sid]?.state === 'wait';
      const waitingText = host.modelStore.sessions[sid]?.waiting_for?.includes('permission')
        ? 'Waiting on permission'
        : 'Waiting for your input';
      const status = bg
        ? null
        : waiting
          ? waitingText
          : live
            ? e.secs
            : e.unfinished
              ? 'no result'
              : e.exit != null
                ? 'exit ' + e.exit + ' · ' + e.secs
                : e.ok
                  ? e.secs
                  : e.ok === null
                    ? 'exit unknown · ' + e.secs
                    : 'failed · ' + e.secs;
      const title = e.title ? String(e.title) : null,
        command = e.in ?? e.arg;
      const firstLine =
        typeof command === 'string'
          ? command.split(/\r\n|\n|\r/).find((line) => line.trim())
          : null;
      const bgSecs =
        bgRunning && bg.since != null ? host.tickerOwner.running(host.now - bg.since) : bg?.secs;
      return {
        className:
          'step' +
          (bg
            ? (bg.state === 'failed' ? ' err' : '') +
              ' background' +
              (bgRunning ? ' background-running' : '')
            : live
              ? ' live'
              : e.ok || e.ok === null
                ? ''
                : ' err'),
        key: e.key,
        entryKey: e.key ? entryKey(e) : undefined,
        tid: e.tid,
        sid: live || bgRunning ? sid : undefined,
        since: live ? e.since : bgRunning ? bg.since : undefined,
        running: live || bgRunning,
        background: !!bg,
        waiting: host.modelStore.sessions[sid]?.state === 'wait',
        named: !!title,
        prefix: title
          ? waiting
            ? waitingText + ': '
            : live
              ? 'Running: '
              : e.ok === false
                ? 'Failed: '
                : v + ': '
          : undefined,
        verb: waiting ? 'Waiting' : live ? verbNow(e.name) : v,
        label: title ?? e.arg,
        tip: title && firstLine != null ? firstLine.slice(0, 200) : undefined,
        status: status == null ? null : String(status),
        backgroundStatus: bg ? backgroundText({ ...bg, secs: bgSecs }) : undefined,
        icon: I[ic],
        chevron: I.chev,
        data: e,
      };
    };
    for (const e of entries) {
      if (owner && opts.only && !opts.only.has(owner.get(e.key) ?? '')) continue;
      if (host.isGap(e)) {
        closeTurn();
        if (!filtering)
          tx.push({
            kind: 'label',
            className: 'divider',
            text: e.k === 'end' ? (e.text ?? '') : '',
          });
        closeTurn();
        continue;
      }
      const first = firsts.get(e.key);
      if (first) openTurn(first);
      if (e.k === 'think' && (!host.show.thinking || host.find)) continue;
      if (e.k === 'signal') {
        const label = signalLabel(e);
        if (hit(label))
          tx.push(keyed({ kind: 'label', className: 'signal-marker', text: label }, e));
        continue;
      }
      if (e.k === 'think' && isMaskedThought(e)) {
        if (!masked) {
          masked = true;
          tx.push(
            keyed({ kind: 'thought', mode: 'masked', label: 'Thinking hidden by the harness' }, e),
          );
        }
        continue;
      }
      if (e.k === 'bgend') {
        if (!host.show.tools || !hit(e.label ?? '')) continue;
        const word =
          e.state === 'failed' ? 'failed' : e.state === 'killed' ? 'stopped' : 'completed';
        const view = keyed(
          {
            kind: 'background',
            failed: e.state === 'failed',
            label: 'Background command ' + word + ' · ' + (e.label ?? ''),
            call: e.call,
            loaded: entries.some((entry) => entry.k === 'tool' && entry.tid === e.call),
          },
          e,
        );
        run.push({ view, end: true, state: e.state, key: e.key });
        continue;
      }
      if (e.k === 'tool') {
        if (
          !host.show.tools ||
          !hit(
            e.name + ' ' + (e.title ?? '') + ' ' + e.arg + ' ' + (e.in ?? '') + ' ' + (e.out ?? ''),
          )
        )
          continue;
        const [ic, v] = verb(e.name),
          view = keyed({ kind: 'tool', step: toolStep(e, v, ic, !!e.live && !e.bg) }, e);
        if (e.live && !e.bg) run.push({ view, v, k: e.name, live: true, secs: e.secs, key: e.key });
        else if (e.bg) {
          const live = e.bg.state === 'running';
          run.push({
            view,
            v,
            k: e.name,
            err: e.bg.state === 'failed',
            bg: true,
            tid: e.tid,
            live,
            secs:
              live && e.bg.since != null
                ? host.tickerOwner.running(host.now - e.bg.since)
                : (e.bg.secs ?? e.secs),
            key: e.key,
          });
        } else run.push({ view, v, k: e.name, err: e.ok === false, key: e.key });
        continue;
      }
      flush();
      if (e.k === 'u' || e.k === 'a') {
        if (!host.show.messages || !hit(e.text)) continue;
        tx.push(
          keyed(
            {
              kind: 'message',
              flavor: e.k === 'u' ? 'user' : 'assistant',
              text: e.text,
              images: e.k === 'u' ? host.toolViewsOwner.attachmentSnapshot(e) : undefined,
            },
            e,
          ),
        );
      } else if (e.k === 'think')
        tx.push(
          keyed(
            isPendingThought(e)
              ? { kind: 'thought', mode: 'pending' }
              : {
                  kind: 'thought',
                  mode: 'readable',
                  label: thoughtLabel(e.displaySecs),
                  text: thoughtText(e),
                },
            e,
          ),
        );
      else if (e.k === 'harness') {
        if (!host.show.messages || host.find) continue;
        tx.push(
          keyed(
            {
              kind: 'label',
              className: 'harness-note',
              text: 'Harness text added before the prompt (' + e.label + ')',
            },
            e,
          ),
        );
      } else if (e.k === 'end') {
        if (!filtering)
          tx.push(keyed({ kind: 'label', className: 'divider', text: e.text ?? '' }, e));
      } else if (e.k === 'h') {
        const h = host.modelStore.handoff.get(e.id);
        if (!h || !hit(h.brief + ' ' + (h.result ?? ''))) continue;
        if (h.kind === 'ask') {
          if (!host.show.messages) continue;
          tx.push(
            keyed(
              {
                kind: 'message',
                flavor: 'user',
                text: h.brief ?? '',
                images: host.toolViewsOwner.attachmentSnapshot(e),
              },
              e,
            ),
          );
          if (currentTurn.value?.t.start === h)
            tx.push({ kind: 'label', className: 'msg-tm', text: host.clock(h.at) });
          continue;
        }
        if (currentTurn.value && currentTurn.value.t.start === h) {
          if (host.show.messages)
            tx.push(
              keyed({ kind: 'message', flavor: 'incoming', text: h.brief ?? '', handoff: h.id }, e),
            );
          continue;
        }
        if (h.kind === 'spawn' && h.from === sid && host.modelStore.sessions[h.to]) {
          if (host.show.tools)
            tx.push(keyed(host.toolViewsOwner.childSnapshot(h, host.modelStore.sessions[h.to]), e));
          continue;
        }
        if ((host.find && h.kind === 'move') || (!host.show.messages && h.kind !== 'move'))
          continue;
        const sentence = host.sentencesOwner.sentenceSnapshot(h, sid, true);
        tx.push(
          keyed(
            {
              kind: 'event',
              handoff: h.id,
              className:
                'event' +
                (h.kind === 'toyou' && h.status === 'wait' ? ' waiting' : '') +
                (h.kind === 'move' ? ' move' : ''),
              icon: sentence.icon,
              parts: sentence.parts,
              time: host.clock(h.at),
              brief: preview(h.brief ?? ''),
              result: h.result || undefined,
              resultLabel: h.kind === 'relay' ? 'Reply: ' : 'Returned: ',
              answers: host.domain.answersOf(h) ?? undefined,
              waiting: h.kind === 'toyou' && h.status === 'wait',
              stateLabel: STATE.wait,
            },
            e,
          ),
        );
      }
    }
    closeTurn();
    const range = projection.range,
      s = host.modelStore.sessions[sid],
      origin = host.domain.originHandoff(sid);
    return {
      id: sid,
      name: s.name,
      observation: host.transcripts.staleBriefs.has(sid)
        ? 'This conversation changed at its source. The saved transcript is incomplete; reopen this session to update it.'
        : undefined,
      blocks,
      order: (host.modelStore.turns[sid] ?? []).map((t) => t.id),
      dirty: opts.only,
      before: range?.from > 0 ? host.pagingOwner.pagerSnapshot(sid, 'before') : undefined,
      after:
        range && range.to < range.total ? host.pagingOwner.pagerSnapshot(sid, 'after') : undefined,
      started:
        !range?.from && !filtering
          ? {
              lead: 'Started ' + host.clock(s.start) + ' on\u00a0',
              machine: host.modelStore.machines[s.movedFrom ?? s.machine],
            }
          : undefined,
      empty:
        !opts.only && !blocks.some((b) => content(b.kind === 'turn' ? b.turn.entries : b.entries))
          ? host.find
            ? 'Nothing matches “' + host.find + '”.'
            : 'Nothing to show with these filters.'
          : undefined,
      footer: host.screenViews.showsFooter(s, origin)
        ? host.toolViewsOwner.footerSnapshot(s, origin)
        : undefined,
    };
  }

  return { transcriptEntries, transcriptSnapshot, verb, verbNow };
}

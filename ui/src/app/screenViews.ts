import type { createLocalControl } from './localControl';
import { I } from './registry';
import { HARNESSES } from './registry';
import { STATE } from './registry';
import { HARNESS } from './registry';
import type { ViewerModelStore } from '../state/model';
import type { TranscriptStore } from '../state/transcript';
import type { EffectScope } from '../app/effects';
import type { NavigationController } from '../navigation/routes';
import type { createDomain } from '../domain/calculations';
import type { createSessionChrome } from './sessionChrome';
import type { createDestination } from './destination';
import type { createSeenPersistence } from './seenPersistence';
import type { createDocumentRenderer } from './documentRenderer';
import type { createOrderingControls } from './orderingControls';
import type { createSentences } from './sentences';
import type { createSeenResults } from './seenResults';
import type { createTranscriptView } from './transcriptView';
import type { createToolViews } from './toolViews';
import type { createViewport } from './viewport';
import type { createPaging } from './paging';
import { compactCount, machineShorts, preview } from '../domain/format';
import { createTraceCalculations } from '../domain/trace';
import type { Entry, Handoff, Session, TokenKind, Turn } from '../domain/types';
import type {
  HarnessMark,
  InboxRow,
  LiveRow,
  MenuRun,
  SentencePart,
  SessionMenuSnapshot,
} from '../lib';
import {
  renderHomeScreen,
  renderMachineScreen,
  renderMachinesScreen,
  renderSessionScreen,
  renderTraceScreen,
} from '../lib';
import type { Hop } from '../lib/trace';
type ToolEntry = Extract<Entry, { k: 'tool' }> & { full?: boolean; scriptLoaded?: boolean };
interface ScreenViewsHost {
  controlOwner: Pick<ReturnType<typeof createLocalControl>, 'view'>;
  darkTheme: () => boolean;
  ago: (t: number) => string;
  navigation: NavigationController;
  clock: (t: number) => string;
  admin: { href: string; label: string } | null;
  now: number;
  scope: EffectScope;

  pagingOwner: Pick<ReturnType<typeof createPaging>, 'loadPager'>;

  viewport: Pick<
    ReturnType<typeof createViewport>,
    'stopOpeningEndPin' | 'opener' | 'jumpToLatest'
  >;

  toolViewsOwner: Pick<
    ReturnType<typeof createToolViews>,
    'openStepViewer' | 'openScript' | 'openImage'
  >;

  transcripts: Pick<TranscriptStore, 'entries'>;

  transcriptView: Pick<
    ReturnType<typeof createTranscriptView>,
    'transcriptEntries' | 'transcriptSnapshot' | 'verb'
  >;

  seenResultsOwner: Pick<ReturnType<typeof createSeenResults>, 'SEEN_RESULTS'>;

  sentencesOwner: Pick<ReturnType<typeof createSentences>, 'sentenceHost' | 'sentenceSnapshot'>;

  orderingControlsOwner: Pick<
    ReturnType<typeof createOrderingControls>,
    'orderList' | 'orderScope' | 'pageSig' | 'pageState' | 'byLast'
  >;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  seenPersistenceOwner: Pick<ReturnType<typeof createSeenPersistence>, 'markSeenResults'>;

  destination: Pick<ReturnType<typeof createDestination>, 'goSession' | 'goTrace' | 'go'>;

  modelStore: Pick<
    ViewerModelStore,
    | 'machines'
    | 'machineUp'
    | 'turns'
    | 'handoffs'
    | 'holds'
    | 'sessions'
    | 'machineLast'
    | 'starts'
    | 'turn'
  >;

  sessionChrome: Pick<
    ReturnType<typeof createSessionChrome>,
    'kindText' | 'observeTitle' | 'centre'
  >;

  domain: Pick<
    ReturnType<typeof createDomain>,
    | 'costForSession'
    | 'costMissing'
    | 'costText'
    | 'costForSessions'
    | 'asMoney'
    | 'sessionChildren'
    | 'nameOf'
    | 'oneLine'
    | 'answersOf'
    | 'traceRoot'
    | 'inbox'
    | 'isResult'
    | 'working'
    | 'onMachine'
    | 'movedOff'
    | 'movesOf'
    | 'usageTotal'
    | 'shortMoney'
    | 'byState'
    | 'machineLabel'
    | 'hcls'
    | 'hostOf'
    | 'turnEnd'
    | 'statWord'
    | 'machineLabels'
  >;
}
/** Owns screenViews behavior through explicit application ports. */
export function createScreenViews(host: ScreenViewsHost) {
  const MENU_KINDS: [string, TokenKind[]][] = [
    ['Input', ['input']],
    ['Output', ['output']],
    ['Cache write', ['cache_write_5m', 'cache_write_1h']],
    ['Cache read', ['cache_read']],
    ['Web search', ['web_search']],
  ];
  const COST_NOTE =
    "What these tokens would cost at API rates. Subscriptions aren't billed this way.";
  function costSnapshot(s: Session, kids: Session[]): SessionMenuSnapshot['cost'] {
    const own = host.domain.costForSession(s.id),
      all = host.domain.costForSession(s.id, true),
      missing = host.domain.costMissing(all),
      details = [];
    if (kids.length)
      details.push(
        { label: 'This session', value: host.domain.costText(own) },
        {
          label: kids.length === 1 ? 'Its run' : 'Its ' + kids.length + ' runs',
          value: host.domain.costText(host.domain.costForSessions(kids)),
        },
      );
    const reports = s.reported_runs ?? [],
      reported = reports.filter((r) => Number.isFinite(r.cost_usd));
    if (reports.length)
      details.push({
        label: HARNESS[s.harness] + "'s own figure",
        value: reported.length
          ? host.domain.asMoney(reported.reduce((n, r) => n + (r.cost_usd ?? 0), 0)) +
            (reported.length === 1 ? ', last run' : ', last ' + reported.length + ' runs')
          : 'not reported',
      });
    const check = [...(s.cost_check ?? [])]
      .reverse()
      .find(
        (c) => c.ok === false && Number.isFinite(c.computed_usd) && Number.isFinite(c.reported_usd),
      );
    const mismatch = check
      ? "Semon's estimate for that run is " +
        (check.reported_usd === 0
          ? 100
          : Math.round(
              (Math.abs((check.computed_usd ?? 0) - (check.reported_usd ?? 0)) /
                Math.abs(check.reported_usd ?? 0)) *
                100,
            )) +
        '% ' +
        ((check.computed_usd ?? 0) > (check.reported_usd ?? 0) ? 'above' : 'below') +
        ' ' +
        HARNESS[s.harness] +
        "'s figure: API rates differ from what a plan is charged."
      : undefined;
    const children = host.domain.sessionChildren(),
      runs: MenuRun[] = [],
      walk = (id: string, depth: number) => {
        for (const c of [...(children.get(id) ?? [])].sort((a, b) => b.last - a.last)) {
          const cost = host.domain.costText(host.domain.costForSession(c.id));
          runs.push({
            id: c.id,
            depth,
            label:
              'Open ' +
              c.name +
              ', ' +
              host.sessionChrome.kindText(c) +
              ', ' +
              STATE[c.state] +
              ', ' +
              cost,
            name: c.name,
            kind: host.sessionChrome.kindText(c),
            state: c.state,
            stateLabel: STATE[c.state],
            cost,
          });
          walk(c.id, depth + 1);
        }
      };
    if (kids.length) walk(s.id, 0);
    const models = Object.entries(all.by_model ?? {}).map(([id, model]) => {
      const priced = model.usd != null && !missing.includes(id),
        rows = [];
      for (const [label, keys] of MENU_KINDS) {
        const tokens = keys.reduce((n, k) => n + (Number(model.tokens?.[k]) || 0), 0),
          usd = keys.reduce((n, k) => n + (Number(model.usd_by_kind?.[k]) || 0), 0);
        if (tokens === 0 && (!priced || usd < 0.005)) continue;
        rows.push({
          label,
          count: tokens ? compactCount(tokens) : '',
          exact: tokens ? tokens.toLocaleString() : undefined,
          cost: priced ? host.domain.asMoney(usd) : '—',
        });
      }
      return { id, rows };
    });
    return {
      figure: host.domain.costText(kids.length ? all : own),
      caption: kids.length
        ? 'this session and its ' + kids.length + (kids.length === 1 ? ' run' : ' runs')
        : 'this session',
      note:
        COST_NOTE +
        ([s, ...kids].some((s) => s.claude_usage)
          ? ' Copied usage is excluded. Its original owner and the fresh usage total are unknown.'
          : '') +
        ([s, ...kids].some((s) => s.copilot)
          ? ' Copilot usage is a cumulative saved snapshot. Fresh usage and complete cost are unknown.'
          : '') +
        (missing.length ? ' No price for ' + missing.join(', ') + '.' : ''),
      details,
      mismatch,
      runs,
      models,
      includesRuns: !!kids.length,
    };
  }

  // ---- Home: what needs you, then what is running ------------------------------------------------------
  const upCount = () =>
    Object.keys(host.modelStore.machines).filter((m) => host.modelStore.machineUp[m]).length;
  let allAnswered = false;
  function harnessSnapshot(id: string): HarnessMark | undefined {
    const h = Object.hasOwn(HARNESSES, id) ? HARNESSES[id] : null;
    return h
      ? { id, name: h.name, light: h.icon.light, dark: h.icon.dark, darkTheme: host.darkTheme() }
      : undefined;
  }
  function liveSnapshot(s: Session, showMachine: boolean): LiveRow {
    const cur = (host.modelStore.turns[s.id] ?? []).at(-1),
      msg = cur?.start?.brief ?? cur?.u?.text;
    const inb =
      cur?.start ??
      (cur?.u
        ? { from: 'you' }
        : host.modelStore.handoffs.find((h) => h.to === s.id && h.kind !== 'move'));
    return {
      id: s.id,
      name: s.name,
      state: s.state,
      stateLabel: STATE[s.state] ?? s.state,
      status: s.state === 'work' ? HARNESS[s.harness] : host.ago(s.last),
      harness: s.state === 'work' ? harnessSnapshot(s.harness) : undefined,
      detail: [
        showMachine ? host.modelStore.machines[s.machine] : null,
        inb ? (inb.from === 'you' ? 'for you' : 'for ' + host.domain.nameOf(inb.from)) : null,
        msg ? host.domain.oneLine(msg) : null,
      ]
        .filter(Boolean)
        .join(' · '),
      activity:
        s.state === 'work' && s.activity
          ? [s.activity[0], s.activity[1], s.activity[2]]
          : undefined,
    };
  }
  function inboxSnapshot(h: Handoff, quiet: boolean = false): InboxRow {
    const sid = h.kind === 'move' ? h.to : h.from,
      t = host.modelStore.holds.get(h.id),
      s = host.modelStore.sessions[sid];
    const parts: SentencePart[] = [],
      part = (className: string, text: string, tip?: string) =>
        parts.push({ className, text, tip });
    let path = I.more;
    if (h.kind === 'ask') {
      path = I.ask;
      part('who', host.domain.nameOf('you'));
      part('verb', ' asked ');
      part('who', host.domain.nameOf(h.to));
    } else if (h.kind === 'spawn' || h.kind === 'relay') {
      path = I.out;
      part('who', host.domain.nameOf(h.from));
      part(
        'verb',
        h.kind === 'spawn'
          ? ' handed off to ' +
              (host.modelStore.sessions[h.to]?.kind === 'Subagent'
                ? 'subagent'
                : (host.modelStore.sessions[h.to]?.kind ?? '')) +
              ' '
          : ' relayed to ',
      );
      part('who', host.domain.nameOf(h.to));
    } else if (h.kind === 'move') {
      path = I.move;
      const short = machineShorts(
        [h.fromMachine, h.toMachine].map((id) => [id, host.modelStore.machines[id] ?? id]),
      );
      part('verb', 'Semon moved ');
      part('who', host.domain.nameOf(h.to));
      part('verb', ' from ');
      part(
        'verb mach',
        short.get(h.fromMachine) ?? h.fromMachine,
        'Machine: ' + (host.modelStore.machines[h.fromMachine] ?? h.fromMachine),
      );
      part('verb', ' to ');
      part(
        'verb mach',
        short.get(h.toMachine) ?? h.toMachine,
        'Machine: ' + (host.modelStore.machines[h.toMachine] ?? h.toMachine),
      );
    } else if (h.kind === 'toyou') {
      path =
        h.status === 'done' && (h.ask === 'question' || h.ask === 'decision')
          ? I.done
          : h.ask === 'question'
            ? I.qc
            : h.ask === 'decision'
              ? I.decide
              : I.result;
      part('who', host.domain.nameOf(h.from));
      part(
        'verb',
        { question: ' asked you', result: ' sent you a result', decision: ' needs your decision' }[
          h.ask
        ],
      );
    }
    const answer = quiet ? host.domain.answersOf(h) : null,
      root = t ? host.domain.traceRoot(t) : null,
      msg = root?.start?.from === 'you' ? root.start.brief : root?.u?.text;
    return {
      id: h.id,
      quiet,
      icon: quiet && h.kind === 'toyou' ? I.done : path,
      parts,
      age: host.ago(h.at),
      preview: preview(h.brief ?? ''),
      answer: answer
        ? answer.length
          ? 'You answered: ' + answer.join(' · ')
          : 'Answered · reply not in these logs'
        : undefined,
      origin: root
        ? msg
          ? { message: host.domain.oneLine(msg) }
          : { text: 'Started by ' + host.domain.nameOf(root.start ? root.start.from : root.sid) }
        : undefined,
      context: [HARNESS[s.harness], host.modelStore.machines[s.machine]].join(' · '),
      harness: harnessSnapshot(s.harness),
      trace: t?.out.length ? t.id : undefined,
    };
  }
  const activityHost = {
    session: (...args: Parameters<typeof host.destination.goSession>) =>
      host.destination.goSession(...args),
    committed: (...args: Parameters<typeof host.sessionChrome.observeTitle>) =>
      host.sessionChrome.observeTitle(...args),
    trace: (...args: Parameters<typeof host.destination.goTrace>) =>
      host.destination.goTrace(...args),
    inbox(id: string) {
      const h =
        host.modelStore.handoffs.find((h) => h.id === id) ??
        host.domain.inbox().find((h) => h.id === id);
      if (!h) return;
      if (host.domain.isResult(h)) host.seenPersistenceOwner.markSeenResults([h]);
      host.destination.goSession(
        h.kind === 'move' ? h.to : h.from,
        host.modelStore.holds.get(h.id)?.id,
      );
    },
    answered() {
      allAnswered = true;
      host.documentRendererOwner.render();
    },
  };
  function renderHome(page: HTMLElement) {
    const open = host.domain.inbox(),
      running = host.domain.working(),
      many = Object.keys(host.modelStore.machines).length > 1;
    const rows = host.orderingControlsOwner.orderList(
      host.orderingControlsOwner.orderScope(
        'page',
        host.orderingControlsOwner.pageSig(),
        host.navigation.route,
        host.orderingControlsOwner.pageState(),
      ),
      'working',
      running,
      (...args: Parameters<typeof host.orderingControlsOwner.byLast>) =>
        host.orderingControlsOwner.byLast(...args),
    );
    const done = host.modelStore.handoffs
      .filter((h) => h.kind === 'toyou' && h.status === 'done')
      .sort((a, b) => b.at - a.at);
    renderHomeScreen(
      page,
      {
        control: host.controlOwner.view(),
        waiting: open.length,
        working: running.length,
        up: upCount(),
        machines: Object.keys(host.modelStore.machines).length,
        inbox: open.map((h) => inboxSnapshot(h, false)),
        live: rows.map((s) => liveSnapshot(s, many)),
        answered: (allAnswered ? done : done.slice(0, 3)).map((h) => inboxSnapshot(h, true)),
        totalAnswered: done.length,
        allAnswered,
      },
      activityHost,
    );
  }
  // ---- Machines: where sessions run, and what happens when a machine goes away ------------------------------
  function renderMachines(page: HTMLElement) {
    const ms = Object.keys(host.modelStore.machines).sort(
      (a, b) => Number(host.modelStore.machineUp[a]) - Number(host.modelStore.machineUp[b]),
    );
    const rows = ms.map((m) => {
      const here = host.domain.onMachine(m),
        w = here.filter((s) => s.state === 'work').length,
        up = host.modelStore.machineUp[m],
        state = !up ? 'err' : w ? 'work' : 'idle';
      const mv = host.domain.movedOff(m).length,
        mh = host.domain.movesOf(m).find((h) => h.kind === 'move' && h.fromMachine === m);
      return {
        id: m,
        name: host.modelStore.machines[m],
        state,
        stateLabel: STATE[state] ?? state,
        status: !up ? 'offline' : w ? 'up' : 'idle',
        detail: up
          ? [w + ' working', here.length + (here.length === 1 ? ' session' : ' sessions')].join(
              ' · ',
            )
          : [
              'Not responding' +
                (mh
                  ? ' since ' + host.clock(mh.at)
                  : host.modelStore.machineLast[m] != null
                    ? ' since ' + host.clock(host.modelStore.machineLast[m])
                    : ''),
              mv ? mv + (mv === 1 ? ' session' : ' sessions') + ' moved off' : null,
            ]
              .filter(Boolean)
              .join(' · '),
      };
    });
    renderMachinesScreen(
      page,
      { rows, up: upCount(), working: host.domain.working().length, admin: host.admin },
      {
        machine(id) {
          host.destination.go({ v: 'machine', id });
        },
        admin(href) {
          location.assign(href);
        },
        committed: (...args: Parameters<typeof host.sessionChrome.observeTitle>) =>
          host.sessionChrome.observeTitle(...args),
      },
    );
  }
  function renderMachine(page: HTMLElement, m: string) {
    const here = host.domain.onMachine(m),
      off = host.domain.movedOff(m),
      moves = host.domain.movesOf(m);
    const ordered = host.orderingControlsOwner.orderList(
      host.orderingControlsOwner.orderScope(
        'page',
        host.orderingControlsOwner.pageSig(),
        host.navigation.route,
        host.orderingControlsOwner.pageState(),
      ),
      'machine:' + m,
      here,
      (...args: Parameters<typeof host.domain.byState>) => host.domain.byState(...args),
    );
    renderMachineScreen(
      page,
      {
        name: host.modelStore.machines[m],
        totalSessions: here.length,
        sessions: ordered.map((s) => liveSnapshot(s, false)),
        off: off.map((s) => ({
          ...liveSnapshot(s, false),
          detail: 'Now on ' + host.modelStore.machines[s.machine],
        })),
        moves: moves.map((h) => inboxSnapshot(h, true)),
      },
      activityHost,
    );
  }

  // ---- Trace: one turn and what it set off -------------------------------------------------------------------------
  // The root is the turn. Each spawn or relay it sent leads to the turn that handoff started in the receiving session,
  // and on down from there; a message to you is a leaf. One rail, as everywhere: depth shows as a smaller node.
  // One observer measures expandable messages, and releases detached nodes after a redraw.
  const RUN_EXPANDED = new Map<string, Set<string>>();
  const { agentSnapshot } = createTraceCalculations(
    {
      sessions: host.modelStore.sessions,
      turns: host.modelStore.turns,
      starts: host.modelStore.starts,
    },
    host.domain,
    () => host.now,
    RUN_EXPANDED,
    HARNESS,
    STATE,
  );
  function renderTrace(page: HTMLElement, id: string) {
    const root = host.modelStore.turn.get(id);
    if (!root) {
      renderTraceScreen(
        page,
        { empty: true, hops: [] },
        {
          ...host.sentencesOwner.sentenceHost,
          committed: (...args: Parameters<typeof host.sessionChrome.observeTitle>) =>
            host.sessionChrome.observeTitle(...args),
          fold() {},
        },
      );
      return;
    }
    const visited = new Set<string>(),
      read = (turn: Turn | undefined) => {
        if (!turn || visited.has(turn.id)) return;
        visited.add(turn.id);
        host.seenPersistenceOwner.markSeenResults(turn.out);
        for (const h of turn.out)
          if (h.kind === 'spawn' || h.kind === 'relay') read(host.modelStore.starts.get(h.id));
      };
    read(root);
    const seen = new Set([root.id]),
      sessions = new Set([root.sid]),
      scope = new Set([root.sid]),
      reached = new Set([root.id]);
    let n = 0;
    const reach = (turn: Turn) => {
      for (const h of turn.sent) {
        scope.add(h.kind === 'toyou' ? h.from : h.to);
        const child =
          h.kind === 'spawn' || h.kind === 'relay' ? host.modelStore.starts.get(h.id) : null;
        if (child && !reached.has(child.id)) {
          reached.add(child.id);
          reach(child);
        }
      }
    };
    reach(root);
    const meta = (
      state: string,
      text: string,
      sid: string,
      turn: Turn | null | undefined,
      note?: string,
    ) => {
      const s = host.modelStore.sessions[sid],
        label = host.domain.machineLabel(s, scope);
      return {
        state,
        stateLabel: STATE[state] ?? state,
        text,
        chip: s ? [s.kind ?? HARNESS[s.harness], label].filter(Boolean).join(' · ') : undefined,
        chipClass: s ? host.domain.hcls(sid) : undefined,
        tip: label ? 'Machine: ' + host.domain.hostOf(s) : undefined,
        harness: s ? harnessSnapshot(s.harness) : undefined,
        note: note ?? undefined,
        session: s && !s.stub ? sid : undefined,
        turn: turn?.id,
        name: s?.name,
      };
    };
    const start = root.start,
      text = start ? start.brief : root.u?.text,
      initial = start
        ? host.sentencesOwner.sentenceSnapshot(start, null)
        : root.u
          ? host.sentencesOwner.sentenceSnapshot(
              {
                kind: 'ask',
                id: root.id,
                from: 'you',
                to: root.sid,
                at: root.at ?? host.now,
                status: 'done',
                brief: '',
              },
              null,
            )
          : {
              icon: I.more,
              parts: [
                { className: 'who', text: host.modelStore.sessions[root.sid].name },
                { className: 'verb', text: " · a turn whose start isn't in these logs" },
              ],
            };
    const outcome = host.domain.turnEnd(root),
      hops: Hop[] = [
        {
          key: 'root:' + root.id,
          className: 'k-root',
          icon: initial.icon,
          parts: initial.parts,
          nodeClass: host.domain.hcls(start ? start.from : root.u ? 'you' : root.sid),
          turn: root.id,
          handoff: start?.id,
          time: start ? host.clock(start.at) : undefined,
          brief: text || undefined,
          meta: meta(outcome?.st ?? 'idle', outcome?.text ?? 'Nothing recorded', root.sid, root),
        },
      ];
    const walk = (turn: Turn) => {
      for (const h of turn.sent) {
        const result = h.kind === 'toyou' && h.ask === 'result',
          child =
            h.kind === 'spawn' || h.kind === 'relay' ? host.modelStore.starts.get(h.id) : null,
          target = h.kind === 'toyou' ? h.from : h.to;
        const sentence = result
          ? {
              icon: I.result,
              parts: [{ className: 'verb', text: host.domain.statWord(h) ?? '' }],
            }
          : host.sentencesOwner.sentenceSnapshot(h, null);
        const hop: Hop = {
          key: h.id,
          className:
            'child k-' +
            h.kind +
            ' s-' +
            h.status +
            (child || h.kind === 'toyou' || h.kind === 'move' ? '' : ' stub'),
          icon: sentence.icon,
          parts: sentence.parts,
          nodeClass: host.domain.hcls(target),
          handoff: h.id,
          turn: child?.id,
          time: host.clock(h.at),
        };
        hops.push(hop);
        if (result) {
          n++;
          continue;
        }
        hop.brief = h.brief;
        hop.answers = host.domain.answersOf(h) ?? undefined;
        hop.result = h.result || undefined;
        if (h.kind === 'move') {
          hop.meta = meta('done', 'Moved', h.to, turn);
          continue;
        }
        n++;
        if (h.kind === 'toyou') {
          hop.meta = meta(
            host.domain.isResult(h)
              ? host.seenResultsOwner.SEEN_RESULTS.has(h.id)
                ? 'read'
                : 'new'
              : h.status === 'done'
                ? 'done'
                : h.status,
            host.domain.statWord(h) ?? '',
            h.from,
            turn,
          );
          continue;
        }
        sessions.add(h.to);
        const end = child && host.domain.turnEnd(child);
        hop.meta = meta(
          end ? (end.st ?? 'idle') : h.status === 'done' ? 'done' : h.status,
          end ? (end.text ?? 'Nothing recorded') : (host.domain.statWord(h) ?? ''),
          h.to,
          child,
          child ? undefined : "Its turn isn't in these logs",
        );
        if (child && !seen.has(child.id)) {
          seen.add(child.id);
          walk(child);
        }
      }
    };
    walk(root);
    renderTraceScreen(
      page,
      {
        empty: false,
        hops,
        agents: agentSnapshot(root),
        summary: [
          sessions.size + (sessions.size === 1 ? ' session' : ' sessions'),
          n + (n === 1 ? ' handoff' : ' handoffs'),
          [...host.domain.machineLabels(scope).values()].join(', '),
        ]
          .filter(Boolean)
          .join(' · '),
      },
      {
        ...host.sentencesOwner.sentenceHost,
        committed: (...args: Parameters<typeof host.sessionChrome.observeTitle>) =>
          host.sessionChrome.observeTitle(...args),
        fold(id) {
          RUN_EXPANDED.get(root.id)?.add(id);
          renderTrace(page, root.id);
        },
      },
    );
    return [
      sessions.size + (sessions.size === 1 ? ' session' : ' sessions'),
      n + (n === 1 ? ' handoff' : ' handoffs'),
      [...host.domain.machineLabels(scope).values()].join(', '),
    ]
      .filter(Boolean)
      .join(' · ');
  }
  // ---- Session page --------------------------------------------------------------------------------------------------
  function renderSession(
    page: HTMLElement,
    sid: string,
    opts: { only?: ReadonlySet<string> } = {},
  ) {
    host.seenPersistenceOwner.markSeenResults(
      host.modelStore.handoffs.filter((h) => host.domain.isResult(h) && h.from === sid),
    );
    const raw = new Map(
      host.transcriptView
        .transcriptEntries(host.transcripts.entries[sid] ?? [], sid)
        .map((e) => [e.slot != null ? sid + '#slot:' + e.slot : e.key, e]),
    );
    renderSessionScreen(
      page,
      {
        ...host.transcriptView.transcriptSnapshot(sid, opts),
        control: host.controlOwner.view(sid),
      },
      {
        ...host.sentencesOwner.sentenceHost,
        committed: (...args: Parameters<typeof host.sessionChrome.observeTitle>) =>
          host.sessionChrome.observeTitle(...args),
        trace: (...args: Parameters<typeof host.destination.goTrace>) =>
          host.destination.goTrace(...args),
        toolAll(key, label) {
          const e = raw.get(key);
          if (e?.k === 'tool') {
            const [ic, v] = host.transcriptView.verb(e.name);
            host.toolViewsOwner.openStepViewer(e, v, ic, label);
          }
        },
        script(key) {
          const e = raw.get(key);
          if (e?.k === 'tool') host.toolViewsOwner.openScript(e);
        },
        image: (...args: Parameters<typeof host.toolViewsOwner.openImage>) =>
          host.toolViewsOwner.openImage(...args),
        background(call, trigger) {
          const target = trigger
            .closest('section[aria-label="Transcript"]')
            ?.querySelector<HTMLElement>('.step[data-tid="' + CSS.escape(call) + '"]');
          if (!target) return;
          host.viewport.stopOpeningEndPin();
          for (let parent = target.parentElement; parent; parent = parent.parentElement) {
            const toggle = host.viewport.opener(parent);
            if (toggle?.getAttribute('aria-expanded') === 'false') toggle.click();
          }
          host.sessionChrome.centre(target);
          target.classList.add('flash');
          host.scope.timeout(() => target.classList.remove('flash'), 1500);
        },
        pager(button) {
          host.pagingOwner.loadPager(button, true);
        },
        jump: (...args: Parameters<typeof host.viewport.jumpToLatest>) =>
          host.viewport.jumpToLatest(...args),
      },
    );
  }
  // Whether a session page ends in its status line: a child's when it is running or has returned, any other session's always.
  // renderSession and patchSession share it.
  const showsFooter = (s: Session, origin: Handoff | undefined) =>
    origin
      ? s.state === 'work' ||
        s.state === 'done' ||
        s.state === 'err' ||
        origin.status === 'done' ||
        origin.status === 'err'
      : !s.stub && !s.role && s.state in STATE;

  return {
    harnessSnapshot,
    costSnapshot,
    showsFooter,
    renderHome,
    renderMachines,
    renderMachine,
    renderTrace,
    renderSession,
  };
}

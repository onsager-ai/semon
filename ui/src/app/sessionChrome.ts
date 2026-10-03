import { render as releaseRoot } from 'preact';
import { shortModel } from '../domain/format';
import type { Entry, Session } from '../domain/types';
import type { BarLabel, HarnessMark, MenuAction, MenuDetail } from '../lib';
import {
  createPanelChrome,
  createViewerBar,
  measureViewerBar,
  renderSessionMenu,
  setGeometry,
} from '../lib';
import { createErrorNavigation } from '../navigation/errors';
interface TailResult {
  cut: number | null;
  reload?: boolean;
  patched?: Entry[];
}
interface TopbarOptions {
  session?: Session;
  traceSession?: Session;
  lineage?: Session[];
  line2?: BarLabel[];
  analytics?: boolean;
}
interface SessionChromeHost {
  HARNESS: { [k: string]: string };
  ACCOUNT: import('../lib/account').Account | null;
  wideMode: boolean;
  setWideMode: (on: boolean) => void;
  findOpen: boolean;
  STATE: Record<string, string>;
  phone: MediaQueryList;
  analyticsRange: number;
  find: string;
  show: { messages: boolean; tools: boolean; thinking: boolean };
  countOf: (s: import('../domain/types').Session, key: 'calls' | 'errors') => number | null;
  I: Record<string, string>;
  goSession: (id: string, turn?: string | undefined) => void;
  render: () => void;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  SHOW_ALL: { messages: boolean; tools: boolean; thinking: boolean };
  SESS: Record<string, import('../domain/types').Session>;
  currentScroll: () => number;
  restoreScroll: (top: number) => void;
  refreshAnalytics: (asked?: boolean) => Promise<void>;
  navigation: import('../navigation/routes').NavigationController;
  shellChrome: import('../lib/shell').ShellChrome | null;
  scope: import('../app/effects').EffectScope;
  lineageOf: (sid: string) => import('../domain/types').Session[];
  scroller: () => HTMLElement;
  edge: () => number;
  scrollProgrammatically: (fn: () => void, jump?: boolean) => void;
  syncJump: () => void;
  saveHistoryScroll: () => void;
  TX: Record<string, import('../domain/types').Entry[]>;
  TXM: Record<string, import('../domain/types').TranscriptMeta>;
  TOK: Record<string, string>;
  SIDEBAR_ONLY: boolean;
  capture: () => import('../navigation/scroll').ScrollSnapshot;
  restore: (st: import('../navigation/scroll').ScrollSnapshot, pin?: boolean) => void;
  opener: (n: HTMLElement) => HTMLButtonElement | null;
  resetPagerInput: () => void;
  stopOpeningEndPin: () => void;
  fetchTx: (
    sid: string,
    q?: string,
    where?: import('../state/transcript').PageDirection | undefined,
    signal?: AbortSignal | undefined,
    onPage?: (() => void) | undefined,
  ) => Promise<void>;
  dropTx: (sid: string) => void;
  spread: (sid: string) => void;
  tail: (sid: string) => Promise<TailResult>;
  TURNS: Record<string, import('../domain/types').Turn[]>;
  hasTurn: (t: import('../domain/types').Turn) => boolean;
  descendantsOf: (
    sid: string,
    children: ReadonlyMap<string, import('../domain/types').Session[]>,
    out?: import('../domain/types').Session[],
    seen?: Set<string>,
  ) => import('../domain/types').Session[];
  sessionChildren: () => Map<string, import('../domain/types').Session[]>;
  costForSessions: (
    sessions: Iterable<import('../domain/types').Session>,
  ) => Required<import('../domain/types').Cost>;
  costForSession: (sid: string, includeRuns?: boolean) => Required<import('../domain/types').Cost>;
  MACHINE: Record<string, string>;
  hostOf: (s: import('../domain/types').Session) => string;
  costText: (cost: import('../domain/types').Cost) => string;
  onMachine: (m: string) => import('../domain/types').Session[];
  MACHINE_UP: Record<string, boolean>;
  movedOff: (m: string) => import('../domain/types').Session[];
  MACHINE_LAST: Record<string, number>;
  clock: (t: number) => string;
  syncLayoutPrefs: () => void;
  viewerEl: HTMLDialogElement | null;
  dialogs: Map<HTMLDialogElement, { destroy(): void }>;
  disposed: boolean;
  skipPop: boolean;
  pendingSessionOpen: string | null;
  LIVE: import('../lib/live').LiveState;
  refresh: (dirty?: ReadonlySet<string> | null) => void;
  TURN: Map<string, import('../domain/types').Turn>;
  harnessSnapshot: (id: string) => import('../lib/screens').HarnessMark | undefined;
  branchOf: (s: import('../domain/types').Session) => string;
  dur: (a: number, b: number | null | undefined) => string;
  costSnapshot: (
    s: import('../domain/types').Session,
    kids: import('../domain/types').Session[],
  ) => {
    figure: string;
    caption: string;
    note: string;
    details: readonly import('../lib/session-menu').MenuDetail[];
    mismatch?: string | undefined;
    runs: readonly import('../lib/session-menu').MenuRun[];
    models: readonly import('../lib/session-menu').MenuTokenModel[];
    includesRuns: boolean;
  };
  afterPop: (() => void) | null;
  goTrace: (turn: string) => void;
}
/** Owns sessionChrome behavior through explicit application ports. */
export function createSessionChrome(host: SessionChromeHost) {
  const kindText = (s: Session) => s.kind ?? host.HARNESS[s.harness] ?? s.harness;
  // The model the session is on (what the line's label abbreviates), so the label, its tip and the Details row name the same one; a session that used more than one is priced per model in the cost section.
  const modelIdOf = (s: Session) => s.model ?? Object.keys(s.tokens_by_model ?? {})[0];
  const viewerBar = createViewerBar();
  function renderTopbar(
    title: string,
    crumb: { label: string; go: () => void } | null = null,
    opts: TopbarOptions = {},
  ) {
    const s = opts.session ?? opts.traceSession;
    const account = host.ACCOUNT
      ? {
          account: host.ACCOUNT,
          compact: false,
          wide: host.wideMode,
          onWideChange: () => host.setWideMode(!host.wideMode),
        }
      : null;
    const mode = s && errOn(s.id) ? 'errors' : s && host.findOpen ? 'find' : 'normal';
    const content = viewerBar.update(
      {
        mode,
        name: title,
        session: s?.id,
        state: s?.state,
        stateLabel: s ? host.STATE[s.state] : undefined,
        stateTip: s ? 'Status: ' + host.STATE[s.state] + ' · ' + turnsLabel(s) : undefined,
        showState: !!s && !opts.traceSession && !(host.phone.matches && opts.lineage?.length),
        ancestors: !host.phone.matches
          ? (opts.lineage?.map((a) => ({
              ...a,
              harnessName: host.HARNESS[a.harness] ?? a.harness,
            })) ?? [])
          : [],
        crumb: opts.lineage?.length ? undefined : crumb?.label,
        labels: opts.line2 ?? [],
        trace: !!opts.traceSession,
        analytics: !!opts.analytics,
        days: host.analyticsRange,
        query: host.find,
        count: matchText(matchCount()),
        filter:
          host.show.messages && host.show.tools ? 'all' : host.show.messages ? 'messages' : 'steps',
        failed: s ? (host.countOf(s, 'errors') ?? 0) : 0,
        signals: s ? signalCount(s) : 0,
        errorMode: ERR.mode,
        errorText: ERR.notice ?? errText(),
        errorDisabled: ERR.listed && !ERR.slots.length,
        icons: {
          search: host.I.search,
          back: host.I.back,
          x: host.I.x,
          up: host.I.up,
          more: host.I.more,
          dn: host.I.dn,
        },
      },
      {
        ancestor: host.goSession,
        crumb() {
          crumb?.go();
        },
        find() {
          host.findOpen = true;
          host.render();
          host.$('#find')?.focus();
        },
        closeFind() {
          host.findOpen = false;
          host.find = '';
          host.show = { ...host.SHOW_ALL };
          host.render();
        },
        query(value) {
          host.find = value.toLowerCase();
          host.render();
        },
        filter(key) {
          if (key === 'failures' || key === 'signals') {
            s && openErrors(s.id, key === 'signals' ? 'signals' : 'errors');
            return;
          }
          host.show =
            key === 'messages'
              ? { messages: true, tools: false, thinking: false }
              : key === 'steps'
                ? { messages: false, tools: true, thinking: false }
                : { ...host.SHOW_ALL };
          host.render();
        },
        menu(trigger, runs) {
          s && openSessionMenu(host.SESS[s.id] ?? s, trigger, runs ? '.runs' : undefined);
        },
        errors() {
          s && openErrors(s.id);
        },
        closeErrors,
        step: stepErrors,
        range(days) {
          if (host.analyticsRange === days) return;
          const top = host.currentScroll();
          host.analyticsRange = days;
          host.render();
          host.restoreScroll(top);
          host.refreshAnalytics(true);
        },
      },
    );
    const lead = {
      label: opts.traceSession ? 'Back to ' + s?.name : 'Open navigation',
      icon: opts.traceSession ? host.I.chev : host.I.menu,
      back: opts.traceSession
        ? () =>
            s &&
            host.goSession(
              s.id,
              'turn' in host.navigation.route ? host.navigation.route.turn : undefined,
            )
        : undefined,
    };
    host.shellChrome?.topbar(
      mode === 'normal'
        ? { titleSlot: content.titleSlot, actions: [content.actions], session: !!s, lead, account }
        : { mode: [content.mode], session: true, account, accountTarget: content.accountTarget },
    );
    if (s && mode === 'normal')
      host.scope.frame(() => {
        const line = host.$('#topbar .meta-line');
        if (line) measureViewerBar(host.$('#topbar'));
      });
  }
  // The label and buttons in place, so focus stays where it is.
  // ---- Errors mode: "N errors" steps through the session's failed steps ---------------------------------------------------------
  // The bar reads "Error k of N" with previous and next, and a close button (Escape). /api/tx?errors=1 says where every failed
  // step is (its slot), so a step on a page not loaded yet is reachable: a page next to the loaded range is added to it, one
  // further away replaces it with the page around the step. Each step is scrolled to the middle and marked, never opened;
  // its tool group opens so it shows. Closing puts back the pages, what was open and the scroll position from before. The
  // mode is its own controller, apart from find, so the two can become one mode later.
  const drawSessionBar = () => {
    const s = host.SESS['id' in host.navigation.route ? host.navigation.route.id : ''];
    if (host.navigation.route.v !== 'session' || !s) return;
    renderTopbar(s.name, null, {
      session: s,
      lineage: host
        .lineageOf('id' in host.navigation.route ? host.navigation.route.id : '')
        .slice(0, -1),
      line2: sessionLine(s),
    });
    setGeometry(document.documentElement, 'barHeight', host.$('#topbar').offsetHeight);
    syncBarLine();
  };
  // A redraw keeps focus on the bar's control that had it.
  const keepFocus = (fn: () => void) => {
    const id = document.activeElement?.id;
    fn();
    const n = id && document.getElementById(id);
    if (n && n !== document.activeElement) n.focus({ preventScroll: true });
  };
  function centre(node: HTMLElement) {
    if (!node.isConnected) return;
    const sc = host.scroller(),
      r = (node.querySelector(':scope > button') ?? node).getBoundingClientRect(),
      bottom = host.phone.matches
        ? window.innerHeight
        : host.$('#main').getBoundingClientRect().bottom;
    const d = (r.top + r.bottom) / 2 - (host.edge() + bottom) / 2;
    host.scrollProgrammatically(() => {
      if (Math.abs(d) >= 1) sc.scrollTop += d;
    });
    syncBarLine();
    host.syncJump();
    host.saveHistoryScroll();
  }
  const errorNavigation = createErrorNavigation({
    navigation: host.navigation,
    TX: host.TX,
    TXM: host.TXM,
    TOK: host.TOK,
    SESS: host.SESS,
    show: host.show,
    sidebarOnly: host.SIDEBAR_ONLY,
    page: () => host.$('#page'),
    drawSessionBar,
    render: host.render,
    keepFocus,
    countOf: (session, field) =>
      session ? (host.countOf(session, field) ?? undefined) : undefined,
    capture: () => host.capture(),
    restore: (saved) => host.restore(saved),
    opener: (node) => host.opener(node),
    resetPagerInput: host.resetPagerInput,
    stopOpeningEndPin: () => host.stopOpeningEndPin(),
    clearFind() {
      host.find = '';
    },
    centre,
    fetchTx: host.fetchTx,
    dropTx: host.dropTx,
    spread: host.spread,
    tail: (sid) => host.tail(sid),
  });
  const {
    state: ERR,
    text: errText,
    on: errOn,
    open: openErrors,
    step: stepErrors,
    mark: markError,
    drop: dropErrors,
    close: closeErrors,
    live: errorsLive,
  } = errorNavigation;
  const signalCount = (s: Session) => Object.values(s?.signals ?? {}).reduce((n, x) => n + x, 0);
  // A label is information; one that leads somewhere (`act`) is a button that looks the same, with its hit area padded to the tap size.
  const turnsLabel = (s: Session) => {
    const n = (host.TURNS[s.id] ?? []).filter(host.hasTurn).length;
    return n + (n === 1 ? ' turn' : ' turns');
  };
  // On a phone the line of labels leaves the bar and the state is the small dot before the title. The dot names the state for a screen reader, and
  // its tip (a tap on a phone) adds the turn count. A desktop hides it, since the line shows the state there.
  // "Started 21:57 on <machine>" stays on one line: the machine name ellipsises (its tip, only while cut off, has the whole name).
  // A session's line: its state, then kind, model, failed steps, machine, branch and API-equivalent cost, each a plain label.
  const sessionLine = (s: Session): BarLabel[] => {
    const failed = host.countOf(s, 'errors') ?? 0,
      runs = host.descendantsOf(s.id, host.sessionChildren()),
      cost = runs.length ? host.costForSessions([s, ...runs]) : host.costForSession(s.id),
      labels: BarLabel[] = [];
    const add = (
      key: string,
      text: string,
      tip: string | undefined,
      drop: number,
      extra: Partial<BarLabel> = {},
    ) => labels.push({ key, text, tip: tip ?? undefined, drop, ...extra });
    add('state', host.STATE[s.state], undefined, 0, {
      className: 'state ' + s.state,
      state: s.state,
      stateLabel: host.STATE[s.state],
    });
    add('kind', kindText(s), s.kind ? s.kind + ' · ' + host.HARNESS[s.harness] : undefined, 2);
    add(
      'model',
      shortModel(s.model),
      'Model: ' + modelIdOf(s) + (s.effort ? '. Reasoning effort: ' + s.effort : ''),
      3,
      { className: 'meta-model', effort: s.effort },
    );
    if (failed)
      add(
        'errors',
        failed + ' failed',
        failed + (failed === 1 ? ' failed step' : ' failed steps') + ': step through them',
        0,
        { className: 'lab-errs', action: 'errors' },
      );
    if (runs.length)
      add(
        'runs',
        runs.length + (runs.length === 1 ? ' run' : ' runs'),
        runs.length +
          (runs.length === 1 ? ' run' : ' runs') +
          ' under this session: open the list with their cost',
        1,
        { className: 'lab-runs', action: 'runs' },
      );
    add(
      'machine',
      host.MACHINE[s.machine],
      'Machine: ' + host.MACHINE[s.machine] + ' · ' + host.hostOf(s),
      5,
    );
    if (s.branch) add('branch', s.branch, 'Branch: ' + s.branch, 6);
    add(
      'cost',
      host.costText(cost),
      'API-equivalent cost' +
        (runs.length ? ', with ' + runs.length + (runs.length === 1 ? ' run' : ' runs') : '') +
        '. Details in the session menu.',
      7,
    );
    return labels;
  };
  const machineLine = (m: string): BarLabel[] => {
    const here = host.onMachine(m),
      w = here.filter((s) => s.state === 'work').length,
      up = host.MACHINE_UP[m];
    const sessions = here.length + (here.length === 1 ? ' session' : ' sessions');
    return [
      {
        key: 'state',
        text: up ? 'Up' : 'Not responding',
        drop: 0,
        className: 'state ' + (up ? 'done' : 'err'),
        state: up ? (w ? 'work' : 'idle') : 'err',
        stateLabel: host.STATE[up ? (w ? 'work' : 'idle') : 'err'],
      },
      {
        key: 'activity',
        text: up
          ? w + ' working · ' + sessions
          : host.movedOff(m).length
            ? host.movedOff(m).length + ' moved off'
            : [
                host.MACHINE_LAST[m] != null
                  ? 'Last seen ' + host.clock(host.MACHINE_LAST[m])
                  : null,
                sessions,
              ]
                .filter(Boolean)
                .join(' · '),
        drop: 1,
      },
    ];
  };
  // What Find counts: the matches on the page when there is a search or a filter, else nothing.
  const matchCount = () =>
    host.find || !host.show.messages || !host.show.tools || !host.show.thinking
      ? host
          .$('#page')
          .querySelectorAll<HTMLElement>(
            '.turns .msg, .turns .bubble, .turns .step:not(.bgend), .turns .event, .turns .child-card',
          ).length
      : null;
  const matchText = (n: number | null) =>
    n == null ? '' : n ? n + (n === 1 ? ' match' : ' matches') : 'No matches';
  // Find and filter: one mode. Search takes over the bar and the filters sit under it as chips, one choice at a time.
  // One observer for the current page title; the previous page's is disconnected so it can't flip the new bar.
  let titleObs = null;
  function observeTitle() {
    syncBarLine();
  }
  // The bar's divider shows only once the page has scrolled.
  function syncBarLine() {
    const y = host.phone.matches ? window.scrollY : host.$('#main').scrollTop;
    host.$('#topbar').classList.toggle('scrolled', y > 4);
  }
  if (!host.SIDEBAR_ONLY) {
    host.scope.listen(window, 'scroll', syncBarLine, { passive: true });
    host.scope.listen(host.$('#main'), 'scroll', syncBarLine, { passive: true });
  }
  if (!host.SIDEBAR_ONLY)
    host.scope.listen(
      window,
      'resize',
      () => {
        const l2 = host.$('#topbar .meta-line');
        if (l2 && host.navigation.route.v === 'session') measureViewerBar(host.$('#topbar'));
        host.syncLayoutPrefs();
        host.syncJump();
      },
      { passive: true },
    );

  // ---- Panels: one builder for the sheets and menus opened from the top bar --------------------------------------------
  // A phone gets a bottom sheet; a desktop a dialog, or for the session menu a panel that hangs from its button. Each is a
  // history entry, so back closes it without leaving the page, and a live update waits until it closes.
  function panel(
    title: string,
    opts: {
      cls?: string;
      sub?: string;
      label?: string;
      from?: HTMLElement | null;
      onClose?: () => void;
    } = {},
  ) {
    const chrome = createPanelChrome(
      { title, className: opts.cls, label: opts.label, sub: opts.sub },
      {
        opened() {
          host.viewerEl = chrome.dialog;
          document.documentElement.classList.add('panel-open');
          try {
            history.pushState(
              { ...host.navigation.route, sheet: 1, scrollTop: host.currentScroll() },
              '',
            );
          } catch {}
        },
        closed() {
          releaseRoot(null, chrome.body);
          host.dialogs.delete(chrome.dialog);
          if (host.disposed) return;
          const d = chrome.dialog;
          document.documentElement.classList.remove('panel-open');
          opts.onClose?.();
          if (host.viewerEl === d) {
            host.viewerEl = null;
            if (history.state?.sheet) {
              host.skipPop = true;
              history.back();
            } else if (host.pendingSessionOpen) {
              const id = host.pendingSessionOpen;
              host.pendingSessionOpen = null;
              host.goSession(id);
            }
          }
          if (host.LIVE.pending) host.refresh();
        },
      },
    );
    host.dialogs.set(chrome.dialog, chrome);
    return { d: chrome.dialog, body: chrome.body, show: () => chrome.show() };
  }

  // The session menu: actions, then details, then cost. It is the one place for all three.
  const RUNS_CAP = 5;
  // Shown once in the UI, at the foot of the session menu; NOTICE.md and the README carry it too. The harness marks are the property of their owners.
  const TRADEMARK_NOTICE =
    'Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.';
  function openSessionMenu(
    s: Session,
    anchor: HTMLElement | null = null,
    scrollTo: string | undefined = undefined,
  ) {
    const kids = host.descendantsOf(s.id, host.sessionChildren());
    const {
      d,
      body,
      show: open,
    } = panel(s.name, {
      cls: 'anchored session-menu',
      label: 'Session menu for ' + s.name,
      sub: [host.STATE[s.state], kindText(s), shortModel(s.model)].join(' · '),
      onClose: () => {
        anchor?.setAttribute('aria-expanded', 'false');
        anchor?.focus({ focusVisible: false });
      },
    });
    const traceTurn =
      host.navigation.route.v === 'trace'
        ? host.TURN.get(
            ('turn' in host.navigation.route ? host.navigation.route.turn : undefined) ?? '',
          )
        : (() => {
            const top = host.$('#topbar').getBoundingClientRect().bottom;
            const nodes = [
              ...document.querySelectorAll<HTMLElement>('#page .turn[data-turn]'),
            ].filter((n) => !n.closest('.cw-body'));
            const visible = nodes.find((n) => n.getBoundingClientRect().bottom > top);
            return host.TURN.get(visible?.dataset.turn ?? '') ?? (host.TURNS[s.id] ?? []).at(-1);
          })();
    const actions: MenuAction[] = [],
      addAction = (
        key: string,
        text: string,
        icon?: string,
        className?: string,
        note?: string,
        checked?: boolean,
        dot?: string,
      ) => actions.push({ key, text, icon, className, note, checked, dot });
    if (traceTurn?.out.length) addAction('trace', 'Trace this turn', host.I.trace, 'menu-trace');
    const command =
      s.harness === 'codex' ? 'codex resume ' + s.id : 'claude --resume ' + (s.sessionId ?? s.id);
    addAction('copy', 'Copy resume command', host.I.copy);
    if (s.harness === 'claude') addAction('external', 'Open in claude.ai', host.I.ext);
    if (!host.phone.matches)
      addAction('wide', 'Wide transcript', host.I.wide, undefined, undefined, host.wideMode);
    const signals = signalCount(s);
    if (signals)
      addAction('signals', signals + ' signals', undefined, 'menu-signals', 'Step through');
    const errorCount = host.countOf(s, 'errors') ?? 0;
    if (host.phone.matches) {
      if (errorCount)
        addAction(
          'errors',
          errorCount + ' failed',
          undefined,
          'menu-errors',
          'Step through',
          undefined,
          'err',
        );
      if (kids.length) addAction('runs', 'Runs · ' + kids.length, host.I.stack, 'menu-runs');
    }
    const machine = host.MACHINE[s.machine] ?? s.machine ?? 'Unknown machine',
      calls = host.countOf(s, 'calls');
    const detailRows: [string, string | number | undefined, boolean?, HarnessMark?][] = [
      ['Status', host.STATE[s.state] + ' · ' + turnsLabel(s)],
      ...(s.kind ? [['Kind', s.kind] as [string, string]] : []),
      ['Harness', host.HARNESS[s.harness] ?? s.harness, false, host.harnessSnapshot(s.harness)],
      ['Model', modelIdOf(s), true],
      ...(s.effort ? [['Effort', s.effort] as [string, string]] : []),
      [
        'Machine',
        machine +
          (host.hostOf(s) !== machine ? ' · ' + host.hostOf(s) : '') +
          (s.movedFrom ? ' (moved from ' + (host.MACHINE[s.movedFrom] ?? s.movedFrom) + ')' : ''),
      ],
      ['Directory', s.cwd ?? s.dir ?? s.directory, true],
      [s.worktree ? 'Worktree' : 'Branch', host.branchOf(s), true],
      ['Tool calls', calls == null ? '—' : String(calls)],
      ...(errorCount ? [['Errors', String(errorCount)] as [string, string]] : []),
      ['Started', host.clock(s.start)],
      ['Duration', host.dur(s.start, s.state === 'work' ? null : s.last)],
      ['Process id', s.pid, true],
      ['Session id', s.sessionId ?? s.id, true],
      ...Object.entries(s.signals ?? {}).map(([kind, n]): [string, string] => [
        'Signals · ' + kind,
        String(n),
      ]),
    ];
    const details: MenuDetail[] = detailRows
      .filter(([, value]) => value != null && value !== '')
      .map(([label, value, mono, harness]) => ({ label, value: String(value), mono, harness }));
    renderSessionMenu(
      body,
      {
        actions,
        command,
        path: host.phone.matches
          ? host
              .lineageOf(s.id)
              .map((a) => ({
                id: a.id,
                name: a.name,
                harness: a.harness,
                harnessName: host.HARNESS[a.harness] ?? a.harness,
              }))
          : [],
        status: host.STATE[s.state] + ' · ' + turnsLabel(s),
        state: s.state,
        stateLabel: host.STATE[s.state],
        details,
        cost: host.costSnapshot(s, kids),
        notice: TRADEMARK_NOTICE,
        icons: { chevron: host.I.chev, copy: host.I.copy },
      },
      {
        wide: () => host.wideMode,
        session(id) {
          host.pendingSessionOpen = id;
          d.close();
        },
        action(key) {
          if (key === 'trace' && traceTurn) {
            host.afterPop = () => host.goTrace(traceTurn.id);
            d.close();
          } else if (key === 'wide') host.setWideMode(!host.wideMode);
          else if (key === 'signals' || key === 'errors') {
            d.close();
            s && openErrors(s.id, key === 'signals' ? 'signals' : 'errors');
          } else if (key === 'runs')
            body.querySelector('.runs')?.scrollIntoView({ block: 'start' });
        },
      },
    );
    anchor?.setAttribute('aria-expanded', 'true');
    open();
    if (scrollTo) body.querySelector(scrollTo)?.scrollIntoView({ block: 'nearest' });
    return d;
  }
  // 739,682 reads "740k" and 12,422,228 "12.4M"; the exact figure is the cell's tooltip.

  return {
    errorNavigation,
    viewerBar,
    drawSessionBar,
    syncBarLine,
    dropErrors,
    modelIdOf,
    kindText,
    observeTitle,
    centre,
    panel,
    renderTopbar,
    machineLine,
    sessionLine,
    errOn,
    markError,
    errorsLive,
  };
}

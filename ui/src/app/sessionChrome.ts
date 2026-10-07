import { I } from './registry';
import { STATE } from './registry';
import { HARNESS } from './registry';
import type { ViewerModelStore } from '../state/model';
import type { TranscriptStore } from '../state/transcript';
import type { EffectScope } from '../app/effects';
import type { NavigationController } from '../navigation/routes';
import type { Account } from '../lib/account';
import type { createLayout } from './layout';
import type { createDomain } from '../domain/calculations';
import type { createDestination } from './destination';
import type { createDocumentRenderer } from './documentRenderer';
import type { createHistoryScroll } from './historyScroll';
import type { createAnalytics } from './analytics';
import type { createAccountControls } from './accountControls';
import type { createViewport } from './viewport';
import type { createPaging } from './paging';
import type { createTransport } from './transport';
import type { createLiveUpdates } from './liveUpdates';
import type { createToolViews } from './toolViews';
import type { createLiveModel } from './liveModel';
import type { createApplicationRefresh } from './applicationRefresh';
import type { createScreenViews } from './screenViews';
import { render as releaseRoot } from 'preact';
import { shortModel } from '../domain/format';
import { resumeCommand } from '../domain/resume';
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
  account: Account | null;
  findOpen: boolean;
  phone: MediaQueryList;
  analyticsRange: number;
  find: string;
  show: { messages: boolean; tools: boolean; thinking: boolean };
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  allVisibility: { messages: boolean; tools: boolean; thinking: boolean };
  navigation: NavigationController;
  scope: EffectScope;
  sidebarOnly: boolean;
  clock: (t: number) => string;
  dialogs: Map<HTMLDialogElement, { destroy(): void }>;
  disposed: boolean;
  pendingSessionOpen: string | null;
  dur: (a: number, b: number | null | undefined) => string;
  afterPop: (() => void) | null;

  screenViews: Pick<ReturnType<typeof createScreenViews>, 'harnessSnapshot' | 'costSnapshot'>;

  applicationRefreshOwner: Pick<ReturnType<typeof createApplicationRefresh>, 'refresh'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  toolViewsOwner: Pick<ReturnType<typeof createToolViews>, 'viewerEl' | 'skipPop'>;

  liveUpdates: Pick<ReturnType<typeof createLiveUpdates>, 'tail'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'TOK' | 'fetchTx' | 'spread'>;

  transcripts: Pick<TranscriptStore, 'entries' | 'meta'>;

  pagingOwner: Pick<
    ReturnType<typeof createPaging>,
    'scrollProgrammatically' | 'resetPagerInput' | 'dropTx'
  >;

  viewport: Pick<
    ReturnType<typeof createViewport>,
    'scroller' | 'edge' | 'syncJump' | 'capture' | 'restore' | 'opener' | 'stopOpeningEndPin'
  >;

  accountControlsOwner: Pick<ReturnType<typeof createAccountControls>, 'shellChrome'>;

  analytics: Pick<ReturnType<typeof createAnalytics>, 'refreshAnalytics'>;

  historyScrollOwner: Pick<
    ReturnType<typeof createHistoryScroll>,
    'currentScroll' | 'restoreScroll' | 'saveHistoryScroll'
  >;

  modelStore: Pick<
    ViewerModelStore,
    'sessions' | 'turns' | 'machines' | 'machineUp' | 'machineLast' | 'turn'
  >;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  destination: Pick<ReturnType<typeof createDestination>, 'goSession' | 'goTrace'>;

  domain: Pick<
    ReturnType<typeof createDomain>,
    | 'countOf'
    | 'lineageOf'
    | 'hasTurn'
    | 'descendantsOf'
    | 'sessionChildren'
    | 'costForSessions'
    | 'costForSession'
    | 'hostOf'
    | 'costText'
    | 'onMachine'
    | 'movedOff'
    | 'branchOf'
  >;

  layoutOwner: Pick<
    ReturnType<typeof createLayout>,
    'wideMode' | 'setWideMode' | 'syncLayoutPrefs'
  >;
}
/** Owns sessionChrome behavior through explicit application ports. */
export function createSessionChrome(host: SessionChromeHost) {
  const kindText = (s: Session) => s.kind ?? HARNESS[s.harness] ?? s.harness;
  // The model the session is on (what the line's label abbreviates), so the label, its tip and the Details row name the same one; a session that used more than one is priced per model in the cost section.
  const modelIdOf = (s: Session) => s.model ?? Object.keys(s.tokens_by_model ?? {})[0];
  const viewerBar = createViewerBar();
  function renderTopbar(
    title: string,
    crumb: { label: string; go: () => void } | null = null,
    opts: TopbarOptions = {},
  ) {
    const s = opts.session ?? opts.traceSession;
    const account = host.account
      ? {
          account: host.account,
          compact: false,
          wide: host.layoutOwner.wideMode,
          onWideChange: () => host.layoutOwner.setWideMode(!host.layoutOwner.wideMode),
        }
      : null;
    const mode = s && errOn(s.id) ? 'errors' : s && host.findOpen ? 'find' : 'normal';
    const content = viewerBar.update(
      {
        mode,
        name: title,
        session: s?.id,
        state: s?.state,
        stateLabel: s ? STATE[s.state] : undefined,
        stateTip: s ? 'Status: ' + STATE[s.state] + ' · ' + turnsLabel(s) : undefined,
        showState: !!s && !opts.traceSession && !(host.phone.matches && opts.lineage?.length),
        ancestors: !host.phone.matches
          ? (opts.lineage?.map((a) => ({
              ...a,
              harnessName: HARNESS[a.harness] ?? a.harness,
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
        failed: s ? (host.domain.countOf(s, 'errors') ?? 0) : 0,
        signals: s ? signalCount(s) : 0,
        errorMode: ERR.mode,
        errorText: ERR.notice ?? errText(),
        errorDisabled: ERR.listed && !ERR.slots.length,
        icons: {
          search: I.search,
          back: I.back,
          x: I.x,
          up: I.up,
          more: I.more,
          dn: I.dn,
        },
      },
      {
        ancestor: (...args: Parameters<typeof host.destination.goSession>) =>
          host.destination.goSession(...args),
        crumb() {
          crumb?.go();
        },
        find() {
          host.findOpen = true;
          host.documentRendererOwner.render();
          host.$('#find')?.focus();
        },
        closeFind() {
          host.findOpen = false;
          host.find = '';
          host.show = { ...host.allVisibility };
          host.documentRendererOwner.render();
        },
        query(value) {
          host.find = value.toLowerCase();
          host.documentRendererOwner.render();
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
                : { ...host.allVisibility };
          host.documentRendererOwner.render();
        },
        menu(trigger, runs) {
          s &&
            openSessionMenu(
              host.modelStore.sessions[s.id] ?? s,
              trigger,
              runs ? '.runs' : undefined,
            );
        },
        errors() {
          s && openErrors(s.id);
        },
        closeErrors,
        step: stepErrors,
        range(days) {
          if (host.analyticsRange === days) return;
          const top = host.historyScrollOwner.currentScroll();
          host.analyticsRange = days;
          host.documentRendererOwner.render();
          host.historyScrollOwner.restoreScroll(top);
          host.analytics.refreshAnalytics(true);
        },
      },
    );
    const lead = {
      label: opts.traceSession ? 'Back to ' + s?.name : 'Open navigation',
      icon: opts.traceSession ? I.chev : I.menu,
      back: opts.traceSession
        ? () =>
            s &&
            host.destination.goSession(
              s.id,
              'turn' in host.navigation.route ? host.navigation.route.turn : undefined,
            )
        : undefined,
    };
    host.accountControlsOwner.shellChrome?.topbar(
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
    const s =
      host.modelStore.sessions['id' in host.navigation.route ? host.navigation.route.id : ''];
    if (host.navigation.route.v !== 'session' || !s) return;
    renderTopbar(s.name, null, {
      session: s,
      lineage: host.domain
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
    const sc = host.viewport.scroller(),
      r = (node.querySelector(':scope > button') ?? node).getBoundingClientRect(),
      bottom = host.phone.matches
        ? window.innerHeight
        : host.$('#main').getBoundingClientRect().bottom;
    const d = (r.top + r.bottom) / 2 - (host.viewport.edge() + bottom) / 2;
    host.pagingOwner.scrollProgrammatically(() => {
      if (Math.abs(d) >= 1) sc.scrollTop += d;
    });
    syncBarLine();
    host.viewport.syncJump();
    host.historyScrollOwner.saveHistoryScroll();
  }
  const errorNavigation = createErrorNavigation({
    navigation: host.navigation,
    TX: host.transcripts.entries,
    TXM: host.transcripts.meta,
    get TOK() {
      return host.transportOwner.TOK;
    },
    SESS: host.modelStore.sessions,
    get show() {
      return host.show;
    },
    sidebarOnly: host.sidebarOnly,
    page: () => host.$('#page'),
    drawSessionBar,
    render: (...args: Parameters<typeof host.documentRendererOwner.render>) =>
      host.documentRendererOwner.render(...args),
    keepFocus,
    countOf: (session, field) =>
      session ? (host.domain.countOf(session, field) ?? undefined) : undefined,
    capture: () => host.viewport.capture(),
    restore: (saved) => host.viewport.restore(saved),
    opener: (node) => host.viewport.opener(node),
    resetPagerInput: (...args: Parameters<typeof host.pagingOwner.resetPagerInput>) =>
      host.pagingOwner.resetPagerInput(...args),
    stopOpeningEndPin: () => host.viewport.stopOpeningEndPin(),
    clearFind() {
      host.find = '';
    },
    centre,
    fetchTx: (...args: Parameters<typeof host.transportOwner.fetchTx>) =>
      host.transportOwner.fetchTx(...args),
    dropTx: (...args: Parameters<typeof host.pagingOwner.dropTx>) =>
      host.pagingOwner.dropTx(...args),
    spread: (...args: Parameters<typeof host.transportOwner.spread>) =>
      host.transportOwner.spread(...args),
    tail: (sid) => host.liveUpdates.tail(sid),
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
    const n = (host.modelStore.turns[s.id] ?? []).filter(
      (...args: Parameters<typeof host.domain.hasTurn>) => host.domain.hasTurn(...args),
    ).length;
    return n + (n === 1 ? ' turn' : ' turns');
  };
  // On a phone the line of labels leaves the bar and the state is the small dot before the title. The dot names the state for a screen reader, and
  // its tip (a tap on a phone) adds the turn count. A desktop hides it, since the line shows the state there.
  // "Started 21:57 on <machine>" stays on one line: the machine name ellipsises (its tip, only while cut off, has the whole name).
  // A session's line: its state, then kind, model, failed steps, machine, branch and API-equivalent cost, each a plain label.
  const sessionLine = (s: Session): BarLabel[] => {
    const failed = host.domain.countOf(s, 'errors') ?? 0,
      runs = host.domain.descendantsOf(s.id, host.domain.sessionChildren()),
      cost = runs.length
        ? host.domain.costForSessions([s, ...runs])
        : host.domain.costForSession(s.id),
      labels: BarLabel[] = [];
    const add = (
      key: string,
      text: string,
      tip: string | undefined,
      drop: number,
      extra: Partial<BarLabel> = {},
    ) => labels.push({ key, text, tip: tip ?? undefined, drop, ...extra });
    add('state', STATE[s.state], undefined, 0, {
      className: 'state ' + s.state,
      state: s.state,
      stateLabel: STATE[s.state],
    });
    add('kind', kindText(s), s.kind ? s.kind + ' · ' + HARNESS[s.harness] : undefined, 2);
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
      host.modelStore.machines[s.machine],
      'Machine: ' + host.modelStore.machines[s.machine] + ' · ' + host.domain.hostOf(s),
      5,
    );
    if (s.branch) add('branch', s.branch, 'Branch: ' + s.branch, 6);
    add(
      'cost',
      host.domain.costText(cost),
      'API-equivalent cost' +
        (runs.length ? ', with ' + runs.length + (runs.length === 1 ? ' run' : ' runs') : '') +
        '. Details in the session menu.',
      7,
    );
    return labels;
  };
  const machineLine = (m: string): BarLabel[] => {
    const here = host.domain.onMachine(m),
      w = here.filter((s) => s.state === 'work').length,
      up = host.modelStore.machineUp[m];
    const sessions = here.length + (here.length === 1 ? ' session' : ' sessions');
    return [
      {
        key: 'state',
        text: up ? 'Up' : 'Not responding',
        drop: 0,
        className: 'state ' + (up ? 'done' : 'err'),
        state: up ? (w ? 'work' : 'idle') : 'err',
        stateLabel: STATE[up ? (w ? 'work' : 'idle') : 'err'],
      },
      {
        key: 'activity',
        text: up
          ? w + ' working · ' + sessions
          : host.domain.movedOff(m).length
            ? host.domain.movedOff(m).length + ' moved off'
            : [
                host.modelStore.machineLast[m] != null
                  ? 'Last seen ' + host.clock(host.modelStore.machineLast[m])
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
  if (!host.sidebarOnly) {
    host.scope.listen(window, 'scroll', syncBarLine, { passive: true });
    host.scope.listen(host.$('#main'), 'scroll', syncBarLine, { passive: true });
  }
  if (!host.sidebarOnly)
    host.scope.listen(
      window,
      'resize',
      () => {
        const l2 = host.$('#topbar .meta-line');
        if (l2 && host.navigation.route.v === 'session') measureViewerBar(host.$('#topbar'));
        host.layoutOwner.syncLayoutPrefs();
        host.viewport.syncJump();
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
          host.toolViewsOwner.viewerEl = chrome.dialog;
          document.documentElement.classList.add('panel-open');
          try {
            history.pushState(
              {
                ...host.navigation.route,
                sheet: 1,
                scrollTop: host.historyScrollOwner.currentScroll(),
              },
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
          if (host.toolViewsOwner.viewerEl === d) {
            host.toolViewsOwner.viewerEl = null;
            if (history.state?.sheet) {
              host.toolViewsOwner.skipPop = true;
              history.back();
            } else if (host.pendingSessionOpen) {
              const id = host.pendingSessionOpen;
              host.pendingSessionOpen = null;
              host.destination.goSession(id);
            }
          }
          if (host.liveModelOwner.LIVE.pending) host.applicationRefreshOwner.refresh();
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
    const kids = host.domain.descendantsOf(s.id, host.domain.sessionChildren());
    const {
      d,
      body,
      show: open,
    } = panel(s.name, {
      cls: 'anchored session-menu',
      label: 'Session menu for ' + s.name,
      sub: [STATE[s.state], kindText(s), shortModel(s.model)].join(' · '),
      onClose: () => {
        anchor?.setAttribute('aria-expanded', 'false');
        anchor?.focus({ focusVisible: false });
      },
    });
    const traceTurn =
      host.navigation.route.v === 'trace'
        ? host.modelStore.turn.get(
            ('turn' in host.navigation.route ? host.navigation.route.turn : undefined) ?? '',
          )
        : (() => {
            const top = host.$('#topbar').getBoundingClientRect().bottom;
            const nodes = [
              ...document.querySelectorAll<HTMLElement>('#page .turn[data-turn]'),
            ].filter((n) => !n.closest('.cw-body'));
            const visible = nodes.find((n) => n.getBoundingClientRect().bottom > top);
            return (
              host.modelStore.turn.get(visible?.dataset.turn ?? '') ??
              (host.modelStore.turns[s.id] ?? []).at(-1)
            );
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
    if (traceTurn?.out.length) addAction('trace', 'Trace this turn', I.trace, 'menu-trace');
    const command = resumeCommand(s);
    if (command !== null) addAction('copy', 'Copy resume command', I.copy);
    if (s.harness === 'claude') addAction('external', 'Open in claude.ai', I.ext);
    if (!host.phone.matches)
      addAction('wide', 'Wide transcript', I.wide, undefined, undefined, host.layoutOwner.wideMode);
    const signals = signalCount(s);
    if (signals)
      addAction('signals', signals + ' signals', undefined, 'menu-signals', 'Step through');
    const errorCount = host.domain.countOf(s, 'errors') ?? 0;
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
      if (kids.length) addAction('runs', 'Runs · ' + kids.length, I.stack, 'menu-runs');
    }
    const machine = host.modelStore.machines[s.machine] ?? s.machine ?? 'Unknown machine',
      calls = host.domain.countOf(s, 'calls');
    const detailRows: [string, string | number | undefined, boolean?, HarnessMark?][] = [
      ['Status', STATE[s.state] + ' · ' + turnsLabel(s)],
      ...(s.kind ? [['Kind', s.kind] as [string, string]] : []),
      [
        'Harness',
        HARNESS[s.harness] ?? s.harness,
        false,
        host.screenViews.harnessSnapshot(s.harness),
      ],
      ['Model', modelIdOf(s), true],
      ...(s.effort ? [['Effort', s.effort] as [string, string]] : []),
      [
        'Machine',
        machine +
          (host.domain.hostOf(s) !== machine ? ' · ' + host.domain.hostOf(s) : '') +
          (s.movedFrom
            ? ' (moved from ' + (host.modelStore.machines[s.movedFrom] ?? s.movedFrom) + ')'
            : ''),
      ],
      ['Directory', s.cwd ?? s.dir ?? s.directory, true],
      [s.worktree ? 'Worktree' : 'Branch', host.domain.branchOf(s), true],
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
        command: command ?? '',
        path: host.phone.matches
          ? host.domain.lineageOf(s.id).map((a) => ({
              id: a.id,
              name: a.name,
              harness: a.harness,
              harnessName: HARNESS[a.harness] ?? a.harness,
            }))
          : [],
        status: STATE[s.state] + ' · ' + turnsLabel(s),
        state: s.state,
        stateLabel: STATE[s.state],
        details,
        cost: host.screenViews.costSnapshot(s, kids),
        notice: TRADEMARK_NOTICE,
        icons: { chevron: I.chev, copy: I.copy },
      },
      {
        wide: () => host.layoutOwner.wideMode,
        session(id) {
          host.pendingSessionOpen = id;
          d.close();
        },
        action(key) {
          if (key === 'trace' && traceTurn) {
            host.afterPop = () => host.destination.goTrace(traceTurn.id);
            d.close();
          } else if (key === 'wide') host.layoutOwner.setWideMode(!host.layoutOwner.wideMode);
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

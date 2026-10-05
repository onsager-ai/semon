import { createLocalControl } from './localControl';
import { createRenderTransaction } from './renderTransaction';
import { ViewUpdates } from '../state/viewUpdates';
import { createPageRoot } from './pageRoot';
import { createRouteControls } from './routeControls';
import type { HostFocus } from '../navigation/routes';
import type { Account } from '../lib/account';
import { createDomain } from '../domain/calculations';
import { ago as formatAgo, clock as formatClock, dur as formatDuration } from '../domain/format';
import type { Entry, Session, TranscriptMeta } from '../domain/types';
import { releaseGeometry } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
import { NavigationController } from '../navigation/routes';
import { ViewerModelStore } from '../state/model';
import { TranscriptStore } from '../state/transcript';
import type { ViewerHost } from '../viewer-host';
import { createAccountControls } from './accountControls';
import { createAnalytics } from './analytics';
import { createApplicationRefresh } from './applicationRefresh';
import { createBootstrap } from './bootstrap';
import { createDestination } from './destination';
import { createDocumentEvents } from './documentEvents';
import { createDocumentRenderer } from './documentRenderer';
import { EffectScope } from './effects';
import { createHistoryScroll } from './historyScroll';
import { createLayout } from './layout';
import { createLiveModel } from './liveModel';
import { createLiveUpdates } from './liveUpdates';
import { createNavigationView } from './navigationView';
import { createOrderingControls } from './orderingControls';
import { createPaging } from './paging';
import { createRecentNavigation } from './recentNavigation';
import { HARNESS } from './registry';
import { createScreenViews } from './screenViews';
import { createSeenPersistence } from './seenPersistence';
import { createSeenResults } from './seenResults';
import { createSentences } from './sentences';
import { createSessionChrome } from './sessionChrome';
import { createSessionList } from './sessionList';
import { createTicker } from './ticker';
import { createToolLoader } from './toolLoader';
import { createToolViews } from './toolViews';
import { createTranscriptRevalidation } from './transcriptRevalidation';
import { createTranscriptView } from './transcriptView';
import { createTransport } from './transport';
import type { ViewerApplication } from './viewer';
import { createViewport } from './viewport';
/** The document composition owns services; focused factories own behavior and mutable feature state. */
export class ViewerComposition {
  readonly scope: EffectScope;
  readonly controlOwner: ReturnType<typeof createLocalControl>;
  readonly dialogs: Map<HTMLDialogElement, { destroy(): void }>;
  disposed: boolean;
  readonly application: ViewerApplication;
  now: number;
  readonly updates = new ViewUpdates();
  readonly modelStore: ViewerModelStore;
  admin: { href: string; label: string } | null;
  account: Account | null;
  readonly viewerHost: ViewerHost | null;
  readonly nativePage: { title: string; nav: string } | undefined;
  machinesPath: string | null;
  readonly sidebarOnly: boolean;
  readonly transcripts: TranscriptStore;
  readonly seenResultsOwner: ReturnType<typeof createSeenResults>;
  readonly domain: ReturnType<typeof createDomain>;
  readonly clock: (t: number) => string;
  readonly ago: (t: number) => string;
  readonly dur: (a: number, b: number | null | undefined) => string;
  readonly $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  readonly spaced: (t: unknown) => string;
  readonly darkTheme: () => boolean;
  readonly facetLine: (s: Session) => string;
  readonly seenPersistenceOwner: ReturnType<typeof createSeenPersistence>;
  readonly sentencesOwner: ReturnType<typeof createSentences>;
  readonly isGap: (e: Entry) => boolean;
  readonly transportOwner: ReturnType<typeof createTransport>;
  readonly cacheTx: (sid: string, entries: Entry[], meta: TranscriptMeta) => void;
  readonly adoptCached: (
    r: { v: 'session'; id: string; turn?: string | undefined } & {
      scrollTop?: number | undefined;
      hostFocus?: HostFocus | undefined;
      q?: string | undefined;
      sheet?: number | undefined;
    },
  ) => boolean;
  readonly transcriptRevalidationOwner: ReturnType<typeof createTranscriptRevalidation>;
  readonly pagingOwner: ReturnType<typeof createPaging>;
  readonly toolLoaderOwner: ReturnType<typeof createToolLoader>;
  readonly bootstrapOwner: ReturnType<typeof createBootstrap>;
  readonly phone: MediaQueryList;
  readonly navigation: NavigationController;
  readonly layoutOwner: ReturnType<typeof createLayout>;
  groupBy: string;
  query: string;
  focusSessionsSearchOnRender: ApplicationRoute | null;
  analyticsRange: number;
  analyticsMeasure: string;
  showApprovalReviews: boolean;
  readonly sessionFilters: { repo: string; machine: string; harness: string; model: string };
  pendingSessionOpen: string | null;
  accountSheet: boolean;
  afterPop: (() => void) | null;
  readonly allVisibility: { messages: boolean; tools: boolean; thinking: boolean };
  show: { messages: boolean; tools: boolean; thinking: boolean };
  find: string;
  findOpen: boolean;
  readonly historyScrollOwner: ReturnType<typeof createHistoryScroll>;
  readonly destination: ReturnType<typeof createDestination>;
  readonly accountControlsOwner: ReturnType<typeof createAccountControls>;
  readonly orderingControlsOwner: ReturnType<typeof createOrderingControls>;
  readonly navigationViewOwner: ReturnType<typeof createNavigationView>;
  readonly recentNavigation: ReturnType<typeof createRecentNavigation>;
  readonly sessionChrome: ReturnType<typeof createSessionChrome>;
  readonly screenViews: ReturnType<typeof createScreenViews>;
  readonly transcriptView: ReturnType<typeof createTranscriptView>;
  readonly toolViewsOwner: ReturnType<typeof createToolViews>;
  readonly pageRoot = createPageRoot();
  readonly routeControls: ReturnType<typeof createRouteControls>;
  readonly renderTransaction: ReturnType<typeof createRenderTransaction>;
  readonly documentRendererOwner: ReturnType<typeof createDocumentRenderer>;
  readonly analytics: ReturnType<typeof createAnalytics>;
  readonly sessionListOwner: ReturnType<typeof createSessionList>;
  readonly documentEventsOwner: ReturnType<typeof createDocumentEvents>;
  readonly liveModelOwner: ReturnType<typeof createLiveModel>;
  readonly liveUpdates: ReturnType<typeof createLiveUpdates>;
  readonly applicationRefreshOwner: ReturnType<typeof createApplicationRefresh>;
  readonly viewport: ReturnType<typeof createViewport>;
  readonly tickerOwner: ReturnType<typeof createTicker>;
  constructor(host: ViewerHost | null, onDestroyed: (owner: ViewerApplication) => void) {
    const context = this;
    this.scope = new EffectScope();
    this.dialogs = new Map<
      HTMLDialogElement,
      {
        destroy(): void;
      }
    >();
    this.disposed = false;
    this.application = {
      destroy() {
        if (context.disposed) return;
        context.disposed = true;
        context.controlOwner.destroy();
        context.updates.destroy();
        context.scope.destroy();
        context.liveModelOwner.liveController.destroy();
        context.navigation.destroy();
        context.transcripts.destroy();
        context.viewport.stopOpeningEndPin();
        context.pagingOwner.pagerController.disconnect();
        context.sessionChrome.errorNavigation.destroy();
        context.viewport.scrollController.destroy();
        for (const dialog of context.dialogs.values()) dialog.destroy();
        context.dialogs.clear();
        context.routeControls.destroy();
        if (!context.sidebarOnly) context.pageRoot.destroy(context.$('#page'));
        context.recentNavigation.recentRenderer.destroy();
        context.sessionChrome.viewerBar.destroy();
        context.accountControlsOwner.shellChrome?.destroy();
        context.accountControlsOwner.accountChrome.destroy();
        releaseGeometry(context.layoutOwner.app);
        releaseGeometry(document.documentElement, ['barHeight']);
        document.querySelectorAll('.livenote, .livenote-side').forEach((node) => node.remove());
        document.documentElement.classList.remove('viewer-open', 'panel-open');
        onDestroyed(context.application);
      },
    };
    this.now = Date.now();
    this.modelStore = new ViewerModelStore(this.updates);
    this.admin = null;
    this.account = null;
    this.viewerHost = host;
    this.nativePage = context.viewerHost?.nativePage;
    this.machinesPath = context.viewerHost?.machinesPath ?? null;
    this.sidebarOnly = document.querySelector<HTMLElement>('.app')?.dataset.viewer === 'sidebar';
    this.transcripts = new TranscriptStore(
      {
        request: (path, signal) => context.transportOwner.api(path, signal),
        turns: (sid) => context.modelStore.turns[sid] ?? [],
        turn: (id) => context.modelStore.turn.get(id),
        entry: (e) => context.transportOwner.txEntry(e),
        cleared(sid) {
          if (
            context.navigation.route.v === 'session' &&
            ('id' in context.navigation.route ? context.navigation.route.id : '') === sid
          )
            context.pagingOwner.resetPagerInput();
        },
      },
      this.updates,
    );
    this.seenResultsOwner = createSeenResults({});
    this.domain = createDomain(
      {
        sessions: context.modelStore.sessions,
        machines: context.modelStore.machines,
        handoffs: context.modelStore.handoffs,
        turns: context.modelStore.turns,
        turn: context.modelStore.turn,
        starts: context.modelStore.starts,
        holds: context.modelStore.holds,
        handoff: context.modelStore.handoff,
        transcriptMeta: context.transcripts.meta,
      },
      () => context.now,
      context.seenResultsOwner.SEEN_RESULTS,
    );
    this.updates.subscribe(() => this.domain.invalidate());
    this.clock = (t: number) => formatClock(t, context.now);
    this.ago = (t: number) => formatAgo(t, context.now);
    this.dur = (a: number, b: number | null | undefined) =>
      formatDuration(a, b ?? undefined, context.now);
    this.$ = <T extends HTMLElement = HTMLElement>(s: string, r: ParentNode = document) =>
      r.querySelector<T>(s)!;
    this.spaced = (t: unknown) =>
      String(t).replace(/ · /g, '\u2009 · \u2009').replace(/^· /, '·\u2009 ');
    this.darkTheme = () => {
      const t = document.documentElement.getAttribute('data-theme');
      return (
        t === 'dark' ||
        (t !== 'light' && !!window.matchMedia?.('(prefers-color-scheme: dark)').matches)
      );
    };
    this.facetLine = (s: Session) =>
      [
        s.kind ?? HARNESS[s.harness],
        context.modelStore.machines[s.machine],
        context.domain.where(s),
      ].join(' · ');
    this.seenPersistenceOwner = createSeenPersistence(context);
    this.sentencesOwner = createSentences(context);
    this.isGap = (e: Entry) =>
      e.k === 'end' && /entries (not included|omitted)|^No activity/.test(e.text ?? '');
    this.controlOwner = createLocalControl(this.scope, () =>
      context.applicationRefreshOwner.refresh(),
    );
    this.transportOwner = createTransport(context);
    this.cacheTx = (sid: string, entries: Entry[], meta: TranscriptMeta) =>
      context.transcripts.keep(sid, entries, meta, !!context.domain.originHandoff(sid));
    this.adoptCached = (
      r: Extract<
        ApplicationRoute,
        {
          v: 'session';
        }
      >,
    ) => context.transcripts.adoptCached(r.id, r.turn);
    this.transcriptRevalidationOwner = createTranscriptRevalidation(context);
    this.pagingOwner = createPaging(context);
    this.toolLoaderOwner = createToolLoader(context);
    this.bootstrapOwner = createBootstrap(context);
    this.phone = window.matchMedia('(max-width: 760px)');
    this.navigation = new NavigationController(
      {
        model: context.bootstrapOwner.routeModel,
        loadMachines: host ? (signal) => host.loadMachines(signal) : undefined,
      },
      context.viewerHost?.initialMachines ?? null,
    );
    this.routeControls = createRouteControls(this.navigation);
    this.layoutOwner = createLayout(context);
    this.groupBy = 'recent';
    this.query = '';
    this.focusSessionsSearchOnRender = null;
    this.analyticsRange = 7;
    this.analyticsMeasure = 'hours';
    this.showApprovalReviews = false;
    this.sessionFilters = { repo: '', machine: '', harness: '', model: '' };
    this.pendingSessionOpen = null;
    this.accountSheet = false;
    this.afterPop = null;
    if (!context.sidebarOnly)
      try {
        history.scrollRestoration = 'manual';
      } catch {}
    this.allVisibility = { messages: true, tools: true, thinking: true };
    this.show = { ...context.allVisibility };
    this.find = '';
    this.findOpen = false;
    this.historyScrollOwner = createHistoryScroll(context);
    this.destination = createDestination(context);
    this.accountControlsOwner = createAccountControls(context);
    this.orderingControlsOwner = createOrderingControls(context);
    this.navigationViewOwner = createNavigationView(context);
    this.recentNavigation = createRecentNavigation(context);
    this.sessionChrome = createSessionChrome(context);
    this.screenViews = createScreenViews(context);
    this.transcriptView = createTranscriptView(context);
    this.toolViewsOwner = createToolViews(context);
    this.renderTransaction = createRenderTransaction(context);
    this.documentRendererOwner = createDocumentRenderer(context);
    this.analytics = createAnalytics(context);
    this.sessionListOwner = createSessionList(context);
    this.documentEventsOwner = createDocumentEvents(context);
    this.liveModelOwner = createLiveModel(context);
    this.liveUpdates = createLiveUpdates(context);
    this.applicationRefreshOwner = createApplicationRefresh(context);
    this.viewport = createViewport(context);
    this.tickerOwner = createTicker(context);
    // An embedding page's sidebar: the row its data-viewer-nav names (home, sessions or machines) is current.
    if (context.sidebarOnly) {
      const nav = context.layoutOwner.app.dataset.viewerNav;
      context.navigation.route = context.navigation.historyRoute({ v: nav }, { v: 'home' });
    }
    if (context.viewerHost) {
      this.account = context.transportOwner.accountOf(context.viewerHost.account);
      if (context.nativePage)
        context.navigation.route = context.navigation.historyRoute(
          { v: context.nativePage.nav },
          { v: 'home' },
        );
      else if (context.navigation.content) context.navigation.route = { v: 'machines' };
      if (context.nativePage || context.navigation.content) context.documentRendererOwner.render();
    }
    context.bootstrapOwner.boot();
  }
}

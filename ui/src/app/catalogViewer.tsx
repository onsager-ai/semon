import {
  sessionPath,
  tracePath,
  parseSessionRoute,
  canonicalSessionHash,
  sessionHashTurn,
  canonicalSessionRoute,
  resolveSessionBookmark,
  type SessionRoute,
} from '../lib/sessionRoute';
import { compareCatalogKeys } from '../lib/catalogIdentity';
import { readShellPreference, writeShellPreference } from '../lib/navigation';
import {
  parseCatalogSourceCandidates,
  parseCatalogSourceCandidateProgress,
  type CatalogSourceCandidate,
} from '../state/catalog-source-candidates';
import { render } from 'preact';
import { createOverviewPages } from './overviewPages';
import { parseCausalTrace, renderCausalTrace, type CausalTrace } from './tracePresentation';
import { reachableResultIds } from '../domain/trace';
import type { ControlView } from '../lib/control';
import type { ViewerApplication } from './viewer';
import { object, text } from '../domain/validate';
import type { ViewerHost, ViewerPage } from '../viewer-host';
import { parseCatalogCapabilities, type CatalogCapabilities } from '../state/catalog-capabilities';
import {
  catalogSessionKey,
  parseCatalogPage,
  type CatalogPage,
  type CatalogSession,
} from '../state/catalog-wire';
import {
  catalogCountWitness,
  completeCatalogCount,
  type CatalogCountWitness,
} from '../state/catalog-count';
import { CatalogSelectionStore, type CatalogSelectionRequest } from '../state/catalog-selection';
import { parseCatalogAnnotations, type CatalogAnnotations } from '../state/catalog-annotations';
import { CatalogTranscriptStore } from '../state/catalog-transcript';
import { CatalogRecovery, type CatalogRecoveryProgress } from '../state/catalog-recovery';
import { createShellChrome } from '../lib/shell';
import { createOrdering, orderRows, type OrderScope } from '../lib/ordering';
import { createRecentRenderer } from '../lib/recent';
import {
  catalogFilterDescription,
  renderCatalogList,
  type CatalogObservationState,
} from '../lib/catalog';
import { createCatalogBrowser, type CatalogFilters } from './catalogBrowser';
import {
  renderSessionScreen,
  closeSessionComposer,
  updateSessionControl,
  updateSessionClock,
  updateSessionName,
  updateSessionPager,
  updateSessionRuntime,
  updateSessionJump,
} from '../lib/transcript';
import { releaseScreen, Glyph, screenText } from '../lib/screens';
import { requestJson } from '../lib/api';
import { parseAccount } from '../lib/account';
import {
  setGeometry,
  releaseGeometry,
  projectShellNavigation,
  createViewerBar,
  measureViewerBar,
  createPanelChrome,
  renderSessionMenu,
  FullTool,
  createImageViewer,
  measureSessionScreen,
  parseShellNavigation,
  safePath,
  type ShellNavigation,
} from '../lib';
import {
  selectedSessionLabels,
  selectedSessionMenu,
  selectedSessionState,
  selectedSessionStatusTip,
  selectedSessionFooter,
  catalogSessionRow,
  catalogSessionGroups,
} from './sessionPresentation';
import { NavigationController } from '../navigation/routes';
import { createRuntimeObservation } from './runtimeObservation';
import { EffectScope } from './effects';
import {
  catalogTranscriptBlocks,
  recordedRelationshipViews,
  recordedSignalLabel,
} from './catalogTranscriptView';
import { parseNativeRelationships } from '../domain/relationships';
import {
  parseCatalogTranscriptPage,
  type CatalogTranscriptEntry,
} from '../state/catalog-transcript-wire';
import { parseCatalogField } from '../state/catalog-field-wire';
import { I, STATE, HARNESS } from './registry';
import { toolInfo } from '../domain/toolNames';
import type { ToolData } from '../lib/tool-details';
import { clock } from '../domain/format';
import { parseCatalogSources, type CatalogSourceItem } from '../state/catalog-sources';
import { renderCatalogSources } from '../lib/catalogSources';
interface SelectedView {
  sourceKey: string;
  key: string;
  root: HTMLElement;
  store: CatalogTranscriptStore;
  meta: CatalogSession | null;
  observedName?: string;
  sourceLabel?: string | null;
  scroll: number;
  loading: boolean;
  observing?: boolean;
  note: string;
  transientNote?: boolean;
  observationFailure?: { text: string; retryable: boolean };
  anchorNote?: string;
  ticket?: CatalogSelectionRequest;
  identityLoading: boolean;
  opening: boolean;
  retry?: number;
  recovery: CatalogRecovery;
  renderedRevision: number;
  renderedNote: string;
  renderedFooter: string;
  fields: Map<
    string,
    {
      generation: string;
      text?: string;
      next: number | null;
      loading: boolean;
      note: string;
      promise?: Promise<void>;
    }
  >;
  following: boolean;
  jumpVisible: boolean;
  jumpGeneration: string | null;
  newCount: number;
  lastTotal: number | null;
  queuedAfter?: { after: number | null; preservePosition: boolean };
  contextLoading?: boolean;
  contextText?: Map<
    string,
    {
      brief?: string;
      result?: string;
      answer?: string | string[];
      activity?: readonly [string, string];
    }
  >;
  find?: string;
  findOpen?: boolean;
  filter?: string;
  errorMode?: 'errors' | 'signals';
  errorIndex?: number;
  annotations?: CatalogAnnotations;
  annotationsLoading?: boolean;
  annotationRange?: number;
  navigationEpoch?: number;
  annotationsEpoch?: number;
  annotationsFocus?: boolean;
  errorRestore?: {
    scroll: number;
    first: number;
    key?: string;
    ranges: { first: number; end: number }[];
    expanded: Set<string>;
    findOpen: boolean;
    find?: string;
    filter?: string;
    focus: string;
  };
}

function catalogRecoveryProgress(value: CatalogSession): CatalogRecoveryProgress {
  return {
    source: JSON.stringify(
      value.source_refs.map(({ state, source }) => [
        state,
        source.offset,
        source.prefix_sha256,
        source.tail_sha256,
      ]),
    ),
    catalog: JSON.stringify(
      value.transcript
        ? [value.transcript.generation, value.transcript.errorsGeneration, value.transcript.total]
        : null,
    ),
  };
}

/** The advertised bounded reader has no complete ModelStore or global content poller. */
export function createCatalogViewer(
  initialCapabilities: CatalogCapabilities | null,
  viewerHost: ViewerHost | null,
): ViewerApplication {
  const capabilities: CatalogCapabilities = initialCapabilities ?? {
    api: 1,
    read_contract: 'catalog-v1',
    source_key: '',
    selected_transcript: false,
    selected_identity: false,
    selected_entry: false,
    source_candidates: false,
    metadata_search: false,
    attachment: false,
    relationship_context: false,
    large_native_records: false,
    pagination: true,
    filters: [],
    order: 'last_desc_key_asc',
    full_text_search: false,
    global_union: false,
  };
  const scope = new EffectScope(),
    selection = new CatalogSelectionStore(),
    navigation = new NavigationController((route) => {
      // Navigation handlers push their destination URL synchronously. Publish
      // its observed route afterward, preserving the preceding history entry.
      queueMicrotask(() => {
        if (!disposed && navigation.route === route)
          history.replaceState({ ...history.state, ...route }, '', location.href);
      });
    });
  const previousScrollRestoration = history.scrollRestoration;
  if (!viewerHost?.nativePage) history.scrollRestoration = 'manual';
  let hostPage: ViewerPage | null = null;
  let hostRoute: { title: string; nav: string } | null = null;
  const hostScroll = new Map<string, { main: number; window: number }>();
  let hostRequest: AbortController | null = null;
  let hostEpoch = 0;
  let initialHostPage = viewerHost?.pages?.initial;
  const initialHostUrl = location.href;
  let nameRefreshOffset = 0;
  let listRetry: number | undefined,
    listRetryDelay = 1000;
  let disposed = false,
    page: CatalogPage | null = null,
    countWitness: CatalogCountWitness | null = null,
    items: CatalogSession[] = [],
    cursor: string | null = null,
    listEpoch = 0,
    updating = false,
    note = '',
    harness = new URLSearchParams(location.search).get('harness') ?? '',
    repo =
      new URLSearchParams(location.search).get('no_repo') === '1'
        ? '__none__'
        : (new URLSearchParams(location.search).get('repo') ?? ''),
    model = new URLSearchParams(location.search).get('model') ?? '',
    query = new URLSearchParams(location.search).get('q') ?? '',
    listScroll = 0,
    listFocus = '',
    active: SelectedView | null = null;
  let candidates: CatalogSourceCandidate[] = [],
    candidateCursor: string | null = null,
    candidateLoaded = false,
    candidateUpdating = false,
    candidateNote = '',
    candidateEpoch = 0,
    candidateTimer: number | undefined;
  const selected = new Map<string, SelectedView>();
  const sourceViews = new Map<
    string,
    {
      page: CatalogPage | null;
      countWitness: CatalogCountWitness | null;
      items: CatalogSession[];
      cursor: string | null;
      harness: string;
      repo: string;
      model: string;
      query: string;
      listScroll: number;
      listFocus: string;
      selectedKey: string | null;
    }
  >();
  let sourcesOpen = false,
    sourcesEpoch = 0,
    sourceItems: CatalogSourceItem[] = [],
    sourceCursor: string | null = null,
    sourcesUpdating = false,
    sourcesNote = '',
    sourcesRetryDelay = 1000,
    locating = false;
  let bookmarkMatches: { source_key: string; catalog_key: string; harness: string }[] = [];
  let pinView: SelectedView | null = null,
    pinUntil = 0;
  const cacheKey = (key: string) => JSON.stringify([capabilities.source_key, key]);
  let showApprovalReviews = false;
  const listExpanded = new Set<string>();
  const ordering = createOrdering<object>();
  let orderTouch = false,
    orderIdle: number | undefined;
  const idleHook = Reflect.get(window, '__semonOrderIdleMs');
  const orderIdleMs =
    Number.isFinite(idleHook) && idleHook >= 200 && idleHook <= 60000 ? idleHook : 10000;
  function orderScope(name: string, signature = ''): OrderScope<object> {
    const side = name === 'recent',
      region = document.querySelector<HTMLElement>(side ? '#side-list' : '#main');
    const rows = side ? '#lanes .treeitem' : '#page .nrow';
    const first = document.querySelector<HTMLElement>(rows),
      focus = document.activeElement;
    const inView = side
      ? (region?.scrollTop ?? 0) <= 1
      : !first ||
        first.getBoundingClientRect().top >=
          (document.querySelector('#topbar')?.getBoundingClientRect().bottom ?? 0) - 1;
    const touched =
      orderTouch ||
      !!(focus?.matches(':focus-visible') && focus.closest(side ? '#lanes' : rows)) ||
      (matchMedia('(hover: hover)').matches &&
        !!document.querySelector(side ? '#lanes .treeitem:hover' : '#page .nrow:hover'));
    if (rail && side) ordering.scopes.delete(name);
    return ordering.begin(name, signature, side ? null : navigation.route, { inView, touched });
  }
  function armOrderIdle() {
    scope.clearTimeout(orderIdle);
    orderIdle = undefined;
    if (matchMedia('(max-width: 760px)').matches || !ordering.scopes.get('recent')?.n) return;
    orderIdle = scope.timeout(() => {
      orderIdle = undefined;
      const sidebar = document.querySelector('#sidebar'),
        focused = document.activeElement;
      if (
        orderTouch ||
        sidebar?.matches(':hover') ||
        sessionPanel ||
        shell.account.open ||
        (focused &&
          sidebar?.contains(focused) &&
          focused.matches(':focus-visible, input, textarea, select, [contenteditable]'))
      )
        armOrderIdle();
      else {
        ordering.scopes.delete('recent');
        chrome();
      }
    }, orderIdleMs);
  }
  let account = parseAccount(viewerHost?.account);
  let standaloneNavigation: ShellNavigation | undefined;
  let standaloneAdmin: { label: string; href: string } | null = null;
  let restoringOverlayHistory = false,
    overlayBackPending = false,
    pendingChrome = false;
  const machinesHref = () =>
    viewerHost?.machinesPath ?? standaloneNavigation?.paths?.machines ?? '/machines';
  let wide = readShellPreference('wide'),
    rail = readShellPreference('rail');
  const shell = createShellChrome({
    account: {
      place(widget, trigger) {
        const at = trigger.getBoundingClientRect();
        setGeometry(widget, 'accountLeft', at.left);
        setGeometry(widget, 'accountWidth', at.width);
        setGeometry(widget, 'accountBottom', Math.max(0, innerHeight - at.top + 6));
      },
      opened(compact) {
        if (compact) history.pushState({ ...history.state, sheet: 1 }, '');
      },
      closed(options) {
        if (pendingChrome && !options.navigating && !disposed) {
          pendingChrome = false;
          scope.frame(chrome);
        }
        if (
          !disposed &&
          !options.keepEntry &&
          !options.navigating &&
          !restoringOverlayHistory &&
          history.state?.sheet
        ) {
          overlayBackPending = true;
          history.back();
        }
      },
      navigate(href) {
        if (!history.state?.sheet) return false;
        const destination = applicationURL(href);
        if (destination) {
          navigateApplication(destination.pathname + destination.search + destination.hash);
          return true;
        }
        const { sheet, ...route } = history.state;
        history.replaceState(route, '');
        location.assign(href);
        return true;
      },
      submit(form) {
        if (!history.state?.sheet) return false;
        const { sheet, ...route } = history.state;
        history.replaceState(route, '');
        form.submit();
        return true;
      },
    },
    navigate(destination) {
      if (viewerHost?.nativePage) return false;
      if (viewerHost?.pages?.match(new URL(destination.href, location.href))) {
        navigateApplication(destination.href);
        return true;
      }
      if (destination.key === 'analytics') {
        goOverview(destination.key);
        return true;
      }
      if (
        destination.key === 'machines' &&
        !viewerHost?.catalogSources &&
        machinesHref() === '/machines'
      ) {
        goOverview('machines');
        return true;
      }
      if (destination.key !== 'sessions') return false;
      if (capabilities.source_key || capabilities.global_union) goList();
      else {
        history.pushState(null, '', '/sessions');
        fromLocation();
      }
      return true;
    },
    drawerOpened() {
      ordering.scopes.delete('recent');
      chrome();
      history.pushState({ ...history.state, drawer: 1 }, '');
    },
    drawerClosed() {
      ordering.scopes.delete('recent');
      chrome();
      if (!disposed && !restoringOverlayHistory && history.state?.drawer && !history.state?.sheet) {
        overlayBackPending = true;
        history.back();
      }
    },
    railChanged() {
      ordering.scopes.delete('recent');
      rail = !rail;
      writeShellPreference('rail', rail);
      chrome();
    },
  });
  shell.mount(document.querySelector<HTMLElement>('.app')!);
  scope.listen(
    document,
    'pointerdown',
    () => {
      orderTouch = true;
    },
    true,
  );
  for (const type of ['pointerup', 'pointercancel'])
    scope.listen(
      document,
      type,
      () => {
        orderTouch = false;
      },
      true,
    );
  scope.listen(window, 'blur', () => {
    orderTouch = false;
  });
  scope.listen(document, 'visibilitychange', () => {
    orderTouch = false;
    if (document.visibilityState === 'visible') {
      ordering.scopes.clear();
      chrome();
      drawList();
      overview.draw();
    }
  });
  for (const type of [
    'pointermove',
    'pointerdown',
    'pointerleave',
    'focusin',
    'focusout',
    'wheel',
    'keydown',
  ])
    scope.listen(
      document.querySelector<HTMLElement>('#sidebar')!,
      type,
      () => {
        if (orderIdle) armOrderIdle();
      },
      { passive: true },
    );
  const root = shell.slots.content,
    title = document.createElement('div'),
    back = document.createElement('button'),
    jumpActions = document.createElement('div'),
    jumpSlot = document.createElement('span'),
    jumpTarget = document.createElement('div');
  const restoreReaderLayout = () => {
    // Annotation navigation owns its target across paged/group layout commits,
    // just as opening-end navigation owns the tail across those same commits.
    if (active?.errorMode) {
      const view = active;
      scope.frame(() => highlightAnnotation(view));
      return;
    }
    if (
      active &&
      active === pinView &&
      active.following &&
      !active.errorMode &&
      performance.now() < pinUntil
    ) {
      active.following = true;
      scrollEnd();
    }
  };
  const readerResize = new ResizeObserver(restoreReaderLayout);
  // On phones the document grows with its content: changing only the viewport
  // height need not resize any observed content box. Recenter against the new
  // viewport as well as against transcript and toolbar layout changes.
  scope.listen(window, 'resize', restoreReaderLayout);
  if (window.visualViewport) scope.listen(window.visualViewport, 'resize', restoreReaderLayout);
  readerResize.observe(root);
  readerResize.observe(document.querySelector<HTMLElement>('#main')!);
  readerResize.observe(document.querySelector<HTMLElement>('#topbar')!);
  scope.listen(
    document,
    'wheel',
    () => {
      pinUntil = 0;
    },
    { passive: true, capture: true },
  );
  // Keyboard activation and native control clicks also end the opening hold.
  // Their expansion/navigation owns the resulting reading position.
  scope.listen(
    document,
    'click',
    () => {
      pinUntil = 0;
    },
    { capture: true },
  );
  scope.listen(
    document,
    'pointerdown',
    () => {
      pinUntil = 0;
    },
    { passive: true, capture: true },
  );
  const sessionBar = createViewerBar();
  const traceActions = document.createElement('div'),
    traceMenu = document.createElement('button');
  traceActions.className = 'viewer-bar-actions';
  traceMenu.type = 'button';
  traceMenu.id = 'more-btn';
  traceMenu.className = 'ibtn';
  traceMenu.setAttribute('aria-label', 'Session menu');
  traceMenu.setAttribute('aria-haspopup', 'dialog');
  traceMenu.setAttribute('aria-expanded', 'false');
  render(<Glyph path={I.more} className="" />, traceMenu);
  traceActions.append(traceMenu);
  scope.listen(traceMenu, 'click', () => openSessionMenu(null, traceMenu));
  let sessionPanel: { destroy(): void } | null = null;
  function panelOpened(panel: { destroy(): void }) {
    sessionPanel?.destroy();
    sessionPanel = panel;
    history.pushState({ ...navigation.route, sheet: 1 }, '');
  }
  function panelClosed(panel: { destroy(): void }) {
    if (sessionPanel === panel) sessionPanel = null;
    if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
      overlayBackPending = true;
      history.back();
    }
  }
  let afterOverlayClose: (() => void) | null = null;
  function navigateFromPanel(panel: { destroy(): void }, action: () => void) {
    if (history.state?.sheet) {
      afterOverlayClose = action;
      panel.destroy();
    } else {
      panel.destroy();
      action();
    }
  }
  let traceTitle = false,
    machineTitle = false;
  title.className = 'ttl catalog-title';
  back.type = 'button';
  back.className = 'ibtn catalog-back';
  back.setAttribute('aria-label', 'Back to sessions');
  back.setAttribute('data-tip', 'Back to sessions');
  render(<Glyph path={I.back} className="" />, back);
  const sources = document.createElement('a');
  sources.className = 'ibtn catalog-sources';
  sources.href = '/sessions?choose_source=1';
  sources.setAttribute('aria-label', 'Choose a machine');
  sources.setAttribute('data-tip', 'Choose a machine');
  render(<Glyph path={I.machine} className="" />, sources);
  scope.listen(sources, 'click', (event) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
      return;
    event.preventDefault();
    history.pushState(null, '', '/sessions?choose_source=1');
    void showSources();
  });
  scope.listen(back, 'click', () => goList());
  jumpActions.className = 'viewer-bar-actions';
  jumpSlot.className = 'viewer-jump';
  jumpTarget.className = 'jump-wrap';
  jumpTarget.hidden = true;
  jumpSlot.append(jumpTarget);
  jumpActions.append(jumpSlot);
  const recent = createRecentRenderer(shell.slots.recent.querySelector<HTMLElement>('#lanes')!, {
    open(key) {
      const item = items.find(
        (row) => JSON.stringify([row.machine?.key ?? capabilities.source_key, row.key]) === key,
      );
      if (item?.machine) {
        openCatalogSession(item);
        return;
      }
      if (overview.openRecent(key)) return;
      if (item) void goSession(item.key, item);
      else {
        const identity: unknown = JSON.parse(key);
        if (
          Array.isArray(identity) &&
          identity.length === 2 &&
          identity[0] === capabilities.source_key &&
          typeof identity[1] === 'string'
        )
          void goSession(identity[1]);
      }
    },
    toggle(key, value) {
      overview.toggleRecent(key, value);
    },
    all(key, trigger) {
      overview.allRecent(key, true, trigger);
    },
    fewer(key) {
      overview.allRecent(key, false);
    },
  });
  // Collection observes native history; optional controllers never enter this reader.
  function controlFor(_view: SelectedView): ControlView | undefined {
    return undefined;
  }
  const runtimeObservation = createRuntimeObservation({
    scope,
    navigation,
    viewerHost,
    catalogRuntimeSelection: selection,
  });
  function runtimeFor(view: SelectedView) {
    const frame = runtimeObservation.view();
    return frame?.source_key === capabilities.source_key && frame.catalog_key === view.key
      ? frame
      : undefined;
  }
  const removeRuntimeListener = runtimeObservation.subscribe(() => {
    if (!active) return;
    updateSessionRuntime(active.root, runtimeFor(active));
    updateSessionControl(active.root, controlFor(active));
  });
  const overviewRequests = new Set<AbortController>();
  let causal: CausalTrace | null = null,
    causalEpoch = 0,
    causalLoading = false;
  const causalExpanded = new Map<string, Set<string>>();
  const api = async (path: string, body?: unknown): Promise<unknown> => {
    const controller = scope.request(),
      deadline = scope.timeout(() => controller.abort(), 15000);
    if (
      path === '/api/viewer/home' ||
      path.startsWith('/api/viewer/home?') ||
      path.startsWith('/api/viewer/analytics?') ||
      path.startsWith('/api/session-home?') ||
      path.startsWith('/api/session-observe?') ||
      path.startsWith('/api/session-trace?')
    )
      overviewRequests.add(controller);
    try {
      return await requestJson(
        path,
        controller.signal,
        path.startsWith('/api/viewer/analytics?'),
        body,
      );
    } finally {
      scope.clearTimeout(deadline);
      scope.releaseRequest(controller);
      overviewRequests.delete(controller);
    }
  };
  const currentParams = () =>
    new URLSearchParams(capabilities.source_key ? { machine: capabilities.source_key } : {});
  const sourceLabel = (key: string) => {
    if (!key && capabilities.global_union) return 'All machines';
    const identity = selection.selectedIdentity();
    return (
      (active?.sourceKey === key ? active.sourceLabel : null) ??
      (identity?.source_key === key ? identity.machine_label : null) ??
      sourceItems.find((source) => source.source_key === key)?.label ??
      (page?.machine === key ? page.machine_info.label : null) ??
      key
    );
  };
  const browsingParams = () => {
    const value = currentParams();
    if (harness) value.set('harness', harness);
    if (repo === '__none__') value.set('no_repo', '1');
    else if (repo) value.set('repo', repo);
    if (model) value.set('model', model);
    if (query) value.set('q', query);
    if (active && new URLSearchParams(location.search).get('all_machines') === '1')
      value.set('all_machines', '1');
    return value;
  };
  const sessionControls = () => {
    const p = browsingParams();
    p.delete('machine');
    return p;
  };
  const sessionAnchor = () => {
    return sessionHashTurn(location.hash);
  };
  const saveFilters = () =>
    history.replaceState(history.state, '', '/sessions?' + browsingParams());
  const countFilters = () => JSON.stringify([harness, repo, model, query]);
  let groupBy = 'recent';
  function applyListFilters(values: CatalogFilters) {
    if (
      (values.machine === undefined || values.machine === capabilities.source_key) &&
      harness === values.harness &&
      repo === values.repo &&
      model === values.model &&
      query === values.q
    )
      return;
    cancelCandidate();
    ({ harness, repo, model, q: query } = values);
    page = null;
    countWitness = null;
    items = [];
    cursor = null;
    if (
      viewerHost?.catalogSources &&
      values.machine !== undefined &&
      values.machine !== capabilities.source_key
    ) {
      const next = browsingParams();
      if (values.machine) next.set('machine', values.machine);
      else next.delete('machine');
      history.pushState({ v: 'sessions' }, '', '/sessions?' + next);
      fromLocation();
      return;
    }
    saveFilters();
    void loadList(false);
  }
  function clearListFilters() {
    cancelCandidate();
    if (viewerHost?.catalogSources && capabilities.source_key) {
      applyListFilters({ machine: '', harness: '', repo: '', model: '', q: '' });
      return;
    }
    harness = repo = model = query = '';
    page = null;
    countWitness = null;
    items = [];
    cursor = null;
    saveFilters();
    void loadList(false);
    root.querySelector<HTMLInputElement>('#sq')?.focus();
  }
  const catalogBrowser = createCatalogBrowser({
    canOpen: () => !disposed && navigation.route.v === 'sessions' && !active && !sourcesOpen,
    opened: panelOpened,
    closed: panelClosed,
    changed(values) {
      const source = capabilities.source_key;
      const commit = () => {
        if (
          !disposed &&
          navigation.route.v === 'sessions' &&
          !active &&
          !sourcesOpen &&
          capabilities.source_key === source
        )
          applyListFilters(values);
      };
      // Closing shared Panel chrome pops its overlay history entry. Commit the
      // native filter URL only after that transaction, so Back cannot erase it.
      if (overlayBackPending) afterOverlayClose = commit;
      else commit();
    },
    clearFilters: clearListFilters,
  });
  const params = () => {
    const value = currentParams();
    value.set('scope', 'retained_history');
    return value;
  };
  const overview = createOverviewPages({
    effects: scope,
    orderScope,
    reviews: () => showApprovalReviews,
    root,
    homeIntro: viewerHost?.initialHome?.element,
    nativeContent: !!viewerHost?.nativePage,
    enabled: viewerHost?.workspaceMetadata !== false,
    api,
    panelOpened,
    panelClosed,
    route: () => (viewerHost?.nativePage ? 'home' : navigation.route.v),
    navigationRoute: () => navigation.route,
    selectedMachine: () => (navigation.route.v === 'machine' ? navigation.route.id : null),
    admin: () => standaloneAdmin,
    machine(key) {
      history.pushState(
        null,
        '',
        '/machines/' + encodeURIComponent(key) + '?' + new URLSearchParams({ machine: key }),
      );
      fromLocation();
    },
    chrome,
    session(intent, turn) {
      if (sessionPanel && history.state?.sheet) {
        navigateFromPanel(sessionPanel, () => overviewSession(intent, turn));
        return;
      }
      overviewSession(intent, turn);
    },
    trace(intent, turn) {
      if (sessionPanel && history.state?.sheet) {
        navigateFromPanel(sessionPanel, () => overviewTrace(intent, turn));
        return;
      }
      overviewTrace(intent, turn);
    },
    sessions(filters) {
      const open = () => {
        const value = new URLSearchParams();
        if (filters.machine) value.set('machine', filters.machine);
        if (filters.harness) value.set('harness', filters.harness);
        if (filters.repo === '__none__') value.set('no_repo', '1');
        else if (filters.repo) value.set('repo', filters.repo);
        if (filters.model) value.set('model', filters.model);
        history.pushState(null, '', '/sessions?' + value);
        fromLocation();
      };
      if (sessionPanel && history.state?.sheet) navigateFromPanel(sessionPanel, open);
      else open();
    },
  });
  function overviewSession(intent: { source_key: string; catalog_key: string }, turn?: string) {
    const url =
      sessionPath(intent.source_key, intent.catalog_key) +
      (turn ? '#' + encodeURIComponent(turn) : '');
    if (viewerHost?.nativePage) {
      location.assign(url);
      return;
    }
    history.pushState(null, '', url);
    fromLocation();
  }
  function overviewTrace(intent: { source_key: string; catalog_key: string }, turn: string) {
    const url = tracePath(intent.source_key, intent.catalog_key, turn);
    if (viewerHost?.nativePage) {
      location.assign(url);
      return;
    }
    history.pushState(null, '', url);
    fromLocation();
  }
  function leaveScreen(keepOverview = false) {
    if (navigation.route.v === 'host') {
      hostScroll.delete(navigation.route.id);
      hostScroll.set(navigation.route.id, {
        main: document.querySelector<HTMLElement>('#main')!.scrollTop,
        window: window.scrollY,
      });
      if (hostScroll.size > 24) hostScroll.delete(hostScroll.keys().next().value!);
    }
    ++hostEpoch;
    hostRequest?.abort();
    hostRequest = null;
    hostPage?.destroy();
    hostPage = null;
    hostRoute = null;
    sessionPanel?.destroy();
    ++causalEpoch;
    causal = null;
    causalLoading = false;
    root.removeAttribute('aria-busy');
    causalExpanded.clear();
    if (active) {
      preserveScroll();
      // Cached transcripts keep their bounded store and draft; their modal cannot
      // survive a destination change.
      closeSessionComposer(active.root);
    }
    if (!keepOverview) {
      overview.leave();
      for (const request of overviewRequests) request.abort();
      overviewRequests.clear();
    }
    ++listEpoch;
    updating = false;
    scope.clearTimeout(listRetry);
    cancelCandidate();
    active = null;
    selection.clear();
    sourcesOpen = false;
    releaseScreen(root);
    render(null, root);
    root.replaceChildren();
  }
  function applicationURL(href: string) {
    const url = new URL(href, location.href);
    return viewerHost?.pages &&
      url.origin === location.origin &&
      (viewerHost.pages.match(url) ||
        ['/', '/sessions', '/analytics', '/timeline'].includes(url.pathname) ||
        /^\/(session|s|trace)\//.test(url.pathname))
      ? url
      : null;
  }
  function navigateApplication(href: string) {
    const { sheet: _sheet, drawer: _drawer, ...state } = history.state ?? {};
    history.replaceState(state, '');
    shell.account.close({ navigating: true });
    shell.closeDrawer(true);
    history.pushState(null, '', href);
    fromLocation();
  }
  async function showHostPage(url: URL) {
    const pages = viewerHost?.pages;
    const route = pages?.match(url);
    if (!pages || !route) return;
    leaveScreen();
    hostRoute = route;
    // A source capability response cannot turn a later administrative route
    // back into a reader. The same generation also fences names/source lists.
    ++sourcesEpoch;
    sourcesUpdating = false;
    const epoch = hostEpoch;
    const controller = scope.request();
    const deadline = scope.timeout(() => controller.abort(), 15000);
    hostRequest = controller;
    navigation.route = { v: 'host', id: url.pathname + url.search };
    shell.closeDrawer(true);
    render(
      <p class="catalog-note" role="status">
        Loading…
      </p>,
      root,
    );
    root.setAttribute('aria-busy', 'true');
    chrome();
    void overview.refresh();
    try {
      const initial = initialHostPage;
      initialHostPage = undefined;
      const next =
        initial && url.href === initialHostUrl
          ? initial
          : (initial?.destroy(), await pages.load(url, controller.signal));
      if (disposed || epoch !== hostEpoch || controller.signal.aborted) {
        next?.destroy();
        return;
      }
      if (!next) {
        location.assign(url.href);
        return;
      }
      hostPage = next;
      render(null, root);
      root.replaceChildren(next.element);
      next.mount?.();
      chrome();
      const scroll = hostScroll.get(url.pathname + url.search);
      document.querySelector<HTMLElement>('#main')!.scrollTop = scroll?.main ?? 0;
      window.scrollTo(0, scroll?.window ?? 0);
    } catch (error) {
      if (!disposed && epoch === hostEpoch)
        render(
          <p class="catalog-note" role="status">
            {error && typeof error === 'object' && 'status' in error && error.status === 403
              ? 'You do not have permission to open this page.'
              : 'This page is temporarily unavailable.'}{' '}
            <button class="link" type="button" onClick={() => void showHostPage(url)}>
              Retry
            </button>
          </p>,
          root,
        );
    } finally {
      scope.clearTimeout(deadline);
      scope.releaseRequest(controller);
      if (!disposed && epoch === hostEpoch) {
        root.removeAttribute('aria-busy');
        hostRequest = null;
      }
    }
  }
  // Native forms keep their server method/authorization. Only ordinary same-origin
  // application links enter the sole router; downloads and modified clicks stay native.
  scope.listen(document, 'click', (event) => {
    if (
      event.defaultPrevented ||
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey ||
      !(event.target instanceof Element)
    )
      return;
    const link = event.target.closest<HTMLAnchorElement>('a[href]');
    if (!link || link.hasAttribute('download') || (link.target && link.target !== '_self')) return;
    const url = new URL(link.href, location.href);
    if (
      url.origin !== location.origin ||
      (url.pathname === location.pathname && url.search === location.search && url.hash)
    )
      return;
    if (!applicationURL(url.href)) return;
    event.preventDefault();
    navigateApplication(url.pathname + url.search + url.hash);
  });
  function goOverview(route: 'analytics' | 'machines', push = true) {
    leaveScreen();
    navigation.route = { v: route };
    if (push) history.pushState(navigation.route, '', '/' + route);
    shell.closeDrawer(true);
    overview.draw();
    void overview.refresh();
  }
  function drawCausal() {
    if (!causal || navigation.route.v !== 'trace') return;
    const accepted = causal;
    const results = reachableResultIds(accepted.root, accepted.starts);
    overview.markResults(accepted.source, results);
    accepted.readResults = new Set(results);
    renderCausalTrace(root, accepted, Date.now(), causalExpanded, {
      session(key, turn) {
        const url =
          sessionPath(accepted.source, key) + (turn ? '#' + encodeURIComponent(turn) : '');
        history.pushState(null, '', url);
        fromLocation();
      },
      fold: drawCausal,
    });
    chrome();
  }
  async function loadCausal() {
    const route = navigation.route;
    if (disposed || route.v !== 'trace' || causalLoading) return;
    const epoch = causalEpoch,
      source = capabilities.source_key;
    causalLoading = true;
    if (!causal) root.setAttribute('aria-busy', 'true');
    try {
      const p = params();
      p.set('sid', route.sid);
      p.set('turn', route.turn);
      const next = parseCausalTrace(
        await api('/api/session-trace?' + p),
        source,
        route.sid,
        route.turn,
      );
      const metadataParams = params();
      metadataParams.set('sid', route.sid);
      const metadata = parseCatalogPage(await api('/api/sessions?' + metadataParams));
      if (metadata.machine !== source || metadata.read_scope !== 'retained_history')
        throw Error('Causal metadata selection changed');
      next.metadata = metadata.items.find((item) => item.key === route.sid);
      if (
        disposed ||
        epoch !== causalEpoch ||
        navigation.route !== route ||
        source !== capabilities.source_key
      )
        return;
      const previous = causal;
      next.previewReady = new Set();
      if (previous?.source === source) {
        for (const sid of previous.previewReady ?? []) {
          if (previous.previews[sid] !== next.previews[sid]) continue;
          if (
            [...next.handoffs.values()].some((edge) => {
              const owner = next.previewOwners.get(edge.id),
                old = previous.previewOwners.get(edge.id);
              return (
                (owner?.brief === sid && old?.brief !== sid) ||
                (owner?.result === sid && old?.result !== sid)
              );
            }) ||
            (next.turns[sid] ?? []).some(
              (turn) => !previous.turns[sid]?.some((old) => old.id === turn.id),
            )
          )
            continue;
          next.previewReady.add(sid);
          for (const edge of next.handoffs.values()) {
            const old = previous.handoffs.get(edge.id),
              owner = next.previewOwners.get(edge.id);
            if (!old) continue;
            if (owner?.brief === sid)
              Object.assign(edge, {
                brief: old.brief,
                answer: old.kind === 'toyou' ? old.answer : undefined,
              });
            if (owner?.result === sid) edge.result = old.result;
          }
          for (const turn of next.turns[sid] ?? []) {
            const old = previous.turns[sid]?.find((value) => value.id === turn.id);
            if (turn.u && old?.u) turn.u = old.u;
          }
        }
      }
      const jobs: { sid: string; events: string[]; turns: string[] }[] = [];
      for (const sid of new Set([...Object.keys(next.previews), ...Object.keys(next.sessions)])) {
        if (next.previewReady.has(sid)) continue;
        const entries = [...next.handoffs.values()]
            .filter((edge) => {
              const owner = next.previewOwners.get(edge.id);
              return owner?.brief === sid || owner?.result === sid;
            })
            .map((edge) => ({ event: edge.id })),
          turns = (next.turns[sid] ?? []).map((turn) => ({ turn: turn.id }));
        const selected: ({ event: string } | { turn: string })[] = [...entries, ...turns];
        for (let start = 0; start < selected.length; start += 8) {
          const group = selected.slice(start, start + 8);
          jobs.push({
            sid,
            events: group.flatMap((value) => ('event' in value ? [value.event] : [])),
            turns: group.flatMap((value) => ('turn' in value ? [value.turn] : [])),
          });
        }
      }
      const remaining = new Map<string, number>();
      for (const job of jobs) remaining.set(job.sid, (remaining.get(job.sid) ?? 0) + 1);
      const unavailable = new Set<string>();
      const updateObservation = () => {
        next.observation =
          unavailable.size || next.previewsIncomplete
            ? 'Some recorded text is unavailable. Retrying…'
            : remaining.size
              ? 'Loading recorded text…'
              : undefined;
        if (causal === next && !disposed && epoch === causalEpoch && navigation.route === route)
          drawCausal();
      };
      causal = next;
      updateObservation();
      let pending = 0;
      await Promise.all(
        Array.from({ length: Math.min(4, jobs.length) }, async () => {
          while (pending < jobs.length && !disposed && epoch === causalEpoch) {
            const job = jobs[pending++],
              generation = next.previews[job.sid];
            if (!generation) {
              unavailable.add(job.sid);
              remaining.delete(job.sid);
              updateObservation();
              continue;
            }
            try {
              const query = params();
              query.set('sid', job.sid);
              query.set('generation', generation);
              query.set('events', JSON.stringify(job.events));
              query.set('turns', JSON.stringify(job.turns));
              const detail = object(await api('/api/session-home?' + query));
              if (
                disposed ||
                epoch !== causalEpoch ||
                navigation.route !== route ||
                source !== capabilities.source_key
              )
                return;
              const identity = object(detail.identity);
              if (
                detail.api !== 1 ||
                detail.generation !== generation ||
                identity.source_key !== source ||
                identity.catalog_key !== job.sid ||
                typeof detail.incomplete !== 'boolean'
              )
                throw Error('Causal text selection changed');
              if (detail.incomplete) {
                const values: ({ event: string } | { turn: string })[] = [
                  ...job.events.map((event) => ({ event })),
                  ...job.turns.map((turn) => ({ turn })),
                ];
                if (values.length > 1) {
                  const half = Math.ceil(values.length / 2);
                  for (const part of [values.slice(0, half), values.slice(half)])
                    jobs.push({
                      sid: job.sid,
                      events: part.flatMap((value) => ('event' in value ? [value.event] : [])),
                      turns: part.flatMap((value) => ('turn' in value ? [value.turn] : [])),
                    });
                  remaining.set(job.sid, (remaining.get(job.sid) ?? 0) + 2);
                } else unavailable.add(job.sid);
              }
              const events = object(detail.events),
                texts = object(detail.turns);
              const eventUpdates = job.events.flatMap((id) => {
                if (!events[id]) return [];
                const value = object(events[id]),
                  edge = next.handoffs.get(id)!;
                if (value.brief != null && typeof value.brief !== 'string')
                  throw Error('Invalid causal text');
                if (value.result != null && typeof value.result !== 'string')
                  throw Error('Invalid causal result');
                if (
                  value.answer != null &&
                  (!Array.isArray(value.answer) ||
                    !value.answer.every((item) => typeof item === 'string'))
                )
                  throw Error('Invalid recorded answer');
                const owner = next.previewOwners.get(id);
                return [
                  {
                    edge,
                    ...(owner?.brief === job.sid
                      ? {
                          brief: typeof value.brief === 'string' ? value.brief : '',
                          answer:
                            edge.kind === 'toyou' && Array.isArray(value.answer)
                              ? (value.answer as string[])
                              : undefined,
                        }
                      : {}),
                    ...(owner?.result === job.sid
                      ? { result: typeof value.result === 'string' ? value.result : undefined }
                      : {}),
                  },
                ];
              });
              const turnUpdates = job.turns.map((id) => {
                const turn = next.turns[job.sid].find((turn) => turn.id === id)!;
                if (texts[id] != null && typeof texts[id] !== 'string')
                  throw Error('Invalid recorded prompt');
                return { turn, text: texts[id] };
              });
              for (const { edge, ...fields } of eventUpdates) Object.assign(edge, fields);
              for (const { turn, text } of turnUpdates)
                if (typeof text === 'string') turn.u = { k: 'u', text };
            } catch {
              unavailable.add(job.sid);
            }
            const left = remaining.get(job.sid)! - 1;
            if (left > 0) remaining.set(job.sid, left);
            else {
              remaining.delete(job.sid);
              if (!unavailable.has(job.sid)) next.previewReady!.add(job.sid);
            }
            updateObservation();
          }
        }),
      );
      if (
        disposed ||
        epoch !== causalEpoch ||
        navigation.route !== route ||
        source !== capabilities.source_key
      )
        return;
      causal = next;
      drawCausal();
    } catch (error) {
      if (disposed || epoch !== causalEpoch || navigation.route !== route) return;
      if (causal) {
        causal.observation = readError(error);
        drawCausal();
      } else
        render(
          <p class="catalog-note" role="status">
            {readError(error)}{' '}
            <button class="link" type="button" onClick={() => void loadCausal()}>
              Retry Trace
            </button>
          </p>,
          root,
        );
    } finally {
      if (epoch === causalEpoch) {
        causalLoading = false;
        if (!disposed && navigation.route === route) root.removeAttribute('aria-busy');
      }
    }
  }
  function goCausal(key: string, turn: string) {
    leaveScreen();
    navigation.route = { v: 'trace', sid: key, turn };
    shell.closeDrawer(true);
    render(
      <p class="catalog-note" role="status">
        Loading recorded Trace…
      </p>,
      root,
    );
    chrome();
    void loadCausal();
  }
  function chrome() {
    if (shell.account.open) {
      pendingChrome = true;
      return;
    }
    const route = navigation.route.v;
    title.classList.toggle('catalog-title', route !== 'machine' && route !== 'trace');
    const name =
      route === 'host'
        ? (hostPage?.title ?? hostRoute?.title ?? '')
        : viewerHost?.nativePage && !['home', 'analytics', 'sessions', 'session'].includes(route)
          ? viewerHost.nativePage.title
          : route === 'home'
            ? ((viewerHost?.initialHome?.element.parentElement === root
                ? viewerHost.initialHome.title
                : undefined) ?? 'Home')
            : route === 'analytics'
              ? 'Analytics'
              : route === 'machine'
                ? (overview.machine()?.label ?? 'Machine')
                : route === 'machines'
                  ? 'Machines'
                  : route === 'trace'
                    ? 'Trace'
                    : sourcesOpen
                      ? 'Choose a machine'
                      : (active?.meta?.name ?? (active ? 'Session' : 'Sessions'));
    const recentHeading = document.querySelector('.side-h'),
      recentTree = shell.slots.recent.querySelector<HTMLElement>('#lanes'),
      machineRecent =
        !viewerHost?.nativePage &&
        !['home', 'analytics', 'machines', 'machine', 'host'].includes(route),
      recentScope = overview.recentScope(machineRecent);
    if (recentHeading) {
      recentHeading.textContent = 'Recent';
      if (recentScope) recentHeading.setAttribute('data-tip', recentScope);
      else recentHeading.removeAttribute('data-tip');
    }
    if (recentTree) {
      if (recentScope) recentTree.setAttribute('aria-description', recentScope);
      else recentTree.removeAttribute('aria-description');
    }
    shell.update(
      projectShellNavigation(
        {
          ...(viewerHost?.navigation ?? standaloneNavigation),
          leading: (
            viewerHost?.navigation?.leading ??
            standaloneNavigation?.leading ??
            viewerHost?.nativeNavigation
          )?.filter((item) => item.key !== 'new-session'),
          trailing: (viewerHost?.navigation?.trailing ?? standaloneNavigation?.trailing)?.filter(
            (item) => item.key !== 'new-session',
          ),
        },
        route === 'host'
          ? (hostPage?.nav ?? hostRoute?.nav ?? '')
          : viewerHost?.nativePage && !['home', 'analytics', 'sessions', 'session'].includes(route)
            ? viewerHost.nativePage.nav
            : route === 'machine'
              ? 'machines'
              : route === 'session' || route === 'trace'
                ? 'sessions'
                : route,
        {
          analytics: viewerHost?.navigation?.paths?.analytics ?? '/analytics',
          sessions:
            '/sessions?' +
            (() => {
              const p = browsingParams();
              if (p.has('all_machines')) {
                p.delete('machine');
                p.delete('all_machines');
              }
              return p;
            })(),
          machines: machinesHref(),
        },
      ),
      rail,
    );
    {
      const session = active?.meta;
      const ancestors = session?.details?.family?.ancestors ?? [];
      const phone = matchMedia('(max-width: 760px)').matches;
      const content =
        active && session
          ? sessionBar.update(
              {
                mode: active.errorMode ? 'errors' : active.findOpen ? 'find' : 'normal',
                name,
                session: active.key,
                state: selectedSessionState(session),
                stateLabel: STATE[selectedSessionState(session)] ?? 'Recorded history',
                stateTip: selectedSessionStatusTip(session),
                showState: !(phone && ancestors.length),
                ancestors: (phone ? [] : ancestors).map((row) => ({
                  id: row.key,
                  name: row.name,
                })),
                labels: selectedSessionLabels(session, sourceLabel(active!.sourceKey)),
                trace: false,
                analytics: false,
                days: 7,
                query: active.find ?? '',
                count:
                  visibleEntries(active).length +
                  (active.find
                    ? ' loaded ' + (visibleEntries(active).length === 1 ? 'match' : 'matches')
                    : ' loaded ' + (visibleEntries(active).length === 1 ? 'record' : 'records')),
                filter: active.filter ?? 'all',
                failed: session.details?.errors ?? 0,
                signals: Object.values(session.details?.signals ?? {}).reduce(
                  (sum, count) => sum + count,
                  0,
                ),
                errorMode: active.errorMode ?? '',
                errorText: active.errorMode
                  ? (active.errorMode === 'signals' ? 'Signal ' : 'Error ') +
                    (active.annotations?.total ? (active.errorIndex ?? 0) + 1 : 0) +
                    ' of ' +
                    (active.annotations?.total ?? 0)
                  : '',
                errorDisabled: (active.annotations?.total ?? 0) < 2,
                icons: { search: I.search, more: I.more, back: I.back, x: I.x, up: I.up, dn: I.dn },
              },
              {
                ancestor(key) {
                  void goSession(key);
                },
                crumb() {
                  goList();
                },
                find() {
                  if (!active) return;
                  active.errorMode = undefined;
                  for (const node of active.root.querySelectorAll('.err-current'))
                    node.classList.remove('err-current');
                  active.findOpen = true;
                  chrome();
                  document.querySelector<HTMLInputElement>('#find')?.focus();
                },
                closeFind() {
                  if (!active) return;
                  active.findOpen = false;
                  active.find = '';
                  active.filter = 'all';
                  active.renderedRevision = -1;
                  chrome();
                  drawSelected(active);
                },
                query(value) {
                  if (!active) return;
                  active.find = value.toLowerCase();
                  active.renderedRevision = -1;
                  chrome();
                  drawSelected(active);
                },
                filter(value) {
                  if (!active) return;
                  if (value === 'signals' || value === 'failures') {
                    openAnnotations(active, value === 'signals' ? 'signals' : 'errors');
                    return;
                  }
                  active.filter = value;
                  active.renderedRevision = -1;
                  chrome();
                  drawSelected(active);
                },
                menu(trigger, runs) {
                  if (active) openSessionMenu(active, trigger, runs);
                },
                errors() {
                  if (active) openAnnotations(active, 'errors');
                },
                closeErrors() {
                  if (!active) return;
                  closeAnnotations(active);
                },
                step(direction) {
                  if (!active) return;
                  const count = active.annotations?.total ?? 0;
                  if (!count) return;
                  active.errorIndex = ((active.errorIndex ?? 0) + direction + count) % count;
                  chrome();
                  void readAnnotations(active);
                },
                range() {},
              },
            )
          : null;
      const label = document.createElement('span');
      label.className = 't catalog-session-name l1';
      label.textContent = name;
      if (route === 'trace') {
        if (!traceTitle) {
          render(null, title);
          title.replaceChildren();
        }
        traceTitle = true;
        machineTitle = false;
        const key = navigation.route.v === 'trace' ? navigation.route.sid : '',
          turn = navigation.route.v === 'trace' ? navigation.route.turn : undefined,
          session = causal?.sessions[key];
        render(
          <div class="l1">
            <button
              type="button"
              class="crumb"
              aria-label={'Back to ' + (session?.name ?? 'session')}
              onClick={() => {
                void goSession(key, undefined, true, turn);
              }}
            >
              {session?.name ?? 'Session'}
            </button>
            <span class="crumb-sep">›</span>
            <span class="t">Trace</span>
          </div>,
          title,
        );
      } else if (route === 'machine') {
        if (!machineTitle) {
          render(null, title);
          title.replaceChildren();
        }
        machineTitle = true;
        traceTitle = false;
        const machine = overview.machine();
        render(
          <>
            <div class="l1">
              <button
                type="button"
                class="crumb"
                aria-label="Back to Machines"
                onClick={() => goOverview('machines')}
              >
                Machines
              </button>
              <span class="crumb-sep">›</span>
              <span class="t">{name}</span>
            </div>
            {machine && (
              <div class="meta-line">
                <span class={'lab state ' + (machine.up ? 'done' : 'err')}>
                  <span
                    class={'dot ' + (machine.up ? (machine.working ? 'work' : 'idle') : 'err')}
                    role="img"
                    aria-label={machine.up ? 'Up' : 'Not responding'}
                  />
                  <span>{machine.up ? 'Up' : 'Not responding'}</span>
                </span>
                <span class="lab" data-drop="1">
                  <span>
                    {screenText(
                      machine.working +
                        ' working · ' +
                        machine.sessions +
                        (machine.sessions === 1 ? ' session' : ' sessions'),
                    )}
                  </span>
                </span>
              </div>
            )}
          </>,
          title,
        );
      } else {
        traceTitle = false;
        machineTitle = false;
        render(null, title);
        title.replaceChildren(label);
        if (active) title.prepend(back);
      }
      shell.topbar({
        titleSlot: content && !active?.findOpen && !active?.errorMode ? content.titleSlot : title,
        mode: (active?.findOpen || active?.errorMode) && content ? [content.mode] : undefined,
        actions:
          route === 'analytics'
            ? [overview.actions]
            : active
              ? content
                ? [content.actions]
                : [sources]
              : route === 'sessions' && !sourcesOpen && capabilities.source_candidates
                ? [sources]
                : route === 'trace'
                  ? [traceActions]
                  : [],
        session: !!active,
        lead:
          route === 'trace' && navigation.route.v === 'trace'
            ? {
                label: 'Back to ' + (causal?.metadata?.name ?? 'session'),
                icon: I.chev,
                back: () => {
                  if (navigation.route.v === 'trace')
                    void goSession(navigation.route.sid, undefined, true, navigation.route.turn);
                },
              }
            : { label: 'Open menu', icon: I.menu },
        account: account
          ? {
              account,
              compact: false,
              wide,
              onWideChange() {
                wide = !wide;
                writeShellPreference('wide', wide);
                root.classList.toggle('wide-mode', wide);
                shell.account.updateWide(wide);
              },
            }
          : null,
        accountTarget: content?.accountTarget,
      });
      shell.drawerAccount(account ? { account, compact: true, wide, onWideChange() {} } : null);
    }
    if (active || route === 'machine')
      scope.frame(() => measureViewerBar(document.querySelector<HTMLElement>('#topbar')!));
    if (
      viewerHost?.nativePage ||
      ['home', 'analytics', 'machines', 'machine', 'host'].includes(route)
    )
      recent.update(overview.recentSnapshot(undefined, [], undefined, rail));
    else
      recent.update(
        overview.recentSnapshot(
          capabilities.source_key,
          items,
          navigation.route.v === 'trace' ? navigation.route.sid : active?.key,
          rail,
          active && active.sourceKey === capabilities.source_key ? active.meta : undefined,
        ),
      );
    const pinned = shell.slots.recent.querySelector<HTMLElement>('.tree-row.stuck');
    setGeometry(shell.slots.recent, 'scrollPaddingTop', pinned ? pinned.offsetHeight + 8 : null);
    if (ordering.scopes.get('recent')?.n) {
      if (!orderIdle) armOrderIdle();
    } else {
      scope.clearTimeout(orderIdle);
      orderIdle = undefined;
    }
  }
  function visibleEntries(view: SelectedView) {
    const query = view.find ?? '',
      filter = view.filter ?? 'all';
    return view.store.entries().filter((item) => {
      const entry = item.entry;
      if (filter === 'messages' && !['u', 'a', 'h'].includes(entry.k)) return false;
      if (filter === 'steps' && entry.k !== 'tool') return false;
      if (
        filter === 'failures' &&
        !(entry.k === 'tool' && (entry.ok === false || entry.unfinished))
      )
        return false;
      if (filter === 'signals' && entry.k !== 'signal') return false;
      if (!query) return true;
      const text =
        entry.k === 'tool'
          ? [
              entry.name,
              entry.title,
              entry.arg,
              entry.in,
              entry.out,
              view.fields.get(item.entry_id)?.text,
            ].join(' ')
          : entry.k === 'signal'
            ? recordedSignalLabel(entry)
            : 'text' in entry
              ? (entry.text ?? '')
              : (item.native_action_text ?? '');
      return text.toLowerCase().includes(query);
    });
  }
  function annotationGeneration(view: SelectedView): string | undefined {
    const projection = view.store.selectedPage()?.projection;
    return view.errorMode === 'errors' ? projection?.errorsGeneration : projection?.generation;
  }
  async function readAnnotations(view: SelectedView, force = false) {
    const current = view.store.selectedPage(),
      mode = view.errorMode;
    if (disposed || active !== view || !mode || !current || view.annotationsLoading) return;
    const index = view.errorIndex ?? 0,
      first = Math.floor(index / 100) * 100;
    if (
      !force &&
      view.annotations &&
      view.annotations.generation === annotationGeneration(view) &&
      view.annotations.kind === mode &&
      view.annotations.first === first
    ) {
      highlightAnnotation(view);
      return;
    }
    const epoch = (view.annotationsEpoch ?? 0) + 1;
    view.annotationsEpoch = epoch;
    view.annotationsLoading = true;
    const query = params();
    query.set('sid', view.key);
    query.set('kind', mode);
    query.set('after', String(first));
    query.set('limit', '100');
    query.set('generation', annotationGeneration(view)!);
    chrome();
    try {
      const next = parseCatalogAnnotations(await api('/api/session-annotations?' + query));
      if (
        disposed ||
        active !== view ||
        view.errorMode !== mode ||
        epoch !== view.annotationsEpoch ||
        annotationGeneration(view) !== next.generation
      )
        return;
      if (
        next.source !== view.sourceKey ||
        next.key !== view.key ||
        next.kind !== mode ||
        next.first !== first
      )
        throw Error('Annotation identity changed');
      view.annotations = next;
      view.errorIndex = Math.min(index, Math.max(0, next.total - 1));
      highlightAnnotation(view);
    } catch (error) {
      if (!disposed && active === view && epoch === view.annotationsEpoch) {
        view.note = readError(error);
        drawSelected(view);
      }
    } finally {
      if (epoch === view.annotationsEpoch) view.annotationsLoading = false;
      if (!disposed && active === view) {
        chrome();
        if (view.annotationsFocus && view.errorMode && view.annotations) {
          view.annotationsFocus = false;
          document.querySelector<HTMLButtonElement>('#err-next')?.focus({ preventScroll: true });
        }
      }
    }
  }
  function highlightAnnotation(view: SelectedView) {
    if (active !== view || !view.errorMode) return;
    for (const node of view.root.querySelectorAll('.err-current'))
      node.classList.remove('err-current');
    const page = view.annotations,
      target = page?.entries[(view.errorIndex ?? 0) - page.first];
    if (!target) return;
    const identity = view.store.entries().find((item) => item.slot === target.slot)?.entry_id;
    const node = [...view.root.querySelectorAll<HTMLElement>('[data-entry-key]')].find(
      (node) =>
        node.dataset.entryKey === identity &&
        (node.classList.contains('signal-marker') || node.classList.contains('step')),
    );
    if (!node) {
      if (view.annotationRange !== target.slot) {
        view.annotationRange = target.slot;
        void loadSelected(view, Math.max(0, target.slot - 30)).finally(() => {
          view.annotationRange = undefined;
        });
      }
      return;
    }
    view.following = false;
    node.classList.add('err-current');
    node
      .closest('.tgroup')
      ?.querySelector<HTMLButtonElement>('.tgroup > button[aria-expanded="false"]')
      ?.click();
    // Opening a group changes retained content-visibility turn heights. Commit
    // those heights before computing the target's viewport-relative position.
    measureSessionScreen(view.root);
    const phone = matchMedia('(max-width: 760px)').matches,
      main = document.querySelector<HTMLElement>('#main')!;
    const row = (node.querySelector(':scope > button') ?? node).getBoundingClientRect();
    const top = document.querySelector('#topbar')!.getBoundingClientRect().bottom;
    const bottom = phone ? innerHeight : main.getBoundingClientRect().bottom;
    const delta = (row.top + row.bottom - top - bottom) / 2;
    if (phone) window.scrollBy(0, delta);
    else main.scrollTop += delta;
    syncJump(view, true);
  }
  function closeAnnotations(view: SelectedView) {
    view.errorMode = undefined;
    view.errorIndex = undefined;
    view.annotationsEpoch = (view.annotationsEpoch ?? 0) + 1;
    view.annotationsLoading = false;
    const restore = view.errorRestore;
    view.findOpen = restore?.findOpen ?? false;
    view.find = restore?.find ?? '';
    view.filter = restore?.filter ?? 'all';
    view.errorRestore = undefined;
    for (const node of view.root.querySelectorAll('.err-current'))
      node.classList.remove('err-current');
    if (restore) view.store.retainRanges(restore.ranges);
    view.renderedRevision = -1;
    drawSelected(view);
    const restoreScreen = () => {
      if (disposed || active !== view || view.errorMode || !restore) return;
      for (const button of view.root.querySelectorAll<HTMLButtonElement>(
        '.tgroup > button, .step > button',
      )) {
        const key = button.parentElement?.dataset.entryKey ?? button.parentElement?.dataset.e;
        if (key && (button.getAttribute('aria-expanded') === 'true') !== restore.expanded.has(key))
          button.click();
      }
      if (matchMedia('(max-width: 760px)').matches) window.scrollTo(0, restore.scroll);
      else document.querySelector<HTMLElement>('#main')!.scrollTop = restore.scroll;
      syncJump(view, true);
      chrome();
      document.querySelector<HTMLElement>(restore.focus)?.focus({ preventScroll: true });
    };
    if (restore?.key && !view.store.entries().some((item) => item.entry_id === restore.key))
      void loadSelected(view, restore.first).finally(() => scope.frame(restoreScreen));
    else scope.frame(restoreScreen);
    chrome();
  }
  function openAnnotations(view: SelectedView, mode: 'errors' | 'signals') {
    if (!view.errorMode) {
      const entries = view.store.entries(),
        first = entries[0];
      view.errorRestore = {
        scroll: matchMedia('(max-width: 760px)').matches
          ? window.scrollY
          : document.querySelector<HTMLElement>('#main')!.scrollTop,
        first: first?.slot ?? 0,
        key: first?.entry_id,
        ranges: view.store.loadedRanges(),
        findOpen: view.findOpen ?? false,
        find: view.find,
        filter: view.filter,
        focus: view.findOpen
          ? '#find'
          : matchMedia('(max-width: 760px)').matches
            ? '#more-btn'
            : '.lab-errs',
        expanded: new Set(
          [
            ...view.root.querySelectorAll<HTMLElement>(
              '.tgroup > button[aria-expanded="true"], .step > button[aria-expanded="true"]',
            ),
          ]
            .map((node) => node.parentElement?.dataset.entryKey ?? node.parentElement?.dataset.e)
            .filter((key): key is string => !!key),
        ),
      };
    }
    view.find = '';
    view.filter = 'all';
    view.findOpen = false;
    view.renderedRevision = -1;
    view.errorMode = mode;
    view.errorIndex = 0;
    view.annotations = undefined;
    view.annotationsFocus = true;
    drawSelected(view);
    chrome();
    void readAnnotations(view);
    document.querySelector<HTMLButtonElement>('#err-close')?.focus();
  }
  function openSessionMenu(view: SelectedView | null, trigger: HTMLButtonElement, runs = false) {
    const metadata = view?.meta ?? causal?.metadata;
    if (!metadata || (view ? active !== view : navigation.route.v !== 'trace')) return;
    sessionPanel?.destroy();
    const turn = view
      ? [...view.root.querySelectorAll<HTMLElement>('[data-turn]')].find(
          (node) =>
            node.getBoundingClientRect().bottom >
            document.querySelector<HTMLElement>('#topbar')!.getBoundingClientRect().bottom,
        )?.dataset.turn
      : undefined;
    const snapshot = selectedSessionMenu(
      metadata,
      sourceLabel(view?.sourceKey ?? capabilities.source_key),
      wide,
      !!view &&
        view.store
          .context()
          .turns.some(
            (row) =>
              row.id === turn &&
              row.sent.some((id) =>
                view.store
                  .context()
                  .handoffs.some(({ edge }) => edge.id === id && edge.kind !== 'move'),
              ),
          ),
    );
    const panel = createPanelChrome(
      {
        title: metadata.name,
        sub: snapshot.status,
        label: 'Session menu for ' + metadata.name,
        className: 'anchored session-menu',
      },
      {
        opened() {
          history.pushState({ ...navigation.route, sheet: 1 }, '');
          trigger.setAttribute('aria-expanded', 'true');
        },
        closed() {
          render(null, panel.body);
          if (sessionPanel === panel) sessionPanel = null;
          trigger.setAttribute('aria-expanded', 'false');
          if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
            overlayBackPending = true;
            history.back();
          }
          if (trigger.isConnected) trigger.focus({ preventScroll: true });
        },
      },
    );
    sessionPanel = panel;
    renderSessionMenu(panel.body, snapshot, {
      wide: () => wide,
      session(key) {
        navigateFromPanel(panel, () => {
          void goSession(key);
        });
      },
      action(key) {
        if (key === 'runs') {
          panel.body.querySelector('.runs')?.scrollIntoView({ block: 'start' });
        } else if (key === 'wide') {
          wide = !wide;
          writeShellPreference('wide', wide);
          root.classList.toggle('wide-mode', wide);
          shell.account.updateWide(wide);
        } else if (key === 'signals' || key === 'errors') {
          navigateFromPanel(panel, () => {
            const kind = key === 'errors' ? 'errors' : 'signals';
            if (view) openAnnotations(view, kind);
            else
              void goSession(metadata.key).then(() => {
                if (active?.key === metadata.key) openAnnotations(active, kind);
              });
          });
        } else if (key === 'trace' && turn && view) {
          navigateFromPanel(panel, () => {
            history.pushState(
              null,
              '',
              tracePath(view.sourceKey, view.key, turn) + '?' + sessionControls(),
            );
            goCausal(view.key, turn);
          });
        }
      },
    });
    panel.show();
    if (runs) panel.body.querySelector<HTMLElement>('.runs')?.scrollIntoView({ block: 'nearest' });
  }
  function openRecordedTool(view: SelectedView, key: string, inputLabel: string, script = false) {
    if (active !== view || disposed || !view.meta) return;
    const item = view.store.entries().find((item) => item.entry_id === key);
    if (!item || item.entry.k !== 'tool') return;
    const entry = item.entry,
      trigger = document.activeElement;
    sessionPanel?.destroy();
    const panel = createPanelChrome(
      {
        title: toolInfo(entry.name)[1] + ' ' + entry.arg,
        sub: entry.name + ' · ' + (entry.ok === false ? 'Failed' : 'Recorded result'),
        className: 'full',
      },
      {
        opened() {
          history.pushState({ ...navigation.route, sheet: 1 }, '');
        },
        closed() {
          render(null, panel.body);
          if (sessionPanel === panel) sessionPanel = null;
          if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
            overlayBackPending = true;
            history.back();
          }
          if (active === view && trigger instanceof HTMLElement && trigger.isConnected)
            trigger.focus({ preventScroll: true });
        },
      },
    );
    panel.body.classList.add('viewer-b');
    sessionPanel = panel;
    let scriptResult: { scriptText?: string; scriptTruncated?: boolean; scriptFailed?: boolean } =
      {};
    let outputResult: Pick<ToolData, 'out' | 'cut'> | undefined,
      outputNote = '';
    const draw = () => {
      if (disposed || active !== view || sessionPanel !== panel) return;
      const field = script ? null : fieldView(view, item),
        fieldAction = field?.action,
        fieldNote = script
          ? ''
          : field?.note ||
            (item.clipped ? 'Recorded text preview. The complete source text is not loaded.' : ''),
        data = {
          ...entry,
          ...(script ? scriptResult : { scriptText: undefined }),
          ...(field?.text === undefined ? {} : { out: field.text }),
          ...(field ? { boundedPreview: true } : {}),
          ...outputResult,
        };
      render(
        <>
          {script && scriptResult.scriptText === undefined && !scriptResult.scriptFailed ? (
            <p role="status" class="vnote">
              Loading recorded script…
            </p>
          ) : (
            <FullTool
              data={data}
              inputLabel={inputLabel}
              waiting={selectedSessionState(view.meta!) === 'wait'}
            />
          )}
          {(fieldNote || fieldAction) && (
            <p class="vnote">
              {fieldNote}
              {fieldAction && (
                <button
                  class="btn sh-quiet"
                  type="button"
                  disabled={fieldAction.busy}
                  onClick={() => {
                    void loadField(view, item).then(draw);
                    draw();
                  }}
                >
                  {fieldAction.label}
                </button>
              )}
            </p>
          )}
          {outputNote && (
            <p class="vnote" role="status">
              {outputNote}
            </p>
          )}
        </>,
        panel.body,
      );
    };
    draw();
    panel.show();
    if (script) {
      const generation = view.store.selectedPage()!.projection.generation,
        p = params();
      p.set('sid', view.key);
      p.set('after', String(item.slot));
      p.set('limit', '1');
      p.set('generation', generation);
      p.set('part', 'script');
      void api('/api/session-entry?' + p)
        .then((value) => {
          if (
            disposed ||
            active !== view ||
            sessionPanel !== panel ||
            view.store.selectedPage()?.projection.generation !== generation
          )
            return;
          const page = parseCatalogTranscriptPage(value),
            saved = page.entries[0];
          if (
            page.identity.source_key !== view.sourceKey ||
            page.identity.catalog_key !== view.key ||
            page.identity.read_scope !== view.store.selectedPage()!.identity.read_scope ||
            page.projection.generation !== generation ||
            page.range.first !== item.slot ||
            page.entries.length !== 1 ||
            saved.entry_id !== item.entry_id ||
            JSON.stringify(saved.provenance) !== JSON.stringify(item.provenance) ||
            saved.entry.k !== 'tool' ||
            typeof saved.entry.scriptText !== 'string'
          )
            throw Error('Invalid recorded script scope');
          scriptResult = {
            scriptText: saved.entry.scriptText,
            scriptTruncated: saved.entry.scriptTruncated,
          };
          draw();
        })
        .catch(() => {
          scriptResult = { scriptFailed: true };
          draw();
        });
    } else if (fieldView(view, item)?.action) {
      void loadField(view, item).then(draw);
      draw();
    } else if (entry.cut) {
      const generation = view.store.selectedPage()!.projection.generation,
        p = params();
      p.set('sid', view.key);
      p.set('after', String(item.slot));
      p.set('limit', '1');
      p.set('generation', generation);
      p.set('part', 'output');
      outputNote = 'Loading recorded output…';
      draw();
      void api('/api/session-entry?' + p)
        .then((value) => {
          if (
            disposed ||
            active !== view ||
            sessionPanel !== panel ||
            view.store.selectedPage()?.projection.generation !== generation
          )
            return;
          const page = parseCatalogTranscriptPage(value),
            saved = page.entries[0];
          if (
            page.identity.source_key !== view.sourceKey ||
            page.identity.catalog_key !== view.key ||
            page.identity.read_scope !== view.store.selectedPage()!.identity.read_scope ||
            page.projection.generation !== generation ||
            page.range.first !== item.slot ||
            page.entries.length !== 1 ||
            saved.entry_id !== item.entry_id ||
            JSON.stringify(saved.provenance) !== JSON.stringify(item.provenance) ||
            saved.entry.k !== 'tool'
          )
            throw Error('Invalid recorded output scope');
          outputResult = { out: saved.entry.out, cut: saved.entry.cut };
          outputNote = saved.entry.outputTruncated
            ? 'The output preview is bounded; the remaining text is not shown.'
            : '';
          draw();
        })
        .catch(() => {
          outputNote = "Couldn't load the full text: this is the preview.";
          draw();
        });
    }
  }
  function openCatalogSession(item: Pick<CatalogSession, 'key' | 'harness' | 'machine'>) {
    if (item.machine) {
      saveSource();
      const p = browsingParams();
      p.delete('machine');
      p.set('all_machines', '1');
      history.pushState(
        { v: 'session', id: item.key },
        '',
        sessionPath(item.machine.key, item.key) + '?' + p,
      );
      fromLocation();
    } else
      void goSession(
        item.key,
        items.find((row) => row.key === item.key),
      );
  }
  function drawList() {
    if (disposed || navigation.route.v !== 'sessions' || active || sourcesOpen) return;
    chrome();
    const focused = document.activeElement;
    const draft =
      focused instanceof HTMLInputElement && root.contains(focused)
        ? {
            node: focused,
            value: focused.value,
            start: focused.selectionStart,
            end: focused.selectionEnd,
          }
        : null;
    const descendants = new Map<string, CatalogSession[]>(),
      loaded = new Map(items.map((item) => [catalogSessionKey(item), item]));
    for (const item of items) {
      const seen = new Set([catalogSessionKey(item)]);
      let parent = item.parent
        ? catalogSessionKey({ key: item.parent, machine: item.machine })
        : null;
      while (parent && !seen.has(parent) && loaded.has(parent)) {
        seen.add(parent);
        if (showApprovalReviews || item.presentation_kind !== 'Approval review')
          descendants.set(parent, [...(descendants.get(parent) ?? []), item]);
        const ancestor = loaded.get(parent)!;
        parent = ancestor.parent
          ? catalogSessionKey({ key: ancestor.parent, machine: ancestor.machine })
          : null;
      }
    }
    const ordered = orderRows(
      orderScope(
        'sessions',
        JSON.stringify([capabilities.source_key, harness, repo, model, query, showApprovalReviews]),
      ),
      'sessions',
      items
        .filter((item) => showApprovalReviews || item.presentation_kind !== 'Approval review')
        .map((item) => ({ ...item, id: catalogSessionKey(item) })),
      (a, b) =>
        (b.last ?? 0) - (a.last ?? 0) ||
        (capabilities.global_union
          ? compareCatalogKeys(a.key, b.key)
          : a.key.localeCompare(b.key)) ||
        compareCatalogKeys(a.machine?.key ?? '', b.machine?.key ?? ''),
    ).map((item) => ({
      ...item,
      presentation: catalogSessionRow(
        item,
        item.parent ? [] : descendants.get(catalogSessionKey(item)),
      ),
    }));
    const observationState: CatalogObservationState = page?.freshness ?? 'cached',
      observation = {
        cached: 'Cached history',
        updating: 'Discovering history',
        stale: 'History observation is stale',
        incomplete: 'History is incomplete',
        unavailable: 'History observation is unavailable',
      }[observationState],
      selectedSourceLabel = sourceLabel(capabilities.source_key),
      filterDescription = catalogFilterDescription(
        selectedSourceLabel,
        observation,
        capabilities.metadata_search,
        capabilities.global_union,
      );
    catalogBrowser.update(
      {
        machine: viewerHost?.catalogSources ? capabilities.source_key : undefined,
        harness,
        repo,
        model,
        q: query,
      },
      items,
      capabilities.source_key,
      filterDescription,
      sourceItems,
      !!viewerHost?.catalogSources,
    );
    const groupKey = (item: CatalogSession) =>
      groupBy === 'project'
        ? (item.repo ?? 'No repo (roles)')
        : groupBy === 'harness'
          ? (HARNESS[item.harness] ?? item.harness)
          : groupBy === 'machine'
            ? (item.machine?.label ?? sourceLabel(capabilities.source_key))
            : '';
    const titles = [...new Set(ordered.map(groupKey))].sort(
      (a, b) =>
        Number(a.startsWith('No repo')) - Number(b.startsWith('No repo')) || a.localeCompare(b),
    );
    const keys = new Set(ordered.map((item) => cacheKey(item.key)));
    for (const key of listExpanded) if (!keys.has(key)) listExpanded.delete(key);
    renderCatalogList(
      root,
      {
        items: ordered,
        completeCount: completeCatalogCount({
          page,
          items,
          witness: countWitness,
          source: capabilities.source_key,
          filters: countFilters(),
          searchRequested: query !== '',
          updating,
          more: cursor !== null,
        }),
        browser: {
          facets: catalogBrowser.element,
          groupBy,
          working: ordered.filter((item) => item.status?.state === 'work').length,
          waiting: ordered.filter((item) => item.status?.state === 'wait').length,
          groups: titles.map((title) => ({
            title,
            items: ordered.filter((item) => groupKey(item) === title),
          })),
        },
        groups: catalogSessionGroups(ordered).map((group) => ({
          ...group,
          expanded: listExpanded.has(cacheKey(group.root.key)),
        })),
        reviews: showApprovalReviews,
        hasReviews:
          showApprovalReviews || items.some((item) => item.presentation_kind === 'Approval review'),
        sourceLabel: page?.machine_info.label ?? (capabilities.source_key || 'This source'),
        updating,
        observationState,
        candidates,
        candidatesMore: candidateCursor !== null,
        candidateUpdating,
        candidateNote,
        discovering: page?.completeness.state === 'partial',
        observation,
        filterDescription: filterDescription.description,
        searchQualification: filterDescription.searchQualification,
        note,
        harness,
        repo,
        model,
        query,
        metadataSearch: capabilities.metadata_search,
        globalUnion: capabilities.global_union,
        searchPartial: page?.search?.partial ?? false,
        searchIndexIncomplete: page?.search ? !page.search.index_complete : false,
        more: cursor !== null,
      },
      {
        group(value) {
          groupBy = value;
          drawList();
        },
        session: openCatalogSession,
        children(key, expanded) {
          if (expanded) listExpanded.add(cacheKey(key));
          else listExpanded.delete(cacheKey(key));
          drawList();
        },
        reviews() {
          showApprovalReviews = !showApprovalReviews;
          ordering.scopes.clear();
          drawList();
          chrome();
        },
        candidate(item) {
          void prepareCandidate(item);
        },
        candidateMore() {
          void loadCandidates(true);
        },
        candidateRetry() {
          void loadCandidates(false);
        },
        searchDraft(value) {
          // Preserve bookmarkable input immediately; native observation starts
          // only when the form commits its filters.
          const draft = browsingParams();
          if (value.trim()) draft.set('q', value.trim());
          else draft.delete('q');
          history.replaceState(history.state, '', '/sessions?' + draft);
        },
        filters: applyListFilters,
        clearFilters: clearListFilters,
        more() {
          void loadList(true);
        },
        retry() {
          void loadList(false);
        },
      },
    );
    if (draft?.node.isConnected && document.activeElement === draft.node) {
      draft.node.value = draft.value;
      if (draft.start !== null && draft.end !== null)
        draft.node.setSelectionRange(draft.start, draft.end);
    }
  }
  async function loadList(append: boolean) {
    if (
      disposed ||
      (!capabilities.source_key && !capabilities.global_union) ||
      (navigation.route.v !== 'sessions' && !active) ||
      (append && updating)
    )
      return;
    scope.clearTimeout(listRetry);
    const epoch = ++listEpoch,
      p = params(),
      filters = countFilters();
    p.set('limit', '60');
    if (harness) p.set('harness', harness);
    if (repo === '__none__') p.set('no_repo', '1');
    else if (repo) p.set('repo', repo);
    if (model) p.set('model', model);
    if (query && capabilities.metadata_search) p.set('q', query);
    if (append && cursor !== null) p.set('cursor', cursor);
    updating = true;
    note = '';
    drawList();
    try {
      const next = parseCatalogPage(await api('/api/sessions?' + p));
      if (disposed || epoch !== listEpoch) return;
      if (
        (query && capabilities.metadata_search && next.search === null) ||
        next.machine !== capabilities.source_key ||
        next.read_scope !== 'retained_history' ||
        (append && page?.generation !== next.generation)
      )
        throw new Error('History changed; refresh this list before loading another page.');
      if (
        append &&
        next.items.some((item) =>
          items.some((old) => catalogSessionKey(old) === catalogSessionKey(item)),
        )
      )
        throw new Error('History page repeated a session. Refresh this list.');
      const pendingFacts = next.items.some(
        (item) =>
          item.lifecycle === 'current' &&
          (item.facts_observation?.state === 'stale' ||
            item.facts_observation?.state === 'unavailable'),
      );
      if (!pendingFacts) listRetryDelay = 1000;
      page = next;
      countWitness = catalogCountWitness(next, filters, !append);
      if (capabilities.source_candidates && !candidateLoaded) void loadCandidates(false);
      cursor = next.next_cursor;
      items = append ? [...items, ...next.items].slice(-300) : next.items;
      // Explicit pagination admits the newly requested rows immediately. The
      // passive observation ordering must not hold them outside the viewport.
      if (append) ordering.scopes.delete('sessions');
      updating = false;
      drawList();
      chrome();
      if (
        (next.completeness.state === 'partial' ||
          next.search?.index_complete === false ||
          pendingFacts) &&
        items.length <= 60
      ) {
        const refresh = () => {
          if (disposed || epoch !== listEpoch || active || sourcesOpen) return;
          // drawList preserves the focused filter's draft and selection while
          // provisional discovery advances; focus must not stall publication.
          void loadList(false);
        };
        listRetry = scope.timeout(refresh, pendingFacts ? listRetryDelay : 1000);
        if (pendingFacts) listRetryDelay = Math.min(8000, listRetryDelay * 2);
      }
    } catch (error) {
      if (disposed || epoch !== listEpoch) return;
      updating = false;
      if (
        append &&
        error &&
        typeof error === 'object' &&
        'status' in error &&
        error.status === 409
      ) {
        void loadList(false);
        return;
      }
      note = readError(error);
      drawList();
      if (retryable(error)) {
        listRetry = scope.timeout(() => void loadList(append), listRetryDelay);
        listRetryDelay = Math.min(8000, listRetryDelay * 2);
      }
    }
  }
  function cancelCandidate() {
    candidateEpoch++;
    scope.clearTimeout(candidateTimer);
    candidateUpdating = false;
  }
  async function loadCandidates(append: boolean) {
    if (!capabilities.source_candidates || disposed || (append && candidateUpdating)) return;
    cancelCandidate();
    const epoch = candidateEpoch,
      sourceKey = capabilities.source_key;
    candidateLoaded = true;
    candidateUpdating = true;
    candidateNote = '';
    drawList();
    const p = new URLSearchParams({ machine: sourceKey, limit: '60' });
    if (append && candidateCursor) p.set('cursor', candidateCursor);
    try {
      const result = parseCatalogSourceCandidates(await api('/api/session-source-candidates?' + p));
      if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
      if (
        result.source_key !== sourceKey ||
        (append &&
          result.items.some((item) =>
            candidates.some((old) => old.candidate_key === item.candidate_key),
          ))
      )
        throw new Error('Invalid archive source scope');
      candidates = append ? [...candidates, ...result.items] : result.items;
      candidateCursor = result.next_cursor;
    } catch (error) {
      if (disposed || epoch !== candidateEpoch) return;
      candidateNote = readError(error);
    } finally {
      if (!disposed && epoch === candidateEpoch) {
        candidateUpdating = false;
        drawList();
      }
    }
  }
  async function prepareCandidate(candidate: CatalogSourceCandidate) {
    cancelCandidate();
    const epoch = candidateEpoch,
      sourceKey = capabilities.source_key;
    candidateUpdating = true;
    candidateNote = '';
    drawList();
    let delay = 1000;
    const poll = async () => {
      if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
      try {
        const p = new URLSearchParams({
          machine: sourceKey,
          candidate: candidate.candidate_key,
          generation: candidate.generation,
        });
        const result = parseCatalogSourceCandidateProgress(
          await api('/api/session-source-candidate?' + p),
        );
        if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
        if (
          result.source_key !== sourceKey ||
          result.candidate_key !== candidate.candidate_key ||
          result.generation !== candidate.generation
        )
          throw new Error('Archive source changed; resynchronize source hints');
        if (result.state === 'ready') {
          const metadata = parseCatalogPage(
            await api(
              '/api/sessions?' +
                new URLSearchParams({
                  machine: sourceKey,
                  scope: 'retained_history',
                  sid: result.catalog_key!,
                }),
            ),
          );
          if (disposed || epoch !== candidateEpoch || sourceKey !== capabilities.source_key) return;
          if (
            metadata.machine !== sourceKey ||
            metadata.read_scope !== 'retained_history' ||
            metadata.items.length !== 1 ||
            metadata.items[0].key !== result.catalog_key
          )
            throw new Error('Parsed archive session identity is unavailable');
          candidateUpdating = false;
          await goSession(result.catalog_key!, metadata.items[0]);
          return;
        }
        candidateNote =
          result.reason ??
          (result.state === 'updating'
            ? 'Recorded source is being indexed. Its session will open automatically.'
            : 'This archived source cannot currently be read.');
        if (result.state === 'unavailable' && !result.retryable) {
          candidateUpdating = false;
          drawList();
          return;
        }
        drawList();
        candidateTimer = scope.timeout(() => void poll(), delay);
        delay = Math.min(delay * 2, 8000);
      } catch (error) {
        if (disposed || epoch !== candidateEpoch) return;
        candidateNote = readError(error);
        candidateUpdating = retryable(error);
        drawList();
        if (candidateUpdating) {
          candidateTimer = scope.timeout(() => void poll(), delay);
          delay = Math.min(delay * 2, 8000);
        }
      }
    };
    await poll();
  }
  function readError(error: unknown): string {
    if (
      !(error && typeof error === 'object' && 'status' in error) &&
      !(error instanceof Error && error.name === 'AbortError')
    )
      return 'The source returned an invalid history view. Retry after checking this source.';
    const status = error && typeof error === 'object' && 'status' in error ? error.status : 0;
    if (status === 401 || status === 403)
      return 'Authorization is required to read this source. Reconnect your account and retry.';
    if (status === 400) return 'The requested filter or range is unsupported by this source.';
    if (status === 404) return 'This recorded session is unavailable from the selected source.';
    if (status === 409) return 'Recorded history changed. Resynchronizing…';
    if (status === 503) return 'The source is updating or temporarily unavailable. Retrying…';
    if (status === 0) return 'Source observation is unavailable. Retrying…';
    return 'History could not be read. Retry after checking your connection.';
  }
  function retryable(error: unknown) {
    if (!(error && typeof error === 'object' && 'status' in error))
      return error instanceof Error && error.name === 'AbortError';
    const status = error && typeof error === 'object' && 'status' in error ? error.status : 0;
    return (
      status === 0 ||
      status === 429 ||
      status === 500 ||
      status === 502 ||
      status === 503 ||
      status === 504
    );
  }
  function preserveScroll() {
    const scroll = window.matchMedia('(max-width: 760px)').matches
      ? window.scrollY
      : document.querySelector<HTMLElement>('#main')!.scrollTop;
    if (active) active.scroll = scroll;
    else if (!sourcesOpen) listScroll = scroll;
  }
  function goList(push = true, keepOverview = false) {
    if (push && active && new URLSearchParams(location.search).get('all_machines') === '1') {
      const p = browsingParams();
      p.delete('machine');
      p.delete('all_machines');
      history.pushState({ v: 'sessions' }, '', '/sessions?' + p);
      fromLocation();
      return;
    }
    leaveScreen(keepOverview);
    navigation.route = { v: 'sessions' };
    if (push) history.pushState(navigation.route, '', '/sessions?' + browsingParams());
    root.replaceChildren();
    drawList();
    const row = [...root.querySelectorAll<HTMLButtonElement>('[data-session-row="list"]')].find(
      (node) => node.dataset.id === listFocus,
    );
    row?.focus({ preventScroll: true });
    if (window.matchMedia('(max-width: 760px)').matches) window.scrollTo(0, listScroll);
    else document.querySelector<HTMLElement>('#main')!.scrollTop = listScroll;
    if ((!page || (page.completeness.state === 'partial' && items.length <= 60)) && !updating)
      void loadList(false);
    shell.closeDrawer(true);
  }
  function observeSelectedProgress(view: SelectedView, progress: CatalogRecoveryProgress) {
    if (view.sourceKey !== capabilities.source_key) return;
    if (!view.recovery.observeProgress(progress)) return;
    view.note = '';
    view.transientNote = false;
    view.renderedRevision = -1;
    drawSelected(view);
    if (
      !disposed &&
      active === view &&
      !view.loading &&
      view.retry === undefined &&
      capabilities.selected_transcript
    )
      void loadSelected(view);
  }
  function drawSelected(view: SelectedView, captureReading = true) {
    if (disposed || active !== view) return;
    const current = view.store.selectedPage(),
      identity = selection.selectedIdentity(),
      recoveryNote = view.recovery.stopped
        ? 'Recorded history could not be loaded. Retry to try again.'
        : undefined,
      observationNote =
        recoveryNote ??
        [
          view.anchorNote,
          view.note,
          view.observationFailure?.text,
          view.meta?.source_refs.some(
            (reference) => reference.state === 'unavailable' || reference.state === 'incomplete',
          )
            ? 'History is incomplete. Previously loaded text is retained.'
            : undefined,
        ]
          .filter(Boolean)
          .join(' ');
    if (!current) {
      renderSessionScreen(
        view.root,
        {
          id: view.key,
          name: view.meta?.name ?? 'Session',
          control: controlFor(view),
          runtime: runtimeFor(view),
          blocks: [],
          order: [],
          observation: view.recovery.stopped
            ? 'Recorded history is unavailable'
            : 'Recorded history is updating',
          retry:
            view.recovery.stopped || (view.note && !view.loading)
              ? {
                  label: 'Retry selected transcript',
                  run() {
                    scope.clearTimeout(view.retry);
                    view.retry = undefined;
                    view.recovery.reset();
                    void loadSelected(view);
                  },
                }
              : undefined,
          empty:
            recoveryNote ||
            observationNote ||
            (capabilities.selected_transcript
              ? 'Loading recorded session…'
              : 'Recorded transcript is updating. The source refresh will appear automatically.'),
        },
        {
          committed() {},
          trace(turn) {
            const url = tracePath(view.sourceKey, view.key, turn) + '?' + sessionControls();
            history.pushState(null, '', url);
            goCausal(view.key, turn);
          },
          session(key) {
            void goSession(key);
          },
          machine() {},
          sender() {},
          toolAll() {},
          script() {},
          image() {},
          background() {},
          jump() {},
          pager() {},
        },
      );
      return;
    }
    const changedProjection = view.jumpGeneration !== current.projection.generation;
    view.jumpGeneration = current.projection.generation;
    // A reader can move before the browser delivers its scroll event. Capture
    // the existing reading geometry before a late context redraw, as the
    // complete viewer's scroll transaction does, rather than trusting that
    // event's cached following flag. Initial tail placement keeps its owner.
    if (
      captureReading &&
      view.following &&
      !view.opening &&
      view.root.querySelector('[data-entry-key]') &&
      !(view === pinView && performance.now() < pinUntil) &&
      readingGap() > 80
    ) {
      view.following = false;
      pinUntil = 0;
    }
    if (view.lastTotal !== null && !view.following && current.projection.total > view.lastTotal)
      view.newCount += current.projection.total - view.lastTotal;
    view.lastTotal = current.projection.total;
    overview.markResults(
      view.sourceKey,
      view.store
        .context()
        .handoffs.filter(
          (row) =>
            row.edge.kind === 'toyou' && row.edge.ask === 'result' && row.edge.from === view.key,
        )
        .map((row) => row.edge.id),
    );
    const context = view.store.context(),
      origin = context.handoffs.find(
        (row) => row.edge.kind === 'spawn' && row.edge.to === view.key,
      ),
      parent = origin && context.sessions[origin.edge.from],
      footer = view.meta
        ? selectedSessionFooter(
            view.meta,
            origin && parent
              ? {
                  id: origin.edge.from,
                  name: parent.name,
                  status: origin.edge.status,
                  done: origin.edge.done,
                  turn: origin.sourceTurn,
                }
              : undefined,
          )
        : undefined,
      footerKey = JSON.stringify(footer ?? null);
    if (
      view.renderedRevision === view.store.revision &&
      view.renderedNote === observationNote &&
      view.renderedFooter === footerKey
    ) {
      measureSessionScreen(view.root);
      updateSessionControl(view.root, controlFor(view));
      updateSessionRuntime(view.root, runtimeFor(view));
      syncJump(view, changedProjection);
      if ((view.store.loadedRanges()[0]?.first ?? 0) > 0)
        updateSessionPager(view.root, {
          sid: view.key,
          where: 'before',
          text: 'Load earlier records',
          disabled: view.loading,
          busy: view.loading,
        });
      // A cached redraw can still change clamp/pager geometry. Keep the same
      // post-commit annotation placement as a full transcript redraw.
      if (view.errorMode) scope.frame(() => highlightAnnotation(view));
      return;
    }
    view.renderedRevision = view.store.revision;
    view.renderedNote = observationNote;
    view.renderedFooter = footerKey;
    const anchor = [...view.root.querySelectorAll<HTMLElement>('[data-entry-key]')].find(
      (node) =>
        node.getBoundingClientRect().bottom >
        (window.matchMedia('(max-width: 760px)').matches
          ? 0
          : document.querySelector<HTMLElement>('#main')!.getBoundingClientRect().top),
    );
    const anchorTop = anchor?.getBoundingClientRect().top;
    renderSessionScreen(
      view.root,
      {
        id: view.key,
        name: view.meta?.name ?? 'Session',
        footer,
        control: controlFor(view),
        runtime: runtimeFor(view),
        retry: view.recovery.stopped
          ? {
              label: 'Retry selected transcript',
              run() {
                scope.clearTimeout(view.retry);
                view.retry = undefined;
                view.recovery.reset();
                void loadSelected(view);
              },
            }
          : undefined,
        observation:
          observationNote ||
          {
            cached: 'Cached history',
            stale: 'Stale history',
            incomplete: 'History is incomplete',
            unavailable: 'Source history is unavailable',
          }[current.freshness.state] +
            (view.store.context().state === 'incomplete'
              ? ' · related session context is incomplete'
              : ''),
        observationSummary:
          !view.anchorNote &&
          (!view.note || view.transientNote) &&
          (!view.observationFailure || view.observationFailure.retryable)
            ? view.note || view.observationFailure
              ? 'Updates delayed'
              : 'History details'
            : undefined,
        started:
          view.store.loadedRanges()[0]?.first === 0 && view.meta?.start != null
            ? {
                lead: 'Started ' + clock(view.meta.start, Date.now()) + ' on\u00a0',
                machine: sourceLabel(view.sourceKey),
              }
            : undefined,
        blocks: catalogTranscriptBlocks(
          visibleEntries(view),
          (entry) => fieldView(view, entry),
          (item) => {
            if (item.entry.k !== 'h') return null;
            const context = view.store.context(),
              relationship = context.handoffs.find(
                (row) => row.edge.id === (item.entry as { id: string }).id,
              );
            if (!relationship) return null;
            const preview = (owner: string | undefined) =>
              owner
                ? view.contextText?.get(
                    JSON.stringify([owner, context.previews[owner], relationship.edge.id]),
                  )
                : undefined;
            return recordedRelationshipViews(item, relationship, view.key, context.sessions, {
              activity: view.contextText?.get(
                JSON.stringify([
                  relationship.edge.to,
                  context.previews[relationship.edge.to],
                  null,
                ]),
              )?.activity,
              brief: preview(relationship.briefOwner)?.brief,
              result: preview(relationship.resultOwner)?.result,
              answer: preview(relationship.briefOwner)?.answer,
              trace: context.turns.find((turn) => turn.sent.includes(relationship.edge.id))?.id,
            });
          },
          view.meta?.details?.live_observation === true,
          view.store.context(),
          (item) =>
            (item.entry.img ?? []).map((image, index) => {
              const p = params();
              p.set('sid', view.key);
              p.set('after', String(item.slot));
              p.set('limit', '1');
              p.set('generation', current.projection.generation);
              p.set('part', 'attachment');
              p.set('image', String(index));
              const scale = image.w && image.h ? Math.min(1, 200 / image.w, 160 / image.h) : null;
              const size =
                image.size == null
                  ? '?'
                  : image.size >= 1048576
                    ? (image.size / 1048576).toFixed(1) + ' MB'
                    : image.size >= 1024
                      ? Math.round(image.size / 1024) + ' KB'
                      : image.size + ' bytes';
              return {
                unavailable: !!image.na || !capabilities.attachment,
                url: '/api/session-entry?' + p,
                label:
                  'Attached image ' +
                  (index + 1) +
                  ' (' +
                  (image.type?.replace('image/', '').toUpperCase() ?? 'image') +
                  ', ' +
                  size +
                  ')',
                width: scale && image.w ? Math.max(1, Math.round(scale * image.w)) : undefined,
                height: scale && image.h ? Math.max(1, Math.round(scale * image.h)) : undefined,
              };
            }),
          view.meta?.harness,
          view.key,
          view.meta?.details?.state === 'work' ? view.meta.details.activity : undefined,
        ),
        order: [...new Set(view.store.entries().map((item) => item.entry.turn ?? item.entry_id))],
        before:
          (view.store.loadedRanges()[0]?.first ?? 0) > 0
            ? {
                sid: view.key,
                where: 'before',
                text: 'Load earlier records',
                disabled: view.loading,
                busy: view.loading,
              }
            : undefined,
        empty: view.recovery.stopped
          ? view.note
          : visibleEntries(view).length
            ? undefined
            : view.store.entries().length
              ? 'No loaded records match your search or filter.'
              : 'No native records in this range.',
      },
      {
        committed() {},
        composerOpened(panel) {
          panelOpened(panel);
        },
        composerClosed(panel) {
          panelClosed(panel);
        },
        trace(turn) {
          const url = tracePath(view.sourceKey, view.key, turn) + '?' + sessionControls();
          history.pushState(null, '', url);
          goCausal(view.key, turn);
        },
        session(key, turn) {
          void goSession(key, undefined, true, turn);
        },
        machine() {},
        sender() {},
        toolAll(key, label) {
          openRecordedTool(view, key, label);
        },
        script(key) {
          openRecordedTool(view, key, 'Script', true);
        },
        image(url, label, trigger) {
          sessionPanel?.destroy();
          const image = createImageViewer(url, label, {
            opened() {
              history.pushState({ ...navigation.route, sheet: 1 }, '');
            },
            closed() {
              if (sessionPanel === image) sessionPanel = null;
              if (!disposed && !restoringOverlayHistory && history.state?.sheet) {
                overlayBackPending = true;
                history.back();
              }
              if (active === view && trigger.isConnected) trigger.focus({ preventScroll: true });
            },
          });
          sessionPanel = image;
          image.show();
        },
        background(call) {
          const saved = view.store
            .entries()
            .find((item) => item.entry.k === 'tool' && item.entry.tid === call);
          const target =
            saved &&
            [...view.root.querySelectorAll<HTMLElement>('[data-entry-key]')].find(
              (node) => node.dataset.entryKey === saved.entry_id,
            );
          if (!target) return;
          const group = target.closest('.tgroup');
          const summary = group?.querySelector<HTMLButtonElement>('.tsum[aria-expanded="false"]');
          summary?.click();
          target.scrollIntoView({ block: 'center' });
          target.classList.add('flash');
          scope.timeout(() => target.classList.remove('flash'), 1200);
        },
        jump() {
          jumpLatest(view);
        },
        pager() {
          void loadSelected(view, Math.max(0, view.store.loadedRanges()[0].first - 100));
        },
      },
    );
    measureSessionScreen(view.root);
    const main = document.querySelector<HTMLElement>('#main')!;
    if (view.following) {
      if (view.opening) {
        pinView = view;
        pinUntil = performance.now() + 2000;
        scope.frame(() => {
          if (active === view && pinView === view && performance.now() < pinUntil) {
            scrollEnd();
            scope.frame(() => {
              if (active === view && pinView === view && performance.now() < pinUntil) scrollEnd();
            });
          }
        });
      }
      view.opening = false;
      scrollEnd();
    } else if (anchor?.isConnected && anchorTop !== undefined) {
      const shift = anchor.getBoundingClientRect().top - anchorTop;
      if (window.matchMedia('(max-width: 760px)').matches) window.scrollBy(0, shift);
      else main.scrollTop += shift;
    } else if (view.opening) {
      view.opening = false;
      if (window.matchMedia('(max-width: 760px)').matches)
        window.scrollTo(0, document.documentElement.scrollHeight);
      else main.scrollTop = main.scrollHeight;
    }
    syncJump(view, changedProjection);
    if (view.errorMode)
      scope.frame(() => {
        if (view.annotations?.generation !== annotationGeneration(view)) void readAnnotations(view);
        else highlightAnnotation(view);
      });
  }
  function scrollEnd() {
    if (window.matchMedia('(max-width: 760px)').matches)
      window.scrollTo(0, document.documentElement.scrollHeight);
    else {
      const main = document.querySelector<HTMLElement>('#main')!;
      main.scrollTop = main.scrollHeight;
    }
  }
  function syncJump(view: SelectedView, observeGeometry = false) {
    if (active !== view || disposed) return;
    // Position restoration captures geometry before every redraw. The dock's
    // visibility instead follows scroll/navigation and native content updates,
    // as in the complete reader: expanding local content or accepting unchanged
    // metadata must not turn a later identity redraw into a scroll observation.
    if (observeGeometry) {
      const gap = readingGap(),
        page = view.store.selectedPage(),
        latestEnd = view.store.loadedRanges().at(-1)?.end ?? 0;
      view.jumpVisible = gap > 80 || !!(page && latestEnd < page.projection.total);
      if (gap <= 80) view.newCount = 0;
    }
    updateSessionJump(view.root, view.jumpVisible, view.newCount, false, jumpTarget);
  }
  function jumpLatest(view: SelectedView) {
    if (active !== view || disposed) return;
    view.following = true;
    view.jumpVisible = false;
    view.newCount = 0;
    syncJump(view);
    scrollEnd();
    void loadSelected(view).finally(() => {
      if (active === view && !disposed) scrollEnd();
    });
  }
  function fieldView(view: SelectedView, entry: CatalogTranscriptEntry) {
    if (!entry.clipped || !entry.field) return null;
    const progress = view.fields.get(entry.entry_id),
      generation = view.store.selectedPage()!.projection.generation,
      changed = progress && progress.generation !== generation;
    return {
      text: progress?.text,
      note: changed
        ? 'Source projection changed. Previously loaded text is retained. '
        : progress?.note ||
          (progress?.next === null
            ? 'Complete recorded text loaded.'
            : progress?.text === undefined
              ? 'Recorded text preview. '
              : 'Part of the recorded text is loaded. '),
      action: capabilities.selected_entry
        ? {
            label: progress?.loading
              ? 'Loading text…'
              : changed
                ? 'Reload recorded text'
                : progress?.text === undefined
                  ? 'Load recorded text'
                  : 'Load more text',
            busy: !!progress?.loading,
            run() {
              void loadField(view, entry);
            },
          }
        : undefined,
      ...(progress?.next === null && !changed ? { action: undefined } : {}),
    };
  }
  async function loadField(view: SelectedView, entry: CatalogTranscriptEntry) {
    if (
      disposed ||
      view.sourceKey !== capabilities.source_key ||
      !capabilities.selected_entry ||
      !entry.field
    )
      return;
    const generation = view.store.selectedPage()?.projection.generation;
    if (!generation) return;
    let progress = view.fields.get(entry.entry_id);
    if (progress?.loading) return progress.promise;
    if (!progress || progress.generation !== generation) {
      progress = { generation, next: 0, loading: false, note: '' };
      view.fields.set(entry.entry_id, progress);
    }
    if (progress.next === null) return;
    const chunk = progress.next,
      p = params(),
      pending = progress;
    p.set('sid', view.key);
    p.set('after', String(entry.slot));
    p.set('limit', '1');
    p.set('generation', generation);
    p.set('field_chunk', String(chunk));
    pending.loading = true;
    pending.note = '';
    view.renderedRevision = -1;
    drawSelected(view);
    pending.promise = (async () => {
      try {
        const value = await api('/api/session-entry?' + p);
        if (disposed || view.fields.get(entry.entry_id) !== pending) return;
        if (view.store.selectedPage()?.projection.generation !== generation) {
          pending.note = 'Source projection changed. Reload recorded text. ';
          return;
        }
        const field = parseCatalogField(value, {
          source_key: capabilities.source_key,
          catalog_key: view.key,
          generation,
          entry,
          chunk,
        });
        pending.text = (chunk === 0 ? '' : (pending.text ?? '')) + field.text;
        pending.next = field.next;
      } catch (error) {
        if (disposed) return;
        pending.note =
          error && typeof error === 'object' && 'status' in error && error.status === 422
            ? 'Complete text is unavailable for this native format. '
            : "Couldn't load the full text. " + readError(error) + ' ';
        if (error && typeof error === 'object' && 'status' in error && error.status === 409)
          void loadSelected(view);
      } finally {
        pending.loading = false;
        if (!disposed) {
          view.renderedRevision = -1;
          drawSelected(view);
        }
      }
    })();
    return pending.promise;
  }
  async function resynchronize(view: SelectedView) {
    const previousStore = view.store,
      navigationEpoch = view.navigationEpoch;
    const candidate = new CatalogTranscriptStore();
    candidate.select(
      { source_key: capabilities.source_key, catalog_key: view.key },
      'retained_history',
    );
    const read = async (after: number | null, limit: number) => {
      const ticket = candidate.request(after, limit),
        p = params();
      p.set('sid', view.key);
      p.set('limit', String(limit));
      if (after !== null) p.set('after', String(after));
      if (ticket.generation !== null) p.set('generation', ticket.generation);
      if (ticket.errorsGeneration !== null) p.set('errors_generation', ticket.errorsGeneration);
      candidate.accept(ticket, await api('/api/session-transcript?' + p));
    };
    try {
      await read(null, 60);
      // The new tail already contains current-generation entries. Re-read
      // only the earlier portion of the previously loaded ranges.
      const tailFirst = candidate.selectedPage()!.range.first;
      for (const range of view.store.loadedRanges())
        for (let after = range.first; after < Math.min(range.end, tailFirst); after += 100)
          await read(after, Math.min(100, Math.min(range.end, tailFirst) - after));
      if (disposed || view.store !== previousStore || view.navigationEpoch !== navigationEpoch) {
        candidate.destroy();
        return;
      }
      view.store.destroy();
      view.store = candidate;
      pruneSelectedFields(view);
      view.renderedRevision = -1;
      view.recovery.reset();
      view.meta = {
        ...candidate.selectedPage()!.session,
        ...(view.observedName === undefined ? {} : { name: view.observedName }),
      };
      view.recovery.observeProgress(catalogRecoveryProgress(view.meta));
      view.note = '';
      if (active === view) {
        void loadIdentity(view);
      }
    } catch (error) {
      candidate.destroy();
      throw error;
    }
  }
  function pruneSelectedFields(view: SelectedView) {
    const retainedEntries = new Set(view.store.entries().map((item) => item.entry_id));
    for (const key of view.fields.keys()) if (!retainedEntries.has(key)) view.fields.delete(key);
  }
  async function loadSelected(
    view: SelectedView,
    after: number | null = null,
    preservePosition = false,
  ) {
    if (view.loading && !disposed && capabilities.selected_transcript) {
      if (view.queuedAfter === undefined || after !== null)
        view.queuedAfter = { after, preservePosition };
      return;
    }
    if (
      disposed ||
      view.sourceKey !== capabilities.source_key ||
      view.recovery.stopped ||
      view.retry !== undefined ||
      !capabilities.selected_transcript
    ) {
      drawSelected(view);
      return;
    }
    if (after !== null && !preservePosition) {
      view.following = false;
      view.jumpVisible = true;
    }
    scope.clearTimeout(view.retry);
    const store = view.store,
      request = store.request(after, after === null ? 60 : 100),
      p = params();
    p.set('sid', view.key);
    p.set('limit', String(request.limit));
    if (after !== null) p.set('after', String(after));
    if (request.generation !== null) p.set('generation', request.generation);
    if (request.errorsGeneration !== null) p.set('errors_generation', request.errorsGeneration);
    view.loading = true;
    view.root.setAttribute('aria-busy', 'true');
    view.note = '';
    view.transientNote = false;
    drawSelected(view);
    try {
      const value = await api('/api/session-transcript?' + p);
      if (disposed || view.store !== store) return;
      if (view.store.accept(request, value)) {
        void hydrateContext(view);
        view.recovery.reset();
        const current = view.store.selectedPage()!;
        pruneSelectedFields(view);
        view.meta = {
          ...current.session,
          ...(view.observedName === undefined ? {} : { name: view.observedName }),
        };
        view.recovery.observeProgress(catalogRecoveryProgress(view.meta));
        if (active === view) {
          if (view.errorMode && view.annotations?.generation !== annotationGeneration(view))
            void readAnnotations(view, true);
          void loadIdentity(view);
          chrome();
        }
      }
    } catch (error) {
      if (disposed || view.store !== store) return;
      view.store.reject(request);
      if (error && typeof error === 'object' && 'status' in error && error.status === 409) {
        view.note = 'Recorded history changed. Resynchronizing…';
        try {
          await resynchronize(view);
          return;
        } catch (failure) {
          error = failure;
        }
      }
      view.note = readError(error);
      view.transientNote =
        retryable(error) ||
        !!(error && typeof error === 'object' && 'status' in error && error.status === 409);
      if (view.transientNote) {
        const delay = view.recovery.fail();
        if (delay === null) {
          view.transientNote = false;
          view.note = 'Recorded history could not be loaded. Retry to try again.';
        } else {
          view.retry = scope.timeout(() => {
            view.retry = undefined;
            if (active === view) void loadSelected(view, after, preservePosition);
          }, delay);
        }
      }
    } finally {
      view.loading = false;
      if (!disposed) view.root.setAttribute('aria-busy', 'false');
      drawSelected(view);
      const queued = view.queuedAfter;
      view.queuedAfter = undefined;
      if (!disposed && active === view && queued !== undefined)
        void loadSelected(view, queued.after, queued.preservePosition);
      else if (
        !disposed &&
        active === view &&
        view.store.selectedPage() &&
        view.sourceLabel === undefined
      )
        void observeSelected(view);
    }
  }
  async function observeSelected(view: SelectedView) {
    if (disposed || active !== view || view.loading || view.observing) return;
    const source = view.sourceKey,
      p = params();
    p.set('sid', view.key);
    p.set('limit', '1');
    view.observing = true;
    try {
      const next = parseCatalogPage(await api('/api/sessions?' + p));
      if (disposed || active !== view || capabilities.source_key !== source) return;
      const metadata = next.items[0],
        current = view.store.selectedPage();
      if (!metadata || metadata.key !== view.key || next.machine !== source)
        throw Error('Selected observation changed');
      const recoveryReopened = view.recovery.observeProgress(catalogRecoveryProgress(metadata));
      const recovered = !!view.observationFailure;
      view.observationFailure = undefined;
      const labelChanged = view.sourceLabel !== next.machine_info.label;
      view.sourceLabel = next.machine_info.label;
      const projectionChanged =
        metadata.transcript &&
        (metadata.transcript.generation !== current?.projection.generation ||
          metadata.transcript.total !== current?.projection.total ||
          metadata.transcript.errorsGeneration !== current?.projection.errorsGeneration);
      if (projectionChanged || recoveryReopened) await loadSelected(view);
      else if (
        labelChanged ||
        JSON.stringify(metadata) !== JSON.stringify(view.meta) ||
        recovered
      ) {
        view.meta = metadata;
        view.renderedRevision = -1;
        drawSelected(view);
        chrome();
      }
      // A successful source observation need not publish a new projection.
      // Retry only failed native ranges, using the existing cadence and fences.
      const incomplete = view.store.incompleteRange();
      if (!projectionChanged && incomplete && !view.loading)
        await loadSelected(view, incomplete.first, true);
      void loadIdentity(view);
    } catch (error) {
      if (!disposed && active === view) {
        view.observationFailure = { text: readError(error), retryable: retryable(error) };
        drawSelected(view);
      }
    } finally {
      view.observing = false;
    }
  }
  async function hydrateContext(view: SelectedView) {
    if (disposed || active !== view || view.contextLoading) return;
    const store = view.store,
      generation = store.selectedPage()?.projection.generation,
      context = store.context();
    const cache = (view.contextText ??= new Map());
    const owners = new Map<string, Set<string>>();
    for (const relationship of context.handoffs)
      for (const owner of [relationship.briefOwner, relationship.resultOwner]) {
        if (!owner || !context.previews[owner]) continue;
        const key = JSON.stringify([owner, context.previews[owner], relationship.edge.id]);
        if (cache.has(key)) continue;
        if (!owners.has(owner)) owners.set(owner, new Set());
        owners.get(owner)!.add(relationship.edge.id);
      }
    const activityOwners = new Set<string>();
    for (const relationship of context.handoffs) {
      const target = relationship.edge.to,
        participant = context.sessions[target];
      if (
        relationship.edge.kind !== 'spawn' ||
        participant?.state !== 'work' ||
        !participant.live_observation ||
        !participant.activity ||
        !context.previews[target]
      )
        continue;
      activityOwners.add(target);
    }
    const jobs = [...owners].flatMap(([sid, ids]) => {
      const values = [...ids],
        result: { sid: string; ids: string[]; activity?: boolean }[] = [];
      for (let start = 0; start < values.length; start += 8)
        result.push({ sid, ids: values.slice(start, start + 8) });
      return result;
    });
    for (const sid of activityOwners)
      if (!cache.has(JSON.stringify([sid, context.previews[sid], null])))
        jobs.push({ sid, ids: [], activity: true });
    if (!jobs.length) return;
    view.contextLoading = true;
    const current = () =>
      !disposed &&
      active === view &&
      view.store === store &&
      store.selectedPage()?.projection.generation === generation;
    let pending = 0;
    try {
      await Promise.all(
        Array.from({ length: Math.min(4, jobs.length) }, async () => {
          while (current() && pending < jobs.length) {
            const job = jobs[pending++];
            try {
              const params = new URLSearchParams({
                machine: view.sourceKey,
                scope: 'retained_history',
                sid: job.sid,
                generation: context.previews[job.sid],
                events: JSON.stringify(job.ids),
                session_fields: JSON.stringify(job.activity ? ['activity'] : []),
              });
              const data = object(await api('/api/session-home?' + params));
              if (!current()) return;
              const identity = object(data.identity);
              if (
                data.api !== 1 ||
                identity.catalog_key !== job.sid ||
                identity.source_key !== view.sourceKey ||
                data.generation !== context.previews[job.sid] ||
                typeof data.incomplete !== 'boolean'
              )
                throw Error('Native relationship generation changed');
              const parsed = parseNativeRelationships(data, job.ids);
              if (data.incomplete === true) {
                if (job.ids.length > 1) {
                  const mid = Math.ceil(job.ids.length / 2);
                  jobs.push(
                    { sid: job.sid, ids: job.ids.slice(0, mid) },
                    { sid: job.sid, ids: job.ids.slice(mid) },
                  );
                }
                continue;
              }
              for (const row of parsed)
                cache.set(JSON.stringify([job.sid, context.previews[job.sid], row.id]), row);
              if (job.activity) {
                const activity = object(data.session).activity,
                  expected = context.sessions[job.sid]?.activity;
                if (
                  activity != null &&
                  (!Array.isArray(activity) ||
                    activity.length !== 4 ||
                    typeof activity[0] !== 'string' ||
                    typeof activity[1] !== 'string' ||
                    activity[1].length > 1000 ||
                    activity[0] !== expected?.tool ||
                    activity[3] !== expected?.since)
                )
                  throw Error('Recorded child activity changed');
                cache.set(JSON.stringify([job.sid, context.previews[job.sid], null]), {
                  activity: activity == null ? undefined : [activity[0], activity[1]],
                });
              }
              const allowed = new Set(
                context.handoffs.flatMap((row) =>
                  [row.briefOwner, row.resultOwner]
                    .filter((sid): sid is string => !!sid)
                    .map((sid) => JSON.stringify([sid, context.previews[sid], row.edge.id])),
                ),
              );
              for (const sid of activityOwners)
                allowed.add(JSON.stringify([sid, context.previews[sid], null]));
              for (const key of cache.keys()) if (!allowed.has(key)) cache.delete(key);
              view.renderedRevision = -1;
              drawSelected(view);
            } catch {
              /* Retain the coherent native range; the existing selected refresh retries previews. */
            }
          }
        }),
      );
    } finally {
      view.contextLoading = false;
    }
  }
  async function loadIdentity(view: SelectedView) {
    if (
      disposed ||
      view.sourceKey !== capabilities.source_key ||
      view.identityLoading ||
      !view.ticket ||
      !capabilities.selected_identity
    )
      return;
    const ticket = view.ticket,
      p = currentParams();
    p.set('sid', view.key);
    view.identityLoading = true;
    let invalidated = false;
    try {
      const reply = object(await api('/api/session-identity?' + p));
      if (reply.api !== 1) throw new Error('Unsupported source identity response');
      const previousLabel = sourceLabel(view.sourceKey);
      if (active === view && selection.accept(ticket, reply.identity)) {
        const accepted = selection.selectedIdentity();
        if (!view.recovery.stopped) {
          if (accepted?.owner_qualification === 'provisional')
            view.note = 'Session identity is provisional while history is being discovered.';
          else if (view.note.startsWith('Session identity is provisional')) view.note = '';
        }
        if (previousLabel !== sourceLabel(view.sourceKey)) {
          view.renderedRevision = -1;
          chrome();
        }
        drawSelected(view);
      }
    } catch (error) {
      if (disposed || active !== view || ticket !== view.ticket) return;
      if (
        (error &&
          typeof error === 'object' &&
          'status' in error &&
          [401, 403, 404].includes(Number(error.status))) ||
        (error instanceof Error &&
          error.message === 'Historical identity cannot supply current catalog authority')
      ) {
        invalidated = true;
        view.ticket = selection.begin({
          source_key: capabilities.source_key,
          catalog_key: view.key,
        });
      }
      if (!view.recovery.stopped)
        view.note =
          error && typeof error === 'object' && 'status' in error && error.status === 404
            ? 'Current native session selection is unavailable. Retained history remains readable.'
            : readError(error);
      drawSelected(view);
    } finally {
      view.identityLoading = false;
      if (!disposed && active === view && !invalidated && view.ticket !== ticket)
        scope.timeout(() => void loadIdentity(view), 1000);
    }
  }
  async function goSession(key: string, meta?: CatalogSession, push = true, anchor?: string) {
    if (disposed || !capabilities.source_key) return;
    const navigationFocus = document.activeElement;
    cancelCandidate();
    preserveScroll();
    if (active) closeSessionComposer(active.root);
    if (!active && !sourcesOpen) listFocus = key;
    sourcesOpen = false;
    let view = selected.get(cacheKey(key));
    if (!view) {
      const store = new CatalogTranscriptStore();
      store.select({ source_key: capabilities.source_key, catalog_key: key }, 'retained_history');
      view = {
        sourceKey: capabilities.source_key,
        key,
        root: document.createElement('div'),
        store,
        meta: meta ?? null,
        scroll: 0,
        loading: false,
        note: '',
        opening: true,
        identityLoading: false,
        recovery: new CatalogRecovery(),
        renderedRevision: -1,
        renderedNote: '',
        renderedFooter: '',
        fields: new Map(),
        following: true,
        jumpVisible: false,
        jumpGeneration: null,
        newCount: 0,
        lastTotal: null,
      };
      if (meta) view.recovery.observeProgress(catalogRecoveryProgress(meta));
      view.root.className = 'catalog-session';
      readerResize.observe(view.root);
      selected.set(cacheKey(key), view);
    }
    selected.delete(cacheKey(key));
    selected.set(cacheKey(key), view);
    while (selected.size > 5) {
      const oldest = selected.entries().next().value!;
      selected.delete(oldest[0]);
      scope.clearTimeout(oldest[1].retry);
      readerResize.unobserve(oldest[1].root);
      oldest[1].store.destroy();
      releaseScreen(oldest[1].root);
      render(null, oldest[1].root);
    }
    sessionPanel?.destroy();
    active = view;
    const turn = anchor ?? (push ? undefined : sessionAnchor());
    if (turn) {
      // An anchored reader must not inherit or start the delayed tail pin. The
      // first render happens before its generation-bound range is available.
      view.following = false;
      view.jumpVisible = false;
      view.opening = false;
      pinUntil = 0;
    } else if (push) {
      view.following = true;
      view.jumpVisible = false;
      view.newCount = 0;
      view.opening = true;
    }
    view.errorMode = undefined;
    view.annotationsEpoch = (view.annotationsEpoch ?? 0) + 1;
    view.annotationsLoading = false;
    view.errorRestore = undefined;
    view.findOpen = false;
    view.find = '';
    view.filter = 'all';
    view.renderedRevision = -1;
    for (const node of view.root.querySelectorAll('.err-current'))
      node.classList.remove('err-current');
    sourcesOpen = false;
    view.ticket = selection.begin({ source_key: capabilities.source_key, catalog_key: key });
    overview.leave();
    for (const request of overviewRequests) request.abort();
    overviewRequests.clear();
    navigation.route = {
      v: 'session',
      id: key,
      turn,
    };
    releaseScreen(root);
    render(null, root);
    root.replaceChildren(view.root);
    root.classList.toggle('wide-mode', wide);
    shell.closeDrawer(true);
    if (push)
      history.pushState(
        navigation.route,
        '',
        sessionPath(view.sourceKey, key) +
          '?' +
          sessionControls() +
          (anchor ? '#' + encodeURIComponent(anchor) : ''),
      );
    chrome();
    // The navigation transaction restores its saved position below; the
    // preceding screen's scroll geometry does not belong to this reader.
    drawSelected(view, false);
    const navigationEpoch = (view.navigationEpoch ?? 0) + 1;
    view.navigationEpoch = navigationEpoch;
    const focusHeading = () => {
      if (disposed || active !== view || view.navigationEpoch !== navigationEpoch) return;
      // Match the complete reader's navigation focus without stealing a focus
      // the reader moved while the generation-bound transcript was loading.
      if (document.activeElement !== document.body && document.activeElement !== navigationFocus)
        return;
      const heading = view.root.querySelector<HTMLElement>('.ph > h1');
      if (heading) {
        heading.tabIndex = -1;
        heading.focus({ preventScroll: true });
      }
    };
    focusHeading();
    view.anchorNote = undefined;
    if (turn && view.store.selectedPage()) {
      if (window.matchMedia('(max-width: 760px)').matches) window.scrollTo(0, view.scroll);
      else document.querySelector<HTMLElement>('#main')!.scrollTop = view.scroll;
    }
    if (turn) {
      const sourceKey = view.sourceKey;
      const p = params();
      p.set('sid', key);
      p.set('turn', turn);
      const currentAnchor = () =>
        !disposed &&
        active === view &&
        view.navigationEpoch === navigationEpoch &&
        capabilities.selected_transcript &&
        sourceKey === capabilities.source_key &&
        sessionAnchor() === turn;
      if (!capabilities.selected_transcript) {
        view.anchorNote =
          'Recorded transcript is updating. The source refresh will appear automatically.';
        drawSelected(view);
        return;
      }
      try {
        for (let attempt = 0; attempt < 2; attempt++) {
          const anchor = object(await api('/api/session-anchor?' + p));
          if (!currentAnchor()) return;
          if (
            anchor.api !== 1 ||
            anchor.source_key !== sourceKey ||
            anchor.catalog_key !== key ||
            anchor.turn !== turn ||
            !/^[a-f0-9]{64}$/.test(text(anchor.generation)) ||
            (anchor.slot !== null &&
              (!Number.isSafeInteger(anchor.slot) || Number(anchor.slot) < 0))
          )
            throw Error('Invalid turn anchor');
          if (anchor.slot === null) {
            view.anchorNote = 'This recorded turn is unavailable in the retained history.';
            drawSelected(view);
            break;
          }
          // Accept the anchored range as one replacement transaction. A cold
          // read must bind to the anchor generation too; an old slot is never
          // reinterpreted after a rewrite or silently replaced with the tail.
          const previousStore = view.store,
            sameProjection =
              previousStore.selectedPage()?.projection.generation === anchor.generation,
            retained = attempt === 0 && sameProjection,
            anchored = retained ? previousStore : new CatalogTranscriptStore();
          if (!retained)
            anchored.select({ source_key: sourceKey, catalog_key: key }, 'retained_history');
          const request = anchored.request(Number(anchor.slot), 100, text(anchor.generation)),
            query = params();
          query.set('sid', key);
          query.set('after', String(anchor.slot));
          query.set('limit', String(request.limit));
          query.set('generation', text(anchor.generation));
          if (request.errorsGeneration !== null)
            query.set('errors_generation', request.errorsGeneration);
          try {
            const value = await api('/api/session-transcript?' + query);
            if (!currentAnchor()) {
              if (!retained) anchored.destroy();
              else anchored.reject(request);
              return;
            }
            if (view.store !== previousStore || !capabilities.selected_transcript)
              throw Object.assign(Error('Anchor selection changed'), { status: 409 });
            if (!anchored.accept(request, value)) throw Error('Retired turn anchor');
            if (!retained && sameProjection) {
              // A runtime observation may change without rewriting native
              // content. Revalidate only the already retained bounded ranges
              // under the new observation before committing their replacement.
              for (const range of previousStore.loadedRanges()) {
                for (let first = range.first; first < range.end; first += 100) {
                  const limit = Math.min(100, range.end - first);
                  if (first >= Number(anchor.slot) && first + limit <= Number(anchor.slot) + 100)
                    continue;
                  const retainedRequest = anchored.request(first, limit),
                    retainedQuery = params();
                  retainedQuery.set('sid', key);
                  retainedQuery.set('after', String(first));
                  retainedQuery.set('limit', String(retainedRequest.limit));
                  retainedQuery.set('generation', text(anchor.generation));
                  retainedQuery.set('errors_generation', retainedRequest.errorsGeneration!);
                  if (!currentAnchor()) throw Error('Retired turn anchor');
                  const retainedValue = await api('/api/session-transcript?' + retainedQuery);
                  if (!currentAnchor() || view.store !== previousStore)
                    throw Error('Retired turn anchor');
                  if (!anchored.accept(retainedRequest, retainedValue))
                    throw Error('Retired turn anchor');
                }
              }
              // Keep the requested turn newest in the bounded cache. Reuse the
              // validated response instead of rereading its native range.
              const finalAnchor = anchored.request(
                Number(anchor.slot),
                100,
                text(anchor.generation),
              );
              if (!anchored.accept(finalAnchor, value)) throw Error('Retired turn anchor');
            }
          } catch (error) {
            if (!retained) anchored.destroy();
            else anchored.reject(request);
            // A generation conflict proves this exact anchor needs resolving
            // again. One bounded re-resolution retains the same turn identity.
            if (
              attempt === 0 &&
              currentAnchor() &&
              error &&
              typeof error === 'object' &&
              'status' in error &&
              error.status === 409
            )
              continue;
            throw error;
          }
          if (!retained) {
            view.store.destroy();
            view.store = anchored;
          }
          pruneSelectedFields(view);
          view.meta = {
            ...anchored.selectedPage()!.session,
            ...(view.observedName === undefined ? {} : { name: view.observedName }),
          };
          view.renderedRevision = -1;
          view.note = '';
          chrome();
          drawSelected(view);
          void hydrateContext(view);
          void loadIdentity(view);
          focusHeading();
          if (currentAnchor())
            scope.timeout(() => {
              if (!currentAnchor()) return;
              const target = [...view!.root.querySelectorAll<HTMLElement>('[data-turn]')].find(
                (node) => node.dataset.turn === turn,
              );
              target?.scrollIntoView({ block: 'start' });
              syncJump(view, true);
              target?.classList.add('flash');
              scope.timeout(() => target?.classList.remove('flash'), 1400);
            }, 0);
          return;
        }
      } catch (error) {
        if (currentAnchor()) {
          view.anchorNote = readError(error);
          drawSelected(view);
        }
        return;
      }
      if (!currentAnchor()) return;
    }
    if (view.store.selectedPage()) {
      void loadIdentity(view);
      view.opening = false;
      if (push && !turn) scrollEnd();
      else if (window.matchMedia('(max-width: 760px)').matches) window.scrollTo(0, view.scroll);
      else document.querySelector<HTMLElement>('#main')!.scrollTop = view.scroll;
      syncJump(view, true);
    } else {
      void loadIdentity(view);
      if (capabilities.selected_transcript) await loadSelected(view);
    }
    focusHeading();
  }
  async function refreshMetadata(sourceKey: string, epoch: number) {
    const p = params(),
      filters = countFilters();
    p.set('limit', '60');
    if (harness) p.set('harness', harness);
    if (repo === '__none__') p.set('no_repo', '1');
    else if (repo) p.set('repo', repo);
    if (model) p.set('model', model);
    if (query && capabilities.metadata_search) p.set('q', query);
    const value = parseCatalogPage(await api('/api/sessions?' + p));
    if (
      disposed ||
      epoch !== sourcesEpoch ||
      sourceKey !== capabilities.source_key ||
      value.machine !== sourceKey ||
      value.read_scope !== 'retained_history'
    )
      return;
    const observed = new Map(value.items.map((item) => [item.key, item]));
    const names = new Map(value.items.map((item) => [item.key, item.name]));
    const view = active;
    let activeMetadata = view ? observed.get(view.key) : undefined;
    if (view && !activeMetadata) {
      const selectedParams = params();
      selectedParams.set('sid', view.key);
      const selectedMeta = parseCatalogPage(await api('/api/sessions?' + selectedParams));
      if (
        disposed ||
        epoch !== sourcesEpoch ||
        sourceKey !== capabilities.source_key ||
        selectedMeta.machine !== sourceKey ||
        selectedMeta.read_scope !== 'retained_history'
      )
        return;
      const match = selectedMeta.items.find((item) => item.key === view.key);
      if (match) {
        names.set(match.key, match.name);
        activeMetadata = match;
      }
    }
    if (view && activeMetadata)
      observeSelectedProgress(view, catalogRecoveryProgress(activeMetadata));
    // Rotate bounded exact lookups for loaded history and retained Recent views
    // outside the first page. The active reader is always checked above.
    const remaining = [
      ...new Set([
        ...items.map((item) => item.key),
        ...[...selected.values()]
          .filter((view) => view.sourceKey === sourceKey)
          .map((view) => view.key),
      ]),
    ].filter((key) => !names.has(key));
    for (let i = 0; i < Math.min(8, remaining.length); i++) {
      const key = remaining[(nameRefreshOffset + i) % remaining.length];
      const lookup = params();
      lookup.set('sid', key);
      const metadata = parseCatalogPage(await api('/api/sessions?' + lookup));
      if (
        disposed ||
        epoch !== sourcesEpoch ||
        sourceKey !== capabilities.source_key ||
        metadata.machine !== sourceKey ||
        metadata.read_scope !== 'retained_history'
      )
        return;
      const item = metadata.items.find((item) => item.key === key);
      if (item) names.set(key, item.name);
    }
    if (remaining.length) nameRefreshOffset = (nameRefreshOffset + 8) % remaining.length;
    const before = JSON.stringify(items);
    const previousKeys = new Set(items.map((item) => item.key));
    const refreshed = items.map((item) => observed.get(item.key) ?? item);
    items = [...value.items.filter((item) => !previousKeys.has(item.key)), ...refreshed].slice(
      0,
      300,
    );
    const changedPage = !page || (page.generation !== value.generation && items.length <= 60);
    if (changedPage) {
      page = value;
      countWitness = catalogCountWitness(value, filters, true);
      cursor = value.next_cursor;
    }
    let changed = before !== JSON.stringify(items);
    for (const cached of selected.values()) {
      const name = names.get(cached.key);
      if (
        cached.sourceKey === sourceKey &&
        cached.meta &&
        name !== undefined &&
        cached.meta.name !== name
      ) {
        cached.observedName = name;
        cached.meta = { ...cached.meta, name };
        updateSessionName(cached.root, name);
        changed = true;
      }
    }
    if (changed) {
      if (!active && !sourcesOpen) drawList();
      chrome();
    }
    if (view && active === view && view.recovery.stopped) drawSelected(view);
  }
  async function recheckCapabilities() {
    const observedSource = capabilities.source_key,
      observedEpoch = sourcesEpoch;
    try {
      if (viewerHost?.nativePage) {
        if (viewerHost.nativePage.workspaceChrome) await overview.refresh();
        return;
      }
      if (
        navigation.route.v === 'host' ||
        navigation.route.v === 'home' ||
        navigation.route.v === 'analytics' ||
        navigation.route.v === 'machines' ||
        navigation.route.v === 'machine'
      ) {
        await overview.refresh();
        return;
      }
      if (navigation.route.v === 'trace') {
        void overview.refresh();
        await loadCausal();
        return;
      }
      void overview.refresh();
      if (!capabilities.source_key && !capabilities.global_union) return;
      const next = parseCatalogCapabilities(
        await api('/api/session-capabilities?' + currentParams()),
      );
      if (disposed || observedEpoch !== sourcesEpoch || observedSource !== capabilities.source_key)
        return;
      if (next.source_key !== capabilities.source_key)
        throw new Error('Source selection changed. Choose the source again.');
      const ready = !capabilities.selected_transcript && next.selected_transcript;
      Object.assign(capabilities, next);
      if (next.source_candidates && !candidateLoaded && !active && !sourcesOpen)
        void loadCandidates(false);
      if (active) void loadIdentity(active);
      if (ready && active && !active.store.selectedPage()) void loadSelected(active);
      if (capabilities.global_union) {
        if (!active && !sourcesOpen && items.length <= 60) await loadList(false);
      } else await refreshMetadata(observedSource, observedEpoch);
    } catch (error) {
      if (disposed || observedEpoch !== sourcesEpoch || observedSource !== capabilities.source_key)
        return;
      if (active && !active.store.selectedPage()) {
        active.note = readError(error);
        drawSelected(active);
      }
    } finally {
      if (!disposed) scope.timeout(() => void recheckCapabilities(), 8000);
    }
  }
  function drawSources() {
    if (disposed || !sourcesOpen) return;
    chrome();
    renderCatalogSources(
      root,
      {
        items: sourceItems,
        updating: sourcesUpdating,
        note: sourcesNote,
        more: sourceCursor !== null,
        machinesHref: machinesHref(),
        bookmarks: bookmarkMatches.map((item) => {
          const route = parseSessionRoute(location.pathname, location.search);
          return {
            ...item,
            href: canonicalSessionRoute(route, item.source_key, item.catalog_key) + location.hash,
          };
        }),
      },
      {
        select(key) {
          if (sourceItems.some((item) => item.source_key === key)) void switchSource(key);
        },
        more() {
          void loadSources(true);
        },
        retry() {
          void loadSources(false);
        },
      },
    );
  }
  async function loadSources(append: boolean) {
    if (disposed || sourcesUpdating) return;
    const epoch = ++sourcesEpoch,
      p = new URLSearchParams({ limit: '60' });
    if (append && sourceCursor !== null) p.set('cursor', sourceCursor);
    sourcesUpdating = true;
    sourcesNote = '';
    drawSources();
    try {
      const next = parseCatalogSources(await api('/api/session-sources?' + p));
      if (disposed || epoch !== sourcesEpoch) return;
      if (
        append &&
        next.items.some((item) => sourceItems.some((old) => old.source_key === item.source_key))
      )
        throw new Error('Source page repeated a machine');
      sourceItems = append ? [...sourceItems, ...next.items] : next.items;
      sourceCursor = next.next_cursor;
      sourcesRetryDelay = 1000;
      // Standalone readers discover source authority before scoped reads.
      // A sole source preserves the single-machine landing behavior; several
      // sources retain the existing picker until the user chooses one.
      if (
        !viewerHost?.catalogSources &&
        !capabilities.source_key &&
        sourcesOpen &&
        navigation.route.v === 'sessions' &&
        sourceCursor === null &&
        sourceItems.length === 1 &&
        new URLSearchParams(location.search).get('choose_source') !== '1'
      )
        await switchSource(sourceItems[0].source_key, false, true);
    } catch (error) {
      if (disposed || epoch !== sourcesEpoch) return;
      sourcesNote = readError(error);
      if (retryable(error)) {
        scope.timeout(() => {
          if (sourcesOpen && epoch === sourcesEpoch) void loadSources(append);
        }, sourcesRetryDelay);
        sourcesRetryDelay = Math.min(8000, sourcesRetryDelay * 2);
      }
    } finally {
      if (!disposed && epoch === sourcesEpoch) {
        sourcesUpdating = false;
        if (sourcesOpen) drawSources();
        else drawList();
        if (sourceCursor !== null && !sourcesOpen && sourceItems.length < 64)
          void loadSources(true);
      }
    }
  }
  function saveSource() {
    preserveScroll();
    if (!capabilities.source_key && !capabilities.global_union) return;
    sourceViews.delete(capabilities.source_key);
    sourceViews.set(capabilities.source_key, {
      page,
      countWitness,
      items,
      cursor,
      harness,
      repo,
      model,
      query,
      listScroll,
      listFocus,
      selectedKey: active?.key ?? null,
    });
    while (sourceViews.size > 5) sourceViews.delete(sourceViews.keys().next().value!);
  }
  // Source metadata has no transcript bodies and a bounded retained cache.
  async function showSources() {
    if (disposed) return;
    bookmarkMatches = [];
    if (!sourcesOpen) saveSource();
    leaveScreen();
    navigation.route = { v: 'sessions' };
    sourcesOpen = true;
    root.replaceChildren();
    shell.closeDrawer(true);
    drawSources();
    if (!sourceItems.length) await loadSources(false);
  }
  function showBookmarkOutcome(
    message: string,
    matches: { source_key: string; catalog_key: string; harness: string }[],
  ) {
    leaveScreen();
    navigation.route = { v: 'sessions' };
    sourcesOpen = true;
    sourcesUpdating = false;
    bookmarkMatches = matches;
    sourcesNote = message;
    drawSources();
  }
  async function switchSource(key: string, push = true, fromChooser = push) {
    if (disposed) return;
    const epoch = ++sourcesEpoch,
      requestedURL = location.href,
      // Discovery and chooser clicks already started workspace metadata. Direct
      // scoped entry has not, and must perform fromLocation's normal refresh.
      keepOverview =
        fromChooser &&
        sourcesOpen &&
        navigation.route.v === 'sessions' &&
        (location.pathname === '/' || location.pathname === '/sessions');
    sourcesUpdating = true;
    sourcesNote = '';
    drawSources();
    try {
      const next = parseCatalogCapabilities(
        await api('/api/session-capabilities?' + new URLSearchParams(key ? { machine: key } : {})),
      );
      if (disposed || epoch !== sourcesEpoch || location.href !== requestedURL) return;
      if (next.source_key !== key)
        throw new Error('Source capability identity does not match this selection');
      if (!sourcesOpen) saveSource();
      ++listEpoch;
      scope.clearTimeout(listRetry);
      selection.clear();
      Object.assign(capabilities, next);
      const retained = sourceViews.get(key);
      page = retained?.page ?? null;
      countWitness = retained?.countWitness ?? null;
      items = retained?.items ?? [];
      cursor = retained?.cursor ?? null;
      cancelCandidate();
      candidates = [];
      candidateCursor = null;
      candidateLoaded = false;
      candidateNote = '';
      const requested = new URLSearchParams(location.search);
      harness = requested.get('harness') ?? retained?.harness ?? '';
      repo =
        requested.get('no_repo') === '1'
          ? '__none__'
          : (requested.get('repo') ?? retained?.repo ?? '');
      model = requested.get('model') ?? retained?.model ?? '';
      query = requested.get('q') ?? retained?.query ?? '';
      listScroll = retained?.listScroll ?? 0;
      listFocus = retained?.listFocus ?? '';
      updating = false;
      note = '';
      sourcesOpen = false;
      active = null;
      if (!push) {
        // The current URL owns this transition. Re-enter it once without
        // admitting a list before and after fromLocation's own goList call.
        fromLocation(keepOverview);
        chrome();
        if (!sourcesOpen && !updating) void loadList(false);
        return;
      }
      if (retained?.selectedKey) await goSession(retained.selectedKey, undefined, false);
      else goList(false, keepOverview);
      const destination = retained?.selectedKey
        ? sessionPath(key, retained.selectedKey) + '?' + sessionControls()
        : '/sessions?' + browsingParams();
      history.pushState(navigation.route, '', destination);
      chrome();
      if (!updating) void loadList(false);
    } catch (error) {
      if (disposed || epoch !== sourcesEpoch) return;
      sourcesNote = readError(error);
      if (!sourcesOpen) {
        note = sourcesNote;
        updating = false;
        drawList();
      }
      if (retryable(error))
        scope.timeout(() => {
          if (
            !disposed &&
            epoch === sourcesEpoch &&
            location.href === requestedURL &&
            ((location.pathname.startsWith('/session/')
              ? parseSessionRoute(location.pathname, location.search).source
              : (new URLSearchParams(location.search).get('machine') ?? '')) === key ||
              (!viewerHost?.catalogSources &&
                !capabilities.source_key &&
                sourcesOpen &&
                navigation.route.v === 'sessions' &&
                sourceCursor === null &&
                sourceItems.length === 1 &&
                sourceItems[0].source_key === key))
          )
            void switchSource(key, false, keepOverview);
        }, 1000);
    } finally {
      if (!disposed && epoch === sourcesEpoch) {
        sourcesUpdating = false;
        if (sourcesOpen) drawSources();
        else {
          drawList();
          if (viewerHost?.catalogSources && !sourceItems.length) void loadSources(false);
        }
      }
    }
  }
  async function locateBookmark(key: string, sourceKey?: string) {
    if (locating) return;
    locating = true;
    const path = location.pathname,
      search = location.search;
    const route = parseSessionRoute(path, search);
    const legacyMachine = route.legacyHarness ? new URLSearchParams(search).get('machine') : null;
    const selectedSource = route.source ?? sourceKey ?? legacyMachine ?? undefined;
    const lookupRoute = selectedSource ? { ...route, source: selectedSource } : route;
    const current = () => !disposed && location.pathname === path && location.search === search;
    const retry = () =>
      scope.timeout(() => {
        if (current()) void locateBookmark(key, sourceKey);
      }, 1000);
    try {
      const reply = object(
        await api(
          '/api/session-locate?' +
            new URLSearchParams({
              sid: key,
              ...(selectedSource ? { machine: selectedSource } : {}),
            }),
        ),
      );
      if (!current()) return;
      if (
        reply.api !== 1 ||
        !Array.isArray(reply.matches) ||
        !Number.isSafeInteger(reply.unavailable) ||
        Number(reply.unavailable) < 0
      )
        throw Error('Bookmark lookup unavailable');
      const candidates = reply.matches.map((value) => {
        const match = object(value);
        return {
          source_key: text(match.source_key),
          catalog_key: text(match.catalog_key),
          harness: text(match.harness),
        };
      });
      const resolved = resolveSessionBookmark(lookupRoute, candidates);
      if (resolved.outcome === 'valid' && reply.unavailable === 0) {
        const match = resolved.candidate;
        history.replaceState(
          history.state,
          '',
          canonicalSessionRoute(lookupRoute, match.source_key, match.catalog_key) +
            (route.anchor ? '' : location.hash),
        );
        fromLocation();
      } else {
        const matches = (resolved.outcome === 'ambiguous' ? reply.matches : [])
          .slice(0, 100)
          .map((value) => {
            const match = object(value);
            return {
              source_key: text(match.source_key),
              catalog_key: text(match.catalog_key),
              harness: typeof match.harness === 'string' ? match.harness : 'native',
            };
          });
        const message =
          Number(reply.unavailable) > 0
            ? 'The recorded session index is updating. Retrying…'
            : reply.matches.length > 1
              ? 'This bookmark has several recorded origins. Choose a session.'
              : resolved.outcome === 'mismatch'
                ? 'This bookmark does not match the recorded harness.'
                : 'Recorded session not found.';
        showBookmarkOutcome(message, matches);
        if (Number(reply.unavailable) > 0) retry();
      }
    } catch (error) {
      if (current()) {
        showBookmarkOutcome(readError(error), []);
        if (retryable(error)) retry();
      }
    } finally {
      locating = false;
    }
  }
  function fromLocation(keepOverview = false) {
    if (/^\/(session|s|trace)(?:\/|$)/.test(location.pathname)) {
      const hash = canonicalSessionHash(location.hash);
      if (hash !== location.hash)
        history.replaceState(history.state, '', location.pathname + location.search + hash);
    }
    const url = new URL(location.href);
    if (viewerHost?.pages?.match(url)) {
      void showHostPage(url);
      return;
    }
    if (viewerHost?.nativePage) {
      navigation.route = { v: 'machines' };
      chrome();
      if (viewerHost.nativePage.workspaceChrome) void overview.refresh();
      return;
    }
    let sessionRoute: SessionRoute | null = null;
    if (/^\/(session|s|trace)(?:\/|$)/.test(location.pathname)) {
      try {
        sessionRoute = parseSessionRoute(location.pathname, location.search);
      } catch {
        leaveScreen();
        navigation.route = { v: 'sessions' };
        sourcesOpen = false;
        note = 'Malformed session address.';
        goList(false);
        return;
      }
      if (sessionRoute.legacyHarness) {
        showBookmarkOutcome('Resolving recorded session…', []);
        void locateBookmark(sessionRoute.key, sessionRoute.source ?? undefined);
        return;
      }
    }
    if (location.pathname === '/') {
      history.replaceState({ v: 'sessions' }, '', '/sessions' + location.search + location.hash);
    }
    const queryParams = new URLSearchParams(location.search);
    const machineBookmark = location.pathname.match(/^\/(?:m|machines)\/([^/]+)$/);
    if (machineBookmark && !viewerHost?.nativePage) {
      let key: string;
      try {
        key = decodeURIComponent(machineBookmark[1]);
      } catch {
        void resolveMachineBookmark(machineBookmark[1]);
        return;
      }
      if (queryParams.get('machine') !== key) {
        void resolveMachineBookmark(machineBookmark[1]);
        return;
      }
    }
    if (location.pathname === '/machines' && !viewerHost?.nativePage) {
      if (machinesHref() !== '/machines') {
        location.assign(machinesHref());
        return;
      }
      goOverview('machines', false);
      return;
    }
    if (location.pathname === '/analytics' || location.pathname === '/timeline') {
      goOverview('analytics', false);
      return;
    }
    const machine = sessionRoute?.source ?? queryParams.get('machine');
    if (
      queryParams.get('choose_source') === '1' ||
      (viewerHost?.catalogSources && !machine && /^\/(s|trace)\//.test(location.pathname))
    ) {
      void showSources().then(() => {
        const parts = location.pathname.split('/');
        if ((parts[1] === 's' || parts[1] === 'trace') && parts[3])
          void locateBookmark(decodeURIComponent(parts[3]));
      });
      void overview.refresh();
      return;
    }
    if (
      (!capabilities.source_key && !capabilities.global_union) ||
      (viewerHost?.catalogSources
        ? (machine ?? '') !== capabilities.source_key
        : !!machine && machine !== capabilities.source_key)
    ) {
      if (machine) void showSources();
      else if (viewerHost?.catalogSources) {
        leaveScreen();
        sourcesOpen = false;
        navigation.route = { v: 'sessions' };
        updating = true;
        drawList();
      } else {
        leaveScreen();
        navigation.route = { v: 'sessions' };
        sourcesOpen = true;
        drawSources();
      }
      if (machine || viewerHost?.catalogSources) void switchSource(machine ?? '', false);
      else {
        void showSources();
        void overview.refresh();
      }
      return;
    }
    const beforeFilters = countFilters();
    harness = queryParams.get('harness') ?? '';
    repo = queryParams.get('no_repo') === '1' ? '__none__' : (queryParams.get('repo') ?? '');
    model = queryParams.get('model') ?? '';
    query = queryParams.get('q') ?? '';
    if (beforeFilters !== countFilters()) {
      ++listEpoch;
      page = null;
      countWitness = null;
      items = [];
      cursor = null;
      updating = false;
    }
    if (!sourcesOpen && viewerHost?.catalogSources && !sourceItems.length) void loadSources(false);
    if (sessionRoute?.turn) {
      goCausal(sessionRoute.key, sessionRoute.turn);
    } else if (sessionRoute) {
      void goSession(sessionRoute.key, undefined, false);
    } else if (machineBookmark) {
      leaveScreen();
      navigation.route = { v: 'machine', id: capabilities.source_key };
      shell.closeDrawer(true);
      overview.draw();
      void overview.refresh(true);
    } else goList(false, keepOverview);
    if (!machineBookmark && !keepOverview) void overview.refresh();
  }
  scope.listen(window, 'popstate', () => {
    if (overlayBackPending) {
      overlayBackPending = false;
      const action = afterOverlayClose;
      afterOverlayClose = null;
      if (!disposed) action?.();
      return;
    }
    restoringOverlayHistory = true;
    try {
      if (sessionPanel && !history.state?.sheet) {
        sessionPanel.destroy();
        if (
          history.state?.v === navigation.route.v &&
          history.state?.sid === ('sid' in navigation.route ? navigation.route.sid : undefined) &&
          history.state?.id === ('id' in navigation.route ? navigation.route.id : undefined) &&
          history.state?.turn ===
            ('turn' in navigation.route ? navigation.route.turn : undefined) &&
          (!active ||
            (location.pathname.startsWith('/session/')
              ? parseSessionRoute(location.pathname, location.search).source
              : new URLSearchParams(location.search).get('machine')) === active.sourceKey)
        )
          return;
      }
      if (shell.account.open && !history.state?.sheet) {
        shell.account.close();
        return;
      }
      if (shell.drawerOpen && !history.state?.drawer) {
        shell.closeDrawer(true);
        return;
      }
      fromLocation();
    } finally {
      restoringOverlayHistory = false;
    }
  });
  async function resolveMachineBookmark(encoded: string) {
    const path = location.pathname;
    await showSources();
    if (disposed || path !== location.pathname) return;
    let key: string;
    try {
      key = decodeURIComponent(encoded);
    } catch {
      sourcesNote = 'This machine bookmark is invalid.';
      drawSources();
      return;
    }
    const exact = sourceItems.find((source) => source.source_key === key);
    const matches = exact ? [exact] : sourceItems.filter((source) => source.label === key);
    if (matches.length === 1 && (exact || !sourceCursor)) {
      const url = new URL(location.href);
      url.pathname = '/machines/' + encodeURIComponent(matches[0].source_key);
      url.searchParams.set('machine', matches[0].source_key);
      url.searchParams.delete('choose_source');
      history.replaceState(history.state, '', url.pathname + url.search + url.hash);
      fromLocation();
    } else {
      sourcesNote =
        matches.length > 1
          ? 'Several machines share this name. Choose the source for this bookmark.'
          : 'Choose the source for this machine bookmark.';
      drawSources();
    }
  }
  function readingGap() {
    const main = document.querySelector<HTMLElement>('#main')!;
    return window.matchMedia('(max-width: 760px)').matches
      ? document.documentElement.scrollHeight - window.scrollY - window.innerHeight
      : main.scrollHeight - main.scrollTop - main.clientHeight;
  }
  const readingScroll = () => {
    if (!active || active.opening || !active.store.selectedPage()) return;
    const gap = readingGap();
    // Content-visibility can adjust a scroll offset after layout has settled,
    // without changing the observed boxes. Keep the initial tail transaction
    // pinned until it settles; reader input cancels it synchronously above.
    if (
      active === pinView &&
      performance.now() < pinUntil &&
      !active.errorMode &&
      gap > 1 &&
      gap <= 80
    ) {
      active.following = true;
      scrollEnd();
      return;
    }
    active.following = gap <= 80;
    if (!active.following) pinUntil = 0;
    if (active.following) active.newCount = 0;
    syncJump(active, true);
  };
  scope.listen(window, 'scroll', readingScroll, { passive: true });
  scope.listen(document.querySelector<HTMLElement>('#main')!, 'scroll', readingScroll, {
    passive: true,
  });
  scope.listen(document, 'keydown', (event) => {
    if (
      ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(event.key) &&
      !(event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement)
    )
      pinUntil = 0;
    if (
      active?.errorMode &&
      !sessionPanel &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !(event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) &&
      ['n', 'p', 'Enter'].includes(event.key)
    ) {
      event.preventDefault();
      const direction = event.key === 'p' || (event.key === 'Enter' && event.shiftKey) ? -1 : 1;
      const count = active.annotations?.total ?? 0;
      if (count) {
        active.errorIndex = ((active.errorIndex ?? 0) + direction + count) % count;
        void readAnnotations(active);
        chrome();
      }
    }
    if (event.key === 'Escape') {
      if (sessionPanel) return;
      if (active?.errorMode) {
        closeAnnotations(active);
      } else if (active?.findOpen) {
        active.findOpen = false;
        active.find = '';
        active.filter = 'all';
        active.renderedRevision = -1;
        drawSelected(active);
        chrome();
        document.querySelector<HTMLButtonElement>('#find-btn')?.focus();
      } else if (shell.account.open) shell.account.escape();
      else shell.closeDrawer();
    }
  });
  scope.interval(() => {
    if (active?.meta?.details?.live_observation) updateSessionClock(active.root, Date.now(), {});
  }, 1000);
  const removeNavigation = viewerHost?.subscribeNavigation?.(chrome);
  if (!viewerHost)
    void api('/api/viewer/chrome')
      .then((value) => {
        if (disposed) return;
        const data = object(value);
        if (data.api !== 1) throw Error('Unsupported chrome metadata');
        account = parseAccount(data.account);
        standaloneNavigation = parseShellNavigation(data.navigation) ?? undefined;
        if (data.admin != null) {
          const link = object(data.admin),
            label = text(link.label),
            href = text(link.href);
          if (
            !label.trim() ||
            label.length > 80 ||
            /[\x00-\x1f\x7f]/.test(label) ||
            !safePath(href)
          )
            throw Error('Invalid machine management link');
          standaloneAdmin = { label, href };
        }
        if (location.pathname === '/machines' && machinesHref() !== '/machines') {
          fromLocation();
          return;
        }
        chrome();
        if (navigation.route.v === 'machines') overview.draw();
      })
      .catch(() => {});
  fromLocation();
  scope.timeout(() => void recheckCapabilities(), 8000);
  scope.interval(() => {
    if (
      active &&
      capabilities.selected_transcript &&
      active.store.selectedPage() &&
      document.visibilityState === 'visible'
    )
      void observeSelected(active);
  }, 3000);
  return {
    destroy() {
      if (disposed) return;
      disposed = true;
      afterOverlayClose = null;
      if (!viewerHost?.nativePage && history.scrollRestoration === 'manual')
        history.scrollRestoration = previousScrollRestoration;
      ++listEpoch;
      removeRuntimeListener();
      runtimeObservation.destroy();
      removeNavigation?.();
      scope.destroy();
      readerResize.disconnect();
      sessionPanel?.destroy();
      sessionBar.destroy();
      ++hostEpoch;
      hostRequest?.abort();
      hostPage?.destroy();
      initialHostPage?.destroy();
      viewerHost?.initialMachines?.destroy();
      viewerHost?.initialHome?.destroy();
      selection.destroy();
      navigation.destroy();
      for (const view of selected.values()) {
        view.store.destroy();
        releaseScreen(view.root);
        render(null, view.root);
      }
      overview.destroy();
      catalogBrowser.destroy();
      releaseGeometry(shell.slots.recent, ['scrollPaddingTop']);
      recent.destroy();
      releaseScreen(root);
      render(null, root);
      shell.destroy();
      selected.clear();
    },
  };
}

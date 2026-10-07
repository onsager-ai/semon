import type { ViewerModelStore } from '../state/model';
import type { TranscriptStore } from '../state/transcript';
import type { EffectScope } from '../app/effects';
import type { NavigationController } from '../navigation/routes';
import type { createPaging } from './paging';
import type { createHistoryScroll } from './historyScroll';
import type { createToolViews } from './toolViews';
import type { createSessionChrome } from './sessionChrome';
import type { createAccountControls } from './accountControls';
import type { createTransport } from './transport';
import type { createScreenViews } from './screenViews';
import type { createNavigationView } from './navigationView';
import type { createRecentNavigation } from './recentNavigation';
import type { createTicker } from './ticker';
import type { createLiveModel } from './liveModel';
import type { createDestination } from './destination';
import { updateSessionJump } from '../lib';
import { createScrollTransactions } from '../navigation/scroll';
interface ViewportHost {
  phone: MediaQueryList;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  navigation: NavigationController;
  scope: EffectScope;
  sidebarOnly: boolean;

  destination: Pick<ReturnType<typeof createDestination>, 'goSession'>;

  transcripts: Pick<TranscriptStore, 'meta'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  tickerOwner: Pick<ReturnType<typeof createTicker>, 'ticker'>;

  recentNavigation: Pick<ReturnType<typeof createRecentNavigation>, 'renderLanes'>;

  navigationViewOwner: Pick<ReturnType<typeof createNavigationView>, 'renderNav'>;

  screenViews: Pick<ReturnType<typeof createScreenViews>, 'renderSession'>;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'turn'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'tick' | 'fetchTx'>;

  accountControlsOwner: Pick<ReturnType<typeof createAccountControls>, 'shellChrome'>;

  sessionChrome: Pick<
    ReturnType<typeof createSessionChrome>,
    'syncBarLine' | 'drawSessionBar' | 'viewerBar'
  >;

  toolViewsOwner: Pick<ReturnType<typeof createToolViews>, 'viewerEl'>;

  historyScrollOwner: Pick<ReturnType<typeof createHistoryScroll>, 'saveHistoryScroll'>;

  pagingOwner: Pick<
    ReturnType<typeof createPaging>,
    | 'queuePagerObservers'
    | 'scrollProgrammatically'
    | 'resetPagerInput'
    | 'disconnectPagerObservers'
    | 'scrollRevision'
    | 'holdProgrammaticScroll'
    | 'paintPager'
    | 'readerScrollInput'
    | 'programmaticScrollPending'
  >;
}
/** Owns viewport behavior through explicit application ports. */
export function createViewport(host: ViewportHost) {
  const scroller = () => (host.phone.matches ? document.documentElement : host.$('#main'));
  const edge = () => host.$('#topbar').getBoundingClientRect().bottom;
  // The opened transcript can still grow as fonts and clamped cards settle; hold the tail briefly, then yield on reader input.
  let openingEndUntil = 0;
  let openingEndTimer: number | undefined;
  let openingEndObserver: ResizeObserver | null = null;
  function stopOpeningEndPin() {
    const wasPinned = !!openingEndUntil;
    openingEndUntil = 0;
    host.scope.clearTimeout(openingEndTimer);
    openingEndTimer = undefined;
    openingEndObserver?.disconnect();
    openingEndObserver = null;
    if (wasPinned) host.pagingOwner.queuePagerObservers();
  }
  function pinOpeningEnd() {
    if (host.navigation.route.v !== 'session' || performance.now() >= openingEndUntil) {
      stopOpeningEndPin();
      return;
    }
    const sc = scroller();
    host.pagingOwner.scrollProgrammatically(() => {
      sc.scrollTop = sc.scrollHeight;
    });
    scrollController.anchor = null;
    syncJump();
    host.historyScrollOwner.saveHistoryScroll();
  }
  function startOpeningEndPin() {
    host.pagingOwner.resetPagerInput();
    stopOpeningEndPin();
    if (host.navigation.route.v !== 'session' || location.hash) return;
    openingEndUntil = performance.now() + 2000;
    host.pagingOwner.disconnectPagerObservers();
    const turns = host.$("#page section[aria-label='Transcript'] .turns");
    if (turns) {
      openingEndObserver = new ResizeObserver(pinOpeningEnd);
      openingEndObserver.observe(turns);
    }
    pinOpeningEnd();
    openingEndTimer = host.scope.timeout(stopOpeningEndPin, 2000);
  }
  const scrollController = createScrollTransactions({
    page: () => host.$('#page'),
    main: () => host.$('#main'),
    phone: () => host.phone.matches,
    edge,
    rendered: () => host.navigation.rendered,
    sheet: () => !!host.toolViewsOwner.viewerEl,
    revision: () => host.pagingOwner.scrollRevision,
    programmatic: (...args: Parameters<typeof host.pagingOwner.scrollProgrammatically>) =>
      host.pagingOwner.scrollProgrammatically(...args),
    sync: (...args: Parameters<typeof host.sessionChrome.syncBarLine>) =>
      host.sessionChrome.syncBarLine(...args),
    restoreDrawer: () => host.accountControlsOwner.shellChrome?.restoreDrawer(),
  });
  const { capture, restore, opener, stateKey, identOf } = scrollController;
  // A session page in place: its turns are drawn again and only those that changed (or are new) replace the ones shown, so
  // the rest keep their nodes and state. The bar's summary line, the title and the sidebar follow. Returns how many entries
  // are new.
  function patchSession(dirty: ReadonlySet<string> | null) {
    host.pagingOwner.resetPagerInput();
    host.pagingOwner.holdProgrammaticScroll();
    host.transportOwner.tick();
    const s =
        host.modelStore.sessions['id' in host.navigation.route ? host.navigation.route.id : ''],
      box = host.$('#page .turns');
    const keys = () =>
      new Set(
        [
          ...host
            .$('#page')
            .querySelectorAll<HTMLElement>(
              '.turns :is(.msg, .bubble, .step, .event, .child-card, .thought, .think-pending)[data-e]',
            ),
        ].map((n) => n.dataset.e),
      );
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole =
      !dirty ||
      box.querySelector(':scope > p.empty') ||
      [...box.querySelectorAll<HTMLElement>(':scope > .turn')].some(
        (b) => !host.modelStore.turn.has(b.dataset.turn ?? ''),
      );
    host.screenViews.renderSession(
      host.$('#page'),
      'id' in host.navigation.route ? host.navigation.route.id : '',
      whole ? {} : { only: dirty },
    );
    host.sessionChrome.drawSessionBar();
    for (const pager of box.querySelectorAll<HTMLElement>('[data-pager-where]'))
      host.pagingOwner.paintPager(pager);
    host.navigationViewOwner.renderNav();
    host.recentNavigation.renderLanes();
    host.tickerOwner.ticker();
    let n = 0;
    for (const k of keys()) if (!before.has(k)) n++;
    host.pagingOwner.queuePagerObservers();
    return n;
  }
  // Jump to the latest: centred at the transcript column's foot, sticky, with the count of what arrived while the reader was away.
  function scrollMetrics() {
    if (host.phone.matches)
      return {
        top: window.scrollY,
        height: document.documentElement.scrollHeight,
        viewport: window.innerHeight,
        gap: Math.max(
          0,
          document.documentElement.scrollHeight - window.innerHeight - window.scrollY,
        ),
      };
    const m = host.$('#main');
    return {
      top: m.scrollTop,
      height: m.scrollHeight,
      viewport: m.clientHeight,
      gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop),
    };
  }
  function scrollToEnd(behavior: ScrollBehavior = 'smooth') {
    host.pagingOwner.scrollProgrammatically(() => {
      if (host.phone.matches)
        window.scrollTo({ top: document.documentElement.scrollHeight, behavior });
      else {
        const m = host.$('#main');
        m.scrollTo({ top: m.scrollHeight, behavior });
      }
    });
  }
  // The button is rebuilt only when what it shows changes (hidden or not, and the new-entry count), not on every scroll.
  let jumpBusy = false;
  function syncJump() {
    if (host.navigation.route.v !== 'session') {
      host.liveModelOwner.LIVE.fresh = 0;
      return;
    }
    const { gap } = scrollMetrics(),
      newer =
        host.transcripts.meta['id' in host.navigation.route ? host.navigation.route.id : '']
          ?.newer ?? 0;
    if (gap <= 80) host.liveModelOwner.LIVE.fresh = 0;
    updateSessionJump(
      host.$('#page'),
      gap > 80 || !!newer,
      host.liveModelOwner.LIVE.fresh + newer,
      jumpBusy,
      host.sessionChrome.viewerBar.jumpTarget,
    );
  }
  function clearNewEntries() {
    host.liveModelOwner.LIVE.fresh = 0;
    updateSessionJump(host.$('#page'), false, 0, jumpBusy);
  }
  function jumpToLatest() {
    const sid = 'id' in host.navigation.route ? host.navigation.route.id : '',
      r = host.navigation.route,
      m = host.transcripts.meta[sid];
    if (m && m.to < m.total) {
      jumpBusy = true;
      syncJump();
      host.transportOwner
        .fetchTx(sid, '')
        .then(() => {
          if (host.navigation.route === r) host.destination.goSession(sid);
        })
        .catch(() => {})
        .finally(() => {
          jumpBusy = false;
          syncJump();
        });
    } else scrollToEnd('smooth');
  }
  if (!host.sidebarOnly) {
    host.scope.listen(window, 'scroll', syncJump, { passive: true });
    host.scope.listen(host.$('#main'), 'scroll', syncJump, { passive: true });
  }
  const cancelOpeningEndPin = () => {
    if (openingEndUntil) stopOpeningEndPin();
  };
  if (!host.sidebarOnly) {
    host.scope.listen(window, 'wheel', cancelOpeningEndPin, { passive: true });
    host.scope.listen(window, 'touchmove', cancelOpeningEndPin, { passive: true });
    host.scope.listen(window, 'pointerdown', cancelOpeningEndPin, { passive: true });
  } // a press anywhere, a scrollbar drag included
  const transcriptInput = (e: Event) =>
    e.target instanceof Element &&
    !e.target.closest?.("#sidebar, dialog, input, textarea, select, [contenteditable='true']") &&
    (host.phone.matches || host.$('#main').contains(e.target));
  if (!host.sidebarOnly) {
    const input = (e: Event) => {
      if (transcriptInput(e)) host.pagingOwner.readerScrollInput();
    };
    host.scope.listen(window, 'wheel', input, { passive: true });
    host.scope.listen(window, 'touchmove', input, { passive: true });
    const scroll = () => {
      if (host.pagingOwner.programmaticScrollPending) host.pagingOwner.holdProgrammaticScroll();
      else if (!openingEndUntil) host.pagingOwner.readerScrollInput();
    };
    host.scope.listen(
      window,
      'scroll',
      () => {
        if (host.phone.matches) scroll();
      },
      { passive: true },
    );
    host.scope.listen(
      host.$('#main'),
      'scroll',
      () => {
        if (!host.phone.matches) scroll();
      },
      { passive: true },
    );
  }
  if (!host.sidebarOnly)
    host.scope.listen(document, 'keydown', (e) => {
      if (
        !e.defaultPrevented &&
        (transcriptInput(e) ||
          e.target === document.body ||
          e.target === document.documentElement) &&
        ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(e.key)
      )
        host.pagingOwner.readerScrollInput();
    });

  return {
    scrollController,
    stopOpeningEndPin,
    get openingEndUntil(): number {
      return openingEndUntil;
    },
    scroller,
    capture,
    restore,
    syncJump,
    startOpeningEndPin,
    clearNewEntries,
    edge,
    opener,
    jumpToLatest,
    patchSession,
  };
}

import type { ViewerModelStore } from '../state/model';
import type { TranscriptStore } from '../state/transcript';
import type { Entry, TranscriptMeta } from '../domain/types';
import type { ViewerHost } from '../viewer-host';
import type { NavigationController, HostFocus } from '../navigation/routes';
import type { EffectScope } from '../app/effects';
import type { createViewport } from './viewport';
import type { createNavigationView } from './navigationView';
import type { createRecentNavigation } from './recentNavigation';
import type { createSessionChrome } from './sessionChrome';
import type { createLiveModel } from './liveModel';
import type { createHistoryScroll } from './historyScroll';
import type { createBootstrap } from './bootstrap';
import type { createDocumentEvents } from './documentEvents';
import type { createAccountControls } from './accountControls';
import type { createPaging } from './paging';
import type { createDomain } from '../domain/calculations';
import type { createOrderingControls } from './orderingControls';
import type { createDocumentRenderer } from './documentRenderer';
import type { createTransport } from './transport';
import type { createTranscriptRevalidation } from './transcriptRevalidation';
import type { createToolViews } from './toolViews';
import type { Handoff } from '../domain/types';
import { renderPlaceholder, revealMeasuredTurn, setGeometry } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
import type { ViewerContent } from '../viewer-host';
interface DestinationHost {
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  scope: EffectScope;
  navigation: NavigationController;
  nativePage: { title: string; nav: string } | undefined;
  machinesPath: string | null;
  viewerHost: ViewerHost | null;
  focusSessionsSearchOnRender: ApplicationRoute | null;
  sidebarOnly: boolean;
  cacheTx: (sid: string, entries: Entry[], meta: TranscriptMeta) => void;
  show: { messages: boolean; tools: boolean; thinking: boolean };
  allVisibility: { messages: boolean; tools: boolean; thinking: boolean };
  find: string;
  findOpen: boolean;
  adoptCached: (
    r: { v: 'session'; id: string; turn?: string | undefined } & {
      scrollTop?: number | undefined;
      hostFocus?: HostFocus | undefined;
      q?: string | undefined;
      sheet?: number | undefined;
    },
  ) => boolean;
  afterPop: (() => void) | null;
  pendingSessionOpen: string | null;
  accountSheet: boolean;
  phone: MediaQueryList;

  toolViewsOwner: Pick<ReturnType<typeof createToolViews>, 'skipPop' | 'viewerEl'>;

  transcriptRevalidationOwner: Pick<ReturnType<typeof createTranscriptRevalidation>, 'revalidate'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'spread' | 'load'>;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  orderingControlsOwner: Pick<ReturnType<typeof createOrderingControls>, 'ORD'>;

  domain: Pick<ReturnType<typeof createDomain>, 'originHandoff'>;

  transcripts: Pick<TranscriptStore, 'entries' | 'meta' | 'staleBriefs' | 'cache'>;

  pagingOwner: Pick<
    ReturnType<typeof createPaging>,
    'clearPaging' | 'resetPagerInput' | 'scrollProgrammatically'
  >;

  accountControlsOwner: Pick<ReturnType<typeof createAccountControls>, 'closeAccountMenu'>;

  documentEventsOwner: Pick<ReturnType<typeof createDocumentEvents>, 'closeDrawer'>;

  bootstrapOwner: Pick<ReturnType<typeof createBootstrap>, 'urlOf' | 'routeOf'>;

  modelStore: Pick<ViewerModelStore, 'turn' | 'sessions' | 'holds'>;

  historyScrollOwner: Pick<
    ReturnType<typeof createHistoryScroll>,
    'quietTop' | 'saveHistoryScroll' | 'restoreScroll' | 'restoreHostFocus'
  >;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  sessionChrome: Pick<
    ReturnType<typeof createSessionChrome>,
    'drawSessionBar' | 'syncBarLine' | 'dropErrors'
  >;

  recentNavigation: Pick<ReturnType<typeof createRecentNavigation>, 'renderLanes'>;

  navigationViewOwner: Pick<ReturnType<typeof createNavigationView>, 'renderNav'>;

  viewport: Pick<
    ReturnType<typeof createViewport>,
    'startOpeningEndPin' | 'stopOpeningEndPin' | 'clearNewEntries' | 'syncJump'
  >;
}
/** Owns destination behavior through explicit application ports. */
export function createDestination(host: DestinationHost) {
  function openSessionAtEnd() {
    if (location.hash) return;
    host.viewport.startOpeningEndPin();
  }
  // Opening a session draws what the model already holds at once, before its transcript arrives: the sidebar row, the top bar,
  // and the old page held dimmed and inert (aria-busy). If the transcript is still on its way after 150 ms, a skeleton of turn-shaped
  // placeholders stands in for the old page, so fast switches don't flash it. `navAbort` cancels the transcript request of a
  // navigation the reader has left; a response that still arrives late is dropped because its route is no longer the current one.
  const SKELETON_MS = 150;
  // The sidebar's list was drawn for this route under this model version: the render that follows draws it only if either changed.
  let skeletonTimer: number | undefined;
  let lanesFor: { r: ApplicationRoute; version: string | null } | null = null;
  // The turn shapes the skeleton cycles through: a bubble (yours), text lines, a step row. Widths are classes, sk-w1..sk-w5, in percent.
  function paintPending(r: Extract<ApplicationRoute, { v: 'session' }>) {
    const page = host.$('#page'),
      hadFocus = host.$('#sidebar').contains(document.activeElement);
    host.navigationViewOwner.renderNav();
    host.recentNavigation.renderLanes();
    host.sessionChrome.drawSessionBar();
    lanesFor = { r, version: host.liveModelOwner.LIVE.version };
    if (hadFocus)
      host.$("#lanes .srow[data-id='" + CSS.escape(r.id) + "']")?.focus({ preventScroll: true });
    page.setAttribute('aria-busy', 'true');
    page.inert = true;
    page.classList.add('loading');
    host.scope.clearTimeout(skeletonTimer);
    skeletonTimer = host.scope.timeout(() => {
      if (host.navigation.route !== r || !page.classList.contains('loading')) return;
      page.classList.remove('loading');
      page.classList.remove('child-page');
      setGeometry(page, 'paddingBottom', null);
      renderPlaceholder(page);
      host.historyScrollOwner.quietTop();
      host.sessionChrome.syncBarLine();
    }, SKELETON_MS);
  }
  function endLoading() {
    host.scope.clearTimeout(skeletonTimer);
    skeletonTimer = undefined;
    const page = host.$('#page');
    page.removeAttribute('aria-busy');
    page.inert = false;
    page.classList.remove('loading');
  }
  // Once the transcript is in, a reader who hasn't put focus anywhere else on the page moves to the session's title.
  function focusTitle() {
    const a = document.activeElement;
    if (a && a !== document.body && !host.$('#sidebar').contains(a)) return;
    const h = host.$('#page .ph h1');
    if (h) {
      h.tabIndex = -1;
      h.focus({ preventScroll: true });
    }
  }
  // The session's own transcript couldn't be loaded: the skeleton gives way to the reason and a way to try again.
  function failLoad(r: ApplicationRoute, err: unknown) {
    if (host.navigation.route !== r || (err instanceof Error && err.name === 'AbortError')) return;
    endLoading();
    const page = host.$('#page');
    page.classList.remove('child-page');
    setGeometry(page, 'paddingBottom', null);
    renderPlaceholder(
      page,
      "Couldn't load this session: " + (err instanceof Error ? err.message : 'no response'),
      () => go({ ...r }, true),
    );
  }
  // A deep link to a turn the loaded transcript doesn't hold yet.
  const isDeep = (r: Extract<ApplicationRoute, { v: 'session' }>) => {
    const t = r.turn ? host.modelStore.turn.get(r.turn) : null;
    return !!t && t.sid === r.id && !t.entries.length;
  };
  function go(
    r: ApplicationRoute,
    fromHistory: boolean = false,
    prepared: boolean = false,
    nextContent: ViewerContent | null = null,
  ) {
    host.navigation.cancelNative();
    if (host.nativePage) {
      if (!fromHistory)
        location.assign(
          r.v === 'machines'
            ? (host.machinesPath ?? host.bootstrapOwner.urlOf(r))
            : host.bootstrapOwner.urlOf(r),
        );
      return;
    }
    if (r.v === 'machines' && host.viewerHost && !prepared) {
      host.documentEventsOwner.closeDrawer(true);
      host.accountControlsOwner.closeAccountMenu(true, true);
      host.navigation.loadNative(
        r,
        (content) => go(r, fromHistory, true, content),
        () => {
          if (host.viewerHost) location.assign(host.viewerHost.machinesPath);
        },
      );
      return;
    }
    if (r.v !== 'sessions' || r !== host.focusSessionsSearchOnRender)
      host.focusSessionsSearchOnRender = null;
    if (host.sidebarOnly) {
      if (!fromHistory) {
        host.documentEventsOwner.closeDrawer(true);
        location.assign(
          r.v === 'machines' && host.machinesPath
            ? host.machinesPath
            : host.bootstrapOwner.urlOf(r),
        );
      }
      return;
    } // an embedding page's sidebar leads to the viewer's pages
    if (host.navigation.route.v === 'session')
      host.pagingOwner.clearPaging('id' in host.navigation.route ? host.navigation.route.id : '');
    host.pagingOwner.resetPagerInput();
    host.viewport.stopOpeningEndPin();
    host.navigation.cancelRoute();
    if (r.v === 'machines' && host.machinesPath && !host.viewerHost) {
      location.assign(host.machinesPath);
      return;
    }
    if (!fromHistory) host.historyScrollOwner.saveHistoryScroll();
    // Capture outgoing history before removing host content: removal can clamp its scroll offset.
    host.navigation.replaceContent(r, nextContent);
    host.accountControlsOwner.closeAccountMenu(true, true);
    host.sessionChrome.dropErrors(true); // (first: it drops a range the error stepper moved, and that is not kept)
    // Keep the session left after the cached destination has had a frame to draw; weighing it must not delay that draw.
    if (
      host.navigation.route.v === 'session' &&
      (r.v !== 'session' ||
        r.v !== 'session' ||
        r.id !== ('id' in host.navigation.route ? host.navigation.route.id : '')) &&
      host.transcripts.entries['id' in host.navigation.route ? host.navigation.route.id : ''] &&
      host.transcripts.meta['id' in host.navigation.route ? host.navigation.route.id : '']
    ) {
      const sid = 'id' in host.navigation.route ? host.navigation.route.id : '',
        entries = host.transcripts.entries[sid],
        meta = { ...host.transcripts.meta[sid], origin: !!host.domain.originHandoff(sid) };
      host.scope.frame(() =>
        host.scope.frame(() => host.scope.timeout(() => host.cacheTx(sid, entries, meta), 0)),
      );
    }
    if (
      r.v !== 'session' ||
      r.v !== 'session' ||
      r.id !== ('id' in host.navigation.route ? host.navigation.route.id : '')
    )
      host.show = { ...host.allVisibility };
    if (r.v !== 'sessions') host.orderingControlsOwner.ORD.delete('page');
    host.navigation.route = r;
    host.find = '';
    host.findOpen = false;
    host.documentEventsOwner.closeDrawer(true);
    host.viewport.clearNewEntries();
    if (!fromHistory) {
      const state = { ...r };
      delete state.scrollTop;
      try {
        history.pushState(state, '', host.bootstrapOwner.urlOf(r));
      } catch {}
    }
    const done = () => {
      if (host.navigation.route !== r) return;
      endLoading();
      host.documentRendererOwner.render();
      if (r.v === 'session' || (host.viewerHost && r.v === 'machines')) focusTitle();
      if (fromHistory && Number.isFinite(r.scrollTop)) {
        // A fresh offscreen turn has only its intrinsic estimate. Measure once on history navigation
        // before setting the saved offset, so the browser cannot clamp it to the estimated height.
        const turns = [...host.$('#page').querySelectorAll<HTMLElement>('.turn')];
        for (const turn of turns) revealMeasuredTurn(turn, true);
        const heights = turns.map((turn) => turn.getBoundingClientRect().height);
        turns.forEach((turn, i) => {
          setGeometry(turn, 'intrinsicHeight', Math.ceil(heights[i]));
          revealMeasuredTurn(turn, false);
        });
        host.historyScrollOwner.restoreScroll(r.scrollTop!);
        host.historyScrollOwner.restoreHostFocus(r.hostFocus);
      } else if (r.v === 'session' && r.turn) {
        revealTurn(r.turn, !fromHistory);
        if (location.hash) host.scope.frame(() => host.scope.frame(revealEntryHash));
      } else if (r.v === 'session' && location.hash) revealEntryHash();
      else if (r.v === 'session') openSessionAtEnd();
      else host.historyScrollOwner.quietTop();
      host.viewport.syncJump();
    };
    // A transcript already in memory, still in TX or kept in the cache, needs no network: the top bar and the sidebar are drawn
    // from the model at once, the page itself on the next frame (so the first two are on screen before a long transcript is
    // built), and it is brought up to date afterwards. Otherwise the route waits for its data, with the top bar and the
    // sidebar already drawn.
    if (r.v === 'session' && host.modelStore.sessions[r.id]) {
      if (host.transcripts.staleBriefs.has(r.id)) {
        delete host.transcripts.entries[r.id];
        delete host.transcripts.meta[r.id];
        host.transcripts.cache.delete(r.id);
      }
      const kept = !host.transcripts.entries[r.id];
      if (kept ? host.adoptCached(r) : !isDeep(r)) {
        host.transcripts.cache.delete(r.id);
        paintPending(r);
        host.scope.frame(() =>
          host.scope.timeout(() => {
            if (host.navigation.route !== r) return;
            if (kept && !r.turn) host.transportOwner.spread(r.id);
            done();
            host.transcriptRevalidationOwner.revalidate(r);
          }, 0),
        );
        return;
      }
    }
    if (r.v === 'session') paintPending(r);
    host.navigation.load(
      r,
      (signal) => host.transportOwner.load(r, signal),
      done,
      (error) => failLoad(r, error),
    );
  }
  if (!host.sidebarOnly)
    host.scope.listen(window, 'popstate', (e) => {
      if (host.toolViewsOwner.skipPop) {
        host.toolViewsOwner.skipPop = false;
        if (host.afterPop) {
          const leave = host.afterPop;
          host.afterPop = null;
          leave();
          return;
        }
        if (host.pendingSessionOpen) {
          const id = host.pendingSessionOpen;
          host.pendingSessionOpen = null;
          goSession(id);
        }
        return;
      } // close a sheet before opening its session
      if (host.accountSheet) {
        host.accountSheet = false;
        host.accountControlsOwner.closeAccountMenu(true);
        return;
      } // back gesture closes the phone's account menu
      if (host.toolViewsOwner.viewerEl) {
        const d = host.toolViewsOwner.viewerEl;
        host.toolViewsOwner.viewerEl = null;
        d.close();
        return;
      } // back gesture closes the viewer, page stays
      if (e.state?.v)
        go(host.navigation.historyRoute(e.state, host.bootstrapOwner.routeOf(location)), true);
    });
  const goSession = (id: string, turn: string | undefined = undefined) =>
    go(turn ? { v: 'session', id, turn } : { v: 'session', id });
  const goTrace = (turn: string) => {
    const found = host.modelStore.turn.get(turn);
    if (found) go({ v: 'trace', sid: found.sid, turn });
  };
  const openSender = (h: Handoff) => {
    if (host.modelStore.sessions[h.from]) goSession(h.from, host.modelStore.holds.get(h.id)?.id);
  };
  // A deep link to a turn: scroll it just below the top bar (measured, since its height varies) and mark it for a moment.
  // It is placed again two frames later, after "Show more" buttons above it have appeared.
  function revealTurn(id: string, flash: boolean) {
    host.pagingOwner.resetPagerInput();
    const b = [...document.querySelectorAll<HTMLElement>('.turn')].find(
      (x) => x.dataset.turn === id,
    );
    if (!b) {
      host.historyScrollOwner.quietTop();
      return;
    }
    const place = () => {
      if (!b.isConnected) return;
      const gap = host.$('#topbar').offsetHeight + 8;
      host.pagingOwner.scrollProgrammatically(() => {
        if (host.phone.matches)
          window.scrollTo(0, Math.max(0, window.scrollY + b.getBoundingClientRect().top - gap));
        else {
          const m = host.$('#main');
          m.scrollTop += b.getBoundingClientRect().top - m.getBoundingClientRect().top - gap;
        }
      });
      host.viewport.syncJump();
      host.historyScrollOwner.saveHistoryScroll();
    };
    place();
    host.scope.frame(() => host.scope.frame(place));
    if (flash) {
      b.classList.add('flash');
      host.scope.timeout(() => b.classList.remove('flash'), 1500);
    }
  }
  function revealEntryHash() {
    host.pagingOwner.resetPagerInput();
    if (!location.hash) return;
    let key = '';
    try {
      key = decodeURIComponent(location.hash.slice(1));
    } catch {
      key = location.hash.slice(1);
    }
    const target =
      document.getElementById(key) ??
      [...document.querySelectorAll<HTMLElement>('[data-e]')].find((n) => n.dataset.e === key) ??
      [...document.querySelectorAll<HTMLElement>('.turn[data-turn]')].find(
        (n) => n.dataset.turn === key,
      );
    if (!target) return;
    const place = () => {
      if (!target.isConnected) return;
      const gap = host.$('#topbar').offsetHeight + 8;
      host.pagingOwner.scrollProgrammatically(() => {
        if (host.phone.matches)
          window.scrollTo(
            0,
            Math.max(0, window.scrollY + target.getBoundingClientRect().top - gap),
          );
        else {
          const m = host.$('#main');
          m.scrollTop += target.getBoundingClientRect().top - m.getBoundingClientRect().top - gap;
        }
      });
      host.viewport.syncJump();
      host.historyScrollOwner.saveHistoryScroll();
    };
    place();
    host.scope.frame(() => host.scope.frame(place));
  }

  // ---- Sidebar ----------------------------------------------------------------------------------
  // One typed owner for trigger/menu DOM, open state, focus and dismissal listeners. The viewer owns
  // history, pending model transactions and CSSOM placement; none of those globals enter the library.

  return {
    goSession,
    go,
    openSender,
    revealTurn,
    revealEntryHash,
    openSessionAtEnd,
    goTrace,
    get lanesFor(): { r: ApplicationRoute; version: string | null } | null {
      return lanesFor;
    },
    set lanesFor(value: { r: ApplicationRoute; version: string | null } | null) {
      lanesFor = value;
    },
  };
}

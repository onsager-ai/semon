import type { TranscriptStore } from '../state/transcript';
import type { NavigationController } from '../navigation/routes';
import type { EffectScope } from '../app/effects';
import type { createViewport } from './viewport';
import type { createTransport } from './transport';
import type { createDocumentRenderer } from './documentRenderer';
import type { createHistoryScroll } from './historyScroll';
import { createPagerController, updateSessionPager } from '../lib';
import type { PageDirection } from '../state/transcript';
interface PagingHost {
  scope: EffectScope;
  navigation: NavigationController;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  findOpen: boolean;
  find: string;
  show: { messages: boolean; tools: boolean; thinking: boolean };
  phone: MediaQueryList;
  disposed: boolean;
  sidebarOnly: boolean;

  historyScrollOwner: Pick<ReturnType<typeof createHistoryScroll>, 'saveHistoryScroll'>;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'fetchTx'>;

  transcripts: Pick<TranscriptStore, 'meta' | 'paging' | 'clearPaging' | 'drop'>;

  viewport: Pick<
    ReturnType<typeof createViewport>,
    'stopOpeningEndPin' | 'scroller' | 'capture' | 'restore' | 'syncJump' | 'openingEndUntil'
  >;
}
/** Owns paging behavior through explicit application ports. */
export function createPaging(host: PagingHost) {
  const pagingStore = host.transcripts.paging,
    PAGING = pagingStore.states;
  let pagerArmed = false,
    automaticLoads = 0,
    scrollRevision = 0,
    programmaticScrollPending = false,
    programmaticScrollTimer: number | undefined = undefined;
  const clearPaging = (sid: string) => host.transcripts.clearPaging(sid);
  const dropTx = (sid: string) => host.transcripts.drop(sid);
  function resetPagerInput() {
    pagerArmed = false;
    automaticLoads = 0;
    scrollRevision++;
    disconnectPagerObservers();
  }
  function holdProgrammaticScroll() {
    programmaticScrollPending = true;
    host.scope.clearTimeout(programmaticScrollTimer);
    // Scroll events arrive after scrollTop writes. Smooth jumps keep extending this guard until scrolling is quiet.
    programmaticScrollTimer = host.scope.timeout(() => {
      programmaticScrollPending = false;
      programmaticScrollTimer = undefined;
    }, 120);
  }
  function scrollProgrammatically(fn: () => void, jump = true) {
    if (jump) resetPagerInput();
    holdProgrammaticScroll();
    fn();
  }
  function readerScrollInput() {
    if (
      host.navigation.route.v !== 'session' ||
      host.navigation.rendered !== host.navigation.route ||
      host.$('#page').hasAttribute('aria-busy')
    )
      return;
    host.scope.clearTimeout(programmaticScrollTimer);
    programmaticScrollTimer = undefined;
    programmaticScrollPending = false;
    host.viewport.stopOpeningEndPin();
    pagerArmed = true;
    automaticLoads = 0;
    scrollRevision++;
    queuePagerObservers();
  }
  const automaticPagingAllowed = () =>
    pagerArmed &&
    automaticLoads < 3 &&
    !host.viewport.openingEndUntil &&
    !host.findOpen &&
    !host.find &&
    host.show.messages &&
    host.show.tools &&
    host.show.thinking;
  const pagingState = (sid: string, where: PageDirection) => pagingStore.get(sid, where);
  function pagerSnapshot(sid: string, where: PageDirection) {
    const state = pagingState(sid, where),
      direction = where === 'before' ? 'earlier' : 'later';
    return {
      sid,
      where,
      disabled: state.busy,
      busy: state.busy,
      text: state.busy
        ? 'Loading ' + direction + '…'
        : state.failed
          ? "Couldn't load " + direction + ' entries · Retry'
          : 'Load ' +
            direction +
            (where === 'after' && host.transcripts.meta[sid]?.newer
              ? ' · ' + host.transcripts.meta[sid].newer + ' new'
              : ''),
    };
  }
  function paintPager(b: HTMLElement) {
    updateSessionPager(
      host.$('#page'),
      pagerSnapshot(b.dataset.pagerSid!, b.dataset.pagerWhere === 'before' ? 'before' : 'after'),
    );
  }
  const pagerController = createPagerController({
    route(sid) {
      return host.navigation.route.v === 'session' &&
        ('id' in host.navigation.route ? host.navigation.route.id : '') === sid &&
        host.navigation.rendered === host.navigation.route
        ? host.navigation.route
        : null;
    },
    range(sid) {
      return host.transcripts.meta[sid];
    },
    state: pagingState,
    automatic: automaticPagingAllowed,
    current(sid, where, state, r) {
      return PAGING.get(sid)?.[where] === state;
    },
    beginManual: () => host.viewport.stopOpeningEndPin(),
    countAutomatic() {
      automaticLoads++;
    },
    paint: paintPager,
    load(sid, where, boundary, signal, applied) {
      return host.transportOwner.fetchTx(sid, where + '=' + boundary, where, signal, applied);
    },
    commit(r, where, manual) {
      const box = host.viewport.scroller(),
        top = host.phone.matches ? 0 : box.getBoundingClientRect().top,
        st = host.viewport.capture();
      const entry = [
        ...host
          .$('#page')
          .querySelectorAll<HTMLElement>('.turns [data-e][data-entry-key]:not(.tgroup)'),
      ].find((n) => {
        const rect = n.getBoundingClientRect();
        return rect.height && rect.top >= top;
      });
      st.paging = {
        anchor: entry
          ? { key: entry.dataset.entryKey, off: entry.getBoundingClientRect().top - top }
          : null,
        height: box.scrollHeight,
        before: where === 'before',
      };
      const armed = pagerArmed,
        used = automaticLoads;
      host.documentRendererOwner.render();
      host.viewport.restore(st);
      if (!manual) {
        pagerArmed = armed;
        automaticLoads = used;
      }
      host.viewport.syncJump();
      host.historyScrollOwner.saveHistoryScroll();
    },
    queue: queuePagerObservers,
  });
  function disconnectPagerObservers() {
    pagerController.disconnect();
  }
  function queuePagerObservers() {
    if (host.disposed || host.sidebarOnly) return;
    pagerController.queue(
      host.$('#page'),
      host.phone.matches ? null : host.$('#main'),
      () =>
        host.navigation.route.v === 'session' &&
        host.navigation.rendered === host.navigation.route &&
        automaticPagingAllowed() &&
        !host.$('#page').hasAttribute('aria-busy'),
    );
  }
  function loadPager(button: HTMLButtonElement, manual: boolean) {
    return pagerController.load(button, manual);
  }
  // "Load earlier" at the top of a transcript, and "Load later" at its end when a deep link loaded a middle page.

  // "View all" reads the whole call: each part the server cut ("more") is fetched in full from /api/entry, by the entry's
  // session and slot. A part cut again at 8 MB is noted (fullCut).

  return {
    pagerController,
    resetPagerInput,
    scrollProgrammatically,
    clearPaging,
    dropTx,
    loadPager,
    pagerSnapshot,
    holdProgrammaticScroll,
    queuePagerObservers,
    disconnectPagerObservers,
    get scrollRevision() {
      return scrollRevision;
    },
    set scrollRevision(value: number) {
      scrollRevision = value;
    },
    paintPager,
    readerScrollInput,
    get programmaticScrollPending() {
      return programmaticScrollPending;
    },
    set programmaticScrollPending(value: boolean) {
      programmaticScrollPending = value;
    },
  };
}

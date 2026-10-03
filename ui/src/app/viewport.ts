import { updateSessionJump } from "../lib";
import { createScrollTransactions } from "../navigation/scroll";
interface ViewportHost {
  phone: MediaQueryList;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  queuePagerObservers: () => void;
  navigation: import("../navigation/routes").NavigationController;
  scrollProgrammatically: (fn: () => void, jump?: boolean) => void;
  saveHistoryScroll: () => void;
  resetPagerInput: () => void;
  disconnectPagerObservers: () => void;
  scope: import("../app/effects").EffectScope;
  viewerEl: HTMLDialogElement | null;
  scrollRevision: number;
  syncBarLine: () => void;
  shellChrome: import("../lib/shell").ShellChrome | null;
  holdProgrammaticScroll: () => void;
  tick: () => void;
  SESS: Record<string, import("../domain/types").Session>;
  TURN: Map<string, import("../domain/types").Turn>;
  renderSession: (page: HTMLElement, sid: string, opts?: { only?: ReadonlySet<string> | undefined; }) => void;
  drawSessionBar: () => void;
  paintPager: (b: HTMLElement) => void;
  renderNav: () => void;
  renderLanes: () => void;
  ticker: () => void;
  LIVE: import("../lib/live").LiveState;
  TXM: Record<string, import("../domain/types").TranscriptMeta>;
  fetchTx: (sid: string, q?: string, where?: import("../state/transcript").PageDirection | undefined, signal?: AbortSignal | undefined, onPage?: (() => void) | undefined) => Promise<void>;
  goSession: (id: string, turn?: string | undefined) => void;
  SIDEBAR_ONLY: boolean;
  readerScrollInput: () => void;
  programmaticScrollPending: boolean;
}
/** Owns viewport behavior through explicit application ports. */
export function createViewport(host: ViewportHost) {
  const scroller = () => (host.phone.matches ? document.documentElement : host.$("#main"));
  const edge = () => host.$("#topbar").getBoundingClientRect().bottom;
  // The opened transcript can still grow as fonts and clamped cards settle; hold the tail briefly, then yield on reader input.
  let openingEndUntil = 0; let openingEndTimer: number | undefined; let openingEndObserver: ResizeObserver | null = null;
  function stopOpeningEndPin() {
    const wasPinned = !!openingEndUntil;
    openingEndUntil = 0;
    host.scope.clearTimeout(openingEndTimer); openingEndTimer = undefined;
    openingEndObserver?.disconnect(); openingEndObserver = null;
    if (wasPinned) host.queuePagerObservers();
  }
  function pinOpeningEnd() {
    if (host.navigation.route.v !== "session" || performance.now() >= openingEndUntil) { stopOpeningEndPin(); return; }
    const sc = scroller(); host.scrollProgrammatically(() => { sc.scrollTop = sc.scrollHeight; }); scrollController.anchor = null; syncJump(); host.saveHistoryScroll();
  }
  function startOpeningEndPin() {
    host.resetPagerInput();
    stopOpeningEndPin(); if (host.navigation.route.v !== "session" || location.hash) return;
    openingEndUntil = performance.now() + 2000;
    host.disconnectPagerObservers();
    const turns = host.$("#page section[aria-label='Transcript'] .turns");
    if (turns) { openingEndObserver = new ResizeObserver(pinOpeningEnd); openingEndObserver.observe(turns); }
    pinOpeningEnd(); openingEndTimer = host.scope.timeout(stopOpeningEndPin, 2000);
  }
  const scrollController = createScrollTransactions({ page: () => host.$("#page"), main: () => host.$("#main"), phone: () => host.phone.matches, edge, rendered: () => host.navigation.rendered, sheet: () => !!host.viewerEl, revision: () => host.scrollRevision, programmatic: host.scrollProgrammatically, sync: host.syncBarLine, restoreDrawer: () => host.shellChrome?.restoreDrawer() });
  const { capture, restore, opener, stateKey, identOf } = scrollController;
  // A session page in place: its turns are drawn again and only those that changed (or are new) replace the ones shown, so
  // the rest keep their nodes and state. The bar's summary line, the title and the sidebar follow. Returns how many entries
  // are new.
  function patchSession(dirty: ReadonlySet<string> | null) {
    host.resetPagerInput(); host.holdProgrammaticScroll();
    host.tick(); const s = host.SESS[("id" in host.navigation.route ? host.navigation.route.id : "")], box = host.$("#page .turns");
    const keys = () => new Set([...host.$("#page").querySelectorAll<HTMLElement>(".turns :is(.msg, .bubble, .step, .event, .child-card, .thought, .think-pending)[data-e]")].map((n) => n.dataset.e));
    const before = keys();
    // Only the changed turns are drawn again, unless the turns shown no longer match the index or nothing was shown.
    const whole = !dirty || box.querySelector(":scope > p.empty") || [...box.querySelectorAll<HTMLElement>(":scope > .turn")].some((b) => !host.TURN.has(b.dataset.turn ?? ""));
    host.renderSession(host.$("#page"), ("id" in host.navigation.route ? host.navigation.route.id : ""), whole ? {} : { only: dirty });
    host.drawSessionBar();
    for (const pager of box.querySelectorAll<HTMLElement>("[data-pager-where]")) host.paintPager(pager);
    host.renderNav(); host.renderLanes(); host.ticker();
    let n = 0; for (const k of keys()) if (!before.has(k)) n++;
    host.queuePagerObservers();
    return n;
  }
  // Jump to the latest: centred at the transcript column's foot, sticky, with the count of what arrived while the reader was away.
  function scrollMetrics() {
    if (host.phone.matches) return { top: window.scrollY, height: document.documentElement.scrollHeight, viewport: window.innerHeight, gap: Math.max(0, document.documentElement.scrollHeight - window.innerHeight - window.scrollY) };
    const m = host.$("#main"); return { top: m.scrollTop, height: m.scrollHeight, viewport: m.clientHeight, gap: Math.max(0, m.scrollHeight - m.clientHeight - m.scrollTop) };
  }
  function scrollToEnd(behavior: ScrollBehavior = "smooth") { host.scrollProgrammatically(() => { if (host.phone.matches) window.scrollTo({ top: document.documentElement.scrollHeight, behavior }); else { const m = host.$("#main"); m.scrollTo({ top: m.scrollHeight, behavior }); } }); }
  // The button is rebuilt only when what it shows changes (hidden or not, and the new-entry count), not on every scroll.
  let jumpBusy = false;
  function syncJump() {
    if (host.navigation.route.v !== "session") { host.LIVE.fresh = 0; return; }
    const { gap } = scrollMetrics(), newer = host.TXM[("id" in host.navigation.route ? host.navigation.route.id : "")]?.newer ?? 0; if (gap <= 80) host.LIVE.fresh = 0;
    updateSessionJump(host.$("#page"), gap > 80 || !!newer, host.LIVE.fresh + newer, jumpBusy);
  }
  function clearNewEntries() { host.LIVE.fresh = 0; updateSessionJump(host.$("#page"), false, 0, jumpBusy); }
  function jumpToLatest() {
    const sid = ("id" in host.navigation.route ? host.navigation.route.id : ""), r = host.navigation.route, m = host.TXM[sid];
    if (m && m.to < m.total) {
      jumpBusy = true; syncJump(); host.fetchTx(sid, "").then(() => { if (host.navigation.route === r) host.goSession(sid); }).catch(() => {}).finally(() => { jumpBusy = false; syncJump(); });
    } else scrollToEnd("smooth");
  }
  if (!host.SIDEBAR_ONLY) { host.scope.listen(window, "scroll", syncJump, { passive: true }); host.scope.listen(host.$("#main"), "scroll", syncJump, { passive: true }); }
  const cancelOpeningEndPin = () => { if (openingEndUntil) stopOpeningEndPin(); };
  if (!host.SIDEBAR_ONLY) { host.scope.listen(window, "wheel", cancelOpeningEndPin, { passive: true }); host.scope.listen(window, "touchmove", cancelOpeningEndPin, { passive: true }); host.scope.listen(window, "pointerdown", cancelOpeningEndPin, { passive: true }); } // a press anywhere, a scrollbar drag included
  const transcriptInput = (e: Event) => e.target instanceof Element && !e.target.closest?.("#sidebar, dialog, input, textarea, select, [contenteditable='true']") && (host.phone.matches || host.$("#main").contains(e.target));
  if (!host.SIDEBAR_ONLY) {
    const input = (e: Event) => { if (transcriptInput(e)) host.readerScrollInput(); };
    host.scope.listen(window, "wheel", input, { passive: true }); host.scope.listen(window, "touchmove", input, { passive: true });
    const scroll = () => { if (host.programmaticScrollPending) host.holdProgrammaticScroll(); else if (!openingEndUntil) host.readerScrollInput(); };
    host.scope.listen(window, "scroll", () => { if (host.phone.matches) scroll(); }, { passive: true });
    host.scope.listen(host.$("#main"), "scroll", () => { if (!host.phone.matches) scroll(); }, { passive: true });
  }
  if (!host.SIDEBAR_ONLY) host.scope.listen(document, "keydown", (e) => {
    if (!e.defaultPrevented && (transcriptInput(e) || e.target === document.body || e.target === document.documentElement) && ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) host.readerScrollInput();
  });


  return {scrollController, stopOpeningEndPin, get openingEndUntil(): number { return openingEndUntil; }, scroller, capture, restore, syncJump, startOpeningEndPin, clearNewEntries, edge, opener, jumpToLatest, patchSession};
}

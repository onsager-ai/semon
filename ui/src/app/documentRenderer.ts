import { ownsScreen,releaseScreen,screenKind,setGeometry } from "../lib";
import type { ApplicationRoute } from '../navigation/routes';
interface SlotContext { destroy?: () => void; sync: () => void; onChange: () => void }
interface ControlSlot { route: ApplicationRoute; box: HTMLElement; ctx: SlotContext; el: HTMLElement }
interface DocumentRendererHost {
  navigation: import("../navigation/routes").NavigationController;
  viewerHost: import("../viewer-host").ViewerHost | null;
  NATIVE_PAGE: { title: string; nav: string; } | undefined;
  disposed: boolean;
  focusSessionsSearchOnRender: import("../navigation/routes").ApplicationRoute | null;
  SIDEBAR_ONLY: boolean;
  domain: import("../domain/calculations").DomainController;
  tick: () => void;
  renderNav: () => void;
  renderLanes: () => void;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  renderTopbar: ReturnType<typeof import("./sessionChrome").createSessionChrome>["renderTopbar"];
  syncLayoutPrefs: () => void;
  syncBarLine: () => void;
  renderDrawerAccount: () => void;
  syncJump: () => void;
  resetPagerInput: () => void;
  holdProgrammaticScroll: () => void;
  closeAccountMenu: (keepEntry?: boolean | undefined, navigating?: boolean | undefined) => void;
  stopOpeningEndPin: () => void;
  orderingControlsOwner: ReturnType<typeof import("./orderingControls").createOrderingControls>;
  ordState: (name: string) => { inView: boolean; touched: boolean; };
  renderHome: (page: HTMLElement) => void;
  renderAnalytics: (page: HTMLElement) => void;
  renderSessions: (page: HTMLElement, focusSearch?: boolean) => void;
  renderMachines: (page: HTMLElement) => void;
  renderMachine: (page: HTMLElement, m: string) => void;
  MACHINE: Record<string, string>;
  go: (r: import("../navigation/routes").ApplicationRoute, fromHistory?: boolean, prepared?: boolean, nextContent?: import("../viewer-host").ViewerContent | null) => void;
  machineLine: (m: string) => import("../lib/viewer-bar").BarLabel[];
  renderTrace: (page: HTMLElement, id: string) => string | undefined;
  SESS: Record<string, import("../domain/types").Session>;
  goSession: (id: string, turn?: string | undefined) => void;
  lineageOf: (sid: string) => import("../domain/types").Session[];
  renderSession: (page: HTMLElement, sid: string, opts?: { only?: ReadonlySet<string> | undefined; }) => void;
  sessionLine: (s: import("../domain/types").Session) => import("../lib/viewer-bar").BarLabel[];
  errOn: (sid: string) => boolean;
  markError: (ring: boolean) => HTMLElement | null;
  destination: { goSession: (id: string, turn?: string | undefined) => void; go: (r: import("../navigation/routes").ApplicationRoute, fromHistory?: boolean, prepared?: boolean, nextContent?: import("../viewer-host").ViewerContent | null) => void; openSender: (h: import("../domain/types").Handoff) => void; revealTurn: (id: string, flash: boolean) => void; revealEntryHash: () => void; openSessionAtEnd: () => void; goTrace: (turn: string) => void; get lanesFor(): { r: import("../navigation/routes").ApplicationRoute; version: string | null; } | null; set lanesFor(value: { r: import("../navigation/routes").ApplicationRoute; version: string | null; } | null); };
  LIVE: import("../lib/live").LiveState;
}
/** Owns documentRenderer behavior through explicit application ports. */
export function createDocumentRenderer(host: DocumentRendererHost) {
  const SLOTS = new Map<string,ControlSlot>();
  function slot(key: string, box: HTMLElement, build: (ctx: SlotContext) => HTMLElement) {
    let s = SLOTS.get(key);
    if (!s || s.route !== host.navigation.route || s.box !== box || !box.contains(s.el)) { s?.ctx.destroy?.(); const ctx: SlotContext = {sync() {}, onChange() {}}; s = { route: host.navigation.route, box, ctx, el: build(ctx) }; SLOTS.set(key, s); }
    return s;
  }
  // Empties `box` of everything but the slots the route being drawn already holds in it.
  function clearBox(box: HTMLElement, r: ApplicationRoute | null = null) {
    for (const [key, s] of SLOTS) if (s.route !== r) { s.ctx.destroy?.(); SLOTS.delete(key); }
    if (ownsScreen(box)) { if (screenKind(box) === r?.v && !(r?.v === "machines" && host.viewerHost) && !host.NATIVE_PAGE) return; releaseScreen(box); }
    const kept = new Set([...SLOTS.values()].filter((s) => s.box === box).map((s) => s.el));
    for (const n of [...box.childNodes]) if (!(n instanceof HTMLElement && kept.has(n))) n.remove();
  }
  // Adds nodes to `box` in order, after `clearBox`: a node already at the next place stays where it is, and any other
  // goes in before it. `put.done()` drops what is left over.
  function placer(box: HTMLElement) {
    let cur = box.firstChild;
    const put = (...nodes: Node[]) => { for (const n of nodes) { if (n === cur) cur = cur?.nextSibling ?? null; else box.insertBefore(n, cur); } };
    put.done = () => { while (cur) { const next = cur.nextSibling; cur.remove(); cur = next; } };
    return put;
  }
  function render() {
    if (host.disposed) return;

    const focusSearch = host.focusSessionsSearchOnRender === host.navigation.route; host.focusSessionsSearchOnRender = null;
    if (host.SIDEBAR_ONLY) { host.domain.invalidate(); host.tick(); host.navigation.rendered = host.navigation.route; host.renderNav(); host.renderLanes(); return; } // the embedding page draws its own page and bar
    if (host.NATIVE_PAGE || (host.navigation.route.v === "machines" && host.viewerHost)) {
      host.tick(); host.navigation.rendered = host.navigation.route;
      if (host.navigation.content && !host.navigation.content.element.isConnected) { clearBox(host.$("#page"), host.navigation.route); host.$("#page").append(host.navigation.content.element); }
      document.title = (host.NATIVE_PAGE?.title ?? "Machines") + " · Semon";
      host.renderTopbar(host.NATIVE_PAGE?.title ?? "Machines"); host.syncLayoutPrefs(); host.syncBarLine(); host.renderNav(); host.renderLanes(); host.renderDrawerAccount(); host.syncJump(); return;
    }
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    if (host.viewerHost) document.title = ({ home: "Home", sessions: "Sessions", analytics: "Analytics", machines:"Machines", machine:"Machine", session:"Session", trace:"Trace" }[host.navigation.route.v] ?? "Semon") + " · Semon";
    host.resetPagerInput(); host.holdProgrammaticScroll(); host.closeAccountMenu(); host.stopOpeningEndPin(); host.domain.invalidate(); // a redraw inside the open-at-end window ends the pin
    host.orderingControlsOwner.ordPageState = host.ordState("page"); host.tick(); const page = host.$("#page"), r = host.navigation.route; host.navigation.rendered = r; setGeometry(page, "paddingBottom", null); clearBox(page, r); page.classList.remove("child-page");
    if (r.v === "home") { host.renderHome(page); host.renderTopbar("Home"); }
    else if (r.v === "analytics") { host.renderAnalytics(page); host.renderTopbar("Analytics", null, { analytics: true }); }
    else if (r.v === "sessions") { host.renderSessions(page, focusSearch); host.renderTopbar("Sessions"); }
    else if (r.v === "machines") { host.renderMachines(page); host.renderTopbar("Machines"); }
    else if (r.v === "machine") { host.renderMachine(page, r.id); host.renderTopbar(host.MACHINE[r.id], { label: "Machines", go: () => host.go({ v: "machines" }) }, { line2: host.machineLine(r.id) }); }
    else if (r.v === "trace") { host.renderTrace(page, r.turn); host.renderTopbar("Trace", { label: host.SESS[r.sid].name, go: () => host.goSession(r.sid, r.turn) }, { traceSession: host.SESS[r.sid] }); }
    else if (r.v === "session") { const s = host.SESS[r.id], lineage = host.lineageOf(r.id).slice(0, -1); host.renderSession(page, r.id); host.renderTopbar(s.name, null, { session: s, lineage, line2: host.sessionLine(s) }); }
    if (r.v === "session" && host.errOn(r.id)) host.markError(false);
    setGeometry(document.documentElement, "barHeight", host.$("#topbar").offsetHeight);
    const lanesKept = host.destination.lanesFor && host.destination.lanesFor.r === r && host.destination.lanesFor.version === host.LIVE.version; host.destination.lanesFor = null;
    host.syncLayoutPrefs(); host.syncBarLine(); host.renderNav(); if (!lanesKept) host.renderLanes(); host.renderDrawerAccount(); host.syncJump(); host.orderingControlsOwner.ordPageState = null;
  }

  // ---- Analytics: the server computes each range (/api/analytics) -----------------------------------------------------------
  // The model holds only its own window (a day), so the page asks the server for the range it shows: 24 h, 7 d or 30 d, with
  // the filters. The answer has every figure, chart column and list the page draws, and the page does no range math. It is
  // asked for when the page opens, when the range or a filter changes, after every model update (the server answers 304
  // while nothing changed), and every 10 s while the page shows, as time moves the range. Answers are kept per range and
  // filters, so switching back draws at once while the page asks again.

  return {SLOTS, render, slot};
}

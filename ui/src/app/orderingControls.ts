import type { Session } from '../domain/types';
import { createOrdering,orderRows } from "../lib";
import type { ApplicationRoute } from '../navigation/routes';
interface OrderingControlsHost {
  query: string;
  groupBy: string;
  sessionFilters: { repo: string; machine: string; harness: string; model: string; };
  scope: import("../app/effects").EffectScope;
  visible: () => boolean;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  navigation: import("../navigation/routes").NavigationController;
  toolViewsOwner: ReturnType<typeof import("./toolViews").createToolViews>;
  renderLanes: () => void;
  capture: () => import("../navigation/scroll").ScrollSnapshot;
  render: () => void;
  restore: (st: import("../navigation/scroll").ScrollSnapshot, pin?: boolean) => void;
  phone: MediaQueryList;
}
/** Owns orderingControls behavior through explicit application ports. */
export function createOrderingControls(host: OrderingControlsHost) {
  const ordering = createOrdering<ApplicationRoute>(), ORD = ordering.scopes, ordTouch = { down: false }, byLast = (a: Session, b: Session) => b.last - a.last;
  const pageSig = () => JSON.stringify([host.query, host.groupBy, host.sessionFilters]);
  host.scope.listen(document, "pointerdown", () => { ordTouch.down = true; }, true);
  for (const t of ["pointerup", "pointercancel"]) host.scope.listen(document, t, () => { ordTouch.down = false; }, true);
  host.scope.listen(window, "blur", () => { ordTouch.down = false; });
  // A tab that comes back from the background applies what was held, once, before it paints, from the data it has. What the catch-up
  // poll brings after that is held like any other update.
  host.scope.listen(document, "visibilitychange", () => { ordTouch.down = false; if (host.visible()) { orderApply("page"); orderApply("side"); } });
  // How many rows must move to put `rows` in `cmp` order: all of them but the longest run already in it.
  // Whether the reader could be using a screen's list: is its top in view, is it touched. Read before a draw empties the list (an
  // emptied scroller clamps to 0 and the rows and their hover are gone). Focus counts only when it is keyboard focus on a row of this
  // list, a mouse only over one of its rows.
  function ordState(name: string) {
    const side = name === "side", rows = side ? "#lanes .treeitem" : "#page .nrow", a = document.activeElement;
    const inView = side ? sideRegion().scrollTop <= 1 : !host.$(rows) || host.$(rows).getBoundingClientRect().top >= host.$("#topbar").getBoundingClientRect().bottom - 1;
    const touched = ordTouch.down || (!!a?.matches?.(":focus-visible") && !!a.closest(side ? "#lanes" : rows)) || (matchMedia("(hover: hover)").matches && !!host.$(rows + ":hover"));
    return { inView, touched };
  }
  let ordPageState: ReturnType<typeof ordState> | null = null;
  const pageState = () => ordPageState ?? ordState("page");
  // One draw of a screen's lists. `sig` is what the reader chose to show, `tie` the route it belongs to (null: the sidebar, which
  // outlives routes); a draw with another sig or tie starts from the sorted order. `st` is ordState from before the draw.
  const orderScope = (name: string, sig: string, tie: ApplicationRoute | null, state: {inView: boolean; touched: boolean}) => ordering.begin(name, sig, tie, state), orderList = orderRows;
  const sideRegion = () => host.$("#side-list") ?? host.$("#sidebar");
  // How long the wide screen's sidebar is left alone before it applies what it holds. Read once, at load, with 10 s as the default; a
  // browser check may set window.__semonOrderIdleMs to a finite number from 200 to 60000 ms before load; other values use the default.
  const ordIdleMs = Reflect.get(window,"__semonOrderIdleMs");
  const ORD_IDLE_MS = Number.isFinite(ordIdleMs) && ordIdleMs >= 200 && ordIdleMs <= 60000 ? ordIdleMs : 10000;
  const ORD_DRAWER_MS = 320; // the drawer's slide (0.24 s) and a little
  // Applies what a screen holds: the list is drawn sorted, from scratch.
  function orderApply(name: string) {
    const sc = ORD.get(name); if (!sc?.n || (name === "page" && (sc.tie !== host.navigation.route || host.navigation.rendered !== host.navigation.route || host.toolViewsOwner.viewerEl || host.$("#page").hasAttribute("aria-busy")))) return;
    ORD.delete(name);
    if (name === "side") host.renderLanes(); else { const st = host.capture(); host.render(); host.restore(st); }
  }
  // The wide screen's sidebar is always in view: what it holds is applied once it has been left alone for ORD_IDLE_MS. Left alone means
  // no pointer over it or down, no keyboard focus or focused text field in it, and no menu or dialog open (the session menu hangs from
  // the top bar, outside the sidebar, so it is named here); anything the reader does to it starts the wait again.
  let ordIdle: number | undefined;
  function ordIdleArm() {
    host.scope.clearTimeout(ordIdle); ordIdle = undefined;
    if (host.phone.matches || !ORD.get("side")?.n) return;
    ordIdle = host.scope.timeout(() => {
      ordIdle = undefined; const bar = host.$("#sidebar"), a = document.activeElement;
      const menu = host.$(".session-menu, .account-popover, .runs-popover, .filters.pop:not([hidden])");
      if (ordTouch.down || bar.matches(":hover") || (a && bar.contains(a) && (a.matches(":focus-visible") || a.matches("input, textarea, select, [contenteditable]"))) || menu || host.toolViewsOwner.viewerEl) ordIdleArm(); else orderApply("side");
    }, ORD_IDLE_MS);
  }
  for (const t of ["pointermove", "pointerdown", "pointerleave", "focusin", "focusout", "wheel", "keydown"]) host.scope.listen(host.$("#sidebar"), t, () => { if (ordIdle) ordIdleArm(); }, { passive: true });

  return {ORD, orderApply, ORD_DRAWER_MS, byLast, orderList, ordState, orderScope, get ordIdle() { return ordIdle; }, set ordIdle(value: number | undefined) { ordIdle = value; }, ordIdleArm, pageSig, pageState, get ordPageState() { return ordPageState; }, set ordPageState(value: { inView: boolean; touched: boolean; } | null) { ordPageState = value; }};
}

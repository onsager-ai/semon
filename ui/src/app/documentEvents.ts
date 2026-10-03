import { measureViewerBar } from "../lib";
import type { ApplicationRoute } from '../navigation/routes';
interface DocumentEventsHost {
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  shellChrome: import("../lib/shell").ShellChrome | null;
  SIDEBAR_ONLY: boolean;
  scope: import("../app/effects").EffectScope;
  orderApply: (name: string) => void;
  accountSheet: boolean;
  accountChrome: import("../lib/account-chrome").AccountChrome;
  toolViewsOwner: ReturnType<typeof import("./toolViews").createToolViews>;
  closeAccountMenu: (keepEntry?: boolean | undefined, navigating?: boolean | undefined) => void;
  navigation: import("../navigation/routes").NavigationController;
  findOpen: boolean;
  render: () => void;
  query: string;
  focusSessionsSearchOnRender: import("../navigation/routes").ApplicationRoute | null;
  go: (r: import("../navigation/routes").ApplicationRoute, fromHistory?: boolean, prepared?: boolean, nextContent?: import("../viewer-host").ViewerContent | null) => void;
  phone: MediaQueryList;
  syncLayoutPrefs: () => void;
  recentNavigation: { recentRenderer: import("../lib/recent").RecentRenderer; expandedAll: string | null; renderLanes: () => void; COST_TIP: string; isApprovalReview: (s: import("../domain/types").Session) => boolean; };
  renderLanes: () => void;
  currentScroll: () => number;
  restoreScroll: (top: number) => void;
  syncJump: () => void;
}
/** Owns documentEvents behavior through explicit application ports. */
export function createDocumentEvents(host: DocumentEventsHost) {
  const sidebar = host.$("#sidebar");
  function openDrawer() { host.shellChrome?.openDrawer(); }
  function closeDrawer(quiet: boolean | undefined = undefined) { host.shellChrome?.closeDrawer(quiet); }
  // Sidebar-only consumers retain their existing server shell.js owner.
  if (host.SIDEBAR_ONLY) host.scope.listen(window, "semon:drawer-open", () => host.orderApply("side"));
    if (!host.SIDEBAR_ONLY) host.scope.listen(document, "keydown", (e) => { if (e.key === "Escape" && host.accountSheet) host.accountChrome.escape(); else if (e.key === "Escape" && !host.toolViewsOwner.viewerEl) { closeDrawer(); host.closeAccountMenu(); host.$(".session-menu")?.remove(); host.$("#more-btn")?.setAttribute("aria-expanded", "false"); } if (e.key === "/" && !/INPUT|TEXTAREA/.test(document.activeElement?.tagName ?? "") && !(document.activeElement instanceof HTMLElement && document.activeElement.isContentEditable) && !host.toolViewsOwner.viewerEl) { e.preventDefault(); if (host.navigation.route.v === "session") { host.findOpen = true; host.render(); host.$("#find")?.focus(); } else if (host.navigation.route.v === "sessions") { host.$("#sq")?.focus(); } else { const r: ApplicationRoute = { v: "sessions", q: host.query }; host.focusSessionsSearchOnRender = r; host.go(r); } } });
  host.scope.listen(host.phone, "change", () => {
    closeDrawer(true); host.syncLayoutPrefs(); host.recentNavigation.expandedAll = null; host.renderLanes();
    if (!host.phone.matches && host.toolViewsOwner.viewerEl?.classList.contains("kids-sheet")) host.toolViewsOwner.viewerEl.close(); // a sheet is a phone's: a wide screen opens the list in the tree
    if (host.navigation.route.v === "session" || host.navigation.route.v === "analytics") { const top = host.currentScroll(); host.render(); host.restoreScroll(top); }
    else host.renderLanes();
    const l2 = host.$("#topbar .meta-line"); if (l2 && host.navigation.route.v === "session") measureViewerBar(host.$("#topbar")); host.syncJump();
  });

  // ---- Live updates (deliberate difference 3) --------------------------------------------------------------------------------
  // Every screen polls /api/model?since= every 2 s while the tab is visible, one request at a time: it backs off up to 30 s
  // on errors and stops on 403 (the server restarted with a new token). A new model swaps the globals and draws the screen
  // again with its view state kept: the scroll position, anchored to the first visible block; what is open, by stable keys;
  // focus, find and filters; the drawer. A session page follows its transcript's tail
  // with /api/tx?after= and replaces only the turns that changed. An open View all sheet holds the redraw until it closes.

  return {closeDrawer};
}

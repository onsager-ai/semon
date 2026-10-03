import type { HostFocus } from '../navigation/routes';
interface HistoryScrollHost {
  phone: MediaQueryList;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  scrollProgrammatically: (fn: () => void, jump?: boolean) => void;
  viewerHost: import("../viewer-host").ViewerHost | null;
  scope: import("../app/effects").EffectScope;
  toolViewsOwner: ReturnType<typeof import("./toolViews").createToolViews>;
  SIDEBAR_ONLY: boolean;
}
/** Owns historyScroll behavior through explicit application ports. */
export function createHistoryScroll(host: HistoryScrollHost) {
  const currentScroll = () => host.phone.matches ? window.scrollY : host.$("#main").scrollTop;
  const restoreScroll = (top: number) => host.scrollProgrammatically(() => { if (host.phone.matches) window.scrollTo(0, top); else host.$("#main").scrollTop = top; });
  let hostFocus: HostFocus | undefined;
  if (host.viewerHost) host.scope.listen(document, "focusin", (event) => {
    const node = event.target;
    if (!(node instanceof HTMLElement) || !host.$("#page").contains(node)) return;
    hostFocus = node.id ? { id: node.id } : node.dataset.id ? { row: node.dataset.id } : node.getAttribute("aria-label") ? { label: node.getAttribute("aria-label") ?? undefined } : undefined;
  });
  function restoreHostFocus(saved: HostFocus | undefined) {
    if (!host.viewerHost || !saved) return;
    const selector = saved.id ? "#" + CSS.escape(saved.id) : saved.row ? '[data-id="' + CSS.escape(saved.row) + '"]' : saved.label ? '[aria-label="' + CSS.escape(saved.label) + '"]' : null;
    if (selector) host.$("#page")?.querySelector<HTMLElement>(selector)?.focus({ preventScroll: true });
  }
  const saveHistoryScroll = () => { if (host.toolViewsOwner.viewerEl) return; try { if (history.state?.v) history.replaceState({ ...history.state, scrollTop: currentScroll(), ...(host.viewerHost ? { hostFocus } : {}) }, ""); } catch {} };
  let scrollSaveFrame = false;
  const queueScrollSave = () => { if (scrollSaveFrame) return; scrollSaveFrame = true; host.scope.frame(() => { scrollSaveFrame = false; saveHistoryScroll(); }); };
  if (!host.SIDEBAR_ONLY) { host.scope.listen(window, "scroll", queueScrollSave, { passive: true }); host.scope.listen(host.$("#main"), "scroll", queueScrollSave, { passive: true }); }
  const quietTop = () => restoreScroll(0);

  return {saveHistoryScroll, quietTop, restoreScroll, restoreHostFocus, currentScroll};
}

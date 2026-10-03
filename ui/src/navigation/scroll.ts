import { measureSessionScreen, measureTraceScreen, revealMeasuredTurn, setGeometry } from '../lib';
export interface ScrollSnapshot {
 top: number; bottom: boolean; drawer: boolean; anchor: {id: string; off: number} | null;
 open: Set<string | undefined>; groups: Set<string | undefined>; groupMembers: Set<string | undefined>;
 focus: { id: string | null; host: string | null; sel: string | null; i: number; range: [number, number] | null; label: string | null; at: number; of: number; foot: string | null } | null;
 paging?: {anchor: {key: string | undefined; off: number} | null; height: number; before: boolean};
}
export interface ScrollHost { page(): HTMLElement; main(): HTMLElement; phone(): boolean; edge(): number; rendered(): object | null; sheet(): boolean; revision(): number; programmatic(fn: () => void, jump?: boolean): void; sync(): void; restoreDrawer(): void }
export function createScrollTransactions(host: ScrollHost) {
 let anchor: {id: string; off: number; route: object | null; top: number} | null = null, disposed = false;
 const frames = new Set<number>();
 const frame = (fn: () => void) => { const id = requestAnimationFrame(() => { frames.delete(id); if (!disposed) fn(); }); frames.add(id); };
 const scroller = () => host.phone() ? document.scrollingElement ?? document.documentElement : host.main();
  const ANCHORS = "[data-e], .turn, .hop, .ib, .nrow, .sec-h, .ph, .divider, .analytics-metric, .analytics-panel, .facet-filters, .groupby, .find, .empty";
  const HOSTS = "[data-e], [data-h], [data-id], [data-sid], [data-go], [data-turn], [data-g], [data-m]";
  const FOCUSABLE = "button, input, [tabindex], a[href]";
  const stateKey = (n: HTMLElement) => n.dataset.entryKey ?? n.dataset.e;
  const identOf = (n: HTMLElement) => { const d = n.dataset, keys = [stateKey(n), d.turn, d.h, d.id, d.m, d.sid, d.go, d.g];
    return [n.classList[0], ...keys, keys.some((x) => x != null) ? "" : n.firstChild?.nodeType === 3 ? n.firstChild.textContent : ""].map((x) => x ?? "").join("|"); };
  // Each anchor candidate on the page, in order, with its identity made unique by how many came before it.
  function anchors(fn: (node: HTMLElement, id: string) => boolean): HTMLElement | null { const seen = new Map<string, number>(); for (const n of host.page().querySelectorAll<HTMLElement>(ANCHORS)) { const id = identOf(n), k = seen.get(id) ?? 0; seen.set(id, k + 1); if (fn(n, id + "#" + k)) return n; } return null; }
  const opener = (n: HTMLElement) => n.classList.contains("step") ? n.querySelector<HTMLButtonElement>(":scope > button") : n.classList.contains("tgroup") ? n.querySelector<HTMLButtonElement>(":scope > .tsum") : null;
  function capture(): ScrollSnapshot {
    const sc = scroller(), line = host.edge();
    const st: ScrollSnapshot = { top: sc.scrollTop, bottom: sc.scrollHeight - sc.scrollTop - sc.clientHeight <= 80, anchor: null, open: new Set(), groups: new Set(), groupMembers: new Set(), focus: null, drawer: document.body.classList.contains("drawer-open") };
    // The first block, innermost, still visible under the bar, and how far its top is from the bar. While the reader hasn't
    // scrolled since the last redraw placed it, that placement's block and offset are kept as they were, so the fraction
    // of a pixel each placement rounds off can't add up over many updates.
    const kept = anchor;
    const still = kept && kept.route === host.rendered() && Math.abs(sc.scrollTop - kept.top) < 1 ? anchors((n, id) => id === kept.id) : null;
    // ... and only while that block is still where it was placed (a filter, find or zoom since has moved it).
    if (still && kept && Math.abs(still.getBoundingClientRect().top - line - kept.off) <= 1) st.anchor = { id: kept.id, off: kept.off };
    else {
      const turnBounds = new Map<Element, DOMRect>();
      anchors((n, id) => {
        // Hidden turn contents need no measurements: only the enclosing turn's box can be near the reader.
        const turn = n.closest(".turn");
        if (turn) {
          let bounds = turnBounds.get(turn); if (!bounds) { bounds = turn.getBoundingClientRect(); turnBounds.set(turn, bounds); }
          if (bounds.bottom <= line || bounds.top >= (host.phone() ? innerHeight : sc.getBoundingClientRect().bottom)) return false;
        }
        if (n.querySelector(ANCHORS)) return false;
        const b = n.getBoundingClientRect(); if (!b.height || b.bottom <= line) return false;
        st.anchor = { id, off: b.top - line }; return true;
      });
    }
    for (const n of host.page().querySelectorAll<HTMLElement>("[data-e]")) {
      if (n.classList.contains("tgroup")) st.groups.add(stateKey(n));
        if (opener(n)?.getAttribute("aria-expanded") === "true") for (const step of n.querySelectorAll<HTMLElement>(".step[data-e]")) st.groupMembers.add(stateKey(step));
      if (opener(n)?.getAttribute("aria-expanded") === "true" || (n.classList.contains("event") && n.querySelector(":scope > .ev-text.open"))) st.open.add(stateKey(n));
    }
    for (const n of host.page().querySelectorAll<HTMLElement>(".hop")) if (n.querySelector(".brief.open")) st.open.add("hop:" + identOf(n));
    const a = document.activeElement;
    if (a instanceof HTMLElement && a !== document.body && !a.closest("dialog")) {
      const owner = a.id ? null : a.closest<HTMLElement>(HOSTS), sel = owner && owner !== a ? a.tagName.toLowerCase() + [...a.classList].map((c) => "." + CSS.escape(c)).join("") : null;
      // By id, else by its keyed block and place in it, else by its label, else by its place among the page's controls.
      st.focus = { id: a.id || null, host: owner ? identOf(owner) : null, sel, i: sel && owner ? [...owner.querySelectorAll<HTMLElement>(sel)].indexOf(a) : 0, range: null,
        label: a.getAttribute("aria-label"), at: [...host.page().querySelectorAll<HTMLElement>(FOCUSABLE)].indexOf(a), of: host.page().querySelectorAll<HTMLElement>(FOCUSABLE).length,
        foot: a.closest("#page > .session-foot") ? a.closest<HTMLElement>("[data-foot]")?.dataset.foot ?? null : null };
      try { if ((a instanceof HTMLInputElement || a instanceof HTMLTextAreaElement) && typeof a.selectionStart === "number" && a.selectionEnd !== null) st.focus.range = [a.selectionStart, a.selectionEnd]; } catch {}
    }
    return st;
  }
  function restore(st: ScrollSnapshot, pin = false) {
    const all = (sel: string) => [...host.page().querySelectorAll<HTMLElement>(sel)], r0 = host.rendered(), revision = host.revision();
    // A new card or brief measures its "Show more" now, as its ResizeObserver would a frame later, so nothing moves after the
    // scroll position is set.
    measureSessionScreen(host.page()); measureTraceScreen(host.page());
    // Groups first (a new one opens if it holds an open step), then steps and events.
    for (const n of all(".tgroup[data-e]")) {
      const want = st.groups.has(stateKey(n)) ? st.open.has(stateKey(n)) : [...n.querySelectorAll<HTMLElement>(".step[data-e]")].some((x) => st.open.has(stateKey(x)) || st.groupMembers.has(stateKey(x)));
      if (want && opener(n)!.getAttribute("aria-expanded") === "false") opener(n)!.click();
    }
    for (const n of all(".step[data-e]")) if (st.open.has(stateKey(n)) && opener(n)?.getAttribute("aria-expanded") === "false") opener(n)!.click();
    // An event opened before its size was measured: its "Show less" is shown by hand.
    for (const n of all(".event[data-e]")) if (st.open.has(stateKey(n)) && !n.querySelector(":scope > .ev-text.open")) { const m = n.querySelector<HTMLButtonElement>(":scope > .ev-more")!; m.click(); }
    for (const n of all(".hop")) if (st.open.has("hop:" + identOf(n)) && !n.querySelector(".brief.open")) { const m = n.querySelector<HTMLButtonElement>(".body > .more")!; m.click(); }
    if (st.drawer) host.restoreDrawer();
    if (st.focus) {
      let n = st.focus.id ? document.getElementById(st.focus.id) : null;
      if (!n && st.focus.host) { const owner = [...document.querySelectorAll<HTMLElement>(HOSTS)].find((x) => identOf(x) === st.focus!.host); n = owner && st.focus.sel ? owner.querySelectorAll<HTMLElement>(st.focus.sel)[st.focus.i] : owner ?? null; }
      if (!n && st.focus.label) n = [...document.querySelectorAll<HTMLElement>("#page [aria-label], #topbar [aria-label]")].find((x) => x.getAttribute("aria-label") === st.focus!.label) ?? null;
      // A session footer's item or button, by its kind: the footer's items change as the session runs and finishes, so its place
      // among the page's controls can name another one (patchSession's footer rule: else the time item, never the button).
      if (!n && st.focus.foot) { const f = host.page().querySelector<HTMLElement>(":scope > .session-foot"); n = f?.querySelector<HTMLElement>('[data-foot="' + st.focus.foot + '"]') ?? f?.querySelector<HTMLElement>('[data-foot="time"]') ?? null; }
      // By place only when it had no keyed block and the page has as many controls as before: never onto another row.
      if (!n && !st.focus.host && st.focus.at >= 0 && host.page().querySelectorAll<HTMLElement>(FOCUSABLE).length === st.focus.of) n = host.page().querySelectorAll<HTMLElement>(FOCUSABLE)[st.focus.at];
      if (n && n !== document.activeElement) { n.focus({ preventScroll: true }); if (st.focus.range) try { if (n instanceof HTMLInputElement || n instanceof HTMLTextAreaElement) n.setSelectionRange(...st.focus.range); } catch {} }
    }
    const pagingTurns = st.paging ? all(".turn") : [];
    if (st.paging) {
      // Newly prepended offscreen turns must have measured heights before placing the visible
      // entry; otherwise their lazy intrinsic estimates change after the anchor has been restored.
      for (const turn of pagingTurns) revealMeasuredTurn(turn, true);
      const heights = pagingTurns.map((turn) => turn.getBoundingClientRect().height);
      pagingTurns.forEach((turn, i) => { setGeometry(turn, "intrinsicHeight", Math.ceil(heights[i])); });
    }
    const place = (first: boolean) => {
      const sc = scroller();
      host.programmatic(() => {
        if (pin) sc.scrollTop = sc.scrollHeight;
        else {
          const paging = st.paging;
          let found: HTMLElement | undefined | null = null;
          if (paging?.anchor) found = all(".turns [data-e][data-entry-key]:not(.tgroup)").find((n) => n.dataset.entryKey === paging.anchor!.key);
          else if (!paging && st.anchor) found = anchors((n, id) => id === st.anchor!.id);
          if (found) {
            // Rows that moved from below the anchor to above it (a re-sorted list) need more room below than the page may
            // have: the page's bottom padding grows by what is missing, rather than the view sliding. The next redraw or
            // navigation drops it.
            const line = paging ? (host.phone() ? 0 : sc.getBoundingClientRect().top) : host.edge();
            const d = found.getBoundingClientRect().top - line - (paging ? paging.anchor!.off : st.anchor!.off), want = sc.scrollTop + d, room = sc.scrollHeight - sc.clientHeight;
            if (want > room + 0.5) { const page = host.page(); setGeometry(page, "paddingBottom", parseFloat(getComputedStyle(page).paddingBottom) + Math.ceil(want - room)); }
            if (d) sc.scrollTop = want;
            anchor = paging || !st.anchor ? null : { ...st.anchor, route: r0, top: sc.scrollTop };
          } else if (first) sc.scrollTop = st.top + (paging?.before ? sc.scrollHeight - paging.height : 0);
        }
      }, false);
      if (pin) anchor = null;
      host.sync();
    };
    // And once more two frames later, in case something above changed size after all (as revealTurn does).
    place(true); const placedTop = scroller().scrollTop;
    frame(() => frame(() => { for (const turn of pagingTurns) revealMeasuredTurn(turn, false); if (host.rendered() === r0 && !host.sheet() && host.revision() === revision && Math.abs(scroller().scrollTop - placedTop) < 1) place(false); })); // yield to a new scroll or jump
  }


return { capture, restore, opener, stateKey, identOf, get anchor() { return anchor; }, set anchor(value) { anchor = value; }, destroy() { if (disposed) return; disposed = true; for (const id of frames) cancelAnimationFrame(id); frames.clear(); anchor = null; } };
}

// The tooltip. Any element with a non-empty data-tip="text" gets one; a single element (#sh-tooltip) is reused for all of
// them, and the styles are .tip in viewer.css. Nothing calls into this file: a page sets or changes data-tip and the
// delegated listeners below read it when they need it, so re-rendered and live-updated nodes work without wiring.
// The text goes in through textContent only. This file is served inside both /viewer.js and /shell.js.
//
// Timing (the Radix defaults): it opens after 500 ms of hover; once one has shown, another tipped element reached within
// 300 ms shows at once; keyboard focus (:focus-visible only) shows it with no delay. It closes on pointer leave, blur, Esc,
// a click, or a scroll that moves the target. When a live update replaces the element under a shown tip, it moves to the
// replacement under the pointer (or holding focus) and stays open. On touch a tap on a static tipped element toggles its tip, and a tap
// anywhere else closes it; a tap on a button, link or other control runs the control and shows no tip.
// data-tip-clipped: show only while the element's own text is cut off (an ellipsis), for a tip that repeats visible text.
(() => {
  // One controller per page, even if a page loads both /viewer.js and /shell.js.
  if (window.__semonTooltip) return;
  window.__semonTooltip = true;
  const SHOW_DELAY = 500, SKIP_WINDOW = 300, MARGIN = 8, GAP = 8, MAX_WIDTH = 280, ID = "sh-tooltip";
  const INTERACTIVE = 'a[href], button, input, select, textarea, summary, label, [role="button"], [role="link"], [role="menuitem"], [role="tab"], [contenteditable="true"]';
  const HAS_TIP = '[data-tip]:not([data-tip=""])';

  let tip = null;        // the element, made on first use
  let target = null;     // the element the tip is open for, or scheduled for
  let opened = false;
  let timer = 0;
  let closedAt = -Infinity;   // when a shown tip last closed by moving away: the skip-delay window runs from here
  let dismissed = null;  // the target Esc or a click closed: it stays closed until the pointer leaves that element
  let pointerType = "mouse";
  let described = null;  // the aria-describedby to give back when the tip closes
  let watcher = null;
  let anchor = null;     // where the target stood when the tip opened, to tell whether a scroll moved it
  let pointer = null;    // the last mouse or pen position, to find the element a re-render put under it
  let origin = "pointer"; // what opened the tip: the pointer, keyboard focus or a touch

  const tipOf = (node) => node?.closest?.(HAS_TIP) ?? null;
  const focusVisible = (node) => { try { return node.matches(":focus-visible"); } catch { return true; } };
  const shown = (node) => node.isConnected && node.getClientRects().length > 0;
  const clipped = (node) => node.scrollWidth > node.clientWidth + 1 || node.scrollHeight > node.clientHeight + 1;

  function element() {
    if (tip) return tip;
    tip = document.createElement("div");
    tip.id = ID; tip.className = "tip"; tip.hidden = true; tip.setAttribute("role", "tooltip");
    // A popover sits in the top layer, above an open modal dialog; without popover support the z-index does the work.
    if (typeof tip.showPopover === "function") tip.setAttribute("popover", "manual");
    document.body.append(tip);
    return tip;
  }

  // Above the target by default, below when there is no room above, then centred on it and kept 8 px inside the viewport.
  function place(node) {
    const box = node.getBoundingClientRect(), width = document.documentElement.clientWidth, height = document.documentElement.clientHeight;
    tip.style.maxWidth = Math.min(MAX_WIDTH, width - 2 * MARGIN) + "px";
    tip.style.left = "0px"; tip.style.top = "0px"; // measured at the origin, so its wrapped size is known
    const size = tip.getBoundingClientRect(), need = size.height + GAP + MARGIN, room = { top: box.top, bottom: height - box.bottom };
    const side = room.top >= need ? "top" : room.bottom >= need ? "bottom" : room.top >= room.bottom ? "top" : "bottom";
    const top = side === "top" ? box.top - size.height - GAP : box.bottom + GAP;
    const left = Math.min(Math.max(box.left + box.width / 2 - size.width / 2, MARGIN), Math.max(MARGIN, width - MARGIN - size.width));
    tip.style.left = Math.round(left) + "px";
    tip.style.top = Math.round(Math.min(Math.max(top, MARGIN), Math.max(MARGIN, height - MARGIN - size.height))) + "px";
    tip.style.setProperty("--arrow", Math.round(Math.min(Math.max(box.left + box.width / 2 - left, 12), Math.max(12, size.width - 12))) + "px");
    tip.dataset.side = side;
  }

  // The target is described by the tip while it shows, unless its own name or text already says the same thing.
  function link(node, text) {
    const label = node.getAttribute("aria-label") ?? "";
    const before = node.getAttribute("aria-describedby");
    const said = (before ?? "").split(/\s+/).some((id) => id && document.getElementById(id)?.textContent?.includes(text));
    if ((label && label.includes(text)) || node.textContent?.trim() === text || said) return;
    node.setAttribute("aria-describedby", (before ? before + " " : "") + ID);
    described = { node, before };
  }
  function unlink() {
    if (!described) return;
    const { node, before } = described;
    if (before == null) node.removeAttribute("aria-describedby"); else node.setAttribute("aria-describedby", before);
    described = null;
  }

  // The tipped element that stands where a re-render took the target: under the pointer, or holding focus. A live update
  // rebuilds the top bar and the sidebar every few seconds, so a tip on one of their items must move to the new node.
  function successor() {
    const found = origin === "focus" ? tipOf(document.activeElement) : origin === "pointer" && pointer ? tipOf(document.elementFromPoint(pointer.x, pointer.y)) : null;
    return found && found !== target && shown(found) && !(found.hasAttribute("data-tip-clipped") && !clipped(found)) ? found : null;
  }

  // While a tip is open, a live re-render can remove or hide its target or change its text: follow it, or close.
  function watch() {
    if (watcher || !window.MutationObserver) return;
    watcher = new MutationObserver(() => {
      if (!opened || !target) return;
      const text = target.getAttribute("data-tip");
      if (!shown(target)) { const next = successor(); if (next) show(next, true); else hide(); return; }
      if (!text) { hide(); return; }
      if (text !== tip.textContent) { unlink(); tip.textContent = text; link(target, text); place(target); }
    });
    watcher.observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-tip", "hidden"] });
  }
  function unwatch() { watcher?.disconnect(); watcher = null; }

  function show(node, instant) {
    clearTimeout(timer); timer = 0;
    const text = node.getAttribute("data-tip");
    if (!text || !shown(node) || (node.hasAttribute("data-tip-clipped") && !clipped(node))) { if (target === node && !opened) target = null; return; }
    const box = element();
    unlink();
    target = node;
    box.textContent = text;
    box.toggleAttribute("data-instant", instant);
    box.hidden = false;
    if (box.hasAttribute("popover") && !box.matches(":popover-open")) { try { box.showPopover(); } catch { /* shown by its z-index instead */ } }
    place(node);
    const at = node.getBoundingClientRect();
    anchor = { top: at.top, left: at.left };
    link(node, text);
    opened = true;
    watch();
  }

  // `moved` keeps the skip-delay window open, for a tip that closed because the pointer or focus went elsewhere.
  function hide(moved = false) {
    clearTimeout(timer); timer = 0;
    if (opened) {
      unlink(); unwatch();
      if (tip.hasAttribute("popover") && tip.matches(":popover-open")) { try { tip.hidePopover(); } catch { /* already closed */ } }
      tip.hidden = true;
      closedAt = moved ? performance.now() : -Infinity;
    }
    opened = false; target = null;
  }

  function schedule(node) {
    target = node;
    if (performance.now() - closedAt < SKIP_WINDOW) show(node, true);
    else timer = window.setTimeout(() => { const now = shown(node) ? node : successor(); if (now) show(now, false); }, SHOW_DELAY);
  }

  document.addEventListener("pointerover", (event) => {
    pointerType = event.pointerType || "mouse";
    if (event.pointerType === "touch") return;
    pointer = { x: event.clientX, y: event.clientY };
    origin = "pointer";
    const node = tipOf(event.target);
    if (node !== dismissed) dismissed = null;
    if (node === target) return;
    if (node && node === dismissed) return;
    hide(true);
    if (node) schedule(node);
  });
  document.addEventListener("pointermove", (event) => { if (event.pointerType !== "touch") pointer = { x: event.clientX, y: event.clientY }; }, { capture: true, passive: true });
  // Leaving the window sends no pointerover to anything else.
  document.addEventListener("pointerout", (event) => { if (!event.relatedTarget && event.pointerType !== "touch") hide(true); });

  document.addEventListener("pointerdown", (event) => {
    pointerType = event.pointerType || "mouse";
    if (event.pointerType === "touch") return; // a touch tap is settled by its click, below
    if (target) dismissed = target;
    hide();
  }, true);

  // The browser scrolls a newly focused element into view after it sends focusin, so the tip waits a frame: it is placed
  // where the element ends up, and that scroll does not count as one that moved the target.
  document.addEventListener("focusin", (event) => {
    const focused = event.target, node = tipOf(focused);
    if (!node || !focusVisible(focused)) return;
    hide(true);
    target = node;
    origin = "focus";
    window.requestAnimationFrame(() => { if (target === node && document.activeElement === focused) show(node, true); });
  });
  document.addEventListener("focusout", (event) => { if (target && tipOf(event.target) === target) hide(true); });

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || !target) return;
    // Inside an open dialog, one Esc closes the tip and leaves the dialog: the next Esc closes that.
    const inDialog = opened && !!target.closest?.("dialog[open]");
    dismissed = target;
    hide();
    if (inDialog) { event.preventDefault(); event.stopPropagation(); }
  }, true);

  // Capture phase, so a control that stops the click's propagation still closes the tip.
  document.addEventListener("click", (event) => {
    if (pointerType !== "touch") return;
    const node = tipOf(event.target), wasOpen = opened && target === node;
    hide();
    if (!node || wasOpen || event.target.closest?.(INTERACTIVE) || node.closest(INTERACTIVE)) return;
    target = node;
    origin = "touch";
    show(node, true);
  }, true);

  // A scroll that carries the target away closes the tip; one that leaves it where it is (the page tailing a live
  // session behind a fixed bar) does not.
  window.addEventListener("scroll", () => {
    if (!opened || !target || !anchor) return;
    const box = target.getBoundingClientRect();
    if (Math.abs(box.top - anchor.top) > 0.5 || Math.abs(box.left - anchor.left) > 0.5) hide();
  }, { capture: true, passive: true });

  window.addEventListener("resize", () => hide());
  window.addEventListener("blur", () => hide());
  document.addEventListener("visibilitychange", () => { if (document.hidden) hide(); });
})();

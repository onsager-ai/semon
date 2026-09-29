// Select: a button that opens a list of options, in the look and behaviour of shadcn/ui's Select. On a desktop it opens a
// popover anchored to the button (flipping up when there is no room below); on a phone (max-width 760px) it opens a bottom
// sheet. It follows the WAI-ARIA APG select-only combobox pattern, and adds a search field above 8 options.
//
//   const s = SemonShell.select({ label: "Repo", options: [{ value: "", label: "All repos" }, { value: "a", label: "alpha" }],
//                                 value: "", onChange: (value) => {} });
//   parent.append(s.el);
//   s.setOptions(next);   // in place: an open list stays open, keeping its highlight and search text
//   s.setValue("a");      // the selection, without calling onChange
//   s.value, s.options, s.open(), s.close(), s.focus(), s.isOpen
//
//   SemonShell.enhance(nativeSelect)   // replaces a <select data-select data-label="Repo"> with a Select; the native select stays
//                                      // in the form (hidden) and follows the choice, so a form still submits it
//
// Everything is built with createElement and textContent; the styles are select.css (shipped inside viewer.css, which every
// page that uses the shell loads).
(() => {
  const shell = (window.SemonShell = window.SemonShell || {});
  const SVG = "http://www.w3.org/2000/svg";
  const ICON = { chevron: "M6 9l6 6 6-6", check: "M5 12.5l4.5 4.5L19 7", search: "M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4", close: "M6 6l12 12M18 6L6 18" };
  const svg = (path, cls) => {
    const node = document.createElementNS(SVG, "svg");
    node.setAttribute("viewBox", "0 0 24 24"); node.setAttribute("fill", "none"); node.setAttribute("stroke", "currentColor");
    node.setAttribute("stroke-width", "2"); node.setAttribute("stroke-linecap", "round"); node.setAttribute("stroke-linejoin", "round");
    node.setAttribute("aria-hidden", "true"); if (cls) node.setAttribute("class", cls);
    const p = document.createElementNS(SVG, "path"); p.setAttribute("d", path); node.append(p); return node;
  };
  const make = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text != null) n.textContent = text; return n; };
  const phone = () => window.matchMedia?.("(max-width: 760px)").matches === true;
  const printable = (e) => e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey;
  let serial = 0;

  // A phone's sheet is a history entry, like the viewer's own sheets: Back closes the list instead of leaving the page. `sheet` is
  // the open one; `swallow` counts the popstates that closing it by hand causes (history.back) so no one else sees them. This
  // listener is registered before the viewer's (select.js comes first), and stops the event when it is the sheet's.
  let sheet = null, swallow = 0;
  window.addEventListener("popstate", (e) => {
    if (swallow) { swallow--; e.stopImmediatePropagation(); return; }
    if (sheet && !orphaned()) { e.stopImmediatePropagation(); sheet.popped(); }
  });
  // The page behind a sheet does not scroll while it is open. Not by overflow: hidden on the page (a CI run showed that resets a phone
  // page to the top, and the offset can't be put back while it is hidden), but by refusing the wheel and touch moves that don't
  // start in the list; the list's own overscroll-behavior keeps it from chaining on to the page.
  let sheetList = null, touchY = 0;
  // Whether the sheet's page went away while it was open (a navigation removed the Select): the lock and the history hold end then,
  // instead of a detached list refusing every scroll and swallowing a Back.
  const orphaned = () => { if (!sheetList || sheetList.isConnected) return false; lock(false); sheet = null; return true; };
  // A wheel or touch move is refused unless it starts in the list and the list can move that way: a list with nothing to scroll, or
  // one at its end, would otherwise hand the move on to the page (overscroll-behavior is not enough on every browser).
  const refuse = (e) => {
    if (orphaned() || !sheetList) return;
    if (e.type === "touchstart") { touchY = e.touches[0]?.clientY ?? 0; return; }
    if (!sheetList.contains(e.target)) { e.preventDefault(); return; }
    const room = sheetList.scrollHeight - sheetList.clientHeight, delta = e.type === "wheel" ? e.deltaY : touchY - (e.touches[0]?.clientY ?? touchY); // > 0: toward the end
    if (room <= 1 || (delta > 0 && sheetList.scrollTop >= room - 1) || (delta < 0 && sheetList.scrollTop <= 0)) e.preventDefault();
  };
  const lock = (on, list) => {
    const method = on ? "addEventListener" : "removeEventListener"; sheetList = on ? list : null;
    for (const type of ["wheel", "touchmove"]) document[method](type, refuse, { capture: true, passive: false });
    document[method]("touchstart", refuse, { capture: true, passive: true });
  };

  function select(config = {}) {
    const n = ++serial, listId = "sh-select-list-" + n, labelId = "sh-select-label-" + n, valueId = "sh-select-value-" + n;
    const label = config.label ?? "", searchAbove = config.searchAbove ?? 8;
    let options = [], value = "", open = null, query = "", active = null, typed = "", typedAt = 0;
    const cache = new Map(); // option value -> its element, kept while the option exists

    const root = make("div", "sh-select"); if (label) root.dataset.label = label;
    const trigger = make("button", "sh-select-trigger"); trigger.type = "button";
    trigger.setAttribute("role", "combobox"); trigger.setAttribute("aria-haspopup", "listbox"); trigger.setAttribute("aria-expanded", "false");
    trigger.setAttribute("aria-controls", listId); trigger.setAttribute("aria-labelledby", (label ? labelId + " " : "") + valueId);
    const text = make("span", "sh-select-text"), labelEl = make("span", "sh-select-label", label ? label + ":" : ""), valueEl = make("span", "sh-select-value");
    labelEl.id = labelId; valueEl.id = valueId; text.setAttribute("data-tip-clipped", ""); text.append(labelEl); if (label) text.append(" "); text.append(valueEl);
    trigger.append(text, svg(ICON.chevron, "sh-select-chevron"));

    // The parts the list is made of, moved into the popover or the sheet while it is open and parked (hidden) otherwise, so
    // the trigger's aria-controls always names an element.
    const park = make("div"); park.hidden = true;
    const list = make("div", "sh-select-list"); list.id = listId; list.setAttribute("role", "listbox"); if (label) list.setAttribute("aria-label", label); list.tabIndex = -1;
    const empty = make("div", "sh-select-empty", "No matches"); empty.hidden = true; empty.setAttribute("role", "status");
    const searchBox = make("div", "sh-select-search"), search = make("input"); search.type = "search"; search.autocomplete = "off"; search.spellcheck = false;
    search.placeholder = "Search"; search.setAttribute("aria-label", "Search " + (label || "options"));
    // The field drives the list while it is open, so it is a combobox too (the highlight is its aria-activedescendant).
    search.setAttribute("role", "combobox"); search.setAttribute("aria-autocomplete", "list"); search.setAttribute("aria-expanded", "true"); search.setAttribute("aria-controls", listId);
    searchBox.append(svg(ICON.search), search);
    const pop = make("div", "sh-select-pop"); pop.hidden = true;
    let dialog = null; // the bottom sheet, made the first time a phone opens the list
    root.append(trigger, pop, park); park.append(searchBox, list, empty);

    const shown = () => { const q = query.trim().toLowerCase(); return q ? options.filter((o) => o.label.toLowerCase().includes(q)) : options; };
    const textOf = (v) => options.find((o) => o.value === v)?.label ?? "";
    const paintTrigger = () => { valueEl.textContent = textOf(value); text.dataset.tip = (label ? label + ": " : "") + valueEl.textContent; };
    const owner = () => (!open ? null : open.sheet ? list : open.search ? search : trigger);

    function paintList() {
      const items = shown(); let cur = list.firstChild;
      const put = (node) => { if (node === cur) cur = cur.nextSibling; else list.insertBefore(node, cur); };
      for (const o of items) {
        let node = cache.get(o.value);
        if (!node) {
          node = make("div", "sh-select-option"); node.setAttribute("role", "option"); node.append(svg(ICON.check, "sh-select-check"), make("span", "sh-select-option-text"));
          node.addEventListener("pointerdown", (e) => e.preventDefault()); // a click keeps the focus where it is
          node.addEventListener("pointermove", () => { if (active !== node.dataset.value) setActive(node.dataset.value, false); });
          node.addEventListener("click", () => choose(node.dataset.value));
          cache.set(o.value, node);
        }
        node.id = listId + "-" + options.indexOf(o); node.dataset.value = o.value;
        const span = node.lastChild; if (span.textContent !== o.label) span.textContent = o.label;
        node.setAttribute("aria-selected", String(o.value === value)); put(node);
      }
      while (cur) { const next = cur.nextSibling; cur.remove(); cur = next; }
      empty.hidden = items.length > 0;
    }
    function setActive(v, scroll = true) {
      active = v; const node = v == null ? null : cache.get(v);
      for (const o of list.children) o.classList.toggle("sh-active", o === node);
      const target = owner(); if (!target) return;
      if (node?.isConnected) { target.setAttribute("aria-activedescendant", node.id); if (scroll) node.scrollIntoView({ block: "nearest" }); } else target.removeAttribute("aria-activedescendant");
    }
    // The option to highlight when the list has just been drawn: the one already highlighted, else the selected one, else the first.
    const settle = () => { const items = shown(); setActive(items.some((o) => o.value === active) ? active : items.some((o) => o.value === value) ? value : (items[0]?.value ?? null), true); };
    const move = (to) => { const items = shown(); if (!items.length) return; const at = items.findIndex((o) => o.value === active); setActive(items[Math.max(0, Math.min(items.length - 1, to(at, items.length)))].value); };

    // The popover sits under the trigger, or above it when the room below is smaller than the list and the room above is larger;
    // it never leaves the viewport.
    function position() {
      if (!open || open.sheet) return;
      const r = trigger.getBoundingClientRect(), vw = document.documentElement.clientWidth, vh = window.innerHeight, gap = 4, margin = 8;
      pop.style.left = "0px"; pop.style.top = "0px"; pop.style.maxHeight = ""; pop.style.minWidth = Math.max(r.width, 200) + "px";
      // Never higher than the sticky top bar's bottom edge, which the list would otherwise slide under.
      const bar = document.getElementById("topbar"), floor = Math.max(margin, bar ? bar.getBoundingClientRect().bottom + gap : 0);
      const width = Math.min(pop.offsetWidth, vw - 2 * margin), height = pop.offsetHeight, below = vh - r.bottom - gap - margin, above = r.top - gap - floor;
      const up = height > below && above > below, room = Math.max(48, Math.min(up ? above : below, 360));
      pop.style.maxHeight = room + "px";
      pop.style.left = Math.max(margin, Math.min(r.left, vw - width - margin)) + "px";
      pop.style.top = Math.max(floor, up ? r.top - gap - Math.min(height, room) : r.bottom + gap) + "px";
      pop.dataset.side = up ? "top" : "bottom";
    }
    const outside = (e) => { if (open && !root.contains(e.target)) close(false); };
    const reposition = (e) => { if (!pop.contains(e.target)) position(); };

    function makeSheet() {
      const d = make("dialog", "sh-select-sheet"); d.setAttribute("aria-label", label || "Options");
      const head = make("div", "sh-select-sheet-h"), close_ = make("button", "sh-select-close"); close_.type = "button"; close_.setAttribute("aria-label", "Close"); close_.append(svg(ICON.close));
      head.append(make("span", "sh-select-sheet-title", label), close_);
      close_.addEventListener("click", () => close(true));
      d.addEventListener("click", (e) => { if (e.target === d) close(true); });
      d.addEventListener("close", () => { if (open && !d.open) close(true); }); // Escape closes a dialog by itself
      d.head = head; return d;
    }
    function openList() {
      if (open) return;
      const asSheet = phone() && typeof HTMLDialogElement !== "undefined" && typeof HTMLDialogElement.prototype.showModal === "function";
      const withSearch = options.length > searchAbove;
      query = ""; search.value = ""; searchBox.hidden = !withSearch; open = { sheet: asSheet, search: withSearch };
      trigger.setAttribute("aria-expanded", "true"); paintList();
      if (asSheet) {
        dialog ??= makeSheet(); dialog.head.firstChild.textContent = label; dialog.replaceChildren(dialog.head, searchBox, list, empty); root.append(dialog); dialog.showModal(); lock(true, list);
        try { history.pushState({ ...history.state, shSelect: n }, ""); swallow = 0; open.entry = true; sheet = { popped: () => { open.entry = false; close(true); } }; } catch {}
        settle(); list.focus({ preventScroll: true });
      } else {
        pop.replaceChildren(searchBox, list, empty); pop.hidden = false; position(); settle();
        document.addEventListener("pointerdown", outside, true); window.addEventListener("resize", position); document.addEventListener("scroll", reposition, true);
        if (withSearch) search.focus({ preventScroll: true });
      }
    }
    function close(refocus = true) {
      if (!open) return;
      const was = open; open = null; typed = "";
      trigger.setAttribute("aria-expanded", "false"); for (const n of [trigger, search, list]) n.removeAttribute("aria-activedescendant");
      if (was.sheet) {
        // The dialog closes first (it makes the rest of the page inert, and hands focus back to the page).
        if (dialog.open) dialog.close(); dialog.remove(); lock(false); sheet = null;
        if (was.entry) { swallow++; history.back(); }
      } else {
        // The button takes the focus before the search field leaves the page.
        if (refocus) trigger.focus({ preventScroll: true });
        pop.hidden = true; document.removeEventListener("pointerdown", outside, true); window.removeEventListener("resize", position); document.removeEventListener("scroll", reposition, true);
      }
      park.append(searchBox, list, empty);
      if (refocus) trigger.focus({ preventScroll: true });
    }
    function choose(v) {
      const changed = v !== value; close(true);
      if (changed) { value = v; paintTrigger(); config.onChange?.(value, api); }
    }

    // Keys. The trigger, the search field (above 8 options) and, in the sheet, the list all take the same ones; the highlight
    // is announced with aria-activedescendant on whichever of them has the focus.
    function typeahead(ch) {
      const now = Date.now(); typed = now - typedAt > 500 ? ch : typed + ch; typedAt = now;
      const items = shown(); if (!items.length) return;
      // One letter (or the same letter again and again) steps through the options that start with it; more letters narrow the match.
      const at = items.findIndex((o) => o.value === active), cycle = [...typed].every((c) => c === typed[0]);
      const key = cycle ? typed[0] : typed, from = cycle ? at + 1 : Math.max(0, at);
      const hit = [...items.slice(from), ...items.slice(0, from)].find((o) => o.label.toLowerCase().startsWith(key)); if (hit) setActive(hit.value);
    }
    // A letter typed while the list has a search field goes into the field (opening the list first, and moving focus to the field).
    function toSearch(text) {
      search.focus({ preventScroll: true }); search.value += text; query = search.value; search.setSelectionRange(query.length, query.length);
      paintList(); settle(); position();
    }
    function onKey(e) {
      const k = e.key, inField = e.target === search;
      if (!open) {
        if (k === "ArrowDown" || k === "ArrowUp" || k === "Enter" || k === " ") { e.preventDefault(); openList(); }
        else if (k === "Home" || k === "End") { e.preventDefault(); openList(); move((_, len) => (k === "Home" ? 0 : len - 1)); }
        else if (printable(e)) { e.preventDefault(); openList(); if (open.search) toSearch(k); else typeahead(k.toLowerCase()); }
        return;
      }
      if (k === "Escape") { e.preventDefault(); e.stopPropagation(); close(true); }
      else if (k === "ArrowDown") { e.preventDefault(); if (e.altKey) return; move((at) => at + 1); }
      else if (k === "ArrowUp") { e.preventDefault(); if (e.altKey) { if (active != null) choose(active); else close(true); } else move((at, len) => (at < 0 ? len - 1 : at - 1)); }
      else if (k === "PageDown") { e.preventDefault(); move((at) => at + 10); }
      else if (k === "PageUp") { e.preventDefault(); move((at) => at - 10); }
      else if ((k === "Home" || k === "End") && !inField) { e.preventDefault(); move((_, len) => (k === "Home" ? 0 : len - 1)); }
      else if (k === "Enter") { e.preventDefault(); if (active != null) choose(active); else close(true); }
      else if (k === " " && !inField) { e.preventDefault(); if (Date.now() - typedAt < 500 && typed) typeahead(" "); else if (active != null) choose(active); }
      // Tab picks the highlighted option and moves on: focus goes to the button first, so the Tab itself carries it to the next control.
      else if (k === "Tab") { if (open.sheet) return; const v = active; close(true); if (v != null && v !== value) { value = v; paintTrigger(); config.onChange?.(value, api); } }
      else if (printable(e) && !inField) { e.preventDefault(); if (open.search) toSearch(k); else typeahead(k.toLowerCase()); }
    }
    trigger.addEventListener("keydown", onKey); search.addEventListener("keydown", onKey); list.addEventListener("keydown", onKey);
    trigger.addEventListener("keyup", (e) => { if (e.key === " ") e.preventDefault(); }); // a space must not also click the button
    trigger.addEventListener("click", () => (open ? close(true) : openList()));
    pop.addEventListener("mousedown", (e) => { if (e.target !== search) e.preventDefault(); });
    root.addEventListener("focusout", (e) => { if (open && !open.sheet && !root.contains(e.relatedTarget)) close(false); });
    search.addEventListener("input", () => { query = search.value; paintList(); settle(); position(); });

    const setOptions = (next) => {
      options = next.map((o) => ({ value: String(o.value), label: String(o.label ?? o.value) }));
      const keep = new Set(options.map((o) => o.value)); for (const [v, node] of cache) if (!keep.has(v)) { node.remove(); cache.delete(v); }
      paintTrigger(); if (open) { paintList(); settle(); position(); }
    };
    const api = {
      el: root,
      get value() { return value; },
      get options() { return options.map((o) => ({ ...o })); },
      get isOpen() { return !!open; },
      setOptions,
      setValue(v) { value = String(v ?? ""); paintTrigger(); if (open) paintList(); },
      open: openList, close, focus: () => trigger.focus(),
    };
    root.semonSelect = api;
    value = String(config.value ?? ""); setOptions(config.options ?? []);
    return api;
  }

  function enhance(native) {
    if (!(native instanceof HTMLSelectElement) || native.dataset.selectReady != null) return null;
    const api = select({ label: native.dataset.label ?? native.getAttribute("aria-label") ?? "", value: native.value,
      options: [...native.options].map((o) => ({ value: o.value, label: o.textContent.trim() })),
      onChange: (v) => { native.value = v; native.dispatchEvent(new Event("change", { bubbles: true })); } });
    native.dataset.selectReady = ""; native.hidden = true; native.after(api.el); return api;
  }

  shell.select = select; shell.enhance = enhance;
  const run = () => document.querySelectorAll("select[data-select]").forEach(enhance);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", run); else run();
})();

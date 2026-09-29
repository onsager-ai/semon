// crates/semon-sessions/src/tooltip.js, typed: the same behaviour, line for line. It is a behaviour module with no
// components (delegated listeners read data-tip when they need it), so every runtime's sample imports this one copy and
// the bundles differ only in what the runtime adds. See tooltip.js for the timing and touch rules it implements.

declare global {
  interface Window {
    __semonTooltip?: boolean;
  }
}

type Origin = "pointer" | "focus" | "touch";
interface Point {
  x: number;
  y: number;
}
interface Dismissed {
  node: Element;
  text: string | null;
  x: number | undefined;
  y: number | undefined;
}

export function installTooltip(): void {
  // One controller per page, even if a page loads both /viewer.js and /shell.js.
  if (window.__semonTooltip) return;
  window.__semonTooltip = true;
  const SHOW_DELAY = 500, SKIP_WINDOW = 300, MARGIN = 8, GAP = 8, MAX_WIDTH = 280, ID = "sh-tooltip";
  const INTERACTIVE = 'a[href], button, input, select, textarea, summary, label, [role="button"], [role="link"], [role="menuitem"], [role="tab"], [contenteditable="true"]';
  const HAS_TIP = '[data-tip]:not([data-tip=""])';

  let tip: HTMLDivElement | null = null;
  let target: Element | null = null;
  let opened = false;
  let timer = 0;
  let closedAt = -Infinity;
  let dismissed: Dismissed | null = null;
  let pointerType = "mouse";
  let described: { node: Element; before: string | null } | null = null;
  let watcher: MutationObserver | null = null;
  let anchor: { top: number; left: number } | null = null;
  let pointer: Point | null = null;
  let origin: Origin = "pointer";

  const tipOf = (node: EventTarget | Element | null | undefined): Element | null => (node instanceof Element ? node.closest(HAS_TIP) : null);
  const focusVisible = (node: Element): boolean => {
    try {
      return node.matches(":focus-visible");
    } catch {
      return true;
    }
  };
  const shown = (node: Element): boolean => node.isConnected && node.getClientRects().length > 0;
  const dismiss = (): void => {
    dismissed = target ? { node: target, text: target.getAttribute("data-tip"), x: pointer?.x, y: pointer?.y } : dismissed;
  };
  const isDismissed = (node: Element | null): boolean =>
    !!dismissed && !!node && (node === dismissed.node || (pointer != null && pointer.x === dismissed.x && pointer.y === dismissed.y && node.getAttribute("data-tip") === dismissed.text));
  const clipped = (node: Element): boolean => node.scrollWidth > node.clientWidth + 1 || node.scrollHeight > node.clientHeight + 1;

  function element(): HTMLDivElement {
    if (tip) return tip;
    tip = document.createElement("div");
    tip.id = ID;
    tip.className = "tip";
    tip.hidden = true;
    tip.setAttribute("role", "tooltip");
    if (typeof tip.showPopover === "function") tip.setAttribute("popover", "manual");
    document.body.append(tip);
    return tip;
  }

  function place(node: Element, box: HTMLDivElement): void {
    const at = node.getBoundingClientRect(), width = document.documentElement.clientWidth, height = document.documentElement.clientHeight;
    box.style.maxWidth = Math.min(MAX_WIDTH, width - 2 * MARGIN) + "px";
    box.style.left = "0px";
    box.style.top = "0px";
    const size = box.getBoundingClientRect(), need = size.height + GAP + MARGIN, room = { top: at.top, bottom: height - at.bottom };
    const side = room.top >= need ? "top" : room.bottom >= need ? "bottom" : room.top >= room.bottom ? "top" : "bottom";
    const top = side === "top" ? at.top - size.height - GAP : at.bottom + GAP;
    const left = Math.min(Math.max(at.left + at.width / 2 - size.width / 2, MARGIN), Math.max(MARGIN, width - MARGIN - size.width));
    box.style.left = Math.round(left) + "px";
    box.style.top = Math.round(Math.min(Math.max(top, MARGIN), Math.max(MARGIN, height - MARGIN - size.height))) + "px";
    box.style.setProperty("--arrow", Math.round(Math.min(Math.max(at.left + at.width / 2 - left, 12), Math.max(12, size.width - 12))) + "px");
    box.dataset.side = side;
  }

  function link(node: Element, text: string): void {
    const label = node.getAttribute("aria-label") ?? "";
    const before = node.getAttribute("aria-describedby");
    const said = (before ?? "").split(/\s+/).some((id) => id && document.getElementById(id)?.textContent?.includes(text));
    if ((label && label.includes(text)) || node.textContent?.trim() === text || said) return;
    node.setAttribute("aria-describedby", (before ? before + " " : "") + ID);
    described = { node, before };
  }
  function unlink(): void {
    if (!described) return;
    const { node, before } = described;
    if (before == null) node.removeAttribute("aria-describedby");
    else node.setAttribute("aria-describedby", before);
    described = null;
  }

  function successor(): Element | null {
    const found = origin === "focus" ? tipOf(document.activeElement) : origin === "pointer" && pointer ? tipOf(document.elementFromPoint(pointer.x, pointer.y)) : null;
    return found && found !== target && shown(found) && !(found.hasAttribute("data-tip-clipped") && !clipped(found)) ? found : null;
  }

  function watch(): void {
    if (watcher || !window.MutationObserver) return;
    watcher = new MutationObserver(() => {
      if (!opened || !target || !tip) return;
      const text = target.getAttribute("data-tip");
      if (!shown(target)) {
        const next = successor();
        if (next) show(next, true);
        else hide();
        return;
      }
      if (!text) {
        hide();
        return;
      }
      if (text !== tip.textContent) {
        unlink();
        tip.textContent = text;
        link(target, text);
        place(target, tip);
      }
    });
    watcher.observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-tip", "hidden"] });
  }
  function unwatch(): void {
    watcher?.disconnect();
    watcher = null;
  }

  function show(node: Element, instant: boolean): void {
    clearTimeout(timer);
    timer = 0;
    const text = node.getAttribute("data-tip");
    if (!text || !shown(node) || (node.hasAttribute("data-tip-clipped") && !clipped(node))) {
      if (target === node && !opened) target = null;
      return;
    }
    const box = element();
    unlink();
    target = node;
    box.textContent = text;
    box.toggleAttribute("data-instant", instant);
    box.hidden = false;
    if (box.hasAttribute("popover") && !box.matches(":popover-open")) {
      try {
        box.showPopover();
      } catch {
        /* shown by its z-index instead */
      }
    }
    place(node, box);
    const at = node.getBoundingClientRect();
    anchor = { top: at.top, left: at.left };
    link(node, text);
    opened = true;
    watch();
  }

  function hide(moved = false): void {
    clearTimeout(timer);
    timer = 0;
    if (opened && tip) {
      unlink();
      unwatch();
      if (tip.hasAttribute("popover") && tip.matches(":popover-open")) {
        try {
          tip.hidePopover();
        } catch {
          /* already closed */
        }
      }
      tip.hidden = true;
      closedAt = moved ? performance.now() : -Infinity;
    }
    opened = false;
    target = null;
  }

  function schedule(node: Element): void {
    target = node;
    if (performance.now() - closedAt < SKIP_WINDOW) show(node, true);
    else
      timer = window.setTimeout(() => {
        const now = shown(node) ? node : successor();
        if (now) show(now, false);
      }, SHOW_DELAY);
  }

  document.addEventListener("pointerover", (event) => {
    pointerType = event.pointerType || "mouse";
    if (event.pointerType === "touch") return;
    pointer = { x: event.clientX, y: event.clientY };
    origin = "pointer";
    const node = tipOf(event.target);
    if (!isDismissed(node)) dismissed = null;
    if (node === target) return;
    if (node && isDismissed(node)) return;
    hide(true);
    if (node) schedule(node);
  });
  document.addEventListener(
    "pointermove",
    (event) => {
      if (event.pointerType === "touch") return;
      const over = dismissed ? tipOf(event.target) : null;
      if (over && dismissed && isDismissed(over)) {
        dismissed.node = over;
        dismissed.x = event.clientX;
        dismissed.y = event.clientY;
      }
      pointer = { x: event.clientX, y: event.clientY };
    },
    { capture: true, passive: true },
  );
  document.addEventListener("pointerout", (event) => {
    if (!event.relatedTarget && event.pointerType !== "touch") hide(true);
  });

  document.addEventListener(
    "pointerdown",
    (event) => {
      pointerType = event.pointerType || "mouse";
      if (event.pointerType === "touch") return;
      dismiss();
      hide();
    },
    true,
  );

  document.addEventListener("focusin", (event) => {
    const focused = event.target, node = tipOf(focused);
    if (!node || !(focused instanceof Element) || !focusVisible(focused)) return;
    hide(true);
    target = node;
    origin = "focus";
    window.requestAnimationFrame(() => {
      if (target === node && document.activeElement === focused) show(node, true);
    });
  });
  document.addEventListener("focusout", (event) => {
    if (target && tipOf(event.target) === target) hide(true);
  });

  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key !== "Escape" || !target) return;
      const inDialog = opened && !!target.closest("dialog[open]");
      dismiss();
      hide();
      if (inDialog) {
        event.preventDefault();
        event.stopPropagation();
      }
    },
    true,
  );

  document.addEventListener(
    "click",
    (event) => {
      if (pointerType !== "touch") return;
      const node = tipOf(event.target), wasOpen = opened && target === node;
      hide();
      const hit = event.target instanceof Element ? event.target : null;
      if (!node || wasOpen || hit?.closest(INTERACTIVE) || node.closest(INTERACTIVE)) return;
      target = node;
      origin = "touch";
      show(node, true);
    },
    true,
  );

  window.addEventListener(
    "scroll",
    () => {
      if (!opened || !target || !anchor) return;
      const box = target.getBoundingClientRect();
      if (Math.abs(box.top - anchor.top) > 0.5 || Math.abs(box.left - anchor.left) > 0.5) hide();
    },
    { capture: true, passive: true },
  );

  window.addEventListener("resize", () => hide());
  window.addEventListener("blur", () => hide());
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) hide();
  });
}

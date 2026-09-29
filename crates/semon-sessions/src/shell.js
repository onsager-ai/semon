(() => {
  const phone = window.matchMedia?.("(max-width: 760px)") ?? { matches: false };
  const lead = document.getElementById("lead-btn");
  const sidebar = document.getElementById("sidebar");
  const drawerClose = document.getElementById("drawer-close");
  const scrim = document.getElementById("scrim");

  function openDrawer() {
    if (!phone.matches || !sidebar) return;
    document.body.classList.add("drawer-open");
    lead?.setAttribute("aria-expanded", "true");
  }

  function closeDrawer(restoreFocus = true) {
    if (!document.body.classList.contains("drawer-open")) return;
    document.body.classList.remove("drawer-open");
    lead?.setAttribute("aria-expanded", "false");
    if (restoreFocus) lead?.focus();
  }

  lead?.addEventListener("click", openDrawer);
  drawerClose?.addEventListener("click", () => closeDrawer());
  scrim?.addEventListener("click", () => closeDrawer());
  // Esc closes an open popover first (the browser does that after this listener), and the drawer only when none is open, so
  // a menu opened from the drawer closes on its own. A browser without popovers has none open.
  const popoverOpen = () => { try { return !!document.querySelector(":popover-open"); } catch { return false; } };
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !popoverOpen()) closeDrawer();
  });

  let touchStart = null;
  sidebar?.addEventListener("touchstart", (event) => {
    touchStart = event.touches[0]?.clientX ?? null;
  }, { passive: true });
  sidebar?.addEventListener("touchmove", (event) => {
    const x = event.touches[0]?.clientX;
    if (phone.matches && touchStart !== null && x !== undefined && x - touchStart < -50) {
      touchStart = null;
      closeDrawer();
    }
  }, { passive: true });
  phone.addEventListener?.("change", () => closeDrawer(false));

  const topbar = document.getElementById("topbar");
  const main = document.getElementById("main");
  function syncBarLine() {
    if (!topbar) return;
    const y = phone.matches ? window.scrollY : (main?.scrollTop ?? 0);
    topbar.classList.toggle("scrolled", y > 4);
  }
  window.addEventListener("scroll", syncBarLine, { passive: true });
  main?.addEventListener("scroll", syncBarLine, { passive: true });
  syncBarLine();

  const copyTimers = new WeakMap();
  function selectText(element) {
    const selection = window.getSelection();
    if (!selection) return;
    const range = document.createRange();
    range.selectNodeContents(element);
    selection.removeAllRanges();
    selection.addRange(range);
  }
  function copied(button) {
    if (!button.dataset.copyLabel) {
      button.dataset.copyLabel = button.getAttribute("aria-label") || button.textContent.trim() || "Copy";
    }
    button.setAttribute("aria-label", "Copied");
    button.classList.add("copied");
    window.clearTimeout(copyTimers.get(button));
    copyTimers.set(button, window.setTimeout(() => {
      button.setAttribute("aria-label", button.dataset.copyLabel);
      button.classList.remove("copied");
      copyTimers.delete(button);
    }, 1500));
  }
  document.addEventListener("click", async (event) => {
    const button = event.target?.closest?.("[data-copy]");
    if (!button) return;
    const source = document.getElementById(button.dataset.copy);
    if (!source) return;
    const value = source.textContent ?? "";
    if (navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(value);
      } catch {
        selectText(source);
      }
    } else {
      selectText(source);
    }
    copied(button);
  });

  const waitingForms = new WeakMap();
  let submittingConfirmed = null;
  document.addEventListener("submit", (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !form.matches("form[data-confirm]")) return;
    if (submittingConfirmed === form) return;
    const dialog = document.getElementById(form.dataset.confirm);
    if (!(dialog instanceof HTMLDialogElement) || typeof dialog.showModal !== "function") return;
    event.preventDefault();
    waitingForms.set(dialog, form);
    if (!dialog.open) dialog.showModal();
  });

  function submitConfirmed(form) {
    const previous = submittingConfirmed;
    submittingConfirmed = form;
    try {
      if (typeof form.requestSubmit === "function") form.requestSubmit();
      else form.submit();
    } finally {
      submittingConfirmed = previous;
    }
  }

  document.addEventListener("click", (event) => {
    const target = event.target;
    const openButton = target?.closest?.("button[data-open]");
    if (openButton) {
      const dialog = document.getElementById(openButton.dataset.open);
      if (dialog instanceof HTMLDialogElement && !dialog.open) dialog.showModal?.();
      return;
    }

    const closeButton = target?.closest?.("[data-close]");
    if (closeButton) {
      closeButton.closest("dialog")?.close();
      return;
    }

    const dialog = target?.closest?.("dialog");
    if (dialog instanceof HTMLDialogElement && target === dialog) {
      if (event.clientX === 0 && event.clientY === 0) return;
      const box = dialog.getBoundingClientRect();
      if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) dialog.close();
      return;
    }

    const button = target?.closest?.("button");
    const parentDialog = button?.closest?.("dialog");
    if (!(parentDialog instanceof HTMLDialogElement) || !waitingForms.has(parentDialog)) return;
    const form = waitingForms.get(parentDialog);
    waitingForms.delete(parentDialog);
    if (button.value === "confirm") {
      event.preventDefault();
      parentDialog.close("confirm");
      if (form) submitConfirmed(form);
    } else {
      event.preventDefault();
      parentDialog.close("cancel");
    }
  });

  document.querySelectorAll("dialog").forEach((dialog) => {
    dialog.addEventListener("close", () => waitingForms.delete(dialog));
  });

  document.querySelectorAll("[data-poll]").forEach((element) => {
    const url = element.dataset.poll;
    if (!url) return;
    const deadline = Date.now() + 30 * 60 * 1000;
    let pending = false;
    const interval = window.setInterval(async () => {
      if (Date.now() >= deadline) {
        window.clearInterval(interval);
        return;
      }
      if (pending) return;
      pending = true;
      try {
        const response = await fetch(url, { credentials: "same-origin", cache: "no-store" });
        if (Date.now() < deadline && response.status === 200) {
          window.clearInterval(interval);
          window.location.assign(element.dataset.pollGo || "/");
        }
      } catch {
        // A later poll can recover from a temporary network error.
      } finally {
        pending = false;
      }
    }, 3000);
    window.setTimeout(() => window.clearInterval(interval), 30 * 60 * 1000);
  });
})();

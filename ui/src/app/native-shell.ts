import { EffectScope } from './effects';
import { mountComposers } from '../lib/composer';
import { createStatusNote } from '../lib';
let current: { destroy(): void } | null = null;
/** Native Rust pages keep their forms and fallback markup; this owner only adds enhancements. */
export interface NativeShellOptions {
  /** A viewer host already owns drawer and bar chrome. */ chrome?: boolean;
}
export function mountNativeShell(options: NativeShellOptions = {}) {
  current?.destroy();
  const effects = new EffectScope();
  const composers = mountComposers();
  let disposed = false;
  const dialogs = new Set<HTMLDialogElement>();

  let releaseChrome = () => {};
  if (options.chrome !== false) {
    const phone = window.matchMedia('(max-width: 760px)');
    const lead = document.getElementById('lead-btn');
    const sidebar = document.getElementById('sidebar');
    const drawerClose = document.getElementById('drawer-close');
    const scrim = document.getElementById('scrim');

    function openDrawer() {
      if (!phone.matches || !sidebar) return;
      // Named before it opens, so the viewer's sidebar on an embedding page can re-sort what its list held, as the viewer's own drawer does.
      window.dispatchEvent(new Event('semon:drawer-open'));
      document.body.classList.add('drawer-open');
      lead?.setAttribute('aria-expanded', 'true');
    }

    function closeDrawer(restoreFocus = true) {
      if (!document.body.classList.contains('drawer-open')) return;
      // A popover opened from the drawer (its account menu) goes with it, or it would float over the page on its own.
      try {
        sidebar
          ?.querySelectorAll(':popover-open')
          .forEach((popover) => popover instanceof HTMLElement && popover.hidePopover());
      } catch {
        /* no popovers here */
      }
      document.body.classList.remove('drawer-open');
      lead?.setAttribute('aria-expanded', 'false');
      if (restoreFocus) lead?.focus();
    }

    lead && effects.listen(lead, 'click', openDrawer);
    drawerClose && effects.listen(drawerClose, 'click', () => closeDrawer());
    scrim && effects.listen(scrim, 'click', () => closeDrawer());
    // Esc closes an open popover first (the browser does that after this listener), and the drawer only when none is open, so
    // a menu opened from the drawer closes on its own. A browser without popovers has none open.
    const popoverOpen = () => {
      try {
        return !!document.querySelector(':popover-open');
      } catch {
        return false;
      }
    };
    effects.listen(document, 'keydown', (event) => {
      if (event.key === 'Escape' && !popoverOpen() && !document.querySelector('dialog[open]'))
        closeDrawer(); // an open dialog takes its own Esc
    });

    let touchStart: number | null = null;
    sidebar &&
      effects.listen(
        sidebar,
        'touchstart',
        (event) => {
          touchStart = event.touches[0]?.clientX ?? null;
        },
        { passive: true },
      );
    sidebar &&
      effects.listen(
        sidebar,
        'touchmove',
        (event) => {
          const x = event.touches[0]?.clientX;
          if (phone.matches && touchStart !== null && x !== undefined && x - touchStart < -50) {
            touchStart = null;
            closeDrawer();
          }
        },
        { passive: true },
      );
    effects.listen(phone, 'change', () => closeDrawer(false));

    const topbar = document.getElementById('topbar');
    const main = document.getElementById('main');
    function syncBarLine() {
      if (!topbar) return;
      const y = phone.matches ? window.scrollY : (main?.scrollTop ?? 0);
      topbar.classList.toggle('scrolled', y > 4);
    }
    effects.listen(window, 'scroll', syncBarLine, { passive: true });
    main && effects.listen(main, 'scroll', syncBarLine, { passive: true });
    syncBarLine();

    releaseChrome = () => {
      closeDrawer(false);
      topbar?.classList.remove('scrolled');
    };
  }

  const copyTimers = new Map<HTMLElement, number>();
  function selectText(element: HTMLElement) {
    const selection = window.getSelection();
    if (!selection) return;
    const range = document.createRange();
    range.selectNodeContents(element);
    selection.removeAllRanges();
    selection.addRange(range);
  }
  function copied(button: HTMLElement) {
    if (!button.dataset.copyLabel) {
      button.dataset.copyLabel =
        button.getAttribute('aria-label') || (button.textContent ?? '').trim() || 'Copy';
    }
    button.setAttribute('aria-label', 'Copied');
    button.classList.add('copied');
    effects.clearTimeout(copyTimers.get(button));
    copyTimers.set(
      button,
      effects.timeout(() => {
        button.setAttribute('aria-label', button.dataset.copyLabel ?? 'Copy');
        button.classList.remove('copied');
        copyTimers.delete(button);
      }, 1500),
    );
  }
  effects.listen(document, 'click', async (event) => {
    const button =
      event.target instanceof Element ? event.target.closest<HTMLElement>('[data-copy]') : null;
    if (!button) return;
    const source = document.getElementById(button.dataset.copy ?? '');
    if (!source) return;
    const value = source.textContent ?? '';
    if (navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(value);
      } catch {
        if (!disposed && button.isConnected && source.isConnected) selectText(source);
      }
    } else {
      selectText(source);
    }
    if (!disposed && button.isConnected) copied(button);
  });

  const waitingForms = new WeakMap<HTMLDialogElement, HTMLFormElement>();
  let submittingConfirmed: HTMLFormElement | null = null;
  effects.listen(document, 'submit', (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !form.matches('form[data-confirm]')) return;
    if (submittingConfirmed === form) return;
    const dialog = document.getElementById(form.dataset.confirm ?? '');
    if (!(dialog instanceof HTMLDialogElement) || typeof dialog.showModal !== 'function') return;
    event.preventDefault();
    waitingForms.set(dialog, form);
    if (!dialog.open) {
      dialogs.add(dialog);
      dialog.showModal();
    }
  });

  function submitConfirmed(form: HTMLFormElement) {
    const previous = submittingConfirmed;
    submittingConfirmed = form;
    try {
      if (typeof form.requestSubmit === 'function') form.requestSubmit();
      else form.submit();
    } finally {
      submittingConfirmed = previous;
    }
  }

  effects.listen(document, 'click', (event) => {
    const target = event.target instanceof Element ? event.target : null;
    const openButton = target?.closest<HTMLButtonElement>('button[data-open]');
    if (openButton) {
      const dialog = document.getElementById(openButton.dataset.open ?? '');
      if (dialog instanceof HTMLDialogElement && !dialog.open) {
        dialogs.add(dialog);
        dialog.showModal?.();
      }
      return;
    }

    const closeButton = target?.closest?.('[data-close]');
    if (closeButton) {
      closeButton.closest('dialog')?.close();
      return;
    }

    const dialog = target?.closest?.('dialog');
    if (dialog instanceof HTMLDialogElement && target === dialog) {
      if (event.clientX === 0 && event.clientY === 0) return;
      const box = dialog.getBoundingClientRect();
      if (
        event.clientX < box.left ||
        event.clientX > box.right ||
        event.clientY < box.top ||
        event.clientY > box.bottom
      )
        dialog.close();
      return;
    }

    const button = target?.closest?.('button');
    const parentDialog = button?.closest?.('dialog');
    if (!button || !(parentDialog instanceof HTMLDialogElement) || !waitingForms.has(parentDialog))
      return;
    const form = waitingForms.get(parentDialog);
    waitingForms.delete(parentDialog);
    if (button.value === 'confirm') {
      event.preventDefault();
      parentDialog.close('confirm');
      if (form) submitConfirmed(form);
    } else {
      event.preventDefault();
      parentDialog.close('cancel');
    }
  });

  effects.listen(
    document,
    'close',
    (event) => {
      if (event.target instanceof HTMLDialogElement) {
        waitingForms.delete(event.target);
        dialogs.delete(event.target);
      }
    },
    true,
  );

  document.querySelectorAll<HTMLElement>('[data-poll]').forEach((element) => {
    const url = element.dataset.poll;
    if (!url) return;
    const deadline = Date.now() + 30 * 60 * 1000;
    let active: AbortController | null = null;
    let stopped = false;
    const originalLabel = element.getAttribute('aria-label');
    let status: HTMLElement | null = null;
    const note = (message: string) => {
      if (status && element.getAttribute('aria-label') === message) return;
      element.setAttribute('aria-label', message);
      status?.remove();
      status = createStatusNote(message, 'sub');
      element.after(status);
    };
    const stop = () => {
      stopped = true;
      effects.clearInterval(interval);
      effects.clearTimeout(expiry);
      active?.abort();
      active = null;
      observer.disconnect();
    };
    const observer = new MutationObserver(() => {
      if (!element.isConnected) stop();
    });
    observer.observe(document, { childList: true, subtree: true });
    effects.own(() => {
      stop();
      status?.remove();
      if (originalLabel === null) element.removeAttribute('aria-label');
      else element.setAttribute('aria-label', originalLabel);
    });
    const interval = effects.interval(async () => {
      if (stopped || !element.isConnected || Date.now() >= deadline) {
        if (!stopped && element.isConnected && Date.now() >= deadline)
          note('Status checking expired. Return to this page to check again.');
        stop();
        return;
      }
      if (active) return;
      const request = effects.request();
      active = request;
      const timeout = effects.timeout(() => request.abort(), 10000);
      let abort: (() => void) | undefined;
      try {
        // The abort promise also settles transports that fail to reject on abort.
        const response = await Promise.race([
          fetch(url, {
            signal: request.signal,
            credentials: 'same-origin',
            cache: 'no-store',
          }),
          new Promise<never>((_resolve, reject) => {
            abort = () => reject(new DOMException('Status request cancelled', 'AbortError'));
            request.signal.addEventListener('abort', abort, { once: true });
          }),
        ]);
        if (
          !disposed &&
          !stopped &&
          !request.signal.aborted &&
          element.isConnected &&
          Date.now() < deadline &&
          response.status === 200
        ) {
          stop();
          window.location.assign(element.dataset.pollGo || '/');
        } else if (!stopped) note('Waiting for completion; checking again automatically.');
      } catch {
        if (!disposed && !stopped)
          note('Status temporarily unavailable; checking again automatically.');
      } finally {
        if (abort) request.signal.removeEventListener('abort', abort);
        effects.clearTimeout(timeout);
        effects.releaseRequest(request);
        if (active === request) active = null;
      }
    }, 3000);
    const expiry = effects.timeout(
      () => {
        note('Status checking expired. Return to this page to check again.');
        stop();
      },
      30 * 60 * 1000,
    );
  });
  const controller = {
    destroy() {
      if (disposed) return;
      disposed = true;
      composers.destroy();
      effects.destroy();
      for (const button of copyTimers.keys()) {
        button.setAttribute('aria-label', button.dataset.copyLabel ?? 'Copy');
        button.classList.remove('copied');
      }
      copyTimers.clear();
      releaseChrome();
      for (const dialog of dialogs) {
        waitingForms.delete(dialog);
        if (dialog.open) dialog.close();
      }
      dialogs.clear();
      if (current === controller) current = null;
    },
  };
  current = controller;
  return controller;
}

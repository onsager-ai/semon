import { createPanelChrome, type PanelChrome } from './panel';
import { createSelect, type SelectController } from './select';

/** Presentation only: the host retains form controls, values and submission. */
export function mountComposers(root: ParentNode = document): { destroy(): void } {
  const abort = new AbortController();
  let destroyed = false;
  const cleanups: (() => void)[] = [];
  for (const composer of root.querySelectorAll<HTMLElement>('.sh-composer')) {
    let panel: PanelChrome | null = null;
    let disposed = false;
    const selects: SelectController[] = [];
    for (const native of composer.querySelectorAll<HTMLSelectElement>('.sh-picker select')) {
      // Disabled/structured choices remain native until the shared select supports them.
      if (native.disabled || native.multiple || native.querySelector('optgroup, option:disabled'))
        continue;
      const wasHidden = native.hidden;
      const select = createSelect({
        label: native.getAttribute('aria-label') ?? native.labels?.[0]?.textContent?.trim() ?? '',
        value: native.value,
        options: [...native.options].map((o) => ({ value: o.value, label: o.label })),
        onChange(value) {
          native.value = value;
          native.dispatchEvent(new Event('change', { bubbles: true }));
        },
      });
      native.hidden = true;
      native.after(select.el);
      selects.push(select);
      native.addEventListener('change', () => select.setValue(native.value), {
        signal: abort.signal,
      });
      native.form?.addEventListener(
        'reset',
        () => {
          queueMicrotask(() => {
            if (!disposed) select.setValue(native.value);
          });
        },
        { signal: abort.signal },
      );
      cleanups.push(() => {
        select.destroy();
        native.hidden = wasHidden;
      });
    }
    for (const picker of composer.querySelectorAll<HTMLDetailsElement>('.sh-picker')) {
      const trigger = picker.querySelector<HTMLElement>('summary');
      const body = picker.querySelector<HTMLElement>('.sh-picker-body');
      const heading = body?.querySelector<HTMLElement>('h2');
      if (
        !trigger ||
        !body ||
        !heading ||
        typeof HTMLDialogElement.prototype.showModal !== 'function'
      )
        continue;
      trigger.setAttribute('aria-haspopup', 'dialog');
      trigger.setAttribute('aria-expanded', 'false');
      trigger.addEventListener(
        'click',
        (event) => {
          event.preventDefault();
          if (disposed || !trigger.isConnected) return;
          panel?.destroy();
          picker.open = true;
          const hidden = heading.hidden;
          heading.hidden = true;
          trigger.setAttribute('aria-expanded', 'true');
          panel = createPanelChrome(
            { title: heading.textContent ?? '', className: 'sh-composer-panel' },
            {
              opened() {},
              closed() {
                for (const select of selects) select.close(false);
                picker.append(body);
                heading.hidden = hidden;
                picker.open = false;
                trigger.setAttribute('aria-expanded', 'false');
                panel = null;
                if (!disposed && trigger.isConnected) trigger.focus({ preventScroll: true });
              },
            },
          );
          // Keep all named controls inside their original form, including submit buttons.
          composer.append(panel.dialog);
          panel.body.classList.add('sh-composer-panel-body');
          panel.body.append(body);
          panel.show();
        },
        { signal: abort.signal },
      );
      cleanups.push(() => {
        trigger.removeAttribute('aria-haspopup');
        trigger.removeAttribute('aria-expanded');
        picker.open = false;
      });
    }
    cleanups.push(() => {
      disposed = true;
      panel?.destroy();
    });
  }
  return {
    destroy() {
      if (destroyed) return;
      destroyed = true;
      abort.abort();
      for (const cleanup of cleanups.reverse()) cleanup();
    },
  };
}

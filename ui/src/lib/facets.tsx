import { render } from 'preact';

export interface FacetOption {
  value: string;
  label: string;
}
export interface FacetField {
  key: string;
  label: string;
  value: string;
  options: readonly FacetOption[];
  display: string;
}
export interface FacetSelect {
  el: HTMLElement;
  setOptions(options: readonly FacetOption[]): void;
  setValue(value: string): void;
  focus(): void;
  close(): void;
  destroy?(): void;
}
export interface FacetHost {
  select(label: string, value: string, changed: (value: string) => void): FacetSelect;
  change(key: string, value: string): void;
  cleared(key: string): void;
  canOpen(): boolean;
  opened(dialog: HTMLDialogElement): void;
  closed(dialog: HTMLDialogElement, reason: 'dismissed' | 'destroyed'): void;
  clear(): void;
}
export interface FacetChrome {
  element: HTMLElement;
  update(fields: readonly FacetField[]): void;
  destroy(): void;
}
const X = 'M6 6l12 12M18 6L6 18';
function Icon({ path }: { path: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d={path} />
    </svg>
  );
}
/** Persistent Preact controls; Select owns only its separate body slot. */
export function createFacetChrome(host: FacetHost): FacetChrome {
  const element = document.createElement('div');
  element.className = 'facet-filters';
  element.setAttribute('role', 'group');
  element.setAttribute('aria-label', 'Filter sessions');
  let dialog: HTMLDialogElement | null = null,
    body: HTMLDivElement | null = null;
  const selects = new Map<string, FacetSelect>();
  let trigger: HTMLButtonElement | null = null,
    disposed = false,
    opened = false;
  function release() {
    for (const type of ['wheel', 'touchmove'])
      document.removeEventListener(type, hold, { capture: true });
  }
  function hold(event: Event) {
    if (!dialog?.isConnected || !dialog.open) {
      release();
      if (opened && dialog) {
        opened = false;
        host.closed(dialog, 'destroyed');
      }
      return;
    }
    const target = event.target;
    if (
      target instanceof Element &&
      (target.closest('.sh-select-list') ||
        (body && body.contains(target) && body.scrollHeight > body.clientHeight + 1))
    )
      return;
    event.preventDefault();
  }
  function open() {
    if (disposed || opened || !element.isConnected || !dialog || !host.canOpen()) return;
    host.opened(dialog);
    opened = true;
    dialog.showModal();
    for (const type of ['wheel', 'touchmove'])
      document.addEventListener(type, hold, { capture: true, passive: false });
    selects.values().next().value?.focus();
  }
  function close() {
    if (!disposed && dialog?.open) dialog.close();
  }
  function closed() {
    release();
    if (!opened) return;
    opened = false;
    trigger?.focus({ preventScroll: true });
    if (dialog) host.closed(dialog, 'dismissed');
  }
  function backdrop(event: MouseEvent) {
    if (event.target === dialog) close();
  }
  function tab(event: KeyboardEvent) {
    if (event.key !== 'Tab' || event.defaultPrevented) return;
    if (!dialog) return;
    const controls = [
      ...dialog.querySelectorAll<HTMLElement>(
        "button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex='-1'])",
      ),
    ].filter((node) => node.getClientRects().length && !node.closest('[hidden]'));
    const first = controls[0],
      last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  }
  return {
    element,
    update(fields) {
      if (disposed) return;
      for (const field of fields) {
        let select = selects.get(field.key);
        if (!select) {
          select = host.select(field.label, field.value, (value) => {
            if (!disposed) host.change(field.key, value);
          });
          selects.set(field.key, select);
        }
        select.setOptions(field.options);
        select.setValue(field.value);
      }
      const active = fields.filter((field) => field.value !== '').length;
      render(
        <>
          <button
            class="facet-btn"
            type="button"
            aria-haspopup="dialog"
            aria-label={active ? 'Filter, ' + active + ' active' : 'Filter'}
            ref={(node) => {
              trigger = node;
            }}
            onClick={open}
          >
            <Icon path="M4 6h16M7 12h10M10 18h4" />
            <span>Filter</span>
            <span class="facet-n" hidden={!active}>
              {active || ''}
            </span>
          </button>
          {fields.map((field) => (
            <button
              key={field.key}
              class="facet-chip"
              type="button"
              hidden={field.value === ''}
              data-facet={field.key}
              aria-label={field.value ? 'Remove ' + field.label + ': ' + field.display : undefined}
              onClick={() => {
                if (disposed) return;
                host.cleared(field.key);
                trigger?.focus({ preventScroll: true });
              }}
            >
              <span class="txt">{field.value ? field.label + ': ' + field.display : ''}</span>
              <Icon path={X} />
            </button>
          ))}
          <dialog
            class="viewer filters-sheet"
            aria-label="Filter"
            ref={(node) => {
              dialog = node;
            }}
            onClose={closed}
            onClick={backdrop}
            onKeyDown={tab}
          >
            <div class="vh">
              <div class="vt">
                <span>Filter</span>
              </div>
              <button class="vclose" type="button" aria-label="Close filters" onClick={close}>
                <Icon path={X} />
              </button>
            </div>
            <div
              class="vb"
              ref={(node) => {
                body = node;
              }}
            />
            <div class="vf">
              <button
                class="fclear"
                type="button"
                onClick={() => {
                  host.clear();
                  close();
                }}
              >
                Clear all
              </button>
              <button class="fdone" type="button" onClick={close}>
                Done
              </button>
            </div>
          </dialog>
        </>,
        element,
      );
      for (const select of selects.values())
        if (body && select.el.parentElement !== body) body.append(select.el);
    },
    destroy() {
      if (disposed) return;
      disposed = true;
      release();
      for (const select of selects.values()) {
        if (select.destroy) select.destroy();
        else select.close();
      }
      if (dialog?.open) dialog.close();
      if (opened && dialog) {
        opened = false;
        host.closed(dialog, 'destroyed');
      }
      render(null, element);
      element.remove();
    },
  };
}

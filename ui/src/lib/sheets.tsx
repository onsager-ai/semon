import { render } from 'preact';
import { Glyph, screenText } from './screens';
export interface NativeSheetHost {
  opened(dialog: HTMLDialogElement): void;
  closed(dialog: HTMLDialogElement): void;
}
export interface NativeSheetOptions {
  className: string;
  label: string;
  heading: string;
  caption?: string;
  closeLabel: string;
  search?: { placeholder: string; label: string; change(value: string): void };
}
export function createNativeSheet(options: NativeSheetOptions, host: NativeSheetHost) {
  const dialog = document.createElement('dialog');
  dialog.className = 'viewer ' + options.className;
  dialog.setAttribute('aria-label', options.label);
  let body: HTMLDivElement | null = null,
    close: HTMLButtonElement | null = null,
    shown = false,
    disposed = false;
  const cleanups: (() => void)[] = [];
  function finish() {
    if (disposed) return;
    disposed = true;
    dialog.removeEventListener('close', finish);
    dialog.removeEventListener('click', backdrop);
    if (dialog.open) dialog.close();
    for (const cleanup of cleanups) cleanup();
    render(null, dialog);
    dialog.remove();
    if (shown) host.closed(dialog);
  }
  function backdrop(event: MouseEvent) {
    if (event.target === dialog) dialog.close();
  }
  render(
    <>
      <div class="vh">
        <div class="vt">
          <span>{screenText(options.heading)}</span>
        </div>
        {options.caption && <div class="vm">{screenText(options.caption)}</div>}
        <button
          class="vclose"
          type="button"
          aria-label={options.closeLabel}
          ref={(node) => {
            close = node;
          }}
          onClick={() => {
            if (!disposed) dialog.close();
          }}
        >
          <Glyph path="M6 6l12 12M18 6L6 18" className="" />
        </button>
      </div>
      {options.search && (
        <label class="kids-search">
          <Glyph path="M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4" className="" />
          <input
            type="search"
            placeholder={options.search.placeholder}
            aria-label={options.search.label}
            onInput={(event) => {
              if (!disposed) options.search?.change(event.currentTarget.value);
            }}
          />
        </label>
      )}
      <div
        class="vb"
        ref={(node) => {
          body = node;
        }}
      />
    </>,
    dialog,
  );
  const slot = body as HTMLDivElement | null;
  if (!slot) throw new Error('Missing sheet content slot');
  dialog.addEventListener('close', finish);
  dialog.addEventListener('click', backdrop);
  return {
    dialog,
    body: slot,
    cleanup(callback: () => void) {
      cleanups.push(callback);
    },
    show() {
      if (shown || disposed) return;
      shown = true;
      document.body.append(dialog);
      dialog.showModal();
      close?.focus({ focusVisible: false } as FocusOptions);
      host.opened(dialog);
    },
    destroy: finish,
  };
}
export interface KidsRow {
  id: string;
  name: string;
  state: string;
  stateLabel: string;
  age: string;
  under?: string;
  bucket: number;
}
export interface KidsHost extends NativeSheetHost {
  matches(id: string, query: string): boolean;
  select(id: string): void;
}
export function createKidsSheet(name: string, rows: readonly KidsRow[], host: KidsHost) {
  let query = '';
  const sheet = createNativeSheet(
    {
      className: 'kids-sheet',
      label: 'All sessions under ' + name,
      heading: name,
      caption: rows.length + (rows.length === 1 ? ' session' : ' sessions'),
      closeLabel: 'Close',
      search: {
        placeholder: 'Search these sessions',
        label: 'Search these sessions',
        change(value) {
          query = value.trim();
          paint();
        },
      },
    },
    host,
  );
  function paint() {
    const filtered = rows.filter((row) => host.matches(row.id, query));
    render(
      <div class="kids-list">
        {!filtered.length && (
          <p class="empty">{'No sessions match “' + screenText(query) + '”.'}</p>
        )}
        {['Waiting for you', 'Running', 'Finished'].map((label, i) => {
          const items = filtered.filter((row) => row.bucket === i);
          return (
            items.length > 0 && (
              <section key={i} class="kids-sec">
                <h3 class="kids-h">{label + ' (' + items.length + ')'}</h3>
                {items.map((row) => (
                  <button
                    key={row.id}
                    class="kids-row"
                    type="button"
                    data-id={row.id}
                    aria-label={row.name + ', ' + row.stateLabel}
                    onClick={(event) => {
                      if (event.currentTarget.isConnected) host.select(row.id);
                    }}
                  >
                    <span
                      class={'dot ' + row.state}
                      role="img"
                      aria-label={row.stateLabel}
                      data-tip={row.stateLabel}
                    />
                    <span class="nm">{screenText(row.name)}</span>
                    {row.under && <span class="under">{'under ' + screenText(row.under)}</span>}
                    <span class="ag">{row.age}</span>
                  </button>
                ))}
              </section>
            )
          );
        })}
      </div>,
      sheet.body,
    );
  }
  paint();
  sheet.cleanup(() => render(null, sheet.body));
  return sheet;
}

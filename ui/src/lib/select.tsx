import { render } from 'preact';
import { createMeasuredLayout } from './layout';
export interface SelectOption {
  value: string;
  label: string;
}
export interface SelectConfig {
  label?: string;
  options?: readonly SelectOption[];
  value?: string;
  searchAbove?: number;
  onChange?(value: string, api: SelectController): void;
}
export interface SelectController {
  el: HTMLDivElement;
  readonly value: string;
  readonly options: SelectOption[];
  readonly isOpen: boolean;
  setOptions(options: readonly SelectOption[]): void;
  setValue(value: string): void;
  open(): void;
  close(refocus?: boolean): void;
  focus(): void;
  destroy(): void;
}
const ICON = {
  chevron: 'M6 9l6 6 6-6',
  check: 'M5 12.5l4.5 4.5L19 7',
  search: 'M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zM20 20l-4-4',
  close: 'M6 6l12 12M18 6L6 18',
};
function Icon({ path, className }: { path: string; className?: string }) {
  return (
    <svg
      class={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={path} />
    </svg>
  );
}
let serial = 0,
  sheet: { popped(): void } | null = null,
  swallow = 0,
  sheetList: HTMLElement | null = null,
  touchY = 0,
  installed = false;
const phone = () => window.matchMedia('(max-width: 760px)').matches;
function orphaned() {
  if (!sheetList || sheetList.isConnected) return false;
  lock(null);
  sheet = null;
  return true;
}
function refuse(event: Event) {
  if (orphaned() || !sheetList) return;
  if (event instanceof TouchEvent && event.type === 'touchstart') {
    touchY = event.touches[0]?.clientY ?? 0;
    return;
  }
  if (!(event.target instanceof Node) || !sheetList.contains(event.target)) {
    event.preventDefault();
    return;
  }
  const room = sheetList.scrollHeight - sheetList.clientHeight,
    delta =
      event instanceof WheelEvent
        ? event.deltaY
        : event instanceof TouchEvent
          ? touchY - (event.touches[0]?.clientY ?? touchY)
          : 0;
  if (
    room <= 1 ||
    (delta > 0 && sheetList.scrollTop >= room - 1) ||
    (delta < 0 && sheetList.scrollTop <= 0)
  )
    event.preventDefault();
}
function lock(list: HTMLElement | null) {
  sheetList = list;
  for (const type of ['wheel', 'touchmove']) {
    document.removeEventListener(type, refuse, true);
    if (list) document.addEventListener(type, refuse, { capture: true, passive: false });
  }
  document.removeEventListener('touchstart', refuse, true);
  if (list) document.addEventListener('touchstart', refuse, { capture: true, passive: true });
}
export function createSelect(config: SelectConfig = {}): SelectController {
  const n = ++serial,
    listId = 'sh-select-list-' + n,
    labelId = 'sh-select-label-' + n,
    valueId = 'sh-select-value-' + n,
    label = config.label ?? '',
    searchAbove = config.searchAbove ?? 8;
  const root = document.createElement('div');
  root.className = 'sh-select';
  if (label) root.dataset.label = label;
  const layout = createMeasuredLayout(),
    triggerRoot = document.createElement('div'),
    park = document.createElement('div'),
    pop = document.createElement('div'),
    parts = document.createElement('div');
  triggerRoot.className = 'sh-select-trigger-slot';
  parts.className = 'sh-select-parts';
  park.hidden = true;
  pop.className = 'sh-select-pop';
  pop.hidden = true;
  root.append(triggerRoot, pop, park);
  park.append(parts);
  let options: SelectOption[] = [],
    value = config.value ?? '',
    opened: { sheet: boolean; search: boolean; entry?: boolean } | null = null,
    query = '',
    active: string | null = null,
    typed = '',
    typedAt = 0,
    disposed = false;
  let trigger: HTMLButtonElement | null = null,
    search: HTMLInputElement | null = null,
    list: HTMLDivElement | null = null,
    dialog: HTMLDialogElement | null = null,
    popClass = '',
    sizing: number | null = null;
  const shown = () => {
    const q = query.trim().toLowerCase();
    return q ? options.filter((o) => o.label.toLowerCase().includes(q)) : options;
  };
  const owner = () => (!opened ? null : opened.sheet ? list : opened.search ? search : trigger);
  const descendant = () =>
    active === null ? undefined : listId + '-' + options.findIndex((o) => o.value === active);
  function paintTrigger() {
    const text = options.find((o) => o.value === value)?.label ?? '';
    render(
      <button
        class="sh-select-trigger"
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={!!opened}
        aria-controls={listId}
        aria-labelledby={(label ? labelId + ' ' : '') + valueId}
        aria-activedescendant={owner() === trigger ? descendant() : undefined}
        ref={(node) => {
          trigger = node;
        }}
        onClick={(event) => {
          if (event.currentTarget.isConnected && !disposed) {
            if (opened) close();
            else openList();
          }
        }}
        onKeyDown={onKey}
        onKeyUp={(event) => {
          if (event.key === ' ') event.preventDefault();
        }}
      >
        <span
          class="sh-select-text"
          data-tip-clipped=""
          data-tip={(label ? label + ': ' : '') + text}
        >
          <span id={labelId} class="sh-select-label">
            {label ? label + ':' : ''}
          </span>
          {label ? ' ' : ''}
          <span id={valueId} class="sh-select-value">
            {text}
          </span>
        </span>
        <Icon path={ICON.chevron} className="sh-select-chevron" />
      </button>,
      triggerRoot,
    );
  }
  function paintList() {
    const items = shown();
    render(
      <>
        <div class="sh-select-search" hidden={!opened?.search}>
          <Icon path={ICON.search} />
          <input
            type="search"
            autoComplete="off"
            spellcheck={false}
            placeholder="Search"
            aria-label={'Search ' + (label || 'options')}
            role="combobox"
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls={listId}
            aria-activedescendant={owner() === search ? descendant() : undefined}
            value={query}
            ref={(node) => {
              search = node;
            }}
            onKeyDown={onKey}
            onInput={(event) => {
              if (!event.currentTarget.isConnected || disposed) return;
              query = event.currentTarget.value;
              paintList();
              settle();
              position();
            }}
          />
        </div>
        <div
          class="sh-select-list"
          id={listId}
          role="listbox"
          aria-label={label || undefined}
          tabIndex={-1}
          aria-activedescendant={owner() === list ? descendant() : undefined}
          ref={(node) => {
            list = node;
          }}
          onKeyDown={onKey}
        >
          {items.map((option) => (
            <div
              key={option.value}
              class={'sh-select-option' + (option.value === active ? ' sh-active' : '')}
              id={listId + '-' + options.indexOf(option)}
              data-value={option.value}
              role="option"
              aria-selected={option.value === value}
              onPointerDown={(event) => event.preventDefault()}
              onPointerMove={(event) => {
                if (event.currentTarget.isConnected && option.value !== active)
                  setActive(option.value, false);
              }}
              onClick={(event) => {
                if (event.currentTarget.isConnected && !disposed) choose(option.value);
              }}
            >
              <Icon path={ICON.check} className="sh-select-check" />
              <span class="sh-select-option-text">{option.label}</span>
            </div>
          ))}
        </div>
        <div class="sh-select-empty" role="status" hidden={items.length > 0}>
          No matches
        </div>
      </>,
      parts,
    );
  }
  function setActive(next: string | null, scroll = true) {
    active = next;
    paintList();
    paintTrigger();
    if (scroll && active !== null)
      document.getElementById(descendant()!)?.scrollIntoView({ block: 'nearest' });
  }
  function settle() {
    const items = shown();
    setActive(
      items.some((o) => o.value === active)
        ? active
        : items.some((o) => o.value === value)
          ? value
          : (items[0]?.value ?? null),
    );
  }
  function move(to: (at: number, length: number) => number) {
    const items = shown();
    if (!items.length) return;
    const at = items.findIndex((o) => o.value === active);
    setActive(items[Math.max(0, Math.min(items.length - 1, to(at, items.length)))]!.value);
  }
  function geometry(left: number, top: number, min: number, max?: number) {
    layout.reset();
    popClass = [
      layout.className('left', left, 'px'),
      layout.className('top', top, 'px'),
      layout.className('min-width', min, 'px'),
      ...(max === undefined ? [] : [layout.className('max-height', max, 'px')]),
    ].join(' ');
    pop.className = 'sh-select-pop ' + popClass;
  }
  function position() {
    if (!opened || opened.sheet || !trigger) return;
    const r = trigger.getBoundingClientRect(),
      vw = document.documentElement.clientWidth,
      vh = innerHeight,
      gap = 4,
      margin = 8,
      min = Math.max(r.width, 200);
    geometry(0, 0, min);
    const bar = document.getElementById('topbar'),
      floor = Math.max(margin, bar ? bar.getBoundingClientRect().bottom + gap : 0),
      width = Math.min(pop.offsetWidth, vw - 2 * margin),
      height = pop.offsetHeight,
      below = vh - r.bottom - gap - margin,
      above = r.top - gap - floor;
    const up = height > below && above > below,
      room = Math.max(48, Math.min(up ? above : below, 360));
    geometry(
      Math.max(margin, Math.min(r.left, vw - width - margin)),
      Math.max(floor, up ? r.top - gap - Math.min(height, room) : r.bottom + gap),
      min,
      room,
    );
    pop.dataset.side = up ? 'top' : 'bottom';
  }
  const outside = (event: Event) => {
    if (opened && event.target instanceof Node && !root.contains(event.target)) close(false);
  };
  const reposition = (event: Event) => {
    if (!(event.target instanceof Node) || !pop.contains(event.target)) position();
  };
  function openList() {
    if (opened || disposed || !root.isConnected) return;
    const asSheet = phone() && typeof HTMLDialogElement.prototype.showModal === 'function';
    query = '';
    opened = { sheet: asSheet, search: options.length > searchAbove };
    paintTrigger();
    paintList();
    if (asSheet) {
      dialog = document.createElement('dialog');
      dialog.className = 'sh-select-sheet';
      dialog.setAttribute('aria-label', label || 'Options');
      const head = document.createElement('div');
      head.className = 'sh-select-sheet-h';
      render(
        <>
          <span class="sh-select-sheet-title">{label}</span>
          <button class="sh-select-close" type="button" aria-label="Close" onClick={() => close()}>
            <Icon path={ICON.close} />
          </button>
        </>,
        head,
      );
      dialog.append(head, parts);
      root.append(dialog);
      dialog.addEventListener('click', (event) => {
        if (event.target === dialog) close();
      });
      dialog.addEventListener('close', () => {
        if (opened && !dialog?.open) close();
      });
      dialog.showModal();
      lock(list);
      try {
        history.pushState({ ...history.state, shSelect: n }, '');
        swallow = 0;
        opened.entry = true;
        sheet = {
          popped() {
            if (opened) opened.entry = false;
            close();
          },
        };
      } catch {}
      settle();
      list?.focus({ preventScroll: true });
    } else {
      pop.append(parts);
      pop.hidden = false;
      position();
      settle();
      document.addEventListener('pointerdown', outside, true);
      window.addEventListener('resize', position);
      document.addEventListener('scroll', reposition, true);
      if (opened.search) search?.focus({ preventScroll: true });
    }
  }
  function close(refocus = true) {
    if (!opened) return;
    const was = opened;
    opened = null;
    typed = '';
    active = null;
    if (was.sheet) {
      if (dialog?.open) dialog.close();
      if (dialog) {
        const head = dialog.firstElementChild;
        if (head) render(null, head);
        dialog.remove();
        dialog = null;
      }
      lock(null);
      sheet = null;
      if (was.entry) {
        swallow++;
        history.back();
      }
    } else {
      if (refocus) trigger?.focus({ preventScroll: true });
      pop.hidden = true;
      document.removeEventListener('pointerdown', outside, true);
      window.removeEventListener('resize', position);
      document.removeEventListener('scroll', reposition, true);
    }
    park.append(parts);
    paintTrigger();
    paintList();
    if (refocus) trigger?.focus({ preventScroll: true });
  }
  function choose(next: string) {
    const changed = next !== value;
    close();
    if (changed) {
      value = next;
      paintTrigger();
      config.onChange?.(value, api);
    }
  }
  function typeahead(ch: string) {
    const now = Date.now();
    typed = now - typedAt > 500 ? ch : typed + ch;
    typedAt = now;
    const items = shown(),
      at = items.findIndex((o) => o.value === active),
      cycle = [...typed].every((c) => c === typed[0]),
      key = cycle ? typed[0]! : typed,
      from = cycle ? at + 1 : Math.max(0, at);
    const hit = [...items.slice(from), ...items.slice(0, from)].find((o) =>
      o.label.toLowerCase().startsWith(key),
    );
    if (hit) setActive(hit.value);
  }
  function toSearch(text: string) {
    search?.focus({ preventScroll: true });
    query += text;
    paintList();
    search?.setSelectionRange(query.length, query.length);
    settle();
    position();
  }
  function onKey(event: KeyboardEvent) {
    if (
      disposed ||
      !(event.currentTarget instanceof HTMLElement) ||
      !event.currentTarget.isConnected
    )
      return;
    const key = event.key,
      inField = event.target === search,
      printable = key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
    if (!opened) {
      if (['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(key)) {
        event.preventDefault();
        openList();
      } else if (key === 'Home' || key === 'End') {
        event.preventDefault();
        openList();
        move((_, len) => (key === 'Home' ? 0 : len - 1));
      } else if (printable) {
        event.preventDefault();
        openList();
        if (options.length > searchAbove) toSearch(key);
        else typeahead(key.toLowerCase());
      }
      return;
    }
    if (key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (key === 'ArrowDown') {
      event.preventDefault();
      if (!event.altKey) move((at) => at + 1);
    } else if (key === 'ArrowUp') {
      event.preventDefault();
      if (event.altKey) {
        if (active !== null) choose(active);
        else close();
      } else move((at, len) => (at < 0 ? len - 1 : at - 1));
    } else if (key === 'PageDown' || key === 'PageUp') {
      event.preventDefault();
      move((at) => at + (key === 'PageDown' ? 10 : -10));
    } else if ((key === 'Home' || key === 'End') && !inField) {
      event.preventDefault();
      move((_, len) => (key === 'Home' ? 0 : len - 1));
    } else if (key === 'Enter') {
      event.preventDefault();
      if (active !== null) choose(active);
      else close();
    } else if (key === ' ' && !inField) {
      event.preventDefault();
      if (Date.now() - typedAt < 500 && typed) typeahead(' ');
      else if (active !== null) choose(active);
    } else if (key === 'Tab') {
      if (opened.sheet) return;
      const next = active;
      close();
      if (next !== null && next !== value) {
        value = next;
        paintTrigger();
        config.onChange?.(value, api);
      }
    } else if (printable && !inField) {
      event.preventDefault();
      if (opened.search) toSearch(key);
      else typeahead(key.toLowerCase());
    }
  }
  pop.addEventListener('mousedown', (event) => {
    if (event.target !== search) event.preventDefault();
  });
  root.addEventListener('focusout', (event) => {
    if (
      opened &&
      !opened.sheet &&
      (!(event.relatedTarget instanceof Node) || !root.contains(event.relatedTarget))
    )
      close(false);
  });
  const api: SelectController = {
    el: root,
    get value() {
      return value;
    },
    get options() {
      return options.map((option) => ({ ...option }));
    },
    get isOpen() {
      return !!opened;
    },
    setOptions(next) {
      if (disposed) return;
      options = next.map((option) => ({
        value: String(option.value),
        label: String(option.label ?? option.value),
      }));
      paintTrigger();
      if (opened) {
        paintList();
        settle();
        position();
      }
    },
    setValue(next) {
      if (disposed) return;
      value = String(next ?? '');
      paintTrigger();
      if (opened) paintList();
    },
    open: openList,
    close,
    focus() {
      trigger?.focus();
    },
    destroy() {
      if (disposed) return;
      close(false);
      disposed = true;
      if (sizing !== null) cancelAnimationFrame(sizing);
      render(null, triggerRoot);
      render(null, parts);
      layout.destroy();
      root.remove();
    },
  };
  (root as HTMLDivElement & { semonSelect: SelectController }).semonSelect = api;
  api.setOptions(config.options ?? []);
  paintList();
  return api;
}
export function enhanceSelect(native: HTMLSelectElement): SelectController | null {
  if (native.dataset.selectReady !== undefined) return null;
  const api = createSelect({
    label: native.dataset.label ?? native.getAttribute('aria-label') ?? '',
    value: native.value,
    options: [...native.options].map((option) => ({
      value: option.value,
      label: option.textContent?.trim() ?? '',
    })),
    onChange(value) {
      native.value = value;
      native.dispatchEvent(new Event('change', { bubbles: true }));
    },
  });
  native.dataset.selectReady = '';
  native.hidden = true;
  native.after(api.el);
  return api;
}
export function installSelect() {
  if (installed) return;
  installed = true;
  window.addEventListener('popstate', (event) => {
    if (swallow) {
      swallow--;
      event.stopImmediatePropagation();
      return;
    }
    if (sheet && !orphaned()) {
      event.stopImmediatePropagation();
      sheet.popped();
    }
  });
  const run = () =>
    document.querySelectorAll<HTMLSelectElement>('select[data-select]').forEach(enhanceSelect);
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', run);
  else run();
}

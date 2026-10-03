import { render } from 'preact';

export interface PanelOptions {
  title: string;
  label?: string;
  sub?: string;
  className?: string;
}
export interface PanelHost {
  /** History/live ownership stays with the host. Called after modal commit/focus. */
  opened(): void;
  /** Called once, after roots and document listeners are released. */
  closed(): void;
}
export interface PanelChrome {
  readonly dialog: HTMLDialogElement;
  /** Host-owned slot: Preact never renders or removes its descendants. */
  readonly body: HTMLDivElement;
  show(): void;
  destroy(): void;
}

function PanelHeader({ heading, sub, close }: { heading: string; sub?: string; close(): void }) {
  return <><div class="panel-t">{heading}</div>{sub && <div class="panel-sub">{sub}</div>}
    <button class="ibtn" type="button" aria-label="Close" onClick={close}>
      <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 6l12 12M18 6L6 18" /></svg>
    </button></>;
}

/** One-shot native modal, with a disjoint Preact header and imperative content slot. */
export function createPanelChrome(options: PanelOptions, host: PanelHost): PanelChrome {
  const dialog = document.createElement('dialog');
  dialog.className = 'panel' + (options.className ? ' ' + options.className : '');
  dialog.setAttribute('aria-label', options.label ?? options.title);
  const header = document.createElement('div'); header.className = 'panel-h';
  const body = document.createElement('div'); body.className = 'panel-b'; body.tabIndex = -1;
  let disposed = false, shown = false;
  const close = () => { if (!disposed && dialog.open) dialog.close(); };
  render(<PanelHeader heading={options.title} sub={options.sub} close={close} />, header);
  dialog.append(header, body);
  document.body.append(dialog);
  const hold = (event: Event) => {
    if (!dialog.isConnected) { destroy(); return; }
    if (!dialog.open) return;
    if (!(event.target instanceof Node) || !body.contains(event.target) || body.scrollHeight <= body.clientHeight + 1) event.preventDefault();
  };
  const backdrop = (event: MouseEvent) => { if (event.target === dialog) close(); };
  function destroy() {
    if (disposed) return;
    disposed = true;
    dialog.removeEventListener('click', backdrop);
    dialog.removeEventListener('close', destroy);
    for (const type of ['wheel', 'touchmove']) document.removeEventListener(type, hold, { capture: true });
    if (dialog.open) dialog.close();
    render(null, header);
    dialog.remove();
    if (shown) host.closed();
  }
  dialog.addEventListener('click', backdrop);
  dialog.addEventListener('close', destroy);
  return {
    dialog, body, destroy,
    show() {
      if (disposed || shown || !dialog.isConnected) return;
      dialog.showModal();
      shown = true;
      for (const type of ['wheel', 'touchmove']) document.addEventListener(type, hold, { capture: true, passive: false });
      body.focus({ preventScroll: true });
      host.opened();
    },
  };
}

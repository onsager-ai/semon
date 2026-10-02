import { render } from 'preact';
import { AccountAvatar, AccountMenu, type AccountMenuProps } from './AccountMenu';

export interface AccountCloseOptions {
  keepEntry?: boolean;
  navigating?: boolean;
}
/** History, live data and measured CSS placement belong to the embedding host. */
export interface AccountChromeHost {
  place(widget: HTMLElement, trigger: HTMLButtonElement): void;
  opened(compact: boolean): void;
  closed(options: AccountCloseOptions): void;
  /** Return true when the host consumed a phone sheet navigation/submission. */
  navigate(href: string): boolean;
  submit(form: HTMLFormElement): boolean;
}
export interface AccountChrome {
  readonly open: boolean;
  mount(props: AccountMenuProps): HTMLElement;
  close(options?: AccountCloseOptions): void;
  updateWide(wide: boolean): void;
  unmount(root: HTMLElement): void;
  /** Forward Escape from the host's overlay arbitration before dismissing its drawer. */
  escape(): void;
  destroy(): void;
}
type Widget = { root: HTMLElement; props: AccountMenuProps };

/** One controller per application shell; explicit renders commit before focus/measurement. */
export function createAccountChrome(host: AccountChromeHost): AccountChrome {
  const widgets = new Map<HTMLElement, Widget>();
  let active: Widget | null = null;
  let destroyed = false;
  const initialFocus: FocusOptions & { focusVisible: boolean } = { focusVisible: false };
  const returnFocus: FocusOptions & { focusVisible: boolean } = { focusVisible: false, preventScroll: true };
  const triggerOf = (widget: Widget) => widget.root.querySelector<HTMLButtonElement>('.account-trigger')!;
  function commit(widget: Widget) {
    const { root, props } = widget;
    const { account, compact, wide, onWideChange } = props;
    const current = account.workspaces.find(workspace => workspace.current);
    const expanded = active === widget;
    render(<>
      {expanded && compact && <div key="backdrop" class="account-backdrop" onClick={event => { event.stopPropagation(); close(); }} />}
      <button key="trigger" class={'account-trigger' + (compact ? '' : ' account-avatar-button')} type="button"
        aria-haspopup="menu" aria-expanded={expanded}
        aria-label={compact ? account.name + ', ' + (current?.name ?? account.login) : account.name + ' account menu'}
        onClick={event => { event.stopPropagation(); toggle(widget); }}>
        <AccountAvatar account={account} />
        {compact && <span class="account-summary"><span class="account-summary-name">{account.name}</span><span class="account-summary-workspace">{current?.name ?? account.login}</span></span>}
      </button>
      {expanded && <div key="menu" class="menu account-popover" role="menu" aria-label="Account"
        onClick={event => {
          const target = event.target;
          const link = target instanceof Element ? target.closest<HTMLAnchorElement>('a[href]') : null;
          if (compact && link && host.navigate(link.href)) event.preventDefault();
        }}
        onSubmit={event => {
          if (compact && event.target instanceof HTMLFormElement && host.submit(event.target)) event.preventDefault();
        }}>
        <AccountMenu account={account} compact={compact} wide={wide} onWideChange={onWideChange} />
      </div>}
    </>, root);
  }
  function close(options: AccountCloseOptions = {}) {
    const widget = active;
    const menu = widget?.root.querySelector('.account-popover');
    const focused = document.activeElement;
    const refocus = !!menu && (!focused || focused === document.body || menu.contains(focused));
    active = null;
    if (widget) commit(widget);
    if (refocus && widget?.root.isConnected) triggerOf(widget).focus(returnFocus);
    host.closed(options);
  }
  function toggle(widget: Widget) {
    // Detached or unmounted triggers cannot reopen stale account roots.
    if (destroyed || !widgets.has(widget.root) || !widget.root.isConnected) return;
    if (active) { close(); return; }
    close();
    const trigger = triggerOf(widget);
    if (widget.props.compact) host.place(widget.root, trigger);
    active = widget;
    commit(widget);
    host.opened(widget.props.compact);
    widget.root.querySelector<HTMLElement>('.account-menu-row')?.focus(initialFocus);
  }
  const outside = (event: MouseEvent) => {
    if (active && event.target instanceof Node && !active.root.contains(event.target)) close();
  };
  const pageshow = (event: PageTransitionEvent) => {
    if (event.persisted && active) close({ keepEntry: true });
  };
  document.addEventListener('click', outside);
  window.addEventListener('pageshow', pageshow);
  function unmount(root: HTMLElement) {
    const widget = widgets.get(root);
    if (!widget) return;
    if (active === widget) close();
    render(null, root);
    widgets.delete(root);
  }
  return {
    get open() { return active !== null; },
    mount(props) {
      if (destroyed) throw new Error('Account chrome is destroyed');
      const root = document.createElement('div');
      root.className = 'account-widget ' + (props.compact ? 'account-widget-phone' : 'account-widget-desktop');
      const widget = { root, props };
      widgets.set(root, widget);
      commit(widget);
      return root;
    },
    close,
    escape: () => close(),
    updateWide(wide) {
      for (const widget of widgets.values()) {
        widget.props = { ...widget.props, wide };
        commit(widget);
      }
    },
    unmount,
    destroy() {
      if (destroyed) return;
      destroyed = true;
      for (const root of widgets.keys()) unmount(root);
      document.removeEventListener('click', outside);
      window.removeEventListener('pageshow', pageshow);
    },
  };
}

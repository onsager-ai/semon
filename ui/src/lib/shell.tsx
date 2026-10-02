import { render } from 'preact';
import { safePath } from './account';
import { createAccountChrome, type AccountChromeHost, type AccountChrome } from './account-chrome';
import type { AccountMenuProps } from './AccountMenu';

export interface ShellDestination {
  key: string;
  label: string;
  href: string;
  icon: string;
  current: boolean;
  count?: number;
  hot?: boolean;
}
export interface ShellHost {
  account: AccountChromeHost;
  /** Consume only ordinary primary clicks. False keeps native server navigation. */
  navigate(destination: ShellDestination): boolean;
  drawerOpened(): void;
  drawerClosed(): void;
  railChanged(): void;
}
/** These containers and their descendants belong to the host, never to a VNode. */
export interface ShellSlots {
  recent: HTMLElement;
  content: HTMLElement;
}
export interface ShellBar {
  /** Direct children preserve existing flex geometry; each node is a host-owned slot. */
  titleSlot?: HTMLElement;
  actions?: readonly HTMLElement[];
  /** Find/error modes replace title/actions/lead without migrating their screen state. */
  mode?: readonly HTMLElement[];
  session?: boolean;
  lead?: { label: string; icon: string; back?: () => void };
  account?: AccountMenuProps | null;
  /** Find's account stays inside its host-owned row. */
  accountTarget?: HTMLElement;
}
export interface ShellChrome {
  readonly account: AccountChrome;
  readonly slots: ShellSlots;
  readonly drawerOpen: boolean;
  mount(container: HTMLElement): void;
  update(destinations: readonly ShellDestination[], rail: boolean): void;
  topbar(props: ShellBar): void;
  drawerAccount(props: AccountMenuProps | null): void;
  openDrawer(): void;
  closeDrawer(quiet?: boolean): void;
  restoreDrawer(): void;
  unmount(): void;
  destroy(): void;
}
const NS = 'http://www.w3.org/2000/svg';
function element(tag: string, className: string, id?: string) {
  const node = document.createElement(tag); node.className = className;
  if (id) node.id = id;
  return node;
}
function svg(path: string, stroke = '1.8') {
  const node = document.createElementNS(NS, 'svg');
  for (const [key, value] of Object.entries({ viewBox: '0 0 24 24', 'aria-hidden': 'true', fill: 'none', stroke: 'currentColor', 'stroke-width': stroke, 'stroke-linecap': 'round', 'stroke-linejoin': 'round' })) node.setAttribute(key, value);
  const p = document.createElementNS(NS, 'path'); p.setAttribute('d', path); node.append(p); return node;
}
function Icon({ path }: { path: string }) {
  return <svg class="icon" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d={path} /></svg>;
}
/** Sidebar-only hosts use this same root without adopting their server-owned frame. */
export function renderShellNavigation(nav: HTMLElement, destinations: readonly ShellDestination[], navigate: (destination: ShellDestination) => boolean) {
  const keys = new Set<string>();
  for (const destination of destinations) { if (!destination.key || keys.has(destination.key) || !safePath(destination.href)) throw new Error('Invalid shell destination'); keys.add(destination.key); }
  render(<>{destinations.map(destination => <a key={destination.key} class="nav-item" data-go={destination.key} href={destination.href} aria-current={destination.current ? 'page' : undefined}
        onClick={event => {
          if (!event.currentTarget.isConnected || !nav.contains(event.currentTarget) || event.defaultPrevented || event.button !== 0 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
          if (navigate(destination)) event.preventDefault();
        }}><Icon path={destination.icon} /><span>{destination.label}</span>{!!destination.count && <span class={'cnt' + (destination.hot ? ' hot' : '')}>{destination.count}</span>}</a>)}</>, nav);
}

/** Chrome roots are disjoint from host slots. Explicit renders commit synchronously. */
export function createShellChrome(host: ShellHost): ShellChrome {
  const account = createAccountChrome(host.account);
  const phone = window.matchMedia('(max-width: 760px)');
  let app: HTMLElement | null = null, sidebar: HTMLElement, head: HTMLElement, nav: HTMLElement, bar: HTMLElement, scrim: HTMLElement;
  let recent: HTMLElement, content: HTMLElement;
  let desktopAccount: HTMLElement | null = null, phoneAccount: HTMLElement | null = null;
  let lead: HTMLButtonElement | null = null, destroyed = false, sx: number | null = null;
  let destinations: ShellDestination[] = [], rail = false;
  const listeners: (() => void)[] = [];
  function listen<K extends keyof HTMLElementEventMap>(target: HTMLElement, type: K, handler: (event: HTMLElementEventMap[K]) => void, options?: AddEventListenerOptions) {
    target.addEventListener(type, handler, options); listeners.push(() => target.removeEventListener(type, handler, options));
  }
  function ready() { if (destroyed || !app) throw new Error('Shell is not mounted'); }
  function closeDrawer(quiet = false) {
    if (!app || !document.body.classList.contains('drawer-open')) return;
    document.body.classList.remove('drawer-open'); account.close(); lead?.setAttribute('aria-expanded', 'false');
    if (!quiet) lead?.focus(); host.drawerClosed();
  }
  function openDrawer() {
    if (!app?.isConnected || !phone.matches || destroyed) return;
    host.drawerOpened(); document.body.classList.add('drawer-open'); lead?.setAttribute('aria-expanded', 'true');
  }
  function paintHead() {
    const label = rail ? 'Expand sidebar' : 'Collapse sidebar';
    render(<>
      <div class="brandrow"><span class="mark" aria-hidden="true" /><span class="brandname">Semon</span>
        <button class="ibtn close" id="drawer-close" type="button" aria-label="Close menu" onClick={() => closeDrawer()}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </button>
      </div>
      <button class="ibtn rail-toggle" id="rail-toggle" type="button" aria-label={label} aria-expanded={!rail} data-tip={label} onClick={() => { if (app?.isConnected && !destroyed) host.railChanged(); }}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 5h16v14H4zM9 5v14" /></svg>
      </button>
    </>, head);
  }
  function dropDesktop() { if (desktopAccount) { account.unmount(desktopAccount); desktopAccount.remove(); desktopAccount = null; } }
  function drawerAccount(props: AccountMenuProps | null) {
    ready(); if (phoneAccount) { account.unmount(phoneAccount); phoneAccount.remove(); phoneAccount = null; }
    if (props) { phoneAccount = account.mount(props); phoneAccount.id = 'account-drawer'; sidebar.append(phoneAccount); }
  }
  function unmount() {
    if (!app) return;
    closeDrawer(true); dropDesktop(); drawerAccount(null);
    for (const remove of listeners.splice(0)) remove();
    phone.removeEventListener('change', resized);
    render(null, nav); render(null, head);
    // Detach host slots intact. Never clear their children, including on teardown.
    for (const node of [...bar.childNodes]) node.remove();
    lead = null; app = null; sx = null;
  }
  const resized = () => { closeDrawer(true); app?.classList.toggle('rail', rail && !phone.matches); };
  return {
    account,
    get slots() { ready(); return { recent, content }; },
    get drawerOpen() { return !!app && document.body.classList.contains('drawer-open'); },
    mount(container) {
      if (destroyed) throw new Error('Shell is destroyed');
      if (app) throw new Error('Shell is already mounted');
      // Adopt the Rust fallback or create the same frame without parsing HTML.
      if (!container.querySelector('#sidebar')) {
        const side = element('aside', 'sidebar', 'sidebar'); side.setAttribute('aria-label', 'Navigation');
        const heading = element('div', 'sidebar-head'); const navigation = element('div', '', 'nav');
        const caption = element('div', 'side-h'); caption.textContent = 'Recent';
        const list = element('div', 'side-list', 'side-list'); const lanes = element('div', '', 'lanes'); lanes.setAttribute('role', 'tree'); lanes.setAttribute('aria-label', 'Recent sessions'); list.append(lanes);
        side.append(heading, navigation, caption, list);
        const main = element('div', 'main', 'main'); main.append(element('header', 'topbar', 'topbar'), element('div', 'page', 'page'));
        container.append(side, element('div', 'scrim', 'scrim'), main);
      }
      const required = (selector: string) => { const node = container.querySelector<HTMLElement>(selector); if (!node) throw new Error('Missing shell slot ' + selector); return node; };
      sidebar = required('#sidebar'); head = required('.sidebar-head'); nav = required('#nav'); bar = required('#topbar'); scrim = required('#scrim'); recent = required('#side-list'); content = required('#page');
      app = container; paintHead();
      listen(scrim, 'click', () => closeDrawer());
      listen(sidebar, 'touchstart', event => { sx = event.touches[0]?.clientX ?? null; }, { passive: true });
      listen(sidebar, 'touchmove', event => { if (sx !== null && event.touches[0] && event.touches[0].clientX - sx < -50) { sx = null; closeDrawer(); } }, { passive: true });
      listen(sidebar, 'touchend', () => { sx = null; }, { passive: true });
      phone.addEventListener('change', resized);
    },
    update(next, collapsed) {
      ready();
      const keys = new Set<string>();
      destinations = next.map(destination => {
        if (!destination.key || keys.has(destination.key) || !safePath(destination.href)) throw new Error('Invalid shell destination');
        keys.add(destination.key); return { ...destination };
      });
      rail = collapsed; app!.classList.toggle('rail', rail && !phone.matches); paintHead();
      renderShellNavigation(nav, destinations, host.navigate);
    },
    topbar(props) {
      ready(); account.close(); dropDesktop();
      lead = null; const nodes: HTMLElement[] = [];
      if (!props.mode && props.lead) {
        const button = document.createElement('button'); button.type = 'button'; button.className = 'ibtn lead' + (props.lead.back ? ' trace-back' : ''); button.id = 'lead-btn';
        button.setAttribute('aria-label', props.lead.label); button.setAttribute('aria-controls', 'sidebar'); button.setAttribute('aria-expanded', String(document.body.classList.contains('drawer-open')));
        button.append(svg(props.lead.icon)); const back = props.lead.back;
        button.addEventListener('click', () => { if (button !== lead || !button.isConnected || !app || destroyed) return; if (back) back(); else openDrawer(); });
        lead = button; nodes.push(button);
      }
      if (props.mode) nodes.push(...props.mode); else { if (props.titleSlot) nodes.push(props.titleSlot); if (props.actions) nodes.push(...props.actions); }
      if (props.account) { desktopAccount = account.mount(props.account); if (props.accountTarget) props.accountTarget.append(desktopAccount); else nodes.push(desktopAccount); }
      bar.classList.remove('scrolled'); bar.classList.toggle('session-bar', !!props.session);
      // Keep unchanged action slots in place (Analytics range/focus), like the host's placer.
      const kept = new Set(nodes);
      for (const node of [...bar.children]) if (!kept.has(node as HTMLElement)) node.remove();
      let cursor = bar.firstChild;
      for (const node of nodes) { if (node === cursor) cursor = cursor.nextSibling; else bar.insertBefore(node, cursor); }
      while (cursor) { const next = cursor.nextSibling; cursor.remove(); cursor = next; }
    },
    drawerAccount, openDrawer, closeDrawer,
    restoreDrawer() { ready(); document.body.classList.add('drawer-open'); lead?.setAttribute('aria-expanded', 'true'); },
    unmount,
    destroy() { if (destroyed) return; unmount(); account.destroy(); destroyed = true; },
  };
}

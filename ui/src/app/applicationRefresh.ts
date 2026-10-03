import { setGeometry } from '../lib';
interface ApplicationRefreshHost {
  disposed: boolean;
  accountChrome: import('../lib/account-chrome').AccountChrome;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  closeAccountMenu: (keepEntry?: boolean | undefined, navigating?: boolean | undefined) => void;
  toolViewsOwner: ReturnType<typeof import('./toolViews').createToolViews>;
  LIVE: import('../lib/live').LiveState;
  SIDEBAR_ONLY: boolean;
  render: () => void;
  navigation: import('../navigation/routes').NavigationController;
  SESS: Record<string, import('../domain/types').Session>;
  MACHINE: Record<string, string>;
  capture: () => import('../navigation/scroll').ScrollSnapshot;
  restore: (st: import('../navigation/scroll').ScrollSnapshot, pin?: boolean) => void;
  patchSession: (dirty: ReadonlySet<string> | null) => number;
  syncJump: () => void;
}
/** Owns applicationRefresh behavior through explicit application ports. */
export function createApplicationRefresh(host: ApplicationRefreshHost) {
  function refresh(dirty: ReadonlySet<string> | null = null) {
    if (host.disposed) return;
    if (host.accountChrome.open && !host.$('.account-popover')?.isConnected)
      host.closeAccountMenu(); // a menu some redraw took away is closed
    if (host.toolViewsOwner.viewerEl || host.accountChrome.open) {
      host.LIVE.pending = true;
      return;
    } // drawn whole when the sheet or the account menu closes
    if (host.SIDEBAR_ONLY) {
      host.LIVE.pending = false;
      host.render();
      return;
    } // the page is the embedding page's: only the sidebar is redrawn
    host.LIVE.pending = false;
    const r = host.navigation.route;
    if (
      host.navigation.rendered !== r ||
      (r.v === 'session' && !host.SESS[r.id]) ||
      (r.v === 'trace' && !host.SESS[r.sid]) ||
      (r.v === 'machine' && !host.MACHINE[r.id])
    )
      return;
    const st = host.capture();
    setGeometry(host.$('#page'), 'paddingBottom', null);
    if (r.v !== 'session') {
      host.render();
      host.restore(st);
      return;
    }
    const n = host.patchSession(dirty);
    host.restore(st, st.bottom);
    if (!st.bottom && n) host.LIVE.fresh += n;
    host.syncJump();
  }

  // View state. A block's identity: its class and keys, or its own text when it has no key (a section heading).

  return { refresh };
}

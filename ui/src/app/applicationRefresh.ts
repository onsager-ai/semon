import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { createToolViews } from './toolViews';
import type { createAccountControls } from './accountControls';
import type { createLiveModel } from './liveModel';
import type { createDocumentRenderer } from './documentRenderer';
import type { createViewport } from './viewport';
import { setGeometry, updateSessionControl } from '../lib';
import type { createLocalControl } from './localControl';
import type { createNavigationView } from './navigationView';
interface ApplicationRefreshHost {
  disposed: boolean;
  controlOwner: Pick<ReturnType<typeof createLocalControl>, 'view'>;
  navigationViewOwner: Pick<ReturnType<typeof createNavigationView>, 'renderNav'>;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  toolViewsOwner: ReturnType<typeof createToolViews>;
  sidebarOnly: boolean;
  navigation: NavigationController;

  viewport: Pick<
    ReturnType<typeof createViewport>,
    'capture' | 'restore' | 'patchSession' | 'syncJump'
  >;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'machines'>;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  accountControlsOwner: Pick<
    ReturnType<typeof createAccountControls>,
    'accountChrome' | 'closeAccountMenu'
  >;
}
/** Owns applicationRefresh behavior through explicit application ports. */
export function createApplicationRefresh(host: ApplicationRefreshHost) {
  function refresh(dirty: ReadonlySet<string> | null = null) {
    if (host.disposed) return;
    if (host.accountControlsOwner.accountChrome.open && !host.$('.account-popover')?.isConnected)
      host.accountControlsOwner.closeAccountMenu(); // a menu some redraw took away is closed
    if (host.toolViewsOwner.viewerEl || host.accountControlsOwner.accountChrome.open) {
      host.liveModelOwner.LIVE.pending = true;
      return;
    } // drawn whole when the sheet or the account menu closes
    if (host.sidebarOnly) {
      host.liveModelOwner.LIVE.pending = false;
      host.documentRendererOwner.render();
      return;
    } // the page is the embedding page's: only the sidebar is redrawn
    host.liveModelOwner.LIVE.pending = false;
    const r = host.navigation.route;
    if (
      host.navigation.rendered !== r ||
      (r.v === 'session' && !host.modelStore.sessions[r.id]) ||
      (r.v === 'trace' && !host.modelStore.sessions[r.sid]) ||
      (r.v === 'machine' && !host.modelStore.machines[r.id])
    )
      return;
    const st = host.viewport.capture();
    setGeometry(host.$('#page'), 'paddingBottom', null);
    if (r.v !== 'session') {
      host.documentRendererOwner.render();
      host.viewport.restore(st);
      return;
    }
    const n = host.viewport.patchSession(dirty);
    host.viewport.restore(st, st.bottom);
    if (!st.bottom && n) host.liveModelOwner.LIVE.fresh += n;
    host.viewport.syncJump();
  }

  // View state. A block's identity: its class and keys, or its own text when it has no key (a section heading).

  function controls() {
    if (host.disposed) return;
    if (host.toolViewsOwner.viewerEl || host.accountControlsOwner.accountChrome.open) {
      host.liveModelOwner.LIVE.pending = true;
      return;
    }
    host.navigationViewOwner.renderNav();
    const route = host.navigation.route;
    if (host.sidebarOnly || route.v !== 'session' || host.navigation.rendered !== route) return;
    const anchor = host.viewport.capture();
    updateSessionControl(host.$('#page'), host.controlOwner.view(route.id));
    host.viewport.restore(anchor, anchor.bottom);
  }
  return { refresh, controls };
}

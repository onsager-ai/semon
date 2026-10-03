import type { NavigationController } from '../navigation/routes';
import type { createToolViews } from './toolViews';
import type { EffectScope } from '../app/effects';
import type { createAccountControls } from './accountControls';
import type { createOrderingControls } from './orderingControls';
import type { createDocumentRenderer } from './documentRenderer';
import type { createDestination } from './destination';
import type { createLayout } from './layout';
import type { createRecentNavigation } from './recentNavigation';
import type { createHistoryScroll } from './historyScroll';
import type { createViewport } from './viewport';
import { measureViewerBar } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
interface DocumentEventsHost {
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  sidebarOnly: boolean;
  scope: EffectScope;
  accountSheet: boolean;
  toolViewsOwner: ReturnType<typeof createToolViews>;
  navigation: NavigationController;
  findOpen: boolean;
  query: string;
  focusSessionsSearchOnRender: ApplicationRoute | null;
  phone: MediaQueryList;

  viewport: Pick<ReturnType<typeof createViewport>, 'syncJump'>;

  historyScrollOwner: Pick<
    ReturnType<typeof createHistoryScroll>,
    'currentScroll' | 'restoreScroll'
  >;

  recentNavigation: Pick<ReturnType<typeof createRecentNavigation>, 'renderLanes' | 'expandedAll'>;

  layoutOwner: Pick<ReturnType<typeof createLayout>, 'syncLayoutPrefs'>;

  destination: Pick<ReturnType<typeof createDestination>, 'go'>;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  orderingControlsOwner: Pick<ReturnType<typeof createOrderingControls>, 'orderApply'>;

  accountControlsOwner: Pick<
    ReturnType<typeof createAccountControls>,
    'shellChrome' | 'accountChrome' | 'closeAccountMenu'
  >;
}
/** Owns documentEvents behavior through explicit application ports. */
export function createDocumentEvents(host: DocumentEventsHost) {
  const sidebar = host.$('#sidebar');
  function openDrawer() {
    host.accountControlsOwner.shellChrome?.openDrawer();
  }
  function closeDrawer(quiet: boolean | undefined = undefined) {
    host.accountControlsOwner.shellChrome?.closeDrawer(quiet);
  }
  // Sidebar-only consumers retain their existing server shell.js owner.
  if (host.sidebarOnly)
    host.scope.listen(window, 'semon:drawer-open', () =>
      host.orderingControlsOwner.orderApply('side'),
    );
  if (!host.sidebarOnly)
    host.scope.listen(document, 'keydown', (e) => {
      if (e.key === 'Escape' && host.accountSheet) host.accountControlsOwner.accountChrome.escape();
      else if (e.key === 'Escape' && !host.toolViewsOwner.viewerEl) {
        closeDrawer();
        host.accountControlsOwner.closeAccountMenu();
        host.$('.session-menu')?.remove();
        host.$('#more-btn')?.setAttribute('aria-expanded', 'false');
      }
      if (
        e.key === '/' &&
        !/INPUT|TEXTAREA/.test(document.activeElement?.tagName ?? '') &&
        !(
          document.activeElement instanceof HTMLElement && document.activeElement.isContentEditable
        ) &&
        !host.toolViewsOwner.viewerEl
      ) {
        e.preventDefault();
        if (host.navigation.route.v === 'session') {
          host.findOpen = true;
          host.documentRendererOwner.render();
          host.$('#find')?.focus();
        } else if (host.navigation.route.v === 'sessions') {
          host.$('#sq')?.focus();
        } else {
          const r: ApplicationRoute = { v: 'sessions', q: host.query };
          host.focusSessionsSearchOnRender = r;
          host.destination.go(r);
        }
      }
    });
  host.scope.listen(host.phone, 'change', () => {
    closeDrawer(true);
    host.layoutOwner.syncLayoutPrefs();
    host.recentNavigation.expandedAll = null;
    host.recentNavigation.renderLanes();
    if (!host.phone.matches && host.toolViewsOwner.viewerEl?.classList.contains('kids-sheet'))
      host.toolViewsOwner.viewerEl.close(); // a sheet is a phone's: a wide screen opens the list in the tree
    if (host.navigation.route.v === 'session' || host.navigation.route.v === 'analytics') {
      const top = host.historyScrollOwner.currentScroll();
      host.documentRendererOwner.render();
      host.historyScrollOwner.restoreScroll(top);
    } else host.recentNavigation.renderLanes();
    const l2 = host.$('#topbar .meta-line');
    if (l2 && host.navigation.route.v === 'session') measureViewerBar(host.$('#topbar'));
    host.viewport.syncJump();
  });

  // ---- Live updates (deliberate difference 3) --------------------------------------------------------------------------------
  // Every screen polls /api/model?since= every 2 s while the tab is visible, one request at a time: it backs off up to 30 s
  // on errors and stops on 403 (the server restarted with a new token). A new model swaps the globals and draws the screen
  // again with its view state kept: the scroll position, anchored to the first visible block; what is open, by stable keys;
  // focus, find and filters; the drawer. A session page follows its transcript's tail
  // with /api/tx?after= and replaces only the turns that changed. An open View all sheet holds the redraw until it closes.

  return { closeDrawer };
}

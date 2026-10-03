import type { createRenderTransaction } from './renderTransaction';
import type { PageRoot } from './pageRoot';
import type { createRouteControls } from './routeControls';
import type { ViewerModelStore } from '../state/model';
import type { ViewerHost } from '../viewer-host';
import type { NavigationController } from '../navigation/routes';
import type { createTransport } from './transport';
import type { createNavigationView } from './navigationView';
import type { createRecentNavigation } from './recentNavigation';
import type { createSessionChrome } from './sessionChrome';
import type { createLayout } from './layout';
import type { createAccountControls } from './accountControls';
import type { createViewport } from './viewport';
import type { createPaging } from './paging';
import type { createOrderingControls } from './orderingControls';
import type { createScreenViews } from './screenViews';
import type { createAnalytics } from './analytics';
import type { createSessionList } from './sessionList';
import type { createDestination } from './destination';
import type { createDomain } from '../domain/calculations';
import type { createLiveModel } from './liveModel';
import { setGeometry } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
export interface DocumentRendererHost {
  renderTransaction: Pick<ReturnType<typeof createRenderTransaction>, 'begin' | 'complete'>;
  pageRoot: PageRoot;
  routeControls: Pick<ReturnType<typeof createRouteControls>, 'retain'>;
  navigation: NavigationController;
  viewerHost: ViewerHost | null;
  nativePage: { title: string; nav: string } | undefined;
  disposed: boolean;
  focusSessionsSearchOnRender: ApplicationRoute | null;
  sidebarOnly: boolean;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE'>;

  domain: Pick<ReturnType<typeof createDomain>, 'lineageOf'>;

  destination: Pick<ReturnType<typeof createDestination>, 'go' | 'goSession' | 'lanesFor'>;

  modelStore: Pick<ViewerModelStore, 'machines' | 'sessions'>;

  sessionListOwner: Pick<ReturnType<typeof createSessionList>, 'renderSessions'>;

  analytics: Pick<ReturnType<typeof createAnalytics>, 'renderAnalytics'>;

  screenViews: Pick<
    ReturnType<typeof createScreenViews>,
    'renderHome' | 'renderMachines' | 'renderMachine' | 'renderTrace' | 'renderSession'
  >;

  orderingControlsOwner: Pick<
    ReturnType<typeof createOrderingControls>,
    'ordState' | 'ordPageState'
  >;

  pagingOwner: Pick<ReturnType<typeof createPaging>, 'resetPagerInput' | 'holdProgrammaticScroll'>;

  viewport: Pick<ReturnType<typeof createViewport>, 'syncJump' | 'stopOpeningEndPin'>;

  accountControlsOwner: Pick<
    ReturnType<typeof createAccountControls>,
    'renderDrawerAccount' | 'closeAccountMenu'
  >;

  layoutOwner: Pick<ReturnType<typeof createLayout>, 'syncLayoutPrefs'>;

  sessionChrome: Pick<
    ReturnType<typeof createSessionChrome>,
    'renderTopbar' | 'syncBarLine' | 'machineLine' | 'sessionLine' | 'errOn' | 'markError'
  >;

  recentNavigation: Pick<ReturnType<typeof createRecentNavigation>, 'renderLanes'>;

  navigationViewOwner: Pick<ReturnType<typeof createNavigationView>, 'renderNav'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'tick'>;
}
/** Owns documentRenderer behavior through explicit application ports. */
export function createDocumentRenderer(host: DocumentRendererHost) {
  function render() {
    if (host.disposed) return;

    const focusSearch = host.focusSessionsSearchOnRender === host.navigation.route;
    host.focusSessionsSearchOnRender = null;
    if (host.sidebarOnly) {
      host.transportOwner.tick();
      host.navigation.rendered = host.navigation.route;
      host.navigationViewOwner.renderNav();
      host.recentNavigation.renderLanes();
      return;
    } // the embedding page draws its own page and bar
    if (host.nativePage || (host.navigation.route.v === 'machines' && host.viewerHost)) {
      host.routeControls.retain(host.navigation.route);
      host.transportOwner.tick();
      host.navigation.rendered = host.navigation.route;
      if (host.navigation.content && !host.navigation.content.element.isConnected) {
        host.pageRoot.native(host.$('#page'));
        host.$('#page').append(host.navigation.content.element);
      }
      document.title = (host.nativePage?.title ?? 'Machines') + ' · Semon';
      host.sessionChrome.renderTopbar(host.nativePage?.title ?? 'Machines');
      host.layoutOwner.syncLayoutPrefs();
      host.sessionChrome.syncBarLine();
      host.navigationViewOwner.renderNav();
      host.recentNavigation.renderLanes();
      host.accountControlsOwner.renderDrawerAccount();
      host.viewport.syncJump();
      return;
    }
    // The page first, then the bar: the bar's summary (a trace's counts, a search's matches) comes from the page.
    if (host.viewerHost)
      document.title =
        ({
          home: 'Home',
          sessions: 'Sessions',
          analytics: 'Analytics',
          machines: 'Machines',
          machine: 'Machine',
          session: 'Session',
          trace: 'Trace',
        }[host.navigation.route.v] ?? 'Semon') + ' · Semon';
    host.renderTransaction.begin();
    const page = host.$('#page'),
      r = host.navigation.route;
    host.navigation.rendered = r;
    setGeometry(page, 'paddingBottom', null);
    host.pageRoot.viewer(page, r.v);
    host.routeControls.retain(r);
    page.classList.remove('child-page');
    if (r.v === 'home') {
      host.screenViews.renderHome(page);
      host.sessionChrome.renderTopbar('Home');
    } else if (r.v === 'analytics') {
      host.analytics.renderAnalytics(page);
      host.sessionChrome.renderTopbar('Analytics', null, { analytics: true });
    } else if (r.v === 'sessions') {
      host.sessionListOwner.renderSessions(page, focusSearch);
      host.sessionChrome.renderTopbar('Sessions');
    } else if (r.v === 'machines') {
      host.screenViews.renderMachines(page);
      host.sessionChrome.renderTopbar('Machines');
    } else if (r.v === 'machine') {
      host.screenViews.renderMachine(page, r.id);
      host.sessionChrome.renderTopbar(
        host.modelStore.machines[r.id],
        { label: 'Machines', go: () => host.destination.go({ v: 'machines' }) },
        { line2: host.sessionChrome.machineLine(r.id) },
      );
    } else if (r.v === 'trace') {
      host.screenViews.renderTrace(page, r.turn);
      host.sessionChrome.renderTopbar(
        'Trace',
        {
          label: host.modelStore.sessions[r.sid].name,
          go: () => host.destination.goSession(r.sid, r.turn),
        },
        { traceSession: host.modelStore.sessions[r.sid] },
      );
    } else if (r.v === 'session') {
      const s = host.modelStore.sessions[r.id],
        lineage = host.domain.lineageOf(r.id).slice(0, -1);
      host.screenViews.renderSession(page, r.id);
      host.sessionChrome.renderTopbar(s.name, null, {
        session: s,
        lineage,
        line2: host.sessionChrome.sessionLine(s),
      });
    }
    if (r.v === 'session' && host.sessionChrome.errOn(r.id)) host.sessionChrome.markError(false);
    setGeometry(document.documentElement, 'barHeight', host.$('#topbar').offsetHeight);
    host.renderTransaction.complete(r);
  }

  return { render };
}

import type { ViewerModelStore } from '../state/model';
import type { EffectScope } from '../app/effects';
import type { NavigationController } from '../navigation/routes';
import type { ViewerHost } from '../viewer-host';
import type { createTransport } from './transport';
import type { createLiveModel } from './liveModel';
import type { createDocumentRenderer } from './documentRenderer';
import type { createDestination } from './destination';
import type { createViewport } from './viewport';
import type { createHistoryScroll } from './historyScroll';
import type { createTicker } from './ticker';
import type { RouteModel } from '../lib';
import { parseRoute, renderPlaceholder, routeUrl } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
interface BootstrapHost {
  viewerHost: ViewerHost | null;
  sidebarOnly: boolean;
  nativePage: { title: string; nav: string } | undefined;
  navigation: NavigationController;
  query: string;
  machinesPath: string | null;
  scope: EffectScope;
  disposed: boolean;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;

  tickerOwner: Pick<ReturnType<typeof createTicker>, 'ticker'>;

  historyScrollOwner: Pick<ReturnType<typeof createHistoryScroll>, 'quietTop'>;

  viewport: Pick<ReturnType<typeof createViewport>, 'syncJump'>;

  destination: Pick<
    ReturnType<typeof createDestination>,
    'revealTurn' | 'revealEntryHash' | 'openSessionAtEnd'
  >;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'LIVE' | 'remember' | 'schedule'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'api' | 'adopt' | 'enc' | 'load'>;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'machines' | 'turn'>;
}
/** Owns bootstrap behavior through explicit application ports. */
export function createBootstrap(host: BootstrapHost) {
  const routeModel: RouteModel = {
    session(id) {
      return host.modelStore.sessions[id];
    },
    machine(id) {
      return !!host.modelStore.machines[id];
    },
    turn(id) {
      return host.modelStore.turn.get(id);
    },
    get machinesPath() {
      return host.viewerHost?.machinesPath;
    },
  };
  const compatibility = new URLSearchParams(location.search).get('compat') === '1';
  const urlOf = (r: ApplicationRoute) => {
      const address = routeUrl(r, routeModel);
      if (!compatibility) return address;
      const url = new URL(address, location.href);
      url.searchParams.set('compat', '1');
      return url.pathname + url.search + url.hash;
    },
    routeOf = (location: Pick<Location, 'pathname' | 'search' | 'hash'>) =>
      parseRoute(location, routeModel);
  let retryDelay = 1000;
  let retryTimer: number | undefined;
  let loading = false;
  function boot() {
    if (host.disposed || loading) return;
    host.scope.clearTimeout(retryTimer);
    loading = true;
    host.transportOwner
      .api(
        '/api/model?delta=1' +
          (host.viewerHost?.controlStream || host.viewerHost?.catalogControlStream
            ? '&content=1'
            : ''),
      )
      .then(async (m) => {
        if (host.disposed) return;
        const adopted = host.transportOwner.adopt(m);
        host.liveModelOwner.LIVE.version = adopted.version;
        host.liveModelOwner.remember(adopted);
        if (host.sidebarOnly || host.nativePage) {
          if (host.nativePage)
            host.navigation.route = host.navigation.historyRoute(
              { v: host.nativePage.nav },
              { v: 'home' },
            );
          host.documentRendererOwner.render();
          host.liveModelOwner.schedule(2000);
          return;
        }
        host.navigation.route = routeOf(location);
        if (host.navigation.route.v === 'sessions')
          host.query = (new URLSearchParams(location.search).get('q') ?? '').trim(); // direct Sessions links can prefill its search field
        if (host.navigation.route.v === 'machines' && host.machinesPath && !host.viewerHost) {
          location.assign(host.machinesPath);
          return;
        }
        try {
          history.replaceState(
            { ...host.navigation.route, scrollTop: 0 },
            '',
            urlOf(host.navigation.route) +
              (host.navigation.route.v === 'session'
                ? location.hash
                : host.navigation.route.v === 'sessions' && host.query
                  ? '?q=' + host.transportOwner.enc(host.query)
                  : ''),
          );
        } catch {}
        const done = () => {
          host.documentRendererOwner.render();
          if (
            host.navigation.route.v === 'session' &&
            ('turn' in host.navigation.route ? host.navigation.route.turn : undefined)
          ) {
            host.destination.revealTurn(
              ('turn' in host.navigation.route ? host.navigation.route.turn : undefined) ?? '',
              true,
            );
            if (location.hash)
              host.scope.frame(() =>
                host.scope.frame((...args: Parameters<typeof host.destination.revealEntryHash>) =>
                  host.destination.revealEntryHash(...args),
                ),
              );
          } else if (host.navigation.route.v === 'session' && location.hash)
            host.destination.revealEntryHash();
          else if (host.navigation.route.v === 'session') {
            host.destination.openSessionAtEnd();
            host.viewport.syncJump();
          } else host.historyScrollOwner.quietTop();
          host.liveModelOwner.schedule(2000);
          host.scope.interval(
            (...args: Parameters<typeof host.tickerOwner.ticker>) =>
              host.tickerOwner.ticker(...args),
            1000,
          );
        };
        const initialRoute = host.navigation.route,
          p = host.transportOwner.load(initialRoute);
        if (p) {
          try {
            await p;
          } catch (error) {
            if (host.disposed || host.navigation.route !== initialRoute) return;
            throw error;
          }
        }
        if (!host.disposed && host.navigation.route === initialRoute) done();
      })
      .catch((err) => {
        if (host.disposed) return;
        if (host.viewerHost?.modelFailed?.(err?.status ?? 0)) return;
        if (err?.status !== 403) {
          retryTimer = host.scope.timeout(boot, retryDelay);
          retryDelay = Math.min(30000, retryDelay * 2);
        }
        if (host.viewerHost) return;
        renderPlaceholder(
          host.$(host.sidebarOnly ? '#lanes' : '#page'),
          "Couldn't load the sessions: " +
            err.message +
            (err?.status === 403
              ? '. Reload with an authorized URL.'
              : '. Retrying automatically…'),
        );
      })
      .finally(() => {
        loading = false;
      });
  }

  return { routeModel, urlOf, routeOf, boot };
}

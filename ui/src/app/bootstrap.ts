import type { RouteModel } from '../lib';
import { parseRoute, renderPlaceholder, routeUrl } from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
interface BootstrapHost {
  SESS: Record<string, import('../domain/types').Session>;
  MACHINE: Record<string, string>;
  TURN: Map<string, import('../domain/types').Turn>;
  viewerHost: import('../viewer-host').ViewerHost | null;
  api: (path: string, signal?: AbortSignal | undefined, unchanged?: boolean) => Promise<unknown>;
  adopt: (value: unknown) => import('../lib/model').ModelWire;
  LIVE: import('../lib/live').LiveState;
  remember: (_m: import('../lib/model').ModelWire) => void;
  SIDEBAR_ONLY: boolean;
  NATIVE_PAGE: { title: string; nav: string } | undefined;
  navigation: import('../navigation/routes').NavigationController;
  render: () => void;
  schedule: (ms: number) => void;
  query: string;
  NAV_MACHINES: string | null;
  enc: (uriComponent: string | number | boolean) => string;
  revealTurn: (id: string, flash: boolean) => void;
  scope: import('../app/effects').EffectScope;
  revealEntryHash: () => void;
  openSessionAtEnd: () => void;
  syncJump: () => void;
  quietTop: () => void;
  ticker: () => void;
  load: (
    r: import('../navigation/routes').ApplicationRoute,
    signal?: AbortSignal | undefined,
  ) => Promise<void> | null;
  disposed: boolean;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
}
/** Owns bootstrap behavior through explicit application ports. */
export function createBootstrap(host: BootstrapHost) {
  const routeModel: RouteModel = {
    session(id) {
      return host.SESS[id];
    },
    machine(id) {
      return !!host.MACHINE[id];
    },
    turn(id) {
      return host.TURN.get(id);
    },
    get machinesPath() {
      return host.viewerHost?.machinesPath;
    },
  };
  const urlOf = (r: ApplicationRoute) => routeUrl(r, routeModel),
    routeOf = (location: Pick<Location, 'pathname' | 'search' | 'hash'>) =>
      parseRoute(location, routeModel);
  function boot() {
    host.api('/api/model?delta=1').then(
      (m) => {
        const adopted = host.adopt(m);
        host.LIVE.version = adopted.version;
        host.remember(adopted);
        if (host.SIDEBAR_ONLY || host.NATIVE_PAGE) {
          if (host.NATIVE_PAGE)
            host.navigation.route = host.navigation.historyRoute(
              { v: host.NATIVE_PAGE.nav },
              { v: 'home' },
            );
          host.render();
          host.schedule(2000);
          return;
        }
        host.navigation.route = routeOf(location);
        if (host.navigation.route.v === 'sessions')
          host.query = (new URLSearchParams(location.search).get('q') ?? '').trim(); // direct Sessions links can prefill its search field
        if (host.navigation.route.v === 'machines' && host.NAV_MACHINES && !host.viewerHost) {
          location.assign(host.NAV_MACHINES);
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
                  ? '?q=' + host.enc(host.query)
                  : ''),
          );
        } catch {}
        const done = () => {
          host.render();
          if (
            host.navigation.route.v === 'session' &&
            ('turn' in host.navigation.route ? host.navigation.route.turn : undefined)
          ) {
            host.revealTurn(
              ('turn' in host.navigation.route ? host.navigation.route.turn : undefined) ?? '',
              true,
            );
            if (location.hash) host.scope.frame(() => host.scope.frame(host.revealEntryHash));
          } else if (host.navigation.route.v === 'session' && location.hash) host.revealEntryHash();
          else if (host.navigation.route.v === 'session') {
            host.openSessionAtEnd();
            host.syncJump();
          } else host.quietTop();
          host.schedule(2000);
          host.scope.interval(host.ticker, 1000);
        };
        const initialRoute = host.navigation.route,
          p = host.load(initialRoute);
        if (p)
          p.then(() => {
            done();
          }, done);
        else {
          done();
        }
      },
      (err) => {
        if (host.disposed) return;
        if (host.viewerHost?.modelFailed?.(err?.status ?? 0)) return;
        if (host.viewerHost) {
          console.warn('semon: model unavailable', err.status);
          return;
        }
        renderPlaceholder(
          host.$(host.SIDEBAR_ONLY ? '#lanes' : '#page'),
          "Couldn't load the sessions: " + err.message,
        );
      },
    );
  }

  return { routeModel, urlOf, routeOf, boot };
}

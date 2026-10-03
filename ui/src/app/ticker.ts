import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { createLiveModel } from './liveModel';
import type { createTransport } from './transport';
import type { createScreenViews } from './screenViews';
import { updateSessionClock } from '../lib';
interface TickerHost {
  navigation: NavigationController;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  now: number;

  screenViews: Pick<ReturnType<typeof createScreenViews>, 'renderHome'>;

  modelStore: Pick<ViewerModelStore, 'sessions'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'tick' | 'fetchedAt'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'visible'>;
}
/** Owns ticker behavior through explicit application ports. */
export function createTicker(host: TickerHost) {
  const running = (ms: number) => {
    const x = Math.max(0, Math.floor(ms / 1000));
    return x < 60 ? x + 's' : Math.floor(x / 60) + 'm ' + (x % 60) + 's';
  };
  function ticker() {
    if (!host.liveModelOwner.visible() || Date.now() === host.transportOwner.fetchedAt) return;
    host.transportOwner.tick();
    if (host.navigation.route.v === 'session' && host.navigation.rendered === host.navigation.route)
      updateSessionClock(
        host.$('#page'),
        host.now,
        Object.fromEntries(
          Object.values(host.modelStore.sessions)
            .filter((s) => s.activity?.[3] != null)
            .map((s) => [s.id, s.activity![3]!]),
        ),
      );
    else if (
      host.navigation.route.v === 'home' &&
      host.navigation.rendered === host.navigation.route
    )
      host.screenViews.renderHome(host.$('#page'));
  }

  return { ticker, running };
}

import type { ViewerHost } from '../viewer-host';
import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { createBootstrap } from './bootstrap';
import type { createLayout } from './layout';
import type { createDomain } from '../domain/calculations';
import type { createAccountControls } from './accountControls';
import type { createDestination } from './destination';
import type { Session } from '../domain/types';
import type { EffectScope } from './effects';
import { renderShellNavigation, projectShellNavigation } from '../lib';
import { HARNESS } from './registry';
interface NavigationViewHost {
  viewerHost: ViewerHost | null;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  machinesPath: string | null;
  sidebarOnly: boolean;
  nativePage: { title: string; nav: string } | undefined;
  navigation: NavigationController;
  scope: Pick<EffectScope, 'own' | 'timeout'>;

  destination: Pick<ReturnType<typeof createDestination>, 'go'>;

  accountControlsOwner: Pick<ReturnType<typeof createAccountControls>, 'shellChrome'>;

  modelStore: Pick<ViewerModelStore, 'machines' | 'machineUp' | 'turns'>;

  domain: Pick<ReturnType<typeof createDomain>, 'inbox'>;

  layoutOwner: Pick<ReturnType<typeof createLayout>, 'app' | 'railMode'>;

  bootstrapOwner: Pick<ReturnType<typeof createBootstrap>, 'urlOf'>;
}
/** Owns navigationView behavior through explicit application ports. */
export function createNavigationView(host: NavigationViewHost) {
  function renderNav() {
    const nav = host.$('#nav');
    const route =
      (host.sidebarOnly ? host.layoutOwner.app.dataset.viewerNav : host.nativePage?.nav) ??
      host.navigation.route.v;
    const destinations = projectShellNavigation(
      host.viewerHost?.navigation ?? { leading: host.viewerHost?.nativeNavigation },
      route,
      {
        home: host.bootstrapOwner.urlOf({ v: 'home' }),
        sessions: host.bootstrapOwner.urlOf({ v: 'sessions' }),
        analytics: host.bootstrapOwner.urlOf({ v: 'analytics' }),
        machines: host.machinesPath ?? host.bootstrapOwner.urlOf({ v: 'machines' }),
      },
      host.nativePage
        ? {}
        : {
            home: host.domain.inbox().length,
            machines: Object.keys(host.modelStore.machines).filter(
              (m) => !host.modelStore.machineUp[m],
            ).length,
          },
    );
    if (host.accountControlsOwner.shellChrome)
      host.accountControlsOwner.shellChrome.update(destinations, host.layoutOwner.railMode);
    else
      renderShellNavigation(nav, destinations, (destination) => {
        if (host.viewerHost?.nativeNavigation?.some((item) => item.key === destination.key))
          return false;
        host.destination.go(host.navigation.historyRoute({ v: destination.key }, { v: 'home' }));
        return true;
      });
  }
  // Sessions match by name, repo, branch, machine, harness and the messages that started their turns.
  const sessMatch = (s: Session, q: string) =>
    !q ||
    [
      s.name,
      s.repo,
      s.branch,
      host.modelStore.machines[s.machine],
      s.movedFrom ? host.modelStore.machines[s.movedFrom] : '',
      HARNESS[s.harness],
      s.role ? 'role no repo' : '',
      ...(host.modelStore.turns[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? ''),
    ]
      .join(' ')
      .toLowerCase()
      .includes(q.toLowerCase());
  // Relationship indexes are invalidated by accepted model/transcript transactions.
  const unsubscribe = host.viewerHost?.subscribeNavigation?.(() =>
    host.scope.timeout(renderNav, 0),
  );
  if (unsubscribe) host.scope.own(unsubscribe);

  return { renderNav, sessMatch };
}

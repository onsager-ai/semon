import type { ViewerHost } from '../viewer-host';
import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { createBootstrap } from './bootstrap';
import type { createLayout } from './layout';
import type { createDomain } from '../domain/calculations';
import type { createAccountControls } from './accountControls';
import type { createDestination } from './destination';
import type { Session } from '../domain/types';
import type { ShellDestination } from '../lib';
import { renderShellNavigation } from '../lib';
import { HARNESS, I } from './registry';
interface NavigationViewHost {
  viewerHost: ViewerHost | null;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  machinesPath: string | null;
  sidebarOnly: boolean;
  nativePage: { title: string; nav: string } | undefined;
  navigation: NavigationController;

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
    const nav = host.$('#nav'),
      destinations: ShellDestination[] = [];
    // A session or a trace sits under Sessions, a machine under Machines.
    const under: Record<'home' | 'analytics' | 'sessions' | 'machines', string[]> = {
      home: ['home'],
      analytics: ['analytics'],
      sessions: ['sessions', 'session', 'trace'],
      machines: ['machines', 'machine'],
    };
    const item = (
      v: 'home' | 'analytics' | 'sessions' | 'machines',
      label: string,
      ic: string,
      count?: number,
      hot?: boolean,
    ) => {
      destinations.push({
        key: v,
        label,
        icon: ic,
        href:
          v === 'machines' && host.machinesPath
            ? host.machinesPath
            : host.bootstrapOwner.urlOf({ v }),
        current: under[v].includes(
          (host.sidebarOnly ? host.layoutOwner.app.dataset.viewerNav : host.nativePage?.nav) ??
            host.navigation.route.v,
        ),
        count,
        hot,
      });
    };
    destinations.push(...(host.viewerHost?.nativeNavigation ?? []));
    item('home', 'Home', I.home, host.domain.inbox().length, true);
    item('sessions', 'Sessions', I.sessions);
    item('analytics', 'Analytics', I.chart);
    item(
      'machines',
      'Machines',
      I.machine,
      Object.keys(host.modelStore.machines).filter((m) => !host.modelStore.machineUp[m]).length,
      true,
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

  return { renderNav, sessMatch };
}

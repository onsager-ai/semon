import type { Session } from '../domain/types';
import type { ShellDestination } from '../lib';
import { renderShellNavigation } from '../lib';
import { HARNESS, I } from './registry';
interface NavigationViewHost {
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  NAV_MACHINES: string | null;
  urlOf: (r: import('../navigation/routes').ApplicationRoute) => string;
  SIDEBAR_ONLY: boolean;
  app: HTMLElement;
  NATIVE_PAGE: { title: string; nav: string } | undefined;
  navigation: import('../navigation/routes').NavigationController;
  inbox: () => import('../domain/types').Handoff[];
  MACHINE: Record<string, string>;
  MACHINE_UP: Record<string, boolean>;
  shellChrome: import('../lib/shell').ShellChrome | null;
  layoutOwner: ReturnType<typeof import('./layout').createLayout>;
  go: (
    r: import('../navigation/routes').ApplicationRoute,
    fromHistory?: boolean,
    prepared?: boolean,
    nextContent?: import('../viewer-host').ViewerContent | null,
  ) => void;
  TURNS: Record<string, import('../domain/types').Turn[]>;
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
        href: v === 'machines' && host.NAV_MACHINES ? host.NAV_MACHINES : host.urlOf({ v }),
        current: under[v].includes(
          (host.SIDEBAR_ONLY ? host.app.dataset.viewerNav : host.NATIVE_PAGE?.nav) ??
            host.navigation.route.v,
        ),
        count,
        hot,
      });
    };
    item('home', 'Home', I.home, host.inbox().length, true);
    item('sessions', 'Sessions', I.sessions);
    item('analytics', 'Analytics', I.chart);
    item(
      'machines',
      'Machines',
      I.machine,
      Object.keys(host.MACHINE).filter((m) => !host.MACHINE_UP[m]).length,
      true,
    );
    if (host.shellChrome) host.shellChrome.update(destinations, host.layoutOwner.railMode);
    else
      renderShellNavigation(nav, destinations, (destination) => {
        host.go(host.navigation.historyRoute({ v: destination.key }, { v: 'home' }));
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
      host.MACHINE[s.machine],
      s.movedFrom ? host.MACHINE[s.movedFrom] : '',
      HARNESS[s.harness],
      s.role ? 'role no repo' : '',
      ...(host.TURNS[s.id] ?? []).map((t) => t.start?.brief ?? t.u?.text ?? ''),
    ]
      .join(' ')
      .toLowerCase()
      .includes(q.toLowerCase());
  // Built once per model and per render (both drop it) and shared: callers copy an array before reordering it.

  return { renderNav, sessMatch };
}

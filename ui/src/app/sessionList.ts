import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { createDomain } from '../domain/calculations';
import type { createRecentNavigation } from './recentNavigation';
import type { createAnalytics } from './analytics';
import type { createNavigationView } from './navigationView';
import type { createOrderingControls } from './orderingControls';
import type { createScreenViews } from './screenViews';
import type { createSessionChrome } from './sessionChrome';
import type { createDestination } from './destination';
import type { createDocumentRenderer } from './documentRenderer';
import { shortModel } from '../domain/format';
import type { Session } from '../domain/types';
import { renderSessionsScreen } from '../lib';
import { HARNESS, I, STATE } from './registry';
interface SessionListHost {
  showApprovalReviews: boolean;
  query: string;
  navigation: NavigationController;
  groupBy: string;
  dur: (a: number, b: number | null | undefined) => string;
  ago: (t: number) => string;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  destination: Pick<ReturnType<typeof createDestination>, 'goSession'>;

  sessionChrome: Pick<ReturnType<typeof createSessionChrome>, 'modelIdOf' | 'observeTitle'>;

  screenViews: Pick<ReturnType<typeof createScreenViews>, 'harnessSnapshot'>;

  orderingControlsOwner: Pick<
    ReturnType<typeof createOrderingControls>,
    'orderScope' | 'pageSig' | 'pageState' | 'orderList' | 'byLast' | 'ORD'
  >;

  navigationViewOwner: Pick<ReturnType<typeof createNavigationView>, 'sessMatch'>;

  analytics: Pick<
    ReturnType<typeof createAnalytics>,
    'matchesSessionFacets' | 'renderFacetFilters'
  >;

  recentNavigation: Pick<
    ReturnType<typeof createRecentNavigation>,
    'isApprovalReview' | 'renderLanes'
  >;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'machines'>;

  domain: Pick<ReturnType<typeof createDomain>, 'parentOf' | 'shortHost' | 'hostOf'>;
}
/** Owns sessionList behavior through explicit application ports. */
export function createSessionList(host: SessionListHost) {
  const laneOf = (sid: string) => {
    const seen = new Set<string>();
    while (host.domain.parentOf(sid) && !seen.has(sid)) {
      seen.add(sid);
      sid = host.domain.parentOf(sid)!;
    }
    return sid;
  };
  function childRuns(sid: string) {
    const kids = Object.values(host.modelStore.sessions).filter(
        (x) =>
          x.id !== sid &&
          laneOf(x.id) === sid &&
          (host.showApprovalReviews || !host.recentNavigation.isApprovalReview(x)),
      ),
      sub = kids.filter((x) => x.kind === 'Subagent').length,
      cdx = kids.filter((x) => x.kind === 'Codex run').length,
      other = kids.length - sub - cdx;
    return [
      sub ? sub + (sub === 1 ? ' subagent' : ' subagents') : null,
      cdx ? cdx + (cdx === 1 ? ' Codex run' : ' Codex runs') : null,
      other ? other + (other === 1 ? ' other run' : ' other runs') : null,
    ]
      .filter(Boolean)
      .join(' · ');
  }
  function renderSessions(page: HTMLElement, focusSearch: boolean = false) {
    const all = Object.values(host.modelStore.sessions).filter(
      (s) =>
        (host.showApprovalReviews || !host.recentNavigation.isApprovalReview(s)) &&
        host.analytics.matchesSessionFacets(s),
    );
    const lanes = all.filter((s) => host.navigationViewOwner.sessMatch(s, host.query)),
      order = host.orderingControlsOwner.orderScope(
        'page',
        host.orderingControlsOwner.pageSig(),
        host.navigation.route,
        host.orderingControlsOwner.pageState(),
      );
    let groups: [string, Session[], number][];
    if (host.groupBy === 'recent')
      groups = [
        [
          '',
          host.orderingControlsOwner.orderList(
            order,
            'recent',
            lanes,
            (...args: Parameters<typeof host.orderingControlsOwner.byLast>) =>
              host.orderingControlsOwner.byLast(...args),
          ),
          lanes.length,
        ],
      ];
    else {
      const keysOf: Record<string, (s: Session) => string> = {
        machine: (s) => host.modelStore.machines[s.machine],
        project: (s) => s.repo ?? 'No repo (roles)',
        harness: (s) => HARNESS[s.harness],
      };
      const key = keysOf[host.groupBy] ?? ((s: Session) => s.name);
      const keys = [...new Set(lanes.map(key))].sort(
        (a, b) =>
          Number(a.startsWith('No repo')) - Number(b.startsWith('No repo')) || a.localeCompare(b),
      );
      groups = keys.map((k) => {
        const rows = lanes.filter((s) => key(s) === k);
        return [
          k,
          host.orderingControlsOwner.orderList(
            order,
            'g:' + k,
            rows,
            (...args: Parameters<typeof host.orderingControlsOwner.byLast>) =>
              host.orderingControlsOwner.byLast(...args),
          ),
          rows.length,
        ];
      });
    }
    const rows = groups
      .filter(([, items, total]) => items.length || !total)
      .map(([title, items, total]) => ({
        title,
        total,
        harness:
          host.groupBy === 'harness'
            ? host.screenViews.harnessSnapshot(
                Object.keys(HARNESS).find((k) => HARNESS[k] === title) ?? '',
              )
            : undefined,
        rows: items.map((s) => {
          const fields: {
            className: string;
            text: string;
            priority: number;
            tip?: string;
            icon?: string;
          }[] = [
            {
              className: 'row-duration',
              text: host.dur(s.start, s.state === 'work' || s.state === 'wait' ? null : s.last),
              priority: 1,
              tip: 'Duration',
              icon: I.duration,
            },
          ];
          if (Object.keys(host.modelStore.machines).length > 1)
            fields.push({
              className: 'row-machine host',
              text: host.domain.shortHost(s),
              priority: 2,
              tip: 'Machine: ' + host.domain.hostOf(s),
              icon: I.machine,
            });
          if (s.repo)
            fields.push({
              className: 'repo-short',
              text: s.repo,
              priority: 3,
              tip: 'Repo: ' + s.repo,
              icon: I.repo,
            });
          const counts = childRuns(s.id);
          if (counts)
            fields.push({
              className: 'row-counts',
              text: counts.replace(/Codex runs/g, 'runs'),
              priority: 4,
            });
          return {
            id: s.id,
            name: s.name,
            state: s.state,
            stateLabel: STATE[s.state] ?? s.state,
            age: host.ago(s.last),
            harness: host.screenViews.harnessSnapshot(s.harness),
            model: shortModel(s.model ?? s.modelId),
            modelTip: 'Model: ' + host.sessionChrome.modelIdOf(s),
            delegation: host.domain.parentOf(s.id) ? I.stack : undefined,
            fields,
          };
        }),
      }));
    renderSessionsScreen(
      page,
      {
        total: all.length,
        working: all.filter((s) => s.state === 'work').length,
        waiting: all.filter((s) => s.state === 'wait').length,
        query: host.query,
        groupBy: host.groupBy,
        reviews: host.showApprovalReviews,
        showReviews:
          Object.values(host.modelStore.sessions).some(
            (...args: Parameters<typeof host.recentNavigation.isApprovalReview>) =>
              host.recentNavigation.isApprovalReview(...args),
          ) || host.showApprovalReviews,
        groups: rows,
        matches: lanes.length,
      },
      {
        session: (...args: Parameters<typeof host.destination.goSession>) =>
          host.destination.goSession(...args),
        committed: (...args: Parameters<typeof host.sessionChrome.observeTitle>) =>
          host.sessionChrome.observeTitle(...args),
        facets: () =>
          host.analytics.renderFacetFilters(page, () => host.documentRendererOwner.render()),
        search(value) {
          host.query = value.trim();
          const address = new URL(location.href);
          if (host.query) address.searchParams.set('q', host.query);
          else address.searchParams.delete('q');
          history.replaceState(history.state, '', address);
          renderSessions(page);
          host.recentNavigation.renderLanes();
        },
        group(value) {
          host.groupBy = value;
          renderSessions(page);
          host.recentNavigation.renderLanes();
        },
        reviews() {
          host.showApprovalReviews = !host.showApprovalReviews;
          host.orderingControlsOwner.ORD.delete('page');
          host.orderingControlsOwner.ORD.delete('side');
          host.documentRendererOwner.render();
        },
      },
      focusSearch,
    );
    host.recentNavigation.renderLanes();
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------

  return { renderSessions };
}

import { shortModel } from '../domain/format';
import type { Session } from '../domain/types';
import { renderSessionsScreen } from '../lib';
import { HARNESS, I, STATE } from './registry';
interface SessionListHost {
  parentOf: (sid: string) => string | undefined;
  SESS: Record<string, import('../domain/types').Session>;
  showApprovalReviews: boolean;
  isApprovalReview: (s: import('../domain/types').Session) => boolean;
  matchesSessionFacets: (s: import('../domain/types').Session) => boolean;
  sessMatch: (s: import('../domain/types').Session, q: string) => boolean;
  query: string;
  orderScope: (
    name: string,
    sig: string,
    tie: import('../navigation/routes').ApplicationRoute | null,
    state: { inView: boolean; touched: boolean },
  ) => import('../lib/ordering').OrderScope<import('../navigation/routes').ApplicationRoute>;
  pageSig: () => string;
  navigation: import('../navigation/routes').NavigationController;
  pageState: () => { inView: boolean; touched: boolean };
  groupBy: string;
  orderList: <Row extends { id: string }, Tie extends object>(
    scope: import('../lib/ordering').OrderScope<Tie>,
    key: string,
    items: readonly Row[],
    compare: (a: Row, b: Row) => number,
    {
      must,
      limit,
      quiet,
      seed,
    }?: {
      must?: ReadonlySet<string> | null | undefined;
      limit?: number | undefined;
      quiet?: boolean | undefined;
      seed?: boolean | undefined;
    },
  ) => Row[];
  byLast: (a: import('../domain/types').Session, b: import('../domain/types').Session) => number;
  MACHINE: Record<string, string>;
  harnessSnapshot: (id: string) => import('../lib/screens').HarnessMark | undefined;
  dur: (a: number, b: number | null | undefined) => string;
  shortHost: (s: import('../domain/types').Session) => string;
  hostOf: (s: import('../domain/types').Session) => string;
  ago: (t: number) => string;
  modelIdOf: (s: import('../domain/types').Session) => string;
  goSession: (id: string, turn?: string | undefined) => void;
  observeTitle: () => void;
  renderFacetFilters: (box: HTMLElement, onChange: () => void) => HTMLElement;
  render: () => void;
  renderLanes: () => void;
  ORD: Map<
    string,
    import('../lib/ordering').OrderScope<import('../navigation/routes').ApplicationRoute>
  >;
}
/** Owns sessionList behavior through explicit application ports. */
export function createSessionList(host: SessionListHost) {
  const laneOf = (sid: string) => {
    const seen = new Set<string>();
    while (host.parentOf(sid) && !seen.has(sid)) {
      seen.add(sid);
      sid = host.parentOf(sid)!;
    }
    return sid;
  };
  function childRuns(sid: string) {
    const kids = Object.values(host.SESS).filter(
        (x) =>
          x.id !== sid &&
          laneOf(x.id) === sid &&
          (host.showApprovalReviews || !host.isApprovalReview(x)),
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
    const all = Object.values(host.SESS).filter(
      (s) =>
        (host.showApprovalReviews || !host.isApprovalReview(s)) && host.matchesSessionFacets(s),
    );
    const lanes = all.filter((s) => host.sessMatch(s, host.query)),
      order = host.orderScope('page', host.pageSig(), host.navigation.route, host.pageState());
    let groups: [string, Session[], number][];
    if (host.groupBy === 'recent')
      groups = [['', host.orderList(order, 'recent', lanes, host.byLast), lanes.length]];
    else {
      const keysOf: Record<string, (s: Session) => string> = {
        machine: (s) => host.MACHINE[s.machine],
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
        return [k, host.orderList(order, 'g:' + k, rows, host.byLast), rows.length];
      });
    }
    const rows = groups
      .filter(([, items, total]) => items.length || !total)
      .map(([title, items, total]) => ({
        title,
        total,
        harness:
          host.groupBy === 'harness'
            ? host.harnessSnapshot(Object.keys(HARNESS).find((k) => HARNESS[k] === title) ?? '')
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
          if (Object.keys(host.MACHINE).length > 1)
            fields.push({
              className: 'row-machine host',
              text: host.shortHost(s),
              priority: 2,
              tip: 'Machine: ' + host.hostOf(s),
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
            harness: host.harnessSnapshot(s.harness),
            model: shortModel(s.model ?? s.modelId),
            modelTip: 'Model: ' + host.modelIdOf(s),
            delegation: host.parentOf(s.id) ? I.stack : undefined,
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
          Object.values(host.SESS).some(host.isApprovalReview) || host.showApprovalReviews,
        groups: rows,
        matches: lanes.length,
      },
      {
        session: host.goSession,
        committed: host.observeTitle,
        facets: () => host.renderFacetFilters(page, () => host.render()),
        search(value) {
          host.query = value.trim();
          const address = new URL(location.href);
          if (host.query) address.searchParams.set('q', host.query);
          else address.searchParams.delete('q');
          history.replaceState(history.state, '', address);
          renderSessions(page);
          host.renderLanes();
        },
        group(value) {
          host.groupBy = value;
          renderSessions(page);
          host.renderLanes();
        },
        reviews() {
          host.showApprovalReviews = !host.showApprovalReviews;
          host.ORD.delete('page');
          host.ORD.delete('side');
          host.render();
        },
      },
      focusSearch,
    );
    host.renderLanes();
  }

  // ---- Drawer (phone) ---------------------------------------------------------------------------------------------------------

  return { renderSessions };
}

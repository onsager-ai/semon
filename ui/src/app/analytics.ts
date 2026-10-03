import { HARNESS_SHORT } from './registry';
import { HARNESS } from './registry';
import type { ViewerModelStore } from '../state/model';
import type { NavigationController } from '../navigation/routes';
import type { EffectScope } from '../app/effects';
import type { createTransport } from './transport';
import type { createLiveModel } from './liveModel';
import type { createToolViews } from './toolViews';
import type { createAccountControls } from './accountControls';
import type { createViewport } from './viewport';
import type { createDocumentRenderer } from './documentRenderer';
import type { createRouteControls } from './routeControls';
import type { createHistoryScroll } from './historyScroll';
import type { createApplicationRefresh } from './applicationRefresh';
import type { createSessionChrome } from './sessionChrome';
import type { createDomain } from '../domain/calculations';
import type { createDestination } from './destination';
import type { createScreenViews } from './screenViews';
import type { createBootstrap } from './bootstrap';
import type { createRecentNavigation } from './recentNavigation';
import { render as releaseRoot } from 'preact';
import { countText, hLabel, liveUrl, niceStep, shortModel, timeText } from '../domain/format';
import type { Session } from '../domain/types';
import type { AnalyticsSnapshot, Breakdown, Chart, Metric, ModelBand } from '../lib';
import {
  createFacetChrome,
  createNativeSheet,
  createSelect,
  renderAnalyticsScreen,
  renderModelItems,
  renderSliceBody,
} from '../lib';
import type { ApplicationRoute } from '../navigation/routes';
import {
  parseAnalytics,
  type AnalyticsData,
  type AnalyticsModelGroup,
  type AnalyticsSliceItem,
} from '../state/analytics-wire';
interface SlotContext {
  destroy?: () => void;
  sync: () => void;
  onChange: () => void;
}
interface ControlSlot {
  route: ApplicationRoute;
  box: HTMLElement;
  ctx: SlotContext;
  el: HTMLElement;
}
type FacetKey = 'repo' | 'machine' | 'harness' | 'model';
interface AnalyticsHost {
  routeControls: Pick<ReturnType<typeof createRouteControls>, 'slot'>;
  dialogs: Map<HTMLDialogElement, { destroy(): void }>;
  analyticsRange: number;
  sessionFilters: { repo: string; machine: string; harness: string; model: string };
  scope: EffectScope;
  disposed: boolean;
  navigation: NavigationController;
  sidebarOnly: boolean;
  $: <T extends HTMLElement = HTMLElement>(s: string, r?: ParentNode) => T;
  pendingSessionOpen: string | null;
  afterPop: (() => void) | null;
  clock: (t: number) => string;
  analyticsMeasure: string;
  query: string;
  groupBy: string;

  recentNavigation: Pick<ReturnType<typeof createRecentNavigation>, 'COST_TIP'>;

  bootstrapOwner: Pick<ReturnType<typeof createBootstrap>, 'urlOf'>;

  screenViews: Pick<ReturnType<typeof createScreenViews>, 'harnessSnapshot'>;

  destination: Pick<ReturnType<typeof createDestination>, 'goTrace' | 'goSession' | 'go'>;

  domain: Pick<ReturnType<typeof createDomain>, 'asMoney'>;

  sessionChrome: Pick<ReturnType<typeof createSessionChrome>, 'panel' | 'observeTitle'>;

  applicationRefreshOwner: Pick<ReturnType<typeof createApplicationRefresh>, 'refresh'>;

  historyScrollOwner: Pick<
    ReturnType<typeof createHistoryScroll>,
    'currentScroll' | 'restoreScroll'
  >;

  modelStore: Pick<ViewerModelStore, 'sessions' | 'machines' | 'turns'>;

  documentRendererOwner: Pick<ReturnType<typeof createDocumentRenderer>, 'render'>;

  viewport: Pick<ReturnType<typeof createViewport>, 'capture' | 'restore'>;

  accountControlsOwner: Pick<ReturnType<typeof createAccountControls>, 'accountChrome'>;

  toolViewsOwner: Pick<ReturnType<typeof createToolViews>, 'viewerEl' | 'skipPop'>;

  liveModelOwner: Pick<ReturnType<typeof createLiveModel>, 'ended' | 'LIVE' | 'visible'>;

  transportOwner: Pick<ReturnType<typeof createTransport>, 'enc'>;
}
/** Owns analytics behavior through explicit application ports. */
export function createAnalytics(host: AnalyticsHost) {
  const MIN = 60000,
    HOUR = 60 * MIN,
    AN_EVERY = 10000,
    AN_KEEP = 8;
  const AN: {
    answers: Map<string, { etag: string | null; data: AnalyticsData }>;
    inflight: Promise<boolean> | null;
    again: boolean;
    againAsked: boolean;
    timer: number | undefined;
    error: string | null;
    failedAt: number;
  } = {
    answers: new Map(),
    inflight: null,
    again: false,
    againAsked: false,
    timer: undefined,
    error: null,
    failedAt: 0,
  };
  function analyticsQuery() {
    const q = ['range=' + (host.analyticsRange === 1 ? '24h' : host.analyticsRange + 'd')];
    // "No repo" is an empty repo; an unset filter isn't sent.
    if (host.sessionFilters.repo)
      q.push(
        'repo=' +
          (host.sessionFilters.repo === '__none__'
            ? ''
            : host.transportOwner.enc(host.sessionFilters.repo)),
      );
    for (const key of ['machine', 'harness', 'model'] as const)
      if (host.sessionFilters[key])
        q.push(key + '=' + host.transportOwner.enc(host.sessionFilters[key]));
    return q.join('&');
  }
  const analyticsData = () => AN.answers.get(analyticsQuery())?.data ?? null;
  // One request at a time: a change while one is out asks again when it is back. Resolves true when the answer changed.
  function fetchAnalytics(askedByUser = false): Promise<boolean> {
    if (AN.inflight) {
      AN.again = true;
      AN.againAsked ||= askedByUser;
      return AN.inflight;
    }
    const key = analyticsQuery(),
      kept = AN.answers.get(key);
    const controller = host.scope.request();
    const asked = fetch('/api/analytics?' + key, {
      signal: controller.signal,
      credentials: 'same-origin',
      headers: kept?.etag ? { 'If-None-Match': kept.etag } : {},
    })
      .then((r) => {
        if (host.disposed) throw new DOMException('Viewer destroyed', 'AbortError');
        if (r.status === 304) {
          const cleared = AN.error != null;
          AN.error = null;
          return cleared;
        } // unchanged, and asking works again
        if (r.status === 403) {
          host.liveModelOwner.ended(403);
          return false;
        }
        if (!r.ok)
          throw Object.assign(new Error(r.status + ' ' + r.statusText), { status: r.status });
        const etag = r.headers.get('ETag');
        return r.json().then((value: unknown) => {
          if (host.disposed) return false;
          const data = parseAnalytics(value);
          AN.answers.delete(key);
          AN.answers.set(key, { etag, data });
          while (AN.answers.size > AN_KEEP) AN.answers.delete(AN.answers.keys().next().value!);
          const changed = AN.error != null || kept?.etag !== etag;
          AN.error = null;
          return changed;
        });
      })
      .catch((e) => {
        if (host.disposed) return false;
        const changed = AN.error !== e.message;
        AN.error = e.message;
        AN.failedAt = performance.now();
        return changed;
      });
    AN.inflight = asked.then((changed) => {
      host.scope.releaseRequest(controller);
      AN.inflight = null;
      if (host.disposed || !AN.again) return changed;
      const askedAgain = AN.againAsked;
      AN.again = AN.againAsked = false;
      if (!askedAgain && backingOff()) return changed;
      return fetchAnalytics(askedAgain).then((more) => changed || more);
    });
    return AN.inflight;
  }
  // Asks again in 10 s, or in a little over a second when the answer came from an older model than the page has. While
  // asking fails (a 409 or a 500), it asks 10 s after the last failure: the kept answer stays drawn, with the error above it.
  const backingOff = () => AN.error != null && performance.now() - AN.failedAt < AN_EVERY; // a monotonic clock: a wall-clock jump neither stalls nor rushes it
  function scheduleAnalytics() {
    host.scope.clearTimeout(AN.timer);
    AN.timer = undefined;
    if (
      host.navigation.route.v !== 'analytics' ||
      host.liveModelOwner.LIVE.ended ||
      !host.liveModelOwner.visible()
    )
      return;
    const data = analyticsData(),
      behind =
        !AN.error &&
        data &&
        host.liveModelOwner.LIVE.version &&
        data.version !== host.liveModelOwner.LIVE.version;
    AN.timer = host.scope.timeout(
      () => {
        AN.timer = undefined;
        refreshAnalytics();
      },
      AN.error ? Math.max(0, AN.failedAt + AN_EVERY - performance.now()) : behind ? 1200 : AN_EVERY,
    );
  }
  // `asked`: the reader changed the range or a filter, which asks at once. Anything else (a model update, the tab showing
  // again) waits out the backoff while asking fails, so failed asks keep 10 s apart however fast the model moves.
  function refreshAnalytics(asked = false) {
    if (host.navigation.route.v !== 'analytics') return Promise.resolve();
    if (asked !== true && backingOff()) {
      if (!AN.timer) scheduleAnalytics();
      return Promise.resolve();
    }
    host.scope.clearTimeout(AN.timer);
    AN.timer = undefined;
    return fetchAnalytics(asked === true).then((changed) => {
      if (
        changed &&
        host.navigation.route.v === 'analytics' &&
        host.navigation.rendered === host.navigation.route
      ) {
        if (host.toolViewsOwner.viewerEl || host.accountControlsOwner.accountChrome.open)
          host.liveModelOwner.LIVE.pending = true; // drawn when the sheet or the account menu closes
        else {
          const st = host.viewport.capture();
          host.documentRendererOwner.render();
          host.viewport.restore(st);
        }
      }
      scheduleAnalytics();
    });
  }
  if (!host.sidebarOnly)
    host.scope.listen(document, 'visibilitychange', () => {
      if (host.liveModelOwner.visible() && host.navigation.route.v === 'analytics')
        refreshAnalytics();
      else if (!host.liveModelOwner.visible()) {
        host.scope.clearTimeout(AN.timer);
        AN.timer = undefined;
      }
    });
  const nameOfSid = (A: AnalyticsData, sid: string) =>
    host.modelStore.sessions[sid]?.name ?? A.sessions[sid]?.name ?? sid;
  const harnessOfSid = (A: AnalyticsData, sid: string) =>
    host.modelStore.sessions[sid]?.harness ?? A.sessions[sid]?.harness ?? '';
  const sessionFacetValue = (s: Session, key: FacetKey) =>
    key === 'repo'
      ? (s.repo ?? '__none__')
      : key === 'model'
        ? (s.model ?? s.modelId ?? 'Unknown model')
        : (s[key] ?? '');
  function matchesSessionFacets(s: Session) {
    return (Object.keys(host.sessionFilters) as FacetKey[]).every(
      (key) => !host.sessionFilters[key] || sessionFacetValue(s, key) === host.sessionFilters[key],
    );
  }
  // The four filters (Repo, Machine, Harness, Model) of Analytics and Sessions: one persistent control per page. It is one
  // "Filter" button (with the count of active filters) and one chip per active filter, whose × clears it. The button opens a
  // sheet (a bottom sheet on a phone, a dialog on a wide screen) holding the four Selects, with "Clear all" and "Done". The
  // choices apply when the sheet closes, however it closes. A redraw (`sync`) brings the Selects' option lists, the chips and the
  // count up to date in place. A selected value that no session has now stays selected, marked "(no sessions)", until the reader
  // changes it. The sheet lives inside the control, so its Selects are in the page even while it is shut.
  // On Analytics the range's own values join the model's: a repo that worked last week is a choice there.
  const rangeFacet = (key: FacetKey) =>
    host.navigation.route.v !== 'analytics'
      ? []
      : (analyticsData()?.facets?.[key] ?? []).map((v) => v ?? '__none__');
  const FACETS: [FacetKey, string, string, () => string[], (v: string) => string][] = [
    [
      'repo',
      'Repo',
      'All repos',
      () =>
        [
          ...new Set([
            ...Object.values(host.modelStore.sessions).map((s) => sessionFacetValue(s, 'repo')),
            ...rangeFacet('repo'),
          ]),
        ].sort((a, b) => (a === '__none__' ? 1 : b === '__none__' ? -1 : a.localeCompare(b))),
      (v) => (v === '__none__' ? 'No repo' : v),
    ],
    [
      'machine',
      'Machine',
      'All machines',
      () =>
        [
          ...new Set([
            ...Object.values(host.modelStore.sessions).map((s) => s.machine ?? ''),
            ...rangeFacet('machine'),
          ]),
        ].sort(),
      (v) => host.modelStore.machines[v] ?? v,
    ],
    [
      'harness',
      'Harness',
      'All harnesses',
      () =>
        [
          ...new Set([
            ...Object.values(host.modelStore.sessions).map((s) => s.harness ?? ''),
            ...rangeFacet('harness'),
          ]),
        ].sort(),
      (v) => HARNESS[v] ?? v,
    ],
    [
      'model',
      'Model',
      'All models',
      () =>
        [
          ...new Set([
            ...Object.values(host.modelStore.sessions).map((s) => sessionFacetValue(s, 'model')),
            ...rangeFacet('model'),
          ]),
        ].sort(),
      shortModel,
    ],
  ];
  function renderFacetFilters(box: HTMLElement, onChange: () => void) {
    const s = host.routeControls.slot('facets', box, (ctx) => {
      let before = '';
      const control = createFacetChrome({
        select(label, value, onChange) {
          return createSelect({ label, value, options: [], onChange });
        },
        change(key, value) {
          host.sessionFilters[key as FacetKey] = value;
        },
        cleared(key) {
          host.sessionFilters[key as FacetKey] = '';
          ctx.sync();
          ctx.onChange();
        },
        canOpen() {
          return !host.toolViewsOwner.viewerEl;
        },
        opened(d) {
          ctx.sync();
          before = JSON.stringify(host.sessionFilters);
          host.toolViewsOwner.viewerEl = d;
          try {
            history.pushState(
              {
                ...host.navigation.route,
                sheet: 1,
                scrollTop: host.historyScrollOwner.currentScroll(),
              },
              '',
            );
          } catch {}
        },
        closed(d, reason) {
          if (host.disposed || reason === 'destroyed') return;
          if (host.toolViewsOwner.viewerEl === d) {
            host.toolViewsOwner.viewerEl = null;
            if (history.state?.sheet) {
              host.toolViewsOwner.skipPop = true;
              history.back();
            }
          }
          if (JSON.stringify(host.sessionFilters) !== before) {
            host.liveModelOwner.LIVE.pending = false;
            ctx.onChange();
          } else if (host.liveModelOwner.LIVE.pending) host.applicationRefreshOwner.refresh();
        },
        clear() {
          for (const key of Object.keys(host.sessionFilters) as FacetKey[])
            host.sessionFilters[key as FacetKey] = '';
        },
      });
      ctx.destroy = () => control.destroy();
      ctx.sync = () =>
        control.update(
          FACETS.map(([key, label, allLabel, valuesOf, showValue]) => {
            const current = host.sessionFilters[key],
              values = valuesOf().filter((value) => value !== ''),
              gone = current !== '' && !values.includes(current);
            if (gone) values.push(current);
            return {
              key,
              label,
              value: current,
              display: showValue(current),
              options: [
                { value: '', label: allLabel },
                ...values.map((value) => ({
                  value,
                  label: showValue(value) + (gone && value === current ? ' (no sessions)' : ''),
                })),
              ],
            };
          }),
        );
      return control.element;
    });
    s.ctx.onChange = onChange;
    s.ctx.sync();
    return s.el;
  }
  const hoursText = (ms: number) => (ms / HOUR).toFixed(1) + ' h',
    rangeName = () => (host.analyticsRange === 1 ? '24 h' : host.analyticsRange + ' d');
  function chartWidth() {
    const page = host.$('#page'),
      style = getComputedStyle(page);
    return Math.max(
      280,
      Math.round(page.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)),
    );
  }
  const rangeAgo = (A: AnalyticsData) => (A.days === 1 ? '24 h ago' : A.days + ' d ago');
  function openModelItems(group: AnalyticsModelGroup) {
    const {
      d,
      body,
      show: open,
    } = host.sessionChrome.panel(shortModel(group.model) + ' · ' + group.band, {
      label: 'Work items for ' + group.model + ', ' + group.band,
    });
    renderModelItems(
      body,
      group.n + ' work items' + (group.small_sample ? ' · small sample' : ''),
      (group.items ?? []).map((item) => ({
        id: item.sid,
        name: host.modelStore.sessions[item.sid]?.name,
        description:
          (item.models ?? []).map(shortModel).join(' → ') +
          ' · ' +
          (item.cost_usd == null ? 'cost unknown' : host.domain.asMoney(item.cost_usd)),
        trace: (host.modelStore.turns[item.sid] ?? []).find((t) => t.out.length)?.id,
        url: liveUrl(item.pr_url ?? '') ?? undefined,
      })),
      group.items_more ? group.items_more + ' more items in the selected range' : undefined,
      {
        session(id) {
          host.pendingSessionOpen = id;
          d.close();
        },
        trace(id) {
          host.afterPop = () => host.destination.goTrace(id);
          d.close();
        },
      },
    );
    open();
  }
  // A chart column's sessions, as the server listed them (most first); `more` counts those it left out.
  function openAnalyticsSlice(
    A: AnalyticsData,
    a: number,
    b: number,
    items: AnalyticsSliceItem[],
    more: number,
    costMode: boolean = false,
  ) {
    const when =
        new Date(a).toLocaleString([], { weekday: 'short', hour: 'numeric', minute: '2-digit' }) +
        '–' +
        new Date(b).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }),
      heading = costMode ? 'Sessions with cost' : 'Sessions busy';
    const sheet = createNativeSheet(
      {
        className: 'analytics-slice',
        heading: heading + ' · ' + when,
        label: heading + ' ' + when,
        closeLabel: 'Close sessions list',
      },
      {
        opened(d) {
          host.toolViewsOwner.viewerEl = d;
          document.documentElement.classList.add('viewer-open');
          try {
            history.pushState(
              {
                ...host.navigation.route,
                sheet: 1,
                scrollTop: host.historyScrollOwner.currentScroll(),
              },
              '',
            );
          } catch {}
        },
        closed(d) {
          host.dialogs.delete(d);
          if (host.disposed) return;
          document.documentElement.classList.remove('viewer-open');
          if (host.toolViewsOwner.viewerEl === d) {
            host.toolViewsOwner.viewerEl = null;
            if (history.state?.sheet) {
              host.toolViewsOwner.skipPop = true;
              history.back();
            } else if (host.pendingSessionOpen) {
              const id = host.pendingSessionOpen;
              host.pendingSessionOpen = null;
              host.destination.goSession(id);
            }
          }
        },
      },
    );
    const rows = items.map((item) => {
      const harness = harnessOfSid(A, item.sid);
      return {
        id: item.sid,
        name: nameOfSid(A, item.sid),
        harness,
        harnessName: HARNESS_SHORT[harness] ?? harness,
        harnessTip:
          HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness]
            ? HARNESS[harness]
            : undefined,
        mark: host.screenViews.harnessSnapshot(harness),
        value: costMode
          ? host.domain.asMoney('usd' in item ? (item.usd ?? 0) : 0)
          : timeText('ms' in item ? item.ms : 0) + ' busy',
        href: host.modelStore.sessions[item.sid]
          ? host.bootstrapOwner.urlOf({ v: 'session', id: item.sid })
          : undefined,
        className: 'analytics-session analytics-slice',
        missing:
          costMode && 'unpriced_models' in item && item.unpriced_models.length
            ? 'no price for ' + item.unpriced_models.join(', ')
            : undefined,
      };
    });
    renderSliceBody(
      sheet.body,
      rows,
      more ?? 0,
      costMode ? 'No sessions had a recorded cost then.' : 'No sessions were busy then.',
      (id) => {
        host.pendingSessionOpen = id;
        sheet.dialog.close();
      },
    );
    sheet.cleanup(() => releaseRoot(null, sheet.body));
    host.dialogs.set(sheet.dialog, sheet);
    sheet.show();
  }
  function renderAnalytics(page: HTMLElement) {
    const A = analyticsData(),
      metrics: Metric[] = [],
      charts: Chart[] = [],
      breakdowns: Breakdown[] = [],
      lists: NonNullable<AnalyticsSnapshot['lists']>[number][] = [],
      modelGroups = new Map<string, AnalyticsModelGroup>(),
      slices = new Map<
        string,
        {
          bin: { a: number; b: number; sessions: AnalyticsSliceItem[]; more: number };
          costMode: boolean;
        }
      >();
    const heading =
      'Measured activity · Last ' +
      (host.analyticsRange === 1 ? '24 hours' : host.analyticsRange + ' days');
    let error = AN.error
      ? A
        ? "Couldn't update Analytics: " + AN.error + '. Showing the last answer.'
        : "Couldn't load Analytics: " + AN.error
      : A
        ? undefined
        : 'Loading…';
    const note = (
      value: number,
      previous: number,
      format: (n: number) => string,
    ): Metric['note'] => {
      const delta = value - previous;
      return Math.abs(delta) < 1e-9
        ? { text: 'No change vs previous ' + rangeName() }
        : {
            lead: (delta > 0 ? '+' : '−') + format(Math.abs(delta)),
            tone: delta > 0 ? 'up' : 'down',
            text: ' vs previous ' + rangeName(),
          };
    };
    const metric = (
      label: string,
      value: string,
      delta: Metric['note'],
      explanation?: string,
      info?: string,
    ) =>
      metrics.push({
        id: 'metric-more-analytics-' + metrics.length,
        label,
        value,
        note: delta,
        explanation,
        info,
      });
    const widthOf = (measure: number, max: number) =>
      Math.max(measure ? 2 : 0, (measure / max) * 100);
    const makeChart = (costMode: boolean): Chart => {
      if (!A) throw new Error('Analytics model unavailable');
      const key = costMode ? 'cost' : 'agents',
        W = chartWidth(),
        left = costMode ? 46 : 40,
        right = W - 4;
      if (costMode && !A.cost)
        return {
          key,
          heading: 'Cost over time',
          info: host.recentNavigation.COST_TIP,
          empty:
            'Cost is recorded per UTC day, so there is no hourly series. Pick 7 d or 30 d for a daily chart.',
          width: W,
          left,
          right,
          grid: [],
          bins: [],
          legend: [],
          role: 'img',
          label: '',
          ago: rangeAgo(A),
        };
      const raw = costMode
        ? A.cost!.days.map((d) => ({
            a: d.from,
            b: d.to,
            claude: d.claude_usd,
            codex: d.codex_usd,
            sessions: d.sessions,
            more: d.more,
          }))
        : A.agents.columns.map((c) => ({
            a: c.from,
            b: c.to,
            claude: c.claude_ms / HOUR,
            codex: c.codex_ms / HOUR,
            sessions: c.sessions,
            more: c.more,
          }));
      const most = Math.max(0, ...raw.map((b) => b.claude + b.codex)),
        stepY = niceStep(most || 1),
        max = costMode ? Math.max(0.01, most) : Math.max(stepY, Math.ceil(most / stepY) * stepY);
      const grid = [];
      if (costMode)
        for (let i = 0; i <= 2; i++)
          grid.push({ y: 151 - (139 * i) / 2, label: '$' + ((max * i) / 2).toFixed(2) });
      else
        for (let n = 0; n <= max + 1e-9; n += stepY)
          grid.push({ y: 151 - (139 * n) / max, label: hLabel(n) });
      const step = (right - left) / Math.max(1, raw.length),
        barWidth = Math.max(2, step * 0.64);
      const bins = raw.map((bin, i) => {
        const id = key + ':' + i,
          total = bin.claude + bin.codex,
          when = host.clock(bin.a) + '–' + host.clock(bin.b),
          tip = when + ': ' + hLabel(total);
        slices.set(id, { bin, costMode });
        return {
          key: id,
          x: left + i * step,
          width: step,
          barX: left + i * step + (step - barWidth) / 2,
          barWidth,
          claude: (139 * bin.claude) / max,
          codex: (139 * bin.codex) / max,
          active: costMode ? !!bin.sessions.length : total > 0,
          tip: costMode ? undefined : tip,
          label: costMode
            ? host.clock(bin.a) + ' to ' + host.clock(bin.b) + ': ' + host.domain.asMoney(total)
            : tip + '. Open the sessions busy then',
          when,
          value: costMode ? host.domain.asMoney(total) : hLabel(total) + ' agent-hours',
        };
      });
      return {
        key,
        heading: costMode ? 'Cost over time' : 'Agents at work',
        info: costMode ? host.recentNavigation.COST_TIP : undefined,
        sub: costMode
          ? 'API-equivalent cost per UTC day · today so far · stacked by harness'
          : 'Agent-hours ' + A.agents.unit + ' · stacked by harness',
        width: W,
        left,
        right,
        grid,
        bins,
        label: costMode
          ? 'API-equivalent cost per day, stacked by harness'
          : 'Agent-hours ' + A.agents.unit + ' over the selected range, stacked by harness',
        role: costMode ? 'img' : 'group',
        ago: rangeAgo(A),
        legend: [
          ['claude', 'Claude'],
          ['codex', 'Codex'],
        ].map(([id, label]) => ({ id, label, mark: host.screenViews.harnessSnapshot(id) })),
        missing:
          costMode && A.cost!.unpriced_models.length
            ? 'no price for ' +
              A.cost!.unpriced_models.join(', ') +
              '; unpriced usage is omitted from bars.'
            : undefined,
      };
    };
    let models: ModelBand[] | undefined, allowance: AnalyticsSnapshot['allowance'];
    const measures: [
      string,
      (
        | 'first_pass_acceptance'
        | 'median_review_rounds'
        | 'median_red_ci_heads'
        | 'median_model_ms'
        | 'median_cost_usd'
        | 'allowance_per_million_input'
      ),
      'acceptance_n' | 'review_rounds_n' | 'red_ci_n' | 'model_time_n' | 'cost_n' | 'allowance_n',
      string,
      (n: number) => string,
    ][] = [
      [
        'Acceptance',
        'first_pass_acceptance',
        'acceptance_n',
        'acceptance',
        (n) => (n * 100).toFixed(0) + '%',
      ],
      ['Review rounds', 'median_review_rounds', 'review_rounds_n', 'review_rounds', String],
      ['Red CI heads', 'median_red_ci_heads', 'red_ci_n', 'ci', String],
      ['Model time', 'median_model_ms', 'model_time_n', 'model_time', timeText],
      [
        'API cost',
        'median_cost_usd',
        'cost_n',
        'cost',
        (...args: Parameters<typeof host.domain.asMoney>) => host.domain.asMoney(...args),
      ],
      [
        'Allowance / M input',
        'allowance_per_million_input',
        'allowance_n',
        'allowance',
        (n) => n.toFixed(2) + '%',
      ],
    ];
    if (A) {
      const now = A.current,
        previous = A.previous,
        pct = (errors: number, tools: number) =>
          tools ? Math.round((errors / tools) * 100) + '%' : '0%';
      metric(
        'Agent-hours',
        hoursText(now.agent_ms),
        note(now.agent_ms, previous.agent_ms, hoursText),
        'Busy time summed across sessions; two sessions busy for an hour count two hours.',
      );
      metric(
        A.days === 1 ? 'Cost today (UTC)' : 'Cost, last ' + A.days + ' UTC days',
        now.cost.usd == null ? '—' : host.domain.asMoney(now.cost.usd),
        now.cost.usd == null || previous.cost.usd == null
          ? {
              text:
                'no price for ' +
                [...new Set([...now.cost.unpriced_models, ...previous.cost.unpriced_models])].join(
                  ', ',
                ),
            }
          : note(
              now.cost.usd,
              previous.cost.usd,
              (...args: Parameters<typeof host.domain.asMoney>) => host.domain.asMoney(...args),
            ),
        A.days === 1
          ? 'API-equivalent cost. Cost is recorded per UTC day: this is the whole current UTC day so far, compared with the whole day before.'
          : 'API-equivalent cost. Cost is recorded per UTC day: the last ' +
              A.days +
              ' UTC days count, today so far, compared with the ' +
              A.days +
              ' whole UTC days before.',
        host.recentNavigation.COST_TIP,
      );
      metric(
        'Sessions started',
        countText(now.started),
        note(now.started, previous.started, countText),
      );
      metric('Turns', countText(now.turns), note(now.turns, previous.turns, countText));
      const toolNote = note(now.tools, previous.tools, countText);
      if (A.calls_unknown)
        toolNote.tail =
          ' · — for ' + A.calls_unknown + (A.calls_unknown === 1 ? ' session' : ' sessions');
      metric(
        'Tool calls',
        countText(now.tools),
        toolNote,
        countText(now.errors) +
          ' failed (' +
          pct(now.errors, now.tools) +
          ') · previous ' +
          rangeName() +
          ': ' +
          countText(previous.errors) +
          ' failed (' +
          pct(previous.errors, previous.tools) +
          ')',
      );
      metric(
        'Peak concurrency',
        countText(now.peak),
        note(now.peak, previous.peak, countText),
        'The most sessions busy at the same moment.',
      );
      metric(
        'Waited on you',
        timeText(now.wait_ms),
        note(now.wait_ms, previous.wait_ms, timeText),
        'Median wait ' +
          timeText(now.median_wait_ms) +
          ' · previous ' +
          rangeName() +
          ': ' +
          timeText(previous.median_wait_ms),
      );
      const wait = A.longest_current_wait;
      metric(
        'Longest current wait',
        wait ? timeText(wait.ms) : '—',
        note(wait ? wait.ms : 0, previous.longest_wait_ms, timeText),
        wait
          ? nameOfSid(A, wait.sid) + ' has waited on you for ' + timeText(wait.ms)
          : 'No session is waiting on you',
      );
      charts.push(makeChart(false), makeChart(true));
      for (const [key, title, groups] of [
        ['repo', 'By repo', A.breakdown.repo],
        ['machine', 'By machine', A.breakdown.machine],
        ['harness', 'By harness and model', A.breakdown.model],
      ] satisfies [string, string, AnalyticsData['breakdown']['repo']][]) {
        const keyFor = (g: AnalyticsData['breakdown']['repo'][number]) =>
          key === 'repo'
            ? (g.repo ?? '__none__')
            : key === 'machine'
              ? (g.machine ?? '')
              : g.harness + '\u0000' + g.model;
        const labelFor = (id: string) =>
          key === 'repo'
            ? id === '__none__'
              ? 'No repo (roles)'
              : id
            : key === 'machine'
              ? (host.modelStore.machines[id] ?? id)
              : (HARNESS[id.split('\u0000')[0]] ?? id.split('\u0000')[0]) +
                ' · ' +
                shortModel(id.split('\u0000')[1]);
        const selected = (g: AnalyticsData['breakdown']['repo'][number]) =>
            host.analyticsMeasure === 'cost' ? g.usd : g.ms,
          rows = [...groups].sort(
            (a, b) =>
              selected(b) - selected(a) || labelFor(keyFor(a)).localeCompare(labelFor(keyFor(b))),
          ),
          max = Math.max(1, ...rows.map(selected));
        breakdowns.push({
          key,
          heading: title,
          rows: rows.map((g) => ({
            key: keyFor(g),
            name: labelFor(keyFor(g)),
            count: g.sessions + (g.sessions === 1 ? ' session' : ' sessions'),
            width: widthOf(selected(g), max),
            color:
              key === 'harness' ? (keyFor(g).startsWith('claude') ? 'claude' : 'codex') : undefined,
            hours: hoursText(g.ms),
            cost: g.unpriced_models.length ? '—' : host.domain.asMoney(g.usd),
            missing: g.unpriced_models.length
              ? 'no price for ' + g.unpriced_models.join(', ')
              : undefined,
          })),
        });
      }
      const topLists: [
        string,
        AnalyticsSliceItem[],
        (x: AnalyticsSliceItem) => string,
        (x: AnalyticsSliceItem) => number,
      ][] = [
        [
          'Top sessions · busy time',
          A.top.busy,
          (x) => timeText('ms' in x ? x.ms : 0),
          (x) => ('ms' in x ? x.ms : 0),
        ],
        [
          'Top sessions · waited on',
          A.top.waited,
          (x) => timeText('ms' in x ? x.ms : 0),
          (x) => ('ms' in x ? x.ms : 0),
        ],
        [
          'Most expensive sessions · API-equivalent cost',
          A.top.cost,
          (x) => ('usd' in x && x.usd != null ? host.domain.asMoney(x.usd) : '—'),
          (x) => ('usd' in x ? (x.usd ?? 0) : 0),
        ],
      ];
      for (const [title, items, value, measure] of topLists) {
        const max = Math.max(1, ...items.map(measure));
        lists.push({
          heading: title,
          rows: items.map((item) => {
            const harness = harnessOfSid(A, item.sid),
              missing = 'unpriced_models' in item ? item.unpriced_models : [];
            return {
              id: item.sid,
              name: nameOfSid(A, item.sid),
              harness,
              harnessName: HARNESS_SHORT[harness] ?? harness,
              harnessTip:
                HARNESS[harness] && HARNESS[harness] !== HARNESS_SHORT[harness]
                  ? HARNESS[harness]
                  : undefined,
              mark: host.screenViews.harnessSnapshot(harness),
              value: value(item),
              href: host.modelStore.sessions[item.sid]
                ? host.bootstrapOwner.urlOf({ v: 'session', id: item.sid })
                : undefined,
              rank: widthOf(measure(item), max),
              missing: missing.length ? 'no price for ' + missing.join(', ') : undefined,
            };
          }),
        });
      }
      if (A.models) {
        const reasons = A.models.unknown_reasons ?? {};
        models = [];
        for (const band of ['easy', 'medium', 'hard', 'unknown']) {
          const rows = (A.models.groups ?? []).filter((g) => g.band === band);
          if (!rows.length) continue;
          const points = rows.filter(
              (g) => g.first_pass_acceptance != null && g.median_cost_usd != null,
            ),
            W = Math.max(280, chartWidth()),
            maxCost = Math.max(0.01, ...points.map((g) => g.median_cost_usd ?? 0));
          models.push({
            key: band,
            heading:
              band === 'unknown' ? 'Unknown difficulty' : band[0].toUpperCase() + band.slice(1),
            width: W,
            rows: rows.map((g) => {
              const key = band + '\u0000' + g.model;
              modelGroups.set(key, g);
              const tokens = g.tokens ?? {};
              return {
                key,
                name: shortModel(g.model),
                tip: g.model,
                count: g.n + (g.small_sample ? ' · small sample' : ''),
                cells: [
                  ...measures.map(([, valueKey, nKey, reason, format]) => ({
                    text: g[valueKey] == null ? 'Unknown' : format(g[valueKey]!),
                    n: g[nKey] ?? 0,
                    tip: g[valueKey] == null ? reasons[reason] : undefined,
                  })),
                  {
                    text: g.tokens_n
                      ? [
                          tokens.input ?? 0,
                          tokens.output ?? 0,
                          (tokens.cache_read ?? 0) + (tokens.cache_write ?? 0),
                        ]
                          .map(countText)
                          .join(' / ')
                      : 'Unknown',
                    n: g.tokens_n ?? 0,
                  },
                ],
              };
            }),
            points: points.map((g) => ({
              x: 42 + (g.median_cost_usd! / maxCost) * (W - 58),
              y: 150 - g.first_pass_acceptance! * 130,
              label: shortModel(g.model),
              tip:
                g.model +
                ': ' +
                host.domain.asMoney(g.median_cost_usd ?? 0) +
                ', ' +
                (g.first_pass_acceptance! * 100).toFixed(0) +
                '% accepted; acceptance n=' +
                g.acceptance_n +
                ', cost n=' +
                g.cost_n,
            })),
          });
        }
      }
      const limits = A.allowance;
      if (limits?.recorded_at != null && limits.windows?.length)
        allowance = {
          when: new Date(limits.recorded_at).toLocaleString([], {
            hour: 'numeric',
            minute: '2-digit',
          }),
          windows: limits.windows.map((limit) => ({
            label:
              limit.minutes === 300
                ? '5-hour window'
                : limit.minutes === 10080
                  ? 'Weekly window'
                  : limit.minutes + '-minute window',
            used: limit.used_percent + '% used',
            reset:
              'Resets ' +
              new Date(limit.resets_at).toLocaleString([], {
                weekday: 'short',
                hour: 'numeric',
                minute: '2-digit',
              }),
          })),
        };
    }
    renderAnalyticsScreen(
      page,
      {
        heading,
        error,
        ready: !!A,
        query: analyticsQuery(),
        metrics,
        charts,
        breakdowns,
        lists,
        models,
        measure: host.analyticsMeasure,
        allowance,
        modelHeaders: [
          'Model',
          'Work items',
          ...measures.map((m) => m[0]),
          'Tokens · input / output / cache',
        ],
      },
      {
        committed: (...args: Parameters<typeof host.sessionChrome.observeTitle>) =>
          host.sessionChrome.observeTitle(...args),
        facets: () =>
          renderFacetFilters(page, () => {
            host.documentRendererOwner.render();
            refreshAnalytics(true);
          }),
        session: (...args: Parameters<typeof host.destination.goSession>) =>
          host.destination.goSession(...args),
        measure(value) {
          if (host.analyticsMeasure === value) return;
          const top = host.historyScrollOwner.currentScroll();
          host.analyticsMeasure = value;
          host.documentRendererOwner.render();
          host.historyScrollOwner.restoreScroll(top);
        },
        breakdown(group, key) {
          if (group === 'repo') host.sessionFilters.repo = key;
          else if (group === 'machine') host.sessionFilters.machine = key;
          else {
            const [harness, model] = key.split('\u0000');
            host.sessionFilters.harness = harness ?? '';
            host.sessionFilters.model = model ?? '';
          }
          host.query = '';
          host.groupBy = 'recent';
          host.destination.go({ v: 'sessions' });
        },
        slice(key) {
          const slice = slices.get(key);
          if (!slice || !A) return;
          const { bin, costMode } = slice;
          openAnalyticsSlice(A, bin.a, bin.b, bin.sessions, bin.more, costMode);
        },
        model(key) {
          const model = modelGroups.get(key);
          if (model) openModelItems(model);
        },
      },
    );
  }

  return {
    fetchAnalytics,
    scheduleAnalytics,
    refreshAnalytics,
    renderAnalytics,
    matchesSessionFacets,
    renderFacetFilters,
  };
}

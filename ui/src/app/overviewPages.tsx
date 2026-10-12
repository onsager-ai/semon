import { render } from 'preact';
import { orderRows, type OrderScope } from '../lib/ordering';
import type { RecentItem, RecentSnapshot } from '../lib/recent';
import type { CatalogSession } from '../state/catalog-wire';
import { array, object, text, number, boolean, dictionary } from '../domain/validate';
import {
  renderHomeScreen,
  renderMachinesScreen,
  renderMachineScreen,
  type InboxRow,
  type HarnessMark,
  renderSliceBody,
  renderModelItems,
  createPanelChrome,
  createFacetChrome,
  createSelect,
  createKidsSheet,
} from '../lib';
import { parseAnalytics, type AnalyticsData } from '../state/analytics-wire';
import { renderAnalyticsPresentation } from './analyticsPresentation';
import { createSeenResults } from './seenResults';
import { HARNESSES, I } from './registry';
import { runningToolLabel } from '../domain/toolNames';
import { ago, timeText, shortModel, liveUrl, previewLine } from '../domain/format';
import type { EffectScope } from './effects';

interface Selection {
  source_key: string;
  catalog_key: string;
}
interface HomeRow extends Selection {
  id: string;
  session: {
    name: string;
    kind?: string;
    harness: string;
    model: string;
    repo: string | null;
    state: string;
    last: number;
    start: number;
    waiting_for: string | null;
    waiting_since: number | null;
    cost: number | null;
    detail?: string;
    parent?: string;
    branch?: string;
    activity?: readonly [string, string, number];
  };
  contentGeneration: string | null;
  previewReady?: boolean;
  event?: {
    id: string;
    at: number;
    done: number | null;
    ask: string | null;
    status: string;
    brief: string;
    answer: string[] | null;
    declined: boolean;
    origin?: string;
    turn?: string;
  };
}
interface HomeData {
  version: string;
  now: number;
  next: { inbox: number | null; working: number | null; answered: number | null };
  totals: { waiting: number; working: number; answered: number };
  machines: {
    key: string;
    label: string;
    up: boolean;
    working: number;
    sessions: number;
    last: number | null;
  }[];
  recent: HomeRow[];
  inbox: HomeRow[];
  working: HomeRow[];
  answered: HomeRow[];
}
const nullable = <T,>(value: unknown, parse: (value: unknown) => T): T | null =>
  value == null ? null : parse(value);
function parseSelection(value: unknown): Selection {
  const row = object(value);
  return { source_key: text(row.source_key), catalog_key: text(row.catalog_key) };
}
function parseActivity(value: unknown, now: number): readonly [string, string, number] | undefined {
  if (value == null) return undefined;
  const parts = value as unknown[];
  const age = number(parts[2]);
  return [
    text(parts[0]),
    text(parts[1]),
    parts[3] == null ? age : Math.max(0, Math.floor((now - number(parts[3])) / 1000)),
  ];
}
function parseHomeRow(value: unknown, now: number): HomeRow {
  const row = object(value),
    session = object(row.session),
    event = row.event == null ? null : object(row.event);
  return {
    ...parseSelection(row),
    id: text(row.id),
    contentGeneration: nullable(session.content_generation, text),
    session: {
      name: text(session.name),
      kind: session.kind == null ? undefined : text(session.kind),
      harness: text(session.harness),
      model: text(session.model),
      repo: nullable(session.repo, text),
      state: text(session.state),
      last: number(session.last),
      start: number(session.start),
      waiting_for: nullable(session.waiting_for, text),
      waiting_since: nullable(session.waiting_since, number),
      cost: nullable(session.cost, number),
      parent: nullable(session.parent, text) ?? undefined,
      branch: nullable(session.branch, text) ?? undefined,
      detail: nullable(session.detail, text) ?? undefined,
      activity: parseActivity(session.activity, now),
    },
    event: event
      ? {
          id: text(event.id),
          at: number(event.at),
          done: nullable(event.done, number),
          ask: nullable(event.ask, text),
          status: text(event.status),
          brief: text(event.brief),
          answer: nullable(event.answer, (value) => array(value, text)),
          declined: boolean(event.declined),
          origin: nullable(event.origin, text) ?? undefined,
          turn: nullable(event.turn, text) ?? undefined,
        }
      : undefined,
  };
}
function parseHome(value: unknown): HomeData {
  const data = object(value),
    totals = object(data.totals);
  if (data.api !== 1) throw Error('Unsupported Home response');
  for (const key of ['inbox', 'working', 'answered', 'recent']) {
    if (
      !Array.isArray(data[key]) ||
      (data[key] as unknown[]).length > (key === 'recent' ? 60 : 100)
    )
      throw Error('Home response exceeds its bound');
  }
  return {
    version: text(data.version),
    now: number(data.now),
    next: {
      inbox: nullable(object(data.next).inbox, number),
      working: nullable(object(data.next).working, number),
      answered: nullable(object(data.next).answered, number),
    },
    totals: {
      waiting: number(totals.waiting),
      working: number(totals.working),
      answered: number(totals.answered),
    },
    machines: array(data.machines, (value) => {
      const row = object(value);
      return {
        key: text(row.key),
        label: text(row.label),
        up: boolean(row.up),
        working: number(row.working),
        sessions: number(row.sessions),
        last: nullable(row.last, number),
      };
    }),
    recent: array(data.recent, (row) => parseHomeRow(row, number(data.now))),
    inbox: array(data.inbox, (row) => parseHomeRow(row, number(data.now))),
    working: array(data.working, (row) => parseHomeRow(row, number(data.now))),
    answered: array(data.answered, (row) => parseHomeRow(row, number(data.now))),
  };
}
interface OverviewHost {
  root: HTMLElement;
  effects: EffectScope;
  orderScope(name: string, signature?: string): OrderScope<object>;
  reviews(): boolean;
  homeIntro?: HTMLElement;
  nativeContent?: boolean;
  api(path: string, body?: unknown): Promise<unknown>;
  route(): string;
  selectedMachine(): string | null;
  admin(): { label: string; href: string } | null;
  machine(key: string): void;
  chrome(): void;
  panelOpened(panel: { destroy(): void }): void;
  panelClosed(panel: { destroy(): void }): void;
  session(selection: Selection, turn?: string): void;
  trace(selection: Selection, turn: string): void;
  sessions(filters: { harness: string; repo: string; machine: string; model: string }): void;
}
/** Home/Analytics own only their accepted compact response, filters and panels.
 * The canonical reader owns navigation, transport, refresh and shell lifetime. */
export function createOverviewPages(host: OverviewHost) {
  let retryTimer: number | undefined,
    retryDelay = 500;
  const cancelRetry = () => {
    host.effects.clearTimeout(retryTimer);
    retryTimer = undefined;
  };
  let disposed = false,
    epoch = 0,
    loading = false,
    queued = false,
    queuedForce = false,
    error: string | null = null,
    home: HomeData | null = null,
    homeSource: string | null = null,
    analytics: AnalyticsData | null = null,
    machineLabels: Record<string, string> = {},
    selections: Record<string, Selection> = {},
    range = 7,
    measure = 'hours',
    allAnswered = false;
  const filters = { repo: '', machine: '', harness: '', model: '' };
  interface AcceptedAnalytics {
    data: AnalyticsData;
    selections: Record<string, Selection>;
    labels: Record<string, string>;
    recent: HomeRow[];
    now: number;
  }
  const analyticsCache = new Map<string, AcceptedAnalytics>();
  let analyticsRetryAt = 0;
  let observationAfter: Selection | null = null;
  function acceptAnalytics(value: AcceptedAnalytics | undefined) {
    analytics = value?.data ?? null;
    if (!value) return;
    selections = value.selections;
    machineLabels = value.labels;
    recentRows = value.recent;
    recentNow = value.now;
  }
  let recentRows: HomeRow[] = [],
    recentNow = 0;
  const recentOpen = new Map<string, boolean>();
  const recentAll = new Set<string>();
  try {
    const preferences = object(JSON.parse(localStorage.getItem('semon.tree') ?? '{}'));
    for (const [id, value] of Object.entries(preferences).slice(-200)) {
      if (value && typeof value === 'object' && 'open' in value && typeof value.open === 'boolean')
        recentOpen.set(id, value.open);
    }
  } catch {}
  function recentSnapshot(
    source?: string,
    catalog: readonly CatalogSession[] = [],
    current?: string,
    rail = false,
    selectedMetadata?: CatalogSession | null,
  ): RecentSnapshot {
    const borrowed = source ? recentRows.filter((row) => row.source_key === source) : recentRows;
    const rows: HomeRow[] = borrowed.length
      ? borrowed
          .map((row) => {
            const observed = catalog.find((item) => item.key === row.catalog_key);
            return observed
              ? {
                  ...row,
                  session: {
                    ...row.session,
                    name: observed.name,
                    kind: observed.presentation_kind ?? row.session.kind,
                    last: observed.last ?? row.session.last,
                    parent: observed.parent ?? undefined,
                    model: observed.model,
                    repo: observed.repo,
                  },
                }
              : row;
          })
          .concat(
            catalog
              .filter((item) => !borrowed.some((row) => row.catalog_key === item.key))
              .slice(0, 60)
              .map((row) => ({
                id: JSON.stringify([source, row.key]),
                source_key: source!,
                catalog_key: row.key,
                contentGeneration: null,
                session: {
                  name: row.name,
                  kind: row.presentation_kind ?? undefined,
                  harness: row.harness,
                  model: row.model,
                  repo: row.repo,
                  state: 'off',
                  start: row.start ?? 0,
                  last: row.last ?? 0,
                  parent: row.parent ?? undefined,
                  cost: row.cost.usd,
                  waiting_for: null,
                  waiting_since: null,
                },
              })),
          )
          .sort((a, b) => b.session.last - a.session.last)
          .slice(0, 60)
      : catalog.slice(0, 60).map((row) => ({
          id: JSON.stringify([source, row.key]),
          source_key: source!,
          catalog_key: row.key,
          contentGeneration: null,
          session: {
            name: row.name,
            kind: row.presentation_kind ?? undefined,
            harness: row.harness,
            model: row.model,
            repo: row.repo,
            state: row.details?.state ?? 'off',
            start: row.start ?? 0,
            last: row.last ?? 0,
            parent: row.parent ?? undefined,
            branch: row.branch ?? undefined,
            cost: row.cost.usd,
            waiting_for: null,
            waiting_since: null,
          },
        }));
    if (source && selectedMetadata && selectedMetadata.key === current) {
      for (const relative of [
        ...(selectedMetadata.details?.family?.ancestors ?? []),
        ...(selectedMetadata.details?.family?.runs ?? []),
        {
          key: selectedMetadata.key,
          name: selectedMetadata.name,
          harness: selectedMetadata.harness,
          kind: selectedMetadata.presentation_kind ?? selectedMetadata.kind,
          parent: selectedMetadata.parent,
          cost: selectedMetadata.cost,
        },
      ]) {
        if (rows.some((row) => row.catalog_key === relative.key)) continue;
        rows.push({
          id: JSON.stringify([source, relative.key]),
          source_key: source,
          catalog_key: relative.key,
          contentGeneration: null,
          session: {
            name: relative.name,
            kind: relative.kind,
            harness: relative.harness,
            parent: relative.parent ?? undefined,
            model: '',
            repo: null,
            state: 'off',
            start: 0,
            last: 0,
            cost: relative.cost.usd,
            waiting_for: null,
            waiting_since: null,
          },
        });
      }
    }
    const byId = new Map(rows.map((row) => [row.id, row]));
    const canonical = new Map<string, HomeRow[]>();
    const idOf = (row: HomeRow) => JSON.stringify([row.source_key, row.session.parent]);
    for (const row of rows)
      if (row.session.parent && byId.has(idOf(row))) {
        const parent = idOf(row);
        canonical.set(parent, [...(canonical.get(parent) ?? []), row]);
      }
    const path = new Set<string>();
    let selected = rows.find((row) => row.source_key === source && row.catalog_key === current);
    while (selected && !path.has(selected.id)) {
      path.add(selected.id);
      selected = byId.get(idOf(selected));
    }
    const children = new Map<string, HomeRow[]>(),
      roots: HomeRow[] = [],
      reachable = new Set<string>();
    function visit(row: HomeRow, parent?: string) {
      if (reachable.has(row.id)) return;
      reachable.add(row.id);
      let nearest = parent;
      if (host.reviews() || row.session.kind !== 'Approval review' || path.has(row.id)) {
        if (parent) children.set(parent, [...(children.get(parent) ?? []), row]);
        else roots.push(row);
        nearest = row.id;
      }
      for (const child of canonical.get(row.id) ?? []) visit(child, nearest);
    }
    for (const row of rows.filter((row) => !row.session.parent || !byId.has(idOf(row)))) visit(row);
    for (const row of rows) if (!reachable.has(row.id)) visit(row);
    const order = host.orderScope('recent', JSON.stringify([source, host.reviews(), rail]));
    const visibleParents = new Set(
      [...document.querySelectorAll<HTMLElement>('#lanes .treeitem')]
        .filter((node) => node.querySelector(':scope > .tree-row .srow')?.getClientRects().length)
        .map((node) => node.dataset.id),
    );
    const keys = new Set(children.keys());
    if (order.keep)
      for (const [parent, kids] of children)
        if (!order.prevKids.has(parent) && visibleParents.has(parent) && !path.has(parent)) {
          order.n += kids.length;
          children.delete(parent);
          keys.delete(parent);
        }
    order.kids = keys;
    function item(row: HomeRow, depth: number, visited: Set<string>): RecentItem {
      const next = new Set(visited).add(row.id);
      const rank = (candidate: HomeRow, seen = new Set<string>()): number => {
        if (seen.has(candidate.id)) return 2;
        seen.add(candidate.id);
        return Math.min(
          candidate.session.state === 'wait' ? 0 : candidate.session.state === 'work' ? 1 : 2,
          ...(children.get(candidate.id) ?? []).map((child) => rank(child, seen)),
        );
      };
      const candidates = (children.get(row.id) ?? []).filter((child) => !next.has(child.id));
      const kids = [...candidates].sort(
        (a, b) => rank(a) - rank(b) || b.session.last - a.session.last,
      );
      const visible = recentAll.has(row.id)
        ? kids
        : kids.filter((child, index) => index < 3 || (index < 8 && rank(child) < 2));
      const open =
        recentOpen.get(row.id) ??
        (home?.machines.length === 1 ? recentOpen.get(row.catalog_key) : undefined) ??
        (path.has(row.id) || kids.some((child) => rank(child) < 2));
      const shown = orderRows(
        order,
        (recentAll.has(row.id) ? 'all:' : 'kids:') + row.id,
        visible,
        (a, b) => rank(a) - rank(b) || b.session.last - a.session.last,
        { must: path, quiet: !open, seed: recentAll.has(row.id) || !visibleParents.has(row.id) },
      );
      return {
        id: row.id,
        name: row.session.name,
        label:
          row.session.name + ', ' + (HARNESSES[row.session.harness]?.name ?? row.session.harness),
        current: current === row.catalog_key ? 'page' : undefined,
        state: row.session.state,
        stateLabel:
          row.session.state === 'work'
            ? 'Working'
            : row.session.state === 'wait'
              ? 'Waiting on you'
              : 'Recorded history',
        age: timeText(Math.max(0, recentNow - row.session.last)),
        model: shortModel(row.session.model),
        modelTip: row.session.model,
        harness: mark(row.session.harness),
        fields: row.session.repo
          ? [
              {
                className: 'repo',
                text: row.session.repo.split('/').pop() ?? row.session.repo,
                priority: 1,
                tip: row.session.repo,
                icon: I.repo,
              },
            ]
          : [],
        rail,
        open,
        depth,
        stuck: recentAll.has(row.id),
        flag: (() => {
          const below = (id: string, seen = new Set<string>()): HomeRow[] => {
            if (seen.has(id)) return [];
            seen.add(id);
            return (children.get(id) ?? []).flatMap((child) => [child, ...below(child.id, seen)]);
          };
          const descendants = below(row.id),
            waiting = descendants.filter((child) => child.session.state === 'wait').length,
            working = descendants.filter((child) => child.session.state === 'work').length;
          return waiting
            ? { state: 'wait', tip: waiting + ' needs you' }
            : working
              ? { state: 'work', tip: working + ' working' }
              : undefined;
        })(),
        childState: kids.some((child) => rank(child) === 0)
          ? 'wait'
          : kids.some((child) => rank(child) === 1)
            ? 'work'
            : undefined,
        children: kids.length ? shown.map((child) => item(child, depth + 1, next)) : undefined,
        all: shown.length < kids.length ? kids.length : undefined,
      };
    }
    const orderedRoots = orderRows(
      order,
      'roots',
      roots,
      (a, b) => b.session.last - a.session.last || a.id.localeCompare(b.id),
      { limit: 8, must: path },
    );
    const visibleRoots = orderedRoots.slice(0, 8);
    for (const row of orderedRoots.filter((row) => path.has(row.id))) {
      if (visibleRoots.includes(row)) continue;
      let last = visibleRoots.length - 1;
      while (last >= 0 && path.has(visibleRoots[last]!.id)) last--;
      if (last >= 0) visibleRoots.splice(last, 1, row);
    }
    return {
      empty: !rows.length,
      items: visibleRoots.map((row) => item(row, 0, new Set())),
    };
  }
  let beforeFilters = '';
  let facetPanel: { destroy(): void } | null = null;
  const facetOwner = createFacetChrome({
    select(label, value, onChange) {
      return createSelect({ label, value, options: [], onChange });
    },
    change(key, value) {
      filters[key as keyof typeof filters] = value;
    },
    cleared(key) {
      filters[key as keyof typeof filters] = '';
      analyticsChanged();
    },
    clear() {
      for (const key of Object.keys(filters) as (keyof typeof filters)[]) filters[key] = '';
      draw();
    },
    canOpen() {
      return host.route() === 'analytics' && !disposed;
    },
    opened(dialog) {
      beforeFilters = JSON.stringify(filters);
      facetPanel = { destroy: () => dialog.close() };
      host.panelOpened(facetPanel);
    },
    closed(_dialog, reason) {
      if (facetPanel) host.panelClosed(facetPanel);
      facetPanel = null;
      if (reason === 'dismissed' && beforeFilters !== JSON.stringify(filters)) analyticsChanged();
    },
  });
  const facets = facetOwner.element;
  const actions = document.createElement('div');
  actions.className = 'analytics-range';
  actions.setAttribute('role', 'group');
  actions.setAttribute('aria-label', 'Analytics range');
  const dialogs = new Set<{ destroy(): void }>();
  const seen = createSeenResults({});
  const money = (n: number) => '$' + n.toFixed(2);
  const mark = (id: string): HarnessMark | undefined => {
    const harness = HARNESSES[id];
    return harness
      ? {
          id,
          name: harness.name,
          light: harness.icon.light,
          dark: harness.icon.dark,
          darkTheme: matchMedia('(prefers-color-scheme: dark)').matches,
        }
      : undefined;
  };
  const query = () => {
    const value = new URLSearchParams({ range: range === 1 ? '24h' : range + 'd' });
    for (const key of ['repo', 'machine', 'harness', 'model'] as const)
      if (filters[key]) value.set(key, filters[key] === '__none__' ? '' : filters[key]);
    return value.toString();
  };
  function analyticsChanged() {
    acceptAnalytics(analyticsCache.get(query()));
    error = null;
    draw();
    void refresh(true);
  }
  function open(selection: Selection | undefined, turn?: string) {
    if (!disposed && selection) host.session(selection, turn);
  }
  function panel(title: string, className?: string) {
    const value = createPanelChrome(
      { title, className },
      {
        opened() {
          host.panelOpened(value);
        },
        closed() {
          host.panelClosed(value);
          dialogs.delete(value);
          render(null, value.body);
        },
      },
    );
    dialogs.add(value);
    return value;
  }
  function drawHome() {
    if (!home) return;
    if (!home.machines.length && host.homeIntro) {
      if (host.homeIntro.parentElement !== host.root) {
        render(null, host.root);
        host.root.replaceChildren(host.homeIntro);
      }
      return;
    }
    const inbox = home.inbox.filter(
      (row) =>
        row.event?.ask !== 'result' ||
        (!(home!.machines.length === 1 && seen.SEEN_RESULTS.has(row.event.id)) &&
          !seen.SEEN_RESULTS.has(JSON.stringify([row.source_key, row.event.id]))),
    );
    const byId = new Map<string, HomeRow>();
    const rowId = (row: HomeRow) =>
      JSON.stringify([row.source_key, row.event?.id ?? row.catalog_key]);
    const inboxRow = (row: HomeRow, quiet: boolean): InboxRow => {
      const event = row.event!;
      const id = rowId(row);
      byId.set(id, row);
      return {
        id,
        quiet,
        icon: quiet
          ? I.done
          : event.ask === 'question'
            ? I.qc
            : event.ask === 'decision'
              ? I.decide
              : I.result,
        parts: [
          { className: 'who', text: row.session.name },
          {
            className: 'verb',
            text:
              event.ask === 'question'
                ? ' asked you'
                : event.ask === 'decision'
                  ? ' needs your decision'
                  : ' sent you a result',
          },
        ],
        age: timeText(Math.max(0, home!.now - event.at)) + ' ago',
        preview:
          event.brief || (loading ? 'Loading recorded text…' : 'Recorded preview unavailable'),
        answer:
          quiet && event.ask !== 'result'
            ? event.answer?.length
              ? 'You answered: ' + event.answer.join(' · ')
              : event.declined
                ? 'Declined'
                : 'Answered · reply not in these logs'
            : undefined,
        origin: event.origin ? { message: event.origin } : undefined,
        context: [
          HARNESSES[row.session.harness]?.name ?? row.session.harness,
          home!.machines.find((machine) => machine.key === row.source_key)?.label ?? row.source_key,
        ].join(' · '),
        harness: mark(row.session.harness),
        trace: event.turn ? id : undefined,
      };
    };
    const sessions = new Map(home.working.map((row) => [row.id, row]));
    renderHomeScreen(
      host.root,
      {
        observation: error ?? undefined,
        waiting: home.totals.waiting - (home.inbox.length - inbox.length),
        working: home.totals.working,
        up: home.machines.filter((machine) => machine.up).length,
        machines: home.machines.length,
        inbox: inbox.map((row) => inboxRow(row, false)),
        live: orderRows(
          host.orderScope('working'),
          'working',
          home.working,
          (a, b) => b.session.last - a.session.last,
        ).map((row) => ({
          id: row.id,
          name: row.session.name,
          state: 'work',
          stateLabel: 'Working',
          status: HARNESSES[row.session.harness]?.name ?? row.session.harness,
          detail: [
            home!.machines.length > 1
              ? home!.machines.find((machine) => machine.key === row.source_key)?.label
              : null,
            row.session.detail ? 'for you' : null,
            row.session.detail,
          ]
            .filter(Boolean)
            .join(' · '),
          activity: row.session.activity
            ? ([
                runningToolLabel(row.session.activity[0]),
                row.session.activity[1],
                row.session.activity[2],
              ] as const)
            : undefined,
          harness: mark(row.session.harness),
        })),
        answered: (allAnswered ? home.answered : home.answered.slice(0, 3)).map((row) =>
          inboxRow(row, true),
        ),
        totalAnswered: home.totals.answered,
        allAnswered,
        moreInbox: home.next.inbox !== null,
        moreLive: home.next.working !== null,
        moreAnswered: home.next.answered !== null,
      },
      {
        committed() {},
        trace(id) {
          const row = byId.get(id);
          if (row?.event?.turn) host.trace(row, row.event.turn);
        },
        session(id) {
          open(sessions.get(id));
        },
        inbox(id) {
          const row = byId.get(id);
          if (!row) return;
          if (row.event?.ask === 'result') {
            seen.SEEN_RESULTS.add(JSON.stringify([row.source_key, row.event.id]));
            while (seen.SEEN_RESULTS.size > seen.SEEN_LIMIT)
              seen.SEEN_RESULTS.delete(seen.SEEN_RESULTS.values().next().value!);
            try {
              localStorage.setItem(seen.SEEN_KEY, JSON.stringify([...seen.SEEN_RESULTS]));
            } catch {}
          }
          open(row, row.event?.turn);
        },
        answered() {
          allAnswered = true;
          draw();
        },
        more(section) {
          void moreHome(section);
        },
      },
    );
  }
  function drawAnalytics() {
    render(
      <>
        {[
          [1, '24 h'],
          [7, '7 d'],
          [30, '30 d'],
        ].map(([days, label]) => (
          <button
            type="button"
            class={range === days ? 'on' : ''}
            aria-pressed={range === days}
            onClick={() => {
              range = Number(days);
              analyticsChanged();
            }}
          >
            {label}
          </button>
        ))}
      </>,
      actions,
    );
    const choices = analytics?.facets;
    facetOwner.update(
      (['repo', 'machine', 'harness', 'model'] as const).map((key) => ({
        key,
        label: key === 'repo' ? 'Repo' : key[0].toUpperCase() + key.slice(1),
        value: filters[key],
        display:
          key === 'machine'
            ? (machineLabels[filters[key]] ?? filters[key])
            : key === 'harness'
              ? (HARNESSES[filters[key]]?.name ?? filters[key])
              : key === 'model'
                ? shortModel(filters[key])
                : filters[key] === '__none__'
                  ? 'No repo'
                  : filters[key],
        options: [
          {
            value: '',
            label:
              'All ' + (key === 'repo' ? 'repos' : key === 'harness' ? 'harnesses' : key + 's'),
          },
          ...(choices?.[key] ?? []).map((value) => ({
            value: value ?? '__none__',
            label:
              value == null
                ? 'No repo'
                : key === 'machine'
                  ? (machineLabels[value] ?? value)
                  : key === 'harness'
                    ? (HARNESSES[value]?.name ?? value)
                    : key === 'model'
                      ? shortModel(value)
                      : value,
          })),
        ],
      })),
    );
    const machines: Record<string, string> = { ...machineLabels };
    for (const machine of home?.machines ?? []) machines[machine.key] = machine.label;
    renderAnalyticsPresentation(
      host.root,
      analytics,
      error,
      {
        range,
        measure,
        query: query(),
        machines,
        mark,
        money,
        clock: (time) =>
          new Date(time).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }),
        costTip: 'API-equivalent cost; actual plan charges may differ.',
        width: () => {
          const style = getComputedStyle(host.root);
          return Math.max(
            280,
            host.root.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight),
          );
        },
        sessionHref: (id) =>
          selections[id]
            ? '/s/native/' +
              encodeURIComponent(selections[id].catalog_key) +
              '?machine=' +
              encodeURIComponent(selections[id].source_key)
            : undefined,
      },
      {
        committed() {},
        facets: () => facets,
        session: (id) => open(selections[id]),
        measure: (value) => {
          measure = value;
          draw();
        },
        breakdown(group, key) {
          if (group === 'repo') filters.repo = key;
          else if (group === 'machine') filters.machine = key;
          else [filters.harness, filters.model] = key.split('\u0000');
          const machines = Object.keys(machineLabels);
          host.sessions({
            ...filters,
            machine: filters.machine || (machines.length === 1 ? machines[0] : ''),
          });
        },
        slice(data, bin, costMode) {
          const value = panel(costMode ? 'Sessions with cost' : 'Sessions busy', 'analytics-slice');
          renderSliceBody(
            value.body,
            bin.sessions.map((item) => ({
              id: item.sid,
              name: data.sessions[item.sid]?.name ?? item.sid,
              harness: data.sessions[item.sid]?.harness ?? '',
              harnessName: data.sessions[item.sid]?.harness ?? '',
              mark: mark(data.sessions[item.sid]?.harness ?? ''),
              value:
                costMode && 'usd' in item
                  ? item.usd == null
                    ? '—'
                    : money(item.usd)
                  : timeText('ms' in item ? item.ms : 0),
            })),
            bin.more,
            'No sessions in this range.',
            (id) => {
              open(selections[id]);
            },
          );
          value.show();
        },
        model(group) {
          const value = panel(shortModel(group.model) + ' · ' + group.band);
          renderModelItems(
            value.body,
            group.n + ' work items',
            (group.items ?? []).map((item) => ({
              id: item.sid,
              name: analytics?.sessions[item.sid]?.name,
              description:
                (item.models ?? []).map(shortModel).join(' → ') +
                ' · ' +
                (item.cost_usd == null ? 'cost unknown' : money(item.cost_usd)),
              url: liveUrl(item.pr_url ?? '') ?? undefined,
            })),
            group.items_more ? group.items_more + ' more items' : undefined,
            {
              session(id) {
                open(selections[id]);
              },
              trace() {},
            },
          );
          value.show();
        },
      },
    );
  }
  function draw() {
    if (disposed) return;
    if (host.nativeContent) {
      host.chrome();
      return;
    }
    if (loading) host.root.setAttribute('aria-busy', 'true');
    else host.root.removeAttribute('aria-busy');
    if (host.route() === 'home') {
      if (home && homeSource === null) drawHome();
      else if (host.homeIntro) host.root.replaceChildren(host.homeIntro);
      else
        render(
          <p class="catalog-note" role="status">
            {error ?? 'Loading Home…'}
          </p>,
          host.root,
        );
    } else if (host.route() === 'machine') {
      const key = host.selectedMachine();
      const machine = home?.machines.find((row) => row.key === key);
      if (machine && home)
        renderMachineScreen(
          host.root,
          {
            name: machine.label,
            observation: error ?? undefined,
            totalSessions: machine.sessions,
            more: machine.sessions > home.recent.filter((row) => row.source_key === key).length,
            sessions: home.recent
              .filter((row) => row.source_key === key)
              .sort((a, b) => {
                const rank = (state: string) =>
                  state === 'wait' ? 0 : state === 'work' ? 1 : state === 'err' ? 2 : 3;
                return (
                  rank(a.session.state) - rank(b.session.state) || b.session.last - a.session.last
                );
              })
              .map((row) => ({
                id: row.id,
                name: row.session.name,
                state: row.session.state,
                stateLabel:
                  row.session.state === 'work'
                    ? 'Working'
                    : row.session.state === 'wait'
                      ? 'Waiting on you'
                      : 'Recorded history',
                status:
                  row.session.kind ?? HARNESSES[row.session.harness]?.name ?? row.session.harness,
                age: ago(row.session.last, home!.now),
                detail: [
                  row.session.kind ?? HARNESSES[row.session.harness]?.name ?? row.session.harness,
                  machine.label,
                  row.session.detail ? previewLine(row.session.detail) : null,
                ]
                  .filter(Boolean)
                  .join(' · '),
                activity:
                  row.session.state === 'work' && row.session.activity
                    ? ([
                        runningToolLabel(row.session.activity[0]),
                        row.session.activity[1],
                        row.session.activity[2],
                      ] as const)
                    : undefined,
                harness: mark(row.session.harness),
              })),
            off: [],
            moves: [],
          },
          {
            session(id) {
              open(home?.recent.find((row) => row.id === id));
            },
            moreSessions() {
              host.sessions({ machine: key!, harness: '', repo: '', model: '' });
            },
            inbox() {},
            trace() {},
            committed() {},
          },
        );
      else
        render(
          <p class="catalog-note" role="status">
            {error ?? 'Loading machine…'}
          </p>,
          host.root,
        );
    } else if (host.route() === 'analytics') drawAnalytics();
    else if (host.route() === 'machines') {
      if (home && homeSource === null)
        renderMachinesScreen(
          host.root,
          {
            up: home.machines.filter((machine) => machine.up).length,
            working: home.machines.reduce((sum, machine) => sum + machine.working, 0),
            rows: home.machines.map((machine) => ({
              id: machine.key,
              name: machine.label,
              state: machine.up ? 'work' : 'off',
              stateLabel: machine.up ? 'Online' : 'Offline',
              status: machine.working
                ? machine.working + ' working'
                : machine.up
                  ? 'Online'
                  : 'Offline',
              detail:
                machine.sessions +
                ' sessions' +
                (machine.last == null
                  ? ''
                  : ' · ' + timeText(Math.max(0, home!.now - machine.last)) + ' ago'),
            })),
            admin: host.admin(),
          },
          {
            machine(machine) {
              host.machine(machine);
            },
            admin() {
              const link = host.admin();
              if (link) location.assign(link.href);
            },
            committed() {},
          },
        );
      else
        render(
          <p class="catalog-note" role="status">
            {error ?? 'Loading Machines…'}
          </p>,
          host.root,
        );
    }
    host.chrome();
  }
  async function moreHome(section: 'inbox' | 'working' | 'answered') {
    const offset = home?.next[section];
    if (disposed || loading || offset == null || host.route() !== 'home') return;
    const ticket = epoch;
    loading = true;
    host.root.setAttribute('aria-busy', 'true');
    try {
      const next = parseHome(
        await host.api(
          '/api/viewer/home?' + new URLSearchParams({ [section + '_offset']: String(offset) }),
          { seen: [...seen.SEEN_RESULTS] },
        ),
      );
      if (disposed || ticket !== epoch || host.route() !== 'home' || !home) return;
      if (next.version !== home.version) {
        queued = true;
        return;
      }
      const unique = new Map(
        home[section].map((row) => [
          JSON.stringify([row.source_key, row.event?.id ?? row.catalog_key]),
          row,
        ]),
      );
      for (const row of next[section])
        unique.set(JSON.stringify([row.source_key, row.event?.id ?? row.catalog_key]), row);
      error = null;
      const previewsIncomplete = await hydrateHome(next, ticket);
      if (disposed || ticket !== epoch || host.route() !== 'home') return;
      if (previewsIncomplete) error = 'Some recorded previews are unavailable. Retrying…';
      home = {
        ...home,
        [section]: [...unique.values()],
        next: { ...home.next, [section]: next.next[section] },
        totals: next.totals,
      };
    } catch (failure) {
      if (!disposed && ticket === epoch)
        error = failure instanceof Error ? failure.message : 'Activity unavailable';
    } finally {
      if (!disposed && ticket === epoch) {
        loading = false;
        draw();
        if (queued) {
          queued = false;
          void refresh();
        }
      }
    }
  }
  async function hydrateHome(next: HomeData, ticket: number) {
    const selected = new Map<string, HomeRow[]>();
    for (const row of host.route() === 'machine'
      ? next.recent
      : [...next.inbox, ...next.working, ...next.answered]) {
      if (row.previewReady) continue;
      if (!row.contentGeneration) continue;
      const key = JSON.stringify([row.source_key, row.catalog_key]);
      selected.set(key, [...(selected.get(key) ?? []), row]);
    }
    let incomplete = false;
    const work = [...selected.values()].flatMap((rows) =>
      Array.from({ length: Math.ceil(rows.length / 8) }, (_, page) =>
        rows.slice(page * 8, (page + 1) * 8),
      ),
    );
    let cursor = 0;
    await Promise.all(
      Array.from({ length: Math.min(4, work.length) }, async () => {
        while (
          cursor < work.length &&
          !disposed &&
          ticket === epoch &&
          ['home', 'machine'].includes(host.route())
        ) {
          const rows = work[cursor++],
            first = rows[0];
          const ids = [...new Set(rows.flatMap((row) => (row.event ? [row.event.id] : [])))];
          const args = new URLSearchParams({
            machine: first.source_key,
            sid: first.catalog_key,
            scope: 'retained_history',
            generation: first.contentGeneration!,
            events: JSON.stringify(ids),
          });
          try {
            const value = object(await host.api('/api/session-home?' + args));
            if (disposed || ticket !== epoch || !['home', 'machine'].includes(host.route())) return;
            const identity = object(value.identity),
              session = object(value.session);
            if (
              value.api !== 1 ||
              identity.source_key !== first.source_key ||
              identity.catalog_key !== first.catalog_key ||
              value.generation !== first.contentGeneration
            )
              throw Error('Home preview identity changed');
            const events = dictionary(value.events, (value) => {
              const row = object(value);
              return {
                brief: text(row.brief),
                answer: nullable(row.answer, (value) => array(value, text)),
                origin: nullable(row.origin, text),
              };
            });
            const detail = nullable(session.detail, text);
            const activity = parseActivity(session.activity, next.now);
            const failed = boolean(value.incomplete);
            if (failed && rows.length > 1) {
              const half = Math.ceil(rows.length / 2);
              work.push(rows.slice(0, half), rows.slice(half));
            } else incomplete ||= failed;
            for (const row of rows) {
              row.previewReady = !failed;
              row.session.detail = detail ?? undefined;
              row.session.activity = activity;
              if (row.event && events[row.event.id])
                Object.assign(row.event, events[row.event.id], {
                  origin: events[row.event.id].origin ?? undefined,
                });
            }
            if (
              home === next &&
              !disposed &&
              ticket === epoch &&
              ['home', 'machine'].includes(host.route())
            )
              draw();
          } catch {
            if (!disposed && ticket === epoch) incomplete = true;
          }
        }
      }),
    );
    return incomplete;
  }
  async function refresh(force = false) {
    if (disposed || !['home', 'analytics', 'machines', 'machine'].includes(host.route())) return;
    if (!force && host.route() === 'analytics' && Date.now() < analyticsRetryAt) return;
    if (loading) {
      queued = true;
      queuedForce ||= force;
      return;
    }
    cancelRetry();
    const route = host.route(),
      ticket = ++epoch,
      requestedQuery =
        route === 'analytics'
          ? query()
          : route === 'machine'
            ? new URLSearchParams({ source: host.selectedMachine()! }).toString()
            : '';
    const current = () =>
      !disposed &&
      ticket === epoch &&
      host.route() === route &&
      (route !== 'analytics' || query() === requestedQuery) &&
      (route !== 'machine' ||
        new URLSearchParams({ source: host.selectedMachine()! }).toString() === requestedQuery);
    loading = true;
    host.root.setAttribute('aria-busy', 'true');
    try {
      const args = new URLSearchParams(requestedQuery);
      if (observationAfter)
        args.set(
          'observation_after',
          JSON.stringify([observationAfter.source_key, observationAfter.catalog_key]),
        );
      const value = await host.api(
        '/api/viewer/' +
          (route === 'analytics' ? 'analytics' : 'home') +
          (args.size ? '?' + args : ''),
        route === 'analytics' ? undefined : { seen: [...seen.SEEN_RESULTS] },
      );
      if (!current()) return;
      if (route === 'analytics' && value === null) {
        const kept = analyticsCache.get(requestedQuery);
        if (!kept) throw Error('Unchanged Analytics has no accepted result');
        acceptAnalytics(kept);
        error = null;
        analyticsRetryAt = 0;
        return;
      }
      const observations = array(object(value).observations, (value) => object(value));
      const pending = array(object(value).pending_observations ?? [], parseSelection);
      if (pending.length > 8) throw Error('Pending observations exceed their bound');
      if (pending.length) observationAfter = pending.at(-1)!;
      const observe = async () => {
        let cursor = 0;
        await Promise.all(
          Array.from({ length: Math.min(2, pending.length) }, async () => {
            while (cursor < pending.length && current()) {
              const selected = pending[cursor++];
              try {
                await host.api(
                  '/api/session-observe?' +
                    new URLSearchParams({
                      machine: selected.source_key,
                      sid: selected.catalog_key,
                      scope: 'retained_history',
                    }),
                );
              } catch {
                /* The existing overview refresh observes producer recovery. */
              }
            }
          }),
        );
      };
      if (route !== 'analytics') {
        const next = parseHome(value);
        if (
          route === 'machine' &&
          (next.machines.length !== 1 || next.machines[0].key !== host.selectedMachine())
        )
          throw Error('Machine activity source changed');
        const pages = Math.max(
          1,
          ...(['inbox', 'working', 'answered'] as const).map((section) =>
            Math.ceil((home?.[section].length ?? 0) / 100),
          ),
        );
        for (let page = 1; page < pages; page++) {
          const args = new URLSearchParams();
          for (const section of ['inbox', 'working', 'answered'] as const)
            if ((home?.[section].length ?? 0) > page * 100)
              args.set(section + '_offset', String(page * 100));
          const additional = parseHome(
            await host.api('/api/viewer/home?' + args, { seen: [...seen.SEEN_RESULTS] }),
          );
          if (disposed || ticket !== epoch || host.route() !== route) return;
          if (additional.version !== next.version)
            throw Error('Activity changed while loading. Retrying…');
          for (const section of ['inbox', 'working', 'answered'] as const)
            if (args.has(section + '_offset')) {
              next[section].push(...additional[section]);
              next.next[section] = additional.next[section];
            }
        }
        if (home?.version === next.version) {
          const previous = new Map(
            (route === 'machine'
              ? home.recent
              : [...home.inbox, ...home.working, ...home.answered]
            ).map((row) => [JSON.stringify([row.source_key, row.catalog_key, row.event?.id]), row]),
          );
          for (const row of route === 'machine'
            ? next.recent
            : [...next.inbox, ...next.working, ...next.answered]) {
            const retained = previous.get(
              JSON.stringify([row.source_key, row.catalog_key, row.event?.id]),
            );
            if (!retained || retained.contentGeneration !== row.contentGeneration) continue;
            row.previewReady = retained.previewReady;
            row.session.detail = retained.session.detail;
            if (retained.session.activity) row.session.activity = retained.session.activity;
            if (row.event && retained.event) {
              row.event.brief = retained.event.brief;
              row.event.answer = retained.event.answer;
              row.event.origin = retained.event.origin;
            }
          }
        }
        home = next;
        homeSource = route === 'machine' ? host.selectedMachine() : null;
        recentRows = next.recent;
        recentNow = next.now;
        error = observations.length
          ? 'Some machine history is still being observed. Retrying…'
          : null;
        draw();
        if (route !== 'home' && route !== 'machine') {
          await observe();
          return;
        }
        const [previewsIncomplete] = await Promise.all([
          host.nativeContent ? Promise.resolve(false) : hydrateHome(next, ticket),
          observe(),
        ]);
        if (!current()) return;
        if (previewsIncomplete) observations.push({ state: 'previews_unavailable' });
        home = next;
        homeSource = route === 'machine' ? host.selectedMachine() : null;
        recentRows = next.recent;
        recentNow = next.now;
      } else {
        const data = object(value);
        const nextAnalytics = parseAnalytics(data),
          nextSelections = dictionary(data.selections, parseSelection),
          nextLabels = dictionary(data.machine_labels, text),
          nextRecent = array(data.recent, (row) => parseHomeRow(row, number(data.now))),
          nextNow = number(data.now);
        if (nextRecent.length > 60) throw Error('Recent response exceeds its bound');
        const accepted = {
          data: nextAnalytics,
          selections: nextSelections,
          labels: nextLabels,
          recent: nextRecent,
          now: nextNow,
        };
        analyticsCache.delete(requestedQuery);
        analyticsCache.set(requestedQuery, accepted);
        while (analyticsCache.size > 3) analyticsCache.delete(analyticsCache.keys().next().value!);
        acceptAnalytics(accepted);
        analyticsRetryAt = 0;
        draw();
        await observe();
      }
      if (!current()) return;
      error = observations.length
        ? 'Some machine history is still being observed. Retrying…'
        : null;
    } catch (failure) {
      if (current()) {
        error = failure instanceof Error ? failure.message : 'Overview unavailable. Retrying…';
        if (route === 'analytics') analyticsRetryAt = Date.now() + 10000;
      }
    } finally {
      if (!disposed && ticket === epoch) {
        loading = false;
        draw();
        if (queued) {
          const force = queuedForce;
          queued = false;
          queuedForce = false;
          void refresh(force);
        } else if (current() && error && (route === 'home' || route === 'machine')) {
          retryTimer = host.effects.timeout(() => {
            retryTimer = undefined;
            if (current()) void refresh();
          }, retryDelay);
          retryDelay = Math.min(8000, retryDelay * 2);
        } else if (current() && !error) {
          retryDelay = 500;
        }
      }
    }
  }
  return {
    machine: () => home?.machines.find((row) => row.key === host.selectedMachine()),
    actions,
    waitingCount: () =>
      home && homeSource === null
        ? Math.max(
            0,
            home.totals.waiting -
              home.inbox.filter(
                (row) =>
                  row.event?.ask === 'result' &&
                  ((home!.machines.length === 1 && seen.SEEN_RESULTS.has(row.event.id)) ||
                    seen.SEEN_RESULTS.has(JSON.stringify([row.source_key, row.event.id]))),
              ).length,
          )
        : undefined,
    recentSnapshot,
    markResults(source: string, ids: readonly string[]) {
      let changed = false;
      for (const id of ids) {
        const key = JSON.stringify([source, id]);
        if (seen.SEEN_RESULTS.has(key)) continue;
        seen.SEEN_RESULTS.add(key);
        changed = true;
      }
      if (!changed) return;
      while (seen.SEEN_RESULTS.size > seen.SEEN_LIMIT)
        seen.SEEN_RESULTS.delete(seen.SEEN_RESULTS.values().next().value!);
      try {
        localStorage.setItem(seen.SEEN_KEY, JSON.stringify([...seen.SEEN_RESULTS]));
      } catch {}
      host.chrome();
    },
    openRecent(id: string) {
      const row = recentRows.find((row) => row.id === id);
      if (!row) return false;
      open(row);
      return true;
    },
    toggleRecent(id: string, value: boolean) {
      recentOpen.set(id, value);
      while (recentOpen.size > 200) recentOpen.delete(recentOpen.keys().next().value!);
      try {
        localStorage.setItem(
          'semon.tree',
          JSON.stringify(Object.fromEntries([...recentOpen].map(([key, open]) => [key, { open }]))),
        );
      } catch {}
      host.chrome();
    },
    allRecent(id: string, value: boolean, trigger?: HTMLButtonElement) {
      if (value && trigger && matchMedia('(max-width: 760px)').matches) {
        const root = recentRows.find((row) => row.id === id);
        if (!root) return;
        const rows: HomeRow[] = [],
          visited = new Set<string>([root.id]);
        function collect(parent: HomeRow) {
          for (const row of recentRows) {
            if (
              visited.has(row.id) ||
              row.source_key !== parent.source_key ||
              row.session.parent !== parent.catalog_key
            )
              continue;
            visited.add(row.id);
            rows.push(row);
            collect(row);
          }
        }
        collect(root);
        const sheet = createKidsSheet(
          root.session.name,
          rows.map((row) => ({
            id: row.id,
            name: row.session.name,
            state: row.session.state,
            stateLabel:
              row.session.state === 'wait'
                ? 'Waiting on you'
                : row.session.state === 'work'
                  ? 'Working'
                  : 'Recorded history',
            age: timeText(Math.max(0, recentNow - row.session.last)),
            under:
              row.session.parent === root.catalog_key
                ? undefined
                : recentRows.find(
                    (parent) =>
                      parent.source_key === row.source_key &&
                      parent.catalog_key === row.session.parent,
                  )?.session.name,
            bucket: row.session.state === 'wait' ? 0 : row.session.state === 'work' ? 1 : 2,
          })),
          {
            opened() {
              host.panelOpened(sheet);
            },
            closed() {
              host.panelClosed(sheet);
              dialogs.delete(sheet);
              if (trigger.isConnected) trigger.focus({ preventScroll: true });
            },
            matches(key, query) {
              const row = rows.find((row) => row.id === key);
              return (
                !!row &&
                (!query ||
                  [row.session.name, row.session.repo, row.session.model, row.session.harness].some(
                    (field) => field?.toLowerCase().includes(query.toLowerCase()),
                  ))
              );
            },
            select(key) {
              open(rows.find((row) => row.id === key));
            },
          },
        );
        dialogs.add(sheet);
        sheet.show();
        return;
      }
      if (value) recentAll.add(id);
      else recentAll.delete(id);
      host.chrome();
    },
    draw,
    refresh,
    leave() {
      cancelRetry();
      retryDelay = 500;
      ++epoch;
      queued = false;
      queuedForce = false;
      loading = false;
      host.root.removeAttribute('aria-busy');
      facets.querySelector<HTMLDialogElement>('dialog[open]')?.close();
      error = null;
      for (const value of [...dialogs]) value.destroy();
    },
    destroy() {
      cancelRetry();
      disposed = true;
      ++epoch;
      for (const value of [...dialogs]) value.destroy();
      facetOwner.destroy();
      analyticsCache.clear();
      render(null, actions);
    },
  };
}

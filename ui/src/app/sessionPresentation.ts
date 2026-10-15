import type { CatalogSession } from '../state/catalog-wire';
import type { BarLabel, HarnessMark, MenuAction, MenuDetail, SessionMenuSnapshot } from '../lib';
import { ago, clock, compactCount, dur, shortModel } from '../domain/format';
import type { SessionRow } from '../lib';
import { resumeCommand } from '../domain/resume';
import type { Cost, TokenKind } from '../domain/types';
import { HARNESSES, HARNESS, I, STATE } from './registry';

const money = (value: number | null | undefined) => (value == null ? '—' : '$' + value.toFixed(2));
const trademark =
  'Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.';
export function selectedSessionState(session: CatalogSession): string {
  const details = session.details;
  if (!details) return 'off';
  return (details.state === 'work' || details.state === 'wait') && !details.live_observation
    ? 'off'
    : details.state;
}
function familyCost(session: CatalogSession): Cost {
  const own: Cost = session.details?.cost ?? session.cost,
    family = session.details?.family;
  if (!family?.runs.length) return own;
  const costs = [own, ...family.runs.map((run) => run.cost)],
    models: NonNullable<Cost['by_model']> = {};
  for (const cost of costs)
    for (const [id, model] of Object.entries(cost.by_model ?? {})) {
      const total = (models[id] ??= { usd: 0, tokens: {}, usd_by_kind: {} });
      total.usd = total.usd == null || model.usd == null ? null : total.usd + model.usd;
      for (const field of ['tokens', 'usd_by_kind'] as const)
        for (const [kind, value] of Object.entries(model[field] ?? {})) {
          const key = kind as TokenKind;
          total[field]![key] = (total[field]![key] ?? 0) + value;
        }
    }
  return {
    usd:
      family.state === 'complete' && costs.every((cost) => cost.usd != null)
        ? costs.reduce((sum, cost) => sum + cost.usd!, 0)
        : null,
    unpriced_models: [...new Set(costs.flatMap((cost) => cost.unpriced_models ?? []))],
    by_model: models,
  };
}
function mark(harness: string): HarnessMark | undefined {
  const h = HARNESSES[harness];
  return h
    ? {
        id: harness,
        name: h.name,
        light: h.icon.light,
        dark: h.icon.dark,
        darkTheme: matchMedia('(prefers-color-scheme: dark)').matches,
      }
    : undefined;
}
/** Group only loaded canonical parents; absent parents remain selectable roots. */
export function catalogSessionGroups<T extends { key: string; parent: string | null }>(
  items: readonly T[],
): { root: T; children: T[] }[] {
  const byKey = new Map(items.map((item) => [item.key, item])),
    parents = new Map(
      items.map((item) => [
        item.key,
        item.parent && item.parent !== item.key && byKey.has(item.parent) ? item.parent : null,
      ]),
    ),
    resolved = new Set<string>();
  for (const item of items) {
    const chain: string[] = [],
      positions = new Map<string, number>();
    let key: string | null = item.key;
    while (key && !resolved.has(key)) {
      const repeat = positions.get(key);
      if (repeat !== undefined) {
        // A malformed relationship cycle has one deterministic visible root.
        parents.set(chain.slice(repeat).sort()[0], null);
        break;
      }
      positions.set(key, chain.length);
      chain.push(key);
      key = parents.get(key) ?? null;
    }
    for (const key of chain) resolved.add(key);
  }
  const children = new Map<string, T[]>();
  for (const item of items) {
    const parent = parents.get(item.key);
    if (parent) {
      const values = children.get(parent) ?? [];
      values.push(item);
      children.set(parent, values);
    }
  }
  return items
    .filter((item) => !parents.get(item.key))
    .map((root) => {
      const descendants: T[] = [],
        pending = [...(children.get(root.key) ?? [])].reverse();
      while (pending.length) {
        const child = pending.pop()!;
        descendants.push(child);
        pending.push(...[...(children.get(child.key) ?? [])].reverse());
      }
      return { root, children: descendants };
    });
}

/** Fenced list observations describe status without granting live control. */
export function catalogSessionRow(session: CatalogSession): SessionRow {
  const state = session.status?.state ?? 'off';
  return {
    id: session.key,
    name: session.name,
    kind: session.presentation_kind ?? session.kind,
    state,
    stateLabel:
      STATE[state] ??
      (session.status?.observation === 'retained' ? 'Recorded history' : 'Status unavailable'),
    age: session.last == null ? '' : ago(session.last, Date.now()),
    harness: mark(session.harness),
    model: shortModel(session.model),
    modelTip: session.model,
    fields: [
      ...(session.repo
        ? [
            {
              className: 'repo-short',
              text: session.repo.split('/').at(-1)!,
              tip: session.repo,
              priority: 1,
              icon: I.repo,
            },
          ]
        : []),
      ...(session.branch
        ? [
            {
              className: 'branch',
              text: session.branch,
              tip: session.branch,
              priority: 2,
              icon: I.branch,
            },
          ]
        : []),
    ],
  };
}
export function selectedSessionLabels(session: CatalogSession, machine?: string): BarLabel[] {
  const details = session.details,
    state = selectedSessionState(session),
    kind =
      session.presentation_kind ??
      (
        {
          subagent: 'Subagent',
          'codex-run': 'Codex run',
          'copilot-session': 'Copilot session',
        } as Record<string, string>
      )[session.kind],
    cost = familyCost(session);
  const rows: BarLabel[] = [
    {
      key: 'state',
      text: STATE[state] ?? 'Recorded history',
      tip: 'Recorded status · ' + (details?.turns ?? 0) + ' turns',
      drop: 0,
      className: 'state ' + state,
      state,
      stateLabel: STATE[state] ?? 'Recorded history',
    },
    {
      key: 'harness',
      className: 'hname',
      text: kind ?? HARNESS[session.harness] ?? session.harness,
      tip: HARNESS[session.harness] ?? session.harness,
      drop: 2,
    },
    {
      key: 'model',
      className: 'meta-model',
      text: shortModel(session.model),
      tip: 'Model: ' + session.model,
      drop: 3,
      effort: session.effort ?? undefined,
    },
  ];
  if (details?.errors)
    rows.push({
      key: 'errors',
      text: details.errors + ' failed',
      tip: 'Recorded failed steps',
      drop: 0,
      className: 'errs lab-errs',
      action: 'errors',
    });
  if (details?.family?.runs.length)
    rows.push({
      key: 'runs',
      text:
        details.family.runs.length +
        (details.family.runs.length === 1 ? ' run' : ' runs') +
        (details.family.state === 'incomplete' ? '+' : ''),
      tip: 'Recorded child sessions',
      drop: 1,
      action: 'runs',
    });
  if (machine) rows.push({ key: 'machine', text: machine, tip: 'Machine: ' + machine, drop: 5 });
  if (session.branch)
    rows.push({ key: 'branch', text: session.branch, tip: 'Branch: ' + session.branch, drop: 6 });
  rows.push({
    key: 'cost',
    text: money(cost.usd),
    tip:
      'API-equivalent cost' +
      (details?.family?.runs.length ? ' · this session and its runs' : ' · this session'),
    drop: 7,
  });
  return rows;
}

export function selectedSessionMenu(
  session: CatalogSession,
  machine: string,
  wide: boolean,
  trace: boolean,
): SessionMenuSnapshot {
  const metadata = session.details,
    ownCost: Cost = metadata?.cost ?? session.cost,
    cost = familyCost(session),
    state = selectedSessionState(session),
    now = Date.now(),
    native = session.native_ids.length === 1 ? session.native_ids[0] : undefined,
    command = native
      ? resumeCommand({ harness: session.harness, id: session.key, sessionId: native })
      : null,
    actions: MenuAction[] = [];
  if (trace)
    actions.push({ key: 'trace', text: 'Trace this turn', icon: I.trace, className: 'menu-trace' });
  if (command) actions.push({ key: 'copy', text: 'Copy resume command', icon: I.copy });
  if (metadata?.errors)
    actions.push({
      key: 'errors',
      text: metadata.errors + ' failed',
      dot: 'err',
      note: 'Step through',
      className: 'menu-errors',
    });
  actions.push({ key: 'source', text: 'Choose a machine', icon: I.machine });
  if (!matchMedia('(max-width: 760px)').matches)
    actions.push({ key: 'wide', text: 'Wide transcript', icon: I.wide, checked: wide });
  else if (metadata?.family?.runs.length)
    actions.push({
      key: 'runs',
      text: 'Runs · ' + metadata.family.runs.length,
      icon: I.stack,
      className: 'menu-runs',
    });
  const signals = Object.values(metadata?.signals ?? {}).reduce((sum, count) => sum + count, 0);
  if (signals)
    actions.push({
      key: 'signals',
      text: signals + ' signals',
      note: 'Step through',
      className: 'menu-signals',
    });
  const details: MenuDetail[] = [
    {
      label: 'Status',
      value: metadata
        ? (STATE[state] ?? 'Recorded history') +
          ' · ' +
          metadata.turns +
          (metadata.turns === 1 ? ' turn' : ' turns')
        : 'Recorded history',
    },
    ...(session.presentation_kind ? [{ label: 'Kind', value: session.presentation_kind }] : []),
    {
      label: 'Harness',
      value: HARNESS[session.harness] ?? session.harness,
      harness: mark(session.harness),
    },
    { label: 'Model', value: session.model, mono: true },
    { label: 'Machine', value: machine },
  ];
  for (const [label, value] of [
    ['Effort', session.effort],
    ['Directory', metadata?.cwd ?? session.repo],
    ['Branch', session.branch],
  ])
    if (value)
      details.push({
        label: label!,
        value,
        mono: ['Directory', 'Branch', 'Session id'].includes(label!),
      });
  if (metadata?.calls != null) details.push({ label: 'Tool calls', value: String(metadata.calls) });
  if (metadata?.errors) details.push({ label: 'Errors', value: String(metadata.errors) });
  if (session.start != null)
    details.push(
      { label: 'Started', value: clock(session.start, now) },
      { label: 'Duration', value: dur(session.start, session.last, now) },
    );
  if (native) details.push({ label: 'Session id', value: native, mono: true });
  for (const [kind, count] of Object.entries(metadata?.signals ?? {}))
    details.push({ label: 'Signals · ' + kind, value: String(count) });
  const kinds: [string, TokenKind[]][] = [
      ['Input', ['input']],
      ['Output', ['output']],
      ['Cache write', ['cache_write_5m', 'cache_write_1h', 'cache_write']],
      ['Cache read', ['cache_read']],
      ['Web search', ['web_search']],
    ],
    models = Object.entries(cost.by_model ?? {}).map(([id, model]) => ({
      id,
      rows: kinds.flatMap(([label, keys]) => {
        const tokens = keys.reduce((sum, key) => sum + (model.tokens?.[key] ?? 0), 0),
          usd = keys.reduce((sum, key) => sum + (model.usd_by_kind?.[key] ?? 0), 0);
        return tokens || usd
          ? [
              {
                label,
                count: tokens ? compactCount(tokens) : '',
                exact: tokens ? tokens.toLocaleString() : undefined,
                cost: model.usd == null ? '—' : money(usd),
              },
            ]
          : [];
      }),
    }));
  const reports = metadata?.reported_runs ?? [],
    reported = reports.filter((row) => row.cost_usd != null),
    costDetails: MenuDetail[] = [];
  if (metadata?.family?.runs.length)
    costDetails.push({ label: 'This session', value: money(ownCost.usd) });
  if (reports.length)
    costDetails.push({
      label: (HARNESS[session.harness] ?? session.harness) + "'s own figure",
      value: reported.length
        ? money(reported.reduce((sum, row) => sum + row.cost_usd!, 0)) +
          ', last ' +
          reported.length +
          (reported.length === 1 ? ' run' : ' runs')
        : 'not reported',
    });
  return {
    actions,
    path: metadata?.family?.ancestors.length
      ? [
          ...metadata.family.ancestors.map((row) => ({
            id: row.key,
            name: row.name,
            harness: row.harness,
            harnessName: HARNESS[row.harness] ?? row.harness,
          })),
          {
            id: session.key,
            name: session.name,
            harness: session.harness,
            harnessName: HARNESS[session.harness] ?? session.harness,
          },
        ]
      : [],
    status: [
      STATE[state] ?? 'Recorded history',
      session.presentation_kind ?? HARNESS[session.harness] ?? session.harness,
      shortModel(session.model),
    ].join(' · '),
    state,
    stateLabel: STATE[state] ?? 'Recorded history',
    details,
    command: command ?? '',
    notice: trademark,
    icons: { chevron: I.chev, copy: I.copy },
    cost: {
      figure: money(cost.usd),
      caption:
        'this session' +
        (metadata?.family?.runs.length
          ? ' and its ' +
            metadata.family.runs.length +
            (metadata.family.runs.length === 1 ? ' run' : ' runs')
          : ''),
      note:
        "What these tokens would cost at API rates. Subscriptions aren't billed this way." +
        (metadata?.copied_usage
          ? ' Copied usage is excluded. Its original owner and the fresh usage total are unknown.'
          : '') +
        (metadata?.family?.state === 'incomplete'
          ? ' The recorded family is incomplete; its total cost is unknown.'
          : '') +
        (metadata?.copilot
          ? ' Copilot usage is a cumulative saved snapshot. Fresh usage and complete cost are unknown.'
          : '') +
        (cost.unpriced_models?.length
          ? ' No price for ' + cost.unpriced_models.join(', ') + '.'
          : ''),
      details: costDetails,
      mismatch: (() => {
        const check = metadata?.cost_check
          .slice()
          .reverse()
          .find(
            (row) =>
              row.ok === false &&
              row.reported_usd != null &&
              row.reported_usd > 0 &&
              row.computed_usd != null,
          );
        if (!check) return undefined;
        const difference = (check.computed_usd! / check.reported_usd! - 1) * 100;
        return (
          "Semon's estimate for that run is " +
          Math.round(Math.abs(difference)) +
          '% ' +
          (difference > 0 ? 'above' : 'below') +
          ' ' +
          (HARNESS[session.harness] ?? session.harness) +
          "'s figure."
        );
      })(),
      runs: (metadata?.family?.runs ?? []).map((run) => ({
        id: run.key,
        depth: run.parent === session.key ? 0 : 1,
        label: 'Open ' + run.name,
        name: run.name,
        kind: run.kind,
        state: 'off',
        stateLabel: 'Recorded history',
        cost: money(run.cost.usd),
      })),
      models,
      includesRuns: !!metadata?.family?.runs.length,
    },
  };
}

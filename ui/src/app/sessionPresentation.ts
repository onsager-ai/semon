import type { CatalogSession } from '../state/catalog-wire';
import type {
  BarLabel,
  HarnessMark,
  MenuAction,
  MenuDetail,
  SessionMenuSnapshot,
  FooterView,
} from '../lib';
import { ago, clock, compactCount, dur, shortModel } from '../domain/format';
import { catalogSessionKey } from '../state/catalog-wire';
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
  if (
    session.harness === 'codex' &&
    (!details.turn_lifecycle || details.turn_lifecycle.phase === 'unknown')
  )
    return 'off';
  return (details.state === 'work' || details.state === 'wait') && !details.live_observation
    ? 'off'
    : details.state;
}
function turnOutcome(session: CatalogSession): string | undefined {
  const phase = session.details?.turn_lifecycle?.phase;
  if (!phase) return session.harness === 'codex' && session.details ? 'Unknown' : undefined;
  return {
    active: 'Terminal not recorded',
    completed: 'Completed',
    aborted: 'Aborted',
    errored: 'Errored',
    unknown: 'Unknown',
  }[phase];
}
function recordedCostTip(cost: Cost): string {
  const groups = Object.entries(cost.by_model ?? {}).map(([id, model]) => {
    const values = model.usd_by_kind ?? {},
      kinds = [
        ['Input', values.input],
        ['Output', values.output],
        ['Cache read', values.cache_read],
        [
          'Cache write',
          (Number(values.cache_write_5m) || 0) + (Number(values.cache_write_1h) || 0),
        ],
      ];
    if (Number(values.web_search) >= 0.005) kinds.push(['Web search', values.web_search]);
    return {
      id,
      text: kinds.map(([label, usd]) => label + ' ' + money(Number(usd) || 0)).join(' · '),
    };
  });
  return (
    (groups.length > 1
      ? groups.map((group) => group.id + ': ' + group.text).join('; ')
      : groups.map((group) => group.text).join('')) +
    (groups.length ? '. ' : '') +
    "What these tokens would cost at API rates. Subscriptions (Claude Max, ChatGPT plans) aren't billed this way."
  );
}
/** Recorded footer facts share the selected observation fence; they grant no control authority. */
export function selectedSessionFooter(
  session: CatalogSession,
  origin?: { id: string; name: string; status: string; done?: number; turn?: string },
): FooterView | undefined {
  const details = session.details;
  if (!details || (!origin && details.role)) return undefined;
  if (
    origin &&
    !['work', 'done', 'err'].includes(details.state) &&
    !['done', 'err'].includes(origin.status)
  )
    return undefined;
  const recordedState = selectedSessionState(session),
    returned =
      !!origin &&
      ['done', 'err'].some((state) => recordedState === state || origin.status === state),
    state = returned
      ? recordedState === 'err' || origin?.status === 'err'
        ? 'err'
        : 'done'
      : recordedState,
    finished = returned || ['done', 'err'].includes(recordedState),
    now = Date.now(),
    finishedAt = origin?.done ?? session.last,
    calls = details.calls,
    cost = details.cost,
    items: FooterView['items'][number][] = [],
    callsTip = Object.entries(details.tool_calls)
      .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1))
      .map(([name, count]) => name + ' ' + count);
  if (details.errors) callsTip.push(details.errors + ' failed');
  if (!returned)
    items.push({
      kind: 'calls',
      text: (calls ?? '—') + (calls === 1 ? ' tool call' : ' tool calls'),
      tip: callsTip.join(' · '),
    });
  if (session.start != null)
    items.push({
      kind: 'time',
      text: dur(session.start, state === 'work' ? null : session.last, Date.now()),
      tip:
        'Started ' +
        clock(session.start, now) +
        ' · last activity ' +
        (session.last == null ? 'Unknown' : clock(session.last, now)) +
        (finished && finishedAt != null ? ' · finished ' + clock(finishedAt, now) : ''),
    });
  if (cost.usd != null && cost.usd >= 0.005 && !cost.unpriced_models?.length)
    items.push({
      kind: 'cost',
      text: money(cost.usd),
      tip: recordedCostTip(cost),
    });
  return {
    state,
    stateLabel: STATE[state] ?? 'Recorded history',
    text: returned
      ? 'Returned to ' + origin!.name + ' · ' + STATE[state]
      : (STATE[state] ?? 'Recorded history'),
    items,
    parent: returned ? { id: origin!.id, name: origin!.name, turn: origin!.turn } : undefined,
  };
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
export function catalogSessionRow(
  session: CatalogSession,
  children: readonly CatalogSession[] = [],
): SessionRow {
  const state = session.status?.state ?? 'off';
  const subagents = children.filter(
      (row) => row.presentation_kind === 'Subagent' || row.kind === 'subagent',
    ).length,
    runs = children.filter(
      (row) => row.presentation_kind === 'Codex run' || row.kind === 'codex-run',
    ).length,
    other = children.length - subagents - runs;
  const counts = [
    subagents ? subagents + (subagents === 1 ? ' subagent' : ' subagents') : '',
    runs ? runs + (runs === 1 ? ' Codex run' : ' runs') : '',
    other ? other + (other === 1 ? ' other run' : ' other runs') : '',
  ]
    .filter(Boolean)
    .join(' · ');
  return {
    id: catalogSessionKey(session),
    name: session.name,
    kind: session.presentation_kind ?? session.kind,
    state,
    stateLabel:
      STATE[state] ??
      (session.status?.observation === 'retained' ? 'Recorded history' : 'Status unavailable'),
    age: session.last == null ? '' : ago(session.last, Date.now()),
    harness: mark(session.harness),
    delegation: session.parent ? I.stack : undefined,
    model: shortModel(session.model),
    modelTip: session.model,
    fields: [
      ...(session.machine
        ? [
            {
              className: 'row-machine',
              text: session.machine.label,
              tip: session.machine.label,
              priority: 0,
              icon: I.machine,
            },
          ]
        : []),
      ...(session.start !== null
        ? [
            {
              className: 'row-duration',
              text: dur(
                session.start,
                state === 'work' || state === 'wait' ? null : session.last,
                Date.now(),
              ),
              tip: 'Duration',
              priority: 1,
              icon: I.duration,
            },
          ]
        : []),
      ...(session.repo
        ? [
            {
              className: 'repo-short',
              text: session.repo,
              tip: session.repo,
              priority: 3,
              icon: I.repo,
            },
          ]
        : []),
      ...(counts
        ? [
            {
              className: 'row-counts',
              text: counts,
              priority: 4,
              tip: 'Recorded child sessions in the loaded page',
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
      tip:
        'Recorded status · ' +
        (details?.turns ?? 0) +
        ' turns' +
        (turnOutcome(session) ? ' · Last turn: ' + turnOutcome(session) : ''),
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
    phone = matchMedia('(max-width: 760px)').matches,
    now = Date.now(),
    native = session.native_ids.length === 1 ? session.native_ids[0] : undefined,
    command = native
      ? resumeCommand({ harness: session.harness, id: session.key, sessionId: native })
      : null,
    actions: MenuAction[] = [];
  if (trace)
    actions.push({ key: 'trace', text: 'Trace this turn', icon: I.trace, className: 'menu-trace' });
  if (command) actions.push({ key: 'copy', text: 'Copy resume command', icon: I.copy });
  if (session.harness === 'claude')
    actions.push({
      key: 'external',
      text: 'Open in claude.ai',
      icon: I.ext,
      unavailable: 'An external conversation link is unavailable for this recorded session.',
    });
  if (phone && metadata?.errors)
    actions.push({
      key: 'errors',
      text: metadata.errors + ' failed',
      dot: 'err',
      note: 'Step through',
      className: 'menu-errors',
    });
  if (!phone) actions.push({ key: 'wide', text: 'Wide transcript', icon: I.wide, checked: wide });
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
  if (turnOutcome(session)) details.push({ label: 'Last turn', value: turnOutcome(session)! });
  for (const [label, value] of [
    ['Effort', session.effort],
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
    family = metadata?.family,
    familyRuns = family?.runs ?? [],
    costDetails: MenuDetail[] = [];
  if (familyRuns.length)
    costDetails.push(
      { label: 'This session', value: money(ownCost.usd) },
      {
        label: familyRuns.length === 1 ? 'Its run' : 'Its ' + familyRuns.length + ' runs',
        value: money(
          family?.state === 'complete' && familyRuns.every((run) => run.cost.usd != null)
            ? familyRuns.reduce((sum, run) => sum + run.cost.usd!, 0)
            : null,
        ),
      },
    );
  if (reports.length)
    costDetails.push({
      label: (HARNESS[session.harness] ?? session.harness) + "'s own figure",
      value: reported.length
        ? money(reported.reduce((sum, row) => sum + row.cost_usd!, 0)) +
          (reported.length === 1 ? ', last run' : ', last ' + reported.length + ' runs')
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
            id: catalogSessionKey(session),
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
          .find((row) => row.ok === false && row.reported_usd != null && row.computed_usd != null);
        if (!check) return undefined;
        const difference =
          check.reported_usd === 0
            ? 100
            : (Math.abs(check.computed_usd! - check.reported_usd!) /
                Math.abs(check.reported_usd!)) *
              100;
        return (
          "Semon's estimate for that run is " +
          Math.round(difference) +
          '% ' +
          (check.computed_usd! > check.reported_usd! ? 'above' : 'below') +
          ' ' +
          (HARNESS[session.harness] ?? session.harness) +
          "'s figure: API rates differ from what a plan is charged."
        );
      })(),
      runs: catalogSessionGroups(
        [...familyRuns].sort((a, b) => (b.last ?? 0) - (a.last ?? 0)),
      ).flatMap(({ root, children }) => {
        const depths = new Map([[root.key, 0]]);
        return [root, ...children].map((run) => {
          const depth = run === root ? 0 : (depths.get(run.parent ?? '') ?? -1) + 1,
            state = run.status?.state ?? 'off',
            stateLabel =
              run.status?.state == null
                ? 'Status unavailable'
                : (STATE[state] ?? 'Recorded history'),
            cost = money(run.cost.usd);
          depths.set(run.key, depth);
          return {
            id: run.key,
            depth,
            label: 'Open ' + [run.name, run.kind, stateLabel, cost].join(', '),
            name: run.name,
            kind: run.kind,
            state,
            stateLabel,
            cost,
          };
        });
      }),
      models,
      includesRuns: !!metadata?.family?.runs.length,
    },
  };
}

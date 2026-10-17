import { array, boolean, dictionary, number, object, text } from './validate';
import type { Cost, TokenKind, Usage } from './types';

export interface SessionDetails {
  state: string;
  role?: boolean | null;
  cwd?: string | null;
  family?: {
    state: 'complete' | 'incomplete';
    ancestors: SessionRelative[];
    runs: SessionRelative[];
  };
  turns: number;
  calls: number | null;
  errors: number | null;
  tool_calls: Record<string, number>;
  signals: Record<string, number>;
  cost: Cost;
  copied_usage: boolean;
  live_observation?: boolean;
  activity?: { tool: string; since: number };
  copilot: {
    version: string;
    schema: number;
    lifecycle: string | null;
    logical_parent: string | null;
    approvals: string | null;
  } | null;
  reported_runs: { start: number; cost_usd?: number }[];
  cost_check: { start: number; computed_usd?: number; reported_usd?: number; ok?: boolean }[];
}
export interface SessionRelative {
  key: string;
  name: string;
  harness: string;
  kind: string;
  parent: string | null;
  last?: number | null;
  cost: Cost;
}
const optionalNumber = (value: unknown) => (value == null ? undefined : number(value));
const nullableText = (value: unknown) => (value == null ? null : text(value));
function count(value: unknown) {
  const n = number(value);
  if (!Number.isSafeInteger(n) || n < 0) throw Error('Invalid recorded session count');
  return n;
}
function usage(value: unknown): Usage {
  const row = object(value),
    result: Usage = {};
  for (const key of [
    'input',
    'output',
    'cache_write',
    'cache_write_5m',
    'cache_write_1h',
    'cache_read',
    'web_search',
  ] as TokenKind[]) {
    if (row[key] == null) continue;
    const n = number(row[key]);
    if (n < 0) throw Error('Invalid recorded usage');
    result[key] = n;
  }
  return result;
}
function parseCost(value: unknown): Cost {
  const cost = object(value);
  const models = dictionary(cost.by_model ?? {}, (value) => {
    const model = object(value);
    return {
      usd: model.usd == null ? null : number(model.usd),
      tokens: usage(model.tokens ?? {}),
      usd_by_kind: usage(model.usd_by_kind ?? {}),
    };
  });
  if (Object.keys(models).length > 256) throw Error('Recorded model breakdown exceeds its bound');
  return {
    usd: cost.usd == null ? null : number(cost.usd),
    unpriced_models: array(cost.unpriced_models ?? [], text),
    by_model: models,
  };
}
function parseRelative(value: unknown): SessionRelative {
  const row = object(value);
  return {
    key: text(row.key),
    name: text(row.name),
    harness: text(row.harness),
    kind: text(row.kind),
    parent: nullableText(row.parent),
    last: row.last == null ? null : number(row.last),
    cost: parseCost(row.cost),
  };
}
export function parseSessionDetails(value: unknown): SessionDetails {
  const row = object(value),
    state = text(row.state);
  if (!['work', 'wait', 'idle', 'done', 'err', 'off'].includes(state))
    throw Error('Invalid recorded session state');
  const family = row.family == null ? null : object(row.family);
  if (
    family &&
    ((family.state !== 'complete' && family.state !== 'incomplete') ||
      !Array.isArray(family.ancestors) ||
      family.ancestors.length > 32 ||
      !Array.isArray(family.runs) ||
      family.runs.length > 99)
  )
    throw Error('Recorded family exceeds its bound');
  const copilot = row.copilot == null ? null : object(row.copilot);
  const activity = row.activity == null ? null : object(row.activity);
  if (activity && text(activity.tool).length > 256) throw Error('Invalid recorded activity tool');
  const ancestors = family ? array(family.ancestors, parseRelative) : [],
    runs = family ? array(family.runs, parseRelative) : [],
    keys = [...ancestors, ...runs].map((row) => row.key);
  if (new Set(keys).size !== keys.length) throw Error('Invalid recorded family identity');
  return {
    state,
    role: row.role == null ? null : boolean(row.role),
    turns: count(row.turns),
    calls: row.calls == null ? null : count(row.calls),
    errors: row.errors == null ? null : count(row.errors),
    tool_calls: dictionary(row.tool_calls, count),
    signals: dictionary(row.signals, count),
    cost: parseCost(row.cost),
    cwd: nullableText(row.cwd),
    family: family
      ? { state: family.state as 'complete' | 'incomplete', ancestors, runs }
      : undefined,
    copied_usage: boolean(row.copied_usage),
    live_observation: row.live_observation == null ? undefined : boolean(row.live_observation),
    activity: activity ? { tool: text(activity.tool), since: count(activity.since) } : undefined,
    copilot: copilot
      ? {
          version: text(copilot.version),
          schema: count(copilot.schema),
          lifecycle: nullableText(copilot.lifecycle),
          logical_parent: nullableText(copilot.logical_parent),
          approvals: nullableText(copilot.approvals),
        }
      : null,
    reported_runs: array(row.reported_runs, (value) => {
      const item = object(value);
      return { start: number(item.start), cost_usd: optionalNumber(item.cost_usd) };
    }),
    cost_check: array(row.cost_check, (value) => {
      const item = object(value);
      return {
        start: number(item.start),
        computed_usd: optionalNumber(item.computed_usd),
        reported_usd: optionalNumber(item.reported_usd),
        ok: item.ok == null ? undefined : boolean(item.ok),
      };
    }),
  };
}

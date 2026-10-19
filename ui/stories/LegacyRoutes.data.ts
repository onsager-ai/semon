import retained from '../../tests/ui/fixture-data/overhaul.json';
import type { Cost, Entry, Handoff, Session, Turn, Usage } from '../src/domain/types';
import type { CatalogSession } from '../src/state/catalog-wire';
import type {
  CatalogTranscriptEntry,
  CatalogTranscriptPage,
} from '../src/state/catalog-transcript-wire';
import type { SessionSnapshot } from '../src/lib/transcript';
import type {
  HomeSnapshot,
  InboxRow,
  LiveRow,
  MachineSnapshot,
  MachinesSnapshot,
} from '../src/lib/screens';
import type { RecentItem } from '../src/lib/recent';
import type { CausalTrace } from '../src/app/tracePresentation';
import {
  catalogTranscriptBlocks,
  recordedRelationshipViews,
} from '../src/app/catalogTranscriptView';
import { catalogSessionRow, selectedSessionFooter } from '../src/app/sessionPresentation';
import { HARNESSES, I, STATE } from '../src/app/registry';
import { ago, shortModel } from '../src/domain/format';

// This is a presentation adapter over the retained synthetic sample, not a
// second fixture corpus. Its source date/revision is owned by fixture.mjs.
export const fixtureSource = 'tests/ui/fixture-data/overhaul.json';
export const fixtureRevision = 'bf9190ef2b80fcd56726f61dc480346b698039a2';
export const fixtureNow = Date.UTC(2026, 8, 28) + retained.NOW * 60000;
const at = (minutes: number) => Date.UTC(2026, 8, 28) + minutes * 60000;
export type FixtureScope = 'main' | 'two-machines' | 'pushed';
type SampleSession = Omit<Session, 'state'> & {
  state: string;
  tokensByModel?: Record<
    string,
    { input: number; output: number; cacheRead: number; cacheWrite: number; cacheWrite1h: number }
  >;
};
const sample = retained as unknown as {
  SESS: Record<string, SampleSession>;
  H: (Handoff & { t: string })[];
  TX: Record<string, Entry[]>;
  MACHINE: Record<string, string>;
  MACHINE_UP: Record<string, boolean>;
};
export const sessions: Record<string, Session> = Object.fromEntries(
  Object.entries(sample.SESS).map(([id, row]) => [
    id,
    {
      ...row,
      id,
      start: at(row.start),
      last: at(row.last),
      state: row.state === 'off' ? 'idle' : (row.state as Session['state']),
      liveObservation: true,
      busy: row.busy?.map(([a, b]) => [at(a), at(b)] as [number, number]),
    },
  ]),
);
export const edges: Handoff[] = sample.H.map((edge) => ({
  ...edge,
  at: at(edge.at),
  done: edge.done === undefined ? undefined : at(edge.done),
}));
export function recordedCost(id: string): Cost {
  const models = sample.SESS[id]?.tokensByModel ?? {};
  const by_model: NonNullable<Cost['by_model']> = {},
    unpriced_models: string[] = [];
  const prices = retained.API_PRICE as Record<
    string,
    { input: number; output: number; cacheRead: number; cacheWrite: number }
  >;
  for (const [model, values] of Object.entries(models)) {
    const price = prices[model];
    const tokens: Usage = {
      input: values.input,
      output: values.output,
      cache_read: values.cacheRead,
      cache_write_5m: Math.max(0, values.cacheWrite - values.cacheWrite1h),
      cache_write_1h: values.cacheWrite1h,
    };
    if (!price) {
      unpriced_models.push(model);
      by_model[model] = { usd: null, tokens };
      continue;
    }
    const usd_by_kind: Usage = {
      input: (values.input * price.input) / 1000000,
      output: (values.output * price.output) / 1000000,
      cache_read: (values.cacheRead * price.cacheRead) / 1000000,
      cache_write_5m: ((tokens.cache_write_5m ?? 0) * price.cacheWrite) / 1000000,
      cache_write_1h: (values.cacheWrite1h * price.input * 2) / 1000000,
    };
    by_model[model] = {
      usd: Object.values(usd_by_kind).reduce((sum, value) => sum + value, 0),
      tokens,
      usd_by_kind,
    };
  }
  return {
    usd:
      !Object.keys(models).length || unpriced_models.length
        ? null
        : Object.values(by_model).reduce((sum, row) => sum + (row.usd ?? 0), 0),
    unpriced_models,
    by_model,
  };
}
export function recordedCounts(id: string) {
  const entries = sample.TX[id] ?? [],
    tools = entries.filter((entry) => entry.k === 'tool');
  const outgoing = edges.filter(
    (edge) => edge.from === id && (edge.kind === 'spawn' || edge.kind === 'relay'),
  );
  return {
    tools: tools.length + outgoing.length + (id === 'quill' ? 1 : 0),
    errors:
      tools.filter((entry) => entry.k === 'tool' && entry.ok === false).length +
      outgoing.filter((edge) => edge.kind === 'spawn' && edge.status === 'err').length,
    turns: Math.max(
      1,
      entries.filter(
        (entry) =>
          entry.k === 'u' ||
          (entry.k === 'h' &&
            edges.some((edge) => edge.id === entry.id && edge.to === id && edge.from !== id)),
      ).length,
    ),
  };
}
export function mark(id: string) {
  const harness = HARNESSES[id];
  if (!harness) return undefined;
  return {
    id,
    name: harness.name,
    light: harness.icon.light,
    dark: harness.icon.dark,
    darkTheme: matchMedia('(prefers-color-scheme: dark)').matches,
  };
}
export function metadata(id: string): CatalogSession {
  const row = sessions[id];
  if (!row) throw new Error('Unknown retained session ' + id);
  const tools = (sample.TX[id] ?? []).filter((entry) => entry.k === 'tool');
  const toolCalls: Record<string, number> = {};
  for (const entry of tools)
    if (entry.k === 'tool') toolCalls[entry.name] = (toolCalls[entry.name] ?? 0) + 1;
  const parent = edges.find((edge) => edge.kind === 'spawn' && edge.to === id)?.from ?? null;
  const cost = recordedCost(id),
    counts = recordedCounts(id);
  const relative = (key: string) => ({
    key,
    name: sessions[key].name,
    harness: sessions[key].harness,
    kind: sessions[key].kind ?? 'interactive',
    parent: edges.find((edge) => edge.kind === 'spawn' && edge.to === key)?.from ?? null,
    last: sessions[key].last,
    status: { state: sessions[key].state, observation: 'current' as const },
    cost: recordedCost(key),
  });
  const ancestors = [];
  for (
    let key = parent;
    key;
    key = edges.find((edge) => edge.kind === 'spawn' && edge.to === key)?.from ?? null
  )
    ancestors.unshift(relative(key));
  const descendants = new Set<string>();
  const gather = (key: string) => {
    for (const edge of edges.filter((edge) => edge.kind === 'spawn' && edge.from === key))
      if (!descendants.has(edge.to)) {
        descendants.add(edge.to);
        gather(edge.to);
      }
  };
  gather(id);
  return {
    key: id,
    name: row.name,
    harness: row.harness,
    kind: parent
      ? row.harness === 'codex'
        ? 'codex-run'
        : 'subagent'
      : row.role
        ? 'role'
        : 'interactive',
    presentation_kind: row.kind,
    lifecycle: 'current',
    native_ids: [id],
    parent,
    parent_source: parent ? 'laptop' : null,
    machine: { key: 'laptop', label: 'laptop' },
    repo: row.repo ?? null,
    branch: row.branch ?? null,
    model: row.model ?? '',
    effort: row.effort ?? null,
    start: row.start,
    last: row.last,
    tokens: row.tokens ?? [],
    cost: { usd: cost.usd, unpriced_models: cost.unpriced_models ?? [] },
    source_refs: [],
    freshness: { state: 'cached' },
    status: { state: row.state, observation: 'current' },
    details: {
      state: row.state,
      role: row.role,
      live_observation: true,
      cwd: row.cwd ?? (row.repo ? '~/work/' + row.repo : null),
      family: { state: 'complete', ancestors, runs: [...descendants].map(relative) },
      turns: counts.turns,
      calls: counts.tools,
      errors: counts.errors,
      tool_calls: toolCalls,
      signals: {},
      cost,
      copied_usage: false,
      copilot: null,
      reported_runs: [],
      cost_check: [],
    },
  };
}
function transcriptEntries(id: string): CatalogTranscriptEntry[] {
  let turn = id + ':initial';
  // fixture.mjs records Principal receiving Sentinel's peer message before the
  // later human ask; the compact historical TX block omits that earlier turn.
  const source: Entry[] = [
    ...(id === 'principal' ? [{ k: 'h' as const, id: 'h9' }] : []),
    ...(sample.TX[id] ?? []),
  ];
  return source.map((entry, slot) => {
    if (entry.k === 'h') {
      const edge = edges.find((row) => row.id === entry.id);
      if (edge && edge.to === id && edge.from !== id) turn = edge.id;
    }
    return {
      entry_id: id + ':' + slot,
      slot,
      entry: { ...entry, tid: entry.k === 'tool' ? id + ':tool:' + slot : undefined, turn },
      provenance: null,
    };
  });
}
export function transcript(id: string): SessionSnapshot {
  const meta = metadata(id),
    entries = transcriptEntries(id);
  const relationships = edges.map((edge) => ({ edge, sourceTurn: edge.id, targetTurn: edge.id }));
  const context: CatalogTranscriptPage['relationship_context'] = {
    state: 'complete',
    turn_ids: [...new Set(entries.map((item) => item.entry.turn!))],
    turns: [...new Set(entries.map((item) => item.entry.turn!))].map((key) => ({
      id: key,
      start: edges.some((edge) => edge.id === key) ? key : undefined,
      u: edges.some((edge) => edge.id === key && edge.kind === 'ask'),
      end: { st: sessions[id].state, why: '' },
      sent: edges.filter((edge) => edge.from === id).map((edge) => edge.id),
      last: true,
    })),
    handoffs: relationships,
    sessions: Object.fromEntries(
      Object.entries(sessions).map(([key, row]) => [
        key,
        { ...row, model: row.model ?? '', live_observation: true },
      ]),
    ),
    previews: {},
  };
  const blocks = catalogTranscriptBlocks(
    entries,
    undefined,
    (item) => {
      if (item.entry.k !== 'h') return null;
      const relationship = relationships.find(
        (row) => row.edge.id === (item.entry as Extract<Entry, { k: 'h' }>).id,
      );
      return relationship
        ? recordedRelationshipViews(item, relationship, id, context.sessions, {
            brief: relationship.edge.brief,
            result: relationship.edge.result,
            trace: relationship.targetTurn,
          })
        : null;
    },
    true,
    context,
    undefined,
    meta.harness,
    id,
  );
  const origin = edges.find((edge) => edge.kind === 'spawn' && edge.to === id);
  return {
    id,
    name: meta.name,
    blocks,
    order: blocks.map((block) => (block.kind === 'turn' ? block.turn.id : block.key)),
    started: { lead: 'Started', machine: sessions[id].machine },
    footer: selectedSessionFooter(
      meta,
      origin
        ? {
            id: origin.from,
            name: sessions[origin.from].name,
            status: origin.status,
            done: origin.done,
            turn: origin.id,
          }
        : undefined,
    ),
  };
}
export function causalTrace(id: string, rootId: string): CausalTrace {
  const turns: Record<string, Turn[]> = {};
  const starts = new Map<string, Turn>();
  for (const [key, row] of Object.entries(sessions)) {
    const entries = transcriptEntries(key);
    const incoming = edges
      .filter(
        (edge) =>
          edge.to === key &&
          edge.kind !== 'move' &&
          entries.some((item) => item.entry.k === 'h' && item.entry.id === edge.id),
      )
      .sort((a, b) => a.at - b.at);
    turns[key] = (incoming.length ? incoming : [null]).map((edge, index) => {
      const user = entries.find((item) => item.entry.k === 'u')?.entry;
      const sent = edges.filter(
        (item) =>
          item.from === key &&
          item.at >= (edge?.at ?? row.start) &&
          item.at < (incoming[index + 1]?.at ?? Infinity),
      );
      const turn: Turn = {
        id: edge?.id ?? key + '_0',
        sid: key,
        start: edge,
        at: edge?.at ?? row.start,
        u: !edge && user?.k === 'u' ? { k: 'u', text: user.text } : null,
        entries: entries
          .filter((item) => item.entry.turn === (edge?.id ?? key + ':initial'))
          .map((item) => item.entry),
        out: sent,
        sent,
        end: { st: row.state, why: '', at: row.last },
        last: true,
      };
      if (edge) starts.set(edge.id, turn);
      return turn;
    });
  }
  const root = turns[id]?.find((turn) => turn.id === rootId);
  if (!root) throw new Error('Unknown retained trace root ' + id + '/' + rootId);
  return {
    source: 'laptop',
    key: id,
    root,
    generation: fixtureRevision,
    sessions: Object.fromEntries(
      Object.entries(sessions).map(([key, row]) => [key, { ...row, cost: recordedCost(key) }]),
    ),
    turns,
    starts,
    handoffs: new Map(edges.map((edge) => [edge.id, edge])),
    participants: {},
    participantSummaries: {},
    frontier: [],
    incomplete: false,
    previewsIncomplete: false,
    previews: {},
    previewOwners: new Map(),
  };
}
export function liveRow(id: string): LiveRow {
  const row = sessions[id];
  return {
    id,
    name: row.name,
    state: row.state,
    stateLabel: STATE[row.state],
    status: STATE[row.state],
    detail: [row.repo, row.branch, row.model].filter(Boolean).join(' · '),
    age: ago(row.last, fixtureNow),
    harness: mark(row.harness),
    activity: row.activity ? [row.activity[0], row.activity[1], row.activity[2]] : undefined,
  };
}
function inboxRow(edge: Handoff): InboxRow {
  const row = sessions[edge.from];
  return {
    id: edge.id,
    quiet: edge.status !== 'wait',
    icon: I[edge.kind === 'move' ? 'move' : 'result'],
    parts: [{ className: 'who', text: row?.name ?? edge.from }],
    age: ago(edge.at, fixtureNow),
    preview: edge.brief,
    context: row?.repo ?? '',
    harness: mark(row?.harness ?? ''),
    trace: edge.id,
  };
}
export function home(): HomeSnapshot {
  const inbox = edges.filter((edge) => edge.kind === 'toyou' && edge.status === 'wait');
  const answered = edges.filter((edge) => edge.kind === 'toyou' && edge.status === 'done');
  const live = Object.keys(sessions).filter((id) => sessions[id].state === 'work');
  return {
    waiting: inbox.length,
    working: live.length,
    up: 1,
    machines: 1,
    inbox: inbox.map(inboxRow),
    live: live.map(liveRow),
    answered: answered.map(inboxRow),
    totalAnswered: answered.length,
    allAnswered: false,
  };
}
// The second machine's exact names, IDs and task text are authored by
// fixture.mjs writeSecond (not renamed versions of laptop sessions).
const desktopRows: LiveRow[] = [
  {
    id: '5e1c0a2e-0d1c-4b7e-9a51-2f6c1d0e7a02',
    name: 'Release build',
    state: 'work',
    stateLabel: 'Working',
    status: 'Working',
    detail: 'foundry · main · sonnet-5',
    harness: undefined,
  },
  {
    id: '5e1c0a2e-0d1c-4b7e-9a51-2f6c1d0e7a01',
    name: 'Nightly review',
    state: 'idle',
    stateLabel: 'Idle',
    status: 'Idle',
    detail: 'foundry · main · sonnet-5',
    harness: undefined,
  },
];
export function machine(id: string, scope: FixtureScope): MachineSnapshot {
  if (id === 'desktop') {
    if (scope !== 'two-machines')
      throw new Error('Desktop is only present in the two-machines fixture');
    return {
      name: 'desktop',
      sessions: desktopRows.map((row) => ({ ...row, harness: mark('claude') })),
      totalSessions: 2,
      off: [],
      moves: [],
    };
  }
  if (id !== 'laptop') throw new Error('Unknown fixture machine ' + id);
  const keys = Object.keys(sessions).filter((key) => sessions[key].machine === 'laptop');
  return {
    name: 'laptop',
    sessions: keys.map(liveRow),
    totalSessions: keys.length,
    off: Object.keys(sessions)
      .filter((key) => sessions[key].movedFrom === id)
      .map(liveRow),
    moves: edges.filter((edge) => edge.kind === 'move').map(inboxRow),
  };
}
export function machines(scope: FixtureScope): MachinesSnapshot {
  const rows = [
    {
      id: 'laptop',
      name: 'laptop',
      state: 'work',
      stateLabel: 'Up',
      status: 'Up',
      detail: Object.keys(sessions).length + ' sessions',
    },
  ];
  if (scope === 'two-machines')
    rows.push({
      id: 'desktop',
      name: 'desktop',
      state: 'work',
      stateLabel: 'Up',
      status: 'Up',
      detail: '2 sessions',
    });
  return {
    up: rows.length,
    working:
      Object.values(sessions).filter((row) => row.state === 'work').length +
      (scope === 'two-machines' ? 1 : 0),
    rows,
    admin: null,
  };
}
export function recentItems(current?: string): RecentItem[] {
  const rows = Object.keys(sessions).sort((a, b) => sessions[b].last - sessions[a].last);
  function item(id: string, depth: number): RecentItem {
    const row = sessions[id],
      presentation = catalogSessionRow(metadata(id));
    const children = rows.filter((key) => metadata(key).parent === id);
    return {
      ...presentation,
      model: shortModel(row.model ?? ''),
      label: row.name,
      rail: false,
      open: true,
      depth,
      current: id === current ? 'page' : undefined,
      children: children.map((key) => item(key, depth + 1)),
    };
  }
  return rows.filter((id) => !metadata(id).parent).map((id) => item(id, 0));
}
export function catalogItems() {
  return Object.keys(sessions)
    .sort((a, b) => sessions[b].last - sessions[a].last)
    .map((id) => {
      const row = metadata(id);
      return {
        key: id,
        name: row.name,
        harness: row.harness,
        repo: row.repo,
        branch: row.branch,
        model: row.model,
        freshness: row.freshness,
        presentation: catalogSessionRow(row),
      };
    });
}

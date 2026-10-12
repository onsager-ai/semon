import { array, boolean, dictionary, number, object, text } from '../domain/validate';
import { createTraceCalculations } from '../domain/trace';
import type { Cost, Handoff, Session, SessionState, Turn } from '../domain/types';
import { clock, shortModel } from '../domain/format';
import { renderTraceScreen, type Hop } from '../lib/trace';
import { HARNESSES, HARNESS, I, STATE } from './registry';
import { parseRecordedRelationship } from '../domain/relationships';
import type { CatalogSession } from '../state/catalog-wire';

export interface CausalTrace {
  source: string;
  key: string;
  root: Turn;
  generation: string;
  sessions: Record<string, Session>;
  turns: Record<string, Turn[]>;
  starts: Map<string, Turn>;
  handoffs: Map<string, Handoff>;
  frontier: { catalog_key: string; turn: string }[];
  incomplete: boolean;
  previewsIncomplete: boolean;
  previews: Record<string, string>;
  previewOwners: Map<string, { brief?: string; result?: string }>;
  previewReady?: Set<string>;
  observation?: string;
  metadata?: CatalogSession;
}
const optionalText = (value: unknown) => (value == null ? undefined : text(value));
const optionalNumber = (value: unknown) => (value == null ? undefined : number(value));
function state(value: unknown): SessionState {
  if (value === 'off') return 'idle';
  if (
    value !== 'work' &&
    value !== 'wait' &&
    value !== 'idle' &&
    value !== 'done' &&
    value !== 'err'
  )
    throw Error('Invalid recorded state');
  return value;
}
function parseCost(value: unknown): Cost {
  const cost = object(value);
  return {
    usd: cost.usd == null ? null : number(cost.usd),
    unpriced_models: cost.unpriced_models == null ? [] : array(cost.unpriced_models, text),
  };
}
export function parseCausalTrace(
  value: unknown,
  source: string,
  key: string,
  root: string,
): CausalTrace {
  const data = object(value);
  if (
    data.api !== 1 ||
    data.source_key !== source ||
    data.catalog_key !== key ||
    data.root !== root
  )
    throw Error('Causal selection changed');
  const previews = dictionary(data.previews, text);
  if (
    Object.keys(previews).length > 128 ||
    Object.values(previews).some((generation) => !/^[a-f0-9]{64}$/.test(generation))
  )
    throw Error('Invalid causal preview scope');
  const sessions = dictionary(data.sessions, (value) => {
    const row = object(value);
    const session: Session = {
      id: '',
      name: text(row.name),
      harness: text(row.harness),
      state: state(row.state),
      liveObservation: row.live_observation == null ? false : boolean(row.live_observation),
      machine: source,
      model: text(row.model),
      kind: optionalText(row.kind),
      start: number(row.start),
      last: number(row.last),
      cost: parseCost(row.cost),
      tokens: array(row.tokens, number),
      busy: array(row.busy, (value) => {
        const span = array(value, number);
        if (span.length !== 2) throw Error('Invalid busy interval');
        return [span[0], span[1]] as [number, number];
      }),
      wait_edges: array(row.wait_edges, (value) => {
        const edge = object(value);
        return {
          call: text(edge.call),
          tool: text(edge.tool),
          targets: array(edge.targets, text),
          start: number(edge.start),
          end: optionalNumber(edge.end),
          turn: optionalText(edge.turn),
        };
      }),
      wait_edges_truncated: boolean(row.wait_edges_truncated),
    };
    return session;
  });
  for (const [id, session] of Object.entries(sessions)) {
    session.id = id;
  }
  const handoffs = new Map<string, Handoff>();
  const previewOwners = new Map<string, { brief?: string; result?: string }>();
  for (const value of array(data.handoffs, object)) {
    const relationship = parseRecordedRelationship(value),
      edge = relationship.edge;
    if (edge.kind === 'move') {
      edge.fromMachine = source;
      edge.toMachine = source;
    }
    if (handoffs.has(edge.id)) throw Error('Duplicate causal handoff');
    handoffs.set(edge.id, edge);
    previewOwners.set(edge.id, {
      brief: relationship.briefOwner,
      result: relationship.resultOwner,
    });
  }
  const turns: Record<string, Turn[]> = {},
    starts = new Map<string, Turn>(),
    byId = new Map<string, Turn>();
  for (const value of array(data.turns, object)) {
    const end = object(value.end),
      sid = text(value.sid),
      id = text(value.id),
      start = optionalText(value.start);
    if (!sessions[sid] || byId.has(id)) throw Error('Invalid causal turn identity');
    const sent = array(value.sent, text).map((id) => {
      const edge = handoffs.get(id);
      if (!edge) throw Error('Missing causal handoff');
      return edge;
    });
    const turn: Turn = {
      id,
      sid,
      start: start ? (handoffs.get(start) ?? null) : null,
      at: optionalNumber(value.at),
      u: boolean(value.u) ? { k: 'u', text: '' } : null,
      entries: [],
      out: sent,
      sent,
      end: { st: state(end.st), why: text(end.why), at: optionalNumber(end.at) },
      last: boolean(value.last),
    };
    byId.set(id, turn);
    (turns[sid] ??= []).push(turn);
    if (start) starts.set(start, turn);
  }
  const selected = byId.get(root);
  if (
    !selected ||
    selected.sid !== key ||
    byId.size > 100 ||
    handoffs.size > 1000 ||
    Object.keys(sessions).length > 100
  )
    throw Error('Causal response exceeds its selection or bound');
  const frontier = array(data.frontier, (value) => {
    const row = object(value);
    return { catalog_key: text(row.catalog_key), turn: text(row.turn) };
  });
  if (frontier.length > 1000) throw Error('Causal frontier exceeds its bound');
  return {
    source,
    key,
    root: selected,
    sessions,
    turns,
    starts,
    handoffs,
    generation: text(data.generation),
    frontier,
    incomplete: boolean(data.incomplete),
    previewsIncomplete: boolean(data.previews_incomplete),
    previews,
    previewOwners,
  };
}

/** Related names come from the selected, bounded family observation, never a workspace lookup. */
export function causalSessionPart(
  data: CausalTrace,
  id: string,
  turn?: string,
): Hop['parts'][number] {
  const own = data.sessions[id],
    family = data.metadata?.details?.family,
    relatives = [...(family?.ancestors ?? []), ...(family?.runs ?? [])].filter(
      (relative) => relative.key === id,
    ),
    related = relatives.length === 1 ? relatives[0] : undefined;
  return {
    className: 'who',
    text: id === 'you' ? 'You' : (own?.name ?? related?.name ?? 'Unrecorded session'),
    action: id !== 'you' && (own || related) ? { kind: 'session', id, turn } : undefined,
  };
}

/** Existing rail/chart presentation, fed by one bounded source query. */
export function renderCausalTrace(
  root: HTMLElement,
  data: CausalTrace,
  now: number,
  expanded: Map<string, Set<string>>,
  host: { session(id: string, turn?: string): void; fold(): void },
) {
  const costForSessions = (sessions: Session[]): Cost => ({
    usd: sessions.some((session) => session.cost?.usd == null)
      ? null
      : sessions.reduce((sum, session) => sum + session.cost!.usd!, 0),
    unpriced_models: [
      ...new Set(sessions.flatMap((session) => session.cost?.unpriced_models ?? [])),
    ],
  });
  const costText = (cost: Cost) =>
    cost.usd == null ? 'Cost unavailable' : '$' + cost.usd.toFixed(2);
  const { agentSnapshot } = createTraceCalculations(
    data,
    {
      costForSessions,
      costForSession: (id) => data.sessions[id]?.cost ?? { usd: null },
      costText,
      shortMoney: (value) => '$' + value.toFixed(2),
      hcls: (id) => data.sessions[id]?.harness ?? 'you',
      usageTotal: (session) =>
        (session.tokens ?? []).reduce((sum, value) => sum + value, 0) * 1000000,
      nameOf: (id) => causalSessionPart(data, id).text,
    },
    () => now,
    expanded,
    HARNESS,
    STATE,
  );
  const hops: Hop[] = [];
  const seen = new Set<string>();
  const sessionPart = (id: string, turn?: string) => causalSessionPart(data, id, turn);
  const harness = (sid: string) => {
    const known = HARNESSES[data.sessions[sid]?.harness];
    return known
      ? {
          name: known.name,
          light: known.icon.light,
          dark: known.icon.dark,
          id: data.sessions[sid].harness,
          darkTheme: matchMedia('(prefers-color-scheme: dark)').matches,
        }
      : undefined;
  };
  const endLabel = (turn: Turn) => {
    if (turn.end?.why === 'toyou') {
      const edge = [...turn.sent].reverse().find((edge) => edge.kind === 'toyou');
      if (edge?.kind !== 'toyou') return 'Recorded turn';
      if (edge.ask === 'result') return 'Sent you a result';
      if (edge.status === 'wait') return 'Waiting on you';
      if (edge.declined) return 'You declined';
      return edge.answer?.length ? 'You answered' : 'Asked you';
    }
    const labels: Record<string, string> = {
      permission: 'Waiting for permission',
      input: 'Waiting for your input',
      working: 'Working',
      failed: 'Failed',
      failed_step: 'A step failed',
      unfinished_step: 'Step result not recorded',
      handoff_failed: 'Handoff failed',
      returned: 'Returned',
      replied: 'Replied',
      no_reply: 'No reply recorded',
    };
    return labels[turn.end?.why ?? ''] ?? 'Recorded turn';
  };
  const meta = (turn: Turn) => ({
    state: turn.end?.st ?? 'done',
    stateLabel: STATE[turn.end?.st ?? 'done'] ?? 'Recorded',
    text: endLabel(turn),
    session: turn.sid,
    turn: turn.id,
    name: data.sessions[turn.sid]?.name,
    harness: harness(turn.sid),
    chip: shortModel(data.sessions[turn.sid]?.model),
  });
  hops.push({
    key: 'root:' + data.root.id,
    className: 'k-root',
    nodeClass: data.root.start ? (data.sessions[data.key]?.harness ?? 'you') : 'you',
    icon: data.root.start ? (I[data.root.start.kind] ?? I.ask) : I.ask,
    handoff: data.root.start?.id,
    turn: data.root.id,
    parts: [
      sessionPart(data.root.start?.from ?? (data.root.u ? 'you' : data.key)),
      {
        className: 'verb',
        text: data.root.start
          ? data.root.start.kind === 'spawn'
            ? ' spawned '
            : data.root.start.kind === 'relay'
              ? ' relayed to '
              : ' started a turn in '
          : ' started a turn in ',
      },
      sessionPart(data.key, data.root.id),
    ],
    time: clock(data.root.at ?? data.sessions[data.key].start, now),
    brief: data.root.u?.text || data.root.start?.brief || undefined,
    meta: meta(data.root),
  });
  const walk = (turn: Turn) => {
    if (seen.has(turn.id)) return;
    seen.add(turn.id);
    for (const edge of turn.sent) {
      const child = data.starts.get(edge.id);
      hops.push({
        key: edge.id,
        className: 'child k-' + edge.kind + ' s-' + edge.status,
        nodeClass: data.sessions[edge.to]?.harness ?? 'you',
        icon: I[edge.kind] ?? I.result,
        handoff: edge.id,
        turn: child?.id,
        parts: [
          sessionPart(edge.from, turn.id),
          {
            className: 'verb',
            text:
              edge.kind === 'spawn'
                ? ' spawned '
                : edge.kind === 'relay'
                  ? ' relayed to '
                  : edge.kind === 'toyou'
                    ? edge.ask === 'result'
                      ? ' sent a result to '
                      : ' asked '
                    : ' sent to ',
          },
          sessionPart(edge.to, child?.id),
        ],
        time: clock(edge.at, now),
        brief: edge.kind === 'toyou' && edge.ask === 'result' ? undefined : edge.brief || undefined,
        answers: edge.kind === 'toyou' && Array.isArray(edge.answer) ? edge.answer : undefined,
        result: edge.kind === 'toyou' && edge.ask === 'result' ? undefined : edge.result,
        meta: child
          ? meta(child)
          : {
              state: edge.status,
              stateLabel: STATE[edge.status] ?? edge.status,
              text: edge.declined
                ? 'Declined'
                : edge.status === 'done'
                  ? 'Completed'
                  : edge.status === 'wait'
                    ? 'Waiting on you'
                    : 'Recorded',
              note:
                edge.to !== 'you' && !child
                  ? 'Its turn is not in this source publication'
                  : undefined,
            },
      });
      if (child) walk(child);
    }
  };
  walk(data.root);
  renderTraceScreen(
    root,
    {
      empty: false,
      summary:
        `${Object.keys(data.sessions).length} sessions · ${data.handoffs.size} handoffs` +
        (data.incomplete ? ' · some relationship context is unavailable' : ''),
      hops,
      agents: agentSnapshot(data.root),
      observation: data.observation,
      continuations: data.frontier.map((branch) => ({
        label: data.sessions[branch.catalog_key]?.name ?? 'Recorded branch',
        href:
          '/trace/native/' +
          encodeURIComponent(branch.catalog_key) +
          '/' +
          encodeURIComponent(branch.turn) +
          '?' +
          new URLSearchParams({ machine: data.source }),
      })),
    },
    {
      ...host,
      machine() {},
      sender() {},
      committed() {},
      fold(id) {
        const values = expanded.get(data.root.id) ?? new Set<string>();
        values.add(id);
        expanded.set(data.root.id, values);
        host.fold();
      },
    },
  );
}

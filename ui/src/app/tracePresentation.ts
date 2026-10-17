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
  participants: Record<string, string>;
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
  const participants = dictionary(data.participants, text),
    related = new Set([...handoffs.values()].flatMap((edge) => [edge.from, edge.to]));
  if (
    Object.keys(participants).length > 128 ||
    Object.keys(participants).some(
      (id) => id === 'you' || Object.hasOwn(sessions, id) || !related.has(id),
    )
  )
    throw Error('Invalid causal participant scope');
  const turns: Record<string, Turn[]> = Object.create(null),
    starts = new Map<string, Turn>(),
    byId = new Map<string, Turn>();
  for (const value of array(data.turns, object)) {
    const end = object(value.end),
      sid = text(value.sid),
      id = text(value.id),
      start = optionalText(value.start);
    if (!Object.hasOwn(sessions, sid) || byId.has(id)) throw Error('Invalid causal turn identity');
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
      end: {
        st: state(end.st),
        why: text(end.why),
        h: optionalText(end.h),
        at: optionalNumber(end.at),
      },
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
    participants,
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

/** Names come from this branch's bounded participants or the selected family observation. */
export function causalSessionPart(
  data: CausalTrace,
  id: string,
  turn?: string,
): Hop['parts'][number] {
  const own = Object.hasOwn(data.sessions, id) ? data.sessions[id] : undefined,
    family = data.metadata?.details?.family,
    relatives = [...(family?.ancestors ?? []), ...(family?.runs ?? [])].filter(
      (relative) => relative.key === id,
    ),
    related = relatives.length === 1 ? relatives[0] : undefined,
    participant =
      data.participants && Object.hasOwn(data.participants, id) ? data.participants[id] : undefined;
  return {
    className: 'who',
    text:
      id === 'you' ? 'You' : (own?.name ?? participant ?? related?.name ?? 'Unrecorded session'),
    action:
      id !== 'you' && (own || participant !== undefined || related)
        ? { kind: 'session', id, turn }
        : undefined,
  };
}

export function reachableTraceSummary(root: Turn, starts: Map<string, Turn>, incomplete = false) {
  const visited = new Set<string>(),
    sessions = new Set([root.sid]);
  let handoffs = 0;
  const walk = (turn: Turn) => {
    if (visited.has(turn.id)) return;
    visited.add(turn.id);
    for (const edge of turn.sent) {
      if (edge.kind === 'move') continue;
      handoffs++;
      if (edge.kind === 'toyou') continue;
      sessions.add(edge.to);
      if (edge.kind === 'spawn' || edge.kind === 'relay') {
        const child = starts.get(edge.id);
        if (child) walk(child);
      }
    }
  };
  walk(root);
  return [
    `${sessions.size} ${sessions.size === 1 ? 'session' : 'sessions'}`,
    `${handoffs} ${handoffs === 1 ? 'handoff' : 'handoffs'}`,
    incomplete ? 'some relationship context is unavailable' : '',
  ]
    .filter(Boolean)
    .join(' · ');
}

export function traceRootActor(turn: Turn) {
  return turn.start?.from ?? (turn.u ? 'you' : turn.sid);
}

export function traceHopActor(edge: Handoff) {
  return edge.kind === 'toyou' ? edge.from : edge.to;
}

export function traceSentenceParts(
  edge: Handoff,
  session: (id: string, turn?: string) => Hop['parts'][number],
  starts: Map<string, Turn>,
  currentTurn?: string,
  recipientKind?: string,
): Hop['parts'] {
  const verb = (text: string): Hop['parts'][number] => ({ className: 'verb', text });
  if (edge.kind === 'ask')
    return [
      session('you'),
      verb(' asked '),
      session(edge.to, starts.get(edge.id)?.id ?? currentTurn),
    ];
  if (edge.kind === 'spawn' || edge.kind === 'relay') {
    const text =
      edge.kind === 'spawn'
        ? ' handed off to ' +
          (recipientKind === 'Subagent' ? 'subagent' : (recipientKind ?? '')) +
          ' '
        : ' relayed to ';
    return [session(edge.from, currentTurn), verb(text), session(edge.to, starts.get(edge.id)?.id)];
  }
  if (edge.kind === 'move')
    return [
      verb('Semon moved '),
      session(edge.to),
      verb(' from '),
      { className: 'verb mach', text: edge.fromMachine },
      verb(' to '),
      { className: 'verb mach', text: edge.toMachine },
    ];
  const text = {
    question: ' asked you',
    result: ' sent you a result',
    decision: ' needs your decision',
  }[edge.ask];
  return [session(edge.from, currentTurn), verb(text)];
}

export function traceHandoffOutcome(edge: Handoff) {
  if (edge.kind === 'move') return { state: 'done', text: 'Moved' };
  if (edge.kind === 'toyou' && edge.ask === 'result')
    return {
      state: edge.status,
      text: {
        new: 'new',
        work: 'working',
        wait: 'waiting on you',
        err: 'failed',
        done: 'delivered',
      }[edge.status],
    };
  return {
    state: edge.status,
    text: {
      new: undefined,
      work: 'working',
      wait: 'waiting on you',
      err: 'failed',
      done: edge.kind === 'toyou' ? 'answered' : edge.result ? 'returned' : 'delivered',
    }[edge.status],
  };
}

export function traceTurnOutcome(
  turn: Turn,
  handoffs: Map<string, Handoff>,
  nameOf: (id: string) => string,
  formatTime: (at: number) => string,
) {
  const end = turn.end,
    state = end?.st ?? 'idle',
    handoff = end?.h
      ? handoffs.get(end.h)
      : end?.why === 'toyou'
        ? turn.sent.filter((edge) => edge.kind === 'toyou').length === 1
          ? turn.sent.find((edge) => edge.kind === 'toyou')
          : undefined
        : undefined;
  const withTime = (label: string, at: number | undefined) =>
    at == null ? label : `${label} · ${formatTime(at)}`;
  if (end?.why === 'input') return { state: 'wait', text: 'Waiting for your input' };
  if (['permission', 'waiting', 'waiting_permission'].includes(end?.why ?? ''))
    return { state: 'wait', text: 'Waiting on permission' };
  if (end?.why === 'working') return { state: 'work', text: 'Still working' };
  if (end?.why === 'toyou' && handoff?.kind === 'toyou') {
    const outcome = traceHandoffOutcome(handoff),
      label = [edgeAskLabel(handoff), outcome.text].filter(Boolean).join(' · ');
    return {
      state: outcome.state,
      text: withTime(label, handoff.at),
    };
  }
  if (end?.why === 'failed') return { state: 'err', text: withTime('Failed', turn.start?.done) };
  if (end?.why === 'unfinished_step')
    return { state: 'err', text: 'Stopped on a step with no result' };
  if (end?.why === 'failed_step') return { state: 'err', text: 'Stopped on a failed step' };
  if (end?.why === 'handoff_failed' && handoff)
    return { state: 'err', text: `Handoff to ${nameOf(handoff.to)} failed` };
  if (end?.why === 'returned' && turn.start)
    return {
      state: 'done',
      text: withTime(`Returned to ${nameOf(turn.start.from)}`, turn.start.done),
    };
  if (end?.why === 'replied') return { state: 'done', text: 'Replied' };
  return { state, text: 'No reply in these logs' };
}

function edgeAskLabel(edge: Extract<Handoff, { kind: 'toyou' }>) {
  return {
    question: 'Asked you',
    result: 'Sent you a result',
    decision: 'Needs your decision',
  }[edge.ask];
}

export function traceCostText(cost: Cost) {
  return cost.usd == null || cost.unpriced_models?.length ? '—' : '$' + cost.usd.toFixed(2);
}

export function traceShortMoney(usd: number) {
  return '$' + usd.toFixed(1);
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
  const { agentSnapshot } = createTraceCalculations(
    data,
    {
      costForSessions,
      costForSession: (id) => data.sessions[id]?.cost ?? { usd: null },
      costText: traceCostText,
      shortMoney: traceShortMoney,
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
  const meta = (turn: Turn) => {
    const outcome = traceTurnOutcome(
      turn,
      data.handoffs,
      (id) => causalSessionPart(data, id).text,
      (at) => clock(at, now),
    );
    return {
      state: outcome.state,
      stateLabel: STATE[outcome.state] ?? 'Recorded',
      text: outcome.text,
      session: turn.sid,
      turn: turn.id,
      name: data.sessions[turn.sid]?.name,
      harness: harness(turn.sid),
      chip: shortModel(data.sessions[turn.sid]?.model),
    };
  };
  const rootParts: Hop['parts'] = data.root.start
    ? traceSentenceParts(
        data.root.start,
        sessionPart,
        data.starts,
        undefined,
        data.sessions[data.root.start.to]?.kind,
      )
    : data.root.u
      ? traceSentenceParts(
          {
            id: data.root.id,
            kind: 'ask',
            from: 'you',
            to: data.key,
            at: data.root.at ?? now,
            status: 'done',
            brief: '',
          },
          sessionPart,
          data.starts,
          data.root.id,
        )
      : [
          sessionPart(data.key),
          { className: 'verb', text: " · a turn whose start isn't in these logs" },
        ];
  hops.push({
    key: 'root:' + data.root.id,
    className: 'k-root',
    nodeClass: data.sessions[traceRootActor(data.root)]?.harness ?? 'you',
    icon: data.root.start ? (I[data.root.start.kind] ?? I.ask) : data.root.u ? I.ask : I.more,
    handoff: data.root.start?.id,
    turn: data.root.id,
    parts: rootParts,
    time: data.root.start
      ? clock(data.root.start.at, now)
      : data.root.u
        ? clock(data.root.at ?? now, now)
        : undefined,
    brief: data.root.u?.text || data.root.start?.brief || undefined,
    meta: meta(data.root),
  });
  const walk = (turn: Turn) => {
    if (seen.has(turn.id)) return;
    seen.add(turn.id);
    for (const edge of turn.sent) {
      const child = data.starts.get(edge.id),
        isResult = edge.kind === 'toyou' && edge.ask === 'result',
        edgeOutcome = traceHandoffOutcome(edge);
      hops.push({
        key: edge.id,
        className: 'child k-' + edge.kind + ' s-' + edge.status,
        nodeClass: data.sessions[traceHopActor(edge)]?.harness ?? 'you',
        icon: I[edge.kind] ?? I.result,
        handoff: edge.id,
        turn: child?.id,
        parts: isResult
          ? [{ className: 'verb', text: edgeOutcome.text ?? '' }]
          : traceSentenceParts(
              edge,
              sessionPart,
              data.starts,
              turn.id,
              data.sessions[edge.to]?.kind,
            ),
        time: clock(edge.at, now),
        brief: edge.kind === 'toyou' && edge.ask === 'result' ? undefined : edge.brief || undefined,
        answers: edge.kind === 'toyou' && Array.isArray(edge.answer) ? edge.answer : undefined,
        result: edge.kind === 'toyou' && edge.ask === 'result' ? undefined : edge.result,
        ...(isResult
          ? {}
          : {
              meta: child
                ? meta(child)
                : {
                    state: edgeOutcome.state,
                    stateLabel: STATE[edgeOutcome.state] ?? edgeOutcome.state,
                    text: edgeOutcome.text ?? '',
                    note:
                      (edge.kind === 'spawn' || edge.kind === 'relay') && !child
                        ? "Its turn isn't in these logs"
                        : undefined,
                  },
            }),
      });
      if (child) walk(child);
    }
  };
  walk(data.root);
  renderTraceScreen(
    root,
    {
      empty: false,
      summary: reachableTraceSummary(data.root, data.starts, data.incomplete),
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

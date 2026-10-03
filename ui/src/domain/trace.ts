import type { Session, Handoff, Turn } from './types';
import type { AgentRow, AgentChart } from '../lib/trace';
import type { createDomain } from './calculations';
import { clock as formatClock, dur as formatDuration, tok, shortModel } from './format';
interface AgentNode {
  sid: string;
  turn: Turn | null;
  handoff: Handoff | null;
  kind: 'root' | 'spawn' | 'relay';
  depth: number;
  children: AgentNode[];
}
interface AgentTree {
  rootNode: AgentNode;
  peers: AgentNode[];
  handoffs: Handoff[];
  spawns: Handoff[];
}
const finite = (value: number | undefined): value is number =>
  typeof value === 'number' && Number.isFinite(value);
export function createTraceCalculations(
  state: {
    sessions: Record<string, Session>;
    turns: Record<string, Turn[]>;
    starts: Map<string, Turn>;
  },
  domain: Pick<
    ReturnType<typeof createDomain>,
    | 'costForSessions'
    | 'costForSession'
    | 'costText'
    | 'shortMoney'
    | 'hcls'
    | 'usageTotal'
    | 'nameOf'
  >,
  now: () => number,
  RUN_EXPANDED: Map<string, Set<string>>,
  HARNESS: Record<string, string>,
  STATE: Record<string, string>,
) {
  const { sessions: SESS, turns: TURNS, starts: STARTS } = state;
  const { costForSessions, costForSession, costText, shortMoney, hcls, usageTotal, nameOf } =
    domain;
  const clock = (at: number) => formatClock(at, now()),
    dur = (from: number, to: number) => formatDuration(from, to, now()),
    kindText = (s: Session) => s.kind ?? HARNESS[s.harness];
  const allRunNodes = (model: AgentTree) => {
    const rows: AgentNode[] = [],
      walk = (n: AgentNode) => {
        rows.push(n);
        n.children.forEach(walk);
      };
    walk(model.rootNode);
    model.peers.forEach(walk);
    return rows;
  };
  // Walk only the turns connected by recorded spawns and relays; the relay rows remain peers in the view.
  function traceAgentTree(root: Turn): AgentTree {
    const rootNode: AgentNode = {
      sid: root.sid,
      turn: root,
      handoff: null,
      kind: 'root',
      depth: 0,
      children: [],
    };
    const peers: AgentNode[] = [],
      handoffs: Handoff[] = [],
      spawns: Handoff[] = [],
      seenTurns = new Set(),
      seenSessions = new Set([root.sid]);
    const walk = (node: AgentNode) => {
      const turn = node.turn;
      if (!turn || seenTurns.has(turn.id)) return;
      seenTurns.add(turn.id);
      const sent = [...(turn.sent ?? [])].sort((a, b) => a.at - b.at);
      handoffs.push(...sent);
      const children: AgentNode[] = [];
      for (const h of sent) {
        if (h.kind === 'spawn') {
          spawns.push(h);
          const childTurn = STARTS.get(h.id);
          if (!SESS[h.to] || seenSessions.has(h.to)) continue;
          seenSessions.add(h.to);
          const child: AgentNode = {
            sid: h.to,
            turn: childTurn ?? null,
            handoff: h,
            kind: 'spawn',
            depth: node.depth + 1,
            children: [],
          };
          node.children.push(child);
          children.push(child);
        } else if (h.kind === 'relay' && SESS[h.to] && !seenSessions.has(h.to)) {
          const peerTurn = STARTS.get(h.id);
          seenSessions.add(h.to);
          const peer: AgentNode = {
            sid: h.to,
            turn: peerTurn ?? null,
            handoff: h,
            kind: 'relay',
            depth: 0,
            children: [],
          };
          peers.push(peer);
          children.push(peer);
        }
      }
      for (const child of children) walk(child);
    };
    walk(rootNode);
    return { rootNode, peers, handoffs, spawns };
  }
  function agentSnapshot(root: Turn): AgentChart {
    const model = traceAgentTree(root),
      session = SESS[root.sid];
    const start = root.at ?? root.start?.at ?? session.start;
    const nextTurn = (TURNS[root.sid] ?? []).find((t) => (t.at ?? 0) > start);
    const upper =
      root.end?.at ?? nextTurn?.at ?? (session.state === 'work' ? now() : session.last) ?? start;
    const end = Math.max(
      start,
      upper,
      ...model.handoffs.map((h) => h.done).filter(finite),
      ...allRunNodes(model)
        .filter((n) => n.kind !== 'root')
        .map((n) =>
          SESS[n.sid]?.state === 'work' ? now() : (n.handoff?.done ?? SESS[n.sid]?.last),
        )
        .filter(finite),
    );
    const span = Math.max(1, end - start);
    const waits = allRunNodes(model).flatMap((node) =>
      (SESS[node.sid]?.wait_edges ?? [])
        .filter((w) => (!w.turn || w.turn === node.turn?.id) && w.start >= start && w.start <= end)
        .map((w) => ({ ...w, sid: node.sid })),
    );
    const critical = new Set(
      model.spawns
        .filter((h) => waits.some((w) => w.sid === h.from && w.targets?.includes(h.to)))
        .map((h) => h.id),
    );
    const pct = (m: number) => Math.max(0, Math.min(100, ((m - start) / span) * 100));
    const steps = [1, 2, 5, 10, 15, 30, 60, 120, 240]
      .map((n) => n * 60000)
      .map((step) => ({ step, count: Math.floor((span - 1) / step) }))
      .filter((x) => x.count >= 3 && x.count <= 6)
      .sort((a, b) => Math.abs(a.count - 5) - Math.abs(b.count - 5) || a.step - b.step);
    const step =
      steps[0]?.step ??
      [1, 2, 5, 10, 15, 30, 60, 120, 240]
        .map((n) => n * 60000)
        .sort(
          (a, b) =>
            Math.abs(Math.floor((span - 1) / a) - 5) - Math.abs(Math.floor((span - 1) / b) - 5),
        )[0];
    const ticks = [];
    for (let offset = step; offset < span; offset += step) ticks.push(offset);
    const rowCount = (node: AgentNode): number =>
      1 + node.children.reduce((n, child) => n + rowCount(child), 0);
    const allNodes = (node: AgentNode, into: AgentNode[] = []): AgentNode[] => {
      into.push(node);
      for (const child of node.children) allNodes(child, into);
      return into;
    };
    const totalRows =
        rowCount(model.rootNode) + model.peers.reduce((n, peer) => n + rowCount(peer), 0),
      foldRows = totalRows > 12;
    const spawned = model.spawns
      .filter((h) => Number.isFinite(h.at) && Number.isFinite(h.done) && h.done! > h.at)
      .map((h) => ({ start: h.at, end: h.done! }));
    const missingSpawnEnds = model.spawns.filter((h) => !Number.isFinite(h.done)).length;
    const points = spawned
      .flatMap((x) => [
        { at: x.start, change: 1 },
        { at: x.end, change: -1 },
      ])
      .sort((a, b) => a.at - b.at || a.change - b.change);
    let active = 0,
      peak = 0;
    for (const p of points) {
      active += p.change;
      peak = Math.max(peak, active);
    }
    const totalCost = costForSessions(
      allRunNodes(model)
        .map((n) => SESS[n.sid])
        .filter(Boolean),
    );
    const participantCount = allRunNodes(model).length;
    const summary = [
      participantCount + (participantCount === 1 ? ' agent' : ' agents'),
      dur(start, end),
      costText(totalCost) + ' session totals',
      ...(spawned.length ? ['Recorded spawn overlap: ' + peak] : []),
      ...(missingSpawnEnds
        ? [missingSpawnEnds + ' spawn end' + (missingSpawnEnds === 1 ? '' : 's') + ' not recorded']
        : []),
    ];
    while (RUN_EXPANDED.size > 16) RUN_EXPANDED.delete(RUN_EXPANDED.keys().next().value!);
    const expanded = RUN_EXPANDED.get(root.id) ?? new Set<string>();
    RUN_EXPANDED.set(root.id, expanded);
    const rows: AgentRow[] = [];
    const add = (node: AgentNode, depth: number, parent?: string): void => {
      const s = SESS[node.sid];
      if (!s) return;
      const handoff = node.handoff,
        from = node.kind === 'root' ? start : handoff!.at,
        to =
          node.kind === 'root'
            ? end
            : s.state === 'work'
              ? now()
              : (handoff!.done ?? s.last ?? from);
      const duration = dur(from, to),
        ownCost = costForSession(node.sid),
        rollup = costForSessions(
          allNodes(node)
            .map((x) => SESS[x.sid])
            .filter(Boolean),
        ),
        status = STATE[s.state] ?? s.state;
      const segments: { left: number; width: number; kind: string }[] = [],
        segment = (a: number, b: number, kind: string) => {
          if (b > a) segments.push({ left: pct(a), width: Math.max(0.2, pct(b) - pct(a)), kind });
        };
      const intervals = (s.busy ?? [])
        .map(([a, b]) => [Math.max(from, a), Math.min(to, b)])
        .filter(([a, b]) => b > a)
        .sort((a, b) => a[0] - b[0]);
      let cursor = from;
      for (const [a, b] of intervals) {
        if (a > cursor) segment(cursor, a, 'idle');
        segment(a, b, 'busy');
        cursor = Math.max(cursor, b);
      }
      if (to > cursor) segment(cursor, to, 'idle');
      rows.push({
        kind: 'agent',
        id: node.sid,
        depth: Math.min(3, depth),
        label: [s.name, s.kind ?? HARNESS[s.harness], status, duration, costText(rollup)].join(
          ', ',
        ),
        name: s.name,
        relay: node.kind === 'relay',
        critical: !!handoff && critical.has(handoff.id),
        harnessClass: hcls(node.sid),
        model: [kindText(s), shortModel(s.model)].filter(Boolean).join(' · '),
        state: s.state,
        stateLabel: status,
        tokens:
          tok(allNodes(node).reduce((sum, n) => sum + usageTotal(SESS[n.sid]), 0) / 1000000) +
          ' tokens',
        duration,
        cost: rollup.usd == null ? '—' : shortMoney(rollup.usd),
        costTip: 'Own cost: ' + costText(ownCost),
        segments,
        spawns: node.children.map((child) => pct(child.handoff!.at)),
        waits: waits
          .filter((w) => w.sid === node.sid)
          .map((w) => ({
            left: pct(w.start),
            width: Math.max(0.2, pct(w.end ?? end) - pct(w.start)),
            tip: w.targets?.length
              ? 'Waiting on ' + w.targets.map(nameOf).join(', ')
              : 'Wait without a recorded target',
          })),
        turn: node.turn?.id,
        edge: parent
          ? {
              parent,
              started: pct(handoff!.at),
              done: pct(handoff!.done ?? (s.state === 'work' ? now() : s.last)),
              returned: Number.isFinite(handoff!.done),
            }
          : undefined,
      });
      const folded = foldRows && node.children.length > 8 && !expanded.has(node.sid);
      for (const child of folded ? node.children.slice(0, 8) : node.children)
        add(child, Math.min(3, depth + 1), node.sid);
      if (folded) {
        const hidden = node.children.slice(8),
          ids = new Set(hidden.flatMap((n) => allNodes(n).map((x) => x.sid))),
          cost = costForSessions([...ids].map((id) => SESS[id]).filter(Boolean));
        rows.push({
          kind: 'more',
          id: 'more:' + node.sid,
          fold: node.sid,
          depth: Math.min(3, depth + 1),
          name: '+' + hidden.length + ' more',
          label: 'Show ' + hidden.length + ' more agents under ' + s.name,
          cost: cost.usd == null ? '—' : shortMoney(cost.usd),
          costTip: "Hidden agents' rollup cost: " + costText(cost),
        });
      }
    };
    add(model.rootNode, 0);
    if (model.peers.length) {
      rows.push({ kind: 'heading', id: 'relayed-heading', depth: 0, label: '', name: '' });
      for (const peer of model.peers) add(peer, 0);
    }
    return {
      summary,
      start: clock(start),
      end: clock(end),
      ticks: ticks.map((offset) => ({
        left: pct(start + offset),
        label: '+' + Math.round(offset / 60000) + 'm',
        nearEnd: offset / span > 0.86,
      })),
      legend: waits.some((w) => w.targets?.length)
        ? 'Recorded waits on agents'
        : waits.length
          ? 'Recorded waits without known targets'
          : 'Wait dependencies not recorded',
      incomplete: allRunNodes(model).some((node) => SESS[node.sid]?.wait_edges_truncated),
      rows,
    };
  }

  return { traceAgentTree, agentSnapshot };
}

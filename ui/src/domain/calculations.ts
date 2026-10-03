import type {
  Session,
  Handoff,
  Turn,
  SessionState,
  DomainState,
  Cost,
  ModelCost,
  TokenKind,
} from './types';
import { shortName, machineShorts, clean, clock as formatClock, shortModel } from './format';
export function createDomain(
  state: DomainState,
  now: () => number,
  SEEN_RESULTS: ReadonlySet<string>,
) {
  const {
    sessions: SESS,
    machines: MACHINE,
    handoffs: H,
    turns: TURNS,
    turn: TURN,
    starts: STARTS,
    holds: HOLDS,
    handoff: HID,
    transcriptMeta: TXM,
  } = state;
  const clock = (at: number) => formatClock(at, now());
  const nameOf = (id: string) => (id === 'you' ? 'You' : SESS[id].name);
  const hcls = (id: string) => (id === 'you' ? 'h-you' : 'h-' + SESS[id].harness);
  const where = (s: Session) =>
    s.repo
      ? s.repo + (s.branch && s.branch !== 'main' && s.branch !== s.name ? ' · ' + s.branch : '')
      : 'No repo';
  const hostOf = (s: Session) => s.host ?? MACHINE[s.machine] ?? s.machine ?? 'Unknown machine';

  const machineLabels = (
    scope: Iterable<Session | string> = Object.values(SESS),
  ): Map<string, string> => {
    const names = new Map<string, string>();
    for (const x of scope) {
      const s = typeof x === 'string' ? SESS[x] : x;
      if (s?.machine != null && !names.has(s.machine))
        names.set(s.machine, s.host ?? MACHINE[s.machine] ?? s.machine);
    }
    return names.size > 1 ? machineShorts([...names]) : new Map<string, string>();
  };
  // A session's machine by its short name, or "" where the view is of one machine.
  const machineLabel = (s: Session | null | undefined, scope?: Iterable<Session | string>) =>
    s ? (machineLabels(scope).get(s.machine) ?? '') : '';
  const branchOf = (s: Session) => s.worktree ?? s.branch ?? 'No branch';

  const shortHost = (s: Session) =>
    s.host == null && MACHINE[s.machine] == null && s.machine == null
      ? hostOf(s)
      : shortName(hostOf(s));

  const parentOf = (sid: string) =>
    SESS[sid]?.parent ?? H.find((h: Handoff) => h.kind === 'spawn' && h.to === sid)?.from;
  const originHandoff = (sid: string) =>
    H.find(
      (h) =>
        (h.kind === 'spawn' || h.kind === 'relay') &&
        h.to === sid &&
        h.from !== sid &&
        (h.kind === 'spawn' || SESS[sid]?.kind === 'Relayed' || !SESS[sid]?.lane),
    );
  const RANK: Partial<Record<SessionState, number>> = { wait: 0, work: 1, err: 2, done: 3 };
  const isResult = (h: Handoff) => h.kind === 'toyou' && h.ask === 'result';

  const inbox = () => {
    const entries: Handoff[] = H.filter(
      (h: Handoff) =>
        h.kind === 'toyou' && (h.status === 'wait' || (isResult(h) && !SEEN_RESULTS.has(h.id))),
    );
    const waiting = new Set(entries.filter((h) => h.status === 'wait').map((h) => h.from));
    for (const s of Object.values(SESS))
      if (s.state === 'wait' && !waiting.has(s.id))
        entries.push({
          id: 'wait:' + s.id,
          kind: 'toyou',
          from: s.id,
          to: 'you',
          ask: 'decision',
          status: 'wait',
          at: s.waiting_since ?? s.last,
          brief: s.waiting_for ?? 'Waiting for your input or permission',
        });
    return entries.sort((a, b) => b.at - a.at);
  };
  const working = () => Object.values(SESS).filter((s) => s.state === 'work');

  const answersOf = (h: Handoff) =>
    h.kind === 'toyou' && h.ask !== 'result' && h.status === 'done'
      ? (Array.isArray(h.answers)
          ? h.answers.map((a) => (a.values ?? []).join(', '))
          : Array.isArray(h.answer)
            ? h.answer
            : h.answer != null
              ? [h.answer]
              : []
        )
          .map((a) => String(a).trim())
          .filter(Boolean)
      : null;

  const statWord = (h: Handoff) =>
    isResult(h)
      ? SEEN_RESULTS.has(h.id)
        ? 'read'
        : 'new'
      : {
          new: undefined,
          work: 'working',
          wait: 'waiting on you',
          err: 'failed',
          done: h.kind === 'toyou' ? 'answered' : h.result ? 'returned' : 'delivered',
        }[h.status];

  const hasTurn = (t: Turn) =>
    !!t.end ||
    !!(t.start || t.u) ||
    t.entries.some((e) => e.k === 'a' || e.k === 'tool' || e.k === 'h');
  const oneLine = (s: string) => clean(s).replace(/\s+/g, ' ').trim();
  // How a turn ended: still working, a message to you, a failure, a return or reply, or nothing recorded.
  const TOYOU: Record<string, string> = {
    question: 'Asked you',
    result: 'Sent you a result',
    decision: 'Needs your decision',
  };
  function turnEnd(t: Turn): { st: SessionState; text: string } | null {
    if (!hasTurn(t)) return null;
    // The model leaves an active or partially indexed turn without an end record. Keep the mockup's
    // transcript-derived fallback so outgoing work remains traceable while the run is in progress.
    if (!t.end) {
      if (t.last && SESS[t.sid]?.state === 'wait')
        return { st: 'wait', text: 'Waiting on permission or input' };
      if (t.last && SESS[t.sid]?.state === 'work') return { st: 'work', text: 'Still working' };
      const ty = t.out.filter((h: Handoff) => h.kind === 'toyou').at(-1);
      if (ty)
        return {
          st: ty.status === 'wait' ? 'wait' : 'done',
          text:
            (TOYOU[ty.kind === 'toyou' ? ty.ask : ''] ?? 'Sent you a message') +
            ' · ' +
            statWord(ty) +
            ' · ' +
            clock(ty.at),
        };
      const entries = t.entries.filter(
        (e) => e.k === 'a' || e.k === 'tool' || (e.k === 'h' && t.out.includes(HID.get(e.id)!)),
      );
      const last = entries.at(-1),
        h = last?.k === 'h' ? HID.get(last.id) : null;
      if (t.start?.status === 'err')
        return { st: 'err', text: 'Failed' + (t.start.done ? ' · ' + clock(t.start.done) : '') };
      if (last?.k === 'tool' && last.ok === false && !last.live)
        return {
          st: 'err',
          text: last.unfinished ? 'Stopped on a step with no result' : 'Stopped on a failed step',
        };
      if (h?.status === 'err') return { st: 'err', text: 'Handoff to ' + nameOf(h.to) + ' failed' };
      if (t.start?.kind === 'spawn' && t.start.status === 'done')
        return {
          st: 'done',
          text:
            'Returned to ' +
            nameOf(t.start.from) +
            (t.start.done ? ' · ' + clock(t.start.done) : ''),
        };
      if (entries.some((e) => e.k === 'a')) return { st: 'done', text: 'Replied' };
      return { st: 'idle', text: 'No reply in these logs' };
    }
    const { st, why } = t.end,
      mh = t.end.h ? HID.get(t.end.h) : null;
    if (why === 'input') return { st: 'wait', text: 'Waiting for your input' };
    if (why === 'permission' || why === 'waiting' || why === 'waiting_permission')
      return { st: 'wait', text: 'Waiting on permission' };
    if (why === 'working') return { st: 'work', text: 'Still working' };
    if (why === 'toyou' && mh)
      return {
        st: mh.status === 'wait' ? 'wait' : 'done',
        text:
          (TOYOU[mh.kind === 'toyou' ? mh.ask : ''] ?? 'Sent you a message') +
          ' · ' +
          statWord(mh) +
          ' · ' +
          clock(mh.at),
      };
    if (why === 'failed')
      return { st: 'err', text: 'Failed' + (t.start?.done ? ' · ' + clock(t.start.done) : '') };
    if (why === 'unfinished_step') return { st: 'err', text: 'Stopped on a step with no result' };
    if (why === 'failed_step') return { st: 'err', text: 'Stopped on a failed step' };
    if (why === 'handoff_failed' && mh)
      return { st: 'err', text: 'Handoff to ' + nameOf(mh.to) + ' failed' };
    if (why === 'returned' && t.start)
      return {
        st: 'done',
        text:
          'Returned to ' + nameOf(t.start.from) + (t.start.done ? ' · ' + clock(t.start.done) : ''),
      };
    if (why === 'replied') return { st: 'done', text: 'Replied' };
    return { st: st ?? 'idle', text: 'No reply in these logs' };
  }
  // Where a turn's trace began: from a relay or brief, step back to the sender's turn that sent it, until a turn
  // that started with your message or whose sender's side isn't in the logs.
  function traceRoot(t: Turn) {
    const seen = new Set();
    while (!seen.has(t.id)) {
      seen.add(t.id);
      const up = t.start && t.start.from !== 'you' ? HOLDS.get(t.start.id) : null;
      if (!up) break;
      t = up;
    }
    return t;
  }

  const countOf = (s: Session, key: 'calls' | 'errors') => {
    const m = TXM[s.id];
    return (m && m.to >= m.total ? m[key] : undefined) ?? s[key] ?? m?.[key] ?? null;
  };
  const callsText = (calls: number | null | undefined) =>
    (calls == null ? '—' : calls) + (calls === 1 ? ' tool call' : ' tool calls');

  let childrenCache: Map<string, Session[]> | null = null;
  const invalidate = () => {
    childrenCache = null;
  };
  const sessionChildren = () => {
    if (childrenCache) return childrenCache;
    const children = new Map<string, Session[]>();
    for (const session of Object.values(SESS)) {
      const parent = parentOf(session.id);
      if (parent && SESS[parent]) {
        const rows = children.get(parent) ?? [];
        rows.push(session);
        children.set(parent, rows);
      }
    }
    for (const rows of children.values()) rows.sort((a, b) => b.last - a.last);
    return (childrenCache = children);
  };

  const childSessions = (sid: string) =>
    [...(sessionChildren().get(sid) ?? [])].sort(
      (a, b) => (originHandoff(a.id)?.at ?? a.last) - (originHandoff(b.id)?.at ?? b.last),
    );
  const descendantsOf = (
    sid: string,
    children: ReadonlyMap<string, Session[]>,
    out: Session[] = [],
    seen = new Set([sid]),
  ) => {
    for (const child of children.get(sid) ?? [])
      if (!seen.has(child.id)) {
        seen.add(child.id);
        out.push(child);
        descendantsOf(child.id, children, out, seen);
      }
    return out;
  };

  const TOTAL_TOKEN_KINDS: TokenKind[] = ['input', 'output', 'cache_write', 'cache_read'];
  const TOKEN_KINDS: [TokenKind, string][] = [
    ['input', 'Input'],
    ['output', 'Output'],
    ['cache_read', 'Cache read'],
    ['cache_write_5m', 'Cache write · 5m'],
    ['cache_write_1h', 'Cache write · 1h'],
    ['web_search', 'Web search'],
  ];
  const asMoney = (usd: number) => '$' + usd.toFixed(2),
    shortMoney = (usd: number) => '$' + usd.toFixed(1);
  const usageTotal = (s: Session) =>
    Object.values(s.tokens_by_model ?? {}).reduce(
      (sum, usage) => sum + TOTAL_TOKEN_KINDS.reduce((n, key) => n + (Number(usage[key]) || 0), 0),
      0,
    );
  function costForSessions(sessions: Iterable<Session>) {
    const total: Required<Cost> = {
        usd: 0,
        unpriced_models: [],
        split_unknown_messages: 0,
        by_model: {},
        by_day: {},
      },
      unpriced = new Set<string>();
    let allPriced = true;
    for (const s of sessions) {
      const cost: Partial<Cost> = s.cost ?? {};
      if (cost.usd == null) allPriced = false;
      else total.usd = (total.usd ?? 0) + (Number(cost.usd) || 0);
      for (const model of cost.unpriced_models ?? []) unpriced.add(model);
      total.split_unknown_messages += Number(cost.split_unknown_messages) || 0;
      for (const [day, amount] of Object.entries(cost.by_day ?? {}))
        total.by_day[day] = (total.by_day[day] ?? 0) + (Number(amount) || 0);
      for (const [modelId, model] of Object.entries(cost.by_model ?? {})) {
        const current: Required<ModelCost> = Object.assign(
          { tokens: {}, usd_by_kind: {} },
          total.by_model[modelId] ?? { usd: 0, tokens: {}, usd_by_kind: {} },
        );
        if (model.usd == null) current.usd = null;
        else if (current.usd != null) current.usd += Number(model.usd) || 0;
        for (const [key, amount] of Object.entries(model.tokens ?? {}))
          current.tokens[key as TokenKind] =
            (current.tokens[key as TokenKind] ?? 0) + (Number(amount) || 0);
        for (const [key, amount] of Object.entries(model.usd_by_kind ?? {}))
          current.usd_by_kind[key as TokenKind] =
            (current.usd_by_kind[key as TokenKind] ?? 0) + (Number(amount) || 0);
        total.by_model[modelId] = current;
      }
    }
    total.unpriced_models = [...unpriced].sort();
    if (!allPriced || unpriced.size) total.usd = null;
    return total;
  }
  const costForSession = (sid: string, includeRuns = false) =>
    costForSessions(
      SESS[sid] ? [SESS[sid], ...(includeRuns ? descendantsOf(sid, sessionChildren()) : [])] : [],
    );
  const costText = (cost: Cost) =>
    cost.usd == null || cost.unpriced_models?.length ? '—' : asMoney(cost.usd);
  const costMissing = (cost: Cost) => cost.unpriced_models ?? [];

  const TREE_RANK: Record<SessionState, number> = { wait: 0, work: 1, err: 2, idle: 3, done: 4 };
  const urgentDescendant = (sid: string, children: ReadonlyMap<string, Session[]>) =>
    descendantsOf(sid, children)
      .filter((s) => s.state in TREE_RANK)
      .sort((a, b) => TREE_RANK[a.state] - TREE_RANK[b.state] || b.last - a.last)[0]?.state;
  // What a parent's descendants are doing, as parts to join: the runs, then only the non-zero needs-you, working and failed counts (the viewer's own state words).
  const childParts = (all: Session[]) => {
    const n = (state: SessionState) => all.filter((x) => x.state === state).length,
      wait = n('wait'),
      work = n('work'),
      err = n('err');
    return [
      all.length + (all.length === 1 ? ' run' : ' runs'),
      wait && wait + ' needs you',
      work && work + ' working',
      err && err + ' failed',
    ].filter(Boolean);
  };
  const defaultTreeOpen = (sid: string, children: ReadonlyMap<string, Session[]>) =>
    descendantsOf(sid, children).some((s) => s.state === 'wait' || s.state === 'work');

  function kidRank(c: Session, children: ReadonlyMap<string, Session[]>) {
    let rank = c.state === 'wait' ? 0 : c.state === 'work' ? 1 : 2;
    if (rank)
      for (const d of descendantsOf(c.id, children)) {
        if (d.state === 'wait') return 0;
        if (d.state === 'work') rank = 1;
      }
    return rank;
  }

  function lineageOf(sid: string) {
    const path = [],
      seen = new Set();
    let id: string | undefined = sid;
    while (id && SESS[id] && !seen.has(id)) {
      seen.add(id);
      path.push(SESS[id]);
      id = parentOf(id);
    }
    return path.reverse();
  }

  const byState = (a: Session, b: Session) =>
    (RANK[a.state] ?? 3) - (RANK[b.state] ?? 3) || b.last - a.last;
  const onMachine = (m: string) =>
    Object.values(SESS)
      .filter((s) => s.machine === m)
      .sort(byState);
  const movedOff = (m: string) => Object.values(SESS).filter((s) => s.movedFrom === m);
  const movesOf = (m: string) =>
    H.filter((h: Handoff) => h.kind === 'move' && (h.fromMachine === m || h.toMachine === m)).sort(
      (a, b) => b.at - a.at,
    );

  return {
    invalidate,
    nameOf,
    hcls,
    where,
    hostOf,
    machineLabels,
    machineLabel,
    branchOf,
    shortHost,
    parentOf,
    originHandoff,
    RANK,
    isResult,
    inbox,
    working,
    answersOf,
    statWord,
    hasTurn,
    oneLine,
    TOYOU,
    turnEnd,
    traceRoot,
    countOf,
    callsText,
    sessionChildren,
    childSessions,
    descendantsOf,
    TOTAL_TOKEN_KINDS,
    TOKEN_KINDS,
    asMoney,
    usageTotal,
    costForSessions,
    costForSession,
    costText,
    costMissing,
    TREE_RANK,
    urgentDescendant,
    childParts,
    defaultTreeOpen,
    kidRank,
    lineageOf,
    byState,
    onMachine,
    movedOff,
    movesOf,
    shortMoney,
  };
}

export type DomainController = ReturnType<typeof createDomain>;

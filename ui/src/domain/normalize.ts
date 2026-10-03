import type { ModelWire } from '../lib/model';
import type { Session, Cost, ModelCost, Usage, Handoff, Turn, TurnEnd } from './types';
import { object, text, number, boolean, optional, array, dictionary, state, status } from './validate';
const strings = (v: unknown) => array(v, text), numbers = (v: unknown) => dictionary(v, number);
function usage(value: unknown): Usage {
  const v = object(value);
  return { input: optional(v.input, number), output: optional(v.output, number), cache_write: optional(v.cache_write, number), cache_write_5m: optional(v.cache_write_5m, number), cache_write_1h: optional(v.cache_write_1h, number), cache_read: optional(v.cache_read, number), web_search: optional(v.web_search, number) };
}
function modelCost(value: unknown): ModelCost { const v = object(value); return { usd: v.usd == null ? null : number(v.usd), tokens: optional(v.tokens, usage), usd_by_kind: optional(v.usd_by_kind, usage) }; }
function cost(value: unknown): Cost {
  const v = object(value);
  return { usd: v.usd == null ? null : number(v.usd), unpriced_models: optional(v.unpriced_models, strings), split_unknown_messages: optional(v.split_unknown_messages, number), by_model: optional(v.by_model, v => dictionary(v, modelCost)), by_day: optional(v.by_day, numbers) };
}
function activity(value: unknown): Session['activity'] {
  if (!Array.isArray(value) || value.length < 3 || value.length > 4) throw new Error('Invalid activity');
  return [text(value[0]), text(value[1]), number(value[2]), optional(value[3], number)];
}
export function parseSession(id: string, value: unknown): Session {
  const v = object(value);
  return { id, name: text(v.name), harness: text(v.harness), state: state(v.state), machine: text(v.machine), start: number(v.start), last: number(v.last),
    effort: optional(v.effort, text), cwd: optional(v.cwd, text), dir: optional(v.dir, text), directory: optional(v.directory, text), sessionId: optional(v.sessionId, text), pid: optional(v.pid, number),
    wait_edges_truncated: optional(v.wait_edges_truncated, boolean), busy: optional(v.busy, v => array(v, v => { if (!Array.isArray(v) || v.length !== 2) throw new Error('Invalid interval'); return [number(v[0]), number(v[1])] as [number, number]; })),
    wait_edges: optional(v.wait_edges, v => array(v, v => { const w = object(v); return { call: text(w.call), tool: text(w.tool), targets: strings(w.targets), start: number(w.start), end: optional(w.end, number), turn: optional(w.turn, text) }; })),
    reported_runs: optional(v.reported_runs, v => array(v, v => { const r = object(v); return { start: number(r.start), cost_usd: optional(r.cost_usd, number) }; })),
    cost_check: optional(v.cost_check, v => array(v, v => { const c = object(v); return { start: number(c.start), computed_usd: optional(c.computed_usd, number), reported_usd: optional(c.reported_usd, number), ok: optional(c.ok, boolean) }; })),
    kind: optional(v.kind, text), host: optional(v.host, text), repo: optional(v.repo, text), branch: optional(v.branch, text), worktree: optional(v.worktree, text), parent: optional(v.parent, text), lane: optional(v.lane, boolean),
    model: optional(v.model, text), modelId: optional(v.modelId, text), role: optional(v.role, boolean), stub: optional(v.stub, boolean), movedFrom: optional(v.movedFrom, text),
    calls: optional(v.calls, number), errors: optional(v.errors, number), waiting_since: optional(v.waiting_since, number), waiting_for: optional(v.waiting_for, text),
    tokens: optional(v.tokens, v => array(v, number)), tokens_by_model: optional(v.tokens_by_model, v => dictionary(v, usage)), cost: optional(v.cost, cost),
    activity: optional(v.activity, activity), tool_calls: optional(v.tool_calls, numbers), signals: optional(v.signals, numbers) };
}
function answer(value: unknown) { return typeof value === 'string' ? value : strings(value); }
export function parseHandoff(value: unknown): Handoff {
  const v = object(value), common = { id: text(v.id), from: text(v.from), to: v.to == null ? '' : text(v.to), at: number(v.at), status: status(v.status), brief: text(v.brief), done: optional(v.done, number), result: optional(v.result, text), target: optional(v.target, text), declined: optional(v.declined, boolean) };
  switch (v.kind) {
    case 'ask': case 'spawn': case 'relay': return { ...common, kind: v.kind };
    case 'move': return { ...common, kind: 'move', fromMachine: text(v.fromMachine), toMachine: text(v.toMachine) };
    case 'toyou': {
      if (v.ask !== 'question' && v.ask !== 'decision' && v.ask !== 'result') throw new Error('Invalid ask');
      return { ...common, kind: 'toyou', ask: v.ask, answer: optional(v.answer, answer), answers: optional(v.answers, v => array(v, v => ({ values: optional(object(v).values, strings) }))) };
    }
    default: throw new Error('Invalid handoff kind');
  }
}
function turnEnd(value: unknown): TurnEnd { const v = object(value); return { st: optional(v.st, state), why: text(v.why), h: optional(v.h, text), at: optional(v.at, number) }; }
/** A snapshot is never mutated to make stand-ins or transcript relationships. */
export function normalizeModel(model: ModelWire) {
  const sessions = Object.fromEntries(Object.entries(model.sessions).map(([id, v]) => [id, parseSession(id, v)]));
  const handoffs = model.handoffs.map(parseHandoff);
  for (const h of handoffs) if (!h.to) {
    const machine = sessions[h.from]?.machine ?? model.machine.id, id = 'unsent:' + (h.target ?? '') + (model.machines ? '@' + machine : '');
    const s = sessions[id] ??= { id, name: h.target || 'unknown', harness: 'claude', stub: true, machine, state: 'err', model: '—', tokens: [0, 0, 0], start: h.at, last: h.at };
    s.start = Math.min(s.start, h.at); s.last = Math.max(s.last, h.at); h.to = id;
  }
  const handoff = new Map(handoffs.map(h => [h.id, h])), turns: Record<string, Turn[]> = Object.create(null), turn = new Map<string, Turn>(), starts = new Map<string, Turn>(), holds = new Map<string, Turn>();
  for (const row of model.turns) {
    const x = object(row), id = text(x.id), sid = text(x.sid), start = optional(x.start, text), sent = array(x.sent, text).flatMap(id => { const h = handoff.get(id); return h ? [h] : []; });
    const t: Turn = { id, sid, start: start ? handoff.get(start) ?? null : null, at: optional(x.at, number), u: x.u ? { k: 'u', text: optional(x.text, text) ?? '' } : null, entries: [], sent, out: sent.filter(h => h.kind !== 'move'), end: optional(x.end, turnEnd), last: optional(x.last, boolean) };
    (turns[sid] ??= []).push(t); turn.set(id, t); if (t.start) starts.set(t.start.id, t); for (const h of t.sent) holds.set(h.id, t);
  }
  return { sessions, handoffs, handoff, turns, turn, starts, holds };
}

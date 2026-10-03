import type { Entry, Background, Image } from '../domain/types';
import type { ToolData } from '../lib/tool-details';
import { object, text, number, boolean, optional, array } from '../domain/validate';
const strings = (value: unknown) => array(value, text);
function background(value: unknown): Background {
  const v = object(value), state = v.state;
  if (state !== 'running' && state !== 'unknown' && state !== 'failed' && state !== 'killed' && state !== 'done') throw new Error('Invalid background state');
  return { state, secs: optional(v.secs, text), since: optional(v.since, number), exit: optional(v.exit, number), summary: optional(v.summary, text) };
}
function image(value: unknown): Image { const v = object(value); return { o: number(v.o), b: number(v.b), v: text(v.v), w: optional(v.w, number), h: optional(v.h, number), type: optional(v.type, text), size: optional(v.size, number), na: optional(v.na, boolean) }; }
const diff = (value: unknown) => array(value, (value): [string, string] => { if (!Array.isArray(value) || value.length !== 2) throw new Error('Invalid diff'); return [text(value[0]), text(value[1])]; });
function cut(value: unknown): ToolData['cut'] { const v = object(value); return { original_tokens: optional(v.original_tokens, number), parts: optional(v.parts, v => array(v, v => { const p = object(v); return { text: optional(p.text, text), gap: optional(p.gap, v => { const g = object(v); return { unit: text(g.unit), n: number(g.n), of: optional(g.of, number) }; }) }; })) }; }
export function parseEntry(value: unknown): Entry {
  const v = object(value), base = { img: optional(v.img, v => array(v, image)), turn: optional(v.turn, text), slot: optional(v.slot, number), live: optional(v.live, boolean), unfinished: optional(v.unfinished, boolean), tid: optional(v.tid, text), bg: optional(v.bg, background) };
  switch (v.k) {
    case 'u': case 'a': return { ...base, k: v.k, text: text(v.text), img: optional(v.img, v => array(v, image)) };
    case 'think': return { ...base, k: 'think', text: optional(v.text, text), pending: optional(v.pending, boolean), status: optional(v.status, text), secs: optional(v.secs, v => typeof v === 'string' ? v : number(v)) };
    case 'h': return { ...base, k: 'h', id: text(v.id) };
    case 'end': return { ...base, k: 'end', text: optional(v.text, text), ret: optional(v.ret, v => { const r = object(v); return { to: text(r.to), failed: optional(r.failed, boolean), at: optional(r.at, number) }; }) };
    case 'harness': return { ...base, k: 'harness', label: text(v.label) };
    case 'signal': return { ...base, k: 'signal', signal: (() => { const s = object(v.signal); return { kind: text(s.kind), tag: optional(s.tag, text), tool: optional(s.tool, text), value: optional(s.value, number), previous: optional(s.previous, text) }; })() };
    case 'bgend': return { ...base, k: 'bgend', call: text(v.call), state: text(v.state), label: optional(v.label, text) };
    case 'tool': return { ...base, k: 'tool', name: text(v.name), arg: text(v.arg), title: optional(v.title, text), secs: optional(v.secs, text), since: optional(v.since, number), exit: optional(v.exit, number), ok: v.ok == null ? v.ok : boolean(v.ok), in: optional(v.in, text), out: optional(v.out, text), cwd: optional(v.cwd, text), diff: optional(v.diff, diff), changes: optional(v.changes, v => array(v, v => { const c = object(v); return { path: text(c.path), move: optional(c.move, text), diff: optional(c.diff, diff) }; })), more: optional(v.more, strings), script: v.script, cut: optional(v.cut, cut) };
    default: throw new Error('Invalid transcript entry');
  }
}
export interface TranscriptPage { entries: Entry[]; from: number; to: number; total: number; calls?: number; errors?: number; bg_running?: string[] }
export function parseTranscriptPage(value: unknown): TranscriptPage {
  const v = object(value), from = number(v.from), to = number(v.to), total = number(v.total);
  if (![from, to, total].every(Number.isInteger) || from < 0 || to < from || total < to) throw new Error('Invalid transcript range');
  return { entries: array(v.entries, parseEntry), from, to, total, calls: optional(v.calls, number), errors: optional(v.errors, number), bg_running: optional(v.bg_running, strings) };
}

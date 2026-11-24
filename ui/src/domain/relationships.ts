import { array, boolean, object, text, number } from './validate';
import type { Handoff } from './types';

export interface RecordedRelationship {
  edge: Handoff;
  briefOwner?: string;
  resultOwner?: string;
  targetTurn?: string;
  sourceTurn?: string;
}
export interface RecordedTurn {
  id: string;
  start?: string;
  u: boolean;
  at?: number;
  end: { st: string; why: string; at?: number };
  sent: string[];
  last: boolean;
}
export function parseRecordedTurn(value: unknown): RecordedTurn {
  const row = object(value),
    end = object(row.end),
    sent = array(row.sent, text),
    st = text(end.st);
  if (sent.length > 1000 || !['work', 'wait', 'idle', 'done', 'err', 'off'].includes(st))
    throw Error('Invalid recorded turn');
  return {
    id: text(row.id),
    start: row.start == null ? undefined : text(row.start),
    u: boolean(row.u),
    at: row.at == null ? undefined : number(row.at),
    end: { st, why: text(end.why), at: end.at == null ? undefined : number(end.at) },
    sent,
    last: boolean(row.last),
  };
}
export function parseRecordedRelationship(value: unknown): RecordedRelationship {
  const row = object(value),
    kind = text(row.kind),
    status = text(row.status);
  if (!['work', 'wait', 'idle', 'done', 'err', 'new', 'off'].includes(status))
    throw Error('Invalid recorded handoff state');
  const reason = row.reason == null ? undefined : text(row.reason);
  const targetLabel = row.target_label == null ? undefined : text(row.target_label);
  if (
    (reason !== undefined &&
      (reason !== 'not linked yet' ||
        (kind !== 'relay' && kind !== 'spawn') ||
        row.to != null ||
        row.target_turn != null ||
        row.target_evidence != null ||
        row.brief_session != null ||
        row.result_session != null)) ||
    (targetLabel !== undefined && (reason === undefined || targetLabel.length > 192))
  )
    throw Error('Invalid sender relationship target');
  const base = {
    id: text(row.id),
    from: text(row.from),
    to: row.to == null ? '' : text(row.to),
    at: number(row.at),
    done: row.done == null ? undefined : number(row.done),
    status: status as Handoff['status'],
    brief: '',
    result: row.result == null ? undefined : text(row.result),
    declined: boolean(row.declined),
    reason: reason === 'not linked yet' ? ('not linked yet' as const) : undefined,
    targetLabel,
  };
  let edge: Handoff;
  if (kind === 'toyou') {
    const ask = text(row.ask);
    if (!['question', 'decision', 'result'].includes(ask)) throw Error('Invalid recorded question');
    edge = { ...base, kind, ask: ask as 'question' | 'decision' | 'result' };
  } else if (kind === 'move') edge = { ...base, kind, fromMachine: '', toMachine: '' };
  else if (kind === 'ask' || kind === 'spawn' || kind === 'relay') edge = { ...base, kind };
  else throw Error('Invalid recorded handoff');
  return {
    edge,
    briefOwner: row.brief_session == null ? undefined : text(row.brief_session),
    resultOwner: row.result_session == null ? undefined : text(row.result_session),
    targetTurn: row.target_turn == null ? undefined : text(row.target_turn),
    sourceTurn: row.source_turn == null ? undefined : text(row.source_turn),
  };
}
export function senderRelationshipPresentation(edge: Handoff) {
  if (!edge.reason || (edge.kind !== 'relay' && edge.kind !== 'spawn')) return undefined;
  const label =
    edge.targetLabel ?? (edge.kind === 'spawn' ? 'an unnamed agent' : 'an unnamed recipient');
  return {
    parts: [{ className: 'verb', text: (edge.kind === 'spawn' ? 'Spawned ' : 'Sent to ') + label }],
    reason: edge.reason,
    status:
      edge.status === 'done'
        ? 'Sender recorded success'
        : edge.status === 'err'
          ? 'Sender recorded failure'
          : 'Sender outcome unavailable',
  };
}
export function parseNativeRelationships(value: unknown, ids: readonly string[]) {
  const data = object(value),
    allowed = new Set(ids);
  const rows = Object.entries(object(data.events)).map(([id, value]) => {
    const row = object(value);
    if (!allowed.has(id)) throw Error('Native relationship exceeds its issued selection');
    return {
      id,
      brief: row.brief == null ? undefined : text(row.brief),
      result: row.result == null ? undefined : text(row.result),
      answer:
        row.answer == null
          ? undefined
          : Array.isArray(row.answer)
            ? array(row.answer, text)
            : text(row.answer),
    };
  });
  if (rows.length > ids.length || new Set(rows.map((row) => row.id)).size !== rows.length)
    throw Error('Repeated native relationship');
  return rows;
}

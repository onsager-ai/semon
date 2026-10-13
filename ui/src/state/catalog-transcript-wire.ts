import { array, boolean, dictionary, number, object, text } from '../domain/validate';
import {
  parseRecordedRelationship,
  parseRecordedTurn,
  type RecordedRelationship,
  type RecordedTurn,
} from '../domain/relationships';
import type { Entry } from '../domain/types';
import type {
  CatalogSessionIdentity,
  CatalogSourceReference,
  CatalogFreshness,
} from '../domain/catalog';
import {
  parseCatalogIdentity,
  parseCatalogSession,
  parseCatalogSource,
  type CatalogSession,
} from './catalog-wire';
import { parseEntry } from './transcript-wire';
export interface CatalogTranscriptEntry {
  entry_id: string;
  slot: number;
  observed_at?: number | null;
  entry: Entry;
  native_action_text?: string;
  clipped?: boolean;
  field?: {
    name: 'text' | 'out';
    chunks: number;
    complete: boolean;
    layers: 1 | 2;
    provenance?: NonNullable<CatalogTranscriptEntry['provenance']>;
  };
  freshness?: { state: CatalogFreshness };
  provenance: {
    source: CatalogSourceReference['source'];
    native_event_id: string | null;
    offset: number;
    block: number;
  } | null;
}
export interface RecordedSessionPresentation {
  name: string;
  harness: string;
  model: string;
  kind?: string;
  state?: string;
  start?: number;
  last?: number;
  calls?: number;
  usd?: number;
  live_observation?: boolean;
}
/** Selected native content is a ranged read view, not a complete model graph. */
export interface CatalogTranscriptPage {
  api: 1;
  identity: CatalogSessionIdentity;
  session: CatalogSession;
  projection: { version: 6; generation: string; errorsGeneration: string; total: number };
  range: { first: number; end: number; next: number | null };
  entries: CatalogTranscriptEntry[];
  freshness: { state: CatalogFreshness };
  relationship_context: {
    state: 'incomplete' | 'complete';
    turn_ids: string[];
    turns: RecordedTurn[];
    handoffs: RecordedRelationship[];
    sessions: Record<string, RecordedSessionPresentation>;
    previews: Record<string, string>;
  };
}
function ordinal(value: unknown): number {
  const v = number(value);
  if (!Number.isSafeInteger(v) || v < 0) throw new Error('Invalid catalog transcript ordinal');
  return v;
}
function entry(value: unknown): CatalogTranscriptEntry {
  const row = object(value),
    entryId = text(row.entry_id),
    slot = ordinal(row.slot),
    parsed = parseEntry(row);
  const provenance = row.provenance === null && parsed.k === 'end' ? null : object(row.provenance);
  if (!provenance && (row.field !== undefined || row.clipped === true))
    throw new Error('Unprovenanced boundary cannot expose native text');
  if (!entryId) throw new Error('Invalid catalog entry identity');
  let field: CatalogTranscriptEntry['field'];
  if (row.field !== undefined) {
    const reference = object(row.field);
    if (reference.name !== 'text' && reference.name !== 'out')
      throw new Error('Unsupported native text field');
    const layers = reference.layers ?? 1;
    if (layers !== 1 && layers !== 2) throw new Error('Unsupported native field encoding');
    field = {
      layers,
      name: reference.name,
      chunks: ordinal(reference.chunks),
      complete: boolean(reference.complete),
      provenance:
        reference.provenance === undefined
          ? undefined
          : (() => {
              const anchor = object(reference.provenance),
                source = parseCatalogSource(anchor.source);
              if (
                !provenance ||
                JSON.stringify(source) !== JSON.stringify(parseCatalogSource(provenance.source))
              )
                throw Error('Native field must belong to its selected source');
              const offset = ordinal(anchor.offset),
                block = ordinal(anchor.block);
              if (offset > source.offset || block > 1024)
                throw Error('Invalid native field anchor');
              return {
                source,
                offset,
                block,
                native_event_id:
                  anchor.native_event_id === null ? null : text(anchor.native_event_id),
              };
            })(),
    };
  }
  let freshness: CatalogTranscriptEntry['freshness'];
  if (row.freshness !== undefined) {
    const state = object(row.freshness).state;
    if (
      state !== 'cached' &&
      state !== 'stale' &&
      state !== 'incomplete' &&
      state !== 'unavailable'
    )
      throw new Error('Invalid catalog entry freshness');
    freshness = { state };
  }
  parsed.key = entryId;
  parsed.slot = slot;
  return {
    entry_id: entryId,
    slot,
    observed_at: row.observed_at == null ? null : number(row.observed_at),
    entry: parsed,
    clipped: row.clipped === undefined ? undefined : boolean(row.clipped),
    field,
    freshness,
    native_action_text: parsed.k === 'h' && row.text !== undefined ? text(row.text) : undefined,
    provenance: provenance
      ? {
          source: parseCatalogSource(provenance.source),
          native_event_id:
            provenance.native_event_id === null ? null : text(provenance.native_event_id),
          offset: ordinal(provenance.offset),
          block: ordinal(provenance.block),
        }
      : null,
  };
}
export function parseCatalogTranscriptPage(value: unknown): CatalogTranscriptPage {
  const row = object(value);
  if (!Array.isArray(row.entries) || row.entries.length > 100)
    throw new Error('Invalid catalog transcript page size');
  const identity = parseCatalogIdentity(row.identity),
    session = parseCatalogSession(row.session),
    projection = object(row.projection),
    range = object(row.range),
    context = object(row.relationship_context),
    generation = text(projection.generation),
    errorsGeneration = text(projection.errors_generation),
    total = ordinal(projection.total),
    first = ordinal(range.first),
    end = ordinal(range.end),
    next = range.next === null ? null : ordinal(range.next),
    entries = array(row.entries, entry),
    state = object(row.freshness).state;
  if (
    row.api !== 1 ||
    projection.version !== 6 ||
    !/^[0-9a-f]{64}$/i.test(generation) ||
    !/^[0-9a-f]{64}$/i.test(errorsGeneration) ||
    session.key !== identity.catalog_key ||
    session.harness !== identity.harness ||
    end < first ||
    end > total ||
    end - first > 100 ||
    entries.length !== end - first ||
    next !== (end < total ? end : null) ||
    entries.some((item, i) => item.slot !== first + i) ||
    new Set(entries.map((item) => item.entry_id)).size !== entries.length ||
    (context.state !== 'incomplete' && context.state !== 'complete') ||
    !Array.isArray(context.handoffs) ||
    context.handoffs.length > 200 ||
    (state !== 'cached' && state !== 'stale' && state !== 'incomplete' && state !== 'unavailable')
  )
    throw new Error('Invalid catalog transcript page');
  const relationships = array(context.handoffs, parseRecordedRelationship);
  const turns = array(context.turns ?? [], (value) => {
      const row = object(value);
      if (row.sid !== identity.catalog_key) throw Error('Invalid ranged turn owner');
      return parseRecordedTurn(row);
    }),
    turnIds = new Set(entries.flatMap((item) => (item.entry.turn ? [item.entry.turn] : []))),
    starts = new Set(turns.flatMap((turn) => (turn.start ? [turn.start] : [])));
  if (
    turns.length > 100 ||
    new Set(turns.map((turn) => turn.id)).size !== turns.length ||
    turns.some((turn) => !turnIds.has(turn.id))
  )
    throw Error('Invalid ranged turn scope');
  const named = new Set(relationships.map((row) => row.edge.id));
  const actions = new Set(entries.flatMap((item) => (item.entry.k === 'h' ? [item.entry.id] : [])));
  if (
    named.size !== relationships.length ||
    relationships.some(
      (row) =>
        (!actions.has(row.edge.id) && !starts.has(row.edge.id)) ||
        (row.edge.from !== identity.catalog_key && row.edge.to !== identity.catalog_key),
    ) ||
    (context.state === 'complete' && [...actions].some((id) => !named.has(id))) ||
    Object.keys(object(context.sessions ?? {})).length > 201 ||
    Object.keys(object(context.previews ?? {})).length > 128
  )
    throw Error('Invalid ranged relationship scope');
  return {
    api: 1,
    identity,
    session,
    projection: { version: 6, generation, errorsGeneration, total },
    range: { first, end, next },
    entries,
    freshness: { state },
    relationship_context: {
      state: context.state,
      turn_ids: array(context.turn_ids, text),
      turns,
      handoffs: relationships,
      sessions: dictionary(context.sessions ?? {}, (value) => {
        const row = object(value);
        return {
          name: text(row.name),
          harness: text(row.harness),
          model: text(row.model),
          kind: row.kind == null ? undefined : text(row.kind),
          state: row.state == null ? undefined : text(row.state),
          start: row.start == null ? undefined : number(row.start),
          last: row.last == null ? undefined : number(row.last),
          calls: row.calls == null ? undefined : ordinal(row.calls),
          usd: row.usd == null ? undefined : number(row.usd),
          live_observation:
            row.live_observation == null
              ? undefined
              : row.live_observation === 0
                ? false
                : row.live_observation === 1
                  ? true
                  : boolean(row.live_observation),
        };
      }),
      previews: dictionary(context.previews ?? {}, (value) => {
        const generation = text(value);
        if (!/^[a-f0-9]{64}$/.test(generation)) throw Error('Invalid context generation');
        return generation;
      }),
    },
  };
}

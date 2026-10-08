import { array, boolean, number, object, text } from '../domain/validate';
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
  entry: Entry;
  native_action_text?: string;
  clipped?: boolean;
  field?: { name: 'text' | 'out'; chunks: number; complete: boolean; layers: 1 | 2 };
  freshness?: { state: CatalogFreshness };
  provenance: {
    source: CatalogSourceReference['source'];
    native_event_id: string | null;
    offset: number;
    block: number;
  } | null;
}
/** Selected native content is a ranged read view, not a complete model graph. */
export interface CatalogTranscriptPage {
  api: 1;
  identity: CatalogSessionIdentity;
  session: CatalogSession;
  projection: { version: 5; generation: string; total: number };
  range: { first: number; end: number; next: number | null };
  entries: CatalogTranscriptEntry[];
  freshness: { state: CatalogFreshness };
  relationship_context: { state: 'incomplete'; turn_ids: string[]; handoffs: [] };
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
    total = ordinal(projection.total),
    first = ordinal(range.first),
    end = ordinal(range.end),
    next = range.next === null ? null : ordinal(range.next),
    entries = array(row.entries, entry),
    state = object(row.freshness).state;
  if (
    row.api !== 1 ||
    projection.version !== 5 ||
    !/^[0-9a-f]{64}$/i.test(generation) ||
    session.key !== identity.catalog_key ||
    session.harness !== identity.harness ||
    end < first ||
    end > total ||
    end - first > 100 ||
    entries.length !== end - first ||
    next !== (end < total ? end : null) ||
    entries.some((item, i) => item.slot !== first + i) ||
    new Set(entries.map((item) => item.entry_id)).size !== entries.length ||
    context.state !== 'incomplete' ||
    !Array.isArray(context.handoffs) ||
    context.handoffs.length !== 0 ||
    (state !== 'cached' && state !== 'stale' && state !== 'incomplete' && state !== 'unavailable')
  )
    throw new Error('Invalid catalog transcript page');
  return {
    api: 1,
    identity,
    session,
    projection: { version: 5, generation, total },
    range: { first, end, next },
    entries,
    freshness: { state },
    relationship_context: {
      state: 'incomplete',
      turn_ids: array(context.turn_ids, text),
      handoffs: [],
    },
  };
}

import { array, boolean, number, object, text } from '../domain/validate';

import type {
  CatalogFreshness,
  CatalogSourceReference,
  CatalogSessionIdentity,
  CatalogReadScope,
  CatalogNativeSelection,
} from '../domain/catalog';
export type {
  CatalogFreshness,
  CatalogSourceReference,
  CatalogSessionIdentity,
  CatalogReadScope,
  CatalogNativeSelection,
} from '../domain/catalog';

/** Metadata is a partial read view, independent of runtime status and model deltas. */
export interface CatalogSession {
  key: string;
  name: string;
  harness: string;
  kind: string;
  native_ids: string[];
  parent: string | null;
  parent_source: string | null;
  repo: string | null;
  branch: string | null;
  model: string;
  effort: string | null;
  start: number | null;
  last: number | null;
  tokens: number[];
  cost: { usd: number | null; unpriced_models: string[] };
  source_refs: CatalogSourceReference[];
  freshness: { state: CatalogFreshness };
  facts_observation?: { state: CatalogFreshness };
  native_selection?: { state: CatalogNativeSelection } | null;
}
export interface CatalogPage {
  api: 1;
  machine: string;
  read_scope: CatalogReadScope;
  machine_info: { key: string; label: string; freshness: 'cached' };
  generation: string;
  observed_at: number | null;
  freshness: CatalogFreshness | 'updating';
  completeness: { state: 'partial' | 'complete' };
  capabilities: {
    pagination: boolean;
    filters: string[];
    order: string;
    full_text_search: boolean;
    selected_session_lookup: boolean;
    runtime_status: boolean;
    global_union: boolean;
  };
  items: CatalogSession[];
  next_cursor: string | null;
  search: {
    semantics: 'unicode_lowercase_substring';
    fields: string[];
    partial: boolean;
    candidates: number;
    index_complete: boolean;
    candidate_budget: 512;
    byte_budget: 2097152;
  } | null;
}
const nullable = <T>(value: unknown, parse: (value: unknown) => T): T | null =>
  value === null ? null : parse(value);
const strings = (value: unknown) => array(value, text);
function integer(value: unknown) {
  const parsed = number(value);
  if (!Number.isSafeInteger(parsed)) throw new Error('Invalid catalog integer');
  return parsed;
}
function freshness(value: unknown): CatalogFreshness {
  if (value === 'cached' || value === 'stale' || value === 'incomplete' || value === 'unavailable')
    return value;
  throw new Error('Invalid catalog freshness');
}
function readScope(value: unknown): CatalogReadScope {
  if (value === undefined || value === 'current') return 'current';
  if (value === 'retained_history') return value;
  throw new Error('Invalid catalog read scope');
}
function nativeSelection(value: unknown): CatalogNativeSelection {
  return value === 'retired' ? value : freshness(value);
}
function observations(row: Record<string, unknown>) {
  return {
    facts_observation:
      row.facts_observation === undefined
        ? undefined
        : { state: freshness(object(row.facts_observation).state) },
    native_selection:
      row.native_selection === undefined
        ? undefined
        : row.native_selection === null
          ? null
          : { state: nativeSelection(object(row.native_selection).state) },
  };
}
function digest(value: unknown) {
  const bytes = array(value, integer);
  if (bytes.length !== 32 || bytes.some((byte) => byte < 0 || byte > 255))
    throw new Error('Invalid catalog source digest');
  return bytes;
}
export function parseCatalogSource(value: unknown): CatalogSourceReference['source'] {
  const source = object(value),
    offset = integer(source.offset);
  if (offset < 0) throw new Error('Invalid catalog source offset');
  const immutable = source.immutable_generation;
  if (
    immutable !== undefined &&
    immutable !== null &&
    (typeof immutable !== 'string' || !/^[0-9a-f]{64}$/.test(immutable))
  )
    throw new Error('Invalid immutable source generation');
  return {
    root: text(source.root),
    path: text(source.path),
    native_id: text(source.native_id),
    offset,
    prefix_sha256: digest(source.prefix_sha256),
    tail_sha256: digest(source.tail_sha256),
    ...(source.immutable_generation === undefined
      ? {}
      : { immutable_generation: immutable as string | null }),
  };
}
function sourceReference(value: unknown): CatalogSourceReference {
  const row = object(value);
  return { source: parseCatalogSource(row.source), state: freshness(row.state) };
}
export function parseCatalogSession(value: unknown): CatalogSession {
  const row = object(value),
    cost = object(row.cost),
    state = object(row.freshness),
    tokens = array(row.tokens, number);
  if (tokens.length !== 3 || tokens.some((token) => token < 0))
    throw new Error('Invalid catalog tokens');
  return {
    key: text(row.key),
    name: text(row.name),
    harness: text(row.harness),
    kind: text(row.kind),
    native_ids: strings(row.native_ids),
    parent: nullable(row.parent, text),
    parent_source: nullable(row.parent_source, text),
    repo: nullable(row.repo, text),
    branch: nullable(row.branch, text),
    model: text(row.model),
    effort: nullable(row.effort, text),
    start: nullable(row.start, integer),
    last: nullable(row.last, integer),
    tokens,
    cost: { usd: nullable(cost.usd, number), unpriced_models: strings(cost.unpriced_models) },
    source_refs: array(row.source_refs, sourceReference),
    freshness: { state: freshness(state.state) },
    ...observations(row),
  };
}
function searchObservation(value: unknown): CatalogPage['search'] {
  if (value === undefined || value === null) return null;
  const row = object(value),
    fields = strings(row.fields),
    candidates = integer(row.candidates);
  if (
    row.semantics !== 'unicode_lowercase_substring' ||
    fields.length !== 6 ||
    new Set(fields).size !== 6 ||
    fields.some(
      (field) => !['name', 'key', 'repo', 'branch', 'model', 'harness'].includes(field),
    ) ||
    row.candidate_budget !== 512 ||
    row.byte_budget !== 2097152 ||
    candidates < 0 ||
    candidates > 512
  )
    throw new Error('Unsupported metadata search observation');
  return {
    semantics: 'unicode_lowercase_substring',
    fields,
    partial: boolean(row.partial),
    candidates,
    index_complete: boolean(row.index_complete),
    candidate_budget: 512,
    byte_budget: 2097152,
  };
}
export function parseCatalogPage(value: unknown): CatalogPage {
  const page = object(value),
    machine = text(page.machine),
    info = object(page.machine_info),
    capabilities = object(page.capabilities),
    generation = text(page.generation),
    items = array(page.items, parseCatalogSession),
    next = nullable(page.next_cursor, text),
    completeness = page.completeness === undefined ? 'complete' : object(page.completeness).state,
    collectionFreshness = page.freshness === 'updating' ? 'updating' : freshness(page.freshness);
  if (
    page.api !== 1 ||
    (completeness !== 'partial' && completeness !== 'complete') ||
    (collectionFreshness === 'updating' && completeness !== 'partial') ||
    info.key !== machine ||
    info.freshness !== 'cached' ||
    !/^[0-9a-f]{64}$/i.test(generation) ||
    items.length > 100 ||
    new Set(items.map((item) => item.key)).size !== items.length ||
    (next !== null && (!next || next.length > 8192))
  )
    throw new Error('Invalid catalog page');
  return {
    api: 1,
    machine,
    read_scope: readScope(page.read_scope),
    machine_info: { key: machine, label: text(info.label), freshness: 'cached' },
    generation,
    observed_at: nullable(page.observed_at, integer),
    freshness: collectionFreshness,
    completeness: { state: completeness as 'partial' | 'complete' },
    capabilities: {
      pagination: boolean(capabilities.pagination),
      filters: strings(capabilities.filters),
      order: text(capabilities.order),
      full_text_search: boolean(capabilities.full_text_search),
      selected_session_lookup: boolean(capabilities.selected_session_lookup),
      runtime_status: boolean(capabilities.runtime_status),
      global_union: boolean(capabilities.global_union),
    },
    items,
    next_cursor: next,
    search: searchObservation(page.search),
  };
}

/** Preserve explicit ambiguity; do not resolve controls from a first source. */
export function parseCatalogIdentity(value: unknown): CatalogSessionIdentity {
  const row = object(value),
    refs = array(row.source_refs, sourceReference),
    nativeIds = strings(row.native_ids),
    nativeId = nullable(row.native_id, text),
    ids = new Set(refs.map((ref) => ref.source.native_id)),
    generation = text(row.generation),
    state = object(row.freshness),
    sourceKey = text(row.source_key),
    catalogKey = text(row.catalog_key),
    harness = text(row.harness),
    qualification = row.owner_qualification;
  if (
    (qualification !== undefined &&
      qualification !== 'qualified' &&
      qualification !== 'provisional') ||
    (qualification === 'provisional' && nativeId !== null) ||
    !sourceKey ||
    !catalogKey ||
    !harness ||
    !/^[0-9a-f]{64}$/i.test(generation) ||
    nativeIds.some((id) => !id) ||
    ids.has('') ||
    new Set(nativeIds).size !== nativeIds.length ||
    ids.size !== nativeIds.length ||
    nativeIds.some((id) => !ids.has(id)) ||
    (nativeId !== null && (ids.size !== 1 || !ids.has(nativeId)))
  )
    throw new Error('Invalid catalog identity');
  return {
    source_key: sourceKey,
    read_scope: readScope(row.read_scope),
    catalog_key: catalogKey,
    harness,
    native_id: nativeId,
    owner_qualification: qualification as CatalogSessionIdentity['owner_qualification'],
    native_ids: nativeIds,
    source_refs: refs,
    machine_label: nullable(row.machine_label, text),
    generation,
    observed_at: nullable(row.observed_at, integer),
    freshness: { state: freshness(state.state) },
    ...observations(row),
  };
}

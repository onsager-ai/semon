import { array, boolean, number, object, text } from '../domain/validate';

export type CatalogFreshness = 'cached' | 'stale' | 'incomplete' | 'unavailable';
export interface CatalogSourceReference {
  source: {
    root: string;
    path: string;
    native_id: string;
    offset: number;
    prefix_sha256: number[];
    tail_sha256: number[];
  };
  state: CatalogFreshness;
}
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
}
export interface CatalogPage {
  api: 1;
  machine: string;
  machine_info: { key: string; label: string; freshness: 'cached' };
  generation: string;
  observed_at: number | null;
  freshness: 'cached';
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
function digest(value: unknown) {
  const bytes = array(value, integer);
  if (bytes.length !== 32 || bytes.some((byte) => byte < 0 || byte > 255))
    throw new Error('Invalid catalog source digest');
  return bytes;
}
function sourceReference(value: unknown): CatalogSourceReference {
  const row = object(value),
    source = object(row.source),
    offset = integer(source.offset);
  if (offset < 0) throw new Error('Invalid catalog source offset');
  return {
    source: {
      root: text(source.root),
      path: text(source.path),
      native_id: text(source.native_id),
      offset,
      prefix_sha256: digest(source.prefix_sha256),
      tail_sha256: digest(source.tail_sha256),
    },
    state: freshness(row.state),
  };
}
function session(value: unknown): CatalogSession {
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
  };
}
export function parseCatalogPage(value: unknown): CatalogPage {
  const page = object(value),
    machine = text(page.machine),
    info = object(page.machine_info),
    capabilities = object(page.capabilities),
    generation = text(page.generation),
    items = array(page.items, session),
    next = nullable(page.next_cursor, text);
  if (
    page.api !== 1 ||
    page.freshness !== 'cached' ||
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
    machine_info: { key: machine, label: text(info.label), freshness: 'cached' },
    generation,
    observed_at: nullable(page.observed_at, integer),
    freshness: 'cached',
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
  };
}

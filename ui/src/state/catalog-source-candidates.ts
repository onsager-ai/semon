import { array, boolean, number, object, text } from '../domain/validate';
export interface CatalogSourceCandidate {
  candidate_key: string;
  source: { root: string; path: string };
  native_name_hint: string | null;
  generation: string;
  archive_observed_at: number | null;
}
export interface CatalogSourceCandidatePage {
  api: 1;
  source_key: string;
  items: CatalogSourceCandidate[];
  next_cursor: string | null;
}
export interface CatalogSourceCandidateProgress {
  api: 1;
  source_key: string;
  candidate_key: string;
  generation: string;
  state: 'updating' | 'ready' | 'unavailable';
  catalog_key: string | null;
  retryable: boolean;
  reason: string | null;
}
const nullableText = (v: unknown) => (v === null ? null : text(v));
function key(v: unknown) {
  const value = text(v);
  if (!value || value.length > 8192) throw new Error('Invalid source candidate identity');
  return value;
}
function generation(v: unknown) {
  const value = text(v);
  if (!/^[a-f0-9]{64}$/.test(value)) throw new Error('Invalid source candidate generation');
  return value;
}
/** Archive hints are not canonical session metadata or native control authority. */
export function parseCatalogSourceCandidates(v: unknown): CatalogSourceCandidatePage {
  const row = object(v);
  const items = array(row.items, (value) => {
    const candidate = object(value),
      source = object(candidate.source),
      path = text(source.path),
      root = text(source.root);
    const observed =
      candidate.archive_observed_at === null ? null : number(candidate.archive_observed_at);
    if (
      !root ||
      !path ||
      path.length > 4096 ||
      path.startsWith('/') ||
      path.includes('\\') ||
      path.includes('\0') ||
      path.split('/').includes('..') ||
      (observed !== null && (!Number.isSafeInteger(observed) || observed < 0))
    )
      throw new Error('Invalid archive source hint');
    return {
      candidate_key: key(candidate.candidate_key),
      source: { root, path },
      native_name_hint: nullableText(candidate.native_name_hint),
      generation: generation(candidate.generation),
      archive_observed_at: observed,
    };
  });
  if (
    row.api !== 1 ||
    items.length > 60 ||
    new Set(items.map((item) => item.candidate_key)).size !== items.length
  )
    throw new Error('Invalid source candidate page');
  return {
    api: 1,
    source_key: key(row.source_key),
    items,
    next_cursor: row.next_cursor === null ? null : key(row.next_cursor),
  };
}
export function parseCatalogSourceCandidateProgress(v: unknown): CatalogSourceCandidateProgress {
  const row = object(v),
    catalogKey = row.catalog_key === null ? null : key(row.catalog_key);
  if (
    row.api !== 1 ||
    !['updating', 'ready', 'unavailable'].includes(text(row.state)) ||
    (row.state === 'ready') !== (catalogKey !== null)
  )
    throw new Error('Invalid source candidate progress');
  return {
    api: 1,
    source_key: key(row.source_key),
    candidate_key: key(row.candidate_key),
    generation: generation(row.generation),
    state: row.state as CatalogSourceCandidateProgress['state'],
    catalog_key: catalogKey,
    retryable: boolean(row.retryable),
    reason: nullableText(row.reason),
  };
}

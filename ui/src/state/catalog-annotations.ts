import { array, number, object, text } from '../domain/validate';

export interface CatalogAnnotations {
  source: string;
  key: string;
  generation: string;
  kind: 'errors' | 'signals';
  total: number;
  first: number;
  entries: { slot: number }[];
}
const integer = (value: unknown) => {
  const result = number(value);
  if (!Number.isSafeInteger(result) || result < 0) throw Error('Invalid annotation address');
  return result;
};
export function parseCatalogAnnotations(value: unknown): CatalogAnnotations {
  const row = object(value);
  if (
    row.api !== 1 ||
    row.scope !== 'retained_history' ||
    !['errors', 'signals'].includes(String(row.kind))
  )
    throw Error('Invalid annotation response');
  const entries = array(row.entries, (value) => {
    const entry = object(value);
    return { slot: integer(entry.slot) };
  });
  const first = integer(row.first),
    total = integer(row.total),
    generation = text(row.generation);
  if (
    entries.length > 100 ||
    first + entries.length > total ||
    !/^[a-f0-9]{64}$/.test(generation) ||
    entries.some((entry, index) => index > 0 && entry.slot <= entries[index - 1]!.slot)
  )
    throw Error('Annotation response exceeds its bound');
  return {
    source: text(row.source_key),
    key: text(row.catalog_key),
    generation,
    kind: row.kind as 'errors' | 'signals',
    first,
    total,
    entries,
  };
}

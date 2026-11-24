import { array, number, object, text } from '../domain/validate';
import { parseNativeEntryAddress, type NativeEntryAddress } from '../lib/sessionRoute';

export interface CatalogAnnotations {
  source: string;
  key: string;
  generation: string;
  kind: 'errors' | 'signals';
  total: number;
  first: number;
  scope: 'current' | 'retained_history';
  entries: ({ slot: number } | { address: NativeEntryAddress })[];
}
const integer = (value: unknown) => {
  const result = number(value);
  if (!Number.isSafeInteger(result) || result < 0) throw Error('Invalid annotation address');
  return result;
};
export function parseCatalogAnnotations(value: unknown): CatalogAnnotations {
  const row = object(value),
    native = row.projection === 7;
  if (
    row.api !== 1 ||
    (row.scope !== 'retained_history' && !(native && row.scope === 'current')) ||
    !['errors', 'signals'].includes(String(row.kind))
  )
    throw Error('Invalid annotation response');
  const entries = array(row.entries, (value) => {
    const entry = object(value);
    return native
      ? { address: parseNativeEntryAddress(entry.address) }
      : { slot: integer(entry.slot) };
  });
  const first = integer(row.first),
    total = integer(row.total),
    generation = text(row.generation);
  if (
    entries.length > 100 ||
    first + entries.length > total ||
    !/^[a-f0-9]{64}$/.test(generation) ||
    entries.some((entry, index) => {
      const previous = entries[index - 1];
      return (
        entry.slot !== undefined && previous?.slot !== undefined && entry.slot <= previous.slot
      );
    })
  )
    throw Error('Annotation response exceeds its bound');
  return {
    source: text(row.source_key),
    key: text(row.catalog_key),
    generation,
    scope: row.scope,
    kind: row.kind as 'errors' | 'signals',
    first,
    total,
    entries,
  };
}

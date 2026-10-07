import { array, object, text } from '../domain/validate';
export interface CatalogSourceItem {
  source_key: string;
  label: string;
}
export function parseCatalogSources(value: unknown): {
  api: 1;
  items: CatalogSourceItem[];
  next_cursor: string | null;
} {
  const row = object(value);
  if (row.api !== 1 || !Array.isArray(row.items) || row.items.length > 100)
    throw new Error('Invalid source inventory');
  const items = array(row.items, (value) => {
    const item = object(value),
      source_key = text(item.source_key),
      label = text(item.label);
    if (!source_key || !label) throw new Error('Invalid source identity');
    return { source_key, label };
  });
  if (new Set(items.map((item) => item.source_key)).size !== items.length)
    throw new Error('Repeated source inventory identity');
  const cursor = row.next_cursor === null ? null : text(row.next_cursor);
  if (cursor === '') throw new Error('Invalid source inventory cursor');
  return { api: 1, items, next_cursor: cursor };
}

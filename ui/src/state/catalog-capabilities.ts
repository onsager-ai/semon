import { boolean, object, text, array } from '../domain/validate';
export interface CatalogCapabilities {
  api: 1;
  read_contract: 'catalog-v1';
  source_key: string;
  selected_transcript: boolean;
  selected_identity: boolean;
  selected_entry: boolean;
  attachment: boolean;
  relationship_context: boolean;
  large_native_records: boolean;
  pagination: true;
  filters: string[];
  order: 'last_desc_key_asc';
  full_text_search: false;
  global_union: false;
}
/** Unknown contracts must not be normalized into either complete graph or bounded authority. */
export function parseCatalogCapabilities(value: unknown): CatalogCapabilities {
  const row = object(value), filters = array(row.filters, text);
  if (row.api !== 1 || row.read_contract !== 'catalog-v1' || row.pagination !== true ||
    row.order !== 'last_desc_key_asc' || row.full_text_search !== false || row.global_union !== false ||
    filters.some((filter) => filter !== 'harness' && filter !== 'repo') || new Set(filters).size !== filters.length)
    throw new Error('Unsupported catalog read contract');
  return {
    api: 1, read_contract: 'catalog-v1', source_key: text(row.source_key),
    selected_transcript: boolean(row.selected_transcript), selected_identity: boolean(row.selected_identity),
    selected_entry: boolean(row.selected_entry), attachment: boolean(row.attachment),
    relationship_context: boolean(row.relationship_context), large_native_records: boolean(row.large_native_records),
    pagination: true, filters, order: 'last_desc_key_asc', full_text_search: false, global_union: false,
  };
}

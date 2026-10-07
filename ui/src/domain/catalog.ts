export type CatalogReadScope = 'current' | 'retained_history';
export type CatalogNativeSelection = 'cached' | 'stale' | 'incomplete' | 'unavailable' | 'retired';
export type CatalogFreshness = 'cached' | 'stale' | 'incomplete' | 'unavailable';
export interface CatalogSourceReference {
  source: {
    root: string;
    path: string;
    native_id: string;
    offset: number;
    prefix_sha256: number[];
    tail_sha256: number[];
    immutable_generation?: string | null;
  };
  state: CatalogFreshness;
}
/** A cached source identity is neither runtime presence nor write authority. */
export interface CatalogSessionIdentity {
  source_key: string;
  read_scope: CatalogReadScope;
  catalog_key: string;
  harness: string;
  native_id: string | null;
  owner_qualification?: 'qualified' | 'provisional';
  native_ids: string[];
  source_refs: CatalogSourceReference[];
  machine_label: string | null;
  generation: string;
  observed_at: number | null;
  freshness: { state: CatalogFreshness };
  facts_observation?: { state: CatalogFreshness };
  native_selection?: { state: CatalogNativeSelection } | null;
}

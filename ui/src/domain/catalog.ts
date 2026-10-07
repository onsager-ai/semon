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
/** A cached source identity is neither runtime presence nor write authority. */
export interface CatalogSessionIdentity {
  source_key: string;
  catalog_key: string;
  harness: string;
  native_id: string | null;
  native_ids: string[];
  source_refs: CatalogSourceReference[];
  machine_label: string | null;
  generation: string;
  observed_at: number | null;
  freshness: { state: CatalogFreshness };
}

import type { CatalogSessionIdentity } from '../domain/catalog';
import { parseCatalogIdentity } from './catalog-wire';
export interface CatalogSelectionScope {
  source_key: string;
  catalog_key: string;
}
export interface CatalogSelectionRequest extends CatalogSelectionScope {
  readonly epoch: number;
}
/** Owns selected source identity independently of a complete graph/model baseline. */
export class CatalogSelectionStore {
  private epoch = 0;
  private disposed = false;
  private pending: CatalogSelectionRequest | null = null;
  private scope: Readonly<CatalogSelectionScope> | null = null;
  private identity: CatalogSessionIdentity | null = null;
  private readonly listeners = new Set<() => void>();
  begin(scope: CatalogSelectionScope): CatalogSelectionRequest {
    if (this.disposed) throw new Error('Catalog selection is destroyed');
    if (
      typeof scope.source_key !== 'string' ||
      typeof scope.catalog_key !== 'string' ||
      !scope.catalog_key
    )
      throw new Error('Invalid catalog selection');
    const request = Object.freeze({
      source_key: scope.source_key,
      catalog_key: scope.catalog_key,
      epoch: ++this.epoch,
    });
    this.pending = request;
    if (
      this.scope?.source_key !== scope.source_key ||
      this.scope?.catalog_key !== scope.catalog_key
    )
      this.scope = Object.freeze({ source_key: scope.source_key, catalog_key: scope.catalog_key });
    this.identity = null;
    this.changed();
    return request;
  }
  accept(request: CatalogSelectionRequest, value: unknown): boolean {
    if (request !== this.pending || request.epoch !== this.epoch) return false;
    const identity = parseCatalogIdentity(value);
    if (identity.read_scope !== 'current' || identity.native_selection?.state === 'retired')
      throw new Error('Historical identity cannot supply current catalog authority');
    if (identity.source_key !== request.source_key || identity.catalog_key !== request.catalog_key)
      throw new Error('Catalog selection scope mismatch');
    for (const ref of identity.source_refs) {
      Object.freeze(ref.source.prefix_sha256);
      Object.freeze(ref.source.tail_sha256);
      Object.freeze(ref.source);
      Object.freeze(ref);
    }
    Object.freeze(identity.source_refs);
    Object.freeze(identity.native_ids);
    Object.freeze(identity.freshness);
    if (identity.facts_observation) Object.freeze(identity.facts_observation);
    if (identity.native_selection) Object.freeze(identity.native_selection);
    this.identity = Object.freeze(identity);
    this.changed();
    return true;
  }
  selectedIdentity(): CatalogSessionIdentity | null {
    // Consumers cannot mutate the accepted identity or its provenance.
    return this.identity;
  }
  /** Read intent survives a failed current identity check; it grants no native authority. */
  selectedScope(): Readonly<CatalogSelectionScope> | null {
    return this.scope;
  }
  subscribe(listener: () => void): () => void {
    if (this.disposed) throw new Error('Catalog selection is destroyed');
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  clear(): void {
    ++this.epoch;
    this.pending = null;
    this.scope = null;
    this.identity = null;
    this.changed();
  }
  destroy(): void {
    this.disposed = true;
    this.listeners.clear();
    this.clear();
  }
  private changed(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

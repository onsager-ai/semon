import type { CatalogSelectionScope } from './catalog-selection';
import {
  parseCatalogTranscriptPage,
  type CatalogTranscriptPage,
  type CatalogTranscriptEntry,
} from './catalog-transcript-wire';
export interface CatalogRangeRequest extends CatalogSelectionScope {
  readonly epoch: number;
  readonly after: number | null;
  readonly limit: number;
  readonly generation: string | null;
}
export interface CatalogLoadedRange {
  first: number;
  end: number;
}
/** Owns only requested native ranges; metadata is never adopted as a complete graph. */
export class CatalogTranscriptStore {
  private epoch = 0;
  private contentVersion = 0;
  get revision(): number {
    return this.contentVersion;
  }
  private scope: CatalogSelectionScope | null = null;
  private disposed = false;
  private requests = new Set<CatalogRangeRequest>();
  private pages: CatalogTranscriptPage[] = [];
  private listeners = new Set<() => void>();
  select(scope: CatalogSelectionScope): void {
    if (this.disposed) throw new Error('Catalog transcript is destroyed');
    if (
      typeof scope.source_key !== 'string' ||
      typeof scope.catalog_key !== 'string' ||
      !scope.catalog_key
    )
      throw new Error('Invalid catalog transcript scope');
    ++this.epoch;
    ++this.contentVersion;
    this.scope = { ...scope };
    this.requests.clear();
    this.pages = [];
    this.changed();
  }
  request(after: number | null = null, limit = 60): CatalogRangeRequest {
    if (this.disposed || !this.scope) throw new Error('No selected catalog transcript');
    if (
      !Number.isSafeInteger(limit) ||
      limit < 1 ||
      limit > 100 ||
      (after !== null && (!Number.isSafeInteger(after) || after < 0))
    )
      throw new Error('Invalid catalog transcript range');
    const request = Object.freeze({
      ...this.scope,
      epoch: this.epoch,
      after,
      limit,
      generation: this.pages[0]?.projection.generation ?? null,
    });
    this.requests.add(request);
    return request;
  }
  accept(request: CatalogRangeRequest, value: unknown): boolean {
    if (this.disposed || !this.requests.has(request) || request.epoch !== this.epoch) return false;
    const page = parseCatalogTranscriptPage(value);
    const generation = this.pages[0]?.projection.generation ?? request.generation;
    const first =
      request.after === null
        ? Math.max(0, page.projection.total - request.limit)
        : Math.min(request.after, page.projection.total);
    if (
      page.identity.source_key !== request.source_key ||
      page.identity.catalog_key !== request.catalog_key ||
      page.range.first !== first ||
      page.range.end !== Math.min(first + request.limit, page.projection.total) ||
      (generation !== null && page.projection.generation !== generation)
    )
      throw new Error('Catalog transcript response does not match the requested range');
    const sameRange = this.pages.find(
      (old) => old.range.first === page.range.first && old.range.end === page.range.end,
    );
    if (
      sameRange &&
      sameRange.projection.total === page.projection.total &&
      JSON.stringify(sameRange.entries) === JSON.stringify(page.entries)
    ) {
      this.requests.delete(request);
      this.pages[this.pages.indexOf(sameRange)] = page;
      return true;
    }
    const existing = new Map<number, CatalogTranscriptEntry>(),
      ids = new Map<string, number>();
    for (const loaded of this.pages) {
      if (loaded.projection.total !== page.projection.total)
        throw new Error('Catalog transcript total changed within a projection');
      for (const item of loaded.entries) {
        existing.set(item.slot, item);
        ids.set(item.entry_id, item.slot);
      }
    }
    for (const item of page.entries) {
      const old = existing.get(item.slot);
      if (
        (ids.has(item.entry_id) && ids.get(item.entry_id) !== item.slot) ||
        (old && JSON.stringify(old) !== JSON.stringify(item))
      )
        throw new Error('Catalog transcript entry changed within a projection');
    }
    this.requests.delete(request);
    this.pages = [
      ...this.pages.filter((old) => old.range.first !== first || old.range.end !== page.range.end),
      page,
    ].sort((a, b) => a.range.first - b.range.first);
    ++this.contentVersion;
    this.changed();
    return true;
  }
  reject(request: CatalogRangeRequest): void {
    this.requests.delete(request);
  }
  selectedPage(): CatalogTranscriptPage | null {
    return this.pages.at(-1) ?? null;
  }
  loadedRanges(): CatalogLoadedRange[] {
    const ranges: CatalogLoadedRange[] = [];
    for (const page of this.pages) {
      const last = ranges.at(-1);
      if (last && page.range.first <= last.end) last.end = Math.max(last.end, page.range.end);
      else ranges.push({ first: page.range.first, end: page.range.end });
    }
    return ranges;
  }
  entries(): CatalogTranscriptEntry[] {
    const bySlot = new Map<number, CatalogTranscriptEntry>();
    for (const page of this.pages)
      for (const item of page.entries) if (!bySlot.has(item.slot)) bySlot.set(item.slot, item);
    return [...bySlot.values()].sort((a, b) => a.slot - b.slot);
  }
  subscribe(listener: () => void): () => void {
    if (this.disposed) throw new Error('Catalog transcript is destroyed');
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  destroy(): void {
    this.disposed = true;
    ++this.epoch;
    this.requests.clear();
    this.pages = [];
    this.scope = null;
    this.listeners.clear();
  }
  private changed(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

import { ApiError } from '../lib/api';
import type { CatalogRecovery } from './catalog-recovery';
import type { CatalogSelectionScope } from './catalog-selection';
import type { CatalogReadScope } from '../domain/catalog';
import type { NativeEntryAddress } from '../lib/sessionRoute';
import {
  parseCatalogNativeTranscriptPage,
  parseCatalogTranscriptPage,
  type CatalogNativeTranscriptPage,
  type CatalogTranscriptPage,
  type CatalogTranscriptEntry,
} from './catalog-transcript-wire';
function content(item: CatalogTranscriptEntry): string {
  const { freshness, ...body } = item;
  return JSON.stringify(body);
}
function emptyObservedBody(item: CatalogTranscriptEntry): boolean {
  const entry = item.entry;
  return entry.k === 'tool'
    ? !entry.out
    : entry.k === 'h'
      ? !item.native_action_text
      : (entry.k === 'u' || entry.k === 'a' || entry.k === 'think') && !entry.text;
}
function missingObservedBody(item: CatalogTranscriptEntry): boolean {
  return (
    (item.freshness?.state === 'incomplete' || item.freshness?.state === 'unavailable') &&
    emptyObservedBody(item)
  );
}
function bodyMetadata(item: CatalogTranscriptEntry): string {
  const { freshness, field, clipped, native_action_text, entry, ...identity } = item;
  if (entry.k === 'tool') {
    const { out, ...recorded } = entry;
    return JSON.stringify({ ...identity, entry: recorded });
  }
  if (entry.k === 'u' || entry.k === 'a' || entry.k === 'think') {
    const { text, ...recorded } = entry;
    return JSON.stringify({ ...identity, entry: recorded });
  }
  return JSON.stringify({ ...identity, entry });
}
function mergeRelationshipContext(
  pages: readonly Pick<CatalogTranscriptPage, 'relationship_context'>[],
): CatalogTranscriptPage['relationship_context'] {
  const handoffs = new Map<
    string,
    CatalogTranscriptPage['relationship_context']['handoffs'][number]
  >();
  const sessions: CatalogTranscriptPage['relationship_context']['sessions'] = {};
  const previews: Record<string, string> = {};
  const turns = new Map<string, CatalogTranscriptPage['relationship_context']['turns'][number]>();
  for (const page of pages) {
    for (const relationship of page.relationship_context.handoffs)
      handoffs.set(relationship.edge.id, relationship);
    Object.assign(sessions, page.relationship_context.sessions);
    Object.assign(previews, page.relationship_context.previews);
    for (const turn of page.relationship_context.turns) {
      const previous = turns.get(turn.id);
      turns.set(
        turn.id,
        previous
          ? {
              ...(turn.at == null && previous.at != null ? previous : turn),
              sent: [...new Set([...previous.sent, ...turn.sent])],
            }
          : turn,
      );
    }
  }
  return {
    state: pages.some((page) => page.relationship_context.state === 'incomplete')
      ? 'incomplete'
      : 'complete',
    turn_ids: [...new Set(pages.flatMap((page) => page.relationship_context.turn_ids))],
    turns: [...turns.values()],
    handoffs: [...handoffs.values()],
    sessions,
    previews,
  };
}
export interface CatalogRangeRequest extends CatalogSelectionScope {
  readonly epoch: number;
  readonly after: number | null;
  readonly limit: number;
  readonly generation: string | null;
  readonly errorsGeneration: string | null;
  readonly read_scope: CatalogReadScope;
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
  private readScope: CatalogReadScope = 'current';
  private disposed = false;
  private requests = new Set<CatalogRangeRequest>();
  private pages: CatalogTranscriptPage[] = [];
  private access = new Map<string, number>();
  private pageKey(page: CatalogTranscriptPage) {
    return `${page.range.first}:${page.range.end}`;
  }
  private clock = 0;
  private listeners = new Set<() => void>();
  select(scope: CatalogSelectionScope, readScope: CatalogReadScope = 'current'): void {
    if (this.disposed) throw new Error('Catalog transcript is destroyed');
    if (
      typeof scope.source_key !== 'string' ||
      !scope.source_key ||
      typeof scope.catalog_key !== 'string' ||
      !scope.catalog_key ||
      (readScope !== 'current' && readScope !== 'retained_history')
    )
      throw new Error('Invalid catalog transcript scope');
    ++this.epoch;
    ++this.contentVersion;
    this.scope = { ...scope };
    this.readScope = readScope;
    this.requests.clear();
    this.pages = [];
    this.access.clear();
    this.changed();
  }
  request(
    after: number | null = null,
    limit = 60,
    expectedGeneration?: string,
  ): CatalogRangeRequest {
    if (this.disposed || !this.scope) throw new Error('No selected catalog transcript');
    if (
      !Number.isSafeInteger(limit) ||
      limit < 1 ||
      limit > 100 ||
      (after !== null && (!Number.isSafeInteger(after) || after < 0)) ||
      (expectedGeneration !== undefined && !/^[a-f0-9]{64}$/.test(expectedGeneration))
    )
      throw new Error('Invalid catalog transcript range');
    const request = Object.freeze({
      ...this.scope,
      epoch: this.epoch,
      after,
      limit,
      generation: expectedGeneration ?? this.pages[0]?.projection.generation ?? null,
      errorsGeneration: this.pages[0]?.projection.errorsGeneration ?? null,
      read_scope: this.readScope,
    });
    this.requests.add(request);
    return request;
  }
  accept(request: CatalogRangeRequest, value: unknown): boolean {
    if (this.disposed || !this.requests.has(request) || request.epoch !== this.epoch) return false;
    const page = parseCatalogTranscriptPage(value);
    const generation = this.pages[0]?.projection.generation ?? request.generation;
    const errorsGeneration = this.pages[0]?.projection.errorsGeneration ?? request.errorsGeneration;
    if (errorsGeneration !== null && page.projection.errorsGeneration !== errorsGeneration)
      throw Object.assign(new Error('Catalog transcript runtime observation changed'), {
        status: 409,
      });
    const first =
      request.after === null
        ? Math.max(0, page.projection.total - request.limit)
        : Math.min(request.after, page.projection.total);
    if (
      page.identity.source_key !== request.source_key ||
      page.identity.catalog_key !== request.catalog_key ||
      page.identity.read_scope !== request.read_scope ||
      page.range.first !== first ||
      page.range.end !== Math.min(first + request.limit, page.projection.total) ||
      (generation !== null && page.projection.generation !== generation)
    )
      throw new Error('Catalog transcript response does not match the requested range');
    this.access.set(this.pageKey(page), ++this.clock);
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
      if (
        JSON.stringify(sameRange.relationship_context) !==
          JSON.stringify(page.relationship_context) ||
        sameRange.session.details?.live_observation !== page.session.details?.live_observation ||
        sameRange.projection.errorsGeneration !== page.projection.errorsGeneration
      ) {
        ++this.contentVersion;
        this.changed();
      }
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
    page.entries = page.entries.map((item) => {
      const old = existing.get(item.slot);
      // Failed source verification must remain visible without erasing useful retained content.
      if (
        old &&
        missingObservedBody(item) &&
        old.entry_id === item.entry_id &&
        old.entry.k === item.entry.k &&
        JSON.stringify(old.provenance) === JSON.stringify(item.provenance)
      )
        return { ...old, freshness: item.freshness };
      return item;
    });
    for (const item of page.entries) {
      const old = existing.get(item.slot);
      if (
        (ids.has(item.entry_id) && ids.get(item.entry_id) !== item.slot) ||
        (old &&
          content(old) !== content(item) &&
          !(missingObservedBody(old) && bodyMetadata(old) === bodyMetadata(item)))
      )
        throw new Error('Catalog transcript entry changed within a projection');
    }
    if (sameRange && JSON.stringify(sameRange.entries) === JSON.stringify(page.entries)) {
      this.requests.delete(request);
      this.pages[this.pages.indexOf(sameRange)] = page;
      if (
        JSON.stringify(sameRange.relationship_context) !== JSON.stringify(page.relationship_context)
      ) {
        ++this.contentVersion;
        this.changed();
      }
      return true;
    }
    const observed = new Map(page.entries.map((item) => [item.slot, item]));
    this.requests.delete(request);
    this.pages = [
      ...this.pages
        .filter((old) => old.range.first !== first || old.range.end !== page.range.end)
        .map((old) => ({
          ...old,
          entries: old.entries.map((item) => observed.get(item.slot) ?? item),
        })),
      page,
    ].sort((a, b) => a.range.first - b.range.first);
    while (this.pages.length > 5 || this.entries().length > 300) {
      const oldest = this.pages.reduce((a, b) =>
        (this.access.get(this.pageKey(a)) ?? 0) < (this.access.get(this.pageKey(b)) ?? 0) ? a : b,
      );
      this.pages = this.pages.filter((value) => value !== oldest);
      this.access.delete(this.pageKey(oldest));
    }
    ++this.contentVersion;
    this.changed();
    return true;
  }
  reject(request: CatalogRangeRequest): void {
    this.requests.delete(request);
  }
  retainRanges(ranges: readonly CatalogLoadedRange[]): void {
    this.pages = this.pages.filter((page) =>
      ranges.some((range) => page.range.first >= range.first && page.range.end <= range.end),
    );
    const keys = new Set(this.pages.map((page) => this.pageKey(page)));
    for (const key of this.access.keys()) if (!keys.has(key)) this.access.delete(key);
    ++this.contentVersion;
    this.changed();
  }
  selectedPage(): CatalogTranscriptPage | null {
    return this.pages.at(-1) ?? null;
  }
  incompleteRange(): CatalogLoadedRange | null {
    const page = this.pages.find((page) =>
      page.entries.some(
        (entry) =>
          entry.freshness?.state === 'incomplete' || entry.freshness?.state === 'unavailable',
      ),
    );
    return page ? { first: page.range.first, end: page.range.end } : null;
  }
  context(): CatalogTranscriptPage['relationship_context'] {
    return mergeRelationshipContext(this.pages);
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
    this.access.clear();
    this.scope = null;
    this.listeners.clear();
  }
  private changed(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

export type CatalogNativeWindow =
  | { position: 'latest' }
  | { position: 'earlier'; cursor: string }
  | { position: 'address'; address: NativeEntryAddress };

/** Source-scoped native windows, ordered by the server's committed continuations. */
export class CatalogNativeTranscriptStore {
  private pending?: CatalogNativeWindow;
  private pages: CatalogNativeTranscriptPage[] = [];
  private disposed = false;
  revision = 0;

  constructor(
    private readonly scope: CatalogSelectionScope,
    private readonly readScope: CatalogReadScope,
  ) {}

  request(window: CatalogNativeWindow): CatalogNativeWindow {
    if (this.disposed || (window.position === 'earlier' && window.cursor !== this.earlierCursor()))
      throw new Error('Committed transcript request is outside its loaded view');
    const request = Object.freeze({ ...window });
    this.pending = request;
    return request;
  }

  accept(request: CatalogNativeWindow, value: unknown): boolean {
    if (this.disposed || this.pending !== request) return false;
    const page = parseCatalogNativeTranscriptPage(value);
    if (
      page.identity.source_key !== this.scope.source_key ||
      page.identity.catalog_key !== this.scope.catalog_key ||
      page.identity.read_scope !== this.readScope ||
      page.position !== request.position
    )
      throw new Error('Committed transcript response belongs to another selection');
    if (request.position === 'earlier') {
      if (page.binding !== this.selectedPage()?.binding)
        throw Object.assign(new Error('Committed transcript selection changed'), { status: 409 });
      const present = new Set(this.entries().map(({ entry_id }) => entry_id));
      if (
        page.entries.some(({ entry_id }) => present.has(entry_id)) ||
        page.earlierCursor === request.cursor
      )
        throw new Error('Committed transcript continuation did not advance');
      this.pages.unshift(page);
      this.pages.length = Math.min(this.pages.length, 5);
    } else if (request.position === 'address') {
      if (
        page.binding !== request.address.binding ||
        !page.entries.some(
          ({ address }) => JSON.stringify(address) === JSON.stringify(request.address),
        )
      )
        throw new Error('Committed transcript response omitted its exact address');
      this.pages = [page];
    } else {
      const latest = this.selectedPage();
      const start = page.entries[0]?.entry_id;
      const overlap =
        latest?.binding === page.binding && start
          ? this.pages.findIndex((previous) =>
              previous.entries.some(({ entry_id }) => entry_id === start),
            )
          : -1;
      if (overlap >= 0) {
        const previous = this.pages[overlap];
        const keep = previous.entries.findIndex(({ entry_id }) => entry_id === start);
        this.pages = this.pages.slice(0, overlap);
        if (keep > 0) {
          const entries = previous.entries.slice(0, keep),
            context = previous.relationship_context,
            turns = new Set(entries.flatMap(({ entry }) => (entry.turn ? [entry.turn] : []))),
            actions = new Set(entries.flatMap(({ entry }) => (entry.k === 'h' ? [entry.id] : []))),
            retainedTurns = context.turns.filter((turn) => turns.has(turn.id));
          for (const turn of retainedTurns) {
            if (turn.start) actions.add(turn.start);
            for (const id of turn.sent) actions.add(id);
          }
          this.pages.push({
            ...previous,
            entries,
            relationship_context: {
              ...context,
              turn_ids: context.turn_ids.filter((id) => turns.has(id)),
              turns: retainedTurns,
              handoffs: context.handoffs.filter(({ edge }) => actions.has(edge.id)),
            },
          });
        } else if (!this.pages.length) page.earlierCursor = previous.earlierCursor;
        this.pages = [...this.pages, page].slice(-5);
      } else {
        this.pages = [page];
      }
    }
    this.pending = undefined;
    ++this.revision;
    return true;
  }

  reject(request: CatalogNativeWindow): void {
    if (this.pending === request) this.pending = undefined;
  }

  earlierCursor(): string | null {
    return this.pages[0]?.earlierCursor ?? null;
  }

  selectedPage(): CatalogNativeTranscriptPage | null {
    return this.pages.at(-1) ?? null;
  }

  context(): CatalogTranscriptPage['relationship_context'] {
    return mergeRelationshipContext(this.pages);
  }

  entries(): CatalogNativeTranscriptPage['entries'] {
    return this.pages.flatMap((page) => page.entries);
  }

  destroy(): void {
    this.disposed = true;
    this.pending = undefined;
    this.pages = [];
  }
}

/** Read one native window and schedule recovery while the selection is current. */
export async function readNativeTranscript(
  store: CatalogNativeTranscriptStore,
  window: CatalogNativeWindow,
  options: {
    read(timeoutMs: number): Promise<unknown>;
    current(): boolean;
    recovery: CatalogRecovery;
    retry(delay: number, error: ApiError | undefined): void;
  },
): Promise<boolean> {
  const initial = !store.selectedPage();
  const timeout = initial ? Math.min(15_000, options.recovery.remaining()) : 15_000;
  if (initial && options.recovery.stopped) return false;
  const request = store.request(window);
  try {
    const value = await options.read(timeout);
    return (
      options.current() && (!initial || !options.recovery.stopped) && store.accept(request, value)
    );
  } catch (error) {
    if (!options.current()) return false;
    const response = error instanceof ApiError ? error : undefined;
    // The reader can recover a stale saved address by opening the latest page.
    if (window.position === 'address' && response?.code === 'stale_cursor') throw error;
    if (initial ? response?.retryable === true : response?.code === 'retained_source_preparing') {
      const delay = initial
        ? options.recovery.fail(Date.now(), Math.random(), 5)
        : options.recovery.fail();
      if (delay !== null) {
        options.retry(delay, response);
        return false;
      }
    }
    throw error;
  } finally {
    store.reject(request);
  }
}

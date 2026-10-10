import { ViewUpdates } from './viewUpdates';
import type { Entry, TranscriptMeta, Turn } from '../domain/types';
import { TranscriptCache } from '../lib/routes';
import { createPagingStore } from '../lib/live';
import { parseTranscriptPage } from './transcript-wire';
export type PageDirection = 'before' | 'after';
export interface TranscriptHost {
  request(path: string, signal?: AbortSignal): Promise<unknown>;
  turns(sid: string): Turn[];
  turn(id: string): Turn | undefined;
  entry(entry: Entry): Entry;
  cleared(sid: string): void;
}
/** Range identity is the transaction token; replaced ranges cannot receive old pages. */
export class TranscriptStore {
  readonly entries: Record<string, Entry[]> = Object.create(null);
  readonly meta: Record<string, TranscriptMeta> = Object.create(null);
  marks: Record<string, string> = Object.create(null);
  readonly staleBriefs = new Set<string>();
  readonly cache = new TranscriptCache<Entry, TranscriptMeta>();
  readonly paging = createPagingStore();
  private readonly requests = new Set<AbortController>();
  private readonly replacements = new Map<string, AbortController>();
  private disposed = false;
  readonly updates: ViewUpdates;
  private readonly ownsUpdates: boolean;
  constructor(
    private readonly host: TranscriptHost,
    updates?: ViewUpdates,
  ) {
    this.ownsUpdates = updates === undefined;
    this.updates = updates ?? new ViewUpdates();
  }
  spread(sid: string) {
    for (const turn of this.host.turns(sid)) turn.entries = [];
    let turn: Turn | undefined,
      pre = 0;
    for (const entry of this.entries[sid] ?? []) {
      if (entry.turn) turn = this.host.turn(entry.turn);
      if (turn && turn.sid === sid) {
        entry.key = turn.id + '#' + turn.entries.length;
        turn.entries.push(entry);
      } else entry.key = sid + '#' + pre++;
    }
  }
  clearPaging(sid: string) {
    this.paging.clear(sid);
    this.host.cleared(sid);
  }
  private cancelReplacement(sid: string) {
    this.replacements.get(sid)?.abort();
    this.replacements.delete(sid);
  }
  drop(sid: string) {
    this.cancelReplacement(sid);
    this.clearPaging(sid);
    delete this.entries[sid];
    delete this.meta[sid];
    this.updates.transcript(sid);
  }
  async fetch(
    sid: string,
    query = '',
    direction?: PageDirection,
    signal?: AbortSignal,
    onPage?: () => void,
  ) {
    if (this.disposed || signal?.aborted) return;
    const controller = new AbortController(),
      abort = () => controller.abort();
    signal?.addEventListener('abort', abort, { once: true });
    this.requests.add(controller);
    if (!direction) {
      this.cancelReplacement(sid);
      this.replacements.set(sid, controller);
    }
    const mark = this.marks[sid],
      range = this.meta[sid],
      boundary = direction === 'before' ? range?.from : range?.to;
    try {
      const response = await this.host.request(
        '/api/tx?sid=' + encodeURIComponent(sid) + (query ? '&' + query : ''),
        controller.signal,
      );
      if (
        this.disposed ||
        controller.signal.aborted ||
        (direction &&
          (this.meta[sid] !== range ||
            (direction === 'before' ? this.meta[sid]?.from : this.meta[sid]?.to) !== boundary))
      )
        return;
      const page = parseTranscriptPage(response),
        entries = page.entries.map((entry, index) =>
          this.host.entry({ ...entry, sid, slot: entry.slot ?? page.from + index }),
        ),
        meta = this.meta[sid];
      if (direction === 'before' && meta) {
        this.entries[sid] = entries.concat(this.entries[sid]);
        meta.from = page.from;
      } else if (direction === 'after' && meta) {
        this.entries[sid] = this.entries[sid].concat(entries);
        meta.to = page.to;
      } else {
        this.clearPaging(sid);
        this.entries[sid] = entries;
        this.meta[sid] = { from: page.from, to: page.to, total: page.total };
        this.staleBriefs.delete(sid);
      }
      Object.assign(this.meta[sid], {
        total: page.total,
        calls: page.calls,
        errors: page.errors,
        watchTok: mark,
      });
      if (direction !== 'before' && page.to >= page.total) {
        this.meta[sid].tok = mark;
        this.meta[sid].newer = 0;
      }
      this.spread(sid);
      this.updates.transcript(sid);
      onPage?.();
    } catch (error) {
      if (!this.disposed && !controller.signal.aborted) throw error;
    } finally {
      this.requests.delete(controller);
      signal?.removeEventListener('abort', abort);
      if (this.replacements.get(sid) === controller) this.replacements.delete(sid);
    }
  }
  keep(
    sid: string,
    entries: Entry[] | undefined,
    meta: TranscriptMeta | undefined,
    origin: boolean,
  ) {
    if (
      !entries ||
      !meta ||
      meta.to < meta.total ||
      meta.tok == null ||
      this.staleBriefs.has(sid) ||
      meta.origin !== origin
    )
      return;
    this.cache.keep(sid, entries, meta);
  }
  adoptCached(sid: string, turn?: string): boolean {
    const cached = this.cache.get(sid);
    if (!cached) return false;
    this.cancelReplacement(sid);
    this.cache.delete(sid);
    this.entries[sid] = cached.entries;
    this.meta[sid] = cached.meta;
    this.spread(sid);
    if (!turn) {
      this.updates.transcript(sid);
      return true;
    }
    const target = this.host.turn(turn);
    if (target && target.sid === sid && !target.entries.length) {
      this.drop(sid);
      return false;
    }
    this.updates.transcript(sid);
    return true;
  }
  /** A coherent projection borrows accepted entries; no full-transcript copies. */
  view(sid: string) {
    return { entries: this.entries[sid], range: this.meta[sid], revision: this.updates.revision };
  }
  destroy() {
    if (this.disposed) return;
    this.disposed = true;
    if (this.ownsUpdates) this.updates.destroy();
    for (const request of this.requests) request.abort();
    this.requests.clear();
    this.replacements.clear();
    this.paging.destroy();
    this.cache.clear();
    this.staleBriefs.clear();
    for (const id of Object.keys(this.entries)) delete this.entries[id];
    for (const id of Object.keys(this.meta)) delete this.meta[id];
  }
}

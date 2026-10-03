import type { PagingState } from './live';
export type PagingDirection = 'before' | 'after';
export interface PagingRange {
  from: number;
  to: number;
  total: number;
}
export interface PagerHost<Route extends object, Range extends PagingRange> {
  route(sid: string): Route | null;
  range(sid: string): Range | undefined;
  state(sid: string, direction: PagingDirection): PagingState;
  current(sid: string, direction: PagingDirection, state: PagingState, route: Route): boolean;
  automatic(): boolean;
  beginManual(): void;
  countAutomatic(): void;
  paint(button: HTMLButtonElement): void;
  load(
    sid: string,
    direction: PagingDirection,
    boundary: number,
    signal: AbortSignal,
    applied: () => void,
  ): Promise<void>;
  commit(route: Route, direction: PagingDirection, manual: boolean): void;
  queue(): void;
}
export function createPagerController<Route extends object, Range extends PagingRange>(
  host: PagerHost<Route, Range>,
) {
  let disposed = false,
    frame: number | null = null;
  const observers = new Map<HTMLButtonElement, IntersectionObserver>();
  const requests = new Set<AbortController>();
  function disconnect() {
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    for (const observer of observers.values()) observer.disconnect();
    observers.clear();
  }
  async function load(button: HTMLButtonElement, manual: boolean) {
    const sid = button.dataset.pagerSid,
      direction = button.dataset.pagerWhere;
    if (
      disposed ||
      !sid ||
      (direction !== 'before' && direction !== 'after') ||
      !button.isConnected
    )
      return;
    const route = host.route(sid),
      state = host.state(sid, direction);
    if (!route || state.busy || (!manual && (state.failed || !host.automatic()))) return;
    if (manual) host.beginManual();
    const range = host.range(sid);
    if (!range || (direction === 'before' ? range.from <= 0 : range.to >= range.total)) return;
    const boundary = direction === 'before' ? range.from : range.to;
    if (!manual) host.countAutomatic();
    state.busy = true;
    state.controller = new AbortController();
    const controller = state.controller;
    requests.add(controller);
    host.paint(button);
    try {
      let applied = false;
      await host.load(sid, direction, boundary, controller.signal, () => {
        applied = true;
      });
      if (
        disposed ||
        !applied ||
        !host.current(sid, direction, state, route) ||
        host.range(sid) !== range
      )
        return;
      state.failed = false;
      if (host.route(sid) === route) host.commit(route, direction, manual);
    } catch (error) {
      if (
        !(error instanceof Error && error.name === 'AbortError') &&
        host.current(sid, direction, state, route)
      )
        state.failed = true;
    } finally {
      requests.delete(controller);
      state.busy = false;
      state.controller = null;
      if (!disposed && host.current(sid, direction, state, route)) {
        for (const next of document.querySelectorAll<HTMLButtonElement>('#page [data-pager-where]'))
          if (next.dataset.pagerSid === sid && next.dataset.pagerWhere === direction)
            host.paint(next);
        host.queue();
      }
    }
  }
  function queue(root: HTMLElement, scrollRoot: HTMLElement | null, enabled: () => boolean) {
    if (disposed || frame !== null) return;
    frame = requestAnimationFrame(() => {
      frame = null;
      disconnect();
      if (
        disposed ||
        !root.isConnected ||
        !enabled() ||
        typeof IntersectionObserver === 'undefined'
      )
        return;
      for (const button of root.querySelectorAll<HTMLButtonElement>('[data-pager-where]')) {
        const sid = button.dataset.pagerSid,
          direction = button.dataset.pagerWhere;
        if (!sid || (direction !== 'before' && direction !== 'after')) continue;
        const state = host.state(sid, direction);
        if (state.busy || state.failed) continue;
        const observer = new IntersectionObserver(
          (entries) => {
            if (observers.get(button) === observer && entries.some((entry) => entry.isIntersecting))
              void load(button, false);
          },
          {
            root: scrollRoot,
            rootMargin: direction === 'before' ? '800px 0px 0px 0px' : '0px 0px 800px 0px',
          },
        );
        observers.set(button, observer);
        observer.observe(button);
      }
    });
  }
  return {
    load,
    queue,
    disconnect,
    destroy() {
      if (disposed) return;
      disposed = true;
      for (const controller of requests) controller.abort();
      requests.clear();
      disconnect();
    },
  };
}

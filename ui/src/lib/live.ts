import type { ViewerRoute } from './routes';
export interface LiveState { anchor?: { id: string; off: number; route: ViewerRoute; top: number } | null; late: string | null; lateTries: number; retry: boolean; version: string | null; timer: number | null; due: number; busy: boolean; started: number; delay: number; ended: boolean; again: boolean; pending: boolean; fresh: number; turns: Map<string,string> }
export interface LiveHost { poll(): Promise<void>; failed(error: unknown): boolean; ended(status: number): void }
export function createLiveController(host: LiveHost) {
  const state: LiveState = { late: null, lateTries: 0, retry: false, version: null, timer: null, due: 0, busy: false, started: -Infinity, delay: 2000, ended: false, again: false, pending: false, fresh: 0, turns: new Map() };
  let disposed = false;
  const visible = () => document.visibilityState === 'visible', floorWait = () => Math.max(0,state.started + 1000 - performance.now());
  function cancel() { if (state.timer !== null) clearTimeout(state.timer); state.timer = null; }
  function schedule(ms: number) { cancel(); if (!disposed && !state.ended && visible()) { state.due = performance.now() + ms; state.timer = window.setTimeout(poll,ms); } }
  function refresh() { if (disposed || !state.version || state.ended) return; if (state.busy) { state.again = true; return; } const wait = floorWait(); if (!state.timer || performance.now() + wait < state.due) schedule(wait); }
  async function poll() {
    state.timer = null; if (disposed || state.busy || state.ended || !visible()) return; state.busy = true; state.started = performance.now(); state.retry = false; let ok = false;
    try { await host.poll(); state.delay = state.retry ? Math.min(30000,state.delay * 2) : 2000; ok = true; }
    catch (error) { const status = typeof error === 'object' && error !== null && 'status' in error ? error.status : undefined; if (host.failed(error)) { state.ended = true; cancel(); } else if (status === 403) host.ended(403); else { state.delay = Math.min(30000,state.delay * 2); if (status === undefined) window.setTimeout(() => { throw error; }); } }
    finally { state.busy = false; if (!disposed) { schedule(state.again ? floorWait() : state.delay); state.again = false; window.dispatchEvent(new CustomEvent('semon:polled',{ detail: { ok } })); } }
  }
  function visibility() { if (!visible()) cancel(); else if (state.version && !state.busy && !state.timer) schedule(state.delay > 2000 ? state.delay : 0); }
  document.addEventListener('visibilitychange',visibility); window.addEventListener('semon:refresh',refresh);
  return { state, schedule, stop() { state.ended = true; cancel(); }, destroy() { if (disposed) return; disposed = true; cancel(); document.removeEventListener('visibilitychange',visibility); window.removeEventListener('semon:refresh',refresh); } };
}
export interface PagingState { busy: boolean; failed: boolean; controller?: AbortController | null }
export function createPagingStore() {
  const states = new Map<string, { before: PagingState; after: PagingState }>();
  return { states, get(sid: string, direction: 'before' | 'after') { let pair = states.get(sid); if (!pair) { pair = { before: { busy: false, failed: false }, after: { busy: false, failed: false } }; states.set(sid,pair); } return pair[direction]; }, clear(sid: string) { for (const state of Object.values(states.get(sid) ?? {})) state.controller?.abort(); states.delete(sid); }, destroy() { for (const sid of states.keys()) this.clear(sid); } };
}

export type ViewerRoute = { v: 'home' | 'analytics' | 'sessions' | 'machines' } | { v: 'machine'; id: string } | { v: 'session'; id: string; turn?: string } | { v: 'trace'; sid: string; turn: string };
export interface RouteModel { session(id: string): { harness: string } | undefined; machine(id: string): boolean; turn(id: string): { id: string; sid: string } | undefined; machinesPath?: string }
export function routeUrl(route: ViewerRoute, model: RouteModel): string {
  const enc = encodeURIComponent, harness = (id: string) => model.session(id)?.harness ?? 'claude';
  switch (route.v) { case 'home': return '/'; case 'analytics': return '/analytics'; case 'sessions': return '/sessions'; case 'machines': return model.machinesPath ?? '/machines'; case 'machine': return '/machines/' + enc(route.id); case 'session': return '/s/' + harness(route.id) + '/' + enc(route.id) + (route.turn ? '?turn=' + enc(route.turn) : ''); case 'trace': return '/trace/' + harness(route.sid) + '/' + enc(route.sid) + '/' + enc(route.turn); }
}
export function parseRoute(location: Pick<Location,'pathname' | 'search' | 'hash'>, model: RouteModel): ViewerRoute {
  if (model.machinesPath && location.pathname === model.machinesPath) return { v: 'machines' };
  const decode = (text: string) => { try { return decodeURIComponent(text); } catch { return text; } }, parts = location.pathname.split('/').filter(Boolean).map(decode), sid = parts[2], turn = new URLSearchParams(location.search).get('turn');
  if (parts[0] === 'timeline' || parts[0] === 'analytics') return { v: 'analytics' };
  if (parts[0] === 'sessions') return { v: 'sessions' };
  if (parts[0] === 'machines') return parts[1] && model.machine(parts[1]) ? { v: 'machine', id: parts[1] } : { v: 'machines' };
  if (parts[0] === 's' && sid && model.session(sid)) { const fragmentTurn = model.turn(decode(location.hash.slice(1)).split('#')[0] ?? ''), target = turn ?? (fragmentTurn?.sid === sid ? fragmentTurn.id : null); return target ? { v: 'session', id: sid, turn: target } : { v: 'session', id: sid }; }
  if (parts[0] === 'trace' && sid && model.session(sid) && parts[3]) return { v: 'trace', sid, turn: parts[3] };
  return { v: 'home' };
}
export class TranscriptCache<Entry extends object, Meta> extends Map<string,{ entries: Entry[]; meta: Meta; bytes: number }> {
  constructor(private readonly maxEntries = 5, private readonly maxBytes = 2 * 1024 * 1024) { super(); }
  keep(sid: string, entries: Entry[], meta: Meta) {
    this.delete(sid); let weight = 0;
    for (const entry of entries) for (const value of Object.values(entry)) weight += typeof value === 'string' ? value.length : value && typeof value === 'object' ? JSON.stringify(value).length : 4;
    const bytes = weight * 2; if (bytes > this.maxBytes) return; this.set(sid,{ entries,meta,bytes });
    let sum = 0; for (const record of this.values()) sum += record.bytes;
    for (const [id,record] of this) { if (this.size <= this.maxEntries && sum <= this.maxBytes) break; this.delete(id); sum -= record.bytes; }
  }
}

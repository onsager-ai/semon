import type { Handoff } from '../domain/types';
interface SeenPersistenceHost {
  isResult: (h: import('../domain/types').Handoff) => boolean;
  SEEN_RESULTS: Set<string>;
  SEEN_LIMIT: number;
  SEEN_KEY: string;
}
/** Owns seenPersistence behavior through explicit application ports. */
export function createSeenPersistence(host: SeenPersistenceHost) {
  function markSeenResults(handoffs: Iterable<Handoff>) {
    let changed = false;
    for (const h of handoffs)
      if (host.isResult(h) && typeof h.id === 'string' && !host.SEEN_RESULTS.has(h.id)) {
        host.SEEN_RESULTS.add(h.id);
        changed = true;
      }
    while (host.SEEN_RESULTS.size > host.SEEN_LIMIT)
      host.SEEN_RESULTS.delete(host.SEEN_RESULTS.values().next().value!);
    if (changed)
      try {
        window.localStorage.setItem(host.SEEN_KEY, JSON.stringify([...host.SEEN_RESULTS]));
      } catch {}
  }

  return { markSeenResults };
}

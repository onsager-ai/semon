import type { createDomain } from '../domain/calculations';
import type { createSeenResults } from './seenResults';
import type { Handoff } from '../domain/types';
interface SeenPersistenceHost {
  seenResultsOwner: Pick<
    ReturnType<typeof createSeenResults>,
    'SEEN_RESULTS' | 'SEEN_LIMIT' | 'SEEN_KEY'
  >;

  domain: Pick<ReturnType<typeof createDomain>, 'isResult'>;
}
/** Owns seenPersistence behavior through explicit application ports. */
export function createSeenPersistence(host: SeenPersistenceHost) {
  function markSeenResults(handoffs: Iterable<Handoff>) {
    let changed = false;
    for (const h of handoffs)
      if (
        host.domain.isResult(h) &&
        typeof h.id === 'string' &&
        !host.seenResultsOwner.SEEN_RESULTS.has(h.id)
      ) {
        host.seenResultsOwner.SEEN_RESULTS.add(h.id);
        changed = true;
      }
    while (host.seenResultsOwner.SEEN_RESULTS.size > host.seenResultsOwner.SEEN_LIMIT)
      host.seenResultsOwner.SEEN_RESULTS.delete(
        host.seenResultsOwner.SEEN_RESULTS.values().next().value!,
      );
    if (changed)
      try {
        window.localStorage.setItem(
          host.seenResultsOwner.SEEN_KEY,
          JSON.stringify([...host.seenResultsOwner.SEEN_RESULTS]),
        );
      } catch {}
  }

  return { markSeenResults };
}

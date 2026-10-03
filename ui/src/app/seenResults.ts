interface SeenResultsHost {}
/** Owns seenResults behavior through explicit application ports. */
export function createSeenResults(host: SeenResultsHost) {
  const SEEN_KEY = 'semon.seen',
    SEEN_LIMIT = 2000;
  const SEEN_RESULTS = (() => {
    try {
      const ids: unknown = JSON.parse(window.localStorage.getItem(SEEN_KEY) ?? '[]');
      if (!Array.isArray(ids)) return new Set<string>();
      const clean = ids
          .filter((id: unknown): id is string => typeof id === 'string')
          .slice(-SEEN_LIMIT),
        seen = new Set(clean);
      if (seen.size !== ids.length)
        try {
          window.localStorage.setItem(SEEN_KEY, JSON.stringify([...seen]));
        } catch {}
      return seen;
    } catch {
      return new Set<string>();
    }
  })();

  return { SEEN_RESULTS, SEEN_LIMIT, SEEN_KEY };
}

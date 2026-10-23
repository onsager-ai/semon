/** Public addresses bind source and Catalog key. Harness/native IDs are provenance. */
export interface SessionRoute {
  source: string | null;
  key: string;
  turn: string | null;
  legacyHarness: string | null;
  anchor: string | null;
  controls: URLSearchParams;
}
export interface SessionCandidate {
  source_key: string;
  catalog_key: string;
  harness: string;
}
const identifier = (value: string) =>
  !!value &&
  new TextEncoder().encode(value).length <= 4096 &&
  value !== '.' &&
  value !== '..' &&
  !/[\u0000-\u001f\u007f-\u009f]/u.test(value);
export const routeSegment = (value: string) =>
  encodeURIComponent(value).replace(
    /[!'()*]/g,
    (c) => '%' + c.charCodeAt(0).toString(16).toUpperCase(),
  );
export const sessionPath = (source: string, key: string) =>
  '/session/' + routeSegment(source) + '/' + routeSegment(key);
export const tracePath = (source: string, key: string, turn: string) =>
  sessionPath(source, key) + '/trace/' + routeSegment(turn);
export function parseSessionRoute(path: string, query = ''): SessionRoute {
  const fail = () => {
    throw new Error('Malformed session address');
  };
  if (!path.startsWith('/') || path.length > 32768 || query.replace(/^\?/, '').length > 8192)
    return fail();
  const parts = path.slice(1).split('/').map(decodeURIComponent);
  if (parts.some((part) => !identifier(part))) return fail();
  let source: string | null = null,
    key: string,
    turn: string | null = null,
    legacyHarness: string | null = null;
  if (
    parts[0] === 'session' &&
    (parts.length === 3 || (parts.length === 5 && parts[3] === 'trace'))
  ) {
    source = parts[1];
    key = parts[2];
    turn = parts[4] ?? null;
  } else if (
    (parts[0] === 's' && parts.length === 3) ||
    (parts[0] === 'trace' && parts.length === 4)
  ) {
    if (!['native', 'codex', 'claude', 'copilot'].includes(parts[1])) return fail();
    legacyHarness = parts[1];
    key = parts[2];
    turn = parts[3] ?? null;
  } else return fail();
  const controls = new URLSearchParams();
  let anchor: string | null = null;
  for (const pair of query.replace(/^\?/, '').split('&').filter(Boolean)) {
    const equal = pair.indexOf('=');
    const name = decodeURIComponent((equal < 0 ? pair : pair.slice(0, equal)).replace(/\+/g, ' '));
    const value = decodeURIComponent((equal < 0 ? '' : pair.slice(equal + 1)).replace(/\+/g, ' '));
    if (/[\u0000-\u001f\u007f-\u009f]/u.test(value)) return fail();
    if (name === 'machine' && legacyHarness) {
      if (source !== null || !identifier(value)) return fail();
      source = value;
      continue;
    }
    if (name === 'turn' && legacyHarness && !turn) {
      if (anchor !== null || !identifier(value)) return fail();
      anchor = value;
      continue;
    }
    const valid =
      name === 'harness'
        ? ['', 'codex', 'claude', 'copilot'].includes(value)
        : ['repo', 'model', 'q'].includes(name)
          ? new TextEncoder().encode(value).length <= 4096
          : ['no_repo', 'all_machines', 'native'].includes(name)
            ? value === '1'
            : ['after', 'field_chunk'].includes(name)
              ? /^\d+$/.test(value) && Number.isSafeInteger(Number(value))
              : name === 'generation'
                ? !!value && new TextEncoder().encode(value).length <= 4096
                : name === 'part'
                  ? ['output', 'script', 'attachment'].includes(value)
                  : false;
    if (!valid || controls.has(name)) return fail();
    controls.set(name, value);
  }
  if (
    (controls.has('repo') && controls.has('no_repo')) ||
    (controls.has('field_chunk') && controls.has('part'))
  )
    return fail();
  if (
    (Number(controls.get('after') ?? 0) > 0 && !controls.has('generation')) ||
    ((controls.has('field_chunk') || controls.has('part')) &&
      (!controls.has('after') || !controls.has('generation')))
  )
    return fail();
  controls.sort();
  return { source, key, turn, legacyHarness, anchor, controls };
}
export function canonicalSessionRoute(route: SessionRoute, source: string, key: string) {
  const query = [...route.controls]
    .map(([name, value]) => routeSegment(name) + '=' + routeSegment(value))
    .join('&');
  return (
    (route.turn ? tracePath(source, key, route.turn) : sessionPath(source, key)) +
    (query ? '?' + query : '') +
    (route.anchor ? '#' + routeSegment(route.anchor) : '')
  );
}
export function resolveSessionRoute(
  route: SessionRoute,
  candidates: readonly SessionCandidate[],
):
  | { outcome: 'valid'; candidate: SessionCandidate }
  | { outcome: 'not_found' | 'ambiguous' | 'mismatch' } {
  const matches = candidates.filter(
    (candidate) =>
      candidate.catalog_key === route.key &&
      (!route.source || route.source === candidate.source_key),
  );
  if (!matches.length) return { outcome: 'not_found' };
  if (matches.length > 1) return { outcome: 'ambiguous' };
  const candidate = matches[0];
  if (
    route.legacyHarness &&
    route.legacyHarness !== 'native' &&
    route.legacyHarness !== candidate.harness
  )
    return { outcome: 'mismatch' };
  return { outcome: 'valid', candidate };
}

/** Only authorized complete session-locate index results may use this compatibility path. */
export function resolveSessionBookmark(
  route: SessionRoute,
  candidates: readonly SessionCandidate[],
) {
  if (route.legacyHarness && route.key.startsWith('unsent:')) {
    const mapped = candidates.map((candidate) => ({ ...candidate, catalog_key: '' }));
    const result = resolveSessionRoute({ ...route, key: '' }, mapped);
    if (result.outcome === 'valid')
      return { outcome: 'valid' as const, candidate: candidates[mapped.indexOf(result.candidate)] };
    return result;
  }
  return resolveSessionRoute(route, candidates);
}

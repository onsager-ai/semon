/** Public addresses bind source and Catalog key. Harness/native IDs are provenance. */
export interface SessionRoute {
  source: string;
  key: string;
  turn: string | null;
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
/** Read the canonical transcript anchor fragment strictly. */
export function sessionHashTurn(hash: string) {
  if (!hash.startsWith('#')) return undefined;
  try {
    const turn = decodeURIComponent(hash.slice(1));
    return identifier(turn) ? turn : undefined;
  } catch {
    return undefined;
  }
}
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
  if (
    parts[0] !== 'session' ||
    !(parts.length === 3 || (parts.length === 5 && parts[3] === 'trace'))
  )
    return fail();
  const source = parts[1],
    key = parts[2],
    turn = parts[4] ?? null;
  const controls = new URLSearchParams();
  for (const pair of query.replace(/^\?/, '').split('&').filter(Boolean)) {
    const equal = pair.indexOf('=');
    const name = decodeURIComponent((equal < 0 ? pair : pair.slice(0, equal)).replace(/\+/g, ' '));
    const value = decodeURIComponent((equal < 0 ? '' : pair.slice(equal + 1)).replace(/\+/g, ' '));
    if (/[\u0000-\u001f\u007f-\u009f]/u.test(value)) return fail();
    const valid =
      name === 'harness'
        ? ['', 'codex', 'claude', 'copilot'].includes(value)
        : ['repo', 'model', 'q'].includes(name)
          ? new TextEncoder().encode(value).length <= 4096
          : ['no_repo', 'all_machines'].includes(name)
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
  return { source, key, turn, controls };
}
export function canonicalSessionRoute(route: SessionRoute, source: string, key: string) {
  const query = [...route.controls]
    .map(([name, value]) => routeSegment(name) + '=' + routeSegment(value))
    .join('&');
  return (
    (route.turn ? tracePath(source, key, route.turn) : sessionPath(source, key)) +
    (query ? '?' + query : '')
  );
}
export function resolveSessionRoute(
  route: SessionRoute,
  candidates: readonly SessionCandidate[],
): { outcome: 'valid'; candidate: SessionCandidate } | { outcome: 'not_found' | 'ambiguous' } {
  const matches = candidates.filter(
    (candidate) => candidate.catalog_key === route.key && route.source === candidate.source_key,
  );
  if (!matches.length) return { outcome: 'not_found' };
  if (matches.length > 1) return { outcome: 'ambiguous' };
  return { outcome: 'valid', candidate: matches[0] };
}

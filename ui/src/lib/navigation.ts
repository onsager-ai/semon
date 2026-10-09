import definitions from './navigation.json';
import type { ShellDestination } from './shell';
import { safePath } from './account';

/** Generic host policy, shared by native pages and every session reader. */
export interface ShellNavigation {
  paths?: Readonly<Record<string, string>>;
  leading?: readonly ShellDestination[];
  trailing?: readonly ShellDestination[];
}

export function readShellPreference(key: 'wide' | 'rail'): boolean {
  try {
    return localStorage.getItem('semon.' + key) === '1';
  } catch {
    return false;
  }
}
export function writeShellPreference(key: 'wide' | 'rail', value: boolean) {
  try {
    localStorage.setItem('semon.' + key, value ? '1' : '0');
  } catch {}
}

export function withReturnPath(href: string, path: string): string {
  if (!safePath(href) || !safePath(path)) return href;
  const url = new URL(href, location.href);
  url.searchParams.set('return_to', path.split('#')[0]);
  return url.pathname + url.search;
}

export function parseShellNavigation(value: unknown): ShellNavigation | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const policy: ShellNavigation = {};
  if ('paths' in value) {
    if (!value.paths || typeof value.paths !== 'object' || Array.isArray(value.paths))
      return undefined;
    const paths: Record<string, string> = {};
    for (const [key, path] of Object.entries(value.paths)) {
      if (typeof path !== 'string' || !safePath(path)) return undefined;
      paths[key] = path;
    }
    policy.paths = paths;
  }
  for (const position of ['leading', 'trailing'] as const) {
    const entries =
      position === 'leading'
        ? 'leading' in value
          ? value.leading
          : []
        : 'trailing' in value
          ? value.trailing
          : [];
    if (!Array.isArray(entries)) return undefined;
    const destinations: ShellDestination[] = [];
    for (const entry of entries) {
      if (
        !entry ||
        typeof entry !== 'object' ||
        typeof entry.key !== 'string' ||
        typeof entry.label !== 'string' ||
        typeof entry.icon !== 'string' ||
        typeof entry.href !== 'string' ||
        !safePath(entry.href) ||
        definitions.some((definition) => definition.key === entry.key) ||
        destinations.some((destination) => destination.key === entry.key)
      )
        return undefined;
      destinations.push({
        key: entry.key,
        label: entry.label,
        icon: entry.icon,
        href: entry.href,
        current: false,
      });
    }
    policy[position] = destinations;
  }
  if (policy.leading?.some((entry) => policy.trailing?.some((other) => other.key === entry.key)))
    return undefined;
  return policy;
}

export function projectShellNavigation(
  policy: ShellNavigation | undefined,
  route: string,
  paths: Readonly<Record<string, string>> = {},
  counts: Readonly<Record<string, number>> = {},
): ShellDestination[] {
  const current = ['session', 'trace', 'sources'].includes(route)
    ? 'sessions'
    : route === 'machine'
      ? 'machines'
      : route;
  return [
    ...(policy?.leading ?? []),
    ...definitions.map(({ path, ...definition }) => ({
      ...definition,
      href: paths[definition.key] ?? policy?.paths?.[definition.key] ?? path,
      current: false,
    })),
    ...(policy?.trailing ?? []),
  ].map((destination) => ({
    ...destination,
    href: paths[destination.key] ?? destination.href,
    current: destination.key === current,
    ...(counts[destination.key] === undefined ? {} : { count: counts[destination.key], hot: true }),
  }));
}

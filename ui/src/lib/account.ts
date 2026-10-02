/** Copied at the untrusted /api/model or window.semonEmbed boundary. */
export interface Account {
  name: string; login: string; initials: string; avatar_href: string | null;
  workspaces: { name: string; role: string; current: boolean; switch_href: string }[];
  links: { label: string; href: string; method: 'get' | 'post'; danger: boolean }[];
}
export const safePath = (href: unknown): href is string => typeof href === 'string' && href.startsWith('/') && !href.startsWith('//') && !href.includes('\\') && !/[\u0000-\u001f\u007f-\u009f]/.test(href) && href.length <= 512;
const textField = (value: unknown, min: number, max: number): value is string => typeof value === 'string' && [...value].length >= min && [...value].length <= max && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
const record = (x: unknown): x is Record<string, unknown> => !!x && typeof x === 'object';
/** Read each property once, including hostile getters, then validate only the copy. */
export function parseAccount(source: unknown): Account | null {
  let value;
  try {
    if (!record(source)) return null;
    const list = (xs: unknown, max: number, pick: (x: Record<string, unknown>) => unknown) => {
      if (!Array.isArray(xs)) return null;
      const n = xs.length; if (!(n <= max)) return null;
      const out = []; for (let i = 0; i < n; i++) { const x: unknown = xs[i]; out.push(record(x) ? pick(x) : null); }
      return out;
    };
    value = {
      name: source.name, login: source.login, initials: source.initials, avatar_href: source.avatar_href ?? null,
      workspaces: list(source.workspaces, 50, w => ({ name: w.name, role: w.role, current: w.current, switch_href: w.switch_href })),
      links: list(source.links, 12, a => ({ label: a.label, href: a.href, method: a.method, danger: a.danger })),
    };
  } catch { return null; }
  if (!textField(value.name, 1, 80) || !value.name.trim() || !textField(value.login, 0, 80) || !textField(value.initials, 1, 3) || !value.initials.trim()) return null;
  if (value.avatar_href !== null && !safePath(value.avatar_href)) return null;
  if (!value.workspaces || !value.links) return null;
  // The snapshot above has no getters. Do not pass arbitrary source props to JSX.
  const workspaces = value.workspaces as Account['workspaces'];
  const links = value.links as Account['links'];
  if (workspaces.some(w => !w || !textField(w.name, 1, 80) || !w.name.trim() || !textField(w.role, 0, 80) || typeof w.current !== 'boolean' || !safePath(w.switch_href))) return null;
  if (links.some(a => !a || !textField(a.label, 1, 80) || !a.label.trim() || !safePath(a.href) || (a.method !== 'get' && a.method !== 'post') || typeof a.danger !== 'boolean')) return null;
  return { name: value.name, login: value.login, initials: value.initials, avatar_href: value.avatar_href, workspaces, links };
}

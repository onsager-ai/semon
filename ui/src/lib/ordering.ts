/** Stable ordering is document state, independent of keyed screen rendering. */
export interface OrderScope<Tie extends object> {
  sig: string;
  tie: Tie | null;
  keep: boolean;
  calm: boolean;
  prev: Map<string, string[]>;
  prevKids: Set<string>;
  kids: Set<string>;
  prevExp: string | null;
  exp: string | null;
  reseed: boolean;
  lists: Map<string, string[]>;
  n: number;
  rail?: boolean;
}
export function createOrdering<Tie extends object>() {
  const scopes = new Map<string, OrderScope<Tie>>();
  function begin(
    name: string,
    sig: string,
    tie: Tie | null,
    state: { inView: boolean; touched: boolean },
  ): OrderScope<Tie> {
    const previous = scopes.get(name),
      keep = !!previous && previous.sig === sig && previous.tie === tie;
    const scope: OrderScope<Tie> = {
      sig,
      tie,
      keep,
      calm: state.inView && !state.touched,
      prev: keep ? previous.lists : new Map(),
      prevKids: keep ? previous.kids : new Set(),
      kids: new Set(),
      prevExp: keep ? previous.exp : null,
      exp: null,
      reseed: false,
      lists: new Map(),
      n: 0,
    };
    scopes.set(name, scope);
    return scope;
  }
  return { scopes, begin };
}
function moves<Row>(rows: Row[], compare: (a: Row, b: Row) => number) {
  const tails: Row[] = [];
  for (const row of rows) {
    let low = 0,
      high = tails.length;
    while (low < high) {
      const mid = (low + high) >> 1;
      if (compare(tails[mid]!, row) > 0) high = mid;
      else low = mid + 1;
    }
    tails[low] = row;
  }
  return rows.length - tails.length;
}
export function orderRows<Row extends { id: string }, Tie extends object>(
  scope: OrderScope<Tie>,
  key: string,
  items: readonly Row[],
  compare: (a: Row, b: Row) => number,
  {
    must = null,
    limit = Infinity,
    quiet = false,
    seed = false,
  }: { must?: ReadonlySet<string> | null; limit?: number; quiet?: boolean; seed?: boolean } = {},
): Row[] {
  const sorted = [...items].sort(compare),
    by = new Map(items.map((row) => [row.id, row]));
  let ids = sorted.map((row) => row.id);
  if (scope.keep && !quiet && !(seed && !scope.prev.has(key))) {
    const existed = scope.prev.has(key),
      old = (scope.prev.get(key) ?? []).filter((id) => by.has(id)),
      have = new Set(old),
      fresh = sorted.filter((row) => !have.has(row.id)),
      up = fresh.filter((row) => must?.has(row.id) || (scope.calm && existed));
    ids = [...up.map((row) => row.id), ...old];
    const shown = ids.slice(0, limit),
      inShown = new Set(shown),
      top = new Set(sorted.slice(0, limit).map((row) => row.id)),
      held = fresh.filter((row) => top.has(row.id) && !up.includes(row)).length;
    scope.n +=
      held +
      sorted.slice(0, limit).filter((row) => have.has(row.id) && !inShown.has(row.id)).length +
      moves(
        shown.map((id) => by.get(id)!),
        compare,
      );
  }
  scope.lists.set(key, ids);
  return ids.map((id) => by.get(id)!);
}

/** Matches Recent's active-first, required-path, then three-row fill policy. */
const TREE_ACTIVE = 8,
  TREE_ROWS = 3;

export function recentOrderSource(
  source: string | undefined,
  machines: readonly { key: string }[],
): string | undefined {
  return source ?? (machines.length === 1 ? machines[0]!.key : undefined);
}

export function selectRecentChildren<Row extends { id: string }>(
  items: readonly Row[],
  rank: (row: Row) => number,
  required: ReadonlySet<string>,
  full = false,
): Row[] {
  if (full) return [...items];
  const keep = new Set<string>();
  // Inputs are status/last-seen sorted; these passes preserve the original selection order.
  for (const row of items) if (rank(row) < 2 && keep.size < TREE_ACTIVE) keep.add(row.id);
  for (const row of items) if (required.has(row.id)) keep.add(row.id);
  for (const row of items) if (keep.size < TREE_ROWS) keep.add(row.id);
  return items.filter((row) => keep.has(row.id));
}

/** The original eight-root cap, with a bounded selected-metadata fallback. */
export function selectRecentRoots<Row extends { id: string }>(
  ordered: readonly Row[],
  required: ReadonlySet<string>,
  published: ReadonlySet<string>,
): Row[] {
  const visible = ordered.slice(0, 8),
    fallback = ordered.find((row) => required.has(row.id) && !published.has(row.id));
  if (fallback && !visible.some((row) => row.id === fallback.id)) {
    if (visible.length === 8) visible[7] = fallback;
    else visible.push(fallback);
  }
  return visible;
}

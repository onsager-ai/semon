// The live list's order rule (#126, simplified): a list keeps its order as a snapshot of ids between updates. A session
// new since the snapshot goes first only while the list is free (its top in view and untouched); otherwise it is held.
// What is held shows as "N updated": the held sessions plus the shown rows that would have to move to be in recency order.
import { byRecency, type Session } from "./model";

export interface Kept {
  order: string[];
  held: number;
}

export function keepOrder(prev: readonly string[] | null, fresh: readonly Session[], free: boolean): Kept {
  const recency = [...fresh].sort(byRecency).map((s) => s.id);
  if (!prev) return { order: recency, held: 0 };
  const alive = new Set(recency);
  const kept = prev.filter((id) => alive.has(id));
  const known = new Set(kept);
  const added = recency.filter((id) => !known.has(id));
  const shown = recency.filter((id) => known.has(id));
  const moved = kept.filter((id, i) => shown[i] !== id).length;
  return free ? { order: [...added, ...kept], held: moved } : { order: kept, held: added.length + moved };
}

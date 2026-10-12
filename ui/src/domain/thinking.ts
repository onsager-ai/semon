import type { CatalogTranscriptEntry } from '../state/catalog-transcript-wire';
/** Display adjacent Codex snapshots without changing accepted records or crossing native turn boundaries. */
export function coalesceRecordedThoughts(
  entries: readonly CatalogTranscriptEntry[],
  harness: string,
  knownTurns: ReadonlySet<string>,
): readonly CatalogTranscriptEntry[] {
  if (harness !== 'codex') return entries;
  const out: CatalogTranscriptEntry[] = [];
  let chain: { index: number; owner: string | null; text: string | null } | null = null;
  let owner: string | null = null,
    sawOwner = false,
    unresolved = false;
  for (const item of entries) {
    const entry = item.entry;
    if (entry.turn !== undefined) {
      owner = knownTurns.has(entry.turn) ? entry.turn : null;
      unresolved = owner === null;
      if (owner) sawOwner = true;
    }
    if (
      entry.k !== 'think' ||
      entry.pending ||
      entry.text === '' ||
      unresolved ||
      (!owner && sawOwner)
    ) {
      out.push(item);
      chain = null;
      continue;
    }
    if (
      chain &&
      chain.owner === owner &&
      ((entry.text === undefined && chain.text === null) ||
        (typeof entry.text === 'string' &&
          typeof chain.text === 'string' &&
          (entry.text === chain.text ||
            entry.text.startsWith(chain.text + '\n') ||
            entry.text.startsWith(chain.text + '\r\n'))))
    ) {
      const prior = out[chain.index];
      if (prior.entry.k !== 'think') throw Error('Invalid recorded thought chain');
      out[chain.index] = {
        ...prior,
        entry: { ...prior.entry, text: entry.text, secs: entry.secs },
      };
      chain.text = entry.text ?? null;
    } else {
      out.push(item);
      chain = { index: out.length - 1, owner, text: entry.text ?? null };
    }
  }
  return out;
}
/** Masked reasoning has one quiet placeholder per qualified native turn. */
export function visibleMaskedThoughts(
  entries: readonly CatalogTranscriptEntry[],
  knownTurns: ReadonlySet<string>,
): readonly CatalogTranscriptEntry[] {
  const seen = new Set<string>();
  let owner: string | undefined;
  return entries.filter(({ entry }) => {
    if (entry.turn !== undefined) owner = knownTurns.has(entry.turn) ? entry.turn : undefined;
    if (entry.k === 'end') owner = undefined;
    if (entry.k !== 'think' || entry.text || entry.pending || owner === undefined) return true;
    if (seen.has(owner)) return false;
    seen.add(owner);
    return true;
  });
}
export function recordedThoughtLabel(seconds: string | number | undefined): string {
  const match = /^(\d+(?:\.\d+)?)s?$/.exec(String(seconds ?? ''));
  const n = match ? Math.round(Number(match[1])) : 0;
  return n < 1
    ? 'Thinking'
    : 'Thinking · ' + (n >= 60 ? Math.floor(n / 60) + 'm ' + (n % 60) + 's' : n + 's');
}

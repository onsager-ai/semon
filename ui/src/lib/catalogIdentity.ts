/** Session IDs are unique only inside a source. */
export function catalogSessionKey(item: { key: string; machine?: { key: string } }): string {
  return item.machine ? JSON.stringify([item.machine.key, item.key]) : item.key;
}

/** Match SQLite BINARY/Rust UTF-8 key ordering, independent of browser locale. */
export function compareCatalogKeys(left: string, right: string): number {
  const a = left[Symbol.iterator](),
    b = right[Symbol.iterator]();
  for (;;) {
    const x = a.next(),
      y = b.next();
    if (x.done || y.done) return Number(y.done) - Number(x.done);
    const difference = x.value.codePointAt(0)! - y.value.codePointAt(0)!;
    if (difference) return difference;
  }
}

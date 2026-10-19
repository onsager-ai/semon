import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/catalogIdentity.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { compareCatalogKeys, catalogSessionKey } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('Catalog ties match UTF-8 binary order and retain source-local duplicates', () => {
  const keys = ['session-2', 'session-1', 'Z', 'a', '_', 'é', '中', '😀', '\uE000'];
  assert.deepEqual(
    [...keys].sort(compareCatalogKeys),
    [...keys].sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b))),
  );
  assert.notEqual(
    catalogSessionKey({ key: 'same', machine: { key: 'a' } }),
    catalogSessionKey({ key: 'same', machine: { key: 'b' } }),
  );
});

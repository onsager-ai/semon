import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-sources.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseCatalogSources } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
test('source inventory preserves exact keys and permits identical display names without authority inference', () => {
  const parsed = parseCatalogSources({
    api: 1,
    items: [
      { source_key: 'one', label: 'Same hostname' },
      { source_key: 'two', label: 'Same hostname' },
    ],
    next_cursor: 'opaque',
  });
  assert.deepEqual(
    parsed.items.map((item) => item.source_key),
    ['one', 'two'],
  );
  assert.equal(parsed.next_cursor, 'opaque');
  for (const value of [
    { api: 2, items: [], next_cursor: null },
    { api: 1, items: [{ source_key: '', label: 'Host' }], next_cursor: null },
    {
      api: 1,
      items: [
        { source_key: 'one', label: 'Host' },
        { source_key: 'one', label: 'Other' },
      ],
      next_cursor: null,
    },
    {
      api: 1,
      items: Array.from({ length: 101 }, (_, i) => ({ source_key: String(i), label: 'Host' })),
      next_cursor: null,
    },
  ])
    assert.throws(() => parseCatalogSources(value));
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/sessionRoute.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseSessionRoute, resolveSessionRoute, canonicalSessionRoute } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const vectors = JSON.parse(
  await readFile(
    new URL('../../tests/fixtures/session-route-vectors.json', import.meta.url),
    'utf8',
  ),
);
for (const vector of vectors)
  test('session route: ' + vector.name, () => {
    if (vector.outcome === 'malformed') {
      assert.throws(() => parseSessionRoute(vector.path, vector.query));
      return;
    }
    const route = parseSessionRoute(vector.path, vector.query);
    const result = resolveSessionRoute(route, vector.candidates);
    assert.equal(result.outcome, vector.outcome);
    if (result.outcome === 'valid')
      assert.equal(
        canonicalSessionRoute(route, result.candidate.source_key, result.candidate.catalog_key),
        vector.canonical,
      );
  });

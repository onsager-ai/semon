import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-recovery.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { CatalogRecovery, CATALOG_RECOVERY_WALL_BUDGET_MS } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);

test('unchanged selected projections retain the 1/2/4 second retry schedule and manual reset', () => {
  const recovery = new CatalogRecovery();
  const start = Date.now();
  recovery.observeProgress({ source: 'source-a', catalog: 'catalog-a' }, start);
  assert.equal(recovery.stopped, false);
  assert.deepEqual(
    [recovery.fail(start), recovery.fail(start), recovery.fail(start)],
    [1000, 2000, 4000],
  );
  assert.equal(recovery.fail(start), null);
  assert.equal(recovery.stopped, true);
  assert.equal(recovery.fail(start), null);
  assert.equal(
    recovery.observeProgress({ source: 'source-a', catalog: 'catalog-a' }, start + 8000),
    false,
    'repeated metadata cannot reopen recovery',
  );
  recovery.reset();
  assert.equal(recovery.stopped, false);
  assert.equal(recovery.fail(start + 8000), 1000);
  recovery.reset();
  assert.equal(recovery.fail(start + 8000), 1000, 'successful reads start a fresh budget');
});

test('published source or catalog progress reopens exhaustion within one fixed wall-clock budget', () => {
  const recovery = new CatalogRecovery();
  const start = Date.now();
  recovery.observeProgress({ source: 'source-a', catalog: 'catalog-a' }, start);
  for (let attempt = 0; attempt < 4; ++attempt) recovery.fail(start);
  assert.equal(recovery.stopped, true);
  assert.equal(
    recovery.observeProgress({ catalog: 'catalog-b' }, start + 5000),
    true,
    'a new published Catalog generation permits one bounded recovery window',
  );
  assert.equal(recovery.stopped, false);
  assert.equal(recovery.fail(start + 5000), 1000);
  assert.equal(
    recovery.observeProgress({ source: 'source-b' }, start + 6000),
    false,
    'progress during backoff resets the next failure count without bypassing its timer',
  );
  assert.equal(recovery.fail(start + 6000), 1000);
  assert.equal(recovery.stopped, false);
});

test('metadata progress after the total wall-clock cap cannot restart recovery', () => {
  const recovery = new CatalogRecovery();
  const start = Date.now();
  recovery.observeProgress({ source: 'source-a' }, start);
  for (let attempt = 0; attempt < 4; ++attempt) recovery.fail(start);
  assert.equal(
    recovery.observeProgress(
      { source: 'source-b' },
      start + CATALOG_RECOVERY_WALL_BUDGET_MS + 1,
    ),
    false,
  );
  assert.equal(recovery.stopped, true);
});

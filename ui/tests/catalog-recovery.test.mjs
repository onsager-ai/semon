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
const { CatalogRecovery } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
test('selected recovery stops after three automatic retries and manual retry resets the budget', () => {
  const recovery = new CatalogRecovery();
  assert.equal(recovery.stopped, false);
  assert.deepEqual([recovery.fail(), recovery.fail(), recovery.fail()], [1000, 2000, 4000]);
  assert.equal(recovery.fail(), null);
  assert.equal(recovery.stopped, true);
  assert.equal(recovery.fail(), null);
  recovery.reset();
  assert.equal(recovery.stopped, false);
  assert.equal(recovery.fail(), 1000);
  recovery.reset();
  assert.equal(recovery.fail(), 1000, 'successful reads start a fresh recovery budget');
});

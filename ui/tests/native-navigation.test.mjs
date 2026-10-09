import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/routes.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { routeUrl } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('Rust native navigation paths agree with the Viewer router defaults', async () => {
  const rows = JSON.parse(
    await readFile(new URL('../src/lib/navigation.json', import.meta.url), 'utf8'),
  );
  assert.equal(rows.length, 4);
  const model = { session: () => undefined, machine: () => false, turn: () => undefined };
  for (const { key, path } of rows)
    assert.equal(routeUrl({ v: key }, model), path, key + ' route contract');
  assert.equal(routeUrl({ v: 'machines' }, { ...model, machinesPath: '/devices' }), '/devices');
});

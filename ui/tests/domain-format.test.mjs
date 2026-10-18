import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/domain/format.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { machineShorts, dur, compactCount } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('machine labels resolve collisions and time/count formatting retains the viewer contract', () => {
  const labels = machineShorts([
    ['a', 'build-host-very-long-east.example'],
    ['b', 'build-host-very-long-west.example'],
  ]);
  assert.notEqual(labels.get('a'), labels.get('b'));
  assert.equal(dur(0, 90061000, 0), '1d 1h');
  assert.equal(compactCount(12422228), '12.4M');
});

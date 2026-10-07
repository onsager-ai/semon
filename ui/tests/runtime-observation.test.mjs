import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/runtimeObservation.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseRuntimeObservation } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
test('readonly runtime observations preserve phase and freshness without native authority', () => {
  const raw = {
    state: 'ended',
    phase: 'released',
    freshness: 'current',
    presence: 'absent',
    observedAt: '2026-10-07T00:00:00Z',
    observationError: null,
    updating: false,
    reconnectable: true,
    capabilities: { input: true },
  };
  const result = parseRuntimeObservation(raw);
  assert.equal(result.state, 'ended');
  assert.equal(result.phase, 'released');
  assert.equal(result.presence, 'absent');
  assert.equal('reconnectable' in result, false);
  assert.equal('capabilities' in result, false);
  assert.ok(Object.isFrozen(result));
  for (const patch of [
    { state: 'idle' },
    { freshness: 'cached' },
    { phase: 'a'.repeat(65) },
    { presence: 'up' },
    { observedAt: 'yesterday' },
    { observationError: 'a'.repeat(513) },
    { updating: 1 },
  ])
    assert.throws(() => parseRuntimeObservation({ ...raw, ...patch }));
});

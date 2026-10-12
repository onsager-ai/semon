import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/app/tracePresentation.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { causalSessionPart } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('incoming trace sender uses selected family identity without broadening the trace', () => {
  const data = {
    sessions: { selected: { name: 'Selected' } },
    metadata: { details: { family: { ancestors: [{ key: 'sender', name: 'Updated sender' }], runs: [] } } },
  };
  const before = JSON.stringify(data);
  assert.deepEqual(causalSessionPart(data, 'sender', 'sender-turn'), {
    className: 'who', text: 'Updated sender', action: { kind: 'session', id: 'sender', turn: 'sender-turn' },
  });
  assert.deepEqual(Object.keys(data.sessions), ['selected']);
  assert.equal(JSON.stringify(data), before);
  assert.equal(causalSessionPart(data, 'you').text, 'You');
  assert.equal(causalSessionPart(data, 'you').action, undefined);
  assert.equal(causalSessionPart(data, 'unrelated').text, 'Unrecorded session');
  assert.equal(causalSessionPart(data, 'unrelated').action, undefined);
});
test('trace-owned identity wins and ambiguous family identities remain unavailable', () => {
  const data = {
    sessions: { selected: { name: 'Current selection' } },
    metadata: { details: { family: { ancestors: [{ key: 'selected', name: 'Old selection' }, { key: 'sender', name: 'One' }], runs: [{ key: 'sender', name: 'Two' }] } } },
  };
  assert.equal(causalSessionPart(data, 'selected').text, 'Current selection');
  assert.equal(causalSessionPart(data, 'sender').text, 'Unrecorded session');
  assert.equal(causalSessionPart(data, 'sender').action, undefined);
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';

const { outputFiles } = await build({
  entryPoints: [new URL('../src/domain/trace.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { reachableResultIds } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);

test('viewing a bounded trace marks only root-reachable results through recorded child starts', () => {
  const result = (id) => ({ id, kind: 'toyou', ask: 'result' });
  const root = {
    id: 'root',
    sent: [
      result('root-result'),
      { id: 'spawn', kind: 'spawn' },
      { id: 'move', kind: 'move' },
      { id: 'question', kind: 'toyou', ask: 'question' },
    ],
  };
  const child = { id: 'child', sent: [result('child-result'), { id: 'relay', kind: 'relay' }] };
  const peer = { id: 'peer', sent: [result('peer-result'), { id: 'cycle', kind: 'spawn' }] };
  const unrelated = { id: 'unrelated', sent: [result('unrelated-result')] };
  const starts = new Map([
    ['spawn', child],
    ['relay', peer],
    ['cycle', root],
    ['move', unrelated],
    ['question', unrelated],
    ['unrelated', unrelated],
  ]);
  assert.deepEqual(reachableResultIds(root, starts), [
    'root-result',
    'child-result',
    'peer-result',
  ]);
  starts.delete('relay');
  assert.deepEqual(reachableResultIds(root, starts), ['root-result', 'child-result']);
});

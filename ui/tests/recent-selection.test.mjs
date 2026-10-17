import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';

const { outputFiles } = await build({
  entryPoints: [new URL('../src/domain/recent.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { recentOrderSource, selectRecentChildren } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);

test('one-machine Home and selected-session Recent ordering share scope, multi-machine does not', () => {
  const oneMachine = [{ key: 'laptop' }],
    multiMachine = [{ key: 'laptop' }, { key: 'workstation' }],
    signature = (source, machines) =>
      JSON.stringify([recentOrderSource(source, machines), false, false]);
  assert.equal(signature(undefined, oneMachine), signature('laptop', oneMachine));
  assert.notEqual(signature(undefined, multiMachine), signature('laptop', multiMachine));
});

test('the selected path fills an ordinary Recent preview before the three-row cap', () => {
  const rows = [
    { id: 'newest', rank: 2 },
    { id: 'next', rank: 2 },
    { id: 'third', rank: 2 },
    { id: 'selected', rank: 2 },
  ];
  assert.deepEqual(
    selectRecentChildren(rows, (row) => row.rank, new Set(['selected'])).map((row) => row.id),
    ['newest', 'next', 'selected'],
  );
});

test('Recent keeps up to eight busy children plus the selected path exception', () => {
  const rows = [
    ...Array.from({ length: 9 }, (_, i) => ({ id: 'busy-' + i, rank: 1 })),
    { id: 'selected', rank: 2 },
  ];
  assert.deepEqual(
    selectRecentChildren(rows, (row) => row.rank, new Set(['selected'])).map((row) => row.id),
    [...Array.from({ length: 8 }, (_, i) => 'busy-' + i), 'selected'],
  );
});

test('an expanded Recent group returns every loaded child', () => {
  const rows = Array.from({ length: 10 }, (_, i) => ({ id: 'child-' + i, rank: 2 }));
  assert.deepEqual(
    selectRecentChildren(rows, (row) => row.rank, new Set(), true).map((row) => row.id),
    rows.map((row) => row.id),
  );
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/domain/thinking.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { coalesceRecordedThoughts, recordedThoughtLabel } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const rows = (values) =>
  values.map((entry, slot) => ({
    entry_id: 'native-' + slot,
    slot,
    entry,
    provenance: { offset: slot },
  }));
test('recorded Codex snapshots coalesce only adjacent line extensions under the same known native owner', () => {
  const input = rows([
    { k: 'think', text: 'Prelude' },
    { k: 'think', text: 'Prelude\nA' },
    { k: 'think', turn: 'a', text: 'Plan', secs: '1s' },
    { k: 'think', turn: 'a', text: 'Plan\nA', secs: '2s' },
    { k: 'think', turn: 'b', text: 'Plan\nA\nB' },
    { k: 'tool', turn: 'b', name: 'native' },
    { k: 'think', turn: 'b', text: 'Plan\nA\nB\nC' },
    { k: 'think', turn: 'unknown', text: 'Later' },
    { k: 'think', text: 'Later\nB' },
    { k: 'think', turn: 'a', text: 'Plan' },
    { k: 'think', turn: 'a', text: 'Planning' },
    { k: 'think', turn: 'a', text: '' },
    { k: 'think', turn: 'a', text: 'Plan', pending: true },
    { k: 'think', turn: 'a', text: 'Plan\nTail' },
  ]);
  const original = JSON.stringify(input),
    display = coalesceRecordedThoughts(input, 'codex', new Set(['a', 'b']));
  assert.equal(display.length, input.length - 2);
  assert.equal(display[0].entry.text, 'Prelude\nA');
  assert.equal(display[1].entry_id, 'native-2');
  assert.equal(display[1].entry.text, 'Plan\nA');
  assert.equal(display[1].entry.secs, '2s');
  assert.equal(display[2].entry.turn, 'b');
  assert.equal(JSON.stringify(input), original);
  assert.equal(coalesceRecordedThoughts(input, 'claude', new Set(['a', 'b'])), input);
});
test('native thinking durations omit unmeasured time and preserve readable rounded durations', () => {
  assert.equal(recordedThoughtLabel(undefined), 'Thinking');
  assert.equal(recordedThoughtLabel('0.1s'), 'Thinking');
  assert.equal(recordedThoughtLabel('12s'), 'Thinking · 12s');
  assert.equal(recordedThoughtLabel('62s'), 'Thinking · 1m 2s');
});

test('adjacent hidden Codex thinking collapses within its known turn without changing evidence', () => {
  const input = rows([
    { k: 'think', turn: 'a' },
    { k: 'think', turn: 'a' },
    { k: 'think', turn: 'b' },
    { k: 'tool', turn: 'b', name: 'native' },
    { k: 'think', turn: 'b' },
    { k: 'think', turn: 'unknown' },
    { k: 'think' },
    { k: 'think', turn: 'a', pending: true },
    { k: 'think', turn: 'a' },
  ]);
  const original = JSON.stringify(input);
  const display = coalesceRecordedThoughts(input, 'codex', new Set(['a', 'b']));
  assert.deepEqual(
    display.map((item) => item.entry_id),
    [
      'native-0',
      'native-2',
      'native-3',
      'native-4',
      'native-5',
      'native-6',
      'native-7',
      'native-8',
    ],
  );
  assert.equal(JSON.stringify(input), original);
  assert.equal(coalesceRecordedThoughts(input, 'claude', new Set(['a', 'b'])), input);
});

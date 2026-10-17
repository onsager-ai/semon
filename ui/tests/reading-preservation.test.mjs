import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  stdin: {
    contents:
      "export {recordedRelationshipViews} from './src/app/catalogTranscriptView'; export {selectedSessionFooter} from './src/app/sessionPresentation'; export {parseRecordedRelationship} from './src/domain/relationships';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { recordedRelationshipViews, selectedSessionFooter, parseRecordedRelationship } =
  await import(
    'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
  );
globalThis.matchMedia = () => ({ matches: false });
const edge = {
  id: 'spawn',
  from: 'parent',
  to: 'child',
  kind: 'spawn',
  status: 'work',
  at: 10,
  declined: false,
};
test('parent return preserves only an explicitly recorded holding turn', () => {
  const relationship = parseRecordedRelationship({ ...edge, source_turn: 'holding-turn' });
  assert.equal(relationship.sourceTurn, 'holding-turn');
  assert.equal(parseRecordedRelationship(edge).sourceTurn, undefined);
  assert.throws(() => parseRecordedRelationship({ ...edge, source_turn: 42 }));
  const session = {
    start: 0,
    last: 20,
    details: { state: 'done', calls: 0, tool_calls: {}, cost: { usd: null } },
  };
  const origin = { id: 'parent', name: 'Parent', status: 'done', turn: relationship.sourceTurn };
  assert.deepEqual(selectedSessionFooter(session, origin).parent, {
    id: 'parent',
    name: 'Parent',
    turn: 'holding-turn',
  });
});
test('compact child activity belongs only to the current working observation', () => {
  const item = { entry_id: 'entry' },
    relationship = { edge };
  const participant = {
    name: 'Child',
    harness: 'codex',
    model: 'gpt-6-luna',
    kind: 'codex-run',
    state: 'work',
    live_observation: true,
  };
  const native = { activity: ['apply_patch', 'src/native.rs'] };
  const view = (child, read = native) =>
    recordedRelationshipViews(item, relationship, 'parent', { child }, read)[0];
  assert.deepEqual(view(participant).activity, ['Patching', 'src/native.rs']);
  assert.equal(view({ ...participant, live_observation: false }).activity, undefined);
  assert.equal(view({ ...participant, state: 'done' }).activity, undefined);
  assert.equal(view(participant, { ...native, result: 'Complete' }).activity, undefined);
  assert.equal(view(participant, {}).activity, undefined);
});

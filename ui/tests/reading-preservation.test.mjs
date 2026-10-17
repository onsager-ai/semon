import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  stdin: {
    contents:
      "export {catalogTranscriptBlocks, recordedRelationshipViews} from './src/app/catalogTranscriptView'; export {selectedSessionFooter} from './src/app/sessionPresentation'; export {parseRecordedRelationship} from './src/domain/relationships';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const {
  catalogTranscriptBlocks,
  recordedRelationshipViews,
  selectedSessionFooter,
  parseRecordedRelationship,
} = await import(
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
test('recorded footers retain the original role and returned-child visibility rules', () => {
  const session = {
    name: 'Principal',
    start: 0,
    last: 20,
    details: { state: 'idle', calls: 0, tool_calls: {}, cost: { usd: null } },
  };
  assert.ok(selectedSessionFooter(session), 'a role is never inferred from the session name');
  session.details.role = true;
  assert.equal(selectedSessionFooter(session), undefined);
  session.details.role = false;
  const origin = { id: 'parent', name: 'Parent', status: 'work' };
  assert.equal(selectedSessionFooter(session, origin), undefined);
  session.details.state = 'wait';
  assert.equal(selectedSessionFooter(session, origin), undefined);
  session.details.state = 'work';
  assert.ok(selectedSessionFooter(session, origin));
  session.details.role = true;
  assert.ok(selectedSessionFooter(session, origin), 'a known origin retains child footer rules');
  session.details.role = false;
  session.details.state = 'idle';
  origin.status = 'done';
  assert.equal(selectedSessionFooter(session, origin).text, 'Returned to Parent · Done');
});
test('incoming sender links use their recorded holding turn and describe unavailable anchors honestly', () => {
  const parent = { name: 'Parent', harness: 'claude' },
    item = { entry_id: 'brief', slot: 0, entry: { k: 'h', id: 'spawn', turn: 'child-turn' } };
  const header = (sourceTurn, sessions = { parent }, from = 'parent', sessionKey = 'child') =>
    catalogTranscriptBlocks(
      [item],
      undefined,
      () => [{ kind: 'message', key: 'brief', flavor: 'incoming', text: 'Saved brief' }],
      false,
      {
        turns: [
          {
            id: 'child-turn',
            start: 'spawn',
            u: false,
            sent: [],
            end: { st: 'done', why: 'complete' },
          },
        ],
        handoffs: [{ edge: { ...edge, from }, sourceTurn }],
        sessions,
      },
      undefined,
      'claude',
      sessionKey,
    )[0].turn.header.parts[1];
  assert.deepEqual(header('holding-turn').action, {
    kind: 'session',
    id: 'parent',
    turn: 'holding-turn',
  });
  assert.equal(header('holding-turn').label, 'Open Parent where it sent this');
  assert.deepEqual(header(undefined).action, { kind: 'session', id: 'parent', turn: undefined });
  assert.equal(header(undefined).label, 'Open Parent');
  assert.equal(header('holding-turn', {}).action, undefined);
  assert.equal(header('holding-turn', { parent }, 'parent', 'parent').action, undefined);
  assert.equal(header('holding-turn', { you: parent }, 'you').action, undefined);
});
test('relationship sender and recipient links retain their separate recorded turn anchors', () => {
  const parent = { name: 'Parent' },
    child = { name: 'Child' };
  const parts = (sourceTurn, targetTurn, kind = 'relay', selected = 'other') =>
    recordedRelationshipViews(
      { entry_id: 'relay' },
      {
        edge: { ...edge, kind, ...(kind === 'toyou' ? { ask: 'result' } : {}) },
        sourceTurn,
        targetTurn,
      },
      selected,
      { parent, child },
      {},
    )[0].parts;
  const known = parts('holding-turn', 'receiving-turn');
  assert.deepEqual(known[0].action, { kind: 'session', id: 'parent', turn: 'holding-turn' });
  assert.equal(known[0].label, 'Open Parent where it sent this');
  assert.deepEqual(known[2].action, { kind: 'session', id: 'child', turn: 'receiving-turn' });
  assert.equal(known[2].label, 'Open Child at the turn this started');
  const unavailable = parts(undefined, undefined);
  assert.equal(unavailable[0].action.turn, undefined);
  assert.equal(unavailable[0].label, 'Open Parent');
  assert.equal(unavailable[2].action.turn, undefined);
  assert.equal(unavailable[2].label, 'Open Child');
  assert.equal(parts('holding-turn', 'receiving-turn', 'relay', 'parent')[0].action, undefined);
  assert.equal(parts('holding-turn', undefined, 'toyou')[0].action.turn, 'holding-turn');
  const unlinked = (from, to, selected) =>
    recordedRelationshipViews(
      { entry_id: 'self' },
      {
        edge: { ...edge, kind: 'relay', from, to },
        sourceTurn: 'holding-turn',
        targetTurn: 'receiving-turn',
      },
      selected,
      { parent, child, you: { name: 'You' } },
      {},
    )[0].parts;
  assert.equal(unlinked('you', 'child', 'other')[0].action, undefined);
  assert.equal(unlinked('parent', 'you', 'other')[2].action, undefined);
  assert.equal(unlinked('parent', 'parent', 'parent')[0].action, undefined);
  assert.equal(unlinked('parent', 'parent', 'parent')[2].action, undefined);
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

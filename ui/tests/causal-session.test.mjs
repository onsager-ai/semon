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
const { causalSessionPart, parseCausalTrace } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('incoming trace sender uses selected family identity without broadening the trace', () => {
  const data = {
    sessions: { selected: { name: 'Selected' } },
    metadata: {
      details: { family: { ancestors: [{ key: 'sender', name: 'Updated sender' }], runs: [] } },
    },
  };
  const before = JSON.stringify(data);
  assert.deepEqual(causalSessionPart(data, 'sender', 'sender-turn'), {
    className: 'who',
    text: 'Updated sender',
    action: { kind: 'session', id: 'sender', turn: 'sender-turn' },
  });
  assert.deepEqual(Object.keys(data.sessions), ['selected']);
  assert.equal(JSON.stringify(data), before);
  assert.equal(causalSessionPart(data, 'you').text, 'You');
  assert.equal(causalSessionPart(data, 'you').action, undefined);
  assert.equal(causalSessionPart(data, 'unrelated').text, 'Unrecorded session');
  assert.equal(causalSessionPart(data, 'unrelated').action, undefined);
  for (const id of ['constructor', 'toString', '__proto__']) {
    assert.equal(causalSessionPart(data, id).text, 'Unrecorded session');
    assert.equal(causalSessionPart(data, id).action, undefined);
  }
});
test('trace-owned identity wins and ambiguous family identities remain unavailable', () => {
  const data = {
    sessions: { selected: { name: 'Current selection' } },
    metadata: {
      details: {
        family: {
          ancestors: [
            { key: 'selected', name: 'Old selection' },
            { key: 'sender', name: 'One' },
          ],
          runs: [{ key: 'sender', name: 'Two' }],
        },
      },
    },
  };
  assert.equal(causalSessionPart(data, 'selected').text, 'Current selection');
  assert.equal(causalSessionPart(data, 'sender').text, 'Unrecorded session');
  assert.equal(causalSessionPart(data, 'sender').action, undefined);
});
test('qualified peer names preserve navigation without adding branch sessions', () => {
  const data = {
    sessions: { selected: { name: 'Selected' } },
    participants: { peer: 'Updated peer' },
  };
  const before = JSON.stringify(data);
  assert.deepEqual(causalSessionPart(data, 'peer'), {
    className: 'who',
    text: 'Updated peer',
    action: { kind: 'session', id: 'peer', turn: undefined },
  });
  assert.equal(JSON.stringify(data), before);
  assert.equal(causalSessionPart(data, 'unrelated').text, 'Unrecorded session');
  assert.equal(causalSessionPart(data, 'unrelated').action, undefined);
});
test('causal participant parser rejects unrelated or excessive identity scope', () => {
  const data = {
    api: 1,
    source_key: 'fixture',
    catalog_key: 'selected',
    root: 'turn',
    generation: 'generation',
    previews: {},
    participants: {},
    handoffs: [],
    sessions: {
      selected: {
        name: 'Selected',
        harness: 'claude',
        state: 'done',
        model: 'model',
        start: 1,
        last: 2,
        cost: { usd: null },
        tokens: [],
        busy: [],
        wait_edges: [],
        wait_edges_truncated: false,
      },
    },
    turns: [
      {
        sid: 'selected',
        id: 'turn',
        end: { st: 'done', why: 'replied' },
        sent: [],
        u: true,
        last: true,
      },
    ],
    frontier: [],
    incomplete: false,
    previews_incomplete: false,
  };
  assert.deepEqual(parseCausalTrace(data, 'fixture', 'selected', 'turn').participants, {});
  for (const id of ['__proto__', 'constructor', 'toString']) {
    const named = {
      ...data,
      catalog_key: id,
      sessions: Object.fromEntries([[id, data.sessions.selected]]),
      turns: [{ ...data.turns[0], sid: id }],
    };
    const parsed = parseCausalTrace(named, 'fixture', id, 'turn');
    assert.equal(parsed.turns[id][0].sid, id);
    assert.equal(causalSessionPart(parsed, id).text, 'Selected');
    assert.throws(
      () => parseCausalTrace({ ...data, turns: named.turns }, 'fixture', 'selected', 'turn'),
      /Invalid causal turn identity/,
    );
  }
  for (const participants of [
    { unrelated: 'Outside branch' },
    { you: 'Human' },
    { selected: 'Duplicate branch identity' },
    Object.fromEntries(Array.from({ length: 129 }, (_, index) => ['peer-' + index, 'Peer'])),
  ])
    assert.throws(
      () => parseCausalTrace({ ...data, participants }, 'fixture', 'selected', 'turn'),
      /Invalid causal participant scope/,
    );
});

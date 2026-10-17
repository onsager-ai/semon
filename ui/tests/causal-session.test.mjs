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
const {
  causalSessionPart,
  parseCausalTrace,
  reachableTraceSummary,
  traceRootActor,
  traceRootTime,
  traceHopActor,
  traceHopClass,
  traceHandoffIconKey,
  traceSessionChip,
  traceSentenceParts,
  traceHandoffOutcome,
  traceTurnOutcome,
  traceCostText,
  traceShortMoney,
} = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const relationship = (id, kind, from, to, fields = {}) => ({
  id,
  kind,
  from,
  to,
  at: 110,
  status: 'done',
  brief: '',
  ...fields,
});

test('trace summary counts only root-reachable outgoing handoffs and keeps incomplete context explicit', () => {
  const rootAsk = relationship('root-ask', 'ask', 'you', 'selected'),
    spawn = relationship('spawn', 'spawn', 'selected', 'child'),
    move = relationship('move', 'move', 'child', 'moved', {
      fromMachine: 'machine-a',
      toMachine: 'machine-b',
    }),
    result = relationship('result', 'toyou', 'selected', 'you', { ask: 'result' }),
    question = relationship('question', 'toyou', 'selected', 'you', { ask: 'question' }),
    relay = relationship('relay', 'relay', 'child', 'worker'),
    child = { id: 'child-turn', sid: 'child', sent: [relay] },
    worker = { id: 'worker-turn', sid: 'worker', sent: [] },
    root = {
      id: 'selected-turn',
      sid: 'selected',
      start: rootAsk,
      sent: [spawn, move, result, question],
    },
    starts = new Map([
      ['spawn', child],
      ['relay', worker],
    ]);

  assert.equal(reachableTraceSummary(root, starts), '3 sessions · 4 handoffs');
  assert.equal(
    reachableTraceSummary(root, starts, true),
    '3 sessions · 4 handoffs · some relationship context is unavailable',
  );
  assert.equal(traceRootActor(root), 'you');
  assert.equal(traceRootActor({ ...root, start: spawn }), 'selected');
  assert.equal(traceHopActor(result), 'selected');
  assert.equal(traceHopActor(spawn), 'child');
});

test('trace relationship sentences preserve recorded ask, subagent, relay and user-message wording', () => {
  const who = (id) => ({ className: 'who', text: id === 'you' ? 'You' : id }),
    parts = (edge, recipientKind) =>
      traceSentenceParts(edge, who, new Map(), undefined, recipientKind)
        .map((part) => part.text)
        .join('');

  assert.equal(parts(relationship('ask', 'ask', 'you', 'atlas')), 'You asked atlas');
  assert.equal(
    parts(relationship('spawn', 'spawn', 'atlas', 'child'), 'Subagent'),
    'atlas handed off to subagent child',
  );
  assert.equal(parts(relationship('relay', 'relay', 'atlas', 'child')), 'atlas relayed to child');
  assert.equal(
    parts(relationship('result', 'toyou', 'atlas', 'you', { ask: 'result' })),
    'atlas sent you a result',
  );
  assert.equal(
    parts(relationship('question', 'toyou', 'atlas', 'you', { ask: 'question' })),
    'atlas asked you',
  );
  assert.equal(
    parts(relationship('decision', 'toyou', 'atlas', 'you', { ask: 'decision' })),
    'atlas needs your decision',
  );
});

test('trace hops preserve the recorded icons, generic move wording, and missing-child marker', () => {
  const icon = (kind, fields = {}) =>
      traceHandoffIconKey(relationship(kind, kind, 'atlas', 'child', fields)),
    who = (id) => ({ className: 'who', text: id });

  assert.equal(icon('ask'), 'ask');
  assert.equal(icon('spawn'), 'out');
  assert.equal(icon('relay'), 'out');
  assert.equal(icon('move'), 'move');
  assert.equal(icon('toyou', { ask: 'question', status: 'work' }), 'qc');
  assert.equal(icon('toyou', { ask: 'question', status: 'done' }), 'done');
  assert.equal(icon('toyou', { ask: 'decision', status: 'wait' }), 'decide');
  assert.equal(icon('toyou', { ask: 'decision', status: 'done' }), 'done');
  assert.equal(icon('toyou', { ask: 'result', status: 'new' }), 'result');

  const move = relationship('move', 'move', 'atlas', 'target', {
    fromMachine: 'machine-a',
    toMachine: 'machine-b',
  });
  assert.equal(
    traceSentenceParts(move, who, new Map())
      .map((part) => part.text)
      .join(''),
    'Semon moved target',
  );
  assert.equal(
    traceHopClass(relationship('spawn', 'spawn', 'atlas', 'child'), false),
    'child k-spawn s-done stub',
  );
  assert.equal(
    traceHopClass(relationship('relay', 'relay', 'atlas', 'child'), true),
    'child k-relay s-done',
  );
  assert.equal(
    traceHopClass(relationship('move', 'move', 'atlas', 'target'), false),
    'child k-move s-done',
  );
  assert.equal(
    traceHopClass(relationship('toyou', 'toyou', 'atlas', 'you'), false),
    'child k-toyou s-done',
  );
});

test('trace root metadata does not invent time and restores the harness chip', () => {
  const formatTime = (at) => `at:${at}`,
    unstarted = { id: 'unstarted', sid: 'atlas', start: null, u: { k: 'u', text: '' }, at: 120 },
    started = {
      id: 'started',
      sid: 'atlas',
      start: relationship('start', 'ask', 'you', 'atlas'),
      at: 120,
    };
  assert.equal(traceRootTime(unstarted, formatTime), undefined);
  assert.equal(traceRootTime(started, formatTime), 'at:110');
  assert.deepEqual(traceSessionChip({ harness: 'claude' }), {
    chip: 'Claude Code',
    chipClass: 'claude',
  });
  assert.deepEqual(traceSessionChip({ harness: 'claude', kind: 'Subagent' }), {
    chip: 'Subagent',
    chipClass: 'claude',
  });
});

test('trace outcomes retain recorded time and distinguish locally viewed results', () => {
  const result = relationship('result', 'toyou', 'atlas', 'you', { ask: 'result' }),
    end = { st: 'done', why: 'toyou', h: 'result' },
    formatTime = (at) => `at:${at}`;

  assert.deepEqual(
    traceTurnOutcome(
      { id: 'turn', sid: 'atlas', end, sent: [result] },
      new Map([['result', result]]),
      (id) => id,
      formatTime,
    ),
    { state: 'done', text: 'Sent you a result · new · at:110' },
  );
  assert.deepEqual(
    traceTurnOutcome(
      { id: 'turn', sid: 'atlas', end, sent: [result] },
      new Map([['result', result]]),
      (id) => id,
      formatTime,
      new Set(['result']),
    ),
    { state: 'done', text: 'Sent you a result · read · at:110' },
  );
  assert.deepEqual(
    traceTurnOutcome(
      { id: 'working', sid: 'atlas', end: { st: 'work', why: 'working' }, sent: [] },
      new Map(),
      (id) => id,
      formatTime,
    ),
    { state: 'work', text: 'Still working' },
  );
  assert.deepEqual(
    traceTurnOutcome(
      { id: 'waiting', sid: 'atlas', end: { st: 'wait', why: 'input' }, sent: [] },
      new Map(),
      (id) => id,
      formatTime,
    ),
    { state: 'wait', text: 'Waiting for your input' },
  );
  assert.deepEqual(traceHandoffOutcome(result), { state: 'new', text: 'new' });
  assert.deepEqual(traceHandoffOutcome(result, new Set(['other'])), { state: 'new', text: 'new' });
  assert.deepEqual(traceHandoffOutcome(result, new Set(['result'])), {
    state: 'read',
    text: 'read',
  });
  assert.deepEqual(traceHandoffOutcome(relationship('move', 'move', 'atlas', 'atlas')), {
    state: 'done',
    text: 'Moved',
  });
  assert.deepEqual(
    traceHandoffOutcome(
      relationship('handoff', 'spawn', 'atlas', 'child', { result: 'Returned with the fix' }),
    ),
    { state: 'done', text: 'returned' },
  );
  assert.deepEqual(
    traceTurnOutcome(
      {
        id: 'returned',
        sid: 'child',
        start: relationship('handoff', 'spawn', 'atlas', 'child', { done: 150 }),
        end: { st: 'done', why: 'returned' },
        sent: [],
      },
      new Map(),
      (id) => (id === 'atlas' ? 'Atlas' : id),
      formatTime,
    ),
    { state: 'done', text: 'Returned to Atlas · at:150' },
  );
  assert.deepEqual(
    traceTurnOutcome(
      { id: 'unresolved', sid: 'atlas', end: { st: 'idle', why: 'unrecognized' }, sent: [] },
      new Map(),
      (id) => id,
      formatTime,
    ),
    { state: 'idle', text: 'No reply in these logs' },
  );
});

test('a user-directed turn waits only for a handoff recorded as waiting', () => {
  for (const status of ['new', 'work', 'err', 'done', 'wait']) {
    const handoff = relationship('to-you', 'toyou', 'atlas', 'you', {
      ask: 'question',
      status,
    });
    assert.equal(
      traceTurnOutcome(
        {
          id: 'turn',
          sid: 'atlas',
          end: { st: status, why: 'toyou', h: handoff.id },
          sent: [handoff],
        },
        new Map([[handoff.id, handoff]]),
        (id) => id,
        (at) => `at:${at}`,
      ).state,
      status === 'wait' ? 'wait' : 'done',
      status,
    );
  }
});

test('trace cost labels keep the original precision and mark unpriced totals', () => {
  assert.equal(traceCostText({ usd: 6.51 }), '$6.51');
  assert.equal(traceCostText({ usd: 6.51, unpriced_models: ['unknown-model'] }), '—');
  assert.equal(traceShortMoney(6.51), '$6.5');
});

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
        end: { st: 'done', why: 'replied', h: 'end-handoff' },
        sent: [],
        u: true,
        last: true,
      },
    ],
    frontier: [],
    incomplete: false,
    previews_incomplete: false,
  };
  const parsed = parseCausalTrace(data, 'fixture', 'selected', 'turn');
  assert.deepEqual(parsed.participants, {});
  assert.equal(parsed.turns.selected[0].end.h, 'end-handoff');
  const moveTrace = parseCausalTrace(
    {
      ...data,
      handoffs: [
        {
          ...relationship('move', 'move', 'selected', 'selected'),
          declined: false,
          from_machine: 'machine-a',
          to_machine: 'machine-b',
        },
      ],
      turns: [{ ...data.turns[0], sent: ['move'] }],
    },
    'fixture',
    'selected',
    'turn',
  );
  assert.equal(moveTrace.handoffs.get('move')?.fromMachine, '');
  assert.equal(moveTrace.handoffs.get('move')?.toMachine, '');
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

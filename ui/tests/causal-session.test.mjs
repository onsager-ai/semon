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
  traceHarnessClass,
  traceSessionHarnessClass,
  traceHandoffIconKey,
  traceSessionChip,
  traceSessionMeta,
  traceQualifiedUnjoinedRelay,
  traceMissingContextLocallyCovered,
  traceTokenTotal,
  traceTokenText,
  traceSentenceParts,
  traceHandoffOutcome,
  traceTurnOutcome,
  traceCostText,
  traceShortMoney,
} = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const { outputFiles: traceDomainFiles } = await build({
  entryPoints: [new URL('../src/domain/trace.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { createTraceCalculations } = await import(
  'data:text/javascript;base64,' + Buffer.from(traceDomainFiles[0].contents).toString('base64')
);
const relationship = (id, kind, from, to, fields = {}) => ({
  id,
  kind,
  from,
  to,
  at: 110,
  status: 'done',
  brief: '',
  declined: false,
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

test('trace token totals preserve exact integer sums and round positive half-million boundaries up', () => {
  const referencePrincipal = [0.52, 44.1, 0.23],
    nativePrincipal = [0.64, 43.98, 0.23],
    referenceSentinel = [0.12, 9.4, 0.03],
    nativeSentinel = [0.12, 9.4, 0.03],
    legacyLabel = (parts) =>
      ((parts.reduce((sum, value) => sum + value, 0) * 1_000_000) / 1_000_000).toFixed(1) + 'M';

  assert.equal(legacyLabel(nativePrincipal), '44.8M');
  assert.equal(legacyLabel(nativeSentinel), '9.5M');
  assert.equal(traceTokenTotal(referencePrincipal), 44_850_000);
  assert.equal(traceTokenTotal(nativePrincipal), 44_850_000);
  assert.equal(traceTokenTotal(referenceSentinel), 9_550_000);
  assert.equal(traceTokenTotal(nativeSentinel), 9_550_000);
  assert.equal(traceTokenText(traceTokenTotal(nativePrincipal)), '44.9M');
  assert.equal(traceTokenText(traceTokenTotal(nativeSentinel)), '9.6M');
  assert.equal(traceTokenText(44_849_000), '44.8M');
  assert.equal(traceTokenText(44_851_000), '44.9M');
  assert.equal(traceTokenText(999_499), '999k');
  assert.equal(traceTokenText(999_500), '1000k');
  assert.equal(traceTokenText(0), '0k');
});

test('createTraceCalculations agentSnapshot formats sessions and passive peers from integer totals', () => {
  const session = (id, tokens) => ({
      id,
      name: id,
      harness: 'claude',
      kind: 'Session',
      state: 'done',
      liveObservation: false,
      machine: 'local',
      model: 'claude-opus-5-5',
      start: 100,
      last: 200,
      tokens,
      cost: { usd: 0, unpriced_models: [] },
      busy: [],
      wait_edges: [],
      wait_edges_truncated: false,
    }),
    turn = (id, sid, sent = []) => ({
      id,
      sid,
      start: null,
      u: true,
      at: 100,
      end: { at: 200, st: 'done', why: 'replied', h: null },
      sent,
      last: false,
    }),
    sessions = {
      principal: session('principal', [0.64, 43.98, 0.23]),
      sentinel: session('sentinel', [0.12, 9.4, 0.03]),
      atlas: session('atlas', [0, 0, 0]),
    },
    participantSummaries = {
      harbor: {
        name: 'Harbor',
        harness: 'claude',
        kind: 'Subagent',
        model: 'claude-opus-5-5',
        tokens: [0.12, 9.4, 0.03],
        cost: { usd: 0, unpriced_models: [] },
      },
    },
    traceState = { sessions, participantSummaries, turns: {}, starts: new Map() },
    baseDomain = {
      costForSessions: () => ({ usd: 0, unpriced_models: [] }),
      costForSession: () => ({ usd: 0, unpriced_models: [] }),
      costText: () => '$0.00',
      shortMoney: () => '$0.00',
      hcls: () => '',
      usageTotal: (value) =>
        (value.tokens ?? []).reduce((sum, token) => sum + token, 0) * 1_000_000,
      nameOf: (id) => id,
    },
    calculator = createTraceCalculations(
      traceState,
      {
        ...baseDomain,
        usageTotal: (value) => traceTokenTotal(value.tokens ?? []),
        tokenTotal: traceTokenTotal,
        tokenText: traceTokenText,
      },
      () => 200,
      new Map(),
      { claude: 'Claude Code' },
      { done: 'Done', wait: 'Waiting', work: 'Working', idle: 'Idle', err: 'Error' },
    ),
    legacyCalculator = createTraceCalculations(
      traceState,
      baseDomain,
      () => 200,
      new Map(),
      { claude: 'Claude Code' },
      { done: 'Done', wait: 'Waiting', work: 'Working', idle: 'Idle', err: 'Error' },
    ),
    rowTokens = (chart, id) => chart.rows.find((row) => row.id === id)?.tokens;

  assert.equal(
    rowTokens(calculator.agentSnapshot(turn('principal-turn', 'principal')), 'principal'),
    '44.9M tokens',
  );
  assert.equal(
    rowTokens(calculator.agentSnapshot(turn('sentinel-turn', 'sentinel')), 'sentinel'),
    '9.6M tokens',
  );

  const relay = relationship('atlas-harbor', 'relay', 'atlas', 'harbor', { at: 110, done: 150 }),
    passiveTurn = turn('atlas-turn', 'atlas', [relay]),
    passiveChart = calculator.agentSnapshot(passiveTurn),
    legacyPassiveChart = legacyCalculator.agentSnapshot(passiveTurn);
  assert.equal(rowTokens(passiveChart, 'harbor'), '9.6M tokens');
  assert.equal(
    rowTokens(legacyCalculator.agentSnapshot(turn('principal-legacy', 'principal')), 'principal'),
    '44.8M tokens',
  );
  assert.equal(
    rowTokens(legacyCalculator.agentSnapshot(turn('sentinel-legacy', 'sentinel')), 'sentinel'),
    '9.5M tokens',
  );
  assert.equal(rowTokens(legacyPassiveChart, 'harbor'), '9.5M tokens');
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

test('Trace relationship names remain static prose', () => {
  const linkedName = (id, turn) => ({
      className: 'who',
      text: id,
      action: { kind: 'session', id, turn },
      label: 'Open ' + id,
    }),
    parts = traceSentenceParts(
      relationship('relay', 'relay', 'atlas', 'harbor'),
      linkedName,
      new Map(),
    );
  assert.equal(parts.map((part) => part.text).join(''), 'atlas relayed to harbor');
  assert.ok(parts.every((part) => part.action === undefined && part.label === undefined));
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
    chipClass: 'h-claude',
  });
  assert.deepEqual(traceSessionChip({ harness: 'claude', kind: 'Subagent' }), {
    chip: 'Subagent',
    chipClass: 'h-claude',
  });
});

test('trace harness classes use qualified CSS tokens and leave unknown harnesses neutral', () => {
  assert.equal(traceHarnessClass('atlas', 'claude'), 'h-claude');
  assert.equal(traceHarnessClass('you'), 'h-you');
  assert.equal(traceHarnessClass('atlas'), undefined);
  assert.equal(traceHarnessClass('atlas', 'unrecognized'), undefined);
  assert.equal(traceSessionChip({ harness: 'unrecognized' }).chipClass, undefined);
});

test('root node takes a name-only sender harness from selected-family metadata', () => {
  const data = {
      sessions: { 'a-review': { harness: 'claude' } },
      participants: { 'atlas-ingest': 'atlas ingest' },
      participantSummaries: {},
      metadata: {
        details: {
          family: {
            state: 'complete',
            ancestors: [{ key: 'atlas-ingest', harness: 'claude' }],
            runs: [],
          },
        },
      },
    },
    root = { sid: 'a-review', start: { from: 'atlas-ingest' }, u: null };

  assert.equal(traceRootActor(root), 'atlas-ingest');
  assert.equal(traceSessionHarnessClass(data, traceRootActor(root)), 'h-claude');
  assert.equal(traceSessionHarnessClass(data, 'unrecorded'), undefined);
  assert.equal(traceSessionHarnessClass({ ...data, participants: {} }, 'atlas-ingest'), undefined);
  assert.equal(
    traceSessionHarnessClass(
      {
        ...data,
        metadata: {
          details: {
            family: {
              state: 'complete',
              ancestors: [
                { key: 'atlas-ingest', harness: 'claude' },
                { key: 'atlas-ingest', harness: 'codex' },
              ],
              runs: [],
            },
          },
        },
      },
      'atlas-ingest',
    ),
    undefined,
  );
});

test('to-you hop metadata keeps its source session, current turn, and harness chip', () => {
  const edge = relationship('question', 'toyou', 'quill', 'you', {
      ask: 'question',
      status: 'wait',
    }),
    meta = traceSessionMeta(
      { name: 'quill', harness: 'claude' },
      edge.from,
      'current-turn',
      traceHandoffOutcome(edge),
    );

  assert.equal(meta.state, 'wait');
  assert.equal(meta.text, 'waiting on you');
  assert.equal(meta.session, 'quill');
  assert.equal(meta.turn, 'current-turn');
  assert.equal(meta.name, 'quill');
  assert.equal(meta.chip, 'Claude Code');
  assert.equal(meta.chipClass, 'h-claude');
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

test('Codex child handoff outcomes retain idle and unknown observations', () => {
  for (const [status, text] of [
    ['idle', 'idle'],
    ['off', 'status unknown'],
  ]) {
    const edge = relationship('child', 'spawn', 'claude-parent', 'codex-child', { status });
    assert.deepEqual(traceHandoffOutcome(edge), { state: status, text });
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

test('passive relay summaries are scoped catalog facts and retain safe session navigation', () => {
  const relay = relationship('relay', 'relay', 'selected', 'harbor'),
    data = {
      api: 1,
      source_key: 'fixture',
      catalog_key: 'selected',
      root: 'turn',
      generation: 'generation',
      previews: {},
      participants: { harbor: 'Harbor' },
      participant_summaries: {
        harbor: {
          name: 'Harbor',
          harness: 'codex',
          kind: 'Subagent',
          model: 'model-fixture',
          tokens: [1.5, 2, 0.5],
          cost: { usd: null, unpriced_models: ['unknown-model'] },
        },
      },
      handoffs: [relay],
      sessions: {
        selected: {
          name: 'Selected',
          harness: 'claude',
          state: 'done',
          model: 'model-fixture',
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
          sent: ['relay'],
          u: true,
          last: true,
        },
      ],
      frontier: [],
      incomplete: false,
      previews_incomplete: false,
    },
    parsed = parseCausalTrace(data, 'fixture', 'selected', 'turn');

  assert.deepEqual(parsed.participantSummaries.harbor, {
    name: 'Harbor',
    harness: 'codex',
    kind: 'Subagent',
    model: 'model-fixture',
    tokens: [1.5, 2, 0.5],
    cost: { usd: null, unpriced_models: ['unknown-model'] },
  });
  const extent = {
      scope: 'whole_session',
      last: 200,
      status: { state: 'work', observation: 'current' },
    },
    withExtent = (observed_extent) =>
      parseCausalTrace(
        {
          ...data,
          incomplete: true,
          incomplete_only_missing_receiving_turns: true,
          participant_summaries: {
            harbor: { ...data.participant_summaries.harbor, observed_extent },
          },
        },
        'fixture',
        'selected',
        'turn',
      ),
    qualified = withExtent(extent);
  assert.deepEqual(qualified.participantSummaries.harbor.observedExtent, {
    scope: 'whole_session',
    last: 200,
    status: { state: 'work', observation: 'current' },
  });
  assert.equal(qualified.starts.size, 0);
  assert.deepEqual(
    Object.values(qualified.turns)
      .flat()
      .map((turn) => turn.id),
    ['turn'],
  );
  assert.equal(Object.hasOwn(qualified.sessions, 'harbor'), false);
  assert.equal(traceMissingContextLocallyCovered(qualified), true);
  for (const changed of [
    { incompleteOnlyMissingReceivingTurns: false },
    { incompleteOnlyMissingReceivingTurns: undefined },
    { incomplete: false },
    { previewsIncomplete: true },
    { frontier: [{ catalog_key: 'harbor', turn: 'unread' }] },
    { participants: {} },
    { participants: { harbor: '' } },
    { root: { ...qualified.root, sent: [] } },
  ])
    assert.equal(traceMissingContextLocallyCovered({ ...qualified, ...changed }), false);
  assert.equal(traceMissingContextLocallyCovered(parsed), false);
  const activity = {
      state: 'available',
      kind: 'timestamp_clusters',
      intervals: [
        [10, 20],
        [30, 40],
      ],
    },
    activityExtent = (recorded_activity, status = extent.status) =>
      withExtent({ ...extent, status, recorded_activity });
  assert.deepEqual(
    activityExtent(activity).participantSummaries.harbor.observedExtent.recordedActivity,
    activity,
  );
  assert.deepEqual(
    activityExtent({ ...activity, intervals: [] }).participantSummaries.harbor.observedExtent
      .recordedActivity.intervals,
    [],
  );
  assert.deepEqual(
    activityExtent({ state: 'unavailable' }).participantSummaries.harbor.observedExtent
      .recordedActivity,
    { state: 'unavailable' },
  );
  assert.equal(traceMissingContextLocallyCovered(activityExtent({ state: 'unavailable' })), true);
  for (const invalid of [
    { state: 'unknown' },
    { state: 'unavailable', intervals: [] },
    { ...activity, kind: 'receiving_turn' },
    { ...activity, busy: [] },
    { ...activity, intervals: [[20, 10]] },
    { ...activity, intervals: [[-1, 10]] },
    { ...activity, intervals: [[1, Number.MAX_SAFE_INTEGER + 1]] },
    { ...activity, intervals: [[1.5, 2]] },
    {
      ...activity,
      intervals: [
        [10, 20],
        [15, 30],
      ],
    },
    {
      ...activity,
      intervals: [
        [30, 40],
        [10, 20],
      ],
    },
    { ...activity, intervals: Array(257).fill([0, 0]) },
  ])
    assert.throws(() => activityExtent(invalid));
  assert.throws(() => activityExtent(activity, { state: null, observation: 'unavailable' }));
  let visited = 0;
  const oversized = Array(257),
    oversizedPair = Array(3);
  Object.defineProperty(oversized, 0, {
    get() {
      visited++;
      return [0, 0];
    },
  });
  Object.defineProperty(oversizedPair, 0, {
    get() {
      visited++;
      return 0;
    },
  });
  assert.throws(
    () => activityExtent({ ...activity, intervals: oversized }),
    /Invalid causal recorded activity/,
  );
  assert.throws(
    () => activityExtent({ ...activity, intervals: [oversizedPair] }),
    /Invalid causal recorded activity/,
  );
  assert.equal(visited, 0, 'oversized collections are rejected before decoding their elements');
  assert.equal(
    withExtent({ ...extent, last: null }).participantSummaries.harbor.observedExtent.last,
    null,
  );
  for (const invalid of [
    { ...extent, scope: 'receiving_turn' },
    { ...extent, last: -1 },
    { ...extent, last: 1.5 },
    { ...extent, last: Number.MAX_SAFE_INTEGER + 1 },
    { ...extent, last: Infinity },
    { ...extent, busy: [[1, 2]] },
    { ...extent, status: undefined },
    { ...extent, status: { state: 'work', observation: 'retained' } },
    { ...extent, status: { state: 'work', observation: 'unavailable' } },
  ])
    assert.throws(() => withExtent(invalid));
  assert.deepEqual(causalSessionPart(parsed, 'harbor'), {
    className: 'who',
    text: 'Harbor',
    action: { kind: 'session', id: 'harbor', turn: undefined },
  });
  const relayEdge = parsed.handoffs.get('relay');
  assert.ok(relayEdge);
  assert.deepEqual(traceQualifiedUnjoinedRelay(parsed, relayEdge, false), {
    session: 'harbor',
    name: 'Harbor',
    summary: parsed.participantSummaries.harbor,
  });
  assert.deepEqual(
    traceQualifiedUnjoinedRelay({ ...parsed, participantSummaries: {} }, relayEdge, false),
    { session: 'harbor', name: 'Harbor', summary: undefined },
  );
  assert.equal(traceQualifiedUnjoinedRelay(parsed, relayEdge, true), undefined);
  assert.equal(
    traceQualifiedUnjoinedRelay(
      parsed,
      relationship('unqualified', 'relay', 'selected', 'unrecorded'),
      false,
    ),
    undefined,
  );
  assert.equal(
    traceQualifiedUnjoinedRelay(
      parsed,
      relationship('spawn', 'spawn', 'selected', 'harbor'),
      false,
    ),
    undefined,
  );
  assert.throws(
    () =>
      parseCausalTrace(
        {
          ...data,
          handoffs: [relationship('move', 'move', 'selected', 'harbor')],
          turns: [{ ...data.turns[0], sent: ['move'] }],
        },
        'fixture',
        'selected',
        'turn',
      ),
    /Invalid causal participant summary scope/,
  );
  assert.throws(
    () =>
      parseCausalTrace(
        {
          ...data,
          participant_summaries: {
            harbor: { ...data.participant_summaries.harbor, name: 'Changed identity' },
          },
        },
        'fixture',
        'selected',
        'turn',
      ),
    /Invalid causal participant summary scope/,
  );
  assert.throws(
    () =>
      parseCausalTrace(
        {
          ...data,
          participant_summaries: {
            harbor: { ...data.participant_summaries.harbor, cwd: '/private/path' },
          },
        },
        'fixture',
        'selected',
        'turn',
      ),
    /Invalid causal participant summary fields/,
  );
  assert.throws(
    () =>
      parseCausalTrace(
        {
          ...data,
          participant_summaries: {
            harbor: { ...data.participant_summaries.harbor, tokens: [1, 2] },
          },
        },
        'fixture',
        'selected',
        'turn',
      ),
    /Invalid causal participant summary metrics/,
  );
});

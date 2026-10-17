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
const { reachableResultIds, createTraceCalculations, traceParticipantObservedEnd } = await import(
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

test('qualified relay participants contribute session totals without joining causal activity', () => {
  const sessions = {
      root: {
        id: 'root',
        name: 'Atlas',
        harness: 'claude',
        state: 'done',
        machine: 'source',
        start: 100,
        last: 200,
        tokens: [1, 2, 0],
        cost: { usd: 1 },
      },
      joined: {
        id: 'joined',
        name: 'Joined relay',
        harness: 'codex',
        state: 'done',
        machine: 'source',
        start: 150,
        last: 180,
        tokens: [0.5, 1, 0],
        cost: { usd: 0.5 },
      },
    },
    relay = (id, to, at, done) => ({
      id,
      kind: 'relay',
      from: 'root',
      to,
      at,
      done,
      status: 'done',
      brief: '',
    }),
    root = {
      id: 'root-turn',
      sid: 'root',
      at: 100,
      end: { at: 200, st: 'done', why: 'replied' },
      sent: [
        relay('harbor-relay', 'harbor', 130, 150),
        relay('unpriced-relay', 'unpriced', 145, 155),
        relay('known-relay', 'joined', 160, 180),
        relay('constructor-relay', 'constructor', 170, 185),
        relay('prototype-relay', '__proto__', 180, 190),
        relay('harbor-again', 'harbor', 190, 195),
      ],
    },
    joinedTurn = {
      id: 'joined-turn',
      sid: 'joined',
      start: root.sent[2],
      sent: [],
      end: { at: 180 },
    },
    participantSummary = (name, cost) => ({
      name,
      harness: 'codex',
      kind: 'Subagent',
      model: 'model-fixture',
      tokens: [1.5, 2, 0.5],
      cost,
    }),
    snapshot = (participantSummaries, observedNow = 1000) => {
      const costForSessions = (rows) => ({
          usd: rows.some((row) => row.cost?.usd == null)
            ? null
            : rows.reduce((sum, row) => sum + row.cost.usd, 0),
          unpriced_models: [...new Set(rows.flatMap((row) => row.cost?.unpriced_models ?? []))],
        }),
        calculations = createTraceCalculations(
          {
            sessions,
            participantSummaries,
            turns: { root: [] },
            starts: new Map([['known-relay', joinedTurn]]),
          },
          {
            costForSessions,
            costForSession: (id) => sessions[id]?.cost ?? { usd: null },
            costText: (cost) =>
              cost.usd == null || cost.unpriced_models?.length ? '—' : '$' + cost.usd.toFixed(2),
            shortMoney: (usd) => '$' + usd.toFixed(1),
            hcls: (id) => sessions[id]?.harness ?? participantSummaries?.[id]?.harness ?? 'you',
            usageTotal: (session) =>
              (session.tokens ?? []).reduce((sum, value) => sum + value, 0) * 1e6,
            nameOf: (id) => sessions[id]?.name ?? id,
          },
          () => observedNow,
          new Map(),
          { claude: 'Claude Code', codex: 'Codex' },
          { done: 'Finished', work: 'Working', wait: 'Waiting', idle: 'Idle', err: 'Failed' },
        );
      return {
        ...calculations.agentSnapshot(root),
        tree: calculations.traceAgentTree(root),
      };
    },
    baseline = snapshot(undefined),
    withPeers = snapshot({
      harbor: participantSummary('Harbor', { usd: 2.25, unpriced_models: [] }),
      unpriced: participantSummary('Unpriced peer', {
        usd: 2.25,
        unpriced_models: ['unknown-model'],
      }),
      // A summary cannot turn an already joined relay into a duplicate peer row.
      joined: participantSummary('Joined relay', { usd: 0.5, unpriced_models: [] }),
    }),
    peers = withPeers.rows.filter((row) => row.kind === 'peer'),
    joinedRows = withPeers.rows.filter((row) => row.kind === 'agent'),
    knownPeer = snapshot({
      harbor: participantSummary('Harbor', { usd: 2.25, unpriced_models: [] }),
      joined: participantSummary('Joined relay', { usd: 0.5, unpriced_models: [] }),
    }),
    unknownPeer = snapshot({
      harbor: participantSummary('Harbor', { usd: null, unpriced_models: [] }),
    });

  assert.equal(peers.length, 2);
  assert.deepEqual(
    peers.map((row) => row.name),
    ['Harbor', 'Unpriced peer'],
  );
  assert.equal(peers[0].qualification, "Its turn isn't in these logs");
  assert.equal(peers[0].cost, '$2.3');
  assert.match(peers[0].costTip, /\$2\.25/);
  assert.equal(peers[1].cost, '—');
  assert.match(peers[1].label, /Its turn isn't in these logs/);
  for (const row of peers) {
    for (const absent of [
      'state',
      'stateLabel',
      'critical',
      'duration',
      'segments',
      'spawns',
      'waits',
      'turn',
      'edge',
    ])
      assert.equal(Object.hasOwn(row, absent), false, absent);
  }
  assert.deepEqual(
    joinedRows.map((row) => row.id),
    ['root', 'joined'],
  );
  assert.deepEqual(
    joinedRows,
    baseline.rows.filter((row) => row.kind === 'agent'),
  );
  assert.deepEqual(
    withPeers.tree.peers.map((node) => node.sid),
    ['joined'],
  );
  assert.deepEqual(
    withPeers.tree.passivePeers.map((node) => node.sid),
    ['harbor', 'unpriced'],
  );
  for (const peer of withPeers.tree.passivePeers) {
    assert.equal(Object.hasOwn(peer, 'turn'), false);
    assert.equal(Object.hasOwn(peer, 'children'), false);
  }
  for (const key of ['start', 'end', 'ticks', 'legend', 'incomplete'])
    assert.deepEqual(withPeers[key], baseline[key], key);
  assert.equal(baseline.summary[0], '2 agents');
  assert.equal(baseline.summary[2], '$1.50 session totals');
  // Repeated relays and a summary for an already joined session do not count twice.
  assert.equal(knownPeer.summary[0], '3 agents');
  assert.equal(knownPeer.summary[2], '$3.75 session totals');
  assert.equal(unknownPeer.summary[0], '3 agents');
  assert.equal(unknownPeer.summary[2], '— session totals');
  assert.equal(withPeers.summary[0], '4 agents');
  assert.equal(withPeers.summary[2], '— session totals');
  assert.equal(
    withPeers.summary.some((item) => item.includes('relayed peer')),
    false,
  );
  const observedSummary = (status, last = 800) => ({
      ...participantSummary('Harbor', { usd: 2.25, unpriced_models: [] }),
      observedExtent: { scope: 'whole_session', last, status },
    }),
    current = observedSummary({ state: 'work', observation: 'current' }),
    withExtent = snapshot({ harbor: current }, 100000),
    extentPeer = withExtent.rows.find((row) => row.kind === 'peer');
  assert.equal(traceParticipantObservedEnd(current, 1000), 1000);
  assert.equal(
    traceParticipantObservedEnd(observedSummary({ state: 'off', observation: 'retained' }), 1000),
    800,
  );
  assert.equal(
    traceParticipantObservedEnd(observedSummary({ state: 'done', observation: 'current' }), 1000),
    800,
  );
  for (const summary of [
    participantSummary('Harbor', { usd: 2.25 }),
    observedSummary({ state: null, observation: 'unavailable' }),
    observedSummary({ state: 'work', observation: 'retained' }),
    observedSummary({ state: 'wait', observation: 'retained' }),
    observedSummary({ state: 'done', observation: 'retained' }, null),
    observedSummary({ state: 'done', observation: 'current' }, -1),
    observedSummary({ state: 'done', observation: 'current' }, 1.5),
  ]) {
    assert.equal(traceParticipantObservedEnd(summary, 1000), undefined);
    const unavailable = snapshot({ harbor: summary });
    assert.equal(unavailable.end, baseline.end);
    assert.equal(unavailable.rows.find((row) => row.kind === 'peer').duration, undefined);
  }
  assert.equal(traceParticipantObservedEnd(current, Infinity), undefined);
  assert.notEqual(withExtent.end, baseline.end);
  assert.equal(withExtent.summary[0], '3 agents');
  assert.equal(withExtent.summary[2], '$3.75 session totals');
  assert.equal(extentPeer.state, 'work');
  assert.equal(extentPeer.stateLabel, 'Working');
  assert.equal(extentPeer.harnessClass, 'codex');
  assert.equal(extentPeer.relay, true);
  assert.equal(typeof extentPeer.duration, 'string');
  assert.match(extentPeer.extentTip, /Whole-session.*receiving turn/);
  assert.match(withExtent.extentTip, /whole-session.*do not add causal activity/);
  assert.equal(extentPeer.qualification, "Its turn isn't in these logs");
  const predatingRelay = snapshot({
    harbor: observedSummary({ state: 'done', observation: 'retained' }, 129),
  });
  assert.equal(predatingRelay.end, baseline.end);
  assert.equal(predatingRelay.extentTip, undefined);
  assert.equal(predatingRelay.rows.find((row) => row.kind === 'peer').duration, undefined);
  for (const absent of ['turn', 'segments', 'waits', 'critical', 'spawns', 'edge'])
    assert.equal(Object.hasOwn(extentPeer, absent), false, absent);
  assert.deepEqual(withExtent.tree.rootNode, knownPeer.tree.rootNode);
  assert.deepEqual(withExtent.tree.peers, knownPeer.tree.peers);
  assert.deepEqual(
    withExtent.tree.passivePeers.map((peer) => peer.sid),
    ['harbor'],
  );
  assert.deepEqual(reachableResultIds(root, new Map([['known-relay', joinedTurn]])), []);
  assert.deepEqual(
    withExtent.rows.filter((row) => row.kind === 'agent').map((row) => row.id),
    ['root', 'joined'],
  );
});

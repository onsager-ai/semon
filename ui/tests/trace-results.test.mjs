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
const { reachableResultIds, createTraceCalculations } = await import(
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

test('unjoined relay participants stay passive and do not affect joined-turn calculations', () => {
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
    snapshot = (participantSummaries) => {
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
            hcls: (id) => sessions[id]?.harness ?? 'you',
            usageTotal: (session) =>
              (session.tokens ?? []).reduce((sum, value) => sum + value, 0) * 1e6,
            nameOf: (id) => sessions[id]?.name ?? id,
          },
          () => 1000,
          new Map(),
          { claude: 'Claude Code', codex: 'Codex' },
          { done: 'Finished', work: 'Working', wait: 'Waiting', idle: 'Idle', err: 'Failed' },
        );
      return calculations.agentSnapshot(root);
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
    joinedRows = withPeers.rows.filter((row) => row.kind === 'agent');

  assert.equal(peers.length, 2);
  assert.deepEqual(
    peers.map((row) => row.name),
    ['Harbor', 'Unpriced peer'],
  );
  assert.equal(peers[0].qualification, 'No joined turn in this trace');
  assert.equal(peers[0].cost, '$2.3');
  assert.match(peers[0].costTip, /\$2\.25/);
  assert.equal(peers[1].cost, '—');
  assert.match(peers[1].label, /No joined turn in this trace/);
  for (const row of peers) {
    for (const absent of [
      'state',
      'stateLabel',
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
  for (const key of ['start', 'end', 'ticks', 'legend', 'incomplete'])
    assert.deepEqual(withPeers[key], baseline[key], key);
  assert.equal(baseline.summary[2], '$1.50 session totals');
  assert.equal(withPeers.summary[2], '$1.50 joined session totals');
  assert.ok(withPeers.summary.some((item) => item.includes('2 relayed peers with catalog totals')));
});

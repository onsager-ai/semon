import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/app/catalogTranscriptView.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { catalogTranscriptBlocks, recordedRelationshipViews } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('recorded child cards retain kind, duration, steps and cost without claiming historical activity', () => {
  const previous = globalThis.matchMedia;
  globalThis.matchMedia = () => ({ matches: false });
  try {
    const [card] = recordedRelationshipViews(
      entry(1, 'turn', { k: 'h', id: 'spawn' }),
      { edge: { id: 'spawn', kind: 'spawn', from: 'owner', to: 'child', status: 'work' } },
      'owner',
      {
        child: {
          name: 'Review parser',
          harness: 'claude',
          model: 'claude-sonnet-5',
          kind: 'subagent',
          state: 'work',
          start: 1000,
          last: 181000,
          calls: 2,
          usd: 0.61,
          live_observation: false,
        },
      },
      { brief: 'Inspect parser safety' },
    );
    assert.equal(card.state, 'off');
    assert.equal(card.meta, 'Subagent · sonnet 5 · 3m · 2 steps · $0.61');
    assert.equal(card.brief, 'Inspect parser safety');
    assert.equal(card.meta.match(/Subagent/g).length, 1);
    assert.equal(card.failed, false);
  } finally {
    globalThis.matchMedia = previous;
  }
});
const entry = (slot, turn, data) => ({
  slot,
  entry_id: 'native-' + slot,
  entry: { ...data, turn },
  freshness: { state: 'cached' },
});
const entries = () => [
  entry(0, 'turn', { k: 'a', text: 'Earlier reply' }),
  entry(1, 'other', { k: 'a', text: 'Exact result' }),
  entry(2, 'turn', { k: 'a', text: 'Exact result' }),
  entry(3, 'turn', { k: 'h', id: 'result' }),
];
const context = () => ({
  state: 'complete',
  turn_ids: ['turn', 'other'],
  previews: {},
  sessions: {},
  turns: [
    { id: 'turn', u: true, sent: ['result'], end: { st: 'done', why: 'result' } },
    { id: 'other', u: true, sent: [], end: { st: 'done', why: 'result' } },
  ],
  handoffs: [{ edge: { id: 'result', from: 'owner', to: 'you', kind: 'toyou', ask: 'result' } }],
});
const event = (item) => [
  {
    kind: 'event',
    key: item.entry_id,
    entryKey: item.entry_id,
    handoff: 'result',
    brief: 'Old preview',
  },
];
const values = (blocks) =>
  blocks.filter((block) => block.kind === 'turn').flatMap((block) => block.turn.entries);
test('a native result folds only its closing reply, preserving other accepted identities', () => {
  const shown = values(
    catalogTranscriptBlocks(
      entries(),
      undefined,
      event,
      false,
      context(),
      undefined,
      'claude',
      'owner',
    ),
  );
  assert.equal(shown.find((row) => row.kind === 'event').brief, 'Exact result');
  assert.deepEqual(
    shown.filter((row) => row.kind === 'message').map((row) => row.entryKey),
    ['native-0', 'native-1'],
  );
  assert.equal(shown.find((row) => row.kind === 'event').entryKey, 'native-3');
});
test('unrelated, filtered, or incomplete result context never consumes a native reply', () => {
  for (const change of [
    (value) => {
      value.handoffs[0].edge.from = 'elsewhere';
    },
    (value) => {
      value.turns[0].sent = [];
    },
  ]) {
    const metadata = context();
    change(metadata);
    assert.equal(
      values(
        catalogTranscriptBlocks(
          entries(),
          undefined,
          event,
          false,
          metadata,
          undefined,
          'claude',
          'owner',
        ),
      ).filter((row) => row.kind === 'message').length,
      3,
    );
  }
  assert.equal(
    values(
      catalogTranscriptBlocks(
        entries().slice(0, 3),
        undefined,
        event,
        false,
        context(),
        undefined,
        'claude',
        'owner',
      ),
    ).filter((row) => row.kind === 'message').length,
    3,
  );
});

test('only a matching current observation presents an unfinished native call as running', () => {
  const call = {
    ...entry(0, 'turn', { k: 'tool', name: 'Bash', arg: 'cargo test', ok: null, secs: '—' }),
    observed_at: 1000,
  };
  const activity = { tool: 'Bash', since: 1000 };
  const present = (item, live = true, observed = activity) =>
    values(
      catalogTranscriptBlocks(
        [item],
        undefined,
        undefined,
        live,
        undefined,
        undefined,
        'claude',
        'selected',
        observed,
      ),
    ).flatMap((row) => (row.kind === 'group' ? row.entries : [row]))[0].step;
  assert.equal(present(call).running, true);
  assert.equal(present(call).since, 1000);
  assert.equal(present(call).sid, 'selected');
  assert.equal(present(call, false).running, false);
  assert.equal(present(call, true, { ...activity, since: 999 }).running, false);
  assert.equal(present(call, true, { ...activity, tool: 'Read' }).running, false);
  assert.equal(present({ ...call, entry: { ...call.entry, ok: true } }).running, false);
});

test('derived return labels require a matching source-qualified spawn and recorded return time', () => {
  const previous = globalThis.matchMedia;
  globalThis.matchMedia = () => ({ matches: false });
  try {
    const ret = {
      ...entry(1, 'turn', { k: 'end', ret: { to: 'parent', at: 1000, failed: false } }),
      provenance: null,
    };
    const metadata = {
      ...context(),
      turns: [
        { id: 'turn', start: 'spawn', u: false, sent: [], end: { st: 'done', why: 'result' } },
      ],
      sessions: { parent: { name: 'Parent task', harness: 'claude', model: 'model' } },
      handoffs: [
        { edge: { id: 'spawn', kind: 'spawn', from: 'parent', to: 'selected', at: 0, done: 1000 } },
      ],
    };
    const present = (value) =>
      values(
        catalogTranscriptBlocks(
          [ret],
          undefined,
          undefined,
          false,
          value,
          undefined,
          'claude',
          'selected',
        ),
      )[0].text;
    assert.match(present(metadata), /^Returned to Parent task · /);
    assert.equal(
      present({ ...metadata, handoffs: [] }),
      'Session boundary · native provenance unavailable',
    );
    assert.equal(
      present({ ...metadata, sessions: {} }),
      'Session boundary · native provenance unavailable',
    );
    assert.equal(
      present({ ...metadata, handoffs: [{ edge: { ...metadata.handoffs[0].edge, done: 999 } }] }),
      'Session boundary · native provenance unavailable',
    );
  } finally {
    globalThis.matchMedia = previous;
  }
});

test('a qualified failed return reports the outcome once and incomplete provenance keeps its failure', () => {
  const previous = globalThis.matchMedia;
  globalThis.matchMedia = () => ({ matches: false });
  try {
    const returned = entry(0, 'turn', {
      k: 'end',
      ret: { to: 'parent', at: 1000, failed: true },
    });
    const metadata = {
      ...context(),
      turns: [
        { id: 'turn', start: 'spawn', u: false, sent: [], end: { st: 'err', why: 'failed' } },
      ],
      sessions: { parent: { name: 'Parent task', harness: 'claude' } },
      handoffs: [
        { edge: { id: 'spawn', kind: 'spawn', from: 'parent', to: 'selected', at: 0, done: 1000 } },
      ],
    };
    const present = (value) =>
      catalogTranscriptBlocks(
        [returned],
        undefined,
        undefined,
        false,
        value,
        undefined,
        'claude',
        'selected',
      )[0].turn;
    const qualified = present(metadata);
    assert.match(qualified.entries[0].text, /^Returned to Parent task · failed · /);
    assert.equal(qualified.error, undefined);
    for (const value of [
      { ...metadata, sessions: {} },
      { ...metadata, handoffs: [] },
      { ...metadata, handoffs: [{ edge: { ...metadata.handoffs[0].edge, done: 999 } }] },
    ]) {
      assert.equal(present(value).error, 'Failed');
      assert.equal(
        present(value).entries[0].text,
        'Session boundary · native provenance unavailable',
      );
    }
  } finally {
    globalThis.matchMedia = previous;
  }
});

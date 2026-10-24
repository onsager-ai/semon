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
test('a native result uses its closing reply while preserving every accepted identity', () => {
  const accepted = entries();
  const original = JSON.stringify(accepted);
  const shown = values(
    catalogTranscriptBlocks(
      accepted,
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
    ['native-0', 'native-1', 'native-2'],
  );
  assert.equal(shown.find((row) => row.kind === 'event').entryKey, 'native-3');
  assert.equal(shown.find((row) => row.entryKey === 'native-2').text, 'Exact result');
  assert.equal(shown.find((row) => row.entryKey === 'native-1').text, 'Exact result');
  assert.equal(JSON.stringify(accepted), original);
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

test('unavailable, clipped and long replies remain independently readable', () => {
  for (const change of [
    (rows, metadata) => {
      metadata.state = 'partial';
    },
    (rows) => {
      rows[2].clipped = true;
    },
    (rows) => {
      rows[2].entry.text = 'Long result '.repeat(1000);
    },
    (rows) => {
      rows[2].entry.img = [{ url: '/image' }];
    },
  ]) {
    const rows = entries(),
      metadata = context();
    change(rows, metadata);
    const shown = values(
      catalogTranscriptBlocks(
        rows,
        undefined,
        event,
        false,
        metadata,
        undefined,
        'claude',
        'owner',
      ),
    );
    assert.equal(shown.find((row) => row.entryKey === 'native-2').kind, 'message');
    assert.equal(shown.find((row) => row.entryKey === 'native-2').text, rows[2].entry.text);
  }
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

test('outgoing human questions and peer relays retain the established heading and direction', () => {
  const question = recordedRelationshipViews(
    entry(1, 'turn', { k: 'h', id: 'ask' }),
    {
      edge: {
        id: 'ask',
        kind: 'toyou',
        ask: 'question',
        from: 'owner',
        to: 'you',
        status: 'wait',
        at: 0,
      },
    },
    'owner',
    { owner: { name: 'Native owner' } },
    { brief: 'Recorded question' },
  )[0];
  assert.equal(question.parts.map((part) => part.text).join(''), 'Native owner asked you');
  assert.equal(question.brief, 'Recorded question');
  assert.equal(question.parts[0].action, undefined);
  const relay = recordedRelationshipViews(
    entry(2, 'turn', { k: 'h', id: 'relay' }),
    { edge: { id: 'relay', kind: 'relay', from: 'owner', to: 'peer', status: 'done', at: 0 } },
    'owner',
    { owner: { name: 'Native owner' }, peer: { name: 'Peer' } },
    { brief: 'Recorded handoff' },
  )[0];
  assert.equal(relay.parts.map((part) => part.text).join(''), 'Native owner relayed to Peer');
  assert.equal(relay.icon, 'M7 17L17 7M9 7h8v8');
  assert.equal(relay.parts[2].action.id, 'peer');
  assert.equal(relay.parts[0].action, undefined);
});

test('tool previews keep qualified expansion without repeating availability warnings in the transcript', () => {
  const call = entry(0, 'turn', {
    k: 'tool',
    name: 'exec_command',
    arg: 'synthetic command',
    out: 'Recorded preview',
    ok: true,
    more: ['legacy-output-reference'],
  });
  for (const expanded of [false, true]) {
    const shown = values(
      catalogTranscriptBlocks([call], () =>
        expanded
          ? {
              text: 'Complete recorded output',
              note: 'Complete recorded text loaded.',
            }
          : {
              note: 'Recorded text preview.',
              action: { label: 'Load recorded text', busy: false, run() {} },
            },
      ),
    ).flatMap((row) => (row.kind === 'group' ? row.entries : [row]));
    assert.equal(shown.filter((item) => item.kind === 'tool').length, 1);
    assert.equal(
      shown.some((item) => item.kind === 'label' && item.text.includes('Full result unavailable')),
      false,
    );
    assert.equal(shown.find((item) => item.kind === 'tool').step.data.more, undefined);
    if (expanded)
      assert.equal(
        shown.find((item) => item.kind === 'tool').step.data.out,
        'Complete recorded output',
      );
  }
});

test('clipped tool notices stay with tool details while message loading actions and signals remain', () => {
  const rows = [
    {
      ...entry(0, 'turn', {
        k: 'tool',
        name: 'Read',
        arg: 'first.txt',
        out: 'First preview',
        ok: true,
      }),
      clipped: true,
    },
    entry(1, 'turn', { k: 'signal', signal: { kind: 'effort', tag: 'high' } }),
    {
      ...entry(2, 'turn', {
        k: 'tool',
        name: 'Read',
        arg: 'second.txt',
        out: 'Second preview',
        ok: true,
      }),
      clipped: true,
    },
    {
      ...entry(3, 'turn', {
        k: 'tool',
        name: 'Read',
        arg: 'unsupported.txt',
        out: 'Unsupported preview',
        ok: true,
      }),
      clipped: true,
    },
    { ...entry(4, 'turn', { k: 'u', text: 'Long user message preview' }), clipped: true },
    { ...entry(5, 'turn', { k: 'a', text: 'Long assistant message preview' }), clipped: true },
  ];
  const fieldView = (item) => {
    if (!item.clipped || (item.entry.k === 'tool' && item.slot === 3)) return null;
    return {
      note:
        item.entry.k === 'tool'
          ? `tool field ${item.slot} preview`
          : `${item.entry.k} field preview`,
      action: { label: 'Load recorded text', busy: false, run() {} },
    };
  };
  const shown = values(catalogTranscriptBlocks(rows, fieldView)),
    groups = shown.filter((row) => row.kind === 'group'),
    notices = shown.filter((row) => row.kind === 'label' && row.className === 'vnote');
  assert.equal(groups.length, 1);
  assert.equal(groups[0].entries.length, 3);
  assert.equal(groups[0].entries[2].step.data.expandable, true);
  assert.equal(
    notices.some((row) => /tool field|complete source text is not loaded/i.test(row.text)),
    false,
  );
  assert.deepEqual(
    notices.map((row) => [row.text, row.action?.label]),
    [
      ['u field preview', 'Load recorded text'],
      ['a field preview', 'Load recorded text'],
    ],
  );
  assert.equal(
    shown.filter((row) => row.kind === 'label' && row.className === 'signal-marker').length,
    1,
  );
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  stdin: {
    contents:
      "export {parseSessionDetails} from './src/domain/sessionDetails'; export {selectedSessionMenu, selectedSessionLabels, selectedSessionState, catalogSessionRow, catalogSessionGroups} from './src/app/sessionPresentation';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const {
  parseSessionDetails,
  selectedSessionMenu,
  selectedSessionLabels,
  selectedSessionState,
  catalogSessionRow,
  catalogSessionGroups,
} = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
globalThis.matchMedia = () => ({ matches: false });
const details = () => ({
  state: 'idle',
  turns: 2,
  calls: 1,
  errors: 0,
  tool_calls: { Read: 1 },
  signals: {},
  cost: {
    usd: null,
    unpriced_models: ['native-model'],
    by_model: { 'native-model': { usd: null, tokens: { input: 7, output: 3 }, usd_by_kind: {} } },
  },
  copied_usage: true,
  copilot: null,
  reported_runs: [],
  cost_check: [],
});
test('selected usage preserves unknown cost and copied usage while native resume identity stays distinct', () => {
  const metadata = parseSessionDetails(details());
  const menu = selectedSessionMenu(
    {
      key: 'viewer-key',
      name: 'Recorded',
      harness: 'claude',
      kind: 'Claude Code',
      native_ids: ['native;id'],
      repo: null,
      branch: null,
      model: 'native-model',
      effort: null,
      start: 0,
      last: 1000,
      cost: { usd: null, unpriced_models: ['native-model'] },
      details: metadata,
    },
    'source',
    false,
    true,
  );
  assert.equal(menu.command, "claude --resume 'native;id'");
  assert.equal(menu.cost.figure, '—');
  assert.match(menu.cost.note, /Copied usage is excluded.*unknown/);
  assert.equal(menu.cost.models[0].rows[0].count, '7');
  assert.equal(menu.cost.models[0].rows[0].cost, '—');
  assert.equal(
    menu.actions.some(({ key }) => key === 'source'),
    false,
  );
});
test('malformed selected metadata and excessive model breakdowns cannot replace an accepted session', () => {
  for (const replacement of [
    { calls: -1 },
    { turns: 0.5 },
    { state: 'invented' },
    { copied_usage: 'yes' },
  ])
    assert.throws(() => parseSessionDetails({ ...details(), ...replacement }));
  const huge = details();
  huge.cost.by_model = Object.fromEntries(
    Array.from({ length: 257 }, (_, i) => [String(i), { usd: null }]),
  );
  assert.throws(() => parseSessionDetails(huge), /bound/);
});

test('failed-step navigation stays in the desktop bar and the phone menu', () => {
  const raw = { ...details(), errors: 2 },
    session = {
      key: 'selected',
      name: 'Selected',
      harness: 'claude',
      native_ids: ['native'],
      model: 'native-model',
      cost: raw.cost,
      details: parseSessionDetails(raw),
    },
    previousMatchMedia = globalThis.matchMedia;
  try {
    for (const phone of [false, true]) {
      globalThis.matchMedia = (query) => {
        assert.ok(['(max-width: 760px)', '(prefers-color-scheme: dark)'].includes(query));
        return { matches: query === '(max-width: 760px)' && phone };
      };
      const menu = selectedSessionMenu(session, 'machine', false, true),
        errorAction = menu.actions.find(({ key }) => key === 'errors'),
        barAction = selectedSessionLabels(session, 'machine').find(({ key }) => key === 'errors');
      assert.equal(barAction.action, 'errors');
      assert.equal(barAction.text, '2 failed');
      assert.equal(barAction.tip, 'Recorded failed steps');
      assert.equal(menu.details.find(({ label }) => label === 'Errors').value, '2');
      if (phone)
        assert.deepEqual(errorAction, {
          key: 'errors',
          text: '2 failed',
          dot: 'err',
          note: 'Step through',
          className: 'menu-errors',
        });
      else assert.equal(errorAction, undefined);
      assert.equal(
        menu.actions.some(({ key }) => key === 'wide'),
        !phone,
      );
    }
    session.details = parseSessionDetails({ ...raw, errors: 0 });
    assert.equal(
      selectedSessionMenu(session, 'machine', false, true).actions.some(
        ({ key }) => key === 'errors',
      ),
      false,
    );
  } finally {
    globalThis.matchMedia = previousMatchMedia;
  }
});

test('selected family costs, ancestry and mismatches remain bounded and unknown totals stay unknown', () => {
  const relative = (key, usd) => ({
    key,
    name: key,
    harness: 'claude',
    kind: 'subagent',
    parent: 'selected',
    cost: { usd, by_model: { m: { usd, tokens: { input: 7 }, usd_by_kind: { input: usd ?? 0 } } } },
  });
  const raw = {
    ...details(),
    copied_usage: false,
    cost: relative('selected', 1).cost,
    cwd: '/recorded/directory',
    live_observation: false,
    state: 'work',
    family: {
      state: 'complete',
      ancestors: [relative('parent', 10)],
      runs: [relative('child', 2)],
    },
    cost_check: [{ start: 0, computed_usd: 1.5, reported_usd: 1, ok: false }],
  };
  const session = {
    key: 'selected',
    name: 'Selected',
    harness: 'claude',
    kind: 'session',
    native_ids: ['native'],
    cost: raw.cost,
    model: 'm',
    start: 0,
    last: 1000,
    details: parseSessionDetails(raw),
  };
  const menu = selectedSessionMenu(session, 'machine', false, false);
  assert.equal(menu.cost.figure, '$3.00');
  assert.equal(menu.cost.caption, 'this session and its 1 run');
  assert.equal(menu.cost.models[0].rows[0].count, '14');
  assert.deepEqual(menu.cost.details.slice(0, 2), [
    { label: 'This session', value: '$1.00' },
    { label: 'Its run', value: '$2.00' },
  ]);
  assert.equal(menu.cost.runs[0].id, 'child');
  assert.equal(menu.cost.runs[0].state, 'off');
  assert.equal(menu.cost.runs[0].stateLabel, 'Status unavailable');
  assert.equal(menu.cost.runs[0].label, 'Open child, subagent, Status unavailable, $2.00');
  raw.family.runs[0].status = { state: 'work', observation: 'current' };
  session.details = parseSessionDetails(raw);
  const currentRun = selectedSessionMenu(session, 'machine', false, false).cost.runs[0];
  assert.equal(currentRun.state, 'work');
  assert.equal(currentRun.stateLabel, 'Working');
  assert.equal(currentRun.label, 'Open child, subagent, Working, $2.00');
  raw.family.runs[0].status = { state: 'off', observation: 'retained' };
  session.details = parseSessionDetails(raw);
  assert.equal(selectedSessionMenu(session, 'machine', false, false).cost.runs[0].state, 'off');
  assert.deepEqual(
    menu.path.map((row) => row.id),
    ['parent', 'selected'],
  );
  assert.equal(
    menu.details.some((row) => row.label === 'Directory'),
    false,
  );
  const repoFallback = selectedSessionMenu(
    {
      ...session,
      repo: '/recorded/repository',
      details: { ...session.details, cwd: undefined },
    },
    'machine',
    false,
    false,
  );
  assert.equal(
    repoFallback.details.some((row) => row.label === 'Directory'),
    false,
  );
  assert.match(menu.cost.mismatch, /50% above/);
  assert.equal(selectedSessionState(session), 'off');
  session.details.live_observation = true;
  assert.equal(selectedSessionState(session), 'work');
  raw.family.state = 'incomplete';
  session.details = parseSessionDetails(raw);
  assert.equal(selectedSessionMenu(session, 'machine', false, false).cost.figure, '—');
  assert.equal(selectedSessionMenu(session, 'machine', false, false).cost.details[1].value, '—');
  raw.family.state = 'complete';
  raw.family.runs[0].cost.usd = null;
  session.details = parseSessionDetails(raw);
  assert.equal(selectedSessionMenu(session, 'machine', false, false).cost.figure, '—');
  assert.equal(selectedSessionMenu(session, 'machine', false, false).cost.details[1].value, '—');
  raw.family.runs.push(raw.family.runs[0]);
  assert.throws(() => parseSessionDetails(raw), /identity/);
});

test('selected run costs retain nested reading order, depth and original reported-cost copy', () => {
  const relative = (key, parent, last) => ({
    key,
    parent,
    last,
    name: key,
    harness: 'claude',
    kind: 'subagent',
    cost: { usd: 1 },
  });
  const raw = {
    ...details(),
    copied_usage: false,
    cost: { usd: 2 },
    family: {
      state: 'complete',
      ancestors: [],
      runs: [
        relative('second', 'selected', 1),
        relative('first', 'selected', 3),
        relative('nested', 'first', 4),
        relative('deep', 'nested', 5),
      ],
    },
    reported_runs: [{ start: 0, cost_usd: 1 }],
    cost_check: [{ start: 0, computed_usd: 2, reported_usd: 1, ok: false }],
  };
  const session = {
    key: 'selected',
    name: 'Selected',
    harness: 'claude',
    native_ids: ['native'],
    model: 'm',
    cost: raw.cost,
    details: parseSessionDetails(raw),
  };
  const menu = selectedSessionMenu(session, 'machine', false, false);
  assert.deepEqual(
    menu.cost.runs.map(({ id, depth }) => [id, depth]),
    [
      ['first', 0],
      ['nested', 1],
      ['deep', 2],
      ['second', 0],
    ],
  );
  assert.deepEqual(menu.cost.details.slice(0, 2), [
    { label: 'This session', value: '$2.00' },
    { label: 'Its 4 runs', value: '$4.00' },
  ]);
  assert.equal(menu.cost.details[2].value, '$1.00, last run');
  assert.equal(
    menu.cost.mismatch,
    "Semon's estimate for that run is 100% above Claude Code's figure: API rates differ from what a plan is charged.",
  );
  raw.reported_runs.push({ start: 1, cost_usd: 2 });
  session.details = parseSessionDetails(raw);
  assert.equal(
    selectedSessionMenu(session, 'machine', false, false).cost.details[2].value,
    '$3.00, last 2 runs',
  );
  raw.cost_check = [{ start: 0, computed_usd: 2, reported_usd: 0, ok: false }];
  session.details = parseSessionDetails(raw);
  assert.match(selectedSessionMenu(session, 'machine', false, false).cost.mismatch, /100% above/);
});

test('list status labels preserve recorded observations without enabling controls', () => {
  const session = { key: 'selected', name: 'Selected', harness: 'claude', model: 'm', last: null };
  for (const [state, label] of [
    ['work', 'Working'],
    ['wait', 'Needs you'],
    ['done', 'Done'],
    ['err', 'Failed'],
  ]) {
    session.status = { state, observation: 'current' };
    const row = catalogSessionRow(session);
    assert.equal(row.state, state);
    assert.equal(row.stateLabel, label);
    assert.equal(selectedSessionState(session), 'off');
  }
  session.status = { state: null, observation: 'unavailable' };
  assert.equal(catalogSessionRow(session).stateLabel, 'Status unavailable');
  session.status = { state: 'off', observation: 'retained' };
  assert.equal(catalogSessionRow(session).stateLabel, 'Recorded history');
});

test('Catalog grouping keeps loaded descendants and missing parents selectable without identity guesses', () => {
  const items = [
    { key: 'grandchild', parent: 'child' },
    { key: 'unrelated', parent: null },
    { key: 'child', parent: 'parent' },
    { key: 'parent', parent: null },
    { key: 'orphan', parent: 'unloaded' },
    { key: 'self', parent: 'self' },
  ];
  const before = JSON.stringify(items);
  const groups = catalogSessionGroups(items);
  assert.deepEqual(
    groups.map((group) => group.root.key),
    ['unrelated', 'parent', 'orphan', 'self'],
  );
  assert.deepEqual(
    groups.find((group) => group.root.key === 'parent').children.map((child) => child.key),
    ['child', 'grandchild'],
  );
  assert.equal(JSON.stringify(items), before);
  assert.deepEqual(
    groups
      .flatMap((group) => [group.root, ...group.children])
      .map((item) => item.key)
      .sort(),
    items.map((item) => item.key).sort(),
  );
});
test('cyclic Catalog parents have a deterministic root and show every loaded row exactly once', () => {
  const items = [
    { key: 'b', parent: 'a' },
    { key: 'a', parent: 'b' },
    { key: 'c', parent: 'b' },
  ];
  for (const ordered of [items, [...items].reverse()]) {
    const groups = catalogSessionGroups(ordered);
    assert.equal(groups.length, 1);
    assert.equal(groups[0].root.key, 'a');
    assert.deepEqual(
      groups[0].children.map((item) => item.key),
      ['b', 'c'],
    );
  }
});

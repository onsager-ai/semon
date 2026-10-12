import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  stdin: {
    contents:
      "export {parseSessionDetails} from './src/domain/sessionDetails'; export {selectedSessionMenu, selectedSessionState, catalogSessionRow, catalogSessionGroups} from './src/app/sessionPresentation';",
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
  assert.equal(menu.cost.runs[0].id, 'child');
  assert.deepEqual(
    menu.path.map((row) => row.id),
    ['parent', 'selected'],
  );
  assert.equal(menu.details.find((row) => row.label === 'Directory').value, '/recorded/directory');
  assert.match(menu.cost.mismatch, /50% above/);
  assert.equal(selectedSessionState(session), 'off');
  session.details.live_observation = true;
  assert.equal(selectedSessionState(session), 'work');
  raw.family.state = 'incomplete';
  session.details = parseSessionDetails(raw);
  assert.equal(selectedSessionMenu(session, 'machine', false, false).cost.figure, '—');
  raw.family.state = 'complete';
  raw.family.runs[0].cost.usd = null;
  session.details = parseSessionDetails(raw);
  assert.equal(selectedSessionMenu(session, 'machine', false, false).cost.figure, '—');
  raw.family.runs.push(raw.family.runs[0]);
  assert.throws(() => parseSessionDetails(raw), /identity/);
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

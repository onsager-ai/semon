import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';

const { outputFiles } = await build({
  stdin: {
    contents: "export {parseSessionDetails} from './src/domain/sessionDetails';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseSessionDetails } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);

const relative = (last) => ({
  key: 'relative',
  name: 'Relative',
  harness: 'claude',
  kind: 'session',
  parent: 'selected',
  ...(last === undefined ? {} : { last }),
  cost: { usd: 1 },
});
function details(row, section = 'runs') {
  return {
    state: 'done',
    turns: 1,
    calls: 0,
    errors: 0,
    tool_calls: {},
    signals: {},
    cost: { usd: 1 },
    copied_usage: false,
    copilot: null,
    reported_runs: [],
    cost_check: [],
    family: { state: 'complete', ancestors: [], runs: [], [section]: [row] },
  };
}

test('selected relatives retain finite activity times and accept absent legacy metadata', () => {
  for (const section of ['ancestors', 'runs']) {
    assert.equal(
      parseSessionDetails(details(relative(1234), section)).family[section][0].last,
      1234,
    );
    assert.equal(parseSessionDetails(details(relative(0), section)).family[section][0].last, 0);
    for (const last of [undefined, null])
      assert.equal(
        parseSessionDetails(details(relative(last), section)).family[section][0].last,
        null,
      );
  }
});

test('selected relative activity times reject nonfinite and nonnumeric metadata', () => {
  for (const section of ['ancestors', 'runs'])
    for (const last of [NaN, Infinity, -Infinity, '1234', true, {}])
      assert.throws(() => parseSessionDetails(details(relative(last), section)), /Invalid number/);
});

test('selected role metadata preserves published booleans without inventing absent roles', () => {
  const raw = details(relative(1));
  assert.equal(parseSessionDetails(raw).role, null);
  assert.equal(parseSessionDetails({ ...raw, role: null }).role, null);
  for (const role of [true, false]) assert.equal(parseSessionDetails({ ...raw, role }).role, role);
  for (const role of ['true', 'role', 1, {}])
    assert.throws(() => parseSessionDetails({ ...raw, role }), /Invalid boolean/);
});

test('relative status preserves qualified observations and keeps unknown history explicit', () => {
  for (const section of ['ancestors', 'runs']) {
    const parse = (status) =>
      parseSessionDetails(
        details({ ...relative(1), ...(status === undefined ? {} : { status }) }, section),
      ).family[section][0].status;
    assert.deepEqual(parse(undefined), { state: null, observation: 'unavailable' });
    for (const state of ['work', 'wait', 'idle', 'done', 'err', 'new', 'read', 'off']) {
      assert.deepEqual(parse({ state, observation: 'current' }), { state, observation: 'current' });
      if (state !== 'work' && state !== 'wait')
        assert.deepEqual(parse({ state, observation: 'retained' }), {
          state,
          observation: 'retained',
        });
    }
    for (const status of [
      { state: 'work', observation: 'retained' },
      { state: 'wait', observation: 'retained' },
      { state: 'done', observation: 'unavailable' },
      { state: null, observation: 'current' },
      { state: 'invented', observation: 'current' },
      { state: 'done', observation: 'invented' },
    ])
      assert.throws(() => parse(status), /Invalid catalog status/);
  }
});

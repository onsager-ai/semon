import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';
import { GROUPS, TAP_SESSIONS, selectChecks, selectSchemes, tapSessions } from './suite-plan.mjs';

// Read the actual registry without importing browser modules or starting servers.
const source = fs.readFileSync(new URL('./checks/run.mjs', import.meta.url), 'utf8');
const registry = source.split('const checks = [')[1].split('\n];')[0];
const checks = [...registry.matchAll(/\[\s*(['"])(.*?)\1\s*,/g)].map(([, , name]) => [
  name,
  () => true,
]);

test('the CI groups partition the actual functional registry without losing checks', () => {
  assert.ok(checks.length > 30);
  assert.deepEqual(selectChecks(checks), checks);
  const selected = Object.keys(GROUPS).flatMap((group) => selectChecks(checks, ['--group', group]));
  assert.equal(new Set(selected.map(([name]) => name)).size, checks.length);
  assert.deepEqual(selected.map(([name]) => name).sort(), checks.map(([name]) => name).sort());
});

test('a new unassigned check and invalid selection fail rather than silently skipping coverage', () => {
  assert.throws(
    () => selectChecks([...checks, ['new-check', () => true]]),
    /every registered check/,
  );
  assert.throws(() => selectChecks(checks, ['--group', 'typo']), /Usage/);
  assert.throws(() => selectChecks(checks, ['--group']), /Usage/);
  assert.throws(() => selectChecks(checks, ['--group', 'shell', 'extra']), /Usage/);
});

test('the three order shards cover all four schemes exactly once', () => {
  const schemes = ['phone-light', 'phone-dark', 'desktop-light', 'desktop-dark'].map((name) => [
    name,
    {},
  ]);
  const shards = ['phone-light,phone-dark', 'desktop-light', 'desktop-dark'];
  assert.deepEqual(
    shards.flatMap((names) => selectSchemes(schemes, names)),
    schemes,
  );
  assert.throws(() => selectSchemes(schemes, 'phone-ligth'), /Unknown/);
  assert.throws(() => selectSchemes(schemes, 'phone-light,phone-light'), /duplicate/);
});

test('representative touch fixtures are mandatory; exhaustive mode retains every session', () => {
  for (const [fixture, ids] of Object.entries(TAP_SESSIONS)) {
    const sessions = Object.fromEntries([...ids, 'another-session'].map((id) => [id, {}]));
    assert.deepEqual(tapSessions(sessions, fixture), ids);
    assert.deepEqual(tapSessions(sessions, fixture, true), Object.keys(sessions));
    delete sessions[ids[0]];
    assert.throws(() => tapSessions(sessions, fixture), /Missing representative/);
    assert.throws(() => tapSessions(sessions, fixture, true), /Missing representative/);
  }
  assert.throws(() => tapSessions({}, 'unknown'), /Unknown/);
});

test('the workflow matrix and functional condition reach every declared group', () => {
  const workflow = fs.readFileSync(
    new URL('../../.github/workflows/ui.yml', import.meta.url),
    'utf8',
  );
  const matrix = workflow
    .match(/suite: \[([^\]]+)\]/)[1]
    .split(',')
    .map((s) => s.trim());
  const functional = JSON.parse(workflow.match(/contains\(fromJSON\('([^']+)'\)/)[1]);
  assert.deepEqual(functional.sort(), Object.keys(GROUPS).sort());
  assert.equal(new Set(matrix).size, matrix.length);
  assert.ok(functional.every((group) => matrix.includes(group)));
  assert.ok(
    [
      'visual',
      'live',
      'order-phone',
      'order-desktop-light',
      'order-desktop-dark',
      'performance',
      'transport',
    ].every((group) => matrix.includes(group)),
  );
});

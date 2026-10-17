import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {
  assessPreservation,
  EXPECTED,
  REFERENCE,
  THRESHOLD,
  MAX_RATIO,
} from './preservation-contract.mjs';

const report = () => ({
  reference: REFERENCE.source,
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  results: EXPECTED.map((key) => ({ key, pixels: 0, ratio: 0, size: null, pass: true })),
  errors: [],
});

test('complete application gate enforces all 62 states and pixel/geometry results', () => {
  assert.equal(EXPECTED.length, 62);
  assert.deepEqual(assessPreservation(report()), { coverage: [], visual: [] });
  for (const change of [{ pixels: 100, ratio: MAX_RATIO * 2 }, { size: ['390x844', '390x845'] }]) {
    const changed = report();
    Object.assign(changed.results[0], change);
    // Even a stale or forged pass flag cannot turn a mismatch into acceptance.
    const result = assessPreservation(changed);
    assert.deepEqual(result.coverage, []);
    assert.equal(result.visual.length, 1);
  }
});

test('equal row counts cannot hide missing, duplicate or unexpected captures', () => {
  const duplicate = report();
  duplicate.results[0] = duplicate.results[1];
  assert.match(assessPreservation(duplicate).coverage.join(), /Duplicate.*Missing/);
  const unknown = report();
  unknown.results[0].key = '390-light-unapproved-state';
  assert.match(assessPreservation(unknown).coverage.join(), /Unknown.*Missing/);
});

test('execution errors, invalid measurements, changed reference and relaxed thresholds fail', () => {
  for (const change of [
    (r) => r.errors.push('browser failed'),
    (r) => (r.reference = 'candidate'),
    (r) => (r.maxRatio = 0.1),
    (r) => (r.threshold = 1),
    (r) => (r.results[0].ratio = NaN),
    (r) => (r.results[0].pixels = -1),
    (r) => delete r.results[0].size,
    (r) => delete r.errors,
  ]) {
    const invalid = report();
    change(invalid);
    assert.ok(assessPreservation(invalid).coverage.length);
  }
});

test('required actual comparison reaches every visual workflow, independently of the recovery PR', () => {
  const workflow = fs.readFileSync(
    new URL('../../.github/workflows/ui.yml', import.meta.url),
    'utf8',
  );
  const step = (name) =>
    workflow.split('      - name: ' + name + '\n')[1]?.split('\n      - name:')[0];
  for (const name of [
    'Build the selected complete UI reference',
    'Retain selected UI reference binary',
    'Download selected UI reference',
    'Compare actual native and selected complete reading UI',
  ]) {
    const body = step(name);
    assert.ok(body, name);
    assert.doesNotMatch(body, /pull_request\.number|continue-on-error/);
  }
  const candidate = step('Compare actual native and selected complete reading UI');
  assert.match(candidate, /node preservation\.mjs\n/);
  assert.doesNotMatch(candidate, /--historical-before/);
  assert.match(
    step('Capture the before candidate under the same comparison protocol'),
    /node preservation\.mjs --historical-before/,
  );
});

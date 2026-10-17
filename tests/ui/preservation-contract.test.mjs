import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import {
  assessPreservation,
  EXPECTED,
  assessPixelComparison,
  assessVisualAcceptances,
  expectedPixelCoverage,
  PARITY_MODEL_SOURCE,
  REFERENCE,
  REFERENCE_MODEL_SOURCE,
  THRESHOLD,
  MAX_RATIO,
} from './preservation-contract.mjs';

const artifactDigests = () => ({
  candidate: { image: 'a'.repeat(64), evidence: 'b'.repeat(64) },
  reference: { image: 'c'.repeat(64), evidence: 'd'.repeat(64) },
  diff: { image: 'e'.repeat(64), evidence: 'f'.repeat(64) },
});
const report = () => ({
  candidate: 'a'.repeat(40),
  candidateFingerprint: 'b'.repeat(64),
  reference: REFERENCE.source,
  referenceHub: REFERENCE.hub,
  mode: 'required',
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  results: EXPECTED.map((key) => ({
    key,
    pixels: 0,
    ratio: 0,
    size: null,
    pass: true,
    artifactDigests: artifactDigests(),
  })),
  errors: [],
});

const pixelReport = (scope = 'main') => {
  const expected = expectedPixelCoverage(scope);
  return {
    candidate: 'a'.repeat(40),
    candidateFingerprint: 'b'.repeat(64),
    reference: scope === 'pushed' ? 'served-parity' : REFERENCE.source,
    ...(scope === 'pushed' ? {} : { referenceHub: REFERENCE.hub }),
    referenceModelSource: scope === 'pushed' ? PARITY_MODEL_SOURCE : REFERENCE_MODEL_SOURCE,
    referenceModelDigest: scope === 'pushed' ? null : 'c'.repeat(64),
    scope,
    coverageSource: 'tests/ui/visual-coverage.json',
    threshold: THRESHOLD,
    maxRatio: MAX_RATIO,
    errors: [],
    results: expected.frames.map((key) => ({
      key,
      pixels: 0,
      ratio: 0,
      size: null,
      artifactDigests: artifactDigests(),
    })),
    regions: expected.regions.map((key) => ({
      key,
      pixels: 0,
      ratio: 0,
      size: null,
      artifactDigests: artifactDigests(),
    })),
  };
};

function assertHistoricalCoverageMap(coverage, historical, historicalMap) {
  const legacyFrames = (section) =>
    [...new Set(Object.keys(section).map((key) => key.slice(key.indexOf('|') + 1)))].sort();
  assert.deepEqual(
    [...coverage.frames].sort(),
    legacyFrames(historical.main),
    'expanded frame names must match the retained main pixel ratchets',
  );
  assert.deepEqual(
    [...coverage.scopes['two-machines'].frames].sort(),
    legacyFrames(historical['two-machines']),
    'two-machine frame names must match retained pixel ratchets',
  );
  assert.equal(
    JSON.stringify(coverage.regions),
    JSON.stringify(historicalMap.regions),
    'expanded region definitions must preserve the historical region map exactly',
  );
  const activeMain = expectedPixelCoverage('main').frames;
  assert.ok(
    activeMain.every((key) => Object.hasOwn(historical.main, key)),
    'every active main frame key must remain in the historical ratchets',
  );
  assert.deepEqual(
    expectedPixelCoverage('two-machines').frames.slice().sort(),
    Object.keys(historical['two-machines']).sort(),
    'all active two-machine frame keys must map to the historical ratchets',
  );
  const legacyRegions = new Set(historicalMap.regions.map((region) => region.name));
  for (const scope of ['main', 'two-machines'])
    assert.ok(
      expectedPixelCoverage(scope).regions.every((key) => legacyRegions.has(key.split('|')[2])),
      `${scope} region keys must map to a retained historical region definition`,
    );
}

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

test('historical actual-frame contract retains 113 frames and 162 strict regions', () => {
  const main = expectedPixelCoverage('main');
  assert.equal(main.frames.length, 113);
  assert.equal(main.regions.length, 162);
  assert.deepEqual(assessPixelComparison(pixelReport()), { coverage: [], visual: [] });
  assert.equal(expectedPixelCoverage('two-machines').frames.length, 12);
  assert.equal(expectedPixelCoverage('two-machines').regions.length, 20);
  assert.equal(expectedPixelCoverage('pushed').frames.length, 15);
  assert.equal(expectedPixelCoverage('pushed').regions.length, 24);
});

test('retired mockup data remains available as inert historical provenance', () => {
  const ratchetBytes = fs.readFileSync(new URL('./historical-pixel-ratchet.json', import.meta.url)),
    mapBytes = fs.readFileSync(new URL('./historical-reference-map.json', import.meta.url));
  assert.equal(
    createHash('sha256').update(ratchetBytes).digest('hex'),
    '6fb837ae888ab4d80707f1e602157a010b83a0486aa34c597d60aba8414d0118',
  );
  assert.equal(
    createHash('sha256').update(mapBytes).digest('hex'),
    'a3f21bb842b69fe70766595a16bd712d630101e07336b6125d2d63323806b335',
  );
  const historical = JSON.parse(ratchetBytes.toString('utf8'));
  const historicalMap = JSON.parse(mapBytes.toString('utf8'));
  assert.deepEqual(historicalMap.references, {
    sample: 'reference/semon-sample.html',
    overhaul: 'reference/overhaul.html',
  });
  assert.equal(Object.keys(historical.main).length, 150);
  assert.equal(Object.keys(historical['two-machines']).length, 12);
  for (const row of [
    ...Object.values(historical.main),
    ...Object.values(historical['two-machines']),
  ]) {
    assert.ok(Number.isFinite(row.ratio) && row.ratio >= 0 && row.ratio <= 1);
    assert.ok(Array.isArray(row.size) && row.size.length === 2);
  }
});

test('expanded coverage maps exact retained frame keys and region definitions', () => {
  const historical = JSON.parse(
      fs.readFileSync(new URL('./historical-pixel-ratchet.json', import.meta.url), 'utf8'),
    ),
    historicalMap = JSON.parse(
      fs.readFileSync(new URL('./historical-reference-map.json', import.meta.url), 'utf8'),
    ),
    coverage = JSON.parse(
      fs.readFileSync(new URL('./visual-coverage.json', import.meta.url), 'utf8'),
    );
  assertHistoricalCoverageMap(coverage, historical, historicalMap);

  const renamedFrame = structuredClone(coverage);
  renamedFrame.frames[0] = 'renamed-without-changing-cardinality';
  assert.throws(
    () => assertHistoricalCoverageMap(renamedFrame, historical, historicalMap),
    /expanded frame names must match the retained main pixel ratchets/,
  );

  const renamedRegion = structuredClone(coverage);
  renamedRegion.regions[0].name = 'renamed-without-changing-region-count';
  assert.throws(
    () => assertHistoricalCoverageMap(renamedRegion, historical, historicalMap),
    /expanded region definitions must preserve the historical region map exactly/,
  );
});

test('actual-frame contract rejects gaps, wrong provenance, bad digests and raw differences', () => {
  const missing = pixelReport();
  missing.results.pop();
  assert.match(assessPixelComparison(missing).coverage.join(), /Missing frame case/);
  const wrongReference = pixelReport();
  wrongReference.reference = 'candidate';
  assert.match(
    assessPixelComparison(wrongReference).coverage.join(),
    /Wrong actual visual reference/,
  );
  const badDigest = pixelReport();
  delete badDigest.results[0].artifactDigests.diff.evidence;
  assert.match(assessPixelComparison(badDigest).coverage.join(), /artifact digests/);
  const wrongModelSource = pixelReport();
  wrongModelSource.referenceModelSource = 'untrusted fixture source';
  assert.match(
    assessPixelComparison(wrongModelSource).coverage.join(),
    /Wrong visual reference model source/,
  );
  const missingModelDigest = pixelReport();
  missingModelDigest.referenceModelDigest = null;
  assert.match(
    assessPixelComparison(missingModelDigest).coverage.join(),
    /Missing pinned reference model digest/,
  );
  const mismatch = pixelReport();
  mismatch.results[0].ratio = MAX_RATIO * 2;
  assert.equal(assessPixelComparison(mismatch).visual.length, 1);
});

test('visual acceptance requires an exact reviewed runtime/reference/state and artifact binding', () => {
  const changed = pixelReport();
  changed.results[0].ratio = MAX_RATIO * 2;
  const raw = assessPixelComparison(changed);
  const pending = assessVisualAcceptances(changed, 'pixels', raw, { version: 1, acceptances: [] });
  assert.equal(pending.pass, false);
  assert.equal(pending.pending.length, 1);
  assert.equal(raw.visual.length, 1);

  const row = changed.results[0];
  const entry = {
    comparator: 'pixels',
    scope: 'main',
    kind: 'frame',
    key: row.key,
    candidateFingerprint: changed.candidateFingerprint,
    reference: changed.reference,
    referenceHub: changed.referenceHub,
    artifactDigests: row.artifactDigests,
    decision: 'accepted',
    reviewedCandidate: changed.candidate,
    reviewedBy: 'human-reviewer',
    reviewedAt: '2026-10-17T11:00:00Z',
    rationale: 'Exact state and image artifacts reviewed.',
  };
  const accepted = assessVisualAcceptances(changed, 'pixels', raw, {
    version: 1,
    acceptances: [entry],
  });
  assert.equal(accepted.pass, true);
  assert.equal(accepted.accepted.length, 1);
  assert.equal(raw.visual.length, 1, 'raw strict findings remain unchanged');

  const stale = assessVisualAcceptances(changed, 'pixels', raw, {
    version: 1,
    acceptances: [{ ...entry, candidateFingerprint: 'c'.repeat(64) }],
  });
  assert.equal(stale.pass, false);
  assert.ok(stale.errors.some((error) => /stale/.test(error)));
});

test('valid approvals for another scope are retained without being treated as unused', () => {
  const unchanged = pixelReport();
  const otherScope = {
    comparator: 'pixels',
    scope: 'pushed',
    kind: 'frame',
    key: 'phone-light|home',
    candidateFingerprint: 'c'.repeat(64),
    reference: 'served-parity',
    referenceHub: null,
    artifactDigests: artifactDigests(),
    decision: 'accepted',
    reviewedCandidate: 'd'.repeat(40),
    reviewedBy: 'human-reviewer',
    reviewedAt: '2026-10-17T11:00:00Z',
    rationale: 'A separate pushed-data state was reviewed.',
  };
  const result = assessVisualAcceptances(unchanged, 'pixels', assessPixelComparison(unchanged), {
    version: 1,
    acceptances: [otherScope],
  });
  assert.equal(result.pass, true);
  assert.deepEqual(result.errors, []);

  const duplicate = assessVisualAcceptances(unchanged, 'pixels', assessPixelComparison(unchanged), {
    version: 1,
    acceptances: [otherScope, otherScope],
  });
  assert.equal(duplicate.pass, false);
  assert.ok(duplicate.errors.some((error) => /Duplicate visual acceptance binding/.test(error)));
});

test('execution errors, invalid measurements, changed reference and relaxed thresholds fail', () => {
  for (const change of [
    (r) => r.errors.push('browser failed'),
    (r) => (r.reference = 'candidate'),
    (r) => (r.referenceHub = 'unselected-hub'),
    (r) => (r.candidate = 'missing-sha'),
    (r) => (r.candidateFingerprint = 'missing-fingerprint'),
    (r) => (r.mode = 'unknown'),
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
  assert.match(candidate, /node preservation\.mjs \|\| failed=1\n/);
  assert.match(candidate, /SEMON_PIXEL_SCOPE=main node pixels\.mjs/);
  assert.doesNotMatch(candidate, /--historical-before/);
  assert.match(
    step('Download selected UI reference'),
    /matrix\.suite == 'visual' \|\| matrix\.suite == 'transport'/,
  );
  assert.match(
    step('Two machines in one viewer against the selected complete UI'),
    /SEMON_PIXEL_SCOPE=two-machines node pixels\.mjs/,
  );
  assert.match(step('Pushed data in the viewer'), /SEMON_PIXEL_SCOPE: pushed/);
  assert.match(
    step('Capture the before candidate under the same comparison protocol'),
    /node preservation\.mjs --historical-before/,
  );
});

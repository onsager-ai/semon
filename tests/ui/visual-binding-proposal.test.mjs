import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { proposeVisualBindings } from './visual-binding-proposal.mjs';

const hash = (value) => createHash('sha256').update(value).digest('hex');
const fingerprint = 'a'.repeat(64);
const previousFingerprint = 'b'.repeat(64);
const sourcePin = 'source-review-pin';
const hubPin = 'hub-review-pin';
const sourceSha = 'c'.repeat(40);

function fixture({
  coverageGap = false,
  errors = false,
  duplicate = false,
  missing = false,
  changedDigest = null,
  auditRefusal = false,
  suite = 'visual',
} = {}) {
  let specs = [
    {
      source: 'preservation/report.json',
      comparator: 'preservation',
      scope: 'required',
      frames: 68,
      regions: 0,
    },
    {
      source: 'pixels/report.json',
      comparator: 'pixels',
      scope: 'main',
      frames: 113,
      regions: 162,
    },
  ];
  if (suite === 'transport')
    specs = [
      {
        source: 'two-machines/pixels/report.json',
        comparator: 'pixels',
        scope: 'two-machines',
        frames: 12,
        regions: 20,
      },
      {
        source: 'pushed/pixels/report.json',
        comparator: 'pixels',
        scope: 'pushed',
        frames: 15,
        regions: 24,
      },
    ];
  const reports = {};
  const acceptances = [];
  const qualified = [];
  const auditReports = [];
  for (const [reportIndex, spec] of specs.entries()) {
    const acceptedDigests = {
      candidate: { image: hash('candidate image'), evidence: hash(`old evidence ${reportIndex}`) },
      reference: { image: hash('reference image'), evidence: hash('reference evidence') },
      diff: { image: hash('diff image'), evidence: hash('diff evidence') },
    };
    const currentDigests = structuredClone(acceptedDigests);
    if (reportIndex === 0) currentDigests.candidate.evidence = hash('canonical pathname evidence');
    if (changedDigest === 'candidate.image' && reportIndex === 0)
      currentDigests.candidate.image = hash('changed image');
    if (changedDigest === 'reference.evidence' && reportIndex === 0)
      currentDigests.reference.evidence = hash('changed reference evidence');

    const key = `${spec.scope}-failed-frame`;
    const report = {
      scope: spec.scope,
      reference: sourcePin,
      referenceHub: hubPin,
      candidateFingerprint: fingerprint,
      errors: errors && reportIndex === 0 ? ['render error'] : [],
      failures: { coverage: coverageGap && reportIndex === 1 ? ['missing state'] : [] },
      results: Array.from({ length: spec.frames }, (_, index) => ({
        key: index === 0 ? key : `${spec.scope}-frame-${index}`,
        pass: index !== 0,
        ...(index === 0 ? { artifactDigests: currentDigests } : {}),
      })),
      regions: Array.from({ length: spec.regions }, (_, index) => ({
        key: `${spec.scope}-region-${index}`,
        pass: true,
      })),
    };
    if (coverageGap && reportIndex === 1) report.regions.pop();
    reports[spec.source] = report;
    auditReports.push({
      source: spec.source,
      scope: spec.scope,
      errorCount: 0,
      candidateFingerprint: fingerprint,
      reportSha256: hash(JSON.stringify(report)),
    });

    const accepted = {
      comparator: spec.comparator,
      scope: spec.scope,
      kind: 'frame',
      key,
      reference: sourcePin,
      referenceHub: hubPin,
      candidateFingerprint: previousFingerprint,
      artifactDigests: acceptedDigests,
      decision: 'accepted',
      imageReview: { reviewer: 'existing review record' },
      provenance: ['kept byte-for-byte'],
      reviewedCandidate: 'existing reviewed head',
    };
    if (!(missing && reportIndex === 0)) acceptances.push(accepted);
    if (duplicate && reportIndex === 0) acceptances.push(structuredClone(accepted));
    if (!(missing && reportIndex === 0)) {
      qualified.push({
        comparator: spec.comparator,
        scope: spec.scope,
        kind: 'frame',
        key,
        reason: reportIndex === 0 ? 'canonical-pathname-only' : 'fingerprint-only',
        candidateFingerprint: fingerprint,
        artifactDigests: currentDigests,
        acceptedCandidateEvidenceSha256: acceptedDigests.candidate.evidence,
      });
    }
  }
  const audit = {
    diagnosticOnly: true,
    reports: auditReports,
    qualified,
    refused: auditRefusal
      ? [
          {
            comparator: 'pixels',
            scope: 'main',
            kind: 'frame',
            key: 'main-failed-frame',
            reason: 'unmatched',
          },
        ]
      : [],
  };
  const registry = { version: 1, acceptances };
  return {
    reports,
    audit,
    registry,
    registryText: JSON.stringify(registry, null, 2) + '\n',
  };
}

function propose(options = {}) {
  return proposeVisualBindings({ suite: 'visual', ...fixture(options) });
}

test('proposes only candidate fingerprint/evidence changes and preserves review history', () => {
  const input = fixture();
  const original = structuredClone(input.registry);
  const result = proposeVisualBindings({ suite: 'visual', ...input });
  assert.equal(result.status, 'proposed');
  assert.match(
    result.patch,
    /^diff --git a\/tests\/ui\/visual-acceptance\.json b\/tests\/ui\/visual-acceptance\.json/m,
  );
  assert.deepEqual(input.registry, original);
  for (let index = 0; index < input.registry.acceptances.length; index++) {
    const before = input.registry.acceptances[index];
    const after = result.proposedRegistry.acceptances[index];
    assert.deepEqual(
      {
        ...after,
        candidateFingerprint: before.candidateFingerprint,
        artifactDigests: {
          ...after.artifactDigests,
          candidate: {
            ...after.artifactDigests.candidate,
            evidence: before.artifactDigests.candidate.evidence,
          },
        },
      },
      before,
    );
    assert.equal(after.decision, before.decision);
    assert.deepEqual(after.imageReview, before.imageReview);
    assert.deepEqual(after.provenance, before.provenance);
    assert.equal(after.reviewedCandidate, before.reviewedCandidate);
  }
});

test('refuses incomplete coverage and report errors', () => {
  assert.equal(propose({ coverageGap: true }).status, 'refused');
  assert.equal(propose({ errors: true }).status, 'refused');
});

test('refuses duplicate or missing accepted records', () => {
  assert.equal(propose({ duplicate: true }).status, 'refused');
  assert.equal(propose({ missing: true }).status, 'refused');
});

test('refuses changed image, other evidence digests, or reference pins', () => {
  assert.equal(propose({ changedDigest: 'candidate.image' }).status, 'refused');
  assert.equal(propose({ changedDigest: 'reference.evidence' }).status, 'refused');
  const input = fixture();
  input.reports['preservation/report.json'].reference = 'changed-source-pin';
  assert.equal(proposeVisualBindings({ suite: 'visual', ...input }).status, 'refused');
});

test('refuses any state the pathname audit refused', () => {
  assert.equal(propose({ auditRefusal: true }).status, 'refused');
});

function assertRefused(input, reason) {
  const result = proposeVisualBindings({ suite: 'visual', ...input });
  assert.equal(result.status, 'refused');
  assert.equal(result.patch, '');
  assert.deepEqual(result.updates, []);
  if (reason) assert.match(result.refusals.join('\n'), reason);
}

test('proposal patch is deterministic and preserves canonical registry text', () => {
  const input = fixture();
  const result = proposeVisualBindings({ suite: 'visual', ...input });
  assert.equal(result.patch, proposeVisualBindings({ suite: 'visual', ...input }).patch);
  assert.equal(JSON.stringify(input.registry, null, 2) + '\n', input.registryText);
  assertRefused({ ...input, registryText: JSON.stringify(input.registry) }, /canonical JSON/);
});

test('refuses nested coverage failures even with complete counts and a null console summary', () => {
  const input = fixture();
  input.reports['pixels/report.json'].coverageFailures = null;
  input.reports['pixels/report.json'].failures.coverage = ['missing required state'];
  assertRefused(input, /missing or failed coverage/);
  delete input.reports['pixels/report.json'].failures;
  assertRefused(input, /missing or failed coverage/);
});

test('refuses missing execution status and duplicate current states', () => {
  const input = fixture();
  delete input.reports['pixels/report.json'].errors;
  assertRefused(input, /execution errors/);
  const duplicate = fixture();
  const report = duplicate.reports['pixels/report.json'];
  report.results[2] = structuredClone(report.results[1]);
  assertRefused(duplicate, /duplicate visual state/);
});

test('enforces all visual and transport frame and region counts', () => {
  for (const suite of ['visual', 'transport']) {
    const input = fixture({ suite });
    assert.equal(proposeVisualBindings({ suite, ...input }).status, 'proposed');
    for (const source of Object.keys(input.reports)) {
      for (const field of ['results', 'regions']) {
        const incomplete = fixture({ suite });
        const rows = incomplete.reports[source][field];
        if (rows.length) rows.pop();
        else rows.push({ key: 'unexpected region', pass: true });
        const result = proposeVisualBindings({ suite, ...incomplete });
        assert.equal(result.status, 'refused', `${suite}/${source}/${field}`);
        assert.equal(result.patch, '');
      }
    }
  }
});

test('refuses each changed non-candidate-evidence binding', () => {
  for (const [side, type] of [
    ['candidate', 'image'],
    ['reference', 'image'],
    ['reference', 'evidence'],
    ['diff', 'image'],
    ['diff', 'evidence'],
  ]) {
    const input = fixture();
    input.reports['preservation/report.json'].results[0].artifactDigests[side][type] = hash(
      `changed ${side}.${type}`,
    );
    assertRefused(input, new RegExp(`${side}\\.${type}`));
  }
});

test('refuses missing decisions, duplicate qualifications and extra audit states', () => {
  for (const decision of [undefined, 'pending', 'rejected']) {
    const input = fixture();
    input.registry.acceptances[0].decision = decision;
    assertRefused(input, /no accepted decision/);
  }
  const duplicate = fixture();
  duplicate.audit.qualified.push(structuredClone(duplicate.audit.qualified[0]));
  assertRefused(duplicate, /uniquely qualify/);
  const extra = fixture();
  extra.audit.qualified.push({ ...extra.audit.qualified[0], scope: 'outside-suite' });
  assertRefused(extra, /exactly match report failures/);
});

test('refuses inconsistent evidence qualification and runtime fingerprints', () => {
  const evidence = fixture();
  evidence.audit.qualified[0].acceptedCandidateEvidenceSha256 = hash('wrong evidence');
  assertRefused(evidence, /audit hashes do not match/);
  const reason = fixture();
  reason.audit.qualified[0].reason = 'fingerprint-only';
  assertRefused(reason, /audit hashes do not match/);
  const fingerprints = fixture();
  fingerprints.reports['pixels/report.json'].candidateFingerprint = hash('other runtime');
  assertRefused(fingerprints, /one valid runtime fingerprint/);
});

test('refuses missing or duplicate audit reports and mismatched summary fingerprints', () => {
  const missing = fixture();
  missing.audit.reports.pop();
  assertRefused(missing, /report set/);
  const duplicate = fixture();
  duplicate.audit.reports[1] = structuredClone(duplicate.audit.reports[0]);
  assertRefused(duplicate, /audit summary/);
  const fingerprint = fixture();
  fingerprint.audit.reports[0].candidateFingerprint = hash('other runtime');
  assertRefused(fingerprint, /audit summary/);
});

test('proposal CLI exits nonzero and removes stale patches on report or audit refusal', (t) => {
  for (const failure of ['coverage', 'errors', 'refused', 'digest']) {
    const input = fixture();
    const report = input.reports['pixels/report.json'];
    if (failure === 'coverage') {
      report.failures.coverage = ['missing required state'];
      report.coverageFailures = null;
    }
    if (failure === 'errors') report.errors = ['render error'];
    if (failure === 'refused') input.audit.refused.push(input.audit.qualified[0]);
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'visual-proposal-cli-'));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    for (const summary of input.audit.reports) {
      const bytes = JSON.stringify(input.reports[summary.source]);
      summary.reportSha256 = hash(bytes);
      fs.mkdirSync(path.dirname(path.join(root, summary.source)), { recursive: true });
      fs.writeFileSync(path.join(root, summary.source), bytes);
    }
    if (failure === 'digest') input.audit.reports[0].reportSha256 = hash('stale report');
    fs.writeFileSync(path.join(root, 'visual-pathname-audit.json'), JSON.stringify(input.audit));
    const patch = path.join(root, 'visual-acceptance-visual-review.patch');
    fs.writeFileSync(patch, 'stale patch');
    const cli = spawnSync(
      process.execPath,
      [fileURLToPath(new URL('./visual-binding-proposal.mjs', import.meta.url))],
      {
        encoding: 'utf8',
        env: {
          ...process.env,
          SEMON_UI_OUT: root,
          SEMON_VISUAL_PROPOSAL_SUITE: 'visual',
          GITHUB_SHA: sourceSha,
        },
      },
    );
    assert.equal(cli.status, 1, cli.stderr);
    assert.equal(fs.existsSync(patch), false);
    const summary = JSON.parse(
      fs.readFileSync(path.join(root, 'visual-acceptance-visual-proposal.json'), 'utf8'),
    );
    assert.equal(summary.status, 'refused');
    const expected = {
      coverage: /missing or failed coverage/,
      errors: /execution errors/,
      refused: /uniquely qualify|exactly match report failures/,
      digest: /digest does not match current bytes/,
    };
    assert.match(summary.refusals.join('\n'), expected[failure]);
  }
});

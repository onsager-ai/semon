import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { proposeVisualBindings } from './visual-binding-proposal.mjs';

const hash = (value) => createHash('sha256').update(value).digest('hex');
const fingerprint = 'a'.repeat(64);
const previousFingerprint = 'b'.repeat(64);
const sourcePin = 'source-review-pin';
const hubPin = 'hub-review-pin';

function fixture({
  coverageGap = false,
  errors = false,
  duplicate = false,
  missing = false,
  changedDigest = null,
  auditRefusal = false,
} = {}) {
  const specs = [
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
    reports[
      spec.source === 'pixels/report.json' ? 'pixels/report.json' : 'preservation/report.json'
    ] = report;
    auditReports.push({ source: spec.source, scope: spec.scope, errorCount: 0 });

    const accepted = {
      comparator: spec.comparator,
      scope: spec.scope,
      kind: 'frame',
      key,
      reference: sourcePin,
      referenceHub: hubPin,
      candidateFingerprint: previousFingerprint,
      artifactDigests: acceptedDigests,
      decision: 'previously accepted',
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

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { audit, qualifyCandidateEvidence } from './visual-pathname-audit.mjs';

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const previousBytes = Buffer.from(
  JSON.stringify({ pathname: '/s/claude/harbor', selectors: { text: 'same', box: [0, 1] } }),
);
const canonicalBytes = Buffer.from(
  JSON.stringify({ pathname: '/session/local/harbor', selectors: { text: 'same', box: [0, 1] } }),
);

function fixture(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'visual-audit-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const root = path.join(dir, 'current');
  fs.mkdirSync(path.join(root, 'pixels'), { recursive: true });
  const state = {
    comparator: 'pixels',
    scope: 'main',
    kind: 'frame',
    key: 'phone-light|session-harbor',
  };
  const digests = {
    candidate: { image: hash(Buffer.from('candidate image')), evidence: hash(previousBytes) },
    reference: {
      image: hash(Buffer.from('reference image')),
      evidence: hash(Buffer.from('reference evidence')),
    },
    diff: { image: hash(Buffer.from('diff image')), evidence: hash(Buffer.from('diff evidence')) },
  };
  const accepted = {
    ...state,
    reference: 'source-pin',
    referenceHub: 'hub-pin',
    artifactDigests: digests,
    reviewedCandidate: 'reviewed-head',
  };
  const baseReport = {
    scope: 'main',
    reference: 'source-pin',
    referenceHub: 'hub-pin',
    results: [
      {
        key: state.key,
        pass: false,
        artifactDigests: structuredClone(digests),
        artifactPaths: { candidateEvidence: 'pixels/candidate.json' },
      },
    ],
  };
  const run = ({ bytes = canonicalBytes, report = baseReport, acceptances = [accepted] } = {}) => {
    const currentReport = structuredClone(report);
    currentReport.results[0].artifactDigests.candidate.evidence = hash(bytes);
    fs.writeFileSync(path.join(root, 'pixels/candidate.json'), bytes);
    fs.writeFileSync(path.join(root, 'pixels/report.json'), JSON.stringify(currentReport));
    return audit({ root, registry: { acceptances } });
  };
  return { run, accepted, baseReport };
}

test('accepts only the exact pathname restoration or an identical accepted evidence hash', (t) => {
  const pathname = qualifyCandidateEvidence(canonicalBytes, hash(previousBytes));
  assert.equal(pathname.reason, 'canonical-pathname-only');
  assert.deepEqual(pathname.historicalFixture, {
    namespace: 'claude',
    digest: hash(previousBytes),
  });
  assert.deepEqual(qualifyCandidateEvidence(previousBytes, hash(previousBytes)), {
    reason: 'fingerprint-only',
    historicalFixture: null,
  });

  const { run } = fixture(t);
  const result = run();
  assert.equal(result.qualified[0].reason, 'canonical-pathname-only');
  assert.equal(result.refused.length, 0);
  const fingerprint = run({ bytes: previousBytes });
  assert.equal(fingerprint.qualified[0].reason, 'fingerprint-only');
});

test('refuses wrong namespace, wrong identity, extra evidence changes and changed escaping', (t) => {
  const { run } = fixture(t);
  const wrongNamespace = Buffer.from(
    canonicalBytes.toString('utf8').replace('/session/local/', '/session/remote/'),
  );
  const wrongId = Buffer.from(canonicalBytes.toString('utf8').replace('/harbor', '/other'));
  const extraChange = Buffer.from(canonicalBytes.toString('utf8').replace('"same"', '"changed"'));
  const changedEscaping = Buffer.from(
    JSON.stringify({
      pathname: '/session/local/harbor%2fchild',
      selectors: { text: 'same', box: [0, 1] },
    }),
  );
  for (const bytes of [wrongNamespace, wrongId, extraChange, changedEscaping])
    assert.equal(run({ bytes }).refused.length, 1);
});

test('refuses duplicate or missing pathname properties and a missing accepted record', (t) => {
  const { run } = fixture(t);
  const duplicate = Buffer.from(
    '{"pathname":"/session/local/harbor","pathname":"/session/local/harbor","selectors":{"text":"same","box":[0,1]}}',
  );
  const missing = Buffer.from(JSON.stringify({ selectors: { text: 'same', box: [0, 1] } }));
  assert.match(run({ bytes: duplicate }).refused[0].reason, /exactly one/);
  assert.match(run({ bytes: missing }).refused[0].reason, /exactly one/);
  assert.match(run({ acceptances: [] }).refused[0].reason, /one accepted state/);
});

test('refuses changed PNG digests and reference pins', (t) => {
  const { run, baseReport } = fixture(t);
  const changedPng = structuredClone(baseReport);
  changedPng.results[0].artifactDigests.candidate.image = hash(Buffer.from('changed image'));
  assert.match(run({ report: changedPng }).refused[0].reason, /candidate.image/);
  const changedReference = structuredClone(baseReport);
  changedReference.reference = 'different-source-pin';
  assert.match(run({ report: changedReference }).refused[0].reason, /Reference pins changed/);
});

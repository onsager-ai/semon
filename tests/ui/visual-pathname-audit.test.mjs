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

function fixture(t, { source = 'pixels/report.json', scope = 'main' } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'visual-audit-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const root = path.join(dir, 'current');
  const reportPath = path.join(root, source);
  const captureRoot = path.dirname(path.dirname(reportPath));
  fs.mkdirSync(path.dirname(reportPath), { recursive: true });
  fs.mkdirSync(path.join(captureRoot, 'pixels'), { recursive: true });
  const state = {
    comparator: 'pixels',
    scope,
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
    scope,
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
  const run = ({
    bytes = canonicalBytes,
    report = baseReport,
    acceptances = [accepted],
    candidatePath = 'pixels/candidate.json',
    writeEvidence = true,
    recordedDigest = hash(bytes),
  } = {}) => {
    const currentReport = structuredClone(report);
    currentReport.results[0].artifactPaths.candidateEvidence = candidatePath;
    currentReport.results[0].artifactDigests.candidate.evidence = recordedDigest;
    if (writeEvidence)
      fs.writeFileSync(path.join(captureRoot, candidatePath), bytes);
    fs.writeFileSync(reportPath, JSON.stringify(currentReport));
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

test('resolves nested transport evidence from each report capture root', (t) => {
  for (const [source, scope] of [
    ['two-machines/pixels/report.json', 'two-machines'],
    ['pushed/pixels/report.json', 'pushed'],
  ]) {
    const { run } = fixture(t, { source, scope });
    const result = run();
    assert.equal(result.qualified[0].reason, 'canonical-pathname-only');
    assert.equal(result.refused.length, 0);
  }
});

test('refuses nested transport evidence that escapes, is missing or has the wrong digest', (t) => {
  const { run } = fixture(t, {
    source: 'two-machines/pixels/report.json',
    scope: 'two-machines',
  });
  assert.match(
    run({ candidatePath: '../outside.json', writeEvidence: false }).refused[0].reason,
    /escapes root/,
  );
  assert.match(
    run({ candidatePath: 'pixels/missing.json', writeEvidence: false }).refused[0].reason,
    /ENOENT/,
  );
  assert.match(
    run({ bytes: Buffer.from('wrong evidence'), recordedDigest: hash(canonicalBytes) })
      .refused[0].reason,
    /bytes do not match recorded digest/,
  );
});

test('restores only canonical trace paths using the historical fixture encoding', () => {
  const turn = 'aa0043ac7697ce969';
  const current = (pathname, text = 'same') =>
    Buffer.from(JSON.stringify({ pathname, selectors: { text, box: [0, 1] } }));
  const approved = current(`/trace/claude/atlas/${turn}`);
  const canonical = current(`/session/local/atlas/trace/${turn}`);
  const approvedDigest = hash(approved);

  assert.deepEqual(qualifyCandidateEvidence(canonical, approvedDigest), {
    reason: 'canonical-pathname-only',
    historicalFixture: { namespace: 'claude', digest: approvedDigest },
  });
  assert.throws(
    () =>
      qualifyCandidateEvidence(
        current(`/session/local/atlas/trace/${turn}`, 'changed'),
        approvedDigest,
      ),
    /No unique accepted historical fixture identity hash/,
  );
});

test('refuses trace paths with wrong identity, namespace, extra bytes, duplicate or weak encoding', () => {
  const turn = 'aa0043ac7697ce969';
  const current = (pathname, text = 'same') =>
    Buffer.from(JSON.stringify({ pathname, selectors: { text, box: [0, 1] } }));
  const approvedDigest = hash(current(`/trace/claude/atlas/${turn}`));
  const refused = [
    current(`/session/local/atlas/trace/${turn}0`),
    current(`/session/local/other/trace/${turn}`),
    current(`/session/claude/atlas/trace/${turn}`),
    current(`/session/local/atlas/trace/${turn}`, 'changed'),
    Buffer.from(
      `{"pathname":"/session/local/atlas/trace/${turn}","pathname":"/session/local/atlas/trace/${turn}","selectors":{"text":"same","box":[0,1]}}`,
    ),
    Buffer.from(JSON.stringify({ selectors: { text: 'same', box: [0, 1] } })),
    current(`/session/local/atlas%2fchild/trace/${turn}`),
  ];
  for (const bytes of refused) assert.throws(() => qualifyCandidateEvidence(bytes, approvedDigest));

  const unsupportedNamespaceDigest = hash(current(`/trace/native/atlas/${turn}`));
  assert.throws(() =>
    qualifyCandidateEvidence(
      current(`/session/local/atlas/trace/${turn}`),
      unsupportedNamespaceDigest,
    ),
  );
});

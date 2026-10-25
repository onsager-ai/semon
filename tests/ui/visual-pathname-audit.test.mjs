import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import {
  audit,
  createBindingProposal,
  qualifyCandidateEvidence,
} from './visual-pathname-audit.mjs';

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const fingerprint = 'a'.repeat(64);
const previousFingerprint = 'b'.repeat(64);
const sourceSha = 'c'.repeat(40);
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
    candidateFingerprint: previousFingerprint,
    artifactDigests: digests,
    reviewedCandidate: 'reviewed-head',
  };
  const baseReport = {
    scope,
    candidateFingerprint: fingerprint,
    reference: 'source-pin',
    referenceHub: 'hub-pin',
    errors: [],
    coverageFailures: null,
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
    if (writeEvidence) fs.writeFileSync(path.join(captureRoot, candidatePath), bytes);
    fs.writeFileSync(reportPath, JSON.stringify(currentReport));
    return audit({ root, registry: { acceptances } });
  };
  return { run, accepted, baseReport, root };
}

function proposalFixture(t, options = {}) {
  const current = fixture(t, options);
  const result = current.run();
  const registry = { version: 1, acceptances: [structuredClone(current.accepted)] };
  const registryText = JSON.stringify(registry, null, 2) + '\n';
  return { root: current.root, result, registry, registryText };
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

test('refuses a duplicated current row and duplicate accepted registry record', (t) => {
  const duplicateCurrent = fixture(t);
  const report = structuredClone(duplicateCurrent.baseReport);
  report.results.push(structuredClone(report.results[0]));
  assert.match(duplicateCurrent.run({ report }).refused[0].reason, /duplicated/);

  const duplicateRegistry = fixture(t);
  assert.match(
    duplicateRegistry.run({
      acceptances: [duplicateRegistry.accepted, structuredClone(duplicateRegistry.accepted)],
    }).refused[0].reason,
    /one accepted state/,
  );
});

test('refuses changed reference and diff evidence digests independently of PNG digests', (t) => {
  const { run, baseReport } = fixture(t);
  for (const [side, type] of [
    ['reference', 'evidence'],
    ['diff', 'evidence'],
  ]) {
    const changed = structuredClone(baseReport);
    changed.results[0].artifactDigests[side][type] = hash(Buffer.from(`changed ${side} evidence`));
    assert.match(run({ report: changed }).refused[0].reason, new RegExp(`${side}\\.evidence`));
  }
});

test('proposal patch changes only the two candidate binding fields and is deterministic', (t) => {
  const input = proposalFixture(t);
  const proposal = createBindingProposal({ ...input, sourceSha });
  const repeated = createBindingProposal({ ...input, sourceSha });
  assert.equal(proposal.status, 'proposed');
  assert.equal(proposal.patch, repeated.patch);
  assert.match(proposal.patch, new RegExp(`# source_sha: ${sourceSha}`));
  assert.match(proposal.patch, new RegExp(`# runtime_fingerprint: ${fingerprint}`));
  assert.match(proposal.patch, /# report_sha256: \{"pixels\/report\.json":"[a-f0-9]{64}"\}/);
  assert.match(proposal.patch, /# qualified_count: 1\n# refused_count: 0/);

  const before = input.registryText.split('\n');
  const after = proposal.proposedRegistryText.split('\n');
  const changed = before.flatMap((line, index) =>
    line === after[index] ? [] : [line, after[index]],
  );
  assert.equal(changed.length, 4);
  assert.match(changed[0], /"candidateFingerprint"/);
  assert.match(changed[0], new RegExp(previousFingerprint));
  assert.match(changed[1], new RegExp(fingerprint));
  assert.match(changed[2], /"evidence"/);
  assert.match(changed[2], new RegExp(hash(previousBytes)));
  assert.match(changed[3], new RegExp(hash(canonicalBytes)));
  assert.equal(JSON.stringify(input.registry, null, 2) + '\n', input.registryText);
  assert.equal(proposal.proposedRegistryText.endsWith('\n'), true);
});

test('proposal refuses audit rows, report errors and non-null coverage failures without a patch', (t) => {
  const refusedCurrent = fixture(t);
  const duplicate = structuredClone(refusedCurrent.baseReport);
  duplicate.results.push(structuredClone(duplicate.results[0]));
  const refusedAudit = refusedCurrent.run({ report: duplicate });
  const refusedRegistry = {
    version: 1,
    acceptances: [structuredClone(refusedCurrent.accepted)],
  };
  const refused = createBindingProposal({
    root: refusedCurrent.root,
    registry: refusedRegistry,
    registryText: JSON.stringify(refusedRegistry, null, 2) + '\n',
    result: refusedAudit,
    sourceSha,
  });
  assert.equal(refused.status, 'refused');
  assert.equal(refused.patch, '');

  for (const field of ['errors', 'coverageFailures']) {
    const current = fixture(t);
    const report = structuredClone(current.baseReport);
    if (field === 'errors') report.errors = ['render failed'];
    else report.coverageFailures = 1;
    const result = current.run({ report });
    const registry = { version: 1, acceptances: [structuredClone(current.accepted)] };
    const proposal = createBindingProposal({
      root: current.root,
      registry,
      registryText: JSON.stringify(registry, null, 2) + '\n',
      result,
      sourceSha,
    });
    assert.equal(proposal.status, 'refused');
    assert.equal(proposal.patch, '');
  }
});

test('proposal CLI exits nonzero and writes no patch when rows or report gates fail', (t) => {
  const reports = [
    {
      scope: 'main',
      candidateFingerprint: fingerprint,
      reference: 'source-pin',
      referenceHub: 'hub-pin',
      errors: [],
      coverageFailures: null,
      results: [{ key: 'no-accepted-record', pass: false }],
    },
    {
      scope: 'main',
      candidateFingerprint: fingerprint,
      errors: ['render error'],
      coverageFailures: null,
      results: [],
    },
    {
      scope: 'main',
      candidateFingerprint: fingerprint,
      errors: [],
      coverageFailures: 1,
      results: [],
    },
  ];
  for (const [index, report] of reports.entries()) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'visual-audit-cli-'));
    t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
    const root = path.join(dir, 'current');
    fs.mkdirSync(path.join(root, 'pixels'), { recursive: true });
    fs.writeFileSync(path.join(root, 'pixels/report.json'), JSON.stringify(report));
    const proposalPath = path.join(dir, `proposal-${index}.patch`);
    const cli = spawnSync(
      process.execPath,
      [fileURLToPath(new URL('./visual-pathname-audit.mjs', import.meta.url)), '--propose'],
      {
        encoding: 'utf8',
        env: {
          ...process.env,
          SEMON_UI_OUT: root,
          GITHUB_SHA: sourceSha,
          SEMON_VISUAL_PROPOSAL_PATH: proposalPath,
        },
      },
    );
    assert.equal(cli.status, 1, cli.stderr);
    assert.equal(fs.existsSync(proposalPath), false);
  }
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
    run({ bytes: Buffer.from('wrong evidence'), recordedDigest: hash(canonicalBytes) }).refused[0]
      .reason,
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

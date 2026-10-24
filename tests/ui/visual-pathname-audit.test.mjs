import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { audit, pathnameOnly } from './visual-pathname-audit.mjs';

test('only the local canonical pathname may change; every other byte stays exact', () => {
  const old = JSON.stringify({
    pathname: '/s/claude/harbor',
    selectors: { text: 'same', box: [0, 1] },
  });
  const next = old.replace('/s/claude/harbor', '/session/local/harbor');
  assert.equal(pathnameOnly(old, next), true);
  assert.equal(pathnameOnly(old, next.replace('same', 'changed')), false);
  assert.equal(pathnameOnly(old, next.replace('[0,1]', '[0,2]')), false);
  assert.equal(pathnameOnly(old, next.replace('/local/', '/remote/')), false);
  assert.equal(pathnameOnly(old, next.replace('harbor', 'other')), false);
  assert.equal(pathnameOnly(old, next + '\n'), false);
  assert.equal(pathnameOnly(old.replace('/s/claude/', '/s/unknown/'), next), false);
  assert.equal(pathnameOnly(old.replace('claude', 'codex'), next), true);
});

test('audit refuses changed images, evidence, pins and unmatched states', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'visual-audit-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const root = path.join(dir, 'current'),
    previousRoot = path.join(dir, 'previous');
  for (const base of [root, previousRoot])
    fs.mkdirSync(path.join(base, 'pixels'), { recursive: true });
  const old = JSON.stringify({ pathname: '/s/claude/harbor', selectors: { text: 'same' } });
  const next = old.replace('/s/claude/harbor', '/session/local/harbor');
  const hash = (text) => createHash('sha256').update(text).digest('hex');
  const state = {
    comparator: 'pixels',
    scope: 'main',
    kind: 'frame',
    key: 'phone-light|session-harbor',
  };
  const digests = {
    candidate: { image: hash('candidate'), evidence: hash(old) },
    reference: { image: hash('reference'), evidence: hash('reference evidence') },
    diff: { image: hash('diff'), evidence: hash('diff evidence') },
  };
  const accepted = {
    ...state,
    reference: 'source-pin',
    referenceHub: 'hub-pin',
    artifactDigests: digests,
  };
  const prior = {
    scope: 'main',
    reference: 'source-pin',
    referenceHub: 'hub-pin',
    results: [
      {
        key: state.key,
        pass: false,
        artifactDigests: digests,
        artifactPaths: { candidateEvidence: 'pixels/candidate.json' },
      },
    ],
  };
  const current = structuredClone(prior);
  current.results[0].artifactDigests.candidate.evidence = hash(next);
  fs.writeFileSync(path.join(previousRoot, 'pixels/candidate.json'), old);
  fs.writeFileSync(path.join(root, 'pixels/candidate.json'), next);
  const write = (base, report) =>
    fs.writeFileSync(path.join(base, 'pixels/report.json'), JSON.stringify(report));
  write(previousRoot, prior);
  const run = (report = current, registry = { acceptances: [accepted] }) => {
    write(root, report);
    return audit({ root, previousRoot, registry });
  };
  assert.equal(run().qualified[0].reason, 'canonical-pathname-only');
  assert.equal(run(prior).qualified[0].reason, 'all-six-digests-identical');
  for (const mutate of [
    (report) => {
      report.reference = 'other';
    },
    (report) => {
      report.results[0].artifactDigests.candidate.image = hash('changed');
    },
    (report) => {
      report.results[0].artifactDigests.reference.evidence = hash('changed');
    },
    (report) => {
      report.results[0].artifactDigests.diff.evidence = hash('changed');
    },
    (report) => {
      report.results[0].artifactPaths.candidateEvidence = '../escape.json';
    },
    (report) => {
      report.results[0].key = 'unmatched';
    },
  ]) {
    const changed = structuredClone(current);
    mutate(changed);
    assert.equal(run(changed).refused.length, 1);
  }
  assert.equal(run(current, { acceptances: [accepted, accepted] }).refused.length, 1);
  const unapproved = structuredClone(prior);
  unapproved.results[0].artifactDigests.candidate.evidence = hash('not accepted');
  write(previousRoot, unapproved);
  assert.equal(run().refused.length, 1);
  write(previousRoot, prior);
  const changedText = next.replace('same', 'changed');
  fs.writeFileSync(path.join(root, 'pixels/candidate.json'), changedText);
  const changedReport = structuredClone(current);
  changedReport.results[0].artifactDigests.candidate.evidence = hash(changedText);
  assert.equal(run(changedReport).refused.length, 1);
  assert.equal(run().refused.length, 1); // Recorded digest cannot substitute for actual bytes.
});

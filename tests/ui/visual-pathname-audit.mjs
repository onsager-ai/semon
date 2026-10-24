// CI-only qualification evidence. Never writes acceptance records or changes pixels.
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';

const SIDES = ['candidate', 'reference', 'diff'];
const TYPES = ['image', 'evidence'];
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const identity = (row) => JSON.stringify([row.comparator, row.scope, row.kind, row.key]);

export function pathnameOnly(previousBytes, currentBytes) {
  const previous = JSON.parse(previousBytes),
    current = JSON.parse(currentBytes);
  // Synthetic local fixtures only. Preserve the exact encoded native id.
  const match = /^\/s\/(claude|codex)\/([^/?#]+)$/.exec(previous.pathname ?? '');
  if (!match || current.pathname !== `/session/local/${match[2]}`) return false;
  let count = 0;
  const replaced = previousBytes.replace(/"pathname":(\s*)"(?:[^"\\]|\\.)*"/g, (_, space) => {
    count++;
    return `"pathname":${space}${JSON.stringify(current.pathname)}`;
  });
  return count === 1 && replaced === currentBytes;
}

function bytes(file) {
  if (fs.statSync(file).size > 8 * 1024 * 1024) throw new Error('Input exceeds 8 MiB');
  return fs.readFileSync(file, 'utf8');
}
function read(file) {
  return JSON.parse(bytes(file));
}
function evidenceFile(root, relative) {
  if (typeof relative !== 'string') throw new Error('Missing evidence path');
  const result = path.resolve(root, relative);
  if (!result.startsWith(path.resolve(root) + path.sep))
    throw new Error('Evidence path escapes root');
  return result;
}
function rows(report) {
  return [
    ...(report.results ?? []).map((row) => ({ kind: 'frame', row })),
    ...(report.regions ?? []).map((row) => ({ kind: 'region', row })),
  ];
}
function sameReferences(a, b) {
  return a.reference === b.reference && (a.referenceHub ?? null) === (b.referenceHub ?? null);
}

export function audit({ root, previousRoot, registry }) {
  const qualified = [],
    refused = [],
    reports = [];
  const sources = [
    ['preservation', 'preservation/report.json'],
    ['pixels', 'pixels/report.json'],
    ['pixels', 'two-machines/pixels/report.json'],
    ['pixels', 'pushed/pixels/report.json'],
  ];
  for (const [comparator, source] of sources) {
    if (!fs.existsSync(path.join(root, source))) continue;
    const current = read(path.join(root, source));
    const currentRows = rows(current);
    if (currentRows.length > 1000) throw new Error('Report exceeds 1000 rows');
    const scope = current.scope ?? current.mode ?? 'required';
    reports.push({
      source,
      scope,
      errors: current.errors ?? [],
      coverageFailures: current.coverageFailures ?? null,
    });
    let previous;
    try {
      previous = read(path.join(previousRoot, source));
    } catch {
      previous = null;
    }
    for (const { kind, row } of currentRows.filter(({ row }) => row.pass === false)) {
      const state = { comparator, scope, kind, key: row.key };
      try {
        const currentMatches = currentRows.filter(
          (entry) => entry.kind === kind && entry.row.key === row.key,
        );
        if (currentMatches.length !== 1) throw new Error('Current exact state duplicated');
        const matches = registry.acceptances.filter((entry) => identity(entry) === identity(state));
        if (matches.length !== 1) throw new Error('Expected one accepted state');
        const accepted = matches[0];
        if (!sameReferences(current, accepted)) throw new Error('Reference pins changed');
        if (
          !previous ||
          !sameReferences(previous, accepted) ||
          (previous.scope ?? previous.mode ?? 'required') !== scope
        )
          throw new Error('Previous exact report missing or references/scope changed');
        const priorRows = rows(previous).filter(
          (entry) => entry.kind === kind && entry.row.key === row.key,
        );
        if (priorRows.length !== 1) throw new Error('Previous exact state missing or duplicated');
        const prior = priorRows[0].row;
        for (const side of SIDES) {
          for (const type of TYPES) {
            const approved = accepted.artifactDigests?.[side]?.[type];
            if (!approved || prior.artifactDigests?.[side]?.[type] !== approved)
              throw new Error(
                'Previous artifact is not the accepted binding: ' + side + '.' + type,
              );
            if (
              (side !== 'candidate' || type !== 'evidence') &&
              row.artifactDigests?.[side]?.[type] !== approved
            )
              throw new Error('Non-path evidence/image changed: ' + side + '.' + type);
          }
        }
        const approvedDigest = accepted.artifactDigests.candidate.evidence;
        const currentDigest = row.artifactDigests?.candidate?.evidence;
        let reason = 'all-six-digests-identical';
        if (currentDigest !== approvedDigest) {
          const oldBytes = bytes(
            evidenceFile(previousRoot, prior.artifactPaths?.candidateEvidence),
          );
          const newBytes = bytes(evidenceFile(root, row.artifactPaths?.candidateEvidence));
          if (digest(oldBytes) !== approvedDigest || digest(newBytes) !== currentDigest)
            throw new Error('Candidate evidence bytes do not match recorded digests');
          if (!pathnameOnly(oldBytes, newBytes))
            throw new Error('Evidence differs beyond canonical local pathname');
          reason = 'canonical-pathname-only';
        }
        qualified.push({
          ...state,
          reason,
          candidateFingerprint: current.candidateFingerprint,
          artifactDigests: row.artifactDigests,
          priorReviewedCandidate: accepted.reviewedCandidate,
        });
      } catch (error) {
        refused.push({ ...state, reason: error.message });
      }
    }
  }
  if (!reports.length) throw new Error('No current capture reports');
  return {
    version: 1,
    diagnosticOnly: true,
    previousRun: 39948275965,
    previousArtifacts: [12500060720, 12499047700],
    reports,
    qualified,
    refused,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const root = process.env.SEMON_UI_OUT,
    previousRoot = process.env.SEMON_VISUAL_PREVIOUS;
  if (!root || !previousRoot)
    throw new Error('SEMON_UI_OUT and SEMON_VISUAL_PREVIOUS are required');
  const result = audit({
    root,
    previousRoot,
    registry: read(new URL('./visual-acceptance.json', import.meta.url)),
  });
  fs.writeFileSync(
    path.join(root, 'visual-pathname-audit.json'),
    JSON.stringify(result, null, 2) + '\n',
  );
  console.log(
    JSON.stringify({
      qualified: result.qualified.length,
      refused: result.refused.length,
      firstRefusals: result.refused.slice(0, 8).map((entry) => ({
        key: String(entry.key).slice(0, 180),
        reason: entry.reason,
      })),
      output: 'visual-pathname-audit.json',
    }),
  );
  if (result.refused.length) process.exitCode = 1;
}

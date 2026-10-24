// CI-only qualification evidence. Never writes acceptance records or changes pixels.
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';

const SIDES = ['candidate', 'reference', 'diff'];
const TYPES = ['image', 'evidence'];
const MAX_FILE_BYTES = 8 * 1024 * 1024;
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const isDigest = (value) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const identity = (row) => JSON.stringify([row.comparator, row.scope, row.kind, row.key]);

function readBytes(file) {
  if (fs.statSync(file).size > MAX_FILE_BYTES) throw new Error('Input exceeds 8 MiB');
  return fs.readFileSync(file);
}

function decodeJson(bytes) {
  return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
}

function evidenceFile(root, relative) {
  if (typeof relative !== 'string') throw new Error('Missing candidate evidence path');
  const resolvedRoot = path.resolve(root);
  const result = path.resolve(resolvedRoot, relative);
  if (!result.startsWith(resolvedRoot + path.sep))
    throw new Error('Candidate evidence path escapes root');
  return result;
}

function stringEnd(text, start) {
  for (let index = start + 1; index < text.length; index++) {
    if (text[index] === '\\') index++;
    else if (text[index] === '"') return index + 1;
  }
  throw new Error('Invalid pathname JSON string');
}

function pathnameToken(text) {
  const parsed = JSON.parse(text);
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed))
    throw new Error('Candidate evidence root must be an object');

  const matches = [];
  let depth = 0;
  for (let index = 0; index < text.length; ) {
    const character = text[index];
    if (character === '"') {
      const end = stringEnd(text, index);
      const value = JSON.parse(text.slice(index, end));
      let after = end;
      while (/\s/.test(text[after] ?? '')) after++;
      if (text[after] === ':' && value === 'pathname') {
        if (depth !== 1) throw new Error('Pathname property must be top-level');
        after++;
        while (/\s/.test(text[after] ?? '')) after++;
        if (text[after] !== '"') throw new Error('Pathname value must be a JSON string');
        const valueEnd = stringEnd(text, after);
        matches.push({ start: after, end: valueEnd, value: JSON.parse(text.slice(after, valueEnd)) });
      }
      index = end;
    } else {
      if (character === '{' || character === '[') depth++;
      else if (character === '}' || character === ']') depth--;
      index++;
    }
  }
  if (matches.length !== 1) throw new Error('Expected exactly one top-level pathname string');
  return matches[0];
}

function historicalPathname(bytes, approvedDigest) {
  const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  const token = pathnameToken(text);
  const currentPath = token.value;
  if (text.slice(token.start, token.end) !== JSON.stringify(currentPath))
    throw new Error('Pathname JSON escaping is not canonical');
  const match = /^\/session\/local\/([^/?#]+)$/.exec(currentPath);
  if (!match) throw new Error('Current pathname is outside /session/local/<id>');
  let decodedId;
  try {
    decodedId = decodeURIComponent(match[1]);
  } catch {
    throw new Error('Current pathname has invalid encoded identity');
  }
  if (encodeURIComponent(decodedId) !== match[1])
    throw new Error('Current pathname identity escaping changed');

  const matches = [];
  for (const namespace of ['claude', 'codex']) {
    const replacement = JSON.stringify(`/s/${namespace}/${match[1]}`);
    const restored = Buffer.from(
      text.slice(0, token.start) + replacement + text.slice(token.end),
      'utf8',
    );
    if (digest(restored) === approvedDigest) matches.push(namespace);
  }
  if (matches.length !== 1)
    throw new Error('No unique accepted historical fixture identity hash');
  return { namespace: matches[0], digest: approvedDigest };
}

export function qualifyCandidateEvidence(currentBytes, approvedDigest) {
  const currentDigest = digest(currentBytes);
  if (currentDigest === approvedDigest)
    return { reason: 'fingerprint-only', historicalFixture: null };
  const historicalFixture = historicalPathname(currentBytes, approvedDigest);
  return { reason: 'canonical-pathname-only', historicalFixture };
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

function reportFiles(root) {
  return [
    ['preservation', 'preservation/report.json'],
    ['pixels', 'pixels/report.json'],
    ['pixels', 'two-machines/pixels/report.json'],
    ['pixels', 'pushed/pixels/report.json'],
  ];
}

export function audit({ root, registry }) {
  const qualified = [],
    refused = [],
    reports = [];
  let totalRows = 0;
  for (const [comparator, source] of reportFiles(root)) {
    const reportPath = path.join(root, source);
    if (!fs.existsSync(reportPath)) continue;
    const current = decodeJson(readBytes(reportPath));
    const currentRows = rows(current);
    totalRows += currentRows.length;
    if (totalRows > 1000) throw new Error('Combined reports exceed 1000 rows');
    const scope = current.scope ?? current.mode ?? 'required';
    reports.push({
      source,
      scope: String(scope).slice(0, 100),
      errorCount: Array.isArray(current.errors) ? current.errors.length : 0,
      firstErrors: (Array.isArray(current.errors) ? current.errors : [])
        .slice(0, 4)
        .map((error) => String(error).slice(0, 180)),
      coverageFailures:
        typeof current.coverageFailures === 'number' ||
        typeof current.coverageFailures === 'boolean' ||
        current.coverageFailures === null ||
        current.coverageFailures === undefined
          ? (current.coverageFailures ?? null)
          : String(current.coverageFailures).slice(0, 180),
    });
    for (const { kind, row } of currentRows.filter(({ row }) => row.pass === false)) {
      const exactState = { comparator, scope, kind, key: row.key };
      const state = {
        comparator,
        scope: String(scope).slice(0, 100),
        kind,
        key: String(row.key).slice(0, 180),
      };
      try {
        const currentMatches = currentRows.filter(
          (entry) => entry.kind === kind && entry.row.key === row.key,
        );
        if (currentMatches.length !== 1) throw new Error('Current exact state duplicated');
        const matches = registry.acceptances.filter(
          (entry) => identity(entry) === identity(exactState),
        );
        if (matches.length !== 1) throw new Error('Expected one accepted state');
        const accepted = matches[0];
        if (!sameReferences(current, accepted)) throw new Error('Reference pins changed');
        const approved = accepted.artifactDigests;
        if (!isDigest(approved?.candidate?.evidence))
          throw new Error('Accepted candidate evidence digest missing or invalid');
        const currentDigests = {};
        for (const side of SIDES) {
          currentDigests[side] = {};
          for (const type of TYPES) {
            if (side === 'candidate' && type === 'evidence') {
              currentDigests[side][type] = row.artifactDigests?.[side]?.[type];
              continue;
            }
            if (
              !isDigest(approved?.[side]?.[type]) ||
              row.artifactDigests?.[side]?.[type] !== approved[side][type]
            )
              throw new Error(`Non-path evidence/image changed: ${side}.${type}`);
            currentDigests[side][type] = row.artifactDigests[side][type];
          }
        }

        const candidateBytes = readBytes(
          evidenceFile(root, row.artifactPaths?.candidateEvidence),
        );
        const candidateDigest = digest(candidateBytes);
        if (row.artifactDigests?.candidate?.evidence !== candidateDigest)
          throw new Error('Current candidate evidence bytes do not match recorded digest');
        const evidence = qualifyCandidateEvidence(candidateBytes, approved.candidate.evidence);
        qualified.push({
          ...state,
          reason: evidence.reason,
          historicalFixture: evidence.historicalFixture,
          acceptedCandidateEvidenceSha256: approved.candidate.evidence,
          candidateFingerprint: String(current.candidateFingerprint ?? '').slice(0, 128),
          artifactDigests: currentDigests,
          priorReviewedCandidate: String(accepted.reviewedCandidate ?? '').slice(0, 128),
        });
      } catch (error) {
        refused.push({ ...state, reason: String(error?.message ?? error).slice(0, 180) });
      }
    }
  }
  if (!reports.length) throw new Error('No current capture reports');
  return {
    version: 2,
    diagnosticOnly: true,
    reports,
    qualified,
    refused,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const root = process.env.SEMON_UI_OUT;
  if (!root) throw new Error('SEMON_UI_OUT is required');
  const result = audit({
    root,
    registry: decodeJson(readBytes(new URL('./visual-acceptance.json', import.meta.url))),
  });
  fs.writeFileSync(
    path.join(root, 'visual-pathname-audit.json'),
    JSON.stringify(result, null, 2) + '\n',
  );
  console.log(
    JSON.stringify({
      qualified: result.qualified.length,
      refused: result.refused.length,
      qualifiedStates: result.qualified.slice(0, 6).map(({ key, reason }) => ({
        key: String(key).slice(0, 80),
        reason: String(reason).slice(0, 80),
      })),
      refusedStates: result.refused.slice(0, 6).map(({ key, reason }) => ({
        key: String(key).slice(0, 80),
        reason: String(reason).slice(0, 120),
      })),
      output: 'visual-pathname-audit.json',
    }),
  );
  if (result.refused.length) process.exitCode = 1;
}

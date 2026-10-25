// Diagnostic-only proposal for exact visual bindings. This module never edits the registry.
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';

const MAX_FILE_BYTES = 8 * 1024 * 1024;
const SHA256 = /^[a-f0-9]{64}$/;
const SIDES = ['candidate', 'reference', 'diff'];
const TYPES = ['image', 'evidence'];
const SUITES = {
  visual: [
    {
      reportPath: 'preservation/report.json',
      auditSource: 'preservation/report.json',
      comparator: 'preservation',
      scope: 'required',
      frames: 68,
      regions: 0,
    },
    {
      reportPath: 'pixels/report.json',
      auditSource: 'pixels/report.json',
      comparator: 'pixels',
      scope: 'main',
      frames: 113,
      regions: 162,
    },
  ],
  transport: [
    {
      reportPath: 'two-machines/pixels/report.json',
      auditSource: 'two-machines/pixels/report.json',
      comparator: 'pixels',
      scope: 'two-machines',
      frames: 12,
      regions: 20,
    },
    {
      reportPath: 'pushed/pixels/report.json',
      auditSource: 'pushed/pixels/report.json',
      comparator: 'pixels',
      scope: 'pushed',
      frames: 15,
      regions: 24,
    },
  ],
};

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const isSha256 = (value) => typeof value === 'string' && SHA256.test(value);
const identity = (row) => JSON.stringify([row.comparator, row.scope, row.kind, row.key]);

function rows(report) {
  return [
    ...(Array.isArray(report?.results)
      ? report.results.map((row) => ({ kind: 'frame', row }))
      : []),
    ...(Array.isArray(report?.regions)
      ? report.regions.map((row) => ({ kind: 'region', row }))
      : []),
  ];
}

function sameReferences(report, accepted) {
  return (
    report.reference === accepted.reference &&
    (report.referenceHub ?? null) === (accepted.referenceHub ?? null)
  );
}

function sameNonCandidateEvidence(current, accepted) {
  for (const side of SIDES) {
    for (const type of TYPES) {
      if (side === 'candidate' && type === 'evidence') continue;
      const actual = current?.[side]?.[type];
      const prior = accepted?.[side]?.[type];
      if (!isSha256(actual) || !isSha256(prior) || actual !== prior)
        return `${side}.${type} changed or is invalid`;
    }
  }
  return null;
}

function validateCompleteness(report, spec) {
  const problems = [];
  if (!report || typeof report !== 'object') return ['missing report'];
  const scope = report.scope ?? report.mode ?? 'required';
  if (scope !== spec.scope) problems.push(`expected scope ${spec.scope}`);
  if (!Array.isArray(report.errors) || report.errors.length !== 0)
    problems.push('report has execution errors');
  if (!Array.isArray(report.failures?.coverage) || report.failures.coverage.length !== 0)
    problems.push('report has missing or failed coverage');
  if (!Array.isArray(report.results) || report.results.length !== spec.frames)
    problems.push(`expected ${spec.frames} frames`);
  if (spec.regions === 0) {
    if ((report.regions ?? []).length !== 0) problems.push('unexpected regions');
  } else if (!Array.isArray(report.regions) || report.regions.length !== spec.regions) {
    problems.push(`expected ${spec.regions} regions`);
  }
  if (!isSha256(report.candidateFingerprint)) problems.push('invalid candidate fingerprint');

  const allRows = rows(report);
  const seen = new Set();
  for (const { kind, row } of allRows) {
    if (!row || typeof row.key !== 'string' || typeof row.pass !== 'boolean') {
      problems.push('invalid or unclassified visual row');
      break;
    }
    const state = identity({ comparator: spec.comparator, scope, kind, key: row.key });
    if (seen.has(state)) {
      problems.push(`duplicate visual state ${row.key.slice(0, 100)}`);
      break;
    }
    seen.add(state);
  }
  return problems;
}

function patchForRegistry(beforeText, afterText) {
  if (beforeText === afterText) return '';
  if (!beforeText.endsWith('\n') || !afterText.endsWith('\n'))
    throw new Error('Registry text must end with a newline');
  const before = beforeText.slice(0, -1).split('\n');
  const after = afterText.slice(0, -1).split('\n');
  if (before.length !== after.length) throw new Error('Proposed registry changes its line count');
  const changed = [];
  for (let index = 0; index < before.length; index++)
    if (before[index] !== after[index]) changed.push(index);
  if (!changed.length) return '';

  const hunks = [];
  for (const index of changed) {
    const start = Math.max(0, index - 3);
    const end = Math.min(before.length, index + 4);
    const previous = hunks.at(-1);
    if (previous && start <= previous.end) previous.end = Math.max(previous.end, end);
    else hunks.push({ start, end });
  }
  const lines = [
    'diff --git a/tests/ui/visual-acceptance.json b/tests/ui/visual-acceptance.json',
    '--- a/tests/ui/visual-acceptance.json',
    '+++ b/tests/ui/visual-acceptance.json',
  ];
  for (const { start, end } of hunks) {
    const count = end - start;
    lines.push(`@@ -${start + 1},${count} +${start + 1},${count} @@`);
    for (let index = start; index < end; index++) {
      if (before[index] === after[index]) lines.push(' ' + before[index]);
      else lines.push('-' + before[index], '+' + after[index]);
    }
  }
  return lines.join('\n') + '\n';
}

function withoutProposedFields(registry, changedIndexes) {
  const copy = structuredClone(registry);
  for (const index of changedIndexes) {
    const entry = copy.acceptances[index];
    delete entry.candidateFingerprint;
    delete entry.artifactDigests.candidate.evidence;
  }
  return copy;
}

export function proposeVisualBindings({ suite, reports, audit, registry, registryText }) {
  const refusals = [];
  const specs = SUITES[suite];
  if (!specs) refusals.push('unknown proposal suite');
  if (!registry || !Array.isArray(registry.acceptances))
    refusals.push('invalid acceptance registry');
  if (!audit || audit.diagnosticOnly !== true) refusals.push('missing diagnostic pathname audit');
  if (!Array.isArray(audit?.qualified) || !Array.isArray(audit?.refused))
    refusals.push('invalid pathname audit result');
  if (!Array.isArray(audit?.reports)) refusals.push('missing pathname audit report summaries');
  if (refusals.length) return { suite, status: 'refused', refusals, patch: '', updates: [] };

  if (!reports || typeof reports !== 'object')
    return {
      suite,
      status: 'refused',
      refusals: ['missing visual reports'],
      patch: '',
      updates: [],
    };
  const expectedSources = new Set(specs.map((spec) => spec.auditSource));
  if (
    audit.reports.length !== specs.length ||
    audit.reports.some((item) => !expectedSources.has(item.source))
  )
    refusals.push('pathname audit report set does not match this suite');
  const fingerprints = new Set(specs.map((spec) => reports[spec.reportPath]?.candidateFingerprint));
  if (fingerprints.size !== 1 || !isSha256([...fingerprints][0]))
    refusals.push('reports do not share one valid runtime fingerprint');

  const registryCopy = structuredClone(registry);
  const updates = [];
  const expectedStates = new Set();
  for (const spec of specs) {
    const report = reports[spec.reportPath];
    for (const problem of validateCompleteness(report, spec))
      refusals.push(`${spec.reportPath}: ${problem}`);

    const auditSummaries = audit.reports.filter((item) => item.source === spec.auditSource);
    if (
      auditSummaries.length !== 1 ||
      auditSummaries[0]?.scope !== spec.scope ||
      auditSummaries[0]?.errorCount !== 0 ||
      auditSummaries[0]?.candidateFingerprint !== report?.candidateFingerprint ||
      !isSha256(auditSummaries[0]?.reportSha256)
    ) {
      refusals.push(`${spec.auditSource}: audit summary missing, duplicated, or has errors`);
    }
    if (!report || typeof report !== 'object') continue;

    for (const { kind, row } of rows(report).filter(({ row }) => row?.pass === false)) {
      const state = { comparator: spec.comparator, scope: spec.scope, kind, key: row.key };
      const stateId = identity(state);
      if (expectedStates.has(stateId)) {
        refusals.push(
          `${spec.reportPath}: duplicate failing state ${String(row.key).slice(0, 100)}`,
        );
        continue;
      }
      expectedStates.add(stateId);
      const acceptedMatches = registry.acceptances
        .map((entry, index) => ({ entry, index }))
        .filter(({ entry }) => identity(entry ?? {}) === stateId);
      if (acceptedMatches.length !== 1) {
        refusals.push(
          `${spec.reportPath}: expected one accepted record for ${String(row.key).slice(0, 100)}`,
        );
        continue;
      }
      const { entry: accepted, index } = acceptedMatches[0];
      if (accepted.decision !== 'accepted') {
        refusals.push(`${spec.reportPath}: state has no accepted decision`);
        continue;
      }
      const qualifiedMatches = audit.qualified.filter((item) => identity(item) === stateId);
      const refusedMatches = audit.refused.filter((item) => identity(item) === stateId);
      if (qualifiedMatches.length !== 1 || refusedMatches.length !== 0) {
        refusals.push(
          `${spec.reportPath}: audit did not uniquely qualify ${String(row.key).slice(0, 100)}`,
        );
        continue;
      }
      const qualification = qualifiedMatches[0];
      if (!['fingerprint-only', 'canonical-pathname-only'].includes(qualification.reason)) {
        refusals.push(`${spec.reportPath}: unsupported audit qualification`);
        continue;
      }
      if (!sameReferences(report, accepted)) {
        refusals.push(
          `${spec.reportPath}: reference pins changed for ${String(row.key).slice(0, 100)}`,
        );
        continue;
      }
      const currentDigests = row.artifactDigests;
      const digestProblem = sameNonCandidateEvidence(currentDigests, accepted.artifactDigests);
      if (digestProblem) {
        refusals.push(`${spec.reportPath}: ${digestProblem} for ${String(row.key).slice(0, 100)}`);
        continue;
      }
      const currentEvidence = currentDigests?.candidate?.evidence;
      const acceptedEvidence = accepted.artifactDigests?.candidate?.evidence;
      if (
        !isSha256(currentEvidence) ||
        !isSha256(acceptedEvidence) ||
        qualification.candidateFingerprint !== report.candidateFingerprint ||
        qualification.artifactDigests?.candidate?.evidence !== currentEvidence ||
        qualification.acceptedCandidateEvidenceSha256 !== acceptedEvidence ||
        (qualification.reason === 'fingerprint-only' && currentEvidence !== acceptedEvidence) ||
        (qualification.reason === 'canonical-pathname-only' && currentEvidence === acceptedEvidence)
      ) {
        refusals.push(
          `${spec.reportPath}: audit hashes do not match ${String(row.key).slice(0, 100)}`,
        );
        continue;
      }
      if (!isSha256(accepted.candidateFingerprint) || !accepted.artifactDigests?.candidate) {
        refusals.push(
          `${spec.reportPath}: accepted binding is incomplete for ${String(row.key).slice(0, 100)}`,
        );
        continue;
      }

      const proposed = registryCopy.acceptances[index];
      proposed.candidateFingerprint = report.candidateFingerprint;
      proposed.artifactDigests.candidate.evidence = currentEvidence;
      if (
        accepted.candidateFingerprint !== report.candidateFingerprint ||
        acceptedEvidence !== currentEvidence
      ) {
        updates.push({ index, ...state });
      }
    }
  }

  if (audit.qualified.length !== expectedStates.size || audit.refused.length !== 0)
    refusals.push('audit states do not exactly match report failures');
  if (refusals.length)
    return {
      suite,
      status: 'refused',
      refusals: refusals.slice(0, 40),
      patch: '',
      updates: [],
    };

  if (!updates.length) return { suite, status: 'no-changes', refusals: [], patch: '', updates: [] };
  const canonicalRegistryText = JSON.stringify(registry, null, 2) + '\n';
  if (registryText !== canonicalRegistryText)
    return {
      suite,
      status: 'refused',
      refusals: ['acceptance registry is not in canonical JSON form'],
      patch: '',
      updates: [],
    };
  const proposedText = JSON.stringify(registryCopy, null, 2) + '\n';
  const updatedIndexes = new Set(updates.map(({ index }) => index));
  if (
    JSON.stringify(withoutProposedFields(registry, updatedIndexes)) !==
    JSON.stringify(withoutProposedFields(registryCopy, updatedIndexes))
  ) {
    return {
      suite,
      status: 'refused',
      refusals: ['proposal changes fields outside candidate fingerprint/evidence hashes'],
      patch: '',
      updates: [],
    };
  }
  const patch = patchForRegistry(registryText, proposedText);
  return {
    suite,
    status: 'proposed',
    refusals: [],
    patch,
    updates,
    proposedRegistry: registryCopy,
  };
}

function boundedBytes(file) {
  if (fs.statSync(file).size > MAX_FILE_BYTES) throw new Error('Input exceeds 8 MiB');
  return fs.readFileSync(file);
}

const decodeJson = (bytes) => JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));

function runCli() {
  const root = process.env.SEMON_UI_OUT;
  const suite = process.env.SEMON_VISUAL_PROPOSAL_SUITE;
  if (!root) {
    console.log(JSON.stringify({ status: 'refused', reason: 'SEMON_UI_OUT is required' }));
    process.exitCode = 1;
    return;
  }
  fs.mkdirSync(root, { recursive: true });
  const safeSuite = SUITES[suite] ? suite : 'unknown';
  const patchFile = path.join(root, `visual-acceptance-${safeSuite}-review.patch`);
  const summaryFile = path.join(root, `visual-acceptance-${safeSuite}-proposal.json`);
  let result;
  let provenance;
  try {
    const specs = SUITES[suite];
    if (!specs) throw new Error('unknown proposal suite');
    const sourceSha = process.env.GITHUB_SHA;
    if (typeof sourceSha !== 'string' || !/^[a-f0-9]{40}$/.test(sourceSha))
      throw new Error('missing or invalid CI source SHA');
    const audit = decodeJson(boundedBytes(path.join(root, 'visual-pathname-audit.json')));
    const reportDigests = {};
    const reports = Object.fromEntries(
      specs.map((spec) => {
        const bytes = boundedBytes(path.join(root, spec.reportPath));
        const reportSha256 = sha256(bytes);
        const summaries = audit.reports?.filter((item) => item.source === spec.auditSource);
        if (summaries?.length !== 1 || summaries[0].reportSha256 !== reportSha256)
          throw new Error(`${spec.reportPath}: audit report digest does not match current bytes`);
        reportDigests[spec.reportPath] = reportSha256;
        return [spec.reportPath, decodeJson(bytes)];
      }),
    );
    const registryPath = new URL('./visual-acceptance.json', import.meta.url);
    const registryBytes = boundedBytes(registryPath);
    const registryText = new TextDecoder('utf-8', { fatal: true }).decode(registryBytes);
    provenance = {
      sourceSha,
      runtimeFingerprint: reports[specs[0].reportPath].candidateFingerprint,
      reportDigests,
      qualifiedCount: audit.qualified?.length,
      refusedCount: audit.refused?.length,
    };
    result = proposeVisualBindings({
      suite,
      reports,
      audit,
      registry: JSON.parse(registryText),
      registryText,
    });
  } catch (error) {
    result = {
      suite: safeSuite,
      status: 'refused',
      refusals: [String(error?.message ?? error).slice(0, 240)],
      patch: '',
      updates: [],
    };
  }
  if (result.status === 'proposed' && result.patch) fs.writeFileSync(patchFile, result.patch);
  else fs.rmSync(patchFile, { force: true });
  fs.writeFileSync(
    summaryFile,
    JSON.stringify(
      {
        version: 1,
        diagnosticOnly: true,
        provenance,
        suite: result.suite,
        status: result.status,
        proposedEntries: result.updates.map(({ comparator, scope, kind, key }) => ({
          comparator,
          scope,
          kind,
          key: String(key).slice(0, 120),
        })),
        refusalCount: result.refusals.length,
        refusals: result.refusals.slice(0, 20).map((reason) => String(reason).slice(0, 240)),
        patch: result.status === 'proposed' ? path.basename(patchFile) : null,
      },
      null,
      2,
    ) + '\n',
  );
  console.log(
    JSON.stringify({
      suite: result.suite,
      status: result.status,
      proposedEntries: result.updates.length,
      refusalCount: result.refusals.length,
      refusals: result.refusals.slice(0, 4).map((reason) => String(reason).slice(0, 160)),
      patch: result.status === 'proposed' ? path.basename(patchFile) : null,
    }),
  );
  if (result.status === 'refused') process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) runCli();

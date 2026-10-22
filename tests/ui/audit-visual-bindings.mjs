// CI-only advisory proof. Never writes registry entries or changes raw gates.
import fs from 'node:fs';
import path from 'node:path';
import {
  assessPixelComparison,
  assessPreservation,
  assessVisualAcceptances,
  sha256,
} from './preservation-contract.mjs';

const out = path.resolve(process.env.SEMON_UI_OUT ?? new URL('./out', import.meta.url).pathname);
const audit = {
  version: 1,
  advisoryOnly: true,
  pass: false,
  errors: [],
  sources: [],
  entries: [],
  unregistered: [],
};
const requireProof = (condition, message) => {
  if (!condition) audit.errors.push(message);
};
const bindingId = (entry) => `${entry.comparator}/${entry.scope}/${entry.kind}/${entry.key}`;
const knownNew = ['phone-light', 'phone-dark', 'desktop-light']
  .map((scheme) => `pixels/main/region/${scheme}|session-deps|menu`)
  .sort();
const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);

function load(file) {
  const bytes = fs.readFileSync(file);
  return { value: JSON.parse(bytes), sha256: sha256(bytes) };
}

// Verify the actual same-run files as well as the capture report's digests.
// Artifact paths are confined to this report's output family, never arbitrary IO.
function digests(row, comparator, expected) {
  const checks = [];
  for (const side of ['candidate', 'reference', 'diff']) {
    for (const kind of ['image', 'evidence']) {
      const key =
        kind === 'image'
          ? `${side}Image`
          : side === 'diff'
            ? 'comparisonEvidence'
            : `${side}Evidence`;
      const relative = row?.artifactPaths?.[key];
      let fileDigest = null;
      try {
        if (
          typeof relative !== 'string' ||
          !relative.startsWith(comparator === 'pixels' ? 'pixels/' : 'preservation/') ||
          relative.split('/').some((part) => part === '..' || part === '.')
        )
          throw new Error('Invalid artifact path');
        const absolute = path.resolve(out, relative);
        if (!absolute.startsWith(out + path.sep)) throw new Error('Artifact escapes output');
        fileDigest = sha256(fs.readFileSync(absolute));
      } catch (error) {
        audit.errors.push(`${comparator}/${row?.key}/${side}/${kind}: ${error.message}`);
      }
      const observed = row?.artifactDigests?.[side]?.[kind] ?? null;
      const prior = expected?.[side]?.[kind] ?? null;
      const valid = /^[a-f0-9]{64}$/.test(observed ?? '');
      checks.push({
        side,
        kind,
        path: relative ?? null,
        expected: prior,
        observed,
        fileDigest,
        match: valid && fileDigest === observed && (expected === undefined || prior === observed),
      });
    }
  }
  return checks;
}

try {
  const registry = load(new URL('./visual-acceptance.json', import.meta.url));
  audit.registrySha256 = registry.sha256;
  const specs = [
    {
      comparator: 'preservation',
      scope: 'required',
      file: 'preservation/report.json',
      assess: assessPreservation,
      frames: 68,
      regions: 0,
      failures: 42,
      accepted: 42,
    },
    {
      comparator: 'pixels',
      scope: 'main',
      file: 'pixels.json',
      assess: assessPixelComparison,
      frames: 113,
      regions: 162,
      failures: 72,
      accepted: 69,
    },
  ];
  const loaded = specs.map((spec) => ({ ...spec, ...load(path.join(out, spec.file)) }));
  const fingerprint = loaded[0].value.candidateFingerprint;
  audit.candidateFingerprint = fingerprint;
  audit.candidate = process.env.GITHUB_SHA ?? null;
  requireProof(/^[a-f0-9]{40}$/.test(audit.candidate ?? ''), 'Missing CI candidate SHA');
  requireProof(/^[a-f0-9]{64}$/.test(fingerprint ?? ''), 'Invalid runtime fingerprint');
  requireProof(
    registry.value.acceptances?.length === 111,
    'Expected exactly 111 existing bindings',
  );
  const entries = registry.value.acceptances;
  if (!Array.isArray(entries)) throw new Error('Missing acceptance records');
  const ids = entries.map(bindingId);
  requireProof(new Set(ids).size === 111, 'Existing bindings are not unique');
  // The sole projection: retain every original review/pin/digest field verbatim.
  const projected = {
    ...registry.value,
    acceptances: entries.map((entry) => ({ ...entry, candidateFingerprint: fingerprint })),
  };
  const pending = [],
    accepted = [];
  for (const spec of loaded) {
    const report = spec.value;
    const raw = spec.assess(report);
    const assessment = assessVisualAcceptances(report, spec.comparator, raw, projected);
    audit.sources.push({
      file: spec.file,
      sha256: spec.sha256,
      candidate: report.candidate,
      candidateFingerprint: report.candidateFingerprint,
      reference: report.reference,
      referenceHub: report.referenceHub,
      comparator: spec.comparator,
      scope: report.scope ?? report.mode,
      frames: report.results?.length,
      regions: report.regions?.length ?? 0,
      coverageErrors: raw.coverage,
      rawVisualFailures: raw.visual.length,
      projectedAssessment: assessment,
    });
    requireProof(report.candidate === audit.candidate, `${spec.file}: wrong CI candidate`);
    requireProof(report.candidateFingerprint === fingerprint, `${spec.file}: fingerprint differs`);
    requireProof((report.scope ?? report.mode) === spec.scope, `${spec.file}: wrong scope`);
    requireProof(
      report.results?.length === spec.frames && (report.regions?.length ?? 0) === spec.regions,
      `${spec.file}: wrong coverage counts`,
    );
    requireProof(raw.coverage.length === 0, `${spec.file}: coverage errors`);
    requireProof(raw.visual.length === spec.failures, `${spec.file}: unexpected raw failures`);
    requireProof(same(report.failures, raw), `${spec.file}: stored raw failures do not reconcile`);
    requireProof(assessment.errors.length === 0, `${spec.file}: projected binding errors`);
    requireProof(
      assessment.accepted.length === spec.accepted,
      `${spec.file}: wrong accepted count`,
    );
    pending.push(...assessment.pending);
    accepted.push(...assessment.accepted);
    const rows = [
      ...(report.results ?? []).map((row) => ({ kind: 'frame', row })),
      ...(report.regions ?? []).map((row) => ({ kind: 'region', row })),
    ];
    for (const entry of entries.filter(
      (entry) => entry.comparator === spec.comparator && entry.scope === spec.scope,
    )) {
      const matches = rows.filter(({ kind, row }) => kind === entry.kind && row.key === entry.key);
      const found = matches.length === 1 ? matches[0] : null;
      const expectedIdentity = {
        comparator: entry.comparator,
        scope: entry.scope,
        kind: entry.kind,
        key: entry.key,
        reference: entry.reference,
        referenceHub: entry.referenceHub ?? null,
      };
      const observedIdentity = {
        comparator: spec.comparator,
        scope: report.scope ?? report.mode,
        kind: found?.kind ?? null,
        key: found?.row.key ?? null,
        reference: report.reference,
        referenceHub: report.referenceHub ?? null,
      };
      const identityMatch = matches.length === 1 && same(expectedIdentity, observedIdentity);
      const checks = digests(found?.row, spec.comparator, entry.artifactDigests);
      audit.entries.push({
        id: bindingId(entry),
        expectedIdentity,
        observedIdentity,
        identityMatch,
        priorCandidateFingerprint: entry.candidateFingerprint,
        candidateFingerprint: fingerprint,
        digests: checks,
        pass:
          identityMatch &&
          checks.every((check) => check.match) &&
          assessment.accepted.includes(bindingId(entry)),
      });
    }
    for (const id of assessment.pending) {
      const found = rows.filter(
        ({ kind, row }) =>
          bindingId({ comparator: spec.comparator, scope: spec.scope, kind, key: row.key }) === id,
      );
      requireProof(found.length === 1, `${id}: unregistered row not unique`);
      const row = found[0]?.row;
      const checks = digests(row, spec.comparator);
      audit.unregistered.push({
        id,
        comparator: spec.comparator,
        scope: spec.scope,
        kind: found[0]?.kind,
        key: row?.key,
        reference: report.reference,
        referenceHub: report.referenceHub,
        candidateFingerprint: fingerprint,
        artifactDigests: row?.artifactDigests,
        pixels: row?.pixels,
        ratio: row?.ratio,
        size: row?.size,
        digests: checks,
        pass: knownNew.includes(id) && checks.every((check) => check.match),
      });
    }
  }
  requireProof(
    same([...pending].sort(), knownNew),
    'Unregistered findings differ from the three reviewed Last-turn regions',
  );
  requireProof(
    accepted.length === 111 &&
      new Set(accepted).size === 111 &&
      ids.every((id) => accepted.includes(id)),
    'Existing binding coverage is incomplete',
  );
  requireProof(
    audit.entries.length === 111 && audit.entries.every((entry) => entry.pass),
    'At least one existing binding differs or is missing',
  );
  requireProof(
    audit.unregistered.length === 3 && audit.unregistered.every((entry) => entry.pass),
    'Unregistered region artifacts are incomplete',
  );
  audit.summary = {
    existingBindings: audit.entries.length,
    matchingBindings: audit.entries.filter((entry) => entry.pass).length,
    existingDigests: audit.entries.reduce((sum, entry) => sum + entry.digests.length, 0),
    matchingDigests: audit.entries.flatMap((entry) => entry.digests).filter((check) => check.match)
      .length,
    unregisteredRegions: audit.unregistered.length,
  };
  requireProof(
    audit.summary.existingDigests === 666 && audit.summary.matchingDigests === 666,
    'Expected exactly 666 matching existing artifact digests',
  );
  audit.pass = audit.errors.length === 0;
} catch (error) {
  audit.errors.push(error.message);
}
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(
  path.join(out, 'visual-binding-audit.json'),
  JSON.stringify(audit, null, 2) + '\n',
);
console.log(
  JSON.stringify({
    advisoryOnly: true,
    pass: audit.pass,
    ...audit.summary,
    errors: audit.errors.length,
    report: 'visual-binding-audit.json',
  }),
);
if (!audit.pass) process.exitCode = 1;

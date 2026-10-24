// CI-only diagnostic: compare each capture's exact bindings with the retained
// review. This never writes the registry or changes a comparator's result.
import fs from 'node:fs';
import path from 'node:path';

const root = process.env.SEMON_UI_OUT;
if (!root) throw new Error('SEMON_UI_OUT is required');
function read(file) {
  if (fs.statSync(file).size > 8 * 1024 * 1024)
    throw new Error('Visual provenance input exceeds 8 MiB: ' + file);
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}
const registry = read(new URL('./visual-acceptance.json', import.meta.url));
const sources = [
  ['preservation', path.join(root, 'preservation/report.json')],
  ['pixels', path.join(root, 'pixels/report.json')],
  ['pixels', path.join(root, 'two-machines/pixels/report.json')],
  ['pixels', path.join(root, 'pushed/pixels/report.json')],
];
const reports = [];
for (const [comparator, file] of sources) {
  if (!fs.existsSync(file)) continue;
  const report = read(file);
  const scope = report.scope ?? report.mode ?? 'required';
  const rows = [
    ...(report.results ?? []).map((row) => ({ kind: 'frame', row })),
    ...(report.regions ?? []).map((row) => ({ kind: 'region', row })),
  ];
  if (rows.length > 1000) throw new Error('Visual provenance exceeds 1000 rows');
  reports.push({
    comparator,
    scope,
    candidate: report.candidate,
    candidateFingerprint: report.candidateFingerprint,
    reference: report.reference,
    referenceHub: report.referenceHub ?? null,
    // Raw reports, errors, coverage and pixels remain in the same CI artifact.
    source: path.relative(root, file),
    cases: rows.map(({ kind, row }) => {
      const matches = registry.acceptances.filter(
        (entry) =>
          entry.comparator === comparator &&
          entry.scope === scope &&
          entry.kind === kind &&
          entry.key === row.key,
      );
      const previous = matches.length === 1 ? matches[0] : null;
      const changed = [];
      for (const side of ['candidate', 'reference', 'diff']) {
        for (const type of ['image', 'evidence']) {
          const actual = row.artifactDigests?.[side]?.[type];
          if (!actual || actual !== previous?.artifactDigests?.[side]?.[type])
            changed.push(side + '.' + type);
        }
      }
      return {
        kind,
        key: row.key,
        priorRecords: matches.length,
        priorDecision: previous?.decision ?? null,
        priorReviewedCandidate: previous?.reviewedCandidate ?? null,
        priorFingerprint: previous?.candidateFingerprint ?? null,
        sameReferences:
          Boolean(previous) &&
          previous.reference === report.reference &&
          (previous.referenceHub ?? null) === (report.referenceHub ?? null),
        changedDigests: changed,
        artifactDigests: row.artifactDigests ?? null,
        priorArtifactDigests: previous?.artifactDigests ?? null,
        ratio: row.ratio,
        size: row.size,
      };
    }),
  });
}
fs.mkdirSync(root, { recursive: true });
fs.writeFileSync(
  path.join(root, 'visual-provenance.json'),
  JSON.stringify({ version: 1, diagnosticOnly: true, reports }, null, 2) + '\n',
);
console.log('Retained per-case visual provenance for ' + reports.length + ' reports');

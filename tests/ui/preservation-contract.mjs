import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

// The selected complete application is independent of both the candidate and
// derived pen/mockup exports. Keep its identity and required coverage together.
export const REFERENCE = {
  source: '263a02da278716ab2309b8e34a463419e5c781e5',
  hub: '503fbe090749ea1d58b9ca9fa4ccae7289ee426e',
};
export const REFERENCE_MODEL_SOURCE =
  'pinned reference /api/model?delta=1&content=1 (reference-only)';
export const PARITY_MODEL_SOURCE = 'served parity model';
export const BEFORE = 'eb535ff8920fe9aa54e78babc9649cf6c9400ad6';
export const THRESHOLD = 0.1;
export const MAX_RATIO = 0.0001;
export const WIDTHS = [390, 820, 1280];
export const SCHEMES = ['light', 'dark'];
export const STATES = [
  'home',
  'sessions',
  'session-harbor',
  'harbor-thinking-open',
  'harbor-tools-open',
  'harbor-tool-output-open',
  'session-atlas-ingest',
  'session-deps',
  'session-h-failed',
  'analytics',
];
export const FILTER_STATE = 'sessions-filter-open';
export const HISTORICAL_EXPECTED = WIDTHS.flatMap((width) =>
  SCHEMES.flatMap((scheme) =>
    [...STATES, ...(width === 390 ? ['drawer'] : [])].map((state) => `${width}-${scheme}-${state}`),
  ),
);
export const EXPECTED = WIDTHS.flatMap((width) =>
  SCHEMES.flatMap((scheme) =>
    [...STATES, FILTER_STATE, ...(width === 390 ? ['drawer'] : [])].map(
      (state) => `${width}-${scheme}-${state}`,
    ),
  ),
);
const PIXEL_COVERAGE = JSON.parse(
  readFileSync(new URL('./visual-coverage.json', import.meta.url), 'utf8'),
);
const VISUAL_ACCEPTANCE = JSON.parse(
  readFileSync(new URL('./visual-acceptance.json', import.meta.url), 'utf8'),
);
export const PIXEL_REGIONS = PIXEL_COVERAGE.regions;

export function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function validArtifactDigests(value) {
  return (
    value &&
    ['candidate', 'reference', 'diff'].every((side) =>
      ['image', 'evidence'].every((kind) => /^[a-f0-9]{64}$/.test(value[side]?.[kind] ?? '')),
    )
  );
}

function sameArtifactDigests(left, right) {
  return (
    validArtifactDigests(left) &&
    validArtifactDigests(right) &&
    ['candidate', 'reference', 'diff'].every((side) =>
      ['image', 'evidence'].every((kind) => left[side][kind] === right[side][kind]),
    )
  );
}

function registryErrors(registry) {
  if (registry?.version !== 1 || !Array.isArray(registry.acceptances))
    return ['Invalid visual acceptance registry'];
  const errors = [],
    keys = new Set();
  for (const entry of registry.acceptances) {
    const allowedScope =
      (entry?.comparator === 'pixels' &&
        ['main', 'two-machines', 'pushed'].includes(entry.scope)) ||
      (entry?.comparator === 'preservation' && entry.scope === 'required');
    const expectedReference = entry?.scope === 'pushed' ? 'served-parity' : REFERENCE.source;
    const expectedHub = entry?.scope === 'pushed' ? null : REFERENCE.hub;
    let expectedKeys = [];
    if (allowedScope && entry.comparator === 'pixels') {
      const coverage = expectedPixelCoverage(entry.scope);
      expectedKeys = entry.kind === 'region' ? coverage.regions : coverage.frames;
    } else if (allowedScope && entry.comparator === 'preservation' && entry.kind === 'frame') {
      expectedKeys = EXPECTED;
    }
    const id = `${entry?.comparator}/${entry?.scope}/${entry?.kind}/${entry?.key}`;
    if (keys.has(id)) errors.push(`Duplicate visual acceptance binding: ${id}`);
    keys.add(id);
    if (
      !allowedScope ||
      !['frame', 'region'].includes(entry?.kind) ||
      (entry.comparator === 'preservation' && entry.kind !== 'frame') ||
      typeof entry.key !== 'string' ||
      !expectedKeys.includes(entry.key) ||
      entry.reference !== expectedReference ||
      (entry.referenceHub ?? null) !== expectedHub ||
      !/^[a-f0-9]{64}$/.test(entry.candidateFingerprint ?? '') ||
      !validArtifactDigests(entry.artifactDigests) ||
      entry.decision !== 'accepted' ||
      !/^[a-f0-9]{40}$/.test(entry.reviewedCandidate ?? '') ||
      typeof entry.reviewedBy !== 'string' ||
      !entry.reviewedBy.trim() ||
      typeof entry.reviewedAt !== 'string' ||
      Number.isNaN(Date.parse(entry.reviewedAt)) ||
      typeof entry.rationale !== 'string' ||
      !entry.rationale.trim()
    )
      errors.push(`Invalid visual acceptance record: ${id}`);
  }
  return errors;
}

function covers(region, screen) {
  return region.screens.some((name) =>
    name.endsWith('-') ? screen.startsWith(name) : screen === name,
  );
}

export function expectedPixelCoverage(scope) {
  const config = PIXEL_COVERAGE.scopes[scope];
  if (!config) throw new Error('Unknown pixel coverage scope: ' + scope);
  const frames = config.frames === 'all' ? PIXEL_COVERAGE.frames : config.frames;
  const full = [],
    regions = [];
  for (const scheme of config.schemes) {
    for (const frame of frames) {
      if (frame === 'drawer' && !scheme.startsWith('phone-')) continue;
      full.push(`${scheme}|${frame}`);
      for (const region of PIXEL_COVERAGE.regions)
        if (covers(region, frame)) regions.push(`${scheme}|${frame}|${region.name}`);
    }
  }
  return { frames: full, regions };
}

function inspectPixelRows(rows, expected, kind, coverage, visual) {
  const seen = new Set();
  if (!Array.isArray(rows)) {
    coverage.push(`Missing ${kind} comparison results`);
    return;
  }
  for (const row of rows) {
    if (!row || typeof row.key !== 'string') {
      coverage.push(`Invalid ${kind} comparison row`);
      continue;
    }
    if (!expected.includes(row.key)) coverage.push(`Unknown ${kind} case: ${row.key}`);
    if (seen.has(row.key)) coverage.push(`Duplicate ${kind} case: ${row.key}`);
    seen.add(row.key);
    if (
      !Number.isSafeInteger(row.pixels) ||
      row.pixels < 0 ||
      !Number.isFinite(row.ratio) ||
      row.ratio < 0 ||
      row.ratio > 1 ||
      !Object.hasOwn(row, 'size') ||
      (row.size !== null && (!Array.isArray(row.size) || row.size.length !== 2))
    ) {
      coverage.push(`Invalid ${kind} measurement: ${row.key}`);
      continue;
    }
    if (!validArtifactDigests(row.artifactDigests))
      coverage.push(`Missing or invalid ${kind} artifact digests: ${row.key}`);
    if (row.size !== null || row.ratio > MAX_RATIO)
      visual.push(
        `${row.key}: ${row.size ? 'size ' + row.size.join(' vs ') : 'pixel mismatch ' + row.ratio}`,
      );
  }
  for (const key of expected) if (!seen.has(key)) coverage.push(`Missing ${kind} case: ${key}`);
}

// Actual screenshot comparisons use a static coverage contract. Historical
// mockup ratios are preserved as provenance, never as active approval rules.
export function assessPixelComparison(report) {
  const coverage = [],
    visual = [];
  let expected;
  try {
    expected = expectedPixelCoverage(report.scope);
  } catch (error) {
    coverage.push(error.message);
    return { coverage, visual };
  }
  const expectedReference = report.scope === 'pushed' ? 'served-parity' : REFERENCE.source;
  if (report.reference !== expectedReference) coverage.push('Wrong actual visual reference');
  if (!/^[a-f0-9]{40}$/.test(report.candidate ?? '')) coverage.push('Missing candidate commit SHA');
  if (!/^[a-f0-9]{64}$/.test(report.candidateFingerprint ?? ''))
    coverage.push('Missing candidate runtime fingerprint');
  if (report.scope !== 'pushed' && report.referenceHub !== REFERENCE.hub)
    coverage.push('Wrong selected Hub provenance');
  if (report.coverageSource !== 'tests/ui/visual-coverage.json')
    coverage.push('Wrong visual coverage source');
  const expectedModelSource =
    report.scope === 'pushed' ? PARITY_MODEL_SOURCE : REFERENCE_MODEL_SOURCE;
  if (report.referenceModelSource !== expectedModelSource)
    coverage.push('Wrong visual reference model source');
  if (report.scope === 'pushed') {
    if (report.referenceModelDigest !== null)
      coverage.push('Unexpected pushed-parity reference model digest');
  } else if (!/^[a-f0-9]{64}$/.test(report.referenceModelDigest ?? '')) {
    coverage.push('Missing pinned reference model digest');
  }
  if (report.threshold !== THRESHOLD || report.maxRatio !== MAX_RATIO)
    coverage.push('Changed visual thresholds');
  if (!Array.isArray(report.errors) || report.errors.length)
    coverage.push(...(report.errors ?? ['Missing execution-error report']));
  inspectPixelRows(report.results, expected.frames, 'frame', coverage, visual);
  inspectPixelRows(report.regions, expected.regions, 'region', coverage, visual);
  return { coverage, visual };
}

// Raw strict visual failures are never edited. A separate, exact binding can
// record a human-reviewed acceptance for one candidate/reference/state and its
// screenshot, diff, and bounded text/geometry evidence. Any stale or duplicate
// record fails closed instead of turning into a broad waiver. The reviewed
// commit is provenance; current approval matches the runtime binary fingerprint
// and exact screenshot/diff/evidence hashes to avoid a self-referential commit.
export function assessVisualAcceptances(
  report,
  comparator,
  rawFailures,
  registry = VISUAL_ACCEPTANCE,
) {
  const errors = registryErrors(registry),
    pending = [],
    accepted = [],
    entries = registry?.acceptances;
  if (!Array.isArray(entries))
    return { errors: ['Invalid visual acceptance registry'], pending, accepted, pass: false };

  const rows = [
    ...(report.results ?? []).map((row) => ({ kind: 'frame', row })),
    ...(report.regions ?? []).map((row) => ({ kind: 'region', row })),
  ];
  const failures = rows.filter(({ row }) => row && (row.size !== null || row.ratio > MAX_RATIO));
  const scope = report.scope ?? report.mode ?? 'required';
  const used = new Set();

  for (const { kind, row } of failures) {
    const binding = {
      comparator,
      scope,
      key: row.key,
      candidateFingerprint: report.candidateFingerprint,
      reference: report.reference,
      referenceHub: report.referenceHub ?? null,
      artifactDigests: row.artifactDigests,
    };
    const candidates = entries
      .map((entry, index) => ({ entry, index }))
      .filter(
        ({ entry }) =>
          entry &&
          entry.comparator === comparator &&
          entry.scope === scope &&
          entry.key === row.key &&
          entry.kind === kind,
      );
    const label = `${comparator}/${scope}/${kind}/${row.key}`;
    if (candidates.length !== 1) {
      pending.push(label);
      if (candidates.length > 1) errors.push(`Duplicate acceptance records: ${label}`);
      continue;
    }
    const { entry, index } = candidates[0];
    const exact =
      entry.candidateFingerprint === binding.candidateFingerprint &&
      entry.reference === binding.reference &&
      (entry.referenceHub ?? null) === binding.referenceHub &&
      sameArtifactDigests(entry.artifactDigests, binding.artifactDigests);
    if (!exact) {
      pending.push(label);
      errors.push(`Acceptance is stale, incomplete, or unreviewed: ${label}`);
      continue;
    }
    accepted.push(label);
    used.add(index);
  }
  entries.forEach((entry, index) => {
    if (
      entry?.comparator === comparator &&
      entry?.scope === scope &&
      !used.has(index) &&
      !failures.some(({ kind, row }) => entry?.kind === kind && entry?.key === row.key)
    )
      errors.push(
        `Unused visual acceptance record: ${entry?.comparator ?? 'unknown'}/${entry?.scope ?? 'unknown'}/${entry?.key ?? 'unknown'}`,
      );
  });

  const rawVisualCount = rawFailures?.visual?.length ?? 0;
  if (rawVisualCount !== failures.length)
    errors.push('Raw visual failure rows do not match strict measurements');
  return {
    errors,
    pending,
    accepted,
    pass:
      (rawFailures?.coverage?.length ?? 0) === 0 &&
      pending.length === 0 &&
      errors.length === 0 &&
      accepted.length === failures.length,
  };
}

// Do not trust a report's `pass` flag or its row count: duplicate/missing cases,
// non-finite measurements, stale provenance and size changes must fail closed.
export function assessPreservation(report) {
  const coverage = [],
    visual = [],
    seen = new Set(),
    historical = report.mode === 'historical-before' && report.candidate === BEFORE,
    expected = historical ? HISTORICAL_EXPECTED : EXPECTED;
  if (report.reference !== REFERENCE.source) coverage.push('Wrong selected Source reference');
  if (report.referenceHub !== REFERENCE.hub) coverage.push('Wrong selected Hub provenance');
  if (!/^[a-f0-9]{40}$/.test(report.candidate ?? '')) coverage.push('Missing candidate commit SHA');
  if (!/^[a-f0-9]{64}$/.test(report.candidateFingerprint ?? ''))
    coverage.push('Missing candidate runtime fingerprint');
  if (!['required', 'historical-before'].includes(report.mode))
    coverage.push('Unknown preservation mode');
  if (report.mode === 'historical-before' && report.candidate !== BEFORE)
    coverage.push('Historical preservation mode requires the pinned before candidate');
  if (report.threshold !== THRESHOLD || report.maxRatio !== MAX_RATIO)
    coverage.push('Changed visual thresholds');
  if (!Array.isArray(report.errors) || report.errors.length)
    coverage.push(...(report.errors ?? ['Missing execution-error report']));
  if (!Array.isArray(report.results)) {
    coverage.push('Missing comparison results');
    return { coverage, visual };
  }
  for (const row of report.results) {
    if (!row || typeof row.key !== 'string') {
      coverage.push('Invalid visual comparison row');
      continue;
    }
    if (!expected.includes(row.key)) coverage.push('Unknown visual case: ' + row.key);
    if (seen.has(row.key)) coverage.push('Duplicate visual case: ' + row.key);
    seen.add(row.key);
    if (
      !Number.isSafeInteger(row.pixels) ||
      row.pixels < 0 ||
      !Number.isFinite(row.ratio) ||
      row.ratio < 0 ||
      row.ratio > 1 ||
      !Object.hasOwn(row, 'size')
    ) {
      coverage.push('Invalid visual measurement: ' + row.key);
      continue;
    }
    if (!validArtifactDigests(row.artifactDigests))
      coverage.push('Missing or invalid visual artifact digests: ' + row.key);
    if (row.size !== null || row.ratio > MAX_RATIO)
      visual.push(`${row.key}: ${row.size ? 'size ' + row.size : 'pixel mismatch ' + row.ratio}`);
  }
  for (const key of expected) if (!seen.has(key)) coverage.push('Missing visual case: ' + key);
  return { coverage, visual };
}

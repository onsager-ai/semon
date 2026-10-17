// The selected complete application is independent of both the candidate and
// derived pen/mockup exports. Keep its identity and required coverage together.
export const REFERENCE = {
  source: '263a02da278716ab2309b8e34a463419e5c781e5',
  hub: '503fbe090749ea1d58b9ca9fa4ccae7289ee426e',
};
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
export const EXPECTED = WIDTHS.flatMap((width) =>
  SCHEMES.flatMap((scheme) =>
    [...STATES, ...(width === 390 ? ['drawer'] : [])].map((state) => `${width}-${scheme}-${state}`),
  ),
);

// Do not trust a report's `pass` flag or its row count: duplicate/missing cases,
// non-finite measurements, stale provenance and size changes must fail closed.
export function assessPreservation(report) {
  const coverage = [],
    visual = [],
    seen = new Set();
  if (report.reference !== REFERENCE.source) coverage.push('Wrong selected Source reference');
  if (report.threshold !== THRESHOLD || report.maxRatio !== MAX_RATIO)
    coverage.push('Changed visual thresholds');
  if (!Array.isArray(report.errors) || report.errors.length)
    coverage.push(...(report.errors ?? ['Missing execution-error report']));
  if (!Array.isArray(report.results)) {
    coverage.push('Missing comparison results');
    return { coverage, visual };
  }
  for (const row of report.results) {
    if (!EXPECTED.includes(row.key)) coverage.push('Unknown visual case: ' + row.key);
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
    if (row.size !== null || row.ratio > MAX_RATIO)
      visual.push(`${row.key}: ${row.size ? 'size ' + row.size : 'pixel mismatch ' + row.ratio}`);
  }
  for (const key of EXPECTED) if (!seen.has(key)) coverage.push('Missing visual case: ' + key);
  return { coverage, visual };
}

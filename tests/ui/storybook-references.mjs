import { createHash } from 'node:crypto';

export const captureIdentityDigest = (item) => createHash('sha256').update(JSON.stringify(item)).digest('hex');

// Runtime equivalent of the checked-in JSON schema; no new schema dependency.
export function referenceErrors(map, plan) {
  const errors = [], byKey = new Map(plan.map((item) => [item.key, item]));
  const text = (value) => typeof value === 'string' && value.trim().length > 0;
  const digest = (value) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
  const allowed = new Set(['decision', 'path', 'sha256', 'storyId', 'interactionId', 'width', 'scheme', 'threshold', 'maxRatio', 'sourceDigest', 'captureDigest', 'reviewedRevision', 'reviewedBy', 'reviewedAt', 'rationale', 'provenance']);
  if (map?.version !== 2 || map.$schema !== './storybook-reference-map.schema.json' || !text(map.pending) || !map.approved || Array.isArray(map.approved) || typeof map.approved !== 'object')
    return ['Invalid provenance-bound Storybook reference map'];
  if (Object.keys(map).some((key) => !['$schema', 'version', 'approved', 'pending'].includes(key))) errors.push('Unknown reference map field');
  for (const [key, record] of Object.entries(map.approved)) {
    const item = byKey.get(key);
    if (!item) { errors.push(`unknown approved capture ${key}`); continue; }
    if (!record || typeof record !== 'object' || Array.isArray(record)) { errors.push(`invalid reference ${key}`); continue; }
    if (Object.keys(record).some((field) => !allowed.has(field)) ||
        record.decision !== 'accepted' || !/^baselines\/storybook\/(?!.*(?:^|\/)\.\.(?:\/|$)).+\.png$/.test(record.path ?? '') ||
        record.storyId !== item.storyId || (record.interactionId ?? null) !== (item.interactionId ?? null) ||
        record.width !== item.width || record.scheme !== item.scheme ||
        record.threshold !== 0.1 || record.maxRatio !== 0.0001 ||
        !['sha256', 'sourceDigest', 'captureDigest'].every((field) => digest(record[field])) ||
        !/^[a-f0-9]{40}$/.test(record.reviewedRevision ?? '') ||
        !text(record.reviewedBy) || !text(record.rationale) || !text(record.reviewedAt) || !Number.isFinite(Date.parse(record.reviewedAt)) ||
        !record.provenance || Object.keys(record.provenance).some((field) => !['source', 'state', 'geometry'].includes(field)) ||
        !['source', 'state', 'geometry'].every((field) => text(record.provenance[field])))
      errors.push(`invalid/unreviewed reference provenance ${key}`);
  }
  return errors;
}

// Historical reference provenance and current candidate provenance are distinct.
// A source edit is allowed to reach the pixel comparator with the same capture
// contract; it neither rewrites nor invalidates a valid historical review.
export function referenceComparisonErrors(approval, item, referenceBytes, candidateProvenance) {
  const errors = referenceErrors({
    $schema: './storybook-reference-map.schema.json', version: 2,
    pending: 'Unapproved captures remain pending', approved: { [item.key]: approval },
  }, [item]);
  if (errors.length) return errors;
  if (!/^[a-f0-9]{64}$/.test(candidateProvenance?.sourceDigest ?? '')) errors.push('Invalid current candidate source digest');
  if (candidateProvenance?.sourceRevision !== null && !/^[a-f0-9]{40}$/.test(candidateProvenance?.sourceRevision ?? '')) errors.push('Invalid current candidate revision');
  if (approval.captureDigest !== captureIdentityDigest(item)) errors.push('Approved reference capture identity changed');
  const imageDigest = createHash('sha256').update(referenceBytes).digest('hex');
  if (imageDigest !== approval.sha256) errors.push('Approved reference digest changed');
  return errors;
}

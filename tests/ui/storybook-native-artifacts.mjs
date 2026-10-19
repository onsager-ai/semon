import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';

// Layout of the existing .github/workflows/ui.yml uploads, downloaded without
// merging artifact directories. Native comparators keep their original paths.
export function nativeArtifactPaths(root, item) {
  const [comparator, scope, kind, ...rest] = item.legacyKey.split(':');
  const key = rest.join(':');
  let directory, stem;
  if (comparator === 'gallery') {
    directory = path.join(root, 'viewer-ui-interaction/gallery');
    stem = key;
  } else if (comparator === 'preservation') {
    directory = path.join(root, 'viewer-ui-visual/preservation');
    stem = key + '-candidate';
  } else if (comparator === 'pixels') {
    const prefix = scope === 'main' ? 'viewer-ui-visual' : 'viewer-ui-transport/' + scope;
    directory = path.join(root, prefix, 'pixels');
    stem = key.replace(/[^a-zA-Z0-9_-]+/g, '_') + '-candidate';
  } else throw Error('Unknown native artifact family ' + comparator);
  return {
    image: path.join(directory, stem + '.png'),
    witness: path.join(directory, stem + '-presentation.json'),
    report:
      comparator === 'gallery'
        ? path.join(directory, 'visual-report.json')
        : path.join(directory, 'report.json'),
    comparator,
    scope,
    kind,
    key,
  };
}

export function nativeWitness(root, item) {
  if (!root) throw Error('SEMON_STORYBOOK_NATIVE_ARTIFACT_ROOT is required before legacy capture');
  const paths = nativeArtifactPaths(root, item);
  const record = JSON.parse(fs.readFileSync(paths.witness, 'utf8'));
  assert.deepEqual(Object.keys(record).sort(), ['imageSha256', 'presentation', 'sourceRevision']);
  assert.equal(
    record.sourceRevision,
    process.env.GITHUB_SHA,
    'Native presentation witness is from another revision',
  );
  assert.equal(
    record.imageSha256,
    createHash('sha256').update(fs.readFileSync(paths.image)).digest('hex'),
    'Native witness/image binding changed',
  );
  return { paths, value: record.presentation };
}

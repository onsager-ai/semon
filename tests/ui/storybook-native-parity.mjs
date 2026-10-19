// Current-head migration parity over original native PNG artifacts. The pinned
// native comparators remain independent required gates with their original policy.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import { nativeWitness } from './storybook-native-artifacts.mjs';
import { witnessDifferences } from './storybook-presentation-witness.mjs';
import { captureIdentityDigest } from './storybook-references.mjs';

assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Native migration parity is CI-only');
const here = path.dirname(fileURLToPath(import.meta.url));
const output = path.resolve(process.env.SEMON_UI_OUT ?? path.join(here, 'out'));
const out = path.join(output, 'storybook/native-parity');
fs.mkdirSync(out, { recursive: true });
const read = (file) => JSON.parse(fs.readFileSync(file, 'utf8'));
const capture = read(path.join(output, 'storybook/candidates/capture-report.json'));
const contract = read(path.join(output, 'storybook/contract-report.json'));
assert.equal(capture.sourceRevision, process.env.GITHUB_SHA);
const plan = contract.executableStoryCaptures.filter((item) => item.legacyKey);
assert.ok(plan.length > 0);
assert.equal(new Set(plan.map((item) => item.legacyKey)).size, plan.length);
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const results = [], failures = [];
for (const item of plan) {
  const key = item.legacyKey, stem = sha256(key).slice(0, 20);
  const result = { key, captureDigest: captureIdentityDigest(item), pass: false };
  try {
    const storyRows = capture.results.filter((row) => row.key === item.key);
    assert.equal(storyRows.length, 1, 'One exact story capture required');
    const story = storyRows[0];
    assert.ok(story.rendered && story.candidate && story.presentationWitness, 'Missing rendered Storybook evidence');
    assert.equal(story.captureDigest, result.captureDigest, 'Capture identity changed');
    const native = nativeWitness(process.env.SEMON_STORYBOOK_NATIVE_ARTIFACT_ROOT, item);
    const nativeReport = read(native.paths.report);
    if (native.paths.comparator !== 'gallery') {
      assert.equal(nativeReport.candidate, process.env.GITHUB_SHA, 'Native evidence is from another revision');
      const rows = (native.paths.kind === 'region' ? nativeReport.regions : nativeReport.results).filter((row) => row.key === native.paths.key);
      assert.equal(rows.length, 1, 'Missing or duplicate native exact-key result');
      const nativeBytes = fs.readFileSync(native.paths.image);
      assert.equal(rows[0].artifactDigests.candidate.image, sha256(nativeBytes), 'Native artifact integrity changed');
      result.nativeBinarySha256 = nativeReport.candidateFingerprint;
    } else {
      assert.equal(nativeReport.results.filter((row) => row.key === native.paths.key).length, 1, 'Missing exact gallery result');
    }
    const nativeBytes = fs.readFileSync(native.paths.image), storyBytes = fs.readFileSync(path.resolve(here, story.candidate));
    assert.equal(story.sha256, sha256(storyBytes), 'Story artifact integrity changed');
    const storyWitness = read(path.resolve(here, story.presentationWitness));
    const differences = witnessDifferences(native.value, storyWitness);
    const diff = compare(PNG.sync.read(nativeBytes), PNG.sync.read(storyBytes), item.pixelPolicy.threshold);
    fs.writeFileSync(path.join(out, stem + '-native.png'), nativeBytes);
    fs.writeFileSync(path.join(out, stem + '-story.png'), storyBytes);
    fs.writeFileSync(path.join(out, stem + '-diff.png'), PNG.sync.write(diff.diff));
    fs.writeFileSync(path.join(out, stem + '-native.json'), JSON.stringify(native.value, null, 2) + '\n');
    fs.writeFileSync(path.join(out, stem + '-story.json'), JSON.stringify(storyWitness, null, 2) + '\n');
    Object.assign(result, { artifact: stem, nativeSha256: sha256(nativeBytes), storySha256: sha256(storyBytes),
      ratio: diff.ratio, size: diff.size, threshold: item.pixelPolicy.threshold, maxRatio: item.pixelPolicy.maxRatio,
      presentationDifferences: differences, pass: !diff.size && diff.ratio <= item.pixelPolicy.maxRatio && differences.length === 0 });
    if (!result.pass) failures.push(key);
  } catch (error) { result.error = error.message; failures.push(key + ': ' + error.message); }
  results.push(result);
}
const report = { version: 1, candidateOnly: true, approvedReferencesWritten: false,
  sourceRevision: process.env.GITHUB_SHA, expectedKeys: plan.map((item) => item.legacyKey), results, failures,
  normalizations: ['Loopback URL origin and runtime-only DOM owner IDs are outside the declared presentation schema. No text, order, source identity, disclosure state or geometry normalization.'],
};
fs.writeFileSync(path.join(out, 'report.json'), JSON.stringify(report, null, 2) + '\n');
if (failures.length) { console.error(`Native Storybook migration parity: ${failures.length}/${plan.length} failed; see native-parity/report.json`); process.exitCode = 1; }
else console.log(`Native Storybook migration parity: ${plan.length} exact keys passed; references remain unapproved`);

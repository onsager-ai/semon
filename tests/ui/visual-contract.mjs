import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';

// Baselines are deliberate, reviewable source artifacts. CI never creates them.
export function createVisualContract({ baselineDir, outDir, manifest, update = false }) {
  if (update && process.env.CI) throw new Error('CI cannot update visual baselines');
  const expected = new Set(manifest.states.flatMap(state => manifest.widths.flatMap(width => manifest.schemes.map(scheme => `${state}-${width}-${scheme}`))));
  if (!expected.size || expected.size !== manifest.states.length * manifest.widths.length * manifest.schemes.length) throw new Error('Empty or duplicate visual coverage');
  if (!(manifest.threshold >= 0 && manifest.threshold <= 1) || !(manifest.maxRatio >= 0 && manifest.maxRatio <= 1)) throw new Error('Invalid visual thresholds');
  const seen = new Set(), results = [], failures = [];
  fs.mkdirSync(outDir, { recursive: true });
  return {
    async capture(page, key, options = {}) {
      if (!expected.has(key) || seen.has(key)) throw new Error('Unknown or duplicate visual case: ' + key);
      seen.add(key);
      await page.evaluate(() => document.fonts.ready);
      const actualPath = path.join(outDir, key + '.png'), baselinePath = path.join(baselineDir, key + '.png');
      const bytes = await page.screenshot({ ...options, path: actualPath, animations: 'disabled' });
      if (update) { fs.mkdirSync(baselineDir, { recursive: true }); fs.writeFileSync(baselinePath, bytes); results.push({ key, updated: true }); return; }
      if (!fs.existsSync(baselinePath)) { failures.push('Missing baseline: ' + key); results.push({ key, missing: true }); return; }
      const result = compare(PNG.sync.read(fs.readFileSync(baselinePath)), PNG.sync.read(bytes), manifest.threshold);
      results.push({ key, ratio: result.ratio, size: result.size });
      if (result.size || result.ratio > manifest.maxRatio) {
        failures.push(`${key}: ${result.size ? 'size ' + result.size.join(' -> ') : 'pixel mismatch ' + (100 * result.ratio).toFixed(3) + '%'}`);
        fs.writeFileSync(path.join(outDir, key + '-diff.png'), PNG.sync.write(result.diff));
      }
    },
    finish() {
      for (const key of expected) if (!seen.has(key)) failures.push('Missing visual coverage: ' + key);
      fs.writeFileSync(path.join(outDir, 'visual-report.json'), JSON.stringify({ results, failures }, null, 2) + '\n');
      return failures;
    },
  };
}

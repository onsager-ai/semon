import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { PNG } from 'pngjs';
import { createVisualContract } from './visual-contract.mjs';

test('visual gate rejects missing cases, missing images, geometry changes and pixel changes', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'visual-contract-'));
  const baselineDir = path.join(dir, 'baseline'),
    outDir = path.join(dir, 'out');
  const manifest = {
    states: ['page'],
    widths: [390],
    schemes: ['light'],
    threshold: 0.1,
    maxRatio: 0.001,
  };
  const key = 'page-390-light';
  const png = (width, color) => {
    const image = new PNG({ width, height: 10 });
    for (let i = 0; i < image.data.length; i += 4) image.data.set([color, color, color, 255], i);
    return PNG.sync.write(image);
  };
  const page = (bytes) => ({
    evaluate: async () => {},
    screenshot: async (options) => {
      fs.writeFileSync(options.path, bytes);
      return bytes;
    },
  });
  const gate = () => createVisualContract({ baselineDir, outDir, manifest });
  try {
    assert.match(gate().finish().join(), /Missing visual coverage/);
    const missing = gate();
    await missing.capture(page(png(10, 255)), key);
    assert.match(missing.finish().join(), /Missing baseline/);
    fs.mkdirSync(baselineDir);
    fs.writeFileSync(path.join(baselineDir, key + '.png'), png(10, 255));
    for (const [bytes, pattern] of [
      [png(10, 255), null],
      [png(11, 255), /size/],
      [png(10, 0), /pixel mismatch/],
    ]) {
      const run = gate();
      await run.capture(page(bytes), key);
      const failures = run.finish();
      if (pattern) assert.match(failures.join(), pattern);
      else assert.deepEqual(failures, []);
      await assert.rejects(run.capture(page(bytes), key), /duplicate/);
    }
    assert.throws(
      () => createVisualContract({ baselineDir, outDir, manifest: { ...manifest, states: [] } }),
      /Empty/,
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

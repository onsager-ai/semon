// Review evidence differences; this never changes a baseline or a CI tolerance.
import fs from 'node:fs/promises';
import path from 'node:path';
import { PNG } from '../../../../tests/ui/node_modules/pngjs/lib/png.js';
import pixelmatch from '../../../../tests/ui/node_modules/pixelmatch/index.js';
const root = process.argv[2] ?? new URL('.', import.meta.url).pathname;
await fs.mkdir(path.join(root, 'diff'), { recursive: true });
const pairs = [];
for (const name of (await fs.readdir(path.join(root, 'before'))).filter(name => name.endsWith('.png')).sort()) {
  const a = PNG.sync.read(await fs.readFile(path.join(root, 'before', name)));
  const b = PNG.sync.read(await fs.readFile(path.join(root, 'after', name)));
  if (a.width !== b.width || a.height !== b.height) throw Error(`Unmatched dimensions: ${name}`);
  const diff = new PNG({ width: a.width, height: a.height });
  const changed = pixelmatch(a.data, b.data, diff.data, a.width, a.height, { threshold: 0.1 });
  await fs.writeFile(path.join(root, 'diff', name), PNG.sync.write(diff));
  pairs.push({ name, width: a.width, height: a.height, changedPixels: changed, percent: Number((100 * changed / (a.width * a.height)).toFixed(4)) });
}
await fs.writeFile(path.join(root, 'pixel-review.json'), JSON.stringify({ threshold: 0.1, purpose: 'Before/after implementation review; not an approved baseline comparison', pairs }, null, 2) + '\n');
console.log(`${pairs.length} matched before/after pixel diffs written without baseline changes`);

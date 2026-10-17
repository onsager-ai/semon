// CI-only diagnosis of an immutable failed artifact. This does not assess acceptance.
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { PNG } from 'pngjs';
import { compare } from '../pixel-diff.mjs';

const root = process.argv[2];
const inputs = ['candidate', 'reference'].map((side) => {
  const bytes = fs.readFileSync(path.join(root, side + '.png'));
  const digest = createHash('sha256').update(bytes).digest('hex');
  const expected = {
    candidate: '32f5203d17f35320bea4f9723de43804246632d81bba1005991b2b23492824be',
    reference: '8f3335d3abf0b2810c20d97b4d7daf5d764e9408bab3a26137d4a0750dd7d7ba',
  };
  if (digest !== expected[side]) throw new Error('Retained ' + side + ' image digest changed');
  return {
    side,
    sha256: digest,
    image: PNG.sync.read(bytes),
  };
});
const [candidate, reference] = inputs.map((input) => input.image);
if (candidate.width !== reference.width || candidate.height !== reference.height)
  throw new Error('The retained capture dimensions differ');
const { width, height } = candidate;
const raw = compare(candidate, reference, 0.1);
const color = (image, x, y) => {
  const offset = (y * width + x) * 4;
  return [...image.data.subarray(offset, offset + 4)].join(',');
};
const pairs = new Map(),
  tiles = new Map(),
  palettes = [new Map(), new Map()];
for (let y = 0; y < height; y++) {
  for (let x = 0; x < width; x++) {
    const a = color(candidate, x, y),
      b = color(reference, x, y);
    palettes[0].set(a, (palettes[0].get(a) ?? 0) + 1);
    palettes[1].set(b, (palettes[1].get(b) ?? 0) + 1);
    if (a === b) continue;
    const pair = a + ' -> ' + b;
    pairs.set(pair, (pairs.get(pair) ?? 0) + 1);
    const tile = Math.floor(x / 32) * 32 + ',' + Math.floor(y / 64) * 64;
    tiles.set(tile, (tiles.get(tile) ?? 0) + 1);
  }
}
const rank = (map, count = 30) => [...map].sort((a, b) => b[1] - a[1]).slice(0, count);
// Informational translation scores over the retained difference's bounding box.
// The original raw comparison above remains intact; no translated image is accepted.
const translations = [];
for (let dy = -4; dy <= 4; dy++) {
  for (let dx = -4; dx <= 4; dx++) {
    let compared = 0,
      changed = 0,
      absoluteError = 0;
    for (let y = 224; y < Math.min(height - 4, 1913); y++) {
      for (let x = 32; x < Math.min(width - 4, 747); x++) {
        const a = (y * width + x) * 4,
          b = ((y + dy) * width + x + dx) * 4;
        let delta = 0;
        for (let channel = 0; channel < 3; channel++)
          delta += Math.abs(candidate.data[a + channel] - reference.data[b + channel]);
        absoluteError += delta;
        changed += delta > 30 ? 1 : 0;
        compared++;
      }
    }
    translations.push({ dx, dy, compared, changed, absoluteError });
  }
}
console.log(
  JSON.stringify(
    {
      purpose: 'Immutable failed-attempt pixel diagnosis only; not acceptance or a retry',
      run: 39074185832,
      artifact: 12116701987,
      key: 'phone-dark|session-harbor',
      inputs: inputs.map(({ side, sha256 }) => ({ side, sha256 })),
      width,
      height,
      raw: { pixels: raw.pixels, ratio: raw.ratio, size: raw.size, difference: raw.difference },
      dominantColorPairs: rank(pairs, 50),
      changedTiles: rank(tiles, 40),
      candidatePalette: rank(palettes[0]),
      referencePalette: rank(palettes[1]),
      translations: translations.sort((a, b) => a.changed - b.changed).slice(0, 15),
      zeroTranslation: translations.find(({ dx, dy }) => dx === 0 && dy === 0),
    },
    null,
    2,
  ),
);

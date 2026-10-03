import assert from 'node:assert/strict';
import test from 'node:test';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';

function solid(width, height, channel) {
  const image = new PNG({ width, height });
  for (let i = 0; i < image.data.length; i += 4) {
    image.data[i] = image.data[i + 1] = image.data[i + 2] = channel;
    image.data[i + 3] = 255;
  }
  return image;
}

test('identical dark content at different heights has no pixel regression but retains its size change', () => {
  const result = compare(solid(20, 10, 20), solid(20, 30, 20), 0.1);
  assert.equal(result.ratio, 0);
  assert.equal(result.pixels, 0);
  assert.deepEqual(result.size, ['20×10', '20×30']);
  assert.equal(result.A.height, 10);
  assert.equal(result.B.height, 30);
});

test('extra height cannot dilute a difference in shared content', () => {
  const result = compare(solid(20, 10, 20), solid(20, 30, 240), 0.1);
  assert.equal(result.ratio, 1);
  assert.equal(result.pixels, 200);
});

test('different widths remain a size regression even when shared pixels agree', () => {
  const result = compare(solid(20, 10, 20), solid(30, 10, 20), 0.1);
  assert.equal(result.ratio, 0);
  assert.deepEqual(result.size, ['20×10', '30×10']);
});

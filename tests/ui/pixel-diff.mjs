import { PNG } from 'pngjs';
import pixelmatch from 'pixelmatch';

// Compare shared content; missing rows and columns are a geometry change,
// reported separately through size. White padding would count dark missing
// rows as content differences and make the ratio depend on the page's height.
export function compare(a, b, threshold) {
  const width = Math.min(a.width, b.width),
    height = Math.min(a.height, b.height);
  const crop = (image) => {
    const out = new PNG({ width, height });
    PNG.bitblt(image, out, 0, 0, width, height, 0, 0);
    return out;
  };
  const left = crop(a),
    right = crop(b),
    diff = new PNG({ width, height });
  const pixels = pixelmatch(left.data, right.data, diff.data, width, height, {
    threshold,
    includeAA: false,
  });
  // Diagnostic coordinates of the existing red mismatch pixels. This does not
  // change the comparison, crop, anti-alias rule, threshold or acceptance ratio.
  let bounds = null;
  const bands = new Map();
  if (pixels) {
    let minX = width,
      minY = height,
      maxX = -1,
      maxY = -1;
    for (let y = 0; y < height; y++)
      for (let x = 0; x < width; x++) {
        const at = (y * width + x) * 4;
        if (diff.data[at] !== 255 || diff.data[at + 1] !== 0 || diff.data[at + 2] !== 0) continue;
        minX = Math.min(minX, x);
        maxX = Math.max(maxX, x);
        minY = Math.min(minY, y);
        maxY = Math.max(maxY, y);
        const band = Math.floor(y / 64) * 64;
        bands.set(band, (bands.get(band) ?? 0) + 1);
      }
    if (maxX >= 0) bounds = [minX, minY, maxX - minX + 1, maxY - minY + 1];
  }
  return {
    pixels,
    ratio: pixels / (width * height),
    difference: {
      bounds,
      bands: [...bands]
        .sort((a, b) => b[1] - a[1])
        .slice(0, 16)
        .map(([y, pixels]) => ({ y, height: Math.min(64, height - y), pixels })),
    },
    size:
      a.width === b.width && a.height === b.height
        ? null
        : [a.width + '×' + a.height, b.width + '×' + b.height],
    diff,
    A: a,
    B: b,
  };
}

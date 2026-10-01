import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";

// Compare shared content; missing rows and columns are a geometry change,
// reported separately through size. White padding would count dark missing
// rows as content differences and make the ratio depend on the page's height.
export function compare(a, b, threshold) {
  const width = Math.min(a.width, b.width), height = Math.min(a.height, b.height);
  const crop = (image) => {
    const out = new PNG({ width, height });
    PNG.bitblt(image, out, 0, 0, width, height, 0, 0);
    return out;
  };
  const left = crop(a), right = crop(b), diff = new PNG({ width, height });
  const pixels = pixelmatch(left.data, right.data, diff.data, width, height, { threshold, includeAA: false });
  return {
    pixels, ratio: pixels / (width * height),
    size: a.width === b.width && a.height === b.height ? null : [a.width + "×" + a.height, b.width + "×" + b.height],
    diff, A: a, B: b,
  };
}

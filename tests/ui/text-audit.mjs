// Shared rendered-text contract. Keep this independent of viewer routes and fixtures
// so consumers can audit their own production pages and open overlays.
export async function auditText(page) {
  return page.evaluate(() => {
    const canvas = document.createElement('canvas').getContext('2d', { willReadFrequently: true });
    const rgba = css => { canvas.clearRect(0, 0, 1, 1); canvas.fillStyle = '#000'; canvas.fillStyle = css; canvas.fillRect(0, 0, 1, 1); const [r, g, b, a] = canvas.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
    const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return a ? [0, 1, 2].map(i => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a) : [0, 0, 0, 0]; };
    const backdrop = node => {
      const layers = []; for (let e = node; e; e = e.parentElement) layers.push(rgba(getComputedStyle(e).backgroundColor));
      let c = rgba(getComputedStyle(document.documentElement).getPropertyValue('--ground') || '#fff'); if (c[3] < 1) c = [255, 255, 255, 1];
      for (const layer of layers.reverse()) c = over(layer, c);
      return c;
    };
    const lum = ([r, g, b]) => { const f = v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
    const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
    const drawn = e => {
      const box = e.getBoundingClientRect(); if (box.width < 2 || box.height < 2) return false;
      for (let a = e; a; a = a.parentElement) { const cs = getComputedStyle(a); if (cs.display === 'none' || cs.visibility === 'hidden') return false; }
      return true;
    };
    const opacityOf = e => { let o = 1; for (let a = e; a; a = a.parentElement) o *= parseFloat(getComputedStyle(a).opacity); return o; };
    const seen = new Set(), small = [], low = [];
    let texts = 0;
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.nodeValue.trim(), e = node.parentElement;
      if (!text || !/[\p{L}\p{N}]/u.test(text) || !e || e.closest("svg, script, style, title, [disabled], [aria-disabled='true']") || !drawn(e)) continue;
      const cs = getComputedStyle(e), size = parseFloat(cs.fontSize), weight = Number(cs.fontWeight);
      texts++;
      const name = e.tagName.toLowerCase() + (e.className && typeof e.className === 'string' ? '.' + e.className.trim().split(/\s+/).join('.') : '');
      // Different instances can have different surfaces. Do not deduplicate solely
      // by class and text, or a compliant copy could hide a failing one.
      const bg = backdrop(e), fg0 = rgba(cs.color), fg = over([fg0[0], fg0[1], fg0[2], fg0[3] * opacityOf(e)], bg);
      const key = [name, text.slice(0, 24), size, weight, ...bg, ...fg].join('|');
      if (seen.has(key)) continue; seen.add(key);
      if (size < 12) small.push({ el: name, text: text.slice(0, 40), size });
      const large = size >= 24 || (size >= 18.66 && weight >= 700), need = large ? 3 : 4.5, got = ratio(fg, bg);
      if (got < need) low.push({ el: name, text: text.slice(0, 40), ratio: Math.round(got * 100) / 100, need, size });
    }
    return { texts, small: small.slice(0, 12), smallCount: small.length, low: low.slice(0, 12), lowCount: low.length };
  });
}

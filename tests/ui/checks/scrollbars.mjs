// Scrollbars are soft: a low-contrast tint of --ink on a transparent track, not the browser's default.
//   - the page scroller, the sidebar and a code block (and a table that scrolls sideways) compute `scrollbar-color` equal to
//     --scroll-thumb on a transparent track, and `scrollbar-width: thin`, in light and dark, at 390 and 1280 px.
//   - the thumb, composited over the surface it sits on, is below 2:1 against it and at least 1.3:1 (soft, not invisible);
//     both bounds are asserted.
//   - --scroll-thumb-hover is a little stronger than --scroll-thumb.
// Playwright's headless Chromium hides scrollbars, so this check opens its own browser with them shown, to screenshot a
// long session and the sidebar in out/scrollbars/ for the visual pass. Where the platform draws overlay scrollbars the
// shot shows none; the computed styles above are still asserted.
import fs from "node:fs";
import path from "node:path";
import { chromium } from "playwright";
import { ENV, served, goto, data, reporter } from "../lib.mjs";

const OUT = path.join(ENV.out, "scrollbars");
fs.mkdirSync(OUT, { recursive: true });

export default async function scrollbarsCheck() {
  const browser = await chromium.launch({ ignoreDefaultArgs: ["--hide-scrollbars"], args: ["--disable-gpu", "--font-render-hinting=none"] });
  const D = await data({ extras: true }), r = reporter("scrollbars"), results = {};
  const tallest = Object.values(D.SESS).sort((a, b) => (D.TX[b.id]?.length ?? 0) - (D.TX[a.id]?.length ?? 0))[0];
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = size + (dark ? "-dark" : "-light"), page = await served(browser, { size, dark, extras: true });
    await goto(page, { v: "session", id: "harbor" }, D);
    const seen = await page.evaluate(() => {
      const parse = (css) => {
        // "rgba(r, g, b, a)" or "color(srgb r g b / a)", either resolved from a colour-mix, as 0-255 channels plus alpha.
        const n = css.match(/-?\d*\.?\d+(?:e-?\d+)?/g).map(Number);
        if (css.startsWith("color(")) return [n[0] * 255, n[1] * 255, n[2] * 255, n[3] ?? 1];
        return [n[0], n[1], n[2], n[3] ?? 1];
      };
      const colours = (css) => (css.match(/(?:rgba?|color)\([^)]*\)/g) ?? []).map(parse);
      const token = (name) => { const p = document.createElement("span"); p.style.color = "var(--" + name + ")"; document.body.append(p); const c = getComputedStyle(p).color; p.remove(); return parse(c); };
      const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return [0, 1, 2].map((i) => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a); };
      const surface = (node) => { let c = [255, 255, 255, 1]; const chain = []; for (let e = node; e; e = e.parentElement) chain.push(parse(getComputedStyle(e).backgroundColor)); for (const layer of chain.reverse()) if (layer[3] > 0) c = over(layer, c); return c; };
      const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
      const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
      const thumb = token("scroll-thumb"), hover = token("scroll-thumb-hover"), near = (a, b) => a.every((v, i) => Math.abs(v - b[i]) < (i === 3 ? 0.02 : 1.5));
      const spots = { page: document.scrollingElement, main: document.querySelector("#main"), sidebar: document.querySelector(".sidebar"), lanes: document.querySelector("#lanes"), code: document.querySelector(".md .codeblock"), table: document.querySelector(".md .tbl") };
      const out = {};
      for (const [name, el] of Object.entries(spots)) {
        if (!el) { out[name] = null; continue; }
        const cs = getComputedStyle(el), [t, track] = colours(cs.scrollbarColor), s = surface(el === document.scrollingElement ? document.body : el);
        out[name] = { color: cs.scrollbarColor, width: cs.scrollbarWidth, thumbIsToken: !!t && near(t, thumb), trackClear: !!track && track[3] === 0, ratio: t ? Math.round(ratio(over(t, s), s) * 100) / 100 : null };
      }
      return { thumb, hover, hoverStronger: hover[3] > thumb[3], spots: out };
    });
    results[tag] = seen;
    r.expect(seen.thumb[3] > 0.1 && seen.thumb[3] < 0.5, tag + ": --scroll-thumb is not a partial tint: " + JSON.stringify(seen.thumb));
    r.expect(seen.hoverStronger, tag + ": --scroll-thumb-hover is not stronger than --scroll-thumb");
    for (const name of ["page", "sidebar", "code"]) r.expect(seen.spots[name] !== null, tag + ": the " + name + " scroller is missing from the fixture");
    for (const [name, s] of Object.entries(seen.spots)) {
      if (!s) continue;
      r.expect(s.thumbIsToken && s.trackClear, tag + " " + name + ": scrollbar-color is " + s.color + ", not --scroll-thumb on a transparent track");
      r.expect(s.width === "thin", tag + " " + name + ": scrollbar-width is " + s.width);
      r.expect(s.ratio !== null && s.ratio < 2 && s.ratio >= 1.3, tag + " " + name + ": the thumb is " + s.ratio + ":1 against its surface, outside 1.3 to 2");
    }
    await page.screenshot({ path: path.join(OUT, "session-" + tag + ".png") });
    await page.locator(".md .codeblock").first().scrollIntoViewIfNeeded().catch(() => {});
    await page.screenshot({ path: path.join(OUT, "code-" + tag + ".png") });
    if (size === "phone") { await goto(page, { v: "home" }, D); await page.click("#lead-btn"); await page.waitForTimeout(320); }
    await page.screenshot({ path: path.join(OUT, "sidebar-" + tag + ".png") });
    if (tallest && size === "desktop") { await goto(page, { v: "session", id: tallest.id }, D); await page.waitForTimeout(200); await page.screenshot({ path: path.join(OUT, "long-" + tag + ".png") }); }
    r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }
  await browser.close();
  r.results = results;
  return r.done();
}

// The visual system's rules for text (docs/design/overhaul.md, principles P3 and P7), on every screen that is drawn, at 390 and
// 1280 px in light and dark:
//   - no text is smaller than 12 px;
//   - every text meets WCAG AA against the colour behind it: 4.5:1, or 3:1 for large text (24 px, or 18.66 px and bold).
// "Faint" is not a text colour, and its ratio is about 2.6:1, so it fails here wherever it is used for words. Text that holds no
// letter or digit (a separator such as "·" or "›") is a rule, not words, and isn't measured. Disabled controls and text that is
// not drawn (hidden, closed drawer, under 2 px) are skipped.
import { served, data, reporter, goto } from "../lib.mjs";

const audit = (page) => page.evaluate(() => {
  const canvas = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgba = (css) => { canvas.clearRect(0, 0, 1, 1); canvas.fillStyle = "#000"; canvas.fillStyle = css; canvas.fillRect(0, 0, 1, 1); const [r, g, b, a] = canvas.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
  const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return a ? [0, 1, 2].map((i) => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a) : [0, 0, 0, 0]; };
  const backdrop = (node) => {
    const layers = []; for (let e = node; e; e = e.parentElement) layers.push(rgba(getComputedStyle(e).backgroundColor));
    let c = rgba(getComputedStyle(document.documentElement).getPropertyValue("--ground") || "#fff"); if (c[3] < 1) c = [255, 255, 255, 1];
    for (const layer of layers.reverse()) c = over(layer, c);
    return c;
  };
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
  const drawn = (e) => {
    const box = e.getBoundingClientRect(); if (box.width < 2 || box.height < 2) return false;
    for (let a = e; a; a = a.parentElement) { const cs = getComputedStyle(a); if (cs.display === "none" || cs.visibility === "hidden") return false; }
    return true;
  };
  const opacityOf = (e) => { let o = 1; for (let a = e; a; a = a.parentElement) o *= parseFloat(getComputedStyle(a).opacity); return o; };
  const seen = new Set(), small = [], low = [];
  let texts = 0;
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node.nodeValue.trim(), e = node.parentElement;
    if (!text || !/[\p{L}\p{N}]/u.test(text) || !e || e.closest("svg, script, style, title, [disabled], [aria-disabled='true']") || !drawn(e)) continue;
    const cs = getComputedStyle(e), size = parseFloat(cs.fontSize), weight = Number(cs.fontWeight);
    texts++;
    const name = e.tagName.toLowerCase() + (e.className && typeof e.className === "string" ? "." + e.className.trim().split(/\s+/).join(".") : "");
    const key = name + "|" + text.slice(0, 24);
    if (seen.has(key)) continue; seen.add(key);
    if (size < 12) small.push({ el: name, text: text.slice(0, 40), size });
    const bg = backdrop(e), fg0 = rgba(cs.color), fg = over([fg0[0], fg0[1], fg0[2], fg0[3] * opacityOf(e)], bg);
    const large = size >= 24 || (size >= 18.66 && weight >= 700), need = large ? 3 : 4.5, got = Math.round(ratio(fg, bg) * 100) / 100;
    if (got < need) low.push({ el: name, text: text.slice(0, 40), ratio: got, need, size });
  }
  return { texts, small: small.slice(0, 12), smallCount: small.length, low: low.slice(0, 12), lowCount: low.length };
});

export default async function tokensCheck(browser) {
  const D = await data(), r = reporter("tokens"), results = {};
  const harbor = D.SESS.harbor ? "harbor" : Object.keys(D.SESS)[0];
  const parent = Object.values(D.SESS).find((s) => Object.values(D.SESS).filter((c) => c.parent === s.id).length >= 5)?.id ?? harbor;
  const turn = D.turns.find((t) => t.sent.length);
  const screens = [
    ["home", { v: "home" }], ["sessions", { v: "sessions" }], ["analytics", { v: "analytics" }], ["machines", { v: "machines" }],
    ["session-" + harbor, { v: "session", id: harbor }], ["session-" + parent, { v: "session", id: parent }],
    ...(turn ? [["trace", { v: "trace", sid: turn.sid, turn: turn.id }]] : []),
  ];
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = size + (dark ? "-dark" : "-light"), rec = (results[tag] = {}), page = await served(browser, { size, dark });
    const judge = (where, a) => {
      rec[where] = { texts: a.texts, small: a.smallCount, low: a.lowCount };
      r.expect(a.texts > 20, tag + " " + where + ": only " + a.texts + " texts were measured");
      r.expect(a.smallCount === 0, tag + " " + where + ": " + a.smallCount + " texts are under 12px: " + JSON.stringify(a.small));
      r.expect(a.lowCount === 0, tag + " " + where + ": " + a.lowCount + " texts are under AA contrast: " + JSON.stringify(a.low));
    };
    for (const [name, route] of screens) {
      await goto(page, route, D);
      await page.waitForTimeout(150);
      judge(name, await audit(page));
    }
    // The phone's navigation drawer is closed everywhere else; open it on Home. On desktop the sidebar is drawn beside every screen.
    if (size === "phone") { await goto(page, { v: "home" }, D); await page.click("#lead-btn"); await page.waitForTimeout(320); judge("drawer", await audit(page)); }
    r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }
  r.results = results;
  return r.done();
}

// The harness is named in plain text, never drawn. Every label (`.hname`) must read "Claude Code", "Claude" or "Codex", be a
// text-only element (no svg, no img), stay on one line at 390 px, and meet WCAG AA (4.5:1) against the colour behind it in
// both colour schemes. Claude and Codex must differ by the word itself. Screenshots of the sidebar, a session's top bar and
// Analytics are written to out/names/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto, overflow } from "../lib.mjs";

const ALLOWED = new Set(["Claude Code", "Claude", "Codex"]);
const OUT = path.join(ENV.out, "names");
fs.mkdirSync(OUT, { recursive: true });

// Reads every visible label in `root`: its text, whether it holds a graphic, its lines, and its contrast ratio.
const measure = (page, root) => page.evaluate((rootSelector) => {
  const canvas = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgba = (css) => { canvas.clearRect(0, 0, 1, 1); canvas.fillStyle = "#000"; canvas.fillStyle = css; canvas.fillRect(0, 0, 1, 1); const [r, g, b, a] = canvas.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
  const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return a ? [0, 1, 2].map((i) => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a) : [0, 0, 0, 0]; };
  const backdrop = (node) => { const layers = []; for (let e = node; e; e = e.parentElement) layers.push(rgba(getComputedStyle(e).backgroundColor)); let c = rgba(getComputedStyle(document.documentElement).getPropertyValue("--ground") || "#fff"); if (c[3] < 1) c = [255, 255, 255, 1]; for (const layer of layers.reverse()) c = over(layer, c); return c; };
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
  return [...document.querySelectorAll(rootSelector + " .hname")].filter((n) => n.getClientRects().length).map((n) => {
    const cs = getComputedStyle(n), box = n.getBoundingClientRect(), lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.2;
    const fg = rgba(cs.color), bg = backdrop(n.parentElement);
    return { text: n.textContent, harness: [...n.classList].find((c) => c.startsWith("h-")), graphics: n.querySelectorAll("svg,img").length + n.children.length, tag: n.tagName, lines: Math.round(box.height / lh), fontSize: parseFloat(cs.fontSize), weight: cs.fontWeight, ratio: Math.round(ratio(fg, over(fg.slice(0, 3).concat(1), bg)) * 100) / 100, fromHidden: n.getAttribute("aria-hidden") === "true" };
  });
}, root);

export default async function namesCheck(browser) {
  const D = await data(), r = reporter("names"), results = {};
  const claude = Object.values(D.SESS).find((s) => s.harness === "claude" && s.lane), codex = Object.values(D.SESS).find((s) => s.harness === "codex");
  r.expect(!!claude && !!codex, "the fixture must hold a Claude and a Codex session");
  const seen = { "Claude Code": 0, Claude: 0, Codex: 0 };

  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = size + (dark ? "-dark" : "-light"), rec = (results[tag] = {}), page = await served(browser, { size, dark });
    const audit = (where, labels) => {
      rec[where] = labels.map((l) => l.text + " " + l.ratio + (l.lines > 1 ? " WRAPS" : ""));
      r.expect(labels.length > 0, tag + " " + where + ": no harness label is shown");
      for (const l of labels) {
        r.expect(ALLOWED.has(l.text), tag + " " + where + ": label reads " + JSON.stringify(l.text));
        r.expect(l.graphics === 0 && l.tag === "SPAN", tag + " " + where + ": a label holds an svg, img or child element");
        r.expect(l.lines <= 1, tag + " " + where + ": " + l.text + " wraps onto " + l.lines + " lines");
        r.expect(l.fontSize >= 11 && l.fontSize <= 12 && Number(l.weight) <= 500, tag + " " + where + ": " + l.text + " is " + l.fontSize + "px weight " + l.weight);
        r.expect(l.ratio >= 4.5, tag + " " + where + ": " + l.text + " has contrast " + l.ratio + ", under 4.5");
        if (seen[l.text] !== undefined) seen[l.text]++;
        r.expect((l.harness === "h-codex") === (l.text === "Codex"), tag + " " + where + ": " + l.text + " carries " + l.harness);
      }
    };
    const shot = (name) => page.screenshot({ path: path.join(OUT, "names-" + tag + "-" + name + ".png") });
    for (const s of [claude, codex]) {
      await goto(page, { v: "session", id: s.id }, D);
      await page.waitForTimeout(200);
      const top = await measure(page, "#topbar");
      if (top.length) audit("topbar-" + s.harness, top); else rec["topbar-" + s.harness] = "dropped by the line fitter";
      if (s === claude) await shot("session");
    }
    await goto(page, { v: "analytics" }, D);
    await page.waitForTimeout(200);
    audit("analytics", await measure(page, ".page"));
    rec.sideways = size === "phone" ? await overflow(page) : 0;
    r.expect(rec.sideways === 0, tag + ": Analytics scrolls sideways (" + rec.sideways + ")");
    await shot("analytics");
    // The sidebar last: on a phone it is the drawer over Home, on desktop it sits beside the open session (its row selected).
    if (size === "phone") { await goto(page, { v: "home" }, D); await page.click("#lead-btn"); await page.waitForTimeout(320); } else { await goto(page, { v: "session", id: claude.id }, D); await page.waitForTimeout(200); }
    audit("sidebar", await measure(page, "#lanes"));
    r.expect(await page.evaluate(() => document.querySelectorAll(".hmark").length === 0 && !/[✳⌘]/.test(document.querySelector("#lanes").textContent + document.querySelector("#topbar").textContent)), tag + ": an old text glyph is still drawn");
    // A sidebar row keeps the harness in its accessible label, so its visible label is hidden from screen readers.
    r.expect(await page.evaluate(() => [...document.querySelectorAll("#lanes .srow-meta .hname")].every((n) => n.getAttribute("aria-hidden") === "true")), tag + ": a sidebar label is not aria-hidden");
    r.expect(await page.evaluate(() => [...document.querySelectorAll("#lanes .srow-meta")].every((m) => { const tops = [...m.children].filter((c) => c.getClientRects().length).map((c) => Math.round(c.getBoundingClientRect().top / 3)); return !tops.length || Math.max(...tops) - Math.min(...tops) <= 1; })), tag + ": a sidebar meta line has more than one line");
    await shot("sidebar");
    r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }
  r.expect(seen.Codex > 0 && seen.Claude + seen["Claude Code"] > 0, "both a Claude and a Codex label must appear");
  r.results = { seen, ...results };
  return r.done();
}

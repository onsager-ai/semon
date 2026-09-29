// The harness is named in plain text, never drawn, and never coloured. Every label (`.hname`) must read "Claude Code", "Claude"
// or "Codex", be a text-only element (no svg, no img), stay on one line at 390 px, meet WCAG AA (4.5:1) against the colour
// behind it in both colour schemes, and be painted in the neutral --muted ink, the same for Claude and Codex, so the word
// itself is what tells them apart. Sidebar rows carry no label (the row's aria-label names the harness), and a top bar's kind
// chip is neutral too. Screenshots of the sidebar, a session's top bar and Analytics are written to out/names/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto, overflow } from "../lib.mjs";

const ALLOWED = new Set(["Claude Code", "Claude", "Codex"]);
const OUT = path.join(ENV.out, "names");
fs.mkdirSync(OUT, { recursive: true });

// Reads every visible label in `root`: its text, whether it holds a graphic, its lines, and its contrast ratio.
const measure = (page, root) => page.evaluate((rootSelector) => {
  const canvas = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const muted = (() => { const probe = document.createElement("span"); probe.style.color = "var(--muted)"; document.body.append(probe); const c = getComputedStyle(probe).color; probe.remove(); return c; })();
  const rgba = (css) => { canvas.clearRect(0, 0, 1, 1); canvas.fillStyle = "#000"; canvas.fillStyle = css; canvas.fillRect(0, 0, 1, 1); const [r, g, b, a] = canvas.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
  const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return a ? [0, 1, 2].map((i) => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a) : [0, 0, 0, 0]; };
  const backdrop = (node) => { const layers = []; for (let e = node; e; e = e.parentElement) layers.push(rgba(getComputedStyle(e).backgroundColor)); let c = rgba(getComputedStyle(document.documentElement).getPropertyValue("--ground") || "#fff"); if (c[3] < 1) c = [255, 255, 255, 1]; for (const layer of layers.reverse()) c = over(layer, c); return c; };
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
  return [...document.querySelectorAll(rootSelector + " .hname")].filter((n) => n.getClientRects().length).map((n) => {
    const cs = getComputedStyle(n), box = n.getBoundingClientRect(), lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.2;
    const fg = rgba(cs.color), bg = backdrop(n.parentElement);
    return { text: n.textContent, harness: [...n.classList].find((c) => c.startsWith("h-")), graphics: n.querySelectorAll("svg,img").length + n.children.length, tag: n.tagName, lines: Math.round(box.height / lh), fontSize: parseFloat(cs.fontSize), weight: cs.fontWeight, ratio: Math.round(ratio(fg, bg) * 100) / 100, color: cs.color, muted };
  });
}, root);

export default async function namesCheck(browser) {
  const D = await data(), r = reporter("names"), results = {};
  const claude = Object.values(D.SESS).find((s) => s.harness === "claude" && s.lane), codex = Object.values(D.SESS).find((s) => s.harness === "codex" && s.lane);
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
        r.expect(l.color === l.muted, tag + " " + where + ": " + l.text + " is painted " + l.color + ", not the neutral --muted " + l.muted);
        if (seen[l.text] !== undefined) seen[l.text]++;
        r.expect((l.harness === "h-codex") === (l.text === "Codex"), tag + " " + where + ": " + l.text + " carries " + l.harness);
      }
    };
    const shot = (name) => page.screenshot({ path: path.join(OUT, "names-" + tag + "-" + name + ".png") });
    const kinded = Object.values(D.SESS).find((s) => s.kind);
    for (const s of [claude, codex, ...(kinded ? [kinded] : [])]) {
      await goto(page, { v: "session", id: s.id }, D);
      await page.waitForTimeout(200);
      const top = await measure(page, "#topbar");
      // A session's top bar always shows its harness. If the line fitter dropped the label, nothing was audited, so fail.
      // A child's own top bar keeps its kind chip first, so its model label may be dropped there.
      if (s !== kinded || s === claude || s === codex) r.expect(top.length > 0, tag + ": the top bar of " + s.name + " (" + s.harness + ") shows no harness label, so none was audited");
      if (top.length) audit("topbar-" + s.harness, top);
      if (s === kinded) {
        // A child's kind chip in the top bar is neutral: no tinted fill, and the neutral ink.
        const chip = await page.evaluate(() => { const c = document.querySelector("#topbar .meta-kind"); if (!c) return null; const probe = document.createElement("span"); probe.style.color = "var(--muted)"; document.body.append(probe); const muted = getComputedStyle(probe).color; probe.remove(); const cs = getComputedStyle(c), v = getComputedStyle(c.querySelector(".meta-value")); return { bg: cs.backgroundColor, color: cs.color, valueColor: v.color, muted }; });
        rec["kind-chip"] = chip ?? "dropped by the line fitter";
        if (chip) r.expect(chip.bg === "rgba(0, 0, 0, 0)" && chip.color === chip.muted && chip.valueColor === chip.muted, tag + ": the kind chip is tinted: " + JSON.stringify(chip));
      }
      if (s === claude) await shot("session");
    }
    await goto(page, { v: "analytics" }, D);
    await page.waitForTimeout(200);
    audit("analytics", await measure(page, ".page"));
    rec.sideways = size === "phone" ? await overflow(page) : 0;
    r.expect(rec.sideways === 0, tag + ": Analytics scrolls sideways (" + rec.sideways + ")");
    await shot("analytics");
    await page.locator(".analytics-session").first().scrollIntoViewIfNeeded(); await page.waitForTimeout(150);
    await shot("analytics-list");
    // The sidebar last: on a phone it is the drawer over Home, on desktop it sits beside the open session (its row selected).
    if (size === "phone") { await goto(page, { v: "home" }, D); await page.click("#lead-btn"); await page.waitForTimeout(320); } else { await goto(page, { v: "session", id: claude.id }, D); await page.waitForTimeout(200); }
    // Sidebar rows carry no harness label: the row's aria-label names it for screen readers.
    r.expect((await measure(page, "#lanes")).length === 0 && await page.evaluate(() => document.querySelectorAll("#lanes .hname").length === 0), tag + ": a sidebar row still shows a harness label");
    r.expect(await page.evaluate(() => { const rows = [...document.querySelectorAll("#lanes .srow")]; return rows.length > 0 && rows.every((row) => /, (Claude|Codex)/.test(row.getAttribute("aria-label") ?? "")); }), tag + ": a sidebar row's aria-label does not name its harness");
    r.expect(await page.evaluate(() => document.querySelectorAll(".hmark").length === 0 && !/[✳⌘]/.test(document.querySelector("#lanes").textContent + document.querySelector("#topbar").textContent)), tag + ": an old text glyph is still drawn");
    r.expect(await page.evaluate(() => [...document.querySelectorAll("#lanes .srow-meta")].every((m) => { const tops = [...m.children].filter((c) => c.getClientRects().length).map((c) => Math.round(c.getBoundingClientRect().top / 3)); return !tops.length || Math.max(...tops) - Math.min(...tops) <= 1; })), tag + ": a sidebar meta line has more than one line");
    await shot("sidebar");
    r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }
  r.expect(seen.Codex > 0 && seen.Claude + seen["Claude Code"] > 0, "both a Claude and a Codex label must appear");
  r.results = { seen, ...results };
  return r.done();
}

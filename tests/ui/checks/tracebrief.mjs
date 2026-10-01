// A trace's brief looks the same open or closed, and a hop's line names its machine only when the trace spans several.
//
// Two phone reports: "Show more" turned the whole brief the link colour and a smaller size (the button's own rule matched the open
// brief's class too), and each hop's line ended in the raw hostname, which wrapped and pushed a marker onto a line of its own. Held here, at
// 390 and 1280 px, light and dark, on the served trace with the most clamped briefs:
//  - every clamped brief keeps its colour, size, family, weight and line height when opened (its first paragraph too), and is not the
//    accent colour of the "Show more" button;
//  - with one machine (given a long hostname), no hop line and no body summary contains the hostname, in full or in part;
//  - with two (each given a long hostname), the hop lines and the summary name them by their short names, never the hostname, and
//    the root hop's line names the machine the root ran on;
//  - with two whose short names would cut alike ("build-runner-east-1" and "-2"), the chips and the summary tell them apart, and the
//    subtitle names each machine once;
//  - a chip and a move hop's machine names carry the full name as their tooltip, and a move hop says "from <short> to <short>", each
//    name on one line (the sentence wraps between words, never inside a name);
//  - each part of a hop's line (its state, its kind chip, its note) is one line tall, and the page does not scroll sideways;
//  - the chip carries the harness's icon and no harness-coloured swatch of its own.
// The fixture's hostnames are short, so the served model is rewritten on the way to the page: the machine names are made long, and for
// two machines every session but the trace's root moves to a second one, and a move hop is added to the root's turn. Screenshots of the trace collapsed and opened are written
// to out/tracebrief/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto, settled, overflow } from "../lib.mjs";

const OUT = path.join(ENV.out, "tracebrief");
fs.mkdirSync(OUT, { recursive: true });
const HOST_A = "marvin-HP-EliteBook-X-G2i-14-inch-Notebook-Next-Gen-AI-PC", SHORT_A = "marvin-HP-Elit…";
const HOST_B = "build-runner-eu-west-4-node-17-large", SHORT_B = "build-runner-e…";
const EAST_1 = "build-runner-east-1", EAST_2 = "build-runner-east-2", TAIL_1 = "build-…-east-1", TAIL_2 = "build-…-east-2";
// The machines a run gives the trace: their full names, the short names the page must draw, and pieces of a hostname that a short name
// does not hold.
const MODES = [
  { name: "one-machine", hosts: [HOST_A], shorts: [], pieces: ["EliteBook", "Notebook", "AI-PC"] },
  { name: "two-machines", hosts: [HOST_A, HOST_B], shorts: [SHORT_A, SHORT_B], pieces: ["EliteBook", "Notebook", "AI-PC", "build-runner-eu", "node-17"] },
  { name: "colliding-names", hosts: [EAST_1, EAST_2], shorts: [TAIL_1, TAIL_2], pieces: [EAST_1, EAST_2, "build-runner"] },
];
const SECOND = "tracebrief-b";

// The served model with long machine names; with two, every session but `rootSid` on a second machine, and a move between them in the
// root's turn (the served logs hold none).
async function longNames(page, { hosts, rootSid, turn }) {
  await page.route("**/api/model*", async (route) => {
    const res = await route.fetch();
    let m; try { m = await res.json(); } catch { return route.fulfill({ response: res }); }
    if (m?.sessions && m.machine) {
      const first = { ...m.machine, name: hosts[0] };
      if (hosts.length > 1) {
        m.machines = [first, { id: SECOND, name: hosts[1], up: true }]; m.machine = first;
        for (const [id, s] of Object.entries(m.sessions)) s.machine = id === rootSid ? first.id : SECOND;
        if (!m.handoffs.some((h) => h.id === "tracebrief-move")) {
          m.handoffs.push({ id: "tracebrief-move", kind: "move", from: rootSid, to: rootSid, fromMachine: first.id, toMachine: SECOND, at: m.now - 60000, status: "done", brief: "Moved." });
          m.turns.find((t) => t.id === turn)?.sent.push("tracebrief-move");
        }
      } else m.machine = first;
    }
    return route.fulfill({ status: res.status(), contentType: "application/json", body: JSON.stringify(m) });
  });
  await page.reload({ waitUntil: "load" });
  await settled(page);
}

// How each hop's brief is drawn: the brief's own boxes and its first paragraph's.
const snap = (page) => page.evaluate(() => [...document.querySelectorAll(".hop .body > .brief")].map((b) => {
  const more = b.parentElement.querySelector(":scope > .more");
  const look = (n) => { if (!n) return null; const c = getComputedStyle(n); return { color: c.color, fontSize: c.fontSize, fontFamily: c.fontFamily, fontWeight: c.fontWeight, fontStyle: c.fontStyle, lineHeight: c.lineHeight }; };
  return { text: b.textContent.slice(0, 40), open: b.classList.contains("open"), clamped: !!more && !more.hidden, brief: look(b), inner: look(b.querySelector("p, li")), moreColor: more ? getComputedStyle(more).color : null };
}));

// Each part of every hop's line, with how many lines it takes (the text's own boxes, not the row's).
const lines = (page) => page.evaluate(() => [...document.querySelectorAll(".hop .meta > .stat, .hop .meta > .chip-h, .hop .meta > .gone")].map((n) => {
  const cs = getComputedStyle(n), lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.4, tops = [];
  const walk = document.createTreeWalker(n, NodeFilter.SHOW_TEXT);
  for (let t = walk.nextNode(); t; t = walk.nextNode()) { if (!t.textContent.trim()) continue; const r = document.createRange(); r.selectNodeContents(t); for (const q of r.getClientRects()) if (q.width) tops.push(q.top); }
  const spread = tops.length ? Math.max(...tops) - Math.min(...tops) : 0;
  return { cls: n.className, text: n.textContent.trim(), oneLine: spread < lh * 0.7, spread: Math.round(spread * 10) / 10 };
}));

const page_ = (page) => page.evaluate(() => ({
  metaText: [...document.querySelectorAll(".hop .meta")].map((m) => m.textContent.replace(/\s+/g, " ").trim()),
  chips: [...document.querySelectorAll(".hop .meta .chip-h")].map((c) => c.textContent.replace(/[\s\u2009\u00a0]+/g, " ").trim()),
  chipIcons: [...document.querySelectorAll(".hop .meta .chip-h")].map((c) => !!c.querySelector(".hicon")),
  chipTips: [...document.querySelectorAll(".hop .meta .chip-h")].map((c) => c.dataset.tip ?? null),
  sent: [...document.querySelectorAll(".hop.k-move .sent")].map((c) => c.textContent.replace(/[\s\u2009\u00a0]+/g, " ").trim()),
  sentLines: [...document.querySelectorAll(".hop.k-move .sent .mach")].map((n) => {
    const lh = parseFloat(getComputedStyle(n).lineHeight) || parseFloat(getComputedStyle(n).fontSize) * 1.4, tops = [];
    const walk = document.createTreeWalker(n, NodeFilter.SHOW_TEXT);
    for (let t = walk.nextNode(); t; t = walk.nextNode()) { const r = document.createRange(); r.selectNodeContents(t); for (const q of r.getClientRects()) if (q.width) tops.push(q.top); }
    return { text: n.textContent, oneLine: tops.length > 0 && Math.max(...tops) - Math.min(...tops) < lh * 0.7 };
  }),
  sentTips: [...document.querySelectorAll(".hop.k-move .sent .mach")].map((c) => c.dataset.tip ?? null),
  bar: (document.querySelector("#page .trace-summary")?.textContent ?? "").replace(/\s+/g, " ").trim(),
  chipMark: [...document.querySelectorAll(".hop .meta .chip-h")].map((c) => { const b = getComputedStyle(c, "::before"); return b.content !== "none" && b.content !== "normal" ? b.content : null; }).filter(Boolean),
}));

export default async function tracebrief(browser) {
  const D = await data();
  const r = reporter("tracebrief");
  r.results.runs = [];

  // The served trace with the most clamped briefs, and its root session.
  const spawners = [...new Set(D.H.filter((h) => h.kind === "spawn" || h.kind === "relay").map((h) => h.from))].filter((id) => D.SESS[id]);
  const probe = await served(browser, { size: "phone" });
  let pick = null;
  for (const sid of spawners) {
    await goto(probe, { v: "session", id: sid }, D);
    const turns = await probe.evaluate(() => [...document.querySelectorAll(".turn-end .link")].map((b) => b.closest(".turn").dataset.turn));
    for (const turn of turns) {
      await goto(probe, { v: "trace", sid, turn }, D);
      const n = await probe.evaluate(() => ({ clamped: [...document.querySelectorAll(".hop .body > .more")].filter((x) => !x.hidden).length, hops: document.querySelectorAll(".hop.child").length, chips: document.querySelectorAll(".hop .meta .chip-h").length }));
      if (n.hops && (!pick || n.clamped > pick.clamped || (n.clamped === pick.clamped && n.hops > pick.hops))) pick = { sid, turn, ...n };
    }
  }
  await probe.context().close();
  r.results.trace = pick;
  r.expect(!!pick, "the fixture needs a trace with a hop to check");
  if (!pick) return r.done();
  r.expect(pick.clamped > 0, "the chosen trace needs a brief that clamps behind \"Show more\": " + JSON.stringify(pick));

  for (const size of ["phone", "desktop"]) {
    for (const dark of [false, true]) {
      for (const mode of MODES) {
        const two = mode.hosts.length > 1, name = size + (dark ? "-dark" : "-light") + "-" + mode.name;
        const page = await served(browser, { size, dark });
        await longNames(page, { hosts: mode.hosts, rootSid: pick.sid, turn: pick.turn });
        await goto(page, { v: "trace", sid: pick.sid, turn: pick.turn }, D);
        await page.waitForTimeout(150);

        // The brief, collapsed, then opened.
        const before = await snap(page);
        await page.screenshot({ path: path.join(OUT, name + "-collapsed.png"), fullPage: true });
        const idx = before.map((b, i) => (b.clamped && !b.open ? i : -1)).filter((i) => i >= 0);
        // At 1280 px a brief may fit whole; at 390 the chosen trace's briefs must clamp.
        if (size === "phone") r.expect(idx.length > 0, name + ": no clamped brief to open");
        await page.evaluate(() => document.querySelectorAll(".hop .body > .more:not([hidden])").forEach((x) => x.click()));
        await page.waitForTimeout(80);
        const after = await snap(page);
        await page.screenshot({ path: path.join(OUT, name + "-expanded.png"), fullPage: true });
        for (const i of idx) {
          const a = before[i], b = after[i];
          r.expect(b.open, name + ": brief " + i + " opened");
          for (const k of ["color", "fontSize", "fontFamily", "fontWeight", "fontStyle", "lineHeight"]) {
            r.expect(a.brief[k] === b.brief[k], name + ": brief " + i + " changes " + k + " when opened: " + a.brief[k] + " then " + b.brief[k]);
            if (a.inner && b.inner) r.expect(a.inner[k] === b.inner[k], name + ": brief " + i + "'s first paragraph changes " + k + " when opened: " + a.inner[k] + " then " + b.inner[k]);
          }
          r.expect(b.brief.color !== b.moreColor, name + ": the open brief is drawn in the button's colour (" + b.brief.color + ")");
        }

        // Machines: named by their short names where the trace spans two, and not at all where it spans one.
        const p = await page_(page), all = p.metaText.join(" | ") + " | " + p.sent.join(" | ") + " | " + p.bar, [s0, s1] = mode.shorts;
        r.expect(p.chips.length > 0, name + ": no hop line has a kind chip");
        r.expect(/handoff/.test(p.bar), name + ": the body summary is missing: " + JSON.stringify(p.bar));
        for (const piece of [...mode.hosts, ...mode.pieces]) r.expect(!all.includes(piece), name + ": the hostname shows (" + piece + "): " + JSON.stringify({ bar: p.bar, chips: p.chips, sent: p.sent }));
        if (two) {
          const named = (c) => [s0, s1].find((x) => c.endsWith(" · " + x));
          r.expect(p.bar.includes(s0) && p.bar.includes(s1), name + ": the summary names both machines by their short names: " + JSON.stringify(p.bar));
          r.expect(p.bar.split(s0).length === 2 && p.bar.split(s1).length === 2, name + ": the summary names a machine more than once: " + JSON.stringify(p.bar));
          r.expect(p.chips.every(named), name + ": every chip ends in a machine's short name: " + JSON.stringify(p.chips));
          r.expect(p.chips[0]?.endsWith(" · " + s0), name + ": the root hop's chip names its machine: " + JSON.stringify(p.chips[0]));
          r.expect(p.chips.some((c) => c.endsWith(" · " + s1)), name + ": a hop on the second machine names it: " + JSON.stringify(p.chips));
          r.expect(new Set(p.chips.map(named)).size === 2, name + ": the chips do not tell the two machines apart: " + JSON.stringify(p.chips));
          r.expect(p.chipTips.every((t) => t === "Machine: " + mode.hosts[0] || t === "Machine: " + mode.hosts[1]), name + ": a chip's tooltip is not \"Machine: <full name>\": " + JSON.stringify(p.chipTips));
          r.expect(p.chipTips[0] === "Machine: " + mode.hosts[0] && p.chipTips.includes("Machine: " + mode.hosts[1]), name + ": the chips' tooltips do not name both machines in full: " + JSON.stringify(p.chipTips));
          r.expect(p.sent.length === 1 && p.sent[0].includes(" from " + s0 + " to " + s1), name + ": the move hop does not say from <short> to <short>: " + JSON.stringify(p.sent));
          r.expect(p.sentLines.length === 2 && p.sentLines.every((l) => l.oneLine), name + ": a machine name in the move hop's sentence is split across lines: " + JSON.stringify(p.sentLines));
          r.expect(p.sentTips.join("|") === ["Machine: " + mode.hosts[0], "Machine: " + mode.hosts[1]].join("|"), name + ": the move hop's machine names lack their full-name tooltips: " + JSON.stringify(p.sentTips));
        } else {
          r.expect(p.chips.every((c) => !c.includes(" · ")), name + ": a chip names a machine although the trace spans one: " + JSON.stringify(p.chips));
          r.expect(p.chipTips.every((t) => t == null), name + ": a chip has a machine tooltip although the trace spans one: " + JSON.stringify(p.chipTips));
        }
        r.expect(p.chipIcons.length > 0 && p.chipIcons.every(Boolean), name + ": a chip has lost its harness icon: " + JSON.stringify(p.chipIcons));
        r.expect(p.chipMark.length === 0, name + ": a chip carries a coloured mark: " + JSON.stringify(p.chipMark));

        // Every part of every hop's line is one line, and nothing scrolls sideways.
        const ls = await lines(page);
        r.expect(ls.length > 0, name + ": no hop line found");
        for (const l of ls) r.expect(l.oneLine, name + ": a hop line wraps (" + l.cls + "): " + JSON.stringify(l));
        const over = await overflow(page);
        r.expect(over === 0, name + ": the trace sticks out sideways (" + over + ")");
        r.expect(page.errors.length === 0, name + ": page errors: " + page.errors.join(" | "));
        r.results.runs.push({ name, briefs: idx.length, chips: p.chips, bar: p.bar, wrapped: ls.filter((l) => !l.oneLine).length });
        await page.context().close();
      }
    }
  }
  return r.done();
}

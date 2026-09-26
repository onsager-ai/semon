// Ported from the mockup's timeline.js: page overflow, pinned labels and axis, rows vs an independent parentOf model,
// collapse/expand, bars vs busy intervals and the time map, a handoff-mark census, connector ends on row centres,
// zoom keeping the centre, taps landing on the right turn with Back restoring scroll and zoom, tap targets, and page
// errors — phone light, phone dark and desktop.
//
// The original ran 'sample' (test7.html) and 'real' (test-real.html) tagged. Ported: 'real' is dropped; 'sample'
// drives the served fixture. Nothing here names a sample handoff or turn id — every lookup is by session id, by a
// handoff read live off the DOM (`data-hs`), or by the model's own `parentOf`/`turnAt` — so there is no id mapping.
// Per gaps.json this fixture has one machine and no moves, so there is nothing to collapse-test beyond whichever
// session has the deepest subagent chain (harbor's or ledger's or quill's spawn), same as the original picked by
// "longest disclosure title" — that logic is unchanged.
//
// Assertions:
//  - no page errors.
//  - zero page overflow, in every mode.
//  - rows: hierarchyMismatch === 0 and missing === 0 (every session has exactly one row, ordered and indented under
//    its parentOf), and tracksMatchLabels === true.
//  - bars: missing === 0 and extra === 0 (every busy interval is drawn once, nothing else is), xOffBy1px === 0 (each
//    bar sits exactly where the time map places it), insideGap === 0 (no bar crosses a drawn gap).
//  - the handoff census: for every kind (ask/spawn/relay/toyou), missing === 0 and twice === 0 (drawn exactly once);
//    endsOffRowCentre === 0, marksOffRowCentre === 0, marksOrLinesOffTime === 0.
//  - collapsing a parent drops exactly its descendant rows (droppedMatches) without breaking the row/track parity
//    (tracksMatchLabels) or its connector's row-centre reattachment, and expanding restores every row (reopened).
//  - labels don't move when the chart scrolls sideways (labelXStill), the axis stays aligned under the chart
//    (axisTicksOffChart === 0) and under the top bar when the page scrolls (axisUnderBar, firstRowUnderAxis).
//  - zooming in and back out keeps the centre time under the middle of the view (centreShiftPxIn/Out small).
//  - every tap target checked is at least 36px (tapTargets.under36 === 0).
//  - every tap tried lands on the right turn and Back restores the same horizontal scroll and zoom (taps.bad is
//    empty), and on desktop, wheel over the axis and shift+wheel over the chart both scroll it sideways.
import path from "node:path";
import { ENV, served, data, reporter, overflow } from "../lib.mjs";

const isGap = (e) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");

function model(D0) {
  const D = { ...D0 };
  const HID = new Map(D.H.map((h) => [h.id, h])), turns = {}, starts = new Set();
  for (const [sid, es] of Object.entries(D.TX)) { const ts = turns[sid] = []; let t = null;
    es.forEach((e, i) => { if (isGap(e)) { t = null; return; } const h = e.k === "h" ? HID.get(e.id) : null;
      const inc = h ? h.to === sid && ["ask", "relay", "spawn"].includes(h.kind) && !starts.has(h.id) : e.k === "u";
      if (inc || !t) { t = { id: inc && h ? h.id : (e.turn ?? sid + ":" + i) /* the server names a turn with no start handoff <sid>:<file>:<offset>, on its first entry */, start: inc ? h : null }; ts.push(t); if (inc && h) starts.add(h.id); } });
  }
  D.turnAt = (sid, t) => (turns[sid] ?? []).filter((x) => x.start?.at != null && x.start.at <= t).at(-1);
  D.parentOf = (sid) => D.H.find((h) => h.kind === "spawn" && h.to === sid)?.from;
  D.busyOf = (s) => { const iv = (s.busy ?? []).map(([a, b]) => [a, b]); if (s.state === "work" && iv.length) iv.at(-1)[1] = Math.max(iv.at(-1)[1], D.NOW); return iv; };
  return D;
}
const Xof = (pieces) => (t) => { for (const p of pieces) if (t <= p.b) return p.x0 + Math.max(0, t - p.a) / ((p.b - p.a) || 1) * p.w; const q = pieces.at(-1); return q.x0 + q.w; };
const Tof = (pieces) => (v) => { for (const p of pieces) if (v <= p.x0 + p.w) return p.a + Math.max(0, v - p.x0) / (p.w || 1) * (p.b - p.a); return pieces.at(-1).b; };
const afterTitle = (page, text) => page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t, text);

export default async function timelineCheck(browser) {
  const D0 = await data();
  const D = model(D0);
  const MIN = D.NOW > 1e11 ? 60000 : 1;
  const r = reporter("timeline");
  const modes = [];

  for (const [mode, dark] of [["phone-light", false], ["phone-dark", true], ["desktop", false]]) {
    const phone = mode !== "desktop";
    const errs = []; const R = { mode };
    const page = await served(browser, { size: phone ? "phone" : "desktop", dark });
    if (phone) { await page.click("#lead-btn"); await page.waitForTimeout(300); }
    await page.click('.nav-item[data-go="timeline"]'); await afterTitle(page, "Timeline"); await page.waitForTimeout(250);
    const over = () => overflow(page);
    const pieces = async () => JSON.parse(await page.evaluate(() => document.querySelector(".tl-canvas").dataset.map));
    R.open = await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); return { atRightEdge: Math.abs(s.scrollLeft - (s.scrollWidth - s.clientWidth)) <= 1, chartW: s.scrollWidth, viewW: s.clientWidth, zoom: document.querySelector(".tl-ctl .zl").textContent, docScrollW: document.documentElement.scrollWidth, vw: document.documentElement.clientWidth }; });
    R.overflow = await over();
    if (mode === "phone-light") await page.screenshot({ path: path.join(ENV.out, "timeline-sample.png") });
    if (mode === "phone-dark") await page.screenshot({ path: path.join(ENV.out, "timeline-sample-dark.png") });
    if (mode === "desktop") {
      await page.screenshot({ path: path.join(ENV.out, "timeline-desktop.png") });
      await page.evaluate(() => { document.querySelector(".tl-scroll").scrollLeft = 0; }); await page.waitForTimeout(150); await page.screenshot({ path: path.join(ENV.out, "timeline-desktop-left.png") });
      await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); s.scrollLeft = s.scrollWidth; }); await page.waitForTimeout(100);
    }
    R.defaults = await page.evaluate(() => [...document.querySelectorAll(".tl-disc")].map((d) => d.getAttribute("aria-expanded") + ":" + d.title));
    const counts = () => page.evaluate(() => ({ labels: document.querySelectorAll(".tl-lab").length, tracks: document.querySelectorAll('.tl-track[data-k="s"]').length }));
    for (let k = 0; k < 6; k++) { const d = await page.$('.tl-disc[aria-expanded="false"]'); if (!d) break; await d.click(); await page.waitForTimeout(120); }
    const full = await counts();
    const disc = await page.evaluate(() => { const d = [...document.querySelectorAll(".tl-disc")].sort((a, b) => b.title.length - a.title.length)[0]; return d ? d.dataset.sid : null; });
    if (disc) {
      const n = Object.values(D.SESS).filter((s) => { let x = s.id; const seen = new Set(); while (D.parentOf(x) && !seen.has(x)) { seen.add(x); x = D.parentOf(x); if (x === disc) return true; } return false; }).length;
      await page.click('.tl-disc[data-sid="' + disc + '"]'); await page.waitForTimeout(150);
      const shut = await counts();
      const reattached = await page.evaluate((disc) => [...document.querySelectorAll(".tl-conn .hd, .tl-conn .ft")].filter((h) => h.dataset.row === disc).length, disc);
      await page.click('.tl-disc[data-sid="' + disc + '"]'); await page.waitForTimeout(150);
      R.collapse = { parent: D.SESS[disc].name, descendants: n, rowsOpen: full.labels, rowsShut: shut.labels, dropped: full.labels - shut.labels, droppedMatches: full.labels - shut.labels === n, tracksMatchLabels: shut.labels === shut.tracks, connectorEndsOnParentWhileShut: reattached, reopened: (await counts()).labels === full.labels };
    }
    const rows = await page.evaluate(() => [...document.querySelectorAll(".tl-lab")].map((b) => ({ sid: b.dataset.sid, depth: +b.dataset.depth, parent: b.dataset.parent })));
    const idx = new Map(rows.map((row, i) => [row.sid, i])); let hierBad = 0;
    for (const row of rows) { const par = D.parentOf(row.sid) && D.SESS[D.parentOf(row.sid)] ? D.parentOf(row.sid) : ""; if (row.parent !== par) hierBad++; if (par) { if (!(idx.get(par) < idx.get(row.sid)) || row.depth !== rows[idx.get(par)].depth + 1) hierBad++; } else if (row.depth !== 0) hierBad++; }
    R.rows = { sessions: Object.keys(D.SESS).length, rows: rows.length, missing: Object.keys(D.SESS).filter((s) => !idx.has(s)).length, hierarchyMismatch: hierBad, tracksMatchLabels: full.labels === full.tracks };
    let P = await pieces(), X = Xof(P);
    const bars = await page.evaluate(() => { const c = document.querySelector(".tl-canvas").getBoundingClientRect(); return [...document.querySelectorAll(".tl-barhit")].map((h) => { const v = h.querySelector(".tl-bar").getBoundingClientRect(); return { sid: h.dataset.sid, i: +h.dataset.i, a: +h.dataset.a, b: +h.dataset.b, l: v.left - c.left, w: v.width }; }); });
    const exp = Object.values(D.SESS).flatMap((s) => D.busyOf(s).map(([a, bb], i) => s.id + ":" + i + ":" + a + ":" + bb)), got = bars.map((x) => x.sid + ":" + x.i + ":" + x.a + ":" + x.b);
    let xOff = 0, inGap = 0; for (const x of bars) { const w = Math.max(3, X(x.b) - X(x.a)); if (Math.abs(x.l - X(x.a)) > 1 || Math.abs(x.w - w) > 1) xOff++; for (const g of P.filter((q) => q.gap)) if (x.a < g.b && x.b > g.a) inGap++; }
    const gaps = P.filter((q) => q.gap); const pts = [...Object.values(D.SESS).flatMap((s) => D.busyOf(s).flat()), ...D.H.flatMap((h) => [h.at, h.done].filter((v) => v != null))];
    R.bars = { expected: exp.length, drawn: got.length, missing: exp.filter((k) => !got.includes(k)).length, extra: got.filter((k) => !exp.includes(k)).length, xOffBy1px: xOff, insideGap: inGap,
      gaps: gaps.map((g) => Math.round((g.b - g.a + 20 * MIN) / MIN / 60) + "h"), gapsUnder2h: gaps.filter((g) => g.b - g.a + 20 * MIN <= 120 * MIN).length, pointsInsideGaps: pts.filter((t) => gaps.some((g) => t > g.a && t < g.b)).length };
    const barsBy = {}; for (const x of bars) (barsBy[x.sid] ??= []).push([x.a, x.b]);
    R.coverage = {}; let covIn = 0, covAll = 0;
    for (const [sid, s] of Object.entries(D.SESS)) { const ev = D.H.filter((h) => h.from === sid || h.to === sid); const outside = ev.filter((h) => !(barsBy[sid] ?? []).some(([a, bb]) => h.at >= a - MIN && h.at <= bb + MIN));
      covIn += ev.length - outside.length; covAll += ev.length; R.coverage[s.name.slice(0, 24)] = (ev.length - outside.length) + "/" + ev.length + (outside.length ? " outside: " + outside.map((h) => h.id + ":" + h.kind).join(",") : ""); }
    R.coverageTotal = covIn + "/" + covAll;
    const marks = await page.evaluate(() => { const c = document.querySelector(".tl-canvas").getBoundingClientRect(), rowC = {};
      document.querySelectorAll('.tl-track[data-k="s"]').forEach((t) => { const r = t.getBoundingClientRect(); rowC[t.dataset.sid] = r.top + r.height / 2 - c.top; });
      const items = [...document.querySelectorAll(".tl-canvas [data-hs]")].map((m) => ({ k: m.dataset.k, hs: m.dataset.hs.split(","), cls: m.className }));
      let endOff = 0, ends = 0, markOff = 0; const endEx = [];
      for (const h of document.querySelectorAll(".tl-conn .hd, .tl-conn .ft")) { ends++; const r = h.getBoundingClientRect(); const y = (h.classList.contains("dn") ? r.bottom : h.classList.contains("up") ? r.top : r.top + 0.75) - c.top;
        if (Math.abs(y - rowC[h.dataset.row]) > 1) { endOff++; if (endEx.length < 3) endEx.push(h.dataset.row + " " + (y - rowC[h.dataset.row]).toFixed(2)); } }
      const mk = [...document.querySelectorAll(".tl-mark")].map((m) => { const r = m.querySelector(".vis").getBoundingClientRect(); return { row: m.dataset.row, y: r.top + r.height / 2 - c.top, x: r.left + r.width / 2 - c.left, hs: m.dataset.hs }; });
      for (const m of mk) if (Math.abs(m.y - rowC[m.row]) > 1) markOff++;
      const lines = [...document.querySelectorAll(".tl-conn")].map((m) => ({ x: +m.dataset.x, lx: m.querySelector(".ln").getBoundingClientRect().left + 0.75 - c.left, hs: m.dataset.hs }));
      return { items, ends, endOff, endEx, markOff, mk, lines }; });
    const HMap = new Map(D.H.map((h) => [h.id, h])), occ = new Map(); for (const it of marks.items) for (const id of it.hs) occ.set(id, (occ.get(id) ?? 0) + 1);
    const want = D.H.filter((h) => ["ask", "spawn", "relay", "toyou"].includes(h.kind)), per = {};
    for (const h of want) { const c = occ.get(h.id) ?? 0; per[h.kind] ??= { handoffs: 0, drawn: 0, missing: 0, twice: 0 }; per[h.kind].handoffs++; if (c === 1) per[h.kind].drawn++; else if (!c) per[h.kind].missing++; else per[h.kind].twice++; }
    let timeOff = 0; for (const m of marks.mk) if (Math.abs(m.x - X(HMap.get(m.hs).at)) > 1) timeOff++; for (const l of marks.lines) if (Math.abs(l.lx - X(HMap.get(l.hs.split(",")[0]).at)) > 1) timeOff++;
    R.census = { perKind: per, broadcasts: marks.items.filter((i) => i.hs.length > 1).map((i) => i.hs.length), connectorEnds: marks.ends, endsOffRowCentre: marks.endOff, endEx: marks.endEx, marksOffRowCentre: marks.markOff, marksOrLinesOffTime: timeOff };
    R.pinned = await page.evaluate(async () => { const s = document.querySelector(".tl-scroll"), lab = document.querySelector(".tl-lab"), x0 = lab.getBoundingClientRect().left;
      s.scrollLeft = s.scrollWidth / 3; await new Promise((r) => setTimeout(r, 80)); const x1 = lab.getBoundingClientRect().left;
      const c = document.querySelector(".tl-canvas").getBoundingClientRect(); let axisOff = 0; for (const t of [...document.querySelectorAll(".tl-tick")].slice(0, 30)) if (Math.abs(t.getBoundingClientRect().left - (c.left + parseFloat(t.style.left))) > 1) axisOff++;
      return { labelXStill: Math.abs(x1 - x0) < 0.5, axisTicksOffChart: axisOff }; });
    await page.setViewportSize({ width: phone ? 390 : 1280, height: 480 }); await page.waitForTimeout(150);
    Object.assign(R.pinned, await page.evaluate(async () => { const m = document.querySelector("#main"), ph = matchMedia("(max-width: 760px)").matches, bar = document.querySelector("#topbar").getBoundingClientRect();
      const ax0 = document.querySelector(".tl-axisrow").getBoundingClientRect(), by = ax0.top - bar.bottom + 150; if (ph) window.scrollTo(0, by); else m.scrollTop = by; await new Promise((r) => setTimeout(r, 150));
      const ax = document.querySelector(".tl-axisrow").getBoundingClientRect(), b2 = document.querySelector("#topbar").getBoundingClientRect(), scrolled = ph ? scrollY : m.scrollTop, row = document.querySelector(".tl-lab").getBoundingClientRect();
      if (ph) window.scrollTo(0, 0); else m.scrollTop = 0;
      return { axisUnderBar: Math.abs(ax.top - b2.bottom) < 1.5, pageScrolledBy: Math.round(scrolled), firstRowUnderAxis: row.top < ax.bottom }; }));
    await page.setViewportSize(phone ? { width: 390, height: 844 } : { width: 1280, height: 860 }); await page.waitForTimeout(150);
    if (mode === "phone-light") { await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); s.scrollLeft = (s.scrollWidth - s.clientWidth) * 0.45; }); await page.waitForTimeout(100); await page.screenshot({ path: path.join(ENV.out, "timeline-sample-mid.png") }); }
    const centreT = async () => { const P2 = await pieces(); const v = await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); return s.scrollLeft + s.clientWidth / 2; }); return { t: Tof(P2)(v), X: Xof(P2) }; };
    // Zooming keeps the centre, and the wheel scrolls, only where the chart is wider than the view: record whether
    // that's already true at the default 60 px/h (before touching zoom at all — widen() below would otherwise make
    // this always true, since it zooms in until it's wide, hiding whether it started out that way), then zoom in
    // until it is wide (for the wheel, until it has 400 px to scroll: that check starts at 200 px), as the original
    // measured on data wider than a desktop screen.
    const naturalWide = await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); return s.scrollWidth > s.clientWidth + 1; });
    const widen = async (room) => { for (let k = 0; k < 4; k++) { const wide = await page.evaluate((room) => { const s = document.querySelector(".tl-scroll"); return s.scrollWidth - s.clientWidth >= room; }, room); if (wide) return; await page.click('[aria-label="Zoom in"]'); await page.waitForTimeout(150); } };
    await widen(1);
    await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); s.scrollLeft = (s.scrollWidth - s.clientWidth) / 2; }); await page.waitForTimeout(80);
    const wideAfterWiden = await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); return s.scrollWidth > s.clientWidth + 1; });
    const z0 = await centreT(), zl0 = await page.evaluate(() => document.querySelector(".tl-ctl .zl").textContent);
    await page.click('[aria-label="Zoom in"]'); await page.waitForTimeout(150); const z1 = await centreT(), zl1 = await page.evaluate(() => document.querySelector(".tl-ctl .zl").textContent);
    if (mode === "phone-light") await page.screenshot({ path: path.join(ENV.out, "timeline-sample-zoomed.png") });
    await page.click('[aria-label="Zoom out"]'); await page.waitForTimeout(150); const z2 = await centreT();
    R.zoom = { naturalWide, wideAfterWiden, from: zl0, to: zl1, centreShiftPxIn: +Math.abs(z1.X(z1.t) - z1.X(z0.t)).toFixed(2), centreShiftPxOut: +Math.abs(z2.X(z2.t) - z2.X(z0.t)).toFixed(2), back: await page.evaluate(() => document.querySelector(".tl-ctl .zl").textContent) };
    const relN = () => page.evaluate(() => [...document.querySelectorAll(".tl-conn.rel")].filter((c) => c.getClientRects().length).length);
    const before = await relN(); await page.click(".tl-tog"); await page.waitForTimeout(80); const off = await relN(); await page.click(".tl-tog"); await page.waitForTimeout(80);
    R.messagesToggle = { shown: before, hidden: off === 0, back: (await relN()) === before };
    R.tapTargets = await page.evaluate(() => { const all = [...document.querySelectorAll(".tl-hit, .tl-lab, .tl-disc, .tl-ctl button")].filter((x) => x.getClientRects().length); const small = all.filter((x) => { const r = x.getBoundingClientRect(); return r.height < 35.5 || r.width < 35.5; }); return { checked: all.length, under36: small.length, ex: small.slice(0, 3).map((x) => x.className) }; });
    if (mode !== "phone-dark") {
      const T = { tried: 0, ok: 0, occludedSkipped: 0, bad: [], byKind: {} };
      const picks = await page.evaluate(() => { const by = {}; for (const h of document.querySelectorAll(".tl-barhit, .tl-mark, .tl-conn")) { const k = h.classList.contains("tl-barhit") ? "bar" : h.dataset.k; (by[k] ??= []).push(h); }
        const sel = (h, k) => k === "bar" ? '.tl-barhit[data-sid="' + h.dataset.sid + '"][data-i="' + h.dataset.i + '"]' : '.tl-hit[data-k="' + k + '"][data-hs="' + h.dataset.hs + '"]';
        return Object.entries(by).flatMap(([k, a]) => [a[0], a[Math.floor(a.length / 2)], a.at(-1)].filter((x, i, all) => all.indexOf(x) === i).map((h) => [k, sel(h, k)])); });
      for (const [k, id] of picks) {
        const pt = await page.evaluate((id) => { const h = document.querySelector(id), s = document.querySelector(".tl-scroll"), vis = h.querySelector(".vis, .tl-bar, .ln") ?? h;
          const hl = parseFloat(h.style.left) + (vis.classList.contains("tl-bar") ? parseFloat(vis.style.left) + parseFloat(vis.style.width) / 2 : 18);
          s.scrollLeft = hl - s.clientWidth / 2; h.scrollIntoView({ block: "center", inline: "nearest" });
          const r = vis.getBoundingClientRect(); const cands = [[r.left + r.width / 2, r.top + r.height / 2]]; if (vis.classList.contains("ln")) for (let f = 0.1; f < 1; f += 0.1) cands.push([r.left + r.width / 2, r.top + r.height * f]);
          for (const [x, y] of cands) { const e = document.elementFromPoint(x, y); if (e && (e === h || h.contains(e))) return { x, y, left: s.scrollLeft, zoom: document.querySelector(".tl-ctl .zl").textContent, a: +h.dataset.a, b: +h.dataset.b, sid: h.dataset.sid, to: h.dataset.to, turn: h.dataset.turn }; }
          return null; }, id);
        if (!pt) { T.occludedSkipped++; continue; }
        await page.waitForTimeout(100); T.tried++; T.byKind[k] = (T.byKind[k] ?? 0) + 1;
        let expId, expTurn;
        if (k === "bar") { const P3 = await pieces(); const cl = await page.evaluate(() => document.querySelector(".tl-canvas").getBoundingClientRect().left); const t = Math.min(pt.b, Math.max(pt.a, Tof(P3)(pt.x - cl))); expId = pt.sid; expTurn = D.turnAt(pt.sid, t)?.id ?? ""; }
        else { expId = pt.to; expTurn = pt.turn; }
        await page.mouse.click(pt.x, pt.y); await page.waitForTimeout(300);
        const gotAt = await page.evaluate(() => { const st = history.state, t = st?.turn ? [...document.querySelectorAll(".turn")].find((y) => y.dataset.turn === st.turn) : null, bar = document.querySelector("#topbar").getBoundingClientRect(); return { v: st?.v, id: st?.id, turn: st?.turn ?? "", found: !!t, below: t ? t.getBoundingClientRect().top >= bar.bottom - 0.5 : null, flash: t ? t.classList.contains("flash") : null }; });
        const good = gotAt.v === "session" && gotAt.id === expId && gotAt.turn === expTurn && (expTurn ? gotAt.found && gotAt.below && gotAt.flash : true);
        await page.goBack(); await page.waitForTimeout(250);
        const back = await page.evaluate(() => ({ v: history.state?.v, left: document.querySelector(".tl-scroll")?.scrollLeft, zoom: document.querySelector(".tl-ctl .zl")?.textContent }));
        const restored = back.v === "timeline" && Math.abs(back.left - pt.left) <= 1 && back.zoom === pt.zoom;
        if (good && restored) T.ok++; else T.bad.push(id + " " + JSON.stringify({ exp: [expId?.slice(0, 8), expTurn], got: gotAt, back, left: pt.left }));
      }
      R.taps = T;
    }
    if (!phone) await widen(400);
    if (!phone) { R.wheel = await page.evaluate(() => { const s = document.querySelector(".tl-scroll"); s.scrollLeft = 200; const a = s.scrollLeft; document.querySelector(".tl-axisclip").dispatchEvent(new WheelEvent("wheel", { deltaY: 120, bubbles: true, cancelable: true })); const b = s.scrollLeft;
      s.dispatchEvent(new WheelEvent("wheel", { deltaY: 120, shiftKey: true, bubbles: true, cancelable: true })); return { axisWheelMoved: b - a, shiftWheelMoved: s.scrollLeft - b }; }); }
    R.nav = await page.evaluate(() => [...document.querySelectorAll(".nav-item")].map((n) => n.textContent + (n.getAttribute("aria-current") ? "*" : "")));
    R.errors = page.errors;
    modes.push(R);
    await page.context().close();
  }

  r.results = modes;
  for (const R of modes) {
    r.expect(R.errors.length === 0, R.mode + ": page errors: " + R.errors.join(" | "));
    r.expect(R.overflow === 0, R.mode + ": overflow=" + R.overflow);
    r.expect(R.rows.hierarchyMismatch === 0 && R.rows.missing === 0, R.mode + ": rows model mismatch: " + JSON.stringify(R.rows));
    r.expect(R.rows.tracksMatchLabels === true, R.mode + ": rows/tracks count mismatch");
    r.expect(R.bars.missing === 0 && R.bars.extra === 0, R.mode + ": bars missing/extra: " + JSON.stringify(R.bars));
    r.expect(R.bars.xOffBy1px === 0, R.mode + ": bars off by >1px: " + R.bars.xOffBy1px);
    r.expect(R.bars.insideGap === 0, R.mode + ": bars drawn inside a gap: " + R.bars.insideGap);
    for (const [kind, c] of Object.entries(R.census.perKind)) r.expect(c.missing === 0 && c.twice === 0, R.mode + ": " + kind + " census missing/twice: " + JSON.stringify(c));
    r.expect(R.census.endsOffRowCentre === 0, R.mode + ": connector ends off row centre: " + JSON.stringify(R.census.endEx));
    r.expect(R.census.marksOffRowCentre === 0, R.mode + ": marks off row centre=" + R.census.marksOffRowCentre);
    r.expect(R.census.marksOrLinesOffTime === 0, R.mode + ": marks/lines off time=" + R.census.marksOrLinesOffTime);
    r.expect(!!R.collapse, R.mode + ": no collapsible parent row (.tl-disc) was found to test collapse/expand on");
    if (R.collapse) {
      r.expect(R.collapse.droppedMatches === true, R.mode + ": collapse dropped the wrong row count: " + JSON.stringify(R.collapse));
      r.expect(R.collapse.tracksMatchLabels === true, R.mode + ": collapsed rows/tracks mismatch");
      r.expect(R.collapse.reopened === true, R.mode + ": expanding again did not restore every row");
    }
    r.expect(R.pinned.labelXStill === true, R.mode + ": row labels moved while the chart scrolled");
    r.expect(R.pinned.axisTicksOffChart === 0, R.mode + ": axis ticks off chart=" + R.pinned.axisTicksOffChart);
    r.expect(R.pinned.axisUnderBar === true, R.mode + ": axis not pinned under the top bar");
    r.expect(R.pinned.firstRowUnderAxis === true, R.mode + ": first row not under the axis after scrolling");
    // On phone the chart is wider than the 390px view at the default zoom, so that's asserted directly; on desktop
    // (1280px) it may not be, so only the widened run (after widen() forced it wide) is asserted there.
    if (R.mode !== "desktop") r.expect(R.zoom.naturalWide === true, R.mode + ": chart is not wider than the view at the default zoom: " + JSON.stringify(R.zoom));
    r.expect(R.zoom.wideAfterWiden === true, R.mode + ": chart still isn't wider than the view after widen(): " + JSON.stringify(R.zoom));
    r.expect(R.zoom.centreShiftPxIn <= 1.5 && R.zoom.centreShiftPxOut <= 1.5, R.mode + ": zoom did not keep the centre time: " + JSON.stringify(R.zoom));
    r.expect(R.messagesToggle.hidden === true && R.messagesToggle.back === true, R.mode + ": messages toggle did not hide/restore");
    r.expect(R.tapTargets.under36 === 0, R.mode + ": tap targets under 36px: " + JSON.stringify(R.tapTargets.ex));
    if (R.taps) {
      r.expect(R.taps.tried > 0, R.mode + ": no tap was actually tried (picks empty or all occluded): " + JSON.stringify(R.taps));
      r.expect(R.taps.bad.length === 0, R.mode + ": bad taps: " + JSON.stringify(R.taps.bad.slice(0, 3)));
    }
    if (R.wheel) r.expect(R.wheel.axisWheelMoved !== 0 && R.wheel.shiftWheelMoved !== 0, R.mode + ": wheel scrolling did not move the chart: " + JSON.stringify(R.wheel));
  }

  return r.done();
}

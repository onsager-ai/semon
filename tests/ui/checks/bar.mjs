// Ported from the mockup's bar.js: the always-visible top bar, the Sessions page, and the four review fixes (the
// "You" header, gap markers, tables, tool summaries), across phone-light, phone-dark and desktop.
//
// The original ran 'sample' (test7.html) and 'real' (test-real.html) tagged, and screenshotted several real-only
// views (search, filter, details, the sessions page, the sidebar, a busy real session, a table, a You turn). Ported:
// 'real' and all of its screenshots are dropped; 'sample' now drives the served fixture. Session ids are the
// sample's, so no id mapping is needed here — the only sample-id reference was 'quill'/'h4' for a trace screenshot,
// and h4 (quill's inbound ask) is found at runtime the same way turns.mjs does: the served turn id for an incoming
// turn is that handoff's own (hashed) id.
//
// Assertions:
//  - no page errors, in any mode.
//  - the bar stays pinned at the top after scrolling (notPinned is empty), is never sideways (sideways === 0), and
//    every one of its controls is at least 36px (smallControls is empty) — the tap-target floor this suite measures.
//  - line 2's summary is one line (l2NotOneLine empty) and always has text-overflow: ellipsis on its 'rest' span
//    (l2NoEllipsis === 0), whether or not it's actually clipped.
//  - zero overflow screens.
//  - gap markers: this fixture's data has none at all (neither the sample nor the extras fixture produces a gap
//    marker), so rendered counts are asserted directly against the literal 0, not against a same-data expectation
//    that would also be 0 by construction (see the report for why this can't be a positive check here).
//  - tables: the sample fixture also has zero table messages, so "tables render" is checked as a positive count on
//    the extras fixture instead (harbor's markdown message there has a genuine table): extras.tables > 0. With no
//    unknown tool name in this fixture (expected.fallbackTools is empty), no turn summary falls back to a
//    lowercased tool name (tsumLowercasedUnknown === 0).
//  - search: the match count text agrees with the highlighted hit count (countMatchesHits === true), and closing
//    search restores the summary line.
//  - the filter dropdown opens without moving the transcript underneath it (contentMoved === false), narrows and
//    is undone, and closes both by its own button and by an outside tap; unchecking "tools" actually hides every
//    step (toolsOff.steps === 0), not just a `turns <= of` comparison that can't fail.
//  - the errors segment jumps to the first failed step, expanded and in view, with no menu open — and a session
//    with a failed step must actually be found, or this fails instead of silently not running.
//  - deep links land the target turn below the bar; a relay header's sender link opens the sender's turn — both
//    must actually be found (a trace child node, a Home item, a relay header), not silently skipped.
//  - the Sessions page shows exactly one row per lane, every grouping produces at least one section, search narrows
//    to at least the sessions the model predicts and empties on no match, and opening a row navigates to it.
import path from "node:path";
import { ENV, VIEWPORTS, settled, served, goto, data, reporter, overflow } from "../lib.mjs";

const isGap = (e) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
const TABLE = /^\s*\|.*\n\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/m;
const KNOWN = new Set(["Bash", "shell", "exec_command", "local_shell", "Grep", "Glob", "Read", "Edit", "MultiEdit", "Write", "apply_patch", "NotebookEdit", "AskUserQuestion", "ToolSearch", "SendMessage", "SendUserFile", "Agent", "Task", "Monitor", "ScheduleWakeup", "TaskStop", "Artifact", "WebFetch", "WebSearch", "Skill"]);
const afterTitle = (page, text) => page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t, text);

// The extras fixture (SEMON_EXTRA_BASE/SEMON_EXTRA_TOKEN): sample + a markdown message in harbor with a genuine
// table, used below because the sample fixture's own data has zero gap markers and zero tables (nothing to detect
// a false negative against), so the "tables rendered" half of the four-fixes check needs data that actually has one.
async function servedExtra(browser, { size = "phone", dark = false } = {}) {
  const base = (process.env.SEMON_EXTRA_BASE ?? "").replace(/\/$/, "");
  const token = process.env.SEMON_EXTRA_TOKEN ?? "";
  const ctx = await browser.newContext({ ...VIEWPORTS[size], colorScheme: dark ? "dark" : "light", timezoneId: "UTC", locale: "en-US", reducedMotion: "no-preference" });
  const page = await ctx.newPage();
  page.errors = [];
  page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
  page.setDefaultTimeout(5000);
  await page.clock.setFixedTime(ENV.now);
  await page.route(/.*/, (r) => (r.request().url().startsWith(base + "/") ? r.continue() : r.abort()));
  await page.goto(base + "/?t=" + token, { waitUntil: "load" });
  await settled(page);
  return page;
}

export default async function barCheck(browser) {
  const D = await data();
  const r = reporter("bar");
  const lanes = Object.values(D.SESS).filter((s) => s.lane);
  const X = { gapMarkers: 0, tableMessages: 0, fallbackTools: {} };
  for (const es of Object.values(D.TX)) for (const e of es) { if (isGap(e)) X.gapMarkers++; if ((e.k === "a" || e.k === "u") && TABLE.test(e.text)) X.tableMessages++; if (e.k === "tool" && !KNOWN.has(e.name) && !/^mcp__/.test(e.name)) X.fallbackTools[e.name] = (X.fallbackTools[e.name] ?? 0) + 1; }

  const modes = [];
  for (const mode of ["phone-light", "phone-dark", "desktop"]) {
    const phone = mode !== "desktop", dark = mode === "phone-dark";
    const page = await served(browser, { size: phone ? "phone" : "desktop", dark });
    const over = () => overflow(page);
    const barCheckOnce = () => page.evaluate(async () => {
      const main = document.querySelector("#main"), phone = matchMedia("(max-width: 760px)").matches;
      if (phone) window.scrollTo(0, document.documentElement.scrollHeight); else main.scrollTop = main.scrollHeight;
      await new Promise((r) => setTimeout(r, 120));
      const bar = document.querySelector("#topbar"), br = bar.getBoundingClientRect(), vw = document.documentElement.clientWidth;
      const scrolled = phone ? scrollY : main.scrollTop;
      const l2 = bar.querySelector(".l2"), rest = l2?.querySelector(".rest");
      const ctl = [...bar.querySelectorAll("button, input, [role=link]")].filter((x) => x.offsetParent || getComputedStyle(x).position === "absolute").map((x) => [x.className || x.tagName, x.getBoundingClientRect().height]);
      const small = ctl.filter(([, h]) => h < 35.5);
      const side = [...bar.querySelectorAll("*")].filter((x) => { const r = x.getBoundingClientRect(); return r.width && (r.right > vw + 0.5 || r.left < -0.5) && !x.closest(".rest"); }).length + (bar.scrollWidth > bar.clientWidth + 1 ? 1 : 0);
      const out = { pinned: Math.abs(br.top) < 0.5 && br.height > 30 && br.bottom > 0 && getComputedStyle(bar).visibility !== "hidden", scrolled: scrolled > 0, barH: Math.round(br.height), small: small.map(([c, h]) => c + ":" + Math.round(h)), side,
        l2: l2 ? { h: Math.round(l2.getBoundingClientRect().height), oneLine: l2.getBoundingClientRect().height < 24, ellipsis: rest ? getComputedStyle(rest).textOverflow === "ellipsis" : null, overflows: rest ? rest.scrollWidth > rest.clientWidth + 1 : false } : null };
      if (phone) window.scrollTo(0, 0); else main.scrollTop = 0; return out;
    });
    const R = { mode, pages: 0, notPinned: [], l2Pages: 0, l2NotOneLine: [], l2Overflowing: 0, l2NoEllipsis: 0, sideways: 0, smallControls: [], overflowScreens: 0 };
    const measure = async (name) => { const c = await barCheckOnce(); R.pages++; if (!c.pinned) R.notPinned.push(name + (c.scrolled ? "" : "(no scroll)")); if (c.l2) { R.l2Pages++; if (!c.l2.oneLine) R.l2NotOneLine.push(name + ":" + c.l2.h); if (c.l2.overflows) R.l2Overflowing++; if (c.l2.ellipsis === false) R.l2NoEllipsis++; } R.sideways += c.side; if (c.small.length) R.smallControls.push(name + " " + c.small.join(",")); if (await over()) R.overflowScreens++; return c; };
    const sids = Object.keys(D.SESS), traceTurns = [];
    const F = { youTurns: 0, youWithHeader: 0, msgTimes: 0, relayHeaders: 0, gapMarkersBetweenTurns: 0, gapMarkersInsideTurns: 0, tables: 0, tsum: 0, tsumFallback: [], tsumLowercasedUnknown: 0 };
    for (const sid of sids) {
      await goto(page, { v: "session", id: sid }, D);
      const info = await page.evaluate(() => ({ trace: [...document.querySelectorAll(".turn-end .tracebtn")].map((b) => b.closest(".turn").dataset.turn),
        you: [...document.querySelectorAll('.turn[aria-label^="Your message"]')].map((t) => !!t.querySelector(":scope > .turn-h")), times: document.querySelectorAll(".msg-tm").length, relays: document.querySelectorAll(".turn-h .from").length,
        gapOut: [...document.querySelectorAll(".turns > .divider")].filter((d) => /not included|omitted|No activity/.test(d.textContent)).length, gapIn: [...document.querySelectorAll(".turn .divider")].filter((d) => /not included|omitted|No activity/.test(d.textContent)).length,
        tables: document.querySelectorAll(".msg .tbl").length, tsum: [...document.querySelectorAll(".tsum .tt")].map((x) => x.textContent) }));
      info.trace.forEach((t) => traceTurns.push([sid, t]));
      F.youTurns += info.you.length; F.youWithHeader += info.you.filter(Boolean).length; F.msgTimes += info.times; F.relayHeaders += info.relays; F.gapMarkersBetweenTurns += info.gapOut; F.gapMarkersInsideTurns += info.gapIn; F.tables += info.tables; F.tsum += info.tsum.length;
      for (const t of info.tsum) { if (/ \d+ steps?\b/.test(t)) F.tsumFallback.push(t); if (/(^|, )[a-z]+[a-z0-9_]* \d+ steps?/.test(t) && !/^(ran|read|edited|wrote|patched|searched)/i.test(t)) F.tsumLowercasedUnknown++; }
      await measure("session " + D.SESS[sid].name.slice(0, 16));
    }
    for (const [sid, t] of traceTurns) { await goto(page, { v: "trace", sid, turn: t }, D); const c = await measure("trace " + t); if (!c.l2) R.traceWithoutL2 = (R.traceWithoutL2 ?? 0) + 1; }
    for (const m of Object.keys(D.MACHINE ?? {})) { await goto(page, { v: "machine", id: m }, D); await measure("machine " + m); }
    for (const v of ["home", "sessions", "machines"]) { await goto(page, { v }, D); await measure(v); }
    R.traces = traceTurns.length;
    R.smallControls = R.smallControls.slice(0, 6);
    const out = { ...R };
    if (mode !== "phone-dark") {
      // Search, filter and details on the busiest session.
      const busy = sids.sort((a, b) => (D.TX[b]?.length ?? 0) - (D.TX[a]?.length ?? 0))[0];
      await goto(page, { v: "session", id: busy }, D);
      const word = await page.evaluate(() => { const t = document.querySelector(".msg.assistant")?.textContent ?? ""; return (t.match(/[A-Za-z]{6,}/) ?? ["the"])[0].toLowerCase(); });
      const turnsBefore = await page.evaluate(() => document.querySelectorAll(".turns > .turn").length);
      await page.click('.topbar [aria-label="Find in transcript"]'); await page.waitForTimeout(100);
      const S = { session: D.SESS[busy].name, word, turnsBefore, searching: await page.evaluate(() => document.querySelector("#topbar").classList.contains("searching") && document.activeElement?.id === "find") };
      await page.keyboard.type(word, { delay: 10 }); await page.waitForTimeout(200);
      Object.assign(S, await page.evaluate(() => ({ count: document.querySelector(".fcount")?.textContent, hits: document.querySelectorAll(".turns .msg, .turns .step, .turns .hcard").length, turns: document.querySelectorAll(".turns > .turn").length, focus: document.activeElement?.id, barH: document.querySelector("#topbar").offsetHeight })));
      S.countMatchesHits = S.count === S.hits + (S.hits === 1 ? " match" : " matches");
      await page.click('.topbar [aria-label="Close search"]'); await page.waitForTimeout(150);
      Object.assign(S, await page.evaluate(() => ({ restored: !document.querySelector("#topbar").classList.contains("searching") && !!document.querySelector(".topbar .l2"), turnsAfter: document.querySelectorAll(".turns > .turn").length })));
      await page.keyboard.press("Escape");
      out.search = S;
      // Filter: a dropdown under its button; the content does not move while it opens, changes a toggle, and closes.
      await page.evaluate(() => window.scrollTo(0, 0)); await page.evaluate(() => { document.querySelector("#main").scrollTop = 0; });
      const firstTop = () => page.evaluate(() => Math.round(document.querySelector(".turns").getBoundingClientRect().top));
      const Fl = { before: await firstTop() };
      await page.click("#filter-btn"); await page.waitForTimeout(100);
      Object.assign(Fl, await page.evaluate(() => { const pop = document.querySelector(".filters.pop"), r = pop.getBoundingClientRect(), bt = document.querySelector("#filter-btn").getBoundingClientRect(), bar = document.querySelector("#topbar").getBoundingClientRect(); return { open: !pop.hidden, belowBar: r.top >= bar.bottom - 1, anchoredRight: Math.abs(r.right - bt.right) < 24, inView: r.left >= 0 && r.right <= innerWidth, labels: [...pop.querySelectorAll("label")].map((l) => Math.round(l.getBoundingClientRect().height)) }; }));
      Fl.whileOpen = await firstTop();
      const turnsAll = await page.evaluate(() => document.querySelectorAll(".turns > .turn").length);
      await page.click("#f-tools"); await page.waitForTimeout(150);
      Fl.toolsOff = { stillOpen: await page.evaluate(() => !document.querySelector(".filters.pop").hidden), turns: await page.evaluate(() => document.querySelectorAll(".turns > .turn").length), of: turnsAll, steps: await page.evaluate(() => document.querySelectorAll(".turns .step").length), btnMarked: await page.evaluate(() => document.querySelector("#filter-btn").classList.contains("on")) };
      await page.click("#f-tools"); await page.waitForTimeout(150);
      await page.click("#filter-btn"); await page.waitForTimeout(100); Fl.closedByButton = await page.evaluate(() => document.querySelector(".filters.pop").hidden);
      const vp = phone ? { width: 390, height: 844 } : { width: 1280, height: 860 };
      await page.click("#filter-btn"); await page.waitForTimeout(100); await page.mouse.click(phone ? 6 : 306, vp.height - 40); await page.waitForTimeout(150); Fl.closedByOutside = await page.evaluate(() => document.querySelector(".filters.pop")?.hidden ?? true);
      Fl.after = await firstTop(); Fl.contentMoved = Fl.before !== Fl.whileOpen || Fl.before !== Fl.after;
      out.filter = Fl;
      // Details from line 2 (a tap on the summary text lands on the title block's button).
      await page.evaluate(() => { window.scrollTo(0, 400); document.querySelector("#main").scrollTop = 400; }); await page.waitForTimeout(100);
      { const bb = await page.locator(".topbar .l2 .rest").boundingBox(); if (bb) await page.mouse.click(bb.x + bb.width / 2, bb.y + bb.height / 2); } await page.waitForTimeout(120);
      out.details = await page.evaluate(() => { const m = document.querySelector(".menu"); if (!m) return { open: false }; const r = m.getBoundingClientRect(), bar = document.querySelector("#topbar").getBoundingClientRect(); return { open: true, inView: r.top >= bar.bottom - 1 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth, rows: [...m.querySelectorAll("dt")].map((d) => d.textContent) }; });
      await page.mouse.click(phone ? 6 : 306, vp.height - 40); await page.waitForTimeout(100); out.details.closedByOutside = await page.evaluate(() => !document.querySelector(".menu"));
      // The errors segment keeps its jump: the first failed step, expanded, in view; no menu.
      const errSid = sids.find((s) => (D.TX[s] ?? []).some((e) => e.k === "tool" && e.ok === false));
      r.expect(!!errSid, "no session with a failed tool call to test the errors-segment jump on");
      if (errSid) { await goto(page, { v: "session", id: errSid }, D); await page.click(".topbar .errs"); await page.waitForTimeout(700);
        out.errsJump = { session: D.SESS[errSid].name, ...(await page.evaluate(() => { const e = document.querySelector(".step.err"), r = e.getBoundingClientRect(), bar = document.querySelector("#topbar").getBoundingClientRect(); return { label: document.querySelector(".topbar .errs").textContent, expanded: e.querySelector("button")?.getAttribute("aria-expanded"), inView: r.top >= bar.bottom - 1 && r.top < innerHeight, menu: !!document.querySelector(".menu") }; })) }; }
      // Deep links land the turn fully below the bar.
      const landed = () => page.evaluate(() => { const st = history.state, t = [...document.querySelectorAll(".turn")].find((x) => x.dataset.turn === st?.turn), bar = document.querySelector("#topbar").getBoundingClientRect(); if (!t) return { found: false }; const r = t.getBoundingClientRect(); return { found: true, turn: st.turn, top: Math.round(r.top), barBottom: Math.round(bar.bottom), barTop: Math.round(bar.top), belowBar: r.top >= bar.bottom - 0.5 && r.top < innerHeight - 40, flash: t.classList.contains("flash") }; });
      const DL = {};
      let pick = null; for (const [sid, t] of traceTurns) { if (pick) break; await goto(page, { v: "trace", sid, turn: t }, D); const n = await page.evaluate(() => [...document.querySelectorAll(".hop.child[data-turn] .open")].length); if (n) pick = [sid, t]; }
      if (pick) { await goto(page, { v: "trace", sid: pick[0], turn: pick[1] }, D); const lastNode = await page.evaluate(() => { const n = [...document.querySelectorAll(".hop.child[data-turn]")].at(-1); return n.dataset.h; }); await page.click('.hop.child[data-h="' + lastNode + '"] .open'); await page.waitForTimeout(300); DL.fromTrace = { node: lastNode, ...(await landed()) }; await page.goBack(); await page.waitForTimeout(150); DL.fromTrace.backTo = await page.evaluate(() => history.state?.v); }
      await goto(page, { v: "home" }, D);
      if (await page.locator(".ib .q").count()) { await page.click(".ib .q"); await page.waitForTimeout(300); DL.fromHome = await landed(); await page.goBack(); await page.waitForTimeout(150); DL.fromHome.backTo = await page.evaluate(() => history.state?.v); }
      out.deepLinks = DL;
      // A relay header's sender link opens the sender's turn.
      const relaySid = sids.find((s) => D.TX[s]?.some((e) => e.k === "h" && D.H.find((h) => h.id === e.id && h.to === s && h.kind === "relay")));
      if (relaySid) { await goto(page, { v: "session", id: relaySid }, D); const has = await page.$(".turn-h .from"); if (has) { await has.click(); await page.waitForTimeout(250); out.relayHeaderLink = await page.evaluate(() => ({ v: history.state?.v, id: history.state?.id?.slice(0, 8), turn: history.state?.turn ?? null })); } }
      // Sessions page: rows = top-level sessions; each grouping; search; the sidebar's short list.
      await goto(page, { v: "sessions" }, D);
      const SP = { expected: lanes.length, rows: await page.evaluate(() => document.querySelectorAll(".page .nrow").length), nav: await page.evaluate(() => [...document.querySelectorAll(".nav-item")].map((n) => n.textContent + (n.getAttribute("aria-current") ? "*" : ""))) };
      SP.groups = {};
      for (const g of ["project", "machine", "harness", "recent"]) { await page.click('.page .groupby button[data-g="' + g + '"]'); await page.waitForTimeout(80); SP.groups[g] = await page.evaluate(() => ({ heads: [...document.querySelectorAll(".page .sess .sec-h")].map((h) => h.textContent), rows: document.querySelectorAll(".page .nrow").length, pressed: document.querySelector(".page .groupby [aria-pressed=\"true\"]")?.dataset.g })); }
      const term = lanes[0].name.slice(0, 4).toLowerCase();
      const expect = lanes.filter((s) => [s.name, s.repo, s.branch, D.MACHINE[s.machine], s.movedFrom ? D.MACHINE[s.movedFrom] : "", { claude: "Claude Code", codex: "Codex" }[s.harness], s.role ? "role no repo" : ""].join(" ").toLowerCase().includes(term) || (D.TX[s.id] ?? []).some((e) => (e.k === "h" && (() => { const h = D.H.find((x) => x.id === e.id); return h && h.to === s.id && ["ask", "relay", "spawn"].includes(h.kind) && h.brief.toLowerCase().includes(term); })()) || (e.k === "u" && e.text.toLowerCase().includes(term)))).length;
      await page.fill("#sq", term); await page.waitForTimeout(100);
      SP.search = { term, rows: await page.evaluate(() => document.querySelectorAll(".page .nrow").length), expectedAtLeast: expect };
      await page.fill("#sq", "zzqqxx"); await page.waitForTimeout(80); SP.search.none = await page.evaluate(() => document.querySelector(".page .empty")?.textContent);
      await page.fill("#sq", ""); await page.waitForTimeout(80);
      await page.click(".page .nrow"); await page.waitForTimeout(150); SP.rowOpens = await page.evaluate(() => ({ v: history.state?.v, navCurrent: document.querySelector(".nav-item[aria-current]")?.dataset.go }));
      await goto(page, { v: "sessions" }, D);
      if (phone) {
        await page.click("#lead-btn"); await page.waitForTimeout(300);
        SP.sidebar = await page.evaluate(() => ({ head: document.querySelector(".side-h")?.textContent, rows: document.querySelectorAll("#lanes .srow").length, all: document.querySelector("#all-sessions")?.textContent, chips: document.querySelectorAll(".sidebar .groupby").length }));
        await page.fill("#q", term); await page.waitForTimeout(80); SP.sidebar.typedRows = await page.evaluate(() => document.querySelectorAll("#lanes .srow").length);
        await page.press("#q", "Enter"); await page.waitForTimeout(300); SP.sidebar.enter = await page.evaluate(() => ({ v: history.state?.v, pageQuery: document.querySelector("#sq")?.value, rows: document.querySelectorAll(".page .nrow").length, drawerClosed: !document.body.classList.contains("drawer-open") }));
        await page.fill("#sq", ""); await page.waitForTimeout(80);
      } else SP.sidebar = await page.evaluate(() => ({ rows: document.querySelectorAll("#lanes .srow").length, all: document.querySelector("#all-sessions")?.textContent }));
      out.sessionsPage = SP;
    }
    out.fixes = { ...F, tsumFallback: F.tsumFallback.slice(0, 8), expected: X };
    if (mode !== "desktop") { const h4 = D.H.find((h) => h.kind === "ask" && h.to === "quill")?.id; r.expect(!!h4, mode + ": quill's inbound ask handoff was not found (needed for the trace screenshot)"); if (h4) { await goto(page, { v: "session", id: "quill" }, D); await page.click('.turn[data-turn="' + h4 + '"] .tracebtn'); await page.waitForTimeout(200); await page.screenshot({ path: path.join(ENV.out, "bar-sample-trace" + (dark ? "-dark" : "") + ".png") }); } }
    out.errors = page.errors;
    modes.push(out);
    await page.context().close();
  }

  // Tables, on the extras fixture: the sample fixture's own data has zero table messages (X.tableMessages === 0 there
  // always, so a rendered-vs-expected equality on it can never catch a missing table, only a spurious one). Harbor's
  // extras-only markdown message has a genuine table, so this is where "tables render" gets a positive check.
  let extra = null;
  if (process.env.SEMON_EXTRA_BASE) {
    const page = await servedExtra(browser, { size: "phone" });
    await page.evaluate(() => { history.pushState({ v: "session", id: "harbor" }, ""); dispatchEvent(new PopStateEvent("popstate", { state: { v: "session", id: "harbor" } })); });
    await page.waitForFunction(() => !!document.querySelector("#page section[aria-label='Transcript']"));
    await page.evaluate(() => { for (let k = 0; k < 3; k++) document.querySelectorAll('.cw-toggle[aria-expanded="false"]').forEach((x) => x.click()); document.querySelectorAll(".hcard .more:not([hidden])").forEach((x) => x.click()); });
    await page.waitForTimeout(150);
    extra = { tables: await page.evaluate(() => document.querySelectorAll(".msg .tbl").length), errors: page.errors };
    await page.context().close();
  }

  r.results = { modes, expected: X, extra };
  r.expect(!!process.env.SEMON_EXTRA_BASE, "SEMON_EXTRA_BASE not set: the tables-render positive check did not run");
  if (extra) {
    r.expect(extra.errors.length === 0, "extras: page errors: " + extra.errors.join(" | "));
    r.expect(extra.tables > 0, "extras: harbor's markdown message has a table in its source but rendered tables=" + extra.tables);
  }

  for (const m of modes) {
    r.expect(m.errors.length === 0, m.mode + ": page errors: " + m.errors.join(" | "));
    r.expect(m.notPinned.length === 0, m.mode + ": bar not pinned: " + JSON.stringify(m.notPinned));
    r.expect(m.sideways === 0, m.mode + ": bar sideways=" + m.sideways);
    r.expect(m.smallControls.length === 0, m.mode + ": controls under 36px: " + JSON.stringify(m.smallControls));
    r.expect(m.l2NotOneLine.length === 0, m.mode + ": l2 not one line: " + JSON.stringify(m.l2NotOneLine));
    r.expect(m.l2NoEllipsis === 0, m.mode + ": l2 missing ellipsis count=" + m.l2NoEllipsis);
    r.expect(m.overflowScreens === 0, m.mode + ": overflowScreens=" + m.overflowScreens);
    // Gap markers: this fixture's own data has none (X.gapMarkers is always 0 here — every fixture available to this
    // suite is gap-free, per the extras fixture's own description), so there is no source of a positive count to
    // check against; asserted directly against the literal 0 rather than against X.gapMarkers, so a bug that made
    // both sides wrong the same way can't hide behind an equality of two derived numbers. See the report for why
    // this can't be made a positive check without a fixture that actually has a gap.
    r.expect(m.fixes.gapMarkersBetweenTurns === 0 && m.fixes.gapMarkersInsideTurns === 0, m.mode + ": gap markers rendered where the data has none: between=" + m.fixes.gapMarkersBetweenTurns + " inside=" + m.fixes.gapMarkersInsideTurns);
    r.expect(m.fixes.tables === X.tableMessages, m.mode + ": tables rendered=" + m.fixes.tables + " expected=" + X.tableMessages + " (both 0 on this fixture; see the extras check above for a positive count)");
    if (Object.keys(X.fallbackTools).length === 0) r.expect(m.fixes.tsumLowercasedUnknown === 0, m.mode + ": unexpected lowercased-unknown tool summaries=" + m.fixes.tsumLowercasedUnknown);
    // Search, filter, details, deep links, the relay-header link and the Sessions page are only exercised once
    // (mode !== "phone-dark", matching the original mockup script) — not on the phone-dark pass.
    if (m.mode !== "phone-dark") {
      r.expect(!!m.search, m.mode + ": the search test never ran");
      if (m.search) {
        r.expect(m.search.countMatchesHits === true, m.mode + ": search count/hits mismatch: " + JSON.stringify(m.search));
        r.expect(m.search.restored === true, m.mode + ": closing search did not restore the summary line");
      }
      r.expect(!!m.filter, m.mode + ": the filter test never ran");
      if (m.filter) {
        r.expect(m.filter.contentMoved === false, m.mode + ": filter dropdown moved the transcript");
        r.expect(m.filter.closedByButton === true && m.filter.closedByOutside === true, m.mode + ": filter did not close: " + JSON.stringify(m.filter));
        // A real check, not just turns <= of (always true): the tools-off filter must hide every step, whether or
        // not that also drops a whole turn.
        r.expect(m.filter.toolsOff.steps === 0, m.mode + ": tools-off filter left steps visible: " + m.filter.toolsOff.steps);
      }
      r.expect(!!m.errsJump, m.mode + ": no session with a failed tool call was found for the errors-jump test");
      if (m.errsJump) {
        r.expect(m.errsJump.expanded === "true", m.mode + ": errors-jump step not expanded: " + m.errsJump.expanded);
        r.expect(m.errsJump.inView === true, m.mode + ": errors-jump step not in view");
        r.expect(m.errsJump.menu === false, m.mode + ": errors-jump left a menu open");
      }
      r.expect(!!m.details, m.mode + ": the details-menu test never ran");
      if (m.details) r.expect(m.details.open === true, m.mode + ": details menu did not open on tapping the summary text");
      if (m.details?.open) {
        r.expect(m.details.inView === true, m.mode + ": details menu not in view");
        r.expect(m.details.closedByOutside === true, m.mode + ": details menu did not close on outside tap");
      }
      r.expect(!!m.deepLinks?.fromTrace, m.mode + ": no fromTrace deep link was tried (no trace child node found)");
      r.expect(!!m.deepLinks?.fromHome, m.mode + ": no fromHome deep link was tried (no Home item found)");
      if (m.deepLinks?.fromTrace) r.expect(m.deepLinks.fromTrace.belowBar === true, m.mode + ": fromTrace deep link not below bar");
      if (m.deepLinks?.fromHome) r.expect(m.deepLinks.fromHome.belowBar === true, m.mode + ": fromHome deep link not below bar");
      r.expect(!!m.relayHeaderLink, m.mode + ": no relay-header sender link was found to click");
      if (m.relayHeaderLink) r.expect(m.relayHeaderLink.v === "session", m.mode + ": relay header link did not open a session: " + JSON.stringify(m.relayHeaderLink));
      r.expect(!!m.sessionsPage, m.mode + ": the Sessions page test never ran");
      if (m.sessionsPage) r.expect(m.sessionsPage.rows === m.sessionsPage.expected, m.mode + ": sessions rows=" + m.sessionsPage.rows + " expected=" + m.sessionsPage.expected);
      if (m.sessionsPage?.search) {
        r.expect(m.sessionsPage.search.rows >= m.sessionsPage.search.expectedAtLeast, m.mode + ": sessions search too narrow: " + JSON.stringify(m.sessionsPage.search));
        r.expect(!!m.sessionsPage.search.none, m.mode + ": no-match sessions search did not show empty state");
      }
      if (m.sessionsPage?.rowOpens) r.expect(m.sessionsPage.rowOpens.v === "session", m.mode + ": opening a sessions row did not navigate to a session");
    }
  }

  return r.done();
}

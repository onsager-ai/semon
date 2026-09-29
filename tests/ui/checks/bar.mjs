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
//    controls are at least 36px, except line 2 which uses the approved 30px desktop / 44px phone hit area and the
//    inline error link which uses 28px desktop / 40px phone.
//  - line 2 fits in one row; session metadata is ordered state/model/machine/branch/tools/tokens, while other detail
//    pages keep their ellipsized summary.
//  - zero overflow screens.
//  - gap markers: this fixture's data has none at all (neither the sample nor the extras fixture produces a gap
//    marker), so rendered counts are asserted directly against the literal 0, not against a same-data expectation
//    that would also be 0 by construction (see the report for why this can't be a positive check here).
//  - tables: the sample fixture also has zero table messages, so "tables render" is checked as a positive count on
//    the extras fixture instead (harbor's markdown message there has a genuine table): extras.tables > 0. With no
//    unknown tool name in this fixture (expected.fallbackTools is empty), no turn summary falls back to a
//    lowercased tool name (tsumLowercasedUnknown === 0).
//  - search: on phones, Find and Filter live in ⋯; the match count agrees with the hits and closing search restores line 2.
//  - the filter dropdown opens without moving the transcript underneath it (contentMoved === false), narrows and
//    is undone, and closes both by its own button and by an outside tap; unchecking "tools" actually hides every
//    step (toolsOff.steps === 0), not just a `turns <= of` comparison that can't fail.
//  - the errors segment jumps to the first failed step, expanded and in view, with no menu open — and a session
//    with a failed step must actually be found, or this fails instead of silently not running.
//  - deep links land the target turn below the bar; a relay header's sender link opens the sender's turn — both
//    must actually be found (a trace child node, a Home item, a relay header), not silently skipped.
//  - the sidebar shows the 8 most recent top-level tree rows with nested children; the Sessions page keeps all session rows,
//    every grouping produces sections, search narrows to model matches, and opening a row lands at the end.
import path from "node:path";
import { ENV, VIEWPORTS, settled, served, goto, data, reporter, overflow } from "../lib.mjs";

const isGap = (e) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
const TABLE = /^\s*\|.*\n\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/m;
const KNOWN = new Set(["Bash", "shell", "exec_command", "local_shell", "write_stdin", "Grep", "Glob", "Read", "Edit", "MultiEdit", "Write", "apply_patch", "NotebookEdit", "AskUserQuestion", "ToolSearch", "SendMessage", "SendUserFile", "Agent", "Task", "Monitor", "ScheduleWakeup", "TaskStop", "Artifact", "WebFetch", "WebSearch", "Skill"]);
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
  const parentOf = (sid) => D.SESS[sid]?.parent ?? D.H.find((h) => h.kind === "spawn" && h.to === sid)?.from;
  const allSessions = Object.values(D.SESS), lanes = allSessions.filter((s) => s.lane);
  const childrenOf = (sid) => allSessions.filter((s) => parentOf(s.id) === sid);
  const roots = allSessions.filter((s) => !parentOf(s.id) && s.lane).sort((a, b) => b.last - a.last);
  const shownRoots = new Set(roots.slice(0, 8).map((s) => s.id));
  const treeRoot = (sid) => { const seen = new Set(); while (parentOf(sid) && !seen.has(sid)) { seen.add(sid); sid = parentOf(sid); } return sid; };
  const expectedTreeRows = allSessions.filter((s) => shownRoots.has(treeRoot(s.id))).length;
  const treePair = D.H.find((h) => h.kind === "spawn" && roots.slice(0, 8).some((s) => s.id === h.from) && D.SESS[h.to]) ?? null;
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
      const l2 = bar.querySelector(".l2"), rest = l2?.querySelector(".rest"), sessionMeta = !!l2?.classList.contains("session-meta");
      const ctl = [...bar.querySelectorAll("button, input, [role=link]")].filter((x) => x.offsetParent || getComputedStyle(x).position === "absolute").map((x) => [x.className || x.tagName, x.getBoundingClientRect().height]);
      const small = ctl.filter(([c, h]) => h < 35.5 && !String(c).split(/\s+/).some((x) => x === "errs" || x === "meta-runs"));
      const errorsTarget = bar.querySelector(".l2.session-meta .errs"), runsTarget = bar.querySelector(".l2.session-meta .meta-runs");
      const line2Targets = { errors: errorsTarget ? Math.round(errorsTarget.getBoundingClientRect().height) : null, runs: runsTarget ? Math.round(runsTarget.getBoundingClientRect().height) : null };
      // The session line: the state dot is whole and on the title's left edge, no run dot, no whole-line button, quiet errors.
      let metaFacts = null;
      if (sessionMeta) {
        const dot = l2.querySelector(".meta-state > .dot"), dr = dot?.getBoundingClientRect(), lr = l2.getBoundingClientRect(), tr = bar.querySelector(".l1").getBoundingClientRect(), first = l2.firstElementChild?.getBoundingClientRect(), dotFirst = !!l2.firstElementChild?.classList.contains("meta-state"), errs = l2.querySelector(".errs"), tools = l2.querySelector(".meta-tools");
        metaFacts = { dot: !!dot, dotFirst, ringRoom: dr && dotFirst ? Math.round((dr.left - lr.left) * 10) / 10 : null, dotInside: dr ? dr.left >= 0 && dr.left >= lr.left && dr.right <= lr.right && dr.top >= lr.top - 0.5 && dr.bottom <= lr.bottom + 0.5 : null, firstOffTitle: first ? Math.round((first.left - tr.left) * 10) / 10 : null,
          runsDot: !!l2.querySelector(".meta-runs .dot"), hit: bar.querySelectorAll(".meta-hit").length, errsWeight: errs ? Number(getComputedStyle(errs).fontWeight) : null, toolsWeight: tools ? Number(getComputedStyle(tools).fontWeight) : null, errsColor: errs ? getComputedStyle(errs).color : null, toolsColor: tools ? getComputedStyle(tools).color : null,
          buttons: [...l2.querySelectorAll("button")].map((b) => b.className) };
      }
      const side = [...bar.querySelectorAll("*")].filter((x) => { const r = x.getBoundingClientRect(); return r.width && (r.right > vw + 0.5 || r.left < -0.5) && !x.closest(".rest"); }).length + (bar.scrollWidth > bar.clientWidth + 1 ? 1 : 0);
      const out = { pinned: Math.abs(br.top) < 0.5 && br.height > 30 && br.bottom > 0 && getComputedStyle(bar).visibility !== "hidden", scrolled: scrolled > 0, barH: Math.round(br.height), small: small.map(([c, h]) => c + ":" + Math.round(h)), side,
        line2Targets, metaFacts, line2Hidden: { errors: !!errorsTarget?.hidden, runs: !!runsTarget?.hidden },
        l2: l2 ? { h: Math.round(l2.getBoundingClientRect().height), oneLine: l2.scrollHeight <= l2.clientHeight + 1, sessionMeta, metaOrder: [...l2.querySelectorAll(":scope > .meta-item")].map((x) => [...x.classList].find((c) => c.startsWith("meta-") && c !== "meta-item")), ellipsis: rest ? getComputedStyle(rest).textOverflow === "ellipsis" : null, overflows: l2.scrollWidth > l2.clientWidth + 1 } : null };
      if (phone) window.scrollTo(0, 0); else main.scrollTop = 0; return out;
    });
    const R = { mode, pages: 0, notPinned: [], l2Pages: 0, l2NotOneLine: [], l2Overflowing: 0, l2NoEllipsis: 0, metaOrderFailures: [], metaDotFailures: [], sessionMetaPages: 0, sideways: 0, smallControls: [], line2TargetFailures: [], overflowScreens: 0 };
    const measure = async (name, expectedMeta = null) => { const c = await barCheckOnce(); R.pages++; if (!c.pinned) R.notPinned.push(name + (c.scrolled ? "" : "(no scroll)")); if (c.l2) { R.l2Pages++; if (!c.l2.oneLine) R.l2NotOneLine.push(name + ":" + c.l2.h); if (c.l2.overflows) R.l2Overflowing++; if (c.l2.ellipsis === false) R.l2NoEllipsis++; if (c.l2.sessionMeta) { R.sessionMetaPages++; const want = ["meta-kind", "meta-state", "meta-model", "meta-machine", "meta-branch", "meta-tools", "meta-runs", "meta-tokens", "meta-cost"], got = c.l2.metaOrder, required = [...(expectedMeta?.kind ? ["meta-kind"] : []), "meta-state", "meta-model", "meta-machine", "meta-branch", "meta-tools", ...(expectedMeta?.runs ? ["meta-runs"] : []), "meta-tokens", "meta-cost"], ordered = got.every((x) => want.includes(x)) && got.every((x, i) => i === 0 || want.indexOf(got[i - 1]) < want.indexOf(x)); if (!ordered || required.some((x) => !got.includes(x))) R.metaOrderFailures.push(name + ":" + JSON.stringify(got)); const sizes = { errors: phone ? 40 : 28, runs: phone ? 44 : 30 }, needed = { errors: !!expectedMeta?.errors, runs: !!expectedMeta?.runs }; for (const key of Object.keys(sizes)) { const h = c.line2Targets[key]; if (needed[key] && h == null) R.line2TargetFailures.push(name + ": missing expected line-2 " + key + " target"); else if (h != null && !c.line2Hidden[key] && Math.abs(h - sizes[key]) > 1) R.line2TargetFailures.push(name + " " + key + ":" + h + "px, expected " + sizes[key] + "px"); else if (key === "runs" && needed.runs && c.line2Hidden.runs && !phone) R.line2TargetFailures.push(name + ": runs are hidden outside the phone menu"); } } } const mf = c.metaFacts; if (mf) { if (!mf.dot || !mf.dotInside || (mf.dotFirst && mf.ringRoom < 5.5) || Math.abs(mf.firstOffTitle) > 1) R.metaDotFailures.push(name + ": " + JSON.stringify({ first: mf.dotFirst, ringRoom: mf.ringRoom, inside: mf.dotInside, offTitle: mf.firstOffTitle })); if (mf.errsColor != null && mf.errsColor !== mf.toolsColor) R.metaDotFailures.push(name + ": errors are " + mf.errsColor + " where the other items are " + mf.toolsColor); if (mf.runsDot) R.metaDotFailures.push(name + ": the runs item has a state dot"); if (mf.hit) R.metaDotFailures.push(name + ": a whole-line .meta-hit button is back"); if (mf.errsWeight != null && mf.errsWeight > mf.toolsWeight) R.metaDotFailures.push(name + ": errors weigh " + mf.errsWeight + " over the other items' " + mf.toolsWeight); if (mf.buttons.some((b) => !/\b(errs|meta-runs)\b/.test(b))) R.metaDotFailures.push(name + ": a meta item other than errors and runs is a button: " + JSON.stringify(mf.buttons)); } R.sideways += c.side; if (c.small.length) R.smallControls.push(name + " " + c.small.join(",")); if (await over()) R.overflowScreens++; return c; };
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
      await measure("session " + D.SESS[sid].name.slice(0, 16), { kind: !!D.SESS[sid].kind, runs: childrenOf(sid).length > 0, errors: (D.TX[sid] ?? []).some((e) => e.k === "tool" && e.ok === false) });
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
      let phoneMenu = null;
      if (phone) { await page.click("#more-btn"); phoneMenu = await page.evaluate(() => [...document.querySelectorAll(".menu [role=menuitem]")].map((x) => x.textContent.trim())); await page.locator(".menu [role=menuitem]").filter({ hasText: "Find in transcript" }).click(); }
      else await page.click('.topbar [aria-label="Find in transcript"]');
      await page.waitForTimeout(100);
      const S = { session: D.SESS[busy].name, word, turnsBefore, phoneMenu, searching: await page.evaluate(() => document.querySelector("#topbar").classList.contains("searching") && document.activeElement?.id === "find") };
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
      const openFilter = async () => { if (phone) { await page.click("#more-btn"); await page.locator(".menu [role=menuitem]").filter({ hasText: "Filter transcript" }).click(); } else await page.click("#filter-btn"); };
      const Fl = { before: await firstTop() };
      await openFilter(); await page.waitForTimeout(100);
      Object.assign(Fl, await page.evaluate((phone) => { const pop = document.querySelector(".filters.pop"), r = pop.getBoundingClientRect(), bt = document.querySelector(phone ? "#more-btn" : "#filter-btn").getBoundingClientRect(), bar = document.querySelector("#topbar").getBoundingClientRect(); return { open: !pop.hidden, belowBar: r.top >= bar.bottom - 1, anchoredRight: Math.abs(r.right - bt.right) < 24, inView: r.left >= 0 && r.right <= innerWidth, labels: [...pop.querySelectorAll("label")].map((l) => Math.round(l.getBoundingClientRect().height)) }; }, phone));
      Fl.whileOpen = await firstTop();
      const turnsAll = await page.evaluate(() => document.querySelectorAll(".turns > .turn").length);
      await page.click("#f-tools"); await page.waitForTimeout(150);
      Fl.toolsOff = { stillOpen: await page.evaluate(() => !document.querySelector(".filters.pop").hidden), turns: await page.evaluate(() => document.querySelectorAll(".turns > .turn").length), of: turnsAll, steps: await page.evaluate(() => document.querySelectorAll(".turns .step").length), btnMarked: await page.evaluate((phone) => document.querySelector(phone ? "#more-btn" : "#filter-btn").classList.contains("on"), phone) };
      await page.click("#f-tools"); await page.waitForTimeout(150);
      await openFilter(); await page.waitForTimeout(100); Fl.closedByButton = await page.evaluate(() => document.querySelector(".filters.pop").hidden);
      const vp = phone ? { width: 390, height: 844 } : { width: 1280, height: 860 };
      await openFilter(); await page.waitForTimeout(100); await page.mouse.click(phone ? 6 : 306, vp.height - 40); await page.waitForTimeout(150); Fl.closedByOutside = await page.evaluate(() => document.querySelector(".filters.pop")?.hidden ?? true);
      Fl.after = await firstTop(); Fl.contentMoved = Fl.before !== Fl.whileOpen || Fl.before !== Fl.after;
      out.filter = Fl;
      // Line 2 opens the Session details sheet/dialog with its API-equivalent cost breakdown.
      const detailsSid = Object.values(D.SESS).find((s) => s.reported_runs?.length)?.id ?? busy;
      if (detailsSid !== busy) await goto(page, { v: "session", id: detailsSid }, D);
      await page.evaluate(() => { window.scrollTo(0, 400); document.querySelector("#main").scrollTop = 400; }); await page.waitForTimeout(100);
      // Clicking a plain meta item (a badge with a tooltip) does nothing: no dialog, no menu, no navigation.
      // Real pointer clicks at each badge's centre (the state badge's dot, since its row also holds the errors button), so an
      // overlay under a badge is caught: the badge must be the element under the pointer, and the click must do nothing.
      const badges = await page.evaluate(() => [...document.querySelectorAll("#topbar .l2.session-meta .meta-item:not(button)")].map((item) => { const t = item.querySelector(":scope > .dot") ?? item, r = t.getBoundingClientRect(), x = r.left + r.width / 2, y = r.top + r.height / 2; return { shown: r.width > 0 && item.getClientRects().length > 0, x, y, titled: !!item.title, underPointer: r.width > 0 && item.contains(document.elementFromPoint(x, y)) }; }));
      const before = await page.evaluate(() => ({ depth: history.length, route: JSON.stringify(history.state) }));
      for (const b of badges) if (b.shown) await page.mouse.click(b.x, b.y);
      await page.waitForTimeout(120);
      const inert = await page.evaluate((before) => ({ dialog: !!document.querySelector("dialog[open]"), menu: !!document.querySelector(".menu"), moved: history.length !== before.depth || JSON.stringify(history.state) !== before.route }), before);
      Object.assign(inert, { items: badges.length, clicked: badges.filter((b) => b.shown).length, titled: badges.every((b) => b.titled), overlaid: badges.filter((b) => b.shown && !b.underPointer).length });
      out.inertBadges = inert;
      // Details are reached from the ⋯ menu (PR B of the top-bar work redesigns them).
      await page.click("#more-btn"); await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click(); await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true);
      out.details = await page.evaluate((phone) => {
        const d = document.querySelector("dialog.session-details"), r = d.getBoundingClientRect(), labels = [...d.querySelectorAll(".detail-label")].map((x) => x.textContent), cost = d.querySelector(".cost-row");
        cost?.click();
        const normalize = (text) => text.replace(/\s+/g, " ").trim();
        const breakdown = d.querySelector(".cost-breakdown"), breakdownText = normalize(breakdown?.textContent ?? "");
        const breakdownModels = [...(breakdown?.querySelectorAll(".cost-model") ?? [])].map((group) => ({
          model: group.querySelector(".cost-model-name")?.textContent,
          labels: [...group.querySelectorAll(".cost-line span:first-child")].map((x) => normalize(x.textContent)),
        }));
        const kinds = ["Input", "Output", "Cache read", "Cache write · 5m", "Cache write · 1h", "Web search"];
        return {
          open: d.open, inView: r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth,
          phoneSheet: phone ? r.bottom >= innerHeight - 1 && r.width >= innerWidth - 1 : null,
          desktopDialog: phone ? null : r.width <= 680, labels,
          hasCost: labels.includes("API-equivalent cost"), costExpanded: cost?.getAttribute("aria-expanded") === "true",
          costBreakdown: !!breakdown && !breakdown.hidden && breakdownModels.length > 0 && kinds.every((kind) => breakdownModels.some((group) => group.labels.includes(kind))),
          breakdownModels, breakdownText, hasSessionId: labels.includes("Session id"), hasDirectory: labels.includes("Directory"),
          hasPid: labels.includes("Process id"), reported: d.querySelector(".reported-cost")?.textContent ?? null,
          costWarning: d.querySelector(".cost-warning")?.textContent ?? null,
        };
      }, phone);
      out.details.session = detailsSid;
      await page.click(".session-details .vclose"); await page.waitForTimeout(100); out.details.closed = await page.evaluate(() => !document.querySelector("dialog.session-details"));
      // Opened from the menu and dismissed with Escape, focus goes back to the ⋯ button, not the document body.
      await page.click("#more-btn"); await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click(); await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true);
      await page.keyboard.press("Escape"); await page.waitForTimeout(150);
      out.details.escFocus = await page.evaluate(() => ({ closed: !document.querySelector("dialog.session-details"), focus: document.activeElement?.id ?? document.activeElement?.tagName }));
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
      const SP = { expected: allSessions.length, expectedRoots: Math.min(8, roots.length), expectedTreeRows, rows: await page.evaluate(() => document.querySelectorAll(".page .nrow").length), nav: await page.evaluate(() => [...document.querySelectorAll(".nav-item")].map((n) => n.textContent + (n.getAttribute("aria-current") ? "*" : ""))) };
      SP.groups = {};
      for (const g of ["project", "machine", "harness", "recent"]) { await page.click('.page .groupby button[data-g="' + g + '"]'); await page.waitForTimeout(80); SP.groups[g] = await page.evaluate(() => ({ heads: [...document.querySelectorAll(".page .sess .sec-h")].map((h) => h.textContent), rows: document.querySelectorAll(".page .nrow").length, pressed: document.querySelector(".page .groupby [aria-pressed=\"true\"]")?.dataset.g })); }
      const term = roots[0].name.slice(0, 4).toLowerCase();
      const expect = allSessions.filter((s) => [s.name, s.repo, s.branch, D.MACHINE[s.machine], s.movedFrom ? D.MACHINE[s.movedFrom] : "", { claude: "Claude Code", codex: "Codex" }[s.harness], s.role ? "role no repo" : ""].join(" ").toLowerCase().includes(term) || (D.TX[s.id] ?? []).some((e) => (e.k === "h" && (() => { const h = D.H.find((x) => x.id === e.id); return h && h.to === s.id && ["ask", "relay", "spawn"].includes(h.kind) && h.brief.toLowerCase().includes(term); })()) || (e.k === "u" && e.text.toLowerCase().includes(term)))).length;
      await page.fill("#sq", term); await page.waitForTimeout(100);
      SP.search = { term, rows: await page.evaluate(() => document.querySelectorAll(".page .nrow").length), expectedAtLeast: expect };
      await page.fill("#sq", "zzqqxx"); await page.waitForTimeout(80); SP.search.none = await page.evaluate(() => document.querySelector(".page .empty")?.textContent);
      await page.fill("#sq", ""); await page.waitForTimeout(80);
      await page.click(".page .nrow"); await page.waitForTimeout(150); SP.rowOpens = await page.evaluate(() => { const s = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main"); return { v: history.state?.v, navCurrent: document.querySelector(".nav-item[aria-current]")?.dataset.go, gap: s.scrollHeight - s.scrollTop - s.clientHeight }; });
      await goto(page, { v: "sessions" }, D);
      if (phone) {
        await page.click("#lead-btn"); await page.waitForTimeout(300);
        SP.sidebar = await page.evaluate(({ parent, child }) => { const roots = [...document.querySelectorAll("#lanes > .treeitem")], item = roots.find((x) => x.dataset.id === parent); return { head: document.querySelector(".side-h")?.textContent, rows: document.querySelectorAll("#lanes .srow").length, roots: roots.length, children: document.querySelectorAll("#lanes .tree-group .treeitem").length, childUnderParent: [...(item?.querySelectorAll(":scope > .tree-group .treeitem") ?? [])].some((x) => x.dataset.id === child), all: document.querySelector("#all-sessions")?.textContent, chips: document.querySelectorAll(".sidebar .groupby").length }; }, treePair ? { parent: treePair.from, child: treePair.to } : {});
        await page.fill("#q", term); await page.waitForTimeout(80); SP.sidebar.typedRows = await page.evaluate(() => document.querySelectorAll("#lanes .srow").length);
        await page.press("#q", "Enter"); await page.waitForTimeout(300); SP.sidebar.enter = await page.evaluate(() => ({ v: history.state?.v, pageQuery: document.querySelector("#sq")?.value, rows: document.querySelectorAll(".page .nrow").length, drawerClosed: !document.body.classList.contains("drawer-open") }));
        await page.fill("#sq", ""); await page.waitForTimeout(80);
      } else SP.sidebar = await page.evaluate(({ parent, child }) => { const roots = [...document.querySelectorAll("#lanes > .treeitem")], item = roots.find((x) => x.dataset.id === parent); return { rows: document.querySelectorAll("#lanes .srow").length, roots: roots.length, children: document.querySelectorAll("#lanes .tree-group .treeitem").length, childUnderParent: [...(item?.querySelectorAll(":scope > .tree-group .treeitem") ?? [])].some((x) => x.dataset.id === child), all: document.querySelector("#all-sessions")?.textContent }; }, treePair ? { parent: treePair.from, child: treePair.to } : {});
      out.sessionsPage = SP;
    }
    out.fixes = { ...F, tsumFallback: F.tsumFallback.slice(0, 8), expected: X };
    if (mode !== "desktop") { const h4 = D.H.find((h) => h.kind === "ask" && h.to === "quill")?.id; r.expect(!!h4, mode + ": quill's inbound ask handoff was not found (needed for the trace screenshot)"); if (h4) { await goto(page, { v: "session", id: "quill" }, D); await page.click('.turn[data-turn="' + h4 + '"] .tracebtn'); await page.waitForTimeout(200); await page.screenshot({ path: path.join(ENV.out, "bar-sample-trace" + (dark ? "-dark" : "") + ".png") }); } }
    out.errors = page.errors;
    modes.push(out);
    await page.context().close();
  }

  // Child-session path, sibling navigation, return rows and nested Runs views.
  const childAssertions = [];
  const parentKids = Object.values(D.SESS).filter((s) => parentOf(s.id) === "harbor").sort((a, b) => D.H.find((h) => h.kind === "spawn" && h.to === a.id).at - D.H.find((h) => h.kind === "spawn" && h.to === b.id).at);
  const grandchild = Object.values(D.SESS).find((s) => parentOf(s.id) && parentOf(parentOf(s.id)) === "harbor");
  const failedChild = parentKids.find((s) => s.state === "err" || D.H.some((h) => h.kind === "spawn" && h.to === s.id && h.status === "err"));
  r.expect(!!grandchild, "fixture has no grandchild session for the lineage and nested Runs checks");
  r.expect(parentKids.length >= 3 && parentKids.length % 2 === 1, "fixture needs a middle sibling with previous and next child sessions: " + parentKids.map((s) => s.id).join(", "));
  r.expect(!!failedChild, "fixture has no failed child session for the return-row check");
  if (grandchild && parentKids.length >= 3 && failedChild) for (const size of ["phone", "desktop"]) {
    const page = await served(browser, { size }); await goto(page, { v: "session", id: grandchild.id }, D);
    const pathNames = [];
    if (size === "phone") {
      await page.click(".topbar .lineage-parent"); await page.waitForSelector(".lineage-menu");
      pathNames.push(...await page.locator(".lineage-menu [role=menuitem] span:first-child").allTextContents());
    } else pathNames.push(...await page.locator("#topbar .l1 .crumb").allTextContents(), await page.locator("#topbar .t").textContent());
    const expectedPath = []; let cursor = grandchild.id;
    while (cursor && D.SESS[cursor]) { expectedPath.unshift(D.SESS[cursor].name); cursor = parentOf(cursor); }
    const pathOk = expectedPath.every((name) => pathNames.some((got) => got.trim() === name));

    await goto(page, { v: "session", id: grandchild.id }, D);
    const origin = D.H.find((h) => h.kind === "spawn" && h.to === grandchild.id);
    const briefCard = await page.evaluate((text) => { const intro = document.querySelector(".child-intro"), brief = intro?.querySelector(".brief"); return { visible: !!intro && !!brief, includesBrief: !!brief && brief.textContent.includes(text.slice(0, 48)), openInParent: intro?.querySelector(".intro-open")?.textContent }; }, origin?.brief ?? "");
    await page.click(".child-intro .intro-open"); await afterTitle(page, D.SESS[parentOf(grandchild.id)].name); await page.waitForTimeout(260);
    const openedParent = await page.evaluate((id) => ({ id: history.state?.id, handoff: !!document.querySelector('.hcard[data-h="' + id + '"].flash') }), origin?.id);

    await goto(page, { v: "session", id: failedChild.id }, D);
    const returnRow = await page.evaluate(() => { const row = document.querySelector(".child-return"); return { text: row?.textContent, openParent: row?.querySelector("button")?.textContent }; });
    await page.click(".child-return button"); await afterTitle(page, D.SESS[parentOf(failedChild.id)].name); await page.waitForTimeout(260);
    const failedOrigin = D.H.find((h) => h.kind === "spawn" && h.to === failedChild.id);
    const returnParent = await page.evaluate((id) => ({ id: history.state?.id, handoff: !!document.querySelector('.hcard[data-h="' + id + '"].flash') }), failedOrigin?.id);

    const middleIndex = Math.floor(parentKids.length / 2), middle = parentKids[middleIndex];
    await goto(page, { v: "session", id: middle.id }, D);
    const siblingSelector = size === "phone" ? ".child-intro .sibling-nav" : "#topbar .sibling-nav";
    const siblingNav = await page.evaluate((selector) => { const nav = document.querySelector(selector); return { present: !!nav, prev: nav?.querySelector('[aria-label^="Previous sibling"]')?.disabled === false, next: nav?.querySelector('[aria-label^="Next sibling"]')?.disabled === false, count: nav?.querySelector(".sibling-count")?.textContent }; }, siblingSelector);
    await page.locator(siblingSelector + ' [aria-label^="Previous sibling"]').click(); await afterTitle(page, parentKids[middleIndex - 1].name);
    const prevId = await page.evaluate(() => history.state?.id);
    await goto(page, { v: "session", id: middle.id }, D); await page.locator(siblingSelector + ' [aria-label^="Next sibling"]').click(); await afterTitle(page, parentKids[middleIndex + 1].name);
    const nextId = await page.evaluate(() => history.state?.id);

    await goto(page, { v: "session", id: "harbor" }, D);
    const runsButton = page.locator("#topbar .meta-runs"), runsVisible = await runsButton.isVisible(); let runsViaMenu = false;
    if (runsVisible) await runsButton.click();
    else if (size === "phone") { await page.click("#more-btn"); runsViaMenu = await page.locator(".session-menu .menu-runs").isVisible(); if (runsViaMenu) await page.locator(".session-menu .menu-runs").click(); }
    const runsSelector = size === "phone" ? "dialog.runs-sheet" : ".runs-popover";
    if (runsVisible || runsViaMenu) await page.waitForSelector(runsSelector);
    const runs = runsVisible || runsViaMenu ? await page.evaluate((selector) => { const box = document.querySelector(selector); return { open: !!box, rows: box?.querySelectorAll(".runs-row").length ?? 0, nested: box?.querySelectorAll(".runs-group .runs-row").length ?? 0, costs: box?.querySelectorAll(".run-cost").length ?? 0, apiLabel: box?.textContent.includes("API-equivalent cost") ?? false }; }, runsSelector) : { open: false, rows: 0, nested: 0, costs: 0, apiLabel: false };
    const expectedRuns = Object.values(D.SESS).filter((s) => { let p = parentOf(s.id); while (p && p !== "harbor") p = parentOf(p); return p === "harbor"; }).length;
    childAssertions.push({ size, pathNames, expectedPath, pathOk, briefCard, openedParent, returnRow, returnParent, siblingNav, prevId, nextId, expectedPrev: parentKids[middleIndex - 1].id, expectedNext: parentKids[middleIndex + 1].id, runs, runsViaMenu, expectedRuns });
    await page.context().close();
  }

  for (const child of childAssertions) {
    r.expect(child.pathOk === true, child.size + ": lineage breadcrumb did not show the full parent path: " + JSON.stringify({ expected: child.expectedPath, got: child.pathNames }));
    r.expect(child.briefCard.visible && child.briefCard.includesBrief && child.briefCard.openInParent?.includes("Open in") && child.openedParent.id === parentOf(grandchild.id) && child.openedParent.handoff, child.size + ": child brief or Open in parent handoff link failed: " + JSON.stringify({ brief: child.briefCard, opened: child.openedParent }));
    r.expect(child.returnRow.text?.toLowerCase().includes("failed") && child.returnRow.openParent?.includes("Open in") && child.returnParent.id === parentOf(failedChild.id) && child.returnParent.handoff, child.size + ": failed child return row did not reopen its parent handoff: " + JSON.stringify({ row: child.returnRow, parent: child.returnParent }));
    r.expect(child.siblingNav.present && child.siblingNav.prev && child.siblingNav.next && child.prevId === child.expectedPrev && child.nextId === child.expectedNext, child.size + ": previous/next sibling controls did not open adjacent runs: " + JSON.stringify(child));
    r.expect(child.runs.open && child.runs.rows === child.expectedRuns && child.runs.nested > 0 && child.runs.costs === child.expectedRuns && child.runs.apiLabel, child.size + ": Runs view did not list the full nested tree and its API-equivalent costs: " + JSON.stringify(child.runs));
  }
  r.expect(childAssertions.length === 2, "lineage, sibling, brief/return and Runs checks did not run on phone and desktop: " + childAssertions.length);

  // Masked thinking (Claude redacts it, leaving only a duration) draws nothing. Injected through the served /api/tx path:
  // a readable thought, two timed masked thoughts, another readable thought and two untimed masked thoughts. Only the two
  // readable thoughts get a row, each with its disclosure control.
  const thoughtSid = Object.keys(D.SESS).find((sid) => (D.TX[sid] ?? []).length);
  let thoughts = null;
  if (thoughtSid) {
    const plain = await served(browser, { size: "phone" });
    await goto(plain, { v: "session", id: thoughtSid }, D);
    const baseThoughts = await plain.evaluate(() => document.querySelectorAll(".turns .think").length);
    await plain.context().close();
    const page = await served(browser, { size: "phone" });
    await page.route("**/api/tx*", async (route) => {
      const u = new URL(route.request().url()); if (u.searchParams.get("sid") !== thoughtSid) return route.continue();
      const response = await route.fetch(), body = await response.json();
      body.entries.push({ k: "think", text: "Timing separator" }, { k: "think", text: "", secs: 2 }, { k: "think", text: "", secs: 3 }, { k: "think", text: "A readable thought" }, { k: "think", text: "" }, { k: "think", text: "" });
      await route.fulfill({ response, json: body });
    });
    await goto(page, { v: "session", id: thoughtSid }, D);
    await page.waitForFunction((base) => document.querySelectorAll(".turns .think").length >= base + 2, baseThoughts);
    thoughts = await page.evaluate((base) => { const rows = [...document.querySelectorAll(".turns .think")]; return { addedThoughts: rows.length - base, readable: rows.length > 0 && rows.every((x) => x.tagName === "BUTTON" && x.querySelector(".chev") && x.nextElementSibling?.hidden), emptyBlocks: document.querySelectorAll(".turn > .tx:empty").length, errors: [] }; }, baseThoughts);
    thoughts.errors = page.errors;
    await page.context().close();
  }
  // The fixture's principal session opens a turn with three masked thoughts before its first tool call: no thought rows,
  // the tool call still there, and no turn block left empty.
  let maskedTurn = null;
  if (D.SESS.principal) {
    const page = await served(browser, { size: "phone" });
    await goto(page, { v: "session", id: "principal" }, D);
    await page.waitForSelector(".turns .step", { state: "attached" });
    maskedTurn = await page.evaluate(() => ({ thoughtRows: document.querySelectorAll(".turns .think, .turns .think-pending").length, steps: document.querySelectorAll(".turns .step").length, emptyBlocks: document.querySelectorAll(".turn > .tx:empty").length, errors: [] }));
    maskedTurn.errors = page.errors;
    await page.context().close();
  }

  // Two turns are added to the served model and transcript of the principal session: one holding only masked thoughts, with
  // no start, no message and no end (nothing to draw: its block must go, leaving no empty .tx), and one with a masked thought
  // and then a reply (its block stays, with the reply and no thought row). The second also proves the injection reached the
  // page: without it, "the first turn is gone" could mean the turn never existed.
  let bareTurns = null;
  if (D.SESS.principal) {
    const page = await served(browser, { size: "phone" });
    await page.route("**/api/model*", async (route) => {
      const response = await route.fetch(); if (response.status() !== 200) return route.fulfill({ response }); // a 304 has no body
      const body = await response.json();
      const base = body.turns.filter((t) => t.sid === "principal").at(-1);
      for (const id of ["bt-bare-turn", "bt-masked-then-reply"]) body.turns.push({ id, sid: "principal", at: base?.at ?? 0, start: null, u: false, text: "", sent: [], end: null });
      await route.fulfill({ response, json: body });
    });
    await page.route("**/api/tx*", async (route) => {
      const u = new URL(route.request().url()); if (u.searchParams.get("sid") !== "principal") return route.continue();
      const response = await route.fetch(); if (response.status() !== 200) return route.fulfill({ response });
      const body = await response.json();
      body.entries.push({ k: "think", text: "", secs: 4, turn: "bt-bare-turn" }, { k: "think", text: "", secs: 5, turn: "bt-masked-then-reply" }, { k: "a", text: "Reply after a masked thought" });
      await route.fulfill({ response, json: body });
    });
    await goto(page, { v: "session", id: "principal" }, D);
    await page.waitForSelector('section.turn[data-turn="bt-masked-then-reply"]', { timeout: 8000 }).catch(() => {});
    bareTurns = await page.evaluate(() => {
      const withReply = document.querySelector('section.turn[data-turn="bt-masked-then-reply"]');
      return { bareBlock: !!document.querySelector('section.turn[data-turn="bt-bare-turn"]'), replyBlock: !!withReply, replyText: withReply?.querySelector(".msg")?.textContent ?? null, thoughtRowsInReply: withReply?.querySelectorAll(".think, .think-pending").length ?? null, emptyBlocks: document.querySelectorAll(".turn > .tx:empty").length, errors: [] };
    });
    bareTurns.errors = page.errors;
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

  r.results = { modes, expected: X, thoughts, maskedTurn, bareTurns, extra, childAssertions };
  r.expect(!!thoughtSid, "no session transcript was available for the masked-thinking check");
  if (thoughts) {
    r.expect(thoughts.errors.length === 0, "masked-thinking route: page errors: " + thoughts.errors.join(" | "));
    r.expect(thoughts.addedThoughts === 2, "the two readable injected thoughts should add exactly two thought rows, the four masked ones none: " + JSON.stringify(thoughts));
    r.expect(thoughts.readable === true, "readable thinking did not keep its disclosure control");
    r.expect(thoughts.emptyBlocks === 0, "a turn block was left empty by masked thoughts: " + JSON.stringify(thoughts));
  }
  if (bareTurns) {
    r.expect(bareTurns.errors.length === 0, "bare masked turn: page errors: " + bareTurns.errors.join(" | "));
    r.expect(bareTurns.replyBlock && bareTurns.replyText?.includes("Reply after a masked thought") && bareTurns.thoughtRowsInReply === 0, "the injected masked-then-reply turn did not draw as its reply alone: " + JSON.stringify(bareTurns));
    r.expect(!bareTurns.bareBlock && bareTurns.emptyBlocks === 0, "a turn holding only masked thoughts was left as a block: " + JSON.stringify(bareTurns));
  }
  r.expect(!D.SESS.principal || !!bareTurns, "the bare masked turn check did not run");
  r.expect(!D.SESS.principal || !!maskedTurn, "the principal session's masked-thinking turn was not checked");
  if (maskedTurn) {
    r.expect(maskedTurn.errors.length === 0, "principal masked-thinking route: page errors: " + maskedTurn.errors.join(" | "));
    r.expect(maskedTurn.thoughtRows === 0, "a turn of masked thoughts still shows thought rows: " + JSON.stringify(maskedTurn));
    r.expect(maskedTurn.steps > 0 && maskedTurn.emptyBlocks === 0, "hiding masked thoughts lost the turn's steps or left an empty block: " + JSON.stringify(maskedTurn));
  }
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
    r.expect(m.line2TargetFailures.length === 0, m.mode + ": approved line-2 target sizes differ: " + JSON.stringify(m.line2TargetFailures));
    r.expect(m.l2NotOneLine.length === 0, m.mode + ": l2 not one line: " + JSON.stringify(m.l2NotOneLine));
    r.expect(m.l2NoEllipsis === 0, m.mode + ": l2 missing ellipsis count=" + m.l2NoEllipsis);
    r.expect(m.l2Overflowing === 0, m.mode + ": l2 content overflow count=" + m.l2Overflowing);
    r.expect(m.sessionMetaPages > 0 && m.metaOrderFailures.length === 0, m.mode + ": session line 2 order=" + JSON.stringify(m.metaOrderFailures));
    r.expect(m.metaDotFailures.length === 0, m.mode + ": session line 2 facts (whole state dot on the title edge, no run dot, no whole-line button, quiet errors, only errors and runs are buttons): " + JSON.stringify(m.metaDotFailures.slice(0, 6)));
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
        if (m.mode.startsWith("phone")) r.expect(m.search.phoneMenu?.includes("Find in transcript") && m.search.phoneMenu?.includes("Filter transcript"), m.mode + ": Find and Filter were missing from the phone ⋯ menu: " + JSON.stringify(m.search.phoneMenu));
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
      r.expect(!!m.inertBadges && m.inertBadges.items >= 4 && m.inertBadges.clicked >= 1 && m.inertBadges.overlaid === 0 && m.inertBadges.titled && !m.inertBadges.dialog && !m.inertBadges.menu && !m.inertBadges.moved, m.mode + ": a plain line-2 badge lost its tooltip or acted on click: " + JSON.stringify(m.inertBadges));
      r.expect(!!m.details, m.mode + ": the Session details test never ran");
      if (m.details) {
        r.expect(m.details.open === true && m.details.inView === true, m.mode + ": Session details did not open visibly from the ⋯ menu: " + JSON.stringify(m.details));
        r.expect(m.details.labels.includes("Model") && m.details.labels.includes("Machine") && (m.details.labels.includes("Branch") || m.details.labels.includes("Worktree")) && m.details.labels.includes("Started") && m.details.labels.includes("Duration") && m.details.labels.includes("Tokens in / out") && m.details.labels.includes("Cached context") && m.details.hasSessionId, m.mode + ": Session details omitted a menu fact: " + JSON.stringify(m.details.labels));
        r.expect(m.details.escFocus?.closed === true && m.details.escFocus?.focus === "more-btn", m.mode + ": Escape on Session details did not return focus to the ⋯ button: " + JSON.stringify(m.details.escFocus));
        r.expect(m.details.hasCost === true && m.details.costExpanded === true && m.details.costBreakdown === true && m.details.closed === true, m.mode + ": Session details omitted or failed to expand the API-equivalent cost breakdown, or did not close: " + JSON.stringify(m.details));
        const detailFacts = D.SESS[m.details.session], reports = detailFacts?.reported_runs ?? [], costChecks = detailFacts?.cost_check ?? [];
        r.expect(reports.length > 0, m.mode + ": the fixture did not expose a reported Claude Code run for Session details");
        if (reports.length) r.expect(m.details.reported?.startsWith("Claude Code reported $") === true && m.details.reported.includes(reports.length === 1 ? "for its last run" : "for its last " + reports.length + " runs"), m.mode + ": reported Claude Code run cost was not shown in details: " + JSON.stringify(m.details.reported));
        if (costChecks.some((check) => check.ok === false)) r.expect(/^Differs from Claude Code's figure by \d+%$/.test(m.details.costWarning ?? ""), m.mode + ": the unaccepted Claude Code cost comparison was not shown: " + JSON.stringify(m.details.costWarning));
        if (m.mode.startsWith("phone")) r.expect(m.details.phoneSheet === true, m.mode + ": details did not use the phone bottom sheet");
        else r.expect(m.details.desktopDialog === true, m.mode + ": details did not use the desktop dialog");
        const busyFacts = D.SESS[m.details.session];
        if (busyFacts?.cwd != null && busyFacts.cwd !== "") r.expect(m.details.hasDirectory, m.mode + ": details omitted the directory present in the served model");
        if (busyFacts?.pid != null && busyFacts.pid !== "") r.expect(m.details.hasPid, m.mode + ": details omitted the pid present in the served model");
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
      if (m.sessionsPage?.rowOpens) r.expect(m.sessionsPage.rowOpens.v === "session" && m.sessionsPage.rowOpens.gap <= 1, m.mode + ": opening a Sessions row did not land at the end: " + JSON.stringify(m.sessionsPage.rowOpens));
      if (m.sessionsPage?.sidebar) {
        r.expect(m.sessionsPage.sidebar.roots === m.sessionsPage.expectedRoots, m.mode + ": sidebar top-level rows=" + m.sessionsPage.sidebar.roots + " expected=" + m.sessionsPage.expectedRoots);
        r.expect(m.sessionsPage.sidebar.rows === m.sessionsPage.expectedTreeRows && m.sessionsPage.sidebar.children === m.sessionsPage.expectedTreeRows - m.sessionsPage.expectedRoots, m.mode + ": sidebar tree rows did not contain the eight most recent roots and their children: " + JSON.stringify({ sidebar: m.sessionsPage.sidebar, expectedTreeRows: m.sessionsPage.expectedTreeRows, expectedRoots: m.sessionsPage.expectedRoots }));
        if (treePair) r.expect(m.sessionsPage.sidebar.childUnderParent === true, m.mode + ": child session wasn't nested beneath its parent: " + JSON.stringify(treePair));
      }
    }
  }

  return r.done();
}

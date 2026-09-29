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
//  - the bar stays pinned at the top after scrolling (notPinned is empty), is never sideways (sideways === 0), and every
//    control is at least the tap size (36px, 44px on a phone).
//  - a session's second line is one line of labels: the state first with its dot, then kind, model, failed steps, runs, machine,
//    branch and cost. Two of them are buttons that look the same: "N failed" (enters errors mode) and "N runs" (opens the menu
//    at its runs); they are the last two the fitter drops (failed steps last of all), the others drop from the end. The bar's
//    other actions are Find and the ⋯ menu. Other detail pages keep their summary in the same line.
//  - zero overflow screens.
//  - gap markers: this fixture's data has none at all (neither the sample nor the extras fixture produces a gap
//    marker), so rendered counts are asserted directly against the literal 0, not against a same-data expectation
//    that would also be 0 by construction (see the report for why this can't be a positive check here).
//  - tables: the sample fixture also has zero table messages, so "tables render" is checked as a positive count on
//    the extras fixture instead (harbor's markdown message there has a genuine table): extras.tables > 0. With no
//    unknown tool name in this fixture (expected.fallbackTools is empty), no turn summary falls back to a
//    lowercased tool name (tsumLowercasedUnknown === 0).
//  - find and filter are one mode: Find takes over the bar, the match count agrees with the hits, and chips (All, Messages,
//    Steps) choose one at a time: Messages hides every step, Steps every message, and All restores the transcript. The
//    Failed steps chip opens errors mode: "Error 1 of N" (N the chip's count), the first failed step marked, in view and not
//    expanded, with no menu open; Escape leaves it and returns to Find — and a session with a failed step must actually be found, or this fails
//    instead of silently not running (checks/errnav.mjs covers the mode itself).
//  - the ⋯ menu is one panel (a phone's bottom sheet, a desktop's anchored panel): actions (copy the resume command, open in
//    claude.ai, the wide switch on desktop), details, and cost (one figure, this session and its runs, the harness's own figure,
//    a quiet note when the estimate differs, the runs list five at a time, tokens by model folded). Escape returns focus to ⋯.
//  - deep links land the target turn below the bar; a relay header's sender link opens the sender's turn — both
//    must actually be found (a trace child node, a Home item, a relay header), not silently skipped.
//  - the sidebar shows the 8 most recent top-level tree rows with nested children; the Sessions page keeps all session rows,
//    every grouping produces sections, search narrows to model matches, and opening a row lands at the end.
import path from "node:path";
import { ENV, VIEWPORTS, settled, served, goto, data, reporter, overflow, wide } from "../lib.mjs";

const isGap = (e) => e.k === "end" && /entries (not included|omitted)|^No activity/.test(e.text ?? "");
const TABLE = /^\s*\|.*\n\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/m;
const KNOWN = new Set(["Bash", "shell", "exec_command", "local_shell", "write_stdin", "Grep", "Glob", "Read", "Edit", "MultiEdit", "Write", "apply_patch", "NotebookEdit", "AskUserQuestion", "ToolSearch", "SendMessage", "SendUserFile", "Agent", "Task", "Monitor", "ScheduleWakeup", "TaskStop", "Artifact", "WebFetch", "WebSearch", "Skill"]);
// The title is in the bar as soon as a session is clicked; the page is ready once it is no longer aria-busy.
const afterTitle = (page, text) => page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t && !document.querySelector("#page").hasAttribute("aria-busy"), text);

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
  // A parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows are listed;
  // the rest, with everything below them, are behind "All N", so those rows aren't in the sidebar tree. A finished child with a waiting
  // or running session below it ranks as that session does.
  const kidsOf = (sid) => Object.values(D.SESS).filter((s) => s.id !== sid && s.parent === sid);
  const below = (sid, seen = new Set([sid])) => kidsOf(sid).flatMap((k) => (seen.has(k.id) ? [] : (seen.add(k.id), [k, ...below(k.id, seen)])));
  const rankOf = (k) => below(k.id).reduce((rank, d) => (d.state === "wait" ? 0 : d.state === "work" ? Math.min(rank, 1) : rank), k.state === "wait" ? 0 : k.state === "work" ? 1 : 2);
  const folded = new Set();
  const foldAll = (sid) => { if (folded.has(sid)) return; folded.add(sid); kidsOf(sid).forEach((k) => foldAll(k.id)); };
  const foldBelow = (sid, seen = new Set()) => {
    if (seen.has(sid)) return; seen.add(sid);
    const kids = kidsOf(sid).map((k) => ({ k, rank: rankOf(k) })).sort((a, b) => a.rank - b.rank || b.k.last - a.k.last), keep = new Set();
    for (const { k, rank } of kids) if (rank < 2 && keep.size < 8) keep.add(k.id);
    for (const { k } of kids) if (keep.size < 3) keep.add(k.id);
    for (const { k } of kids) if (keep.has(k.id)) foldBelow(k.id, seen); else foldAll(k.id);
  };
  Object.values(D.SESS).filter((s) => !s.parent).forEach((s) => foldBelow(s.id));
  const expectedTreeRows = allSessions.filter((s) => shownRoots.has(treeRoot(s.id)) && !folded.has(s.id)).length;
  const treePair = D.H.find((h) => h.kind === "spawn" && roots.slice(0, 8).some((s) => s.id === h.from) && D.SESS[h.to]) ?? null;
  const X = { gapMarkers: 0, tableMessages: 0, fallbackTools: {} };
  for (const es of Object.values(D.TX)) for (const e of es) { if (isGap(e)) X.gapMarkers++; if ((e.k === "a" || e.k === "u") && TABLE.test(e.text)) X.tableMessages++; if (e.k === "tool" && !KNOWN.has(e.name) && !/^mcp__/.test(e.name)) X.fallbackTools[e.name] = (X.fallbackTools[e.name] ?? 0) + 1; }

  const modes = [];
  for (const mode of ["phone-light", "phone-dark", "desktop"]) {
    const phone = mode !== "desktop", dark = mode === "phone-dark";
    const page = await served(browser, { size: phone ? "phone" : "desktop", dark });
    const over = () => overflow(page), stateWord = { work: "Working", wait: "Needs you", idle: "Idle", done: "Done", err: "Failed", new: "New result", read: "Read result" };
    const barCheckOnce = () => page.evaluate(async () => {
      const main = document.querySelector("#main"), phone = matchMedia("(max-width: 760px)").matches;
      if (phone) window.scrollTo(0, document.documentElement.scrollHeight); else main.scrollTop = main.scrollHeight;
      await new Promise((r) => setTimeout(r, 120));
      const bar = document.querySelector("#topbar"), br = bar.getBoundingClientRect(), vw = document.documentElement.clientWidth;
      const scrolled = phone ? scrollY : main.scrollTop;
      // On a phone a session's line of labels is not drawn (its state is the dot before the title); it stays in the page, undisplayed.
      const l2 = bar.querySelector(".meta-line"), l2Shown = !!l2 && getComputedStyle(l2).display !== "none", lead = bar.querySelector(".l1-state");
      const sessionMeta = l2Shown && !!l2.querySelector(".lab.state") && !!bar.querySelector("#more-btn");
      const crumb = bar.querySelector(".l1 .crumb"), lb = lead?.getBoundingClientRect(), cb = crumb?.getBoundingClientRect();
      const phoneRow = bar.classList.contains("session-bar") ? { phone, chevW: cb ? Math.round(cb.width) : null, leadW: lb ? Math.round(lb.width) : null, leadH: lb ? Math.round(lb.height) : null, l2Shown, leadShown: !!lead && getComputedStyle(lead).display !== "none", dot: !!lead?.querySelector(".dot[role=img]"), tip: lead?.dataset.tip ?? null, h: Math.round(br.height * 10) / 10 } : null;
      // A segmented control's buttons are drawn 24px tall on purpose; their tap target is the ::before box, so that is what counts here.
      const segmentHit = (x) => { if (!x.closest(".analytics-range")) return null; const p = getComputedStyle(x, "::before"), h = x.getBoundingClientRect().height, top = parseFloat(p.top), bottom = parseFloat(p.bottom); return Number.isFinite(top) && Number.isFinite(bottom) ? h - top - bottom : h; };
      // Every control in the bar is at least the tap size (36 px, 44 on a phone). A crumb is link text with a padded hit area.
      const tap = phone ? 44 : 36;
      const ctl = [...bar.querySelectorAll("button, input, [role=link]")].filter((x) => x.offsetParent || getComputedStyle(x).position === "absolute").map((x) => [x.className || x.tagName, segmentHit(x) ?? x.getBoundingClientRect().height, x.classList.contains("crumb")]);
      const small = ctl.filter(([, h, crumb]) => h < tap - 0.5 && !(crumb && !phone));
      // Labels are information: the state is first and holds its dot, the rest keep their order, and only failed steps and runs are controls.
      let metaFacts = null;
      if (sessionMeta) {
        const labs = [...l2.querySelectorAll(":scope > .lab")].filter((x) => !x.hidden), first = labs[0];
        const drops = labs.slice(1).map((x) => Number(x.dataset.drop));
        metaFacts = { first: first?.classList.contains("state") ?? false, dot: !!first?.querySelector(".dot"), drops, inOrder: drops.filter((d) => d > 1).every((d, i, all) => i === 0 || all[i - 1] < d), keep: labs.some((x) => x.classList.contains("lab-errs")) ? Math.min(...labs.filter((x) => !x.classList.contains("lab-errs") && !x.classList.contains("state")).map((x) => Number(x.dataset.drop))) > 0 : true, buttons: l2.querySelectorAll("a, input, button:not(.lab-errs):not(.lab-runs)").length, actions: [...bar.querySelectorAll(":scope > .ibtn:not(.lead)")].map((b) => b.id), text: labs.map((x) => x.textContent) };
      }
      const side = [...bar.querySelectorAll("*")].filter((x) => { const r = x.getBoundingClientRect(); return r.width && (r.right > vw + 0.5 || r.left < -0.5) && !x.closest(".meta-line"); }).length + (bar.scrollWidth > bar.clientWidth + 1 ? 1 : 0);
      const out = { pinned: Math.abs(br.top) < 0.5 && br.height > 30 && br.bottom > 0 && getComputedStyle(bar).visibility !== "hidden", scrolled: scrolled > 0, barH: Math.round(br.height), small: small.map(([c, h]) => c + ":" + Math.round(h)), side, metaFacts,
        phoneRow, l2: l2Shown ? { h: Math.round(l2.getBoundingClientRect().height), oneLine: l2.scrollHeight <= l2.clientHeight + 1, sessionMeta, overflows: l2.scrollWidth > l2.clientWidth + 1 } : null };
      if (phone) window.scrollTo(0, 0); else main.scrollTop = 0; return out;
    });
    const R = { mode, pages: 0, notPinned: [], l2Pages: 0, l2NotOneLine: [], l2Overflowing: 0, metaFailures: [], sessionMetaPages: 0, sideways: 0, smallControls: [], overflowScreens: 0 };
    const measure = async (name) => {
      const c = await barCheckOnce(); R.pages++;
      if (!c.pinned) R.notPinned.push(name + (c.scrolled ? "" : "(no scroll)"));
      if (c.l2) { R.l2Pages++; if (!c.l2.oneLine) R.l2NotOneLine.push(name + ":" + c.l2.h); if (c.l2.overflows) R.l2Overflowing++; if (c.l2.sessionMeta) R.sessionMetaPages++; }
      const pr = c.phoneRow;
      if (pr) {
        if (pr.phone) { if (pr.l2Shown || !pr.leadShown || !pr.dot || !/^Status: /.test(pr.tip ?? "") || pr.h > 57.5 || pr.leadW < 32 || pr.leadH < 32 || (pr.chevW != null && (pr.chevW < 44 || pr.chevW > 48))) R.metaFailures.push(name + ": a phone's session bar is not one row (line hidden, state dot at least 32 px before the title with its tip, a 44-48 px back chevron, at most 57.5 px): " + JSON.stringify(pr)); }
        else if (pr.leadShown || !pr.l2Shown) R.metaFailures.push(name + ": a desktop's session bar shows the phone's state dot or hides its line: " + JSON.stringify(pr));
      }
      const mf = c.metaFacts;
      if (mf) {
        if (!mf.first || !mf.dot) R.metaFailures.push(name + ": the state is not the first label with its dot: " + JSON.stringify(mf.text));
        if (!mf.inOrder) R.metaFailures.push(name + ": labels are out of order: " + JSON.stringify(mf.drops));
        if (mf.buttons) R.metaFailures.push(name + ": " + mf.buttons + " control(s) inside the label line besides failed steps and runs");
        if (!mf.keep) R.metaFailures.push(name + ": another label outlasts the failed-steps label in the fitter");
        if (mf.actions.join() !== "find-btn,more-btn") R.metaFailures.push(name + ": a session bar's actions are " + JSON.stringify(mf.actions) + ", not Find and the menu");
      }
      R.sideways += c.side; if (c.small.length) R.smallControls.push(name + " " + c.small.join(","));
      if (await over()) R.overflowScreens++;
      return c;
    };
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
    // The "Started … on <machine>" line is one line on a phone even when the machine name is long: the name (set here to a long
    // one, since the fixture's are short) ellipsises, its full name stays in the tip, and nothing scrolls sideways.
    if (phone) {
      for (const sid of sids) {
        await goto(page, { v: "session", id: sid }, D);
        if (!(await page.locator(".turns > .divider.started").count())) continue;
        const real = await page.evaluate(() => document.querySelector(".turns > .divider.started .dv-machine").dataset.tip);
        const LONG = "marvin-HP-EliteBook-X-G2i-14-inch-Notebook-Next-Gen-AI-PC";
        await page.evaluate((name) => { document.querySelector(".turns > .divider.started .dv-machine").textContent = name; }, LONG);
        const f = await page.evaluate(() => { const d = document.querySelector(".turns > .divider.started"), lead = d.querySelector(".dv-lead"), m = d.querySelector(".dv-machine"), dr = d.getBoundingClientRect(), vw = document.documentElement.clientWidth; return { h: Math.round(dr.height * 10) / 10, leadH: Math.round(lead.getBoundingClientRect().height * 10) / 10, cut: m.scrollWidth > m.clientWidth, ellipsis: getComputedStyle(m).textOverflow === "ellipsis", inside: dr.left >= -0.5 && dr.right <= vw + 0.5, sideways: document.documentElement.scrollWidth > vw, text: d.textContent }; });
        R.startedLine = { sid, real, ...f, sidewaysOverflow: await overflow(page) };
        break;
      }
    }
    R.smallControls = R.smallControls.slice(0, 6);
    const out = { ...R };
    if (mode !== "phone-dark") {
      // Find and filter are one mode on the busiest session: the field, its match count and four chips.
      // The busiest session that has both messages and tool steps, so each chip has something to hide and something to keep.
      const busy = sids.filter((x) => (D.TX[x] ?? []).some((e) => e.k === "tool") && (D.TX[x] ?? []).some((e) => e.k === "a")).sort((a, b) => (D.TX[b]?.length ?? 0) - (D.TX[a]?.length ?? 0))[0];
      await goto(page, { v: "session", id: busy }, D);
      const word = await page.evaluate(() => { const t = document.querySelector(".msg.assistant")?.textContent ?? ""; return (t.match(/[A-Za-z]{6,}/) ?? ["the"])[0].toLowerCase(); });
      const turnsBefore = await page.evaluate(() => document.querySelectorAll(".turns > .turn").length);
      await page.click("#find-btn"); await page.waitForTimeout(100);
      const S = { session: D.SESS[busy].name, word, turnsBefore, searching: await page.evaluate(() => !!document.querySelector("#topbar .find-row") && document.activeElement?.id === "find"), chips: await page.evaluate(() => [...document.querySelectorAll("#topbar .find-chips .chip")].map((c) => c.dataset.filter + ":" + c.getAttribute("aria-pressed"))) };
      await page.keyboard.type(word, { delay: 10 }); await page.waitForTimeout(200);
      Object.assign(S, await page.evaluate(() => ({ count: document.querySelector(".fcount")?.textContent, hits: document.querySelectorAll(".turns .msg, .turns .step, .turns .hcard").length, turns: document.querySelectorAll(".turns > .turn").length, focus: document.activeElement?.id, barH: document.querySelector("#topbar").offsetHeight })));
      S.countMatchesHits = S.count === S.hits + (S.hits === 1 ? " match" : " matches");
      // One choice at a time: Messages hides every step, Steps every message, and All brings both back.
      // A step is a row of its group, and a group is collapsed until opened: both count.
      const counts = () => page.evaluate(() => ({ msgs: document.querySelectorAll(".turns .msg").length, steps: document.querySelectorAll(".turns .step, .turns .tsum").length, pressed: [...document.querySelectorAll("#topbar .find-chips .chip[aria-pressed=true]")].map((c) => c.dataset.filter) }));
      await page.fill("#find", ""); await page.waitForTimeout(150);
      const Fl = { all: await counts() };
      await page.click('.find-chips .chip[data-filter="messages"]'); await page.waitForTimeout(150); Fl.messages = await counts();
      await page.click('.find-chips .chip[data-filter="steps"]'); await page.waitForTimeout(150); Fl.steps = await counts();
      await page.click('.find-chips .chip[data-filter="all"]'); await page.waitForTimeout(150); Fl.restored = await counts();
      out.filter = Fl;
      await page.click('.topbar [aria-label="Close find"]'); await page.waitForTimeout(150);
      Object.assign(S, await page.evaluate(() => ({ restored: !document.querySelector("#topbar .find-row") && !!document.querySelector("#topbar .meta-line"), turnsAfter: document.querySelectorAll(".turns > .turn").length })));
      out.search = S;
      // The session menu: actions, then details, then cost, in one panel.
      const detailsSid = Object.values(D.SESS).find((s) => s.reported_runs?.length)?.id ?? busy;
      await goto(page, { v: "session", id: detailsSid }, D);
      await page.evaluate(() => { window.scrollTo(0, 400); document.querySelector("#main").scrollTop = 400; }); await page.waitForTimeout(100);
      await page.click("#more-btn"); await page.waitForFunction(() => document.querySelector("dialog.session-menu")?.open === true);
      out.details = await page.evaluate((phone) => {
        const d = document.querySelector("dialog.session-menu"), r = d.getBoundingClientRect(), text = (x) => x.textContent.replace(/\s+/g, " ").trim();
        const cost = d.querySelector(".cost");
        return {
          open: d.open, inView: r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth,
          phoneSheet: phone ? r.bottom >= innerHeight - 1 && r.width >= innerWidth - 1 : null, desktopPanel: phone ? null : r.width <= 420,
          title: text(d.querySelector(".panel-t")), sub: text(d.querySelector(".panel-sub")),
          actions: [...d.querySelectorAll(".menu-list .menu-item")].map(text), wideSwitch: !!d.querySelector('[role="menuitemcheckbox"]'),
          labels: [...d.querySelectorAll(".panel-sec:not(.cost) dl.kv dt")].map(text),
          costBig: text(cost.querySelector(".cost-big")), costCap: text(cost.querySelector(".cost-cap")), costRows: [...cost.querySelectorAll("dl.kv dt")].map(text), notes: [...cost.querySelectorAll(".cost-note")].map(text),
          runRows: cost.querySelectorAll(".run-row").length, runRowsShown: [...cost.querySelectorAll(".run-row")].filter((x) => !x.hidden).length, runsMore: cost.querySelector(".runs .link")?.textContent ?? null,
          tokensHidden: cost.querySelector(".tokens")?.hidden ?? null,
        };
      }, phone);
      out.details.session = detailsSid;
      await page.click("dialog.session-menu .runs .link").catch(() => {}); await page.waitForTimeout(80);
      out.details.runRowsAfterMore = await page.evaluate(() => [...document.querySelectorAll("dialog.session-menu .run-row")].filter((x) => !x.hidden).length);
      await page.click("dialog.session-menu .disclose"); await page.waitForTimeout(80);
      out.details.tokens = await page.evaluate(() => { const t = document.querySelector("dialog.session-menu .tokens"); return { shown: t && !t.hidden, models: [...t.querySelectorAll(".tok-model")].map((x) => x.textContent), kinds: [...new Set([...t.querySelectorAll(".tok-line span:first-child")].map((x) => x.textContent))], expanded: document.querySelector("dialog.session-menu .disclose").getAttribute("aria-expanded") }; });
      // Escape closes it and puts focus back on the ⋯ button, not on the document body.
      await page.keyboard.press("Escape"); await page.waitForTimeout(150);
      out.details.escFocus = await page.evaluate(() => ({ closed: !document.querySelector("dialog.session-menu"), focus: document.activeElement?.id ?? document.activeElement?.tagName }));
      // The Failed steps chip opens errors mode on the first failed step: marked, in view, not expanded; no menu.
      const errSid = sids.find((s) => (D.TX[s] ?? []).some((e) => e.k === "tool" && e.ok === false));
      r.expect(!!errSid, "no session with a failed tool call to test the Failed steps chip on");
      if (errSid) {
        await goto(page, { v: "session", id: errSid }, D);
        // On a phone the line is not drawn: the ⋯ menu's errors item carries the count.
        let menuLabel = null; if (phone) { await page.click("#more-btn"); await page.waitForSelector("dialog.session-menu[open]"); menuLabel = await page.evaluate(() => document.querySelector("dialog.session-menu .menu-errors > span:not(.dot):not(.menu-note)")?.textContent ?? null); await page.keyboard.press("Escape"); await page.waitForTimeout(150); }
        await page.click("#find-btn");
        const badge = await page.evaluate(() => (document.querySelector('.find-chips .chip[data-filter="failures"] .n')?.textContent ?? "") + " errors");
        await page.click('.find-chips .chip[data-filter="failures"]'); await page.waitForTimeout(700);
        out.errsJump = { session: D.SESS[errSid].name, badge, menuLabel, ...(await page.evaluate(() => { const e = document.querySelector("#page .step.err-current"), r = e?.getBoundingClientRect(), bar = document.querySelector("#topbar").getBoundingClientRect(); return { label: document.querySelector("#topbar .errnav-count")?.textContent ?? null, marked: !!e?.classList.contains("err"), expanded: e?.querySelector("button")?.getAttribute("aria-expanded") ?? null, inView: !!r && r.top >= bar.bottom - 1 && r.top < innerHeight, menu: !!document.querySelector("dialog[open]") }; })) };
        await page.keyboard.press("Escape"); await page.waitForTimeout(300); out.errsJump.closed = await page.evaluate(() => !!document.querySelector("#topbar .find-row") && !document.querySelector("#topbar .errnav-count"));
      }
      // The "N failed" label enters errors mode too, and Escape puts focus back on it; a search typed before the mode opens is cleared;
      // a filter chosen on one session is gone on the next; "N runs" opens the menu at its runs; wide mode is for session pages only.
      out.labs = {};
      if (errSid) {
        await goto(page, { v: "session", id: errSid }, D);
        // On a phone the line of labels is not in the bar: errors mode is entered from the ⋯ menu's item, and focus returns to ⋯.
        const L = out.labs.errs = { count: phone ? await (async () => { await page.click("#more-btn"); await page.waitForSelector("dialog.session-menu[open]"); const n = await page.locator("dialog.session-menu .menu-errors").count(); if (!n) await page.keyboard.press("Escape"); return n; })() : await page.locator("#topbar .lab-errs").count() };
        if (L.count) {
          if (phone) { L.h = Math.round((await page.locator("dialog.session-menu .menu-errors").boundingBox()).height); await page.click("dialog.session-menu .menu-errors"); }
          else { L.h = Math.round((await page.locator("#topbar .lab-errs").boundingBox()).height); await page.click("#topbar .lab-errs"); }
          await page.waitForTimeout(700);
          L.label = await page.evaluate(() => document.querySelector("#topbar .errnav-count")?.textContent ?? null);
          await page.keyboard.press("Escape"); await page.waitForTimeout(300);
          L.focusBack = await page.evaluate((phone) => phone ? document.activeElement?.id === "more-btn" : document.activeElement?.classList.contains("lab-errs") === true, phone);
          await page.click("#find-btn"); await page.keyboard.type("zzzz", { delay: 10 }); await page.waitForTimeout(150);
          await page.click('.find-chips .chip[data-filter="failures"]'); await page.waitForTimeout(600);
          await page.keyboard.press("Escape"); await page.waitForTimeout(300);
          L.findAfter = await page.evaluate(() => document.querySelector("#find")?.value ?? null);
          await page.click('.find-chips .chip[data-filter="messages"]'); await page.waitForTimeout(100);
          await goto(page, { v: "home" }, D); await goto(page, { v: "session", id: errSid }, D); await page.click("#find-btn");
          L.chipsAfterNav = await page.evaluate(() => [...document.querySelectorAll(".find-chips .chip")].filter((c) => c.getAttribute("aria-pressed") === "true").map((c) => c.dataset.filter).join());
        }
      }
      {
        const parent = Object.values(D.SESS).find((x) => x.name && Object.values(D.SESS).some((c) => c.parent === x.id))?.id ?? "harbor";
        await goto(page, { v: "session", id: parent }, D);
        const L = out.labs.runs = { count: phone ? await (async () => { await page.click("#more-btn"); await page.waitForSelector("dialog.session-menu[open]"); const n = await page.locator("dialog.session-menu .menu-runs").count(); if (!n) await page.keyboard.press("Escape"); return n; })() : await page.locator("#topbar .lab-runs").count() };
        if (L.count) {
          if (phone) { L.h = Math.round((await page.locator("dialog.session-menu .menu-runs").boundingBox()).height); await page.click("dialog.session-menu .menu-runs"); }
          else { L.h = Math.round((await page.locator("#topbar .lab-runs").boundingBox()).height); await page.click("#topbar .lab-runs"); }
          await page.waitForFunction(() => document.querySelector("dialog.session-menu")?.open === true); await page.waitForTimeout(200);
          L.runsShown = await page.evaluate(() => { const runs = document.querySelector("dialog.session-menu .runs"), body = document.querySelector("dialog.session-menu .panel-b"); if (!runs || !body) return false; const a = runs.getBoundingClientRect(), b = body.getBoundingClientRect(); return a.top >= b.top - 1 && a.top < b.bottom; });
          await page.keyboard.press("Escape"); await page.waitForTimeout(150);
        }
      }
      if (mode === "desktop") {
        await goto(page, { v: "session", id: "harbor" }, D); await wide(page, true);
        const isWide = () => page.evaluate(() => document.querySelector("#page").classList.contains("wide-mode"));
        const W = out.labs.wide = { onSession: await isWide() };
        await goto(page, { v: "home" }, D); W.onHome = await isWide();
        await goto(page, { v: "session", id: "harbor" }, D); W.backOnSession = await isWide(); await wide(page, false);
      }
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

  // Child-session path, no sibling navigation, return rows and nested Runs views.
  const childAssertions = [];
  const parentKids = Object.values(D.SESS).filter((s) => parentOf(s.id) === "harbor").sort((a, b) => D.H.find((h) => h.kind === "spawn" && h.to === a.id).at - D.H.find((h) => h.kind === "spawn" && h.to === b.id).at);
  const grandchild = Object.values(D.SESS).find((s) => parentOf(s.id) && parentOf(parentOf(s.id)) === "harbor");
  const failedChild = parentKids.find((s) => s.state === "err" || D.H.some((h) => h.kind === "spawn" && h.to === s.id && h.status === "err"));
  r.expect(!!grandchild, "fixture has no grandchild session for the lineage and nested Runs checks");
  r.expect(parentKids.length >= 3, "fixture needs a middle child session that has siblings on both sides: " + parentKids.map((s) => s.id).join(", "));
  r.expect(!!failedChild, "fixture has no failed child session for the return-row check");
  if (grandchild && parentKids.length >= 3 && failedChild) for (const size of ["phone", "desktop"]) {
    const page = await served(browser, { size }); await goto(page, { v: "session", id: grandchild.id }, D);
    // The bar names one step up: the parent as a crumb, then this session. The crumb opens the parent.
    const pathNames = [(await page.locator("#topbar .crumb").allTextContents()).join("|"), await page.locator("#topbar .t").textContent()].map((x) => x.trim());
    const expectedPath = [D.SESS[parentOf(grandchild.id)].name, grandchild.name];
    const pathOk = pathNames[0] === expectedPath[0] && pathNames[1] === expectedPath[1];
    await page.click("#topbar .crumb"); await afterTitle(page, expectedPath[0]);
    const crumbOpens = await page.evaluate(() => history.state?.id);

    await goto(page, { v: "session", id: grandchild.id }, D);
    const origin = D.H.find((h) => h.kind === "spawn" && h.to === grandchild.id);
    const briefCard = await page.evaluate((text) => { const intro = document.querySelector(".child-intro"), brief = intro?.querySelector(".brief"); return { visible: !!intro && !!brief, includesBrief: !!brief && brief.textContent.includes(text.slice(0, 48)), openInParent: intro?.querySelector(".intro-open")?.textContent, bars: [...document.querySelectorAll(".child-intro, .child-return")].map((x) => getComputedStyle(x).borderLeftWidth), transcript: (() => { const sec = document.querySelector('#page section[aria-label="Transcript"]'), cs = sec && getComputedStyle(sec); return sec ? { rail: cs.borderLeftWidth, pad: cs.paddingLeft } : null; })() }; }, origin?.brief ?? "");
    // "Show more" and "Open in <parent>" are separate targets: with the first shown (unhidden for the measurement) they never touch.
    const introActions = await page.evaluate(() => {
      const more = document.querySelector(".child-intro .more"), open = document.querySelector(".child-intro .intro-open"); if (!more || !open) return null;
      const wasHidden = more.hidden; more.hidden = false; const a = more.getBoundingClientRect(), b = open.getBoundingClientRect(); more.hidden = wasHidden;
      const sameLine = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 0;
      return { sameLine, gap: sameLine ? Math.round((b.left - a.right) * 10) / 10 : null, moreH: Math.round(a.height), openH: Math.round(b.height), moreW: Math.round(a.width) };
    });
    await page.click(".child-intro .intro-open"); await afterTitle(page, D.SESS[parentOf(grandchild.id)].name); await page.waitForTimeout(260);
    const openedParent = await page.evaluate((id) => ({ id: history.state?.id, handoff: !!document.querySelector('.hcard[data-h="' + id + '"].flash') }), origin?.id);

    await goto(page, { v: "session", id: failedChild.id }, D);
    const returnRow = await page.evaluate(() => { const row = document.querySelector(".child-return"); return { text: row?.textContent, openParent: row?.querySelector("button")?.textContent }; });
    await page.click(".child-return button"); await afterTitle(page, D.SESS[parentOf(failedChild.id)].name); await page.waitForTimeout(260);
    const failedOrigin = D.H.find((h) => h.kind === "spawn" && h.to === failedChild.id);
    const returnParent = await page.evaluate((id) => ({ id: history.state?.id, handoff: !!document.querySelector('.hcard[data-h="' + id + '"].flash') }), failedOrigin?.id);

    const middleIndex = Math.floor(parentKids.length / 2), middle = parentKids[middleIndex];
    await goto(page, { v: "session", id: middle.id }, D);
    for (const scheme of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme: scheme }); await page.waitForTimeout(120);
      await page.screenshot({ path: path.join(ENV.out, "bar-child-" + size + "-" + scheme + ".png"), clip: { x: 0, y: 0, width: size === "phone" ? 390 : 1280, height: size === "phone" ? 360 : 220 } });
    }
    await page.emulateMedia({ colorScheme: "light" });
    const siblingNav = await page.evaluate(() => ({ nav: document.querySelectorAll(".sibling-nav, .sibling-count").length, buttons: [...document.querySelectorAll("button")].filter((b) => /^(Previous|Next) sibling/.test(b.getAttribute("aria-label") ?? "")).length, count: /\b\d+ of \d+\b/.test(document.querySelector("#topbar")?.textContent ?? "") }));

    // Runs and their cost are a list in the session menu's cost section: every descendant, five shown, the rest behind "Show N more".
    await goto(page, { v: "session", id: "harbor" }, D);
    await page.click("#more-btn"); await page.waitForFunction(() => document.querySelector("dialog.session-menu")?.open === true);
    const runs = await page.evaluate(() => { const box = document.querySelector("dialog.session-menu .runs"); return { open: !!box, rows: box?.querySelectorAll(".run-row").length ?? 0, shown: [...(box?.querySelectorAll(".run-row") ?? [])].filter((x) => !x.hidden).length, nested: box?.querySelectorAll(".run-row.depth1").length ?? 0, costs: box?.querySelectorAll(".run-row .v").length ?? 0, apiLabel: document.querySelector("dialog.session-menu .cost")?.textContent.includes("API rates") ?? false }; });
    // On a phone the line is not drawn, so the ⋯ menu's Runs item takes you to the list (and Escape returns focus to ⋯).
    const runsViaMenu = size === "phone" ? await page.locator("dialog.session-menu .menu-runs").isVisible() : null; let runsFocus = null;
    if (runsViaMenu) { await page.locator("dialog.session-menu .menu-runs").click(); runsFocus = await page.evaluate(() => { const runs = document.querySelector("dialog.session-menu .runs"), body = document.querySelector("dialog.session-menu .panel-b"), a = runs.getBoundingClientRect(), b = body.getBoundingClientRect(); return a.top >= b.top - 1 && a.top < b.bottom ? "in view" : "not in view"; }); }
    await page.keyboard.press("Escape"); await page.waitForTimeout(150);
    const focusAfter = await page.evaluate(() => document.activeElement?.id ?? document.activeElement?.tagName ?? null);
    const expectedRuns = Object.values(D.SESS).filter((s) => { let p = parentOf(s.id); while (p && p !== "harbor") p = parentOf(p); return p === "harbor"; }).length;
    childAssertions.push({ size, introActions, pathNames, expectedPath, pathOk, crumbOpens, briefCard, openedParent, returnRow, returnParent, siblingNav, runs, runsViaMenu, runsFocus, focusAfter, expectedRuns });
    await page.context().close();
  }

  for (const child of childAssertions) {
    r.expect(child.pathOk === true && child.crumbOpens === parentOf(grandchild.id), child.size + ": the bar did not show the parent as a crumb before the title, or the crumb did not open the parent: " + JSON.stringify({ expected: child.expectedPath, got: child.pathNames, opens: child.crumbOpens }));
    r.expect(child.briefCard.visible && child.briefCard.includesBrief && child.briefCard.openInParent?.includes("Open in") && child.openedParent.id === parentOf(grandchild.id) && child.openedParent.handoff, child.size + ": child brief or Open in parent handoff link failed: " + JSON.stringify({ brief: child.briefCard, opened: child.openedParent }));
    r.expect(child.briefCard.bars.length > 0 && child.briefCard.bars.every((w) => parseFloat(w) === 0), child.size + ": the child intro still has a left bar: " + JSON.stringify(child.briefCard.bars));
    r.expect(child.briefCard.transcript && parseFloat(child.briefCard.transcript.rail) === 0 && parseFloat(child.briefCard.transcript.pad) === 0, child.size + ": a child session's transcript still carries a left rail or the padding for one: " + JSON.stringify(child.briefCard.transcript));
    r.expect(child.introActions && (!child.introActions.sameLine || child.introActions.gap >= 12) && (child.size !== "phone" || Math.min(child.introActions.moreH, child.introActions.openH) >= 44), child.size + ": the intro's Show more and Open in buttons touch or are under the phone tap size: " + JSON.stringify(child.introActions));
    r.expect(child.returnRow.text?.toLowerCase().includes("failed") && child.returnRow.openParent?.includes("Open in") && child.returnParent.id === parentOf(failedChild.id) && child.returnParent.handoff, child.size + ": failed child return row did not reopen its parent handoff: " + JSON.stringify({ row: child.returnRow, parent: child.returnParent }));
    r.expect(child.siblingNav.nav === 0 && child.siblingNav.buttons === 0 && !child.siblingNav.count, child.size + ": a child session still shows previous/next sibling controls or an \"N of M\" count: " + JSON.stringify(child.siblingNav));
    r.expect(child.runs.open && child.runs.rows === child.expectedRuns && child.runs.shown === Math.min(5, child.expectedRuns) && child.runs.nested > 0 && child.runs.costs === child.expectedRuns && child.runs.apiLabel, child.size + ": the menu's runs list did not hold every descendant with its cost, five shown: " + JSON.stringify(child.runs));
  }
  for (const child of childAssertions) if (child.size === "phone") r.expect(child.runsViaMenu === true && child.runsFocus === "in view" && child.focusAfter === "more-btn", "phone: the ⋯ menu's Runs item did not scroll to the list (" + child.runsFocus + "), or closing the menu left focus on " + child.focusAfter + ", not ⋯ (via menu " + child.runsViaMenu + ")");
  r.expect(childAssertions.length === 2, "lineage, no-sibling-nav, brief/return and Runs checks did not run on phone and desktop: " + childAssertions.length);

  // Thinking shows inline. Readable thinking is drawn in full, with no control to open (no button, nothing hidden), under a
  // "Thinking" label that carries a duration only from one second up. Masked thinking (Claude redacts it, Codex encrypts it) is
  // one quiet line, "Thinking hidden by the harness", at most one per turn. Checked at 390 and 1280.
  // Injected through the served /api/tx path: a readable thought timed 0 s holding one very long unbroken token, two masked
  // thoughts, a readable thought timed 12 s holding a fenced code block with a long line, then two more masked thoughts: two
  // readable thoughts are added, at most one masked line however many the session had, and the page never scrolls sideways.
  const thoughtSid = Object.keys(D.SESS).find((sid) => (D.TX[sid] ?? []).length);
  const countThoughts = () => ({ readable: document.querySelectorAll(".turns .thought:not(.masked)").length, masked: document.querySelectorAll(".turns .thought.masked").length });
  const thoughtsBySize = {};
  if (thoughtSid) for (const size of ["phone", "desktop"]) {
    const plain = await served(browser, { size });
    await goto(plain, { v: "session", id: thoughtSid }, D);
    const base = await plain.evaluate(countThoughts);
    await plain.context().close();
    const page = await served(browser, { size });
    await page.route("**/api/tx*", async (route) => {
      const u = new URL(route.request().url()); if (u.searchParams.get("sid") !== thoughtSid) return route.continue();
      const response = await route.fetch(), body = await response.json();
      body.entries.push({ k: "think", text: "Timing separator " + "A".repeat(300), secs: 0 }, { k: "think", text: "", secs: 2 }, { k: "think", text: "", secs: 3 }, { k: "think", text: "A readable thought\n\n```\nconst longLine = \"" + "y".repeat(240) + "\";\n```", secs: 12 }, { k: "think", text: "" }, { k: "think", text: "" });
      await route.fulfill({ response, json: body });
    });
    await goto(page, { v: "session", id: thoughtSid }, D);
    await page.waitForFunction((n) => document.querySelectorAll(".turns .thought:not(.masked)").length >= n + 2, base.readable);
    const t = await page.evaluate((base) => {
      const readable = [...document.querySelectorAll(".turns .thought:not(.masked)")], masked = [...document.querySelectorAll(".turns .thought.masked")];
      const shown = (x) => { const b = x.getBoundingClientRect(); return b.width > 0 && b.height > 0; };
      const labels = readable.map((x) => (x.querySelector(".think-label")?.textContent ?? "").replace(/\s+/g, " ").trim()); // the viewer spaces a "·" out
      return { addedReadable: readable.length - base.readable, addedMasked: masked.length - base.masked, maxPerTurn: Math.max(0, ...[...document.querySelectorAll(".turns .turn")].map((b) => b.querySelectorAll(".thought.masked").length)),
        inline: readable.length > 0 && readable.every((x) => shown(x.querySelector(".think-text")) && !x.querySelector("button, [aria-expanded], [hidden]") && x.querySelector(".think-text").textContent.trim().length > 0),
        maskedLines: masked.every((x) => x.children.length === 1 && x.textContent.trim() === "Thinking hidden by the harness" && !x.querySelector("button, [aria-expanded]")),
        pairs: masked.filter((x) => x.nextElementSibling?.classList.contains("masked")).length,
        zero: [...document.querySelectorAll(".turns .think-label")].filter((x) => /\b0\s*s\b|Thought for/.test(x.textContent)).map((x) => x.textContent),
        timed: labels.includes("Thinking · 12s"), untimed: labels.filter((x) => x === "Thinking").length > 0,
        oldRows: document.querySelectorAll(".turns button.think").length, emptyBlocks: document.querySelectorAll(".turn > .tx:empty").length, errors: [] };
    }, base);
    t.overflow = await overflow(page); t.errors = page.errors;
    await page.context().close();
    thoughtsBySize[size] = t;
  }
  // The fixture's harbor session has real readable thinking: it is on the page with no click, at both sizes.
  const harborThinking = {};
  if (D.SESS.harbor) for (const size of ["phone", "desktop"]) {
    const page = await served(browser, { size });
    await goto(page, { v: "session", id: "harbor" }, D);
    harborThinking[size] = await page.evaluate(() => { const x = document.querySelector(".turns .thought:not(.masked) .think-text"), b = x?.getBoundingClientRect(); return { text: x?.textContent.trim().length ?? 0, visible: !!b && b.width > 0 && b.height > 0, fits: !!b && b.right <= document.documentElement.clientWidth + 0.5 && b.left >= -0.5 }; });
    harborThinking[size].overflow = await overflow(page); harborThinking[size].errors = page.errors;
    await page.context().close();
  }
  // The fixture's principal session opens a turn with three masked thoughts before its first tool call: they are one quiet
  // line, the tool call is still there, and no turn block is left empty.
  let maskedTurn = null;
  if (D.SESS.principal) {
    const page = await served(browser, { size: "phone" });
    await goto(page, { v: "session", id: "principal" }, D);
    await page.waitForSelector(".turns .step", { state: "attached" });
    maskedTurn = await page.evaluate(() => ({ maskedLines: document.querySelectorAll(".turns .thought.masked").length, adjacent: [...document.querySelectorAll(".turns .thought.masked")].filter((x) => x.nextElementSibling?.classList.contains("masked")).length, label: document.querySelector(".turns .thought.masked")?.textContent.trim() ?? null, steps: document.querySelectorAll(".turns .step").length, emptyBlocks: document.querySelectorAll(".turn > .tx:empty").length, errors: [] }));
    maskedTurn.errors = page.errors;
    await page.context().close();
  }

  // Two turns are added to the served model and transcript of the principal session: one holding only masked thoughts, with
  // no start, no message and no end (it draws its one quiet line and nothing else), and one with a masked thought and then a
  // reply (the line, then the reply). The second also proves the injection reached the page.
  let bareTurns = null;
  if (D.SESS.principal) {
    const page = await served(browser, { size: "phone" });
    await page.route("**/api/model*", async (route) => {
      const response = await route.fetch(); if (response.status() !== 200) return route.fulfill({ response }); // a 304 has no body
      const body = await response.json();
      const base = body.turns.filter((t) => t.sid === "principal").at(-1);
      for (const id of ["bt-bare-turn", "bt-masked-then-reply", "bt-alternating", "bt-in-run"]) body.turns.push({ id, sid: "principal", at: base?.at ?? 0, start: null, u: false, text: "", sent: [], end: null });
      await route.fulfill({ response, json: body });
    });
    await page.route("**/api/tx*", async (route) => {
      const u = new URL(route.request().url()); if (u.searchParams.get("sid") !== "principal") return route.continue();
      const response = await route.fetch(); if (response.status() !== 200) return route.fulfill({ response });
      const body = await response.json();
      const call = (extra = {}) => ({ k: "tool", name: "Bash", arg: "true", in: "true", out: "", ok: true, exit: 0, secs: "0.1s", ...extra }), masked = (extra = {}) => ({ k: "think", text: "", ...extra });
      body.entries.push({ k: "think", text: "", secs: 4, turn: "bt-bare-turn" }, { k: "think", text: "", secs: 5, turn: "bt-masked-then-reply" }, { k: "a", text: "Reply after a masked thought" },
        // masked, call, masked, call, masked, call: one line, then one group of three calls
        masked({ turn: "bt-alternating" }), call(), masked(), call(), masked(), call(),
        // a call, then masked inside the run that is still gathering, then two more calls: the line goes before the run
        call({ turn: "bt-in-run" }), masked(), call(), call());
      await route.fulfill({ response, json: body });
    });
    await page.reload({ waitUntil: "load" }); await settled(page); // served() loaded the model before these routes existed; the app is ready when its bar is drawn
    await goto(page, { v: "session", id: "principal" }, D);
    await page.waitForSelector('section.turn[data-turn="bt-masked-then-reply"]', { timeout: 8000 }).catch(() => {});
    bareTurns = await page.evaluate(() => {
      const withReply = document.querySelector('section.turn[data-turn="bt-masked-then-reply"]'), bare = document.querySelector('section.turn[data-turn="bt-bare-turn"]');
      const shape = (id) => { const b = document.querySelector('section.turn[data-turn="' + id + '"]'), tx = b?.querySelector(".tx"), g = b ? [...b.querySelectorAll(".tgroup")] : [];
        return { block: !!b, masked: b?.querySelectorAll(".thought.masked").length ?? null, groups: g.length, summary: g[0]?.querySelector(".tt")?.textContent.trim() ?? null, steps: g[0]?.querySelectorAll(".step").length ?? null, singles: b?.querySelectorAll(".tx > .steps").length ?? null, order: tx ? [...tx.children].map((c) => c.classList.contains("thought") ? "masked" : c.classList.contains("tgroup") ? "group" : c.className) : null }; };
      return { alternating: shape("bt-alternating"), inRun: shape("bt-in-run"), bareBlock: !!bare, bareLines: bare?.querySelectorAll(".thought.masked").length ?? null, bareMessages: bare?.querySelectorAll(".msg").length ?? null, replyBlock: !!withReply, replyText: withReply?.querySelector(".msg")?.textContent ?? null, linesInReply: withReply?.querySelectorAll(".thought.masked").length ?? null, emptyBlocks: document.querySelectorAll(".turn > .tx:empty").length, errors: [] };
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

  r.results = { modes, expected: X, thoughtsBySize, harborThinking, maskedTurn, bareTurns, extra, childAssertions };
  r.expect(!!thoughtSid, "no session transcript was available for the thinking check");
  for (const [size, t] of Object.entries(thoughtsBySize)) {
    r.expect(t.errors.length === 0, size + " thinking route: page errors: " + t.errors.join(" | "));
    r.expect(t.addedReadable === 2 && t.addedMasked <= 1 && t.maxPerTurn <= 1, size + ": the injected thoughts should add two readable thoughts and at most one masked line, and no turn should hold two: " + JSON.stringify(t));
    r.expect(t.inline === true, size + ": readable thinking is not shown in full without a click: " + JSON.stringify(t));
    r.expect(t.maskedLines === true && t.pairs === 0, size + ": masked thinking is not a single quiet line: " + JSON.stringify(t));
    r.expect(t.zero.length === 0 && t.oldRows === 0, size + ": a thinking label names a zero duration, or the collapsed row is back: " + JSON.stringify(t));
    r.expect(t.timed && t.untimed, size + ": labels should read \"Thinking · 12s\" for 12 s and \"Thinking\" for 0 s: " + JSON.stringify(t));
    r.expect(t.emptyBlocks === 0, size + ": a turn block was left empty by thinking: " + JSON.stringify(t));
    r.expect(t.overflow === 0, size + ": thinking makes the page scroll sideways: " + t.overflow);
  }
  r.expect(!D.SESS.harbor || Object.keys(harborThinking).length === 2, "harbor's inline thinking was not checked at both sizes");
  for (const [size, t] of Object.entries(harborThinking)) {
    r.expect(t.errors.length === 0, size + " harbor: page errors: " + t.errors.join(" | "));
    r.expect(t.text > 0 && t.visible && t.fits, size + ": harbor's readable thinking is not on the page without a click: " + JSON.stringify(t));
    r.expect(t.overflow === 0, size + ": harbor scrolls sideways: " + t.overflow);
  }
  if (bareTurns) {
    for (const id of ["alternating", "inRun"]) {
      const x = bareTurns[id];
      r.expect(x.block && x.masked === 1 && x.groups === 1 && x.summary === "Ran 3 commands" && x.steps === 3 && x.singles === 0 && x.order.join() === "masked,group", id + ": a turn with masked thoughts among its calls should draw one masked line, then one \"Ran 3 commands\" group: " + JSON.stringify(x));
    }
    r.expect(bareTurns.errors.length === 0, "bare masked turn: page errors: " + bareTurns.errors.join(" | "));
    r.expect(bareTurns.replyBlock && bareTurns.replyText?.includes("Reply after a masked thought") && bareTurns.linesInReply === 1, "the injected masked-then-reply turn did not draw as one quiet line and its reply: " + JSON.stringify(bareTurns));
    r.expect(bareTurns.bareBlock && bareTurns.bareLines === 1 && bareTurns.bareMessages === 0 && bareTurns.emptyBlocks === 0, "a turn holding only masked thoughts did not draw as one quiet line: " + JSON.stringify(bareTurns));
  }
  r.expect(!D.SESS.principal || !!bareTurns, "the bare masked turn check did not run");
  r.expect(!D.SESS.principal || !!maskedTurn, "the principal session's masked-thinking turn was not checked");
  if (maskedTurn) {
    r.expect(maskedTurn.errors.length === 0, "principal masked-thinking route: page errors: " + maskedTurn.errors.join(" | "));
    r.expect(maskedTurn.maskedLines > 0 && maskedTurn.adjacent === 0 && maskedTurn.label === "Thinking hidden by the harness", "a turn of masked thoughts is not one quiet line: " + JSON.stringify(maskedTurn));
    r.expect(maskedTurn.steps > 0 && maskedTurn.emptyBlocks === 0, "masked thoughts lost the turn's steps or left an empty block: " + JSON.stringify(maskedTurn));
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
    r.expect(m.smallControls.length === 0, m.mode + ": bar controls under the tap size (36px, 44px on a phone): " + JSON.stringify(m.smallControls));
    r.expect(m.l2NotOneLine.length === 0, m.mode + ": the label line is not one line: " + JSON.stringify(m.l2NotOneLine));
    r.expect(m.l2Overflowing === 0, m.mode + ": the label line overflows: count=" + m.l2Overflowing);
    r.expect((m.mode !== "desktop" || m.sessionMetaPages > 0) && m.metaFailures.length === 0, m.mode + ": session label line (state first with its dot, labels in order, none a control, Find and the menu only): " + JSON.stringify(m.metaFailures.slice(0, 6)));
    if (m.mode !== "desktop") {
      const sl = m.startedLine;
      r.expect(!!sl, m.mode + ": no session showed a Started line to check");
      if (sl) {
        r.expect(sl.h <= sl.leadH + 1, m.mode + ": the Started line wraps with a long machine name: " + JSON.stringify(sl));
        r.expect(sl.cut && sl.ellipsis && sl.inside && !sl.sideways && sl.sidewaysOverflow === 0, m.mode + ": the long machine name is not ellipsised inside the screen: " + JSON.stringify(sl));
        r.expect(!!sl.real && sl.real !== "" && sl.text.startsWith("Started "), m.mode + ": the machine name's full tip is missing: " + JSON.stringify(sl));
      }
    }
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
      r.expect(!!m.search, m.mode + ": the find test never ran");
      if (m.search) {
        r.expect(m.search.searching === true && m.search.chips.slice(0, 3).join() === "all:true,messages:false,steps:false", m.mode + ": find did not open with the field focused and the All chip chosen: " + JSON.stringify({ searching: m.search.searching, chips: m.search.chips }));
        r.expect(m.search.countMatchesHits === true, m.mode + ": find count/hits mismatch: " + JSON.stringify(m.search));
        r.expect(m.search.restored === true, m.mode + ": closing find did not restore the label line");
      }
      r.expect(!!m.filter, m.mode + ": the chips test never ran");
      if (m.filter) {
        r.expect(m.filter.messages.steps === 0 && m.filter.messages.msgs > 0 && m.filter.messages.pressed.join() === "messages", m.mode + ": the Messages chip left steps visible: " + JSON.stringify(m.filter.messages));
        r.expect(m.filter.steps.msgs === 0 && m.filter.steps.steps > 0 && m.filter.steps.pressed.join() === "steps", m.mode + ": the Steps chip left messages visible: " + JSON.stringify(m.filter.steps));
        r.expect(m.filter.restored.msgs === m.filter.all.msgs && m.filter.restored.steps === m.filter.all.steps && m.filter.restored.pressed.join() === "all", m.mode + ": the All chip did not restore the transcript: " + JSON.stringify(m.filter));
      }
      r.expect(!!m.errsJump, m.mode + ": no session with a failed tool call was found for the errors-jump test");
      if (m.errsJump) {
        const n = /^(\d+) errors?$/.exec(m.errsJump.badge ?? "")?.[1];
        r.expect(!!n && m.errsJump.label === "Error 1 of " + n, m.mode + ": errors mode does not read Error 1 of the segment's count: " + JSON.stringify(m.errsJump));
        r.expect(m.errsJump.marked && m.errsJump.expanded === "false", m.mode + ": errors mode did not mark the failed step, or expanded it: " + JSON.stringify(m.errsJump));
        r.expect(m.errsJump.closed === true, m.mode + ": Escape did not leave errors mode");
        r.expect(m.errsJump.inView === true, m.mode + ": errors-jump step not in view");
        r.expect(m.errsJump.menu === false, m.mode + ": errors-jump left a menu open");
        if (m.mode !== "desktop") r.expect(m.errsJump.menuLabel === m.errsJump.badge.replace(" errors", " failed"), m.mode + ": the ⋯ menu's errors item does not read the failed count: " + JSON.stringify({ menu: m.errsJump.menuLabel, badge: m.errsJump.badge }));
      }
      const labs = m.labs ?? {};
      if (labs.errs?.count) {
        r.expect(/^Error 1 of \d+$/.test(labs.errs.label ?? "") && labs.errs.focusBack === true, m.mode + ": the N failed label did not enter errors mode, or Escape did not return focus to it: " + JSON.stringify(labs.errs));
        r.expect(labs.errs.h >= (m.mode.startsWith("phone") ? 44 : 36), m.mode + ": the N failed label is under the tap size: " + JSON.stringify(labs.errs));
        r.expect(labs.errs.findAfter === "", m.mode + ": entering errors mode left the search text in Find: " + JSON.stringify(labs.errs));
        r.expect(labs.errs.chipsAfterNav === "all", m.mode + ": a filter chosen on one session was still chosen on the next: " + JSON.stringify(labs.errs));
      } else r.expect(false, m.mode + ": no N failed label on a session with failed steps: " + JSON.stringify(labs.errs));
      if (labs.runs?.count) r.expect(labs.runs.runsShown === true && labs.runs.h >= (m.mode.startsWith("phone") ? 44 : 36), m.mode + ": the N runs label did not open the menu at its runs, or is under the tap size: " + JSON.stringify(labs.runs));
      else r.expect(false, m.mode + ": no N runs label on a session with runs");
      if (labs.wide) r.expect(labs.wide.onSession === true && labs.wide.onHome === false && labs.wide.backOnSession === true, m.mode + ": wide mode is not limited to session pages: " + JSON.stringify(labs.wide));

      r.expect(!!m.details, m.mode + ": the session menu test never ran");
      if (m.details) {
        const d = m.details, facts = D.SESS[d.session];
        r.expect(d.open === true && d.inView === true, m.mode + ": the session menu did not open visibly from the ⋯ button: " + JSON.stringify(d));
        r.expect(m.mode.startsWith("phone") ? d.phoneSheet === true : d.desktopPanel === true, m.mode + ": the session menu is not the phone's bottom sheet / the desktop's anchored panel: " + JSON.stringify({ phoneSheet: d.phoneSheet, desktopPanel: d.desktopPanel }));
        r.expect(d.title === facts.name && /^Working|^Needs you|^Idle|^Done|^Failed/.test(d.sub), m.mode + ": the menu's header is not the session's name and its state: " + JSON.stringify({ title: d.title, sub: d.sub }));
        r.expect(d.actions.some((a) => a.startsWith("Copy resume command")) && (facts.harness !== "claude" || d.actions.some((a) => a.startsWith("Open in claude.ai"))) && d.wideSwitch === !m.mode.startsWith("phone"), m.mode + ": the menu's actions are wrong (copy, open in claude.ai for Claude, and the wide switch on desktop only): " + JSON.stringify({ actions: d.actions, wide: d.wideSwitch }));
        r.expect(["Harness", "Model", "Machine", "Started", "Duration", "Session id"].every((k) => d.labels.includes(k)) && (d.labels.includes("Branch") || d.labels.includes("Worktree")), m.mode + ": the menu omitted a detail: " + JSON.stringify(d.labels));
        if (facts.cwd) r.expect(d.labels.includes("Directory"), m.mode + ": the menu omitted the directory the model has");
        if (facts.pid != null && facts.pid !== "") r.expect(d.labels.includes("Process id"), m.mode + ": the menu omitted the process id the model has");
        r.expect(/^\$|^—$/.test(d.costBig) && d.costCap.startsWith("this session"), m.mode + ": the menu's cost figure or caption is wrong: " + JSON.stringify({ big: d.costBig, cap: d.costCap }));
        const reports = facts.reported_runs ?? [], costChecks = facts.cost_check ?? [];
        r.expect(reports.length > 0, m.mode + ": the fixture did not expose a reported Claude Code run");
        if (reports.length) r.expect(d.costRows.includes("Claude Code's own figure"), m.mode + ": the harness's own figure was not a row of the cost section: " + JSON.stringify(d.costRows));
        if (costChecks.some((check) => check.ok === false)) r.expect(d.notes.some((n) => /^Semon's estimate for that run is \d+% (above|below) Claude Code's figure/.test(n)), m.mode + ": the difference from the harness's figure was not a quiet note: " + JSON.stringify(d.notes));
        if (childrenOf(d.session).length) r.expect(d.costRows.includes("This session") && d.runRows > 0 && d.runRowsShown === Math.min(5, d.runRows) && (d.runRows <= 5 || (d.runsMore?.startsWith("Show") && d.runRowsAfterMore === d.runRows)), m.mode + ": the runs list is not five rows and Show more: " + JSON.stringify({ rows: d.runRows, shown: d.runRowsShown, more: d.runsMore, after: d.runRowsAfterMore }));
        r.expect(d.tokensHidden === true && d.tokens.shown === true && d.tokens.expanded === "true" && d.tokens.models.length > 0 && ["Input", "Output", "Cache write", "Cache read"].every((k) => d.tokens.kinds.includes(k)), m.mode + ": Tokens by model did not fold and unfold with a line per kind: " + JSON.stringify({ hidden: d.tokensHidden, tokens: d.tokens }));
        r.expect(d.escFocus?.closed === true && d.escFocus?.focus === "more-btn", m.mode + ": Escape on the session menu did not close it and return focus to the ⋯ button: " + JSON.stringify(d.escFocus));
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

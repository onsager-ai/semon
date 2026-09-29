// Stable session order: a list sorted by recency keeps its rows where they are while live updates arrive.
//
// Runs on its own copy of the sample fixture, served by its own `semon sessions --serve`, for 390×844 and 1280×860, light and
// dark (the window is cut to 420 px high so the lists scroll). It appends lines to the fixture's logs as Claude Code would.
// A list is "in use" when its top is scrolled out of view or it is touched; the checks:
//   1. the Sessions list scrolled down (and on the wide screen the sidebar's list too), two sessions from the bottom becoming the
//      most recent and a new session appearing:
//      - no row's top moves, no row is added or reordered, and the scroll position is kept;
//      - the pill shows "N updated" with N = the new session (held) plus the rows that would move to put the old ones in recency
//        order (worked out here from the served model by a plain dynamic programme, not by the page's method);
//      - the pill is on screen, at least 40 px tall (44 on the phone), its label starts with its visible text, and a polite status
//        announces it (screenshots: out/order-<scheme>-page.png and -side.png);
//      - tapping it lists the sessions in recency order with the new session first, scrolled to the top, and the pill gone;
//   2. grouped by Project, scrolled down: a session in a new repo is held (no group appears, the pill counts 1) until the pill is tapped;
//   3. wide screen: a session with no children gets its first child while the sidebar is scrolled down: nothing in the tree moves, the pill
//      counts, and tapping it shows the child;
//   4. the list at its top, untouched (the pointer over the group-by buttons, which are not rows): two sessions bump and the list holds:
//      no row's top moves, a chip "N updated" shows in the group-by row (the count worked out as in 1.) covering no control and with no
//      floating pill, and tapping it sorts; then a new session is put first and the others keep their order, with no chip;
//   5. wide screen: after a mouse click on a tree control (focus stays there, but not as keyboard focus) a new session still goes in;
//   6. Home's Working now and a machine page's Sessions, scrolled down: a session bumps, nothing moves, the pill counts the move, and
//      tapping it sorts;
//   5b. wide screen: a first child under a session below the sidebar's top eight shows no chip and moves nothing; 5c. a new session that
//      arrives with a subagent is put first with its child and no chip; 5d. a hovered row, and a row with keyboard focus, hold a new
//      session (chip "1 updated") until the chip is tapped; 5e. in the rail a session that turns active enters the icons at once;
//   7. on the phone, the drawer's list scrolled down: the same as 1. for the sidebar, with the drawer open. (The sidebar's count in 1. and
//      7. is worked out from the served model.)
// 0 page errors on every page.
//
//   SEMON_BIN   the semon binary built with the test-clock feature (default: target/debug/semon)
//   SEMON_UI_OUT where the report and screenshots go (default: ./out)
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ENV, launch, reporter } from "../lib.mjs";
import { write, ms } from "../fixture.mjs";
import { serve, model, open } from "./live.mjs";

const at = (h, m, s = 0) => ms(h * 60 + m, s);
const iso = (t) => new Date(t).toISOString();
const sleep = (n) => new Promise((r) => setTimeout(r, n));
const SCHEMES = [["phone-light", { size: "phone", dark: false }], ["phone-dark", { size: "phone", dark: true }], ["desktop-light", { size: "desktop", dark: false }], ["desktop-dark", { size: "desktop", dark: true }]];
const OLD = ["harbor", "sentinel", "principal", "atlas"]; // Claude sessions of the sample, each with a log to append to
const PARENTS = ["principal", "sentinel", "advisor"]; // sessions with no child runs, saved open in the tree so a child shows at once

// ---- Log lines ----------------------------------------------------------------------------------------------------------------
function logs(dir) {
  const projects = path.join(dir, "claude/projects"), listing = () => fs.readdirSync(projects, { recursive: true }); // (read afresh: sessions come and go)
  const file = (sid) => { const f = listing().find((p) => path.basename(p) === sid + ".jsonl" && !p.includes("subagents")); if (!f) throw new Error("no " + sid + ".jsonl"); return path.join(projects, f); };
  const cwdOf = (f) => fs.readFileSync(f, "utf8").split("\n").filter(Boolean).map((l) => { try { return JSON.parse(l).cwd; } catch { return null; } }).find(Boolean);
  const said = (sid, cwd, t, text) => JSON.stringify({ parentUuid: null, isSidechain: false, type: "assistant", timestamp: iso(t), sessionId: sid, cwd, version: "2.1.0", uuid: "u-order-" + sid + "-" + t,
    message: { id: "msg-order-" + sid + "-" + t, model: "claude-opus-5-5", role: "assistant", type: "message", content: [{ type: "text", text }] } }) + "\n";
  let n = 0;
  return {
    has: (sid) => listing().some((p) => path.basename(p) === sid + ".jsonl" && !p.includes("subagents")),
    // The session says something at `t`: it is the most recent one from then on.
    bump: (sid, t) => { const f = file(sid); fs.appendFileSync(f, said(sid, cwdOf(f), t, "Order check: more work " + n++)); },
    // The session starts a subagent (its Agent call and the subagent's own file), as live.mjs does.
    spawn: (sid, child, t) => {
      const f = file(sid), cwd = cwdOf(f), sub = path.join(path.dirname(f), sid, "subagents", "agent-" + child + ".jsonl");
      const call = "toolu-order-" + child;
      fs.appendFileSync(f, JSON.stringify({ parentUuid: null, isSidechain: false, type: "assistant", timestamp: iso(t), sessionId: sid, cwd, version: "2.1.0", uuid: "u-order-" + call, message: { id: "msg-order-" + call, model: "claude-opus-5-5", role: "assistant", type: "message", content: [{ type: "tool_use", id: call, name: "Agent", input: { description: "Order child", subagent_type: "general-purpose", prompt: "Order check: a child run", run_in_background: true } }] } }) + "\n"
        + JSON.stringify({ parentUuid: null, isSidechain: false, type: "user", timestamp: iso(t + 500), sessionId: sid, cwd, version: "2.1.0", uuid: "u-order-r-" + call, message: { role: "user", content: [{ type: "tool_result", tool_use_id: call, content: "Async agent launched successfully." }] }, toolUseResult: { status: "async_launched", agentId: child } }) + "\n");
      fs.mkdirSync(path.dirname(sub), { recursive: true });
      fs.writeFileSync(sub.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Order child", toolUseId: call }));
      const line = (at2, type, message) => JSON.stringify({ parentUuid: null, isSidechain: true, type, timestamp: iso(at2), sessionId: sid, agentId: child, cwd, version: "2.1.0", uuid: "u-order-" + child + "-" + at2, message }) + "\n";
      fs.writeFileSync(sub, line(t + 1000, "user", { role: "user", content: "Order check: a child run" }) + line(t + 20000, "assistant", { id: "msg-order-" + child, model: "claude-opus-5-5", role: "assistant", type: "message", content: [{ type: "text", text: "Order check: the child says hello." }] }));
    },
    // A new session in a repo of its own.
    create: (sid, t) => {
      const cwd = path.join(dir, "work", "order-" + sid), f = path.join(projects, cwd.replace(/[^A-Za-z0-9]/g, "-"), sid + ".jsonl");
      fs.mkdirSync(path.join(cwd, ".git"), { recursive: true }); fs.mkdirSync(path.dirname(f), { recursive: true });
      fs.writeFileSync(f, JSON.stringify({ parentUuid: null, isSidechain: false, type: "user", timestamp: iso(t), sessionId: sid, cwd, gitBranch: "main", version: "2.1.0", uuid: "u-order-" + sid, message: { role: "user", content: "Order check: a new session " + sid } }) + "\n" + said(sid, cwd, t + 1000, "Order check: hello from " + sid));
    },
  };
}

// ---- What the page shows ------------------------------------------------------------------------------------------------------
const pageRows = (page) => page.evaluate(() => [...document.querySelectorAll("#page .nrow")].map((n) => ({ id: n.dataset.id, top: n.getBoundingClientRect().top })));
// The rows of the page's first list of sessions (Home's Working now, a machine's Sessions).
const listRows = (page) => page.evaluate(() => { const first = document.querySelector("#page .nrow"); return [...(first?.parentElement.querySelectorAll(":scope > .nrow") ?? [])].map((n) => ({ id: n.dataset.id, top: n.getBoundingClientRect().top })); });
const sideRows = (page) => page.evaluate(() => [...document.querySelectorAll("#lanes > .treeitem")].map((n) => ({ id: n.dataset.id, top: n.getBoundingClientRect().top })));
const pill = (page, name) => page.evaluate((name) => {
  const b = document.querySelector('.order-pill[data-order="' + name + '"]'); if (!b || b.hidden) return null;
  const r = b.getBoundingClientRect(), vw = document.documentElement.clientWidth, vh = innerHeight;
  return { text: b.textContent.trim(), label: b.getAttribute("aria-label"), status: document.querySelector("#order-status")?.textContent ?? "", w: r.width, h: r.height, on: r.left >= 0 && r.right <= vw + 0.5 && r.top >= 0 && r.bottom <= vh + 0.5 };
}, name);
// The chip (a held count in a list's header row): its text, the row it sits in, and every control it overlaps.
const chip = (page, name) => page.evaluate((name) => {
  const c = document.querySelector('.order-chip[data-order="' + name + '"]'); if (!c || c.hidden || !c.parentElement || !c.getClientRects().length || getComputedStyle(c).visibility !== "visible") return null; // (a closed drawer is not visible)
  const r = c.getBoundingClientRect(), hit = (x) => { const q = x.getBoundingClientRect(); return q.width && q.left < r.right - 0.5 && q.right > r.left + 0.5 && q.top < r.bottom - 0.5 && q.bottom > r.top + 0.5; };
  const hits = [...document.querySelectorAll("#page :is(button, input, select), #sidebar :is(button, input), #topbar :is(button, input)")].filter((x) => x !== c && !c.contains(x) && x.getClientRects().length && hit(x)).map((x) => (x.className || x.tagName) + ":" + (x.id || x.textContent.trim().slice(0, 12)));
  return { text: c.textContent.trim(), label: c.getAttribute("aria-label"), h: r.height, host: c.parentElement.className, hits, on: r.left >= 0 && r.right <= document.documentElement.clientWidth + 0.5 };
}, name);
const scrollOf = (page, name) => page.evaluate((name) => (name === "side" ? document.querySelector("#side-list") : window.__sc()).scrollTop, name);
const setScroll = (page, name, top) => page.evaluate(({ name, top }) => { const s = name === "side" ? document.querySelector("#side-list") : window.__sc(); s.scrollTop = Math.min(top, s.scrollHeight - s.clientHeight); return s.scrollTop; }, { name, top });
const ids = (rows) => rows.map((r) => r.id);
const same = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
const still = (before, after) => before.every((b) => { const a = after.find((x) => x.id === b.id); return a && Math.abs(a.top - b.top) <= 0.5; });
// Rows that must move to put `order` (ids, as shown) in `cmp` order: all but the longest run already in it.
const expectedMoves = (order, cmp) => { const d = order.map(() => 1); let best = 0; for (let i = 0; i < order.length; i++) { for (let j = 0; j < i; j++) if (cmp(order[j], order[i]) <= 0) d[i] = Math.max(d[i], d[j] + 1); best = Math.max(best, d[i]); } return order.length - best; };
const inRecency = (order, last) => order.every((id, i) => i === 0 || last[order[i - 1]] >= last[id]);
const RANK = { wait: 0, work: 1, err: 2, done: 3 };
const cmpLast = (last) => (a, b) => last[b] - last[a];
const cmpState = (m) => (a, b) => (RANK[m.sessions[a].state] ?? 3) - (RANK[m.sessions[b].state] ?? 3) || m.sessions[b].last - m.sessions[a].last;
const lastOf = (m) => Object.fromEntries(Object.entries(m.sessions).map(([id, s]) => [id, s.last]));
const scrollPastFirst = (page) => page.evaluate(() => { const s = window.__sc(), n = document.querySelector("#page .nrow"); s.scrollTop += n.getBoundingClientRect().top - window.__line() + 40; return s.scrollTop; });
const allRows = (page) => page.evaluate(() => [...document.querySelectorAll("#lanes .treeitem")].map((n) => ({ id: n.dataset.id, top: n.getBoundingClientRect().top })));
const modelWith = async (srv, ok, limit = 8000) => { const t0 = Date.now(); while (Date.now() - t0 < limit) { const m = await model(srv); if (ok(m)) return m; await sleep(100); } return null; };
const go = async (page, route, title) => { await page.evaluate((r) => { history.pushState(r, ""); dispatchEvent(new PopStateEvent("popstate", { state: r })); }, route); await page.waitForFunction((t) => document.querySelector("#page h1")?.textContent === t && !document.querySelector("#page").hasAttribute("aria-busy") && document.querySelector("#page .nrow"), title); await sleep(300); };
const tap = (page, phone, sel) => (phone ? page.tap(sel) : page.click(sel));
const until = async (page, fn, arg, limit = 14000) => { try { await page.waitForFunction(fn, arg, { timeout: limit, polling: 100 }); return true; } catch { return false; } };

async function scheme(browser, name, opts, r) {
  const R = { name }, phone = opts.size === "phone", say = (ok, what) => r.expect(ok, name + ": " + what), minH = phone ? 44 : 40;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-order-")), now = write(dir);
  const L = logs(dir);
  L.create("order-old", at(0, 5)); // an old session of its own, below the sidebar's top eight from the start
  const srv = await serve(dir, now + 90 * 60000), pages = [];
  const saved = () => { try { localStorage.setItem("semon.tree", JSON.stringify(Object.fromEntries(["principal", "sentinel", "advisor"].map((id) => [id, { open: true, at: 1 }])))); } catch {} };
  try {
    const page = await open(browser, srv, "/sessions", opts, (p) => p.addInitScript(saved)); pages.push(page);
    await page.setViewportSize(phone ? { width: 390, height: 420 } : { width: 1280, height: 420 }); await sleep(300);
    await page.waitForFunction(() => document.querySelectorAll("#page .sess .nrow").length >= 6);
    const pillOf = (n) => pill(page, n);
    const tapPill = (n) => tap(page, phone, '.order-pill[data-order="' + n + '"]');
    const pillShown = (n) => until(page, (n) => document.querySelector('.order-pill[data-order="' + n + '"]:not([hidden])'), n);

    // ---- 1. scrolled down: nothing moves, the pill counts, tapping it sorts ----
    const pageBefore = await pageRows(page), oldIds = ids(pageBefore), targets = OLD.filter((id) => oldIds.includes(id)).sort((a, b) => oldIds.indexOf(b) - oldIds.indexOf(a)).slice(0, 2);
    say(targets.length === 2, "fewer than two of the sample's Claude sessions are on the Sessions page: " + oldIds.join(","));
    R.scrolled = await scrollPastFirst(page);
    say(R.scrolled > 20, "the Sessions list can't be scrolled down (" + R.scrolled + " px)");
    let sideBefore = [];
    if (!phone) { R.sideScrolled = await setScroll(page, "side", 60); say(R.sideScrolled > 10, "the sidebar's list can't be scrolled down (" + R.sideScrolled + " px)"); sideBefore = await sideRows(page); }
    await sleep(300); const pageTops = await pageRows(page);
    const modelPre = await model(srv);
    L.bump(targets[0], at(12, 50)); L.bump(targets[1], at(12, 51)); L.create("order-a", at(12, 52));
    const modelA = await modelWith(srv, (m) => m.sessions["order-a"] && m.sessions[targets[1]].last > m.sessions[targets[0]].last);
    say(!!modelA, "the served model has no new session and no new order within 8 s");
    const last = lastOf(modelA ?? { sessions: {} });
    R.expected = 1 + expectedMoves(oldIds, cmpLast(last));
    await until(page, (n) => document.querySelector('.order-pill[data-order="page"]:not([hidden])')?.textContent.trim() === n + " updated", R.expected);
    // The sidebar's count, from the model: the new lanes that would show (of its top eight), the old lanes that would enter them, and the
    // rows among the eight it shows that are out of order.
    const lanesOf = (m) => Object.entries(m.sessions).filter(([, x]) => x.lane && !x.parent).map(([id]) => id);
    const sideWant = (m0, m1, shown8, l1) => { const before = new Set(lanesOf(m0)), top8 = lanesOf(m1).sort((a, b) => l1[b] - l1[a]).slice(0, 8);
      return top8.filter((id) => !before.has(id)).length + top8.filter((id) => before.has(id) && !shown8.includes(id)).length + expectedMoves(shown8, cmpLast(l1)); };
    R.sideExpected = phone ? null : sideWant(modelPre, modelA ?? { sessions: {} }, ids(sideBefore), last);
    R.pill = await pillOf("page");
    say(R.pill?.text === R.expected + " updated", "the pill says " + JSON.stringify(R.pill?.text) + ", expected " + R.expected + " updated (1 new, " + (R.expected - 1) + " to move)");
    say(!!R.pill && R.pill.on && R.pill.h >= minH && R.pill.w >= 40, "the pill is off screen or under " + minH + " px: " + JSON.stringify(R.pill));
    say(!!R.pill && R.pill.label.startsWith(R.pill.text) && /^\d+ sessions? updated$/.test(R.pill.status), "the pill's label or the status announcement: " + JSON.stringify([R.pill?.label, R.pill?.status]));
    const pageAfter = await pageRows(page);
    say(same(ids(pageAfter), oldIds), "the rows were added or reordered under a scrolled list: " + ids(pageAfter).join(",") + " vs " + oldIds.join(","));
    say(still(pageTops, pageAfter), "a row's top moved under a scrolled list");
    say(Math.abs((await scrollOf(page, "page")) - R.scrolled) <= 1, "the scroll position changed");
    await page.screenshot({ path: path.join(ENV.out, "order-" + name + "-page.png") });
    if (!phone) {
      await pillShown("side"); await until(page, (n) => document.querySelector('.order-pill[data-order="side"]:not([hidden])')?.textContent.trim() === n + " updated", R.sideExpected);
      R.sidePill = await pillOf("side"); const sideAfter = await sideRows(page);
      say(R.sidePill?.text === R.sideExpected + " updated" && R.sidePill.on && R.sidePill.h >= minH, "the sidebar's pill: " + JSON.stringify(R.sidePill) + ", expected " + R.sideExpected + " updated");
      say(same(ids(sideAfter), ids(sideBefore)) && still(sideBefore, sideAfter), "the sidebar's rows moved or changed under a scrolled list: " + ids(sideAfter).join(",") + " vs " + ids(sideBefore).join(","));
      say(Math.abs((await scrollOf(page, "side")) - R.sideScrolled) <= 1, "the sidebar's scroll position changed");
      await page.screenshot({ path: path.join(ENV.out, "order-" + name + "-side.png") });
    }
    await tapPill("page");
    await until(page, () => document.querySelector("#page .sess .nrow")?.dataset.id === "order-a");
    const sorted = ids(await pageRows(page));
    R.sorted = sorted.slice(0, 4);
    say(sorted[0] === "order-a" && inRecency(sorted, last) && same([...sorted].sort(), Object.keys(last).sort()), "tapping the pill didn't list every session in recency order, the new one first: " + sorted.join(","));
    say((await scrollOf(page, "page")) <= 1 && !(await pillOf("page")), "tapping the pill didn't scroll to the top and clear it");
    if (!phone) {
      await tapPill("side");
      await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-a");
      const side = ids(await sideRows(page)); const lastRoots = side.map((id) => last[id]);
      say(side[0] === "order-a" && lastRoots.every((t, i) => i === 0 || lastRoots[i - 1] >= t), "tapping the sidebar's pill didn't list its rows in recency order, the new one first: " + side.join(","));
      say((await scrollOf(page, "side")) <= 1 && !(await pillOf("side")), "tapping the sidebar's pill didn't scroll it to the top and clear it");
    }

    // ---- 2. grouped by Project, scrolled down: a session in a new repo is held, and so is its group ----
    await tap(page, phone, '#page .groupby button[data-g="project"]'); await sleep(300);
    R.groupScrolled = await scrollPastFirst(page); await sleep(300);
    const groupsBefore = await page.evaluate(() => [...document.querySelectorAll("#page .sess .sec-h")].map((n) => n.textContent)), gRows = await pageRows(page);
    L.create("order-d", at(12, 53));
    await modelWith(srv, (m) => m.sessions["order-d"]);
    await pillShown("page"); await sleep(600);
    const groupsHeld = await page.evaluate(() => [...document.querySelectorAll("#page .sess .sec-h")].map((n) => n.textContent)), gAfter = await pageRows(page), gPill = await pillOf("page");
    say(same(groupsHeld, groupsBefore) && same(ids(gAfter), ids(gRows)) && still(gRows, gAfter), "a new group or its row appeared under a scrolled list grouped by Project: " + JSON.stringify([groupsBefore.length, groupsHeld.length]));
    say(gPill?.text === "1 updated", "the held group's pill says " + JSON.stringify(gPill?.text) + ", not 1 updated");
    await tapPill("page");
    await until(page, () => [...document.querySelectorAll("#page .sess .sec-h")].some((n) => n.textContent.includes("order-order-d")));
    say((await page.evaluate(() => [...document.querySelectorAll("#page .sess .sec-h")].map((n) => n.textContent))).some((t) => t.includes("order-order-d")), "tapping the pill didn't show the new group");
    await tap(page, phone, '#page .groupby button[data-g="recent"]'); await sleep(300);

    // ---- 3. wide screen: a session with no children gets its first child while the sidebar is scrolled down ----
    if (!phone) {
      const parent = await page.evaluate((c) => c.find((id) => { const it = document.querySelector('#lanes > .treeitem[data-id="' + id + '"]'); return it && !it.querySelector(":scope > .tree-group"); }), PARENTS.filter((id) => L.has(id)));
      say(!!parent, "no session without children is in the sidebar's top eight");
      if (parent) {
        await setScroll(page, "side", 60); await sleep(300);
        const before = await allRows(page);
        L.spawn(parent, "orderchild", at(12, 54));
        await modelWith(srv, (m) => m.sessions.orderchild);
        await pillShown("side"); await sleep(2500);
        const held = await allRows(page), sp = await pillOf("side");
        say(same(ids(held), ids(before)) && still(before, held), "the tree moved when a session got its first child: " + ids(held).join(",") + " vs " + ids(before).join(","));
        say(!!sp && /^[1-9]\d* updated$/.test(sp.text), "the sidebar's pill after a first child: " + JSON.stringify(sp));
        await tapPill("side");
        say(await until(page, () => !!document.querySelector('#lanes .treeitem[data-id="orderchild"]'), null, 6000), "tapping the pill didn't show the first child");
      }
    }

    // ---- 4. at the top, the pointer over the group-by buttons (not a row): a reorder is held, a new session is put first ----
    await page.evaluate(() => { window.__sc().scrollTop = 0; document.querySelector("#side-list").scrollTop = 0; }); await sleep(300);
    if (!phone) await page.hover("#page .groupby"); await sleep(2500);
    for (const n of ["page", "side"]) if (await chip(page, n)) { await tap(page, phone, '.order-chip[data-order="' + n + '"]'); await sleep(500); } // what step 3 left pending
    const rows0 = await pageRows(page), order0 = ids(rows0), side0 = phone ? [] : await sideRows(page);
    const t0 = (await model(srv)).sessions[targets[1]].last;
    L.bump(targets[0], at(12, 56)); L.bump(targets[1], at(12, 57));
    const modelB = await modelWith(srv, (m) => m.sessions[targets[1]].last > t0);
    const lastB = lastOf(modelB ?? { sessions: {} }), want4 = expectedMoves(order0, cmpLast(lastB));
    say(want4 > 0, "the bumps leave the Sessions list in recency order already, so the chip check proves nothing");
    await until(page, (n) => document.querySelector('.order-chip[data-order="page"]:not([hidden])')?.textContent.trim() === n + " updated", want4);
    await sleep(400);
    const c4 = await chip(page, "page"), rows1 = await pageRows(page);
    say(c4?.text === want4 + " updated" && c4.on && c4.hits.length === 0 && /^\d+ updated, /.test(c4.label), "the chip at the top of the list: " + JSON.stringify(c4) + ", expected " + want4 + " updated");
    say(same(ids(rows1), order0) && still(rows0, rows1), "a row moved at the top of a list with a reorder pending");
    say(!(await pillOf("page")), "a floating pill shows over a list at its top");
    await page.screenshot({ path: path.join(ENV.out, "order-" + name + "-chip.png") });
    if (!phone) {
      const c4s = await chip(page, "side"), sd = await sideRows(page);
      say(same(ids(sd), ids(side0)) && still(side0, sd) && !(await pillOf("side")) && (!c4s || (/^[1-9]\d* updated$/.test(c4s.text) && c4s.hits.length === 0)), "the sidebar at its top: " + JSON.stringify(c4s) + " " + ids(sd).join(","));
      if (c4s) await page.screenshot({ path: path.join(ENV.out, "order-" + name + "-chip-side.png") });
    }
    await tap(page, phone, '.order-chip[data-order="page"]');
    await until(page, (id) => document.querySelector("#page .nrow")?.dataset.id === id, [...order0].sort((a, b) => lastB[b] - lastB[a])[0]);
    const topNow = ids(await pageRows(page));
    say(inRecency(topNow, lastB) && same([...topNow].sort(), [...order0].sort()) && !(await chip(page, "page")), "tapping the chip didn't list the sessions in recency order and clear it: " + topNow.slice(0, 6).join(","));
    // a new session: put first, the others keep their order
    if (!phone) await page.mouse.move(640, 4);
    await sleep(300);
    L.create("order-b", at(12, 58));
    say(await until(page, () => document.querySelector("#page .nrow")?.dataset.id === "order-b"), "the new session isn't put first in a list at its top and untouched");
    await sleep(600);
    const afterNew = ids(await pageRows(page));
    say(same(afterNew.slice(1), topNow) && !(await chip(page, "page")) && !(await pillOf("page")), "the rows below the inserted session changed order, or a chip or pill shows for a pure insertion: " + afterNew.slice(0, 5).join(","));
    if (!phone) {
      await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-b");
      const side = ids(await sideRows(page));
      say(side[0] === "order-b" && !(await pillOf("side")), "the sidebar at its top didn't put the new session first: " + side.join(","));
      // ---- 5. a mouse click on a tree control leaves focus there, which doesn't hold new sessions ----
      await page.click("#lanes .tree-toggle"); await sleep(300);
      say(await page.evaluate(() => !!document.activeElement?.closest("#lanes")), "the click left focus outside the tree, so the check below proves nothing");
      await page.mouse.move(640, 4); await page.evaluate(() => { document.querySelector("#side-list").scrollTop = 0; }); await sleep(300); // (the click may have scrolled the toggle into view)
      L.create("order-e", at(12, 59));
      say(await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-e"), "a new session was held after a mouse click left focus on a tree control");

      // What an earlier step left pending is applied by tapping the chips, so each case below starts from lists in recency order.
      const settle = async () => { await page.mouse.move(640, 4); await sleep(2500); for (const n of ["page", "side"]) if (await chip(page, n)) { await page.click('.order-chip[data-order="' + n + '"]'); await sleep(500); } };

      // ---- 5b. a first child under a session below the sidebar's top eight is nothing anyone can see: no chip, no pill ----
      await settle();
      const side5 = await sideRows(page);
      say(!ids(side5).includes("order-old"), "the old session is among the sidebar's top eight, so the check below proves nothing: " + ids(side5).join(","));
      L.spawn("order-old", "oldchild", at(0, 10));
      await modelWith(srv, (m) => m.sessions.oldchild); await sleep(3500);
      const side5b = await sideRows(page);
      say(same(ids(side5b), ids(side5)) && still(side5, side5b) && !(await chip(page, "side")) && !(await pillOf("side")), "a first child under a session nobody sees showed a chip or pill or moved the tree: " + JSON.stringify(await chip(page, "side")));

      // ---- 5c. a new session arriving with a subagent, at the top: put first (with its child), no chip ----
      await settle();
      L.create("order-f", at(13, 20)); L.spawn("order-f", "fchild", at(13, 21));
      await modelWith(srv, (m) => m.sessions.fchild);
      say(await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-f"), "the new session with a subagent wasn't put first in the sidebar");
      await sleep(3000);
      say(!(await chip(page, "side")) && !(await pillOf("side")) && (await page.evaluate(() => !!document.querySelector('#lanes .treeitem[data-id="fchild"]'))), "the new session's own subagent was held, or a chip or pill shows for it: " + JSON.stringify(await chip(page, "side")));

      // ---- 5d. holding while a row is hovered, and while a row has keyboard focus ----
      await settle();
      const first0 = (await pageRows(page))[0].id;
      await page.hover("#page .nrow");
      L.create("order-g", at(13, 30));
      await modelWith(srv, (m) => m.sessions["order-g"]); await sleep(3000);
      const c5 = await chip(page, "page");
      say((await pageRows(page))[0].id === first0 && c5?.text === "1 updated", "a new session was put first over a hovered row, or the chip is missing: " + JSON.stringify(c5));
      await page.mouse.move(640, 4); await page.click('.order-chip[data-order="page"]');
      say(await until(page, () => document.querySelector("#page .nrow")?.dataset.id === "order-g", null, 5000), "tapping the chip didn't show the held session");
      await settle();
      const first1 = (await pageRows(page))[0].id;
      await page.keyboard.press("Shift"); await page.locator("#page .nrow").first().focus();
      say(await page.evaluate(() => document.activeElement?.matches(".nrow:focus-visible")), "the row's focus isn't keyboard focus, so the check below proves nothing");
      L.create("order-h", at(13, 31));
      await modelWith(srv, (m) => m.sessions["order-h"]); await sleep(3000);
      const c5k = await chip(page, "page");
      say((await pageRows(page))[0].id === first1 && c5k?.text === "1 updated", "a new session was put first over a row with keyboard focus, or the chip is missing: " + JSON.stringify(c5k));
      await page.evaluate(() => document.activeElement.blur()); await page.click('.order-chip[data-order="page"]');
      say(await until(page, () => document.querySelector("#page .nrow")?.dataset.id === "order-h", null, 5000), "tapping the chip didn't show the held session (keyboard)");

      // ---- 5e. the rail: a session below its eight icons that turns active enters them at once, with nothing held ----
      await settle();
      await page.click("#rail-toggle"); await page.waitForFunction(() => document.querySelector(".app.rail")); await sleep(400);
      L.bump("order-old", at(13, 40));
      say(await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-old"), "in the rail a session that turned active didn't enter the icons (the sidebar is frozen)");
      say(!(await chip(page, "side")) && !(await pillOf("side")), "the rail shows a chip or pill");
      await page.click("#rail-toggle"); await page.waitForFunction(() => !document.querySelector(".app.rail")); await sleep(400);
      say((await sideRows(page))[0]?.id === "order-old", "leaving the rail didn't draw the sidebar sorted");
    }

    // ---- 6. Home's Working now, then a machine page: scrolled down, a session bumps ----
    const m0 = await model(srv);
    for (const [key, route, title, cmpOf, when] of [
      ["home", { v: "home" }, "Home", null, at(13, 50)],
      ["machine", null, null, null, at(13, 55)],
    ]) {
      let rt = route, ti = title, cand;
      if (key === "home") {
        await go(page, rt, ti);
      } else {
        const c0 = OLD.find((id) => m0.sessions[id]?.machine);
        const mid = m0.sessions[c0].machine, mm = (m0.machines ?? [m0.machine]).find((x) => x.id === mid);
        rt = { v: "machine", id: mid }; ti = mm?.name ?? mid; await go(page, rt, ti);
      }
      const rows = await listRows(page), order = ids(rows);
      const rank = (id) => RANK[m0.sessions[id]?.state] ?? 3;
      cand = OLD.filter((id) => order.includes(id) && order.indexOf(id) > 0 && (key === "home" || rank(order[order.indexOf(id) - 1]) === rank(id))).sort((a, b) => order.indexOf(b) - order.indexOf(a))[0];
      say(!!cand, key + ": no Claude session of the sample to bump in the list: " + order.join(","));
      if (!cand) continue;
      R[key + "Scrolled"] = await scrollPastFirst(page); await sleep(300);
      const tops = await listRows(page);
      L.bump(cand, when);
      const m1 = await modelWith(srv, (m) => m.sessions[cand].last > m0.sessions[cand].last);
      const want = expectedMoves(order, key === "home" ? cmpLast(lastOf(m1)) : cmpState(m1));
      await until(page, (n) => document.querySelector('.order-pill[data-order="page"]:not([hidden])')?.textContent.trim() === n + " updated", want);
      const p = await pillOf("page"), held = await listRows(page);
      say(p?.text === want + " updated", key + ": the pill says " + JSON.stringify(p?.text) + ", expected " + want + " updated");
      say(same(ids(held), order) && still(tops, held), key + ": rows moved under a scrolled list");
      await tapPill("page");
      await until(page, (id) => document.querySelector("#page .nrow")?.dataset.id === id, key === "home" ? cand : ids(rows)[0], 3000);
      const after = ids(await listRows(page));
      say(key === "home" ? after[0] === cand && inRecency(after, lastOf(m1)) : after.indexOf(cand) < order.indexOf(cand) && !(await pillOf("page")), key + ": tapping the pill didn't sort the list: " + after.join(","));
      say((await scrollOf(page, "page")) <= 1 && !(await pillOf("page")), key + ": tapping the pill didn't scroll to the top and clear it");
    }

    // ---- 7. the phone's drawer, its list scrolled down ----
    if (phone) {
      await page.tap("#lead-btn"); await sleep(400);
      R.sideScrolled = await setScroll(page, "side", 60); say(R.sideScrolled > 10, "the drawer's list can't be scrolled down (" + R.sideScrolled + " px)");
      const drawerBefore = await sideRows(page), lanesOld = ids(drawerBefore);
      const t2 = OLD.filter((id) => lanesOld.includes(id)).sort((a, b) => lanesOld.indexOf(b) - lanesOld.indexOf(a)).slice(0, 2);
      say(t2.length === 2, "fewer than two of the sample's Claude sessions are in the drawer: " + lanesOld.join(","));
      await sleep(300); const drawerTops = await sideRows(page), modelPre2 = await model(srv);
      L.bump(t2[0], at(14, 0)); L.bump(t2[1], at(14, 1)); L.create("order-c", at(14, 2));
      const modelC = await modelWith(srv, (m) => m.sessions["order-c"]), want7 = sideWant(modelPre2, modelC ?? { sessions: {} }, lanesOld, lastOf(modelC ?? { sessions: {} }));
      await pillShown("side"); await until(page, (n) => document.querySelector('.order-pill[data-order="side"]:not([hidden])')?.textContent.trim() === n + " updated", want7); await sleep(300);
      R.sidePill = await pillOf("side"); const drawerAfter = await sideRows(page);
      say(R.sidePill?.text === want7 + " updated" && R.sidePill.on && R.sidePill.h >= minH, "the drawer's pill: " + JSON.stringify(R.sidePill) + ", expected " + want7 + " updated");
      say(same(ids(drawerAfter), lanesOld) && still(drawerTops, drawerAfter), "the drawer's rows moved or changed under a scrolled list: " + ids(drawerAfter).join(",") + " vs " + lanesOld.join(","));
      say(Math.abs((await scrollOf(page, "side")) - R.sideScrolled) <= 1, "the drawer's scroll position changed");
      await page.screenshot({ path: path.join(ENV.out, "order-" + name + "-side.png") });
      await tapPill("side");
      await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-c");
      const m = await model(srv), l = lastOf(m), side = ids(await sideRows(page));
      say(side[0] === "order-c" && side.every((id, i) => i === 0 || l[side[i - 1]] >= l[id]), "tapping the drawer's pill didn't list its rows in recency order, the new one first: " + side.join(","));
      say((await scrollOf(page, "side")) <= 1 && !(await pillOf("side")), "tapping the drawer's pill didn't scroll it to the top and clear it");
    }
    R.errors = page.errors; say(page.errors.length === 0, "page errors: " + page.errors.join(" | "));
  } finally {
    for (const p of pages) await p.context().close();
    srv.proc.kill("SIGTERM"); fs.rmSync(dir, { recursive: true, force: true });
  }
  return R;
}

export default async function orderCheck(browser) {
  const r = reporter("order"), out = {};
  for (const [name, opts] of SCHEMES) {
    try { out[name] = await scheme(browser, name, opts, r); }
    catch (e) { r.expect(false, name + ": threw " + (e?.stack ?? e)); }
  }
  r.results = out;
  return r.done();
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  let ok = false;
  try { ok = await orderCheck(browser); } finally { await browser.close(); }
  if (!ok) process.exit(1);
}

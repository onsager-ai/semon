// Held session order: a list sorted by recency keeps its rows where they are while the reader can see it, and what it held is applied
// when the list is out of sight. Nothing on the screen tells the reader what is held (no chip, no pill, no announcement).
//
// Runs on its own copy of the sample fixture, served by its own `semon sessions --serve`, for 390×844 and 1280×860, light and dark (the
// window is cut to 420 px high so the lists scroll). It appends lines to the fixture's logs as Claude Code would. The viewer's 10 s idle
// constant for the wide screen's sidebar is shortened to IDLE through the test hook window.__semonOrderIdleMs.
// After each change a page draw is awaited (a /api/model answer, then a whole poll's time), so "nothing moved" means it was drawn.
//   1. the Sessions list scrolled down, two sessions from the bottom becoming the most recent and a new session appearing (on the wide
//      screen the sidebar's list scrolled down too): no row's top moves, no row is added or reordered, the scroll is kept, and no
//      chip, pill or status element exists. Applying: on the wide screen a pointer over the sidebar's rows holds it past IDLE, and once
//      the pointer is away it draws in recency order within IDLE; the Sessions page draws in recency order, the new session first, when
//      the route changes (Home and back);
//   2. grouped by Project, scrolled down: a session in a new repo is held with its group until the route changes;
//   3. wide screen: a session with no children gets its first child while the sidebar is scrolled down: nothing in the tree moves; it
//      shows once the sidebar has been idle;
//   4. at the top, the pointer over the group-by buttons (not a row): two bumps are held (no row moves) and a new session is put first,
//      the other rows keeping their order;
//   5. wide screen: a mouse click on a tree control leaves focus there, which doesn't hold a new session; 5b. a first child under a session
//      below the sidebar's top eight moves nothing; 5c. a new session arriving with a subagent is put first with its child; 5d. a hovered
//      row, and a row with keyboard focus, hold a new session until the route changes; 5e. in the rail a session that turns active
//      enters the icons at once;
//   6. Home's Working now, then a machine page, scrolled down: a bump moves nothing, and applies when the tab comes back (a
//      visibilitychange to visible);
//   8. the tab comes back (a visibilitychange to visible) with one change held and a second one waiting in a poll the test holds on the
//      wire: the return applies the first from the data the page has, and when the held poll then lands (a real redraw, observed) the
//      rows in view do not move;
//   9. wide screen, the sidebar's idle timer (IDLE, the hook's shortened time): with the pointer away it applies after IDLE and not
//      before; keyboard focus inside the sidebar blocks it and the focus leaving starts the wait again; a pointer passing over it
//      resets the wait; an open session menu (which hangs from the top bar) blocks it;
//   7. on the phone: with the drawer closed a held order stays in the (hidden) list; opening the drawer shows it in recency order at once;
//      while it is open a change moves nothing; closing it applies the change.
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
// No chip, pill or status element exists anywhere.
const noUi = (page) => page.evaluate(() => !document.querySelector(".order-pill, .order-chip, .order-slot, #order-status"));
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
const lastOf = (m) => Object.fromEntries(Object.entries(m.sessions).map(([id, s]) => [id, s.last]));
const scrollPastFirst = (page) => page.evaluate(() => { const s = window.__sc(), n = document.querySelector("#page .nrow"); s.scrollTop += n.getBoundingClientRect().top - window.__line() + 40; return s.scrollTop; });
const allRows = (page) => page.evaluate(() => [...document.querySelectorAll("#lanes .treeitem")].map((n) => ({ id: n.dataset.id, top: n.getBoundingClientRect().top })));
const modelWith = async (srv, ok, limit = 8000) => { const t0 = Date.now(); while (Date.now() - t0 < limit) { const m = await model(srv); if (ok(m)) return m; await sleep(100); } return null; };
const go = async (page, route, title) => { await page.evaluate((r) => { history.pushState(r, ""); dispatchEvent(new PopStateEvent("popstate", { state: r })); }, route); await page.waitForFunction((t) => document.querySelector("#page h1")?.textContent === t && !document.querySelector("#page").hasAttribute("aria-busy") && document.querySelector("#page .nrow"), title); await sleep(300); };
const tap = (page, phone, sel) => (phone ? page.tap(sel) : page.click(sel));
const until = async (page, fn, arg, limit = 14000) => { try { await page.waitForFunction(fn, arg, { timeout: limit, polling: 100 }); return true; } catch { return false; } };

const IDLE = 2500; // the viewer's 10 s idle constant, shortened for the wide screen's sidebar through the test hook
const idleHook = (ms) => { window.__semonOrderIdleMs = ms; };

async function scheme(browser, name, opts, r) {
  const R = { name }, phone = opts.size === "phone", say = (ok, what) => r.expect(ok, name + ": " + what);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-order-")), now = write(dir);
  const L = logs(dir);
  L.create("order-old", at(0, 5)); // an old session of its own, below the sidebar's top eight from the start
  const srv = await serve(dir, now + 150 * 60000), pages = [];
  const gate = { hold: false, held: [], release: async () => { gate.hold = false; const rs = gate.held.splice(0); for (const x of rs) await x.continue(); } }; // holds the page's /api/model polls while set
  const saved = () => { try { localStorage.setItem("semon.tree", JSON.stringify(Object.fromEntries(["principal", "sentinel", "advisor"].map((id) => [id, { open: true, at: 1 }])))); } catch {} };
  try {
    const page = await open(browser, srv, "/sessions", opts, async (p) => { await p.addInitScript(saved); await p.addInitScript(idleHook, IDLE); await p.route(/\/api\/model/, async (route) => { if (gate.hold) gate.held.push(route); else await route.continue(); }); }); pages.push(page);
    await page.setViewportSize(phone ? { width: 390, height: 420 } : { width: 1280, height: 420 }); await sleep(300);
    await page.waitForFunction(() => document.querySelectorAll("#page .sess .nrow").length >= 6);
    // A change has reached the page and been drawn: the served model has it (the caller waited), the page has had an update since
    // `u0`, and a whole poll has passed since (the page polls every 2 s).
    const drawn = async (u0) => { const t0 = Date.now(); while (page.updates <= u0 && Date.now() - t0 < 12000) await sleep(100); await sleep(2600); return page.updates > u0; };
    // The Sessions page draws again for a new route: Home and back applies what it held.
    const hop = async () => { await go(page, { v: "home" }, "Home"); await go(page, { v: "sessions" }, "Sessions"); };
    // What every earlier step held is applied: the page by a route change; the wide screen's sidebar by leaving it alone for IDLE.
    const applyAll = async () => { await hop(); await page.evaluate(() => document.activeElement?.blur?.()); /* (a click in the tree leaves focus there, which blocks the timer) */ if (!phone) { await page.mouse.move(640, 4); await sleep(IDLE + 1500); } };

    // ---- 1. scrolled down: nothing moves; the held order applies when the list is out of sight ----
    const pageBefore = await pageRows(page), oldIds = ids(pageBefore), targets = OLD.filter((id) => oldIds.includes(id)).sort((a, b) => oldIds.indexOf(b) - oldIds.indexOf(a)).slice(0, 2);
    say(targets.length === 2, "fewer than two of the sample's Claude sessions are on the Sessions page: " + oldIds.join(","));
    R.scrolled = await scrollPastFirst(page);
    say(R.scrolled > 20, "the Sessions list can't be scrolled down (" + R.scrolled + " px)");
    let sideBefore = [];
    // (on the wide screen the pointer rests on the sidebar's rows while it holds, or the idle time would apply it before the check)
    if (!phone) { await page.hover("#lanes .srow"); await sleep(200); R.sideScrolled = await setScroll(page, "side", 60); say(R.sideScrolled > 10, "the sidebar's list can't be scrolled down (" + R.sideScrolled + " px)"); sideBefore = await sideRows(page); }
    await sleep(300); const pageTops = await pageRows(page);
    let u0 = page.updates;
    L.bump(targets[0], at(12, 50)); L.bump(targets[1], at(12, 51)); L.create("order-a", at(12, 52));
    const modelA = await modelWith(srv, (m) => m.sessions["order-a"] && m.sessions[targets[1]].last > m.sessions[targets[0]].last);
    say(!!modelA, "the served model has no new session and no new order within 8 s");
    const last = lastOf(modelA ?? { sessions: {} });
    say(await drawn(u0), "the page drew no update after the change");
    const pageAfter = await pageRows(page);
    say(same(ids(pageAfter), oldIds), "the rows were added or reordered under a scrolled list: " + ids(pageAfter).join(",") + " vs " + oldIds.join(","));
    say(still(pageTops, pageAfter), "a row's top moved under a scrolled list");
    say(Math.abs((await scrollOf(page, "page")) - R.scrolled) <= 1, "the scroll position changed");
    say(await noUi(page), "a chip, pill or status element exists");
    if (!phone) {
      const sideAfter = await sideRows(page);
      say(same(ids(sideAfter), ids(sideBefore)) && still(sideBefore, sideAfter), "the sidebar's rows moved or changed under a scrolled list: " + ids(sideAfter).join(",") + " vs " + ids(sideBefore).join(","));
      say(Math.abs((await scrollOf(page, "side")) - R.sideScrolled) <= 1, "the sidebar's scroll position changed");
      // A pointer over the sidebar's rows holds it past the idle time; once the pointer is away it applies within the idle time.
      await page.hover("#lanes .srow"); await sleep(IDLE * 1.8);
      say(same(ids(await sideRows(page)).filter((id) => ids(sideBefore).includes(id)), ids(sideBefore)) && !ids(await sideRows(page)).includes("order-a"), "the sidebar applied what it held under the pointer");
      await page.mouse.move(640, 4);
      say(await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-a", null, IDLE + 7000), "the sidebar didn't apply what it held once the pointer was away and it was idle");
      const side = ids(await sideRows(page)), lastRoots = side.map((id) => last[id]);
      say(side[0] === "order-a" && lastRoots.every((t, i) => i === 0 || lastRoots[i - 1] >= t), "the idle sidebar isn't in recency order, the new one first: " + side.join(","));
    }
    await hop();
    const sorted = ids(await pageRows(page));
    R.sorted = sorted.slice(0, 4);
    say(sorted[0] === "order-a" && inRecency(sorted, last) && same([...sorted].sort(), Object.keys(last).sort()), "a route change didn't list every session in recency order, the new one first: " + sorted.join(","));
    say(await noUi(page), "a chip, pill or status element exists after applying");

    // ---- 2. grouped by Project, scrolled down: a session in a new repo is held, and so is its group ----
    await tap(page, phone, '#page .groupby button[data-g="project"]'); await sleep(300);
    R.groupScrolled = await scrollPastFirst(page); await sleep(300);
    const groupsBefore = await page.evaluate(() => [...document.querySelectorAll("#page .sess .sec-h")].map((n) => n.textContent)), gRows = await pageRows(page);
    u0 = page.updates; L.create("order-d", at(12, 53));
    await modelWith(srv, (m) => m.sessions["order-d"]); await drawn(u0);
    const groupsHeld = await page.evaluate(() => [...document.querySelectorAll("#page .sess .sec-h")].map((n) => n.textContent)), gAfter = await pageRows(page);
    say(same(groupsHeld, groupsBefore) && same(ids(gAfter), ids(gRows)) && still(gRows, gAfter), "a new group or its row appeared under a scrolled list grouped by Project: " + JSON.stringify([groupsBefore.length, groupsHeld.length]));
    await hop();
    say((await page.evaluate(() => [...document.querySelectorAll("#page .sess .sec-h")].map((n) => n.textContent))).some((t) => t.includes("order-order-d")), "a route change didn't show the new group");
    await tap(page, phone, '#page .groupby button[data-g="recent"]'); await sleep(300);

    // ---- 3. wide screen: a session with no children gets its first child while the sidebar is scrolled down ----
    if (!phone) {
      await applyAll();
      const parent = await page.evaluate((c) => c.find((id) => { const it = document.querySelector('#lanes > .treeitem[data-id="' + id + '"]'); return it && !it.querySelector(":scope > .tree-group"); }), PARENTS.filter((id) => L.has(id)));
      say(!!parent, "no session without children is in the sidebar's top eight");
      if (parent) {
        await page.hover("#lanes .srow"); await sleep(200); await setScroll(page, "side", 60); await sleep(300); // (the pointer rests on the sidebar while it holds)
        const before = await allRows(page);
        u0 = page.updates; L.spawn(parent, "orderchild", at(12, 54));
        await modelWith(srv, (m) => m.sessions.orderchild); await drawn(u0);
        const held = await allRows(page);
        say(same(ids(held), ids(before)) && still(before, held), "the tree moved when a session got its first child: " + ids(held).join(",") + " vs " + ids(before).join(","));
        await page.mouse.move(640, 4);
        say(await until(page, () => !!document.querySelector('#lanes .treeitem[data-id="orderchild"]'), null, IDLE + 7000), "the idle sidebar didn't show the first child");
      }
    }

    // ---- 4. at the top, the pointer over the group-by buttons (not a row): a reorder is held, a new session is put first ----
    await applyAll();
    await page.evaluate(() => { window.__sc().scrollTop = 0; document.querySelector("#side-list").scrollTop = 0; }); await sleep(300);
    if (!phone) await page.hover("#page .groupby"); await sleep(500);
    const rows0 = await pageRows(page), order0 = ids(rows0);
    const t0 = (await model(srv)).sessions[targets[1]].last;
    u0 = page.updates; L.bump(targets[0], at(12, 56)); L.bump(targets[1], at(12, 57));
    const modelB = await modelWith(srv, (m) => m.sessions[targets[1]].last > t0);
    const lastB = lastOf(modelB ?? { sessions: {} });
    say(expectedMoves(order0, cmpLast(lastB)) > 0, "the bumps leave the Sessions list in recency order already, so the check below proves nothing");
    say(await drawn(u0), "the page drew no update after the bumps");
    const rows1 = await pageRows(page);
    say(same(ids(rows1), order0) && still(rows0, rows1), "a row moved at the top of an untouched list with a reorder pending");
    say(await noUi(page), "a chip, pill or status element exists at the top of a list");
    // a new session is put first, the others keep the order they had
    if (!phone) await page.mouse.move(640, 4);
    await sleep(300);
    L.create("order-b", at(12, 58));
    say(await until(page, () => document.querySelector("#page .nrow")?.dataset.id === "order-b"), "the new session isn't put first in a list at its top and untouched");
    await sleep(600);
    const afterNew = ids(await pageRows(page));
    say(same(afterNew.slice(1), order0), "a row moved when a new session was put first: " + afterNew.slice(0, 5).join(","));
    if (!phone) {
      await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-b");
      const side = ids(await sideRows(page));
      say(side[0] === "order-b", "the sidebar at its top didn't put the new session first: " + side.join(","));
      // ---- 5. a mouse click on a tree control leaves focus there, which doesn't hold new sessions ----
      await page.click("#lanes .tree-toggle"); await sleep(300);
      say(await page.evaluate(() => !!document.activeElement?.closest("#lanes")), "the click left focus outside the tree, so the check below proves nothing");
      await page.mouse.move(640, 4); await page.evaluate(() => { document.querySelector("#side-list").scrollTop = 0; }); await sleep(300); // (the click may have scrolled the toggle into view)
      L.create("order-e", at(12, 59));
      say(await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-e"), "a new session was held after a mouse click left focus on a tree control");

      // ---- 5b. a first child under a session below the sidebar's top eight is nothing anyone can see ----
      await applyAll();
      const side5 = await sideRows(page);
      say(!ids(side5).includes("order-old"), "the old session is among the sidebar's top eight, so the check below proves nothing: " + ids(side5).join(","));
      u0 = page.updates; L.spawn("order-old", "oldchild", at(0, 10));
      await modelWith(srv, (m) => m.sessions.oldchild); await drawn(u0);
      const side5b = await sideRows(page);
      say(same(ids(side5b), ids(side5)) && still(side5, side5b), "a first child under a session nobody sees moved the tree: " + ids(side5b).join(","));

      // ---- 5c. a new session arriving with a subagent, at the top: put first, with its child ----
      await applyAll();
      L.create("order-f", at(13, 20)); L.spawn("order-f", "fchild", at(13, 21));
      await modelWith(srv, (m) => m.sessions.fchild);
      say(await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-f"), "the new session with a subagent wasn't put first in the sidebar");
      await sleep(3000);
      say(await page.evaluate(() => !!document.querySelector('#lanes .treeitem[data-id="fchild"]')), "the new session's own subagent was held");

      // ---- 5d. holding while a row is hovered, and while a row has keyboard focus ----
      await applyAll();
      const first0 = (await pageRows(page))[0].id;
      await page.hover("#page .nrow");
      u0 = page.updates; L.create("order-g", at(13, 30));
      await modelWith(srv, (m) => m.sessions["order-g"]); await drawn(u0);
      say((await pageRows(page))[0].id === first0, "a new session was put first over a hovered row");
      await page.mouse.move(640, 4); await hop();
      say((await pageRows(page))[0].id === "order-g", "a route change didn't show the session held over a hovered row");
      await applyAll();
      const first1 = (await pageRows(page))[0].id;
      await page.keyboard.press("Shift"); await page.locator("#page .nrow").first().focus();
      say(await page.evaluate(() => document.activeElement?.matches(".nrow:focus-visible")), "the row's focus isn't keyboard focus, so the check below proves nothing");
      u0 = page.updates; L.create("order-h", at(13, 31));
      await modelWith(srv, (m) => m.sessions["order-h"]); await drawn(u0);
      say((await pageRows(page))[0].id === first1, "a new session was put first over a row with keyboard focus");
      await page.evaluate(() => document.activeElement.blur()); await hop();
      say((await pageRows(page))[0].id === "order-h", "a route change didn't show the session held over a row with keyboard focus");

      // ---- 5e. the rail: a session below its eight icons that turns active enters them at once ----
      await applyAll();
      await page.click("#rail-toggle"); await page.waitForFunction(() => document.querySelector(".app.rail")); await sleep(400);
      L.bump("order-old", at(13, 40));
      say(await until(page, () => document.querySelector("#lanes > .treeitem")?.dataset.id === "order-old"), "in the rail a session that turned active didn't enter the icons (the sidebar is frozen)");
      await page.evaluate(() => document.querySelector("#rail-toggle").click()); await page.waitForFunction(() => !document.querySelector(".app.rail")); await sleep(400); // (in the rail a nav item covers the toggle)
      say((await sideRows(page))[0]?.id === "order-old", "leaving the rail didn't draw the sidebar sorted");
    }

    // ---- 6. Home's Working now, then a machine page: scrolled down, a session bumps; it applies when the tab comes back ----
    const m0 = await model(srv);
    for (const [key, route, title, when] of [
      ["home", { v: "home" }, "Home", at(13, 50)],
      ["machine", null, null, at(13, 55)],
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
      u0 = page.updates; L.bump(cand, when);
      const m1 = await modelWith(srv, (m) => m.sessions[cand].last > m0.sessions[cand].last);
      await drawn(u0);
      const held = await listRows(page);
      say(same(ids(held), order) && still(tops, held), key + ": rows moved under a scrolled list");
      say(await noUi(page), key + ": a chip, pill or status element exists");
      await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange"))); await sleep(500); // the tab comes back
      const after = ids(await listRows(page));
      say(key === "home" ? after[0] === cand && inRecency(after, lastOf(m1)) : after.indexOf(cand) < order.indexOf(cand), key + ": the held order didn't apply when the tab came back: " + after.join(","));
    }

    // ---- 7. the phone's drawer: a held order stays while it is closed, shows sorted when it opens, holds while it is open, applies when it closes ----
    if (phone) {
      await hop(); await sleep(400);
      const drawerBefore = await sideRows(page), lanesOld = ids(drawerBefore);
      const t2 = OLD.filter((id) => lanesOld.includes(id)).sort((a, b) => lanesOld.indexOf(b) - lanesOld.indexOf(a)).slice(0, 2);
      say(t2.length === 2, "fewer than two of the sample's Claude sessions are in the drawer: " + lanesOld.join(","));
      u0 = page.updates; L.bump(t2[0], at(14, 0)); L.bump(t2[1], at(14, 1)); L.create("order-c", at(14, 2));
      const modelC = await modelWith(srv, (m) => m.sessions["order-c"]); await drawn(u0);
      // (a new session is put first, being at the top of an untouched list; the reorder is held: the old rows keep their order)
      const closedNow = ids(await sideRows(page));
      say(same(closedNow.slice(1), lanesOld.slice(0, closedNow.length - 1)) && closedNow[0] === "order-c", "the closed drawer's list was reordered with nothing to apply it: " + closedNow.join(","));
      await page.tap("#lead-btn"); await sleep(60);
      const lc = lastOf(modelC ?? { sessions: {} }), opened = ids(await sideRows(page)), openedRoots = opened.map((id) => lc[id]);
      say(opened[0] === "order-c" && openedRoots.every((t, i) => i === 0 || openedRoots[i - 1] >= t), "the drawer didn't show its list in recency order as it opened: " + opened.join(","));
      // open: a change moves nothing
      await sleep(400); R.sideScrolled = await setScroll(page, "side", 60); await sleep(300);
      const open0 = await sideRows(page);
      u0 = page.updates; L.bump(t2[0], at(14, 4)); L.bump(t2[1], at(14, 5)); L.create("order-p", at(14, 6));
      const modelP = await modelWith(srv, (m) => m.sessions["order-p"]); await drawn(u0);
      const open1 = await sideRows(page);
      say(same(ids(open1), ids(open0)) && still(open0, open1), "the open drawer's rows moved: " + ids(open1).join(",") + " vs " + ids(open0).join(","));
      say(Math.abs((await scrollOf(page, "side")) - R.sideScrolled) <= 1, "the open drawer's scroll position changed");
      say(await noUi(page), "a chip, pill or status element exists in the drawer");
      await page.tap("#drawer-close"); await sleep(900);
      const lp = lastOf(modelP ?? { sessions: {} }), closed = ids(await sideRows(page)), closedRoots = closed.map((id) => lp[id]);
      say(closed[0] === "order-p" && closedRoots.every((t, i) => i === 0 || closedRoots[i - 1] >= t), "closing the drawer didn't apply what it held: " + closed.join(","));
    }
    // ---- 8. the tab comes back: what was held applies once, from the data the page has; the catch-up poll's changes are held ----
    {
      await hop(); await page.mouse.move(640, 4); await sleep(500);
      const list = ids(await pageRows(page)), pair = OLD.filter((id) => list.includes(id)).sort((a, b) => list.indexOf(b) - list.indexOf(a)).slice(0, 2), [A, B] = pair;
      say(pair.length === 2, "fewer than two of the sample's Claude sessions are on the Sessions page: " + list.join(","));
      let a0 = (await model(srv)).sessions[A].last; u0 = page.updates;
      L.bump(A, at(14, 20));
      await modelWith(srv, (m) => m.sessions[A].last > a0); say(await drawn(u0), "the page drew no update after the first change");
      say(same(ids(await pageRows(page)), list), "the first change was applied to a list being looked at");
      const modelA1 = await model(srv);
      // The page's next poll is held on the wire; the second change reaches the server; the tab returns; only then does the poll land.
      gate.hold = true; const b0 = (await model(srv)).sessions[B].last; L.bump(B, at(14, 21));
      await modelWith(srv, (m) => m.sessions[B].last > b0);
      const heldPoll = await (async () => { const t0 = Date.now(); while (!gate.held.length && Date.now() - t0 < 6000) await sleep(50); return gate.held.length > 0; })();
      say(heldPoll, "no poll was held, so the check below proves nothing");
      const mark = () => page.evaluate(() => document.querySelectorAll("#page .nrow, #lanes .treeitem").forEach((x) => { x.__d = 1; }));
      const redrawn = () => until(page, () => !document.querySelector("#page .nrow")?.__d, null, 8000);
      await mark(); await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
      say(await redrawn(), "the page didn't draw when the tab returned");
      const applied = await pageRows(page), sideApplied = phone ? [] : await sideRows(page);
      say(applied[0]?.id === A && inRecency(ids(applied), lastOf(modelA1)) && same([...ids(applied)].sort(), [...list].sort()), "the return didn't apply the first change from the data the page has: " + ids(applied).slice(0, 5).join(","));
      say(applied[0]?.id !== B, "the second change was applied by the return, before its poll landed");
      await mark(); u0 = page.updates; await gate.release();
      say(await redrawn() && page.updates > u0, "the catch-up poll wasn't drawn");
      const caught = await pageRows(page);
      say(same(ids(caught), ids(applied)) && still(applied, caught), "the catch-up poll after the tab's return reordered rows that were in view: " + ids(caught).slice(0, 5).join(",") + " vs " + ids(applied).slice(0, 5).join(","));
      if (!phone) say(same(ids(await sideRows(page)), ids(sideApplied)), "the catch-up poll reordered the sidebar's rows");
    }

    // ---- 9. wide screen: the sidebar's idle timer and its guards ----
    if (!phone) {
      let clk = at(14, 30); const nextT = () => (clk += 60000);
      const sideFirst = () => page.evaluate(() => document.querySelector("#lanes > .treeitem")?.dataset.id);
      const waitFirst = async (id, limit) => { const t0 = Date.now(); while (Date.now() - t0 < limit) { if ((await sideFirst()) === id) return Date.now(); await sleep(50); } return null; };
      // Something is held in the sidebar (its two lowest Claude lanes become the most recent), the pointer away: returns when the sidebar drew it.
      const holdSide = async ({ skipApply = false, avoid = null } = {}) => {
        if (!skipApply) await applyAll();
        await page.evaluate(() => { document.querySelector("#side-list").scrollTop = 40; });
        const lanes = ids(await sideRows(page)), t = OLD.filter((id) => id !== avoid && lanes.includes(id)).sort((a, b) => lanes.indexOf(b) - lanes.indexOf(a)).slice(0, 2);
        say(t.length === 2, "fewer than two of the sample's Claude sessions are in the sidebar: " + lanes.join(","));
        await page.evaluate(() => document.querySelectorAll("#lanes .treeitem").forEach((x) => { x.__d = 1; }));
        const pre = (await model(srv)).sessions[t[1]].last;
        L.bump(t[0], nextT()); L.bump(t[1], nextT());
        await modelWith(srv, (m) => m.sessions[t[1]].last > pre);
        say(await until(page, () => !document.querySelector("#lanes .treeitem")?.__d, null, 12000), "the sidebar didn't draw the change");
        return { Td: Date.now(), newest: t[1] };
      };
      // (a) the pointer away: it doesn't apply before IDLE, and does after
      let h = await holdSide(); let tf = await waitFirst(h.newest, IDLE + 4000);
      R.idleMs = tf && tf - h.Td;
      say(tf !== null && tf - h.Td >= IDLE - 400 && tf - h.Td <= IDLE + 3000, "the idle sidebar applied after " + R.idleMs + " ms, not after " + IDLE + " ms");
      // (b) keyboard focus inside it blocks it; taking the focus away starts the wait again
      h = await holdSide();
      await page.keyboard.press("Shift"); await page.locator("#lanes .srow").first().focus(); await sleep(IDLE * 2 + 500);
      say((await sideFirst()) !== h.newest, "the sidebar applied what it held with keyboard focus inside it");
      let tb = Date.now(); await page.evaluate(() => document.activeElement.blur()); tf = await waitFirst(h.newest, IDLE + 4000);
      R.blurMs = tf && tf - tb;
      say(tf !== null && tf - tb >= IDLE - 400, "the sidebar applied " + R.blurMs + " ms after the focus left, not after " + IDLE + " ms (focus leaving didn't start the wait again)");
      // (c) a pointer event resets it: the pointer passes over the sidebar and leaves at Tr, and the wait runs from Tr
      h = await holdSide(); await sleep(1500);
      await page.hover("#lanes .srow"); await page.mouse.move(640, 4); const tr = Date.now(); tf = await waitFirst(h.newest, IDLE + 4000);
      R.pointerMs = tf && tf - tr;
      say(tf !== null && tf - tr >= IDLE - 400 && tf - h.Td >= 1500 + IDLE - 400, "the sidebar applied " + R.pointerMs + " ms after the pointer left, " + (tf && tf - h.Td) + " ms after it drew (a pointer event didn't reset the wait)");
      // (d) an open menu blocks it: the session menu hangs from the top bar, outside the sidebar
      await applyAll();
      await page.evaluate((r) => { history.pushState(r, ""); dispatchEvent(new PopStateEvent("popstate", { state: r })); }, { v: "session", id: "harbor" });
      await page.waitForFunction(() => document.querySelector("#topbar .t")?.textContent === "harbor" && !document.querySelector("#page").hasAttribute("aria-busy") && document.querySelector("#page section[aria-label='Transcript']"));
      await page.click("#more-btn"); await page.waitForSelector(".session-menu"); await page.mouse.move(640, 4);
      h = await holdSide({ skipApply: true, avoid: "harbor" }); await sleep(IDLE * 2 + 500);
      say((await sideFirst()) !== h.newest && (await page.evaluate(() => !!document.querySelector(".session-menu"))), "the sidebar applied what it held with the session menu open");
      const tm = Date.now(); await page.keyboard.press("Escape");
      say(await page.evaluate(() => !document.querySelector(".session-menu")), "Escape didn't close the session menu, so the check below proves nothing");
      tf = await waitFirst(h.newest, IDLE * 2 + 3000); R.menuMs = tf && tf - tm;
      say(tf !== null, "the sidebar never applied what it held once the session menu closed");
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

// Live updates (#40 M3, deliberate difference 3): the viewer follows the logs while it is open, and keeps the view as it was.
//
// Runs on its own copy of the sample fixture, served by its own `semon sessions --serve` (the other checks' servers stay
// untouched), for each screen: 390×844 light and dark, and 1280×860. With Home, harbor's session page and the Timeline open,
// it appends lines to the fixture's log files, as Claude Code would write them:
//   1. harbor's running call gets its result, then an assistant message;
//   2. a new tool call in harbor, then its result;
//   3. harbor starts a subagent (its Agent call, and the subagent's own file and .meta.json);
//   4. Sentinel relays to Principal (the SendMessage and the received message);
//   5. atlas asks you a question (AskUserQuestion), and harbor says one more thing.
//
// Asserted, per screen:
//   - each new piece of work appears within 4 s: the message and the call in harbor's turn, the subagent's Timeline row,
//     the relay's Timeline connector, the question in Home's Needs you;
//   - harbor, scrolled up: the block at the top of the view moves by at most 1 px, what was expanded (steps, groups, child
//     work) stays expanded, and a counted jump button appears (≥40 px, on screen); tapping it goes to the end;
//   - harbor at its end: it stays pinned to the end with no pill; a running call's time ticks, and it becomes a finished
//     step when its result lands;
//   - an open View all sheet stays open with the same text while an update arrives, and the page under it changes only once
//     it closes;
//   - the filter dropdown stays open with its toggles, and find keeps its query, focus, caret, and counts the new match;
//   - Home: the row at the top of the view moves by at most 1 px while rows reorder and a question is inserted above it;
//     the drawer stays
//     open (phone);
//   - the Timeline keeps its zoom, horizontal scroll, and a collapsed child row; at its right edge (within 8 px) it follows
//     new activity, and elsewhere it keeps its position in pixels while the chart grows;
//   - steps opened inside child work that was then closed are still open when it opens again after a redraw;
//   - no poll overlaps another (the page's own count of /api/model and /api/tx requests in flight never exceeds 1), and
//     an update asks only for transcripts that grew: none for another session's lines, exactly one when one child grew;
//   - a step stops running within 4 s when its process dies (no line written);
//   - open child work shows its child's new message, with the view below it held; a selection in a turn whose call is
//     running survives updates; the Sessions search and the Timeline's focused Zoom in survive redraws;
//   - a 403 stops polling and shows "Session ended: reload with the printed URL";
//   - 0 page errors and 0 sideways overflow on every page.
// Once (desktop): polling pauses while the tab is hidden and resumes when it shows; failed polls back off from 2 s,
// doubling (4 s, 8 s, 16 s, capped at 30 s).
//
//   SEMON_BIN   the semon binary built with the test-clock feature (default: target/debug/semon)
//   SEMON_UI_OUT where the report and screenshots go (default: ./out)
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ENV, launch, context, settled, reporter, overflow } from "../lib.mjs";
import { write, ms } from "../fixture.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const BIN = process.env.SEMON_BIN ?? path.resolve(here, "../../../target/debug/semon");
const at = (h, m, s = 0, milli = 0) => ms(h * 60 + m, s, milli);
const iso = (t) => new Date(t).toISOString();
const sleep = (n) => new Promise((r) => setTimeout(r, n));

const SCHEMES = [["phone-light", { size: "phone", dark: false }], ["phone-dark", { size: "phone", dark: true }], ["desktop", { size: "desktop", dark: false }]];
const MESSAGE = "Live check: the suite passed on the second run.";
const LATER = "Live check: one more thing to find.";
const BRIEF = "Live check: review the flush change before it lands.";
const RELAY = "Live check: a new CI failure on main, relayed for triage.";
const QUESTION = "Live check: ship the flush change now?";
const ENDED = "Session ended: reload with the printed URL";

// ---- The server --------------------------------------------------------------------------------------------------------
function serve(dir, now) {
  return new Promise((resolve, reject) => {
    const proc = spawn(BIN, ["sessions", "--serve", "--listen", "127.0.0.1:0", "--claude-home", path.join(dir, "claude"), "--codex-home", path.join(dir, "codex"), "--proc-root", path.join(dir, "proc"), "--cache", path.join(dir, "index.json")],
      { env: { ...process.env, SEMON_TEST_NOW: String(now) }, stdio: ["ignore", "pipe", "pipe"] });
    let log = "";
    const timer = setTimeout(() => reject(new Error("semon printed no URL: " + log)), 20000);
    const read = (d) => { log += d; const m = /http:\/\/127\.0\.0\.1:(\d+)\/\?t=([0-9a-f]+)/.exec(log); if (m) { clearTimeout(timer); resolve({ proc, base: "http://127.0.0.1:" + m[1], token: m[2], log: () => log }); } };
    proc.stdout.on("data", read); proc.stderr.on("data", read);
    proc.on("exit", (code) => { clearTimeout(timer); reject(new Error("semon exited " + code + ": " + log)); });
  });
}
const model = async (srv) => { const r = await fetch(srv.base + "/api/model?t=" + srv.token); if (!r.ok) throw new Error("/api/model " + r.status); return r.json(); };

// ---- Log lines, in the fixture's shapes ------------------------------------------------------------------------------------
function logs(dir) {
  const projects = path.join(dir, "claude/projects"), files = fs.readdirSync(projects, { recursive: true });
  const file = (name) => { const f = files.find((p) => path.basename(p) === name + ".jsonl" && !p.includes("subagents")); if (!f) throw new Error("no " + name + ".jsonl"); return path.join(projects, f); };
  const cwdOf = (f) => fs.readFileSync(f, "utf8").split("\n").filter(Boolean).map((l) => { try { return JSON.parse(l).cwd; } catch { return null; } }).find(Boolean);
  let seq = 0;
  const lane = (sid, agent) => {
    const f = agent ? path.join(path.dirname(file(agent.parent)), agent.parent, "subagents", "agent-" + sid + ".jsonl") : file(sid), cwd = agent ? cwdOf(file(agent.parent)) : cwdOf(f);
    const base = (t, type, extra) => ({ parentUuid: null, isSidechain: !!agent, type, timestamp: iso(t), sessionId: agent ? agent.parent : sid, ...(agent ? { agentId: sid } : {}), cwd, version: "2.1.0", uuid: "u-live-" + sid + "-" + seq++, ...extra });
    const said = (t, content) => base(t, "assistant", { message: { id: "msg-live-" + sid + "-" + seq, model: "claude-opus-5-5", role: "assistant", type: "message", content } });
    return {
      path: f,
      append: (...lines) => fs.appendFileSync(f, lines.map((l) => JSON.stringify(l) + "\n").join("")),
      text: (t, text) => said(t, [{ type: "text", text }]),
      tool: (t, id, name, input) => said(t, [{ type: "tool_use", id, name, input }]),
      result: (t, id, content, extra = {}) => base(t, "user", { message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content }] }, ...extra }),
      prompt: (t, text) => base(t, "user", { message: { role: "user", content: text } }),
      filler: (t) => base(t, "system", { subtype: "turn_duration" }),
      peer: (t, from, name, msg, body) => base(t, "user", { origin: { kind: "peer", from: "uds:/run/user/1000/cc-socks/" + from + ".sock", name, msg_id: msg, body }, message: { role: "user", content: body } }),
    };
  };
  return { lane };
}

// ---- Pages -------------------------------------------------------------------------------------------------------------
// The page counts its own /api/model and /api/tx requests in flight (window.__live.max), from the call to its response.
function counter() {
  const orig = window.fetch; let n = 0;
  window.__live = { max: 0, reset() { this.max = n; } };
  window.fetch = function (...a) { const mine = /\/api\/(model|tx)\b/.test(String(a[0])); if (mine) { n++; window.__live.max = Math.max(window.__live.max, n); } const p = orig.apply(window, a); return mine ? p.finally(() => { n--; }) : p; };
  window.__sc = () => (matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main"));
  window.__line = () => document.querySelector("#topbar").getBoundingClientRect().bottom;
  window.__left = () => { const s = window.__sc(); return s.scrollHeight - s.scrollTop - s.clientHeight; };
}
async function open(browser, srv, where, scheme) {
  const ctx = await context(browser, scheme);
  await ctx.addInitScript(counter);
  const page = await ctx.newPage();
  page.errors = []; page.models = []; page.updates = 0; page.txs = [];
  page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
  page.on("request", (q) => { if (q.url().includes("/api/model")) page.models.push(Date.now()); if (q.url().includes("/api/tx")) page.txs.push(q.url()); });
  page.on("response", (r) => { if (r.url().includes("/api/model") && r.status() === 200) page.updates++; });
  page.setDefaultTimeout(8000);
  await page.route(/.*/, (r) => (r.request().url().startsWith(srv.base + "/") ? r.continue() : r.abort()));
  await page.goto(srv.base + where + "?t=" + srv.token, { waitUntil: "load" });
  await settled(page);
  if (where.startsWith("/s/")) await page.waitForFunction(() => !!document.querySelector("#page section[aria-label='Transcript']"));
  return page;
}
// How long `fn` took to hold after `since`, or null when it didn't within `limit` ms.
async function appear(page, since, fn, arg, limit = 4000) {
  try { await page.waitForFunction(fn, arg, { timeout: Math.max(1, limit - (Date.now() - since)), polling: 50 }); return Date.now() - since; } catch { return null; }
}
// An update drawn: the page got a new model, then had time to draw it.
async function updated(page, before, limit = 4000) {
  const t0 = Date.now(); while (page.updates <= before && Date.now() - t0 < limit) await sleep(50);
  await sleep(700); return page.updates > before;
}
const openKeys = (page) => page.evaluate(() => [...document.querySelectorAll("#page [data-e]")].filter((n) => {
  const t = n.matches(".step") ? n.querySelector(":scope > button") : n.matches(".tgroup") ? n.querySelector(":scope > .tsum") : n.matches(".childwork") ? n.querySelector(":scope > .cw-toggle") : null;
  return t?.getAttribute("aria-expanded") === "true"; }).map((n) => n.dataset.e));
const topOf = (page, sel) => page.evaluate((sel) => document.querySelector(sel)?.getBoundingClientRect().top ?? null, sel);
const byKey = (k) => '[data-e="' + k.replace(/["\\]/g, "\\$&") + '"]';
const secsOf = (t) => { const m = /^(?:(\d+)m )?(\d+)s$/.exec(t ?? ""); return m ? Number(m[1] ?? 0) * 60 + Number(m[2]) : null; };

async function scheme(browser, name, opts, r, protocol) {
  const R = { name };
  const phone = opts.size === "phone";
  const menuAction = async (page, label) => { await page.click("#more-btn"); await page.locator('.menu [role="menuitem"]').filter({ hasText: label }).click(); };
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-"));
  const now = write(dir);
  // The server's clock stands 10 minutes after the sample's now, so the appended lines (12:41–12:46) are in its past.
  const srv = await serve(dir, now + 10 * 60000);
  const L = logs(dir), harbor = L.lane("harbor"), sentinel = L.lane("sentinel"), principal = L.lane("principal"), atlas = L.lane("atlas");
  const pages = [];
  try {
    const S = await open(browser, srv, "/s/claude/harbor", opts); pages.push(S);
    const TL = await open(browser, srv, "/timeline", opts); pages.push(TL);
    const Hm = await open(browser, srv, "/", opts); pages.push(Hm);
    const SP = await open(browser, srv, "/sessions", opts); pages.push(SP);

    // ---- Set up the views ----
    // harbor: every group, every step and the first child work open; scrolled to 40%.
    await S.evaluate(() => { document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click()); document.querySelectorAll('.cw-toggle[aria-expanded="false"]').forEach((x) => x.click()); });
    await S.evaluate(() => document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click()));
    await S.evaluate(() => document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((x) => x.click()));
    const open0 = await openKeys(S);
    R.opened = { steps: open0.filter((k) => !k.startsWith("g:") && !k.startsWith("cw:")).length, groups: open0.filter((k) => k.startsWith("g:")).length, childWork: open0.filter((k) => k.startsWith("cw:")).length };
    r.expect(R.opened.steps >= 3 && R.opened.groups >= 1 && R.opened.childWork >= 2, name + ": harbor didn't open steps, a group and child work to watch: " + JSON.stringify(R.opened));
    const live0 = await S.evaluate(() => document.querySelector('.step.live[data-live="harbor"]')?.dataset.e ?? null);
    r.expect(!!live0, name + ": harbor shows no running step");
    R.room = await S.evaluate(() => { const s = window.__sc(); s.scrollTop = Math.round((s.scrollHeight - s.clientHeight) * 0.4); return window.__left(); });
    r.expect(R.room > 200, name + ": harbor isn't long enough to scroll up from its end: " + R.room);
    await sleep(150);
    const anchor = await S.evaluate(() => { const line = window.__line(); const n = [...document.querySelectorAll("#page .turns :is(.msg, .step, .hcard, .divider, .think)[data-e]")].find((x) => { const b = x.getBoundingClientRect(); return b.height && b.top >= line; }); return n ? { e: n.dataset.e, top: n.getBoundingClientRect().top } : null; });
    r.expect(!!anchor, name + ": no block in view on harbor");
    // Timeline: zoomed in once, scrolled to the middle, quill's child runs collapsed.
    await TL.click('[aria-label="Zoom in"]'); await TL.waitForFunction(() => document.querySelector(".tl-ctl .zl")?.textContent === "120 px/h");
    await TL.evaluate(() => { const s = document.querySelector(".tl-scroll"); s.scrollLeft = Math.round((s.scrollWidth - s.clientWidth) / 2); });
    await TL.click('.tl-disc[data-sid="quill"]'); await TL.waitForFunction(() => document.querySelector('.tl-disc[data-sid="quill"]')?.getAttribute("aria-expanded") === "false");
    await sleep(150);
    const tlState = () => TL.evaluate(() => ({ zoom: document.querySelector(".tl-ctl .zl").textContent, left: document.querySelector(".tl-scroll").scrollLeft, discs: Object.fromEntries([...document.querySelectorAll(".tl-disc")].map((d) => [d.dataset.sid, d.getAttribute("aria-expanded")])) }));
    const tl0 = await tlState();
    r.expect(tl0.left > 0 && tl0.discs.quill === "false" && tl0.discs.harbor === "true", name + ": Timeline setup: " + JSON.stringify(tl0));
    // Home: a short window, scrolled so Working now's first row is the first block in view, 10 px under the bar (a block
    // whose top is just below the bar could leave the heading above it in view, and that heading would be the anchor).
    await Hm.setViewportSize({ width: opts.size === "phone" ? 390 : 1280, height: 420 }); await sleep(150);
    const homeRow = await Hm.evaluate(() => { const n = document.querySelector("#page .nrow"); const s = window.__sc(); s.scrollTop += n.getBoundingClientRect().top - window.__line() + 10; return { id: n.dataset.id, off: n.getBoundingClientRect().top - window.__line() }; });
    r.expect(Math.abs(homeRow.off + 10) < 1.5, name + ": Home can't scroll its first working row to the top: " + JSON.stringify(homeRow));
    const homeTop0 = await topOf(Hm, '#page .nrow[data-id="' + homeRow.id + '"]');
    // Home's view after each step: the row's offset under the bar, the scroll, the room left below, and the rows' order.
    const homeState = () => Hm.evaluate((id) => { const n = document.querySelector('#page .nrow[data-id="' + id + '"]'), s = window.__sc();
      return { off: n ? Math.round((n.getBoundingClientRect().top - window.__line()) * 10) / 10 : null, top: Math.round(s.scrollTop), left: Math.round(window.__left()), pad: document.querySelector("#page").style.paddingBottom, rows: [...document.querySelectorAll("#page .nrow")].map((x) => x.dataset.id).join(","), needs: document.querySelectorAll("#page .ib:not(.quiet)").length }; }, homeRow.id);
    R.homeTrail = [await homeState()];
    for (const p of pages) await p.evaluate(() => window.__live.reset());

    // ---- 1. harbor's running call returns, and it says something; the reader is scrolled up ----
    const beforeEntries = await S.evaluate(() => [...document.querySelectorAll("#page .turns :is(.msg, .step, .hcard, .think, .think-masked, .think-pending)[data-e]")].filter((n) => !n.closest(".cw-body")).map((n) => n.dataset.e));
    let t0 = Date.now();
    harbor.append(harbor.result(at(12, 41), "toolu-b3", "test result: ok. 214 passed; 0 failed"), harbor.text(at(12, 41, 5), MESSAGE));
    R.message = await appear(S, t0, (m) => [...document.querySelectorAll("#page .msg.assistant")].some((x) => x.textContent.includes(m)), MESSAGE);
    r.expect(R.message != null, name + ": harbor's new message didn't appear within 4 s");
    await sleep(300);
    R.finished = await S.evaluate((k) => { const n = document.querySelector('.step[data-e="' + k + '"]'); return n ? !n.classList.contains("live") : null; }, live0);
    r.expect(R.finished === true, name + ": harbor's running step didn't become a finished step: " + R.finished);
    R.homeTrail.push(await homeState());
    R.anchorMoved = (await topOf(S, byKey(anchor.e))) - anchor.top;
    r.expect(Math.abs(R.anchorMoved) <= 1, name + ": harbor's view moved " + R.anchorMoved + " px");
    const open1 = new Set(await openKeys(S));
    R.stillOpen1 = open0.filter((k) => !open1.has(k));
    r.expect(R.stillOpen1.length === 0, name + ": closed by the update: " + R.stillOpen1.join(", "));
    R.jump = await S.evaluate((before) => { const p = document.querySelector(".jump-bottom"); if (!p || p.hidden) return null; const b = p.getBoundingClientRect(), keys = new Set([...document.querySelectorAll("#page .turns :is(.msg, .step, .hcard, .think, .think-masked, .think-pending)[data-e]")].filter((n) => !n.closest(".cw-body")).map((n) => n.dataset.e)); return { text: p.textContent.trim(), added: [...keys].filter((key) => !before.includes(key)).length, w: b.width, h: b.height, top: b.top, left: b.left, right: b.right, bottom: b.bottom, vw: document.documentElement.clientWidth, vh: innerHeight }; }, beforeEntries);
    const jumpCount = R.jump && /^([1-9]\d*) new$/.exec(R.jump.text);
    r.expect(!!jumpCount && Number(jumpCount[1]) === R.jump.added, name + ": jump button count didn't match newly rendered entries: " + JSON.stringify(R.jump));
    if (R.jump) r.expect(R.jump.w >= 40 && R.jump.h >= 40 && R.jump.top >= 0 && R.jump.left >= 0 && R.jump.right <= R.jump.vw + 0.5 && R.jump.bottom <= R.jump.vh, name + ": the jump button is off screen or under 40 px: " + JSON.stringify(R.jump));
    await S.screenshot({ path: path.join(ENV.out, "live-" + name + "-jump.png") });
    await S.click(".jump-bottom"); await S.waitForFunction(() => window.__left() <= 1, null, { timeout: 5000 });
    R.afterJump = await S.evaluate(() => ({ hidden: document.querySelector(".jump-bottom")?.hidden, left: window.__left() }));
    r.expect(R.afterJump.hidden && R.afterJump.left <= 1, name + ": tapping the jump button didn't go to the end: " + JSON.stringify(R.afterJump));

    // ---- 2. a new call while the reader is at the end: pinned, ticking, then finished ----
    // Child work closed with steps open inside it keeps them through a redraw of its turn.
    const inner0 = await S.evaluate(() => { const w = document.querySelector('.childwork[data-e] > .cw-toggle[aria-expanded="true"]').parentElement;
      const keys = [...w.querySelectorAll('.step[data-e] > button[aria-expanded="true"]')].map((b) => b.parentElement.dataset.e); w.querySelector(":scope > .cw-toggle").click(); return { cw: w.dataset.e, keys }; });
    r.expect(inner0.keys.length >= 1, name + ": no step open inside child work to close it over");
    await sleep(200); const top1 = await S.evaluate(() => window.__sc().scrollTop);
    t0 = Date.now();
    harbor.append(harbor.tool(at(12, 42), "toolu-live1", "Bash", { command: "sleep 30 && echo live", description: "Wait" }));
    R.call = await appear(S, t0, () => [...document.querySelectorAll(".step.live .sa")].some((x) => x.textContent.includes("sleep 30")));
    r.expect(R.call != null, name + ": the new call didn't appear within 4 s");
    await sleep(300);
    const live1 = await S.evaluate(() => [...document.querySelectorAll(".step.live")].find((x) => x.querySelector(".sa")?.textContent.includes("sleep 30"))?.dataset.e ?? null);
    R.pinned = await S.evaluate(() => ({ hidden: document.querySelector(".jump-bottom")?.hidden, left: window.__left(), top: window.__sc().scrollTop }));
    R.pinned.before = top1;
    r.expect(R.pinned.hidden && R.pinned.left <= 1 && R.pinned.top > top1, name + ": not kept at the end: " + JSON.stringify(R.pinned));
    R.innerKept = await S.evaluate(({ cw, keys }) => { const w = document.querySelector('.childwork[data-e="' + cw + '"]'), t = w.querySelector(":scope > .cw-toggle"), closed = t.getAttribute("aria-expanded") === "false"; t.click();
      return { closed, open: keys.filter((k) => w.querySelector('.step[data-e="' + k + '"] > button')?.getAttribute("aria-expanded") === "true").length, of: keys.length }; }, inner0);
    r.expect(R.innerKept.closed && R.innerKept.open === R.innerKept.of, name + ": steps open inside closed child work didn't survive the redraw: " + JSON.stringify(R.innerKept));
    const sd = () => S.evaluate((k) => document.querySelector('.step.live[data-e="' + k + '"] .sd')?.textContent ?? null, live1);
    const tickA = await sd(); await sleep(2500); const tickB = await sd();
    R.ticks = [tickA, tickB];
    r.expect(secsOf(tickA) != null && secsOf(tickB) > secsOf(tickA), name + ": the running step's time didn't tick: " + JSON.stringify(R.ticks));
    // A selection in the last turn, while its call runs, survives two updates of harbor itself that change nothing it shows
    // (a line with no entry of its own): the turn is drawn again off-DOM, and only the running time the clock has since
    // rewritten differs, which the comparison leaves out. Each asks for harbor's transcript once.
    const picked = await S.evaluate((m) => { const n = [...document.querySelectorAll("#page .msg.assistant")].find((x) => x.textContent.includes(m)), t = n.querySelector("p").firstChild, r = document.createRange();
      r.setStart(t, 0); r.setEnd(t, 10); getSelection().removeAllRanges(); getSelection().addRange(r); return getSelection().toString(); }, MESSAGE);
    let txs = S.txs.length;
    for (const [k, sec] of [[1, 10], [2, 20]]) { const u0 = S.updates; harbor.append(harbor.filler(at(12, 42, sec))); r.expect(await updated(S, u0), name + ": harbor got no update " + k + " for its own line"); }
    R.selection = await S.evaluate(() => { const g = getSelection(); return { text: g.toString(), connected: !!g.anchorNode?.isConnected }; });
    R.selection.picked = picked; R.selection.tx = S.txs.length - txs;
    r.expect(picked === "Live check" && R.selection.text === picked && R.selection.connected, name + ": the selection didn't survive two updates: " + JSON.stringify(R.selection));
    r.expect(R.selection.tx === 2, name + ": two updates of harbor asked for " + R.selection.tx + " transcripts, not 2");
    t0 = Date.now();
    harbor.append(harbor.result(at(12, 42, 40), "toolu-live1", "live"));
    R.callDone = await appear(S, t0, (k) => { const n = document.querySelector('.step[data-e="' + k + '"]'); return !!n && !n.classList.contains("live"); }, live1);
    r.expect(R.callDone != null, name + ": the call didn't become a finished step within 4 s");

    // ---- 3. a subagent starts while View all is open ----
    await S.locator(".viewall:visible").first().click();
    await S.waitForFunction(() => document.querySelector("dialog.viewer")?.open === true);
    const sheet0 = await S.evaluate(() => document.querySelector("dialog.viewer").textContent);
    t0 = Date.now();
    harbor.append(harbor.tool(at(12, 43), "toolu-live2", "Agent", { description: "Live reviewer", subagent_type: "general-purpose", prompt: BRIEF, run_in_background: true }),
      harbor.result(at(12, 43, 0, 500), "toolu-live2", "Async agent launched successfully.", { toolUseResult: { status: "async_launched", agentId: "live-sub" } }));
    const sub = L.lane("live-sub", { parent: "harbor" });
    fs.mkdirSync(path.dirname(sub.path), { recursive: true });
    fs.writeFileSync(sub.path.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Live reviewer", toolUseId: "toolu-live2" }));
    sub.append(sub.prompt(at(12, 43, 1), BRIEF), sub.text(at(12, 43, 20), "Live check: the flush change looks right."));
    R.subagentRow = await appear(TL, t0, () => !!document.querySelector('.tl-lab[data-sid="live-sub"]'));
    r.expect(R.subagentRow != null, name + ": the subagent's Timeline row didn't appear within 4 s");
    const spawn = (await model(srv)).handoffs.find((h) => h.kind === "spawn" && h.to === "live-sub");
    r.expect(!!spawn, name + ": the model has no spawn to live-sub");
    await sleep(Math.max(0, 4500 - (Date.now() - t0)));
    R.underSheet = await S.evaluate((id) => ({ open: document.querySelector("dialog.viewer")?.open === true, text: document.querySelector("dialog.viewer")?.textContent, card: !!document.querySelector('.hcard[data-h="' + id + '"]') }), spawn?.id ?? "");
    r.expect(R.underSheet.open && R.underSheet.text === sheet0, name + ": the View all sheet changed while open");
    r.expect(!R.underSheet.card, name + ": the page under the View all sheet was redrawn while it was open");
    delete R.underSheet.text;
    t0 = Date.now(); await S.click(".viewer .vclose");
    R.afterSheet = await appear(S, t0, (id) => { const c = document.querySelector('.hcard[data-h="' + id + '"]'); return !!c && c.nextElementSibling?.classList.contains("childwork"); }, spawn?.id ?? "", 2000);
    r.expect(R.afterSheet != null, name + ": the subagent's card and child work didn't appear once the sheet closed");
    const open3 = new Set(await openKeys(S));
    R.stillOpen3 = open0.filter((k) => k !== live0 && !open3.has(k));
    r.expect(R.stillOpen3.length === 0, name + ": closed by the updates: " + R.stillOpen3.join(", "));

    // ---- 3b. open child work grows above the view: the new message shows in it, the view stays, one transcript is asked for ----
    // The first block in view is the card after h-codex's open child work (h-review's), its top 10 px under the bar; the
    // Codex run then says something, so its child work grows above the view. Plenty of the turn is below it.
    await S.setViewportSize({ width: opts.size === "phone" ? 390 : 1280, height: 480 }); await sleep(200);
    const above = await S.evaluate(() => { const cws = [...document.querySelectorAll("#page .turns .childwork")].filter((x) => !x.parentElement.closest(".cw-body")), n = cws[1]?.previousElementSibling;
      const s = window.__sc(); s.scrollTop += n.getBoundingClientRect().top - window.__line() + 10;
      return { e: n.dataset.e, card: n.matches(".hcard"), off: n.getBoundingClientRect().top - window.__line(), left: window.__left(), cw: cws[0].querySelector(":scope > .cw-toggle").getAttribute("aria-expanded") === "true" }; });
    r.expect(above.card && Math.abs(above.off + 10) < 1.5 && above.left > 80 && above.cw, name + ": couldn't put the card under open child work at the top, away from the end: " + JSON.stringify(above));
    await sleep(200); const aboveTop = await topOf(S, byKey(above.e)); txs = S.txs.length;
    const rollout = path.join(dir, "codex/sessions/2026/09/24", fs.readdirSync(path.join(dir, "codex/sessions/2026/09/24")).find((f) => f.endsWith("-h-codex.jsonl")));
    t0 = Date.now();
    fs.appendFileSync(rollout, JSON.stringify({ timestamp: iso(at(12, 43, 30)), type: "response_item", payload: { type: "message", role: "assistant", content: [{ type: "output_text", text: "Live check: the child's new note." }] } }) + "\n");
    R.childNote = await appear(S, t0, () => [...document.querySelectorAll("#page .cw-body .msg")].some((x) => x.textContent.includes("the child's new note")));
    r.expect(R.childNote != null, name + ": the child's new message didn't appear in its open child work within 4 s");
    await sleep(500);
    R.aboveMoved = (await topOf(S, byKey(above.e))) - aboveTop; R.childTx = S.txs.length - txs;
    r.expect(Math.abs(R.aboveMoved) <= 1, name + ": harbor's view moved " + R.aboveMoved + " px when child work above it grew");
    r.expect(R.childTx === 1, name + ": an update to one child asked for " + R.childTx + " transcripts: " + S.txs.slice(txs).join(" "));
    await S.setViewportSize(opts.size === "phone" ? { width: 390, height: 844 } : { width: 1280, height: 860 }); await sleep(200);

    // ---- 4. a relay, with harbor's filter dropdown open and Thinking off ----
    if (phone) await menuAction(S, "Filter transcript"); else await S.click("#filter-btn");
    await S.waitForFunction(() => document.querySelector(".filters.pop")?.hidden === false);
    await S.click("#f-thinking"); await S.waitForFunction(() => document.querySelector("#f-thinking")?.checked === false && document.querySelector(".filters.pop")?.hidden === false);
    await TL.evaluate(() => document.querySelector('[aria-label="Zoom in"]').focus());
    let u = S.updates; t0 = Date.now();
    sentinel.append(sentinel.tool(at(12, 44), "toolu-live3", "SendMessage", { to: "Principal", message: RELAY }),
      sentinel.result(at(12, 44, 0, 200), "toolu-live3", "Message sent to Principal", { toolUseResult: { success: true, msg_id: "m-live" } }));
    principal.append(principal.peer(at(12, 44, 1), 102, "Sentinel", "m-live", RELAY));
    const relay = await (async () => { for (let k = 0; k < 40; k++) { const h = (await model(srv)).handoffs.find((x) => x.kind === "relay" && x.brief === RELAY); if (h) return h; await sleep(100); } return null; })();
    r.expect(!!relay, name + ": the model has no relay from Sentinel");
    R.relayMark = await appear(TL, t0, (id) => [...document.querySelectorAll(".tl-conn.rel")].some((c) => c.dataset.hs.split(",").includes(id)), relay?.id ?? "");
    r.expect(R.relayMark != null, name + ": the relay's Timeline connector didn't appear within 4 s");
    await sleep(300);
    R.tlFocus = await TL.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.tagName);
    r.expect(R.tlFocus === "Zoom in", name + ": the Timeline's focus didn't stay on Zoom in: " + R.tlFocus);
    r.expect(await updated(S, u), name + ": harbor's page got no update for the relay");
    R.homeTrail.push(await homeState());
    R.filters = await S.evaluate((phone) => ({ pop: document.querySelector(".filters.pop")?.hidden === false, expanded: document.querySelector(phone ? "#more-btn" : "#filter-btn")?.getAttribute("aria-expanded"), thinking: document.querySelector("#f-thinking")?.checked, tools: document.querySelector("#f-tools")?.checked }), phone);
    r.expect(R.filters.pop && R.filters.expanded === (phone ? "false" : "true") && R.filters.thinking === false && R.filters.tools === true, name + ": the filters changed: " + JSON.stringify(R.filters));
    await S.keyboard.press("Escape");
    R.timeline = await tlState();
    r.expect(R.timeline.zoom === tl0.zoom && Math.abs(R.timeline.left - tl0.left) <= 1 && JSON.stringify(Object.entries(tl0.discs).filter(([k]) => R.timeline.discs[k] !== tl0.discs[k])) === "[]",
      name + ": the Timeline's zoom, scroll or rows changed: " + JSON.stringify({ before: tl0, after: R.timeline }));

    // ---- 5. a question for you, and one more message while find is open; the drawer open on the phone ----
    if (phone) await menuAction(S, "Find in transcript"); else await S.click('#topbar button[aria-label="Find in transcript"]');
    await S.waitForSelector("#find");
    await S.fill("#find", "Live check"); await S.waitForFunction(() => /^\d+ match/.test(document.querySelector("#topbar .fcount")?.textContent ?? ""));
    const find0 = await S.evaluate(() => { const f = document.querySelector("#find"); return { value: f.value, focus: document.activeElement === f, caret: f.selectionStart, count: document.querySelector("#topbar .fcount").textContent }; });
    if (opts.size === "phone") { await Hm.click("#lead-btn"); await Hm.waitForFunction(() => document.body.classList.contains("drawer-open")); }
    // The Sessions page, redrawn whole by each update: its search keeps its text, focus and caret.
    await SP.fill("#sq", "harbor"); await SP.evaluate(() => { const f = document.querySelector("#sq"); f.focus(); f.setSelectionRange(3, 3); });
    const sp0 = await SP.evaluate(() => { const f = document.querySelector("#sq"); return { value: f.value, focus: document.activeElement === f, caret: f.selectionStart, rows: document.querySelectorAll("#page .nrow").length }; });
    const spU = SP.updates;
    u = S.updates; t0 = Date.now();
    atlas.append(atlas.tool(at(12, 45), "toolu-live4", "AskUserQuestion", { questions: [{ question: QUESTION, header: "Ship", multiSelect: false, options: [{ label: "Ship it", description: "Merge now" }, { label: "Wait", description: "Hold for review" }] }] }));
    harbor.append(harbor.text(at(12, 45, 30), LATER));
    const question = await (async () => { for (let k = 0; k < 40; k++) { const h = (await model(srv)).handoffs.find((x) => x.kind === "toyou" && (x.brief ?? "").includes(QUESTION)); if (h) return h; await sleep(100); } return null; })();
    r.expect(!!question, name + ": the model has no question from atlas");
    R.question = await appear(Hm, t0, (id) => !!document.querySelector('#page .ib:not(.quiet)[data-h="' + id + '"]'), question?.id ?? "");
    r.expect(R.question != null, name + ": the question didn't appear in Home's Needs you within 4 s");
    await sleep(300);
    R.homeTrail.push(await homeState());
    R.homeMoved = (await topOf(Hm, '#page .nrow[data-id="' + homeRow.id + '"]')) - homeTop0;
    r.expect(Math.abs(R.homeMoved) <= 1, name + ": Home's view moved " + R.homeMoved + " px");
    if (opts.size === "phone") {
      R.drawer = await Hm.evaluate(() => ({ open: document.body.classList.contains("drawer-open"), expanded: document.querySelector("#lead-btn")?.getAttribute("aria-expanded") }));
      r.expect(R.drawer.open && R.drawer.expanded === "true", name + ": the drawer didn't stay open: " + JSON.stringify(R.drawer));
      await Hm.click("#drawer-close"); await sleep(300);
    }
    r.expect(await updated(SP, spU), name + ": the Sessions page got no update");
    R.sessionsFind = { before: sp0, after: await SP.evaluate(() => { const f = document.querySelector("#sq"); return { value: f.value, focus: document.activeElement === f, caret: f.selectionStart, rows: document.querySelectorAll("#page .nrow").length }; }) };
    r.expect(sp0.focus && sp0.caret === 3 && JSON.stringify(R.sessionsFind.after) === JSON.stringify(sp0), name + ": the Sessions search changed: " + JSON.stringify(R.sessionsFind));
    const n0 = parseInt(find0.count, 10);
    R.findCount = await appear(S, t0, (n) => parseInt(document.querySelector("#topbar .fcount")?.textContent ?? "", 10) === n + 1, n0);
    r.expect(R.findCount != null, name + ": find didn't count the new match within 4 s (was " + find0.count + ")");
    R.find = await S.evaluate(() => { const f = document.querySelector("#find"); return { value: f?.value, focus: !!f && document.activeElement === f, caret: f?.selectionStart, shown: [...document.querySelectorAll("#page .msg")].some((x) => x.textContent.includes("one more thing to find")) }; });
    r.expect(R.find.value === find0.value && R.find.focus === find0.focus && find0.focus && R.find.caret === find0.caret && R.find.shown, name + ": find changed: " + JSON.stringify({ before: find0, after: R.find }));

    // ---- 6. the Timeline at its right edge follows new activity; elsewhere it keeps its position ----
    const edgeOf = () => TL.evaluate(() => { const s = document.querySelector(".tl-scroll"); return { width: s.scrollWidth, left: s.scrollLeft, gap: s.scrollWidth - s.scrollLeft - s.clientWidth }; });
    await TL.evaluate(() => { const s = document.querySelector(".tl-scroll"); s.scrollLeft = s.scrollWidth; }); await sleep(200);
    const edge0 = await edgeOf(); u = TL.updates;
    principal.append(principal.filler(at(13, 30)));
    r.expect(await updated(TL, u), name + ": the Timeline got no update for new activity");
    R.edge = { before: edge0, after: await edgeOf() };
    r.expect(edge0.gap <= 1 && R.edge.after.width > edge0.width + 50 && R.edge.after.gap <= 1, name + ": the Timeline at its right edge didn't follow new activity: " + JSON.stringify(R.edge));
    await TL.evaluate(() => { const s = document.querySelector(".tl-scroll"); s.scrollLeft = Math.round((s.scrollWidth - s.clientWidth) / 2); }); await sleep(200);
    const mid0 = await edgeOf(); u = TL.updates;
    principal.append(principal.filler(at(13, 40)));
    r.expect(await updated(TL, u), name + ": the Timeline got no update for more activity");
    R.mid = { before: mid0, after: await edgeOf() };
    r.expect(R.mid.after.width > mid0.width + 10 && Math.abs(R.mid.after.left - mid0.left) <= 1, name + ": the Timeline away from its edge didn't keep its position: " + JSON.stringify(R.mid));

    // ---- 7. harbor's process dies mid-call: the step stops running, though no line is written ----
    await S.click('#topbar button[aria-label="Close search"]'); await S.waitForFunction(() => !document.querySelector("#find")); // find from step 5 would hide the call
    t0 = Date.now();
    harbor.append(harbor.tool(at(12, 46), "toolu-live5", "Bash", { command: "sleep 99 && echo gone" }));
    R.lastCall = await appear(S, t0, () => [...document.querySelectorAll(".step.live .sa")].some((x) => x.textContent.includes("sleep 99")));
    r.expect(R.lastCall != null, name + ": the last call didn't appear within 4 s");
    const live5 = await S.evaluate(() => [...document.querySelectorAll(".step.live")].find((x) => x.querySelector(".sa")?.textContent.includes("sleep 99"))?.dataset.e ?? null);
    const pidFile = fs.readdirSync(path.join(dir, "claude/sessions")).find((f) => JSON.parse(fs.readFileSync(path.join(dir, "claude/sessions", f), "utf8")).sessionId === "harbor");
    t0 = Date.now(); fs.rmSync(path.join(dir, "proc", path.basename(pidFile, ".json"), "stat"));
    R.died = await appear(S, t0, (k) => { const n = document.querySelector('.step[data-e="' + k + '"]'); return !!n && !n.classList.contains("live") && n.querySelector(".sd")?.textContent === "no result"; }, live5);
    r.expect(R.died != null, name + ": the step kept running after its process died");

    // ---- Overflow at the screen's own size ----
    await Hm.setViewportSize(opts.size === "phone" ? { width: 390, height: 844 } : { width: 1280, height: 860 }); await sleep(200);
    R.overflow = { harbor: await overflow(S), timeline: await overflow(TL), home: await overflow(Hm), sessions: await overflow(SP) };
    for (const [k, v] of Object.entries(R.overflow)) r.expect(v === 0, name + ": " + k + " overflow=" + v);
    await S.screenshot({ path: path.join(ENV.out, "live-" + name + "-harbor.png") });
    await TL.screenshot({ path: path.join(ENV.out, "live-" + name + "-timeline.png") });
    await Hm.screenshot({ path: path.join(ENV.out, "live-" + name + "-home.png") });

    // ---- Protocol, once: hidden pauses, visible resumes, errors back off ----
    if (protocol) {
      await Hm.evaluate(() => { Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" }); document.dispatchEvent(new Event("visibilitychange")); });
      await sleep(600); let n = Hm.models.length; await sleep(5000);
      R.hiddenPolls = Hm.models.length - n;
      r.expect(R.hiddenPolls === 0, name + ": polled " + R.hiddenPolls + " times while hidden");
      n = Hm.models.length; t0 = Date.now();
      await Hm.evaluate(() => { Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" }); document.dispatchEvent(new Event("visibilitychange")); });
      while (Hm.models.length === n && Date.now() - t0 < 3000) await sleep(50);
      R.resumedIn = Hm.models.length > n ? Date.now() - t0 : null;
      r.expect(R.resumedIn != null, name + ": polling didn't resume when the tab showed again");
      await Hm.route("**/api/model**", (q) => q.fulfill({ status: 500, body: "no" }));
      n = Hm.models.length; t0 = Date.now();
      while (Hm.models.length < n + 4 && Date.now() - t0 < 40000) await sleep(50);
      const times = Hm.models.slice(n, n + 4), gaps = times.slice(1).map((t, i) => t - times[i]);
      R.backoff = gaps;
      r.expect(gaps.length === 3 && gaps[0] >= 3500 && gaps[1] >= 7000 && gaps[2] >= 14000 && gaps[2] < 20000, name + ": errors didn't back off 4 s, 8 s, 16 s: " + JSON.stringify(gaps));
    }

    // ---- 403: polling stops, with a note ----
    await TL.route("**/api/model**", (q) => q.fulfill({ status: 403, contentType: "text/plain", body: "Forbidden" }));
    t0 = Date.now();
    R.ended = await appear(TL, t0, (t) => document.querySelector(".livenote")?.textContent === t, ENDED, 5000);
    r.expect(R.ended != null, name + ": no \"" + ENDED + "\" note after a 403");
    let n = TL.models.length; await sleep(5000);
    R.pollsAfter403 = TL.models.length - n;
    r.expect(R.pollsAfter403 === 0, name + ": polled " + R.pollsAfter403 + " times after a 403");
    R.note = await TL.evaluate(() => { const b = document.querySelector(".livenote")?.getBoundingClientRect(); return b ? { left: b.left, right: b.right, bottom: b.bottom, vw: document.documentElement.clientWidth, vh: innerHeight } : null; });
    r.expect(!!R.note && R.note.left >= 0 && R.note.right <= R.note.vw + 0.5 && R.note.bottom <= R.note.vh, name + ": the note is off screen: " + JSON.stringify(R.note));
    R.overflow.ended = await overflow(TL);
    r.expect(R.overflow.ended === 0, name + ": overflow with the note=" + R.overflow.ended);
    await TL.screenshot({ path: path.join(ENV.out, "live-" + name + "-ended.png") });

    R.inFlight = {}; for (const [k, p] of [["harbor", S], ["timeline", TL], ["home", Hm], ["sessions", SP]]) R.inFlight[k] = await p.evaluate(() => window.__live.max);
    for (const [k, v] of Object.entries(R.inFlight)) r.expect(v <= 1, name + ": " + k + " had " + v + " polls in flight at once");
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, name + ": page errors: " + R.errors.join(" | "));
  } finally {
    for (const p of pages) await p.context().close();
    srv.proc.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return R;
}

export default async function liveCheck(browser) {
  const r = reporter("live");
  const out = {};
  for (const [name, opts] of SCHEMES) {
    try { out[name] = await scheme(browser, name, opts, r, name === "desktop"); }
    catch (e) { r.expect(false, name + ": threw " + (e?.stack ?? e)); }
  }
  r.results = out;
  return r.done();
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  let ok = false;
  try { ok = await liveCheck(browser); } finally { await browser.close(); }
  if (!ok) process.exit(1);
}

// Live updates (#40 M3, deliberate difference 3): the viewer follows the logs while it is open, and keeps the view as it was.
//
// Runs on its own copy of the sample fixture, served by its own `semon sessions --serve` (the other checks' servers stay
// untouched), for each screen: 390×844 light and dark, and 1280×860. With Home, harbor's session page and Analytics open,
// it appends lines to the fixture's log files, as Claude Code would write them:
//   1. harbor's running call gets its result, then an assistant message;
//   2. a new tool call in harbor, then its result;
//   3. harbor starts a subagent (its Agent call, and the subagent's own file and .meta.json);
//   4. Sentinel relays to Principal (the SendMessage and the received message);
//   5. atlas asks you a question (AskUserQuestion), and harbor says one more thing.
//
// Asserted, per screen:
//   - each new piece of work appears within 4 s: the message and the call in harbor's turn, the subagent and relay in the
//     served model, and the question in Home's Needs you;
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
//   - Analytics keeps its selected range and eight figures while live model updates redraw the page;
//   - a session page deep-linked to a middle turn (not loaded to its end) shows the model's tool-call count in its meta line, and
//     follows it as the session makes more calls;
//   - Analytics makes no /api/tx request: its tool-call counts come from the model, equal to each session's /api/tx totals;
//   - steps opened inside child work that was then closed are still open when it opens again after a redraw;
//   - no poll overlaps another (the page's own count of /api/model and /api/tx requests in flight never exceeds 1), and
//     an update asks only for transcripts that grew: none for another session's lines, exactly one when one child grew;
//   - a step stops running within 4 s when its process dies (no line written);
//   - open child work shows its child's new message, with the view below it held; a selection in a turn whose call is
//     running survives updates; the Sessions search and Analytics range focus survive redraws;
//   - the filters (Repo, Machine, Harness, Model: Selects) and the Sessions search field are kept, not rebuilt, by a live update
//     that adds a repo: the same elements stay in the document, the focused one keeps focus (and the search field its text and
//     caret), an open list stays open with its highlight, the new repo is an option, and the selection is unchanged; a selected
//     repo that leaves the data stays, marked "(no sessions)";
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
const THOUGHT = "Live check thinking: the second run needs no new seed.";
const LATER = "Live check: one more thing to find.";
const BRIEF = "Live check: review the flush change before it lands.";
const RELAY = "Live check: a new CI failure on main, relayed for triage.";
const QUESTION = "Live check: ship the flush change now?";
const ENDED = "Session ended: reload with the printed URL";

// ---- The server --------------------------------------------------------------------------------------------------------
export function serve(dir, now) {
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
export const model = async (srv) => { const r = await fetch(srv.base + "/api/model?t=" + srv.token); if (!r.ok) throw new Error("/api/model " + r.status); return r.json(); };

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
      think: (t, text) => said(t, [{ type: "thinking", thinking: text, signature: "sig" }]),
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
  const orig = window.fetch; let n = 0, models = 0;
  window.__live = { max: 0, models: 0, reset() { this.max = n; this.models = models; } };
  window.fetch = function (...a) {
    const url = String(a[0]), mine = /\/api\/(model|tx)\b/.test(url), model = /\/api\/model\b/.test(url);
    if (mine) { n++; window.__live.max = Math.max(window.__live.max, n); }
    if (model) { models++; window.__live.models = Math.max(window.__live.models, models); }
    const p = orig.apply(window, a);
    return mine ? p.finally(() => { n--; if (model) models--; }) : p;
  };
  window.__sc = () => (matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main"));
  window.__line = () => document.querySelector("#topbar").getBoundingClientRect().bottom;
  window.__left = () => { const s = window.__sc(); return s.scrollHeight - s.scrollTop - s.clientHeight; };
}
export async function open(browser, srv, where, scheme, before = null) {
  const ctx = await context(browser, scheme);
  await ctx.addInitScript(counter);
  const page = await ctx.newPage();
  page.errors = []; page.models = []; page.updates = 0; page.txs = [];
  page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
  page.on("request", (q) => { if (q.url().includes("/api/model")) page.models.push(Date.now()); if (q.url().includes("/api/tx")) page.txs.push(q.url()); });
  page.on("response", (r) => { if (r.url().includes("/api/model") && r.status() === 200) page.updates++; });
  page.setDefaultTimeout(8000);
  await page.route(/.*/, (r) => (r.request().url().startsWith(srv.base + "/") ? r.continue() : r.abort()));
  await before?.(page); // routes added here run before the one above
  await page.goto(srv.base + where + (where.includes("?") ? "&" : "?") + "t=" + srv.token, { waitUntil: "load" });
  await settled(page);
  if (where.startsWith("/s/")) await page.waitForFunction(() => !!document.querySelector("#page section[aria-label='Transcript']"));
  return page;
}
// How long `fn` took to hold after `since`, or null when it didn't within `limit` ms.
export async function appear(page, since, fn, arg, limit = 4000) {
  try { await page.waitForFunction(fn, arg, { timeout: Math.max(1, limit - (Date.now() - since)), polling: 50 }); return Date.now() - since; } catch { return null; }
}
// An update drawn: the page got a new model, then had time to draw it.
async function updated(page, before, limit = 4000) {
  const t0 = Date.now(); while (page.updates <= before && Date.now() - t0 < limit) await sleep(50);
  await sleep(700); return page.updates > before;
}
const openKeys = (page) => page.evaluate(() => [...document.querySelectorAll("#page [data-e]")].filter((n) => {
  const t = n.matches(".step") ? n.querySelector(":scope > button") : n.matches(".tgroup") ? n.querySelector(":scope > .tsum") : null;
  return t?.getAttribute("aria-expanded") === "true"; }).map((n) => n.dataset.e));
const topOf = (page, sel) => page.evaluate((sel) => document.querySelector(sel)?.getBoundingClientRect().top ?? null, sel);
const byKey = (k) => '[data-e="' + k.replace(/["\\]/g, "\\$&") + '"]';
const secsOf = (t) => { const m = /^(?:(\d+)m )?(\d+)s$/.exec(t ?? ""); return m ? Number(m[1] ?? 0) * 60 + Number(m[2]) : null; };

async function scheme(browser, name, opts, r, protocol) {
  const R = { name };
  const phone = opts.size === "phone";
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-"));
  const now = write(dir);
  // The server's clock stands 10 minutes after the sample's now, so the appended lines (12:41–12:46) are in its past.
  const srv = await serve(dir, now + 10 * 60000);
  const L = logs(dir), harbor = L.lane("harbor"), sentinel = L.lane("sentinel"), principal = L.lane("principal"), atlas = L.lane("atlas");
  const pages = [];
  try {
    const S = await open(browser, srv, "/s/claude/harbor", opts); pages.push(S);
    const AN = await open(browser, srv, "/analytics", opts); pages.push(AN);
    const Hm = await open(browser, srv, "/", opts); pages.push(Hm);
    const SP = await open(browser, srv, "/sessions", opts); pages.push(SP);

    // ---- Set up the views ----
    // harbor: every group, every step and the first child work open; scrolled to 40%.
    await S.evaluate(() => { document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click()); });
    await S.evaluate(() => document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click()));
    await S.evaluate(() => document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((x) => x.click()));
    const open0 = await openKeys(S);
    R.opened = { steps: open0.filter((k) => !k.startsWith("g:")).length, groups: open0.filter((k) => k.startsWith("g:")).length };
    r.expect(R.opened.steps >= 3 && R.opened.groups >= 1, name + ": harbor didn't open steps and a group to watch: " + JSON.stringify(R.opened));
    const live0 = await S.evaluate(() => document.querySelector('.step.live[data-live="harbor"]')?.dataset.e ?? null);
    r.expect(!!live0, name + ": harbor shows no running step");
    const mainBox = await S.locator("#main").boundingBox();
    await S.mouse.move(mainBox ? mainBox.x + Math.min(mainBox.width / 2, 200) : 195, mainBox ? mainBox.y + Math.min(mainBox.height / 2, 200) : 300);
    await S.mouse.wheel(0, -1000); // Reader input yields the temporary open-at-end pin before we place a precise anchor.
    R.room = await S.evaluate(() => { const s = window.__sc(); s.scrollTop = Math.round((s.scrollHeight - s.clientHeight) * 0.4); return window.__left(); });
    r.expect(R.room > 200, name + ": harbor isn't long enough to scroll up from its end: " + R.room);
    await sleep(150);
    const anchor = await S.evaluate(() => { const line = window.__line(); const n = [...document.querySelectorAll("#page .turns :is(.msg, .step, .event, .child-card, .bubble, .divider, .thought, .think)[data-e]")].find((x) => { const b = x.getBoundingClientRect(); return b.height && b.top >= line; }); return n ? { e: n.dataset.e, top: n.getBoundingClientRect().top } : null; });
    r.expect(!!anchor, name + ": no block in view on harbor");
    // Analytics: select a range and keep its figures present while the other live views change.
    await AN.click('.analytics-range button:has-text("30 d")');
    await AN.waitForFunction(() => document.querySelectorAll(".analytics-metric").length === 8 && document.querySelectorAll(".analytics-chart svg").length === 2);
    await AN.locator('.analytics-range button:has-text("30 d")').focus();
    const analyticsState = () => AN.evaluate(() => ({ range: [...document.querySelectorAll(".analytics-range button")].find((b) => b.getAttribute("aria-pressed") === "true")?.textContent.trim(), metrics: document.querySelectorAll(".analytics-metric").length, charts: document.querySelectorAll(".analytics-chart svg").length }));
    const analytics0 = await analyticsState();
    r.expect(analytics0.range === "30 d" && analytics0.metrics === 8 && analytics0.charts === 2, name + ": Analytics setup: " + JSON.stringify(analytics0));
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
    const beforeEntries = await S.evaluate(() => [...document.querySelectorAll("#page .turns :is(.msg, .step, .event, .child-card, .bubble, .think, .think-pending)[data-e]")].map((n) => n.dataset.e));
    let t0 = Date.now();
    harbor.append(harbor.result(at(12, 41), "toolu-b3", "test result: ok. 214 passed; 0 failed"), harbor.think(at(12, 41, 2), THOUGHT), harbor.text(at(12, 41, 5), MESSAGE));
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
    R.jump = await S.evaluate((before) => { const p = document.querySelector("#jump-bottom"); if (!p || p.closest(".jump-wrap")?.hidden) return null; const b = p.getBoundingClientRect(), keys = new Set([...document.querySelectorAll("#page .turns :is(.msg, .step, .event, .child-card, .bubble, .think, .think-pending)[data-e]")].map((n) => n.dataset.e)); return { text: p.textContent.trim(), added: [...keys].filter((key) => !before.includes(key)).length, w: b.width, h: b.height, top: b.top, left: b.left, right: b.right, bottom: b.bottom, vw: document.documentElement.clientWidth, vh: innerHeight }; }, beforeEntries);
    // The new thought is an entry like any other: it counts toward "N new" (the message and the thought make at least two). The overhaul draws it as a "Thought for Ns" row that opens the text.
    R.thought = await S.evaluate((t) => { const g = [...document.querySelectorAll("#page .turns .thought")].find((x) => x.querySelector(".think-text")?.textContent.includes(t)); const b = g?.querySelector("button.think[data-e]"); return b ? { key: b.dataset.e, collapsed: b.getAttribute("aria-expanded") === "false" } : null; }, THOUGHT);
    r.expect(!!R.thought && !beforeEntries.includes(R.thought.key), name + ": harbor's new thought did not appear as a row, or was already counted: " + JSON.stringify(R.thought));
    r.expect(R.jump && R.jump.added >= 2, name + ": the new thought and message should both count as new: " + JSON.stringify(R.jump));
    const jumpCount = R.jump && /^([1-9]\d*) new$/.exec(R.jump.text);
    r.expect(!!jumpCount && Number(jumpCount[1]) === R.jump.added, name + ": jump button count didn't match newly rendered entries: " + JSON.stringify(R.jump));
    if (R.jump) r.expect(R.jump.w >= 40 && R.jump.h >= 40 && R.jump.top >= 0 && R.jump.left >= 0 && R.jump.right <= R.jump.vw + 0.5 && R.jump.bottom <= R.jump.vh, name + ": the jump button is off screen or under 40 px: " + JSON.stringify(R.jump));
    await S.screenshot({ path: path.join(ENV.out, "live-" + name + "-jump.png") });
    await S.click("#jump-bottom"); await S.waitForFunction(() => window.__left() <= 1, null, { timeout: 5000 });
    R.afterJump = await S.evaluate(() => ({ hidden: document.querySelector(".jump-wrap")?.hidden, left: window.__left() }));
    r.expect(R.afterJump.hidden && R.afterJump.left <= 1, name + ": tapping the jump button didn't go to the end: " + JSON.stringify(R.afterJump));

    // ---- 2. a new call while the reader is at the end: pinned, ticking, then finished ----
    // A group closed with steps open inside it keeps them through a redraw of its turn.
    const inner0 = await S.evaluate(() => { const w = [...document.querySelectorAll('.tgroup[data-e]')].find((g) => g.querySelector(':scope > .steps .step[data-e] > button[aria-expanded="true"]') && g.querySelector(':scope > .tsum[aria-expanded="true"]'));
      if (!w) return { cw: null, keys: [] };
      const keys = [...w.querySelectorAll('.step[data-e] > button[aria-expanded="true"]')].map((b) => b.parentElement.dataset.e); w.querySelector(":scope > .tsum").click(); return { cw: w.dataset.e, keys }; });
    r.expect(inner0.keys.length >= 1, name + ": no step open inside a group to close it over");
    await sleep(200); const top1 = await S.evaluate(() => window.__sc().scrollTop);
    t0 = Date.now();
    harbor.append(harbor.tool(at(12, 42), "toolu-live1", "Bash", { command: "sleep 30 && echo live", description: "Wait" }));
    R.call = await appear(S, t0, () => [...document.querySelectorAll(".step.live .sa")].some((x) => x.textContent.includes("sleep 30")));
    r.expect(R.call != null, name + ": the new call didn't appear within 4 s");
    await sleep(300);
    const live1 = await S.evaluate(() => [...document.querySelectorAll(".step.live")].find((x) => x.querySelector(".sa")?.textContent.includes("sleep 30"))?.dataset.e ?? null);
    R.pinned = await S.evaluate(() => ({ hidden: document.querySelector(".jump-wrap")?.hidden, left: window.__left(), top: window.__sc().scrollTop }));
    R.pinned.before = top1;
    r.expect(R.pinned.hidden && R.pinned.left <= 1 && R.pinned.top > top1, name + ": not kept at the end: " + JSON.stringify(R.pinned));
    R.innerKept = await S.evaluate(({ cw, keys }) => { const w = [...document.querySelectorAll('.tgroup[data-e]')].find((g) => g.dataset.e === cw), t = w.querySelector(":scope > .tsum"), closed = t.getAttribute("aria-expanded") === "false"; t.click();
      return { closed, open: keys.filter((k) => w.querySelector('.step[data-e="' + k + '"] > button')?.getAttribute("aria-expanded") === "true").length, of: keys.length }; }, inner0);
    r.expect(R.innerKept.closed && R.innerKept.open === R.innerKept.of, name + ": steps open inside a closed group didn't survive the redraw: " + JSON.stringify(R.innerKept));
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
    await S.waitForFunction(() => document.querySelector("dialog.panel.full")?.open === true);
    const sheet0 = await S.evaluate(() => document.querySelector("dialog.panel.full").textContent);
    t0 = Date.now();
    harbor.append(harbor.tool(at(12, 43), "toolu-live2", "Agent", { description: "Live reviewer", subagent_type: "general-purpose", prompt: BRIEF, run_in_background: true }),
      harbor.result(at(12, 43, 0, 500), "toolu-live2", "Async agent launched successfully.", { toolUseResult: { status: "async_launched", agentId: "live-sub" } }));
    const sub = L.lane("live-sub", { parent: "harbor" });
    fs.mkdirSync(path.dirname(sub.path), { recursive: true });
    fs.writeFileSync(sub.path.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Live reviewer", toolUseId: "toolu-live2" }));
    sub.append(sub.prompt(at(12, 43, 1), BRIEF), sub.text(at(12, 43, 20), "Live check: the flush change looks right."));
    await AN.evaluate(() => { document.querySelector(".analytics-metrics").dataset.liveProbe = "before-subagent"; });
    R.subagentUpdate = await appear(AN, t0, () => document.querySelector(".analytics-metrics")?.dataset.liveProbe !== "before-subagent");
    r.expect(R.subagentUpdate != null, name + ": Analytics didn't redraw after the subagent's lines were written");
    R.analyticsAfterSubagent = await analyticsState();
    r.expect(R.analyticsAfterSubagent.range === "30 d" && R.analyticsAfterSubagent.metrics === 8, name + ": Analytics changed after the subagent update: " + JSON.stringify(R.analyticsAfterSubagent));
    // The server answers from its last built model and rebuilds in the background, so the redraw above can come from a
    // model built before the subagent's files, and a model read at once can be a rebuild behind them. The spawn is in the
    // served model within the server's bound (a rebuild at most a second after the last, one 250 ms check, and the
    // build), taken here as 2.5 s from the lines; the relay and the question below are held to the 4 s rule.
    const spawn = await (async () => { while (true) { const h = (await model(srv)).handoffs.find((x) => x.kind === "spawn" && x.to === "live-sub"); if (h || Date.now() - t0 > 4000) return h ?? null; await sleep(50); } })();
    R.spawnMs = spawn ? Date.now() - t0 : null;
    r.expect(!!spawn, name + ": the model has no spawn to live-sub within 4 s");
    r.expect(R.spawnMs != null && R.spawnMs <= 2500, name + ": the spawn reached the served model after " + R.spawnMs + " ms, over the server's bound (2.5 s)");
    await sleep(Math.max(0, 4500 - (Date.now() - t0)));
    R.underSheet = await S.evaluate((id) => ({ open: document.querySelector("dialog.panel.full")?.open === true, text: document.querySelector("dialog.panel.full")?.textContent, card: !!document.querySelector('.child-card[data-h="' + id + '"]') }), spawn?.id ?? "");
    r.expect(R.underSheet.open && R.underSheet.text === sheet0, name + ": the View all sheet changed while open");
    r.expect(!R.underSheet.card, name + ": the page under the View all sheet was redrawn while it was open");
    delete R.underSheet.text;
    t0 = Date.now(); await S.click("dialog.panel.full .panel-h .ibtn");
    R.afterSheet = await appear(S, t0, (id) => !!document.querySelector('.child-card[data-h="' + id + '"]'), spawn?.id ?? "", 2000);
    r.expect(R.afterSheet != null, name + ": the subagent's card didn't appear once the sheet closed");
    const open3 = new Set(await openKeys(S));
    R.stillOpen3 = open0.filter((k) => k !== live0 && !open3.has(k));
    r.expect(R.stillOpen3.length === 0, name + ": closed by the updates: " + R.stillOpen3.join(", "));

    // ---- 3b. a child card above the view changes: the card shows it, the view stays, no transcript is asked for ----
    // The first block in view is the second child card (h-review's), its top 10 px under the bar; the Codex run above it then
    // makes a call, so its card (steps, current call) changes above the view. Plenty of the turn is below it.
    await S.setViewportSize({ width: opts.size === "phone" ? 390 : 1280, height: 480 }); await sleep(200);
    const cardsText = () => S.evaluate(() => [...document.querySelectorAll("#page .turns .child-card")].map((x) => x.textContent).join("|"));
    const above = await S.evaluate(() => { const cards = [...document.querySelectorAll("#page .turns .child-card")], n = cards[1];
      const s = window.__sc(); s.scrollTop += n.getBoundingClientRect().top - window.__line() + 10;
      return { e: n.dataset.e, card: n.matches(".child-card"), off: n.getBoundingClientRect().top - window.__line(), left: window.__left(), cards: cards.length }; });
    r.expect(above.card && above.cards >= 2 && Math.abs(above.off + 10) < 1.5 && above.left > 80, name + ": couldn't put the second child card at the top, away from the end: " + JSON.stringify(above));
    await sleep(200); const aboveTop = await topOf(S, byKey(above.e)); txs = S.txs.length; const cards0 = await cardsText();
    const rollout = path.join(dir, "codex/sessions/2026/09/28", fs.readdirSync(path.join(dir, "codex/sessions/2026/09/28")).find((f) => f.endsWith("-h-codex.jsonl")));
    t0 = Date.now();
    fs.appendFileSync(rollout, JSON.stringify({ timestamp: iso(at(12, 43, 30)), type: "response_item", payload: { type: "function_call", name: "shell", arguments: JSON.stringify({ command: ["echo", "child-live"] }), call_id: "call-child-live" } }) + "\n");
    R.childNote = await appear(S, t0, (was) => [...document.querySelectorAll("#page .turns .child-card")].map((x) => x.textContent).join("|") !== was, cards0);
    r.expect(R.childNote != null, name + ": the child card didn't change after its run made a call, within 4 s");
    await sleep(500);
    R.aboveMoved = (await topOf(S, byKey(above.e))) - aboveTop; R.childTx = S.txs.length - txs;
    r.expect(Math.abs(R.aboveMoved) <= 1, name + ": harbor's view moved " + R.aboveMoved + " px when a child card above it changed");
    r.expect(R.childTx === 0, name + ": an update to one child asked for " + R.childTx + " transcripts: " + S.txs.slice(txs).join(" "));
    await S.setViewportSize(opts.size === "phone" ? { width: 390, height: 844 } : { width: 1280, height: 860 }); await sleep(200);

    // ---- 4. a relay, with harbor's find mode open and its "Messages" chip chosen ----
    await S.click("#find-btn"); await S.waitForSelector("#find");
    await S.click('.chip[data-filter="messages"]'); await S.waitForFunction(() => document.querySelector('.chip[data-filter="messages"]')?.getAttribute("aria-pressed") === "true");
    await AN.locator('.analytics-range button:has-text("30 d")').focus();
    let u = S.updates; t0 = Date.now();
    sentinel.append(sentinel.tool(at(12, 44), "toolu-live3", "SendMessage", { to: "Principal", message: RELAY }),
      sentinel.result(at(12, 44, 0, 200), "toolu-live3", "Message sent to Principal", { toolUseResult: { success: true, msg_id: "m-live" } }));
    principal.append(principal.peer(at(12, 44, 1), 102, "Sentinel", "m-live", RELAY));
    const relay = await (async () => { for (let k = 0; k < 40; k++) { const h = (await model(srv)).handoffs.find((x) => x.kind === "relay" && x.brief === RELAY); if (h) return h; await sleep(100); } return null; })();
    r.expect(!!relay, name + ": the model has no relay from Sentinel");
    await AN.evaluate(() => { document.querySelector(".analytics-metrics").dataset.liveProbe = "before-relay"; });
    R.relayUpdate = await appear(AN, t0, () => document.querySelector(".analytics-metrics")?.dataset.liveProbe !== "before-relay");
    r.expect(R.relayUpdate != null, name + ": Analytics didn't redraw after the relay appeared in the served model");
    await sleep(300);
    R.analyticsAfterRelay = { ...(await analyticsState()), focus: await AN.evaluate(() => document.activeElement?.textContent.trim()) };
    r.expect(R.analyticsAfterRelay.range === "30 d" && R.analyticsAfterRelay.metrics === 8, name + ": Analytics changed after the relay update: " + JSON.stringify(R.analyticsAfterRelay));
    r.expect(R.analyticsAfterRelay.focus === "30 d", name + ": Analytics range focus did not survive redraw: " + JSON.stringify(R.analyticsAfterRelay));
    r.expect(await updated(S, u), name + ": harbor's page got no update for the relay");
    R.homeTrail.push(await homeState());
    R.filters = await S.evaluate(() => ({ find: !!document.querySelector("#find"), messages: document.querySelector('.chip[data-filter="messages"]')?.getAttribute("aria-pressed"), all: document.querySelector('.chip[data-filter="all"]')?.getAttribute("aria-pressed") }));
    r.expect(R.filters.find && R.filters.messages === "true" && R.filters.all === "false", name + ": find mode or its chip changed: " + JSON.stringify(R.filters));
    await S.click('button[aria-label="Close find"]');
    R.analytics = await analyticsState();
    r.expect(R.analytics.range === "30 d" && R.analytics.metrics === 8 && R.analytics.charts === 2,
      name + ": Analytics range, figures or charts changed during a live update: " + JSON.stringify({ before: analytics0, after: R.analytics }));

    // ---- 5. a question for you, and one more message while find is open; the drawer open on the phone ----
    await S.click("#find-btn");
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

    // ---- 6. Analytics continues to redraw from served busy intervals and turns ----
    u = AN.updates; await AN.evaluate(() => { document.querySelector(".analytics-metrics").dataset.liveProbe = "before-activity"; });
    principal.append(principal.filler(at(13, 30)));
    r.expect(await updated(AN, u), name + ": Analytics got no update for new activity");
    R.activity = await analyticsState();
    r.expect(R.activity.range === "30 d" && R.activity.metrics === 8, name + ": Analytics did not retain its range and figures after activity: " + JSON.stringify(R.activity));
    u = AN.updates;
    principal.append(principal.filler(at(13, 40)));
    r.expect(await updated(AN, u), name + ": Analytics got no update for more activity");
    R.moreActivity = await analyticsState();
    r.expect(R.moreActivity.range === "30 d" && R.moreActivity.metrics === 8, name + ": Analytics changed after more activity: " + JSON.stringify(R.moreActivity));

    // ---- 6b. the filters and the search field are kept through an update that adds a repo ----
    // The same elements stay in the document (tagged before the update), the focused one keeps focus (the search field its
    // text and caret), an open list stays open, the new repo is an option, and the selection is unchanged. A selected repo that
    // leaves the data stays, marked "(no sessions)".
    const ROOT = '.facet-filters .sh-select[data-label="Repo"]';
    const repoLog = (repo, sid, t) => {
      const cwd = path.join(dir, "work", repo), file = path.join(dir, "claude/projects", cwd.replace(/[^A-Za-z0-9]/g, "-"), sid + ".jsonl");
      fs.mkdirSync(path.join(cwd, ".git"), { recursive: true }); fs.mkdirSync(path.dirname(file), { recursive: true });
      const line = (at, type, extra) => JSON.stringify({ parentUuid: null, isSidechain: false, type, timestamp: iso(at), sessionId: sid, cwd, gitBranch: "main", version: "2.1.0", uuid: "u-" + sid + "-" + at, ...extra }) + "\n";
      fs.writeFileSync(file, line(t, "user", { message: { role: "user", content: "Live check: a session in " + repo } }) + line(t + 1000, "assistant", { message: { id: "msg-" + sid, model: "claude-opus-5-5", role: "assistant", type: "message", content: [{ type: "text", text: "Live check: hello from " + repo }] } }));
      return file;
    };
    const hasRepo = (page, repo) => appear(page, Date.now(), ([root, name]) => document.querySelector(root)?.semonSelect.options.some((o) => o.value.endsWith(name)), [ROOT, repo]);
    const facetState = (page) => page.evaluate((root) => { const box = document.querySelector(root), api = box.semonSelect, t = box.querySelector(".sh-select-trigger"), bar = document.querySelector(".facet-filters");
      return { kept: t.__kept === true, barKept: bar.__kept === true, focus: document.activeElement === t, open: api.isOpen, expanded: t.getAttribute("aria-expanded"), value: api.value, text: t.textContent.trim(), labels: api.options.map((o) => o.label), values: api.options.map((o) => o.value),
        listed: [...box.querySelectorAll('[role="option"]')].filter((o) => o.getClientRects().length).map((o) => o.dataset.value), active: box.querySelector('[role="option"].sh-active')?.dataset.value ?? null }; }, ROOT);
    const pick = async (page, value) => { await page.click(ROOT + " .sh-select-trigger"); await page.locator(ROOT + ' [role="option"][data-value="' + value + '"]').click(); await page.waitForFunction((root) => document.querySelector(root + " .sh-select-trigger").getAttribute("aria-expanded") === "false", ROOT); await sleep(300); };
    const tag = (page) => page.evaluate((root) => { document.querySelector(".facet-filters").__kept = true; const t = document.querySelector(root + " .sh-select-trigger"); t.__kept = true; t.focus(); }, ROOT);
    // The Sessions search: same input, still focused, its text and its caret.
    await SP.evaluate(() => { const f = document.querySelector("#sq"); f.__kept = true; f.focus(); f.setSelectionRange(3, 3); });
    let uSp = SP.updates; repoLog("nova", "live-nova", at(12, 44, 10));
    r.expect(await hasRepo(SP, "nova") != null, name + ": the Sessions page has no Nova repo option after a new session");
    await sleep(700);
    R.searchKept = await SP.evaluate(() => { const f = document.querySelector("#sq"); return { kept: f?.__kept === true, focus: document.activeElement === f, value: f?.value, caret: f?.selectionStart }; });
    r.expect(SP.updates > uSp && R.searchKept.kept && R.searchKept.focus && R.searchKept.value === "harbor" && R.searchKept.caret === 3, name + ": the Sessions search field was replaced or lost its focus, text or caret: " + JSON.stringify(R.searchKept));
    // The filters, on both pages: pick the first repo, focus the button, then add another repo.
    const first = await AN.evaluate((root) => document.querySelector(root).semonSelect.options.map((o) => o.value).find((v) => v && v !== "__none__"), ROOT);
    R.facets = {};
    // The Sessions list follows a filter choice: with the search cleared, choosing the repo leaves only its sessions.
    await SP.fill("#sq", ""); await sleep(200);
    const rowsOf = () => SP.evaluate((repo) => ({ n: document.querySelectorAll("#page .nrow").length, other: [...document.querySelectorAll("#page .nrow .for")].filter((f) => !f.textContent.includes(repo)).length }), first);
    const rowsAll = (await rowsOf()).n;
    for (const [key, page] of [["analytics", AN], ["sessions", SP]]) { await pick(page, first); await tag(page); R.facets[key] = { before: await facetState(page) }; }
    R.sessionsList = { all: rowsAll, filtered: await rowsOf() };
    r.expect(R.sessionsList.filtered.n > 0 && R.sessionsList.filtered.n < rowsAll && R.sessionsList.filtered.other === 0, name + ": the Sessions list didn't follow the repo filter: " + JSON.stringify(R.sessionsList));
    // The other persistent controls are tagged too: the range in Analytics' bar, its breakdown measure toggle, the Sessions group-by.
    await AN.evaluate(() => { document.querySelector("#topbar .analytics-range").__kept = true; document.querySelector(".analytics-bd-head").__kept = true; });
    await SP.evaluate(() => { document.querySelector(".groupby").__kept = true; });
    const uFacets = [AN.updates, SP.updates]; repoLog("orbit", "live-orbit", at(12, 44, 20));
    for (const [key, page] of [["analytics", AN], ["sessions", SP]]) {
      r.expect(await hasRepo(page, "orbit") != null, name + ": " + key + ": the new repo isn't an option within 4 s");
      await sleep(700); R.facets[key].after = await facetState(page);
      const { before, after } = R.facets[key];
      r.expect(before.kept && before.focus && after.kept && after.barKept && after.focus && after.value === first && before.value === first && after.text === "Repo: " + first, name + ": " + key + ": the filters were replaced, lost focus or changed their selection: " + JSON.stringify(R.facets[key]));
      r.expect(after.values.some((v) => v.endsWith("orbit")) && !after.labels.some((l) => l.includes("(no sessions)")), name + ": " + key + ": the options after the update: " + JSON.stringify(after.labels));
    }
    r.expect(AN.updates > uFacets[0] && SP.updates > uFacets[1], name + ": a page got no update for the new repo");
    R.keptControls = { range: await AN.evaluate(() => document.querySelector("#topbar .analytics-range")?.__kept === true), measure: await AN.evaluate(() => document.querySelector(".analytics-bd-head")?.__kept === true), groupby: await SP.evaluate(() => document.querySelector(".groupby")?.__kept === true) };
    r.expect(R.keptControls.range && R.keptControls.measure && R.keptControls.groupby, name + ": the range, measure or group-by control was rebuilt by an update: " + JSON.stringify(R.keptControls));
    // An open list stays open, with its highlight, while an update adds a repo to it; Escape then closes it and returns focus.
    await AN.click(ROOT + " .sh-select-trigger"); await AN.waitForFunction((root) => document.querySelector(root).semonSelect.isOpen, ROOT);
    R.open = { before: await facetState(AN) }; uFacets[0] = AN.updates; repoLog("vega", "live-vega", at(12, 44, 30));
    r.expect(await hasRepo(AN, "vega") != null, name + ": Analytics: the open list has no Vega option within 4 s");
    await sleep(700); R.open.after = await facetState(AN);
    r.expect(AN.updates > uFacets[0] && R.open.after.open && R.open.after.expanded === "true" && R.open.after.kept && R.open.after.active === R.open.before.active && R.open.after.listed.some((v) => v.endsWith("vega")) && R.open.after.value === first, name + ": the open list closed, lost its highlight or lacks the new repo: " + JSON.stringify(R.open));
    await AN.keyboard.press("Escape"); await sleep(200); R.open.closed = await facetState(AN);
    r.expect(!R.open.closed.open && R.open.closed.focus && R.open.closed.kept, name + ": Escape didn't close the list and return focus: " + JSON.stringify(R.open.closed));
    // A selected repo that no session has now stays selected, marked.
    const orbit = R.facets.analytics.after.values.find((v) => v.endsWith("orbit"));
    await pick(AN, orbit); await tag(AN);
    fs.rmSync(path.join(dir, "claude/projects", path.join(dir, "work", "orbit").replace(/[^A-Za-z0-9]/g, "-")), { recursive: true, force: true });
    R.stale = await appear(AN, Date.now(), (root) => document.querySelector(root).semonSelect.options.some((o) => o.label.includes("(no sessions)")), ROOT);
    await sleep(300); R.facets.stale = await facetState(AN);
    r.expect(R.stale != null && R.facets.stale.kept && R.facets.stale.focus && R.facets.stale.value === orbit && R.facets.stale.labels.filter((l) => l.includes("(no sessions)")).length === 1, name + ": a selected repo that left the data isn't kept and marked: " + JSON.stringify(R.facets.stale));
    await pick(AN, ""); await pick(SP, "");
    R.sessionsList.reset = (await rowsOf()).n;
    r.expect(R.sessionsList.reset > R.sessionsList.filtered.n, name + ": the Sessions list didn't widen when the filter was cleared: " + JSON.stringify(R.sessionsList));

    // ---- 7. harbor's process dies mid-call: the step stops running, though no line is written ----
    await S.click('#topbar button[aria-label="Close find"]'); await S.waitForFunction(() => !document.querySelector("#find")); // find from step 5 would hide the call
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
    R.overflow = { harbor: await overflow(S), analytics: await overflow(AN), home: await overflow(Hm), sessions: await overflow(SP) };
    for (const [k, v] of Object.entries(R.overflow)) r.expect(v === 0, name + ": " + k + " overflow=" + v);
    await S.screenshot({ path: path.join(ENV.out, "live-" + name + "-harbor.png") });
    await AN.screenshot({ path: path.join(ENV.out, "live-" + name + "-analytics.png") });
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
    await AN.route("**/api/model**", (q) => q.fulfill({ status: 403, contentType: "text/plain", body: "Forbidden" }));
    t0 = Date.now();
    R.ended = await appear(AN, t0, (t) => document.querySelector(".livenote")?.textContent === t, ENDED, 5000);
    r.expect(R.ended != null, name + ": no \"" + ENDED + "\" note after a 403");
    let n = AN.models.length; await sleep(5000);
    R.pollsAfter403 = AN.models.length - n;
    r.expect(R.pollsAfter403 === 0, name + ": polled " + R.pollsAfter403 + " times after a 403");
    R.note = await AN.evaluate(() => { const b = document.querySelector(".livenote")?.getBoundingClientRect(); return b ? { left: b.left, right: b.right, bottom: b.bottom, vw: document.documentElement.clientWidth, vh: innerHeight } : null; });
    r.expect(!!R.note && R.note.left >= 0 && R.note.right <= R.note.vw + 0.5 && R.note.bottom <= R.note.vh, name + ": the note is off screen: " + JSON.stringify(R.note));
    R.overflow.ended = await overflow(AN);
    r.expect(R.overflow.ended === 0, name + ": overflow with the note=" + R.overflow.ended);
    await AN.screenshot({ path: path.join(ENV.out, "live-" + name + "-ended.png") });

    // The same ending on a session page scrolled up, where the jump button shows: the note sits above it, not under it.
    await S.evaluate(() => { window.__sc().scrollTop = 0; });
    await S.waitForFunction(() => { const b = document.querySelector(".jump-wrap"); return !!b && !b.hidden; }, null, { timeout: 5000 });
    await S.route("**/api/model**", (q) => q.fulfill({ status: 403, contentType: "text/plain", body: "Forbidden" }));
    R.endedOnSession = await appear(S, Date.now(), (t) => document.querySelector(".livenote")?.textContent === t, ENDED, 8000);
    r.expect(R.endedOnSession != null, name + ": no \"" + ENDED + "\" note on the session page after a 403");
    R.noteVsJump = await S.evaluate(() => { const n = document.querySelector(".livenote")?.getBoundingClientRect(), j = document.querySelector("#jump-bottom"), b = j?.getBoundingClientRect(); return n && b ? { jumpShown: !j.closest(".jump-wrap").hidden, noteTop: n.top, noteBottom: n.bottom, noteLeft: n.left, noteRight: n.right, jumpTop: b.top, jumpBottom: b.bottom, jumpLeft: b.left, jumpRight: b.right, overlaps: n.left < b.right && n.right > b.left && n.top < b.bottom && n.bottom > b.top } : null; });
    r.expect(!!R.noteVsJump && R.noteVsJump.jumpShown && !R.noteVsJump.overlaps, name + ": the ended note and the jump button overlap or the button was gone: " + JSON.stringify(R.noteVsJump));
    await S.screenshot({ path: path.join(ENV.out, "live-" + name + "-ended-session.png") });

    // One poll at a time everywhere.
    R.inFlight = {}; R.pollsInFlight = {}; for (const [k, p] of [["harbor", S], ["analytics", AN], ["home", Hm], ["sessions", SP]]) { R.inFlight[k] = await p.evaluate(() => window.__live.max); R.pollsInFlight[k] = await p.evaluate(() => window.__live.models); }
    for (const [k, v] of Object.entries(R.inFlight)) r.expect(v <= 1, name + ": " + k + " had " + v + " requests in flight at once");
    for (const [k, v] of Object.entries(R.pollsInFlight)) r.expect(v <= 1, name + ": " + k + " had " + v + " polls in flight at once");
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, name + ": page errors: " + R.errors.join(" | "));
  } finally {
    for (const p of pages) await p.context().close();
    srv.proc.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return R;
}

// Analytics reads its tool-call and error counts from the model, so it never asks /api/tx for a transcript. Here it opens
// with no /api/tx request at all, through a live update that adds a call; the model's per-session `calls` and `errors` equal
// what each session's /api/tx page reports; and the Tool calls figure is the sum of them for the sessions it counts.
async function analyticsCounts(browser, r) {
  const R = { name: "analytics-counts" }, dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-counts-")), now = write(dir);
  const srv = await serve(dir, now + 10 * 60000), L = logs(dir), atlas = L.lane("atlas"), pages = [];
  const scheme = { size: "desktop", dark: false };
  const tool = (page) => page.locator(".analytics-metric").filter({ hasText: "Tool calls" });
  const total = async (page) => Number((await tool(page).locator(".value").textContent()).replace(/,/g, ""));
  const tx = async (sid) => { const q = await fetch(srv.base + "/api/tx?sid=" + encodeURIComponent(sid) + "&t=" + srv.token); if (!q.ok) throw new Error("/api/tx " + sid + " " + q.status); return q.json(); };
  // What the counts must be, from the model and each session's own /api/tx page: the sessions started in the last 7 days.
  const expected = async () => {
    const m = await model(srv), rows = [];
    for (const [id, s] of Object.entries(m.sessions)) { if (s.stub) continue; const p = await tx(id); rows.push({ id, start: s.start, model: { calls: s.calls, errors: s.errors }, page: { calls: p.calls, errors: p.errors } }); }
    return { rows, sum: rows.filter((x) => x.start >= m.now - 7 * 86400000 && x.start < m.now).reduce((n, x) => n + x.page.calls, 0) };
  };
  try {
    const page = await open(browser, srv, "/analytics", scheme); pages.push(page);
    await page.waitForFunction(() => document.querySelectorAll(".analytics-metric").length === 8);
    R.metrics = 8; R.first = await total(page);
    R.want = await expected();
    r.expect(R.want.rows.length > 0 && R.want.rows.every((x) => Number.isInteger(x.model.calls) && Number.isInteger(x.model.errors)), "analytics-counts: a session in the model has no counts: " + JSON.stringify(R.want.rows));
    r.expect(R.want.rows.every((x) => x.model.calls === x.page.calls && x.model.errors === x.page.errors), "analytics-counts: the model's counts differ from /api/tx: " + JSON.stringify(R.want.rows.filter((x) => x.model.calls !== x.page.calls || x.model.errors !== x.page.errors)));
    r.expect(R.first > 0 && R.first === R.want.sum, "analytics-counts: the Tool calls figure is " + R.first + ", not the sessions' " + R.want.sum);
    // A later update: atlas makes one more tool call. The update applies and the figure follows, with still no /api/tx request.
    await page.evaluate(() => { document.querySelector(".analytics-metrics").dataset.liveProbe = "before"; });
    const updates = page.updates; let t0 = Date.now();
    atlas.append(atlas.tool(at(12, 42), "toolu-counts1", "Bash", { command: "true" }), atlas.result(at(12, 42, 5), "toolu-counts1", "ok"));
    R.redrawn = await appear(page, t0, () => document.querySelector(".analytics-metrics")?.dataset.liveProbe !== "before", null, 6000);
    r.expect(R.redrawn != null && page.updates > updates, "analytics-counts: a live update did not redraw Analytics");
    R.grew = await appear(page, t0, (was) => { const v = document.querySelector(".analytics-metric:nth-child(5) .value")?.textContent; return Number((v ?? "").replace(/,/g, "")) === was + 1; }, R.first, 8000);
    r.expect(R.grew != null, "analytics-counts: the new tool call never reached the Tool calls figure: " + await tool(page).locator(".value").textContent());
    R.later = await expected();
    r.expect(await total(page) === R.later.sum, "analytics-counts: after the update the figure is " + await total(page) + ", not the sessions' " + R.later.sum);
    R.txRequests = page.txs.slice();
    r.expect(R.txRequests.length === 0, "analytics-counts: Analytics asked for /api/tx: " + R.txRequests.join(" "));
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "analytics-counts: page errors: " + R.errors.join(" | "));
  } finally {
    for (const p of pages) await p.context().close();
    srv.proc.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return R;
}

// A transcript loaded only part of the way (a deep link to a middle turn) isn't tailed, so the totals it was fetched with go
// stale. The meta line's failed-step count follows the model there: it is the model's before, and grows with the session.
async function middleCounts(browser, r) {
  const R = { name: "middle-counts" }, dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-middle-")), now = write(dir, { extras: true });
  const srv = await serve(dir, now + 10 * 60000), L = logs(dir), backlog = L.lane("backlog"), pages = [];
  const count = (page) => page.evaluate(() => { const n = [...document.querySelectorAll("#topbar .meta-line .lab")].map((x) => /^(\d[\d,]*) failed$/.exec(x.textContent)).find(Boolean); return n ? Number(n[1].replace(/,/g, "")) : 0; });
  try {
    const m0 = await model(srv), turn = m0.turns.filter((t) => t.sid === "backlog")[1];
    r.expect(!!turn && m0.sessions.backlog.calls > 200, "middle-counts: the backlog fixture is not long enough: " + JSON.stringify({ turn: turn?.id, calls: m0.sessions.backlog?.calls }));
    const failedNow = m0.sessions.backlog.errors ?? 0;
    const page = await open(browser, srv, "/s/claude/backlog?turn=" + encodeURIComponent(turn.id), { size: "desktop", dark: false }); pages.push(page);
    await page.waitForFunction((id) => !!document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]'), turn.id);
    R.later = await page.evaluate(() => [...document.querySelectorAll("#page button.more")].some((b) => b.textContent === "Load later"));
    r.expect(R.later, "middle-counts: the deep link did not stop short of the end (no Load later)");
    R.before = await count(page);
    r.expect(R.before === failedNow, "middle-counts: the meta line shows " + R.before + " failed steps, the model " + failedNow);
    const t0 = Date.now();
    for (let i = 0; i < 3; i++) backlog.append(backlog.tool(at(12, 43 + i), "toolu-mid" + i, "Bash", { command: "true" }), { ...backlog.result(at(12, 43 + i, 5), "toolu-mid" + i, "boom"), message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu-mid" + i, content: "boom", is_error: true }] } });
    R.grew = await appear(page, t0, (want) => { const n = [...document.querySelectorAll("#topbar .meta-line .lab")].map((x) => /^(\d[\d,]*) failed$/.exec(x.textContent)).find(Boolean); return n && Number(n[1].replace(/,/g, "")) === want; }, R.before + 3, 8000);
    R.after = await count(page);
    r.expect(R.grew != null, "middle-counts: the meta line stayed at " + R.after + " failed steps after the session failed 3 more calls (model " + (await model(srv)).sessions.backlog.errors + ")");
    R.stillMiddle = await page.evaluate(() => [...document.querySelectorAll("#page button.more")].some((b) => b.textContent === "Load later"));
    r.expect(R.stillMiddle, "middle-counts: the page reached the end, so it isn't a middle page any more");
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "middle-counts: page errors: " + R.errors.join(" | "));
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
  try { out.analyticsCounts = await analyticsCounts(browser, r); }
  catch (e) { r.expect(false, "analytics-counts: threw " + (e?.stack ?? e)); }
  try { out.middleCounts = await middleCounts(browser, r); }
  catch (e) { r.expect(false, "middle-counts: threw " + (e?.stack ?? e)); }
  r.results = out;
  return r.done();
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  let ok = false;
  try { ok = await liveCheck(browser); } finally { await browser.close(); }
  if (!ok) process.exit(1);
}

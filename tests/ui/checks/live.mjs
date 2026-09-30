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
//   - a running subagent's page shows its brief once, in the intro block, on opening and after a live update redraws its turn;
//   - a running subagent's page ends in a status line that follows it: "Working" with a tool-call count and a cost that advance,
//     then "Returned to <parent> · Done" when it finishes, without a reload; each item (calls, time, cost) has a plain-text
//     breakdown tip that follows the numbers, and a tap on it shows the tip on a phone; and when its origin only appears after
//     the page is open, the intro block is added (once) and the brief is in it alone; the focus stays on the same kind of footer
//     item (cost, "Open in") while the items change around it;
//   - a page whose origin arrived late retries a failed reload of its transcript on the next poll (which asks for the whole
//     model), keeps polling when hidden and shown in between, and gives up after 4 tries spaced 4 s, 8 s, 16 s;
//   - any other session's page ends in the same footer ("Working · N tool calls · …" that follows its calls, with no "Still
//     working" line under its last turn; "Done · N tool calls · …" when it has stopped);
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
import { ENV, launch, context, settled, goto, reporter, overflow, filterSheet, doneFilterSheet, pickFilter } from "../lib.mjs";
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
      // One message's token use, as the model prices it by message id: `input` and `cacheWrite` (5 min) tokens and so on.
      usage: (t, { input = 0, output = 0, cacheRead = 0, cacheWrite = 0 }) => base(t, "assistant", { message: { id: "msg-live-" + sid + "-" + seq, model: "claude-opus-5-5", role: "assistant", type: "message", content: [],
        usage: { input_tokens: input, output_tokens: output, cache_read_input_tokens: cacheRead, cache_creation_input_tokens: cacheWrite, cache_creation: { ephemeral_5m_input_tokens: cacheWrite, ephemeral_1h_input_tokens: 0 } } } }),
      fail: (t, id, content) => base(t, "user", { message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content, is_error: true }] } }),
      // What Claude Code writes into the parent when a background subagent finishes.
      notify: (t, id, status, body) => base(t, "user", { origin: { kind: "task-notification" }, message: { role: "user", content: "<task-notification><task-id>x</task-id><tool-use-id>" + id + "</tool-use-id><status>" + status + "</status><result>" + body + "</result></task-notification>" } }),
      peer: (t, from, name, msg, body) => base(t, "user", { origin: { kind: "peer", from: "uds:/run/user/1000/cc-socks/" + from + ".sock", name, msg_id: msg, body }, message: { role: "user", content: body } }),
    };
  };
  return { lane, dirOf: (sid) => path.dirname(file(sid)), cwdOf: (sid) => cwdOf(file(sid)) };
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
  const t = n.matches(".step") ? n.querySelector(":scope > button") : n.matches(".tgroup") ? n.querySelector(":scope > .tsum") : n.matches(".child-work") ? n.querySelector(":scope > .cw-toggle") : null;
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
    const AN = await open(browser, srv, "/analytics", opts); pages.push(AN);
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
    const mainBox = await S.locator("#main").boundingBox();
    await S.mouse.move(mainBox ? mainBox.x + Math.min(mainBox.width / 2, 200) : 195, mainBox ? mainBox.y + Math.min(mainBox.height / 2, 200) : 300);
    await S.mouse.wheel(0, -1000); // Reader input yields the temporary open-at-end pin before we place a precise anchor.
    R.room = await S.evaluate(() => { const s = window.__sc(); s.scrollTop = Math.round((s.scrollHeight - s.clientHeight) * 0.4); return window.__left(); });
    r.expect(R.room > 200, name + ": harbor isn't long enough to scroll up from its end: " + R.room);
    await sleep(150);
    const anchor = await S.evaluate(() => { const line = window.__line(); const n = [...document.querySelectorAll("#page .turns :is(.msg, .step, .hcard, .divider, .thought)[data-e]")].find((x) => { const b = x.getBoundingClientRect(); return b.height && b.top >= line; }); return n ? { e: n.dataset.e, top: n.getBoundingClientRect().top } : null; });
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
    const beforeEntries = await S.evaluate(() => [...document.querySelectorAll("#page .turns :is(.msg, .step, .hcard, .thought, .think-pending)[data-e]")].filter((n) => !n.closest(".cw-body")).map((n) => n.dataset.e));
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
    R.jump = await S.evaluate((before) => { const p = document.querySelector(".jump-bottom"); if (!p || p.hidden) return null; const b = p.getBoundingClientRect(), keys = new Set([...document.querySelectorAll("#page .turns :is(.msg, .step, .hcard, .thought, .think-pending)[data-e]")].filter((n) => !n.closest(".cw-body")).map((n) => n.dataset.e)); return { text: p.textContent.trim(), added: [...keys].filter((key) => !before.includes(key)).length, w: b.width, h: b.height, top: b.top, left: b.left, right: b.right, bottom: b.bottom, vw: document.documentElement.clientWidth, vh: innerHeight }; }, beforeEntries);
    // The new thought is an entry like any other: it counts toward "N new" (the message and the thought make at least two).
    R.thought = await S.evaluate((t) => { const n = [...document.querySelectorAll("#page .turns .thought[data-e]")].find((x) => x.querySelector(".think-text")?.textContent.includes(t)); return n ? { key: n.dataset.e, open: !n.querySelector("button, [hidden]") } : null; }, THOUGHT);
    r.expect(!!R.thought && R.thought.open && !beforeEntries.includes(R.thought.key), name + ": harbor's new thought did not appear in full, or was already counted: " + JSON.stringify(R.thought));
    r.expect(R.jump && R.jump.added >= 2, name + ": the new thought and message should both count as new: " + JSON.stringify(R.jump));
    const jumpCount = R.jump && /^([1-9]\d*) new$/.exec(R.jump.text);
    r.expect(!!jumpCount && Number(jumpCount[1]) === R.jump.added, name + ": jump button count didn't match newly rendered entries: " + JSON.stringify(R.jump));
    if (R.jump) r.expect(R.jump.w >= 40 && R.jump.h >= 40 && R.jump.top >= 0 && R.jump.left >= 0 && R.jump.right <= R.jump.vw + 0.5 && R.jump.bottom <= R.jump.vh, name + ": the jump button is off screen or under 40 px: " + JSON.stringify(R.jump));
    await S.screenshot({ path: path.join(ENV.out, "live-" + name + "-jump.png") });
    await S.click(".jump-bottom"); await S.waitForFunction(() => window.__left() <= 1, null, { timeout: 5000 });
    R.afterJump = await S.evaluate(() => ({ hidden: document.querySelector(".jump-bottom")?.hidden, left: window.__left() }));
    r.expect(R.afterJump.hidden && R.afterJump.left <= 1, name + ": tapping the jump button didn't go to the end: " + JSON.stringify(R.afterJump));

    // ---- 2. a new call while the reader is at the end: pinned, ticking, then finished ----
    // Child work closed with steps open inside it keeps them through a redraw of its turn.
    const inner0 = await S.evaluate(() => { const w = document.querySelector('.child-work[data-e] > .cw-toggle[aria-expanded="true"]').parentElement;
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
    R.innerKept = await S.evaluate(({ cw, keys }) => { const w = document.querySelector('.child-work[data-e="' + cw + '"]'), t = w.querySelector(":scope > .cw-toggle"), closed = t.getAttribute("aria-expanded") === "false"; t.click();
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
    R.underSheet = await S.evaluate((id) => ({ open: document.querySelector("dialog.viewer")?.open === true, text: document.querySelector("dialog.viewer")?.textContent, card: !!document.querySelector('.hcard[data-h="' + id + '"]') }), spawn?.id ?? "");
    r.expect(R.underSheet.open && R.underSheet.text === sheet0, name + ": the View all sheet changed while open");
    r.expect(!R.underSheet.card, name + ": the page under the View all sheet was redrawn while it was open");
    delete R.underSheet.text;
    t0 = Date.now(); await S.click(".viewer .vclose");
    R.afterSheet = await appear(S, t0, (id) => { const c = document.querySelector('.hcard[data-h="' + id + '"]'); return !!c?.querySelector(".child-work"); }, spawn?.id ?? "", 2000);
    r.expect(R.afterSheet != null, name + ": the subagent's card and child work didn't appear once the sheet closed");
    const open3 = new Set(await openKeys(S));
    R.stillOpen3 = open0.filter((k) => k !== live0 && !open3.has(k));
    r.expect(R.stillOpen3.length === 0, name + ": closed by the updates: " + R.stillOpen3.join(", "));

    // ---- 3b. open child work grows above the view: the new message shows in it, the view stays, one transcript is asked for ----
    // The first block in view is the card after h-codex's open child work (h-review's), its top 10 px under the bar; the
    // Codex run then says something, so its child work grows above the view. Plenty of the turn is below it.
    await S.setViewportSize({ width: opts.size === "phone" ? 390 : 1280, height: 480 }); await sleep(200);
    const above = await S.evaluate(() => { const cards = [...document.querySelectorAll("#page .turns .hcard.child-card")].filter((x) => !x.closest(".cw-body")), cws = cards.map((x) => x.querySelector(":scope .child-work")).filter(Boolean), n = cards[1];
      const s = window.__sc(); s.scrollTop += n.getBoundingClientRect().top - window.__line() + 10;
      return { e: n.dataset.e, card: n.matches(".hcard"), off: n.getBoundingClientRect().top - window.__line(), left: window.__left(), cw: cws[0].querySelector(":scope > .cw-toggle").getAttribute("aria-expanded") === "true" }; });
    r.expect(above.card && Math.abs(above.off + 10) < 1.5 && above.left > 80 && above.cw, name + ": couldn't put the card under open child work at the top, away from the end: " + JSON.stringify(above));
    await sleep(200); const aboveTop = await topOf(S, byKey(above.e)); txs = S.txs.length;
    const rollout = path.join(dir, "codex/sessions/2026/09/28", fs.readdirSync(path.join(dir, "codex/sessions/2026/09/28")).find((f) => f.endsWith("-h-codex.jsonl")));
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
    R.filters = await S.evaluate((phone) => ({ pop: document.querySelector(".filters.pop")?.hidden === false, expanded: document.querySelector(phone ? "#more-btn" : "#filter-btn")?.getAttribute("aria-expanded"), thinking: document.querySelector("#f-thinking")?.checked, tools: document.querySelector("#f-tools")?.checked }), phone);
    r.expect(R.filters.pop && R.filters.expanded === (phone ? "false" : "true") && R.filters.thinking === false && R.filters.tools === true, name + ": the filters changed: " + JSON.stringify(R.filters));
    await S.keyboard.press("Escape");
    R.analytics = await analyticsState();
    r.expect(R.analytics.range === "30 d" && R.analytics.metrics === 8 && R.analytics.charts === 2,
      name + ": Analytics range, figures or charts changed during a live update: " + JSON.stringify({ before: analytics0, after: R.analytics }));

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
    // The filters are one Filter button and chips, with the four Selects in the sheet the button opens. The same elements stay in the
    // document (tagged before the update), the focused one keeps focus (the search field its text and caret), the new repo is an
    // option, the chip and the selection are unchanged. While the sheet is open with a list open, an update draws nothing under
    // it: the list stays open with its highlight, and the update lands when the sheet closes. A selected repo that leaves the data
    // stays, marked "(no sessions)".
    const ROOT = '.facet-filters .sh-select[data-label="Repo"]';
    const repoLog = (repo, sid, t) => {
      const cwd = path.join(dir, "work", repo), file = path.join(dir, "claude/projects", cwd.replace(/[^A-Za-z0-9]/g, "-"), sid + ".jsonl");
      fs.mkdirSync(path.join(cwd, ".git"), { recursive: true }); fs.mkdirSync(path.dirname(file), { recursive: true });
      const line = (at, type, extra) => JSON.stringify({ parentUuid: null, isSidechain: false, type, timestamp: iso(at), sessionId: sid, cwd, gitBranch: "main", version: "2.1.0", uuid: "u-" + sid + "-" + at, ...extra }) + "\n";
      fs.writeFileSync(file, line(t, "user", { message: { role: "user", content: "Live check: a session in " + repo } }) + line(t + 1000, "assistant", { message: { id: "msg-" + sid, model: "claude-opus-5-5", role: "assistant", type: "message", content: [{ type: "text", text: "Live check: hello from " + repo }] } }));
      return file;
    };
    const hasRepo = (page, repo) => appear(page, Date.now(), ([root, name]) => document.querySelector(root)?.semonSelect.options.some((o) => o.value.endsWith(name)), [ROOT, repo]);
    const facetState = (page) => page.evaluate((root) => { const box = document.querySelector(root), api = box.semonSelect, t = box.querySelector(".sh-select-trigger"), bar = document.querySelector(".facet-filters"), chip = bar.querySelector('.facet-chip[data-facet="repo"]');
      return { kept: t.__kept === true, barKept: bar.__kept === true, focus: document.activeElement === bar.querySelector(".facet-btn"), triggerFocus: document.activeElement === t, chip: chip && chip.getClientRects().length ? chip.textContent.trim() : null, open: api.isOpen, expanded: t.getAttribute("aria-expanded"), value: api.value, text: t.textContent.trim(), labels: api.options.map((o) => o.label), values: api.options.map((o) => o.value),
        listed: [...box.querySelectorAll('[role="option"]')].filter((o) => o.getClientRects().length).map((o) => o.dataset.value), active: box.querySelector('[role="option"].sh-active')?.dataset.value ?? null }; }, ROOT);
    const pick = async (page, value) => { await pickFilter(page, "Repo", value); await sleep(300); };
    const tag = (page) => page.evaluate((root) => { document.querySelector(".facet-filters").__kept = true; document.querySelector(root + " .sh-select-trigger").__kept = true; document.querySelector(".facet-filters .facet-btn").focus(); }, ROOT);
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
      r.expect(before.kept && before.focus && after.kept && after.barKept && after.focus && after.value === first && before.value === first && after.text === "Repo: " + first && after.chip === "Repo: " + first && before.chip === after.chip, name + ": " + key + ": the filters were replaced, lost focus or changed their selection or chip: " + JSON.stringify(R.facets[key]));
      r.expect(after.values.some((v) => v.endsWith("orbit")) && !after.labels.some((l) => l.includes("(no sessions)")), name + ": " + key + ": the options after the update: " + JSON.stringify(after.labels));
    }
    r.expect(AN.updates > uFacets[0] && SP.updates > uFacets[1], name + ": a page got no update for the new repo");
    R.keptControls = { range: await AN.evaluate(() => document.querySelector("#topbar .analytics-range")?.__kept === true), measure: await AN.evaluate(() => document.querySelector(".analytics-bd-head")?.__kept === true), groupby: await SP.evaluate(() => document.querySelector(".groupby")?.__kept === true) };
    r.expect(R.keptControls.range && R.keptControls.measure && R.keptControls.groupby, name + ": the range, measure or group-by control was rebuilt by an update: " + JSON.stringify(R.keptControls));
    // An open sheet with an open list is left as it is while an update adds a repo: the list stays open, with its highlight (the update
    // is drawn when the sheet closes); Escape closes the list and returns focus to its button, Escape again closes the sheet and returns
    // focus to the Filter button, and the new repo is an option then.
    await filterSheet(AN); await AN.click(ROOT + " .sh-select-trigger"); await AN.waitForFunction((root) => document.querySelector(root).semonSelect.isOpen, ROOT);
    R.open = { before: await facetState(AN) }; uFacets[0] = AN.updates; repoLog("vega", "live-vega", at(12, 44, 30));
    r.expect(await updated(AN, uFacets[0], 6000), name + ": Analytics got no update for the new repo while the sheet was open");
    R.open.after = await facetState(AN);
    r.expect(R.open.after.open && R.open.after.expanded === "true" && R.open.after.kept && R.open.after.active === R.open.before.active && R.open.after.value === first && await AN.evaluate((sel) => document.querySelector(sel)?.open === true, ".facet-filters dialog.filters-sheet"), name + ": the open list or sheet closed or lost its highlight under an update: " + JSON.stringify(R.open));
    await AN.keyboard.press("Escape"); await sleep(200); R.open.closed = await facetState(AN);
    r.expect(!R.open.closed.open && R.open.closed.triggerFocus && R.open.closed.kept, name + ": Escape didn't close the list and return focus: " + JSON.stringify(R.open.closed));
    await AN.keyboard.press("Escape"); await AN.waitForFunction(() => !document.querySelector(".facet-filters dialog.filters-sheet")?.open); R.open.shut = await facetState(AN);
    r.expect(R.open.shut.focus && R.open.shut.kept && R.open.shut.barKept, name + ": Escape didn't close the sheet and return focus to the Filter button: " + JSON.stringify(R.open.shut));
    r.expect(await hasRepo(AN, "vega") != null, name + ": Analytics: the filter has no Vega option within 4 s of the sheet closing");
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
    await S.waitForFunction(() => { const b = document.querySelector(".jump-bottom"); return !!b && !b.hidden; }, null, { timeout: 5000 });
    await S.route("**/api/model**", (q) => q.fulfill({ status: 403, contentType: "text/plain", body: "Forbidden" }));
    R.endedOnSession = await appear(S, Date.now(), (t) => document.querySelector(".livenote")?.textContent === t, ENDED, 8000);
    r.expect(R.endedOnSession != null, name + ": no \"" + ENDED + "\" note on the session page after a 403");
    R.noteVsJump = await S.evaluate(() => { const n = document.querySelector(".livenote")?.getBoundingClientRect(), j = document.querySelector(".jump-bottom"), b = j?.getBoundingClientRect(); return n && b ? { jumpShown: !j.hidden, noteTop: n.top, noteBottom: n.bottom, noteLeft: n.left, noteRight: n.right, jumpTop: b.top, jumpBottom: b.bottom, jumpLeft: b.left, jumpRight: b.right, overlaps: n.left < b.right && n.right > b.left && n.top < b.bottom && n.bottom > b.top } : null; });
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
// stale. The meta line's tool-call count follows the model there: it is the model's before, and grows with the session.
async function middleCounts(browser, r) {
  const R = { name: "middle-counts" }, dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-middle-")), now = write(dir, { extras: true });
  const srv = await serve(dir, now + 10 * 60000), L = logs(dir), backlog = L.lane("backlog"), pages = [];
  const count = (page) => page.evaluate(() => { const n = document.querySelector(".meta-tools .meta-value"); return n ? Number(n.textContent.replace(/,/g, "")) : null; });
  try {
    const m0 = await model(srv), turn = m0.turns.filter((t) => t.sid === "backlog")[1];
    r.expect(!!turn && m0.sessions.backlog.calls > 200, "middle-counts: the backlog fixture is not long enough: " + JSON.stringify({ turn: turn?.id, calls: m0.sessions.backlog?.calls }));
    const page = await open(browser, srv, "/s/claude/backlog?turn=" + encodeURIComponent(turn.id), { size: "desktop", dark: false }); pages.push(page);
    await page.waitForFunction((id) => !!document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]'), turn.id);
    R.later = await page.evaluate(() => [...document.querySelectorAll("#page button.more")].some((b) => b.textContent === "Load later"));
    r.expect(R.later, "middle-counts: the deep link did not stop short of the end (no Load later)");
    R.before = await count(page);
    r.expect(R.before === m0.sessions.backlog.calls, "middle-counts: the meta line shows " + R.before + " tool calls, the model " + m0.sessions.backlog.calls);
    const t0 = Date.now();
    for (let i = 0; i < 3; i++) backlog.append(backlog.tool(at(12, 43 + i), "toolu-mid" + i, "Bash", { command: "true" }), backlog.result(at(12, 43 + i, 5), "toolu-mid" + i, "ok"));
    R.grew = await appear(page, t0, (want) => Number(document.querySelector(".meta-tools .meta-value")?.textContent.replace(/,/g, "")) === want, R.before + 3, 8000);
    R.after = await count(page);
    r.expect(R.grew != null, "middle-counts: the meta line stayed at " + R.after + " after the session made 3 more calls (model " + (await model(srv)).sessions.backlog.calls + ")");
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

// A subagent's page shows its brief once, in the intro block at the top. The transcript leaves out the handoff that started it,
// and a live update (the subagent is still running) redraws its turns the same way: it must not bring the brief back as a turn
// header and a quoted copy under the "Started" divider.
async function childBriefOnce(browser, r) {
  const R = { name: "child-brief-once" }, dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-brief-")), now = write(dir);
  const srv = await serve(dir, now + 10 * 60000), L = logs(dir), harbor = L.lane("harbor"), pages = [];
  const NOTE = "Live check: the brief-once note, written while the page is open.";
  // The elements whose own text holds the brief (the deepest ones), and whether each sits in the intro block.
  const copies = (page) => page.evaluate((brief) => [...document.querySelectorAll("#page *")]
    .filter((n) => n.textContent.includes(brief) && ![...n.children].some((c) => c.textContent.includes(brief)))
    .map((n) => ({ tag: n.tagName.toLowerCase(), cls: n.className, intro: !!n.closest(".child-intro") })), BRIEF);
  try {
    harbor.append(harbor.tool(at(12, 43), "toolu-brief1", "Agent", { description: "Live reviewer", subagent_type: "general-purpose", prompt: BRIEF, run_in_background: true }),
      harbor.result(at(12, 43, 0, 500), "toolu-brief1", "Async agent launched successfully.", { toolUseResult: { status: "async_launched", agentId: "brief-sub" } }));
    const sub = L.lane("brief-sub", { parent: "harbor" });
    fs.mkdirSync(path.dirname(sub.path), { recursive: true });
    fs.writeFileSync(sub.path.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Live reviewer", toolUseId: "toolu-brief1" }));
    sub.append(sub.prompt(at(12, 43, 1), BRIEF), sub.text(at(12, 43, 20), "Live check: reading the flush change."));
    const t0 = Date.now();
    while (Date.now() - t0 < 8000 && !(await model(srv)).handoffs.some((h) => h.kind === "spawn" && h.to === "brief-sub")) await sleep(50);
    const page = await open(browser, srv, "/s/claude/brief-sub", { size: "phone", dark: false }); pages.push(page);
    await page.waitForFunction(() => !!document.querySelector("#page .child-intro"));
    R.opened = await copies(page);
    r.expect(R.opened.length === 1 && R.opened[0].intro, "child-brief-once: on opening, the brief is in " + R.opened.length + " elements, not only in the intro block: " + JSON.stringify(R.opened));
    // The subagent goes on working: a live update draws its turn again.
    const updates = page.updates, t1 = Date.now();
    sub.append(sub.text(at(12, 44), NOTE));
    R.followed = await appear(page, t1, (note) => [...document.querySelectorAll("#page .turns .msg")].some((x) => x.textContent.includes(note)), NOTE, 8000);
    r.expect(R.followed != null && page.updates > updates, "child-brief-once: the live update never drew the subagent's new message");
    await sleep(300);
    R.live = await copies(page);
    r.expect(R.live.length === 1 && R.live[0].intro, "child-brief-once: after a live update the brief is in " + R.live.length + " elements, not only in the intro block: " + JSON.stringify(R.live));
    R.turnHeads = await page.evaluate(() => [...document.querySelectorAll("#page .turn-h")].map((h) => h.textContent));
    r.expect(R.turnHeads.every((t) => !t.includes("Brief from")), "child-brief-once: a turn header repeats the brief's sender: " + JSON.stringify(R.turnHeads));
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "child-brief-once: page errors: " + R.errors.join(" | "));
  } finally {
    for (const p of pages) await p.context().close();
    srv.proc.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return R;
}

// A subagent's page ends in its status line, and it follows the subagent while the page is open: the tool-call count and the
// cost advance while it works, and it reads "Returned to <parent> · Done" once the subagent finishes, with no reload. Each item
// (calls, time, cost) has a plain-text tip with its breakdown, kept up with the numbers; the state text has none.
async function childReturnLive(browser, r) {
  const R = { name: "child-return-live" }, dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-return-")), now = write(dir);
  const srv = await serve(dir, now + 10 * 60000), L = logs(dir), harbor = L.lane("harbor"), pages = [];
  const COST_TIP = "What these tokens would cost at API rates. Subscriptions (Claude Max, ChatGPT plans) aren't billed this way.";
  // The status line: its text, its state class, whether it has the button to the parent, and each item's text and tip.
  const block = (page) => page.evaluate(() => { const n = document.querySelector("#page .session-foot"); return n ? { n: document.querySelectorAll("#page .session-foot").length, text: n.querySelector(".stat")?.textContent ?? "", state: n.querySelector(".stat")?.className ?? "", open: !!n.querySelector("button"),
    items: [...n.querySelectorAll(".cr-item")].map((x) => ({ text: x.textContent, tip: x.dataset.tip ?? null, tab: x.tabIndex === 0, control: !!x.closest("a[href], button, input, select, textarea, summary, label, [role='button'], [role='link'], [role='menuitem'], [role='tab']") })),
    untipped: [...n.querySelectorAll("[data-tip]")].filter((x) => !x.classList.contains("cr-item")).length } : null; });
  const CACHE = { input: 100000, output: 10000, cacheRead: 1000000, cacheWrite: 100000 }; // $0.40, $0.20, $0.20, $0.50 at Opus 5.5 rates
  // The focused element: its footer item kind (data-foot), its text, and whether it is in the footer and in the document.
  const focused = (page) => page.evaluate(() => { const a = document.activeElement; return { inFoot: !!a?.closest("#page .session-foot"), foot: a?.dataset?.foot ?? null, text: a?.textContent ?? null, connected: !!a?.isConnected }; });
  let lastModel = null;
  try {
    harbor.append(harbor.tool(at(12, 43), "toolu-ret1", "Agent", { description: "Live returner", subagent_type: "general-purpose", prompt: BRIEF, run_in_background: true }),
      harbor.result(at(12, 43, 0, 500), "toolu-ret1", "Async agent launched successfully.", { toolUseResult: { status: "async_launched", agentId: "ret-sub" } }));
    const sub = L.lane("ret-sub", { parent: "harbor" });
    fs.mkdirSync(path.dirname(sub.path), { recursive: true });
    fs.writeFileSync(sub.path.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Live returner", toolUseId: "toolu-ret1" }));
    sub.append(sub.prompt(at(12, 43, 1), BRIEF), sub.tool(at(12, 43, 5), "toolu-ret-t0", "Bash", { command: "true" }), sub.result(at(12, 43, 6), "toolu-ret-t0", "ok"), sub.usage(at(12, 43, 10), CACHE), sub.text(at(12, 43, 20), "Live check: reading the flush change."));
    // A second subagent that has already returned and has no token use yet, so its footer has no cost: [time, "Open in"].
    harbor.append(harbor.tool(at(12, 43, 2), "toolu-ret2", "Agent", { description: "Live unpriced", subagent_type: "general-purpose", prompt: BRIEF, run_in_background: true }),
      harbor.result(at(12, 43, 2, 500), "toolu-ret2", "Async agent launched successfully.", { toolUseResult: { status: "async_launched", agentId: "ret-sub2" } }));
    const sub2 = L.lane("ret-sub2", { parent: "harbor" });
    fs.writeFileSync(sub2.path.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Live unpriced", toolUseId: "toolu-ret2" }));
    sub2.append(sub2.prompt(at(12, 43, 3), BRIEF), sub2.text(at(12, 43, 30), "Live check: nothing to change."));
    harbor.append(harbor.notify(at(12, 43, 40), "toolu-ret2", "completed", "Live check: nothing to change."));
    const t0 = Date.now();
    while (Date.now() - t0 < 8000 && !["ret-sub", "ret-sub2"].every((id) => lastModel?.handoffs.some((h) => h.kind === "spawn" && h.to === id))) { lastModel = await model(srv); await sleep(50); }
    const page = await open(browser, srv, "/s/claude/ret-sub", { size: "phone", dark: false }); pages.push(page);
    await page.evaluate(() => { window.__noReload = true; });
    await page.waitForFunction(() => !!document.querySelector("#page .session-foot"));
    R.opened = await block(page);
    const o = R.opened;
    r.expect(o?.n === 1 && /^Working\s+·\s+1 tool call\s+·\s+[\dhdm ]+\s+·\s+\$1\.30$/.test(o.text) && o.state.includes("work") && !o.open, "child-return-live: on opening, the running subagent's status line is " + JSON.stringify(o));
    r.expect(o?.items.length === 3 && o.untipped === 0 && !o.items.some((x) => x.control), "child-return-live: the status line's items are not three static labels: " + JSON.stringify(o?.items));
    r.expect(o?.items.every((x) => x.tab), "child-return-live: a tipped item can't take keyboard focus: " + JSON.stringify(o?.items));
    r.expect(o?.items[0]?.tip === "Bash 1" && /^Started 12:43 · last activity 12:\d\d$/.test(o.items[1]?.tip ?? ""), "child-return-live: on opening, the calls and time tips are " + JSON.stringify(o?.items.slice(0, 2).map((x) => x.tip)));
    r.expect(o?.items[2]?.tip === "Input $0.40 · Output $0.20 · Cache read $0.20 · Cache write $0.50. " + COST_TIP, "child-return-live: on opening, the cost tip is " + JSON.stringify(o?.items[2]?.tip));
    // On a phone a tap on a static label shows its tip.
    await page.locator("#page .session-foot .cr-item").nth(0).tap();
    R.tapped = await page.evaluate(() => { const tip = document.getElementById("sh-tooltip"); return tip && !tip.hidden ? tip.textContent : null; });
    r.expect(R.tapped === "Bash 1", "child-return-live: tapping the calls label showed " + JSON.stringify(R.tapped));
    await page.locator("#page .session-foot .cr-item").nth(0).tap(); // a second tap on it closes the tip
    // It makes two more calls (a read, and a bash that fails) and more token use: the count, the tips and the cost advance.
    let u0 = page.updates, t1 = Date.now();
    sub.append(sub.tool(at(12, 44), "toolu-ret-t1", "Read", { file_path: "/x" }), sub.result(at(12, 44, 1), "toolu-ret-t1", "ok"), sub.tool(at(12, 44, 10), "toolu-ret-t2", "Bash", { command: "false" }), sub.fail(at(12, 44, 11), "toolu-ret-t2", "exit 1"), sub.usage(at(12, 44, 30), CACHE));
    R.counted = await appear(page, t1, () => /^Working\s+·\s+3 tool calls\s+·\s+[\dhdm ]+\s+·\s+\$2\.60$/.test(document.querySelector("#page .session-foot .stat")?.textContent ?? ""), null, 8000);
    R.working = await block(page);
    const w = R.working;
    r.expect(R.counted != null && page.updates > u0, "child-return-live: the status line stayed at " + JSON.stringify(w) + " after the subagent made 2 more calls and more token use");
    r.expect(w?.items[0]?.tip === "Bash 2 · Read 1 · 1 failed", "child-return-live: after 2 more calls the calls tip is " + JSON.stringify(w?.items[0]?.tip));
    r.expect(w?.items[1]?.tip === "Started 12:43 · last activity 12:44", "child-return-live: after the update the time tip is " + JSON.stringify(w?.items[1]?.tip));
    r.expect(w?.items[2]?.tip === "Input $0.80 · Output $0.40 · Cache read $0.40 · Cache write $1.00. " + COST_TIP, "child-return-live: after the update the cost tip is " + JSON.stringify(w?.items[2]?.tip));
    // It finishes (the parent is told): the line says so, with the button to the parent, and the time tip says when. The cost item
    // has the focus meanwhile: the items go from [calls, time, cost] to [time, cost, "Open in"], and the focus stays on the cost.
    await page.locator('#page .session-foot [data-foot="cost"]').focus();
    u0 = page.updates; t1 = Date.now();
    harbor.append(harbor.notify(at(12, 45), "toolu-ret1", "completed", "Live check: the flush change looks right."));
    R.returned = await appear(page, t1, () => /^Returned to .+\s+·\s+Done\s+·\s+[\dhdm ]+\s+·\s+\$2\.60$/.test(document.querySelector("#page .session-foot .stat")?.textContent ?? ""), null, 8000);
    R.done = await block(page);
    const d = R.done;
    r.expect(R.returned != null && page.updates > u0, "child-return-live: the status line stayed at " + JSON.stringify(d) + " after the subagent finished");
    r.expect(d?.n === 1 && d.state.includes("done") && d.open && d.untipped === 0, "child-return-live: after it finished, the status line is " + JSON.stringify(d));
    r.expect(d?.items.length === 2 && d.items[0].tip === "Started 12:43 · last activity 12:44 · finished 12:45" && d.items[1].tip === "Input $0.80 · Output $0.40 · Cache read $0.40 · Cache write $1.00. " + COST_TIP, "child-return-live: after it finished, the items are " + JSON.stringify(d?.items));
    R.costFocus = await focused(page);
    r.expect(R.costFocus.inFoot && R.costFocus.foot === "cost" && R.costFocus.text === "$2.60" && R.costFocus.connected, "child-return-live: with the cost item focused while the subagent finished, the focus went to " + JSON.stringify(R.costFocus));
    // A redraw that changes the footer's text (more cost) keeps the focus on "Open in <parent>", on the button that replaced it.
    await page.locator("#page .session-foot button").focus();
    t1 = Date.now();
    sub.append(sub.usage(at(12, 46), CACHE));
    R.recosted = await appear(page, t1, () => /\$3\.90$/.test(document.querySelector("#page .session-foot .stat")?.textContent ?? ""), null, 8000);
    R.focus = await focused(page);
    r.expect(R.recosted != null, "child-return-live: the finished footer stayed at " + JSON.stringify(await block(page)) + " after the subagent's cost grew");
    r.expect(R.focus.inFoot && R.focus.foot === "open" && R.focus.text === "Open in harbor" && R.focus.connected, "child-return-live: the focus after the footer's text changed is " + JSON.stringify(R.focus));
    // A returned subagent with no cost yet: "Open in" has the focus when its cost gets priced, and the items go from
    // [time, "Open in"] to [time, cost, "Open in"]. The focus stays on the button, not on the cost that took its place.
    const unpriced = await open(browser, srv, "/s/claude/ret-sub2", { size: "phone", dark: false }); pages.push(unpriced);
    await unpriced.waitForFunction(() => /^Returned to .+\s+·\s+Done\s+·\s+[\dhdm ]+$/.test(document.querySelector("#page .session-foot .stat")?.textContent ?? ""));
    R.unpriced = await block(unpriced);
    r.expect(R.unpriced?.items.length === 1 && R.unpriced.open, "child-return-live: a returned subagent with no token use has the footer " + JSON.stringify(R.unpriced));
    await unpriced.locator("#page .session-foot button").focus();
    t1 = Date.now();
    sub2.append(sub2.usage(at(12, 43, 35), CACHE));
    R.priced = await appear(unpriced, t1, () => /\$1\.30$/.test(document.querySelector("#page .session-foot .stat")?.textContent ?? ""), null, 8000);
    R.pricedFocus = await focused(unpriced);
    r.expect(R.priced != null, "child-return-live: the unpriced subagent's footer stayed at " + JSON.stringify(await block(unpriced)) + " after it used tokens");
    r.expect(R.pricedFocus.inFoot && R.pricedFocus.foot === "open" && R.pricedFocus.text === "Open in harbor" && R.pricedFocus.connected, "child-return-live: with \"Open in\" focused while the cost got priced, the focus went to " + JSON.stringify(R.pricedFocus));
    R.kept = await page.evaluate(() => window.__noReload === true);
    r.expect(R.kept, "child-return-live: the page was reloaded");
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "child-return-live: page errors: " + R.errors.join(" | "));
  } finally {
    for (const p of pages) await p.context().close();
    srv.proc.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return R;
}

// A session that is not a subagent ends its page in the same footer: "Working · N tool calls · time · cost" while it runs, with
// the count following its calls, and no "Still working" line under its last turn (the footer says it once). A session that has
// stopped reads its state ("Done · N tool calls · …") with the model's call count.
async function sessionFooterLive(browser, r) {
  const R = { name: "session-footer-live" }, dir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-live-foot-")), now = write(dir);
  const srv = await serve(dir, now + 10 * 60000), L = logs(dir), harbor = L.lane("harbor"), pages = [];
  const foot = (page) => page.evaluate(() => { const n = document.querySelector("#page .session-foot"); return n ? { n: document.querySelectorAll("#page .session-foot").length, text: n.querySelector(".stat")?.textContent ?? "", state: n.querySelector(".stat")?.className ?? "", open: !!n.querySelector("button"),
    calls: Number(/(\d+) tool call/.exec(n.textContent)?.[1] ?? NaN), tips: [...n.querySelectorAll(".cr-item")].map((x) => x.dataset.tip ?? null),
    still: [...document.querySelectorAll("#page .turn-end")].filter((x) => x.textContent.includes("Still working")).length, intro: document.querySelectorAll("#page .child-intro").length } : null; });
  const WORDS = { done: "Done", err: "Failed", idle: "Idle", wait: "Needs you" };
  try {
    // A running session: harbor, which starts a subagent in its last turn (that turn sent something, so it has a trace button).
    harbor.append(harbor.tool(at(12, 41, 30), "toolu-foot0", "Agent", { description: "Footer reviewer", subagent_type: "general-purpose", prompt: BRIEF, run_in_background: true }),
      harbor.result(at(12, 41, 31), "toolu-foot0", "Async agent launched successfully.", { toolUseResult: { status: "async_launched", agentId: "foot-sub" } }));
    const sub = L.lane("foot-sub", { parent: "harbor" });
    fs.mkdirSync(path.dirname(sub.path), { recursive: true });
    fs.writeFileSync(sub.path.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Footer reviewer", toolUseId: "toolu-foot0" }));
    sub.append(sub.prompt(at(12, 41, 32), BRIEF), sub.text(at(12, 41, 40), "Live check: reading."));
    const t0 = Date.now();
    while (Date.now() - t0 < 8000 && !(await model(srv)).handoffs.some((h) => h.kind === "spawn" && h.to === "foot-sub")) await sleep(50);
    const page = await open(browser, srv, "/s/claude/harbor", { size: "phone", dark: false }); pages.push(page);
    const lastTurn = (pg) => pg.evaluate(() => { const turns = [...document.querySelectorAll("#page .turns > .turn")], last = turns.at(-1); return { turns: turns.length, trace: last?.querySelectorAll(":scope > .turn-end .tracebtn").length ?? 0, still: last?.querySelectorAll(":scope > .turn-end .stat.work").length ?? 0 }; });
    await page.waitForFunction(() => !!document.querySelector("#page .session-foot"));
    R.opened = await foot(page);
    const o = R.opened;
    r.expect(o?.n === 1 && o.intro === 0 && /^Working\s+·\s+\d+ tool calls?\s+·\s+[\dhdm ]+(\s+·\s+\$\d+\.\d\d)?$/.test(o.text) && o.state.includes("work") && !o.open, "session-footer-live: on opening, the running session's footer is " + JSON.stringify(o));
    r.expect(o?.still === 0, "session-footer-live: the last turn still says \"Still working\" under the footer (" + o?.still + ")");
    R.last = await lastTurn(page);
    r.expect(R.last.trace === 1 && R.last.still === 0, "session-footer-live: the last turn has " + R.last.trace + " trace buttons and " + R.last.still + " working lines (want one trace button, no working line): " + JSON.stringify(R.last));
    r.expect(o?.tips.length >= 2 && o.tips.every((x) => !!x) && /^Started (?:\w{3} )?\d\d:\d\d · last activity (?:\w{3} )?\d\d:\d\d$/.test(o.tips[1] ?? ""), "session-footer-live: the footer's tips are " + JSON.stringify(o?.tips));
    const u0 = page.updates, t1 = Date.now();
    harbor.append(harbor.tool(at(12, 42, 50), "toolu-foot1", "Bash", { command: "true" }), harbor.result(at(12, 42, 55), "toolu-foot1", "ok"));
    R.counted = await appear(page, t1, (was) => Number(/(\d+) tool call/.exec(document.querySelector("#page .session-foot")?.textContent ?? "")?.[1]) === was + 1, o?.calls, 8000);
    R.working = await foot(page);
    r.expect(R.counted != null && page.updates > u0, "session-footer-live: the footer stayed at " + JSON.stringify(R.working) + " after the session made one more call (was " + o?.calls + ")");
    r.expect(R.working?.n === 1 && R.working.still === 0 && R.working.state.includes("work"), "session-footer-live: after the update the footer is " + JSON.stringify(R.working));
    R.lastAfter = await lastTurn(page);
    r.expect(R.lastAfter.trace === 1 && R.lastAfter.still === 0, "session-footer-live: after the update the last turn has " + R.lastAfter.trace + " trace buttons and " + R.lastAfter.still + " working lines: " + JSON.stringify(R.lastAfter));
    // A session that has stopped: its state and the model's call count.
    const sessions = Object.entries((await model(srv)).sessions).filter(([, s]) => !s.parent && !s.stub && !s.role && s.state in WORDS && s.calls > 0);
    const pick = sessions.find(([, s]) => s.state === "done") ?? sessions[0];
    r.expect(!!pick, "session-footer-live: the fixture has no stopped top-level session");
    if (pick) {
      const [sid, s] = pick, second = await open(browser, srv, "/s/" + s.harness + "/" + sid, { size: "phone", dark: false }); pages.push(second);
      await second.waitForFunction(() => !!document.querySelector("#page .session-foot"));
      R.stopped = await foot(second); R.stopped.session = sid; R.stopped.model = { state: s.state, calls: s.calls };
      const d = R.stopped, want = new RegExp("^" + WORDS[s.state] + "\\s+·\\s+" + s.calls + " tool calls?\\s+·\\s+[\\dhdm ]+(\\s+·\\s+\\$\\d+\\.\\d\\d)?$");
      r.expect(d.n === 1 && want.test(d.text) && d.state.includes(s.state) && !d.open, "session-footer-live: a stopped session's footer is " + JSON.stringify(d));
      if (s.state === "done") r.expect(/ · finished (?:\w{3} )?\d\d:\d\d$/.test(d.tips[1] ?? ""), "session-footer-live: a done session's time tip is " + JSON.stringify(d.tips));
    }
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "session-footer-live: page errors: " + R.errors.join(" | "));
  } finally {
    for (const p of pages) await p.context().close();
    srv.proc.kill("SIGTERM");
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return R;
}

// A subagent's page opened before its parent's log exists: its own transcript, and the parent's log to write once the page is
// open (the subagent then has an origin). `shown` reads the intro blocks, the child-page class and where the brief is drawn.
async function lateFixture(browser, prefix) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix)), now = write(dir);
  const srv = await serve(dir, now + 10 * 60000), L = logs(dir), pages = [];
  const cwd = L.cwdOf("harbor"), projects = L.dirOf("harbor"), NOTE = "Live check: the late brief.";
  const line = (sub, t, type, extra) => ({ parentUuid: null, isSidechain: sub, type, timestamp: iso(t), sessionId: "late-par", ...(sub ? { agentId: "late-sub" } : {}), cwd, version: "2.1.0", uuid: "u-late-" + (sub ? "s" : "p") + "-" + t, ...extra });
  const say = (sub, t, text) => line(sub, t, "assistant", { message: { id: "msg-late-" + t, model: "claude-opus-5-5", role: "assistant", type: "message", content: [{ type: "text", text }] } });
  const ask = (sub, t, text) => line(sub, t, "user", { message: { role: "user", content: text } });
  const put = (f, lines) => { fs.mkdirSync(path.dirname(f), { recursive: true }); fs.appendFileSync(f, lines.map((l) => JSON.stringify(l) + "\n").join("")); };
  const shown = (page) => page.evaluate((note) => ({ intros: document.querySelectorAll("#page .child-intro").length, child: document.querySelector("#page").classList.contains("child-page"), foot: document.querySelectorAll("#page .session-foot").length,
    copies: [...document.querySelectorAll("#page *")].filter((n) => n.textContent.includes(note) && ![...n.children].some((c) => c.textContent.includes(note))).map((n) => ({ tag: n.tagName.toLowerCase(), intro: !!n.closest(".child-intro") })) }), NOTE);
  const close = async () => { for (const p of pages) await p.context().close(); srv.proc.kill("SIGTERM"); fs.rmSync(dir, { recursive: true, force: true }); };
  try {
    const subFile = path.join(projects, "late-par", "subagents", "agent-late-sub.jsonl");
    put(subFile, [ask(true, at(12, 43, 1), NOTE), say(true, at(12, 43, 20), "Live check: reading the flush change.")]);
    fs.writeFileSync(subFile.replace(/\.jsonl$/, ".meta.json"), JSON.stringify({ agentType: "general-purpose", description: "Late reviewer", toolUseId: "toolu-late1" }));
    const page = await open(browser, srv, "/s/claude/late-sub", { size: "phone", dark: false }); pages.push(page);
    // Each /api/model request the page makes: when, and the `since` it sent ("" asks for the whole model).
    page.polls = []; page.on("request", (q) => { if (q.url().includes("/api/model")) page.polls.push({ t: Date.now(), since: new URL(q.url()).searchParams.get("since") }); });
    const parent = () => put(path.join(projects, "late-par.jsonl"), [ask(false, at(12, 40), "Live check: start a late reviewer."), say(false, at(12, 43), "Live check: started it.")]);
    return { page, pages, parent, shown, close, NOTE };
  } catch (e) { await close(); throw e; }
}
const RELOAD = /\/api\/tx\?sid=late-sub$/;
// The viewer's LATE_TRIES: how many times a page whose origin arrived late asks for its transcript again before it gives up.
const LATE_TRIES = 4;
// The page's visibility, as the browser reports it when the phone is locked and unlocked.
const setVisible = (page, state) => page.evaluate((state) => { Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state }); document.dispatchEvent(new Event("visibilitychange")); }, state);

// A subagent's page opened before its parent's log exists has no origin and no intro. When the parent's log appears, the live
// update adds the intro (once) and marks the page a child's, and the brief is in that intro alone (the transcript loads again, so
// the prompt it drew as a message is gone). The first request to load it again fails, and the page is hidden and shown again
// before the retry: polling resumes (asking for the whole model), the retry loads it, and it is not asked for again.
async function childIntroLate(browser, r) {
  const R = { name: "child-intro-late" }, F = await lateFixture(browser, "semon-live-late-"), { page, pages, shown, NOTE } = F;
  try {
    R.before = await shown(page);
    r.expect(R.before.intros === 0 && !R.before.child, "child-intro-late: before its parent's log exists the page already has an intro or the child-page class: " + JSON.stringify(R.before));
    const base = page.txs.length, reloads = () => page.txs.slice(base).filter((u) => RELOAD.test(u)).length;
    let asked = 0;
    await page.route(RELOAD, (route) => (asked++ === 0 ? route.fulfill({ status: 500, body: "no" }) : route.continue()));
    const failed = page.waitForResponse((q) => RELOAD.test(q.url()) && q.status() === 500, { timeout: 10000 }).then(() => true, () => false);
    // The parent's log appears: the subagent now has an origin.
    const u0 = page.updates, t1 = Date.now();
    F.parent();
    R.added = await appear(page, t1, () => document.querySelectorAll("#page .child-intro").length > 0, null, 8000);
    R.failedFirst = await failed;
    r.expect(R.failedFirst, "child-intro-late: the first reload of the transcript was never made (or didn't fail)");
    // The phone is locked before the retry, and unlocked: the page polls again (a poll that asks for the whole model), at the
    // backoff the failed request set.
    await setVisible(page, "hidden");
    let n = page.polls.length; await sleep(1500);
    R.hiddenPolls = page.polls.length - n;
    n = page.polls.length; const t2 = Date.now();
    await setVisible(page, "visible");
    while (page.polls.length === n && Date.now() - t2 < 8000) await sleep(50);
    R.resumedIn = page.polls.length > n ? Date.now() - t2 : null; R.resumed = page.polls[n] ?? null;
    r.expect(R.hiddenPolls === 0, "child-intro-late: polled " + R.hiddenPolls + " times while hidden");
    r.expect(R.resumedIn != null, "child-intro-late: shown again after a failed reload of its transcript, the page never polled again");
    r.expect(R.resumed?.since === "", "child-intro-late: the poll after the failed reload asked for " + JSON.stringify(R.resumed) + " (want since=\"\": the whole model)");
    R.fixed = await appear(page, t2, (note) => { const c = [...document.querySelectorAll("#page *")].filter((n) => n.textContent.includes(note) && ![...n.children].some((k) => k.textContent.includes(note))); return c.length === 1 && !!c[0].closest(".child-intro"); }, NOTE, 12000);
    n = page.polls.length; const t3 = Date.now();
    while (page.polls.length < n + 2 && Date.now() - t3 < 8000) await sleep(50); // two more polls: a transcript that loaded is not loaded again
    R.reloads = reloads(); R.later = page.polls.slice(n).map((p) => p.since);
    R.after = await shown(page);
    r.expect(R.reloads === 2, "child-intro-late: the transcript was asked for " + R.reloads + " times after the origin appeared (want 2: one that failed, one retry)");
    r.expect(R.later.length >= 2 && R.later.every((x) => !!x), "child-intro-late: the polls after the transcript loaded sent " + JSON.stringify(R.later) + " (want the model's version)");
    r.expect(R.fixed != null, "child-intro-late: the brief was not in the intro alone after the retry: " + JSON.stringify(R.after.copies));
    r.expect(R.added != null && page.updates > u0, "child-intro-late: the intro never appeared after the parent's log did: " + JSON.stringify(R.after));
    r.expect(R.after.intros === 1 && R.after.child, "child-intro-late: after the origin appeared the page has " + R.after.intros + " intro blocks (child-page " + R.after.child + ")");
    r.expect(R.after.copies.length === 1 && R.after.copies[0].intro, "child-intro-late: after the origin appeared the brief is in " + R.after.copies.length + " elements, not only in the intro block: " + JSON.stringify(R.after.copies));
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "child-intro-late: page errors: " + R.errors.join(" | "));
  } finally {
    await F.close();
  }
  return R;
}

// When the transcript never loads again (every request fails), the page asks for it LATE_TRIES times in all, each retry on a poll
// that asks for the whole model, with gaps that double (4 s, 8 s, 16 s); then it gives up and polls as usual (2 s, sending the
// model's version), with the intro and footer drawn and no page errors. Spacing comes from the request timestamps.
async function childIntroLateGivesUp(browser, r) {
  const R = { name: "child-intro-late-gives-up" }, F = await lateFixture(browser, "semon-live-giveup-"), { page, pages, shown } = F;
  try {
    const tries = [];
    page.on("request", (q) => { if (RELOAD.test(q.url())) tries.push(Date.now()); });
    await page.route(RELOAD, (route) => route.fulfill({ status: 500, body: "no" }));
    const t1 = Date.now();
    F.parent();
    while (tries.length < LATE_TRIES && Date.now() - t1 < 50000) await sleep(100);
    // Three polls after the last try: none asks for the transcript again.
    const last = () => tries[tries.length - 1] ?? Infinity, after = () => page.polls.filter((p) => p.t > last());
    const t2 = Date.now();
    while (after().length < 3 && Date.now() - t2 < 15000) await sleep(100);
    R.tries = tries.length; R.gaps = tries.slice(1).map((t, i) => t - tries[i]);
    R.between = page.polls.filter((p) => p.t > tries[0] && p.t < last()).map((p) => p.since);
    R.after = after().map((p) => ({ since: p.since, at: p.t - last() }));
    R.shown = await shown(page);
    r.expect(R.tries === LATE_TRIES, "child-intro-late-gives-up: the failing transcript was asked for " + R.tries + " times (want " + LATE_TRIES + ")");
    r.expect(R.gaps.length === 3 && R.gaps[0] >= 3500 && R.gaps[1] >= 7000 && R.gaps[2] >= 14000 && R.gaps[2] < 24000 && R.gaps[0] < R.gaps[1] && R.gaps[1] < R.gaps[2], "child-intro-late-gives-up: the retries didn't back off 4 s, 8 s, 16 s: " + JSON.stringify(R.gaps));
    r.expect(R.between.length === LATE_TRIES - 1 && R.between.every((x) => x === ""), "child-intro-late-gives-up: the polls that retried sent " + JSON.stringify(R.between) + " (want " + (LATE_TRIES - 1) + " asking for the whole model)");
    r.expect(R.after.length >= 3 && R.after.every((p) => !!p.since) && R.after[0].at < 5000, "child-intro-late-gives-up: after giving up, the polls are " + JSON.stringify(R.after) + " (want the model's version, every 2 s)");
    r.expect(R.shown.intros === 1 && R.shown.child, "child-intro-late-gives-up: after giving up the page shows " + JSON.stringify(R.shown));
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "child-intro-late-gives-up: page errors: " + R.errors.join(" | "));
  } finally {
    await F.close();
  }
  return R;
}

// A failed late-origin reload leaves the original brief in the transcript. Leaving the page caches those entries, so when the
// route change clears the retry state, that cache must be dropped before the session is opened again.
async function childIntroLateAway(browser, r) {
  const R = { name: "child-intro-late-away" }, F = await lateFixture(browser, "semon-live-late-away-"), { page, pages, shown } = F;
  try {
    let asked = 0;
    await page.route(RELOAD, (route) => (asked++ === 0 ? route.fulfill({ status: 500, body: "no" }) : route.continue()));
    const failed = page.waitForResponse((q) => RELOAD.test(q.url()) && q.status() === 500, { timeout: 10000 }).then(() => true, () => false);
    const t1 = Date.now();
    F.parent();
    R.origin = await appear(page, t1, () => !!document.querySelector("#page .child-intro"), null, 8000);
    R.failedFirst = await failed;
    r.expect(R.origin != null && R.failedFirst, "child-intro-late-away: the origin did not appear with its first transcript reload failing");

    await goto(page, { v: "home" }, {});
    R.home = await page.evaluate(() => document.querySelector("#page .ph h1")?.textContent);
    const redraws = await page.evaluate(() => {
      const box = document.querySelector("#page");
      window.__lateAwayHomeRedraws = 0;
      new MutationObserver(() => { window.__lateAwayHomeRedraws++; }).observe(box, { childList: true, subtree: true });
      return window.__lateAwayHomeRedraws;
    });
    let homeHandoff = false;
    const homePoll = page.waitForResponse((q) => {
      if (!q.url().includes("/api/model") || q.status() !== 200) return false;
      const u = new URL(q.url()); if (u.searchParams.get("since") !== "") return false;
      return q.json().then((m) => homeHandoff = m.handoffs.some((h) => h.kind === "spawn" && h.to === "late-sub"));
    }, { timeout: 12000 }).then(() => true, () => false);
    R.homePoll = await homePoll;
    if (R.homePoll) await page.waitForFunction((n) => window.__lateAwayHomeRedraws > n, redraws, { timeout: 12000 });
    R.homeHandoff = homeHandoff;
    r.expect(R.home === "Home" && R.homePoll && R.homeHandoff, "child-intro-late-away: Home did not settle on the model update with the child's origin");

    const returnedTx = page.waitForResponse((q) => RELOAD.test(q.url()) && q.status() === 200, { timeout: 10000 }).then(() => true, () => false);
    await goto(page, { v: "session", id: "late-sub" }, { SESS: { "late-sub": { name: "Late reviewer" } } });
    R.returnedReload = await returnedTx;
    await page.waitForFunction(() => !!document.querySelector("#page .child-intro") && !!document.querySelector('#page section[aria-label="Transcript"]') && !document.querySelector("#page").hasAttribute("aria-busy"));
    R.after = await shown(page);
    r.expect(R.returnedReload, "child-intro-late-away: returning to the subagent reused cached transcript entries");
    r.expect(R.after.copies.length === 1 && R.after.copies[0].intro, "child-intro-late-away: after returning the brief is in " + R.after.copies.length + " elements, not only in the intro block: " + JSON.stringify(R.after.copies));
    R.errors = pages.flatMap((p) => p.errors);
    r.expect(R.errors.length === 0, "child-intro-late-away: page errors: " + R.errors.join(" | "));
  } finally {
    await F.close();
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
  try { out.childBriefOnce = await childBriefOnce(browser, r); }
  catch (e) { r.expect(false, "child-brief-once: threw " + (e?.stack ?? e)); }
  try { out.childReturnLive = await childReturnLive(browser, r); }
  catch (e) { r.expect(false, "child-return-live: threw " + (e?.stack ?? e)); }
  try { out.sessionFooterLive = await sessionFooterLive(browser, r); }
  catch (e) { r.expect(false, "session-footer-live: threw " + (e?.stack ?? e)); }
  try { out.childIntroLate = await childIntroLate(browser, r); }
  catch (e) { r.expect(false, "child-intro-late: threw " + (e?.stack ?? e)); }
  try { out.childIntroLateGivesUp = await childIntroLateGivesUp(browser, r); }
  catch (e) { r.expect(false, "child-intro-late-gives-up: threw " + (e?.stack ?? e)); }
  try { out.childIntroLateAway = await childIntroLateAway(browser, r); }
  catch (e) { r.expect(false, "child-intro-late-away: threw " + (e?.stack ?? e)); }
  r.results = out;
  return r.done();
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  let ok = false;
  try { ok = await liveCheck(browser); } finally { await browser.close(); }
  if (!ok) process.exit(1);
}

// Request, byte and main-thread budget per screen (perf step 0). Records, without changing any behaviour and without any
// timing assertion, what each screen costs the browser: requests grouped by path (count, status codes, transferred and
// decoded bytes), long tasks (count, total, total blocking time, largest) and, for a session, the time until its first
// entry is drawn. Each viewport gets a fresh copy of the long fixture (tests/ui/long.mjs: a 3000-entry session, a 1500-entry
// one, five child runs and twenty short sessions) served by its own `semon sessions --serve`, and each scenario gets its
// own browser context, so nothing is cached between them. At 390 the CPU is throttled 4x, as tests/ui/checks/perf.mjs does.
//
// Scenarios, at 1280 and 390, light only:
//   home-idle        open Home, then stay 10 s
//   sessions         open Sessions
//   session-open     open the longest session (marathon), until its first entry is drawn and the network is quiet
//   session-older    then load every older page ("Load earlier", from the top), until none is left
//   analytics-7d     open Analytics until its figures and charts are drawn
//   analytics-30d    then select 30 d
//   live             with marathon open at its end, append ten records every 5 s for 30 s: requests and bytes per update
//
// tests/ui/perf-budget.json holds a ceiling per scenario and metric. This check only reports: the lines over budget, the rows
// that did not settle and the scenarios that threw (a warning row: the rows it could not measure are missing) go under the
// summary heading and, on GitHub Actions, become ::warning annotations. It fails only when nothing could be measured (no
// server, no browser, no rows). It writes budget.json (and
// perf-budget.suggested.json: the measured values plus 10%, in the budget file's shape) to SEMON_UI_OUT and a table to
// $GITHUB_STEP_SUMMARY.
//
//   SEMON_BIN     the semon binary built with the test clock (default: target/debug/semon)
//   SEMON_UI_OUT  where budget.json is written (default: ./out)
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { context, ENV, launch, settled } from "../lib.mjs";
import { writeLong } from "../long.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const BIN = process.env.SEMON_BIN ?? path.resolve(here, "../../../target/debug/semon");
const BUDGET_FILE = path.resolve(here, "../perf-budget.json");
const SIZES = [
  { width: 1280, size: "desktop", throttle: 1 },
  { width: 390, size: "phone", throttle: 4 },
];
// What a budget can bound. Everything else in a row is recorded for reading only.
const BUDGETED = ["requests", "transferBytes", "decodedBytes", "txRequests", "txDecodedBytes", "blockingMs", "longestTaskMs", "firstEntryMs", "requestsPerUpdate", "transferBytesPerUpdate", "txRequestsPerUpdate", "txDecodedBytesPerUpdate"];
const HEADROOM = 1.1;
// The "Load earlier" control: its own hook, else the button the viewer drew before it had one.
const LOAD_EARLIER = "[data-load-earlier], #page section[aria-label='Transcript'] button.more:has-text('Load earlier')";
// The Analytics range buttons: by their data-range hook, else the topbar group of the current viewer.
const RANGE_BUTTONS = "[data-range], #topbar .analytics-range button";
// The 30 d button by its hooks (the mockup's data-range, the current viewer's data-e), with its label as the last fallback.
const RANGE_30 = "[data-range=\"30\"], [data-e=\"analytics-range:30\"]";
const LIVE_BATCHES = 6, LIVE_EVERY_MS = 5000, LIVE_WINDOW_MS = 30_000;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const round = (value, digits = 1) => value == null ? null : Math.round(value * 10 ** digits) / 10 ** digits;

// ---- The server -------------------------------------------------------------------------------------------------------------
function serve(dir, now) {
  return new Promise((resolve, reject) => {
    const proc = spawn(BIN, [
      "sessions", "--serve", "--listen", "127.0.0.1:0",
      "--claude-home", path.join(dir, "claude"),
      "--codex-home", path.join(dir, "codex"),
      "--proc-root", path.join(dir, "proc"),
      "--cache", path.join(dir, "index.json"),
    ], { env: { ...process.env, SEMON_TEST_NOW: String(now) }, stdio: ["ignore", "pipe", "pipe"] });
    let log = "", done = false;
    const fail = (error) => { if (done) return; done = true; clearTimeout(timer); reject(error); };
    const timer = setTimeout(() => fail(new Error(`semon printed no URL within 20 s: ${log}`)), 20_000);
    const read = (chunk) => {
      log += chunk;
      const match = /http:\/\/127\.0\.0\.1:(\d+)\/\?t=([0-9a-f]+)/.exec(log);
      if (!match || done) return;
      done = true; clearTimeout(timer);
      resolve({ proc, base: `http://127.0.0.1:${match[1]}`, token: match[2] });
    };
    proc.stdout.on("data", read);
    proc.stderr.on("data", read);
    proc.on("error", fail);
    proc.on("exit", (code) => fail(new Error(`semon exited ${code} before printing its URL: ${log}`)));
  });
}

async function stopServer(proc) {
  if (!proc || proc.exitCode !== null) return;
  proc.kill("SIGTERM");
  await new Promise((resolve) => {
    const timer = setTimeout(() => { proc.kill("SIGKILL"); resolve(); }, 3000);
    proc.once("exit", () => { clearTimeout(timer); resolve(); });
  });
}

// ---- Requests ---------------------------------------------------------------------------------------------------------------
// Every request of the context (its pages and their workers): grouped by path without the query, /api/* by path and
// everything else as "static". A request counts where it ended, finished or failed. A bare 304 answer to a fetch arrived
// as a failed request (seen in the first CI run: as many failures as polls), so the status comes from the response event:
// a failed 304 counts as an answered one; a request with no response, or aborted after its response began, counts as failed. The browser cannot
// size such an answer, so it adds no bytes. Transfer bytes are the browser's own (headers plus body as sent); decoded bytes are the
// body's length, read from the body when the response is compressed and equal to the sent body otherwise (the served
// responses are never compressed: reading the body back is unreliable for `Cache-Control: no-store` responses).
function recorder(ctx) {
  const state = { paths: {}, inflight: 0, lastActivity: Date.now(), pending: [], unread: 0 };
  const answered = new WeakMap();
  const mine = (request) => /^https?:/.test(request.url());
  const keyOf = (request) => { try { const p = new URL(request.url()).pathname; return p.startsWith("/api/") ? p : "static"; } catch { return "static"; } };
  const entry = (key) => (state.paths[key] ??= { count: 0, statuses: {}, failed: 0, errors: {}, streams: 0, transferBytes: 0, decodedBytes: 0 });
  // A server-sent event stream never finishes while the page is open: it is counted once, apart, and is neither in flight
  // nor activity, so it cannot keep idle() from seeing the network quiet.
  // Limits for the step that adds /api/events: (1) a stream's bytes are never counted, only its opening; (2) a stream opened
  // before rec.reset() is missing from every later row, so live's per-update bytes will fall because of how they are measured,
  // not because the viewer got cheaper; (3) a stream built with fetch() and a ReadableStream has resourceType "fetch", so
  // this exclusion does not apply and idle() would wait for it. Handle those in that step's brief.
  const streams = new WeakSet();
  ctx.on("request", (request) => {
    if (!mine(request)) return;
    if (request.resourceType() === "eventsource") { streams.add(request); entry(keyOf(request)).streams++; return; }
    state.inflight++; state.lastActivity = Date.now();
  });
  ctx.on("response", (response) => { answered.set(response.request(), response); });
  const ended = (request, failure) => {
    if (!mine(request) || streams.has(request)) return;
    state.inflight--; state.lastActivity = Date.now();
    const e = entry(keyOf(request)), response = answered.get(request);
    if (!response || (failure && response.status() !== 304)) { if (failure) e.errors[failure] = (e.errors[failure] ?? 0) + 1; e.failed++; return; }
    e.count++;
    const status = String(response.status());
    e.statuses[status] = (e.statuses[status] ?? 0) + 1;
    if (failure) return;
    state.pending.push((async () => {
      try {
        const sizes = await request.sizes();
        e.transferBytes += sizes.responseBodySize + sizes.responseHeadersSize;
        const encoding = response.headers()["content-encoding"] ?? "identity";
        e.decodedBytes += encoding === "identity" ? sizes.responseBodySize : (await response.body()).length;
      } catch { state.unread++; }
    })());
  };
  ctx.on("requestfinished", (request) => ended(request, null));
  ctx.on("requestfailed", (request) => ended(request, request.failure()?.errorText ?? "failed"));
  const settle = async () => { while (state.pending.length) { const batch = state.pending; state.pending = []; await Promise.all(batch); } };
  return {
    // Forgets what was recorded so far: the next snapshot covers what follows.
    async reset() { await settle(); state.paths = {}; state.unread = 0; },
    async snapshot() { await settle(); return { paths: JSON.parse(JSON.stringify(state.paths)), unreadBodies: state.unread }; },
    // True once nothing is in flight and nothing ended for `quiet` ms; false when `cap` ms pass first.
    async idle(quiet = 1000, cap = 20_000) {
      const start = Date.now();
      while (Date.now() - start < cap) {
        if (state.inflight <= 0 && Date.now() - state.lastActivity >= quiet) return true;
        await sleep(100);
      }
      return false;
    },
  };
}

// ---- The page's main thread -------------------------------------------------------------------------------------------------
// Installed before the page's own scripts. `first` is when a transcript entry was first drawn with a size, in ms from the
// start of the navigation. The frame loop only looks the entry up; it measures its box once it exists.
function installProbes() {
  const probe = { tasks: [], first: null, observer: null };
  window.__budget = probe;
  try {
    probe.observer = new PerformanceObserver((list) => { for (const e of list.getEntries()) probe.tasks.push({ start: e.startTime, duration: e.duration }); });
    probe.observer.observe({ type: "longtask", buffered: true });
  } catch { /* no long-task support: the counts stay zero */ }
  const look = () => {
    if (probe.first != null) return;
    const entry = document.querySelector("#page section[aria-label='Transcript'] .turns [data-e]");
    if (entry && entry.getBoundingClientRect().height > 0) { probe.first = performance.now(); return; }
    requestAnimationFrame(look);
  };
  requestAnimationFrame(look);
}

async function longTasks(page, since) {
  return page.evaluate((from) => {
    const probe = window.__budget;
    for (const e of probe.observer?.takeRecords() ?? []) probe.tasks.push({ start: e.startTime, duration: e.duration });
    const tasks = probe.tasks.filter((t) => t.start >= from);
    return {
      count: tasks.length,
      totalMs: tasks.reduce((n, t) => n + t.duration, 0),
      blockingMs: tasks.reduce((n, t) => n + Math.max(0, t.duration - 50), 0),
      longestMs: tasks.reduce((n, t) => Math.max(n, t.duration), 0),
    };
  }, since);
}

// ---- Rows -------------------------------------------------------------------------------------------------------------------
function summarize(paths, tasks, extra = {}) {
  const all = Object.values(paths), sum = (pick) => all.reduce((n, p) => n + pick(p), 0);
  const tx = paths["/api/tx"] ?? { count: 0, transferBytes: 0, decodedBytes: 0 };
  const metrics = {
    requests: sum((p) => p.count),
    notModified: sum((p) => p.statuses["304"] ?? 0),
    failedRequests: sum((p) => p.failed),
    transferBytes: sum((p) => p.transferBytes),
    decodedBytes: sum((p) => p.decodedBytes),
    txRequests: tx.count,
    txTransferBytes: tx.transferBytes,
    txDecodedBytes: tx.decodedBytes,
    longTasks: tasks.count,
    longTaskTotalMs: round(tasks.totalMs),
    blockingMs: round(tasks.blockingMs),
    longestTaskMs: round(tasks.longestMs),
    ...extra,
  };
  return metrics;
}

async function row(page, rec, since, extra = {}, notes = {}) {
  const snapshot = await rec.snapshot();
  const tasks = await longTasks(page, since);
  return { metrics: summarize(snapshot.paths, tasks, extra), paths: snapshot.paths, unreadBodies: snapshot.unreadBodies, pageErrors: [...page.errors], ...notes };
}

// ---- Scenarios --------------------------------------------------------------------------------------------------------------
async function open(browser, vp, run) {
  const ctx = await context(browser, { size: vp.size });
  try {
    await ctx.addInitScript(installProbes);
    const page = await ctx.newPage();
    page.setDefaultTimeout(60_000);
    page.setDefaultNavigationTimeout(60_000);
    page.errors = [];
    page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
    if (vp.throttle > 1) await (await ctx.newCDPSession(page)).send("Emulation.setCPUThrottlingRate", { rate: vp.throttle });
    return await run(page, recorder(ctx));
  } finally {
    await ctx.close().catch(() => {});
  }
}

const url = (server, where) => `${server.base}${where}${where.includes("?") ? "&" : "?"}t=${server.token}`;
const scrollTop = (page) => page.evaluate(() => { (matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main")).scrollTop = 0; });
const nowIn = (page) => page.evaluate(() => performance.now());
const softly = async (promise) => { try { await promise; return true; } catch { return false; } };

// The transcript's first entry is drawn, and the requests it made are done.
async function sessionOpened(page, rec) {
  const drawn = await softly(page.waitForFunction(() => window.__budget.first != null, null, { timeout: 60_000, polling: 100 }));
  const quiet = await rec.idle(1500, 30_000);
  return { drawn, quiet, firstEntryMs: round(await page.evaluate(() => window.__budget.first)) };
}

const scenarios = {
  "home-idle": (server, vp, browser) => open(browser, vp, async (page, rec) => {
    await page.goto(url(server, "/"), { waitUntil: "load" });
    await settled(page);
    await sleep(10_000);
    return { "home-idle": await row(page, rec, 0) };
  }),

  sessions: (server, vp, browser) => open(browser, vp, async (page, rec) => {
    await page.goto(url(server, "/sessions"), { waitUntil: "load" });
    await settled(page);
    const quiet = await rec.idle();
    return { sessions: await row(page, rec, 0, {}, { quiet }) };
  }),

  session: (server, vp, browser, fixture, warn) => open(browser, vp, async (page, rec) => {
    await page.goto(url(server, "/s/claude/marathon"), { waitUntil: "load" });
    const opened = await sessionOpened(page, rec);
    const out = { "session-open": await row(page, rec, 0, { firstEntryMs: opened.firstEntryMs }, opened) };

    // Older pages: the viewer offers "Load earlier" at the top of the transcript; press it until it is gone. This half has
    // its own catch: a failure here keeps the session-open row above.
    try {
      const since = await nowIn(page);
      await rec.reset();
      let presses = 0, stuck = false;
      for (; presses < 60; presses++) {
        await scrollTop(page);
        const button = page.locator(LOAD_EARLIER).first();
        if (!(await button.count())) break;
        const handle = await button.elementHandle();
        await handle.evaluate((b) => b.click());
        if (!(await softly(page.waitForFunction((b) => !b.isConnected, handle, { timeout: 30_000 })))) { stuck = true; break; }
      }
      const quiet = await rec.idle();
      // A press can fetch more than one /api/tx (the page and the child runs it shows), so presses and requests differ.
      // No press at all means the control was not found: the row measured nothing, and is listed as not settled.
      out["session-older"] = await row(page, rec, since, { olderPresses: presses }, { quiet, stuck, noOlder: presses === 0 });
    } catch (error) {
      warn("session-older", error);
    }
    return out;
  }),

  analytics: (server, vp, browser, fixture, warn) => open(browser, vp, async (page, rec) => {
    // Analytics draws all its figures at once (the tool-call counts come with the model), so the screen is done when they are
    // there: the page's own hook, else the classes the viewer drew before it had one.
    const counted = () => softly(page.waitForFunction(() => !!document.querySelector("[data-analytics-ready]")
      || (document.querySelectorAll(".analytics-metric").length === 8 && document.querySelectorAll(".analytics-chart svg").length === 2), null, { timeout: 30_000, polling: 100 }));
    await page.goto(url(server, "/analytics"), { waitUntil: "load" });
    await settled(page);
    const ready = await counted();
    const quiet = await rec.idle();
    const out = { "analytics-7d": await row(page, rec, 0, {}, { ready, quiet }) };

    // 30 d has its own catch: a failure here keeps the analytics-7d row above.
    try {
      const since = await nowIn(page);
      await rec.reset();
      const hooked = page.locator(RANGE_30);
      await (await hooked.count() ? hooked : page.locator(RANGE_BUTTONS).filter({ hasText: /^\s*30\s*d/i })).first().click();
      // Selected: aria-pressed or aria-selected true, or a class saying so. Kept as `selected`: 30 d fetches nothing, so a
      // click that changed nothing would otherwise look like a settled row.
      const selected = await softly(page.waitForFunction(({ hook, all }) => {
        const marked = [...document.querySelectorAll(hook)], candidates = marked.length ? marked : [...document.querySelectorAll(all)].filter((b) => /^\s*30\s*d/i.test(b.textContent));
        return candidates.some((b) => b.getAttribute("aria-pressed") === "true" || b.getAttribute("aria-selected") === "true" || ["on", "active", "selected", "is-active"].some((c) => b.classList.contains(c)));
      }, { hook: RANGE_30, all: RANGE_BUTTONS }, { timeout: 5000 }));
      const readyAfter = await counted();
      const quietAfter = await rec.idle();
      out["analytics-30d"] = await row(page, rec, since, {}, { ready: readyAfter, quiet: quietAfter, selected });
    } catch (error) {
      warn("analytics-30d", error);
    }
    return out;
  }),

  live: (server, vp, browser, fixture) => open(browser, vp, async (page, rec) => {
    await page.goto(url(server, "/s/claude/marathon"), { waitUntil: "load" });
    const opened = await sessionOpened(page, rec);
    const since = await nowIn(page);
    await rec.reset();
    const started = Date.now();
    for (let k = 1; k <= LIVE_BATCHES; k++) {
      await sleep(Math.max(0, started + (k - 1) * LIVE_EVERY_MS - Date.now()));
      fs.appendFileSync(fixture.marathonFile, liveRecords(fixture, k).map((r) => JSON.stringify(r) + "\n").join(""));
    }
    await sleep(Math.max(0, started + LIVE_WINDOW_MS - Date.now()));
    const quiet = await rec.idle(1000, 6000);
    const windowMs = Date.now() - started;
    const seen = await page.evaluate((marker) => document.body.textContent.includes(marker), `BUDGET_LIVE_${LIVE_BATCHES}_LAST`);
    const snapshot = await rec.snapshot();
    const updates = snapshot.paths["/api/model"]?.statuses["200"] ?? 0;
    const each = (n) => updates ? round(n / updates) : null;
    const tasks = await longTasks(page, since);
    const metrics = summarize(snapshot.paths, tasks, {
      updates,
      requestsPerUpdate: each(Object.values(snapshot.paths).reduce((n, p) => n + p.count, 0)),
      transferBytesPerUpdate: each(Object.values(snapshot.paths).reduce((n, p) => n + p.transferBytes, 0)),
      txRequestsPerUpdate: each(snapshot.paths["/api/tx"]?.count ?? 0),
      txDecodedBytesPerUpdate: each(snapshot.paths["/api/tx"]?.decodedBytes ?? 0),
    });
    return { live: { metrics, paths: snapshot.paths, unreadBodies: snapshot.unreadBodies, pageErrors: [...page.errors], batches: LIVE_BATCHES, appendedRecords: LIVE_BATCHES * 10, windowMs, quiet, lastUpdateSeen: seen, opened } };
  }),
};

// Ten records appended to marathon's log, in the shapes checks/live.mjs and checks/perf.mjs write: three tool calls with
// their results and a note each, then a closing message carrying the batch's marker. Their times are just before the
// server's pinned clock.
function liveRecords(fixture, batch) {
  const records = [];
  let seq = 0;
  const start = fixture.now - 6000 + (batch - 1) * 900;
  const base = (type, extra) => ({
    parentUuid: null, isSidechain: false, type, timestamp: new Date(start + records.length * 50).toISOString(),
    sessionId: "marathon", cwd: fixture.cwd, gitBranch: "main", version: "2.1.0", uuid: `u-budget-${batch}-${seq++}`, ...extra,
  });
  const said = (content) => records.push(base("assistant", { message: { id: `msg-budget-${batch}-${seq}`, model: "claude-opus-5-5", role: "assistant", type: "message", content } }));
  const calls = [["Bash", { command: `printf budget-${batch}-1` }], ["Read", { file_path: `src/budget-${batch}.txt` }], ["Grep", { pattern: `budget-pattern-${batch}` }]];
  calls.forEach(([name, input], i) => {
    const id = `toolu-budget-${batch}-${i + 1}`;
    said([{ type: "tool_use", id, name, input }]);
    records.push(base("user", { message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content: `Synthetic budget result ${batch}-${i + 1}.` }] } }));
    said([{ type: "text", text: `Synthetic budget note ${batch}-${i + 1}.` }]);
  });
  said([{ type: "text", text: `Final budget update: BUDGET_LIVE_${batch}_LAST` }]);
  return records;
}

// ---- The run ----------------------------------------------------------------------------------------------------------------
async function runSize(browser, vp, warnings) {
  // A fresh directory per viewport, made and removed here.
  const fixtureDir = fs.mkdtempSync(path.join(os.tmpdir(), "semon-budget-"));
  const errorText = (error) => String(error.stack ?? error).split("\n").slice(0, 4).join(" | ");
  const warn = (scenario, error) => warnings.push({ scenario: `${scenario}@${vp.width}`, error: errorText(error) });
  const rows = {};
  let server = null, fixture = null;
  try {
    fixture = writeLong(fixtureDir);
    fixture.cwd = path.join(fixtureDir, "work", "aster");
    server = await serve(fixtureDir, fixture.now);
    // The server's first read of the homes builds its index; that isn't what a screen is measured on.
    const warm = await fetch(`${server.base}/api/model?t=${server.token}`);
    if (!warm.ok) throw new Error(`/api/model answered ${warm.status}`);
    await warm.arrayBuffer();
    for (const [name, run] of Object.entries(scenarios)) {
      try {
        for (const [label, result] of Object.entries(await run(server, vp, browser, fixture, warn))) rows[`${label}@${vp.width}`] = { viewport: vp.width, cpuThrottle: vp.throttle, ...result };
      } catch (error) {
        // Report-only: that scenario's rows are missing, and the summary says why.
        warn(name, error);
      }
    }
  } finally {
    if (server) await stopServer(server.proc);
    fs.rmSync(fixtureDir, { recursive: true, force: true });
  }
  return { rows, counts: fixture?.counts ?? null };
}

// What the reader must not miss: a row whose screen did not settle (a figure not drawn, the network not quiet, no first
// entry, no older page, 30 d not selected) measured a stuck page or nothing, and a scenario that threw has no rows at all.
function unsettledOf(rows) {
  const flags = [["ready", (r) => r.ready === false], ["quiet", (r) => r.quiet === false], ["drawn", (r) => r.drawn === false], ["stuck", (r) => r.stuck === true],
    ["no older page loaded", (r) => r.noOlder === true], ["30 d not selected", (r) => r.selected === false]];
  return Object.entries(rows).map(([scenario, r]) => ({ scenario, why: flags.filter(([, hit]) => hit(r)).map(([label]) => label) })).filter((u) => u.why.length);
}
function attention(rows, warnings) {
  const lines = [], unsettled = unsettledOf(rows);
  if (warnings.length) lines.push("**Scenarios that failed (their rows are missing):**", "", ...warnings.map((w) => `- ${w.scenario}: ${w.error}`));
  if (unsettled.length) lines.push(...(lines.length ? [""] : []), `**Did not settle (read those rows with care):** ${unsettled.map((u) => `${u.scenario} (${u.why.join(", ")})`).join("; ")}.`);
  return lines;
}

function compare(rows, budget) {
  const over = [];
  for (const [name, result] of Object.entries(rows)) {
    const ceilings = budget.scenarios?.[name];
    if (!ceilings) { over.push({ scenario: name, note: "no budget" }); continue; }
    for (const [metric, ceiling] of Object.entries(ceilings)) {
      const value = result.metrics[metric];
      if (typeof value === "number" && typeof ceiling === "number" && value > ceiling) over.push({ scenario: name, metric, value, ceiling });
    }
  }
  return over;
}

function suggest(rows) {
  const scenarios = {};
  for (const [name, result] of Object.entries(rows)) {
    scenarios[name] = {};
    for (const metric of BUDGETED) {
      const value = result.metrics[metric];
      if (typeof value === "number") scenarios[name][metric] = Math.ceil(value * HEADROOM);
    }
  }
  return { note: "Ceilings per scenario and metric: what the check measured on main, plus 10%, rounded up. Report-only until a later step lowers them and makes the check fail.", scenarios };
}

const kb = (n) => n == null ? "-" : (n / 1024).toFixed(1);
function markdown(rows, over, warnings = []) {
  const notes = attention(rows, warnings);
  const lines = [
    "### Request and main-thread budget (report only)",
    "",
    ...(notes.length ? [...notes, ""] : []),
    "| Scenario | Requests | 304 | /api/tx req | /api/tx KB decoded | Transfer KB | Decoded KB | Long tasks | Blocking ms | Longest ms | First entry ms |",
    "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
  ];
  for (const [name, r] of Object.entries(rows)) {
    const m = r.metrics;
    lines.push(`| ${name} | ${m.requests} | ${m.notModified} | ${m.txRequests} | ${kb(m.txDecodedBytes)} | ${kb(m.transferBytes)} | ${kb(m.decodedBytes)} | ${m.longTasks} | ${m.blockingMs} | ${m.longestTaskMs} | ${m.firstEntryMs ?? "-"} |`);
  }
  const live = Object.entries(rows).filter(([name]) => name.startsWith("live@"));
  if (live.length) {
    lines.push("", "Live, per model update (30 s, ten records every 5 s):", "", "| Scenario | Updates | Requests | /api/tx req | /api/tx KB decoded | Transfer KB |", "|---|---:|---:|---:|---:|---:|");
    for (const [name, r] of live) {
      const m = r.metrics;
      lines.push(`| ${name} | ${m.updates} | ${m.requestsPerUpdate ?? "-"} | ${m.txRequestsPerUpdate ?? "-"} | ${kb(m.txDecodedBytesPerUpdate)} | ${kb(m.transferBytesPerUpdate)} |`);
    }
  }
  lines.push("", "By path:", "", "| Scenario | Path | Requests | Statuses | Transfer KB | Decoded KB |", "|---|---|---:|---|---:|---:|");
  for (const [name, r] of Object.entries(rows)) {
    for (const [p, e] of Object.entries(r.paths).sort((a, b) => b[1].decodedBytes - a[1].decodedBytes)) {
      lines.push(`| ${name} | ${p} | ${e.count}${e.failed ? ` (+${e.failed} failed)` : ""} | ${Object.entries(e.statuses).map(([s, n]) => `${s}x${n}`).concat(Object.entries(e.errors).map(([t, n]) => `${t}x${n}`), e.streams ? [`stream x${e.streams}`] : []).join(" ")} | ${kb(e.transferBytes)} | ${kb(e.decodedBytes)} |`);
    }
  }
  const overLines = over.filter((o) => o.metric);
  lines.push("", overLines.length ? "Over budget:" : "Over budget: none.", ...overLines.map((o) => `- ${o.scenario} ${o.metric}: ${o.value} > ${o.ceiling}`));
  const unbudgeted = over.filter((o) => o.note);
  if (unbudgeted.length) lines.push("", `No budget yet for: ${unbudgeted.map((o) => o.scenario).join(", ")}.`);
  return lines.join("\n");
}

async function main() {
  fs.mkdirSync(ENV.out, { recursive: true });
  const failures = [], warnings = [], rows = {};
  let counts = null, browser;
  try {
    browser = await launch();
    for (const vp of SIZES) {
      try {
        const ran = await runSize(browser, vp, warnings);
        Object.assign(rows, ran.rows);
        counts = ran.counts ?? counts;
      } catch (error) {
        failures.push(`${vp.width}: ${error.stack ?? error}`); // no server or fixture for this size: nothing to measure
      }
    }
  } catch (error) {
    failures.push(error.stack ?? String(error));
  } finally {
    try { await browser?.close(); } catch { /* already gone */ }
  }

  let budget = { scenarios: {} };
  try { budget = JSON.parse(fs.readFileSync(BUDGET_FILE, "utf8")); } catch (error) { console.warn(`no usable ${BUDGET_FILE}: ${error.message}`); }
  const over = compare(rows, budget);
  fs.writeFileSync(path.join(ENV.out, "budget.json"), JSON.stringify({ fixture: counts, rows, over, warnings, failures }, null, 2) + "\n");
  fs.writeFileSync(path.join(ENV.out, "perf-budget.suggested.json"), JSON.stringify(suggest(rows), null, 2) + "\n");
  const table = markdown(rows, over, warnings);
  console.log(table);
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `\n${table}\n`);
  for (const o of over.filter((x) => x.metric)) console.log(`OVER BUDGET ${o.scenario} ${o.metric}: ${o.value} > ${o.ceiling}`);
  for (const [name, r] of Object.entries(rows)) for (const e of r.pageErrors ?? []) console.warn(`page error in ${name}: ${e}`);
  for (const w of warnings) console.warn(`WARN ${w.scenario} failed: ${w.error}`);
  // On GitHub Actions, one annotation per problem, so it shows on the PR's checks page and not only in the step's log.
  if (process.env.GITHUB_ACTIONS) {
    const annotate = (text) => console.log(`::warning title=budget::${text.replace(/%/g, "%25").replace(/\r/g, "%0D").replace(/\n/g, "%0A")}`);
    for (const w of warnings) annotate(`${w.scenario} failed, its rows are missing: ${w.error}`);
    for (const u of unsettledOf(rows)) annotate(`${u.scenario} did not settle: ${u.why.join(", ")}`);
  }
  for (const failure of failures) console.error(`FAIL ${failure}`);
  if (failures.length || !Object.keys(rows).length) process.exitCode = 1;
}

main().catch((error) => {
  console.error(error.stack ?? String(error));
  process.exitCode = 1;
});

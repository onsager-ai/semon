// Long-session performance baseline (#29 item 0). A `semon sessions --serve` process reads the generated homes;
// Chromium measures cold open, session switches, a live tail, scrolling, DOM size and request costs at phone and desktop
// sizes with 4× CPU throttling. Each browser metric is sampled three times and summarized by its median; values are
// reported only, while page errors and missing paint markers fail the check.
//
//   SEMON_BIN     the semon binary built with the test clock (default: target/debug/semon)
//   SEMON_UI_OUT  where perf.json is written (default: ./out)
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { context, ENV, launch } from "../lib.mjs";
import { writeLong } from "../long.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const BIN = process.env.SEMON_BIN ?? path.resolve(here, "../../../target/debug/semon");
const BUCKETS = ["/api/model", "/api/tx", "other"];
const SCREENS = [
  { name: "phone", size: "phone" },
  { name: "desktop", size: "desktop" },
];
const PAINT_MARKERS = { marathon: "PERF_MARATHON_LAST", relay: "PERF_RELAY_LAST" };
const median = (values) => {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const round = (value) => value == null ? null : Math.round(value * 10) / 10;
const sessionUrl = (base, token, id) => `${base}/s/claude/${encodeURIComponent(id)}?t=${token}`;

function bucket(url) {
  try {
    const pathname = new URL(url).pathname;
    return pathname === "/api/model" || pathname === "/api/tx" ? pathname : "other";
  } catch {
    return "other";
  }
}

function serve(dir, now) {
  return new Promise((resolve, reject) => {
    const proc = spawn(BIN, [
      "sessions", "--serve", "--listen", "127.0.0.1:0",
      "--claude-home", path.join(dir, "claude"),
      "--codex-home", path.join(dir, "codex"),
      "--proc-root", path.join(dir, "proc"),
      "--cache", path.join(dir, "index.json"),
    ], { env: { ...process.env, SEMON_TEST_NOW: String(now) }, stdio: ["ignore", "pipe", "pipe"] });
    let log = "";
    let settled = false;
    const fail = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(error);
    };
    const read = (chunk) => {
      log += chunk;
      const match = /http:\/\/127\.0\.0\.1:(\d+)\/\?t=([0-9a-f]+)/.exec(log);
      if (!match || settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ proc, base: `http://127.0.0.1:${match[1]}`, token: match[2], log: () => log });
    };
    const timer = setTimeout(() => fail(new Error(`semon printed no URL within 20 s: ${log}`)), 20_000);
    proc.stdout.on("data", read);
    proc.stderr.on("data", read);
    proc.on("error", (error) => fail(error));
    proc.on("exit", (code) => fail(new Error(`semon exited ${code} before printing its URL: ${log}`)));
  });
}

async function stopServer(proc) {
  if (!proc || proc.exitCode !== null) return;
  proc.kill("SIGTERM");
  await new Promise((resolve) => {
    const timer = setTimeout(() => {
      proc.kill("SIGKILL");
      resolve();
    }, 3000);
    proc.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function waitPaint(page, marker) {
  await page.waitForFunction((expected) => {
    const messages = document.querySelectorAll('#page section[aria-label="Transcript"] .msg.assistant');
    const last = messages[messages.length - 1];
    if (!last || !last.textContent.includes(expected)) return false;
    if (window.__perfPaintWait?.marker !== expected) {
      const state = { marker: expected, node: last, done: false };
      window.__perfPaintWait = state;
      requestAnimationFrame(() => requestAnimationFrame(() => {
        if (state.node.isConnected && state.node.textContent.includes(expected)) state.done = true;
      }));
    }
    return window.__perfPaintWait?.done === true;
  }, marker, { timeout: 60_000, polling: "raf" });
}

async function checkLongSessionOpenEnd(page) {
  const opened = await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    await new Promise((resolve) => setTimeout(resolve, 500));
    const sc = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main");
    return { top: sc.scrollTop, gap: sc.scrollHeight - sc.scrollTop - sc.clientHeight, jumpHidden: document.querySelector(".jump-bottom")?.hidden ?? true };
  });
  const main = await page.locator("#main").boundingBox();
  await page.mouse.move(main ? main.x + Math.min(main.width / 2, 200) : 195, 300);
  await page.mouse.wheel(0, await page.evaluate(() => innerHeight * 2));
  await page.waitForFunction(() => {
    const button = document.querySelector(".jump-bottom"), rect = button?.getBoundingClientRect();
    return !!button && !button.hidden && !!rect && rect.width >= 40 && rect.height >= 40;
  });
  const raised = await page.evaluate(() => {
    const button = document.querySelector(".jump-bottom"), rect = button.getBoundingClientRect(), bar = document.querySelector("#topbar").getBoundingClientRect();
    const sc = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main");
    const composer = document.querySelector("#composer, .composer, [data-composer]")?.getBoundingClientRect();
    const overlaps = (a, b) => !!a && !!b && a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
    return {
      top: sc.scrollTop,
      gap: sc.scrollHeight - sc.scrollTop - sc.clientHeight,
      inside: rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight,
      overlapsBar: overlaps(rect, bar),
      overlapsComposer: overlaps(rect, composer),
    };
  });
  await page.locator(".jump-bottom").click();
  await page.waitForFunction(() => {
    const sc = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main");
    return sc.scrollHeight - sc.scrollTop - sc.clientHeight <= 1;
  }, null, { timeout: 10_000 });
  const returnedGap = await page.evaluate(() => {
    const sc = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main");
    return sc.scrollHeight - sc.scrollTop - sc.clientHeight;
  });
  return {
    openedGap: opened.gap,
    jumpHiddenAtOpen: opened.jumpHidden,
    scrollUpDistance: opened.top - raised.top,
    raisedGap: raised.gap,
    jumpInsideViewport: raised.inside,
    overlapsBar: raised.overlapsBar,
    overlapsComposer: raised.overlapsComposer,
    returnedGap,
    ok: opened.gap <= 1 && opened.jumpHidden && opened.top - raised.top >= 2 * (await page.evaluate(() => innerHeight)) - 1 && raised.gap > 80 && raised.inside && !raised.overlapsBar && !raised.overlapsComposer && returnedGap <= 1,
  };
}

function liveRecords({ sid, cwd, branch, sample, now }) {
  const records = [];
  let seq = 0;
  const base = (time, type, extra = {}) => ({
    parentUuid: null,
    isSidechain: false,
    type,
    timestamp: new Date(time).toISOString(),
    sessionId: sid,
    cwd,
    gitBranch: branch,
    version: "2.1.0",
    uuid: `u-perf-${sample}-${seq++}`,
    ...extra,
  });
  const said = (time, blocks) => base(time, "assistant", {
    message: { id: `msg-perf-${sample}-${seq}`, model: "claude-opus-5-5", role: "assistant", type: "message", content: blocks },
  });
  const start = now - 30_000 + (sample - 1) * 10_000;
  const time = (i) => start + i * 100;
  const names = ["Bash", "Read", "Grep"];
  for (let i = 0; i < names.length; i++) {
    const id = `toolu-perf-${sample}-${i + 1}`;
    const name = names[i];
    const input = name === "Bash" ? { command: `printf live-sample-${sample}-${i + 1}` }
      : name === "Read" ? { file_path: `src/live-${sample}.txt` }
        : { pattern: `live-pattern-${sample}-${i + 1}` };
    records.push(said(time(records.length), [{ type: "tool_use", id, name, input }]));
    records.push(base(time(records.length), "user", {
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content: `Synthetic live result ${sample}-${i + 1}.` }] },
    }));
    records.push(said(time(records.length), [{ type: "text", text: `Synthetic live assistant note ${sample}-${i + 1}.` }]));
  }
  records.push(said(time(records.length), [{ type: "text", text: `Final live update: PERF_LIVE_${sample}_LAST` }]));
  if (records.length !== 10) throw new Error(`live sample ${sample} has ${records.length} JSONL records instead of 10`);
  return records;
}

async function runScreen(browser, screen, server, fixture) {
  const errors = [];
  const failures = [];
  const network = {
    counts: Object.fromEntries(BUCKETS.map((key) => [key, 0])),
    bytes: Object.fromEntries(BUCKETS.map((key) => [key, 0])),
    ttfb: Object.fromEntries(BUCKETS.map((key) => [key, []])),
  };
  const managedPages = new Set();
  const metrics = {
    coldOpen: { samplesMs: [], medianMs: null },
    openAtEnd: null,
    switch: {
      marathonToRelay: { samplesMs: [], medianMs: null },
      relayToMarathon: { samplesMs: [], medianMs: null },
    },
    liveUpdate: { samples: [], medianPaintMs: null, medianLongestTaskMs: null },
    scroll: { samples: [], medianFramesOver20Ms: null, medianLongestFrameMs: null },
    domSize: { samples: [], medianElements: null },
  };

  let ctx;
  try {
    ctx = await context(browser, { size: screen.size });
    const makePage = async () => {
      const page = await ctx.newPage();
      page.setDefaultTimeout(60_000);
      page.setDefaultNavigationTimeout(60_000);
      page.on("pageerror", (error) => errors.push(error.message.split("\n")[0]));
      await page.addInitScript(() => {
        window.__perfLongTasks = [];
        window.__perfLongTaskObserver = null;
        window.__perfResourceTimings = [];
        window.__perfTimingObservers = [];
        try { performance.setResourceTimingBufferSize(10000); } catch {}
        try {
          const observer = new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) window.__perfLongTasks.push({ startTime: entry.startTime, duration: entry.duration });
          });
          observer.observe({ type: "longtask", buffered: true });
          window.__perfLongTaskObserver = observer;
        } catch {}
        for (const type of ["resource", "navigation"]) {
          try {
            const observer = new PerformanceObserver((list) => {
              for (const entry of list.getEntries()) window.__perfResourceTimings.push({
                name: entry.name,
                requestStart: entry.requestStart,
                responseStart: entry.responseStart,
              });
            });
            observer.observe({ type, buffered: true });
            window.__perfTimingObservers.push(observer);
          } catch {}
        }
      });
      const cdp = await ctx.newCDPSession(page);
      const byRequest = new Map();
      cdp.on("Network.requestWillBeSent", (event) => {
        if (event.redirectResponse) {
          const previous = byRequest.get(event.requestId);
          if (previous) network.bytes[previous] += Math.max(0, Number(event.redirectResponse.encodedDataLength ?? 0));
        }
        const key = bucket(event.request.url);
        network.counts[key]++;
        byRequest.set(event.requestId, key);
      });
      cdp.on("Network.loadingFinished", (event) => {
        const key = byRequest.get(event.requestId);
        if (key) network.bytes[key] += Math.max(0, Number(event.encodedDataLength ?? 0));
        byRequest.delete(event.requestId);
      });
      cdp.on("Network.loadingFailed", (event) => byRequest.delete(event.requestId));
      await cdp.send("Network.enable");
      await cdp.send("Network.setCacheDisabled", { cacheDisabled: true });
      await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
      const managed = { page, cdp, byRequest };
      managedPages.add(managed);
      return managed;
    };

    const readResourceTiming = async (page) => {
      const entries = await page.evaluate(() => [
        ...performance.getEntriesByType("navigation"),
        ...performance.getEntriesByType("resource"),
        ...window.__perfResourceTimings,
        ...window.__perfTimingObservers.flatMap((observer) => observer.takeRecords()),
      ].map((entry) => ({
        url: entry.name,
        requestStart: Number(entry.requestStart),
        responseStart: Number(entry.responseStart),
      })));
      const unique = new Set();
      for (const entry of entries) {
        const value = entry.responseStart - entry.requestStart;
        const key = `${entry.url}\n${entry.requestStart}\n${entry.responseStart}`;
        if (unique.has(key) || !Number.isFinite(value) || value < 0) continue;
        unique.add(key);
        network.ttfb[bucket(entry.url)].push(value);
      }
    };
    const finishPage = async (managed) => {
      if (!managedPages.has(managed)) return;
      try { await readResourceTiming(managed.page); }
      catch (error) { failures.push(`Resource Timing collection failed: ${error.message}`); }
      managedPages.delete(managed);
      try { await managed.cdp.detach(); } catch {}
      try { await managed.page.close(); } catch {}
    };
    const openMarathon = async (managed) => {
      const started = Date.now();
      await managed.page.goto(sessionUrl(server.base, server.token, "marathon"), { waitUntil: "load" });
      await waitPaint(managed.page, PAINT_MARKERS.marathon);
      return Date.now() - started;
    };
    const prepareSidebarRow = async (page, id) => {
      await page.waitForSelector(`.srow[data-id="${id}"]`, { state: "attached", timeout: 60_000 });
      const mobile = await page.evaluate(() => matchMedia("(max-width: 760px)").matches);
      if (mobile && !(await page.evaluate(() => document.body.classList.contains("drawer-open")))) {
        await page.locator("#lead-btn").click();
      }
      await page.waitForFunction((selector) => {
        const row = document.querySelector(selector);
        return !!row && row.getBoundingClientRect().width > 0 && row.getBoundingClientRect().height > 0;
      }, `.srow[data-id="${id}"]`, { timeout: 60_000 });
    };
    const clickAndPaint = async (page, id) => {
      await prepareSidebarRow(page, id);
      await page.evaluate(() => { window.__perfPaintWait = null; });
      const started = await page.evaluate(() => performance.now());
      await page.locator(`.srow[data-id="${id}"]`).click();
      await waitPaint(page, PAINT_MARKERS[id]);
      return await page.evaluate((start) => performance.now() - start, started);
    };

    let switchPage = null;
    for (let i = 0; i < 3; i++) {
      const managed = await makePage();
      metrics.coldOpen.samplesMs.push(await openMarathon(managed));
      if (i === 0) {
        metrics.openAtEnd = await checkLongSessionOpenEnd(managed.page);
        if (!metrics.openAtEnd.ok) failures.push(`long-session end pin/jump assertions failed: ${JSON.stringify(metrics.openAtEnd)}`);
      }
      metrics.domSize.samples.push(await managed.page.evaluate(() => document.getElementsByTagName("*").length));
      if (i === 2) switchPage = managed;
      else await finishPage(managed);
    }

    for (let i = 0; i < 3; i++) {
      metrics.switch.marathonToRelay.samplesMs.push(await clickAndPaint(switchPage.page, "relay"));
      metrics.switch.relayToMarathon.samplesMs.push(await clickAndPaint(switchPage.page, "marathon"));
    }

    for (let i = 1; i <= 3; i++) {
      const page = switchPage.page;
      await page.evaluate(() => { window.__perfPaintWait = null; });
      const started = await page.evaluate(() => performance.now());
      fs.appendFileSync(fixture.marathonFile, liveRecords({ sid: "marathon", cwd: fixture.cwd, branch: "main", sample: i, now: fixture.now })
        .map((record) => JSON.stringify(record) + "\n").join(""));
      await waitPaint(page, `PERF_LIVE_${i}_LAST`);
      const measured = await page.evaluate((start) => {
        const end = performance.now();
        const observer = window.__perfLongTaskObserver;
        if (observer) {
          for (const entry of observer.takeRecords()) window.__perfLongTasks.push({ startTime: entry.startTime, duration: entry.duration });
        }
        const tasks = window.__perfLongTasks.filter((entry) => entry.startTime < end && entry.startTime + entry.duration > start);
        return {
          paintMs: end - start,
          longestTaskMs: tasks.length ? Math.max(...tasks.map((entry) => entry.duration)) : 0,
          appendedJsonlEntries: 10,
        };
      }, started);
      metrics.liveUpdate.samples.push({ paintMs: round(measured.paintMs), longestTaskMs: round(measured.longestTaskMs), appendedJsonlEntries: measured.appendedJsonlEntries });
    }

    for (let i = 0; i < 3; i++) {
      metrics.scroll.samples.push(await switchPage.page.evaluate(() => new Promise((resolve) => {
        const scroller = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main");
        const bottom = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
        scroller.scrollTop = bottom;
        let start = null;
        let previous = null;
        let framesOver20Ms = 0;
        let longestFrameMs = 0;
        const step = (time) => {
          if (start == null) { start = time; previous = time; requestAnimationFrame(step); return; }
          const delta = time - previous;
          previous = time;
          if (delta > 20) framesOver20Ms++;
          longestFrameMs = Math.max(longestFrameMs, delta);
          const elapsed = time - start;
          scroller.scrollTop = bottom * (1 - Math.min(1, elapsed / 3000));
          if (elapsed >= 3000) {
            scroller.scrollTop = 0;
            resolve({ framesOver20Ms, longestFrameMs: Math.round(longestFrameMs * 10) / 10 });
          } else requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
      })));
    }

    await finishPage(switchPage);
  } catch (error) {
    failures.push(error.stack ?? String(error));
  } finally {
    for (const managed of [...managedPages]) {
      try { await managed.cdp.detach(); } catch {}
      try { await managed.page.close(); } catch {}
    }
    try { await ctx?.close(); } catch {}
  }

  metrics.coldOpen.medianMs = round(median(metrics.coldOpen.samplesMs));
  metrics.domSize.medianElements = median(metrics.domSize.samples);
  metrics.switch.marathonToRelay.medianMs = round(median(metrics.switch.marathonToRelay.samplesMs));
  metrics.switch.relayToMarathon.medianMs = round(median(metrics.switch.relayToMarathon.samplesMs));
  metrics.liveUpdate.medianPaintMs = round(median(metrics.liveUpdate.samples.map((sample) => sample.paintMs)));
  metrics.liveUpdate.medianLongestTaskMs = round(median(metrics.liveUpdate.samples.map((sample) => sample.longestTaskMs)));
  metrics.scroll.medianFramesOver20Ms = median(metrics.scroll.samples.map((sample) => sample.framesOver20Ms));
  metrics.scroll.medianLongestFrameMs = round(median(metrics.scroll.samples.map((sample) => sample.longestFrameMs)));

  const requests = Object.fromEntries(BUCKETS.map((key) => [key, {
    count: network.counts[key],
    encodedBytes: network.bytes[key],
    ttfbMs: {
      samples: network.ttfb[key].length,
      median: round(median(network.ttfb[key])),
      max: round(network.ttfb[key].length ? Math.max(...network.ttfb[key]) : null),
    },
  }]));
  return { metrics, requests, pageErrors: errors, failures };
}

function markdownTable(screens) {
  const number = (value) => value == null ? "—" : String(round(value));
  const request = (entry) => `${entry.count} / ${entry.encodedBytes} B / ${number(entry.ttfbMs.median)} / ${number(entry.ttfbMs.max)} ms`;
  const rows = [
    "| Screen | Cold open ms | Switch marathon→relay ms | Switch relay→marathon ms | Live paint ms | Live task ms | Scroll frames >20 ms | Longest frame ms | DOM elements | `/api/model`: req / bytes / TTFB median / max | `/api/tx`: req / bytes / TTFB median / max | Other: req / bytes / TTFB median / max |",
    "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
  ];
  for (const name of ["phone", "desktop"]) {
    const screen = screens[name];
    const m = screen?.metrics;
    const r = screen?.requests ?? Object.fromEntries(BUCKETS.map((key) => [key, { count: 0, encodedBytes: 0, ttfbMs: { median: null, max: null } }]));
    rows.push(`| ${name} | ${number(m?.coldOpen.medianMs)} | ${number(m?.switch.marathonToRelay.medianMs)} | ${number(m?.switch.relayToMarathon.medianMs)} | ${number(m?.liveUpdate.medianPaintMs)} | ${number(m?.liveUpdate.medianLongestTaskMs)} | ${number(m?.scroll.medianFramesOver20Ms)} | ${number(m?.scroll.medianLongestFrameMs)} | ${number(m?.domSize.medianElements)} | ${request(r["/api/model"])} | ${request(r["/api/tx"])} | ${request(r.other)} |`);
  }
  return rows.join("\n");
}

async function main() {
  fs.mkdirSync(ENV.out, { recursive: true });
  const fixtureDir = path.join(os.tmpdir(), "semon-long-perf-fixture");
  let browser;
  const failures = [];
  const screens = {};
  let fixtureSummary;
  try {
    browser = await launch();
    for (const screen of SCREENS) {
      fs.rmSync(fixtureDir, { recursive: true, force: true });
      fs.mkdirSync(fixtureDir, { recursive: true });
      const fixture = writeLong(fixtureDir);
      fixture.cwd = path.join(fixtureDir, "work", "aster");
      fixtureSummary = { now: fixture.now, counts: fixture.counts };
      const server = await serve(fixtureDir, fixture.now);
      try {
        screens[screen.name] = await runScreen(browser, screen, server, fixture);
      } finally {
        await stopServer(server.proc);
      }
      for (const error of screens[screen.name].pageErrors) failures.push(`${screen.name} page error: ${error}`);
      for (const failure of screens[screen.name].failures) failures.push(`${screen.name}: ${failure}`);
    }
  } catch (error) {
    failures.push(error.stack ?? String(error));
  } finally {
    try { await browser?.close(); } catch {}
    fs.rmSync(fixtureDir, { recursive: true, force: true });
  }

  const report = {
    fixture: fixtureSummary ?? null,
    cpuThrottlingRate: 4,
    screens,
    failures,
  };
  fs.writeFileSync(path.join(ENV.out, "perf.json"), JSON.stringify(report, null, 2) + "\n");
  const table = markdownTable(screens);
  console.log(table);
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `\n${table}\n`);
  for (const failure of failures) console.error(`FAIL ${failure}`);
  if (failures.length) process.exitCode = 1;
}

main().catch((error) => {
  console.error(error.stack ?? String(error));
  process.exitCode = 1;
});

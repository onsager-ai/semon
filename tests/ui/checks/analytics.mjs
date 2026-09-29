// Analytics: range, eight headline figures, stacked column charts, slice drill-in, breakdown filtering, cost measure,
// phone width and the served Codex allowance. This replaces the retired Timeline check. The figures come from the server
// (/api/analytics), which reaches past the model's day: on the extras fixture, a session from five days ago (`archive`,
// absent from /api/model) counts in 7 d and 30 d and not in 24 h. Every range is shot at 390 and 1280, light and dark, into
// out/analytics/. A request that fails (500, 409) keeps the last answer drawn under a message, asks again only every 10 s
// while it fails, and a 304 or 200 clears it.
import fs from "node:fs";
import path from "node:path";
import { served, data, reporter, overflow, goto, settled, ENV } from "../lib.mjs";

// The drawn answer's range and filters (data-query on the figures), once it is drawn.
const drawn = (page, query) => page.waitForFunction((q) => document.querySelector(".analytics-metrics[data-analytics-ready]")?.dataset.query === q, query);

export default async function analyticsCheck(browser) {
  const D = await data(), r = reporter("analytics"), modes = [];
  const expectedLabels = ["Agent-hours", "Cost, last 7 UTC days", "Sessions started", "Turns", "Tool calls", "Peak concurrency", "Waited on you", "Longest current wait"];
  const expectedHarborIds = Object.values(D.SESS).filter((s) => s.repo === "harbor").map((s) => s.id).sort();
  r.expect(!!D.SESS.deps?.rate_limits, "the fixture must serve Codex rate limits for the allowance panel");

  for (const [mode, size, dark] of [["phone", "phone", false], ["desktop", "desktop", false]]) {
    const page = await served(browser, { size, dark, path: "/analytics" });
    await goto(page, { v: "analytics" }, D);
    const record = { mode };
    record.range = await page.evaluate(() => ({ labels: [...document.querySelectorAll("#topbar .analytics-range button")].map((b) => b.textContent.trim()), selected: document.querySelector('#topbar .analytics-range button[aria-pressed="true"]')?.textContent.trim(), heading: document.querySelector(".page .sub")?.textContent }));
    await page.click('#topbar .analytics-range button:has-text("30 d")');
    await drawn(page, "range=30d");
    record.range30 = await page.evaluate(() => ({ selected: document.querySelector('#topbar .analytics-range button[aria-pressed="true"]')?.textContent.trim(), heading: document.querySelector(".page .sub")?.textContent }));
    await page.click('#topbar .analytics-range button:has-text("7 d")');
    await drawn(page, "range=7d");
    record.labels = await page.evaluate(() => [...document.querySelectorAll(".analytics-metric .label")].map((x) => x.textContent.trim()));
    record.figures = await page.locator(".analytics-metric").count();
    record.longestCurrentWait = await page.locator('.analytics-metric').filter({ hasText: "Longest current wait" }).locator(".value").textContent();
    // Cost is served per UTC day: 7 d counts the last 7 UTC days, today so far included.
    const DAY = 86400000, today = Math.floor(D.NOW / DAY) * DAY, costFrom = today - 6 * DAY, costTo = today + DAY, missingModels = new Set(); let expectedUsd = 0;
    for (const session of Object.values(D.SESS)) {
      const days = Object.entries(session.cost?.by_day ?? {}).filter(([day]) => { const start = Date.parse(day + "T00:00:00.000Z"); return start >= costFrom && start + DAY <= costTo; });
      for (const [, amount] of days) expectedUsd += Number(amount) || 0;
      if (days.length) for (const model of session.cost?.unpriced_models ?? []) missingModels.add(model);
    }
    record.costExpected = missingModels.size ? "—" : "$" + expectedUsd.toFixed(2);
    record.costHeadline = await page.locator('.analytics-metric').filter({ hasText: "Cost, last 7 UTC days" }).locator(".value").textContent();
    record.charts = await page.evaluate(() => [...document.querySelectorAll(".analytics-chart svg")].map((svg) => ({ label: svg.getAttribute("aria-label"), columns: [...svg.querySelectorAll("rect.cost-claude, rect.cost-codex")].map((x) => ({ width: Number(x.getAttribute("width")), height: Number(x.getAttribute("height")) })).filter((x) => x.width > 0 && x.height > 0).length })));
    record.allowance = await page.evaluate(() => ({ heading: [...document.querySelectorAll(".analytics-panel h2")].find((x) => x.textContent === "Codex allowance")?.textContent, windows: [...document.querySelectorAll(".allowance-window .window-name")].map((x) => x.textContent.trim()), used: [...document.querySelectorAll(".allowance-window .window-used")].map((x) => x.textContent.trim()) }));
    record.sideways = size === "phone" ? await overflow(page) : 0;

    await page.click('.analytics-measure button:has-text("API-equivalent cost")');
    record.costMode = await page.evaluate(() => ({ pressed: document.querySelector('.analytics-measure button[aria-pressed="true"]')?.textContent.trim(), costRows: document.querySelectorAll(".analytics-row .row-cost.on").length, hoursRows: document.querySelectorAll(".analytics-row .row-hours.on").length }));
    await page.click('.analytics-measure button:has-text("Agent-hours")');

    const hit = page.locator(".analytics-panel .chart-hit[role=button]").first();
    if (await hit.count()) await hit.click();
    await page.waitForSelector("dialog.analytics-slice[open]");
    record.slice = await page.evaluate(() => { const d = document.querySelector("dialog.analytics-slice"), rect = d.getBoundingClientRect(); return { open: d.open, sessions: d.querySelectorAll(".analytics-session").length, title: d.getAttribute("aria-label"), bottomSheet: matchMedia("(max-width: 760px)").matches ? rect.bottom >= innerHeight - 1 && rect.width >= innerWidth - 1 : null }; });
    await page.click('dialog.analytics-slice button[aria-label="Close sessions list"]');

    const repoRow = page.locator('.analytics-row[data-breakdown="repo"][data-key="harbor"]');
    record.repoRow = await repoRow.count();
    if (record.repoRow) await repoRow.click();
    await page.waitForFunction(() => history.state?.v === "sessions");
    record.filtered = await page.evaluate(() => ({ title: document.querySelector(".page h1")?.textContent, rows: [...document.querySelectorAll(".page .nrow")].map((x) => x.dataset.id).sort() }));
    modes.push({ ...record, errors: page.errors });
    await page.context().close();
  }

  // Cost is recorded per UTC day: 24 h shows today's UTC day and no hourly series; 7 d has a column per UTC day. (Which days
  // each range counts, and the previous period's, is the server's: crates/semon-sessions/src/analytics.rs tests it.)
  const fx = await served(browser, { size: "desktop", path: "/analytics" });
  await drawn(fx, "range=7d");
  const costPanel = ".analytics-panel:has(h2:has-text('Cost over time'))";
  const costTwoDays = {};
  await fx.click('#topbar .analytics-range button:has-text("24 h")');
  await drawn(fx, "range=24h");
  costTwoDays.day = { label: (await fx.locator(".analytics-metric .label").allTextContents()).map((x) => x.trim()).find((x) => x.startsWith("Cost")), charts: await fx.locator(".analytics-chart svg").count(), costChart: await fx.locator(costPanel + " .analytics-chart").count(), note: await fx.locator(costPanel + " p.empty").count() };
  await fx.click('#topbar .analytics-range button:has-text("7 d")');
  await drawn(fx, "range=7d");
  costTwoDays.week = { label: (await fx.locator(".analytics-metric .label").allTextContents()).map((x) => x.trim()).find((x) => x.startsWith("Cost")), chart: await fx.locator(costPanel + " .panel-sub").textContent(), hits: await fx.locator(costPanel + " rect.chart-hit").count() };
  costTwoDays.errors = fx.errors;
  await fx.context().close();
  r.expect(costTwoDays.day.label === "Cost today (UTC)" && costTwoDays.day.charts === 1 && costTwoDays.day.costChart === 0 && costTwoDays.day.note === 1, "the 24 h range must count today's UTC day and draw no hourly cost series: " + JSON.stringify(costTwoDays.day));
  r.expect(costTwoDays.week.label === "Cost, last 7 UTC days" && costTwoDays.week.chart.includes("today so far") && costTwoDays.week.hits === 7, "7 d must draw a cost column per UTC day: " + JSON.stringify(costTwoDays.week));
  r.expect(costTwoDays.errors.length === 0, "cost: page errors: " + costTwoDays.errors.join(" | "));

  // A session older than the model's day: `archive`, five days before the extras fixture's now, in its own repo. The model
  // leaves it out; with the Repo filter on archive, Sessions started is 1 in 7 d and 30 d and 0 in 24 h, and its repo is a
  // breakdown row in 7 d and 30 d only.
  const older = { ranges: {} };
  const ox = await served(browser, { extras: true, size: "desktop", path: "/analytics" });
  await drawn(ox, "range=7d");
  older.inModel = await ox.evaluate(async () => Object.keys((await (await fetch("/api/model")).json()).sessions).includes("archive"));
  const archiveRow = '.analytics-row[data-breakdown="repo"][data-key="archive"]';
  older.rowWithoutFilter = await ox.locator(archiveRow).count();
  older.option = await ox.evaluate(() => [...document.querySelectorAll('.facet-filters select[aria-label="Repo"] option')].map((o) => o.value).includes("archive"));
  if (older.option) {
    await ox.selectOption('.facet-filters select[aria-label="Repo"]', "archive");
    for (const [label, range] of [["7 d", "7d"], ["30 d", "30d"], ["24 h", "24h"]]) {
      await ox.click('#topbar .analytics-range button:has-text("' + label + '")');
      await drawn(ox, "range=" + range + "&repo=archive");
      older.ranges[range] = { started: (await ox.locator(".analytics-metric").filter({ hasText: "Sessions started" }).locator(".value").textContent()).trim(), row: await ox.locator(archiveRow).count(), heading: await ox.locator(".page .sub").textContent() };
    }
  }
  older.errors = ox.errors;
  await ox.context().close();
  r.expect(older.inModel === false, "the extras fixture's archive session must be older than the model's day: " + JSON.stringify(older));
  r.expect(older.rowWithoutFilter === 1 && older.option, "7 d must list archive's repo as a breakdown row and a Repo filter value: " + JSON.stringify(older));
  r.expect(older.ranges["7d"]?.started === "1" && older.ranges["30d"]?.started === "1" && older.ranges["24h"]?.started === "0", "Sessions started for archive must be 1 in 7 d and 30 d, 0 in 24 h: " + JSON.stringify(older.ranges));
  r.expect(older.ranges["7d"]?.row === 1 && older.ranges["30d"]?.row === 1 && older.ranges["24h"]?.row === 0, "archive's repo row must show in 7 d and 30 d only: " + JSON.stringify(older.ranges));
  r.expect(older.errors.length === 0, "older session: page errors: " + older.errors.join(" | "));

  // A request for the range that fails (a 500, then a 409) leaves the last answer drawn under "Couldn't update Analytics…",
  // and while it fails the page asks again only every 10 s, however fast the model moves. An answer that works again (a
  // 304, then a 200) clears the message.
  const failing = { steps: {} };
  const fp = await served(browser, { size: "desktop", path: "/analytics" });
  await drawn(fp, "range=7d");
  await fp.click('#topbar .analytics-range button:has-text("30 d")'); await drawn(fp, "range=30d");
  let mode = "real", asks = 0, updates = 0, fakeVersion = 0;
  fp.on("request", (q) => { if (new URL(q.url()).pathname === "/api/analytics") asks++; });
  await fp.route((u) => u.pathname === "/api/analytics", async (route) => {
    if (mode === "real") return route.continue(); // with the page's If-None-Match: the server's 304 for a kept answer
    if (mode === "fresh") { // a 200 with a new tag, whatever the page holds
      const headers = { ...route.request().headers() }; delete headers["if-none-match"];
      const res = await route.fetch({ headers });
      return route.fulfill({ status: 200, headers: { "content-type": "application/json", etag: '"fresh-' + Date.now() + '"' }, body: await res.text() });
    }
    return route.fulfill({ status: mode === "conflict" ? 409 : 500, contentType: "text/plain", body: "no" });
  });
  const shown = () => fp.evaluate(() => ({ query: document.querySelector(".analytics-metrics[data-analytics-ready]")?.dataset.query ?? null, figures: document.querySelectorAll(".analytics-metric").length, message: [...document.querySelectorAll("#page p.empty")].map((p) => p.textContent).find((t) => t.startsWith("Couldn't update Analytics")) ?? null }));
  const settle = () => fp.waitForTimeout(400);
  // 500: 7 d is kept, so it is drawn at once, and the failure puts the message over it.
  mode = "error"; await fp.click('#topbar .analytics-range button:has-text("7 d")');
  await fp.waitForFunction(() => [...document.querySelectorAll("#page p.empty")].some((p) => p.textContent.startsWith("Couldn't update Analytics: 500")), null, { timeout: 5000 }).catch(() => {});
  failing.steps.error500 = await shown();
  // Model updates every poll (each a new version) while it fails: no new ask for 7 s.
  const modelUrl = (u) => u.pathname === "/api/model";
  await fp.route(modelUrl, async (route) => {
    // Without `since` the server answers the whole model (never a 304), which goes out under a new version.
    const url = new URL(route.request().url()); url.searchParams.delete("since");
    const res = await route.fetch({ url: url.toString() }); if (res.status() !== 200) return route.fulfill({ response: res });
    const m = await res.json(); m.version = "fake-" + ++fakeVersion; updates++;
    return route.fulfill({ status: 200, headers: { "content-type": "application/json" }, body: JSON.stringify(m) });
  });
  const asked0 = asks; await fp.waitForTimeout(6000);
  failing.spacing = { asks: asks - asked0, modelUpdates: updates };
  await fp.unroute(modelUrl);
  failing.steps.during = await shown();
  // 409, on 30 d (kept): the reader's click asks at once.
  mode = "conflict"; await fp.click('#topbar .analytics-range button:has-text("30 d")');
  await fp.waitForFunction(() => [...document.querySelectorAll("#page p.empty")].some((p) => p.textContent.startsWith("Couldn't update Analytics: 409")), null, { timeout: 5000 }).catch(() => {});
  failing.steps.error409 = await shown();
  // 304: back on 7 d, the server answers the kept tag, and the message goes.
  mode = "real"; await fp.click('#topbar .analytics-range button:has-text("7 d")'); await settle();
  await fp.waitForFunction(() => ![...document.querySelectorAll("#page p.empty")].some((p) => p.textContent.startsWith("Couldn't update Analytics")), null, { timeout: 5000 }).catch(() => {});
  failing.steps.after304 = await shown();
  // Fails again on 30 d, then a 200 on 7 d clears it.
  mode = "error"; await fp.click('#topbar .analytics-range button:has-text("30 d")');
  await fp.waitForFunction(() => [...document.querySelectorAll("#page p.empty")].some((p) => p.textContent.startsWith("Couldn't update Analytics: 500")), null, { timeout: 5000 }).catch(() => {});
  failing.steps.error500again = await shown();
  mode = "fresh"; await fp.click('#topbar .analytics-range button:has-text("7 d")'); await settle();
  await fp.waitForFunction(() => ![...document.querySelectorAll("#page p.empty")].some((p) => p.textContent.startsWith("Couldn't update Analytics")), null, { timeout: 5000 }).catch(() => {});
  failing.steps.after200 = await shown();
  failing.errors = fp.errors;
  await fp.context().close();
  const kept = (step, query) => step?.query === query && step.figures === 8;
  r.expect(kept(failing.steps.error500, "range=7d") && failing.steps.error500.message?.includes("500") && failing.steps.error500.message.includes("Showing the last answer"), "a 500 must leave the last 7 d answer drawn under the message: " + JSON.stringify(failing.steps.error500));
  r.expect(failing.spacing.modelUpdates >= 2 && failing.spacing.asks === 0 && kept(failing.steps.during, "range=7d") && failing.steps.during.message, "while it fails, model updates must not ask again within 10 s: " + JSON.stringify({ spacing: failing.spacing, during: failing.steps.during }));
  r.expect(kept(failing.steps.error409, "range=30d") && failing.steps.error409.message?.includes("409"), "a 409 must leave the last 30 d answer drawn under the message: " + JSON.stringify(failing.steps.error409));
  r.expect(kept(failing.steps.after304, "range=7d") && failing.steps.after304.message === null, "a 304 must clear the message: " + JSON.stringify(failing.steps.after304));
  r.expect(kept(failing.steps.error500again, "range=30d") && failing.steps.error500again.message && kept(failing.steps.after200, "range=7d") && failing.steps.after200.message === null, "a 200 must clear the message: " + JSON.stringify({ before: failing.steps.error500again, after: failing.steps.after200 }));
  r.expect(failing.errors.length === 0, "failing answers: page errors: " + failing.errors.join(" | "));

  // Every range, at 390 and 1280, light and dark: out/analytics/<scheme>-<range>.png.
  const shots = path.join(ENV.out, "analytics"); fs.mkdirSync(shots, { recursive: true });
  const shotErrors = [];
  for (const [scheme, size, dark] of [["390-light", "phone", false], ["390-dark", "phone", true], ["1280-light", "desktop", false], ["1280-dark", "desktop", true]]) {
    const page = await served(browser, { size, dark, path: "/analytics" });
    await drawn(page, "range=7d");
    for (const [label, range] of [["24 h", "24h"], ["7 d", "7d"], ["30 d", "30d"]]) {
      await page.click('#topbar .analytics-range button:has-text("' + label + '")');
      await drawn(page, "range=" + range);
      await page.evaluate(() => document.fonts.ready);
      await page.screenshot({ path: path.join(shots, scheme + "-" + range + ".png"), fullPage: size === "phone" });
    }
    shotErrors.push(...page.errors);
    await page.context().close();
  }
  r.expect(shotErrors.length === 0, "range shots: page errors: " + shotErrors.join(" | "));

  const legacy = await served(browser, { size: "desktop", path: "/timeline" });
  const oldRoute = await legacy.evaluate(() => ({ state: history.state?.v, path: location.pathname, title: document.querySelector("#topbar .t")?.textContent }));
  const legacyErrors = legacy.errors;
  await legacy.context().close();

  r.results = { modes, oldRoute, costTwoDays, older, failing };
  for (const m of modes) {
    r.expect(m.errors.length === 0, m.mode + ": page errors: " + m.errors.join(" | "));
    r.expect(m.range.labels.join(",") === "24 h,7 d,30 d" && m.range30.selected === "30 d" && m.range30.heading?.includes("Last 30 days"), m.mode + ": Analytics range control did not change the selected range: " + JSON.stringify({ before: m.range, after: m.range30 }));
    r.expect(m.figures === 8 && expectedLabels.every((label) => m.labels.some((value) => value.startsWith(label))), m.mode + ": expected eight headline figures: " + JSON.stringify(m.labels));
    r.expect(m.longestCurrentWait === "8m", m.mode + ": completed result messages were counted as current waits: " + m.longestCurrentWait);
    r.expect(m.costHeadline === m.costExpected, m.mode + ": Analytics headline did not sum the served by_day cost: " + JSON.stringify({ actual: m.costHeadline, expected: m.costExpected }));
    r.expect(m.charts.length === 2 && m.charts.every((chart) => chart.columns > 0), m.mode + ": expected both charts to render vertical columns: " + JSON.stringify(m.charts));
    r.expect(m.costExpected === "—" || Number(m.costHeadline.replace("$", "")) > 0, m.mode + ": the fixture's cost is all on today, which the 7 d figure must include: " + m.costHeadline);
    r.expect(m.slice?.open && m.slice.sessions > 0, m.mode + ": an Analytics slice did not open with busy sessions: " + JSON.stringify(m.slice));
    if (m.mode === "phone") r.expect(m.slice?.bottomSheet && m.sideways === 0, "phone: the slice must be a bottom sheet with no sideways page scroll: " + JSON.stringify({ slice: m.slice, sideways: m.sideways }));
    r.expect(m.repoRow === 1 && m.filtered?.title === "Sessions" && m.filtered.rows.length === expectedHarborIds.length && m.filtered.rows.join(",") === expectedHarborIds.join(",") && m.filtered.rows.length < Object.keys(D.SESS).length, m.mode + ": a repo breakdown did not filter the Sessions list to every harbor session: " + JSON.stringify({ found: m.repoRow, expected: expectedHarborIds, filtered: m.filtered }));
    r.expect(m.costMode?.pressed === "API-equivalent cost" && m.costMode.costRows > 0 && m.costMode.hoursRows === 0, m.mode + ": the cost measure did not switch breakdown columns: " + JSON.stringify(m.costMode));
    r.expect(m.allowance.heading === "Codex allowance" && m.allowance.windows.includes("5-hour window") && m.allowance.windows.includes("Weekly window") && m.allowance.used.length === 2, m.mode + ": served rate limits did not render both allowance windows: " + JSON.stringify(m.allowance));
  }
  r.expect(legacyErrors.length === 0 && oldRoute.state === "analytics" && oldRoute.path === "/analytics" && oldRoute.title === "Analytics", "the old /timeline URL did not resolve to /analytics: " + JSON.stringify({ ...oldRoute, errors: legacyErrors }));
  return r.done();
}

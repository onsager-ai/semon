// Analytics: range, eight headline figures, stacked column charts, slice drill-in, breakdown filtering, cost measure,
// phone width and the served Codex allowance. This replaces the retired Timeline check.
import { served, data, reporter, overflow, goto } from "../lib.mjs";

export default async function analyticsCheck(browser) {
  const D = await data(), r = reporter("analytics"), modes = [];
  const expectedLabels = ["Agent-hours", "API-equivalent cost", "Sessions started", "Turns", "Tool calls", "Peak concurrency", "Waited on you", "Longest current wait"];
  r.expect(!!D.SESS.deps?.rate_limits, "the fixture must serve Codex rate limits for the allowance panel");

  for (const [mode, size, dark] of [["phone", "phone", false], ["desktop", "desktop", false]]) {
    const page = await served(browser, { size, dark, path: "/analytics" });
    await goto(page, { v: "analytics" }, D);
    const record = { mode };
    record.range = await page.evaluate(() => ({ labels: [...document.querySelectorAll("#topbar .analytics-range button")].map((b) => b.textContent.trim()), selected: document.querySelector('#topbar .analytics-range button[aria-pressed="true"]')?.textContent.trim(), heading: document.querySelector(".page .sub")?.textContent }));
    await page.click('#topbar .analytics-range button:has-text("30 d")');
    record.range30 = await page.evaluate(() => ({ selected: document.querySelector('#topbar .analytics-range button[aria-pressed="true"]')?.textContent.trim(), heading: document.querySelector(".page .sub")?.textContent }));
    await page.click('#topbar .analytics-range button:has-text("7 d")');
    record.labels = await page.evaluate(() => [...document.querySelectorAll(".analytics-metric .label")].map((x) => x.textContent.trim()));
    record.figures = await page.locator(".analytics-metric").count();
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
    record.filtered = await page.evaluate(() => ({ title: document.querySelector(".page h1")?.textContent, repo: document.querySelector('.facet-field select[aria-label="Repo"]')?.value, rows: [...document.querySelectorAll(".page .nrow")].map((x) => x.dataset.id) }));
    modes.push({ ...record, errors: page.errors });
    await page.context().close();
  }

  const legacy = await served(browser, { size: "desktop", path: "/timeline" });
  const oldRoute = await legacy.evaluate(() => ({ state: history.state?.v, path: location.pathname, title: document.querySelector("#topbar .t")?.textContent }));
  const legacyErrors = legacy.errors;
  await legacy.context().close();

  r.results = { modes, oldRoute };
  for (const m of modes) {
    r.expect(m.errors.length === 0, m.mode + ": page errors: " + m.errors.join(" | "));
    r.expect(m.range.labels.join(",") === "24 h,7 d,30 d" && m.range30.selected === "30 d" && m.range30.heading?.includes("Last 30 days"), m.mode + ": Analytics range control did not change the selected range: " + JSON.stringify({ before: m.range, after: m.range30 }));
    r.expect(m.figures === 8 && expectedLabels.every((label) => m.labels.some((value) => value.startsWith(label))), m.mode + ": expected eight headline figures: " + JSON.stringify(m.labels));
    r.expect(m.charts.length === 2 && m.charts.every((chart) => chart.columns > 0), m.mode + ": expected both charts to render vertical columns: " + JSON.stringify(m.charts));
    r.expect(m.slice?.open && m.slice.sessions > 0, m.mode + ": an Analytics slice did not open with busy sessions: " + JSON.stringify(m.slice));
    if (m.mode === "phone") r.expect(m.slice?.bottomSheet && m.sideways === 0, "phone: the slice must be a bottom sheet with no sideways page scroll: " + JSON.stringify({ slice: m.slice, sideways: m.sideways }));
    r.expect(m.repoRow === 1 && m.filtered?.title === "Sessions" && m.filtered.repo === "harbor" && m.filtered.rows.length > 0 && m.filtered.rows.every((id) => D.SESS[id]?.repo === "harbor"), m.mode + ": a repo breakdown did not filter the Sessions list: " + JSON.stringify({ found: m.repoRow, filtered: m.filtered }));
    r.expect(m.costMode?.pressed === "API-equivalent cost" && m.costMode.costRows > 0 && m.costMode.hoursRows === 0, m.mode + ": the cost measure did not switch breakdown columns: " + JSON.stringify(m.costMode));
    r.expect(m.allowance.heading === "Codex allowance" && m.allowance.windows.includes("5-hour window") && m.allowance.windows.includes("Weekly window") && m.allowance.used.length === 2, m.mode + ": served rate limits did not render both allowance windows: " + JSON.stringify(m.allowance));
  }
  r.expect(legacyErrors.length === 0 && oldRoute.state === "analytics" && oldRoute.path === "/analytics" && oldRoute.title === "Analytics", "the old /timeline URL did not resolve to /analytics: " + JSON.stringify({ ...oldRoute, errors: legacyErrors }));
  return r.done();
}

// Regressions for the viewer backlog: URL search, shared rows, permission inbox,
// sheet scroll preservation and trace navigation.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { ENV, launch, served, data, goto } from "../lib.mjs";

const ready = (page, view) => page.waitForFunction((v) => history.state?.v === v && !document.querySelector('#page[aria-busy="true"]') && document.querySelector("#topbar .t"), view);
const closePage = async (page) => { await page.unrouteAll({ behavior: "ignoreErrors" }); await page.context().close(); };

export default async function viewerBacklog(browser) {
  const D = await data();
  for (const size of ["phone", "desktop"]) {
    const page = await served(browser, { size });
    try {
      await ready(page, "home");
      await page.evaluate(() => document.querySelector('.nav-item[data-go="sessions"]').click()); await ready(page, "sessions");
      await page.locator("#sq").fill("harbor");
      assert.equal(new URL(page.url()).searchParams.get("q"), "harbor");
      await page.locator("#sq").fill("");
      assert.equal(new URL(page.url()).searchParams.has("q"), false);
      await page.reload(); await ready(page, "sessions");
      assert.equal(await page.locator("#sq").inputValue(), "");
      await page.waitForTimeout(100);
      const rows = await page.locator('[data-session-row="list"]').evaluateAll((rows) => rows.map((row) => {
        const meta = row.querySelector(".session-row-meta");
        return { main: !!row.querySelector(".session-row-main"), model: meta.querySelector(".row-model").textContent, fits: meta.scrollWidth <= meta.clientWidth + 1, items: [...meta.children].filter((x) => !x.hidden).map((x) => ({ cls: x.className, cut: x.scrollWidth > x.clientWidth + 1 })) };
      }));
      assert(rows.length > 0 && rows.every((x) => x.main && x.model && x.fits && x.items.every((v) => !v.cut)), size + ": metadata must drop complete values");
      assert(await page.locator('#lanes [data-session-row="compact"] .session-row-main').count() > 0);
      if (size === "phone") {
        await goto(page, { v: "analytics" }, D);
        await page.waitForSelector("#page [data-analytics-ready]");
        await page.locator(".chart-slices summary").first().click();
        const slices = page.locator(".chart-slices").first().locator(".chart-slice");
        assert(await slices.count() > 0, "touch chart has equivalent slice targets");
        assert(await slices.evaluateAll((buttons) => buttons.every((b) => b.getBoundingClientRect().height >= 44)), "slice targets are finger-sized");
        await slices.first().click();
        await page.waitForSelector("dialog[open]");
        await page.keyboard.press("Escape");
        await page.waitForFunction(() => !document.querySelector("dialog[open]") && !history.state?.sheet);
      }
      await goto(page, { v: "sessions" }, D);
      for (const [close, label, value] of [["escape", "Harness", "claude"], ["back", "Repo", "harbor"]]) {
        await page.locator(".facet-btn").click();
        const select = page.locator('.filters-sheet .sh-select[data-label="' + label + '"]');
        await select.locator(".sh-select-trigger").click();
        await select.locator('[role="option"][data-value="' + value + '"]').click();
        if (close === "escape") await page.keyboard.press("Escape"); else await page.goBack();
        await page.waitForFunction(() => !document.querySelector("dialog[open]") && !history.state?.sheet);
        const chip = page.locator('.facet-chip[data-facet="' + label.toLowerCase() + '"]');
        await chip.waitFor({ state: "visible" });
        assert((await chip.getAttribute("aria-label")).includes((await chip.textContent()).trim()), "chip accessible name contains its visible text");
        await chip.click();
      }
      const sid = Object.keys(D.TX).find((sid) => D.TX[sid].length > 20) ?? Object.keys(D.SESS)[0];
      await goto(page, { v: "session", id: sid }, D);
      if (size === "phone") {
        for (const close of ["button", "escape", "back"]) {
          await page.evaluate(() => window.scrollTo(0, Math.min(400, document.documentElement.scrollHeight - innerHeight)));
          const before = await page.evaluate(() => scrollY);
          await page.locator("#more-btn").click();
          await page.waitForTimeout(50);
          assert.equal(await page.evaluate(() => scrollY), before, "opening details must preserve transcript scroll");
          if (close === "button") await page.locator(".session-menu .panel-h button").click();
          else if (close === "escape") await page.keyboard.press("Escape");
          else await page.goBack();
          await page.waitForFunction(() => !document.querySelector("dialog[open]") && !history.state?.sheet);
          await page.waitForTimeout(100);
          assert.equal(await page.evaluate(() => scrollY), before, close + " must restore the same scroll");
        }
      }
      const trace = D.turns.find((t) => t.sent.length && D.SESS[t.sid]);
      assert(trace, "fixture needs an outgoing trace");
      await goto(page, { v: "session", id: trace.sid, turn: trace.id }, D);
      await page.locator('#page .turn[data-turn="' + trace.id + '"]').scrollIntoViewIfNeeded();
      await page.locator("#more-btn").click();
      const traceIcon = await page.locator(".menu-trace > svg").boundingBox();
      assert(traceIcon && traceIcon.width === 18 && traceIcon.height === 18, "the Trace menu icon must match the compact action icons");
      await page.locator(".menu-trace").click();
      await ready(page, "trace");
      assert.equal(await page.locator("#topbar .t").textContent(), "Trace");
      assert.equal(await page.locator("#topbar #more-btn").count(), 1);
      assert.equal(await page.locator("#topbar .trace-back").count(), 1);
      assert.equal(await page.locator("#page .trace-summary").count(), 1);
      assert.equal(await page.locator("#topbar .meta-line").count(), 0);
      assert.equal(page.errors.length, 0, page.errors.join(" | "));
    } finally { await closePage(page); }
  }

  // A pid permission wait with no handoff must still appear once on Home.
  const page = await served(browser, { size: "desktop" });
  try {
    const model = structuredClone(D.model), sid = Object.keys(model.sessions).find((id) => model.sessions[id].lane);
    const versions = Object.keys(model.sessions).slice(0, 2); model.sessions[versions[0]].model = "gpt-6-sol"; model.sessions[versions[1]].model = "gpt-6.1-sol";
    model.sessions[sid].state = "wait"; model.sessions[sid].waiting_for = "permission prompt";
    model.handoffs = model.handoffs.filter((h) => h.from !== sid || h.status !== "wait"); model.version += "-wait";
    await page.route("**/api/model**", (r) => r.fulfill({ json: model }));
    await page.reload(); await ready(page, "home");
    assert.equal(await page.locator('[data-h="wait:' + sid + '"]').count(), 1);
    assert.match(await page.locator('[data-h="wait:' + sid + '"]').textContent(), /permission prompt/);
    await goto(page, { v: "sessions" }, D);
    assert.equal(await page.locator('.nrow[data-id="' + versions[0] + '"] .row-model').textContent(), "sol");
    assert.equal(await page.locator('.nrow[data-id="' + versions[1] + '"] .row-model').textContent(), "sol 6.1");
  } finally { await closePage(page); }

  console.log("viewer backlog regressions passed");
  return true;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  try { await viewerBacklog(browser); } finally { await browser.close(); }
}

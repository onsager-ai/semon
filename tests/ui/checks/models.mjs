// Models compare recorded work within a band, expose each metric's sample count,
// and keep missing facts unknown instead of substituting session guesses.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { ENV, launch, served, data, goto, overflow } from "../lib.mjs";

export default async function models(browser) {
  const D = await data(), sid = Object.keys(D.SESS)[0];
  for (const size of ["phone", "desktop"]) {
    const page = await served(browser, { size });
    try {
      await page.waitForFunction(() => history.state?.v === "home");
      const actual = await (await page.request.get(ENV.base + "/api/analytics?range=7d")).json();
      const item = { sid, machine: D.SESS[sid].machine, harness: D.SESS[sid].harness, band: "easy", models: ["gpt-6-sol", "gpt-6.1-sol"], cost_usd: 2, pr_url: "https://github.com/onsager-ai/semon/pull/1" };
      const known = { model: "gpt-6-sol", band: "easy", n: 2, small_sample: true, first_pass_acceptance: .5, acceptance_n: 2, median_review_rounds: 1, review_rounds_n: 2, median_red_ci_heads: 0, red_ci_n: 2, median_model_ms: 120000, model_time_n: 2, median_cost_usd: 2, cost_n: 2, allowance_per_million_input: null, allowance_n: 0, tokens: { input: 100, output: 50, cache_read: 10, cache_write: 20 }, tokens_n: 2, items: [item], items_more: 0 };
      const unknown = { ...known, model: "gpt-6.1-sol", band: "hard", n: 1, first_pass_acceptance: null, acceptance_n: 0, median_cost_usd: null, cost_n: 0, items: [{ ...item, cost_usd: null, band: "hard" }] };
      const answer = { ...actual, models: { groups: [known, unknown], unknown_reasons: { acceptance: "Review facts were not recorded", allowance: "Allowance delta was not recorded", cost: "No public model price" } } };
      await page.route("**/api/analytics?**", (route) => route.fulfill({ json: answer }));
      await goto(page, { v: "analytics" }, D);
      await page.waitForSelector(".models-panel");
      assert.equal(await page.locator(".models-panel .model-table tbody tr").count(), 2);
      assert.equal(await page.locator(".models-panel .model-n").count(), 14);
      const easy = page.locator('.model-table-scroll[aria-label="easy model comparison"]');
      assert.match(await easy.textContent(), /50%/);
      assert.match(await easy.textContent(), /small sample/);
      assert.match(await easy.textContent(), /100 \/ 50 \/ 30/);
      const hard = page.locator('.model-table-scroll[aria-label="hard model comparison"]');
      assert.match(await hard.textContent(), /Unknown/);
      assert.equal(await page.locator(".models-panel .model-point").count(), 1, "unknown acceptance or cost must not become a plot point");
      assert.equal(await overflow(page), 0, "model table must scroll inside its region");
      await easy.locator(".model-comparison-open").click();
      await page.waitForSelector("dialog[open]");
      assert.match(await page.locator("dialog[open]").textContent(), /sol → sol 6.1/);
      assert.equal(await page.locator('dialog[open] a[href="https://github.com/onsager-ai/semon/pull/1"]').count(), 1);
      await page.locator("dialog[open] .model-item-session").click();
      await page.waitForFunction((id) => history.state?.v === "session" && history.state.id === id && !document.querySelector('#page[aria-busy="true"]'), sid);
      assert.equal(page.errors.length, 0, page.errors.join(" | "));
    } finally { await page.unrouteAll({ behavior: "ignoreErrors" }); await page.context().close(); }
  }
  console.log("Models comparison browser regressions passed"); return true;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch(); try { await models(browser); } finally { await browser.close(); }
}

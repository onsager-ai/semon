import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { launch, served, data, goto } from "../lib.mjs";

export default async function stepNames(browser) {
  const D = await data(), sid = Object.keys(D.TX).find((id) => D.TX[id].some((e) => e.k === "tool"));
  assert(sid, "fixture needs a command step");
  for (const live of [true, false]) {
    const page = await served(browser, { size: "desktop" });
    try {
      await page.waitForFunction(() => history.state?.v === "home");
      await page.route("**/api/tx?**", async (route) => {
        if (new URL(route.request().url()).searchParams.get("sid") !== sid) return route.continue();
        const response = await route.fetch(), tx = await response.json(), tool = tx.entries.find((e) => e.k === "tool");
        Object.assign(tool, { title: "Review command output", live, secs: "2s", ok: false, unfinished: false, exit: null });
        await route.fulfill({ json: tx });
      });
      await goto(page, { v: "session", id: sid }, D);
      const button = page.locator(".step > button").filter({ hasText: "Review command output" });
      const name = (await button.textContent()).replace(/\s+/g, " ");
      assert.match(name, live ? /Running:.*Review command output.*2s/ : /Failed:.*Review command output.*failed.*2s/);
      assert.equal(await button.getAttribute("aria-label"), null, "a fixed label must not hide changing status");
      assert.equal(page.errors.length, 0, page.errors.join(" | "));
    } finally { await page.unrouteAll({ behavior: "ignoreErrors" }); await page.context().close(); }
  }
  console.log("Step accessible-name regressions passed"); return true;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch(); try { await stepNames(browser); } finally { await browser.close(); }
}

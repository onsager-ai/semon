import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { launch, served, data, goto } from "../lib.mjs";
export default async function signalsCheck(browser) {
  const D = await data(), sid = Object.keys(D.TX).find((id) => D.TX[id].some((e) => e.k === "tool"));
  for (const size of ["phone", "desktop"]) {
    const page = await served(browser, { size });
    try {
      const model = structuredClone(D.model); model.sessions[sid].signals = {compact:1,effort:1}; model.version += "-signals";
      await page.route("**/api/model**", r => r.fulfill({json:model}));
      await page.route("**/api/tx?**", async r => {
        const q = new URL(r.request().url()).searchParams;
        if (q.get("sid") !== sid) return r.continue();
        if (q.has("signals")) return r.fulfill({json:{version:model.version,signals:2,slots:[1,2]}});
        const response = await r.fetch(), tx = await response.json(), tool = tx.entries.find(e=>e.k==="tool");
        tx.entries = [{...tool,slot:0}, {k:"signal",slot:1,signal:{kind:"compact",value:180000},at:tool.at}, {k:"signal",slot:2,signal:{kind:"effort",previous:"high",tag:"max"},at:tool.at}, {...tool,slot:3}];
        tx.from=0; tx.to=4; tx.total=4;
        await r.fulfill({json:tx});
      });
      await page.reload(); await page.waitForFunction(()=>history.state?.v==="home");
      await goto(page,{v:"session",id:sid},D);
      assert.equal(await page.locator(".signal-marker").count(),2);
      assert.match(await page.locator(".signal-marker").first().textContent(),/180k tokens before/);
      assert.match(await page.locator(".signal-marker").last().textContent(),/Effort high → max/);
      // Annotation rows precede a gathered run rather than breaking it in two.
      assert.equal(await page.locator(".tgroup").count(),1);
      await page.locator("#more-btn").click();
      assert.match(await page.locator(".session-menu .kv").textContent(),/Signals\s*·\s*compact1/);
      await page.locator(".menu-signals").click();
      await page.waitForSelector(".signal-marker.err-current");
      assert.equal(await page.locator(".errnav-count").textContent(),"Signal 1 of 2");
      await page.getByRole("button",{name:"Next signal",exact:true}).click();
      await page.waitForFunction(()=>document.querySelector(".errnav-count")?.textContent==="Signal 2 of 2");
      assert.match(await page.locator(".signal-marker.err-current").textContent(),/Effort/);
      await page.getByRole("button",{name:"Close signals",exact:true}).click();
      assert.equal(await page.locator(".errnav-bar").count(),0);
      await page.locator("#find-btn").click();
      await page.locator('.chip[data-filter="signals"]').click();
      await page.waitForSelector(".signal-marker.err-current");
      assert.equal(page.errors.length,0,page.errors.join(" | "));
    } finally {await page.unrouteAll({behavior:"ignoreErrors"});await page.context().close();}
  }
  console.log("Signal marker and stepper regressions passed");return true;
}
if(process.argv[1]===fileURLToPath(import.meta.url)){const b=await launch();try{await signalsCheck(b);}finally{await b.close();}}
